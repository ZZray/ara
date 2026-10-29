//! One original NativeRun settlement group: queued and pending Edit side work
//! close before worker joins, while actual producer and captured usage joins
//! survive graph-future disposal. Controlled gates expose the ordering risk.
use super::*;
use crate::{
    native_factory::{NativeFactoryImports, PreparedNativeSession},
    native_realm::NativeRealm,
    native_run::NativeRun,
};
use ara_ai::retry::result::ActualCompletionStream;
use ara_tools::edit_post_apply::{EditNativeRepairInvocation, EditRepairCompletion};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

struct FutureDisposed(Arc<Notify>);
impl Drop for FutureDisposed {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct GatedCompletion {
    dispatched: AtomicUsize,
    started: Arc<Notify>,
    disposed: Arc<Notify>,
    producer_release: Arc<Notify>,
    write_started: Arc<Notify>,
    write_release: Arc<Notify>,
    original_reason: ValueHandle,
    producer: Mutex<Option<tokio::task::JoinHandle<()>>>,
}
#[async_trait::async_trait]
impl EditRepairCompletion for GatedCompletion {
    fn model_name(&self) -> String {
        "controlled-original-repair".into()
    }
    async fn complete(&self, _: String, _: CancellationToken) -> Result<String, String> {
        panic!("Original Native repair must not fall back to typed completion")
    }
    fn complete_actual(
        &self,
        invocation: EditNativeRepairInvocation,
        _: String,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<String, ValueError>> {
        self.dispatched.fetch_add(1, Ordering::SeqCst);
        let resources = invocation.resources.clone();
        let factory = invocation.factory.clone();
        let started = self.started.clone();
        let disposed = self.disposed.clone();
        let release = self.producer_release.clone();
        let write_started = self.write_started.clone();
        let write_release = self.write_release.clone();
        let reason = self.original_reason.clone();
        // Original completion dispatch registers its real producer before its
        // graph future can be cancelled. The producer has an independent gate.
        let key = resources.begin_attempt(cancel.clone()).unwrap();
        let (mut producer, stream, receipt) = ActualCompletionStream::channel(&factory);
        resources.register_started(key, receipt).unwrap();
        let write = resources
            .register_usage(
                key,
                1,
                Box::pin(async move {
                    write_started.notify_one();
                    write_release.notified().await;
                    Err("original captured usage write failed".into())
                }),
            )
            .unwrap();
        // The owner retains this exact write even when the graph waiter drops.
        drop(write);
        let task = tokio::spawn(async move {
            release.notified().await;
            producer.fail_original(ValueError::Thrown(reason)).unwrap();
        });
        *self.producer.lock().unwrap() = Some(task);
        Box::pin(async move {
            let _disposed = FutureDisposed(disposed);
            let _original_stream = stream;
            started.notify_one();
            // Deliberately uncooperative original graph waiter. Closing the
            // Run must dispose it before awaiting the worker's original reply.
            futures::future::pending::<Result<String, ValueError>>().await
        })
    }
}

fn contains_original(error: &ActualProviderError, original: &ValueHandle) -> bool {
    match error {
        ActualProviderError::Value(ValueError::Thrown(value)) => value.same_identity(original),
        ActualProviderError::Compound { primary, secondary } => {
            contains_original(primary, original) || contains_original(secondary, original)
        }
        _ => false,
    }
}
fn contains_usage_failure(error: &ActualProviderError) -> bool {
    match error {
        ActualProviderError::Callback(message) => message
            .contains("Repair usage persistence failed: original captured usage write failed"),
        ActualProviderError::Compound { primary, secondary } => {
            contains_usage_failure(primary) || contains_usage_failure(secondary)
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn original_native_run_closes_queued_pending_repair_before_join_and_retains_resource_failures_group()
 {
    for pending in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let realm = NativeRealm::prepare().unwrap();
        let source =
            BuiltinToolSource::new(&realm.values, ToolContext::new(directory.path()), &[]).unwrap();
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
        let run = NativeRun::begin(prepared, CancellationToken::new()).unwrap();
        let lease = run.lease();
        let tools = source.bound_run().unwrap().unwrap();
        let factory = realm.values.factory().clone();
        let calling = run.calling_run(&factory).unwrap();
        let mut reservation = None;
        calling
            .with_runtime(&factory, &mut |runtime| {
                let scope = tools.admission.capture_scope(runtime)?;
                let parent = tools.admission.capture_parent(runtime)?;
                // Repair notices use the admitted job's scope/parent; this port is
                // never invoked by the original repair notice branch.
                reservation = Some(
                    tools
                        .reserve(&realm.values, scope, parent, &factory.heap().undefined()?)
                        .map_err(|_| {
                            ValueError::UnsupportedOperation(
                                "Original repair test reservation failed",
                            )
                        })?,
                );
                factory.heap().undefined()
            })
            .unwrap();
        let reservation = reservation.unwrap();
        let effect_id = reservation.id();
        let publisher = reservation.publisher();
        let completion = Arc::new(GatedCompletion {
            dispatched: AtomicUsize::new(0),
            started: Arc::new(Notify::new()),
            disposed: Arc::new(Notify::new()),
            producer_release: Arc::new(Notify::new()),
            write_started: Arc::new(Notify::new()),
            write_release: Arc::new(Notify::new()),
            original_reason: factory.heap().object().unwrap(),
            producer: Mutex::new(None),
        });
        let queued = Arc::new(Notify::new());
        let queued_in_worker = queued.clone();
        let completion_in_worker = completion.clone();
        let repair_cancel = CancellationToken::new();
        let repair_cancel_in_worker = repair_cancel.clone();
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        reservation
            .enter(
                CancellationToken::new(),
                Box::pin(async move {
                    let original = publisher.complete_edit_repair(
                        completion_in_worker,
                        "original repair".into(),
                        repair_cancel_in_worker,
                    );
                    tokio::pin!(original);
                    let reply = futures::future::poll_fn(|cx| match original.as_mut().poll(cx) {
                        std::task::Poll::Pending => {
                            queued_in_worker.notify_one();
                            std::task::Poll::Pending
                        }
                        std::task::Poll::Ready(reply) => std::task::Poll::Ready(reply),
                    })
                    .await;
                    let _ = reply_tx.send(reply);
                    Err(NativeToolFault::NotDispatched(
                        "Controlled worker closed without a native edit".into(),
                    ))
                }),
            )
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), queued.notified())
            .await
            .unwrap();
        if pending {
            assert!(matches!(
                tools.drain_one(&realm.values).unwrap(),
                NativeHostDrain::RanJob
            ));
            assert!(matches!(
                tools.drain_one(&realm.values).unwrap(),
                NativeHostDrain::RanJob
            ));
            tokio::time::timeout(Duration::from_secs(5), completion.started.notified())
                .await
                .unwrap();
        }
        let settling = lease.settle();
        tokio::pin!(settling);
        // This first poll must close queued/pending repair replies before the
        // worker join. The unmodified ordering hung awaiting that worker.
        let first_settlement = match futures::poll!(&mut settling) {
            std::task::Poll::Pending => None,
            std::task::Poll::Ready(result) => Some(result),
        };
        let reply = tokio::time::timeout(Duration::from_secs(5), reply_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(
            reply.is_err(),
            "Run closure must never manufacture repair success"
        );
        assert!(
            repair_cancel.is_cancelled(),
            "Original repair cancellation was not closed"
        );
        if pending {
            assert!(
                first_settlement.is_none(),
                "Run settled before original resource gates"
            );
            tokio::time::timeout(Duration::from_secs(5), completion.disposed.notified())
                .await
                .unwrap();
            assert_eq!(completion.dispatched.load(Ordering::SeqCst), 1);
            // Poll settlement through the completed worker into resource joins.
            tokio::time::timeout(Duration::from_secs(5), async {
                tokio::select! {
                    biased;
                    result = &mut settling => panic!("Run settled before original resource gates: {result:?}"),
                    _ = completion.write_started.notified() => {},
                }
            }).await.unwrap();
            assert!(futures::poll!(&mut settling).is_pending());
            completion.producer_release.notify_one();
            let producer = completion.producer.lock().unwrap().take().unwrap();
            tokio::time::timeout(Duration::from_secs(5), producer)
                .await
                .unwrap()
                .unwrap();
            assert!(
                futures::poll!(&mut settling).is_pending(),
                "Original usage write must still be joined"
            );
            completion.write_release.notify_one();
            let error = tokio::time::timeout(Duration::from_secs(5), &mut settling)
                .await
                .unwrap()
                .unwrap_err();
            assert!(
                contains_original(&error, &completion.original_reason),
                "Late original producer throw identity lost: {error:?}"
            );
            assert!(
                contains_usage_failure(&error),
                "Captured persistence failure lost: {error:?}"
            );
            let repeated = tokio::time::timeout(Duration::from_secs(5), lease.settle())
                .await
                .unwrap()
                .unwrap_err();
            assert!(contains_original(&repeated, &completion.original_reason));
            assert!(contains_usage_failure(&repeated));
        } else {
            assert_eq!(
                completion.dispatched.load(Ordering::SeqCst),
                0,
                "Undispatched queued repair was replayed during settlement"
            );
            match first_settlement {
                Some(result) => result.unwrap(),
                None => tokio::time::timeout(Duration::from_secs(5), &mut settling)
                    .await
                    .unwrap()
                    .unwrap(),
            }
        }
        let receipt = lease.effects().settle_one(effect_id).await.unwrap();
        assert!(receipt.failed());
        assert!(
            !receipt.uncertain(),
            "Original worker join lost its not-dispatched receipt"
        );
        drop(run);
    }
}
