//! Fixed OMP Codex usage adapter, source
//! `596f2da7101178214aa27a753529d15e6b7ad91d`,
//! `packages/ai/src/usage/{openai-codex,openai-codex-base-url,openai-codex-reset}.ts`.
//!
//! Implements usage/header parsing, GET reset-credit detail enrichment and the
//! native reset-credit consume wire. The Host owns permission to redeem.
//! Reports preserve provider raw data; neither credentials nor transport errors
//! are logged. The host owns refresh, usage cache/history, identity attribution,
//! ranking, persisted blocks and permission to fetch. The Codex ranking strategy
//! itself lives in auth_storage_policy.
//!
//! Native canonical-origin normalization is retained. Production requests never
//! follow redirects. Explicit test-fixture endpoint callbacks may target only
//! loopback. One caller timeout/cancellation covers usage and detail requests.
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

use crate::auth_storage_policy::{
    UsageAmount, UsageCredential, UsageCredentialType, UsageLimit, UsageReport, UsageResetCreditDetail,
    UsageResetCredits, UsageScope, UsageStatus, UsageUnit, UsageWindow,
};
use crate::model_policy::to_number;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

#[cfg(any(test, feature = "test-fixture"))]
use std::sync::Arc;

pub const CODEX_BASE_URL: &str = "https://chatgpt.com/backend-api";
// Fixed packages/utils/src/dirs.ts:30-33 and package.json version 18.1.8.
pub const CODEX_USAGE_USER_AGENT: &str = "omp/18.1.8";
const USAGE_PATH: &str = "wham/usage";
const RESET_CREDITS_PATH: &str = "wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_PATH: &str = "wham/rate-limit-reset-credits/consume";
const JWT_AUTH_CLAIM: &str = "https://api.openai.com/auth";
const JWT_PROFILE_CLAIM: &str = "https://api.openai.com/profile";

fn js_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}'
        | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}'
        | '\u{3000}' | '\u{feff}')
}

fn js_trim(text: &str) -> &str {
    text.trim_matches(js_whitespace)
}

/// Account endpoints are canonical ChatGPT APIs. A streaming proxy override
/// does not own these routes. URL.host includes non-default ports in native JS.
pub fn normalize_codex_base_url(base_url: Option<&str>) -> String {
    let Some(text) = base_url.map(js_trim).map(|text| text.trim_end_matches('/')).filter(|text| !text.is_empty())
    else {
        return CODEX_BASE_URL.into();
    };
    let Ok(url) = reqwest::Url::parse(text) else {
        return CODEX_BASE_URL.into();
    };
    let accepted_host = matches!(url.host_str(), Some("chatgpt.com" | "chat.openai.com")) && url.port().is_none();
    if !accepted_host {
        return CODEX_BASE_URL.into();
    }
    format!("{}/backend-api", url.origin().ascii_serialization())
}

pub fn codex_usage_url(base_url: Option<&str>) -> String {
    format!("{}/{USAGE_PATH}", normalize_codex_base_url(base_url))
}

pub fn codex_reset_credits_url(base_url: Option<&str>) -> String {
    format!("{}/{RESET_CREDITS_PATH}", normalize_codex_base_url(base_url))
}

fn parse_jwt(token: &str) -> Option<Value> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let body = parts[1].replace('-', "+").replace('_', "/");
    // Buffer's base64 decoder ignores non-alphabet characters, stops at the
    // first padding marker, and ignores an unmatched final sextet.
    let mut base64: String = body
        .chars()
        .take_while(|character| *character != '=')
        .filter(|character| character.is_ascii_alphanumeric() || matches!(*character, '+' | '/'))
        .collect();
    if base64.len() % 4 == 1 {
        base64.pop();
    }
    base64.extend(std::iter::repeat_n('=', (4 - base64.len() % 4) % 4));
    // Buffer.from(...,"base64") accepts whitespace and unpadded input. The
    // general-purpose config mirrors its permissive padding for JWT payloads.
    let config = base64::engine::GeneralPurposeConfig::new()
        .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true);
    let engine = base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD, config);
    let bytes = engine.decode(base64).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    serde_json::from_str(&text).ok()
}

fn normalize_email(email: Option<&str>) -> Option<String> {
    email.map(js_trim).map(str::to_lowercase).filter(|email| !email.is_empty())
}

/// Identity metadata comes from caller fields first, then an unverified JWT
/// payload. Parsing JWT claims is not credential validation or authorization.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodexUsageIdentity {
    pub account_id: Option<String>,
    pub email: Option<String>,
}

pub fn codex_usage_identity(credential: &UsageCredential) -> CodexUsageIdentity {
    let jwt = credential.access_token.as_deref().filter(|token| !token.is_empty()).and_then(parse_jwt);
    let jwt_account = jwt
        .as_ref()
        .and_then(|payload| payload.get(JWT_AUTH_CLAIM))
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str);
    let jwt_email = jwt
        .as_ref()
        .and_then(|payload| payload.get(JWT_PROFILE_CLAIM))
        .and_then(|claim| claim.get("email"))
        .and_then(Value::as_str);
    CodexUsageIdentity {
        account_id: credential.account_id.clone().or_else(|| jwt_account.map(str::to_owned)),
        email: normalize_email(credential.email.as_deref().or(jwt_email)),
    }
}

#[derive(Clone, Copy, Default)]
struct ParsedWindow {
    used_percent: Option<f64>,
    limit_window_seconds: Option<f64>,
    reset_after_seconds: Option<f64>,
    reset_at: Option<f64>,
}

struct ParsedAdditional {
    limit_name: Option<String>,
    metered_feature: Option<String>,
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary: Option<ParsedWindow>,
    secondary: Option<ParsedWindow>,
}

struct ParsedUsage {
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary: Option<ParsedWindow>,
    secondary: Option<ParsedWindow>,
    additional: Vec<ParsedAdditional>,
}

fn number(record: &Map<String, Value>, field: &str) -> Option<f64> {
    record.get(field).and_then(to_number)
}

fn parse_window(payload: Option<&Value>) -> Option<ParsedWindow> {
    let record = payload?.as_object()?;
    let parsed = ParsedWindow {
        used_percent: number(record, "used_percent"),
        limit_window_seconds: number(record, "limit_window_seconds"),
        reset_after_seconds: number(record, "reset_after_seconds"),
        reset_at: number(record, "reset_at"),
    };
    (parsed.used_percent.is_some()
        || parsed.limit_window_seconds.is_some()
        || parsed.reset_after_seconds.is_some()
        || parsed.reset_at.is_some())
    .then_some(parsed)
}

fn parse_additional(payload: &Value) -> Option<ParsedAdditional> {
    let record = payload.as_object()?;
    let rate = record.get("rate_limit")?.as_object()?;
    let parsed = ParsedAdditional {
        limit_name: record.get("limit_name").and_then(Value::as_str).map(str::to_owned),
        metered_feature: record.get("metered_feature").and_then(Value::as_str).map(str::to_owned),
        allowed: rate.get("allowed").and_then(Value::as_bool),
        limit_reached: rate.get("limit_reached").and_then(Value::as_bool),
        primary: parse_window(rate.get("primary_window")),
        secondary: parse_window(rate.get("secondary_window")),
    };
    (parsed.primary.is_some()
        || parsed.secondary.is_some()
        || parsed.allowed.is_some()
        || parsed.limit_reached.is_some())
    .then_some(parsed)
}

