//! Host consumers of fixed OMP AgentSession saved-reset planning, consent,
//! blocked adoption and usage sweeps (agent-session.ts:9920-10216 at
//! 596f2da7101178214aa27a753529d15e6b7ad91d). Unknown consume effects retain
//! a durable account fence instead of the native thirty-minute replay policy.
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

use crate::auth_storage::{
    AuthSessionScope, AuthStorage, AuthStorageError, CredentialBaseUrlResolver, FetchUsageReportsOptions, ListResetCreditsOptions,
    RedeemResetCreditOptions, ResetCreditAccountStatus, ResetRedemption,
};
use crate::auth_storage_policy::UsageReport;
use crate::auth_storage_state::OAuthAccountSummary;
use crate::codex_auto_reset::{
    ATTEMPT_COOLDOWN_MS, CodexAutoRedeemMode, CodexResetAction, CodexResetPlanInput, CodexResetSettings,
    CodexResetTrigger, REDEEM_RETRY_DEFER_MS, SWEEP_MIN_INTERVAL_MS, is_terminal_redeem_outcome,
    overlay_live_reset_credits, plan_codex_resets_with_permits, should_evaluate_codex_auto_redeem, should_prompt_codex_auto_redeem,
};
use crate::codex_reset_receipts::{ResetOperationReceipt, ResetReceiptState, ResetReceiptStore};
use crate::model_route::{
    AuthResolveError, AuthRetryAction, CredentialIdentity, QuotaResetReplay, RequestAuthFailure,
    RequestAuthFailureKind, RequestAuthLease, RequestAuthResolver,
};
use ara_ai::Model;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock, atomic::{AtomicBool, Ordering}};
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;

