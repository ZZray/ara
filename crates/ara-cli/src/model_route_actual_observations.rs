//! Fixture-only observations of the existing private cancellation driver.
//! Called by the existing model_route coarse entrance; no new test attribute.
use super::*;
use ara_rpc::value_heap::{NoRuntime, ValueHeap};
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// Fixture-only equality observation. Credentials remain private and are
/// never included in assertion diagnostics or made publicly readable.
pub fn lease_snapshot_matches_fixture(left: &RequestAuthLease, right: &RequestAuthLease) -> bool {
    left.identity == right.identity && left.api_key == right.api_key && left.headers == right.headers
}

struct Observer {
    request: RequestIdentity,
    lease: RequestAuthLease,
    root: ValueHandle,
    settled: AtomicUsize,
    quota: AtomicUsize,
    interrupted: AtomicUsize,
    thrown: Option<ValueHandle>,
    throw_settlement: bool,
    calls: Mutex<Vec<&'static str>>,
}
impl RequestReceiptObserver for Observer {
    fn started(&self, _: &RequestIdentity) -> Result<(), ReceiptError> {
        panic!("fixture enters after dispatch")
    }
    fn settled(&self, _: &RequestIdentity, _: &AssistantMessage) -> Result<(), ReceiptError> {
        panic!("typed fixture settlement")
    }
    fn settled_actual(
        &self,
        request: &RequestIdentity,
        root: &ValueHandle,
        _: &GraphValueFactory,
    ) -> Result<(), ValueError> {
        assert_eq!(request, &self.request);
        assert!(root.same_identity(&self.root));
        self.settled.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().unwrap().push("settled");
        if self.throw_settlement {
            return Err(ValueError::Thrown(self.thrown.clone().unwrap()));
        }
        Ok(())
    }
    fn quota_rejected_actual(
        &self,
        request: &RequestIdentity,
        lease: &RequestAuthLease,
        root: &ValueHandle,
        _: &GraphValueFactory,
    ) -> Result<bool, ValueError> {
        assert_eq!(request, &self.request);
        assert_eq!(lease.identity(), self.lease.identity());
        assert_eq!(lease.api_key(), self.lease.api_key());
        assert!(root.same_identity(&self.root));
        self.quota.fetch_add(1, Ordering::SeqCst);
        self.calls.lock().unwrap().push("quota");
        match &self.thrown {
            Some(thrown) => Err(ValueError::Thrown(thrown.clone())),
            None => Ok(true),
        }
    }
    fn interrupted(&self, request: &RequestIdentity) {
        assert_eq!(request, &self.request);
        self.interrupted.fetch_add(1, Ordering::SeqCst);
    }
}
fn put(factory: &GraphValueFactory, root: &ValueHandle, key: &str, value: ValueHandle) {
    actual_put(factory, root, key, value).unwrap();
}

