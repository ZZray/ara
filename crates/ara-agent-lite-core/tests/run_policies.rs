//! Controlled tests of the Host opt-in Run policies and the new Core hooks:
//! Completion_Gate (17), Stuck_Detector (18), structured unknown effects on
//! `tool_end` (22.2), duplicate native call IDs (24.7) and the output-limit
//! receipt on `model_call` (15.4). A loopback provider replays scripted
//! responses; no real model or OS effect is involved.
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_core::{
    completion_gate::{CompletionGate, GateRequest, GateVerdict, Registration},
    execute,
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    stuck_detector::StuckPolicy,
    RunMode, RunRequest, RuntimePaths, ToolRegistration,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Fresh directory under the system temp dir, removed on drop.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

mod tempfile {
    pub fn tempdir() -> std::io::Result<super::TempDir> {
        let path = std::env::temp_dir().join(format!("lite-run-policies-{}", ara_agent_lite_core::fresh_id()));
        std::fs::create_dir_all(&path)?;
        Ok(super::TempDir(path.canonicalize()?))
    }
}

fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}

fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish":"tool_calls"})
}

/// Loopback OpenAI-compatible provider: catalog GET plus scripted streams.
struct Provider {
    endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Provider {
    fn new(responses: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let worker = thread::spawn(move || {
            let mut responses = VecDeque::from(responses);
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else { break };
                if done.load(Ordering::SeqCst) {
                    break;
                }
                stream.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
                let mut bytes = Vec::new();
                let mut chunk = [0u8; 8192];
                let header_end = loop {
                    let Ok(n) = stream.read(&mut chunk) else { break None };
                    if n == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        break Some(i + 4);
                    }
                };
                let Some(header_end) = header_end else { continue };
                let headers = String::from_utf8_lossy(&bytes[..header_end]).to_string();
                if headers.starts_with("GET /v1/models ") {
                    let body = json!({"data":[{"id":"policy-fixture","protocols":["openai-completions"],
                        "context_window":1_000_000,"max_tokens":4096,
                        "omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]})
                    .to_string();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while bytes.len() < header_end + length {
                    let Ok(n) = stream.read(&mut chunk) else { break };
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                let Ok(body) = serde_json::from_slice::<Value>(&bytes[header_end..]) else { continue };
                captured.lock().unwrap().push(body);
                let response = responses.pop_front().unwrap_or_else(|| text("No further controlled response."));
                let _ =
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for (delta, finish) in [
                    (json!({"role":"assistant"}), Value::Null),
                    (response["delta"].clone(), Value::Null),
                    (json!({}), response["finish"].clone()),
                ] {
                    let packet = json!({"id":"policy","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
                    let _ = write!(stream, "data: {packet}\n\n");
                }
                let _ = stream.write_all(b"data: [DONE]\n\n");
            }
        });
        Self { endpoint: format!("http://{addr}/v1"), requests, stop, worker: Some(worker) }
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
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

/// Business tool with a fixed result and optional structured details.
struct Probe {
    schema: Tool,
    calls: AtomicUsize,
    details: Value,
    fail: bool,
}

impl Probe {
    fn new(details: Value, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            schema: Tool {
                name: "probe".into(),
                description: "Return the fixture state.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            calls: AtomicUsize::new(0),
            details,
            fail,
        })
    }
}

#[async_trait]
impl AgentTool for Probe {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "probe"
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(AgentError::Tool("fixture state unchanged".into()));
        }
        let mut result = AgentToolResult::text("fixture state unchanged");
        result.details = self.details.clone();
        Ok(result)
    }
}

fn request(root: &Path, provider: &Provider, probe: &Arc<Probe>, max_calls: usize) -> RunRequest {
    let mut request = RunRequest::new(
        format!("run-{}", ara_agent_lite_core::fresh_id()),
        "Use probe and report the state.",
        RuntimePaths::new(root, root.join("session.json"), root.join("environment")),
    );
    request.endpoint = provider.endpoint.clone();
    request.token = "policy-fixture-key".into();
    request.model = "policy-fixture".into();
    request.route.model = request.model.clone();
    request.route.protocol = "openai-completions".into();
    request.mode = RunMode::Execute;
    request.max_calls = max_calls;
    request.max_seconds = 30;
    request.max_output = 512;
    request.extra_tools.push(ToolRegistration { tool: probe.clone(), allow_in_plan: true });
    request
}

async fn run(request: RunRequest) -> (ara_agent_lite_core::RunResult, Vec<Value>) {
    let (sender, mut receiver) = mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let result = execute(request, CancellationToken::new(), sender).await.unwrap();
    (result, collector.await.unwrap())
}

fn of_type<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|event| event["type"] == kind).collect()
}

/// Fails the first check, passes later ones.
struct FlakyGate {
    checks: AtomicUsize,
    pass_after: usize,
}

#[async_trait]
impl CompletionGate for FlakyGate {
    async fn check(&self, request: GateRequest, _: CancellationToken) -> Result<GateVerdict> {
        let number = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        assert_eq!(request.sequence as usize, number);
        Ok(if number > self.pass_after {
            GateVerdict::Passed
        } else {
            GateVerdict::Failed { feedback: format!("public check {number} failed: 2 of 7 cases") }
        })
    }
}

fn gate(pass_after: usize) -> (Arc<FlakyGate>, Registration) {
    let gate = Arc::new(FlakyGate { checks: AtomicUsize::new(0), pass_after });
    (gate.clone(), Registration { check_id: "public-checks".into(), gate, timeout: Some(Duration::from_secs(5)) })
}

#[tokio::test]
async fn failed_gate_continues_the_same_run_with_sourced_feedback_then_finishes() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![text("done"), text("fixed")]);
    let probe = Probe::new(Value::Null, false);
    let mut request = request(root.path(), &provider, &probe, 4);
    let (checks, registration) = gate(1);
    request.policies.completion_gate = Some(registration);
    let (result, events) = run(request).await;
    assert_eq!((result.code, result.text.as_str()), (0, "fixed"), "{}", result.error);
    assert_eq!(checks.checks.load(Ordering::SeqCst), 2);
    let gates = of_type(&events, "completion_gate");
    assert_eq!(gates.iter().map(|event| event["result"].as_str().unwrap()).collect::<Vec<_>>(), ["failed", "passed"]);
    assert!(gates[0]["feedback_ref"].as_str().unwrap().starts_with("ref-"));
    assert!(gates[1]["feedback_ref"].is_null());
    // The check itself reserved no model call: two agent calls for two answers.
    assert_eq!(provider.request_count(), 2);
    let second = provider.requests.lock().unwrap()[1].to_string();
    assert!(second.contains("source=completion_gate; check_id=public-checks; sequence=1"));
    assert!(second.contains("public check 1 failed"));
    let end = of_type(&events, "agent_end")[0];
    assert!(end.get("completion_gate").is_none() && end.get("stop_evidence").is_none());
}

#[tokio::test]
async fn failed_gate_without_budget_stops_with_the_existing_budget_reason() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![text("done")]);
    let probe = Probe::new(Value::Null, false);
    let mut request = request(root.path(), &provider, &probe, 1);
    let (_, registration) = gate(usize::MAX);
    request.policies.completion_gate = Some(registration);
    let (result, events) = run(request).await;
    assert_eq!(result.error, "model call budget exhausted before a final response");
    assert_eq!(provider.request_count(), 1);
    let end = of_type(&events, "agent_end")[0];
    assert_eq!(end["completion_gate"]["result"], "failed");
    assert_eq!(end["completion_gate"]["sequence"], 1);
}

