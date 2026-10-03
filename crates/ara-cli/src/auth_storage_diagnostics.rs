//! Per-credential diagnostics for fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-storage.ts:206–325, 3245–3301 and 4373–4536.
//! The Host supplies the optional completion probe. Usage probes bypass the
//! normal cache, ranking, block reconciliation and history paths.
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

use super::*;
use std::future::Future;

pub type CredentialBaseUrlResolver = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

#[derive(Clone, Default)]
pub struct CheckCredentialsOptions {
    pub base_url_resolver: Option<CredentialBaseUrlResolver>,
    pub timeout: Option<Duration>,
    pub completion_timeout: Option<Duration>,
    pub completion_probe: Option<Arc<dyn CredentialCompletionProbe>>,
}

/// A read-only receipt. Neither the stored key nor an OAuth bearer is included.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialHealthResult {
    pub id: i64,
    pub provider: String,
    #[serde(rename = "type")]
    pub credential_type: UsageCredentialType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_refresh: Option<bool>,
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<UsageReport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion: Option<CredentialCompletionResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialCompletionResult {
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<f64>,
}

/// Private callback input, intentionally without serialization or Debug.
#[derive(Clone)]
pub enum CredentialCompletionCredential {
    ApiKey {
        api_key: String,
    },
    OAuth {
        access_token: String,
        refresh_token: Option<String>,
        expires_at: Option<f64>,
        account_id: Option<String>,
        project_id: Option<String>,
        email: Option<String>,
        enterprise_url: Option<String>,
        api_endpoint: Option<String>,
    },
}

/// The probe may compose a provider-specific bearer from these refreshed bytes.
#[derive(Clone)]
pub struct CredentialCompletionRequest {
    pub provider: String,
    pub credential_id: i64,
    pub credential: CredentialCompletionCredential,
}

#[async_trait]
pub trait CredentialCompletionProbe: Send + Sync {
    async fn probe(
        &self,
        request: CredentialCompletionRequest,
        cancel: &CancellationToken,
    ) -> Result<CredentialCompletionResult, String>;
}

impl AuthStorage {
    /// Sequentially inspect the active stored snapshot, independently of
    /// request overrides and the cached quota displayed by ordinary polling.
    pub async fn check_credentials(
        &self,
        options: &CheckCredentialsOptions,
        cancel: &CancellationToken,
    ) -> Result<Vec<CredentialHealthResult>, AuthStorageError> {
        check_cancel(cancel)?;
        let rows = self
            .database(|store, state| {
                store.list_auth_credentials(None).map_err(|error| {
                    state.assignments.observe_store_error(&error);
                    AuthStorageError::Storage
                })
            })
            .await?;
        let timeout = options.timeout.unwrap_or(self.inner.options.usage_request_timeout);
        let completion_timeout = options.completion_timeout.unwrap_or(timeout);
        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            // Native checks the outer signal at entry and between rows. A
            // probe error, including its cancellation, remains a row receipt.
            check_cancel(cancel)?;
            let credential_type = if matches!(row.credential, AuthCredential::OAuth { .. }) {
                UsageCredentialType::Oauth
            } else {
                UsageCredentialType::ApiKey
            };
            let mut result = CredentialHealthResult {
                id: row.id,
                provider: row.provider.clone(),
                credential_type,
                email: diagnostic_identity(&row, "email"),
                account_id: diagnostic_identity(&row, "accountId"),
                org_id: diagnostic_identity(&row, "orgId"),
                org_name: diagnostic_identity(&row, "orgName"),
                remote_refresh: (text_field(&row, "refresh").as_deref() == Some("__remote__")).then_some(true),
                ok: None,
                reason: None,
                report: None,
                completion: None,
            };
            let base_url = options.base_url_resolver.as_ref().and_then(|resolver| resolver(&row.provider));
            let resolved_key = if let AuthCredential::ApiKey { key, .. } = &row.credential {
                let key = self.inner.options.config_key_resolver.resolve(key, cancel).await?;
                if key.as_ref().is_none_or(String::is_empty) {
                    result.reason = Some("api key reference could not be resolved".into());
                    results.push(result);
                    continue;
                }
                key
            } else {
                None
            };
            let credential = usage_credential(&row, resolved_key);
            let initial_request = UsageRequest {
                provider: row.provider.clone(),
                account_key: usage_identity(&credential),
                credential,
                credential_id: Some(row.id),
                base_url,
            };
            let mut request = initial_request.clone();
            let probe_cancel = cancel.child_token();
            // Refresh and usage share one deadline. Completion gets a new one.
            let deadline = tokio::time::Instant::now() + timeout;
            let credential = &initial_request.credential;
            if credential.credential_type == UsageCredentialType::Oauth
                && credential.expires_at.is_some_and(|expires| self.now() >= expires)
                && credential.access_token.as_ref().is_some_and(|access| !access.is_empty())
                && credential.refresh_token.as_ref().is_some_and(|refresh| !refresh.is_empty())
            {
                let refresh = async {
                    self.prepare_diagnostic_oauth(&row.provider, row.clone(), &probe_cancel)
                        .await
                        .map_err(|error| error.to_string())
                };
                match diagnostic_deadline(refresh, deadline, &probe_cancel).await {
                    Ok(credential) => {
                        request.credential = merge_diagnostic_refresh(request.credential, credential);
                        request.account_key = usage_identity(&request.credential);
                    }
                    Err(reason) => {
                        result.ok = Some(false);
                        result.reason = Some(format!("oauth refresh failed: {reason}"));
                        results.push(result);
                        continue;
                    }
                }
            }
            match self.usage_hook(&row.provider) {
                None => result.reason = Some(format!("no usage probe configured for provider {}", row.provider)),
                Some(provider) if !provider.supports(&initial_request) => {
                    result.reason = Some(format!(
                        "usage probe does not support {} credentials for {}",
                        row.credential.credential_type(),
                        row.provider
                    ));
                }
                Some(provider) if !provider.validates_credentials() => {
                    result.reason = Some(format!("usage probe for {} does not validate credentials", row.provider));
                }
                Some(provider) => {
                    let fetch = async {
                        provider.fetch_usage(request.clone(), &probe_cancel).await.map_err(diagnostic_usage_error)
                    };
                    match diagnostic_deadline(fetch, deadline, &probe_cancel).await {
                        Ok(Some(mut report)) => {
                            result.ok = Some(true);
                            if let Some(account) = diagnostic_report_identity(&report, "accountId") {
                                result.account_id = Some(account);
                            }
                            if let Some(email) = diagnostic_report_identity(&report, "email") {
                                result.email = Some(email);
                            }
                            report.raw = None;
                            report.unknown_fields.remove("raw");
                            result.report = Some(report);
                        }
                        Ok(None) => result.reason = Some("usage probe returned no data for this credential".into()),
                        Err(reason) => {
                            result.ok = Some(false);
                            result.reason = Some(reason);
                        }
                    }
                }
            }
            if let Some(probe) = &options.completion_probe {
                result.completion = Some(match diagnostic_completion_credential(&request.credential) {
                    None => CredentialCompletionResult {
                        reason: Some(format!(
                            "no bearer bytes available for {} credential",
                            row.credential.credential_type()
                        )),
                        ..Default::default()
                    },
                    Some(credential) => {
                        let completion_cancel = cancel.child_token();
                        let input = CredentialCompletionRequest {
                            provider: row.provider.clone(),
                            credential_id: row.id,
                            credential,
                        };
                        let deadline = tokio::time::Instant::now() + completion_timeout;
                        match diagnostic_deadline(probe.probe(input, &completion_cancel), deadline, &completion_cancel)
                            .await
                        {
                            Ok(completion) => completion,
                            Err(reason) => CredentialCompletionResult {
                                ok: Some(false),
                                reason: Some(reason),
                                ..Default::default()
                            },
                        }
                    }
                });
            }
            results.push(result);
        }
        Ok(results)
    }
}

