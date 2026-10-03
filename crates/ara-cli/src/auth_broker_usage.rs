//! HTTP usage capabilities of the fixed OMP auth-broker client/store at
//! `596f2da7101178214aa27a753529d15e6b7ad91d`:
//! `packages/ai/src/auth-broker/{client.ts,remote-store.ts,wire-schemas.ts}`.
//!
//! The Host supplies the broker endpoint, bearer, clock and a secret-free raw
//! credential snapshot. This adapter does not implement credential persistence,
//! snapshot transport/SSE, refresh, or the broker's credential write operations.
//
// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::auth_storage::{AggregateUsageSource, AuthStorageError, UsageRequest, UsageStoreHooks};
use crate::auth_storage_policy::{UsageCredential, UsageCredentialType, UsageLimit, UsageReport};
use async_trait::async_trait;
use reqwest::{Client, Method, Response};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

const USAGE_CACHE_TTL_MS: f64 = 15_000.0;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Missing providers are unrestricted; an empty set excludes that provider's
/// OAuth identities. Snapshot API keys remain visible, as in the native store.
pub type AuthBrokerAccountPool = BTreeMap<String, BTreeSet<String>>;

/// Identity-only projection: deliberately contains no access/refresh/API token.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AuthBrokerUsageIdentity {
    pub account_id: Option<String>,
    pub email: Option<String>,
    pub project_id: Option<String>,
    pub org_id: Option<String>,
}

impl From<&UsageCredential> for AuthBrokerUsageIdentity {
    fn from(credential: &UsageCredential) -> Self {
        Self {
            account_id: credential.account_id.clone(),
            email: credential.email.clone(),
            project_id: credential.project_id.clone(),
            org_id: credential.org_id.clone(),
        }
    }
}

/// The Host must supply the full raw snapshot, before account-pool filtering.
/// Durable IDs count each account once when sizing serialized broker probes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthBrokerUsageSnapshotEntry {
    pub id: i64,
    pub provider: String,
    pub credential_type: UsageCredentialType,
    pub identity_key: Option<String>,
    pub identity: AuthBrokerUsageIdentity,
}

pub struct AuthBrokerUsageOptions {
    pub client: Client,
    pub request_timeout: Duration,
    /// Unix milliseconds, matching native Date.now() and report.fetchedAt.
    pub clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub initial_snapshot: Vec<AuthBrokerUsageSnapshotEntry>,
    pub account_pool: Option<AuthBrokerAccountPool>,
}

impl Default for AuthBrokerUsageOptions {
    fn default() -> Self {
        Self {
            client: Client::new(),
            request_timeout: DEFAULT_TIMEOUT,
            clock: Arc::new(|| {
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1_000.0
            }),
            initial_snapshot: Vec::new(),
            account_pool: None,
        }
    }
}

/// Only this usage adapter is implemented; this is not a RemoteCredentialStore.
/// Bearer material is private and has neither Debug nor serialization support.
#[derive(Clone)]
pub struct AuthBrokerUsageStore {
    inner: Arc<BrokerUsageInner>,
}

struct BrokerUsageInner {
    base_url: String,
    bearer: String,
    client: Client,
    request_timeout: Duration,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    account_pool: Option<AuthBrokerAccountPool>,
    state: Mutex<BrokerUsageState>,
    pending: AtomicUsize,
    settled: Notify,
}

#[derive(Default)]
struct BrokerUsageState {
    epoch: u64,
    next_flight: u64,
    cache: Option<UsageCache>,
    flight: Option<UsageFlight>,
    snapshot: Vec<AuthBrokerUsageSnapshotEntry>,
    // Vec preserves native Map insertion order, including replacement in place.
    overlays: Vec<(String, UsageReport)>,
}

struct UsageCache {
    reports: Option<Vec<UsageReport>>,
    fetched_at: f64,
}

struct UsageFlight {
    id: u64,
    epoch: u64,
    receiver: watch::Receiver<FlightOutcome>,
}

#[derive(Clone)]
enum FlightOutcome {
    Pending,
    Ready { epoch: u64, reports: Option<Vec<UsageReport>> },
    Obsolete,
}

