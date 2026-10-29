//! Frozen issuing-Run borrow and original Promise adoption. This contains no
//! runtime Arc or Promise driver and cannot pump graph jobs from a worker.
use crate::native_realm::{
    context::NativeScope,
    parent::{NativeParentId, NativeParentLease},
    promise::{NativePromiseAdmission, NativePromiseState, NativePromiseTicket},
};
use ara_ai::{
    ActualStreamFailureReceipt,
    graph_values::{GraphExecution, GraphValueFactory},
    event::LocalWorkTicket,
    retry::{NativeWorkObservation, TimerLease},
};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, ValueError, ValueHandle, ValueHeap, ValueRuntime,
};
use futures::future::BoxFuture;
use std::sync::{Arc, Weak, Mutex, OnceLock, atomic::{AtomicU64, Ordering}};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone)]
pub struct NativeCallingRun {
    factory: Weak<GraphValueFactory>,
    admission: NativePromiseAdmission,
    scope: NativeScope,
    parent: NativeParentId,
    // Metadata only, not the original InvocationRuntime Arc. Shared clones
    // retain one operation reference until the final frozen binding is gone.
    _lease: Arc<NativeParentLease>,
}
impl NativeCallingRun {
    /// Synchronous original-work adoption and finally-handler registration.
    /// Only the existing Promise driver invokes the handlers; this future
    /// observes publication and never borrows a runtime from a worker.
    pub fn adopt_local_work(
        &self,
        factory: Arc<GraphValueFactory>,
        globals: ValueHandle,
        value: ValueHandle,
        ticket: LocalWorkTicket,
    ) -> Result<BoxFuture<'static, NativeWorkObservation>> {
        self.check(&factory)?;
        let heap = factory.heap();
        heap.adopt(&value)?;
        heap.adopt(&globals)?;
        let original = self.admission.prepare_pending(self.scope, self.parent)?;
        let wrapper = self.admission.prepare_pending(self.scope, self.parent)?;
        let id = LOCAL_FINALLY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        if id > (1u64 << 53) { return Err(ValueError::Allocation); }
        let key = (heap.heap_token(), id);
        let wrapper_ticket = wrapper.ticket();
        let state = Arc::new(LocalFinallyState { ticket, heap: Arc::downgrade(heap), published: Mutex::new(LocalFinallyPublication::default()) });
        local_finally_entries().lock().map_err(|_| ValueError::PoisonedHeap)?.insert(key, Arc::downgrade(&state));
        let owner = LocalFinallyOwner { key, state };
        let captures = [globals, wrapper.resolve.clone(), wrapper.reject.clone(), heap.number(id as f64)?];
        let fulfilled = heap.fresh_compiled_function(&LOCAL_FINALLY_FULFILL, &captures)?;
        let rejected = heap.fresh_compiled_function(&LOCAL_FINALLY_REJECT, &captures)?;
        self.admission.attach_native_await(&original.ticket(), &fulfilled, &rejected, self.scope, self.parent)?;
        // The async function's returned Promise may adopt a thenable again
        // after finally reenters/mutates the original fulfillment object.
        let derived_fulfilled = heap.fresh_compiled_function(&LOCAL_DERIVED_FULFILL, &[captures[3].clone()])?;
        let derived_rejected = heap.fresh_compiled_function(&LOCAL_DERIVED_REJECT, &[captures[3].clone()])?;
        self.admission.attach_native_await(&wrapper_ticket, &derived_fulfilled, &derived_rejected, self.scope, self.parent)?;
        self.with_runtime(&factory, &mut |runtime| heap.call(&original.resolve, &heap.undefined()?, &[value.clone()], runtime))?;
        let calling = self.clone();
        Ok(Box::pin(async move {
            let _owner = owner;
            loop {
                let wake = calling.admission.wake();
                let changed = wake.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                // An authoritative original callback wins a later revoke.
                // It already ran finally; never relabel that known settlement.
                if let Some(observation) = _owner.state.observation() { return observation; }
                if let Err(error) = calling.check(&factory) {
                    return _owner.state.failure(error);
                }
                match calling.admission.ticket_state(&wrapper_ticket) {
                    Ok(NativePromiseState::Pending) => changed.await,
                    Ok(NativePromiseState::Fulfilled(value)) => {
                        _owner.state.publish_derived(DeferredWorkOutcome::Value(value));
                        return _owner.state.observation().unwrap_or_else(|| _owner.state.failure(ValueError::InvalidNode));
                    },
                    Ok(NativePromiseState::Rejected(value)) => {
                        _owner.state.publish_derived(DeferredWorkOutcome::Error(DeferredWorkError::Thrown(value)));
                        return _owner.state.observation().unwrap_or_else(|| _owner.state.failure(ValueError::InvalidNode));
                    },
                    Err(error) => return _owner.state.failure(error),
                }
            }
        }))
    }
    pub fn capture(
        factory: &Arc<GraphValueFactory>,
        values: &ara_agent::SessionValues,
        admission: NativePromiseAdmission,
        effective: &dyn ValueRuntime,
    ) -> Result<Self> {
        admission.check_values(values)?;
        if !Arc::ptr_eq(factory.heap(), values.heap()) {
            return Err(ValueError::ForeignHeap);
        }
        let scope = admission.capture_scope(effective)?;
        let parent = admission.capture_parent(effective)?;
        // Capture is authorized by the effective parent, never by supplied IDs.
        let lease = Arc::new(admission.retain_parent(parent)?);
        admission.scoped(scope, parent)?;
        Ok(Self {
            factory: Arc::downgrade(factory),
            admission,
            scope,
            parent,
            _lease: lease,
        })
    }
    pub fn check(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Retry Session factory expired",
            ))?;
        if !Arc::ptr_eq(factory, &original) {
            return Err(ValueError::ForeignHeap);
        }
        self.admission.check_live()?;
        self.admission.scoped(self.scope, self.parent)?;
        Ok(())
    }
    pub fn with_runtime(
        &self,
        factory: &Arc<GraphValueFactory>,
        operation: &mut dyn FnMut(&dyn ValueRuntime) -> Result<ValueHandle>,
    ) -> Result<ValueHandle> {
        self.check(factory)?;
        // This local borrow must end before a future yields or an OS worker is
        // started. The closure is called once; its original error is returned.
        let runtime = self.admission.scoped(self.scope, self.parent)?;
        operation(&runtime)
    }
    /// Reverse lookup only: an expired/different epoch is not this original
    /// callback registration. Other infrastructure errors remain observable.
    pub(crate) fn matches_runtime(
        &self,
        factory: &Arc<GraphValueFactory>,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        let observed = (|| {
            self.check(factory)?;
            Ok(self.admission.capture_scope(effective)? == self.scope
                && self.admission.capture_parent(effective)? == self.parent)
        })();
        match observed {
            Err(ValueError::ExpiredCapability(_)) => Ok(false),
            other => other,
        }
    }
    pub fn await_value(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, Result<ValueHandle>> {
        let observed = self.await_observed(factory, value);
        Box::pin(async move { observed.await.result })
    }
    pub fn await_observed(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, NativeWorkObservation> {
        let prepared: Result<NativePromiseTicket> = (|| {
            self.check(&factory)?;
            factory.heap().adopt(&value)?;
            let pending = self.admission.prepare_pending(self.scope, self.parent)?;
            self.with_runtime(&factory, &mut |runtime| {
                factory.heap().call(
                    &pending.resolve,
                    &factory.heap().undefined()?,
                    std::slice::from_ref(&value),
                    runtime,
                )
            })?;
            Ok(pending.ticket())
        })();
        let calling = self.clone();
        Box::pin(async move {
            let ticket = match prepared {
                Ok(ticket) => ticket,
                Err(error) => {
                    return NativeWorkObservation {
                        result: Err(error),
                        settled: false,
                    };
                }
            };
            loop {
                let wake = calling.admission.wake();
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Err(error) = calling.check(&factory) {
                    return NativeWorkObservation {
                        result: Err(error),
                        settled: false,
                    };
                }
                let state = match calling.admission.ticket_state(&ticket) {
                    Ok(state) => state,
                    Err(error) => {
                        return NativeWorkObservation {
                            result: Err(error),
                            settled: false,
                        };
                    }
                };
                match state {
                    NativePromiseState::Fulfilled(value) => {
                        return NativeWorkObservation {
                            result: factory.heap().restore_deferred(&value),
                            settled: true,
                        };
                    }
                    NativePromiseState::Rejected(value) => {
                        return NativeWorkObservation {
                            result: factory
                                .heap()
                                .restore_deferred(&value)
                                .and_then(|value| Err(ValueError::Thrown(value))),
                            settled: true,
                        };
                    }
                    NativePromiseState::Pending => notified.await,
                }
            }
        })
    }
    pub fn invocation_ended(&self) -> BoxFuture<'static, ValueError> {
        let calling = self.clone();
        Box::pin(async move {
            loop {
                let wake = calling.admission.wake();
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                if let Err(error) = calling.admission.check_live() {
                    return error;
                }
                notified.await;
            }
        })
    }
    /// Private publication primitive. Its caller must latch callback suppression
    /// before clearing this lease; clear intentionally still publishes the tick
    /// so the existing driver releases the original native awaiter.
    pub(crate) fn dispatch_after(
        &self,
        factory: &Arc<GraphValueFactory>,
        delay_ms: f64,
        compiled_callback: ValueHandle,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<TimerLease> {
        self.check(factory)?;
        // NativeTimerDomain supplies the original Node delay coercion. Reject
        // accidental direct invalid imports rather than panic in Duration.
        if !delay_ms.is_finite() || !(1.0..=2_147_483_647.0).contains(&delay_ms) {
            return Err(ValueError::UnsupportedOperation(
                "Timer dispatch requires the coerced source delay",
            ));
        }
        let pending = self.admission.prepare_pending(self.scope, self.parent)?;
        let reject = factory.heap().fresh_compiled_function(&TIMER_REJECT, &[])?;
        self.admission.attach_native_await(
            &pending.ticket(),
            &compiled_callback,
            &reject,
            self.scope,
            self.parent,
        )?;
        let tick = self.admission.prepare_timer_tick(&pending.ticket())?;
        let admission = self.admission.clone();
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        // No factory, runtime, ValueHandle or parent lease goes to this worker.
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = worker_cancel.cancelled() => {},
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay_ms.trunc() as u64)) => {},
            }
            if let Err(error) = admission.publish_timer_tick(&tick) {
                receipt.record(error);
            }
        });
        Ok(TimerLease::new(Arc::new(move || {
            cancel.cancel();
            Ok(())
        })))
    }
}

