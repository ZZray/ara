//! MCP tools under the real Core `execute` driven by the Harness (42.1, 42.6):
//! one successful call, and one timed-out call that stops the Task at
//! `ReconciliationRequired` with the call in `unknown_effects`.
mod common;

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

use anyhow::Result;
use ara_agent_lite_harness::{
    core::{
        model::CatalogModelHost,
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    *,
};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}
fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}
fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish":"tool_calls"})
}

/// Loopback OpenAI-compatible provider (pattern of the Harness test helper).
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
                    let body = json!({"data":[{"id":"harness-fixture","protocols":["openai-completions"],"context_window":1_000_000,"max_tokens":4096,"omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]}).to_string();
                    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
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
                let _ =
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for (delta, finish) in [
                    (json!({"role":"assistant"}), Value::Null),
                    (response["delta"].clone(), Value::Null),
                    (json!({}), response["finish"].clone()),
                ] {
                    let packet = json!({"id":"mcp-controlled","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
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

/// The Task's answer channel; the Host verifies the file.
struct WriteAnswer {
    schema: Tool,
    path: PathBuf,
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
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let content = args["content"].as_str().ok_or_else(|| AgentError::Tool("content missing".into()))?;
        std::fs::write(&self.path, content).map_err(|e| AgentError::Tool(e.to_string()))?;
        Ok(AgentToolResult::text("Answer written."))
    }
}

struct Host {
    root: PathBuf,
    endpoint: String,
    tools: Vec<Arc<dyn AgentTool>>,
    reviews: AtomicUsize,
}
impl Host {
    fn request(&self, id: &str) -> RunRequest {
        let mut req = RunRequest::new(
            id,
            "Call the MCP tool, then submit the answer with write_answer.",
            RuntimePaths::new(&self.root, self.root.join("session.json"), self.root.join("environment")),
        );
        req.endpoint = self.endpoint.clone();
        req.token = "mcp-fixture-key".into();
        req.model = "harness-fixture".into();
        req.route.model = req.model.clone();
        req.route.protocol = "openai-completions".into();
        req.model_host = Arc::new(CatalogModelHost);
        req.device_fp = "test-device".into();
        req.session_key = "test/session".into();
        req.mode = RunMode::Execute;
        req.max_calls = 4;
        req.max_seconds = 30;
        req.max_output = 512;
        let answer = WriteAnswer {
            schema: Tool {
                name: "write_answer".into(),
                description: "Write the answer file.".into(),
                parameters: Schema(
                    json!({"type":"object","properties":{"content":{"type":"string"}},"required":["content"],"additionalProperties":false}),
                ),
                constrained_sampling: None,
            },
            path: self.root.join("answer.txt"),
        };
        req.extra_tools.push(ToolRegistration { tool: Arc::new(answer), allow_in_plan: false });
        for tool in &self.tools {
            req.extra_tools.push(ToolRegistration { tool: tool.clone(), allow_in_plan: false });
        }
        req
    }
    fn spec(&self, name: &str) -> TaskSpec {
        TaskSpec {
            task_id: name.into(),
            criteria_revision: "mcp-v1".into(),
            authorization_revision: "mcp-v1".into(),
            kind: TaskKind::Coding,
            evidence_dir: self.root.join(name),
            max_runs: 1,
            max_calls: 8,
            deadline_unix_ms: now_ms() + 30_000,
        }
    }
}
#[async_trait]
impl HarnessHost for Host {
    async fn verify(&self, candidate: Candidate, _: CancellationToken) -> Result<ReviewReceipt> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        let passed = std::fs::read(self.root.join("answer.txt")).unwrap_or_default() == b"correct";
        let proof = self.root.join("verifier.json");
        std::fs::write(&proof, serde_json::to_vec(&json!({"passed":passed}))?)?;
        Ok(ReviewReceipt {
            basis: candidate.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict: if passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges { feedback: "Wrong answer.".into() }
            },
            evidence: vec![proof.display().to_string()],
        })
    }
    async fn prepare_continuation(&self, _: Continuation, _: CancellationToken) -> Result<RunRequest> {
        anyhow::bail!("single-run fixture")
    }
}

async fn run_task(root: &Path, responses: Vec<Value>, call_timeout: Duration) -> (Arc<TaskOutcome>, Vec<Value>, usize) {
    let mut opts = options();
    opts.call_timeout = call_timeout;
    let session = connect(config(vec![("fake", fake(&[], None, &[]))]), opts).await;
    let provider = Provider::new(responses);
    let host = Arc::new(Host {
        root: root.into(),
        endpoint: provider.endpoint.clone(),
        tools: session.tools(),
        reviews: AtomicUsize::new(0),
    });
    let handle = start(host.spec("mcp-task"), host.request("mcp-run"), host.clone()).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), handle.wait()).await.unwrap();
    let requests = provider.requests.lock().unwrap().clone();
    session.shutdown().await;
    (outcome, requests, host.reviews.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_tool_succeeds_inside_a_task() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let responses = vec![
        call("mcp-1", "mcp__fake__echo", json!({"text":"mcp-echo-payload"})),
        call("write-1", "write_answer", json!({"content":"correct"})),
        text("Submitted."),
    ];
    let (outcome, requests, reviews) = run_task(&root, responses, Duration::from_secs(20)).await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert!(outcome.unknown_effects.is_empty());
    assert_eq!(reviews, 1);
    let tools = requests[0]["tools"].to_string();
    assert!(tools.contains("mcp__fake__echo"), "the provider saw the MCP tool");
    assert!(requests[1].to_string().contains("mcp-echo-payload"), "the MCP result reached the model");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_timeout_stops_at_reconciliation() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let responses = vec![
        call("mcp-2", "mcp__fake__slow", json!({"ms":5000})),
        call("write-2", "write_answer", json!({"content":"correct"})),
        text("Submitted."),
    ];
    let (outcome, _, reviews) = run_task(&root, responses, Duration::from_millis(700)).await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(reviews, 0);
    assert_eq!(outcome.unknown_effects.len(), 1, "{outcome:?}");
    let record = &outcome.unknown_effects[0];
    assert_eq!(record.run_id, "mcp-run");
    assert_eq!(record.native_call_id, "mcp-2");
    assert_eq!(record.tool, "mcp__fake__slow");
}
