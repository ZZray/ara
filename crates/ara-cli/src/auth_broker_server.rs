//! Fixed OMP Broker HTTP server; storage owns all dispatched mutations.

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
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use crate::auth_broker_refresher::AuthBrokerRefresher;
use crate::auth_broker_server_snapshot::{entry_value, fingerprint, snapshot_value, sse};
use crate::auth_broker_wire::{
    AUTH_BROKER_CAPABILITIES_HEADER, AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES, BrokerBlock, BrokerWireSchema,
    validate_wire,
};
use crate::auth_storage::{AuthRequestContext, AuthStorage, BrokerStorageError, BrokerStorageFacade};
use crate::credential_store::{AuthCredential, ClientUsageReport, UsageHistoryQuery};
use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Body, Frame, Incoming, SizeHint},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::{TcpListener, TcpStream},
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

pub struct AuthBrokerServerOptions {
    pub storage: Arc<AuthStorage>,
    pub bind: String,
    pub bearer_tokens: Vec<String>,
    pub version: Option<String>,
    pub refresh_skew: Duration,
    pub refresh_interval: Duration,
    pub disable_refresher: bool,
    pub stream_keepalive: Duration,
    pub external_change_poll: Duration,
    pub managed_mcp_client: Option<reqwest::Client>,
}
impl AuthBrokerServerOptions {
    pub fn new(storage: Arc<AuthStorage>, bearer_tokens: Vec<String>) -> Self {
        Self {
            storage,
            bearer_tokens,
            bind: "127.0.0.1:8765".into(),
            version: None,
            refresh_skew: Duration::from_millis(300_000),
            refresh_interval: Duration::from_millis(60_000),
            disable_refresher: false,
            stream_keepalive: Duration::from_millis(20_000),
            external_change_poll: Duration::from_millis(250),
            managed_mcp_client: None,
        }
    }
}
#[derive(Debug)]
pub enum BrokerServerError {
    InvalidBind(String),
    Bind(io::Error),
    Transport,
}
impl std::fmt::Display for BrokerServerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBind(message) => f.write_str(message),
            Self::Bind(error) => write!(f, "broker bind failed: {error}"),
            Self::Transport => f.write_str("broker transport failed"),
        }
    }
}
impl std::error::Error for BrokerServerError {}

