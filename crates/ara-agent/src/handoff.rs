//! Native handoff oneshot from fixed OMP `compaction.ts:1045-1122` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; THIRD_PARTY_NOTICES.md).
//! Hosts assemble the live Context and own side-session/cache routing, empty
//! document policy, file-operation preparation and history commits. This
//! module only requests a completion; it never executes returned tool calls.

use std::time::Instant;

use ara_ai::retry_classification::classify_retry;
use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, CallOptions, Context, Model, ModelProvider, StopReason,
    ToolChoice,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::compaction::{AcceptedSummary, SummaryCallError, SummaryCallErrorKind, SummaryInvocationReceipt};

#[path = "handoff_file_ops.rs"]
mod file_ops;
pub use file_ops::{HandoffFileDetails, PreparedHandoffSummary, prepare_handoff_summary};

const HANDOFF_DOCUMENT: &str = include_str!("../prompts/handoff-document.md");

/// Fixed native template, including the post-render formatter and the
/// additionalFocus branch. Empty focus follows the upstream no-focus path.
pub fn render_handoff_prompt(custom_instructions: Option<&str>) -> String {
    let context = match custom_instructions.filter(|focus| !focus.is_empty()) {
        Some(focus) => json!({"additionalFocus": focus}),
        None => json!({}),
    };
    ara_prompt::render(HANDOFF_DOCUMENT, &context).expect("fixed handoff template is valid")
}

// JavaScript's non-Unicode /\bword\b/i uses ASCII word boundaries. In
// particular, `unsupported`, `automatic` and `my_tool_choice` do not match.
fn contains_native_word(text: &str, word: &[u8]) -> bool {
    let bytes = text.as_bytes();
    let word_char = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    bytes.windows(word.len()).enumerate().any(|(index, candidate)| {
        candidate.eq_ignore_ascii_case(word)
            && (index == 0 || !word_char(bytes[index - 1]))
            && bytes.get(index + word.len()).is_none_or(|byte| !word_char(*byte))
    })
}

fn should_retry_auto(message: &AssistantMessage) -> bool {
    if message.error_status != Some(400) {
        return false;
    }
    let diagnostic = message.error_message.as_deref().unwrap_or("");
    [b"tool_choice".as_slice(), b"auto".as_slice(), b"supported".as_slice()]
        .into_iter()
        .all(|word| contains_native_word(diagnostic, word))
}

fn invocation(
    kind: Option<SummaryCallErrorKind>,
    terminal: Option<(StopReason, &AssistantMessage)>,
    actual_api: &str,
    auto_retry: bool,
) -> SummaryInvocationReceipt {
    SummaryInvocationReceipt {
        window_source_entry_ids: Vec::new(),
        usage: terminal.map(|(_, message)| message.usage.clone()),
        response_id: terminal.and_then(|(_, message)| message.response_id.clone()),
        stop_reason: terminal.map(|(reason, _)| reason),
        provider_status: terminal.and_then(|(_, message)| message.error_status),
        error_kind: kind,
        classification: terminal.filter(|_| kind.is_some()).map(|(_, message)| classify_retry(message, actual_api)),
        oneshot_retry_eligible: auto_retry,
        oneshot_retry_wait_ms: None,
        overflow_replanned: false,
    }
}

fn failed(
    kind: SummaryCallErrorKind,
    terminal: Option<(StopReason, &AssistantMessage)>,
    _actual_api: &str,
) -> SummaryCallError {
    let diagnostic = terminal.and_then(|(_, message)| message.error_message.as_deref());
    SummaryCallError {
        kind,
        usage: terminal.map(|(_, message)| Box::new(message.usage.clone())),
        provider_status: terminal.and_then(|(_, message)| message.error_status),
        stop_reason: terminal.map(|(reason, _)| reason),
        provider_message: (kind == SummaryCallErrorKind::ProviderError).then(|| {
            format!(
                "Handoff generation failed: {}",
                diagnostic.filter(|text| !text.is_empty()).unwrap_or("Unknown error")
            )
            .chars()
            .take(512)
            .collect()
        }),
        invocations: Vec::new(),
    }
}

