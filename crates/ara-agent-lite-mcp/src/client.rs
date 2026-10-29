//! JSON-RPC client over one transport: request IDs, timeouts, cancellation
//! with `notifications/cancelled`, `initialize` version negotiation and
//! bounded `tools/list` pagination (requirements 42.3, 42.6, 42.7).
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    config::ServerConfig,
    text::{bounded, Redactor},
    transport::{http::HttpLink, stdio::StdioLink, CallError},
};

/// Protocol versions accepted from the server; the first is requested.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
/// `tools/list` and `resources/list` page bound.
pub const MAX_PAGES: usize = 32;
/// Tools kept per server.
pub const MAX_TOOLS: usize = 512;
/// Longest server `instructions` kept.
pub const MAX_INSTRUCTIONS: usize = 8192;
const SERVER_TEXT_MAX: usize = 1000;

enum Link {
    Stdio(StdioLink),
    Http(HttpLink),
}

pub(crate) struct Client {
    pub(crate) server: String,
    link: Link,
    next_id: AtomicU64,
    pub(crate) capabilities: Value,
    pub(crate) instructions: Option<String>,
    redactor: Redactor,
}

/// `tools/list` outcome.
pub(crate) struct Listing {
    pub(crate) tools: Vec<Value>,
    /// Set when pages or tools were left unread at the bound.
    pub(crate) truncated: Option<String>,
}

impl Client {
    /// Open the transport. No protocol traffic yet.
    pub(crate) fn open(server: &str, config: &ServerConfig, redactor: Redactor) -> Result<Self, String> {
        let link = match config {
            ServerConfig::Stdio { command, args, env } => {
                Link::Stdio(StdioLink::spawn(command, args, env, redactor.clone())?)
            }
            ServerConfig::Http { url, headers } => Link::Http(HttpLink::new(url, headers)?),
        };
        Ok(Self {
            server: server.into(),
            link,
            next_id: AtomicU64::new(1),
            capabilities: Value::Null,
            instructions: None,
            redactor,
        })
    }

    /// stdio diagnostics appended to startup failures.
    pub(crate) async fn diagnostics(&self) -> String {
        match &self.link {
            Link::Stdio(link) => {
                let tail = link.stderr_tail().await;
                if tail.trim().is_empty() {
                    String::new()
                } else {
                    format!("; stderr tail: {}", crate::text::tail(tail.trim(), SERVER_TEXT_MAX))
                }
            }
            Link::Http(_) => String::new(),
        }
    }

    pub(crate) fn has_capability(&self, name: &str) -> bool {
        self.capabilities.get(name).is_some_and(|value| !value.is_null())
    }

    /// One request. `Ok` is the JSON-RPC `result`.
    pub(crate) async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, CallError> {
        if cancel.is_cancelled() {
            return Err(CallError::NotSent("cancelled before dispatch".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        let exchange = async {
            match &self.link {
                Link::Stdio(link) => {
                    let reply = link.register(id);
                    if let Err(reason) = link.send(&message).await {
                        link.unregister(id);
                        return Err(CallError::NotSent(reason));
                    }
                    tokio::select! {
                        biased;
                        reply = reply => reply.map_err(|_| CallError::Lost(link.lost_reason())),
                        _ = link.lost().cancelled() => Err(CallError::Lost(link.lost_reason())),
                    }
                }
                Link::Http(link) => link.request(&message, id).await,
            }
        };
        let outcome = tokio::select! {
            biased;
            outcome = exchange => outcome,
            _ = tokio::time::sleep(timeout) => Err(CallError::Timeout),
            _ = cancel.cancelled() => Err(CallError::Cancelled),
        };
        if matches!(outcome, Err(CallError::Timeout | CallError::Cancelled)) {
            if let Link::Stdio(link) = &self.link {
                link.unregister(id);
            }
            let reason = if matches!(outcome, Err(CallError::Timeout)) { "timeout" } else { "cancelled by host" };
            self.notify("notifications/cancelled", json!({"requestId":id,"reason":reason})).await;
        }
        let reply = outcome?;
        if let Some(error) = reply.get("error") {
            let code = error.get("code").and_then(Value::as_i64).unwrap_or_default();
            let text = error.get("message").and_then(Value::as_str).unwrap_or("no message");
            return Err(CallError::Rejected(format!(
                "MCP error {code}: {}",
                self.redactor.server_text(text, SERVER_TEXT_MAX)
            )));
        }
        match reply.get("result") {
            Some(result @ Value::Object(_)) => Ok(result.clone()),
            _ => Err(CallError::Protocol("result is not an object".into())),
        }
    }

    /// Best-effort notification, bounded in time.
    pub(crate) async fn notify(&self, method: &str, params: Value) {
        let message = json!({"jsonrpc":"2.0","method":method,"params":params});
        let send = async {
            match &self.link {
                Link::Stdio(link) => link.send(&message).await,
                Link::Http(link) => link.notify(&message).await,
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), send).await;
    }

    /// `initialize`, version check, `notifications/initialized`, `tools/list`.
    pub(crate) async fn handshake(&mut self, timeout: Duration, cancel: &CancellationToken) -> Result<Listing, String> {
        let params = json!({
            "protocolVersion": SUPPORTED_PROTOCOL_VERSIONS[0],
            "capabilities": {},
            "clientInfo": {"name": "ara-agent-lite", "version": env!("CARGO_PKG_VERSION")},
        });
        let result =
            self.request("initialize", params, timeout, cancel).await.map_err(|e| format!("initialize failed: {e}"))?;
        let version = result.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
        if !SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
            return Err(format!(
                "unsupported protocol version \"{}\" (supported: {})",
                bounded(&crate::text::sanitize(version), 40),
                SUPPORTED_PROTOCOL_VERSIONS.join(", ")
            ));
        }
        if let Link::Http(link) = &self.link {
            link.set_version(version);
        }
        self.capabilities = result.get("capabilities").filter(|value| value.is_object()).cloned().unwrap_or(json!({}));
        self.instructions = result
            .get("instructions")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(|text| self.redactor.server_text(text, MAX_INSTRUCTIONS));
        self.notify("notifications/initialized", json!({})).await;
        if !self.has_capability("tools") {
            return Ok(Listing { tools: Vec::new(), truncated: None });
        }
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = cursor.take().map_or(json!({}), |cursor| json!({"cursor":cursor}));
            let page = self
                .request("tools/list", params, timeout, cancel)
                .await
                .map_err(|e| format!("tools/list failed: {e}"))?;
            let items = page.get("tools").and_then(Value::as_array).ok_or("tools/list result has no tools array")?;
            for item in items {
                if tools.len() == MAX_TOOLS {
                    return Ok(Listing { tools, truncated: Some(format!("tool count limit {MAX_TOOLS} reached")) });
                }
                tools.push(item.clone());
            }
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) => cursor = Some(next.to_string()),
                None => return Ok(Listing { tools, truncated: None }),
            }
        }
        Ok(Listing { tools, truncated: Some(format!("tools/list page limit {MAX_PAGES} reached")) })
    }

    pub(crate) async fn shutdown(&self) {
        match &self.link {
            Link::Stdio(link) => link.shutdown().await,
            Link::Http(link) => link.shutdown().await,
        }
    }
}