#[derive(Clone)]
enum DeferredWorkError {
    Thrown(DeferredValue),
    IteratorClose { original: Box<DeferredWorkError>, close: Box<DeferredWorkError> },
    Other(ValueError),
}
impl DeferredWorkError {
    fn capture(heap: &Arc<ValueHeap>, error: &ValueError) -> Result<Self> {
        Ok(match error {
            ValueError::Thrown(value) => Self::Thrown(heap.capture_deferred(value)?),
            ValueError::IteratorClose { original, close } => Self::IteratorClose { original: Box::new(Self::capture(heap, original)?), close: Box::new(Self::capture(heap, close)?) },
            // All other current ValueError variants contain no ValueHandle.
            other => Self::Other(other.clone()),
        })
    }
    fn restore(&self, heap: &Arc<ValueHeap>) -> Result<ValueError> {
        Ok(match self {
            Self::Thrown(value) => ValueError::Thrown(heap.restore_deferred(value)?),
            Self::IteratorClose { original, close } => ValueError::IteratorClose { original: Box::new(original.restore(heap)?), close: Box::new(close.restore(heap)?) },
            Self::Other(error) => error.clone(),
        })
    }
}
#[derive(Clone)]
enum DeferredWorkOutcome { Value(DeferredValue), Error(DeferredWorkError) }
impl DeferredWorkOutcome {
    fn capture(heap: &Arc<ValueHeap>, result: &Result<ValueHandle>) -> Result<Self> {
        match result { Ok(value) => Ok(Self::Value(heap.capture_deferred(value)?)), Err(error) => Ok(Self::Error(DeferredWorkError::capture(heap, error)?)) }
    }
    fn restore(&self, heap: &Arc<ValueHeap>) -> Result<ValueHandle> {
        match self { Self::Value(value) => heap.restore_deferred(value), Self::Error(error) => Err(error.restore(heap)?) }
    }
}
#[derive(Default)]
struct LocalFinallyPublication {
    original_known: bool,
    original: Option<DeferredWorkOutcome>,
    derived: Option<DeferredWorkOutcome>,
}
struct LocalFinallyState {
    ticket: LocalWorkTicket,
    heap: Weak<ValueHeap>,
    published: Mutex<LocalFinallyPublication>,
}
impl LocalFinallyState {
    fn observation(&self) -> Option<NativeWorkObservation> {
        let result = {
            let state = self.published.lock().unwrap_or_else(|e| e.into_inner());
            // Normal observation waits for the real wrapper resolution,
            // including async return-value assimilation after finally.
            state.derived.clone()
        }?;
        let result = match self.heap.upgrade() {
            Some(heap) => result.restore(&heap),
            None => Err(ValueError::UnsupportedOperation("Settled local-work heap expired")),
        };
        Some(NativeWorkObservation { result, settled: true })
    }
    fn failure(&self, error: ValueError) -> NativeWorkObservation {
        if let Some(observation) = self.observation() { return observation; }
        let (known, original) = {
            let state = self.published.lock().unwrap_or_else(|e| e.into_inner());
            (state.original_known, state.original.clone())
        };
        // After revoke, an original finally rejection is already known. A
        // successful original value whose derived thenable is still pending
        // retains work-settled=true but cannot fabricate a derived success.
        if let Some(DeferredWorkOutcome::Error(original)) = original {
            let result = match self.heap.upgrade() { Some(heap) => original.restore(&heap).and_then(Err), None => Err(ValueError::UnsupportedOperation("Settled local-work heap expired")) };
            return NativeWorkObservation { result, settled: true };
        }
        NativeWorkObservation { result: Err(error), settled: known }
    }
    fn publish_derived(&self, result: DeferredWorkOutcome) {
        let mut state = self.published.lock().unwrap_or_else(|e| e.into_inner());
        // Non-handle native failures use the private wrapper only as a wake;
        // the exact original error is never replaced by its undefined tick.
        let result = match &state.original { Some(DeferredWorkOutcome::Error(error)) => DeferredWorkOutcome::Error(error.clone()), _ => result };
        state.derived.get_or_insert(result);
    }
}
struct LocalFinallyOwner { key: (u64,u64), state: Arc<LocalFinallyState> }
impl Drop for LocalFinallyOwner { fn drop(&mut self) { if let Ok(mut entries) = local_finally_entries().lock() { entries.remove(&self.key); } } }
static LOCAL_FINALLY_SEQUENCE: AtomicU64 = AtomicU64::new(1);
fn local_finally_entries() -> &'static Mutex<std::collections::HashMap<(u64,u64),Weak<LocalFinallyState>>> {
    static ENTRIES: OnceLock<Mutex<std::collections::HashMap<(u64,u64),Weak<LocalFinallyState>>>> = OnceLock::new();
    ENTRIES.get_or_init(Mutex::default)
}
fn local_finally(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>, captures: &[ValueHandle], args: &[ValueHandle],
    runtime: &dyn ValueRuntime, rejected: bool,
) -> Result<ValueHandle> {
    let ara_rpc::value_heap::ValueSnapshot::Number(id) = heap.snapshot(&captures[3])? else { return Err(ValueError::InvalidNode); };
    let state = local_finally_entries().lock().map_err(|_| ValueError::PoisonedHeap)?.get(&(heap.heap_token(), id as u64)).and_then(Weak::upgrade).ok_or(ValueError::UnsupportedOperation("Original local-work finally owner expired"))?;
    let value = args.first().cloned().unwrap_or(heap.undefined()?);
    let original = if rejected { Err(ValueError::Thrown(value)) } else { Ok(value) };
    let observation = state.ticket.finish_in_original(GraphExecution::new(heap, runtime), &captures[0], original);
    let (setter, value) = match &observation.result {
        Ok(value) => (&captures[1], value.clone()),
        Err(ValueError::Thrown(reason)) => (&captures[2], reason.clone()),
        // The private wrapper is a notification only. Non-handle native
        // errors stay in the original observation, never fabricated strings.
        Err(_) => (&captures[1], heap.undefined()?),
    };
    {
        let mut publication = state.published.lock().map_err(|_| ValueError::PoisonedHeap)?;
        publication.original_known = true;
        publication.original = Some(DeferredWorkOutcome::capture(heap, &observation.result).unwrap_or_else(|error| DeferredWorkOutcome::Error(DeferredWorkError::Other(error))));
    }
    heap.call(setter, &heap.undefined()?, &[value], runtime)?;
    heap.undefined()
}
fn local_derived(heap: &Arc<ValueHeap>, captures: &[ValueHandle], args: &[ValueHandle], rejected: bool) -> Result<ValueHandle> {
    let ara_rpc::value_heap::ValueSnapshot::Number(id) = heap.snapshot(&captures[0])? else { return Err(ValueError::InvalidNode); };
    let state = local_finally_entries().lock().map_err(|_| ValueError::PoisonedHeap)?.get(&(heap.heap_token(), id as u64)).and_then(Weak::upgrade);
    // The native waiter may already have observed the actual wrapper state.
    // Its completion releases this metadata owner; no new work is admitted.
    if let Some(state) = state {
        let value = args.first().cloned().unwrap_or(heap.undefined()?);
        let value = heap.capture_deferred(&value)?;
        state.publish_derived(if rejected { DeferredWorkOutcome::Error(DeferredWorkError::Thrown(value)) } else { DeferredWorkOutcome::Value(value) });
    }
    heap.undefined()
}
static LOCAL_DERIVED_FULFILL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP trackLocalWork derived async result after original finally", name:"localWorkResult", length:1,
    capture_count:1, form:CompiledFunctionForm::Arrow,
    body: |heap,captures,_,args,_| local_derived(heap,captures,args,false),
};
static LOCAL_DERIVED_REJECT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP trackLocalWork derived async rejection after original finally", name:"localWorkFailure", length:1,
    capture_count:1, form:CompiledFunctionForm::Arrow,
    body: |heap,captures,_,args,_| local_derived(heap,captures,args,true),
};
static LOCAL_FINALLY_FULFILL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91 event-stream.trackLocalWork original fulfilled finally", name:"localWorkFulfilled", length:1,
    capture_count:4, form:CompiledFunctionForm::Arrow,
    body: |heap,captures,_,args,runtime| local_finally(heap,captures,args,runtime,false),
};
static LOCAL_FINALLY_REJECT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91 event-stream.trackLocalWork original rejected finally", name:"localWorkRejected", length:1,
    capture_count:4, form:CompiledFunctionForm::Arrow,
    body: |heap,captures,_,args,runtime| local_finally(heap,captures,args,runtime,true),
};

static TIMER_REJECT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ARA issuing-Run timer native-await rejection propagator",
    name: "timerRejected",
    length: 1,
    capture_count: 0,
    form: CompiledFunctionForm::Arrow,
    body: |heap, _, _, args, _| {
        Err(ValueError::Thrown(
            args.first().cloned().unwrap_or(heap.undefined()?),
        ))
    },
};