fn now_ms() -> f64 {
    chrono::Utc::now().timestamp_millis() as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexResetConsent {
    Yes,
    No,
}

/// An actual Host selection is required before an unset setting can persist.
/// Reference print/REPL/RPC Hosts have no native selection UI and return None.
#[async_trait]
pub trait CodexResetHost: Send + Sync {
    fn settings(&self) -> Result<CodexResetSettings, String>;
    fn persist_mode(&self, mode: CodexAutoRedeemMode) -> Result<(), String>;
    fn has_ui(&self) -> bool {
        false
    }
    async fn select(&self, _actions: &[CodexResetAction]) -> Result<Option<CodexResetConsent>, String> {
        Ok(None)
    }
    fn notice(&self, level: &str, message: &str);
    fn epoch(&self) -> Option<u64> {
        None
    }
    fn is_current(&self, _context: &CodexResetContext) -> bool {
        true
    }
}

#[derive(Clone)]
pub struct CodexResetContext {
    pub provider: String,
    pub model_id: String,
    pub usage: FetchUsageReportsOptions,
    pub host_epoch: Option<u64>,
}

impl CodexResetContext {
    pub fn with_session_scope(mut self, scope: Arc<AuthSessionScope>) -> Self {
        self.usage.context = scope.context_for(&self.provider, &self.usage.context);
        self
    }
    pub fn for_model(model: &Model, session_id: Option<String>, resolver: Option<CredentialBaseUrlResolver>) -> Self {
        Self {
            provider: model.provider.clone(),
            model_id: model.id.clone(),
            host_epoch: None,
            usage: FetchUsageReportsOptions {
                context: crate::auth_storage::AuthRequestContext {
                    session_id,
                    model_id: Some(model.id.clone()),
                    base_url: Some(model.base_url.clone()),
                    ..Default::default()
                },
                base_url_resolver: resolver,
                ..Default::default()
            },
        }
    }
}

#[derive(Clone, Debug, Default)]
struct SharedResetPass {
    receipts: Vec<ResetOperationReceipt>,
    pool_limited: bool,
}
type PassResult = SharedResetPass;
type PassReceiver = watch::Receiver<Option<PassResult>>;

#[derive(Default)]
struct Coordination {
    attempted_keys: HashSet<String>,
    deferred_until_by_key: HashMap<String, f64>,
    last_attempt_at_by_account: HashMap<String, f64>,
    in_flight_by_account: HashMap<String, PassReceiver>,
    sweeps_in_flight: HashSet<String>,
    last_sweep_at: HashMap<String, f64>,
    sweep_settlement: HashMap<String, PassReceiver>,
    notified_keys: HashSet<String>,
    refreshes: HashMap<String, (f64, u64, bool)>,
    jobs: usize,
    closed: bool,
}

#[derive(Default)]
pub struct CodexResetCoordinator {
    state: Mutex<Coordination>,
    settled: Notify,
}

impl CodexResetCoordinator {
    pub fn process_shared() -> Arc<Self> {
        static OWNER: OnceLock<Arc<CodexResetCoordinator>> = OnceLock::new();
        OWNER.get_or_init(|| Arc::new(Self::default())).clone()
    }

    pub async fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.wait_for_settlement().await;
    }

    pub async fn wait_for_settlement(&self) {
        loop {
            let notified = self.settled.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.state.lock().unwrap().jobs == 0 {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait_for_sweep(&self) {
        let receivers: Vec<_> = self.state.lock().unwrap().sweep_settlement.values().cloned().collect();
        for receiver in receivers {
            let _ = await_pass(receiver, &CancellationToken::new()).await;
        }
    }
}

struct JobGuard {
    coordinator: Arc<CodexResetCoordinator>,
    account: Option<String>,
    sweep: Option<String>,
}
impl Drop for JobGuard {
    fn drop(&mut self) {
        let mut state = self.coordinator.state.lock().unwrap();
        if let Some(key) = &self.account {
            state.in_flight_by_account.remove(key);
        }
        if let Some(scope) = &self.sweep {
            state.sweeps_in_flight.remove(scope);
        }
        state.jobs -= 1;
        drop(state);
        self.coordinator.settled.notify_waiters();
    }
}

async fn await_pass(mut receiver: PassReceiver, cancel: &CancellationToken) -> PassResult {
    loop {
        if let Some(result) = receiver.borrow().clone() {
            return result;
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return PassResult::default(),
            changed = receiver.changed() => if changed.is_err() { return PassResult::default() },
        }
    }
}

pub struct CodexResetController {
    storage: Arc<AuthStorage>,
    receipts: Arc<dyn ResetReceiptStore>,
    host: Arc<dyn CodexResetHost>,
    coordinator: Arc<CodexResetCoordinator>,
    notice_sink: RwLock<Option<CodexResetNoticeSink>>,
}

pub type CodexResetNoticeSink = Arc<dyn Fn(&str, &str) + Send + Sync>;

impl CodexResetController {
    pub fn new(
        storage: Arc<AuthStorage>,
        receipts: Arc<dyn ResetReceiptStore>,
        host: Arc<dyn CodexResetHost>,
        coordinator: Arc<CodexResetCoordinator>,
    ) -> Arc<Self> {
        Arc::new(Self { storage, receipts, host, coordinator, notice_sink: RwLock::new(None) })
    }

    pub fn set_notice_sink(&self, sink: CodexResetNoticeSink) {
        *self.notice_sink.write().unwrap() = Some(sink);
    }

    fn notice(&self, level: &str, message: &str) {
        let sink = self.notice_sink.read().unwrap().clone();
        if let Some(sink) = sink {
            sink(level, message);
        } else {
            self.host.notice(level, message);
        }
    }

    pub fn coordinator(&self) -> &Arc<CodexResetCoordinator> {
        &self.coordinator
    }

    fn reset_scope(&self, context: &CodexResetContext) -> Result<String, AuthStorageError> {
        let base = context.usage.base_url_resolver.as_ref().and_then(|resolve| resolve("openai-codex"));
        self.storage.reset_operation_scope("openai-codex", base.as_deref())
    }

    fn freeze_context(&self, mut context: CodexResetContext) -> Result<(CodexResetContext, String), AuthStorageError> {
        let original = context.usage.base_url_resolver.clone();
        let codex_base = original.as_ref().and_then(|resolve| resolve("openai-codex"));
        let scope = self.storage.reset_operation_scope("openai-codex", codex_base.as_deref())?;
        context.usage.base_url_resolver = Some(Arc::new(move |provider| {
            if provider == "openai-codex" {
                codex_base.clone()
            } else {
                original.as_ref().and_then(|resolve| resolve(provider))
            }
        }));
        Ok((context, scope))
    }

    fn refresh_key(&self, context: &CodexResetContext) -> Result<String, AuthStorageError> {
        Ok(format!(
            "{}:{}\0{}\0{}\0{:?}",
            self.reset_scope(context)?,
            context.usage.context.session_id.as_deref().unwrap_or(""),
            context.provider,
            context.model_id,
            context.host_epoch
        ))
    }

    pub async fn close(&self) {
        self.coordinator.close().await;
        self.storage.wait_for_settlement().await;
    }

    pub fn decorate(
        self: &Arc<Self>,
        inner: Arc<dyn RequestAuthResolver>,
        session_id: Option<String>,
        base_url_resolver: Option<CredentialBaseUrlResolver>,
    ) -> Arc<dyn RequestAuthResolver> {
        Arc::new(ResetRequestAuth {
            inner,
            controller: self.clone(),
            session_id,
            base_url_resolver,
            host_epoch: self.host.epoch(),
            session_scope: None,
        })
    }

    pub fn decorate_scoped(
        self: &Arc<Self>, inner: Arc<dyn RequestAuthResolver>, session_id: Option<String>,
        base_url_resolver: Option<CredentialBaseUrlResolver>, scope: Arc<AuthSessionScope>,
    ) -> Arc<dyn RequestAuthResolver> {
        Arc::new(ResetRequestAuth { inner, controller: self.clone(), session_id, base_url_resolver,
            host_epoch: self.host.epoch(), session_scope: Some(scope) })
    }

    fn receipt_usable(&self, context: &CodexResetContext, receipt: &ResetOperationReceipt) -> bool {
        if !context.usage.context.session_scope.as_ref()
            .is_some_and(|scope| scope.oauth_account_pools.contains_key("openai-codex")) { return true }
        self.storage.list_oauth_accounts_with_context("openai-codex", &context.usage.context).ok()
            .is_some_and(|rows| rows.iter().any(|row| row.credential_id == receipt.credential_id
                && receipt.account_id.as_ref().is_none_or(|id| row.account_id.as_ref() == Some(id))
                && receipt.email.as_ref().is_none_or(|email| row.email.as_ref() == Some(email))))
    }

    fn pass_usable(&self, context: &CodexResetContext, pass: &SharedResetPass) -> bool {
        if pass.receipts.is_empty() { return !pass.pool_limited }
        pass.receipts.iter().any(|receipt| {
            receipt.is_confirmed_reset() && self.receipt_usable(context, receipt)
        })
    }

    pub async fn fetch_usage_reports(
        self: &Arc<Self>,
        context: &CodexResetContext,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        let (context, _) = self.freeze_context(context.clone())?;
        let reports = self.storage.fetch_usage_reports_with_options(&context.usage, cancel).await?;
        if let Some(reports) = &reports
            && self.host.is_current(&context)
        {
            self.coordinator.state.lock().unwrap().refreshes.entry(self.refresh_key(&context)?).or_default().0 =
                now_ms();
            self.schedule_sweep(context.clone(), reports.clone());
        }
        Ok(reports)
    }

    /// Called when a Host renders quota-capable state. The five-minute gate
    /// owns freshness, not a permanent Core timer; rendering never waits for IO.
    pub fn refresh_if_stale(self: &Arc<Self>, context: CodexResetContext) {
        let Ok((context, _)) = self.freeze_context(context) else { return };
        let Ok(key) = self.refresh_key(&context) else { return };
        let sequence = {
            let mut state = self.coordinator.state.lock().unwrap();
            if state.closed {
                return;
            }
            let fresh = state.refreshes.entry(key.clone()).or_default();
            if fresh.2 || now_ms() - fresh.0 < 300_000.0 {
                return;
            }
            fresh.1 = fresh.1.wrapping_add(1);
            fresh.2 = true;
            let sequence = fresh.1;
            state.jobs += 1;
            sequence
        };
        let controller = self.clone();
        let guard = JobGuard { coordinator: self.coordinator.clone(), account: None, sweep: None };
        tokio::spawn(async move {
            let _guard = guard;
            if controller.host.is_current(&context) {
                let _ = controller.fetch_usage_reports(&context, &CancellationToken::new()).await;
            }
            let mut state = controller.coordinator.state.lock().unwrap();
            if let Some(fresh) = state.refreshes.get_mut(&key)
                && fresh.1 == sequence
            {
                fresh.2 = false;
            }
        });
    }

    pub fn schedule_sweep(self: &Arc<Self>, context: CodexResetContext, reports: Vec<UsageReport>) {
        let Ok(settings) = self.host.settings() else { return };
        if !should_evaluate_codex_auto_redeem(settings.auto_redeem) || settings.salvage_horizon_hours <= 0.0 {
            return;
        }
        if !reports.iter().any(|report| {
            report.provider == "openai-codex"
                && report
                    .reset_credits
                    .as_ref()
                    .and_then(|credits| credits.credits.as_ref())
                    .is_some_and(|credits| !credits.is_empty())
        }) {
            return;
        }
        if !self.host.is_current(&context) {
            return;
        }
        let Ok((context, scope)) = self.freeze_context(context) else { return };
        let prefix = format!("{scope}:");
        let (tx, rx) = watch::channel(None);
        {
            let mut state = self.coordinator.state.lock().unwrap();
            let now = now_ms();
            if state.closed
                || state.sweeps_in_flight.contains(&scope)
                || state.in_flight_by_account.keys().any(|key| key.starts_with(&prefix))
                || state.last_sweep_at.get(&scope).is_some_and(|last| now - last < SWEEP_MIN_INTERVAL_MS)
            {
                return;
            }
            state.sweeps_in_flight.insert(scope.clone());
            state.last_sweep_at.insert(scope.clone(), now);
            state.sweep_settlement.insert(scope.clone(), rx);
            state.jobs += 1;
        }
        let controller = self.clone();
        let guard = JobGuard { coordinator: self.coordinator.clone(), account: None, sweep: Some(scope) };
        tokio::spawn(async move {
            let _guard = guard;
            let identity = controller
                .storage
                .get_oauth_account_identity("openai-codex", context.usage.context.session_id.as_deref())
                .ok()
                .flatten();
            let (actions, _) =
                controller.plan(&context, CodexResetTrigger::Sweep, Some(&reports), identity.as_ref(), None, settings);
            let result = if controller.confirm(&context, &actions, settings).await {
                controller.execute(&context, actions).await
            } else {
                PassResult::default()
            };
            // Settle the owner before publishing completion. A waiter may
            // immediately admit a distinct pass without an older guard removing it.
            drop(_guard);
            let _ = tx.send(Some(result));
        });
    }

    pub async fn blocked(
        self: &Arc<Self>,
        context: CodexResetContext,
        identity: OAuthAccountSummary,
        unblock_at_ms: Option<f64>,
        cancel: &CancellationToken,
    ) -> Vec<ResetOperationReceipt> {
        let Ok(settings) = self.host.settings() else { return Vec::new() };
        if cancel.is_cancelled() || !should_evaluate_codex_auto_redeem(settings.auto_redeem)
            || context.provider != "openai-codex" { return Vec::new() }
        let Ok((context, scope)) = self.freeze_context(context) else { return Vec::new() };
        let base = context.usage.base_url_resolver.as_ref().and_then(|resolve| resolve("openai-codex"));
        let Ok(key) = self.storage.reset_operation_account_key("openai-codex", base.as_deref(),
            identity.credential_id, identity.account_id.as_deref(), identity.email.as_deref()) else { return Vec::new() };
        loop {
            if cancel.is_cancelled() || !self.host.is_current(&context) { return Vec::new() }
            let (tx, rx) = watch::channel(None);
            let (existing, sweep) = {
                let mut state = self.coordinator.state.lock().unwrap();
                if state.closed || cancel.is_cancelled() { return Vec::new() }
                if let Some(existing) = state.in_flight_by_account.get(&key) {
                    (Some(existing.clone()), None)
                } else {
                    let sweep = state.sweeps_in_flight.contains(&scope)
                        .then(|| state.sweep_settlement.get(&scope).cloned()).flatten();
                    state.in_flight_by_account.insert(key.clone(), rx.clone());
                    state.jobs += 1;
                    (None, sweep)
                }
            };
            if let Some(existing) = existing {
                let result = await_pass(existing, cancel).await;
                if cancel.is_cancelled() || !self.host.is_current(&context) { return Vec::new() }
                if self.pass_usable(&context, &result) { return result.receipts }
                // The original owner removed its key before publishing this
                // result. Re-admit under our scope, never re-observe a completed watch.
                continue;
            }
            let controller = self.clone();
            let pass_context = context.clone();
            let pass_identity = identity.clone();
            let pass_scope = scope.clone();
            let guard = JobGuard { coordinator: self.coordinator.clone(), account: Some(key.clone()), sweep: None };
            tokio::spawn(async move {
                let _guard = guard;
                let adopted = if let Some(sweep) = sweep {
                    let result = await_pass(sweep, &CancellationToken::new()).await;
                    let receipts = result.receipts.into_iter().filter(|receipt| receipt.is_confirmed_reset()
                        && receipt.account_key.starts_with(&format!("{pass_scope}:"))).collect();
                    let result = SharedResetPass { receipts, pool_limited: result.pool_limited };
                    if !result.receipts.is_empty() && controller.pass_usable(&pass_context, &result) {
                        Some(result)
                    } else { None }
                } else { None };
                let result = if let Some(adopted) = adopted { adopted }
                else if controller.coordinator.state.lock().unwrap().closed
                    || !controller.host.is_current(&pass_context)
                    || !controller.host.settings().is_ok_and(|settings| should_evaluate_codex_auto_redeem(settings.auto_redeem)) {
                    PassResult::default()
                } else {
                    controller.blocked_pass(&pass_context, &pass_identity, unblock_at_ms, settings).await
                };
                drop(_guard);
                let _ = tx.send(Some(result));
            });
            return await_pass(rx, cancel).await.receipts;
        }
    }

    async fn blocked_pass(
        self: &Arc<Self>,
        context: &CodexResetContext,
        identity: &OAuthAccountSummary,
        unblock_at_ms: Option<f64>,
        settings: CodexResetSettings,
    ) -> PassResult {
        let token = CancellationToken::new();
        if self.storage.invalidate_usage_cache_and_notify(Some("openai-codex"), &token).await.is_err() {
            return PassResult::default();
        }
        let Ok(reports) = self.fetch_usage_reports(context, &token).await else { return PassResult::default() };
        let live_token = CancellationToken::new();
        let options = ListResetCreditsOptions {
            session_id: context.usage.context.session_id.clone(),
            base_url_resolver: context.usage.base_url_resolver.clone(),
            ..Default::default()
        };
        let live =
            tokio::time::timeout(Duration::from_secs(10), self.storage.list_reset_credits(&options, &live_token)).await;
        let effective = match live {
            Ok(Ok(statuses)) => overlay_live_reset_credits(reports.as_deref(), &statuses),
            _ => {
                live_token.cancel();
                reports
            }
        };
        let (actions, pool_limited) = self.plan(
            context,
            CodexResetTrigger::Blocked,
            effective.as_deref(),
            Some(identity),
            unblock_at_ms,
            settings,
        );
        let mut result = if self.confirm(context, &actions, settings).await {
            self.execute(context, actions).await
        } else { PassResult::default() };
        result.pool_limited |= pool_limited;
        result
    }

    fn plan(
        &self,
        context: &CodexResetContext,
        trigger: CodexResetTrigger,
        reports: Option<&[UsageReport]>,
        identity: Option<&OAuthAccountSummary>,
        unblock_at_ms: Option<f64>,
        settings: CodexResetSettings,
    ) -> (Vec<CodexResetAction>, bool) {
        let Ok(scope) = self.reset_scope(context) else { return (Vec::new(), false) };
        let prefix = format!("{scope}:");
        let state = self.coordinator.state.lock().unwrap();
        // The pure native planner sees only this authority's logical keys.
        // Shared process state and durable receipts retain the scope prefix.
        let attempted_keys =
            state.attempted_keys.iter().filter_map(|key| key.strip_prefix(&prefix).map(str::to_owned)).collect();
        let deferred_until_by_key = state
            .deferred_until_by_key
            .iter()
            .filter_map(|(key, at)| key.strip_prefix(&prefix).map(|key| (key.to_owned(), *at)))
            .collect();
        let last_attempt_at_by_account = state
            .last_attempt_at_by_account
            .iter()
            .filter_map(|(key, at)| key.strip_prefix(&prefix).map(|key| (key.to_owned(), *at)))
            .collect();
        drop(state);
        let permits = |target: &crate::auth_storage::ResetCreditTarget| {
            self.storage.permits_reset_target("openai-codex", target, &context.usage.context).unwrap_or(false)
        };
        let plan = plan_codex_resets_with_permits(&CodexResetPlanInput {
            now_ms: now_ms(),
            trigger,
            provider: &context.provider,
            model_id: &context.model_id,
            settings,
            identity,
            reports,
            attempted_keys: &attempted_keys,
            deferred_until_by_key: &deferred_until_by_key,
            last_attempt_at_by_account: &last_attempt_at_by_account,
            active_block_unblock_at_ms: unblock_at_ms,
        }, Some(&permits));
        let pool_limited = plan.skipped.iter().any(|skip| skip.reason == "outside-account-pool");
        let mut actions = plan.actions;
        for action in &mut actions {
            action.account_key = format!("{prefix}{}", action.account_key);
            action.attempt_key = format!("{prefix}{}", action.attempt_key);
        }
        (actions, pool_limited)
    }

    async fn confirm(
        &self,
        context: &CodexResetContext,
        actions: &[CodexResetAction],
        settings: CodexResetSettings,
    ) -> bool {
        let Some(first) = actions.first() else { return false };
        if self.coordinator.state.lock().unwrap().closed || !self.host.is_current(context) {
            return false;
        }
        if !should_prompt_codex_auto_redeem(settings.auto_redeem) {
            return settings.auto_redeem == CodexAutoRedeemMode::Yes;
        }
        if !self.host.has_ui() {
            let notify = self.coordinator.state.lock().unwrap().notified_keys.insert(first.attempt_key.clone());
            if notify {
                self.notice("warning", "Saved Codex resets are eligible, but auto-redeem is unset and no prompt UI is available. Run /usage reset or set codexResets.autoRedeem.");
            }
            return false;
        }
        let choice = self.host.select(actions).await;
        if self.coordinator.state.lock().unwrap().closed || !self.host.is_current(context) {
            return false;
        }
        let mode = match choice {
            Ok(Some(CodexResetConsent::Yes)) => CodexAutoRedeemMode::Yes,
            Ok(Some(CodexResetConsent::No)) => CodexAutoRedeemMode::No,
            _ => return false,
        };
        if self.host.persist_mode(mode).is_err() {
            self.notice("warning", "Saved Codex reset consent could not be saved; no reset was dispatched.");
            return false;
        }
        mode == CodexAutoRedeemMode::Yes
    }

    async fn execute(self: &Arc<Self>, context: &CodexResetContext, actions: Vec<CodexResetAction>) -> PassResult {
        let mut confirmed = Vec::new();
        let mut pool_limited = false;
        for action in actions {
            if !self.host.is_current(context) {
                break;
            }
            // Consent may be revoked while an earlier account is settling.
            // Re-read the Host policy before each separate spend.
            if !self.host.settings().is_ok_and(|settings| settings.auto_redeem == CodexAutoRedeemMode::Yes) {
                break;
            }
            let blocked = action.reason == "blocked-account";
            if blocked && !self.storage.permits_reset_target("openai-codex", &action.target, &context.usage.context).unwrap_or(false) {
                pool_limited = true;
                continue;
            }
            let (attempted_at, previous_attempt) = {
                let mut state = self.coordinator.state.lock().unwrap();
                let now = now_ms();
                if state.closed
                    || state.attempted_keys.contains(&action.attempt_key)
                    || state
                        .last_attempt_at_by_account
                        .get(&action.account_key)
                        .is_some_and(|last| now - last < ATTEMPT_COOLDOWN_MS)
                {
                    continue;
                }
                state.attempted_keys.insert(action.attempt_key.clone());
                let previous = state.last_attempt_at_by_account.insert(action.account_key.clone(), now);
                (now, previous)
            };
            let admission_host = self.host.clone();
            let admission_context = context.clone();
            let admission_coordinator = self.coordinator.clone();
            let pool_denied = Arc::new(AtomicBool::new(false));
            let resolved_guard = if blocked {
                let denied = pool_denied.clone();
                let storage = self.storage.clone();
                let auth_context = context.usage.context.clone();
                Some(Arc::new(move |row: &crate::credential_store::StoredAuthCredential| {
                    let allowed = storage.permits_stored_credential("openai-codex", row, &auth_context);
                    if !allowed { denied.store(true, Ordering::Release); }
                    allowed
                }) as Arc<dyn Fn(&crate::credential_store::StoredAuthCredential) -> bool + Send + Sync>)
            } else { None };
            let outcome = self
                .consume(RedeemResetCreditOptions {
                    target: action.target.clone(),
                    admit_resolved_credential: resolved_guard,
                    base_url_resolver: context.usage.base_url_resolver.clone(),
                    admit_consume: Some(Arc::new(move || {
                        !admission_coordinator.state.lock().unwrap().closed
                            && admission_host.is_current(&admission_context)
                            && admission_host
                                .settings()
                                .is_ok_and(|settings| settings.auto_redeem == CodexAutoRedeemMode::Yes)
                    })),
                    ..Default::default()
                })
                .await;
            if pool_denied.load(Ordering::Acquire) && outcome.as_ref().is_ok_and(|result|
                result.operation.is_none() && result.outcome.code == "account_unavailable"
                    && result.outcome.settlement_error.is_none()) {
                // Proven pre-dispatch denial: undo only this attempt's additions.
                // Unknown/Pending/dispatched operations never enter this branch.
                let mut state = self.coordinator.state.lock().unwrap();
                state.attempted_keys.remove(&action.attempt_key);
                if state.last_attempt_at_by_account.get(&action.account_key) == Some(&attempted_at) {
                    if let Some(previous) = previous_attempt {
                        state.last_attempt_at_by_account.insert(action.account_key.clone(), previous);
                    } else { state.last_attempt_at_by_account.remove(&action.account_key); }
                }
                pool_limited = true;
                continue;
            }
            match outcome {
                Ok(result) => {
                    let is_known =
                        result.operation.as_ref().is_some_and(|receipt| receipt.state == ResetReceiptState::Known);
                    let safe_pre_dispatch = result.operation.is_none()
                        && result.outcome.settlement_error.is_none()
                        && matches!(
                            result.outcome.code.as_str(),
                            "no_account" | "account_unavailable" | "credit_list_failed"
                        );
                    let is_reset = result.outcome.ok && result.outcome.code == "reset";
                    let durable_reset =
                        result.operation.as_ref().is_some_and(ResetOperationReceipt::is_confirmed_reset)
                            && result.outcome.settlement_error.is_none();
                    if safe_pre_dispatch || (is_known && !is_terminal_redeem_outcome(&result.outcome.code)) {
                        let mut state = self.coordinator.state.lock().unwrap();
                        state.attempted_keys.remove(&action.attempt_key);
                        state
                            .deferred_until_by_key
                            .insert(action.attempt_key.clone(), now_ms() + REDEEM_RETRY_DEFER_MS);
                    }
                    if is_reset {
                        if durable_reset {
                            confirmed.push(result.operation.unwrap());
                        }
                        self.notice("info", &format!("Auto-redeemed a saved Codex reset for {}; {}.", action.label,
                            if !durable_reset { "the confirmed response could not be durably settled; automatic replay remains blocked" }
                            else if action.reason == "blocked-account" { "retrying now" } else { "the credit was nearing expiry" }));
                    } else if result.outcome.code == "already_redeemed" && is_known {
                        self.notice(
                            "warning",
                            &format!("A saved Codex reset for {} was already redeemed elsewhere.", action.label),
                        );
                    } else if action.reason == "blocked-account" && result.outcome.code != "no_credit" {
                        self.notice("warning", if safe_pre_dispatch || is_known { "Saved Codex reset was declined; this episode is deferred." }
                            else if result.operation.as_ref().is_some_and(|receipt| matches!(receipt.state, ResetReceiptState::Pending | ResetReceiptState::Unknown)) {
                                "Saved Codex reset outcome is unknown; the account is fenced and the consume will not be replayed."
                            } else { "Saved Codex reset did not yield a confirmed receipt; no replay is authorized." });
                    }
                }
                Err(_) => self
                    .notice("warning", "Saved Codex reset did not yield a confirmed receipt; no replay is authorized."),
            }
        }
        if !confirmed.is_empty() {
            // The pass/sweep guard remains held during refresh, preventing its
            // successful snapshot from recursively scheduling another sweep.
            let _ = self.storage.fetch_usage_reports_with_options(&context.usage, &CancellationToken::new()).await;
        }
        SharedResetPass { receipts: confirmed, pool_limited }
    }

    async fn consume(&self, options: RedeemResetCreditOptions) -> Result<ResetRedemption, AuthStorageError> {
        // A fresh GET cannot release an unknown operation, even when its
        // balance now says zero or the credential row was replaced.
        let provider = options.provider.as_deref().unwrap_or("openai-codex");
        let base = options.base_url_resolver.as_ref().and_then(|resolve| resolve(provider));
        let key = self.storage.reset_operation_account_key(
            provider,
            base.as_deref(),
            options.target.credential_id.unwrap_or(0),
            options.target.account_id.as_deref(),
            options.target.email.as_deref(),
        )?;
        if let Some(fence) =
            self.receipts.receipts().map_err(|_| AuthStorageError::Storage)?.into_iter().find(|receipt| {
                receipt.fences_account(
                    &key,
                    options.target.credential_id,
                    options.target.account_id.as_deref(),
                    options.target.email.as_deref(),
                )
            })
        {
            return Ok(ResetRedemption {
                outcome: crate::auth_storage::ResetCreditRedeemOutcome {
                    ok: false,
                    code: "outcome_unknown".into(),
                    account_id: fence.account_id.clone(),
                    email: fence.email.clone(),
                    credit_id: Some(fence.credit_id.clone()),
                    settlement_error: None,
                },
                operation: Some(fence),
            });
        }
        let token = CancellationToken::new();
        let deadline = token.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            deadline.cancel();
        });
        let result = self.storage.redeem_reset_credit_observed(&options, self.receipts.clone(), &token).await;
        timer.abort();
        result
    }

    pub async fn list(
        self: &Arc<Self>,
        context: &CodexResetContext,
        cancel: &CancellationToken,
    ) -> Result<Vec<ResetCreditAccountStatus>, AuthStorageError> {
        let mut statuses = self
            .storage
            .list_reset_credits(
                &ListResetCreditsOptions {
                    session_id: context.usage.context.session_id.clone(),
                    base_url_resolver: context.usage.base_url_resolver.clone(),
                    ..Default::default()
                },
                cancel,
            )
            .await?;
        let collation =
            crate::catalog_discovery::CatalogCollation::for_host().map_err(|_| AuthStorageError::Configuration)?;
        statuses.sort_by(|a, b| {
            b.active
                .cmp(&a.active)
                .then_with(|| b.available_count.total_cmp(&a.available_count))
                .then_with(|| collation.compare(&label(a).into(), &label(b).into()))
        });
        Ok(statuses)
    }

    pub async fn manual(
        self: &Arc<Self>,
        context: CodexResetContext,
        selector: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<ResetRedemption>, AuthStorageError> {
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled);
        }
        let (context, _) = self.freeze_context(context)?;
        let statuses = self.list(&context, cancel).await?;
        if cancel.is_cancelled() {
            return Err(AuthStorageError::Cancelled);
        }
        let selector = ara_prompt::js::trim(selector).to_lowercase();
        let selected = statuses.iter().find(|status| {
            if selector == "active" {
                status.active
            } else {
                [status.account_id.as_deref(), status.email.as_deref(), Some(label(status))]
                    .into_iter()
                    .flatten()
                    .any(|value| value.to_lowercase() == selector)
            }
        });
        let Some(selected) = selected else { return Ok(None) };
        let options = RedeemResetCreditOptions {
            target: crate::auth_storage::ResetCreditTarget {
                credential_id: Some(selected.credential_id),
                account_id: selected.account_id.clone(),
                email: selected.email.clone(),
            },
            base_url_resolver: context.usage.base_url_resolver.clone(),
            ..Default::default()
        };
        let (tx, rx) = watch::channel(None);
        {
            let mut state = self.coordinator.state.lock().unwrap();
            if state.closed || cancel.is_cancelled() {
                return Err(AuthStorageError::Cancelled);
            }
            state.jobs += 1;
        }
        let controller = self.clone();
        let guard = JobGuard { coordinator: self.coordinator.clone(), account: None, sweep: None };
        tokio::spawn(async move {
            let _guard = guard;
            let result = controller.consume(options).await;
            let _ = tx.send(Some(result));
        });
        let mut rx = rx;
        loop {
            if let Some(result) = rx.borrow().clone() {
                return result.map(Some);
            }
            tokio::select! { biased; _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
            changed = rx.changed() => if changed.is_err() { return Err(AuthStorageError::Storage) } }
        }
    }
}

