//! Baseline record examples (requirements 7.4, 7.6, 7.7).

mod runner_support;

use async_trait::async_trait;
use lite_eval::{
    baseline::{BaselineIndex, BaselineStatus, CheckStatus, BASELINE_INDEX_DIR},
    compare::{compare, load, BaselineValidity, Conclusions},
    manifest::{ManifestStatus, TaskState},
    runner::{run, Correctness, RunInputs, RunReport, Selection, TaskContext, TaskExecutor, TaskRecord},
};
use runner_support::{Provider, TestRepo};
use serde_json::Value;

const VARIABLE: &str = "LITE_EVAL_BASELINE_KEY";

/// Writes one artifact per task and returns an Oracle judgement; tasks in
/// `fail` are judged failed.
struct Judged {
    fail: Vec<&'static str>,
}

#[async_trait]
impl TaskExecutor for Judged {
    async fn execute(&self, context: &TaskContext) -> TaskRecord {
        std::fs::write(context.task_dir.join("artifact.txt"), context.entry.task_id.as_bytes()).unwrap();
        let mut record = TaskRecord::not_run("controlled".into());
        record.error = None;
        record.logical_calls = Some(0);
        record.wall_ms = Some(1);
        let passed = !self.fail.contains(&context.entry.task_id.as_str());
        record.correctness =
            Correctness::Judged { passed, public: None, hidden: None, holdout: None, raw_output: None };
        record
    }
}

fn execute(repo: &TestRepo, name: &str, baseline: bool, executor: &Judged) -> RunReport {
    let evidence = repo.evidence(name);
    let env = |variable: &str| (variable == VARIABLE).then(|| "baseline-key-4e2a".to_owned());
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime
        .block_on(run(RunInputs {
            repo_root: &repo.root,
            evidence_root: &evidence,
            selection: &Selection::All,
            executor,
            env: &env,
            probe_only: false,
            baseline,
        }))
        .unwrap()
}

fn index(repo: &TestRepo, report: &RunReport) -> BaselineIndex {
    let path = repo.root.join(BASELINE_INDEX_DIR).join(format!("{}.json", report.manifest.manifest_id));
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_refused_task_makes_an_incomplete_baseline() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    std::fs::write(repo.fixtures.join("S2").join("README.md"), "changed\n").unwrap();
    let report = execute(&repo, "incomplete", true, &Judged { fail: Vec::new() });
    assert_eq!(report.manifest.status, ManifestStatus::IncompleteBaseline);
    assert!(report.manifest_path.starts_with(repo.root.parent().unwrap().join("evidence/incomplete/baseline")));
    let mark = report.manifest.baseline.as_ref().unwrap();
    assert!(!mark.usable);
    assert_eq!(mark.missing_judgement.len(), 1);
    assert_eq!(mark.missing_judgement[0].task_id, "S2");
    assert!(mark.missing_judgement[0].reason.contains("refused"));
    let index = index(&repo, &report);
    assert_eq!(index.status, BaselineStatus::IncompleteBaseline);
    assert_eq!(index.missing_judgement, mark.missing_judgement);

    let loaded = load(&report.manifest_path).unwrap();
    let record = compare(std::slice::from_ref(&loaded), std::slice::from_ref(&loaded), "P2-a");
    assert_eq!(record.conclusions, Conclusions::Invalid, "an incomplete baseline is not a Baseline_Record");
}

#[test]
fn failed_tasks_are_valid_results_files_are_reverified_and_p2_code_invalidates() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    // An uncommitted change: the complete diff must be kept and match.
    std::fs::write(repo.root.join("untracked-change.txt"), "dirty\n").unwrap();
    let report = execute(&repo, "baseline", true, &Judged { fail: vec!["S3"] });
    assert_eq!(report.manifest.status, ManifestStatus::Completed);
    assert!(report.manifest.baseline.as_ref().unwrap().usable, "an Oracle failure is a valid baseline result");
    let s3 = report.manifest.selected.iter().find(|record| record.task_id == "S3").unwrap();
    assert_eq!(s3.state, TaskState::Failed);
    let index = index(&repo, &report);
    assert_eq!(index.status, BaselineStatus::Baseline);
    assert_eq!(index.diff_matches_manifest, Some(true));
    let names: Vec<&str> = index.files.iter().map(|file| file.path.as_str()).collect();
    for name in [
        "manifest.json",
        "code-version/worktree.diff",
        "tasks/S3/failure.json",
        "tasks/S3/result.json",
        "tasks/S1/artifact.txt",
    ] {
        assert!(names.contains(&name), "{name} not in {names:?}");
    }
    assert!(report.manifest.baseline_checks.is_empty(), "no earlier baseline");

    // A later run re-verifies the baseline files.
    let later = execute(&repo, "later", false, &Judged { fail: Vec::new() });
    assert_eq!(later.manifest.baseline_checks.len(), 1);
    assert_eq!(later.manifest.baseline_checks[0].status, CheckStatus::Intact, "{:?}", later.manifest.baseline_checks);
    let artifact = index.run_dir.join("tasks/S1/artifact.txt");
    std::fs::write(&artifact, "tampered").unwrap();
    let tampered = execute(&repo, "tampered", false, &Judged { fail: Vec::new() });
    let check = &tampered.manifest.baseline_checks[0];
    assert_eq!(check.status, CheckStatus::Changed);
    assert_eq!(check.problems, ["tasks/S1/artifact.txt changed"]);
    std::fs::write(&artifact, "S1").unwrap();

    // Against this baseline the comparison has valid conclusions …
    let base = load(&report.manifest_path).unwrap();
    let current = load(&later.manifest_path).unwrap();
    let record = compare(std::slice::from_ref(&base), std::slice::from_ref(&current), "P2-a");
    assert_eq!(record.conclusions, Conclusions::Valid, "{:?}", record.baseline_validity);
    let s3 = record.tasks.iter().find(|task| task.task_id == "S3").unwrap();
    assert_eq!(s3.metrics["success"].baseline, Value::Bool(false));

    // … but a baseline whose code includes a P2 item invalidates them all.
    let mut value: Value = serde_json::from_slice(&std::fs::read(&report.manifest_path).unwrap()).unwrap();
    value["code_version"]["capability_host_features"] = serde_json::json!(["P2-a"]);
    let with_p2 = repo.root.parent().unwrap().join("with-p2.json");
    std::fs::write(&with_p2, serde_json::to_vec(&value).unwrap()).unwrap();
    let record = compare(&[load(&with_p2).unwrap()], std::slice::from_ref(&current), "P2-b");
    assert_eq!(record.conclusions, Conclusions::Invalid);
    let BaselineValidity::Invalid { included_p2_items, .. } = &record.baseline_validity else { panic!() };
    assert_eq!(included_p2_items, &["P2-a"]);
}
