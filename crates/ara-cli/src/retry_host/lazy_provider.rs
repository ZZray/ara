//! Host builtin modules/custom APIs -> lazy forwarding -> common idle owner.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md (MIT).
//! Fixed OMP596f2da providers/register-builtins.ts:225-384 (MIT).
//! Bedrock override installation/lookup:163-170,461-470,507 in the same source.
use crate::{
    model_registry_extensions::{
        ExtensionRegistry, ExtensionRegistryHost, ExtensionResult, RuntimeApiRegistration,
    },
    native_run::{NativeRun, ProviderDispatchReservation},
};
use ara_agent::ActualProviderError;
use ara_ai::{
    ActualAssistantEvent, ActualStreamFailureReceipt, StopReason,
    event::LocalWorkSource,
    graph_values::GraphValueFactory,
    retry::{
        RetryBindings,
        abort::create_abort_source_tracker,
        idle::{
            IdleIterator, IdlePullSource, get_openai_stream_first_event_timeout_ms,
            get_openai_stream_idle_timeout_ms, get_stream_first_event_timeout_ms,
            get_stream_idle_timeout_ms,
        },
        result::{ActualCompletionStream, OriginalStreamResult},
    },
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NodeId, ValueError, ValueHandle, ValueKind,
        ValueRuntime,
    },
};
use futures::{FutureExt, future::BoxFuture};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ValueError>;
const IDLE_ERROR: &str = "Provider stream stalled while waiting for the next event";
const FIRST_ERROR: &str = "Provider stream timed out while waiting for the first event";

fn execute<T>(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    call: impl FnOnce(&dyn ValueRuntime) -> Result<T>,
) -> Result<T> {
    let mut call = Some(call);
    let mut result = None;
    bindings.with_runtime(factory, &mut |runtime| {
        result = Some(call.take().ok_or(ValueError::UnsupportedOperation(
            "Lazy runtime import invoked twice",
        ))?(runtime)?);
        factory.heap().undefined()
    })?;
    result.ok_or(ValueError::UnsupportedOperation(
        "Lazy runtime import did not invoke",
    ))
}
fn nullish(factory: &Arc<GraphValueFactory>, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(
        factory.heap().kind(value)?,
        ValueKind::Null | ValueKind::Undefined
    ))
}
fn get(
    factory: &Arc<GraphValueFactory>,
    object: &ValueHandle,
    name: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    factory.heap().get(object, &name.into(), runtime)
}
fn option_number(
    factory: &Arc<GraphValueFactory>,
    options: &ValueHandle,
    name: &str,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    let value = get(factory, options, name, runtime)?;
    if nullish(factory, &value)? {
        Ok(None)
    } else {
        Ok(Some(factory.heap().to_number(&value, runtime)?))
    }
}

