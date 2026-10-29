use crate::types::*;
use ara_agent::{
    Agent, AgentConfig, AgentEventSink, AgentInput, FinalProviderGate, FinalProviderGateError, FinalProviderGateStop,
    GateRegistration, RunEnd,
};
use ara_ai::{Context, Message, StopReason, UserMessage};
use async_trait::async_trait;
use futures::FutureExt;
use serde_json::json;
use std::{
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

/// Starts an owned worker over a real Agent. The Host must exclusively own the
/// Agent and its Session generation until this worker has fully settled.
pub struct Workflow;

impl Workflow {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        agent: Arc<Agent>,
        host: Arc<dyn WorkflowHost>,
        request: WorkflowRequest,
        config: AgentConfig,
        sink: Arc<dyn AgentEventSink>,
        policy: WorkflowPolicy,
        cancel: CancellationToken,
    ) -> Result<WorkflowHandle, StartError> {
        Self::start_inner(agent, host, request, config, sink, policy, cancel, None)
    }

    /// Explicitly permit Host-settled continuation after a tool-free Length
    /// stop. Normal artifact review still requires a later normal Stop.
    #[allow(clippy::too_many_arguments)]
    pub fn start_with_length_recovery(
        agent: Arc<Agent>,
        host: Arc<dyn WorkflowHost>,
        request: WorkflowRequest,
        config: AgentConfig,
        sink: Arc<dyn AgentEventSink>,
        policy: WorkflowPolicy,
        cancel: CancellationToken,
        recovery: LengthRecoveryPolicy,
    ) -> Result<WorkflowHandle, StartError> {
        Self::start_inner(agent, host, request, config, sink, policy, cancel, Some(recovery))
    }

    #[allow(clippy::too_many_arguments)]
    fn start_inner(
        agent: Arc<Agent>,
        host: Arc<dyn WorkflowHost>,
        request: WorkflowRequest,
        mut config: AgentConfig,
        sink: Arc<dyn AgentEventSink>,
        policy: WorkflowPolicy,
        cancel: CancellationToken,
        recovery: Option<LengthRecoveryPolicy>,
    ) -> Result<WorkflowHandle, StartError> {
        for (name, value) in [
            ("workflow_id", &request.basis.workflow_id),
            ("session_id", &request.basis.session_id),
            ("session_generation", &request.basis.session_generation),
            ("criteria_revision", &request.basis.criteria_revision),
            ("artifact_evidence_basis", &request.basis.artifact_evidence_basis),
        ] {
            if value.trim().is_empty() {
                return Err(StartError::EmptyBasis(name));
            }
        }
        if request.inputs.is_empty() {
            return Err(StartError::EmptyInput);
        }
        if agent.is_busy() {
            return Err(StartError::AgentBusy);
        }
        if agent.has_queued_messages() {
            return Err(StartError::QueuedInput);
        }
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| StartError::NoTokioRuntime)?;
        let deadline = config.deadline.map_or(policy.deadline, |old| old.min(policy.deadline));
        config.deadline = Some(deadline);
        config.max_model_calls =
            Some(config.max_model_calls.unwrap_or(usize::MAX).min(policy.max_prepared_requests.get()));
        let cancel = cancel.child_token();
        let gate = Arc::new(RequestGate {
            state: Mutex::new(RequestState { closed: false, used: 0 }),
            limit: policy.max_prepared_requests.get(),
            deadline,
            cancel: cancel.clone(),
        });
        // Register before a Run can sample gate presence. Existing primary and
        // additional Host gates remain installed and keep their original order.
        let registration = agent.add_final_provider_gate(gate.clone());
        let worker_cancel = cancel.clone();
        let worker = runtime.spawn(async move {
            let _cleanup = CloseOnDrop { gate: gate.clone(), cancel: worker_cancel.clone() };
            let deadline_cancel = worker_cancel.clone();
            let timer = tokio::spawn(async move {
                tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                deadline_cancel.cancel();
            });
            let timer = TimerGuard(timer);
            let mut outcome =
                drive(agent, host, request, config, sink, policy, worker_cancel, gate.clone(), recovery).await;
            // Every original owner and Host callback has returned before this
            // point. Abnormal worker drop closes admission but keeps the gate.
            gate.close();
            let settled = outcome.attempts.last().is_none_or(attempt_settled);
            if settled {
                registration.remove();
            } else {
                let basis = outcome.attempts.last().expect("unsettled attempt").basis.clone();
                outcome.reconciliation = Some(ReconciliationFence {
                    registration: Mutex::new(Some(registration)),
                    gate: gate.clone(),
                    basis,
                });
            }
            drop(timer);
            outcome
        });
        let completion = async move { worker.await.map(Arc::new).map_err(|error| WorkflowJoinError(Arc::new(error))) }
            .boxed()
            .shared();
        Ok(WorkflowHandle { completion, cancel })
    }
}