pub fn label(status: &ResetCreditAccountStatus) -> &str {
    status.email.as_deref().or(status.account_id.as_deref()).unwrap_or("account")
}

struct ResetRequestAuth {
    inner: Arc<dyn RequestAuthResolver>,
    controller: Arc<CodexResetController>,
    session_id: Option<String>,
    base_url_resolver: Option<CredentialBaseUrlResolver>,
    host_epoch: Option<u64>,
    session_scope: Option<Arc<AuthSessionScope>>,
}

#[async_trait]
impl RequestAuthResolver for ResetRequestAuth {
    #[cfg(feature = "test-fixture")]
    fn session_scope_receipt(&self) -> Option<serde_json::Value> {
        self.inner.session_scope_receipt()
    }
    async fn resolve(&self, model: &Model, cancel: &CancellationToken) -> Result<RequestAuthLease, AuthResolveError> {
        self.inner.resolve(model, cancel).await
    }
    fn requires_settlement(&self) -> bool {
        self.inner.requires_settlement()
    }
    fn supports_auth_refresh(&self) -> bool {
        self.inner.supports_auth_refresh()
    }
    fn supports_auth_retry(&self) -> bool {
        self.inner.supports_auth_retry()
    }
    async fn refresh(
        &self,
        model: &Model,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        self.inner.refresh(model, cancel).await
    }
    async fn retry(
        &self,
        model: &Model,
        failed: &RequestAuthLease,
        failure: &RequestAuthFailure,
        action: AuthRetryAction,
        cancel: &CancellationToken,
    ) -> Result<Option<RequestAuthLease>, AuthResolveError> {
        self.inner.retry(model, failed, failure, action, cancel).await
    }
    fn for_session(&self, id: &str) -> Option<Arc<dyn RequestAuthResolver>> {
        self.inner
            .for_session(id)
            .map(|inner| if let Some(scope) = &self.session_scope {
                self.controller.decorate_scoped(inner, Some(id.to_owned()), self.base_url_resolver.clone(), scope.clone())
            } else { self.controller.decorate(inner, Some(id.to_owned()), self.base_url_resolver.clone()) })
    }
    fn for_primary_session(&self, id: &str) -> Option<Arc<dyn RequestAuthResolver>> {
        // Preparing an immutable scope does not admit a new Host epoch.
        // The existing committed for_session/decorate path captures that epoch.
        self.inner.for_primary_session(id).map(|inner| Arc::new(Self {
            inner,
            controller: self.controller.clone(),
            session_id: Some(id.to_owned()),
            base_url_resolver: self.base_url_resolver.clone(),
            session_scope: self.session_scope.as_ref().map(|scope| scope.for_primary(id)),
            host_epoch: self.host_epoch,
        }) as Arc<dyn RequestAuthResolver>)
    }
    async fn quota_reset(
        &self,
        model: &Model,
        failed: &RequestAuthLease,
        failure: &RequestAuthFailure,
        cancel: &CancellationToken,
    ) -> Result<Option<QuotaResetReplay>, AuthResolveError> {
        if model.provider != "openai-codex" || failure.kind != RequestAuthFailureKind::Quota || cancel.is_cancelled() {
            return Ok(None);
        }
        let CredentialIdentity::Stored { id, .. } = failed.identity() else { return Ok(None) };
        let Some(identity) = self
            .controller
            .storage
            .list_oauth_accounts_with_context("openai-codex", &crate::auth_storage::AuthRequestContext {
                session_id: self.session_id.clone(), session_scope: self.session_scope.clone(), ..Default::default()
            })
            .ok()
            .and_then(|accounts| accounts.into_iter().find(|account| account.credential_id == *id))
        else {
            return Ok(None);
        };
        let mut context = CodexResetContext::for_model(model, self.session_id.clone(), self.base_url_resolver.clone());
        if let Some(scope) = &self.session_scope { context = context.with_session_scope(scope.clone()); }
        context.host_epoch = self.host_epoch;
        let Ok((context, scope)) = self.controller.freeze_context(context) else { return Ok(None) };
        let unblock = failure.retry_after_ms.filter(|wait| wait.is_finite() && *wait > 0.0).map(|wait| now_ms() + wait);
        let receipts = self.controller.blocked(context.clone(), identity, unblock, cancel).await;
        if cancel.is_cancelled() || receipts.is_empty() || !self.controller.host.is_current(&context) {
            return Ok(None);
        }
        let lease = self.inner.resolve(model, cancel).await?;
        if cancel.is_cancelled() || !self.controller.host.is_current(&context) {
            return Ok(None);
        }
        let restored = match lease.identity() {
            CredentialIdentity::Stored { id, .. } => Some(*id),
            _ => None,
        };
        let receipt = receipts.iter().find(|receipt| {
            Some(receipt.credential_id) == restored && receipt.account_key.starts_with(&format!("{scope}:"))
                && self.controller.receipt_usable(&context, receipt)
        });
        Ok(receipt.and_then(|receipt| QuotaResetReplay::from_confirmed(lease, receipt)))
    }
}


