//! Supported-route retry evidence and fixed OMP error flags.
//!
//! Source: `packages/ai/src/error/{flags,finalize,retryable}.ts` and
//! `packages/ai/src/utils/retry-after.ts`, fixed
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see THIRD_PARTY_NOTICES.md).
//! Host receipt/lifecycle gates and credential/model selection are separate.

use crate::{AssistantMessage, ProviderError, StopReason};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub mod flag {
    pub const CLASS: u32 = 0x1000;
    pub const THINKING_LOOP: u32 = 0x0001_0000;
    pub const TRANSIENT: u32 = 0x0002_0000;
    pub const TIMEOUT: u32 = 0x0004_0000;
    pub const USAGE_LIMIT: u32 = 0x0008_0000;
    pub const STALE_RESPONSES_ITEM: u32 = 0x0010_0000;
    pub const MALFORMED_FUNCTION_CALL: u32 = 0x0020_0000;
    pub const PROVIDER_FINISH_ERROR: u32 = 0x0040_0000;
    pub const EMPTY_RESPONSE: u32 = 0x0000_2000;
    pub const CONTENT_BLOCKED: u32 = 0x0000_8000;
    pub const ACCOUNT_POLICY: u32 = 0x0000_4000;
    pub const CONTEXT_OVERFLOW: u32 = 0x0080_0000;
    pub const AUTH_FAILED: u32 = 0x0100_0000;
    pub const ABORT: u32 = 0x0800_0000;
    pub const GRAMMAR: u32 = 0x1000_0000;
    pub const FAST_MODE_UNSUPPORTED: u32 = 0x2000_0000;
    pub const PAYLOAD_REJECTED: u32 = 0x8000_0000;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProviderErrorKind {
    Http,
    Stream,
    Transport,
    Incomplete,
    Timeout,
    Config,
    Aborted,
}

/// Only provider-safe facts survive. Never capture whole response headers,
/// authorization, cookies, request bodies or credential identifiers here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderFailureEvidence {
    pub kind: ProviderErrorKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Stops the provider's seconds-scale outer replay budget.
    pub replay_blocked: bool,
    /// Stops another request on this route. A provider wait-budget ceiling
    /// alone does not set this: the Session has its own configured ceiling.
    #[serde(default)]
    pub same_route_blocked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<f64>,
    pub error_id: u32,
}

impl ProviderFailureEvidence {
    pub fn from_error(error: &ProviderError, replay_blocked: bool) -> Self {
        let kind = match error {
            ProviderError::Http { .. } => ProviderErrorKind::Http,
            ProviderError::Stream(_) => ProviderErrorKind::Stream,
            ProviderError::Transport(_) => ProviderErrorKind::Transport,
            ProviderError::Incomplete(_) => ProviderErrorKind::Incomplete,
            ProviderError::Timeout(_) => ProviderErrorKind::Timeout,
            ProviderError::Config(_) => ProviderErrorKind::Config,
            ProviderError::Aborted => ProviderErrorKind::Aborted,
        };
        Self {
            kind,
            status: error.status(),
            code: None,
            replay_blocked,
            same_route_blocked: replay_blocked,
            wait_ms: None,
            error_id: 0,
        }
    }

