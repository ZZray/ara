//! In-memory owner of the fixed OMP remote credential store.
//! Source: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-broker/remote-store.ts. Credentials remain remote;
//! serialized row data below is a memory projection, never a durable CAS receipt.
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

use crate::auth_broker_client::{AuthBrokerClient, BrokerError, BrokerErrorKind};
use crate::auth_broker_usage::{
    AuthBrokerAccountPool, AuthBrokerUsageIdentity, AuthBrokerUsageSnapshotEntry, AuthBrokerUsageStore,
};
use crate::auth_broker_wire::{
    BrokerBlock, BrokerSnapshot, BrokerSnapshotEntry, BrokerSnapshotResult, BrokerStreamEvent, REMOTE_REFRESH_SENTINEL,
};
use crate::auth_storage::{AggregateUsageSource, AuthStorageError, UsageRequest, UsageStoreHooks};
use crate::auth_storage_policy::{UsageCredentialType, UsageReport};
use crate::credential_store::{
    AuthCredential, ClientUsageEntry, ClientUsageReport, CredentialRefreshLeaseFence, DisabledCredentialSummary,
    StoredAuthCredential, StoredCredentialBlock, UsageHistoryEntry, UsageHistoryQuery, serialize_credential,
};
use crate::credential_store_port::{AuthCredentialStore, CredentialBlockSettlement};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Handle;
use tokio::sync::{Notify, oneshot, watch};
use tokio_util::sync::CancellationToken;

const BLOCK_RECONCILE_DELAY_MS: f64 = 300_000.0;
const BACKGROUND_WAIT_MS: u64 = 30_000;

/// Attribution is supplied by the Host, never discovered from private config.
#[derive(Clone, Default)]
pub struct ClientUsageIdentity {
    pub install_id: String,
    pub hostname: Option<String>,
    pub app: Option<String>,
}

pub struct RemoteAuthCredentialStoreOptions {
    pub initial_snapshot: Option<BrokerSnapshot>,
    pub stream_snapshots: bool,
    pub account_pool: Option<AuthBrokerAccountPool>,
    pub observed_usage_flush: Duration,
    pub background_idle: Duration,
    pub clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    pub on_snapshot: Option<Arc<dyn Fn(BrokerSnapshot, i64) + Send + Sync>>,
    pub default_client_identity: ClientUsageIdentity,
}

