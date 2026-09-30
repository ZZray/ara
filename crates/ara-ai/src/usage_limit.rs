//! Account usage limits that must not be replayed by the provider stream.
//!
//! Adapted from fixed OMP `packages/ai/src/error/{rate-limit,flags,retryable}.ts`
//! and `packages/utils/src/fetch-retry.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT). This private
//! classifier governs only the outer OpenAI Chat replay; credential rotation
//! and the inner HTTP retry policy belong to other layers.

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

macro_rules! regex {
    ($name:ident, $pattern:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).expect("fixed usage-limit pattern"));
    };
}

regex!(ACCOUNT_RATE_LIMIT, r"(?i)\baccount(?:'s)?\b[^\n]{0,80}\brate.?limit\b|\brate.?limit\b[^\n]{0,80}\baccount\b");
regex!(
    CREDITS_EXHAUSTED,
    r"(?i)\b(?:exceed\w*|insufficient|not enough)\b[^\n]{0,40}\bcredits?\b|\bcredits?\b[^\n]{0,40}\b(?:exhausted|depleted)\b"
);
regex!(
    SUBSCRIPTION_CAP,
    r"(?i)\b(?:subscription|plan|membership)\b[^\n]{0,80}\b(?:rate.?limits?|quota|cap)\b|\b(?:rate.?limits?|quota|cap)\b[^\n]{0,80}\b(?:subscription|plan|membership)\b"
);
regex!(
    CONCURRENT_CAP,
    r"(?i)\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"
);
regex!(
    ACCOUNT_SCOPED,
    r"(?i)\b(?:overall|account|organization|team|workspace)\b[^\n]{0,40}\b(?:message |request )?rate.?limit\b|\byour\b[^\n]{0,30}\b(?:limit )?will reset\b"
);
regex!(OPENROUTER_FREE_DAY, r"(?i)\bfree[-_ ]models[-_ ]per[-_ ]day\b");
regex!(CLINE_CAP, r"(?i)clinepass limit|free limit reached on model");
regex!(
    USAGE_LIMIT,
    r"(?i)usage.?limit|usage_limit_reached|usage_not_included|limit_reached|quota.?(?:exceeded|reached|insufficient)|额度不足|额度耗尽|resource.?exhausted|exhausted your capacity|quota will reset|insufficient.?(?:balance|quota)|balance.?exhausted|run out of credits|out of credits|spending[- _]?limit|personal-team-blocked|clinepass limit|free limit reached on model"
);
regex!(CN_QUOTA, r"使用.{0,30}?上限|(?:额度|配额)已?(?:用|耗)(?:完|尽)|限额.{0,30}重置|余额不足");
regex!(
    CN_TRANSIENT,
    r"速率.{0,30}上限|频率.{0,30}上限|每分钟.{0,30}上限|并发.{0,30}上限|使用.{0,30}(?:速率|频率|每分钟|并发).{0,30}上限"
);
regex!(CN_THROTTLE, r"速率(?:限制|过快)|频率(?:过高|过快)|过于频繁|稍后[重再]试");
regex!(DASHSCOPE_ANCHOR, r"(?i)error-code[^()\s]*#token-limit");
regex!(DASHSCOPE_WORDING, r"(?i)\byou exceeded your current quota, please check your plan and billing details\b");
regex!(STATUS_FRAMING, r"(?i)\b(?:429|402|http|https|status|error|code|response|message)\b|\(?\bno body\b\)?");
regex!(INFORMATIVE_LATIN, r"(?i)[a-z\d]{3,}");
regex!(PAYMENT_CAP, r"(?i)\b(?:payment(?:\s+is)?[-_.\s]*required|deactivated_workspace|insufficient.?balance)\b");
regex!(SPEND_LIMIT, r"(?i)spend.?limit");
regex!(RESET_AFTER, r"(?i)reset after (?:(\d+)h)?(?:(\d+)m)?(\d+(?:\.\d+)?)s");
regex!(
    RESET_AT,
    r"(?i)(?:will\s+)?reset at\s+([0-9]{4}-[0-9]{2}-[0-9]{2}[ T][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)"
);
regex!(CN_RESET_AT, r"将在\s*([0-9]{4}-[0-9]{2}-[0-9]{2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2})\s*重置");
regex!(RESET_IN, r"(?i)(?:will\s+)?reset in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b");
regex!(RETRY_AFTER_MS, r"(?i)\bretry-after-ms=([0-9]+)\b");
regex!(PLEASE_RETRY, r"(?i)please retry in ([0-9.]+)(ms|s)");
regex!(RETRY_DELAY_FIELD, r#"(?i)"retryDelay":\s*"([0-9.]+)(ms|s)""#);
regex!(TRY_AGAIN, r"(?i)try again in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)\b");

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reason {
    Quota,
    Rate,
    Concurrent,
    Capacity,
    Server,
    Unknown,
}

fn dashscope_token_throttle(message: &str) -> bool {
    DASHSCOPE_ANCHOR.is_match(message) && DASHSCOPE_WORDING.is_match(message)
}

fn chinese_quota(message: &str) -> bool {
    CN_QUOTA.is_match(message) && !CN_TRANSIENT.is_match(message)
}

fn subscription_cap(message: &str) -> bool {
    SUBSCRIPTION_CAP.is_match(message)
        && !message.to_ascii_lowercase().contains("per minute")
        && !message.to_ascii_lowercase().contains("per second")
}

fn reason(message: &str) -> Reason {
    let lower = message.to_ascii_lowercase();
    if lower.contains("quota will reset") || lower.contains("exhausted your capacity") || chinese_quota(message) {
        return Reason::Quota;
    }
    if dashscope_token_throttle(message) {
        return Reason::Rate;
    }
    if CONCURRENT_CAP.is_match(message) {
        return Reason::Concurrent;
    }
    if ["capacity", "overloaded", "529", "503"].iter().any(|part| lower.contains(part)) {
        return Reason::Capacity;
    }
    if ACCOUNT_RATE_LIMIT.is_match(message)
        || SPEND_LIMIT.is_match(message)
        || subscription_cap(message)
        || OPENROUTER_FREE_DAY.is_match(message)
        || CLINE_CAP.is_match(message)
    {
        return Reason::Quota;
    }
    if ["per minute", "rate limit", "too many requests", "presque"].iter().any(|part| lower.contains(part)) {
        return Reason::Rate;
    }
    if ["exhausted", "quota", "usage limit", "run out of credits", "out of credits", "spending-limit", "spending limit"]
        .iter()
        .any(|part| lower.contains(part))
        || CREDITS_EXHAUSTED.is_match(message)
    {
        return Reason::Quota;
    }
    if lower.contains("500") || lower.contains("internal error") || lower.contains("internal server error") {
        return Reason::Server;
    }
    Reason::Unknown
}

fn matches_usage_text(message: &str) -> bool {
    !dashscope_token_throttle(message)
        && (USAGE_LIMIT.is_match(message)
            || CREDITS_EXHAUSTED.is_match(message)
            || chinese_quota(message)
            || SPEND_LIMIT.is_match(message)
            || ACCOUNT_RATE_LIMIT.is_match(message)
            || subscription_cap(message)
            || OPENROUTER_FREE_DAY.is_match(message))
}

fn opaque_status_body(body: &str) -> bool {
    let cleaned = STATUS_FRAMING.replace_all(body, "");
    !INFORMATIVE_LATIN.is_match(&cleaned)
        && !CN_QUOTA.is_match(&cleaned)
        && !CN_TRANSIENT.is_match(&cleaned)
        && !CN_THROTTLE.is_match(&cleaned)
}

fn parsed_embedded_json(text: &str) -> Option<Value> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok()
}