#[tokio::test]
async fn unregistered_gate_and_default_policies_add_no_events() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![text("done")]);
    let probe = Probe::new(Value::Null, false);
    let (result, events) = run(request(root.path(), &provider, &probe, 2)).await;
    assert_eq!(result.code, 0, "{}", result.error);
    for kind in ["completion_gate", "stuck_detector", "compaction_refresh"] {
        assert!(of_type(&events, kind).is_empty(), "{kind}");
    }
    let call = of_type(&events, "model_call")[0];
    // 15.4: the fixture catalog declares 4096, below the builtin 8192.
    assert_eq!(call["output_limit"], json!({"effective":512,"sources":["request"]}));
}

#[tokio::test]
async fn repeated_identical_calls_get_one_correction_then_stop_with_no_progress_evidence() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(
        (1..=4).map(|n| call(&format!("c{n}"), "probe", json!({}))).chain([text("unreachable")]).collect(),
    );
    let probe = Probe::new(Value::Null, false);
    let mut request = request(root.path(), &provider, &probe, 8);
    request.policies.stuck = Some(StuckPolicy { repeated_call: Some(2), ..StuckPolicy::default() });
    let (result, events) = run(request).await;
    assert_eq!(result.error, "no_progress: repeated_call");
    assert_eq!(probe.calls.load(Ordering::SeqCst), 4);
    assert_eq!(provider.request_count(), 4);
    let actions: Vec<_> =
        of_type(&events, "stuck_detector").iter().map(|event| event["action"].as_str().unwrap().to_owned()).collect();
    assert_eq!(actions, ["rule_hit", "correction", "no_progress"]);
    let third = provider.requests.lock().unwrap()[2].to_string();
    assert!(third.contains("source=stuck_detector; rule=repeated_call; native_call_ids=c1,c2"));
    let evidence = &of_type(&events, "agent_end")[0]["stop_evidence"];
    assert_eq!(evidence["cause"], "no_progress");
    assert_eq!(evidence["native_call_ids"], json!(["c3", "c4"]));
    assert_eq!(evidence["native_round_closed"], true);
}

#[tokio::test]
async fn invalid_stuck_threshold_is_refused_before_any_request() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![]);
    let probe = Probe::new(Value::Null, false);
    let mut request = request(root.path(), &provider, &probe, 2);
    request.policies.stuck = Some(StuckPolicy { repeated_error: Some(1), ..StuckPolicy::default() });
    let (sender, _receiver) = mpsc::channel(8);
    let error = execute(request, CancellationToken::new(), sender).await.err().unwrap();
    assert!(error.to_string().contains("repeated_error"), "{error}");
    assert_eq!(provider.request_count(), 0);
    assert!(!root.path().join("session.json").exists());
}

#[tokio::test]
async fn structured_unknown_details_mark_tool_end() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("u1", "probe", json!({})), text("done")]);
    let probe = Probe::new(json!({"executed":"unknown","source":"fixture"}), false);
    let (_, events) = run(request(root.path(), &provider, &probe, 3)).await;
    let end = of_type(&events, "tool_end")[0];
    assert_eq!(end["effects_unknown"], true);
    let plain = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("k1", "probe", json!({})), text("done")]);
    let known = Probe::new(json!({"executed":"no"}), false);
    let (_, events) = run(request(plain.path(), &provider, &known, 3)).await;
    assert!(of_type(&events, "tool_end")[0].get("effects_unknown").is_none());
}

#[tokio::test]
async fn a_reused_native_call_id_is_never_dispatched_again() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("dup", "probe", json!({})), call("dup", "probe", json!({})), text("x")]);
    let probe = Probe::new(Value::Null, false);
    let (result, _) = run(request(root.path(), &provider, &probe, 4)).await;
    assert_eq!(probe.calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.error, "duplicate_native_call_id: dup");
}
