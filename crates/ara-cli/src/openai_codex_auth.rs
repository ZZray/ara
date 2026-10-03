//! Host-owned OpenAI Codex device authorization and per-request OAuth refresh.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `packages/ai/src/registry/oauth/openai-codex.ts`, `auth/auth-storage.ts`,
//! and `packages/catalog/src/wire/codex.ts`. No existing installation is read.
//!
//! Authorizing unknown refresh outcomes disable the unchanged credential.
//! Advisory usage preserves it while a durable grant fingerprint prevents
//! replay. A detached worker settles shared refresh even if a consumer drops.
//! SQLite locks never span a network operation.
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
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

#[path = "openai_codex_device_login.rs"]
mod device_login;
pub use device_login::OpenAiCodexDeviceLogin;

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
    /// Confirmed refresh rejection. The private body classifier's decision
    /// survives Host coordination; a bare HTTP status cannot reconstruct it.
    RefreshRejected {
        status: u16,
        definitive: bool,
    },
    InvalidResponse,
    TimedOut,
    LoginRequired,
    OutcomeUnknown,
    /// Internal zero-dispatch signal: join the current grant's flight instead
    /// of executing a changed grant under an older activity identity.
    CredentialChanged,
    InvalidEndpoint,
}

impl std::fmt::Display for CodexAuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("OpenAI Codex authentication cancelled"),
            Self::Storage => f.write_str("OpenAI Codex authentication storage failed"),
            Self::Transport => f.write_str("OpenAI Codex authentication transport failed"),
            Self::HttpStatus(status) => write!(f, "OpenAI Codex authentication HTTP {status}"),
            Self::RefreshRejected { status, .. } => write!(f, "OpenAI Codex refresh HTTP {status}"),
            Self::InvalidResponse => f.write_str("OpenAI Codex authentication response is invalid"),
            Self::TimedOut => f.write_str("OpenAI Codex authentication timed out"),
            Self::LoginRequired => f.write_str("OpenAI Codex login required"),
            Self::OutcomeUnknown => f.write_str("OpenAI Codex authentication outcome unknown"),
            Self::CredentialChanged => f.write_str("OpenAI Codex credential changed before refresh"),
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
    refresh_flights: Arc<Mutex<HashMap<String, RefreshFlight>>>,
}

type RefreshResult = Result<StoredAuthCredential, CodexAuthError>;
struct RefreshFlight {
    identity: uuid::Uuid,
    receiver: watch::Receiver<Option<RefreshResult>>,
    cancel: CancellationToken,
    consumers: usize,
    usage: bool,
    policy: Arc<RefreshPolicy>,
}
#[derive(Default)]
struct RefreshPolicy {
    authorizing: AtomicBool,
    dispatch_data: Mutex<Option<String>>,
}
struct RefreshConsumer {
    flights: Arc<Mutex<HashMap<String, RefreshFlight>>>,
    key: String,
    identity: uuid::Uuid,
    registered: bool,
}
impl RefreshConsumer {
    fn release(&mut self) -> bool {
        if !self.registered {
            return false;
        }
        self.registered = false;
        let Ok(mut flights) = self.flights.lock() else { return false };
        let Some(flight) = flights.get_mut(&self.key).filter(|flight| flight.identity == self.identity) else {
            return false;
        };
        flight.consumers -= 1;
        if flight.consumers == 0 && !flight.usage {
            // Registration and this decision share the same lock. A caller
            // already sharing the grant cannot lose its owner to cancellation.
            flight.cancel.cancel();
            return true;
        }
        false
    }
}
impl Drop for RefreshConsumer {
    fn drop(&mut self) {
        self.release();
    }
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

#[derive(Default)]
struct RefreshObservation {
    dispatched: AtomicBool,
    rejected: AtomicBool,
    definitive: AtomicBool,
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
            refresh_flights: Arc::new(Mutex::new(HashMap::new())),
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

