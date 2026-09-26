//! Replay-safe retries for a provider stream before it reaches the Agent.
//!
//! Fixed OMP source: `packages/ai/src/utils/empty-completion-retry.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. An attempt becomes
//! irrevocable as soon as it emits meaningful content or a tool-call event.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::event::{AssistantMessageEvent, AssistantStream, EventSink};
use crate::types::{AssistantBlock, AssistantMessage, Model, StopReason};

const MAX_EMPTY_RETRIES: u32 = 2;
const MAX_ERROR_RETRIES: u32 = 1;
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_BUFFERED_MARKERS: usize = 64;

/// The private error side channel preserves a typed transport cause; the
/// public event protocol deliberately exposes only the final attempt.
pub(crate) struct AttemptStream {
    pub events: AssistantStream,
    pub error: Arc<Mutex<Option<ProviderError>>>,
}

fn meaningful(event: &AssistantMessageEvent) -> bool {
    match event {
        AssistantMessageEvent::TextDelta { delta, .. }
        | AssistantMessageEvent::ThinkingDelta { delta, .. }
        | AssistantMessageEvent::ToolcallDelta { delta, .. } => !delta.is_empty(),
        AssistantMessageEvent::TextEnd { content, .. } | AssistantMessageEvent::ThinkingEnd { content, .. } => {
            !content.is_empty()
        }
        AssistantMessageEvent::ToolcallStart { .. } | AssistantMessageEvent::ToolcallEnd { .. } => true,
        _ => false,
    }
}

fn visible(message: &AssistantMessage) -> bool {
    message.content.iter().any(|block| match block {
        AssistantBlock::Image(_) | AssistantBlock::ToolCall(_) => true,
        AssistantBlock::Text(text) => text.text.chars().any(|c| !c.is_whitespace()),
        AssistantBlock::Thinking(_) | AssistantBlock::RedactedThinking { .. } => false,
    })
}

fn retryable_started_stream_error(error: &ProviderError) -> bool {
    match error {
        ProviderError::Http { status, .. } => matches!(status, 408 | 429 | 500..=599),
        _ => error.is_retryable(),
    }
}

async fn send(sink: &EventSink, event: AssistantMessageEvent, cancel: &CancellationToken) -> bool {
    // A ready send wins even when cancellation has fired. A full channel does
    // not hold a cancelled provider task alive waiting for a slow consumer.
    tokio::select! {
        biased;
        sent = sink.push(event) => sent,
        _ = cancel.cancelled() => false,
    }
}

async fn flush(
    sink: &EventSink,
    buffered: &mut Vec<AssistantMessageEvent>,
    sent_start: &mut bool,
    cancel: &CancellationToken,
) -> bool {
    for event in buffered.drain(..) {
        if matches!(event, AssistantMessageEvent::Start { .. }) {
            *sent_start = true;
        }
        if !send(sink, event, cancel).await {
            return false;
        }
    }
    true
}

async fn finish(
    sink: &EventSink,
    buffered: &mut Vec<AssistantMessageEvent>,
    sent_start: &mut bool,
    terminal: AssistantMessageEvent,
    model: &Model,
    cancel: &CancellationToken,
) {
    if !flush(sink, buffered, sent_start, cancel).await {
        if cancel.is_cancelled() {
            send_abort(sink, model, Some(terminal.partial().clone())).await;
        }
        return;
    }
    if !*sent_start
        && matches!(terminal, AssistantMessageEvent::Done { .. })
        && !send(sink, AssistantMessageEvent::Start { partial: terminal.partial().clone() }, cancel).await
    {
        if cancel.is_cancelled() {
            send_abort(sink, model, Some(terminal.partial().clone())).await;
        }
        return;
    }
    if !send(sink, terminal.clone(), cancel).await && cancel.is_cancelled() {
        send_abort(sink, model, Some(terminal.partial().clone())).await;
    }
}

fn aborted(model: &Model, partial: Option<AssistantMessage>) -> AssistantMessageEvent {
    let mut message = partial.unwrap_or_else(|| AssistantMessage::empty(&model.api, &model.provider, &model.id));
    message.stop_reason = StopReason::Aborted;
    message.error_message = Some(ProviderError::Aborted.to_string());
    AssistantMessageEvent::Error { reason: StopReason::Aborted, error: message }
}

async fn send_abort(sink: &EventSink, model: &Model, partial: Option<AssistantMessage>) {
    // The active HTTP attempt is cancelled before this call. If the consumer
    // is behind a full channel, retain the terminal until it drains or drops.
    let _ = sink.push(aborted(model, partial)).await;
}

