//! Native user Bash receipts, fixed OMP 596f2da messages/session-manager.

use ara_ai::{
    AssistantBlock, AssistantMessage, JsonObject, Message, StopReason, ToolCall, UserBlock, UserContent, UserMessage,
};
use ara_session::{
    BashExecutionMessage, CompactedContextItem, CompactionSourceError, Recovery, SessionJournal, UNKNOWN_EFFECT_TEXT,
    bash_output_meta_from_summary,
};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

fn bash() -> BashExecutionMessage {
    BashExecutionMessage {
        command: "printf 'native proof'".into(),
        output: "native proof".into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        meta: None,
        timestamp: 42,
        exclude_from_context: None,
    }
}

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::text("known answer"));
    Message::Assistant(message)
}

fn pending_tool_call() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "pending-bash".into(),
        name: "bash".into(),
        arguments: JsonObject::new(),
        thought_signature: None,
    }));
    message.stop_reason = StopReason::ToolUse;
    message.timestamp = 41;
    Message::Assistant(message)
}

fn recovery_journal_paths(directory: &Path, message: &BashExecutionMessage) -> [PathBuf; 2] {
    let mut journal = SessionJournal::create(directory, directory).unwrap();
    journal.append_message(&pending_tool_call()).unwrap();
    journal.append_bash_execution(message).unwrap();
    journal.materialize().unwrap();
    let created = journal.path().to_path_buf();
    let imported = directory.join("imported.jsonl");
    let header = json!({"type":"session","version":3,"id":"recovery-fixture","cwd":directory,"timestamp":"t"});
    let assistant = json!({"type":"message","id":"assistant-call","parentId":null,
        "timestamp":"2026-09-30T00:00:00.000Z","message":pending_tool_call()});
    let native = json!({"type":"message","id":"native-bash","parentId":"assistant-call",
        "timestamp":"2026-09-30T00:00:01.000Z","message":message.event_message()});
    fs::write(&imported, format!("{header}\n{assistant}\n{native}\n")).unwrap();
    [created, imported]
}

fn model_text(message: &Message) -> String {
    let Message::User(message) = message else { panic!("user projection") };
    message.content.plain_text()
}

#[test]
fn native_event_and_fixed_model_text_keep_exit_cancellation_timestamp_and_metadata_distinct() {
    let mut message = bash();
    assert_eq!(
        message.event_message(),
        json!({"role":"bashExecution","command":"printf 'native proof'",
        "output":"native proof","exitCode":0,"cancelled":false,"truncated":false,"timestamp":42})
    );
    let model = message.model_message().unwrap();
    assert_eq!(model_text(&model), "Ran `printf 'native proof'`\n```\nnative proof\n```");
    let Message::User(model) = model else { unreachable!() };
    assert_eq!(model.timestamp, 42);
    assert!(matches!(model.content, UserContent::Blocks(_)));
    message.output.clear();
    message.exit_code = Some(7);
    assert_eq!(
        model_text(&message.model_message().unwrap()),
        "Ran `printf 'native proof'`\n(no output)\n\nCommand exited with code 7"
    );
    message.cancelled = true;
    assert_eq!(
        model_text(&message.model_message().unwrap()),
        "Ran `printf 'native proof'`\n(no output)\n\n(command cancelled)"
    );
    message.exclude_from_context = Some(false);
    assert_eq!(message.event_message()["excludeFromContext"], false);
    assert!(message.model_message().is_some());
    message.exclude_from_context = Some(true);
    assert!(message.model_message().is_none());
}

