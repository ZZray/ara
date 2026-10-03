//! Grouped native case families from fixed OMP 596f2da,
//! packages/coding-agent/test/codex-auto-reset.test.ts:1-787.
//! These fixtures exercise the pure planner; they never spend a credit and
//! do not stand for controller, transport, consent UI or real-model acceptance.
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

use std::collections::{HashMap, HashSet};

use ara_cli::auth_storage::{ResetCreditAccountStatus, ResetCreditTarget};
use ara_cli::auth_storage_policy::{
    UsageAmount, UsageLimit, UsageReport, UsageResetCreditDetail, UsageResetCredits, UsageScope, UsageStatus,
    UsageUnit, UsageWindow,
};
use ara_cli::auth_storage_state::OAuthAccountSummary;
use ara_cli::codex_auto_reset::*;
use ara_cli::codex_usage::CodexResetCredit;
use serde_json::{Map, Value, json};

const NOW: f64 = 1_700_000_040_000.0;
const HOUR: f64 = 3_600_000.0;
const DAY: f64 = 24.0 * HOUR;
const ACCOUNT: &str = "acct-123";
const EMAIL: &str = "user@example.com";

#[derive(Clone)]
struct AccountOpts {
    account_id: Option<String>,
    email: Option<String>,
    weekly_used: Option<f64>,
    weekly_reset_in_ms: Option<f64>,
    primary_used: f64,
    primary_reset_in_ms: Option<f64>,
    credits: Option<f64>,
    credit_expiries: Vec<Option<f64>>,
    credit_statuses: Vec<Option<String>>,
    limit_reached: bool,
    fetched_ago_ms: f64,
}

impl Default for AccountOpts {
    fn default() -> Self {
        Self {
            account_id: Some(ACCOUNT.into()),
            email: Some(EMAIL.into()),
            weekly_used: Some(1.0),
            weekly_reset_in_ms: Some(3.0 * DAY),
            primary_used: 0.5,
            primary_reset_in_ms: Some(2.0 * HOUR),
            credits: Some(1.0),
            credit_expiries: Vec::new(),
            credit_statuses: Vec::new(),
            limit_reached: true,
            fetched_ago_ms: 0.0,
        }
    }
}

