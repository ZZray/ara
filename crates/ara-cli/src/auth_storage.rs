//! Host AuthStorage coordinator for fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d, packages/ai/src/auth-storage.ts.
//! Native source ranges: 1588–1785, 2132–2285, 2924–2993,
//! 3128–3524, 3981–4013, 4538–4698, 4805–5750, 5761–6058,
//! 6248–6298, 6317–6568 and 6665–6835. Pure ranking and assignment
//! state are shared with auth_storage_policy and auth_storage_state.
//!
//! Config commands, environment access, fallback, usage transport and generic
//! OAuth are supplied by the Host. The coordinator does not read private
//! configuration or open a second database. External OAuth providers own their
//! durable CAS/lease and settlement contract, as OpenAiCodexAuth already does.
//!
//! MIT License
//!
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in all
//! copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
//! OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
//! SOFTWARE.

use crate::{
    auth_storage_policy::{
        self as policy, CredentialRankingContext, CredentialRankingStrategy, PlanRequirement, UsageCandidate,
        UsageCredential, UsageCredentialType, UsageHistoryEntry, UsageRankedCandidate, UsageReport,
    },
    auth_storage_state::{
        AuthStorageState, CredentialKind, CredentialTarget, OAuthAccountSummary, provider_type_key,
        stored_credential_arrays_equal,
    },
    credential_store::{
        AuthCredential, SqliteCredentialStore, StoredAuthCredential, StoredCredentialBlock, USAGE_REPORT_TTL_MS,
    },
    model_route::{
        AuthResolveError, AuthRetryAction, CredentialIdentity, RequestAuthFailure, RequestAuthFailureKind,
        RequestAuthLease, RequestAuthResolver,
    },
    openai_codex_auth::{CodexAuthError, OpenAiCodexAuth, lease_from_row},
};
use ara_ai::Model;
use async_trait::async_trait;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

const REFRESH_SKEW_MS: f64 = 60_000.0;
const REFRESH_FAILURE_BACKOFF_MS: f64 = 300_000.0;
const USAGE_FAILURE_BACKOFF_MS: f64 = 10_000.0;
const USAGE_HEADER_INGEST_INTERVAL_MS: f64 = 60_000.0;
const LAST_GOOD_RETENTION_MS: f64 = 86_400_000.0;
const USAGE_CACHE_PREFIX: &str = "usage_cache:";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthStorageError {
    Cancelled,
    Storage,
    Configuration,
    Unavailable,
    Transient,
    Definitive,
    OutcomeUnknown,
    Unsupported,
}
impl std::fmt::Display for AuthStorageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "authentication cancelled",
            Self::Storage => "authentication storage failed",
            Self::Configuration => "authentication configuration failed",
            Self::Unavailable => "authentication unavailable",
            Self::Transient => "authentication refresh temporarily unavailable",
            Self::Definitive => "authentication grant rejected",
            Self::OutcomeUnknown => "authentication refresh outcome unknown",
            Self::Unsupported => "OAuth provider not installed",
        })
    }
}
impl std::error::Error for AuthStorageError {}
impl From<AuthStorageError> for AuthResolveError {
    fn from(error: AuthStorageError) -> Self {
        match error {
            AuthStorageError::Cancelled => Self::Cancelled,
            AuthStorageError::Storage => Self::Storage,
            AuthStorageError::Configuration => Self::Command,
            AuthStorageError::Unavailable | AuthStorageError::Unsupported => Self::Unavailable,
            _ => Self::Refresh,
        }
    }
}

#[async_trait]
pub trait ConfigKeyResolver: Send + Sync {
    async fn resolve(
        &self,
        configuration: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, AuthStorageError>;
    async fn wait_for_settlement(&self) {}
}
pub struct LiteralConfigKeyResolver;
#[async_trait]
impl ConfigKeyResolver for LiteralConfigKeyResolver {
    async fn resolve(
        &self,
        configuration: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, AuthStorageError> {
        check_cancel(cancel)?;
        // Commands require the existing Host-owned command resolver. Never send
        // a command or unresolved environment expression as a wire credential.
        if configuration.starts_with('!') || configuration.starts_with('$') {
            return Err(AuthStorageError::Configuration);
        }
        Ok(Some(configuration.to_owned()))
    }
}

#[derive(Clone)]
pub struct EnvironmentKey {
    pub variable: String,
    pub value: String,
}
pub trait Environment: Send + Sync {
    fn api_key(&self, provider: &str) -> Option<EnvironmentKey>;
}
pub trait Fallback: Send + Sync {
    fn api_key(&self, provider: &str) -> Option<String>;
}
struct EmptyEnvironment;
impl Environment for EmptyEnvironment {
    fn api_key(&self, _: &str) -> Option<EnvironmentKey> {
        None
    }
}
struct EmptyFallback;
impl Fallback for EmptyFallback {
    fn api_key(&self, _: &str) -> Option<String> {
        None
    }
}

/// Providers return a persisted exact row and its private wire projection.
/// A refresh must coalesce by durable ID, fence publication/disable with CAS,
/// and settle started grants even if the consumer is cancelled or dropped.
#[async_trait]
pub trait OAuthProvider: Send + Sync {
    async fn resolve(
        &self,
        row: StoredAuthCredential,
        force_refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError>;
    fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError>;
    async fn prepare_for_request(
        &self,
        _row: &StoredAuthCredential,
        _cancel: &CancellationToken,
    ) -> Result<(), AuthStorageError> {
        Ok(())
    }
    /// Advisory refresh for usage only. The inner refresh retains its native
    /// definitive-rejection lifecycle; the outer usage probe adds no disable.
    /// Returning None means use the original token; endpoints and native broker
    /// adapters may implement their own non-authoritative path.
    async fn prepare_usage_credential(
        &self,
        _row: &StoredAuthCredential,
        _cancel: &CancellationToken,
    ) -> Result<Option<UsageCredential>, AuthStorageError> {
        Ok(None)
    }
    async fn wait_for_settlement(&self) {}
}
#[derive(Clone)]
pub struct ResolvedOAuth {
    pub row: StoredAuthCredential,
    pub lease: RequestAuthLease,
}

#[derive(Clone)]
pub struct UsageRequest {
    pub provider: String,
    pub credential: UsageCredential,
    pub credential_id: Option<i64>,
    pub base_url: Option<String>,
    pub account_key: String,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UsageFetchError {
    Cancelled,
    Transient,
    Unauthorized,
    Forbidden,
    InvalidResponse,
}
pub type RateLimitHeaderParser = fn(&BTreeMap<String, String>, f64) -> Option<UsageReport>;
#[async_trait]
pub trait UsageProvider: Send + Sync {
    async fn fetch_usage(
        &self,
        request: UsageRequest,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, UsageFetchError>;
    fn rate_limit_header_parser(&self) -> Option<RateLimitHeaderParser> {
        None
    }
    fn supports(&self, _request: &UsageRequest) -> bool {
        true
    }
    fn retain_last_good_on_failure(&self) -> bool {
        true
    }
    /// An authoritative broker OAuth hook bypasses local cache and never falls
    /// back to a local account fetch after returning None.
    fn authoritative_oauth(&self) -> bool {
        false
    }
}

pub struct AuthStorageOptions {
    pub config_key_resolver: Arc<dyn ConfigKeyResolver>,
    pub environment: Arc<dyn Environment>,
    pub fallback: Arc<dyn Fallback>,
    pub usage_request_timeout: Duration,
    pub clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    /// Math.random-style sample in [0,1); injected for deterministic fixtures.
    pub jitter: Arc<dyn Fn() -> f64 + Send + Sync>,
}
impl Default for AuthStorageOptions {
    fn default() -> Self {
        Self {
            config_key_resolver: Arc::new(LiteralConfigKeyResolver),
            environment: Arc::new(EmptyEnvironment),
            fallback: Arc::new(EmptyFallback),
            usage_request_timeout: Duration::from_secs(10),
            clock: Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
            jitter: Arc::new(|| (uuid::Uuid::new_v4().as_u128() >> 75) as f64 / (1_u64 << 53) as f64),
        }
    }
}

#[derive(Clone, Default)]
pub struct AuthRequestContext {
    pub session_id: Option<String>,
    pub model_id: Option<String>,
    pub base_url: Option<String>,
    /// Host default for this request, at the native environment cascade leg.
    /// Private route values never become global Registry overrides.
    pub environment_key: Option<EnvironmentKey>,
    pub force_refresh: bool,
    pub excluded_credential_ids: BTreeSet<i64>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialSource {
    Runtime,
    Config,
    OAuth,
    LoginKey,
    Environment,
    StaticKey,
    Fallback,
}
#[derive(Clone)]
pub struct OAuthAccess {
    pub access_token: String,
    pub credential_id: i64,
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub project_id: Option<String>,
    pub enterprise_url: Option<String>,
    pub api_endpoint: Option<String>,
    pub org_id: Option<String>,
    pub org_name: Option<String>,
}
#[derive(Clone)]
pub struct AuthAccess {
    pub lease: RequestAuthLease,
    pub oauth: Option<OAuthAccess>,
    pub source: CredentialSource,
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UsageLimitMarkResult {
    pub switched: bool,
    pub retry_at_ms: Option<f64>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthFailureCategory {
    Quota,
    AccountPolicy,
    ModelAccountPolicy,
    InvalidatedOAuth,
    HardAuth,
}

#[derive(Default)]
struct HostState {
    assignments: AuthStorageState,
    rows: BTreeMap<String, Vec<StoredAuthCredential>>,
    runtime_keys: BTreeMap<String, String>,
    config_keys: BTreeMap<String, String>,
    resolved_keys: BTreeMap<i64, (String, String)>,
    usage_header_ingest_at: BTreeMap<String, f64>,
    generation_changed: bool,
}
type UsageFlight = watch::Receiver<Option<Option<UsageReport>>>;
struct AuthStorageInner {
    store: Arc<Mutex<SqliteCredentialStore>>,
    codex: Option<Arc<OpenAiCodexAuth>>,
    options: AuthStorageOptions,
    state: Mutex<HostState>,
    oauth_providers: Mutex<BTreeMap<String, Arc<dyn OAuthProvider>>>,
    usage_providers: Mutex<BTreeMap<String, Arc<dyn UsageProvider>>>,
    strategies: Mutex<BTreeMap<String, Arc<dyn CredentialRankingStrategy>>>,
    usage_flights: Mutex<BTreeMap<String, UsageFlight>>,
    usage_epoch: AtomicU64,
    generation: AtomicU64,
    changed: Notify,
    usage_pending: AtomicUsize,
    usage_settled: Notify,
}
#[derive(Clone)]
pub struct AuthStorage {
    inner: Arc<AuthStorageInner>,
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), AuthStorageError> {
    if cancel.is_cancelled() { Err(AuthStorageError::Cancelled) } else { Ok(()) }
}
fn text_field(row: &StoredAuthCredential, key: &str) -> Option<String> {
    match &row.credential {
        AuthCredential::OAuth { fields } => fields.get(key)?.as_str().map(str::to_owned),
        _ => None,
    }
}
fn number_field(row: &StoredAuthCredential, key: &str) -> Option<f64> {
    match &row.credential {
        AuthCredential::OAuth { fields } => fields.get(key)?.as_f64(),
        _ => None,
    }
}
fn access_fresh(row: &StoredAuthCredential, now: f64, skew: f64) -> bool {
    number_field(row, "expires").is_some_and(|expires| now + skew < expires)
        && text_field(row, "access").is_some_and(|access| !access.is_empty())
}
fn access_from_row(row: &StoredAuthCredential) -> Option<OAuthAccess> {
    Some(OAuthAccess {
        access_token: text_field(row, "access")?,
        credential_id: row.id,
        account_id: text_field(row, "accountId"),
        email: text_field(row, "email"),
        project_id: text_field(row, "projectId"),
        enterprise_url: text_field(row, "enterpriseUrl"),
        api_endpoint: text_field(row, "apiEndpoint"),
        org_id: text_field(row, "orgId"),
        org_name: text_field(row, "orgName"),
    })
}
fn map_codex_error(error: CodexAuthError) -> AuthStorageError {
    match error {
        CodexAuthError::Cancelled => AuthStorageError::Cancelled,
        CodexAuthError::Storage => AuthStorageError::Storage,
        CodexAuthError::RefreshRejected { definitive: true, .. } => AuthStorageError::Definitive,
        CodexAuthError::RefreshRejected { definitive: false, .. }
        | CodexAuthError::HttpStatus(_)
        | CodexAuthError::LoginRequired => AuthStorageError::Transient,
        CodexAuthError::Transport | CodexAuthError::TimedOut | CodexAuthError::InvalidResponse => {
            AuthStorageError::OutcomeUnknown
        }
        CodexAuthError::InvalidEndpoint => AuthStorageError::Configuration,
    }
}

impl AuthStorage {
    pub fn for_codex(codex: Arc<OpenAiCodexAuth>, options: AuthStorageOptions) -> Result<Self, AuthStorageError> {
        Self::new(codex.store_handle(), Some(codex), options)
    }
    pub fn new(
        store: Arc<Mutex<SqliteCredentialStore>>,
        codex: Option<Arc<OpenAiCodexAuth>>,
        options: AuthStorageOptions,
    ) -> Result<Self, AuthStorageError> {
        if let Some(codex) = &codex
            && !Arc::ptr_eq(&store, &codex.store_handle())
        {
            return Err(AuthStorageError::Configuration);
        }
        let this = Self {
            inner: Arc::new(AuthStorageInner {
                store,
                codex,
                options,
                state: Mutex::new(HostState::default()),
                oauth_providers: Mutex::new(BTreeMap::new()),
                usage_providers: Mutex::new(BTreeMap::new()),
                strategies: Mutex::new(BTreeMap::new()),
                usage_flights: Mutex::new(BTreeMap::new()),
                usage_epoch: AtomicU64::new(0),
                generation: AtomicU64::new(1),
                changed: Notify::new(),
                usage_pending: AtomicUsize::new(0),
                usage_settled: Notify::new(),
            }),
        };
        this.store_operation(|store, state| {
            let _ = store.clean_expired_cache();
            if let Err(error) = store.clean_expired_credential_blocks(this.now() as i64) {
                state.assignments.observe_store_error(&error);
            }
            Ok(())
        })?;
        this.reload()?;
        if this.inner.codex.is_some() {
            let provider =
                crate::codex_usage::CodexUsageProvider::new().map_err(|_| AuthStorageError::Configuration)?;
            this.inner
                .usage_providers
                .lock()
                .map_err(|_| AuthStorageError::Storage)?
                .insert("openai-codex".to_owned(), Arc::new(provider));
        }
        Ok(this)
    }
    fn now(&self) -> f64 {
        (self.inner.options.clock)()
    }
    fn bump_generation(&self) {
        self.inner.generation.fetch_add(1, Ordering::AcqRel);
        self.inner.changed.notify_waiters();
    }
    pub fn generation(&self) -> u64 {
        self.inner.generation.load(Ordering::Acquire)
    }
    fn store_operation<T>(
        &self,
        operation: impl FnOnce(&SqliteCredentialStore, &mut HostState) -> Result<T, AuthStorageError>,
    ) -> Result<T, AuthStorageError> {
        // All code uses the same store -> state lock order; guards never leave
        // this synchronous closure or cross an await.
        let store = self.inner.store.lock().map_err(|_| AuthStorageError::Storage)?;
        let mut state = self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
        let result = operation(&store, &mut state);
        let changed = std::mem::take(&mut state.generation_changed);
        drop(state);
        drop(store);
        if changed {
            self.bump_generation();
        }
        result
    }
    async fn database<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&SqliteCredentialStore, &mut HostState) -> Result<T, AuthStorageError> + Send + 'static,
    ) -> Result<T, AuthStorageError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.store_operation(operation))
            .await
            .map_err(|_| AuthStorageError::Storage)?
    }
    pub fn reload(&self) -> Result<(), AuthStorageError> {
        self.store_operation(|store, state| {
            let rows = store.list_auth_credentials(None).map_err(|error| {
                state.assignments.observe_store_error(&error);
                AuthStorageError::Storage
            })?;
            let mut grouped = BTreeMap::<String, Vec<StoredAuthCredential>>::new();
            for row in &rows {
                grouped.entry(row.provider.clone()).or_default().push(row.clone());
            }
            // Fixed reload keeps the last stored OAuth row for each native
            // identity and soft-disables older duplicates before publication.
            for (provider, entries) in &mut grouped {
                let mut seen = BTreeSet::new();
                let mut removed = BTreeSet::new();
                for row in entries.iter().rev() {
                    if matches!(row.credential, AuthCredential::OAuth { .. })
                        && let Some(identity) =
                            crate::credential_store::resolve_credential_identity_key(provider, &row.credential)
                        && !seen.insert(identity)
                    {
                        store
                            .delete_auth_credential(row.id, "deduplicated duplicate credential")
                            .map_err(|_| AuthStorageError::Storage)?;
                        removed.insert(row.id);
                    }
                }
                if !removed.is_empty() {
                    entries.retain(|row| !removed.contains(&row.id));
                    state.assignments.reset_provider_assignments(store, provider);
                }
            }
            let changed = state.rows.len() != grouped.len()
                || grouped.iter().any(|(provider, rows)| {
                    !stored_credential_arrays_equal(state.rows.get(provider).map(Vec::as_slice).unwrap_or(&[]), rows)
                });
            let active_rows: Vec<_> = grouped.values().flatten().cloned().collect();
            state.assignments.reload_state(&active_rows);
            state.rows = grouped;
            state.generation_changed |= changed;
            Ok(())
        })
    }
    fn load_provider(
        store: &SqliteCredentialStore,
        state: &mut HostState,
        provider: &str,
    ) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        let rows = store.list_auth_credentials(Some(provider)).map_err(|error| {
            state.assignments.observe_store_error(&error);
            AuthStorageError::Storage
        })?;
        if !stored_credential_arrays_equal(state.rows.get(provider).map(Vec::as_slice).unwrap_or(&[]), &rows) {
            state.assignments.prune_provider_bearer_history(provider, &rows);
            state.generation_changed = true;
        }
        if rows.is_empty() {
            state.rows.remove(provider);
        } else {
            state.rows.insert(provider.to_owned(), rows.clone());
        }
        Ok(rows)
    }
    async fn provider_rows(&self, provider: &str) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        let provider = provider.to_owned();
        self.database(move |store, state| Self::load_provider(store, state, &provider)).await
    }
    fn suppressed(&self, provider: &str) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.runtime_keys.contains_key(provider) || state.config_keys.contains_key(provider))
            .unwrap_or(true)
    }
    pub fn set_runtime_api_key(&self, provider: &str, key: String) -> Result<(), AuthStorageError> {
        self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.runtime_keys.insert(provider.to_owned(), key);
        self.bump_generation();
        Ok(())
    }
    pub fn set_config_api_key(&self, provider: &str, key: String) -> Result<(), AuthStorageError> {
        self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.config_keys.insert(provider.to_owned(), key);
        self.bump_generation();
        Ok(())
    }
    pub fn remove_runtime_api_key(&self, provider: &str) -> Result<(), AuthStorageError> {
        self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.runtime_keys.remove(provider);
        self.bump_generation();
        Ok(())
    }
    pub fn clear_config_api_keys(&self) -> Result<(), AuthStorageError> {
        self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.config_keys.clear();
        self.bump_generation();
        Ok(())
    }
    pub fn remove_config_api_key(&self, provider: &str) -> Result<(), AuthStorageError> {
        self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.config_keys.remove(provider);
        self.bump_generation();
        Ok(())
    }
    pub fn register_oauth_provider(
        &self,
        provider: &str,
        implementation: Arc<dyn OAuthProvider>,
    ) -> Result<(), AuthStorageError> {
        self.inner
            .oauth_providers
            .lock()
            .map_err(|_| AuthStorageError::Storage)?
            .insert(provider.to_owned(), implementation);
        self.bump_generation();
        Ok(())
    }
    pub fn unregister_oauth_provider(&self, provider: &str) -> Result<(), AuthStorageError> {
        self.inner.oauth_providers.lock().map_err(|_| AuthStorageError::Storage)?.remove(provider);
        self.bump_generation();
        Ok(())
    }
    pub fn register_usage_provider(
        &self,
        provider: &str,
        implementation: Arc<dyn UsageProvider>,
    ) -> Result<(), AuthStorageError> {
        self.inner
            .usage_providers
            .lock()
            .map_err(|_| AuthStorageError::Storage)?
            .insert(provider.to_owned(), implementation);
        self.expire_usage_cache(Some(provider))?;
        self.bump_generation();
        Ok(())
    }
    pub fn register_ranking_strategy(
        &self,
        provider: &str,
        strategy: Arc<dyn CredentialRankingStrategy>,
    ) -> Result<(), AuthStorageError> {
        self.inner.strategies.lock().map_err(|_| AuthStorageError::Storage)?.insert(provider.to_owned(), strategy);
        Ok(())
    }
    fn strategy(&self, provider: &str) -> Option<Arc<dyn CredentialRankingStrategy>> {
        self.inner.strategies.lock().ok()?.get(provider).cloned().or_else(|| {
            (provider == "openai-codex")
                .then(|| Arc::new(policy::CodexRankingStrategy) as Arc<dyn CredentialRankingStrategy>)
        })
    }
    fn oauth_hook(&self, provider: &str) -> Option<Arc<dyn OAuthProvider>> {
        self.inner.oauth_providers.lock().ok()?.get(provider).cloned()
    }
    fn usage_hook(&self, provider: &str) -> Option<Arc<dyn UsageProvider>> {
        self.inner.usage_providers.lock().ok()?.get(provider).cloned()
    }
    fn supports_oauth(&self, provider: &str) -> bool {
        self.oauth_hook(provider).is_some() || (provider == "openai-codex" && self.inner.codex.is_some())
    }

