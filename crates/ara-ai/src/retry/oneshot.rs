//! Fixed OMP b91db66e oneshot-retry.ts, shared native/Rust-attempt engine (MIT).
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
//! Original response/error handles stay in the issuing heap. The Host drives
//! this future; no retry task, new realm, or private Promise pump is created.
use super::{RetryBindings, execute, hints::RetryAfterModule};
use crate::{
    graph_values::{GraphExecution, GraphValueFactory},
    schema_validator::graph_values_compare_with_execution,
};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime};
use futures::future::BoxFuture;
use std::sync::{Arc, Weak};
type Result<T> = std::result::Result<T, ValueError>;

pub trait CompletionAttempt: Send + Sync {
    /// Original 1-based source argument. A Rust provider adapter consumes this
    /// same port and produces real roots; it cannot implement a retry policy.
    fn run(&self, attempt: ValueHandle) -> BoxFuture<'static, Result<ValueHandle>>;
}
pub struct NativeCompletionAttempt {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    run: ValueHandle,
}
impl NativeCompletionAttempt {
    pub fn prepare(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        run: ValueHandle,
    ) -> Result<Self> {
        bindings.check_invocation(&factory)?;
        factory.heap().adopt(&run)?;
        Ok(Self {
            factory,
            bindings,
            run,
        })
    }
}
impl CompletionAttempt for NativeCompletionAttempt {
    fn run(&self, attempt: ValueHandle) -> BoxFuture<'static, Result<ValueHandle>> {
        let called = execute(&self.factory, self.bindings.as_ref(), |runtime| {
            self.factory.heap().call(
                &self.run,
                &self.factory.heap().undefined()?,
                &[attempt],
                runtime,
            )
        });
        match called {
            Ok(value) => self.bindings.await_value(self.factory.clone(), value),
            Err(error) => Box::pin(async move { Err(error) }),
        }
    }
}
pub struct OneshotModule {
    factory: Weak<GraphValueFactory>,
    /// Original evaluated AIError module namespace, including callable
    /// classify/classifyMessage/status/is/retriable and Flag/pattern imports.
    pub ai_error: ValueHandle,
    pub retry_after: Arc<RetryAfterModule>,
    pub extract_retry_hint: ValueHandle,
    pub extract_provider_retry_hint: ValueHandle,
}
impl OneshotModule {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        ai_error: ValueHandle,
        retry_after: Arc<RetryAfterModule>,
        extract_retry_hint: ValueHandle,
    ) -> Result<Self> {
        for value in [&ai_error, &extract_retry_hint] {
            factory.heap().adopt(value)?;
        }
        retry_after.check_factory(factory)?;
        factory.heap().module_regexp(
            "ara-ai.omp596f2da.oneshot",
            "RETRY_AFTER_MS_SUFFIX",
            r"(?:^|\s)retry-after-ms=([0-9]+(?:\.[0-9]+)?)(?=\s|$)",
            "i",
        )?;
        let extract_provider_retry_hint = super::native_retry_hint::prepare_provider_extractor(factory, extract_retry_hint.clone())?;
        Ok(Self {
            factory: Arc::downgrade(factory),
            ai_error,
            retry_after,
            extract_retry_hint,
            extract_provider_retry_hint,
        })
    }
    fn check(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation("Oneshot module expired"))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::ForeignHeap);
        }
        self.retry_after.check_factory(factory)
    }
}
#[derive(Clone, Copy)]
struct Ex<'a> {
    heap: &'a Arc<ValueHeap>,
    runtime: &'a dyn ValueRuntime,
    globals: &'a ValueHandle,
    module: &'a OneshotModule,
}
impl Ex<'_> {
    fn get(self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        self.heap.get(value, &key.into(), self.runtime)
    }
    fn optional(self, value: &ValueHandle, key: &str) -> Result<ValueHandle> {
        if self.nullish(value)? {
            self.heap.undefined()
        } else {
            self.get(value, key)
        }
    }
    fn nullish(self, value: &ValueHandle) -> Result<bool> {
        Ok(matches!(
            self.heap.kind(value)?,
            ValueKind::Undefined | ValueKind::Null
        ))
    }
    fn undefined(self, value: &ValueHandle) -> Result<bool> {
        Ok(self.heap.kind(value)? == ValueKind::Undefined)
    }
    fn coalesce(self, value: ValueHandle, fallback: f64) -> Result<ValueHandle> {
        if self.nullish(&value)? {
            self.heap.number(fallback)
        } else {
            Ok(value)
        }
    }
    fn method(
        self,
        receiver: &ValueHandle,
        key: &str,
        args: &[ValueHandle],
    ) -> Result<ValueHandle> {
        self.heap
            .call(&self.get(receiver, key)?, receiver, args, self.runtime)
    }
    fn math(self, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.method(&self.get(self.globals, "Math")?, key, args)
    }
    fn math_expression(
        self,
        key: &str,
        args: impl FnOnce() -> Result<Vec<ValueHandle>>,
    ) -> Result<ValueHandle> {
        let receiver = self.get(self.globals, "Math")?;
        let method = self.get(&receiver, key)?;
        self.heap.call(&method, &receiver, &args()?, self.runtime)
    }
    fn ai(self, key: &str, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.method(&self.module.ai_error, key, args)
    }
    fn flag(self, key: &str) -> Result<ValueHandle> {
        self.get(&self.get(&self.module.ai_error, "Flag")?, key)
    }
    fn is(self, id: &ValueHandle, key: &str) -> Result<bool> {
        let method = self.get(&self.module.ai_error, "is")?;
        let flag = self.flag(key)?;
        self.heap.truthy(&self.heap.call(
            &method,
            &self.module.ai_error,
            &[id.clone(), flag],
            self.runtime,
        )?)
    }
    fn equal_text(self, value: &ValueHandle, text: &str) -> Result<bool> {
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
    fn finite(self, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(&self.method(
            &self.get(self.globals, "Number")?,
            "isFinite",
            std::slice::from_ref(value),
        )?)
    }
    fn retryable(self, id: &ValueHandle, status: &ValueHandle, text: &ValueHandle) -> Result<bool> {
        let pattern = self.get(&self.module.ai_error, "LLAMA_CPP_TOOL_CALL_PARSE_PATTERN")?;
        if self
            .heap
            .truthy(&self.method(&pattern, "test", std::slice::from_ref(text))?)?
        {
            return Ok(false);
        }
        for flag in ["ContentBlocked", "ContextOverflow", "PayloadRejected"] {
            if self.is(id, flag)? {
                return Ok(false);
            }
        }
        if self
            .heap
            .truthy(&self.ai("isTransientStatus", std::slice::from_ref(status))?)?
        {
            return Ok(true);
        }
        if self.is(id, "Transient")? || self.is(id, "UsageLimit")? {
            return Ok(true);
        }
        self.heap
            .truthy(&self.ai("retriable", std::slice::from_ref(id))?)
    }
    fn backoff(self, attempt: f64, base: &ValueHandle) -> Result<ValueHandle> {
        // 2 ** (attempt-1) is a native numeric expression, not integer-normalized.
        let growth = self.math_expression("min", || {
            Ok(vec![
                self.heap
                    .number(self.heap.to_number(base, self.runtime)? * 2_f64.powf(attempt - 1.0))?,
                self.heap.number(8000.0)?,
            ])
        })?;
        self.math_expression("round", || {
            let random = self.math("random", &[])?;
            let factor = 0.75 + self.heap.to_number(&random, self.runtime)? * 0.25;
            Ok(vec![self.heap.number(
                self.heap.to_number(&growth, self.runtime)? * factor,
            )?])
        })
    }
    fn hints(
        self,
        thrown: &ValueHandle,
        text: &ValueHandle,
        options: &ValueHandle,
    ) -> Result<ValueHandle> {
        let headers = if !self.undefined(thrown)? {
            self.heap.call(
                &self.module.retry_after.get_headers_from_error,
                &self.heap.undefined()?,
                std::slice::from_ref(thrown),
                self.runtime,
            )?
        } else {
            let getter = self.optional(options, "getResponseHeaders")?;
            if self.nullish(&getter)? {
                self.heap.undefined()?
            } else {
                self.heap.call(&getter, options, &[], self.runtime)?
            }
        };
        let header = self.heap.call(
            &self.module.retry_after.get_retry_after_ms_from_headers,
            &self.heap.undefined()?,
            &[headers],
            self.runtime,
        )?;
        let extracted = self.heap.call(
            &self.module.extract_provider_retry_hint,
            &self.heap.undefined()?,
            &[self.optional(options, "provider")?, text.clone()],
            self.runtime,
        )?;
        let suffix_re = self.heap.module_regexp(
            "ara-ai.omp596f2da.oneshot",
            "RETRY_AFTER_MS_SUFFIX",
            r"(?:^|\s)retry-after-ms=([0-9]+(?:\.[0-9]+)?)(?=\s|$)",
            "i",
        )?;
        let matched = self.method(&suffix_re, "exec", std::slice::from_ref(text))?;
        let suffix = self.optional(&matched, "1")?;
        let parsed = if self.undefined(&suffix)? {
            self.heap.undefined()?
        } else {
            self.heap.call(
                &self.get(self.globals, "Number")?,
                &self.heap.undefined()?,
                &[suffix],
                self.runtime,
            )?
        };
        let suffix = if !self.undefined(&parsed)?
            && self.finite(&parsed)?
            && self.compare(&parsed, &self.heap.number(0.0)?, ">")?
        {
            self.math("ceil", &[parsed])?
        } else {
            self.heap.undefined()?
        };
        let text = if self.undefined(&extracted)? && self.undefined(&suffix)? {
            self.heap.undefined()?
        } else {
            self.math(
                "max",
                &[self.coalesce(extracted, 0.0)?, self.coalesce(suffix, 0.0)?],
            )?
        };
        if self.undefined(&header)? && self.undefined(&text)? {
            self.heap.undefined()
        } else {
            self.math(
                "max",
                &[self.coalesce(header, 0.0)?, self.coalesce(text, 0.0)?],
            )
        }
    }
}

