//! Runner examples (requirements 1.3, 1.4, 3.4, 3.7, 3.8): refusal of one
//! task, environment unavailable, start failures without a Run_Manifest.

mod runner_support;

use std::sync::atomic::Ordering;

use lite_eval::{
    manifest::{ManifestStatus, ProbeRecord, TaskState},
    runner::{run, HarnessExecutor, RunInputs, RunReport, Selection},
};
use runner_support::{files, git, ProbeMode, Provider, TestRepo};

const VARIABLE: &str = "LITE_EVAL_EXAMPLES_KEY";

fn execute(
    repo: &TestRepo,
    evidence: &std::path::Path,
    selection: Selection,
    key: Option<&str>,
) -> anyhow::Result<RunReport> {
    execute_with(repo, evidence, selection, key, false)
}

fn execute_with(
    repo: &TestRepo,
    evidence: &std::path::Path,
    selection: Selection,
    key: Option<&str>,
    probe_only: bool,
) -> anyhow::Result<RunReport> {
    let env = |name: &str| (name == VARIABLE).then(|| key.map(str::to_owned)).flatten();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    runtime.block_on(run(RunInputs {
        repo_root: &repo.root,
        evidence_root: evidence,
        selection: &selection,
        executor: &HarnessExecutor,
        env: &env,
        probe_only,
        baseline: false,
        features: lite_eval::host::Features::V1,
    }))
}

#[test]
fn probe_only_stops_after_a_successful_probe() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let evidence = repo.evidence("probe-only");
    let report = execute_with(&repo, &evidence, Selection::All, Some("examples-key-5a1f0c"), true).unwrap();
    assert_eq!(report.exit, 0);
    assert_eq!(report.manifest.status, ManifestStatus::ProbeOnly);
    assert!(matches!(report.manifest.probe, ProbeRecord::Ok { .. }));
    assert!(report.manifest.selected.is_empty());
    assert!(!report.manifest_path.parent().unwrap().join("tasks").exists());
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 1);
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 0);
}

fn ids(list: &[&str]) -> Selection {
    Selection::Ids(list.iter().map(|id| id.to_string()).collect())
}

#[test]
fn a_changed_fixture_refuses_only_that_task() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    // Changed after registration: determinable but different (1.4, not 3.8).
    std::fs::write(repo.fixtures.join("S2").join("README.md"), "changed\n").unwrap();
    let evidence = repo.evidence("refuse");
    let report = execute(&repo, &evidence, ids(&["S1", "S2", "S3"]), Some("examples-key")).unwrap();
    assert_eq!(report.exit, 1, "a refused task is not a pass");
    let states: Vec<_> = report.manifest.selected.iter().map(|r| (r.task_id.as_str(), r.state)).collect();
    assert_eq!(states, [("S1", TaskState::Passed), ("S2", TaskState::Refused), ("S3", TaskState::Passed)]);
    let refusal = report.manifest.selected[1].refusal.as_ref().unwrap();
    let registered = repo.set["tasks"][1]["fixture_sha256"].as_str().unwrap();
    assert_eq!(refusal.expected_sha256, registered);
    let actual = lite_eval::digest::tree_digest(&repo.fixtures.join("S2")).unwrap().sha256;
    assert_eq!(refusal.actual_sha256.as_deref(), Some(actual.as_str()));
    assert_eq!(report.manifest.summary.refused, 1);
    let s2 = report.manifest_path.parent().unwrap().join("tasks").join("S2");
    assert!(files(&s2).is_empty(), "nothing ran for the refused task");
    // Two tasks, two model calls each (write, then finish); one probe.
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 4);
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 1);
    let all = &report.manifest.all_tasks;
    assert_eq!(all.len(), 6);
    assert_eq!(all[1].fixture_sha256.as_deref(), Some(actual.as_str()));
}

