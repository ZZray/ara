//! Stability check for the Harness_Lite rows of
//! `crates/ara-agent-lite-core/docs/stability.md` (requirement 29, used by the
//! consumer crate of requirement 31): each stable Harness item is used once
//! through its public path, so deleting, renaming or retyping it fails to
//! compile, and the document's stable set must equal the set used here.
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};

use anyhow::Result;
use ara_agent_lite_core::RunRequest;
use ara_agent_lite_harness::{
    Candidate, Continuation, EventEnvelope, FencePresence, HarnessHost, ReviewBasis, ReviewReceipt, Settlement,
    TaskHandle, TaskKind, TaskOutcome, TaskSpec, TaskStop, Verdict,
};
use tokio_util::sync::CancellationToken;

const HARNESS: &str = "ara_agent_lite_harness::";

const USED_STABLE: &[&str] = &[
    "start",
    "TaskHandle",
    "TaskSpec",
    "TaskKind",
    "HarnessHost",
    "Candidate",
    "ReviewBasis",
    "ReviewReceipt",
    "Settlement",
    "Verdict",
    "Continuation",
    "TaskOutcome",
    "TaskStop",
    "FencePresence",
    "EventEnvelope",
];

fn rows() -> Vec<(String, String)> {
    let doc = include_str!("../../ara-agent-lite-core/docs/stability.md");
    doc.lines()
        .filter_map(|line| {
            let cells: Vec<_> = line.trim().strip_prefix('|')?.split('|').map(str::trim).collect();
            let item = cells.first()?.strip_prefix('`')?.strip_suffix('`')?.strip_prefix(HARNESS)?;
            Some((item.to_string(), cells.get(1)?.to_string()))
        })
        .collect()
}

#[test]
fn harness_rows_name_this_version_and_match_the_used_stable_set() {
    let doc = include_str!("../../ara-agent-lite-core/docs/stability.md");
    let version = format!("`ara-agent-lite-harness {}`", env!("CARGO_PKG_VERSION"));
    assert!(doc.contains(&version), "stability.md must name {version}");
    let rows = rows();
    let mut seen = BTreeSet::new();
    for (item, level) in &rows {
        assert!(matches!(level.as_str(), "stable" | "experimental" | "internal"), "{item}: level {level}");
        assert!(seen.insert(item.clone()), "{item} is listed twice");
    }
    let documented: BTreeSet<String> =
        rows.into_iter().filter(|(_, level)| level == "stable").map(|(item, _)| item).collect();
    let used: BTreeSet<String> = USED_STABLE.iter().map(|item| item.to_string()).collect();
    assert_eq!(USED_STABLE.len(), used.len(), "USED_STABLE has duplicates");
    assert_eq!(documented, used, "stable Harness items in stability.md versus this test");
}

#[test]
fn stable_harness_items_are_usable_through_public_paths() {
    let spec = TaskSpec {
        task_id: String::from("task"),
        criteria_revision: String::from("criteria-v1"),
        authorization_revision: String::from("authorization-v1"),
        kind: TaskKind::Coding,
        evidence_dir: PathBuf::from("evidence"),
        max_runs: 1usize,
        max_calls: 1usize,
        deadline_unix_ms: 0u64,
    };
    assert!(matches!(spec.kind, TaskKind::Coding | TaskKind::Reasoning | TaskKind::ReverseAnalysis));
    // `start` with its full signature; the closure is never called, so no Task starts.
    let _start: fn(TaskSpec, RunRequest, Arc<dyn HarnessHost>) -> Result<TaskHandle> = ara_agent_lite_harness::start;
    let host: Arc<dyn HarnessHost> = Arc::new(SignatureHost);
    drop((spec, host));
}

/// Typed reads of every stable return type, compiled but only called on values the Harness produces.
#[allow(dead_code)]
async fn read_handle(handle: &TaskHandle) {
    let _: tokio::sync::broadcast::Receiver<EventEnvelope> = handle.subscribe();
    let outcome: Arc<TaskOutcome> = handle.wait().await;
    handle.cancel();
    read_outcome(&outcome);
}

#[allow(dead_code)]
fn read_outcome(outcome: &TaskOutcome) {
    let _: (&String, usize, usize, &String, bool) =
        (&outcome.task_id, outcome.runs, outcome.observed_calls, &outcome.detail, outcome.task_accepted);
    let _: (&Option<Candidate>, &Option<ReviewReceipt>, &Option<PathBuf>) =
        (&outcome.last_candidate, &outcome.last_receipt, &outcome.reconciliation_fence);
    match outcome.stop {
        TaskStop::AwaitingHostAcceptance
        | TaskStop::NeedsChanges
        | TaskStop::BudgetExhausted
        | TaskStop::Cancelled
        | TaskStop::GoalWaiting
        | TaskStop::Informational
        | TaskStop::ReconciliationRequired => {}
    }
    match outcome.fence_presence {
        FencePresence::Present | FencePresence::Released | FencePresence::Unknown => {}
    }
}

#[allow(dead_code)]
fn read_event(envelope: &EventEnvelope) {
    let _: (&String, &String, u64, &serde_json::Value) =
        (&envelope.task_id, &envelope.run_id, envelope.seq, &envelope.event);
}

#[allow(dead_code)]
fn read_basis(basis: &ReviewBasis) {
    let _: (&String, &String, &String, &String, usize) =
        (&basis.task_id, &basis.criteria_revision, &basis.authorization_revision, &basis.run_id, basis.run_number);
    let _: (&PathBuf, &PathBuf, &String, u64) =
        (&basis.session_path, &basis.events_path, &basis.events_sha256, basis.event_count);
    let _: (usize, usize, &String, usize, usize, &String) = (
        basis.run_calls,
        basis.total_calls,
        &basis.journal_head_sha256,
        basis.raw_start,
        basis.raw_end,
        &basis.raw_proof,
    );
}

struct SignatureHost;

#[async_trait::async_trait]
impl HarnessHost for SignatureHost {
    async fn verify(&self, candidate: Candidate, _: CancellationToken) -> Result<ReviewReceipt> {
        let Candidate { basis, output, goal, .. } = candidate;
        let _: (&String, &Option<serde_json::Value>) = (&output, &goal);
        read_basis(&basis);
        Ok(ReviewReceipt {
            basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict: Verdict::NeedsChanges { feedback: String::from("repair") },
            evidence: vec![String::from("host-receipt")],
        })
    }

    async fn prepare_continuation(&self, continuation: Continuation, _: CancellationToken) -> Result<RunRequest> {
        let Continuation { previous, receipt, next_run_number, remaining_calls, deadline_unix_ms, .. } = continuation;
        let _: (&Candidate, usize, usize, u64) = (&previous, next_run_number, remaining_calls, deadline_unix_ms);
        match (&receipt.settlement, &receipt.verdict) {
            (Settlement::Settled | Settlement::Unknown, Verdict::Verified | Verdict::Unknown) => {}
            (Settlement::Settled | Settlement::Unknown, Verdict::NeedsChanges { feedback }) => {
                let _: &String = feedback;
            }
        }
        let _: (bool, &Vec<String>) = (receipt.committed, &receipt.evidence);
        anyhow::bail!("signature host never continues")
    }
}