#[derive(Clone, Default)]
pub struct LazyStreamLimits {
    pub default_first_event_timeout_ms: Option<f64>,
    pub default_idle_timeout_ms: Option<f64>,
    pub provider_handles_stream_timeouts: bool,
    pub provider_handles_first_event_timeouts: bool,
    pub openai_idle_env_floors_first_event: bool,
}
/// The actual provider returns its original stream owner and source identity.
/// An override supplies its original getter through LocalWorkSource; no probe
/// counters or copied busy flags are introduced by the forwarding adapter.
pub struct NativeLazySource {
    pub identity: ValueHandle,
    pub stream: ActualCompletionStream,
    pub local_work: Option<Arc<dyn LocalWorkSource>>,
    /// Authentic optional source.result method; None preserves source end().
    pub original_result: Option<Arc<dyn OriginalStreamResult>>,
}
pub trait NativeLazyProvider: Send + Sync {
    fn stream(
        &self,
        factory: &Arc<GraphValueFactory>,
        model: &ValueHandle,
        context: &ValueHandle,
        options: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<NativeLazySource>;
}
pub struct RegisteredLazyProvider {
    pub provider: Arc<dyn NativeLazyProvider>,
    pub limits: LazyStreamLimits,
    /// Actual AIError.classify import from the same prepared realm.
    pub classify: ValueHandle,
}
/// Install/replace the actual Bedrock module override, matching fixed OMP
/// setBedrockProviderModule. This is not a custom API registration and has no
/// extension source, callbacks, or provider wire implementation of its own.
pub fn set_bedrock_provider_module(
    registry: &ExtensionRegistry,
    provider: Arc<RegisteredLazyProvider>,
) {
    registry.set_bedrock_provider_module(provider);
}
pub fn register_lazy_provider(
    registry: &ExtensionRegistry,
    host: &dyn ExtensionRegistryHost,
    api: WireString,
    source_id: Option<WireString>,
    provider: Arc<RegisteredLazyProvider>,
) -> ExtensionResult<()> {
    registry.register_custom_api(
        RuntimeApiRegistration {
            api,
            handle: provider,
            source_id,
        },
        host,
    )
}

struct NativeStreamPull {
    identity: ValueHandle,
    stream: Arc<tokio::sync::Mutex<ActualCompletionStream>>,
    local_work: Arc<dyn LocalWorkSource>,
    originals: Arc<Mutex<HashMap<NodeId, ActualAssistantEvent>>>,
    close: CancellationToken,
}
impl IdlePullSource for NativeStreamPull {
    fn initialize(
        &self,
        factory: &Arc<GraphValueFactory>,
        _effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        factory.heap().adopt(&self.identity)?;
        Ok(self.identity.clone())
    }
    fn next(
        &self,
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
    ) -> BoxFuture<'static, Result<ValueHandle>> {
        let stream = self.stream.clone();
        let originals = self.originals.clone();
        let close = self.close.clone();
        async move {
            // This future is polled by NativeRun::drive with its forwarding
            // consumer, not spawned. A revoked/closed read does no graph work.
            let mut source = stream.lock().await;
            let item = tokio::select! { biased; _ = close.cancelled() => { source.events.close(); return Err(ValueError::UnsupportedOperation("Lazy source read closed")); }, item = source.recv() => item? };
            drop(source);
            if close.is_cancelled() { return Err(ValueError::UnsupportedOperation("Lazy source read closed")); }
            execute(&factory, bindings.as_ref(), |runtime| {
                let heap = factory.heap();
                let result = heap.object()?;
                heap.define_own_data(&result, "done".into(), heap.boolean(item.is_none())?, runtime)?;
                if let Some(item) = item {
                    let value = item.printable_graph(runtime)?;
                    originals.lock().map_err(|_| ValueError::PoisonedHeap)?.insert(value.node_id(), item);
                    heap.define_own_data(&result, "value".into(), value, runtime)?;
                }
                Ok(result)
            })
        }.boxed()
    }
    fn close(
        &self,
        _factory: &Arc<GraphValueFactory>,
        _bindings: &dyn RetryBindings,
        _effective: &dyn ValueRuntime,
    ) -> Result<()> {
        self.close.cancel();
        if let Ok(mut source) = self.stream.try_lock() {
            source.events.close();
        }
        Ok(())
    }
    fn has_pending_local_work(&self) -> Result<bool> {
        self.local_work.has_pending_local_work()
    }
    fn local_work_settled_at_in(&self, execution: ara_ai::graph_values::GraphExecution<'_>, globals: &ValueHandle) -> Result<ValueHandle> {
        self.local_work.local_work_settled_at_in(execution, globals)
    }
}
impl Drop for NativeStreamPull {
    fn drop(&mut self) {
        self.close.cancel();
        if let Ok(mut source) = self.stream.try_lock() {
            source.events.close();
        }
    }
}
struct RustSourceCloseGuard(Arc<NativeStreamPull>);
impl Drop for RustSourceCloseGuard {
    fn drop(&mut self) {
        self.0.close.cancel();
        if let Ok(mut source) = self.0.stream.try_lock() {
            source.events.close();
        }
    }
}
pub struct LazyForwardRun {
    pub stream: ActualCompletionStream,
    /// Must remain inside the current NativeRun's drive. Dropping it closes
    /// only the original Rust source; inherited receipts remain inspectable.
    pub forward: BoxFuture<'static, Result<()>>,
    pub receipt: ActualStreamFailureReceipt,
    // Original request-local abort only. Called from the SAME Run, never an
    // OS cleanup worker, and never cancels the caller's parent signal.
    source_cancel: Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>,
}

pub fn dispatch_registered_lazy(
    registry: &ExtensionRegistry,
    api: &WireString,
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    model: ValueHandle,
    context: ValueHandle,
    options: ValueHandle,
) -> std::result::Result<LazyForwardRun, ActualProviderError> {
    dispatch_registered_lazy_reserved(
        registry, api, factory, bindings, model, context, options, None, None,
    )
}

/// Production dispatch reserves the original Run's provider owner before the
/// actual source Call, so cancellation/drop cannot lose its late receipt.
pub fn dispatch_registered_lazy_in_run(
    native_run: &NativeRun,
    registry: &ExtensionRegistry,
    api: &WireString,
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    model: ValueHandle,
    context: ValueHandle,
    options: ValueHandle,
) -> std::result::Result<LazyForwardRun, ActualProviderError> {
    let lease = native_run.lease();
    let reservation = lease.reserve_provider_dispatch()?;
    let close = lease.provider_cancel_token();
    dispatch_registered_lazy_reserved(
        registry,
        api,
        factory,
        bindings,
        model,
        context,
        options,
        Some(reservation),
        Some(close),
    )
}

struct SourceDispatchJoin {
    receipt: ActualStreamFailureReceipt,
    cancel: Mutex<Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>>,
}

