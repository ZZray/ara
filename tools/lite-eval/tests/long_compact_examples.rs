//! `lite-eval long-compact` examples (requirements 25.5, 25.6): the counts
//! come from the events, a run without compaction is uncovered, and the
//! entry refuses to start without `budgets.LONG-COMPACT`.

mod runner_support;

use std::sync::atomic::Ordering;

use lite_eval::{
    long_compact::{check_counts, count_events, run, LongCompactInputs, LongCompactReport, Verdict},
    metrics::load_events,
    runner::HarnessExecutor,
};
use runner_support::{Provider, TestRepo};
use serde_json::json;

const VARIABLE: &str = "LITE_EVAL_LONG_COMPACT_KEY";

fn execute(repo: &TestRepo, task: &str) -> anyhow::Result<LongCompactReport> {
    execute_capped(repo, task, None)
}

fn execute_capped(repo: &TestRepo, task: &str, cap: Option<u64>) -> anyhow::Result<LongCompactReport> {
    let env = |name: &str| (name == VARIABLE).then(|| "long-compact-key-3c9e".to_owned());
    let evidence = repo.evidence("long-compact");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.block_on(run(LongCompactInputs {
        repo_root: &repo.root,
        evidence_root: &evidence,
        task_id: task,
        executor: &HarnessExecutor,
        env: &env,
        features: lite_eval::host::Features::V1,
        context_window_cap: cap,
    }))
}

fn with_budget(repo: &mut TestRepo, max_calls: usize) {
    repo.policy["budgets"]["LONG-COMPACT"] = json!({"max_runs": 2, "max_calls": max_calls,
        "deadline_seconds": 120, "run_max_calls": 4, "run_max_seconds": 60});
    repo.write_policy();
}

#[test]
fn a_task_without_compaction_is_uncovered_not_passed() {
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    with_budget(&mut repo, 6);
    let report = execute(&repo, "S1").unwrap();
    assert_eq!(report.exit, 1, "uncovered is not a pass");
    let check = &report.record.check;
    assert!(matches!(&check.verdict, Verdict::Uncovered { reason } if reason.contains("no compaction")));
    // The default script writes, then finishes: two agent calls, no summary.
    assert_eq!((check.summary_reservations, check.agent_reservations), (0, 2));
    assert_eq!(check.task_logical_calls, Some(check.reservations_total));
    assert_eq!(check.max_calls, 6, "the LONG-COMPACT budget is used");
    assert!(check.reservations_total <= check.max_calls);
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 2);
    assert!(report.record.session.before.is_empty());
    assert!(report.record.session.session_sha256.is_ok());
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&report.record_path).unwrap()).unwrap();
    assert_eq!(written["check"]["verdict"]["status"], "uncovered");
    assert_eq!(written["budget_key"], "LONG-COMPACT");
    assert!(written["context_window_cap"].is_null(), "no cap: the catalog window is used");
    // The loopback catalog declares 1_000_000 tokens.
    let limits = request_limits(&report);
    assert_eq!(limits.len(), 2);
    assert!(limits.iter().all(|(output, limit)| *limit == 1_000_000 - output - 4096), "{limits:?}");
    assert!(!std::fs::read_to_string(&report.record_path).unwrap().contains("long-compact-key-3c9e"));
}

/// `(effective output tokens, request byte limit)` of every agent call the
/// Core reserved, from the Harness events files.
fn request_limits(report: &LongCompactReport) -> Vec<(u64, u64)> {
    report
        .record
        .events
        .iter()
        .flat_map(|path| load_events(path).lines)
        .filter(|line| line.event["type"] == "model_call" && line.event["phase"] == "agent")
        .filter(|line| line.event["status"] == "reserved")
        .map(|line| {
            let output = line.event["output_limit"]["effective"].as_u64().expect("output_limit.effective");
            let limit = line.event["context_observation"]["request_limit_bytes"].as_u64().expect("request_limit_bytes");
            (output, limit)
        })
        .collect()
}

#[test]
fn a_context_window_cap_reaches_the_model_host_and_the_record() {
    const CAP: u64 = 131_072;
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    with_budget(&mut repo, 6);
    let report = execute_capped(&repo, "S1", Some(CAP)).unwrap();
    assert_eq!(report.record.context_window_cap, Some(CAP));
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&report.record_path).unwrap()).unwrap();
    assert_eq!(written["context_window_cap"], CAP);
    // Core derived the text budget from the capped window, not the 1_000_000 the catalog declares.
    let limits = request_limits(&report);
    assert_eq!(limits.len(), 2);
    assert!(limits.iter().all(|(output, limit)| *limit == CAP - output - 4096), "{limits:?}");
    assert_eq!(report.record.check.agent_reservations, 2, "the capped task still ran");
}

#[test]
fn a_zero_context_window_cap_starts_nothing() {
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    with_budget(&mut repo, 6);
    let error = execute_capped(&repo, "S1", Some(0)).err().expect("a zero window is refused");
    assert!(format!("{error:#}").contains("--context-window must be positive"), "{error:#}");
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn a_missing_long_compact_budget_starts_nothing() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let error = execute(&repo, "S1").err().expect("no LONG-COMPACT budget");
    assert!(format!("{error:#}").contains("/budgets/LONG-COMPACT"), "{error:#}");
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 0);
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 0);
}

/// A compaction is too expensive to provoke through the loopback provider;
/// these are the events a compacting two-Run task writes.
#[test]
fn summary_and_agent_reservations_sum_to_the_task_total() {
    let dir = tempfile::tempdir().unwrap();
    let envelope = |seq: u64, event: serde_json::Value| json!({"seq": seq, "event": event}).to_string();
    let reserve = |phase: &str| json!({"type": "model_call", "phase": phase, "call": 1, "status": "reserved"});
    let run1 = [
        envelope(1, reserve("agent")),
        envelope(2, reserve("agent")),
        envelope(3, reserve("summary")),
        envelope(4, json!({"type": "model_result", "phase": "summary", "call": 2, "status": "received"})),
        envelope(5, json!({"type": "compact", "status": "completed", "covered_prefix": 6, "raw_messages": 9})),
        envelope(6, reserve("agent")),
    ];
    let run2 = [envelope(1, reserve("summary")), envelope(2, reserve("agent"))];
    for (number, lines) in [(1, &run1[..]), (2, &run2[..])] {
        std::fs::write(dir.path().join(format!("run-{number}-events.jsonl")), lines.join("\n")).unwrap();
    }
    let files: Vec<_> =
        (1..=2).map(|number| load_events(&dir.path().join(format!("run-{number}-events.jsonl")))).collect();
    let counts = count_events(&files);
    assert_eq!((counts.summary_reservations, counts.agent_reservations), (2, 4));
    assert_eq!(counts.compactions.len(), 1);
    assert_eq!((counts.compactions[0].run, counts.compactions[0].covered_prefix), (1, 6));

    let check = check_counts(&counts, Some(6), 80);
    assert_eq!(check.verdict, Verdict::Passed);
    assert_eq!(check.reservations_total, 6);
    // A Harness total that differs from the reservations, or a sum above
    // max_calls, fails even though compaction happened.
    assert!(matches!(check_counts(&counts, Some(5), 80).verdict, Verdict::Failed { .. }));
    assert!(matches!(check_counts(&counts, Some(6), 5).verdict, Verdict::Failed { .. }));
}
