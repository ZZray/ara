//! Pure usage and credential selection policy from fixed OMP
//! `596f2da7101178214aa27a753529d15e6b7ad91d`:
//! `packages/ai/src/{usage.ts,auth-storage.ts,usage/openai-codex.ts}`.
//!
//! The host supplies reports, active block deadlines, ordering and time. It
//! owns persistence, credential identity, OAuth refresh, fetching, cancellation,
//! session pins and execution of the OAuth passes. Ranking alone is not auth
//! acceptance. Only the Codex default strategy is ported here; other providers
//! can implement the same native strategy contract without product state.
//
// MIT License
//
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::catalog_behavior::{plan_requirement_for, quota_tier_for};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::cmp::Ordering;

// Persistence already owns the history and client aggregate wire shapes.
pub use crate::credential_store::{
    ClientProviderUsage, ClientUsageClient as ClientUsageClientSummary, ClientUsageEntry as ObservedUsageEntry,
    ClientUsageReport, ClientUsageSummary, UsageHistoryEntry, UsageHistoryQuery,
};

pub const DEFAULT_BACKOFF_MS: f64 = 60_000.0;
pub const PRIMARY_WINDOW_HOT_FRACTION: f64 = 0.85;
const USAGE_RANKING_METRIC_EPSILON: f64 = 1e-9;
const HOUR_MS: f64 = 3_600_000.0;