// The source-equivalent cap surrounds both builtin/custom selection and
// physical Call. Keep the registry alive, not a frozen registration snapshot.
fn dispatch_registered_lazy_reserved(
    registry: &ExtensionRegistry,
    api: &WireString,
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    model: ValueHandle,
    context: ValueHandle,
    options: ValueHandle,
    reservation: Option<ProviderDispatchReservation>,
    source_close: Option<CancellationToken>,
) -> std::result::Result<LazyForwardRun, ActualProviderError> {
    use ara_ai::provider_dispatch::ProviderCoordinationError;
    let policy = crate::provider_inflight::configuration();
    let limit = execute(&factory, bindings.as_ref(), |runtime| {
        let heap = factory.heap();
        let provider = get(&factory, &model, "provider", runtime)?;
        let limits = get(&factory, &options, "maxInFlightRequests", runtime)?;
        let primitive = heap.to_primitive(&provider, true, runtime)?;
        let provider_key = if heap.kind(&primitive)? == ValueKind::Symbol {
            ara_rpc::value_heap::PropertyKey::Symbol(heap.symbol_key(&primitive)?)
        } else { ara_rpc::value_heap::PropertyKey::String(heap.to_string(&primitive, runtime)?) };
        let value = if nullish(&factory, &limits)? {
            match &provider_key {
                ara_rpc::value_heap::PropertyKey::String(provider) => match policy.limits.get(provider) { Some(value) => heap.number(*value)?, None => heap.undefined()? },
                _ => heap.undefined()?,
            }
        } else { heap.get(&limits, &provider_key, runtime)? };
        if heap.kind(&value)? != ValueKind::Number { return Ok(None); }
        let number = get(&factory, &bindings.globals(), "Number", runtime)?;
        let finite = get(&factory, &number, "isFinite", runtime)?;
        if !heap.truthy(&heap.call(&finite, &number, &[value.clone()], runtime)?)? { return Ok(None); }
        let numeric = heap.to_number(&value, runtime)?;
        if numeric <= 0.0 { return Ok(None); }
        let math = get(&factory, &bindings.globals(), "Math", runtime)?;
        let max = get(&factory, &math, "max", runtime)?;
        let floor_math = get(&factory, &bindings.globals(), "Math", runtime)?;
        let floor = get(&factory, &floor_math, "floor", runtime)?;
        let floored = heap.call(&floor, &floor_math, &[value], runtime)?;
        let limited = heap.call(&max, &math, &[heap.number(1.0)?, floored], runtime)?;
        Ok(Some(heap.to_number(&limited, runtime)? as usize))
    })?;
    let Some(limit) = limit else {
        return dispatch_registered_lazy_source(registry, api, factory, bindings, model, context, options, reservation, source_close, None);
    };
    let registry = registry.clone();
    let api = api.clone();
    let (mut sink, stream, receipt) = ActualCompletionStream::channel(&factory);
    let forward_receipt = receipt.clone();
    let cancel = source_close.clone().unwrap_or_default();
    let forward = async move {
        // Acquire arguments are evaluated provider first, then options.signal.
        // The Host port is the declared original Model.provider:string input;
        // Bun.SHA256.hash does not implicitly String-coerce Symbol/object here.
        let provider_value = execute(&factory, bindings.as_ref(), |runtime| get(&factory, &model, "provider", runtime))?;
        let native_signal = execute(&factory, bindings.as_ref(), |runtime| {
            let value = get(&factory, &options, "signal", runtime)?;
            if nullish(&factory, &value)? { Ok(None) } else { Ok(Some(value)) }
        })?;
        let acquisition_cancel = match &native_signal {
            Some(signal) => execute(&factory, bindings.as_ref(), |runtime| bindings.signals().token(&factory, signal, runtime))?,
            None => cancel.clone(),
        };
        let acquisition_cancel = acquisition_cancel.child_token();
        let provider_key = execute(&factory, bindings.as_ref(), |runtime| {
            // Slot acquisition checks its original signal before reaching the
            // hash boundary; argument evaluation itself did not String-coerce.
            if let Some(signal) = &native_signal {
                if factory.heap().truthy(&get(&factory, signal, "aborted", runtime)?)? {
                    let reason = get(&factory, signal, "reason", runtime)?;
                    let reason = if nullish(&factory, &reason)? { bindings.signals().error(&factory, "AbortError", "Provider request aborted before dispatch", runtime)? } else { reason };
                    return Err(ValueError::Thrown(reason));
                }
            }
            match factory.heap().snapshot(&provider_value)? {
                ara_rpc::value_heap::ValueSnapshot::String(value) => Ok(value),
                _ => Err(ValueError::TypeError("Provider in-flight SHA256 input must be a string".into())),
            }
        })?;
        let acquiring = policy.owner.acquire(provider_key, limit, acquisition_cancel.clone());
        tokio::pin!(acquiring);
        let acquired = tokio::select! { biased;
            _ = sink.closed() => { acquisition_cancel.cancel(); acquiring.await },
            _ = cancel.cancelled() => { acquisition_cancel.cancel(); acquiring.await },
            result = &mut acquiring => result,
        };
        let permit = match acquired {
            Ok(permit) => permit,
            Err(error) => {
                let original = native_admission_error(&factory, bindings.as_ref(), native_signal.as_ref(), error)?;
                sink.fail_original(original.clone())?;
                return Err(original);
            }
        };
        let dispatch_join = Arc::new(SourceDispatchJoin { receipt: ActualStreamFailureReceipt::default(), cancel: Mutex::new(None) });
        let dispatch_seal = dispatch_join.receipt.owner_settlement_guard();
        let outcome = async {
            if cancel.is_cancelled() || acquisition_cancel.is_cancelled() {
                return Err(native_admission_error(&factory, bindings.as_ref(), native_signal.as_ref(), ProviderCoordinationError::Cancelled)?);
            }
            // Re-read the source signal after acquire; a late abort does not
            // dispatch even if its OS cancellation notification was delayed.
            execute(&factory, bindings.as_ref(), |runtime| {
                let signal = get(&factory, &options, "signal", runtime)?;
                if !nullish(&factory, &signal)? {
                    let aborted = get(&factory, &signal, "aborted", runtime)?;
                    if factory.heap().truthy(&aborted)? {
                        // Source options.signal.reason repeats the receiver
                        // Get; it need not return the earlier signal object.
                        let reason_signal = get(&factory, &options, "signal", runtime)?;
                        let reason = get(&factory, &reason_signal, "reason", runtime)?;
                        let reason = if nullish(&factory, &reason)? {
                            bindings.signals().error(&factory, "AbortError", "Provider request aborted before dispatch", runtime)?
                        } else { reason };
                        return Err(ValueError::Thrown(reason));
                    }
                }
                Ok(())
            })?;
            let limited_options = execute(&factory, bindings.as_ref(), |runtime| {
                let heap = factory.heap(); let target = heap.object()?;
                heap.spread_into(&target, &options, runtime)?;
                heap.define_own_data(&target, "waitForTerminalDrain".into(), heap.boolean(true)?, runtime)?;
                Ok(target)
            })?;
            let invocation = dispatch_registered_lazy_source(&registry, &api, factory.clone(), bindings.clone(), model, context, limited_options, reservation, source_close, Some(dispatch_join.clone()))
                .map_err(|error| match error { ActualProviderError::Value(error) => error, _ => ValueError::UnsupportedOperation("Native provider dispatch failed outside original value channel") })?;
            forward_receipt.inherit_stream(&invocation.stream.events);
            let source_receipt = invocation.receipt;
            let cancel_source = invocation.source_cancel;
            let mut inner = invocation.stream;
            let source_forward = invocation.forward;
            let collector = async {
                let mut terminal = None;
                let mut delivery_closed = false;
                loop {
                    let next = tokio::select! { biased;
                        _ = sink.closed(), if !delivery_closed => {
                            delivery_closed = true;
                            acquisition_cancel.cancel();
                            if let Some(abort) = &cancel_source { if let Err(error) = abort() { forward_receipt.record(error); } }
                            // Drain the queued events after closing delivery;
                            // do not leave the joined forwarder backpressured.
                            inner.events.close();
                            continue;
                        },
                        next = inner.recv() => next,
                    };
                    let event = match next {
                        Ok(Some(event)) => event,
                        Ok(None) => break,
                        Err(error) => {
                            acquisition_cancel.cancel();
                            if let Some(abort) = &cancel_source { if let Err(secondary) = abort() { forward_receipt.record(secondary); } }
                            inner.events.close();
                            return Err(error);
                        }
                    };
                    if event.is_terminal() { terminal = Some(event); }
                    else if terminal.is_none() && !delivery_closed {
                        match sink.push(event).await {
                            Ok(true) => {},
                            Ok(false) => {
                                delivery_closed = true;
                                acquisition_cancel.cancel();
                                if let Some(abort) = &cancel_source { if let Err(error) = abort() { forward_receipt.record(error); } }
                                inner.events.close();
                            },
                            Err(error) => {
                                acquisition_cancel.cancel();
                                if let Some(abort) = &cancel_source { if let Err(secondary) = abort() { forward_receipt.record(secondary); } }
                                inner.events.close();
                                return Err(error);
                            }
                        }
                    }
                }
                Ok::<_, ValueError>((terminal, delivery_closed))
            };
            // join! never drops the other active owner on an error. A failed
            // forwarder must finish its original close guard while the event
            // consumer keeps draining; result() is not queried after failure.
            let (forwarded, collected) = futures::join!(source_forward, collector);
            let result = match (forwarded, collected) {
                (Err(error), _) | (_, Err(error)) => Err(error),
                (Ok(()), Ok((_, true))) => Ok((None, None)),
                (Ok(()), Ok((terminal, false))) => inner.result().await.map(|result| (terminal, Some(result))),
            };
            // Error is not EOF: abort the original request-local source before
            // joining a producer whose held transport may need that abort.
            if result.is_err() {
                if let Some(abort) = &cancel_source { if let Err(error) = abort() { forward_receipt.record(error); } }
            }
            // Every dispatched outcome joins the authentic producer/resource
            // receipt before release. Error is not evidence of transport EOF.
            let _ = source_receipt.wait_settled().await;
            result
        }.await;
        // source.stream may have returned a real producer before a later
        // native options/iterator preparation throws. Its receipt and local
        // abort are captured at that first boundary as well.
        if outcome.is_err() {
            let abort = dispatch_join.cancel.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if let Some(abort) = abort { if let Err(error) = abort() { forward_receipt.record(error); } }
        }
        drop(dispatch_seal);
        let _ = dispatch_join.receipt.wait_settled().await;
        if let Err(error) = permit.release_once().await { eprintln!("Provider in-flight permit release failed: {error}"); }
        match outcome {
            Ok((Some(terminal), _)) => { forward_receipt.retain_terminal(&terminal); sink.push(terminal).await?; Ok(()) },
            Ok((None, Some(result))) => sink.end_original(result),
            Ok((None, None)) => Ok(()),
            Err(error) => { sink.fail_original(error.clone())?; Err(error) }
        }
    }.boxed();
    Ok(LazyForwardRun { stream, forward, receipt, source_cancel: None })
}

