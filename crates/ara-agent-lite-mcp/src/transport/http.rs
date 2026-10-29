//! Streamable HTTP transport: each request is one POST whose reply is a JSON
//! body or an SSE stream. `Mcp-Session-Id` is stored and sent back, and the
//! negotiated version goes in `MCP-Protocol-Version`. The optional GET stream
//! for unsolicited server messages is not opened (no `listChanged` support).
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

use futures::StreamExt;
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE},
    StatusCode,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::{classify, messages, reply_to_server_request, CallError, Incoming, FRAME_MAX};

const SESSION: &str = "mcp-session-id";
const VERSION: &str = "mcp-protocol-version";
const SIDE_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub(crate) struct HttpLink {
    client: reqwest::Client,
    url: Url,
    headers: HeaderMap,
    session: Mutex<Option<HeaderValue>>,
    version: Mutex<Option<HeaderValue>>,
    lost: CancellationToken,
}

impl HttpLink {
    pub(crate) fn new(url: &str, headers: &BTreeMap<String, String>) -> Result<Self, String> {
        let url = Url::parse(url).map_err(|_| "invalid url".to_string())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("url scheme must be http or https".into());
        }
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let header = HeaderName::from_bytes(name.as_bytes())
                .map_err(|_| format!("invalid header name \"{}\"", crate::text::bounded(name, 64)))?;
            let mut value = HeaderValue::from_str(value)
                .map_err(|_| format!("invalid value for header \"{}\"", crate::text::bounded(name, 64)))?;
            value.set_sensitive(true);
            map.insert(header, value);
        }
        let loopback = match url.host() {
            Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        if loopback {
            builder = builder.no_proxy();
        }
        let client = builder.build().map_err(|_| "HTTP client setup failed".to_string())?;
        Ok(Self {
            client,
            url,
            headers: map,
            session: Mutex::new(None),
            version: Mutex::new(None),
            lost: CancellationToken::new(),
        })
    }

    pub(crate) fn set_version(&self, version: &str) {
        *self.version.lock().unwrap() = HeaderValue::from_str(version).ok();
    }

    fn post(&self, body: &Value) -> reqwest::RequestBuilder {
        let mut request = self
            .client
            .post(self.url.clone())
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(body.to_string());
        if let Some(session) = self.session.lock().unwrap().clone() {
            request = request.header(SESSION, session);
        }
        if let Some(version) = self.version.lock().unwrap().clone() {
            request = request.header(VERSION, version);
        }
        request
    }

    fn remember_session(&self, headers: &HeaderMap) {
        if let Some(value) = headers.get(SESSION).filter(|value| value.to_str().is_ok()) {
            *self.session.lock().unwrap() = Some(value.clone());
        }
    }

    /// POST a notification or a reply; the server answers 202.
    pub(crate) async fn notify(&self, message: &Value) -> Result<(), String> {
        if self.lost.is_cancelled() {
            return Err("session closed".into());
        }
        let response = self.post(message).send().await.map_err(|_| "notification not delivered".to_string())?;
        self.remember_session(response.headers());
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("notification rejected with HTTP {}", response.status().as_u16()))
        }
    }

    async fn answer(&self, request: &Value) {
        let reply = reply_to_server_request(request);
        let _ = tokio::time::timeout(SIDE_REQUEST_TIMEOUT, self.notify(&reply)).await;
    }

    /// One request; returns the JSON-RPC response message carrying `id`.
    pub(crate) async fn request(&self, message: &Value, id: u64) -> Result<Value, CallError> {
        if self.lost.is_cancelled() {
            return Err(CallError::NotSent("session closed".into()));
        }
        let response = match self.post(message).send().await {
            Ok(response) => response,
            Err(error) if error.is_connect() || error.is_builder() => {
                return Err(CallError::NotSent("could not connect to server".into()))
            }
            Err(_) => return Err(CallError::Lost("HTTP request failed after sending".into())),
        };
        self.remember_session(response.headers());
        let status = response.status();
        if status == StatusCode::NOT_FOUND && self.session.lock().unwrap().is_some() {
            self.lost.cancel();
            return Err(CallError::Lost("HTTP session expired (404)".into()));
        }
        if status.is_client_error() {
            return Err(CallError::Rejected(format!("server rejected the request with HTTP {}", status.as_u16())));
        }
        if !status.is_success() {
            return Err(CallError::Lost(format!("server returned HTTP {}", status.as_u16())));
        }
        let kind = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mut stream = response.bytes_stream();
        if kind.starts_with("application/json") {
            let mut body = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| CallError::Lost("HTTP body ended early".into()))?;
                body.extend_from_slice(&chunk);
                if body.len() > FRAME_MAX {
                    return Err(CallError::Protocol("HTTP body exceeds size limit".into()));
                }
            }
            let value: Value =
                serde_json::from_slice(&body).map_err(|_| CallError::Protocol("HTTP body is not JSON".into()))?;
            let mut found = None;
            for message in messages(value) {
                match classify(message) {
                    Incoming::Response(reply, message) if reply == id => found = Some(message),
                    Incoming::Request(request) => self.answer(&request).await,
                    _ => {}
                }
            }
            return found.ok_or_else(|| CallError::Protocol("HTTP body has no reply for the request".into()));
        }
        if !kind.starts_with("text/event-stream") {
            return Err(CallError::Protocol("unexpected HTTP content type".into()));
        }
        let mut buffer = Vec::<u8>::new();
        let mut data = String::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| CallError::Lost("SSE stream broke before reply".into()))?;
            buffer.extend_from_slice(&chunk);
            while let Some(end) = buffer.iter().position(|&b| b == b'\n') {
                let raw: Vec<u8> = buffer.drain(..=end).collect();
                let line = String::from_utf8_lossy(&raw);
                let line = line.trim_end_matches(['\n', '\r']);
                if line.is_empty() {
                    let event = std::mem::take(&mut data);
                    let Ok(value) = serde_json::from_str::<Value>(&event) else { continue };
                    for message in messages(value) {
                        match classify(message) {
                            Incoming::Response(reply, message) if reply == id => return Ok(message),
                            Incoming::Request(request) => self.answer(&request).await,
                            _ => {}
                        }
                    }
                } else if let Some(field) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(field.strip_prefix(' ').unwrap_or(field));
                }
                if data.len() > FRAME_MAX {
                    return Err(CallError::Protocol("SSE event exceeds size limit".into()));
                }
            }
            if buffer.len() > FRAME_MAX {
                return Err(CallError::Protocol("SSE line exceeds size limit".into()));
            }
        }
        Err(CallError::Lost("SSE stream ended before reply".into()))
    }

    /// Ends the HTTP session (DELETE with the session id) when one exists.
    pub(crate) async fn shutdown(&self) {
        self.lost.cancel();
        let Some(session) = self.session.lock().unwrap().clone() else { return };
        let request = self.client.delete(self.url.clone()).headers(self.headers.clone()).header(SESSION, session);
        let _ = tokio::time::timeout(SIDE_REQUEST_TIMEOUT, request.send()).await;
    }
}