    pub(crate) fn from_error_envelope(
        error: &ProviderError,
        envelope: &serde_json::Value,
        replay_blocked: bool,
    ) -> Self {
        let mut evidence = Self::from_error(error, replay_blocked);
        let body = envelope.get("response").unwrap_or(envelope);
        let structured = body.get("error").unwrap_or(body);
        evidence.code = structured
            .get("code")
            .and_then(serde_json::Value::as_str)
            .or_else(|| structured.get("type").and_then(serde_json::Value::as_str))
            .map(str::to_owned);
        if crate::usage_limit::account_usage_limit(evidence.status, Some(body), &error.to_string(), None) {
            evidence.error_id = flag::USAGE_LIMIT | flag::CLASS;
            evidence.replay_blocked = true;
            evidence.same_route_blocked = true;
        }
        evidence
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RetryClass {
    pub retriable: bool,
    pub overflow: bool,
    pub stale_responses: bool,
    pub usage_limit: bool,
    pub abort: bool,
    pub malformed: bool,
    pub wait_ms: Option<f64>,
    pub error_id: u32,
    pub interrupted_stream: bool,
    pub replay_blocked: bool,
}

macro_rules! pattern {
    ($name:ident, $text:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($text).expect("fixed OMP pattern"));
    };
}
pattern!(
    TOKEN_OVERFLOW,
    r"(?i)prompt is too long|input is too long for requested model|exceeds the context window|input token count.*exceeds the maximum|maximum prompt length is \d+|reduce the length of the messages|maximum context length is \d+ tokens|exceeds the available context size|requested tokens?.*exceed.*context (window|length|size)|context (window|length|size).*(exceeded|overflow|too small)|(prompt|input).*(too long|too large).*(context|n_ctx)|requested tokens?.*(exceeds?|greater than).*(n_ctx|context)|greater than the context length|context window exceeds limit|exceeded model token limit|context[_ ]length[_ ]exceeded|too many tokens|token limit exceeded|request_too_large[^\n]*\btokens?\b|\btokens?\b[^\n]*request_too_large|model_context_window_exceeded|prompt filled the context window|exceeds the limit of \d+ tokens?\b|chat history exceeds the \d+-message limit"
);
pattern!(OVERFLOW_NO_BODY, r"(?i)\b4(00|13)\s*(status code)?\s*\(no body\)|exceeds the limit of \d+");
pattern!(
    PAYLOAD,
    r"(?i)\b413\s*(?:status code\s*)?\(no body\)|\b413\b[^.\n]{0,120}\b(?:request|payload|entity|body)\b[^.\n]{0,60}\b(?:exceed|too large|limit)|request_too_large|(?:payload|entity) too large|request exceeds the maximum (?:size|number of bytes)"
);
pattern!(TIMEOUT_PATTERN, r"(?i)\b(?:operation\s+)?timed?\s*out\b|\btimeout\b|\bstream stall\b");
pattern!(
    TRANSIENT_PATTERN,
    r"(?i)\b(?:no[_ -]?capacity|(?:high|peak)[ _-]?demand|(?:at|over|insufficient)[ _-]?capacity|capacity[ _-]?(?:exceeded|exhausted)|peak[ _-]?load)\b|overloaded|provider.?returned.?error|rate.?limit|too many requests|\b(?:429|500|502|503|504)\b|service.?unavailable|server.?error|internal.?error|retry your request|network.?error|connection.?error|connection.?refused|unable.?to.?connect\.\s*is the computer able to access the url\?|other side closed|fetch failed|upstream.?connect|upstream.?request.?failed|reset before headers|socket hang up|timed? out|timeout|terminated|retry delay|stream stall|no error details in response|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)|nghttp2_(?:internal_error|refused_stream)|stream closed with error code nghttp2_(?:internal_error|refused_stream)|malformed.?function.?call|stream[_ -]?read[_ -]?error|\b(?:the\s+)?socket connection (?:was )?closed unexpectedly\b"
);
pattern!(
    AUTH,
    r"(?i)\b(?:401|403|unauthorized|forbidden|authentication|auth[_ ]?unavailable|no auth available|(?:invalid|no)[_ ]?api[_ ]?key)\b"
);
pattern!(MALFORMED, r"(?i)\bmalformed.?function.?call\b");
pattern!(FINISH_ERROR, r"(?i)\bProvider (?:returned error finish_reason|finish_reason:\s*error)\b");
pattern!(EMPTY, r"(?i)\bthought-only response without final output\b");
pattern!(BLOCKED, r"(?i)\b(?:incomplete:\s*)?content_filter\b");
pattern!(ACCOUNT_POLICY_PATTERN, r"(?i)\bcyber_policy\b|trusted access for cyber");
pattern!(
    STALE,
    r#"(?i)\bItem with id ['"][^'"]+['"] not found\.?|previous[ _]?response.*(?:not[ _]?found|invalid|expired|stale|zero[ _-]?data[ _-]?retention)"#
);
pattern!(LOCAL_PARSE, r"(?i)failed to parse tool call arguments as json|\[json\.exception\.parse_error\.101\]");
pattern!(GENERATION_NAN, r"(?is)floating[ _-]?point nan\b.*\bdetected in generation");
pattern!(
    CONCURRENT,
    r"(?i)\btoo many\s+concurren\w*\s+(?:requests?|invocations?)\b|\bconcurren\w*\b[^\n]{0,60}\b(?:limit|quota|exceed\w*|reach\w*)\b|\b(?:limit|quota|exceed\w*|reach\w*)\b[^\n]{0,60}\bconcurren\w*\b|\bconcurren[a-z]*[-_](?:[a-z]+[_-])*(?:limit|quota|exceed\w*|reach\w*)"
);
pattern!(
    INTERRUPTED,
    r"(?i)stream stall|stream closed with error code\s+nghttp2_(?:internal_error|refused_stream)|nghttp2_(?:internal_error|refused_stream)|HTTP2(?:StreamReset|RefusedStream)|stream closed before (?:a )?(?:finish_reason|terminal response event)|stream ended before (?:a )?terminal"
);
pattern!(
    IMMUTABLE_THINKING,
    r"(?is)messages\.\d+\.content\.\d+.*\b(?:thinking|redacted_thinking)\b.*\blatest assistant message cannot be modified\b"
);
pattern!(WAIT_MS_TEXT, r"(?i)retry-after-ms\s*[:=]\s*(\d+)");
pattern!(WAIT_SECONDS_TEXT, r"(?i)retry-after\s*[:=]\s*([^\s,;]+)");

fn finite_positive(value: f64) -> Option<f64> {
    (value.is_finite() && value > 0.0).then_some(value.ceil())
}

/// Fixed header helper takes the maximum finite, positive hint. Only four
/// known headers are read; the returned duration cannot contain secrets.
pub fn retry_wait_ms(headers: &reqwest::header::HeaderMap, now_ms: f64) -> Option<f64> {
    let value = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let number = |name| value(name).and_then(|value| value.trim().parse::<f64>().ok());
    let mut candidates = Vec::new();
    if let Some(value) = number("retry-after-ms").and_then(finite_positive) {
        candidates.push(value);
    }
    if let Some(value) = value("retry-after") {
        let delay = if let Ok(seconds) = value.trim().parse::<f64>() {
            finite_positive(seconds * 1000.0)
        } else {
            httpdate::parse_http_date(value.trim())
                .ok()
                .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|when| finite_positive(when.as_secs_f64() * 1000.0 - now_ms))
        };
        if let Some(delay) = delay {
            candidates.push(delay);
        }
    }
    for (name, seconds) in [("x-ratelimit-reset-ms", false), ("x-ratelimit-reset", true)] {
        if let Some(value) = number(name).filter(|value| value.is_finite() && *value > 0.0) {
            let delay = if value > 1e12 {
                value - now_ms
            } else if value > 1e9 {
                value * 1000.0 - now_ms
            } else if seconds {
                value * 1000.0
            } else {
                value
            };
            if let Some(delay) = finite_positive(delay) {
                candidates.push(delay);
            }
        }
    }
    candidates.into_iter().reduce(f64::max)
}