fn diagnostic_identity(row: &StoredAuthCredential, key: &str) -> Option<String> {
    text_field(row, key).filter(|value| !value.is_empty())
}

fn diagnostic_report_identity(report: &UsageReport, key: &str) -> Option<String> {
    report.metadata.as_ref()?.get(key)?.as_str().map(trim_js).filter(|value| !value.is_empty()).map(str::to_owned)
}

fn diagnostic_usage_error(error: UsageFetchError) -> String {
    match error {
        UsageFetchError::Cancelled => "usage probe cancelled",
        UsageFetchError::Transient => "usage probe temporarily unavailable",
        UsageFetchError::Unauthorized => "usage probe rejected credential (401)",
        UsageFetchError::Forbidden => "usage probe rejected credential (403)",
        UsageFetchError::InvalidResponse => "usage probe returned an invalid response",
    }
    .to_owned()
}

async fn diagnostic_deadline<T>(
    operation: impl Future<Output = Result<T, String>>,
    deadline: tokio::time::Instant,
    cancel: &CancellationToken,
) -> Result<T, String> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("credential probe cancelled".into()),
        result = tokio::time::timeout_at(deadline, operation) => match result {
            Ok(result) => result,
            Err(_) => {
                cancel.cancel();
                Err("credential probe timed out".into())
            }
        },
    }
}

fn merge_diagnostic_refresh(previous: UsageCredential, fresh: UsageCredential) -> UsageCredential {
    UsageCredential {
        access_token: fresh.access_token,
        refresh_token: fresh.refresh_token,
        expires_at: fresh.expires_at,
        account_id: fresh.account_id.or(previous.account_id),
        project_id: fresh.project_id.or(previous.project_id),
        email: fresh.email.or(previous.email),
        enterprise_url: fresh.enterprise_url.or(previous.enterprise_url),
        api_endpoint: fresh.api_endpoint.or(previous.api_endpoint),
        org_id: fresh.org_id.or(previous.org_id),
        org_name: fresh.org_name.or(previous.org_name),
        ..previous
    }
}

