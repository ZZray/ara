use ara_ai::{
    AssistantBlock, AssistantMessage, ImageContent, JsonObject, Message, StopReason, ToolCall, ToolResultMessage,
    UserBlock, UserContent, UserMessage,
};
use ara_session::{
    BashExecutionMessage, CompactedContextItem, CompactionCommitError, CompactionProjectionError,
    CompactionSourceError, ProjectedCompactionSnapshot, SessionJournal,
};
use serde_json::{Value, json};
use std::fs;

fn user(text: &str) -> Message {
    let mut message = UserMessage::text(text);
    message.timestamp = 1;
    Message::User(message)
}

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::text(text));
    message.timestamp = 2;
    Message::Assistant(message)
}

fn turn(journal: &mut SessionJournal, label: &str) -> [String; 2] {
    [journal.append_message(&user(label)).unwrap(), journal.append_message(&assistant(label)).unwrap()]
}

fn prefix_ids(snapshot: &ProjectedCompactionSnapshot, count: usize) -> Vec<String> {
    snapshot.messages[..count].iter().map(|message| message.entry_id.clone()).collect()
}

fn two_soft_summaries(journal: &mut SessionJournal) {
    let first = turn(journal, "first");
    let second = turn(journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    assert!(snapshot.previous_summary.is_none());
    let c1 = journal.commit_projected_compaction(&snapshot, "first completed", &second[0], &first, 100).unwrap();
    let third = turn(journal, "third");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    assert_eq!(snapshot.previous_summary.as_ref().unwrap().entry_id, c1);
    assert_eq!(prefix_ids(&snapshot, 2), second);
    let c2 =
        journal.commit_projected_compaction(&snapshot, "first and second completed", &third[0], &second, 80).unwrap();
    let projection = journal.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 3);
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("latest summary") };
    assert_eq!(summary.entry_id, c2);
    assert_eq!(summary.source_entry_ids.as_ref().unwrap(), &first.into_iter().chain(second).collect::<Vec<_>>());
    assert_eq!(summary.first_kept_entry_id, third[0]);
    assert_eq!(summary.summary, "first and second completed");
    assert_eq!(journal.entries().len(), 8, "six raw observations and both derived summaries remain");
    let context = journal.model_context();
    assert_eq!(context.len(), 3);
    let Message::User(summary_message) = &context[0] else { panic!("lower trust summary projection") };
    assert!(summary_message.content.plain_text().contains("first and second completed"));
    assert_eq!(context[1], user("third"));
    assert_eq!(context[2], assistant("third"));
    let record = journal.entries().last().unwrap();
    assert_eq!(record.raw["method"], "soft");
    assert!(record.raw.get("previousCompactionEntryId").is_none(), "reuse the existing native record shape");
    assert!(record.raw.get("windowSourceEntryIds").is_none());
}

