//! HTTP client for the local service (Go: `call`, `callWait`, `callWaitContext`).
//!
//! Like the Go client it never uses a proxy and never follows redirects: a bearer
//! credential must only ever go to the loopback address written in the profile.
//! A plain `TcpStream` request makes both properties structural rather than
//! configuration.

use crate::err;
use crate::error::{Error, Result};
use crate::profile::loopback_addr;
use crate::snapshot::git_snapshot;
use crate::types::{Connection, Request, Task};
use crate::util::{format_wait, random_hex};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RESPONSE_LIMIT: usize = 16 << 20;
const READ_SLICE: Duration = Duration::from_millis(250);
const SLACK: Duration = Duration::from_secs(12);

/// Lets another thread abort an in-flight request (Go: `context.Context`).
#[derive(Debug, Default)]
pub struct CancelHandle {
    cancelled: AtomicBool,
    stream: Mutex<Option<TcpStream>>,
}

impl CancelHandle {
    pub fn new() -> Arc<CancelHandle> {
        Arc::new(CancelHandle::default())
    }

    /// Mark cancelled and shut down the socket so the server sees the hang-up now.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let stream = self.stream.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(stream) = stream {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    fn attach(&self, stream: &TcpStream) {
        if let Ok(clone) = stream.try_clone() {
            *self.stream.lock().unwrap_or_else(|e| e.into_inner()) = Some(clone);
        }
    }

    fn detach(&self) {
        *self.stream.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    result: Option<Box<RawValue>>,
    #[serde(default)]
    error: String,
}

fn cancelled_error() -> Error {
    Error::new("request cancelled")
}

fn unavailable(e: impl std::fmt::Display) -> Error {
    err!("service unavailable: {e}; check the serve console")
}

fn read_slice(
    stream: &mut TcpStream,
    buf: &mut [u8],
    deadline: Instant,
    cancel: Option<&CancelHandle>,
) -> Result<usize> {
    let cancelled = || cancel.is_some_and(|c| c.is_cancelled());
    loop {
        if cancelled() {
            return Err(cancelled_error());
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(unavailable("request timed out"));
        }
        stream.set_read_timeout(Some(remaining.min(READ_SLICE)))?;
        match stream.read(buf) {
            Ok(n) => {
                if n == 0 && cancelled() {
                    return Err(cancelled_error());
                }
                return Ok(n);
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted) => {
                continue;
            }
            Err(_) if cancelled() => return Err(cancelled_error()),
            Err(e) => return Err(unavailable(e)),
        }
    }
}

/// Decode a complete chunked body; `None` while more data is needed.
fn decode_chunked(data: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut out = Vec::new();
    let mut pos = 0;
    loop {
        let Some(line_end) = data[pos..].windows(2).position(|w| w == b"\r\n") else { return Ok(None) };
        let size_text =
            std::str::from_utf8(&data[pos..pos + line_end]).map_err(|_| Error::new("invalid chunk size"))?;
        let size_text = size_text.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| Error::new("invalid chunk size"))?;
        pos += line_end + 2;
        if size == 0 {
            return Ok(Some(out));
        }
        if data.len() < pos + size + 2 {
            return Ok(None);
        }
        out.extend_from_slice(&data[pos..pos + size]);
        pos += size + 2;
        if out.len() > RESPONSE_LIMIT {
            return Err(Error::new("response exceeds 16 MiB"));
        }
    }
}

/// Read a full HTTP response; returns the status code and body.
fn read_response(stream: &mut TcpStream, deadline: Instant, cancel: Option<&CancelHandle>) -> Result<(u16, Vec<u8>)> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 64 << 10 {
            return Err(Error::new("response header too large"));
        }
        let n = read_slice(stream, &mut chunk, deadline, cancel)?;
        if n == 0 {
            return Err(unavailable("connection closed before a response"));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| err!("malformed response status line: {status_line:?}"))?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().ok(),
                "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
                _ => {}
            }
        }
    }
    let mut body: Vec<u8> = buf[head_end + 4..].to_vec();
    loop {
        if chunked {
            if let Some(decoded) = decode_chunked(&body)? {
                return Ok((status, decoded));
            }
        } else if let Some(l) = length
            && body.len() >= l
        {
            body.truncate(l);
            return Ok((status, body));
        }
        if body.len() > RESPONSE_LIMIT {
            return Err(Error::new("response exceeds 16 MiB"));
        }
        let n = read_slice(stream, &mut chunk, deadline, cancel)?;
        if n == 0 {
            if chunked || length.is_some() {
                return Err(unavailable("connection closed mid-response"));
            }
            return Ok((status, body));
        }
        body.extend_from_slice(&chunk[..n]);
    }
}

