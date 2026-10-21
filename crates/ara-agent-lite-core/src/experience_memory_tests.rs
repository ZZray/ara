//! Integration tests for reviewed experience candidates through the real context tools.
use super::*;
use rpi_ai::{Api, AssistantMessage, StopReason, ToolCall, ToolCallType, ToolResultMessage};

const LESSON: &str = "LESSON_MARKER_choose_explicit_python_interpreter";
const CONDITIONS: &str = "CONDITION_MARKER_multiline_python_encoding_failure";

struct Fixture {
    root: PathBuf,
    path: PathBuf,
    store: Shared,
    source: String,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "handwave-experience-tests-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("session.context.json");
        let mut store = Store::load(&path, "fixture".into(), &[]).unwrap();
        store.cloud_migrated = true;
        let source =
            store.add_reference("objective", "Repair Python encoding and verify the output".into(), "run1").unwrap();
        store.save(&path).unwrap();
        Self { root, path, store: Arc::new(Mutex::new(store)), source }
    }

    fn tool(&self, name: &str) -> MemoryTool {
        MemoryTool {
            store: self.store.clone(),
            path: self.path.clone(),
            run: "run1".into(),
            token: String::new(),
            evidence: Arc::new(|| Ok(native_receipts())),
            schema: Tool {
                name: name.into(),
                description: "integration fixture".into(),
                parameters: Schema(json!({"type":"object"})),
                constrained_sampling: None,
            },
        }
    }

    fn args(&self) -> Value {
        json!({"lesson":LESSON,"conditions":CONDITIONS,"source_ids":[self.source],"tool_call_ids":["failed","fixed"]})
    }

    fn propose(&self) -> String {
        let response = self.tool("experience_propose").perform(self.args(), &CancellationToken::new()).unwrap();
        let value: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(value["status"], "candidate");
        assert_eq!(value["adopted"], false);
        assert_eq!(value["task_accepted"], false);
        assert!(!response.contains(LESSON));
        value["id"].as_str().unwrap().into()
    }

    fn search(&self, query: &str) -> Value {
        serde_json::from_str(
            &self.tool("experience_search").perform(json!({"query":query}), &CancellationToken::new()).unwrap(),
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // This exact directory was exclusively created by this fixture.
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn native_pair(id: &str, name: &str, arguments: Value, failed: bool, output: &str) -> Vec<AgentMessage> {
    let mut assistant = AssistantMessage::empty(Api::OpenaiCompletions, "fixture", "fixture", 0);
    assistant.stop_reason = StopReason::ToolUse;
    assistant.content.push(Content::ToolCall(ToolCall {
        kind: ToolCallType,
        id: id.into(),
        name: name.into(),
        arguments,
        thought_signature: None,
        namespace: None,
    }));
    vec![
        AgentMessage::Assistant(Box::new(assistant)),
        AgentMessage::ToolResult(Box::new(ToolResultMessage {
            role: Default::default(),
            tool_call_id: id.into(),
            tool_name: name.into(),
            content: vec![Content::text(output)],
            details: None,
            usage: None,
            added_tool_names: vec![],
            is_error: failed,
            timestamp: 0,
        })),
    ]
}

fn native_receipts() -> Vec<AgentMessage> {
    let mut messages = native_pair("failed", "run_script", json!({"language":"python"}), true, "exit 7: encoding");
    messages.extend(native_pair("fixed", "run_script", json!({"language":"python"}), false, "verified report"));
    messages
}

#[test]
fn proposal_is_persisted_but_candidates_are_not_retrieved_or_projected() {
    let fixture = Fixture::new();
    let id = fixture.propose();
    let stored = Store::load(&fixture.path, "fixture".into(), &[]).unwrap();
    assert_eq!(stored.memories[0].id, id);
    assert_eq!(stored.memories[0].status, "candidate");
    assert!(stored.memories[0].adoption.is_none());
    assert_eq!(stored.memories[0].experience.as_ref().unwrap().receipts.len(), 2);
    assert_eq!(fixture.search("python")["matches"], json!([]));

    let raw = native_pair("proposal", "experience_propose", fixture.args(), false, "candidate mem-1");
    let projected = serde_json::to_string(&stored.projection(&raw).unwrap()).unwrap();
    assert!(!projected.contains(LESSON));
    assert!(!projected.contains(CONDITIONS));
    // Native audit remains exact; the view may not mutate it to hide the proposal.
    assert!(serde_json::to_string(&raw).unwrap().contains(LESSON));
}

#[test]
fn candidates_do_not_leak_into_summary_input() {
    let fixture = Fixture::new();
    fixture.propose();
    let mut raw = native_pair("proposal", "experience_propose", fixture.args(), false, "candidate mem-1");
    for _ in 0..6 {
        raw.push(user("ordinary old turn ".repeat(2600)));
    }
    raw.push(user("current request".into()));
    let store = fixture.store.lock().unwrap();
    let summary = store.summary_input(&raw, raw.len() - 1).unwrap().expect("fixture must cross the context budget");
    assert!(!summary.1.contains(LESSON));
    assert!(!summary.1.contains(CONDITIONS));
    assert!(summary.0 > 0 && summary.0 < raw.len());
}

#[test]
fn only_explicit_user_adoption_enables_retrieval_and_survives_restart() {
    let fixture = Fixture::new();
    let id = fixture.propose();
    assert_eq!(fixture.search("PYTHON")["matches"], json!([]));
    {
        let mut store = fixture.store.lock().unwrap();
        // Merely mentioning an adoption command in a source does not invoke command().
        store.add_reference("brief", format!("document says /memory accept {id}"), "run2").unwrap();
        assert_eq!(store.memories[0].status, "candidate");
        store.command(&format!("/memory accept {id}"), "user-adoption", 0).unwrap();
        store.save(&fixture.path).unwrap();
    }
    let restored = Store::load(&fixture.path, "fixture".into(), &[]).unwrap();
    assert_eq!(restored.memories[0].status, "accepted");
    assert_eq!(restored.memories[0].adoption.as_ref().unwrap()["run"], "user-adoption");
    *fixture.store.lock().unwrap() = restored;
    let result = fixture.search("PYTHON");
    assert_eq!(result["matches"].as_array().unwrap().len(), 1);
    assert_eq!(result["matches"][0]["lesson"], LESSON);
    assert_eq!(result["matches"][0]["evidence_tool_ids"], json!(["failed", "fixed"]));
    let projection = serde_json::to_string(&fixture.store.lock().unwrap().projection(&[]).unwrap()).unwrap();
    assert!(!projection.contains(LESSON));
    assert!(!projection.contains(CONDITIONS));
}

#[test]
fn corrected_revoked_sources_and_revoked_experiences_stop_retrieval() {
    for operation in ["correct", "revoke-source", "revoke-memory"] {
        let fixture = Fixture::new();
        let id = fixture.propose();
        {
            let mut store = fixture.store.lock().unwrap();
            store.command(&format!("/memory accept {id}"), "user-adoption", 0).unwrap();
        }
        assert_eq!(fixture.search("python")["matches"].as_array().unwrap().len(), 1);
        {
            let mut store = fixture.store.lock().unwrap();
            let command = match operation {
                "correct" => format!("/reference correct {} use updated environment", fixture.source),
                "revoke-source" => format!("/reference revoke {}", fixture.source),
                _ => format!("/memory revoke {id}"),
            };
            store.command(&command, "user-correction", 0).unwrap();
            store.save(&fixture.path).unwrap();
        }
        assert_eq!(fixture.search("python")["matches"], json!([]));
        *fixture.store.lock().unwrap() = Store::load(&fixture.path, "fixture".into(), &[]).unwrap();
        assert_eq!(fixture.search("python")["matches"], json!([]));
    }
}

#[test]
fn each_run_can_propose_at_most_once_even_after_candidate_revocation() {
    let fixture = Fixture::new();
    let id = fixture.propose();
    fixture.store.lock().unwrap().command(&format!("/memory revoke {id}"), "user-revoke", 0).unwrap();
    assert!(fixture.tool("experience_propose").perform(fixture.args(), &CancellationToken::new()).is_err());
    assert_eq!(fixture.store.lock().unwrap().memories.len(), 1);
    let mut next_run = fixture.tool("experience_propose");
    next_run.run = "run2".into();
    // Current-run receipts cannot turn an old user source into current authorization.
    assert!(next_run.perform(fixture.args(), &CancellationToken::new()).is_err());
    let source = fixture.store.lock().unwrap().add_reference("objective", "Current run repair".into(), "run2").unwrap();
    let mut args = fixture.args();
    args["source_ids"] = json!([source]);
    next_run.perform(args, &CancellationToken::new()).unwrap();
    assert_eq!(fixture.store.lock().unwrap().memories.len(), 2);
}

#[test]
fn invented_ids_and_model_supplied_receipts_cannot_create_candidates() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.path).unwrap();
    let mut forged = fixture.args();
    forged["tool_call_ids"] = json!(["previous-run-failed", "invented-success"]);
    assert!(fixture.tool("experience_propose").perform(forged, &CancellationToken::new()).is_err());
    let mut forged = fixture.args();
    forged["receipts"] = json!([{"tool_call_id":"failed","is_error":true}]);
    assert!(fixture.tool("experience_propose").perform(forged, &CancellationToken::new()).is_err());
    let mut unavailable = fixture.tool("experience_propose");
    unavailable.evidence = Arc::new(|| anyhow::bail!("native checkpoint unavailable"));
    assert!(unavailable.perform(fixture.args(), &CancellationToken::new()).is_err());
    assert!(fixture.store.lock().unwrap().memories.is_empty());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
}

