//! Actual RPC process -> shared Broker owner -> live and shutdown observed
//! usage. Synthetic model/account data do not establish real account parity.

#[path = "support/broker_consumers.rs"]
mod broker_consumers;

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use broker_consumers::{BrokerFixture, Request, snapshot};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::time::{Instant, timeout};

const WAIT: Duration = Duration::from_secs(15);
const KEY: &str = "synthetic-rpc-broker-api-key-only";

struct Host {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Host {
    fn new(model_url: &str) -> Self {
        let home = tempfile::Builder::new().prefix("ara-broker-rpc-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-broker-rpc-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        let config = home.path().join("agent/models.yml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        // The explicit CLI model uses provider transport overrides and the
        // existing shared Broker owner supplies its request credentials.
        std::fs::write(
            &config,
            json!({"providers":{"custom":{
                "api":"openai-completions","baseUrl":model_url,"models":[]
            }}})
            .to_string(),
        )
        .unwrap();
        Self { home, work, sessions }
    }

    fn command(&self, broker_url: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", self.home.path())
            .env("USERPROFILE", self.home.path())
            .env("ARA_HOME", self.home.path())
            .env("TEMP", self.home.path())
            .env("TMP", self.home.path())
            .env("ARA_AUTH_BROKER_URL", broker_url)
            .env("ARA_AUTH_BROKER_TOKEN", "synthetic-broker-token")
            .current_dir(self.work.path())
            .args([
                "--mode",
                "rpc",
                "--provider",
                "custom",
                "--model",
                "broker-model",
                "--no-skills",
                "--tools",
                "write",
                "--max-model-calls",
                "4",
                "--compact-threshold",
                "0",
                "--session-dir",
            ])
            .arg(&self.sessions);
        command
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    seen: Vec<Value>,
    stderr: Arc<Mutex<String>>,
    error_reader: tokio::task::JoinHandle<()>,
}

impl RpcChild {
    fn spawn(command: Command) -> Self {
        let mut command = tokio::process::Command::from(command);
        let mut child = command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut pipe = child.stderr.take().unwrap();
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        let error_reader = tokio::spawn(async move {
            let mut bytes = [0; 4096];
            while let Ok(count) = pipe.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
                captured.lock().unwrap().push_str(&String::from_utf8_lossy(&bytes[..count]));
            }
        });
        Self { child, stdin, lines, seen: Vec::new(), stderr, error_reader }
    }

    async fn send(&mut self, frame: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{frame}\n").as_bytes()).await.unwrap();
        stdin.flush().await.unwrap();
    }

    async fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let line = timeout(deadline.saturating_duration_since(Instant::now()), self.lines.next_line())
                .await
                .expect("RPC frame deadline")
                .unwrap()
                .unwrap_or_else(|| panic!("RPC stdout ended; stderr: {}", self.stderr.lock().unwrap()));
            let frame: Value = serde_json::from_str(&line).unwrap();
            self.seen.push(frame.clone());
            if matches(&frame) {
                return frame;
            }
        }
    }

    async fn success(&mut self, id: &str) -> Value {
        let response = self.until(|frame| frame["type"] == "response" && frame["id"] == id).await;
        assert_eq!(response["success"], true, "{response}; stderr: {}", self.stderr.lock().unwrap());
        response
    }

    async fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"})).await;
        self.success(id).await["data"].clone()
    }

    async fn finish(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + WAIT;
        while let Some(line) = timeout(deadline.saturating_duration_since(Instant::now()), self.lines.next_line())
            .await
            .expect("RPC EOF drain deadline")
            .unwrap()
        {
            self.seen.push(serde_json::from_str(&line).unwrap());
        }
        let status = timeout(deadline.saturating_duration_since(Instant::now()), self.child.wait())
            .await
            .expect("RPC process exit deadline")
            .unwrap();
        (&mut self.error_reader).await.unwrap();
        assert_eq!(status.code(), Some(0), "{}", self.stderr.lock().unwrap());
        assert_eq!(self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.start_kill();
        self.error_reader.abort();
    }
}

