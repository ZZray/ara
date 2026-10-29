//! Bidirectional ACP NDJSON JSON-RPC over owned byte streams. Ported from
//! fixed OMP `packages/utils/src/acp/{transport,stream,connection}.ts` and
//! `packages/coding-agent/src/modes/acp/acp-mode.ts`, commit
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Native Session/Job ownership is
//! supplied by the host, not this wire connection.
// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fmt, io,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AcpRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}
impl AcpRpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
    pub fn parse_error() -> Self {
        Self::new(-32700, "Parse error")
    }
    pub fn invalid_request() -> Self {
        Self::new(-32600, "Invalid request")
    }
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("\"Method not found\": {method}")).with_data(json!({"method":method}))
    }
    pub fn invalid_params() -> Self {
        Self::new(-32602, "Invalid params")
    }
    pub fn internal_error() -> Self {
        Self::new(-32603, "Internal error")
    }
}
impl fmt::Display for AcpRpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}
impl std::error::Error for AcpRpcError {}

#[derive(Clone, Debug)]
pub struct RequestFailure {
    /// True after queue admission, even if EOF/write failure prevents a reply.
    /// The caller must not replay an external effect from transport failure.
    pub admitted: bool,
    pub message: String,
    /// Some only for a valid JSON-RPC error reply actually received from peer.
    pub rpc_error: Option<AcpRpcError>,
}
impl RequestFailure {
    fn transport(admitted: bool, message: impl Into<String>) -> Self {
        Self { admitted, message: message.into(), rpc_error: None }
    }
    fn peer(error: AcpRpcError) -> Self {
        Self { admitted: true, message: error.to_string(), rpc_error: Some(error) }
    }
}
impl fmt::Display for RequestFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for RequestFailure {}

#[derive(Debug)]
pub enum Incoming {
    Request { id: Value, method: String, params: Value },
    Notification { method: String, params: Value },
}

struct Outgoing {
    bytes: Vec<u8>,
    receipt: Option<oneshot::Sender<io::Result<()>>>,
}
type PendingReply = oneshot::Sender<Result<Value, RequestFailure>>;
struct State {
    closed: Option<String>,
    pending: HashMap<String, PendingReply>,
}

pub struct AcpConnection {
    state: Mutex<State>,
    output: mpsc::UnboundedSender<Outgoing>,
    next_id: AtomicU64,
    stop: CancellationToken,
}
impl AcpConnection {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn is_closed(&self) -> bool {
        self.state().closed.is_some()
    }
    /// Close pending RPCs before any output/read owner is awaited. Does not
    /// dispose Native Sessions, wait Jobs, retry tools or choose local fallback.
    pub fn close(&self, reason: impl Into<String>) {
        let reason = reason.into();
        let pending = {
            let mut state = self.state();
            if state.closed.is_some() {
                return;
            }
            state.closed = Some(reason.clone());
            std::mem::take(&mut state.pending)
        };
        self.stop.cancel();
        for (_, reply) in pending {
            let _ = reply.send(Err(RequestFailure::transport(true, reason.clone())));
        }
    }
    fn queue(&self, packet: Outgoing) -> io::Result<()> {
        let state = self.state();
        if let Some(reason) = &state.closed {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, reason.clone()));
        }
        self.output.send(packet).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ACP output owner unavailable"))
    }
    async fn send(&self, message: Value) -> io::Result<()> {
        let bytes = frame(&message)?;
        let (receipt, result) = oneshot::channel();
        self.queue(Outgoing { bytes, receipt: Some(receipt) })?;
        // Once admitted, wait for the writer receipt; do not race a caller's
        // cancellation and then retry an unknown partially-written message.
        result.await.map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ACP output receipt owner lost"))?
    }
    pub async fn response(&self, id: Value, result: Result<Value, AcpRpcError>) -> io::Result<()> {
        if !valid_id(&id) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid JSON-RPC response id"));
        }
        let message = match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => json!({"jsonrpc":"2.0","id":id,"error":error}),
        };
        self.send(message).await
    }
    /// Enqueue synchronously behind prior frames; the Native actor can keep
    /// event ordering without spawning a notification task for every event.
    pub fn notify_now(&self, method: &str, params: Value) -> io::Result<()> {
        self.queue(Outgoing { bytes: frame(&json!({"jsonrpc":"2.0","method":method,"params":params}))?, receipt: None })
    }
    pub async fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params})).await
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RequestFailure> {
        let id = Value::from(self.next_id.fetch_add(1, Ordering::Relaxed));
        let key = id.to_string();
        let bytes = frame(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .map_err(|error| RequestFailure::transport(false, error.to_string()))?;
        let (reply, result) = oneshot::channel();
        {
            let mut state = self.state();
            if let Some(reason) = &state.closed {
                return Err(RequestFailure::transport(false, reason.clone()));
            }
            // One admission lock also serializes against close(): a pending
            // record always exists before its bytes become visible to writer.
            state.pending.insert(key.clone(), reply);
            if self.output.send(Outgoing { bytes, receipt: None }).is_err() {
                state.pending.remove(&key);
                return Err(RequestFailure::transport(false, "ACP output owner unavailable"));
            }
        }
        // No request replay, caller-cancel race, or request timeout fallback.
        // EOF/write failure closes and settles this admitted owner as unknown.
        result.await.unwrap_or_else(|_| Err(RequestFailure::transport(true, "ACP reply owner lost")))
    }
    fn error_reply(&self, id: Value, error: AcpRpcError) -> io::Result<()> {
        // Reader must never await writer drain: it continues settling peer
        // replies even while another output frame/handler is waiting.
        self.queue(Outgoing { bytes: frame(&json!({"jsonrpc":"2.0","id":id,"error":error}))?, receipt: None })
    }
    fn receive_response(&self, message: &Value) {
        let Some(id) = message.get("id").filter(|value| valid_id(value)) else {
            return;
        };
        let pending = self.state().pending.remove(&id.to_string());
        let Some(pending) = pending else {
            return;
        }; // Late/unknown ids are ignored.
        let result = if message.get("jsonrpc") == Some(&Value::String("2.0".into()))
            && message.get("result").is_some() != message.get("error").is_some()
        {
            if let Some(error) = message.get("error") {
                match serde_json::from_value::<AcpRpcError>(error.clone()) {
                    Ok(error) => Err(RequestFailure::peer(error)),
                    Err(_) => Err(RequestFailure::transport(true, "Invalid ACP error response; outcome unknown")),
                }
            } else {
                Ok(message["result"].clone())
            }
        } else {
            Err(RequestFailure::transport(true, "Invalid ACP response; outcome unknown"))
        };
        let _ = pending.send(result);
    }
}
impl Drop for AcpConnection {
    fn drop(&mut self) {
        self.close("ACP connection owner dropped");
    }
}