#[test]
fn consecutive_soft_summaries_preserve_cumulative_raw_sources_and_reopen_projection() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    two_soft_summaries(&mut journal);
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.compacted_context_projection().unwrap(), journal.compacted_context_projection().unwrap());
    assert_eq!(reopened.projected_compaction_snapshot().unwrap(), journal.projected_compaction_snapshot().unwrap());
    assert_eq!(reopened.entries(), journal.entries());
    assert!(
        matches!(reopened.compaction_source_snapshot(), Err(CompactionSourceError::UnsupportedContextEntry { kind, .. }) if kind == "compaction"),
        "the old CLI snapshot remains single-level"
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn native_memory_journal_supports_two_soft_summaries_without_file_materialization() {
    let mut journal = SessionJournal::in_memory(
        json!({"type":"session","version":3,"id":"memory-session","timestamp":"t","cwd":"/"}),
    )
    .unwrap();
    assert!(!journal.is_on_disk());
    two_soft_summaries(&mut journal);
    assert!(!journal.is_on_disk());
    assert!(journal.path().as_os_str().is_empty());
}

#[test]
fn foreign_or_modified_projected_snapshots_are_rejected_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    journal.commit_projected_compaction(&snapshot, "first completed", &second[0], &first, 100).unwrap();
    let third = turn(&mut journal, "third");
    let current = journal.projected_compaction_snapshot().unwrap();
    let window = prefix_ids(&current, 2);
    let mut wrong_session = current.clone();
    wrong_session.session_id = "foreign-session".into();
    let mut wrong_leaf = current.clone();
    wrong_leaf.leaf_id = "foreign-leaf".into();
    let mut wrong_previous = current.clone();
    wrong_previous.previous_summary.as_mut().unwrap().entry_id = "foreign-compaction".into();
    let mut missing_previous = current.clone();
    missing_previous.previous_summary = None;
    let mut wrong_messages = current.clone();
    wrong_messages.messages[0].message = user("altered source");
    let entries = journal.entries().to_vec();
    let bytes = fs::read(journal.path()).unwrap();
    for altered in [wrong_session, wrong_leaf, wrong_previous, missing_previous, wrong_messages] {
        assert!(matches!(
            journal.commit_projected_compaction(&altered, "updated", &third[0], &window, 80),
            Err(CompactionCommitError::StaleSnapshot)
        ));
        assert_eq!(journal.entries(), entries);
        assert_eq!(journal.leaf_id(), Some(current.leaf_id.as_str()));
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
}

#[test]
fn a_changed_branch_invalidates_a_prepared_summary() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    journal.append_message(&user("new input while summary was pending")).unwrap();
    let entries = journal.entries().to_vec();
    let bytes = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_projected_compaction(&snapshot, "old snapshot", &second[0], &first, 100),
        Err(CompactionCommitError::StaleSnapshot)
    ));
    assert_eq!(journal.entries(), entries);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
}

#[test]
fn commit_requires_a_nonempty_exact_window_and_kept_user_boundary_before_writing() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    let bytes = fs::read(journal.path()).unwrap();
    let entries = journal.entries().to_vec();
    for window in [vec![], vec![first[0].clone()], vec![first[1].clone(), first[0].clone()], prefix_ids(&snapshot, 4)] {
        assert!(matches!(
            journal.commit_projected_compaction(&snapshot, "summary", &second[0], &window, 100),
            Err(CompactionCommitError::InvalidWindow)
        ));
    }
    for kept in ["missing", first[0].as_str(), first[1].as_str(), second[1].as_str()] {
        assert!(matches!(
            journal.commit_projected_compaction(&snapshot, "summary", kept, &first, 100),
            Err(CompactionCommitError::InvalidWindow)
        ));
    }
    for summary in ["   ".to_owned(), "x".repeat(1_000_001)] {
        assert!(matches!(
            journal.commit_projected_compaction(&snapshot, &summary, &second[0], &first, 100),
            Err(CompactionCommitError::InvalidSummary)
        ));
    }
    assert_eq!(journal.entries(), entries);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    journal.commit_projected_compaction(&snapshot, "valid summary", &second[0], &first, 100).unwrap();
    let next = journal.projected_compaction_snapshot().unwrap();
    let bytes = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_projected_compaction(&next, "no new history", &second[0], &[], 80),
        Err(CompactionCommitError::InvalidWindow)
    ));
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
}

fn call() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "call-1".into(),
        name: "bash".into(),
        arguments: JsonObject::new(),
        thought_signature: None,
    }));
    message.stop_reason = StopReason::ToolUse;
    Message::Assistant(message)
}

fn receipt(id: &str, details: Option<Value>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "bash".into(),
        content: vec![UserBlock::text("receipt")],
        details,
        is_error: true,
        timestamp: 3,
    })
}

#[test]
fn commit_cannot_hide_images_unpaired_calls_or_unknown_effect_receipts() {
    let image = ImageContent { data: "YWJj".into(), mime_type: "image/png".into() };
    let image_user = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(image.clone())]),
        synthetic: None,
        timestamp: 1,
    });
    let mut image_assistant = AssistantMessage::empty("openai-completions", "fixture", "model");
    image_assistant.content.push(AssistantBlock::Image(image));
    let cases = [
        vec![image_user, assistant("answer")],
        vec![user("question"), Message::Assistant(image_assistant)],
        vec![user("question"), call()],
        vec![user("question"), receipt("unpaired", None)],
        vec![
            user("question"),
            call(),
            receipt(
                "call-1",
                Some(json!({"__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown"})),
            ),
        ],
    ];
    for messages in cases {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        let window: Vec<String> = messages.iter().map(|message| journal.append_message(message).unwrap()).collect();
        let kept = turn(&mut journal, "kept");
        let snapshot = journal.projected_compaction_snapshot().unwrap();
        let entries = journal.entries().to_vec();
        let bytes = fs::read(journal.path()).unwrap();
        assert!(matches!(
            journal.commit_projected_compaction(&snapshot, "unsafe summary", &kept[0], &window, 100),
            Err(CompactionCommitError::UnsafeSummaryBoundary)
        ));
        assert_eq!(journal.entries(), entries);
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
}

