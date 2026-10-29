#![allow(dead_code)]
use anyhow::{ensure, Result};
use ara_agent_lite_harness::{
    core::{
        self,
        model::ModelHost,
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    *,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}
pub fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}
pub fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish":"tool_calls"})
}
pub fn write_answer(value: &str) -> Value {
    call(&format!("write-{value}"), "write_answer", json!({"content":value}))
}

pub struct Provider {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Provider {
    pub fn new(responses: Vec<Value>) -> Self {
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
                let headers = String::from_utf8_lossy(&bytes[..header_end]);
                let is_get = headers.starts_with("GET /v1/models ");
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if is_get {
                    let body=json!({"data":[{"id":"harness-fixture","protocols":["openai-completions"],"context_window":1_000_000,"max_tokens":4096,"omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]}).to_string();
                    let _=write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body);
                    continue;
                }
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
                if let Some(ms) = response["delay_ms"].as_u64() {
                    thread::sleep(Duration::from_millis(ms));
                }
                let _ =
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for (delta, finish) in [
                    (json!({"role":"assistant"}), Value::Null),
                    (response["delta"].clone(), Value::Null),
                    (json!({}), response["finish"].clone()),
                ] {
                    let packet = json!({"id":"harness-controlled","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
                    let _ = write!(stream, "data: {packet}\n\n");
                }
                let _ = stream.write_all(b"data: [DONE]\n\n");
            }
        });
        Self { endpoint: format!("http://{addr}/v1"), requests, stop, worker: Some(worker) }
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

pub struct WriteAnswer {
    schema: Tool,
    path: PathBuf,
    pub writes: AtomicUsize,
}
impl WriteAnswer {
    pub fn new(root: &Path) -> Self {
        Self {
            schema: Tool {
                name: "write_answer".into(),
                description: "Write the submitted answer into this fixture's single result file.".into(),
                parameters: Schema(
                    json!({"type":"object","properties":{"content":{"type":"string"}},"required":["content"],"additionalProperties":false}),
                ),
                constrained_sampling: None,
            },
            path: root.join("answer.txt"),
            writes: AtomicUsize::new(0),
        }
    }
}
#[async_trait]
impl AgentTool for WriteAnswer {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "write_answer"
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let content = args["content"].as_str().ok_or_else(|| AgentError::Tool("content missing".into()))?;
        std::fs::write(&self.path, content).map_err(|e| AgentError::Tool(e.to_string()))?;
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(AgentToolResult::text("Actual answer file written; Host must verify it."))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    None,
    BadBasis,
    Uncommitted,
    Unknown,
    EmptyFeedback,
    ChangedSession,
    RepeatId,
    SlowReview,
    TamperEvents,
}

pub struct ReadTerms {
    schema: Tool,
    unknown: bool,
}
impl ReadTerms {
    pub fn new(unknown: bool) -> Self {
        Self {
            schema: Tool {
                name: "read_terms".into(),
                description: "Return task evidence as text.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            unknown,
        }
    }
}
#[async_trait]
impl AgentTool for ReadTerms {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "read_terms"
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let mut result = AgentToolResult::text(
            "Source text includes effects_unknown: and partial effects may remain as literal examples.",
        );
        if self.unknown {
            result.details = json!({"executed":"unknown"});
        }
        Ok(result)
    }
}

/// A real Core-registered tool whose in-flight cancellation still needs Host
/// settlement. The test owns the release barrier; all waits remain bounded.
pub struct DelayedCancelTool {
    schema: Tool,
    root: PathBuf,
    pub executions: AtomicUsize,
    pub started: AtomicBool,
    pub cancellation_seen: AtomicBool,
    pub joined: AtomicBool,
    release: tokio::sync::Notify,
}
impl DelayedCancelTool {
    pub fn new(root: &Path) -> Self {
        Self {
            schema: Tool {
                name: "delayed_cancel".into(),
                description: "Exercise an in-flight operation requiring joined cancellation settlement.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            root: root.into(),
            executions: AtomicUsize::new(0),
            started: AtomicBool::new(false),
            cancellation_seen: AtomicBool::new(false),
            joined: AtomicBool::new(false),
            release: tokio::sync::Notify::new(),
        }
    }
    pub fn release_settlement(&self) {
        // notify_one retains a permit if cancellation was observed just before
        // the callback reached its second wait.
        self.release.notify_one();
    }
}
#[async_trait]
impl AgentTool for DelayedCancelTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "delayed_cancel"
    }
    async fn execute(
        &self,
        call_id: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if args.as_object().is_none_or(|object| !object.is_empty()) {
            return Err(AgentError::Tool("delayed_cancel accepts no arguments".into()));
        }
        if self.executions.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(AgentError::Tool("fixture operation must never be replayed".into()));
        }
        let mut started = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join("inflight-started.json"))
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        started
            .write_all(json!({"call_id":call_id,"state":"started"}).to_string().as_bytes())
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        started.sync_all().map_err(|error| AgentError::Tool(error.to_string()))?;
        self.started.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(4), signal.cancelled())
            .await
            .map_err(|_| AgentError::Tool("fixture cancellation was not received within its bound".into()))?;
        self.cancellation_seen.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(4), self.release.notified())
            .await
            .map_err(|_| AgentError::Tool("fixture settlement was not released within its bound".into()))?;
        tokio::time::sleep(Duration::from_millis(40)).await;
        let mut joined = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.root.join("inflight-joined.json"))
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        joined
            .write_all(
                json!({"call_id":call_id,"callback_joined":true,"external_effects":"unknown"}).to_string().as_bytes(),
            )
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        joined.sync_all().map_err(|error| AgentError::Tool(error.to_string()))?;
        self.joined.store(true, Ordering::SeqCst);
        let mut result = AgentToolResult::text(
            "Original in-flight callback settled after cancellation; external effects remain unknown.",
        );
        result.details = json!({"executed":"unknown","effects_unknown":true,"callback_joined":true});
        Ok(result)
    }
}
/// Filesystem fault injection at the real observation commit boundary. This
/// writes the actual candidate once, then occupies the future file path with a
/// directory. No production persistence hook or synthetic Observation is used.
pub struct ObservationCollisionTool {
    schema: Tool,
    answer: PathBuf,
    observation: PathBuf,
    pub executions: AtomicUsize,
    pub writes: AtomicUsize,
}
impl ObservationCollisionTool {
    pub fn new(root: &Path, evidence: &Path) -> Self {
        Self {
            schema: Tool {
                name: "write_with_observation_collision".into(),
                description: "Write one fixture answer and inject an observation-file path conflict.".into(),
                parameters: Schema(
                    json!({"type":"object","properties":{"content":{"type":"string"}},"required":["content"],"additionalProperties":false}),
                ),
                constrained_sampling: None,
            },
            answer: root.join("answer.txt"),
            observation: evidence.join("run-1-observation.json"),
            executions: AtomicUsize::new(0),
            writes: AtomicUsize::new(0),
        }
    }
}
#[async_trait]
impl AgentTool for ObservationCollisionTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "write_with_observation_collision"
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if args.as_object().is_none_or(|object| object.len() != 1) {
            return Err(AgentError::Tool("fixture requires exactly content".into()));
        }
        let content = args["content"].as_str().ok_or_else(|| AgentError::Tool("content missing".into()))?;
        if self.executions.fetch_add(1, Ordering::SeqCst) != 0 {
            return Err(AgentError::Tool("fixture must not replay its actual write".into()));
        }
        let mut answer = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&self.answer)
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        answer.write_all(content.as_bytes()).map_err(|error| AgentError::Tool(error.to_string()))?;
        answer.sync_all().map_err(|error| AgentError::Tool(error.to_string()))?;
        self.writes.fetch_add(1, Ordering::SeqCst);
        std::fs::create_dir(&self.observation).map_err(|error| AgentError::Tool(error.to_string()))?;
        Ok(AgentToolResult::text("Actual candidate written; fixture observation path conflict prepared."))
    }
}

