//! Host-owned OpenAI Codex device authorization and per-request OAuth refresh.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `packages/ai/src/registry/oauth/openai-codex.ts`, `auth/auth-storage.ts`,
//! and `packages/catalog/src/wire/codex.ts`. No existing installation is read.
//!
//! Unknown refresh outcomes disable the unchanged credential and require a new
//! interactive login. A detached worker settles refresh even if its caller drops
//! the resolver future. SQLite locks never span a network operation.
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

use crate::credential_store::{
    AuthCredential, CredentialRefreshLeaseFence, SqliteCredentialStore, StoredAuthCredential,
};
use crate::model_route::{AuthResolveError, CredentialIdentity, RequestAuthLease, RequestAuthResolver};
use ara_ai::Model;
use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Map, Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

const PROVIDER: &str = "openai-codex";
const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEVICE_AUTH_URL: &str = "https://auth.openai.com/codex/device";
const REDIRECT_URI: &str = "https://auth.openai.com/deviceauth/callback";
const AUTH_CLAIM: &str = "https://api.openai.com/auth";
const PROFILE_CLAIM: &str = "https://api.openai.com/profile";
const REFRESH_SKEW_MS: i64 = 60_000;
const LEASE_TTL_MS: i64 = 15_000;
const UNKNOWN_CAUSE: &str = "refresh outcome unknown; login required";

/// Contains only a public verification URL and the short-lived user-facing code.
pub struct DeviceLoginInfo {
    pub verification_url: &'static str,
    pub user_code: String,
}

pub struct LoginIdentity {
    pub credential_id: i64,
    pub account_id: String,
    pub email: Option<String>,
}

/// Safe categories only: token responses and underlying request errors stay private.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexAuthError {
    Cancelled,
    Storage,
    Transport,
    HttpStatus(u16),
    InvalidResponse,
    TimedOut,
    LoginRequired,
    InvalidEndpoint,
}

impl std::fmt::Display for CodexAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("OpenAI Codex authentication cancelled"),
            Self::Storage => f.write_str("OpenAI Codex authentication storage failed"),
            Self::Transport => f.write_str("OpenAI Codex authentication transport failed"),
            Self::HttpStatus(status) => write!(f, "OpenAI Codex authentication HTTP {status}"),
            Self::InvalidResponse => f.write_str("OpenAI Codex authentication response is invalid"),
            Self::TimedOut => f.write_str("OpenAI Codex authentication timed out"),
            Self::LoginRequired => f.write_str("OpenAI Codex login required"),
            Self::InvalidEndpoint => f.write_str("OpenAI Codex authentication endpoint is not permitted"),
        }
    }
}

impl std::error::Error for CodexAuthError {}

/// Private credentials are intentionally not Debug or Serialize.
#[derive(Clone)]
pub struct OpenAiCodexAuth {
    store: Arc<Mutex<SqliteCredentialStore>>,
    client: reqwest::Client,
    auth_base_url: String,
    fixture: bool,
    pending: Arc<PendingSettlements>,
}

#[derive(Default)]
struct PendingSettlements {
    count: AtomicUsize,
    changed: tokio::sync::Notify,
}

struct PendingSettlement(Arc<PendingSettlements>);
impl PendingSettlement {
    fn register(pending: Arc<PendingSettlements>) -> Self {
        pending.count.fetch_add(1, Ordering::AcqRel);
        Self(pending)
    }
}
impl Drop for PendingSettlement {
    fn drop(&mut self) {
        self.0.count.fetch_sub(1, Ordering::AcqRel);
        self.0.changed.notify_waiters();
    }
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[derive(Clone, Copy)]
enum CredentialSelection {
    LatestInteractive,
    ExactRow(i64),
}

impl OpenAiCodexAuth {
    /// The supplied client must disable redirects to keep OAuth grants at the
    /// authorized endpoint. Hosts may configure their own proxy/TLS policy.
    pub async fn open(db_path: PathBuf, client: reqwest::Client) -> Result<Self, CodexAuthError> {
        Self::open_inner(db_path, client, "https://auth.openai.com".into(), false).await
    }

    /// Controlled loopback upstream only; unavailable in ordinary CLI builds.
    #[cfg(feature = "test-fixture")]
    pub async fn open_with_endpoints(
        db_path: PathBuf,
        client: reqwest::Client,
        auth_base_url: &str,
    ) -> Result<Self, CodexAuthError> {
        let url = reqwest::Url::parse(auth_base_url).map_err(|_| CodexAuthError::InvalidEndpoint)?;
        if !loopback_url(&url) || !matches!(url.path(), "" | "/") {
            return Err(CodexAuthError::InvalidEndpoint);
        }
        Self::open_inner(db_path, client, auth_base_url.trim_end_matches('/').into(), true).await
    }

