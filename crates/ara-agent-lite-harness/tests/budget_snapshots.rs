mod common;

use anyhow::{ensure, Result};
use ara_agent_lite_harness::{
    core::{
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunRequest,
    },
    *,
};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotFault {
    None,
    TamperSnapshot,
    TamperPrepare,
    SlowReview,
    UnknownReview,
    Uncommitted,
    BadBasis,
}

struct SnapshotHost {
    base: Host,
    fault: SnapshotFault,
    snapshots: Mutex<Vec<BudgetStopSnapshot>>,
    snapshot_preparations: AtomicUsize,
    review_started: AtomicBool,
    review_joined: AtomicBool,
    deadline: AtomicU64,
    total_calls: AtomicUsize,
}

impl SnapshotHost {
    fn new(root: &Path, provider: &Provider, fault: SnapshotFault) -> Self {
        Self {
            base: Host::new(root, provider, Fault::None),
            fault,
            snapshots: Mutex::new(Vec::new()),
            snapshot_preparations: AtomicUsize::new(0),
            review_started: AtomicBool::new(false),
            review_joined: AtomicBool::new(false),
            deadline: AtomicU64::new(0),
            total_calls: AtomicUsize::new(0),
        }
    }

    fn request(&self, id: &str, calls: usize) -> RunRequest {
        let mut request = self.base.request(id);
        request.max_calls = calls;
        request
    }

    fn spec(&self, name: &str, calls: usize, runs: usize) -> TaskSpec {
        let mut spec = self.base.spec(name);
        spec.max_calls = calls;
        spec.max_runs = runs;
        self.deadline.store(spec.deadline_unix_ms, Ordering::SeqCst);
        self.total_calls.store(calls, Ordering::SeqCst);
        spec
    }
}

fn tamper_snapshot(snapshot: &BudgetStopSnapshot) -> Result<()> {
    let path = budget_snapshot_path(snapshot.basis.events_path.parent().unwrap(), snapshot.basis.run_number);
    // Keep valid, semantically identical JSON: only an exact byte hash detects this.
    std::fs::OpenOptions::new().append(true).open(path)?.write_all(b"\n ")?;
    Ok(())
}

#[async_trait]
impl HarnessHost for SnapshotHost {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.base.verify(candidate, cancel).await
    }

    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        let mut request = self.base.prepare_continuation(continuation, cancel).await?;
        request.max_calls = 1;
        Ok(request)
    }

    async fn verify_budget_snapshot(
        &self,
        snapshot: BudgetStopSnapshot,
        cancel: CancellationToken,
    ) -> Result<ReviewReceipt> {
        ensure!(snapshot.termination.is_reviewable(), "snapshot is not a closed budget stop");
        self.snapshots.lock().unwrap().push(snapshot.clone());
        self.review_started.store(true, Ordering::SeqCst);
        if self.fault == SnapshotFault::SlowReview {
            tokio::time::timeout(Duration::from_secs(4), cancel.cancelled()).await?;
            tokio::time::sleep(Duration::from_millis(40)).await;
            self.review_joined.store(true, Ordering::SeqCst);
        }
        let actual = std::fs::read(self.base.root.join("answer.txt")).unwrap_or_default();
        let passed = actual == b"correct";
        let proof = self.base.root.join(format!("budget-verifier-{}.json", snapshot.basis.run_number));
        std::fs::write(
            &proof,
            serde_json::to_vec(&json!({"basis":snapshot.basis,"actual_bytes":actual,"passed":passed}))?,
        )?;
        std::fs::OpenOptions::new().write(true).open(&proof)?.sync_all()?;
        if self.fault == SnapshotFault::TamperSnapshot {
            tamper_snapshot(&snapshot)?;
        }
        let mut basis = snapshot.basis;
        if self.fault == SnapshotFault::BadBasis {
            basis.criteria_revision = "unrelated-criteria".into();
        }
        Ok(ReviewReceipt {
            basis,
            committed: self.fault != SnapshotFault::Uncommitted,
            settlement: if self.fault == SnapshotFault::UnknownReview {
                Settlement::Unknown
            } else {
                Settlement::Settled
            },
            verdict: if passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges { feedback: "The actual file must contain exactly correct. Repair it.".into() }
            },
            evidence: vec![proof.to_string_lossy().into_owned()],
        })
    }

    async fn prepare_budget_stop_continuation(
        &self,
        continuation: BudgetStopContinuation,
        cancel: CancellationToken,
    ) -> Result<RunRequest> {
        ensure!(!cancel.is_cancelled(), "continuation was canceled");
        ensure!(continuation.previous.termination.is_reviewable(), "ordinary candidate passed to budget preparation");
        ensure!(continuation.receipt.basis == continuation.previous.basis, "receipt source changed");
        ensure!(continuation.deadline_unix_ms == self.deadline.load(Ordering::SeqCst), "deadline was renewed");
        ensure!(
            continuation.remaining_calls
                == self.total_calls.load(Ordering::SeqCst) - continuation.previous.basis.total_calls,
            "Task budget was renewed"
        );
        self.snapshot_preparations.fetch_add(1, Ordering::SeqCst);
        if self.fault == SnapshotFault::TamperPrepare {
            tamper_snapshot(&continuation.previous)?;
        }
        let mut request = self.request(&format!("budget-repair-{}", continuation.next_run_number), 1);
        request.objective = match continuation.receipt.verdict {
            Verdict::NeedsChanges { feedback } => format!("Committed Host feedback: {feedback}"),
            _ => anyhow::bail!("continuation requires NeedsChanges"),
        };
        Ok(request)
    }
}

