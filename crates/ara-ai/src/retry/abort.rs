//! Fixed OMP596f2da utils/abort.ts Rust semantics (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use super::{AbortControllerRef, RetryBindings, execute};
use crate::graph_values::GraphValueFactory;
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueKind, ValueRuntime, ValueSnapshot};
use futures::{FutureExt, future::BoxFuture};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::Notify;

type Result<T> = std::result::Result<T, ValueError>;

pub struct AbortSourceTracker {
    factory: Weak<GraphValueFactory>,
    bindings: Weak<dyn RetryBindings>,
    caller_signal: Option<ValueHandle>,
    pub request_abort_controller: AbortControllerRef,
    pub request_signal: ValueHandle,
    local_reason: Mutex<Option<ValueHandle>>,
}

pub fn create_abort_source_tracker(
    factory: &Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    caller_signal: Option<ValueHandle>,
    effective: &dyn ValueRuntime,
) -> Result<AbortSourceTracker> {
    bindings.check_invocation(factory)?;
    let signals = bindings.signals();
    let controller = signals.controller(factory, effective)?;
    let request_signal = match caller_signal.as_ref() {
        Some(caller) if factory.heap().truthy(caller)? => signals.any(
            factory,
            &[caller.clone(), controller.signal.clone()],
            effective,
        )?,
        _ => controller.signal.clone(),
    };
    Ok(AbortSourceTracker {
        factory: Arc::downgrade(factory),
        bindings: Arc::downgrade(&bindings),
        caller_signal,
        request_abort_controller: controller,
        request_signal,
        local_reason: Mutex::new(None),
    })
}

impl AbortSourceTracker {
    fn imports(&self, factory: &Arc<GraphValueFactory>) -> Result<Arc<dyn RetryBindings>> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Abort tracker owner expired",
            ))?;
        if !Arc::ptr_eq(&original, factory) {
            return Err(ValueError::UnsupportedOperation(
                "Abort tracker requires original Session factory",
            ));
        }
        let imports = self
            .bindings
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation(
                "Abort tracker imports expired",
            ))?;
        imports.check_invocation(factory)?;
        Ok(imports)
    }
    pub fn abort_locally(
        &self,
        factory: &Arc<GraphValueFactory>,
        reason: ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let imports = self.imports(factory)?;
        factory.heap().adopt(&reason)?;
        // Source writes localAbortReason before invoking the controller, and
        // preserves it even if that original abort Call throws.
        *self
            .local_reason
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(reason.clone());
        imports.signals().abort(
            factory,
            &self.request_abort_controller,
            Some(reason.clone()),
            effective,
        )?;
        Ok(reason)
    }
    pub fn get_local_abort_reason(
        &self,
        factory: &Arc<GraphValueFactory>,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.imports(factory)?;
        let reason = self
            .local_reason
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let Some(reason) = reason else {
            return factory.heap().undefined();
        };
        if !factory.heap().truthy(&reason)? {
            return factory.heap().undefined();
        }
        if caller_aborted(factory, self.caller_signal.as_ref(), effective)? {
            return factory.heap().undefined();
        }
        let actual = factory
            .heap()
            .get(&self.request_signal, &"reason".into(), effective)?;
        if factory.heap().object_is(&actual, &reason)? {
            Ok(reason)
        } else {
            factory.heap().undefined()
        }
    }
    pub fn was_caller_abort(
        &self,
        factory: &Arc<GraphValueFactory>,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        self.imports(factory)?;
        let Some(caller) = &self.caller_signal else {
            return Ok(false);
        };
        if matches!(
            factory.heap().kind(caller)?,
            ValueKind::Null | ValueKind::Undefined
        ) {
            return Ok(false);
        }
        let aborted = factory.heap().get(caller, &"aborted".into(), effective)?;
        Ok(matches!(
            factory.heap().snapshot(&aborted)?,
            ValueSnapshot::Boolean(true)
        ))
    }
}

