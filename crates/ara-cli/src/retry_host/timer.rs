//! Issuing-Run timers. OS workers only publish A's sealed native undefined
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md (MIT).
//! timer ticket. The original Promise driver dispatches the compiled callback.
use super::signal::NativeSignalDomain;
use crate::mock_provider_host::NativeCallingRun;
use ara_ai::{
    ActualStreamFailureReceipt,
    graph_values::GraphValueFactory,
    retry::{AbortSignalImports, TimerImports, TimerLease},
};
use ara_rpc::value_heap::ValueError;
use futures::{FutureExt, future::BoxFuture};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::sync::Notify;

type Result<T> = std::result::Result<T, ValueError>;
struct TimerEntry {
    cancelled: AtomicBool,
    callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    compiled_owner: Mutex<Option<Arc<dyn Fn() -> Result<()> + Send + Sync>>>,
    dispatch: Mutex<Option<TimerLease>>,
}
pub(crate) struct NativeTimerDomain {
    owner: Weak<Self>,
    factory: Weak<GraphValueFactory>,
    calling: NativeCallingRun,
    receipt: ActualStreamFailureReceipt,
    signals: Arc<NativeSignalDomain>,
    entries: Mutex<HashMap<u64, Arc<TimerEntry>>>,
    sequence: AtomicU64,
}
impl NativeTimerDomain {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        calling: NativeCallingRun,
        receipt: ActualStreamFailureReceipt,
        signals: Arc<NativeSignalDomain>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|owner| Self {
            owner: owner.clone(),
            factory: Arc::downgrade(factory),
            calling,
            receipt,
            signals,
            entries: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(1),
        })
    }
    fn clear_entry(&self, key: u64) -> Result<()> {
        let entry = self
            .entries
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .get(&key)
            .cloned();
        let Some(entry) = entry else {
            return Ok(());
        };
        // Must latch before A cancels sleep and publishes the tick. Keep the
        // compiled callback alive until the old driver consumes that tick.
        entry.cancelled.store(true, Ordering::Release);
        let dispatch = entry
            .dispatch
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .clone();
        if let Some(dispatch) = dispatch {
            dispatch.clear()?;
        }
        Ok(())
    }
}
impl TimerImports for NativeTimerDomain {
    fn arm(
        &self,
        delay_ms: f64,
        callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    ) -> Result<TimerLease> {
        let factory = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation("Timer Session expired"))?;
        let owner = self
            .owner
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation("Timer Host expired"))?;
        let key = self.sequence.fetch_add(1, Ordering::Relaxed);
        if key == u64::MAX {
            return Err(ValueError::UnsupportedOperation(
                "Timer identifiers exhausted",
            ));
        }
        let entry = Arc::new(TimerEntry {
            cancelled: AtomicBool::new(false),
            callback,
            compiled_owner: Mutex::new(None),
            dispatch: Mutex::new(None),
        });
        {
            let mut entries = self.entries.lock().map_err(|_| ValueError::PoisonedHeap)?;
            entries.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            entries.insert(key, entry.clone());
        }
        let weak_owner = self.owner.clone();
        let weak_entry = Arc::downgrade(&entry);
        let body: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
            let Some(owner) = weak_owner.upgrade() else {
                return Ok(());
            };
            let Some(entry) = weak_entry.upgrade() else {
                return Ok(());
            };
            let result = if entry.cancelled.load(Ordering::Acquire) {
                Ok(())
            } else {
                (entry.callback)()
            };
            owner
                .entries
                .lock()
                .map_err(|_| ValueError::PoisonedHeap)?
                .remove(&key);
            entry
                .compiled_owner
                .lock()
                .map_err(|_| ValueError::PoisonedHeap)?
                .take();
            result
        });
        *entry
            .compiled_owner
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)? = Some(body.clone());
        let mut compiled = None;
        let preparation = self
            .calling
            .with_runtime(&factory, &mut |runtime| {
                compiled = Some(self.signals.callback(&factory, body.clone(), runtime)?);
                factory.heap().undefined()
            })
            .and_then(|_| {
                // Native setTimeout coerces nonfinite/under-one/overflow delays to
                // 1ms and truncates finite millisecond fractions toward zero.
                let delay = if !delay_ms.is_finite() || !(1.0..=2_147_483_647.0).contains(&delay_ms)
                {
                    1.0
                } else {
                    delay_ms.trunc()
                };
                self.calling.dispatch_after(
                    &factory,
                    delay,
                    compiled.take().ok_or(ValueError::InvalidNode)?,
                    self.receipt.clone(),
                )
            });
        let dispatch = match preparation {
            Ok(dispatch) => dispatch,
            Err(error) => {
                self.entries
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .remove(&key);
                return Err(error);
            }
        };
        *entry
            .dispatch
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)? = Some(dispatch);
        Ok(TimerLease::new(Arc::new(move || owner.clear_entry(key))))
    }
    fn wait(&self, delay_ms: f64) -> BoxFuture<'static, Result<()>> {
        let state = Arc::new((Mutex::new(false), Notify::new()));
        let fired = state.clone();
        let timer = self.arm(
            delay_ms,
            Arc::new(move || {
                *fired.0.lock().map_err(|_| ValueError::PoisonedHeap)? = true;
                fired.1.notify_waiters();
                Ok(())
            }),
        );
        let mut ended = self.calling.invocation_ended();
        async move {
            let timer = timer?;
            loop {
                let changed = state.1.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if *state.0.lock().map_err(|_| ValueError::PoisonedHeap)? {
                    break;
                }
                tokio::select! {
                    _ = &mut changed => {},
                    error = &mut ended => { timer.clear()?; return Err(error); }
                }
            }
            timer.clear()
        }
        .boxed()
    }
}