enum UsageLoad {
    Cached(Option<Vec<UsageReport>>),
    Flight(watch::Receiver<FlightOutcome>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageResponse {
    #[serde(rename = "generatedAt")]
    _generated_at: f64,
    reports: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UsageStaleResponse {
    // Native validates a boolean; it does not require true.
    #[serde(rename = "ok")]
    _ok: bool,
}

impl AuthBrokerUsageStore {
    pub fn new(
        base_url: impl Into<String>,
        bearer: impl Into<String>,
        options: AuthBrokerUsageOptions,
    ) -> Result<Self, AuthStorageError> {
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        reqwest::Url::parse(&base_url).map_err(|_| AuthStorageError::Configuration)?;
        Ok(Self {
            inner: Arc::new(BrokerUsageInner {
                base_url,
                bearer: bearer.into(),
                client: options.client,
                request_timeout: options.request_timeout,
                clock: options.clock,
                account_pool: options.account_pool,
                state: Mutex::new(BrokerUsageState {
                    snapshot: options.initial_snapshot,
                    ..BrokerUsageState::default()
                }),
                pending: AtomicUsize::new(0),
                settled: Notify::new(),
            }),
        })
    }

    /// Host snapshot updates replace only the identity/count view. The Host
    /// invokes invalidate_usage_cache separately for its native stale events.
    pub fn replace_snapshot(&self, raw_snapshot: Vec<AuthBrokerUsageSnapshotEntry>) {
        self.inner.state.lock().unwrap().snapshot = raw_snapshot;
    }

    /// Direct callers can cancel their wait without cancelling a shared GET.
    pub async fn fetch_usage_reports_with_cancel(
        &self,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        self.aggregate_reports(Some(cancel)).await
    }

    async fn aggregate_reports(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        let Some(reports) = self.load_reports(cancel).await? else {
            // Native aggregate does not promote overlays when broker data is null.
            return Ok(None);
        };
        let state = self.inner.state.lock().unwrap();
        let now = (self.inner.clock)();
        let reports = apply_overlays(reports, &state.overlays, now);
        Ok(Some(filter_reports(reports, &state.snapshot, self.inner.account_pool.as_ref())))
    }

    fn usage_load(&self) -> UsageLoad {
        let mut state = self.inner.state.lock().unwrap();
        let now = (self.inner.clock)();
        if let Some(cache) = &state.cache
            && now - cache.fetched_at < USAGE_CACHE_TTL_MS
        {
            return UsageLoad::Cached(cache.reports.clone());
        }
        if let Some(flight) = &state.flight {
            return UsageLoad::Flight(flight.receiver.clone());
        }
        state.next_flight = state.next_flight.wrapping_add(1);
        let id = state.next_flight;
        let epoch = state.epoch;
        let timeout = aggregate_timeout(self.inner.request_timeout, &state.snapshot);
        let (sender, receiver) = watch::channel(FlightOutcome::Pending);
        state.flight = Some(UsageFlight { id, epoch, receiver: receiver.clone() });
        self.inner.pending.fetch_add(1, Ordering::AcqRel);
        let owner = self.clone();
        tokio::spawn(async move {
            let _settlement = UsageOwnerSettlement(owner.inner.clone());
            let reports = owner.fetch_broker_reports(timeout).await.ok();
            let mut state = owner.inner.state.lock().unwrap();
            let current = state.flight.as_ref().is_some_and(|flight| flight.id == id && flight.epoch == epoch);
            if state.epoch == epoch && current {
                // Every failure caches null for 15s; there is no last-good fallback.
                state.cache = Some(UsageCache { reports: reports.clone(), fetched_at: (owner.inner.clock)() });
                let _ = sender.send(FlightOutcome::Ready { epoch, reports });
            } else {
                let _ = sender.send(FlightOutcome::Obsolete);
            }
            // An obsolete owner's cleanup must not detach the newer owner.
            if current {
                state.flight = None;
            }
        });
        UsageLoad::Flight(receiver)
    }

    async fn load_reports(
        &self,
        cancel: Option<&CancellationToken>,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        loop {
            let mut receiver = match self.usage_load() {
                UsageLoad::Cached(reports) => {
                    if cancel.is_some_and(CancellationToken::is_cancelled) {
                        return Err(AuthStorageError::Cancelled);
                    }
                    return Ok(reports);
                }
                UsageLoad::Flight(receiver) => receiver,
            };
            loop {
                if cancel.is_some_and(CancellationToken::is_cancelled) {
                    return Err(AuthStorageError::Cancelled);
                }
                let outcome = receiver.borrow().clone();
                match outcome {
                    FlightOutcome::Ready { epoch, reports } => {
                        if self.inner.state.lock().unwrap().epoch == epoch {
                            return Ok(reports);
                        }
                        break;
                    }
                    FlightOutcome::Obsolete => break,
                    FlightOutcome::Pending => {}
                }
                let changed = if let Some(cancel) = cancel {
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                        result = receiver.changed() => result,
                    }
                } else {
                    receiver.changed().await
                };
                if changed.is_err() {
                    return Err(AuthStorageError::Unavailable);
                }
            }
            // Invalidation advances the epoch. Old waiters join the current
            // cache/flight rather than returning an obsolete owner's response.
        }
    }

    async fn fetch_broker_reports(&self, timeout: Duration) -> Result<Vec<UsageReport>, AuthStorageError> {
        let cancel = CancellationToken::new();
        let response = self.fetch_raw(Method::GET, "/v1/usage", timeout, &cancel).await?;
        let bytes = response.bytes().await.map_err(|_| AuthStorageError::Transient)?;
        // Native parsing/schema checks occur after fetchRaw's transport retries.
        let body: UsageResponse = parse_object_body(&bytes)?;
        body.reports.into_iter().map(project_broker_report).collect()
    }

    async fn fetch_raw(
        &self,
        method: Method,
        path: &str,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Response, AuthStorageError> {
        for attempt in 0..=1 {
            if cancel.is_cancelled() {
                return Err(AuthStorageError::Cancelled);
            }
            let send = self
                .inner
                .client
                .request(method.clone(), format!("{}{path}", self.inner.base_url))
                .header(reqwest::header::ACCEPT, "application/json")
                .bearer_auth(&self.inner.bearer)
                .timeout(timeout)
                .send();
            let response = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                response = send => response,
            };
            let response = match response {
                Ok(response) => response,
                Err(_) => {
                    if cancel.is_cancelled() {
                        return Err(AuthStorageError::Cancelled);
                    }
                    if attempt == 0 {
                        continue;
                    }
                    return Err(AuthStorageError::Transient);
                }
            };
            if !response.status().is_success() && response.status() != reqwest::StatusCode::NOT_MODIFIED {
                // Even an unreadable error body has a known HTTP status and
                // cannot enter the transport retry path. No secrets are logged.
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {},
                    _ = response.bytes() => {},
                }
                return Err(AuthStorageError::Definitive);
            }
            return Ok(response);
        }
        Err(AuthStorageError::Transient)
    }
}

#[async_trait]
impl AggregateUsageSource for AuthBrokerUsageStore {
    async fn fetch_usage_reports(&self) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        self.aggregate_reports(None).await
    }

