//! Sealed isolation (requirements 1.1, 1.5, 2.1, 2.3, 2.5, 2.8).
//!
//! A fake upstream drives the real Capability_Test_Host through the real
//! Harness and Core: absolute, `..` and junction paths to read, write and
//! edit the Oracle, and searches for its feature strings. Every escaping
//! call must get `path_outside_workspace`, every search zero matches, no
//! tool result may carry Oracle content, and the Oracle bytes stay the same.
//! Example checks cover the registered Task_Set, the SH1 structure and the
//! Run_Manifest `sealed_holdout` section of a controlled run.

mod runner_support;
mod support;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use ara_agent_lite_harness::{start_with_options, TaskKind, TaskOptions, TaskSpec};
use lite_eval::{
    common::{sha256_file, Credentials},
    host::{run_id, session_paths, CapabilityHost, Features, HostConfig, PolicyModelHost, RunTemplate},
    registry::{
        check_holdout_log, load, load_sealed, validate, HoldoutLog, HoldoutStatus, TaskEntry, HOLDOUT_LOG_PATH,
        TASK_SET_PATH,
    },
    runner::{run, HarnessExecutor, RunInputs, Selection},
    sealed::{event_records, journal_records, scan},
    workspace::{canonical, prepare},
};
use runner_support::{call, git, text, turn, Provider, MODEL};
use serde_json::{json, Value};

const FEATURES: [&str; 3] = ["SEALED-FEATURE-91a7", "SEALED-FEATURE-b3c2", "SEALED-FEATURE-5e0d"];
const ORACLE_LINE: &str = "ORACLE-BODY-LINE-7f31";

/// `<base>/sealed/oracle/oracle.py` holding the feature strings, outside
/// every workspace; returns its path.
fn oracle(base: &Path) -> PathBuf {
    let dir = base.join("sealed").join("oracle");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("oracle.py");
    std::fs::write(
        &path,
        format!(
            "# {ORACLE_LINE}\n# cases: {}\nimport argparse, json\np = argparse.ArgumentParser()\n\
             p.add_argument('--artifacts'); p.add_argument('--out')\na = p.parse_args()\n\
             open(a.out, 'w').write(json.dumps({{'passed': 40, 'total': 40}}))\n",
            FEATURES.join(" ")
        ),
    )
    .unwrap();
    path
}

fn fixture(base: &Path) -> PathBuf {
    let fixture = base.join("fixture");
    std::fs::create_dir_all(fixture.join("src")).unwrap();
    std::fs::write(fixture.join("README.md"), "Implement the ledger.\n").unwrap();
    std::fs::write(fixture.join("src/main.cpp"), "int main() { return 0; }\n").unwrap();
    fixture
}

fn entry(fixture: &Path) -> TaskEntry {
    serde_json::from_value(json!({
        "task_id": "SH1", "kind": "Coding", "fixture_path": fixture,
        "fixture_sha256": lite_eval::digest::tree_digest(fixture).unwrap().sha256,
        "oracle": {"type": "sealed_test", "holdout_cases": 40, "command": []},
        "holdout": {"status": "new holdout", "registered": "2026-10-29"},
        "profile": {"mode": "execute",
                    "tools": ["ws_read_file", "ws_list_dir", "ws_search_files", "ws_write_file", "ws_edit_file", "run_public_checks"],
                    "public_check": ["python", "-c", "print('public ok')"]}
    }))
    .unwrap()
}

fn relative(from: &Path, to: &Path) -> String {
    let common = from.components().zip(to.components()).take_while(|(a, b)| a == b).count();
    let up = from.components().count() - common;
    let rest: PathBuf = to.components().skip(common).collect();
    format!("{}{}", "../".repeat(up), rest.to_string_lossy().replace('\\', "/"))
}

