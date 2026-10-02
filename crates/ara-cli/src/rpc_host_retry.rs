//! Fixed OMP 596f2da turn-recovery.ts:1213-1425 and retry-fallback-chains.ts:72-77.
//! Host decisions use terminal failure facts plus the actual retained receipts.
use ara_ai::retry_classification::RetryClass;
use ara_ai::{AssistantBlock, AssistantMessage, Message};

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
    let error = message.error_message.as_deref().unwrap_or("").to_ascii_lowercase();
    let bounded = match message.provider.as_str() {
        "openrouter" => error.match_indices("server_error:").any(|(index, marker)| {
            // Native /i is ASCII case folding for this non-Unicode pattern;
            // reuse JS whitespace rather than Rust regex's broader \s set.
            let tail = ara_prompt::js::trim_start(&error[index + marker.len()..]);
            let Some(tail) = tail.strip_prefix("stream closed with reason:") else { return false };
            ara_prompt::js::trim_start(tail).starts_with("error")
        }),
        "github-copilot" if message.model == "grok-4.6" && message.api == "openai-responses" => {
            error.contains("openai responses stream closed before a terminal response event was received")
        }
        _ => false,
    };
    // RetryPolicy rejects nonfinite settings; keep zero, negative and fractional
    // values unchanged when they are already below the native one-retry cap.
    if bounded { configured.min(1.0) } else { configured }
}

pub(super) fn disposition(
    message: &AssistantMessage,
    messages: &[Message],
    class: &RetryClass,
) -> Option<RetryDisposition> {
    if class.overflow || class.replay_blocked {
        return None;
    }
    let index = messages.iter().rposition(|candidate| candidate.as_assistant() == Some(message))?;
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
        return (class.retriable || class.abort).then_some(RetryDisposition::RemoveFailed);
    }
    let mut received = std::collections::HashSet::new();
    let mut all_unexecuted = true;
    for candidate in &messages[index + 1..] {
        let Message::ToolResult(result) = candidate else { return None };
        if !calls.iter().any(|call| call.id == result.tool_call_id && call.name == result.tool_name) {
            return None;
        }
        let details = result.details.as_ref();
        if details.is_some_and(|details| details["executed"] == "unknown" || details["panicked"] == true) {
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
    if calls.iter().any(|call| !received.contains(call.id.as_str())) {
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
    (all_unexecuted && !unsafe_output && (class.retriable || class.malformed))
        .then_some(RetryDisposition::PreserveFailed)
}

pub(super) fn backoff_ms(base: f64, attempt: usize, random: f64) -> f64 {
    if base <= 0.0 {
        return 0.0;
    }
    (base.max(0.0) * 2f64.powi(attempt.saturating_sub(1).min(i32::MAX as usize) as i32)).min(8000.0)
        * (1.0 - random.clamp(0.0, 1.0) * 0.25)
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
    use ara_ai::{AssistantBlock, ImageContent, JsonObject, StopReason, ToolCall, ToolResultMessage, UserMessage};
    use serde_json::json;

    fn call_turn() -> (AssistantMessage, Vec<Message>) {
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
        message.stop_reason = StopReason::Error;
        message.content.push(AssistantBlock::ToolCall(ToolCall {
            id: "one".into(),
            name: "write".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
        }));
        let result = Message::ToolResult(ToolResultMessage {
            tool_call_id: "one".into(),
            tool_name: "write".into(),
            content: vec![],
            details: Some(json!({"__synthetic":true,"executed":false})),
            is_error: true,
            timestamp: 3,
        });
        let messages = vec![Message::User(UserMessage::text("request")), Message::Assistant(message.clone()), result];
        (message, messages)
    }

    #[test]
    fn false_is_positive_proof_unknown_or_duplicate_actual_result_vetoes() {
        let (message, mut messages) = call_turn();
        let class = RetryClass { retriable: true, ..Default::default() };
        assert_eq!(disposition(&message, &messages, &class), Some(RetryDisposition::PreserveFailed));
        let Message::ToolResult(result) = &mut messages[2] else { unreachable!() };
        result.details = Some(json!({"__synthetic":true,"executed":"unknown"}));
        assert_eq!(disposition(&message, &messages, &class), None);
        let (message, mut messages) = call_turn();
        let mut actual = messages[2].clone();
        let Message::ToolResult(result) = &mut actual else { unreachable!() };
        result.details = None;
        messages.push(actual);
        assert_eq!(disposition(&message, &messages, &class), None);
        assert_eq!(
            disposition(&message, &messages, &RetryClass { interrupted_stream: true, ..class }),
            Some(RetryDisposition::PreserveFailed)
        );
    }

    #[test]
    fn visible_text_image_and_newer_user_never_replay_an_empty_failed_turn() {
        let class = RetryClass { retriable: true, ..Default::default() };
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
        message.stop_reason = StopReason::Error;
        let user = Message::User(UserMessage::text("request"));
        assert_eq!(
            disposition(&message, &[user.clone(), Message::Assistant(message.clone())], &class),
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
            assert_eq!(disposition(&message, &[user.clone(), Message::Assistant(message.clone())], &class), None);
        }
        message.content.clear();
        assert_eq!(disposition(&message, &[user.clone(), Message::Assistant(message.clone()), user], &class), None);
    }

    #[test]
    fn source_backoff_has_exact_cap_and_jitter_edges() {
        assert_eq!(backoff_ms(500.0, 1, 0.0), 500.0);
        assert_eq!(backoff_ms(500.0, 1, 1.0), 375.0);
        assert_eq!(backoff_ms(500.0, 9, 0.0), 8000.0);
        assert_eq!(backoff_ms(500.0, 9, 1.0), 6000.0);
        assert_eq!(backoff_ms(-1.0, 10, 0.0), 0.0);
    }
}