fn make_source_cancel(factory: &Arc<GraphValueFactory>, bindings: &Arc<dyn RetryBindings>, tracker: Arc<ara_ai::retry::abort::AbortSourceTracker>) -> Arc<dyn Fn() -> Result<()> + Send + Sync> {
    let factory = Arc::downgrade(factory);
    let bindings = Arc::downgrade(bindings);
    let aborted = std::sync::atomic::AtomicBool::new(false);
    Arc::new(move || {
        if aborted.swap(true, std::sync::atomic::Ordering::AcqRel) { return Ok(()); }
        let factory = factory.upgrade().ok_or(ValueError::UnsupportedOperation("Lazy cancellation Session expired"))?;
        let bindings = bindings.upgrade().ok_or(ValueError::UnsupportedOperation("Lazy cancellation Run expired"))?;
        execute(&factory, bindings.as_ref(), |runtime| bindings.signals().abort(&factory, &tracker.request_abort_controller, None, runtime))
    })
}

fn native_admission_error(factory: &Arc<GraphValueFactory>, bindings: &dyn RetryBindings, signal: Option<&ValueHandle>, error: ara_ai::provider_dispatch::ProviderCoordinationError) -> Result<ValueError> {
    execute(factory, bindings, |runtime| {
        if let Some(signal) = signal {
            if factory.heap().truthy(&get(factory, signal, "aborted", runtime)?)? {
                let reason = get(factory, signal, "reason", runtime)?;
                if !nullish(factory, &reason)? { return Ok(ValueError::Thrown(reason)); }
            }
        }
        let error = factory.heap().construct(&bindings.error_constructor(), &[factory.heap().string(error.to_string().into())?], runtime)?;
        Ok(ValueError::Thrown(error))
    })
}