async fn attempt(
    context: &Context,
    model: &Model,
    provider: &dyn ModelProvider,
    tool_choice: ToolChoice,
    max_tokens: Option<u64>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<(StopReason, AssistantMessage), SummaryCallError> {
    if cancel.is_cancelled() {
        return Err(failed(SummaryCallErrorKind::Cancelled, None, &model.api));
    }
    if Instant::now() >= deadline {
        return Err(failed(SummaryCallErrorKind::Deadline, None, &model.api));
    }
    let provider_cancel = cancel.child_token();
    let _provider_guard = provider_cancel.clone().drop_guard();
    let mut events = provider.stream(
        model,
        context,
        CallOptions {
            cancel: provider_cancel.clone(),
            tool_choice: Some(tool_choice),
            max_tokens,
            temperature: None,
            loop_guard: None,
            on_response: None,
        },
    );
    let timeout = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline));
    tokio::pin!(timeout);
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let mut failure = failed(SummaryCallErrorKind::Cancelled, None, &model.api);
                failure.invocations.push(invocation(Some(failure.kind), None, &model.api, false));
                return Err(failure);
            }
            _ = &mut timeout => {
                let mut failure = failed(SummaryCallErrorKind::Deadline, None, &model.api);
                failure.invocations.push(invocation(Some(failure.kind), None, &model.api, false));
                return Err(failure);
            }
            event = events.recv() => event,
        };
        let terminal = match event {
            Some(AssistantMessageEvent::Done { reason, message }) => (reason, message),
            Some(AssistantMessageEvent::Error { reason, error }) => (reason, error),
            Some(_) => continue,
            None => {
                let mut failure = failed(SummaryCallErrorKind::StreamEndedWithoutTerminal, None, &model.api);
                failure.invocations.push(invocation(Some(failure.kind), None, &model.api, false));
                return Err(failure);
            }
        };
        let interruption = if cancel.is_cancelled() {
            Some(SummaryCallErrorKind::Cancelled)
        } else if Instant::now() >= deadline {
            Some(SummaryCallErrorKind::Deadline)
        } else {
            None
        };
        if let Some(kind) = interruption {
            let mut failure = failed(kind, Some((terminal.0, &terminal.1)), &model.api);
            failure.invocations.push(invocation(Some(kind), Some((terminal.0, &terminal.1)), &model.api, false));
            return Err(failure);
        }
        return Ok(terminal);
    }
}

/// Generate from the exact host-built live Context, without modifying it.
/// The Host has already appended `render_handoff_prompt` as the trailing User.
/// Thinking/service-tier/cache settings remain on the host's bound provider.
///
/// Unlike soft compaction, native handoff extracts Text from ToolUse/Length
/// responses and allows empty text: the Host distinguishes manual failure
/// from best-effort automatic fallback. No non-text block is returned or run.
pub async fn generate_handoff_from_context(
    context: &Context,
    model: &Model,
    provider: &dyn ModelProvider,
    max_tokens: Option<u64>,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    let mut invocations = Vec::new();
    for index in 0..2 {
        let choice = if index == 0 { ToolChoice::None } else { ToolChoice::Auto };
        let (reason, response) = match attempt(context, model, provider, choice, max_tokens, deadline, cancel).await {
            Ok(terminal) => terminal,
            Err(mut failure) => {
                invocations.append(&mut failure.invocations);
                failure.invocations = invocations;
                return Err(failure);
            }
        };
        let kind = if reason == StopReason::Aborted || response.stop_reason == StopReason::Aborted {
            Some(SummaryCallErrorKind::Cancelled)
        } else if reason == StopReason::Error || response.stop_reason == StopReason::Error {
            Some(SummaryCallErrorKind::ProviderError)
        } else {
            None
        };
        let retry = index == 0 && response.stop_reason == StopReason::Error && should_retry_auto(&response);
        invocations.push(invocation(kind, Some((reason, &response)), &model.api, retry));
        if let Some(kind) = kind {
            if retry && kind == SummaryCallErrorKind::ProviderError {
                continue;
            }
            let mut failure = failed(kind, Some((reason, &response)), &model.api);
            failure.invocations = invocations;
            return Err(failure);
        }
        let text = response
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(part) => Some(part.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let interruption = if cancel.is_cancelled() {
            Some(SummaryCallErrorKind::Cancelled)
        } else if Instant::now() >= deadline {
            Some(SummaryCallErrorKind::Deadline)
        } else {
            None
        };
        if let Some(kind) = interruption {
            invocations.last_mut().expect("actual handoff invocation").error_kind = Some(kind);
            let mut failure = failed(kind, Some((reason, &response)), &model.api);
            failure.invocations = invocations;
            return Err(failure);
        }
        return Ok(AcceptedSummary {
            text,
            terminal_reason: reason,
            window_source_entry_ids: Vec::new(),
            model_id: model.id.clone(),
            response_id: response.response_id,
            usage: response.usage,
            duration_ms: response.duration,
            ttft_ms: response.ttft,
            invocations,
        });
    }
    unreachable!("handoff permits at most one auto-only retry")
}