// Native optional properties may be absent, but typed properties may not be
// null. In particular, missing usage is distinct from a reported numeric zero.
// Value's own deserializer still accepts null for the provider-specific raw.
fn deserialize_present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageUnit {
    Percent,
    Tokens,
    Requests,
    Credits,
    Usd,
    Minutes,
    Bytes,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageStatus {
    Ok,
    Warning,
    Exhausted,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageWindow {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub resets_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub reset_label: Option<String>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAmount {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub remaining: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub used_fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub remaining_fraction: Option<f64>,
    pub unit: UsageUnit,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageScope {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub model_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub tier: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub shared: Option<bool>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimit {
    pub id: String,
    pub label: String,
    pub scope: UsageScope,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub window: Option<UsageWindow>,
    pub amount: UsageAmount,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub status: Option<UsageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub notes: Option<Vec<String>>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResetCreditDetail {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub granted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub status: Option<String>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResetCredits {
    pub available_count: f64,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub credits: Option<Vec<UsageResetCreditDetail>>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageReport {
    pub provider: String,
    pub fetched_at: f64,
    pub limits: Vec<UsageLimit>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub reset_credits: Option<UsageResetCredits>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub notes: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub raw: Option<Value>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientUsageIdentity {
    pub install_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub hostname: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub app: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageCredentialType {
    ApiKey,
    Oauth,
}

/// Sensitive endpoint bundle. Deliberately has no Debug/Display implementation.
/// Selection policy does not require this bundle or inspect its secret fields.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageCredential {
    #[serde(rename = "type")]
    pub credential_type: UsageCredentialType,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub expires_at: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub org_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub org_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub enterprise_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub metadata: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub api_endpoint: Option<String>,
    #[serde(default, flatten)]
    pub unknown_fields: Map<String, Value>,
}

/// Explicit fraction > used/limit > percent used > inverted remaining.
/// Overage and unknown values are preserved, rather than clamped or made zero.
pub fn resolve_used_fraction(limit: &UsageLimit) -> Option<f64> {
    let amount = &limit.amount;
    if let Some(fraction) = amount.used_fraction {
        return Some(fraction);
    }
    if let (Some(used), Some(max)) = (amount.used, amount.limit)
        && max > 0.0
    {
        return Some(used / max);
    }
    if amount.unit == UsageUnit::Percent
        && let Some(used) = amount.used
    {
        return Some(used / 100.0);
    }
    amount.remaining_fraction.map(|remaining| js_max(0.0, 1.0 - remaining))
}

/// Native status takes precedence over amounts: an explicitly `ok` 100% window
/// is not exhausted. Numeric fallbacks apply only for absent/unknown status.
pub fn is_usage_limit_exhausted(limit: &UsageLimit) -> bool {
    if let Some(status) = limit.status
        && status != UsageStatus::Unknown
    {
        return status == UsageStatus::Exhausted;
    }
    let amount = &limit.amount;
    amount.used_fraction.is_some_and(|value| value >= 1.0)
        || amount.remaining_fraction.is_some_and(|value| value <= 0.0)
        || matches!((amount.used, amount.limit), (Some(used), Some(max)) if used >= max)
        || amount.remaining.is_some_and(|value| value <= 0.0)
        || (amount.unit == UsageUnit::Percent && amount.used.is_some_and(|value| value >= 100.0))
}

pub fn is_usage_limit_reached(limits: &[&UsageLimit]) -> bool {
    limits.iter().any(|limit| is_usage_limit_exhausted(limit))
}

/// Every exhausted window must reset; the latest known future reset wins.
/// An exhausted window without a future clock contributes no reset candidate.
pub fn usage_reset_at_ms(limits: &[&UsageLimit], now_ms: f64) -> Option<f64> {
    limits
        .iter()
        .filter(|limit| is_usage_limit_exhausted(limit))
        .filter_map(|limit| limit.window.as_ref()?.resets_at)
        .filter(|reset| *reset != 0.0 && *reset > now_ms)
        .reduce(js_max)
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRankingContext {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_present")]
    pub model_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowDefaults {
    pub primary_ms: f64,
    pub secondary_ms: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UsageWindowLimits<'a> {
    pub primary: Option<&'a UsageLimit>,
    pub secondary: Option<&'a UsageLimit>,
}

pub struct HealableBlockScope<'a> {
    pub block_scope: String,
    pub limits: Vec<&'a UsageLimit>,
}

/// Native extension strategy boundary. `None` context means reconciliation;
/// `Some` context without a model still means a request. Those are distinct.
pub trait CredentialRankingStrategy: Send + Sync {
    fn find_window_limits<'a>(
        &self,
        report: &'a UsageReport,
        context: Option<&CredentialRankingContext>,
    ) -> UsageWindowLimits<'a>;

    fn scope_limits<'a>(
        &self,
        report: &'a UsageReport,
        _context: Option<&CredentialRankingContext>,
    ) -> Vec<&'a UsageLimit> {
        report.limits.iter().collect()
    }

    fn scope_limits_for_reserve<'a>(
        &self,
        report: &'a UsageReport,
        context: Option<&CredentialRankingContext>,
    ) -> Vec<&'a UsageLimit> {
        self.scope_limits(report, context)
    }

    fn block_scope(&self, _context: Option<&CredentialRankingContext>) -> Option<String> {
        None
    }

    fn block_scopes(&self, context: Option<&CredentialRankingContext>) -> Vec<String> {
        self.block_scope(context).filter(|scope| !scope.is_empty()).into_iter().collect()
    }

    fn healable_block_scopes<'a>(&self, _report: &'a UsageReport) -> Vec<HealableBlockScope<'a>> {
        Vec::new()
    }

    fn window_defaults(&self) -> WindowDefaults;

    fn has_priority_boost(
        &self,
        _primary: Option<&UsageLimit>,
        _primary_uncapped: bool,
        _context: Option<&CredentialRankingContext>,
    ) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CodexRankingStrategy;

pub static CODEX_RANKING_STRATEGY: CodexRankingStrategy = CodexRankingStrategy;

pub fn default_ranking_strategy(provider: &str) -> Option<&'static dyn CredentialRankingStrategy> {
    (provider == "openai-codex").then_some(&CODEX_RANKING_STRATEGY as &dyn CredentialRankingStrategy)
}

pub fn is_codex_spark_request(context: Option<&CredentialRankingContext>) -> bool {
    context
        .and_then(|context| context.model_id.as_deref())
        .is_some_and(|model| quota_tier_for("openai-codex", model) == Some("spark"))
}

pub fn scope_codex_limits_for_request<'a>(
    report: &'a UsageReport,
    context: Option<&CredentialRankingContext>,
) -> Vec<&'a UsageLimit> {
    let spark = is_codex_spark_request(context);
    report
        .limits
        .iter()
        .filter(|limit| {
            if limit.id == "openai-codex:primary" || limit.id == "openai-codex:secondary" {
                return !spark;
            }
            limit.id.split(':').nth(1) == Some("spark") && spark
        })
        .collect()
}

impl CredentialRankingStrategy for CodexRankingStrategy {
    fn find_window_limits<'a>(
        &self,
        report: &'a UsageReport,
        context: Option<&CredentialRankingContext>,
    ) -> UsageWindowLimits<'a> {
        let limits = self.scope_limits(report, context);
        let find = |key: &str, window_id: &str| {
            let direct = format!("openai-codex:{key}");
            limits
                .iter()
                .copied()
                .find(|limit| limit.id == direct)
                .or_else(|| limits.iter().copied().find(|limit| limit.id.to_lowercase().contains(key)))
                .or_else(|| {
                    limits
                        .iter()
                        .copied()
                        .find(|limit| limit.scope.window_id.as_deref().is_some_and(|id| id.to_lowercase() == window_id))
                })
        };
        UsageWindowLimits { primary: find("primary", "1h"), secondary: find("secondary", "7d") }
    }

    fn scope_limits<'a>(
        &self,
        report: &'a UsageReport,
        context: Option<&CredentialRankingContext>,
    ) -> Vec<&'a UsageLimit> {
        scope_codex_limits_for_request(report, context)
    }

    fn block_scope(&self, context: Option<&CredentialRankingContext>) -> Option<String> {
        Some(if is_codex_spark_request(context) { "spark" } else { "chat" }.into())
    }

    fn block_scopes(&self, context: Option<&CredentialRankingContext>) -> Vec<String> {
        if context.is_none() {
            vec!["chat".into(), "spark".into(), "shared".into()]
        } else {
            vec![self.block_scope(context).unwrap(), "shared".into()]
        }
    }

    fn window_defaults(&self) -> WindowDefaults {
        WindowDefaults { primary_ms: HOUR_MS, secondary_ms: 7.0 * 24.0 * HOUR_MS }
    }

    fn has_priority_boost(
        &self,
        primary: Option<&UsageLimit>,
        primary_uncapped: bool,
        context: Option<&CredentialRankingContext>,
    ) -> bool {
        let Some(primary) = primary else {
            return primary_uncapped && !is_codex_spark_request(context);
        };
        let five_hour = primary.scope.window_id.as_deref().is_some_and(|id| id.to_lowercase() == "5h")
            || primary
                .window
                .as_ref()
                .and_then(|window| window.duration_ms)
                .is_some_and(|duration| duration.is_finite() && (duration - 5.0 * HOUR_MS).abs() <= 60_000.0);
        five_hour && primary.amount.used_fraction.is_some_and(|used| used.is_finite() && used == 0.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanRequirement {
    #[default]
    None,
    Paid,
    Pro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexPlanClass {
    Free,
    Paid,
    Pro,
    Unknown,
}

pub fn resolve_codex_plan_requirement(provider: &str, model_id: Option<&str>) -> PlanRequirement {
    if provider != "openai-codex" {
        return PlanRequirement::None;
    }
    match model_id.and_then(|model| plan_requirement_for(provider, model)) {
        Some("paid") => PlanRequirement::Paid,
        Some("pro") => PlanRequirement::Pro,
        _ => PlanRequirement::None,
    }
}

fn js_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' | '\u{feff}')
}

pub fn usage_plan_type(report: Option<&UsageReport>) -> Option<String> {
    let plan = report?.metadata.as_ref()?.get("planType")?.as_str()?;
    let lower = plan.trim_matches(js_whitespace).to_lowercase();
    let mut normalized = String::new();
    let mut separator = false;
    for character in lower.chars() {
        if character == '-' || js_whitespace(character) {
            if !separator {
                normalized.push('_');
            }
            separator = true;
        } else {
            normalized.push(character);
            separator = false;
        }
    }
    Some(normalized.strip_prefix("chatgpt_").unwrap_or(&normalized).to_owned())
}

pub fn classify_codex_plan(report: Option<&UsageReport>) -> CodexPlanClass {
    let Some(plan) = usage_plan_type(report).filter(|plan| !plan.is_empty()) else {
        return CodexPlanClass::Unknown;
    };
    if plan == "prolite" || plan == "pro_lite" {
        return CodexPlanClass::Paid;
    }
    let tokens: Vec<_> = plan.split('_').collect();
    if tokens.contains(&"pro") {
        return CodexPlanClass::Pro;
    }
    if tokens.iter().any(|token| {
        matches!(
            *token,
            "plus"
                | "business"
                | "team"
                | "enterprise"
                | "edu"
                | "education"
                | "teacher"
                | "teachers"
                | "health"
                | "gov"
                | "government"
        )
    }) {
        return CodexPlanClass::Paid;
    }
    if tokens.iter().any(|token| matches!(*token, "free" | "go")) {
        return CodexPlanClass::Free;
    }
    CodexPlanClass::Unknown
}

/// `None` means unknown eligibility, not ineligible and not eligible.
pub fn codex_plan_eligibility(report: Option<&UsageReport>, requirement: PlanRequirement) -> Option<bool> {
    if requirement == PlanRequirement::None {
        return Some(true);
    }
    let class = classify_codex_plan(report);
    if class == CodexPlanClass::Unknown {
        return None;
    }
    Some(match requirement {
        PlanRequirement::None => true,
        PlanRequirement::Paid => class != CodexPlanClass::Free,
        PlanRequirement::Pro => class == CodexPlanClass::Pro,
    })
}

pub fn codex_plan_priority(report: Option<&UsageReport>, requirement: PlanRequirement) -> u8 {
    match codex_plan_eligibility(report, requirement) {
        Some(true) => 0,
        None => 1,
        Some(false) => 2,
    }
}

pub fn model_account_policy_block_scope(provider: &str, model_id: Option<&str>) -> Option<String> {
    if provider != "openai-codex" && provider != "cursor" {
        return None;
    }
    let bare = model_id?.rsplit('/').next()?.trim_matches(js_whitespace).to_lowercase();
    if bare.is_empty() || bare.contains('\0') {
        return None;
    }
    Some(format!("model-policy:{bare}"))
}

pub fn credential_block_scopes_for_request(
    provider: &str,
    strategy: Option<&dyn CredentialRankingStrategy>,
    context: &CredentialRankingContext,
    block_scope: Option<&str>,
) -> Vec<String> {
    let mut scopes = strategy
        .map(|strategy| strategy.block_scopes(Some(context)))
        .unwrap_or_else(|| block_scope.filter(|scope| !scope.is_empty()).map(str::to_owned).into_iter().collect());
    if let Some(model_scope) = model_account_policy_block_scope(provider, context.model_id.as_deref())
        && !scopes.contains(&model_scope)
    {
        scopes.push(model_scope);
    }
    scopes
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialBlockRouting {
    pub provider_key: String,
    pub ranking_context: CredentialRankingContext,
    pub block_scope: Option<String>,
    pub sibling_block_scopes: Vec<String>,
}

pub fn credential_block_routing(
    provider: &str,
    credential_type: &str,
    strategy: Option<&dyn CredentialRankingStrategy>,
    model_id: Option<&str>,
    block_scope_override: Option<&str>,
) -> CredentialBlockRouting {
    let context = CredentialRankingContext { model_id: model_id.map(str::to_owned) };
    let default_scope = strategy.and_then(|strategy| strategy.block_scope(Some(&context)));
    let block_scope = block_scope_override.map(str::to_owned).or_else(|| default_scope.clone());
    let mut sibling_scopes =
        credential_block_scopes_for_request(provider, strategy, &context, default_scope.as_deref());
    if let Some(override_scope) = block_scope_override.filter(|scope| !scope.is_empty())
        && !sibling_scopes.iter().any(|scope| scope == override_scope)
    {
        sibling_scopes.push(override_scope.into());
    }
    CredentialBlockRouting {
        provider_key: format!("{provider}:{credential_type}"),
        ranking_context: context,
        block_scope,
        sibling_block_scopes: sibling_scopes,
    }
}

pub fn scoped_backoff_key(provider_key: &str, block_scope: Option<&str>) -> String {
    match block_scope.filter(|scope| !scope.is_empty()) {
        Some(scope) => format!("{provider_key}\0{scope}"),
        None => provider_key.into(),
    }
}

// Math.min/max in JS propagate NaN, whereas f64::min/max ignore one NaN.
fn js_min(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.min(right) }
}
fn js_max(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() { f64::NAN } else { left.max(right) }
}

/// Ranking intentionally reads only explicit usedFraction, unlike the UI helper.
pub fn normalize_usage_fraction(limit: Option<&UsageLimit>) -> f64 {
    match limit.and_then(|limit| limit.amount.used_fraction).filter(|used| used.is_finite()) {
        Some(used) => used.clamp(0.0, 1.0),
        None => 0.5,
    }
}

pub fn compute_window_required_drain(limit: Option<&UsageLimit>, now_ms: f64, fallback_duration_ms: f64) -> f64 {
    let headroom = 1.0 - normalize_usage_fraction(limit);
    if headroom <= 0.0 {
        return 0.0;
    }
    let window = limit.and_then(|limit| limit.window.as_ref());
    let reset = window.and_then(|window| window.resets_at).filter(|reset| reset.is_finite());
    let duration = window.and_then(|window| window.duration_ms).unwrap_or(fallback_duration_ms);
    let mut remaining = reset.map(|reset| reset - now_ms).unwrap_or(duration);
    if duration.is_finite() && duration > 0.0 {
        remaining = js_min(remaining, duration);
    }
    headroom / (js_max(remaining, 60_000.0) / HOUR_MS)
}

pub fn compare_usage_ranking_metric(left: f64, right: f64) -> f64 {
    if left == right {
        return 0.0;
    }
    if !left.is_finite() || !right.is_finite() {
        return if left < right { -1.0 } else { 1.0 };
    }
    let delta = left - right;
    let tolerance = USAGE_RANKING_METRIC_EPSILON.max(left.abs().max(right.abs()) * 0.000001);
    if delta.abs() <= tolerance { 0.0 } else { delta }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageRankingScore {
    pub blocked: bool,
    pub blocked_until: Option<f64>,
    pub has_priority_boost: bool,
    pub usage_measured: bool,
    pub plan_priority: u8,
    pub secondary_used: f64,
    pub secondary_required_drain: f64,
    pub primary_used: f64,
    pub primary_required_drain: f64,
    pub order_pos: usize,
}

fn comparison_order(value: f64) -> Ordering {
    if value < 0.0 {
        Ordering::Less
    } else if value > 0.0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

pub fn compare_usage_ranked_candidate_priority(
    left: &UsageRankingScore,
    right: &UsageRankingScore,
    requirement: PlanRequirement,
) -> Ordering {
    if left.blocked != right.blocked {
        return left.blocked.cmp(&right.blocked);
    }
    if left.blocked && right.blocked {
        let left_until = left.blocked_until.unwrap_or(f64::INFINITY);
        let right_until = right.blocked_until.unwrap_or(f64::INFINITY);
        return if left_until != right_until { comparison_order(left_until - right_until) } else { Ordering::Equal };
    }
    if requirement != PlanRequirement::None && left.plan_priority != right.plan_priority {
        return left.plan_priority.cmp(&right.plan_priority);
    }
    if left.has_priority_boost != right.has_priority_boost {
        return right.has_priority_boost.cmp(&left.has_priority_boost);
    }
    let left_hot = left.primary_used >= PRIMARY_WINDOW_HOT_FRACTION;
    let right_hot = right.primary_used >= PRIMARY_WINDOW_HOT_FRACTION;
    if left_hot != right_hot {
        return left_hot.cmp(&right_hot);
    }
    if left.usage_measured != right.usage_measured {
        return right.usage_measured.cmp(&left.usage_measured);
    }
    for (left_metric, right_metric) in [
        (right.secondary_required_drain, left.secondary_required_drain),
        (left.secondary_used, right.secondary_used),
        (right.primary_required_drain, left.primary_required_drain),
        (left.primary_used, right.primary_used),
    ] {
        let metric = comparison_order(compare_usage_ranking_metric(left_metric, right_metric));
        if metric != Ordering::Equal {
            return metric;
        }
    }
    Ordering::Equal
}

pub fn compare_usage_ranked_candidates(
    left: &UsageRankingScore,
    right: &UsageRankingScore,
    requirement: PlanRequirement,
) -> Ordering {
    compare_usage_ranked_candidate_priority(left, right, requirement).then_with(|| left.order_pos.cmp(&right.order_pos))
}

/// `selection` is host-owned; policy neither dereferences nor logs credentials.
/// Only active deadlines belong in blocked_until; the host expires block rows.
#[derive(Clone)]
pub struct UsageCandidate<T> {
    pub selection: T,
    pub usage: Option<UsageReport>,
    pub usage_checked: bool,
    pub blocked_until: Option<f64>,
    pub order_pos: usize,
}

#[derive(Clone)]
pub struct UsageRankedCandidate<T> {
    pub candidate: UsageCandidate<T>,
    pub score: UsageRankingScore,
    pub plan_eligibility: Option<bool>,
    /// Persist under the request's scalar block_scope, after re-resolving its
    /// durable row identity. Ranking does not perform the store mutation.
    pub new_block_until: Option<f64>,
}

pub fn rank_usage_candidates<T>(
    candidates: Vec<UsageCandidate<T>>,
    strategy: &dyn CredentialRankingStrategy,
    context: Option<&CredentialRankingContext>,
    requirement: PlanRequirement,
    now_ms: f64,
) -> Vec<UsageRankedCandidate<T>> {
    let defaults = strategy.window_defaults();
    let mut ranked: Vec<_> = candidates
        .into_iter()
        .map(|candidate| {
            let usage = candidate.usage.as_ref();
            let scoped = usage.map(|report| strategy.scope_limits(report, context));
            let new_block_until = if candidate.blocked_until.is_none()
                && scoped.as_ref().is_some_and(|limits| is_usage_limit_reached(limits))
            {
                Some(usage_reset_at_ms(scoped.as_ref().unwrap(), now_ms).unwrap_or(now_ms + DEFAULT_BACKOFF_MS))
            } else {
                None
            };
            let blocked_until = candidate.blocked_until.or(new_block_until);
            let windows = usage.map(|report| strategy.find_window_limits(report, context)).unwrap_or_default();
            let score = UsageRankingScore {
                blocked: blocked_until.is_some(),
                blocked_until,
                has_priority_boost: strategy.has_priority_boost(
                    windows.primary,
                    windows.primary.is_none() && windows.secondary.is_some(),
                    context,
                ),
                usage_measured: windows.primary.is_some() || windows.secondary.is_some(),
                plan_priority: codex_plan_priority(usage, requirement),
                secondary_used: normalize_usage_fraction(windows.secondary),
                secondary_required_drain: compute_window_required_drain(
                    windows.secondary,
                    now_ms,
                    defaults.secondary_ms,
                ),
                primary_used: normalize_usage_fraction(windows.primary),
                primary_required_drain: compute_window_required_drain(windows.primary, now_ms, defaults.primary_ms),
                order_pos: candidate.order_pos,
            };
            let plan_eligibility = codex_plan_eligibility(usage, requirement);
            UsageRankedCandidate { candidate, score, plan_eligibility, new_block_until }
        })
        .collect();
    ranked.sort_by(|left, right| compare_usage_ranked_candidates(&left.score, &right.score, requirement));
    ranked
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OAuthCandidatePass {
    pub allow_blocked: bool,
    pub enforce_plan_requirement: bool,
}

/// Mirrors the strict -> plan-fitting last resort -> unfiltered last resort
/// ladder. The host must still skip every failed preflight and try every
/// candidate within each pass; this descriptor does not execute OAuth refresh.
pub fn oauth_candidate_passes<T>(
    requirement: PlanRequirement,
    candidates: &[UsageRankedCandidate<T>],
) -> Vec<OAuthCandidatePass> {
    let enforce = requirement != PlanRequirement::None
        && candidates.iter().any(|candidate| candidate.plan_eligibility == Some(true));
    let mut passes = vec![
        OAuthCandidatePass { allow_blocked: false, enforce_plan_requirement: enforce },
        OAuthCandidatePass { allow_blocked: true, enforce_plan_requirement: enforce },
    ];
    if enforce {
        passes.push(OAuthCandidatePass { allow_blocked: true, enforce_plan_requirement: false });
    }
    passes
}

pub fn candidate_allowed_in_pass<T>(candidate: &UsageRankedCandidate<T>, pass: OAuthCandidatePass) -> bool {
    (pass.allow_blocked || !candidate.score.blocked)
        && (!pass.enforce_plan_requirement || candidate.plan_eligibility == Some(true))
}

/// Pure report narrowing used by Codex healing. Fresh-block protection and
/// report/credential attribution remain host responsibilities.
pub fn scope_codex_report_to_block_scope(
    report: &UsageReport,
    block_scope: Option<&str>,
    strategy: Option<&dyn CredentialRankingStrategy>,
) -> UsageReport {
    let (Some(scope), Some(strategy)) = (block_scope.filter(|scope| !scope.is_empty()), strategy) else {
        return report.clone();
    };
    let context = CredentialRankingContext {
        model_id: Some(if scope == "spark" { "gpt-5.3-codex-spark" } else { "gpt-5.3-codex" }.into()),
    };
    if strategy.block_scopes(Some(&context)).first().map(String::as_str) != Some(scope) {
        return report.clone();
    }
    let mut scoped = report.clone();
    scoped.limits = strategy.scope_limits(report, Some(&context)).into_iter().cloned().collect();
    let meter_state =
        report.metadata.as_ref().and_then(|metadata| metadata.get("meterStates")).and_then(|states| states.get(scope));
    if meter_state.is_some_and(js_truthy) || scope == "spark" {
        let metadata = scoped.metadata.get_or_insert_with(Map::new);
        // Missing meter flags become undefined in native JS and must not inherit
        // the chat meter's healthy flags when the Spark meter is absent.
        for key in ["allowed", "limitReached"] {
            match meter_state.and_then(|state| state.get(key)) {
                Some(value) => {
                    metadata.insert(key.into(), value.clone());
                }
                None => {
                    metadata.remove(key);
                }
            }
        }
    }
    scoped
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0 && !value.is_nan()),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

pub fn is_healthy_codex_usage_report(report: &UsageReport) -> bool {
    report.provider == "openai-codex"
        && report.metadata.as_ref().and_then(|metadata| metadata.get("allowed")).and_then(Value::as_bool) == Some(true)
        && report.metadata.as_ref().and_then(|metadata| metadata.get("limitReached")).and_then(Value::as_bool)
            == Some(false)
        && !report.limits.iter().any(is_usage_limit_exhausted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: f64 = 1_800_000_000_000.0;
    const WEEK: f64 = 7.0 * 24.0 * HOUR_MS;

    fn limit(id: &str, used: f64, reset_in: Option<f64>, duration: f64) -> UsageLimit {
        let window_id = if id.ends_with("secondary") { "7d" } else { "1h" };
        UsageLimit {
            id: id.into(),
            label: id.into(),
            scope: UsageScope {
                provider: "openai-codex".into(),
                window_id: Some(window_id.into()),
                shared: Some(true),
                ..Default::default()
            },
            window: Some(UsageWindow {
                id: window_id.into(),
                label: window_id.into(),
                duration_ms: Some(duration),
                resets_at: reset_in.map(|reset| NOW + reset),
                ..Default::default()
            }),
            amount: UsageAmount {
                used: Some(used * 100.0),
                limit: Some(100.0),
                remaining: Some(100.0 * (1.0 - used)),
                used_fraction: Some(used),
                remaining_fraction: Some(1.0 - used),
                unit: UsageUnit::Percent,
                ..Default::default()
            },
            status: Some(if used >= 1.0 {
                UsageStatus::Exhausted
            } else if used >= 0.9 {
                UsageStatus::Warning
            } else {
                UsageStatus::Ok
            }),
            ..Default::default()
        }
    }

    fn report(primary: (f64, f64), secondary: (f64, f64), plan: Option<&str>) -> UsageReport {
        UsageReport {
            provider: "openai-codex".into(),
            fetched_at: NOW,
            limits: vec![
                limit("openai-codex:primary", primary.0, Some(primary.1), HOUR_MS),
                limit("openai-codex:secondary", secondary.0, Some(secondary.1), WEEK),
            ],
            metadata: plan.map(|plan| json!({"planType":plan}).as_object().unwrap().clone()),
            ..Default::default()
        }
    }

    fn candidate(name: &'static str, usage: Option<UsageReport>, order_pos: usize) -> UsageCandidate<&'static str> {
        UsageCandidate { selection: name, usage, usage_checked: true, blocked_until: None, order_pos }
    }

    fn context(model: &str) -> CredentialRankingContext {
        CredentialRankingContext { model_id: Some(model.into()) }
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!((actual - expected).abs() < 1e-10, "expected {expected}, got {actual}");
    }

    // Executable Rust translations of fixed native source cases. These test
    // policy only: upstream AuthStorage's refresh, SQLite, timeout and sticky
    // paths require separate host tests. No Bun oracle run is claimed here.
    #[test]
    fn normalized_usage_schema_fraction_precedence_and_status_contract() {
        let wire = json!({
            "provider":"openai-codex", "fetchedAt":NOW,
            "limits":[{
                "id":"openai-codex:primary", "label":"5 Hour",
                "scope":{"provider":"openai-codex","accountId":"seat","projectId":"project","orgId":"org",
                    "modelId":"model","tier":"paid","windowId":"5h","shared":true,"futureScope":"kept"},
                "window":{"id":"5h","label":"5 Hour","durationMs":5.0*HOUR_MS,"resetsAt":NOW+HOUR_MS,
                    "resetLabel":"regen","futureWindow":true},
                "amount":{"unit":"percent","used":125,"limit":100,"remaining":-25,
                    "usedFraction":1.25,"remainingFraction":-0.25,"futureAmount":3},
                "status":"warning","notes":["explicit provider override"],"futureLimit":false
            }],
            "resetCredits":{"availableCount":2,"credits":[{"grantedAt":"2026-01-01","expiresAt":"2026-12-31",
                "status":"available","futureCredit":"kept"}],"futureCredits":"kept"},
            "notes":["provider-wide"],"metadata":{"planType":"team","unknown":{"nested":[1,"two"]}},
            "raw":{"opaque":"kept"},"futureReport":[1,2]
        });
        let snapshot: UsageReport = serde_json::from_value(wire).unwrap();
        let roundtrip: UsageReport = serde_json::from_value(serde_json::to_value(&snapshot).unwrap()).unwrap();
        assert_eq!(roundtrip, snapshot);
        assert_eq!(snapshot.metadata.as_ref().unwrap()["unknown"], json!({"nested":[1,"two"]}));
        assert_eq!(snapshot.limits[0].scope.unknown_fields["futureScope"], "kept");
        assert_eq!(snapshot.raw, Some(json!({"opaque":"kept"})));
        assert!(!is_usage_limit_exhausted(&snapshot.limits[0]));
        assert_eq!(resolve_used_fraction(&snapshot.limits[0]), Some(1.25));

        // usage.ts resolveUsedFraction and auth-storage.ts:3942-3950 are
        // different contracts; missing values never become a measured zero.
        let cases = [
            (
                UsageAmount { used_fraction: Some(1.2), used: Some(1.0), limit: Some(100.0), ..Default::default() },
                Some(1.2),
            ),
            (
                UsageAmount { used: Some(3.0), limit: Some(2.0), unit: UsageUnit::Percent, ..Default::default() },
                Some(1.5),
            ),
            (
                UsageAmount { used: Some(25.0), limit: Some(0.0), unit: UsageUnit::Percent, ..Default::default() },
                Some(0.25),
            ),
            (UsageAmount { remaining_fraction: Some(1.5), ..Default::default() }, Some(0.0)),
            (UsageAmount { remaining_fraction: Some(-0.5), ..Default::default() }, Some(1.5)),
            (UsageAmount { remaining: Some(0.0), ..Default::default() }, None),
            (UsageAmount::default(), None),
        ];
        for (amount, expected) in cases {
            let item = UsageLimit { amount, ..Default::default() };
            assert_eq!(resolve_used_fraction(&item), expected);
        }
        for (status, expected) in [
            (None, true),
            (Some(UsageStatus::Unknown), true),
            (Some(UsageStatus::Exhausted), true),
            (Some(UsageStatus::Warning), false),
            (Some(UsageStatus::Ok), false),
        ] {
            let item = UsageLimit {
                amount: UsageAmount { used_fraction: Some(1.0), ..Default::default() },
                status,
                ..Default::default()
            };
            assert_eq!(is_usage_limit_exhausted(&item), expected);
        }
        for invalid in [
            json!({"provider":"p","fetchedAt":1,"limits":[{"id":"x","label":"x","scope":{"provider":"p"},"amount":{"unit":"made-up"}}]}),
            json!({"provider":"p","fetchedAt":1,"limits":[{"id":"x","label":"x","scope":{"provider":"p"},"amount":{"unit":"unknown"},"status":"made-up"}]}),
            json!({"provider":"p","fetchedAt":1}),
            json!({"provider":"p","fetchedAt":1,"limits":[],"metadata":null}),
            json!({"provider":"p","fetchedAt":1,"limits":[{"id":"x","label":"x","scope":{"provider":"p"},"amount":{"unit":"unknown","usedFraction":null}}]}),
        ] {
            assert!(serde_json::from_value::<UsageReport>(invalid).is_err());
        }
        let raw_null: UsageReport =
            serde_json::from_value(json!({"provider":"p","fetchedAt":1,"limits":[],"raw":null})).unwrap();
        assert_eq!(raw_null.raw, Some(Value::Null));
        assert_eq!(serde_json::to_value(raw_null).unwrap()["raw"], Value::Null);
        assert_eq!(normalize_usage_fraction(None), 0.5);
        let numeric_only = UsageLimit {
            amount: UsageAmount { used: Some(0.0), limit: Some(100.0), ..Default::default() },
            ..Default::default()
        };
        assert_eq!(resolve_used_fraction(&numeric_only), Some(0.0));
        assert_eq!(normalize_usage_fraction(Some(&numeric_only)), 0.5);
        for (fraction, expected) in [(f64::NAN, 0.5), (f64::INFINITY, 0.5), (-1.0, 0.0), (2.0, 1.0)] {
            let item = UsageLimit {
                amount: UsageAmount { used_fraction: Some(fraction), ..Default::default() },
                ..Default::default()
            };
            assert_eq!(normalize_usage_fraction(Some(&item)), expected);
        }
    }

    #[test]
    fn codex_plan_classes_requirements_and_block_routing_matrix() {
        // auth-storage.ts:1029-1096 and catalog behavior remain the source of
        // model plan requirements; display names and token substrings are not.
        for (plan, class, paid, pro) in [
            ("  ChatGPT-PLUS  ", CodexPlanClass::Paid, Some(true), Some(false)),
            ("CHATGPT Pro", CodexPlanClass::Pro, Some(true), Some(true)),
            ("chatgpt-pro-lite", CodexPlanClass::Paid, Some(true), Some(false)),
            ("prolite", CodexPlanClass::Paid, Some(true), Some(false)),
            ("business", CodexPlanClass::Paid, Some(true), Some(false)),
            ("free", CodexPlanClass::Free, Some(false), Some(false)),
            ("go", CodexPlanClass::Free, Some(false), Some(false)),
            ("free_plus", CodexPlanClass::Paid, Some(true), Some(false)),
            ("free_plus_pro", CodexPlanClass::Pro, Some(true), Some(true)),
            ("notprofessional", CodexPlanClass::Unknown, None, None),
            ("", CodexPlanClass::Unknown, None, None),
        ] {
            let usage = report((0.2, HOUR_MS), (0.3, WEEK), Some(plan));
            assert_eq!(classify_codex_plan(Some(&usage)), class, "{plan}");
            assert_eq!(codex_plan_eligibility(Some(&usage), PlanRequirement::Paid), paid, "{plan}");
            assert_eq!(codex_plan_eligibility(Some(&usage), PlanRequirement::Pro), pro, "{plan}");
        }
        for paid_plan in
            ["team", "enterprise", "edu", "education", "teacher", "teachers", "health", "gov", "government"]
        {
            assert_eq!(
                classify_codex_plan(Some(&report((0.1, HOUR_MS), (0.1, WEEK), Some(paid_plan)))),
                CodexPlanClass::Paid
            );
        }
        assert_eq!(codex_plan_eligibility(None, PlanRequirement::Paid), None);
        assert_eq!(codex_plan_eligibility(None, PlanRequirement::None), Some(true));
        for (provider, model, expected) in [
            ("openai-codex", Some("gpt-5.6-sol"), PlanRequirement::Paid),
            ("openai-codex", Some("gpt-5.3-codex-spark"), PlanRequirement::Pro),
            ("openai-codex", Some("gpt-5.6-terra"), PlanRequirement::None),
            ("openai", Some("gpt-5.3-codex-spark"), PlanRequirement::None),
            ("openai-codex", None, PlanRequirement::None),
        ] {
            assert_eq!(resolve_codex_plan_requirement(provider, model), expected);
        }
        for (provider, model, expected) in [
            ("cursor", Some("provider/ MODEL-A "), Some("model-policy:model-a")),
            ("openai-codex", Some("one/two/MODEL-A"), Some("model-policy:model-a")),
            ("openai-codex", Some("bad\0id"), None),
            ("openai-codex", Some("trailing/"), None),
            ("anthropic", Some("model"), None),
        ] {
            assert_eq!(model_account_policy_block_scope(provider, model).as_deref(), expected);
        }
        assert_eq!(CODEX_RANKING_STRATEGY.block_scopes(None), ["chat", "spark", "shared"]);
        assert_eq!(CODEX_RANKING_STRATEGY.block_scopes(Some(&CredentialRankingContext::default())), ["chat", "shared"]);
        let route = credential_block_routing(
            "openai-codex",
            "oauth",
            Some(&CODEX_RANKING_STRATEGY),
            Some("gpt-5.3-codex-spark"),
            Some("manual"),
        );
        assert_eq!(route.provider_key, "openai-codex:oauth");
        assert_eq!(route.block_scope.as_deref(), Some("manual"));
        assert_eq!(route.sibling_block_scopes, ["spark", "shared", "model-policy:gpt-5.3-codex-spark", "manual"]);
        assert_eq!(
            credential_block_routing("cursor", "api_key", None, Some("A"), None).sibling_block_scopes,
            ["model-policy:a"]
        );
        assert_eq!(scoped_backoff_key("p:oauth", None), "p:oauth");
        assert_eq!(scoped_backoff_key("p:oauth", Some("")), "p:oauth");
        assert_eq!(scoped_backoff_key("p:oauth", Some("chat")), "p:oauth\0chat");
        assert!(default_ranking_strategy("openai-codex").is_some());
        for provider in ["anthropic", "google-antigravity", "kimi-code", "zai", "alibaba-token-plan", "opencode-go"] {
            assert!(default_ranking_strategy(provider).is_none(), "unported default must remain visible: {provider}");
        }
    }

    #[test]
    fn fixed_source_ranked_pool_scenarios() {
        // Source mapping: packages/ai/test/auth-storage-codex-selection.test.ts
        // near-reset:290; five-hour:361; uncapped:397; incomplete:426;
        // exhausted:530; status override:559; earliest unblock:1833;
        // tiered exhausted fallback:2131; Pro preference:2169;
        // three seats:2365; null usage:2404; meter separation:2582.
        struct Case {
            source: &'static str,
            context: Option<CredentialRankingContext>,
            requirement: PlanRequirement,
            candidates: Vec<UsageCandidate<&'static str>>,
            expected: Vec<&'static str>,
        }
        let mut fresh = report((0.0, 5.0 * HOUR_MS), (0.8, 2.0 * HOUR_MS), None);
        fresh.limits[0].scope.window_id = Some("5h".into());
        fresh.limits[0].window.as_mut().unwrap().duration_ms = Some(5.0 * HOUR_MS);
        let mut uncapped = report((0.0, HOUR_MS), (0.9, WEEK), Some("pro"));
        uncapped.limits.remove(0);
        let mut team = report((0.2, HOUR_MS), (1.0, 6.0 * 24.0 * HOUR_MS), Some("team"));
        team.limits[1].status = Some(UsageStatus::Warning);
        let incomplete = UsageReport {
            provider: "openai-codex".into(),
            fetched_at: NOW,
            metadata: Some(json!({"allowed":true,"limitReached":false}).as_object().unwrap().clone()),
            ..Default::default()
        };
        let cases = vec![
            Case {
                source: "native:290 near reset",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("far", Some(report((0.3, 40.0 * 60_000.0), (0.55, 6.0 * 24.0 * HOUR_MS), None)), 0),
                    candidate("near", Some(report((0.4, 10.0 * 60_000.0), (0.92, 15.0 * 60_000.0), None)), 1),
                ],
                expected: vec!["near", "far"],
            },
            Case {
                source: "native:361 fresh five-hour",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("progress", Some(report((0.05, 4.0 * HOUR_MS), (0.1, 6.0 * 24.0 * HOUR_MS), None)), 0),
                    candidate("fresh", Some(fresh), 1),
                ],
                expected: vec!["fresh", "progress"],
            },
            Case {
                source: "native:397 secondary-only chat",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("capped", Some(report((0.0, HOUR_MS), (0.5, WEEK), None)), 0),
                    candidate("uncapped", Some(uncapped), 1),
                ],
                expected: vec!["uncapped", "capped"],
            },
            Case {
                source: "native:426 incomplete",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("incomplete", Some(incomplete), 0),
                    candidate("measured", Some(report((0.8, HOUR_MS), (0.8, WEEK), None)), 1),
                ],
                expected: vec!["measured", "incomplete"],
            },
            Case {
                source: "native:530 exhausted",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("spent", Some(report((0.2, HOUR_MS), (1.0, 60_000.0), None)), 0),
                    candidate("healthy", Some(report((0.8, HOUR_MS), (0.8, WEEK), None)), 1),
                ],
                expected: vec!["healthy", "spent"],
            },
            Case {
                source: "native:559 allowed Team at 100%",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate(
                        "spent",
                        Some(report((1.0, 3.0 * 24.0 * HOUR_MS), (1.0, 3.0 * 24.0 * HOUR_MS), Some("prolite"))),
                        0,
                    ),
                    candidate("team", Some(team), 1),
                ],
                expected: vec!["team", "spent"],
            },
            Case {
                source: "native:1833 earliest unblock",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("later", Some(report((1.0, 30.0 * 60_000.0), (1.0, 30.0 * 60_000.0), None)), 0),
                    candidate("soon", Some(report((1.0, 5.0 * 60_000.0), (1.0, 5.0 * 60_000.0), None)), 1),
                ],
                expected: vec!["soon", "later"],
            },
            Case {
                source: "native:2131 blocked ordering ignores tier",
                context: Some(context("gpt-5.6-sol")),
                requirement: PlanRequirement::Paid,
                candidates: vec![
                    candidate("paid", Some(report((1.0, 2.0 * HOUR_MS), (1.0, 6.0 * 24.0 * HOUR_MS), Some("plus"))), 0),
                    candidate("free", Some(report((1.0, 5.0 * 60_000.0), (1.0, 5.0 * 60_000.0), Some("free"))), 1),
                ],
                expected: vec!["free", "paid"],
            },
            Case {
                source: "native:2169 Pro before Plus",
                context: Some(context("gpt-5.3-codex-spark")),
                requirement: PlanRequirement::Pro,
                candidates: vec![
                    candidate("plus", Some(report((0.05, HOUR_MS), (0.05, WEEK), Some("plus"))), 0),
                    candidate("pro", Some(report((0.2, HOUR_MS), (0.2, WEEK), Some("pro"))), 1),
                ],
                expected: vec!["pro", "plus"],
            },
            Case {
                source: "native:2365 three-seat drain",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("fast", Some(report((0.2, 30.0 * 60_000.0), (0.7, 3.0 * 24.0 * HOUR_MS), None)), 0),
                    candidate("medium", Some(report((0.2, 30.0 * 60_000.0), (0.3, 5.0 * 24.0 * HOUR_MS), None)), 1),
                    candidate("slow", Some(report((0.2, 30.0 * 60_000.0), (0.1, 6.0 * 24.0 * HOUR_MS), None)), 2),
                ],
                expected: vec!["slow", "medium", "fast"],
            },
            Case {
                source: "native:2404 fetch returned null",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("unknown", None, 0),
                    candidate("known", Some(report((0.2, HOUR_MS), (0.3, WEEK), None)), 1),
                ],
                expected: vec!["known", "unknown"],
            },
            Case {
                source: "native comparator:4740 hot before measured",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("hot", Some(report((0.85, HOUR_MS), (0.1, 60_000.0), None)), 0),
                    candidate("unknown", None, 1),
                ],
                expected: vec!["unknown", "hot"],
            },
            Case {
                source: "native comparator:4752 plan before heat",
                context: Some(context("gpt-5.6-sol")),
                requirement: PlanRequirement::Paid,
                candidates: vec![
                    candidate("free", Some(report((0.1, HOUR_MS), (0.1, WEEK), Some("free"))), 0),
                    candidate("paid", Some(report((0.9, HOUR_MS), (0.9, WEEK), Some("plus"))), 1),
                ],
                expected: vec!["paid", "free"],
            },
            Case {
                source: "native:3076 genuine tie uses seeded order",
                context: None,
                requirement: PlanRequirement::None,
                candidates: vec![
                    candidate("other", Some(report((0.25, HOUR_MS), (0.25, WEEK), None)), 1),
                    candidate("pin", Some(report((0.25, HOUR_MS), (0.25, WEEK), None)), 0),
                ],
                expected: vec!["pin", "other"],
            },
        ];
        for case in cases {
            let ranked = rank_usage_candidates(
                case.candidates,
                &CODEX_RANKING_STRATEGY,
                case.context.as_ref(),
                case.requirement,
                NOW,
            );
            let actual: Vec<_> = ranked.iter().map(|candidate| candidate.candidate.selection).collect();
            assert_eq!(actual, case.expected, "{}", case.source);
        }
    }

    #[test]
    fn exhausted_plan_passes_iterate_all_candidates_and_preserve_unknown_access() {
        let ctx = context("gpt-5.6-sol");
        let blocked = rank_usage_candidates(
            vec![
                candidate("free", Some(report((1.0, 5.0 * 60_000.0), (1.0, 5.0 * 60_000.0), Some("free"))), 0),
                candidate("paid", Some(report((1.0, 2.0 * HOUR_MS), (1.0, 6.0 * 24.0 * HOUR_MS), Some("plus"))), 1),
            ],
            &CODEX_RANKING_STRATEGY,
            Some(&ctx),
            PlanRequirement::Paid,
            NOW,
        );
        let passes = oauth_candidate_passes(PlanRequirement::Paid, &blocked);
        assert_eq!(
            passes,
            vec![
                OAuthCandidatePass { allow_blocked: false, enforce_plan_requirement: true },
                OAuthCandidatePass { allow_blocked: true, enforce_plan_requirement: true },
                OAuthCandidatePass { allow_blocked: true, enforce_plan_requirement: false },
            ]
        );
        let attempts: Vec<Vec<_>> = passes
            .iter()
            .map(|pass| {
                blocked
                    .iter()
                    .filter(|item| candidate_allowed_in_pass(item, *pass))
                    .map(|item| item.candidate.selection)
                    .collect()
            })
            .collect();
        assert_eq!(attempts, vec![vec![], vec!["paid"], vec!["free", "paid"]]);
        assert_eq!(blocked[1].new_block_until, Some(NOW + 6.0 * 24.0 * HOUR_MS));

        let unknown = rank_usage_candidates(
            vec![
                candidate("unknown", None, 0),
                candidate("free", Some(report((0.1, HOUR_MS), (0.1, WEEK), Some("free"))), 1),
            ],
            &CODEX_RANKING_STRATEGY,
            Some(&ctx),
            PlanRequirement::Paid,
            NOW,
        );
        assert_eq!(
            oauth_candidate_passes(PlanRequirement::Paid, &unknown),
            vec![
                OAuthCandidatePass { allow_blocked: false, enforce_plan_requirement: false },
                OAuthCandidatePass { allow_blocked: true, enforce_plan_requirement: false },
            ]
        );
        assert_eq!(unknown[0].plan_eligibility, None);
        assert_eq!(unknown[0].score.plan_priority, 1);
        assert_eq!(unknown[1].plan_eligibility, Some(false));
        assert!(
            unknown.iter().all(|item| candidate_allowed_in_pass(
                item,
                oauth_candidate_passes(PlanRequirement::Paid, &unknown)[0]
            ))
        );
        // Preflight failure membership is host state, and must additionally
        // suppress the candidate in every pass; these pure tests don't perform
        // or replay a refresh operation.
    }

    #[test]
    fn codex_meter_windows_boost_and_healing_are_scope_local() {
        let chat = CredentialRankingContext::default();
        let spark = context("gpt-5.3-codex-spark");
        let mut usage = report((0.2, HOUR_MS), (0.3, WEEK), Some("pro"));
        usage.limits.push(limit("openai-codex:spark:primary", 1.0, Some(5.0 * HOUR_MS), 5.0 * HOUR_MS));
        usage.limits.push(limit("openai-codex:spark:secondary", 0.1, Some(WEEK), WEEK));
        usage.metadata = Some(
            json!({"planType":"pro","allowed":true,"limitReached":false,
            "meterStates":{"chat":{"allowed":true,"limitReached":false},"spark":{"allowed":false,"limitReached":true}}})
            .as_object()
            .unwrap()
            .clone(),
        );
        assert!(!is_usage_limit_reached(&CODEX_RANKING_STRATEGY.scope_limits(&usage, Some(&chat))));
        assert!(is_usage_limit_reached(&CODEX_RANKING_STRATEGY.scope_limits(&usage, Some(&spark))));
        assert_eq!(
            CODEX_RANKING_STRATEGY.find_window_limits(&usage, Some(&chat)).primary.unwrap().id,
            "openai-codex:primary"
        );
        assert_eq!(
            CODEX_RANKING_STRATEGY.find_window_limits(&usage, Some(&spark)).primary.unwrap().id,
            "openai-codex:spark:primary"
        );
        assert!(is_healthy_codex_usage_report(&scope_codex_report_to_block_scope(
            &usage,
            Some("chat"),
            Some(&CODEX_RANKING_STRATEGY)
        )));
        assert!(!is_healthy_codex_usage_report(&scope_codex_report_to_block_scope(
            &usage,
            Some("spark"),
            Some(&CODEX_RANKING_STRATEGY)
        )));
        assert!(!is_healthy_codex_usage_report(&scope_codex_report_to_block_scope(
            &usage,
            Some("shared"),
            Some(&CODEX_RANKING_STRATEGY)
        )));
        let mut missing_spark = usage.clone();
        missing_spark.limits.retain(|limit| !limit.id.contains(":spark:"));
        missing_spark.metadata.as_mut().unwrap().remove("meterStates");
        let scoped = scope_codex_report_to_block_scope(&missing_spark, Some("spark"), Some(&CODEX_RANKING_STRATEGY));
        assert!(scoped.limits.is_empty());
        assert!(!is_healthy_codex_usage_report(&scoped));
        let mut spark_only = usage.clone();
        spark_only.limits.retain(|limit| limit.id == "openai-codex:spark:secondary");
        let windows = CODEX_RANKING_STRATEGY.find_window_limits(&spark_only, Some(&spark));
        assert!(windows.primary.is_none());
        assert!(windows.secondary.is_some());
        assert!(!CODEX_RANKING_STRATEGY.has_priority_boost(windows.primary, true, Some(&spark)));
        // Native find fallback is ID substring before windowId, in source order.
        spark_only.limits[0].id = "openai-codex:spark:quota".into();
        assert!(CODEX_RANKING_STRATEGY.find_window_limits(&spark_only, Some(&spark)).secondary.is_some());
        spark_only.limits[0].scope.window_id = Some("7D".into());
        assert!(CODEX_RANKING_STRATEGY.find_window_limits(&spark_only, Some(&spark)).secondary.is_some());
        for (duration, used, expected) in [
            (5.0 * HOUR_MS - 60_000.0, 0.0, true),
            (5.0 * HOUR_MS + 60_000.0, 0.0, true),
            (5.0 * HOUR_MS + 60_001.0, 0.0, false),
            (5.0 * HOUR_MS, 0.1, false),
            (f64::INFINITY, 0.0, false),
        ] {
            let item = limit("openai-codex:primary", used, Some(HOUR_MS), duration);
            assert_eq!(CODEX_RANKING_STRATEGY.has_priority_boost(Some(&item), false, Some(&chat)), expected);
        }
    }

    #[test]
    fn drain_clock_fallback_tolerance_and_generic_strategy_contract() {
        // auth-storage.ts:4701-4790; native clockless case:3017 and tie:3076.
        let clockless = limit("openai-codex:secondary", 0.25, None, WEEK);
        assert_close(compute_window_required_drain(Some(&clockless), NOW, WEEK), 0.75 / 168.0);
        let long_clock = limit("openai-codex:secondary", 0.25, Some(2.0 * WEEK), WEEK);
        assert_close(compute_window_required_drain(Some(&long_clock), NOW, WEEK), 0.75 / 168.0);
        let stale = limit("openai-codex:secondary", 0.25, Some(-HOUR_MS), WEEK);
        assert_close(compute_window_required_drain(Some(&stale), NOW, WEEK), 45.0);
        assert_close(compute_window_required_drain(None, NOW, HOUR_MS), 0.5);
        let mut invalid_clock = clockless.clone();
        invalid_clock.window.as_mut().unwrap().resets_at = Some(f64::INFINITY);
        assert_close(compute_window_required_drain(Some(&invalid_clock), NOW, WEEK), 0.75 / 168.0);
        invalid_clock.window.as_mut().unwrap().duration_ms = Some(f64::NAN);
        assert!(compute_window_required_drain(Some(&invalid_clock), NOW, WEEK).is_nan());
        for (left, right, equal) in [
            (1.0, 1.0 + 0.0000005, true),
            (1.0, 1.0 + 0.000002, false),
            (0.0, 0.0000000005, true),
            (0.0, 0.000000002, false),
            (1e6, 1e6 + 0.5, true),
        ] {
            assert_eq!(compare_usage_ranking_metric(left, right) == 0.0, equal);
        }
        assert_eq!(compare_usage_ranking_metric(f64::INFINITY, f64::INFINITY), 0.0);
        assert_eq!(compare_usage_ranking_metric(0.0, f64::INFINITY), -1.0);
        let exhausted = [
            limit("a", 1.0, Some(HOUR_MS), HOUR_MS),
            limit("b", 1.0, Some(WEEK), WEEK),
            limit("healthy", 0.1, Some(2.0 * WEEK), WEEK),
            limit("stale", 1.0, Some(-HOUR_MS), HOUR_MS),
        ];
        assert_eq!(usage_reset_at_ms(&exhausted.iter().collect::<Vec<_>>(), NOW), Some(NOW + WEEK));

        // A host extension supplies policy; no built-in provider is fabricated.
        struct ExtensionStrategy;
        impl CredentialRankingStrategy for ExtensionStrategy {
            fn find_window_limits<'a>(
                &self,
                report: &'a UsageReport,
                _context: Option<&CredentialRankingContext>,
            ) -> UsageWindowLimits<'a> {
                UsageWindowLimits { primary: report.limits.first(), secondary: report.limits.get(1) }
            }
            fn window_defaults(&self) -> WindowDefaults {
                WindowDefaults { primary_ms: 5.0 * HOUR_MS, secondary_ms: WEEK }
            }
        }
        let extension = ExtensionStrategy;
        let ranked = rank_usage_candidates(
            vec![
                candidate(
                    "clockless",
                    Some(UsageReport {
                        provider: "extension".into(),
                        fetched_at: NOW,
                        limits: vec![limit("p", 0.0, None, 5.0 * HOUR_MS), limit("s", 0.0, None, WEEK)],
                        ..Default::default()
                    }),
                    0,
                ),
                candidate(
                    "clocked",
                    Some(UsageReport {
                        provider: "extension".into(),
                        fetched_at: NOW,
                        limits: vec![
                            limit("p", 0.0, Some(4.0 * HOUR_MS), 5.0 * HOUR_MS),
                            limit("s", 0.05, Some(22.0 * HOUR_MS), WEEK),
                        ],
                        ..Default::default()
                    }),
                    1,
                ),
            ],
            &extension,
            None,
            PlanRequirement::None,
            NOW,
        );
        assert_eq!(ranked[0].candidate.selection, "clocked");
        assert!(extension.block_scopes(None).is_empty());
        assert_eq!(extension.scope_limits_for_reserve(&ranked[0].candidate.usage.clone().unwrap(), None).len(), 2);
    }
}
