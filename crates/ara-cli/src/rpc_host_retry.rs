//! Fixed OMP 596f2da turn-recovery.ts:1213-1425 and retry-fallback-chains.ts:72-77.
//! Host decisions use terminal failure facts plus the actual retained receipts.
use ara_ai::retry_classification::RetryClass;
use ara_ai::{AssistantBlock, AssistantMessage, Message};

#[path = "rpc_host_retry_actual.rs"]
mod actual;
#[cfg(test)]
pub(super) use actual::observe_native_retry_family;
pub(super) use actual::{
    NativeRetryClassifierFunctions, NativeRetryClassifierPort, NativeRetryContextReceipt,
    NativeRetryHintFunctions, NativeRetryHintPort, NativeRetryMetadata, annotate_retry_actual,
    disposition_actual, effective_max_retries_actual, last_assistant_actual,
    replay_disposition_actual, same_route_class_actual, session_retry_hint_actual,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RetryDisposition {
    RemoveFailed,
    PreserveFailed,
}

/// Fixed turn-recovery.ts:1400-1412,2156-2159. This cap belongs to the
/// current failed message; later failures in the same saga are classified anew.
pub(super) fn effective_max_retries(message: &AssistantMessage, configured: f64) -> f64 {
    if !message.content.iter().any(|block| {
        matches!(block, AssistantBlock::Thinking(thinking) if !ara_prompt::js::trim(&thinking.thinking).is_empty())
    }) {
        return configured;
    }
    let error = message
        .error_message
        .as_deref()
        .unwrap_or("")
        .to_ascii_lowercase();
    let bounded = match message.provider.as_str() {
        "openrouter" => error.match_indices("server_error:").any(|(index, marker)| {
            // Native /i is ASCII case folding for this non-Unicode pattern;
            // reuse JS whitespace rather than Rust regex's broader \s set.
            let tail = ara_prompt::js::trim_start(&error[index + marker.len()..]);
            let Some(tail) = tail.strip_prefix("stream closed with reason:") else {
                return false;
            };
            ara_prompt::js::trim_start(tail).starts_with("error")
        }),
        "github-copilot" if message.model == "grok-4.6" && message.api == "openai-responses" => {
            error.contains(
                "openai responses stream closed before a terminal response event was received",
            )
        }
        _ => false,
    };
    // RetryPolicy rejects nonfinite settings; keep zero, negative and fractional
    // values unchanged when they are already below the native one-retry cap.
    if bounded {
        configured.min(1.0)
    } else {
        configured
    }
}

pub(super) fn disposition(
    message: &AssistantMessage,
    messages: &[Message],
    class: &RetryClass,
) -> Option<RetryDisposition> {
    let disposition = replay_disposition(message, messages, class)?;
    let same_route = match disposition {
        RetryDisposition::RemoveFailed => class.retriable || class.abort,
        RetryDisposition::PreserveFailed => class.retriable || class.abort || class.malformed,
    };
    same_route.then_some(disposition)
}

/// Replay safety does not make a hard failure eligible for same-model backoff.
/// Native turn-recovery.ts:1946-1973,2355-2379 consults a configured candidate
/// separately; only an actually adopted candidate may continue a chain-only
/// error. The caller still excludes abort, preflight blocks and real overflow.
pub(super) fn replay_disposition(
    message: &AssistantMessage,
    messages: &[Message],
    class: &RetryClass,
) -> Option<RetryDisposition> {
    if class.overflow || class.replay_blocked {
        return None;
    }
    let index = messages
        .iter()
        .rposition(|candidate| candidate.as_assistant() == Some(message))?;
    let calls = message.tool_calls().collect::<Vec<_>>();
    if calls.is_empty() {
        if index + 1 != messages.len()
            || message.content.iter().any(|block| match block {
                AssistantBlock::Thinking(_) => false,
                AssistantBlock::Text(text) => !ara_prompt::js::trim(&text.text).is_empty(),
                _ => true,
            })
        {
            return None;
        }
        return Some(RetryDisposition::RemoveFailed);
    }
    let mut received = std::collections::HashSet::new();
    let mut all_unexecuted = true;
    for candidate in &messages[index + 1..] {
        let Message::ToolResult(result) = candidate else {
            return None;
        };
        if !calls
            .iter()
            .any(|call| call.id == result.tool_call_id && call.name == result.tool_name)
        {
            return None;
        }
        let details = result.details.as_ref();
        if details
            .is_some_and(|details| details["executed"] == "unknown" || details["panicked"] == true)
        {
            return None;
        }
        let synthetic = details.is_some_and(|details| details["__synthetic"] == true);
        let unexecuted = details.is_some_and(|details| details["executed"] == false);
        // Synthetic with no positive execution proof is not an actual receipt.
        if synthetic && !unexecuted {
            return None;
        }
        all_unexecuted &= synthetic && unexecuted;
        received.insert(result.tool_call_id.as_str());
    }
    if calls
        .iter()
        .any(|call| !received.contains(call.id.as_str()))
    {
        return None;
    }
    if (class.interrupted_stream && class.retriable) || class.abort {
        return Some(RetryDisposition::PreserveFailed);
    }
    let unsafe_output = message.content.iter().any(|block| match block {
        AssistantBlock::Thinking(_) | AssistantBlock::ToolCall(_) => false,
        AssistantBlock::Text(text) => !ara_prompt::js::trim(&text.text).is_empty(),
        _ => true,
    });
    (all_unexecuted && !unsafe_output).then_some(RetryDisposition::PreserveFailed)
}

pub(super) fn backoff_ms(base: f64, attempt: usize, random: f64) -> f64 {
    if base <= 0.0 {
        return 0.0;
    }
    (base.max(0.0) * 2f64.powi(attempt.saturating_sub(1).min(i32::MAX as usize) as i32)).min(8000.0)
        * (1.0 - random.clamp(0.0, 1.0) * 0.25)
}

/// Fixed turn-recovery.ts:2063-2110, after utils/retry-after.ts's formatter.
/// The public error remains unchanged; only this parsing view receives the
/// provider-safe header hint. Same-route waits and selector cooldowns share it.
pub(super) fn session_retry_hint(message: &AssistantMessage, now_ms: f64) -> Option<f64> {
    let error = message.error_message.as_deref().unwrap_or("");
    let header = message
        .failure_evidence
        .as_ref()
        .and_then(|facts| facts.header_wait_ms);
    let text = match header.filter(|wait| wait.is_finite() && *wait > 0.0) {
        Some(wait) if !error.contains("retry-after-ms=") => {
            format!(
                "{error} retry-after-ms={}",
                ara_prompt::js::f64_to_string(wait)
            )
        }
        _ => error.to_owned(),
    };
    parse_retry_after_error(&text, now_ms)
}

fn parse_retry_after_error(message: &str, now_ms: f64) -> Option<f64> {
    use ara_cli::js_regex::JsRegExp;
    let input = message.into();
    let capture = |source: &str| {
        JsRegExp::new(source.into(), "i")
            .expect("fixed Native Session retry hint pattern")
            .exec(&input)
            .and_then(|matched| {
                matched
                    .captures
                    .get(1)
                    .and_then(Option::as_ref)
                    .and_then(|value| value.to_utf8().ok())
            })
    };
    let number =
        |value: &str| ara_prompt::js::to_number(&serde_json::Value::String(value.to_owned()));
    if let Some(value) = capture(r"retry-after-ms\s*[:=]\s*(\d+)") {
        return Some(number(&value).max(0.0));
    }
    if let Some(value) = capture(r"retry-after\s*[:=]\s*([^\s,;]+)") {
        let seconds = number(&value);
        if !seconds.is_nan() {
            return Some((seconds * 1000.0).max(0.0));
        }
        if let Some(date) = retry_date_ms(&value) {
            return Some((date - now_ms).max(0.0));
        }
    }
    if let Some(wait) = ara_ai::retry_hint::extract_retry_hint(None, Some(message), now_ms) {
        return Some(wait);
    }
    if let Some(value) = capture(r"x-ratelimit-reset-ms\s*[:=]\s*(\d+)") {
        let reset = number(&value);
        if !reset.is_nan() {
            return Some(if reset > 1_000_000_000_000.0 {
                (reset - now_ms).max(0.0)
            } else {
                reset.max(0.0)
            });
        }
    }
    if let Some(value) = capture(r"x-ratelimit-reset\s*[:=]\s*(\d+)") {
        let reset = number(&value);
        if !reset.is_nan() {
            return Some(if reset > 1_000_000_000.0 {
                (reset * 1000.0 - now_ms).max(0.0)
            } else {
                (reset * 1000.0).max(0.0)
            });
        }
    }
    None
}

// The production Native Date and typed retry consumer share the same Host
// parser. Unsupported Host timezone domains retain this facade's existing None
// boundary; the actual Native Date operation preserves the explicit error.
fn retry_date_ms(value: &str) -> Option<f64> {
    use ara_cli::native_realm::date::{DateHost, SystemDateHost};
    SystemDateHost
        .parse_ms(&value.into())
        .ok()
        .filter(|date| date.is_finite())
}

// Retain the previous strict-ISO fixture helper for its original observations;
// it is test-only and does not define the shipped parser or broad acceptance.

#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg(test)]
enum RetryIsoDateParse {
    Parsed(f64),
    NotIso,
    Invalid,
    // Host-local conversion is outside chrono's range or crosses a DST gap.
    // Returning a normal parser failure would hide a remaining Native gap.
    HostLocalRequired,
}

#[cfg(test)]
fn retry_iso_date_ms(value: &str) -> RetryIsoDateParse {
    let Some(parts) = RetryIsoParts::parse(value.as_bytes()) else {
        return RetryIsoDateParse::NotIso;
    };
    if parts.negative_zero_year
        || !(1..=12).contains(&parts.month)
        || !(1..=31).contains(&parts.day)
        || parts.hour > 24
        || parts.minute > 59
        || parts.second > 59
        || (parts.hour == 24 && (parts.minute != 0 || parts.second != 0 || parts.fraction_nonzero))
        || parts
            .offset
            .is_some_and(|(hours, minutes, _)| hours > 23 || minutes > 59)
    {
        return RetryIsoDateParse::Invalid;
    }

    // Check day 1..=31 above, then normalize from the first of the month.
    // Unlike NaiveDate::from_ymd_opt(y, m, d), this preserves Bun's Feb 30 and
    // Apr 31 rollover. Integer arithmetic also covers the full TimeClip range,
    // including the portions outside chrono's +/-262k year calendar range.
    let days = retry_iso_days_from_civil(parts.year, parts.month, parts.day);
    let wall_ms = days * 86_400_000
        + i64::from(parts.hour) * 3_600_000
        + i64::from(parts.minute) * 60_000
        + i64::from(parts.second) * 1000
        + i64::from(parts.millisecond);
    let utc_ms = if !parts.has_time || parts.utc {
        wall_ms
    } else if let Some((hours, minutes, sign)) = parts.offset {
        wall_ms - i64::from(sign) * (i64::from(hours) * 60 + i64::from(minutes)) * 60_000
    } else {
        use chrono::{Local, MappedLocalTime, TimeZone};
        let Some(wall) = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(wall_ms) else {
            return RetryIsoDateParse::HostLocalRequired;
        };
        match Local.from_local_datetime(&wall.naive_utc()) {
            MappedLocalTime::Single(date) => date.timestamp_millis(),
            // Bun's observed fall-back overlap chooses the earlier instant.
            MappedLocalTime::Ambiguous(first, second) => {
                first.timestamp_millis().min(second.timestamp_millis())
            }
            // Bun advances a nonexistent spring-forward time through the gap;
            // chrono returns None. Keep this explicit until the Host resolver
            // implements and verifies Native's policy with the same tz data.
            MappedLocalTime::None => return RetryIsoDateParse::HostLocalRequired,
        }
    };
    if utc_ms.unsigned_abs() > 8_640_000_000_000_000_u64 {
        RetryIsoDateParse::Invalid
    } else {
        // Every integer in the TimeClip interval is below 2^53 and exact in f64.
        RetryIsoDateParse::Parsed(utc_ms as f64)
    }
}

#[cfg(test)]
struct RetryIsoParts {
    year: i64,
    negative_zero_year: bool,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millisecond: u16,
    fraction_nonzero: bool,
    has_time: bool,
    utc: bool,
    offset: Option<(u8, u8, i8)>,
}

#[cfg(test)]
impl RetryIsoParts {
    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut at = 0;
        let sign = match bytes.first() {
            Some(b'+') => {
                at += 1;
                1
            }
            Some(b'-') => {
                at += 1;
                -1
            }
            _ => 1,
        };
        let extended = at != 0;
        let magnitude = retry_iso_digits(bytes, &mut at, if extended { 6 } else { 4 })?;
        let mut parts = Self {
            year: sign * magnitude,
            negative_zero_year: extended && sign == -1 && magnitude == 0,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
            fraction_nonzero: false,
            has_time: false,
            utc: false,
            offset: None,
        };
        if at == bytes.len() {
            return Some(parts);
        }
        retry_iso_byte(bytes, &mut at, b'-')?;
        parts.month = retry_iso_digits(bytes, &mut at, 2)? as u8;
        if at == bytes.len() {
            return Some(parts);
        }
        retry_iso_byte(bytes, &mut at, b'-')?;
        parts.day = retry_iso_digits(bytes, &mut at, 2)? as u8;
        if at == bytes.len() {
            return Some(parts);
        }
        if !matches!(bytes.get(at), Some(b'T' | b't')) {
            return None;
        }
        at += 1;
        parts.has_time = true;
        parts.hour = retry_iso_digits(bytes, &mut at, 2)? as u8;
        retry_iso_byte(bytes, &mut at, b':')?;
        parts.minute = retry_iso_digits(bytes, &mut at, 2)? as u8;
        if bytes.get(at) == Some(&b':') {
            at += 1;
            parts.second = retry_iso_digits(bytes, &mut at, 2)? as u8;
            if bytes.get(at) == Some(&b'.') {
                at += 1;
                let start = at;
                while bytes.get(at).is_some_and(u8::is_ascii_digit) {
                    let digit = bytes[at] - b'0';
                    parts.fraction_nonzero |= digit != 0;
                    if at - start < 3 {
                        parts.millisecond = parts.millisecond * 10 + u16::from(digit);
                    }
                    at += 1;
                }
                let count = at - start;
                if count == 0 {
                    return None;
                }
                // Date.parse truncates, never rounds, beyond milliseconds.
                for _ in count..3 {
                    parts.millisecond *= 10;
                }
            }
        }
        match bytes.get(at) {
            None => {}
            Some(b'Z' | b'z') => {
                parts.utc = true;
                at += 1;
            }
            Some(b'+' | b'-') => {
                let offset_sign = if bytes[at] == b'+' { 1 } else { -1 };
                at += 1;
                let hours = retry_iso_digits(bytes, &mut at, 2)? as u8;
                // Native also accepts +0800; that non-colon extension remains
                // explicitly required rather than silently normalized here.
                retry_iso_byte(bytes, &mut at, b':')?;
                let minutes = retry_iso_digits(bytes, &mut at, 2)? as u8;
                parts.offset = Some((hours, minutes, offset_sign));
            }
            _ => return None,
        }
        (at == bytes.len()).then_some(parts)
    }
}

