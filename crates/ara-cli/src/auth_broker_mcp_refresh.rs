//! Fixed OMP managed MCP Broker refresh. Native standard-field persistence is
//! intentionally retained: descriptor material is lost after the first refresh.

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

use crate::auth_storage::AuthStorageError;
use crate::credential_store::{
    AuthCredential, CredentialRefreshLeaseFence, SqliteCredentialStore, StoredAuthCredential,
};
use ring::digest::{SHA256, digest};
use serde_json::{Map, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct ManagedMcpBrokerRefresh {
    store: Arc<Mutex<SqliteCredentialStore>>,
    client: reqwest::Client,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}
impl ManagedMcpBrokerRefresh {
    pub(crate) fn new(
        store: Arc<Mutex<SqliteCredentialStore>>,
        client: reqwest::Client,
        clock: Arc<dyn Fn() -> f64 + Send + Sync>,
    ) -> Self {
        Self { store, client, clock }
    }
    pub(crate) fn accepts(provider: &str) -> bool {
        provider.starts_with("mcp_oauth:") || provider.starts_with("mcp_oauth_")
    }
    async fn db<T: Send + 'static>(
        &self,
        op: impl FnOnce(&SqliteCredentialStore) -> Result<T, AuthStorageError> + Send + 'static,
    ) -> Result<T, AuthStorageError> {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            let guard = store.lock().map_err(|_| AuthStorageError::Storage)?;
            op(&guard)
        })
        .await
        .map_err(|_| AuthStorageError::Storage)?
    }
    pub(crate) async fn refresh(
        &self,
        observed: StoredAuthCredential,
    ) -> Result<StoredAuthCredential, AuthStorageError> {
        if !Self::accepts(&observed.provider) {
            return Err(AuthStorageError::Unsupported);
        }
        let AuthCredential::OAuth { fields } = &observed.credential else {
            return Err(AuthStorageError::Configuration);
        };
        let get = |name: &str| fields.get(name).and_then(Value::as_str);
        let refresh = get("refresh")
            .filter(|v| !v.is_empty() && *v != "__remote__")
            .ok_or(AuthStorageError::Configuration)?
            .to_owned();
        let token_url = get("tokenUrl").filter(|v| !v.is_empty()).ok_or(AuthStorageError::Configuration)?.to_owned();
        let fallback = server_url(&observed.provider);
        let advertised = get("resource");
        let resource = resource_indicator(
            advertised.or(fallback.as_deref()),
            get("authorizationUrl").unwrap_or(&token_url),
            !advertised.is_some_and(|v| !v.is_empty()) && fallback.is_some(),
        )?;
        let mut form =
            vec![("grant_type".to_owned(), "refresh_token".to_owned()), ("refresh_token".to_owned(), refresh.clone())];
        if let Some(id) = get("clientId").map(ara_prompt::js::trim).filter(|v| !v.is_empty()) {
            form.push(("client_id".into(), id.into()));
        }
        if let Some(secret) = get("clientSecret").filter(|v| !v.is_empty()) {
            form.push(("client_secret".into(), secret.into()));
        }
        if let Some(resource) = resource {
            form.push(("resource".into(), resource));
        }
        // Build before taking a lease or persisting a dispatch marker. A malformed
        // URL/form is a proven zero-dispatch configuration failure and can be retried.
        let request = self
            .client
            .post(&token_url)
            .form(&form)
            .timeout(Duration::from_secs(255))
            .build()
            .map_err(|_| AuthStorageError::Configuration)?;
        let fingerprint = digest(&SHA256, format!("{}\0{}\0{}", observed.provider, observed.id, refresh).as_bytes());
        let key = format!(
            "oauth_refresh_dispatch:managed-mcp:{}:{}",
            observed.id,
            fingerprint.as_ref().iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        let owner = uuid::Uuid::new_v4().to_string();
        let started = tokio::time::Instant::now();
        loop {
            let row = observed.clone();
            let key = key.clone();
            let owner = owner.clone();
            let now = (self.clock)() as i64;
            let acquired = self
                .db(move |store| {
                    let current = store
                        .list_auth_credentials(Some(&row.provider))
                        .map_err(|_| AuthStorageError::Storage)?
                        .into_iter()
                        .find(|r| r.id == row.id)
                        .ok_or(AuthStorageError::Unavailable)?;
                    if current.serialized_data != row.serialized_data {
                        return Ok(Some(Err(current)));
                    }
                    if store.get_cache(&key, true).map_err(|_| AuthStorageError::Storage)?.is_some() {
                        return Err(AuthStorageError::OutcomeUnknown);
                    }
                    if !store
                        .try_acquire_credential_refresh_lease(row.id, &owner, now + 300_000)
                        .map_err(|_| AuthStorageError::Storage)?
                    {
                        return Ok(None);
                    }
                    let lease = CredentialRefreshLeaseFence { owner: owner.clone(), now_ms: now };
                    if !store
                        .try_begin_managed_mcp_refresh(row.id, &row.provider, &row.serialized_data, &key, &lease)
                        .map_err(|_| AuthStorageError::Storage)?
                    {
                        store
                            .release_credential_refresh_lease(row.id, &owner)
                            .map_err(|_| AuthStorageError::Storage)?;
                        return Err(AuthStorageError::OutcomeUnknown);
                    }
                    Ok(Some(Ok(())))
                })
                .await?;
            match acquired {
                Some(Ok(())) => break,
                Some(Err(peer)) => return Ok(peer),
                None => {}
            }
            if started.elapsed() >= Duration::from_secs(300) {
                return Err(AuthStorageError::Transient);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let outcome = async {
            let response = match self.client.execute(request).await {
                Ok(response) => response,
                Err(failure) if failure.is_builder() => {
                    // Reqwest rejects an unsupported request scheme before network
                    // dispatch. Clear only this owner's marker; other errors can have
                    // submitted a rotating grant and must retain the durable fence.
                    let key = key.clone();
                    let owner = owner.clone();
                    self.db(move |store| {
                        store.clear_oauth_refresh_dispatch(&key, &owner).map_err(|_| AuthStorageError::Storage)
                    })
                    .await?;
                    return Err(AuthStorageError::Configuration);
                }
                Err(_) => return Err(AuthStorageError::OutcomeUnknown),
            };
            let status = response.status();
            let bytes = response.bytes().await.map_err(|_| AuthStorageError::OutcomeUnknown)?;
            if !status.is_success() {
                let definitive = definitive_failure(status.as_u16(), &String::from_utf8_lossy(&bytes));
                let row = observed.clone();
                let key = key.clone();
                let owner = owner.clone();
                let now = (self.clock)() as i64;
                self.db(move |store| {
                    if definitive {
                        store
                            .try_disable_auth_credential_if_matches(
                                row.id,
                                &row.serialized_data,
                                "oauth refresh failed",
                                Some(&CredentialRefreshLeaseFence { owner: owner.clone(), now_ms: now }),
                            )
                            .map_err(|_| AuthStorageError::Storage)?;
                    }
                    store.clear_oauth_refresh_dispatch(&key, &owner).map_err(|_| AuthStorageError::Storage)
                })
                .await?;
                return Err(if definitive { AuthStorageError::Definitive } else { AuthStorageError::Transient });
            }
            let value: Value = serde_json::from_slice(&bytes).map_err(|_| AuthStorageError::OutcomeUnknown)?;
            let access = value
                .get("access_token")
                .and_then(Value::as_str)
                .filter(|v| !v.is_empty())
                .ok_or(AuthStorageError::OutcomeUnknown)?;
            let new_refresh = match value.get("refresh_token") {
                None | Some(Value::Null) => refresh.clone(),
                Some(Value::String(v)) => v.clone(),
                _ => return Err(AuthStorageError::OutcomeUnknown),
            };
            let expires_in = match value.get("expires_in") {
                None | Some(Value::Null) => 3600.0,
                Some(v) => v.as_f64().filter(|v| v.is_finite()).ok_or(AuthStorageError::OutcomeUnknown)?,
            };
            let mut updated = Map::new();
            // Fresh explicit force projects only Native standard fields. A known
            // expired success persists the inner merge instead (2686-2707), because
            // the usable check then rejects it before outer projection (5359-5379).
            for field in
                ["accountId", "email", "projectId", "enterpriseUrl", "apiEndpoint", "orgId", "orgName", "authorizedAt"]
            {
                if let Some(value) = fields.get(field) {
                    updated.insert(field.into(), value.clone());
                }
            }
            let issued_at = (self.clock)();
            let expires = issued_at + expires_in * 1000.0;
            // Non-finite arithmetic is malformed success; finite but already
            // expired ACKs below are known token replacements, not unknown grants.
            if !expires.is_finite() {
                return Err(AuthStorageError::OutcomeUnknown);
            }
            updated.insert("access".into(), Value::String(access.into()));
            updated.insert("refresh".into(), Value::String(new_refresh));
            updated.insert("expires".into(), serde_json::json!(expires));
            let mut merged = fields.clone();
            merged.extend(updated.clone());
            let row = observed.clone();
            let key = key.clone();
            let owner = owner.clone();
            let clock = self.clock.clone();
            self.db(move |store| {
                let now = clock();
                let usable = expires > now;
                let updated = if usable { updated } else { merged };
                if !store
                    .try_commit_oauth_refresh(
                        row.id,
                        &row.serialized_data,
                        &AuthCredential::OAuth { fields: updated },
                        &key,
                        &CredentialRefreshLeaseFence { owner, now_ms: now as i64 },
                    )
                    .map_err(|_| AuthStorageError::Storage)?
                {
                    return Err(AuthStorageError::OutcomeUnknown);
                }
                let committed = store
                    .list_auth_credentials(Some(&row.provider))
                    .map_err(|_| AuthStorageError::Storage)?
                    .into_iter()
                    .find(|r| r.id == row.id)
                    .ok_or(AuthStorageError::Unavailable)?;
                if usable { Ok(committed) } else { Err(AuthStorageError::Unavailable) }
            })
            .await
        }
        .await;
        let id = observed.id;
        let _ = self
            .db(move |store| store.release_credential_refresh_lease(id, &owner).map_err(|_| AuthStorageError::Storage))
            .await;
        outcome
    }
}
fn server_url(id: &str) -> Option<String> {
    if let Some(rest) = id.strip_prefix("mcp_oauth:profile:") {
        rest.split_once(':').and_then(|(_, url)| (!url.is_empty()).then(|| url.to_owned()))
    } else {
        id.strip_prefix("mcp_oauth:").filter(|v| !v.is_empty()).map(str::to_owned)
    }
}
fn resource_indicator(
    resource: Option<&str>,
    anchor: &str,
    fallback: bool,
) -> Result<Option<String>, AuthStorageError> {
    let Some(resource) = resource.filter(|v| !ara_prompt::js::trim(v).is_empty()) else {
        return Ok(None);
    };
    if ara_prompt::js::trim(resource) != resource {
        return Err(AuthStorageError::Configuration);
    }
    let url = reqwest::Url::parse(resource).map_err(|_| AuthStorageError::Configuration)?;
    if !matches!(url.scheme(), "http" | "https") || url.fragment().is_some_and(|fragment| !fragment.is_empty()) {
        return Err(AuthStorageError::Configuration);
    }
    if fallback && reqwest::Url::parse(anchor).is_ok_and(|anchor| anchor.origin() == url.origin()) {
        return Ok(None);
    }
    Ok(Some(resource.into()))
}
fn definitive_failure(status: u16, detail: &str) -> bool {
    let message = format!("MCP OAuth refresh failed: {status} {detail}");
    let definitive = regex::Regex::new(
        r"(?i)invalid_grant|invalid_token|unauthorized_client|\brevoked\b|refresh[\s_]?token.*expired",
    )
    .expect("fixed regex");
    let transient = regex::Regex::new(r"(?i)timeout|network|fetch failed|ECONN(?:REFUSED|RESET)|ETIMEDOUT|EAI_AGAIN|socket hang up|\b(?:408|425|429|5\d{2})\b|rate.?limit|too many requests|temporar|unavailable|forbidden|permission_denied|cloudflare|captcha").expect("fixed regex");
    definitive.is_match(&message)
        || (regex::Regex::new(r"\b401\b").expect("fixed regex").is_match(&message) && !transient.is_match(&message))
}