/// Send one request and return the raw `result` JSON (Go: `callWaitContext`).
/// `wait` is the server-side long-poll budget; the client allows 12 s more.
pub fn call_wait(c: &Connection, r: &Request, wait: Duration, cancel: Option<&CancelHandle>) -> Result<Box<RawValue>> {
    let payload = serde_json::to_vec(r)?;
    let addr = loopback_addr(&c.url)?;
    let mut path = String::from("/api");
    if !wait.is_zero() || r.op == "poll" {
        path.push_str("?timeout=");
        path.push_str(&format_wait(wait));
    }
    let deadline = Instant::now() + wait + SLACK;
    if cancel.is_some_and(|h| h.is_cancelled()) {
        return Err(cancelled_error());
    }
    let mut stream = TcpStream::connect_timeout(&addr, SLACK.min(Duration::from_secs(10))).map_err(unavailable)?;
    let _ = stream.set_nodelay(true);
    if let Some(h) = cancel {
        h.attach(&stream);
        // A cancel that raced with attach must still close the socket.
        if h.is_cancelled() {
            let _ = stream.shutdown(Shutdown::Both);
            return Err(cancelled_error());
        }
    }
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        c.token,
        payload.len()
    );
    let exchange = (|| -> Result<(u16, Vec<u8>)> {
        stream.set_write_timeout(Some(SLACK)).map_err(unavailable)?;
        stream.write_all(head.as_bytes()).map_err(unavailable)?;
        stream.write_all(&payload).map_err(unavailable)?;
        read_response(&mut stream, deadline, cancel)
    })();
    if let Some(h) = cancel {
        h.detach();
    }
    let (status, body) = exchange?;
    let envelope: Envelope = serde_json::from_slice(&body)?;
    if status != 200 || !envelope.error.is_empty() {
        return Err(err!("request rejected ({status}): {}", envelope.error));
    }
    envelope.result.ok_or_else(|| Error::new("response has no result"))
}

/// Non-waiting call (Go: `call`).
pub fn call(c: &Connection, r: &Request) -> Result<Box<RawValue>> {
    call_wait(c, r, Duration::ZERO, None)
}

/// Fresh 128-bit retry ID for a mutation (Go: the `rand.Read` block in `clientCommand`).
pub fn new_request_id() -> Result<String> {
    random_hex(16)
}

/// Read one task through the `tasks` operation (used before `submit` and `review`).
pub fn fetch_task(c: &Connection, task_id: &str) -> Result<Task> {
    let raw = call(c, &Request { op: "tasks".into(), ..Request::default() })?;
    let tasks: Vec<Task> = serde_json::from_str(raw.get())?;
    tasks.into_iter().find(|t| t.id == task_id).ok_or_else(|| Error::new("task not found"))
}

/// Snapshot evidence for `submit` and `review`: the live Git state of the task workspace.
/// For a review, `given` (captured before the review) must still equal the live state.
pub fn task_snapshot(c: &Connection, task_id: &str, given: Option<&str>) -> Result<String> {
    let task = fetch_task(c, task_id)?;
    let live = git_snapshot(Path::new(&task.workspace))?;
    if let Some(given) = given
        && (given.is_empty() || given != live)
    {
        return Err(Error::new("workspace changed or --snapshot missing; review the current submission again"));
    }
    Ok(live)
}

/// Call and decode `result` into `T`.
pub fn call_typed<T: DeserializeOwned>(
    c: &Connection,
    r: &Request,
    wait: Duration,
    cancel: Option<&CancelHandle>,
) -> Result<T> {
    let raw = call_wait(c, r, wait, cancel)?;
    Ok(serde_json::from_str(raw.get())?)
}
