//! Controlled test of a compaction summary the budget cannot pay for
//! (requirements 25.3, 25.4) over the real Core `execute`. The loopback
//! provider declares a small context window so the second Run must compact.
mod common;

use std::{
    collections::VecDeque,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_harness::{core::RunRequest, *};
use async_trait::async_trait;
use common::{call, events, text, Fault, Host};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// `common::Provider` with a configurable declared context window.
struct SmallProvider {
    endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SmallProvider {
    /// `on_request(n)` runs before the response to the n-th chat request (0-based).
    fn new(context_window: u64, responses: Vec<Value>, on_request: Box<dyn Fn(usize) + Send>) -> Self {
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
                let length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if headers.starts_with("GET /v1/models ") {
                    let body = json!({"data":[{"id":"harness-fixture","protocols":["openai-completions"],
                        "context_window":context_window,"max_tokens":4096,
                        "omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]})
                    .to_string();
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
                let count = {
                    let mut captured = captured.lock().unwrap();
                    captured.push(body);
                    captured.len() - 1
                };
                on_request(count);
                let response = responses.pop_front().unwrap_or_else(|| text("No further controlled response."));
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

impl Drop for SmallProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://").trim_end_matches("/v1"));
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

/// Continues every NeedsChanges review with a one-call Run.
struct OneCallContinuation(Host);

#[async_trait]
impl HarnessHost for OneCallContinuation {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.0.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        let mut request = self.0.prepare_continuation(continuation, cancel).await?;
        request.max_calls = 1;
        Ok(request)
    }
}

/// Output 512 + wire reserve 4096 leave 60 000 input bytes: a 30 000-byte
/// projection. Run 1 adds a ~16 KiB tool call and Run 2 a ~16 KiB answer, so
/// only the turn after Run 2's single call needs a summary of Run 1.
const CONTEXT_WINDOW: u64 = 60_000 + 512 + 4096;

fn big_write(id: &str, fill: char) -> Value {
    call(id, "write_answer", json!({"content": fill.to_string().repeat(16 * 1024)}))
}

/// Chat request index of Run 2's only call. A Host steering note queued
/// during it continues the Run after its Stop, so Core must compact with no
/// call left in the Run budget (the agent-turn budget stop never fires
/// without tool results).
const RUN_2_CALL: usize = 2;

struct Case {
    outcome: Arc<TaskOutcome>,
    requests: Vec<Value>,
    rows: Vec<Value>,
    session: Vec<u8>,
}

async fn run(task_max_calls: usize) -> Case {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let mut base = Host::new(&root, &common::Provider::new(vec![]), Fault::None);
    let steering = base.steering.clone();
    let provider = SmallProvider::new(
        CONTEXT_WINDOW,
        vec![
            big_write("write-1", 'a'),
            text("Submitted."),
            text(&"b".repeat(16 * 1024)),
            text("Never requested: the summary could not be reserved."),
        ],
        Box::new(move |index| {
            if index == RUN_2_CALL {
                steering.lock().unwrap().submit_legacy("Host note: also report the file size.".into()).unwrap();
            }
        }),
    );
    base.endpoint = provider.endpoint.clone();
    let host = Arc::new(OneCallContinuation(base));
    let mut spec = host.0.spec("summary");
    spec.max_calls = task_max_calls;
    let handle = start(spec, host.0.request("first"), host.clone()).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(60), handle.wait()).await.unwrap();
    let rows = events(&root.join("summary/run-2-events.jsonl"));
    let session = std::fs::read_dir(&root)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file() && path.to_string_lossy().contains("session"))
        .flat_map(|path| std::fs::read(path).unwrap())
        .collect();
    let requests = provider.requests.lock().unwrap().clone();
    Case { outcome, requests, rows, session }
}

fn assert_no_summary(case: &Case) {
    assert!(case.rows.iter().all(|row| row["event"]["phase"] != "summary"), "no summary reservation");
    assert!(
        case.requests.iter().all(|body| !body.to_string().contains("Summarize the supplied closed conversation")),
        "no summary request reached upstream"
    );
    let stop = case
        .rows
        .iter()
        .find(|row| row["event"]["type"] == "agent_end")
        .and_then(|row| row["event"].get("stop_evidence"))
        .unwrap_or_else(|| {
            let brief: Vec<String> = case
                .rows
                .iter()
                .map(|row| {
                    let event = &row["event"];
                    format!(
                        "{} {} {} {} {}",
                        event["type"], event["phase"], event["status"], event["detail"], event["stop_reason"]
                    )
                })
                .collect();
            panic!("summary stop evidence missing: {brief:#?}")
        });
    assert_eq!(stop["cause"], "summary_budget_exhausted");
    assert!(!String::from_utf8_lossy(&case.session).contains("Concise sourced working notes"));
}

#[tokio::test]
async fn task_budget_exhausted_before_summary_is_a_typed_budget_stop() {
    // Run 1 uses 2 calls, Run 2 its single remaining Task call.
    let case = run(3).await;
    let outcome = &case.outcome;
    assert_eq!(outcome.stop, TaskStop::BudgetExhausted, "{outcome:?}");
    assert!(!outcome.task_accepted);
    assert_eq!(outcome.runs, 2);
    assert_eq!(case.requests.len(), 3, "zero summary upstream requests");
    assert_no_summary(&case);
    assert_eq!(
        outcome.budget_stop,
        Some(BudgetStopDetail { trigger: BudgetStopTrigger::CompactionSummary, task_max_calls: 3, total_calls: 3 })
    );
    assert!(outcome.unknown_effects.is_empty());
}

#[tokio::test]
async fn run_cap_exhausted_before_summary_requires_reconciliation() {
    // The Task has budget left; only the Host's one-call Run cap ran out.
    let case = run(8).await;
    let outcome = &case.outcome;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert!(!outcome.task_accepted);
    assert_eq!(outcome.runs, 2);
    assert_eq!(outcome.observed_calls, 3);
    assert_eq!(case.requests.len(), 3);
    assert_no_summary(&case);
    assert!(outcome.budget_stop.is_none());
}