impl Default for RemoteAuthCredentialStoreOptions {
    fn default() -> Self {
        Self {
            initial_snapshot: None,
            stream_snapshots: true,
            account_pool: None,
            observed_usage_flush: Duration::from_secs(10),
            background_idle: Duration::from_secs(20),
            clock: Arc::new(|| {
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1_000.0
            }),
            on_snapshot: None,
            default_client_identity: ClientUsageIdentity::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteOperation {
    Refresh { credential_id: i64 },
    Upload { provider: String },
    Disable { credential_id: i64 },
    UpsertBlock { credential_id: i64 },
    DeleteBlocks { credential_id: i64 },
    ObservedUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteOperationOutcome {
    Pending,
    Succeeded,
    Failed,
    OutcomeUnknown,
}

/// Secret-free operation receipt. A settled job may still have unknown effects.
#[derive(Clone, Debug)]
pub struct RemoteOperationReceipt {
    pub id: u64,
    pub operation: RemoteOperation,
    pub outcome: RemoteOperationOutcome,
    pub status: Option<u16>,
    pub at_ms: f64,
}

/// Incremental reports with unknown effects are retained separately, never replayed.
/// Intentionally no Debug implementation, including the Host attribution fields.
#[derive(Clone)]
pub struct UnknownObservedUsageBatch {
    pub receipt_id: u64,
    pub report: ClientUsageReport,
    pub status: Option<u16>,
}

#[derive(Clone, Copy, Debug)]
pub struct RemoteBackgroundStatus {
    pub closed: bool,
    pub streaming_active: bool,
    pub streaming_unsupported: bool,
    pub parked: bool,
    pub background_done: bool,
    pub pending_operations: usize,
    pub observed_usage_unsupported: bool,
}

#[derive(Clone)]
pub struct RemoteAuthCredentialStore {
    inner: Arc<RemoteInner>,
}

impl RemoteAuthCredentialStore {
    pub(crate) fn reset_receipt_authority(&self) -> String {
        self.inner.client.reset_receipt_authority()
    }
}

struct RemoteInner {
    client: AuthBrokerClient,
    usage: AuthBrokerUsageStore,
    handle: Handle,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    account_pool: Option<AuthBrokerAccountPool>,
    stream_snapshots: bool,
    background_idle: Duration,
    observed_usage_flush: Duration,
    default_client_identity: ClientUsageIdentity,
    on_snapshot: Option<Arc<dyn Fn(BrokerSnapshot, i64) + Send + Sync>>,
    state: Mutex<RemoteState>,
    lifecycle: Arc<RemoteLifecycle>,
    projection_changed: watch::Sender<u64>,
}

struct RemoteLifecycle {
    background_abort: CancellationToken,
    activity: Notify,
    pending: AtomicUsize,
    settled: Notify,
    background_done: AtomicBool,
    background_settled: Notify,
}

struct RemoteState {
    snapshot: BrokerSnapshot,
    // Native keeps the HTTP ETag generation separately from snapshot JSON G.
    broker_generation: i64,
    projection_revision: u64,
    raw_usage: Vec<AuthBrokerUsageSnapshotEntry>,
    snapshot_received_at: f64,
    last_activity_ms: f64,
    cache: BTreeMap<String, CacheEntry>,
    reconcile_after: BTreeMap<(i64, String, String), f64>,
    closed: bool,
    streaming_active: bool,
    streaming_unsupported: bool,
    parked: bool,
    observed_usage: Vec<PendingObservedUsage>,
    observed_timer: Option<(u64, CancellationToken)>,
    next_timer: u64,
    observed_usage_unsupported: bool,
    next_receipt: u64,
    receipts: Vec<RemoteOperationReceipt>,
    unknown_observed: Vec<UnknownObservedUsageBatch>,
}

struct CacheEntry {
    value: String,
    expires_at_sec: i64,
}
#[derive(Clone)]
struct PendingObservedUsage {
    identity: ClientUsageIdentity,
    entry: ClientUsageEntry,
}

impl RemoteAuthCredentialStore {
    pub fn new(client: AuthBrokerClient, options: RemoteAuthCredentialStoreOptions) -> Result<Self, AuthStorageError> {
        let handle = Handle::try_current().map_err(|_| AuthStorageError::Configuration)?;
        let now = (options.clock)();
        let usage = client.usage_store(options.clock.clone(), options.account_pool.clone())?;
        let initial = options.initial_snapshot.unwrap_or_default();
        let (projection_changed, _) = watch::channel(0);
        let store = Self {
            inner: Arc::new(RemoteInner {
                client,
                usage,
                handle,
                clock: options.clock,
                account_pool: options.account_pool,
                stream_snapshots: options.stream_snapshots,
                background_idle: options.background_idle,
                observed_usage_flush: options.observed_usage_flush,
                default_client_identity: options.default_client_identity,
                on_snapshot: options.on_snapshot,
                state: Mutex::new(RemoteState {
                    snapshot: BrokerSnapshot::default(),
                    broker_generation: 0,
                    projection_revision: 0,
                    raw_usage: Vec::new(),
                    snapshot_received_at: now,
                    last_activity_ms: now,
                    cache: BTreeMap::new(),
                    reconcile_after: BTreeMap::new(),
                    closed: false,
                    streaming_active: false,
                    streaming_unsupported: false,
                    parked: false,
                    observed_usage: Vec::new(),
                    observed_timer: None,
                    next_timer: 0,
                    observed_usage_unsupported: false,
                    next_receipt: 0,
                    receipts: Vec::new(),
                    unknown_observed: Vec::new(),
                }),
                lifecycle: Arc::new(RemoteLifecycle {
                    background_abort: CancellationToken::new(),
                    activity: Notify::new(),
                    pending: AtomicUsize::new(0),
                    settled: Notify::new(),
                    background_done: AtomicBool::new(false),
                    background_settled: Notify::new(),
                }),
                projection_changed,
            }),
        };
        // Native initial snapshot is applied before registering the callback.
        store.apply_snapshot(initial.clone(), initial.generation, false);
        store.inner.handle.spawn(run_background(Arc::downgrade(&store.inner), store.inner.lifecycle.clone()));
        Ok(store)
    }

    pub fn snapshot(&self) -> BrokerSnapshot {
        self.note_activity();
        self.inner.state.lock().unwrap().snapshot.clone()
    }
    pub fn generation(&self) -> i64 {
        self.inner.state.lock().unwrap().broker_generation
    }
    /// Local projection changes include write acknowledgments with no wire G.
    /// Hosts must not treat the broker generation as this monotonic revision.
    pub fn projection_revision(&self) -> u64 {
        self.inner.state.lock().unwrap().projection_revision
    }
    pub fn subscribe_projection_changes(&self) -> watch::Receiver<u64> {
        self.inner.projection_changed.subscribe()
    }
    pub fn usage_store(&self) -> AuthBrokerUsageStore {
        self.inner.usage.clone()
    }
    pub fn operation_receipts(&self) -> Vec<RemoteOperationReceipt> {
        self.inner.state.lock().unwrap().receipts.clone()
    }
    pub fn unknown_observed_batches(&self) -> Vec<UnknownObservedUsageBatch> {
        self.inner.state.lock().unwrap().unknown_observed.clone()
    }
    pub fn background_status(&self) -> RemoteBackgroundStatus {
        let state = self.inner.state.lock().unwrap();
        RemoteBackgroundStatus {
            closed: state.closed,
            streaming_active: state.streaming_active,
            streaming_unsupported: state.streaming_unsupported,
            parked: state.parked,
            background_done: self.inner.lifecycle.background_done.load(Ordering::Acquire),
            pending_operations: self.inner.lifecycle.pending.load(Ordering::Acquire),
            observed_usage_unsupported: state.observed_usage_unsupported,
        }
    }

    fn note_activity(&self) {
        let now = (self.inner.clock)();
        {
            let mut state = self.inner.state.lock().unwrap();
            if state.closed {
                return;
            }
            state.last_activity_ms = now;
        }
        self.inner.lifecycle.activity.notify_one();
    }
    fn ensure_open(&self) -> Result<(), AuthStorageError> {
        if self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.closed {
            Err(AuthStorageError::Unavailable)
        } else {
            Ok(())
        }
    }
    fn finite_owner(&self) -> Result<FiniteOwner, AuthStorageError> {
        let state = self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
        if state.closed {
            return Err(AuthStorageError::Unavailable);
        }
        Ok(FiniteOwner::new(self.inner.lifecycle.clone()))
    }

    /// A started mutation outlives its caller's wait. The independent token is
    /// never cancelled by one caller or by close's background cancellation.
    async fn run_owned<T, F, Fut>(&self, cancel: &CancellationToken, operation: F) -> Result<T, AuthStorageError>
    where
        T: Send + 'static,
        F: FnOnce(Self, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, AuthStorageError>> + Send + 'static,
    {
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled);
        }
        let finite = self.finite_owner()?;
        self.note_activity();
        let owner = self.clone();
        let (sender, receiver) = oneshot::channel();
        self.inner.handle.spawn(async move {
            let _finite = finite;
            let result = operation(owner, CancellationToken::new()).await;
            let _ = sender.send(result);
        });
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(AuthStorageError::Cancelled),
            result = receiver => result.unwrap_or(Err(AuthStorageError::OutcomeUnknown)),
        }
    }

    fn begin_receipt(&self, operation: RemoteOperation) -> MutationReceipt {
        let now = (self.inner.clock)();
        let mut state = self.inner.state.lock().unwrap();
        state.next_receipt = state.next_receipt.wrapping_add(1);
        let id = state.next_receipt;
        state.receipts.push(RemoteOperationReceipt {
            id,
            operation,
            outcome: RemoteOperationOutcome::Pending,
            status: None,
            at_ms: now,
        });
        MutationReceipt { inner: Arc::downgrade(&self.inner), id, finished: false }
    }

    fn publish_projection(&self, state: &mut RemoteState, previous: &BrokerSnapshot) {
        if !same_projection(previous, &state.snapshot) {
            state.projection_revision = state.projection_revision.wrapping_add(1);
            self.inner.projection_changed.send_replace(state.projection_revision);
        }
    }

    fn apply_snapshot(&self, raw: BrokerSnapshot, generation: i64, notify: bool) {
        self.apply_snapshot_guarded(raw, generation, notify, false);
    }
    fn apply_snapshot_guarded(&self, raw: BrokerSnapshot, generation: i64, notify: bool, ignore_stale: bool) {
        let now = (self.inner.clock)();
        {
            let mut state = self.inner.state.lock().unwrap();
            if ignore_stale && generation < state.broker_generation {
                return;
            }
            let previous_snapshot = state.snapshot.clone();
            let previous = state.snapshot.credentials.clone();
            state.raw_usage = project_usage_entries(&raw.credentials);
            let mut snapshot = raw.clone();
            snapshot.credentials.retain(|entry| in_account_pool(entry, self.inner.account_pool.as_ref()));
            for entry in &mut snapshot.credentials {
                normalize_blocks(entry, now);
            }
            if snapshot_blocks_changed(&previous, &snapshot.credentials) {
                self.inner.usage.invalidate_usage_cache();
            }
            protect_new_blocks(&mut state.reconcile_after, &previous, &snapshot.credentials, now);
            state.snapshot = snapshot;
            state.broker_generation = generation;
            state.snapshot_received_at = now;
            // The usage lock never calls back into Remote; keeping this short
            // projection write under the Remote lock preserves update ordering.
            self.inner.usage.replace_snapshot(state.raw_usage.clone());
            self.publish_projection(&mut state, &previous_snapshot);
        }
        if notify && let Some(callback) = &self.inner.on_snapshot {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(raw, generation)));
        }
    }

    fn apply_stream_event(&self, event: BrokerStreamEvent) {
        if let BrokerStreamEvent::Snapshot(snapshot) = event {
            self.apply_snapshot_guarded(snapshot.clone(), snapshot.generation, true, true);
            return;
        }
        let now = (self.inner.clock)();
        let mut state = self.inner.state.lock().unwrap();
        let previous_snapshot = state.snapshot.clone();
        let (generation, server_now_ms, refresher) = match event {
            BrokerStreamEvent::Entry { mut entry, generation, server_now_ms, refresher } => {
                if generation < state.broker_generation {
                    return;
                }
                upsert_usage_entry(&mut state.raw_usage, &entry);
                let previous = state.snapshot.credentials.clone();
                if in_account_pool(&entry, self.inner.account_pool.as_ref()) {
                    normalize_blocks(&mut entry, now);
                    if let Some(index) =
                        state.snapshot.credentials.iter().position(|candidate| candidate.id == entry.id)
                    {
                        state.snapshot.credentials[index] = entry;
                    } else {
                        state.snapshot.credentials.push(entry);
                    }
                } else {
                    state.snapshot.credentials.retain(|candidate| candidate.id != entry.id);
                }
                if snapshot_blocks_changed(&previous, &state.snapshot.credentials) {
                    self.inner.usage.invalidate_usage_cache();
                    let next = state.snapshot.credentials.clone();
                    protect_new_blocks(&mut state.reconcile_after, &previous, &next, now);
                }
                (generation, server_now_ms, refresher)
            }
            BrokerStreamEvent::Removed { id, generation, server_now_ms, refresher } => {
                if generation < state.broker_generation {
                    return;
                }
                state.raw_usage.retain(|entry| entry.id != id);
                if state.snapshot.credentials.iter().any(|entry| entry.id == id && !entry.blocks.is_empty()) {
                    self.inner.usage.invalidate_usage_cache();
                }
                state.snapshot.credentials.retain(|entry| entry.id != id);
                (generation, server_now_ms, refresher)
            }
            BrokerStreamEvent::Snapshot(_) => unreachable!(),
        };
        state.snapshot.generation = generation;
        state.broker_generation = generation;
        state.snapshot.server_now_ms = server_now_ms;
        state.snapshot.refresher = refresher;
        state.snapshot_received_at = now;
        self.inner.usage.replace_snapshot(state.raw_usage.clone());
        self.publish_projection(&mut state, &previous_snapshot);
    }