fn dispatch_registered_lazy_source(
    registry: &ExtensionRegistry,
    api: &WireString,
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    model: ValueHandle,
    context: ValueHandle,
    options: ValueHandle,
    mut reservation: Option<ProviderDispatchReservation>,
    source_close: Option<CancellationToken>,
    dispatch_join: Option<Arc<SourceDispatchJoin>>,
) -> std::result::Result<LazyForwardRun, ActualProviderError> {
    bindings.check_invocation(&factory)?;
    let handle = registry
        .builtin_provider_module(api)
        .or_else(|| {
            registry.snapshot().custom_apis.into_iter()
                .find(|entry| entry.api == *api)
                .map(|entry| entry.handle)
        })
        .ok_or(ValueError::UnsupportedOperation(
            "Lazy provider module or custom API is not registered",
        ))?;
    let registration = handle
        .downcast::<RegisteredLazyProvider>()
        .map_err(|_| {
            ValueError::UnsupportedOperation(
                "Registered module or custom API is not an executable lazy provider",
            )
        })?;
    let prepared = execute(&factory, bindings.as_ref(), |runtime| {
        let caller = get(&factory, &options, "signal", runtime)?;
        let caller = if nullish(&factory, &caller)? {
            None
        } else {
            Some(caller)
        };
        let tracker = Arc::new(create_abort_source_tracker(
            &factory,
            bindings.clone(),
            caller.clone(),
            runtime,
        )?);
        let provider_options = factory.heap().object()?;
        factory
            .heap()
            .spread_into(&provider_options, &options, runtime)?;
        factory.heap().define_own_data(
            &provider_options,
            "signal".into(),
            tracker.request_signal.clone(),
            runtime,
        )?;
        if let Some(ticket) = reservation.as_mut() {
            if let Err(error) = ticket.enter() {
                return Ok(Err(error));
            }
        }
        let source = match registration.provider.stream(
            &factory,
            &model,
            &context,
            &provider_options,
            runtime,
        ) {
            Ok(source) => source,
            Err(error) => {
                if let Some(ticket) = reservation.take() {
                    ticket.fail(error.clone());
                }
                return Ok(Err(ActualProviderError::from(error)));
            }
        };
        if let Some(join) = &dispatch_join {
            // Publish the authentic producer immediately after source Call,
            // before receipt attachment or any subsequent setup may fail.
            join.receipt.inherit_stream(&source.stream.events);
            *join.cancel.lock().map_err(|_| ValueError::PoisonedHeap)? = Some(make_source_cancel(&factory, &bindings, tracker.clone()));
        }
        if let Some(ticket) = reservation.take() {
            if let Err(error) = ticket.attach(source.stream.events.failure_receipt()) {
                return Ok(Err(error));
            }
        }
        Ok(Ok((caller, tracker, source)))
    });
    let (caller, tracker, source) = match prepared {
        Ok(result) => result?,
        Err(error) => {
            if let Some(ticket) = reservation.take() {
                ticket.fail(error.clone());
            }
            return Err(error.into());
        }
    };
    let source_cancel = dispatch_join.as_ref().and_then(|join| join.cancel.lock().ok().and_then(|cancel| cancel.clone()))
        .unwrap_or_else(|| make_source_cancel(&factory, &bindings, tracker.clone()));
    let (mut sink, stream, receipt) = ActualCompletionStream::channel(&factory);
    receipt.inherit_stream(&source.stream.events);
    let original_result = source.original_result;
    let source_tracker = source.stream.events.local_work_tracker();
    let local_work = source
        .local_work
        .unwrap_or_else(|| Arc::new(source_tracker));
    stream
        .events
        .forward_local_work_from(Some(local_work.clone()));
    // EOF cleanup closes the iterator's local child only. A pending original
    // result waiter still follows the original Run cancellation ancestor.
    let run_close = source_close.unwrap_or_else(CancellationToken::new);
    let pull = Arc::new(NativeStreamPull {
        identity: source.identity,
        stream: Arc::new(tokio::sync::Mutex::new(source.stream)),
        local_work,
        originals: Arc::new(Mutex::new(HashMap::new())),
        close: run_close.child_token(),
    });
    let watched_options = execute(&factory, bindings.as_ref(), |runtime| {
        let heap = factory.heap();
        let limits = &registration.limits;
        let compat = get(&factory, &model, "compat", runtime)?;
        let compat_idle = if heap.kind(&compat)? != ValueKind::Undefined
            && heap.has(&compat, &"streamIdleTimeoutMs".into(), runtime)?
        {
            option_number(&factory, &compat, "streamIdleTimeoutMs", runtime)?
        } else {
            None
        };
        let fallback = compat_idle.or(limits.default_idle_timeout_ms);
        let idle = if limits.provider_handles_stream_timeouts {
            None
        } else {
            match option_number(&factory, &options, "streamIdleTimeoutMs", runtime)? {
                Some(value) => Some(value),
                None if limits.openai_idle_env_floors_first_event => {
                    get_openai_stream_idle_timeout_ms(
                        &factory,
                        bindings.as_ref(),
                        fallback,
                        runtime,
                    )?
                }
                None => get_stream_idle_timeout_ms(&factory, bindings.as_ref(), fallback, runtime)?,
            }
        };
        let first = if limits.provider_handles_stream_timeouts
            || limits.provider_handles_first_event_timeouts
        {
            Some(0.0)
        } else {
            match option_number(&factory, &options, "streamFirstEventTimeoutMs", runtime)? {
                Some(value) => Some(value),
                None if limits.openai_idle_env_floors_first_event => {
                    get_openai_stream_first_event_timeout_ms(
                        &factory,
                        bindings.as_ref(),
                        idle,
                        limits.default_first_event_timeout_ms,
                        runtime,
                    )?
                }
                None => get_stream_first_event_timeout_ms(
                    &factory,
                    bindings.as_ref(),
                    idle,
                    limits.default_first_event_timeout_ms,
                    runtime,
                )?,
            }
        };
        let watched = heap.object()?;
        for (name, value) in [("idleTimeoutMs", idle), ("firstItemTimeoutMs", first)] {
            heap.define_own_data(
                &watched,
                name.into(),
                match value {
                    Some(value) => heap.number(value)?,
                    None => heap.undefined()?,
                },
                runtime,
            )?;
        }
        heap.define_own_data(
            &watched,
            "errorMessage".into(),
            heap.string(IDLE_ERROR.into())?,
            runtime,
        )?;
        heap.define_own_data(
            &watched,
            "firstItemErrorMessage".into(),
            heap.string(FIRST_ERROR.into())?,
            runtime,
        )?;
        heap.define_own_data(
            &watched,
            "abortSignal".into(),
            caller.clone().unwrap_or(heap.undefined()?),
            runtime,
        )?;
        heap.define_own_data(
            &watched,
            "isProgressItem".into(),
            heap.fresh_compiled_function(&PROGRESS, &[])?,
            runtime,
        )?;
        Ok(watched)
    })?;
    let mut callbacks = Vec::new();
    execute(&factory, bindings.as_ref(), |runtime| {
        for (name, message) in [("onIdle", IDLE_ERROR), ("onFirstItemTimeout", FIRST_ERROR)] {
            let owner = Arc::downgrade(&factory);
            let imports = Arc::downgrade(&bindings);
            let tracker = tracker.clone();
            let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
                let owner = owner
                    .upgrade()
                    .ok_or(ValueError::UnsupportedOperation("Lazy Session expired"))?;
                let imports = imports
                    .upgrade()
                    .ok_or(ValueError::UnsupportedOperation("Lazy Run expired"))?;
                execute(&owner, imports.as_ref(), |runtime| {
                    let reason =
                        imports
                            .signals()
                            .error(&owner, "StreamTimeoutError", message, runtime)?;
                    tracker.abort_locally(&owner, reason, runtime)?;
                    Ok(())
                })
            });
            let callable = bindings
                .signals()
                .callback(&factory, body.clone(), runtime)?;
            factory
                .heap()
                .define_own_data(&watched_options, name.into(), callable, runtime)?;
            callbacks.push(body);
        }
        Ok(())
    })?;
    let mut idle = IdleIterator::prepare_source(
        factory.clone(),
        bindings.clone(),
        pull.identity.clone(),
        pull.clone(),
        watched_options,
    )?;
    let source_guard = RustSourceCloseGuard(pull.clone());
    let forward_receipt = receipt.clone();
    let forward = async move {
        let _source_guard = source_guard;
        let _callbacks = callbacks;
        let outcome = async {
            loop {
                // A terminal push closes target event delivery, but does not
                // mean its original source iterator/result has been observed.
                let next = tokio::select! { biased; _ = sink.closed(), if !sink.is_done() => { idle.close()?; return Ok(()); }, next = idle.next() => next? };
                let Some(value) = next else {
                    if let Some(result) = &original_result {
                        // Only this waiter stops on original Run cancellation.
                        // The actual result Promise and source receipt remain
                        // owned by their original producer and are never replayed.
                        let result = tokio::select! { biased;
                            _ = run_close.cancelled() => return Err(ValueError::UnsupportedOperation("Lazy source result wait closed")),
                            // Receiver closure ends this waiter, not the original
                            // result Promise or the authentic producer receipt.
                            _ = sink.closed(), if !sink.is_done() => return Ok(()),
                            value = result.result() => value?,
                        };
                        sink.end_original(result)?;
                    } else if !sink.is_done() {
                        let missing = execute(&factory, bindings.as_ref(), |runtime| {
                            let heap = factory.heap();
                            let options = heap.object()?;
                            heap.define_own_data(&options, "kind".into(), heap.string("envelope".into())?, runtime)?;
                            heap.construct(&bindings.provider_response_error_constructor(),
                                &[heap.string("Stream ended without a final result".into())?, options], runtime)
                        })?;
                        sink.end_missing_result(ValueError::Thrown(missing))?;
                    }
                    return Ok(());
                };
                let event = pull.originals.lock().map_err(|_| ValueError::PoisonedHeap)?.remove(&value.node_id()).ok_or(ValueError::UnsupportedOperation("Lazy owner lost the original event identity"))?;
                if event.is_terminal() { forward_receipt.retain_terminal(&event); }
                if !sink.push(event).await? { idle.close()?; return Ok(()); }
            }
        }.await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error) if error.is_native_exception() => {
                let reason = execute(&factory, bindings.as_ref(), |runtime| tracker.was_caller_abort(&factory, runtime))?;
                let stop = if reason { StopReason::Aborted } else { StopReason::Error };
                let message = lazy_error_message(&factory, bindings.as_ref(), &registration.classify, &model, &error, stop)?;
                let event = ActualAssistantEvent::Error { reason: stop, error: message };
                forward_receipt.retain_terminal(&event);
                sink.push(event).await?;
                Ok(())
            }
            Err(error) => { forward_receipt.record(error.clone()); Err(error) }
        }
    }.boxed();
    Ok(LazyForwardRun {
        stream,
        forward,
        receipt,
        source_cancel: Some(source_cancel),
    })
}