    pub async fn resolve(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<AuthAccess>, AuthStorageError> {
        check_cancel(cancel)?;
        let override_access = {
            let state = self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
            state
                .runtime_keys
                .get(provider)
                .filter(|key| !key.is_empty())
                .map(|key| (CredentialSource::Runtime, CredentialIdentity::Runtime, key.clone()))
                .or_else(|| {
                    state.config_keys.get(provider).filter(|key| !key.is_empty()).map(|key| {
                        (
                            CredentialSource::Config,
                            CredentialIdentity::Config { provider: provider.to_owned() },
                            key.clone(),
                        )
                    })
                })
        };
        if let Some((source, identity, key)) = override_access {
            return Ok(Some(AuthAccess { lease: RequestAuthLease::new(identity, Some(key)), oauth: None, source }));
        }
        if !self.suppressed(provider)
            && let Some(oauth) = self.resolve_oauth_selection(provider, context, cancel).await?
        {
            return Ok(Some(oauth));
        }
        if let Some(row) = self.select_api_key(provider, context, true, cancel).await? {
            return self.stored_key_access(provider, context, row, CredentialSource::LoginKey, cancel).await.map(Some);
        }
        // Native lower cascade legs forget only the in-process assignment.
        let provider_owned = provider.to_owned();
        let session = context.session_id.clone();
        self.database(move |_store, state| {
            state.assignments.forget_session_credential(&provider_owned, session.as_deref());
            Ok(())
        })
        .await?;
        if let Some(env) = context
            .environment_key
            .clone()
            .filter(|env| !env.value.is_empty())
            .or_else(|| self.inner.options.environment.api_key(provider).filter(|env| !env.value.is_empty()))
        {
            return Ok(Some(AuthAccess {
                lease: RequestAuthLease::new(
                    CredentialIdentity::Environment { variable: env.variable },
                    Some(env.value),
                ),
                oauth: None,
                source: CredentialSource::Environment,
            }));
        }
        if let Some(row) = self.select_api_key(provider, context, false, cancel).await? {
            return self.stored_key_access(provider, context, row, CredentialSource::StaticKey, cancel).await.map(Some);
        }
        Ok(self.inner.options.fallback.api_key(provider).map(|key| AuthAccess {
            lease: RequestAuthLease::new(CredentialIdentity::Keyless, Some(key)),
            oauth: None,
            source: CredentialSource::Fallback,
        }))
    }
    pub async fn get_api_key(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, AuthStorageError> {
        Ok(self.resolve(provider, context, cancel).await?.and_then(|access| access.lease.api_key().map(str::to_owned)))
    }
    pub async fn get_oauth_access(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<OAuthAccess>, AuthStorageError> {
        check_cancel(cancel)?;
        if self.suppressed(provider) {
            return Ok(None);
        }
        Ok(self.resolve_oauth_selection(provider, context, cancel).await?.and_then(|access| access.oauth))
    }
    fn active_oauth_row(
        &self,
        provider: &str,
        session_id: Option<&str>,
    ) -> Result<Option<StoredAuthCredential>, AuthStorageError> {
        if self.suppressed(provider) {
            return Ok(None);
        }
        let lower_key_present =
            self.inner.options.environment.api_key(provider).is_some_and(|key| !key.value.is_empty())
                || self.inner.options.fallback.api_key(provider).is_some_and(|key| !key.is_empty());
        self.store_operation(|store, state| {
            Self::active_oauth_row_in_store(store, state, provider, session_id, lower_key_present)
        })
    }
    fn active_oauth_row_in_store(
        store: &SqliteCredentialStore,
        state: &mut HostState,
        provider: &str,
        session_id: Option<&str>,
        lower_key_present: bool,
    ) -> Result<Option<StoredAuthCredential>, AuthStorageError> {
        let rows = Self::load_provider(store, state, provider)?;
        let sticky = state.assignments.read_session_credential(store, provider, session_id, &rows);
        if sticky.is_some_and(|sticky| sticky.kind != CredentialKind::OAuth) {
            return Ok(None);
        }
        if sticky.is_none() && lower_key_present {
            return Ok(None);
        }
        Ok(sticky
            .and_then(|sticky| rows.get(sticky.index))
            .filter(|row| matches!(row.credential, AuthCredential::OAuth { .. }))
            .or_else(|| rows.iter().find(|row| matches!(row.credential, AuthCredential::OAuth { .. })))
            .cloned())
    }
    pub fn peek_oauth_access(
        &self,
        provider: &str,
        session_id: Option<&str>,
    ) -> Result<Option<OAuthAccess>, AuthStorageError> {
        Ok(self
            .active_oauth_row(provider, session_id)?
            .filter(|row| access_fresh(row, self.now(), 0.0))
            .as_ref()
            .and_then(access_from_row))
    }
    pub fn stored_oauth_snapshot(&self, provider: &str) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        self.store_operation(|store, state| {
            Ok(Self::load_provider(store, state, provider)?
                .into_iter()
                .filter(|row| matches!(row.credential, AuthCredential::OAuth { .. }))
                .collect())
        })
    }
    pub fn has_auth(&self, provider: &str) -> Result<bool, AuthStorageError> {
        let has_stored = self.store_operation(|store, state| {
            Ok(state.runtime_keys.contains_key(provider)
                || state.config_keys.contains_key(provider)
                || !Self::load_provider(store, state, provider)?.is_empty())
        })?;
        Ok(has_stored
            || self.inner.options.environment.api_key(provider).is_some_and(|key| !key.value.is_empty())
            || self.inner.options.fallback.api_key(provider).is_some_and(|key| !key.is_empty()))
    }
    /// Offline Registry lookup: commands are never dispatched. A previous
    /// resolved value is reusable only while that exact stored payload remains.
    pub fn peek_api_key(&self, provider: &str, session_id: Option<&str>) -> Result<Option<String>, AuthStorageError> {
        let environment = self.inner.options.environment.api_key(provider).filter(|key| !key.value.is_empty());
        let (high, login, static_key) = self.store_operation(|store, state| {
            let high = state.runtime_keys.get(provider).filter(|key| !key.is_empty()).cloned()
                .or_else(|| state.config_keys.get(provider).filter(|key| !key.is_empty()).cloned());
            if high.is_some() { return Ok((high, None, None)) }
            let rows = Self::load_provider(store, state, provider)?;
            if !state.runtime_keys.contains_key(provider) && !state.config_keys.contains_key(provider) {
                let sticky = state.assignments.read_session_credential(store, provider, session_id, &rows);
                let oauth = sticky.filter(|sticky| sticky.kind == CredentialKind::OAuth).and_then(|sticky| rows.get(sticky.index))
                    .filter(|row| matches!(row.credential, AuthCredential::OAuth { .. }))
                    .cloned().or_else(|| Self::select_snapshot_credential(store, state, provider, CredentialKind::OAuth,
                        session_id, &rows, self.now(), |_| true));
                if let Some(row) = oauth.as_ref().filter(|row| access_fresh(row, self.now(), 0.0)) {
                    let key = if provider == "github-copilot" {
                        let mut value = Map::new(); value.insert("token".into(), Value::String(text_field(row, "access").unwrap_or_default()));
                        for field in ["enterpriseUrl", "apiEndpoint"] { if let Some(text) = text_field(row, field) { value.insert(field.into(), Value::String(text)); } }
                        Value::Object(value).to_string()
                    } else { text_field(row, "access").unwrap_or_default() };
                    return Ok((Some(key), None, None));
                }
            }
            let login_row = Self::select_snapshot_credential(store, state, provider, CredentialKind::ApiKey, session_id,
                &rows, self.now(), |row| matches!(&row.credential, AuthCredential::ApiKey { source, .. } if source.as_deref() == Some("login")));
            let literal = |row: &StoredAuthCredential| -> Option<String> {
                let AuthCredential::ApiKey { key, .. } = &row.credential else { return None };
                if !key.starts_with('!') && !key.starts_with('$') { return Some(key.clone()) }
                state.resolved_keys.get(&row.id).filter(|(payload, _)| payload == &row.serialized_data).map(|(_, key)| key.clone())
            };
            let login = login_row.as_ref().and_then(&literal);
            if login.is_some() || environment.is_some() { return Ok((None, login, None)) }
            let static_row = Self::select_snapshot_credential(store, state, provider, CredentialKind::ApiKey, session_id, &rows, self.now(), |_| true);
            let static_key = static_row.as_ref().and_then(|row| {
                let AuthCredential::ApiKey { key, .. } = &row.credential else { return None };
                if !key.starts_with('!') && !key.starts_with('$') { return Some(key.clone()) }
                state.resolved_keys.get(&row.id).filter(|(payload, _)| payload == &row.serialized_data).map(|(_, key)| key.clone())
            });
            Ok((None, login, static_key))
        })?;
        if high.is_some() {
            return Ok(high);
        }
        if login.is_some() {
            return Ok(login);
        }
        if let Some(environment) = environment {
            return Ok(Some(environment.value));
        }
        Ok(static_key.or_else(|| self.inner.options.fallback.api_key(provider)))
    }
    /// Native selectCredentialByType retains each row's full provider index.
    /// A singleton bypasses both RR and blocked checks; all-blocked falls back
    /// to the first member of the same order rather than searching for expiry.
    // Native selection keeps store/state, identity, rows, clock and predicate independent.
    #[allow(clippy::too_many_arguments)]
    fn select_snapshot_credential(
        store: &SqliteCredentialStore,
        state: &mut HostState,
        provider: &str,
        kind: CredentialKind,
        session_id: Option<&str>,
        rows: &[StoredAuthCredential],
        now: f64,
        filter: impl Fn(&StoredAuthCredential) -> bool,
    ) -> Option<StoredAuthCredential> {
        let credentials: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| CredentialKind::of(&row.credential) == kind && filter(row))
            .collect();
        if credentials.len() <= 1 {
            return credentials.first().map(|(_, row)| (**row).clone());
        }
        let provider_key = provider_type_key(provider, kind);
        let order = state.assignments.credential_order(&provider_key, session_id, credentials.len());
        let fallback = (*credentials[order[0]].1).clone();
        for position in order {
            let (index, row) = credentials[position];
            if state.assignments.blocked_until(store, &provider_key, index, &[], rows, now).is_none() {
                return Some(row.clone());
            }
        }
        Some(fallback)
    }
    pub fn get_oauth_account_id(
        &self,
        provider: &str,
        session_id: Option<&str>,
    ) -> Result<Option<String>, AuthStorageError> {
        Ok(self
            .get_oauth_account_identity(provider, session_id)?
            .and_then(|identity| identity.account_id)
            .filter(|id| !id.is_empty()))
    }
    pub fn get_oauth_account_identity(
        &self,
        provider: &str,
        session_id: Option<&str>,
    ) -> Result<Option<OAuthAccountSummary>, AuthStorageError> {
        let Some(row) = self.active_oauth_row(provider, session_id)? else { return Ok(None) };
        Ok(self.list_oauth_accounts(provider, session_id)?.into_iter().find(|summary| summary.credential_id == row.id))
    }
    pub fn list_oauth_accounts(
        &self,
        provider: &str,
        session_id: Option<&str>,
    ) -> Result<Vec<OAuthAccountSummary>, AuthStorageError> {
        let suppressed = self.suppressed(provider);
        self.store_operation(|store, state| {
            let rows = Self::load_provider(store, state, provider)?;
            Ok(state.assignments.list_oauth_accounts(store, provider, session_id, &rows, suppressed))
        })
    }
    pub fn pin_session_oauth_account(
        &self,
        provider: &str,
        session_id: &str,
        id: i64,
        last_used_at_ms: Option<f64>,
    ) -> Result<bool, AuthStorageError> {
        let suppressed = self.suppressed(provider);
        self.store_operation(|store, state| {
            let rows = Self::load_provider(store, state, provider)?;
            Ok(state.assignments.pin_session_oauth_account(
                store,
                provider,
                session_id,
                id,
                &rows,
                suppressed,
                last_used_at_ms,
                self.now(),
            ))
        })
    }

    async fn stored_key_access(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        row: StoredAuthCredential,
        source: CredentialSource,
        cancel: &CancellationToken,
    ) -> Result<AuthAccess, AuthStorageError> {
        let AuthCredential::ApiKey { key, .. } = &row.credential else { return Err(AuthStorageError::Unavailable) };
        let key = self.inner.options.config_key_resolver.resolve(key, cancel).await?;
        check_cancel(cancel)?;
        if let Some(key) = &key {
            self.inner
                .state
                .lock()
                .map_err(|_| AuthStorageError::Storage)?
                .resolved_keys
                .insert(row.id, (row.serialized_data.clone(), key.clone()));
        }
        self.record_selected(provider, context, &row, key.as_deref()).await?;
        Ok(AuthAccess {
            lease: RequestAuthLease::new(CredentialIdentity::Stored { id: row.id, revision: row.revision }, key),
            oauth: None,
            source,
        })
    }
    async fn record_selected(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        row: &StoredAuthCredential,
        bearer: Option<&str>,
    ) -> Result<(), AuthStorageError> {
        let provider = provider.to_owned();
        let session = context.session_id.clone();
        let id = row.id;
        let bearer = bearer.map(str::to_owned);
        let now = self.now();
        let expected_kind = CredentialKind::of(&row.credential);
        self.database(move |store, state| {
            let rows = Self::load_provider(store, state, &provider)?;
            let Some(index) = rows.iter().position(|row| row.id == id) else {
                return Err(AuthStorageError::Unavailable);
            };
            let kind = CredentialKind::of(&rows[index].credential);
            if kind != expected_kind {
                return Err(AuthStorageError::Unavailable);
            }
            if kind == CredentialKind::OAuth
                && let Some(bearer) = bearer
            {
                state.assignments.record_oauth_bearer_credential_id(&provider, &bearer, Some(id));
            }
            state.assignments.record_session_credential(
                store,
                &provider,
                session.as_deref(),
                kind,
                index,
                &rows,
                None,
                now,
            );
            Ok(())
        })
        .await
    }
    async fn blocked(
        &self,
        provider: &str,
        id: i64,
        kind: CredentialKind,
        scopes: &[String],
    ) -> Result<Option<f64>, AuthStorageError> {
        let provider = provider.to_owned();
        let scopes = scopes.to_vec();
        let now = self.now();
        self.database(move |store, state| {
            let rows = Self::load_provider(store, state, &provider)?;
            let Some(index) = rows.iter().position(|row| row.id == id) else { return Ok(None) };
            Ok(state.assignments.blocked_until(
                store,
                &provider_type_key(&provider, kind),
                index,
                &scopes.iter().map(String::as_str).collect::<Vec<_>>(),
                &rows,
                now,
            ))
        })
        .await
    }
    async fn mark_block(
        &self,
        provider: &str,
        id: i64,
        kind: CredentialKind,
        until: f64,
        scope: Option<&str>,
    ) -> Result<(), AuthStorageError> {
        let provider = provider.to_owned();
        let scope = scope.map(str::to_owned);
        let now = self.now();
        let cache_provider = provider.clone();
        self.database(move |store, state| {
            let rows = Self::load_provider(store, state, &provider)?;
            if let Some(index) = rows.iter().position(|row| row.id == id) {
                state.assignments.mark_credential_blocked(
                    store,
                    &provider_type_key(&provider, kind),
                    index,
                    until,
                    scope.as_deref(),
                    &rows,
                    now,
                );
            }
            Ok(())
        })
        .await?;
        self.expire_usage_cache(Some(&cache_provider))?;
        self.bump_generation();
        Ok(())
    }

    async fn resolve_exact_oauth(
        &self,
        provider: &str,
        row: StoredAuthCredential,
        force: bool,
        cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        check_cancel(cancel)?;
        if let Some(hook) = self.oauth_hook(provider) {
            hook.prepare_for_request(&row, cancel).await?;
            check_cancel(cancel)?;
            let latest = self
                .provider_rows(provider)
                .await?
                .into_iter()
                .find(|latest| latest.id == row.id && matches!(latest.credential, AuthCredential::OAuth { .. }))
                .ok_or(AuthStorageError::Unavailable)?;
            let peer_rotated = latest.serialized_data != row.serialized_data;
            let force = force && !peer_rotated;
            if !force && access_fresh(&latest, self.now(), REFRESH_SKEW_MS) {
                return Ok(ResolvedOAuth { lease: hook.lease(&latest)?, row: latest });
            }
            if !text_field(&latest, "refresh").is_some_and(|refresh| !refresh.trim().is_empty()) {
                return Err(AuthStorageError::Unavailable);
            }
            let id = latest.id;
            let resolved = hook.resolve(latest, force, cancel).await?;
            check_cancel(cancel)?;
            if resolved.row.id != id
                || resolved.row.provider != provider
                || resolved.row.disabled_cause.is_some()
                || !matches!(resolved.row.credential, AuthCredential::OAuth { .. })
                || !matches!(resolved.lease.identity(), CredentialIdentity::Stored { id: lease_id, .. } if *lease_id == id)
            {
                return Err(AuthStorageError::Configuration);
            }
            return Ok(resolved);
        }
        if provider == "openai-codex"
            && let Some(codex) = &self.inner.codex
        {
            let latest = self
                .provider_rows(provider)
                .await?
                .into_iter()
                .find(|latest| latest.id == row.id)
                .ok_or(AuthStorageError::Unavailable)?;
            let force = force && latest.serialized_data == row.serialized_data;
            // Native refresh.ts rejects missing required fields before
            // dispatch. Keep that known local failure separate from an
            // invalid response after a grant was sent and may have rotated.
            if (force || !access_fresh(&latest, self.now(), REFRESH_SKEW_MS))
                && !text_field(&latest, "refresh").is_some_and(|refresh| !refresh.is_empty())
            {
                return Err(AuthStorageError::Transient);
            }
            let row = codex.resolve_account_credential(row.id, force, cancel).await.map_err(map_codex_error)?;
            let lease = lease_from_row(row.clone()).map_err(map_codex_error)?;
            return Ok(ResolvedOAuth { row, lease });
        }
        Err(AuthStorageError::Unsupported)
    }

    async fn handle_oauth_failure(
        &self,
        provider: &str,
        row: &StoredAuthCredential,
        error: AuthStorageError,
        scope: Option<&str>,
    ) -> Result<Option<StoredAuthCredential>, AuthStorageError> {
        if matches!(error, AuthStorageError::Cancelled | AuthStorageError::Storage | AuthStorageError::Configuration) {
            return Err(error);
        }
        let latest = self.provider_rows(provider).await?.into_iter().find(|latest| latest.id == row.id);
        if error == AuthStorageError::Definitive {
            // A peer rotated the observed row: leave its new grant eligible.
            if let Some(latest) = latest.as_ref().filter(|latest| {
                text_field(latest, "refresh") != text_field(row, "refresh")
                    && access_fresh(latest, self.now(), REFRESH_SKEW_MS)
            }) {
                return Ok(Some(latest.clone()));
            }
            let provider = provider.to_owned();
            let row = row.clone();
            let changed = self
                .database(move |store, state| {
                    let disabled = store
                        .try_disable_auth_credential_if_matches(
                            row.id,
                            &row.serialized_data,
                            "oauth refresh rejected",
                            None,
                        )
                        .map_err(|_| AuthStorageError::Storage)?;
                    Self::load_provider(store, state, &provider)?;
                    if disabled {
                        state.assignments.reset_provider_assignments(store, &provider);
                    }
                    Ok(disabled)
                })
                .await?;
            if changed {
                self.bump_generation();
            }
        } else if error == AuthStorageError::Transient && latest.is_some() {
            self.mark_block(provider, row.id, CredentialKind::OAuth, self.now() + REFRESH_FAILURE_BACKOFF_MS, scope)
                .await?;
        }
        // Unknown post-dispatch effects are owned/fenced by the provider. They
        // never become another local replay or a speculative successful row.
        Ok(None)
    }

    async fn resolve_oauth_selection(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<AuthAccess>, AuthStorageError> {
        if !self.supports_oauth(provider) {
            return Ok(None);
        }
        let rows = self.provider_rows(provider).await?;
        let selections: Vec<_> = rows
            .iter()
            .filter(|row| {
                matches!(row.credential, AuthCredential::OAuth { .. })
                    && !context.excluded_credential_ids.contains(&row.id)
            })
            .cloned()
            .collect();
        if selections.is_empty() {
            return Ok(None);
        }
        let provider_key = provider_type_key(provider, CredentialKind::OAuth);
        let provider_owned = provider.to_owned();
        let session = context.session_id.clone();
        let key = provider_key.clone();
        let total = selections.len();
        let (order, sticky) = self
            .database(move |store, state| {
                let rows = Self::load_provider(store, state, &provider_owned)?;
                let order = state.assignments.credential_order(&key, session.as_deref(), total);
                let sticky = state
                    .assignments
                    .read_session_credential(store, &provider_owned, session.as_deref(), &rows)
                    .filter(|sticky| sticky.kind == CredentialKind::OAuth)
                    .and_then(|sticky| rows.get(sticky.index).map(|row| (row.id, sticky.last_used_at_ms)));
                Ok((order, sticky))
            })
            .await?;
        let strategy = self.strategy(provider);
        let ranking_context = CredentialRankingContext { model_id: context.model_id.clone() };
        let routing =
            policy::credential_block_routing(provider, "oauth", strategy.as_deref(), context.model_id.as_deref(), None);
        let requirement = policy::resolve_codex_plan_requirement(provider, context.model_id.as_deref());
        let check_usage = strategy.is_some() && (selections.len() > 1 || requirement != PlanRequirement::None);
        let preferred = sticky
            .and_then(|(id, last_used)| selections.iter().find(|row| row.id == id).map(|row| (row.clone(), last_used)));
        let preferred_available = if let Some((row, _)) = &preferred {
            (text_field(row, "refresh").is_some_and(|refresh| !refresh.trim().is_empty())
                || access_fresh(row, self.now(), REFRESH_SKEW_MS))
                && self.blocked(provider, row.id, CredentialKind::OAuth, &routing.sibling_block_scopes).await?.is_none()
        } else {
            false
        };
        let warm = provider != "anthropic"
            || preferred.as_ref().and_then(|(_, last)| *last).is_none_or(|last| self.now() - last < 3_600_000.0);
        let should_rank = check_usage && (!preferred_available || !warm || requirement != PlanRequirement::None);
        let preferred_id = preferred.as_ref().map(|(row, _)| row.id);
        let mut ranking_order = if should_rank && context.session_id.as_ref().is_some_and(|session| !session.is_empty())
        {
            (0..selections.len()).collect::<Vec<_>>()
        } else {
            order.clone()
        };
        if should_rank
            && requirement == PlanRequirement::None
            && context.session_id.as_ref().is_some_and(|session| !session.is_empty())
            && let Some(position) = ranking_order.iter().position(|index| Some(selections[*index].id) == preferred_id)
        {
            let index = ranking_order.remove(position);
            ranking_order.insert(0, index);
        }
        let raw = if should_rank {
            self.collect_candidate_usage(
                provider,
                context,
                &selections,
                &ranking_order,
                &routing.sibling_block_scopes,
                true,
                cancel,
            )
            .await?
        } else {
            order
                .iter()
                .filter_map(|index| selections.get(*index))
                .enumerate()
                .map(|(order_pos, row)| UsageCandidate {
                    selection: row.clone(),
                    usage: None,
                    usage_checked: false,
                    blocked_until: None,
                    order_pos,
                })
                .collect()
        };
        let mut candidates = if let Some(strategy) = strategy.as_deref() {
            policy::rank_usage_candidates(raw, strategy, Some(&ranking_context), requirement, self.now())
        } else {
            unranked_candidates(raw, requirement)
        };
        for candidate in &candidates {
            if let Some(until) = candidate.new_block_until {
                self.mark_block(
                    provider,
                    candidate.candidate.selection.id,
                    CredentialKind::OAuth,
                    until,
                    routing.block_scope.as_deref(),
                )
                .await?;
            }
        }
        if !should_rank
            && requirement == PlanRequirement::None
            && let Some(position) =
                candidates.iter().position(|candidate| Some(candidate.candidate.selection.id) == preferred_id)
            && self
                .blocked(
                    provider,
                    candidates[position].candidate.selection.id,
                    CredentialKind::OAuth,
                    &routing.sibling_block_scopes,
                )
                .await?
                .is_none()
        {
            let pinned = candidates.remove(position);
            candidates.insert(0, pinned);
        }
        let force_id = context
            .force_refresh
            .then(|| preferred_id.or_else(|| candidates.first().map(|candidate| candidate.candidate.selection.id)))
            .flatten();
        // Native preflight runs all candidates concurrently, then never retries
        // a failed refresh candidate in any later fallback pass.
        let preflight = join_all(candidates.iter().map(|candidate| {
            let row = candidate.candidate.selection.clone();
            async move {
                let result = self.resolve_exact_oauth(provider, row.clone(), Some(row.id) == force_id, cancel).await;
                (row, result)
            }
        }))
        .await;
        let mut failures = BTreeSet::new();
        let mut prepared = BTreeMap::new();
        for (row, result) in preflight {
            match result {
                Ok(result) => {
                    prepared.insert(row.id, result);
                }
                Err(error) => {
                    if let Some(peer) =
                        self.handle_oauth_failure(provider, &row, error, routing.block_scope.as_deref()).await?
                    {
                        // Lease construction only: this branch never repeats a
                        // refresh already attempted during native preflight.
                        let lease = self.project_oauth_lease(provider, &peer)?;
                        prepared.insert(row.id, ResolvedOAuth { row: peer, lease });
                    } else {
                        failures.insert(row.id);
                    }
                }
            }
        }
        check_cancel(cancel)?;
        let passes = policy::oauth_candidate_passes(requirement, &candidates);
        let enforce_plan = passes.first().is_some_and(|pass| pass.enforce_plan_requirement);
        if requirement != PlanRequirement::None
            && let Some(position) =
                candidates.iter().position(|candidate| Some(candidate.candidate.selection.id) == preferred_id)
        {
            let candidate = &candidates[position];
            if (candidate.plan_eligibility == Some(true)
                || (!enforce_plan && candidate.plan_eligibility != Some(false)))
                && self
                    .blocked(
                        provider,
                        candidate.candidate.selection.id,
                        CredentialKind::OAuth,
                        &routing.sibling_block_scopes,
                    )
                    .await?
                    .is_none()
            {
                let pinned = candidates.remove(position);
                candidates.insert(0, pinned);
            }
        }
        for pass in passes {
            for candidate in &mut candidates {
                let id = candidate.candidate.selection.id;
                if failures.contains(&id) {
                    continue;
                }
                // Re-read active blocks and row identity after every preflight.
                candidate.score.blocked_until =
                    self.blocked(provider, id, CredentialKind::OAuth, &routing.sibling_block_scopes).await?;
                candidate.score.blocked = candidate.score.blocked_until.is_some();
                if !policy::candidate_allowed_in_pass(candidate, pass) {
                    continue;
                }
                let Some(result) = prepared.get(&id).cloned() else { continue };
                let latest = self.provider_rows(provider).await?.into_iter().find(|row| row.id == id);
                let Some(latest) = latest.filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })) else {
                    failures.insert(id);
                    continue;
                };
                // An identity changed by prepare/refresh requires usage/plan to
                // be checked against the new account, preserving source 5623.
                let same_account =
                    text_field(&candidate.candidate.selection, "accountId") == text_field(&latest, "accountId");
                if ((check_usage && !pass.allow_blocked) || requirement != PlanRequirement::None)
                    && (!candidate.candidate.usage_checked || !same_account)
                {
                    candidate.candidate.usage = self.usage_report(provider, &latest, context, false, cancel).await?;
                    candidate.candidate.usage_checked = true;
                    candidate.plan_eligibility =
                        policy::codex_plan_eligibility(candidate.candidate.usage.as_ref(), requirement);
                }
                if pass.enforce_plan_requirement && candidate.plan_eligibility != Some(true) {
                    continue;
                }
                if check_usage
                    && !pass.allow_blocked
                    && let (Some(report), Some(strategy)) = (candidate.candidate.usage.as_ref(), strategy.as_deref())
                {
                    let limits = strategy.scope_limits(report, Some(&ranking_context));
                    if policy::is_usage_limit_reached(&limits) {
                        let until = policy::usage_reset_at_ms(&limits, self.now())
                            .unwrap_or(self.now() + policy::DEFAULT_BACKOFF_MS);
                        self.mark_block(provider, id, CredentialKind::OAuth, until, routing.block_scope.as_deref())
                            .await?;
                        continue;
                    }
                }
                // Preflight's owned token remains the request's lease. If a
                // peer changed the row afterwards, only project its current
                // fresh token; never perform another grant exchange here.
                let selected = if latest.serialized_data != result.row.serialized_data {
                    if !access_fresh(&latest, self.now(), REFRESH_SKEW_MS) {
                        failures.insert(id);
                        continue;
                    }
                    ResolvedOAuth { lease: self.project_oauth_lease(provider, &latest)?, row: latest }
                } else {
                    result
                };
                self.record_selected(provider, context, &selected.row, selected.lease.api_key()).await?;
                return Ok(Some(AuthAccess {
                    oauth: access_from_row(&selected.row),
                    lease: selected.lease,
                    source: CredentialSource::OAuth,
                }));
            }
        }
        Ok(None)
    }
    fn project_oauth_lease(
        &self,
        provider: &str,
        row: &StoredAuthCredential,
    ) -> Result<RequestAuthLease, AuthStorageError> {
        if let Some(hook) = self.oauth_hook(provider) {
            return hook.lease(row);
        }
        if provider == "openai-codex" && self.inner.codex.is_some() {
            return lease_from_row(row.clone()).map_err(map_codex_error);
        }
        Err(AuthStorageError::Unsupported)
    }
    pub async fn get_oauth_access_by_credential_id(
        &self,
        provider: &str,
        id: i64,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<AuthAccess>, AuthStorageError> {
        check_cancel(cancel)?;
        if self.suppressed(provider) || !self.supports_oauth(provider) {
            return Ok(None);
        }
        let Some(row) = self
            .provider_rows(provider)
            .await?
            .into_iter()
            .find(|row| row.id == id && matches!(row.credential, AuthCredential::OAuth { .. }))
        else {
            return Ok(None);
        };
        let result = match self.resolve_exact_oauth(provider, row.clone(), context.force_refresh, cancel).await {
            Ok(result) => result,
            Err(error) => {
                self.handle_oauth_failure(provider, &row, error, None).await?;
                return Ok(None);
            }
        };
        Ok(Some(AuthAccess {
            oauth: access_from_row(&result.row),
            lease: result.lease,
            source: CredentialSource::OAuth,
        }))
    }
    pub async fn get_oauth_access_at(
        &self,
        provider: &str,
        position: usize,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<AuthAccess>, AuthStorageError> {
        let rows = self.provider_rows(provider).await?;
        let Some(row) = rows.iter().filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })).nth(position)
        else {
            return Ok(None);
        };
        self.get_oauth_access_by_credential_id(provider, row.id, context, cancel).await
    }
    pub async fn get_oauth_accesses(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Vec<Result<AuthAccess, AuthStorageError>>, AuthStorageError> {
        if self.suppressed(provider) {
            return Ok(Vec::new());
        }
        let rows = self.provider_rows(provider).await?;
        Ok(join_all(rows.iter().filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })).map(
            |row| async move {
                self.get_oauth_access_by_credential_id(provider, row.id, context, cancel)
                    .await?
                    .ok_or(AuthStorageError::Unavailable)
            },
        ))
        .await)
    }

    async fn select_api_key(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        login: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<StoredAuthCredential>, AuthStorageError> {
        let rows = self.provider_rows(provider).await?;
        let selections: Vec<_> = rows.into_iter().filter(|row| !context.excluded_credential_ids.contains(&row.id)
            && matches!(&row.credential, AuthCredential::ApiKey { source, .. } if (source.as_deref() == Some("login")) == login)).collect();
        if selections.len() <= 1 {
            return Ok(selections.into_iter().next());
        }
        let provider_key = provider_type_key(provider, CredentialKind::ApiKey);
        let session = context.session_id.clone();
        let total = selections.len();
        let order = self
            .database(move |_store, state| {
                Ok(state.assignments.credential_order(&provider_key, session.as_deref(), total))
            })
            .await?;
        let fallback = order.first().and_then(|index| selections.get(*index)).cloned();
        let Some(strategy) = self.strategy(provider) else {
            for index in order {
                if let Some(row) = selections.get(index)
                    && self.blocked(provider, row.id, CredentialKind::ApiKey, &[]).await?.is_none()
                {
                    return Ok(Some(row.clone()));
                }
            }
            return Ok(fallback);
        };
        let routing = policy::credential_block_routing(
            provider,
            "api_key",
            Some(strategy.as_ref()),
            context.model_id.as_deref(),
            None,
        );
        let context_policy = CredentialRankingContext { model_id: context.model_id.clone() };
        let raw = self
            .collect_candidate_usage(
                provider,
                context,
                &selections,
                &order,
                &routing.sibling_block_scopes,
                false,
                cancel,
            )
            .await?;
        let ranked = policy::rank_usage_candidates(
            raw,
            strategy.as_ref(),
            Some(&context_policy),
            PlanRequirement::None,
            self.now(),
        );
        for candidate in &ranked {
            if let Some(until) = candidate.new_block_until {
                self.mark_block(
                    provider,
                    candidate.candidate.selection.id,
                    CredentialKind::ApiKey,
                    until,
                    routing.block_scope.as_deref(),
                )
                .await?;
            }
        }
        Ok(ranked.into_iter().next().map(|candidate| candidate.candidate.selection).or(fallback))
    }

    // Native usage batches keep provider/context, rows/order, scopes, kind and cancellation independent.
    #[allow(clippy::too_many_arguments)]
    async fn collect_candidate_usage(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        selections: &[StoredAuthCredential],
        order: &[usize],
        scopes: &[String],
        oauth: bool,
        cancel: &CancellationToken,
    ) -> Result<Vec<UsageCandidate<StoredAuthCredential>>, AuthStorageError> {
        let kind = if oauth { CredentialKind::OAuth } else { CredentialKind::ApiKey };
        let work = join_all(
            order
                .iter()
                .enumerate()
                .filter_map(|(order_pos, index)| selections.get(*index).map(|row| (order_pos, row)))
                .map(|(order_pos, row)| async move {
                    let mut blocked_until = self.blocked(provider, row.id, kind, scopes).await?;
                    let mut usage = None;
                    let mut usage_checked = false;
                    if blocked_until.is_none() || (oauth && provider == "openai-codex") {
                        usage = self.usage_report(provider, row, context, false, cancel).await?;
                        usage_checked = true;
                        blocked_until = self.blocked(provider, row.id, kind, scopes).await?;
                    }
                    Ok(UsageCandidate { selection: row.clone(), usage, usage_checked, blocked_until, order_pos })
                }),
        );
        let timeout = self.inner.options.usage_request_timeout.mul_f64(1.5).max(Duration::from_secs(5));
        tokio::select! {
            _ = cancel.cancelled() => Err(AuthStorageError::Cancelled),
            result = tokio::time::timeout(timeout, work) => match result {
                Ok(results) => results.into_iter().collect(),
                Err(_) => {
                    let mut fallback = Vec::new();
                    for (order_pos, index) in order.iter().enumerate() { if let Some(row) = selections.get(*index) {
                        // Native OAuth batch timeout defaults to unknown blocks;
                        // the actual strict pass subsequently rechecks them.
                        let blocked_until = if oauth { None } else { self.blocked(provider, row.id, kind, scopes).await? };
                        fallback.push(UsageCandidate { selection: row.clone(), usage: None, usage_checked: false, blocked_until, order_pos });
                    }} Ok(fallback)
                }
            }
        }
    }

    fn expire_usage_cache(&self, provider: Option<&str>) -> Result<(), AuthStorageError> {
        self.inner.usage_epoch.fetch_add(1, Ordering::AcqRel);
        let prefix = provider
            .map(|provider| format!("{USAGE_CACHE_PREFIX}report:{}:", usage_provider_key(provider)))
            .unwrap_or_else(|| format!("{USAGE_CACHE_PREFIX}report:"));
        self.store_operation(|store, _state| store.delete_cache_prefix(&prefix).map_err(|_| AuthStorageError::Storage))
    }
    pub fn invalidate_usage_cache(&self, provider: Option<&str>) -> Result<(), AuthStorageError> {
        self.expire_usage_cache(provider)?;
        let key = force_cache_key(provider);
        let now = self.now();
        self.store_operation(|store, _state| {
            write_usage_cache(
                store,
                &key,
                &UsageCacheEntry { value: Some(Value::Bool(true)), expires_at: now + 300_000.0 },
                now,
            )
            .map_err(|_| AuthStorageError::Storage)
        })
    }
    fn force_usage_marked(&self, provider: Option<&str>) -> bool {
        self.store_operation(|store, _state| {
            Ok(read_usage_cache::<Value>(store, &force_cache_key(provider), false)
                .is_some_and(|entry| entry.expires_at > self.now() && entry.value == Some(Value::Bool(true))))
        })
        .unwrap_or(false)
    }

    /// Fixed OMP AuthStorage.ingestUsageHeaders local-cache branch (3631-3711).
    /// Attribution follows the active Session account at callback time. Native
    /// delegated-store ingestion and aggregate fetch overrides remain unported.
    /// This does not record history, heal blocks, refresh auth or select an account.
    pub fn ingest_usage_headers(
        &self,
        provider: &str,
        headers: &BTreeMap<String, String>,
        session_id: Option<&str>,
        base_url: Option<&str>,
    ) -> Result<bool, AuthStorageError> {
        let Some(hook) = self.usage_hook(provider) else { return Ok(false) };
        let Some(parser) = hook.rate_limit_header_parser() else { return Ok(false) };
        let lower_key_present =
            self.inner.options.environment.api_key(provider).is_some_and(|key| !key.value.is_empty())
                || self.inner.options.fallback.api_key(provider).is_some_and(|key| !key.is_empty());
        self.store_operation(|store, state| {
            if state.runtime_keys.contains_key(provider) || state.config_keys.contains_key(provider) {
                return Ok(false);
            }
            let Some(row) = Self::active_oauth_row_in_store(store, state, provider, session_id, lower_key_present)?
            else {
                return Ok(false);
            };
            let now = self.now();
            let Some(mut report) = parser(headers, now) else { return Ok(false) };
            let credential = usage_credential(&row, None);
            let request = UsageRequest {
                provider: provider.to_owned(),
                account_key: usage_identity(&credential),
                credential,
                credential_id: Some(row.id),
                base_url: base_url.map(str::to_owned),
            };
            let cache_key = usage_report_key(&request);
            let exhausted = report.limits.iter().any(policy::is_usage_limit_exhausted);
            if !exhausted
                && state
                    .usage_header_ingest_at
                    .get(&cache_key)
                    .is_some_and(|last| now - last < USAGE_HEADER_INGEST_INTERVAL_MS)
            {
                return Ok(false);
            }
            let metadata = report.metadata.get_or_insert_with(Map::new);
            for field in ["accountId", "email", "projectId", "orgId", "orgName"] {
                if let Some(value) = text_field(&row, field).filter(|value| !value.is_empty()) {
                    // JSON null is a defined native metadata property.
                    metadata.entry(field).or_insert_with(|| Value::String(value));
                }
            }
            // A malformed payload is a cold cache. A real store read failure
            // must not overwrite the entry or consume a successful-ingest slot.
            let prior_entry = store
                .get_cache(&format!("{USAGE_CACHE_PREFIX}{cache_key}"), true)
                .map_err(|_| AuthStorageError::Storage)?
                .and_then(|raw| serde_json::from_str::<UsageCacheEntry<UsageReport>>(&raw).ok());
            let expires_at = prior_entry.as_ref().map(|entry| entry.expires_at).unwrap_or(now - 1.0).max(now - 1.0);
            if let Some(mut prior) = prior_entry.and_then(|entry| entry.value) {
                let header_ids: Vec<_> = report.limits.iter().map(|limit| limit.id.clone()).collect();
                let mut replacements: BTreeMap<_, _> =
                    report.limits.into_iter().map(|limit| (limit.id.clone(), limit)).collect();
                let mut limits = Vec::with_capacity(prior.limits.len() + header_ids.len());
                for limit in prior.limits {
                    limits.push(replacements.remove(&limit.id).unwrap_or(limit));
                }
                for id in header_ids {
                    if let Some(limit) = replacements.remove(&id) {
                        limits.push(limit);
                    }
                }
                let mut metadata = report.metadata.take().unwrap_or_default();
                let source = prior.metadata.as_ref().and_then(|metadata| metadata.get("source")).cloned();
                metadata.extend(prior.metadata.take().unwrap_or_default());
                match source {
                    Some(source) => {
                        metadata.insert("source".into(), source);
                    }
                    None => {
                        metadata.remove("source");
                    }
                }
                metadata.insert("headersUpdatedAt".into(), Value::from(now));
                prior.fetched_at = now;
                prior.limits = limits;
                prior.metadata = Some(metadata);
                report = prior;
            }
            write_usage_cache(store, &cache_key, &UsageCacheEntry { value: Some(report), expires_at }, now)
                .map_err(|_| AuthStorageError::Storage)?;
            state.usage_header_ingest_at.insert(cache_key, now);
            Ok(true)
        })
    }

    pub async fn usage_report(
        &self,
        provider: &str,
        row: &StoredAuthCredential,
        context: &AuthRequestContext,
        force_refresh: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, AuthStorageError> {
        check_cancel(cancel)?;
        let Some(hook) = self.usage_hook(provider) else { return Ok(None) };
        let resolved_key = if let AuthCredential::ApiKey { key, .. } = &row.credential {
            self.inner.options.config_key_resolver.resolve(key, cancel).await?
        } else {
            None
        };
        if matches!(row.credential, AuthCredential::ApiKey { .. })
            && resolved_key.as_ref().is_none_or(|key| key.is_empty())
        {
            return Ok(None);
        }
        let credential = usage_credential(row, resolved_key);
        let request = UsageRequest {
            provider: provider.to_owned(),
            account_key: usage_identity(&credential),
            credential,
            credential_id: Some(row.id),
            base_url: context.base_url.clone(),
        };
        if !hook.supports(&request) {
            return Ok(None);
        }
        if hook.authoritative_oauth() && matches!(row.credential, AuthCredential::OAuth { .. }) {
            let report = tokio::select! {
                _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                result = tokio::time::timeout(self.inner.options.usage_request_timeout, hook.fetch_usage(request.clone(), cancel)) =>
                    result.ok().and_then(Result::ok).flatten(),
            };
            if let Some(report) = &report {
                self.reconcile_usage_request(&request, report).await?;
            }
            return Ok(report);
        }
        let cache_key = usage_report_key(&request);
        let epoch = self.inner.usage_epoch.load(Ordering::Acquire);
        let flight_key = format!("{cache_key}\0{epoch}");
        let mut receiver = {
            // Cache check and flight reservation share one short synchronous
            // section, so fast completion cannot create duplicate cold probes.
            let mut flights = self.inner.usage_flights.lock().map_err(|_| AuthStorageError::Storage)?;
            if !force_refresh {
                let cached = self
                    .store_operation(|store, _state| Ok(read_usage_cache::<UsageReport>(store, &cache_key, false)))?;
                if let Some(cached) = cached.filter(|cached| cached.expires_at > self.now()) {
                    return Ok(cached.value);
                }
            }
            if let Some(receiver) = flights.get(&flight_key) {
                receiver.clone()
            } else {
                let (sender, receiver) = watch::channel(None);
                flights.insert(flight_key.clone(), receiver.clone());
                self.inner.usage_pending.fetch_add(1, Ordering::AcqRel);
                let this = self.clone();
                let flight_key = flight_key.clone();
                let cache_key = cache_key.clone();
                tokio::spawn(async move {
                    let _settlement = UsageSettlement(this.clone());
                    let result = this.fetch_usage_owner(request, hook, cache_key, epoch, force_refresh).await;
                    sender.send_replace(Some(result));
                    if let Ok(mut flights) = this.inner.usage_flights.lock() {
                        flights.remove(&flight_key);
                    }
                });
                receiver
            }
        };
        loop {
            let ready = receiver.borrow().clone();
            if let Some(report) = ready {
                check_cancel(cancel)?;
                return Ok(report);
            }
            tokio::select! { _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
            result = receiver.changed() => if result.is_err() { return Ok(None) } }
        }
    }

    async fn fetch_usage_owner(
        &self,
        mut request: UsageRequest,
        hook: Arc<dyn UsageProvider>,
        cache_key: String,
        epoch: u64,
        force_refresh: bool,
    ) -> Option<UsageReport> {
        let fetch_cancel = CancellationToken::new();
        if request.credential.credential_type == UsageCredentialType::Oauth
            && request.credential.expires_at.is_some_and(|expires| self.now() + REFRESH_SKEW_MS >= expires)
            && let (Some(provider), Some(id)) = (self.oauth_hook(&request.provider), request.credential_id)
            && let Ok(rows) = self.provider_rows(&request.provider).await
            && let Some(row) = rows.into_iter().find(|row| row.id == id)
            && let Ok(Ok(Some(credential))) = tokio::time::timeout(
                self.inner.options.usage_request_timeout,
                provider.prepare_usage_credential(&row, &fetch_cancel),
            )
            .await
        {
            request.credential = credential;
            request.account_key = usage_identity(&request.credential);
        }
        let result = tokio::time::timeout(
            self.inner.options.usage_request_timeout,
            hook.fetch_usage(request.clone(), &fetch_cancel),
        )
        .await;
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                fetch_cancel.cancel();
                Err(UsageFetchError::Transient)
            }
        };
        if epoch != self.inner.usage_epoch.load(Ordering::Acquire) {
            return result.ok().flatten();
        }
        let now = self.now();
        let mut report = result.as_ref().ok().and_then(|report| report.clone());
        if let Some(report) = &mut report {
            attach_organization(&request.credential, report);
        }
        let ttl = if report.is_some() { USAGE_REPORT_TTL_MS as f64 } else { USAGE_FAILURE_BACKOFF_MS };
        let expiry = now + ttl + ttl * ((self.inner.options.jitter)() * 0.5 - 0.25);
        let auth_failure = matches!(result, Err(UsageFetchError::Unauthorized | UsageFetchError::Forbidden));
        let cached_report = if report.is_some() {
            report.clone()
        } else if !force_refresh && !auth_failure && hook.retain_last_good_on_failure() {
            self.store_operation(|store, _state| {
                Ok(read_usage_cache::<UsageReport>(store, &cache_key, true).and_then(|entry| entry.value))
            })
            .ok()
            .flatten()
        } else {
            None
        };
        // Epoch is checked again inside the store operation: invalidation that
        // raced the fetch or queue wait cannot resurrect stale report state.
        let _ = self.store_operation(|store, _state| {
            if epoch == self.inner.usage_epoch.load(Ordering::Acquire) {
                write_usage_cache(
                    store,
                    &cache_key,
                    &UsageCacheEntry { value: cached_report.clone(), expires_at: expiry },
                    now,
                )
                .map_err(|_| AuthStorageError::Storage)?;
                if let Some(report) = &report {
                    record_usage_history(store, &request, report);
                }
            }
            Ok(())
        });
        if let Some(report) = &report
            && epoch == self.inner.usage_epoch.load(Ordering::Acquire)
        {
            let _ = self.reconcile_usage_request(&request, report).await;
        }
        // Drop private tokens before returning the public usage report.
        request.credential.api_key = None;
        request.credential.access_token = None;
        request.credential.refresh_token = None;
        cached_report
    }

    pub async fn fetch_usage_reports(
        &self,
        provider: Option<&str>,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Vec<UsageReport>, AuthStorageError> {
        check_cancel(cancel)?;
        let provider = provider.map(str::to_owned);
        let rows = self
            .database(move |store, state| {
                store.list_auth_credentials(provider.as_deref()).map_err(|error| {
                    state.assignments.observe_store_error(&error);
                    AuthStorageError::Storage
                })
            })
            .await?;
        let mut groups = BTreeMap::<String, Vec<StoredAuthCredential>>::new();
        for row in rows {
            groups.entry(row.provider.clone()).or_default().push(row);
        }
        let epoch = self.inner.usage_epoch.load(Ordering::Acquire);
        let all_forced = self.force_usage_marked(None);
        let results = join_all(groups.into_iter().map(|(provider, rows)| async move {
            let force = all_forced || self.force_usage_marked(Some(&provider));
            let reports = if force {
                let mut reports = Vec::new();
                for row in &rows {
                    if let Some(report) = self.usage_report(&provider, row, context, true, cancel).await? {
                        reports.push(report);
                    }
                }
                reports
            } else {
                join_all(rows.iter().map(|row| self.usage_report(&provider, row, context, false, cancel)))
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect()
            };
            if force && epoch == self.inner.usage_epoch.load(Ordering::Acquire) {
                self.store_operation(|store, _state| {
                    store
                        .set_cache(&format!("{USAGE_CACHE_PREFIX}{}", force_cache_key(Some(&provider))), "", 0)
                        .map_err(|_| AuthStorageError::Storage)
                })?;
            }
            Ok::<_, AuthStorageError>(reports)
        }))
        .await;
        if all_forced && epoch == self.inner.usage_epoch.load(Ordering::Acquire) {
            self.store_operation(|store, _state| {
                store
                    .set_cache(&format!("{USAGE_CACHE_PREFIX}{}", force_cache_key(None)), "", 0)
                    .map_err(|_| AuthStorageError::Storage)
            })?;
        }
        Ok(results.into_iter().collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect())
    }

    async fn reconcile_usage_request(
        &self,
        request: &UsageRequest,
        report: &UsageReport,
    ) -> Result<(), AuthStorageError> {
        let Some(id) = request.credential_id else { return Ok(()) };
        let rows = self.provider_rows(&request.provider).await?;
        let Some(row) = rows.iter().find(|row| row.id == id && matches!(row.credential, AuthCredential::OAuth { .. }))
        else {
            return Ok(());
        };
        if !same_usage_identity(&request.credential, &usage_credential(row, None))
            || !report_matches_credential(report, row, false)
        {
            return Ok(());
        }
        self.reconcile_usage_row(&request.provider, id, report).await
    }
    async fn reconcile_usage_row(&self, provider: &str, id: i64, report: &UsageReport) -> Result<(), AuthStorageError> {
        let Some(strategy) = self.strategy(provider) else { return Ok(()) };
        let scopes: Vec<_> = if provider == "openai-codex" {
            let mut scopes = strategy.block_scopes(None);
            scopes.push(String::new());
            scopes
                .into_iter()
                .filter(|scope| {
                    policy::is_healthy_codex_usage_report(&policy::scope_codex_report_to_block_scope(
                        report,
                        Some(scope),
                        Some(strategy.as_ref()),
                    ))
                })
                .collect()
        } else {
            strategy
                .healable_block_scopes(report)
                .into_iter()
                .filter(|scope| !scope.limits.is_empty() && !policy::is_usage_limit_reached(&scope.limits))
                .map(|scope| scope.block_scope)
                .collect()
        };
        let provider = provider.to_owned();
        let now = self.now();
        let cleared = self
            .database(move |store, state| {
                let rows = Self::load_provider(store, state, &provider)?;
                let Some(index) = rows.iter().position(|row| row.id == id) else { return Ok(false) };
                let mut cleared = false;
                let mut seen = BTreeSet::new();
                for scope in scopes {
                    if seen.insert(scope.clone()) {
                        cleared |= state
                            .assignments
                            .clear_healed_block_scope(
                                store,
                                &provider_type_key(&provider, CredentialKind::OAuth),
                                id,
                                index,
                                Some(&scope),
                                &rows,
                                now,
                            )
                            .is_some();
                    }
                }
                Ok(cleared)
            })
            .await?;
        if cleared {
            self.bump_generation();
        }
        Ok(())
    }
    pub async fn reconcile_usage_reports(&self, reports: &[UsageReport]) -> Result<(), AuthStorageError> {
        let mut reconciled = BTreeSet::new();
        for report in reports {
            for row in self.provider_rows(&report.provider).await? {
                if matches!(row.credential, AuthCredential::OAuth { .. })
                    && report_matches_credential(report, &row, true)
                    && reconciled.insert(row.id)
                {
                    self.reconcile_usage_row(&report.provider, row.id, report).await?;
                }
            }
        }
        Ok(())
    }

    async fn feedback_target(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        quota_alias: bool,
        cancel: &CancellationToken,
    ) -> Result<Option<(StoredAuthCredential, CredentialTarget)>, AuthStorageError> {
        check_cancel(cancel)?;
        // A wire token's bytes do not give an override/env/fallback lease
        // ownership of a durable row, even when a stored sibling uses them.
        if !matches!(failed.identity(), CredentialIdentity::Stored { .. }) {
            return Ok(None);
        }
        let id = match failed.identity() {
            CredentialIdentity::Stored { id, .. } => Some(*id),
            _ => None,
        };
        let bearer = failed.api_key();
        let rows = self.provider_rows(provider).await?;
        let mut resolved_keys = Vec::new();
        if id.is_none() && bearer.is_some() {
            for row in &rows {
                if let AuthCredential::ApiKey { key, .. } = &row.credential
                    && let Some(key) = self.inner.options.config_key_resolver.resolve(key, cancel).await?
                {
                    resolved_keys.push((row.id, key));
                }
            }
        }
        let provider = provider.to_owned();
        let session = context.session_id.clone();
        let bearer = bearer.map(str::to_owned);
        self.database(move |store, state| {
            let rows = Self::load_provider(store, state, &provider)?;
            // An owned failed lease has a durable row boundary. If that row
            // vanished, equal bytes on a sibling cannot authorize adopting it.
            if id.is_some_and(|id| !rows.iter().any(|row| row.id == id)) {
                return Ok(None);
            }
            let target = state
                .assignments
                .resolve_credential_target(
                    store,
                    &provider,
                    session.as_deref(),
                    &rows,
                    id,
                    bearer.as_deref(),
                    &resolved_keys,
                )
                .or_else(|| {
                    if quota_alias && id.is_none() {
                        bearer
                            .as_deref()
                            .and_then(|bearer| state.assignments.find_bearer_quota_target(&provider, bearer, &rows))
                    } else {
                        None
                    }
                });
            Ok(target.and_then(|target| rows.get(target.index).cloned().map(|row| (row, target))))
        })
        .await
    }
    async fn sibling_availability(
        &self,
        provider: &str,
        id: i64,
        kind: CredentialKind,
        scopes: &[String],
        excluded: &BTreeSet<i64>,
    ) -> Result<UsageLimitMarkResult, AuthStorageError> {
        let mut retry_at: Option<f64> = None;
        for row in
            self.provider_rows(provider).await?.into_iter().filter(|row| {
                row.id != id && CredentialKind::of(&row.credential) == kind && !excluded.contains(&row.id)
            })
        {
            match self.blocked(provider, row.id, kind, scopes).await? {
                None => return Ok(UsageLimitMarkResult { switched: true, retry_at_ms: None }),
                Some(until) => {
                    if retry_at.is_none_or(|current| until < current) {
                        retry_at = Some(until)
                    }
                }
            }
        }
        Ok(UsageLimitMarkResult { switched: false, retry_at_ms: retry_at })
    }
    pub async fn mark_usage_limit_reached(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        retry_after_ms: Option<f64>,
        cancel: &CancellationToken,
    ) -> Result<UsageLimitMarkResult, AuthStorageError> {
        check_cancel(cancel)?;
        let Some((row, target)) = self.feedback_target(provider, context, failed, true, cancel).await? else {
            return Ok(UsageLimitMarkResult::default());
        };
        let strategy = self.strategy(provider);
        let routing = policy::credential_block_routing(
            provider,
            target.kind.as_str(),
            strategy.as_deref(),
            context.model_id.as_deref(),
            None,
        );
        let mut until = self.now() + retry_after_ms.unwrap_or(policy::DEFAULT_BACKOFF_MS);
        if let Some(strategy) = strategy.as_deref()
            && let Some(report) = self.usage_report(provider, &row, context, false, cancel).await?
        {
            let limits = strategy.scope_limits(&report, Some(&routing.ranking_context));
            if let Some(reset) = policy::usage_reset_at_ms(&limits, self.now())
                && reset > until
            {
                until = reset
            }
        }
        // The usage await may have changed positional indexes. mark_block
        // resolves the captured durable ID again rather than trusting target.
        self.mark_block(provider, row.id, target.kind, until, routing.block_scope.as_deref()).await?;
        self.sibling_availability(
            provider,
            row.id,
            target.kind,
            &routing.sibling_block_scopes,
            &context.excluded_credential_ids,
        )
        .await
    }
    pub async fn mark_account_policy_denied(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        exact_model: bool,
        cancel: &CancellationToken,
    ) -> Result<UsageLimitMarkResult, AuthStorageError> {
        let Some((row, target)) = self.feedback_target(provider, context, failed, false, cancel).await? else {
            return Ok(UsageLimitMarkResult::default());
        };
        let model_scope = if exact_model {
            policy::model_account_policy_block_scope(provider, context.model_id.as_deref())
        } else {
            None
        };
        if exact_model && model_scope.is_none() {
            return Ok(UsageLimitMarkResult::default());
        }
        let strategy = self.strategy(provider);
        let routing = policy::credential_block_routing(
            provider,
            target.kind.as_str(),
            strategy.as_deref(),
            context.model_id.as_deref(),
            model_scope.as_deref(),
        );
        self.mark_block(
            provider,
            row.id,
            target.kind,
            self.now() + policy::DEFAULT_BACKOFF_MS,
            routing.block_scope.as_deref(),
        )
        .await?;
        self.sibling_availability(
            provider,
            row.id,
            target.kind,
            &routing.sibling_block_scopes,
            &context.excluded_credential_ids,
        )
        .await
    }
    pub async fn refresh_failed_lease(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        cancel: &CancellationToken,
    ) -> Result<Option<AuthAccess>, AuthStorageError> {
        let Some((row, target)) = self.feedback_target(provider, context, failed, false, cancel).await? else {
            return Ok(None);
        };
        if target.kind != CredentialKind::OAuth || self.suppressed(provider) {
            return Ok(None);
        }
        // Peer rotation wins. A failed lease is not permission to exchange the
        // peer's already-fresh refresh token for the same logical failure.
        let same_bearer = current_bearer_matches(&row, failed.api_key());
        let mut exact_context = context.clone();
        exact_context.force_refresh = same_bearer;
        self.get_oauth_access_by_credential_id(provider, row.id, &exact_context, cancel).await
    }
    pub async fn invalidate_credential_matching(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        disable: bool,
        cancel: &CancellationToken,
    ) -> Result<bool, AuthStorageError> {
        let Some((row, target)) = self.feedback_target(provider, context, failed, false, cancel).await? else {
            return Ok(false);
        };
        // Hard auth invalidation is current-token-only; no historical quota alias.
        if target.kind == CredentialKind::OAuth && !current_bearer_matches(&row, failed.api_key()) {
            return Ok(false);
        }
        let siblings =
            self.sibling_availability(provider, row.id, target.kind, &[], &context.excluded_credential_ids).await?;
        self.mark_block(provider, row.id, target.kind, self.now() + policy::DEFAULT_BACKOFF_MS, None).await?;
        let provider = provider.to_owned();
        let session = context.session_id.clone();
        let changed = self
            .database(move |store, state| {
                let rows = Self::load_provider(store, state, &provider)?;
                let sticky = state.assignments.read_session_credential(store, &provider, session.as_deref(), &rows);
                if !target.explicit
                    || sticky.is_some_and(|sticky| rows.get(sticky.index).is_some_and(|selected| selected.id == row.id))
                {
                    state.assignments.clear_session_credential(store, &provider, session.as_deref());
                }
                let changed = if disable {
                    store
                        .try_disable_auth_credential_if_matches(
                            row.id,
                            &row.serialized_data,
                            "upstream invalidated OAuth token",
                            None,
                        )
                        .map_err(|_| AuthStorageError::Storage)?
                } else {
                    false
                };
                if changed {
                    state.assignments.reset_provider_assignments(store, &provider);
                }
                Self::load_provider(store, state, &provider)?;
                Ok(changed)
            })
            .await?;
        if changed {
            self.bump_generation();
        }
        Ok(siblings.switched && (!disable || changed))
    }
    pub async fn rotate_session_credential(
        &self,
        provider: &str,
        context: &AuthRequestContext,
        failed: &RequestAuthLease,
        category: AuthFailureCategory,
        retry_after_ms: Option<f64>,
        cancel: &CancellationToken,
    ) -> Result<UsageLimitMarkResult, AuthStorageError> {
        match category {
            AuthFailureCategory::Quota => {
                self.mark_usage_limit_reached(provider, context, failed, retry_after_ms, cancel).await
            }
            AuthFailureCategory::AccountPolicy => {
                self.mark_account_policy_denied(provider, context, failed, false, cancel).await
            }
            AuthFailureCategory::ModelAccountPolicy => {
                self.mark_account_policy_denied(provider, context, failed, true, cancel).await
            }
            AuthFailureCategory::InvalidatedOAuth | AuthFailureCategory::HardAuth => self
                .invalidate_credential_matching(
                    provider,
                    context,
                    failed,
                    category == AuthFailureCategory::InvalidatedOAuth,
                    cancel,
                )
                .await
                .map(|switched| UsageLimitMarkResult { switched, retry_at_ms: None }),
        }
    }
    pub async fn wait_for_settlement(&self) {
        loop {
            let notified = self.inner.usage_settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.usage_pending.load(Ordering::Acquire) == 0 {
                break;
            }
            notified.await;
        }
        if let Some(codex) = &self.inner.codex {
            codex.wait_for_settlement().await;
        }
        self.inner.options.config_key_resolver.wait_for_settlement().await;
        let providers: Vec<_> = self
            .inner
            .oauth_providers
            .lock()
            .map(|providers| providers.values().cloned().collect())
            .unwrap_or_default();
        for provider in providers {
            provider.wait_for_settlement().await;
        }
    }
    pub fn session_resolver(&self, session_id: Option<String>, base_url: Option<String>) -> Arc<SessionAuthResolver> {
        Arc::new(SessionAuthResolver { storage: self.clone(), session_id, base_url, environment_key: None })
    }
    pub fn list_credential_blocks(&self, ids: &[i64]) -> Result<Vec<StoredCredentialBlock>, AuthStorageError> {
        self.store_operation(|store, state| {
            state.assignments.list_credential_blocks(store, ids).map_err(|_| AuthStorageError::Storage)
        })
    }
    pub fn upsert_credential_block(&self, block: &StoredCredentialBlock) -> Result<(), AuthStorageError> {
        self.store_operation(|store, state| {
            state.assignments.upsert_credential_block(store, block).map_err(|_| AuthStorageError::Storage)
        })?;
        if let Some((provider, _)) = block.provider_key.rsplit_once(':') {
            self.expire_usage_cache(Some(provider))?;
        }
        self.bump_generation();
        Ok(())
    }
    pub fn delete_credential_block(&self, id: i64, provider_key: &str, scope: &str) -> Result<(), AuthStorageError> {
        self.store_operation(|store, state| {
            state
                .assignments
                .delete_credential_block(store, id, provider_key, scope)
                .map_err(|_| AuthStorageError::Storage)
        })?;
        if let Some((provider, _)) = provider_key.rsplit_once(':') {
            self.expire_usage_cache(Some(provider))?;
        }
        self.bump_generation();
        Ok(())
    }
    pub fn delete_credential_blocks(&self, id: i64) -> Result<(), AuthStorageError> {
        self.store_operation(|store, state| {
            state.assignments.delete_credential_blocks(store, id).map_err(|_| AuthStorageError::Storage)
        })?;
        self.bump_generation();
        Ok(())
    }
    pub fn reset_provider_assignments(&self, provider: &str) -> Result<(), AuthStorageError> {
        self.store_operation(|store, state| {
            state.assignments.reset_provider_assignments(store, provider);
            Ok(())
        })
    }
    pub fn persisted_block_store_damaged(&self) -> Result<bool, AuthStorageError> {
        self.store_operation(|_store, state| Ok(state.assignments.persisted_block_store_damaged()))
    }
    pub fn take_block_store_damage_notice(&self) -> Result<bool, AuthStorageError> {
        self.store_operation(|_store, state| Ok(state.assignments.take_block_store_damage_notice()))
    }
}