fn unit_ms(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "ms" => Some(1.0),
        "s" | "sec" => Some(1_000.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60_000.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3_600_000.0),
        _ => None,
    }
}

fn absolute_reset_ms(body: &str) -> Option<f64> {
    for pattern in [&*RESET_AT, &*CN_RESET_AT] {
        let Some(parts) = pattern.captures(body) else { continue };
        let mut value = parts.get(1)?.as_str().replace(' ', "T");
        let has_offset = value.ends_with('Z')
            || value.ends_with('z')
            || value.rfind(['+', '-']).is_some_and(|position| position >= 19);
        if !has_offset {
            value.push('Z'); // Fixed OMP interprets provider timestamps without offsets as UTC.
        } else if let Some(position) = value.rfind(['+', '-'])
            && position >= 19
            && value.len() - position == 5
        {
            value.insert(position + 3, ':');
        }
        let reset = DateTime::parse_from_rfc3339(&value).ok()?;
        let ms = reset.signed_duration_since(Utc::now()).num_milliseconds();
        if ms > 0 {
            return Some(ms as f64);
        }
    }
    None
}

pub(crate) fn retry_hint_ms(body: &str) -> Option<f64> {
    if let Some(parts) = RESET_AFTER.captures(body) {
        let hours = parts.get(1).and_then(|part| part.as_str().parse::<f64>().ok()).unwrap_or(0.0);
        let minutes = parts.get(2).and_then(|part| part.as_str().parse::<f64>().ok()).unwrap_or(0.0);
        let seconds = parts.get(3)?.as_str().parse::<f64>().ok()?;
        let ms = ((hours * 60.0 + minutes) * 60.0 + seconds) * 1_000.0;
        if ms.is_finite() && ms > 0.0 {
            return Some(ms);
        }
    }
    if let Some(ms) = absolute_reset_ms(body) {
        return Some(ms);
    }
    if let Some(parts) = RESET_IN.captures(body) {
        let ms = parts.get(1)?.as_str().parse::<f64>().ok()? * unit_ms(parts.get(2)?.as_str())?;
        if ms.is_finite() && ms > 0.0 {
            return Some(ms);
        }
    }
    if let Some(parts) = RETRY_AFTER_MS.captures(body) {
        let ms = parts.get(1)?.as_str().parse::<f64>().ok()?;
        if ms.is_finite() && ms > 0.0 {
            return Some(ms);
        }
    }
    for pattern in [&*PLEASE_RETRY, &*RETRY_DELAY_FIELD, &*TRY_AGAIN] {
        if let Some(parts) = pattern.captures(body) {
            let ms = parts.get(1)?.as_str().parse::<f64>().ok()? * unit_ms(parts.get(2)?.as_str())?;
            if ms.is_finite() && ms > 0.0 {
                return Some(ms);
            }
        }
    }
    None
}