fn frame(message: &Value) -> io::Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    bytes.push(b'\n');
    Ok(bytes)
}
fn stream_whitespace(value: char) -> bool {
    matches!(
        value,
        '\t' | '\u{000b}'
            | '\u{000c}'
            | ' '
            | '\u{00a0}'
            | '\u{feff}'
            | '\n'
            | '\r'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}' | '\u{202f}' | '\u{205f}' | '\u{3000}'
    )
}
fn valid_id(id: &Value) -> bool {
    matches!(id, Value::Null | Value::String(_) | Value::Number(_))
}
fn read_message(
    connection: &AcpConnection,
    incoming: &mpsc::UnboundedSender<Incoming>,
    message: Value,
) -> io::Result<()> {
    if connection.is_closed() {
        return Ok(());
    }
    let Some(object) = message.as_object() else {
        return connection.error_reply(Value::Null, AcpRpcError::invalid_request());
    };
    // A peer response never enters the application request queue, and a peer
    // request id may collide with an outbound id without stealing its reply.
    if !object.contains_key("method") && object.contains_key("id") {
        connection.receive_response(&message);
        return Ok(());
    }
    let id = object.get("id");
    let reply_id = id.filter(|value| valid_id(value)).cloned().unwrap_or(Value::Null);
    let method = object.get("method").and_then(Value::as_str);
    if object.get("jsonrpc") != Some(&Value::String("2.0".into()))
        || method.is_none()
        || id.is_some_and(|value| !valid_id(value))
        || object.contains_key("result")
        || object.contains_key("error")
    {
        return connection.error_reply(reply_id, AcpRpcError::invalid_request());
    }
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    if !matches!(&params, Value::Object(_) | Value::Array(_) | Value::Null) {
        return if id.is_some() { connection.error_reply(reply_id, AcpRpcError::invalid_params()) } else { Ok(()) };
    }
    let method = method.unwrap().to_string();
    let message = match id {
        Some(id) => Incoming::Request { id: id.clone(), method, params },
        None => Incoming::Notification { method, params },
    };
    incoming.send(message).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ACP incoming handler owner dropped"))
}

struct TaskClose {
    connection: Weak<AcpConnection>,
    reason: &'static str,
}
impl Drop for TaskClose {
    fn drop(&mut self) {
        if let Some(connection) = self.connection.upgrade() {
            connection.close(self.reason);
        }
    }
}

