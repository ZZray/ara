//! HTTP/SSE client for the fixed OMP auth-broker protocol.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-broker/client.ts and packages/utils/src/stream.ts.
//! Mutation transport/receipt uncertainty is surfaced rather than replayed.
//
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

use crate::auth_broker_usage::{AuthBrokerAccountPool, AuthBrokerUsageOptions, AuthBrokerUsageStore};
use crate::auth_broker_wire::{
    self as wire, BrokerBlock, BrokerSnapshotEntry, BrokerSnapshotResult, BrokerStreamEvent, BrokerWireSchema,
};
use crate::auth_storage::AuthStorageError;
use crate::credential_store::{AuthCredential, DisabledCredentialSummary};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use reqwest::{Client, Method, Response, StatusCode};
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct AuthBrokerClientOptions {
    /// SSE uses caller cancellation only; supplied clients must have no
    /// global request timeout if they will be used for snapshot streaming.
    pub client: Client,
    pub request_timeout: Duration,
    pub max_retries: usize,
}
impl Default for AuthBrokerClientOptions {
    fn default() -> Self {
        Self { client: Client::new(), request_timeout: Duration::from_secs(10), max_retries: 1 }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrokerErrorKind {
    Configuration,
    Cancelled,
    Transport,
    OutcomeUnknown,
    Http,
    InvalidJson,
    InvalidSchema,
    StreamUnsupported,
    StreamEnded,
}

/// No endpoint, bearer, request body, response body or transport error text is
/// retained. A status is a transport receipt, not proof of mutation completion.
#[derive(Clone)]
pub struct BrokerError {
    pub status: Option<u16>,
    pub kind: BrokerErrorKind,
    pub outcome_unknown: bool,
}
impl BrokerError {
    fn new(kind: BrokerErrorKind, status: Option<u16>, outcome_unknown: bool) -> Self {
        Self { kind, status, outcome_unknown }
    }
    pub fn is_cancelled(&self) -> bool {
        self.kind == BrokerErrorKind::Cancelled
    }
    pub fn is_outcome_unknown(&self) -> bool {
        self.outcome_unknown
    }
    pub fn is_stream_unsupported(&self) -> bool {
        self.kind == BrokerErrorKind::StreamUnsupported
    }
}
impl fmt::Debug for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrokerError")
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("outcome_unknown", &self.outcome_unknown)
            .finish()
    }
}
impl fmt::Display for BrokerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "auth broker {:?}", self.kind)?;
        if let Some(status) = self.status {
            write!(f, " (HTTP {status})")?;
        }
        Ok(())
    }
}
impl std::error::Error for BrokerError {}

#[derive(Clone)]
pub struct AuthBrokerClient {
    inner: Arc<ClientInner>,
}
struct ClientInner {
    base_url: String,
    bearer: String,
    options: AuthBrokerClientOptions,
}
struct FetchOptions {
    body: Option<Value>,
    auth: bool,
    headers: reqwest::header::HeaderMap,
    timeout: Duration,
}

impl AuthBrokerClient {
    /// Internal authority namespace, never emitted as a receipt or diagnostic.
    pub(crate) fn reset_receipt_authority(&self) -> String {
        let canonical = reqwest::Url::parse(&self.inner.base_url).expect("validated broker endpoint");
        format!("remote:{}", canonical.as_str().trim_end_matches('/'))
    }

