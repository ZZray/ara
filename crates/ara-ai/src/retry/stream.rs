//! OMP596f2da stream policy plus b91db66e commitment/empty-compaction delta.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
//! MIT, THIRD_PARTY_NOTICES.md. The existing Host drives the returned future.
use super::{
    RetryBindings, execute,
    result::{
        ActualCompletionProducer, ActualCompletionStream, CompletionErrorClassifier,
        CompletionEvent,
    },
};
use crate::{
    AssistantMessageEvent,
    graph_values::{GraphExecution, GraphValueFactory},
    schema_validator::graph_values_compare_with_execution,
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle,
    ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use futures::future::BoxFuture;
use std::sync::{Arc, Weak};

type Result<T> = std::result::Result<T, ValueError>;

/// Original source factory arguments, including an absent options value. A
/// Rust provider implements only this attempt port, never another retry loop.
pub trait ReplaySafeCompletionAttempt: Send + Sync {
    fn start(
        &self,
        model: ValueHandle,
        context: ValueHandle,
        options: ValueHandle,
    ) -> Result<ActualCompletionStream>;
}
pub struct NativeReplaySafeCompletionAttempt {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    callable: ValueHandle,
}
impl NativeReplaySafeCompletionAttempt {
    pub fn prepare(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        callable: ValueHandle,
    ) -> Result<Self> {
        bindings.check_invocation(&factory)?;
        factory.heap().adopt(&callable)?;
        Ok(Self {
            factory,
            bindings,
            callable,
        })
    }
}
impl ReplaySafeCompletionAttempt for NativeReplaySafeCompletionAttempt {
    fn start(
        &self,
        model: ValueHandle,
        context: ValueHandle,
        options: ValueHandle,
    ) -> Result<ActualCompletionStream> {
        let stream = execute(&self.factory, self.bindings.as_ref(), |runtime| {
            self.factory.heap().call(
                &self.callable,
                &self.factory.heap().undefined()?,
                &[model, context, options],
                runtime,
            )
        })?;
        ActualCompletionStream::from_native(&self.factory, &self.bindings, stream)
    }
}
struct StreamClassifier {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    classify: ValueHandle,
}
impl CompletionErrorClassifier for StreamClassifier {
    fn classify(&self, root: &ValueHandle) -> Result<()> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            self.factory.heap().call(
                &self.classify,
                &self.factory.heap().undefined()?,
                std::slice::from_ref(root),
                runtime,
            )?;
            Ok(())
        })
    }
    fn is_error(&self, root: &ValueHandle) -> Result<bool> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let ex = Ex {
                heap: self.factory.heap(),
                runtime,
            };
            ex.text(&ex.get(root, "stopReason")?, "error")
        })
    }
    fn original_is_complete(&self, event: &ValueHandle) -> Result<bool> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let ex = Ex {
                heap: self.factory.heap(),
                runtime,
            };
            if ex.text(&ex.get(event, "type")?, "error")?
                && ex.text(&ex.get(&ex.get(event, "error")?, "stopReason")?, "error")?
            {
                ex.heap.call(
                    &self.classify,
                    &ex.heap.undefined()?,
                    &[ex.get(event, "error")?],
                    runtime,
                )?;
            }
            Ok(ex.text(&ex.get(event, "type")?, "done")?
                || ex.text(&ex.get(event, "type")?, "error")?)
        })
    }
    fn original_result(&self, event: &ValueHandle) -> Result<ValueHandle> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let ex = Ex {
                heap: self.factory.heap(),
                runtime,
            };
            if ex.text(&ex.get(event, "type")?, "done")? {
                return ex.get(event, "message");
            }
            if ex.text(&ex.get(event, "type")?, "error")? {
                return ex.get(event, "error");
            }
            let options = ex.heap.object()?;
            ex.heap.define_own_data(
                &options,
                "kind".into(),
                ex.heap.string("envelope".into())?,
                runtime,
            )?;
            Err(ValueError::Thrown(
                ex.heap.construct(
                    &self.bindings.provider_response_error_constructor(),
                    &[
                        ex.heap
                            .string("Unexpected event type for final result".into())?,
                        options,
                    ],
                    runtime,
                )?,
            ))
        })
    }
}
fn event_is_terminal(
    factory: &Arc<GraphValueFactory>,
    event: &CompletionEvent,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    match event {
        CompletionEvent::Actual(event) => Ok(event.is_terminal()),
        CompletionEvent::Original(event) => {
            let ex = Ex {
                heap: factory.heap(),
                runtime,
            };
            Ok(ex.text(&ex.get(event, "type")?, "done")?
                || ex.text(&ex.get(event, "type")?, "error")?)
        }
    }
}
fn event_result(
    factory: &Arc<GraphValueFactory>,
    event: Option<&CompletionEvent>,
    expected: &str,
    field: &str,
    runtime: &dyn ValueRuntime,
) -> Result<Option<ValueHandle>> {
    Ok(match event {
        Some(CompletionEvent::Actual(AssistantMessageEvent::Done { message, .. }))
            if expected == "done" =>
        {
            Some(message.clone())
        }
        Some(CompletionEvent::Actual(AssistantMessageEvent::Error { error, .. }))
            if expected == "error" =>
        {
            Some(error.clone())
        }
        Some(CompletionEvent::Original(event)) => {
            let ex = Ex {
                heap: factory.heap(),
                runtime,
            };
            if ex.text(&ex.get(event, "type")?, expected)? {
                let root = ex.get(event, field)?;
                if ex.undefined(&root)? {
                    None
                } else {
                    Some(root)
                }
            } else {
                None
            }
        }
        _ => None,
    })
}
pub struct ReplaySafeStreamModule {
    factory: Weak<GraphValueFactory>,
    pub provider_retryable: ValueHandle,
    pub classify_message: ValueHandle,
    finalized_error: ValueHandle,
}
impl ReplaySafeStreamModule {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        error: ValueHandle,
        provider_retryable: ValueHandle,
        classify_message: ValueHandle,
    ) -> Result<Self> {
        let heap = factory.heap();
        heap.adopt(&provider_retryable)?;
        heap.adopt(&classify_message)?;
        heap.module_regexp(
            "ara-ai.omp596f2da.empty-retry",
            "NON_WHITESPACE_RE",
            r"\S",
            "",
        )?;
        let class = heap.fresh_compiled_function(&FINALIZED_ERROR, std::slice::from_ref(&error))?;
        heap.set_prototype(&class, Some(&error), factory.runtime())?;
        let prototype = heap.get(&class, &"prototype".into(), factory.runtime())?;
        let parent = heap.get(&error, &"prototype".into(), factory.runtime())?;
        heap.set_prototype(&prototype, Some(&parent), factory.runtime())?;
        Ok(Self {
            factory: Arc::downgrade(factory),
            provider_retryable,
            classify_message,
            finalized_error: class,
        })
    }
    fn check(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Replay-safe module expired",
            ))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
}
static FINALIZED_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da empty-completion-retry.ts FinalizedProviderStreamError",
    name: "FinalizedProviderStreamError",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(|heap, captures, _, target, args, runtime| {
        let message = args.first().cloned().unwrap_or(heap.undefined()?);
        let instance = heap.construct_with_new_target(&captures[0], &[message], target, runtime)?;
        heap.assign(
            &instance,
            "name".into(),
            heap.string("FinalizedProviderStreamError".into())?,
            runtime,
        )?;
        heap.assign(
            &instance,
            "status".into(),
            args.get(1).cloned().unwrap_or(heap.undefined()?),
            runtime,
        )?;
        Ok(instance)
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor FinalizedProviderStreamError cannot be invoked without new".into(),
        ))
    },
};
#[derive(Clone, Copy)]
struct Ex<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
}
impl Ex<'_> {
    fn get(self, target: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(target, &key.into(), self.runtime)
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(
            self.heap.kind(value)?,
            ValueKind::Null | ValueKind::Undefined
        ))
    }
    fn optional(self, target: &ValueHandle, key: &str) -> Result<ValueHandle> {
        if self.nullish(target)? {
            self.heap.undefined()
        } else {
            self.get(target, key)
        }
    }
    fn undefined(self, value: &ValueHandle) -> Result<bool> {
        Ok(self.heap.kind(value)? == ValueKind::Undefined)
    }
    fn true_value(self, value: &ValueHandle) -> Result<bool> {
        self.heap.object_is(value, &self.heap.boolean(true)?)
    }
    fn text(self, value: &ValueHandle, text: &str) -> Result<bool> {
        self.heap.object_is(value, &self.heap.string(text.into())?)
    }
    fn compare(self, left: &ValueHandle, right: &ValueHandle, op: &str) -> Result<bool> {
        graph_values_compare_with_execution(
            GraphExecution::new(self.heap, self.runtime),
            left,
            right,
            op,
        )
    }
    fn coalesce(self, value: ValueHandle, fallback: f64) -> Result<ValueHandle> {
        if self.nullish(&value)? {
            self.heap.number(fallback)
        } else {
            Ok(value)
        }
    }
}