struct UsageSettlement(AuthStorage);
impl Drop for UsageSettlement {
    fn drop(&mut self) {
        self.0.inner.usage_pending.fetch_sub(1, Ordering::AcqRel);
        self.0.inner.usage_settled.notify_waiters();
    }
}

/// Session/model/baseUrl are carried to the same service used by the Registry.
/// No Core state or product identity enters this Host wrapper.
pub struct SessionAuthResolver {
    storage: AuthStorage,
    session_id: Option<String>,
    base_url: Option<String>,
    environment_key: Option<EnvironmentKey>,
}
impl SessionAuthResolver {
    /// Ordinary reference-Host defaults participate after login credentials
    /// and before stored static keys, including custom catalog providers.
    pub fn with_environment_lease(&self, lease: &RequestAuthLease) -> Arc<Self> {
        let environment_key = match (lease.identity(), lease.api_key()) {
            (CredentialIdentity::Environment { variable }, Some(value)) => {
                Some(EnvironmentKey { variable: variable.clone(), value: value.to_owned() })
            }
            _ => None,
        };
        Arc::new(Self {
            storage: self.storage.clone(),
            session_id: self.session_id.clone(),
            base_url: self.base_url.clone(),
            environment_key,
        })
    }
    fn context(&self, model: &Model) -> AuthRequestContext {
        AuthRequestContext {
            session_id: self.session_id.clone(),
            model_id: Some(model.id.clone()),
            base_url: self.base_url.clone().or_else(|| Some(model.base_url.clone())),
            environment_key: self.environment_key.clone(),
            ..Default::default()
        }
    }
}
#[async_trait]
impl RequestAuthResolver for SessionAuthResolver {
    async fn resolve(&self, model: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        self.storage
            .resolve(&model.provider, &self.context(model), cancel)
            .await
            .map_err(AuthResolveError::from)?
            .map(|access| access.lease)
            .ok_or(AuthResolveError::Unavailable)
    }
    fn requires_settlement(&self) -> bool {
        true
    }
    fn supports_auth_retry(&self) -> bool {
        true
    }
    fn supports_auth_refresh(&self) -> bool {
        true
    }
    async fn refresh(
        &self,
        model: &Model,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        let mut context = self.context(model);
        context.force_refresh = true;
        Ok(self
            .storage
            .resolve(&model.provider, &context, cancel)
            .await
            .map_err(AuthResolveError::from)?
            .map(|access| access.lease))
    }
    async fn retry(
        &self,
        model: &Model,
        failed: &RequestAuthLease,
        failure: &RequestAuthFailure,
        action: AuthRetryAction,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        let mut context = self.context(model);
        if action == AuthRetryAction::RefreshSame {
            return Ok(self
                .storage
                .refresh_failed_lease(&model.provider, &context, failed, cancel)
                .await
                .map_err(AuthResolveError::from)?
                .map(|access| access.lease));
        }
        let category = match failure.kind {
            RequestAuthFailureKind::Quota => AuthFailureCategory::Quota,
            RequestAuthFailureKind::AccountPolicy => AuthFailureCategory::AccountPolicy,
            RequestAuthFailureKind::ModelAccountPolicy => AuthFailureCategory::ModelAccountPolicy,
            RequestAuthFailureKind::InvalidatedOAuth => AuthFailureCategory::InvalidatedOAuth,
            RequestAuthFailureKind::Auth | RequestAuthFailureKind::Forbidden | RequestAuthFailureKind::TokenRefresh => {
                AuthFailureCategory::HardAuth
            }
        };
        let outcome = self
            .storage
            .rotate_session_credential(&model.provider, &context, failed, category, failure.retry_after_ms, cancel)
            .await
            .map_err(AuthResolveError::from)?;
        if !outcome.switched {
            // Native hard-auth last chance may use a peer's replacement bearer,
            // while a no-sibling quota result must preserve the blocked failure.
            if category == AuthFailureCategory::HardAuth
                && let Some((row, target)) = self
                    .storage
                    .feedback_target(&model.provider, &context, failed, false, cancel)
                    .await
                    .map_err(AuthResolveError::from)?
                && target.kind == CredentialKind::OAuth
                && !current_bearer_matches(&row, failed.api_key())
            {
                return Ok(self
                    .storage
                    .get_oauth_access_by_credential_id(&model.provider, row.id, &context, cancel)
                    .await
                    .map_err(AuthResolveError::from)?
                    .map(|access| access.lease));
            }
            return Ok(None);
        }
        if let CredentialIdentity::Stored { id, .. } = failed.identity() {
            context.excluded_credential_ids.insert(*id);
        }
        Ok(self
            .storage
            .resolve(&model.provider, &context, cancel)
            .await
            .map_err(AuthResolveError::from)?
            .map(|access| access.lease))
    }
    fn for_session(&self, session_id: &str) -> Option<Arc<dyn RequestAuthResolver>> {
        Some(Arc::new(Self {
            storage: self.storage.clone(),
            session_id: Some(session_id.to_owned()),
            base_url: self.base_url.clone(),
            environment_key: self.environment_key.clone(),
        }))
    }
}