fn report(opts: AccountOpts) -> UsageReport {
    let make_limit = |id: &str, label: &str, window_id: &str, used: f64, reset: Option<f64>| UsageLimit {
        id: id.into(),
        label: label.into(),
        scope: UsageScope {
            provider: "openai-codex".into(),
            account_id: opts.account_id.clone(),
            ..Default::default()
        },
        window: Some(UsageWindow {
            id: window_id.into(),
            label: label.into(),
            resets_at: reset.map(|reset| NOW + reset),
            ..Default::default()
        }),
        amount: UsageAmount { used_fraction: Some(used), unit: UsageUnit::Percent, ..Default::default() },
        ..Default::default()
    };
    let mut limits =
        vec![make_limit("openai-codex:primary", "5 Hour", "5h", opts.primary_used, opts.primary_reset_in_ms)];
    if let Some(used) = opts.weekly_used {
        limits.push(make_limit("openai-codex:secondary", "Weekly", "7d", used, opts.weekly_reset_in_ms));
    }
    let mut metadata = Map::new();
    if let Some(account_id) = &opts.account_id {
        metadata.insert("accountId".into(), json!(account_id));
    }
    if let Some(email) = &opts.email {
        metadata.insert("email".into(), json!(email));
    }
    metadata.insert("limitReached".into(), json!(opts.limit_reached));
    UsageReport {
        provider: "openai-codex".into(),
        fetched_at: NOW - opts.fetched_ago_ms,
        limits,
        metadata: Some(metadata),
        reset_credits: opts.credits.map(|available_count| UsageResetCredits {
            available_count,
            credits: if opts.credit_expiries.is_empty() {
                None
            } else {
                Some(
                    opts.credit_expiries
                        .iter()
                        .enumerate()
                        .map(|(index, offset)| UsageResetCreditDetail {
                            status: opts.credit_statuses.get(index).cloned().unwrap_or(Some("available".into())),
                            expires_at: offset.map(|offset| {
                                chrono::DateTime::from_timestamp_millis((NOW + offset) as i64).unwrap().to_rfc3339()
                            }),
                            ..Default::default()
                        })
                        .collect(),
                )
            },
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn identity() -> OAuthAccountSummary {
    OAuthAccountSummary {
        position: 0,
        credential_id: 7,
        account_id: Some(ACCOUNT.into()),
        email: Some(EMAIL.into()),
        project_id: None,
        enterprise_url: None,
        org_id: None,
        org_name: None,
        active: true,
    }
}

struct Fixture {
    trigger: CodexResetTrigger,
    provider: String,
    model_id: String,
    settings: CodexResetSettings,
    identity: Option<OAuthAccountSummary>,
    reports: Option<Vec<UsageReport>>,
    attempted: HashSet<String>,
    deferred: HashMap<String, f64>,
    last_attempt: HashMap<String, f64>,
    live_unblock: Option<f64>,
}

impl Default for Fixture {
    fn default() -> Self {
        Self {
            trigger: CodexResetTrigger::Blocked,
            provider: "openai-codex".into(),
            model_id: "gpt-5.3-codex".into(),
            settings: CodexResetSettings::default(),
            identity: Some(identity()),
            reports: Some(vec![report(AccountOpts::default())]),
            attempted: HashSet::new(),
            deferred: HashMap::new(),
            last_attempt: HashMap::new(),
            live_unblock: None,
        }
    }
}

impl Fixture {
    fn with_reports(reports: Vec<UsageReport>) -> Self {
        Self { reports: Some(reports), ..Self::default() }
    }
    fn plan(&self) -> CodexResetPlan {
        plan_codex_resets(&CodexResetPlanInput {
            now_ms: NOW,
            trigger: self.trigger,
            provider: &self.provider,
            model_id: &self.model_id,
            settings: self.settings,
            identity: self.identity.as_ref(),
            reports: self.reports.as_deref(),
            attempted_keys: &self.attempted,
            deferred_until_by_key: &self.deferred,
            last_attempt_at_by_account: &self.last_attempt,
            active_block_unblock_at_ms: self.live_unblock,
        })
    }
}

fn skipped(plan: &CodexResetPlan, account: &str, rule: &str, reason: &str) {
    assert!(
        plan.skipped.iter().any(|skip| skip.account_key == account && skip.rule == rule && skip.reason == reason),
        "expected ({account}, {rule}, {reason}); got {plan:?}"
    );
}

#[test]
fn blocked_windows_native_thresholds_status_trap_and_reset_time_family() {
    let default_plan = Fixture::default().plan();
    assert_eq!(
        default_plan.actions,
        vec![CodexResetAction {
            reason: "blocked-account".into(),
            target: ResetCreditTarget {
                account_id: Some(ACCOUNT.into()),
                email: Some(EMAIL.into()),
                ..Default::default()
            },
            account_key: ACCOUNT.into(),
            attempt_key: blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY),
            label: EMAIL.into(),
            available_count: Some(1.0),
            weekly_used_fraction: Some(1.0),
            remaining_ms: Some(3.0 * DAY),
            blocked_windows: Some(vec!["weekly".into()]),
            active: true,
            ..Default::default()
        }]
    );
    for (opts, windows, remaining) in [
        (AccountOpts { primary_used: 1.0, weekly_used: Some(0.8), ..Default::default() }, vec!["5h"], 2.0 * HOUR),
        (AccountOpts { primary_used: 1.0, ..Default::default() }, vec!["5h", "weekly"], 3.0 * DAY),
        (
            AccountOpts { weekly_used: Some(WINDOW_EXHAUSTED_MIN_FRACTION), ..Default::default() },
            vec!["weekly"],
            3.0 * DAY,
        ),
        (AccountOpts { weekly_reset_in_ms: Some(HOUR), ..Default::default() }, vec!["weekly"], HOUR),
        (
            AccountOpts {
                primary_used: 1.0,
                weekly_used: None,
                primary_reset_in_ms: Some(MAX_PLAUSIBLE_PRIMARY_REMAINING_MS),
                ..Default::default()
            },
            vec!["5h"],
            MAX_PLAUSIBLE_PRIMARY_REMAINING_MS,
        ),
        (
            AccountOpts { weekly_reset_in_ms: Some(MAX_PLAUSIBLE_WEEKLY_REMAINING_MS), ..Default::default() },
            vec!["weekly"],
            MAX_PLAUSIBLE_WEEKLY_REMAINING_MS,
        ),
    ] {
        let plan = Fixture::with_reports(vec![report(opts)]).plan();
        assert_eq!(plan.actions.len(), 1, "{plan:?}");
        assert_eq!(plan.actions[0].blocked_windows.as_ref().unwrap(), &windows);
        assert_eq!(plan.actions[0].remaining_ms, Some(remaining));
        assert_eq!(plan.actions[0].attempt_key, blocked_attempt_key(ACCOUNT, NOW + remaining));
    }
    for (opts, reason) in [
        (AccountOpts { weekly_used: Some(0.4), primary_used: 0.9, ..Default::default() }, "no-exhausted-window"),
        (AccountOpts { weekly_used: Some(0.995), ..Default::default() }, "no-exhausted-window"),
        (AccountOpts { weekly_used: None, ..Default::default() }, "no-exhausted-window"),
        (AccountOpts { limit_reached: false, ..Default::default() }, "not-limit-reached"),
        (AccountOpts { weekly_reset_in_ms: Some(2.0 * 60_000.0), ..Default::default() }, "reset-too-soon"),
        (
            AccountOpts {
                primary_used: 1.0,
                weekly_used: Some(0.8),
                primary_reset_in_ms: Some(30.0 * 60_000.0),
                ..Default::default()
            },
            "reset-too-soon",
        ),
        (AccountOpts { weekly_reset_in_ms: Some(-60_000.0), ..Default::default() }, "reset-too-soon"),
        (AccountOpts { weekly_reset_in_ms: Some(8.0 * DAY), ..Default::default() }, "reset-implausible"),
        (
            AccountOpts {
                primary_used: 1.0,
                weekly_used: Some(0.8),
                primary_reset_in_ms: Some(8.0 * HOUR),
                ..Default::default()
            },
            "reset-implausible",
        ),
        (AccountOpts { primary_used: 1.0, weekly_reset_in_ms: None, ..Default::default() }, "no-reset-time"),
        (AccountOpts { primary_used: 1.0, primary_reset_in_ms: None, ..Default::default() }, "no-reset-time"),
    ] {
        let plan = Fixture::with_reports(vec![report(opts)]).plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "blocked-account", reason);
    }
    let mut misleading_status = report(AccountOpts { primary_used: 0.9, weekly_used: Some(0.2), ..Default::default() });
    for limit in &mut misleading_status.limits {
        limit.status = Some(UsageStatus::Exhausted);
    }
    skipped(&Fixture::with_reports(vec![misleading_status]).plan(), ACCOUNT, "blocked-account", "no-exhausted-window");
}

#[test]
fn inventory_reserve_dedupe_jitter_deferral_and_cooldown_family() {
    for (credits, reason) in [(Some(0.0), "no-credits"), (None, "credits-unknown")] {
        let plan = Fixture::with_reports(vec![report(AccountOpts { credits, ..Default::default() })]).plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "account", reason);
    }
    let mut fixture = Fixture::default();
    fixture.settings.keep_credits = 1.0;
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "reserve");
    fixture.reports = Some(vec![report(AccountOpts { credits: Some(2.0), ..Default::default() })]);
    assert_eq!(fixture.plan().actions[0].available_count, Some(2.0));
    fixture.settings.keep_credits = 1.9;
    assert_eq!(fixture.plan().actions.len(), 1, "native reserve truncates fractions");
    fixture.settings.keep_credits = -2.0;
    assert_eq!(fixture.plan().actions.len(), 1, "negative reserve normalizes to zero");
    fixture = Fixture::default();
    fixture.attempted.insert(blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY));
    for jitter in [0.0, 20_000.0] {
        fixture.reports =
            Some(vec![report(AccountOpts { weekly_reset_in_ms: Some(3.0 * DAY + jitter), ..Default::default() })]);
        let plan = fixture.plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "blocked-account", "already-attempted");
    }
    fixture.reports =
        Some(vec![report(AccountOpts { weekly_reset_in_ms: Some(3.0 * DAY + 40_000.0), ..Default::default() })]);
    fixture.last_attempt.insert(ACCOUNT.into(), NOW - 10_000.0);
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "cooldown");
    fixture.last_attempt.insert(ACCOUNT.into(), NOW - ATTEMPT_COOLDOWN_MS);
    assert_eq!(fixture.plan().actions.len(), 1);
    fixture = Fixture::default();
    let key = blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY);
    fixture.deferred.insert(key.clone(), NOW + 60_000.0);
    let plan = fixture.plan();
    assert!(plan.actions.is_empty());
    skipped(&plan, ACCOUNT, "blocked-account", "deferred");
    for until in [NOW - 1.0, NOW] {
        fixture.deferred.insert(key.clone(), until);
        assert_eq!(fixture.plan().actions.len(), 1);
    }
}