pub struct Host {
    pub root: PathBuf,
    pub endpoint: String,
    pub tool: Arc<WriteAnswer>,
    pub model_host: Arc<dyn ModelHost>,
    pub steering: Arc<Mutex<core::steering::SteeringBuffer>>,
    pub read_tool: Option<Arc<dyn AgentTool>>,
    pub reviews: AtomicUsize,
    pub preparations: AtomicUsize,
    pub review_started: AtomicBool,
    pub review_joined: AtomicBool,
    pub fault: Fault,
}
impl Host {
    pub fn new(root: &Path, provider: &Provider, fault: Fault) -> Self {
        Self {
            root: root.into(),
            endpoint: provider.endpoint.clone(),
            tool: Arc::new(WriteAnswer::new(root)),
            model_host: Arc::new(core::model::CatalogModelHost),
            steering: Arc::new(Mutex::new(core::steering::SteeringBuffer::default())),
            read_tool: None,
            reviews: AtomicUsize::new(0),
            preparations: AtomicUsize::new(0),
            review_started: AtomicBool::new(false),
            review_joined: AtomicBool::new(false),
            fault,
        }
    }
    pub fn request(&self, id: &str) -> RunRequest {
        let mut req = RunRequest::new(
            id,
            "Use the write_answer tool to submit the current answer. Stop after submission.",
            RuntimePaths::new(&self.root, self.root.join("session.json"), self.root.join("environment")),
        );
        req.endpoint = self.endpoint.clone();
        req.token = "harness-fixture-key".into();
        req.model = "harness-fixture".into();
        req.route.model = req.model.clone();
        req.route.protocol = "openai-completions".into();
        req.model_host = self.model_host.clone();
        req.steering = self.steering.clone();
        req.device_fp = "test-device".into();
        req.session_key = "test/session".into();
        req.mode = RunMode::Execute;
        req.max_calls = 4;
        req.max_seconds = 30;
        req.max_output = 512;
        req.extra_tools.push(ToolRegistration { tool: self.tool.clone(), allow_in_plan: false });
        if let Some(tool) = &self.read_tool {
            req.extra_tools.push(ToolRegistration { tool: tool.clone(), allow_in_plan: true });
        }
        req
    }
    pub fn spec(&self, name: &str) -> TaskSpec {
        TaskSpec {
            task_id: name.into(),
            criteria_revision: "actual-file-correct-v1".into(),
            authorization_revision: "single-file-v1".into(),
            kind: TaskKind::Coding,
            evidence_dir: self.root.join(name),
            max_runs: 3,
            max_calls: 8,
            deadline_unix_ms: now_ms() + 30_000,
        }
    }
}
#[async_trait]
impl HarnessHost for Host {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        self.review_started.store(true, Ordering::SeqCst);
        if self.fault == Fault::SlowReview {
            cancel.cancelled().await;
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.review_joined.store(true, Ordering::SeqCst);
        }
        let bytes = std::fs::read(self.root.join("answer.txt")).unwrap_or_default();
        let passed = bytes == b"correct";
        let proof = self.root.join(format!("verifier-{}.json", candidate.basis.run_number));
        std::fs::write(
            &proof,
            serde_json::to_vec(&json!({"basis":candidate.basis,"passed":passed,"actual_bytes":bytes}))?,
        )?;
        std::fs::OpenOptions::new().write(true).open(&proof)?.sync_all()?;
        if self.fault == Fault::TamperEvents {
            use std::io::Write;
            std::fs::OpenOptions::new().append(true).open(&candidate.basis.events_path)?.write_all(b"{}\n")?;
        }
        let mut basis = candidate.basis;
        if self.fault == Fault::BadBasis {
            basis.criteria_revision = "other-revision".into();
        }
        Ok(ReviewReceipt {
            basis,
            committed: self.fault != Fault::Uncommitted,
            settlement: if self.fault == Fault::Unknown { Settlement::Unknown } else { Settlement::Settled },
            verdict: if passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges {
                    feedback: if self.fault == Fault::EmptyFeedback {
                        String::new()
                    } else {
                        "The actual file fails the independent contract. Recheck and submit a repaired artifact.".into()
                    },
                }
            },
            evidence: vec![proof.display().to_string()],
        })
    }
    async fn prepare_continuation(&self, continuation: Continuation, _: CancellationToken) -> Result<RunRequest> {
        ensure!(continuation.receipt.committed, "uncommitted continuation");
        self.preparations.fetch_add(1, Ordering::SeqCst);
        let id = if self.fault == Fault::RepeatId {
            continuation.previous.basis.run_id.clone()
        } else {
            format!("repair-{}", continuation.next_run_number)
        };
        let mut req = self.request(&id);
        req.objective = format!(
            "Host verification feedback: {}. Inspect and repair the submitted artifact.",
            match continuation.receipt.verdict {
                Verdict::NeedsChanges { feedback } => feedback,
                _ => String::new(),
            }
        );
        if self.fault == Fault::ChangedSession {
            req.state_path = self.root.join("different-session.json");
        }
        Ok(req)
    }
}

pub fn events(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}