    pub async fn refresh_snapshot(&self, cancel: &CancellationToken) -> Result<BrokerSnapshot, AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        let result = self.inner.client.fetch_snapshot(None, None, cancel).await.map_err(broker_error)?;
        if let BrokerSnapshotResult::Snapshot { snapshot, generation } = result {
            self.apply_snapshot(snapshot, generation, true);
        }
        Ok(self.snapshot())
    }
    pub async fn wait_for_fresh_snapshot(
        &self,
        max_wait_ms: u64,
        cancel: &CancellationToken,
    ) -> Result<bool, AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        let previous = self.generation();
        let result =
            self.inner.client.fetch_snapshot(Some(previous), Some(max_wait_ms), cancel).await.map_err(broker_error)?;
        if let BrokerSnapshotResult::Snapshot { snapshot, generation } = result {
            self.apply_snapshot(snapshot, generation, true);
        }
        Ok(self.generation() != previous)
    }
    pub async fn prepare_for_request(&self, id: i64, cancel: &CancellationToken) -> Result<bool, AuthStorageError> {
        self.ensure_open()?;
        self.note_activity();
        let now = (self.inner.clock)();
        let should_wait = {
            let state = self.inner.state.lock().unwrap();
            state
                .snapshot
                .credentials
                .iter()
                .find(|entry| entry.id == id)
                .filter(|entry| matches!(entry.credential, AuthCredential::OAuth { .. }))
                .and_then(|entry| entry.rotates_in_ms)
                .is_some_and(|rotates| state.snapshot_received_at + rotates - now <= 1_000.0)
        };
        if should_wait { self.wait_for_fresh_snapshot(5_000, cancel).await } else { Ok(false) }
    }

    pub async fn refresh_oauth_credential(
        &self,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<StoredAuthCredential, AuthStorageError> {
        self.run_owned(cancel, move |owner, token| async move {
            let entry = owner.refresh_entry(id, &token).await?;
            if !owner.inner.state.lock().unwrap().streaming_active {
                let _ = owner.refresh_snapshot(&token).await;
            }
            let state = owner.inner.state.lock().unwrap();
            let current = state
                .snapshot
                .credentials
                .iter()
                .find(|candidate| candidate.id == entry.id)
                .ok_or(AuthStorageError::Unavailable)?;
            stored_row(current, state.broker_generation)
        })
        .await
    }
    pub async fn mark_credential_suspect(&self, id: i64, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        self.run_owned(cancel, move |owner, token| async move {
            owner.refresh_entry(id, &token).await?;
            owner.maybe_refresh_snapshot();
            Ok(())
        })
        .await
    }
    async fn refresh_entry(
        &self,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<BrokerSnapshotEntry, AuthStorageError> {
        let expected_provider = self
            .inner
            .state
            .lock()
            .unwrap()
            .snapshot
            .credentials
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| entry.provider.clone());
        let receipt = self.begin_receipt(RemoteOperation::Refresh { credential_id: id });
        let result = self.inner.client.refresh_credential(id, cancel).await;
        receipt.finish(&result);
        let mut entry = result.map_err(broker_error)?;
        if entry.id != id || expected_provider.as_ref().is_some_and(|provider| *provider != entry.provider) {
            return Err(AuthStorageError::Configuration);
        }
        let AuthCredential::OAuth { fields } = &mut entry.credential else {
            return Err(AuthStorageError::Configuration);
        };
        fields.insert("refresh".into(), Value::String(REMOTE_REFRESH_SENTINEL.into()));
        if !self.apply_credential_entry(entry.clone()) {
            return Err(AuthStorageError::Unavailable);
        }
        Ok(entry)
    }

    fn apply_credential_entry(&self, mut entry: BrokerSnapshotEntry) -> bool {
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        upsert_usage_entry(&mut state.raw_usage, &entry);
        let visible = in_account_pool(&entry, self.inner.account_pool.as_ref());
        if visible {
            entry.rotates_in_ms = None;
            let index = state.snapshot.credentials.iter().position(|candidate| candidate.id == entry.id);
            entry.blocks = index.map(|index| state.snapshot.credentials[index].blocks.clone()).unwrap_or_default();
            if let Some(index) = index {
                state.snapshot.credentials[index] = entry;
            } else {
                state.snapshot.credentials.push(entry);
            }
        } else {
            state.snapshot.credentials.retain(|candidate| candidate.id != entry.id);
        }
        self.inner.usage.replace_snapshot(state.raw_usage.clone());
        self.publish_projection(&mut state, &previous);
        visible
    }
    fn apply_provider_entries(&self, provider: &str, entries: Vec<BrokerSnapshotEntry>) {
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        let existing_blocks: BTreeMap<_, _> = state
            .snapshot
            .credentials
            .iter()
            .filter(|entry| entry.provider == provider)
            .map(|entry| (entry.id, entry.blocks.clone()))
            .collect();
        state.snapshot.credentials.retain(|entry| entry.provider != provider);
        state.raw_usage.retain(|entry| entry.provider != provider);
        for mut entry in entries {
            upsert_usage_entry(&mut state.raw_usage, &entry);
            if !in_account_pool(&entry, self.inner.account_pool.as_ref()) {
                continue;
            }
            entry.rotates_in_ms = None;
            entry.blocks = existing_blocks.get(&entry.id).cloned().unwrap_or_default();
            state.snapshot.credentials.push(entry);
        }
        self.inner.usage.replace_snapshot(state.raw_usage.clone());
        self.publish_projection(&mut state, &previous);
    }
    fn remove_credential(&self, id: i64) {
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        state.snapshot.credentials.retain(|entry| entry.id != id);
        state.raw_usage.retain(|entry| entry.id != id);
        self.inner.usage.replace_snapshot(state.raw_usage.clone());
        self.publish_projection(&mut state, &previous);
    }
    fn remove_provider(&self, provider: &str) {
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        state.snapshot.credentials.retain(|entry| entry.provider != provider);
        state.raw_usage.retain(|entry| entry.provider != provider);
        self.inner.usage.replace_snapshot(state.raw_usage.clone());
        self.publish_projection(&mut state, &previous);
    }

    pub async fn upsert_auth_credential_remote(
        &self,
        provider: &str,
        credential: &AuthCredential,
        cancel: &CancellationToken,
    ) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        let provider = provider.to_owned();
        let credential = credential.clone();
        self.run_owned(cancel, move |owner, token| async move {
            owner.upload(&provider, &credential, &token).await?;
            owner.maybe_refresh_snapshot();
            owner.rows(Some(&provider))
        })
        .await
    }
    async fn upload(
        &self,
        provider: &str,
        credential: &AuthCredential,
        cancel: &CancellationToken,
    ) -> Result<(), AuthStorageError> {
        let receipt = self.begin_receipt(RemoteOperation::Upload { provider: provider.into() });
        let result = self.inner.client.upload_credential(provider, credential, cancel).await;
        receipt.finish(&result);
        self.apply_provider_entries(provider, result.map_err(broker_error)?);
        Ok(())
    }
    pub async fn replace_auth_credentials_remote(
        &self,
        provider: &str,
        credentials: &[AuthCredential],
        cancel: &CancellationToken,
    ) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        let provider = provider.to_owned();
        let credentials = credentials.to_vec();
        self.run_owned(cancel, move |owner, token| async move {
            for row in owner.rows(Some(&provider))? {
                let result = owner.disable(row.id, "replaced by newer credential", &token).await;
                // Native continues known individual errors. Unknown effects
                // stop this sequence and preserve its current local rows.
                if result.as_ref().is_err_and(|error| error.is_outcome_unknown() || error.is_cancelled()) {
                    return Err(broker_error(result.err().unwrap()));
                }
            }
            owner.remove_provider(&provider);
            for credential in credentials {
                owner.upload(&provider, &credential, &token).await?;
            }
            owner.maybe_refresh_snapshot();
            owner.rows(Some(&provider))
        })
        .await
    }
    pub async fn delete_auth_credential_remote(
        &self,
        id: i64,
        cause: &str,
        cancel: &CancellationToken,
    ) -> Result<bool, AuthStorageError> {
        let cause = cause.to_owned();
        self.run_owned(cancel, move |owner, token| async move {
            if !owner.inner.state.lock().unwrap().snapshot.credentials.iter().any(|entry| entry.id == id) {
                return Ok(false);
            }
            owner.disable(id, &cause, &token).await.map_err(broker_error)?;
            owner.remove_credential(id);
            owner.maybe_refresh_snapshot();
            Ok(true)
        })
        .await
    }
    pub async fn delete_auth_credentials_remote(
        &self,
        provider: &str,
        cause: &str,
        cancel: &CancellationToken,
    ) -> Result<(), AuthStorageError> {
        let provider = provider.to_owned();
        let cause = cause.to_owned();
        self.run_owned(cancel, move |owner, token| async move {
            for row in owner.rows(Some(&provider))? {
                let result = owner.disable(row.id, &cause, &token).await;
                if result.as_ref().is_err_and(|error| error.is_outcome_unknown() || error.is_cancelled()) {
                    return Err(broker_error(result.err().unwrap()));
                }
            }
            owner.remove_provider(&provider);
            owner.maybe_refresh_snapshot();
            Ok(())
        })
        .await
    }
    async fn disable(&self, id: i64, cause: &str, cancel: &CancellationToken) -> Result<bool, BrokerError> {
        let receipt = self.begin_receipt(RemoteOperation::Disable { credential_id: id });
        let result = self.inner.client.disable_credential(id, cause, cancel).await;
        receipt.finish(&result);
        result
    }
    pub async fn list_disabled_credentials(
        &self,
        provider: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<DisabledCredentialSummary>, AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        self.inner.client.list_disabled_credentials(provider, cancel).await.map_err(broker_error)
    }

    fn rows(&self, provider: Option<&str>) -> Result<Vec<StoredAuthCredential>, AuthStorageError> {
        self.ensure_open()?;
        self.note_activity();
        let state = self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
        state
            .snapshot
            .credentials
            .iter()
            .filter(|entry| provider.is_none_or(|provider| entry.provider == provider))
            .map(|entry| stored_row(entry, state.broker_generation))
            .collect()
    }
    fn maybe_refresh_snapshot(&self) {
        if self.inner.state.lock().unwrap().streaming_active {
            return;
        }
        let Ok(finite) = self.finite_owner() else {
            return;
        };
        let owner = self.clone();
        self.inner.handle.spawn(async move {
            let _finite = finite;
            let _ = owner.refresh_snapshot(&CancellationToken::new()).await;
        });
    }
}

