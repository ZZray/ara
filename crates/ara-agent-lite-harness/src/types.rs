use std::path::PathBuf;

use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub use ara_agent_lite_core::agent::{InterruptedRun, OpenToolCall, ToolCallReconciliation, ToolCallSettlement};
use ara_agent_lite_core::RunRequest;

/// Guidance only. A task kind grants no tool, filesystem or network authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskKind {
    Coding,
    Reasoning,
    ReverseAnalysis,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskSpec {
    pub task_id: String,
    pub criteria_revision: String,
    pub authorization_revision: String,
    pub kind: TaskKind,
    /// Must be a new directory under a Host-chosen existing parent.
    pub evidence_dir: PathBuf,
    pub max_runs: usize,
    /// Agent and summary reservations share this non-refundable logical budget.
    /// This is not provider usage or billing.
    pub max_calls: usize,
    pub deadline_unix_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewBasis {
    pub task_id: String,
    pub criteria_revision: String,
    pub authorization_revision: String,
    pub run_id: String,
    pub run_number: usize,
    pub session_path: PathBuf,
    pub events_path: PathBuf,
    pub events_sha256: String,
    pub event_count: u64,
    pub run_calls: usize,
    pub total_calls: usize,
    pub journal_head_sha256: String,
    pub raw_start: usize,
    pub raw_end: usize,
    pub raw_proof: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    pub basis: ReviewBasis,
    pub output: String,
    /// The durable Core Goal snapshot, separate from `agent_end` and code 0.
    pub goal: Option<Value>,
}

/// Host-owned admission policy. It grants no extra model calls or time.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStopReviewPolicy {
    #[default]
    Reject,
    ReviewClosedToolRound,
}

/// Whether a closed Length stop (Execute only) goes to Host review (16.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthStopReviewPolicy {
    #[default]
    Reject,
    ReviewClosedLength,
}