fn diagnostic_completion_credential(credential: &UsageCredential) -> Option<CredentialCompletionCredential> {
    match credential.credential_type {
        UsageCredentialType::ApiKey => credential
            .api_key
            .as_ref()
            .filter(|key| !key.is_empty())
            .map(|api_key| CredentialCompletionCredential::ApiKey { api_key: api_key.clone() }),
        UsageCredentialType::Oauth => credential.access_token.as_ref().filter(|token| !token.is_empty()).map(|token| {
            CredentialCompletionCredential::OAuth {
                access_token: token.clone(),
                refresh_token: credential.refresh_token.clone(),
                expires_at: credential.expires_at,
                account_id: credential.account_id.clone(),
                project_id: credential.project_id.clone(),
                email: credential.email.clone(),
                enterprise_url: credential.enterprise_url.clone(),
                api_endpoint: credential.api_endpoint.clone(),
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_testkit::{FakeUpstream, Script};
    use serde_json::json;

    const FIXTURE_NOW: f64 = 1_000_000.0;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build().unwrap()
    }

    async fn upstream(responses: Vec<Value>) -> FakeUpstream {
        FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses": responses})).unwrap(), None)
            .await
            .unwrap()
    }

    fn response(body: Value) -> Value {
        json!({"body": body.to_string()})
    }

    fn report() -> Value {
        json!({
            "provider": "fixture", "fetchedAt": FIXTURE_NOW, "limits": [],
            "metadata": {"accountId": " probe-account ", "email": " probe@example.invalid "},
            "raw": {"fixturePrivatePayload": "must not be returned"}
        })
    }

    fn completion_response() -> Value {
        response(json!({"ok": true, "modelId": "fixture-completion", "latencyMs": 42.5}))
    }

    struct References;
    #[async_trait]
    impl ConfigKeyResolver for References {
        async fn resolve(
            &self,
            configuration: &str,
            cancel: &CancellationToken,
        ) -> Result<Option<String>, AuthStorageError> {
            check_cancel(cancel)?;
            Ok(match configuration {
                "!reference" => Some("fixture-resolved-key".into()),
                "!missing" => None,
                _ => Some(configuration.into()),
            })
        }
    }

    fn diagnostic_storage(store: Arc<Mutex<SqliteCredentialStore>>) -> AuthStorage {
        AuthStorage::new(
            store,
            None,
            AuthStorageOptions {
                config_key_resolver: Arc::new(References),
                clock: Arc::new(|| FIXTURE_NOW),
                jitter: Arc::new(|| 0.5),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn seed(store: &Arc<Mutex<SqliteCredentialStore>>, provider: &str, credential: AuthCredential) -> i64 {
        store.lock().unwrap().upsert_auth_credential_for_provider(provider, &credential).unwrap().last().unwrap().id
    }

    fn oauth(expires: f64, refresh: &str, access: &str) -> AuthCredential {
        AuthCredential::oauth(
            json!({
                "access": access, "refresh": refresh, "expires": expires,
                "accountId": "original-account", "email": "original@example.invalid",
                "orgId": "original-org", "orgName": "original-plan", "projectId": "original-project",
                "enterpriseUrl": "https://fixture.invalid", "apiEndpoint": "https://api.fixture.invalid"
            })
            .as_object()
            .unwrap()
            .clone(),
        )
    }

    fn options(fake: &FakeUpstream) -> CheckCredentialsOptions {
        let origin = format!("http://{}", fake.addr);
        CheckCredentialsOptions {
            base_url_resolver: Some(Arc::new(move |_| Some(origin.clone()))),
            timeout: Some(Duration::from_secs(2)),
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct UsageTrace {
        requests: Mutex<Vec<UsageRequest>>,
        active: AtomicUsize,
        max_active: AtomicUsize,
    }
    struct ActiveCall<'a>(&'a AtomicUsize);
    impl Drop for ActiveCall<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::AcqRel);
        }
    }
    struct SnapshotMutation {
        store: Arc<Mutex<SqliteCredentialStore>>,
        disable_id: i64,
    }
    struct HttpUsage {
        client: reqwest::Client,
        trace: Arc<UsageTrace>,
        validates: bool,
        supports: bool,
        initial_account: Option<String>,
        mutation: Mutex<Option<SnapshotMutation>>,
    }
    impl HttpUsage {
        fn new(trace: Arc<UsageTrace>) -> Self {
            Self {
                client: client(),
                trace,
                validates: true,
                supports: true,
                initial_account: None,
                mutation: Mutex::new(None),
            }
        }
    }
    #[async_trait]
    impl UsageProvider for HttpUsage {
        fn validates_credentials(&self) -> bool {
            self.validates
        }
        fn supports(&self, request: &UsageRequest) -> bool {
            assert!(!request.account_key.is_empty(), "initial descriptor carries native identity");
            self.supports
                && self
                    .initial_account
                    .as_ref()
                    .is_none_or(|account| request.credential.account_id.as_ref() == Some(account))
        }
        async fn fetch_usage(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, UsageFetchError> {
            let active = self.trace.active.fetch_add(1, Ordering::AcqRel) + 1;
            self.trace.max_active.fetch_max(active, Ordering::AcqRel);
            let _active = ActiveCall(&self.trace.active);
            self.trace.requests.lock().unwrap().push(request.clone());
            let mutation = self.mutation.lock().unwrap().take();
            if let Some(mutation) = mutation {
                let store = mutation.store.lock().unwrap();
                store.delete_auth_credential(mutation.disable_id, "fixture peer disabled").unwrap();
                store
                    .upsert_auth_credential_for_provider("added-later", &AuthCredential::api_key("fixture-later"))
                    .unwrap();
            }
            let mut call = self.client.get(format!("{}/usage", request.base_url.as_deref().unwrap()));
            if let Some(bearer) = request.credential.api_key.as_deref().or(request.credential.access_token.as_deref()) {
                call = call.bearer_auth(bearer);
            }
            call = call.header("x-fixture-credential-id", request.credential_id.unwrap().to_string());
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(UsageFetchError::Cancelled),
                response = call.send() => response.map_err(|_| UsageFetchError::Transient)?,
            };
            match response.status().as_u16() {
                204 => Ok(None),
                401 => Err(UsageFetchError::Unauthorized),
                403 => Err(UsageFetchError::Forbidden),
                200 => response.json().await.map(Some).map_err(|_| UsageFetchError::InvalidResponse),
                _ => Err(UsageFetchError::Transient),
            }
        }
    }

    struct HttpOAuth {
        store: Arc<Mutex<SqliteCredentialStore>>,
        origin: String,
        client: reqwest::Client,
    }
    #[async_trait]
    impl OAuthProvider for HttpOAuth {
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
            Ok(RequestAuthLease::new(
                CredentialIdentity::Stored { id: row.id, revision: row.revision },
                text_field(row, "access"),
            ))
        }
        async fn resolve(
            &self,
            row: StoredAuthCredential,
            _: bool,
            cancel: &CancellationToken,
        ) -> Result<ResolvedOAuth, AuthStorageError> {
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                response = self.client.post(format!("{}/refresh", self.origin))
                    .json(&json!({"credentialId": row.id, "refresh": text_field(&row, "refresh")})).send() =>
                    response.map_err(|_| AuthStorageError::OutcomeUnknown)?,
            };
            if !response.status().is_success() {
                return Err(AuthStorageError::Definitive);
            }
            let fresh: Map<String, Value> = response.json().await.map_err(|_| AuthStorageError::OutcomeUnknown)?;
            let mut credential = row.credential.clone();
            if let AuthCredential::OAuth { fields } = &mut credential {
                fields.extend(fresh);
            }
            let store = self.store.lock().unwrap();
            if !store.try_update_auth_credential_if_matches(row.id, &row.serialized_data, &credential, None).unwrap() {
                return Err(AuthStorageError::Unavailable);
            }
            let row = store
                .list_auth_credentials(Some(&row.provider))
                .unwrap()
                .into_iter()
                .find(|candidate| candidate.id == row.id)
                .ok_or(AuthStorageError::Unavailable)?;
            Ok(ResolvedOAuth { lease: self.lease(&row)?, row })
        }
    }

    struct HttpCompletion {
        origin: String,
        client: reqwest::Client,
        requests: Mutex<Vec<CredentialCompletionRequest>>,
    }
    impl HttpCompletion {
        fn new(fake: &FakeUpstream) -> Self {
            Self { origin: format!("http://{}", fake.addr), client: client(), requests: Mutex::new(Vec::new()) }
        }
    }
    #[async_trait]
    impl CredentialCompletionProbe for HttpCompletion {
        async fn probe(
            &self,
            request: CredentialCompletionRequest,
            cancel: &CancellationToken,
        ) -> Result<CredentialCompletionResult, String> {
            let bearer = match &request.credential {
                CredentialCompletionCredential::ApiKey { api_key } => api_key.clone(),
                CredentialCompletionCredential::OAuth { access_token, .. } => access_token.clone(),
            };
            let id = request.credential_id;
            self.requests.lock().unwrap().push(request);
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err("fixture completion cancelled".into()),
                response = self.client.post(format!("{}/completion", self.origin)).bearer_auth(bearer)
                    .json(&json!({"credentialId":id})).send() => response.map_err(|error| error.to_string())?,
            };
            if !response.status().is_success() {
                return Err(format!("fixture completion HTTP {}", response.status().as_u16()));
            }
            response.json().await.map_err(|error| error.to_string())
        }
    }

    #[tokio::test]
    async fn host_check_credentials_mixed_native_probe_family() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
        for (provider, key) in [
            ("healthy", "!reference"),
            ("rejected", "fixture-rejected"),
            ("empty", "fixture-empty"),
            ("local-only", "fixture-local"),
            ("unsupported", "fixture-unsupported"),
            ("no-probe", "fixture-none"),
            ("unresolved", "!missing"),
        ] {
            seed(&store, provider, AuthCredential::api_key(key));
        }
        let disabled_id = seed(&store, "disabled", AuthCredential::api_key("fixture-disabled"));
        store.lock().unwrap().delete_auth_credential(disabled_id, "fixture disabled").unwrap();
        let snapshot = store.lock().unwrap().list_auth_credentials(None).unwrap();
        let responses: Vec<_> = snapshot
            .iter()
            .filter_map(|row| match row.provider.as_str() {
                "healthy" => Some(response(report())),
                "rejected" => Some(json!({"status":401})),
                "empty" => Some(json!({"status":204})),
                _ => None,
            })
            .collect();
        let fake = upstream(responses.iter().chain(&responses).cloned().collect()).await;
        let trace = Arc::new(UsageTrace::default());
        let storage = diagnostic_storage(store.clone());
        for provider in ["healthy", "rejected", "empty", "local-only", "unsupported"] {
            let mut usage = HttpUsage::new(trace.clone());
            usage.validates = provider != "local-only";
            usage.supports = provider != "unsupported";
            storage.register_usage_provider(provider, Arc::new(usage)).unwrap();
        }
        storage.set_runtime_api_key("healthy", "fixture-override-must-not-win".into()).unwrap();
        let healthy = snapshot.iter().find(|row| row.provider == "healthy").unwrap();
        let cache_request = UsageRequest {
            provider: "healthy".into(),
            credential: usage_credential(healthy, Some("fixture-resolved-key".into())),
            credential_id: Some(healthy.id),
            base_url: Some(format!("http://{}", fake.addr)),
            account_key: String::new(),
        };
        let cache_key = format!("{USAGE_CACHE_PREFIX}{}", usage_report_key(&cache_request));
        let cached = serde_json::to_string(&UsageCacheEntry {
            value: Some(UsageReport { provider: "cached-must-not-win".into(), ..Default::default() }),
            expires_at: FIXTURE_NOW + 300_000.0,
        })
        .unwrap();
        store.lock().unwrap().set_cache(&cache_key, &cached, 2_000).unwrap();
        for _ in 0..2 {
            let results = storage.check_credentials(&options(&fake), &CancellationToken::new()).await.unwrap();
            assert_eq!(
                results.iter().map(|row| row.id).collect::<Vec<_>>(),
                snapshot.iter().map(|row| row.id).collect::<Vec<_>>()
            );
            for result in &results {
                match result.provider.as_str() {
                    "healthy" => {
                        assert_eq!(result.ok, Some(true));
                        assert_eq!(result.account_id.as_deref(), Some("probe-account"));
                        assert_eq!(result.email.as_deref(), Some("probe@example.invalid"));
                        assert!(result.report.as_ref().unwrap().raw.is_none());
                        assert!(serde_json::to_value(result).unwrap()["report"].get("raw").is_none());
                    }
                    "rejected" => {
                        assert_eq!(result.ok, Some(false));
                        assert!(result.reason.as_ref().unwrap().contains("401"));
                    }
                    "empty" => {
                        assert_eq!(result.ok, None);
                        assert!(result.reason.as_ref().unwrap().contains("no data"));
                    }
                    "local-only" => {
                        assert_eq!(result.ok, None);
                        assert!(result.reason.as_ref().unwrap().contains("does not validate"));
                    }
                    "unsupported" => {
                        assert_eq!(result.ok, None);
                        assert!(result.reason.as_ref().unwrap().contains("does not support"));
                    }
                    "no-probe" => {
                        assert_eq!(result.ok, None);
                        assert!(result.reason.as_ref().unwrap().contains("no usage probe"));
                    }
                    "unresolved" => {
                        assert_eq!(result.ok, None);
                        assert!(result.reason.as_ref().unwrap().contains("could not be resolved"));
                    }
                    provider => panic!("unexpected active row {provider}"),
                }
                assert!(result.completion.is_none());
            }
        }
        assert_eq!(fake.requests.lock().await.len(), 6, "diagnostics are uncached");
        assert_eq!(trace.max_active.load(Ordering::Acquire), 1, "stored probes remain sequential");
        assert!(
            trace
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.provider == "healthy")
                .all(|request| request.credential.api_key.as_deref() == Some("fixture-resolved-key"))
        );
        assert_eq!(store.lock().unwrap().get_cache(&cache_key, true).unwrap().as_deref(), Some(cached.as_str()));
        let reopened = SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap();
        assert_eq!(reopened.list_auth_credentials(None).unwrap().len(), snapshot.len());
    }

    #[derive(Clone, Copy)]
    struct OAuthDiagnosticCase {
        name: &'static str,
        expired: bool,
        refresh_failure: bool,
        refreshable: bool,
        remote: bool,
        usage: bool,
        validates: bool,
        completion_failure: bool,
        bearer: bool,
        api_key: bool,
    }

    #[tokio::test]
    async fn host_check_credentials_oauth_and_completion_native_family() {
        let base = OAuthDiagnosticCase {
            name: "refreshed",
            expired: true,
            refresh_failure: false,
            refreshable: true,
            remote: false,
            usage: true,
            validates: true,
            completion_failure: false,
            bearer: true,
            api_key: false,
        };
        let cases = [
            OAuthDiagnosticCase { ..base },
            OAuthDiagnosticCase { name: "upfront-refresh-without-usage", usage: false, ..base },
            OAuthDiagnosticCase { name: "refresh-failure-skips-both", refresh_failure: true, ..base },
            OAuthDiagnosticCase { name: "expired-unrefreshable-still-probes", refreshable: false, ..base },
            OAuthDiagnosticCase { name: "remote-refresh-sentinel", expired: false, remote: true, ..base },
            OAuthDiagnosticCase { name: "local-only-usage-still-completes", expired: false, validates: false, ..base },
            OAuthDiagnosticCase {
                name: "completion-error-independent",
                expired: false,
                completion_failure: true,
                ..base
            },
            OAuthDiagnosticCase { name: "missing-bearer-skip-completion", bearer: false, ..base },
            OAuthDiagnosticCase { name: "reference-key-completion", expired: false, api_key: true, ..base },
        ];
        for case in cases {
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
            let expires = if case.expired { FIXTURE_NOW - 1.0 } else { FIXTURE_NOW + 30_000.0 };
            let credential = if case.api_key {
                AuthCredential::api_key("!reference")
            } else {
                oauth(
                    expires,
                    if case.remote {
                        "__remote__"
                    } else if case.refreshable {
                        "fixture-refresh"
                    } else {
                        ""
                    },
                    if case.bearer { "fixture-original-access" } else { "" },
                )
            };
            let id = seed(&store, "fixture", credential);
            let refresh = !case.api_key && case.expired && case.refreshable && case.bearer;
            let refreshed = refresh && !case.refresh_failure;
            let usage = case.usage && case.validates && !case.refresh_failure;
            let completion = case.bearer && !case.refresh_failure;
            let mut responses = Vec::new();
            let mut expected_paths = Vec::new();
            if refresh {
                responses.push(if case.refresh_failure {
                    json!({"status":401})
                } else {
                    response(json!({
                        "access": "fixture-fresh-access", "refresh": "fixture-fresh-refresh", "expires": FIXTURE_NOW + 7_200_000.0,
                        "accountId": "fresh-account", "projectId": "fresh-project",
                        "email": "fresh@example.invalid", "apiEndpoint": "https://api.fresh.invalid"
                    }))
                });
                expected_paths.push("POST /refresh HTTP/1.1");
            }
            if usage {
                responses.push(response(report()));
                expected_paths.push("GET /usage HTTP/1.1");
            }
            if completion {
                responses.push(if case.completion_failure { json!({"status":401}) } else { completion_response() });
                expected_paths.push("POST /completion HTTP/1.1");
            }
            let fake = upstream(responses).await;
            let storage = diagnostic_storage(store.clone());
            storage
                .register_oauth_provider(
                    "fixture",
                    Arc::new(HttpOAuth {
                        store: store.clone(),
                        origin: format!("http://{}", fake.addr),
                        client: client(),
                    }),
                )
                .unwrap();
            let trace = Arc::new(UsageTrace::default());
            if case.usage {
                let mut provider = HttpUsage::new(trace.clone());
                provider.validates = case.validates;
                provider.initial_account = (!case.api_key).then(|| "original-account".into());
                storage.register_usage_provider("fixture", Arc::new(provider)).unwrap();
            }
            let probe = Arc::new(HttpCompletion::new(&fake));
            let mut options = options(&fake);
            options.completion_probe = Some(probe.clone());
            let [result]: [_; 1] =
                storage.check_credentials(&options, &CancellationToken::new()).await.unwrap().try_into().unwrap();
            assert_eq!(result.id, id, "{}", case.name);
            assert_eq!(result.remote_refresh, case.remote.then_some(true), "{}", case.name);
            let expected_ok = if case.refresh_failure {
                Some(false)
            } else if usage {
                Some(true)
            } else {
                None
            };
            assert_eq!(result.ok, expected_ok, "{}", case.name);
            if case.refresh_failure {
                assert!(result.reason.as_ref().unwrap().starts_with("oauth refresh failed:"));
                assert!(result.completion.is_none());
                assert!(
                    store
                        .lock()
                        .unwrap()
                        .list_auth_credentials(Some("fixture"))
                        .unwrap()
                        .iter()
                        .any(|row| row.id == id),
                    "outer diagnostic does not add a disable"
                );
            } else if completion {
                let completed = result.completion.as_ref().unwrap();
                assert_eq!(completed.ok, Some(!case.completion_failure), "{}", case.name);
                if case.completion_failure {
                    assert_eq!(completed.reason.as_deref(), Some("fixture completion HTTP 401"));
                } else {
                    assert_eq!(completed.model_id.as_deref(), Some("fixture-completion"));
                    assert_eq!(completed.latency_ms, Some(42.5));
                }
                let requests = probe.requests.lock().unwrap();
                assert_eq!(requests.len(), 1);
                assert_eq!(requests[0].credential_id, id);
                assert_eq!(requests[0].provider, "fixture");
                match &requests[0].credential {
                    CredentialCompletionCredential::ApiKey { api_key } => {
                        assert!(case.api_key);
                        assert_eq!(api_key, "fixture-resolved-key");
                    }
                    CredentialCompletionCredential::OAuth {
                        access_token,
                        refresh_token,
                        expires_at,
                        account_id,
                        project_id,
                        email,
                        enterprise_url,
                        api_endpoint,
                    } => {
                        assert_eq!(
                            access_token,
                            if refreshed { "fixture-fresh-access" } else { "fixture-original-access" }
                        );
                        assert_eq!(
                            refresh_token.as_deref(),
                            Some(if refreshed {
                                "fixture-fresh-refresh"
                            } else if case.remote {
                                "__remote__"
                            } else if case.refreshable {
                                "fixture-refresh"
                            } else {
                                ""
                            })
                        );
                        assert_eq!(*expires_at, Some(if refreshed { FIXTURE_NOW + 7_200_000.0 } else { expires }));
                        assert_eq!(
                            account_id.as_deref(),
                            Some(if refreshed { "fresh-account" } else { "original-account" })
                        );
                        assert_eq!(
                            project_id.as_deref(),
                            Some(if refreshed { "fresh-project" } else { "original-project" })
                        );
                        assert_eq!(
                            email.as_deref(),
                            Some(if refreshed { "fresh@example.invalid" } else { "original@example.invalid" })
                        );
                        assert_eq!(enterprise_url.as_deref(), Some("https://fixture.invalid"));
                        assert_eq!(
                            api_endpoint.as_deref(),
                            Some(if refreshed { "https://api.fresh.invalid" } else { "https://api.fixture.invalid" })
                        );
                    }
                }
            } else {
                assert_eq!(result.completion.as_ref().unwrap().ok, None, "{}", case.name);
                assert!(result.completion.as_ref().unwrap().reason.as_ref().unwrap().contains("no bearer bytes"));
                assert!(probe.requests.lock().unwrap().is_empty());
            }
            let requests = fake.requests.lock().await;
            assert_eq!(
                requests.iter().map(|request| request["request"].as_str().unwrap()).collect::<Vec<_>>(),
                expected_paths,
                "{}",
                case.name
            );
            if let Some(request) = trace.requests.lock().unwrap().first() {
                assert_eq!(request.credential_id, Some(id));
                if refreshed {
                    assert_eq!(request.credential.access_token.as_deref(), Some("fixture-fresh-access"));
                    assert_eq!(request.credential.account_id.as_deref(), Some("fresh-account"));
                    assert!(request.account_key.contains("fresh-account"));
                }
            }
            if refreshed {
                let durable = SqliteCredentialStore::open(temp.path().join("auth.db"))
                    .unwrap()
                    .list_auth_credentials(Some("fixture"))
                    .unwrap()
                    .into_iter()
                    .find(|row| row.id == id)
                    .unwrap();
                assert_eq!(text_field(&durable, "apiEndpoint").as_deref(), Some("https://api.fresh.invalid"));
            }
        }
    }

    #[tokio::test]
    async fn host_check_credentials_builtin_codex_null_family() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
        // The builtin adapter uses wall time independently of the injected
        // Host clock, so its expiry must also be fresh at that boundary.
        let expires = chrono::Utc::now().timestamp_millis() as f64 + 3_600_000.0;
        seed(&store, "openai-codex", oauth(expires, "fixture-refresh", "fixture-access"));
        let fake = upstream(vec![
            response(json!({
                "plan_type": "pro", "rate_limit": {"allowed":true, "limit_reached":false,
                    "primary_window":{"used_percent":11,"limit_window_seconds":18000,"reset_after_seconds":600}}
            })),
            json!({"status":401}),
            json!({"status":403}),
        ])
        .await;
        let storage = diagnostic_storage(store);
        let origin = format!("http://{}", fake.addr);
        let builtin =
            crate::codex_usage::CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |canonical| {
                format!("{}{}", origin, reqwest::Url::parse(canonical).unwrap().path())
            }))
            .unwrap();
        storage.register_usage_provider("openai-codex", Arc::new(builtin)).unwrap();
        for expected in [Some(true), None, None] {
            let result = storage.check_credentials(&options(&fake), &CancellationToken::new()).await.unwrap().remove(0);
            assert_eq!(result.ok, expected, "fixed builtin non-2xx returns null rather than throwing");
            if expected.is_none() {
                assert_eq!(result.reason.as_deref(), Some("usage probe returned no data for this credential"));
            } else {
                assert!(result.report.unwrap().raw.is_none());
            }
        }
        assert_eq!(fake.requests.lock().await.len(), 3);

        let temp = tempfile::tempdir().unwrap();
        let fake = upstream(Vec::new()).await;
        let service = Arc::new(
            OpenAiCodexAuth::open_with_endpoints(
                temp.path().join("auth.db"),
                client(),
                &format!("http://{}", fake.addr),
            )
            .await
            .unwrap(),
        );
        let id =
            seed(&service.store_handle(), "openai-codex", oauth(FIXTURE_NOW - 1.0, "__remote__", "fixture-expired"));
        let storage = AuthStorage::for_codex(
            service,
            AuthStorageOptions { clock: Arc::new(|| FIXTURE_NOW), ..Default::default() },
        )
        .unwrap();
        let result = storage.check_credentials(&options(&fake), &CancellationToken::new()).await.unwrap().remove(0);
        assert_eq!(result.id, id);
        assert_eq!(result.ok, Some(false));
        assert_eq!(result.remote_refresh, Some(true));
        assert!(result.reason.as_ref().unwrap().starts_with("oauth refresh failed:"));
        assert!(result.completion.is_none());
        assert!(fake.requests.lock().await.is_empty(), "remote sentinel never reaches the local refresh endpoint");
    }

    #[tokio::test]
    async fn host_check_credentials_snapshot_deadline_and_cancel_family() {
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
        seed(&store, "fixture", AuthCredential::api_key("fixture-first"));
        seed(&store, "fixture", AuthCredential::api_key("fixture-second"));
        let snapshot = store.lock().unwrap().list_auth_credentials(None).unwrap();
        let fake = upstream(vec![response(report()), response(report())]).await;
        let trace = Arc::new(UsageTrace::default());
        let storage = diagnostic_storage(store.clone());
        let mut usage = HttpUsage::new(trace.clone());
        usage.mutation = Mutex::new(Some(SnapshotMutation { store: store.clone(), disable_id: snapshot[1].id }));
        storage.register_usage_provider("fixture", Arc::new(usage)).unwrap();
        let results = storage.check_credentials(&options(&fake), &CancellationToken::new()).await.unwrap();
        assert_eq!(
            results.iter().map(|row| row.id).collect::<Vec<_>>(),
            snapshot.iter().map(|row| row.id).collect::<Vec<_>>()
        );
        assert!(results.iter().all(|result| result.ok == Some(true)), "probe the original stored snapshot");
        assert_eq!(trace.max_active.load(Ordering::Acquire), 1);
        assert_eq!(fake.requests.lock().await.len(), 2);
        assert!(
            store.lock().unwrap().list_auth_credentials(None).unwrap().iter().any(|row| row.provider == "added-later")
        );

        for (
            name,
            usage_delay,
            usage_timeout,
            completion_delay,
            completion_timeout,
            expected_usage,
            expected_completion,
        ) in [
            ("independent-completion-deadline", 200, 400, 300, 600, Some(true), Some(true)),
            ("usage-timeout-completion-still-runs", 500, 50, 0, 500, Some(false), Some(true)),
            ("completion-timeout-independent", 0, 500, 500, 50, Some(true), Some(false)),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
            seed(&store, "fixture", AuthCredential::api_key("fixture-key"));
            let mut quota = response(report());
            quota["delay_ms"] = usage_delay.into();
            let mut completed = completion_response();
            completed["delay_ms"] = completion_delay.into();
            let fake = upstream(vec![quota, completed]).await;
            let storage = diagnostic_storage(store);
            storage
                .register_usage_provider("fixture", Arc::new(HttpUsage::new(Arc::new(UsageTrace::default()))))
                .unwrap();
            let mut options = options(&fake);
            options.timeout = Some(Duration::from_millis(usage_timeout));
            options.completion_timeout = Some(Duration::from_millis(completion_timeout));
            options.completion_probe = Some(Arc::new(HttpCompletion::new(&fake)));
            let result = storage.check_credentials(&options, &CancellationToken::new()).await.unwrap().remove(0);
            assert_eq!(result.ok, expected_usage, "{name}");
            assert_eq!(result.completion.as_ref().unwrap().ok, expected_completion, "{name}");
            if expected_usage == Some(false) {
                assert!(result.reason.as_ref().unwrap().contains("timed out"));
            }
            if expected_completion == Some(false) {
                assert!(result.completion.as_ref().unwrap().reason.as_ref().unwrap().contains("timed out"));
            }
            assert_eq!(fake.requests.lock().await.len(), 2, "{name}");
        }

        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(Mutex::new(SqliteCredentialStore::open(temp.path().join("auth.db")).unwrap()));
        seed(&store, "fixture", AuthCredential::api_key("fixture-first"));
        seed(&store, "fixture", AuthCredential::api_key("fixture-second"));
        let fake = upstream(vec![json!({"body":report().to_string(),"delay_ms":500})]).await;
        let storage = diagnostic_storage(store);
        storage.register_usage_provider("fixture", Arc::new(HttpUsage::new(Arc::new(UsageTrace::default())))).unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(matches!(
            storage.check_credentials(&options(&fake), &cancelled).await,
            Err(AuthStorageError::Cancelled)
        ));
        assert!(fake.requests.lock().await.is_empty());
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let worker_options = options(&fake);
        let worker = tokio::spawn(async move { storage.check_credentials(&worker_options, &worker_cancel).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while fake.requests.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        assert!(
            matches!(worker.await.unwrap(), Err(AuthStorageError::Cancelled)),
            "next row checks caller cancellation"
        );
        assert_eq!(fake.requests.lock().await.len(), 1, "cancel never starts the next stored probe");
    }
}
