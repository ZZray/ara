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

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-rpc-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-rpc-work-").tempdir().unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions }
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
            .env("ARA_API_KEY", "sk-rpc-fixture-only")
            .args(["--mode", "rpc", "--model", "fake-model", "--base-url", base_url, "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(["--compact-threshold", "0"])
            .args(args);
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

    fn ready(&mut self) {
        let ready = self.next(WAIT);
        assert_eq!(ready["type"], "ready", "{ready}");
        assert_eq!(ready["protocolVersion"], 1);
        assert_eq!(ready["supportedProtocolVersions"], json!([1, 2]));
        assert_eq!(ready["maxFrameBytes"], 1024 * 1024);
        assert_eq!(ready["maxReassembledFrameBytes"], 64 * 1024 * 1024);
        let metadata = self.next(WAIT);
        assert_eq!(metadata["type"], "available_commands_update");
        assert_eq!(metadata["commands"], json!([]));
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

#[tokio::test(flavor = "multi_thread")]
async fn rpc_protocol_recoverable_input_errors_and_explicit_unsupported_commands() {
    let env = Env::new();
    let skill = env.work.path().join(".ara/skills/proof/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(skill, "---\nname: proof\ndescription: proof\n---\nDo the task.\n").unwrap();
    let up = upstream(json!({"responses":[]})).await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send_raw("{broken\n");
    let malformed = child.until(|frame| frame["type"] == "response", WAIT);
    assert_eq!(malformed["success"], false);
    assert!(malformed["error"].as_str().unwrap().contains("parse"));
    child.send(json!({"id":"bad-version","type":"negotiate_protocol","protocolVersion":3}));
    assert_eq!(child.response("bad-version")["success"], false);
    child.send(json!({"id":"v2-中文-🦀","type":"negotiate_protocol","protocolVersion":2}));
    assert_eq!(child.success("v2-中文-🦀")["data"]["protocolVersion"], 2);
    for command in ["new_session", "switch_session", "bash", "compact", "set_model", "does_not_exist"] {
        child.send(json!({"id":command,"type":command}));
        let response = child.response(command);
        assert_eq!(response["command"], command);
        assert_eq!(response["success"], false, "{response}");
        assert!(!response["error"].as_str().unwrap().is_empty());
    }
    child.send(json!({"id":"bad-mode","type":"set_follow_up_mode","mode":"bogus"}));
    assert_eq!(child.response("bad-mode")["success"], false);
    child.send(json!({"id":"skill","type":"prompt","message":"/skill:proof task"}));
    assert_eq!(child.response("skill")["success"], false);
    child.send(json!({"type":"get_state"}));
    let state = child.until(|frame| frame["type"] == "response" && frame["command"] == "get_state", WAIT);
    assert_eq!(state["success"], true);
    assert!(state.get("id").is_none(), "omitted id stays omitted: {state}");
    assert_eq!(state["data"]["messageCount"], 0);
    child.finish(0);
    assert_eq!(up.served(), 0, "protocol stdin and unsupported Skill never reach inference");
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_live_partial_queries_precede_completion_and_abort_joins_the_run() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[text("Held partial." )],"end":"hang"},
        {"events":[text("After abort."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
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
    let up = upstream(json!({"responses":[
        {"status":400,"body":"{\"error\":{\"message\":\"RPC controlled failure\"}}"},
        {"events":[text("Recovered next prompt."),finish("stop"),done()]}
    ]}))
    .await;
    let mut child = RpcChild::spawn(env.command(&up.base_url(), &[]));
    child.ready();
    child.send(json!({"id":"failure","type":"prompt","message":"trigger failure"}));
    child.success("failure");
    let failure = child.until(|frame| frame["id"] == "failure" && frame["success"] == false, WAIT);
    assert_eq!(failure["command"], "prompt");
    assert!(failure["error"].as_str().unwrap().contains("RPC controlled failure"), "{failure}");
    child.send(json!({"id":"recovery","type":"prompt","message":"next task"}));
    child.success("recovery");
    child.until(|frame| frame["type"] == "agent_end", WAIT);
    child.finish(0);
    assert_eq!(up.served(), 2);
    let entries = journal(&env.sessions()[0]);
    assert!(entries.iter().any(|entry| entry["message"]["stopReason"] == "error"));
    assert!(entries.iter().any(|entry| contains_text(&entry["message"], "Recovered next prompt.")));
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_abort_and_prompt_executes_while_stdout_is_backpressured() {
    let env = Env::new();
    let mut held: Vec<Value> = (0..40).map(|_| text(&"x".repeat(4096))).collect();
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
