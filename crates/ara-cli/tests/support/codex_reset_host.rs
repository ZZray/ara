//! Path-routed HTTP fixture shared by saved-reset controller and real Host
//! families. Usage GETs, consume POSTs and Responses streams do not depend on
//! one global script order. Every credential used by these tests is synthetic.

use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
}

impl Request {
    pub fn account(&self) -> &str {
        self.headers.get("chatgpt-account-id").map(String::as_str).unwrap_or("")
    }
}

#[derive(Default)]
pub struct Gate {
    released: AtomicBool,
    changed: Notify,
}

impl Gate {
    pub fn release(&self) {
        self.released.store(true, Ordering::Release);
        self.changed.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.released.load(Ordering::Acquire) {
                return;
            }
            changed.await;
        }
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub gate: Option<Arc<Gate>>,
    pub retry_after_seconds: Option<u64>,
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Self { status, content_type: "application/json", body: body.to_string(), gate: None, retry_after_seconds: None }
    }

    pub fn malformed() -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: "{invalid fixture JSON".into(),
            gate: None,
            retry_after_seconds: None,
        }
    }

    pub fn text(message: &str) -> Self {
        let events = [
            serde_json::json!({"type":"response.output_item.done","output_index":0,"item":{
                "type":"message","id":"msg_reset_fixture","role":"assistant","content":[{"type":"output_text","text":message}]}}),
            serde_json::json!({"type":"response.completed","response":{"status":"completed"}}),
        ];
        Self {
            status: 200,
            content_type: "text/event-stream",
            body: events.iter().map(|event| format!("data: {event}\n\n")).collect(),
            gate: None,
            retry_after_seconds: None,
        }
    }

    pub fn held(mut self, gate: &Arc<Gate>) -> Self {
        self.gate = Some(gate.clone());
        self
    }

    pub fn retry_after(mut self, seconds: u64) -> Self {
        self.retry_after_seconds = Some(seconds);
        self
    }
}

pub struct ResetFixture {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Request>>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl ResetFixture {
    pub async fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let (seen, stop, handler) = (requests.clone(), cancel.clone(), Arc::new(handler));
        let task = tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::select! { biased; _ = stop.cancelled() => break, accepted = listener.accept() => accepted };
                let Ok((mut socket, _)) = accepted else { break };
                let (seen, stop, handler) = (seen.clone(), stop.clone(), handler.clone());
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket, &stop).await else { return };
                    seen.lock().unwrap().push(request.clone());
                    let reply = handler(&request);
                    if let Some(gate) = reply.gate {
                        tokio::select! { biased; _ = stop.cancelled() => return, _ = gate.wait() => {} }
                    }
                    let wire = format!(
                        "HTTP/1.1 {} Fixture\r\nContent-Type: {}\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n{}",
                        reply.status,
                        reply.content_type,
                        reply.body.len(),
                        reply
                            .retry_after_seconds
                            .map(|seconds| format!("Retry-After: {seconds}\r\n"))
                            .unwrap_or_default(),
                        reply.body
                    );
                    let _ = socket.write_all(wire.as_bytes()).await;
                });
            }
        });
        Self { url, requests, cancel, task }
    }

    pub fn count(&self, method: &str, suffix: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.method == method && request.path.ends_with(suffix))
            .count()
    }

    pub async fn wait_count(&self, method: &str, suffix: &str, count: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while self.count(method, suffix) < count {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("controlled request reached its endpoint");
    }
}

impl Drop for ResetFixture {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

async fn read_request(socket: &mut TcpStream, stop: &CancellationToken) -> Option<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let (header_end, length) = loop {
        let count = tokio::select! { biased; _ = stop.cancelled() => return None, read = socket.read(&mut buffer) => read.ok()? };
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&bytes[..end]);
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                .unwrap_or(0);
            break (end + 4, length);
        }
        assert!(bytes.len() < 65_536, "fixture headers bounded");
    };
    while bytes.len() < header_end + length {
        let count = tokio::select! { biased; _ = stop.cancelled() => return None, read = socket.read(&mut buffer) => read.ok()? };
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let mut words = head.lines().next()?.split_whitespace();
    Some(Request {
        method: words.next()?.to_owned(),
        path: words.next()?.to_owned(),
        headers: head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
            .collect(),
        body: if length == 0 {
            Value::Null
        } else {
            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
        },
    })
}
