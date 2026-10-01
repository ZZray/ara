//! Real RPC children for fixed OMP 596f2da soft compaction and native policy.
//! File gates establish process and Session ownership order without timing
//! guesses. Every case shares one deadline, including shutdown and reopen.

use ara_session::SessionJournal;
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
const CASE_BOUND: Duration = Duration::from_secs(20);

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
    deadline: Instant,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-compaction-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-compaction-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions, deadline: Instant::now() + CASE_BOUND }
    }

    fn command(&self, base_url: &str, args: &[&str]) -> Command {
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
            .args(["--mode", "rpc", "--model", "fake-model", "--base-url", base_url, "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(["--compact-keep-tokens", "1"])
            .args(args);
        if !args.contains(&"--compact-threshold") {
            command.args(["--compact-threshold", "0"]);
        }
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
        Self::spawn_url(env, &upstream.base_url(), args)
    }

    fn spawn_url(env: &Env, base_url: &str, args: &[&str]) -> Self {
        let mut child = env
            .command(base_url, args)
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
        self.finish_code(0);
    }

    fn finish_code(&mut self, expected: i32) {
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
        assert_eq!(status.code(), Some(expected), "{}", self.stderr.lock().unwrap());
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

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
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

fn compact(child: &mut RpcChild, id: &str, focus: Option<&str>) -> Value {
    let mut command = json!({"id":id,"type":"compact"});
    if let Some(focus) = focus {
        command["customInstructions"] = json!(focus);
    }
    child.send(command);
    child.success(id)
}

fn toggle(child: &mut RpcChild, id: &str, enabled: bool) {
    child.send(json!({"id":id,"type":"set_auto_compaction","enabled":enabled}));
    child.success(id);
}

fn summary_entries(entries: &[Value]) -> Vec<&Value> {
    entries.iter().filter(|entry| entry["type"] == "compaction").collect()
}

fn assert_summary_sources(entries: &[Value], summary: &Value) {
    let kept = entries.iter().position(|entry| entry["id"] == summary["firstKeptEntryId"]).unwrap();
    let expected = entries[..kept]
        .iter()
        .filter(|entry| entry["type"] == "message")
        .map(|entry| entry["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(summary["sourceEntryIds"], json!(expected));
    assert!(entries.iter().position(|entry| entry == summary).unwrap() > kept);
}

fn answer_with_input(message: &str, input: u64) -> Value {
    json!({"events":[text(message),finish("stop"),
        {"data":{"choices":[],"usage":{"prompt_tokens":input,"completion_tokens":1,"total_tokens":input+1}}},done()]})
}

/// A single request gate in front of FakeUpstream. The gate announces that
/// the actual HTTP request was accepted, then forwards it only on release.
/// All model responses and request evidence still come from FakeUpstream.
struct HttpGate {
    url: String,
    reached: Option<tokio::sync::oneshot::Receiver<()>>,
    release: Option<tokio::sync::oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpGate {
    async fn start(upstream: &FakeUpstream, gated_index: usize) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let target = upstream.addr;
        let (reached_tx, reached) = tokio::sync::oneshot::channel();
        let (release, release_rx) = tokio::sync::oneshot::channel();
        let gate = Arc::new(tokio::sync::Mutex::new(Some((reached_tx, release_rx))));
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let task = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                let Ok((mut client, _)) = listener.accept().await else { break };
                let index = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let gate = gate.clone();
                connections.spawn(async move {
                    let mut request = Vec::new();
                    let header_end = loop {
                        let mut buffer = [0; 8192];
                        let Ok(length) = client.read(&mut buffer).await else { return };
                        if length == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..length]);
                        if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                            break position + 4;
                        }
                    };
                    let length = String::from_utf8_lossy(&request[..header_end])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    while request.len() < header_end + length {
                        let mut buffer = [0; 8192];
                        let Ok(length) = client.read(&mut buffer).await else { return };
                        if length == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..length]);
                    }
                    if index == gated_index {
                        let Some((reached, release)) = gate.lock().await.take() else { return };
                        let _ = reached.send(());
                        if release.await.is_err() {
                            return;
                        }
                    }
                    let Ok(mut server) = tokio::net::TcpStream::connect(target).await else { return };
                    if server.write_all(&request).await.is_err() {
                        return;
                    }
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
                });
            }
        });
        Self { url: format!("http://{address}/v1"), reached: Some(reached), release: Some(release), task }
    }

    async fn reached(&mut self, deadline: Instant) {
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), self.reached.take().unwrap())
            .await
            .unwrap()
            .unwrap();
    }

    fn release(&mut self) {
        self.release.take().unwrap().send(()).unwrap();
    }
}