async fn reader<R: AsyncRead + Unpin>(
    input: R,
    connection: Weak<AcpConnection>,
    incoming: mpsc::UnboundedSender<Incoming>,
    stop: CancellationToken,
) -> io::Result<()> {
    let _owner = TaskClose { connection: connection.clone(), reason: "ACP input owner ended" };
    let mut input = BufReader::new(input);
    let mut line = Vec::new();
    loop {
        line.clear();
        let read =
            tokio::select! {biased;_=stop.cancelled()=>return Ok(()),read=input.read_until(b'\n',&mut line)=>read};
        let size = match read {
            Ok(size) => size,
            Err(error) => {
                if let Some(connection) = connection.upgrade() {
                    connection.close(format!("ACP input failed: {error}"));
                }
                return Err(error);
            }
        };
        let Some(connection) = connection.upgrade() else {
            return Ok(());
        };
        if size == 0 {
            connection.close("ACP peer EOF");
            return Ok(());
        }
        // Fixed TextDecoder uses replacement for malformed UTF-8. Each
        // newline-delimited line is trimEnd; the final EOF line is trim.
        let decoded = String::from_utf8_lossy(&line);
        let line = if line.ends_with(b"\n") {
            decoded.trim_end_matches(stream_whitespace)
        } else {
            decoded.trim_matches(stream_whitespace)
        };
        if line.is_empty() {
            continue;
        }
        // Fixed stream.ts parseMessage failure errors the stream, and
        // transport.ts closes the connection: never keep serving malformed
        // NDJSON by sending a parse-error reply. Admitted RPCs become unknown.
        let result = match serde_json::from_str::<Value>(line) {
            Ok(message) if message.is_object() && message.get("jsonrpc").and_then(Value::as_str) == Some("2.0") => {
                read_message(&connection, &incoming, message)
            }
            Ok(_) => Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid JSON-RPC message")),
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, format!("Invalid ACP NDJSON: {error}"))),
        };
        if let Err(error) = result {
            connection.close(format!("ACP input routing failed: {error}"));
            return Err(error);
        }
    }
}

async fn writer<W: AsyncWrite + Unpin>(
    mut output: W,
    connection: Weak<AcpConnection>,
    mut packets: mpsc::UnboundedReceiver<Outgoing>,
    stop: CancellationToken,
) -> io::Result<()> {
    let _owner = TaskClose { connection: connection.clone(), reason: "ACP output owner ended" };
    let mut failure = None;
    loop {
        let packet = tokio::select! {biased;_=stop.cancelled()=>break,packet=packets.recv()=>match packet{Some(packet)=>packet,None=>break}};
        let operation = async {
            output.write_all(&packet.bytes).await?;
            output.flush().await
        };
        let result = tokio::select! {biased;_=stop.cancelled()=>Err(io::Error::new(io::ErrorKind::BrokenPipe,"ACP connection closed during output; delivery unknown")),result=operation=>result};
        let failed = result.is_err();
        let was_closed = stop.is_cancelled();
        if let Err(error) = &result {
            if !was_closed {
                if let Some(connection) = connection.upgrade() {
                    connection.close(format!("ACP output failed: {error}"));
                }
                failure = Some(io::Error::new(error.kind(), error.to_string()));
            }
        }
        if let Some(receipt) = packet.receipt {
            let _ = receipt.send(result);
        }
        if failed {
            break;
        }
    }
    packets.close();
    while let Some(packet) = packets.recv().await {
        if let Some(receipt) = packet.receipt {
            let _ = receipt.send(Err(io::Error::new(io::ErrorKind::BrokenPipe, "ACP output closed; delivery unknown")));
        }
    }
    // Graceful owner shutdown is attempted before join's abort fallback.
    if failure.is_none() {
        match tokio::time::timeout(Duration::from_secs(2), output.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => failure = Some(error),
            Err(_) => failure = Some(io::Error::new(io::ErrorKind::TimedOut, "ACP output shutdown timed out")),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct TaskOwner {
    handle: Option<JoinHandle<io::Result<()>>>,
}
impl TaskOwner {
    async fn settle(&mut self) -> io::Result<()> {
        let Some(handle) = self.handle.as_mut() else {
            return Ok(());
        };
        let result = match tokio::time::timeout(Duration::from_secs(3), &mut *handle).await {
            Ok(result) => result.map_err(io::Error::other)?,
            Err(_) => {
                handle.abort();
                let _ = (&mut *handle).await;
                Err(io::Error::new(io::ErrorKind::TimedOut, "ACP transport owner did not settle"))
            }
        };
        self.handle.take();
        result
    }
}
impl Drop for TaskOwner {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

pub struct Transport {
    pub connection: Arc<AcpConnection>,
    pub incoming: mpsc::UnboundedReceiver<Incoming>,
    reader: TaskOwner,
    writer: TaskOwner,
}
impl Transport {
    /// Caller settles host-native jobs separately. Close pending RPCs first,
    /// then await both I/O owners; normal EOF takes the graceful output path.
    pub async fn join(mut self) -> io::Result<()> {
        self.connection.close("ACP transport shutdown");
        let (reader, writer) = tokio::join!(self.reader.settle(), self.writer.settle());
        reader?;
        writer
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        self.connection.close("ACP transport owner dropped");
    }
}

pub async fn start<R, W>(input: R, output: W) -> Transport
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (packets, packet_receiver) = mpsc::unbounded_channel();
    let (incoming, incoming_receiver) = mpsc::unbounded_channel();
    let connection = Arc::new(AcpConnection {
        state: Mutex::new(State { closed: None, pending: HashMap::new() }),
        output: packets,
        next_id: AtomicU64::new(0),
        stop: CancellationToken::new(),
    });
    let reader = tokio::spawn(reader(input, Arc::downgrade(&connection), incoming, connection.stop.clone()));
    let writer = tokio::spawn(writer(output, Arc::downgrade(&connection), packet_receiver, connection.stop.clone()));
    Transport {
        connection,
        incoming: incoming_receiver,
        reader: TaskOwner { handle: Some(reader) },
        writer: TaskOwner { handle: Some(writer) },
    }
}