fn usage(input: u64, output: u64, read: u64, write: u64, cost: f64) -> Value {
    json!({"data":{"choices":[],"usage":{"prompt_tokens":input + read + write,
        "completion_tokens":output,"prompt_tokens_details":{"cached_tokens":read,"cache_write_tokens":write},
        "cost":cost}}})
}

fn contains_text(message: &Value, expected: &str) -> bool {
    message["content"].as_array().is_some_and(|blocks| {
        blocks.iter().any(|block| block["text"].as_str().is_some_and(|text| text.contains(expected)))
    })
}

fn reports(broker: &BrokerFixture) -> Vec<Request> {
    broker
        .requests
        .lock()
        .unwrap()
        .iter()
        .filter(|request| request.method == "POST" && request.path == "/v1/usage/observed")
        .cloned()
        .collect()
}

async fn wait_for_live_report(broker: &BrokerFixture) {
    let deadline = Instant::now() + WAIT;
    while reports(broker).is_empty() {
        assert!(Instant::now() < deadline, "no live observed POST before bounded deadline");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rpc_shared_owner_observed_usage_survives_continuation_unknown_cancellation_and_observer_fault() {
    for observed_status in [200, 500] {
        let model = FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":[
            {"events":[tool_call(0,"live-usage","write","{\"path\":\"live.txt\",\"content\":\"known request effect\\n\"}"),
                finish("tool_calls"), usage(8,5,3,2,0.25), done()]},
            {"events":[text("Held after the completed known request")],"end":"hang"},
            {"events":[text("Same Session continuation complete"),finish("stop"),usage(2,1,0,0,0.0),done()]},
            {"events":[text("Unknown usage remains unknown"),finish("stop"),
                {"data":{"choices":[],"usage":{"prompt_tokens":13,"completion_tokens":5}}},done()]}
        ]})).unwrap(), None).await.unwrap();
        let host = Host::new(&model.base_url());
        let initial = snapshot(
            3,
            vec![json!({"id":41,"provider":"custom",
            "credential":{"type":"api_key","key":KEY},"identityKey":null,"rotatesInMs":null,"blocks":[]})],
        );
        let broker = BrokerFixture::start(move |request| {
            if request.method == "GET" && request.path.starts_with("/v1/snapshot") {
                (200, initial.clone())
            } else if request.method == "POST" && request.path == "/v1/usage/observed" {
                (observed_status, json!({"ok":observed_status == 200}))
            } else {
                (404, json!({"error":"unexpected Broker fixture endpoint"}))
            }
        })
        .await;
        let mut child = RpcChild::spawn(host.command(&broker.url));
        child.until(|frame| frame["type"] == "ready").await;
        child.until(|frame| frame["type"] == "available_commands_update").await;
        let session = child.state("initial-state").await["sessionId"].clone();

        child.send(json!({"id":"live","type":"prompt","message":"write live.txt, then hold"})).await;
        child.success("live").await;
        let known = child
            .until(|frame| {
                frame["type"] == "message_end"
                    && frame["message"]["role"] == "assistant"
                    && frame["message"]["stopReason"] == "toolUse"
            })
            .await;
        assert_eq!(known["message"]["usage"]["input"], 8);
        child.until(|frame| frame["type"] == "message_update" && contains_text(&frame["message"], "Held after")).await;
        assert_eq!(std::fs::read_to_string(host.work.path().join("live.txt")).unwrap(), "known request effect\n");
        wait_for_live_report(&broker).await;
        let live = child.state("live-post-state").await;
        assert_eq!(live["isStreaming"], true, "the first POST is from an active Run, before EOF");
        assert_eq!(live["sessionId"], session);
        assert!(!child.seen.iter().any(|frame| frame["type"] == "agent_end"));
        let first_report = reports(&broker);
        assert_eq!(first_report.len(), 1);
        assert_eq!(first_report[0].body["entries"][0]["requests"], 1);

        let abort_begin = child.seen.len();
        child.send(json!({"id":"abort-held","type":"abort"})).await;
        child.success("abort-held").await;
        assert_eq!(child.seen[abort_begin..].iter().filter(|frame| frame["type"] == "agent_end").count(), 1);
        assert_eq!(child.state("after-abort").await["isStreaming"], false);

        child.send(json!({"id":"continue","type":"prompt","message":"continue this same Session"})).await;
        child.success("continue").await;
        child.until(|frame| frame["type"] == "agent_end").await;
        assert!(child.seen.iter().any(|frame| frame["type"] == "message_end"
            && contains_text(&frame["message"], "Same Session continuation complete")));
        assert_eq!(child.state("continued-state").await["sessionId"], session);

        child.send(json!({"id":"unknown","type":"prompt","message":"complete without cost or cache buckets"})).await;
        child.success("unknown").await;
        let unknown = child
            .until(|frame| {
                frame["type"] == "message_end" && contains_text(&frame["message"], "Unknown usage remains unknown")
            })
            .await;
        assert_eq!(unknown["message"]["usage"]["input"], 13);
        for field in ["cacheRead", "cacheWrite", "cost"] {
            assert!(unknown["message"]["usage"].get(field).is_none(), "{field} remains unknown");
        }
        child.until(|frame| frame["type"] == "agent_end").await;
        let final_state = child.state("final-state").await;
        assert_eq!(final_state["sessionId"], session);
        let file = PathBuf::from(final_state["sessionFile"].as_str().unwrap());
        child.finish().await;

        let posted = reports(&broker);
        assert_eq!(posted.len(), 2, "one live batch and one final batch; unknown effects are never replayed");
        let identity = posted[0].body["installId"].as_str().unwrap();
        assert!(!identity.is_empty());
        for (report, (input, output, read, write, cost)) in posted.iter().zip([(8, 5, 3, 2, 0.25), (2, 1, 0, 0, 0.0)]) {
            assert_eq!(report.body["installId"], identity, "same shared process owner through all Runs");
            assert_eq!(report.body["app"], "ara");
            let entries = report.body["entries"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            let entry = &entries[0];
            assert_eq!(entry["provider"], "custom");
            assert_eq!(entry["model"], "broker-model");
            assert_eq!(entry["requests"], 1);
            assert_eq!(entry["inputTokens"], input);
            assert_eq!(entry["outputTokens"], output);
            assert_eq!(entry["cacheReadTokens"], read);
            assert_eq!(entry["cacheWriteTokens"], write);
            assert_eq!(entry["costUsd"], cost);
            assert!(entry["at"].as_i64().unwrap() > 0);
        }
        let entries: Vec<Value> =
            std::fs::read_to_string(file).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        let unknown_journal =
            entries.iter().find(|entry| contains_text(&entry["message"], "Unknown usage remains unknown")).unwrap();
        for field in ["cacheRead", "cacheWrite", "cost"] {
            assert!(unknown_journal["message"]["usage"].get(field).is_none());
        }
        assert_eq!(model.served(), 4, "cancelled and observer-failed requests are not replayed");
        let requests = model.requests.lock().await;
        assert_eq!(requests.len(), 4);
        // The shared testkit intentionally redacts credential headers. This
        // observation proves presence/length, not the complete bearer value.
        assert!(requests.iter().all(|request| request["headers"]["authorization"]
            == format!("<redacted {} chars>", "Bearer ".len() + KEY.len())));
        drop(requests);
        assert!(
            !host.home.path().join("agent/auth.db").exists(),
            "the remote owner does not create local credential storage"
        );
        assert!(!child.stderr.lock().unwrap().contains(KEY));
        assert!(!child.seen.iter().any(|frame| frame.to_string().contains(KEY)));
    }
}