#[test]
fn legacy_summary_without_source_ids_and_corrupt_provenance_cannot_be_updated() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let c1 = journal.append_compaction("first completed", &second[0], &first, 100).unwrap();
    let third = turn(&mut journal, "third");
    let path = journal.path().to_path_buf();
    let mut lines: Vec<Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let compaction_index = lines.iter().position(|line| line["id"] == c1).unwrap();
    lines[compaction_index].as_object_mut().unwrap().remove("sourceEntryIds");
    fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let bytes = fs::read(&path).unwrap();
    let mut legacy = SessionJournal::open(&path).unwrap();
    let snapshot = legacy.projected_compaction_snapshot().unwrap();
    assert!(snapshot.previous_summary.as_ref().unwrap().source_entry_ids.is_none());
    assert!(matches!(
        legacy.commit_projected_compaction(&snapshot, "updated", &third[0], &second, 80),
        Err(CompactionCommitError::MissingPreviousSources)
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    lines[compaction_index]["sourceEntryIds"] = json!(["foreign-source"]);
    fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(matches!(
        SessionJournal::open(&path).unwrap().projected_compaction_snapshot(),
        Err(CompactionProjectionError::SourceIdsMismatch { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn excluded_native_bash_is_retained_raw_and_never_added_to_summary_sources_or_projected_messages() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let q1 = journal.append_message(&user("first")).unwrap();
    let hidden = BashExecutionMessage {
        command: "private".into(),
        output: "private receipt".into(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        meta: None,
        timestamp: 4,
        exclude_from_context: Some(true),
    };
    let hidden1 = journal.append_bash_execution(&hidden).unwrap();
    let a1 = journal.append_message(&assistant("first")).unwrap();
    let second = turn(&mut journal, "second");
    let hidden2 = journal.append_bash_execution(&hidden).unwrap();
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    assert_eq!(snapshot.messages.len(), 4);
    journal.commit_projected_compaction(&snapshot, "first completed", &second[0], &[q1, a1], 100).unwrap();
    let third = turn(&mut journal, "third");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    assert_eq!(snapshot.messages.len(), 4);
    journal.commit_projected_compaction(&snapshot, "first and second completed", &third[0], &second, 80).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.projected_compaction_snapshot().unwrap().messages.len(), 2);
    assert_eq!(reopened.model_context().len(), 3);
    assert!(
        reopened.entries().iter().any(|entry| entry.id == hidden1 && entry.bash_execution() == Some(hidden.clone()))
    );
    assert!(
        reopened.entries().iter().any(|entry| entry.id == hidden2 && entry.bash_execution() == Some(hidden.clone()))
    );
    let sources = reopened.projected_compaction_snapshot().unwrap().previous_summary.unwrap().source_entry_ids.unwrap();
    assert!(!sources.contains(&hidden1) && !sources.contains(&hidden2));
    assert_eq!(reopened.undecodable_messages(), 0);
}

#[test]
fn failed_storage_append_rolls_back_entries_and_leaf_before_a_later_valid_commit() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let entries = journal.entries().to_vec();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(matches!(
        journal.commit_projected_compaction(&snapshot, "summary", &second[0], &first, 100),
        Err(CompactionCommitError::Storage(_))
    ));
    assert_eq!(journal.entries(), entries);
    assert_eq!(journal.leaf_id(), Some(snapshot.leaf_id.as_str()));
    fs::remove_dir(&path).unwrap();
    fs::write(&path, bytes).unwrap();
    journal.materialize().unwrap();
    journal.commit_projected_compaction(&snapshot, "summary", &second[0], &first, 100).unwrap();
    assert_eq!(
        SessionJournal::open(&path).unwrap().compacted_context_projection().unwrap(),
        journal.compacted_context_projection().unwrap()
    );
}
