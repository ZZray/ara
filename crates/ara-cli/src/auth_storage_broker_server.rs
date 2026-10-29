//! Fixed OMP Broker host facade. Storage identity and dispatched refresh settlement
//! remain owned by AuthStorage; transport owns only its own waiters.

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

use super::*;
use crate::auth_broker_mcp_refresh::ManagedMcpBrokerRefresh;
use crate::auth_broker_wire::BrokerBlock;
use crate::credential_store::{ClientUsageReport, ClientUsageSummary, resolve_credential_identity_key};
use std::sync::Weak;

#[derive(Clone)]
pub struct BrokerCredentialEntry {
    pub id: i64,
    pub provider: String,
    pub credential: AuthCredential,
    pub identity_key: Option<String>,
}
#[derive(Clone)]
pub struct BrokerCredentialSnapshot {
    pub generation: u64,
    pub generated_at_ms: f64,
    pub credentials: Vec<BrokerCredentialEntry>,
    pub blocks: Vec<StoredCredentialBlock>,
}
#[derive(Clone, Debug)]
pub enum BrokerStorageError {
    MissingCredential(i64),
    NotOAuth { id: i64, provider: String, credential_type: &'static str },
    Auth(AuthStorageError),
}
impl std::fmt::Display for BrokerStorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingCredential(id) => write!(f, "No credential with id={id}"),
            Self::NotOAuth { id, provider, credential_type } => {
                write!(f, "Credential {id} is not OAuth (provider={provider}, type={credential_type})")
            }
            Self::Auth(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for BrokerStorageError {}
impl From<AuthStorageError> for BrokerStorageError {
    fn from(error: AuthStorageError) -> Self {
        Self::Auth(error)
    }
}
fn redacted(row: &StoredAuthCredential) -> BrokerCredentialEntry {
    let mut credential = row.credential.clone();
    if let AuthCredential::OAuth { fields } = &mut credential {
        fields.insert("refresh".into(), Value::String("__remote__".into()));
    }
    BrokerCredentialEntry {
        id: row.id,
        provider: row.provider.clone(),
        credential,
        identity_key: resolve_credential_identity_key(&row.provider, &row.credential),
    }
}
#[derive(Clone)]
pub struct BrokerStorageFacade {
    storage: Arc<AuthStorage>,
    managed_mcp: Option<Arc<ManagedMcpBrokerRefresh>>,
}
impl BrokerStorageFacade {
    pub fn new(storage: Arc<AuthStorage>) -> Self {
        Self { storage, managed_mcp: None }
    }
    pub(crate) fn with_managed_mcp(mut self, client: reqwest::Client) -> Self {
        if let CredentialStoreOwner::Local(store) = &self.storage.inner.store {
            self.managed_mcp = Some(Arc::new(ManagedMcpBrokerRefresh::new(
                store.clone(),
                client,
                self.storage.inner.options.clock.clone(),
            )));
        }
        self
    }
    pub fn storage(&self) -> &Arc<AuthStorage> {
        &self.storage
    }
    pub fn generation(&self) -> u64 {
        self.storage.generation()
    }
    pub fn subscribe_generation(&self) -> BrokerGenerationSubscription {
        self.subscribe_generation_from(self.generation())
    }
    pub(crate) fn subscribe_generation_from(&self, seen: u64) -> BrokerGenerationSubscription {
        BrokerGenerationSubscription { inner: Arc::downgrade(&self.storage.inner), seen }
    }
    pub async fn reload(&self) -> Result<(), BrokerStorageError> {
        let storage = self.storage.clone();
        tokio::task::spawn_blocking(move || storage.reload()).await.map_err(|_| AuthStorageError::Storage)??;
        Ok(())
    }
    pub async fn poll_external_changes(&self) -> Result<bool, BrokerStorageError> {
        self.storage
            .database(|store, state| {
                if !store.poll_external_changes().map_err(|_| AuthStorageError::Storage)? {
                    return Ok(false);
                }
                AuthStorage::reload_state_from_store(store, state)?;
                // A block-only revision still owns one publication. An unrelated
                // generation bump before this operation cannot acknowledge it.
                state.generation_changed = true;
                Ok(true)
            })
            .await
            .map_err(Into::into)
    }
    pub fn export_snapshot(&self) -> Result<BrokerCredentialSnapshot, BrokerStorageError> {
        self.storage
            .inner
            .store
            .with_store(|store| {
                let mut state = self.storage.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
                let credentials = state
                    .stored_provider_order
                    .iter()
                    .flat_map(|provider| state.rows.get(provider).into_iter().flatten())
                    .map(redacted)
                    .collect::<Vec<_>>();
                let ids = credentials.iter().map(|entry| entry.id).collect::<Vec<_>>();
                let blocks =
                    state.assignments.list_credential_blocks(store, &ids).map_err(|_| AuthStorageError::Storage)?;
                Ok(BrokerCredentialSnapshot {
                    generation: self.storage.inner.generation.load(Ordering::Acquire),
                    generated_at_ms: self.storage.now(),
                    credentials,
                    blocks,
                })
            })
            .map_err(Into::into)
    }
    async fn observed_row(&self, id: i64) -> Result<StoredAuthCredential, BrokerStorageError> {
        self.reload().await?;
        let state = self.storage.inner.state.lock().map_err(|_| AuthStorageError::Storage)?;
        state.rows.values().flatten().find(|row| row.id == id).cloned().ok_or(BrokerStorageError::MissingCredential(id))
    }
    pub async fn force_refresh(
        &self,
        id: i64,
        waiter: &CancellationToken,
    ) -> Result<BrokerCredentialEntry, BrokerStorageError> {
        let facade = self.clone();
        let row = self
            .storage
            .shared_oauth_refresh(id, waiter, move |storage| async move {
                storage.bump_generation();
                let mut attempted = None;
                let mut installed_owner = false;
                let result = async {
                    let observed = facade.observed_row(id).await?;
                    if !matches!(observed.credential, AuthCredential::OAuth { .. }) {
                        return Err(BrokerStorageError::NotOAuth {
                            id,
                            provider: observed.provider,
                            credential_type: observed.credential.credential_type(),
                        });
                    }
                    attempted = Some(observed.clone());
                    let refreshed = if storage.inner.store.remote().is_some()
                        || storage.inner.options.oauth_refresh_override.is_some()
                    {
                        storage
                            .perform_owned_remote_refresh(
                                &observed.provider,
                                id,
                                OAuthRefreshPublication::BrokerExplicit,
                            )
                            .await?
                    } else if ManagedMcpBrokerRefresh::accepts(&observed.provider) && facade.managed_mcp.is_some() {
                        let result = facade
                            .managed_mcp
                            .as_ref()
                            .ok_or(AuthStorageError::Unsupported)?
                            .refresh(observed.clone())
                            .await;
                        if matches!(&result, Err(AuthStorageError::Unavailable)) {
                            // A known expired ACK may already have committed the new
                            // grant. Publish that confirmed row before the failure bump.
                            storage.reload()?;
                        }
                        result?
                    } else if storage.oauth_hook(&observed.provider).is_some() {
                        // This closure already owns the per-ID slot. Calling the
                        // shared request entry here would wait on its own flight.
                        installed_owner = true;
                        storage
                            .perform_owned_installed_oauth(
                                &observed.provider,
                                observed.clone(),
                                true,
                                OAuthRefreshPublication::BrokerExplicit,
                            )
                            .await?
                    } else {
                        storage
                            .resolve_exact_oauth_with_publication(
                                &observed.provider,
                                observed.clone(),
                                true,
                                OAuthRefreshPublication::BrokerExplicit,
                                &CancellationToken::new(),
                            )
                            .await?
                            .row
                    };
                    storage.reload()?;
                    storage
                        .provider_rows(&refreshed.provider)
                        .await?
                        .into_iter()
                        .find(|row| row.id == id)
                        .ok_or(BrokerStorageError::MissingCredential(id))
                }
                .await;
                if let Err(error) = &result {
                    if matches!(error, BrokerStorageError::Auth(AuthStorageError::Definitive))
                        && !installed_owner
                        && let Some(observed) = attempted
                    {
                        let _ = storage
                            .handle_oauth_failure(&observed.provider, &observed, AuthStorageError::Definitive, None)
                            .await;
                    }
                    storage.bump_generation();
                }
                result
            })
            .await?;
        Ok(redacted(&row))
    }
    pub async fn disable(&self, id: i64, cause: &str, waiter: &CancellationToken) -> Result<bool, BrokerStorageError> {
        check_cancel(waiter)?;
        let row = match self.observed_row(id).await {
            Ok(row) => row,
            Err(BrokerStorageError::MissingCredential(_)) => return Ok(false),
            Err(error) => return Err(error),
        };
        if let Some(remote) = self.storage.inner.store.remote() {
            let changed = remote.delete_auth_credential_remote(id, cause, waiter).await?;
            self.reload().await?;
            return Ok(changed);
        }
        let provider = row.provider.clone();
        let cause = cause.to_owned();
        self.storage
            .database(move |store, state| {
                store.delete_auth_credential(id, &cause).map_err(|_| AuthStorageError::Storage)?;
                AuthStorage::load_provider(store, state, &provider)?;
                state.assignments.reset_provider_assignments(store, &provider);
                Ok(())
            })
            .await?;
        Ok(true)
    }
    pub async fn upsert(
        &self,
        provider: &str,
        credential: AuthCredential,
        waiter: &CancellationToken,
    ) -> Result<Vec<BrokerCredentialEntry>, BrokerStorageError> {
        check_cancel(waiter)?;
        if let Some(remote) = self.storage.inner.store.remote() {
            let rows = remote.upsert_auth_credential_remote(provider, &credential, waiter).await?;
            self.reload().await?;
            return Ok(rows.iter().map(redacted).collect());
        }
        let provider = provider.to_owned();
        let rows = self
            .storage
            .database(move |store, state| {
                store
                    .upsert_auth_credential_for_provider(&provider, &credential)
                    .map_err(|_| AuthStorageError::Storage)?;
                let rows = AuthStorage::load_provider(store, state, &provider)?;
                state.assignments.reset_provider_assignments(store, &provider);
                Ok(rows)
            })
            .await?;
        Ok(rows.iter().map(redacted).collect())
    }
    pub(crate) async fn migration_rows(&self) -> Result<Vec<StoredAuthCredential>, BrokerStorageError> {
        Ok(self
            .storage
            .database(|store, _| store.list_auth_credentials(None).map_err(|_| AuthStorageError::Storage))
            .await?)
    }
    pub(crate) async fn logout_provider(
        &self,
        provider: &str,
        waiter: &CancellationToken,
    ) -> Result<(), BrokerStorageError> {
        check_cancel(waiter)?;
        let provider = provider.to_owned();
        self.storage
            .database(move |store, state| {
                store
                    .delete_auth_credentials_for_provider(&provider, "logged out by user")
                    .map_err(|_| AuthStorageError::Storage)?;
                AuthStorage::load_provider(store, state, &provider)?;
                state.assignments.reset_provider_assignments(store, &provider);
                Ok(())
            })
            .await?;
        Ok(())
    }
    pub fn list_blocks(&self, ids: &[i64]) -> Result<Vec<StoredCredentialBlock>, BrokerStorageError> {
        Ok(self.storage.list_credential_blocks(ids)?)
    }
    pub fn upsert_block(&self, id: i64, block: BrokerBlock) -> Result<(), BrokerStorageError> {
        if !self.export_snapshot()?.credentials.iter().any(|entry| entry.id == id) {
            return Err(BrokerStorageError::MissingCredential(id));
        }
        self.storage.upsert_credential_block(&StoredCredentialBlock {
            credential_id: id,
            provider_key: block.provider_key,
            block_scope: block.block_scope,
            blocked_until_ms: block.blocked_until_ms as i64,
            updated_at_ms: block.updated_at_ms.unwrap_or_else(|| self.storage.now()) as i64,
        })?;
        Ok(())
    }
    pub fn delete_blocks(&self, id: i64) -> Result<(), BrokerStorageError> {
        if !self.export_snapshot()?.credentials.iter().any(|entry| entry.id == id) {
            return Err(BrokerStorageError::MissingCredential(id));
        }
        self.storage.delete_credential_blocks(id)?;
        Ok(())
    }
    pub(crate) fn local_usage(&self) -> Option<LocalBrokerUsageFacade> {
        match &self.storage.inner.store {
            CredentialStoreOwner::Local(store) => Some(LocalBrokerUsageFacade(store.clone())),
            CredentialStoreOwner::Remote(_) => None,
        }
    }
}
pub struct BrokerGenerationSubscription {
    inner: Weak<AuthStorageInner>,
    seen: u64,
}
impl BrokerGenerationSubscription {
    pub async fn changed(&mut self, stop: &CancellationToken) -> Result<u64, BrokerStorageError> {
        loop {
            let inner = self.inner.upgrade().ok_or(AuthStorageError::Cancelled)?;
            let notified = inner.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let generation = inner.generation.load(Ordering::Acquire);
            if generation > self.seen {
                self.seen = generation;
                return Ok(generation);
            }
            tokio::select! { biased; _ = stop.cancelled() => return Err(AuthStorageError::Cancelled.into()), _ = notified => {} }
        }
    }
}
#[derive(Clone)]
pub(crate) struct LocalBrokerUsageFacade(Arc<Mutex<SqliteCredentialStore>>);
impl LocalBrokerUsageFacade {
    pub(crate) async fn record(&self, report: ClientUsageReport) -> Result<(), AuthStorageError> {
        let store = self.0.clone();
        tokio::task::spawn_blocking(move || {
            store
                .lock()
                .map_err(|_| AuthStorageError::Storage)?
                .record_client_usage(&report)
                .map_err(|_| AuthStorageError::Storage)
        })
        .await
        .map_err(|_| AuthStorageError::Storage)?
    }
    pub(crate) async fn summary(&self, since_ms: i64) -> Result<ClientUsageSummary, AuthStorageError> {
        let store = self.0.clone();
        tokio::task::spawn_blocking(move || {
            store
                .lock()
                .map_err(|_| AuthStorageError::Storage)?
                .get_client_usage_summary(since_ms)
                .map_err(|_| AuthStorageError::Storage)
        })
        .await
        .map_err(|_| AuthStorageError::Storage)?
    }
}
