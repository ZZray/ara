//! A deterministic HTTP/SSE fixture, never a model reasoning substitute.
use anyhow::{ensure, Result};
use serde_json::{json, Value};
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

pub fn text(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"stop"})
}
pub fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta":{"tool_calls":[{"index":0,"id":id,"type":"function",
        "function":{"name":name,"arguments":args.to_string()}}]},"finish":"tool_calls"})
}

pub struct Provider {
    pub endpoint: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    address: std::net::SocketAddr,
    worker: Option<thread::JoinHandle<()>>,
}
impl Provider {
    pub fn new(replies: Vec<Value>) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let stop = Arc::new(AtomicBool::new(false));
        let done = stop.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let worker = thread::spawn(move || {
            let mut replies = VecDeque::from(replies);
            while !done.load(Ordering::SeqCst) {
                let Ok((mut stream, _)) = listener.accept() else { break };
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
                let _ = stream.set_write_timeout(Some(Duration::from_secs(4)));
                let _ = serve(&mut stream, &mut replies, &observed);
            }
        });
        Ok(Self { endpoint: format!("http://{address}/v1"), requests, stop, address, worker: Some(worker) })
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn serve(stream: &mut TcpStream, replies: &mut VecDeque<Value>, observed: &Mutex<Vec<Value>>) -> Result<()> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    let header_end = loop {
        let count = stream.read(&mut chunk)?;
        ensure!(count > 0 && bytes.len() + count <= 8 * 1024 * 1024, "fixture HTTP bound");
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(index) = bytes.windows(4).position(|item| item == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    if headers.starts_with("GET /v1/models ") {
        let body = json!({"data":[{"id":"capability-controlled","protocols":["openai-completions"],
            "context_window":1000000,"max_tokens":4096,
            "omp":{"supportsTools":true,"input":["text"],"reasoning":false}}]})
        .to_string();
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len())?;
        return Ok(());
    }
    ensure!(headers.starts_with("POST /v1/chat/completions "), "unknown fixture route");
    let length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    ensure!(length <= 8 * 1024 * 1024, "fixture body bound");
    while bytes.len() < header_end + length {
        let count = stream.read(&mut chunk)?;
        ensure!(count > 0, "incomplete fixture body");
        bytes.extend_from_slice(&chunk[..count]);
    }
    observed.lock().unwrap().push(serde_json::from_slice(&bytes[header_end..header_end + length])?);
    let reply = replies.pop_front().unwrap_or_else(|| text("Controlled reply script exhausted; no success claim."));
    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")?;
    for (delta, finish) in [
        (json!({"role":"assistant"}), Value::Null),
        (reply["delta"].clone(), Value::Null),
        (json!({}), reply["finish"].clone()),
    ] {
        let packet = json!({"id":"capability-controlled","object":"chat.completion.chunk",
            "choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        write!(stream, "data: {packet}\n\n")?;
    }
    stream.write_all(b"data: [DONE]\n\n")?;
    Ok(())
}