pub async fn retry_transient_completion(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    module: Arc<OneshotModule>,
    run: Arc<dyn CompletionAttempt>,
    options: Option<ValueHandle>,
) -> Result<ValueHandle> {
    module.check(&factory)?;
    bindings.check_invocation(&factory)?;
    let globals = bindings.globals();
    let options = options.unwrap_or(factory.heap().undefined()?);
    let (max_attempts, base, max_delay, signal) =
        execute(&factory, bindings.as_ref(), |runtime| {
            let ex = Ex {
                heap: factory.heap(),
                runtime,
                globals: &globals,
                module: &module,
            };
            Ok((
                ex.math_expression("max", || {
                    Ok(vec![
                        ex.heap.number(1.0)?,
                        ex.coalesce(ex.optional(&options, "maxAttempts")?, 3.0)?,
                    ])
                })?,
                ex.coalesce(ex.optional(&options, "baseDelayMs")?, 500.0)?,
                ex.coalesce(ex.optional(&options, "maxDelayMs")?, 30000.0)?,
                ex.optional(&options, "signal")?,
            ))
        })?;
    let mut attempt = 1.0;
    loop {
        let mut message = factory.heap().undefined()?;
        let mut thrown = factory.heap().undefined()?;
        match run.run(factory.heap().number(attempt)?).await {
            Ok(value) => {
                message = value;
                let observed = execute(&factory, bindings.as_ref(), |runtime| {
                    let ex = Ex {
                        heap: factory.heap(),
                        runtime,
                        globals: &globals,
                        module: &module,
                    };
                    Ok(!ex.equal_text(&ex.get(&message, "stopReason")?, "error")?)
                });
                match observed {
                    Ok(true) => return Ok(message),
                    Ok(false) => {}
                    Err(ValueError::Thrown(value)) => thrown = value,
                    Err(error) => return Err(error),
                }
            }
            Err(ValueError::Thrown(value)) => thrown = value,
            Err(error) => return Err(error),
        }
        enum Next {
            Return,
            Wait(ValueHandle),
        }
        let decision = execute(&factory, bindings.as_ref(), |runtime| {
            let ex = Ex {
                heap: factory.heap(),
                runtime,
                globals: &globals,
                module: &module,
            };
            if ex.heap.truthy(&ex.optional(&signal, "aborted")?)? {
                return Ok(Next::Return);
            }
            let is_thrown = !ex.undefined(&thrown)?;
            let id = if is_thrown {
                ex.ai("classify", std::slice::from_ref(&thrown))?
            } else {
                ex.ai("classifyMessage", std::slice::from_ref(&message))?
            };
            if ex.is(&id, "Abort")? || ex.is(&id, "UserInterrupt")? {
                return Ok(Next::Return);
            }
            let text = if is_thrown {
                if ex
                    .heap
                    .instance_of(&thrown, &ex.get(&globals, "Error")?, runtime)?
                {
                    ex.get(&thrown, "message")?
                } else {
                    ex.heap.call(
                        &ex.get(&globals, "String")?,
                        &ex.heap.undefined()?,
                        std::slice::from_ref(&thrown),
                        runtime,
                    )?
                }
            } else {
                let text = ex.get(&message, "errorMessage")?;
                if ex.nullish(&text)? {
                    ex.heap.string("unknown error".into())?
                } else {
                    text
                }
            };
            let status = if is_thrown {
                ex.ai("status", std::slice::from_ref(&thrown))?
            } else {
                ex.get(&message, "errorStatus")?
            };
            if ex.compare(&ex.heap.number(attempt)?, &max_attempts, ">=")?
                || !ex.retryable(&id, &status, &text)?
            {
                return Ok(Next::Return);
            }
            let hint = ex.hints(&thrown, &text, &options)?;
            if !ex.undefined(&hint)? && ex.compare(&hint, &max_delay, ">")? {
                return Ok(Next::Return);
            }
            let backoff = ex.backoff(attempt, &base)?;
            let delay = ex.math_expression("min", || {
                Ok(vec![
                    ex.math("max", &[ex.coalesce(hint.clone(), 0.0)?, backoff.clone()])?,
                    max_delay.clone(),
                ])
            })?;
            let hook = ex.optional(&options, "onRetry")?;
            if !ex.nullish(&hook)? {
                let info = ex.heap.object()?;
                for (key, value) in [
                    ("attempt", ex.heap.number(attempt)?),
                    ("maxAttempts", max_attempts.clone()),
                    ("delayMs", delay.clone()),
                    (
                        "fromRetryHint",
                        ex.heap
                            .boolean(!ex.undefined(&hint)? && ex.compare(&hint, &backoff, ">=")?)?,
                    ),
                    ("errorMessage", text),
                    ("errorId", id),
                ] {
                    ex.heap.define_own_data(&info, key.into(), value, runtime)?;
                }
                ex.heap.call(&hook, &options, &[info], runtime)?;
            }
            Ok(Next::Wait(delay))
        })?;
        match decision {
            Next::Return => {
                return if factory.heap().kind(&thrown)? != ValueKind::Undefined {
                    Err(ValueError::Thrown(thrown))
                } else {
                    Ok(message)
                };
            }
            Next::Wait(delay) => sleep(&factory, bindings.clone(), &delay, &signal).await?,
        }
        attempt += 1.0;
    }
}

