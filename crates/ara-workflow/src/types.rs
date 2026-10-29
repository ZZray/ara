use ara_agent::{AgentError, AgentInput, RunReport};
use async_trait::async_trait;
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::{num::NonZeroUsize, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

/// Host-issued identity. The Host must exclusively own this Session generation
/// for the complete workflow, including callbacks and cancellation settlement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowBasis {
    pub workflow_id: String,
    pub session_id: String,
    pub session_generation: String,
    pub criteria_revision: String,
    /// Host-issued identity of the artifact scope and evidence collection lease.
    /// Actual checked artifact revisions appear separately in ReviewReceipt.
    pub artifact_evidence_basis: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptBasis {
    pub workflow: WorkflowBasis,
    pub attempt: usize,
}

pub struct WorkflowRequest {
    pub basis: WorkflowBasis,
    pub inputs: Vec<AgentInput>,
}

pub struct WorkflowPolicy {
    pub max_attempts: NonZeroUsize,
    /// Cumulative prepared logical provider requests, including all attempts.
    /// This does not count HTTP retries or establish a token/cost billing limit.
    pub max_prepared_requests: NonZeroUsize,
    pub deadline: Instant,
}

/// Opt-in recovery of a completed, tool-free Length response. Every approved
/// continuation consumes the original attempt/request/deadline allowances.
pub struct LengthRecoveryPolicy {
    pub max_continuations: NonZeroUsize,
}

#[derive(Clone, Debug)]
pub enum CoreRunFailure {
    Agent(Arc<AgentError>),
    AdmissionClosed,
    OwnerPanicked(String),
}

pub type CoreRunResult = Result<RunReport, CoreRunFailure>;
/// A sealed, single-use view of the SDK's canonical Agent-run owner. Polling it
/// admits the original Run; dropping it never drops a running Agent future.
pub type CoreRunFuture = BoxFuture<'static, CoreRunResult>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostDriverErrorKind {
    Failed,
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostDriverError {
    pub kind: HostDriverErrorKind,
    pub message: String,
}

impl HostDriverError {
    pub fn failed(message: impl Into<String>) -> Self {
        Self { kind: HostDriverErrorKind::Failed, message: message.into() }
    }
}

pub type DriverResult = Result<CoreRunResult, HostDriverError>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEvidence {
    pub artifact_id: String,
    pub revision: String,
    pub evidence_ref: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Settlement {
    /// All admitted Host effects and persistence have authoritative receipts.
    Settled,
    Unknown {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceiptCommit {
    Committed { receipt_id: String },
    Uncommitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReviewVerdict {
    Verified,
    NeedsChanges { feedback: String },
    Unknown { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewReceipt {
    pub basis: AttemptBasis,
    pub artifacts: Vec<ArtifactEvidence>,
    pub settlement: Settlement,
    pub commit: ReceiptCommit,
    pub verdict: ReviewVerdict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReviewError(pub String);

/// A candidate contains the canonical Core result, never a driver-supplied
/// substitute. It is only offered after the canonical owner seals Completed +
/// a real Stop assistant, before publishing the result to the driver. Message
/// records retain the Core's shared mutable identity; this is not an immutable
/// transcript snapshot. Admission does not trust their subsequent edits.
#[derive(Clone, Debug)]
pub struct ReviewCandidate {
    pub basis: AttemptBasis,
    pub run: RunReport,
}

/// A sealed tool-free Length stop. It is never an artifact-review candidate.
/// The Host must settle all earlier effects and persistence before permitting
/// a new input in this same Agent and Session generation.
#[derive(Clone, Debug)]
pub struct LengthStoppedRun {
    pub basis: AttemptBasis,
    pub run: RunReport,
}

/// No Verified verdict exists here. `None` settles the old Run without
/// continuing. `Some` requests a new bounded Run, with sourced Host feedback;
/// it never replays a truncated tool call or accepts the interrupted output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LengthSettlementReceipt {
    pub basis: AttemptBasis,
    pub artifacts: Vec<ArtifactEvidence>,
    pub settlement: Settlement,
    pub commit: ReceiptCommit,
    pub continuation: Option<String>,
}

/// One Host boundary for the original execution driver and authoritative
/// settlement/review. This is a capability contract, not a security sandbox.
///
/// The Host must retain exclusive Session ownership, bind the sink and all
/// effects to the supplied generation, and detect external changes. It must
/// seal/check actual artifact revisions, join all admitted effects, check
/// persistence, and commit the review receipt before returning Committed.
/// A model's final text is never a verifier. Unknown effects must remain Unknown.
#[async_trait]
pub trait WorkflowHost: Send + Sync {
    /// Ordinary typed Core Hosts can use this default. A Native Graph Host can
    /// wrap this exact future in its original concurrent graph driver. Never
    /// replace the Run, replay it, or report completion before this future joins.
    /// Native CLI integration is not provided by this crate.
    async fn drive_run(&self, _basis: &AttemptBasis, run: CoreRunFuture, _cancel: &CancellationToken) -> DriverResult {
        Ok(run.await)
    }

    /// Cancellation closes later admission but does not permit dropping this
    /// callback while it may own effects. A callback that ignores cancellation
    /// keeps the workflow pending until it actually settles.
    async fn review(
        &self,
        candidate: &ReviewCandidate,
        cancel: &CancellationToken,
    ) -> Result<ReviewReceipt, ReviewError>;

    /// Called only by the explicit length-recovery entry point. The canonical
    /// owner must have observed Completed + Length with no final tool calls.
    /// Like review, this callback is joined through cancellation; an error or
    /// unknown effect never grants another Run. The default grants nothing.
    async fn settle_length_stop(
        &self,
        _stopped: &LengthStoppedRun,
        _cancel: &CancellationToken,
    ) -> Result<LengthSettlementReceipt, ReviewError> {
        Err(ReviewError("Host does not support length-stop settlement".into()))
    }
}

#[derive(Clone, Debug)]
pub struct AttemptRecord {
    pub basis: AttemptBasis,
    pub core_result: Option<CoreRunResult>,
    pub driver_result: DriverResult,
    pub review: Option<Result<ReviewReceipt, ReviewError>>,
    pub length_settlement: Option<Result<LengthSettlementReceipt, ReviewError>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkflowStop {
    /// The artifact is verified. The Host still owns Task acceptance.
    VerifiedArtifact,
    AttemptLimit,
    PreparedRequestLimit,
    Cancelled,
    Deadline,
    AgentBusy,
    QueuedInput,
    DriverDidNotJoin,
    DriverFailed,
    CoreFailed,
    NonCompletedRun,
    NonStopAssistant,
    LengthContinuationLimit,
    LengthSettlementFailed,
    ReviewFailed,
    InvalidReceipt,
    Unknown,
}

#[derive(Debug)]
pub struct WorkflowOutcome {
    pub basis: WorkflowBasis,
    pub stop: WorkflowStop,
    pub attempts: Vec<AttemptRecord>,
    pub prepared_requests: usize,
    /// Present when Host effect/persistence settlement is unproven. Retain this
    /// fence until the Host reconciles the exact basis. Dropping it does not
    /// silently reopen model admission.
    pub reconciliation: Option<crate::ReconciliationFence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FenceReleaseError {
    WrongBasis,
    UnsettledOrUncommitted,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StartError {
    EmptyBasis(&'static str),
    EmptyInput,
    AgentBusy,
    QueuedInput,
    NoTokioRuntime,
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for StartError {}

#[derive(Clone, Debug)]
pub struct WorkflowJoinError(pub Arc<tokio::task::JoinError>);
impl std::fmt::Display for WorkflowJoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for WorkflowJoinError {}
