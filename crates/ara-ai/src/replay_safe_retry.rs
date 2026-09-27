//! Replay-safe retries for a provider stream before it reaches the Agent.
//!
//! Fixed OMP source: `packages/ai/src/utils/empty-completion-retry.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. An attempt becomes
//! irrevocable as soon as it emits meaningful content or a tool-call event.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use tokio_util::sync::CancellationToken;

use crate::error::ProviderError;
use crate::event::{AssistantMessageEvent, AssistantStream, EventSink};
use crate::types::{AssistantBlock, AssistantMessage, Model, RetryAccounting, RetryAttempt, StopReason};
use crate::usage_limit::account_usage_limit;

const MAX_EMPTY_RETRIES: u32 = 2;
const MAX_ERROR_RETRIES: u32 = 1;
const BASE_DELAY: Duration = Duration::from_millis(500);
const MAX_BUFFERED_MARKERS: usize = 64;

/// The private error side channel preserves a typed transport cause; the
/// public event protocol deliberately exposes only the final attempt.
pub(crate) struct AttemptStream {
    pub events: AssistantStream,
    pub error: Arc<Mutex<Option<AttemptError>>>,
}

#[derive(Clone)]
pub(crate) struct AttemptError {
    pub cause: ProviderError,
    pub retry_blocked: bool,
}

fn meaningful(event: &AssistantMessageEvent) -> bool {
    match event {
        AssistantMessageEvent::TextDelta { delta, .. }
        | AssistantMessageEvent::ThinkingDelta { delta, .. }
        | AssistantMessageEvent::ToolcallDelta { delta, .. } => !delta.is_empty(),
        AssistantMessageEvent::TextEnd { content, .. } | AssistantMessageEvent::ThinkingEnd { content, .. } => {
            !content.is_empty()
        }
        AssistantMessageEvent::ImageEnd { .. }
        | AssistantMessageEvent::ToolcallStart { .. }
        | AssistantMessageEvent::ToolcallEnd { .. } => true,
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

fn retryable_uncommitted_error(error: &AttemptError) -> bool {
    if error.retry_blocked {
        return false;
    }
    match &error.cause {
        ProviderError::Http { status, .. } => matches!(status, 408 | 429 | 500..=599),
        ProviderError::Stream(detail) => {
            let status = status_in_stream_detail(detail);
            !status.is_some_and(|status| (400..500).contains(&status) && !matches!(status, 408 | 429))
                && !account_usage_limit(status, None, detail, Some(detail))
                && (matches!(status, Some(408 | 429 | 500..=599)) || transient_statusless_stream_message(detail))
        }
        _ => error.cause.is_retryable(),
    }
}

fn status_in_stream_detail(detail: &str) -> Option<u16> {
    // Fixed OMP `error/flags.ts::STATUS_MESSAGE_PATTERNS`. An in-band envelope
    // may have no numeric code field yet still carry an HTTP status in text.
    static PATTERNS: std::sync::LazyLock<[Regex; 5]> = std::sync::LazyLock::new(|| {
        [
            r"(?i)\bstatus(?:_code)?[:=]\s*(\d{3})\b",
            r"(?i)\bstatus\s+(\d{3})\b",
            r"(?i)\bHTTP\s+(\d{3})\b",
            r"(?i)\b(?:error|failed)\s*[:=]?\s*(\d{3})\b",
            r"(?:^|\s)(\d{3})\s+(?:[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*)",
        ]
        .map(|pattern| Regex::new(pattern).expect("valid fixed OMP status regex"))
    });
    PATTERNS.iter().find_map(|pattern| {
        pattern
            .captures(detail)
            .and_then(|match_| match_.get(1)?.as_str().parse::<u16>().ok())
            .filter(|status| (100..=599).contains(status))
    })
}

fn transient_statusless_stream_message(detail: &str) -> bool {
    // Fixed OMP `error/flags.ts` transient text/stream patterns,
    // `error/retryable.ts::PROVIDER_TRANSIENT_EXTRA_PATTERN`, and the socket
    // close pattern in `packages/utils/src/fetch-retry.ts`. Numeric in-band
    // statuses are handled separately as `ProviderError::Http`.
    static TRANSIENT: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(concat!(
            r"(?i)\b(?:no[_ -]?capacity|(?:high|peak)[ _-]?demand|(?:at|over|insufficient)[ _-]?capacity|capacity[ _-]?(?:exceeded|exhausted)|peak[ _-]?load)\b",
            r"|overloaded|provider.?returned.?error|rate.?limit|too many requests|\b(?:429|500|502|503|504)\b",
            r"|service.?unavailable|server.?error|internal.?error|retry your request|network.?error|connection.?error|connection.?refused",
            r"|unable.?to.?connect\.\s*is the computer able to access the url\?|other side closed|fetch failed|upstream.?connect",
            r"|upstream.?request.?failed|reset before headers|socket hang up|timed? out|timeout|terminated|retry delay|stream stall",
            r"|no error details in response|HTTP2(?:StreamReset|RefusedStream|EnhanceYourCalm)|nghttp2_(?:internal_error|refused_stream)",
            r"|stream closed with error code nghttp2_(?:internal_error|refused_stream)|malformed.?function.?call",
            r"|bad record mac|stream error.*received from peer|1302|stream[_ -]?read[_ -]?error",
            r"|\b(?:the\s+)?socket connection (?:was )?closed unexpectedly\b",
            r"|unterminated string|unexpected end of json input|unexpected end of data|unexpected eof|end of file|eof while parsing|truncated",
            r"|stream event order|before message_start"
        ))
        .expect("valid fixed OMP transient stream regex")
    });
    TRANSIENT.is_match(detail)
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
            send_abort(sink, model, Some(terminal.partial().clone()), None).await;
        }
        return;
    }
    if !*sent_start
        && matches!(terminal, AssistantMessageEvent::Done { .. })
        && !send(sink, AssistantMessageEvent::Start { partial: terminal.partial().clone() }, cancel).await
    {
        if cancel.is_cancelled() {
            send_abort(sink, model, Some(terminal.partial().clone()), None).await;
        }
        return;
    }
    if !send(sink, terminal.clone(), cancel).await && cancel.is_cancelled() {
        send_abort(sink, model, Some(terminal.partial().clone()), None).await;
    }
}