/// Source for-of uses the actual content iterator, including IteratorClose on
/// early success and a native thrown completion. It does not snapshot members.
pub fn has_visible_assistant_content(
    factory: &Arc<GraphValueFactory>,
    root: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    let ex = Ex {
        heap: factory.heap(),
        runtime,
    };
    let mut iterator = ex.heap.get_iterator(&ex.get(root, "content")?, runtime)?;
    while let Some(block) = ex.heap.iterator_next(&mut iterator, runtime)? {
        let step = (|| {
            if ex.text(&ex.get(&block, "type")?, "image")? {
                return Ok(true);
            }
            if ex.text(&ex.get(&block, "type")?, "toolCall")? {
                return Ok(true);
            }
            if ex.text(&ex.get(&block, "type")?, "text")? {
                let regex = ex.heap.module_regexp(
                    "ara-ai.omp596f2da.empty-retry",
                    "NON_WHITESPACE_RE",
                    r"\S",
                    "",
                )?;
                let method = ex.get(&regex, "test")?;
                return ex.heap.truthy(&ex.heap.call(
                    &method,
                    &regex,
                    &[ex.get(&block, "text")?],
                    runtime,
                )?);
            }
            Ok(false)
        })();
        match step {
            Ok(true) => {
                ex.heap.iterator_close(&mut iterator, runtime)?;
                return Ok(true);
            }
            Ok(false) => {}
            Err(error) => {
                let close = ex.heap.iterator_close(&mut iterator, runtime);
                return Err(match close {
                    Ok(()) => error,
                    Err(close) if error.is_native_exception() && close.is_native_exception() => {
                        error
                    }
                    Err(close) => ValueError::IteratorClose {
                        original: Box::new(error),
                        close: Box::new(close),
                    },
                });
            }
        }
    }
    Ok(false)
}
fn meaningful(
    factory: &Arc<GraphValueFactory>,
    event: &CompletionEvent,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    let ex = Ex {
        heap: factory.heap(),
        runtime,
    };
    if let CompletionEvent::Original(event) = event {
        let kind = ex.get(event, "type")?;
        for name in ["text_delta", "thinking_delta", "toolcall_delta"] {
            if ex.text(&kind, name)? {
                return ex.compare(
                    &ex.get(&ex.get(event, "delta")?, "length")?,
                    &ex.heap.number(0.0)?,
                    ">",
                );
            }
        }
        for name in ["text_end", "thinking_end"] {
            if ex.text(&kind, name)? {
                return ex.compare(
                    &ex.get(&ex.get(event, "content")?, "length")?,
                    &ex.heap.number(0.0)?,
                    ">",
                );
            }
        }
        return Ok(ex.text(&kind, "image_end")?);
    }
    let CompletionEvent::Actual(event) = event else {
        unreachable!()
    };
    Ok(match event {
        AssistantMessageEvent::TextDelta { delta, .. }
        | AssistantMessageEvent::ThinkingDelta { delta, .. }
        | AssistantMessageEvent::TextEnd { content: delta, .. }
        | AssistantMessageEvent::ThinkingEnd { content: delta, .. } => !delta.is_empty(),
        AssistantMessageEvent::ToolcallDelta { delta, .. } => {
            ex.compare(&ex.get(delta, "length")?, &ex.heap.number(0.0)?, ">")?
        }
        AssistantMessageEvent::ImageEnd { .. } => true,
        _ => false,
    })
}
async fn flush(
    outer: &mut ActualCompletionProducer,
    buffered: &mut Vec<CompletionEvent>,
) -> Result<()> {
    for event in buffered.drain(..) {
        outer.push_event(event).await?;
    }
    Ok(())
}