fn parse_usage(payload: &Value) -> Option<ParsedUsage> {
    let record = payload.as_object()?;
    let rate = record.get("rate_limit").and_then(Value::as_object);
    let additional: Vec<_> = record
        .get("additional_rate_limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(parse_additional)
        .collect();
    if rate.is_none() && additional.is_empty() {
        return None;
    }
    let parsed = ParsedUsage {
        allowed: rate.and_then(|rate| rate.get("allowed")).and_then(Value::as_bool),
        limit_reached: rate.and_then(|rate| rate.get("limit_reached")).and_then(Value::as_bool),
        primary: parse_window(rate.and_then(|rate| rate.get("primary_window"))),
        secondary: parse_window(rate.and_then(|rate| rate.get("secondary_window"))),
        additional,
    };
    (parsed.primary.is_some()
        || parsed.secondary.is_some()
        || parsed.allowed.is_some()
        || parsed.limit_reached.is_some()
        || !parsed.additional.is_empty())
    .then_some(parsed)
}

pub fn parse_codex_usage_reset_credits(payload: &Value) -> Option<UsageResetCredits> {
    let count = payload
        .as_object()?
        .get("rate_limit_reset_credits")?
        .as_object()?
        .get("available_count")
        .and_then(to_number)?;
    Some(UsageResetCredits { available_count: count.trunc().max(0.0), ..Default::default() })
}

fn resolve_reset_time(window: ParsedWindow, now_ms: f64) -> Option<f64> {
    if let Some(reset) = window.reset_at {
        let milliseconds = if reset > 1_000_000_000_000.0 { reset } else { reset * 1000.0 };
        if milliseconds.is_finite() {
            return Some(milliseconds);
        }
    }
    window.reset_after_seconds.map(|seconds| now_ms + seconds * 1000.0)
}

fn window_label(seconds: f64) -> (String, String) {
    // Window inputs are finite; JS Math.round rounds halves towards +infinity.
    // Keep huge finite values unchanged to avoid overflowing value + 0.5.
    let round = |value: f64| {
        if value.abs() >= 4_503_599_627_370_496.0 {
            return value;
        }
        let lower = value.floor();
        if value - lower >= 0.5 { lower + 1.0 } else { lower }
    };
    if seconds >= 86_400.0 {
        let days = round(seconds / 86_400.0);
        (format!("{days}d"), format!("{days} {}", if days == 1.0 { "day" } else { "days" }))
    } else {
        let hours = round(seconds / 3600.0).max(1.0);
        (format!("{hours}h"), format!("{hours} {}", if hours == 1.0 { "hour" } else { "hours" }))
    }
}

fn build_window(window: ParsedWindow, key: &str, now_ms: f64) -> UsageWindow {
    let (id, label, duration_ms) = match window.limit_window_seconds {
        Some(seconds) => {
            let (id, label) = window_label(seconds);
            (id, label, Some(seconds * 1000.0))
        }
        None => (key.into(), if key == "primary" { "Primary window" } else { "Secondary window" }.into(), None),
    };
    UsageWindow { id, label, duration_ms, resets_at: resolve_reset_time(window, now_ms), ..Default::default() }
}

fn build_amount(window: ParsedWindow) -> UsageAmount {
    let Some(used) = window.used_percent else {
        return UsageAmount { unit: UsageUnit::Percent, ..Default::default() };
    };
    let used = used.clamp(0.0, 100.0);
    let fraction = used / 100.0;
    UsageAmount {
        used: Some(used),
        limit: Some(100.0),
        remaining: Some((100.0 - used).max(0.0)),
        used_fraction: Some(fraction),
        remaining_fraction: Some((1.0 - fraction).max(0.0)),
        unit: UsageUnit::Percent,
        ..Default::default()
    }
}

fn usage_status(used_fraction: Option<f64>, explicitly_allowed: bool) -> UsageStatus {
    match used_fraction {
        None => UsageStatus::Unknown,
        Some(used) if used >= 1.0 => {
            if explicitly_allowed {
                UsageStatus::Warning
            } else {
                UsageStatus::Exhausted
            }
        }
        Some(used) if used >= 0.9 => UsageStatus::Warning,
        Some(_) => UsageStatus::Ok,
    }
}

fn main_limit(
    key: &str,
    parsed: ParsedWindow,
    allowed: Option<bool>,
    reached: Option<bool>,
    now_ms: f64,
) -> UsageLimit {
    let window = build_window(parsed, key, now_ms);
    let amount = build_amount(parsed);
    let status = usage_status(amount.used_fraction, allowed == Some(true) && reached == Some(false));
    UsageLimit {
        id: format!("openai-codex:{key}"),
        label: window.label.clone(),
        scope: UsageScope {
            provider: "openai-codex".into(),
            window_id: Some(window.id.clone()),
            shared: Some(true),
            ..Default::default()
        },
        window: Some(window),
        amount,
        status: Some(status),
        ..Default::default()
    }
}

pub fn additional_limit_slug(limit_name: Option<&str>, metered_feature: Option<&str>) -> String {
    let probe = format!("{} {}", limit_name.unwrap_or(""), metered_feature.unwrap_or("")).to_lowercase();
    if probe.contains("spark") || probe.contains("bengalfox") {
        return "spark".into();
    }
    let source = metered_feature.or(limit_name).unwrap_or("extra").to_lowercase();
    let source = source.strip_prefix("codex-").or_else(|| source.strip_prefix("codex_")).unwrap_or(&source);
    let mut slug = String::new();
    let mut separator = false;
    for character in source.chars() {
        if character.is_ascii_lowercase() || character.is_ascii_digit() {
            slug.push(character);
            separator = false;
        } else if !separator {
            slug.push('-');
            separator = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() { "extra".into() } else { slug.into() }
}

fn additional_display_name(slug: &str, limit_name: Option<&str>) -> String {
    if slug == "spark" {
        return "Spark".into();
    }
    if let Some(name) = limit_name.filter(|name| !name.is_empty()) {
        return name.into();
    }
    let mut display = String::new();
    let mut after_separator = true;
    for character in slug.chars() {
        if after_separator && character.is_ascii_lowercase() {
            display.push(character.to_ascii_uppercase());
        } else {
            display.push(character);
        }
        after_separator = character == '-';
    }
    // Regex only replaces a hyphen when immediately followed by [a-z]. A
    // hyphen preceding a digit is retained in the native display name.
    let mut chars = display.chars().peekable();
    let mut result = String::new();
    while let Some(character) = chars.next() {
        result.push(if character == '-' && chars.peek().is_some_and(char::is_ascii_uppercase) {
            ' '
        } else {
            character
        });
    }
    result
}

fn additional_limit(
    key: &str,
    parsed: ParsedWindow,
    extra: &ParsedAdditional,
    slug: &str,
    display_name: &str,
    account_id: Option<&str>,
    now_ms: f64,
) -> UsageLimit {
    let mut limit = main_limit(key, parsed, extra.allowed, extra.limit_reached, now_ms);
    limit.id = format!("openai-codex:{slug}:{key}");
    limit.label = format!("{} ({display_name})", limit.label);
    limit.scope.account_id = account_id.map(str::to_owned);
    limit.scope.tier = Some(slug.into());
    limit.scope.model_id = extra.limit_name.clone();
    limit
}

fn meter_state(allowed: Option<bool>, reached: Option<bool>) -> Value {
    let mut map = Map::new();
    if let Some(allowed) = allowed {
        map.insert("allowed".into(), Value::Bool(allowed));
    }
    if let Some(reached) = reached {
        map.insert("limitReached".into(), Value::Bool(reached));
    }
    Value::Object(map)
}

/// A successful native JSON response always becomes a report, even when the
/// payload has no recognized windows. Empty/unknown usage is not zero usage.
pub fn parse_codex_usage_payload(payload: &Value, credential: &UsageCredential, now_ms: f64) -> UsageReport {
    let identity = codex_usage_identity(credential);
    let parsed = parse_usage(payload);
    let allowed = parsed.as_ref().and_then(|parsed| parsed.allowed);
    let reached = parsed.as_ref().and_then(|parsed| parsed.limit_reached);
    let mut limits = Vec::new();
    let mut states = Map::new();
    states.insert("chat".into(), meter_state(allowed, reached));
    if let Some(parsed) = &parsed {
        if let Some(primary) = parsed.primary {
            limits.push(main_limit("primary", primary, allowed, reached, now_ms));
        }
        if let Some(secondary) = parsed.secondary {
            limits.push(main_limit("secondary", secondary, allowed, reached, now_ms));
        }
        for extra in &parsed.additional {
            let slug = additional_limit_slug(extra.limit_name.as_deref(), extra.metered_feature.as_deref());
            let display = additional_display_name(&slug, extra.limit_name.as_deref());
            states.insert(slug.clone(), meter_state(extra.allowed, extra.limit_reached));
            if let Some(primary) = extra.primary {
                limits.push(additional_limit(
                    "primary",
                    primary,
                    extra,
                    &slug,
                    &display,
                    identity.account_id.as_deref(),
                    now_ms,
                ));
            }
            if let Some(secondary) = extra.secondary {
                limits.push(additional_limit(
                    "secondary",
                    secondary,
                    extra,
                    &slug,
                    &display,
                    identity.account_id.as_deref(),
                    now_ms,
                ));
            }
        }
    }
    let mut metadata = Map::new();
    if let Some(plan) = payload.as_object().and_then(|record| record.get("plan_type")).and_then(Value::as_str) {
        metadata.insert("planType".into(), Value::String(plan.into()));
    }
    if let Some(allowed) = allowed {
        metadata.insert("allowed".into(), Value::Bool(allowed));
    }
    if let Some(reached) = reached {
        metadata.insert("limitReached".into(), Value::Bool(reached));
    }
    if let Some(email) = identity.email {
        metadata.insert("email".into(), Value::String(email));
    }
    if let Some(account_id) = identity.account_id {
        metadata.insert("accountId".into(), Value::String(account_id));
    }
    metadata.insert("meterStates".into(), Value::Object(states));
    UsageReport {
        provider: "openai-codex".into(),
        fetched_at: now_ms,
        limits,
        reset_credits: parse_codex_usage_reset_credits(payload),
        metadata: Some(metadata),
        raw: Some(payload.clone()),
        ..Default::default()
    }
}

/// Header names follow the native lowercased Record contract.
pub fn parse_codex_rate_limit_headers(headers: &BTreeMap<String, String>, now_ms: f64) -> Option<UsageReport> {
    let parse = |key: &str| -> Option<ParsedWindow> {
        let number = |suffix: &str| {
            headers.get(&format!("x-codex-{key}-{suffix}")).and_then(|text| to_number(&Value::String(text.clone())))
        };
        Some(ParsedWindow {
            used_percent: Some(number("used-percent")?),
            limit_window_seconds: number("window-minutes").map(|minutes| minutes * 60.0),
            reset_at: number("reset-at"),
            ..Default::default()
        })
    };
    let mut limits = Vec::new();
    if let Some(primary) = parse("primary") {
        limits.push(main_limit("primary", primary, None, None, now_ms));
    }
    if let Some(secondary) = parse("secondary") {
        limits.push(main_limit("secondary", secondary, None, None, now_ms));
    }
    if limits.is_empty() {
        return None;
    }
    let mut metadata = Map::new();
    metadata.insert("source".into(), Value::String("ratelimit-headers".into()));
    Some(UsageReport {
        provider: "openai-codex".into(),
        fetched_at: now_ms,
        limits,
        metadata: Some(metadata),
        ..Default::default()
    })
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetCredit {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "nullable_string")]
    pub redeem_started_at: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "nullable_string")]
    pub redeemed_at: Option<Option<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

fn nullable_string<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetCreditList {
    pub credits: Vec<CodexResetCredit>,
    pub available_count: f64,
}

/// Native body codes take precedence over HTTP status, including future codes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResetConsumeResult {
    pub ok: bool,
    pub code: String,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Private wire bytes are deliberately neither serializable nor debuggable.
pub struct CodexResetConsumeRequest<'a> {
    pub access_token: &'a str,
    pub account_id: Option<&'a str>,
    pub base_url: Option<&'a str>,
    pub credit_id: &'a str,
    pub redeem_request_id: Option<&'a str>,
}

