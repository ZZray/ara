//! Capability_Test_Host tool list (requirements 20.2, 20.3, 20.5).
//!
//! With the P2-e switch, every Execute session of the registered Task_Set
//! offers `ws_edit_file`, and `ws_write_file` tells the model to use it only
//! for new files. A controlled session through the real Harness and Core
//! checks the tool list of every model request and that an edit result
//! names the SHA-256 of the edited file.

mod runner_support;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ara_agent_lite_harness::{start_with_options, TaskKind, TaskSpec};
use lite_eval::{
    common::{sha256_file, Credentials},
    host::{run_id, session_paths, CapabilityHost, Features, HostConfig, PolicyModelHost, RunTemplate, WRITE_NEW_ONLY},
    registry::{load, ProfileMode, TaskEntry},
    workspace::{canonical, prepare},
};
use runner_support::{call, text, turn, Provider, MODEL};
use serde_json::{json, Value};

fn fixture(base: &Path) -> PathBuf {
    let fixture = base.join("fixture");
    std::fs::create_dir_all(fixture.join("src")).unwrap();
    std::fs::write(fixture.join("README.md"), "Fixture.\n").unwrap();
    fixture
}

async fn host(base: &Path, task: &TaskEntry, features: Features, endpoint: &str) -> CapabilityHost {
    let task_dir = base.join("run").join(&task.task_id);
    std::fs::create_dir_all(&task_dir).unwrap();
    let workspace = prepare(&fixture(base), &task_dir).unwrap();
    let (session_path, environment_state) = session_paths(&workspace.private);
    CapabilityHost::build(HostConfig {
        task: task.clone(),
        workspace,
        features,
        template: RunTemplate {
            endpoint: endpoint.into(),
            token: "schema-test-key".into(),
            route: ara_agent_lite_harness::core::model::Route {
                model: MODEL.into(),
                protocol: "openai-completions".into(),
                binding: "test".into(),
                decision: "test".into(),
            },
            max_output: 1024,
            session_path,
            environment_state,
            session_key: format!("lite-eval/test/{}", task.task_id),
            run_max_calls: 10,
            run_max_seconds: 120,
        },
        model_host: Arc::new(PolicyModelHost {
            generation: Default::default(),
            length_recovery: Default::default(),
            output_ceiling: features.output_ceiling,
            context_window_cap: None,
        }),
        credential_env: "LITE_EVAL_SCHEMA_KEY".into(),
        credentials: Credentials::new(["schema-test-key".to_owned()]),
    })
    .await
    .unwrap()
}

/// Names and descriptions of the tools a Run request offers.
fn offered(host: &CapabilityHost) -> Vec<(String, String)> {
    host.request("schema-run-1", "List the tools.")
        .extra_tools
        .iter()
        .map(|item| (item.tool.schema().name.clone(), item.tool.schema().description.clone()))
        .collect()
}

#[tokio::test]
async fn registered_execute_sessions_offer_ws_edit_file_and_restrict_ws_write_file() {
    let set = load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("task-set.json")).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    let mut execute = 0;
    for task in &set.tasks {
        for (label, features) in [("current", Features::CURRENT), ("v1", Features::V1)] {
            let tools =
                offered(&host(&base.join(label).join(&task.task_id), task, features, "http://127.0.0.1:9/v1").await);
            let names: Vec<&str> = tools.iter().map(|(name, _)| name.as_str()).collect();
            let write = tools.iter().find(|(name, _)| name == "ws_write_file").map(|(_, text)| text.as_str());
            if task.profile.mode == ProfileMode::Plan {
                // Plan sessions are read-only: no whole-file write and no edit tool.
                assert!(!names.contains(&"ws_edit_file") && write.is_none(), "{}: {names:?}", task.task_id);
                continue;
            }
            let write = write.unwrap_or_else(|| panic!("{} has no ws_write_file: {names:?}", task.task_id));
            if features.edit_file {
                assert!(names.contains(&"ws_edit_file"), "{}: {names:?}", task.task_id);
                assert!(write.ends_with(&format!(" {WRITE_NEW_ONLY}")), "{}: {write}", task.task_id);
            } else {
                assert!(!names.contains(&"ws_edit_file"), "v1 has no edit tool: {names:?}");
                assert!(!write.contains(WRITE_NEW_ONLY), "{write}");
            }
            // The workspace note replaces the builtin's absolute-path sentence.
            assert!(!write.to_lowercase().contains("absolute paths are allowed"), "{write}");
        }
        execute += usize::from(task.profile.mode == ProfileMode::Execute);
    }
    assert!(execute >= 4, "the registered Task_Set has Execute tasks");
}