fn google_rpc_quota(error: &Value, full_body: &str) -> Option<bool> {
    if !error.get("status")?.as_str()?.trim().eq_ignore_ascii_case("RESOURCE_EXHAUSTED") {
        return None;
    }
    let details = error.get("details")?.as_array()?;
    for detail in details {
        if detail.get("@type").and_then(Value::as_str) != Some("type.googleapis.com/google.rpc.ErrorInfo") {
            continue;
        }
        match detail.get("reason").and_then(Value::as_str).map(|reason| reason.trim().to_ascii_uppercase()).as_deref() {
            Some("QUOTA_EXHAUSTED" | "INSUFFICIENT_G1_CREDITS_BALANCE") => return Some(true),
            Some("RATE_LIMIT_EXCEEDED") => {
                let model_quota = error.get("message").and_then(Value::as_str).is_some_and(|message| {
                    message.to_ascii_lowercase().contains("exhausted your capacity on this model")
                });
                return Some(model_quota || retry_hint_ms(full_body).is_some_and(|ms| ms >= 300_000.0));
            }
            _ => {}
        }
    }
    None
}

fn has_code(error: Option<&Value>, expected: &[&str]) -> bool {
    ["code", "type"]
        .into_iter()
        .filter_map(|key| error?.get(key)?.as_str())
        .any(|code| expected.iter().any(|candidate| code.trim().eq_ignore_ascii_case(candidate)))
}