fn lazy_error_message(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    classify: &ValueHandle,
    model: &ValueHandle,
    error: &ValueError,
    stop: StopReason,
) -> Result<ValueHandle> {
    execute(factory, bindings, |runtime| {
        let heap = factory.heap();
        let thrown = match error {
            ValueError::Thrown(value) => value.clone(),
            ValueError::TypeError(message)
            | ValueError::RangeError(message)
            | ValueError::SyntaxError(message) => heap.construct(
                &bindings.error_constructor(),
                &[heap.string(message.clone().into())?],
                runtime,
            )?,
            _ => return Err(error.clone()),
        };
        let api = get(factory, model, "api", runtime)?;
        let message = heap.object()?;
        let usage = heap.object()?;
        let cost = heap.object()?;
        for key in ["input", "output", "cacheRead", "cacheWrite", "total"] {
            heap.define_own_data(&cost, key.into(), heap.number(0.0)?, runtime)?;
        }
        for key in ["input", "output", "cacheRead", "cacheWrite", "totalTokens"] {
            heap.define_own_data(&usage, key.into(), heap.number(0.0)?, runtime)?;
        }
        heap.define_own_data(&usage, "cost".into(), cost, runtime)?;
        for (key, value) in [
            ("role", heap.string("assistant".into())?),
            ("content", heap.array(Vec::new())?),
            ("api", api.clone()),
            ("provider", get(factory, model, "provider", runtime)?),
            ("model", get(factory, model, "id", runtime)?),
            ("usage", usage),
            (
                "stopReason",
                heap.string(
                    if stop == StopReason::Aborted {
                        "aborted"
                    } else {
                        "error"
                    }
                    .into(),
                )?,
            ),
        ] {
            heap.define_own_data(&message, key.into(), value, runtime)?;
        }
        let id = if stop == StopReason::Error {
            heap.call(
                classify,
                &heap.undefined()?,
                &[thrown.clone(), api],
                runtime,
            )?
        } else {
            heap.undefined()?
        };
        heap.define_own_data(
            &message,
            "errorId".into(),
            if heap.truthy(&id)? {
                id
            } else {
                heap.undefined()?
            },
            runtime,
        )?;
        let text = if stop == StopReason::Aborted {
            heap.string("Request was aborted".into())?
        } else if heap.instance_of(&thrown, &bindings.error_constructor(), runtime)? {
            get(factory, &thrown, "message", runtime)?
        } else {
            heap.string(heap.to_string(&thrown, runtime)?)?
        };
        heap.define_own_data(&message, "errorMessage".into(), text, runtime)?;
        let date = get(factory, &bindings.globals(), "Date", runtime)?;
        let now = get(factory, &date, "now", runtime)?;
        heap.define_own_data(
            &message,
            "timestamp".into(),
            heap.call(&now, &date, &[], runtime)?,
            runtime,
        )?;
        Ok(message)
    })
}

