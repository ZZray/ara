//! Exercise independent CLI judging across process and workspace boundaries.
mod runner_support;

use lite_eval::workspace::{snapshot, Workspace};
use runner_support::TestRepo;
use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn judge(repo: &TestRepo, task: &str, artifacts: &Path, out: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lite-eval"))
        .current_dir(&repo.root)
        .args(["oracle", task, "--artifacts"])
        .arg(artifacts)
        .arg("--out")
        .arg(out)
        .output()
        .unwrap()
}

#[test]
fn judging_distinguishes_pass_and_failure_without_changing_artifacts() {
    let repo = TestRepo::new("http://127.0.0.1:1/v1", "ORACLE_UNUSED_KEY", &|_| "correct".into());
    let artifacts = repo.fixtures.join("S1");
    let evidence = repo.evidence("judge");
    let workspace = Workspace { root: artifacts.clone(), private: evidence.clone() };
    for (answer, expected) in [("correct", 0), ("wrong", 1)] {
        fs::write(artifacts.join("answer.txt"), answer).unwrap();
        let before = snapshot(&workspace).unwrap();
        let out = evidence.join(format!("{answer}.json"));
        let output = judge(&repo, "S1", &artifacts, &out);
        assert_eq!(output.status.code(), Some(expected), "{}", String::from_utf8_lossy(&output.stderr));
        let record: Value = serde_json::from_slice(&fs::read(&out).unwrap()).unwrap();
        assert_eq!(record["correctness"]["passed"], expected == 0);
        assert_eq!(record["workspace_unchanged"], true);
        assert_eq!(snapshot(&workspace).unwrap(), before);
        assert_eq!(judge(&repo, "S1", &artifacts, &out).status.code(), Some(2));
        assert_eq!(serde_json::from_slice::<Value>(&fs::read(out).unwrap()).unwrap(), record);
    }
}

#[test]
fn invalid_selection_text_and_overlapping_output_fail_before_judging() {
    let mut repo = TestRepo::new("http://127.0.0.1:1/v1", "ORACLE_UNUSED_KEY", &|_| "correct".into());
    let artifacts = repo.fixtures.join("S1");
    let out = repo.evidence("invalid").join("result.json");
    assert_eq!(judge(&repo, "unknown", &artifacts, &out).status.code(), Some(2));
    assert!(!out.exists());
    repo.set["tasks"][0]["kind"] = "ReverseAnalysis".into();
    repo.write_set();
    assert_eq!(judge(&repo, "S1", &artifacts, &out).status.code(), Some(2));
    assert!(!out.exists());
    repo.set["tasks"][0]["kind"] = "Coding".into();
    repo.write_set();
    let nested = artifacts.join("result.json");
    assert_eq!(judge(&repo, "S1", &artifacts, &nested).status.code(), Some(2));
    assert!(!nested.exists());
}

#[test]
fn oracle_process_error_is_recorded_and_never_reported_as_success() {
    let repo = TestRepo::new("http://127.0.0.1:1/v1", "ORACLE_UNUSED_KEY", &|_| "correct".into());
    fs::write(repo.root.parent().unwrap().join("oracle.py"), "raise RuntimeError('controlled failure')\n").unwrap();
    let out = repo.evidence("error").join("result.json");
    assert_eq!(judge(&repo, "S1", &repo.fixtures.join("S1"), &out).status.code(), Some(2));
    let record: Value = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
    assert_eq!(record["correctness"]["status"], "oracle_error");
    assert_eq!(record["workspace_unchanged"], true);
}