impl LengthStopReviewPolicy {
    fn is_default(&self) -> bool {
        *self == Self::Reject
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskOptions {
    #[serde(default)]
    pub budget_stop_review: BudgetStopReviewPolicy,
    #[serde(default, skip_serializing_if = "LengthStopReviewPolicy::is_default")]
    pub length_stop_review: LengthStopReviewPolicy,
}

/// The assistant message cut at the output limit, bound to the journal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TruncatedMessageRef {
    pub raw_index: usize,
    /// SHA-256 of the typed AgentMessage JSON (as in length recovery).
    pub typed_sha256: String,
    /// `journal.proof(raw_index, raw_index + 1)`.
    pub journal_proof: String,
    pub effective_output_tokens: u64,
}

/// A closed Length or NoProgress stop offered to Host review. The output is
/// diagnostic, never a final answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopSnapshot {
    pub basis: ReviewBasis,
    pub diagnostic_output: String,
    pub goal: Option<Value>,
    pub evidence: ara_agent_lite_core::termination::StopEvidence,
    pub truncated: Option<TruncatedMessageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopSnapshotReference {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct StopContinuation {
    pub previous: StopSnapshot,
    pub receipt: ReviewReceipt,
    pub next_run_number: usize,
    pub remaining_calls: usize,
    pub deadline_unix_ms: u64,
}

/// A closed native tool round, with no claim of a model final answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStopSnapshot {
    pub basis: ReviewBasis,
    pub diagnostic_output: String,
    pub goal: Option<Value>,
    pub termination: ara_agent_lite_core::termination::ClosedBudgetTermination,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStopSnapshotReference {
    pub path: PathBuf,
    pub sha256: String,
}

/// Published only after a committed, settled, source-checked snapshot review.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStopReviewRecord {
    pub version: u32,
    pub kind: String,
    pub snapshot: BudgetStopSnapshotReference,
    pub receipt: ReviewReceipt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Settlement {
    Settled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Verdict {
    Verified,
    NeedsChanges { feedback: String },
    Unknown,
}

/// A trusted Host declaration about this exact candidate, never Core-proven
/// effect settlement. `committed` means the Host committed its review and
/// supporting business/artifact evidence before returning this receipt.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewReceipt {
    pub basis: ReviewBasis,
    pub committed: bool,
    pub settlement: Settlement,
    pub verdict: Verdict,
    /// Host-owned immutable artifact/receipt references, not model assertions.
    pub evidence: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Continuation {
    pub previous: Candidate,
    pub receipt: ReviewReceipt,
    pub next_run_number: usize,
    pub remaining_calls: usize,
    pub deadline_unix_ms: u64,
}

#[derive(Clone, Debug)]
pub struct BudgetStopContinuation {
    pub previous: BudgetStopSnapshot,
    pub receipt: ReviewReceipt,
    pub next_run_number: usize,
    pub remaining_calls: usize,
    pub deadline_unix_ms: u64,
}

/// What a fenced Task left behind. Everything here comes from durable files;
/// no field is a claim that effects settled.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResumeReview {
    pub task_id: String,
    pub session_path: PathBuf,
    /// New evidence directory of this resume attempt (already created).
    pub resume_evidence_dir: PathBuf,
    /// Evidence of the fenced owner (per-Run admissions, events, reviews).
    pub fenced_evidence_dir: PathBuf,
    pub fence: Value,
    /// `None` means the owner ended without publishing an outcome (crash/kill).
    pub fenced_outcome: Option<TaskOutcome>,
    /// The native Run the owner left pending, with its unanswered tool calls.
    pub interrupted_run: Option<InterruptedRun>,
    /// Non-refundable Task consumption, including every earlier resume. An
    /// unobserved interrupted Run is charged its whole admitted call limit.
    pub consumed_runs: usize,
    pub consumed_calls: usize,
}

/// Extra checks of `resume_checked`, all made before any Host callback.
#[derive(Clone, Debug, Default)]
pub struct ResumeChecks {
    /// Fence bytes the Host inspected; a different fence is refused under the Session lock.
    pub expected_fence_sha256: Option<String>,
    /// Refuse (instead of clamping) a per-Run call/second cap above the original first admission.
    pub refuse_larger_run_caps: bool,
}

/// Why a resume was refused. `Display` is a fixed English phrase without
/// input values; the error chain may carry the underlying cause.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResumeRefusal {
    NoFence,
    FenceMismatch,
    FenceUnreadable,
    TaskTermsMismatch,
    AuthorityChanged,
    BudgetLarger,
    RunCapsLarger,
    DeadlineElapsed,
    BudgetExhausted,
    HostCallbackFailed,
    ReceiptUncommitted,
    SettlementUnknown,
    EvidenceEmpty,
    CallMismatch,
}

impl std::fmt::Display for ResumeRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NoFence => "Session has no harness fence; nothing to resume",
            Self::FenceMismatch => "Session fence differs from the inspected fence; fence kept",
            Self::FenceUnreadable => "harness fence is unreadable or predates resumable records; fence kept",
            Self::TaskTermsMismatch => {
                "resume must keep the original Task, criteria, authorization, deadline and Session"
            }
            Self::AuthorityChanged => "resume changed Host authority (model, endpoint, grant, tools, paths or mode)",
            Self::BudgetLarger => "resume must keep the original Task budget",
            Self::RunCapsLarger => "resume requested larger per-Run caps than the original admission",
            Self::DeadlineElapsed => "original task deadline elapsed; fence kept",
            Self::BudgetExhausted => "original Task budget is exhausted; resume cannot extend it",
            Self::HostCallbackFailed => "Host resume reconciliation failed; fence kept",
            Self::ReceiptUncommitted => "Host did not commit the reconciliation; fence kept",
            Self::SettlementUnknown => "Host reconciliation is not settled; fence kept",
            Self::EvidenceEmpty => "Host reconciliation has no evidence; fence kept",
            Self::CallMismatch => {
                "Host must settle exactly the open native tool calls, in their original order; fence kept"
            }
        })
    }
}

impl std::error::Error for ResumeRefusal {}

/// Host settlement of a fenced Task, based on its own receipts and artifacts.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResumeReceipt {
    pub committed: bool,
    pub settlement: Settlement,
    /// One entry per open call of `interrupted_run`, in the same order.
    pub tool_calls: Vec<ToolCallReconciliation>,
    pub evidence: Vec<String>,
}