fn broker_error(error: BrokerError) -> AuthStorageError {
    if error.is_outcome_unknown() {
        AuthStorageError::OutcomeUnknown
    } else if error.is_cancelled() {
        AuthStorageError::Cancelled
    } else if matches!(
        error.kind,
        BrokerErrorKind::Configuration | BrokerErrorKind::InvalidJson | BrokerErrorKind::InvalidSchema
    ) || matches!(error.status, Some(401 | 403))
    {
        // Broker authentication and response/configuration errors do not
        // establish anything about the provider's OAuth grant.
        AuthStorageError::Configuration
    } else {
        // The fixed broker error envelope has no provider-grant rejection
        // receipt. An HTTP status alone must never trigger credential disable.
        AuthStorageError::Transient
    }
}

fn stored_row(entry: &BrokerSnapshotEntry, generation: i64) -> Result<StoredAuthCredential, AuthStorageError> {
    let serialized = serialize_credential(&entry.provider, &entry.credential).map_err(|_| AuthStorageError::Storage)?;
    Ok(StoredAuthCredential {
        id: entry.id,
        provider: entry.provider.clone(),
        credential: entry.credential.clone(),
        disabled_cause: None,
        serialized_data: serialized.data,
        revision: generation,
    })
}
fn same_projection(a: &BrokerSnapshot, b: &BrokerSnapshot) -> bool {
    a.generation == b.generation
        && a.generated_at == b.generated_at
        && a.server_now_ms == b.server_now_ms
        && a.refresher == b.refresher
        && a.credentials.len() == b.credentials.len()
        && a.credentials.iter().zip(&b.credentials).all(|(a, b)| {
            a.id == b.id
                && a.provider == b.provider
                && a.credential == b.credential
                && a.identity_key == b.identity_key
                && a.rotates_in_ms == b.rotates_in_ms
                && a.blocks == b.blocks
        })
}
fn in_account_pool(entry: &BrokerSnapshotEntry, pool: Option<&AuthBrokerAccountPool>) -> bool {
    if !matches!(entry.credential, AuthCredential::OAuth { .. }) {
        return true;
    }
    pool.and_then(|pool| pool.get(&entry.provider))
        .is_none_or(|identities| entry.identity_key.as_ref().is_some_and(|identity| identities.contains(identity)))
}
fn usage_entry(entry: &BrokerSnapshotEntry) -> AuthBrokerUsageSnapshotEntry {
    let field = |name: &str| match &entry.credential {
        AuthCredential::OAuth { fields } => fields.get(name).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    };
    AuthBrokerUsageSnapshotEntry {
        id: entry.id,
        provider: entry.provider.clone(),
        identity_key: entry.identity_key.clone(),
        credential_type: if matches!(entry.credential, AuthCredential::OAuth { .. }) {
            UsageCredentialType::Oauth
        } else {
            UsageCredentialType::ApiKey
        },
        identity: AuthBrokerUsageIdentity {
            account_id: field("accountId"),
            email: field("email"),
            project_id: field("projectId"),
            org_id: field("orgId"),
        },
    }
}
fn project_usage_entries(entries: &[BrokerSnapshotEntry]) -> Vec<AuthBrokerUsageSnapshotEntry> {
    let mut projected = Vec::new();
    for entry in entries {
        upsert_usage_entry(&mut projected, entry);
    }
    projected
}
fn upsert_usage_entry(entries: &mut Vec<AuthBrokerUsageSnapshotEntry>, entry: &BrokerSnapshotEntry) {
    if let Some(index) = entries.iter().position(|candidate| candidate.id == entry.id) {
        entries[index] = usage_entry(entry);
    } else {
        entries.push(usage_entry(entry));
    }
}
fn normalize_blocks(entry: &mut BrokerSnapshotEntry, now: f64) {
    entry.blocks.retain(|block| block.blocked_until_ms > now);
    entry.blocks.sort_by(compare_blocks);
}
fn compare_blocks(a: &BrokerBlock, b: &BrokerBlock) -> std::cmp::Ordering {
    a.provider_key
        .cmp(&b.provider_key)
        .then_with(|| a.block_scope.cmp(&b.block_scope))
        .then_with(|| a.blocked_until_ms.total_cmp(&b.blocked_until_ms))
        .then_with(|| a.updated_at_ms.unwrap_or(0.0).total_cmp(&b.updated_at_ms.unwrap_or(0.0)))
}
fn snapshot_blocks_changed(previous: &[BrokerSnapshotEntry], next: &[BrokerSnapshotEntry]) -> bool {
    let mut blocks: BTreeMap<_, _> = previous.iter().map(|entry| (entry.id, &entry.blocks)).collect();
    for entry in next {
        let previous = blocks.remove(&entry.id).map(Vec::as_slice).unwrap_or(&[]);
        if previous != entry.blocks.as_slice() {
            return true;
        }
    }
    blocks.values().any(|blocks| !blocks.is_empty())
}
fn protect_new_blocks(
    reconcile: &mut BTreeMap<(i64, String, String), f64>,
    previous: &[BrokerSnapshotEntry],
    next: &[BrokerSnapshotEntry],
    now: f64,
) {
    let previous: BTreeMap<_, _> = previous
        .iter()
        .flat_map(|entry| {
            entry
                .blocks
                .iter()
                .map(move |block| ((entry.id, block.provider_key.clone(), block.block_scope.clone()), block))
        })
        .collect();
    let mut active = BTreeSet::new();
    for entry in next {
        for block in &entry.blocks {
            let key = (entry.id, block.provider_key.clone(), block.block_scope.clone());
            active.insert(key.clone());
            if previous.get(&key).is_some_and(|old| {
                old.blocked_until_ms == block.blocked_until_ms && old.updated_at_ms == block.updated_at_ms
            }) {
                continue;
            }
            reconcile
                .insert(key, block.blocked_until_ms.min(block.updated_at_ms.unwrap_or(now) + BLOCK_RECONCILE_DELAY_MS));
        }
    }
    reconcile.retain(|key, _| active.contains(key));
}

