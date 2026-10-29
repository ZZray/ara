//! The consumer Host against a controlled loopback provider: the agent loop
//! runs inside Lite Core via the Harness `start` entry, the Host check reads
//! the actual file, and a failed check continues in the same Session.
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ara_agent_lite_core::{
    model::{GenerationPolicy, LengthRecoveryPolicy},
    rpi_agent::AgentTool,
};
use ara_agent_lite_harness::{start, TaskKind, TaskSpec, TaskStop};
use lite_consumer_check::{Config, ConsumerHost, WorkspaceTool};
use serde_json::{json, Value};

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn fresh_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lite-consumer-check-{name}-{}-{}", std::process::id(), now_ms()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_call(id: &str, content: &str) -> Value {
    let arguments = json!({"path":"answer.txt","content":content}).to_string();
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":"ws_write_file","arguments":arguments}}]},"finish":"tool_calls"})
}

fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}

/// OpenAI-compatible loopback upstream: a model catalog plus scripted SSE replies.
struct Provider {
    endpoint: String,
    /// JSON body of every model request, in arrival order.
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn new(replies: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (done, count) = (stop.clone(), requests.clone());
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            for stream in listener.incoming() {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                    }
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(header_end) = header_end else { continue };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
                if headers.starts_with("GET /v1/models ") {
                    let body = json!({"data":[{"id":"consumer-fixture","protocols":["openai-completions"],"context_window":1_000_000,"max_tokens":4096,"omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]}).to_string();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    continue;
                }
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                    }
                }
                let body = serde_json::from_slice(&bytes[header_end..]).unwrap_or(Value::Null);
                count.lock().unwrap().push(body);
                let reply = replies.pop_front().unwrap_or_else(|| text("No further controlled reply."));
                let _ =
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for (delta, finish) in [
                    (json!({"role":"assistant"}), Value::Null),
                    (reply["delta"].clone(), Value::Null),
                    (json!({}), reply["finish"].clone()),
                ] {
                    let packet = json!({"id":"consumer-controlled","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
                    let _ = write!(stream, "data: {packet}\n\n");
                }
                let _ = stream.write_all(b"data: [DONE]\n\n");
            }
        });
        Self { endpoint: format!("http://{address}/v1"), requests, stop, worker: Some(worker) }
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://").trim_end_matches("/v1"));
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

/// Public check: exit 0 only when `answer.txt` holds exactly `ok`.
fn check_command() -> Vec<String> {
    if cfg!(windows) {
        [
            "powershell",
            "-NoProfile",
            "-Command",
            "if ((Get-Content -Raw answer.txt) -ceq 'ok') { exit 0 } else { exit 1 }",
        ]
        .map(String::from)
        .to_vec()
    } else {
        ["sh", "-c", "test \"$(cat answer.txt)\" = ok"].map(String::from).to_vec()
    }
}

fn config(root: &Path, provider: &Provider) -> Config {
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    Config {
        workspace,
        state_dir: root.join("state"),
        endpoint: provider.endpoint.clone(),
        model: String::from("consumer-fixture"),
        protocol: String::from("openai-completions"),
        route_binding: String::from("loopback-binding"),
        route_decision: String::from("loopback fixture route"),
        generation_policy: GenerationPolicy::ProviderDefault,
        length_recovery: LengthRecoveryPolicy::Stop,
        token: String::from("loopback-fixture-key"),
        run_max_calls: 4,
        run_max_seconds: 60,
        max_output: 512,
        check: check_command(),
        check_timeout: Duration::from_secs(60),
    }
}

fn spec(root: &Path, task_id: &str) -> TaskSpec {
    TaskSpec {
        task_id: task_id.into(),
        criteria_revision: String::from("answer-ok-v1"),
        authorization_revision: String::from("workspace-tools-v1"),
        kind: TaskKind::Coding,
        evidence_dir: root.join("evidence"),
        max_runs: 3,
        max_calls: 8,
        deadline_unix_ms: now_ms() + 120_000,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_public_check_continues_in_the_same_session_until_the_check_passes() {
    let root = fresh_dir("repair");
    let provider = Provider::new(vec![write_call("w1", "wrong"), text("done"), write_call("w2", "ok"), text("done")]);
    let config = config(&root, &provider);
    let workspace = config.workspace.clone();
    let host = Arc::new(ConsumerHost::new(config).unwrap());
    let request = host.request("repair-run-1", "Write ok into answer.txt with ws_write_file, then stop.");
    let handle = start(spec(&root, "repair"), request, host).unwrap();
    let outcome = handle.wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{}", outcome.detail);
    assert_eq!(outcome.runs, 2);
    assert!(!outcome.task_accepted);
    assert_eq!(std::fs::read_to_string(workspace.join("answer.txt")).unwrap(), "ok");
    let bodies = provider.requests.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4);
    assert!(
        bodies.iter().all(|body| body.is_object() && body.get("thinking").is_none()),
        "provider_default sends no thinking field"
    );
    let first: Value =
        serde_json::from_slice(&std::fs::read(root.join("state/host/check-run-1.json")).unwrap()).unwrap();
    let second: Value =
        serde_json::from_slice(&std::fs::read(root.join("state/host/check-run-2.json")).unwrap()).unwrap();
    assert_eq!((first["passed"].as_bool(), second["passed"].as_bool()), (Some(false), Some(true)));
    let _ = std::fs::remove_dir_all(&root);
}

/// Run_Policy `generation_policy: thinking_type_disabled` reaches every model
/// request through the consumer's ModelHost, built only from stable items.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_policy_thinking_type_disabled_is_sent_on_every_model_request() {
    let root = fresh_dir("policy");
    let provider = Provider::new(vec![write_call("w1", "ok"), text("done")]);
    let mut config = config(&root, &provider);
    config.generation_policy = serde_json::from_value(json!("thinking_type_disabled")).unwrap();
    config.length_recovery = serde_json::from_value(json!("stop")).unwrap();
    let workspace = config.workspace.clone();
    let host = Arc::new(ConsumerHost::new(config).unwrap());
    let request = host.request("policy-run-1", "Write ok into answer.txt with ws_write_file, then stop.");
    let handle = start(spec(&root, "policy"), request, host).unwrap();
    let outcome = handle.wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{}", outcome.detail);
    assert_eq!(outcome.runs, 1);
    assert_eq!(std::fs::read_to_string(workspace.join("answer.txt")).unwrap(), "ok");
    let bodies = provider.requests.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2);
    for body in &bodies {
        assert_eq!(body["thinking"], json!({"type": "disabled"}), "{body}");
        assert_eq!(body["model"], json!("consumer-fixture"));
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workspace_tools_refuse_paths_outside_the_workspace() {
    let root = fresh_dir("confine");
    let workspace = root.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(root.join("outside.txt"), "secret").unwrap();
    let canonical = workspace.canonicalize().unwrap();
    let partial: Arc<dyn Fn(ara_agent_lite_core::rpi_agent::ToolResultPartial) + Send + Sync> = Arc::new(|_| {});
    let read = WorkspaceTool::read(canonical.clone());
    let write = WorkspaceTool::write(canonical);
    let cancel = tokio_util::sync::CancellationToken::new();
    for path in ["../outside.txt", "sub/../../outside.txt", "", root.join("outside.txt").to_str().unwrap()] {
        let refused = read.execute("r", json!({"path":path}), cancel.clone(), partial.clone()).await;
        assert!(refused.is_err(), "read {path:?} must be refused");
        let refused = write.execute("w", json!({"path":path,"content":"x"}), cancel.clone(), partial.clone()).await;
        assert!(refused.is_err(), "write {path:?} must be refused");
    }
    assert_eq!(std::fs::read_to_string(root.join("outside.txt")).unwrap(), "secret");
    write
        .execute("w", json!({"path":"./dir/a.txt","content":"inside"}), cancel.clone(), partial.clone())
        .await
        .unwrap();
    let text = read.execute("r", json!({"path":"dir/a.txt"}), cancel, partial).await.unwrap();
    assert!(format!("{:?}", text.content).contains("inside"));
    let _ = std::fs::remove_dir_all(&root);
}