pub struct AuthBrokerServerHandle {
    url: String,
    hostname: String,
    port: u16,
    stop: CancellationToken,
    tasks: Mutex<Option<Vec<JoinHandle<()>>>>,
}
impl AuthBrokerServerHandle {
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn hostname(&self) -> &str {
        &self.hostname
    }
    pub fn port(&self) -> u16 {
        self.port
    }
    pub async fn close(&self) -> Result<(), BrokerServerError> {
        self.stop.cancel();
        let tasks = self.tasks.lock().map_err(|_| BrokerServerError::Transport)?.take().unwrap_or_default();
        for task in tasks {
            task.await.map_err(|_| BrokerServerError::Transport)?;
        }
        Ok(())
    }
}
impl Drop for AuthBrokerServerHandle {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

pub fn parse_broker_bind(raw: &str) -> Result<(String, u16), BrokerServerError> {
    let trimmed = ara_prompt::js::trim(raw);
    if trimmed.is_empty() {
        return Err(BrokerServerError::InvalidBind("Invalid bind; expected 'host:port' or 'port'.".into()));
    }
    let (host, port) = if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        ("127.0.0.1", trimmed)
    } else {
        trimmed.rsplit_once(':').ok_or_else(|| {
            BrokerServerError::InvalidBind(format!("Invalid bind '{raw}'; expected 'host:port' or 'port'."))
        })?
    };
    if host.is_empty() {
        return Err(BrokerServerError::InvalidBind(format!("Invalid bind '{raw}'; host must not be empty.")));
    }
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(BrokerServerError::InvalidBind(format!("Invalid bind '{raw}'; port must be an integer.")));
    }
    let numeric = port.trim_start_matches('0');
    let numeric = if numeric.is_empty() { "0" } else { numeric };
    let port = numeric
        .parse::<u16>()
        .map_err(|_| BrokerServerError::InvalidBind(format!("Invalid bind '{raw}'; port out of range.")))?;
    Ok((host.to_owned(), port))
}
struct ServerState {
    storage: BrokerStorageFacade,
    tokens: BTreeSet<String>,
    version: Option<String>,
    refresher: Option<AuthBrokerRefresher>,
    keepalive: Duration,
}
impl ServerState {
    fn now(&self) -> f64 {
        chrono::Utc::now().timestamp_millis() as f64
    }
    fn snapshot(&self, capable: bool) -> Result<Value, BrokerStorageError> {
        snapshot_value(
            self.storage.export_snapshot()?,
            self.refresher.as_ref().map(AuthBrokerRefresher::schedule),
            capable,
            self.now(),
        )
    }
}
pub async fn start_auth_broker(options: AuthBrokerServerOptions) -> Result<AuthBrokerServerHandle, BrokerServerError> {
    let (hostname, port) = parse_broker_bind(&options.bind)?;
    let bind_host = hostname.strip_prefix('[').and_then(|host| host.strip_suffix(']')).unwrap_or(&hostname);
    let listener = TcpListener::bind((bind_host, port)).await.map_err(BrokerServerError::Bind)?;
    let port = listener.local_addr().map_err(BrokerServerError::Bind)?.port();
    let mut storage = BrokerStorageFacade::new(options.storage);
    if let Some(client) = options.managed_mcp_client {
        storage = storage.with_managed_mcp(client);
    }
    let refresher = if options.disable_refresher {
        None
    } else {
        let owner = AuthBrokerRefresher::new(
            storage.clone(),
            options.refresh_skew,
            options.refresh_interval,
            Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
        );
        owner.start();
        Some(owner)
    };
    let state = Arc::new(ServerState {
        storage,
        tokens: options.bearer_tokens.into_iter().collect(),
        version: options.version,
        refresher,
        keepalive: options.stream_keepalive,
    });
    let stop = CancellationToken::new();
    let poll_stop = stop.clone();
    let poll_state = state.clone();
    let poll = tokio::spawn(async move {
        loop {
            if poll_stop.is_cancelled() {
                break;
            }
            let _ = poll_state.storage.poll_external_changes().await;
            tokio::select! { _ = poll_stop.cancelled() => break,
            _ = tokio::time::sleep(options.external_change_poll.max(Duration::from_millis(1))) => {} }
        }
    });
    let listen_stop = stop.clone();
    let server = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        loop {
            tokio::select! { biased;
                _ = listen_stop.cancelled() => break,
                Some(_) = connections.join_next(), if !connections.is_empty() => {},
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        let connection_stop = listen_stop.child_token(); let state = state.clone();
                        connections.spawn(async move {
                            let request_stop = connection_stop.clone();
                            let service = service_fn(move |request| {
                                let state = state.clone(); let token = request_stop.child_token();
                                async move { Ok::<_, Infallible>(handle_request(request, state, token).await) }
                            });
                            let io = TokioIo::new(IdleIo::new(stream));
                            let connection = http1::Builder::new().serve_connection(io, service);
                            tokio::select! { _ = connection_stop.cancelled() => {}, _ = connection => {} }
                            connection_stop.cancel();
                        });
                    }
                    Err(_) => break,
                }
            }
        }
        listen_stop.cancel();
        if let Some(refresher) = &state.refresher {
            refresher.stop();
        }
        // Each connection independently observes stop and drops its own waits.
        while connections.join_next().await.is_some() {}
    });
    Ok(AuthBrokerServerHandle {
        url: format!("http://{hostname}:{port}"),
        hostname,
        port,
        stop,
        tasks: Mutex::new(Some(vec![server, poll])),
    })
}