#[cfg(test)]
mod cold_pool_tests {
    use super::*;
    use crate::auth_storage::{AggregateUsageSource, AuthRequestContext, AuthStorageOptions, OAuthProvider, ResolvedOAuth};
    use crate::auth_storage_policy::{UsageAmount, UsageLimit, UsageWindow, UsageResetCredits};
    use crate::credential_store::{AuthCredential, SqliteCredentialStore, StoredAuthCredential};
    use crate::codex_reset_receipts::{ResetReceiptCode, SqliteResetReceiptStore};
    use crate::codex_usage::tests::{HttpReply, fixture, fixture_provider};
    use std::collections::BTreeMap;

    struct Host;
    #[async_trait]
    impl CodexResetHost for Host {
        fn settings(&self) -> Result<CodexResetSettings, String> {
            Ok(CodexResetSettings { auto_redeem: CodexAutoRedeemMode::Yes, salvage_horizon_hours: 0.0, ..Default::default() })
        }
        fn persist_mode(&self, _: CodexAutoRedeemMode) -> Result<(), String> { Ok(()) }
        fn notice(&self, _: &str, _: &str) {}
    }
    struct FreshOAuth;
    #[async_trait]
    impl OAuthProvider for FreshOAuth {
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
            let AuthCredential::OAuth { fields } = &row.credential else { return Err(AuthStorageError::Unavailable) };
            Ok(RequestAuthLease::new(CredentialIdentity::Stored { id: row.id, revision: row.revision },
                fields.get("access").and_then(serde_json::Value::as_str).map(str::to_owned)))
        }
        async fn resolve(&self, row: StoredAuthCredential, _: bool, _: &CancellationToken) -> Result<ResolvedOAuth, AuthStorageError> {
            Ok(ResolvedOAuth { lease: self.lease(&row)?, row })
        }
    }
    struct Reports(Vec<UsageReport>);
    #[async_trait]
    impl AggregateUsageSource for Reports {
        async fn fetch_usage_reports(&self) -> Result<Option<Vec<UsageReport>>, AuthStorageError> { Ok(Some(self.0.clone())) }
    }
    fn report(email: &str, now: f64) -> UsageReport {
        UsageReport { provider: "openai-codex".into(), fetched_at: now,
            metadata: Some(serde_json::json!({"email":email,"accountId":email,"limitReached":true}).as_object().unwrap().clone()),
            limits: vec![UsageLimit { id: "openai-codex:primary".into(),
                amount: UsageAmount { used_fraction: Some(1.0), ..Default::default() },
                window: Some(UsageWindow { resets_at: Some(now + 7_200_000.0), ..Default::default() }), ..Default::default() }],
            reset_credits: Some(UsageResetCredits { available_count: 1.0, ..Default::default() }), ..Default::default() }
    }
    fn scope(email: &str) -> Arc<AuthSessionScope> {
        AuthSessionScope::new("primary".into(), BTreeMap::from([("openai-codex".into(),
            [format!("email:{email}")].into_iter().collect())]))
    }

    #[tokio::test]
    async fn shared_unusable_or_pool_limited_pass_settles_before_waiter_plans_own_consume() {
        for empty_shared in [false, true] {
            let now = now_ms();
            let mut http = fixture(vec![
                HttpReply::json(200, serde_json::json!({"credits":[{"id":"credit"}]})),
                HttpReply::json(200, serde_json::json!({"credits":[{"id":"credit"}]})),
                HttpReply::json(200, serde_json::json!({"credits":[{"id":"credit"}]})),
                HttpReply::json(200, serde_json::json!({"code":"reset"})),
            ]).await;
            let store = Arc::new(Mutex::new(SqliteCredentialStore::from_connection(
                rusqlite::Connection::open_in_memory().unwrap(), Duration::from_millis(20)).unwrap()));
            let credentials: Vec<_> = ["a@fixture", "b@fixture"].into_iter().map(|email| AuthCredential::oauth(
                serde_json::json!({"email":email,"accountId":email,"access":format!("access-{email}"),"refresh":"refresh","expires":now+7_200_000.0})
                    .as_object().unwrap().clone())).collect();
            store.lock().unwrap().replace_auth_credentials_for_provider("openai-codex", &credentials).unwrap();
            let storage = Arc::new(AuthStorage::new(store, None, AuthStorageOptions {
                reset_credit_client: Some(fixture_provider(&http.base, Arc::new(Mutex::new(Vec::new())))),
                aggregate_usage_override: Some(Arc::new(Reports(vec![report("a@fixture", now), report("b@fixture", now)]))),
                ..Default::default()
            }).unwrap());
            storage.register_oauth_provider("openai-codex", Arc::new(FreshOAuth)).unwrap();
            let auth = scope("a@fixture").context_for("openai-codex", &AuthRequestContext::default());
            let identity = storage.get_oauth_account_identity_with_context("openai-codex", &auth).unwrap().unwrap();
            let b = storage.list_oauth_accounts("openai-codex", None).unwrap().into_iter().find(|row| row.email.as_deref()==Some("b@fixture")).unwrap();
            let context = CodexResetContext { provider: "openai-codex".into(), model_id: "gpt-5.6-sol".into(), host_epoch: None,
                usage: FetchUsageReportsOptions { context: auth, ..Default::default() } };
            let coordinator = Arc::new(CodexResetCoordinator::default());
            let controller = CodexResetController::new(storage.clone(), Arc::new(SqliteResetReceiptStore::memory().unwrap()), Arc::new(Host), coordinator.clone());
            let key = storage.reset_operation_account_key("openai-codex", None, identity.credential_id,
                identity.account_id.as_deref(), identity.email.as_deref()).unwrap();
            let (tx, rx) = watch::channel(None);
            { let mut state = coordinator.state.lock().unwrap(); state.in_flight_by_account.insert(key.clone(), rx); state.jobs += 1; }
            let original = JobGuard { coordinator: coordinator.clone(), account: Some(key), sweep: None };
            let child = controller.clone(); let child_context = context.clone(); let child_identity = identity.clone();
            let waiter = tokio::spawn(async move { child.blocked(child_context, child_identity, Some(now+7_200_000.0), &CancellationToken::new()).await });
            tokio::time::timeout(Duration::from_secs(2), async {
                while tx.receiver_count() < 2 { tokio::task::yield_now().await; }
            }).await.unwrap();
            let receipt = ResetOperationReceipt { request_id: "earlier-flight".into(),
                account_key: storage.reset_operation_account_key("openai-codex", None, b.credential_id, b.account_id.as_deref(), b.email.as_deref()).unwrap(),
                credential_id: b.credential_id, account_id: b.account_id, email: b.email, credit_id: "earlier-credit".into(),
                endpoint_fingerprint: "fixture".into(), created_at: now, updated_at: now,
                state: ResetReceiptState::Known, code: ResetReceiptCode::Reset };
            let result = SharedResetPass { receipts: if empty_shared { Vec::new() } else { vec![receipt] }, pool_limited: empty_shared };
            assert!(!controller.pass_usable(&context, &result));
            drop(original);
            tx.send(Some(result)).unwrap();
            let actual = tokio::time::timeout(Duration::from_secs(5), waiter).await.unwrap().unwrap();
            assert_eq!(actual.len(), 1); assert_eq!(actual[0].credential_id, identity.credential_id);
            assert!(actual[0].is_confirmed_reset());
            let mut posts = Vec::new();
            while let Ok(request) = http.requests.try_recv() { if request.method=="POST" { posts.push(request); } }
            assert_eq!(posts.len(), 1);
            assert_eq!(posts[0].headers.get("chatgpt-account-id").map(String::as_str), Some("a@fixture"));
            coordinator.wait_for_settlement().await;
            assert!(coordinator.state.lock().unwrap().in_flight_by_account.is_empty());
        }
    }


    struct SwapOAuth { store: Arc<Mutex<SqliteCredentialStore>>, replace: AtomicBool }
    #[async_trait]
    impl OAuthProvider for SwapOAuth {
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> { FreshOAuth.lease(row) }
        async fn resolve(&self, row: StoredAuthCredential, force: bool, cancel: &CancellationToken) -> Result<ResolvedOAuth, AuthStorageError> {
            FreshOAuth.resolve(row, force, cancel).await
        }
        async fn prepare_for_request(&self, row: &StoredAuthCredential, _: &CancellationToken) -> Result<(), AuthStorageError> {
            if self.replace.swap(false, Ordering::AcqRel) {
                let mut credential = row.credential.clone();
                if let AuthCredential::OAuth { fields } = &mut credential {
                    fields.insert("email".into(), "b@fixture".into()); fields.insert("accountId".into(), "b@fixture".into());
                }
                self.store.lock().unwrap().try_update_auth_credential_if_matches(row.id, &row.serialized_data, &credential, None).unwrap();
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn final_pool_denial_restores_previous_cooldown_but_unknown_consume_stays_fenced() {
        for replace_identity in [true, false] {
            let now = now_ms();
            let mut http = fixture(vec![
                HttpReply::json(200, serde_json::json!({"credits":[{"id":"credit"}]})),
                HttpReply::json(500, serde_json::json!({"code":"reset"})),
            ]).await;
            let store = Arc::new(Mutex::new(SqliteCredentialStore::from_connection(
                rusqlite::Connection::open_in_memory().unwrap(), Duration::from_millis(20)).unwrap()));
            store.lock().unwrap().replace_auth_credentials_for_provider("openai-codex", &[AuthCredential::oauth(
                serde_json::json!({"email":"a@fixture","accountId":"a@fixture","access":"access-a","refresh":"refresh","expires":now+7_200_000.0})
                    .as_object().unwrap().clone())]).unwrap();
            let storage = Arc::new(AuthStorage::new(store.clone(), None, AuthStorageOptions {
                reset_credit_client: Some(fixture_provider(&http.base, Arc::new(Mutex::new(Vec::new())))), ..Default::default()
            }).unwrap());
            storage.register_oauth_provider("openai-codex", Arc::new(SwapOAuth { store, replace: AtomicBool::new(replace_identity) })).unwrap();
            let auth = scope("a@fixture").context_for("openai-codex", &AuthRequestContext::default());
            let identity = storage.get_oauth_account_identity_with_context("openai-codex", &auth).unwrap().unwrap();
            let context = CodexResetContext { provider: "openai-codex".into(), model_id: "gpt-5.6-sol".into(), host_epoch: None,
                usage: FetchUsageReportsOptions { context: auth, ..Default::default() } };
            let coordinator = Arc::new(CodexResetCoordinator::default());
            let journal = Arc::new(SqliteResetReceiptStore::memory().unwrap());
            let controller = CodexResetController::new(storage.clone(), journal.clone(), Arc::new(Host), coordinator.clone());
            let target = crate::auth_storage::ResetCreditTarget { credential_id: Some(identity.credential_id),
                email: identity.email, account_id: identity.account_id };
            let account_key = storage.reset_operation_account_key("openai-codex", None, identity.credential_id,
                target.account_id.as_deref(), target.email.as_deref()).unwrap();
            let attempt_key = format!("{account_key}:test-attempt");
            let previous = now - 2.0 * ATTEMPT_COOLDOWN_MS;
            coordinator.state.lock().unwrap().last_attempt_at_by_account.insert(account_key.clone(), previous);
            let action = CodexResetAction { reason: "blocked-account".into(), target: target.clone(), account_key: account_key.clone(),
                attempt_key: attempt_key.clone(), label: "fixture".into(), ..Default::default() };
            let result = controller.execute(&context, vec![action]).await;
            assert!(result.receipts.is_empty());
            assert_eq!(http.requests.recv().await.unwrap().method, "GET");
            let state = coordinator.state.lock().unwrap();
            if replace_identity {
                assert!(result.pool_limited);
                assert!(!state.attempted_keys.contains(&attempt_key));
                assert_eq!(state.last_attempt_at_by_account.get(&account_key), Some(&previous));
                assert!(!state.deferred_until_by_key.contains_key(&attempt_key));
                assert!(journal.receipts().unwrap().is_empty());
                assert!(http.requests.try_recv().is_err());
            } else {
                assert!(!result.pool_limited);
                assert!(state.attempted_keys.contains(&attempt_key));
                assert_ne!(state.last_attempt_at_by_account.get(&account_key), Some(&previous));
                assert_eq!(journal.receipts().unwrap()[0].state, ResetReceiptState::Unknown);
                assert_eq!(http.requests.try_recv().unwrap().method, "POST");
                drop(state);
                let again = controller.consume(RedeemResetCreditOptions { target,
                    credit_id: Some("credit".into()), ..Default::default() }).await.unwrap();
                assert_eq!(again.outcome.code, "outcome_unknown");
                assert_eq!(again.operation.unwrap().state, ResetReceiptState::Unknown);
                assert!(http.requests.try_recv().is_err());
            }
        }
    }

}