/// The source utility observes cancellation only at retry waits. Unlike a
/// published provider adapter, it never rewrites an in-flight terminal to an
/// aborted message or races cancellation against every event receive.
pub fn with_replay_safe_stream_retry(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    module: Arc<ReplaySafeStreamModule>,
    model: ValueHandle,
    context: ValueHandle,
    options: Option<ValueHandle>,
    attempt: Arc<dyn ReplaySafeCompletionAttempt>,
    policy: ValueHandle,
) -> Result<(ActualCompletionStream, BoxFuture<'static, Result<()>>)> {
    module.check(&factory)?;
    bindings.check_invocation(&factory)?;
    let options = options.unwrap_or(factory.heap().undefined()?);
    for value in [&model, &context, &options, &policy] {
        factory.heap().adopt(value)?;
    }
    let signal = execute(&factory, bindings.as_ref(), |runtime| {
        Ex {
            heap: factory.heap(),
            runtime,
        }
        .optional(&options, "signal")
    })?;
    let (mut outer, stream, receipt) = ActualCompletionStream::channel(&factory);
    outer.set_error_classifier(Arc::new(StreamClassifier {
        factory: factory.clone(),
        bindings: bindings.clone(),
        classify: module.classify_message.clone(),
    }));
    let work = Box::pin(async move {
        let mut empty_retries = 0_f64;
        let mut provider_retries = 0_f64;
        loop {
            bindings.check_invocation(&factory)?;
            let mut buffered = Vec::new();
            let mut committed = execute(&factory, bindings.as_ref(), |runtime| {
                let ex = Ex {
                    heap: factory.heap(),
                    runtime,
                };
                ex.true_value(&ex.optional(&options, "acceptEmptyResponse")?)
            })?;
            let mut terminal = None;
            let mut inner = match attempt.start(model.clone(), context.clone(), options.clone()) {
                Ok(inner) => inner,
                Err(error) => {
                    outer.fail_original(error)?;
                    return Ok(());
                }
            };
            receipt.inherit(inner.events.failure_receipt());
            let iteration = async {
                while let Some(event) = inner.recv_event().await? {
                    if execute(&factory, bindings.as_ref(), |runtime| {
                        event_is_terminal(&factory, &event, runtime)
                    })? {
                        terminal = Some(event);
                        inner.close_iteration().await?;
                        break;
                    }
                    if !committed
                        && !execute(&factory, bindings.as_ref(), |runtime| {
                            meaningful(&factory, &event, runtime)
                        })?
                    {
                        buffered
                            .try_reserve(1)
                            .map_err(|_| ValueError::Allocation)?;
                        buffered.push(event);
                        continue;
                    }
                    committed = true;
                    flush(&mut outer, &mut buffered).await?;
                    outer.push_event(event).await?;
                    if outer.is_done() {
                        inner.close_iteration().await?;
                        return Ok(true);
                    }
                }
                Ok::<_, ValueError>(false)
            }
            .await;
            match iteration {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(error) => {
                    // The source body throw wins over a native return() throw;
                    // integration errors are retained alongside the primary.
                    if let Err(close) = inner.close_iteration().await {
                        if !error.is_native_exception() || !close.is_native_exception() {
                            receipt.record(close);
                        }
                    }
                    flush(&mut outer, &mut buffered).await?;
                    outer.fail_original(error)?;
                    return Ok(());
                }
            }
            let delay = execute(&factory, bindings.as_ref(), |runtime| {
                let ex = Ex {
                    heap: factory.heap(),
                    runtime,
                };
                let completed =
                    event_result(&factory, terminal.as_ref(), "done", "message", runtime)?;
                let retry_empty = ex.true_value(&ex.get(&policy, "retryEmptyCompletion")?)?
                    && !ex.true_value(&ex.optional(&options, "acceptEmptyResponse")?)?
                    && !committed
                    && completed.is_some()
                    && ex.text(&ex.get(completed.as_ref().unwrap(), "stopReason")?, "stop")?
                    && !ex.text(
                        &ex.optional(&ex.get(completed.as_ref().unwrap(), "stopDetails")?, "type")?,
                        "pause_turn",
                    )?
                    && !ex.text(
                        &ex.optional(&ex.get(completed.as_ref().unwrap(), "stopDetails")?, "type")?,
                        "compaction",
                    )?
                    && !ex
                        .heap
                        .truthy(&ex.get(completed.as_ref().unwrap(), "errorMessage")?)?
                    && ex.compare(
                        &ex.coalesce(
                            ex.optional(&ex.get(completed.as_ref().unwrap(), "usage")?, "output")?,
                            0.0,
                        )?,
                        &ex.heap.number(1.0)?,
                        "<=",
                    )?
                    && !has_visible_assistant_content(
                        &factory,
                        completed.as_ref().unwrap(),
                        runtime,
                    )?
                    && empty_retries < 2.0;
                let failed = event_result(&factory, terminal.as_ref(), "error", "error", runtime)?;
                let retry_provider = ex.true_value(&ex.get(&policy, "retryProviderErrors")?)?
                    && !committed
                    && ex.text(
                        &match failed.as_ref() {
                            Some(root) => ex.optional(root, "stopReason")?,
                            None => ex.heap.undefined()?,
                        },
                        "error",
                    )?
                    && !ex.undefined(&ex.get(failed.as_ref().unwrap(), "errorMessage")?)?
                    && ex.compare(
                        &ex.heap.number(provider_retries)?,
                        &ex.coalesce(ex.get(&policy, "maxProviderErrorRetries")?, 0.0)?,
                        "<",
                    )?
                    && ex.heap.truthy(&{
                        let error = ex.heap.construct(
                            &module.finalized_error,
                            &[
                                ex.get(failed.as_ref().unwrap(), "errorMessage")?,
                                ex.get(failed.as_ref().unwrap(), "errorStatus")?,
                            ],
                            runtime,
                        )?;
                        ex.heap.call(
                            &module.provider_retryable,
                            &ex.heap.undefined()?,
                            &[error],
                            runtime,
                        )?
                    })?;
                Ok(if retry_empty {
                    let delay = 500.0 * 2_f64.powf(empty_retries);
                    empty_retries += 1.0;
                    Some(delay)
                } else if retry_provider {
                    let delay = 500.0 * 2_f64.powf(provider_retries);
                    provider_retries += 1.0;
                    Some(delay)
                } else {
                    None
                })
            })?;
            let aborted = || {
                execute(&factory, bindings.as_ref(), |runtime| {
                    let ex = Ex {
                        heap: factory.heap(),
                        runtime,
                    };
                    ex.heap.truthy(&ex.optional(&signal, "aborted")?)
                })
            };
            if delay.is_some() && !aborted()? {
                let delay = delay.expect("source wait condition established delay");
                // Evaluate options?.providerRetryWait for the test, then read
                // it again for the actual method Call as authored by the source.
                let wait = execute(&factory, bindings.as_ref(), |runtime| {
                    let ex = Ex {
                        heap: factory.heap(),
                        runtime,
                    };
                    if !ex
                        .heap
                        .truthy(&ex.optional(&options, "providerRetryWait")?)?
                    {
                        return Ok(None);
                    }
                    let method = ex.get(&options, "providerRetryWait")?;
                    Ok(Some(ex.heap.call(
                        &method,
                        &options,
                        &[ex.heap.number(delay)?, signal.clone()],
                        runtime,
                    )?))
                });
                let waited = match wait {
                    Ok(Some(value)) => bindings
                        .await_value(factory.clone(), value)
                        .await
                        .map(|_| ()),
                    Ok(None) => scheduler_wait(&factory, bindings.clone(), delay, &signal).await,
                    Err(error) => Err(error),
                };
                if let Err(error) = waited {
                    flush(&mut outer, &mut buffered).await?;
                    if aborted()? {
                        if let Some(terminal) = terminal {
                            outer.push_event(terminal).await?;
                        }
                    } else {
                        outer.fail_original(error)?;
                    }
                    return Ok(());
                }
                continue;
            }
            flush(&mut outer, &mut buffered).await?;
            if let Some(terminal) = terminal {
                outer.push_event(terminal).await?;
            } else if !outer.is_done() {
                match inner.result().await {
                    Ok(root) => outer.end_original(root)?,
                    Err(error) => outer.fail_original(error)?,
                }
            }
            return Ok(());
        }
    });
    Ok((stream, work))
}