// Bun's 255s idle timeout counts inactivity, not total SSE or connection age.
struct IdleIo {
    stream: TcpStream,
    deadline: Pin<Box<tokio::time::Sleep>>,
}
impl IdleIo {
    fn new(stream: TcpStream) -> Self {
        Self { stream, deadline: Box::pin(tokio::time::sleep(Duration::from_secs(255))) }
    }
    fn activity(&mut self) {
        self.deadline.as_mut().reset(tokio::time::Instant::now() + Duration::from_secs(255));
    }
    fn expired(&mut self, cx: &mut Context<'_>) -> bool {
        self.deadline.as_mut().poll(cx).is_ready()
    }
}
impl AsyncRead for IdleIo {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buffer: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        if self.expired(cx) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "broker idle timeout")));
        }
        let before = buffer.filled().len();
        let result = Pin::new(&mut self.stream).poll_read(cx, buffer);
        if buffer.filled().len() > before {
            self.activity();
        }
        result
    }
}
impl AsyncWrite for IdleIo {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
        if self.expired(cx) {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::TimedOut, "broker idle timeout")));
        }
        let result = Pin::new(&mut self.stream).poll_write(cx, bytes);
        if matches!(&result, Poll::Ready(Ok(size)) if *size > 0) {
            self.activity();
        }
        result
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}
struct CancelOnDrop(Option<CancellationToken>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if let Some(token) = self.0.take() {
            token.cancel();
        }
    }
}
enum BrokerBody {
    Full(Full<Bytes>),
    Stream { receiver: mpsc::UnboundedReceiver<Bytes>, stop: CancellationToken },
}
impl Body for BrokerBody {
    type Data = Bytes;
    type Error = Infallible;
    fn poll_frame(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        match &mut *self {
            Self::Full(body) => Pin::new(body).poll_frame(cx),
            Self::Stream { receiver, .. } => {
                receiver.poll_recv(cx).map(|value| value.map(|bytes| Ok(Frame::data(bytes))))
            }
        }
    }
    fn is_end_stream(&self) -> bool {
        match self {
            Self::Full(body) => body.is_end_stream(),
            Self::Stream { .. } => false,
        }
    }
    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Full(body) => body.size_hint(),
            Self::Stream { .. } => SizeHint::default(),
        }
    }
}
impl Drop for BrokerBody {
    fn drop(&mut self) {
        if let Self::Stream { stop, .. } = self {
            stop.cancel();
        }
    }
}
fn response(status: u16, body: Value) -> Response<BrokerBody> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(BrokerBody::Full(Full::new(Bytes::from(body.to_string()))))
        .expect("static response")
}
fn empty(status: u16) -> Response<BrokerBody> {
    Response::builder().status(status).body(BrokerBody::Full(Full::new(Bytes::new()))).expect("static response")
}
fn failure(error: BrokerStorageError) -> Response<BrokerBody> {
    response(
        if matches!(&error, BrokerStorageError::MissingCredential(_)) { 404 } else { 500 },
        json!({"error":error.to_string()}),
    )
}
fn headers(response: &mut Response<BrokerBody>, generation: u64) {
    response.headers_mut().insert("ETag", format!("\"{generation}\"").parse().expect("numeric generation"));
    response.headers_mut().insert("Cache-Control", http::HeaderValue::from_static("no-store"));
    response.headers_mut().insert("Vary", http::HeaderValue::from_static(AUTH_BROKER_CAPABILITIES_HEADER));
}
fn authorized(request: &Request<Incoming>, tokens: &BTreeSet<String>) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let Some(raw) = request.headers().get("authorization").and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(at) = raw.find(char::is_whitespace) else {
        return false;
    };
    raw[..at].eq_ignore_ascii_case("Bearer") && tokens.contains(raw[at..].trim())
}
fn capable(request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get(AUTH_BROKER_CAPABILITIES_HEADER)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|v| v.trim() == AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES))
}
fn generation_tag(header: Option<&str>) -> Option<f64> {
    let raw = header?;
    if raw.is_empty() {
        return None;
    }
    let mut value = ara_prompt::js::trim(raw);
    if let Some(rest) = value.strip_prefix("W/") {
        value = ara_prompt::js::trim(rest);
    }
    if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        value = &value[1..value.len() - 1];
    }
    let generation = ara_prompt::js::to_number(&json!(value));
    (generation.is_finite() && generation >= 0.0 && generation.fract() == 0.0).then_some(generation)
}
fn query(request: &Request<Incoming>, key: &str) -> Option<String> {
    url_query(request.uri().query(), key)
}
fn url_query(raw: Option<&str>, key: &str) -> Option<String> {
    raw.and_then(|raw| reqwest::Url::parse(&format!("http://broker/?{raw}")).ok())
        .and_then(|url| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned()))
}
fn parse_int_prefix(raw: &str) -> Option<i64> {
    let value = ara_prompt::js::trim_start(raw);
    let sign = usize::from(value.starts_with('+') || value.starts_with('-'));
    let end = sign + value[sign..].bytes().take_while(u8::is_ascii_digit).count();
    if end == sign { None } else { value[..end].parse::<f64>().ok().filter(|v| v.is_finite()).map(|v| v as i64) }
}
async fn parse_body(
    request: Request<Incoming>,
    schema: BrokerWireSchema,
    allow_empty: bool,
    cancel: &CancellationToken,
) -> Result<Value, Response<BrokerBody>> {
    let bytes = tokio::select! { biased; _=cancel.cancelled()=>return Err(response(400,json!({"error":"Invalid request body"}))),
    result=request.into_body().collect()=>result.map_err(|_|response(400,json!({"error":"Invalid request body"})))?.to_bytes() };
    if bytes.is_empty() && !allow_empty {
        return Err(response(400, json!({"error":"Request body required"})));
    }
    let value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).map_err(|_| response(400, json!({"error":"Invalid JSON body"})))?
    };
    validate_wire(schema, &value).map_err(|error| response(400, json!({"error":error.to_string()})))?;
    Ok(value)
}
fn route_id(path: &str, operation: &str) -> Option<i64> {
    let value = path.strip_prefix("/v1/credential/")?.strip_suffix(&format!("/{operation}"))?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<i64>().ok()
}
async fn handle_request(
    request: Request<Incoming>,
    state: Arc<ServerState>,
    cancel: CancellationToken,
) -> Response<BrokerBody> {
    let mut lifetime = CancelOnDrop(Some(cancel.clone()));
    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    if method == http::Method::GET && path == "/v1/healthz" {
        let mut body = json!({"ok":true});
        if let Some(version) = &state.version {
            body["version"] = json!(version);
        }
        return response(200, body);
    }
    if !authorized(&request, &state.tokens) {
        return response(401, json!({"error":"unauthorized"}));
    }
    if method == http::Method::GET && path == "/v1/snapshot/stream" {
        lifetime.0.take();
        return stream_response(state, capable(&request), cancel);
    }
    if method == http::Method::GET && path == "/v1/snapshot" {
        return serve_snapshot(&request, &state, &cancel).await;
    }
    if method == http::Method::GET && path == "/v1/usage" {
        return match state.storage.storage().fetch_usage_reports(None, &AuthRequestContext::default(), &cancel).await {
            Ok(reports) => {
                let mut values = serde_json::to_value(reports.unwrap_or_default()).unwrap_or_else(|_| json!([]));
                if let Some(reports) = values.as_array_mut() {
                    for report in reports {
                        if let Some(value) = report.as_object_mut() {
                            value.remove("raw");
                        }
                    }
                }
                response(200, json!({"generatedAt":state.now(),"reports":values}))
            }
            Err(error) => response(502, json!({"error":error.to_string()})),
        };
    }
    if method == http::Method::GET && path == "/v1/usage/history" {
        let query = UsageHistoryQuery {
            since_ms: query(&request, "sinceMs").as_deref().and_then(parse_int_prefix),
            provider: query(&request, "provider"),
        };
        return match state.storage.storage().list_usage_history(Some(&query)) {
            Ok(entries) => response(200, json!({"generatedAt":state.now(),"entries":entries})),
            Err(error) => failure(error.into()),
        };
    }
    if method == http::Method::POST && path == "/v1/usage/observed" {
        let value = match parse_body(request, BrokerWireSchema::ClientUsageReportRequest, false, &cancel).await {
            Ok(value) => value,
            Err(error) => return error,
        };
        let Some(usage) = state.storage.local_usage() else {
            return response(501, json!({"error":"broker store does not persist client usage"}));
        };
        let report: ClientUsageReport = match serde_json::from_value(value) {
            Ok(report) => report,
            Err(_) => return response(400, json!({"error":"Invalid client usage report"})),
        };
        return match usage.record(report).await {
            Ok(()) => response(200, json!({"ok":true})),
            Err(error) => failure(error.into()),
        };
    }
    if method == http::Method::GET && path == "/v1/usage/clients" {
        let since = query(&request, "sinceMs").as_deref().and_then(parse_int_prefix).unwrap_or(0);
        return match state.storage.local_usage() {
            Some(usage) => match usage.summary(since).await {
                Ok(summary) => response(200, json!({"generatedAt":state.now(),"clients":summary.clients})),
                Err(error) => failure(error.into()),
            },
            None => response(200, json!({"generatedAt":state.now(),"clients":[]})),
        };
    }
    if method == http::Method::POST && path == "/v1/usage/stale" {
        return match state.storage.storage().invalidate_usage_cache_and_notify(None, &cancel).await {
            Ok(()) => response(200, json!({"ok":true})),
            Err(error) => failure(error.into()),
        };
    }
    if method == http::Method::GET && path == "/v1/credentials/disabled" {
        return match state
            .storage
            .storage()
            .list_disabled_credentials(query(&request, "provider").as_deref(), &cancel)
            .await
        {
            Ok(disabled) => response(200, json!({"generatedAt":state.now(),"disabled":disabled})),
            Err(error) => failure(error.into()),
        };
    }
    if method == http::Method::POST {
        if let Some(id) = route_id(&path, "refresh") {
            return match state.storage.force_refresh(id, &cancel).await {
                Ok(entry) => response(200, json!({"entry":entry_value(&entry)})),
                Err(error) => failure(error),
            };
        }
        if let Some(id) = route_id(&path, "disable") {
            let value = match parse_body(request, BrokerWireSchema::CredentialDisableRequest, true, &cancel).await {
                Ok(value) => value,
                Err(error) => return error,
            };
            let cause = value["cause"].as_str().filter(|v| !v.is_empty()).unwrap_or("disabled via auth-broker");
            return match state.storage.disable(id, cause, &cancel).await {
                Ok(true) => response(200, json!({"ok":true})),
                Ok(false) => failure(BrokerStorageError::MissingCredential(id)),
                Err(error) => failure(error),
            };
        }
        if let Some(id) = route_id(&path, "block") {
            let value = match parse_body(request, BrokerWireSchema::CredentialBlockRequest, false, &cancel).await {
                Ok(value) => value,
                Err(error) => return error,
            };
            let block: BrokerBlock = match serde_json::from_value(value) {
                Ok(block) => block,
                Err(_) => return response(400, json!({"error":"Invalid credential block"})),
            };
            return match state.storage.upsert_block(id, block) {
                Ok(()) => response(200, json!({"ok":true})),
                Err(error) => failure(error),
            };
        }
        if path == "/v1/credential" {
            let value = match parse_body(request, BrokerWireSchema::CredentialUploadRequest, false, &cancel).await {
                Ok(value) => value,
                Err(error) => return error,
            };
            let provider = value["provider"].as_str().expect("validated provider");
            let credential: AuthCredential = match serde_json::from_value(value["credential"].clone()) {
                Ok(credential) => credential,
                Err(_) => return response(400, json!({"error":"Invalid credential"})),
            };
            return match state.storage.upsert(provider, credential, &cancel).await {
                Ok(entries) => response(200, json!({"entries":entries.iter().map(entry_value).collect::<Vec<_>>()})),
                Err(error) => failure(error),
            };
        }
    }
    if method == http::Method::DELETE
        && let Some(id) = route_id(&path, "blocks")
    {
        return match state.storage.delete_blocks(id) {
            Ok(()) => response(200, json!({"ok":true})),
            Err(error) => failure(error),
        };
    }
    response(404, json!({"error":format!("No route: {method} {path}")}))
}
async fn serve_snapshot(
    request: &Request<Incoming>,
    state: &ServerState,
    cancel: &CancellationToken,
) -> Response<BrokerBody> {
    if let Err(error) = state.storage.reload().await {
        return failure(error);
    }
    let generation = state.storage.generation();
    let client = generation_tag(request.headers().get("If-None-Match").and_then(|v| v.to_str().ok()));
    let wait = query(request, "wait")
        .map(|raw| ara_prompt::js::to_number(&json!(raw)))
        .filter(|n| n.is_finite())
        .unwrap_or(0.0)
        .trunc()
        .clamp(0.0, 30_000.0);
    if client == Some(generation as f64) && wait > 0.0 {
        // Native GenerationGate waits against the generation checked above,
        // including a bump between that check and subscription registration.
        let mut subscription = state.storage.subscribe_generation_from(generation);
        tokio::select! {biased;
            _=cancel.cancelled()=>{let mut result=empty(499);headers(&mut result,generation);return result;},
            _=subscription.changed(cancel)=>{}, _=tokio::time::sleep(Duration::from_millis(wait as u64))=>{}
        }
        if let Err(error) = state.storage.reload().await {
            return failure(error);
        }
        if state.storage.generation() == generation {
            let mut result = empty(304);
            headers(&mut result, generation);
            return result;
        }
    }
    match state.snapshot(capable(request)) {
        Ok(snapshot) => {
            let generation = snapshot["generation"].as_u64().expect("owned generation");
            let mut result = response(200, snapshot);
            headers(&mut result, generation);
            result
        }
        Err(error) => failure(error),
    }
}
fn stream_response(state: Arc<ServerState>, capable: bool, stop: CancellationToken) -> Response<BrokerBody> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let owner_stop = stop.clone();
    tokio::spawn(async move {
        let _lifetime = CancelOnDrop(Some(owner_stop.clone()));
        if state.storage.reload().await.is_err() || owner_stop.is_cancelled() {
            return;
        }
        let Ok(mut initial) = state.snapshot(capable) else {
            return;
        };
        let mut last = BTreeMap::<i64, String>::new();
        for entry in initial["credentials"].as_array().into_iter().flatten() {
            if let Some(id) = entry["id"].as_i64() {
                last.insert(id, fingerprint(entry));
            }
        }
        initial["kind"] = json!("snapshot");
        let generation = initial["generation"].as_u64().expect("snapshot generation is u64");
        if sender.send(sse("snapshot", initial)).is_err() {
            return;
        }
        // Retain the initial generation across the snapshot-to-subscribe window.
        let mut subscription = state.storage.subscribe_generation_from(generation);
        let mut keepalive = tokio::time::interval(state.keepalive.max(Duration::from_millis(1)));
        keepalive.tick().await;
        loop {
            tokio::select! {biased;
                _=owner_stop.cancelled()=>break,
                _=sender.closed()=>break,
                _=keepalive.tick()=>if sender.send(Bytes::from_static(b": keepalive\n\n")).is_err() {break;},
                changed=subscription.changed(&owner_stop)=>{
                    if changed.is_err() || state.storage.reload().await.is_err() || owner_stop.is_cancelled() {break;}
                    let Ok(snapshot)=state.snapshot(capable) else {break;}; let mut seen=BTreeSet::new();
                    for entry in snapshot["credentials"].as_array().into_iter().flatten() {
                        let Some(id)=entry["id"].as_i64() else {continue;};seen.insert(id);let fp=fingerprint(entry);
                        if last.get(&id)==Some(&fp) {continue;}
                        last.insert(id,fp);
                        if sender.send(sse("entry",json!({"kind":"entry","generation":snapshot["generation"],"serverNowMs":snapshot["serverNowMs"],"refresher":snapshot["refresher"],"entry":entry}))).is_err() {return;}
                    }
                    let removed=last.keys().copied().filter(|id|!seen.contains(id)).collect::<Vec<_>>();
                    for id in removed {last.remove(&id);if sender.send(sse("removed",json!({"kind":"removed","generation":snapshot["generation"],"serverNowMs":snapshot["serverNowMs"],"refresher":snapshot["refresher"],"id":id}))).is_err() {return;}}
                }
            }
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "text/event-stream; charset=utf-8")
        .header("Cache-Control", "no-cache")
        .header("Connection", "keep-alive")
        .header("X-Accel-Buffering", "no")
        .header("Vary", AUTH_BROKER_CAPABILITIES_HEADER)
        .body(BrokerBody::Stream { receiver, stop })
        .expect("static SSE response")
}