struct FiniteOwner {
    lifecycle: Arc<RemoteLifecycle>,
}
impl FiniteOwner {
    fn new(lifecycle: Arc<RemoteLifecycle>) -> Self {
        lifecycle.pending.fetch_add(1, Ordering::AcqRel);
        Self { lifecycle }
    }
}
impl Drop for FiniteOwner {
    fn drop(&mut self) {
        self.lifecycle.pending.fetch_sub(1, Ordering::AcqRel);
        self.lifecycle.settled.notify_waiters();
    }
}
struct MutationReceipt {
    inner: Weak<RemoteInner>,
    id: u64,
    finished: bool,
}
impl MutationReceipt {
    fn finish<T>(mut self, result: &Result<T, BrokerError>) {
        if let Some(inner) = self.inner.upgrade() {
            let mut state = inner.state.lock().unwrap();
            if let Some(receipt) = state.receipts.iter_mut().find(|receipt| receipt.id == self.id) {
                receipt.outcome = match result {
                    Ok(_) => RemoteOperationOutcome::Succeeded,
                    Err(error) if error.is_outcome_unknown() => RemoteOperationOutcome::OutcomeUnknown,
                    Err(_) => RemoteOperationOutcome::Failed,
                };
                receipt.status = result.as_ref().err().and_then(|error| error.status);
            }
            // Bound routine diagnostics without ever evicting Pending or
            // OutcomeUnknown receipts that still require explicit review.
            while state
                .receipts
                .iter()
                .filter(|receipt| {
                    matches!(receipt.outcome, RemoteOperationOutcome::Succeeded | RemoteOperationOutcome::Failed)
                })
                .count()
                > 128
            {
                let index = state
                    .receipts
                    .iter()
                    .position(|receipt| {
                        matches!(receipt.outcome, RemoteOperationOutcome::Succeeded | RemoteOperationOutcome::Failed)
                    })
                    .unwrap();
                state.receipts.remove(index);
            }
        }
        self.finished = true;
    }
}
impl Drop for MutationReceipt {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(inner) = self.inner.upgrade()
            && let Ok(mut state) = inner.state.lock()
            && let Some(receipt) = state.receipts.iter_mut().find(|receipt| receipt.id == self.id)
        {
            receipt.outcome = RemoteOperationOutcome::OutcomeUnknown;
        }
    }
}