#[async_trait]
pub trait HarnessHost: Send + Sync + 'static {
    /// Inspect actual artifacts and receipts. Model output alone is insufficient.
    /// Honor cancellation and return only after any callback effects have settled.
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt>;

    /// Supply a new Run ID and correction objective in the SAME Session. The
    /// harness checks immutable authority and overrides remaining call/time caps.
    /// A fresh ID must never be used to replay an operation with unknown effects.
    async fn prepare_continuation(
        &self,
        continuation: Continuation,
        cancellation: CancellationToken,
    ) -> Result<RunRequest>;

    /// Explicitly inspect actual artifacts after a closed budget stop. Existing
    /// Hosts reject this path instead of treating it as an ordinary Candidate.
    async fn verify_budget_snapshot(
        &self,
        _snapshot: BudgetStopSnapshot,
        _cancellation: CancellationToken,
    ) -> Result<ReviewReceipt> {
        bail!("Host does not support closed-budget snapshot review")
    }

    /// Continue only within the original Task budget, Session and authority.
    async fn prepare_budget_stop_continuation(
        &self,
        _continuation: BudgetStopContinuation,
        _cancellation: CancellationToken,
    ) -> Result<RunRequest> {
        bail!("Host does not support closed-budget snapshot continuation")
    }

    /// Inspect actual artifacts after a closed Length or NoProgress stop.
    async fn verify_stop_snapshot(
        &self,
        _snapshot: StopSnapshot,
        _cancellation: CancellationToken,
    ) -> Result<ReviewReceipt> {
        bail!("Host does not support stop snapshot review")
    }

    /// Continue a reviewed stop in the same Session within the original budget.
    async fn prepare_stop_continuation(
        &self,
        _continuation: StopContinuation,
        _cancellation: CancellationToken,
    ) -> Result<RunRequest> {
        bail!("Host does not support stop snapshot continuation")
    }

    /// Inspect what a fenced Task actually did and settle it. Only a committed
    /// `Settled` receipt lets `resume` continue; nothing is replayed for you.
    async fn reconcile_resume(&self, _review: ResumeReview, _cancellation: CancellationToken) -> Result<ResumeReceipt> {
        bail!("Host does not support fenced Task resume")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStop {
    /// Host verification passed; accepting the business Task remains external.
    AwaitingHostAcceptance,
    NeedsChanges,
    BudgetExhausted,
    Cancelled,
    GoalWaiting,
    /// A management/command Run with no model calls is not an artifact candidate.
    Informational,
    ReconciliationRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FencePresence {
    Present,
    Released,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskOutcome {
    pub task_id: String,
    pub stop: TaskStop,
    pub runs: usize,
    pub observed_calls: usize,
    pub last_candidate: Option<Candidate>,
    pub last_receipt: Option<ReviewReceipt>,
    pub detail: String,
    /// Always false: the embedding product owns business Task acceptance.
    pub task_accepted: bool,
    /// The reconciliation location. Inspect `fence_presence`; a filesystem
    /// failure can prevent confirming that the marker physically exists.
    pub reconciliation_fence: Option<PathBuf>,
    pub fence_presence: FencePresence,
    /// Tool calls whose external effects are unknown, in Run then Session
    /// order, one per `(run_id, native_call_id)` (requirement 22.4).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unknown_effects: Vec<UnknownEffectRecord>,
    /// Why a `BudgetExhausted` stop happened, when it was not a plain Run end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_stop: Option<BudgetStopDetail>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnknownEffectRecord {
    pub run_id: String,
    pub native_call_id: String,
    pub tool: String,
    pub receipt_ref: EffectReceiptRef,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectReceiptRef {
    pub evidence: EffectEvidence,
    /// Host receipt named by the tool result's `receipt_ref` (JSONL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_receipt: Option<HostReceiptRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EffectEvidence {
    /// The ToolResult persisted in the Session journal.
    Journal { session_path: PathBuf, raw_index: usize, message_sha256: String },
    /// Only the `tool_end` event exists (the result never became durable).
    Event { events_path: PathBuf, seq: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostReceiptRef {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetStopDetail {
    pub trigger: BudgetStopTrigger,
    pub task_max_calls: usize,
    pub total_calls: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStopTrigger {
    CompactionSummary,
}

/// UI observation only. Lagging or absent subscribers never block Core events.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub task_id: String,
    pub run_id: String,
    pub seq: u64,
    pub event: Value,
}