/// Concrete production poll entry: uses the supplied original NativeRun and
/// its current driver, while consuming the registered provider's outer events.
pub async fn consume_registered_lazy(
    native_run: NativeRun,
    invocation: LazyForwardRun,
) -> std::result::Result<Vec<ActualAssistantEvent>, ActualProviderError> {
    let receipt = invocation.receipt.clone();
    let outcome = native_run.drive(collect_registered_lazy(invocation)).await;
    // The outer caller retains this receipt even if its awaiting future is
    // dropped. This join itself borrows no graph runtime or native driver.
    let failures = receipt.wait_settled().await;
    let mut result = outcome.and_then(|result| result.map_err(ActualProviderError::from));
    for failure in failures {
        let secondary = ActualProviderError::from((*failure).clone());
        result = Err(match result {
            Ok(_) => secondary,
            Err(primary) => ActualProviderError::Compound {
                primary: Arc::new(primary),
                secondary: Arc::new(secondary),
            },
        });
    }
    result
}

/// The one forwarding/event consumer polled by the original Host driver.
/// Exposed separately for same-driver Host integrations and deterministic
/// original-source consumers; this function never creates a native driver.
pub async fn collect_registered_lazy(
    invocation: LazyForwardRun,
) -> Result<Vec<ActualAssistantEvent>> {
    let LazyForwardRun {
        mut stream,
        forward,
        receipt: _,
        source_cancel: _,
    } = invocation;
    let work = async move {
        let collect = async {
            let mut events = Vec::new();
            while let Some(event) = stream.recv().await? {
                events.push(event);
            }
            Ok::<_, ValueError>(events)
        };
        let (_, events) = tokio::try_join!(forward, collect)?;
        Ok(events)
    };
    work.await
}
static PROGRESS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da register-builtins.ts:289 event => event.type !== start",
    name: "isProgressItem",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Arrow,
    body: |heap, _, _, args, runtime| {
        let event = args.first().cloned().unwrap_or(heap.undefined()?);
        let kind = heap.get(&event, &"type".into(), runtime)?;
        heap.boolean(!heap.object_is(&kind, &heap.string("start".into())?)?)
    },
};