    async fn wait_for_settlement(&self) {
        loop {
            let notified = self.inner.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.pending.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

#[async_trait]
impl UsageStoreHooks for AuthBrokerUsageStore {
    async fn get_usage_report(
        &self,
        request: UsageRequest,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, AuthStorageError> {
        let reports = self.load_reports(Some(cancel)).await?;
        let identity = AuthBrokerUsageIdentity::from(&request.credential);
        let mut state = self.inner.state.lock().unwrap();
        let reports = reports.map(|reports| filter_reports(reports, &state.snapshot, self.inner.account_pool.as_ref()));
        let matched = reports
            .as_ref()
            .and_then(|reports| matching_report_index(reports, &request.provider, &identity))
            .map(|index| reports.as_ref().unwrap()[index].clone());
        let overlay = active_overlay(&mut state.overlays, &request.provider, &identity, (self.inner.clock)());
        Ok(match (matched, overlay) {
            (Some(base), Some(overlay)) => Some(merge_reports(base, overlay)),
            (base, overlay) => overlay.or(base),
        })
    }

    fn ingest_usage_report(&self, request: &UsageRequest, report: UsageReport) -> bool {
        let identity = AuthBrokerUsageIdentity::from(&request.credential);
        let Some(key) = overlay_key(&request.provider, &identity) else { return false };
        let mut state = self.inner.state.lock().unwrap();
        let active = active_overlay(&mut state.overlays, &request.provider, &identity, (self.inner.clock)());
        let report = match active {
            Some(base) => merge_reports(base, report),
            None => report,
        };
        if let Some((_, stored)) = state.overlays.iter_mut().find(|(candidate, _)| candidate == &key) {
            *stored = report;
        } else {
            state.overlays.push((key, report));
        }
        true
    }

    fn invalidate_usage_cache(&self) {
        let mut state = self.inner.state.lock().unwrap();
        state.cache = None;
        state.flight = None;
        state.epoch = state.epoch.wrapping_add(1);
    }

    async fn notify_usage_stale(&self, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        // Native POST shares the transport retry loop. This route is a stale
        // notification, not the separate reset-credit no-retry contract.
        let response = self.fetch_raw(Method::POST, "/v1/usage/stale", self.inner.request_timeout, cancel).await?;
        let bytes = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
            result = response.bytes() => result.map_err(|_| AuthStorageError::Transient)?,
        };
        let _: UsageStaleResponse = parse_object_body(&bytes)?;
        Ok(())
    }
}

struct UsageOwnerSettlement(Arc<BrokerUsageInner>);
impl Drop for UsageOwnerSettlement {
    fn drop(&mut self) {
        self.0.pending.fetch_sub(1, Ordering::AcqRel);
        self.0.settled.notify_waiters();
    }
}

fn parse_object_body<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, AuthStorageError> {
    // Native JSON.parse settles duplicate keys before schema validation. Value
    // has the same last-value behavior, unlike direct struct deserialization.
    let value: Value = serde_json::from_slice(bytes).map_err(|_| AuthStorageError::Definitive)?;
    if !value.is_object() {
        // Serde's non-flattened envelope structs also accept positional arrays;
        // the fixed broker schemas require their named object properties.
        return Err(AuthStorageError::Definitive);
    }
    serde_json::from_value(value).map_err(|_| AuthStorageError::Definitive)
}

fn array_properties(values: &[Value]) -> Map<String, Value> {
    values.iter().enumerate().map(|(index, value)| (index.to_string(), value.clone())).collect()
}

fn project_broker_report(mut wire: Value) -> Result<UsageReport, AuthStorageError> {
    let mut opaque_metadata = None;
    let mut opaque_credits = None;
    let mut opaque_reset_labels = Vec::new();
    if let Some(report) = wire.as_object_mut() {
        // Fixed omptype object validation accepts arrays too. Metadata has no
        // required named properties, so an array survives that wire schema.
        if report.get("metadata").is_some_and(Value::is_array) {
            opaque_metadata = report.remove("metadata");
        }
        if let Some(Value::Array(limits)) = report.get_mut("limits") {
            for (index, limit) in limits.iter_mut().enumerate() {
                let Some(window) = limit.get_mut("window").and_then(Value::as_object_mut) else { continue };
                // resetLabel is intentionally NOT declared by usageWindowSchema.
                // Its arbitrary JSON value must not acquire the shared type's
                // stricter Option<String> validation at this broker boundary.
                if window.get("resetLabel").is_some_and(|value| !value.is_string()) {
                    opaque_reset_labels.push((index, window.remove("resetLabel").unwrap()));
                }
            }
        }
        if let Some(reset) = report.get_mut("resetCredits").and_then(Value::as_object_mut)
            && let Some(Value::Array(credits)) = reset.get_mut("credits")
            && credits.iter().any(Value::is_array)
        {
            // Each credit schema has only optional named properties. Preserve
            // the original list, while numeric-property projection lets serde
            // validate every ordinary object and reject invalid sibling values.
            opaque_credits = Some(Value::Array(credits.clone()));
            for credit in credits {
                if let Value::Array(values) = credit {
                    *credit = Value::Object(array_properties(values));
                }
            }
        }
    }
    let mut report: UsageReport = serde_json::from_value(wire).map_err(|_| AuthStorageError::Definitive)?;
    if let Some(metadata) = opaque_metadata {
        // None typed properties skip serialization; the flattened raw property
        // therefore retains the native array without emitting a duplicate key.
        report.unknown_fields.insert("metadata".into(), metadata);
    }
    if let Some(credits) = opaque_credits {
        let reset = report.reset_credits.as_mut().ok_or(AuthStorageError::Definitive)?;
        reset.credits = None;
        reset.unknown_fields.insert("credits".into(), credits);
    }
    for (index, value) in opaque_reset_labels {
        let window =
            report.limits.get_mut(index).and_then(|limit| limit.window.as_mut()).ok_or(AuthStorageError::Definitive)?;
        window.unknown_fields.insert("resetLabel".into(), value);
    }
    Ok(report)
}

fn metadata_properties(report: &UsageReport) -> Map<String, Value> {
    if let Some(metadata) = &report.metadata {
        return metadata.clone();
    }
    match report.unknown_fields.get("metadata") {
        Some(Value::Array(values)) => array_properties(values),
        _ => Map::new(),
    }
}

fn aggregate_timeout(configured: Duration, raw_snapshot: &[AuthBrokerUsageSnapshotEntry]) -> Duration {
    // Native keyed upserts count every durable ID (including API keys) before
    // filtering the snapshot; a provider change for an ID replaces its owner.
    let by_id: BTreeMap<_, _> = raw_snapshot.iter().map(|entry| (entry.id, &entry.provider)).collect();
    let mut counts = BTreeMap::<&str, u32>::new();
    for provider in by_id.values() {
        let count = counts.entry(provider.as_str()).or_default();
        *count = count.saturating_add(1);
    }
    let maximum = counts.values().copied().max().unwrap_or(1).max(1);
    configured.max(DEFAULT_TIMEOUT).saturating_mul(maximum.saturating_add(1))
}

fn normalized(value: Option<&str>) -> Option<String> {
    value.map(|value| value.trim().to_lowercase()).filter(|value| !value.is_empty())
}

fn metadata_string(report: &UsageReport, key: &str) -> Option<String> {
    normalized(report.metadata.as_ref().and_then(|metadata| metadata.get(key)).and_then(Value::as_str))
}

fn overlay_key(provider: &str, identity: &AuthBrokerUsageIdentity) -> Option<String> {
    let base = normalized(identity.account_id.as_deref())
        .map(|value| format!("account:{value}"))
        .or_else(|| normalized(identity.email.as_deref()).map(|value| format!("email:{value}")))
        .or_else(|| normalized(identity.project_id.as_deref()).map(|value| format!("project:{value}")));
    let org = normalized(identity.org_id.as_deref());
    match (org, base) {
        (Some(org), Some(base)) => Some(format!("{provider}\0org:{org}|{base}")),
        (Some(org), None) => Some(format!("{provider}\0org:{org}")),
        (None, Some(base)) => Some(format!("{provider}\0{base}")),
        (None, None) => None,
    }
}

fn report_matches_identity(report: &UsageReport, identity: &AuthBrokerUsageIdentity) -> bool {
    if let Some(account) = normalized(identity.account_id.as_deref()) {
        let metadata = metadata_string(report, "accountId").or_else(|| metadata_string(report, "account_id"));
        if metadata.as_deref() == Some(account.as_str())
            || report
                .limits
                .iter()
                .any(|limit| limit.scope.account_id.as_ref().is_some_and(|value| value.to_lowercase() == account))
        {
            return true;
        }
    }
    if let Some(email) = normalized(identity.email.as_deref())
        && metadata_string(report, "email").as_deref() == Some(email.as_str())
    {
        return true;
    }
    if let Some(project) = normalized(identity.project_id.as_deref()) {
        let metadata = metadata_string(report, "projectId").or_else(|| metadata_string(report, "project_id"));
        if metadata.as_deref() == Some(project.as_str())
            || report
                .limits
                .iter()
                .any(|limit| limit.scope.project_id.as_ref().is_some_and(|value| value.to_lowercase() == project))
        {
            return true;
        }
    }
    false
}

fn matching_report_index(reports: &[UsageReport], provider: &str, identity: &AuthBrokerUsageIdentity) -> Option<usize> {
    let all: Vec<_> = reports.iter().enumerate().filter(|(_, report)| report.provider == provider).collect();
    let org = normalized(identity.org_id.as_deref());
    let candidates: Vec<_> =
        all.iter().copied().filter(|(_, report)| metadata_string(report, "orgId") == org).collect();
    if let Some(_org) = org {
        let has_base = normalized(identity.account_id.as_deref()).is_some()
            || normalized(identity.email.as_deref()).is_some()
            || normalized(identity.project_id.as_deref()).is_some();
        if has_base {
            candidates.into_iter().find(|(_, report)| report_matches_identity(report, identity)).map(|(index, _)| index)
        } else if candidates.len() == 1 {
            Some(candidates[0].0)
        } else {
            None
        }
    } else if all.len() == 1 && candidates.len() == 1 {
        // Preserve the native legacy lone-report fallback only when org-less.
        Some(candidates[0].0)
    } else {
        candidates.into_iter().find(|(_, report)| report_matches_identity(report, identity)).map(|(index, _)| index)
    }
}

fn filter_reports(
    reports: Vec<UsageReport>,
    raw_snapshot: &[AuthBrokerUsageSnapshotEntry],
    pool: Option<&AuthBrokerAccountPool>,
) -> Vec<UsageReport> {
    let Some(pool) = pool else { return reports };
    reports
        .into_iter()
        .filter(|report| {
            let Some(allowed) = pool.get(&report.provider) else { return true };
            raw_snapshot.iter().any(|entry| {
                if entry.provider != report.provider
                    || entry.credential_type != UsageCredentialType::Oauth
                    || !entry.identity_key.as_ref().is_some_and(|key| allowed.contains(key))
                {
                    return false;
                }
                // Preserve even the native blank-vs-missing org distinction here.
                let org = entry.identity.org_id.as_ref().map(|value| value.trim().to_lowercase());
                if org != metadata_string(report, "orgId") {
                    return false;
                }
                let has_base = normalized(entry.identity.account_id.as_deref()).is_some()
                    || normalized(entry.identity.email.as_deref()).is_some()
                    || normalized(entry.identity.project_id.as_deref()).is_some();
                if has_base { report_matches_identity(report, &entry.identity) } else { org.is_some() }
            })
        })
        .collect()
}

fn active_overlay(
    overlays: &mut Vec<(String, UsageReport)>,
    provider: &str,
    identity: &AuthBrokerUsageIdentity,
    now: f64,
) -> Option<UsageReport> {
    let key = overlay_key(provider, identity)?;
    let index = overlays.iter().position(|(candidate, _)| candidate == &key)?;
    if now - overlays[index].1.fetched_at >= USAGE_CACHE_TTL_MS {
        overlays.remove(index);
        None
    } else {
        Some(overlays[index].1.clone())
    }
}

fn apply_overlays(mut reports: Vec<UsageReport>, overlays: &[(String, UsageReport)], now: f64) -> Vec<UsageReport> {
    for (_, overlay) in overlays.iter().filter(|(_, report)| now - report.fetched_at < USAGE_CACHE_TTL_MS) {
        let identity = AuthBrokerUsageIdentity {
            account_id: metadata_string(overlay, "accountId"),
            email: metadata_string(overlay, "email"),
            project_id: metadata_string(overlay, "projectId"),
            org_id: metadata_string(overlay, "orgId"),
        };
        if let Some(index) = matching_report_index(&reports, &overlay.provider, &identity) {
            reports[index] = merge_reports(reports[index].clone(), overlay.clone());
        } else {
            reports.push(overlay.clone());
        }
    }
    reports
}

fn merge_reports(mut base: UsageReport, overlay: UsageReport) -> UsageReport {
    let overlay_metadata = metadata_properties(&overlay);
    let base_metadata = metadata_properties(&base);
    // Native Map uses the last duplicate value at the first insertion position.
    let mut replacements: Vec<UsageLimit> = Vec::new();
    for limit in overlay.limits {
        if let Some(existing) = replacements.iter_mut().find(|existing| existing.id == limit.id) {
            *existing = limit;
        } else {
            replacements.push(limit);
        }
    }
    for limit in &mut base.limits {
        if let Some(index) = replacements.iter().position(|replacement| replacement.id == limit.id) {
            *limit = replacements.remove(index);
        }
    }
    base.limits.extend(replacements);
    base.fetched_at = base.fetched_at.max(overlay.fetched_at);
    let mut metadata = overlay_metadata.clone();
    metadata.extend(base_metadata);
    if let Some(updated) = overlay_metadata.get("headersUpdatedAt") {
        metadata.insert("headersUpdatedAt".into(), updated.clone());
    }
    // Native object spread replaces metadata arrays with the merged object.
    base.unknown_fields.remove("metadata");
    base.metadata = Some(metadata);
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::Semaphore;

    const NOW: f64 = 1_000_000.0;

    struct BrokerReply {
        wire: Option<Vec<u8>>,
        gate: Option<Arc<Semaphore>>,
    }

    impl BrokerReply {
        fn json(status: u16, body: Value) -> Self {
            let body = body.to_string();
            Self::raw(format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ))
        }

        fn raw(wire: String) -> Self {
            Self { wire: Some(wire.into_bytes()), gate: None }
        }

        fn disconnected() -> Self {
            Self { wire: None, gate: None }
        }

        fn gated(mut self, gate: Arc<Semaphore>) -> Self {
            self.gate = Some(gate);
            self
        }
    }

    #[derive(Clone, Debug)]
    struct BrokerRequest {
        line: String,
        bearer_received: bool,
        body_length: usize,
    }

    /// Actual HTTP socket fixture: pre-header disconnects differ from known
    /// HTTP errors and truncated post-header bodies. It records no token text.
    struct LoopbackBroker {
        url: String,
        requests: Arc<Mutex<Vec<BrokerRequest>>>,
        arrivals: Arc<Semaphore>,
        cancel: CancellationToken,
        task: tokio::task::JoinHandle<()>,
    }

    impl LoopbackBroker {
        async fn start(replies: Vec<BrokerReply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let arrivals = Arc::new(Semaphore::new(0));
            let cancel = CancellationToken::new();
            let sequence = Arc::new(AtomicUsize::new(0));
            let replies = Arc::new(replies);
            let (seen, arrived, stop) = (requests.clone(), arrivals.clone(), cancel.clone());
            let task = tokio::spawn(async move {
                loop {
                    let socket = tokio::select! {
                        _ = stop.cancelled() => break,
                        socket = listener.accept() => socket,
                    };
                    let Ok((mut socket, _)) = socket else { break };
                    let (seen, arrived, stop, sequence, replies) =
                        (seen.clone(), arrived.clone(), stop.clone(), sequence.clone(), replies.clone());
                    tokio::spawn(async move {
                        let mut head = Vec::new();
                        let mut buffer = [0u8; 1024];
                        loop {
                            let read = tokio::select! {
                                _ = stop.cancelled() => return,
                                read = socket.read(&mut buffer) => read,
                            };
                            let Ok(count) = read else { return };
                            if count == 0 {
                                return;
                            }
                            head.extend_from_slice(&buffer[..count]);
                            if head.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                                break;
                            }
                            assert!(head.len() < 16_384);
                        }
                        let head = String::from_utf8(head).unwrap();
                        let index = sequence.fetch_add(1, Ordering::SeqCst);
                        let request = BrokerRequest {
                            line: head.lines().next().unwrap().to_owned(),
                            bearer_received: head
                                .lines()
                                .any(|line| line.eq_ignore_ascii_case("authorization: Bearer fixture-broker-bearer")),
                            body_length: head
                                .lines()
                                .filter_map(|line| line.split_once(':'))
                                .find_map(|(name, value)| {
                                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                                })
                                .unwrap_or(0),
                        };
                        seen.lock().unwrap().push(request);
                        arrived.add_permits(1);
                        let Some(reply) = replies.get(index) else { return };
                        if let Some(gate) = &reply.gate {
                            tokio::select! {
                                _ = stop.cancelled() => return,
                                permit = gate.acquire() => permit.unwrap().forget(),
                            }
                        }
                        if let Some(wire) = &reply.wire {
                            let _ = socket.write_all(wire).await;
                        }
                        let _ = socket.shutdown().await;
                    });
                }
            });
            Self { url, requests, arrivals, cancel, task }
        }

        async fn arrived(&self) {
            tokio::time::timeout(Duration::from_secs(2), self.arrivals.acquire()).await.unwrap().unwrap().forget();
        }

        fn count(&self) -> usize {
            self.requests.lock().unwrap().len()
        }
    }

    impl Drop for LoopbackBroker {
        fn drop(&mut self) {
            self.cancel.cancel();
            self.task.abort();
        }
    }

    fn new_clock() -> (Arc<Mutex<f64>>, Arc<dyn Fn() -> f64 + Send + Sync>) {
        let now = Arc::new(Mutex::new(NOW));
        let clock_now = now.clone();
        (now, Arc::new(move || *clock_now.lock().unwrap()))
    }

    fn options(clock: Arc<dyn Fn() -> f64 + Send + Sync>) -> AuthBrokerUsageOptions {
        AuthBrokerUsageOptions {
            client: Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap(),
            clock,
            ..AuthBrokerUsageOptions::default()
        }
    }

    fn make_store(broker: &LoopbackBroker, options: AuthBrokerUsageOptions) -> AuthBrokerUsageStore {
        AuthBrokerUsageStore::new(format!("{}/", broker.url), "fixture-broker-bearer", options).unwrap()
    }

    fn report(provider: &str, account: Option<&str>, org: Option<&str>, label: &str) -> UsageReport {
        let mut metadata = Map::new();
        if let Some(account) = account {
            metadata.insert("accountId".into(), json!(account));
        }
        if let Some(org) = org {
            metadata.insert("orgId".into(), json!(org));
        }
        UsageReport {
            provider: provider.into(),
            fetched_at: NOW,
            limits: vec![UsageLimit { id: "meter".into(), label: label.into(), ..UsageLimit::default() }],
            metadata: Some(metadata),
            ..UsageReport::default()
        }
    }

    fn usage_reply(reports: &[UsageReport]) -> BrokerReply {
        BrokerReply::json(200, json!({"generatedAt": NOW, "reports": reports}))
    }

    fn usage_request(provider: &str, account: Option<&str>, org: Option<&str>) -> UsageRequest {
        let credential = serde_json::from_value(json!({
            "type": "oauth", "accessToken": "fixture-account-token",
        }))
        .unwrap();
        let mut request = UsageRequest {
            provider: provider.into(),
            credential,
            credential_id: None,
            base_url: None,
            account_key: "fixture-account".into(),
        };
        request.credential.account_id = account.map(str::to_owned);
        request.credential.org_id = org.map(str::to_owned);
        request
    }

    fn snapshot(
        id: i64,
        provider: &str,
        key: Option<&str>,
        account: Option<&str>,
        org: Option<&str>,
    ) -> AuthBrokerUsageSnapshotEntry {
        AuthBrokerUsageSnapshotEntry {
            id,
            provider: provider.into(),
            credential_type: UsageCredentialType::Oauth,
            identity_key: key.map(str::to_owned),
            identity: AuthBrokerUsageIdentity {
                account_id: account.map(str::to_owned),
                org_id: org.map(str::to_owned),
                ..AuthBrokerUsageIdentity::default()
            },
        }
    }

    async fn bounded<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(3), future).await.unwrap()
    }

    // Native remote-store.ts:1071-1098,1180-1262; client.ts:275-287.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn broker_usage_native_shared_cache_cancel_and_owner_settlement_family() {
        let gate = Arc::new(Semaphore::new(0));
        let first = report("fixture", Some("account"), None, "first");
        let second = report("fixture", Some("account"), None, "second");
        let broker = LoopbackBroker::start(vec![
            usage_reply(std::slice::from_ref(&first)).gated(gate.clone()),
            usage_reply(std::slice::from_ref(&second)),
        ])
        .await;
        let (now, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        let aggregate = {
            let store = store.clone();
            tokio::spawn(async move { store.fetch_usage_reports().await })
        };
        broker.arrived().await;
        let per_account = {
            let store = store.clone();
            tokio::spawn(async move {
                store.get_usage_report(usage_request("fixture", Some("account"), None), &CancellationToken::new()).await
            })
        };
        let cancel = CancellationToken::new();
        let cancelled = {
            let (store, cancel) = (store.clone(), cancel.clone());
            tokio::spawn(async move { store.fetch_usage_reports_with_cancel(&cancel).await })
        };
        cancel.cancel();
        assert_eq!(bounded(cancelled).await.unwrap(), Err(AuthStorageError::Cancelled));
        let settlement = {
            let store = store.clone();
            tokio::spawn(async move { store.wait_for_settlement().await })
        };
        tokio::task::yield_now().await;
        assert!(!settlement.is_finished());
        assert_eq!(broker.count(), 1);
        gate.add_permits(1);
        assert_eq!(bounded(aggregate).await.unwrap().unwrap(), Some(vec![first.clone()]));
        assert_eq!(bounded(per_account).await.unwrap().unwrap(), Some(first.clone()));
        bounded(settlement).await.unwrap();
        assert_eq!(store.fetch_usage_reports().await.unwrap(), Some(vec![first]));
        assert_eq!(broker.count(), 1);
        *now.lock().unwrap() = NOW + USAGE_CACHE_TTL_MS;
        assert_eq!(bounded(store.fetch_usage_reports()).await.unwrap(), Some(vec![second]));
        assert_eq!(broker.count(), 2);
        assert!(
            broker
                .requests
                .lock()
                .unwrap()
                .iter()
                .all(|request| request.bearer_received && request.line == "GET /v1/usage HTTP/1.1")
        );

        // Full raw accounts, before pool filtering; repeated ID/provider changes
        // follow native keyed upsert counting rather than inflating the count.
        let raw = vec![
            snapshot(1, "many", None, None, None),
            snapshot(2, "many", None, None, None),
            snapshot(3, "many", None, None, None),
            snapshot(4, "many", None, None, None),
            snapshot(9, "many", None, None, None),
            snapshot(9, "other", None, None, None),
        ];
        assert_eq!(aggregate_timeout(Duration::from_millis(1), &raw), Duration::from_secs(50));
        assert_eq!(aggregate_timeout(Duration::from_secs(12), &raw), Duration::from_secs(60));
        assert_eq!(aggregate_timeout(DEFAULT_TIMEOUT, &[]), Duration::from_secs(20));

        // With every foreground waiter cancelled, settlement still waits for
        // the detached HTTP owner rather than reporting premature completion.
        let sole_gate = Arc::new(Semaphore::new(0));
        let sole_broker = LoopbackBroker::start(vec![usage_reply(&[]).gated(sole_gate.clone())]).await;
        let (_, clock) = new_clock();
        let sole_store = make_store(&sole_broker, options(clock));
        let sole_cancel = CancellationToken::new();
        let sole_waiter = {
            let (store, cancel) = (sole_store.clone(), sole_cancel.clone());
            tokio::spawn(async move { store.fetch_usage_reports_with_cancel(&cancel).await })
        };
        sole_broker.arrived().await;
        sole_cancel.cancel();
        assert_eq!(bounded(sole_waiter).await.unwrap(), Err(AuthStorageError::Cancelled));
        let remaining_owner = {
            let store = sole_store.clone();
            tokio::spawn(async move { store.wait_for_settlement().await })
        };
        assert_eq!(sole_store.inner.pending.load(Ordering::Acquire), 1);
        tokio::task::yield_now().await;
        assert!(!remaining_owner.is_finished());
        sole_gate.add_permits(1);
        bounded(remaining_owner).await.unwrap();
        assert_eq!(sole_store.fetch_usage_reports().await.unwrap(), Some(Vec::new()));
        assert_eq!(sole_broker.count(), 1);
    }

    // Native remote-store.ts:1020-1023,1235-1262: both success/failure owners
    // must redirect old waiters and may not clear the new flight during cleanup.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn broker_usage_native_epoch_redirect_and_cleanup_identity_family() {
        for old_fails in [false, true] {
            let old_gate = Arc::new(Semaphore::new(0));
            let new_gate = Arc::new(Semaphore::new(0));
            let old = report("fixture", Some("account"), None, "obsolete");
            let new = report("fixture", Some("account"), None, "current");
            let old_reply =
                if old_fails { BrokerReply::json(503, json!({"error": "fixture"})) } else { usage_reply(&[old]) };
            let broker = LoopbackBroker::start(vec![
                old_reply.gated(old_gate.clone()),
                usage_reply(std::slice::from_ref(&new)).gated(new_gate.clone()),
            ])
            .await;
            let (_, clock) = new_clock();
            let store = make_store(&broker, options(clock));
            let old_waiter = {
                let store = store.clone();
                tokio::spawn(async move { store.fetch_usage_reports().await })
            };
            broker.arrived().await;
            store.invalidate_usage_cache();
            let new_waiter = {
                let store = store.clone();
                tokio::spawn(async move { store.fetch_usage_reports().await })
            };
            broker.arrived().await;
            old_gate.add_permits(1);
            bounded(async {
                while store.inner.pending.load(Ordering::Acquire) != 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await;
            assert!(!old_waiter.is_finished());
            assert_eq!(store.inner.state.lock().unwrap().flight.as_ref().unwrap().epoch, 1);
            let peer = {
                let store = store.clone();
                tokio::spawn(async move {
                    store
                        .get_usage_report(usage_request("fixture", Some("account"), None), &CancellationToken::new())
                        .await
                })
            };
            new_gate.add_permits(1);
            assert_eq!(bounded(old_waiter).await.unwrap().unwrap(), Some(vec![new.clone()]));
            assert_eq!(bounded(new_waiter).await.unwrap().unwrap(), Some(vec![new.clone()]));
            assert_eq!(bounded(peer).await.unwrap().unwrap(), Some(new.clone()));
            bounded(store.wait_for_settlement()).await;
            assert_eq!(store.fetch_usage_reports().await.unwrap(), Some(vec![new]));
            assert_eq!(broker.count(), 2);
        }
    }

    // Native client.ts:399-420,435-505, wire-schemas.ts:260-277. Known HTTP
    // status, successful-header body failure, JSON and schema errors never retry.
    #[tokio::test]
    async fn broker_usage_native_wire_failures_null_cache_and_no_retry_family() {
        let good = report("fixture", Some("account"), None, "good");
        let invalid = vec![
            BrokerReply::json(401, json!({"error": "rejected"})),
            BrokerReply::json(503, json!({"error": "unavailable"})),
            BrokerReply::json(304, json!({})),
            BrokerReply::raw("HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: close\r\n\r\n{".into()),
            BrokerReply::json(200, json!({"reports": []})),
            BrokerReply::json(200, json!({"generatedAt": NOW, "reports": null})),
            BrokerReply::json(200, json!({"generatedAt": NOW, "reports": [], "unknown": true})),
            BrokerReply::json(200, json!([NOW, []])),
            BrokerReply::json(200, json!({"generatedAt": NOW, "reports": [{"fetchedAt": NOW, "limits": []}]})),
            BrokerReply::json(
                200,
                json!({"generatedAt": NOW, "reports": [{"provider": "fixture", "fetchedAt": NOW, "limits": [], "notes": null}]}),
            ),
            BrokerReply::raw(
                "HTTP/1.1 200 OK\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{\"generatedAt\": 1}".into(),
            ),
            BrokerReply::raw("HTTP/1.1 500 Error\r\nContent-Length: 500\r\nConnection: close\r\n\r\n{".into()),
            // Accepting array-shaped metadata/credits must not bypass invalid
            // declared fields on the same report or an ordinary credit sibling.
            BrokerReply::json(
                200,
                json!({"generatedAt": NOW, "reports": [{
                    "provider": "fixture", "fetchedAt": NOW, "limits": [], "metadata": [], "notes": null
                }]}),
            ),
            BrokerReply::json(
                200,
                json!({"generatedAt": NOW, "reports": [{
                    "provider": "fixture", "fetchedAt": NOW, "limits": [],
                    "resetCredits": {"availableCount": 2.0, "credits": [[], {"status": null}]}
                }]}),
            ),
            BrokerReply::json(
                200,
                json!({"generatedAt": NOW, "reports": [{
                    "provider": "fixture", "fetchedAt": NOW, "limits": [],
                    "resetCredits": {"availableCount": 2.0, "credits": [[], "not-an-object"]}
                }]}),
            ),
        ];
        for reply in invalid {
            let broker = LoopbackBroker::start(vec![reply, usage_reply(std::slice::from_ref(&good))]).await;
            let (now, clock) = new_clock();
            let store = make_store(&broker, options(clock));
            assert_eq!(bounded(store.fetch_usage_reports()).await.unwrap(), None);
            assert_eq!(
                store
                    .get_usage_report(usage_request("fixture", Some("account"), None), &CancellationToken::new())
                    .await
                    .unwrap(),
                None
            );
            *now.lock().unwrap() = NOW + USAGE_CACHE_TTL_MS - 1.0;
            assert_eq!(store.fetch_usage_reports().await.unwrap(), None);
            assert_eq!(broker.count(), 1);
            *now.lock().unwrap() += 1.0;
            assert_eq!(bounded(store.fetch_usage_reports()).await.unwrap(), Some(vec![good.clone()]));
            assert_eq!(broker.count(), 2);
        }

        // Fixed wire leaves resetLabel undeclared. omptype's object validator
        // accepts arrays for metadata and for all-optional credit objects; raw
        // field projection must preserve their original serializable shapes.
        let prototype = json!({
            "provider": "fixture", "fetchedAt": NOW,
            "limits": [{"id": "meter", "label": "wire", "scope": {"provider": "fixture"},
                "amount": {"unit": "requests"}, "window": {"id": "w", "label": "W"}}],
            "metadata": {"accountId": "account"}, "raw": null, "futureReport": {"value": true}
        });
        let mut legal = Vec::new();
        for value in
            [json!("regen"), Value::Null, json!(true), json!(42), json!([1, "future"]), json!({"future": false})]
        {
            let mut wire = prototype.clone();
            wire["limits"][0]["window"]["resetLabel"] = value;
            legal.push(wire);
        }
        for metadata in [json!([]), json!(["first", {"future": true}, null])] {
            let mut wire = prototype.clone();
            wire["metadata"] = metadata;
            legal.push(wire);
        }
        for credits in [
            json!([[]]),
            json!([[1, "untyped", {"future": null}]]),
            json!([{"grantedAt": "2026-01-01", "futureCredit": true}, [], {"status": "available"}]),
        ] {
            let mut wire = prototype.clone();
            wire["resetCredits"] = json!({"availableCount": 3.0, "credits": credits});
            legal.push(wire);
        }
        let mut combined = prototype.clone();
        combined["metadata"] = json!(["base-array"]);
        combined["limits"][0]["window"]["resetLabel"] = Value::Null;
        combined["resetCredits"] = json!({"availableCount": 1.0, "credits": [[], {"status": "available"}]});
        legal.push(combined);
        for wire in legal {
            let broker = LoopbackBroker::start(vec![BrokerReply::json(
                200,
                json!({"generatedAt": NOW, "reports": [wire.clone()]}),
            )])
            .await;
            let (_, clock) = new_clock();
            let store = make_store(&broker, options(clock));
            let reports = bounded(store.fetch_usage_reports()).await.unwrap().unwrap();
            assert_eq!(reports.len(), 1);
            assert_eq!(serde_json::to_value(&reports[0]).unwrap(), wire);
            let account = usage_request("fixture", Some("account"), None);
            assert_eq!(
                store.get_usage_report(account.clone(), &CancellationToken::new()).await.unwrap(),
                Some(reports[0].clone())
            );
            assert_eq!(broker.count(), 1);
            if let Some(values) = wire["metadata"].as_array() {
                assert!(reports[0].metadata.is_none());
                let mut overlay = report("fixture", Some("account"), None, "overlay");
                overlay.limits.clear();
                overlay.metadata.as_mut().unwrap().insert("0".into(), json!("overlay-zero"));
                overlay.metadata.as_mut().unwrap().insert("headersUpdatedAt".into(), Value::Null);
                let mut expected = overlay.metadata.clone().unwrap();
                expected.extend(array_properties(values));
                assert!(store.ingest_usage_report(&account, overlay));
                let merged = store.fetch_usage_reports().await.unwrap().unwrap().remove(0);
                assert_eq!(merged.metadata, Some(expected.clone()));
                assert!(!merged.unknown_fields.contains_key("metadata"));
                let serialized = serde_json::to_value(&merged).unwrap();
                assert_eq!(serialized["metadata"], Value::Object(expected));
                // Native base-owned raw fields and its opaque window/credits
                // survive while metadata itself becomes the spread object.
                assert_eq!(serialized["limits"], wire["limits"]);
                assert_eq!(serialized.get("resetCredits"), wire.get("resetCredits"));
            }
        }

        // JSON.parse permits duplicate named fields; the final value is what
        // the fixed schema validates, including nested report properties.
        let body = format!(
            "{{\"generatedAt\":\"wrong\",\"generatedAt\":{NOW},\"reports\":null,\"reports\":[{{\"provider\":123,\"provider\":\"fixture\",\"fetchedAt\":{NOW},\"limits\":[],\"notes\":false,\"notes\":[\"last\"]}}]}}"
        );
        let reply = BrokerReply::raw(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ));
        let broker = LoopbackBroker::start(vec![reply]).await;
        let (_, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        let duplicate = store.fetch_usage_reports().await.unwrap().unwrap().remove(0);
        assert_eq!(duplicate.provider, "fixture");
        assert_eq!(duplicate.notes, Some(vec!["last".into()]));
        assert_eq!(broker.count(), 1);

        // A previous positive report must not survive a later failed epoch.
        let broker = LoopbackBroker::start(vec![usage_reply(&[good]), BrokerReply::json(503, json!({}))]).await;
        let (_, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        assert!(store.fetch_usage_reports().await.unwrap().is_some());
        store.invalidate_usage_cache();
        assert_eq!(store.fetch_usage_reports().await.unwrap(), None);
        assert_eq!(store.fetch_usage_reports().await.unwrap(), None);
        assert_eq!(broker.count(), 2);
    }

    // Native client.ts:322-326,435-505: retry one transport fault for GET and
    // POST, including transport timeout; caller cancellation never retries.
    #[tokio::test]
    async fn broker_usage_native_get_post_transport_and_stale_family() {
        let good = report("fixture", Some("account"), None, "retried");
        let broker = LoopbackBroker::start(vec![
            BrokerReply::disconnected(),
            usage_reply(std::slice::from_ref(&good)),
            BrokerReply::disconnected(),
            BrokerReply::json(200, json!({"ok": false})),
        ])
        .await;
        let (_, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        assert_eq!(bounded(store.fetch_usage_reports()).await.unwrap(), Some(vec![good]));
        store.notify_usage_stale(&CancellationToken::new()).await.unwrap();
        let seen = broker.requests.lock().unwrap().clone();
        assert_eq!(
            seen.iter().map(|request| request.line.as_str()).collect::<Vec<_>>(),
            vec![
                "GET /v1/usage HTTP/1.1",
                "GET /v1/usage HTTP/1.1",
                "POST /v1/usage/stale HTTP/1.1",
                "POST /v1/usage/stale HTTP/1.1"
            ]
        );
        assert!(seen.iter().all(|request| request.bearer_received && request.body_length == 0));

        let broker =
            LoopbackBroker::start(vec![BrokerReply::disconnected(), BrokerReply::disconnected(), usage_reply(&[])])
                .await;
        let (_, clock) = new_clock();
        let failed_store = make_store(&broker, options(clock));
        assert_eq!(bounded(failed_store.fetch_usage_reports()).await.unwrap(), None);
        assert_eq!(failed_store.fetch_usage_reports().await.unwrap(), None);
        assert_eq!(broker.count(), 2);

        for reply in [
            BrokerReply::json(503, json!({})),
            BrokerReply::json(200, json!({"ok": "true"})),
            BrokerReply::json(200, json!([true])),
            BrokerReply::json(200, json!({"ok": true, "unknown": 1})),
            BrokerReply::raw("HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}".into()),
        ] {
            let broker = LoopbackBroker::start(vec![reply, BrokerReply::json(200, json!({"ok": true}))]).await;
            let (_, clock) = new_clock();
            let store = make_store(&broker, options(clock));
            assert!(bounded(store.notify_usage_stale(&CancellationToken::new())).await.is_err());
            assert_eq!(broker.count(), 1);
        }

        let body = "{\"ok\":\"wrong\",\"ok\":false}";
        let broker = LoopbackBroker::start(vec![BrokerReply::raw(format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ))])
        .await;
        let (_, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        assert_eq!(store.notify_usage_stale(&CancellationToken::new()).await, Ok(()));
        assert_eq!(broker.count(), 1);

        let gate = Arc::new(Semaphore::new(0));
        let broker = LoopbackBroker::start(vec![
            BrokerReply::json(200, json!({"ok": true})).gated(gate.clone()),
            BrokerReply::json(200, json!({"ok": true})),
        ])
        .await;
        let (_, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(store.notify_usage_stale(&cancelled).await, Err(AuthStorageError::Cancelled));
        assert_eq!(broker.count(), 0);
        let cancel = CancellationToken::new();
        let notification = {
            let (store, cancel) = (store.clone(), cancel.clone());
            tokio::spawn(async move { store.notify_usage_stale(&cancel).await })
        };
        broker.arrived().await;
        cancel.cancel();
        assert_eq!(bounded(notification).await.unwrap(), Err(AuthStorageError::Cancelled));
        assert_eq!(broker.count(), 1);
        gate.add_permits(1);

        // Per-attempt timeout is a transport fault and retries once on POST.
        let gate = Arc::new(Semaphore::new(0));
        let broker = LoopbackBroker::start(vec![
            BrokerReply::json(200, json!({"ok": true})).gated(gate.clone()),
            BrokerReply::json(200, json!({"ok": true})),
        ])
        .await;
        let (_, clock) = new_clock();
        let mut configured = options(clock);
        configured.request_timeout = Duration::from_millis(30);
        let store = make_store(&broker, configured);
        bounded(store.notify_usage_stale(&CancellationToken::new())).await.unwrap();
        assert_eq!(broker.count(), 2);
        gate.add_permits(1);
    }

    // Native remote-store.ts:42-49,1108-1133,1367-1518: pool filtering is
    // snapshot OAuth identity based; organization gates are not member identity.
    #[tokio::test]
    async fn broker_usage_native_pool_org_and_base_identity_family() {
        let team_a = report("fixture", Some("a"), Some("team"), "team-a");
        let team_b = report("fixture", Some("b"), Some("team"), "team-b");
        let personal = report("fixture", Some("a"), None, "personal");
        let untouched = report("other", None, None, "unrestricted");
        let all = vec![team_a.clone(), team_b.clone(), personal.clone(), untouched.clone()];
        let broker = LoopbackBroker::start(vec![usage_reply(&all), usage_reply(&all)]).await;
        let (_, clock) = new_clock();
        let mut configured = options(clock.clone());
        configured.initial_snapshot = vec![
            snapshot(1, "fixture", Some("visible"), Some(" A "), Some(" TEAM ")),
            snapshot(2, "fixture", Some("hidden"), Some("b"), Some("team")),
            snapshot(3, "fixture", None, Some("a"), None),
        ];
        let mut api_key = snapshot(4, "fixture", None, Some("a"), None);
        api_key.credential_type = UsageCredentialType::ApiKey;
        configured.initial_snapshot.push(api_key);
        configured.account_pool = Some(BTreeMap::from([("fixture".into(), BTreeSet::from(["visible".into()]))]));
        let filtered = make_store(&broker, configured);
        assert_eq!(filtered.fetch_usage_reports().await.unwrap(), Some(vec![team_a.clone(), untouched]));
        assert_eq!(
            filtered
                .get_usage_report(usage_request("fixture", Some("a"), Some("team")), &CancellationToken::new())
                .await
                .unwrap(),
            Some(team_a.clone())
        );
        assert_eq!(
            filtered
                .get_usage_report(usage_request("fixture", Some("b"), Some("team")), &CancellationToken::new())
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            filtered
                .get_usage_report(usage_request("fixture", Some("a"), None), &CancellationToken::new())
                .await
                .unwrap(),
            None
        );
        filtered.replace_snapshot(vec![snapshot(1, "fixture", Some("hidden"), Some("a"), Some("team"))]);
        assert_eq!(filtered.fetch_usage_reports().await.unwrap().unwrap().len(), 1);
        assert_eq!(broker.count(), 1); // host identity changes do not refetch by themselves

        let unrestricted = make_store(&broker, options(clock));
        for (account, org, expected) in [
            (Some(" A "), Some(" TEAM "), Some(team_a)),
            (Some("a"), Some("missing"), None),
            (Some("missing"), Some("team"), None),
            (None, Some("team"), None),
            (Some("a"), None, Some(personal)),
        ] {
            assert_eq!(
                unrestricted
                    .get_usage_report(usage_request("fixture", account, org), &CancellationToken::new())
                    .await
                    .unwrap(),
                expected
            );
        }
        // Separate native legacy fallback from strict pool identity admission.
        let lone = report("fixture", None, None, "lone");
        assert_eq!(
            matching_report_index(std::slice::from_ref(&lone), "fixture", &AuthBrokerUsageIdentity::default()),
            Some(0)
        );
        assert_eq!(
            matching_report_index(
                &[lone],
                "fixture",
                &AuthBrokerUsageIdentity { org_id: Some("team".into()), ..AuthBrokerUsageIdentity::default() }
            ),
            None
        );
        let org_only = report("fixture", None, Some("team"), "org-only");
        assert_eq!(
            matching_report_index(
                &[org_only],
                "fixture",
                &AuthBrokerUsageIdentity { org_id: Some("team".into()), ..AuthBrokerUsageIdentity::default() }
            ),
            Some(0)
        );

        let mut attributed = report("fixture", None, None, "scope");
        attributed.metadata.as_mut().unwrap().insert("account_id".into(), json!(" ALIAS "));
        assert!(report_matches_identity(
            &attributed,
            &AuthBrokerUsageIdentity { account_id: Some("alias".into()), ..AuthBrokerUsageIdentity::default() }
        ));
        attributed.limits[0].scope.project_id = Some(" PROJECT ".into());
        assert!(!report_matches_identity(
            &attributed,
            &AuthBrokerUsageIdentity { project_id: Some("project".into()), ..AuthBrokerUsageIdentity::default() }
        ));
        attributed.limits[0].scope.project_id = Some("PROJECT".into());
        assert!(report_matches_identity(
            &attributed,
            &AuthBrokerUsageIdentity { project_id: Some(" project ".into()), ..AuthBrokerUsageIdentity::default() }
        ));
        let empty_pool = BTreeMap::from([("fixture".into(), BTreeSet::new())]);
        assert!(
            filter_reports(all, &[snapshot(1, "fixture", Some("visible"), Some("a"), Some("team"))], Some(&empty_pool))
                .iter()
                .all(|report| report.provider != "fixture")
        );
    }

    // Native remote-store.ts:164-212,1136-1171: overlay TTL uses fetchedAt,
    // stable native limit ordering/duplicates and base-owned fields survive.
    #[tokio::test]
    async fn broker_usage_native_overlay_merge_ttl_and_null_difference_family() {
        let mut base = report("fixture", Some("a"), Some("team"), "base");
        base.limits.push(base.limits[0].clone());
        base.limits[1].label = "second-duplicate-base".into();
        base.raw = Some(json!({"base": true}));
        base.notes = Some(vec!["base-notes".into()]);
        base.unknown_fields.insert("extension".into(), json!("base"));
        base.metadata.as_mut().unwrap().insert("conflict".into(), json!("base"));
        base.metadata.as_mut().unwrap().insert("headersUpdatedAt".into(), json!(NOW - 1.0));
        let broker = LoopbackBroker::start(vec![usage_reply(&[base.clone()]), usage_reply(&[base.clone()])]).await;
        let (now, clock) = new_clock();
        let store = make_store(&broker, options(clock));
        let request = usage_request("fixture", Some(" A "), Some(" TEAM "));
        let mut overlay = report("fixture", Some("a"), Some("team"), "first-overlay");
        let mut extra = overlay.limits[0].clone();
        extra.id = "extra".into();
        extra.label = "extra-first".into();
        overlay.limits.push(extra.clone());
        let mut duplicate = overlay.limits[0].clone();
        duplicate.label = "last-overlay".into();
        overlay.limits.push(duplicate);
        extra.label = "extra-last".into();
        overlay.limits.push(extra);
        overlay.metadata.as_mut().unwrap().insert("conflict".into(), json!("overlay"));
        overlay.metadata.as_mut().unwrap().insert("headersUpdatedAt".into(), Value::Null);
        overlay.raw = Some(json!("overlay"));
        overlay.notes = Some(vec!["overlay-notes".into()]);
        assert!(store.ingest_usage_report(&request, overlay.clone()));
        let merged = store.get_usage_report(request.clone(), &CancellationToken::new()).await.unwrap().unwrap();
        assert_eq!(
            merged.limits.iter().map(|limit| limit.label.as_str()).collect::<Vec<_>>(),
            vec!["last-overlay", "second-duplicate-base", "extra-last"]
        );
        assert_eq!(merged.raw, base.raw);
        assert_eq!(merged.notes, base.notes);
        assert_eq!(merged.unknown_fields, base.unknown_fields);
        assert_eq!(merged.metadata.as_ref().unwrap()["conflict"], json!("base"));
        assert_eq!(merged.metadata.as_ref().unwrap()["headersUpdatedAt"], Value::Null);
        assert_eq!(store.fetch_usage_reports().await.unwrap(), Some(vec![merged]));
        let sibling_request = usage_request("fixture", Some("b"), Some("team"));
        let sibling = report("fixture", Some("b"), Some("team"), "sibling");
        assert!(store.ingest_usage_report(&sibling_request, sibling.clone()));
        let aggregate = store.fetch_usage_reports().await.unwrap().unwrap();
        assert_eq!(aggregate.len(), 2);
        assert_eq!(aggregate[1], sibling);
        assert_eq!(
            store.get_usage_report(sibling_request, &CancellationToken::new()).await.unwrap().unwrap().limits[0].label,
            "sibling"
        );
        assert!(!store.ingest_usage_report(&usage_request("fixture", None, None), overlay));
        *now.lock().unwrap() = NOW + USAGE_CACHE_TTL_MS;
        assert_eq!(
            store.get_usage_report(request.clone(), &CancellationToken::new()).await.unwrap(),
            Some(base.clone())
        );
        let mut stale_overlay = report("fixture", Some("a"), Some("team"), "already-stale");
        stale_overlay.fetched_at = NOW;
        assert!(store.ingest_usage_report(&request, stale_overlay));
        assert_eq!(store.get_usage_report(request, &CancellationToken::new()).await.unwrap(), Some(base));

        let broker = LoopbackBroker::start(vec![BrokerReply::json(503, json!({}))]).await;
        let (_, clock) = new_clock();
        let null_store = make_store(&broker, options(clock));
        let request = usage_request("fixture", Some("a"), Some("team"));
        let active = report("fixture", Some("a"), Some("team"), "header-only");
        assert!(null_store.ingest_usage_report(&request, active.clone()));
        assert_eq!(null_store.fetch_usage_reports().await.unwrap(), None);
        assert_eq!(null_store.get_usage_report(request, &CancellationToken::new()).await.unwrap(), Some(active));
        assert_eq!(broker.count(), 1);
    }
}
