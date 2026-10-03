//! Aggregate usage collection for fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-storage.ts:1707–1737, 3187–3242, 3714–3940,
//! 4221–4344. Per-request transport/cache/history remains in the parent owner.
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
use futures::future::try_join_all;

#[derive(Clone, Default)]
pub struct FetchUsageReportsOptions {
    /// Restricts local collection. An authoritative aggregate source returns
    /// its own collection, as in the pinned override/store contract.
    pub provider: Option<String>,
    pub context: AuthRequestContext,
    /// When present, owns each provider's URL, including an explicit None.
    /// Otherwise the legacy context URL is used for every provider.
    pub base_url_resolver: Option<CredentialBaseUrlResolver>,
}

struct ForcedUsageRefresh {
    all: bool,
    providers: BTreeSet<String>,
}

impl AuthStorage {
    pub async fn fetch_usage_reports(
        &self,
        provider: Option<&str>,
        context: &AuthRequestContext,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        self.fetch_usage_reports_with_options(
            &FetchUsageReportsOptions {
                provider: provider.map(str::to_owned),
                context: context.clone(),
                base_url_resolver: None,
            },
            cancel,
        )
        .await
    }

    pub async fn fetch_usage_reports_with_options(
        &self,
        options: &FetchUsageReportsOptions,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        // Native 4258–4283: each override/store waiter may cancel, while the
        // shared source sees no caller token. Explicit override has no local
        // dedupe/reconciliation, including when it returns None or [].
        if self.inner.options.aggregate_usage_override.is_some() || self.inner.options.usage_store.is_some() {
            let epoch = self.inner.usage_epoch.load(Ordering::Acquire);
            let key = format!("__override__\0{epoch}");
            let receiver = self.aggregate_usage_flight(key, move |this| async move {
                if let Some(source) = &this.inner.options.aggregate_usage_override {
                    source.fetch_usage_reports().await
                } else if let Some(store) = &this.inner.options.usage_store {
                    store.fetch_usage_reports().await
                } else {
                    Err(AuthStorageError::Unavailable)
                }
            })?;
            let reports = await_aggregate_usage(receiver, Some(cancel)).await?;
            if self.inner.options.aggregate_usage_override.is_none()
                && let Some(reports) = &reports
            {
                self.reconcile_usage_reports(reports).await?;
            }
            return Ok(reports);
        }

        if self.inner.usage_providers.lock().map_err(|_| AuthStorageError::Storage)?.is_empty()
            && self.inner.builtin_usage_providers.lock().map_err(|_| AuthStorageError::Storage)?.is_empty()
        {
            return Ok(None);
        }

        // Native local collection/waiting does not consume the caller signal.
        // Config resolution and the eventual per-request owners use this
        // independent token, even if the caller was already cancelled.
        let owner_cancel = CancellationToken::new();
        let requests = self.collect_usage_requests(options, &owner_cancel).await?;
        if requests.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let epoch = self.inner.usage_epoch.load(Ordering::Acquire);
        let forced = self.aggregate_forced_usage(&requests);
        let key = format!("{}\0{epoch}", aggregate_usage_key(&requests));
        let receiver = self.aggregate_usage_flight(key, move |this| async move {
            let reports = this
                .fetch_collected_usage(requests, &forced.providers, owner_cancel)
                .await?
                .into_iter()
                .flatten()
                .collect();
            let reports = dedupe_usage_reports(reports);
            this.clear_aggregate_forced_usage(&forced, epoch)?;
            Ok(Some(reports))
        })?;
        await_aggregate_usage(receiver, None).await
    }

    fn aggregate_usage_flight<F, Fut>(&self, key: String, fetch: F) -> Result<AggregateUsageFlight, AuthStorageError>
    where
        F: FnOnce(AuthStorage) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Option<Vec<UsageReport>>, AuthStorageError>> + Send + 'static,
    {
        let mut flights = self.inner.aggregate_flights.lock().map_err(|_| AuthStorageError::Storage)?;
        if let Some(receiver) = flights.get(&key) {
            return Ok(receiver.clone());
        }
        let (sender, receiver) = watch::channel(None);
        flights.insert(key.clone(), receiver.clone());
        self.inner.usage_pending.fetch_add(1, Ordering::AcqRel);
        let this = self.clone();
        let owned_receiver = receiver.clone();
        tokio::spawn(async move {
            let _settlement = UsageSettlement(this.clone());
            let _flight_settlement = AggregateFlightSettlement { storage: this.clone(), key, receiver: owned_receiver };
            let result = fetch(this.clone()).await;
            sender.send_replace(Some(result));
        });
        Ok(receiver)
    }

    async fn collect_usage_requests(
        &self,
        options: &FetchUsageReportsOptions,
        cancel: &CancellationToken,
    ) -> Result<Vec<UsageRequest>, AuthStorageError> {
        let mut hooks = self.inner.builtin_usage_providers.lock().map_err(|_| AuthStorageError::Storage)?.clone();
        // The installed builtin owner currently contains only Codex. Its fixed
        // DEFAULT_USAGE_PROVIDERS ordering belongs with further builtin ports.
        let builtin_providers: Vec<_> = hooks.keys().cloned().collect();
        let (runtime_hooks, runtime_providers, extension_keys) = {
            let hooks = self.inner.usage_providers.lock().map_err(|_| AuthStorageError::Storage)?;
            let order = self.inner.runtime_usage_provider_order.lock().map_err(|_| AuthStorageError::Storage)?;
            let keys = self.inner.runtime_usage_keys.lock().map_err(|_| AuthStorageError::Storage)?;
            (hooks.clone(), order.clone(), keys.clone())
        };
        hooks.extend(runtime_hooks);
        let runtime_keys = self.inner.state.lock().map_err(|_| AuthStorageError::Storage)?.runtime_keys.clone();
        let local_providers: BTreeSet<_> = hooks.keys().cloned().collect();
        let filter = options.provider.clone();
        let rows = self
            .database(move |store, state| {
                let stored = store.list_auth_credentials(None).map_err(|error| {
                    state.assignments.observe_store_error(&error);
                    AuthStorageError::Storage
                })?;
                let stored_providers: BTreeSet<_> = stored.iter().map(|row| row.provider.clone()).collect();
                let mut seen = BTreeSet::new();
                let mut providers = Vec::new();
                for provider in state
                    .stored_provider_order
                    .iter()
                    .filter(|provider| stored_providers.contains(*provider))
                    .chain(stored.iter().map(|row| &row.provider))
                    .chain(runtime_providers.iter())
                    .chain(builtin_providers.iter())
                {
                    if seen.insert(provider.clone()) {
                        providers.push(provider.clone());
                    }
                }
                let mut result = Vec::new();
                for provider in providers {
                    if !local_providers.contains(&provider) || filter.as_ref().is_some_and(|filter| filter != &provider)
                    {
                        continue;
                    }
                    let mut rows = Self::load_provider(store, state, &provider)?;
                    // The public collector also prunes newly imported duplicate
                    // OAuth rows; reload alone cannot cover later store writes.
                    let mut identities = BTreeSet::new();
                    let mut removed = false;
                    for row in rows.iter().rev() {
                        if matches!(row.credential, AuthCredential::OAuth { .. })
                            && let Some(identity) =
                                crate::credential_store::resolve_credential_identity_key(&provider, &row.credential)
                            && !identities.insert(identity)
                        {
                            store
                                .delete_auth_credential(row.id, "deduplicated duplicate credential")
                                .map_err(|_| AuthStorageError::Storage)?;
                            removed = true;
                        }
                    }
                    if removed {
                        state.assignments.reset_provider_assignments(store, &provider);
                        rows = Self::load_provider(store, state, &provider)?;
                    }
                    result.push((provider, rows));
                }
                Ok(result)
            })
            .await?;

        let mut requests = Vec::new();
        for (provider, rows) in rows {
            let Some(hook) = hooks.get(&provider) else { continue };
            let base_url = match &options.base_url_resolver {
                Some(resolve) => resolve(&provider),
                None => options.context.base_url.clone(),
            };
            if provider == "xai-oauth" {
                let mut usable_stored_oauth = false;
                for row in rows.iter().filter(|row| matches!(row.credential, AuthCredential::OAuth { .. })) {
                    let request =
                        build_usage_request(&provider, usage_credential(row, None), Some(row.id), base_url.clone());
                    if hook.supports(&request) {
                        requests.push(request);
                        usable_stored_oauth = true;
                    }
                }
                if !usable_stored_oauth
                    && let Some(token) = self.inner.options.environment.variable("XAI_OAUTH_TOKEN")
                    && !trim_js(&token).is_empty()
                {
                    let credential = transient_usage_credential(UsageCredentialType::Oauth, trim_js(&token).to_owned());
                    let request = build_usage_request(&provider, credential, None, base_url);
                    if hook.supports(&request) {
                        requests.push(request);
                    }
                }
                continue;
            }
            if rows.is_empty() {
                // Nullish precedence is intentional: Some("") suppresses lower
                // legs. Native resolves an extension configuration even when a
                // runtime key is present, so its failure still propagates.
                let extension_key = match extension_keys.get(&provider).filter(|key| !key.is_empty()) {
                    Some(config) => self.inner.options.config_key_resolver.resolve(config, cancel).await?,
                    None => None,
                };
                let env_key = if options.provider.as_deref() == Some(provider.as_str()) {
                    options
                        .context
                        .environment_key
                        .clone()
                        .or_else(|| self.inner.options.environment.api_key(&provider))
                } else {
                    self.inner.options.environment.api_key(&provider)
                };
                let api_key =
                    runtime_keys.get(&provider).cloned().or(extension_key).or_else(|| env_key.map(|key| key.value));
                if let Some(api_key) = api_key.filter(|key| !key.is_empty()) {
                    let credential = transient_usage_credential(UsageCredentialType::ApiKey, api_key);
                    let request = build_usage_request(&provider, credential, None, base_url);
                    if hook.supports(&request) {
                        requests.push(request);
                    }
                }
                continue;
            }
            for row in rows {
                let key = if let AuthCredential::ApiKey { key, .. } = &row.credential {
                    let resolved = self.inner.options.config_key_resolver.resolve(key, cancel).await?;
                    if resolved.as_ref().is_none_or(String::is_empty) {
                        continue;
                    }
                    resolved
                } else {
                    None
                };
                let request =
                    build_usage_request(&provider, usage_credential(&row, key), Some(row.id), base_url.clone());
                if hook.supports(&request) {
                    requests.push(request);
                }
            }
        }
        Ok(requests)
    }

