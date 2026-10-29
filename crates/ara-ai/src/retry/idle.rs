//! Fixed OMP596f2da utils/idle-iterator.ts Rust generator owners (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::{
    RetryBindings, TimerLease,
    abort::{AbortListener, caller_aborted, nullish},
    execute,
};
use crate::graph_values::{GraphExecution, GraphValueFactory};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueKind, ValueRuntime};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Notify;

type Result<T> = std::result::Result<T, ValueError>;
const DEFAULT_TIMEOUT: f64 = 300_000.0;

fn get(
    factory: &Arc<GraphValueFactory>,
    object: &ValueHandle,
    name: &str,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    factory.heap().get(object, &name.into(), runtime)
}
fn method(
    factory: &Arc<GraphValueFactory>,
    object: &ValueHandle,
    name: &str,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let callable = get(factory, object, name, runtime)?;
    factory.heap().call(&callable, object, args, runtime)
}
fn now(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    runtime: &dyn ValueRuntime,
) -> Result<f64> {
    let value = now_raw(factory, bindings, runtime)?;
    factory.heap().to_number(&value, runtime)
}
fn now_raw(factory: &Arc<GraphValueFactory>, bindings: &dyn RetryBindings, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
    let date = get(factory, &bindings.globals(), "Date", runtime)?;
    method(factory, &date, "now", &[], runtime)
}
fn number_option(
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
fn compare(
    factory: &Arc<GraphValueFactory>,
    value: &ValueHandle,
    right: f64,
    op: &str,
    runtime: &dyn ValueRuntime,
) -> Result<bool> {
    crate::schema_validator::graph_values_compare_with_execution(
        GraphExecution::new(factory.heap(), runtime),
        value,
        &factory.heap().number(right)?,
        op,
    )
}
fn normalize(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    raw: Option<String>,
    fallback: f64,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    let Some(raw) = raw else {
        return Ok(Some(fallback));
    };
    let number = get(factory, &bindings.globals(), "Number", runtime)?;
    let parsed = factory.heap().call(
        &number,
        &factory.heap().undefined()?,
        &[factory.heap().string(raw.into())?],
        runtime,
    )?;
    let finite = method(
        factory,
        &number,
        "isFinite",
        std::slice::from_ref(&parsed),
        runtime,
    )?;
    if !factory.heap().truthy(&finite)? {
        return Ok(Some(fallback));
    }
    if compare(factory, &parsed, 0.0, "<=", runtime)? {
        return Ok(None);
    }
    let math = get(factory, &bindings.globals(), "Math", runtime)?;
    let value = method(factory, &math, "trunc", &[parsed], runtime)?;
    Ok(Some(factory.heap().to_number(&value, runtime)?))
}
fn maximum(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    left: f64,
    right: f64,
    runtime: &dyn ValueRuntime,
) -> Result<f64> {
    let math = get(factory, &bindings.globals(), "Math", runtime)?;
    let value = method(
        factory,
        &math,
        "max",
        &[factory.heap().number(left)?, factory.heap().number(right)?],
        runtime,
    )?;
    factory.heap().to_number(&value, runtime)
}
pub fn get_stream_idle_timeout_ms(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    fallback: Option<f64>,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    normalize(
        factory,
        bindings,
        bindings
            .environment("PI_STREAM_IDLE_TIMEOUT_MS")
            .or_else(|| bindings.environment("PI_OPENAI_STREAM_IDLE_TIMEOUT_MS")),
        fallback.unwrap_or(DEFAULT_TIMEOUT),
        runtime,
    )
}
pub fn get_openai_stream_idle_timeout_ms(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    fallback: Option<f64>,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    normalize(
        factory,
        bindings,
        bindings
            .environment("PI_OPENAI_STREAM_IDLE_TIMEOUT_MS")
            .or_else(|| bindings.environment("PI_STREAM_IDLE_TIMEOUT_MS")),
        fallback.unwrap_or(DEFAULT_TIMEOUT),
        runtime,
    )
}
pub fn get_stream_first_event_timeout_ms(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    idle: Option<f64>,
    fallback: Option<f64>,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    let fallback = fallback.unwrap_or(DEFAULT_TIMEOUT);
    let fallback = match idle {
        Some(idle) => maximum(factory, bindings, fallback, idle, runtime)?,
        None => fallback,
    };
    normalize(
        factory,
        bindings,
        bindings.environment("PI_STREAM_FIRST_EVENT_TIMEOUT_MS"),
        fallback,
        runtime,
    )
}
pub fn get_openai_stream_first_event_timeout_ms(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    idle: Option<f64>,
    fallback: Option<f64>,
    runtime: &dyn ValueRuntime,
) -> Result<Option<f64>> {
    let fallback = fallback.unwrap_or(DEFAULT_TIMEOUT);
    if let Some(raw) = bindings.environment("PI_OPENAI_STREAM_FIRST_EVENT_TIMEOUT_MS") {
        return Ok(Some(
            normalize(factory, bindings, Some(raw), fallback, runtime)?.unwrap_or(0.0),
        ));
    }
    let base = normalize(
        factory,
        bindings,
        bindings.environment("PI_STREAM_FIRST_EVENT_TIMEOUT_MS"),
        fallback,
        runtime,
    )?;
    let Some(base) = base.filter(|value| *value > 0.0) else {
        return Ok(Some(0.0));
    };
    match idle.filter(|value| *value > 0.0) {
        Some(idle) => Ok(Some(maximum(factory, bindings, base, idle, runtime)?)),
        None => Ok(Some(base)),
    }
}

pub struct PreResponseTimeout {
    pub signal: Option<ValueHandle>,
    timer: Option<TimerLease>,
}
impl PreResponseTimeout {
    pub fn clear(&self) -> Result<()> {
        match &self.timer {
            Some(timer) => timer.clear(),
            None => Ok(()),
        }
    }
}
/// Source only disables undefined or <=0. NaN/Infinity reach the Host timer.
pub fn arm_pre_response_timeout(
    factory: &Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    caller: Option<ValueHandle>,
    timeout: ValueHandle,
) -> Result<PreResponseTimeout> {
    bindings.check_invocation(factory)?;
    let controller = execute(factory, bindings.as_ref(), |runtime| {
        if factory.heap().kind(&timeout)? == ValueKind::Undefined
            || compare(factory, &timeout, 0.0, "<=", runtime)?
        {
            return Ok(None);
        }
        let controller = bindings.signals().controller(factory, runtime)?;
        let delay = factory.heap().to_number(&timeout, runtime)?;
        Ok(Some((controller, delay)))
    })?;
    let Some((controller, delay)) = controller else {
        return Ok(PreResponseTimeout {
            signal: caller,
            timer: None,
        });
    };
    let owner = Arc::downgrade(factory);
    let imports = Arc::downgrade(&bindings);
    let captured = controller.clone();
    let timer = bindings.timers().arm(
        delay,
        Arc::new(move || {
            let owner = owner.upgrade().ok_or(ValueError::UnsupportedOperation(
                "Pre-response Session expired",
            ))?;
            let imports = imports
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation("Pre-response Run expired"))?;
            execute(&owner, imports.as_ref(), |runtime| {
                let reason = imports.signals().error(
                    &owner,
                    "TimeoutError",
                    "The operation timed out.",
                    runtime,
                )?;
                imports
                    .signals()
                    .abort(&owner, &captured, Some(reason), runtime)
            })
        }),
    )?;
    let combined = execute(factory, bindings.as_ref(), |runtime| {
        match caller.as_ref() {
            Some(caller) if factory.heap().truthy(caller)? => bindings.signals().any(
                factory,
                &[caller.clone(), controller.signal.clone()],
                runtime,
            ),
            _ => Ok(controller.signal.clone()),
        }
    });
    let signal = match combined {
        Ok(signal) => signal,
        Err(error) => {
            timer.clear()?;
            return Err(error);
        }
    };
    Ok(PreResponseTimeout {
        signal: Some(signal),
        timer: Some(timer),
    })
}

