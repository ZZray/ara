//! Fixed READ delimiter children on the one admitted Native graph driver.
//! Native workers wait on unit replies; Graph edges/results never enter them.
use super::*;
use futures::task::{ArcWake, waker};
use std::{
    sync::atomic::AtomicBool,
    task::{Context, Poll},
};

pub(super) struct ReadWake {
    ready: AtomicBool,
    wake: Arc<Notify>,
}
impl ReadWake {
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
impl ArcWake for ReadWake {
    fn wake_by_ref(owner: &Arc<Self>) {
        owner.mark();
    }
}
struct PendingRead {
    future: BoxFuture<'static, Result<ValueHandle, ActualProviderError>>,
    reply: tokio::sync::oneshot::Sender<Result<(), NativeToolFault>>,
}
#[derive(Default)]
pub(super) struct ReadQueue {
    closed: bool,
    pending: HashMap<(HostEffectId, u64), PendingRead>,
    completed: HashMap<HostEffectId, Result<DeferredValue, StoredActualFailure>>,
}
impl NativeBuiltinRun {
    pub(in crate::actual_tools) fn bind_read_parent(
        &self,
        id: HostEffectId,
        ancestor: &Arc<InvocationRuntime>,
    ) -> Result<(), ActualProviderError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let job = state
            .jobs
            .get_mut(&id)
            .ok_or(ActualProviderError::Protocol(
                "Original READ parent has no admitted effect",
            ))?;
        if ancestor.parent.get() != Some(&job.parent) {
            return Err(ActualProviderError::Protocol(
                "Original READ effect lost invocation parent",
            ));
        }
        if job.read_parent.is_some() {
            return Err(ActualProviderError::Protocol(
                "Original READ effect parent already bound",
            ));
        }
        job.read_parent = Some(Arc::downgrade(ancestor));
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_delimited_read(
        &self,
        values: &SessionValues,
        id: HostEffectId,
        sequence: u64,
        scope: NativeScope,
        parent: NativeParentId,
        parts: Vec<String>,
        reply: tokio::sync::oneshot::Sender<Result<(), NativeToolFault>>,
    ) -> Result<(), ActualProviderError> {
        if reply.is_closed() {
            return Ok(());
        }
        let prepared = (|| {
            self.check_values(values)?;
            self.check_live()?;
            let ancestor = {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let job = state.jobs.get(&id).ok_or(ActualProviderError::Protocol(
                    "Original READ delimiter notice has no admitted job",
                ))?;
                if job.scope != scope || job.parent != parent {
                    return Err(ActualProviderError::Protocol(
                        "Original READ delimiter notice metadata differs",
                    ));
                }
                job.read_parent.as_ref().and_then(Weak::upgrade).ok_or(
                    ActualProviderError::Protocol("Original READ delimiter parent expired"),
                )?
            };
            let run = ancestor.run.as_ref().and_then(Weak::upgrade).ok_or(
                ActualProviderError::Protocol("Original READ delimiter Run expired"),
            )?;
            if !std::ptr::eq(run.as_ref(), self) {
                return Err(ActualProviderError::Protocol(
                    "Original READ delimiter belongs to another Run",
                ));
            }
            let bindings = self.read.delimited_bindings(values)?;
            Ok::<_, ActualProviderError>(super::super::read_delimited::execute(
                ancestor, run, scope, parent, parts, bindings,
            ))
        })();
        let future = match prepared {
            Ok(future) => future,
            Err(error) => {
                let _ = reply.send(Err(NativeToolFault::Closed));
                return Err(error);
            }
        };
        let mut queue = self
            .pending_read
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Original READ pending owner poisoned"))?;
        if queue.closed {
            let _ = reply.send(Err(NativeToolFault::Closed));
            drop(future);
            return Ok(());
        }
        if queue.pending.contains_key(&(id, sequence)) || queue.completed.contains_key(&id) {
            let _ = reply.send(Err(NativeToolFault::Closed));
            return Err(ActualProviderError::Protocol(
                "Original READ delimiter notice already queued",
            ));
        }
        queue
            .pending
            .insert((id, sequence), PendingRead { future, reply });
        drop(queue);
        self.read_wake.mark();
        Ok(())
    }
    pub(super) fn poll_delimited_reads(&self) -> Result<(), ActualProviderError> {
        self.check_live()?;
        self.read_wake.ready.store(false, Ordering::Release);
        let mut pending = {
            let mut queue = self.pending_read.lock().map_err(|_| {
                ActualProviderError::Protocol("Original READ pending owner poisoned")
            })?;
            if queue.closed {
                return Ok(());
            }
            std::mem::take(&mut queue.pending)
        };
        let wake = waker(self.read_wake.clone());
        let mut context = Context::from_waker(&wake);
        let mut unfinished = HashMap::new();
        for (key, mut work) in pending.drain() {
            if self
                .pending_read
                .lock()
                .map_err(|_| ActualProviderError::Protocol("Original READ pending owner poisoned"))?
                .closed
                || work.reply.is_closed()
            {
                let _ = work.reply.send(Err(NativeToolFault::Closed));
                drop(work.future);
                continue;
            }
            match work.future.as_mut().poll(&mut context) {
                Poll::Pending => {
                    unfinished.insert(key, work);
                }
                Poll::Ready(result) => {
                    let values = self.values.upgrade().ok_or(ActualProviderError::Protocol(
                        "Original READ completed values expired",
                    ))?;
                    let captured = match result {
                        Ok(result) => Ok(values.heap().capture_deferred(&result)?),
                        Err(error) => Err(StoredActualFailure::capture(&values, error)?),
                    };
                    let mut queue = self.pending_read.lock().map_err(|_| {
                        ActualProviderError::Protocol("Original READ pending owner poisoned")
                    })?;
                    if queue.closed {
                        let _ = work.reply.send(Err(NativeToolFault::Closed));
                    } else {
                        queue.completed.insert(key.0, captured);
                        let _ = work.reply.send(Ok(()));
                    }
                }
            }
        }
        let mut queue = self
            .pending_read
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Original READ pending owner poisoned"))?;
        if queue.closed {
            drop(queue);
            for (_, work) in unfinished {
                let _ = work.reply.send(Err(NativeToolFault::Closed));
                drop(work.future);
            }
        } else {
            queue.pending.extend(unfinished);
        }
        Ok(())
    }
    pub(super) fn take_delimited_read_result(
        &self,
        id: HostEffectId,
    ) -> Result<Option<Result<DeferredValue, StoredActualFailure>>, ActualProviderError> {
        Ok(self
            .pending_read
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Original READ pending owner poisoned"))?
            .completed
            .remove(&id))
    }
    pub(super) fn prepare_delimited_read_settlement(&self) -> Result<(), ActualProviderError> {
        // Queue closure precedes every worker join, including notice-only work.
        let queued = {
            let mut state = self.effects.state.lock().unwrap_or_else(|e| e.into_inner());
            state.sealed = true;
            let mut queued = Vec::new();
            for notice in &mut state.notices {
                if matches!(&notice.kind, NativeNoticeKind::ReadDelimited { .. }) {
                    queued.push(std::mem::replace(
                        &mut notice.kind,
                        NativeNoticeKind::ReadDelimitedClosed,
                    ));
                }
            }
            queued
        };
        for notice in queued {
            if let NativeNoticeKind::ReadDelimited { reply, .. } = notice {
                let _ = reply.send(Err(NativeToolFault::Closed));
            }
        }
        let pending = {
            let mut queue = self.pending_read.lock().map_err(|_| {
                ActualProviderError::Protocol("Original READ pending owner poisoned")
            })?;
            queue.closed = true;
            queue.completed.clear();
            std::mem::take(&mut queue.pending)
        };
        for (_, work) in pending {
            let _ = work.reply.send(Err(NativeToolFault::Closed));
            drop(work.future);
        }
        self.read_wake.ready.store(false, Ordering::Release);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        native_factory::{NativeFactoryImports, PreparedNativeSession},
        native_realm::NativeRealm,
        native_run::NativeRun,
    };
    #[tokio::test]
    async fn native_read_delimited_queued_pending_settle_before_worker_join_group() {
        for pending in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let realm = NativeRealm::prepare().unwrap();
            let source =
                BuiltinToolSource::new(&realm.values, ToolContext::new(directory.path()), &[])
                    .unwrap();
            let prepared = PreparedNativeSession::from_complete_bindings(
                realm.clone(),
                NativeFactoryImports {
                    source: source.clone(),
                    string_function: realm.globals.string.clone(),
                    bun: realm.globals.bun.clone(),
                    original_logger: None,
                },
            )
            .unwrap();
            let driver = NativeRun::begin(prepared, CancellationToken::new()).unwrap();
            let lease = driver.lease();
            let tools = source.bound_run().unwrap().unwrap();
            let mut reservation = None;
            let factory = realm.values.factory().clone();
            driver
                .calling_run(&factory)
                .unwrap()
                .with_runtime(&factory, &mut |r| {
                    reservation = Some(
                        tools
                            .reserve(
                                &realm.values,
                                tools.admission.capture_scope(r)?,
                                tools.admission.capture_parent(r)?,
                                &factory.heap().undefined()?,
                            )
                            .unwrap(),
                    );
                    factory.heap().undefined()
                })
                .unwrap();
            let reservation = reservation.unwrap();
            let id = reservation.id();
            let publisher = reservation.publisher();
            let late_publisher = publisher.clone();
            let (sent, response) = tokio::sync::oneshot::channel();
            reservation
                .enter(
                    CancellationToken::new(),
                    Box::pin(async move {
                        publisher.publish_read_output(ara_rpc::WireValue::String("pending exact output".into())).unwrap();
                        let result = publisher.read_delimited(vec!["a".into(), "b".into()]).await;
                        let _ = sent.send(result);
                        Err(NativeToolFault::NotDispatched(
                            "controlled read closure".into(),
                        ))
                    }),
                )
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !tools.effects.has_ready() {
                    tokio::task::yield_now().await
                }
            })
            .await
            .unwrap();
            if pending {
                let notice = tools.effects.take_notice().unwrap();
                let NativeNoticeKind::ReadDelimited { reply, .. } = notice.kind else {
                    panic!("expected delimiter notice")
                };
                assert_eq!(notice.id, id);
                // Controlled uncooperative Graph future demonstrates that closure
                // disposes/replies pending work before any native worker join.
                tools.pending_read.lock().unwrap().pending.insert(
                    (id, notice.sequence),
                    PendingRead {
                        future: Box::pin(futures::future::pending()),
                        reply,
                    },
                );
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), lease.settle())
                .await
                .unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(5), response)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_err()
            );
            assert!(tools.pending_read.lock().unwrap().pending.is_empty());
            assert!(tools.effects.take_read_publication(id).is_none(), "settlement clears pending exact output");
            assert!(matches!(late_publisher.publish_read_output(ara_rpc::WireValue::Null), Err(NativeToolFault::Closed)), "late output cannot cross a closed Run");
            assert!(matches!(late_publisher.publish_read_type_error("late TypeError".into()), Err(NativeToolFault::Closed)));
            assert!(tools.effects.take_read_publication(id).is_none());
            drop(driver);
        }
        // Exact READ completion uses the same admitted driver and captured
        // completion port as original delimiter results. No worker graph edge.
        use ara_rpc::value_heap::{CompiledFunctionForm, CompiledFunctionSpec, ValueHeap, ValueRuntime, ValueSnapshot};
        fn capture_completion(
            heap: &Arc<ValueHeap>, captures: &[ValueHandle], _: &ValueHandle,
            arguments: &[ValueHandle], runtime: &dyn ValueRuntime,
        ) -> Result<ValueHandle, ValueError> {
            let state = &captures[0];
            let count = heap.get(state, &"count".into(), runtime)?;
            let ValueSnapshot::Number(count) = heap.snapshot(&count)? else { panic!("callback count") };
            heap.define_own_data(state, "count".into(), heap.number(count + 1.0)?, runtime)?;
            heap.define_own_data(state, "kind".into(), arguments[0].clone(), runtime)?;
            heap.define_own_data(state, "payload".into(), arguments[1].clone(), runtime)?;
            heap.undefined()
        }
        static COMPLETION: CompiledFunctionSpec = CompiledFunctionSpec {
            source: "ARA controlled READ completion test", name: "captureCompletion", length: 2,
            form: CompiledFunctionForm::Arrow, capture_count: 1, body: capture_completion,
        };
        for mode in ["success", "TypeError", "delimiter-result", "delimiter-reason", "cancel"] {
            let directory = tempfile::tempdir().unwrap();
            let realm = NativeRealm::prepare().unwrap();
            let source = BuiltinToolSource::new(&realm.values, ToolContext::new(directory.path()), &[]).unwrap();
            let prepared = PreparedNativeSession::from_complete_bindings(realm.clone(), NativeFactoryImports {
                source: source.clone(), string_function: realm.globals.string.clone(),
                bun: realm.globals.bun.clone(), original_logger: None,
            }).unwrap();
            let driver = NativeRun::begin(prepared, CancellationToken::new()).unwrap();
            let lease = driver.lease(); let tools = source.bound_run().unwrap().unwrap();
            let factory = realm.values.factory().clone(); let heap = factory.heap();
            let captured = heap.object().unwrap();
            let original = heap.object().unwrap();
            let mut reservation = None;
            driver.calling_run(&factory).unwrap().with_runtime(&factory, &mut |runtime| {
                heap.define_own_data(&captured, "count".into(), heap.number(0.0)?, runtime)?;
                let port = heap.fresh_compiled_function(&COMPLETION, &[captured.clone()])?;
                reservation = Some(tools.reserve(&realm.values, tools.admission.capture_scope(runtime)?,
                    tools.admission.capture_parent(runtime)?, &port).unwrap());
                heap.undefined()
            }).unwrap();
            let reservation = reservation.unwrap(); let id = reservation.id(); let publisher = reservation.publisher();
            assert!(matches!(publisher.publish_read_output(ara_rpc::WireValue::Null), Err(NativeToolFault::Closed)), "unentered publication is closed");
            let cancel = CancellationToken::new(); let (release, wait) = tokio::sync::oneshot::channel();
            reservation.enter(cancel.clone(), Box::pin(async move {
                let _ = wait.await;
                if mode == "TypeError" { Err(NativeToolFault::Kernel("worker TypeError token".into())) }
                else { Ok(ToolOutput::text("legacy completion sentinel")) }
            })).unwrap();
            let exact = ara_rpc::WireValue::String(ara_rpc::WireString::from_units(vec![0x41, 0xd800, 0x42]));
            if mode == "TypeError" { publisher.publish_read_type_error("children is not iterable".into()).unwrap(); }
            else { publisher.publish_read_output(exact.clone()).unwrap(); }
            assert!(matches!(publisher.publish_read_output(ara_rpc::WireValue::Null), Err(NativeToolFault::Kernel(_))), "duplicate pending publication is rejected");
            if mode.starts_with("delimiter-") {
                let edge = if mode == "delimiter-result" { Ok(heap.capture_deferred(&original).unwrap()) }
                    else { Err(StoredActualFailure::capture(&realm.values, ActualProviderError::Value(ValueError::Thrown(original.clone()))).unwrap()) };
                tools.pending_read.lock().unwrap().completed.insert(id, edge);
            }
            if mode == "cancel" {
                tools.effects.cancel();
                assert!(cancel.is_cancelled());
                assert!(tools.effects.take_read_publication(id).is_none(), "cancel clears exact data before worker completion");
                assert!(matches!(publisher.publish_read_output(exact), Err(NativeToolFault::Closed)), "cancelled worker cannot publish late");
                release.send(()).unwrap();
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), lease.settle()).await.unwrap();
                assert!(tools.effects.take_read_publication(id).is_none());
                drop(driver); continue;
            }
            release.send(()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !tools.effects.has_ready() { tokio::task::yield_now().await; }
            }).await.unwrap();
            assert!(matches!(tools.drain_one(&realm.values).unwrap(), NativeHostDrain::RanJob));
            driver.calling_run(&factory).unwrap().with_runtime(&factory, &mut |runtime| {
                assert!(matches!(heap.snapshot(&heap.get(&captured, &"count".into(), runtime)?)?, ValueSnapshot::Number(1.0)));
                let kind = heap.get(&captured, &"kind".into(), runtime)?;
                let expected_kind = if mode == "TypeError" || mode == "delimiter-reason" { 2.0 } else { 1.0 };
                assert!(matches!(heap.snapshot(&kind)?, ValueSnapshot::Number(value) if value == expected_kind));
                let payload = heap.get(&captured, &"payload".into(), runtime)?;
                match mode {
                    "success" => assert!(matches!(heap.snapshot(&payload)?, ValueSnapshot::String(value) if value.units() == [0x41, 0xd800, 0x42])),
                    "TypeError" => {
                        assert!(matches!(heap.snapshot(&heap.get(&payload, &"name".into(), runtime)?)?, ValueSnapshot::String(value) if value.equals_ascii("TypeError")), "original same-realm TypeError ctor brand");
                        assert!(matches!(heap.snapshot(&heap.get(&payload, &"message".into(), runtime)?)?, ValueSnapshot::String(value) if value.equals_ascii("children is not iterable")));
                    }
                    _ => assert!(payload.same_identity(&original), "delimiter aggregate/reason precedes exact slot"),
                }
                heap.undefined()
            }).unwrap();
            assert!(tools.effects.take_read_publication(id).is_none(), "driver consumes exact slot once");
            assert!(tools.effects.take_read_publication(id).is_none());
            assert!(tools.take_delimited_read_result(id).unwrap().is_none());
            assert!(matches!(publisher.publish_read_type_error("after completion".into()), Err(NativeToolFault::Closed)));
            assert!(matches!(tools.drain_one(&realm.values).unwrap(), NativeHostDrain::Idle), "no duplicate terminal callback");
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), lease.settle()).await.unwrap();
            assert!(tools.effects.take_read_publication(id).is_none()); drop(driver);
        }

    }
}
