mod common;
use ara_agent_lite_harness::*;
use common::*;
use serde_json::Value;
use std::sync::{atomic::Ordering, Arc};

#[tokio::test]
async fn actual_file_feedback_repair_reuses_session_and_original_call_budget() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        write_answer("partial"),
        text("First submission."),
        write_answer("correct"),
        text("Repaired submission."),
    ]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let handle = start(host.spec("repair"), host.request("initial"), host.clone()).unwrap();
    let outcome = handle.wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.runs, 2);
    assert_eq!(outcome.observed_calls, 4);
    assert!(!outcome.task_accepted);
    assert!(outcome.reconciliation_fence.is_none());
    assert_eq!(host.tool.writes.load(Ordering::SeqCst), 2);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 2);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(directory.path().join("answer.txt")).unwrap(), b"correct");
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
    let candidate = outcome.last_candidate.as_ref().unwrap();
    assert_eq!(
        candidate.basis.session_path.canonicalize().unwrap(),
        directory.path().join("session.json").canonicalize().unwrap()
    );
    let rows = events(&candidate.basis.events_path);
    assert!(rows.iter().any(|row| row["event"]["type"] == "tool_end"));
    assert!(directory.path().join("repair/released.json").is_file());
}

#[tokio::test]
async fn wrong_uncommitted_and_unknown_receipts_stop_without_second_run() {
    for fault in [Fault::BadBasis, Fault::Uncommitted, Fault::Unknown] {
        let directory = tempfile::tempdir().unwrap();
        let provider = Provider::new(vec![write_answer("partial"), text("Submitted.")]);
        let host = Arc::new(Host::new(directory.path(), &provider, fault));
        let outcome = start(host.spec("case"), host.request("initial"), host.clone()).unwrap().wait().await;
        assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
        assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
        let fence = outcome.reconciliation_fence.as_ref().unwrap();
        assert!(fence.is_file());
        assert!(start(host.spec("another"), host.request("another-run"), host.clone()).is_err());
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn changed_session_or_repeated_run_id_is_rejected_without_replaying_tools() {
    for fault in [Fault::ChangedSession, Fault::RepeatId] {
        let directory = tempfile::tempdir().unwrap();
        let provider = Provider::new(vec![write_answer("partial"), text("Submitted.")]);
        let host = Arc::new(Host::new(directory.path(), &provider, fault));
        let outcome = start(host.spec("case"), host.request("initial"), host.clone()).unwrap().wait().await;
        assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
        assert_eq!(host.tool.writes.load(Ordering::SeqCst), 1);
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
        assert!(!directory.path().join("different-session.json").exists());
    }
}

#[tokio::test]
async fn cancellation_joins_the_original_host_review_before_releasing_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::SlowReview));
    let handle = start(host.spec("cancel"), host.request("initial"), host.clone()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(4), async {
        while !host.review_started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    handle.cancel();
    assert!(start(host.spec("collision"), host.request("collision-run"), host.clone()).is_err());
    let outcome = handle.wait().await;
    assert!(host.review_joined.load(Ordering::SeqCst));
    assert_ne!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancellation_during_real_core_tool_waits_for_settlement_and_never_replays() {
    use std::time::Duration;

    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("inflight-operation", "delayed_cancel", serde_json::json!({})),
        write_answer("must-not-be-admitted"),
        text("Must not become a review candidate."),
    ]);
    let tool = Arc::new(DelayedCancelTool::new(directory.path()));
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let mut request = host.request("initial-inflight");
    request.objective = "Call delayed_cancel exactly once; await its actual result before doing anything else.".into();
    let handle = start(host.spec("tool-cancel"), request, host.clone()).unwrap();

    tokio::time::timeout(Duration::from_secs(4), async {
        while !tool.started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(directory.path().join("inflight-started.json").is_file());
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    handle.cancel();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !tool.cancellation_seen.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    // Only this waiter expires. The same real Core/tool future remains owned
    // and must keep the Session lock until its delayed callback has joined.
    assert!(tokio::time::timeout(Duration::from_millis(40), handle.wait()).await.is_err());
    assert!(!tool.joined.load(Ordering::SeqCst));
    assert!(!directory.path().join("inflight-joined.json").exists());
    assert!(start(host.spec("inflight-contender"), host.request("contender"), host.clone()).is_err());
    assert!(!directory.path().join("inflight-contender").exists());
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);

    tool.release_settlement();
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle.wait()).await.unwrap();
    assert!(tool.joined.load(Ordering::SeqCst), "Harness returned before the original tool settled");
    assert!(directory.path().join("inflight-joined.json").is_file());
    assert_eq!(outcome.stop, TaskStop::Cancelled, "{outcome:?}");
    assert_eq!(outcome.runs, 1);
    assert_eq!(outcome.observed_calls, 1);
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(outcome.reconciliation_fence.as_ref().unwrap().is_file());
    assert!(outcome.last_candidate.is_none());
    assert!(outcome.last_receipt.is_none());
    assert!(!outcome.task_accepted);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(host.tool.writes.load(Ordering::SeqCst), 0);
    assert!(!directory.path().join("answer.txt").exists());
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(start(host.spec("after-cancel"), host.request("do-not-replay"), host).is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn dropped_waiter_and_absent_ui_subscriber_do_not_drop_the_worker() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let handle = start(host.spec("drop"), host.request("initial"), host).unwrap();
    let owner = handle.clone();
    let waiter = tokio::spawn(async move { owner.wait().await });
    waiter.abort();
    let _ = waiter.await;
    let outcome = handle.wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.runs, 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cumulative_budget_is_not_refilled_after_needs_changes() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        write_answer("partial"),
        text("Submitted."),
        write_answer("correct"),
        text("Never admitted."),
    ]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let mut spec = host.spec("budget");
    spec.max_calls = 2;
    let outcome = start(spec, host.request("initial"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::BudgetExhausted, "{outcome:?}");
    assert_eq!(outcome.observed_calls, 2);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(std::fs::read(directory.path().join("answer.txt")).unwrap(), b"partial");
}

#[tokio::test]
async fn expired_deadline_refuses_admission_without_provider_or_tool_effects() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let mut spec = host.spec("expired");
    spec.deadline_unix_ms = now_ms() - 1;
    assert!(start(spec, host.request("initial"), host).is_err());
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(!directory.path().join("answer.txt").exists());
}

