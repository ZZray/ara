//! Line-delimited JSON-RPC over a child process and Streamable HTTP.
pub(crate) mod http;
pub(crate) mod stdio;

use serde_json::{json, Value};

/// Largest accepted JSON-RPC frame (one stdio line, one HTTP body or one SSE event).
pub(crate) const FRAME_MAX: usize = 8 * 1024 * 1024;

/// Why a request produced no result.
#[derive(Debug)]
pub(crate) enum CallError {
    /// Never handed to the transport; the server cannot have acted on it.
    NotSent(String),
    Timeout,
    Cancelled,
    /// Connection ended after dispatch and before the reply.
    Lost(String),
    /// The server answered without running the request (JSON-RPC error or HTTP 4xx).
    Rejected(String),
    /// The reply arrived but is not a usable result.
    Protocol(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSent(reason) => write!(f, "request not sent: {reason}"),
            Self::Timeout => write!(f, "request timed out"),
            Self::Cancelled => write!(f, "request cancelled"),
            Self::Lost(reason) => write!(f, "connection lost before reply: {reason}"),
            Self::Rejected(reason) => write!(f, "{reason}"),
            Self::Protocol(reason) => write!(f, "invalid reply: {reason}"),
        }
    }
}

/// Reply to a server→client request. `ping` is answered; sampling, roots,
/// elicitation, tasks and every other method get `-32601`.
pub(crate) fn reply_to_server_request(request: &Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("method").and_then(Value::as_str) == Some("ping") {
        json!({"jsonrpc":"2.0","id":id,"result":{}})
    } else {
        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not found"}})
    }
}

/// Classified incoming JSON-RPC message.
pub(crate) enum Incoming {
    Response(u64, Value),
    Request(Value),
    Other,
}

pub(crate) fn classify(message: Value) -> Incoming {
    let has_method = message.get("method").is_some();
    match message.get("id") {
        Some(_) if has_method => Incoming::Request(message),
        Some(id) if message.get("result").is_some() || message.get("error").is_some() => match id.as_u64() {
            Some(id) => Incoming::Response(id, message),
            None => Incoming::Other,
        },
        _ => Incoming::Other,
    }
}

/// A single message or a JSON-RPC batch.
pub(crate) fn messages(value: Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items,
        other => vec![other],
    }
}