#[test]
fn live_429_healthy_snapshots_exact_window_preference_and_synthesis_family() {
    let healthy =
        report(AccountOpts { weekly_used: Some(0.5), primary_used: 0.6, limit_reached: false, ..Default::default() });
    let mut fixture = Fixture::with_reports(vec![healthy]);
    for (remaining, window) in [(3.0 * DAY, "weekly"), (2.0 * HOUR, "5h")] {
        fixture.live_unblock = Some(NOW + remaining);
        let plan = fixture.plan();
        assert_eq!(plan.actions.len(), 1);
        assert!(plan.actions[0].active);
        assert_eq!(plan.actions[0].remaining_ms, Some(remaining));
        assert_eq!(plan.actions[0].blocked_windows, Some(vec![window.into()]));
    }
    for (remaining, reason) in [(30.0 * 60_000.0, "reset-too-soon"), (9.0 * DAY, "reset-implausible")] {
        fixture.live_unblock = Some(NOW + remaining);
        let plan = fixture.plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "blocked-account", reason);
    }
    fixture = Fixture::default();
    fixture.live_unblock = Some(NOW + 2.0 * HOUR);
    assert_eq!(
        fixture.plan().actions[0].remaining_ms,
        Some(3.0 * DAY),
        "exact exhausted report windows outrank live hint"
    );
    fixture.reports = Some(vec![report(AccountOpts {
        account_id: Some("acct-sib".into()),
        email: Some("sib@example.com".into()),
        weekly_used: Some(0.5),
        limit_reached: false,
        ..Default::default()
    })]);
    fixture.live_unblock = Some(NOW + 3.0 * DAY);
    let plan = fixture.plan();
    skipped(&plan, "acct-sib", "blocked-account", "not-limit-reached");
    assert_eq!(plan.actions[0].account_key, ACCOUNT);
    assert_eq!(plan.actions[0].available_count, None);
    fixture.reports = Some(vec![report(AccountOpts { fetched_ago_ms: 11.0 * 60_000.0, ..Default::default() })]);
    let expected = CodexResetAction {
        reason: "blocked-account".into(),
        target: ResetCreditTarget { account_id: Some(ACCOUNT.into()), email: Some(EMAIL.into()), ..Default::default() },
        account_key: ACCOUNT.into(),
        attempt_key: blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY),
        label: EMAIL.into(),
        remaining_ms: Some(3.0 * DAY),
        blocked_windows: Some(vec!["weekly".into()]),
        active: true,
        ..Default::default()
    };
    assert_eq!(fixture.plan().actions, vec![expected.clone()]);
    fixture.reports = None;
    assert_eq!(fixture.plan().actions, vec![expected.clone()]);
    fixture.reports = Some(vec![report(AccountOpts { credits: None, ..Default::default() })]);
    assert_eq!(fixture.plan().actions, vec![expected]);
    fixture.reports = None;
    fixture.settings.keep_credits = 1.0;
    let plan = fixture.plan();
    assert!(plan.actions.is_empty());
    skipped(&plan, ACCOUNT, "blocked-account", "credits-unknown");
    fixture.settings.keep_credits = 0.0;
    fixture.reports =
        Some(vec![report(AccountOpts { credits: Some(0.0), limit_reached: false, ..Default::default() })]);
    assert!(fixture.plan().actions.is_empty(), "fresh zero credits suppresses synthetic spending");
    fixture.reports = None;
    fixture.trigger = CodexResetTrigger::Sweep;
    assert!(fixture.plan().actions.is_empty());
    fixture.trigger = CodexResetTrigger::Blocked;
    fixture.identity = None;
    let plan = fixture.plan();
    assert!(plan.actions.is_empty());
    skipped(&plan, "*", "blocked-account", "no-identity");
    fixture.identity = Some(identity());
    let key = blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY);
    fixture.attempted.insert(key.clone());
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "already-attempted");
    fixture.attempted.clear();
    fixture.deferred.insert(key, NOW + 1.0);
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "deferred");
    fixture.deferred.clear();
    fixture.last_attempt.insert(ACCOUNT.into(), NOW - 1.0);
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "cooldown");
}

