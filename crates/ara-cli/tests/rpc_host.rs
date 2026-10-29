//! Real RPC child processes, controlled HTTP streams, file effects and journals.
//! Reader threads keep every stdout wait bounded; the guard kills failed tests.

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const WAIT: Duration = Duration::from_secs(15);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_core_hooks_interrupt_setting_raw_result_and_journal_restart_group() {
    let env = Env::new();
    let directory = env.home.path().join("agent");
    std::fs::create_dir_all(&directory).unwrap();
    let settings = directory.join("config.yml");
    std::fs::write(&settings, "unrelated: retained\nfeatures:\n  unexpectedStopDetection: none\n").unwrap();
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"valid-metadata","held","{}"),
            tool_call(1,"invalid-metadata","held","{}"),tool_call(2,"self-error","held","{}"),finish("tool_calls"),done()]},
        {"events":[text("All actual tool outcomes retained."),finish("stop"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    assert_eq!(child.state("default-mode")["interruptMode"], "immediate");
    child.send(json!({"id":"wait-mode","type":"set_interrupt_mode","mode":"wait"}));
    child.success("wait-mode");
    assert_eq!(child.state("wait-state")["interruptMode"], "wait");
    let before = std::fs::read(&settings).unwrap();
    child.send(json!({"id":"bad-mode","type":"set_interrupt_mode","mode":"invalid"}));
    assert_eq!(child.response("bad-mode")["success"], false);
    assert_eq!(std::fs::read(&settings).unwrap(), before);
    child.send(json!({"id":"tools","type":"set_host_tools","tools":[{
        "name":"held","description":"controlled foreground tool","parameters":{"type":"object","properties":{}}
    }]}));
    child.success("tools");
    child.send(json!({"id":"run","type":"prompt","message":"preserve all actual settled results"}));
    child.success("run");
    let calls: Vec<_> = (0..3).map(|_| child.until(|frame| frame["type"] == "host_tool_call", WAIT)).collect();
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"));
    child.send(json!({"id":"next-mode","type":"set_interrupt_mode","mode":"immediate"}));
    child.success("next-mode");
    child.send(json!({"id":"queued-user","type":"steer","message":"queued user source must survive"}));
    child.success("queued-user");
    let metadata = json!({"type":"computer", "screenshot":{"type":"computer_screenshot","file_id":"retained-file"},
        "acknowledgedSafetyChecks":[{"id":"safety","message":null}]});
    for call in calls {
        let result = match call["toolCallId"].as_str().unwrap() {
            "valid-metadata" => json!({"content":[{"type":"text","text":"real output"}],
                "details":{"providerMetadata":"ordinary detail"},"providerMetadata":metadata,"useless":true}),
            "invalid-metadata" => json!({"content":[{"type":"text","text":"malformed metadata output"}],
                "providerMetadata":{"type":"computer","screenshot":{"type":"computer_screenshot", "image_url":"image","file_id":null},
                    "acknowledgedSafetyChecks":[]}, "useless":true}),
            "self-error" => json!({"content":[],"isError":true,"useless":true}),
            other => panic!("unexpected tool {other}"),
        };
        child.send(json!({"type":"host_tool_result","id":call["id"],"result":result}));
    }
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let messages = child.messages("actual-results");
    let valid = messages.iter().find(|message| message["toolCallId"] == "valid-metadata").unwrap();
    assert_eq!(valid["providerMetadata"], metadata);
    assert_eq!(valid["useless"], true);
    assert_eq!(valid["isError"], false);
    assert_eq!(valid["details"], json!({"providerMetadata":"ordinary detail"}));
    let invalid = messages.iter().find(|message| message["toolCallId"] == "invalid-metadata").unwrap();
    assert_eq!(invalid["isError"], true);
    assert!(invalid.get("providerMetadata").is_none() && invalid.get("useless").is_none());
    assert!(contains_text(invalid, "computer providerMetadata had an unsupported shape"));
    let error = messages.iter().find(|message| message["toolCallId"] == "self-error").unwrap();
    assert_eq!(error["isError"], true);
    assert!(error.get("useless").is_none());
    assert!(contains_text(error, "Tool failed with no output."));
    assert_eq!(
        user_texts(&messages).iter().filter(|text| text.as_str() == "queued user source must survive").count(),
        1
    );
    assert!(!child.seen.iter().any(|frame| frame["type"] == "host_tool_cancel"));
    let state = child.state("settled-mode");
    assert_eq!(state["interruptMode"], "immediate");
    let file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    child.finish(0);
    let entries = journal(&file);
    let persisted = entries.iter().find(|entry| entry["message"]["toolCallId"] == "valid-metadata").unwrap();
    assert_eq!(persisted["message"]["providerMetadata"], metadata);
    assert_eq!(persisted["message"]["useless"], true);
    assert!(std::fs::read_to_string(&settings).unwrap().contains("unrelated: retained"));
    let mut reopened = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    reopened.ready();
    assert_eq!(reopened.state("restored-mode")["interruptMode"], "immediate");
    assert_eq!(reopened.messages("restored-results"), messages);
    reopened.finish(0);
    assert_eq!(up.served(), 2);
}

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-rpc-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-rpc-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions }
    }

    fn command(&self, base_url: &str, args: &[&str]) -> Command {
        self.command_at(base_url, args, self.work.path())
    }

    fn command_at(&self, base_url: &str, args: &[&str], cwd: &Path) -> Command {
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
            .env("ARA_API_KEY", "sk-rpc-fixture-only")
            .args(["--mode", "rpc", "--model", "fake-model", "--base-url", base_url, "--cwd"])
            .arg(cwd)
            .arg("--session-dir")
            .arg(&self.sessions);
        if !args.contains(&"--compact-threshold") {
            command.args(["--compact-threshold", "0"]);
        }
        command.args(args);
        command
    }

    // Reuse the real fresh-cache path for unrelated catalogue producers.
    // Configured auto/switch endpoints stay cold and fully observed by this group.
    fn selection_command(&self, base_url: &str, args: &[&str]) -> Command {
        use ara_cli::provider_models::{
            catalog_providers, models_dev_catalog_provider_ids, provider_descriptors, resolve_model_cache_provider_id,
        };
        let cache = ara_cli::model_cache::SqliteModelCache::for_path(self.home.path().join("agent/model-cache.db"));
        let mut providers = models_dev_catalog_provider_ids();
        providers.extend(provider_descriptors().into_iter().map(|provider| provider.provider_id));
        providers.push("openai-codex".into());
        providers.sort_by(|a, b| a.units().cmp(b.units()));
        providers.dedup();
        let no_environment = |_: &str| -> Option<ara_rpc::WireString> { None };
        let now = chrono::Utc::now().timestamp_millis() as f64;
        for provider in providers {
            let codex = provider.equals_ascii("openai-codex");
            let bundled =
                if codex { ara_cli::model_identity_wire::bundled_provider_models(&provider) } else { Vec::new() };
            let fingerprint = if codex {
                ara_cli::model_manager::fingerprint_static_models(
                    &ara_cli::model_manager::ModelArray::new(bundled.clone()),
                    true,
                )
            } else {
                "".into()
            };
            let cache_base = provider.equals_ascii("ollama").then(|| ara_rpc::WireString::from("http://127.0.0.1:1"));
            let key = resolve_model_cache_provider_id(&provider, None, cache_base.as_ref(), &no_environment);
            cache
                .write_model_cache_wire(
                    &key,
                    now,
                    &[],
                    ara_cli::model_cache::WireModelCacheWriteOptions {
                        authoritative: codex,
                        static_fingerprint: &fingerprint,
                        static_header_sources: &bundled,
                        restorable_header_fallback: None,
                    },
                )
                .unwrap();
        }
        let mut command = self.command(base_url, args);
        for provider in catalog_providers() {
            for key in provider
                .env_vars
                .iter()
                .flatten()
                .chain(provider.catalog_discovery.iter().flat_map(|discovery| discovery.env_vars.iter().flatten()))
            {
                let key = key.to_utf8().unwrap();
                if key != "ARA_API_KEY" {
                    command.env_remove(key);
                }
            }
        }
        for key in [
            "LITELLM_BASE_URL",
            "GOOGLE_CLOUD_PROJECT",
            "GOOGLE_CLOUD_LOCATION",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_CLOUD_QUOTA_PROJECT",
        ] {
            command.env_remove(key);
        }
        // Native startup and later catalog queries also discover local models.
        // Keep those reads away from unrelated services on the test machine.
        for key in ["OLLAMA_BASE_URL", "OLLAMA_HOST", "LLAMA_CPP_BASE_URL", "LM_STUDIO_BASE_URL"] {
            command.env(key, "http://127.0.0.1:1");
        }
        command
    }

    fn sessions(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = std::fs::read_dir(&self.sessions)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.extension().is_some_and(|x| x == "jsonl"))
                    .collect()
            })
            .unwrap_or_default();
        paths.sort();
        paths
    }

    /// Actual startup selection: retain fixture isolation/cache seeding while
    /// omitting command()'s explicit model and endpoint override.
    fn startup_command(&self, args: &[&str]) -> Command {
        let seeded = self.selection_command("http://127.0.0.1:1", &[]);
        let mut command = Command::new(BIN);
        for (key, value) in seeded.get_envs() {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
        let mut source = seeded.get_args();
        while let Some(arg) = source.next() {
            if arg == "--model" || arg == "--base-url" {
                source.next();
            } else {
                command.arg(arg);
            }
        }
        command.args(args);
        command
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Result<Value, String>>,
    seen: Vec<Value>,
    stderr: Arc<Mutex<String>>,
    paused: Arc<AtomicBool>,
    close_stdout: Arc<AtomicBool>,
    readers: Vec<JoinHandle<()>>,
}

impl RpcChild {
    fn spawn(mut command: Command) -> Self {
        let mut child = command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (tx, frames) = mpsc::channel();
        let paused = Arc::new(AtomicBool::new(false));
        let reader_paused = paused.clone();
        let close_stdout = Arc::new(AtomicBool::new(false));
        let reader_close = close_stdout.clone();
        let output_reader = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                while reader_paused.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if reader_close.load(Ordering::Acquire) {
                            break;
                        }
                        let frame =
                            serde_json::from_str(&line).map_err(|error| format!("invalid RPC stdout: {error}: {line}"));
                        if tx.send(frame).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(format!("reading RPC stdout: {error}")));
                        break;
                    }
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
            paused,
            close_stdout,
            readers: vec![output_reader, error_reader],
        }
    }

    fn send(&mut self, frame: Value) {
        self.send_raw(&format!("{frame}\n"));
    }

    fn send_raw(&mut self, bytes: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(bytes.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    fn next(&mut self, bound: Duration) -> Value {
        match self.frames.recv_timeout(bound) {
            Ok(Ok(frame)) => {
                self.seen.push(frame.clone());
                frame
            }
            result => panic!("RPC frame wait failed: {result:?}; stderr: {}; frames: {:?}", self.errors(), self.seen),
        }
    }

    fn until(&mut self, matches: impl Fn(&Value) -> bool, bound: Duration) -> Value {
        let deadline = Instant::now() + bound;
        loop {
            let frame = self.next(deadline.saturating_duration_since(Instant::now()));
            if matches(&frame) {
                return frame;
            }
        }
    }

    fn response(&mut self, id: &str) -> Value {
        self.until(|frame| frame["type"] == "response" && frame["id"] == id, WAIT)
    }

    fn success(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], true, "{response}");
        response
    }

    fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"}));
        self.success(id)["data"].clone()
    }

    fn messages(&mut self, id: &str) -> Vec<Value> {
        self.send(json!({"id":id,"type":"get_messages"}));
        self.success(id)["data"]["messages"].as_array().unwrap().clone()
    }

    fn adopt(&mut self, id: &str, frame: Value) {
        self.adopt_commands(id, frame, json!([]));
    }

    fn adopt_commands(&mut self, id: &str, frame: Value, commands: Value) {
        let begin = self.seen.len();
        self.send(frame);
        assert_eq!(self.success(id)["data"], json!({"cancelled":false}));
        let updates: Vec<_> = self.seen[begin..]
            .iter()
            .enumerate()
            .filter(|(_, frame)| frame["type"] == "available_commands_update")
            .collect();
        assert_eq!(updates.len(), 1, "exactly one command metadata update precedes adoption ACK");
        assert_eq!(updates[0].1["commands"], commands);
        assert!(updates[0].0 < self.seen.len() - begin - 1, "metadata precedes correlated response");
    }

    fn ready(&mut self) {
        self.ready_commands(json!([]));
    }

    fn ready_commands(&mut self, commands: Value) {
        let ready = self.next(WAIT);
        assert_eq!(ready["type"], "ready", "{ready}");
        assert_eq!(ready["protocolVersion"], 1);
        assert_eq!(ready["supportedProtocolVersions"], json!([1, 2]));
        assert_eq!(ready["maxFrameBytes"], 1024 * 1024);
        assert_eq!(ready["maxReassembledFrameBytes"], 64 * 1024 * 1024);
        let metadata = self.next(WAIT);
        assert_eq!(metadata["type"], "available_commands_update");
        assert_eq!(metadata["commands"], commands);
    }

    fn commands(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_available_commands"}));
        self.success(id)["data"]["commands"].clone()
    }

    fn errors(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    fn finish(&mut self, expected_code: i32) {
        self.finish_inner(expected_code, false, true);
    }

    fn finish_inner(&mut self, expected_code: i32, keep_stdin_open: bool, expect_shutdown: bool) {
        self.paused.store(false, Ordering::Release);
        if !keep_stdin_open {
            self.stdin.take();
        }
        let deadline = Instant::now() + WAIT;
        loop {
            match self.frames.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(frame)) => self.seen.push(frame),
                Ok(Err(error)) => panic!("{error}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("RPC did not drain EOF: {error}; stderr: {}", self.errors()),
            }
        }
        let code = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status.code();
            }
            assert!(Instant::now() < deadline, "RPC did not exit; stderr: {}", self.errors());
            std::thread::sleep(Duration::from_millis(10));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert_eq!(code, Some(expected_code), "{}", self.errors());
        assert_eq!(
            self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(),
            usize::from(expect_shutdown)
        );
        if keep_stdin_open {
            assert!(self.stdin.is_some(), "stdin stayed open until process exit");
        }
        self.stdin.take();
    }

    fn close_stdout_pipe(&mut self) {
        self.close_stdout.store(true, Ordering::Release);
        // Wake the reader's existing blocking read; it then drops the actual
        // pipe before the following command provokes another output write.
        self.send(json!({"id":"close-reader","type":"get_state"}));
        assert!(matches!(self.frames.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected)));
        self.readers.remove(0).join().unwrap();
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.paused.store(false, Ordering::Release);
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

