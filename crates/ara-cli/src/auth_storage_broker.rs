//! Credential write and observer consumers for the fixed OMP store hooks.
//! Source: packages/ai/src/auth-storage.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (2453-2480, 2736-2783,
//! 3576-3614, 6888). This child shares its parent's one store owner.
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

use super::*;
use crate::auth_broker_store::ClientUsageIdentity;
use crate::credential_store::{ClientUsageEntry, DisabledCredentialSummary, UsageHistoryQuery};

impl AuthStorage {
    /// Replace a provider's credentials. Remote persistence completes before
    /// the Host updates its selected rows and assignments.
    pub async fn set(
        &self,
        provider: &str,
        credentials: &[AuthCredential],
        cancel: &CancellationToken,
    ) -> Result<(), AuthStorageError> {
        check_cancel(cancel)?;
        // Native keeps the last OAuth row for each resolved account identity,
        // while preserving API keys and OAuth rows without an identity.
        let mut seen = std::collections::BTreeSet::new();
        let mut credentials = credentials
            .iter()
            .rev()
            .filter(|credential| {
                !matches!(credential, AuthCredential::OAuth { .. })
                    || crate::credential_store::resolve_credential_identity_key(provider, credential)
                        .is_none_or(|identity| seen.insert(identity))
            })
            .cloned()
            .collect::<Vec<_>>();
        credentials.reverse();
        if let Some(remote) = self.inner.store.remote() {
            remote.replace_auth_credentials_remote(provider, &credentials, cancel).await?;
        } else {
            let provider = provider.to_owned();
            self.database(move |store, _state| {
                store
                    .replace_auth_credentials_for_provider(&provider, &credentials)
                    .map(|_| ())
                    .map_err(|_| AuthStorageError::Storage)
            })
            .await?;
        }
        self.finish_credential_write(provider).await
    }

    pub async fn upsert_oauth_credential(
        &self,
        provider: &str,
        credential: &AuthCredential,
        cancel: &CancellationToken,
    ) -> Result<(), AuthStorageError> {
        check_cancel(cancel)?;
        if !matches!(credential, AuthCredential::OAuth { .. }) {
            return Err(AuthStorageError::Configuration);
        }
        if let Some(remote) = self.inner.store.remote() {
            remote.upsert_auth_credential_remote(provider, credential, cancel).await?;
        } else {
            let provider = provider.to_owned();
            let credential = credential.clone();
            self.database(move |store, _state| {
                store
                    .upsert_auth_credential_for_provider(&provider, &credential)
                    .map(|_| ())
                    .map_err(|_| AuthStorageError::Storage)
            })
            .await?;
        }
        self.finish_credential_write(provider).await
    }

    pub async fn remove(&self, provider: &str, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        check_cancel(cancel)?;
        if let Some(remote) = self.inner.store.remote() {
            remote.delete_auth_credentials_remote(provider, "deleted by user", cancel).await?;
        } else {
            let provider = provider.to_owned();
            self.database(move |store, _state| {
                store
                    .delete_auth_credentials_for_provider(&provider, "deleted by user")
                    .map_err(|_| AuthStorageError::Storage)
            })
            .await?;
        }
        self.finish_credential_write(provider).await
    }

    pub async fn remove_credential(
        &self,
        provider: &str,
        id: i64,
        cancel: &CancellationToken,
    ) -> Result<bool, AuthStorageError> {
        check_cancel(cancel)?;
        if !self.provider_rows(provider).await?.iter().any(|row| row.id == id) {
            return Ok(false);
        }
        if let Some(remote) = self.inner.store.remote() {
            if !remote.delete_auth_credential_remote(id, "deleted by user", cancel).await? {
                return Ok(false);
            }
        } else {
            self.database(move |store, _state| {
                store.delete_auth_credential(id, "deleted by user").map_err(|_| AuthStorageError::Storage)
            })
            .await?;
        }
        self.finish_credential_write(provider).await?;
        Ok(true)
    }

    async fn finish_credential_write(&self, provider: &str) -> Result<(), AuthStorageError> {
        let provider = provider.to_owned();
        self.database(move |store, state| {
            Self::load_provider(store, state, &provider)?;
            state.assignments.reset_provider_assignments(store, &provider);
            Ok(())
        })
        .await?;
        self.bump_generation();
        Ok(())
    }

    /// Revalidate a remote snapshot before identity-sensitive diagnostics.
    pub async fn refresh_credential_snapshot(&self, cancel: &CancellationToken) -> Result<(), AuthStorageError> {
        check_cancel(cancel)?;
        if let Some(remote) = self.inner.store.remote() {
            remote.refresh_snapshot(cancel).await?;
        }
        self.reload()
    }

    pub async fn list_disabled_credentials(
        &self,
        provider: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<Vec<DisabledCredentialSummary>, AuthStorageError> {
        check_cancel(cancel)?;
        if let Some(remote) = self.inner.store.remote() {
            return remote.list_disabled_credentials(provider, cancel).await;
        }
        let provider = provider.map(str::to_owned);
        self.database(move |store, _state| {
            store.list_disabled_credentials(provider.as_deref()).map_err(|_| AuthStorageError::Storage)
        })
        .await
    }

    /// Native synchronous history is absent on a remote store. Broker-wide
    /// history remains available through the client's explicit history endpoint.
    pub fn list_usage_history(
        &self,
        query: Option<&UsageHistoryQuery>,
    ) -> Result<Vec<crate::credential_store::UsageHistoryEntry>, AuthStorageError> {
        self.store_operation(|store, _state| store.list_usage_history(query).map_err(|_| AuthStorageError::Storage))
    }

    pub fn record_observed_usage(&self, entries: &[ClientUsageEntry], client: Option<ClientUsageIdentity>) {
        if let Some(remote) = self.inner.store.remote() {
            remote.record_observed_usage(entries, client);
        }
    }

    pub fn close_remote_store(&self) {
        if let Some(remote) = self.inner.store.remote() {
            remote.close();
        }
    }

    /// A normal Host exit stops the background owner and attempts its final
    /// observed flush, then drains already-dispatched finite request owners.
    pub async fn close_and_wait(&self) {
        self.close_remote_store();
        self.wait_for_settlement().await;
        if let Some(remote) = self.inner.store.remote() {
            remote.close_and_wait().await;
        }
        self.inner.remote_observer_cancel.cancel();
        let observer = self.inner.remote_observer.lock().ok().and_then(|mut observer| observer.take());
        if let Some(observer) = observer {
            let _ = observer.await;
        }
    }
}