struct WatchState {
    first: bool,
    first_budget: Option<f64>,
    first_deadline: Option<f64>,
    last_progress: f64,
    timer: Option<TimerLease>,
    timer_at: f64,
    generation: u64,
    fired: bool,
    closed: bool,
    callback_error: Option<ValueError>,
}
struct IdleWatch {
    factory: Weak<GraphValueFactory>,
    bindings: Weak<dyn RetryBindings>,
    options: ValueHandle,
    local_work: Option<Arc<dyn IdlePullSource>>,
    state: Mutex<WatchState>,
    changed: Notify,
}
impl IdleWatch {
    fn imports(&self) -> Result<(Arc<GraphValueFactory>, Arc<dyn RetryBindings>)> {
        Ok((
            self.factory
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation("Idle Session expired"))?,
            self.bindings
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation("Idle Run expired"))?,
        ))
    }
    fn deadline(&self) -> Result<Option<f64>> {
        let (factory, bindings) = self.imports()?;
        let (first, deadline, last) = {
            let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            (state.first, state.first_deadline, state.last_progress)
        };
        execute(&factory, bindings.as_ref(), |runtime| {
            let heap = factory.heap();
            if first {
                let budget = self.state.lock().unwrap_or_else(|e| e.into_inner()).first_budget;
                let (Some(deadline), Some(budget)) = (deadline, budget) else { return Ok(deadline) };
                let settled = local_work_settled_at(&factory, bindings.as_ref(), &self.options, self.local_work.as_deref(), runtime)?;
                if !compare(&factory, &settled, 0.0, ">", runtime)? { return Ok(Some(deadline)); }
                let math = get(&factory, &bindings.globals(), "Math", runtime)?;
                let max = get(&factory, &math, "max", runtime)?;
                let renewed = heap.add(&settled, &heap.number(budget)?, runtime)?;
                let value = heap.call(&max, &math, &[heap.number(deadline)?, renewed], runtime)?;
                return Ok(Some(heap.to_number(&value, runtime)?));
            }
            // Source intentionally reads this mutable property at each
            // expression: undefined guard, positive guard, then RHS of +.
            let configured = get(&factory, &self.options, "idleTimeoutMs", runtime)?;
            if heap.kind(&configured)? == ValueKind::Undefined { return Ok(None); }
            let positive = get(&factory, &self.options, "idleTimeoutMs", runtime)?;
            if !compare(&factory, &positive, 0.0, ">", runtime)? { return Ok(None); }
            let math = get(&factory, &bindings.globals(), "Math", runtime)?;
            let max = get(&factory, &math, "max", runtime)?;
            let settled = local_work_settled_at(&factory, bindings.as_ref(), &self.options, self.local_work.as_deref(), runtime)?;
            let value = heap.call(&max, &math, &[heap.number(last)?, settled], runtime)?;
            let budget = get(&factory, &self.options, "idleTimeoutMs", runtime)?;
            let deadline = heap.add(&value, &budget, runtime)?;
            Ok(Some(heap.to_number(&deadline, runtime)?))
        })
    }

    fn arm(self: &Arc<Self>, deadline: f64) -> Result<()> {
        let (factory, bindings) = self.imports()?;
        let (prior, generation) = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.closed {
                return Ok(());
            }
            if state.fired {
                state.fired = false;
            }
            if state.timer.is_some() && state.timer_at <= deadline {
                return Ok(());
            }
            state.generation = state.generation.wrapping_add(1);
            state.timer_at = deadline;
            (state.timer.take(), state.generation)
        };
        if let Some(prior) = prior {
            prior.clear()?;
        }
        let delay = execute(&factory, bindings.as_ref(), |runtime| {
            Ok((deadline - now(&factory, bindings.as_ref(), runtime)?).max(0.0))
        })?;
        let weak = Arc::downgrade(self);
        let timer = bindings.timers().arm(
            delay,
            Arc::new(move || {
                let Some(watch) = weak.upgrade() else {
                    return Ok(());
                };
                if let Err(error) = watch.fire(generation) {
                    watch
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .callback_error = Some(error.clone());
                    watch.changed.notify_waiters();
                    return Err(error);
                }
                Ok(())
            }),
        )?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.closed && state.generation == generation {
            state.timer = Some(timer);
        } else {
            drop(state);
            timer.clear()?;
        }
        Ok(())
    }
    fn fire(self: &Arc<Self>, generation: u64) -> Result<()> {
        {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            if state.closed || state.generation != generation {
                return Ok(());
            }
            state.timer = None;
            state.timer_at = f64::INFINITY;
        }
        let Some(deadline) = self.deadline()? else {
            return Ok(());
        };
        let (factory, bindings) = self.imports()?;
        let remaining = execute(&factory, bindings.as_ref(), |runtime| {
            Ok(deadline - now(&factory, bindings.as_ref(), runtime)?)
        })?;
        if remaining > 0.0 {
            return self.arm(deadline);
        }
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .fired = true;
        self.changed.notify_waiters();
        Ok(())
    }
    fn clear(&self) -> Result<()> {
        let prior = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.closed = true;
            state.generation = state.generation.wrapping_add(1);
            state.fired = true;
            state.timer.take()
        };
        self.changed.notify_waiters();
        if let Some(prior) = prior {
            prior.clear()?;
        }
        Ok(())
    }
    async fn timeout(&self) -> Result<()> {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                if let Some(error) = &state.callback_error {
                    return Err(error.clone());
                }
                if state.fired {
                    return Ok(());
                }
            }
            changed.await;
        }
    }
}

