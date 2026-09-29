//! HTTP front end (Go: `apiHandler` and the `http.Server` settings in `main.go`).
//!
//! This is a deliberately small HTTP/1.1 server over `std::net`: one thread per
//! connection, `Connection: close`, no keep-alive, no TLS. A hand-written server is
//! used instead of a framework because the service needs (a) many concurrently
//! blocked long polls, (b) prompt detection of a client that hung up mid-poll, and
//! (c) an exact, auditable set of accepted requests. The parser and writer are
//! exposed (hidden from docs) so tests can build mock upstreams with the same code.

use crate::store::Store;
use crate::types::*;
use crate::util::{parse_go_duration, query_value};
use serde::Serialize;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_BODY: usize = 1 << 20;
const MAX_HEAD: usize = 64 << 10;
const HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(15);
const WRITE_TIMEOUT: Duration = Duration::from_secs(40);
const MAX_POLL: Duration = Duration::from_secs(30);
const POLL_SLICE: Duration = Duration::from_millis(250);
const MAX_HANDLERS: usize = 256;

/// One parsed request. Header names are lowercased.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub enum ReadError {
    /// Peer closed before sending anything.
    Closed,
    Timeout,
    Malformed(&'static str),
    TooLarge,
    /// `Transfer-Encoding` request bodies are not supported.
    Unsupported,
    Io(std::io::Error),
}

fn read_some(stream: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> Result<usize, ReadError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ReadError::Timeout);
    }
    stream.set_read_timeout(Some(remaining)).map_err(ReadError::Io)?;
    loop {
        match stream.read(buf) {
            Ok(n) => return Ok(n),
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return Err(ReadError::Timeout);
            }
            Err(e) => return Err(ReadError::Io(e)),
        }
    }
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Read one request. The header block must arrive by `header_deadline` and the whole
/// request by `read_deadline`.
#[doc(hidden)]
pub fn read_request(
    stream: &mut TcpStream,
    header_deadline: Instant,
    read_deadline: Instant,
) -> Result<HttpRequest, ReadError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_HEAD {
            return Err(ReadError::TooLarge);
        }
        let n = read_some(stream, &mut chunk, header_deadline)?;
        if n == 0 {
            return Err(if buf.is_empty() { ReadError::Closed } else { ReadError::Malformed("truncated request") });
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).map_err(|_| ReadError::Malformed("request head is not UTF-8"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split(' ');
    let (method, target, version) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(m), Some(t), Some(v), None) if !m.is_empty() && !t.is_empty() => (m, t, v),
        _ => return Err(ReadError::Malformed("malformed request line")),
    };
    if !version.starts_with("HTTP/1.") {
        return Err(ReadError::Malformed("unsupported HTTP version"));
    }
    let mut headers = Vec::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(ReadError::Malformed("malformed header"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    let mut request = HttpRequest { method: method.to_string(), path, query, headers, body: Vec::new() };
    if request.header("transfer-encoding").is_some() {
        return Err(ReadError::Unsupported);
    }
    let length = match request.header("content-length") {
        None => 0,
        Some(v) => v.parse::<usize>().map_err(|_| ReadError::Malformed("invalid Content-Length"))?,
    };
    if length > MAX_BODY {
        return Err(ReadError::TooLarge);
    }
    let mut body = buf[head_end + 4..].to_vec();
    body.truncate(length);
    while body.len() < length {
        let want = (length - body.len()).min(chunk.len());
        let n = read_some(stream, &mut chunk[..want], read_deadline)?;
        if n == 0 {
            return Err(ReadError::Malformed("truncated request body"));
        }
        body.extend_from_slice(&chunk[..n]);
    }
    request.body = body;
    Ok(request)
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        409 => "Conflict",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Write a complete JSON response and mark the connection as closing.
#[doc(hidden)]
pub fn write_response(stream: &mut TcpStream, status: u16, no_store: bool, body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\n", reason(status));
    if no_store {
        head.push_str("Cache-Control: no-store\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", body.len()));
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

#[doc(hidden)]
pub fn error_body(message: &str) -> Vec<u8> {
    let mut out = serde_json::to_vec(&serde_json::json!({ "error": message })).unwrap_or_default();
    out.push(b'\n');
    out
}

/// Everything `POST /api` can return in `result`.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ApiResult {
    Outcome(Outcome),
    Poll(PollResult),
    Ack(AckResult),
    Message(Message),
    History(Vec<Message>),
}

#[derive(Serialize)]
struct Envelope<'a> {
    result: &'a ApiResult,
}

/// Route one operation to the store (Go: the `switch req.Op` in `apiHandler`).
pub fn dispatch(store: &Store, token: &str, req: &Request) -> crate::Result<ApiResult> {
    Ok(match req.op.as_str() {
        "send" => ApiResult::Message(store.send_chat(token, req)?),
        "poll" => ApiResult::Poll(store.poll_messages(token, req.after, req.limit)?),
        "ack" => ApiResult::Ack(store.ack_messages(token, req.expected_cursor, req.ack_seq)?),
        "history" => ApiResult::History(store.message_history(token, &req.task_id, &req.worker_id, req.limit)?),
        _ => ApiResult::Outcome(store.execute(token, req)?),
    })
}

/// True when the peer has closed or reset its side of the connection.
fn peer_gone(stream: &TcpStream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return true;
    }
    let mut probe = [0u8; 1];
    let gone = match stream.peek(&mut probe) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != ErrorKind::WouldBlock,
    };
    let _ = stream.set_nonblocking(false);
    gone
}