#[test]
fn an_unavailable_environment_writes_no_task_record_and_exits_3() {
    let provider = Provider::standard();
    *provider.probe_mode.lock().unwrap() = ProbeMode::Reply { status: 401, body: "{}".into(), delay_ms: 0 };
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let evidence = repo.evidence("unavailable");
    let report = execute(&repo, &evidence, Selection::All, Some("examples-key")).unwrap();
    assert_eq!(report.exit, 3);
    assert_ne!(report.exit, 1, "differs from a task failure");
    assert_eq!(report.manifest.status, ManifestStatus::EnvironmentUnavailable);
    assert!(matches!(report.manifest.probe, ProbeRecord::Failed { category: "auth_rejected", .. }));
    assert!(report.manifest.selected.is_empty());
    assert!(!report.manifest_path.parent().unwrap().join("tasks").exists());
    assert_eq!(provider.model_requests.load(Ordering::SeqCst), 0);

    // A missing credential: no probe request at all, same exit.
    let report = execute(&repo, &evidence, Selection::All, None).unwrap();
    assert_eq!(report.exit, 3);
    assert!(matches!(report.manifest.probe, ProbeRecord::Failed { category: "credential_missing", .. }));
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 1, "only the first run sent a probe");
}

#[test]
fn start_failures_write_no_manifest() {
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let evidence = repo.evidence("start");
    let no_manifest = || files(&evidence).iter().all(|path| !path.ends_with("manifest.json"));

    let error = execute(&repo, &evidence, ids(&["S1", "X9"]), Some("examples-key-5a1f0c")).unwrap_err();
    assert!(format!("{error:#}").contains("X9"), "{error:#}");
    assert!(no_manifest());

    let mut broken = repo.policy.clone();
    broken["model"].as_object_mut().unwrap().remove("id");
    broken["budgets"].as_object_mut().unwrap().remove("S3");
    let saved = std::mem::replace(&mut repo.policy, broken);
    repo.write_policy();
    let error = format!("{:#}", execute(&repo, &evidence, Selection::All, Some("examples-key-5a1f0c")).unwrap_err());
    assert!(error.contains("/model/id"), "{error}");
    repo.policy["model"]["id"] = saved["model"]["id"].clone();
    repo.write_policy();
    let error = format!("{:#}", execute(&repo, &evidence, ids(&["S3"]), Some("examples-key-5a1f0c")).unwrap_err());
    assert!(error.contains("/budgets/S3"), "{error}");
    repo.policy = saved;
    repo.write_policy();

    // A selected fixture that cannot be read (3.8) versus an unselected one.
    let s1 = repo.fixtures.join("S1");
    std::fs::rename(&s1, repo.fixtures.join("S1-moved")).unwrap();
    let error = format!("{:#}", execute(&repo, &evidence, ids(&["S1"]), Some("examples-key-5a1f0c")).unwrap_err());
    assert!(error.contains("fixture SHA-256 of selected task S1"), "{error}");
    std::fs::rename(repo.fixtures.join("S1-moved"), &s1).unwrap();
    assert!(no_manifest());
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn an_undeterminable_code_version_exits_before_the_probe() {
    let provider = Provider::standard();
    let repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    let evidence = repo.evidence("code-version");
    // A repository without any commit has no HEAD.
    std::fs::remove_dir_all(repo.root.join(".git")).unwrap();
    git(&repo.root, &["init", "-q"]);
    let error = format!("{:#}", execute(&repo, &evidence, Selection::All, Some("examples-key-5a1f0c")).unwrap_err());
    assert!(error.contains("code version"), "{error}");
    assert!(files(&evidence).is_empty());
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst), 0);
}

#[test]
fn an_invalid_task_set_is_recorded_and_exits_4_without_requests() {
    let provider = Provider::standard();
    let mut repo = TestRepo::new(&provider.endpoint, VARIABLE, &|_| "correct".into());
    repo.set["tasks"].as_array_mut().unwrap().truncate(5);
    repo.set["tasks"][0]["oracle"]["type"] = serde_json::json!("");
    repo.write_set();
    let evidence = repo.evidence("invalid");
    let report = execute(&repo, &evidence, ids(&["S1"]), Some("examples-key-5a1f0c")).unwrap();
    assert_eq!(report.exit, 4);
    assert_eq!(report.manifest.status, ManifestStatus::TaskSetInvalid);
    let violations = serde_json::to_value(&report.manifest.task_set.violations).unwrap().to_string();
    assert!(violations.contains("task_count") && violations.contains("oracle.type"), "{violations}");
    assert!(matches!(report.manifest.probe, ProbeRecord::NotSent));
    assert_eq!(provider.probe_requests.load(Ordering::SeqCst) + provider.model_requests.load(Ordering::SeqCst), 0);
}