impl RemoteAuthCredentialStore {
    pub fn record_observed_usage(&self, entries: &[ClientUsageEntry], identity: Option<ClientUsageIdentity>) {
        let identity = identity.unwrap_or_else(|| self.inner.default_client_identity.clone());
        let timer = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closed || state.observed_usage_unsupported {
                return;
            }
            for entry in entries {
                merge_observed(&mut state.observed_usage, &identity, entry);
            }
            if state.observed_usage.is_empty() || state.observed_timer.is_some() {
                return;
            }
            state.next_timer = state.next_timer.wrapping_add(1);
            let id = state.next_timer;
            let cancel = CancellationToken::new();
            state.observed_timer = Some((id, cancel.clone()));
            (id, cancel, FiniteOwner::new(self.inner.lifecycle.clone()))
        };
        let weak = Arc::downgrade(&self.inner);
        let interval = self.inner.observed_usage_flush;
        self.inner.handle.spawn(async move {
            let (id, cancel, finite) = timer;
            let _finite = finite;
            tokio::select! { biased; _ = cancel.cancelled() => return, _ = tokio::time::sleep(interval) => {} }
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let batch = {
                let mut state = inner.state.lock().unwrap();
                if state.observed_timer.as_ref().is_none_or(|(current, _)| *current != id) {
                    return;
                }
                state.observed_timer = None;
                if state.closed || state.observed_usage_unsupported {
                    return;
                }
                std::mem::take(&mut state.observed_usage)
            };
            let owner = RemoteAuthCredentialStore { inner };
            let _ = owner.flush_observed_batch(batch).await;
        });
    }

    /// Explicit flush is useful to Hosts at a bounded request boundary. It is
    /// owned independently of the caller, just like the scheduled native flush.
    pub async fn flush_observed_usage(&self, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        self.run_owned(cancel, |owner, _| async move {
            let batch = {
                let mut state = owner.inner.state.lock().unwrap();
                if let Some((_, timer)) = state.observed_timer.take() {
                    timer.cancel();
                }
                if state.observed_usage_unsupported {
                    return Ok(());
                }
                std::mem::take(&mut state.observed_usage)
            };
            owner.flush_observed_batch(batch).await
        })
        .await
    }

    async fn flush_observed_batch(&self, batch: Vec<PendingObservedUsage>) -> Result<(), AuthStorageError> {
        let mut groups: Vec<ClientUsageReport> = Vec::new();
        for pending in batch {
            if let Some(group) = groups.iter_mut().find(|group| {
                group.install_id == pending.identity.install_id
                    && group.app.as_deref().unwrap_or("") == pending.identity.app.as_deref().unwrap_or("")
            }) {
                group.entries.push(pending.entry);
            } else {
                groups.push(ClientUsageReport {
                    install_id: pending.identity.install_id,
                    hostname: pending.identity.hostname,
                    app: pending.identity.app,
                    entries: vec![pending.entry],
                });
            }
        }
        let mut outcome = Ok(());
        for report in groups {
            if self.inner.state.lock().unwrap().observed_usage_unsupported {
                break;
            }
            let receipt = self.begin_receipt(RemoteOperation::ObservedUsage);
            let receipt_id = receipt.id;
            let mut body = serde_json::to_value(&report).map_err(|_| AuthStorageError::Configuration)?;
            // JS omits undefined optional identity properties. The existing
            // local report struct serializes None as null, which is forbidden
            // by the broker's optional string schema at this boundary.
            let fields = body.as_object_mut().ok_or(AuthStorageError::Configuration)?;
            if report.hostname.is_none() {
                fields.remove("hostname");
            }
            if report.app.is_none() {
                fields.remove("app");
            }
            let result = self.inner.client.report_client_usage(&body, &CancellationToken::new()).await;
            receipt.finish(&result);
            if let Err(error) = result {
                if matches!(error.status, Some(400 | 404 | 501)) {
                    let mut state = self.inner.state.lock().unwrap();
                    state.observed_usage_unsupported = true;
                    state.observed_usage.clear();
                    if let Some((_, timer)) = state.observed_timer.take() {
                        timer.cancel();
                    }
                    return Err(broker_error(error));
                }
                if error.is_outcome_unknown() {
                    // Incremental effects cannot be inferred from a post-state
                    // snapshot. Preserve this exact batch and do not replay it.
                    self.inner.state.lock().unwrap().unknown_observed.push(UnknownObservedUsageBatch {
                        receipt_id,
                        report,
                        status: error.status,
                    });
                    outcome = Err(AuthStorageError::OutcomeUnknown);
                    continue;
                }
                if !self.inner.state.lock().unwrap().closed {
                    self.record_observed_usage(
                        &report.entries,
                        Some(ClientUsageIdentity {
                            install_id: report.install_id,
                            hostname: report.hostname,
                            app: report.app,
                        }),
                    );
                }
                if outcome.is_ok() {
                    outcome = Err(broker_error(error));
                }
            }
        }
        outcome
    }

    pub fn close(&self) {
        let final_flush = {
            let mut state = self.inner.state.lock().unwrap();
            if state.closed {
                return;
            }
            state.closed = true;
            state.streaming_active = false;
            if let Some((_, timer)) = state.observed_timer.take() {
                timer.cancel();
            }
            state.cache.clear();
            self.inner.usage.clear_ephemeral_state();
            let batch = std::mem::take(&mut state.observed_usage);
            if batch.is_empty() || state.observed_usage_unsupported {
                None
            } else {
                Some((batch, FiniteOwner::new(self.inner.lifecycle.clone())))
            }
        };
        self.inner.lifecycle.background_abort.cancel();
        self.inner.lifecycle.activity.notify_waiters();
        if let Some((batch, finite)) = final_flush {
            let owner = self.clone();
            self.inner.handle.spawn(async move {
                let _finite = finite;
                let _ = owner.flush_observed_batch(batch).await;
            });
        }
    }

    /// Waits for finite request/write/report owners; an open background stream
    /// is deliberately excluded. Unknown receipts remain observable afterward.
    pub async fn wait_for_settlement(&self) {
        loop {
            let notified = self.inner.lifecycle.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.lifecycle.pending.load(Ordering::Acquire) == 0 {
                self.inner.usage.wait_for_settlement().await;
                if self.inner.lifecycle.pending.load(Ordering::Acquire) == 0 {
                    return;
                }
            }
            notified.await;
        }
    }
    pub async fn close_and_wait(&self) {
        self.close();
        loop {
            let notified = self.inner.lifecycle.background_settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.inner.lifecycle.background_done.load(Ordering::Acquire) {
                break;
            }
            notified.await;
        }
        self.wait_for_settlement().await;
    }

    fn spawn_write<F, Fut>(&self, operation: RemoteOperation, followup: bool, work: F) -> Result<(), AuthStorageError>
    where
        F: FnOnce(AuthBrokerClient, CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = Result<bool, BrokerError>> + Send + 'static,
    {
        let finite = self.finite_owner()?;
        let receipt = self.begin_receipt(operation);
        let owner = self.clone();
        self.inner.handle.spawn(async move {
            let _finite = finite;
            let result = work(owner.inner.client.clone(), CancellationToken::new()).await;
            let succeeded = result.is_ok();
            receipt.finish(&result);
            if succeeded && followup {
                owner.maybe_refresh_snapshot();
            }
        });
        Ok(())
    }

    fn project_credential_block(&self, block: &StoredCredentialBlock) -> Result<BrokerBlock, AuthStorageError> {
        self.ensure_open()?;
        self.note_activity();
        let now = (self.inner.clock)();
        let body = BrokerBlock {
            provider_key: block.provider_key.clone(),
            block_scope: block.block_scope.clone(),
            blocked_until_ms: block.blocked_until_ms as f64,
            updated_at_ms: Some(block.updated_at_ms as f64),
        };
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        if let Some(entry) = state.snapshot.credentials.iter_mut().find(|entry| entry.id == block.credential_id) {
            if let Some(old) = entry
                .blocks
                .iter_mut()
                .find(|old| old.provider_key == body.provider_key && old.block_scope == body.block_scope)
            {
                old.blocked_until_ms = old.blocked_until_ms.max(body.blocked_until_ms);
            } else {
                entry.blocks.push(body.clone());
            }
            entry.blocks.sort_by(compare_blocks);
        }
        state.reconcile_after.insert(
            (block.credential_id, block.provider_key.clone(), block.block_scope.clone()),
            (block.blocked_until_ms as f64).min(now + BLOCK_RECONCILE_DELAY_MS),
        );
        self.inner.usage.invalidate_usage_cache();
        self.publish_projection(&mut state, &previous);
        Ok(body)
    }

    fn spawn_observed_block_write(
        &self,
        id: i64,
        body: BrokerBlock,
    ) -> Result<CredentialBlockSettlement, AuthStorageError> {
        let finite = self.finite_owner()?;
        let receipt = self.begin_receipt(RemoteOperation::UpsertBlock { credential_id: id });
        let owner = self.clone();
        let (done, completion) = oneshot::channel();
        self.inner.handle.spawn(async move {
            let _finite = finite;
            let result = owner.inner.client.upsert_credential_block(id, &body, &CancellationToken::new()).await;
            let confirmed = matches!(result, Ok(true));
            let succeeded = result.is_ok();
            receipt.finish(&result);
            if succeeded {
                owner.maybe_refresh_snapshot();
            }
            let _ = done.send(confirmed);
        });
        Ok(CredentialBlockSettlement::pending(completion))
    }
    fn prune_blocks(&self, now: f64) {
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        for entry in &mut state.snapshot.credentials {
            entry.blocks.retain(|block| block.blocked_until_ms > now);
        }
        state.reconcile_after.retain(|_, until| *until > now);
        self.publish_projection(&mut state, &previous);
    }
}

fn merge_observed(pending: &mut Vec<PendingObservedUsage>, identity: &ClientUsageIdentity, entry: &ClientUsageEntry) {
    if let Some(previous) = pending.iter_mut().find(|previous| {
        previous.identity.install_id == identity.install_id
            && previous.identity.app.as_deref().unwrap_or("") == identity.app.as_deref().unwrap_or("")
            && previous.entry.provider == entry.provider
            && previous.entry.model == entry.model
    }) {
        let previous = &mut previous.entry;
        previous.at = previous.at.max(entry.at);
        previous.requests = previous.requests.saturating_add(entry.requests);
        previous.input_tokens = previous.input_tokens.saturating_add(entry.input_tokens);
        previous.output_tokens = previous.output_tokens.saturating_add(entry.output_tokens);
        previous.cache_read_tokens = previous.cache_read_tokens.saturating_add(entry.cache_read_tokens);
        previous.cache_write_tokens = previous.cache_write_tokens.saturating_add(entry.cache_write_tokens);
        previous.cost_usd += entry.cost_usd;
    } else {
        pending.push(PendingObservedUsage { identity: identity.clone(), entry: entry.clone() });
    }
}