async fn upstream(script: Value) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(script).unwrap(), None).await.unwrap()
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn user_texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message["role"] == "user")
        .map(|message| {
            if let Some(text) = message["content"].as_str() {
                return text.to_owned();
            }
            message["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|block| block["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .collect()
}

fn contains_text(message: &Value, expected: &str) -> bool {
    message["content"].as_str().is_some_and(|text| text.contains(expected))
        || message["content"].as_array().is_some_and(|content| {
            content.iter().any(|block| block["text"].as_str().is_some_and(|text| text.contains(expected)))
        })
}

fn invocation_skill(env: &Env, source: &str, name: &str, body: &str) -> PathBuf {
    // Keep ancestor Skills on the real developer machine out of this fixture.
    std::fs::create_dir_all(env.work.path().join(".git")).unwrap();
    let path = env.work.path().join(source).join(name).join("SKILL.md");
    write_skill(&path, name, body);
    path
}

fn write_skill(path: &Path, name: &str, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, format!("---\nname: {name}\ndescription: RPC invocation fixture\n---\n{body}\n")).unwrap();
}

fn command_metadata(name: &str, description: &str) -> Value {
    json!({"name":format!("skill:{name}"),"description":description,"input":{"hint":"arguments"},"source":"skill"})
}

fn proof_commands() -> Value {
    json!([command_metadata("proof", "RPC invocation fixture")])
}

fn metadata_skill(env: &Env, source: &str, name: &str, description: &str, flags: &str) -> PathBuf {
    let path = invocation_skill(env, source, name, "metadata fixture body");
    std::fs::write(&path, format!("---\nname: {name}\ndescription: {description}\n{flags}---\nmetadata body\n"))
        .unwrap();
    path
}

fn query_data(child: &mut RpcChild, id: &str, kind: &str) -> Value {
    child.send(json!({"id":id,"type":kind}));
    child.success(id)["data"].clone()
}

fn branch_rpc(child: &mut RpcChild, id: &str, entry_id: &str, text: &str, commands: Value) {
    let begin = child.seen.len();
    child.send(json!({"id":id,"type":"branch","entryId":entry_id}));
    assert_eq!(child.success(id)["data"], json!({"text":text,"cancelled":false}));
    let updates: Vec<_> =
        child.seen[begin..].iter().filter(|frame| frame["type"] == "available_commands_update").collect();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0]["commands"], commands);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_branch_forks_native_skill_and_tool_history_then_reopens_without_touching_source() {
    let env = Env::new();
    let skill = invocation_skill(&env, ".ara/skills", "proof", "Native fork Skill body");
    let up = upstream(json!({"responses":[
        {"events":[text("Ordinary answer"),finish("stop"),done()]},
        {"events":[tool_call(0,"fork-source-write","write","{\"path\":\"source.txt\",\"content\":\"source effect\"}"),finish("tool_calls"),done()]},
        {"events":[text("Skill answer"),finish("stop"),done()]},
        {"events":[text("Excluded answer"),finish("stop"),done()]},
        {"events":[tool_call(0,"fork-new-write","write","{\"path\":\"fork.txt\",\"content\":\"fork effect\"}"),finish("tool_calls"),done()]},
        {"events":[text("Fork answer"),finish("stop"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"name-fork","type":"set_session_name","name":"Inherited 🦀"}));
    child.success("name-fork");
    for (id, message) in [
        ("native-one", "ordinary retained"),
        ("native-skill", "/skill:proof retained"),
        ("native-selected", "selected excluded α"),
    ] {
        child.send(json!({"id":id,"type":"prompt","message":message}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
    }
    let old = child.state("fork-old");
    let old_file = PathBuf::from(old["sessionFile"].as_str().unwrap());
    let before = std::fs::read(&old_file).unwrap();
    let old_entries = journal(&old_file);
    let selected = old_entries
        .iter()
        .find(|entry| entry["message"]["role"] == "user" && contains_text(&entry["message"], "selected excluded"))
        .unwrap();
    let parent = selected["parentId"].clone();
    let selected_index = old_entries.iter().position(|entry| entry["id"] == selected["id"]).unwrap();
    let retained = old_entries[..selected_index]
        .iter()
        .filter(|entry| entry.get("id").is_some() && entry["type"] != "session")
        .cloned()
        .collect::<Vec<_>>();
    let public = child.messages("fork-old-messages");
    branch_rpc(&mut child, "native-fork", selected["id"].as_str().unwrap(), "selected excluded α", proof_commands());
    let fork = child.state("fork-state");
    assert_ne!(fork["sessionId"], old["sessionId"]);
    assert_eq!(fork["sessionName"], "Inherited 🦀");
    let fork_file = PathBuf::from(fork["sessionFile"].as_str().unwrap());
    assert_ne!(fork_file, old_file);
    assert_eq!(native_header(&journal(&fork_file))["parentSession"], json!(old_file));
    let fork_entries = journal(&fork_file);
    assert_eq!(
        fork_entries
            .iter()
            .filter(|entry| entry.get("id").is_some() && entry["type"] != "session")
            .cloned()
            .collect::<Vec<_>>(),
        retained
    );
    assert_eq!(fork_entries.last().unwrap()["id"], parent);
    assert_eq!(child.messages("fork-public"), public[..public.len() - 2]);
    child.send(json!({"id":"fork-task","type":"prompt","message":"continue fork only"}));
    child.success("fork-task");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let final_public = child.messages("fork-final-public");
    child.finish(0);
    assert_eq!(std::fs::read(&old_file).unwrap(), before);
    assert_eq!(std::fs::read_to_string(env.work.path().join("fork.txt")).unwrap(), "fork effect");
    std::fs::remove_file(skill).unwrap();
    let mut reopened =
        RpcChild::spawn(env.command(&up.base_url(), &["--resume", fork_file.to_str().unwrap(), "--tools", "write"]));
    reopened.ready();
    assert_eq!(reopened.messages("fork-reopened"), final_public);
    reopened.finish(0);
    assert_eq!(std::fs::read(&old_file).unwrap(), before);
    assert_eq!(up.served(), 6);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_memory_fork_keeps_native_ids_duplicate_text_images_and_custom_receipts_without_files() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Memory fork Skill body");
    let up = upstream(json!({"responses":[
        {"events":[text("First answer"),finish("stop"),done()]},
        {"events":[text("Skill answer"),finish("stop"),done()]},
        {"events":[text("Selected answer"),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--no-session"]));
    child.ready_commands(proof_commands());
    for (id, message) in
        [("memory-first", "same text"), ("memory-skill", "/skill:proof memory"), ("memory-selected", "same text")]
    {
        child
            .send(json!({"id":id,"type":"prompt","message":message,"images":[{"data":"aA==","mimeType":"image/png"}]}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
    }
    let old = child.state("memory-old");
    let public = child.messages("memory-public");
    let picks = query_data(&mut child, "memory-picks", "get_branch_messages");
    let picks = picks["messages"].as_array().unwrap();
    assert_eq!(picks.len(), 2);
    assert_ne!(picks[0]["entryId"], picks[1]["entryId"]);
    branch_rpc(&mut child, "memory-fork", picks[1]["entryId"].as_str().unwrap(), "same text", proof_commands());
    let fork = child.state("memory-fork-state");
    assert_ne!(fork["sessionId"], old["sessionId"]);
    assert!(fork.get("sessionFile").is_none());
    assert_eq!(child.messages("memory-kept"), public[..4]);
    assert_eq!(query_data(&mut child, "memory-kept-ids", "get_branch_messages")["messages"], json!([picks[0]]));
    assert!(public[0]["content"].as_array().unwrap().iter().any(|block| block["type"] == "image"));
    assert_eq!(public[2]["role"], "custom");
    child.finish(0);
    assert!(env.sessions().is_empty());
    assert_eq!(up.served(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_branch_joins_held_run_clears_queues_and_busy_page_returns_before_terminal() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("Retained answer"),finish("stop"),done()]},
        {"events":[text("Held branch answer")],"end":"hang"},
        {"events":[text("New fork answer"),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"held-seed","type":"prompt","message":"retained seed"}));
    child.success("held-seed");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let old = child.state("held-origin");
    let old_file = PathBuf::from(old["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"held-selected","type":"prompt","message":"held selected"}));
    child.success("held-selected");
    child.until(
        |frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Held branch answer"),
        WAIT,
    );
    let begin = child.seen.len();
    child.send(json!({"id":"busy-🦀","type":"get_messages_page","limit":1}));
    let busy = child.until(|frame| frame["command"] == "get_messages_page", Duration::from_secs(2));
    assert_eq!(busy["success"], false);
    assert_eq!(busy["id"], "busy-🦀");
    assert_eq!(busy["code"], "session_busy");
    assert_eq!(busy["error"], "Cannot page messages while the session is changing");
    assert!(!child.seen[begin..].iter().any(|frame| frame["type"] == "agent_end"));
    child.send(json!({"id":"invalid-fork","type":"branch","entryId":"missing"}));
    assert_eq!(child.response("invalid-fork")["error"], "Invalid entry ID for branching");
    assert_eq!(child.state("still-held")["isStreaming"], true);
    for kind in ["steer", "follow_up"] {
        child.send(json!({"id":kind,"type":kind,"message":"discard old queued input"}));
        child.success(kind);
    }
    let entries = query_data(&mut child, "held-picks", "get_branch_messages");
    let selected = entries["messages"].as_array().unwrap().last().unwrap()["entryId"].as_str().unwrap();
    branch_rpc(&mut child, "held-fork", selected, "held selected", json!([]));
    let fork = child.state("held-fork-state");
    assert_eq!(fork["queuedMessageCount"], 0);
    assert_eq!(fork["messageCount"], 2);
    let old_after = std::fs::read(&old_file).unwrap();
    let old_entries = journal(&old_file);
    assert_eq!(old_entries.last().unwrap()["message"]["stopReason"], "aborted");
    assert!(!json!(old_entries).to_string().contains("discard old queued input"));
    child.send(json!({"id":"after-held-fork","type":"prompt","message":"fresh fork input"}));
    child.success("after-held-fork");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(std::fs::read(old_file).unwrap(), old_after);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let fresh = requests[2]["body"]["messages"].to_string();
    assert!(fresh.contains("retained seed") && fresh.contains("fresh fork input"));
    assert!(!fresh.contains("held selected") && !fresh.contains("discard old"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_paging_reconstructs_snapshot_and_rename_new_message_and_adoption_stale_cursors() {
    use ara_ai::{Message, UserMessage};
    use ara_session::SessionJournal;
    let env = Env::new();
    let mut native = SessionJournal::create(&env.sessions, env.work.path()).unwrap();
    for i in 0..9 {
        native.append_message(&Message::User(UserMessage::text(format!("page {i}")))).unwrap();
    }
    native.materialize().unwrap();
    let file = native.path().to_path_buf();
    let up = upstream(json!({"responses":[{"events":[text("New message answer"),finish("stop"),done()]}]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    child.ready();
    let all = child.messages("page-all");
    let before = std::fs::read(&file).unwrap();
    let mut cursor = None;
    let mut rebuilt = Vec::new();
    let mut first_cursor = Value::Null;
    for i in 0..5 {
        let id = format!("page-{i}");
        let mut frame = json!({"id":id,"type":"get_messages_page","limit":2});
        if let Some(cursor) = cursor {
            frame["cursor"] = cursor;
        }
        child.send(frame);
        let page = child.success(&id)["data"].clone();
        assert_eq!(page["totalMessages"], 9);
        rebuilt.extend(page["messages"].as_array().unwrap().clone());
        if i == 0 {
            first_cursor = page["nextCursor"].clone();
        }
        cursor = page.get("nextCursor").cloned();
    }
    assert!(cursor.is_none());
    assert_eq!(rebuilt, all);
    assert_eq!(std::fs::read(&file).unwrap(), before);
    child.send(json!({"id":"page-rename","type":"set_session_name","name":"New leaf, same messages"}));
    child.success("page-rename");
    child.send(json!({"id":"page-stale-leaf","type":"get_messages_page","cursor":first_cursor}));
    let stale = child.response("page-stale-leaf");
    assert_eq!(stale["code"], "stale_cursor");
    assert_eq!(stale["error"], "RPC message cursor is stale");
    child.send(json!({"id":"page-current","type":"get_messages_page","limit":1}));
    let current_cursor = child.success("page-current")["data"]["nextCursor"].clone();
    child.send(json!({"id":"page-new-message","type":"prompt","message":"new count"}));
    child.success("page-new-message");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"page-stale-count","type":"get_messages_page","cursor":current_cursor}));
    assert_eq!(child.response("page-stale-count")["code"], "stale_cursor");
    child.adopt("page-new-session", json!({"id":"page-new-session","type":"new_session"}));
    child.send(json!({"id":"page-stale-session","type":"get_messages_page","cursor":first_cursor}));
    assert_eq!(child.response("page-stale-session")["code"], "stale_cursor");
    child.send(json!({"id":"page-invalid","type":"get_messages_page","limit":0}));
    let invalid = child.response("page-invalid");
    assert_eq!(invalid["success"], false);
    assert!(invalid.get("code").is_none());
    child.send(json!({"id":"page-null-limit","type":"get_messages_page","limit":null}));
    assert_eq!(child.success("page-null-limit")["data"], json!({"messages":[],"totalMessages":0}));
    child.finish(0);
    assert_eq!(up.served(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_paging_oversized_first_message_round_trips_through_v2_chunks() {
    use ara_ai::{Message, UserMessage};
    use ara_session::SessionJournal;
    let env = Env::new();
    let mut native = SessionJournal::create(&env.sessions, env.work.path()).unwrap();
    let large = "🦀中文".repeat(140_000);
    native.append_message(&Message::User(UserMessage::text(large.clone()))).unwrap();
    native.append_message(&Message::User(UserMessage::text("second message"))).unwrap();
    native.materialize().unwrap();
    let file = native.path().to_path_buf();
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    child.ready();
    child.send(json!({"id":"page-v2","type":"negotiate_protocol","protocolVersion":2}));
    child.success("page-v2");
    child.send(json!({"id":"page-large","type":"get_messages_page"}));
    let mut decoder = ara_rpc::RpcFrameDecoder::new();
    let mut chunks = 0;
    let page = loop {
        let frame = child.next(WAIT);
        if frame["type"] == "rpc_chunk" {
            chunks += 1;
        }
        if let Some(decoded) = decoder.push(ara_rpc::WireValue::parse(&frame.to_string()).unwrap()).unwrap() {
            let response: Value = serde_json::from_str(&decoded.stringify()).unwrap();
            if response["id"] == "page-large" {
                break response["data"].clone();
            }
        }
    };
    assert!(chunks >= 2, "oversized page uses the negotiated chunk writer");
    assert_eq!(page["totalMessages"], 2);
    assert_eq!(page["messages"].as_array().unwrap().len(), 1);
    assert_eq!(page["messages"][0]["content"], large);
    child.send(json!({"id":"page-large-next","type":"get_messages_page","cursor":page["nextCursor"]}));
    let last = child.success("page-large-next")["data"].clone();
    assert_eq!(last["messages"][0]["content"], "second message");
    assert!(last.get("nextCursor").is_none());
    child.finish(0);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_branch_side_path_root_and_target_failure_preserve_original_identity_and_history() {
    use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
    use ara_session::SessionJournal;
    let env = Env::new();
    let source_dir = tempfile::tempdir().unwrap();
    let mut native = SessionJournal::create(source_dir.path(), env.work.path()).unwrap();
    let root = native.append_message(&Message::User(UserMessage::text("root selection"))).unwrap();
    let mut answer = AssistantMessage::empty("openai-completions", "fixture", "fake-model");
    answer.content.push(AssistantBlock::text("retained root answer"));
    let parent = native.append_message(&Message::Assistant(answer)).unwrap();
    let selected = native.append_message(&Message::User(UserMessage::text("side selection"))).unwrap();
    native.append_message(&Message::User(UserMessage::text("active different branch"))).unwrap();
    native.set_session_name("Root inherited title", "user").unwrap();
    native.materialize().unwrap();
    let file = native.path().to_path_buf();
    let mut entries = journal(&file);
    entries.iter_mut().find(|entry| entry["id"] == root).unwrap()["parentId"] = json!("");
    entries.iter_mut().find(|entry| entry["message"]["content"] == "active different branch").unwrap()["parentId"] =
        json!(root);
    entries.iter_mut().find(|entry| entry["type"] == "session").unwrap()["additionalDirectories"] =
        json!([env.work.path().join("extra")]);
    std::fs::write(&file, entries.iter().map(|entry| format!("{entry}\n")).collect::<String>()).unwrap();
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    child.ready();
    let old = child.state("side-origin");
    let old_messages = child.messages("side-original-messages");
    let before = std::fs::read(&file).unwrap();
    for (id, entry_id) in [("bad-assistant", parent.as_str()), ("bad-missing", "missing")] {
        child.send(json!({"id":id,"type":"branch","entryId":entry_id}));
        assert_eq!(child.response(id)["error"], "Invalid entry ID for branching");
    }
    if env.sessions.exists() {
        std::fs::remove_dir(&env.sessions).unwrap();
    }
    std::fs::write(&env.sessions, "blocked target directory").unwrap();
    child.send(json!({"id":"branch-disk-failure","type":"branch","entryId":selected}));
    assert_eq!(child.response("branch-disk-failure")["success"], false);
    assert_eq!(child.state("side-failed-id")["sessionId"], old["sessionId"]);
    assert_eq!(child.messages("side-failed-messages"), old_messages);
    assert_eq!(std::fs::read(&file).unwrap(), before);
    std::fs::remove_file(&env.sessions).unwrap();
    branch_rpc(&mut child, "side-fork", &selected, "side selection", json!([]));
    let side = child.state("side-fork-state");
    let side_file = PathBuf::from(side["sessionFile"].as_str().unwrap());
    assert_eq!(user_texts(&child.messages("side-fork-messages")), ["root selection"]);
    assert_eq!(child.messages("side-fork-all").len(), 2);
    assert_eq!(native_header(&journal(&side_file))["additionalDirectories"], json!([env.work.path().join("extra")]));
    child.adopt("back-to-root-source", json!({"id":"back-to-root-source","type":"switch_session","sessionPath":file}));
    branch_rpc(&mut child, "root-fork", &root, "root selection", json!([]));
    let root_state = child.state("root-fork-state");
    assert_eq!(root_state["messageCount"], 0);
    assert_eq!(root_state["sessionName"], "Root inherited title");
    let root_file = PathBuf::from(root_state["sessionFile"].as_str().unwrap());
    let root_entries = journal(&root_file);
    assert!(native_header(&root_entries).get("additionalDirectories").is_none());
    assert_eq!(root_entries.iter().filter(|entry| entry["type"] == "title_change").count(), 1);
    assert!(root_entries.iter().all(|entry| entry["type"] != "message"));
    child.finish(0);
    assert_eq!(std::fs::read(file).unwrap(), before);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_native_stats_count_completed_skill_tool_turns_and_branch_queries_use_all_raw_entry_ids() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Use the controlled write tool.");
    let up = upstream(json!({"responses":[
        {"events":[text("Ordinary complete"),finish("stop"),done()]},
        {"events":[tool_call(0,"native-stats-write","write","{\"path\":\"stats-proof.txt\",\"content\":\"native effect\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Skill complete"),finish("stop"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready_commands(proof_commands());
    let empty = query_data(&mut child, "empty-stats", "get_session_stats");
    assert_eq!(empty["totalMessages"], 0);
    assert_eq!(empty["tokens"], json!({"input":0,"output":0,"reasoning":0,"cacheRead":0,"cacheWrite":0,"total":0}));
    assert_eq!(empty["cost"], 0.0);
    assert_eq!(empty["premiumRequests"], 0);
    child.send(json!({"id":"ordinary-native","type":"prompt","message":"ordinary α"}));
    child.success("ordinary-native");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"skill-native","type":"prompt","message":"/skill:proof metadata stats"}));
    assert_eq!(child.success("skill-native")["data"]["agentInvoked"], true);
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let state = child.state("native-query-state");
    let file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    let before = std::fs::read(&file).unwrap();
    let stats = query_data(&mut child, "completed-stats", "get_session_stats");
    assert_eq!(stats["sessionId"], state["sessionId"]);
    assert_eq!(stats["sessionFile"], state["sessionFile"]);
    for (field, expected) in
        [("userMessages", 1), ("assistantMessages", 3), ("toolCalls", 1), ("toolResults", 1), ("totalMessages", 6)]
    {
        assert_eq!(stats[field], expected, "{field}");
    }
    assert_eq!(
        stats["tokens"],
        json!({"input":null,"output":null,"reasoning":null,"cacheRead":null,"cacheWrite":null,"total":null})
    );
    assert!(stats["cost"].is_null() && stats["premiumRequests"].is_null(), "missing usage/pricing is unknown");
    let entries = journal(&file);
    let ordinary = entries.iter().find(|entry| entry["message"]["role"] == "user").unwrap().clone();
    let expected_branch = json!([{"entryId":ordinary["id"],"text":"ordinary α"}]);
    assert_eq!(
        query_data(&mut child, "active-raw-entries", "get_branch_messages"),
        json!({"messages":expected_branch})
    );
    assert_eq!(std::fs::read(&file).unwrap(), before, "read-only native queries do not mutate receipts");
    assert_eq!(std::fs::read_to_string(env.work.path().join("stats-proof.txt")).unwrap(), "native effect\n");
    child.finish(0);

    let mut entries = journal(&file);
    let mut detached = ordinary.clone();
    detached["id"] = json!("abcdef01");
    detached["parentId"] = Value::Null;
    detached["message"]["content"] = json!([
        {"type":"text","text":"left"}, {"type":"image","data":"aA==","mimeType":"image/png"}, {"type":"text","text":"right"}
    ]);
    let mut empty = ordinary.clone();
    empty["id"] = json!("abcdef02");
    empty["parentId"] = json!("abcdef01");
    empty["message"]["content"] = json!([]);
    let position = entries.iter().position(|entry| entry["id"] == ordinary["id"]).unwrap();
    entries.splice(position..position, [detached, empty]);
    std::fs::write(&file, entries.iter().map(|entry| format!("{entry}\n")).collect::<String>()).unwrap();
    let edited = std::fs::read(&file).unwrap();
    let mut resumed =
        RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap(), "--tools", "write"]));
    resumed.ready_commands(proof_commands());
    assert_eq!(
        query_data(&mut resumed, "all-raw-user-entries", "get_branch_messages"),
        json!({"messages":[
            {"entryId":"abcdef01","text":"leftright"}, {"entryId":ordinary["id"],"text":"ordinary α"}
        ]})
    );
    assert_eq!(
        query_data(&mut resumed, "restored-completed-stats", "get_session_stats")["totalMessages"],
        6,
        "off-branch messages are excluded from completed active statistics"
    );
    resumed.finish(0);
    assert_eq!(std::fs::read(&file).unwrap(), edited);
    assert_eq!(up.served(), 3, "raw branch/stat queries never call the model");
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_compacted_stats_use_kept_usage_and_custom_identity_while_branch_lists_raw_users() {
    use ara_ai::{AssistantBlock, AssistantMessage, Message, UserContent, UserMessage};
    use ara_session::{SessionJournal, UserSkillPrompt};
    let env = Env::new();
    let mut native = SessionJournal::create(&env.sessions, env.work.path()).unwrap();
    let first = native.append_message(&Message::User(UserMessage::text("Summarized raw request"))).unwrap();
    let mut answer = AssistantMessage::empty("openai-completions", "fixture", "fake-model");
    answer.content.push(AssistantBlock::text("Completed answer"));
    answer.usage.input = Some(100);
    let first_answer = native.append_message(&Message::Assistant(answer.clone())).unwrap();
    let kept = native
        .append_skill_prompt(&UserSkillPrompt::new(UserContent::Text("Historical Skill body".into()), None))
        .unwrap();
    answer.usage.input = Some(7);
    native.append_message(&Message::Assistant(answer)).unwrap();
    native
        .append_compaction("Summary of the earlier complete turn", &kept, &[first.clone(), first_answer], 100)
        .unwrap();
    native.set_session_name("Compacted retained title", "user").unwrap();
    let file = native.path().to_path_buf();
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    child.ready();
    // Existing startup may persist the launch-selected model before ready.
    let before = std::fs::read(&file).unwrap();
    child.send(json!({"id":"compacted-stats","type":"get_session_stats"}));
    let stats = child.success("compacted-stats")["data"].clone();
    assert_eq!(stats["totalMessages"], 3);
    assert_eq!(stats["userMessages"], 0, "summary and Skill preserve their non-user identity");
    assert_eq!(stats["assistantMessages"], 1);
    assert_eq!(stats["tokens"]["input"], 7, "summarized assistant usage is excluded");
    child.send(json!({"id":"compacted-branches","type":"get_branch_messages"}));
    assert_eq!(
        child.success("compacted-branches")["data"],
        json!({"messages":[{"entryId":first,"text":"Summarized raw request"}]})
    );
    assert_eq!(child.state("compacted-title")["sessionName"], "Compacted retained title");
    child.finish(0);
    assert_eq!(std::fs::read(file).unwrap(), before);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_session_name_during_held_run_acknowledges_before_terminal_and_restores_through_adoption() {
    let env = Env::new();
    let up = upstream(json!({"responses":[{"events":[text("Held while renamed")],"end":"hang"}]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"rename-held","type":"prompt","message":"hold rename lifecycle"}));
    child.success("rename-held");
    child.until(
        |frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Held while renamed"),
        WAIT,
    );
    child.send(json!({"id":"name-held","type":"set_session_name","name":"\u{feff} 标题\t🦀\u{85}新\n 名 \u{a0}"}));
    let ack = child.until(|frame| frame["id"] == "name-held", Duration::from_secs(2));
    assert_eq!(ack["success"], true);
    assert!(ack.get("data").is_none());
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"), "rename does not wait for the held Run");
    let state = child.state("named-held-state");
    assert_eq!(state["sessionName"], "标题 🦀 新 名");
    assert_eq!(state["isStreaming"], true);
    let file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    assert!(!file.exists(), "a rename during the first partial preserves lazy journal materialization");
    for (id, name) in [
        ("name-empty", json!("\u{feff}\t \u{a0}")),
        ("name-control-empty", json!("\u{85}\u{7f}")),
        ("name-type", json!(42)),
    ] {
        child.send(json!({"id":id,"type":"set_session_name","name":name}));
        assert_eq!(child.response(id)["success"], false);
        assert!(!file.exists(), "a rejected rename must not materialize the lazy Session");
    }
    child.send(json!({"id":"abort-renamed","type":"abort"}));
    child.success("abort-renamed");
    let named = std::fs::read(&file).unwrap();
    let first_line = named.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    assert_eq!(first_line, 256, "native fixed title slot remains fixed width");
    let entries = journal(&file);
    assert_eq!(entries[0]["title"], "标题 🦀 新 名");
    assert_eq!(native_header(&entries)["title"], "标题 🦀 新 名");
    let changes: Vec<_> = entries.iter().filter(|entry| entry["type"] == "title_change").collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["title"], "标题 🦀 新 名");
    assert_eq!(changes[0]["source"], "user");
    for (id, name) in [
        ("name-empty", json!("\u{feff}\t \u{a0}")),
        ("name-control-empty", json!("\u{85}\u{7f}")),
        ("name-type", json!(42)),
    ] {
        child.send(json!({"id":id,"type":"set_session_name","name":name}));
        assert_eq!(child.response(id)["success"], false);
        assert_eq!(std::fs::read(&file).unwrap(), named, "rejected name leaves title/journal unchanged");
    }
    child.finish(0);
    let mut resumed = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    resumed.ready();
    assert_eq!(resumed.state("restored-name")["sessionName"], "标题 🦀 新 名");
    resumed.adopt("fresh-name", json!({"id":"fresh-name","type":"new_session"}));
    assert!(resumed.state("fresh-name-state").get("sessionName").is_none());
    resumed.adopt("old-name", json!({"id":"old-name","type":"switch_session","sessionPath":file}));
    assert_eq!(resumed.state("switched-name")["sessionName"], "标题 🦀 新 名");
    resumed.finish(0);
    assert_eq!(up.served(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_ephemeral_name_and_user_entry_ids_are_stable_for_consumed_nonempty_plain_inputs() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Ephemeral custom input");
    let up = upstream(json!({"responses":[
        {"events":[text("Empty user completed"),finish("stop"),done()]},
        {"events":[text("Plain user completed"),finish("stop"),done()]},
        {"events":[text("Skill completed"),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--no-session"]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"ephemeral-name","type":"set_session_name","name":"\u{feff}  临时\t🦀 \u{a0}"}));
    child.success("ephemeral-name");
    assert_eq!(child.state("ephemeral-named")["sessionName"], "临时 🦀");
    for (id, message) in
        [("empty-user", ""), ("plain-user", "same user"), ("skill-user", "/skill:proof user entry metadata")]
    {
        child.send(json!({"id":id,"type":"prompt","message":message}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
    }
    let branch = query_data(&mut child, "ephemeral-entries", "get_branch_messages");
    let entries = branch["messages"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "empty ordinary and custom Skill are not raw user entries");
    assert_eq!(entries[0]["text"], "same user");
    let id = entries[0]["entryId"].as_str().unwrap();
    assert_eq!(id.len(), 8);
    assert!(id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(query_data(&mut child, "ephemeral-entries-again", "get_branch_messages"), branch);
    let stats = query_data(&mut child, "ephemeral-stats", "get_session_stats");
    assert!(stats.get("sessionFile").is_none());
    assert_eq!(stats["userMessages"], 2);
    assert_eq!(stats["totalMessages"], 6);
    child.adopt_commands("ephemeral-reset", json!({"id":"ephemeral-reset","type":"new_session"}), proof_commands());
    assert!(child.state("ephemeral-reset-state").get("sessionName").is_none());
    assert_eq!(query_data(&mut child, "ephemeral-reset-entries", "get_branch_messages"), json!({"messages":[]}));
    child.finish(0);
    assert_eq!(up.served(), 3);
    assert!(env.sessions().is_empty());
}

fn skill_entries(entries: &[Value]) -> Vec<&Value> {
    entries.iter().filter(|entry| entry["type"] == "custom_message" && entry["customType"] == "skill-prompt").collect()
}

fn completed_events(frames: &[Value]) -> Vec<Value> {
    frames.iter().filter(|frame| frame["type"] == "message_end").map(|frame| frame["message"].clone()).collect()
}

fn assert_skill_public(message: &Value, original: &str, body: &str) {
    assert_eq!(message["role"], "custom", "{message}");
    assert_eq!(message["customType"], "skill-prompt");
    assert_eq!(message["display"], true);
    assert_eq!(message["attribution"], "user");
    assert_eq!(message["details"]["name"], "proof");
    assert_eq!(message["details"]["originalText"], original);
    assert!(contains_text(message, body), "fresh body in public Skill: {message}");
    assert!(message["timestamp"].as_i64().is_some());
}

fn assert_no_local_prompt_result(frames: &[Value], ids: &[&str]) {
    assert!(
        !frames.iter().any(|frame| { frame["type"] == "prompt_result" && ids.iter().any(|id| frame["id"] == *id) }),
        "successfully invoked or queued prompts must not report a local skip: {frames:?}"
    );
}

fn provider_users_without_reminder(messages: &[Value], cwd: &Path) -> Vec<Value> {
    let mut users: Vec<_> = messages.iter().filter(|message| message["role"] == "user").cloned().collect();
    let first = users.first_mut().expect("provider has a user input");
    // The existing CliHooks intentionally prepends this one date/cwd block to
    // the first provider user turn. Check it exactly, then compare all actual
    // user content and image blocks without that provider-only reminder.
    let reminder = if let Some(text) = first["content"].as_str() {
        let (reminder, content) = text.split_once("\n\n").expect("plain first-user reminder separator");
        let reminder = reminder.to_owned();
        first["content"] = json!(content);
        reminder
    } else {
        let content = first["content"].as_array_mut().expect("typed provider first user");
        let reminder = content.remove(0);
        assert_eq!(reminder["type"], "text");
        reminder["text"].as_str().unwrap().to_owned()
    };
    let date = reminder.strip_prefix("<system-reminder>\nToday: ").unwrap().split_once(';').unwrap().0;
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").expect("actual child reminder date");
    let absolute = std::fs::canonicalize(cwd).unwrap();
    let absolute = absolute.to_string_lossy();
    let normalized = absolute.strip_prefix("\\\\?\\").unwrap_or(&absolute).replace('\\', "/");
    assert_eq!(
        reminder,
        format!(
            "<system-reminder>\nToday: {date}; current working directory: '{normalized}'. Do not repeat this information in your reply.\n</system-reminder>"
        )
    );
    users
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_protocol_recoverable_input_errors_and_explicit_unsupported_commands() {
    let env = Env::new();
    let skill = invocation_skill(&env, ".ara/skills", "proof", "Do the task.");
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    child.send_raw("{broken\n");
    let malformed = child.until(|frame| frame["type"] == "response", WAIT);
    assert_eq!(malformed["success"], false);
    assert!(malformed["error"].as_str().unwrap().contains("parse"));
    child.send(json!({"id":"bad-version","type":"negotiate_protocol","protocolVersion":3}));
    assert_eq!(child.response("bad-version")["success"], false);
    child.send(json!({"id":"v2-中文-🦀","type":"negotiate_protocol","protocolVersion":2}));
    assert_eq!(child.success("v2-中文-🦀")["data"]["protocolVersion"], 2);
    for command in ["bash", "compact", "set_model", "does_not_exist"] {
        child.send(json!({"id":command,"type":command}));
        let response = child.response(command);
        assert_eq!(response["command"], command);
        assert_eq!(response["success"], false, "{response}");
        assert!(!response["error"].as_str().unwrap().is_empty());
    }
    child.send(json!({"id":"bad-switch","type":"switch_session"}));
    assert_eq!(child.response("bad-switch")["success"], false);
    child.send(json!({"id":"bad-parent","type":"new_session","parentSession":123}));
    assert_eq!(child.response("bad-parent")["success"], false);
    child.send(json!({"id":"bad-mode","type":"set_follow_up_mode","mode":"bogus"}));
    assert_eq!(child.response("bad-mode")["success"], false);
    // Registered Skill admission re-reads the file before ACK. A stale
    // snapshot must reject its deleted file rather than invoke or fall back.
    std::fs::remove_file(&skill).unwrap();
    child.send(json!({"id":"skill","type":"prompt","message":"/skill:proof task"}));
    let missing = child.response("skill");
    assert_eq!(missing["success"], false);
    assert!(missing["error"].as_str().unwrap().contains("proof"));
    assert!(missing.get("data").is_none(), "no successful invocation ACK: {missing}");
    child.send(json!({"type":"get_state"}));
    let state = child.until(|frame| frame["type"] == "response" && frame["command"] == "get_state", WAIT);
    assert_eq!(state["success"], true);
    assert!(state.get("id").is_none(), "omitted id stays omitted: {state}");
    assert_eq!(state["data"]["messageCount"], 0);
    child.finish(0);
    assert_eq!(up.served(), 0, "protocol errors and a deleted Skill never reach inference");
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_start"));
    assert!(env.sessions().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_skill_command_metadata_preserves_registered_order_hidden_unicode_and_filters() {
    let env = Env::new();
    metadata_skill(&env, ".ara/skills", "Alpha", "Native duplicate winner", "");
    metadata_skill(&env, ".agents/skills", "Alpha", "Excluded duplicate loser", "");
    metadata_skill(&env, ".agents/skills", "beta", "Agents entry", "");
    metadata_skill(&env, ".codex/skills", "z-hidden", "Hidden command remains invocable", "hide: true\n");
    metadata_skill(&env, ".claude/skills", "编排🦀", "中文说明 🦀", "");
    metadata_skill(&env, ".opencode/skills", "ghost", "Disabled source", "");
    metadata_skill(&env, ".ara/skills", "disabled", "Disabled frontmatter", "enabled: false\n");
    let alpha = command_metadata("Alpha", "Native duplicate winner");
    let beta = command_metadata("beta", "Agents entry");
    let hidden = command_metadata("z-hidden", "Hidden command remains invocable");
    let unicode = command_metadata("编排🦀", "中文说明 🦀");
    let up = upstream(json!({"responses":[]})).await;
    for (args, expected) in [
        (vec![], json!([alpha, beta, hidden, unicode])),
        (vec!["--no-skills"], json!([])),
        (vec!["--skill-sources", "agents"], json!([alpha, beta])),
        (vec!["--skills", "*hidden"], json!([hidden])),
    ] {
        let mut child = RpcChild::spawn(env.command(&up.base_url(), &args));
        child.ready_commands(expected.clone());
        let id = json!({"metadata":"中文🦀","parts":[null,42]});
        let begin = child.seen.len();
        child.send(json!({"id":id,"type":"get_available_commands"}));
        let reply = child.until(|frame| frame["type"] == "response" && frame["id"] == id, WAIT);
        assert_eq!(reply["success"], true);
        assert_eq!(reply["data"], json!({"commands":expected}));
        assert_eq!(child.seen[begin..], [reply], "metadata query has no Agent or update effects");
        child.finish(0);
        assert!(env.sessions().is_empty());
    }
    assert!(up.requests.lock().await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_skill_command_snapshot_survives_removal_until_successful_adoption_refresh() {
    let env = Env::new();
    let skill = invocation_skill(&env, ".ara/skills", "proof", "Initial metadata body");
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    std::fs::remove_file(skill).unwrap();
    assert_eq!(child.commands("removed-list"), proof_commands());
    child.send(json!({"id":"removed-invoke","type":"prompt","message":"/skill:proof removed"}));
    let failed = child.response("removed-invoke");
    assert_eq!(failed["success"], false);
    assert!(failed.get("data").is_none());
    let next = metadata_skill(&env, ".agents/skills", "next", "New snapshot", "");
    let begin = child.seen.len();
    child.send(
        json!({"id":"missing-adopt","type":"switch_session","sessionPath":env.home.path().join("missing.jsonl")}),
    );
    assert_eq!(child.response("missing-adopt")["success"], false);
    assert_eq!(child.commands("failed-list"), proof_commands());
    assert!(!child.seen[begin..].iter().any(|frame| frame["type"] == "available_commands_update"));
    assert!(env.sessions().is_empty());
    child.adopt_commands(
        "refresh-new",
        json!({"id":"refresh-new","type":"new_session"}),
        json!([command_metadata("next", "New snapshot")]),
    );
    let file = PathBuf::from(child.state("snapshot-file")["sessionFile"].as_str().unwrap());
    let before = std::fs::read(&file).unwrap();
    std::fs::write(next, "---\nname: next\ndescription: Refreshed after switch\n---\nnext body\n").unwrap();
    assert_eq!(child.commands("unchanged-before-switch"), json!([command_metadata("next", "New snapshot")]));
    child.adopt_commands(
        "refresh-switch",
        json!({"id":"refresh-switch","type":"switch_session","sessionPath":file}),
        json!([command_metadata("next", "Refreshed after switch")]),
    );
    assert_eq!(child.commands("switched-list"), json!([command_metadata("next", "Refreshed after switch")]));
    assert_eq!(std::fs::read(&file).unwrap(), before, "metadata query/refresh never journals model input");
    child.finish(0);
    assert_eq!(up.served(), 0);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_start"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_known_skill_uses_fresh_body_ignores_images_and_keeps_public_receipt() {
    let env = Env::new();
    let path = invocation_skill(&env, ".ara/skills", "proof", "Obsolete discovery body.");
    let up = upstream(json!({"responses":[
        {"events":[text("Fresh Skill completed."),finish("stop"),done()]},
        {"events":[text("Plain continuation completed."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    write_skill(&path, "proof", "Fresh body after ready.\nSecond fresh line.");
    let original = "prefix\u{b}/skill:proof  focus";
    let id = json!({"request":"skill-中文-🦀","parts":[42,true,null]});
    let begin = child.seen.len();
    child.send(
        json!({"id":id,"type":"prompt","message":original,"images":{"malformed":"ignored for a registered Skill"}}),
    );
    let ack = child.until(|frame| frame["type"] == "response" && frame["id"] == id, WAIT);
    assert_eq!(ack["success"], true, "{ack}");
    assert_eq!(ack["data"], json!({"agentInvoked":true}));
    assert!(!child.seen[begin..].iter().any(|frame| frame["type"] == "agent_start"), "ACK precedes dispatch");
    let end = child.until(|frame| frame["type"] == "agent_end", WAIT);
    let completed = completed_events(&child.seen[begin..]);
    assert_eq!(end["messages"], json!(completed));
    assert_eq!(completed.len(), 2);
    assert_skill_public(&completed[0], original, "Fresh body after ready.");
    assert_eq!(completed[0]["details"]["args"], "prefix focus");
    assert_eq!(completed[0]["details"]["lineCount"], 2);
    assert_eq!(
        std::fs::canonicalize(completed[0]["details"]["path"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(&path).unwrap()
    );
    assert!(!contains_text(&completed[0], "Obsolete discovery body."));
    let starts: Vec<_> = child.seen[begin..]
        .iter()
        .filter(|frame| frame["type"] == "message_start" && frame["message"]["role"] == "custom")
        .collect();
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0]["message"], completed[0]);
    assert_eq!(child.messages("fresh-public"), completed);
    // A second Run's terminal contains just that Run, not a global slice which
    // could match an equal earlier Skill message.
    let second_begin = child.seen.len();
    child.send(json!({"id":"plain-after-skill","type":"prompt","message":"plain after Skill"}));
    child.success("plain-after-skill");
    let second_end = child.until(|frame| frame["type"] == "agent_end", WAIT);
    let second_completed = completed_events(&child.seen[second_begin..]);
    assert_eq!(second_end["messages"], json!(second_completed));
    assert_eq!(second_completed.len(), 2);
    assert_eq!(second_completed[0]["role"], "user");
    child.finish(0);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "prompt_result" && frame["id"] == id));
    let entries = journal(&env.sessions()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 1);
    for field in ["customType", "content", "details", "display", "attribution"] {
        assert_eq!(skills[0][field], completed[0][field], "journal/public {field}");
    }
    assert_eq!(
        entries.iter().filter(|entry| entry["message"]["role"] == "user").count(),
        1,
        "only the plain turn is an ordinary user receipt"
    );
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let users = user_texts(&provider_users_without_reminder(
        requests[0]["body"]["messages"].as_array().unwrap(),
        env.work.path(),
    ));
    assert_eq!(users, [completed[0]["content"].as_str().unwrap()]);
    let provider_history = requests[1]["body"]["messages"].to_string();
    assert!(!provider_history.contains("originalText") && !provider_history.contains("customType"));
    assert!(!provider_history.contains("malformed") && !provider_history.contains("Obsolete discovery body."));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_unknown_disabled_filtered_and_builtin_skill_text_stays_ordinary_with_images() {
    for (case, source, args, input) in [
        ("unknown", ".ara/skills", Vec::<&str>::new(), "/skill:missing task"),
        ("disabled", ".ara/skills", vec!["--no-skills"], "/skill:proof task"),
        ("filtered", ".ara/skills", vec!["--skills", "unmatched*"], "/skill:proof task"),
        ("source-off", ".agents/skills", vec!["--skill-sources", ""], "/skill:proof task"),
        ("builtin-args", ".ara/skills", Vec::<&str>::new(), "/help /skill:proof task"),
    ] {
        let env = Env::new();
        invocation_skill(&env, source, "proof", "Must not expand this body.");
        let up = upstream(json!({"responses":[{"events":[text("Ordinary completed."),finish("stop"),done()]}]})).await;
        let mut child = RpcChild::spawn(env.command(&up.base_url(), &args));
        child.ready_commands(if matches!(case, "unknown" | "builtin-args") { proof_commands() } else { json!([]) });
        child.send(json!({"id":"bad-images","type":"prompt","message":input,"images":42}));
        let error = child.response("bad-images");
        assert_eq!(error["success"], false, "{case}: {error}");
        assert!(error["error"].as_str().unwrap().contains("images"));
        assert_eq!(child.state("before-ordinary")["messageCount"], 0, "{case}");
        child.send(json!({"id":case,"type":"prompt","message":input,"images":[{"data":"iVBORw0KGgo=","mimeType":"image/png"}]}));
        let ack = child.success(case);
        assert!(ack.get("data").is_none(), "ordinary ACK has no Skill admission flag: {case}: {ack}");
        let end = child.until(|frame| frame["type"] == "agent_end", WAIT);
        let messages = child.messages("ordinary-public");
        assert_eq!(messages.len(), 2, "{case}");
        assert_eq!(end["messages"], json!(messages));
        assert_eq!(messages[0]["role"], "user", "{case}");
        assert_eq!(messages[0]["content"][0]["text"], input);
        assert_eq!(messages[0]["content"][1]["type"], "image");
        child.finish(0);
        let entries = journal(&env.sessions()[0]);
        assert!(skill_entries(&entries).is_empty(), "{case}");
        assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 1, "{case}");
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 1, "{case}");
        let users =
            provider_users_without_reminder(requests[0]["body"]["messages"].as_array().unwrap(), env.work.path());
        assert_eq!(users.len(), 1, "{case}");
        assert_eq!(users[0]["content"][0]["text"], input);
        assert_eq!(users[0]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_skill_queues_preserve_custom_and_plain_order_and_eof_drain() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Queued Skill body.");
    let up = upstream(json!({"responses":[
        {"events":[text("Queue window."),{"sleep_ms":1500},finish("stop"),done()]},
        {"events":[text("Custom steering."),finish("stop"),done()]},
        {"events":[text("Plain steering."),finish("stop"),done()]},
        {"events":[text("Both followups."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"follow-all","type":"set_follow_up_mode","mode":"all"}));
    child.success("follow-all");
    let begin = child.seen.len();
    child.send(json!({"id":"initial","type":"prompt","message":"initial queue owner"}));
    child.success("initial");
    child.until(|frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Queue window."), WAIT);
    let first_original = "/skill:proof twin";
    let second_original = " /skill:proof twin ";
    child.send(json!({"id":"skill-steer","type":"prompt","message":first_original}));
    assert_eq!(child.success("skill-steer")["data"], json!({"agentInvoked":true}));
    child.send(json!({"id":"plain-steer","type":"steer","message":"plain steering input"}));
    child.success("plain-steer");
    child.send(json!({"id":"skill-follow","type":"prompt","message":second_original,"streamingBehavior":"followUp","images":false}));
    assert_eq!(child.success("skill-follow")["data"], json!({"agentInvoked":true}));
    child.send(json!({"id":"plain-follow","type":"follow_up","message":"plain followup input"}));
    child.success("plain-follow");
    let queued = child.state("all-four-queued");
    assert_eq!(queued["queuedMessageCount"], 4);
    assert_eq!(queued["steeringMode"], "one-at-a-time");
    assert_eq!(queued["followUpMode"], "all");
    // Close stdin before queue consumption: accepted custom envelopes must
    // still drain using their original Session and independent provenance.
    child.finish(0);
    let completed = completed_events(&child.seen[begin..]);
    let ends: Vec<_> = child.seen[begin..].iter().filter(|frame| frame["type"] == "agent_end").collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["messages"], json!(completed));
    let inputs: Vec<_> =
        completed.iter().filter(|message| message["role"] == "custom" || message["role"] == "user").collect();
    assert_eq!(inputs.len(), 5);
    assert_eq!(inputs[0]["content"], "initial queue owner");
    assert_skill_public(inputs[1], first_original, "Queued Skill body.");
    assert_eq!(inputs[2]["content"], "plain steering input");
    assert_skill_public(inputs[3], second_original, "Queued Skill body.");
    assert_eq!(inputs[4]["content"], "plain followup input");
    assert_eq!(inputs[1]["content"], inputs[3]["content"], "equal provider text keeps distinct originalText metadata");
    let starts: Vec<_> = child.seen[begin..]
        .iter()
        .filter(|frame| frame["type"] == "message_start" && frame["message"]["role"] == "custom")
        .map(|frame| &frame["message"])
        .collect();
    assert_eq!(starts, [inputs[1], inputs[3]]);
    assert_no_local_prompt_result(&child.seen, &["initial", "skill-steer", "skill-follow"]);
    let entries = journal(&env.sessions()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 2);
    assert_eq!(skills[0]["details"], inputs[1]["details"]);
    assert_eq!(skills[1]["details"], inputs[3]["details"]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 3);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    let users = |index: usize| {
        user_texts(&provider_users_without_reminder(
            requests[index]["body"]["messages"].as_array().unwrap(),
            env.work.path(),
        ))
    };
    assert_eq!(users(1), ["initial queue owner", inputs[1]["content"].as_str().unwrap()]);
    assert_eq!(users(2).last().unwrap(), "plain steering input");
    assert_eq!(users(3).last().unwrap(), "plain followup input");
    assert_eq!(users(3).iter().filter(|text| text.as_str() == inputs[1]["content"].as_str().unwrap()).count(), 2);
    assert!(requests.iter().all(|request| !request["body"]["messages"].to_string().contains("originalText")));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_direct_queue_and_abort_prompt_keep_skill_text_and_typed_images_ordinary() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Must not directly invoke.");
    let up = upstream(json!({"responses":[
        {"events":[text("Direct queue window."),{"sleep_ms":1200},finish("stop"),done()]},
        {"events":[text("Direct steering."),finish("stop"),done()]},
        {"events":[text("Direct followup."),finish("stop"),done()]},
        {"events":[text("Ordinary abort prompt."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"initial","type":"prompt","message":"ordinary initial"}));
    child.success("initial");
    child.until(
        |frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Direct queue window."),
        WAIT,
    );
    for (id, kind) in [("direct-steer", "steer"), ("direct-follow", "follow_up")] {
        child.send(json!({"id":format!("bad-{id}"),"type":kind,"message":"/skill:proof direct","images":{}}));
        assert_eq!(child.response(&format!("bad-{id}"))["success"], false);
        child.send(json!({"id":id,"type":kind,"message":format!("/skill:proof {id}"),"images":[{"data":"iVBORw0KGgo=","mimeType":"image/png"}]}));
        assert!(child.success(id).get("data").is_none());
    }
    assert_eq!(child.state("direct-queued")["queuedMessageCount"], 2);
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"direct-abort-prompt","type":"abort_and_prompt","message":"/skill:proof direct-abort","streamingBehavior":42,"images":[{"data":"iVBORw0KGgo=","mimeType":"image/png"}]}));
    assert!(child.success("direct-abort-prompt").get("data").is_none());
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let messages = child.messages("direct-public");
    assert!(messages.iter().all(|message| message["role"] != "custom"));
    assert_eq!(
        user_texts(&messages),
        ["ordinary initial", "/skill:proof direct-steer", "/skill:proof direct-follow", "/skill:proof direct-abort"]
    );
    for message in messages.iter().filter(|message| message["role"] == "user").skip(1) {
        assert_eq!(message["content"][1]["type"], "image");
    }
    child.finish(0);
    let entries = journal(&env.sessions()[0]);
    assert!(skill_entries(&entries).is_empty());
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 4);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    let last_users: Vec<_> = requests[3]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .collect();
    for message in last_users.iter().skip(1) {
        assert_eq!(message["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_skill_public_identity_restores_active_branch_after_source_deletion() {
    let env = Env::new();
    let path = invocation_skill(&env, ".ara/skills", "proof", "Historical saved Skill body.");
    let up = upstream(json!({"responses":[
        {"events":[text("Saved Skill answer."),finish("stop"),done()]},
        {"events":[text("Resumed saved identity."),finish("stop"),done()]}
    ]}))
    .await;
    let mut seed = RpcChild::spawn(env.command(&up.base_url(), &[]));
    seed.ready_commands(proof_commands());
    seed.send(json!({"id":"seed-skill","type":"prompt","message":"/skill:proof historical args"}));
    assert_eq!(seed.success("seed-skill")["data"], json!({"agentInvoked":true}));
    seed.until(|frame| frame["type"] == "agent_end", WAIT);
    let saved_public = seed.messages("saved-public");
    seed.finish(0);
    let file = env.sessions().pop().unwrap();
    let mut entries = journal(&file);
    let inactive = json!({"type":"custom_message","id":"detached-skill-fixture","parentId":null,"timestamp":"2026-09-27T00:00:00.123Z","customType":"skill-prompt","content":"Detached branch must stay invisible.","display":true,"attribution":"user","details":{"name":"proof","originalText":"/skill:proof detached"}});
    let first_context = entries.iter().position(|entry| entry["type"] == "custom_message").unwrap();
    entries.insert(first_context, inactive);
    std::fs::write(&file, entries.iter().map(|entry| format!("{entry}\n")).collect::<String>()).unwrap();
    std::fs::remove_file(path).unwrap();
    let mut resumed = RpcChild::spawn(env.command(&up.base_url(), &["--resume", file.to_str().unwrap()]));
    resumed.ready();
    assert_eq!(
        resumed.messages("restored-public"),
        saved_public,
        "restore recorded content/details/timestamp without reopening the deleted Skill"
    );
    assert_skill_public(&saved_public[0], "/skill:proof historical args", "Historical saved Skill body.");
    let begin = resumed.seen.len();
    resumed.send(json!({"id":"continuation","type":"prompt","message":"continue saved history"}));
    resumed.success("continuation");
    let end = resumed.until(|frame| frame["type"] == "agent_end", WAIT);
    assert_eq!(end["messages"], json!(completed_events(&resumed.seen[begin..])));
    assert_eq!(end["messages"].as_array().unwrap().len(), 2, "native history is not re-emitted as this Run");
    let continued = resumed.messages("continued-public");
    assert_eq!(&continued[..saved_public.len()], saved_public.as_slice());
    resumed.adopt("blank-after-skill", json!({"id":"blank-after-skill","type":"new_session"}));
    assert!(resumed.messages("blank-public").is_empty());
    resumed.adopt("switch-saved-skill", json!({"id":"switch-saved-skill","type":"switch_session","sessionPath":file}));
    assert_eq!(resumed.messages("switched-public"), continued);
    resumed.finish(0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let history = requests[1]["body"]["messages"].as_array().unwrap();
    assert_eq!(
        user_texts(&provider_users_without_reminder(history, env.work.path())),
        [saved_public[0]["content"].as_str().unwrap(), "continue saved history"]
    );
    assert!(
        !json!(history).to_string().contains("Detached branch") && !json!(history).to_string().contains("originalText")
    );
    assert_eq!(
        skill_entries(&journal(&file)).len(),
        2,
        "one active and one intentionally detached receipt, no replay duplicate"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_live_partial_queries_precede_completion_and_abort_joins_the_run() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Metadata while the ordinary Run is held.");
    let up = upstream(json!({"responses":[
        {"events":[text("Held partial." )],"end":"hang"},
        {"events":[text("After abort."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"held","type":"prompt","message":"hold this request"}));
    child.success("held");
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_start"), "ACK precedes run events");
    let update = child
        .until(|frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Held partial."), WAIT);
    assert_eq!(update["message"]["role"], "assistant");
    assert!(
        contains_text(&update["assistantMessageEvent"]["partial"], "Held partial."),
        "full partial, not print projection: {update}"
    );
    child.send(json!({"id":"live-state","type":"get_state"}));
    let state = child.until(|frame| frame["id"] == "live-state", Duration::from_secs(2));
    assert_eq!(state["success"], true);
    assert_eq!(state["data"]["isStreaming"], true);
    assert_eq!(state["data"]["messageCount"], 1);
    child.send(json!({"id":"live-messages","type":"get_messages"}));
    let messages = child.until(|frame| frame["id"] == "live-messages", Duration::from_secs(2));
    assert_eq!(messages["success"], true);
    assert_eq!(user_texts(messages["data"]["messages"].as_array().unwrap()), ["hold this request"]);
    assert_eq!(messages["data"]["messages"].as_array().unwrap().len(), 1, "partial remains separate");
    let journals_before: Vec<_> = env.sessions().iter().map(|path| std::fs::read(path).unwrap()).collect();
    let query_begin = child.seen.len();
    child.send(json!({"id":"held-metadata","type":"get_available_commands"}));
    let metadata = child.until(|frame| frame["id"] == "held-metadata", Duration::from_secs(2));
    assert_eq!(metadata["success"], true);
    assert_eq!(metadata["data"], json!({"commands":proof_commands()}));
    assert_eq!(child.seen[query_begin..], [metadata]);
    assert_eq!(env.sessions().iter().map(|path| std::fs::read(path).unwrap()).collect::<Vec<_>>(), journals_before);
    assert_eq!(up.requests.lock().await.len(), 1);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"));
    child.send(json!({"id":"abort","type":"abort"}));
    child.success("abort");
    let end = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let abort = child.seen.iter().position(|frame| frame["id"] == "abort").unwrap();
    assert!(end < abort, "abort response is after owned Run settlement");
    child.send(json!({"id":"idle","type":"get_state"}));
    assert_eq!(child.success("idle")["data"]["isStreaming"], false);
    child.send(json!({"id":"next","type":"prompt","message":"continue after abort"}));
    child.success("next");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"last","type":"get_last_assistant_text"}));
    assert_eq!(child.success("last")["data"]["text"], "After abort.");
    child.finish(0);
    assert_eq!(up.served(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_same_agent_retains_prior_context_and_direct_images() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("Remembered first."),finish("stop"),done()]},
        {"events":[text("Saw the picture and history."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"first","type":"prompt","message":"remember RPC history"}));
    child.success("first");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"image","type":"prompt","message":"describe picture with prior context",
        "images":[{"type":"image","data":"iVBORw0KGgo=","mimeType":"image/png"}]}));
    child.success("image");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"all","type":"get_messages"}));
    let response = child.success("all");
    let messages = response["data"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(user_texts(messages), ["remember RPC history", "describe picture with prior context"]);
    assert_eq!(messages[2]["content"][1]["type"], "image");
    child.finish(0);
    let requests = up.requests.lock().await;
    let history = requests[1]["body"]["messages"].as_array().unwrap();
    assert!(history.iter().any(|message| contains_text(message, "remember RPC history")));
    assert!(history.iter().any(|message| contains_text(message, "Remembered first.")));
    assert!(
        history.iter().any(|message| message["content"].as_array().is_some_and(|content| {
            content.iter().any(|block| {
                block["type"] == "image_url" && block["image_url"]["url"] == "data:image/png;base64,iVBORw0KGgo="
            })
        })),
        "direct image reaches provider: {history:?}"
    );
    assert_eq!(env.sessions().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_queue_modes_steering_followup_and_eof_drain() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("Before queues."),{"sleep_ms":700},finish("stop"),done()]},
        {"events":[text("Steered one."),finish("stop"),done()]},
        {"events":[text("Steered two."),finish("stop"),done()]},
        {"events":[text("Both followups."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"mode","type":"set_follow_up_mode","mode":"all"}));
    child.success("mode");
    child.send(json!({"id":"start","type":"prompt","message":"initial"}));
    child.success("start");
    child.until(|frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Before queues."), WAIT);
    for (id, kind, message) in [
        ("s1", "steer", "steer-one"),
        ("s2", "steer", "steer-two"),
        ("f1", "follow_up", "follow-one"),
        ("f2", "follow_up", "follow-two"),
    ] {
        child.send(json!({"id":id,"type":kind,"message":message}));
        child.success(id);
    }
    child.send(json!({"id":"queued","type":"get_state"}));
    let state = child.success("queued");
    assert_eq!(state["data"]["queuedMessageCount"], 4);
    assert_eq!(state["data"]["steeringMode"], "one-at-a-time");
    assert_eq!(state["data"]["followUpMode"], "all");
    child.finish(0);
    assert_eq!(up.served(), 4);
    let requests = up.requests.lock().await;
    let history = |index: usize| user_texts(requests[index]["body"]["messages"].as_array().unwrap());
    assert_eq!(history(1).iter().filter(|text| text.as_str() == "steer-one").count(), 1);
    assert!(!history(1).iter().any(|text| text == "steer-two" || text == "follow-one"));
    assert!(history(2).iter().any(|text| text == "steer-two"));
    assert!(!history(2).iter().any(|text| text == "follow-one"));
    assert!(history(3).iter().any(|text| text == "follow-one"));
    assert!(history(3).iter().any(|text| text == "follow-two"));
    let entries = journal(&env.sessions()[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 5);
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_late_idle_followup_runs_without_a_new_prompt_and_drains_eof() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("First settled."),finish("stop"),done()]},
        {"events":[text("Late queue settled."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"first","type":"prompt","message":"initial"}));
    child.success("first");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"late","type":"follow_up","message":"late queued user"}));
    child.success("late");
    child.finish(0);
    assert_eq!(up.served(), 2);
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_end").count(), 2);
    let requests = up.requests.lock().await;
    assert!(
        user_texts(requests[1]["body"]["messages"].as_array().unwrap()).iter().any(|text| text == "late queued user")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_provider_failure_is_correlated_and_next_prompt_remains_usable() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Skill before controlled provider failure.");
    let up = upstream(json!({"responses":[
        {"status":400,"body":"{\"error\":{\"message\":\"RPC controlled failure\"}}"},
        {"events":[text("Recovered next prompt."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready_commands(proof_commands());
    let id = json!({"request":"failure-中文","parts":[false,123,null]});
    child.send(json!({"id":id,"type":"prompt","message":"/skill:proof trigger failure"}));
    let ack = child.until(|frame| frame["type"] == "response" && frame["id"] == id, WAIT);
    assert_eq!(ack["success"], true);
    assert_eq!(ack["data"], json!({"agentInvoked":true}));
    let failure = child.until(|frame| frame["id"] == id && frame["success"] == false, WAIT);
    assert_eq!(failure["command"], "prompt");
    assert!(failure["error"].as_str().unwrap().contains("RPC controlled failure"), "{failure}");
    let terminal = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let late_error = child.seen.iter().position(|frame| frame["id"] == id && frame["success"] == false).unwrap();
    assert!(terminal < late_error, "late correlated failure follows owned Run settlement");
    let failed_public = child.messages("failed-skill-public");
    assert_skill_public(&failed_public[0], "/skill:proof trigger failure", "Skill before controlled provider failure.");
    child.send(json!({"id":"recovery","type":"prompt","message":"next task"}));
    child.success("recovery");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(up.served(), 2);
    let entries = journal(&env.sessions()[0]);
    assert_eq!(skill_entries(&entries).len(), 1, "provider failure retains one admitted custom prompt");
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 1);
    assert!(entries.iter().any(|entry| entry["message"]["stopReason"] == "error"));
    assert!(entries.iter().any(|entry| contains_text(&entry["message"], "Recovered next prompt.")));
    assert!(!child.seen.iter().any(|frame| frame["type"] == "prompt_result" && frame["id"] == id));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_abort_and_prompt_executes_while_stdout_is_backpressured() {
    let env = Env::new();
    // Keep identical snapshot/pipe pressure using numeric data. Repeated
    // alphabetic prose is now correctly interrupted by the fixed loop guard.
    let mut held: Vec<Value> = (0..40).map(|_| text(&"0".repeat(4096))).collect();
    held.extend([
        tool_call(
            0,
            "pressure-marker",
            "write",
            "{\"path\":\"pressure-ready.txt\",\"content\":\"all snapshots emitted\\n\"}",
        ),
        finish("tool_calls"),
        done(),
    ]);
    let up = upstream(json!({"responses":[
        {"events":held},
        {"events":[],"end":"hang"},
        {"events":[tool_call(0,"after-abort","write","{\"path\":\"after-abort.txt\",\"content\":\"control reached\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Control completed."),finish("stop"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready();
    child.paused.store(true, Ordering::Release);
    child.send(json!({"id":"large","type":"prompt","message":"large held output"}));
    let pressure_marker = env.work.path().join("pressure-ready.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    while up.served() < 2 || !pressure_marker.exists() {
        assert!(Instant::now() < deadline, "large snapshots blocked tool/next-call progress while stdout was paused");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // These cumulative snapshots fill the OS pipe even if its reader consumed
    // one in-flight frame just before observing the pause flag. The marker's
    // tool ran after all snapshots; the following held request is already live.
    assert_eq!(std::fs::read_to_string(pressure_marker).unwrap(), "all snapshots emitted\n");
    child.send(json!({"id":"replacement","type":"abort_and_prompt","message":"write control proof"}));
    let artifact = env.work.path().join("after-abort.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !artifact.exists() {
        assert!(
            Instant::now() < deadline,
            "control waited for stdout flush; requests: {}; stderr: {}",
            up.served(),
            child.errors()
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(child.paused.load(Ordering::Acquire));
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "control reached\n");
    child.paused.store(false, Ordering::Release);
    child.success("large");
    child.success("replacement");
    child.finish(0);
    assert_eq!(up.served(), 4);
    let initial_end = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let starts: Vec<_> = child
        .seen
        .iter()
        .enumerate()
        .filter(|(_, frame)| frame["type"] == "agent_start")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(starts.len(), 2);
    assert!(initial_end < starts[1], "replacement cannot overlap the cancelled Run");
    assert_eq!(
        child
            .seen
            .iter()
            .filter(|frame| frame["type"] == "tool_execution_start" && frame["toolCallId"] == "after-abort")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_responses_tool_journal_and_restart_use_native_history_without_replay() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_rpc","call_id":"rpc_write","name":"write"}}},
            {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"native.txt\",\"content\":\"written once\\n\"}"}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_rpc","call_id":"rpc_write","name":"write","arguments":"{}"}}},
            {"data":{"type":"response.completed","response":{"id":"resp_rpc_tool","status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_rpc_done","content":[{"type":"output_text","text":"Native write settled."}]}}},
            {"data":{"type":"response.completed","response":{"id":"resp_rpc_done","status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_rpc_resume","content":[{"type":"output_text","text":"Native journal resumed."}]}}},
            {"data":{"type":"response.completed","response":{"id":"resp_rpc_resume","status":"completed"}}}
        ]}
    ]})).await;
    let args = ["--api", "openai-responses", "--responses-stateful", "--tools", "write"];
    let mut first = RpcChild::spawn(env.command(&up.base_url(), &args));
    first.ready();
    first.send(json!({"id":"write","type":"prompt","message":"write native proof"}));
    first.success("write");
    first.until(|frame| frame["type"] == "agent_end", WAIT);
    first.finish(0);
    let artifact = env.work.path().join("native.txt");
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "written once\n");
    let sessions = env.sessions();
    assert_eq!(sessions.len(), 1);
    let before = journal(&sessions[0]);
    assert_eq!(before.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    std::fs::write(&artifact, "sentinel after restart\n").unwrap();
    let mut resumed_args = args.to_vec();
    resumed_args.extend(["--resume", sessions[0].to_str().unwrap()]);
    let mut second = RpcChild::spawn(env.command(&up.base_url(), &resumed_args));
    second.ready();
    second.send(json!({"id":"resume-state","type":"get_messages"}));
    let restored = second.success("resume-state");
    assert_eq!(restored["data"]["messages"].as_array().unwrap().len(), 4);
    second.send(json!({"id":"resume","type":"prompt","message":"confirm previous artifact"}));
    second.success("resume");
    second.until(|frame| frame["type"] == "agent_end", WAIT);
    second.finish(0);
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "sentinel after restart\n");
    assert_eq!(env.sessions(), sessions);
    let after = journal(&sessions[0]);
    assert_eq!(&after[..before.len()], before.as_slice(), "native resume appends to the original journal");
    assert_eq!(after.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request["request"] == "POST /v1/responses HTTP/1.1"));
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_rpc_tool");
    assert!(
        requests[2]["body"].get("previous_response_id").is_none(),
        "process restart starts a cold provider session"
    );
    let full = requests[2]["body"]["input"].as_array().unwrap();
    assert!(full.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "rpc_write"));
    assert!(full.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "rpc_write"));
    assert!(full.iter().any(|item| {
        item["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["text"] == "Native write settled."))
    }));
    drop(requests);
    rpc_native_edit_preview_guard_and_disabled_settings_group().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_persistence_failure_cancels_before_the_next_tool_effect_and_exits_one() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("Journal materialized."),finish("stop"),done()]},
        {"events":[tool_call(0,"must-not-write","write","{\"path\":\"forbidden.txt\",\"content\":\"must not execute\"}"),finish("tool_calls"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready();
    child.send(json!({"id":"seed","type":"prompt","message":"materialize journal"}));
    child.success("seed");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let sessions = env.sessions();
    assert_eq!(sessions.len(), 1);
    let original = std::fs::read(&sessions[0]).unwrap();
    // An append target replaced with a directory is a deterministic I/O fault
    // on Windows and Unix, without depending on privileged ACL behavior.
    std::fs::remove_file(&sessions[0]).unwrap();
    std::fs::create_dir(&sessions[0]).unwrap();
    child.send(json!({"id":"fault","type":"prompt","message":"write only after durable receipt"}));
    child.success("fault");
    child.finish_inner(1, true, true);
    let failure = child.seen.iter().find(|frame| frame["id"] == "fault" && frame["success"] == false).unwrap();
    assert!(failure["error"].as_str().unwrap().contains("Session persistence failed"), "{failure}");
    assert!(failure["error"].as_str().unwrap().contains("restart from the journal"), "{failure}");
    assert!(!env.work.path().join("forbidden.txt").exists());
    assert_eq!(up.served(), 1, "even the new user receipt failed before inference");
    assert!(child.errors().contains("session persistence failed"));
    assert!(!original.is_empty());
    assert!(!child.seen.iter().any(|frame| frame["type"] == "tool_execution_start"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_assistant_tool_receipt_failure_prevents_a_real_write() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"unpersisted-call","write","{\"path\":\"unpersisted.txt\",\"content\":\"must not execute\"}"),finish("tool_calls"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready();
    child.send(json!({"id":"file","type":"get_state"}));
    let state = child.success("file");
    let journal_path = PathBuf::from(state["data"]["sessionFile"].as_str().unwrap());
    assert!(!journal_path.exists(), "journal is lazy until the first assistant");
    // A directory at the planned file blocks the first atomic rename. The
    // lazy user receipt still succeeds, and the actual tool-call assistant is
    // received from the provider before persistence fails at MessageEnd.
    std::fs::create_dir(&journal_path).unwrap();
    child.send(json!({"id":"assistant-fault","type":"prompt","message":"write after assistant durability"}));
    child.success("assistant-fault");
    child.finish_inner(1, true, true);
    assert_eq!(up.served(), 1, "actual assistant tool call reached the host");
    assert!(!env.work.path().join("unpersisted.txt").exists());
    assert!(child.seen.iter().any(|frame| frame["type"] == "message_end"
        && frame["message"]["content"].as_array().is_some_and(|content| {
            content.iter().any(|block| block["type"] == "toolCall" && block["id"] == "unpersisted-call")
        })));
    let refused = child
        .seen
        .iter()
        .find(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "unpersisted-call")
        .unwrap();
    assert_eq!(refused["isError"], true);
    assert!(contains_text(&refused["result"], "Tool was not executed because the run was aborted"), "{refused}");
    let failure =
        child.seen.iter().find(|frame| frame["id"] == "assistant-fault" && frame["success"] == false).unwrap();
    assert!(failure["error"].as_str().unwrap().contains("Session persistence failed"), "{failure}");
    assert!(failure["error"].as_str().unwrap().contains("restart from the journal"), "{failure}");
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_missing_tool_receipt_resume_reports_unknown_effect_and_never_replays() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"unknown-write","write","{\"path\":\"unknown.txt\",\"content\":\"written once\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Write settled."),finish("stop"),done()]},
        {"events":[text("Unknown receipt inspected."),finish("stop"),done()]}
    ]})).await;
    let mut first = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    first.ready();
    first.send(json!({"id":"first","type":"prompt","message":"write original once"}));
    first.success("first");
    first.until(|frame| frame["type"] == "agent_end", WAIT);
    first.finish(0);
    let sessions = env.sessions();
    let entries = journal(&sessions[0]);
    let assistant = entries
        .iter()
        .position(|entry| {
            entry["message"]["content"].as_array().is_some_and(|content| {
                content.iter().any(|block| block["type"] == "toolCall" && block["id"] == "unknown-write")
            })
        })
        .unwrap();
    // Simulate the crash window after a real file effect but before its tool
    // receipt. Preserve the original header/IDs and only remove the suffix.
    let truncated = entries[..=assistant].iter().map(|entry| format!("{entry}\n")).collect::<String>();
    std::fs::write(&sessions[0], truncated).unwrap();
    let artifact = env.work.path().join("unknown.txt");
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "written once\n");
    std::fs::write(&artifact, "external sentinel\n").unwrap();
    let mut second =
        RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write", "--resume", sessions[0].to_str().unwrap()]));
    second.ready();
    second.send(json!({"id":"unknown-state","type":"get_messages"}));
    let restored = second.success("unknown-state");
    let receipt = restored["data"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["toolCallId"] == "unknown-write")
        .unwrap();
    assert_eq!(receipt["isError"], true);
    assert_eq!(receipt["details"]["executed"], "unknown");
    assert!(contains_text(receipt, "effects are unknown"));
    second.send(json!({"id":"inspect","type":"prompt","message":"inspect history without rerunning write"}));
    second.success("inspect");
    second.until(|frame| frame["type"] == "agent_end", WAIT);
    second.finish(0);
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "external sentinel\n");
    assert_eq!(up.served(), 3);
    assert!(!second.seen.iter().any(|frame| frame["type"] == "tool_execution_start"));
    assert!(second.errors().contains("effects are unknown and they were not re-run"));
    let entries = journal(&sessions[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["toolCallId"] == "unknown-write").count(), 1);
    let requests = up.requests.lock().await;
    assert!(requests[2]["body"]["messages"].as_array().unwrap().iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "unknown-write"
            && contains_text(message, "effects are unknown")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_last_assistant_text_trims_and_skips_empty_aborted_messages() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text(" \u{feff}Prior answer.\u{00a0}\n"),finish("stop"),done()]},
        {"events":[],"end":"hang"}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"empty","type":"get_last_assistant_text"}));
    assert_eq!(child.success("empty")["data"], json!({}));
    child.send(json!({"id":"first","type":"prompt","message":"produce prior answer"}));
    child.success("first");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.send(json!({"id":"trimmed","type":"get_last_assistant_text"}));
    assert_eq!(child.success("trimmed")["data"]["text"], "Prior answer.");
    child.send(json!({"id":"held","type":"prompt","message":"abort before visible content"}));
    child.success("held");
    let deadline = Instant::now() + Duration::from_secs(5);
    while up.served() < 2 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    child.send(json!({"id":"abort","type":"abort"}));
    child.success("abort");
    child.send(json!({"id":"prior","type":"get_last_assistant_text"}));
    assert_eq!(child.success("prior")["data"]["text"], "Prior answer.");
    child.finish(0);
    assert!(child.seen.iter().any(|frame| frame["type"] == "message_end"
        && frame["message"]["stopReason"] == "aborted"
        && frame["message"]["content"] == json!([])));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_broken_stdout_exits_bounded_while_stdin_remains_open() {
    let env = Env::new();
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.close_stdout_pipe();
    let stdin = child.stdin.as_mut().unwrap();
    let _ = stdin.write_all(b"{\"id\":\"broken-pipe\",\"type\":\"get_state\"}\n");
    let _ = stdin.flush();
    let started = Instant::now();
    child.finish_inner(2, true, false);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(child.errors().contains("writing RPC output"), "{}", child.errors());
    assert_eq!(up.served(), 0);
}

fn native_header(entries: &[Value]) -> &Value {
    entries.iter().find(|entry| entry["type"] == "session").unwrap()
}

fn project_markers(env: &Env, label: &str) {
    std::fs::create_dir_all(env.work.path().join(".ara")).unwrap();
    std::fs::write(env.work.path().join(".ara/SYSTEM.md"), format!("SYSTEM-{label}")).unwrap();
    std::fs::write(env.work.path().join("AGENTS.md"), format!("AGENTS-{label}")).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_new_session_joins_active_tool_preserves_modes_and_clears_old_queues() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Original Session Skill body.");
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"old-active","bash","{\"command\":\"printf old-effect >> old-effect.txt; printf 'old tool running\\n'; sleep 30\"}"),finish("tool_calls"),done()]},
        {"events":[tool_call(0,"new-write","write","{\"path\":\"fresh.txt\",\"content\":\"new session artifact\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Fresh task settled."),finish("stop"),done()]}
    ]})).await;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = env.command(&up.base_url(), &["--tools", "bash,write"]);
    #[cfg(windows)]
    command.env("PATH", format!("C:\\Program Files\\Git\\usr\\bin;{}", std::env::var("PATH").unwrap_or_default()));
    let mut child = RpcChild::spawn(command);
    child.ready_commands(proof_commands());
    // Keep the tool active while messages queue so new_session owns its abort.
    child.send(json!({"id":"wait-interrupt","type":"set_interrupt_mode","mode":"wait"}));
    child.success("wait-interrupt");
    let old = child.state("old-state");
    let old_file = PathBuf::from(old["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"steering-mode","type":"set_steering_mode","mode":"all"}));
    child.success("steering-mode");
    child.send(json!({"id":"follow-mode","type":"set_follow_up_mode","mode":"all"}));
    child.success("follow-mode");
    child.send(json!({"id":"old","type":"prompt","message":"/skill:proof old active task"}));
    assert_eq!(child.success("old")["data"], json!({"agentInvoked":true}));
    child.until(
        |frame| frame["type"] == "tool_execution_update" && contains_text(&frame["partialResult"], "old tool running"),
        WAIT,
    );
    child.send(json!({"id":"steer-old","type":"prompt","message":"/skill:proof discard old steering"}));
    assert_eq!(child.success("steer-old")["data"], json!({"agentInvoked":true}));
    child.send(json!({"id":"follow-old","type":"prompt","message":"/skill:proof discard old followup","streamingBehavior":"followUp"}));
    assert_eq!(child.success("follow-old")["data"], json!({"agentInvoked":true}));
    assert_eq!(child.state("queued-old")["queuedMessageCount"], 2);
    let parent = old_file.to_string_lossy().to_string();
    child.adopt_commands("new", json!({"id":"new","type":"new_session","parentSession":parent}), proof_commands());
    let adopted = child.state("new-state");
    assert_ne!(adopted["sessionId"], old["sessionId"]);
    assert_eq!(adopted["messageCount"], 0);
    assert_eq!(adopted["queuedMessageCount"], 0);
    assert_eq!(adopted["steeringMode"], "all");
    assert_eq!(adopted["followUpMode"], "all");
    assert_eq!(adopted["interruptMode"], "wait");
    let new_file = PathBuf::from(adopted["sessionFile"].as_str().unwrap());
    assert!(new_file.is_file(), "new_session is durable before its successful response");
    let initial_new = journal(&new_file);
    assert_eq!(native_header(&initial_new)["parentSession"], parent);
    assert_eq!(native_header(&initial_new)["id"], adopted["sessionId"]);
    assert_eq!(native_header(&initial_new)["cwd"], native_header(&journal(&old_file))["cwd"]);
    let end = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let ack = child.seen.iter().position(|frame| frame["id"] == "new").unwrap();
    assert!(end < ack);
    let old_entries = journal(&old_file);
    let old_skills = skill_entries(&old_entries);
    assert_eq!(old_skills.len(), 1, "only the consumed initial Skill belongs to the old Session");
    assert_eq!(old_skills[0]["details"]["originalText"], "/skill:proof old active task");
    assert!(!json!(old_entries).to_string().contains("discard old"));
    assert_eq!(old_entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 0);
    assert_eq!(child.seen[end]["messages"], json!(completed_events(&child.seen[..=end])));
    let receipt = old_entries.iter().find(|entry| entry["message"]["toolCallId"] == "old-active").unwrap();
    assert_eq!(receipt["message"]["isError"], true);
    assert!(contains_text(&receipt["message"], "aborted"), "settled old-active receipt: {receipt}");
    child.send(json!({"id":"fresh","type":"prompt","message":"new task after adoption"}));
    child.success("fresh");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(journal(&old_file), old_entries, "later events stay bound to the new journal");
    assert_eq!(
        std::fs::read_to_string(env.work.path().join("old-effect.txt")).unwrap(),
        "old-effect",
        "old effect is never replayed"
    );
    assert_eq!(std::fs::read_to_string(env.work.path().join("fresh.txt")).unwrap(), "new session artifact\n");
    assert!(journal(&new_file).iter().any(|entry| entry["message"]["toolCallId"] == "new-write"));
    assert!(journal(&new_file).iter().all(|entry| entry["message"]["toolCallId"] != "old-active"));
    assert!(skill_entries(&journal(&new_file)).is_empty(), "discarded custom queues never migrate to the new journal");
    assert_no_local_prompt_result(&child.seen, &["old", "steer-old", "follow-old"]);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let fresh = requests[1]["body"]["messages"].to_string();
    assert!(fresh.contains("new task after adoption"));
    assert!(!fresh.contains("old active task") && !fresh.contains("discard old"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_changed_cwd_switch_is_cancelled_and_new_keeps_current_context_and_tools() {
    let original = Env::new();
    let target = Env::new();
    invocation_skill(&original, ".ara/skills", "origin", "Original metadata snapshot");
    invocation_skill(&target, ".ara/skills", "target", "Cancelled candidate snapshot");
    project_markers(&original, "ORIGINAL");
    project_markers(&target, "TARGET");
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"target-seed","write","{\"path\":\"target-seed.txt\",\"content\":\"seed target\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Target seeded."),finish("stop"),done()]},
        {"events":[text("Original seeded."),finish("stop"),done()]},
        {"events":[tool_call(0,"target-switch","write","{\"path\":\"switched.txt\",\"content\":\"target cwd effect\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Target switch settled."),finish("stop"),done()]},
        {"events":[tool_call(0,"target-fresh","write","{\"path\":\"new-after-switch.txt\",\"content\":\"inherited target cwd\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Fresh target session settled."),finish("stop"),done()]}
    ]})).await;
    let mut seed = RpcChild::spawn(target.command(&up.base_url(), &["--tools", "write"]));
    seed.ready_commands(json!([command_metadata("target", "RPC invocation fixture")]));
    seed.send(json!({"id":"seed","type":"prompt","message":"target original source"}));
    seed.success("seed");
    seed.until(|frame| frame["type"] == "agent_end", WAIT);
    let target_state = seed.state("target-state");
    seed.finish(0);
    let target_file = PathBuf::from(target_state["sessionFile"].as_str().unwrap());
    let target_before = journal(&target_file);
    let mut child = RpcChild::spawn(original.command(&up.base_url(), &["--tools", "write"]));
    child.ready_commands(json!([command_metadata("origin", "RPC invocation fixture")]));
    child.send(json!({"id":"original","type":"prompt","message":"original source must stay behind"}));
    child.success("original");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let original_state = child.state("original-state");
    let original_messages = child.messages("original-messages");
    let original_file = PathBuf::from(original_state["sessionFile"].as_str().unwrap());
    let original_before = journal(&original_file);
    let before_cancelled = child.seen.len();
    child.send(json!({"id":"switch","type":"switch_session","sessionPath":target_file}));
    assert_eq!(child.success("switch")["data"], json!({"cancelled":true}));
    let switched = child.state("switched-state");
    assert_eq!(switched["sessionId"], original_state["sessionId"]);
    assert_eq!(switched["sessionFile"], original_state["sessionFile"]);
    assert_eq!(child.messages("switched-messages"), original_messages);
    assert_eq!(
        child.commands("cancelled-switch-commands"),
        json!([command_metadata("origin", "RPC invocation fixture")])
    );
    assert!(!child.seen[before_cancelled..].iter().any(|frame| frame["type"] == "available_commands_update"));
    child.send(json!({"id":"target-task","type":"prompt","message":"task after target switch"}));
    child.success("target-task");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    assert_eq!(std::fs::read_to_string(original.work.path().join("switched.txt")).unwrap(), "target cwd effect\n");
    assert!(!target.work.path().join("switched.txt").exists());
    project_markers(&original, "REFRESHED");
    child.adopt_commands(
        "new-target",
        json!({"id":"new-target","type":"new_session"}),
        json!([command_metadata("origin", "RPC invocation fixture")]),
    );
    let fresh = child.state("fresh-target-state");
    let fresh_file = PathBuf::from(fresh["sessionFile"].as_str().unwrap());
    assert_eq!(fresh["messageCount"], 0);
    assert_ne!(fresh["sessionId"], original_state["sessionId"]);
    assert_eq!(native_header(&journal(&fresh_file))["cwd"], native_header(&original_before)["cwd"]);
    child.send(json!({"id":"fresh-target","type":"prompt","message":"fresh adopted cwd task"}));
    child.success("fresh-target");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(
        std::fs::read_to_string(original.work.path().join("new-after-switch.txt")).unwrap(),
        "inherited target cwd\n"
    );
    assert!(!target.work.path().join("new-after-switch.txt").exists());
    assert_eq!(&journal(&original_file)[..original_before.len()], original_before.as_slice());
    assert_eq!(journal(&target_file), target_before);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 7);
    for index in [3, 5] {
        let messages = requests[index]["body"]["messages"].as_array().unwrap();
        let systems = messages
            .iter()
            .filter(|message| message["role"] == "system")
            .map(|message| message["content"].to_string())
            .collect::<String>();
        let expected = if index == 3 { "ORIGINAL" } else { "REFRESHED" };
        assert!(
            systems.contains(&format!("SYSTEM-{expected}")) && systems.contains(&format!("AGENTS-{expected}")),
            "{systems}"
        );
        assert!(!systems.contains("SYSTEM-TARGET") && !systems.contains("AGENTS-TARGET"));
        let cwd = native_header(&original_before)["cwd"].as_str().unwrap().replace('\\', "/");
        assert!(
            messages.iter().any(|message| message["role"] == "user" && contains_text(message, &cwd)),
            "rebuilt date/cwd reminder: {messages:?}"
        );
        assert!(!messages.iter().any(|message| contains_text(message, "target original source")));
    }
    assert!(requests[3]["body"]["messages"].to_string().contains("original source must stay behind"));
    assert!(!requests[5]["body"]["messages"].to_string().contains("original source must stay behind"));
}

fn responses_answer(id: &str, answer: &str) -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":format!("msg-{id}"),"role":"assistant","status":"completed","content":[{"type":"output_text","text":answer,"annotations":[]}]}}},
        {"data":{"type":"response.completed","response":{"id":id,"status":"completed"}}}
    ]})
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_adoption_resets_responses_state_but_unchanged_reload_preserves_it() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        responses_answer("ra1","Answer A1."),responses_answer("ra2","Answer A2."),
        responses_answer("rb1","Answer B1."),responses_answer("rb2","Answer B2."),
        responses_answer("ra3","Answer A3."),responses_answer("ra4","Answer A4."),
        responses_answer("ra-meta","Metadata reload answer."),responses_answer("ra5","Answer A5."),
        responses_answer("ra6","Replacement identity answer.")
    ]}))
    .await;
    let mut child =
        RpcChild::spawn(env.command(&up.base_url(), &["--api", "openai-responses", "--responses-stateful"]));
    child.ready();
    for id in ["A1", "A2"] {
        child.send(json!({"id":id,"type":"prompt","message":format!("native prompt {id}")}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
    }
    let original = child.state("native-original");
    let original_messages = child.messages("native-original-messages");
    let original_file = PathBuf::from(original["sessionFile"].as_str().unwrap());
    let original_entries = journal(&original_file);
    child.adopt("native-new", json!({"id":"native-new","type":"new_session"}));
    for id in ["B1", "B2"] {
        child.send(json!({"id":id,"type":"prompt","message":format!("native prompt {id}")}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
    }
    child.adopt("native-switch", json!({"id":"native-switch","type":"switch_session","sessionPath":original_file}));
    assert_eq!(child.messages("native-restored"), original_messages);
    child.send(json!({"id":"A3","type":"prompt","message":"native prompt A3"}));
    child.success("A3");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let before_reload = child.messages("before-reload");
    child.adopt("same-reload", json!({"id":"same-reload","type":"switch_session","sessionPath":original_file}));
    assert_eq!(child.messages("after-reload"), before_reload);
    assert_eq!(child.state("reload-id")["sessionId"], original["sessionId"]);
    child.send(json!({"id":"A4","type":"prompt","message":"native prompt A4"}));
    child.success("A4");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let mut metadata_only = journal(&original_file);
    let lexical_alias = env.work.path().join("uncreated-component").join("..");
    assert!(!env.work.path().join("uncreated-component").exists());
    metadata_only.iter_mut().find(|entry| entry["type"] == "session").unwrap()["cwd"] = json!(lexical_alias);
    for entry in &mut metadata_only {
        if entry["message"]["role"] == "assistant" {
            entry["message"]["timestamp"] = json!(0);
            entry["message"]["usage"]["input"] = json!(9999);
        }
    }
    std::fs::write(&original_file, metadata_only.iter().map(|entry| format!("{entry}\n")).collect::<String>()).unwrap();
    child.adopt("metadata-reload", json!({"id":"metadata-reload","type":"switch_session","sessionPath":original_file}));
    let metadata_messages = child.messages("metadata-restored");
    assert!(
        metadata_messages
            .iter()
            .filter(|message| message["role"] == "assistant")
            .all(|message| message["timestamp"] == 0 && message["usage"]["input"] == 9999)
    );
    child.send(json!({"id":"metadata-prompt","type":"prompt","message":"metadata-only reload still warm"}));
    child.success("metadata-prompt");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    // A different saved conversation at the same path requires fresh native
    // state; unchanged reload above retains the existing provider baseline.
    std::fs::write(&original_file, original_entries.iter().map(|entry| format!("{entry}\n")).collect::<String>())
        .unwrap();
    child.adopt("changed-reload", json!({"id":"changed-reload","type":"switch_session","sessionPath":original_file}));
    assert_eq!(child.messages("changed-restored"), original_messages);
    child.send(json!({"id":"A5","type":"prompt","message":"native prompt A5"}));
    child.success("A5");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let mut changed_identity = journal(&original_file);
    let replacement_id = "00000000-0000-4000-8000-000000000001";
    changed_identity.iter_mut().find(|entry| entry["type"] == "session").unwrap()["id"] = json!(replacement_id);
    std::fs::write(&original_file, changed_identity.iter().map(|entry| format!("{entry}\n")).collect::<String>())
        .unwrap();
    child.adopt("identity-reload", json!({"id":"identity-reload","type":"switch_session","sessionPath":original_file}));
    assert_eq!(child.state("replacement-identity")["sessionId"], replacement_id);
    child.send(json!({"id":"A6","type":"prompt","message":"same path replacement identity"}));
    child.success("A6");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 9);
    assert_eq!(requests[1]["body"]["previous_response_id"], "ra1");
    assert_eq!(requests[3]["body"]["previous_response_id"], "rb1");
    let controls = |body: &Value| {
        let mut controls = body.as_object().unwrap().clone();
        controls.remove("input");
        controls.remove("previous_response_id");
        controls
    };
    let before_controls = controls(&requests[4]["body"]);
    let after_controls = controls(&requests[5]["body"]);
    let changed_controls: Vec<_> = before_controls
        .keys()
        .chain(after_controls.keys())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|key| before_controls.get(*key) != after_controls.get(*key))
        .map(|key| json!({"field": key, "before": before_controls.get(key), "after": after_controls.get(key)}))
        .collect();
    assert_eq!(
        requests[5]["body"]["previous_response_id"],
        "ra3",
        "unchanged reload controls diff: {changed_controls:?}; stderr: {}",
        child.errors()
    );
    assert_eq!(requests[6]["body"]["previous_response_id"], "ra4");
    for index in [0, 2, 4, 7, 8] {
        assert!(
            requests[index]["body"].get("previous_response_id").is_none(),
            "fresh provider state at {index}: {}",
            requests[index]["body"]
        );
    }
    assert!(!requests[2]["body"]["input"].to_string().contains("native prompt A"));
    for index in [4, 7] {
        let history = requests[index]["body"]["input"].to_string();
        assert!(
            history.contains("native prompt A1")
                && history.contains("Answer A1.")
                && history.contains("native prompt A2")
                && history.contains("Answer A2.")
        );
        assert!(!history.contains("native prompt B"));
    }
    assert_eq!(requests[5]["body"]["input"].as_array().unwrap().len(), 1);
    assert!(requests[5]["body"]["input"].to_string().contains("native prompt A4"));
    assert_eq!(requests[6]["body"]["input"].as_array().unwrap().len(), 1);
    assert!(requests[6]["body"]["input"].to_string().contains("metadata-only reload still warm"));
    assert!(!requests[7]["body"]["input"].to_string().contains("Answer A3."));
    assert!(requests[8]["body"]["input"].to_string().contains("Answer A5."));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_failed_switch_never_adopts_a_fallback_and_original_remains_usable() {
    let env = Env::new();
    invocation_skill(&env, ".ara/skills", "proof", "Skill snapshot retained through failed adoption.");
    let up = upstream(json!({"responses":[
        {"events":[text("Original preserved."),finish("stop"),done()]},
        {"events":[tool_call(0,"still-original","write","{\"path\":\"original-after-failure.txt\",\"content\":\"original owner kept\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Original task still usable."),finish("stop"),done()]}
    ]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--tools", "write"]));
    child.ready_commands(proof_commands());
    child.send(json!({"id":"original","type":"prompt","message":"/skill:proof original before failed adoption"}));
    assert_eq!(child.success("original")["data"], json!({"agentInvoked":true}));
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let original = child.state("before-failure");
    let messages = child.messages("before-failure-messages");
    let original_file = PathBuf::from(original["sessionFile"].as_str().unwrap());
    let original_bytes = std::fs::read(&original_file).unwrap();
    let faults = tempfile::Builder::new().prefix("ara-rpc-switch-fault-").tempdir().unwrap();
    let missing = faults.path().join("missing.jsonl");
    let corrupt = faults.path().join("corrupt.jsonl");
    std::fs::write(&corrupt, "{broken session\n").unwrap();
    let missing_cwd = faults.path().join("missing-cwd.jsonl");
    let mut invalid = journal(&original_file);
    let header = invalid.iter_mut().find(|entry| entry["type"] == "session").unwrap();
    header["cwd"] = json!(faults.path().join("directory-that-does-not-exist"));
    std::fs::write(&missing_cwd, invalid.iter().map(|entry| format!("{entry}\n")).collect::<String>()).unwrap();
    for (id, target) in [("missing", &missing), ("corrupt", &corrupt), ("missing-cwd", &missing_cwd)] {
        let before = std::fs::read(target).ok();
        let before_failure = child.seen.len();
        child.send(json!({"id":id,"type":"switch_session","sessionPath":target}));
        let error = child.response(id);
        if id == "missing-cwd" {
            assert_eq!(error["success"], true, "{error}");
            assert_eq!(error["data"], json!({"cancelled":true}));
        } else {
            assert_eq!(error["success"], false, "{error}");
            assert!(!error["error"].as_str().unwrap().is_empty());
        }
        assert_eq!(child.state(&format!("state-{id}"))["sessionId"], original["sessionId"]);
        assert_eq!(child.messages(&format!("messages-{id}")), messages);
        assert_eq!(child.commands(&format!("commands-{id}")), proof_commands());
        assert!(!child.seen[before_failure..].iter().any(|frame| frame["type"] == "available_commands_update"));
        assert_eq!(std::fs::read(&original_file).unwrap(), original_bytes);
        assert_eq!(std::fs::read(target).ok(), before);
        assert_eq!(env.sessions().as_slice(), std::slice::from_ref(&original_file));
    }
    child.send(json!({"id":"still-usable","type":"prompt","message":"/skill:proof task on preserved original"}));
    assert_eq!(
        child.success("still-usable")["data"],
        json!({"agentInvoked":true}),
        "failed adoption keeps the original registered Skill snapshot"
    );
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(
        std::fs::read_to_string(env.work.path().join("original-after-failure.txt")).unwrap(),
        "original owner kept\n"
    );
    let custom = journal(&original_file);
    assert_eq!(skill_entries(&custom).len(), 2);
    assert_eq!(custom.iter().filter(|entry| entry["message"]["role"] == "user").count(), 0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests[1]["body"]["messages"].to_string().contains("original before failed adoption"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_no_session_new_ids_are_unique_without_journals_and_switch_fails_explicitly() {
    let env = Env::new();
    let up = upstream(json!({"responses":[{"events":[text("Ephemeral task settled."),finish("stop"),done()]}]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--no-session"]));
    child.ready();
    let original = child.state("ephemeral-original");
    assert!(original.get("sessionFile").is_none());
    child.adopt("ephemeral-new", json!({"id":"ephemeral-new","type":"new_session"}));
    let first = child.state("ephemeral-first");
    assert_ne!(first["sessionId"], original["sessionId"]);
    assert!(first.get("sessionFile").is_none());
    child.send(json!({"id":"ephemeral-task","type":"prompt","message":"ephemeral task"}));
    child.success("ephemeral-task");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.adopt("ephemeral-next", json!({"id":"ephemeral-next","type":"new_session"}));
    let second = child.state("ephemeral-second");
    assert_ne!(second["sessionId"], first["sessionId"]);
    assert_eq!(second["messageCount"], 0);
    let before_failure = child.seen.len();
    child.send(
        json!({"id":"ephemeral-switch","type":"switch_session","sessionPath":env.home.path().join("anything.jsonl")}),
    );
    assert_eq!(child.response("ephemeral-switch")["success"], false);
    assert_eq!(child.state("after-ephemeral-failure")["sessionId"], second["sessionId"]);
    assert!(!child.seen[before_failure..].iter().any(|frame| frame["type"] == "available_commands_update"));
    child.finish(0);
    assert!(env.sessions().is_empty());
    assert!(!env.sessions.exists(), "no-session never initializes a journal directory");
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_runtime_switch_recovers_unknown_receipt_without_replaying_file_effect() {
    let original = Env::new();
    let target = Env::new();
    project_markers(&original, "BEFORE-SWITCH");
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"runtime-unknown","write","{\"path\":\"runtime-unknown.txt\",\"content\":\"write once\\n\"}"),finish("tool_calls"),done()]},
        {"events":[text("Target write settled."),finish("stop"),done()]},
        {"events":[text("Runtime unknown inspected."),finish("stop"),done()]}
    ]})).await;
    let mut seed = RpcChild::spawn(target.command_at(&up.base_url(), &["--tools", "write"], original.work.path()));
    seed.ready();
    seed.send(json!({"id":"seed","type":"prompt","message":"target original write"}));
    seed.success("seed");
    seed.until(|frame| frame["type"] == "agent_end", WAIT);
    seed.finish(0);
    let target_file = target.sessions().pop().unwrap();
    let entries = journal(&target_file);
    let assistant = entries
        .iter()
        .position(|entry| {
            entry["message"]["content"].as_array().is_some_and(|content| {
                content.iter().any(|block| block["type"] == "toolCall" && block["id"] == "runtime-unknown")
            })
        })
        .unwrap();
    std::fs::write(&target_file, entries[..=assistant].iter().map(|entry| format!("{entry}\n")).collect::<String>())
        .unwrap();
    let artifact = original.work.path().join("runtime-unknown.txt");
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "write once\n");
    std::fs::write(&artifact, "external runtime sentinel\n").unwrap();
    let mut child = RpcChild::spawn(original.command(&up.base_url(), &["--tools", "write"]));
    child.ready();
    let before = child.state("before-runtime-switch");
    assert!(before["systemPrompt"].to_string().contains("SYSTEM-BEFORE-SWITCH"));
    project_markers(&original, "AFTER-SWITCH");
    child.adopt("runtime-switch", json!({"id":"runtime-switch","type":"switch_session","sessionPath":target_file}));
    let adopted = child.state("after-runtime-switch");
    assert_ne!(adopted["sessionId"], before["sessionId"]);
    assert_eq!(adopted["sessionId"], native_header(&entries)["id"]);
    assert!(adopted["systemPrompt"].to_string().contains("SYSTEM-AFTER-SWITCH"));
    assert!(!adopted["systemPrompt"].to_string().contains("SYSTEM-BEFORE-SWITCH"));
    let restored = child.messages("runtime-restored");
    let unknown = restored.iter().find(|message| message["toolCallId"] == "runtime-unknown").unwrap();
    assert_eq!(unknown["isError"], true);
    assert_eq!(unknown["details"]["executed"], "unknown");
    assert!(contains_text(unknown, "effects are unknown"));
    child.send(json!({"id":"inspect-runtime","type":"prompt","message":"inspect unknown without replay"}));
    child.success("inspect-runtime");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(std::fs::read_to_string(artifact).unwrap(), "external runtime sentinel\n");
    assert!(!target.work.path().join("runtime-unknown.txt").exists());
    assert!(!child.seen.iter().any(|frame| frame["type"] == "tool_execution_start"));
    assert_eq!(
        journal(&target_file).iter().filter(|entry| entry["message"]["toolCallId"] == "runtime-unknown").count(),
        1
    );
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let system = requests[2]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "system")
        .map(|message| message["content"].to_string())
        .collect::<String>();
    assert!(system.contains("SYSTEM-AFTER-SWITCH") && system.contains("AGENTS-AFTER-SWITCH"));
    assert!(!system.contains("BEFORE-SWITCH"));
    assert!(requests[2]["body"]["messages"].as_array().unwrap().iter().any(|message| {
        message["role"] == "tool"
            && message["tool_call_id"] == "runtime-unknown"
            && contains_text(message, "effects are unknown")
    }));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_same_file_switch_joins_active_run_before_opening_its_lazy_journal() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[],"end":"hang"},
        {"events":[text("Reloaded lazy session continued."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    let before = child.state("before-lazy-switch");
    let file = PathBuf::from(before["sessionFile"].as_str().unwrap());
    assert!(!file.exists());
    child.send(json!({"id":"lazy-run","type":"prompt","message":"source before lazy switch"}));
    child.success("lazy-run");
    let deadline = Instant::now() + Duration::from_secs(5);
    while up.served() == 0 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!file.exists(), "no assistant output, so the journal remains lazy");
    assert_eq!(child.state("lazy-active")["isStreaming"], true);
    child.adopt("lazy-switch", json!({"id":"lazy-switch","type":"switch_session","sessionPath":file}));
    let switched = child.state("after-lazy-switch");
    assert_eq!(switched["sessionId"], before["sessionId"]);
    assert_eq!(switched["sessionFile"], before["sessionFile"]);
    assert!(file.is_file(), "abort settled an assistant receipt before open");
    let restored = child.messages("lazy-restored");
    assert_eq!(user_texts(&restored), ["source before lazy switch"]);
    assert!(restored.iter().any(|message| message["role"] == "assistant" && message["stopReason"] == "aborted"));
    let ended = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let switched_ack = child.seen.iter().position(|frame| frame["id"] == "lazy-switch").unwrap();
    assert!(ended < switched_ack);
    child.send(json!({"id":"after-lazy","type":"prompt","message":"continue same file after join"}));
    child.success("after-lazy");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(env.sessions(), [file]);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["body"]["messages"].to_string().contains("source before lazy switch"));
}

#[cfg(feature = "test-fixture")]
#[tokio::test(flavor = "multi_thread")]
async fn rpc_failed_candidate_mcp_setup_keeps_original_session_and_live_tool_connection() {
    let original = Env::new();
    let target = Env::new();
    let marker = original.work.path().join("retained-mcp-marker.txt");
    let args = json!({"text":"retained original connection","marker":marker}).to_string();
    let up = upstream(json!({"responses":[
        {"events":[text("Target saved."),finish("stop"),done()]},
        {"events":[text("Original before setup failure."),finish("stop"),done()]},
        {"events":[tool_call(0,"retained-mcp","mcp__fixture__echo",&args),finish("tool_calls"),done()]},
        {"events":[text("Retained original MCP task settled."),finish("stop"),done()]}
    ]}))
    .await;
    let mut seed = RpcChild::spawn(target.command_at(&up.base_url(), &[], original.work.path()));
    seed.ready();
    seed.send(json!({"id":"seed","type":"prompt","message":"target for candidate setup failure"}));
    seed.success("seed");
    seed.until(|frame| frame["type"] == "agent_end", WAIT);
    seed.finish(0);
    let target_file = target.sessions().pop().unwrap();
    let target_before = std::fs::read(&target_file).unwrap();
    let old_skill = metadata_skill(&original, ".agents/skills", "old-mcp", "Original MCP metadata snapshot", "");
    let launcher = original.home.path().join("mcp-launch.sh");
    std::fs::write(&launcher, "#!/usr/bin/env bash\nexec \"$1\" --record \"$2\"\n").unwrap();
    let configuration = original.home.path().join("mcp.json");
    #[cfg(windows)]
    let bash = PathBuf::from("C:/Program Files/Git/usr/bin/bash.exe");
    #[cfg(not(windows))]
    let bash = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("bash"))
        .find(|path| path.is_file())
        .expect("MCP failure fixture requires Bash")
        .canonicalize()
        .unwrap();
    assert!(bash.is_absolute() && bash.is_file(), "MCP command must be an existing absolute executable");
    std::fs::write(
        &configuration,
        json!({
            "name":"fixture","command":bash,"args":[
                launcher.to_string_lossy().replace('\\',"/"),
                env!("CARGO_BIN_EXE_ara-mcp-fixture").replace('\\',"/"),
                original.home.path().join("mcp-record.json").to_string_lossy().replace('\\',"/")
            ]
        })
        .to_string(),
    )
    .unwrap();
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = original
        .command(&up.base_url(), &["--mcp-config", configuration.to_str().unwrap(), "--mcp-allow", "fixture:echo"]);
    #[cfg(windows)]
    command.env("PATH", format!("C:\\Program Files\\Git\\usr\\bin;{}", std::env::var("PATH").unwrap_or_default()));
    let mut child = RpcChild::spawn(command);
    child.ready_commands(json!([command_metadata("old-mcp", "Original MCP metadata snapshot")]));
    child.send(json!({"id":"original","type":"prompt","message":"original before MCP setup failure"}));
    child.success("original");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let state = child.state("before-mcp-failure");
    let messages = child.messages("before-mcp-failure-messages");
    let file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    let before = std::fs::read(&file).unwrap();
    // The original launcher already exec'd the fixture. Removing only that
    // script blocks later reconnects without touching the running MCP child.
    std::fs::remove_file(&launcher).unwrap();
    std::fs::remove_file(old_skill).unwrap();
    metadata_skill(&original, ".agents/skills", "candidate-mcp", "Must not adopt on setup failure", "");
    for (id, command) in [
        ("mcp-new-failure", json!({"id":"mcp-new-failure","type":"new_session"})),
        ("mcp-switch-failure", json!({"id":"mcp-switch-failure","type":"switch_session","sessionPath":target_file})),
    ] {
        let begin = child.seen.len();
        child.send(command);
        let error = child.response(id);
        assert_eq!(error["success"], false, "{error}");
        assert!(!error["error"].as_str().unwrap().is_empty());
        let retained = child.state(&format!("state-{id}"));
        assert_eq!(retained["sessionId"], state["sessionId"]);
        assert_eq!(retained["sessionFile"], state["sessionFile"]);
        assert!(retained["dumpTools"].as_array().unwrap().iter().any(|tool| tool["name"] == "mcp__fixture__echo"));
        assert_eq!(child.messages(&format!("messages-{id}")), messages);
        assert_eq!(
            child.commands(&format!("commands-{id}")),
            json!([command_metadata("old-mcp", "Original MCP metadata snapshot")])
        );
        assert_eq!(std::fs::read(&file).unwrap(), before);
        assert_eq!(std::fs::read(&target_file).unwrap(), target_before);
        assert_eq!(original.sessions().as_slice(), std::slice::from_ref(&file));
        assert!(!child.seen[begin..].iter().any(|frame| frame["type"] == "available_commands_update"));
    }
    child.send(json!({"id":"retained-task","type":"prompt","message":"use the retained original MCP connection"}));
    child.success("retained-task");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(std::fs::read_to_string(marker).unwrap(), "dispatched");
    let entries = journal(&file);
    let receipt = entries.iter().find(|entry| entry["message"]["toolCallId"] == "retained-mcp").unwrap();
    assert_eq!(receipt["message"]["isError"], false);
    assert!(contains_text(&receipt["message"], "echo: retained original connection"));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert!(requests[2]["body"]["messages"].to_string().contains("original before MCP setup failure"));
    assert!(!requests[2]["body"]["messages"].to_string().contains("target for candidate setup failure"));
}

// Fixed ModelControls Auto input families on the real JSON-lines Host. This
// covers the online consumer only; Local runtime/full model controls stay WIP.
impl Env {
    fn auto_models(&self, base_url: &str, default_auto: bool) {
        let agent = self.home.path().join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("config.yml"),
            serde_json::to_vec_pretty(&json!({
                "retry":{"enabled":false,"modelFallback":false,"usageAwareFallback":false},
                "features":{"unexpectedStopDetection":"none"},
                "defaultThinkingLevel":if default_auto { "auto" } else { "high" },
                "modelRoles":{"tiny":["auto-fixture/classifier-fixture"]}
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            agent.join("models.yml"),
            serde_json::to_vec_pretty(&json!({
                "providers":{"auto-fixture":{"api":"openai-completions","auth":"apiKey","apiKey":"ARA_API_KEY","baseUrl":base_url,
                    "compat":{"maxTokensField":"max_tokens"},
                    "models":[
                        {"id":"fake-model","contextWindow":131072,"maxTokens":8192,"reasoning":true,
                            "thinking":{"mode":"effort","efforts":["low","high"]}},
                        {"id":"classifier-fixture","contextWindow":131072,"maxTokens":2048,"reasoning":false}
                    ]}}
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

fn auto_reply(value: &str) -> Value {
    json!({"events":[text(value),finish("stop"),
        {"data":{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}},
        done()]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_startup_scope_cli_precedence_snapshot_and_thinking_sources() {
    let cases: &[(&[&str], &str, &str)] = &[
        (
            &["--models", "auto-fixture/fake-model:high", "--models=switch-fixture/target-model:low"],
            "target-model",
            "low",
        ),
        (
            &["--model", "auto-fixture/fake-model:high", "--models", "switch-fixture/target-model:low"],
            "fake-model",
            "low",
        ),
        (&["--models", "switch-fixture/target-model:low,auto-fixture/fake-model:high"], "fake-model", "low"),
    ];
    for (index, (args, expected_model, expected_thinking)) in cases.iter().enumerate() {
        let env = Env::new();
        let first = upstream(json!({"responses":[auto_reply("startup primary")]})).await;
        let target = SwitchWire::new(json!({"responses":[auto_reply("startup target")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let path = env.home.path().join("agent/config.yml");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings["enabledModels"] = json!(["switch-fixture/target-model:low", "auto-fixture/fake-model:high"]);
        settings["modelRoles"]["default"] = json!("auto-fixture/fake-model:high");
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let settings_before = std::fs::read(&path).unwrap();
        let mut child = RpcChild::spawn(env.startup_command(args));
        child.ready();
        let state = child.state("startup-selected");
        assert_eq!(state["model"]["id"], *expected_model, "case {index}");
        assert_eq!(state["configuredThinkingLevel"], *expected_thinking, "case {index}");
        child.send(json!({"id":"startup-run","type":"prompt","message":"exercise the selected startup route once"}));
        child.success("startup-run");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        if index == 0 {
            child.send(json!({"id":"single-scope","type":"cycle_model"}));
            assert_eq!(
                child.success("single-scope")["data"],
                Value::Null,
                "CLI one-entry snapshot must survive background catalog publication"
            );
        }
        child.finish(0);
        let (selected, unselected) = if *expected_model == "target-model" {
            (target.requests.lock().await, first.requests.lock().await)
        } else {
            (first.requests.lock().await, target.requests.lock().await)
        };
        let calls: Vec<_> = selected.iter().filter(|request| request["body"].is_object()).collect();
        assert_eq!(calls.len(), 1, "case {index}: {selected:?}");
        assert_eq!(calls[0]["body"]["model"], *expected_model);
        assert_eq!(calls[0]["body"]["reasoning_effort"], *expected_thinking);
        assert!(!unselected.iter().any(|request| request["body"].is_object()));
        assert_eq!(std::fs::read(&path).unwrap(), settings_before, "startup role selection must not persist settings");
        let entries = journal(Path::new(state["sessionFile"].as_str().unwrap()));
        assert_eq!(entries.iter().filter(|entry| entry["type"] == "model_change").count(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_startup_resume_candidates_and_cli_scope_preserve_saved_receipts() {
    for (role, expected_model) in [("smol", "target-model"), (ara_session::EPHEMERAL_MODEL_CHANGE_ROLE, "fake-model")] {
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        let target = SwitchWire::new(json!({"responses":[]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let mut saved = ara_session::SessionJournal::create(&env.sessions, env.work.path()).unwrap();
        saved.append_model_change("auto-fixture/fake-model").unwrap();
        saved.append_model_change_with_role("switch-fixture/target-model:high", Some(role), false).unwrap();
        saved.append_thinking_level_change(Some("low"), None).unwrap();
        saved.materialize().unwrap();
        let file = saved.path().to_path_buf();
        let before = std::fs::read(&file).unwrap();
        drop(saved);
        let mut child = RpcChild::spawn(env.startup_command(&[
            "--resume",
            file.to_str().unwrap(),
            "--models",
            "auto-fixture/fake-model:high",
        ]));
        child.ready();
        let state = child.state("restored-startup");
        assert_eq!(state["model"]["id"], expected_model);
        assert_eq!(state["configuredThinkingLevel"], "low", "saved receipt outranks scope/default/saved-model suffix");
        child.finish(0);
        assert_eq!(std::fs::read(&file).unwrap(), before, "resume must not append model/thinking startup receipts");
        assert!(!first.requests.lock().await.iter().any(|request| request["body"].is_object()));
        assert!(!target.requests.lock().await.iter().any(|request| request["body"].is_object()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_startup_unmatched_scope_and_invalid_explicit_selector_never_run_fallback() {
    for args in
        [vec!["--models", "missing-provider/nonexistent-model"], vec!["--model", "auto-fixture/fake-model:invalid"]]
    {
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        env.auto_models(&first.base_url(), false);
        let mut child = RpcChild::spawn(env.startup_command(&args));
        child.finish_inner(1, false, false);
        assert!(child.errors().contains("not found"), "{}", child.errors());
        assert!(env.sessions().is_empty());
        assert!(!child.seen.iter().any(|frame| frame["type"] == "ready"));
        assert!(!first.requests.lock().await.iter().any(|request| request["body"].is_object()));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_cycle_scoped_author_order_thinking_and_same_session_receipts() {
    for explicit in [true, false] {
        let env = Env::new();
        let first = upstream(json!({"responses":[auto_reply("cycled back to the original model")]})).await;
        let target = SwitchWire::new(json!({"responses":[auto_reply("scoped target call")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let path = env.home.path().join("agent/config.yml");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        settings["enabledModels"] = if explicit {
            json!([
                "switch-fixture/target-model:low",
                "auto-fixture/fake-model:high",
                "switch-fixture/target-model:high"
            ])
        } else {
            // A missing scope level clears the current effort. The target's
            // model default must not become a Session selector through cycle.
            settings.as_object_mut().unwrap().remove("defaultThinkingLevel");
            json!(["switch-fixture/target-model", "auto-fixture/fake-model"])
        };
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let settings_before = std::fs::read(&path).unwrap();
        let mut child = RpcChild::spawn(
            env.selection_command(&first.base_url(), &["--provider", "auto-fixture", "--thinking", "high"]),
        );
        child.ready();
        let before = child.state("cycle-before");
        child.send(json!({"id":"cycle-target","type":"cycle_model"}));
        let selected = child.success("cycle-target");
        assert_eq!(selected["data"]["isScoped"], true);
        assert_eq!(selected["data"]["model"]["id"], "target-model");
        if explicit {
            assert_eq!(selected["data"]["thinkingLevel"], "low", "first duplicate owns the level");
        } else {
            assert!(selected["data"].get("thinkingLevel").is_none(), "Native undefined is omitted on RPC");
        }
        let after = child.state("cycle-after");
        assert_eq!(after["sessionId"], before["sessionId"]);
        assert_eq!(after["sessionFile"], before["sessionFile"]);
        assert_eq!(after["model"]["id"], "target-model");
        child.send(json!({"id":"target-run","type":"prompt","message":"call the scoped target once"}));
        child.success("target-run");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        // Native MRU uses whole seconds without a tie-break. Age the actual
        // first adoption's row in this fixture before the next real adoption.
        // This proves both records without waiting for a clock boundary.
        let usage_db = rusqlite::Connection::open(env.home.path().join("agent/agent.db")).unwrap();
        assert_eq!(
            usage_db
                .execute(
                    "UPDATE model_usage SET last_used_at=1 WHERE model_key='switch-fixture/target-model' AND last_used_at>1",
                    [],
                )
                .unwrap(),
            1,
            "the first cycle must have recorded its actual target usage"
        );
        drop(usage_db);
        child.send(json!({"id":"cycle-original","type":"cycle_model"}));
        let original = child.success("cycle-original");
        assert_eq!(original["data"]["model"]["id"], "fake-model");
        assert_eq!(original["data"]["isScoped"], true);
        if explicit {
            assert_eq!(original["data"]["thinkingLevel"], "high");
        } else {
            assert!(original["data"].get("thinkingLevel").is_none());
        }
        child.send(json!({"id":"original-run","type":"prompt","message":"call the original once"}));
        child.success("original-run");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        child.finish(0);
        let requests = target.requests.lock().await;
        assert_eq!(requests.len(), 1, "cycling has no selected metadata GET");
        assert_eq!(requests[0]["body"]["model"], "target-model");
        if explicit {
            assert_eq!(requests[0]["body"]["reasoning_effort"], "low");
            assert_eq!(first.requests.lock().await[0]["body"]["reasoning_effort"], "high");
        }
        assert_eq!(*target.auth.lock().await, ["Bearer sk-target-fixture-only"]);
        assert_eq!(std::fs::read(&path).unwrap(), settings_before, "cycle does not update global roles/settings");
        let entries = journal(&env.sessions()[0]);
        let models: Vec<_> = entries.iter().filter(|entry| entry["type"] == "model_change").collect();
        assert_eq!(models.len(), 3, "startup records the initial model before the two cycle changes");
        assert_eq!(models[0]["model"], "auto-fixture/fake-model");
        let cycles = &models[1..];
        assert!(cycles.iter().all(|entry| entry.get("role").is_none() && entry["resolvedModelIsFallback"] == false));
        assert_eq!(cycles[0]["model"], "switch-fixture/target-model");
        assert_eq!(cycles[1]["model"], "auto-fixture/fake-model");
        let mru = ara_cli::model_usage_order::SqliteModelUsageOrder::for_agent_dir(env.home.path().join("agent"));
        assert_eq!(mru.get_model_usage_order().unwrap(), ["auto-fixture/fake-model", "switch-fixture/target-model"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_cycle_available_retains_effort_and_single_model_is_null() {
    for single in [false, true] {
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        let target = upstream(json!({"responses":[auto_reply("available target call")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let path = env.home.path().join("agent/models.yml");
        let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        models["providers"]["auto-fixture"]["models"].as_array_mut().unwrap().pop();
        if single {
            models["providers"].as_object_mut().unwrap().remove("switch-fixture");
        }
        std::fs::write(path, serde_json::to_vec(&models).unwrap()).unwrap();
        let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        child.send(json!({"id":"available","type":"get_available_models"}));
        assert_eq!(child.success("available")["data"]["models"].as_array().unwrap().len(), if single { 1 } else { 2 });
        child.send(json!({"id":"cycle","type":"cycle_model"}));
        let selected = child.success("cycle");
        if single {
            assert!(selected["data"].is_null());
            assert!(!child.seen.iter().any(|frame| frame["type"] == "model_changed"));
        } else {
            assert_eq!(selected["data"]["isScoped"], false);
            assert_eq!(selected["data"]["model"]["id"], "target-model");
            assert_eq!(selected["data"]["thinkingLevel"], "high", "current high wins over the target default low");
            child.send(json!({"id":"run","type":"prompt","message":"call the available target"}));
            child.success("run");
            child.until(|frame| frame["type"] == "agent_end", WAIT);
        }
        child.finish(0);
        assert!(first.requests.lock().await.is_empty());
        let requests = target.requests.lock().await;
        assert_eq!(requests.len(), usize::from(!single));
        if !single {
            assert_eq!(requests[0]["body"]["reasoning_effort"], "high");
        }
    }
}

impl Env {
    fn switch_models(&self, primary: &str, target: &str, auto: bool) {
        self.auto_models(primary, auto);
        let agent = self.home.path().join("agent");
        let path = agent.join("models.yml");
        let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        models["providers"]["switch-fixture"] = json!({
            "api":"openai-completions","apiKey":"sk-target-fixture-only","auth":"apiKey","baseUrl":target,
            "compat":{"maxTokensField":"max_tokens"},
            "models":[{"id":"target-model","contextWindow":131072,"maxTokens":256,"reasoning":true,
                "thinking":{"mode":"effort","efforts":["low","high"],"defaultLevel":"low"}}]
        });
        std::fs::write(path, serde_json::to_vec_pretty(&models).unwrap()).unwrap();
    }
}

// Reuse the controlled FakeUpstream, capturing only synthetic fixture auth
// before its ordinary recorder redacts header values.
struct SwitchWire {
    up: FakeUpstream,
    addr: std::net::SocketAddr,
    auth: Arc<tokio::sync::Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl std::ops::Deref for SwitchWire {
    type Target = FakeUpstream;
    fn deref(&self) -> &Self::Target {
        &self.up
    }
}
impl SwitchWire {
    async fn new(script: Value) -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let up = upstream(script).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let target = up.addr;
        let auth = Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let records = auth.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let records = records.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    while !bytes.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if socket.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        bytes.push(byte[0]);
                        assert!(bytes.len() <= 64 * 1024);
                    }
                    let head = String::from_utf8(bytes.clone()).unwrap();
                    let bearer = head
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default();
                    records.lock().await.push(bearer);
                    let mut forward = tokio::net::TcpStream::connect(target).await.unwrap();
                    forward.write_all(&bytes).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut forward).await;
                });
            }
        });
        Self { up, addr, auth, task }
    }
    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}
impl Drop for SwitchWire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_scope_same_identity_mru_and_target_generation() {
    for explicit_cap in [None, Some("512")] {
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        let target = SwitchWire::new(json!({"responses":[auto_reply("selected target")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let config = env.home.path().join("agent/config.yml");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        settings["enabledModels"] = json!(["auto-fixture/fake-model", "@task"]);
        settings["modelRoles"]["task"] = json!("switch-fixture/target-model");
        let settings_before = serde_json::to_vec_pretty(&settings).unwrap();
        std::fs::write(&config, &settings_before).unwrap();
        let mut args = vec!["--provider", "auto-fixture"];
        if let Some(cap) = explicit_cap {
            args.extend(["--max-tokens", cap]);
        }
        let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &args));
        child.ready();
        let initial = child.state("initial");
        child.send(json!({"id":"models","type":"get_available_models"}));
        let models = child.success("models")["data"]["models"].as_array().unwrap().clone();
        assert_eq!(models.len(), 2, "{models:?}");
        assert_eq!(models[0]["id"], "fake-model");
        assert_eq!(models[1]["id"], "target-model");
        assert!(models.iter().all(|model| model.get("headers").is_none() && model.get("apiKey").is_none()));
        for (provider, id) in [("Auto-fixture", "fake-model"), ("auto-fixture", "classifier-fixture")] {
            child.send(json!({"id":"missing","type":"set_model","provider":provider,"modelId":id}));
            let denied = child.response("missing");
            assert_eq!(denied["success"], false);
            assert_eq!(denied["error"], format!("Model not found: {provider}/{id}"));
            assert_eq!(child.state("unchanged")["model"], initial["model"]);
        }
        for id in ["target", "same"] {
            child.send(json!({"id":id,"type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
            let response = child.success(id);
            assert_eq!(response["data"]["id"], "target-model");
            assert_eq!(response["data"]["maxTokens"], 256);
        }
        let selected = child.state("selected");
        assert_eq!(selected["sessionId"], initial["sessionId"]);
        assert_eq!(selected["model"]["provider"], "switch-fixture");
        assert_eq!(selected["thinkingLevel"], "low");
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "model_changed").count(), 1);
        let changed = child.seen.iter().position(|frame| frame["type"] == "model_changed").unwrap();
        let thinking = child
            .seen
            .iter()
            .enumerate()
            .skip(changed)
            .find(|(_, frame)| frame["type"] == "thinking_level_changed")
            .unwrap()
            .0;
        assert!(changed < thinking, "Native model event precedes reapplied thinking");
        assert_eq!(std::fs::read(&config).unwrap(), settings_before, "RPC does not persist modelRoles");
        child.send(json!({"id":"run","type":"prompt","message":"use selected target"}));
        child.success("run");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        child.finish(0);
        let entries = journal(&env.sessions()[0]);
        let changes: Vec<_> = entries
            .iter()
            .filter(|entry| entry["type"] == "model_change" && entry["model"] == "switch-fixture/target-model")
            .collect();
        assert_eq!(changes.len(), 2, "same identity still records selection");
        assert!(changes.iter().all(|entry| entry["role"] == "default"));
        let db = rusqlite::Connection::open(env.home.path().join("agent/agent.db")).unwrap();
        let usage: Vec<String> = db
            .prepare("SELECT model_key FROM model_usage")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(usage, ["switch-fixture/target-model"]);
        assert!(first.requests.lock().await.is_empty());
        let requests = target.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["body"]["model"], "target-model");
        assert_eq!(requests[0]["body"]["reasoning_effort"], "low");
        assert_eq!(
            requests[0]["body"]["max_tokens"],
            explicit_cap.map(|cap| cap.parse::<u64>().unwrap()).unwrap_or(256)
        );
        assert_eq!(*target.auth.lock().await, ["Bearer sk-target-fixture-only"]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_during_active_call_and_host_tool_keeps_original_receipts() {
    for aborted in [false, true] {
        let env = Env::new();
        let first = upstream(json!({"responses":[{"events":[text("A visible"),{"sleep_ms":500},
            tool_call(0,"old-call","held_tool","{}"),finish("tool_calls"),done()]}]}))
        .await;
        let target = SwitchWire::new(json!({"responses":[auto_reply("B continuation")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        child.send(json!({"id":"tools","type":"set_host_tools","tools":[{
            "name":"held_tool","label":"Held tool","description":"controlled original adapter",
            "parameters":{"type":"object","properties":{}}
        }]}));
        child.success("tools");
        let initial = child.state("initial");
        child.send(json!({"id":"run","type":"prompt","message":"make one held tool call then continue"}));
        child.success("run");
        child.until(|frame| frame["type"] == "message_update" && frame.to_string().contains("A visible"), WAIT);
        child.send(json!({"id":"switch","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
        child.success("switch");
        let selected = child.state("selected-live");
        assert_eq!(selected["isStreaming"], true);
        assert_eq!(selected["sessionId"], initial["sessionId"]);
        assert_eq!(selected["model"]["id"], "target-model");
        assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"));
        let call = child.until(|frame| frame["type"] == "host_tool_call", WAIT);
        assert_eq!(call["toolCallId"], "old-call", "{call}");
        child.send(json!({"id":"same-held","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
        child.success("same-held");
        if aborted {
            child.send(json!({"id":"abort-switched-tool","type":"abort"}));
            child.until(|frame| frame["type"] == "agent_end", WAIT);
            child.success("abort-switched-tool");
        } else {
            child.send(json!({"type":"host_tool_result","id":call["id"],
                "result":{"content":[{"type":"text","text":"one original receipt"}],"isError":false}}));
            child.until(|frame| frame["type"] == "agent_end", WAIT);
        }
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
        assert_eq!(first.requests.lock().await.len(), 1);
        let requests = target.requests.lock().await;
        assert_eq!(requests.len(), usize::from(!aborted), "abort must not request the switched provider");
        if !aborted {
            assert_eq!(requests[0]["body"]["model"], "target-model");
            assert_eq!(requests[0]["body"]["reasoning_effort"], "low");
            assert!(
                requests[0]["body"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "tool" && message["content"] == "one original receipt")
            );
            assert_eq!(*target.auth.lock().await, ["Bearer sk-target-fixture-only"]);
        } else {
            assert!(target.auth.lock().await.is_empty());
        }
        drop(requests);
        assert_eq!(env.sessions().len(), 1);
        let entries = journal(&env.sessions()[0]);
        let assistants: Vec<_> = entries.iter().filter(|entry| entry["message"]["role"] == "assistant").collect();
        assert_eq!(assistants.len(), 2);
        assert_eq!(assistants[0]["message"]["provider"], "auto-fixture");
        assert_eq!(assistants[0]["message"]["model"], "fake-model");
        assert_eq!(assistants[1]["message"]["provider"], "switch-fixture");
        assert_eq!(assistants[1]["message"]["model"], "target-model");
        assert_eq!(assistants[1]["message"]["stopReason"], if aborted { "aborted" } else { "stop" });
        assert_eq!(entries.iter().filter(|entry| entry["message"]["toolCallId"] == "old-call").count(), 1);
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_start").count(), 1);
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "model_changed").count(), 1);
        if aborted {
            // Keep every original zero-dispatch/aborted receipt observation
            // above, then continue this same Session through its actual Graph
            // history. A typed synthetic metadata record rejects this request
            // at Host conversion before any target HTTP call can be emitted.
            let after_abort = child.state("settled-abort");
            assert_eq!(after_abort["sessionId"], initial["sessionId"]);
            assert_eq!(after_abort["isStreaming"], false);
            let original_host_calls = child.seen.iter().filter(|frame| frame["type"] == "host_tool_call").count();
            child.send(json!({"id":"after-abort","type":"prompt","message":"continue the same Session after abort"}));
            child.success("after-abort");
            child.until(|frame| frame["type"] == "agent_end", WAIT);
            let after = child.state("same-session-successor");
            assert_eq!(after["sessionId"], initial["sessionId"]);
            assert_eq!(after["model"]["id"], "target-model");
            assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_start").count(), 2);
            assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_end").count(), 2);
            assert_eq!(
                child.seen.iter().filter(|frame| frame["type"] == "host_tool_call").count(),
                original_host_calls,
                "successor must not replay the original held tool"
            );
            assert_eq!(first.requests.lock().await.len(), 1);
            let successor_requests = target.requests.lock().await;
            assert_eq!(successor_requests.len(), 1, "one normal target request after the prior zero-dispatch abort");
            assert_eq!(successor_requests[0]["body"]["model"], "target-model");
            assert_eq!(successor_requests[0]["body"]["reasoning_effort"], "low");
            assert!(
                successor_requests[0]["body"]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "user"
                        && message["content"] == "continue the same Session after abort")
            );
            assert_eq!(*target.auth.lock().await, ["Bearer sk-target-fixture-only"]);
            let after_entries = journal(&env.sessions()[0]);
            let after_assistants: Vec<_> =
                after_entries.iter().filter(|entry| entry["message"]["role"] == "assistant").collect();
            assert_eq!(after_assistants.len(), 3);
            assert_eq!(after_assistants[0]["message"], assistants[0]["message"]);
            assert_eq!(
                after_assistants[1]["message"], assistants[1]["message"],
                "retain the original live-model aborted metadata"
            );
            assert_eq!(after_assistants[2]["message"]["provider"], "switch-fixture");
            assert_eq!(after_assistants[2]["message"]["model"], "target-model");
            assert_eq!(after_assistants[2]["message"]["stopReason"], "stop");
            assert!(contains_text(&after_assistants[2]["message"], "B continuation"));
            assert_eq!(after_entries.iter().filter(|entry| entry["message"]["toolCallId"] == "old-call").count(), 1);
            assert_eq!(env.sessions().len(), 1);
        }
        child.finish(0);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_and_cycle_rebind_accepted_auto_input_and_preserve_nonmutating_input() {
    for (cycle, valid) in [(false, false), (false, true), (true, false), (true, true)] {
        let env = Env::new();
        let classifier = if valid {
            json!({"events":[text("high")], "end":"hang"})
        } else {
            json!({"events":[{"sleep_ms":500},text("high"),finish("stop"),done()]})
        };
        let first = upstream(json!({"responses":[classifier, auto_reply("original primary")]})).await;
        let target = upstream(json!({"responses":[auto_reply("new provisional primary")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), true);
        if cycle {
            let path = env.home.path().join("agent/config.yml");
            let mut settings: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            settings["enabledModels"] = if valid {
                json!(["auto-fixture/fake-model:low", "switch-fixture/target-model:high"])
            } else {
                json!(["auto-fixture/fake-model"])
            };
            std::fs::write(path, serde_json::to_vec(&settings).unwrap()).unwrap();
        }
        let mut child = RpcChild::spawn(
            env.selection_command(&first.base_url(), &["--provider", "auto-fixture", "--thinking", "auto"]),
        );
        child.ready();
        child.send(json!({"id":"accepted","type":"prompt","message":"retain this accepted user"}));
        child.success("accepted");
        wait_auto_requests(&first, 1).await;
        let model = if valid { "target-model" } else { "missing-model" };
        child.send(if cycle {
            json!({"id":"switch","type":"cycle_model"})
        } else {
            json!({"id":"switch","type":"set_model","provider":"switch-fixture","modelId":model})
        });
        let response = child.response("switch");
        assert_eq!(response["success"], cycle || valid);
        if cycle && !valid {
            assert!(response["data"].is_null(), "a one-entry cycle preserves pending accepted input");
        }
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        let state = child.state("after");
        assert_eq!(state["configuredThinkingLevel"], "auto");
        assert_eq!(state["model"]["id"], if valid { "target-model" } else { "fake-model" });
        assert_eq!(state["thinkingLevel"], if valid { "low" } else { "high" });
        child.finish(0);
        assert!(!child.seen.iter().any(|frame| frame["type"] == "prompt_result" && frame["agentInvoked"] == false));
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_start").count(), 1);
        let first_requests = first.requests.lock().await;
        let target_requests = target.requests.lock().await;
        assert_eq!(first_requests.len(), if valid { 1 } else { 2 });
        assert_eq!(target_requests.len(), usize::from(valid));
        if valid {
            assert_eq!(target_requests[0]["body"]["reasoning_effort"], "low");
        }
        let entries = journal(&env.sessions()[0]);
        assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 1);
        assert!(entries.iter().any(|entry| entry["message"].to_string().contains("retain this accepted user")));
        let usage: Vec<_> = entries.iter().filter(|entry| entry["type"] == "model_usage").collect();
        assert_eq!(usage.len(), 1, "the initiating classifier retains its actual terminal receipt");
        if valid {
            assert_eq!(usage[0]["stopReason"], "aborted");
            assert!(
                usage[0]["usage"]["input"].is_null() && usage[0]["usage"]["output"].is_null(),
                "cancelled before reported usage is unknown, not zero"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_revalidates_old_admission_after_selected_metadata_await() {
    let env = Env::new();
    let first = upstream(json!({"responses":[{"events":[
        tool_call(0,"before-selection","held_tool","{}"),finish("tool_calls"),done()]}]}))
    .await;
    let metadata = json!({"data":[{"id":"target-model","state":"loaded",
        "loaded_context_length":4096,"max_context_length":131072,"type":"llm"}]})
    .to_string();
    let initial_metadata = json!({"data":[{"id":"target-model","state":"loaded",
        "loaded_context_length":131072,"max_context_length":131072,"type":"llm"}]})
    .to_string();
    let target = SwitchWire::new(json!({"responses":[
        {"body":initial_metadata},{"body":initial_metadata},{"body":metadata,"delay_ms":500},auto_reply("new admission continuation")
    ]})).await;
    env.switch_models(&first.base_url(), &target.base_url(), false);
    let agent = env.home.path().join("agent");
    let path = agent.join("models.yml");
    let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    models["providers"]["switch-fixture"]["discovery"] = json!({"type":"lm-studio","timeoutMs":2000});
    models["providers"]["switch-fixture"]["models"][0].as_object_mut().unwrap().remove("contextWindow");
    std::fs::write(&path, serde_json::to_vec(&models).unwrap()).unwrap();
    let settings_path = agent.join("config.yml");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    settings["retry"]["usageAwareFallback"] = json!(true);
    std::fs::write(settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &["--provider", "auto-fixture"]));
    child.ready();
    child.send(json!({"id":"catalog","type":"get_available_models"}));
    child.success("catalog");
    let baseline = target.requests.lock().await.len();
    assert_eq!(baseline, 2, "one configured discovery catalog/native pair");
    child.send(json!({"id":"tools","type":"set_host_tools","tools":[{
        "name":"held_tool","description":"controlled old admission",
        "parameters":{"type":"object","properties":{}}
    }]}));
    child.success("tools");
    child.send(json!({"id":"run","type":"prompt","message":"hold one tool then continue the same Run"}));
    child.success("run");
    let held = child.until(|frame| frame["type"] == "host_tool_call", WAIT);
    child.send(json!({"id":"switch","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
    wait_model_selection_probe(&target, baseline + 1).await;
    assert!(!child.seen.iter().any(|frame| frame["type"] == "response" && frame["id"] == "switch"));
    child.send(json!({"type":"host_tool_result","id":held["id"],
        "result":{"content":[{"type":"text","text":"old adapter settled once"}]}}));
    // The actual tool settles while the serial model-selection handler is
    // still awaiting native metadata, so its continuation queues old admission.
    std::thread::sleep(Duration::from_millis(80));
    let selected = child.success("switch");
    assert_eq!(selected["data"]["contextWindow"], 131072, "RPC returns the original found model");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let state = child.state("after");
    assert_eq!(state["model"]["id"], "target-model");
    child.send(json!({"id":"refreshed","type":"get_available_models"}));
    let refreshed = child.success("refreshed");
    assert_eq!(
        refreshed["data"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["provider"] == "switch-fixture" && model["id"] == "target-model")
            .unwrap()["contextWindow"],
        4096
    );
    child.finish(0);
    let requests = target.requests.lock().await;
    assert_eq!(requests.len(), baseline + 2);
    assert_eq!(requests[baseline]["request"], "GET /api/v0/models HTTP/1.1");
    assert_eq!(requests.last().unwrap()["body"]["model"], "target-model");
    assert!(
        requests.last().unwrap()["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["role"] == "tool" && message["content"] == "old adapter settled once")
    );
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_start").count(), 1);
    let entries = journal(&env.sessions()[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["toolCallId"] == "before-selection").count(), 1);
    assert!(
        entries
            .iter()
            .any(|entry| entry["message"]["role"] == "assistant" && entry["message"]["model"] == "target-model")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_reader_controls_revoke_replacement_and_preserve_serial_input() {
    for stop in ["abort", "new_session", "eof", "prompt", "set_auto_retry"] {
        let revokes = matches!(stop, "abort" | "new_session" | "eof");
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        let metadata = json!({"data":[{"id":"target-model","state":"loaded",
            "loaded_context_length":131072,"max_context_length":131072,"type":"llm"}]})
        .to_string();
        let target = upstream(json!({"responses":[{"body":metadata},{"body":metadata},
            {"body":metadata,"delay_ms":500},auto_reply("queued input uses B")]}))
        .await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let path = env.home.path().join("agent/models.yml");
        let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        models["providers"]["switch-fixture"]["discovery"] = json!({"type":"lm-studio","timeoutMs":2000});
        std::fs::write(path, serde_json::to_vec(&models).unwrap()).unwrap();
        let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        child.send(json!({"id":"catalog","type":"get_available_models"}));
        child.success("catalog");
        let baseline = target.requests.lock().await.len();
        child.send(json!({"id":"switch","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
        wait_model_selection_probe(&target, baseline + 1).await;
        if stop == "eof" {
            child.stdin.take();
        } else {
            let mut command = json!({"id":"stop","type":stop});
            if stop == "prompt" {
                command["message"] = json!("use the model selected by the preceding command");
            }
            if stop == "set_auto_retry" {
                command["enabled"] = json!(false);
            }
            child.send(command);
        }
        let rejected = child.response("switch");
        assert_eq!(rejected["success"], !revokes, "Reader {stop} selection admission: {rejected}");
        if stop != "eof" {
            child.success("stop");
            if stop == "prompt" {
                child.until(|frame| frame["type"] == "agent_end", WAIT);
            }
            assert_eq!(
                child.state("after")["model"]["provider"],
                if revokes { "auto-fixture" } else { "switch-fixture" }
            );
        }
        child.finish(0);
        assert_eq!(child.seen.iter().any(|frame| frame["type"] == "model_changed"), !revokes);
        assert!(first.requests.lock().await.is_empty());
        if stop == "prompt" {
            let requests = target.requests.lock().await;
            assert_eq!(requests.len(), baseline + 2);
            assert_eq!(requests.last().unwrap()["body"]["model"], "target-model");
        }
        for path in env.sessions() {
            assert_eq!(
                journal(&path)
                    .iter()
                    .any(|entry| entry["type"] == "model_change" && entry["model"] == "switch-fixture/target-model"),
                !revokes,
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_selection_defers_command_auth_and_rejects_failed_journal_publication() {
    for fail_journal in [false, true] {
        let env = Env::new();
        let first = upstream(json!({"responses":[auto_reply("materialized original Session")]})).await;
        let target = SwitchWire::new(json!({"responses":[auto_reply("command account selected")]})).await;
        env.switch_models(&first.base_url(), &target.base_url(), false);
        let marker = env.home.path().join("selected-key.count");
        #[cfg(windows)]
        let helper = {
            let path = env.home.path().join("selected-key.cmd");
            std::fs::write(
                &path,
                format!("@echo off\r\n>>\"{}\" echo run\r\necho sk-target-fixture-only\r\n", marker.display()),
            )
            .unwrap();
            format!("!call \"{}\"", path.display())
        };
        #[cfg(not(windows))]
        let helper = format!("!printf 'run\\n' >> '{}'; printf %s sk-target-fixture-only", marker.display());
        let path = env.home.path().join("agent/models.yml");
        let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        models["providers"]["switch-fixture"]["apiKey"] = json!(helper);
        std::fs::write(path, serde_json::to_vec(&models).unwrap()).unwrap();
        let mut child = RpcChild::spawn(env.selection_command(&first.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        let startup_calls = std::fs::read_to_string(&marker).unwrap().lines().count();
        assert_eq!(startup_calls, 1, "Native startup eagerly installs configured provider keys");
        if fail_journal {
            child.send(json!({"id":"materialize","type":"prompt","message":"materialize the original Session"}));
            child.success("materialize");
            child.until(|frame| frame["type"] == "agent_end", WAIT);
            assert_eq!(first.requests.lock().await.len(), 1);
            let state = child.state("journal-path");
            let path = PathBuf::from(state["sessionFile"].as_str().unwrap());
            assert!(journal(&path).iter().any(|entry| entry["message"]["role"] == "assistant"));
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(path).unwrap();
        }
        child.send(json!({"id":"switch","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
        let selected = child.response("switch");
        assert_eq!(selected["success"], !fail_journal, "{selected}");
        assert_eq!(
            std::fs::read_to_string(&marker).unwrap().lines().count(),
            startup_calls,
            "explicit selection must only check configured auth presence"
        );
        assert!(target.requests.lock().await.is_empty());
        if fail_journal {
            child.finish(1);
            assert!(!child.seen.iter().any(|frame| frame["type"] == "model_changed"));
        } else {
            child.send(json!({"id":"run","type":"prompt","message":"resolve key only for this selected call"}));
            child.success("run");
            child.until(|frame| frame["type"] == "agent_end", WAIT);
            child.finish(0);
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap().lines().count(),
                startup_calls,
                "the actual selected request shares Native command-value cache"
            );
            assert_eq!(*target.auth.lock().await, ["Bearer sk-target-fixture-only"]);
            assert_eq!(target.requests.lock().await.len(), 1);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[cfg(feature = "test-fixture")]
async fn rpc_model_selection_rejects_explicit_codex_caps_before_adopting_configured_account() {
    use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
    for extra in [["--max-tokens", "512"], ["--temperature", "0.5"]] {
        let env = Env::new();
        let first = upstream(json!({"responses":[]})).await;
        env.auto_models(&first.base_url(), false);
        let path = env.home.path().join("agent/models.yml");
        let mut models: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        models["providers"]["openai-codex"] = json!({"api":"openai-codex-responses","auth":"oauth",
            "baseUrl":first.base_url(),"models":[{"id":"codex-target","input":["text"]}]});
        std::fs::write(path, serde_json::to_vec(&models).unwrap()).unwrap();
        let store = SqliteCredentialStore::open(env.home.path().join("agent/auth.db")).unwrap();
        let fields = json!({"access":"synthetic-account-access","refresh":"synthetic-account-refresh",
            "expires":chrono::Utc::now().timestamp_millis()+3_600_000,
            "authorizedAt":chrono::Utc::now().timestamp_millis(),"accountId":"synthetic-account"});
        store
            .upsert_auth_credential_for_provider(
                "openai-codex",
                &AuthCredential::oauth(fields.as_object().unwrap().clone()),
            )
            .unwrap();
        let mut child = RpcChild::spawn(
            env.selection_command(&first.base_url(), &["--provider", "auto-fixture", extra[0], extra[1]]),
        );
        child.ready();
        let initial = child.state("before");
        child.send(json!({"id":"switch","type":"set_model","provider":"openai-codex","modelId":"codex-target"}));
        let rejected = child.response("switch");
        assert_eq!(rejected["success"], false);
        assert!(rejected["error"].as_str().unwrap().contains("output caps and sampling"), "{rejected}");
        assert_eq!(child.state("after")["model"], initial["model"]);
        child.finish(0);
        assert!(!child.seen.iter().any(|frame| frame["type"] == "model_changed"));
        assert!(first.requests.lock().await.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_native_thinking_cli_initial_and_resume_keep_saved_receipts() {
    let env = Env::new();
    let up = upstream(json!({"responses":[auto_reply("fixed low"),auto_reply("override high"),auto_reply("low"),auto_reply("auto low")]})).await;
    env.auto_models(&up.base_url(), false);
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture", "--thinking", "lo"]));
    child.until(|frame| frame["type"] == "ready", WAIT);
    assert_eq!(child.state("initial")["thinkingLevel"], "low");
    child.send(json!({"id":"one","type":"prompt","message":"first fixed User"}));
    child.success("one");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    let file = env.sessions().pop().unwrap();
    let before =
        journal(&file).into_iter().filter(|entry| entry["type"] == "thinking_level_change").collect::<Vec<_>>();
    assert_eq!(before.len(), 1);
    let path = file.to_str().unwrap();
    let mut child = RpcChild::spawn(
        env.command(&up.base_url(), &["--provider", "auto-fixture", "--resume", path, "--thinking", "hi"]),
    );
    child.until(|frame| frame["type"] == "ready", WAIT);
    let state = child.state("resumed");
    assert_eq!(state["thinkingLevel"], "high");
    assert_eq!(state["configuredThinkingLevel"], "high");
    child.send(json!({"id":"two","type":"prompt","message":"runtime override User"}));
    child.success("two");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(
        journal(&file).into_iter().filter(|entry| entry["type"] == "thinking_level_change").collect::<Vec<_>>(),
        before
    );
    let mut child = RpcChild::spawn(
        env.command(&up.base_url(), &["--provider", "auto-fixture", "--resume", path, "--thinking", "auto"]),
    );
    child.until(|frame| frame["type"] == "ready", WAIT);
    assert_eq!(child.state("auto-initial")["configuredThinkingLevel"], "auto");
    child.send(json!({"id":"three","type":"prompt","message":"classified User"}));
    child.success("three");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0]["body"]["reasoning_effort"], "low");
    assert_eq!(requests[1]["body"]["reasoning_effort"], "high");
    assert_eq!(requests[2]["body"]["model"], "classifier-fixture");
    assert_eq!(requests[3]["body"]["reasoning_effort"], "low");
}

async fn wait_model_selection_probe(up: &FakeUpstream, count: usize) {
    let started = Instant::now();
    tokio::time::timeout(WAIT, async {
        while up.requests.lock().await.len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("selected native metadata probe reached the controlled HTTP route");
    eprintln!("selected metadata probe observed after {:?}", started.elapsed());
}

async fn wait_auto_requests(up: &FakeUpstream, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while up.requests.lock().await.len() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("classifier reached the actual controlled HTTP route");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_users_tool_continuation_usage_pin_and_same_file_reload() {
    let up = upstream(json!({"responses":[
        auto_reply("low"),
        {"events":[tool_call(0,"auto-write","write",r#"{"path":"auto.txt","content":"one"}"#),finish("tool_calls"),
            {"data":{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}},done()]},
        auto_reply("wrote one"), auto_reply("high"), auto_reply("second complete"),
        auto_reply("pinned high"), auto_reply("reloaded low"), auto_reply("reloaded high")
    ]}))
    .await;
    let env = Env::new();
    env.auto_models(&up.base_url(), false);
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
    child.ready();
    let session = child.state("initial")["sessionId"].clone();
    child.send(json!({"id":"auto","type":"set_thinking_level","level":"auto"}));
    child.success("auto");
    for (id, message, effort) in
        [("first", "write auto.txt with one", "low"), ("second", "solve a harder problem", "high")]
    {
        child.send(json!({"id":id,"type":"prompt","message":message}));
        child.success(id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        let state = child.state(&format!("{id}-state"));
        assert_eq!(state["sessionId"], session);
        assert_eq!(state["thinkingLevel"], effort, "host stderr: {}", child.errors());
        assert_eq!(state["configuredThinkingLevel"], "auto");
    }
    assert_eq!(std::fs::read_to_string(env.work.path().join("auto.txt")).unwrap(), "one");
    // Even a pin to the currently resolved effort exits Auto.
    child.send(json!({"id":"pin","type":"set_thinking_level","level":"high"}));
    child.success("pin");
    child.send(json!({"id":"third","type":"prompt","message":"keep the explicit high pin"}));
    child.success("third");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let path = env.sessions().pop().unwrap();
    child.adopt("same-file", json!({"id":"same-file","type":"switch_session","sessionPath":path}));
    for effort in ["low", "high"] {
        let id = format!("reload-{effort}");
        child.send(json!({"id":format!("{id}-pin"),"type":"set_thinking_level","level":effort}));
        child.success(&format!("{id}-pin"));
        child.send(json!({"id":id,"type":"prompt","message":format!("use explicit {effort} after reload")}));
        child.success(&id);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        assert_eq!(child.state(&format!("{id}-state"))["thinkingLevel"], effort);
    }
    let messages = child.messages("public-messages");
    assert_eq!(user_texts(&messages).len(), 5);
    assert!(!messages.iter().any(|message| message["model"] == "classifier-fixture"));
    child.send(json!({"id":"stats","type":"get_session_stats"}));
    let stats = child.success("stats")["data"].clone();
    assert_eq!(stats["totalMessages"], messages.len());
    assert_eq!(stats["toolCalls"], 1);
    assert_eq!(stats["tokens"]["total"], 32, "includes private classifier terminals");
    child.finish(0);
    let entries = journal(&path);
    let usage: Vec<_> = entries.iter().filter(|entry| entry["type"] == "model_usage").collect();
    assert_eq!(usage.len(), 2);
    assert!(usage.iter().all(|entry| entry["purpose"] == "auto-thinking" && entry["model"] == "classifier-fixture"));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 8);
    for index in [0, 3] {
        assert_eq!(requests[index]["body"]["model"], "classifier-fixture");
        assert_eq!(requests[index]["body"]["max_tokens"], 4096);
        assert!(requests[index]["body"].get("tools").is_none());
    }
    for (index, effort) in [(1, "low"), (2, "low"), (4, "high"), (5, "high"), (6, "low"), (7, "high")] {
        assert_eq!(requests[index]["body"]["model"], "fake-model");
        assert_eq!(requests[index]["body"]["reasoning_effort"], effort, "actual wire request {index}");
        assert!(!requests[index]["body"]["messages"].to_string().contains("Classify"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_accepted_prompt_finishes_after_input_eof() {
    let up = upstream(json!({"responses":[auto_reply("low"),auto_reply("completed after EOF")]})).await;
    let env = Env::new();
    env.auto_models(&up.base_url(), true);
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
    child.ready();
    child.send(json!({"id":"eof-prompt","type":"prompt","message":"finish this already accepted turn"}));
    child.stdin.take();
    child.finish(0);
    assert!(child.seen.iter().any(|frame| frame["type"] == "agent_end"));
    assert!(
        child
            .seen
            .iter()
            .any(|frame| frame["type"] == "message_end" && contains_text(&frame["message"], "completed after EOF"))
    );
    assert!(!child.seen.iter().any(|frame| frame["type"] == "response" && frame["success"] == false));
    let entries = journal(&env.sessions().pop().unwrap());
    assert_eq!(entries.iter().filter(|entry| entry["type"] == "model_usage").count(), 1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["body"]["reasoning_effort"], "low");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_deadline_falls_back_and_host_remains_responsive() {
    let mut delayed = auto_reply("low");
    delayed["delay_ms"] = json!(4500);
    let up = upstream(json!({"responses":[delayed,auto_reply("fallback high")]})).await;
    let env = Env::new();
    env.auto_models(&up.base_url(), true);
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
    child.ready();
    child.send(json!({"id":"timeout","type":"prompt","message":"classify with a bounded deadline"}));
    child.success("timeout");
    wait_auto_requests(&up, 1).await;
    let start = Instant::now();
    let state = child.state("during-classification");
    assert!(start.elapsed() < Duration::from_secs(2), "serial Host stays responsive");
    assert_eq!(state["isStreaming"], true);
    assert!(child.messages("pending-history").is_empty(), "initial User waits outside Agent/journal");
    child.send(json!({"id":"pending-page","type":"get_messages_page","limit":2}));
    assert_eq!(child.response("pending-page")["code"], "session_busy");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    let state = child.state("after-timeout");
    assert_eq!(state["thinkingLevel"], "high", "Native provisional/default fallback");
    assert_eq!(state["configuredThinkingLevel"], "auto");
    child.finish(0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1]["body"]["reasoning_effort"], "high");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_explicit_pin_abort_and_session_replacement_supersede_pending_classification() {
    for action in ["pin", "abort", "new_session"] {
        let mut delayed = auto_reply("low");
        delayed["delay_ms"] = json!(5000);
        let up = upstream(json!({"responses":[delayed,auto_reply("current main only")]})).await;
        let env = Env::new();
        env.auto_models(&up.base_url(), false);
        let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        child.send(json!({"id":"auto","type":"set_thinking_level","level":"auto"}));
        child.success("auto");
        let original = child.state("original");
        child.send(json!({"id":"pending","type":"prompt","message":"pending classification"}));
        child.success("pending");
        wait_auto_requests(&up, 1).await;
        let changed = if action == "pin" {
            json!({"id":"change","type":"set_thinking_level","level":"low"})
        } else {
            json!({"id":"change","type":action})
        };
        child.send(changed);
        child.success("change");
        if action == "pin" {
            child.until(|frame| frame["type"] == "agent_end", WAIT);
        } else {
            assert_eq!(up.requests.lock().await.len(), 1, "abort/replacement vetoes the previous main POST");
            assert!(child.messages("revoked-history").is_empty(), "revoked input was never journaled");
            assert_eq!(
                child
                    .seen
                    .iter()
                    .filter(|frame| frame["type"] == "prompt_result"
                        && frame["id"] == "pending"
                        && frame["agentInvoked"] == false)
                    .count(),
                1
            );
            if action == "new_session" {
                assert_ne!(child.state("new-owner")["sessionId"], original["sessionId"]);
            }
            child.send(json!({"id":"explicit","type":"set_thinking_level","level":"high"}));
            child.success("explicit");
            child.send(json!({"id":"fresh","type":"prompt","message":"use the current owner and explicit pin"}));
            child.success("fresh");
            child.until(|frame| frame["type"] == "agent_end", WAIT);
        }
        child.finish(0);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 2, "{action} launches only the current main call");
        assert_eq!(requests[1]["body"]["model"], "fake-model");
        assert_eq!(requests[1]["body"]["reasoning_effort"], if action == "pin" { "low" } else { "high" });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_native_steering_and_followup_queue_do_not_add_classifiers() {
    for during_pending in [true, false] {
        let mut classification = auto_reply("low");
        if during_pending {
            classification["delay_ms"] = json!(2500);
        }
        let up = upstream(json!({"responses":[
            classification,
            {"events":[text("Before Auto queues."),{"sleep_ms":700},finish("stop"),done()]},
            auto_reply("steered"), auto_reply("followed up")
        ]}))
        .await;
        let env = Env::new();
        env.auto_models(&up.base_url(), true);
        let seeded = if during_pending {
            let mut journal = ara_session::SessionJournal::create(&env.sessions, env.work.path()).unwrap();
            // Match launch identity so startup does not append a newer model
            // selection after the expected initiating leaf was captured.
            journal.append_model_change("auto-fixture/fake-model").unwrap();
            let owner = journal.append_thinking_level_change(Some("high"), Some("auto")).unwrap();
            journal.materialize().unwrap();
            Some((journal.path().to_owned(), owner))
        } else {
            None
        };
        let mut command = env.command(&up.base_url(), &["--provider", "auto-fixture"]);
        if let Some((path, _)) = &seeded {
            command.arg("--resume").arg(path);
        }
        let mut child = RpcChild::spawn(command);
        child.ready();
        child.send(json!({"id":"initial","type":"prompt","message":"initial real user"}));
        child.success("initial");
        let owner = if during_pending {
            wait_auto_requests(&up, 1).await;
            assert!(child.messages("held-history").is_empty());
            let owner = json!(seeded.as_ref().unwrap().1);
            // The actual rejected commands must leave the accepted initial turn
            // alive. A metadata append independently advances the current leaf.
            child.send(json!({"id":"busy","type":"prompt","message":"must be rejected"}));
            let refusal = child
                .until(|frame| frame["type"] == "response" && frame["id"] == "busy" && frame["success"] == false, WAIT);
            assert!(refusal["error"].as_str().unwrap().contains("already running"));
            child.send(json!({"id":"invalid-branch","type":"branch","entryId":"absent"}));
            assert_eq!(child.response("invalid-branch")["success"], false);
            child.send(json!({"id":"policy","type":"set_auto_retry","enabled":false}));
            child.success("policy");
            child.send(json!({"id":"name","type":"set_session_name","name":"independent successor"}));
            child.success("name");
            Some(owner)
        } else {
            child.until(
                |frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Before Auto queues."),
                WAIT,
            );
            None
        };
        for (id, kind, message) in [("steer", "steer", "queued steering"), ("follow", "follow_up", "queued followup")] {
            if during_pending {
                child.send(json!({"id":id,"type":"prompt","streamingBehavior":if kind=="steer" {"steer"} else {"followUp"},"message":message}));
            } else {
                child.send(json!({"id":id,"type":kind,"message":message}));
            }
            child.success(id);
        }
        assert_eq!(child.state("queued")["queuedMessageCount"], 2);
        if during_pending {
            assert_eq!(up.requests.lock().await.len(), 1, "queues do not overtake the held classifier");
            assert!(child.messages("queued-history").is_empty());
        }
        child.finish(0);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), if during_pending { 3 } else { 4 });
        assert_eq!(requests.iter().filter(|request| request["body"]["model"] == "classifier-fixture").count(), 1);
        // Fixed agent-loop.ts:1071 dequeues initial steering before its first
        // model call. A steer queued while classification waits joins that
        // first main request; a steer arriving during main needs another call.
        let steered = if during_pending { 1 } else { 2 };
        assert!(requests[steered]["body"]["messages"].to_string().contains("queued steering"));
        assert!(requests[steered + 1]["body"]["messages"].to_string().contains("queued followup"));
        let entries = journal(&env.sessions().pop().unwrap());
        let usage: Vec<_> = entries.iter().filter(|entry| entry["type"] == "model_usage").collect();
        assert_eq!(usage.len(), 1);
        if let Some(owner) = owner {
            assert_eq!(usage[0]["parentId"], owner, "usage retains its original leaf after metadata advances it");
        }
        let users: Vec<_> =
            entries.iter().filter(|entry| entry["type"] == "message" && entry["message"]["role"] == "user").collect();
        assert_eq!(users.len(), 3, "accepted initial and two queued inputs only");
        assert!(!users.iter().any(|entry| entry["message"].to_string().contains("must be rejected")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_classification_precedes_preprompt_compaction_and_user_skill_journal() {
    use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
    use ara_session::SessionJournal;
    for case in ["user", "skill", "eof-before-handoff", "eof-during-handoff", "abort-during-handoff"] {
        let skill = case == "skill";
        let handoff = case.ends_with("handoff");
        let mut classification = auto_reply("low");
        classification["delay_ms"] = json!(1500);
        let mut summary_reply = auto_reply("preserved source summary");
        if handoff {
            summary_reply["delay_ms"] = json!(1500);
        }
        let up = upstream(json!({"responses":[classification,summary_reply,auto_reply("main complete")]})).await;
        let env = Env::new();
        env.auto_models(&up.base_url(), true);
        let config = env.home.path().join("agent/config.yml");
        let mut settings: Value = serde_json::from_slice(&std::fs::read(&config).unwrap()).unwrap();
        settings["compaction"] = json!({"enabled":true,"methodOrder":[if handoff {"handoff"} else {"soft"}]});
        std::fs::write(config, serde_json::to_vec(&settings).unwrap()).unwrap();
        if skill {
            invocation_skill(&env, ".ara/skills", "proof", "Expanded classifier task marker.");
        }
        let mut seed = SessionJournal::create(&env.sessions, env.work.path()).unwrap();
        for (input, answer, tokens) in
            [("old user source", "old answer source", 1), ("kept user source", "kept billed source", 100_000)]
        {
            seed.append_message(&Message::User(UserMessage::text(input))).unwrap();
            let mut message = AssistantMessage::empty("openai-completions", "auto-fixture", "fake-model");
            message.content.push(AssistantBlock::text(answer));
            message.usage.input = Some(tokens);
            message.usage.total_tokens = Some(tokens);
            seed.append_message(&Message::Assistant(message)).unwrap();
        }
        let owner = seed.append_thinking_level_change(Some("high"), Some("auto")).unwrap();
        let path = seed.path().to_owned();
        drop(seed);
        let mut child = RpcChild::spawn(env.command(
            &up.base_url(),
            &[
                "--provider",
                "auto-fixture",
                "--resume",
                path.to_str().unwrap(),
                "--compact-threshold",
                "50000",
                "--compact-keep-tokens",
                "6",
            ],
        ));
        if skill {
            child.ready_commands(proof_commands());
        } else {
            child.ready();
        }
        let message = if skill { "/skill:proof pending new input" } else { "pending new input" };
        child.send(json!({"id":"ordered","type":"prompt","message":message}));
        child.success("ordered");
        wait_auto_requests(&up, 1).await;
        assert_eq!(child.state("held")["isStreaming"], true);
        assert_eq!(child.messages("before-invocation").len(), 4);
        assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_compaction_start"));
        assert_eq!(journal(&path).last().unwrap()["id"], owner, "classification captures the preprompt leaf");
        if case == "abort-during-handoff" {
            wait_auto_requests(&up, 2).await;
            child.send(json!({"id":"stop-maintenance","type":"abort"}));
            child.success("stop-maintenance");
            assert_eq!(child.messages("aborted-maintenance").len(), 4);
            child.finish(0);
            assert_eq!(up.requests.lock().await.len(), 2, "abort still cancels the accepted maintenance");
            assert_eq!(
                child
                    .seen
                    .iter()
                    .filter(|frame| frame["type"] == "prompt_result"
                        && frame["id"] == "ordered"
                        && frame["agentInvoked"] == false)
                    .count(),
                1
            );
            assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"));
            assert!(!journal(&path).iter().any(|entry| entry["message"].to_string().contains("pending new input")));
            continue;
        } else if case.starts_with("eof") {
            if case == "eof-during-handoff" {
                wait_auto_requests(&up, 2).await;
            }
            child.stdin.take();
            child.finish(0);
            assert!(!child.seen.iter().any(|frame| frame["type"] == "prompt_result" || frame["success"] == false));
        } else {
            child.until(|frame| frame["type"] == "agent_end", WAIT);
            child.state("settled");
            child.finish(0);
        }
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 3, "classifier then summary then main: {}", child.errors());
        assert_eq!(requests[0]["body"]["model"], "classifier-fixture");
        if skill {
            assert!(requests[0]["body"]["messages"].to_string().contains("Expanded classifier task marker."));
        }
        assert_eq!(requests[1]["body"]["model"], "fake-model");
        // Fixed custom Chat compatibility may omit an empty tools array.
        // The summary has no tools; the main request carries the Host tools.
        if handoff {
            assert!(
                !requests[1]["body"]["tools"].as_array().unwrap().is_empty(),
                "native handoff keeps live context/tools"
            );
        } else {
            assert!(requests[1]["body"].get("tools").is_none_or(|tools| tools == &json!([])));
        }
        assert!(!requests[2]["body"]["tools"].as_array().unwrap().is_empty());
        assert!(!requests[1]["body"]["messages"].to_string().contains("pending new input"));
        assert!(requests[2]["body"]["messages"].to_string().contains("pending new input"));
        assert_eq!(requests[2]["body"]["reasoning_effort"], "low");
        let entries = journal(&path);
        let usage = entries.iter().position(|entry| entry["type"] == "model_usage").unwrap();
        assert_eq!(entries[usage]["parentId"], owner);
        let summary = entries.iter().position(|entry| entry["type"] == "compaction").unwrap();
        let input = entries
            .iter()
            .position(|entry| {
                (entry["type"] == "message" || entry["type"] == "custom_message")
                    && entry["message"].to_string().contains("pending new input")
                    || entry["customType"] == "skill-prompt"
                        && entry["content"].to_string().contains("pending new input")
            })
            .unwrap();
        assert!(usage < summary && summary < input, "native usage/maintenance/input publication order");
        assert_eq!(entries.iter().filter(|entry| entry["type"] == "compaction").count(), 1);
        assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_role_selection_reads_native_mru_without_recording_ordinary_calls() {
    let up = upstream(json!({"responses":[auto_reply("low"),auto_reply("selected MRU classifier")]})).await;
    let env = Env::new();
    env.auto_models(&up.base_url(), true);
    let agent = env.home.path().join("agent");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(agent.join("config.yml")).unwrap()).unwrap();
    settings["modelRoles"]["tiny"] = json!(["auto-fixture/classifier-*"]);
    std::fs::write(agent.join("config.yml"), serde_json::to_vec(&settings).unwrap()).unwrap();
    let mut models: Value = serde_json::from_slice(&std::fs::read(agent.join("models.yml")).unwrap()).unwrap();
    let mut alternate = models["providers"]["auto-fixture"]["models"][1].clone();
    alternate["id"] = json!("classifier-alternate");
    models["providers"]["auto-fixture"]["models"].as_array_mut().unwrap().push(alternate);
    std::fs::write(agent.join("models.yml"), serde_json::to_vec(&models).unwrap()).unwrap();
    let db_path = agent.join("agent.db");
    let db = rusqlite::Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE model_usage(model_key TEXT PRIMARY KEY,last_used_at INTEGER NOT NULL);
        INSERT INTO model_usage VALUES('auto-fixture/classifier-fixture',1),('auto-fixture/classifier-alternate',2);",
    )
    .unwrap();
    drop(db);
    let before = std::fs::read(&db_path).unwrap();
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
    child.ready();
    child.send(json!({"id":"mru","type":"prompt","message":"choose the last explicitly selected classifier"}));
    child.success("mru");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["body"]["model"], "classifier-alternate");
    assert_eq!(requests[1]["body"]["model"], "fake-model");
    assert_eq!(std::fs::read(db_path).unwrap(), before, "startup and ordinary calls do not write MRU");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_auto_keyless_classifier_and_missing_nonkeyless_credentials() {
    for keyless in [true, false] {
        let responses = if keyless {
            vec![auto_reply("low"), auto_reply("keyless main complete")]
        } else {
            vec![auto_reply("missing classifier uses provisional high")]
        };
        let up = upstream(json!({"responses":responses})).await;
        let env = Env::new();
        env.auto_models(&up.base_url(), true);
        let agent = env.home.path().join("agent");
        let mut models: Value = serde_json::from_slice(&std::fs::read(agent.join("models.yml")).unwrap()).unwrap();
        let classifier = models["providers"]["auto-fixture"]["models"].as_array_mut().unwrap().pop().unwrap();
        models["providers"]["classifier-owner"] = json!({"api":"openai-completions","baseUrl":up.base_url(),
            "auth":if keyless {"none"} else {"oauth"},"models":[classifier]});
        std::fs::write(agent.join("models.yml"), serde_json::to_vec(&models).unwrap()).unwrap();
        let mut settings: Value = serde_json::from_slice(&std::fs::read(agent.join("config.yml")).unwrap()).unwrap();
        settings["modelRoles"]["tiny"] = json!(["classifier-owner/classifier-fixture"]);
        std::fs::write(agent.join("config.yml"), serde_json::to_vec(&settings).unwrap()).unwrap();
        let mut child = RpcChild::spawn(env.command(&up.base_url(), &["--provider", "auto-fixture"]));
        child.ready();
        child.send(json!({"id":"keyless","type":"prompt","message":"use only the classifier's authored auth"}));
        child.success("keyless");
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        assert_eq!(child.state("result")["thinkingLevel"], if keyless { "low" } else { "high" }, "{}", child.errors());
        child.finish(0);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), if keyless { 2 } else { 1 });
        if keyless {
            assert_eq!(requests[0]["body"]["model"], "classifier-fixture");
            assert!(requests[0]["headers"].get("authorization").is_none());
        }
        assert_eq!(requests.last().unwrap()["body"]["model"], "fake-model");
        assert!(requests.last().unwrap()["headers"].get("authorization").is_some());
    }
}

// Append to crates/ara-cli/tests/rpc_host.rs; uses its existing process fixtures.
// One child covers repeated model changes, retained snapshots, MCP/Host adapters,
// disabled/order preservation, and the existing RPC fresh-Session store boundary.

fn model_tools_definition<'a>(state: &'a Value, name: &str) -> &'a Value {
    state["dumpTools"].as_array().unwrap().iter().find(|tool| tool["name"] == name).unwrap()
}

fn assert_model_tools_snapshot(state: &Value, replace: bool) {
    let edit = model_tools_definition(state, "edit");
    let properties = edit["parameters"]["properties"].as_object().unwrap();
    assert_eq!(properties.contains_key("old_string"), replace, "{edit}");
    assert_eq!(properties.contains_key("input"), !replace, "{edit}");
    assert_eq!(
        model_tools_definition(state, "read")["description"].as_str().unwrap().contains("snapshot header"),
        !replace
    );
    let names: Vec<_> =
        state["dumpTools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["read", "write", "edit", "grep", "mcp__fixture__echo", "held_tool"]);
}

fn model_tools_prompt(child: &mut RpcChild, id: &str, held_id: Option<&str>) {
    child.send(json!({"id":id,"type":"prompt","message":id}));
    child.success(id);
    let mut held = 0;
    loop {
        let frame = child.until(|frame| frame["type"] == "host_tool_call" || frame["type"] == "agent_end", WAIT);
        if frame["type"] == "agent_end" {
            break;
        }
        assert_eq!(frame["toolCallId"].as_str(), held_id, "{frame}");
        held += 1;
        child.send(json!({"type":"host_tool_result","id":frame["id"],
            "result":{"content":[{"type":"text","text":"retained Host adapter"}],"isError":false}}));
    }
    assert_eq!(held, usize::from(held_id.is_some()));
}

fn model_tools_receipt<'a>(messages: &'a [Value], id: &str) -> &'a Value {
    messages.iter().find(|message| message["toolCallId"] == id).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_model_tools_repeat_switch_retains_store_and_external_adapters_then_fresh_rpc_store() {
    let env = Env::new();
    let work = env.work.path();
    std::fs::create_dir_all(work.join("src")).unwrap();
    let source = "def add(a, b):\n    return a - b  # BUG\n";
    std::fs::write(work.join("src/math.py"), source).unwrap();
    std::fs::write(work.join("format.txt"), "plain marker\n").unwrap();
    let anchored = json!({"input":"[math.py#450E]\nPUT 2.=2:\n+    return a + b\n"}).to_string();
    let first = upstream(json!({"responses":[
        {"events":[tool_call(0,"before-switch-read","read","{\"path\":\"src/math.py\"}"),finish("tool_calls"),done()]},
        auto_reply("snapshot recorded"),
        {"events":[tool_call(0,"retained-snapshot-edit","edit",&anchored),finish("tool_calls"),done()]},
        auto_reply("old snapshot reused"),
        {"events":[tool_call(0,"fresh-snapshot-edit","edit",&anchored),finish("tool_calls"),done()]},
        auto_reply("fresh Session failure inspected")
    ]}))
    .await;
    let marker = env.home.path().join("retained-mcp-marker.txt");
    let target = upstream(json!({"responses":[
        {"events":[tool_call(0,"target-read-one","read","{\"path\":\"format.txt\"}"),finish("tool_calls"),done()]},
        auto_reply("plain read one"),
        {"events":[tool_call(0,"target-read-two","read","{\"path\":\"format.txt\"}"),finish("tool_calls"),done()]},
        {"events":[tool_call(0,"retained-mcp-call","mcp__fixture__echo",&json!({"text":"after repeated switch","marker":marker}).to_string()),finish("tool_calls"),done()]},
        {"events":[tool_call(0,"retained-host-call","held_tool","{}"),finish("tool_calls"),done()]},
        auto_reply("both external adapters retained")
    ]})).await;
    env.switch_models(&first.base_url(), &target.base_url(), false);
    let settings_path = env.home.path().join("agent/config.yml");
    let mut settings: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    settings["edit"] = json!({"mode":"hashline","modelVariants":{"switch-fixture/target-model":"replace"}});
    settings["enabledModels"] = json!(["auto-fixture/fake-model", "switch-fixture/target-model"]);
    std::fs::write(&settings_path, serde_json::to_vec(&settings).unwrap()).unwrap();
    let mcp_record = env.home.path().join("mcp-model-tools-record.json");
    let mcp_config = env.home.path().join("mcp-model-tools.json");
    std::fs::write(
        &mcp_config,
        json!({"name":"fixture","command":env!("CARGO_BIN_EXE_ara-mcp-fixture"),
        "args":["--record",mcp_record]})
        .to_string(),
    )
    .unwrap();
    let mut command = env.selection_command(
        &first.base_url(),
        &[
            "--provider",
            "auto-fixture",
            "--tools",
            "read,write,edit,grep",
            "--mcp-config",
            mcp_config.to_str().unwrap(),
            "--mcp-allow",
            "fixture:echo",
        ],
    );
    command.env_remove("ARA_EDIT_VARIANT").env_remove("ARA_STRICT_EDIT_MODE");
    let mut child = RpcChild::spawn(command);
    child.ready();
    child.send(json!({"id":"held-definition","type":"set_host_tools","tools":[{
        "name":"held_tool","label":"Held tool","description":"retained Host adapter",
        "parameters":{"type":"object","properties":{}}
    }]}));
    child.success("held-definition");
    let original = child.state("original-tools");
    assert_model_tools_snapshot(&original, false);
    let initial_mcp: Value = serde_json::from_slice(&std::fs::read(&mcp_record).unwrap()).unwrap();
    model_tools_prompt(&mut child, "record-snapshot", None);
    child.send(json!({"id":"explicit-target","type":"set_model","provider":"switch-fixture","modelId":"target-model"}));
    child.success("explicit-target");
    assert_model_tools_snapshot(&child.state("replace-one-tools"), true);
    model_tools_prompt(&mut child, "plain-read-one", None);
    // Observe the first read before Native supersede pruning replaces it after
    // a later read of the same path. Keep the actual display/content gate.
    let first_plain_messages = child.messages("first-plain-read-messages");
    let first_plain = model_tools_receipt(&first_plain_messages, "target-read-one");
    assert_eq!(first_plain["isError"], false);
    assert!(contains_text(first_plain, "plain marker"), "{first_plain}");
    assert!(!first_plain["content"].to_string().contains("[format.txt#"), "{first_plain}");
    child.send(json!({"id":"cycle-original","type":"cycle_model"}));
    assert_eq!(child.success("cycle-original")["data"]["model"]["id"], "fake-model");
    assert_model_tools_snapshot(&child.state("hashline-two-tools"), false);
    model_tools_prompt(&mut child, "reuse-old-snapshot", None);
    let messages = child.messages("retained-snapshot-messages");
    assert_eq!(model_tools_receipt(&messages, "retained-snapshot-edit")["isError"], false);
    assert_eq!(std::fs::read_to_string(work.join("src/math.py")).unwrap(), "def add(a, b):\n    return a + b\n");
    child.send(json!({"id":"cycle-target-again","type":"cycle_model"}));
    assert_eq!(child.success("cycle-target-again")["data"]["model"]["id"], "target-model");
    let repeated = child.state("replace-two-tools");
    assert_eq!(repeated["sessionId"], original["sessionId"]);
    assert_eq!(repeated["sessionFile"], original["sessionFile"]);
    assert_model_tools_snapshot(&repeated, true);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&mcp_record).unwrap()).unwrap()["pid"],
        initial_mcp["pid"],
        "model changes keep the same MCP child"
    );
    model_tools_prompt(&mut child, "plain-read-and-adapters", Some("retained-host-call"));
    let messages = child.messages("plain-read-messages");
    {
        let id = "target-read-two";
        let receipt = model_tools_receipt(&messages, id);
        assert_eq!(receipt["isError"], false);
        assert!(contains_text(receipt, "plain marker"), "{receipt}");
        assert!(!receipt["content"].to_string().contains("[format.txt#"), "{receipt}");
    }
    let superseded = model_tools_receipt(&messages, "target-read-one");
    assert_eq!(superseded["isError"], false);
    assert!(contains_text(superseded, "[Superseded by a newer read of this file]"), "{superseded}");
    assert_eq!(model_tools_receipt(&messages, "retained-mcp-call")["isError"], false);
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
    child.send(json!({"id":"hashline-before-new","type":"set_model","provider":"auto-fixture","modelId":"fake-model"}));
    child.success("hashline-before-new");
    std::fs::write(work.join("src/math.py"), source).unwrap();
    // Existing ARA RPC reset creates a new store. The bare nested-file recovery
    // cannot succeed from a previous Session's snapshot after this boundary.
    child.adopt("fresh-store", json!({"id":"fresh-store","type":"new_session"}));
    let fresh = child.state("fresh-store-tools");
    assert_ne!(fresh["sessionId"], original["sessionId"]);
    assert_model_tools_snapshot(&fresh, false);
    model_tools_prompt(&mut child, "reject-old-snapshot", None);
    let messages = child.messages("fresh-snapshot-messages");
    assert_eq!(model_tools_receipt(&messages, "fresh-snapshot-edit")["isError"], true);
    assert_eq!(std::fs::read_to_string(work.join("src/math.py")).unwrap(), source);
    child.finish(0);
    let first_requests = first.requests.lock().await;
    let target_requests = target.requests.lock().await;
    assert_eq!(first_requests.len(), 6);
    assert_eq!(target_requests.len(), 6);
    for (request, replace) in [
        (&first_requests[0], false),
        (&target_requests[0], true),
        (&first_requests[2], false),
        (&target_requests[2], true),
    ] {
        let edit = request["body"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "edit")
            .unwrap();
        assert_eq!(edit["function"]["parameters"]["properties"].get("old_string").is_some(), replace, "{edit}");
    }
}

/// Extend the original production Responses/Native journal gate with real
/// streamed Edit effects. The original reader deadlines and assertions remain.
async fn rpc_native_edit_preview_guard_and_disabled_settings_group() {
    let env = Env::new();
    let agent_dir = env.home.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::write(agent_dir.join("config.yml"), "edit:\n  streamingAbort: true\n  fuzzyMatch: false\n").unwrap();
    let artifact = env.work.path().join("stream.txt");
    let generated_path = env.work.path().join("generated.ts");
    let generated = "// Code generated by sqlc. DO NOT EDIT.\nexport const value = 1;\n";
    std::fs::write(&artifact, "before\n").unwrap();
    std::fs::write(&generated_path, generated).unwrap();
    let streamed = json!({"path":"stream.txt","old_string":"before","new_string":"after"}).to_string();
    let provider_final =
        json!({"path":"stream.txt","old_string":"before","new_string":"provider-final-must-not-replace-stream"})
            .to_string();
    let invalid =
        json!({"path":"stream.txt","old_string":"completely absent target","new_string":"must not write"}).to_string();
    let edit_response = |call_id: &str, delta: &str, final_args: &str| {
        json!({"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":format!("fc_{call_id}"),"call_id":call_id,"name":"edit"}}},
            {"data":{"type":"response.function_call_arguments.delta","output_index":0,"delta":delta}},
            {"sleep_ms":1000},
            {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":final_args}},
            {"sleep_ms":1000},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":format!("fc_{call_id}"),"call_id":call_id,"name":"edit","arguments":final_args}}},
            {"sleep_ms":1000},
            {"data":{"type":"response.completed","response":{"id":format!("resp_{call_id}"),"status":"completed"}}}
        ]})
    };
    let answer = |id: &str, text: &str| {
        json!({"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":id,"content":[{"type":"output_text","text":text}]}}},
            {"data":{"type":"response.completed","response":{"id":format!("resp_{id}"),"status":"completed"}}}
        ]})
    };
    let up = upstream(json!({"responses":[
        edit_response("native-preview", &streamed, &provider_final),
        answer("edited", "Streamed session applied."),
        edit_response("generated-preview", "{\"path\":\"generated.ts\",", "{}"),
        answer("after-generated", "Original Session continues after generated guard."),
        edit_response("invalid-final-preview", &invalid, &invalid),
        answer("after-invalid", "Original Session continues after final preview guard.")
    ]}))
    .await;
    let mut command =
        env.command(&up.base_url(), &["--api", "openai-responses", "--tools", "edit", "--edit-mode", "replace"]);
    command.env("ARA_EDIT_FUZZY", "false").env_remove("ARA_EDIT_FUZZY_THRESHOLD");
    let mut child = RpcChild::spawn(command);
    child.ready();
    let initial = child.state("preview-session");
    child.send(
        json!({"id":"streamed-edit","type":"prompt","message":"edit through the original Native argument stream"}),
    );
    child.success("streamed-edit");
    let preview = child.until(
        |frame| {
            frame["type"] == "tool_stream_update"
                && frame["toolCallId"] == "native-preview|fc_native-preview"
                && frame["update"]["streaming"] == true
        },
        WAIT,
    );
    assert!(
        preview["update"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["diff"].as_str().is_some_and(|diff| diff.contains("after"))),
        "{preview}"
    );
    assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end" || frame["type"] == "tool_execution_start"));
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "before\n", "preview must not write");
    let active = child.state("preview-still-active");
    assert_eq!(active["isStreaming"], true);
    assert_eq!(active["sessionId"], initial["sessionId"]);
    let active_messages = child.messages("preview-no-receipt");
    assert!(!active_messages.iter().any(|message| message["role"] == "toolResult"));
    let final_preview = child.until(
        |frame| {
            frame["type"] == "tool_stream_update"
                && frame["toolCallId"] == "native-preview|fc_native-preview"
                && frame["update"]["streaming"] == false
        },
        WAIT,
    );
    assert!(
        final_preview["update"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|file| file["diff"].as_str().is_some_and(|diff| diff.contains("after")))
    );
    assert!(
        !final_preview["update"]["files"].as_array().unwrap().iter().any(|file| file["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("provider-final-must-not-replace-stream")))
    );
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "before\n");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "after\n", "execute must reuse the preview Session buffer");
    let messages = child.messages("preview-one-receipt");
    assert_eq!(messages.iter().filter(|message| message["role"] == "toolResult").count(), 1);
    assert_eq!(
        messages
            .iter()
            .find(|message| message["toolCallId"] == "native-preview|fc_native-preview"
                && message["role"] == "toolResult")
            .unwrap()["isError"],
        false
    );

    for (id, followup, expected_abort) in [
        ("generated-preview", "after-generated", "auto-generated file"),
        ("invalid-final-preview", "after-invalid", "stream.txt"),
    ] {
        let start = child.seen.len();
        child.send(json!({"id":id,"type":"prompt","message":"controlled rejected streamed edit"}));
        child.success(id);
        let ending = child.until(|frame| frame["type"] == "agent_end", WAIT);
        // Fixed OMP emits start/end for a synthetic aborted result even
        // though the real tool was never entered. Keep its pairing in history.
        let rejected: Vec<_> =
            child.seen[start..].iter().filter(|frame| frame["type"] == "tool_execution_end").collect();
        if id == "generated-preview" {
            // Fixed retainCompletedToolCalls removes the delta-only call.
            assert!(rejected.is_empty(), "{:?}", &child.seen[start..]);
            assert!(!child.seen[start..].iter().any(|frame| frame["assistantMessageEvent"]["type"] == "toolcall_end"));
            let assistant = ending["messages"]
                .as_array()
                .unwrap()
                .iter()
                .rev()
                .find(|message| message["role"] == "assistant")
                .unwrap();
            assert_eq!(assistant["stopReason"], "aborted");
            assert!(!assistant["content"].as_array().unwrap().iter().any(|block| block["type"] == "toolCall"));
        } else {
            assert_eq!(rejected.len(), 1, "{id}: {:?}", &child.seen[start..]);
            assert_eq!(rejected[0]["toolCallId"], format!("{id}|fc_{id}"));
            assert_eq!(rejected[0]["isError"], true);
            assert_eq!(rejected[0]["result"]["details"]["__synthetic"], true);
            assert_eq!(rejected[0]["result"]["details"]["executed"], false);
        }
        assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "after\n");
        assert_eq!(std::fs::read_to_string(&generated_path).unwrap(), generated);
        assert!(child.errors().contains(expected_abort), "{}", child.errors());
        if id == "invalid-final-preview" {
            assert!(child.seen[start..].iter().any(|frame| frame["type"] == "tool_stream_update"
                && frame["toolCallId"] == format!("{id}|fc_{id}")
                && frame["update"]["streaming"] == false
                && frame["update"]["files"].as_array().is_some_and(|files| {
                    files.iter().any(|file| file["error"].as_str().is_some_and(|error| !error.is_empty()))
                })));
        }
        let stopped = child.state(&format!("{id}-state"));
        assert_eq!(stopped["isStreaming"], false);
        assert_eq!(stopped["sessionId"], initial["sessionId"]);
        child
            .send(json!({"id":followup,"type":"prompt","message":"continue the original Session after cancelled Run"}));
        child.success(followup);
        child.until(|frame| frame["type"] == "agent_end", WAIT);
        assert_eq!(child.state(&format!("{followup}-state"))["sessionId"], initial["sessionId"]);
    }
    child.finish(0);
    let entries = journal(&env.sessions()[0]);
    let tool_results: Vec<_> = entries
        .iter()
        .filter(|entry| entry["message"]["role"] == "toolResult")
        .map(|entry| &entry["message"])
        .collect();
    assert_eq!(tool_results.len(), 2, "one real result plus one completed-call aborted placeholder");
    assert_eq!(tool_results.iter().filter(|result| result["isError"] == false).count(), 1);
    assert!(!tool_results.iter().any(|result| result["toolCallId"] == "generated-preview|fc_generated-preview"));
    for id in ["invalid-final-preview"] {
        let result = tool_results.iter().find(|result| result["toolCallId"] == format!("{id}|fc_{id}")).unwrap();
        assert_eq!(result["isError"], true);
        assert_eq!(result["details"]["__synthetic"], true);
        assert_eq!(result["details"]["executed"], false);
    }
    assert!(!entries.iter().any(|entry| entry.to_string().contains("tool_stream_update")));
    assert_eq!(up.served(), 6);

    // A disabled Edit constructor must not consume its malformed settings or
    // fuzzy environment. This exercises the real startup branch from PRE v2.
    let disabled = Env::new();
    let dir = disabled.home.path().join("agent");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.yml"), "edit:\n  fuzzyMatch: null\n  fuzzyThreshold: invalid\n  enforceSeenLines: invalid\n  blockAutoGenerated: invalid\n").unwrap();
    let unused = upstream(json!({"responses":[]})).await;
    let mut command = disabled.command(&unused.base_url(), &["--tools", "read"]);
    command.env("ARA_EDIT_FUZZY", "invalid").env("ARA_EDIT_FUZZY_THRESHOLD", "invalid");
    let mut child = RpcChild::spawn(command);
    child.ready();
    child.state("edit-disabled");
    child.finish(0);
    assert_eq!(unused.served(), 0);
}

#[path = "support/ast_edit_rpc.rs"]
mod ast_edit_rpc;

#[path = "support/edit_session_upgrade.rs"]
mod edit_session_upgrade;
