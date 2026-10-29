//! Failure_Record examples (requirements 5.4, 5.6, 5.7): injected provider
//! error, missing tool and budget exhaustion through the real Harness and
//! Core; an Oracle that never ran; a Failure_Record that cannot be written;
//! manual classification into a new file.

mod runner_support;

use std::{path::PathBuf, sync::Arc};

use async_trait::async_trait;
use lite_eval::{
    failure::{classify, current_class, EvidenceRef, FailureClass, FailureRecord},
    manifest::TaskState,
    metrics::load_events,
    runner::{run, HarnessExecutor, RunInputs, RunReport, Selection, TaskContext, TaskExecutor, TaskRecord},
};
use runner_support::{call, text, turn, Provider, TestRepo};
use serde_json::{json, Value};

const VARIABLE: &str = "LITE_EVAL_FAILURE_KEY";

fn execute(repo: &TestRepo, name: &str, task: &str, executor: &dyn TaskExecutor) -> RunReport {
    let evidence = repo.evidence(name);
    let env = |variable: &str| (variable == VARIABLE).then(|| "failure-key-93be".to_owned());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime
        .block_on(run(RunInputs {
            repo_root: &repo.root,
            evidence_root: &evidence,
            selection: &Selection::Ids(vec![task.to_owned()]),
            executor,
            env: &env,
            probe_only: false,
            baseline: false,
        }))
        .unwrap()
}