    async fn open_inner(
        db_path: PathBuf,
        client: reqwest::Client,
        auth_base_url: String,
        fixture: bool,
    ) -> Result<Self, CodexAuthError> {
        let store = tokio::task::spawn_blocking(move || {
            SqliteCredentialStore::open_with_busy_timeout(db_path, Duration::from_millis(250))
        })
        .await
        .map_err(|_| CodexAuthError::Storage)?
        .map_err(|_| CodexAuthError::Storage)?;
        Ok(Self {
            store: Arc::new(Mutex::new(store)),
            client,
            auth_base_url,
            fixture,
            pending: Arc::new(PendingSettlements::default()),
        })
    }

    async fn db<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&SqliteCredentialStore) -> anyhow::Result<T> + Send + 'static,
    ) -> Result<T, CodexAuthError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let store = store.lock().map_err(|_| CodexAuthError::Storage)?;
            operation(&store).map_err(|_| CodexAuthError::Storage)
        })
        .await
        .map_err(|_| CodexAuthError::Storage)?
    }

    pub async fn login_device(
        &self,
        cancel: &CancellationToken,
        on_auth: impl Fn(DeviceLoginInfo) + Send + Sync,
    ) -> Result<LoginIdentity, CodexAuthError> {
        let init = self.post_json("/api/accounts/deviceauth/usercode", json!({"client_id": CLIENT_ID}), cancel).await?;
        let device_id = required(&init, "device_auth_id")?.to_owned();
        let user_code = required(&init, "user_code")?.to_owned();
        let seconds = interval_seconds(init.get("interval"));
        let interval = if self.fixture { Duration::from_millis(5) } else { Duration::from_secs_f64(seconds + 3.0) };
        on_auth(DeviceLoginInfo { verification_url: DEVICE_AUTH_URL, user_code: user_code.clone() });
        for poll in 0..120 {
            let delay = if poll == 0 { interval.min(Duration::from_secs(5)) } else { interval };
            cancellable_sleep(delay, cancel).await?;
            let result = self
                .post_json(
                    "/api/accounts/deviceauth/token",
                    json!({
                        "device_auth_id": device_id, "user_code": user_code,
                    }),
                    cancel,
                )
                .await;
            let authorized = match result {
                Err(CodexAuthError::HttpStatus(403 | 404)) => continue,
                result => result?,
            };
            let code = required(&authorized, "authorization_code")?;
            let verifier = required(&authorized, "code_verifier")?;
            let token = self
                .post_token(
                    &[
                        ("grant_type", "authorization_code"),
                        ("client_id", CLIENT_ID),
                        ("code", code),
                        ("code_verifier", verifier),
                        ("redirect_uri", REDIRECT_URI),
                    ],
                    Duration::from_secs(15),
                    cancel,
                )
                .await?;
            let credential = login_credential(&token)?;
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled);
            }
            return self
                .db(move |store| {
                    let rows = store.upsert_auth_credential_for_provider(PROVIDER, &credential)?;
                    let fields = oauth_fields(&credential).expect("login constructs OAuth");
                    let account_id = fields["accountId"].as_str().expect("validated account").to_owned();
                    let access = fields["access"].as_str().expect("validated access");
                    let row = rows
                        .iter()
                        .find(|row| {
                            oauth_fields(&row.credential).and_then(|f| f.get("access")).and_then(Value::as_str)
                                == Some(access)
                        })
                        .ok_or_else(|| anyhow::anyhow!("login not persisted"))?;
                    Ok(LoginIdentity {
                        credential_id: row.id,
                        account_id,
                        email: fields.get("email").and_then(Value::as_str).map(str::to_owned),
                    })
                })
                .await;
        }
        Err(CodexAuthError::TimedOut)
    }

    pub async fn logout(&self) -> Result<(), CodexAuthError> {
        self.db(|store| store.delete_auth_credentials_for_provider(PROVIDER, "logged out")).await
    }

    /// Hosts await this after stopping Run admission and cancelling their calls,
    /// before normal process shutdown. Hard termination cannot run this barrier.
    pub async fn wait_for_settlement(&self) {
        loop {
            let notified = self.pending.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.pending.count.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    async fn post_json(&self, path: &str, body: Value, cancel: &CancellationToken) -> Result<Value, CodexAuthError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let request =
            self.client.post(format!("{}{path}", self.auth_base_url)).timeout(Duration::from_secs(15)).json(&body);
        response_json(request, cancel).await
    }

    async fn post_token(
        &self,
        form: &[(&str, &str)],
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, CodexAuthError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let request = self.client.post(format!("{}/oauth/token", self.auth_base_url)).timeout(timeout).form(form);
        response_json(request, cancel).await
    }

    /// Read-only observation for the synchronous Registry/extension interfaces.
    /// The lock covers only the SQLite snapshot, never a Host callback or network.
    pub fn stored_oauth_snapshot(&self) -> Result<Vec<StoredAuthCredential>, CodexAuthError> {
        let store = self.store.lock().map_err(|_| CodexAuthError::Storage)?;
        Ok(store
            .list_auth_credentials(Some(PROVIDER))
            .map_err(|_| CodexAuthError::Storage)?
            .into_iter()
            .filter(|row| oauth_fields(&row.credential).is_some())
            .collect())
    }

    /// Native discovery selection observes the persisted unscoped block. A
    /// damaged advisory block read does not make an otherwise valid row unusable.
    pub async fn discovery_blocked_ids(&self) -> Result<Vec<i64>, CodexAuthError> {
        self.db(|store| {
            let rows = store.list_auth_credentials(Some(PROVIDER))?;
            Ok(rows
                .into_iter()
                .filter_map(|row| {
                    store.get_credential_block(row.id, "openai-codex:oauth", "").ok().flatten().map(|_| row.id)
                })
                .collect())
        })
        .await
    }

    async fn selected(&self, selection: CredentialSelection) -> Result<StoredAuthCredential, CodexAuthError> {
        let rows = self.db(|store| store.list_auth_credentials(Some(PROVIDER))).await?;
        match selection {
            CredentialSelection::LatestInteractive => rows
                .into_iter()
                .filter_map(|row| {
                    let at = oauth_fields(&row.credential)?.get("authorizedAt")?.as_i64()?;
                    Some((at, row.id, row))
                })
                .max_by_key(|(at, id, _)| (*at, *id))
                .map(|(_, _, row)| row),
            CredentialSelection::ExactRow(id) => {
                rows.into_iter().find(|row| row.id == id && oauth_fields(&row.credential).is_some())
            }
        }
        .ok_or(CodexAuthError::LoginRequired)
    }

    /// Resolve only this durable account. A refresh fence never adopts a sibling.
    pub async fn resolve_discovery_account(
        &self,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        self.resolve_owned(CredentialSelection::ExactRow(id), cancel).await
    }

    async fn resolve_owned(
        &self,
        selection: CredentialSelection,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        let settlement = PendingSettlement::register(self.pending.clone());
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let child = cancel.child_token();
        let _cleanup = CancelOnDrop(child.clone());
        let service = self.clone();
        tokio::spawn(async move {
            let _settlement = settlement;
            service.resolve_credential(selection, &child).await
        })
        .await
        .map_err(|_| CodexAuthError::Storage)?
    }

    fn permitted_model(&self, model: &Model) -> bool {
        if model.provider != PROVIDER || model.api != "openai-codex-responses" {
            return false;
        }
        let Ok(url) = reqwest::Url::parse(&model.base_url) else {
            return false;
        };
        if self.fixture && loopback_url(&url) {
            return true;
        }
        url.scheme() == "https"
            && url.host_str() == Some("chatgpt.com")
            && url.port().is_none_or(|port| port == 443)
            && matches!(url.path(), "/backend-api" | "/backend-api/")
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
    }

    async fn resolve_credential(
        &self,
        selection: CredentialSelection,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        let wait_started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled);
            }
            let selected = self.selected(selection).await?;
            if fresh(&selected) {
                return Ok(selected);
            }
            // Validate the selected account before taking a lease; do not rotate
            // silently to an older account when its credentials are malformed.
            required(&oauth_value(&selected)?, "refresh")?;
            let id = selected.id;
            let owner = uuid::Uuid::new_v4().to_string();
            let acquire_owner = owner.clone();
            let acquired = self
                .db(move |store| {
                    store.try_acquire_credential_refresh_lease(id, &acquire_owner, now_ms() + LEASE_TTL_MS)
                })
                .await?;
            if !acquired {
                if wait_started.elapsed() >= Duration::from_secs(30) {
                    return Err(CodexAuthError::TimedOut);
                }
                cancellable_sleep(Duration::from_millis(50), cancel).await?;
                continue;
            }
            let result = self.refresh_owned(id, &owner, selection, cancel).await;
            let release_owner = owner.clone();
            self.db(move |store| store.release_credential_refresh_lease(id, &release_owner)).await?;
            match result {
                Ok(Some(row)) => return Ok(row),
                Ok(None) => {
                    // CAS/fence loss: only a persisted, currently selected fresh
                    // credential may be returned. Never use the uncommitted token.
                    let current = self.selected(selection).await?;
                    return if fresh(&current) { Ok(current) } else { Err(CodexAuthError::LoginRequired) };
                }
                Err(error) => return Err(error),
            }
        }
    }

    async fn refresh_owned(
        &self,
        id: i64,
        owner: &str,
        selection: CredentialSelection,
        cancel: &CancellationToken,
    ) -> Result<Option<StoredAuthCredential>, CodexAuthError> {
        // Re-read after acquiring the durable lease, never refresh the pre-lease
        // snapshot. Login/logout from another connection may already have won.
        let current = self.selected(selection).await?;
        if current.id != id {
            return Ok(if fresh(&current) { Some(current) } else { None });
        }
        if fresh(&current) {
            return Ok(Some(current));
        }
        let fields = oauth_fields(&current.credential).ok_or(CodexAuthError::LoginRequired)?;
        let refresh = fields
            .get("refresh")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(CodexAuthError::LoginRequired)?
            .to_owned();
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let dispatched = AtomicBool::new(false);
        let form = [("grant_type", "refresh_token"), ("client_id", CLIENT_ID), ("refresh_token", &refresh)];
        let request = self
            .client
            .post(format!("{}/oauth/token", self.auth_base_url))
            .timeout(Duration::from_secs(10))
            .form(&form);
        let network = response_json_tracked(request, cancel, Some(&dispatched));
        tokio::pin!(network);
        let result = loop {
            tokio::select! {
                result = &mut network => break result,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {
                    let owner = owner.to_owned();
                    let renewed = self.db(move |store| {
                        store.renew_credential_refresh_lease(id, &owner, now_ms() + LEASE_TTL_MS)
                    }).await;
                    if !matches!(renewed, Ok(true)) { break Err(CodexAuthError::Transport); }
                }
            }
        };
        if result.is_err() && !dispatched.load(Ordering::SeqCst) {
            return result.map(|_| None);
        }
        let updated = result.and_then(|token| refreshed_credential(&current.credential, &token));
        let expected = current.serialized_data.clone();
        let owner = owner.to_owned();
        match updated {
            Ok(credential) => {
                let commit_expected = expected.clone();
                let commit_owner = owner.clone();
                let committed = self
                    .db(move |store| {
                        store.try_update_auth_credential_if_matches(
                            id,
                            &commit_expected,
                            &credential,
                            Some(&CredentialRefreshLeaseFence { owner: commit_owner, now_ms: now_ms() }),
                        )
                    })
                    .await;
                let committed = match committed {
                    Ok(committed) => committed,
                    Err(error) => {
                        // A storage error can occur after SQL committed. First
                        // consult the durable row; otherwise fence the unchanged
                        // grant so a later request cannot replay it automatically.
                        let recovered = self.adopt_or_disable_after_fence_loss(id, expected, selection).await?;
                        if recovered.is_some() {
                            return Ok(recovered);
                        }
                        return Err(error);
                    }
                };
                if !committed {
                    return self.adopt_or_disable_after_fence_loss(id, expected, selection).await;
                }
                let persisted = self.selected(selection).await?;
                Ok(if fresh(&persisted) { Some(persisted) } else { None })
            }
            Err(error) => {
                // No code or refresh grant is retried automatically. Even a
                // malformed successful response can have rotated the grant.
                let cause = if matches!(error, CodexAuthError::HttpStatus(_)) {
                    "refresh rejected; login required"
                } else {
                    UNKNOWN_CAUSE
                };
                let disabled = self
                    .db(move |store| {
                        store.try_disable_auth_credential_if_matches(
                            id,
                            &expected,
                            cause,
                            Some(&CredentialRefreshLeaseFence { owner, now_ms: now_ms() }),
                        )
                    })
                    .await?;
                if disabled {
                    Err(error)
                } else {
                    self.adopt_or_disable_after_fence_loss(id, current.serialized_data, selection).await
                }
            }
        }
    }

    async fn adopt_or_disable_after_fence_loss(
        &self,
        id: i64,
        expected: String,
        selection: CredentialSelection,
    ) -> Result<Option<StoredAuthCredential>, CodexAuthError> {
        let current = self.selected(selection).await?;
        if fresh(&current) {
            return Ok(Some(current));
        }
        if current.id == id && current.serialized_data == expected {
            // A lost lease is not permission to overwrite a peer's data. An
            // unchanged old grant with an unknown outcome must nevertheless be
            // fenced from automatic replay, including a peer pending on that
            // same grant. Exact raw-data CAS cannot disable any fresh new row.
            self.db(move |store| store.try_disable_auth_credential_if_matches(id, &expected, UNKNOWN_CAUSE, None))
                .await?;
        }
        Ok(None)
    }
}

