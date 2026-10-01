//! Real bounded RPC children for fixed OMP 596f2da Session retry.
//! Observe request counts, native receipt IDs and actual file effects;
//! provider retries are exhausted before claiming a Session retry.

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
    model: &'static str,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-retry-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-retry-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions, deadline: Instant::now() + CASE_BOUND, model: "fake-model" }
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
            .args(["--mode", "rpc", "--model", self.model, "--base-url", base_url, "--cwd"])
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
}

// Share the existing bounded child/socket/journal harness with this recovery
// module. No second harness or separate test for every ordinary mapping.
#[path = "support/rpc_loop_guard.rs"]
mod loop_guard;
#[path = "support/rpc_tool_loop_guard.rs"]
mod tool_loop_guard;

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
                let frame = line.map_err(|error| error.to_string()).and_then(|line| parse_rpc_test_frame(&line));
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

// The production codec can retain a native UTF-16 summary that serde_json's
// UTF-8 value cannot represent. Keep its exact JSON for the guard refusal
// scenario while preserving the existing error behavior for other frames.
fn parse_rpc_test_frame(line: &str) -> Result<Value, String> {
    match serde_json::from_str(line) {
        Ok(frame) => Ok(frame),
        Err(error) => {
            let mut native = ara_rpc::WireValue::parse(line).map_err(|_| format!("{error}: {line}"))?;
            if native.get("type") != Some(&ara_rpc::WireValue::String("notice".into()))
                || native.get("level") != Some(&ara_rpc::WireValue::String("error".into()))
                || native.get("source") != Some(&ara_rpc::WireValue::String("loop-guard".into()))
            {
                return Err(format!("{error}: {line}"));
            }
            let details = native.get("details").cloned().ok_or_else(|| format!("{error}: {line}"))?;
            native.insert("details", ara_rpc::WireValue::Null);
            let mut frame: Value = serde_json::from_str(&native.stringify()).map_err(|_| format!("{error}: {line}"))?;
            frame["losslessDetailsJson"] = json!(details.stringify());
            Ok(frame)
        }
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

async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn answer(message: &str) -> Value {
    json!({"events":[text(message),finish("stop"),done()]})
}

fn streamed_error(message: &str) -> Value {
    json!({"events":[{"data":{"error":{"code":503,"message":message}}}]})
}

fn http_error(wait_ms: u64) -> Value {
    json!({"status":503,"headers":{"retry-after-ms":wait_ms.to_string()},
        "body":json!({"error":{"message":"service unavailable, retry fixture","type":"server_error"}}).to_string()})
}

fn tool(command: &str, id: &str) -> Value {
    json!({"events":[tool_call(0,id,"bash",&json!({"command":command}).to_string()),finish("tool_calls"),done()]})
}

fn config(env: &Env, enabled: bool, attempts: f64, base_ms: f64, max_ms: f64) {
    let agent = env.home.path().join("agent");
    std::fs::create_dir_all(&agent).unwrap();
    std::fs::write(agent.join("config.yml"), format!(
        "retry:\n  enabled: {enabled}\n  maxRetries: {attempts}\n  baseDelayMs: {base_ms}\n  maxDelayMs: {max_ms}\n  modelFallback: false\ncustom:\n  retained: retry-fixture\n"
    )).unwrap();
}

fn toggle(child: &mut RpcChild, id: &str, enabled: bool) {
    child.send(json!({"id":id,"type":"set_auto_retry","enabled":enabled}));
    child.success(id);
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn session_file(child: &mut RpcChild, id: &str) -> PathBuf {
    PathBuf::from(child.state(id)["sessionFile"].as_str().unwrap())
}

fn failed_entries(entries: &[Value]) -> Vec<&Value> {
    entries
        .iter()
        .filter(|entry| {
            entry["message"]["role"] == "assistant"
                && matches!(entry["message"]["stopReason"].as_str(), Some("error" | "aborted"))
        })
        .collect()
}

fn persistence_key(message: &Value) -> String {
    format!(
        "assistant:{}:{}:{}:{}:{}",
        message["timestamp"].as_i64().unwrap(),
        message["provider"].as_str().unwrap(),
        message["model"].as_str().unwrap(),
        message["responseId"].as_str().unwrap_or(""),
        message["stopReason"].as_str().unwrap()
    )
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

fn end_count(child: &RpcChild) -> usize {
    child.seen.iter().filter(|frame| frame["type"] == "auto_retry_end").count()
}

fn assert_no_retry(child: &RpcChild) {
    assert!(
        !child.seen.iter().any(|frame| matches!(frame["type"].as_str(), Some("auto_retry_start" | "auto_retry_end"))),
        "unexpected Session retry events: {:?}",
        child.seen
    );
}

fn assert_recovery_receipt(original: &Value, updated: &Value, end: &Value, status: &str) {
    let receipts = end["retryErrors"].as_array().unwrap();
    let receipt = receipts.iter().find(|receipt| receipt["entryId"] == original["id"]).unwrap();
    assert_eq!(receipt["persistenceKey"], persistence_key(&original["message"]));
    assert_eq!(receipt["retryRecovery"], updated["message"]["retryRecovery"]);
    assert_eq!(receipt["note"], receipt["retryRecovery"]["note"]);
    assert_eq!(receipt["retryRecovery"]["kind"], "auto-retry");
    assert_eq!(receipt["retryRecovery"]["status"], status);
    assert_eq!(receipt["retryRecovery"]["attempt"], 1);
    let mut expected = original.clone();
    expected["message"]["retryRecovery"] = receipt["retryRecovery"].clone();
    assert_eq!(updated, &expected, "metadata rewrite must retain the actual original journal receipt");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_retry_exhausts_http_and_outer_budgets_then_records_one_actual_effect_and_reopens() {
    let env = Env::new();
    config(&env, true, 2.0, 100.0, 300_000.0);
    let nonce = env.work.path().file_name().unwrap().to_str().unwrap();
    let effect = format!("printf '{nonce}\\n' >> retry-effect.txt");
    // Six HTTP attempts in each of two outer provider attempts precede the
    // first Session start event. Explicit zero hints avoid exponential sleeps.
    let mut responses = vec![http_error(0); 12];
    responses.extend([tool(&effect, "recovered-effect"), answer("Recovered with one actual effect")]);
    let up = upstream(responses).await;
    let mut gate = HttpGate::start(&up, 12).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &["--tools", "bash"]);
    child.ready();
    assert_eq!(child.state("initial-retry-state")["autoRetryEnabled"], true);
    child.prompt("recover", "recover the transient provider failure and append the nonce exactly once");
    let start = child.until(|frame| frame["type"] == "auto_retry_start");
    assert_eq!(start["attempt"], 1);
    assert_eq!(start["maxAttempts"].as_f64(), Some(2.0));
    assert!(start["errorId"].as_u64().is_some());
    gate.reached(env.deadline).await;
    assert_eq!(up.served(), 12);
    let file = session_file(&mut child, "original-error-file");
    let before = journal(&file);
    let original = failed_entries(&before)[0].clone();
    assert!(original["message"].get("retryRecovery").is_none());
    gate.release();
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(end["success"], true, "{end}");
    assert_eq!(end["attempt"], 1);
    let final_state = child.state("after-recovery");
    assert_eq!(final_state["isRetrying"], false);
    assert_eq!(final_state["isStreaming"], false);
    let after = journal(&file);
    let updated = after.iter().find(|entry| entry["id"] == original["id"]).unwrap();
    assert_recovery_receipt(&original, updated, &end, "recovered");
    assert!(updated["message"]["retryRecovery"]["recoveredAt"].as_str().is_some());
    assert_eq!(std::fs::read_to_string(env.work.path().join("retry-effect.txt")).unwrap(), format!("{nonce}\n"));
    assert_eq!(after.iter().filter(|entry| entry["message"]["toolCallId"] == "recovered-effect").count(), 1);
    assert!(
        child
            .messages("successful-native")
            .iter()
            .any(|message| message_text(message) == "Recovered with one actual effect")
    );
    child.finish();
    assert_eq!(end_count(&child), 1);
    assert_eq!(up.served(), 14);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 14);
    let original_body = requests[0]["body"].clone();
    for request in requests.iter().take(13) {
        assert_eq!(request["body"], original_body, "same Session retry must not inject a second user prompt");
    }
    drop(requests);
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap(), "--tools", "bash"]);
    reopened.ready();
    assert_eq!(reopened.state("reopen-retry-state")["isRetrying"], false);
    assert!(
        reopened
            .messages("reopen-native")
            .iter()
            .any(|message| message_text(message) == "Recovered with one actual effect")
    );
    reopened.finish();
    assert_eq!(journal(&file), after);
    assert_eq!(up.served(), 14, "opening a recovered Session never starts another billed turn");
    assert_eq!(std::fs::read_to_string(env.work.path().join("retry-effect.txt")).unwrap(), format!("{nonce}\n"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_abort_retry_is_reachable_during_wait_and_emits_one_end_without_next_request() {
    let env = Env::new();
    config(&env, true, 2.0, 5_000.0, 300_000.0);
    let up = upstream(vec![
        streamed_error("service unavailable"),
        streamed_error("service unavailable"),
        answer("must not run"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.prompt("cancel-wait", "observe a cancellable Session retry wait");
    let start = child.until(|frame| frame["type"] == "auto_retry_start");
    assert!((3_750.0..=5_000.0).contains(&start["delayMs"].as_f64().unwrap()));
    assert_eq!(up.served(), 2, "only the provider wrapper has used its one retry so far");
    assert_eq!(child.state("waiting-state")["isRetrying"], true);
    let requested = Instant::now();
    child.send(json!({"id":"abort-wait","type":"abort_retry"}));
    child.success("abort-wait");
    assert!(requested.elapsed() < Duration::from_secs(2), "ordinary command owner was blocked by backoff");
    let end = if let Some(end) = child.seen.iter().find(|frame| frame["type"] == "auto_retry_end") {
        end.clone()
    } else {
        child.until(|frame| frame["type"] == "auto_retry_end")
    };
    assert_eq!(end["success"], false);
    assert!(end["finalError"].as_str().unwrap().to_ascii_lowercase().contains("cancel"));
    assert_eq!(child.state("after-abort-retry")["isRetrying"], false);
    child.send(json!({"id":"abort-again","type":"abort_retry"}));
    child.success("abort-again");
    child.finish();
    assert_eq!(end_count(&child), 1);
    assert_eq!(up.served(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_disabled_retry_persists_across_new_switch_restart_and_keeps_provider_budget_distinct() {
    let env = Env::new();
    config(&env, true, 2.0, 10.0, 300_000.0);
    let up = upstream(vec![
        streamed_error("service unavailable"),
        streamed_error("service unavailable"),
        answer("fresh seed"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    toggle(&mut child, "disable-retry", false);
    child.send(json!({"id":"invalid-toggle","type":"set_auto_retry","enabled":"false"}));
    assert_eq!(child.response("invalid-toggle")["success"], false);
    assert_eq!(child.state("disabled-state")["autoRetryEnabled"], false);
    child.run("disabled-error", "provider wrapper still owns its separate replay-safe budget");
    let original = session_file(&mut child, "disabled-original");
    assert_eq!(up.served(), 2);
    assert_eq!(failed_entries(&journal(&original)).len(), 1);
    child.send(json!({"id":"new-disabled-session","type":"new_session"}));
    child.success("new-disabled-session");
    assert_eq!(child.state("new-policy")["autoRetryEnabled"], false);
    child.run("fresh-disabled-seed", "materialize the new Session");
    child.send(json!({"id":"switch-disabled-origin","type":"switch_session","sessionPath":original}));
    child.success("switch-disabled-origin");
    assert_eq!(child.state("switched-policy")["autoRetryEnabled"], false);
    child.finish();
    assert_no_retry(&child);
    let saved = std::fs::read_to_string(env.home.path().join("agent/config.yml")).unwrap();
    assert!(saved.contains("enabled: false") && saved.contains("retry-fixture"));
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", original.to_str().unwrap()]);
    reopened.ready();
    assert_eq!(reopened.state("restarted-policy")["autoRetryEnabled"], false);
    assert_eq!(reopened.state("restarted-idle")["isRetrying"], false);
    reopened.finish();
    assert_no_retry(&reopened);
    assert_eq!(up.served(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_retry_exhaustion_preserves_terminal_error_and_supersedes_only_prior_failed_receipt() {
    let env = Env::new();
    config(&env, true, 1.0, 0.0, 300_000.0);
    let up = upstream(vec![
        streamed_error("service unavailable first"),
        streamed_error("service unavailable first"),
        streamed_error("service unavailable terminal"),
        streamed_error("service unavailable terminal"),
        answer("must not run"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.prompt("exhaust", "exhaust the one Session retry attempt");
    let start = child.until(|frame| frame["type"] == "auto_retry_start");
    assert_eq!(start["attempt"], 1);
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(end["success"], false);
    // Fixed turn-recovery.ts:2310-2325 reports retryAttempt - 1 here.
    assert_eq!(end["attempt"], 1);
    assert!(end["finalError"].as_str().unwrap().contains("service unavailable terminal"));
    assert_eq!(child.state("exhausted-state")["isRetrying"], false);
    let entries = journal(&session_file(&mut child, "exhausted-file"));
    let failed = failed_entries(&entries);
    assert_eq!(failed.len(), 2);
    assert_eq!(failed[0]["message"]["retryRecovery"]["status"], "superseded");
    assert!(failed[1]["message"].get("retryRecovery").is_none());
    assert_eq!(end["retryErrors"].as_array().unwrap().len(), 1);
    assert_eq!(end["retryErrors"][0]["entryId"], failed[0]["id"]);
    assert_eq!(end["retryErrors"][0]["persistenceKey"], persistence_key(&failed[0]["message"]));
    child.finish();
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "auto_retry_start").count(), 1);
    assert_eq!(end_count(&child), 1);
    assert_eq!(up.served(), 4);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_http_header_wait_ceiling_fails_fast_and_zero_ceiling_honors_stream_hint() {
    for zero_ceiling in [false, true] {
        let env = Env::new();
        config(&env, true, 1.0, if zero_ceiling { 1_000.0 } else { 0.0 }, if zero_ceiling { 0.0 } else { 100.0 });
        let responses = if zero_ceiling {
            vec![
                streamed_error("service unavailable; retry-after-ms: 150"),
                streamed_error("service unavailable; retry-after-ms: 150"),
                answer("wait hint honored"),
            ]
        } else {
            vec![http_error(150); 12]
        };
        let up = upstream(responses).await;
        let mut child = RpcChild::spawn(&env, &up, &[]);
        child.ready();
        child.prompt("hinted-retry", "exercise the actual provider wait hint and configured ceiling");
        if zero_ceiling {
            let start = child.until(|frame| frame["type"] == "auto_retry_start");
            assert!((750.0..=1_000.0).contains(&start["delayMs"].as_f64().unwrap()), "{start}");
            let end = child.until(|frame| frame["type"] == "auto_retry_end");
            assert_eq!(end["success"], true, "{end}");
            assert_eq!(up.served(), 3);
        } else {
            let end = child.until(|frame| frame["type"] == "auto_retry_end");
            assert_eq!(end["success"], false, "{end}");
            let error = end["finalError"].as_str().unwrap();
            assert!(error.contains("150") && error.contains("maxDelayMs") && error.contains("100"), "{error}");
            assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
            assert_eq!(
                up.served(),
                12,
                "HTTP and outer budgets exhausted; Session does not perform a thirteenth request"
            );
            let entries = journal(&session_file(&mut child, "ceiling-file"));
            assert_eq!(failed_entries(&entries).len(), 1);
            assert!(failed_entries(&entries)[0]["message"].get("retryRecovery").is_none());
        }
        assert_eq!(child.state("hint-idle")["isRetrying"], false);
        child.finish();
        assert_eq!(end_count(&child), 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_committed_visible_text_vetoes_provider_and_session_replay() {
    let env = Env::new();
    config(&env, true, 2.0, 0.0, 300_000.0);
    let up = upstream(vec![
        json!({"events":[text("Already committed visible answer"),
        {"data":{"error":{"code":503,"message":"service unavailable after output"}}}]}),
        answer("must not replay"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.run("unsafe-output", "do not replay an answer that was already committed");
    assert_eq!(child.state("unsafe-idle")["isRetrying"], false);
    let entries = journal(&session_file(&mut child, "unsafe-file"));
    let failed = failed_entries(&entries);
    assert_eq!(failed.len(), 1);
    assert_eq!(message_text(&failed[0]["message"]), "Already committed visible answer");
    assert!(failed[0]["message"].get("retryRecovery").is_none());
    child.finish();
    assert_no_retry(&child);
    assert_eq!(up.served(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_synthetic_false_tool_pair_is_retained_and_retry_does_not_duplicate_the_effect() {
    let env = Env::new();
    config(&env, true, 1.0, 100.0, 300_000.0);
    let effect = "printf 'once\\n' >> synthetic-retry-effect.txt";
    let failed = json!({"events":[tool_call(0,"never-executed","bash",&json!({"command":effect}).to_string()),
        {"data":{"error":{"code":503,"message":"service unavailable after unexecuted tool emission"}}}]});
    let up = upstream(vec![failed, tool(effect, "actual-effect"), answer("unexecuted pair recovered")]).await;
    let mut gate = HttpGate::start(&up, 1).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &["--tools", "bash"]);
    child.ready();
    child.prompt("paired-retry", "recover only with proof that the failed emitted call was not executed");
    child.until(|frame| frame["type"] == "auto_retry_start");
    gate.reached(env.deadline).await;
    assert_eq!(up.served(), 1, "emitted calls forbid the provider wrapper from replaying this response");
    let file = session_file(&mut child, "paired-file");
    let before = journal(&file);
    let original = failed_entries(&before)[0].clone();
    let synthetic = before.iter().find(|entry| entry["message"]["toolCallId"] == "never-executed").unwrap().clone();
    assert_eq!(synthetic["message"]["details"]["__synthetic"], true);
    assert_eq!(synthetic["message"]["details"]["executed"], false);
    let native = child.messages("paired-active");
    assert!(
        native
            .iter()
            .any(|message| message["role"] == "assistant" && message["content"].to_string().contains("never-executed"))
    );
    assert!(
        native
            .iter()
            .any(|message| message["toolCallId"] == "never-executed" && message["details"]["executed"] == false)
    );
    gate.release();
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(end["success"], true, "{end}");
    let after = journal(&file);
    assert_recovery_receipt(
        &original,
        after.iter().find(|entry| entry["id"] == original["id"]).unwrap(),
        &end,
        "recovered",
    );
    assert_eq!(after.iter().find(|entry| entry["id"] == synthetic["id"]).unwrap(), &synthetic);
    assert_eq!(after.iter().filter(|entry| entry["message"]["toolCallId"] == "actual-effect").count(), 1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("synthetic-retry-effect.txt")).unwrap(), "once\n");
    child.finish();
    assert_eq!(end_count(&child), 1);
    assert_eq!(up.served(), 3);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_reopen_interrupted_journal_preserves_unknown_effect_and_never_automatically_replays() {
    let env = Env::new();
    config(&env, true, 1.0, 0.0, 300_000.0);
    let effect = "printf 'once\\n' >> unknown-effect.txt";
    let up = upstream(vec![
        tool(effect, "lost-receipt-effect"),
        answer("effect recorded before controlled journal interruption"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "bash"]);
    child.ready();
    child.run("unknown-effect-seed", "perform the actual effect before the incomplete-journal fixture");
    let file = session_file(&mut child, "unknown-file");
    child.finish();
    assert_eq!(std::fs::read_to_string(env.work.path().join("unknown-effect.txt")).unwrap(), "once\n");
    let entries = journal(&file);
    let emitted = entries
        .iter()
        .position(|entry| {
            entry["message"]["role"] == "assistant"
                && entry["message"]["content"].to_string().contains("lost-receipt-effect")
        })
        .unwrap();
    let emitted_id = entries[emitted]["id"].clone();
    // Controlled native interruption fixture, not a claim of a live crash:
    // the effect is real, but its result and final answer are absent on disk.
    // Only this test's private journal is rewritten.
    let incomplete = entries[..=emitted].iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(&file, incomplete).unwrap();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap(), "--tools", "bash"]);
    reopened.ready();
    assert_eq!(reopened.state("unknown-reopened-state")["isRetrying"], false);
    let messages = reopened.messages("unknown-reopened-native");
    let unknown = messages.iter().find(|message| message["toolCallId"] == "lost-receipt-effect").unwrap();
    assert_eq!(unknown["details"]["executed"], "unknown");
    assert_eq!(unknown["details"]["__synthetic"], true);
    assert!(message_text(unknown).to_ascii_lowercase().contains("unknown"));
    reopened.finish();
    assert_no_retry(&reopened);
    let after = journal(&file);
    assert!(after.iter().any(|entry| entry["id"] == emitted_id));
    assert_eq!(after.iter().filter(|entry| entry["message"]["toolCallId"] == "lost-receipt-effect").count(), 1);
    assert_eq!(up.served(), 2, "native unknown recovery is not permission to replay or silently start another Run");
    assert_eq!(std::fs::read_to_string(env.work.path().join("unknown-effect.txt")).unwrap(), "once\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_long_http_hint_blocks_inner_replay_but_session_ceiling_and_cancellation_remain_reachable() {
    for zero_ceiling in [false, true] {
        let env = Env::new();
        config(&env, true, 2.0, 500.0, if zero_ceiling { 0.0 } else { 300_000.0 });
        let up = upstream(vec![http_error(600_000), answer("must not run")]).await;
        let mut child = RpcChild::spawn(&env, &up, &[]);
        child.ready();
        child.prompt("long-hint", "the Session owns its wait ceiling after the provider refuses a long inner wait");
        let end = if zero_ceiling {
            let start = child.until(|frame| frame["type"] == "auto_retry_start");
            assert_eq!(start["delayMs"].as_f64(), Some(600_000.0));
            assert_eq!(up.served(), 1, "long header stops both HTTP inner and provider outer replay");
            assert_eq!(child.state("long-wait-state")["isRetrying"], true);
            let requested = Instant::now();
            child.send(json!({"id":"abort-long-hint","type":"abort_retry"}));
            child.success("abort-long-hint");
            assert!(requested.elapsed() < Duration::from_secs(2));
            child
                .seen
                .iter()
                .find(|frame| frame["type"] == "auto_retry_end")
                .cloned()
                .unwrap_or_else(|| child.until(|frame| frame["type"] == "auto_retry_end"))
        } else {
            let end = child.until(|frame| frame["type"] == "auto_retry_end");
            let error = end["finalError"].as_str().unwrap();
            assert!(error.contains("600000") && error.contains("300000") && error.contains("maxDelayMs"), "{error}");
            assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
            end
        };
        assert_eq!(end["success"], false);
        assert_eq!(end["attempt"], 1);
        assert_eq!(child.state("long-hint-idle")["isRetrying"], false);
        let entries = journal(&session_file(&mut child, "long-hint-file"));
        assert_eq!(failed_entries(&entries).len(), 1);
        assert!(failed_entries(&entries)[0]["message"].get("retryRecovery").is_none());
        child.finish();
        assert_eq!(end_count(&child), 1);
        assert_eq!(up.served(), 1, "abort/fail-fast must not send another request or actually wait ten minutes");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_retry_backoff_accepts_independent_bash_receipt_and_continues_the_same_session() {
    let env = Env::new();
    config(&env, true, 1.0, 3_000.0, 300_000.0);
    let up = upstream(vec![
        streamed_error("service unavailable before user Bash"),
        streamed_error("service unavailable before user Bash"),
        answer("Independent Bash context consumed"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.prompt(
        "bash-during-retry",
        "continue after the transient failure and inspect the authorized user Bash receipt",
    );
    let start = child.until(|frame| frame["type"] == "auto_retry_start");
    assert!((2_250.0..=3_000.0).contains(&start["delayMs"].as_f64().unwrap()));
    assert_eq!(up.served(), 2);
    let origin = child.state("before-independent-bash");
    assert_eq!(origin["isRetrying"], true);
    assert_eq!(origin["isStreaming"], false);
    let file = PathBuf::from(origin["sessionFile"].as_str().unwrap());
    let nonce = env.work.path().file_name().unwrap().to_str().unwrap();
    let command = format!("printf '{nonce}\\n' >> independent-retry-effect.txt; printf 'authorized:{nonce}\\n'");
    child.send(json!({"id":"independent-retry-bash","type":"bash","command":command}));
    let result = child.success("independent-retry-bash");
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["exitCode"], 0);
    assert_eq!(result["output"], format!("authorized:{nonce}\n"));
    assert_eq!(
        std::fs::read_to_string(env.work.path().join("independent-retry-effect.txt")).unwrap(),
        format!("{nonce}\n")
    );
    let waiting = child.state("after-independent-bash");
    assert_eq!(waiting["sessionId"], origin["sessionId"]);
    assert_eq!(waiting["sessionFile"], origin["sessionFile"]);
    assert_eq!(waiting["isRetrying"], true);
    assert_eq!(
        waiting["isStreaming"], false,
        "the Bash receipt must settle during backoff, before continuation starts"
    );
    let before_continue = journal(&file);
    let bash_entries = before_continue
        .iter()
        .filter(|entry| entry["message"]["role"] == "bashExecution" && entry["message"]["command"] == command)
        .collect::<Vec<_>>();
    assert_eq!(bash_entries.len(), 1);
    let bash_entry = bash_entries[0].clone();
    assert_eq!(bash_entry["message"]["output"], result["output"]);
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(
        end["success"], true,
        "known authorized same-Session Bash append must not be rejected as arbitrary context drift: {end}"
    );
    let completed = child.state("after-bash-continuation");
    assert_eq!(completed["sessionId"], origin["sessionId"]);
    assert_eq!(completed["isRetrying"], false);
    let after = journal(&file);
    assert_eq!(after.iter().find(|entry| entry["id"] == bash_entry["id"]).unwrap(), &bash_entry);
    assert_eq!(
        after
            .iter()
            .filter(|entry| entry["message"]["role"] == "bashExecution" && entry["message"]["command"] == command)
            .count(),
        1
    );
    child.finish();
    assert_eq!(end_count(&child), 1);
    assert_eq!(up.served(), 3);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(!requests[0]["body"]["messages"].to_string().contains(&format!("authorized:{nonce}")));
    let projected = requests[2]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "user" && message_text(message).contains(&format!("Ran `{command}`")))
        .expect("actual continuation HTTP request must include the completed authorized Bash model context");
    assert!(message_text(projected).contains(&format!("authorized:{nonce}")));
    assert!(!requests[2]["body"]["messages"].to_string().contains("bashExecution"));
    assert_eq!(
        std::fs::read_to_string(env.work.path().join("independent-retry-effect.txt")).unwrap(),
        format!("{nonce}\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_zero_retry_budget_settles_once_without_start_or_rewriting_the_native_error() {
    let env = Env::new();
    config(&env, true, 0.0, 500.0, 300_000.0);
    let up = upstream(vec![
        streamed_error("service unavailable with zero Session budget"),
        streamed_error("service unavailable with zero Session budget"),
        answer("must not retry"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &[]);
    child.ready();
    child.prompt("zero-budget", "settle the failed turn after the separate provider retry budget is exhausted");
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(end["success"], false);
    assert_eq!(end["attempt"], 0);
    assert_eq!(end["retryErrors"], json!([]));
    assert!(end["finalError"].as_str().unwrap().contains("zero Session budget"));
    assert!(!child.seen.iter().any(|frame| frame["type"] == "auto_retry_start"));
    let state = child.state("zero-budget-idle");
    assert_eq!(state["isRetrying"], false);
    assert_eq!(state["isStreaming"], false);
    let file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    let entries = journal(&file);
    let failed = failed_entries(&entries);
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["message"]["errorMessage"].as_str().unwrap().contains("zero Session budget"));
    assert!(failed[0]["message"].get("retryRecovery").is_none());
    child.finish();
    assert_eq!(end_count(&child), 1);
    assert_eq!(
        up.served(),
        2,
        "only the existing provider wrapper gets its one replay; Session budget zero sends no third request"
    );
    assert_eq!(journal(&file), entries);
}