/// Native expiry ordering for ISO/RFC dates, with stable ties and first-row
/// fallback. Bun's wider Date.parse inputs remain a platform parity boundary.
pub fn pick_soonest_expiring_credit(credits: &[CodexResetCredit]) -> Option<&CodexResetCredit> {
    let mut dated: Option<(&CodexResetCredit, i64)> = None;
    let mut undated = None;
    for credit in credits {
        if credit.status.as_deref().unwrap_or("available") != "available" {
            continue;
        }
        let expiry = credit.expires_at.as_deref().and_then(|text| {
            chrono::DateTime::parse_from_rfc3339(text)
                .or_else(|_| chrono::DateTime::parse_from_rfc2822(text))
                .map(|date| date.timestamp_millis())
                .ok()
                .or_else(|| {
                    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                        .ok()
                        .and_then(|date| date.and_hms_opt(0, 0, 0))
                        .map(|date| date.and_utc().timestamp_millis())
                })
        });
        match expiry {
            Some(expiry) if dated.is_none_or(|(_, best)| expiry < best) => dated = Some((credit, expiry)),
            None if undated.is_none() => undated = Some(credit),
            _ => {}
        }
    }
    dated.map(|(credit, _)| credit).or(undated).or_else(|| credits.first())
}

/// Preserve the safe request identity when an externally visible POST may
/// already have occurred. No automatic retry creates another request identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodexResetConsumeError {
    Cancelled,
    InvalidEndpoint,
    OutcomeUnknown { redeem_request_id: String },
}
impl std::fmt::Display for CodexResetConsumeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => "saved reset cancelled before dispatch",
            Self::InvalidEndpoint => "saved reset endpoint is invalid",
            Self::OutcomeUnknown { .. } => "saved reset outcome unknown",
        })
    }
}
impl std::error::Error for CodexResetConsumeError {}

fn parse_credit(payload: &Value) -> Option<CodexResetCredit> {
    let record = payload.as_object()?;
    let id = record.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let string = |key: &str| record.get(key).and_then(Value::as_str).map(str::to_owned);
    let nullable = |key: &str| match record.get(key) {
        Some(Value::Null) => Some(None),
        Some(Value::String(text)) => Some(Some(text.clone())),
        _ => None,
    };
    Some(CodexResetCredit {
        id,
        reset_type: string("reset_type"),
        status: string("status"),
        granted_at: string("granted_at"),
        expires_at: string("expires_at"),
        redeem_started_at: nullable("redeem_started_at"),
        redeemed_at: nullable("redeemed_at"),
        title: string("title"),
        description: string("description"),
    })
}