#[test]
fn output_meta_summary_produces_fixed_tail_middle_and_column_notices_without_tools_dependency() {
    let mut message = bash();
    message.truncated = true;
    message.meta = bash_output_meta_from_summary(&json!({"truncated":true,"totalLines":6,"totalBytes":30,
        "outputLines":3,"outputBytes":15,"artifactId":"receipt-1"}));
    assert_eq!(message.meta.as_ref().unwrap()["truncation"]["shownRange"], json!({"start":4,"end":6}));
    assert!(
        model_text(&message.model_message().unwrap())
            .ends_with("\n\n[Showing lines 4-6 of 6 (15B limit). Read artifact://receipt-1 for full output]")
    );
    message.meta = bash_output_meta_from_summary(&json!({"truncated":true,"totalLines":2000,"totalBytes":4096,
        "outputLines":5,"outputBytes":1024,"elidedBytes":3072,"elidedLines":1996}));
    assert_eq!(message.meta.as_ref().unwrap()["truncation"]["headRange"], json!({"start":1,"end":2}));
    assert!(
        model_text(&message.model_message().unwrap())
            .ends_with("\n\n[Showing lines 1-2 and 1999-2000 of 2000; 1,996 middle lines (3.0KB) elided]")
    );
    message.meta = bash_output_meta_from_summary(&json!({"truncated":false,"columnMax":120,"columnTruncatedLines":1}));
    assert!(model_text(&message.model_message().unwrap()).ends_with("\n\n[Some lines truncated to 120 chars]"));
    assert!(bash_output_meta_from_summary(&json!({"truncated":false})).is_none());
}

#[test]
fn receipts_materialize_reopen_and_fork_without_replacing_raw_role_or_excluded_identity() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let visible = bash();
    let visible_id = journal.append_bash_execution(&visible).unwrap();
    assert!(!journal.is_on_disk(), "existing lazy materialization is owned by Host");
    journal.materialize().unwrap();
    let mut excluded = bash();
    excluded.command = "private native command".into();
    excluded.exclude_from_context = Some(true);
    let excluded_id = journal.append_bash_execution(&excluded).unwrap();
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries()[0].bash_execution(), Some(visible.clone()));
    assert_eq!(reopened.entries()[1].bash_execution(), Some(excluded.clone()));
    assert_eq!(reopened.entries()[0].raw["message"]["role"], "bashExecution");
    assert_eq!(reopened.undecodable_messages(), 0);
    assert_eq!(reopened.build_context(), vec![visible.model_message().unwrap()]);
    assert_eq!(reopened.model_context(), reopened.build_context());
    assert_eq!(
        reopened
            .compaction_source_snapshot()
            .unwrap()
            .messages
            .iter()
            .map(|message| message.entry_id.as_str())
            .collect::<Vec<_>>(),
        vec![visible_id.as_str()]
    );
    let fork = reopened.fork_at(Some(&excluded_id), Some(&directory.path().join("fork"))).unwrap();
    assert_eq!(fork.entries()[0].id, visible_id);
    assert_eq!(fork.entries()[1].id, excluded_id);
    assert_eq!(fork.entries()[1].bash_execution(), Some(excluded));
    assert_eq!(fork.model_context(), reopened.model_context());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(SessionJournal::open(fork.path()).unwrap().entries(), fork.entries());
}

#[test]
fn append_to_owner_branch_keeps_active_leaf_and_missing_parent_never_changes_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let owner = journal.append_message(&user("original owner")).unwrap();
    let active = journal.append_message(&assistant()).unwrap();
    let recorded = journal.append_bash_execution_to_branch(&bash(), Some(&owner)).unwrap();
    assert_eq!(journal.leaf_id(), Some(active.as_str()));
    assert_eq!(journal.entries().last().unwrap().parent_id.as_deref(), Some(owner.as_str()));
    assert_eq!(journal.build_context().len(), 2, "non-active receipt never enters active context");
    let before = fs::read(journal.path()).unwrap();
    let entries = journal.entries().to_vec();
    assert!(
        journal
            .append_bash_execution_to_branch(&bash(), Some("absent-id"))
            .unwrap_err()
            .to_string()
            .contains("Entry absent-id not found")
    );
    assert_eq!(journal.entries(), entries);
    assert_eq!(journal.leaf_id(), Some(active.as_str()));
    assert_eq!(fs::read(journal.path()).unwrap(), before);
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.entries().last().unwrap().id, recorded);
    assert!(reopened.entries().last().unwrap().bash_execution().is_some());
    let fork = journal.fork_at(Some(&recorded), None).unwrap();
    assert_eq!(fork.build_context().len(), 2, "owner prompt then owner Bash receipt");
    assert_eq!(fork.entries()[1].id, recorded);
    let root = journal.append_bash_execution_to_branch(&bash(), None).unwrap();
    assert_eq!(journal.entries().last().unwrap().id, root);
    assert_eq!(journal.entries().last().unwrap().parent_id, None);
    assert_eq!(journal.leaf_id(), Some(active.as_str()));
}

