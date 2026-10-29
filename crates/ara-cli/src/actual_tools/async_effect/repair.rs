//! Edit side-completion polling under the existing original NativeRun driver.
//! Resource settlement contains no graph future and performs no Graph Get/Call.
use super::*;
use crate::{
    mock_provider_host::NativeCallingRun,
    native_realm::NativeRealm,
    retry_host::{NativeRetryBindings, repair::RepairEffectResources},
};
use ara_ai::{
    ActualStreamFailureReceipt,
    retry::{AbortSignalImports, RetryBindings},
};
use futures::task::{ArcWake, waker};
use std::{
    sync::atomic::AtomicBool,
    task::{Context, Poll},
};

pub(super) struct RepairWake {
    ready: AtomicBool,
    wake: Arc<Notify>,
}
impl RepairWake {
    pub fn new(wake: Arc<Notify>) -> Arc<Self> {
        Arc::new(Self {
            ready: AtomicBool::new(false),
            wake,
        })
    }
    pub fn is_ready(&self) -> bool {
        self.ready.load(Ordering::Acquire)
    }
    fn mark(&self) {
        self.ready.store(true, Ordering::Release);
        self.wake.notify_waiters();
    }
}
impl ArcWake for RepairWake {
    fn wake_by_ref(owner: &Arc<Self>) {
        owner.mark();
    }
}
pub(super) struct PendingRepair {
    future: BoxFuture<'static, Result<String, ValueError>>,
    reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    cancel: CancellationToken,
    receipt: ActualStreamFailureReceipt,
    calling: NativeCallingRun,
    factory: Arc<ara_ai::graph_values::GraphValueFactory>,
}
#[derive(Default)]
pub(super) struct RepairQueue {
    closed: bool,
    pending: HashMap<(HostEffectId, u64), PendingRepair>,
}
impl NativeBuiltinRun {
    pub fn bind_retry_realm(&self, realm: &Arc<NativeRealm>) -> Result<(), ActualProviderError> {
        let values = self.values.upgrade().ok_or(ActualProviderError::Protocol(
            "Original Edit repair values expired",
        ))?;
        self.check_values(&realm.values)?;
        if !Arc::ptr_eq(values.factory(), realm.values.factory()) {
            return Err(ValueError::ForeignHeap.into());
        }
        if let Some(existing) = self.retry_realm.get() {
            let existing = existing.upgrade().ok_or(ActualProviderError::Protocol(
                "Original Edit repair realm expired",
            ))?;
            if !Arc::ptr_eq(&existing, realm) {
                return Err(ValueError::ForeignHeap.into());
            }
            return Ok(());
        }
        self.retry_realm
            .set(Arc::downgrade(realm))
            .map_err(|_| ActualProviderError::Protocol("Original Edit repair realm binding raced"))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_repair(
        &self,
        values: &SessionValues,
        effect: HostEffectId,
        sequence: u64,
        scope: NativeScope,
        parent: NativeParentId,
        completion: Arc<dyn ara_tools::edit_post_apply::EditRepairCompletion>,
        prompt: String,
        cancel: CancellationToken,
        reply: tokio::sync::oneshot::Sender<Result<String, String>>,
    ) -> Result<(), ActualProviderError> {
        if reply.is_closed() || cancel.is_cancelled() {
            let _ = reply.send(Err("Edit repair cancelled before model dispatch".into()));
            return Ok(());
        }
        let receipt = ActualStreamFailureReceipt::default();
        let settled = receipt.owner_settlement_guard();
        let prepared = (|| {
            self.check_values(values)?;
            self.check_live()?;
            let realm = self.retry_realm.get().and_then(Weak::upgrade).ok_or(
                ActualProviderError::Protocol("Original Edit repair realm is not bound"),
            )?;
            let factory = values.factory().clone();
            let calling = {
                let runtime = self.admission.scoped(scope, parent)?;
                NativeCallingRun::capture(&factory, values, self.admission.clone(), &runtime)?
            };
            self.repair_resources.retain_auxiliary(receipt.clone())?;
            let bindings =
                NativeRetryBindings::prepare(&factory, &realm, calling.clone(), receipt.clone())?;
            let module = bindings.oneshot_module(&factory)?;
            let controller = {
                let mut controller = None;
                calling.with_runtime(&factory, &mut |runtime| {
                    controller = Some(bindings.signals().controller(&factory, runtime)?);
                    factory.heap().undefined()
                })?;
                controller.ok_or(ValueError::InvalidNode)?
            };
            let invocation = ara_tools::edit_post_apply::EditNativeRepairInvocation {
                factory: factory.clone(),
                bindings: bindings.clone(),
                module,
                signal: controller.signal.clone(),
                resources: RepairEffectResources::prepare(self.repair_resources.clone(), effect),
            };
            let original = completion.complete_actual(invocation, prompt, cancel.clone());
            let cancelled = cancel.clone();
            let retained_calling = calling.clone();
            let retained_factory = factory.clone();
            let future = Box::pin(async move {
                let _settled = settled;
                tokio::pin!(original);
                tokio::select! {
                    result=&mut original=>result,
                    _=cancelled.cancelled()=>{
                        calling.with_runtime(&factory, &mut |runtime| {
                            bindings.signals().abort(&factory, &controller, None, runtime)?;
                            factory.heap().undefined()
                        })?;
                        original.await
                    }
                }
            });
            Ok::<_, ActualProviderError>((
                future as BoxFuture<'static, Result<String, ValueError>>,
                retained_calling,
                retained_factory,
            ))
        })();
        let (future, calling, factory) = match prepared {
            Ok(future) => future,
            Err(error) => {
                let _ = reply.send(Err("Original Edit repair preparation failed".into()));
                return Err(error);
            }
        };
        let key = (effect, sequence);
        let mut pending = self.pending_repair.lock().map_err(|_| {
            ActualProviderError::Protocol("Original Edit repair pending owner poisoned")
        })?;
        if pending.closed {
            cancel.cancel();
            let _ = reply.send(Err(
                "Original Edit repair graph closed before completion".into()
            ));
            drop(future);
            return Ok(());
        }
        if pending.pending.contains_key(&key) {
            let _ = reply.send(Err("Original Edit repair notice was already queued".into()));
            return Err(ActualProviderError::Protocol(
                "Original Edit repair notice was already queued",
            ));
        }
        pending.pending.insert(
            key,
            PendingRepair {
                future,
                reply,
                cancel,
                receipt,
                calling,
                factory,
            },
        );
        drop(pending);
        self.repair_wake.mark();
        Ok(())
    }
    pub(super) fn poll_repairs(&self) -> Result<(), ActualProviderError> {
        self.check_live()?;
        self.repair_wake.ready.store(false, Ordering::Release);
        let mut pending = {
            let mut queue = self.pending_repair.lock().map_err(|_| {
                ActualProviderError::Protocol("Original Edit repair pending owner poisoned")
            })?;
            if queue.closed {
                return Ok(());
            }
            std::mem::take(&mut queue.pending)
        };
        let wake = waker(self.repair_wake.clone());
        let mut context = Context::from_waker(&wake);
        let mut unfinished = HashMap::new();
        // No bookkeeping lock spans a future poll or its native expressions.
        for (key, mut work) in pending.drain() {
            if self
                .pending_repair
                .lock()
                .map_err(|_| {
                    ActualProviderError::Protocol("Original Edit repair pending owner poisoned")
                })?
                .closed
            {
                work.cancel.cancel();
                let _ = work.reply.send(Err(
                    "Original Edit repair graph closed before completion".into()
                ));
                drop(work.future);
                continue;
            }
            if work.reply.is_closed() {
                work.cancel.cancel();
            }
            match work.future.as_mut().poll(&mut context) {
                Poll::Ready(Ok(text)) => {
                    let _ = work.reply.send(Ok(text));
                }
                Poll::Ready(Err(error)) => {
                    let message = match &error {
                        ValueError::Thrown(thrown) => work
                            .calling
                            .with_runtime(&work.factory, &mut |runtime| {
                                let heap = work.factory.heap();
                                let realm = self.retry_realm.get().and_then(Weak::upgrade).ok_or(
                                    ValueError::UnsupportedOperation(
                                        "Original Edit repair realm expired",
                                    ),
                                )?;
                                let error_constructor =
                                    heap.get(&realm.globals.environment, &"Error".into(), runtime)?;
                                let value =
                                    if heap.instance_of(thrown, &error_constructor, runtime)? {
                                        heap.get(thrown, &"message".into(), runtime)?
                                    } else {
                                        let string = heap.get(
                                            &realm.globals.environment,
                                            &"String".into(),
                                            runtime,
                                        )?;
                                        heap.call(
                                            &string,
                                            &heap.undefined()?,
                                            std::slice::from_ref(thrown),
                                            runtime,
                                        )?
                                    };
                                Ok(value)
                            })
                            .and_then(|value| match work.factory.heap().snapshot(&value)? {
                                ara_rpc::value_heap::ValueSnapshot::String(text) => {
                                    text.to_utf8().map_err(|_| {
                                        ValueError::LegacyBridge(
                                            "Edit diagnostic requires scalar text",
                                        )
                                    })
                                }
                                _ => Err(ValueError::TypeError(
                                    "Edit diagnostic is not a string".into(),
                                )),
                            }),
                        _ => Ok(error.to_string()),
                    };
                    let message = match message {
                        Ok(message) => message,
                        Err(secondary) => {
                            work.receipt.record(secondary);
                            "Edit auto-repair failed".into()
                        }
                    };
                    self.repair_resources.retain_caught(error);
                    let _ = work.reply.send(Err(message));
                }
                Poll::Pending => {
                    unfinished.insert(key, work);
                }
            }
        }
        let mut queue = self.pending_repair.lock().map_err(|_| {
            ActualProviderError::Protocol("Original Edit repair pending owner poisoned")
        })?;
        if queue.closed {
            drop(queue);
            for (_, work) in unfinished {
                work.cancel.cancel();
                let _ = work.reply.send(Err(
                    "Original Edit repair graph closed before completion".into()
                ));
                drop(work.future);
            }
        } else {
            queue.pending.extend(unfinished);
        }
        Ok(())
    }
    /// Called at final settlement before joining worker effects. The live
    /// graph future may be what that worker is waiting for, so it must be
    /// dropped/replied first. Started producer receipts and captured writes
    /// remain retained, including explicit unknown usage after revocation.
    pub fn prepare_repair_settlement(&self) -> Result<(), ActualProviderError> {
        self.prepare_delimited_read_settlement()?;
        // Seal worker notice admission atomically with closing every queued
        // reply. Keep a placeholder so subsequent notices retain source order.
        let queued = {
            let mut state = self
                .effects
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.sealed = true;
            let mut queued = Vec::new();
            for notice in &mut state.notices {
                if matches!(&notice.kind, NativeNoticeKind::EditRepairComplete { .. }) {
                    queued.push(std::mem::replace(
                        &mut notice.kind,
                        NativeNoticeKind::EditRepairClosed,
                    ));
                }
            }
            queued
        };
        for notice in queued {
            if let NativeNoticeKind::EditRepairComplete { cancel, reply, .. } = notice {
                cancel.cancel();
                let _ = reply.send(Err(
                    "Original Edit repair graph closed before dispatch".into()
                ));
            }
        }
        self.repair_resources.revoke_graph();
        let pending = {
            let mut queue = self.pending_repair.lock().map_err(|_| {
                ActualProviderError::Protocol("Original Edit repair pending owner poisoned")
            })?;
            queue.closed = true;
            std::mem::take(&mut queue.pending)
        };
        for (_, work) in pending {
            work.cancel.cancel();
            let _ = work.reply.send(Err(
                "Original Edit repair graph closed before completion".into()
            ));
            drop(work.future);
        }
        self.repair_wake.ready.store(false, Ordering::Release);
        Ok(())
    }
    pub async fn settle_repair_resources(&self) -> Result<(), ActualProviderError> {
        self.repair_resources.settle().await
    }
}