#[async_trait]
impl RequestAuthResolver for OpenAiCodexAuth {
    async fn resolve(&self, model: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        if !self.permitted_model(model) {
            return Err(AuthResolveError::Unavailable);
        }
        self.resolve_owned(CredentialSelection::LatestInteractive, cancel).await.and_then(lease_from_row).map_err(
            |error| match error {
                CodexAuthError::Cancelled => AuthResolveError::Cancelled,
                CodexAuthError::Storage => AuthResolveError::Storage,
                CodexAuthError::LoginRequired | CodexAuthError::InvalidEndpoint => AuthResolveError::Unavailable,
                _ => AuthResolveError::Refresh,
            },
        )
    }
}

async fn response_json(request: reqwest::RequestBuilder, cancel: &CancellationToken) -> Result<Value, CodexAuthError> {
    response_json_tracked(request, cancel, None).await
}

async fn response_json_tracked(
    request: reqwest::RequestBuilder,
    cancel: &CancellationToken,
    dispatched: Option<&AtomicBool>,
) -> Result<Value, CodexAuthError> {
    let operation = async {
        if let Some(dispatched) = dispatched {
            dispatched.store(true, Ordering::SeqCst);
        }
        let response = request.send().await.map_err(|_| CodexAuthError::Transport)?;
        if !response.status().is_success() {
            return Err(CodexAuthError::HttpStatus(response.status().as_u16()));
        }
        response.json().await.map_err(|_| CodexAuthError::InvalidResponse)
    };
    tokio::select! { biased; _ = cancel.cancelled() => Err(CodexAuthError::Cancelled), result = operation => result }
}