#[test]
fn pool_wide_rules_identity_freshness_and_restore_selection_family() {
    let mut fixture = Fixture { model_id: "gpt-5.3-codex-spark".into(), ..Default::default() };
    let plan = fixture.plan();
    assert!(plan.actions.is_empty());
    skipped(&plan, "*", "blocked-account", "spark-model");
    fixture = Fixture::default();
    fixture.provider = "anthropic".into();
    skipped(&fixture.plan(), "*", "blocked-account", "wrong-provider");
    fixture = Fixture::default();
    fixture.trigger = CodexResetTrigger::Sweep;
    assert!(fixture.plan().actions.is_empty());
    fixture = Fixture::default();
    fixture.settings.auto_redeem = CodexAutoRedeemMode::No;
    assert_eq!(
        fixture.plan(),
        CodexResetPlan {
            actions: Vec::new(),
            skipped: vec![CodexResetSkip {
                account_key: "*".into(),
                rule: "account".into(),
                reason: "disabled".into()
            }]
        }
    );
    fixture =
        Fixture::with_reports(vec![report(AccountOpts { fetched_ago_ms: 11.0 * 60_000.0, ..Default::default() })]);
    skipped(&fixture.plan(), ACCOUNT, "account", "stale-report");
    fixture.reports = Some(vec![report(AccountOpts { fetched_ago_ms: REPORT_FRESHNESS_MS, ..Default::default() })]);
    assert_eq!(fixture.plan().actions.len(), 1);
    fixture.identity = None;
    assert!(!fixture.plan().actions[0].active);
    let sibling = |account: &str, credits: f64, expiry: f64, wait: f64| {
        report(AccountOpts {
            account_id: Some(account.into()),
            email: Some(format!("{account}@example.com")),
            credits: Some(credits),
            credit_expiries: vec![Some(expiry)],
            weekly_reset_in_ms: Some(wait),
            ..Default::default()
        })
    };
    fixture = Fixture::with_reports(vec![
        report(AccountOpts { credits: Some(0.0), ..Default::default() }),
        sibling("acct-sib", 2.0, 2.0 * DAY, 3.0 * DAY),
    ]);
    assert_eq!(fixture.plan().actions[0].account_key, "acct-sib");
    assert!(!fixture.plan().actions[0].active);
    fixture.reports = Some(vec![report(AccountOpts::default()), sibling("acct-sib", 3.0, 2.0 * DAY, 3.0 * DAY)]);
    assert_eq!(fixture.plan().actions[0].account_key, ACCOUNT, "active overrides expiry and credit depth");
    fixture.identity = None;
    for (a, b, selected) in [
        (sibling("a", 1.0, 5.0 * DAY, 3.0 * DAY), sibling("b", 1.0, 2.0 * DAY, 3.0 * DAY), "b"),
        (sibling("a", 1.0, 5.0 * DAY, 3.0 * DAY), sibling("b", 3.0, 5.0 * DAY, 3.0 * DAY), "b"),
        (sibling("a", 1.0, 5.0 * DAY, 2.0 * DAY), sibling("b", 1.0, 5.0 * DAY, 3.0 * DAY), "b"),
        (sibling("a", 1.0, 5.0 * DAY, 3.0 * DAY), sibling("b", 1.0, 5.0 * DAY, 3.0 * DAY), "a"),
    ] {
        fixture.reports = Some(vec![a, b]);
        let plan = fixture.plan();
        assert_eq!(plan.actions.iter().filter(|action| action.reason == "blocked-account").count(), 1);
        assert_eq!(plan.actions[0].account_key, selected);
    }
    let mut foreign = report(AccountOpts::default());
    foreign.provider = "anthropic".into();
    fixture.reports = Some(vec![foreign]);
    assert!(fixture.plan().actions.is_empty());
    fixture.reports = None;
    assert!(fixture.plan().actions.is_empty());
    fixture.reports = Some(vec![report(AccountOpts { account_id: None, email: None, ..Default::default() })]);
    skipped(&fixture.plan(), "*", "account", "no-identity");
    fixture.reports = Some(vec![report(AccountOpts {
        account_id: Some("\u{feff} ACCT-123 \u{feff}".into()),
        email: Some("USER@example.com ".into()),
        ..Default::default()
    })]);
    fixture.identity = Some(identity());
    let action = fixture.plan().actions.remove(0);
    assert_eq!(action.account_key, ACCOUNT);
    assert_eq!(action.target.account_id.as_deref(), Some("\u{feff} ACCT-123 \u{feff}"));
    assert_eq!(action.label, "USER@example.com ");
    assert!(action.active);
    fixture.reports = Some(vec![report(AccountOpts {
        account_id: None,
        email: Some(" ONLY@EXAMPLE.COM ".into()),
        ..Default::default()
    })]);
    assert_eq!(fixture.plan().actions[0].account_key, "only@example.com");
}