#[cfg(test)]
fn retry_iso_digits(bytes: &[u8], at: &mut usize, width: usize) -> Option<i64> {
    let digits = bytes.get(*at..at.checked_add(width)?)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let value = digits
        .iter()
        .fold(0_i64, |value, digit| value * 10 + i64::from(digit - b'0'));
    *at += width;
    Some(value)
}

#[cfg(test)]
fn retry_iso_byte(bytes: &[u8], at: &mut usize, expected: u8) -> Option<()> {
    if bytes.get(*at) != Some(&expected) {
        return None;
    }
    *at += 1;
    Some(())
}

#[cfg(test)]
fn retry_iso_days_from_civil(year: i64, month: u8, day: u8) -> i64 {
    const BEFORE_MONTH: [i64; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let previous = year - 1;
    let leap = year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    365 * (year - 1970) + previous.div_euclid(4) - 492 - (previous.div_euclid(100) - 19)
        + previous.div_euclid(400)
        - 4
        + BEFORE_MONTH[usize::from(month - 1)]
        + i64::from(leap && month > 2)
        + i64::from(day - 1)
}

#[cfg(test)]
mod retry_iso_date_tests {
    use super::{RetryIsoDateParse, retry_iso_date_ms};

    #[test]
    fn iso_utc_fixtures_match_bun_140_probe() {
        for (input, expected) in [
            ("2026-10", 1_790_812_800_000_i64),
            ("2026-10-04", 1_791_072_000_000),
            ("2026-10-04T01:02Z", 1_791_075_720_000),
            ("2026-10-04T01:02:03.123456Z", 1_791_075_723_123),
            ("2026-10-04t01:02:03z", 1_791_075_723_000),
            ("2026-10-04T01:02:03+08:00", 1_791_046_923_000),
            ("2026-10-04T24:00:00Z", 1_791_158_400_000),
            ("2026-02-29", 1_772_323_200_000),
            ("2026-02-30", 1_772_409_600_000),
            ("2026-04-31", 1_777_593_600_000),
            ("0000-01-01", -62_167_219_200_000),
            ("-000001-01-01", -62_198_755_200_000),
            ("+010000-01-01", 253_402_300_800_000),
            ("+275760-09-13T00:00:00.000Z", 8_640_000_000_000_000),
            ("-271821-04-20T00:00:00.000Z", -8_640_000_000_000_000),
        ] {
            assert_eq!(
                retry_iso_date_ms(input),
                RetryIsoDateParse::Parsed(expected as f64),
                "{input}"
            );
        }
    }

    #[test]
    fn invalid_iso_is_not_rescued_by_chrono_leap_second_support() {
        for input in [
            "2026-01-00",
            "2026-01-32",
            "2026-00-01",
            "2026-13-01",
            "2026-10-04T01:02:60Z",
            "2026-10-04T25:00Z",
            "2026-10-04T01:60Z",
            "2026-10-04T24:00:00.001Z",
            "+275760-09-13T00:00:00.001Z",
            "-271821-04-19T23:59:59.999Z",
        ] {
            assert_eq!(
                retry_iso_date_ms(input),
                RetryIsoDateParse::Invalid,
                "{input}"
            );
        }
    }

    #[test]
    fn extensions_keep_their_required_boundary() {
        for input in [
            "2026-1-2",
            "2026/10/04",
            "Oct-04-2026",
            "2026-10-04Z",
            "2026-10-04(foo)",
            "\"2026-10-04\"",
            "2026-10-04junk",
            "2026-10-04T01:02:03+0800",
            "2026-10-04+08:00",
        ] {
            assert_eq!(
                retry_iso_date_ms(input),
                RetryIsoDateParse::NotIso,
                "{input}"
            );
        }
        // Bun falls back to its wider legacy parser and returns year 2001.
        // This candidate reports the forbidden ISO negative-zero spelling;
        // the Native legacy fallback is still required, not claimed fixed.
        assert_eq!(
            retry_iso_date_ms("-000000-01-01"),
            RetryIsoDateParse::Invalid
        );
    }
}

pub(super) fn persistence_key(message: &AssistantMessage) -> String {
    format!(
        "assistant:{}:{}:{}:{}:{}",
        message.timestamp,
        message.provider,
        message.model,
        message.response_id.as_deref().unwrap_or(""),
        message.stop_reason.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ara_ai::{
        AssistantBlock, ImageContent, JsonObject, StopReason, ToolCall, ToolResultMessage,
        UserMessage,
    };
    use serde_json::json;

    fn call_turn() -> (AssistantMessage, Vec<Message>) {
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
        message.stop_reason = StopReason::Error;
        message.content.push(AssistantBlock::ToolCall(ToolCall {
            id: "one".into(),
            name: "write".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
            ..Default::default()
        }));
        let result = Message::ToolResult(ToolResultMessage {
            tool_call_id: "one".into(),
            tool_name: "write".into(),
            content: vec![],
            details: Some(json!({"__synthetic":true,"executed":false})),
            is_error: true,
            timestamp: 3,
            provider_metadata: None,
            useless: false,
        });
        let messages = vec![
            Message::User(UserMessage::text("request")),
            Message::Assistant(message.clone()),
            result,
        ];
        (message, messages)
    }

    #[test]
    fn false_is_positive_proof_unknown_or_duplicate_actual_result_vetoes() {
        let (message, mut messages) = call_turn();
        let class = RetryClass {
            retriable: true,
            ..Default::default()
        };
        assert_eq!(
            disposition(&message, &messages, &class),
            Some(RetryDisposition::PreserveFailed)
        );
        let Message::ToolResult(result) = &mut messages[2] else {
            unreachable!()
        };
        result.details = Some(json!({"__synthetic":true,"executed":"unknown"}));
        assert_eq!(disposition(&message, &messages, &class), None);
        let (message, mut messages) = call_turn();
        let mut actual = messages[2].clone();
        let Message::ToolResult(result) = &mut actual else {
            unreachable!()
        };
        result.details = None;
        messages.push(actual);
        assert_eq!(disposition(&message, &messages, &class), None);
        assert_eq!(
            disposition(
                &message,
                &messages,
                &RetryClass {
                    interrupted_stream: true,
                    ..class
                }
            ),
            Some(RetryDisposition::PreserveFailed)
        );
    }

    #[test]
    fn visible_text_image_and_newer_user_never_replay_an_empty_failed_turn() {
        let class = RetryClass {
            retriable: true,
            ..Default::default()
        };
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
        message.stop_reason = StopReason::Error;
        let user = Message::User(UserMessage::text("request"));
        let hard = RetryClass::default();
        let safe_tail = [user.clone(), Message::Assistant(message.clone())];
        assert_eq!(
            replay_disposition(&message, &safe_tail, &hard),
            Some(RetryDisposition::RemoveFailed)
        );
        assert_eq!(
            disposition(&message, &safe_tail, &hard),
            None,
            "a chain-only failure must not acquire same-route retry eligibility"
        );
        assert_eq!(
            disposition(
                &message,
                &[user.clone(), Message::Assistant(message.clone())],
                &class
            ),
            Some(RetryDisposition::RemoveFailed)
        );
        for block in [
            AssistantBlock::text("delivered"),
            AssistantBlock::Image(ImageContent {
                data: "opaque".into(),
                mime_type: "image/png".into(),
                detail: None,
                compaction_frame: false,
            }),
        ] {
            message.content = vec![block];
            assert_eq!(
                disposition(
                    &message,
                    &[user.clone(), Message::Assistant(message.clone())],
                    &class
                ),
                None
            );
        }
        message.content.clear();
        assert_eq!(
            disposition(
                &message,
                &[user.clone(), Message::Assistant(message.clone()), user],
                &class
            ),
            None
        );
    }

    #[test]
    fn source_backoff_has_exact_cap_and_jitter_edges() {
        assert_eq!(backoff_ms(500.0, 1, 0.0), 500.0);
        assert_eq!(backoff_ms(500.0, 1, 1.0), 375.0);
        assert_eq!(backoff_ms(500.0, 9, 0.0), 8000.0);
        assert_eq!(backoff_ms(500.0, 9, 1.0), 6000.0);
        assert_eq!(backoff_ms(-1.0, 10, 0.0), 0.0);
    }

    #[test]
    fn session_hint_keeps_native_branch_order_numbers_dates_and_reset_thresholds() {
        let now = 1_700_000_000_000.0;
        for (text, expected) in [
            ("retry-after-ms:0; retry-after:7200", Some(0.0)),
            (
                "retry-after-ms\u{feff}:\u{a0}25; reset in 2 hours",
                Some(25.0),
            ),
            ("retry-after-ms:1.5", Some(1.0)),
            ("retry-after-ms:-1; retry-after:.5", Some(500.0)),
            ("retry-after:0x10", Some(16_000.0)),
            ("retry-after:0b10", Some(2000.0)),
            ("retry-after:0o10", Some(8000.0)),
            ("retry-after:1e-3; reset in 2 hours", Some(1.0)),
            ("retry-after:-2", Some(0.0)),
            ("retry-after:Infinity", Some(f64::INFINITY)),
            ("retry-after:-Infinity", Some(0.0)),
            ("retry-after:inf; x-ratelimit-reset-ms:25", Some(25.0)),
            ("retry-after:2023-11-14T22:13:21Z", Some(1000.0)),
            ("retry-after:2023-11-15", Some(6_400_000.0)),
            (
                "retry-after:2023-11-14T22:13:20Z; reset in 2 hours",
                Some(0.0),
            ),
            (
                "retry-after:Tue, 14 Nov 2023 22:13:21 GMT; x-ratelimit-reset-ms:25",
                Some(25.0),
            ),
            (
                "retry-after:NaN; reset in 2 hours; x-ratelimit-reset-ms:25",
                Some(7_200_000.0),
            ),
            ("x-ratelimit-reset-ms:0", Some(0.0)),
            ("x-ratelimit-reset-ms:25; x-ratelimit-reset:50", Some(25.0)),
            ("x-ratelimit-reset-ms:1700000001", Some(1_700_000_001.0)),
            (
                "x-ratelimit-reset-ms:1000000000000",
                Some(1_000_000_000_000.0),
            ),
            ("x-ratelimit-reset-ms:1000000000001", Some(0.0)),
            ("x-ratelimit-reset-ms:1700000001000", Some(1000.0)),
            ("x-ratelimit-reset:1000000000", Some(1_000_000_000_000.0)),
            ("x-ratelimit-reset:1000000001", Some(0.0)),
            ("x-ratelimit-reset:1700000001", Some(1000.0)),
            ("x-ratelimit-reset-ms:-1; x-ratelimit-reset:2", Some(2000.0)),
            (
                "x-ratelimit-reset-ms:12junk; Please retry in 2s",
                Some(2000.0),
            ),
            ("RETRY-AFTER:0.125", Some(125.0)),
            ("retry-after\u{85}:1; x-ratelimit-reset-ms:25", Some(25.0)),
            ("no timing hint", None),
        ] {
            assert_eq!(parse_retry_after_error(text, now), expected, "{text}");
        }
    }

    #[test]
    fn header_provenance_preserves_native_exact_suppression_and_number_stringification() {
        let now = 1_700_000_000_000.0;
        for (text, header, expected) in [
            ("reset in 2 hours", Some(100.0), Some(100.0)),
            (
                "retry-after-ms:25; reset in 2 hours",
                Some(100.0),
                Some(25.0),
            ),
            ("retry-after-ms=0", Some(100.0), Some(0.0)),
            ("retry-after-ms=bogus", Some(100.0), None),
            ("RETRY-AFTER-MS=bogus", Some(100.0), Some(100.0)),
            (
                "retry-after-ms=bogus; x-ratelimit-reset-ms:25",
                Some(100.0),
                Some(25.0),
            ),
            ("retry-after:0.25", Some(100.0), Some(100.0)),
            ("no body hint", Some(1e21), Some(1.0)),
            ("reset in 2 hours", None, Some(7_200_000.0)),
        ] {
            let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
            message.error_message = Some(text.into());
            let mut evidence = ara_ai::ProviderError::Http {
                status: 503,
                detail: String::new(),
            }
            .failure_evidence(false);
            evidence.wait_ms = Some(999.0);
            evidence.header_wait_ms = header;
            message.failure_evidence = Some(evidence);
            assert_eq!(session_retry_hint(&message, now), expected, "{text}");
        }
        let legacy =
            json!({"kind":"http","status":503,"replayBlocked":false,"waitMs":999,"errorId":0});
        let decoded: ara_ai::retry_classification::ProviderFailureEvidence =
            serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.header_wait_ms, None);
    }
}