pub(crate) fn caller_aborted(
    factory: &Arc<GraphValueFactory>,
    caller: Option<&ValueHandle>,
    effective: &dyn ValueRuntime,
) -> Result<bool> {
    let Some(caller) = caller else {
        return Ok(false);
    };
    if matches!(
        factory.heap().kind(caller)?,
        ValueKind::Null | ValueKind::Undefined
    ) {
        return Ok(false);
    }
    factory
        .heap()
        .truthy(&factory.heap().get(caller, &"aborted".into(), effective)?)
}

/// Source listener lifetime. The callback is retained by its caller, while the
/// Host's reverse callback registry holds only a Weak locator.
pub(crate) struct AbortListener {
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    signal: ValueHandle,
    callback: ValueHandle,
    _body: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    attached: bool,
}

impl AbortListener {
    pub(crate) fn attach(
        factory: Arc<GraphValueFactory>,
        bindings: Arc<dyn RetryBindings>,
        signal: ValueHandle,
        body: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    ) -> Result<Self> {
        let callback = execute(&factory, bindings.as_ref(), |runtime| {
            let heap = factory.heap();
            let callback = bindings
                .signals()
                .callback(&factory, body.clone(), runtime)?;
            let options = heap.object()?;
            heap.define_own_data(&options, "once".into(), heap.boolean(true)?, runtime)?;
            let add = heap.get(&signal, &"addEventListener".into(), runtime)?;
            heap.call(
                &add,
                &signal,
                &[heap.string("abort".into())?, callback.clone(), options],
                runtime,
            )?;
            Ok(callback)
        })?;
        Ok(Self {
            factory,
            bindings,
            signal,
            callback,
            _body: body,
            attached: true,
        })
    }
    pub(crate) fn remove(&mut self) -> Result<()> {
        if !self.attached {
            return Ok(());
        }
        // Source invokes the original remove method in finally, even after the
        // once-listener fired. Retain its exception as the final result.
        self.attached = false;
        execute(&self.factory, self.bindings.as_ref(), |runtime| {
            let heap = self.factory.heap();
            let remove = heap.get(&self.signal, &"removeEventListener".into(), runtime)?;
            heap.call(
                &remove,
                &self.signal,
                &[heap.string("abort".into())?, self.callback.clone()],
                runtime,
            )?;
            Ok(())
        })
    }
}
impl Drop for AbortListener {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            self.bindings.receipt().record(error);
        }
    }
}

pub(crate) fn nullish(factory: &Arc<GraphValueFactory>, value: &ValueHandle) -> Result<bool> {
    Ok(matches!(
        factory.heap().kind(value)?,
        ValueKind::Undefined | ValueKind::Null
    ))
}

pub(crate) fn race_abort_reason(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    signal: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let reason = factory.heap().get(signal, &"reason".into(), runtime)?;
    if nullish(factory, &reason)? {
        bindings
            .signals()
            .error(factory, "AbortError", "Request was aborted", runtime)
    } else {
        Ok(reason)
    }
}

/// Adopts only once. A losing caller never cancels the underlying native
/// Promise, and the ticket retains its original fulfillment or rejection.
pub fn race_with_signal(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    promise: ValueHandle,
    signal: Option<ValueHandle>,
) -> BoxFuture<'static, Result<ValueHandle>> {
    let prepared = (|| {
        bindings.check_invocation(&factory)?;
        let signal = match signal {
            Some(signal) if factory.heap().truthy(&signal)? => Some(signal),
            _ => None,
        };
        if let Some(signal) = &signal {
            let early = execute(&factory, bindings.as_ref(), |runtime| {
                if caller_aborted(&factory, Some(signal), runtime)? {
                    Ok(Some(race_abort_reason(
                        &factory,
                        bindings.as_ref(),
                        signal,
                        runtime,
                    )?))
                } else {
                    Ok(None)
                }
            })?;
            if let Some(reason) = early {
                return Err(ValueError::Thrown(reason));
            }
        }
        let original = bindings.await_value(factory.clone(), promise);
        let Some(signal) = signal else {
            return Ok((original, None));
        };
        let rejected = Arc::new((Mutex::new(None::<Result<ValueHandle>>), Notify::new()));
        let captured = rejected.clone();
        let owner = Arc::downgrade(&factory);
        let imports = Arc::downgrade(&bindings);
        let original_signal = signal.clone();
        let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
            let owner = owner.upgrade().ok_or(ValueError::UnsupportedOperation(
                "Signal race Session expired",
            ))?;
            let imports = imports
                .upgrade()
                .ok_or(ValueError::UnsupportedOperation("Signal race Run expired"))?;
            let result = execute(&owner, imports.as_ref(), |runtime| {
                race_abort_reason(&owner, imports.as_ref(), &original_signal, runtime)
            })
            .and_then(|reason| Err(ValueError::Thrown(reason)));
            *captured.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(result);
            captured.1.notify_waiters();
            Ok(())
        });
        let listener = AbortListener::attach(factory.clone(), bindings.clone(), signal, body)?;
        Ok((original, Some((listener, rejected))))
    })();
    async move {
        let (original, guarded) = prepared?;
        let Some((mut listener, rejected)) = guarded else { return original.await; };
        let aborted = async {
            loop {
                let changed = rejected.1.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let result = rejected.0.lock().unwrap_or_else(|error| error.into_inner()).clone();
                if let Some(result) = result { return result; }
                changed.await;
            }
        };
        tokio::pin!(original);
        tokio::pin!(aborted);
        let outcome = tokio::select! { biased; result = &mut original => result, result = &mut aborted => result };
        listener.remove()?;
        outcome
    }.boxed()
}