impl Drop for HttpGate {
    fn drop(&mut self) {
        self.release.take();
        self.task.abort();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_compact_focus_native_sources_chained_summary_next_prompt_and_reopen() {
    let env = Env::new();
    let up = upstream(vec![
        answer("old answer one"),
        answer("old answer two"),
        answer("kept answer"),
        answer("first derived summary"),
        answer("answer after first"),
        answer("second derived summary"),
        answer("answer after second"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    assert_eq!(child.state("default-policy")["autoCompactionEnabled"], true);
    for (id, input) in [("one", "old input one"), ("two", "old input two"), ("three", "kept input three")] {
        child.run(id, input);
    }
    assert_eq!(up.served(), 3, "threshold zero suppresses automatic summary calls despite enabled=true");
    let file = PathBuf::from(child.state("manual-file")["sessionFile"].as_str().unwrap());
    let raw_before = journal(&file);
    let first = compact(&mut child, "first-compact", Some("retain protocol facts and identifiers"));
    assert_eq!(first["summary"], "first derived summary");
    assert!(first["tokensBefore"].as_u64().unwrap() > 0);
    let after_first = journal(&file);
    assert_eq!(&after_first[..raw_before.len()], raw_before.as_slice());
    let first_entry = summary_entries(&after_first)[0];
    assert_summary_sources(&after_first, first_entry);
    let first_public = child.messages("first-public");
    assert_eq!(first_public[0]["role"], "compactionSummary");
    assert_eq!(first_public[0]["summary"], first["summary"]);
    assert_eq!(first_public[0]["method"], "soft");
    assert!(first_public.iter().any(|message| message_text(message) == "kept input three"));
    assert!(!first_public.iter().any(|message| message_text(message) == "old input one"));
    child.send(json!({"id":"already-compact","type":"compact"}));
    assert_eq!(child.response("already-compact")["success"], false);
    assert_eq!(up.served(), 4, "no new source window must not bill another summary");
    child.run("after-first", "new input after first summary");
    let second = compact(&mut child, "second-compact", Some("merge the previous protocol summary"));
    assert_eq!(second["summary"], "second derived summary");
    let after_second = journal(&file);
    let summaries = summary_entries(&after_second);
    assert_eq!(summaries.len(), 2);
    assert_summary_sources(&after_second, summaries[1]);
    for source in first_entry["sourceEntryIds"].as_array().unwrap() {
        assert!(summaries[1]["sourceEntryIds"].as_array().unwrap().contains(source));
    }
    child.run("after-second", "new input after second summary");
    let final_public = child.messages("final-public");
    child.finish();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    assert_eq!(reopened.messages("reopened-public"), final_public);
    reopened.finish();
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 7);
    for (index, focus) in [(3, "retain protocol facts and identifiers"), (5, "merge the previous protocol summary")] {
        assert!(requests[index]["body"]["tools"].as_array().is_none_or(Vec::is_empty));
        // Existing Chat Completions encoding omits a none choice when there
        // are no advertised tools; fixed summary context likewise has none.
        assert!(requests[index]["body"].get("tool_choice").is_none_or(|choice| choice == "none"));
        let content = requests[index]["body"]["messages"].to_string();
        assert!(content.contains(&format!("Additional focus: {focus}")));
    }
    let update = requests[5]["body"]["messages"].to_string();
    assert!(update.contains("previous-summary") && update.contains("first derived summary"));
    let next = requests[6]["body"]["messages"].to_string();
    assert!(next.contains("second derived summary") && next.contains("new input after second summary"));
    assert!(!next.contains("old input one") && !next.contains("first derived summary"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_compact_rejected_focus_and_failed_summaries_do_not_append_or_execute_tools() {
    let env = Env::new();
    let up = upstream(vec![answer("seed one"), answer("seed two"),
        json!({"events":[tool_call(0,"forbidden-summary-tool","write","{\"path\":\"summary-effect.txt\",\"content\":\"bad\"}"),finish("tool_calls"),done()]}),
        json!({"events":[text("unfinished summary"),finish("length"),done()]})]).await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.send(json!({"id":"nothing","type":"compact"}));
    assert_eq!(child.response("nothing")["success"], false);
    assert_eq!(up.served(), 0);
    child.run("seed-one", "first source input");
    child.run("seed-two", "second kept input");
    let file = PathBuf::from(child.state("failed-file")["sessionFile"].as_str().unwrap());
    let before = std::fs::read(&file).unwrap();
    child.send(json!({"id":"invalid-focus","type":"compact","customInstructions":42}));
    assert_eq!(child.response("invalid-focus")["success"], false);
    assert_eq!(up.served(), 2);
    for id in ["tool-summary", "partial-summary"] {
        child.send(json!({"id":id,"type":"compact"}));
        assert_eq!(child.response(id)["success"], false);
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert!(summary_entries(&journal(&file)).is_empty());
        assert_eq!(child.state(&format!("state-{id}"))["isCompacting"], false);
    }
    child.finish();
    assert!(!env.work.path().join("summary-effect.txt").exists());
    assert_eq!(up.served(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_compact_serial_summary_keeps_bash_independent_and_fixed_late_branch_reopen() {
    let env = Env::new();
    let up = upstream(vec![answer("seed one"), answer("seed two"), answer("summary before late Bash")]).await;
    let mut gate = HttpGate::start(&up, 2).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &[]);
    child.ready();
    child.run("branch-one", "old branch source");
    child.run("branch-two", "old branch kept");
    let file = PathBuf::from(child.state("branch-file")["sessionFile"].as_str().unwrap());
    let old_leaf = journal(&file).last().unwrap()["id"].clone();
    child.send(json!({"id":"gated-compact","type":"compact"}));
    gate.reached(env.deadline).await;
    let command = held_command("late-bash");
    child.bash("late-job", &command);
    env.wait_marker("late-bash.started");
    assert_eq!(std::fs::read_to_string(env.work.path().join("late-bash.effect")).unwrap(), "effect:late-bash");
    // Bash was dispatched from the reader while compact owns the serial tail.
    gate.release();
    assert_eq!(child.success("gated-compact")["summary"], "summary before late Bash");
    let live = child.messages("compact-live");
    assert_eq!(live[0]["role"], "compactionSummary");
    env.release("late-bash");
    assert_eq!(child.success("late-job")["cancelled"], false);
    assert_eq!(
        child.messages("after-late-native"),
        live,
        "late old-owner receipt does not enter compacted live context"
    );
    let entries = journal(&file);
    let receipt = entries.last().unwrap();
    assert_eq!(receipt["message"]["role"], "bashExecution");
    assert_eq!(receipt["parentId"], old_leaf);
    assert_eq!(receipt["message"]["command"], command);
    child.finish();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    let reopened_messages = reopened.messages("fixed-last-raw-leaf");
    // Fixed SessionEntryIndex.rebuild chooses the last raw record. Its live
    // index-only leaf restoration is not a durable selected-leaf marker.
    assert!(!reopened_messages.iter().any(|message| message["role"] == "compactionSummary"));
    assert_eq!(reopened_messages.last().unwrap(), &receipt["message"]);
    assert!(reopened_messages.iter().any(|message| message_text(message) == "old branch source"));
    reopened.finish();
    assert_eq!(up.served(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_eof_drains_accepted_serial_summary_and_commits_before_shutdown() {
    let env = Env::new();
    let up = upstream(vec![answer("first"), answer("second"), answer("EOF accepted summary")]).await;
    let mut gate = HttpGate::start(&up, 2).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &[]);
    child.ready();
    child.run("eof-one", "first EOF source");
    child.run("eof-two", "second EOF kept");
    let file = PathBuf::from(child.state("eof-summary-file")["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"eof-summary","type":"compact"}));
    gate.reached(env.deadline).await;
    child.stdin.take();
    gate.release();
    child.finish();
    let reply = child.seen.iter().find(|frame| frame["id"] == "eof-summary" && frame["type"] == "response").unwrap();
    assert_eq!(reply["success"], true, "{reply}");
    assert_eq!(reply["data"]["summary"], "EOF accepted summary");
    assert_eq!(summary_entries(&journal(&file)).len(), 1);
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap()]);
    reopened.ready();
    assert_eq!(reopened.messages("eof-summary-native")[0]["role"], "compactionSummary");
    reopened.finish();
    assert_eq!(up.served(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_auto_compaction_persistent_policy_billed_threshold_and_single_fire() {
    let env = Env::new();
    let up = upstream(vec![
        answer_with_input("small first", 1),
        answer_with_input("small billed tail", 5000),
        answer("automatic billed summary"),
        answer_with_input("after automatic summary", 1),
        answer("restart disabled answer"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &["--compact-threshold", "1000"]);
    child.ready();
    assert_eq!(child.state("auto-default")["autoCompactionEnabled"], true);
    toggle(&mut child, "auto-off", false);
    child.run("auto-one", "one small source");
    child.run("auto-two", "two small kept source");
    let original = PathBuf::from(child.state("auto-file")["sessionFile"].as_str().unwrap());
    assert_eq!(up.served(), 2);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_compaction_start"));
    toggle(&mut child, "auto-on", true);
    child.run("auto-next", "pending small input triggers billed threshold");
    // A state command is a serial barrier after the joined automatic boundary.
    assert_eq!(child.state("auto-after")["autoCompactionEnabled"], true);
    let starts = child.seen.iter().filter(|frame| frame["type"] == "auto_compaction_start").collect::<Vec<_>>();
    let ends = child.seen.iter().filter(|frame| frame["type"] == "auto_compaction_end").collect::<Vec<_>>();
    assert_eq!(starts.len(), 1, "{:?}", child.seen);
    assert_eq!(ends.len(), 1);
    assert_eq!(starts[0]["reason"], "threshold");
    assert_eq!(ends[0]["aborted"], false);
    assert_eq!(ends[0]["willRetry"], false);
    assert_eq!(ends[0]["result"]["summary"], "automatic billed summary");
    assert_eq!(up.served(), 4, "terminal success must not schedule a spurious continuation");
    assert_eq!(summary_entries(&journal(&original)).len(), 1);
    child.send(json!({"id":"auto-new","type":"new_session"}));
    child.success("auto-new");
    assert_eq!(child.state("auto-new-state")["autoCompactionEnabled"], true);
    child.send(json!({"id":"auto-switch","type":"switch_session","sessionPath":original}));
    child.success("auto-switch");
    assert_eq!(child.state("auto-switch-state")["autoCompactionEnabled"], true);
    toggle(&mut child, "persist-auto-off", false);
    child.finish();
    let config = env.home.path().join("agent/config.yml");
    assert!(config.is_file());
    let mut reopened =
        RpcChild::spawn(&env, &up, &["--resume", original.to_str().unwrap(), "--compact-threshold", "1"]);
    reopened.ready();
    assert_eq!(reopened.state("restart-auto-off")["autoCompactionEnabled"], false);
    reopened.run("restart-prompt", "disabled policy must not compact at threshold one");
    assert!(!reopened.seen.iter().any(|frame| frame["type"] == "auto_compaction_start"));
    reopened.finish();
    assert_eq!(up.served(), 5);
    let requests = up.requests.lock().await;
    assert!(requests[2]["body"]["tools"].as_array().is_none_or(Vec::is_empty));
    assert!(requests[3]["body"]["messages"].to_string().contains("automatic billed summary"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_compaction_commit_disk_failure_stops_connection_before_queued_prompt() {
    let env = Env::new();
    let up = upstream(vec![answer("durable first"), answer("durable second"), answer("summary whose commit must fail"),
        json!({"events":[tool_call(0,"must-not-run","write","{\"path\":\"queued-effect.txt\",\"content\":\"forbidden\"}"),finish("tool_calls"),done()]})]).await;
    let mut gate = HttpGate::start(&up, 2).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &["--tools", "write"]);
    child.ready();
    child.run("disk-one", "first durable source");
    child.run("disk-two", "second durable kept source");
    let file = PathBuf::from(child.state("disk-file")["sessionFile"].as_str().unwrap());
    let original = std::fs::read(&file).unwrap();
    child.send(json!({"id":"disk-compact","type":"compact"}));
    gate.reached(env.deadline).await;
    // Fault the actual destination only after the source snapshot and HTTP
    // dispatch. An on-disk directory cannot accept the native append.
    std::fs::remove_file(&file).unwrap();
    std::fs::create_dir(&file).unwrap();
    child.send(json!({"id":"queued-after-disk-failure","type":"prompt","message":"write the queued effect"}));
    gate.release();
    let failure = child.response("disk-compact");
    assert_eq!(failure["success"], false, "{failure}");
    assert!(!failure["error"].as_str().unwrap().is_empty());
    child.finish_code(1);
    assert!(file.is_dir(), "a failed commit must not replace the actual bad destination");
    assert_eq!(up.served(), 3, "the queued input must not dispatch another provider call after persistence failed");
    assert!(!env.work.path().join("queued-effect.txt").exists());
    assert!(!child.seen.iter().any(|frame| frame["id"] == "queued-after-disk-failure" && frame["success"] == true));
    assert!(!original.is_empty(), "fault was applied to an actual materialized source Session");
}

fn overflow_config(env: &Env, window: u64, reserve: Option<u64>) {
    let agent = env.home.path().join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(
        agent.join("models.yml"),
        json!({"providers":{"overflow-fixture":{
            "api":"openai-completions", "baseUrl":"http://127.0.0.1:1/v1", "auth":"none",
            "models":[{"id":"fake-model","contextWindow":window,"maxTokens":512,"input":["text"]}]
        }}})
        .to_string(),
    )
    .unwrap();
    let reserve = reserve.map(|tokens| format!("  reserveTokens: {tokens}\n")).unwrap_or_default();
    std::fs::write(agent.join("config.yml"), format!(
        "compaction:\n  enabled: true\n  methodOrder: [soft]\n{reserve}retry:\n  enabled: false\n  modelFallback: false\n"
    )).unwrap();
}

fn failed_overflow(content: Option<&str>, payload: bool) -> Value {
    let mut events = Vec::new();
    if let Some(content) = content {
        events.push(text(content));
    }
    let input = if payload { 1000 } else { 250_000 };
    // Every reported bucket is explicit; omitted cache buckets stay unknown in
    // production and cannot establish the native trusted-payload headroom case.
    events.push(json!({"data":{"choices":[],"usage":{"prompt_tokens":input,"completion_tokens":0,
        "prompt_tokens_details":{"cached_tokens":0,"cache_write_tokens":0},"total_tokens":input}}}));
    events.push(json!({"data":{"error":{"code":if payload {413} else {400},"message":if payload {
        "413 request exceeds the maximum size"
    } else { "prompt is too long: 250000 tokens > 200000 maximum" }}}}));
    json!({"events":events})
}

fn responses_answer(text: &str) -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{
            "type":"message","id":"msg-summary-fixture","content":[{"type":"output_text","text":text}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp-summary-fixture","status":"completed"}}}
    ]})
}

// Native message.done recovery is separate from transparent replay. Exercise
// the actual Provider -> Host -> branch transaction, including unknown output.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_responses_content_only_overflow_and_native_output_veto_module() {
    for unsafe_output in [false, true] {
        let env = Env::new();
        overflow_config(&env, 200_000, Some(25_000));
        let path = env.home.path().join("agent/models.yml");
        let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        config["providers"]["overflow-fixture"]["api"] = json!("openai-responses");
        config["providers"]["overflow-fixture"]["models"][0]["maxTokens"] = json!(20_000);
        std::fs::write(path, config.to_string()).unwrap();
        let mut failure = responses_answer("visible completed text before overflow");
        failure["events"].as_array_mut().unwrap().pop();
        let output =
            if unsafe_output { json!([{"type":"computer_call","id":"unobserved-native-effect"}]) } else { json!([]) };
        failure["events"].as_array_mut().unwrap().push(json!({"data":{
            "type":"response.failed","response":{"id":"resp-overflow","status":"failed",
                "output":output,"error":{"code":"context_length_exceeded","message":"maximum context length exceeded"}}
        }}));
        let mut responses = vec![responses_answer("completed prior source"), failure];
        if !unsafe_output {
            responses.extend([
                responses_answer("native Responses recovery summary"),
                responses_answer("continued original input"),
            ]);
        }
        let up = upstream(responses).await;
        let args = ["--provider", "overflow-fixture", "--max-tokens", "20000"];
        let mut child = RpcChild::spawn(&env, &up, &args);
        child.ready();
        child.run("seed", "completed earlier source turn");
        let before = child.state("before");
        child.prompt("overflow", "recover the original pending input");
        if !unsafe_output {
            let end = child.until(|frame| frame["type"] == "auto_compaction_end");
            assert_eq!(end["willRetry"], true, "{end}");
        }
        child.until(|frame| frame["type"] == "agent_end");
        let after = child.state("after");
        assert_eq!(after["sessionId"], before["sessionId"]);
        assert_eq!(after["isCompacting"], false);
        assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
        child.finish();
        assert_eq!(up.served(), if unsafe_output { 2 } else { 4 });
        let file = PathBuf::from(after["sessionFile"].as_str().unwrap());
        let entries = journal(&file);
        assert_eq!(summary_entries(&entries).len(), usize::from(!unsafe_output));
        let failed = entries.iter().find(|row| row["message"]["stopReason"] == "error").unwrap();
        assert_eq!(
            failed["message"]["failureEvidence"]["contextRecovery"],
            if unsafe_output { "nativeOutput" } else { "contentOnly" }
        );
        if !unsafe_output {
            let requests = up.requests.lock().await;
            assert_eq!(requests[2]["body"]["max_output_tokens"], 16_384);
            assert!(requests[2]["body"]["tools"].as_array().is_none_or(Vec::is_empty));
            assert!(!requests[2]["body"]["input"].to_string().contains("visible completed text before overflow"));
            assert!(requests[3]["body"]["input"].to_string().contains("native Responses recovery summary"));
            let projected = SessionJournal::open(&file).unwrap().model_context();
            assert!(
                !projected
                    .iter()
                    .any(|message| message.as_assistant().is_some_and(|m| m.stop_reason == ara_ai::StopReason::Error))
            );
        }
    }
}

// One module sequence verifies raw reserve -> wire cap -> message fold ->
// carried summary -> one durable compaction containing all original IDs.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_manual_summary_budget_folding_module() {
    let env = Env::new();
    overflow_config(&env, 8192, Some(25_000));
    let path = env.home.path().join("agent/models.yml");
    let mut config: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    config["providers"]["overflow-fixture"]["models"][0]["maxTokens"] = json!(20_000);
    std::fs::write(path, config.to_string()).unwrap();
    let up = upstream(vec![
        answer("completed source"),
        answer("kept turn"),
        answer("first window summary"),
        answer("final carried summary"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &["--provider", "overflow-fixture", "--max-tokens", "20000"]);
    child.ready();
    child.run("seed", &"original oversized input ".repeat(350));
    child.run("kept", "keep this recent turn");
    let before = child.state("before-fold");
    compact(&mut child, "fold", Some("Retain original receipts"));
    let after = child.state("after-fold");
    assert_eq!(before["sessionId"], after["sessionId"]);
    child.finish();
    assert_eq!(up.served(), 4);
    let requests = up.requests.lock().await;
    for index in [2, 3] {
        assert_eq!(requests[index]["body"]["max_tokens"], 16_384);
        assert!(requests[index]["body"]["tools"].as_array().is_none_or(Vec::is_empty));
        assert!(requests[index]["body"]["messages"].to_string().contains("Retain original receipts"));
    }
    assert!(requests[2]["body"]["messages"].to_string().contains("more characters truncated"));
    assert!(requests[3]["body"]["messages"].to_string().contains("first window summary"));
    let entries = journal(Path::new(after["sessionFile"].as_str().unwrap()));
    let summaries = summary_entries(&entries);
    assert_eq!(summaries.len(), 1);
    assert_summary_sources(&entries, summaries[0]);
    assert_eq!(summaries[0]["summary"], "final carried summary");
}

// Fixed native scenarios: auto-compaction-progress-guard:620-870,1343-1404,
//1537-1565 and payload-rejection-413:239-268. One real Host/Session sequence per
// case covers disposition, summary result, provider request and restart together.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_native_soft_overflow_recovery_module_scenarios() {
    for case in [
        "soft-success",
        "contentful-summary-failure",
        "empty-summary-failure",
        "payload-headroom",
        "unknown-http-payload",
        "reserve-deadend",
    ] {
        let env = Env::new();
        let deadend = case == "reserve-deadend";
        let unknown_usage = case == "unknown-http-payload";
        let payload = case == "payload-headroom" || unknown_usage;
        let empty = case == "empty-summary-failure";
        let summary_failure = case.ends_with("summary-failure");
        overflow_config(&env, if deadend { 10_000 } else { 200_000 }, deadend.then_some(8000));
        let partial = format!("visible-overflow-{case}");
        let summary = if summary_failure {
            json!({"events":[text("rejected incomplete summary"),finish("length"),done()]})
        } else {
            answer("native soft overflow summary")
        };
        let failure = if unknown_usage {
            json!({"status":413,"body":json!({"error":{"message":"request exceeds the maximum size"}}).to_string()})
        } else {
            failed_overflow((!empty && !payload).then_some(partial.as_str()), payload)
        };
        let mut responses = vec![answer("completed prior turn"), failure];
        if !payload {
            responses.push(summary);
        }
        if case == "soft-success" {
            responses.push(answer("continued after overflow"));
        }
        responses.push(answer("explicit restart answer"));
        let up = upstream(responses).await;
        let mut child = RpcChild::spawn(&env, &up, &["--provider", "overflow-fixture"]);
        child.ready();
        child.run("seed", "earlier completed source");
        let initial = child.state("initial-session");
        let file = PathBuf::from(initial["sessionFile"].as_str().unwrap());
        let request =
            if deadend { "unavoidable kept input ".repeat(2000) } else { "current request needs recovery".into() };
        child.prompt("overflow", &request);
        if payload {
            child.until(|frame| {
                frame["type"] == "notice"
                    && frame["source"] == "compaction"
                    && frame["message"].as_str().is_some_and(|message| message.contains("payload"))
            });
        } else {
            let end = child.until(|frame| frame["type"] == "auto_compaction_end");
            assert_eq!(end["aborted"], false, "{case}: {end}");
            assert_eq!(end["willRetry"], case == "soft-success", "{case}: {end}");
            if summary_failure {
                assert!(end["errorMessage"].as_str().is_some_and(|error| !error.is_empty()));
            }
            if case == "soft-success" {
                child.until(|frame| frame["type"] == "agent_end");
            }
        }
        let settled = child.state("settled");
        assert_eq!(settled["sessionId"], initial["sessionId"]);
        assert_eq!(settled["sessionFile"], initial["sessionFile"]);
        assert_eq!(settled["isCompacting"], false);
        assert_eq!(settled["isStreaming"], false);
        assert_eq!(settled["isRetrying"], false);
        let starts = child.seen.iter().filter(|frame| frame["type"] == "auto_compaction_start").collect::<Vec<_>>();
        assert_eq!(starts.len(), usize::from(!payload), "{case}: {:?}", child.seen);
        if let Some(start) = starts.first() {
            assert_eq!(start["reason"], "overflow");
        }
        assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
        if deadend {
            assert_eq!(
                child
                    .seen
                    .iter()
                    .filter(|frame| frame["type"] == "notice"
                        && frame["source"] == "compaction"
                        && frame["message"].as_str().is_some_and(|text| text.contains("enough context")))
                    .count(),
                1
            );
        }
        child.finish();
        let served_before_restart = if payload {
            2
        } else if case == "soft-success" {
            4
        } else {
            3
        };
        assert_eq!(up.served(), served_before_restart, "{case}: no blind automatic extra request");
        let loaded = SessionJournal::open(&file).unwrap();
        let entries = journal(&file);
        assert_eq!(summary_entries(&entries).len(), usize::from(!payload && !summary_failure));
        assert!(
            entries.iter().any(|entry| entry["message"]["stopReason"] == "error"),
            "{case}: keep original raw failure receipt"
        );
        if unknown_usage {
            let error = entries.iter().find(|entry| entry["message"]["stopReason"] == "error").unwrap();
            for bucket in ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] {
                assert!(
                    error["message"]["usage"].get(bucket).is_none(),
                    "{case}: unknown {bucket} must not become zero"
                );
            }
        }
        if summary_failure && !empty {
            let raw_errors = entries
                .iter()
                .filter(|entry| entry["message"]["stopReason"] == "error" && message_text(&entry["message"]) == partial)
                .collect::<Vec<_>>();
            assert_eq!(raw_errors.len(), 2, "native rollback appends a new receipt while retaining the original");
            assert_ne!(raw_errors[0]["id"], raw_errors[1]["id"]);
            assert_eq!(
                loaded.model_context().last().unwrap().as_assistant().unwrap().stop_reason,
                ara_ai::StopReason::Error
            );
        }
        if empty || payload {
            assert!(loaded.model_context().iter().all(|message| {
                message.as_assistant().is_none_or(|assistant| assistant.stop_reason != ara_ai::StopReason::Error)
            }));
        }
        if case == "soft-success" {
            assert_eq!(
                loaded.model_context().last().unwrap().as_assistant().unwrap().stop_reason,
                ara_ai::StopReason::Stop
            );
        }
        let mut reopened =
            RpcChild::spawn(&env, &up, &["--provider", "overflow-fixture", "--resume", file.to_str().unwrap()]);
        reopened.ready();
        reopened.run("explicit-restart", "explicit user request after restart");
        reopened.finish();
        assert_eq!(up.served(), served_before_restart + 1, "{case}");
        let requests = up.requests.lock().await;
        if !payload {
            assert!(requests[2]["body"]["tools"].as_array().is_none_or(Vec::is_empty));
            assert!(!requests[2]["body"]["messages"].to_string().contains(&partial));
        }
        if case == "soft-success" {
            let continuation = requests[3]["body"]["messages"].to_string();
            assert!(continuation.contains("native soft overflow summary"));
            assert!(continuation.contains("current request needs recovery"));
            assert!(!continuation.contains(&partial));
        }
        let restarted = requests.last().unwrap()["body"]["messages"].to_string();
        assert_eq!(
            restarted.contains(&partial),
            summary_failure && !empty,
            "{case}: replay only the restored visible failure"
        );
    }
}

// Native queue:148-223,267-325 and cancellation:113-164. A real HTTP gate
// establishes summary ownership before abort/queue commands, without a sleep.
#[tokio::test(flavor = "multi_thread")]
async fn rpc_native_overflow_summary_owned_abort_and_queued_input_scenarios() {
    for cancel in [true, false] {
        let env = Env::new();
        overflow_config(&env, 200_000, None);
        let partial = "owned-summary visible overflow";
        let mut responses = vec![
            answer("owned earlier completed answer"),
            failed_overflow(Some(partial), false),
            answer("owned overflow summary"),
            answer("continued current request after summary"),
        ];
        if !cancel {
            responses.push(answer("completed explicit queued follow-up"));
        }
        let up = upstream(responses).await;
        let mut gate = HttpGate::start(&up, 2).await;
        let mut child = RpcChild::spawn_url(&env, &gate.url, &["--provider", "overflow-fixture"]);
        child.ready();
        child.run("owned-seed", "earlier completed owned source");
        child.prompt("owned-overflow", "recover while the host accepts other commands");
        gate.reached(env.deadline).await;
        let active = child.state("summary-active");
        assert_eq!(active["isCompacting"], true);
        assert_eq!(active["isStreaming"], false);
        let file = PathBuf::from(active["sessionFile"].as_str().unwrap());
        assert!(summary_entries(&journal(&file)).is_empty());
        if cancel {
            child.send(json!({"id":"cancel-owned-summary","type":"abort"}));
            child.success("cancel-owned-summary");
            let end = child.seen.iter().find(|frame| frame["type"] == "auto_compaction_end").unwrap();
            assert_eq!(end["aborted"], true);
            assert_eq!(end["willRetry"], false);
            assert_eq!(child.state("after-abort")["isCompacting"], false);
            assert_eq!(up.served(), 2, "cancel the gated summary before it can commit or continue");
            assert!(summary_entries(&journal(&file)).is_empty());
            assert_eq!(
                SessionJournal::open(&file)
                    .unwrap()
                    .model_context()
                    .last()
                    .unwrap()
                    .as_assistant()
                    .unwrap()
                    .stop_reason,
                ara_ai::StopReason::Error
            );
            // The aborted gate must not forward its stale request. A replacement
            // uses the original fixture's next response after cleanup completes.
            child.run("replacement", "replacement prompt after summary abort");
            child.finish();
            assert_eq!(up.served(), 3);
        } else {
            child.send(json!({"id":"queue-owned-summary","type":"follow_up","message":"explicit queued follow-up"}));
            child.success("queue-owned-summary");
            let queued = child.state("queued-during-summary");
            assert_eq!(queued["isCompacting"], true);
            assert_eq!(queued["queuedMessageCount"], 1);
            assert_eq!(up.served(), 2);
            gate.release();
            let end = child.until(|frame| frame["type"] == "auto_compaction_end");
            assert_eq!(end["aborted"], false);
            assert_eq!(end["willRetry"], true);
            child.until(|frame| frame["type"] == "agent_end");
            let settled = child.state("owned-queue-settled");
            assert_eq!(settled["queuedMessageCount"], 0);
            assert_eq!(settled["isCompacting"], false);
            assert_eq!(settled["isStreaming"], false);
            assert_eq!(settled["sessionId"], active["sessionId"]);
            assert_eq!(settled["sessionFile"], active["sessionFile"]);
            let messages = child.messages("completed-owned-queue");
            assert_eq!(
                messages.iter().filter(|message| message_text(message) == "explicit queued follow-up").count(),
                1
            );
            assert!(messages.iter().any(|message| message_text(message) == "completed explicit queued follow-up"));
            child.finish();
            assert_eq!(up.served(), 5, "one summary, native clean-user continuation, then one explicit follow-up");
            let requests = up.requests.lock().await;
            // Native Agent continue only dequeues follow-up immediately for an
            // assistant/empty tail. The clean user tail completes first, and
            // the same owned Run drains follow-up without another Host retry.
            for request in &requests[3..] {
                let continuation = request["body"]["messages"].to_string();
                assert!(continuation.contains("owned overflow summary"));
                assert!(!continuation.contains(partial));
            }
            assert!(requests[4]["body"]["messages"].to_string().contains("explicit queued follow-up"));
            assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
        }
    }
}