fn salvage_fixture(opts: AccountOpts) -> Fixture {
    Fixture { trigger: CodexResetTrigger::Sweep, reports: Some(vec![report(opts)]), ..Fixture::default() }
}

#[test]
fn expiring_credit_horizon_dates_usage_threshold_and_reserve_family() {
    let base = AccountOpts {
        weekly_used: Some(0.8),
        limit_reached: false,
        credit_expiries: vec![Some(2.0 * HOUR)],
        ..Default::default()
    };
    let plan = salvage_fixture(base.clone()).plan();
    assert_eq!(
        plan.actions,
        vec![CodexResetAction {
            reason: "expiring-credit".into(),
            target: ResetCreditTarget {
                account_id: Some(ACCOUNT.into()),
                email: Some(EMAIL.into()),
                ..Default::default()
            },
            account_key: ACCOUNT.into(),
            attempt_key: salvage_attempt_key(ACCOUNT, NOW + 2.0 * HOUR),
            label: EMAIL.into(),
            available_count: Some(1.0),
            weekly_used_fraction: Some(0.8),
            salvage_window: Some("weekly".into()),
            salvage_used_fraction: Some(0.8),
            expires_in_ms: Some(2.0 * HOUR),
            active: true,
            ..Default::default()
        }]
    );
    let at_horizon = salvage_fixture(AccountOpts { credit_expiries: vec![Some(12.0 * HOUR)], ..base.clone() });
    assert_eq!(at_horizon.plan().actions.len(), 1);
    for opts in [
        AccountOpts { credit_expiries: vec![Some(13.0 * HOUR)], ..base.clone() },
        AccountOpts {
            credit_expiries: vec![Some(-HOUR), Some(2.0 * HOUR)],
            credit_statuses: vec![Some("available".into()), Some("redeemed".into())],
            ..base.clone()
        },
        AccountOpts { credit_expiries: vec![None], ..base.clone() },
    ] {
        let plan = salvage_fixture(opts).plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "expiring-credit", "no-expiring-credit");
    }
    for (primary, weekly, expected_window, expected_used) in [
        (1.0, Some(0.2), "5h", 1.0),
        (0.0, Some(SALVAGE_MIN_USED_FRACTION), "weekly", SALVAGE_MIN_USED_FRACTION),
        (0.9, None, "5h", 0.9),
        (0.5, Some(0.5), "5h", 0.5),
    ] {
        let plan = salvage_fixture(AccountOpts { primary_used: primary, weekly_used: weekly, ..base.clone() }).plan();
        assert_eq!(plan.actions[0].salvage_window.as_deref(), Some(expected_window));
        assert_eq!(plan.actions[0].salvage_used_fraction, Some(expected_used));
    }
    for (primary, weekly) in [(0.1, Some(0.1)), (0.0, None)] {
        let plan = salvage_fixture(AccountOpts { primary_used: primary, weekly_used: weekly, ..base.clone() }).plan();
        assert!(plan.actions.is_empty());
        skipped(&plan, ACCOUNT, "expiring-credit", "window-mostly-free");
    }
    let mut fixture = salvage_fixture(base);
    fixture.settings.keep_credits = 5.0;
    assert_eq!(fixture.plan().actions.len(), 1, "expiring credits bypass reserve");
    fixture.settings.salvage_horizon_hours = 0.0;
    assert!(fixture.plan().actions.is_empty());
    fixture.settings.salvage_horizon_hours = -1.0;
    assert!(fixture.plan().actions.is_empty());
    fixture.settings.salvage_horizon_hours = f64::NAN;
    assert!(fixture.plan().actions.is_empty());
    fixture.settings.salvage_horizon_hours = 12.0;
    for date in ["not a date", "", "2020-01-01"] {
        fixture.reports.as_mut().unwrap()[0].reset_credits.as_mut().unwrap().credits.as_mut().unwrap()[0].expires_at =
            Some(date.into());
        assert!(fixture.plan().actions.is_empty(), "{date}");
    }
    for date in ["2023-11-15T00:14:00Z", "Wed, 15 Nov 2023 00:14:00 +0000", "2023-11-15"] {
        fixture.reports.as_mut().unwrap()[0].reset_credits.as_mut().unwrap().credits.as_mut().unwrap()[0].expires_at =
            Some(date.into());
        assert_eq!(fixture.plan().actions.len(), 1, "{date}");
    }
}