/// Pull owner used by real Host adapters. No second next() is issued while an
/// adopted ticket is pending, including repeated local-work extensions.
pub trait IdlePullSource: Send + Sync {
    /// Preserve the original source identity. This synchronous step runs at
    /// the generator's first pull, under the current issuing Run runtime.
    fn initialize(
        &self,
        factory: &Arc<GraphValueFactory>,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle>;
    /// The Host polls this future as part of its current native Run driver.
    /// No OS worker may call graph/runtime operations or pump native jobs.
    fn next(
        &self,
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
    ) -> BoxFuture<'static, Result<ValueHandle>>;
    fn close(
        &self,
        factory: &Arc<GraphValueFactory>,
        bindings: &dyn RetryBindings,
        effective: &dyn ValueRuntime,
    ) -> Result<()>;
    fn has_pending_local_work(&self) -> Result<bool> {
        Ok(false)
    }
    fn local_work_settled_at_in(
        &self,
        execution: GraphExecution<'_>,
        globals: &ValueHandle,
    ) -> Result<ValueHandle>;
}

fn local_work_settled_at(
    factory: &Arc<GraphValueFactory>, bindings: &dyn RetryBindings,
    options: &ValueHandle, source: Option<&dyn IdlePullSource>, runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let work = get(factory, options, "localWork", runtime)?;
    let value = if !nullish(factory, &work)? {
        get(factory, &work, "localWorkSettledAt", runtime)?
    } else if let Some(source) = source {
        source.local_work_settled_at_in(GraphExecution::new(factory.heap(), runtime), &bindings.globals())?
    } else { factory.heap().number(0.0)? };
    if nullish(factory, &value)? { factory.heap().number(0.0) } else { Ok(value) }
}

pub struct IdleIterator {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    iterable: ValueHandle,
    options: ValueHandle,
    iterator: Option<ValueHandle>,
    watch: Option<Arc<IdleWatch>>,
    pending: Option<Shared<BoxFuture<'static, Result<ValueHandle>>>>,
    listener: Option<AbortListener>,
    abort: Arc<(Mutex<bool>, Notify)>,
    signal: Option<ValueHandle>,
    closed: bool,
    no_timeout: bool,
    pull_source: Option<Arc<dyn IdlePullSource>>,
}
impl IdleIterator {
    pub fn prepare(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        iterable: ValueHandle,
        options: ValueHandle,
    ) -> Result<Self> {
        bindings.check_invocation(&factory)?;
        factory.heap().adopt(&iterable)?;
        factory.heap().adopt(&options)?;
        Ok(Self {
            factory,
            bindings,
            iterable,
            options,
            iterator: None,
            watch: None,
            pending: None,
            listener: None,
            abort: Arc::new((Mutex::new(false), Notify::new())),
            signal: None,
            closed: false,
            no_timeout: false,
            pull_source: None,
        })
    }
    pub fn prepare_source(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        original_identity: ValueHandle,
        source: Arc<dyn IdlePullSource>,
        options: ValueHandle,
    ) -> Result<Self> {
        let mut owner = Self::prepare(factory, bindings, original_identity, options)?;
        owner.pull_source = Some(source);
        Ok(owner)
    }
    fn initialize(&mut self) -> Result<()> {
        if self.iterator.is_some() || self.closed {
            return Ok(());
        }
        let (first_budget, first_deadline, signal, iterator, started, idle) =
            execute(&self.factory, self.bindings.as_ref(), |runtime| {
                let first =
                    number_option(&self.factory, &self.options, "firstItemTimeoutMs", runtime)?;
                let first = match first {
                    Some(first) => Some(first),
                    None => number_option(&self.factory, &self.options, "idleTimeoutMs", runtime)?,
                };
                let deadline = match first.filter(|value| *value > 0.0) {
                    Some(budget) => {
                        Some(now(&self.factory, self.bindings.as_ref(), runtime)? + budget)
                    }
                    None => None,
                };
                let signal = get(&self.factory, &self.options, "abortSignal", runtime)?;
                let signal = if nullish(&self.factory, &signal)? {
                    None
                } else {
                    Some(signal)
                };
                let heap = self.factory.heap();
                let iterator = match &self.pull_source {
                    Some(source) => source.initialize(&self.factory, runtime)?,
                    None => {
                        let iterator_fn =
                            heap.get(&self.iterable, &self.bindings.async_iterator_key(), runtime)?;
                        heap.call(&iterator_fn, &self.iterable, &[], runtime)?
                    }
                };
                // Source's early abort occurs before Date.now/timeout setup.
                let aborted = caller_aborted(&self.factory, signal.as_ref(), runtime)?;
                if aborted {
                    return Ok((first, deadline, signal, iterator, None, None));
                }
                let started = now(&self.factory, self.bindings.as_ref(), runtime)?;
                let idle = number_option(&self.factory, &self.options, "idleTimeoutMs", runtime)?;
                Ok((first, deadline, signal, iterator, Some(started), idle))
            })?;
        self.iterator = Some(iterator);
        self.signal = signal;
        let Some(started) = started else {
            self.close_iterator(false)?;
            return Err(self.abort_error()?);
        };
        self.no_timeout = first_budget.map_or(true, |value| value <= 0.0)
            && idle.map_or(true, |value| value <= 0.0);
        let watch = Arc::new(IdleWatch {
            factory: Arc::downgrade(&self.factory),
            bindings: Arc::downgrade(&self.bindings),
            options: self.options.clone(),
            local_work: self.pull_source.clone(),
            state: Mutex::new(WatchState {
                first: true,
                first_budget,
                first_deadline,
                last_progress: started,
                timer: None,
                timer_at: f64::INFINITY,
                generation: 0,
                fired: false,
                closed: false,
                callback_error: None,
            }),
            changed: Notify::new(),
        });
        self.watch = Some(watch);
        if let Some(signal) = &self.signal {
            let latch = self.abort.clone();
            let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
                *latch.0.lock().unwrap_or_else(|error| error.into_inner()) = true;
                latch.1.notify_waiters();
                Ok(())
            });
            self.listener = Some(AbortListener::attach(
                self.factory.clone(),
                self.bindings.clone(),
                signal.clone(),
                body,
            )?);
        }
        Ok(())
    }
    fn abort_error(&self) -> Result<ValueError> {
        let signal = self
            .signal
            .as_ref()
            .ok_or(ValueError::UnsupportedOperation(
                "Idle abort without signal",
            ))?;
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let reason = get(&self.factory, signal, "reason", runtime)?;
            if self.factory.heap().instance_of(
                &reason,
                &self.bindings.error_constructor(),
                runtime,
            )? {
                return Ok(ValueError::Thrown(reason));
            }
            let message = if self.factory.heap().kind(&reason)? == ValueKind::String {
                reason
            } else {
                self.factory.heap().string("Request was aborted".into())?
            };
            Ok(ValueError::Thrown(self.bindings.signals().error_value(
                &self.factory,
                "AbortError",
                message,
                runtime,
            )?))
        })
    }
    fn probe_local_work(&self) -> Result<bool> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let work = get(&self.factory, &self.options, "localWork", runtime)?;
            if !nullish(&self.factory, &work)? {
                let value = get(&self.factory, &work, "hasPendingLocalWork", runtime)?;
                return if nullish(&self.factory, &value)? { Ok(false) } else { self.factory.heap().truthy(&value) };
            }
            match &self.pull_source {
                Some(source) => source.has_pending_local_work(),
                None => Ok(false),
            }
        })
    }
    fn extend_local_work(&self) -> Result<()> {
        let watch = self.watch.as_ref().ok_or(ValueError::InvalidNode)?;
        let current = execute(&self.factory, self.bindings.as_ref(), |runtime| {
            now(&self.factory, self.bindings.as_ref(), runtime)
        })?;
        let mut state = watch
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.first {
            if state.first_deadline.is_some() {
                state.first_deadline = state.first_budget.map(|budget| current + budget);
            }
        } else {
            state.last_progress = current;
        }
        Ok(())
    }
    fn timeout_error(&mut self) -> Result<ValueError> {
        let first = self
            .watch
            .as_ref()
            .ok_or(ValueError::InvalidNode)?
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .first;
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let callback = get(
                &self.factory,
                &self.options,
                if first {
                    "onFirstItemTimeout"
                } else {
                    "onIdle"
                },
                runtime,
            )?;
            if !nullish(&self.factory, &callback)? {
                self.factory
                    .heap()
                    .call(&callback, &self.options, &[], runtime)?;
            }
            Ok(())
        })?;
        self.close_iterator(false)?;
        let message = execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let message = if first {
                get(
                    &self.factory,
                    &self.options,
                    "firstItemErrorMessage",
                    runtime,
                )?
            } else {
                self.factory.heap().undefined()?
            };
            if nullish(&self.factory, &message)? {
                get(&self.factory, &self.options, "errorMessage", runtime)
            } else {
                Ok(message)
            }
        })?;
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            Ok(ValueError::Thrown(self.bindings.signals().error_value(
                &self.factory,
                "StreamTimeoutError",
                message.clone(),
                runtime,
            )?))
        })
    }
    /// Yields one original item, or None for clean source completion. This is
    /// the generator's yield boundary; consumer return calls close explicitly.
    pub async fn next(&mut self) -> Result<Option<ValueHandle>> {
        if self.closed {
            return Ok(None);
        }
        let outcome = self.next_inner().await;
        if matches!(outcome, Ok(None) | Err(_)) {
            // Source finally can replace an earlier error with return()/clear
            // throws. Cleanup remains observable on the issuing runtime.
            self.close()?;
        }
        outcome
    }
    async fn next_inner(&mut self) -> Result<Option<ValueHandle>> {
        self.initialize()?;
        let watch = self.watch.as_ref().ok_or(ValueError::InvalidNode)?.clone();
        loop {
            let deadline = watch.deadline()?;
            let mut remaining = match deadline {
                Some(deadline) => {
                    Some(execute(&self.factory, self.bindings.as_ref(), |runtime| {
                        Ok(deadline - now(&self.factory, self.bindings.as_ref(), runtime)?)
                    })?)
                }
                None => None,
            };
            if remaining.is_some_and(|value| value <= 0.0) {
                if !self.probe_local_work()? {
                    return Err(self.timeout_error()?);
                }
                self.extend_local_work()?;
                remaining = match watch.deadline()? {
                    Some(deadline) => {
                        Some(execute(&self.factory, self.bindings.as_ref(), |runtime| {
                            Ok(deadline - now(&self.factory, self.bindings.as_ref(), runtime)?)
                        })?)
                    }
                    None => None,
                };
            }
            if self.pending.is_none() {
                if let Some(source) = &self.pull_source {
                    self.pending = Some(
                        source
                            .next(self.factory.clone(), self.bindings.clone())
                            .shared(),
                    );
                } else {
                    let promise = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                        method(
                            &self.factory,
                            self.iterator.as_ref().ok_or(ValueError::InvalidNode)?,
                            "next",
                            &[],
                            runtime,
                        )
                    })?;
                    self.pending = Some(
                        self.bindings
                            .await_value(self.factory.clone(), promise)
                            .shared(),
                    );
                }
            }
            let enforce = !self.no_timeout && remaining.is_some_and(|value| value > 0.0);
            if enforce {
                let deadline = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                    Ok(now(&self.factory, self.bindings.as_ref(), runtime)?
                        + remaining.unwrap_or(0.0))
                })?;
                watch.arm(deadline)?;
            }
            let pending = self
                .pending
                .as_ref()
                .ok_or(ValueError::InvalidNode)?
                .clone();
            let latch = self.abort.clone();
            let abort_wait = async move {
                loop {
                    let changed = latch.1.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if *latch.0.lock().unwrap_or_else(|error| error.into_inner()) {
                        return;
                    }
                    changed.await;
                }
            };
            enum Outcome {
                Next(Result<ValueHandle>),
                Timeout(Result<()>),
                Abort,
                InvocationEnded(ValueError),
            }
            let outcome = tokio::select! {
                biased;
                value = pending => Outcome::Next(value),
                value = watch.timeout(), if enforce => Outcome::Timeout(value),
                _ = abort_wait, if self.signal.is_some() => Outcome::Abort,
                error = self.bindings.invocation_ended() => Outcome::InvocationEnded(error),
            };
            match outcome {
                Outcome::InvocationEnded(error) => return Err(error),
                Outcome::Abort => {
                    self.close_iterator(false)?;
                    return Err(self.abort_error()?);
                }
                Outcome::Timeout(result) => {
                    result?;
                    if self.probe_local_work()? {
                        self.extend_local_work()?;
                        continue;
                    }
                    return Err(self.timeout_error()?);
                }
                Outcome::Next(result) => {
                    self.pending = None;
                    let result = result?;
                    let (done, item) = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                        let done = get(&self.factory, &result, "done", runtime)?;
                        if self.factory.heap().truthy(&done)? {
                            return Ok((true, None));
                        }
                        Ok((false, Some(get(&self.factory, &result, "value", runtime)?)))
                    })?;
                    if done {
                        watch
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .first = false;
                        return Ok(None);
                    }
                    let item = item.ok_or(ValueError::InvalidNode)?;
                    let progress = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                        let callback =
                            get(&self.factory, &self.options, "isProgressItem", runtime)?;
                        if !self.factory.heap().truthy(&callback)? {
                            return Ok(true);
                        }
                        match self
                            .factory
                            .heap()
                            .call(
                                &callback,
                                &self.options,
                                std::slice::from_ref(&item),
                                runtime,
                            )
                            .and_then(|value| self.factory.heap().truthy(&value))
                        {
                            Ok(value) => Ok(value),
                            Err(error) if error.is_native_exception() => Ok(true),
                            Err(error) => Err(error),
                        }
                    })?;
                    if progress {
                        let current = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                            now(&self.factory, self.bindings.as_ref(), runtime)
                        })?;
                        let mut state = watch
                            .state
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        state.first = false;
                        state.last_progress = current;
                    }
                    return Ok(Some(item));
                }
            }
        }
    }
    fn close_iterator(&mut self, resolve_return: bool) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let Some(iterator) = self.iterator.as_ref() else {
            return Ok(());
        };
        if let Some(source) = &self.pull_source {
            return execute(&self.factory, self.bindings.as_ref(), |runtime| {
                source.close(&self.factory, self.bindings.as_ref(), runtime)
            });
        }
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let callable = get(&self.factory, iterator, "return", runtime)?;
            if nullish(&self.factory, &callable)? {
                return Ok(());
            }
            let returned = self
                .factory
                .heap()
                .call(&callable, iterator, &[], runtime)?;
            if self.factory.heap().truthy(&returned)? {
                if resolve_return {
                    let work = self.bindings.await_value(self.factory.clone(), returned);
                    tokio::spawn(async move {
                        let _ = work.await;
                    });
                } else {
                    // Idle source directly reads .catch rather than adopting
                    // Promise.resolve, so malformed return values may throw.
                    let catch = get(&self.factory, &returned, "catch", runtime)?;
                    let swallowed: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(|| Ok(()));
                    let callback = self.bindings.signals().callback(
                        &self.factory,
                        swallowed.clone(),
                        runtime,
                    )?;
                    let result =
                        self.factory
                            .heap()
                            .call(&catch, &returned, &[callback], runtime)?;
                    let adopted = self.bindings.await_value(self.factory.clone(), result);
                    // Keep the rejection handler alive until its native
                    // settlement, without runtime work in the worker.
                    tokio::spawn(async move {
                        let _handler = swallowed;
                        let _ = adopted.await;
                    });
                }
            }
            Ok(())
        })
    }
    pub fn close(&mut self) -> Result<()> {
        let upstream = self.close_iterator(false);
        let timer = self.watch.as_ref().map(|watch| watch.clear()).transpose();
        let listener = match &mut self.listener {
            Some(listener) => listener.remove(),
            None => Ok(()),
        };
        *self
            .abort
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = true;
        self.abort.1.notify_waiters();
        upstream?;
        timer?;
        listener
    }
}
impl Drop for IdleIterator {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            self.bindings.receipt().record(error);
        }
    }
}

