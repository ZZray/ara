//! Real RPC children for fixed OMP 596f2da user Bash / abort_bash.
//! File gates establish process and Session ownership order without timing
//! guesses. Every case shares one deadline, including shutdown and reopen.

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const CASE_BOUND: Duration = Duration::from_secs(15);

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
    deadline: Instant,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-bash-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-bash-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions, deadline: Instant::now() + CASE_BOUND }
    }

    fn command(&self, upstream: &FakeUpstream, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        for key in [
            "OPENROUTER_API_KEY",
            "ANTHROPIC_API_KEY",
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ARA_MODEL",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "OPENROUTER_BASE_URL",
            "ARA_TEST_MODEL_ID",
            "CLAUDE_CONFIG_DIR",
            "COPILOT_HOME",
            "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            "WSL_DISTRO_NAME",
            "WSL_INTEROP",
        ] {
            command.env_remove(key);
        }
        command
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .env("ARA_API_KEY", "sk-user-bash-fixture-only")
            .args(["--mode", "rpc", "--model", "fake-model", "--base-url", &upstream.base_url(), "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(["--compact-threshold", "0"])
            .args(args);
        command
    }

    fn wait_marker(&self, name: &str) {
        let path = self.work.path().join(name);
        while !path.is_file() {
            assert!(Instant::now() < self.deadline, "process never reached marker {}", path.display());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn release(&self, name: &str) {
        std::fs::write(self.work.path().join(format!("{name}.release")), "release").unwrap();
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Result<Value, String>>,
    seen: Vec<Value>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    deadline: Instant,
}

impl RpcChild {
    fn spawn(env: &Env, upstream: &FakeUpstream, args: &[&str]) -> Self {
        let mut child = env
            .command(upstream, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (tx, frames) = mpsc::channel();
        let output_reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let frame = line
                    .map_err(|error| error.to_string())
                    .and_then(|line| serde_json::from_str(&line).map_err(|error| format!("{error}: {line}")));
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        let error_reader = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(length) = stderr_pipe.read(&mut bytes) {
                if length == 0 {
                    break;
                }
                captured.lock().unwrap().push_str(&String::from_utf8_lossy(&bytes[..length]));
            }
        });
        Self {
            child,
            stdin,
            frames,
            seen: Vec::new(),
            stderr,
            readers: vec![output_reader, error_reader],
            deadline: env.deadline,
        }
    }

    fn send(&mut self, frame: Value) {
        self.send_many(&[frame]);
    }

    fn send_many(&mut self, frames: &[Value]) {
        let stdin = self.stdin.as_mut().unwrap();
        for frame in frames {
            writeln!(stdin, "{frame}").unwrap();
        }
        stdin.flush().unwrap();
    }

    fn next(&mut self) -> Value {
        match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(frame)) => {
                self.seen.push(frame.clone());
                frame
            }
            result => {
                panic!("RPC wait failed: {result:?}; stderr: {}; frames: {:?}", self.stderr.lock().unwrap(), self.seen)
            }
        }
    }

    fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let frame = self.next();
            if matches(&frame) {
                return frame;
            }
        }
    }

    fn response(&mut self, id: &str) -> Value {
        // Parallel Bash jobs can finish in either order. Keep already-read
        // replies rather than losing the other job while waiting for this ID.
        if let Some(frame) = self.seen.iter().find(|frame| frame["type"] == "response" && frame["id"] == id) {
            return frame.clone();
        }
        self.until(|frame| frame["type"] == "response" && frame["id"] == id)
    }

    fn success(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }

    fn ready(&mut self) {
        assert_eq!(self.next()["type"], "ready");
        let metadata = self.next();
        assert_eq!(metadata["type"], "available_commands_update");
        assert_eq!(metadata["commands"], json!([]));
    }

    fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"}));
        self.success(id)
    }

    fn messages(&mut self, id: &str) -> Vec<Value> {
        self.send(json!({"id":id,"type":"get_messages"}));
        self.success(id)["messages"].as_array().unwrap().clone()
    }

    fn prompt(&mut self, id: &str, message: &str) {
        self.send(json!({"id":id,"type":"prompt","message":message}));
        self.success(id);
    }

    fn run(&mut self, id: &str, message: &str) {
        self.prompt(id, message);
        self.until(|frame| frame["type"] == "agent_end");
    }

    fn bash(&mut self, id: &str, command: &str) {
        self.send(json!({"id":id,"type":"bash","command":command}));
    }

    fn finish(&mut self) {
        self.stdin.take();
        loop {
            match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(frame)) => self.seen.push(frame),
                Ok(Err(error)) => panic!("{error}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("RPC EOF did not settle: {error}; {}", self.stderr.lock().unwrap()),
            }
        }
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < self.deadline, "RPC child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert_eq!(status.code(), Some(0), "{}", self.stderr.lock().unwrap());
        assert_eq!(self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn answer(message: &str) -> Value {
    json!({"events":[text(message),finish("stop"),done()]})
}

fn held_command(name: &str) -> String {
    format!(
        "printf 'effect:{name}' > {name}.effect; printf 'held:{name}\\n'; \
        printf started > {name}.started; \
        while [ ! -f {name}.release ]; do sleep 0.02; done; printf 'done:{name}\\n'"
    )
}

fn held_tree(name: &str) -> String {
    format!(
        "(printf child > {name}.child.started; \
        while [ ! -f {name}.child.release ]; do sleep 0.02; done; \
        printf orphan > {name}.orphan) & printf '%s' \"$!\" > {name}.child.pid; {}",
        held_command(name)
    )
}

fn dead_probe(names: &[&str]) -> String {
    // kill is Bash's builtin and understands the MSYS PID used by Git Bash.
    // Reaping may trail process termination, so poll the actual child state.
    let probes = names
        .iter()
        .map(|name| {
            format!(
                "pid=$(cat {name}.child.pid); dead=; \
            for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
            if ! kill -0 \"$pid\" 2>/dev/null; then dead=yes; break; fi; \
            if [ -r /proc/\"$pid\"/stat ] && \
            [ \"$(awk '{{print $3}}' /proc/\"$pid\"/stat)\" = Z ]; then dead=yes; break; fi; \
            sleep 0.02; done; [ \"$dead\" = yes ] || exit 9; printf '{name}:dead\\n';"
            )
        })
        .collect::<Vec<_>>();
    probes.join(" ")
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn bash_receipt<'a>(entries: &'a [Value], command: &str) -> &'a Value {
    let matches = entries
        .iter()
        .filter(|entry| entry["message"]["role"] == "bashExecution" && entry["message"]["command"] == command)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "one durable Bash receipt for {command}: {entries:?}");
    &matches[0]["message"]
}