/// A closed admission fence retained after an unproven Host settlement.
/// The Host must keep this value and supply a committed authoritative
/// reconciliation receipt. Dropping it leaves admission closed; discarding the
/// Agent releases its registry, but never reconciles or replays external effects.
#[must_use = "retain the fence until the Host has reconciled the exact Session generation"]
pub struct ReconciliationFence {
    registration: Mutex<Option<GateRegistration>>,
    gate: Arc<RequestGate>,
    basis: AttemptBasis,
}

impl std::fmt::Debug for ReconciliationFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReconciliationFence")
            .field("basis", &self.basis)
            .field("released", &self.registration.lock().unwrap_or_else(|e| e.into_inner()).is_none())
            .finish()
    }
}

impl ReconciliationFence {
    pub fn basis(&self) -> &AttemptBasis {
        &self.basis
    }

    /// The receipt must describe current reconciled Host effects and persisted
    /// evidence. A model report or an old unrelated successful receipt is not
    /// authority to release this fence. The Host is the trusted verifier.
    pub fn release(&self, receipt: &ReviewReceipt) -> Result<(), FenceReleaseError> {
        if receipt.basis != self.basis {
            return Err(FenceReleaseError::WrongBasis);
        }
        if !valid_receipt(receipt) || !matches!(receipt.settlement, Settlement::Settled) {
            return Err(FenceReleaseError::UnsettledOrUncommitted);
        }
        self.gate.close();
        if let Some(registration) = self.registration.lock().unwrap_or_else(|e| e.into_inner()).take() {
            registration.remove();
        }
        Ok(())
    }
}

/// Cloneable control and completion observer. Retain at least one handle until
/// settlement or explicit reconciliation. Dropping a wait future loses neither
/// the worker nor its outcome: later waits return the same shared outcome.
/// Dropping every handle does not cancel the worker or reopen its gate, but
/// discards in-process access to its eventual reconciliation fence. The Host
/// must persist its own receipts; there is no cross-process resume guarantee.
#[derive(Clone)]
pub struct WorkflowHandle {
    completion:
        futures::future::Shared<futures::future::BoxFuture<'static, Result<Arc<WorkflowOutcome>, WorkflowJoinError>>>,
    cancel: CancellationToken,
}

impl WorkflowHandle {
    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub async fn wait(&self) -> Result<Arc<WorkflowOutcome>, WorkflowJoinError> {
        self.completion.clone().await
    }
}

struct TimerGuard(tokio::task::JoinHandle<()>);
impl Drop for TimerGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct CloseOnDrop {
    gate: Arc<RequestGate>,
    cancel: CancellationToken,
}
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.gate.close();
        self.cancel.cancel();
    }
}

struct RequestState {
    closed: bool,
    used: usize,
}
struct RequestGate {
    state: Mutex<RequestState>,
    limit: usize,
    deadline: Instant,
    cancel: CancellationToken,
}

impl RequestGate {
    fn close(&self) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
    }
    fn used(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).used
    }
    fn external_stop(&self) -> Option<WorkflowStop> {
        if Instant::now() >= self.deadline {
            Some(WorkflowStop::Deadline)
        } else if self.cancel.is_cancelled() {
            Some(WorkflowStop::Cancelled)
        } else {
            None
        }
    }
}

#[async_trait]
impl FinalProviderGate for RequestGate {
    async fn before_model_call(
        &self,
        _context: &mut Context,
        _cancel: &CancellationToken,
    ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let reason = if state.closed {
            Some("workflow admission is closed")
        } else if Instant::now() >= self.deadline {
            Some("workflow absolute deadline reached")
        } else if self.cancel.is_cancelled() {
            Some("workflow cancelled")
        } else if state.used >= self.limit {
            Some("workflow prepared-request budget exhausted")
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Some(FinalProviderGateStop { reason: Some(reason.into()) }));
        }
        state.used += 1;
        Ok(None)
    }
}

/// Only the proxy's first poll can ask the canonical owner to start. Closing
/// this witness prevents a Host-retained future from starting a later Run.
struct ProxyState {
    open: bool,
    joined: bool,
}