/// Whether a finalized provider error is an account-local cap rather than a
/// short throttle. `body` is the complete structured envelope when available;
/// `detail` is the user-facing message; `raw_body` distinguishes empty/opaque
/// HTTP responses from the display fallback added by `parse_error_envelope`.
pub(crate) fn account_usage_limit(
    status: Option<u16>,
    body: Option<&Value>,
    detail: &str,
    raw_body: Option<&str>,
) -> bool {
    let parsed = if body.is_none() { raw_body.and_then(parsed_embedded_json) } else { None };
    let error = body.or(parsed.as_ref()).and_then(|value| value.get("error").or(Some(value)));
    let owned_body = body.map(Value::to_string);
    let full_body = raw_body.or(owned_body.as_deref()).unwrap_or(detail);
    if let Some(quota) = error.and_then(|error| google_rpc_quota(error, full_body)) {
        return quota;
    }
    if dashscope_token_throttle(detail) {
        return false;
    }
    let reason = reason(detail);
    if has_code(
        error,
        &[
            "insufficient_quota",
            "usage_limit_reached",
            "usage_limit_exceeded",
            "usage_not_included",
            "quota_exhausted",
            "insufficient_balance",
        ],
    ) {
        return true;
    }
    if status == Some(402) && has_code(error, &["payment_required", "deactivated_workspace"]) {
        return true;
    }
    if reason == Reason::Concurrent && status != Some(402) {
        return false;
    }
    if matches_usage_text(detail) {
        return true;
    }
    if (status == Some(403) || status.is_none()) && ACCOUNT_SCOPED.is_match(detail) {
        return true;
    }
    let raw = raw_body.unwrap_or(detail);
    if status == Some(402) {
        return opaque_status_body(raw)
            || PAYMENT_CAP.is_match(detail)
            || matches!(reason, Reason::Quota | Reason::Concurrent);
    }
    status == Some(429) && (opaque_status_body(raw) || reason == Reason::Quota)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn classify(status: Option<u16>, detail: &str) -> bool {
        account_usage_limit(status, None, detail, Some(detail))
    }

    #[test]
    fn opaque_and_transient_429s() {
        for body in ["", "429", "HTTP 429", "{}"] {
            assert!(classify(Some(429), body), "{body:?}");
        }
        for body in [
            "Too many requests",
            "Requests per minute limit reached",
            "Concurrent requests quota exceeded",
            "Please retry in 5s",
            "Service overloaded 529",
            "已达到速率限制",
            "请求过于频繁，请稍后重试",
            "并发请求达到上限",
            "每分钟使用次数已达上限",
            "API 使用频率已达上限",
        ] {
            assert!(!classify(Some(429), body), "{body:?}");
        }
    }

    #[test]
    fn account_text_and_billing() {
        for body in [
            "quota reached",
            "Your account's rate limit was reached",
            "You've exceeded your subscription rate limits",
            "free-models-per-day",
            "clinepass limit reached for this window",
            "Free limit reached on model x/y",
            "已达到 5 小时的使用上限",
            "额度已用完，请充值",
            "您的限额将在 2026-08-06 20:06:00 重置",
            "账户余额不足",
            "usage_limit_reached",
        ] {
            assert!(classify(Some(429), body), "{body:?}");
        }
        assert!(classify(Some(402), "Payment Required"));
        assert!(classify(Some(402), "concurrent requests limit reached"));
        assert!(!classify(Some(402), "A subscription is required for this endpoint"));
        assert!(!classify(Some(401), "Invalid API key"));
        assert!(!classify(Some(400), "model unsupported"));
    }

    #[test]
    fn structured_codes_and_google_reason_precede_text() {
        let code = json!({"error": {"code": "insufficient_quota", "message": "Request declined"}});
        assert!(account_usage_limit(Some(429), Some(&code), "Request declined", None));
        let throttle = json!({"error": {"code": "quota_exceeded", "message": "Rate limit exceeded"}});
        assert!(!account_usage_limit(Some(429), Some(&throttle), "Rate limit exceeded", None));
        let second_code = json!({"error": {
            "code": "quota_exceeded", "type": "usage_limit_reached", "message": "Request declined"
        }});
        assert!(account_usage_limit(Some(429), Some(&second_code), "Request declined", None));
        let payment = json!({"error": {"code": "deactivated_workspace", "message": "Request declined"}});
        assert!(account_usage_limit(Some(402), Some(&payment), "Request declined", None));
        let google = |reason: &str, retry_delay: Option<&str>, message: &str| {
            let mut details = vec![json!({
                "@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": reason
            })];
            if let Some(delay) = retry_delay {
                details.push(json!({"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": delay}));
            }
            json!({"error": {"status": "RESOURCE_EXHAUSTED", "message": message, "details": details}})
        };
        for reason in ["QUOTA_EXHAUSTED", "INSUFFICIENT_G1_CREDITS_BALANCE"] {
            let body = google(reason, None, "Too many requests");
            assert!(account_usage_limit(Some(429), Some(&body), "Too many requests", None));
        }
        let short = google("RATE_LIMIT_EXCEEDED", Some("30s"), "Too many requests");
        assert!(!account_usage_limit(Some(429), Some(&short), "Too many requests", None));
        let long = google("RATE_LIMIT_EXCEEDED", Some("300s"), "Too many requests");
        assert!(account_usage_limit(Some(429), Some(&long), "Too many requests", None));
        let long_ms = google("RATE_LIMIT_EXCEEDED", Some("300000ms"), "Too many requests");
        assert!(account_usage_limit(Some(429), Some(&long_ms), "Too many requests", None));
        let reset = google("RATE_LIMIT_EXCEEDED", None, "Your limit will reset in 10 minutes");
        assert!(account_usage_limit(Some(429), Some(&reset), "Your limit will reset in 10 minutes", None));
        let absolute =
            google("RATE_LIMIT_EXCEEDED", None, "Your limit will reset at 2099-01-01 00:00:00Z. Please retry in 5s");
        assert!(account_usage_limit(Some(429), Some(&absolute), "Too many requests", None));
        let absolute_offset = google("RATE_LIMIT_EXCEEDED", None, "Your limit will reset at 2099-01-01 08:00:00+0800");
        assert!(account_usage_limit(Some(429), Some(&absolute_offset), "Too many requests", None));
        let chinese_absolute = google("RATE_LIMIT_EXCEEDED", None, "您的限额将在 2099-01-01 00:00:00 重置");
        assert!(account_usage_limit(Some(429), Some(&chinese_absolute), "Too many requests", None));
        let short_reset = google("RATE_LIMIT_EXCEEDED", None, "Your limit will reset in 30s");
        assert!(!account_usage_limit(Some(429), Some(&short_reset), "Your limit will reset in 30s", None));
        let past_reset = google("RATE_LIMIT_EXCEEDED", None, "Your limit will reset at 2000-01-01 00:00:00Z");
        assert!(!account_usage_limit(Some(429), Some(&past_reset), "Too many requests", None));
        let model = google("RATE_LIMIT_EXCEEDED", None, "You have exhausted your capacity on this model");
        assert!(account_usage_limit(Some(429), Some(&model), "Too many requests", None));
    }

    #[test]
    fn dashscope_throttle_requires_exact_anchor_and_wording() {
        let message = "You exceeded your current quota, please check your plan and billing details";
        let body = json!({"error": {"code": "insufficient_quota", "message": message}});
        assert!(account_usage_limit(Some(429), Some(&body), message, None));
        let throttled = format!("{message} https://help.aliyun.com/zh/model-studio/error-code#token-limit");
        assert!(!account_usage_limit(Some(429), Some(&body), &throttled, None));
        assert!(classify(Some(429), "Free allocated quota exceeded https://help.aliyun.com/error-code#token-limit"));
    }
}
