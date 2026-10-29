mod support;
use ara_agent::{Agent, NullSink};
use ara_workflow::*;
use async_trait::async_trait;
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::*;
use tokio_util::sync::CancellationToken;

struct ReviewHost {
    mode: &'static str,
    reviews: AtomicUsize,
}
impl ReviewHost {
    fn new(mode: &'static str) -> Arc<Self> {
        Arc::new(Self { mode, reviews: AtomicUsize::new(0) })
    }
}
#[async_trait]
impl WorkflowHost for ReviewHost {
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        let verdict = if self.mode == "repair" {
            ReviewVerdict::NeedsChanges { feedback: "check failed; repair original artifact".into() }
        } else {
            ReviewVerdict::Verified
        };
        let mut receipt = receipt(candidate.basis.clone(), verdict);
        match self.mode {
            "wrong-basis" => receipt.basis.workflow.session_generation = "stale".into(),
            "uncommitted" => receipt.commit = ReceiptCommit::Uncommitted,
            "unknown" => receipt.settlement = Settlement::Unknown { reason: "effect receipt lost".into() },
            _ => {}
        }
        Ok(receipt)
    }
}

#[tokio::test]
async fn invalid_and_unknown_receipts_never_admit_feedback() {
    for (mode, expected) in [
        ("wrong-basis", WorkflowStop::InvalidReceipt),
        ("uncommitted", WorkflowStop::InvalidReceipt),
        ("unknown", WorkflowStop::Unknown),
    ] {
        let server = server(vec![text("stop"), text("stop")]).await;
        let cfg = config(&server, vec![]);
        let agent = Agent::new(cfg.clone(), vec![]);
        let host = ReviewHost::new(mode);
        let handle = Workflow::start(
            agent,
            host.clone(),
            request(),
            cfg,
            Arc::new(NullSink),
            policy(3, 8),
            CancellationToken::new(),
        )
        .unwrap();
        let outcome = handle.wait().await.unwrap();
        assert_eq!(outcome.stop, expected);
        assert_eq!(outcome.attempts.len(), 1);
        assert_eq!(server.served(), 1);
        assert_eq!(host.reviews.load(Ordering::SeqCst), 1);
        assert!(outcome.reconciliation.is_some());
    }
}

#[tokio::test]
async fn completed_length_is_not_a_reviewable_candidate() {
    let server = server(vec![text("length")]).await;
    let cfg = config(&server, vec![]);
    let host = ReviewHost::new("verified");
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        host.clone(),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(2, 8),
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = handle.wait().await.unwrap();
    assert_eq!(outcome.stop, WorkflowStop::NonStopAssistant);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn prepared_request_budget_accumulates_across_runs() {
    let server = server(vec![text("stop"), text("stop"), text("stop")]).await;
    let cfg = config(&server, vec![]);
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        ReviewHost::new("repair"),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(4, 2),
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = handle.wait().await.unwrap();
    assert_eq!(outcome.stop, WorkflowStop::PreparedRequestLimit);
    assert_eq!(outcome.prepared_requests, 2);
    assert_eq!(outcome.attempts.len(), 2);
    assert_eq!(server.served(), 2);
    assert!(outcome.reconciliation.is_none());
    let requests = server.requests.lock().await;
    let input = requests[1]["body"]["messages"].as_array().unwrap();
    assert!(input.iter().any(|m| m["content"].as_str() == Some("check failed; repair original artifact")));
}

struct BlockingReview {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait]
impl WorkflowHost for BlockingReview {
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.entered.notify_one();
        self.release.notified().await;
        Ok(receipt(candidate.basis.clone(), ReviewVerdict::Verified))
    }
}

#[tokio::test]
async fn cancellation_does_not_drop_host_settlement_and_wait_is_reusable() {
    let server = server(vec![text("stop")]).await;
    let cfg = config(&server, vec![]);
    let host = Arc::new(BlockingReview { entered: Default::default(), release: Default::default() });
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        host.clone(),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(2, 8),
        CancellationToken::new(),
    )
    .unwrap();
    host.entered.notified().await;
    let observer = handle.clone();
    let waiter = tokio::spawn(async move { observer.wait().await });
    handle.cancel();
    assert!(tokio::time::timeout(Duration::from_millis(25), handle.wait()).await.is_err());
    waiter.abort();
    let _ = waiter.await;
    host.release.notify_one();
    let first = handle.wait().await.unwrap();
    let second = handle.clone().wait().await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(first.stop, WorkflowStop::Cancelled);
    assert!(first.reconciliation.is_none());
}

#[tokio::test]
async fn expired_deadline_admits_no_core_run() {
    let server = server(vec![]).await;
    let cfg = config(&server, vec![]);
    let mut bounds = policy(2, 8);
    bounds.deadline = std::time::Instant::now() - Duration::from_millis(1);
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        ReviewHost::new("verified"),
        request(),
        cfg,
        Arc::new(NullSink),
        bounds,
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = handle.wait().await.unwrap();
    assert_eq!(outcome.stop, WorkflowStop::Deadline);
    assert_eq!(server.served(), 0);
    assert!(outcome.attempts.is_empty());
}