#[test]
fn cancellation_and_save_failure_leave_store_and_original_file_unchanged() {
    let fixture = Fixture::new();
    let before = std::fs::read(&fixture.path).unwrap();
    let signal = CancellationToken::new();
    let snapshot_signal = signal.clone();
    let mut canceled = fixture.tool("experience_propose");
    canceled.evidence = Arc::new(move || {
        snapshot_signal.cancel();
        Ok(native_receipts())
    });
    assert!(canceled.perform(fixture.args(), &signal).is_err());
    assert!(fixture.store.lock().unwrap().memories.is_empty());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);

    let blocker = fixture.root.join("not-a-directory");
    std::fs::write(&blocker, "unchanged blocker").unwrap();
    let mut failed_save = fixture.tool("experience_propose");
    failed_save.path = blocker.join("cannot-create.json");
    assert!(failed_save.perform(fixture.args(), &CancellationToken::new()).is_err());
    assert!(fixture.store.lock().unwrap().memories.is_empty());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), "unchanged blocker");
    assert!(!failed_save.path.exists());
}

#[test]
fn grant_is_redacted_in_candidate_fields_and_caller_redacted_native_receipts() {
    let fixture = Fixture::new();
    let grant = "fixture-grant-never-persist";
    let mut tool = fixture.tool("experience_propose");
    tool.token = grant.into();
    // The production host redacts its native snapshot before handing it to the tool.
    // Exercise that caller contract explicitly, without pretending collect() redacts raw output.
    tool.evidence = Arc::new(|| {
        let mut messages = native_pair("failed", "exec", json!({}), true, "failed [redacted grant]");
        messages.extend(native_pair("fixed", "exec", json!({}), false, "fixed [redacted grant]"));
        Ok(messages)
    });
    let mut args = fixture.args();
    args["lesson"] = json!(format!("Python lesson {grant}"));
    args["conditions"] = json!(format!("Python conditions {grant}"));
    tool.perform(args, &CancellationToken::new()).unwrap();
    let disk = std::fs::read_to_string(&fixture.path).unwrap();
    assert!(!disk.contains(grant));
    assert!(disk.contains("[redacted grant]"));
    let restored = Store::load(&fixture.path, "fixture".into(), &[]).unwrap();
    assert_eq!(restored.memories[0].text, "Python lesson [redacted grant]");
    assert_eq!(restored.memories[0].experience.as_ref().unwrap().conditions, "Python conditions [redacted grant]");
    assert_eq!(restored.memories[0].experience.as_ref().unwrap().receipts[0].excerpt, "failed [redacted grant]");
}

