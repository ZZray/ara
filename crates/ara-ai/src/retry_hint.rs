//! Ordered HTTP/body retry hints from fixed OMP
//! `packages/utils/src/fetch-retry.ts` at
//! b91db66e9b0b65a2c977e6a5bc74b0f4b1d58d1c.
//! MIT; see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Headers retain first-valid order; body signals merge by longest wait. The Session's
//! separate maximum-header helper remains in `retry_classification`.
//! Dates cover HTTP date and RFC3339 forms, including the provider UTC forms
//! below; JavaScript Date.parse's wider implementation-dependent grammar is
//! not claimed here.

use regex::{Captures, Regex};
use reqwest::header::HeaderMap;
use std::sync::LazyLock;

// Native non-u /i folds ASCII letters in these Latin patterns. Enable Unicode
// only for the explicit JavaScript whitespace class, which has no case folds.
const JS_SPACE: &str =
    r"(?u:[\x09-\x0d\x20\u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}])";

macro_rules! pattern {
    ($name:ident, $text:expr) => {
        static $name: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(&$text.replace(r"\s", JS_SPACE)).expect("fixed OMP retry hint pattern"));
    };
}

pattern!(QUOTA_RESET, r"(?i-u)reset after (?:([0-9]+)h)?(?:([0-9]+)m)?([0-9]+(?:\.[0-9]+)?)s");
pattern!(
    RESET_AT,
    r"(?i-u)(?:will\s+)?reset at\s+([0-9]{4}-[0-9]{2}-[0-9]{2}[ T][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:Z|[+-][0-9]{2}:?[0-9]{2})?)"
);
pattern!(CN_RESET_AT, r"将在\s*([0-9]{4}-[0-9]{2}-[0-9]{2}\s+[0-9]{2}:[0-9]{2}:[0-9]{2})\s*重置");
pattern!(RESET_IN, r"(?i-u)(?:will\s+)?resets?\s+in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h|days?|d)(?-u:\b)");
pattern!(RETRY_AFTER_MS, r"(?i-u)(?-u:\b)retry-after-ms\s*[:=]\s*([0-9]+)(?-u:\b)");
pattern!(RESET_IN_HR_MIN, r"(?i-u)resets?\s+in\s+~?\s*([0-9]+(?:\.[0-9]+)?)\s*hr\s*([0-9]+(?:\.[0-9]+)?)\s*min(?-u:\b)");
// Keep the negated JavaScript whitespace class explicit: pattern!'s \s
// replacement is a group and cannot be placed inside a character class.
pattern!(LEGACY_RETRY_AFTER, r"(?i-u)retry-after\s*[:=]\s*((?u:[^\x09-\x0d\x20\u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff},;]+))");
pattern!(LEGACY_RESET_MS, r"(?i-u)x-ratelimit-reset-ms\s*[:=]\s*([0-9]+)");
pattern!(LEGACY_RESET, r"(?i-u)x-ratelimit-reset\s*[:=]\s*([0-9]+)");
pattern!(PLEASE_RETRY, r"(?i-u)Please retry in ([0-9.]+)(ms|s)");
pattern!(RETRY_DELAY_FIELD, r#"(?i-u)"retryDelay":\s*"([0-9.]+)(ms|s)""#);
pattern!(TRY_AGAIN, r"(?i-u)try again in\s+~?\s*([0-9.]+)\s*(ms|sec|s|minutes?|mins?|m|hours?|hrs?|h)(?-u:\b)");

fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c.is_whitespace() && c != '\u{0085}' || c == '\u{feff}')
}

fn number(value: &str) -> Option<f64> {
    let value = js_trim(value);
    let number = if value.is_empty() {
        0.0
    } else if let Some(digits) = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
        radix_number(digits, 16)?
    } else if let Some(digits) = value.strip_prefix("0b").or_else(|| value.strip_prefix("0B")) {
        radix_number(digits, 2)?
    } else if let Some(digits) = value.strip_prefix("0o").or_else(|| value.strip_prefix("0O")) {
        radix_number(digits, 8)?
    } else {
        value.parse::<f64>().ok()?
    };
    number.is_finite().then_some(number)
}

fn radix_number(value: &str, radix: u32) -> Option<f64> {
    if value.is_empty() {
        return None;
    }
    value.chars().try_fold(0.0, |number, c| Some(number * f64::from(radix) + f64::from(c.to_digit(radix)?)))
}

// The fixed body patterns use Number.parseFloat, so "1.2.3s" has the
// numeric prefix 1.2 rather than failing Number's whole-string parse.
fn float_prefix(value: &str) -> Option<f64> {
    let mut dot = false;
    let end = value
        .char_indices()
        .find_map(|(index, c)| {
            if c.is_ascii_digit() {
                None
            } else if c == '.' && !dot {
                dot = true;
                None
            } else {
                Some(index)
            }
        })
        .unwrap_or(value.len());
    number(&value[..end])
}