    pub fn new(
        base_url: impl Into<String>,
        bearer: impl Into<String>,
        options: AuthBrokerClientOptions,
    ) -> Result<Self, BrokerError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        let url = reqwest::Url::parse(&base_url)
            .map_err(|_| BrokerError::new(BrokerErrorKind::Configuration, None, false))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(BrokerError::new(BrokerErrorKind::Configuration, None, false));
        }
        let bearer = bearer.into();
        reqwest::header::HeaderValue::from_str(&format!("Bearer {bearer}"))
            .map_err(|_| BrokerError::new(BrokerErrorKind::Configuration, None, false))?;
        Ok(Self { inner: Arc::new(ClientInner { base_url, bearer, options }) })
    }

    /// The usage owner receives the same configured endpoint, credentials and
    /// reqwest transport; none are exposed to callers or diagnostics.
    pub fn usage_store(
        &self,
        clock: Arc<dyn Fn() -> f64 + Send + Sync>,
        account_pool: Option<AuthBrokerAccountPool>,
    ) -> Result<AuthBrokerUsageStore, AuthStorageError> {
        AuthBrokerUsageStore::new(
            self.inner.base_url.clone(),
            self.inner.bearer.clone(),
            AuthBrokerUsageOptions {
                client: self.inner.options.client.clone(),
                request_timeout: self.inner.options.request_timeout,
                clock,
                initial_snapshot: Vec::new(),
                account_pool,
            },
        )
    }

    pub async fn healthz(&self, cancel: &CancellationToken) -> Result<Value, BrokerError> {
        self.request(Method::GET, "/v1/healthz", None, BrokerWireSchema::HealthzResponse, false, cancel).await
    }
    pub async fn fetch_snapshot(
        &self,
        if_generation_gt: Option<i64>,
        wait_ms: Option<u64>,
        cancel: &CancellationToken,
    ) -> Result<BrokerSnapshotResult, BrokerError> {
        let mut path = "/v1/snapshot".to_owned();
        if let Some(wait) = wait_ms {
            path.push_str(&format!("?wait={wait}"));
        }
        let timeout = wait_ms
            .filter(|wait| *wait > 0)
            .map(|wait| self.inner.options.request_timeout.max(Duration::from_millis(wait.saturating_add(1000))))
            .unwrap_or(self.inner.options.request_timeout);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            wire::AUTH_BROKER_CAPABILITIES_HEADER,
            reqwest::header::HeaderValue::from_static(wire::AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES),
        );
        if let Some(generation) = if_generation_gt {
            headers.insert(
                reqwest::header::IF_NONE_MATCH,
                reqwest::header::HeaderValue::from_str(&format!("\"{generation}\""))
                    .map_err(|_| BrokerError::new(BrokerErrorKind::Configuration, None, false))?,
            );
        }
        let response = self
            .fetch_raw(Method::GET, &path, FetchOptions { body: None, auth: true, headers, timeout }, cancel)
            .await?;
        let status = response.status().as_u16();
        let tag = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|tag| tag.to_str().ok())
            .and_then(parse_generation_tag);
        if status == 304 {
            return Ok(BrokerSnapshotResult::NotModified { generation: tag.or(if_generation_gt).unwrap_or(0) });
        }
        let value = self.response_value(response, false, cancel).await?;
        let snapshot = wire::parse_snapshot(&value)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(status), false))?;
        let generation = tag.unwrap_or(snapshot.generation);
        Ok(BrokerSnapshotResult::Snapshot { snapshot, generation })
    }

    pub async fn open_snapshot_stream(&self, cancel: &CancellationToken) -> Result<BrokerSnapshotStream, BrokerError> {
        if cancel.is_cancelled() {
            return Err(BrokerError::new(BrokerErrorKind::Cancelled, None, false));
        }
        // Deliberately no per-request timeout or client-side retry. A supplied
        // reqwest Client should likewise have no global timeout for SSE.
        let request = self
            .inner
            .options
            .client
            .get(format!("{}/v1/snapshot/stream", self.inner.base_url))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .bearer_auth(&self.inner.bearer)
            .header(wire::AUTH_BROKER_CAPABILITIES_HEADER, wire::AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES);
        let response = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(BrokerError::new(BrokerErrorKind::Cancelled, None, false)),
            response = request.send() => response.map_err(|_| BrokerError::new(BrokerErrorKind::Transport, None, false))?,
        };
        let status = response.status();
        if status == StatusCode::NOT_FOUND {
            let _ = tokio::select! { biased; _ = cancel.cancelled() => None, body = response.bytes() => Some(body) };
            return Err(BrokerError::new(BrokerErrorKind::StreamUnsupported, Some(404), false));
        }
        if !status.is_success() {
            return Err(BrokerError::new(BrokerErrorKind::Http, Some(status.as_u16()), false));
        }
        let is_sse =
            response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(
                |value| value.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/event-stream"),
            );
        if !is_sse {
            return Err(BrokerError::new(BrokerErrorKind::InvalidSchema, Some(status.as_u16()), false));
        }
        Ok(BrokerSnapshotStream {
            source: Some(Box::pin(response.bytes_stream())),
            cancel: cancel.clone(),
            decoder: BrokerSseDecoder::default(),
            queued: VecDeque::new(),
            saw_first: false,
            ended: false,
            status: status.as_u16(),
        })
    }

    pub async fn refresh_credential(
        &self,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<BrokerSnapshotEntry, BrokerError> {
        let value = self
            .request(
                Method::POST,
                &format!("/v1/credential/{id}/refresh"),
                None,
                BrokerWireSchema::CredentialRefreshResponse,
                true,
                cancel,
            )
            .await?;
        wire::parse_credential_entry(&value["entry"])
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(200), true))
    }
    pub async fn disable_credential(
        &self,
        id: i64,
        cause: &str,
        cancel: &CancellationToken,
    ) -> Result<bool, BrokerError> {
        self.ok_request(
            Method::POST,
            &format!("/v1/credential/{id}/disable"),
            Some(json!({"cause": cause})),
            BrokerWireSchema::CredentialDisableResponse,
            cancel,
        )
        .await
    }
    pub async fn upload_credential(
        &self,
        provider: &str,
        credential: &AuthCredential,
        cancel: &CancellationToken,
    ) -> Result<Vec<BrokerSnapshotEntry>, BrokerError> {
        let body = json!({"provider": provider, "credential": credential});
        wire::validate_wire(BrokerWireSchema::CredentialUploadRequest, &body)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, None, false))?;
        let value = self
            .request(
                Method::POST,
                "/v1/credential",
                Some(body),
                BrokerWireSchema::CredentialUploadResponse,
                true,
                cancel,
            )
            .await?;
        value["entries"]
            .as_array()
            .ok_or_else(|| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(200), true))?
            .iter()
            .map(|value| {
                wire::parse_credential_entry(value)
                    .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(200), true))
            })
            .collect()
    }
    pub async fn upsert_credential_block(
        &self,
        id: i64,
        block: &BrokerBlock,
        cancel: &CancellationToken,
    ) -> Result<bool, BrokerError> {
        let body =
            serde_json::to_value(block).map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, None, false))?;
        wire::validate_wire(BrokerWireSchema::CredentialBlockRequest, &body)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, None, false))?;
        self.ok_request(
            Method::POST,
            &format!("/v1/credential/{id}/block"),
            Some(body),
            BrokerWireSchema::CredentialBlockResponse,
            cancel,
        )
        .await
    }
    pub async fn delete_credential_blocks(&self, id: i64, cancel: &CancellationToken) -> Result<bool, BrokerError> {
        self.ok_request(
            Method::DELETE,
            &format!("/v1/credential/{id}/blocks"),
            None,
            BrokerWireSchema::CredentialBlocksDeleteResponse,
            cancel,
        )
        .await
    }
    pub async fn list_disabled_credentials(
        &self,
        provider: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<DisabledCredentialSummary>, BrokerError> {
        let path = query_path("/v1/credentials/disabled", None, provider);
        let value = match self
            .request(Method::GET, &path, None, BrokerWireSchema::DisabledCredentialsResponse, true, cancel)
            .await
        {
            Err(error) if error.status == Some(404) => return Ok(Vec::new()),
            other => other?,
        };
        value["disabled"]
            .as_array()
            .ok_or_else(|| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(200), false))?
            .iter()
            .map(|value| {
                wire::parse_disabled_summary(value)
                    .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(200), false))
            })
            .collect()
    }
    pub async fn fetch_usage_history(
        &self,
        since_ms: Option<f64>,
        provider: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Value, BrokerError> {
        self.request(
            Method::GET,
            &query_path("/v1/usage/history", since_ms, provider),
            None,
            BrokerWireSchema::UsageHistoryResponse,
            true,
            cancel,
        )
        .await
    }
    /// Native serialized account probes get one base allowance per account
    /// plus one request allowance. This does not create another usage cache.
    pub async fn fetch_usage(
        &self,
        max_accounts_per_provider: Option<f64>,
        cancel: &CancellationToken,
    ) -> Result<Value, BrokerError> {
        let accounts = max_accounts_per_provider
            .filter(|count| count.is_finite())
            .map(|count| count.floor().max(1.0))
            .unwrap_or(1.0);
        let per_account = self.inner.options.request_timeout.max(Duration::from_secs(10));
        if accounts + 1.0 > u32::MAX as f64 {
            return Err(BrokerError::new(BrokerErrorKind::Configuration, None, false));
        }
        let timeout = per_account
            .checked_mul((accounts + 1.0) as u32)
            .ok_or_else(|| BrokerError::new(BrokerErrorKind::Configuration, None, false))?;
        let response = self
            .fetch_raw(
                Method::GET,
                "/v1/usage",
                FetchOptions { body: None, auth: true, headers: reqwest::header::HeaderMap::new(), timeout },
                cancel,
            )
            .await?;
        let status = response.status().as_u16();
        let value = self.response_value(response, false, cancel).await?;
        wire::validate_wire(BrokerWireSchema::UsageResponse, &value)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(status), false))?;
        Ok(value)
    }
    pub async fn notify_usage_stale(&self, cancel: &CancellationToken) -> Result<bool, BrokerError> {
        self.ok_request(Method::POST, "/v1/usage/stale", None, BrokerWireSchema::UsageStaleResponse, cancel).await
    }
    pub async fn report_client_usage(&self, report: &Value, cancel: &CancellationToken) -> Result<bool, BrokerError> {
        wire::validate_wire(BrokerWireSchema::ClientUsageReportRequest, report)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, None, false))?;
        self.ok_request(
            Method::POST,
            "/v1/usage/observed",
            Some(report.clone()),
            BrokerWireSchema::ClientUsageReportResponse,
            cancel,
        )
        .await
    }
    pub async fn fetch_client_usage_summary(
        &self,
        since_ms: Option<f64>,
        cancel: &CancellationToken,
    ) -> Result<Value, BrokerError> {
        self.request(
            Method::GET,
            &query_path("/v1/usage/clients", since_ms, None),
            None,
            BrokerWireSchema::ClientUsageSummaryResponse,
            true,
            cancel,
        )
        .await
    }

    async fn ok_request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        schema: BrokerWireSchema,
        cancel: &CancellationToken,
    ) -> Result<bool, BrokerError> {
        let value = self.request(method, path, body, schema, true, cancel).await?;
        // Native validates a boolean; false is a receipt, not a transport error.
        Ok(value["ok"].as_bool().unwrap_or(false))
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        schema: BrokerWireSchema,
        auth: bool,
        cancel: &CancellationToken,
    ) -> Result<Value, BrokerError> {
        let mutation = method != Method::GET;
        let response = self
            .fetch_raw(
                method,
                path,
                FetchOptions {
                    body,
                    auth,
                    headers: reqwest::header::HeaderMap::new(),
                    timeout: self.inner.options.request_timeout,
                },
                cancel,
            )
            .await?;
        let status = response.status().as_u16();
        let value = self.response_value(response, mutation, cancel).await?;
        wire::validate_wire(schema, &value)
            .map_err(|_| BrokerError::new(BrokerErrorKind::InvalidSchema, Some(status), mutation))?;
        Ok(value)
    }
    async fn response_value(
        &self,
        response: Response,
        mutation: bool,
        cancel: &CancellationToken,
    ) -> Result<Value, BrokerError> {
        let status = response.status().as_u16();
        let bytes = tokio::select! { biased;
            _ = cancel.cancelled() => return Err(BrokerError::new(BrokerErrorKind::Cancelled, Some(status), mutation)),
            bytes = response.bytes() => bytes.map_err(|_| BrokerError::new(BrokerErrorKind::Transport, Some(status), mutation))?,
        };
        // Response.text() decodes UTF-8 with replacement and strips a leading
        // BOM. Value first keeps JSON.parse's repeated-object-key last value.
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        if text.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(text).map_err(|_| BrokerError::new(BrokerErrorKind::InvalidJson, Some(status), mutation))
    }
    async fn fetch_raw(
        &self,
        method: Method,
        path: &str,
        options: FetchOptions,
        cancel: &CancellationToken,
    ) -> Result<Response, BrokerError> {
        if cancel.is_cancelled() {
            return Err(BrokerError::new(BrokerErrorKind::Cancelled, None, false));
        }
        let mutation = method != Method::GET;
        for attempt in 0..=self.inner.options.max_retries {
            let mut request = self
                .inner
                .options
                .client
                .request(method.clone(), format!("{}{}", self.inner.base_url, path))
                .header(reqwest::header::ACCEPT, "application/json")
                .headers(options.headers.clone())
                .timeout(options.timeout);
            if options.auth {
                request = request.bearer_auth(&self.inner.bearer);
            }
            if let Some(body) = &options.body {
                request = request.json(body);
            }
            let response = tokio::select! { biased;
                _ = cancel.cancelled() => return Err(BrokerError::new(BrokerErrorKind::Cancelled, None, mutation)),
                response = request.send() => response,
            };
            match response {
                Ok(response) => {
                    let status = response.status();
                    if !status.is_success() && status != StatusCode::NOT_MODIFIED {
                        // Observed reports append entry by entry without a group
                        // transaction: 500 may follow an already-persisted prefix.
                        // Native observed 501 means the persistence hook is
                        // absent, before any entries can be recorded; keep the
                        // store's permanent unsupported-endpoint latch.
                        let observed_unsupported =
                            path == "/v1/usage/observed" && status == StatusCode::NOT_IMPLEMENTED;
                        let possible_partial_effect = mutation && status.is_server_error() && !observed_unsupported;
                        return Err(BrokerError::new(
                            BrokerErrorKind::Http,
                            Some(status.as_u16()),
                            possible_partial_effect,
                        ));
                    }
                    return Ok(response);
                }
                Err(error) => {
                    if cancel.is_cancelled() {
                        return Err(BrokerError::new(BrokerErrorKind::Cancelled, None, mutation));
                    }
                    if error.is_builder() {
                        return Err(BrokerError::new(BrokerErrorKind::Configuration, None, false));
                    }
                    // The supplied transport may follow redirects or use a
                    // proxy. A generic is_connect error is not proof that no
                    // earlier application request was dispatched. Native
                    // retries these mutations; this explicit adaptation stays
                    // open for parity until broker receipts support replay.
                    if mutation {
                        return Err(BrokerError::new(
                            BrokerErrorKind::OutcomeUnknown,
                            error.status().map(|s| s.as_u16()),
                            true,
                        ));
                    }
                    if error.status().is_some() || attempt == self.inner.options.max_retries {
                        return Err(BrokerError::new(
                            BrokerErrorKind::Transport,
                            error.status().map(|s| s.as_u16()),
                            false,
                        ));
                    }
                }
            }
        }
        Err(BrokerError::new(BrokerErrorKind::Transport, None, false))
    }
}

