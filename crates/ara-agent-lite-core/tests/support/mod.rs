//! Shared fixtures of the Core capability tests: a loopback OpenAI-compatible
//! provider that replays scripted responses, a scripted business tool and a
//! Run driver that collects every event. No real model or OS effect is used.
#![allow(dead_code)]

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
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_core::{
    execute,
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    RunMode, RunRequest, RunResult, RuntimePaths, ToolRegistration,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Fresh canonical directory under the system temp dir, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!("lite-core-test-{}", ara_agent_lite_core::fresh_id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}

pub fn length(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"length"})
}

pub fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]},"finish":"tool_calls"})
}

/// A non-streaming HTTP failure with the given status.
pub fn http_error(status: u16) -> Value {
    json!({"http_status": status})
}

/// A strict summary response accepted by `install_summary`.
pub fn summary() -> Value {
    text(r#"{"summary":"Concise sourced working notes.","candidates":[]}"#)
}

pub const SUMMARY_PROMPT: &str = "Summarize the supplied closed conversation";

pub struct ProviderConfig {
    pub context_window: u64,
    pub max_tokens: u64,
    /// Replies to agent requests, in order.
    pub responses: Vec<Value>,
    /// Replies to compaction summary requests; a valid summary when exhausted.
    pub summary_responses: Vec<Value>,
    /// Runs before the reply to each chat request with its body.
    pub on_request: Box<dyn Fn(&Value) + Send>,
}

impl ProviderConfig {
    pub fn new(responses: Vec<Value>) -> Self {
        Self {
            context_window: 1_000_000,
            max_tokens: 4096,
            responses,
            summary_responses: Vec::new(),
            on_request: Box::new(|_| {}),
        }
    }
}

pub fn is_summary(body: &Value) -> bool {
    body.to_string().contains(SUMMARY_PROMPT)
}

/// One assistant message with several tool calls.
pub fn calls(items: &[(&str, &str, Value)]) -> Value {
    let calls: Vec<Value> = items
        .iter()
        .enumerate()
        .map(|(index, (id, name, args))| {
            json!({"index":index,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})
        })
        .collect();
    json!({"delta":{"tool_calls":calls},"finish":"tool_calls"})
}

/// Loopback OpenAI-compatible provider: catalog GET plus scripted streams.
pub struct Provider {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    pub catalog_requests: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Provider {
    pub fn new(responses: Vec<Value>) -> Self {
        Self::with(ProviderConfig::new(responses))
    }

    pub fn with(config: ProviderConfig) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let catalog_requests = Arc::new(AtomicUsize::new(0));
        let catalog = catalog_requests.clone();
        let ProviderConfig { context_window, max_tokens, responses, summary_responses, on_request } = config;
        let worker = thread::spawn(move || {
            let mut responses = VecDeque::from(responses);
            let mut summary_responses = VecDeque::from(summary_responses);
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
                if headers.starts_with("GET ") {
                    catalog.fetch_add(1, Ordering::SeqCst);
                    let body = json!({"data":[{"id":"core-fixture","protocols":["openai-completions"],
                        "context_window":context_window,"max_tokens":max_tokens,
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
                on_request(&body);
                let response = if is_summary(&body) {
                    summary_responses.pop_front().unwrap_or_else(summary)
                } else {
                    responses.pop_front().unwrap_or_else(|| text("No further controlled response."))
                };
                captured.lock().unwrap().push(body);
                if let Some(status) = response["http_status"].as_u64() {
                    let body =
                        json!({"error":{"message":"controlled upstream failure","type":"server_error"}}).to_string();
                    let _ = write!(stream, "HTTP/1.1 {status} Controlled\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                    continue;
                }
                let _ =
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
                for (delta, finish) in [
                    (json!({"role":"assistant"}), Value::Null),
                    (response["delta"].clone(), Value::Null),
                    (json!({}), response["finish"].clone()),
                ] {
                    let packet = json!({"id":"core","object":"chat.completion.chunk","choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
                    let _ = write!(stream, "data: {packet}\n\n");
                }
                let _ = stream.write_all(b"data: [DONE]\n\n");
            }
        });
        Self { endpoint: format!("http://{addr}/v1"), requests, catalog_requests, stop, worker: Some(worker) }
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
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

type Behavior = dyn Fn(&Value) -> std::result::Result<String, String> + Send + Sync;

/// Business tool whose result is computed from its arguments. While `hold`
/// is set, an execution marks `started` and never returns (a crash fixture).
pub struct ScriptedTool {
    schema: Tool,
    pub calls: AtomicUsize,
    behavior: Box<Behavior>,
    pub hold: Arc<AtomicBool>,
    pub started: Arc<AtomicBool>,
}

impl ScriptedTool {
    pub fn new(
        name: &str,
        behavior: impl Fn(&Value) -> std::result::Result<String, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Self::holding(name, Arc::default(), Arc::default(), behavior)
    }

    pub fn holding(
        name: &str,
        hold: Arc<AtomicBool>,
        started: Arc<AtomicBool>,
        behavior: impl Fn(&Value) -> std::result::Result<String, String> + Send + Sync + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            schema: Tool {
                name: name.into(),
                description: format!("Controlled fixture tool {name}."),
                parameters: Schema(json!({"type":"object","additionalProperties":true})),
                constrained_sampling: None,
            },
            calls: AtomicUsize::new(0),
            behavior: Box::new(behavior),
            hold,
            started,
        })
    }

    pub fn count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AgentTool for ScriptedTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        _: &str,
        arguments: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.hold.load(Ordering::SeqCst) {
            self.started.store(true, Ordering::SeqCst);
            std::future::pending::<()>().await;
        }
        match (self.behavior)(&arguments) {
            Ok(text) => Ok(AgentToolResult::text(text)),
            Err(error) => Err(AgentError::Tool(error)),
        }
    }
}

/// Execute-mode Run over `provider` with the given custom tools.
pub fn request(
    root: &Path,
    provider: &Provider,
    tools: &[Arc<ScriptedTool>],
    max_calls: usize,
    objective: &str,
) -> RunRequest {
    let mut request = RunRequest::new(
        format!("run-{}", ara_agent_lite_core::fresh_id()),
        objective,
        RuntimePaths::new(root, root.join("session.json"), root.join("environment")),
    );
    request.endpoint = provider.endpoint.clone();
    request.token = "core-fixture-key".into();
    request.model = "core-fixture".into();
    request.route.model = request.model.clone();
    request.route.protocol = "openai-completions".into();
    request.mode = RunMode::Execute;
    request.max_calls = max_calls;
    request.max_seconds = 30;
    request.max_output = 512;
    for tool in tools {
        request.extra_tools.push(ToolRegistration { tool: tool.clone(), allow_in_plan: true });
    }
    request
}

pub async fn run(request: RunRequest) -> (RunResult, Vec<Value>) {
    run_with(request, CancellationToken::new()).await
}

pub async fn run_with(request: RunRequest, cancellation: CancellationToken) -> (RunResult, Vec<Value>) {
    let (sender, mut receiver) = mpsc::channel(256);
    let collector = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    let result = execute(request, cancellation, sender).await.unwrap();
    (result, collector.await.unwrap())
}

pub fn of_type<'a>(events: &'a [Value], kind: &str) -> Vec<&'a Value> {
    events.iter().filter(|event| event["type"] == kind).collect()
}

/// Runtime for driving Runs inside synchronous proptest closures.
pub fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

/// Run `request` on its own runtime and drop that runtime once `started` is
/// set, leaving the Session fenced as after a process crash mid-tool.
pub fn crash_mid_tool(request: RunRequest, started: Arc<AtomicBool>) {
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let (sender, mut receiver) = mpsc::channel(256);
            tokio::spawn(async move { while receiver.recv().await.is_some() {} });
            tokio::spawn(async move {
                let _ = execute(request, CancellationToken::new(), sender).await;
            });
            tokio::time::timeout(Duration::from_secs(10), async {
                while !started.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("tool started before the crash");
        });
        runtime.shutdown_timeout(Duration::from_millis(0));
    })
    .join()
    .unwrap();
}

/// Every serialized tool result of the Session checkpoint, in order.
pub fn session_tool_results(session: &Path) -> Vec<Value> {
    let state: Value = serde_json::from_slice(&std::fs::read(session).unwrap()).unwrap();
    state["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message.get("toolCallId").is_some() && message.get("isError").is_some())
        .cloned()
        .collect()
}

/// The current-Run budget carrier of a chat request, if present.
pub fn run_budget(body: &Value) -> Option<Value> {
    const PREFIX: &str = "Fresh Host current-Run budget";
    body["messages"].as_array()?.iter().rev().find_map(|message| {
        let content = match &message["content"] {
            Value::String(text) => text.clone(),
            Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join(""),
            _ => return None,
        };
        let (_, json) = content.strip_prefix(PREFIX)?.split_once('\n')?;
        serde_json::from_str(json).ok()
    })
}