pub fn parse_codex_reset_credit_list(payload: &Value) -> Option<CodexResetCreditList> {
    let record = payload.as_object()?;
    let credits: Vec<_> =
        record.get("credits").and_then(Value::as_array).into_iter().flatten().filter_map(parse_credit).collect();
    let available_count =
        record.get("available_count").and_then(to_number).map(|count| count.trunc().max(0.0)).unwrap_or_else(|| {
            credits.iter().filter(|credit| credit.status.as_deref().unwrap_or("available") == "available").count()
                as f64
        });
    Some(CodexResetCreditList { credits, available_count })
}

/// Live detail count always replaces the possibly stale usage count. Detail
/// transport failure leaves the original count and report usable.
pub fn enrich_codex_reset_credit_details(report: &mut UsageReport, list: Option<&CodexResetCreditList>) {
    let (Some(credits), Some(list)) = (report.reset_credits.as_mut(), list) else {
        return;
    };
    if !list.credits.is_empty() {
        credits.credits = Some(
            list.credits
                .iter()
                .filter(|credit| credit.status.as_deref().unwrap_or("available") == "available")
                .map(|credit| UsageResetCreditDetail {
                    granted_at: credit.granted_at.clone(),
                    expires_at: credit.expires_at.clone(),
                    status: credit.status.clone(),
                    ..Default::default()
                })
                .collect(),
        );
    }
    credits.available_count = list.available_count;
}

/// Explicit callback for controlled host fixtures. Production builds expose no
/// general endpoint override, and callback results are checked for loopback.
#[cfg(any(test, feature = "test-fixture"))]
pub type CodexUsageFixtureEndpoint = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Safe categories for host cache policy. In particular, main usage 401/403
/// must not be confused with a transient outage that may retain last-good data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexUsageError {
    Cancelled,
    Unauthorized,
    Forbidden,
    Transient,
    InvalidResponse,
}

#[derive(Clone)]
pub struct CodexUsageProvider {
    client: reqwest::Client,
    #[cfg(any(test, feature = "test-fixture"))]
    fixture_endpoint: Option<CodexUsageFixtureEndpoint>,
}