#[test]
fn escaping_tool_calls_are_rejected_and_reveal_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    let oracle_path = oracle(&base);
    let oracle_dir = oracle_path.parent().unwrap().to_owned();
    let oracle_sha = sha256_file(&oracle_path).unwrap();
    let task_dir = base.join("run").join("SH1");
    std::fs::create_dir_all(&task_dir).unwrap();
    let workspace = prepare(&fixture(&base), &task_dir).unwrap();
    // Pre-built link inside the workspace to the Oracle directory (2.3).
    support::dir_link(&workspace.root.join("link"), &oracle_dir).unwrap();
    let absolute = oracle_path.to_string_lossy().into_owned();
    let dotdot = relative(&workspace.root, &oracle_path);
    assert!(dotdot.starts_with("../"), "{dotdot}");
    let calls = vec![
        ("ws_read_file", json!({"path": absolute})),
        ("ws_read_file", json!({"path": dotdot})),
        ("ws_write_file", json!({"path": dotdot, "content": "overwritten"})),
        ("ws_edit_file", json!({"path": absolute, "old_text": ORACLE_LINE, "new_text": "edited"})),
        ("ws_read_file", json!({"path": "link/oracle.py"})),
        ("ws_write_file", json!({"path": "link/planted.py", "content": "x"})),
        ("ws_edit_file", json!({"path": "link/oracle.py", "old_text": ORACLE_LINE, "new_text": "edited"})),
        ("ws_list_dir", json!({"path": "link"})),
        ("ws_list_dir", json!({"path": oracle_dir.to_string_lossy()})),
        ("ws_search_files", json!({"query": FEATURES[0], "path": oracle_dir.to_string_lossy()})),
        ("ws_search_files", json!({"query": FEATURES[1]})),
        ("ws_search_files", json!({"query": FEATURES[2], "path": "link"})),
    ];
    let escaping = calls.len() - 1; // all but the plain workspace search
    let script_calls = calls.clone();
    let provider = Provider::new(Arc::new(move |body: &Value| match script_calls.get(turn(body)) {
        Some((name, args)) => call(&format!("c{}", turn(body)), name, args.clone()),
        None => text("Finished probing the workspace."),
    }));
    let credentials = Credentials::new(["sealed-test-key".to_owned()]);
    let (session_path, environment_state) = session_paths(&workspace.private);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let outcome = runtime.block_on(async {
        let host = Arc::new(
            CapabilityHost::build(HostConfig {
                task: entry(&base.join("fixture")),
                workspace: workspace.clone(),
                // The edit wrapper is a P2-e switch; this controlled test turns it on.
                features: Features { edit_file: true, ..Features::V1 },
                template: RunTemplate {
                    endpoint: provider.endpoint.clone(),
                    token: "sealed-test-key".into(),
                    route: ara_agent_lite_harness::core::model::Route {
                        model: MODEL.into(),
                        protocol: "openai-completions".into(),
                        binding: "test".into(),
                        decision: "test".into(),
                    },
                    max_output: 1024,
                    session_path: session_path.clone(),
                    environment_state,
                    session_key: "lite-eval/test/SH1".into(),
                    run_max_calls: 20,
                    run_max_seconds: 120,
                },
                model_host: Arc::new(PolicyModelHost {
                    generation: Default::default(),
                    length_recovery: Default::default(),
                    output_ceiling: None,
                }),
                credential_env: "LITE_EVAL_SEALED_KEY".into(),
                credentials,
            })
            .await
            .unwrap(),
        );
        let registered: Vec<String> = host.tools.iter().map(|(tool, _)| tool.schema().name.clone()).collect();
        assert!(
            registered.contains(&"ws_edit_file".to_owned()) && registered.contains(&"run_public_checks".to_owned())
        );
        assert_eq!(host.unchecked_tools, ["run_public_checks"]);
        let spec = TaskSpec {
            task_id: "SH1".into(),
            criteria_revision: "test".into(),
            authorization_revision: "test".into(),
            kind: TaskKind::Coding,
            evidence_dir: task_dir.join("harness"),
            max_runs: 1,
            max_calls: 20,
            deadline_unix_ms: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64)
                + 120_000,
        };
        let request = host.request(&run_id("SH1", 1), "Probe the workspace.");
        let handle = start_with_options(spec, request, host.clone(), TaskOptions::default()).unwrap();
        handle.wait().await
    });
    assert_eq!(
        provider.model_requests.load(std::sync::atomic::Ordering::SeqCst),
        calls.len() + 1,
        "{}",
        outcome.detail
    );

    let events = task_dir.join("harness").join("run-1-events.jsonl");
    let outputs: Vec<Value> = std::fs::read_to_string(&events)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap()["event"].clone())
        .filter(|event| event["type"] == "tool_end")
        .collect();
    assert_eq!(outputs.len(), calls.len());
    let rejected = outputs.iter().filter(|event| event.to_string().contains("path_outside_workspace")).count();
    assert_eq!(rejected, escaping, "{outputs:#?}");
    let plain = &outputs[10];
    let plain = plain["output"].as_str().map(str::to_owned).unwrap_or_else(|| plain.to_string());
    assert!(plain.contains("\"matches\":[]"), "workspace search must find nothing: {plain}");
    let mut records = journal_records(&session_path, "session").unwrap();
    records.extend(event_records(&events).unwrap());
    let features: Vec<String> = FEATURES.iter().map(|f| f.to_string()).collect();
    assert!(scan(&features, &records).is_empty(), "feature strings reached a tool result");
    for record in &records {
        let text = String::from_utf8_lossy(&record.bytes);
        assert!(!text.contains(ORACLE_LINE), "Oracle content in {:?}", record.location);
    }
    assert_eq!(sha256_file(&oracle_path).unwrap(), oracle_sha);
    assert!(!oracle_dir.join("planted.py").exists());
    assert_eq!(std::fs::read_dir(&oracle_dir).unwrap().count(), 1);
}

