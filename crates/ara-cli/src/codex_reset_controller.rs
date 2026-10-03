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
    AuthStorage, AuthStorageError, CredentialBaseUrlResolver, FetchUsageReportsOptions, ListResetCreditsOptions,
    RedeemResetCreditOptions, ResetCreditAccountStatus, ResetRedemption,
};
use crate::auth_storage_policy::UsageReport;
use crate::auth_storage_state::OAuthAccountSummary;
use crate::codex_auto_reset::{
    ATTEMPT_COOLDOWN_MS, CodexAutoRedeemMode, CodexResetAction, CodexResetPlanInput, CodexResetSettings,
    CodexResetTrigger, REDEEM_RETRY_DEFER_MS, SWEEP_MIN_INTERVAL_MS, is_terminal_redeem_outcome,
    overlay_live_reset_credits, plan_codex_resets, should_evaluate_codex_auto_redeem, should_prompt_codex_auto_redeem,
};
use crate::codex_reset_receipts::{ResetOperationReceipt, ResetReceiptState, ResetReceiptStore};
use crate::model_route::{
    AuthResolveError, AuthRetryAction, CredentialIdentity, QuotaResetReplay, RequestAuthFailure,
    RequestAuthFailureKind, RequestAuthLease, RequestAuthResolver,
};
use ara_ai::Model;
use async_trait::async_trait;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
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

type PassResult = Vec<ResetOperationReceipt>;
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
            _ = cancel.cancelled() => return Vec::new(),
            changed = receiver.changed() => if changed.is_err() { return Vec::new() },
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
            let actions =
                controller.plan(&context, CodexResetTrigger::Sweep, Some(&reports), identity.as_ref(), None, settings);
            let result = if controller.confirm(&context, &actions, settings).await {
                controller.execute(&context, actions).await
            } else {
                Vec::new()
            };
            let _ = tx.send(Some(result));
        });
    }

    pub async fn blocked(
        self: &Arc<Self>,
        context: CodexResetContext,
        identity: OAuthAccountSummary,
        unblock_at_ms: Option<f64>,
        cancel: &CancellationToken,
    ) -> PassResult {
        let Ok(settings) = self.host.settings() else { return Vec::new() };
        if cancel.is_cancelled()
            || !should_evaluate_codex_auto_redeem(settings.auto_redeem)
            || context.provider != "openai-codex"
        {
            return Vec::new();
        }
        let Ok((context, scope)) = self.freeze_context(context) else { return Vec::new() };
        let base = context.usage.base_url_resolver.as_ref().and_then(|resolve| resolve("openai-codex"));
        let Ok(key) = self.storage.reset_operation_account_key(
            "openai-codex",
            base.as_deref(),
            identity.credential_id,
            identity.account_id.as_deref(),
            identity.email.as_deref(),
        ) else {
            return Vec::new();
        };
        let (tx, rx) = watch::channel(None);
        let (existing, sweep) = {
            let mut state = self.coordinator.state.lock().unwrap();
            if state.closed || cancel.is_cancelled() {
                return Vec::new();
            }
            if let Some(existing) = state.in_flight_by_account.get(&key) {
                (Some(existing.clone()), None)
            } else {
                let sweep = state
                    .sweeps_in_flight
                    .contains(&scope)
                    .then(|| state.sweep_settlement.get(&scope).cloned())
                    .flatten();
                state.in_flight_by_account.insert(key.clone(), rx.clone());
                state.jobs += 1;
                (None, sweep)
            }
        };
        if let Some(existing) = existing {
            return await_pass(existing, cancel).await;
        }
        let controller = self.clone();
        let guard = JobGuard { coordinator: self.coordinator.clone(), account: Some(key), sweep: None };
        tokio::spawn(async move {
            let _guard = guard;
            // Fixed native admission gates sweeps on blocked work in one
            // direction. Serialize the reverse direction as an ARA safety
            // difference, adopting only this scope's durable confirmations.
            let adopted = if let Some(sweep) = sweep {
                await_pass(sweep, &CancellationToken::new())
                    .await
                    .into_iter()
                    .filter(|receipt| {
                        receipt.is_confirmed_reset() && receipt.account_key.starts_with(&format!("{scope}:"))
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let result = if !adopted.is_empty() {
                adopted
            } else if controller.coordinator.state.lock().unwrap().closed
                || !controller.host.is_current(&context)
                || !controller
                    .host
                    .settings()
                    .is_ok_and(|settings| should_evaluate_codex_auto_redeem(settings.auto_redeem))
            {
                Vec::new()
            } else {
                controller.blocked_pass(&context, &identity, unblock_at_ms, settings).await
            };
            let _ = tx.send(Some(result));
        });
        await_pass(rx, cancel).await
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
            return Vec::new();
        }
        let Ok(reports) = self.fetch_usage_reports(context, &token).await else { return Vec::new() };
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
        let actions = self.plan(
            context,
            CodexResetTrigger::Blocked,
            effective.as_deref(),
            Some(identity),
            unblock_at_ms,
            settings,
        );
        if self.confirm(context, &actions, settings).await { self.execute(context, actions).await } else { Vec::new() }
    }

    fn plan(
        &self,
        context: &CodexResetContext,
        trigger: CodexResetTrigger,
        reports: Option<&[UsageReport]>,
        identity: Option<&OAuthAccountSummary>,
        unblock_at_ms: Option<f64>,
        settings: CodexResetSettings,
    ) -> Vec<CodexResetAction> {
        let Ok(scope) = self.reset_scope(context) else { return Vec::new() };
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
        let mut actions = plan_codex_resets(&CodexResetPlanInput {
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
        })
        .actions;
        for action in &mut actions {
            action.account_key = format!("{prefix}{}", action.account_key);
            action.attempt_key = format!("{prefix}{}", action.attempt_key);
        }
        actions
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
        for action in actions {
            if !self.host.is_current(context) {
                break;
            }
            // Consent may be revoked while an earlier account is settling.
            // Re-read the Host policy before each separate spend.
            if !self.host.settings().is_ok_and(|settings| settings.auto_redeem == CodexAutoRedeemMode::Yes) {
                break;
            }
            {
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
                state.last_attempt_at_by_account.insert(action.account_key.clone(), now);
            }
            let admission_host = self.host.clone();
            let admission_context = context.clone();
            let admission_coordinator = self.coordinator.clone();
            let outcome = self
                .consume(RedeemResetCreditOptions {
                    target: action.target.clone(),
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
        confirmed
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
}

#[async_trait]
impl RequestAuthResolver for ResetRequestAuth {
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
            .map(|inner| self.controller.decorate(inner, Some(id.to_owned()), self.base_url_resolver.clone()))
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
            .list_oauth_accounts("openai-codex", self.session_id.as_deref())
            .ok()
            .and_then(|accounts| accounts.into_iter().find(|account| account.credential_id == *id))
        else {
            return Ok(None);
        };
        let mut context = CodexResetContext::for_model(model, self.session_id.clone(), self.base_url_resolver.clone());
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
        });
        Ok(receipt.and_then(|receipt| QuotaResetReplay::from_confirmed(lease, receipt)))
    }
}