fn text_wait_ms(text: &str) -> Option<f64> {
    if let Some(captures) = WAIT_MS_TEXT.captures(text) {
        return captures[1].parse::<f64>().ok().filter(|value| value.is_finite() && *value >= 0.0);
    }
    if let Some(captures) = WAIT_SECONDS_TEXT.captures(text)
        && let Ok(seconds) = captures[1].parse::<f64>()
    {
        return (seconds * 1000.0).is_finite().then_some((seconds * 1000.0).max(0.0));
    }
    crate::usage_limit::retry_hint_ms(text)
}

/// Classify using the actual requested API, never an upstream's echoed API.
/// This is eligibility evidence, not authority to replay a tool or rotate an
/// account. The Host still checks receipts, cancellation and policy.
pub fn classify_retry(message: &AssistantMessage, actual_api: &str) -> RetryClass {
    use flag::*;
    let supported = matches!(actual_api, "openai-completions" | "openai-responses" | "anthropic-messages");
    let evidence = message.failure_evidence.as_ref();
    let text = message.error_message.as_deref().unwrap_or("");
    let status = evidence.and_then(|e| e.status).or(message.error_status);
    let mut flags =
        evidence.filter(|e| e.error_id & CLASS != 0).map_or(0, |e| e.error_id & !STALE_RESPONSES_ITEM & !CLASS);
    let kind = evidence.map(|e| e.kind);
    let code = evidence.and_then(|e| e.code.as_deref());
    if TOKEN_OVERFLOW.is_match(text) || OVERFLOW_NO_BODY.is_match(text) {
        flags |= CONTEXT_OVERFLOW;
    }
    let token_overflow = TOKEN_OVERFLOW.is_match(text);
    if !token_overflow && (PAYLOAD.is_match(text) || (status == Some(413) && flags & CONTEXT_OVERFLOW == 0)) {
        flags |= PAYLOAD_REJECTED;
    }
    if token_overflow {
        flags &= !PAYLOAD_REJECTED;
    }
    if MALFORMED.is_match(text) {
        flags |= MALFORMED_FUNCTION_CALL;
    }
    if FINISH_ERROR.is_match(text) {
        flags |= PROVIDER_FINISH_ERROR;
    }
    if EMPTY.is_match(text) {
        flags |= EMPTY_RESPONSE | TRANSIENT;
    }
    if BLOCKED.is_match(text) {
        flags |= CONTENT_BLOCKED;
    }
    if ACCOUNT_POLICY_PATTERN.is_match(text) || code.is_some_and(|code| ACCOUNT_POLICY_PATTERN.is_match(code)) {
        flags |= ACCOUNT_POLICY | CONTENT_BLOCKED;
    }
    if AUTH.is_match(text) || matches!(status, Some(401 | 403)) {
        flags |= AUTH_FAILED;
    }
    if crate::usage_limit::account_usage_limit(status, None, text, Some(text))
        || code.is_some_and(|code| matches!(code, "usage_limit_reached" | "insufficient_quota" | "usage_not_included"))
    {
        flags |= USAGE_LIMIT;
    }
    if TIMEOUT_PATTERN.is_match(text) {
        flags |= TRANSIENT | TIMEOUT;
    } else if TRANSIENT_PATTERN.is_match(text) {
        flags |= TRANSIENT;
    }
    if CONCURRENT.is_match(text) {
        flags |= TRANSIENT;
    }
    if actual_api == "openai-responses" && STALE.is_match(text) {
        flags |= STALE_RESPONSES_ITEM;
    }
    if status == Some(400) && GENERATION_NAN.is_match(text) {
        flags |= TRANSIENT;
    }
    if matches!(kind, Some(ProviderErrorKind::Transport | ProviderErrorKind::Incomplete)) {
        flags |= TRANSIENT;
    }
    if kind == Some(ProviderErrorKind::Timeout) {
        flags |= TRANSIENT | TIMEOUT;
    }
    if kind == Some(ProviderErrorKind::Aborted) {
        flags |= ABORT;
    }
    if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted)
        && matches!(text, "Request was aborted" | "Request was aborted.")
    {
        flags |= ABORT;
    }
    if kind == Some(ProviderErrorKind::Http) {
        if code.is_some_and(|code| matches!(code, "overloaded_error" | "rate_limit_error")) {
            flags |= TRANSIENT;
        }
        if status == Some(429) && flags & USAGE_LIMIT == 0 || status.is_some_and(|status| status >= 500) {
            flags |= TRANSIENT;
        }
    }
    if LOCAL_PARSE.is_match(text) {
        flags &= !TRANSIENT;
    }
    let retriable_bits =
        TRANSIENT | USAGE_LIMIT | THINKING_LOOP | STALE_RESPONSES_ITEM | PROVIDER_FINISH_ERROR | EMPTY_RESPONSE;
    let immutable_thinking = actual_api == "anthropic-messages"
        && (status == Some(400) || evidence.is_some_and(|e| e.error_id == 400) || text.starts_with("400 "))
        && IMMUTABLE_THINKING.is_match(text);
    let preflight_blocked = text.starts_with("Usage preflight blocked:");
    let retriable = supported
        && message.stop_reason == StopReason::Error
        && !preflight_blocked
        && !immutable_thinking
        && kind != Some(ProviderErrorKind::Config)
        && flags & (CONTENT_BLOCKED | PAYLOAD_REJECTED | CONTEXT_OVERFLOW) == 0
        && flags & (retriable_bits | MALFORMED_FUNCTION_CALL) != 0;
    RetryClass {
        retriable,
        overflow: flags & CONTEXT_OVERFLOW != 0,
        stale_responses: flags & STALE_RESPONSES_ITEM != 0,
        usage_limit: flags & USAGE_LIMIT != 0,
        abort: supported
            && matches!(message.stop_reason, StopReason::Error | StopReason::Aborted)
            && flags & ABORT != 0,
        malformed: flags & MALFORMED_FUNCTION_CALL != 0,
        wait_ms: evidence
            .and_then(|e| e.wait_ms)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .or_else(|| text_wait_ms(text)),
        error_id: if flags == 0 { u32::from(status.unwrap_or(0)) } else { flags | CLASS },
        interrupted_stream: supported
            && matches!(
                kind,
                Some(ProviderErrorKind::Transport | ProviderErrorKind::Incomplete | ProviderErrorKind::Timeout)
            )
            || supported && kind == Some(ProviderErrorKind::Stream) && INTERRUPTED.is_match(text),
        replay_blocked: evidence.is_some_and(|e| e.same_route_blocked),
    }
}