async fn cancellable_sleep(delay: Duration, cancel: &CancellationToken) -> Result<(), CodexAuthError> {
    tokio::select! { biased; _ = cancel.cancelled() => Err(CodexAuthError::Cancelled), _ = tokio::time::sleep(delay) => Ok(()) }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis().min(i64::MAX as u128) as i64).unwrap_or(0)
}

fn loopback_url(url: &reqwest::Url) -> bool {
    let host = url.host_str().unwrap_or_default();
    url.scheme() == "http"
        && (host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()))
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, CodexAuthError> {
    value.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or(CodexAuthError::InvalidResponse)
}

fn oauth_fields(credential: &AuthCredential) -> Option<&Map<String, Value>> {
    match credential {
        AuthCredential::OAuth { fields } => Some(fields),
        _ => None,
    }
}

fn oauth_value(row: &StoredAuthCredential) -> Result<Value, CodexAuthError> {
    oauth_fields(&row.credential).cloned().map(Value::Object).ok_or(CodexAuthError::LoginRequired)
}

fn fresh(row: &StoredAuthCredential) -> bool {
    oauth_fields(&row.credential)
        .and_then(|fields| fields.get("expires"))
        .and_then(Value::as_i64)
        .is_some_and(|expires| expires > now_ms().saturating_add(REFRESH_SKEW_MS))
}