fn query_path(base: &str, since: Option<f64>, provider: Option<&str>) -> String {
    let mut url = reqwest::Url::parse("http://broker.invalid").expect("constant URL");
    url.set_path(base);
    {
        let mut query = url.query_pairs_mut();
        if let Some(since) = since {
            query.append_pair("sinceMs", &since.to_string());
        }
        if let Some(provider) = provider.filter(|provider| !provider.is_empty()) {
            query.append_pair("provider", provider);
        }
    }
    match url.query().filter(|query| !query.is_empty()) {
        Some(query) => format!("{base}?{query}"),
        None => base.to_owned(),
    }
}
fn parse_generation_tag(header: &str) -> Option<i64> {
    if header.is_empty() {
        return None;
    }
    let mut text = trim_js_whitespace(header);
    if let Some(rest) = text.strip_prefix("W/") {
        text = trim_js_whitespace(rest);
    }
    if text.starts_with('"') && text.ends_with('"') && text.len() >= 2 {
        text = &text[1..text.len() - 1];
    }
    text = trim_js_whitespace(text);
    let value = if text.is_empty() {
        0.0
    } else if let Some(value) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(value, 16).ok()? as f64
    } else if let Some(value) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        u64::from_str_radix(value, 2).ok()? as f64
    } else if let Some(value) = text.strip_prefix("0o").or_else(|| text.strip_prefix("0O")) {
        u64::from_str_radix(value, 8).ok()? as f64
    } else {
        text.parse::<f64>().ok()?
    };
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value >= 9_223_372_036_854_775_808.0 {
        return None;
    }
    Some(value as i64)
}
fn trim_js_whitespace(text: &str) -> &str {
    text.trim_matches(|character: char| matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'))
}

type BrokerBodyStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
pub struct BrokerSnapshotStream {
    source: Option<BrokerBodyStream>,
    cancel: CancellationToken,
    decoder: BrokerSseDecoder,
    queued: VecDeque<RawBrokerEvent>,
    saw_first: bool,
    ended: bool,
    status: u16,
}
impl BrokerSnapshotStream {
    pub async fn next_event(&mut self) -> Result<Option<BrokerStreamEvent>, BrokerError> {
        loop {
            if self.cancel.is_cancelled() {
                self.ended = true;
                self.source = None;
                self.queued.clear();
                return Ok(None);
            }
            if let Some(event) = self.queued.pop_front() {
                if event.event.is_none() && event.data.is_empty() {
                    continue;
                }
                let value: Value = match serde_json::from_str(&event.data) {
                    Ok(value) => value,
                    Err(_) => return self.end_error(BrokerError::new(BrokerErrorKind::InvalidJson, None, false)),
                };
                let event = match wire::parse_stream_event(&value) {
                    Ok(event) => event,
                    Err(_) => return self.end_error(BrokerError::new(BrokerErrorKind::InvalidSchema, None, false)),
                };
                if !self.saw_first && !matches!(event, BrokerStreamEvent::Snapshot(_)) {
                    return self.end_error(BrokerError::new(BrokerErrorKind::InvalidSchema, None, false));
                }
                self.saw_first = true;
                return Ok(Some(event));
            }
            if self.ended {
                return Err(BrokerError::new(BrokerErrorKind::StreamEnded, Some(self.status), false));
            }
            let chunk = tokio::select! { biased; _ = self.cancel.cancelled() => { self.ended = true; self.source = None; return Ok(None); }, chunk = self.source.as_mut().expect("active snapshot source").next() => chunk };
            match chunk {
                Some(Ok(bytes)) => self.queued.extend(self.decoder.feed(&bytes)),
                Some(Err(_)) => {
                    self.ended = true;
                    self.source = None;
                    return Err(BrokerError::new(BrokerErrorKind::Transport, None, false));
                }
                None => {
                    self.ended = true;
                    self.source = None;
                    if let Some(event) = self.decoder.finish() {
                        self.queued.push_back(event);
                    }
                }
            }
        }
    }
    fn end_error(&mut self, error: BrokerError) -> Result<Option<BrokerStreamEvent>, BrokerError> {
        self.ended = true;
        self.source = None;
        self.queued.clear();
        Err(error)
    }
}

struct RawBrokerEvent {
    event: Option<String>,
    data: String,
}
/// Native pi-utils uses LF-only complete batches, strips one trailing CR and
/// lets event-only frames reach broker JSON validation. The shared AI decoder
/// has different bare-CR/empty-event behavior, so its behavior is left intact.
#[derive(Default)]
struct BrokerSseDecoder {
    pending: Vec<u8>,
    event: Option<String>,
    data: Option<String>,
    id: Option<String>,
    retry: Option<u64>,
}
impl BrokerSseDecoder {
    fn feed(&mut self, bytes: &[u8]) -> Vec<RawBrokerEvent> {
        let Some(last_lf) = bytes.iter().rposition(|byte| *byte == b'\n') else {
            self.pending.extend_from_slice(bytes);
            return Vec::new();
        };
        self.pending.extend_from_slice(&bytes[..=last_lf]);
        let complete = std::mem::take(&mut self.pending);
        let text = String::from_utf8_lossy(&complete);
        // TextDecoder.decode resets BOM state on each native batch.
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let mut events = Vec::new();
        for line in text.split_terminator('\n') {
            if let Some(event) = self.line(line) {
                events.push(event);
            }
        }
        self.pending.extend_from_slice(&bytes[last_lf + 1..]);
        events
    }
    fn finish(&mut self) -> Option<RawBrokerEvent> {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let text = String::from_utf8_lossy(&pending);
            let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
            if let Some(event) = self.line(text) {
                return Some(event);
            }
        }
        self.dispatch()
    }
    fn line(&mut self, line: &str) -> Option<RawBrokerEvent> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => match &mut self.data {
                Some(data) => {
                    data.push('\n');
                    data.push_str(value);
                }
                None => self.data = Some(value.to_owned()),
            },
            "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
            "retry" if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
                if let Ok(retry) = value.parse::<u64>()
                    && retry <= 9_007_199_254_740_991
                {
                    self.retry = Some(retry);
                }
            }
            _ => {}
        }
        None
    }
    fn dispatch(&mut self) -> Option<RawBrokerEvent> {
        if self.event.is_none() && self.data.is_none() && self.id.is_none() && self.retry.is_none() {
            return None;
        }
        self.id = None;
        self.retry = None;
        Some(RawBrokerEvent { event: self.event.take(), data: self.data.take().unwrap_or_default() })
    }
}

#[cfg(test)]
#[path = "auth_broker_client_tests.rs"]
mod tests;