/// Merge transport facts before classifying the public terminal. Existing
/// structured HTTP code/wait facts survive later stream finalization.
pub(crate) fn finalize_failure(
    message: &mut AssistantMessage,
    error: &ProviderError,
    replay_blocked: bool,
    same_route_blocked: bool,
    actual_api: &str,
) {
    let previous = message.failure_evidence.take();
    let mut evidence = ProviderFailureEvidence::from_error(error, replay_blocked);
    evidence.same_route_blocked = same_route_blocked;
    if let Some(previous) = previous {
        evidence.status = evidence.status.or(previous.status);
        evidence.code = previous.code;
        evidence.wait_ms = previous.wait_ms;
        evidence.error_id = previous.error_id;
        evidence.replay_blocked |= previous.replay_blocked;
        evidence.same_route_blocked |= previous.same_route_blocked;
    }
    message.failure_evidence = Some(evidence);
    let classified = classify_retry(message, actual_api);
    message.failure_evidence.as_mut().expect("installed evidence").error_id = classified.error_id;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AssistantMessageEvent;
    use crate::providers::{anthropic, openai_completions, openai_responses};
    use crate::{Context, Model};
    use ara_testkit::{FakeUpstream, Script};
    use serde_json::json;
    use std::time::Duration;
    fn failed(error: ProviderError) -> AssistantMessage {
        let mut message = AssistantMessage::empty("untrusted-echo", "fixture", "m");
        message.stop_reason = error.stop_reason();
        message.error_status = error.status();
        message.error_message = Some(error.to_string());
        finalize_failure(&mut message, &error, false, false, "openai-completions");
        message
    }
    #[test]
    fn typed_transport_status_and_vetoes_survive_roundtrip() {
        let transport = failed(ProviderError::Transport("opaque transport failure".into()));
        let decoded: AssistantMessage = serde_json::from_value(serde_json::to_value(&transport).unwrap()).unwrap();
        let class = classify_retry(&decoded, "openai-completions");
        assert!(class.retriable && class.interrupted_stream);
        for (status, detail) in
            [(413, "payload too large"), (401, "bad credential"), (500, "failed to parse tool call arguments as json")]
        {
            assert!(
                !classify_retry(&failed(ProviderError::Http { status, detail: detail.into() }), "openai-completions")
                    .retriable
            );
        }
        let overflow = classify_retry(
            &failed(ProviderError::Http { status: 413, detail: "maximum context length is 4000 tokens".into() }),
            "openai-completions",
        );
        assert!(overflow.overflow && !overflow.retriable && overflow.error_id & flag::PAYLOAD_REJECTED == 0);
    }
    #[test]
    fn actual_api_controls_stale_response_classification_and_unsupported_routes_do_not_retry() {
        let message = failed(ProviderError::Http { status: 400, detail: "Item with id 'x' not found".into() });
        assert!(classify_retry(&message, "openai-responses").stale_responses);
        assert!(!classify_retry(&message, "openai-completions").stale_responses);
        assert!(!classify_retry(&failed(ProviderError::Transport("network".into())), "unknown-api").retriable);
    }
    #[test]
    fn fixed_wait_headers_choose_the_largest_finite_positive_delay_and_ignore_secrets() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after-ms", "NaN".parse().unwrap());
        headers.insert("retry-after", "2".parse().unwrap());
        headers.insert("x-ratelimit-reset-ms", "3000".parse().unwrap());
        headers.insert("authorization", "secret".parse().unwrap());
        assert_eq!(retry_wait_ms(&headers, 1_700_000_000_000.0), Some(3000.0));
        headers.insert("x-ratelimit-reset", "1700000005".parse().unwrap());
        assert_eq!(retry_wait_ms(&headers, 1_700_000_000_000.0), Some(5000.0));
        headers.insert("retry-after", "infinity".parse().unwrap());
        headers.remove("x-ratelimit-reset");
        headers.remove("x-ratelimit-reset-ms");
        assert_eq!(retry_wait_ms(&headers, 1_700_000_000_000.0), None);
    }
    #[test]
    fn replay_blocked_usage_and_abort_remain_explicit_host_inputs() {
        let mut message = failed(ProviderError::Http { status: 429, detail: "usage_limit_reached".into() });
        message.failure_evidence.as_mut().unwrap().replay_blocked = true;
        message.failure_evidence.as_mut().unwrap().same_route_blocked = true;
        let class = classify_retry(&message, "openai-completions");
        assert!(class.usage_limit && class.replay_blocked);
        assert!(classify_retry(&failed(ProviderError::Aborted), "openai-completions").abort);
        message.error_message = Some("Usage preflight blocked: try again".into());
        assert!(!classify_retry(&message, "openai-completions").retriable);
    }
    #[test]
    fn numeric_status_fallback_stays_numeric_after_reclassification() {
        let message = failed(ProviderError::Http { status: 400, detail: "request declined".into() });
        assert_eq!(message.failure_evidence.as_ref().unwrap().error_id, 400);
        let class = classify_retry(&message, "openai-completions");
        assert_eq!(class.error_id, 400);
        assert!(!class.retriable);
    }
    #[test]
    fn structured_stream_usage_code_survives_terminal_and_json_roundtrip() {
        for envelope in [
            serde_json::json!({"type":"error","error":{"type":"usage_limit_reached","message":"request declined"}}),
            serde_json::json!({"type":"response.failed","response":{"error":{"code":"usage_limit_reached","message":"request declined"}}}),
        ] {
            let error = ProviderError::Stream("request declined".into());
            let mut message = failed(error.clone());
            message.failure_evidence = Some(ProviderFailureEvidence::from_error_envelope(&error, &envelope, false));
            finalize_failure(&mut message, &error, false, false, "openai-responses");
            let decoded: AssistantMessage = serde_json::from_value(serde_json::to_value(&message).unwrap()).unwrap();
            let class = classify_retry(&decoded, "openai-responses");
            assert!(class.usage_limit && class.replay_blocked);
            assert_eq!(decoded.failure_evidence.unwrap().code.as_deref(), Some("usage_limit_reached"));
        }
    }
    #[test]
    fn exact_immutable_thinking_veto_and_generic_abort_have_narrow_scope() {
        let immutable = failed(ProviderError::Http { status:400,
            detail:"messages.2.content.1: thinking blocks in the latest assistant message cannot be modified; retry your request".into() });
        assert!(!classify_retry(&immutable, "anthropic-messages").retriable);
        assert!(classify_retry(&immutable, "openai-completions").retriable);
        let mut message = AssistantMessage::empty("anthropic-messages", "fixture", "m");
        message.stop_reason = StopReason::Aborted;
        message.error_message = Some("User interrupt".into());
        assert!(!classify_retry(&message, "anthropic-messages").abort);
        message.error_message = Some("Request was aborted.".into());
        assert!(classify_retry(&message, "anthropic-messages").abort);
        assert!(!classify_retry(&message, "unsupported-api").abort);
    }
    #[test]
    fn provider_wait_ceiling_does_not_veto_the_session_wait_policy() {
        let error = ProviderError::Http { status: 429, detail: "rate limit; retry after wait".into() };
        let mut message = failed(error.clone());
        let mut evidence = ProviderFailureEvidence::from_error(&error, true);
        evidence.same_route_blocked = false;
        evidence.wait_ms = Some(350_000.0);
        message.failure_evidence = Some(evidence);
        finalize_failure(&mut message, &error, true, false, "openai-completions");
        let class = classify_retry(&message, "openai-completions");
        assert!(class.retriable && !class.replay_blocked);
        assert_eq!(class.wait_ms, Some(350_000.0));
        assert!(message.failure_evidence.as_ref().unwrap().replay_blocked);
        message.failure_evidence.as_mut().unwrap().same_route_blocked = true;
        assert!(classify_retry(&message, "openai-completions").replay_blocked);
    }

    #[tokio::test]
    async fn actual_http_header_facts_survive_body_stall_watchdogs_on_all_supported_routes() {
        for api in ["openai-completions", "openai-responses", "anthropic-messages"] {
            for (headers, wait_ms, same_route_blocked) in [
                (json!({"rate_limit_type":"max_parallel_requests"}), None, true),
                (json!({"retry-after-ms":"120000"}), Some(120_000.0), false),
            ] {
                let script: Script = serde_json::from_value(json!({"responses":[
                    {"status":429,"headers":headers,"events":[],"end":"hang"},
                    {"status":400,"body":"a second request must never be sent"}
                ]}))
                .unwrap();
                let upstream = FakeUpstream::start(script, None).await.unwrap();
                let model = Model {
                    id: "fixture".into(),
                    api: api.into(),
                    provider: "fixture".into(),
                    base_url: upstream.base_url(),
                    reasoning: false,
                    max_tokens: None,
                    tokenizer: None,
                };
                let retry = openai_completions::RetryPolicy {
                    max_attempts: 1,
                    base_delay: Duration::from_millis(1),
                    max_delay: Duration::from_secs(1),
                };
                let first_event_timeout = Some(Duration::from_millis(100));
                let context = Context::default();
                let client = reqwest::Client::new();
                let mut stream = match api {
                    "openai-completions" => openai_completions::stream(
                        client,
                        model,
                        context,
                        openai_completions::StreamOptions { retry, first_event_timeout, ..Default::default() },
                    ),
                    "openai-responses" => openai_responses::stream(
                        client,
                        model,
                        context,
                        openai_responses::StreamOptions { retry, first_event_timeout, ..Default::default() },
                    ),
                    _ => anthropic::stream(
                        client,
                        model,
                        context,
                        anthropic::StreamOptions {
                            retry,
                            first_event_timeout,
                            api_key: Some("fake-header-stall-key".into()),
                            ..Default::default()
                        },
                    ),
                };
                let events = tokio::time::timeout(Duration::from_secs(2), async {
                    let mut events = Vec::new();
                    while let Some(event) = stream.recv().await {
                        events.push(event);
                    }
                    events
                })
                .await
                .expect("body stall must finish within the bounded watchdog");
                assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1, "{api}");
                let AssistantMessageEvent::Error { error: message, .. } = events.last().unwrap() else {
                    panic!("{api}: body stall must end as a provider error");
                };
                let evidence =
                    message.failure_evidence.as_ref().expect("header facts must survive the dropped POST future");
                assert_eq!(evidence.kind, ProviderErrorKind::Timeout, "{api}");
                assert_eq!(evidence.status, Some(429), "{api}");
                assert_eq!(evidence.wait_ms, wait_ms, "{api}");
                assert!(evidence.replay_blocked, "{api}: both header cases stop provider replay");
                assert_eq!(evidence.same_route_blocked, same_route_blocked, "{api}");
                let class = classify_retry(message, api);
                assert_eq!(class.wait_ms, wait_ms, "{api}");
                assert_eq!(class.replay_blocked, same_route_blocked, "{api}");
                assert_eq!(upstream.served(), 1, "{api}: header guard survives the body stall");
            }
        }
    }
}
