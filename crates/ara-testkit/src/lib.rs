//! Controlled fake OpenAI-compatible upstream for deterministic fault tests.
//!
//! A script is a list of responses served in order, one per HTTP request:
//!
//! ```json
//! {"responses": [
//!   {"status": 429, "headers": {"retry-after": "0"}, "body": "{\"error\":{\"message\":\"slow\"}}"},
//!   {"events": [
//!     {"data": {"choices": [{"delta": {"content": "hi"}}]}},
//!     {"sleep_ms": 50},
//!     {"raw": ": keep-alive\n\n"},
//!     {"data": {"choices": [{"delta": {}, "finish_reason": "stop"}]}},
//!     {"done": true}
//!   ]},
//!   {"events": [{"data": {"choices": [{"delta": {"content": "x"}}]}}], "end": "hang"},
//!   {"events": [], "end": "drop"}
//! ]}
//! ```
//!
//! `end`: `close` (default, graceful EOF), `hang` (keep the socket open until
//! the client disconnects), `drop` (reset without a terminal frame).
//! Every request is appended to the record log with the `authorization`
//! header redacted.

use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

#[derive(Debug, Clone, Deserialize)]
pub struct Script {
    pub responses: Vec<ScriptedResponse>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ScriptedResponse {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub headers: serde_json::Map<String, Value>,
    /// Non-streaming body (used when `events` is absent).
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub events: Option<Vec<ScriptedEvent>>,
    #[serde(default)]
    pub end: Option<String>,
    /// Delay before sending response headers.
    #[serde(default)]
    pub delay_ms: Option<u64>,
}

fn default_status() -> u16 {
    200
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ScriptedEvent {
    Data { data: Value },
    Raw { raw: String },
    Sleep { sleep_ms: u64 },
    Done { done: bool },
}

pub struct FakeUpstream {
    pub addr: std::net::SocketAddr,
    pub requests: Arc<Mutex<Vec<Value>>>,
    served: Arc<AtomicUsize>,
    task: tokio::task::JoinHandle<()>,
}

impl FakeUpstream {
    /// Bind 127.0.0.1:0 and serve `script`; optionally append requests to `record`.
    pub async fn start(script: Script, record: Option<PathBuf>) -> anyhow::Result<FakeUpstream> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let script = Arc::new(script);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let served = Arc::new(AtomicUsize::new(0));
        let (req2, served2) = (requests.clone(), served.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else { break };
                let (script, requests, served, record) =
                    (script.clone(), req2.clone(), served2.clone(), record.clone());
                tokio::spawn(async move {
                    let _ = handle(socket, script, requests, served, record).await;
                });
            }
        });
        Ok(FakeUpstream { addr, requests, served, task })
    }

    pub fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }

    pub fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(socket: &mut TcpStream) -> anyhow::Result<Option<(String, Vec<(String, String)>, Vec<u8>)>> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let header_end = loop {
        let n = socket.read(&mut tmp).await?;
        if n == 0 {
            return Ok(None);
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    let len =
        headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse::<usize>().ok()).unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < len {
        let n = socket.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Ok(Some((request_line, headers, body)))
}

fn redact(headers: &[(String, String)]) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in headers {
        let value = if k == "authorization" { format!("<redacted {} chars>", v.len()) } else { v.clone() };
        map.insert(k.clone(), Value::String(value));
    }
    Value::Object(map)
}

async fn handle(
    mut socket: TcpStream,
    script: Arc<Script>,
    requests: Arc<Mutex<Vec<Value>>>,
    served: Arc<AtomicUsize>,
    record: Option<PathBuf>,
) -> anyhow::Result<()> {
    let Some((request_line, headers, body)) = read_request(&mut socket).await? else { return Ok(()) };
    let index = served.fetch_add(1, Ordering::SeqCst);
    let body_json: Value =
        serde_json::from_slice(&body).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into()));
    let entry = json!({"index": index, "request": request_line, "headers": redact(&headers), "body": body_json});
    if let Some(path) = &record {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{entry}");
        }
    }
    requests.lock().await.push(entry);

    let Some(resp) = script.responses.get(index).cloned() else {
        let msg = b"{\"error\":{\"message\":\"fake upstream script exhausted\"}}";
        let head = format!(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            msg.len()
        );
        socket.write_all(head.as_bytes()).await?;
        socket.write_all(msg).await?;
        return Ok(());
    };
    if let Some(ms) = resp.delay_ms {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }
    let mut head = format!("HTTP/1.1 {} Scripted\r\nconnection: close\r\n", resp.status);
    for (k, v) in &resp.headers {
        head.push_str(&format!("{k}: {}\r\n", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())));
    }
    let Some(events) = resp.events else {
        let body = resp.body.unwrap_or_default();
        head.push_str(&format!("content-type: application/json\r\ncontent-length: {}\r\n\r\n", body.len()));
        socket.write_all(head.as_bytes()).await?;
        socket.write_all(body.as_bytes()).await?;
        return Ok(());
    };
    head.push_str("content-type: text/event-stream\r\ncache-control: no-cache\r\n\r\n");
    socket.write_all(head.as_bytes()).await?;
    socket.flush().await?;
    for event in events {
        match event {
            ScriptedEvent::Data { data } => socket.write_all(format!("data: {data}\n\n").as_bytes()).await?,
            ScriptedEvent::Raw { raw } => socket.write_all(raw.as_bytes()).await?,
            ScriptedEvent::Sleep { sleep_ms } => tokio::time::sleep(std::time::Duration::from_millis(sleep_ms)).await,
            ScriptedEvent::Done { .. } => socket.write_all(b"data: [DONE]\n\n").await?,
        }
        socket.flush().await?;
    }
    match resp.end.as_deref() {
        Some("hang") => {
            let mut sink = [0u8; 64];
            // Wait until the client goes away.
            while socket.read(&mut sink).await.map(|n| n > 0).unwrap_or(false) {}
        }
        Some("drop") => {
            // Zero linger sends RST on drop without blocking (the deprecation is about non-zero linger).
            #[allow(deprecated)]
            let _ = socket.set_linger(Some(std::time::Duration::from_secs(0)));
            drop(socket);
        }
        _ => {
            socket.shutdown().await.ok();
        }
    }
    Ok(())
}

/// Convenience builders for Chat Completions chunks.
pub mod chunks {
    use serde_json::{Value, json};

    pub fn text(content: &str) -> Value {
        json!({"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {"content": content}}]}})
    }
    pub fn tool_call(index: u64, id: &str, name: &str, args: &str) -> Value {
        json!({"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {"tool_calls": [{"index": index, "id": id, "type": "function", "function": {"name": name, "arguments": args}}]}}]}})
    }
    pub fn finish(reason: &str) -> Value {
        json!({"data": {"id": "fake-1", "choices": [{"index": 0, "delta": {}, "finish_reason": reason}]}})
    }
    pub fn usage(prompt: u64, completion: u64) -> Value {
        json!({"data": {"id": "fake-1", "choices": [], "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion}}})
    }
    pub fn done() -> Value {
        json!({"done": true})
    }
}