#[test]
fn salvage_dedupe_deferral_cooldown_multi_account_order_and_restore_exclusion_family() {
    let base = AccountOpts { weekly_used: Some(0.8), credit_expiries: vec![Some(2.0 * HOUR)], ..Default::default() };
    let key = salvage_attempt_key(ACCOUNT, NOW + 2.0 * HOUR);
    let mut fixture = salvage_fixture(base.clone());
    fixture.attempted.insert(key.clone());
    let plan = fixture.plan();
    assert!(plan.actions.is_empty());
    skipped(&plan, ACCOUNT, "expiring-credit", "already-attempted");
    fixture.attempted.clear();
    fixture.deferred.insert(key.clone(), NOW + 60_000.0);
    skipped(&fixture.plan(), ACCOUNT, "expiring-credit", "deferred");
    for until in [NOW - 1.0, NOW] {
        fixture.deferred.insert(key.clone(), until);
        assert_eq!(fixture.plan().actions.len(), 1);
    }
    fixture.last_attempt.insert(ACCOUNT.into(), NOW - 10_000.0);
    skipped(&fixture.plan(), ACCOUNT, "expiring-credit", "cooldown");
    fixture.last_attempt.clear();
    fixture.identity = None;
    fixture.reports = Some(vec![
        report(AccountOpts {
            account_id: Some("acct-a".into()),
            email: Some("a@example.com".into()),
            weekly_used: Some(0.9),
            credit_expiries: vec![Some(5.0 * HOUR)],
            ..Default::default()
        }),
        report(AccountOpts {
            account_id: Some("acct-b".into()),
            email: Some("b@example.com".into()),
            weekly_used: Some(0.7),
            credit_expiries: vec![Some(2.0 * HOUR)],
            ..Default::default()
        }),
    ]);
    assert_eq!(
        fixture.plan().actions.iter().map(|action| action.account_key.as_str()).collect::<Vec<_>>(),
        vec!["acct-b", "acct-a"]
    );
    fixture = Fixture::with_reports(vec![
        report(AccountOpts { credit_expiries: vec![Some(2.0 * HOUR)], ..Default::default() }),
        report(AccountOpts {
            account_id: Some("acct-sib".into()),
            email: Some("sib@example.com".into()),
            weekly_used: Some(0.8),
            limit_reached: false,
            credit_expiries: vec![Some(3.0 * HOUR)],
            ..Default::default()
        }),
    ]);
    let plan = fixture.plan();
    assert_eq!(
        plan.actions.iter().map(|action| (action.reason.as_str(), action.account_key.as_str())).collect::<Vec<_>>(),
        vec![("blocked-account", ACCOUNT), ("expiring-credit", "acct-sib")]
    );
    fixture.provider = "anthropic".into();
    assert!(
        fixture.plan().actions.iter().all(|action| action.reason == "expiring-credit"),
        "provider gate applies to restore only"
    );
    fixture.provider = "openai-codex".into();
    fixture.model_id = "gpt-5.3-codex-spark".into();
    assert!(
        fixture.plan().actions.iter().all(|action| action.reason == "expiring-credit"),
        "Spark gate applies to restore only"
    );
}

