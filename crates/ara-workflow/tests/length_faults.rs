//! Independent Length-recovery attacks over the real Agent and HTTP provider.
//! The upstream and Host receipts are controlled; these are not model-capability trials.
mod support;

use ara_agent::{Agent, AgentConfig, AgentMessageBody, NullSink, RunEnd};
use ara_ai::{Message, StopReason};
use ara_workflow::*;
use async_trait::async_trait;
use std::{
    num::NonZeroUsize,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use support::*;
use tokio_util::sync::CancellationToken;

fn length_receipt(basis: AttemptBasis, continuation: Option<&str>) -> LengthSettlementReceipt {
    let checked = receipt(basis, ReviewVerdict::Verified);
    LengthSettlementReceipt {
        basis: checked.basis,
        artifacts: checked.artifacts,
        settlement: checked.settlement,
        commit: checked.commit,
        continuation: continuation.map(str::to_owned),
    }
}

struct ProbeHost {
    mode: &'static str,
    rewrite: Option<StopReason>,
    repair: bool,
    reviews: AtomicUsize,
    settlements: AtomicUsize,
}

impl ProbeHost {
    fn new(mode: &'static str, rewrite: Option<StopReason>, repair: bool) -> Arc<Self> {
        Arc::new(Self { mode, rewrite, repair, reviews: AtomicUsize::new(0), settlements: AtomicUsize::new(0) })
    }
}

#[async_trait]
impl WorkflowHost for ProbeHost {
    async fn drive_run(&self, _: &AttemptBasis, run: CoreRunFuture, _: &CancellationToken) -> DriverResult {
        let result = run.await;
        if let (Some(rewrite), Ok(report)) = (&self.rewrite, &result) {
            for record in report.messages.iter().rev() {
                if let Some(Message::Assistant(mut message)) = record.checked_model_snapshot().unwrap() {
                    message.stop_reason = *rewrite;
                    record.replace_body(AgentMessageBody::Model(Message::Assistant(message))).unwrap();
                    break;
                }
            }
        }
        Ok(result)
    }

    async fn review(&self, candidate: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        Ok(receipt(
            candidate.basis.clone(),
            if self.repair {
                ReviewVerdict::NeedsChanges { feedback: "ordinary review found an artifact mismatch".into() }
            } else {
                ReviewVerdict::Verified
            },
        ))
    }

    async fn settle_length_stop(
        &self,
        stopped: &LengthStoppedRun,
        _: &CancellationToken,
    ) -> Result<LengthSettlementReceipt, ReviewError> {
        self.settlements.fetch_add(1, Ordering::SeqCst);
        let mut checked = length_receipt(stopped.basis.clone(), Some("continue from the committed length boundary"));
        match self.mode {
            "no-continuation" => checked.continuation = None,
            "wrong-basis" => checked.basis.workflow.session_generation = "unrelated-generation".into(),
            "uncommitted" => checked.commit = ReceiptCommit::Uncommitted,
            "blank-receipt-id" => checked.commit = ReceiptCommit::Committed { receipt_id: " \t".into() },
            "empty-feedback" => checked.continuation = Some(" \n\t".into()),
            "empty-artifacts" => checked.artifacts.clear(),
            "empty-artifact-field" => checked.artifacts[0].revision.clear(),
            "unknown" => checked.settlement = Settlement::Unknown { reason: "persistence receipt lost".into() },
            "error" => return Err(ReviewError("controlled settlement error".into())),
            "panic" => panic!("controlled length-settlement panic"),
            "continue" => {}
            other => panic!("unknown test mode: {other}"),
        }
        Ok(checked)
    }
}

fn start_recovery(
    agent: Arc<Agent>,
    cfg: AgentConfig,
    host: Arc<dyn WorkflowHost>,
    bounds: (usize, usize, usize),
) -> WorkflowHandle {
    Workflow::start_with_length_recovery(
        agent,
        host,
        request(),
        cfg,
        Arc::new(NullSink),
        policy(bounds.0, bounds.1),
        CancellationToken::new(),
        LengthRecoveryPolicy { max_continuations: NonZeroUsize::new(bounds.2).unwrap() },
    )
    .unwrap()
}

async fn finished(handle: &WorkflowHandle) -> Arc<WorkflowOutcome> {
    tokio::time::timeout(Duration::from_secs(2), handle.wait()).await.expect("bounded workflow did not settle").unwrap()
}

async fn assert_agent_is_fenced(agent: &Arc<Agent>, cfg: AgentConfig) {
    let blocked = tokio::time::timeout(
        Duration::from_secs(2),
        agent.prompt_inputs_with_config(request().inputs, cfg, CancellationToken::new(), Arc::new(NullSink)),
    )
    .await
    .expect("closed admission must not hang")
    .unwrap();
    assert_eq!(blocked.end, RunEnd::PreModelStop);
}

fn visible_terminal(attempt: &AttemptRecord) -> StopReason {
    let canonical = attempt.core_result.as_ref().unwrap().as_ref().unwrap();
    canonical
        .messages
        .iter()
        .rev()
        .find_map(|record| match record.checked_model_snapshot().unwrap() {
            Some(Message::Assistant(message)) => Some(message.stop_reason),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn ordinary_start_never_calls_opt_in_length_settlement() {
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let host = ProbeHost::new("continue", None, false);
    let handle = Workflow::start(
        Agent::new(cfg.clone(), vec![]),
        host.clone(),
        request(),
        cfg,
        Arc::new(NullSink),
        policy(3, 8),
        CancellationToken::new(),
    )
    .unwrap();
    let outcome = finished(&handle).await;
    assert_eq!(outcome.stop, WorkflowStop::NonStopAssistant);
    assert_eq!(host.settlements.load(Ordering::SeqCst), 0);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert!(outcome.attempts[0].length_settlement.is_none());
    assert_eq!(outcome.prepared_requests, 1);
    assert_eq!(upstream.served(), 1);
}

#[tokio::test]
async fn driver_cannot_forge_length_recovery_by_mutating_a_canonical_stop_alias() {
    let upstream = server(vec![text("stop"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let host = ProbeHost::new("continue", Some(StopReason::Length), false);
    let handle = start_recovery(Agent::new(cfg.clone(), vec![]), cfg, host.clone(), (3, 8, 2));
    let outcome = finished(&handle).await;
    assert_eq!(
        visible_terminal(&outcome.attempts[0]),
        StopReason::Length,
        "attack must mutate the canonical shared alias"
    );
    assert_eq!(
        outcome.stop,
        WorkflowStop::VerifiedArtifact,
        "the owner's original Stop still requires ordinary review"
    );
    assert_eq!(host.settlements.load(Ordering::SeqCst), 0);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 1);
    assert!(outcome.attempts[0].length_settlement.is_none());
    assert!(outcome.attempts[0].review.is_some());
    assert_eq!(upstream.served(), 1);
}

#[tokio::test]
async fn driver_cannot_promote_an_original_length_to_artifact_review_by_mutating_its_alias() {
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let host = ProbeHost::new("no-continuation", Some(StopReason::Stop), false);
    let handle = start_recovery(Agent::new(cfg.clone(), vec![]), cfg, host.clone(), (3, 8, 2));
    let outcome = finished(&handle).await;
    assert_eq!(
        visible_terminal(&outcome.attempts[0]),
        StopReason::Stop,
        "attack must mutate the canonical shared alias"
    );
    assert_eq!(outcome.stop, WorkflowStop::NonStopAssistant);
    assert_eq!(host.settlements.load(Ordering::SeqCst), 1);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert!(outcome.attempts[0].length_settlement.is_some());
    assert!(outcome.attempts[0].review.is_none());
    assert!(
        outcome.reconciliation.is_none(),
        "a valid no-continuation receipt settles, but never verifies, the Length Run"
    );
    assert_eq!(upstream.served(), 1);
}

#[tokio::test]
async fn malformed_unknown_and_failed_length_receipts_never_reopen_provider_admission() {
    for (mode, expected) in [
        ("wrong-basis", WorkflowStop::InvalidReceipt),
        ("uncommitted", WorkflowStop::InvalidReceipt),
        ("blank-receipt-id", WorkflowStop::InvalidReceipt),
        ("empty-feedback", WorkflowStop::InvalidReceipt),
        ("empty-artifacts", WorkflowStop::InvalidReceipt),
        ("empty-artifact-field", WorkflowStop::InvalidReceipt),
        ("unknown", WorkflowStop::Unknown),
        ("error", WorkflowStop::LengthSettlementFailed),
        ("panic", WorkflowStop::LengthSettlementFailed),
    ] {
        let upstream = server(vec![text("length"), text("stop")]).await;
        let cfg = config(&upstream, vec![]);
        let agent = Agent::new(cfg.clone(), vec![]);
        let host = ProbeHost::new(mode, None, false);
        let handle = start_recovery(agent.clone(), cfg.clone(), host.clone(), (3, 8, 2));
        let outcome = finished(&handle).await;
        assert_eq!(outcome.stop, expected, "mode {mode}");
        assert_eq!(outcome.attempts.len(), 1, "mode {mode}");
        assert_eq!(outcome.prepared_requests, 1, "mode {mode}");
        assert_eq!(host.settlements.load(Ordering::SeqCst), 1, "mode {mode}");
        assert_eq!(host.reviews.load(Ordering::SeqCst), 0, "mode {mode}");
        assert!(outcome.attempts[0].length_settlement.is_some(), "mode {mode}");
        assert!(outcome.attempts[0].review.is_none(), "mode {mode}");
        let fence = outcome.reconciliation.as_ref().expect("unproven Length settlement must retain its fence");
        assert_eq!(fence.basis(), &outcome.attempts[0].basis);
        assert_agent_is_fenced(&agent, cfg).await;
        assert_eq!(upstream.served(), 1, "even direct Agent reuse must remain fenced for {mode}");
    }
}

struct BlockingLengthHost {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    effect: PathBuf,
    observed_cancellation: AtomicBool,
    reviews: AtomicUsize,
}

#[async_trait]
impl WorkflowHost for BlockingLengthHost {
    async fn review(&self, _: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        panic!("cancelled Length settlement cannot become ordinary review")
    }

    async fn settle_length_stop(
        &self,
        stopped: &LengthStoppedRun,
        cancel: &CancellationToken,
    ) -> Result<LengthSettlementReceipt, ReviewError> {
        self.entered.notify_one();
        self.release.notified().await;
        self.observed_cancellation.store(cancel.is_cancelled(), Ordering::SeqCst);
        std::fs::write(&self.effect, b"original-owned-length-effect-finished").unwrap();
        let mut checked = length_receipt(stopped.basis.clone(), Some("must not continue after cancellation"));
        checked.settlement = Settlement::Unknown { reason: "effect completed but journal commit is unproven".into() };
        checked.commit = ReceiptCommit::Uncommitted;
        Ok(checked)
    }
}

#[tokio::test]
async fn cancellation_and_dropped_waiter_join_owned_length_effect_and_keep_unknown_fence() {
    let dir = tempfile::tempdir().unwrap();
    let effect = dir.path().join("length-effect.txt");
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let host = Arc::new(BlockingLengthHost {
        entered: Default::default(),
        release: Default::default(),
        effect: effect.clone(),
        observed_cancellation: AtomicBool::new(false),
        reviews: AtomicUsize::new(0),
    });
    let handle = start_recovery(agent.clone(), cfg.clone(), host.clone(), (3, 8, 2));
    tokio::time::timeout(Duration::from_secs(2), host.entered.notified()).await.unwrap();
    handle.cancel();
    assert!(tokio::time::timeout(Duration::from_millis(20), handle.wait()).await.is_err());
    assert!(!effect.exists(), "cancellation must not run or abandon the still-owned effect early");
    let retained = handle.clone();
    drop(handle);
    host.release.notify_one();
    let outcome = finished(&retained).await;
    assert_eq!(std::fs::read(&effect).unwrap(), b"original-owned-length-effect-finished");
    assert!(host.observed_cancellation.load(Ordering::SeqCst));
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(outcome.stop, WorkflowStop::Cancelled);
    assert!(outcome.reconciliation.is_some());
    let checked = outcome.attempts[0].length_settlement.as_ref().unwrap().as_ref().unwrap();
    assert!(matches!(checked.settlement, Settlement::Unknown { .. }));
    assert_eq!(checked.commit, ReceiptCommit::Uncommitted);
    assert_agent_is_fenced(&agent, cfg).await;
    assert_eq!(upstream.served(), 1);
    assert!(Arc::ptr_eq(&outcome, &finished(&retained).await));
}

#[tokio::test]
async fn length_recovery_attempt_and_request_limits_each_close_before_another_settlement() {
    for (bounds, expected) in [
        ((5, 8, 1), WorkflowStop::LengthContinuationLimit),
        ((2, 8, 5), WorkflowStop::AttemptLimit),
        ((5, 2, 5), WorkflowStop::PreparedRequestLimit),
    ] {
        let upstream = server(vec![text("length"), text("length"), text("stop")]).await;
        let cfg = config(&upstream, vec![]);
        let agent = Agent::new(cfg.clone(), vec![]);
        let host = ProbeHost::new("continue", None, false);
        let handle = start_recovery(agent.clone(), cfg.clone(), host.clone(), bounds);
        let outcome = finished(&handle).await;
        assert_eq!(outcome.stop, expected);
        assert_eq!(outcome.prepared_requests, 2);
        assert_eq!(outcome.attempts.len(), 2);
        assert_eq!(host.settlements.load(Ordering::SeqCst), 1);
        assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
        assert!(outcome.attempts[0].length_settlement.is_some());
        assert!(outcome.attempts[1].length_settlement.is_none());
        assert!(outcome.reconciliation.is_some(), "the final unreviewed Length still needs authoritative settlement");
        assert_agent_is_fenced(&agent, cfg).await;
        assert_eq!(upstream.served(), 2);
    }
}

#[tokio::test]
async fn ordinary_needs_changes_does_not_reset_length_or_request_allowances() {
    for (bounds, expected) in [
        ((5, 8, 1), WorkflowStop::LengthContinuationLimit),
        ((5, 3, 5), WorkflowStop::PreparedRequestLimit),
        ((3, 8, 5), WorkflowStop::AttemptLimit),
    ] {
        let upstream = server(vec![text("length"), text("stop"), text("length"), text("stop")]).await;
        let cfg = config(&upstream, vec![]);
        let host = ProbeHost::new("continue", None, true);
        let handle = start_recovery(Agent::new(cfg.clone(), vec![]), cfg, host.clone(), bounds);
        let outcome = finished(&handle).await;
        assert_eq!(outcome.stop, expected);
        assert_eq!(outcome.prepared_requests, 3);
        assert_eq!(outcome.attempts.len(), 3);
        assert_eq!(host.settlements.load(Ordering::SeqCst), 1);
        assert_eq!(host.reviews.load(Ordering::SeqCst), 1);
        assert!(outcome.attempts[0].length_settlement.is_some());
        assert!(outcome.attempts[1].review.is_some());
        assert!(outcome.attempts[2].length_settlement.is_none());
        assert!(outcome.attempts.iter().all(|attempt| attempt.basis.workflow == outcome.basis));
        assert_eq!(upstream.served(), 3);
    }
}

#[tokio::test]
async fn settled_length_continues_same_session_with_source_then_requires_a_real_stop_review() {
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let host = ProbeHost::new("continue", None, false);
    let handle = start_recovery(Agent::new(cfg.clone(), vec![]), cfg, host.clone(), (3, 8, 2));
    let outcome = finished(&handle).await;
    assert_eq!(outcome.stop, WorkflowStop::VerifiedArtifact);
    assert_eq!(outcome.attempts.len(), 2);
    assert_eq!(outcome.prepared_requests, 2);
    assert_eq!(host.settlements.load(Ordering::SeqCst), 1);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 1);
    assert!(outcome.attempts[0].review.is_none());
    assert!(outcome.attempts[1].length_settlement.is_none());
    assert!(outcome.reconciliation.is_none());
    assert_eq!(visible_terminal(&outcome.attempts[0]), StopReason::Length);
    assert_eq!(visible_terminal(&outcome.attempts[1]), StopReason::Stop);
    let first = &outcome.attempts[0];
    let second = &outcome.attempts[1];
    assert_eq!(first.basis.workflow, second.basis.workflow);
    let resumed = second.core_result.as_ref().unwrap().as_ref().unwrap();
    let source = resumed
        .messages
        .iter()
        .filter_map(|record| record.source())
        .find(|source| source["kind"] == "ara-workflow-length-continuation")
        .expect("continuation must retain the Host receipt provenance");
    assert_eq!(source["basis"], serde_json::to_value(&first.basis).unwrap());
    assert_eq!(source["receipt_id"], "fixture-receipt");
    assert_eq!(source["previous_stop"], "length");
    assert_eq!(source["artifacts"][0]["artifact_id"], "answer");
    let requests = upstream.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["body"]["messages"].as_array().unwrap();
    assert!(messages.iter().any(
        |message| message["role"] == "user" && message["content"] == "continue from the committed length boundary"
    ));
}

struct CountingNativeWrite {
    inner: ara_tools::write::WriteTool,
    executions: AtomicUsize,
}

#[async_trait]
impl ara_agent::AgentTool for CountingNativeWrite {
    fn definition(&self) -> ara_ai::Tool {
        ara_agent::AgentTool::definition(&self.inner)
    }

    async fn execute(
        &self,
        id: &str,
        args: ara_ai::JsonObject,
        cancel: CancellationToken,
        update: ara_agent::UpdateFn,
    ) -> Result<ara_agent::ToolOutput, ara_agent::ToolError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        ara_agent::AgentTool::execute(&self.inner, id, args, cancel, update).await
    }
}

#[tokio::test]
async fn truncated_length_tool_call_stays_in_core_budget_path_without_execution_or_length_settlement() {
    use ara_testkit::chunks;
    let dir = tempfile::tempdir().unwrap();
    let artifact = dir.path().join("answer.txt");
    std::fs::write(&artifact, b"original-native-artifact").unwrap();
    let tool = Arc::new(CountingNativeWrite {
        inner: ara_tools::write::WriteTool::new(ara_tools::ToolContext::new(dir.path())),
        executions: AtomicUsize::new(0),
    });
    let upstream = server(vec![
        serde_json::json!({"events":[
            chunks::tool_call(0,"truncated-write","write",r#"{"path":"answer.txt","content":"truncated replacement"#),
            chunks::finish("length"),chunks::done()
        ]}),
        text("stop"),
    ])
    .await;
    let mut cfg = config(&upstream, vec![tool.clone()]);
    cfg.max_model_calls = Some(1);
    let host = ProbeHost::new("continue", None, false);
    let handle = start_recovery(Agent::new(cfg.clone(), vec![]), cfg, host.clone(), (3, 8, 2));
    let outcome = finished(&handle).await;
    assert_eq!(outcome.stop, WorkflowStop::NonCompletedRun);
    assert_eq!(outcome.attempts.len(), 1);
    let canonical = outcome.attempts[0].core_result.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(canonical.end, RunEnd::ModelCallBudget);
    assert!(canonical.messages.iter().any(|record| matches!(
        record.checked_model_snapshot().unwrap(),
        Some(Message::Assistant(message)) if message.stop_reason == StopReason::Length && message.tool_calls().count() == 1
    )), "controlled HTTP response must retain its original truncated Length tool call");
    assert!(canonical.messages.iter().any(|record| matches!(
        record.checked_model_snapshot().unwrap(),
        Some(Message::ToolResult(result)) if result.is_error
            && result.details.as_ref().is_some_and(|details| details["source"] == "assistant_stop_length" && details["executed"] == false)
    )), "Core must preserve its synthetic nonexecution receipt instead of replaying truncated arguments");
    assert_eq!(tool.executions.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&artifact).unwrap(), b"original-native-artifact");
    assert_eq!(host.settlements.load(Ordering::SeqCst), 0);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert!(outcome.attempts[0].length_settlement.is_none());
    assert_eq!(outcome.prepared_requests, 1);
    assert_eq!(upstream.served(), 1);
}

struct DeadlineLengthHost {
    effect: PathBuf,
    entered: AtomicBool,
    joined: AtomicBool,
    reviews: AtomicUsize,
}

#[async_trait]
impl WorkflowHost for DeadlineLengthHost {
    async fn review(&self, _: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        panic!("a deadline-expired Length callback must not become artifact review")
    }

    async fn settle_length_stop(
        &self,
        stopped: &LengthStoppedRun,
        cancel: &CancellationToken,
    ) -> Result<LengthSettlementReceipt, ReviewError> {
        self.entered.store(true, Ordering::SeqCst);
        cancel.cancelled().await;
        // Complete this already-owned Host effect after the original deadline.
        std::fs::write(&self.effect, b"length-settlement-joined-after-absolute-deadline").unwrap();
        self.joined.store(true, Ordering::SeqCst);
        Ok(length_receipt(stopped.basis.clone(), Some("must not receive a fresh deadline")))
    }
}

#[tokio::test]
async fn original_absolute_deadline_cancels_and_joins_length_settlement_without_granting_fresh_time() {
    let dir = tempfile::tempdir().unwrap();
    let effect = dir.path().join("deadline-effect.txt");
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let host = Arc::new(DeadlineLengthHost {
        effect: effect.clone(),
        entered: AtomicBool::new(false),
        joined: AtomicBool::new(false),
        reviews: AtomicUsize::new(0),
    });
    let mut bounds = policy(3, 8);
    let absolute_deadline = std::time::Instant::now() + Duration::from_millis(50);
    bounds.deadline = absolute_deadline;
    let handle = Workflow::start_with_length_recovery(
        agent,
        host.clone(),
        request(),
        cfg,
        Arc::new(NullSink),
        bounds,
        CancellationToken::new(),
        LengthRecoveryPolicy { max_continuations: NonZeroUsize::new(2).unwrap() },
    )
    .unwrap();
    let outcome = finished(&handle).await;
    assert!(host.entered.load(Ordering::SeqCst), "deadline must expire inside the admitted Host callback");
    assert!(host.joined.load(Ordering::SeqCst), "deadline cancellation must join the original callback");
    assert!(std::time::Instant::now() >= absolute_deadline);
    assert_eq!(std::fs::read(&effect).unwrap(), b"length-settlement-joined-after-absolute-deadline");
    assert_eq!(outcome.stop, WorkflowStop::Deadline);
    assert_eq!(outcome.attempts.len(), 1);
    let checked = outcome.attempts[0].length_settlement.as_ref().unwrap().as_ref().unwrap();
    assert_eq!(checked.basis, outcome.attempts[0].basis);
    assert_eq!(checked.settlement, Settlement::Settled);
    assert!(matches!(checked.commit, ReceiptCommit::Committed { .. }));
    assert_eq!(checked.continuation.as_deref(), Some("must not receive a fresh deadline"));
    assert!(
        outcome.reconciliation.is_none(),
        "the joined valid receipt settles effects even though admission remains expired"
    );
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(outcome.prepared_requests, 1);
    assert_eq!(upstream.served(), 1);
}

struct NoLengthSupportHost {
    reviews: AtomicUsize,
}

#[async_trait]
impl WorkflowHost for NoLengthSupportHost {
    async fn review(&self, _: &ReviewCandidate, _: &CancellationToken) -> Result<ReviewReceipt, ReviewError> {
        self.reviews.fetch_add(1, Ordering::SeqCst);
        panic!("default unsupported Length settlement must not become artifact review")
    }
}

#[tokio::test]
async fn opt_in_without_host_length_support_fails_closed_instead_of_implicitly_continuing() {
    let upstream = server(vec![text("length"), text("stop")]).await;
    let cfg = config(&upstream, vec![]);
    let agent = Agent::new(cfg.clone(), vec![]);
    let host = Arc::new(NoLengthSupportHost { reviews: AtomicUsize::new(0) });
    let handle = start_recovery(agent.clone(), cfg.clone(), host.clone(), (3, 8, 2));
    let outcome = finished(&handle).await;
    assert_eq!(outcome.stop, WorkflowStop::LengthSettlementFailed);
    assert_eq!(outcome.attempts.len(), 1);
    let error = outcome.attempts[0].length_settlement.as_ref().unwrap().as_ref().unwrap_err();
    assert!(error.0.contains("does not support length-stop settlement"));
    assert!(outcome.attempts[0].review.is_none());
    assert!(outcome.reconciliation.is_some());
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_agent_is_fenced(&agent, cfg).await;
    assert_eq!(outcome.prepared_requests, 1);
    assert_eq!(upstream.served(), 1);
}