fn options() -> TaskOptions {
    TaskOptions { budget_stop_review: BudgetStopReviewPolicy::ReviewClosedToolRound, ..TaskOptions::default() }
}

async fn wait(handle: TaskHandle) -> Arc<TaskOutcome> {
    tokio::time::timeout(Duration::from_secs(10), handle.wait()).await.unwrap()
}

fn file_hash(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

fn assert_budget_record(root: &Path, evidence: &str, number: usize) -> BudgetStopReviewRecord {
    let directory = root.join(evidence);
    let record: BudgetStopReviewRecord =
        serde_json::from_slice(&std::fs::read(budget_review_index_path(&directory, number)).unwrap()).unwrap();
    assert_eq!(record.version, 1);
    assert_eq!(record.kind, "closed_budget_snapshot_review");
    assert_eq!(record.snapshot.sha256, file_hash(&record.snapshot.path));
    let snapshot: BudgetStopSnapshot = serde_json::from_slice(&std::fs::read(&record.snapshot.path).unwrap()).unwrap();
    assert_eq!(snapshot.basis, record.receipt.basis);
    assert!(snapshot.termination.is_reviewable());
    assert!(!directory.join(format!("run-{number}-candidate.json")).exists());
    let rows = events(&directory.join(format!("run-{number}-events.jsonl")));
    let terminal = rows.iter().find(|row| row["event"]["type"] == "agent_end").unwrap();
    assert_eq!(terminal["event"]["code"], -1);
    assert_eq!(terminal["event"]["status"], "failed");
    assert!(rows.iter().any(|row| row["event"]["type"] == "model_result" && row["event"]["stop_reason"] == "toolUse"));
    assert!(!rows.iter().any(|row| row["event"]["type"] == "model_result" && row["event"]["stop_reason"] == "stop"));
    record
}

#[tokio::test]
async fn default_start_keeps_closed_budget_stop_out_of_ordinary_review() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Never requested.")]);
    // This existing Host only implements the old required methods.
    let host = Arc::new(Host::new(root.path(), &provider, Fault::None));
    let mut request = host.request("original");
    request.max_calls = 1;
    let mut spec = host.spec("default-budget");
    spec.max_calls = 1;
    let outcome = wait(start(spec, request, host.clone()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(outcome.last_candidate.is_none() && outcome.last_receipt.is_none());
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(std::fs::read(root.path().join("answer.txt")).unwrap(), b"correct");
    assert!(!budget_snapshot_path(&root.path().join("default-budget"), 1).exists());
}

#[tokio::test]
async fn opt_in_does_not_fall_back_to_old_host_verification() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct")]);
    let host = Arc::new(Host::new(root.path(), &provider, Fault::None));
    let mut request = host.request("unsupported");
    request.max_calls = 1;
    let mut spec = host.spec("unsupported");
    spec.max_calls = 1;
    let outcome = wait(start_with_options(spec, request, host.clone(), options()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert!(budget_snapshot_path(&root.path().join("unsupported"), 1).is_file());
    assert!(!budget_review_index_path(&root.path().join("unsupported"), 1).exists());
}

#[tokio::test]
async fn actual_file_can_be_verified_without_synthesizing_a_model_stop() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Never requested.")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::None));
    let spec = host.spec("verified-budget", 1, 1);
    let deadline = spec.deadline_unix_ms;
    let outcome = wait(start_with_options(spec, host.request("original", 1), host.clone(), options()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Released);
    assert_eq!((outcome.runs, outcome.observed_calls), (1, 1));
    assert!(outcome.last_candidate.is_none());
    assert!(!outcome.task_accepted);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(host.base.reviews.load(Ordering::SeqCst), 0);
    let record = assert_budget_record(root.path(), "verified-budget", 1);
    assert_eq!(record.receipt.verdict, Verdict::Verified);
    assert_eq!(record.receipt.basis, outcome.last_receipt.as_ref().unwrap().basis);
    let admission: Value =
        serde_json::from_slice(&std::fs::read(root.path().join("verified-budget/run-1-admission.json")).unwrap())
            .unwrap();
    assert_eq!(admission["deadline_unix_ms"], deadline);
}

#[tokio::test]
async fn bad_actual_file_exhausts_original_task_without_preparation_or_replay() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("partial"), write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::None));
    let outcome = wait(
        start_with_options(host.spec("bad-budget", 1, 2), host.request("original", 1), host.clone(), options())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::BudgetExhausted, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Released);
    assert_eq!(host.snapshot_preparations.load(Ordering::SeqCst), 0);
    assert_eq!(host.base.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(std::fs::read(root.path().join("answer.txt")).unwrap(), b"partial");
    assert!(matches!(assert_budget_record(root.path(), "bad-budget", 1).receipt.verdict, Verdict::NeedsChanges { .. }));
}

#[tokio::test]
async fn smaller_run_budget_uses_only_typed_continuation_and_original_task_remainder() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("partial"), write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::None));
    let spec = host.spec("typed-continuation", 2, 2);
    let deadline = spec.deadline_unix_ms;
    let outcome = wait(start_with_options(spec, host.request("original", 1), host.clone(), options()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, outcome.observed_calls), (2, 2));
    assert!(outcome.last_candidate.is_none());
    assert_eq!(host.snapshot_preparations.load(Ordering::SeqCst), 1);
    assert_eq!(host.base.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(host.base.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    for number in 1..=2 {
        let record = assert_budget_record(root.path(), "typed-continuation", number);
        assert_eq!(record.receipt.basis.total_calls, number);
        let admission: Value = serde_json::from_slice(
            &std::fs::read(root.path().join(format!("typed-continuation/run-{number}-admission.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(admission["remaining_calls"], 1);
        assert_eq!(admission["deadline_unix_ms"], deadline);
    }
}

#[tokio::test]
async fn budget_snapshot_clears_a_previous_ordinary_candidate_without_removing_its_files() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("partial"), text("First submission."), write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::None));
    let outcome = wait(
        start_with_options(
            host.spec("ordinary-then-budget", 3, 2),
            host.request("original", 2),
            host.clone(),
            options(),
        )
        .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, outcome.observed_calls), (2, 3));
    assert!(outcome.last_candidate.is_none());
    assert_eq!(outcome.last_receipt.as_ref().unwrap().basis.run_number, 2);
    assert_eq!(host.base.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(host.base.preparations.load(Ordering::SeqCst), 1);
    assert!(root.path().join("ordinary-then-budget/run-1-candidate.json").is_file());
    assert!(root.path().join("ordinary-then-budget/run-1-review.json").is_file());
    assert_budget_record(root.path(), "ordinary-then-budget", 2);
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn snapshot_byte_tampering_during_review_preserves_the_fence_and_raw_receipt() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::TamperSnapshot));
    let outcome = wait(
        start_with_options(host.spec("snapshot-tamper", 1, 1), host.request("original", 1), host, options()).unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(root.path().join("snapshot-tamper/run-1-budget-review.json").is_file());
    assert!(!budget_review_index_path(&root.path().join("snapshot-tamper"), 1).exists());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn snapshot_tampering_in_typed_preparation_stops_before_the_next_http() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("partial"), write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::TamperPrepare));
    let outcome = wait(
        start_with_options(host.spec("prepare-tamper", 2, 2), host.request("original", 1), host.clone(), options())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert_eq!(outcome.runs, 1);
    assert_eq!(host.snapshot_preparations.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(budget_review_index_path(&root.path().join("prepare-tamper"), 1).is_file());
}

#[tokio::test]
async fn unknown_native_effect_cannot_enter_the_budget_snapshot_callback() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("unknown", "read_terms", json!({}))]);
    let mut host = SnapshotHost::new(root.path(), &provider, SnapshotFault::None);
    host.base.read_tool = Some(Arc::new(ReadTerms::new(true)));
    let host = Arc::new(host);
    let outcome = wait(
        start_with_options(host.spec("unknown-budget", 1, 1), host.request("original", 1), host.clone(), options())
            .unwrap(),
    )
    .await;
    assert_ne!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(host.snapshots.lock().unwrap().is_empty());
    assert!(!budget_snapshot_path(&root.path().join("unknown-budget"), 1).exists());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