fn interval_seconds(value: Option<&Value>) -> f64 {
    let seconds = value.and_then(|v| {
        v.as_f64().or_else(|| {
            v.as_str().and_then(|s| {
                let s = s.trim_start();
                let end = s
                    .char_indices()
                    .find(|(i, c)| !c.is_ascii_digit() && !(*i == 0 && (*c == '+' || *c == '-')))
                    .map_or(s.len(), |(i, _)| i);
                s[..end].parse::<i64>().ok().filter(|v| *v != 0).map(|v| v as f64)
            })
        })
    });
    seconds.filter(|s| s.is_finite() && *s >= 0.0 && *s <= 86_400.0).unwrap_or(5.0)
}

fn decode_jwt(token: &str) -> Option<Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1])
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(parts[1]).ok())?;
    serde_json::from_slice(&bytes).ok()
}

/// JWT decoding is profile extraction, not signature or authorization validation.
fn profile(access: &str, id_token: Option<&str>) -> (Option<String>, Option<String>, Option<String>) {
    let access = decode_jwt(access);
    let id = id_token.and_then(decode_jwt);
    let claim = |path: &str, key: &str| {
        access
            .as_ref()
            .and_then(|v| v.get(path))
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .or_else(|| id.as_ref().and_then(|v| v.get(path)).and_then(|v| v.get(key)).and_then(Value::as_str))
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let normalize = |value: Option<String>| value.map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty());
    (
        claim(AUTH_CLAIM, "chatgpt_account_id"),
        normalize(claim(PROFILE_CLAIM, "email")),
        normalize(claim(AUTH_CLAIM, "chatgpt_plan_type")),
    )
}