fn date_ms(value: &str) -> Option<f64> {
    let value = js_trim(value);
    if let Ok(date) = httpdate::parse_http_date(value) {
        return Some(match date.duration_since(std::time::UNIX_EPOCH) {
            Ok(time) => time.as_secs_f64() * 1000.0,
            Err(error) => -error.duration().as_secs_f64() * 1000.0,
        });
    }
    // Native accepts an ISO offset without the colon as well as RFC3339.
    let mut normalized = value.to_owned();
    let bytes = normalized.as_bytes();
    if bytes.len() >= 5 {
        let tail = &bytes[bytes.len() - 5..];
        if matches!(tail[0], b'+' | b'-') && tail[1..].iter().all(u8::is_ascii_digit) {
            normalized.insert(normalized.len() - 2, ':');
        }
    }
    chrono::DateTime::parse_from_rfc3339(&normalized)
        .or_else(|_| chrono::DateTime::parse_from_rfc2822(value))
        .ok()
        .map(|date| date.timestamp_millis() as f64)
}

// Rust f64::max drops NaN, whereas the source Math.max preserves it.
fn clamp_nonnegative(value: f64) -> f64 {
    if value.is_nan() { value } else { value.max(0.0) }
}

fn unit_ms(unit: &str) -> Option<f64> {
    match unit.to_ascii_lowercase().as_str() {
        "ms" => Some(1.0),
        "s" | "sec" => Some(1000.0),
        "m" | "min" | "mins" | "minute" | "minutes" => Some(60_000.0),
        "h" | "hr" | "hrs" | "hour" | "hours" => Some(3_600_000.0),
        "d" | "day" | "days" => Some(86_400_000.0),
        _ => None,
    }
}

fn duration(captures: Captures<'_>) -> Option<f64> {
    let value = float_prefix(captures.get(1)?.as_str())?;
    (value > 0.0).then_some(value * unit_ms(captures.get(2)?.as_str())?)
}

/// Value adapter for the fixed catalog's timezone-naive reset policy.
/// Provider matching is exact; unrelated providers retain UTC fallback.
pub fn retry_reset_timezone_offset_for(provider: &str) -> Option<&'static str> {
    match provider {
        "zai" | "zhipu-coding-plan" => Some("+08:00"),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RetryHintOptions<'a> {
    /// Explicit RFC3339 offset (for example +08:00). Empty/local-zone options
    /// require the original Native Date.parse path, outside this value adapter.
    pub naive_reset_timezone_offset: Option<&'a str>,
}

/// Keep the existing no-options entry point for its published callers.
pub fn extract_retry_hint(headers: Option<&HeaderMap>, body: Option<&str>, now_ms: f64) -> Option<f64> {
    extract_retry_hint_with_options(headers, body, now_ms, RetryHintOptions::default())
}

pub fn extract_provider_retry_hint(provider: Option<&str>, body: Option<&str>, now_ms: f64) -> Option<f64> {
    extract_retry_hint_with_options(None, body, now_ms, RetryHintOptions {
        naive_reset_timezone_offset: provider.and_then(retry_reset_timezone_offset_for),
    })
}