#[test]
fn every_model_request_lists_ws_edit_file_and_edit_results_name_the_new_sha() {
    let dir = tempfile::tempdir().unwrap();
    let base = canonical(dir.path()).unwrap();
    let task: TaskEntry = serde_json::from_value(json!({
        "task_id": "S1", "kind": "Coding", "fixture_path": base.join("fixture"),
        "fixture_sha256": "", "oracle": {"type": "unit_test", "command": []},
        "profile": {"mode": "execute",
                    "tools": ["ws_read_file", "ws_list_dir", "ws_search_files", "ws_write_file", "ws_edit_file"]}
    }))
    .unwrap();
    let bodies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let seen = bodies.clone();
    let provider = Provider::new(Arc::new(move |body: &Value| {
        seen.lock().unwrap().push(body.clone());
        match turn(body) {
            0 => call("w1", "ws_write_file", json!({"path": "src/new.txt", "content": "alpha beta\n"})),
            // No versioned option: the wrapper still returns the SHA receipt.
            1 => call("e1", "ws_edit_file", json!({"path": "src/new.txt", "old_text": "beta", "new_text": "gamma"})),
            2 => call("e2", "ws_edit_file", json!({"path": "src/new.txt", "old_text": "missing", "new_text": "x"})),
            _ => text("Done."),
        }
    }));
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let (outcome, host) = runtime.block_on(async {
        let host = Arc::new(host(&base, &task, Features::CURRENT, &provider.endpoint).await);
        let spec = TaskSpec {
            task_id: "S1".into(),
            criteria_revision: "test".into(),
            authorization_revision: "test".into(),
            kind: TaskKind::Coding,
            evidence_dir: base.join("run").join("S1").join("harness"),
            max_runs: 1,
            max_calls: 10,
            deadline_unix_ms: (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as u64)
                + 120_000,
        };
        let request = host.request(&run_id("S1", 1), "Create and edit a file.");
        let handle = start_with_options(spec, request, host.clone(), host.task_options()).unwrap();
        (handle.wait().await, host)
    });
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 4, "{}", outcome.detail);
    for body in &bodies {
        let tools = body["tools"].as_array().expect("tools in every model request");
        let function =
            |name: &str| tools.iter().map(|tool| &tool["function"]).find(|function| function["name"] == name).cloned();
        let edit = function("ws_edit_file").expect("ws_edit_file offered");
        assert!(edit["parameters"]["properties"]["additional_edits"].is_object(), "{edit}");
        let write = function("ws_write_file").expect("ws_write_file offered");
        assert!(write["description"].as_str().unwrap().contains(WRITE_NEW_ONLY), "{write}");
    }
    let file = host.workspace.root.join("src").join("new.txt");
    assert_eq!(std::fs::read(&file).unwrap(), b"alpha gamma\n");
    let sha = sha256_file(&file).unwrap();
    let results: Vec<String> = bodies[3]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["content"].to_string())
        .collect();
    assert_eq!(results.len(), 3, "{results:?}");
    assert!(results[1].contains(&format!("observed_after_sha256\\\":\\\"{sha}")), "{}", results[1]);
    // An inapplicable block is rejected with its block number and the file is unchanged.
    assert!(results[2].contains("block=0"), "{}", results[2]);
    let counts = host.write_counts();
    assert_eq!((counts.write_new, counts.write_existing, counts.edit_ok, counts.edit_rejected), (1, 0, 1, 1));
}