fn token_expiry(token: &Value) -> Result<i64, CodexAuthError> {
    let seconds = token
        .get("expires_in")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= (i64::MAX / 1000) as f64)
        .ok_or(CodexAuthError::InvalidResponse)?;
    Ok(now_ms().saturating_add((seconds * 1000.0) as i64))
}

fn login_credential(token: &Value) -> Result<AuthCredential, CodexAuthError> {
    let access = required(token, "access_token")?;
    let refresh = required(token, "refresh_token")?;
    let (account, email, plan) = profile(access, token.get("id_token").and_then(Value::as_str));
    let account = account.ok_or(CodexAuthError::InvalidResponse)?;
    let mut fields = Map::new();
    fields.insert("access".into(), access.into());
    fields.insert("refresh".into(), refresh.into());
    fields.insert("expires".into(), token_expiry(token)?.into());
    fields.insert("accountId".into(), account.clone().into());
    fields.insert("orgId".into(), account.into());
    fields.insert("authorizedAt".into(), now_ms().into());
    if let Some(email) = email {
        fields.insert("email".into(), email.into());
    }
    if let Some(plan) = plan {
        fields.insert("orgName".into(), plan.into());
    }
    Ok(AuthCredential::OAuth { fields })
}

fn refreshed_credential(stored: &AuthCredential, token: &Value) -> Result<AuthCredential, CodexAuthError> {
    let mut fields = oauth_fields(stored).cloned().ok_or(CodexAuthError::LoginRequired)?;
    let access = required(token, "access_token")?;
    let (account, email, _) = profile(access, token.get("id_token").and_then(Value::as_str));
    fields.insert("access".into(), access.into());
    fields.insert("expires".into(), token_expiry(token)?.into());
    if let Some(refresh) = token.get("refresh_token").and_then(Value::as_str).filter(|s| !s.is_empty()) {
        fields.insert("refresh".into(), refresh.into());
    }
    if let Some(account) = account {
        fields.insert("accountId".into(), account.into());
    }
    if let Some(email) = email {
        fields.insert("email".into(), email.into());
    }
    Ok(AuthCredential::OAuth { fields })
}

fn lease_from_row(row: StoredAuthCredential) -> Result<RequestAuthLease, CodexAuthError> {
    let fields = oauth_fields(&row.credential).ok_or(CodexAuthError::LoginRequired)?;
    let access =
        fields.get("access").and_then(Value::as_str).filter(|v| !v.is_empty()).ok_or(CodexAuthError::LoginRequired)?;
    let account = profile(access, None)
        .0
        .or_else(|| fields.get("accountId").and_then(Value::as_str).map(str::to_owned))
        .filter(|v| !v.is_empty())
        .ok_or(CodexAuthError::LoginRequired)?;
    let mut headers = vec![("chatgpt-account-id".into(), account)];
    if let Some(payload) = decode_jwt(access)
        && let Some(auth) = payload.get(AUTH_CLAIM)
        && let Some(residency) = ["chatgpt_data_residency", "chatgpt_compute_residency"]
            .into_iter()
            .filter_map(|key| auth.get(key).and_then(Value::as_str))
            .map(str::trim)
            .find(|v| !v.is_empty())
    {
        headers.push(("x-openai-internal-codex-residency".into(), residency.into()));
    }
    Ok(RequestAuthLease::new(CredentialIdentity::Stored { id: row.id, revision: row.revision }, Some(access.into()))
        .with_headers(headers))
}