/// AgentMessage clones retain shared mutable records. Pin the owner's terminal
/// admission before exposing any result alias to the Host driver. Keep the
/// original records and provenance for evidence without trusting later edits.
#[derive(Clone, Copy)]
enum TerminalAdmission {
    CoreFailed,
    NonCompleted,
    NonStopAssistant,
    ToolFreeLength,
    Reviewable,
}

struct CanonicalRunReceipt {
    result: CoreRunResult,
    admission: TerminalAdmission,
}

impl CanonicalRunReceipt {
    fn seal(result: CoreRunResult) -> Self {
        let admission = match &result {
            Err(_) => TerminalAdmission::CoreFailed,
            Ok(report) if report.end != RunEnd::Completed => TerminalAdmission::NonCompleted,
            Ok(report) => terminal_assistant(report),
        };
        Self { result, admission }
    }
}

fn canonical_run(
    agent: Arc<Agent>,
    inputs: Vec<AgentInput>,
    config: AgentConfig,
    sink: Arc<dyn AgentEventSink>,
    cancel: CancellationToken,
) -> (CoreRunFuture, tokio::task::JoinHandle<Option<CanonicalRunReceipt>>, Arc<Mutex<ProxyState>>, CancellationToken) {
    let state = Arc::new(Mutex::new(ProxyState { open: true, joined: false }));
    let closed = CancellationToken::new();
    let (start, admission) = oneshot::channel();
    let (result_tx, mut result_rx) = watch::channel::<Option<CoreRunResult>>(None);
    let owner_closed = closed.clone();
    let owner = tokio::spawn(async move {
        let admitted = tokio::select! {
            biased;
            _ = owner_closed.cancelled() => false,
            signal = admission => signal.is_ok(),
        };
        if !admitted {
            return None;
        }
        let result =
            match AssertUnwindSafe(async { agent.prompt_inputs_with_config(inputs, config, cancel, sink).await })
                .catch_unwind()
                .await
            {
                Ok(result) => result.map_err(|error| CoreRunFailure::Agent(Arc::new(error))),
                Err(panic) => Err(CoreRunFailure::OwnerPanicked(panic_text(panic))),
            };
        let receipt = CanonicalRunReceipt::seal(result);
        result_tx.send_replace(Some(receipt.result.clone()));
        Some(receipt)
    });
    let proxy_state = state.clone();
    let proxy = Box::pin(async move {
        {
            let state = proxy_state.lock().unwrap_or_else(|e| e.into_inner());
            if !state.open {
                return Err(CoreRunFailure::AdmissionClosed);
            }
            // This non-awaiting send is ordered by the same lock as close.
            if start.send(()).is_err() {
                return Err(CoreRunFailure::AdmissionClosed);
            }
        }
        loop {
            let result = result_rx.borrow().clone();
            if let Some(result) = result {
                proxy_state.lock().unwrap_or_else(|e| e.into_inner()).joined = true;
                return result;
            }
            if result_rx.changed().await.is_err() {
                return Err(CoreRunFailure::AdmissionClosed);
            }
        }
    });
    (proxy, owner, state, closed)
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    agent: Arc<Agent>,
    host: Arc<dyn WorkflowHost>,
    request: WorkflowRequest,
    config: AgentConfig,
    sink: Arc<dyn AgentEventSink>,
    policy: WorkflowPolicy,
    cancel: CancellationToken,
    gate: Arc<RequestGate>,
    recovery: Option<LengthRecoveryPolicy>,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome {
        basis: request.basis,
        stop: WorkflowStop::Unknown,
        attempts: vec![],
        prepared_requests: 0,
        reconciliation: None,
    };
    let mut inputs = request.inputs;
    let mut length_continuations = 0;
    for attempt in 1..=policy.max_attempts.get() {
        if let Some(stop) = gate.external_stop() {
            outcome.stop = stop;
            break;
        }
        if gate.used() >= gate.limit {
            outcome.stop = WorkflowStop::PreparedRequestLimit;
            break;
        }
        if agent.is_busy() {
            outcome.stop = WorkflowStop::AgentBusy;
            break;
        }
        if agent.has_queued_messages() {
            outcome.stop = WorkflowStop::QueuedInput;
            break;
        }
        let basis = AttemptBasis { workflow: outcome.basis.clone(), attempt };
        let (run, owner, witness, closed) =
            canonical_run(agent.clone(), inputs, config.clone(), sink.clone(), cancel.clone());
        let driver_result =
            match AssertUnwindSafe(async { host.drive_run(&basis, run, &cancel).await }).catch_unwind().await {
                Ok(result) => result,
                Err(panic) => Err(HostDriverError { kind: HostDriverErrorKind::Panicked, message: panic_text(panic) }),
            };
        let joined = {
            let mut state = witness.lock().unwrap_or_else(|e| e.into_inner());
            state.open = false;
            state.joined
        };
        closed.cancel();
        if !joined || driver_result.is_err() {
            gate.close();
            cancel.cancel();
        }
        // Never detach/drop the canonical Agent future after a Host violation.
        // An uncooperative admitted effect keeps this wait pending and gate shut.
        let (core_result, admission) = match owner.await {
            Ok(Some(receipt)) => (Some(receipt.result), receipt.admission),
            Ok(None) => (None, TerminalAdmission::CoreFailed),
            Err(error) => (Some(Err(CoreRunFailure::OwnerPanicked(error.to_string()))), TerminalAdmission::CoreFailed),
        };
        let record =
            AttemptRecord { basis: basis.clone(), core_result, driver_result, review: None, length_settlement: None };
        outcome.attempts.push(record);
        let record = outcome.attempts.last_mut().expect("just appended attempt");
        if !joined {
            outcome.stop = WorkflowStop::DriverDidNotJoin;
            break;
        }
        if record.driver_result.is_err() {
            outcome.stop = WorkflowStop::DriverFailed;
            break;
        }
        let report = match &record.core_result {
            Some(Ok(report)) => report,
            _ => {
                outcome.stop = WorkflowStop::CoreFailed;
                break;
            }
        };
        if let Some(stop) = gate.external_stop() {
            outcome.stop = stop;
            break;
        }
        if matches!(admission, TerminalAdmission::NonCompleted) {
            outcome.stop = if gate.used() >= gate.limit {
                WorkflowStop::PreparedRequestLimit
            } else {
                WorkflowStop::NonCompletedRun
            };
            break;
        }
        if matches!(admission, TerminalAdmission::ToolFreeLength)
            && let Some(recovery) = &recovery
        {
            if agent.has_queued_messages() {
                outcome.stop = WorkflowStop::QueuedInput;
                break;
            }
            if gate.used() >= gate.limit {
                outcome.stop = WorkflowStop::PreparedRequestLimit;
                break;
            }
            if length_continuations >= recovery.max_continuations.get() {
                outcome.stop = WorkflowStop::LengthContinuationLimit;
                break;
            }
            if attempt == policy.max_attempts.get() {
                outcome.stop = WorkflowStop::AttemptLimit;
                break;
            }
            let stopped = LengthStoppedRun { basis: basis.clone(), run: report.clone() };
            record.length_settlement = Some(
                match AssertUnwindSafe(async { host.settle_length_stop(&stopped, &cancel).await }).catch_unwind().await
                {
                    Ok(result) => result,
                    Err(panic) => Err(ReviewError(format!("Host length settlement panicked: {}", panic_text(panic)))),
                },
            );
            if let Some(stop) = gate.external_stop() {
                outcome.stop = stop;
                break;
            }
            let receipt = match record.length_settlement.as_ref().expect("length settlement retained") {
                Ok(receipt) => receipt,
                Err(_) => {
                    outcome.stop = WorkflowStop::LengthSettlementFailed;
                    break;
                }
            };
            if receipt.basis != basis || !valid_length_receipt(receipt) {
                outcome.stop = WorkflowStop::InvalidReceipt;
                break;
            }
            if !matches!(receipt.settlement, Settlement::Settled) {
                outcome.stop = WorkflowStop::Unknown;
                break;
            }
            let Some(feedback) = &receipt.continuation else {
                outcome.stop = WorkflowStop::NonStopAssistant;
                break;
            };
            if gate.used() >= gate.limit {
                outcome.stop = WorkflowStop::PreparedRequestLimit;
                break;
            }
            inputs = vec![length_feedback_input(receipt, feedback)];
            length_continuations += 1;
            continue;
        }
        if !matches!(admission, TerminalAdmission::Reviewable) {
            outcome.stop = WorkflowStop::NonStopAssistant;
            break;
        }
        if agent.has_queued_messages() {
            outcome.stop = WorkflowStop::QueuedInput;
            break;
        }
        let candidate = ReviewCandidate { basis: basis.clone(), run: report.clone() };
        let review = match AssertUnwindSafe(async { host.review(&candidate, &cancel).await }).catch_unwind().await {
            Ok(result) => result,
            Err(panic) => Err(ReviewError(format!("Host review panicked: {}", panic_text(panic)))),
        };
        record.review = Some(review);
        if let Some(stop) = gate.external_stop() {
            outcome.stop = stop;
            break;
        }
        let receipt = match record.review.as_ref().expect("review retained") {
            Ok(receipt) => receipt,
            Err(_) => {
                outcome.stop = WorkflowStop::ReviewFailed;
                break;
            }
        };
        if receipt.basis != basis || !valid_receipt(receipt) {
            outcome.stop = WorkflowStop::InvalidReceipt;
            break;
        }
        if !matches!(receipt.settlement, Settlement::Settled) {
            outcome.stop = WorkflowStop::Unknown;
            break;
        }
        match &receipt.verdict {
            ReviewVerdict::Verified => {
                outcome.stop = WorkflowStop::VerifiedArtifact;
                break;
            }
            ReviewVerdict::Unknown { .. } => {
                outcome.stop = WorkflowStop::Unknown;
                break;
            }
            ReviewVerdict::NeedsChanges { feedback } => {
                if feedback.trim().is_empty() {
                    outcome.stop = WorkflowStop::InvalidReceipt;
                    break;
                }
                if attempt == policy.max_attempts.get() {
                    outcome.stop = WorkflowStop::AttemptLimit;
                    break;
                }
                inputs = vec![feedback_input(receipt, feedback)];
            }
        }
    }
    outcome.prepared_requests = gate.used();
    outcome
}

