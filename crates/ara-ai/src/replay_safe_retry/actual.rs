//! Graph representation binding for the original replay-safe retry owner.
//! OMP596f2da plus b91db66e commitment/empty-compaction delta (MIT). Budgets, backoff,
//! transient classification and commit markers reuse the parent module policy.
use super::{
    AttemptError, BASE_DELAY, MAX_EMPTY_RETRIES, MAX_ERROR_RETRIES, retryable_uncommitted_error,
};
use crate::actual_provider::{GraphBuilder, new_assistant, trim_empty};
use crate::event::{ActualStreamFailureReceipt, EventSink};
use crate::graph_values::GraphValueFactory;
use crate::{ActualAssistantEvent, ActualAssistantStream, AssistantMessageEvent, Model, StopReason};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueSnapshot};
use std::{
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

pub(crate) struct ActualAttemptStream {
    pub events: ActualAssistantStream,
    pub error: Arc<Mutex<Option<AttemptError>>>,
}
fn meaningful(builder: &GraphBuilder, event: &ActualAssistantEvent) -> Result<bool, ValueError> {
    Ok(match event {
        AssistantMessageEvent::TextDelta { delta, .. } | AssistantMessageEvent::ThinkingDelta { delta, .. } => {
            !delta.is_empty()
        }
        AssistantMessageEvent::ToolcallDelta { delta, .. } => {
            // Fixed source observes delta.length > 0 only while uncommitted.
            // Preserve Native Get and ToNumber, including their exceptions.
            let length = builder.get(delta, "length")?;
            builder.0.heap().to_number(&length, builder.0.runtime())? > 0.0
        }
        AssistantMessageEvent::TextEnd { content, .. } | AssistantMessageEvent::ThinkingEnd { content, .. } => {
            !content.is_empty()
        }
        AssistantMessageEvent::ImageEnd { .. } => true,
        _ => false,
    })
}
pub(crate) fn visible(builder: &GraphBuilder, root: &ValueHandle) -> Result<bool, ValueError> {
    for block in builder.members(&builder.get(root, "content")?)? {
        let kind = builder.string(&builder.get(&block, "type")?)?;
        if kind.equals_ascii("image") || kind.equals_ascii("toolCall") {
            return Ok(true);
        }
        if kind.equals_ascii("text") && !trim_empty(&builder.string(&builder.get(&block, "text")?)?) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn retry_empty(builder: &GraphBuilder, root: &ValueHandle) -> Result<bool, ValueError> {
    let details = builder.get(root, "stopDetails")?;
    if !builder.is_nullish(&details)?
        && builder.optional_string(&details, "type")?.is_some_and(|value| value.equals_ascii("pause_turn"))
    {
        return Ok(false);
    }
    let details = builder.get(root, "stopDetails")?;
    if !builder.is_nullish(&details)?
        && builder.optional_string(&details, "type")?.is_some_and(|value| value.equals_ascii("compaction"))
    {
        return Ok(false);
    }
    if builder.0.heap().truthy(&builder.get(root, "errorMessage")?)? {
        return Ok(false);
    }
    let usage = builder.get(root, "usage")?;
    // Source nullish default is only an internal predicate. The original
    // usage root and accounting remain unknown when absent.
    let output = if builder.is_nullish(&usage)? {
        builder.0.heap().undefined()?
    } else {
        builder.get(&usage, "output")?
    };
    let output = if builder.is_nullish(&output)? {
        builder.0.heap().number(0.0)?
    } else { output };
    let tiny = builder.0.heap().to_number(&output, builder.0.runtime())? <= 1.0;
    Ok(tiny && !visible(builder, root)?)
}
fn attempt_record(
    builder: &GraphBuilder,
    root: &ValueHandle,
    reason: StopReason,
    started: Instant,
) -> Result<ValueHandle, ValueError> {
    let record = builder.object([
        ("usage", builder.get(root, "usage")?),
        ("elapsedMs", builder.0.heap().number(started.elapsed().as_millis() as f64)?),
        ("stopReason", builder.text(reason.as_str())?),
    ])?;
    let duration = builder.get(root, "duration")?;
    if !builder.is_nullish(&duration)? {
        builder.define(&record, "duration", duration)?;
    }
    Ok(record)
}
fn with_accounting(
    builder: &GraphBuilder,
    root: &ValueHandle,
    attempts: &[ValueHandle],
    started: Instant,
) -> Result<(), ValueError> {
    let accounting = builder.object([
        ("attempts", builder.0.heap().array(attempts.to_vec())?),
        ("elapsedMs", builder.0.heap().number(started.elapsed().as_millis() as f64)?),
    ])?;
    builder.put(root, "retryAccounting", accounting)
}
fn aborted(builder: &GraphBuilder, root: ValueHandle) -> Result<ActualAssistantEvent, ValueError> {
    builder.put(&root, "stopReason", builder.text("aborted")?)?;
    builder.put(&root, "errorMessage", builder.text(crate::ProviderError::Aborted.to_string())?)?;
    Ok(AssistantMessageEvent::Error { reason: StopReason::Aborted, error: root })
}
async fn send(sink: &EventSink<ActualAssistantEvent>, event: ActualAssistantEvent, cancel: &CancellationToken) -> bool {
    tokio::select! { biased; result = sink.push(event) => result, _ = cancel.cancelled() => false }
}
async fn flush(
    sink: &EventSink<ActualAssistantEvent>,
    buffered: &mut Vec<ActualAssistantEvent>,
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
fn forward_failures(stream: &ActualAssistantStream, receipt: &ActualStreamFailureReceipt) -> bool {
    let _ = receipt;
    !stream.integration_failures().is_empty()
}
async fn send_abort(
    sink: &EventSink<ActualAssistantEvent>,
    builder: &GraphBuilder,
    root: ValueHandle,
    attempts: &[ValueHandle],
    started: Instant,
    receipt: &ActualStreamFailureReceipt,
) {
    let event = (|| {
        if !attempts.is_empty() {
            with_accounting(builder, &root, attempts, started)?;
        }
        aborted(builder, root)
    })();
    match event {
        Ok(event) => {
            let _ = sink.push(event).await;
        }
        Err(error) => receipt.record(error),
    }
}
async fn finish(
    sink: &EventSink<ActualAssistantEvent>,
    builder: &GraphBuilder,
    buffered: &mut Vec<ActualAssistantEvent>,
    sent_start: &mut bool,
    terminal: ActualAssistantEvent,
    cancel: &CancellationToken,
    receipt: &ActualStreamFailureReceipt,
    started: Instant,
) {
    if !flush(sink, buffered, sent_start, cancel).await {
        if cancel.is_cancelled() {
            send_abort(sink, builder, terminal.partial().clone(), &[], started, receipt).await;
        }
        return;
    }
    if !*sent_start && matches!(terminal, AssistantMessageEvent::Done { .. }) {
        if !send(sink, AssistantMessageEvent::Start { partial: terminal.partial().clone() }, cancel).await {
            if cancel.is_cancelled() {
                send_abort(sink, builder, terminal.partial().clone(), &[], started, receipt).await;
            }
            return;
        }
    }
    let root = terminal.partial().clone();
    if !send(sink, terminal, cancel).await && cancel.is_cancelled() {
        send_abort(sink, builder, root, &[], started, receipt).await;
    }
}

pub(crate) fn with_replay_safe_actual_stream_retry<F>(
    model: Model,
    factory: Arc<GraphValueFactory>,
    cancel: CancellationToken,
    accept_empty_response: bool,
    mut attempt: F,
) -> Result<ActualAssistantStream, ValueError>
where
    F: FnMut(CancellationToken) -> ActualAttemptStream + Send + 'static,
{
    let builder = GraphBuilder(factory);
    let (initial, _) = new_assistant(&builder, &model)?;
    let (sink, stream, receipt) = ActualAssistantStream::channel();
    tokio::spawn(async move {
        let _settlement = receipt.settlement_guard();
        let started = Instant::now();
        let mut attempts = Vec::new();
        let mut empty_retries = 0u32;
        let mut error_retries = 0u32;
        loop {
            if cancel.is_cancelled() {
                send_abort(&sink, &builder, initial.clone(), &attempts, started, &receipt).await;
                return;
            }
            let attempt_started = Instant::now();
            let attempt_cancel = cancel.child_token();
            let _guard = attempt_cancel.clone().drop_guard();
            let ActualAttemptStream { mut events, error } = attempt(attempt_cancel.clone());
            receipt.inherit(events.failure_receipt());
            let mut buffered = Vec::new();
            let mut sent_start = false;
            let mut committed = accept_empty_response;
            let mut last_partial = initial.clone();
            let terminal = loop {
                let next = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        attempt_cancel.cancel();
                        // Linked receipt ownership retains late failures even
                        // when this cancelled inner receiver is dropped.

                        forward_failures(&events, &receipt);
                        if !attempts.is_empty() { match attempt_record(&builder, &last_partial, StopReason::Aborted, attempt_started) { Ok(record) => attempts.push(record), Err(error) => receipt.record(error) } }
                        send_abort(&sink, &builder, last_partial, &attempts, started, &receipt).await; return;
                    },
                    event = events.recv() => event,
                };
                let Some(event) = next else {
                    let integration_failed = forward_failures(&events, &receipt);
                    // A producer integration failure may prevent an error root
                    // from being allocated. The original receipt settles this
                    // call; constructing another fallback root is not success.
                    if integration_failed {
                        let _ = flush(&sink, &mut buffered, &mut sent_start, &cancel).await;
                        return;
                    }
                    let terminal = (|| {
                        builder.put(&last_partial, "stopReason", builder.text("error")?)?;
                        builder.put(
                            &last_partial,
                            "errorMessage",
                            builder.text("Provider stream ended without a terminal event")?,
                        )?;
                        attempts.push(attempt_record(&builder, &last_partial, StopReason::Error, attempt_started)?);
                        if attempts.len() > 1 {
                            with_accounting(&builder, &last_partial, &attempts, started)?;
                        }
                        Ok::<_, ValueError>(AssistantMessageEvent::Error {
                            reason: StopReason::Error,
                            error: last_partial,
                        })
                    })();
                    match terminal {
                        Ok(terminal) => {
                            finish(
                                &sink,
                                &builder,
                                &mut buffered,
                                &mut sent_start,
                                terminal,
                                &cancel,
                                &receipt,
                                started,
                            )
                            .await
                        }
                        Err(error) => receipt.record(error),
                    }
                    return;
                };
                last_partial = event.partial().clone();
                if event.is_terminal() {
                    break event;
                }
                if !committed {
                    // Keep the source short circuit: after a nonempty argument
                    // delta commits, a healed undefined delta forwards untouched.
                    let meaningful = match meaningful(&builder, &event) {
                        Ok(meaningful) => meaningful,
                        Err(error) => {
                            receipt.record(error);
                            attempt_cancel.cancel();
                            forward_failures(&events, &receipt);
                            // Fixed source flushes pre-output markers before
                            // surfacing the original meaningful-event exception.
                            let _ = flush(&sink, &mut buffered, &mut sent_start, &cancel).await;
                            return;
                        }
                    };
                    if !meaningful {
                        buffered.push(event);
                        continue;
                    }
                }
                committed = true;
                if !flush(&sink, &mut buffered, &mut sent_start, &cancel).await {
                    attempt_cancel.cancel();
                    forward_failures(&events, &receipt);
                    if cancel.is_cancelled() {
                        send_abort(&sink, &builder, last_partial, &attempts, started, &receipt).await;
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
                        forward_failures(&events, &receipt);
                        if cancel.is_cancelled() {
                            send_abort(&sink, &builder, last_partial, &attempts, started, &receipt).await;
                        }
                        return;
                    }
                }
                if !send(&sink, event, &cancel).await {
                    attempt_cancel.cancel();
                    forward_failures(&events, &receipt);
                    if cancel.is_cancelled() {
                        send_abort(&sink, &builder, last_partial, &attempts, started, &receipt).await;
                    }
                    return;
                }
            };
            // The original inner receipt remains linked after this receiver
            // is dropped; no failure snapshot becomes an ownership boundary.

            let integration_failed = forward_failures(&events, &receipt);
            let tiny = if !committed
                && !accept_empty_response
                && matches!(terminal, AssistantMessageEvent::Done { reason: StopReason::Stop, .. })
                && empty_retries < MAX_EMPTY_RETRIES
            {
                match retry_empty(&builder, terminal.partial()) {
                    Ok(tiny) => tiny,
                    Err(error) => {
                        receipt.record(error);
                        false
                    }
                }
            } else {
                false
            };
            // This wrapper has already barred transparent provider replay.
            // Publish that receipt on existing failure evidence without changing
            // Session recovery policy or constructing a replacement terminal.
            if committed && matches!(terminal, AssistantMessageEvent::Error { .. }) {
                let observed = (|| {
                    let evidence = builder.get(terminal.partial(), "failureEvidence")?;
                    if !matches!(builder.0.heap().snapshot(&evidence)?, ValueSnapshot::Null | ValueSnapshot::Undefined) {
                        builder.put(&evidence, "replayBlocked", builder.0.heap().boolean(true)?)?;
                    }
                    Ok::<(), ValueError>(())
                })();
                if let Err(error) = observed {
                    receipt.record(error);
                }
            }
            let typed_error = error.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone();
            let retry_error = matches!(terminal, AssistantMessageEvent::Error { reason: StopReason::Error, .. })
                && !committed
                && !integration_failed
                && receipt.failures().is_empty()
                && typed_error.as_ref().is_some_and(retryable_uncommitted_error)
                && error_retries < MAX_ERROR_RETRIES;
            let delay = if tiny && !integration_failed && receipt.failures().is_empty() {
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
            let reason = match &terminal {
                AssistantMessageEvent::Done { reason, .. } | AssistantMessageEvent::Error { reason, .. } => *reason,
                _ => unreachable!(),
            };
            match attempt_record(&builder, terminal.partial(), reason, attempt_started) {
                Ok(record) => attempts.push(record),
                Err(error) => {
                    receipt.record(error);
                    finish(&sink, &builder, &mut buffered, &mut sent_start, terminal, &cancel, &receipt, started).await;
                    return;
                }
            }
            if let Some(delay) = delay {
                tokio::select! { _ = cancel.cancelled() => { send_abort(&sink, &builder, initial.clone(), &attempts, started, &receipt).await; return; }, _ = tokio::time::sleep(delay) => continue }
            }
            if cancel.is_cancelled() {
                send_abort(
                    &sink,
                    &builder,
                    last_partial,
                    if attempts.len() > 1 { &attempts } else { &[] },
                    started,
                    &receipt,
                )
                .await;
                return;
            }
            if attempts.len() > 1 {
                if let Err(error) = with_accounting(&builder, terminal.partial(), &attempts, started) {
                    receipt.record(error);
                }
            }
            finish(&sink, &builder, &mut buffered, &mut sent_start, terminal, &cancel, &receipt, started).await;
            return;
        }
    });
    Ok(stream)
}

/// Native carrier observations run in the existing replay-safe retry entrance.
#[cfg(test)]
pub(super) async fn observe_carrier_retry_failures(model: Model) {
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap, ValueRuntime,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Runtime {
        thrown: ValueHandle,
        calls: Arc<AtomicUsize>,
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(703), .. } => {
                    self.calls.fetch_add(1, Ordering::SeqCst);
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    for mode in ["undefined", "length-getter", "committed-undefined"] {
        let heap = ValueHeap::new().unwrap();
        let thrown = heap.object().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let factory = Arc::new(
            GraphValueFactory::new(heap.clone(), Arc::new(Runtime { thrown: thrown.clone(), calls: calls.clone() }))
                .unwrap(),
        );
        let builder = GraphBuilder(factory.clone());
        let (root, _) = new_assistant(&builder, &model).unwrap();
        let delta = if mode == "length-getter" {
            let delta = heap.object().unwrap();
            heap.define_own(
                &delta,
                "length".into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(703))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                factory.runtime(),
            )
            .unwrap();
            delta
        } else {
            heap.undefined().unwrap()
        };
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let sent_root = root.clone();
        let sent_delta = delta.clone();
        let mut stream =
            with_replay_safe_actual_stream_retry(model.clone(), factory, CancellationToken::new(), false, move |_| {
                assert_eq!(counted.fetch_add(1, Ordering::SeqCst), 0, "native exception must not replay: {mode}");
                let (sink, events, receipt) = ActualAssistantStream::channel();
                let root = sent_root.clone();
                let delta = sent_delta.clone();
                tokio::spawn(async move {
                    let _settlement = receipt.settlement_guard();
                    assert!(sink.push(AssistantMessageEvent::Start { partial: root.clone() }).await);
                    assert!(
                        sink.push(AssistantMessageEvent::TextStart {
                            content_index: root.heap().number(0.0).unwrap(),
                            partial: root.clone()
                        })
                        .await
                    );
                    if mode == "committed-undefined" {
                        assert!(
                            sink.push(AssistantMessageEvent::ToolcallDelta {
                                content_index: root.heap().number(1.0).unwrap(),
                                delta: root.heap().string("{}".into()).unwrap(),
                                partial: root.clone()
                            })
                            .await
                        );
                    }
                    assert!(
                        sink.push(AssistantMessageEvent::ToolcallDelta {
                            content_index: root.heap().number(1.0).unwrap(),
                            delta,
                            partial: root.clone()
                        })
                        .await
                    );
                    if mode == "committed-undefined" {
                        assert!(
                            sink.push(AssistantMessageEvent::Done { reason: StopReason::Stop, message: root }).await
                        );
                    }
                });
                ActualAttemptStream { events, error: Arc::new(Mutex::new(None)) }
            })
            .unwrap();
        let mut observed = Vec::new();
        while let Some(event) = tokio::time::timeout(std::time::Duration::from_secs(2), stream.recv()).await.unwrap() {
            assert!(event.partial().same_identity(&root), "{mode}");
            observed.push(event);
        }
        let failures = tokio::time::timeout(std::time::Duration::from_secs(2), stream.failure_receipt().wait_settled())
            .await
            .unwrap();
        assert!(matches!(observed[0], AssistantMessageEvent::Start { .. }), "{mode}");
        assert!(matches!(observed[1], AssistantMessageEvent::TextStart { .. }), "{mode}");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "{mode}");
        if mode == "committed-undefined" {
            assert_eq!(observed.len(), 5);
            assert!(
                matches!(&observed[2], AssistantMessageEvent::ToolcallDelta { delta: committed, .. }
                    if matches!(root.heap().snapshot(committed).unwrap(), ValueSnapshot::String(text) if text.equals_ascii("{}")))
            );
            assert!(
                matches!(&observed[3], AssistantMessageEvent::ToolcallDelta { delta: received, .. } if received.same_identity(&delta))
            );
            assert!(matches!(observed[4], AssistantMessageEvent::Done { .. }));
            assert!(failures.is_empty());
        } else {
            assert_eq!(observed.len(), 2, "flush only previously buffered markers: {mode}");
            assert_eq!(failures.len(), 1, "{mode}");
            if mode == "length-getter" {
                assert!(matches!(failures[0].as_ref(), ValueError::Thrown(value) if value.same_identity(&thrown)));
            } else {
                assert!(matches!(failures[0].as_ref(), ValueError::TypeError(_)));
            }
        }
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(mode == "length-getter"), "{mode}");
    }
    // Exercise this published actual wrapper's loop, independently from the
    // native utility's carrier tests. Both stops have known empty/tiny usage.
    for kind in ["pause_turn", "compaction"] {
        let heap = ValueHeap::new().unwrap();
        let factory = Arc::new(GraphValueFactory::new(heap.clone(), Arc::new(ara_rpc::value_heap::NoRuntime)).unwrap());
        let builder = GraphBuilder(factory.clone());
        let (root, _) = new_assistant(&builder, &model).unwrap();
        builder.put(&builder.get(&root, "usage").unwrap(), "output", heap.number(0.0).unwrap()).unwrap();
        builder.put(&root, "stopDetails", builder.derived(&serde_json::json!({"type":kind})).unwrap()).unwrap();
        let attempts = Arc::new(AtomicUsize::new(0));
        let counted = attempts.clone();
        let sent_root = root.clone();
        let mut stream = with_replay_safe_actual_stream_retry(
            model.clone(), factory, CancellationToken::new(), false, move |_| {
                assert_eq!(counted.fetch_add(1, Ordering::SeqCst), 0, "protected empty stop replayed: {kind}");
                let (sink, events, receipt) = ActualAssistantStream::channel();
                let root = sent_root.clone();
                tokio::spawn(async move {
                    let _settlement = receipt.settlement_guard();
                    assert!(sink.push(AssistantMessageEvent::Start { partial: root.clone() }).await);
                    assert!(sink.push(AssistantMessageEvent::Done { reason: StopReason::Stop, message: root }).await);
                });
                ActualAttemptStream { events, error: Arc::new(Mutex::new(None)) }
            },
        ).unwrap();
        let receipt = stream.failure_receipt();
        let observed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut events = Vec::new();
            while let Some(event) = stream.recv().await { events.push(event); }
            events
        }).await.expect("bounded actual protected-empty loop");
        assert_eq!(attempts.load(Ordering::SeqCst), 1, "{kind}");
        assert_eq!(observed.len(), 2, "{kind}");
        assert!(matches!(observed[0], AssistantMessageEvent::Start { .. }));
        assert!(matches!(&observed[1], AssistantMessageEvent::Done { reason: StopReason::Stop, message }
            if message.same_identity(&root)));
        assert!(tokio::time::timeout(std::time::Duration::from_secs(2), receipt.wait_settled())
            .await.expect("actual protected-empty receipt").is_empty());
        eprintln!("ARA-RETRY UPGRADE b91db66e ACTUAL PROTECTED EMPTY PASS {kind}");
    }
}