fn poll_wait(req: &Request, query: &str) -> Result<Duration, ()> {
    if req.op != "poll" {
        return Ok(Duration::ZERO);
    }
    match query_value(query, "timeout").filter(|v| !v.is_empty()) {
        None => Ok(MAX_POLL),
        Some(value) => {
            let nanos = parse_go_duration(&value).map_err(|_| ())?;
            let wait = crate::util::duration_from_nanos(nanos).ok_or(())?;
            if wait > MAX_POLL { Err(()) } else { Ok(wait) }
        }
    }
}

/// Handle `POST /api`. `None` means the client is gone and nothing should be written.
fn handle_api(request: &HttpRequest, stream: &TcpStream, store: &Store, stop: &AtomicBool) -> Option<(u16, Vec<u8>)> {
    let fail = |status: u16, message: &str| Some((status, error_body(message)));
    if request.header("origin").is_some_and(|v| !v.is_empty()) {
        return fail(403, "browser requests are not supported; use ara-lite CLI");
    }
    let mut de = serde_json::Deserializer::from_slice(&request.body);
    let req = match <Request as serde::Deserialize>::deserialize(&mut de) {
        Ok(r) => r,
        Err(e) => return fail(400, &e.to_string()),
    };
    if de.end().is_err() {
        return fail(400, "expected one JSON request");
    }
    let token = match request.header("authorization").and_then(|v| v.strip_prefix("Bearer ")) {
        Some(t) => t.to_string(),
        None => return fail(401, "missing client credential"),
    };
    let wait = match poll_wait(&req, &request.query) {
        Ok(w) => w,
        Err(()) => return fail(400, "poll timeout must be 0..30s"),
    };
    let deadline = Instant::now() + wait;
    loop {
        // Capture the change generation before reading so a change during the read
        // wakes the wait below instead of being lost.
        let seen = store.epoch();
        let result = match dispatch(store, &token, &req) {
            Ok(r) => r,
            Err(e) => return fail(409, e.message()),
        };
        let done = |result: &ApiResult| {
            let mut body = serde_json::to_vec(&Envelope { result }).unwrap_or_default();
            body.push(b'\n');
            Some((200, body))
        };
        let ApiResult::Poll(poll) = &result else { return done(&result) };
        if wait.is_zero() || !poll.messages.is_empty() || poll.retention_gap {
            return done(&result);
        }
        loop {
            if stop.load(Ordering::SeqCst) {
                return None;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return done(&result);
            }
            if peer_gone(stream) {
                return None;
            }
            if store.wait_change(seen, remaining.min(POLL_SLICE)) {
                break;
            }
        }
    }
}

