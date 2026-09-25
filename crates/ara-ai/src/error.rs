//! Provider error classification (subset of OMP `packages/ai/src/error/`).
//!
//! A provider never returns `Err` to the loop: failures become a terminal
//! `error` event whose assistant message carries `stopReason` `error` or
//! `aborted`, `errorMessage` and, for HTTP failures, `errorStatus`.

use crate::types::StopReason;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ProviderError {
    /// Non-2xx HTTP response. `detail` is the parsed error envelope message.
    #[error("{status} {detail}")]
    Http { status: u16, detail: String },
    /// Structured in-band stream error envelope.
    #[error("{0}")]
    Stream(String),
    /// Transport failure (connect, reset, body read).
    #[error("{0}")]
    Transport(String),
    /// The stream ended without a terminal signal.
    #[error("{0}")]
    Incomplete(String),
    /// First-event or idle timeout.
    #[error("{0}")]
    Timeout(String),
    /// Configuration problem detected before the request.
    #[error("{0}")]
    Config(String),
    /// Caller cancellation.
    #[error("Request was aborted")]
    Aborted,
}

impl ProviderError {
    pub fn stop_reason(&self) -> StopReason {
        match self {
            ProviderError::Aborted => StopReason::Aborted,
            _ => StopReason::Error,
        }
    }

    pub fn status(&self) -> Option<u16> {
        match self {
            ProviderError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Transient failures a caller may retry before output was committed.
    pub fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Http { status, .. } => matches!(status, 408 | 409 | 425 | 429 | 500..=599),
            ProviderError::Transport(_) | ProviderError::Timeout(_) | ProviderError::Incomplete(_) => true,
            _ => false,
        }
    }
}

/// Extract a readable detail from an OpenAI-style error body
/// (`{"error": {"message": ...}}`, `{"message": ...}` or raw text).
pub fn parse_error_envelope(body: &str) -> String {
    let trimmed = body.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed)
        && let Some(message) = envelope_message(&value)
    {
        return message;
    }
    if trimmed.is_empty() { "(empty response body)".to_string() } else { truncate(trimmed, 2000) }
}

pub fn envelope_message(value: &Value) -> Option<String> {
    match value.get("error") {
        Some(Value::String(s)) => return Some(s.clone()),
        Some(Value::Object(err)) => {
            if let Some(Value::String(m)) = err.get("message") {
                return Some(m.clone());
            }
            return Some(Value::Object(err.clone()).to_string());
        }
        _ => {}
    }
    value.get("message").and_then(Value::as_str).map(str::to_string)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        let head: String = text.chars().take(max).collect();
        format!("{head}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_variants() {
        assert_eq!(parse_error_envelope(r#"{"error":{"message":"bad key","code":401}}"#), "bad key");
        assert_eq!(parse_error_envelope(r#"{"error":"nope"}"#), "nope");
        assert_eq!(parse_error_envelope(r#"{"message":"flat"}"#), "flat");
        assert_eq!(parse_error_envelope("plain text"), "plain text");
        assert_eq!(parse_error_envelope(""), "(empty response body)");
    }

    #[test]
    fn classification() {
        let e = ProviderError::Http { status: 429, detail: "slow down".into() };
        assert_eq!(e.to_string(), "429 slow down");
        assert!(e.is_retryable());
        assert!(!ProviderError::Http { status: 401, detail: String::new() }.is_retryable());
        assert_eq!(ProviderError::Aborted.stop_reason(), StopReason::Aborted);
        assert_eq!(ProviderError::Aborted.to_string(), "Request was aborted");
    }
}
