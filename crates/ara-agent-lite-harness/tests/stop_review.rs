//! Controlled tests of Length stop-snapshot review (requirement 16) over the
//! real Core `execute` and a loopback provider.
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::Result;
use ara_agent_lite_harness::{core::RunRequest, *};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn length(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"length"})
}

/// Wraps the common Host and adds stop-snapshot review over the same file.
struct StopHost {
    base: Host,
    supports: bool,
    stop_reviews: AtomicUsize,
    snapshots: std::sync::Mutex<Vec<StopSnapshot>>,
}

#[async_trait]
impl HarnessHost for StopHost {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.base.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.base.prepare_continuation(continuation, cancel).await
    }
    async fn verify_stop_snapshot(&self, snapshot: StopSnapshot, _: CancellationToken) -> Result<ReviewReceipt> {
        anyhow::ensure!(self.supports, "fixture Host does not review stop snapshots");
        self.stop_reviews.fetch_add(1, Ordering::SeqCst);
        self.snapshots.lock().unwrap().push(snapshot.clone());
        let passed = std::fs::read(self.base.root.join("answer.txt")).unwrap_or_default() == b"correct";
        Ok(ReviewReceipt {
            basis: snapshot.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict: if passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges { feedback: "answer.txt is wrong; write the correct value".into() }
            },
            evidence: vec!["fixture-stop-review".into()],
        })
    }
    async fn prepare_stop_continuation(
        &self,
        continuation: StopContinuation,
        _: CancellationToken,
    ) -> Result<RunRequest> {
        let mut request = self.base.request(&format!("stop-repair-{}", continuation.next_run_number));
        request.objective = "Repair the answer after the truncated response.".into();
        Ok(request)
    }
}

fn host(root: &std::path::Path, provider: &Provider, supports: bool) -> Arc<StopHost> {
    Arc::new(StopHost {
        base: Host::new(root, provider, Fault::None),
        supports,
        stop_reviews: AtomicUsize::new(0),
        snapshots: Default::default(),
    })
}

fn review_length() -> TaskOptions {
    TaskOptions { length_stop_review: LengthStopReviewPolicy::ReviewClosedLength, ..TaskOptions::default() }
}

async fn wait(handle: TaskHandle) -> Arc<TaskOutcome> {
    tokio::time::timeout(std::time::Duration::from_secs(30), handle.wait()).await.unwrap()
}

#[tokio::test]
async fn closed_length_stop_is_reviewed_and_verified_without_a_new_run() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), length("partial report")]);
    let host = host(root.path(), &provider, true);
    let outcome = wait(
        start_with_options(host.base.spec("length"), host.base.request("original"), host.clone(), review_length())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, host.stop_reviews.load(Ordering::SeqCst)), (1, 1));
    assert!(!outcome.task_accepted);
    let snapshot = host.snapshots.lock().unwrap()[0].clone();
    assert!(matches!(snapshot.evidence, core::termination::StopEvidence::Length { .. }));
    let truncated = snapshot.truncated.expect("truncated message reference");
    assert!(truncated.raw_index >= snapshot.basis.raw_start && truncated.raw_index < snapshot.basis.raw_end);
    assert_eq!(snapshot.diagnostic_output, "partial report");
    let evidence = root.path().join("length");
    assert!(stop_snapshot_path(&evidence, 1).is_file());
    assert!(stop_review_index_path(&evidence, 1).is_file());
}

#[tokio::test]
async fn length_review_needs_changes_continues_in_the_same_session() {
    let root = tempfile::tempdir().unwrap();
    let provider =
        Provider::new(vec![write_answer("wrong"), length("cut"), write_answer("correct"), text("submitted")]);
    let host = host(root.path(), &provider, true);
    let outcome = wait(
        start_with_options(host.base.spec("repair"), host.base.request("original"), host.clone(), review_length())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.runs, 2);
    assert_eq!(host.stop_reviews.load(Ordering::SeqCst), 1);
    assert_eq!(host.base.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(root.path().join("answer.txt")).unwrap(), b"correct");
}

#[tokio::test]
async fn default_policy_and_unsupported_hosts_keep_reconciliation_required() {
    for (options, supports, snapshot) in [(TaskOptions::default(), true, false), (review_length(), false, true)] {
        let root = tempfile::tempdir().unwrap();
        let provider = Provider::new(vec![write_answer("correct"), length("cut")]);
        let host = host(root.path(), &provider, supports);
        let outcome = wait(
            start_with_options(host.base.spec("length"), host.base.request("original"), host.clone(), options).unwrap(),
        )
        .await;
        assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
        assert_eq!(outcome.runs, 1);
        assert_eq!(stop_snapshot_path(&root.path().join("length"), 1).is_file(), snapshot);
        let rows = events(&root.path().join("length/run-1-events.jsonl"));
        let end = rows.iter().find(|row| row["event"]["type"] == "agent_end").unwrap();
        assert_eq!(end["event"].get("stop_evidence").is_some(), snapshot, "evidence only when opted in");
    }
}