/// Wrap one-attempt streams. Buffered lifecycle markers from discarded
/// attempts are never emitted. The first meaningful event flushes them and
/// every later event goes through immediately; that attempt can never retry.
pub(crate) fn with_replay_safe_stream_retry<F>(
    model: Model,
    cancel: CancellationToken,
    accept_empty_response: bool,
    mut attempt: F,
) -> AssistantStream
where
    F: FnMut(CancellationToken) -> AttemptStream + Send + 'static,
{
    let (sink, rx) = EventSink::channel();
    tokio::spawn(async move {
        let mut empty_retries = 0u32;
        let mut error_retries = 0u32;
        loop {
            if cancel.is_cancelled() {
                send_abort(&sink, &model, None).await;
                return;
            }
            let attempt_cancel = cancel.child_token();
            let _attempt_guard = attempt_cancel.clone().drop_guard();
            let AttemptStream { mut events, error } = attempt(attempt_cancel.clone());
            let mut buffered = Vec::new();
            let mut sent_start = false;
            let mut saw_start = false;
            let mut committed = accept_empty_response;
            let mut last_partial = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let terminal = loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        attempt_cancel.cancel();
                        send_abort(&sink, &model, Some(last_partial)).await;
                        return;
                    }
                    event = events.recv() => event,
                };
                let Some(event) = next else {
                    let mut message = last_partial;
                    message.stop_reason = StopReason::Error;
                    message.error_message = Some("Provider stream ended without a terminal event".into());
                    finish(
                        &sink,
                        &mut buffered,
                        &mut sent_start,
                        AssistantMessageEvent::Error { reason: StopReason::Error, error: message },
                        &model,
                        &cancel,
                    )
                    .await;
                    return;
                };
                last_partial = event.partial().clone();
                if matches!(event, AssistantMessageEvent::Start { .. }) {
                    saw_start = true;
                }
                if event.is_terminal() {
                    break event;
                }
                if !committed && !meaningful(&event) && buffered.len() < MAX_BUFFERED_MARKERS {
                    buffered.push(event);
                    continue;
                }
                committed = true;
                if !flush(&sink, &mut buffered, &mut sent_start, &cancel).await {
                    attempt_cancel.cancel();
                    if cancel.is_cancelled() {
                        send_abort(&sink, &model, Some(last_partial)).await;
                    }
                    return;
                }
                if matches!(event, AssistantMessageEvent::Start { .. }) {
                    sent_start = true;
                }
                if !sent_start {
                    sent_start = true;
                    if !send(&sink, AssistantMessageEvent::Start { partial: last_partial.clone() }, &cancel).await {
                        attempt_cancel.cancel();
                        if cancel.is_cancelled() {
                            send_abort(&sink, &model, Some(last_partial)).await;
                        }
                        return;
                    }
                }
                if !send(&sink, event, &cancel).await {
                    attempt_cancel.cancel();
                    if cancel.is_cancelled() {
                        send_abort(&sink, &model, Some(last_partial)).await;
                    }
                    return;
                }
            };

            let retry_empty = matches!(&terminal, AssistantMessageEvent::Done { reason: StopReason::Stop, message }
                if !committed
                    && !accept_empty_response
                    && message.stop_details.as_ref().and_then(|v| v.get("type")).and_then(|v| v.as_str()) != Some("pause_turn")
                    && message.error_message.is_none()
                    && message.usage.output.is_some_and(|count| count <= 1)
                    && !visible(message)
                    && empty_retries < MAX_EMPTY_RETRIES);
            let typed_error = error.lock().unwrap().clone();
            // Pre-response HTTP retries and their admission/Retry-After limits
            // belong to post_with_retry. Only a started 2xx stream is eligible
            // for this additional replay-safe provider-error retry.
            let retry_error = matches!(&terminal, AssistantMessageEvent::Error { reason: StopReason::Error, .. })
                && !committed
                && saw_start
                && typed_error.as_ref().is_some_and(retryable_started_stream_error)
                && error_retries < MAX_ERROR_RETRIES;
            let delay = if retry_empty {
                let delay = BASE_DELAY.saturating_mul(2u32.saturating_pow(empty_retries));
                empty_retries += 1;
                Some(delay)
            } else if retry_error {
                let delay = BASE_DELAY.saturating_mul(2u32.saturating_pow(error_retries));
                error_retries += 1;
                Some(delay)
            } else {
                None
            };
            if let Some(delay) = delay {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        attempt_cancel.cancel();
                        send_abort(&sink, &model, None).await;
                        return;
                    }
                    _ = tokio::time::sleep(delay) => continue,
                }
            }
            if cancel.is_cancelled() {
                attempt_cancel.cancel();
                send_abort(&sink, &model, Some(last_partial)).await;
                return;
            }
            finish(&sink, &mut buffered, &mut sent_start, terminal, &model, &cancel).await;
            return;
        }
    });
    rx
}