/// Ordered headers, then longest authoritative body signal, naive fallback,
/// explicit retry-now, or no hint. Native execution owns wider Date semantics.
pub fn extract_retry_hint_with_options(
    headers: Option<&HeaderMap>, body: Option<&str>, now_ms: f64, options: RetryHintOptions<'_>,
) -> Option<f64> {
    if let Some(headers) = headers {
        let header = |name| headers.get(name).and_then(|value| value.to_str().ok()).filter(|value| !value.is_empty());
        if let Some(ms) = header("retry-after-ms").and_then(number).filter(|value| *value >= 0.0) {
            return Some(ms);
        }
        if let Some(value) = header("retry-after") {
            if let Some(seconds) = number(value) {
                return Some(clamp_nonnegative(seconds * 1000.0));
            }
            if let Some(date) = date_ms(value) {
                return Some(clamp_nonnegative(date - now_ms));
            }
        }
        if let Some(value) = header("x-ratelimit-reset-ms").and_then(number).filter(|value| *value > 0.0) {
            let target = if value > 1e12 {
                Some(value)
            } else if value > 1e9 {
                Some(value * 1000.0)
            } else {
                None
            };
            match target {
                None => return Some(value),
                Some(target) if target > now_ms => return Some(target - now_ms),
                Some(_) => {}
            }
        }
        if let Some(value) = header("x-ratelimit-reset") {
            let text = js_trim(value);
            let digits = text.strip_prefix(['+', '-']).unwrap_or(text).bytes().take_while(u8::is_ascii_digit).count();
            if digits > 0 {
                let end = digits + usize::from(text.starts_with(['+', '-']));
                if let Ok(seconds) = text[..end].parse::<f64>() {
                    let delta = seconds * 1000.0 - now_ms;
                    if delta > 0.0 {
                        return Some(delta);
                    }
                }
            }
        }
        if let Some(seconds) = header("x-ratelimit-reset-after").and_then(number).filter(|value| *value > 0.0) {
            return Some(seconds * 1000.0);
        }
    }
    let body = body.filter(|body| !body.is_empty())?;
    let mut longest: Option<f64> = None;
    let mut naive: Option<f64> = None;
    let mut retry_now = false;
    let consider = |slot: &mut Option<f64>, value: f64| {
        if value > 0.0 && slot.is_none_or(|current| value > current) {
            *slot = Some(value);
        }
    };
    let clamped = |slot: &mut Option<f64>, retry_now: &mut bool, value: f64| {
        if value > 0.0 { consider(slot, value); } else { *retry_now = true; }
    };
    if let Some(captures) = QUOTA_RESET.captures(body) {
        let part = |index| captures.get(index).and_then(|capture| capture.as_str().parse::<f64>().ok()).unwrap_or(0.0);
        consider(&mut longest, ((part(1) * 60.0 + part(2)) * 60.0 + part(3)) * 1000.0);
    }
    for pattern in [&*RESET_AT, &*CN_RESET_AT] {
        if let Some(captures) = pattern.captures(body) {
            let mut value = captures[1].replacen(' ', "T", 1);
            let has_offset = value.ends_with(['Z', 'z']) || value[10..].contains(['+', '-']);
            if !has_offset {
                value.push_str(options.naive_reset_timezone_offset.unwrap_or("Z"));
            }
            if let Some(date) = date_ms(&value).filter(|date| *date > now_ms) {
                consider(if has_offset || options.naive_reset_timezone_offset.is_some() { &mut longest } else { &mut naive }, date - now_ms);
            }
        }
    }
    if let Some(captures) = RESET_IN_HR_MIN.captures(body)
        && let (Some(hours), Some(minutes)) = (float_prefix(&captures[1]), float_prefix(&captures[2]))
        && hours >= 0.0 && minutes > 0.0
    {
        consider(&mut longest, hours * 3_600_000.0 + minutes * 60_000.0);
    }
    if let Some(value) = RESET_IN.captures(body).and_then(duration) {
        consider(&mut longest, value);
    }
    if let Some(captures) = RETRY_AFTER_MS.captures(body)
        && let Some(ms) = number(&captures[1])
    {
        clamped(&mut longest, &mut retry_now, ms);
    }
    for pattern in [&*PLEASE_RETRY, &*RETRY_DELAY_FIELD, &*TRY_AGAIN] {
        if let Some(value) = pattern.captures(body).and_then(duration) {
            consider(&mut longest, value);
        }
    }
    if let Some(captures) = LEGACY_RETRY_AFTER.captures(body) {
        if let Some(seconds) = number(&captures[1]) {
            clamped(&mut longest, &mut retry_now, seconds * 1000.0);
        } else if let Some(date) = date_ms(&captures[1]) {
            clamped(&mut longest, &mut retry_now, date - now_ms);
        }
    }
    if let Some(captures) = LEGACY_RESET_MS.captures(body)
        && let Ok(value) = captures[1].parse::<f64>()
    {
        clamped(&mut longest, &mut retry_now, if value > 1e12 { value - now_ms } else { value });
    }
    if let Some(captures) = LEGACY_RESET.captures(body)
        && let Ok(value) = captures[1].parse::<f64>()
    {
        clamped(&mut longest, &mut retry_now, if value > 1e9 { value * 1000.0 - now_ms } else { value * 1000.0 });
    }
    longest.or(naive).or(if retry_now { Some(0.0) } else { None })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_headers_keep_zero_dates_epoch_and_delta_contracts() {
        let now = 1_700_000_000_000.0;
        for (entries, expected) in [
            (vec![("retry-after-ms", "0"), ("retry-after", "7200")], Some(0.0)),
            (vec![("retry-after-ms", "-1"), ("retry-after", "0.25"), ("x-ratelimit-reset-after", "7200")], Some(250.0)),
            (vec![("retry-after", "-2")], Some(0.0)),
            (vec![("retry-after-ms", "0x10")], Some(16.0)),
            (vec![("retry-after-ms", "Infinity"), ("x-ratelimit-reset-ms", "25")], Some(25.0)),
            (vec![("x-ratelimit-reset-ms", "1700000001000")], Some(1000.0)),
            (vec![("x-ratelimit-reset-ms", "1700000001")], Some(1000.0)),
            (vec![("x-ratelimit-reset", "1700000002junk")], Some(2000.0)),
            (vec![("x-ratelimit-reset", "60"), ("x-ratelimit-reset-after", "0.5")], Some(500.0)),
            (vec![("retry-after", "Tue, 14 Nov 2023 22:13:21 GMT")], Some(1000.0)),
            (vec![("retry-after", "2023-11-14T22:13:21Z")], Some(1000.0)),
        ] {
            let mut headers = HeaderMap::new();
            for (name, value) in entries {
                headers.insert(name, value.parse().unwrap());
            }
            assert_eq!(extract_retry_hint(Some(&headers), Some("Please retry in 5s"), now), expected, "{headers:?}");
        }
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "2023-11-14T22:13:21Z".parse().unwrap());
        assert!(extract_retry_hint(Some(&headers), None, f64::NAN).unwrap().is_nan());
    }

    #[test]
    fn body_families_keep_account_precedence_units_dates_and_json() {
        let now = 1_700_000_000_000.0;
        for (body, expected) in [
            ("reset after 1h2m3.5s. Please retry in 1s", Some(3_723_500.0)),
            ("Your limit will reset in ~158 min. Please retry in 5s", Some(9_480_000.0)),
            // b91 relative timing overrides a timezone-naive wall-clock fallback.
            ("Please retry in 5s; Your limit will reset at 2023-11-14 22:13:22", Some(5000.0)),
            ("Your limit will reset at 2023-11-14 23:13:20 retry-after-ms=5000", Some(5000.0)),
            ("reset at 2023-11-15T06:13:22+0800", Some(2000.0)),
            ("将在 2023-11-14 22:13:23 重置", Some(3000.0)),
            ("retry-after-ms=98497000 Please retry in 1s", Some(98_497_000.0)),
            ("Please retry in 250ms", Some(250.0)),
            (r#"{"error":{"retryDelay":"34.074824224s"}}"#, Some(34_074.824_224)),
            ("try again in 12sec", Some(12_000.0)),
            ("try again in 90 minutes", Some(5_400_000.0)),
            ("try again in 1 hour", Some(3_600_000.0)),
            ("Please retry in 1.2.3s", Some(1200.0)),
            ("reset in 0 seconds; Please retry in 0ms", None),
            ("reſet in 7200s", None),
            ("RESET IN 7200s", Some(7_200_000.0)),
            ("RESET IN\u{00a0}7200S", Some(7_200_000.0)),
            ("reset in\u{feff}1s", Some(1000.0)),
            ("Resets in 45min", Some(2_700_000.0)),
            ("Resets in 2hr 15min", Some(8_100_000.0)),
            ("Resets in 3 days", Some(259_200_000.0)),
            ("Resets in 1 day", Some(86_400_000.0)),
            ("reset in 5 minutes; retry-after-ms: 3600000", Some(3_600_000.0)),
            ("reset in 5 minutes; retry-after=3600", Some(3_600_000.0)),
            ("Please retry in 12s retry-after-ms = 7200000", Some(7_200_000.0)),
            ("retry-after-ms=0", Some(0.0)),
            ("retry-after=0", Some(0.0)),
            ("x-ratelimit-reset-ms=1699999999000", Some(0.0)),
            ("x-ratelimit-reset=1699999999", Some(0.0)),
            ("reset in 5 minutes; retry-after-ms=0", Some(300_000.0)),
            ("reset at 2023-11-14T22:13:19Z", None),
        ] {
            let actual = extract_retry_hint(None, Some(body), now);
            match (actual, expected) {
                (Some(actual), Some(expected)) => assert!((actual - expected).abs() < 0.00001, "{body}: {actual}"),
                _ => assert_eq!(actual, expected, "{body}"),
            }
        }
        let offset = RetryHintOptions { naive_reset_timezone_offset: Some("+08:00") };
        let wall = "将在 2023-11-15 06:13:23 重置";
        assert_eq!(extract_retry_hint_with_options(None, Some(wall), now, offset), Some(3000.0));
        assert_eq!(extract_provider_retry_hint(Some("zai"), Some(wall), now), Some(3000.0));
        assert_eq!(extract_provider_retry_hint(Some("zhipu-coding-plan"), Some(wall), now), Some(3000.0));
        assert_eq!(extract_provider_retry_hint(Some("ZAI"), Some(wall), now), Some(28_803_000.0));
        assert_eq!(extract_retry_hint_with_options(None, Some("reset at 2023-11-15 06:13:23 retry-after-ms=2000"), now, offset), Some(3000.0));
        assert_eq!(extract_retry_hint_with_options(None, Some("reset at 2023-11-14T22:13:23Z retry-after-ms=2000"), now, offset), Some(3000.0));
        assert_eq!(extract_retry_hint_with_options(None, Some("reset at 2023-11-15 07:13:20 retry-after-ms=5000"), now, offset), Some(3_600_000.0));
    }
}