#[test]
fn live_overlay_is_exact_first_match_replaces_credit_block_and_preserves_other_fields() {
    assert_eq!(overlay_live_reset_credits(None, &[]), None);
    let mut source = report(AccountOpts::default());
    source.notes = Some(vec!["retained report".into()]);
    source.reset_credits.as_mut().unwrap().unknown_fields.insert("staleUnknown".into(), json!(1));
    let available = CodexResetCredit {
        id: "secret-id-not-in-report".into(),
        reset_type: None,
        status: None,
        granted_at: Some("2023-11-01".into()),
        expires_at: Some("2023-11-15".into()),
        redeem_started_at: None,
        redeemed_at: None,
        title: None,
        description: None,
    };
    let mut spent = available.clone();
    spent.id = "spent".into();
    spent.status = Some("redeemed".into());
    let status = ResetCreditAccountStatus {
        credential_id: 7,
        account_id: Some(ACCOUNT.into()),
        email: Some(EMAIL.into()),
        active: true,
        available_count: 2.0,
        credits: vec![available.clone(), spent],
        error: None,
    };
    let mut duplicate = status.clone();
    duplicate.available_count = 99.0;
    let mut foreign = source.clone();
    foreign.provider = "anthropic".into();
    let reports = vec![source.clone(), foreign.clone()];
    let overlay = overlay_live_reset_credits(Some(&reports), &[status.clone(), duplicate]).unwrap();
    assert_eq!(reports[0], source, "input is never mutated");
    assert_eq!(overlay[1], foreign);
    assert_eq!(overlay[0].notes, source.notes);
    assert_eq!(
        overlay[0].reset_credits.as_ref().unwrap(),
        &UsageResetCredits {
            available_count: 2.0,
            credits: Some(vec![UsageResetCreditDetail {
                granted_at: available.granted_at.clone(),
                expires_at: available.expires_at.clone(),
                status: None,
                ..Default::default()
            }]),
            ..Default::default()
        }
    );
    assert!(!serde_json::to_string(&overlay).unwrap().contains("secret-id-not-in-report"));
    for statuses in [
        vec![],
        vec![ResetCreditAccountStatus { error: Some("lookup failed".into()), ..status.clone() }],
        vec![ResetCreditAccountStatus {
            account_id: Some(ACCOUNT.to_uppercase()),
            email: Some(EMAIL.to_uppercase()),
            ..status.clone()
        }],
    ] {
        assert!(overlay_live_reset_credits(Some(&reports), &statuses).unwrap()[0].reset_credits.is_none());
    }
    assert_eq!(
        overlay_live_reset_credits(
            Some(&reports),
            &[ResetCreditAccountStatus { error: Some(String::new()), ..status.clone() }]
        )
        .unwrap()[0]
            .reset_credits
            .as_ref()
            .unwrap()
            .available_count,
        2.0,
        "native error gate uses string truthiness"
    );
    assert_eq!(
        overlay_live_reset_credits(Some(&reports), &[ResetCreditAccountStatus { account_id: None, ..status }]).unwrap()
            [0]
        .reset_credits
        .as_ref()
        .unwrap()
        .available_count,
        2.0,
        "email alone can match a live row"
    );
}