fn route(request: &HttpRequest, stream: &TcpStream, store: &Store, stop: &AtomicBool) -> Option<(u16, Vec<u8>)> {
    match (request.method.as_str(), request.path.as_str()) {
        ("GET" | "HEAD", "/health") => Some((200, b"{\"status\":\"ready\"}".to_vec())),
        ("POST", "/api") => handle_api(request, stream, store, stop),
        (_, "/api" | "/health") => Some((405, error_body("method not allowed"))),
        _ => Some((404, error_body("not found"))),
    }
}

/// Close politely: stop sending, then drain a little so the peer sees the response
/// rather than a reset when it was still writing.
fn finish(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
    let mut sink = [0u8; 4096];
    for _ in 0..1024 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
    }
}

fn serve_connection(mut stream: TcpStream, store: &Store, stop: &AtomicBool) {
    let _ = stream.set_nodelay(true);
    let start = Instant::now();
    let request = match read_request(&mut stream, start + HEADER_TIMEOUT, start + READ_TIMEOUT) {
        Ok(r) => r,
        Err(ReadError::Closed) | Err(ReadError::Io(_)) => return,
        Err(e) => {
            let (status, message) = match e {
                ReadError::Timeout => (408, "request timed out"),
                ReadError::TooLarge => (400, "request too large"),
                ReadError::Unsupported => (411, "Transfer-Encoding is not supported; send Content-Length"),
                ReadError::Malformed(m) => (400, m),
                ReadError::Closed | ReadError::Io(_) => return,
            };
            let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
            let _ = write_response(&mut stream, status, true, &error_body(message));
            finish(&mut stream);
            return;
        }
    };
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    if let Some((status, body)) = route(&request, &stream, store, stop) {
        let body = if request.method == "HEAD" { Vec::new() } else { body };
        let _ = write_response(&mut stream, status, request.path == "/api", &body);
        finish(&mut stream);
    }
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A running HTTP listener bound to one store.
pub struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    accept: Option<JoinHandle<()>>,
}

impl Server {
    pub fn start(listener: TcpListener, store: Arc<Store>) -> crate::Result<Server> {
        let addr = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let active = Arc::new(AtomicUsize::new(0));
        let accept = {
            let (stop, active) = (stop.clone(), active.clone());
            thread::Builder::new().name("ara-lite-accept".into()).spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    let stream = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(e) if e.kind() == ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(_) => {
                            thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                    };
                    // An accepted socket may inherit the listener's non-blocking mode.
                    let _ = stream.set_nonblocking(false);
                    if active.fetch_add(1, Ordering::SeqCst) >= MAX_HANDLERS {
                        active.fetch_sub(1, Ordering::SeqCst);
                        let mut stream = stream;
                        let _ = write_response(&mut stream, 503, true, &error_body("too many concurrent requests"));
                        continue;
                    }
                    let guard = ActiveGuard(active.clone());
                    let (store, stop) = (store.clone(), stop.clone());
                    let spawned = thread::Builder::new().name("ara-lite-conn".into()).spawn(move || {
                        let _guard = guard;
                        serve_connection(stream, &store, &stop);
                    });
                    // On spawn failure the closure (and its guard) is dropped, releasing the slot.
                    drop(spawned);
                }
            })?
        };
        Ok(Server { addr, stop, active, accept: Some(accept) })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stop accepting, abandon blocked polls, and wait briefly for handlers to end.
    pub fn shutdown(mut self) {
        self.stop_inner();
    }

    fn stop_inner(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.active.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop_inner();
    }
}