struct DriverHost {
    mode: &'static str,
    retained: Mutex<Option<CoreRunFuture>>,
}
#[async_trait]
impl WorkflowHost for DriverHost {
    async fn drive_run(&self, _: &AttemptBasis, run: CoreRunFuture, _: &CancellationToken) -> DriverResult {
        match self.mode {
            "panic" => panic!("controlled driver panic"),
            "spawn" => {
                tokio::spawn(run);
                Ok(Err(CoreRunFailure::AdmissionClosed))
            }
            "late" => {
                *self.retained.lock().unwrap() = Some(run);
                Ok(Err(CoreRunFailure::AdmissionClosed))
            }
            _ => {
                let _ = run.await;
                Err(HostDriverError::failed("settlement failed"))
            }
        }
    }
    async fn review(&self, _: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        panic!("invalid driver must not reach review")
    }
}

#[tokio::test]
async fn driver_panic_spawn_and_late_poll_preserve_closed_admission() {
    for mode in ["panic", "spawn", "late", "failed"] {
        let server = server(vec![text("stop"), text("stop")]).await;
        let cfg = config(&server, vec![]);
        let agent = Agent::new(cfg.clone(), vec![]);
        let host = Arc::new(DriverHost { mode, retained: Mutex::new(None) });
        let handle = Workflow::start(
            agent.clone(),
            host.clone(),
            request(),
            cfg.clone(),
            Arc::new(NullSink),
            policy(3, 8),
            CancellationToken::new(),
        )
        .unwrap();
        let outcome = handle.wait().await.unwrap();
        assert_eq!(
            outcome.stop,
            if mode == "failed" { WorkflowStop::DriverFailed } else { WorkflowStop::DriverDidNotJoin }
        );
        assert_eq!(outcome.attempts.len(), 1);
        assert!(outcome.reconciliation.is_some());
        let retained = host.retained.lock().unwrap().take();
        if let Some(run) = retained {
            assert!(matches!(run.await, Err(CoreRunFailure::AdmissionClosed)));
        }
        let before = server.served();
        let _ =
            agent.prompt_inputs_with_config(request().inputs, cfg, CancellationToken::new(), Arc::new(NullSink)).await;
        assert_eq!(server.served(), before, "closed workflow gate must protect same Agent");
    }
}

struct CountingGate(AtomicUsize);
#[async_trait]
impl ara_agent::FinalProviderGate for CountingGate {
    async fn before_model_call(
        &self,
        _: &mut ara_ai::Context,
        _: &CancellationToken,
    ) -> Result<Option<ara_agent::FinalProviderGateStop>, ara_agent::FinalProviderGateError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

#[tokio::test]
async fn verified_cleanup_preserves_existing_primary_and_additional_host_gates() {
    let server = server(vec![text("stop"), text("stop")]).await;
    let cfg = config(&server, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let primary = Arc::new(CountingGate(AtomicUsize::new(0)));
    let additional = Arc::new(CountingGate(AtomicUsize::new(0)));
    agent.set_final_provider_gate(Some(primary.clone()));
    let registration = agent.add_final_provider_gate(additional.clone());
    let handle = Workflow::start(
        agent.clone(),
        ReviewHost::new("verified"),
        request(),
        cfg.clone(),
        Arc::new(NullSink),
        policy(2, 8),
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = handle.wait().await.unwrap();
    assert_eq!(outcome.stop, WorkflowStop::VerifiedArtifact);
    assert!(outcome.reconciliation.is_none());
    assert_eq!(primary.0.load(Ordering::SeqCst), 1);
    assert_eq!(additional.0.load(Ordering::SeqCst), 1);
    let report = agent
        .prompt_inputs_with_config(request().inputs, cfg, CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap();
    assert_eq!(report.end, ara_agent::RunEnd::Completed);
    assert_eq!(primary.0.load(Ordering::SeqCst), 2);
    assert_eq!(additional.0.load(Ordering::SeqCst), 2);
    assert_eq!(server.served(), 2);
    registration.remove();
}

struct AliasingDriver {
    reviews: AtomicUsize,
}
#[async_trait]
impl WorkflowHost for AliasingDriver {
    async fn drive_run(&self, _: &AttemptBasis, run: CoreRunFuture, _: &CancellationToken) -> DriverResult {
        let result = run.await;
        if let Ok(report) = &result {
            for record in report.messages.iter().rev() {
                if let Some(ara_ai::Message::Assistant(mut message)) = record.checked_model_snapshot().unwrap() {
                    assert_eq!(message.stop_reason, ara_ai::StopReason::Length);
                    message.stop_reason = ara_ai::StopReason::Stop;
                    record
                        .replace_body(ara_agent::AgentMessageBody::Model(ara_ai::Message::Assistant(message)))
                        .unwrap();
                    break;
                }
            }
        }
        Ok(result)
    }
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        Ok(receipt(candidate.basis.clone(), ReviewVerdict::Verified))
    }
}

#[tokio::test]
async fn driver_shared_record_mutation_cannot_admit_a_length_terminal() {
    let server = server(vec![text("length")]).await;
    let cfg = config(&server, vec![]);
    let host = Arc::new(AliasingDriver { reviews: AtomicUsize::new(0) });
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        host.clone(),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(2, 8),
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = handle.wait().await.unwrap();
    // Demonstrate the public Arc record alias was mutated in the canonical
    // report too. Admission must use the owner's pre-publication receipt.
    let canonical = outcome.attempts[0].core_result.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(
        canonical
            .messages
            .last()
            .unwrap()
            .checked_model_snapshot()
            .unwrap()
            .unwrap()
            .as_assistant()
            .unwrap()
            .stop_reason,
        ara_ai::StopReason::Stop
    );
    assert_eq!(outcome.stop, WorkflowStop::NonStopAssistant);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(server.served(), 1);
}