    fn aggregate_forced_usage(&self, requests: &[UsageRequest]) -> ForcedUsageRefresh {
        let all = self.force_usage_marked(None);
        let providers = requests
            .iter()
            .map(|request| request.provider.clone())
            .filter(|provider| all || self.force_usage_marked(Some(provider)))
            .collect();
        ForcedUsageRefresh { all, providers }
    }

    fn clear_aggregate_forced_usage(&self, forced: &ForcedUsageRefresh, epoch: u64) -> Result<(), AuthStorageError> {
        self.store_operation(|store, _state| {
            if epoch != self.inner.usage_epoch.load(Ordering::Acquire) {
                return Ok(());
            }
            for provider in
                forced.providers.iter().map(|provider| Some(provider.as_str())).chain(forced.all.then_some(None))
            {
                write_usage_cache::<Value>(
                    store,
                    &force_cache_key(provider),
                    &UsageCacheEntry { value: None, expires_at: 0.0 },
                    self.now(),
                )
                .map_err(|_| AuthStorageError::Storage)?;
            }
            Ok(())
        })
    }

    async fn fetch_collected_usage(
        &self,
        requests: Vec<UsageRequest>,
        serialized_providers: &BTreeSet<String>,
        cancel: CancellationToken,
    ) -> Result<Vec<Option<UsageReport>>, AuthStorageError> {
        let mut tails = BTreeMap::<String, watch::Receiver<bool>>::new();
        let mut tasks = Vec::with_capacity(requests.len());
        for request in requests {
            let forced = serialized_providers.contains(&request.provider);
            let (previous, completion) = if forced {
                let (sender, receiver) = watch::channel(false);
                (tails.insert(request.provider.clone(), receiver), Some(sender))
            } else {
                (None, None)
            };
            let this = self.clone();
            let cancel = cancel.clone();
            // Promise.all rejection does not cancel siblings or queued native
            // tails. Detached tasks preserve that settlement and are counted.
            this.inner.usage_pending.fetch_add(1, Ordering::AcqRel);
            tasks.push(tokio::spawn(async move {
                let _settlement = UsageSettlement(this.clone());
                let _chain_settlement = UsageChainSettlement(completion);
                if let Some(mut previous) = previous {
                    loop {
                        let ready = *previous.borrow();
                        if ready {
                            break;
                        }
                        if previous.changed().await.is_err() {
                            break;
                        }
                    }
                }
                this.usage_request_report(request, forced, &cancel).await
            }));
        }
        try_join_all(tasks.into_iter().map(|task| async move { task.await.map_err(|_| AuthStorageError::Storage)? }))
            .await
    }
}

struct AggregateFlightSettlement {
    storage: AuthStorage,
    key: String,
    receiver: AggregateUsageFlight,
}
impl Drop for AggregateFlightSettlement {
    fn drop(&mut self) {
        if let Ok(mut flights) = self.storage.inner.aggregate_flights.lock()
            && flights.get(&self.key).is_some_and(|current| current.same_channel(&self.receiver))
        {
            flights.remove(&self.key);
        }
    }
}

struct UsageChainSettlement(Option<watch::Sender<bool>>);
impl Drop for UsageChainSettlement {
    fn drop(&mut self) {
        if let Some(sender) = &self.0 {
            sender.send_replace(true);
        }
    }
}

async fn await_aggregate_usage(
    mut receiver: AggregateUsageFlight,
    cancel: Option<&CancellationToken>,
) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
    loop {
        let ready = receiver.borrow().clone();
        if let Some(result) = ready {
            if let Some(cancel) = cancel {
                check_cancel(cancel)?;
            }
            return result;
        }
        if let Some(cancel) = cancel {
            tokio::select! {
                _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                changed = receiver.changed() => if changed.is_err() { return Err(AuthStorageError::Storage); }
            }
        } else if receiver.changed().await.is_err() {
            return Err(AuthStorageError::Storage);
        }
    }
}

fn transient_usage_credential(credential_type: UsageCredentialType, value: String) -> UsageCredential {
    UsageCredential {
        credential_type,
        api_key: (credential_type == UsageCredentialType::ApiKey).then(|| value.clone()),
        access_token: (credential_type == UsageCredentialType::Oauth).then_some(value),
        refresh_token: None,
        expires_at: None,
        account_id: None,
        project_id: None,
        email: None,
        org_id: None,
        org_name: None,
        enterprise_url: None,
        api_endpoint: None,
        metadata: None,
        unknown_fields: Map::new(),
    }
}

fn build_usage_request(
    provider: &str,
    credential: UsageCredential,
    credential_id: Option<i64>,
    base_url: Option<String>,
) -> UsageRequest {
    UsageRequest {
        provider: provider.to_owned(),
        account_key: usage_identity(&credential),
        credential,
        credential_id,
        base_url,
    }
}

fn aggregate_usage_key(requests: &[UsageRequest]) -> String {
    let mut snapshot: Vec<_> =
        requests.iter().map(|request| usage_report_key(request)["report:".len()..].to_owned()).collect();
    // JavaScript sort compares UTF-16 code units, including supplementary IDs.
    snapshot.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
    let snapshot = snapshot.join("\n");
    format!("reports:{:x}", crate::bun_hash::hash_string(&ara_rpc::WireString::from(snapshot)))
}