/// A file symbolic link to the Oracle (2.3). Creating it needs a Windows
/// privilege; without it this test fails instead of passing silently.
#[test]
fn a_file_symlink_to_the_oracle_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    let oracle_path = oracle(&base);
    let task_dir = base.join("run");
    std::fs::create_dir_all(&task_dir).unwrap();
    let workspace = prepare(&fixture(&base), &task_dir).unwrap();
    let link = workspace.root.join("oracle-link.py");
    if let Err(error) = support::file_symlink(&link, &oracle_path) {
        panic!("file symlink privilege unavailable: {error}");
    }
    for intent in [
        lite_eval::host::tools::Intent::ReadExisting,
        lite_eval::host::tools::Intent::WriteMaybeNew,
        lite_eval::host::tools::Intent::EditExisting,
    ] {
        let rejection = lite_eval::host::tools::confine(&workspace.root, "oracle-link.py", intent).unwrap_err();
        assert_eq!(rejection.code(), "path_outside_workspace");
    }
}

#[test]
fn the_registered_task_set_has_six_valid_tasks_and_a_well_formed_sh1() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let set = load(&repo.join(TASK_SET_PATH)).unwrap();
    let log = HoldoutLog::load(&repo.join(HOLDOUT_LOG_PATH)).unwrap();
    assert_eq!(set.tasks.len(), 6);
    assert_eq!(validate(&set), []);
    assert_eq!(check_holdout_log(&set, log.events()), []);
    let ids: Vec<&str> = set.tasks.iter().map(|entry| entry.task_id.as_str()).collect();
    assert_eq!(ids, ["S1", "S2", "S3", "R1", "H2", "SH1"]);
    let sh1 = set.tasks.iter().find(|entry| entry.task_id == "SH1").unwrap();
    assert!(matches!(sh1.holdout, Some(HoldoutStatus::New { .. })));
    assert!(sh1.oracle.holdout_cases.unwrap() >= 10);
    // 2.1: at least 3 C++ sources or headers in the fixture.
    let cpp = runner_support::files(&sh1.fixture_path)
        .into_iter()
        .filter(|path| matches!(path.extension().and_then(|e| e.to_str()), Some("cpp" | "hpp" | "h" | "cc")))
        .count();
    assert!(cpp >= 3, "{cpp} C++ files");
    let sealed = load_sealed(set.sealed.as_ref().unwrap()).unwrap();
    assert_eq!(sealed.task_id, "SH1");
    assert_eq!(sha256_file(&sealed.oracle_path).unwrap(), sealed.oracle_sha256);
    assert!(sealed.features.len() >= 3);
    assert!(!sealed.oracle_path.starts_with(&sh1.fixture_path));
}