#[test]
fn failed_owner_append_rolls_back_receipt_id_entries_and_active_leaf_then_rewrite_succeeds() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let owner = journal.append_message(&user("owner")).unwrap();
    let active = journal.append_message(&assistant()).unwrap();
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let entries = journal.entries().to_vec();
    // A directory at the owned temporary journal path makes append fail on
    // Windows and Unix without touching permissions or any live installation.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(journal.append_bash_execution_to_branch(&bash(), Some(&owner)).is_err());
    assert_eq!(journal.entries(), entries);
    assert_eq!(journal.leaf_id(), Some(active.as_str()));
    fs::remove_dir(&path).unwrap();
    fs::write(&path, &bytes).unwrap();
    journal.append_bash_execution_to_branch(&bash(), Some(&owner)).unwrap();
    assert_eq!(journal.leaf_id(), Some(active.as_str()));
    assert_eq!(SessionJournal::open(&path).unwrap().entries().len(), entries.len() + 1);
}

#[test]
fn compaction_projects_visible_bash_sources_skips_excluded_and_retains_all_native_entries() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let q1 = journal.append_message(&user("first prompt")).unwrap();
    let b1 = journal.append_bash_execution(&bash()).unwrap();
    let a1 = journal.append_message(&assistant()).unwrap();
    let mut excluded = bash();
    excluded.exclude_from_context = Some(true);
    let hidden1 = journal.append_bash_execution(&excluded).unwrap();
    let q2 = journal.append_message(&user("kept prompt")).unwrap();
    journal.append_message(&assistant()).unwrap();
    let later = journal.append_bash_execution(&bash()).unwrap();
    let hidden2 = journal.append_bash_execution(&excluded).unwrap();
    let sources = journal.compaction_source_snapshot().unwrap();
    assert_eq!(sources.messages.len(), 6);
    assert!(!sources.messages.iter().any(|source| source.entry_id == hidden1 || source.entry_id == hidden2));
    let compaction = journal.append_compaction("Completed first exchange", &q2, &[q1, b1, a1], 20).unwrap();
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let reopened = SessionJournal::open(&path).unwrap();
    let projection = reopened.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 4);
    assert!(matches!(&projection.items[0], CompactedContextItem::Summary(_)));
    assert!(matches!(&projection.items[3], CompactedContextItem::Message(source) if source.entry_id == later));
    assert_eq!(reopened.model_context().len(), 4);
    assert_eq!(reopened.undecodable_messages(), 0);
    assert_eq!(reopened.entries().len(), 9);
    assert!(reopened.entries().iter().any(|entry| entry.id == hidden1 && entry.bash_execution().is_some()));
    let fork = reopened.fork_at(Some(&compaction), None).unwrap();
    assert_eq!(fork.entries(), reopened.entries());
    assert_eq!(fork.compacted_context_projection().unwrap().items, projection.items);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn malformed_excluded_native_bash_is_counted_and_compaction_refuses_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.jsonl");
    let header = json!({"type":"session","version":3,"id":"native-fixture","cwd":directory.path(),"timestamp":"t"});
    let malformed = json!({"type":"message","id":"bad-bash","parentId":null,"timestamp":"2026-09-30T00:00:00.000Z",
        "message":{"role":"bashExecution","command":"x","cancelled":false,"truncated":false,"timestamp":42,"excludeFromContext":true}});
    fs::write(&path, format!("{header}\n{malformed}\n")).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.undecodable_messages(), 1);
    assert!(journal.build_context().is_empty());
    assert_eq!(
        journal.compaction_source_snapshot(),
        Err(CompactionSourceError::UndecodableMessage { id: "bad-bash".into() })
    );
    assert_eq!(journal.entries()[0].raw, malformed);
}

