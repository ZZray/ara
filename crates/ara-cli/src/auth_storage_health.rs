//! Managed-pool reserve health from fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/auth-storage.ts:4067–4215 (origin:2894–2905).
//!
//! This child of AuthStorage reuses its usage, block and sticky owners. Health
//! reads never select a credential or advance the round-robin cursor. Usage
//! refresh/cache/reconciliation retain their existing independent lifecycle.
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelUsageHealthState {
    Healthy,
    Unknown,
    Reserve,
    Depleted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageAccountHealth {
    pub credential_id: i64,
    pub credential_type: CredentialKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
    pub state: ModelUsageHealthState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<f64>,
}

impl ModelUsageAccountHealth {
    fn new(row: &StoredAuthCredential, state: ModelUsageHealthState) -> Self {
        Self {
            credential_id: row.id,
            credential_type: CredentialKind::of(&row.credential),
            selected: None,
            state,
            remaining_fraction: None,
            resets_at: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageHealth {
    pub state: ModelUsageHealthState,
    pub accounts: Vec<ModelUsageAccountHealth>,
}

impl ModelUsageHealth {
    fn unmanaged() -> Self {
        Self { state: ModelUsageHealthState::Unknown, accounts: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsageHealthOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    #[serde(default)]
    pub reserve_fraction: f64,
}

impl AuthStorage {
    /// Inspect the currently managed OAuth/login-key pool without selecting an
    /// account. Static overrides and ordinary stored keys have unknown health.
    pub async fn get_model_usage_health(
        &self,
        provider: &str,
        options: &ModelUsageHealthOptions,
        cancel: &CancellationToken,
    ) -> Result<ModelUsageHealth, AuthStorageError> {
        check_cancel(cancel)?;
        let Some(strategy) = self.strategy(provider) else { return Ok(ModelUsageHealth::unmanaged()) };
        let (pool, selected_id) = self.store_operation(|store, state| {
            // Native origin precedence: runtime/config -> OAuth -> login key.
            // Every lower origin bypasses a managed pool, so no secret resolver
            // or environment/fallback access is needed to classify that tail.
            if state.runtime_keys.contains_key(provider) || state.config_keys.contains_key(provider) {
                return Ok((Vec::new(), None));
            }
            let rows = Self::load_provider(store, state, provider)?;
            let kind = if rows.iter().any(|row| matches!(row.credential, AuthCredential::OAuth { .. })) {
                CredentialKind::OAuth
            } else if rows.iter().any(|row| {
                matches!(&row.credential, AuthCredential::ApiKey { source, .. }
                    if source.as_deref() == Some("login"))
            }) {
                CredentialKind::ApiKey
            } else {
                return Ok((Vec::new(), None));
            };
            let selected_id = state
                .assignments
                .read_session_credential(store, provider, options.session_id.as_deref(), &rows)
                .filter(|selected| selected.kind == kind)
                .and_then(|selected| rows.get(selected.index))
                .map(|row| row.id);
            let pool = rows
                .into_iter()
                .filter(|row| match &row.credential {
                    AuthCredential::OAuth { .. } => kind == CredentialKind::OAuth,
                    AuthCredential::ApiKey { source, .. } => {
                        kind == CredentialKind::ApiKey && source.as_deref() == Some("login")
                    }
                })
                .collect::<Vec<_>>();
            Ok((pool, selected_id))
        })?;
        if pool.is_empty() {
            return Ok(ModelUsageHealth::unmanaged());
        }

        let ranking_context = CredentialRankingContext { model_id: options.model_id.clone() };
        let requirement = policy::resolve_codex_plan_requirement(provider, options.model_id.as_deref());
        let block_scope = strategy.block_scope(Some(&ranking_context));
        let scopes = policy::credential_block_scopes_for_request(
            provider,
            Some(strategy.as_ref()),
            &ranking_context,
            block_scope.as_deref(),
        );
        let reserve = if options.reserve_fraction.is_finite() { options.reserve_fraction.clamp(0.0, 1.0) } else { 0.0 };
        let now = self.now();
        let request_context = AuthRequestContext {
            model_id: options.model_id.clone(),
            base_url: options.base_url.clone(),
            ..Default::default()
        };
        let results = join_all(pool.iter().map(|row| {
            let strategy = strategy.as_ref();
            let scopes = &scopes;
            let request_context = &request_context;
            let ranking_context = &ranking_context;
            async move {
                let kind = CredentialKind::of(&row.credential);
                let mut blocked = self.blocked(provider, row.id, kind, scopes).await?;
                if blocked.is_some() && provider != "openai-codex" {
                    let mut account = ModelUsageAccountHealth::new(row, ModelUsageHealthState::Depleted);
                    account.resets_at = blocked;
                    return Ok((account, None));
                }
                let report = match self.usage_report(provider, row, request_context, false, cancel).await {
                    Ok(report) => report,
                    Err(error) if cancel.is_cancelled() => return Err(error),
                    Err(_) => None,
                };
                let eligibility = policy::codex_plan_eligibility(report.as_ref(), requirement);
                // Codex usage may heal an old block through the existing usage
                // owner. Re-read all request scopes after that owner settles.
                if provider == "openai-codex" {
                    blocked = self.blocked(provider, row.id, kind, scopes).await?;
                }
                let account = if let Some(until) = blocked {
                    let mut account = ModelUsageAccountHealth::new(row, ModelUsageHealthState::Depleted);
                    account.resets_at = Some(until);
                    account
                } else {
                    reserve_account_health(row, report.as_ref(), strategy, ranking_context, now, reserve)
                };
                Ok((account, eligibility))
            }
        }))
        .await;
        let mut accounts = Vec::with_capacity(results.len());
        for result in results {
            let (mut account, eligibility) = result?;
            if requirement != PlanRequirement::None && eligibility == Some(false) {
                continue;
            }
            if selected_id == Some(account.credential_id) {
                account.selected = Some(true);
            }
            accounts.push(account);
        }
        let state = [ModelUsageHealthState::Healthy, ModelUsageHealthState::Unknown, ModelUsageHealthState::Reserve]
            .into_iter()
            .find(|state| accounts.iter().any(|account| account.state == *state))
            .unwrap_or(ModelUsageHealthState::Depleted);
        Ok(ModelUsageHealth { state, accounts })
    }

    /// Release only this session's sticky entry. The next normal resolver call
    /// ranks again; release never blocks, disables or resets provider cursors.
    pub fn release_session_credential_for_reselection(
        &self,
        provider: &str,
        session_id: &str,
    ) -> Result<bool, AuthStorageError> {
        self.store_operation(|store, state| {
            let rows = Self::load_provider(store, state, provider)?;
            if state.assignments.read_session_credential(store, provider, Some(session_id), &rows).is_none() {
                return Ok(false);
            }
            state.assignments.clear_session_credential(store, provider, Some(session_id));
            Ok(true)
        })
    }
}

fn reserve_account_health(
    row: &StoredAuthCredential,
    report: Option<&UsageReport>,
    strategy: &dyn CredentialRankingStrategy,
    context: &CredentialRankingContext,
    now: f64,
    reserve: f64,
) -> ModelUsageAccountHealth {
    let mut account = ModelUsageAccountHealth::new(row, ModelUsageHealthState::Unknown);
    let Some(report) = report else { return account };
    // The trait's default reserve hook delegates to its hard-block scoper.
    let current = strategy
        .scope_limits_for_reserve(report, Some(context))
        .into_iter()
        .filter(|limit| {
            limit
                .window
                .as_ref()
                .and_then(|window| window.resets_at)
                .is_none_or(|reset| reset > now || report.fetched_at >= reset)
        })
        .collect::<Vec<_>>();
    if current.is_empty() {
        return account;
    }
    let exhausted = current.iter().copied().filter(|limit| policy::is_usage_limit_exhausted(limit)).collect::<Vec<_>>();
    if !exhausted.is_empty() {
        account.state = ModelUsageHealthState::Depleted;
        // Reserve preflight advertises the EARLIEST future exhausted window.
        // The hard-block MAX helper intentionally has a different contract.
        account.resets_at = exhausted
            .iter()
            .filter_map(|limit| limit.window.as_ref().and_then(|window| window.resets_at).filter(|reset| *reset > now))
            .reduce(f64::min);
        return account;
    }
    let Some(used) = current
        .iter()
        .filter_map(|limit| policy::resolve_used_fraction(limit))
        .reduce(|left, right| if left.is_nan() || right.is_nan() { f64::NAN } else { left.max(right) })
    else {
        return account;
    };
    // Math.max propagates NaN, unlike Rust f64::max. Do not turn an unknown
    // numeric sample into a measured zero or a reserve/depletion assertion.
    let remaining = if used.is_nan() { f64::NAN } else { (1.0 - used).max(0.0) };
    account.state = if remaining <= reserve { ModelUsageHealthState::Reserve } else { ModelUsageHealthState::Healthy };
    account.remaining_fraction = Some(remaining);
    account
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_storage_policy::{UsageAmount, UsageLimit, UsageScope, UsageStatus, UsageUnit, UsageWindow};
    use tokio::sync::Semaphore;

    const NOW: f64 = 1_800_000_000_000.0;

    struct NativeScopeStrategy {
        reserve_tier: bool,
    }
    impl CredentialRankingStrategy for NativeScopeStrategy {
        fn find_window_limits<'a>(
            &self,
            report: &'a UsageReport,
            context: Option<&CredentialRankingContext>,
        ) -> policy::UsageWindowLimits<'a> {
            let limits = self.scope_limits(report, context);
            policy::UsageWindowLimits { primary: limits.first().copied(), secondary: limits.get(1).copied() }
        }
        fn scope_limits<'a>(
            &self,
            report: &'a UsageReport,
            context: Option<&CredentialRankingContext>,
        ) -> Vec<&'a UsageLimit> {
            report
                .limits
                .iter()
                .filter(|limit| {
                    limit
                        .scope
                        .model_id
                        .as_ref()
                        .is_none_or(|model| context.and_then(|context| context.model_id.as_ref()) == Some(model))
                        && (limit.scope.tier.is_none() || policy::is_usage_limit_exhausted(limit))
                })
                .collect()
        }
        fn scope_limits_for_reserve<'a>(
            &self,
            report: &'a UsageReport,
            context: Option<&CredentialRankingContext>,
        ) -> Vec<&'a UsageLimit> {
            if !self.reserve_tier {
                return self.scope_limits(report, context);
            }
            report
                .limits
                .iter()
                .filter(|limit| match limit.scope.tier.as_deref() {
                    None => limit
                        .scope
                        .model_id
                        .as_ref()
                        .is_none_or(|model| context.and_then(|context| context.model_id.as_ref()) == Some(model)),
                    Some("fable") => context
                        .and_then(|context| context.model_id.as_deref())
                        .is_some_and(|model| model.starts_with("claude-fable")),
                    _ => false,
                })
                .collect()
        }
        fn block_scopes(&self, context: Option<&CredentialRankingContext>) -> Vec<String> {
            vec![
                format!("model:{}", context.and_then(|context| context.model_id.as_deref()).unwrap_or("model")),
                "shared".into(),
            ]
        }
        fn window_defaults(&self) -> policy::WindowDefaults {
            policy::WindowDefaults { primary_ms: 60_000.0, secondary_ms: 60_000.0 }
        }
    }

    struct HealthUsage {
        reports: Mutex<BTreeMap<String, Option<UsageReport>>>,
        calls: Mutex<Vec<(String, Option<String>)>>,
        gate: Option<Arc<Semaphore>>,
        started: Semaphore,
    }
    #[async_trait]
    impl UsageProvider for HealthUsage {
        async fn fetch_usage(
            &self,
            request: UsageRequest,
            cancel: &CancellationToken,
        ) -> Result<Option<UsageReport>, UsageFetchError> {
            let identity = request.credential.account_id.or(request.credential.api_key).unwrap_or_default();
            self.calls.lock().unwrap().push((identity.clone(), request.base_url));
            self.started.add_permits(1);
            if let Some(gate) = &self.gate {
                tokio::select! {
                    _ = cancel.cancelled() => return Err(UsageFetchError::Cancelled),
                    permit = gate.acquire() => permit.unwrap().forget(),
                }
            }
            Ok(self.reports.lock().unwrap().get(&identity).cloned().flatten())
        }
    }

    fn oauth(account: &str) -> AuthCredential {
        AuthCredential::oauth(
            serde_json::json!({
                "access":format!("access-{account}"), "refresh":format!("refresh-{account}"),
                "expires":NOW + 7_200_000.0, "accountId":account,
            })
            .as_object()
            .unwrap()
            .clone(),
        )
    }
    fn key(value: &str, login: bool) -> AuthCredential {
        AuthCredential::ApiKey { key: value.into(), source: login.then(|| "login".into()) }
    }
    fn limit(id: &str, used: f64, reset: Option<f64>, model: Option<&str>) -> UsageLimit {
        UsageLimit {
            id: id.into(),
            label: id.into(),
            scope: UsageScope {
                provider: "anthropic".into(),
                model_id: model.map(str::to_owned),
                ..Default::default()
            },
            window: Some(UsageWindow {
                id: id.into(),
                label: id.into(),
                resets_at: reset,
                duration_ms: Some(60_000.0),
                ..Default::default()
            }),
            amount: UsageAmount { used_fraction: Some(used), unit: UsageUnit::Percent, ..Default::default() },
            status: Some(if used >= 1.0 { UsageStatus::Exhausted } else { UsageStatus::Ok }),
            ..Default::default()
        }
    }
    fn report(provider: &str, identity: &str, limits: Vec<UsageLimit>) -> UsageReport {
        UsageReport {
            provider: provider.into(),
            fetched_at: NOW,
            limits,
            metadata: Some(
                serde_json::json!({"accountId":identity,"allowed":true,"limitReached":false})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
            ..Default::default()
        }
    }
    fn setup(
        provider: &str,
        credentials: &[AuthCredential],
        reports: BTreeMap<String, Option<UsageReport>>,
        strategy: bool,
        gate: Option<Arc<Semaphore>>,
    ) -> (AuthStorage, Arc<HealthUsage>) {
        let store = Arc::new(Mutex::new(
            SqliteCredentialStore::from_connection(
                rusqlite::Connection::open_in_memory().unwrap(),
                Duration::from_millis(20),
            )
            .unwrap(),
        ));
        store.lock().unwrap().replace_auth_credentials_for_provider(provider, credentials).unwrap();
        let storage =
            AuthStorage::new(store, None, AuthStorageOptions { clock: Arc::new(|| NOW), ..Default::default() })
                .unwrap();
        let usage = Arc::new(HealthUsage {
            reports: Mutex::new(reports),
            calls: Mutex::new(Vec::new()),
            gate,
            started: Semaphore::new(0),
        });
        storage.register_usage_provider(provider, usage.clone()).unwrap();
        if strategy {
            storage.register_ranking_strategy(provider, Arc::new(NativeScopeStrategy { reserve_tier: true })).unwrap();
        }
        (storage, usage)
    }
    fn options(model: &str) -> ModelUsageHealthOptions {
        ModelUsageHealthOptions { model_id: Some(model.into()), reserve_fraction: 0.1, ..Default::default() }
    }
    fn sticky(storage: &AuthStorage, provider: &str, session: &str, index: Option<usize>) -> Option<i64> {
        storage
            .store_operation(|store, state| {
                let rows = AuthStorage::load_provider(store, state, provider)?;
                if let Some(index) = index {
                    state.assignments.record_session_credential(
                        store,
                        provider,
                        Some(session),
                        CredentialKind::of(&rows[index].credential),
                        index,
                        &rows,
                        None,
                        NOW,
                    );
                }
                Ok(state
                    .assignments
                    .read_session_credential(store, provider, Some(session), &rows)
                    .and_then(|selected| rows.get(selected.index))
                    .map(|row| row.id))
            })
            .unwrap()
    }

    /// Native model-usage-health inputs: pool precedence, independent windows,
    /// model scoping, mapped tier reserve hook, and reserve numeric boundaries.
    #[tokio::test]
    async fn native_reserve_pool_window_and_scope_family() {
        use ModelUsageHealthState::{Depleted, Healthy, Reserve, Unknown};
        let cancel = CancellationToken::new();
        let samples = [(Healthy, Some(0.2)), (Unknown, None), (Reserve, Some(0.95)), (Depleted, Some(1.0))];
        for (left_state, left) in samples {
            for (right_state, right) in samples {
                let reports = [("account-1", left), ("account-2", right)]
                    .into_iter()
                    .map(|(id, used)| {
                        (
                            id.to_owned(),
                            used.map(|used| {
                                report("anthropic", id, vec![limit("short", used, Some(NOW + 60_000.0), None)])
                            }),
                        )
                    })
                    .collect();
                let (storage, usage) =
                    setup("anthropic", &[oauth("account-1"), oauth("account-2")], reports, true, None);
                let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
                let expected = [Healthy, Unknown, Reserve]
                    .into_iter()
                    .find(|state| [left_state, right_state].contains(state))
                    .unwrap_or(Depleted);
                assert_eq!(health.state, expected);
                assert_eq!(
                    health.accounts.iter().map(|account| account.state).collect::<Vec<_>>(),
                    [left_state, right_state]
                );
                assert_eq!(usage.calls.lock().unwrap().len(), 2);
            }
        }

        let window_cases = [
            (
                NOW - 120_000.0,
                vec![
                    limit("5-hour", 1.0, Some(NOW - 60_000.0), None),
                    limit("7-day", 0.95, Some(NOW + 60_000.0), None),
                ],
                Reserve,
                None,
            ),
            (NOW - 120_000.0, vec![limit("short", 0.95, Some(NOW - 60_000.0), None)], Unknown, None),
            (NOW, vec![limit("short", 1.0, Some(NOW), None)], Depleted, None),
            (
                NOW,
                vec![limit("short", 1.0, Some(NOW + 60_000.0), None), limit("long", 1.0, Some(NOW + 120_000.0), None)],
                Depleted,
                Some(NOW + 60_000.0),
            ),
            (NOW, vec![limit("short", 0.95, None, None), limit("long", 0.2, None, None)], Reserve, None),
            (NOW, Vec::new(), Unknown, None),
        ];
        for (fetched_at, limits, expected, reset) in window_cases {
            let mut value = report("anthropic", "account-1", limits);
            value.fetched_at = fetched_at;
            let (storage, _) = setup(
                "anthropic",
                &[oauth("account-1")],
                BTreeMap::from([("account-1".into(), Some(value))]),
                true,
                None,
            );
            let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
            assert_eq!(health.state, expected);
            assert_eq!(health.accounts[0].resets_at, reset);
        }
        for (model, tier_used, expected) in [
            ("claude-fable-5", 0.96, Reserve),
            ("claude-fable-5", 0.85, Healthy),
            ("claude-opus-4-8", 0.96, Healthy),
            ("claude-fable-5", 1.0, Depleted),
        ] {
            let mut tier = limit("anthropic:7d:fable", tier_used, Some(NOW + 60_000.0), None);
            tier.scope.tier = Some("fable".into());
            let value = report(
                "anthropic",
                "account-1",
                vec![
                    limit("anthropic:5h", 0.1, Some(NOW + 60_000.0), None),
                    limit("anthropic:7d", 0.72, Some(NOW + 60_000.0), None),
                    tier,
                ],
            );
            let (storage, _) = setup(
                "anthropic",
                &[oauth("account-1")],
                BTreeMap::from([("account-1".into(), Some(value))]),
                true,
                None,
            );
            assert_eq!(
                storage.get_model_usage_health("anthropic", &options(model), &cancel).await.unwrap().state,
                expected
            );
        }
        let (storage, _) = setup(
            "anthropic",
            &[oauth("account-1")],
            BTreeMap::from([(
                "account-1".into(),
                Some(report(
                    "anthropic",
                    "account-1",
                    vec![limit("claude", 1.0, None, Some("claude")), limit("haiku", 0.2, None, Some("haiku"))],
                )),
            )]),
            true,
            None,
        );
        let health = storage.get_model_usage_health("anthropic", &options("haiku"), &cancel).await.unwrap();
        assert_eq!(health.state, Healthy);
        assert_eq!(health.accounts[0].remaining_fraction, Some(0.8));
        assert_eq!(
            serde_json::to_value(&health.accounts[0]).unwrap(),
            serde_json::json!({
                "credentialId":1,"credentialType":"oauth","state":"healthy","remainingFraction":0.8
            })
        );
        for (reserve, expected) in
            [(f64::NAN, Healthy), (f64::INFINITY, Healthy), (-1.0, Healthy), (2.0, Reserve), (0.8, Reserve)]
        {
            let mut query = options("haiku");
            query.reserve_fraction = reserve;
            assert_eq!(storage.get_model_usage_health("anthropic", &query, &cancel).await.unwrap().state, expected);
        }
        assert_eq!(
            storage.get_model_usage_health("anthropic", &options("unmapped"), &cancel).await.unwrap().state,
            Unknown
        );
        for (amount, expected, remaining) in [
            (serde_json::json!({"used":95,"limit":100,"unit":"percent"}), Reserve, Some(0.05)),
            (serde_json::json!({"used":95,"unit":"percent"}), Reserve, Some(0.05)),
            (serde_json::json!({"remainingFraction":0.05,"unit":"percent"}), Reserve, Some(0.05)),
            (serde_json::json!({"unit":"percent"}), Unknown, None),
            // Explicit status wins over numeric exhaustion in the native schema.
            (serde_json::json!({"usedFraction":1.25,"unit":"percent"}), Reserve, Some(0.0)),
        ] {
            let mut sample = limit("short", 0.0, None, None);
            sample.amount = serde_json::from_value(amount).unwrap();
            let (storage, _) = setup(
                "anthropic",
                &[oauth("account-1")],
                BTreeMap::from([("account-1".into(), Some(report("anthropic", "account-1", vec![sample])))]),
                true,
                None,
            );
            let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
            assert_eq!(health.state, expected);
            match (health.accounts[0].remaining_fraction, remaining) {
                (Some(actual), Some(expected)) => assert!((actual - expected).abs() < 1e-12),
                (None, None) => {}
                _ => panic!("native fraction presence changed"),
            }
        }
    }

    /// Native managed origins, full-array sticky indices, all persisted scopes,
    /// nonmutating round-robin inspection, Codex re-read/healing and plan gates.
    #[tokio::test]
    async fn native_reserve_origin_sticky_block_and_plan_family() {
        use ModelUsageHealthState::{Depleted, Healthy, Reserve, Unknown};
        let cancel = CancellationToken::new();
        for (credentials, strategy) in
            [(Vec::new(), true), (vec![key("static", false)], true), (vec![oauth("account-1")], false)]
        {
            let (storage, usage) = setup("anthropic", &credentials, BTreeMap::new(), strategy, None);
            let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
            assert_eq!(health, ModelUsageHealth::unmanaged());
            assert!(usage.calls.lock().unwrap().is_empty());
        }
        let (storage, usage) = setup("anthropic", &[key("!unresolved", true)], BTreeMap::new(), true, None);
        let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
        assert_eq!(health.state, Unknown);
        assert_eq!(health.accounts.len(), 1);
        assert!(usage.calls.lock().unwrap().is_empty());
        let (storage, usage) = setup(
            "anthropic",
            &[key("static", false), oauth("account-1"), key("login", true)],
            BTreeMap::from([(
                "account-1".into(),
                Some(report("anthropic", "account-1", vec![limit("short", 0.2, None, None)])),
            )]),
            true,
            None,
        );
        storage.set_runtime_api_key("anthropic", "override".into()).unwrap();
        assert_eq!(
            storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap().state,
            Unknown
        );
        storage.remove_runtime_api_key("anthropic").unwrap();
        storage.set_config_api_key("anthropic", "override".into()).unwrap();
        assert!(
            storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap().accounts.is_empty()
        );
        storage.remove_config_api_key("anthropic").unwrap();
        let selected = sticky(&storage, "anthropic", "session-1", Some(1)).unwrap();
        let other = sticky(&storage, "anthropic", "session-2", Some(1));
        let mut query = options("claude");
        query.session_id = Some("session-1".into());
        query.base_url = Some("https://fixture.example/usage".into());
        let health = storage.get_model_usage_health("anthropic", &query, &cancel).await.unwrap();
        assert_eq!(health.state, Healthy);
        assert_eq!(health.accounts.len(), 1);
        assert_eq!(health.accounts[0].credential_id, selected);
        assert_eq!(health.accounts[0].selected, Some(true));
        assert_eq!(sticky(&storage, "anthropic", "session-1", None), Some(selected));
        assert_eq!(usage.calls.lock().unwrap().as_slice(), [("account-1".into(), query.base_url.clone())]);
        assert!(storage.release_session_credential_for_reselection("anthropic", "session-1").unwrap());
        assert!(!storage.release_session_credential_for_reselection("anthropic", "session-1").unwrap());
        assert_eq!(sticky(&storage, "anthropic", "session-2", None), other);
        assert!(
            storage
                .inner
                .store
                .lock()
                .unwrap()
                .get_cache("session:sticky:anthropic:session-1", false)
                .unwrap()
                .is_none_or(|value| value.is_empty())
        );

        let (storage, usage) = setup(
            "anthropic",
            &[key("static", false), key("key-1", true), key("key-2", true)],
            BTreeMap::from([
                ("key-1".into(), Some(report("anthropic", "key-1", vec![limit("short", 0.95, None, None)]))),
                ("key-2".into(), Some(report("anthropic", "key-2", vec![limit("short", 0.2, None, None)]))),
            ]),
            true,
            None,
        );
        let selected = sticky(&storage, "anthropic", "session-1", Some(1)).unwrap();
        let mut query = options("claude");
        query.session_id = Some("session-1".into());
        let health = storage.get_model_usage_health("anthropic", &query, &cancel).await.unwrap();
        assert_eq!(health.state, Healthy);
        assert_eq!(health.accounts.len(), 2);
        assert_eq!(health.accounts[0].credential_id, selected);
        assert_eq!(health.accounts[0].state, Reserve);
        assert_eq!(health.accounts[0].selected, Some(true));
        assert!(storage.release_session_credential_for_reselection("anthropic", "session-1").unwrap());
        let context = AuthRequestContext {
            session_id: query.session_id.clone(),
            model_id: query.model_id.clone(),
            ..Default::default()
        };
        assert_eq!(storage.get_api_key("anthropic", &context, &cancel).await.unwrap().as_deref(), Some("key-2"));
        assert!(usage.calls.lock().unwrap().iter().all(|(identity, _)| identity != "static"));

        let (storage, _) = setup(
            "anthropic",
            &[key("key-1", true), key("key-2", true)],
            BTreeMap::from([
                ("key-1".into(), Some(report("anthropic", "key-1", vec![limit("short", 0.2, None, None)]))),
                ("key-2".into(), Some(report("anthropic", "key-2", vec![limit("short", 0.2, None, None)]))),
            ]),
            true,
            None,
        );
        storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
        assert_eq!(
            storage
                .get_api_key(
                    "anthropic",
                    &AuthRequestContext { model_id: Some("claude".into()), ..Default::default() },
                    &cancel
                )
                .await
                .unwrap()
                .as_deref(),
            Some("key-1")
        );

        let (storage, usage) = setup("anthropic", &[oauth("account-1")], BTreeMap::new(), true, None);
        let row = storage.provider_rows("anthropic").await.unwrap().remove(0);
        storage
            .upsert_credential_block(&StoredCredentialBlock {
                credential_id: row.id,
                provider_key: "anthropic:oauth".into(),
                block_scope: "shared".into(),
                blocked_until_ms: (NOW + 60_000.0) as i64,
                updated_at_ms: NOW as i64,
            })
            .unwrap();
        let health = storage.get_model_usage_health("anthropic", &options("claude"), &cancel).await.unwrap();
        assert_eq!(health.state, Depleted);
        assert_eq!(health.accounts[0].resets_at, Some(NOW + 60_000.0));
        assert!(usage.calls.lock().unwrap().is_empty());
        assert!(!storage.release_session_credential_for_reselection("anthropic", "missing").unwrap());
        sticky(&storage, "anthropic", "blocked-session", Some(0));
        assert!(storage.release_session_credential_for_reselection("anthropic", "blocked-session").unwrap());
        assert_eq!(storage.list_credential_blocks(&[row.id]).unwrap().len(), 1);

        for (plan, expected, count) in [(Some("free"), Depleted, 0), (Some("pro"), Healthy, 1), (None, Healthy, 1)] {
            let mut value = report(
                "openai-codex",
                "account-1",
                vec![limit("openai-codex:spark:primary", 0.2, Some(NOW + 60_000.0), None)],
            );
            if let Some(plan) = plan {
                value.metadata.as_mut().unwrap().insert("planType".into(), Value::String(plan.into()));
            }
            let (storage, _) = setup(
                "openai-codex",
                &[oauth("account-1")],
                BTreeMap::from([("account-1".into(), Some(value))]),
                false,
                None,
            );
            let health =
                storage.get_model_usage_health("openai-codex", &options("gpt-5.3-codex-spark"), &cancel).await.unwrap();
            assert_eq!(health.state, expected);
            assert_eq!(health.accounts.len(), count);
        }
        let value =
            report("openai-codex", "account-1", vec![limit("openai-codex:primary", 0.2, Some(NOW + 60_000.0), None)]);
        let (storage, usage) = setup(
            "openai-codex",
            &[oauth("account-1")],
            BTreeMap::from([("account-1".into(), Some(value))]),
            false,
            None,
        );
        let row = storage.provider_rows("openai-codex").await.unwrap().remove(0);
        storage
            .upsert_credential_block(&StoredCredentialBlock {
                credential_id: row.id,
                provider_key: "openai-codex:oauth".into(),
                block_scope: "chat".into(),
                blocked_until_ms: (NOW + 60_000.0) as i64,
                updated_at_ms: (NOW - USAGE_REPORT_TTL_MS as f64 - 1.0) as i64,
            })
            .unwrap();
        let health = storage.get_model_usage_health("openai-codex", &options("gpt-5.3-codex"), &cancel).await.unwrap();
        assert_eq!(health.state, Healthy);
        assert_eq!(usage.calls.lock().unwrap().len(), 1);
        assert!(storage.list_credential_blocks(&[row.id]).unwrap().is_empty());
    }

    /// Native caller cancellation detaches from a shared usage owner, whose
    /// successful settlement remains observable to the next health inspection.
    #[tokio::test]
    async fn native_reserve_cancellation_and_shared_settlement_family() {
        let gate = Arc::new(Semaphore::new(0));
        let (storage, usage) = setup(
            "anthropic",
            &[key("key-1", true)],
            BTreeMap::from([(
                "key-1".into(),
                Some(report("anthropic", "key-1", vec![limit("short", 0.2, None, None)])),
            )]),
            true,
            Some(gate.clone()),
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            storage.get_model_usage_health("anthropic", &options("claude"), &cancelled).await,
            Err(AuthStorageError::Cancelled)
        );
        assert!(usage.calls.lock().unwrap().is_empty());
        let cancel = CancellationToken::new();
        let task_storage = storage.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move {
            task_storage.get_model_usage_health("anthropic", &options("claude"), &task_cancel).await
        });
        tokio::time::timeout(Duration::from_secs(1), usage.started.acquire()).await.unwrap().unwrap().forget();
        cancel.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), task).await.unwrap().unwrap(),
            Err(AuthStorageError::Cancelled)
        );
        gate.add_permits(1);
        storage.wait_for_settlement().await;
        let health =
            storage.get_model_usage_health("anthropic", &options("claude"), &CancellationToken::new()).await.unwrap();
        assert_eq!(health.state, ModelUsageHealthState::Healthy);
        assert_eq!(usage.calls.lock().unwrap().len(), 1);
    }
}
