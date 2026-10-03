//! Pure saved-reset planner from fixed OMP
//! 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/coding-agent/src/session/codex-auto-reset.ts:71-638 and
//! slash-commands/helpers/active-oauth-account.ts:18-90.
//! Hosts own consent, process-wide coordination and consume receipts. A plan
//! is neither permission to spend a credit nor evidence that a POST completed.
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

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use ara_prompt::js::{f64_to_string, trim};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::auth_storage::{ResetCreditAccountStatus, ResetCreditTarget};
use crate::auth_storage_policy::{UsageLimit, UsageReport, UsageResetCreditDetail, UsageResetCredits};
use crate::auth_storage_state::OAuthAccountSummary;

pub const WINDOW_EXHAUSTED_MIN_FRACTION: f64 = 0.999;
pub const MAX_PLAUSIBLE_WEEKLY_REMAINING_MS: f64 = 7.0 * 24.0 * 3_600_000.0 + 3_600_000.0;
pub const MAX_PLAUSIBLE_PRIMARY_REMAINING_MS: f64 = 6.0 * 3_600_000.0;
pub const SALVAGE_MIN_USED_FRACTION: f64 = 0.25;
pub const REDEEM_RETRY_DEFER_MS: f64 = 30.0 * 60_000.0;
pub const REPORT_FRESHNESS_MS: f64 = 10.0 * 60_000.0;
pub const ATTEMPT_COOLDOWN_MS: f64 = 60_000.0;
pub const DEBOUNCE_BUCKET_MS: f64 = 60_000.0;
pub const SWEEP_MIN_INTERVAL_MS: f64 = 60_000.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexAutoRedeemMode {
    #[default]
    Unset,
    Yes,
    No,
}

impl CodexAutoRedeemMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Yes => "yes",
            Self::No => "no",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetSettings {
    pub auto_redeem: CodexAutoRedeemMode,
    pub min_blocked_minutes: f64,
    pub keep_credits: f64,
    pub salvage_horizon_hours: f64,
}

impl Default for CodexResetSettings {
    fn default() -> Self {
        Self {
            auto_redeem: CodexAutoRedeemMode::Unset,
            min_blocked_minutes: 60.0,
            keep_credits: 0.0,
            salvage_horizon_hours: 12.0,
        }
    }
}

pub fn should_evaluate_codex_auto_redeem(mode: CodexAutoRedeemMode) -> bool {
    mode != CodexAutoRedeemMode::No
}

pub fn should_prompt_codex_auto_redeem(mode: CodexAutoRedeemMode) -> bool {
    mode == CodexAutoRedeemMode::Unset
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodexResetTrigger {
    Blocked,
    Sweep,
}

pub struct CodexResetPlanInput<'a> {
    pub now_ms: f64,
    pub trigger: CodexResetTrigger,
    pub provider: &'a str,
    pub model_id: &'a str,
    pub settings: CodexResetSettings,
    pub identity: Option<&'a OAuthAccountSummary>,
    pub reports: Option<&'a [UsageReport]>,
    pub attempted_keys: &'a HashSet<String>,
    pub deferred_until_by_key: &'a HashMap<String, f64>,
    pub last_attempt_at_by_account: &'a HashMap<String, f64>,
    pub active_block_unblock_at_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetAction {
    pub reason: String,
    pub target: ResetCreditTarget,
    pub account_key: String,
    pub attempt_key: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available_count: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_used_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_windows: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salvage_window: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salvage_used_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_ms: Option<f64>,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetSkip {
    pub account_key: String,
    pub rule: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CodexResetPlan {
    pub actions: Vec<CodexResetAction>,
    pub skipped: Vec<CodexResetSkip>,
}

struct AccountSnapshot {
    account_key: String,
    target: ResetCreditTarget,
    label: String,
    active: bool,
    available_count: Option<f64>,
    primary_used: Option<f64>,
    primary_resets_at: Option<f64>,
    weekly_used: Option<f64>,
    weekly_resets_at: Option<f64>,
    limit_reached: bool,
    credit_expires_at_ms: Option<f64>,
}

struct RestoreCandidate<'a> {
    snapshot: &'a AccountSnapshot,
    remaining_ms: f64,
    unblock_at_ms: f64,
    blocked_windows: Vec<String>,
}

fn skip(skipped: &mut Vec<CodexResetSkip>, account_key: &str, rule: &str, reason: &str) {
    skipped.push(CodexResetSkip { account_key: account_key.into(), rule: rule.into(), reason: reason.into() });
}

fn metadata_string<'a>(metadata: Option<&'a Map<String, Value>>, key: &str) -> Option<&'a str> {
    metadata.and_then(|metadata| metadata.get(key)).and_then(Value::as_str)
}