/// A throw-away repository whose Task_Set registers a sealed SH1.
fn sealed_repo(endpoint: &str, base: &Path) -> (PathBuf, PathBuf) {
    let root = base.join("repo");
    std::fs::create_dir_all(root.join("tools/lite-eval")).unwrap();
    let oracle_path = oracle(base);
    let sealed = json!({"task_id": "SH1", "oracle_path": oracle_path, "oracle_sha256": sha256_file(&oracle_path).unwrap(),
                        "features": FEATURES, "registered": "2026-10-29"});
    let sealed_path = base.join("sealed").join("task-set.sealed.json");
    std::fs::write(&sealed_path, serde_json::to_vec_pretty(&sealed).unwrap()).unwrap();
    let fixtures = base.join("fixtures");
    let mut tasks = Vec::new();
    let mut log = String::new();
    for (index, id) in runner_support::IDS.iter().enumerate() {
        let fixture = fixtures.join(id);
        std::fs::create_dir_all(&fixture).unwrap();
        std::fs::write(fixture.join("README.md"), "Write the word correct into answer.txt.\n").unwrap();
        let mut task = serde_json::to_value(entry(&fixture)).unwrap();
        task["task_id"] = json!(id);
        let cases = match *id {
            "S3" => json!({"type": "t", "public_cases": 10, "hidden_cases": 48, "command": ["python", "-c", "0"]}),
            "H2" => json!({"type": "t", "hidden_cases": 128, "command": ["python", "-c", "0"]}),
            "SH1" => json!({"type": "sealed", "holdout_cases": 40, "command": []}),
            _ => json!({"type": "t", "hidden_cases": 1, "command": ["python", "-c", "0"]}),
        };
        task["oracle"] = cases;
        let (status, line) = if *id == "SH1" {
            (json!({"status": "new holdout", "registered": "2026-10-29"}), "\"event\":\"registered_new\"".to_owned())
        } else {
            (
                json!({"status": "not new holdout", "first_adaptive_use": "2026-10-23", "changes": ["t"]}),
                "\"event\":\"marked_not_new\",\"date\":\"2026-10-23\",\"changes\":[\"t\"]".to_owned(),
            )
        };
        task["holdout"] = status;
        let line = if *id == "SH1" { format!("{line},\"date\":\"2026-10-29\"") } else { line };
        log.push_str(&format!("{{\"seq\":{},\"task_id\":\"{id}\",{line}}}\n", index + 1));
        tasks.push(task);
    }
    let set = json!({"version": 1, "digest_rule": "lite-eval-tree-v1",
                     "sealed": {"path": sealed_path, "sha256": sha256_file(&sealed_path).unwrap()}, "tasks": tasks});
    std::fs::write(root.join(TASK_SET_PATH), serde_json::to_vec_pretty(&set).unwrap()).unwrap();
    std::fs::write(root.join(HOLDOUT_LOG_PATH), log).unwrap();
    let policy = runner_support::policy(endpoint, "LITE_EVAL_SEALED_RUN_KEY");
    std::fs::write(root.join("tools/lite-eval/run-policy.json"), serde_json::to_vec_pretty(&policy).unwrap()).unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "fixture"]);
    (root, oracle_path)
}

#[test]
fn a_controlled_sealed_run_records_isolation_and_keeps_the_holdout_status() {
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    let provider = Provider::standard();
    let (root, oracle_path) = sealed_repo(&provider.endpoint, &base);
    let before = std::fs::read(root.join(TASK_SET_PATH)).unwrap();
    let env = |name: &str| (name == "LITE_EVAL_SEALED_RUN_KEY").then(|| "sealed-run-key".to_owned());
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let execute = |name: &str| {
        let evidence = base.join(name);
        std::fs::create_dir_all(&evidence).unwrap();
        runtime
            .block_on(run(RunInputs {
                repo_root: &root,
                evidence_root: &evidence,
                selection: &Selection::Ids(vec!["SH1".into()]),
                executor: &HarnessExecutor,
                env: &env,
                probe_only: false,
                baseline: false,
            }))
            .unwrap()
    };
    let report = execute("clean");
    let section = &report.manifest.sealed_holdout;
    assert_eq!(section["isolation"], "oracle_outside_workspace_with_host_path_checks_no_os_sandbox");
    assert_eq!(section["unchecked_tools"], json!(["run_public_checks"]));
    assert_eq!(section["compromised"], false);
    assert_eq!(section["correctness"], json!({"status": "judged", "passed": 40, "total": 40}));
    assert_eq!(section["workspace_unchanged"], true);
    assert!(section["copied"]["answer.txt"].is_string(), "{section:#}");
    assert_eq!(report.exit, 0, "{:#?}", report.manifest.selected);

    // A changed Oracle compromises the result, but a fake-upstream run
    // never changes the Holdout_Status (2.5).
    std::fs::OpenOptions::new()
        .append(true)
        .open(&oracle_path)
        .map(|mut file| {
            use std::io::Write;
            file.write_all(b"# changed\n").unwrap();
        })
        .unwrap();
    let report = execute("tampered");
    let section = &report.manifest.sealed_holdout;
    assert_eq!(section["compromised"], true);
    assert_eq!(section["correctness"]["status"], "unknown");
    assert_eq!(section["holdout_status_changed"], Value::Null);
    assert_eq!(report.exit, 1);
    assert_eq!(std::fs::read(root.join(TASK_SET_PATH)).unwrap(), before);
}