/// Retry sleep uses Host timers and the same original caller signal semantics.
/// The timer is explicitly cleared at every completion or cancellation.
pub fn sleep(
    factory: Arc<GraphValueFactory>,
    bindings: Arc<dyn RetryBindings>,
    delay_ms: f64,
    signal: Option<ValueHandle>,
) -> BoxFuture<'static, Result<()>> {
    let prepared = (|| {
        bindings.check_invocation(&factory)?;
        if let Some(signal) = &signal {
            if execute(&factory, bindings.as_ref(), |runtime| {
                caller_aborted(&factory, Some(signal), runtime)
            })? {
                let reason = execute(&factory, bindings.as_ref(), |runtime| {
                    race_abort_reason(&factory, bindings.as_ref(), signal, runtime)
                })?;
                return Err(ValueError::Thrown(reason));
            }
        }
        let state = Arc::new((Mutex::new(None::<Result<()>>), Notify::new()));
        let fired = state.clone();
        let timer = bindings.timers().arm(
            delay_ms,
            Arc::new(move || {
                *fired.0.lock().unwrap_or_else(|error| error.into_inner()) = Some(Ok(()));
                fired.1.notify_waiters();
                Ok(())
            }),
        )?;
        let listener = if let Some(signal) = signal {
            let rejected = state.clone();
            let owner = Arc::downgrade(&factory);
            let imports = Arc::downgrade(&bindings);
            let captured_signal = signal.clone();
            let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
                let owner = owner.upgrade().ok_or(ValueError::UnsupportedOperation(
                    "Retry sleep Session expired",
                ))?;
                let imports = imports
                    .upgrade()
                    .ok_or(ValueError::UnsupportedOperation("Retry sleep Run expired"))?;
                let result = execute(&owner, imports.as_ref(), |runtime| {
                    race_abort_reason(&owner, imports.as_ref(), &captured_signal, runtime)
                })
                .and_then(|reason| Err(ValueError::Thrown(reason)));
                let mut settled = rejected.0.lock().unwrap_or_else(|error| error.into_inner());
                if settled.is_none() {
                    *settled = Some(result);
                }
                drop(settled);
                rejected.1.notify_waiters();
                Ok(())
            });
            match AbortListener::attach(factory.clone(), bindings.clone(), signal, body) {
                Ok(listener) => Some(listener),
                Err(error) => {
                    timer.clear()?;
                    return Err(error);
                }
            }
        } else {
            None
        };
        Ok((state, timer, listener))
    })();
    async move {
        let (state, timer, mut listener) = prepared?;
        let outcome = loop {
            let changed = state.1.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let result = state
                .0
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if let Some(result) = result {
                break result;
            }
            tokio::select! {
                _ = changed => {},
                error = bindings.invocation_ended() => break Err(error),
            }
        };
        timer.clear()?;
        if let Some(listener) = &mut listener {
            listener.remove()?;
        }
        outcome
    }
    .boxed()
}