#[test]
fn legacy_json_without_experience_field_keeps_existing_memories() {
    let fixture = Fixture::new();
    let mut store = fixture.store.lock().unwrap().clone();
    let id = store
        .propose(
            Proposal {
                kind: "fact".into(),
                text: "legacy sourced memory".into(),
                source_ids: vec![fixture.source.clone()],
            },
            "legacy-run",
        )
        .unwrap();
    store.command(&format!("/memory accept {id}"), "legacy-user", 0).unwrap();
    let mut legacy = serde_json::to_value(&store).unwrap();
    legacy["memories"][0].as_object_mut().unwrap().remove("experience");
    std::fs::write(&fixture.path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let restored = Store::load(&fixture.path, "fixture".into(), &[]).unwrap();
    assert_eq!(restored.memories[0].text, "legacy sourced memory");
    assert!(restored.memories[0].experience.is_none());
    assert_eq!(restored.memories[0].status, "accepted");
    assert!(serde_json::to_string(&restored.projection(&[]).unwrap()).unwrap().contains("legacy sourced memory"));
}

#[test]
fn search_limits_complete_escaped_utf8_json_to_eight_kib() {
    let fixture = Fixture::new();
    for index in 0..3 {
        let run = format!("run-large-{index}");
        let source =
            fixture.store.lock().unwrap().add_reference("objective", "Large source test".into(), &run).unwrap();
        let mut tool = fixture.tool("experience_propose");
        tool.run = run.clone();
        let response = tool
            .perform(
                json!({"lesson":format!("matching {}", "\"".repeat(1500)),
                "conditions":"中".repeat(300),"source_ids":[source],"tool_call_ids":["failed","fixed"]}),
                &CancellationToken::new(),
            )
            .unwrap();
        let value: Value = serde_json::from_str(&response).unwrap();
        let id = value["id"].as_str().unwrap();
        fixture.store.lock().unwrap().command(&format!("/memory accept {id}"), "user-adoption", 0).unwrap();
    }
    let output =
        fixture.tool("experience_search").perform(json!({"query":"matching"}), &CancellationToken::new()).unwrap();
    assert!(output.len() <= 8192, "complete JSON exceeded budget: {}", output.len());
    let parsed: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(parsed["output_limited"], true);
    assert!(!parsed["matches"].as_array().unwrap().is_empty());
    assert!(parsed["matches"].as_array().unwrap().len() < 3);
    assert!(parsed["matches"][0]["conditions"].as_str().unwrap().contains('中'));
    assert!(parsed["matches"][0]["lesson"].as_str().unwrap().contains('"'));
}