impl AuthCredentialStore for RemoteAuthCredentialStore {
    fn list_auth_credentials(&self, provider: Option<&str>) -> anyhow::Result<Vec<StoredAuthCredential>> {
        self.rows(provider).map_err(Into::into)
    }
    fn list_disabled_credentials(&self, _provider: Option<&str>) -> anyhow::Result<Vec<DisabledCredentialSummary>> {
        self.ensure_open()?;
        Ok(Vec::new())
    }
    fn update_auth_credential(&self, id: i64, credential: &AuthCredential) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.note_activity();
        let mut state = self.inner.state.lock().unwrap();
        let previous = state.snapshot.clone();
        if let Some(entry) = state.snapshot.credentials.iter_mut().find(|entry| entry.id == id) {
            entry.credential = credential.clone();
        }
        self.publish_projection(&mut state, &previous);
        Ok(())
    }
    fn delete_auth_credential(&self, id: i64, cause: &str) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.note_activity();
        self.remove_credential(id);
        let cause = cause.to_owned();
        self.spawn_write(RemoteOperation::Disable { credential_id: id }, false, move |client, token| async move {
            client.disable_credential(id, &cause, &token).await
        })?;
        Ok(())
    }
    fn try_disable_auth_credential_if_matches(
        &self,
        id: i64,
        _expected_data: &str,
        cause: &str,
        _lease: Option<&CredentialRefreshLeaseFence>,
    ) -> anyhow::Result<bool> {
        self.ensure_open()?;
        let found = self.inner.state.lock().unwrap().snapshot.credentials.iter().any(|entry| entry.id == id);
        if found {
            AuthCredentialStore::delete_auth_credential(self, id, cause)?;
        }
        Ok(found)
    }
    fn replace_auth_credentials_for_provider(
        &self,
        _provider: &str,
        _credentials: &[AuthCredential],
    ) -> anyhow::Result<Vec<StoredAuthCredential>> {
        anyhow::bail!("remote credentials require the asynchronous broker write interface")
    }
    fn upsert_auth_credential_for_provider(
        &self,
        _provider: &str,
        _credential: &AuthCredential,
    ) -> anyhow::Result<Vec<StoredAuthCredential>> {
        anyhow::bail!("remote credentials require the asynchronous broker write interface")
    }
    fn delete_auth_credentials_for_provider(&self, _provider: &str, _cause: &str) -> anyhow::Result<()> {
        anyhow::bail!("remote credentials require the asynchronous broker write interface")
    }
    fn get_cache(&self, key: &str, _include_expired: bool) -> anyhow::Result<Option<String>> {
        self.ensure_open()?;
        self.note_activity();
        let now = (self.inner.clock)();
        let mut state = self.inner.state.lock().unwrap();
        // Native remote cache never exposes an expired row, including the
        // broader local store's includeExpired option.
        if state.cache.get(key).is_some_and(|entry| entry.expires_at_sec as f64 * 1_000.0 <= now) {
            state.cache.remove(key);
        }
        Ok(state.cache.get(key).map(|entry| entry.value.clone()))
    }
    fn set_cache(&self, key: &str, value: &str, expires_at_sec: i64) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.note_activity();
        self.inner.state.lock().unwrap().cache.insert(key.into(), CacheEntry { value: value.into(), expires_at_sec });
        Ok(())
    }
    fn delete_cache_prefix(&self, prefix: &str) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.inner.state.lock().unwrap().cache.retain(|key, _| !key.starts_with(prefix));
        Ok(())
    }
    fn clean_expired_cache(&self) -> anyhow::Result<()> {
        self.ensure_open()?;
        let now = (self.inner.clock)() / 1_000.0;
        self.inner.state.lock().unwrap().cache.retain(|_, entry| entry.expires_at_sec as f64 > now.floor());
        Ok(())
    }
    fn get_credential_block(&self, id: i64, provider: &str, scope: &str) -> anyhow::Result<Option<i64>> {
        self.ensure_open()?;
        self.note_activity();
        let now = (self.inner.clock)();
        self.prune_blocks(now);
        let state = self.inner.state.lock().unwrap();
        Ok(state
            .snapshot
            .credentials
            .iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| {
                entry.blocks.iter().find(|block| {
                    block.provider_key == provider && block.block_scope == scope && block.blocked_until_ms > now
                })
            })
            .map(|block| block.blocked_until_ms as i64))
    }
    fn get_credential_block_reconcile_after(
        &self,
        id: i64,
        provider: &str,
        scope: &str,
    ) -> anyhow::Result<Option<i64>> {
        if AuthCredentialStore::get_credential_block(self, id, provider, scope)?.is_none() {
            return Ok(None);
        }
        Ok(self
            .inner
            .state
            .lock()
            .unwrap()
            .reconcile_after
            .get(&(id, provider.into(), scope.into()))
            .map(|until| *until as i64))
    }
    fn upsert_credential_block(&self, block: &StoredCredentialBlock) -> anyhow::Result<()> {
        let body = self.project_credential_block(block)?;
        let id = block.credential_id;
        self.spawn_write(RemoteOperation::UpsertBlock { credential_id: id }, true, move |client, token| async move {
            client.upsert_credential_block(id, &body, &token).await
        })?;
        Ok(())
    }
    fn upsert_credential_block_observed(
        &self,
        block: &StoredCredentialBlock,
    ) -> anyhow::Result<CredentialBlockSettlement> {
        let body = self.project_credential_block(block)?;
        self.spawn_observed_block_write(block.credential_id, body).map_err(Into::into)
    }
    fn delete_credential_block(&self, _id: i64, _provider: &str, _scope: &str) -> anyhow::Result<()> {
        self.ensure_open()?;
        // Fixed native no-op: deleting all would erase unrelated/newer scopes.
        Ok(())
    }
    fn delete_credential_blocks(&self, id: i64) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.note_activity();
        {
            let mut state = self.inner.state.lock().unwrap();
            let previous = state.snapshot.clone();
            if let Some(entry) = state.snapshot.credentials.iter_mut().find(|entry| entry.id == id) {
                entry.blocks.clear();
            }
            state.reconcile_after.retain(|(credential, _, _), _| *credential != id);
            self.inner.usage.invalidate_usage_cache();
            self.publish_projection(&mut state, &previous);
        }
        self.spawn_write(RemoteOperation::DeleteBlocks { credential_id: id }, true, move |client, token| async move {
            client.delete_credential_blocks(id, &token).await
        })?;
        Ok(())
    }
    fn clean_expired_credential_blocks(&self, now: i64) -> anyhow::Result<()> {
        self.ensure_open()?;
        self.prune_blocks(now as f64);
        Ok(())
    }
    fn list_credential_blocks(&self, ids: &[i64]) -> anyhow::Result<Vec<StoredCredentialBlock>> {
        self.ensure_open()?;
        self.note_activity();
        let now = (self.inner.clock)();
        self.prune_blocks(now);
        let state = self.inner.state.lock().unwrap();
        let mut blocks: Vec<_> = state
            .snapshot
            .credentials
            .iter()
            .filter(|entry| ids.contains(&entry.id))
            .flat_map(|entry| {
                entry.blocks.iter().map(move |block| StoredCredentialBlock {
                    credential_id: entry.id,
                    provider_key: block.provider_key.clone(),
                    block_scope: block.block_scope.clone(),
                    blocked_until_ms: block.blocked_until_ms as i64,
                    updated_at_ms: block.updated_at_ms.unwrap_or(0.0) as i64,
                })
            })
            .collect();
        blocks.sort_by(|a, b| {
            a.credential_id
                .cmp(&b.credential_id)
                .then_with(|| a.provider_key.cmp(&b.provider_key))
                .then_with(|| a.block_scope.cmp(&b.block_scope))
                .then_with(|| a.blocked_until_ms.cmp(&b.blocked_until_ms))
                .then_with(|| a.updated_at_ms.cmp(&b.updated_at_ms))
        });
        Ok(blocks)
    }
    fn record_usage_snapshots(&self, _entries: &[UsageHistoryEntry]) -> anyhow::Result<()> {
        self.ensure_open()?;
        Ok(())
    }
    fn list_usage_history(&self, _query: Option<&UsageHistoryQuery>) -> anyhow::Result<Vec<UsageHistoryEntry>> {
        self.ensure_open()?;
        Ok(Vec::new())
    }
    fn poll_external_changes(&self) -> anyhow::Result<bool> {
        self.ensure_open()?;
        Ok(false)
    }
    fn acknowledge_local_changes(&self) -> anyhow::Result<()> {
        self.ensure_open()?;
        Ok(())
    }
}

