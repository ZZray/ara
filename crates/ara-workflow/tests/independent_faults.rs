//! Independent fault probes over the real Agent with a controlled HTTP provider.
//! These prove workflow ownership/admission contracts, not real-model capability.
mod support;

use ara_agent::{Agent, FinalProviderGate, FinalProviderGateError, FinalProviderGateStop, NullSink, RunEnd};
use ara_ai::Context;
use ara_workflow::*;
use async_trait::async_trait;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::*;
use tokio_util::sync::CancellationToken;

struct UnknownHost;
#[async_trait]
impl WorkflowHost for UnknownHost {
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        let mut value =
            receipt(candidate.basis.clone(), ReviewVerdict::Unknown { reason: "journal not reconciled".into() });
        value.settlement = Settlement::Unknown { reason: "journal not reconciled".into() };
        Ok(value)
    }
}

#[tokio::test]
async fn unknown_fence_is_recoverable_from_clone_and_release_restores_same_agent() {
    let upstream = server(vec![text("stop"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let handle = Workflow::start(
        agent.clone(),
        Arc::new(UnknownHost),
        request(),
        cfg.clone(),
        Arc::new(NullSink),
        policy(2, 8),
        CancellationToken::new(),
    )
    .unwrap();
    let retained = handle.clone();
    let first = handle.wait().await.unwrap();
    assert_eq!(first.stop, WorkflowStop::Unknown);
    drop(handle);
    let second = retained.wait().await.unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let fence = second.reconciliation.as_ref().expect("unknown effects retain fence");
    let blocked = agent
        .prompt_inputs_with_config(request().inputs, cfg.clone(), CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap();
    assert_eq!(blocked.end, RunEnd::PreModelStop);
    assert_eq!(upstream.served(), 1);
    let mut reconciled = receipt(fence.basis().clone(), ReviewVerdict::Verified);
    reconciled.basis.workflow.session_generation = "unrelated-generation".into();
    assert_eq!(fence.release(&reconciled), Err(FenceReleaseError::WrongBasis));
    reconciled.basis = fence.basis().clone();
    reconciled.commit = ReceiptCommit::Uncommitted;
    assert_eq!(fence.release(&reconciled), Err(FenceReleaseError::UnsettledOrUncommitted));
    reconciled.commit = ReceiptCommit::Committed { receipt_id: "authoritative-reconciliation".into() };
    fence.release(&reconciled).unwrap();
    let resumed = agent
        .prompt_inputs_with_config(request().inputs, cfg, CancellationToken::new(), Arc::new(NullSink))
        .await
        .unwrap();
    assert_eq!(resumed.end, RunEnd::Completed);
    assert_eq!(upstream.served(), 2);
}

struct RefuseGate;
#[async_trait]
impl FinalProviderGate for RefuseGate {
    async fn before_model_call(
        &self,
        _: &mut Context,
        _: &CancellationToken,
    ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
        Ok(Some(FinalProviderGateStop { reason: Some("downstream host veto".into()) }))
    }
}

#[tokio::test]
async fn later_gate_veto_keeps_the_workflow_request_reservation() {
    let upstream = server(vec![text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let handle = Workflow::start(
        agent.clone(),
        Arc::new(UnknownHost),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(2, 1),
        CancellationToken::new(),
    )
    .unwrap();
    // A current-thread runtime cannot poll the worker before this registration.
    // This places the refusing callback after the workflow's additive gate.
    let _downstream = agent.add_final_provider_gate(Arc::new(RefuseGate));
    let outcome = handle.wait().await.unwrap();
    assert_eq!(outcome.prepared_requests, 1);
    assert_eq!(outcome.stop, WorkflowStop::PreparedRequestLimit);
    assert_eq!(upstream.served(), 0);
    assert_eq!(outcome.attempts.len(), 1);
    assert!(outcome.reconciliation.is_some());
}

struct SubstituteDriver {
    reviews: AtomicUsize,
}
#[async_trait]
impl WorkflowHost for SubstituteDriver {
    async fn drive_run(&self, _: &AttemptBasis, run: CoreRunFuture, _: &CancellationToken) -> DriverResult {
        let mut substitute = run.await;
        if let Ok(report) = &mut substitute {
            report.end = RunEnd::Completed;
            report.messages.clear();
        }
        Ok(substitute)
    }
    async fn review(&self, _: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        panic!("driver substitute must never create a review candidate")
    }
}

#[tokio::test]
async fn driver_report_substitution_cannot_hide_a_length_terminal() {
    let upstream = server(vec![text("length")]).await;
    let cfg = config(&upstream, vec![]);
    let host = Arc::new(SubstituteDriver { reviews: AtomicUsize::new(0) });
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
    let canonical = outcome.attempts[0].core_result.as_ref().unwrap().as_ref().unwrap();
    let substitute = outcome.attempts[0].driver_result.as_ref().unwrap().as_ref().unwrap();
    assert!(!canonical.messages.is_empty());
    assert!(substitute.messages.is_empty());
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(upstream.served(), 1);
}

struct FileSettlementHost {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    effect: PathBuf,
}
#[async_trait]
impl WorkflowHost for FileSettlementHost {
    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.entered.notify_one();
        self.release.notified().await;
        // This already-owned Host effect deliberately settles after cancellation.
        std::fs::write(&self.effect, b"original-owned-effect-finished").unwrap();
        let mut value = receipt(
            candidate.basis.clone(),
            ReviewVerdict::Unknown { reason: "effect exists; journal commit lost".into() },
        );
        value.settlement = Settlement::Unknown { reason: "journal commit lost".into() };
        value.commit = ReceiptCommit::Uncommitted;
        Ok(value)
    }
}

#[tokio::test]
async fn cancelled_waiter_cannot_drop_an_owned_file_effect_or_release_its_fence() {
    let dir = tempfile::tempdir().unwrap();
    let effect = dir.path().join("effect.txt");
    let upstream = server(vec![text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let host = Arc::new(FileSettlementHost {
        entered: Default::default(),
        release: Default::default(),
        effect: effect.clone(),
    });
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
    tokio::time::timeout(Duration::from_secs(2), host.entered.notified()).await.unwrap();
    handle.cancel();
    assert!(tokio::time::timeout(Duration::from_millis(25), handle.wait()).await.is_err());
    assert!(!effect.exists());
    let retained = handle.clone();
    drop(handle);
    host.release.notify_one();
    let outcome = tokio::time::timeout(Duration::from_secs(2), retained.wait()).await.unwrap().unwrap();
    assert_eq!(std::fs::read(&effect).unwrap(), b"original-owned-effect-finished");
    assert_eq!(outcome.stop, WorkflowStop::Cancelled);
    assert!(outcome.reconciliation.is_some());
    assert_eq!(upstream.served(), 1);
    assert!(Arc::ptr_eq(&outcome, &retained.wait().await.unwrap()));
}