#[async_trait]
impl UsageProvider for crate::codex_usage::CodexUsageProvider {
    fn rate_limit_header_parser(&self) -> Option<RateLimitHeaderParser> {
        Some(crate::codex_usage::parse_codex_rate_limit_headers)
    }
    async fn fetch_usage(
        &self,
        request: UsageRequest,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, UsageFetchError> {
        crate::codex_usage::CodexUsageProvider::fetch_usage_result(
            self,
            &request.provider,
            &request.credential,
            request.base_url.as_deref(),
            Duration::from_secs(10),
            cancel,
        )
        .await
        .map_err(|error| match error {
            crate::codex_usage::CodexUsageError::Cancelled => UsageFetchError::Cancelled,
            crate::codex_usage::CodexUsageError::Unauthorized => UsageFetchError::Unauthorized,
            crate::codex_usage::CodexUsageError::Forbidden => UsageFetchError::Forbidden,
            crate::codex_usage::CodexUsageError::Transient => UsageFetchError::Transient,
            crate::codex_usage::CodexUsageError::InvalidResponse => UsageFetchError::InvalidResponse,
        })
    }
    fn supports(&self, request: &UsageRequest) -> bool {
        crate::codex_usage::CodexUsageProvider::supports(self, &request.provider, &request.credential)
    }
}

fn unranked_candidates<T>(
    candidates: Vec<UsageCandidate<T>>,
    requirement: PlanRequirement,
) -> Vec<UsageRankedCandidate<T>> {
    candidates
        .into_iter()
        .map(|candidate| {
            let plan_eligibility = policy::codex_plan_eligibility(candidate.usage.as_ref(), requirement);
            let score = policy::UsageRankingScore {
                blocked: candidate.blocked_until.is_some(),
                blocked_until: candidate.blocked_until,
                has_priority_boost: false,
                usage_measured: false,
                plan_priority: policy::codex_plan_priority(candidate.usage.as_ref(), requirement),
                secondary_used: 0.5,
                secondary_required_drain: 0.0,
                primary_used: 0.5,
                primary_required_drain: 0.0,
                order_pos: candidate.order_pos,
            };
            UsageRankedCandidate { candidate, score, plan_eligibility, new_block_until: None }
        })
        .collect()
}
fn current_bearer_matches(row: &StoredAuthCredential, bearer: Option<&str>) -> bool {
    let Some(bearer) = bearer else { return false };
    if text_field(row, "access").as_deref() == Some(bearer) {
        return true;
    }
    bearer.starts_with('{')
        && serde_json::from_str::<Value>(bearer)
            .ok()
            .and_then(|value| value.get("token").and_then(Value::as_str).map(str::to_owned))
            == text_field(row, "access")
}
fn usage_credential(row: &StoredAuthCredential, resolved_key: Option<String>) -> UsageCredential {
    UsageCredential {
        credential_type: match row.credential {
            AuthCredential::ApiKey { .. } => UsageCredentialType::ApiKey,
            _ => UsageCredentialType::Oauth,
        },
        api_key: resolved_key,
        access_token: text_field(row, "access"),
        refresh_token: text_field(row, "refresh"),
        expires_at: number_field(row, "expires"),
        account_id: text_field(row, "accountId"),
        project_id: text_field(row, "projectId"),
        email: text_field(row, "email"),
        org_id: text_field(row, "orgId"),
        org_name: text_field(row, "orgName"),
        enterprise_url: text_field(row, "enterpriseUrl"),
        api_endpoint: text_field(row, "apiEndpoint"),
        metadata: match &row.credential {
            AuthCredential::OAuth { fields } => fields.get("metadata").and_then(Value::as_object).cloned(),
            _ => None,
        },
        unknown_fields: Map::new(),
    }
}
fn js_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn trim_js(text: &str) -> &str {
    text.trim_matches(js_whitespace)
}
fn normalized_identity(text: Option<&str>) -> Option<String> {
    text.map(trim_js).filter(|value| !value.is_empty()).map(str::to_lowercase)
}
fn usage_identity(credential: &UsageCredential) -> String {
    let mut parts =
        vec![if credential.credential_type == UsageCredentialType::Oauth { "oauth" } else { "api_key" }.to_owned()];
    let account = credential.account_id.as_deref().map(trim_js).filter(|value| !value.is_empty());
    let email = credential.email.as_deref().map(trim_js).filter(|value| !value.is_empty());
    let org = credential.org_id.as_deref().map(trim_js).filter(|value| !value.is_empty());
    for (name, value) in [
        ("account", account.map(str::to_owned)),
        ("email", email.map(str::to_lowercase)),
        ("org", org.map(str::to_owned)),
        ("project", credential.project_id.as_deref().map(trim_js).filter(|value| !value.is_empty()).map(str::to_owned)),
        (
            "enterprise",
            credential.enterprise_url.as_deref().map(trim_js).filter(|value| !value.is_empty()).map(str::to_lowercase),
        ),
    ] {
        if let Some(value) = value {
            parts.push(format!("{name}:{value}"));
        }
    }
    if account.is_none() && email.is_none() && org.is_none() {
        let secret =
            [credential.api_key.as_deref(), credential.refresh_token.as_deref(), credential.access_token.as_deref()]
                .into_iter()
                .flatten()
                .map(trim_js)
                .find(|secret| !secret.is_empty());
        parts.push(
            secret
                .map(|secret| format!("secret:{:x}", crate::bun_hash::hash_string(&ara_rpc::WireString::from(secret))))
                .unwrap_or_else(|| "anonymous".to_owned()),
        );
    }
    parts.join("|")
}
fn usage_provider_key(provider: &str) -> String {
    match provider {
        "google-antigravity" | "zai" | "opencode-go" => format!("2:{provider}"),
        "anthropic" => format!("3:{provider}"),
        _ => provider.to_owned(),
    }
}
fn usage_report_key(request: &UsageRequest) -> String {
    let base_url = request
        .base_url
        .as_deref()
        .map(trim_js)
        .map(|value| value.trim_end_matches('/'))
        .filter(|value| !value.is_empty())
        .unwrap_or("default");
    format!("report:{}:{base_url}:{}", usage_provider_key(&request.provider), request.account_key)
}
fn force_cache_key(provider: Option<&str>) -> String {
    provider
        .map(|provider| format!("force-refresh:provider:{provider}"))
        .unwrap_or_else(|| "force-refresh:all".to_owned())
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageCacheEntry<T> {
    value: Option<T>,
    expires_at: f64,
}
fn read_usage_cache<T: for<'de> Deserialize<'de>>(
    store: &SqliteCredentialStore,
    key: &str,
    stale: bool,
) -> Option<UsageCacheEntry<T>> {
    let raw = store.get_cache(&format!("{USAGE_CACHE_PREFIX}{key}"), stale).ok()??;
    serde_json::from_str(&raw).ok()
}
fn write_usage_cache<T: Serialize>(
    store: &SqliteCredentialStore,
    key: &str,
    entry: &UsageCacheEntry<T>,
    now: f64,
) -> anyhow::Result<()> {
    let durable_expiry =
        if entry.value.is_none() { entry.expires_at } else { entry.expires_at.max(now + LAST_GOOD_RETENTION_MS) };
    store.set_cache(
        &format!("{USAGE_CACHE_PREFIX}{key}"),
        &serde_json::to_string(entry)?,
        (durable_expiry / 1000.0).floor() as i64,
    )
}
fn attach_organization(credential: &UsageCredential, report: &mut UsageReport) {
    if let Some(org) = &credential.org_id {
        let metadata = report.metadata.get_or_insert_with(Map::new);
        let same_org = metadata.get("orgId").is_none_or(|report_org| report_org.as_str() == Some(org));
        metadata.entry("orgId").or_insert_with(|| Value::String(org.clone()));
        if same_org && let Some(name) = &credential.org_name {
            metadata.entry("orgName").or_insert_with(|| Value::String(name.clone()));
        }
    }
}
fn same_usage_identity(previous: &UsageCredential, current: &UsageCredential) -> bool {
    let dimensions = [
        (previous.account_id.as_deref(), current.account_id.as_deref()),
        (previous.email.as_deref(), current.email.as_deref()),
        (previous.org_id.as_deref(), current.org_id.as_deref()),
        (previous.project_id.as_deref(), current.project_id.as_deref()),
    ];
    let mut comparable = false;
    for (previous, current) in dimensions {
        if let (Some(previous), Some(current)) = (normalized_identity(previous), normalized_identity(current)) {
            comparable = true;
            if previous != current {
                return false;
            }
        }
    }
    if comparable {
        true
    } else {
        previous.access_token.as_ref().is_some_and(|token| current.access_token.as_ref() == Some(token))
            || previous
                .refresh_token
                .as_ref()
                .filter(|token| !token.is_empty() && token.as_str() != "__remote__")
                .is_some_and(|token| current.refresh_token.as_ref() == Some(token))
    }
}
fn report_matches_credential(report: &UsageReport, row: &StoredAuthCredential, require_comparable: bool) -> bool {
    if report.provider != row.provider {
        return false;
    }
    let metadata = report.metadata.as_ref();
    let report_email = normalized_identity(metadata.and_then(|metadata| metadata.get("email")).and_then(Value::as_str));
    let report_account = normalized_identity(
        metadata
            .and_then(|metadata| metadata.get("accountId"))
            .and_then(Value::as_str)
            .or_else(|| report.limits.iter().find_map(|limit| limit.scope.account_id.as_deref())),
    );
    let email = text_field(row, "email");
    let account = text_field(row, "accountId");
    let mut comparable = false;
    for (report, credential) in [
        (report_email, normalized_identity(email.as_deref())),
        (report_account, normalized_identity(account.as_deref())),
    ] {
        if let (Some(report), Some(credential)) = (report, credential) {
            comparable = true;
            if report != credential {
                return false;
            }
        }
    }
    comparable || !require_comparable
}
fn record_usage_history(store: &SqliteCredentialStore, request: &UsageRequest, report: &UsageReport) {
    let entries: Vec<_> = report
        .limits
        .iter()
        .map(|limit| UsageHistoryEntry {
            recorded_at: report.fetched_at as i64,
            provider: report.provider.clone(),
            account_key: request.account_key.clone(),
            email: request.credential.email.clone(),
            account_id: request.credential.account_id.clone(),
            limit_id: limit.id.clone(),
            label: limit.label.clone(),
            window_label: limit.window.as_ref().map(|window| window.label.clone()),
            used_fraction: policy::resolve_used_fraction(limit),
            status: limit
                .status
                .and_then(|status| serde_json::to_value(status).ok())
                .and_then(|status| status.as_str().map(str::to_owned)),
            resets_at: limit.window.as_ref().and_then(|window| window.resets_at).map(|reset| reset as i64),
        })
        .collect();
    let _ = store.record_usage_snapshots(&entries);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use tokio::sync::Semaphore;

    struct FixtureEnvironment;
    impl Environment for FixtureEnvironment {
        fn api_key(&self, _: &str) -> Option<EnvironmentKey> {
            Some(EnvironmentKey { variable: "FIXTURE_KEY".into(), value: "environment".into() })
        }
    }
    struct FixtureFallback;
    impl Fallback for FixtureFallback {
        fn api_key(&self, _: &str) -> Option<String> {
            Some("fallback".into())
        }
    }
    #[derive(Default)]
    struct FixtureConfig {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl ConfigKeyResolver for FixtureConfig {
        async fn resolve(
            &self,
            configuration: &str,
            cancel: &CancellationToken,
        ) -> Result<Option<String>, AuthStorageError> {
            check_cancel(cancel)?;
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(Some(if configuration == "!login" { "login-resolved" } else { configuration }.to_owned()))
        }
    }
    struct FixtureOAuth {
        store: Arc<Mutex<SqliteCredentialStore>>,
        failures: Mutex<BTreeMap<i64, AuthStorageError>>,
        refreshes: Mutex<Vec<i64>>,
        preparing: AtomicUsize,
        max_parallel: AtomicUsize,
    }
    impl FixtureOAuth {
        fn new(store: Arc<Mutex<SqliteCredentialStore>>) -> Self {
            Self {
                store,
                failures: Mutex::new(BTreeMap::new()),
                refreshes: Mutex::new(Vec::new()),
                preparing: AtomicUsize::new(0),
                max_parallel: AtomicUsize::new(0),
            }
        }
    }
    #[async_trait]
    impl OAuthProvider for FixtureOAuth {
        async fn prepare_for_request(
            &self,
            _: &StoredAuthCredential,
            cancel: &CancellationToken,
        ) -> Result<(), AuthStorageError> {
            check_cancel(cancel)?;
            let parallel = self.preparing.fetch_add(1, Ordering::AcqRel) + 1;
            self.max_parallel.fetch_max(parallel, Ordering::AcqRel);
            tokio::task::yield_now().await;
            self.preparing.fetch_sub(1, Ordering::AcqRel);
            Ok(())
        }
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
            Ok(RequestAuthLease::new(
                CredentialIdentity::Stored { id: row.id, revision: row.revision },
                text_field(row, "access"),
            )
            .with_headers(vec![("fixture-account".into(), text_field(row, "accountId").unwrap_or_default())]))
        }
        async fn resolve(
            &self,
            row: StoredAuthCredential,
            _: bool,
            cancel: &CancellationToken,
        ) -> Result<ResolvedOAuth, AuthStorageError> {
            check_cancel(cancel)?;
            self.refreshes.lock().unwrap().push(row.id);
            if let Some(error) = self.failures.lock().unwrap().get(&row.id).copied() {
                return Err(error);
            }
            let mut credential = row.credential.clone();
            if let AuthCredential::OAuth { fields } = &mut credential {
                fields.insert("access".into(), Value::String(format!("refreshed-{}", row.id)));
                fields.insert("expires".into(), Value::from(chrono::Utc::now().timestamp_millis() + 7_200_000));
            }
            let store = self.store.lock().unwrap();
            assert!(
                store.try_update_auth_credential_if_matches(row.id, &row.serialized_data, &credential, None).unwrap()
            );
            let row = store
                .list_auth_credentials(Some(&row.provider))
                .unwrap()
                .into_iter()
                .find(|fresh| fresh.id == row.id)
                .unwrap();
            Ok(ResolvedOAuth { lease: self.lease(&row)?, row })
        }
    }
    type UsageOutcome = (Option<Arc<Semaphore>>, Result<Option<UsageReport>, UsageFetchError>);
    struct FixtureUsage {
        queue: Mutex<VecDeque<UsageOutcome>>,
        reports: Mutex<BTreeMap<i64, UsageReport>>,
        calls: AtomicUsize,
        started: Semaphore,
    }
    impl FixtureUsage {
        fn new() -> Self {
            Self {
                queue: Mutex::new(VecDeque::new()),
                reports: Mutex::new(BTreeMap::new()),
                calls: AtomicUsize::new(0),
                started: Semaphore::new(0),
            }
        }
        fn push(&self, gate: Option<Arc<Semaphore>>, result: Result<Option<UsageReport>, UsageFetchError>) {
            self.queue.lock().unwrap().push_back((gate, result));
        }
        async fn wait_started(&self) {
            tokio::time::timeout(Duration::from_secs(1), self.started.acquire())
                .await
                .expect("usage fixture did not start")
                .unwrap()
                .forget();
        }
    }
    #[async_trait]
    impl UsageProvider for FixtureUsage {
        async fn fetch_usage(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, UsageFetchError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.started.add_permits(1);
            let queued = self.queue.lock().unwrap().pop_front();
            if let Some((gate, result)) = queued {
                if let Some(gate) = gate {
                    tokio::select! { _ = cancel.cancelled() => return Err(UsageFetchError::Cancelled), permit = gate.acquire() => permit.unwrap().forget() }
                }
                return result;
            }
            Ok(request.credential_id.and_then(|id| self.reports.lock().unwrap().get(&id).cloned()))
        }
    }
    struct FixtureHeaderUsage {
        usage: Arc<FixtureUsage>,
        parser: RateLimitHeaderParser,
    }
    #[async_trait]
    impl UsageProvider for FixtureHeaderUsage {
        async fn fetch_usage(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, UsageFetchError> {
            self.usage.fetch_usage(request, cancel).await
        }
        fn rate_limit_header_parser(&self) -> Option<RateLimitHeaderParser> {
            Some(self.parser)
        }
    }
    fn codex_headers_with_null_email(headers: &BTreeMap<String, String>, now_ms: f64) -> Option<UsageReport> {
        let mut report = crate::codex_usage::parse_codex_rate_limit_headers(headers, now_ms)?;
        report.metadata.get_or_insert_with(Map::new).insert("email".into(), Value::Null);
        Some(report)
    }
    fn codex_headers(now: f64, primary: &str, secondary: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("x-codex-primary-used-percent".into(), primary.into()),
            ("x-codex-primary-window-minutes".into(), "300".into()),
            ("x-codex-primary-reset-at".into(), ((now + 3_600_000.0) / 1000.0).floor().to_string()),
            ("x-codex-secondary-used-percent".into(), secondary.into()),
            ("x-codex-secondary-window-minutes".into(), (7 * 24 * 60).to_string()),
            ("x-codex-secondary-reset-at".into(), ((now + 5.0 * 24.0 * 3_600_000.0) / 1000.0).floor().to_string()),
        ])
    }
    fn header_cache_key(row: &StoredAuthCredential, base_url: Option<&str>) -> String {
        let credential = usage_credential(row, None);
        usage_report_key(&UsageRequest {
            provider: row.provider.clone(),
            account_key: usage_identity(&credential),
            credential,
            credential_id: Some(row.id),
            base_url: base_url.map(str::to_owned),
        })
    }
    fn header_cache(storage: &AuthStorage, key: &str) -> UsageCacheEntry<UsageReport> {
        storage
            .store_operation(|store, _state| {
                let raw = store.get_cache(&format!("{USAGE_CACHE_PREFIX}{key}"), true).unwrap().unwrap();
                Ok(serde_json::from_str(&raw).unwrap())
            })
            .unwrap()
    }
    fn oauth(email: &str, now: f64, expired: bool) -> AuthCredential {
        AuthCredential::oauth(
            serde_json::json!({"access":format!("access-{email}"),"refresh":format!("refresh-{email}"),
            "expires": if expired { now - 1.0 } else { now + 7_200_000.0 }, "email":email,"accountId":email})
            .as_object()
            .unwrap()
            .clone(),
        )
    }
    fn setup(
        provider: &str,
        credentials: &[AuthCredential],
        now: Arc<AtomicU64>,
        lower_keys: bool,
    ) -> (AuthStorage, Arc<Mutex<SqliteCredentialStore>>, Arc<FixtureConfig>, Arc<FixtureOAuth>) {
        let store = Arc::new(Mutex::new(
            SqliteCredentialStore::from_connection(
                rusqlite::Connection::open_in_memory().unwrap(),
                Duration::from_millis(20),
            )
            .unwrap(),
        ));
        store.lock().unwrap().replace_auth_credentials_for_provider(provider, credentials).unwrap();
        let config = Arc::new(FixtureConfig::default());
        let clock = now.clone();
        let options = AuthStorageOptions {
            config_key_resolver: config.clone(),
            clock: Arc::new(move || clock.load(Ordering::Acquire) as f64),
            jitter: Arc::new(|| 0.5),
            usage_request_timeout: Duration::from_secs(2),
            environment: if lower_keys { Arc::new(FixtureEnvironment) } else { Arc::new(EmptyEnvironment) },
            fallback: if lower_keys { Arc::new(FixtureFallback) } else { Arc::new(EmptyFallback) },
        };
        let storage = AuthStorage::new(store.clone(), None, options).unwrap();
        let oauth = Arc::new(FixtureOAuth::new(store.clone()));
        storage.register_oauth_provider(provider, oauth.clone()).unwrap();
        (storage, store, config, oauth)
    }
    fn report(provider: &str, now: f64, fraction: f64, plan: &str, email: &str) -> UsageReport {
        serde_json::from_value(serde_json::json!({"provider":provider,"fetchedAt":now,
            "metadata":{"planType":plan,"allowed":true,"limitReached":false,"email":email,"accountId":email},
            "limits":[{"id":"openai-codex:primary","label":"Primary","scope":{"provider":provider,"accountId":email},
                "window":{"id":"5h","label":"5 hours","durationMs":18_000_000,"resetsAt":now+18_000_000.0},
                "amount":{"usedFraction":fraction,"unit":"percent"}}]}))
        .unwrap()
    }
    fn id(lease: &RequestAuthLease) -> i64 {
        match lease.identity() {
            CredentialIdentity::Stored { id, .. } => *id,
            _ => panic!("expected stored identity"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_usage_header_native_selection_and_guard_case_corpus() {
        // Fixed auth-storage-codex-selection.test.ts:2426-2473, extended with
        // mixed native indexes, partial windows and the active-account guards.
        let provider = "openai-codex";
        let at = chrono::Utc::now().timestamp_millis() as f64;
        let clock = Arc::new(AtomicU64::new(at as u64));
        let healthy = codex_headers(at, "20", "30");
        let builtin = crate::codex_usage::CodexUsageProvider::new().unwrap();
        assert_eq!(
            UsageProvider::rate_limit_header_parser(&builtin).unwrap()(&healthy, at),
            crate::codex_usage::parse_codex_rate_limit_headers(&healthy, at)
        );
        let (storage, store, _, _) = setup(
            provider,
            &[AuthCredential::api_key("static"), oauth("hdr-a@fixture", at, false), oauth("hdr-b@fixture", at, false)],
            clock.clone(),
            false,
        );
        let rows = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        assert!(!storage.ingest_usage_headers(provider, &healthy, Some("headers"), None).unwrap());
        let usage = Arc::new(FixtureUsage::new());
        storage.register_usage_provider(provider, usage.clone()).unwrap();
        assert!(!storage.ingest_usage_headers(provider, &healthy, Some("headers"), None).unwrap());
        storage
            .register_usage_provider(
                provider,
                Arc::new(FixtureHeaderUsage {
                    usage: usage.clone(),
                    parser: crate::codex_usage::parse_codex_rate_limit_headers,
                }),
            )
            .unwrap();
        for row in &rows[1..] {
            let email = text_field(row, "email").unwrap();
            let mut full = report(provider, at, 0.2, "plus", &email);
            full.limits = crate::codex_usage::parse_codex_rate_limit_headers(&healthy, at).unwrap().limits;
            let mut spark = full.limits[0].clone();
            spark.id = "openai-codex:spark:primary".into();
            spark.scope.model_id = Some("gpt-5.3-codex-spark".into());
            spark.unknown_fields.insert("nativeExtra".into(), Value::Bool(true));
            full.limits.push(spark);
            full.raw = Some(serde_json::json!({"extra_usage":{"used":12.34}}));
            full.unknown_fields.insert("nativeReportField".into(), Value::String("retained".into()));
            full.metadata.as_mut().unwrap().insert("source".into(), Value::String("full-endpoint".into()));
            usage.reports.lock().unwrap().insert(row.id, full);
        }
        let context = AuthRequestContext {
            session_id: Some("headers".into()),
            model_id: Some("gpt-5.3-codex".into()),
            ..Default::default()
        };
        let cancel = CancellationToken::new();
        let selected = storage.resolve(provider, &context, &cancel).await.unwrap().unwrap();
        storage.wait_for_settlement().await;
        let selected_row = rows.iter().find(|row| row.id == id(&selected.lease)).unwrap();
        let sibling = rows[1..].iter().find(|row| row.id != selected_row.id).unwrap();
        let key = header_cache_key(selected_row, None);
        let original = header_cache(&storage, &key);
        let history = store.lock().unwrap().list_usage_history(None).unwrap().len();
        assert!(storage.ingest_usage_headers(provider, &healthy, Some("headers"), None).unwrap());
        assert!(!storage.ingest_usage_headers(provider, &healthy, Some("headers"), None).unwrap());
        clock.fetch_add(60_000, Ordering::AcqRel);
        let mut partial = codex_headers(at, "40", "30");
        partial.retain(|name, _| !name.starts_with("x-codex-secondary-"));
        assert!(storage.ingest_usage_headers(provider, &partial, Some("headers"), None).unwrap());
        let merged = header_cache(&storage, &key);
        let merged_report = merged.value.unwrap();
        assert_eq!(merged.expires_at, original.expires_at);
        assert_eq!(merged_report.fetched_at, at + 60_000.0);
        assert_eq!(merged_report.limits.len(), 3);
        assert_eq!(merged_report.limits[0].amount.used_fraction, Some(0.4));
        assert_eq!(merged_report.limits[1].amount.used_fraction, Some(0.3));
        assert_eq!(merged_report.limits[2], original.value.as_ref().unwrap().limits[2]);
        assert_eq!(merged_report.raw, original.value.as_ref().unwrap().raw);
        assert_eq!(merged_report.unknown_fields, original.value.as_ref().unwrap().unknown_fields);
        assert_eq!(merged_report.metadata.as_ref().unwrap()["source"], "full-endpoint");
        assert_eq!(merged_report.metadata.as_ref().unwrap()["headersUpdatedAt"], Value::from(at + 60_000.0));
        assert_eq!(store.lock().unwrap().list_usage_history(None).unwrap().len(), history);
        let exhausted = codex_headers(at, "20", "100");
        assert!(storage.ingest_usage_headers(provider, &exhausted, Some("headers"), None).unwrap());
        assert_eq!(header_cache(&storage, &key).expires_at, original.expires_at);
        assert!(
            storage.list_credential_blocks(&[selected_row.id]).unwrap().is_empty(),
            "header ingestion leaves exhaustion blocking to the next selector"
        );
        let rotated = storage.resolve(provider, &context, &cancel).await.unwrap().unwrap();
        assert_eq!(id(&rotated.lease), sibling.id);
        assert!(storage.ingest_usage_headers(provider, &healthy, Some("headers"), None).unwrap());
        assert_eq!(
            header_cache(&storage, &header_cache_key(sibling, None)).value.unwrap().metadata.unwrap()["accountId"],
            text_field(sibling, "accountId").unwrap()
        );
        assert!(
            storage.ingest_usage_headers(provider, &healthy, Some("headers"), Some(" https://fixture.test/ ")).unwrap()
        );
        assert!(
            !storage.ingest_usage_headers(provider, &healthy, Some("headers"), Some("https://fixture.test")).unwrap()
        );
        for runtime in [true, false] {
            if runtime {
                storage.set_runtime_api_key(provider, "runtime".into()).unwrap();
            } else {
                storage.set_config_api_key(provider, "config".into()).unwrap();
            }
            assert!(!storage.ingest_usage_headers(provider, &exhausted, Some("headers"), None).unwrap());
            if runtime {
                storage.remove_runtime_api_key(provider).unwrap();
            } else {
                storage.remove_config_api_key(provider).unwrap();
            }
        }
        storage
            .store_operation(|store, state| {
                state.assignments.record_session_credential(
                    store,
                    provider,
                    Some("key-sticky"),
                    CredentialKind::ApiKey,
                    0,
                    &rows,
                    None,
                    at,
                );
                Ok(())
            })
            .unwrap();
        assert!(!storage.ingest_usage_headers(provider, &exhausted, Some("key-sticky"), None).unwrap());
        assert!(!storage.ingest_usage_headers(provider, &BTreeMap::new(), Some("headers"), None).unwrap());
        let (lower, lower_store, _, _) = setup(provider, &[oauth("lower@fixture", at, false)], clock.clone(), true);
        lower
            .register_usage_provider(
                provider,
                Arc::new(FixtureHeaderUsage {
                    usage: usage.clone(),
                    parser: crate::codex_usage::parse_codex_rate_limit_headers,
                }),
            )
            .unwrap();
        assert!(!lower.ingest_usage_headers(provider, &healthy, Some("lower"), None).unwrap());
        let lower_row = lower_store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap().remove(0);
        assert!(lower.pin_session_oauth_account(provider, "lower", lower_row.id, None).unwrap());
        // The broker block mutator invalidates usage caches. Keep this cold
        // no-healing fixture separate from the full-backed rotation above.
        lower
            .upsert_credential_block(&StoredCredentialBlock {
                credential_id: lower_row.id,
                provider_key: provider_type_key(provider, CredentialKind::OAuth),
                block_scope: "spark".into(),
                blocked_until_ms: at as i64 + 7_200_000,
                updated_at_ms: at as i64,
            })
            .unwrap();
        let usage_calls = usage.calls.load(Ordering::Acquire);
        assert!(lower.ingest_usage_headers(provider, &healthy, Some("lower"), None).unwrap());
        assert!(
            lower.list_credential_blocks(&[lower_row.id]).unwrap().iter().any(|block| block.block_scope == "spark")
        );
        assert!(lower.ingest_usage_headers(provider, &exhausted, Some("lower"), None).unwrap());
        assert!(
            lower.list_credential_blocks(&[lower_row.id]).unwrap().iter().any(|block| block.block_scope == "spark")
        );
        assert_eq!(
            usage.calls.load(Ordering::Acquire),
            usage_calls,
            "header ingestion never starts a full usage probe"
        );
        let no_oauth = setup(provider, &[AuthCredential::api_key("only-key")], clock.clone(), false).0;
        no_oauth
            .register_usage_provider(
                provider,
                Arc::new(FixtureHeaderUsage { usage, parser: crate::codex_usage::parse_codex_rate_limit_headers }),
            )
            .unwrap();
        assert!(!no_oauth.ingest_usage_headers(provider, &healthy, None, None).unwrap());
        storage.wait_for_settlement().await;
    }

    #[tokio::test]
    async fn host_usage_header_expiry_and_metadata_case_corpus() {
        let provider = "openai-codex";
        let at = chrono::Utc::now().timestamp_millis() as f64;
        let clock = Arc::new(AtomicU64::new(at as u64));
        for (name, expiry, kind, source) in [
            ("fresh", at + 300_000.0, "report", Some(Value::String("full-endpoint".into()))),
            ("missing-source", at + 300_000.0, "report", None),
            ("null-source", at + 300_000.0, "report", Some(Value::Null)),
            ("expired", at - 1_000.0, "report", None),
            ("cooldown", at + 10_000.0, "null", None),
            ("cold", at - 1.0, "absent", None),
            ("invalid", at - 1.0, "invalid", None),
        ] {
            clock.store(at as u64, Ordering::Release);
            let email = format!("{name}@fixture");
            let (storage, store, _, _) = setup(provider, &[oauth(&email, at, false)], clock.clone(), false);
            let row = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap().remove(0);
            let key = header_cache_key(&row, None);
            let usage = Arc::new(FixtureUsage::new());
            // Installing a usage implementation invalidates older snapshots
            // (fixed source 1536 / 6248-6264), so seed prior cache afterwards.
            storage
                .register_usage_provider(
                    provider,
                    Arc::new(FixtureHeaderUsage { usage: usage.clone(), parser: codex_headers_with_null_email }),
                )
                .unwrap();
            let mut full = report(provider, at, 0.1, "plus", &email);
            if let Some(source) = source.clone() {
                full.metadata.as_mut().unwrap().insert("source".into(), source);
            }
            full.metadata.as_mut().unwrap().insert("accountId".into(), Value::Null);
            match kind {
                "report" | "null" => storage
                    .store_operation(|store, _state| {
                        write_usage_cache(
                            store,
                            &key,
                            &UsageCacheEntry { value: (kind == "report").then(|| full.clone()), expires_at: expiry },
                            at,
                        )
                        .map_err(|_| AuthStorageError::Storage)
                    })
                    .unwrap(),
                "invalid" => store
                    .lock()
                    .unwrap()
                    .set_cache(
                        &format!("{USAGE_CACHE_PREFIX}{key}"),
                        "invalid JSON",
                        ((at + LAST_GOOD_RETENTION_MS) / 1000.0).floor() as i64,
                    )
                    .unwrap(),
                _ => (),
            }
            assert!(
                storage.ingest_usage_headers(provider, &codex_headers(at, "25", "30"), None, None).unwrap(),
                "{name}"
            );
            let cached = header_cache(&storage, &key);
            assert_eq!(cached.expires_at, expiry.max(at - 1.0), "{name}");
            let cached_report = cached.value.unwrap();
            assert_eq!(
                cached_report.limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(),
                ["openai-codex:primary", "openai-codex:secondary"],
                "{name} appends new header windows"
            );
            let metadata = cached_report.metadata.unwrap();
            if kind == "report" {
                assert_eq!(metadata.get("source"), source.as_ref(), "{name}");
                assert_eq!(metadata["accountId"], Value::Null, "defined prior null must win");
            } else {
                assert_eq!(metadata["source"], "ratelimit-headers");
                assert_eq!(metadata["email"], Value::Null, "defined parser null must survive enrichment");
            }
            assert!(
                store.lock().unwrap().get_cache(&format!("{USAGE_CACHE_PREFIX}{key}"), false).unwrap().is_some(),
                "stale header reports retain a durable last-good entry"
            );
            assert!(store.lock().unwrap().list_usage_history(None).unwrap().is_empty());
            usage.reports.lock().unwrap().insert(row.id, report(provider, at, 0.1, "plus", &email));
            if cached.expires_at <= at {
                assert!(
                    storage
                        .usage_report(provider, &row, &AuthRequestContext::default(), false, &CancellationToken::new())
                        .await
                        .unwrap()
                        .is_some()
                );
                assert_eq!(usage.calls.load(Ordering::Acquire), 1, "{name} probes a full report");
            } else {
                assert!(
                    storage
                        .usage_report(provider, &row, &AuthRequestContext::default(), false, &CancellationToken::new())
                        .await
                        .unwrap()
                        .is_some()
                );
                assert_eq!(usage.calls.load(Ordering::Acquire), 0, "{name} retains its original deadline");
                clock.store((expiry + 1.0) as u64, Ordering::Release);
                assert!(
                    storage
                        .usage_report(provider, &row, &AuthRequestContext::default(), false, &CancellationToken::new())
                        .await
                        .unwrap()
                        .is_some()
                );
                assert_eq!(usage.calls.load(Ordering::Acquire), 1, "{name} refetches after its original deadline");
            }
            storage.wait_for_settlement().await;
        }
    }

    #[test]
    fn host_usage_header_store_failure_case_corpus() {
        let provider = "openai-codex";
        let at = chrono::Utc::now().timestamp_millis() as f64;
        struct HeaderExternalInputs {
            reads: AtomicUsize,
        }
        impl Environment for HeaderExternalInputs {
            fn api_key(&self, _: &str) -> Option<EnvironmentKey> {
                self.reads.fetch_add(1, Ordering::AcqRel);
                None
            }
        }
        impl Fallback for HeaderExternalInputs {
            fn api_key(&self, _: &str) -> Option<String> {
                self.reads.fetch_add(1, Ordering::AcqRel);
                None
            }
        }
        for fault in ["closed", "account-read"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("no-parser.sqlite");
            let store = Arc::new(Mutex::new(SqliteCredentialStore::open(&path).unwrap()));
            store
                .lock()
                .unwrap()
                .replace_auth_credentials_for_provider(provider, &[oauth("no-parser@fixture", at, false)])
                .unwrap();
            let inputs = Arc::new(HeaderExternalInputs { reads: AtomicUsize::new(0) });
            let clock_reads = Arc::new(AtomicUsize::new(0));
            let clock = clock_reads.clone();
            let storage = AuthStorage::new(
                store.clone(),
                None,
                AuthStorageOptions {
                    environment: inputs.clone(),
                    fallback: inputs.clone(),
                    clock: Arc::new(move || {
                        clock.fetch_add(1, Ordering::AcqRel);
                        at
                    }),
                    ..Default::default()
                },
            )
            .unwrap();
            storage.register_usage_provider(provider, Arc::new(FixtureUsage::new())).unwrap();
            let peer = rusqlite::Connection::open(&path).unwrap();
            if fault == "closed" {
                store.lock().unwrap().close().unwrap();
            } else {
                peer.execute_batch("ALTER TABLE auth_credentials RENAME TO unavailable_auth_credentials").unwrap();
            }
            assert!(
                store.lock().unwrap().list_auth_credentials(Some(provider)).is_err(),
                "{fault} fixture must reject an account read"
            );
            let prior_clock_reads = clock_reads.load(Ordering::Acquire);
            for no_parser_provider in ["without-usage-hook", provider] {
                assert_eq!(
                    storage.ingest_usage_headers(no_parser_provider, &codex_headers(at, "20", "30"), None, None),
                    Ok(false),
                    "{fault}/{no_parser_provider}"
                );
            }
            assert_eq!(inputs.reads.load(Ordering::Acquire), 0, "{fault} must skip environment and fallback");
            assert_eq!(clock_reads.load(Ordering::Acquire), prior_clock_reads, "{fault} must skip clock");
            assert!(storage.inner.state.lock().unwrap().usage_header_ingest_at.is_empty());
            drop(peer);
            store.lock().unwrap().close().unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("headers.sqlite");
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(&path).unwrap()));
        store
            .lock()
            .unwrap()
            .replace_auth_credentials_for_provider(provider, &[oauth("read-error@fixture", at, false)])
            .unwrap();
        let storage = AuthStorage::new(
            store.clone(),
            None,
            AuthStorageOptions { clock: Arc::new(move || at), ..Default::default() },
        )
        .unwrap();
        storage
            .register_usage_provider(
                provider,
                Arc::new(FixtureHeaderUsage {
                    usage: Arc::new(FixtureUsage::new()),
                    parser: crate::codex_usage::parse_codex_rate_limit_headers,
                }),
            )
            .unwrap();
        let row = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap().remove(0);
        let key = header_cache_key(&row, None);
        let durable_key = format!("{USAGE_CACHE_PREFIX}{key}");
        store
            .lock()
            .unwrap()
            .set_cache(&durable_key, "prior payload", ((at + LAST_GOOD_RETENTION_MS) / 1000.0) as i64)
            .unwrap();
        let peer = rusqlite::Connection::open(&path).unwrap();
        peer.execute_batch("ALTER TABLE cache RENAME TO unavailable_cache").unwrap();
        assert_eq!(
            storage.ingest_usage_headers(provider, &codex_headers(at, "20", "30"), None, None),
            Err(AuthStorageError::Storage)
        );
        assert!(storage.inner.state.lock().unwrap().usage_header_ingest_at.is_empty());
        let retained: String = peer
            .query_row("SELECT value FROM unavailable_cache WHERE key=?1", [&durable_key], |row| row.get(0))
            .unwrap();
        assert_eq!(retained, "prior payload");
        peer.execute_batch("ALTER TABLE unavailable_cache RENAME TO cache;
            CREATE TRIGGER reject_header_write BEFORE INSERT ON cache BEGIN SELECT RAISE(FAIL, 'fixture write failure'); END;").unwrap();
        assert_eq!(
            storage.ingest_usage_headers(provider, &codex_headers(at, "20", "30"), None, None),
            Err(AuthStorageError::Storage)
        );
        assert!(storage.inner.state.lock().unwrap().usage_header_ingest_at.is_empty());
        assert_eq!(store.lock().unwrap().get_cache(&durable_key, true).unwrap().as_deref(), Some("prior payload"));
        peer.execute_batch("DROP TRIGGER reject_header_write").unwrap();
        assert!(storage.ingest_usage_headers(provider, &codex_headers(at, "20", "30"), None, None).unwrap());
        assert!(!storage.ingest_usage_headers(provider, &codex_headers(at, "20", "30"), None, None).unwrap());
        drop(peer);
        store.lock().unwrap().close().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_auth_storage_native_flow_case_corpus() {
        let provider = "openai-codex";
        let at = chrono::Utc::now().timestamp_millis() as f64;
        let clock = Arc::new(AtomicU64::new(at as u64));
        let cancel = CancellationToken::new();
        let context = AuthRequestContext {
            session_id: Some("session".into()),
            model_id: Some("gpt-5.3-codex".into()),
            ..Default::default()
        };
        let credentials = [
            AuthCredential::api_key("static"),
            AuthCredential::ApiKey { key: "!login".into(), source: Some("login".into()) },
            oauth("one@fixture", at, false),
            oauth("two@fixture", at, false),
        ];
        let (storage, store, config, hook) = setup(provider, &credentials, clock.clone(), true);
        let rows = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        assert_eq!(rows.len(), 4);
        storage.set_config_api_key(provider, "configuration".into()).unwrap();
        storage.set_runtime_api_key(provider, "runtime".into()).unwrap();
        for (expected, source) in [("runtime", CredentialSource::Runtime), ("configuration", CredentialSource::Config)]
        {
            let result = storage.resolve(provider, &context, &cancel).await.unwrap().unwrap();
            assert_eq!(result.lease.api_key(), Some(expected));
            assert_eq!(result.source, source);
            assert!(result.oauth.is_none());
            assert!(storage.get_oauth_access(provider, &context, &cancel).await.unwrap().is_none());
            assert!(storage.list_oauth_accounts(provider, Some("session")).unwrap().is_empty());
            assert!(!storage.pin_session_oauth_account(provider, "session", rows[2].id, None).unwrap());
            assert!(storage.get_oauth_account_id(provider, Some("session")).unwrap().is_none());
            storage.remove_runtime_api_key(provider).unwrap();
        }
        storage.clear_config_api_keys().unwrap();
        assert!(storage.pin_session_oauth_account(provider, "session", rows[2].id, Some(at - 7_200_000.0)).unwrap());
        let selected = storage.resolve(provider, &context, &cancel).await.unwrap().unwrap();
        assert_eq!(selected.source, CredentialSource::OAuth);
        assert_eq!(id(&selected.lease), rows[2].id);
        assert!(hook.max_parallel.load(Ordering::Acquire) >= 2);
        assert!(hook.refreshes.lock().unwrap().is_empty());
        assert_eq!(storage.peek_api_key(provider, Some("session")).unwrap().as_deref(), Some("access-one@fixture"));
        storage.unregister_oauth_provider(provider).unwrap();
        let login = storage.resolve(provider, &context, &cancel).await.unwrap().unwrap();
        assert_eq!(login.source, CredentialSource::LoginKey);
        assert_eq!(login.lease.api_key(), Some("login-resolved"));
        assert!(login.oauth.is_none());
        assert!(storage.get_oauth_account_id(provider, Some("session")).unwrap().is_none());
        let calls = config.calls.load(Ordering::Acquire);
        storage.peek_api_key(provider, Some("session")).unwrap();
        assert_eq!(config.calls.load(Ordering::Acquire), calls);
        store.lock().unwrap().delete_auth_credential(rows[1].id, "fixture remove login").unwrap();
        assert_eq!(
            storage.resolve(provider, &context, &cancel).await.unwrap().unwrap().source,
            CredentialSource::Environment
        );
        let static_only = setup("static-provider", &[AuthCredential::api_key("static")], clock.clone(), false).0;
        assert_eq!(
            static_only.resolve("static-provider", &context, &cancel).await.unwrap().unwrap().source,
            CredentialSource::StaticKey
        );
        let fallback_only = setup("fallback-provider", &[], clock.clone(), false).0;
        assert!(fallback_only.resolve("fallback-provider", &context, &cancel).await.unwrap().is_none());
        let options = AuthStorageOptions { fallback: Arc::new(FixtureFallback), ..Default::default() };
        let fallback = AuthStorage::new(store.clone(), None, options).unwrap();
        assert_eq!(
            fallback.resolve("unknown-provider", &context, &cancel).await.unwrap().unwrap().source,
            CredentialSource::Fallback
        );
        assert!(storage.has_auth(provider).unwrap());

        // Discovery keeps full provider indexes when it filters credential
        // types, advances native RR, skips global blocks and checks expiry
        // only on the selected OAuth row. Offline expressions never dispatch.
        let (peek, peek_store, _, _) = setup(
            "peek-provider",
            &[
                AuthCredential::api_key("static"),
                oauth("expired@fixture", at, true),
                AuthCredential::api_key("static-two"),
                oauth("fresh@fixture", at, false),
            ],
            clock.clone(),
            false,
        );
        let peek_rows = peek_store.lock().unwrap().list_auth_credentials(Some("peek-provider")).unwrap();
        let peek_type = provider_type_key("peek-provider", CredentialKind::OAuth);
        peek.upsert_credential_block(&StoredCredentialBlock {
            credential_id: peek_rows[1].id,
            provider_key: peek_type.clone(),
            block_scope: "".into(),
            blocked_until_ms: at as i64 + 60_000,
            updated_at_ms: at as i64,
        })
        .unwrap();
        for _ in 0..2 {
            assert_eq!(peek.peek_api_key("peek-provider", None).unwrap().as_deref(), Some("access-fresh@fixture"));
        }
        peek.delete_credential_block(peek_rows[1].id, &peek_type, "").unwrap();
        assert_eq!(peek.peek_api_key("peek-provider", None).unwrap().as_deref(), Some("static"));
        assert_eq!(peek.peek_api_key("peek-provider", None).unwrap().as_deref(), Some("access-fresh@fixture"));
        assert!(peek.pin_session_oauth_account("peek-provider", "pinned", peek_rows[3].id, None).unwrap());
        assert_eq!(
            peek.peek_api_key("peek-provider", Some("pinned")).unwrap().as_deref(),
            Some("access-fresh@fixture")
        );
        let (keys, key_store, _, _) = setup(
            "peek-keys",
            &[
                oauth("expired@fixture", at, true),
                AuthCredential::ApiKey { key: "login-one".into(), source: Some("login".into()) },
                AuthCredential::api_key("static"),
                AuthCredential::ApiKey { key: "login-two".into(), source: Some("login".into()) },
            ],
            clock.clone(),
            false,
        );
        let key_rows = key_store.lock().unwrap().list_auth_credentials(Some("peek-keys")).unwrap();
        let key_type = provider_type_key("peek-keys", CredentialKind::ApiKey);
        keys.upsert_credential_block(&StoredCredentialBlock {
            credential_id: key_rows[1].id,
            provider_key: key_type.clone(),
            block_scope: "".into(),
            blocked_until_ms: at as i64 + 60_000,
            updated_at_ms: at as i64,
        })
        .unwrap();
        for _ in 0..2 {
            assert_eq!(keys.peek_api_key("peek-keys", None).unwrap().as_deref(), Some("login-two"));
        }
        keys.delete_credential_block(key_rows[1].id, &key_type, "").unwrap();
        assert_eq!(keys.peek_api_key("peek-keys", None).unwrap().as_deref(), Some("login-one"));
        let rr = setup(
            "peek-rr",
            &[AuthCredential::api_key("first"), AuthCredential::api_key("second")],
            clock.clone(),
            false,
        )
        .0;
        assert_eq!(rr.peek_api_key("peek-rr", None).unwrap().as_deref(), Some("first"));
        assert_eq!(rr.peek_api_key("peek-rr", None).unwrap().as_deref(), Some("second"));
        let (cold, _, cold_config, _) = setup(
            "cold-command",
            &[
                AuthCredential::ApiKey { key: "!login".into(), source: Some("login".into()) },
                AuthCredential::api_key("$FIXTURE_KEY"),
            ],
            clock.clone(),
            false,
        );
        for _ in 0..2 {
            assert!(cold.peek_api_key("cold-command", None).unwrap().is_none());
        }
        assert_eq!(cold_config.calls.load(Ordering::Acquire), 0);

        // External-origin leases cannot claim a stored row by bearer equality.
        let (owned, owned_store, owned_config, _) =
            setup(provider, &[oauth("one@fixture", at, false)], clock.clone(), false);
        let owned_id = owned_store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap()[0].id;
        for identity in [
            CredentialIdentity::Runtime,
            CredentialIdentity::Config { provider: provider.into() },
            CredentialIdentity::Environment { variable: "FIXTURE_KEY".into() },
            CredentialIdentity::Keyless,
        ] {
            let outside = RequestAuthLease::new(identity, Some("access-one@fixture".into()));
            assert_eq!(
                owned.mark_usage_limit_reached(provider, &context, &outside, None, &cancel).await.unwrap(),
                UsageLimitMarkResult::default()
            );
            assert!(owned.refresh_failed_lease(provider, &context, &outside, &cancel).await.unwrap().is_none());
            assert!(!owned.invalidate_credential_matching(provider, &context, &outside, true, &cancel).await.unwrap());
        }
        assert!(owned.list_credential_blocks(&[owned_id]).unwrap().is_empty());
        assert_eq!(owned.stored_oauth_snapshot(provider).unwrap().len(), 1);
        assert_eq!(owned_config.calls.load(Ordering::Acquire), 0);

        // A peer changes the failed row: exact refresh uses that new bearer,
        // while a historical bearer never becomes hard-auth invalidation.
        let (exact, exact_store, _, exact_hook) =
            setup(provider, &[oauth("one@fixture", at, false), oauth("two@fixture", at, false)], clock.clone(), false);
        let exact_rows = exact_store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        exact.pin_session_oauth_account(provider, "session", exact_rows[1].id, None).unwrap();
        let failed = exact.resolve(provider, &context, &cancel).await.unwrap().unwrap().lease;
        let mut peer = exact_rows[1].credential.clone();
        if let AuthCredential::OAuth { fields } = &mut peer {
            fields.insert("access".into(), Value::String("peer-fresh".into()));
        }
        exact_store.lock().unwrap().update_auth_credential(exact_rows[1].id, &peer).unwrap();
        let replacement = exact.refresh_failed_lease(provider, &context, &failed, &cancel).await.unwrap().unwrap();
        assert_eq!(id(&replacement.lease), exact_rows[1].id);
        assert_eq!(replacement.lease.api_key(), Some("peer-fresh"));
        assert!(exact_hook.refreshes.lock().unwrap().is_empty());
        assert!(!exact.invalidate_credential_matching(provider, &context, &failed, true, &cancel).await.unwrap());
        assert_eq!(exact.stored_oauth_snapshot(provider).unwrap().len(), 2);
        exact_store.lock().unwrap().delete_auth_credential(exact_rows[0].id, "fixture reorder").unwrap();
        exact.set_runtime_api_key(provider, "new-override".into()).unwrap();
        let quota = exact.mark_usage_limit_reached(provider, &context, &failed, Some(90_000.0), &cancel).await.unwrap();
        exact.remove_runtime_api_key(provider).unwrap();
        assert!(!quota.switched);
        let blocks = exact.list_credential_blocks(&[exact_rows[1].id]).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].credential_id, exact_rows[1].id);
        assert_eq!(blocks[0].block_scope, "chat");

        // Preflight failures run once, and the unfiltered third pass still
        // permits a working free row after the sole eligible paid row dies.
        let (ladder, ladder_store, _, ladder_hook) =
            setup(provider, &[oauth("paid@fixture", at, true), oauth("free@fixture", at, false)], clock.clone(), false);
        let ladder_rows = ladder_store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        ladder_hook.failures.lock().unwrap().insert(ladder_rows[0].id, AuthStorageError::Definitive);
        let usage = Arc::new(FixtureUsage::new());
        usage.reports.lock().unwrap().insert(ladder_rows[0].id, report(provider, at, 0.1, "plus", "paid@fixture"));
        usage.reports.lock().unwrap().insert(ladder_rows[1].id, report(provider, at, 0.1, "free", "free@fixture"));
        ladder.register_usage_provider(provider, usage).unwrap();
        let plan_context = AuthRequestContext { model_id: Some("gpt-5.6-sol".into()), ..context.clone() };
        assert_eq!(
            policy::resolve_codex_plan_requirement(provider, plan_context.model_id.as_deref()),
            PlanRequirement::Paid
        );
        let last_resort = ladder.resolve(provider, &plan_context, &cancel).await.unwrap().unwrap();
        assert_eq!(id(&last_resort.lease), ladder_rows[1].id);
        assert_eq!(*ladder_hook.refreshes.lock().unwrap(), vec![ladder_rows[0].id]);
        assert_eq!(ladder.stored_oauth_snapshot(provider).unwrap().len(), 1);

        // Fully blocked candidates still return wire auth on the second pass.
        let (blocked, blocked_store, _, _) =
            setup(provider, &[oauth("one@fixture", at, false), oauth("two@fixture", at, false)], clock.clone(), false);
        let blocked_rows = blocked_store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        for (index, row) in blocked_rows.iter().enumerate() {
            blocked
                .upsert_credential_block(&StoredCredentialBlock {
                    credential_id: row.id,
                    provider_key: provider_type_key(provider, CredentialKind::OAuth),
                    block_scope: "chat".into(),
                    blocked_until_ms: at as i64 + 60_000 + index as i64 * 60_000,
                    updated_at_ms: at as i64,
                })
                .unwrap();
        }
        let wire = blocked.resolve(provider, &context, &cancel).await.unwrap().unwrap();
        assert_eq!(id(&wire.lease), blocked_rows[0].id);
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(blocked.resolve(provider, &context, &cancelled).await, Err(AuthStorageError::Cancelled)));
        assert!(blocked.session_resolver(Some("one".into()), None).for_session("two").is_some());
        blocked.wait_for_settlement().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_usage_cache_and_scope_case_corpus() {
        let provider = "openai-codex";
        let at = chrono::Utc::now().timestamp_millis() as f64;
        let clock = Arc::new(AtomicU64::new(at as u64));
        let context = AuthRequestContext::default();
        let cancel = CancellationToken::new();
        let (storage, store, _, _) = setup(provider, &[oauth("one@fixture", at, false)], clock.clone(), false);
        let row = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap().remove(0);
        let usage = Arc::new(FixtureUsage::new());
        storage.register_usage_provider(provider, usage.clone()).unwrap();
        let good = report(provider, at, 0.2, "plus", "one@fixture");
        let gate = Arc::new(Semaphore::new(0));
        usage.push(Some(gate.clone()), Ok(Some(good.clone())));
        let first_storage = storage.clone();
        let first_row = row.clone();
        let first = tokio::spawn(async move {
            first_storage
                .usage_report(provider, &first_row, &AuthRequestContext::default(), false, &CancellationToken::new())
                .await
        });
        usage.wait_started().await;
        let second_storage = storage.clone();
        let second_row = row.clone();
        let second_cancel = CancellationToken::new();
        let second_cancel_task = second_cancel.clone();
        let second = tokio::spawn(async move {
            second_storage
                .usage_report(provider, &second_row, &AuthRequestContext::default(), false, &second_cancel_task)
                .await
        });
        second_cancel.cancel();
        assert!(matches!(second.await.unwrap(), Err(AuthStorageError::Cancelled)));
        gate.add_permits(1);
        assert!(first.await.unwrap().unwrap().is_some());
        storage.wait_for_settlement().await;
        assert_eq!(usage.calls.load(Ordering::Acquire), 1);
        assert!(storage.usage_report(provider, &row, &context, false, &cancel).await.unwrap().is_some());
        assert_eq!(usage.calls.load(Ordering::Acquire), 1);
        clock.store(at as u64 + USAGE_REPORT_TTL_MS as u64 + 1, Ordering::Release);
        usage.push(None, Err(UsageFetchError::Transient));
        assert!(storage.usage_report(provider, &row, &context, false, &cancel).await.unwrap().is_some());
        storage.invalidate_usage_cache(Some(provider)).unwrap();
        usage.push(None, Err(UsageFetchError::Transient));
        assert!(storage.usage_report(provider, &row, &context, true, &cancel).await.unwrap().is_none());
        usage.push(None, Ok(Some(good.clone())));
        assert!(storage.usage_report(provider, &row, &context, true, &cancel).await.unwrap().is_some());
        clock.fetch_add(USAGE_REPORT_TTL_MS as u64 + 1, Ordering::AcqRel);
        usage.push(None, Err(UsageFetchError::Unauthorized));
        assert!(storage.usage_report(provider, &row, &context, false, &cancel).await.unwrap().is_none());

        // Old-epoch success may satisfy its consumer but cannot resurrect the
        // manually invalidated last-good snapshot or bypass the new flight.
        storage.invalidate_usage_cache(Some(provider)).unwrap();
        let old_gate = Arc::new(Semaphore::new(0));
        usage.push(Some(old_gate.clone()), Ok(Some(good.clone())));
        let old_signals = usage.started.available_permits();
        if old_signals > 0 {
            usage.started.try_acquire_many(old_signals as u32).unwrap().forget();
        }
        let old_storage = storage.clone();
        let old_row = row.clone();
        let old = tokio::spawn(async move {
            old_storage
                .usage_report(provider, &old_row, &AuthRequestContext::default(), false, &CancellationToken::new())
                .await
        });
        usage.wait_started().await;
        storage.invalidate_usage_cache(Some(provider)).unwrap();
        usage.push(None, Ok(None));
        assert!(storage.usage_report(provider, &row, &context, true, &cancel).await.unwrap().is_none());
        old_gate.add_permits(1);
        assert!(old.await.unwrap().unwrap().is_some());
        storage.wait_for_settlement().await;
        let request = UsageRequest {
            provider: provider.into(),
            credential: usage_credential(&row, None),
            credential_id: Some(row.id),
            base_url: None,
            account_key: usage_identity(&usage_credential(&row, None)),
        };
        assert!(
            storage
                .store_operation(|store, _state| Ok(read_usage_cache::<UsageReport>(
                    store,
                    &usage_report_key(&request),
                    true
                )
                .and_then(|entry| entry.value)))
                .unwrap()
                .is_none()
        );

        // Healing checks both identity dimensions and its complete scope set.
        let now = clock.load(Ordering::Acquire) as f64;
        for scope in ["chat", "spark"] {
            storage
                .upsert_credential_block(&StoredCredentialBlock {
                    credential_id: row.id,
                    provider_key: provider_type_key(provider, CredentialKind::OAuth),
                    block_scope: scope.into(),
                    blocked_until_ms: now as i64 + 7_200_000,
                    updated_at_ms: now as i64,
                })
                .unwrap();
        }
        storage.reconcile_usage_reports(std::slice::from_ref(&good)).await.unwrap();
        // The local persisted probe timestamp is based on real wall time;
        // return the fake clock to that fresh-block window before acceptance.
        clock.store(chrono::Utc::now().timestamp_millis() as u64, Ordering::Release);
        let missing_spark = report(provider, at, 0.2, "plus", "one@fixture");
        storage.reconcile_usage_reports(std::slice::from_ref(&missing_spark)).await.unwrap();
        assert!(storage.list_credential_blocks(&[row.id]).unwrap().iter().any(|block| block.block_scope == "spark"));
        clock.fetch_add(USAGE_REPORT_TTL_MS as u64 + 20, Ordering::AcqRel);
        let mut mismatched = missing_spark.clone();
        mismatched.metadata.as_mut().unwrap().insert("email".into(), Value::String("other@fixture".into()));
        storage.reconcile_usage_reports(&[mismatched]).await.unwrap();
        assert!(storage.list_credential_blocks(&[row.id]).unwrap().iter().any(|block| block.block_scope == "spark"));
        let mut all_healthy = missing_spark;
        all_healthy.metadata.as_mut().unwrap().insert("meterStates".into(), serde_json::json!({"chat":{"allowed":true,"limitReached":false},"spark":{"allowed":true,"limitReached":false}}));
        storage.reconcile_usage_reports(&[all_healthy]).await.unwrap();
        assert!(storage.list_credential_blocks(&[row.id]).unwrap().is_empty());
        storage.wait_for_settlement().await;
    }
}