#[async_trait]
impl AggregateUsageSource for RemoteAuthCredentialStore {
    async fn fetch_usage_reports(&self) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        self.inner.usage.fetch_usage_reports().await
    }
    async fn wait_for_settlement(&self) {
        RemoteAuthCredentialStore::wait_for_settlement(self).await;
    }
}
#[async_trait]
impl UsageStoreHooks for RemoteAuthCredentialStore {
    async fn get_usage_report(
        &self,
        request: UsageRequest,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        self.inner.usage.get_usage_report(request, cancel).await
    }
    fn ingest_usage_report(&self, request: &UsageRequest, report: UsageReport) -> bool {
        if self.ensure_open().is_err() {
            return false;
        }
        self.note_activity();
        self.inner.usage.ingest_usage_report(request, report)
    }
    fn invalidate_usage_cache(&self) {
        if self.ensure_open().is_ok() {
            self.note_activity();
            self.inner.usage.invalidate_usage_cache();
        }
    }
    async fn notify_usage_stale(&self, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        let _finite = self.finite_owner()?;
        self.note_activity();
        self.inner.usage.notify_usage_stale(cancel).await
    }
}

impl Drop for RemoteInner {
    fn drop(&mut self) {
        // No network from Drop. Background/timer futures own Weak<RemoteInner>.
        self.lifecycle.background_abort.cancel();
        self.lifecycle.activity.notify_waiters();
        if let Ok(state) = self.state.get_mut()
            && let Some((_, timer)) = state.observed_timer.take()
        {
            timer.cancel();
        }
        self.usage.clear_ephemeral_state();
    }
}

enum BackgroundResult<T> {
    Done(Result<T, BrokerError>),
    Idle,
    Closed,
}

fn idle_remaining(weak: &Weak<RemoteInner>) -> Option<Duration> {
    let inner = weak.upgrade()?;
    let now = (inner.clock)();
    let state = inner.state.lock().ok()?;
    if state.closed {
        return None;
    }
    let remaining = state.last_activity_ms + inner.background_idle.as_secs_f64() * 1_000.0 - now;
    Some(Duration::from_secs_f64((remaining.max(0.0) / 1_000.0).min(86_400.0)))
}

async fn with_idle_guard<T>(
    weak: &Weak<RemoteInner>,
    lifecycle: &RemoteLifecycle,
    cancel: &CancellationToken,
    future: impl Future<Output = Result<T, BrokerError>>,
) -> BackgroundResult<T> {
    tokio::pin!(future);
    loop {
        let activity = lifecycle.activity.notified();
        tokio::pin!(activity);
        activity.as_mut().enable();
        let Some(remaining) = idle_remaining(weak) else {
            cancel.cancel();
            return BackgroundResult::Closed;
        };
        if remaining.is_zero() {
            cancel.cancel();
            return BackgroundResult::Idle;
        }
        tokio::select! {
            biased;
            _ = lifecycle.background_abort.cancelled() => { cancel.cancel(); return BackgroundResult::Closed; },
            result = &mut future => return BackgroundResult::Done(result),
            _ = &mut activity => {},
            _ = tokio::time::sleep(remaining) => {},
        }
    }
}

async fn consume_stream(
    weak: Weak<RemoteInner>,
    client: AuthBrokerClient,
    cancel: CancellationToken,
) -> Result<(), BrokerError> {
    let mut stream = client.open_snapshot_stream(&cancel).await?;
    while let Some(event) = stream.next_event().await? {
        let Some(inner) = weak.upgrade() else {
            break;
        };
        {
            let mut state = inner.state.lock().unwrap();
            if state.closed || cancel.is_cancelled() {
                break;
            }
            state.streaming_active = true;
        }
        RemoteAuthCredentialStore { inner }.apply_stream_event(event);
    }
    Ok(())
}

async fn run_background(weak: Weak<RemoteInner>, lifecycle: Arc<RemoteLifecycle>) {
    let _completion = BackgroundCompletion { weak: weak.clone(), lifecycle: lifecycle.clone() };
    let mut backoff = Duration::from_millis(500);
    loop {
        if lifecycle.background_abort.is_cancelled() {
            break;
        }
        let activity = lifecycle.activity.notified();
        tokio::pin!(activity);
        activity.as_mut().enable();
        let Some(remaining) = idle_remaining(&weak) else {
            break;
        };
        if remaining.is_zero() {
            if let Some(inner) = weak.upgrade() {
                inner.state.lock().unwrap().parked = true;
            }
            tokio::select! { biased; _ = lifecycle.background_abort.cancelled() => break, _ = &mut activity => {} }
            continue;
        }
        let Some((client, stream_enabled, generation)) = weak.upgrade().map(|inner| {
            let mut state = inner.state.lock().unwrap();
            state.parked = false;
            (inner.client.clone(), inner.stream_snapshots && !state.streaming_unsupported, state.broker_generation)
        }) else {
            break;
        };
        let cancel = lifecycle.background_abort.child_token();
        let outcome = if stream_enabled {
            with_idle_guard(&weak, &lifecycle, &cancel, consume_stream(weak.clone(), client, cancel.clone())).await
        } else {
            match with_idle_guard(
                &weak,
                &lifecycle,
                &cancel,
                client.fetch_snapshot(Some(generation), Some(BACKGROUND_WAIT_MS), &cancel),
            )
            .await
            {
                BackgroundResult::Done(Ok(result)) => {
                    if let BrokerSnapshotResult::Snapshot { snapshot, generation } = result
                        && let Some(inner) = weak.upgrade()
                    {
                        RemoteAuthCredentialStore { inner }.apply_snapshot(snapshot, generation, true);
                    }
                    BackgroundResult::Done(Ok(()))
                }
                BackgroundResult::Done(Err(error)) => BackgroundResult::Done(Err(error)),
                BackgroundResult::Idle => BackgroundResult::Idle,
                BackgroundResult::Closed => BackgroundResult::Closed,
            }
        };
        if let Some(inner) = weak.upgrade() {
            inner.state.lock().unwrap().streaming_active = false;
        }
        match outcome {
            BackgroundResult::Closed => break,
            BackgroundResult::Idle => continue,
            BackgroundResult::Done(Ok(())) => {
                backoff = Duration::from_millis(500);
                continue;
            }
            BackgroundResult::Done(Err(error)) if error.is_stream_unsupported() => {
                if let Some(inner) = weak.upgrade() {
                    inner.state.lock().unwrap().streaming_unsupported = true;
                }
                continue;
            }
            BackgroundResult::Done(Err(_)) => {}
        }
        // Do not retain a strong store owner across either parking or backoff.
        let Some(remaining) = idle_remaining(&weak) else {
            break;
        };
        tokio::select! { biased; _ = lifecycle.background_abort.cancelled() => break, _ = tokio::time::sleep(backoff.min(remaining)) => {} }
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

struct BackgroundCompletion {
    weak: Weak<RemoteInner>,
    lifecycle: Arc<RemoteLifecycle>,
}
impl Drop for BackgroundCompletion {
    fn drop(&mut self) {
        if let Some(inner) = self.weak.upgrade() {
            let mut state = inner.state.lock().unwrap();
            state.streaming_active = false;
            state.parked = false;
        }
        self.lifecycle.background_done.store(true, Ordering::Release);
        self.lifecycle.background_settled.notify_waiters();
    }
}

#[cfg(test)]
#[path = "auth_broker_store_tests.rs"]
mod tests;