fn terminal_assistant(report: &ara_agent::RunReport) -> TerminalAdmission {
    // Raw/Graph bodies that cannot reliably expose the last Assistant are not
    // inferred from text, JSON stop strings, or a driver's substitute report.
    for record in report.messages.iter().rev() {
        match record.checked_model_snapshot() {
            Ok(Some(Message::Assistant(message))) => {
                return match message.stop_reason {
                    StopReason::Stop => TerminalAdmission::Reviewable,
                    StopReason::Length if message.tool_calls().next().is_none() && message.error_message.is_none() => {
                        TerminalAdmission::ToolFreeLength
                    }
                    _ => TerminalAdmission::NonStopAssistant,
                };
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => return TerminalAdmission::NonStopAssistant,
        }
    }
    TerminalAdmission::NonStopAssistant
}

fn attempt_settled(attempt: &AttemptRecord) -> bool {
    matches!(&attempt.review, Some(Ok(receipt))
        if receipt.basis == attempt.basis && valid_receipt(receipt)
        && matches!(receipt.settlement, Settlement::Settled))
        || matches!(&attempt.length_settlement, Some(Ok(receipt))
            if receipt.basis == attempt.basis && valid_length_receipt(receipt)
            && matches!(receipt.settlement, Settlement::Settled))
}

fn valid_length_receipt(receipt: &LengthSettlementReceipt) -> bool {
    valid_receipt_fields(&receipt.commit, &receipt.artifacts)
        && receipt.continuation.as_ref().is_none_or(|text| !text.trim().is_empty())
}

fn valid_receipt(receipt: &ReviewReceipt) -> bool {
    valid_receipt_fields(&receipt.commit, &receipt.artifacts)
}

fn valid_receipt_fields(commit: &ReceiptCommit, artifacts: &[ArtifactEvidence]) -> bool {
    matches!(commit, ReceiptCommit::Committed { receipt_id } if !receipt_id.trim().is_empty())
        && !artifacts.is_empty()
        && artifacts.iter().all(|artifact| {
            !artifact.artifact_id.trim().is_empty()
                && !artifact.revision.trim().is_empty()
                && !artifact.evidence_ref.trim().is_empty()
        })
}

fn length_feedback_input(receipt: &LengthSettlementReceipt, feedback: &str) -> AgentInput {
    let ReceiptCommit::Committed { receipt_id } = &receipt.commit else { unreachable!("validated receipt") };
    AgentInput {
        model: Message::User(UserMessage::text(feedback)),
        provenance: Some(Arc::new(json!({
            "kind": "ara-workflow-length-continuation", "basis": receipt.basis,
            "receipt_id": receipt_id, "artifacts": receipt.artifacts,
            "previous_stop": "length",
        }))),
    }
}

fn feedback_input(receipt: &ReviewReceipt, feedback: &str) -> AgentInput {
    let receipt_id = match &receipt.commit {
        ReceiptCommit::Committed { receipt_id } => receipt_id,
        ReceiptCommit::Uncommitted => unreachable!("validated receipt"),
    };
    AgentInput {
        model: Message::User(UserMessage::text(feedback)),
        provenance: Some(Arc::new(json!({
            "kind": "ara-workflow-review", "basis": receipt.basis,
            "receipt_id": receipt_id, "artifacts": receipt.artifacts,
        }))),
    }
}

fn panic_text(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|value| (*value).into()))
        .unwrap_or_else(|| "non-string panic".into())
}