impl CodexUsageProvider {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build()?,
            #[cfg(any(test, feature = "test-fixture"))]
            fixture_endpoint: None,
        })
    }

    #[cfg(any(test, feature = "test-fixture"))]
    pub fn with_fixture_endpoint_resolver(callback: CodexUsageFixtureEndpoint) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().build()?,
            fixture_endpoint: Some(callback),
        })
    }

    pub fn supports(&self, provider: &str, credential: &UsageCredential) -> bool {
        provider == "openai-codex" && credential.credential_type == UsageCredentialType::Oauth
    }

    fn endpoint(&self, canonical: &str) -> Option<reqwest::Url> {
        #[cfg(any(test, feature = "test-fixture"))]
        if let Some(callback) = &self.fixture_endpoint {
            let url = reqwest::Url::parse(&callback(canonical)).ok()?;
            let loopback = url.host_str().is_some_and(|host| {
                host == "localhost"
                    || host
                        .trim_matches(['[', ']'])
                        .parse::<std::net::IpAddr>()
                        .is_ok_and(|address| address.is_loopback())
            });
            if !loopback
                || !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return None;
            }
            return Some(url);
        }
        reqwest::Url::parse(canonical).ok()
    }

    async fn get_json_result(
        &self,
        canonical: &str,
        token: &str,
        account_id: Option<&str>,
    ) -> Result<Value, CodexUsageError> {
        let endpoint = self.endpoint(canonical).ok_or(CodexUsageError::InvalidResponse)?;
        let mut request =
            self.client.get(endpoint).bearer_auth(token).header(reqwest::header::USER_AGENT, CODEX_USAGE_USER_AGENT);
        if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let response = request.send().await.map_err(|_| CodexUsageError::Transient)?;
        match response.status() {
            reqwest::StatusCode::UNAUTHORIZED => return Err(CodexUsageError::Unauthorized),
            reqwest::StatusCode::FORBIDDEN => return Err(CodexUsageError::Forbidden),
            status if !status.is_success() => return Err(CodexUsageError::Transient),
            _ => {}
        }
        response.json().await.map_err(|error| {
            if error.is_decode() { CodexUsageError::InvalidResponse } else { CodexUsageError::Transient }
        })
    }

    async fn get_json(&self, canonical: &str, token: &str, account_id: Option<&str>) -> Option<Value> {
        self.get_json_result(canonical, token, account_id).await.ok()
    }

    async fn fetch_usage_inner_result(
        &self,
        credential: &UsageCredential,
        base_url: Option<&str>,
        now_ms: f64,
    ) -> Result<Option<UsageReport>, CodexUsageError> {
        let Some(token) = credential.access_token.as_deref().filter(|token| !token.is_empty()) else {
            return Ok(None);
        };
        let identity = codex_usage_identity(credential);
        let payload = self.get_json_result(&codex_usage_url(base_url), token, identity.account_id.as_deref()).await?;
        let mut report = parse_codex_usage_payload(&payload, credential, now_ms);
        if report.reset_credits.as_ref().is_some_and(|credits| credits.available_count > 0.0) {
            let list = self
                .get_json(&codex_reset_credits_url(base_url), token, identity.account_id.as_deref())
                .await
                .as_ref()
                .and_then(parse_codex_reset_credit_list);
            enrich_codex_reset_credit_details(&mut report, list.as_ref());
        }
        Ok(Some(report))
    }

    /// Mirrors native null on unsupported/expired/transport failures. The host
    /// may separately inspect its cancellation token for an aborted operation.
    /// Token expiry is checked before any network request, without refresh.
    pub async fn fetch_usage(
        &self,
        provider: &str,
        credential: &UsageCredential,
        base_url: Option<&str>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Option<UsageReport> {
        self.fetch_usage_result(provider, credential, base_url, timeout, cancel).await.ok().flatten()
    }

    /// Main usage errors remain visible to host last-good cache policy. A failed
    /// detail GET is ancillary and retains the valid main report. The overall
    /// timeout and cancellation still cover both requests.
    pub async fn fetch_usage_result(
        &self,
        provider: &str,
        credential: &UsageCredential,
        base_url: Option<&str>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Option<UsageReport>, CodexUsageError> {
        if !self.supports(provider, credential) || credential.access_token.as_deref().is_none_or(str::is_empty) {
            return Ok(None);
        }
        let now_ms =
            SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| CodexUsageError::Transient)?.as_millis() as f64;
        if credential.expires_at.is_some_and(|expires| expires <= now_ms) {
            return Ok(None);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(CodexUsageError::Cancelled),
            result = tokio::time::timeout(timeout, self.fetch_usage_inner_result(credential, base_url, now_ms)) =>
                result.unwrap_or(Err(CodexUsageError::Transient)),
        }
    }

    /// Dedicated GET listing entry for host inspection. No consume endpoint or
    /// idempotency key is generated, and no reset can be redeemed by this API.
    pub async fn list_reset_credits(
        &self,
        access_token: &str,
        account_id: Option<&str>,
        base_url: Option<&str>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Option<CodexResetCreditList> {
        let url = codex_reset_credits_url(base_url);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => None,
            result = tokio::time::timeout(timeout, self.get_json(&url, access_token, account_id)) =>
                result.ok().flatten().as_ref().and_then(parse_codex_reset_credit_list),
        }
    }

    /// Fixed openai-codex-reset.ts consume contract. Only the lower-level
    /// caller may supply a retained idempotency UUID; never retry this POST
    /// implicitly. AuthStorage's native facade selects its own account/credit.
    pub async fn consume_reset_credit(
        &self,
        input: CodexResetConsumeRequest<'_>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<CodexResetConsumeResult, CodexResetConsumeError> {
        if cancel.is_cancelled() {
            return Err(CodexResetConsumeError::Cancelled);
        }
        let CodexResetConsumeRequest { access_token, account_id, base_url, credit_id, redeem_request_id } = input;
        let canonical =
            format!("{}/{}", normalize_codex_base_url(base_url).trim_end_matches('/'), RESET_CREDITS_CONSUME_PATH);
        let endpoint = self.endpoint(&canonical).ok_or(CodexResetConsumeError::InvalidEndpoint)?;
        let request_id = redeem_request_id.map(str::to_owned).unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut body = Map::new();
        body.insert("credit_id".into(), Value::String(credit_id.to_owned()));
        body.insert("redeem_request_id".into(), Value::String(request_id.clone()));
        if let Some(account_id) = account_id {
            body.insert("account_id".into(), Value::String(account_id.to_owned()));
        }
        let mut request = self
            .client
            .post(endpoint)
            .bearer_auth(access_token)
            .header(reqwest::header::USER_AGENT, CODEX_USAGE_USER_AGENT)
            .json(&body);
        if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        let operation = async {
            let response = request.send().await.map_err(|_| ())?;
            let status = response.status();
            let raw = response.json::<Value>().await.ok();
            let code = raw
                .as_ref()
                .and_then(|body| body.get("code"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(
                    || if status.is_success() { "reset".into() } else { format!("http_{}", status.as_u16()) },
                );
            Ok::<_, ()>(CodexResetConsumeResult { ok: code == "reset", code, status: status.as_u16(), raw })
        };
        tokio::select! {
            biased;
            result = tokio::time::timeout(timeout, operation) => match result {
                Ok(Ok(result)) => Ok(result),
                _ => Err(CodexResetConsumeError::OutcomeUnknown { redeem_request_id: request_id.clone() }),
            },
            _ = cancel.cancelled() => Err(CodexResetConsumeError::OutcomeUnknown { redeem_request_id: request_id }),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::auth_storage_policy::{
        CODEX_RANKING_STRATEGY, CredentialRankingContext, CredentialRankingStrategy, is_usage_limit_exhausted,
    };
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::mpsc;

    const NOW: f64 = 1_800_000_000_000.0;

    fn credential() -> UsageCredential {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "https://api.openai.com/auth":{"chatgpt_account_id":"acct-fixture"},
                "https://api.openai.com/profile":{"email":"  Fixture@Example.com  "}
            }))
            .unwrap(),
        );
        serde_json::from_value(json!({"type":"oauth","accessToken":format!("header.{body}.sig")})).unwrap()
    }

    fn payload() -> Value {
        json!({
            "plan_type":"pro",
            "rate_limit":{"allowed":true,"limit_reached":false,
                "primary_window":{"used_percent":4,"limit_window_seconds":17940,"reset_at":2_000_000_000u64},
                "secondary_window":{"used_percent":1,"limit_window_seconds":604740,"reset_at":2_000_500_000u64}},
            "additional_rate_limits":[{"limit_name":"GPT-5.3-Codex-Spark","metered_feature":"codex_bengalfox",
                "rate_limit":{"allowed":true,"limit_reached":false,
                    "primary_window":{"used_percent":17,"limit_window_seconds":18000,"reset_at":2_000_001_000u64},
                    "secondary_window":{"used_percent":61,"limit_window_seconds":604800,"reset_at":2_000_600_000u64}}}],
            "future_field":{"retained":"raw"}
        })
    }

    pub(crate) struct HttpReply {
        pub(crate) status: u16,
        pub(crate) body: String,
        pub(crate) location: Option<String>,
        pub(crate) delay: Duration,
    }

    impl HttpReply {
        pub(crate) fn json(status: u16, body: Value) -> Self {
            Self { status, body: body.to_string(), location: None, delay: Duration::ZERO }
        }
    }

    pub(crate) struct HttpRequest {
        pub(crate) method: String,
        pub(crate) target: String,
        pub(crate) headers: BTreeMap<String, String>,
        pub(crate) body: Option<Value>,
    }

    pub(crate) struct HttpFixture {
        pub(crate) base: String,
        pub(crate) requests: mpsc::UnboundedReceiver<HttpRequest>,
        worker: tokio::task::JoinHandle<()>,
    }

    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.worker.abort();
        }
    }

    pub(crate) async fn fixture(replies: Vec<HttpReply>) -> HttpFixture {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::unbounded_channel();
        let worker = tokio::spawn(async move {
            let mut replies: VecDeque<_> = replies.into();
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut bytes = Vec::new();
                while !bytes.windows(4).any(|window| window == b"\r\n\r\n") && bytes.len() < 64 * 1024 {
                    let mut buffer = [0u8; 4096];
                    let Ok(length) = stream.read(&mut buffer).await else {
                        break;
                    };
                    if length == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..length]);
                }
                let header_end = bytes.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
                let text = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
                let mut lines = text.split("\r\n");
                let first = lines.next().unwrap();
                let method = first.split_whitespace().next().unwrap().to_owned();
                let target = first.split_whitespace().nth(1).unwrap().to_owned();
                let headers: BTreeMap<String, String> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(key, value)| (key.to_lowercase(), value.trim().into()))
                    .collect();
                let length = headers.get("content-length").and_then(|length| length.parse::<usize>().ok()).unwrap_or(0);
                while bytes.len() < header_end + length {
                    let mut buffer = [0u8; 4096];
                    let read = stream.read(&mut buffer).await.unwrap();
                    if read == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buffer[..read]);
                }
                let body = serde_json::from_slice(&bytes[header_end..]).ok();
                if sender.send(HttpRequest { method, target, headers, body }).is_err() {
                    break;
                }
                let reply = replies.pop_front().unwrap_or_else(|| HttpReply::json(500, json!({})));
                if !reply.delay.is_zero() {
                    tokio::time::sleep(reply.delay).await;
                }
                let location = reply.location.map(|url| format!("Location: {url}\r\n")).unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 {} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n{}",
                    reply.status,
                    reply.body.len(),
                    location,
                    reply.body
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        HttpFixture { base, requests, worker }
    }

    pub(crate) fn fixture_provider(base: &str, canonical_calls: Arc<Mutex<Vec<String>>>) -> CodexUsageProvider {
        let base = base.to_owned();
        CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |canonical| {
            canonical_calls.lock().unwrap().push(canonical.to_owned());
            let path = reqwest::Url::parse(canonical).unwrap().path().to_owned();
            format!("{base}{path}")
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn native_reset_consume_wire_and_picker_case_corpus() {
        let credits = parse_codex_reset_credit_list(&json!({"credits":[
            {"id":"spent","status":"redeemed","expires_at":"2026-01-01T00:00:00Z"},
            {"id":"undated","expires_at":"invalid"},
            {"id":"late","expires_at":"2026-12-01T00:00:00Z"},
            {"id":"soon","expires_at":"2026-11-01T00:00:00Z"},
            {"id":"tie","expires_at":"2026-11-01T00:00:00Z"}
        ]}))
        .unwrap()
        .credits;
        assert_eq!(pick_soonest_expiring_credit(&credits).unwrap().id, "soon");
        assert_eq!(pick_soonest_expiring_credit(&credits[..2]).unwrap().id, "undated");
        assert_eq!(pick_soonest_expiring_credit(&credits[..1]).unwrap().id, "spent");
        assert!(pick_soonest_expiring_credit(&[]).is_none());
        for (status, body, expected_code, expected_ok) in [
            (200, "{\"code\":\"reset\"}", "reset", true),
            (500, "{\"code\":\"reset\"}", "reset", true),
            (200, "{\"code\":\"nothing_to_reset\"}", "nothing_to_reset", false),
            (200, "broken", "reset", true),
            (403, "broken", "http_403", false),
            (200, "{\"code\":7}", "reset", true),
            (200, "{\"code\":\"future_business_code\"}", "future_business_code", false),
        ] {
            let mut http =
                fixture(vec![HttpReply { status, body: body.into(), location: None, delay: Duration::ZERO }]).await;
            let client = fixture_provider(&http.base, Arc::new(Mutex::new(Vec::new())));
            let result = client
                .consume_reset_credit(
                    CodexResetConsumeRequest {
                        access_token: "private-fixture",
                        account_id: Some("account-fixture"),
                        base_url: None,
                        credit_id: "credit-fixture",
                        redeem_request_id: Some("retained-uuid"),
                    },
                    Duration::from_secs(2),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            assert_eq!((result.ok, result.code.as_str(), result.status), (expected_ok, expected_code, status));
            let request = http.requests.recv().await.unwrap();
            assert_eq!(request.method, "POST");
            assert_eq!(request.target, "/backend-api/wham/rate-limit-reset-credits/consume");
            assert_eq!(request.headers["user-agent"], CODEX_USAGE_USER_AGENT);
            assert_eq!(request.headers["authorization"], "Bearer private-fixture");
            assert_eq!(request.headers["chatgpt-account-id"], "account-fixture");
            assert_eq!(
                request.body.unwrap(),
                json!({"credit_id":"credit-fixture","redeem_request_id":"retained-uuid","account_id":"account-fixture"})
            );
            assert!(http.requests.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn native_reset_consume_cancel_timeout_and_new_uuid_case_corpus() {
        let mut http = fixture(vec![HttpReply::json(200, json!({})), HttpReply::json(200, json!({}))]).await;
        let client = fixture_provider(&http.base, Arc::new(Mutex::new(Vec::new())));
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let input = CodexResetConsumeRequest {
            access_token: "fixture",
            account_id: None,
            base_url: None,
            credit_id: "credit",
            redeem_request_id: None,
        };
        assert!(matches!(
            client.consume_reset_credit(input, Duration::from_secs(1), &cancelled).await,
            Err(CodexResetConsumeError::Cancelled)
        ));
        assert!(http.requests.try_recv().is_err());
        let mut ids = Vec::new();
        for _ in 0..2 {
            client
                .consume_reset_credit(
                    CodexResetConsumeRequest {
                        access_token: "fixture",
                        account_id: None,
                        base_url: None,
                        credit_id: "credit",
                        redeem_request_id: None,
                    },
                    Duration::from_secs(1),
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            let request = http.requests.recv().await.unwrap();
            let body = request.body.unwrap();
            assert!(body.get("account_id").is_none());
            let id = body["redeem_request_id"].as_str().unwrap().to_owned();
            assert!(uuid::Uuid::parse_str(&id).is_ok());
            ids.push(id);
        }
        assert_ne!(ids[0], ids[1]);
        for cancel_after_dispatch in [false, true] {
            let mut http = fixture(vec![HttpReply {
                status: 200,
                body: "{}".into(),
                location: None,
                delay: Duration::from_secs(2),
            }])
            .await;
            let client = fixture_provider(&http.base, Arc::new(Mutex::new(Vec::new())));
            let cancel = CancellationToken::new();
            let worker_cancel = cancel.clone();
            let worker = tokio::spawn(async move {
                client
                    .consume_reset_credit(
                        CodexResetConsumeRequest {
                            access_token: "fixture",
                            account_id: None,
                            base_url: None,
                            credit_id: "credit",
                            redeem_request_id: Some("known-uuid"),
                        },
                        Duration::from_millis(150),
                        &worker_cancel,
                    )
                    .await
            });
            assert_eq!(http.requests.recv().await.unwrap().method, "POST");
            if cancel_after_dispatch {
                cancel.cancel();
            }
            assert!(
                matches!(worker.await.unwrap(), Err(CodexResetConsumeError::OutcomeUnknown { redeem_request_id }) if redeem_request_id == "known-uuid")
            );
            assert!(http.requests.try_recv().is_err());
        }
    }

    // One family translates fixed native openai-codex-usage.test.ts,
    // openai-codex-reset.test.ts list cases and the header contract in
    // openai-codex.ts:370-393. Controlled real HTTP is separate from live-model
    // acceptance; no native Bun execution is claimed by these Rust tests.
    #[tokio::test]
    async fn native_codex_usage_fixture_family() {
        let mut cred = credential();
        assert_eq!(
            codex_usage_identity(&cred),
            CodexUsageIdentity { account_id: Some("acct-fixture".into()), email: Some("fixture@example.com".into()) }
        );
        cred.account_id = Some("acct-override".into());
        cred.email = Some(" Override@Example.com ".into());
        assert_eq!(codex_usage_identity(&cred).email.as_deref(), Some("override@example.com"));
        let base = payload();
        let parsed = parse_codex_usage_payload(&base, &cred, NOW);
        assert_eq!(parsed.raw, Some(base.clone()));
        assert_eq!(
            parsed.limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(),
            [
                "openai-codex:primary",
                "openai-codex:secondary",
                "openai-codex:spark:primary",
                "openai-codex:spark:secondary"
            ]
        );
        assert_eq!(parsed.limits[0].label, "5 hours");
        assert_eq!(parsed.limits[1].label, "7 days");
        assert_eq!(parsed.limits[2].label, "5 hours (Spark)");
        assert_eq!(parsed.limits[3].label, "7 days (Spark)");
        assert_eq!(parsed.limits[0].scope.account_id, None);
        assert_eq!(parsed.limits[2].scope.account_id.as_deref(), Some("acct-override"));
        assert_eq!(parsed.limits[2].scope.model_id.as_deref(), Some("GPT-5.3-Codex-Spark"));
        assert_eq!(parsed.limits[0].amount.used_fraction, Some(0.04));
        assert_eq!(parsed.limits[2].amount.used_fraction, Some(0.17));
        assert_eq!(parsed.limits[0].window.as_ref().unwrap().resets_at, Some(2_000_000_000_000.0));
        assert_eq!(
            parsed.metadata.as_ref().unwrap()["meterStates"]["spark"],
            json!({"allowed":true,"limitReached":false})
        );

        struct ParserCase {
            source: &'static str,
            payload: Value,
            ids: Vec<&'static str>,
            status: Vec<UsageStatus>,
        }
        let mut team = base.clone();
        team["plan_type"] = json!("team");
        team["rate_limit"]["secondary_window"]["used_percent"] = json!(100);
        let mut rejection = base.clone();
        rejection["rate_limit"]["allowed"] = json!(false);
        rejection["rate_limit"]["limit_reached"] = json!(true);
        rejection["rate_limit"]["primary_window"]["used_percent"] = json!(100);
        let mut codename = base.clone();
        codename["additional_rate_limits"][0].as_object_mut().unwrap().remove("limit_name");
        let cases = vec![
            ParserCase {
                source: "explicitly allowed Team at 100%",
                payload: team,
                ids: vec![
                    "openai-codex:primary",
                    "openai-codex:secondary",
                    "openai-codex:spark:primary",
                    "openai-codex:spark:secondary",
                ],
                status: vec![UsageStatus::Ok, UsageStatus::Warning, UsageStatus::Ok, UsageStatus::Ok],
            },
            ParserCase {
                source: "shared rejection is window-local",
                payload: rejection,
                ids: vec![
                    "openai-codex:primary",
                    "openai-codex:secondary",
                    "openai-codex:spark:primary",
                    "openai-codex:spark:secondary",
                ],
                status: vec![UsageStatus::Exhausted, UsageStatus::Ok, UsageStatus::Ok, UsageStatus::Ok],
            },
            ParserCase {
                source: "bengalfox without limit_name",
                payload: codename,
                ids: vec![
                    "openai-codex:primary",
                    "openai-codex:secondary",
                    "openai-codex:spark:primary",
                    "openai-codex:spark:secondary",
                ],
                status: vec![UsageStatus::Ok, UsageStatus::Ok, UsageStatus::Ok, UsageStatus::Ok],
            },
            ParserCase {
                source: "additional-only usage",
                payload: json!({"additional_rate_limits":[{
                "metered_feature":"codex_bengalfox","rate_limit":{"primary_window":{"used_percent":5,"limit_window_seconds":18000}}}]}),
                ids: vec!["openai-codex:spark:primary"],
                status: vec![UsageStatus::Ok],
            },
            ParserCase {
                source: "unknown percentage with a reset clock",
                payload: json!({"rate_limit":{"primary_window":{"reset_after_seconds":"30"}}}),
                ids: vec!["openai-codex:primary"],
                status: vec![UsageStatus::Unknown],
            },
            ParserCase {
                source: "unrecognized payload remains a report",
                payload: json!({"plan_type":"go","future":true}),
                ids: vec![],
                status: vec![],
            },
            ParserCase {
                source: "non-object successful JSON remains raw",
                payload: json!([1, "opaque"]),
                ids: vec![],
                status: vec![],
            },
            ParserCase {
                source: "strict booleans and numeric string coercion",
                payload: json!({"rate_limit":{"allowed":"true","limit_reached":0,
                "primary_window":{"used_percent":"0x64","limit_window_seconds":"3600","reset_at":1_900_000_000_001u64}}}),
                ids: vec!["openai-codex:primary"],
                status: vec![UsageStatus::Exhausted],
            },
        ];
        for case in cases {
            let report = parse_codex_usage_payload(&case.payload, &cred, NOW);
            assert_eq!(
                report.limits.iter().map(|limit| limit.id.as_str()).collect::<Vec<_>>(),
                case.ids,
                "{}",
                case.source
            );
            assert_eq!(
                report.limits.iter().map(|limit| limit.status.unwrap()).collect::<Vec<_>>(),
                case.status,
                "{}",
                case.source
            );
            assert_eq!(report.raw, Some(case.payload), "{}", case.source);
        }
        let time_payload = json!({"rate_limit":{"primary_window":{"reset_at":"bad","reset_after_seconds":"-2"},
            "secondary_window":{"used_percent":140,"reset_at":1_900_000_000_001u64}}});
        let timed = parse_codex_usage_payload(&time_payload, &cred, NOW);
        assert_eq!(timed.limits[0].window.as_ref().unwrap().resets_at, Some(NOW - 2000.0));
        assert_eq!(timed.limits[0].label, "Primary window");
        assert_eq!(timed.limits[1].window.as_ref().unwrap().resets_at, Some(1_900_000_000_001.0));
        assert_eq!(timed.limits[1].amount.used_fraction, Some(1.0));
        assert_eq!(additional_limit_slug(None, Some("codex_NEW_meter")), "new-meter");
        assert_eq!(additional_limit_slug(Some(" Spark "), Some("anything")), "spark");
        assert_eq!(additional_limit_slug(Some("fallback"), Some("")), "extra");
        assert_eq!(additional_display_name("new-3meter-thing", None), "New-3meter Thing");
        let spark_ctx = CredentialRankingContext { model_id: Some("gpt-5.3-codex-spark".into()) };
        assert_eq!(CODEX_RANKING_STRATEGY.scope_limits(&parsed, Some(&spark_ctx)).len(), 2);

        for (input, expected) in [
            (None, CODEX_BASE_URL),
            (Some("bad"), CODEX_BASE_URL),
            (Some("https://proxy.example/codex/responses"), CODEX_BASE_URL),
            (Some(" https://CHATGPT.com/backend-api/codex/responses/?ignored=1#fragment "), CODEX_BASE_URL),
            (Some("https://chatgpt.com:443/anything"), CODEX_BASE_URL),
            (Some("https://chatgpt.com:444/anything"), CODEX_BASE_URL),
            (Some("https://chat.openai.com/stream/responses"), "https://chat.openai.com/backend-api"),
            (Some("http://chatgpt.com/path"), "http://chatgpt.com/backend-api"),
        ] {
            assert_eq!(normalize_codex_base_url(input), expected);
        }
        let headers = BTreeMap::from([
            ("x-codex-primary-used-percent".into(), "100".into()),
            ("x-codex-primary-window-minutes".into(), "300".into()),
            ("x-codex-primary-reset-at".into(), "2000000000".into()),
            ("x-codex-secondary-used-percent".into(), "bad".into()),
        ]);
        let header_report = parse_codex_rate_limit_headers(&headers, NOW).unwrap();
        assert_eq!(header_report.limits.len(), 1);
        assert!(is_usage_limit_exhausted(&header_report.limits[0]));
        assert_eq!(header_report.metadata.as_ref().unwrap()["source"], "ratelimit-headers");
        assert!(parse_codex_rate_limit_headers(&BTreeMap::new(), NOW).is_none());

        let list = parse_codex_reset_credit_list(&json!({"credits":[
            {"id":"available","granted_at":"2026-06-01","expires_at":"2026-08-12","redeem_started_at":null},
            {"id":"spent","status":"redeemed","redeemed_at":"2026-07-01"}, {"id":""},false,
        ]}))
        .unwrap();
        assert_eq!(list.available_count, 1.0);
        assert_eq!(list.credits.len(), 2);
        assert_eq!(list.credits[0].redeem_started_at, Some(None));
        assert_eq!(list.credits[0].redeemed_at, None);
        assert_eq!(serde_json::to_value(&list.credits[0]).unwrap()["redeemStartedAt"], Value::Null);
        assert_eq!(parse_codex_reset_credit_list(&json!({"available_count":"-2.8"})).unwrap().available_count, 0.0);
        assert_eq!(
            parse_codex_usage_reset_credits(&json!({"rate_limit_reset_credits":{"available_count":"3.9"}}))
                .unwrap()
                .available_count,
            3.0
        );
        assert!(parse_codex_reset_credit_list(&json!([])).is_none());

        // Real request family: both routes inherit bearer/account identity and
        // User-Agent, provider proxy settings still normalize before injection.
        let mut with_credits = base.clone();
        with_credits["rate_limit_reset_credits"] = json!({"available_count":3});
        let mut http = fixture(vec![
            HttpReply::json(200, with_credits.clone()),
            HttpReply::json(
                200,
                json!({
                    "available_count":0,"credits":[{"id":"spent","status":"redeemed","expires_at":"2026-08-12"}]
                }),
            ),
        ])
        .await;
        let calls = Arc::new(Mutex::new(Vec::new()));
        let provider = fixture_provider(&http.base, calls.clone());
        let fetched = provider
            .fetch_usage(
                "openai-codex",
                &cred,
                Some("https://proxy.example/responses"),
                Duration::from_secs(2),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(fetched.reset_credits.as_ref().unwrap().available_count, 0.0);
        assert_eq!(fetched.reset_credits.as_ref().unwrap().credits, Some(Vec::new()));
        assert_eq!(fetched.raw, Some(with_credits.clone()));
        let requests = [http.requests.recv().await.unwrap(), http.requests.recv().await.unwrap()];
        assert_eq!(
            requests.iter().map(|request| request.target.as_str()).collect::<Vec<_>>(),
            ["/backend-api/wham/usage", "/backend-api/wham/rate-limit-reset-credits"]
        );
        for request in requests {
            assert_eq!(request.headers["user-agent"], CODEX_USAGE_USER_AGENT);
            assert_eq!(request.headers["chatgpt-account-id"], "acct-override");
            assert_eq!(request.headers["authorization"], format!("Bearer {}", cred.access_token.as_deref().unwrap()));
        }
        assert_eq!(*calls.lock().unwrap(), vec![codex_usage_url(None), codex_reset_credits_url(None)]);
        assert!(http.requests.try_recv().is_err());
        drop(http);

        for detail_failure in [
            HttpReply::json(401, json!({})),
            HttpReply::json(403, json!({})),
            HttpReply { status: 200, body: "not JSON".into(), location: None, delay: Duration::ZERO },
        ] {
            let mut fail_detail = fixture(vec![HttpReply::json(200, with_credits.clone()), detail_failure]).await;
            let provider = fixture_provider(&fail_detail.base, Arc::new(Mutex::new(Vec::new())));
            let retained = provider
                .fetch_usage_result("openai-codex", &cred, None, Duration::from_secs(2), &CancellationToken::new())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(retained.reset_credits.as_ref().unwrap().available_count, 3.0);
            assert_eq!(retained.reset_credits.as_ref().unwrap().credits, None);
            assert_eq!(retained.raw, Some(with_credits.clone()));
            assert!(fail_detail.requests.recv().await.is_some());
            assert!(fail_detail.requests.recv().await.is_some());
        }

        let mut zero_payload = base.clone();
        zero_payload["rate_limit_reset_credits"] = json!({"available_count":0});
        let mut zero = fixture(vec![HttpReply::json(200, zero_payload)]).await;
        let provider = fixture_provider(&zero.base, Arc::new(Mutex::new(Vec::new())));
        assert!(
            provider
                .fetch_usage("openai-codex", &cred, None, Duration::from_secs(2), &CancellationToken::new())
                .await
                .is_some()
        );
        assert!(zero.requests.recv().await.is_some());
        assert!(zero.requests.try_recv().is_err());
        drop(zero);

        for (reply, expected) in [
            (HttpReply::json(401, json!({})), CodexUsageError::Unauthorized),
            (HttpReply::json(403, json!({})), CodexUsageError::Forbidden),
            (HttpReply::json(500, json!({})), CodexUsageError::Transient),
            (
                HttpReply { status: 200, body: "not JSON".into(), location: None, delay: Duration::ZERO },
                CodexUsageError::InvalidResponse,
            ),
            (
                HttpReply { status: 302, body: "{}".into(), location: Some("/capture".into()), delay: Duration::ZERO },
                CodexUsageError::Transient,
            ),
        ] {
            let mut failure = fixture(vec![reply]).await;
            let provider = fixture_provider(&failure.base, Arc::new(Mutex::new(Vec::new())));
            assert_eq!(
                provider
                    .fetch_usage_result("openai-codex", &cred, None, Duration::from_secs(2), &CancellationToken::new())
                    .await
                    .err(),
                Some(expected)
            );
            assert!(failure.requests.recv().await.is_some());
            assert!(failure.requests.try_recv().is_err());
        }
        let callback_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = callback_count.clone();
        let invalid_endpoint = CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |_| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            "https://non-loopback.example/usage".into()
        }))
        .unwrap();
        let mut expired = cred.clone();
        expired.expires_at = Some(0.0);
        let mut api_key = cred.clone();
        api_key.credential_type = UsageCredentialType::ApiKey;
        let mut missing = cred.clone();
        missing.access_token = None;
        for (provider, credential) in
            [("openai", &cred), ("openai-codex", &expired), ("openai-codex", &api_key), ("openai-codex", &missing)]
        {
            assert!(matches!(
                invalid_endpoint
                    .fetch_usage_result(provider, credential, None, Duration::from_secs(1), &CancellationToken::new())
                    .await,
                Ok(None)
            ));
        }
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert_eq!(
            invalid_endpoint
                .fetch_usage_result("openai-codex", &cred, None, Duration::from_secs(1), &cancelled)
                .await
                .err(),
            Some(CodexUsageError::Cancelled)
        );
        assert_eq!(callback_count.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            invalid_endpoint
                .fetch_usage_result("openai-codex", &cred, None, Duration::from_secs(1), &CancellationToken::new())
                .await
                .err(),
            Some(CodexUsageError::InvalidResponse)
        );
        assert_eq!(callback_count.load(std::sync::atomic::Ordering::SeqCst), 1);

        let mut delayed = HttpReply::json(200, base.clone());
        delayed.delay = Duration::from_secs(5);
        let mut cancel_http = fixture(vec![delayed]).await;
        let provider = fixture_provider(&cancel_http.base, Arc::new(Mutex::new(Vec::new())));
        let token = CancellationToken::new();
        let worker_token = token.clone();
        let worker_cred = cred.clone();
        let request = tokio::spawn(async move {
            provider.fetch_usage_result("openai-codex", &worker_cred, None, Duration::from_secs(2), &worker_token).await
        });
        assert!(cancel_http.requests.recv().await.is_some());
        token.cancel();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), request).await.unwrap().unwrap().err(),
            Some(CodexUsageError::Cancelled)
        );
        drop(cancel_http);

        let mut delayed_detail = HttpReply::json(200, json!({"available_count":0,"credits":[]}));
        delayed_detail.delay = Duration::from_secs(5);
        let mut credit_payload = base;
        credit_payload["rate_limit_reset_credits"] = json!({"available_count":1});
        let timeout_http = fixture(vec![HttpReply::json(200, credit_payload), delayed_detail]).await;
        let provider = fixture_provider(&timeout_http.base, Arc::new(Mutex::new(Vec::new())));
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(1),
                provider.fetch_usage_result(
                    "openai-codex",
                    &cred,
                    None,
                    Duration::from_millis(150),
                    &CancellationToken::new()
                )
            )
            .await
            .unwrap()
            .err(),
            Some(CodexUsageError::Transient)
        );
    }
}