    /// The native Host auth owner uses the same database and refresh settlement
    /// service as account discovery. A second independently cached store would
    /// lose the row ownership established by the request's acquired lease.
    pub(crate) fn store_handle(&self) -> Arc<Mutex<SqliteCredentialStore>> {
        self.store.clone()
    }

    pub async fn login_device(
        &self,
        cancel: &CancellationToken,
        on_auth: impl Fn(DeviceLoginInfo) + Send + Sync,
    ) -> Result<LoginIdentity, CodexAuthError> {
        let issuer =
            OpenAiCodexDeviceLogin::from_transport(self.client.clone(), self.auth_base_url.clone(), self.fixture);
        let credential = issuer.issue(cancel, on_auth).await?;
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        self.db(move |store| {
            let rows = store.upsert_auth_credential_for_provider(PROVIDER, &credential)?;
            let fields = oauth_fields(&credential).expect("login constructs OAuth");
            let account_id = fields["accountId"].as_str().expect("validated account").to_owned();
            let access = fields["access"].as_str().expect("validated access");
            let row = rows
                .iter()
                .find(|row| {
                    oauth_fields(&row.credential).and_then(|f| f.get("access")).and_then(Value::as_str) == Some(access)
                })
                .ok_or_else(|| anyhow::anyhow!("login not persisted"))?;
            Ok(LoginIdentity {
                credential_id: row.id,
                account_id,
                email: fields.get("email").and_then(Value::as_str).map(str::to_owned),
            })
        })
        .await
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

    /// Native auth step (b): refresh this exact durable row even when its access
    /// token has not expired. Peers that rotate it while the lease is acquired
    /// win; this operation never adopts a sibling or replays a lost grant.
    pub async fn resolve_account_credential(
        &self,
        id: i64,
        force_refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        self.resolve_owned_with_refresh(CredentialSelection::ExactRow(id), force_refresh, cancel).await
    }

    async fn resolve_owned(
        &self,
        selection: CredentialSelection,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        self.resolve_owned_with_refresh(selection, false, cancel).await
    }

    async fn resolve_owned_with_refresh(
        &self,
        selection: CredentialSelection,
        force_refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        self.resolve_shared(selection, force_refresh, false, cancel).await
    }

    /// Usage polling refreshes the same exact account and grant as requests.
    /// Durable definitive removal becomes the native wrapper's non-definitive
    /// missing-row error; this advisory wrapper adds no credential disable.
    pub async fn prepare_usage_account_credential(
        &self,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        self.resolve_shared(CredentialSelection::ExactRow(id), false, true, cancel).await.map_err(|error| match error {
            CodexAuthError::RefreshRejected { definitive: true, .. } => CodexAuthError::LoginRequired,
            other => other,
        })
    }

    async fn resolve_shared(
        &self,
        selection: CredentialSelection,
        force_refresh: bool,
        usage: bool,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        if cancel.is_cancelled() {
            return Err(CodexAuthError::Cancelled);
        }
        let started = Instant::now();
        let mut force_observed = None;
        'acquire: loop {
            if started.elapsed() >= Duration::from_secs(30) {
                return Err(CodexAuthError::TimedOut);
            }
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled);
            }
            let observed = self.selected(selection).await?;
            if force_refresh && force_observed.is_none() {
                force_observed = Some(observed.serialized_data.clone());
            }
            if fresh(&observed) && force_observed.as_ref() != Some(&observed.serialized_data) {
                return Ok(observed);
            }
            let refresh = required(&oauth_value(&observed)?, "refresh")?.to_owned();
            let key = refresh_dispatch_key(observed.id, &refresh);
            let expected_data = observed.serialized_data.clone();
            let (mut receiver, mut consumer) = {
                let mut flights = self.refresh_flights.lock().map_err(|_| CodexAuthError::Storage)?;
                if !flights.contains_key(&key) {
                    let identity = uuid::Uuid::new_v4();
                    let child = CancellationToken::new();
                    let policy = Arc::new(RefreshPolicy {
                        authorizing: AtomicBool::new(!usage),
                        dispatch_data: Mutex::new(None),
                    });
                    let (sender, receiver) = watch::channel(None);
                    flights.insert(
                        key.clone(),
                        RefreshFlight {
                            identity,
                            receiver,
                            cancel: child.clone(),
                            consumers: 0,
                            usage,
                            policy: policy.clone(),
                        },
                    );
                    let settlement = PendingSettlement::register(self.pending.clone());
                    let service = self.clone();
                    let flight_key = key.clone();
                    let expected = force_observed.clone();
                    let exact_selection = CredentialSelection::ExactRow(observed.id);
                    tokio::spawn(async move {
                        let _settlement = settlement;
                        let mut result =
                            service.resolve_credential(exact_selection, expected, &child, &policy, &flight_key).await;
                        // Close admission under the registration lock before
                        // final Unknown disposition. A late request/drop is now
                        // either included in this policy or starts a fenced new
                        // flight; neither timing can lose authorizing settlement.
                        if let Ok(mut flights) = service.refresh_flights.lock()
                            && flights.get(&flight_key).is_some_and(|flight| flight.identity == identity)
                        {
                            flights.remove(&flight_key);
                        }
                        if matches!(result, Err(CodexAuthError::OutcomeUnknown))
                            && policy.authorizing.load(Ordering::Acquire)
                        {
                            let expected = policy.dispatch_data.lock().ok().and_then(|value| value.clone());
                            if let Some(expected) = expected {
                                let key = flight_key.clone();
                                let id = observed.id;
                                if let Err(error) = service
                                    .db(move |store| {
                                        if store.get_cache(&key, true)?.is_some() {
                                            store.try_disable_auth_credential_if_matches(
                                                id,
                                                &expected,
                                                UNKNOWN_CAUSE,
                                                None,
                                            )?;
                                        }
                                        Ok(())
                                    })
                                    .await
                                {
                                    result = Err(error);
                                }
                            } else {
                                result = Err(CodexAuthError::Storage);
                            }
                        }
                        sender.send_replace(Some(result));
                    });
                }
                let flight = flights.get_mut(&key).ok_or(CodexAuthError::Storage)?;
                flight.consumers += 1;
                flight.usage |= usage;
                if !usage {
                    flight.policy.authorizing.store(true, Ordering::Release);
                }
                (
                    flight.receiver.clone(),
                    RefreshConsumer {
                        flights: self.refresh_flights.clone(),
                        key,
                        identity: flight.identity,
                        registered: true,
                    },
                )
            };
            loop {
                if cancel.is_cancelled() {
                    let cancelled_owner = consumer.release();
                    if cancelled_owner {
                        // Preserve the existing sole authorizing caller's settled
                        // cancellation contract. Advisory/shared owners keep running.
                        while receiver.borrow().is_none() {
                            if receiver.changed().await.is_err() {
                                break;
                            }
                        }
                    }
                    return Err(CodexAuthError::Cancelled);
                }
                let result = receiver.borrow().clone();
                if let Some(result) = result {
                    if matches!(result, Err(CodexAuthError::CredentialChanged)) {
                        continue 'acquire;
                    }
                    if !usage && matches!(result, Err(CodexAuthError::OutcomeUnknown)) {
                        // A request may join after an advisory-only worker chose its
                        // Unknown disposition but before the receipt was published.
                        // Its exact-row authorizing settlement still applies.
                        let id = observed.id;
                        let expected = expected_data.clone();
                        let key = consumer.key.clone();
                        self.db(move |store| {
                            if store.get_cache(&key, true)?.is_some() {
                                store.try_disable_auth_credential_if_matches(id, &expected, UNKNOWN_CAUSE, None)?;
                            }
                            Ok(())
                        })
                        .await?;
                    }
                    return result;
                }
                tokio::select! {
                    _ = cancel.cancelled() => {},
                    changed = receiver.changed() => if changed.is_err() { return Err(CodexAuthError::Storage) },
                }
            }
        }
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
        force_expected: Option<String>,
        cancel: &CancellationToken,
        policy: &RefreshPolicy,
        flight_key: &str,
    ) -> Result<StoredAuthCredential, CodexAuthError> {
        let wait_started = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Err(CodexAuthError::Cancelled);
            }
            let selected = self.selected(selection).await?;
            if refresh_dispatch_key(selected.id, required(&oauth_value(&selected)?, "refresh")?) != flight_key {
                return Err(CodexAuthError::CredentialChanged);
            }
            if fresh(&selected) && force_expected.as_ref() != Some(&selected.serialized_data) {
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
            let result = self.refresh_owned(id, &owner, force_expected.as_deref(), cancel, policy, flight_key).await;
            let release_owner = owner.clone();
            self.db(move |store| store.release_credential_refresh_lease(id, &release_owner)).await?;
            match result {
                Ok(Some(row)) => return Ok(row),
                Ok(None) => return Err(CodexAuthError::LoginRequired),
                Err(error) => return Err(error),
            }
        }
    }

    async fn refresh_owned(
        &self,
        id: i64,
        owner: &str,
        force_expected: Option<&str>,
        cancel: &CancellationToken,
        policy: &RefreshPolicy,
        flight_key: &str,
    ) -> Result<Option<StoredAuthCredential>, CodexAuthError> {
        // Re-read after acquiring the durable lease, never refresh the pre-lease
        // snapshot. Login/logout from another connection may already have won.
        let selection = CredentialSelection::ExactRow(id);
        let current = self.selected(selection).await?;
        if refresh_dispatch_key(current.id, required(&oauth_value(&current)?, "refresh")?) != flight_key {
            return Err(CodexAuthError::CredentialChanged);
        }
        if current.id != id {
            return Ok(if fresh(&current) { Some(current) } else { None });
        }
        if fresh(&current) && force_expected != Some(current.serialized_data.as_str()) {
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
        let dispatch_key = refresh_dispatch_key(id, &refresh);
        *policy.dispatch_data.lock().map_err(|_| CodexAuthError::Storage)? = Some(current.serialized_data.clone());
        let begin_expected = current.serialized_data.clone();
        let begin_key = dispatch_key.clone();
        let begin_owner = owner.to_owned();
        let began = self
            .db(move |store| {
                store.try_begin_oauth_refresh(
                    id,
                    &begin_expected,
                    &begin_key,
                    &CredentialRefreshLeaseFence { owner: begin_owner, now_ms: now_ms() },
                )
            })
            .await?;
        if !began {
            let prior_key = dispatch_key.clone();
            if self.db(move |store| store.get_cache(&prior_key, true)).await?.is_some() {
                let peer =
                    self.adopt_or_disable_after_fence_loss(id, current.serialized_data, selection, policy).await?;
                return peer.map(Some).ok_or(CodexAuthError::OutcomeUnknown);
            }
            // No grant was dispatched. A row/lease change in the small gap
            // before marker acquisition is a reload, never an unknown effect.
            let peer = self.selected(selection).await?;
            if refresh_dispatch_key(peer.id, required(&oauth_value(&peer)?, "refresh")?) != flight_key {
                return Err(CodexAuthError::CredentialChanged);
            }
            return Ok(if peer.id == id && refreshed_token_changed(&current.serialized_data, &peer) && fresh(&peer) {
                Some(peer)
            } else {
                None
            });
        }
        let observation = RefreshObservation::default();
        let form = [("grant_type", "refresh_token"), ("client_id", CLIENT_ID), ("refresh_token", &refresh)];
        let request = self
            .client
            .post(format!("{}/oauth/token", self.auth_base_url))
            .timeout(Duration::from_secs(10))
            .form(&form);
        let network = response_json_tracked(request, cancel, Some(&observation));
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
        let result = result.map_err(|error| match error {
            CodexAuthError::HttpStatus(status) if observation.rejected.load(Ordering::SeqCst) => {
                CodexAuthError::RefreshRejected { status, definitive: observation.definitive.load(Ordering::SeqCst) }
            }
            other => other,
        });
        if result.is_err() && !observation.dispatched.load(Ordering::SeqCst) {
            let key = dispatch_key;
            let owner = owner.to_owned();
            self.db(move |store| store.clear_oauth_refresh_dispatch(&key, &owner)).await?;
            return result.map(|_| None);
        }
        if observation.rejected.load(Ordering::SeqCst) {
            let key = dispatch_key.clone();
            let owner = owner.to_owned();
            self.db(move |store| store.clear_oauth_refresh_dispatch(&key, &owner)).await?;
        }
        if result.is_err()
            && observation.rejected.load(Ordering::SeqCst)
            && !observation.definitive.load(Ordering::SeqCst)
        {
            // A received transient HTTP rejection confirms that this attempt
            // did not return a rotated grant. Preserve the row; AuthStorage owns
            // its request-scoped five-minute refresh-failure backoff. Transport
            // loss/cancellation after dispatch remains an unknown outcome below.
            return result.map(|_| None);
        }
        let updated = result.and_then(|token| refreshed_credential(&current.credential, &token));
        let expected = current.serialized_data.clone();
        let owner = owner.to_owned();
        match updated {
            Ok(credential) => {
                let submitted = credential.clone();
                let commit_expected = expected.clone();
                let commit_owner = owner.clone();
                let commit_key = dispatch_key.clone();
                let committed = self
                    .db(move |store| {
                        store.try_commit_oauth_refresh(
                            id,
                            &commit_expected,
                            &credential,
                            &commit_key,
                            &CredentialRefreshLeaseFence { owner: commit_owner, now_ms: now_ms() },
                        )
                    })
                    .await;
                let committed = match committed {
                    Ok(committed) => committed,
                    Err(error) => {
                        // A storage error can occur after SQL committed. First
                        // consult the durable row; otherwise fence the unchanged
                        // grant so a later request cannot replay it automatically.
                        let persisted = self.selected(selection).await?;
                        let recovery_key = dispatch_key.clone();
                        if persisted.id == id
                            && persisted.credential == submitted
                            && valid_now(&persisted)
                            && self.db(move |store| store.get_cache(&recovery_key, true)).await?.is_none()
                        {
                            return Ok(Some(persisted));
                        }
                        let recovered = self.adopt_or_disable_after_fence_loss(id, expected, selection, policy).await?;
                        if recovered.is_some() {
                            return Ok(recovered);
                        }
                        return Err(error);
                    }
                };
                if !committed {
                    return self.adopt_or_disable_after_fence_loss(id, expected, selection, policy).await;
                }
                let persisted = self.selected(selection).await?;
                Ok(if persisted.id == id && persisted.credential == submitted {
                    valid_now(&persisted).then_some(persisted)
                } else {
                    fresh(&persisted).then_some(persisted)
                })
            }
            Err(error) => {
                // No code or refresh grant is retried automatically. Even a
                // malformed successful response can have rotated the grant.
                let definitive = matches!(error, CodexAuthError::RefreshRejected { definitive: true, .. });
                if !definitive && !policy.authorizing.load(Ordering::Acquire) {
                    // Advisory polling preserves the native row lifecycle. Its
                    // persistent dispatch record still forbids grant replay.
                    return Err(CodexAuthError::OutcomeUnknown);
                }
                let cause = if definitive { "refresh rejected; login required" } else { UNKNOWN_CAUSE };
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
                    self.adopt_or_disable_after_fence_loss(id, current.serialized_data, selection, policy).await
                }
            }
        }
    }

    async fn adopt_or_disable_after_fence_loss(
        &self,
        id: i64,
        expected: String,
        selection: CredentialSelection,
        policy: &RefreshPolicy,
    ) -> Result<Option<StoredAuthCredential>, CodexAuthError> {
        let current = self.selected(selection).await?;
        if current.id == id && current.serialized_data == expected && policy.authorizing.load(Ordering::Acquire) {
            // A lost lease is not permission to overwrite a peer's data. An
            // unchanged old grant with an unknown outcome must nevertheless be
            // fenced from automatic replay, including a peer pending on that
            // same grant. Exact raw-data CAS cannot disable any fresh new row.
            self.db(move |store| store.try_disable_auth_credential_if_matches(id, &expected, UNKNOWN_CAUSE, None))
                .await?;
        } else if current.id == id && refreshed_token_changed(&expected, &current) && fresh(&current) {
            return Ok(Some(current));
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
    observation: Option<&RefreshObservation>,
) -> Result<Value, CodexAuthError> {
    let operation = async {
        if let Some(observation) = observation {
            observation.dispatched.store(true, Ordering::SeqCst);
        }
        let response = request.send().await.map_err(|_| CodexAuthError::Transport)?;
        if !response.status().is_success() {
            let status = response.status().as_u16();
            if let Some(observation) = observation {
                observation.rejected.store(true, Ordering::SeqCst);
                // Fixed registry/engine/common.ts uses raw text, including
                // non-JSON errors, and slices the first 500 UTF-16 units.
                let Ok(private_body) = response.text().await else {
                    return Err(CodexAuthError::HttpStatus(status));
                };
                let excerpt = String::from_utf16_lossy(&private_body.encode_utf16().take(500).collect::<Vec<_>>());
                let text = format!("HTTP {status}: {excerpt}");
                observation.definitive.store(definitive_oauth_failure(&text), Ordering::SeqCst);
            }
            return Err(CodexAuthError::HttpStatus(status));
        }
        response.json().await.map_err(|_| CodexAuthError::InvalidResponse)
    };
    tokio::select! { biased; _ = cancel.cancelled() => Err(CodexAuthError::Cancelled), result = operation => result }
}

fn definitive_oauth_failure(text: &str) -> bool {
    use std::sync::LazyLock;
    // Fixed error/flags.ts:245-249. Error payloads are classification input
    // only; no provider body or bearer enters diagnostics or a durable cause.
    static DEFINITIVE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)invalid_grant|invalid_token|unauthorized_client|\brevoked\b|refresh[\s_]?token.*expired",
        )
        .expect("fixed OAuth classifier")
    });
    static TRANSIENT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?i)timeout|network|fetch failed|ECONN(?:REFUSED|RESET)|ETIMEDOUT|EAI_AGAIN|socket hang up|\b(?:408|425|429|5\d{2})\b|rate.?limit|too many requests|temporar|unavailable|forbidden|permission_denied|cloudflare|captcha")
            .expect("fixed OAuth transient classifier")
    });
    static HTTP_AUTH: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"\b401\b").expect("fixed OAuth status classifier"));
    DEFINITIVE.is_match(text) || (HTTP_AUTH.is_match(text) && !TRANSIENT.is_match(text))
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

fn valid_now(row: &StoredAuthCredential) -> bool {
    oauth_fields(&row.credential)
        .and_then(|fields| fields.get("expires"))
        .and_then(Value::as_f64)
        .is_some_and(|expires| expires > now_ms() as f64)
}

fn refresh_dispatch_key(id: i64, refresh: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, refresh.as_bytes());
    let fingerprint: String = hash.as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
    format!("oauth-refresh-pending:{PROVIDER}:{id}:{fingerprint}")
}

fn refreshed_token_changed(expected: &str, current: &StoredAuthCredential) -> bool {
    let Ok(original) = serde_json::from_str::<Value>(expected) else { return false };
    let Some(fields) = oauth_fields(&current.credential) else { return false };
    ["access", "refresh"]
        .into_iter()
        .any(|key| original.get(key).and_then(Value::as_str) != fields.get(key).and_then(Value::as_str))
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

pub(crate) fn lease_from_row(row: StoredAuthCredential) -> Result<RequestAuthLease, CodexAuthError> {
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