fn aborted(model: &Model, partial: Option<AssistantMessage>) -> AssistantMessageEvent {
    let mut message = partial.unwrap_or_else(|| AssistantMessage::empty(&model.api, &model.provider, &model.id));
    message.stop_reason = StopReason::Aborted;
    message.error_message = Some(ProviderError::Aborted.to_string());
    AssistantMessageEvent::Error { reason: StopReason::Aborted, error: message }
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn attempt_record(message: &AssistantMessage, stop_reason: StopReason, started: Instant) -> RetryAttempt {
    RetryAttempt {
        usage: message.usage.clone(),
        duration: message.duration,
        elapsed_ms: elapsed_ms(started),
        stop_reason,
    }
}

fn retry_accounting(attempts: Vec<RetryAttempt>, started: Instant) -> RetryAccounting {
    RetryAccounting { attempts, elapsed_ms: elapsed_ms(started) }
}

fn interrupted_accounting(
    attempts: &[RetryAttempt],
    partial: Option<&AssistantMessage>,
    attempt_started: Option<Instant>,
    started: Instant,
) -> Option<RetryAccounting> {
    if attempts.is_empty() {
        return None;
    }
    let mut records = attempts.to_vec();
    if let (Some(message), Some(attempt_started)) = (partial, attempt_started) {
        records.push(attempt_record(message, StopReason::Aborted, attempt_started));
    }
    Some(retry_accounting(records, started))
}

fn with_accounting(mut event: AssistantMessageEvent, accounting: RetryAccounting) -> AssistantMessageEvent {
    match &mut event {
        AssistantMessageEvent::Done { message, .. } => message.retry_accounting = Some(accounting),
        AssistantMessageEvent::Error { error, .. } => error.retry_accounting = Some(accounting),
        _ => unreachable!("only terminal events carry retry accounting"),
    }
    event
}

async fn send_abort(
    sink: &EventSink,
    model: &Model,
    partial: Option<AssistantMessage>,
    accounting: Option<RetryAccounting>,
) {
    // The active HTTP attempt is cancelled before this call. If the consumer
    // is behind a full channel, retain the terminal until it drains or drops.
    let mut event = aborted(model, partial);
    if let Some(accounting) = accounting {
        event = with_accounting(event, accounting);
    }
    let _ = sink.push(event).await;
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
        let started = Instant::now();
        let mut attempts = Vec::new();
        let mut empty_retries = 0u32;
        let mut error_retries = 0u32;
        loop {
            if cancel.is_cancelled() {
                let accounting = interrupted_accounting(&attempts, None, None, started);
                send_abort(&sink, &model, None, accounting).await;
                return;
            }
            let attempt_started = Instant::now();
            let attempt_cancel = cancel.child_token();
            let _attempt_guard = attempt_cancel.clone().drop_guard();
            let AttemptStream { mut events, error } = attempt(attempt_cancel.clone());
            let mut buffered = Vec::new();
            let mut sent_start = false;
            let mut committed = accept_empty_response;
            let mut last_partial = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let terminal = loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        attempt_cancel.cancel();
                        let accounting = interrupted_accounting(&attempts, Some(&last_partial), Some(attempt_started), started);
                        send_abort(&sink, &model, Some(last_partial), accounting).await;
                        return;
                    }
                    event = events.recv() => event,
                };
                let Some(event) = next else {
                    let mut message = last_partial;
                    message.stop_reason = StopReason::Error;
                    message.error_message = Some("Provider stream ended without a terminal event".into());
                    attempts.push(attempt_record(&message, StopReason::Error, attempt_started));
                    let mut terminal = AssistantMessageEvent::Error { reason: StopReason::Error, error: message };
                    if attempts.len() > 1 {
                        terminal = with_accounting(terminal, retry_accounting(attempts, started));
                    }
                    finish(&sink, &mut buffered, &mut sent_start, terminal, &model, &cancel).await;
                    return;
                };
                last_partial = event.partial().clone();
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
                        let accounting =
                            interrupted_accounting(&attempts, Some(&last_partial), Some(attempt_started), started);
                        send_abort(&sink, &model, Some(last_partial), accounting).await;
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
                            let accounting =
                                interrupted_accounting(&attempts, Some(&last_partial), Some(attempt_started), started);
                            send_abort(&sink, &model, Some(last_partial), accounting).await;
                        }
                        return;
                    }
                }
                if !send(&sink, event, &cancel).await {
                    attempt_cancel.cancel();
                    if cancel.is_cancelled() {
                        let accounting =
                            interrupted_accounting(&attempts, Some(&last_partial), Some(attempt_started), started);
                        send_abort(&sink, &model, Some(last_partial), accounting).await;
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
            // The HTTP layer owns its inner attempts and explicit no-retry
            // decisions. One additional attempt is safe before any output is
            // committed, even if the first request never emitted Start.
            let retry_error = matches!(&terminal, AssistantMessageEvent::Error { reason: StopReason::Error, .. })
                && !committed
                && typed_error.as_ref().is_some_and(retryable_uncommitted_error)
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
            let terminal_reason = match &terminal {
                AssistantMessageEvent::Done { reason, .. } | AssistantMessageEvent::Error { reason, .. } => *reason,
                _ => unreachable!("attempt completed with a terminal event"),
            };
            attempts.push(attempt_record(terminal.partial(), terminal_reason, attempt_started));
            if let Some(delay) = delay {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        attempt_cancel.cancel();
                        send_abort(&sink, &model, None, Some(retry_accounting(attempts, started))).await;
                        return;
                    }
                    _ = tokio::time::sleep(delay) => continue,
                }
            }
            if cancel.is_cancelled() {
                attempt_cancel.cancel();
                let accounting = (attempts.len() > 1).then(|| retry_accounting(attempts, started));
                send_abort(&sink, &model, Some(last_partial), accounting).await;
                return;
            }
            let terminal = if attempts.len() > 1 {
                with_accounting(terminal, retry_accounting(attempts, started))
            } else {
                terminal
            };
            finish(&sink, &mut buffered, &mut sent_start, terminal, &model, &cancel).await;
            return;
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ImageContent;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::oneshot;

    fn model() -> Model {
        Model {
            id: "image-model".into(),
            api: "openai-completions".into(),
            provider: "fake".into(),
            base_url: "http://example.invalid/v1".into(),
            reasoning: false,
            max_tokens: None,
            tokenizer: None,
        }
    }

    #[tokio::test]
    async fn image_end_commits_live_and_prevents_error_replay() {
        let model = model();
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted_attempts = attempts.clone();
        let (release, wait_for_release) = oneshot::channel::<()>();
        let mut wait_for_release = Some(wait_for_release);
        let mut stream = with_replay_safe_stream_retry(model.clone(), CancellationToken::new(), false, move |_| {
            counted_attempts.fetch_add(1, Ordering::SeqCst);
            let (sink, events) = EventSink::channel();
            let error = Arc::new(Mutex::new(Some(AttemptError {
                cause: ProviderError::Http { status: 503, detail: "temporary".into() },
                retry_blocked: false,
            })));
            let wait = wait_for_release.take().expect("image output must prevent a second attempt");
            let model = model.clone();
            tokio::spawn(async move {
                let mut partial = AssistantMessage::empty(&model.api, &model.provider, &model.id);
                assert!(sink.push(AssistantMessageEvent::Start { partial: partial.clone() }).await);
                let image = ImageContent { data: "YQ==".into(), mime_type: "image/png".into() };
                partial.content.push(AssistantBlock::Image(image.clone()));
                assert!(
                    sink.push(AssistantMessageEvent::ImageEnd {
                        content_index: 0,
                        content: image,
                        partial: partial.clone()
                    })
                    .await
                );
                let _ = wait.await;
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some("503 temporary".into());
                partial.error_status = Some(503);
                assert!(sink.push(AssistantMessageEvent::Error { reason: StopReason::Error, error: partial }).await);
            });
            AttemptStream { events, error }
        });

        let start = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap().unwrap();
        assert!(matches!(start, AssistantMessageEvent::Start { .. }));
        let image = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap().unwrap();
        assert!(matches!(image, AssistantMessageEvent::ImageEnd { .. }));
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap().unwrap();
        assert!(matches!(terminal, AssistantMessageEvent::Error { error, .. } if error.error_status == Some(503)));
        assert!(tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap().is_none());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }
}