fn nonblank(value: Option<&str>) -> Option<&str> {
    value.filter(|value| !trim(value).is_empty())
}

fn normalize_identity_value(value: Option<&str>) -> Option<String> {
    nonblank(value).map(|value| trim(value).to_lowercase())
}

fn limit_matches_active_account(report: &UsageReport, limit: &UsageLimit, identity: &OAuthAccountSummary) -> bool {
    let metadata = report.metadata.as_ref();
    let active_account = normalize_identity_value(identity.account_id.as_deref());
    let active_email = normalize_identity_value(identity.email.as_deref());
    let active_project = normalize_identity_value(identity.project_id.as_deref());
    let active_org = normalize_identity_value(identity.org_id.as_deref());
    let report_org = normalize_identity_value(metadata_string(metadata, "orgId"));
    if active_org.is_some() || report_org.is_some() {
        if active_org != report_org {
            return false;
        }
        if active_account.is_none() && active_email.is_none() && active_project.is_none() {
            return true;
        }
    }
    if let Some(active_account) = active_account {
        let report_account = normalize_identity_value(metadata_string(metadata, "accountId"))
            .or_else(|| normalize_identity_value(metadata_string(metadata, "account_id")));
        if report_account.as_deref() == Some(active_account.as_str())
            || normalize_identity_value(limit.scope.account_id.as_deref()).as_deref() == Some(active_account.as_str())
        {
            return true;
        }
    }
    if active_email.is_some() && normalize_identity_value(metadata_string(metadata, "email")) == active_email {
        return true;
    }
    if let Some(active_project) = active_project
        && (normalize_identity_value(metadata_string(metadata, "projectId")).as_deref()
            == Some(active_project.as_str())
            || normalize_identity_value(limit.scope.project_id.as_deref()).as_deref() == Some(active_project.as_str()))
    {
        return true;
    }
    false
}

/// Native identity attribution includes the organization gate and at least one
/// usage limit. A metadata-only report does not become the active account.
pub fn report_matches_active_account(report: &UsageReport, identity: Option<&OAuthAccountSummary>) -> bool {
    identity
        .is_some_and(|identity| report.limits.iter().any(|limit| limit_matches_active_account(report, limit, identity)))
}

// Match the existing Codex credit picker: ISO/RFC dates and UTC date-only values.
// Bun's wider Date.parse input grammar remains an explicit platform boundary.
fn parse_credit_expiry(text: &str) -> Option<f64> {
    chrono::DateTime::parse_from_rfc3339(text)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(text))
        .map(|date| date.timestamp_millis() as f64)
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|date| date.and_utc().timestamp_millis() as f64)
        })
}

fn soonest_credit_expiry_ms(credits: Option<&[UsageResetCreditDetail]>, now_ms: f64) -> Option<f64> {
    let mut soonest = None;
    for credit in credits.unwrap_or_default() {
        if credit.status.as_deref().unwrap_or("available") != "available" {
            continue;
        }
        let Some(expiry) = credit.expires_at.as_deref().filter(|text| !text.is_empty()).and_then(parse_credit_expiry)
        else {
            continue;
        };
        if expiry <= now_ms {
            continue;
        }
        if soonest.is_none_or(|best| expiry < best) {
            soonest = Some(expiry);
        }
    }
    soonest
}

// Rust f64::max suppresses a NaN operand; JavaScript Math.max propagates it.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