#[test]
fn excluded_bash_is_transparent_to_interrupted_recovery_after_create_import_and_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let mut excluded = bash();
    excluded.exclude_from_context = Some(true);
    for path in recovery_journal_paths(directory.path(), &excluded) {
        let mut journal = SessionJournal::open(&path).unwrap();
        let native = journal.entries()[1].clone();
        assert_eq!(journal.undecodable_messages(), 0);
        assert_eq!(journal.model_context(), vec![pending_tool_call()]);
        assert_eq!(
            journal.recover_interrupted_tool_calls().unwrap(),
            Recovery { paired: vec!["pending-bash".into()], unpaired_earlier: vec![] }
        );
        assert_eq!(journal.entries().len(), 3);
        assert_eq!(journal.entries()[1], native, "native receipt and identity remain unchanged");
        assert_eq!(journal.entries()[1].bash_execution(), Some(excluded.clone()));
        assert_eq!(journal.undecodable_messages(), 0);
        let context = journal.model_context();
        assert_eq!(context.len(), 2, "excluded Bash does not split the assistant/result pair");
        assert_eq!(context[0], pending_tool_call());
        let Message::ToolResult(result) = &context[1] else { panic!("unknown effect result") };
        assert_eq!(result.tool_call_id, "pending-bash");
        assert_eq!(result.tool_name, "bash");
        assert!(result.is_error);
        let details = result.details.as_ref().unwrap();
        assert_eq!(details["source"], "interrupted_unknown_effect");
        assert_eq!(details["executed"], "unknown");
        assert!(matches!(&result.content[0], UserBlock::Text(text) if text.text == UNKNOWN_EFFECT_TEXT));
        let entries = journal.entries().to_vec();
        let recovered_bytes = fs::read(&path).unwrap();
        let mut reopened = SessionJournal::open(&path).unwrap();
        assert_eq!(reopened.entries(), entries);
        assert_eq!(reopened.model_context(), context);
        assert_eq!(reopened.undecodable_messages(), 0);
        assert_eq!(reopened.recover_interrupted_tool_calls().unwrap(), Recovery::default(), "idempotent");
        assert_eq!(fs::read(&path).unwrap(), recovered_bytes);
    }
}

#[test]
fn included_bash_remains_a_user_barrier_to_interrupted_recovery_after_import_and_reopen() {
    for exclude_from_context in [None, Some(false)] {
        let directory = tempfile::tempdir().unwrap();
        let mut included = bash();
        included.exclude_from_context = exclude_from_context;
        for path in recovery_journal_paths(directory.path(), &included) {
            let mut journal = SessionJournal::open(&path).unwrap();
            let entries = journal.entries().to_vec();
            let bytes = fs::read(&path).unwrap();
            assert_eq!(journal.undecodable_messages(), 0);
            assert_eq!(journal.model_context(), vec![pending_tool_call(), included.model_message().unwrap()]);
            assert_eq!(
                journal.recover_interrupted_tool_calls().unwrap(),
                Recovery { paired: vec![], unpaired_earlier: vec!["pending-bash".into()] }
            );
            assert_eq!(journal.entries(), entries);
            assert_eq!(fs::read(&path).unwrap(), bytes, "no result appended across a User barrier");
            assert_eq!(SessionJournal::open(&path).unwrap().model_context(), journal.model_context());
        }
    }
}

#[test]
fn malformed_excluded_bash_does_not_allow_interrupted_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let mut excluded = bash();
    excluded.exclude_from_context = Some(true);
    let [_, path] = recovery_journal_paths(directory.path(), &excluded);
    let mut lines: Vec<serde_json::Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    lines[2]["message"].as_object_mut().unwrap().remove("output");
    fs::write(&path, lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let bytes = fs::read(&path).unwrap();
    let mut journal = SessionJournal::open(&path).unwrap();
    let entries = journal.entries().to_vec();
    assert_eq!(journal.undecodable_messages(), 1);
    assert_eq!(
        journal.recover_interrupted_tool_calls().unwrap(),
        Recovery { paired: vec![], unpaired_earlier: vec!["pending-bash".into()] }
    );
    assert_eq!(journal.entries(), entries);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.undecodable_messages(), 1);
    assert_eq!(reopened.entries(), entries);
}