#[test]
fn native_identity_org_gate_scope_fallbacks_and_numeric_policy_plumbing_family() {
    let mut active = identity();
    let mut sample = report(AccountOpts::default());
    assert!(report_matches_active_account(&sample, Some(&active)));
    sample.limits.clear();
    assert!(!report_matches_active_account(&sample, Some(&active)));
    sample = report(AccountOpts::default());
    active.org_id = Some("org-a".into());
    assert!(!report_matches_active_account(&sample, Some(&active)));
    sample.metadata.as_mut().unwrap().insert("orgId".into(), json!(" ORG-A "));
    assert!(report_matches_active_account(&sample, Some(&active)));
    active.org_id = None;
    assert!(!report_matches_active_account(&sample, Some(&active)));
    active.org_id = Some("org-b".into());
    assert!(!report_matches_active_account(&sample, Some(&active)));
    active.org_id = Some("org-a".into());
    active.account_id = None;
    active.email = None;
    assert!(report_matches_active_account(&sample, Some(&active)), "org-only identity qualifies via exact org");
    active.org_id = None;
    active.project_id = Some("project".into());
    sample.metadata.as_mut().unwrap().remove("orgId");
    sample.limits[0].scope.project_id = Some(" PROJECT ".into());
    assert!(report_matches_active_account(&sample, Some(&active)));
    sample.limits[0].scope.project_id = None;
    sample.metadata.as_mut().unwrap().insert("projectId".into(), json!("project"));
    assert!(report_matches_active_account(&sample, Some(&active)));
    active = identity();
    active.email = None;
    sample.metadata.as_mut().unwrap().remove("accountId");
    assert!(report_matches_active_account(&sample, Some(&active)), "scope accountId fallback");
    for limit in &mut sample.limits {
        limit.scope.account_id = None;
    }
    sample.metadata.as_mut().unwrap().insert("account_id".into(), json!(ACCOUNT));
    assert!(report_matches_active_account(&sample, Some(&active)), "legacy metadata account_id fallback");
    assert_eq!(
        CodexResetSettings::default(),
        CodexResetSettings {
            auto_redeem: CodexAutoRedeemMode::Unset,
            min_blocked_minutes: 60.0,
            keep_credits: 0.0,
            salvage_horizon_hours: 12.0
        }
    );
    for (mode, evaluate, prompt) in [
        (CodexAutoRedeemMode::Unset, true, true),
        (CodexAutoRedeemMode::Yes, true, false),
        (CodexAutoRedeemMode::No, false, false),
    ] {
        assert_eq!(should_evaluate_codex_auto_redeem(mode), evaluate);
        assert_eq!(should_prompt_codex_auto_redeem(mode), prompt);
        assert_eq!(serde_json::from_value::<CodexAutoRedeemMode>(json!(mode.as_str())).unwrap(), mode);
    }
    for code in ["reset", "already_redeemed", "no_credit"] {
        assert!(is_terminal_redeem_outcome(code));
    }
    for code in [
        "nothing_to_reset",
        "http_500",
        "credit_list_failed",
        "future_code",
        "OutcomeUnknown",
        "cancelled",
        "RESET",
        "",
    ] {
        assert!(!is_terminal_redeem_outcome(code));
    }
    for (timestamp, bucket) in [
        (-90_000.0, "-1"),
        (-30_000.0, "0"),
        (30_000.0, "1"),
        (-0.0, "0"),
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
    ] {
        assert_eq!(blocked_attempt_key("account", timestamp), format!("block|account|{bucket}"));
        assert_eq!(salvage_attempt_key("account", timestamp), format!("salvage|account|{bucket}"));
    }
    assert_eq!(blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY), blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY + 20_000.0));
    assert_ne!(blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY), blocked_attempt_key(ACCOUNT, NOW + 3.0 * DAY + 40_000.0));
    let mut fixture = Fixture::default();
    fixture.settings.keep_credits = f64::NAN;
    assert_eq!(fixture.plan().actions.len(), 1, "JS Math.max/trunc propagates NaN; comparisons remain native");
    fixture.settings.keep_credits = f64::INFINITY;
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "reserve");
    fixture.settings.keep_credits = 0.0;
    fixture.settings.min_blocked_minutes = f64::INFINITY;
    skipped(&fixture.plan(), ACCOUNT, "blocked-account", "reset-too-soon");
    fixture.settings.min_blocked_minutes = f64::NAN;
    assert_eq!(fixture.plan().actions.len(), 1);
    assert!(matches!(serde_json::to_value(fixture.plan()).unwrap(), Value::Object(_)));
}
