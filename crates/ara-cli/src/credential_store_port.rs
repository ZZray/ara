//! Synchronous credential projection consumed by the Host AuthStorage.
//!
//! Fixed OMP `packages/ai/src/auth-storage.ts:383-568` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Local SQLite remains the
//! exact shared owner used by Codex CAS/leases; a remote owner has no disk mirror.
//!
//! MIT License
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

use crate::auth_broker_store::RemoteAuthCredentialStore;
use crate::auth_storage::AuthStorageError;
use crate::credential_store::{
    AuthCredential, CredentialRefreshLeaseFence, DisabledCredentialSummary, SqliteCredentialStore,
    StoredAuthCredential, StoredCredentialBlock, UsageHistoryEntry, UsageHistoryQuery,
};
use anyhow::Result;
use std::sync::{Arc, Mutex};

/// Short synchronous store operations. Network capabilities are obtained from
/// the same owner only after these operations and Host locks have finished.
/// SQLite is Send, not Sync; callers borrow it under its original mutex.
pub trait AuthCredentialStore: Send {
    fn list_auth_credentials(&self, provider: Option<&str>) -> Result<Vec<StoredAuthCredential>>;
    fn list_disabled_credentials(&self, provider: Option<&str>) -> Result<Vec<DisabledCredentialSummary>>;
    fn update_auth_credential(&self, id: i64, item: &AuthCredential) -> Result<()>;
    fn delete_auth_credential(&self, id: i64, cause: &str) -> Result<()>;
    fn try_disable_auth_credential_if_matches(
        &self,
        id: i64,
        expected_data: &str,
        cause: &str,
        lease: Option<&CredentialRefreshLeaseFence>,
    ) -> Result<bool>;
    fn replace_auth_credentials_for_provider(
        &self,
        provider: &str,
        credentials: &[AuthCredential],
    ) -> Result<Vec<StoredAuthCredential>>;
    fn upsert_auth_credential_for_provider(
        &self,
        provider: &str,
        credential: &AuthCredential,
    ) -> Result<Vec<StoredAuthCredential>>;
    fn delete_auth_credentials_for_provider(&self, provider: &str, cause: &str) -> Result<()>;
    fn get_cache(&self, key: &str, include_expired: bool) -> Result<Option<String>>;
    fn set_cache(&self, key: &str, value: &str, expires_at_sec: i64) -> Result<()>;
    fn delete_cache_prefix(&self, prefix: &str) -> Result<()>;
    fn clean_expired_cache(&self) -> Result<()>;
    fn get_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>>;
    fn get_credential_block_reconcile_after(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>>;
    fn upsert_credential_block(&self, block: &StoredCredentialBlock) -> Result<()>;
    fn delete_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<()>;
    fn delete_credential_blocks(&self, id: i64) -> Result<()>;
    fn clean_expired_credential_blocks(&self, now: i64) -> Result<()>;
    fn list_credential_blocks(&self, ids: &[i64]) -> Result<Vec<StoredCredentialBlock>>;
    fn record_usage_snapshots(&self, entries: &[UsageHistoryEntry]) -> Result<()>;
    fn list_usage_history(&self, query: Option<&UsageHistoryQuery>) -> Result<Vec<UsageHistoryEntry>>;
    fn poll_external_changes(&self) -> Result<bool>;
    fn acknowledge_local_changes(&self) -> Result<()>;
}

impl AuthCredentialStore for SqliteCredentialStore {
    fn list_auth_credentials(&self, provider: Option<&str>) -> Result<Vec<StoredAuthCredential>> {
        SqliteCredentialStore::list_auth_credentials(self, provider)
    }
    fn list_disabled_credentials(&self, provider: Option<&str>) -> Result<Vec<DisabledCredentialSummary>> {
        SqliteCredentialStore::list_disabled_credentials(self, provider)
    }
    fn update_auth_credential(&self, id: i64, item: &AuthCredential) -> Result<()> {
        SqliteCredentialStore::update_auth_credential(self, id, item)
    }
    fn delete_auth_credential(&self, id: i64, cause: &str) -> Result<()> {
        SqliteCredentialStore::delete_auth_credential(self, id, cause)
    }
    fn try_disable_auth_credential_if_matches(
        &self,
        id: i64,
        expected_data: &str,
        cause: &str,
        lease: Option<&CredentialRefreshLeaseFence>,
    ) -> Result<bool> {
        SqliteCredentialStore::try_disable_auth_credential_if_matches(self, id, expected_data, cause, lease)
    }
    fn replace_auth_credentials_for_provider(
        &self,
        provider: &str,
        credentials: &[AuthCredential],
    ) -> Result<Vec<StoredAuthCredential>> {
        SqliteCredentialStore::replace_auth_credentials_for_provider(self, provider, credentials)
    }
    fn upsert_auth_credential_for_provider(
        &self,
        provider: &str,
        credential: &AuthCredential,
    ) -> Result<Vec<StoredAuthCredential>> {
        SqliteCredentialStore::upsert_auth_credential_for_provider(self, provider, credential)
    }
    fn delete_auth_credentials_for_provider(&self, provider: &str, cause: &str) -> Result<()> {
        SqliteCredentialStore::delete_auth_credentials_for_provider(self, provider, cause)
    }
    fn get_cache(&self, key: &str, include_expired: bool) -> Result<Option<String>> {
        SqliteCredentialStore::get_cache(self, key, include_expired)
    }
    fn set_cache(&self, key: &str, value: &str, expires_at_sec: i64) -> Result<()> {
        SqliteCredentialStore::set_cache(self, key, value, expires_at_sec)
    }
    fn delete_cache_prefix(&self, prefix: &str) -> Result<()> {
        SqliteCredentialStore::delete_cache_prefix(self, prefix)
    }
    fn clean_expired_cache(&self) -> Result<()> {
        SqliteCredentialStore::clean_expired_cache(self)
    }
    fn get_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>> {
        SqliteCredentialStore::get_credential_block(self, id, provider, scope)
    }
    fn get_credential_block_reconcile_after(&self, id: i64, provider: &str, scope: &str) -> Result<Option<i64>> {
        SqliteCredentialStore::get_credential_block_reconcile_after(self, id, provider, scope)
    }
    fn upsert_credential_block(&self, block: &StoredCredentialBlock) -> Result<()> {
        SqliteCredentialStore::upsert_credential_block(self, block)
    }
    fn delete_credential_block(&self, id: i64, provider: &str, scope: &str) -> Result<()> {
        SqliteCredentialStore::delete_credential_block(self, id, provider, scope)
    }
    fn delete_credential_blocks(&self, id: i64) -> Result<()> {
        SqliteCredentialStore::delete_credential_blocks(self, id)
    }
    fn clean_expired_credential_blocks(&self, now: i64) -> Result<()> {
        SqliteCredentialStore::clean_expired_credential_blocks(self, now)
    }
    fn list_credential_blocks(&self, ids: &[i64]) -> Result<Vec<StoredCredentialBlock>> {
        SqliteCredentialStore::list_credential_blocks(self, ids)
    }
    fn record_usage_snapshots(&self, entries: &[UsageHistoryEntry]) -> Result<()> {
        SqliteCredentialStore::record_usage_snapshots(self, entries)
    }
    fn list_usage_history(&self, query: Option<&UsageHistoryQuery>) -> Result<Vec<UsageHistoryEntry>> {
        SqliteCredentialStore::list_usage_history(self, query)
    }
    fn poll_external_changes(&self) -> Result<bool> {
        SqliteCredentialStore::poll_external_changes(self)
    }
    fn acknowledge_local_changes(&self) -> Result<()> {
        SqliteCredentialStore::acknowledge_local_changes(self)
    }
}

/// Keep the concrete local Arc, preserving the existing Codex ownership check.
/// Remote capabilities all derive from this one shared remote Arc.
#[derive(Clone)]
pub(crate) enum CredentialStoreOwner {
    Local(Arc<Mutex<SqliteCredentialStore>>),
    Remote(Arc<RemoteAuthCredentialStore>),
}

impl CredentialStoreOwner {
    #[cfg(test)]
    pub(crate) fn local_for_test(&self) -> &Arc<Mutex<SqliteCredentialStore>> {
        match self {
            Self::Local(store) => store,
            Self::Remote(_) => panic!("local SQLite fixture required"),
        }
    }
    pub(crate) fn remote(&self) -> Option<Arc<RemoteAuthCredentialStore>> {
        match self {
            Self::Local(_) => None,
            Self::Remote(store) => Some(store.clone()),
        }
    }
    pub(crate) fn with_store<T>(
        &self,
        operation: impl FnOnce(&dyn AuthCredentialStore) -> Result<T, AuthStorageError>,
    ) -> Result<T, AuthStorageError> {
        match self {
            Self::Local(store) => {
                let guard = store.lock().map_err(|_| AuthStorageError::Storage)?;
                operation(&*guard)
            }
            Self::Remote(store) => operation(store.as_ref()),
        }
    }
}