fn usage_metadata_value<'a>(report: &'a UsageReport, key: &str) -> Option<&'a str> {
    report.metadata.as_ref()?.get(key)?.as_str().map(trim_js)
}

fn single_usage_scope_value(report: &UsageReport, project: bool) -> Option<&str> {
    let values: BTreeSet<_> = report
        .limits
        .iter()
        .filter_map(|limit| if project { limit.scope.project_id.as_deref() } else { limit.scope.account_id.as_deref() })
        .map(trim_js)
        .filter(|value| !value.is_empty())
        .collect();
    (values.len() == 1).then(|| *values.first().expect("one scope value"))
}

fn usage_report_identifiers(report: &UsageReport) -> Vec<String> {
    let mut identifiers = Vec::new();
    let email = usage_metadata_value(report, "email").filter(|email| !email.is_empty());
    if let Some(email) = email {
        identifiers.push(format!("email:{}", email.to_lowercase()));
    }
    if report.provider == "anthropic" || report.provider == "openai-codex" {
        if identifiers.is_empty()
            && let Some(account) = usage_metadata_value(report, "accountId")
                .or_else(|| single_usage_scope_value(report, false))
                .filter(|value| !value.is_empty())
        {
            identifiers.push(format!("account:{account}"));
        }
        if let Some(org) = usage_metadata_value(report, "orgId").filter(|org| !org.is_empty()) {
            if identifiers.is_empty() {
                return vec![format!("{}:org:{}", report.provider, org.to_lowercase())];
            }
            return identifiers
                .into_iter()
                .map(|identifier| {
                    format!("{}:org:{}|{}", report.provider, org.to_lowercase(), identifier.to_lowercase())
                })
                .collect();
        }
        return identifiers
            .into_iter()
            .map(|identifier| format!("{}:{}", report.provider, identifier.to_lowercase()))
            .collect();
    }
    if email.is_none()
        && let Some(project) = usage_metadata_value(report, "projectId")
            .or_else(|| single_usage_scope_value(report, true))
            .filter(|value| !value.is_empty())
    {
        identifiers.push(format!("project:{project}"));
    }
    for key in ["accountId", "account", "user", "username"] {
        if let Some(account) = usage_metadata_value(report, key).filter(|account| !account.is_empty()) {
            identifiers.push(format!("account:{account}"));
        }
    }
    if let Some(account) = single_usage_scope_value(report, false) {
        identifiers.push(format!("account:{account}"));
    }
    identifiers.into_iter().map(|identifier| format!("{}:{}", report.provider, identifier.to_lowercase())).collect()
}

fn merge_usage_report_group(mut reports: Vec<UsageReport>) -> UsageReport {
    if reports.len() == 1 {
        return reports.pop().expect("nonempty report group");
    }
    reports.sort_by(|left, right| {
        right
            .limits
            .len()
            .cmp(&left.limits.len())
            .then_with(|| right.fetched_at.partial_cmp(&left.fetched_at).unwrap_or(std::cmp::Ordering::Equal))
    });
    let mut reports = reports.into_iter();
    let mut base = reports.next().expect("nonempty report group");
    let mut limit_ids: BTreeSet<_> = base.limits.iter().map(|limit| limit.id.clone()).collect();
    let mut metadata = base.metadata.take().unwrap_or_default();
    for report in reports {
        // Math.max propagates NaN, while f64::max alone would discard it.
        base.fetched_at = if base.fetched_at.is_nan() || report.fetched_at.is_nan() {
            f64::NAN
        } else {
            base.fetched_at.max(report.fetched_at)
        };
        for limit in report.limits {
            if limit_ids.insert(limit.id.clone()) {
                base.limits.push(limit);
            }
        }
        if let Some(other_metadata) = report.metadata {
            for (key, value) in other_metadata {
                metadata.entry(key).or_insert(value);
            }
        }
    }
    base.metadata = (!metadata.is_empty()).then_some(metadata);
    base
}