pub(crate) async fn sleep(
    factory: &Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    delay: &ValueHandle,
    signal: &ValueHandle,
) -> Result<()> {
    use super::TimerLease;
    use std::sync::Mutex;
    use tokio::sync::Notify;
    let nonpositive = execute(factory, bindings.as_ref(), |runtime| {
        graph_values_compare_with_execution(
            GraphExecution::new(factory.heap(), runtime),
            delay,
            &factory.heap().number(0.0)?,
            "<=",
        )
    })?;
    if nonpositive {
        return Ok(());
    }
    let state = Arc::new((Mutex::new(None::<Result<()>>), Notify::new()));
    let listener = Arc::new(Mutex::new(None::<ValueHandle>));
    let timer_slot = Arc::new(Mutex::new(None::<TimerLease>));
    let weak_factory = Arc::downgrade(factory);
    let weak_bindings = Arc::downgrade(&bindings);
    let fired = state.clone();
    let fired_listener = listener.clone();
    let fired_signal = signal.clone();
    let fulfilled: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
        let factory = weak_factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Oneshot sleep Session expired",
            ))?;
        let bindings = weak_bindings
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Oneshot sleep imports expired",
            ))?;
        execute(&factory, bindings.as_ref(), |runtime| {
            let heap = factory.heap();
            if !matches!(
                heap.kind(&fired_signal)?,
                ValueKind::Null | ValueKind::Undefined
            ) {
                let method = heap.get(&fired_signal, &"removeEventListener".into(), runtime)?;
                let callback = fired_listener
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .clone()
                    .ok_or(ValueError::InvalidNode)?;
                heap.call(
                    &method,
                    &fired_signal,
                    &[heap.string("abort".into())?, callback],
                    runtime,
                )?;
            }
            Ok(())
        })?;
        let mut result = fired.0.lock().map_err(|_| ValueError::PoisonedHeap)?;
        if result.is_none() {
            *result = Some(Ok(()));
            fired.1.notify_waiters();
        }
        Ok(())
    });
    // setTimeout performs its own ToNumber after the source relational test.
    // Preserve both observable coercions and their order for native objects.
    let timer_delay = execute(factory, bindings.as_ref(), |runtime| {
        factory.heap().to_number(delay, runtime)
    })?;
    let timer = bindings.timers().arm(timer_delay, fulfilled.clone())?;
    *timer_slot.lock().map_err(|_| ValueError::PoisonedHeap)? = Some(timer.clone());
    let rejected = state.clone();
    let cancelled_timer = timer_slot.clone();
    let abort_signal = signal.clone();
    let weak_factory = Arc::downgrade(factory);
    let weak_bindings = Arc::downgrade(&bindings);
    let on_abort: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
        cancelled_timer
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .as_ref()
            .ok_or(ValueError::InvalidNode)?
            .clear()?;
        let factory = weak_factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Oneshot sleep Session expired",
            ))?;
        let bindings = weak_bindings
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Oneshot sleep imports expired",
            ))?;
        let reason = execute(&factory, bindings.as_ref(), |runtime| {
            abort_reason(&factory, bindings.as_ref(), &abort_signal, runtime)
        })?;
        let mut result = rejected.0.lock().map_err(|_| ValueError::PoisonedHeap)?;
        if result.is_none() {
            *result = Some(Err(ValueError::Thrown(reason)));
            rejected.1.notify_waiters();
        }
        Ok(())
    });
    let callback = execute(factory, bindings.as_ref(), |runtime| {
        bindings
            .signals()
            .callback(factory, on_abort.clone(), runtime)
    })?;
    *listener.lock().map_err(|_| ValueError::PoisonedHeap)? = Some(callback.clone());
    let immediate = execute(factory, bindings.as_ref(), |runtime| {
        let heap = factory.heap();
        if heap.truthy(signal)? {
            if heap.truthy(&heap.get(signal, &"aborted".into(), runtime)?)? {
                timer.clear()?;
                return Ok(Some(abort_reason(
                    factory,
                    bindings.as_ref(),
                    signal,
                    runtime,
                )?));
            }
            let options = heap.object()?;
            heap.define_own_data(&options, "once".into(), heap.boolean(true)?, runtime)?;
            let method = heap.get(signal, &"addEventListener".into(), runtime)?;
            heap.call(
                &method,
                signal,
                &[heap.string("abort".into())?, callback.clone(), options],
                runtime,
            )?;
        }
        Ok(None)
    })?;
    if let Some(reason) = immediate {
        return Err(ValueError::Thrown(reason));
    }
    // Keep only the original callback owners while waiting. Timer/DOM callback
    // dispatch is performed by the same original driver under registration.
    let _owners = (fulfilled, on_abort, timer);
    loop {
        let changed = state.1.notified();
        tokio::pin!(changed);
        changed.as_mut().enable();
        if let Some(result) = state.0.lock().map_err(|_| ValueError::PoisonedHeap)?.take() {
            return result;
        }
        bindings.check_invocation(factory)?;
        tokio::select! {ended=bindings.invocation_ended()=>return Err(ended),_=&mut changed=>{}}
    }
}
fn abort_reason(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    signal: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let heap = factory.heap();
    let reason = if matches!(heap.kind(signal)?, ValueKind::Null | ValueKind::Undefined) {
        heap.undefined()?
    } else {
        heap.get(signal, &"reason".into(), runtime)?
    };
    if matches!(heap.kind(&reason)?, ValueKind::Null | ValueKind::Undefined) {
        bindings
            .signals()
            .error(factory, "AbortError", "oneshot retry aborted", runtime)
    } else {
        Ok(reason)
    }
}