struct KnownReject(Tool);

#[async_trait]
impl AgentTool for KnownReject {
    fn schema(&self) -> &Tool {
        &self.0
    }
    fn label(&self) -> &str {
        "known_reject"
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        Err(AgentError::Tool("known pre-invocation rejection; no operation started".into()))
    }
}

#[tokio::test]
async fn known_settled_tool_error_is_reviewed_without_becoming_business_success() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("known-reject", "known_reject", json!({}))]);
    let mut host = SnapshotHost::new(root.path(), &provider, SnapshotFault::None);
    host.base.read_tool = Some(Arc::new(KnownReject(Tool {
        name: "known_reject".into(),
        description: "Reject a fixture operation before dispatch.".into(),
        parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
        constrained_sampling: None,
    })));
    let host = Arc::new(host);
    let outcome = wait(
        start_with_options(host.spec("known-error", 1, 1), host.request("original", 1), host.clone(), options())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::BudgetExhausted, "{outcome:?}");
    assert_eq!(host.snapshots.lock().unwrap().len(), 1);
    assert!(!root.path().join("answer.txt").exists());
    assert!(matches!(
        assert_budget_record(root.path(), "known-error", 1).receipt.verdict,
        Verdict::NeedsChanges { .. }
    ));
    let rows = events(&root.path().join("known-error/run-1-events.jsonl"));
    assert!(rows.iter().any(|row| row["event"]["type"] == "tool_end" && row["event"]["status"] == "failed"));
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn complete_multi_tool_round_is_reviewable_without_promoting_literal_unknown_text() {
    let root = tempfile::tempdir().unwrap();
    let response = json!({"delta":{"tool_calls":[
        {"index":0,"id":"write-round","type":"function","function":{"name":"write_answer","arguments":"{\"content\":\"correct\"}"}},
        {"index":1,"id":"read-round","type":"function","function":{"name":"read_terms","arguments":"{}"}}
    ]},"finish":"tool_calls"});
    let provider = Provider::new(vec![response]);
    let mut host = SnapshotHost::new(root.path(), &provider, SnapshotFault::None);
    host.base.read_tool = Some(Arc::new(ReadTerms::new(false)));
    let host = Arc::new(host);
    let outcome = wait(
        start_with_options(host.spec("multi-round", 1, 1), host.request("original", 1), host.clone(), options())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(host.base.tool.writes.load(Ordering::SeqCst), 1);
    assert_budget_record(root.path(), "multi-round", 1);
    let rows = events(&root.path().join("multi-round/run-1-events.jsonl"));
    assert_eq!(rows.iter().filter(|row| row["event"]["type"] == "tool_end").count(), 2);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn cancellation_of_the_last_inflight_tool_cannot_become_a_budget_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("cancel-original", "delayed_cancel", json!({}))]);
    let tool = Arc::new(DelayedCancelTool::new(root.path()));
    let mut host = SnapshotHost::new(root.path(), &provider, SnapshotFault::None);
    host.base.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let handle =
        start_with_options(host.spec("cancel-tool", 1, 1), host.request("original", 1), host.clone(), options())
            .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !tool.started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !tool.cancellation_seen.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(host.snapshots.lock().unwrap().is_empty());
    tool.release_settlement();
    let outcome = wait(handle).await;
    assert!(tool.joined.load(Ordering::SeqCst));
    assert_eq!(outcome.stop, TaskStop::Cancelled, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(host.snapshots.lock().unwrap().is_empty());
    assert!(!budget_snapshot_path(&root.path().join("cancel-tool"), 1).exists());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn canceled_snapshot_review_joins_and_does_not_publish_a_trusted_index() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, SnapshotFault::SlowReview));
    let handle =
        start_with_options(host.spec("cancel-review", 1, 1), host.request("original", 1), host.clone(), options())
            .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !host.review_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    let outcome = wait(handle).await;
    assert_eq!(outcome.stop, TaskStop::Cancelled, "{outcome:?}");
    assert!(host.review_joined.load(Ordering::SeqCst));
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(root.path().join("cancel-review/run-1-budget-review.json").is_file());
    assert!(!budget_review_index_path(&root.path().join("cancel-review"), 1).exists());
}

async fn untrusted_review(fault: SnapshotFault, name: &str) {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct")]);
    let host = Arc::new(SnapshotHost::new(root.path(), &provider, fault));
    let outcome =
        wait(start_with_options(host.spec(name, 1, 1), host.request("original", 1), host, options()).unwrap()).await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(root.path().join(name).join("run-1-budget-review.json").is_file());
    assert!(!budget_review_index_path(&root.path().join(name), 1).exists());
}

#[tokio::test]
async fn unknown_host_review_has_no_trusted_index() {
    untrusted_review(SnapshotFault::UnknownReview, "unknown-review").await;
}

#[tokio::test]
async fn uncommitted_host_review_has_no_trusted_index() {
    untrusted_review(SnapshotFault::Uncommitted, "uncommitted-review").await;
}

#[tokio::test]
async fn mismatched_review_basis_has_no_trusted_index() {
    untrusted_review(SnapshotFault::BadBasis, "bad-basis").await;
}