/// Fixed Bun node:timers/promises scheduler.wait used by source stream retry
/// and completeSimple. Uses the issuing timer/callback owner, including plain
/// signals with an aborted property; it never requires a native signal slot.
pub async fn scheduler_wait(
    factory: &Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    delay: f64,
    signal: &ValueHandle,
) -> Result<()> {
    use super::TimerLease;
    use std::sync::Mutex;
    use tokio::sync::Notify;
    let present = execute(factory, bindings.as_ref(), |runtime| {
        let heap = factory.heap();
        let kind = heap.kind(signal)?;
        if kind == ValueKind::Undefined {
            return Ok(false);
        }
        if !matches!(
            kind,
            ValueKind::Object | ValueKind::Array | ValueKind::Function
        ) || !heap.has(signal, &"aborted".into(), runtime)?
        {
            let detail: ara_rpc::WireString = match kind {
                ValueKind::Null => "null".into(),
                ValueKind::Object => "an instance of Object".into(),
                ValueKind::Array => "an instance of Array".into(),
                ValueKind::Function => "function".into(),
                _ => {
                    let name = match kind {
                        ValueKind::Boolean => "boolean",
                        ValueKind::Number => "number",
                        ValueKind::String => "string",
                        ValueKind::BigInt => "bigint",
                        ValueKind::Symbol => "symbol",
                        _ => "undefined",
                    };
                    let text = match heap.snapshot(signal)? {
                        ValueSnapshot::Symbol { description, .. } => {
                            let mut units = "Symbol(".encode_utf16().collect::<Vec<_>>();
                            if let Some(description) = description {
                                units.extend_from_slice(description.units());
                            }
                            units.push(u16::from(b')'));
                            ara_rpc::WireString::from_units(units)
                        }
                        ValueSnapshot::String(text) => text,
                        _ => heap.to_string(signal, runtime)?,
                    };
                    let mut units = format!("type {name} (").encode_utf16().collect::<Vec<_>>();
                    if kind == ValueKind::String {
                        units.push(u16::from(b'\''));
                    }
                    units.extend_from_slice(text.units());
                    if kind == ValueKind::String {
                        units.push(u16::from(b'\''));
                    }
                    if kind == ValueKind::BigInt {
                        units.push(u16::from(b'n'));
                    }
                    units.push(u16::from(b')'));
                    ara_rpc::WireString::from_units(units)
                }
            };
            let mut message =
                "The \"options.signal\" property must be of type AbortSignal. Received "
                    .encode_utf16()
                    .collect::<Vec<_>>();
            message.extend_from_slice(detail.units());
            let error = heap.construct(
                &bindings.type_error_constructor()?,
                &[heap.string(ara_rpc::WireString::from_units(message))?],
                runtime,
            )?;
            heap.define_own_data(
                &error,
                "code".into(),
                heap.string("ERR_INVALID_ARG_TYPE".into())?,
                runtime,
            )?;
            return Err(ValueError::Thrown(error));
        }
        if heap.truthy(&heap.get(signal, &"aborted".into(), runtime)?)? {
            return Err(scheduler_abort(
                factory,
                bindings.as_ref(),
                signal,
                runtime,
            )?);
        }
        Ok(true)
    })?;
    let state = Arc::new((Mutex::new(None::<Result<()>>), Notify::new()));
    let fired = state.clone();
    let timer = bindings.timers().arm(
        delay,
        Arc::new(move || {
            let mut outcome = fired.0.lock().map_err(|_| ValueError::PoisonedHeap)?;
            if outcome.is_none() {
                *outcome = Some(Ok(()));
                fired.1.notify_waiters();
            }
            Ok(())
        }),
    )?;
    let timer_slot: Arc<Mutex<Option<TimerLease>>> = Arc::new(Mutex::new(Some(timer.clone())));
    let mut callback = None;
    let mut callback_owner = None;
    let setup = if present {
        let cancelled = state.clone();
        let cancel_timer = timer_slot.clone();
        let weak_factory = Arc::downgrade(factory);
        let weak_bindings = Arc::downgrade(&bindings);
        let abort_signal = signal.clone();
        let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
            cancel_timer
                .lock()
                .map_err(|_| ValueError::PoisonedHeap)?
                .as_ref()
                .ok_or(ValueError::InvalidNode)?
                .clear()?;
            let factory = weak_factory
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation(
                    "Scheduler Session expired",
                ))?;
            let bindings = weak_bindings
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation(
                    "Scheduler imports expired",
                ))?;
            let error = execute(&factory, bindings.as_ref(), |runtime| {
                scheduler_abort(&factory, bindings.as_ref(), &abort_signal, runtime)
            })?;
            let mut outcome = cancelled.0.lock().map_err(|_| ValueError::PoisonedHeap)?;
            if outcome.is_none() {
                *outcome = Some(Err(error));
                cancelled.1.notify_waiters();
            }
            Ok(())
        });
        callback_owner = Some(body.clone());
        execute(factory, bindings.as_ref(), |runtime| {
            let heap = factory.heap();
            let listener = bindings.signals().callback(factory, body, runtime)?;
            callback = Some(listener.clone());
            // Bun's resistStopPropagation adds only an inaccessible native
            // symbol to a null-prototype options record, with no once flag.
            let options = heap.object_with_prototype(None)?;
            let method = heap.get(signal, &"addEventListener".into(), runtime)?;
            heap.call(
                &method,
                signal,
                &[heap.string("abort".into())?, listener, options],
                runtime,
            )?;
            Ok(())
        })
    } else {
        Ok(())
    };
    let outcome = match setup {
        Err(error) => Err(error),
        Ok(()) => loop {
            let changed = state.1.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(outcome) = state.0.lock().map_err(|_| ValueError::PoisonedHeap)?.take() {
                break outcome;
            }
            tokio::select! {
                ended=bindings.invocation_ended()=>{timer.clear()?; return Err(ended)},
                _=&mut changed=>{}
            }
        },
    };
    // Source Promise.finally cleanup runs for fulfillment and rejection and
    // its own original throw can replace either outcome.
    if let Some(callback) = callback {
        execute(factory, bindings.as_ref(), |runtime| {
            let heap = factory.heap();
            let method = heap.get(signal, &"removeEventListener".into(), runtime)?;
            heap.call(
                &method,
                signal,
                &[heap.string("abort".into())?, callback],
                runtime,
            )?;
            Ok(())
        })?;
    }
    let _owner = callback_owner;
    outcome
}
fn scheduler_abort(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    signal: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueError> {
    let heap = factory.heap();
    let reason = heap.get(signal, &"reason".into(), runtime)?;
    // Node's AbortError is an ordinary native Error, not OMP's independently
    // imported AbortError subclass with attached classification flags.
    let error = heap.construct(
        &bindings.error_constructor(),
        &[heap.string("The operation was aborted".into())?],
        runtime,
    )?;
    heap.define_own_data(
        &error,
        "name".into(),
        heap.string("AbortError".into())?,
        runtime,
    )?;
    heap.define_own(
        &error,
        "cause".into(),
        PropertyDescriptor::Data {
            value: reason,
            writable: true,
            enumerable: false,
            configurable: true,
        },
        runtime,
    )?;
    heap.define_own_data(
        &error,
        "code".into(),
        heap.string("ABORT_ERR".into())?,
        runtime,
    )?;
    Ok(ValueError::Thrown(error))
}
