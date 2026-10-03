//! Registry binding to the same Host AuthStorage used by request Sessions.
//! Fixed OMP model-registry.ts / auth-storage.ts at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Discovery owns its private rejected bearer; catalog models never own leases.

use crate::{
    auth_storage::{AuthRequestContext, AuthStorage, Environment, EnvironmentKey},
    catalog_discovery::DiscoveryError,
    model_registry::RegistryCredentials,
    model_registry_extensions::OpaqueExtensionHandle,
    model_registry_runtime::RegistryRuntimeResult,
    model_route::RequestAuthLease,
    provider_models::{OpenAiCodexAccount, ProviderEnvironment, get_catalog_provider_entry},
};
use ara_rpc::WireString;
use async_trait::async_trait;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

pub struct CatalogAuthEnvironment(pub Arc<dyn ProviderEnvironment>);
impl Environment for CatalogAuthEnvironment {
    fn api_key(&self, provider: &str) -> Option<EnvironmentKey> {
        get_catalog_provider_entry(&provider.into())?.env_vars.as_ref()?.iter().find_map(|name| {
            let name = name.to_utf8().ok()?;
            let value = self.0.get(&name)?.to_utf8().ok()?;
            (!value.is_empty()).then_some(EnvironmentKey { variable: name, value })
        })
    }
}

pub struct AuthStorageRegistryCredentials {
    storage: AuthStorage,
    cancel: CancellationToken,
    // Keyed by both provider and rejected bearer so parallel discovery cannot
    // refresh whichever account happened to resolve most recently.
    leases: Mutex<HashMap<(String, String), RequestAuthLease>>,
}
impl AuthStorageRegistryCredentials {
    pub fn new(storage: AuthStorage, cancel: CancellationToken) -> Self {
        Self { storage, cancel, leases: Mutex::new(HashMap::new()) }
    }
    fn provider(provider: &WireString) -> RegistryRuntimeResult<String> {
        provider.to_utf8().map_err(|_| DiscoveryError::new("authentication provider is not UTF-8"))
    }
}
#[async_trait]
impl RegistryCredentials for AuthStorageRegistryCredentials {
    fn has_auth(&self, provider: &WireString) -> bool {
        Self::provider(provider).is_ok_and(|provider| self.storage.has_auth(&provider).unwrap_or(false))
    }
    fn has_oauth_credentials(&self, provider: &WireString) -> bool {
        Self::provider(provider)
            .is_ok_and(|provider| self.storage.list_oauth_accounts(&provider, None).is_ok_and(|rows| !rows.is_empty()))
    }
    fn oauth_credential(&self, provider: &WireString) -> Option<OpaqueExtensionHandle> {
        let provider = Self::provider(provider).ok()?;
        self.storage
            .stored_oauth_snapshot(&provider)
            .ok()?
            .into_iter()
            .next()
            .map(|row| Arc::new(row.credential) as OpaqueExtensionHandle)
    }
    async fn peek_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        self.storage
            .peek_api_key(&Self::provider(provider)?, None)
            .map(|key| key.map(Into::into))
            .map_err(|_| DiscoveryError::new("authentication peek failed"))
    }
    async fn resolve_key(&self, provider: &WireString) -> RegistryRuntimeResult<Option<WireString>> {
        let provider = Self::provider(provider)?;
        let Some(access) = self
            .storage
            .resolve(&provider, &AuthRequestContext::default(), &self.cancel)
            .await
            .map_err(|_| DiscoveryError::new("authentication resolution failed"))?
        else {
            return Ok(None);
        };
        let Some(key) = access.lease.api_key().map(str::to_owned) else { return Ok(None) };
        self.leases
            .lock()
            .map_err(|_| DiscoveryError::new("authentication receipt failed"))?
            .insert((provider, key.clone()), access.lease);
        Ok(Some(key.into()))
    }
    async fn refresh_rejected_key(
        &self,
        provider: &WireString,
        rejected: &WireString,
    ) -> RegistryRuntimeResult<Option<WireString>> {
        let provider = Self::provider(provider)?;
        let rejected = rejected.to_utf8().map_err(|_| DiscoveryError::new("authentication bearer is not UTF-8"))?;
        let failed = self
            .leases
            .lock()
            .map_err(|_| DiscoveryError::new("authentication receipt failed"))?
            .get(&(provider.clone(), rejected))
            .cloned();
        let Some(failed) = failed else { return Ok(None) };
        let next = self
            .storage
            .refresh_failed_lease(&provider, &AuthRequestContext::default(), &failed, &self.cancel)
            .await
            .map_err(|_| DiscoveryError::new("authentication refresh failed"))?;
        let Some(next) = next else { return Ok(None) };
        let Some(key) = next.lease.api_key().map(str::to_owned) else { return Ok(None) };
        self.leases
            .lock()
            .map_err(|_| DiscoveryError::new("authentication receipt failed"))?
            .insert((provider, key.clone()), next.lease);
        Ok(Some(key.into()))
    }
    async fn codex_accounts(
        &self,
        resolved_access_token: WireString,
        suppress_stored_oauth: bool,
    ) -> RegistryRuntimeResult<Option<Vec<OpenAiCodexAccount>>> {
        let mut accounts = Vec::new();
        if !suppress_stored_oauth {
            let access = self
                .storage
                .get_oauth_accesses("openai-codex", &AuthRequestContext::default(), &self.cancel)
                .await
                .map_err(|_| DiscoveryError::new("account discovery resolution failed"))?;
            for access in access {
                let Ok(access) = access else { return Ok(None) };
                let Some(oauth) = access.oauth else { return Ok(None) };
                accounts.push(OpenAiCodexAccount {
                    access_token: oauth.access_token.into(),
                    account_id: oauth.account_id.map(Into::into),
                });
            }
        }
        if !accounts.iter().any(|account| account.access_token == resolved_access_token) {
            // A runtime/config bearer must not acquire an unrelated stored
            // account identity merely because catalog discovery also has rows.
            let identity = if suppress_stored_oauth {
                None
            } else {
                self.storage.stored_oauth_snapshot("openai-codex").ok().into_iter().flatten().find_map(|row| {
                    let crate::credential_store::AuthCredential::OAuth { fields } = row.credential else { return None };
                    (fields.get("access")?.as_str()? == resolved_access_token.to_utf8().ok()?)
                        .then(|| fields.get("accountId").and_then(serde_json::Value::as_str).map(Into::into))
                        .flatten()
                })
            };
            accounts.push(OpenAiCodexAccount { access_token: resolved_access_token, account_id: identity });
        }
        Ok(Some(accounts))
    }
}