/// Runs the actual private finish_cancelled_actual branch, not a reimplemented
/// observer simulation. The input terminal is ready before any queue read.
/// An original producer remains unknown until after wrapper EOF. There is no
/// network/provider/reset/tool dispatch in this controlled fault family.
pub async fn cancelled_quota_queue_family() {
    for ahead in [0, 1, 255] {
        for scenario in ["quota", "unknown", "already-owned", "quota-throw", "settlement-throw"] {
            let factory = Arc::new(GraphValueFactory::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap());
            let heap = factory.heap();
            let model = Model {
                id: "queue-quota".into(),
                api: "openai-completions".into(),
                provider: "queue-quota".into(),
                base_url: String::new(),
                reasoning: false,
                max_tokens: None,
                context_window: None,
                tokenizer: None,
            };
            let root = heap.object().unwrap();
            for (key, value) in [
                ("role", "assistant"),
                ("api", model.api.as_str()),
                ("provider", model.provider.as_str()),
                ("model", model.id.as_str()),
                ("stopReason", "error"),
                (
                    "errorMessage",
                    if scenario == "unknown" {
                        "dispatch outcome unknown"
                    } else {
                        "insufficient_quota: quota exceeded"
                    },
                ),
            ] {
                put(&factory, &root, key, heap.string(value.into()).unwrap());
            }
            let content = heap.array(vec![]).unwrap();
            let usage = heap.object().unwrap();
            put(&factory, &root, "content", content.clone());
            put(&factory, &root, "usage", usage.clone());
            put(&factory, &root, "timestamp", heap.number(1.0).unwrap());
            let evidence = heap.object().unwrap();
            put(
                &factory,
                &evidence,
                "kind",
                heap.string(if scenario == "unknown" { "transport" } else { "http" }.into()).unwrap(),
            );
            put(
                &factory,
                &evidence,
                "errorId",
                heap.number(if scenario == "unknown" {
                    0.0
                } else {
                    f64::from(
                        ara_ai::retry_classification::flag::CLASS | ara_ai::retry_classification::flag::USAGE_LIMIT,
                    )
                })
                .unwrap(),
            );
            if scenario != "unknown" {
                put(&factory, &evidence, "status", heap.number(429.0).unwrap());
                put(&factory, &evidence, "code", heap.string("insufficient_quota".into()).unwrap());
                put(&factory, &evidence, "contextRecovery", heap.string("usageAdmission".into()).unwrap());
            }
            put(&factory, &root, "failureEvidence", evidence);
            let lease = RequestAuthLease::new(
                CredentialIdentity::Stored { id: 47, revision: 3 },
                Some("queue-synthetic-key".into()),
            );
            let request = RequestIdentity {
                call_id: uuid::Uuid::new_v4(),
                route_generation: 19,
                provider: model.provider.clone(),
                model_id: model.id.clone(),
                credential: lease.identity().clone(),
            };
            let thrown = matches!(scenario, "quota-throw" | "settlement-throw").then(|| heap.object().unwrap());
            let observer = Arc::new(Observer {
                request: request.clone(),
                lease: lease.clone(),
                root: root.clone(),
                settled: AtomicUsize::new(0),
                quota: AtomicUsize::new(0),
                interrupted: AtomicUsize::new(0),
                thrown: thrown.clone(),
                throw_settlement: scenario == "settlement-throw",
                calls: Mutex::new(vec![]),
            });
            let dyn_observer: Arc<dyn RequestReceiptObserver> = observer.clone();
            let (input_sink, mut inner, original_receipt) = ActualAssistantStream::channel();
            let original_owner = original_receipt.owner_settlement_guard();
            for _ in 0..ahead {
                assert!(input_sink.try_push(ActualAssistantEvent::Start { partial: root.clone() }));
            }
            assert!(
                input_sink.try_push(ActualAssistantEvent::Error { reason: StopReason::Error, error: root.clone() })
            );
            drop(input_sink);
            let (output_sink, mut output, failures) = ActualAssistantStream::channel();
            failures.inherit_stream(&inner);
            let output_owner = failures.owner_settlement_guard();
            let cancel = CancellationToken::new();
            cancel.cancel();
            tokio::time::timeout(
                std::time::Duration::from_secs(2),
                finish_cancelled_actual(
                    &mut inner,
                    &output_sink,
                    &failures,
                    &factory,
                    ActualCancelledRequest {
                        identity: &request,
                        lease: &lease,
                        quota_feedback_owned: scenario == "already-owned",
                        observer: Some(&dyn_observer),
                        model: &model,
                        cancel: &cancel,
                    },
                    None,
                    false,
                ),
            )
            .await
            .expect("queued known outcome cannot wait for unknown original owner");
            drop(output_sink);
            drop(output_owner);
            let mut events = Vec::new();
            while let Some(event) = output.recv().await {
                events.push(event);
            }
            assert_eq!(events.len(), 1, "{ahead}/{scenario}");
            assert!(events[0].is_terminal());
            let received = events[0].partial();
            assert!(heap.get(received, &"content".into(), factory.runtime()).unwrap().same_identity(&content));
            assert!(heap.get(received, &"usage".into(), factory.runtime()).unwrap().same_identity(&usage));
            assert_eq!(observer.settled.load(Ordering::SeqCst), 1);
            assert_eq!(observer.quota.load(Ordering::SeqCst), usize::from(matches!(scenario, "quota" | "quota-throw")));
            assert_eq!(observer.interrupted.load(Ordering::SeqCst), 0);
            assert!(failures.known_terminals().iter().any(|event| event.partial().same_identity(&root)));
            assert!(cancel.is_cancelled());
            assert!(!failures.is_settled(), "unknown original producer remains open");
            if scenario != "settlement-throw" {
                assert!(received.same_identity(&root));
            }
            drop(output);
            drop(inner);
            drop(original_owner);
            let faults =
                tokio::time::timeout(std::time::Duration::from_secs(2), failures.wait_settled()).await.unwrap();
            match thrown {
                Some(thrown) => {
                    assert!(faults.iter().any(
                        |error| matches!(error.as_ref(), ValueError::Thrown(value) if value.same_identity(&thrown))
                    ))
                }
                None => assert!(faults.is_empty()),
            }
        }
    }
}