/// Terminal grace is an independent owner: its fixed deadline cannot be
/// extended by trailing chunks, and expiry is a clean end.
/// Original-Run drain resource, never a detached Graph worker.
pub struct TerminalReturn {
    pub await_drain: bool,
    pub drain: BoxFuture<'static, Result<()>>,
}

pub struct TerminalGraceIterator {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    iterable: ValueHandle,
    options: ValueHandle,
    iterator: Option<ValueHandle>,
    closed: bool,
    suspended_at_yield: bool,
    pending: Option<Shared<BoxFuture<'static, Result<ValueHandle>>>>,
}
impl TerminalGraceIterator {
    pub fn prepare(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        iterable: ValueHandle,
        options: ValueHandle,
    ) -> Result<Self> {
        bindings.check_invocation(&factory)?;
        factory.heap().adopt(&iterable)?;
        factory.heap().adopt(&options)?;
        Ok(Self {
            factory,
            bindings,
            iterable,
            options,
            iterator: None,
            closed: false,
            suspended_at_yield: false,
            pending: None,
        })
    }
    pub async fn next(&mut self) -> Result<Option<ValueHandle>> {
        if self.closed {
            return Ok(None);
        }
        self.suspended_at_yield = false;
        let result = self.next_inner().await;
        self.suspended_at_yield = matches!(result, Ok(Some(_)));
        if matches!(result, Ok(None) | Err(_)) {
            self.close()?;
        }
        result
    }
    async fn next_inner(&mut self) -> Result<Option<ValueHandle>> {
        if self.iterator.is_none() {
            self.iterator = Some(execute(&self.factory, self.bindings.as_ref(), |runtime| {
                let callable = self.factory.heap().get(
                    &self.iterable,
                    &self.bindings.async_iterator_key(),
                    runtime,
                )?;
                self.factory
                    .heap()
                    .call(&callable, &self.iterable, &[], runtime)
            })?);
        }
        let remaining = execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let finished = method(&self.factory, &self.options, "finishedAtMs", &[], runtime)?;
            if self.factory.heap().kind(&finished)? == ValueKind::Undefined {
                return Ok(None);
            }
            let grace = get(&self.factory, &self.options, "graceMs", runtime)?;
            let sum = self.factory.heap().add(&finished, &grace, runtime)?;
            let current = now_raw(&self.factory, self.bindings.as_ref(), runtime)?;
            Ok(Some(self.factory.heap().to_number(&sum, runtime)?
                - self.factory.heap().to_number(&current, runtime)?))
        })?;
        if remaining.is_some_and(|value| value <= 0.0) {
            self.grace_end()?;
            return Ok(None);
        }
        if self.pending.is_none() {
            let promise = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                method(&self.factory, self.iterator.as_ref().ok_or(ValueError::InvalidNode)?, "next", &[], runtime)
            })?;
            self.pending = Some(self.bindings.await_value(self.factory.clone(), promise).shared());
        }
        let pending = self.pending.as_ref().ok_or(ValueError::InvalidNode)?.clone();
        let result = if let Some(remaining) = remaining {
            let state = Arc::new((Mutex::new(false), Notify::new()));
            let fired = state.clone();
            let timer = self.bindings.timers().arm(
                remaining,
                Arc::new(move || {
                    *fired.0.lock().unwrap_or_else(|error| error.into_inner()) = true;
                    fired.1.notify_waiters();
                    Ok(())
                }),
            )?;
            let timeout = async {
                loop {
                    let changed = state.1.notified();
                    tokio::pin!(changed);
                    changed.as_mut().enable();
                    if *state.0.lock().unwrap_or_else(|error| error.into_inner()) {
                        return;
                    }
                    changed.await;
                }
            };
            let outcome = tokio::select! { biased; result = pending => Some(result), _ = timeout => None, error = self.bindings.invocation_ended() => Some(Err(error)) };
            timer.clear()?;
            match outcome {
                Some(result) => result?,
                None => {
                    self.grace_end()?;
                    return Ok(None);
                }
            }
        } else {
            pending.await?
        };
        self.pending = None;
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let done = get(&self.factory, &result, "done", runtime)?;
            if self.factory.heap().truthy(&done)? {
                Ok(None)
            } else {
                Ok(Some(get(&self.factory, &result, "value", runtime)?))
            }
        })
    }
    /// Evaluate generator-finally guards synchronously on its original Run.
    /// A Host retaining the background branch must register this exact drain
    /// as an already-admitted resource before returning logical completion.
    pub fn prepare_consumer_return(mut self) -> Result<TerminalReturn> {
        let drain = if self.suspended_at_yield && !self.closed {
            let guard = execute(&self.factory, self.bindings.as_ref(), |runtime| {
                let finished = method(&self.factory, &self.options, "finishedAtMs", &[], runtime)?;
                if self.factory.heap().kind(&finished)? == ValueKind::Undefined { return Ok(None); }
                let wait = get(&self.factory, &self.options, "awaitDrainOnReturn", runtime)?;
                Ok(Some(self.factory.heap().truthy(&wait)?))
            });
            match guard {
                Ok(guard) => guard,
                Err(error) => {
                    // A throw in the source finally condition never enters
                    // releaseIterator. Do not let Rust Drop invent that Call.
                    self.closed = true;
                    return Err(error);
                }
            }
        } else { None };
        self.suspended_at_yield = false;
        match drain {
            Some(await_drain) => Ok(TerminalReturn {
                await_drain,
                drain: Box::pin(async move {
                    // The accepted consumer result is fixed; tail read/grace
                    // failures cannot replace it. Release remains once-only.
                    while matches!(self.next_inner().await, Ok(Some(_))) {}
                    self.close()
                }),
            }),
            None => {
                self.close()?;
                Ok(TerminalReturn { await_drain: false, drain: Box::pin(async { Ok(()) }) })
            }
        }
    }
    /// Explicit awaited cleanup. Default-background callers instead transfer
    /// prepare_consumer_return().drain to their existing original Run owner.
    pub fn finish_consumer_return(self) -> BoxFuture<'static, Result<()>> {
        let prepared = self.prepare_consumer_return();
        Box::pin(async move { prepared?.drain.await })
    }
    fn grace_end(&self) -> Result<()> {
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let callback = get(&self.factory, &self.options, "onGraceEnd", runtime)?;
            if !nullish(&self.factory, &callback)? {
                self.factory
                    .heap()
                    .call(&callback, &self.options, &[], runtime)?;
            }
            Ok(())
        })
    }
    pub fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.pending = None;
        let Some(iterator) = &self.iterator else {
            return Ok(());
        };
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let callable = get(&self.factory, iterator, "return", runtime)?;
            if nullish(&self.factory, &callable)? {
                return Ok(());
            }
            let returned = self
                .factory
                .heap()
                .call(&callable, iterator, &[], runtime)?;
            if self.factory.heap().truthy(&returned)? {
                let adopted = self.bindings.await_value(self.factory.clone(), returned);
                tokio::spawn(async move {
                    let _ = adopted.await;
                });
            }
            Ok(())
        })
    }
}
impl Drop for TerminalGraceIterator {
    fn drop(&mut self) {
        if let Err(error) = self.close() {
            self.bindings.receipt().record(error);
        }
    }
}