fn has_bash(messages: &[Value], command: &str) -> bool {
    messages.iter().any(|message| message["role"] == "bashExecution" && message["command"] == command)
}

fn message_text(message: &Value) -> String {
    message["content"].as_str().map(str::to_owned).unwrap_or_else(|| {
        message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn request_has_projection(request: &Value, command: &str) -> bool {
    request["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "user" && message_text(message).contains(&format!("Ran `{command}`")))
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_user_bash_native_receipt_stats_projection_and_reopen() {
    let env = Env::new();
    let up = upstream(vec![answer("Receipt inspected")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    let nonce = env.work.path().file_name().unwrap().to_str().unwrap();
    let command = format!("printf '{nonce}' > nonce.txt; printf 'é😀\\nsecond\\n'; exit 7");
    child.bash("nonzero", &command);
    let result = child.success("nonzero");
    assert_eq!(std::fs::read_to_string(env.work.path().join("nonce.txt")).unwrap(), nonce);
    assert_eq!(
        result,
        json!({"output":"é😀\nsecond\n","exitCode":7,"cancelled":false,"truncated":false,
        "totalLines":3,"totalBytes":14,"outputLines":3,"outputBytes":14})
    );
    let native = child.messages("native");
    assert_eq!(native.len(), 1);
    assert_eq!(native[0]["role"], "bashExecution");
    assert_eq!(native[0]["command"], command);
    assert_eq!(native[0]["output"], result["output"]);
    assert_eq!(native[0]["exitCode"], 7);
    assert!(native[0].get("meta").is_none());
    let file = PathBuf::from(child.state("receipt-state")["sessionFile"].as_str().unwrap());
    // Fixed SessionManager keeps a Bash-only journal lazy until the first
    // assistant message (or an explicit forced Session creation).
    assert!(!file.exists());
    child.run("inspect", "inspect the user command receipt");
    assert_eq!(bash_receipt(&journal(&file), &command), &native[0]);
    let final_messages = child.messages("final-native");
    child.finish();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    assert_eq!(reopened.messages("reopened"), final_messages);
    reopened.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert!(request_has_projection(&requests[0], &command));
    let projection = requests[0]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message_text(message).contains("Ran `"))
        .unwrap();
    assert!(message_text(projection).contains("Command exited with code 7"));
    assert!(!requests[0]["body"]["messages"].to_string().contains("bashExecution"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_abort_bash_cancels_parallel_jobs_reaps_children_and_preserves_model_tool_token() {
    let env = Env::new();
    let builtin = held_command("builtin");
    let up = upstream(vec![
        json!({"events":[tool_call(0,"model-bash","bash",&json!({"command":builtin}).to_string()),finish("tool_calls"),done()]}),
        answer("Model Bash completed"),
    ]).await;
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "bash"]);
    child.ready();
    let first = held_tree("first");
    let second = held_tree("second");
    child.send_many(&[
        json!({"id":"first-job","type":"bash","command":first}),
        json!({"id":"second-job","type":"bash","command":second}),
    ]);
    for name in ["first.started", "second.started", "first.child.started", "second.child.started"] {
        env.wait_marker(name);
    }
    child.bash("independent", "printf independent");
    assert_eq!(child.success("independent")["cancelled"], false);
    child.prompt("model", "execute a model Bash independently of user Bash cancellation");
    env.wait_marker("builtin.started");
    child.send(json!({"id":"stop-user-bash","type":"abort_bash"}));
    child.success("stop-user-bash");
    for id in ["first-job", "second-job"] {
        let result = child.success(id);
        assert_eq!(result["cancelled"], true, "{result}");
        assert!(result["output"].as_str().unwrap().starts_with("[Command cancelled]\nheld:"));
        assert!(!result["output"].as_str().unwrap().contains("[Command aborted]"));
        assert!(result.get("timedOut").is_none());
    }
    for name in ["first", "second"] {
        assert_eq!(
            std::fs::read_to_string(env.work.path().join(format!("{name}.effect"))).unwrap(),
            format!("effect:{name}")
        );
        env.release(&format!("{name}.child"));
    }
    assert_eq!(child.state("model-still-running")["isStreaming"], true);
    env.release("builtin");
    child.until(|frame| frame["type"] == "agent_end");
    child.bash("dead-children", &dead_probe(&["first", "second"]));
    let dead = child.success("dead-children");
    assert_eq!(dead["exitCode"], 0, "{dead}");
    assert_eq!(dead["output"], "first:dead\nsecond:dead\n");
    assert!(!env.work.path().join("first.orphan").exists());
    assert!(!env.work.path().join("second.orphan").exists());
    let messages = child.messages("after-cancel");
    for command in [&first, &second] {
        assert!(has_bash(&messages, command));
        assert_eq!(messages.iter().find(|message| message["command"] == *command).unwrap()["cancelled"], true);
    }
    let tool = messages
        .iter()
        .find(|message| message["role"] == "toolResult" && message["toolCallId"] == "model-bash")
        .unwrap();
    assert_eq!(tool["isError"], false, "{tool}");
    assert!(message_text(tool).contains("done:builtin"));
    child.finish();
    assert_eq!(up.served(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_user_bash_new_and_switch_keep_starting_session_owner() {
    let env = Env::new();
    let up = upstream(vec![answer("Original"), answer("Fresh"), answer("Switched")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.run("origin-seed", "original context");
    let original = PathBuf::from(child.state("original")["sessionFile"].as_str().unwrap());
    let first = held_command("before-new");
    child.bash("before-new-job", &first);
    env.wait_marker("before-new.started");
    child.send(json!({"id":"new","type":"new_session"}));
    assert_eq!(child.success("new"), json!({"cancelled":false}));
    let fresh = PathBuf::from(child.state("fresh")["sessionFile"].as_str().unwrap());
    assert_ne!(fresh, original);
    env.release("before-new");
    assert_eq!(child.success("before-new-job")["cancelled"], false);
    assert_eq!(bash_receipt(&journal(&original), &first)["output"], "held:before-new\ndone:before-new\n");
    assert!(child.messages("fresh-empty").is_empty());
    child.run("fresh-prompt", "fresh new context");
    let second = held_command("before-switch");
    child.bash("before-switch-job", &second);
    env.wait_marker("before-switch.started");
    child.send(json!({"id":"switch","type":"switch_session","sessionPath":original}));
    assert_eq!(child.success("switch"), json!({"cancelled":false}));
    env.release("before-switch");
    assert_eq!(child.success("before-switch-job")["cancelled"], false);
    assert_eq!(bash_receipt(&journal(&fresh), &second)["output"], "held:before-switch\ndone:before-switch\n");
    assert!(!has_bash(&child.messages("switched-messages"), &second));
    child.run("switched-prompt", "switched original context");
    child.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(!request_has_projection(&requests[1], &first));
    assert!(!requests[1]["body"]["messages"].to_string().contains("original context"));
    assert!(!request_has_projection(&requests[2], &second));
    assert!(!requests[2]["body"]["messages"].to_string().contains("fresh new context"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_user_bash_branch_keeps_original_leaf_and_does_not_enter_fork_context() {
    let env = Env::new();
    let up = upstream(vec![answer("Retained"), answer("Selected"), answer("Fork")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.run("retained", "retained seed");
    child.run("selected", "selected excluded");
    let original = PathBuf::from(child.state("before-branch")["sessionFile"].as_str().unwrap());
    let entries = journal(&original);
    let leaf = entries.last().unwrap()["id"].clone();
    let selected = entries
        .iter()
        .find(|entry| entry["message"]["role"] == "user" && message_text(&entry["message"]) == "selected excluded")
        .unwrap()["id"]
        .clone();
    let command = held_command("before-branch");
    child.bash("branch-job", &command);
    env.wait_marker("before-branch.started");
    child.send(json!({"id":"branch","type":"branch","entryId":selected}));
    assert_eq!(child.success("branch"), json!({"text":"selected excluded","cancelled":false}));
    let fork = PathBuf::from(child.state("fork-state")["sessionFile"].as_str().unwrap());
    assert_ne!(fork, original);
    let fork_before = std::fs::read(&fork).unwrap();
    env.release("before-branch");
    assert_eq!(child.success("branch-job")["cancelled"], false);
    let original_entries = journal(&original);
    bash_receipt(&original_entries, &command);
    let receipt_entry = original_entries.iter().find(|entry| entry["message"]["command"] == command).unwrap();
    assert_eq!(receipt_entry["parentId"], leaf);
    assert_eq!(std::fs::read(&fork).unwrap(), fork_before);
    assert!(!has_bash(&child.messages("fork-native"), &command));
    child.run("fork-prompt", "continue fork");
    child.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(!request_has_projection(&requests[2], &command));
    let context = requests[2]["body"]["messages"].to_string();
    assert!(context.contains("retained seed") && context.contains("continue fork"));
    assert!(!context.contains("selected excluded"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_user_bash_same_file_reload_has_one_writer_and_failed_switch_keeps_owner() {
    let env = Env::new();
    let up = upstream(vec![answer("Seed"), answer("After reload"), answer("After failed switch")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.run("reload-seed", "seed before reload");
    let original = child.state("reload-origin");
    let file = PathBuf::from(original["sessionFile"].as_str().unwrap());
    let command = held_command("reload-held");
    child.bash("reload-job", &command);
    env.wait_marker("reload-held.started");
    let alias = file.parent().unwrap().join(".").join(file.file_name().unwrap());
    child.send(json!({"id":"reload","type":"switch_session","sessionPath":alias}));
    assert_eq!(child.success("reload"), json!({"cancelled":false}));
    child.run("after-reload", "new prompt after same file reload");
    let parent = journal(&file).last().unwrap()["id"].clone();
    child.send(json!({"id":"failed-switch","type":"switch_session",
        "sessionPath":env.home.path().join("missing.jsonl")}));
    assert_eq!(child.response("failed-switch")["success"], false);
    let state = child.state("failed-still-origin");
    assert_eq!(state["sessionId"], original["sessionId"]);
    assert_eq!(state["sessionFile"], original["sessionFile"]);
    env.release("reload-held");
    assert_eq!(child.success("reload-job")["cancelled"], false);
    let entries = journal(&file);
    let receipt = bash_receipt(&entries, &command).clone();
    assert_eq!(entries.iter().find(|entry| entry["message"] == receipt).unwrap()["parentId"], parent);
    assert!(entries.iter().any(|entry| message_text(&entry["message"]) == "new prompt after same file reload"));
    let messages = child.messages("reload-native");
    assert!(has_bash(&messages, &command));
    child.run("after-failure", "inspect late receipt after failed switch");
    let final_messages = child.messages("reload-final-native");
    child.finish();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    assert_eq!(reopened.messages("reload-reopened"), final_messages);
    reopened.finish();
    assert_eq!(bash_receipt(&journal(&file), &command), &receipt);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(request_has_projection(&requests[2], &command));
    assert!(requests[2]["body"]["messages"].to_string().contains("new prompt after same file reload"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_user_bash_returns_during_stream_but_flushes_native_context_after_run_join() {
    let env = Env::new();
    let up = upstream(vec![json!({"events":[text("Streaming gate held")],"end":"hang"}), answer("Next round")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.prompt("held-run", "hold the streaming response");
    child.until(|frame| {
        frame["type"] == "message_update" && message_text(&frame["message"]).contains("Streaming gate held")
    });
    let file = PathBuf::from(child.state("held-run-state")["sessionFile"].as_str().unwrap());
    let command = "printf stream-effect > stream-effect.txt; printf stream-output";
    child.bash("stream-job", command);
    assert_eq!(child.success("stream-job")["output"], "stream-output");
    assert_eq!(std::fs::read_to_string(env.work.path().join("stream-effect.txt")).unwrap(), "stream-effect");
    assert_eq!(child.state("still-held")["isStreaming"], true);
    assert!(!has_bash(&child.messages("before-join"), command));
    assert!(!file.exists(), "fixed OMP defers a new file until the partial assistant has settled");
    child.send(json!({"id":"join-held","type":"abort"}));
    child.success("join-held");
    assert_eq!(child.state("after-join-state")["isStreaming"], false);
    assert!(has_bash(&child.messages("after-join"), command));
    assert_eq!(bash_receipt(&journal(&file), command)["output"], "stream-output");
    child.run("next-round", "inspect safe flushed Bash context");
    child.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(!request_has_projection(&requests[0], command));
    assert!(request_has_projection(&requests[1], command));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_eof_drains_accepted_user_bash_reply_and_native_receipt_without_orphan() {
    let env = Env::new();
    let up = upstream(vec![answer("EOF durable seed")]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.run("eof-seed", "materialize the Session before held Bash");
    let mut expected_messages = child.messages("eof-seed-messages");
    let command = held_tree("eof");
    child.bash("eof-job", &command);
    env.wait_marker("eof.started");
    env.wait_marker("eof.child.started");
    let file = PathBuf::from(child.state("eof-state")["sessionFile"].as_str().unwrap());
    child.stdin.take();
    assert!(!env.work.path().join("eof.release").exists());
    env.release("eof");
    child.finish();
    let reply = child.seen.iter().find(|frame| frame["type"] == "response" && frame["id"] == "eof-job").unwrap();
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(reply["data"]["cancelled"], false);
    assert_eq!(reply["data"]["output"], "held:eof\ndone:eof\n");
    let receipt = bash_receipt(&journal(&file), &command).clone();
    assert_eq!(receipt["output"], reply["data"]["output"]);
    let reply_index = child.seen.iter().position(|frame| frame == reply).unwrap();
    let shutdown_index = child.seen.iter().position(|frame| frame["type"] == "session_shutdown").unwrap();
    assert!(reply_index < shutdown_index, "EOF must drain the owed Bash reply before shutdown");
    env.release("eof.child");
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    expected_messages.push(receipt);
    assert_eq!(reopened.messages("eof-reopened"), expected_messages);
    reopened.bash("eof-probe", &dead_probe(&["eof"]));
    let dead = reopened.success("eof-probe");
    assert_eq!(dead["exitCode"], 0, "{dead}");
    assert_eq!(dead["output"], "eof:dead\n");
    reopened.finish();
    assert!(!env.work.path().join("eof.orphan").exists());
    assert_eq!(up.served(), 1);
}