#[tokio::test]
async fn length_is_not_an_artifact_candidate_or_an_automatic_replay() {
    let directory = tempfile::tempdir().unwrap();
    let mut length = text("incomplete");
    length["finish"] = Value::String("length".into());
    let provider = Provider::new(vec![length, write_answer("correct")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let outcome = start(host.spec("length"), host.request("initial"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn empty_feedback_never_starts_a_fresh_run() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("partial"), text("Submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::EmptyFeedback));
    let outcome = start(host.spec("empty"), host.request("initial"), host.clone()).unwrap().wait().await;
    assert_ne!(outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn successful_tool_evidence_text_is_not_promoted_to_unknown_effect_control() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("read", "read_terms", serde_json::json!({})),
        write_answer("correct"),
        text("Submitted."),
    ]);
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(Arc::new(ReadTerms::new(false)));
    let host = Arc::new(host);
    let outcome = start(host.spec("text-data"), host.request("initial"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(host.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn native_structured_unknown_receipt_cannot_be_overridden_by_host_verdict() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("unknown", "read_terms", serde_json::json!({})),
        write_answer("correct"),
        text("Submitted."),
    ]);
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(Arc::new(ReadTerms::new(true)));
    let host = Arc::new(host);
    let outcome = start(host.spec("unknown"), host.request("initial"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert!(outcome.reconciliation_fence.as_ref().unwrap().is_file());
}

#[tokio::test]
async fn committed_review_cannot_accept_mutated_event_evidence() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::TamperEvents));
    let outcome = start(host.spec("event-change"), host.request("initial"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn real_observation_commit_failure_preserves_original_effects_and_fences_replay() {
    let directory = tempfile::tempdir().unwrap();
    let evidence = directory.path().join("observation-failure");
    let provider = Provider::new(vec![
        call("write-once", "write_with_observation_collision", serde_json::json!({"content":"correct"})),
        text("Actual artifact submitted; Core can now complete its original Run."),
        write_answer("must-never-replay"),
    ]);
    let tool = Arc::new(ObservationCollisionTool::new(directory.path(), &evidence));
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let mut request = host.request("observation-original-run");
    request.objective = "Call write_with_observation_collision once with content correct, then finish.".into();
    let handle = start(host.spec("observation-failure"), request, host.clone()).unwrap();
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(8), handle.wait()).await.unwrap();

    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
    assert_eq!(outcome.runs, 1);
    assert_eq!(outcome.fence_presence, FencePresence::Present);
    assert!(outcome.reconciliation_fence.as_ref().unwrap().is_file());
    assert!(outcome.last_candidate.is_none());
    assert!(outcome.last_receipt.is_none());
    assert!(!outcome.task_accepted);
    assert_eq!(host.reviews.load(Ordering::SeqCst), 0);
    assert_eq!(host.preparations.load(Ordering::SeqCst), 0);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert_eq!(tool.writes.load(Ordering::SeqCst), 1);
    assert_eq!(host.tool.writes.load(Ordering::SeqCst), 0);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(std::fs::read(directory.path().join("answer.txt")).unwrap(), b"correct");
    assert!(evidence.join("run-1-observation.json").is_dir());
    assert!(!evidence.join("run-1-candidate.json").exists());
    assert!(!evidence.join("released.json").exists());

    let events_path = evidence.join("run-1-events.jsonl");
    let original_events = std::fs::read(&events_path).unwrap();
    let rows = events(&events_path);
    assert!(rows.iter().any(|row| row["event"]["type"] == "tool_end"));
    assert!(rows.iter().any(|row| row["event"]["type"] == "model_result" && row["event"]["stop_reason"] == "stop"));
    assert!(rows.iter().any(|row| row["event"]["type"] == "agent_end" && row["event"]["status"] == "completed"));
    let journal = core::session_journal::Journal::for_session(&directory.path().join("session.json"));
    assert!(journal.exists());
    let head = journal.open().unwrap();
    assert!(head.raw_count > 0);
    assert!(head.metadata.get("pending_run").is_some_and(Value::is_null));
    assert!(journal.contains_run("observation-original-run").unwrap());
    let original_proof = journal.proof(0, head.raw_count).unwrap();

    assert!(start(host.spec("after-observation-failure"), host.request("do-not-replay"), host.clone()).is_err());
    assert!(!directory.path().join("after-observation-failure").exists());
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert_eq!(tool.writes.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(&events_path).unwrap(), original_events);
    assert_eq!(journal.open().unwrap().raw_count, head.raw_count);
    assert_eq!(journal.proof(0, head.raw_count).unwrap(), original_proof);
    assert_eq!(std::fs::read(directory.path().join("answer.txt")).unwrap(), b"correct");
}