fn numeric_order(a: f64, b: f64) -> Ordering {
    // Array.sort treats a NaN comparator result as equality and retains ties.
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// At most one restore action, followed by salvages in expiry order. No IO or
/// coordinator mutation occurs here. Live credit availability is checked again
/// by the executor, and unknown consume outcomes require a durable receipt fence.
pub fn plan_codex_resets(input: &CodexResetPlanInput<'_>) -> CodexResetPlan {
    let now_ms = input.now_ms;
    let settings = input.settings;
    let mut plan = CodexResetPlan::default();
    if !should_evaluate_codex_auto_redeem(settings.auto_redeem) {
        skip(&mut plan.skipped, "*", "account", "disabled");
        return plan;
    }
    // Native AgentSession normalizes these knobs before invoking its planner.
    let min_blocked_minutes = js_max(0.0, settings.min_blocked_minutes);
    let keep_credits = js_max(0.0, settings.keep_credits.trunc());
    let salvage_horizon_ms = js_max(0.0, settings.salvage_horizon_hours) * 3_600_000.0;
    let mut blocked_rule_active = input.trigger == CodexResetTrigger::Blocked;
    if blocked_rule_active && input.provider != "openai-codex" {
        blocked_rule_active = false;
        skip(&mut plan.skipped, "*", "blocked-account", "wrong-provider");
    }
    if blocked_rule_active && input.model_id.contains("-spark") {
        blocked_rule_active = false;
        skip(&mut plan.skipped, "*", "blocked-account", "spark-model");
    }
    let salvage_rule_active = salvage_horizon_ms > 0.0;
    let mut snapshots = Vec::new();
    let mut active_has_snapshot = false;
    let mut active_known_no_credits = false;
    for report in input.reports.unwrap_or_default() {
        if report.provider != "openai-codex" {
            continue;
        }
        let metadata = report.metadata.as_ref();
        let account_id = nonblank(metadata_string(metadata, "accountId"));
        let email = nonblank(metadata_string(metadata, "email"));
        let Some(account_key) = normalize_identity_value(account_id.or(email)) else {
            skip(&mut plan.skipped, "*", "account", "no-identity");
            continue;
        };
        let active = report_matches_active_account(report, input.identity);
        if now_ms - report.fetched_at > REPORT_FRESHNESS_MS {
            skip(&mut plan.skipped, &account_key, "account", "stale-report");
            continue;
        }
        let Some(credits) = &report.reset_credits else {
            skip(&mut plan.skipped, &account_key, "account", "credits-unknown");
            continue;
        };
        if credits.available_count < 1.0 {
            if active {
                active_known_no_credits = true;
            }
            skip(&mut plan.skipped, &account_key, "account", "no-credits");
            continue;
        }
        let primary = report.limits.iter().find(|limit| limit.id == "openai-codex:primary");
        let weekly = report.limits.iter().find(|limit| limit.id == "openai-codex:secondary");
        if active {
            active_has_snapshot = true;
        }
        snapshots.push(AccountSnapshot {
            label: email.or(account_id).unwrap_or(&account_key).into(),
            account_key,
            target: ResetCreditTarget {
                account_id: account_id.map(str::to_owned),
                email: email.map(str::to_owned),
                ..Default::default()
            },
            active,
            available_count: Some(credits.available_count),
            primary_used: primary.and_then(|limit| limit.amount.used_fraction),
            primary_resets_at: primary.and_then(|limit| limit.window.as_ref()).and_then(|window| window.resets_at),
            weekly_used: weekly.and_then(|limit| limit.amount.used_fraction),
            weekly_resets_at: weekly.and_then(|limit| limit.window.as_ref()).and_then(|window| window.resets_at),
            limit_reached: metadata.and_then(|metadata| metadata.get("limitReached")) == Some(&Value::Bool(true)),
            credit_expires_at_ms: soonest_credit_expiry_ms(credits.credits.as_deref(), now_ms),
        });
    }
    let cooled_down = |account_key: &str| {
        input.last_attempt_at_by_account.get(account_key).is_some_and(|last_at| now_ms - last_at < ATTEMPT_COOLDOWN_MS)
    };
    let mut restore = None;
    if blocked_rule_active {
        let mut candidates = Vec::new();
        for snapshot in &snapshots {
            let rule = "blocked-account";
            let account = &snapshot.account_key;
            let live_unblock = if snapshot.active { input.active_block_unblock_at_ms } else { None };
            if !snapshot.limit_reached && live_unblock.is_none() {
                skip(&mut plan.skipped, account, rule, "not-limit-reached");
                continue;
            }
            // The top-level wire flag also marks healthy windows exhausted.
            // Only exact chat limit IDs and usedFraction identify the blocker.
            let mut exhausted = Vec::new();
            if snapshot.primary_used.is_some_and(|used| used >= WINDOW_EXHAUSTED_MIN_FRACTION) {
                exhausted.push(("5h", snapshot.primary_resets_at, MAX_PLAUSIBLE_PRIMARY_REMAINING_MS));
            }
            if snapshot.weekly_used.is_some_and(|used| used >= WINDOW_EXHAUSTED_MIN_FRACTION) {
                exhausted.push(("weekly", snapshot.weekly_resets_at, MAX_PLAUSIBLE_WEEKLY_REMAINING_MS));
            }
            let (unblock_at_ms, blocked_windows) = if !exhausted.is_empty() {
                let mut latest = f64::NEG_INFINITY;
                let mut invalid = None;
                for (_, resets_at, plausible_ms) in &exhausted {
                    let Some(resets_at) = *resets_at else {
                        invalid = Some("no-reset-time");
                        break;
                    };
                    if resets_at - now_ms > *plausible_ms {
                        invalid = Some("reset-implausible");
                        break;
                    }
                    if resets_at > latest {
                        latest = resets_at;
                    }
                }
                if let Some(invalid) = invalid {
                    skip(&mut plan.skipped, account, rule, invalid);
                    continue;
                }
                (latest, exhausted.iter().map(|(window, _, _)| (*window).into()).collect())
            } else if let Some(unblock) = live_unblock {
                if unblock - now_ms > MAX_PLAUSIBLE_WEEKLY_REMAINING_MS {
                    skip(&mut plan.skipped, account, rule, "reset-implausible");
                    continue;
                }
                (
                    unblock,
                    vec![if unblock - now_ms > MAX_PLAUSIBLE_PRIMARY_REMAINING_MS { "weekly" } else { "5h" }.into()],
                )
            } else {
                skip(&mut plan.skipped, account, rule, "no-exhausted-window");
                continue;
            };
            let remaining_ms = unblock_at_ms - now_ms;
            if remaining_ms < min_blocked_minutes * 60_000.0 {
                skip(&mut plan.skipped, account, rule, "reset-too-soon");
                continue;
            }
            if snapshot.available_count.unwrap_or(0.0) - keep_credits < 1.0 {
                skip(&mut plan.skipped, account, rule, "reserve");
                continue;
            }
            let attempt_key = blocked_attempt_key(account, unblock_at_ms);
            if input.attempted_keys.contains(&attempt_key) {
                skip(&mut plan.skipped, account, rule, "already-attempted");
                continue;
            }
            if input.deferred_until_by_key.get(&attempt_key).is_some_and(|until| now_ms < *until) {
                skip(&mut plan.skipped, account, rule, "deferred");
                continue;
            }
            if cooled_down(account) {
                skip(&mut plan.skipped, account, rule, "cooldown");
                continue;
            }
            candidates.push(RestoreCandidate { snapshot, remaining_ms, unblock_at_ms, blocked_windows });
        }
        candidates.sort_by(|a, b| {
            if a.snapshot.active != b.snapshot.active {
                return b.snapshot.active.cmp(&a.snapshot.active);
            }
            let a_expiry = a.snapshot.credit_expires_at_ms.unwrap_or(f64::INFINITY);
            let b_expiry = b.snapshot.credit_expires_at_ms.unwrap_or(f64::INFINITY);
            if a_expiry != b_expiry {
                return numeric_order(a_expiry, b_expiry);
            }
            let a_count = a.snapshot.available_count.unwrap_or(0.0);
            let b_count = b.snapshot.available_count.unwrap_or(0.0);
            if a_count != b_count {
                return numeric_order(b_count, a_count);
            }
            numeric_order(b.remaining_ms, a.remaining_ms)
        });
        if let Some(best) = candidates.first() {
            restore = Some(CodexResetAction {
                reason: "blocked-account".into(),
                target: best.snapshot.target.clone(),
                account_key: best.snapshot.account_key.clone(),
                attempt_key: blocked_attempt_key(&best.snapshot.account_key, best.unblock_at_ms),
                label: best.snapshot.label.clone(),
                available_count: best.snapshot.available_count,
                weekly_used_fraction: best.snapshot.weekly_used,
                remaining_ms: Some(best.remaining_ms),
                blocked_windows: Some(best.blocked_windows.clone()),
                expires_in_ms: best.snapshot.credit_expires_at_ms.map(|expiry| expiry - now_ms),
                active: best.snapshot.active,
                ..Default::default()
            });
        } else if let Some(unblock) =
            input.active_block_unblock_at_ms.filter(|_| !active_has_snapshot && !active_known_no_credits)
        {
            let account_id = nonblank(input.identity.and_then(|identity| identity.account_id.as_deref()));
            let email = nonblank(input.identity.and_then(|identity| identity.email.as_deref()));
            let account_key = normalize_identity_value(account_id.or(email));
            let remaining_ms = unblock - now_ms;
            let account = account_key.as_deref().unwrap_or("*");
            let attempt_key = blocked_attempt_key(account, unblock);
            let reason = if account_key.is_none() {
                Some("no-identity")
            } else if keep_credits > 0.0 {
                Some("credits-unknown")
            } else if remaining_ms > MAX_PLAUSIBLE_WEEKLY_REMAINING_MS {
                Some("reset-implausible")
            } else if remaining_ms < min_blocked_minutes * 60_000.0 {
                Some("reset-too-soon")
            } else if input.attempted_keys.contains(&attempt_key) {
                Some("already-attempted")
            } else if input.deferred_until_by_key.get(&attempt_key).copied().unwrap_or(0.0) > now_ms {
                Some("deferred")
            } else if cooled_down(account) {
                Some("cooldown")
            } else {
                None
            };
            if let Some(reason) = reason {
                skip(&mut plan.skipped, account, "blocked-account", reason);
            } else {
                restore = Some(CodexResetAction {
                    reason: "blocked-account".into(),
                    target: ResetCreditTarget {
                        account_id: account_id.map(str::to_owned),
                        email: email.map(str::to_owned),
                        ..Default::default()
                    },
                    account_key: account.into(),
                    attempt_key,
                    label: email.or(account_id).unwrap_or(account).into(),
                    remaining_ms: Some(remaining_ms),
                    blocked_windows: Some(vec![
                        if remaining_ms > MAX_PLAUSIBLE_PRIMARY_REMAINING_MS { "weekly" } else { "5h" }.into(),
                    ]),
                    active: true,
                    ..Default::default()
                });
            }
        }
    }
    let mut salvages = Vec::new();
    if salvage_rule_active {
        for snapshot in &snapshots {
            let account = &snapshot.account_key;
            if restore.as_ref().is_some_and(|restore| &restore.account_key == account) {
                continue;
            }
            let rule = "expiring-credit";
            let Some(expiry) = snapshot.credit_expires_at_ms else {
                skip(&mut plan.skipped, account, rule, "no-expiring-credit");
                continue;
            };
            if expiry - now_ms > salvage_horizon_ms {
                skip(&mut plan.skipped, account, rule, "no-expiring-credit");
                continue;
            }
            let primary_used = snapshot.primary_used.unwrap_or(0.0);
            let weekly_used = snapshot.weekly_used.unwrap_or(0.0);
            let salvage_used = js_max(primary_used, weekly_used);
            if salvage_used < SALVAGE_MIN_USED_FRACTION {
                skip(&mut plan.skipped, account, rule, "window-mostly-free");
                continue;
            }
            let attempt_key = salvage_attempt_key(account, expiry);
            if input.attempted_keys.contains(&attempt_key) {
                skip(&mut plan.skipped, account, rule, "already-attempted");
                continue;
            }
            if input.deferred_until_by_key.get(&attempt_key).is_some_and(|until| now_ms < *until) {
                skip(&mut plan.skipped, account, rule, "deferred");
                continue;
            }
            if cooled_down(account) {
                skip(&mut plan.skipped, account, rule, "cooldown");
                continue;
            }
            salvages.push(CodexResetAction {
                reason: "expiring-credit".into(),
                target: snapshot.target.clone(),
                account_key: account.clone(),
                attempt_key,
                label: snapshot.label.clone(),
                available_count: snapshot.available_count,
                weekly_used_fraction: snapshot.weekly_used,
                salvage_window: Some(if primary_used >= weekly_used { "5h" } else { "weekly" }.into()),
                salvage_used_fraction: Some(salvage_used),
                expires_in_ms: Some(expiry - now_ms),
                active: snapshot.active,
                ..Default::default()
            });
        }
        salvages.sort_by(|a, b| numeric_order(a.expires_in_ms.unwrap_or(0.0), b.expires_in_ms.unwrap_or(0.0)));
    }
    if let Some(restore) = restore {
        plan.actions.push(restore);
    }
    plan.actions.extend(salvages);
    plan
}

// Math.round ties toward positive infinity, including negative half values.
// Using floor(x + .5) introduces rounding errors at large integral doubles.
fn js_round(value: f64) -> f64 {
    if !value.is_finite() || value.abs() >= 4_503_599_627_370_496.0 {
        return value;
    }
    let floor = value.floor();
    if value - floor < 0.5 { floor } else { floor + 1.0 }
}

pub fn blocked_attempt_key(account_key: &str, resets_at_ms: f64) -> String {
    format!("block|{account_key}|{}", f64_to_string(js_round(resets_at_ms / DEBOUNCE_BUCKET_MS)))
}

pub fn salvage_attempt_key(account_key: &str, expiry_ms: f64) -> String {
    format!("salvage|{account_key}|{}", f64_to_string(js_round(expiry_ms / DEBOUNCE_BUCKET_MS)))
}

/// Replace Codex credit blocks with live rows. Native matching is exact and
/// first-row-wins; an absent or failed row makes availability unknown.
pub fn overlay_live_reset_credits(
    reports: Option<&[UsageReport]>,
    statuses: &[ResetCreditAccountStatus],
) -> Option<Vec<UsageReport>> {
    reports.map(|reports| {
        reports
            .iter()
            .map(|report| {
                let mut report = report.clone();
                if report.provider != "openai-codex" {
                    return report;
                }
                let metadata = report.metadata.as_ref();
                let status = statuses.iter().find(|status| {
                    status
                        .account_id
                        .as_deref()
                        .filter(|account| !account.is_empty())
                        .is_some_and(|account| Some(account) == metadata_string(metadata, "accountId"))
                        || status
                            .email
                            .as_deref()
                            .filter(|email| !email.is_empty())
                            .is_some_and(|email| Some(email) == metadata_string(metadata, "email"))
                });
                report.reset_credits =
                    status.filter(|status| status.error.as_deref().is_none_or(str::is_empty)).map(|status| {
                        UsageResetCredits {
                            available_count: status.available_count,
                            credits: Some(
                                status
                                    .credits
                                    .iter()
                                    .filter(|credit| credit.status.as_deref().unwrap_or("available") == "available")
                                    .map(|credit| UsageResetCreditDetail {
                                        granted_at: credit.granted_at.clone(),
                                        expires_at: credit.expires_at.clone(),
                                        status: credit.status.clone(),
                                        ..Default::default()
                                    })
                                    .collect(),
                            ),
                            ..Default::default()
                        }
                    });
                report
            })
            .collect()
    })
}

/// Native terminal classification only. A thrown/unknown consume outcome is
/// nonterminal here, but the Host receipt layer must forbid an unsafe replay.
pub fn is_terminal_redeem_outcome(code: &str) -> bool {
    matches!(code, "reset" | "already_redeemed" | "no_credit")
}