fn dedupe_usage_reports(reports: Vec<UsageReport>) -> Vec<UsageReport> {
    let mut groups: Vec<Vec<UsageReport>> = Vec::new();
    let mut id_to_group = BTreeMap::<String, usize>::new();
    for report in reports {
        let identifiers = usage_report_identifiers(&report);
        // Native first-match grouping is not a union of all matching groups.
        // A bridge remaps later identifiers without merging older groups.
        let group =
            identifiers.iter().find_map(|identifier| id_to_group.get(identifier).copied()).unwrap_or_else(|| {
                groups.push(Vec::new());
                groups.len() - 1
            });
        groups[group].push(report);
        for identifier in identifiers {
            id_to_group.insert(identifier, group);
        }
    }
    groups.into_iter().map(merge_usage_report_group).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_storage_policy::{UsageAmount, UsageLimit, UsageScope, UsageUnit};
    use std::collections::VecDeque;
    use tokio::sync::Semaphore;

    const NOW: f64 = 1_800_000_000_000.0;

    #[derive(Default)]
    struct Keys {
        values: BTreeMap<String, Result<Option<String>, AuthStorageError>>,
        calls: Mutex<Vec<String>>,
    }
    #[async_trait]
    impl ConfigKeyResolver for Keys {
        async fn resolve(
            &self,
            configuration: &str,
            cancel: &CancellationToken,
        ) -> Result<Option<String>, AuthStorageError> {
            check_cancel(cancel)?;
            self.calls.lock().unwrap().push(configuration.to_owned());
            self.values.get(configuration).cloned().unwrap_or_else(|| Ok(Some(configuration.to_owned())))
        }
    }

    #[derive(Default)]
    struct UsageEnvironment {
        keys: BTreeMap<String, String>,
        named: BTreeMap<String, String>,
    }
    impl Environment for UsageEnvironment {
        fn api_key(&self, provider: &str) -> Option<EnvironmentKey> {
            self.keys
                .get(provider)
                .map(|value| EnvironmentKey { variable: format!("{provider}_KEY"), value: value.clone() })
        }
        fn variable(&self, name: &str) -> Option<String> {
            self.named.get(name).cloned()
        }
    }

    struct CaptureUsage {
        calls: Mutex<Vec<UsageRequest>>,
        responses: Mutex<BTreeMap<String, Option<UsageReport>>>,
        gates: Mutex<VecDeque<Arc<Semaphore>>>,
        started: Semaphore,
        oauth_only: bool,
        headers: bool,
    }
    impl Default for CaptureUsage {
        fn default() -> Self {
            Self {
                calls: Mutex::new(Vec::new()),
                responses: Mutex::new(BTreeMap::new()),
                gates: Mutex::new(VecDeque::new()),
                started: Semaphore::new(0),
                oauth_only: false,
                headers: false,
            }
        }
    }
    #[async_trait]
    impl UsageProvider for CaptureUsage {
        async fn fetch_usage(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, UsageFetchError> {
            let identity = request
                .credential
                .account_id
                .clone()
                .or_else(|| request.credential.api_key.clone())
                .unwrap_or_else(|| "env-oauth".to_owned());
            self.calls.lock().unwrap().push(request.clone());
            let gate = self.gates.lock().unwrap().pop_front();
            self.started.add_permits(1);
            if let Some(gate) = gate {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(UsageFetchError::Cancelled),
                    permit = gate.acquire() => permit.unwrap().forget(),
                }
            }
            let responses = self.responses.lock().unwrap();
            Ok(responses.get(&identity).cloned().unwrap_or_else(|| {
                Some(report(&request.provider, serde_json::json!({"accountId":identity}), &["quota"], NOW))
            }))
        }
        fn supports(&self, request: &UsageRequest) -> bool {
            match request.credential.credential_type {
                UsageCredentialType::Oauth => {
                    request.credential.access_token.as_ref().is_some_and(|token| !token.is_empty())
                }
                UsageCredentialType::ApiKey => {
                    !self.oauth_only && request.credential.api_key.as_ref().is_some_and(|key| !key.is_empty())
                }
            }
        }
        fn rate_limit_header_parser(&self) -> Option<RateLimitHeaderParser> {
            self.headers.then_some(test_header_report)
        }
    }

    struct AggregateFixture {
        results: Mutex<VecDeque<Result<Option<Vec<UsageReport>>, AuthStorageError>>>,
        gates: Mutex<VecDeque<Arc<Semaphore>>>,
        calls: AtomicUsize,
        started: Semaphore,
        get_calls: Mutex<Vec<UsageRequest>>,
        per_report: Mutex<Option<UsageReport>>,
        ingests: AtomicUsize,
        accept_ingest: std::sync::atomic::AtomicBool,
        invalidations: AtomicUsize,
        notifications: AtomicUsize,
    }
    impl Default for AggregateFixture {
        fn default() -> Self {
            Self {
                results: Mutex::new(VecDeque::new()),
                gates: Mutex::new(VecDeque::new()),
                calls: AtomicUsize::new(0),
                started: Semaphore::new(0),
                get_calls: Mutex::new(Vec::new()),
                per_report: Mutex::new(None),
                ingests: AtomicUsize::new(0),
                accept_ingest: std::sync::atomic::AtomicBool::new(false),
                invalidations: AtomicUsize::new(0),
                notifications: AtomicUsize::new(0),
            }
        }
    }
    #[async_trait]
    impl AggregateUsageSource for AggregateFixture {
        async fn fetch_usage_reports(&self) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            let gate = self.gates.lock().unwrap().pop_front();
            self.started.add_permits(1);
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            self.results.lock().unwrap().pop_front().unwrap_or(Ok(Some(Vec::new())))
        }
    }
    #[async_trait]
    impl UsageStoreHooks for AggregateFixture {
        async fn get_usage_report(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, AuthStorageError> {
            check_cancel(cancel)?;
            self.get_calls.lock().unwrap().push(request);
            Ok(self.per_report.lock().unwrap().clone())
        }
        fn ingest_usage_report(&self, _request: &UsageRequest, _report: UsageReport) -> bool {
            self.ingests.fetch_add(1, Ordering::AcqRel);
            self.accept_ingest.load(Ordering::Acquire)
        }
        fn invalidate_usage_cache(&self) {
            self.invalidations.fetch_add(1, Ordering::AcqRel);
        }
        async fn notify_usage_stale(&self, _cancel: &CancellationToken) -> Result<(), AuthStorageError> {
            self.notifications.fetch_add(1, Ordering::AcqRel);
            Err(AuthStorageError::Transient)
        }
    }

    fn report(provider: &str, metadata: Value, ids: &[&str], fetched_at: f64) -> UsageReport {
        UsageReport {
            provider: provider.to_owned(),
            fetched_at,
            metadata: metadata.as_object().cloned(),
            limits: ids
                .iter()
                .map(|id| UsageLimit {
                    id: (*id).to_owned(),
                    label: (*id).to_owned(),
                    scope: UsageScope { provider: provider.to_owned(), ..Default::default() },
                    amount: UsageAmount { used_fraction: Some(0.2), unit: UsageUnit::Percent, ..Default::default() },
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }
    fn test_header_report(_headers: &BTreeMap<String, String>, now: f64) -> Option<UsageReport> {
        Some(report(
            "openai-codex",
            serde_json::json!({"accountId":"acct","allowed":true,"limitReached":false}),
            &["openai-codex:primary"],
            now,
        ))
    }
    fn oauth(account: &str) -> AuthCredential {
        AuthCredential::oauth(serde_json::json!({"access":format!("access-{account}"),"refresh":format!("refresh-{account}"),"expires":NOW+7_200_000.0,"accountId":account}).as_object().unwrap().clone())
    }
    fn new_storage(options: AuthStorageOptions, credentials: &[(&str, Vec<AuthCredential>)]) -> AuthStorage {
        let store = Arc::new(Mutex::new(
            SqliteCredentialStore::from_connection(
                rusqlite::Connection::open_in_memory().unwrap(),
                Duration::from_millis(20),
            )
            .unwrap(),
        ));
        for (provider, credentials) in credentials {
            store.lock().unwrap().replace_auth_credentials_for_provider(provider, credentials).unwrap();
        }
        AuthStorage::new(store, None, options).unwrap()
    }
    fn options() -> AuthStorageOptions {
        AuthStorageOptions {
            clock: Arc::new(|| NOW),
            jitter: Arc::new(|| 0.5),
            usage_request_timeout: Duration::from_secs(2),
            ..Default::default()
        }
    }
    async fn started(semaphore: &Semaphore, count: u32) {
        tokio::time::timeout(Duration::from_secs(2), semaphore.acquire_many(count))
            .await
            .expect("bounded usage start")
            .unwrap()
            .forget();
    }
    fn secret(request: &UsageRequest) -> Option<&str> {
        request.credential.api_key.as_deref().or(request.credential.access_token.as_deref())
    }
    async fn report_provider_order(storage: &AuthStorage) -> Vec<String> {
        storage
            .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
            .await
            .unwrap()
            .unwrap()
            .into_iter()
            .map(|report| report.provider)
            .collect()
    }

    /// Pinned 3714–3796, auth-storage-xai-oauth-usage and usage-history
    /// reference resolution: stored/runtime/extension/env, nullish precedence,
    /// resolver failures, provider supports, URL/context isolation, dedicated xAI.
    #[tokio::test]
    async fn native_usage_collector_sources_and_projection_family() {
        let keys = Arc::new(Keys {
            values: BTreeMap::from([
                ("!stored".into(), Ok(Some("stored-wire".into()))),
                ("!missing".into(), Ok(None)),
                ("!extension".into(), Ok(Some("extension-wire".into()))),
                ("!empty".into(), Ok(Some(String::new()))),
                ("!error".into(), Err(AuthStorageError::Configuration)),
            ]),
            ..Default::default()
        });
        let environment = Arc::new(UsageEnvironment {
            keys: BTreeMap::from([
                ("stored".into(), "stored-env".into()),
                ("env".into(), "env-wire".into()),
                ("extension".into(), "extension-env".into()),
                ("empty".into(), "empty-env".into()),
                ("xai-oauth".into(), "paid-wire".into()),
            ]),
            named: BTreeMap::from([("XAI_OAUTH_TOKEN".into(), " \u{feff}oauth-env\u{feff} ".into())]),
        });
        let storage = new_storage(
            AuthStorageOptions { config_key_resolver: keys.clone(), environment, ..options() },
            &[
                ("stored", vec![AuthCredential::api_key("!stored"), oauth("stored-account")]),
                ("unresolved", vec![AuthCredential::api_key("!missing")]),
                ("no-provider", vec![AuthCredential::api_key("unused")]),
                ("xai-oauth", vec![AuthCredential::api_key("!paid")]),
            ],
        );
        for provider in ["stored", "unresolved", "env"] {
            storage.register_usage_provider(provider, Arc::new(CaptureUsage::default())).unwrap();
        }
        storage
            .register_usage_provider_with_key("empty", Arc::new(CaptureUsage::default()), Some("!empty".into()))
            .unwrap();
        storage
            .register_usage_provider_with_key("extension", Arc::new(CaptureUsage::default()), Some("!extension".into()))
            .unwrap();
        storage
            .register_usage_provider_with_key(
                "xai-oauth",
                Arc::new(CaptureUsage { oauth_only: true, ..Default::default() }),
                Some("!error".into()),
            )
            .unwrap();
        storage.set_runtime_api_key("stored", "masked-runtime".into()).unwrap();
        storage.set_runtime_api_key("unresolved", "masked-unresolved".into()).unwrap();
        let context = AuthRequestContext {
            base_url: Some("https://legacy.example/".into()),
            environment_key: Some(EnvironmentKey { variable: "PRIVATE_ROUTE".into(), value: "route-private".into() }),
            ..Default::default()
        };
        let all = FetchUsageReportsOptions { context: context.clone(), ..Default::default() };
        let requests = storage.collect_usage_requests(&all, &CancellationToken::new()).await.unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests.iter().all(|request| request.base_url == context.base_url));
        assert_eq!(
            requests.iter().filter(|request| request.provider == "stored").map(secret).collect::<Vec<_>>(),
            [Some("stored-wire"), Some("access-stored-account")]
        );
        assert!(
            !requests.iter().any(|request| ["unresolved", "empty", "no-provider"].contains(&request.provider.as_str()))
        );
        assert_eq!(secret(requests.iter().find(|request| request.provider == "env").unwrap()), Some("env-wire"));
        assert_eq!(
            secret(requests.iter().find(|request| request.provider == "extension").unwrap()),
            Some("extension-wire")
        );
        let xai = requests.iter().find(|request| request.provider == "xai-oauth").unwrap();
        assert_eq!(xai.credential.credential_type, UsageCredentialType::Oauth);
        assert_eq!(secret(xai), Some("oauth-env"));
        assert_eq!(xai.credential_id, None);
        assert!(!keys.calls.lock().unwrap().iter().any(|value| value == "!paid" || value == "!error"));

        let resolved_urls = FetchUsageReportsOptions {
            base_url_resolver: Some(Arc::new(|provider| {
                (provider == "stored").then(|| "https://stored.example".into())
            })),
            ..all.clone()
        };
        let requests = storage.collect_usage_requests(&resolved_urls, &CancellationToken::new()).await.unwrap();
        assert!(
            requests
                .iter()
                .all(|request| request.base_url
                    == (request.provider == "stored").then(|| "https://stored.example".into()))
        );
        let selected =
            FetchUsageReportsOptions { provider: Some("env".into()), context: context.clone(), ..Default::default() };
        let requests = storage.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(secret(&requests[0]), Some("route-private"));

        storage.set_runtime_api_key("extension", "runtime-wire".into()).unwrap();
        let selected = FetchUsageReportsOptions { provider: Some("extension".into()), ..Default::default() };
        assert_eq!(
            secret(&storage.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap()[0]),
            Some("runtime-wire")
        );
        storage.set_runtime_api_key("extension", String::new()).unwrap();
        assert!(storage.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap().is_empty());
        storage.remove_runtime_api_key("extension").unwrap();
        storage
            .register_usage_provider_with_key("extension", Arc::new(CaptureUsage::default()), Some("!empty".into()))
            .unwrap();
        assert!(storage.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap().is_empty());
        storage.set_runtime_api_key("extension", "runtime-wire".into()).unwrap();
        storage
            .register_usage_provider_with_key("extension", Arc::new(CaptureUsage::default()), Some("!error".into()))
            .unwrap();
        assert!(matches!(
            storage.collect_usage_requests(&selected, &CancellationToken::new()).await,
            Err(AuthStorageError::Configuration)
        ));

        // Ordinary provider replacement removes extension-owned configuration.
        storage.remove_runtime_api_key("extension").unwrap();
        storage.register_usage_provider("extension", Arc::new(CaptureUsage::default())).unwrap();
        assert_eq!(
            secret(&storage.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap()[0]),
            Some("extension-env")
        );
        let empty = new_storage(options(), &[]);
        assert_eq!(
            empty.fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new()).await.unwrap(),
            None
        );
        empty.register_usage_provider("supported", Arc::new(CaptureUsage::default())).unwrap();
        assert_eq!(
            empty.fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new()).await.unwrap(),
            Some(Vec::new())
        );

        // Usable stored OAuth suppresses the dedicated bearer; unusable stored
        // OAuth or paid-only material falls back solely to XAI_OAUTH_TOKEN.
        for (credential, bearer) in [
            (oauth("stored-xai"), Some("access-stored-xai")),
            (AuthCredential::oauth(serde_json::json!({"access":""}).as_object().unwrap().clone()), Some("dedicated")),
            (AuthCredential::api_key("paid"), Some("dedicated")),
        ] {
            let storage = new_storage(
                AuthStorageOptions {
                    environment: Arc::new(UsageEnvironment {
                        keys: BTreeMap::from([("xai-oauth".into(), "paid-env".into())]),
                        named: BTreeMap::from([("XAI_OAUTH_TOKEN".into(), "dedicated".into())]),
                    }),
                    ..options()
                },
                &[("xai-oauth", vec![credential])],
            );
            storage
                .register_usage_provider("xai-oauth", Arc::new(CaptureUsage { oauth_only: true, ..Default::default() }))
                .unwrap();
            let requests = storage
                .collect_usage_requests(&FetchUsageReportsOptions::default(), &CancellationToken::new())
                .await
                .unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(secret(&requests[0]), bearer);
        }
        let paid_only = new_storage(
            AuthStorageOptions {
                environment: Arc::new(UsageEnvironment {
                    keys: BTreeMap::from([("xai-oauth".into(), "paid-env".into())]),
                    ..Default::default()
                }),
                ..options()
            },
            &[],
        );
        paid_only
            .register_usage_provider("xai-oauth", Arc::new(CaptureUsage { oauth_only: true, ..Default::default() }))
            .unwrap();
        assert!(
            paid_only
                .collect_usage_requests(&FetchUsageReportsOptions::default(), &CancellationToken::new())
                .await
                .unwrap()
                .is_empty()
        );

        // Import two equal identities after construction, then let the public
        // collector keep the latest row and persistently disable the older one.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.sqlite");
        let store = Arc::new(Mutex::new(
            SqliteCredentialStore::from_connection(
                rusqlite::Connection::open(&path).unwrap(),
                Duration::from_millis(20),
            )
            .unwrap(),
        ));
        let imported = AuthStorage::new(store.clone(), None, options()).unwrap();
        imported.register_usage_provider("imported", Arc::new(CaptureUsage::default())).unwrap();
        let writer = rusqlite::Connection::open(&path).unwrap();
        for access in ["older", "newer"] {
            writer
                .execute(
                    "INSERT INTO auth_credentials(provider,credential_type,data) VALUES('imported','oauth',?1)",
                    [serde_json::json!({"access":access,"accountId":"same"}).to_string()],
                )
                .unwrap();
        }
        let requests = imported
            .collect_usage_requests(&FetchUsageReportsOptions::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(secret(&requests[0]), Some("newer"));
        assert_eq!(store.lock().unwrap().list_auth_credentials(Some("imported")).unwrap().len(), 1);
        assert_eq!(store.lock().unwrap().list_disabled_credentials(Some("imported")).unwrap().len(), 1);

        let builtin = Arc::new(CaptureUsage::default());
        let replacing = Arc::new(CaptureUsage { oauth_only: true, ..Default::default() });
        empty.inner.builtin_usage_providers.lock().unwrap().insert("builtin".into(), builtin.clone());
        empty.set_runtime_api_key("builtin", "builtin-key".into()).unwrap();
        empty.register_usage_provider("builtin", replacing).unwrap();
        let selected = FetchUsageReportsOptions { provider: Some("builtin".into()), ..Default::default() };
        assert!(empty.collect_usage_requests(&selected, &CancellationToken::new()).await.unwrap().is_empty());
        empty.unregister_usage_provider("builtin").unwrap();
        assert_eq!(
            empty.fetch_usage_reports_with_options(&selected, &CancellationToken::new()).await.unwrap().unwrap().len(),
            1
        );
        assert_eq!(builtin.calls.lock().unwrap().len(), 1);

        // Native Set union preserves each source's first appearance, and a
        // Map replacement does not move its provider. A removed/re-added
        // override moves to the tail; removing Codex restores the default leg.
        let ordered = new_storage(
            options(),
            &[
                ("stored-z", vec![AuthCredential::api_key("stored-z-wire")]),
                ("stored-a", vec![AuthCredential::api_key("stored-a-wire")]),
            ],
        );
        ordered
            .inner
            .builtin_usage_providers
            .lock()
            .unwrap()
            .insert("openai-codex".into(), Arc::new(CaptureUsage::default()));
        for (provider, key) in [
            ("openai-codex", "default-wire"),
            ("runtime-z", "runtime-z-wire"),
            ("runtime-a", "runtime-a-wire"),
            ("unregistered", "unused-wire"),
        ] {
            ordered.set_runtime_api_key(provider, key.into()).unwrap();
        }
        for provider in ["stored-z", "openai-codex", "runtime-z", "stored-a", "runtime-a"] {
            ordered.register_usage_provider(provider, Arc::new(CaptureUsage::default())).unwrap();
        }
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-z", "stored-a", "openai-codex", "runtime-z", "runtime-a"]
        );
        ordered
            .register_usage_provider_with_key(
                "runtime-z",
                Arc::new(CaptureUsage::default()),
                Some("replacement-key".into()),
            )
            .unwrap();
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-z", "stored-a", "openai-codex", "runtime-z", "runtime-a"]
        );
        ordered.unregister_usage_provider("openai-codex").unwrap();
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-z", "stored-a", "runtime-z", "runtime-a", "openai-codex"]
        );
        ordered.unregister_usage_provider("runtime-z").unwrap();
        ordered.register_usage_provider("runtime-z", Arc::new(CaptureUsage::default())).unwrap();
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-z", "stored-a", "runtime-a", "runtime-z", "openai-codex"]
        );
        let epoch = ordered.inner.usage_epoch.load(Ordering::Acquire);
        let generation = ordered.generation();
        ordered.unregister_usage_provider("never-registered").unwrap();
        assert_eq!(ordered.inner.usage_epoch.load(Ordering::Acquire), epoch);
        assert_eq!(ordered.generation(), generation);

        // Reload preserves the surviving Map position even when its oldest
        // durable row disappears. A fully removed provider loses that position
        // and is appended when a later login reintroduces it.
        let oldest_z = ordered.provider_rows("stored-z").await.unwrap()[0].id;
        ordered
            .inner
            .store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider("stored-z", &AuthCredential::api_key("stored-z-new-wire"))
            .unwrap();
        ordered.reload().unwrap();
        ordered.inner.store.lock().unwrap().delete_auth_credential(oldest_z, "order fixture account removed").unwrap();
        ordered.reload().unwrap();
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-z", "stored-a", "runtime-a", "runtime-z", "openai-codex"]
        );
        ordered.inner.store.lock().unwrap().replace_auth_credentials_for_provider("stored-z", &[]).unwrap();
        ordered.reload().unwrap();
        ordered
            .inner
            .store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider("stored-z", &AuthCredential::api_key("stored-z-returned-wire"))
            .unwrap();
        ordered.reload().unwrap();
        assert_eq!(
            report_provider_order(&ordered).await,
            ["stored-a", "stored-z", "runtime-a", "runtime-z", "openai-codex"]
        );
    }

    /// Pinned 3799–3940 and org-scoped usage tests: merge richness/newness,
    /// null-presence, scope uniqueness, email/project and non-union bridge order.
    #[test]
    fn native_usage_report_identity_merge_and_dedupe_family() {
        for provider in ["anthropic", "openai-codex"] {
            let reports = vec![
                report(
                    provider,
                    serde_json::json!({"email":" USER@EXAMPLE.COM ","accountId":"shared","orgId":"Team"}),
                    &["one"],
                    1.0,
                ),
                report(
                    provider,
                    serde_json::json!({"email":"user@example.com","accountId":"other","orgId":"team"}),
                    &["two"],
                    2.0,
                ),
                report(
                    provider,
                    serde_json::json!({"email":"user@example.com","accountId":"shared","orgId":"personal"}),
                    &["one"],
                    3.0,
                ),
                report(provider, serde_json::json!({"email":"user@example.com"}), &["one"], 4.0),
            ];
            let deduped = dedupe_usage_reports(reports);
            assert_eq!(deduped.len(), 3);
            assert_eq!(deduped[0].limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(), ["two", "one"]);
            assert_eq!(deduped[0].metadata.as_ref().unwrap()["accountId"], "other");
            let no_email = vec![
                report(provider, serde_json::json!({"accountId":"account","orgId":"org-a"}), &["one"], 1.0),
                report(provider, serde_json::json!({"accountId":"account","orgId":"org-b"}), &["one"], 1.0),
            ];
            assert_eq!(dedupe_usage_reports(no_email).len(), 2);
            assert_eq!(
                usage_report_identifiers(&report(provider, serde_json::json!({"orgId":" Org "}), &[], 1.0)),
                [format!("{provider}:org:org")]
            );
            let mut scope = report(provider, Value::Null, &["one", "two"], 1.0);
            scope.limits[0].scope.account_id = Some("account".into());
            scope.limits[1].scope.account_id = Some(" account ".into());
            assert_eq!(usage_report_identifiers(&scope), [format!("{provider}:account:account")]);
            scope.metadata = Some(serde_json::json!({"accountId":" "}).as_object().unwrap().clone());
            assert!(usage_report_identifiers(&scope).is_empty(), "empty metadata string suppresses scope fallback");
            scope.metadata = Some(serde_json::json!({"accountId":null}).as_object().unwrap().clone());
            assert_eq!(usage_report_identifiers(&scope), [format!("{provider}:account:account")]);
            scope.limits[1].scope.account_id = Some("different".into());
            assert!(usage_report_identifiers(&scope).is_empty(), "more than one distinct scope is anonymous");
        }
        let users = vec![
            report("google", serde_json::json!({"email":"first","projectId":"same-project"}), &["one"], 1.0),
            report("google", serde_json::json!({"email":"second","projectId":"same-project"}), &["one"], 1.0),
        ];
        assert_eq!(dedupe_usage_reports(users).len(), 2);
        let cross_provider = vec![
            report("a", serde_json::json!({"accountId":"same"}), &[], 1.0),
            report("b", serde_json::json!({"accountId":"same"}), &[], 1.0),
            report("a", Value::Null, &[], 1.0),
            report("a", Value::Null, &[], 1.0),
        ];
        assert_eq!(dedupe_usage_reports(cross_provider).len(), 4);
        let mut project = report("google", Value::Null, &["one", "two"], 1.0);
        for limit in &mut project.limits {
            limit.scope.project_id = Some("same-project".into());
        }
        assert_eq!(usage_report_identifiers(&project), ["google:project:same-project"]);
        project.metadata = Some(serde_json::json!({"projectId":""}).as_object().unwrap().clone());
        assert!(usage_report_identifiers(&project).is_empty());
        let bridge = dedupe_usage_reports(vec![
            report("other", serde_json::json!({"accountId":"left"}), &["left"], 1.0),
            report("other", serde_json::json!({"accountId":"right"}), &["right"], 1.0),
            report("other", serde_json::json!({"accountId":"left","account":"right"}), &["bridge"], 1.0),
            report("other", serde_json::json!({"accountId":"right"}), &["later"], 1.0),
        ]);
        assert_eq!(bridge.len(), 2);
        assert_eq!(
            bridge[0].limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(),
            ["left", "bridge", "later"]
        );
        assert_eq!(bridge[1].limits[0].id, "right");

        let mut rich = report(
            "a",
            serde_json::json!({"accountId":"same","retained":null}),
            &["duplicate", "duplicate", "base"],
            1.0,
        );
        rich.notes = Some(vec!["base note".into()]);
        rich.raw = Some(serde_json::json!({"base":true}));
        rich.reset_credits = Some(policy::UsageResetCredits { available_count: 2.0, ..Default::default() });
        rich.unknown_fields.insert("future".into(), Value::String("preserved".into()));
        let mut fresh = report(
            "a",
            serde_json::json!({"accountId":"same","retained":"lower","filled":true}),
            &["duplicate", "new"],
            9.0,
        );
        fresh.notes = Some(vec!["lower note".into()]);
        fresh.raw = Some(Value::Bool(false));
        let merged = dedupe_usage_reports(vec![fresh, rich]).pop().unwrap();
        assert_eq!(merged.fetched_at, 9.0);
        assert_eq!(
            merged.limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(),
            ["duplicate", "duplicate", "base", "new"]
        );
        assert_eq!(merged.metadata.as_ref().unwrap()["retained"], Value::Null);
        assert_eq!(merged.metadata.as_ref().unwrap()["filled"], true);
        assert_eq!(merged.notes.unwrap(), ["base note"]);
        assert_eq!(merged.raw, Some(serde_json::json!({"base":true})));
        assert_eq!(merged.reset_credits.unwrap().available_count, 2.0);
        assert_eq!(merged.unknown_fields["future"], "preserved");
        let singleton = report("a", serde_json::json!({}), &[], 1.0);
        assert_eq!(dedupe_usage_reports(vec![singleton.clone()]), [singleton]);
        assert!(
            merge_usage_report_group(vec![report("a", Value::Null, &[], f64::NAN), report("a", Value::Null, &[], 2.0)])
                .fetched_at
                .is_nan()
        );
        let newest = merge_usage_report_group(vec![
            report("a", serde_json::json!({"winner":"old"}), &["one"], 1.0),
            report("a", serde_json::json!({"winner":"new"}), &["two"], 2.0),
        ]);
        assert_eq!(newest.metadata.unwrap()["winner"], "new");

        // Pinned 4033–4055: fallback requires quantitative explicit/shared
        // scope; ambiguous tiers/labels do not identify a model.
        let storage = new_storage(options(), &[]);
        let models = vec!["model-b".into(), "model-a".into(), "model-b".into(), "unknown".into()];
        let mut scoped = report("unknown-provider", Value::Null, &["tier", "explicit", "label-only"], NOW);
        scoped.limits[0].scope.tier = Some("ambiguous".into());
        scoped.limits[1].scope.model_id = Some("model-a".into());
        scoped.limits[2].scope.model_id = Some("model-b".into());
        scoped.limits[2].amount = UsageAmount::default();
        assert_eq!(storage.get_usage_reporting_model_ids("unknown-provider", &models, &[scoped.clone()]), ["model-a"]);
        scoped.limits[1].scope.shared = Some(true);
        assert_eq!(
            storage.get_usage_reporting_model_ids("unknown-provider", &models, &[scoped]),
            ["model-b", "model-a", "unknown"]
        );
        assert!(storage.get_usage_reporting_model_ids("other-provider", &models, &[]).is_empty());
    }

    /// Pinned 4258–4283/4116–4130/3631–3709/6271–6286:
    /// independent waiter cancellation, override precedence/nullability,
    /// store-only healing, authoritative per-OAuth None, ingest and invalidation.
    #[tokio::test]
    async fn native_usage_override_store_and_cancellation_family() {
        let explicit = Arc::new(AggregateFixture::default());
        let bound_store = Arc::new(AggregateFixture::default());
        let gate = Arc::new(Semaphore::new(0));
        explicit.gates.lock().unwrap().push_back(gate.clone());
        let storage = new_storage(
            AuthStorageOptions {
                aggregate_usage_override: Some(explicit.clone()),
                usage_store: Some(bound_store.clone()),
                ..options()
            },
            &[],
        );
        let cancelled = CancellationToken::new();
        let caller_cancel = cancelled.clone();
        let owner = storage.clone();
        let first = tokio::spawn(async move {
            owner.fetch_usage_reports(None, &AuthRequestContext::default(), &caller_cancel).await
        });
        started(&explicit.started, 1).await;
        let peer_context = AuthRequestContext::default();
        let peer_cancel = CancellationToken::new();
        let peer = storage.fetch_usage_reports(None, &peer_context, &peer_cancel);
        tokio::pin!(peer);
        assert!(futures::poll!(peer.as_mut()).is_pending(), "peer joined the existing override owner");
        cancelled.cancel();
        assert!(matches!(first.await.unwrap(), Err(AuthStorageError::Cancelled)));
        assert_eq!(explicit.calls.load(Ordering::Acquire), 1);
        assert_eq!(bound_store.calls.load(Ordering::Acquire), 0);
        gate.add_permits(1);
        assert_eq!(peer.await.unwrap(), Some(Vec::new()));
        storage.wait_for_settlement().await;
        assert!(storage.inner.aggregate_flights.lock().unwrap().is_empty());
        for (source_result, expected) in [
            (Ok(None), Ok(None)),
            (Ok(Some(Vec::new())), Ok(Some(Vec::new()))),
            (Err(AuthStorageError::Transient), Err(AuthStorageError::Transient)),
        ] {
            explicit.results.lock().unwrap().push_back(source_result);
            assert_eq!(
                storage.fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new()).await,
                expected
            );
            storage.wait_for_settlement().await;
        }
        assert_eq!(explicit.calls.load(Ordering::Acquire), 4, "aggregate has no completed-result cache");

        // An already-cancelled override caller still leaves an owned fetch,
        // consistent with constructing the native shared Promise before race.
        let gate = Arc::new(Semaphore::new(0));
        explicit.gates.lock().unwrap().push_back(gate.clone());
        let already_cancelled = CancellationToken::new();
        already_cancelled.cancel();
        assert!(matches!(
            storage.fetch_usage_reports(None, &AuthRequestContext::default(), &already_cancelled).await,
            Err(AuthStorageError::Cancelled)
        ));
        started(&explicit.started, 4).await; // Three prior ready calls plus this source.
        gate.add_permits(1);
        storage.wait_for_settlement().await;

        let healthy = report(
            "openai-codex",
            serde_json::json!({"accountId":"acct","allowed":true,"limitReached":false}),
            &["openai-codex:primary"],
            NOW,
        );
        for override_wins in [false, true] {
            let source = Arc::new(AggregateFixture::default());
            source.results.lock().unwrap().push_back(Ok(Some(vec![healthy.clone()])));
            let broker = Arc::new(AggregateFixture::default());
            broker.results.lock().unwrap().push_back(Ok(Some(vec![healthy.clone()])));
            let storage = new_storage(
                AuthStorageOptions {
                    aggregate_usage_override: override_wins.then(|| source.clone() as Arc<dyn AggregateUsageSource>),
                    usage_store: Some(broker.clone()),
                    ..options()
                },
                &[("openai-codex", vec![oauth("acct")])],
            );
            let row = storage.provider_rows("openai-codex").await.unwrap().remove(0);
            storage
                .upsert_credential_block(&StoredCredentialBlock {
                    credential_id: row.id,
                    provider_key: provider_type_key("openai-codex", CredentialKind::OAuth),
                    block_scope: "chat".into(),
                    blocked_until_ms: (NOW + 60_000.0) as i64,
                    updated_at_ms: NOW as i64,
                })
                .unwrap();
            let reports = storage
                .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reports.as_slice(), std::slice::from_ref(&healthy));
            assert_eq!(
                storage.list_credential_blocks(&[row.id]).unwrap().len(),
                usize::from(override_wins),
                "only the store aggregate heals blocks"
            );
            assert_eq!(source.calls.load(Ordering::Acquire), usize::from(override_wins));
            assert_eq!(broker.calls.load(Ordering::Acquire), usize::from(!override_wins));

            // Broker per-account None is authoritative even with an installed
            // local implementation. API-key rows still use that implementation.
            let local = Arc::new(CaptureUsage { headers: true, ..Default::default() });
            storage.register_usage_provider("openai-codex", local.clone()).unwrap();
            assert_eq!(
                storage
                    .usage_report(
                        "openai-codex",
                        &row,
                        &AuthRequestContext::default(),
                        false,
                        &CancellationToken::new()
                    )
                    .await
                    .unwrap(),
                None
            );
            assert_eq!(local.calls.lock().unwrap().len(), 0);
            assert_eq!(broker.get_calls.lock().unwrap().len(), 1);
            let headers = BTreeMap::from([("observed".into(), "1".into())]);
            let context = AuthRequestContext::default();
            assert!(!storage.ingest_usage_headers("openai-codex", &headers, None, None).unwrap());
            if override_wins {
                assert_eq!(broker.ingests.load(Ordering::Acquire), 0);
            } else {
                assert_eq!(broker.ingests.load(Ordering::Acquire), 1);
                broker.accept_ingest.store(true, Ordering::Release);
                assert!(storage.ingest_usage_headers("openai-codex", &headers, None, None).unwrap());
                assert!(!storage.ingest_usage_headers("openai-codex", &headers, None, None).unwrap());
                assert_eq!(
                    broker.ingests.load(Ordering::Acquire),
                    2,
                    "only a successful ingest consumes the throttle slot"
                );
            }
            storage.invalidate_usage_cache_and_notify(None, &CancellationToken::new()).await.unwrap();
            assert_eq!(broker.invalidations.load(Ordering::Acquire), 1);
            assert_eq!(broker.notifications.load(Ordering::Acquire), 1);
            assert!(!storage.force_usage_marked(None));
            assert!(!storage.force_usage_marked(Some("openai-codex")));

            storage
                .inner
                .store
                .lock()
                .unwrap()
                .replace_auth_credentials_for_provider("api-local", &[AuthCredential::api_key("actual")])
                .unwrap();
            storage.register_usage_provider("api-local", local.clone()).unwrap();
            let api = storage.provider_rows("api-local").await.unwrap().remove(0);
            assert!(
                storage
                    .usage_report("api-local", &api, &context, false, &CancellationToken::new())
                    .await
                    .unwrap()
                    .is_some()
            );
            assert_eq!(local.calls.lock().unwrap().len(), 1);
            assert_eq!(broker.get_calls.lock().unwrap().len(), 1);
        }
    }

    /// Pinned 3187–3230/4221–4344 plus usage-cache jitter/invalidation:
    /// local signal boundary, provider tails/parallel fanout, cohort key,
    /// epoch-fenced force consumption, participating providers, no outer cache.
    #[tokio::test]
    async fn native_usage_local_force_epoch_and_decorrelation_family() {
        let storage = new_storage(
            options(),
            &[
                ("force-a", vec![AuthCredential::api_key("a1"), AuthCredential::api_key("a2")]),
                ("force-b", vec![AuthCredential::api_key("b1"), AuthCredential::api_key("b2")]),
            ],
        );
        let a = Arc::new(CaptureUsage::default());
        let b = Arc::new(CaptureUsage::default());
        let gate = Arc::new(Semaphore::new(0));
        for hook in [&a, &b] {
            hook.gates.lock().unwrap().extend([gate.clone(), gate.clone()]);
        }
        storage.register_usage_provider("force-a", a.clone()).unwrap();
        storage.register_usage_provider("force-b", b.clone()).unwrap();
        storage.invalidate_usage_cache(None).unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let owner = storage.clone();
        let first =
            tokio::spawn(
                async move { owner.fetch_usage_reports(None, &AuthRequestContext::default(), &cancelled).await },
            );
        started(&a.started, 1).await;
        started(&b.started, 1).await;
        assert_eq!(a.calls.lock().unwrap().len(), 1);
        assert_eq!(b.calls.lock().unwrap().len(), 1);
        let owner = storage.clone();
        let peer = tokio::spawn(async move {
            owner.fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new()).await
        });
        gate.add_permits(2);
        started(&a.started, 1).await;
        started(&b.started, 1).await;
        gate.add_permits(2);
        assert_eq!(first.await.unwrap().unwrap().unwrap().len(), 4, "local aggregate ignores caller cancellation");
        assert_eq!(peer.await.unwrap().unwrap().unwrap().len(), 4);
        storage.wait_for_settlement().await;
        assert_eq!(a.calls.lock().unwrap().len(), 2);
        assert_eq!(b.calls.lock().unwrap().len(), 2);
        assert!(!storage.force_usage_marked(None));
        assert!(storage.inner.aggregate_flights.lock().unwrap().is_empty());

        let requests = storage
            .collect_usage_requests(&FetchUsageReportsOptions::default(), &CancellationToken::new())
            .await
            .unwrap();
        let original_key = aggregate_usage_key(&requests);
        let mut reversed = requests.clone();
        reversed.reverse();
        assert_eq!(original_key, aggregate_usage_key(&reversed));
        reversed[0].base_url = Some(" https://different.example/// ".into());
        assert_ne!(original_key, aggregate_usage_key(&reversed));
        let same_url = build_usage_request(
            "test",
            transient_usage_credential(UsageCredentialType::ApiKey, "wire".into()),
            None,
            Some("https://same.example".into()),
        );
        let mut normalized_url = same_url.clone();
        normalized_url.base_url = Some(" \u{feff}https://same.example/// ".into());
        assert_eq!(aggregate_usage_key(&[same_url]), aggregate_usage_key(&[normalized_url]));

        // An older owner cannot consume a newly marked epoch's force request.
        let epoch_storage = new_storage(options(), &[("epoch", vec![AuthCredential::api_key("epoch-key")])]);
        let hook = Arc::new(CaptureUsage::default());
        let gate = Arc::new(Semaphore::new(0));
        hook.gates.lock().unwrap().push_back(gate.clone());
        epoch_storage.register_usage_provider("epoch", hook.clone()).unwrap();
        epoch_storage.invalidate_usage_cache(None).unwrap();
        let owner = epoch_storage.clone();
        let old = tokio::spawn(async move {
            owner.fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new()).await
        });
        started(&hook.started, 1).await;
        epoch_storage.invalidate_usage_cache(None).unwrap();
        gate.add_permits(1);
        assert_eq!(old.await.unwrap().unwrap().unwrap().len(), 1);
        epoch_storage.wait_for_settlement().await;
        assert!(epoch_storage.force_usage_marked(None));
        assert_eq!(
            epoch_storage
                .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        epoch_storage.wait_for_settlement().await;
        assert_eq!(hook.calls.lock().unwrap().len(), 2);
        assert!(!epoch_storage.force_usage_marked(None));

        epoch_storage.register_usage_provider("unused", Arc::new(CaptureUsage::default())).unwrap();
        epoch_storage.invalidate_usage_cache(Some("unused")).unwrap();
        epoch_storage.invalidate_usage_cache(None).unwrap();
        epoch_storage
            .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
            .await
            .unwrap();
        assert!(epoch_storage.force_usage_marked(Some("unused")), "a provider with no requests retains its marker");
        assert!(!epoch_storage.force_usage_marked(None));

        // A null account has the short backoff; when it becomes observable the
        // next aggregate includes it without freezing the previous subset.
        let clock = Arc::new(AtomicU64::new(NOW as u64));
        let now = clock.clone();
        let decorrelated = new_storage(
            AuthStorageOptions { clock: Arc::new(move || now.load(Ordering::Acquire) as f64), ..options() },
            &[("decorrelated", vec![AuthCredential::api_key("good"), AuthCredential::api_key("later")])],
        );
        let hook = Arc::new(CaptureUsage::default());
        hook.responses.lock().unwrap().insert("later".into(), None);
        decorrelated.register_usage_provider("decorrelated", hook.clone()).unwrap();
        assert_eq!(
            decorrelated
                .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap()
                .len(),
            1
        );
        decorrelated.wait_for_settlement().await;
        hook.responses.lock().unwrap().remove("later");
        clock.fetch_add(11_000, Ordering::AcqRel);
        assert_eq!(
            decorrelated
                .fetch_usage_reports(None, &AuthRequestContext::default(), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap()
                .len(),
            2
        );
        decorrelated.wait_for_settlement().await;
        assert_eq!(hook.calls.lock().unwrap().len(), 3, "successful sibling stays per-request cached");
    }
}