fn record(report: &RunReport) -> (PathBuf, FailureRecord, Value) {
    let selected = &report.manifest.selected[0];
    assert_eq!(selected.state, TaskState::Failed);
    let path = selected.failure_record.clone().expect("Failure_Record written");
    let record: FailureRecord = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let result: Value = serde_json::from_slice(&std::fs::read(selected.result.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(result["failure_record"], json!(path));
    // 5.4: the stop reason is the one the Harness reported for the task.
    assert_eq!(serde_json::to_value(record.stop).unwrap(), result["stop"]);
    assert!(record.last_events.iter().map(|group| group.seq.len()).sum::<usize>() <= 20);
    assert_eq!(record.last_events.is_empty(), result["paths"]["events"][0]["status"] == "missing");
    (path, record, result)
}

/// The event an evidence reference points at.
fn referenced(reference: &EvidenceRef) -> Value {
    let EvidenceRef::Event { events_path, seq } = reference else { panic!("not an event reference: {reference:?}") };
    load_events(events_path).lines.into_iter().find(|line| line.seq == *seq).expect("event exists").event
}

#[test]
fn injected_provider_error_is_a_wiring_gap_at_the_failing_model_result() {
    let provider = Provider::new(Arc::new(|_: &Value| json!({"http_status": 500})));
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let (_, record, _) = record(&execute(&repo, "provider", "S1", &HarnessExecutor));
    let lite_eval::failure::Classification::Classified { class, evidence } = &record.classification else {
        panic!("{record:#?}")
    };
    assert_eq!(*class, FailureClass::WiringGap);
    let event = referenced(&evidence[0]);
    assert_eq!((event["type"].as_str(), event["stop_reason"].as_str()), (Some("model_result"), Some("error")));
    assert!(record.oracle.executed, "the Oracle still judged the artifacts");
}

#[test]
fn injected_missing_tool_is_a_tool_gap_at_its_tool_end() {
    let provider = Provider::new(Arc::new(|body: &Value| match turn(body) {
        0 => call("x1", "ws_delete_everything", json!({"path": "."})),
        _ => text("I could not finish."),
    }));
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let report = execute(&repo, "tool", "S1", &HarnessExecutor);
    let (path, record, _) = record(&report);
    let lite_eval::failure::Classification::Classified { class, evidence } = &record.classification else {
        panic!("{record:#?}")
    };
    assert_eq!(*class, FailureClass::ToolGap);
    let event = referenced(&evidence[0]);
    assert_eq!((event["type"].as_str(), event["title"].as_str()), (Some("tool_end"), Some("ws_delete_everything")));
    assert_eq!(record.oracle.hidden.map(|counts| (counts.passed, counts.total)), Some((0, 1)));
    assert!(record.oracle.raw_output.as_ref().unwrap().exists());

    // Manual classification goes to new files; the record is never touched.
    let before = std::fs::read(&path).unwrap();
    let events_path = record.last_events[0].events_path.clone();
    let seq = record.last_events[0].seq[0];
    let raw = record.oracle.raw_output.clone().unwrap();
    let first = classify(
        &path,
        FailureClass::ModelGap,
        vec![EvidenceRef::File { path: raw.clone() }, EvidenceRef::Event { events_path: events_path.clone(), seq }],
        &Default::default(),
    )
    .unwrap();
    assert!(first.ends_with("failure.classification-1.json"));
    assert_eq!(current_class(&path).as_deref(), Some("model_gap"));
    let missing = EvidenceRef::Event { events_path: events_path.clone(), seq: 99_999 };
    let error = classify(&path, FailureClass::CoreDefect, vec![missing], &Default::default()).unwrap_err();
    assert!(format!("{error:#}").contains("99999"), "{error:#}");
    assert!(classify(&path, FailureClass::CoreDefect, Vec::new(), &Default::default()).is_err());
    let second =
        classify(&path, FailureClass::ToolGap, vec![EvidenceRef::File { path: raw }], &Default::default()).unwrap();
    assert!(second.ends_with("failure.classification-2.json"));
    assert!(!path.with_file_name("failure.classification-3.json").exists(), "rejected requests wrote nothing");
    assert_eq!(current_class(&path).as_deref(), Some("tool_gap"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let parsed = EvidenceRef::parse(&format!("event:{}#{seq}", events_path.display())).unwrap();
    assert_eq!(parsed, EvidenceRef::Event { events_path, seq });
}

#[test]
fn injected_budget_exhaustion_is_pending_and_references_the_closing_event() {
    // The Host's public check always fails, so the Host requests changes
    // after Run 1; with one Run allowed the Task budget is exhausted.
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "other".into());
    repo.set["tasks"][0]["profile"]["public_check"] = json!(["python", "-c", "import sys; sys.exit(1)"]);
    repo.write_set();
    repo.policy["budgets"]["S1"] =
        json!({"max_runs": 1, "max_calls": 4, "deadline_seconds": 120, "run_max_calls": 4, "run_max_seconds": 60});
    repo.write_policy();
    let (_, record, result) = record(&execute(&repo, "budget", "S1", &HarnessExecutor));
    assert_eq!(result["stop"], "BudgetExhausted");
    let lite_eval::failure::Classification::Pending { missing } = &record.classification else { panic!("{record:#?}") };
    assert!(missing.iter().any(|item| item.contains("budget")), "{missing:?}");
    let group = record.last_events.last().unwrap();
    let file = load_events(&group.events_path);
    let end = file.lines.iter().rev().find(|line| line.event["type"] == "agent_end").expect("agent_end").seq;
    assert!(group.seq.contains(&end), "the Run's closing event is referenced");
    let all: Vec<u64> = file.lines.iter().map(|line| line.seq).collect();
    assert_eq!(group.seq, all[all.len().saturating_sub(20)..]);
}

/// Ends every task without running the Harness; optionally makes the
/// Failure_Record path unwritable first.
struct NotRun {
    block_failure_record: bool,
}

#[async_trait]
impl TaskExecutor for NotRun {
    async fn execute(&self, context: &TaskContext) -> TaskRecord {
        if self.block_failure_record {
            std::fs::create_dir(context.task_dir.join("failure.json")).unwrap();
        }
        TaskRecord::not_run("Harness did not start: controlled".into())
    }
}

#[test]
fn an_oracle_that_never_ran_and_an_unwritable_failure_record() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let report = execute(&repo, "not-run", "S1", &NotRun { block_failure_record: false });
    let (_, record, result) = record(&report);
    assert!(!record.oracle.executed);
    assert!(record.oracle.reason.as_deref().unwrap().contains("Harness did not start"), "{record:#?}");
    assert!(record.oracle.hidden.is_none() && record.oracle.public.is_none());
    assert_eq!(result["paths"]["session"]["status"], "missing");
    assert!(matches!(record.classification, lite_eval::failure::Classification::Pending { .. }));

    let report = execute(&repo, "unwritable", "S1", &NotRun { block_failure_record: true });
    assert_eq!(report.exit, 1);
    let selected = &report.manifest.selected[0];
    assert_eq!(selected.state, TaskState::FailureRecordMissing);
    assert!(selected.failure_record.is_none());
    assert!(selected.failure_record_error.as_deref().unwrap().contains("Failure_Record"));
    assert_eq!((report.manifest.summary.passed, report.manifest.summary.failure_record_missing), (0, 1));
    let manifest: Value = serde_json::from_slice(&std::fs::read(&report.manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["selected"][0]["state"], "failure_record_missing");
    let result: Value = serde_json::from_slice(&std::fs::read(selected.result.as_ref().unwrap()).unwrap()).unwrap();
    assert_eq!(result["failure_record"]["status"], "missing");
}
