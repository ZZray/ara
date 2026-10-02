use ara_ai::{
    AssistantBlock, AssistantMessage, ImageContent, JsonObject, Message, StopReason, ToolCall, ToolResultMessage,
    UserBlock, UserContent, UserMessage,
};
use ara_session::{
    BashExecutionMessage, CompactedContextItem, CompactionCommitError, CompactionProjectionError,
    CompactionSourceError, NativeEntryOrigin, ProjectedCompactionSnapshot, SessionJournal,
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
fn verified_archive_migrates_opaque_metadata_across_two_soft_summaries_without_admitting_new_images() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let picture =
        || ImageContent { data: "AQI=".into(), mime_type: "image/png".into(), detail: None, compaction_frame: false };
    let first = [
        journal
            .append_message(&Message::User(UserMessage {
                content: UserContent::Blocks(vec![
                    UserBlock::text("prior source picture"),
                    UserBlock::Image(picture()),
                ]),
                synthetic: None,
                timestamp: 1,
            }))
            .unwrap(),
        journal.append_message(&assistant("prior source read")).unwrap(),
    ];
    let second = turn(&mut journal, "second retained");
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let archive = ara_session::NativeSnapcompactSummary {
        summary: "Archive caption".into(),
        short_summary: None,
        preserve_data: Some(
            json!({"snapcompact":{"text":"Full retained archive source.","textHead":"Full retained archive source.",
            "frames":[{"data":"AQI=","mimeType":"image/png","chars":1,"cols":1,"rows":1}]},
            "opaqueHostState":{"marker":"unchanged","nested":[1,2,3]}}),
        ),
        read_files: vec![],
        modified_files: vec![],
    };
    let archive_id = journal.commit_native_entry_snapcompact(&snapshot, &archive, &second[0], &first, 200).unwrap();
    let third = turn(&mut journal, "third retained");
    let snapshot = journal.native_projected_compaction_snapshot().unwrap();
    let before = journal.entries().to_vec();
    let disk = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_native_entry_compaction(&snapshot, "", &third[0], &second, 100),
        Err(CompactionCommitError::InvalidSummary)
    ));
    assert_eq!(journal.entries(), before);
    assert_eq!(fs::read(journal.path()).unwrap(), disk, "failure keeps the old frame archive");
    journal
        .commit_native_entry_compaction(&snapshot, "Archive plus second completed", &third[0], &second, 100)
        .unwrap();
    let mut journal = SessionJournal::open(journal.path()).unwrap();
    let raw = &journal.entries().last().unwrap().raw;
    assert_eq!(raw["preserveData"], json!({"opaqueHostState":{"marker":"unchanged","nested":[1,2,3]}}));
    assert_eq!(raw["archiveSourceEntryId"], archive_id);
    let fourth = turn(&mut journal, "fourth retained");
    let snapshot = journal.native_projected_compaction_snapshot().unwrap();
    journal
        .commit_native_entry_compaction(&snapshot, "Archive plus second and third completed", &fourth[0], &third, 80)
        .unwrap();
    let mut journal = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(journal.entries().last().unwrap().raw["archiveSourceEntryId"], archive_id);
    assert_eq!(
        journal
            .native_projected_compaction_snapshot()
            .unwrap()
            .previous_summary
            .unwrap()
            .archive_image_boundary
            .unwrap()
            .entry_id,
        archive_id
    );
    // The grant describes prior archive coverage, never the current new window.
    let new_picture_id = journal
        .append_message(&Message::User(UserMessage {
            content: UserContent::Blocks(vec![UserBlock::Image(picture())]),
            synthetic: None,
            timestamp: 1,
        }))
        .unwrap();
    let new_answer_id = journal.append_message(&assistant("new picture processed")).unwrap();
    let fifth = turn(&mut journal, "fifth retained");
    let snapshot = journal.native_projected_compaction_snapshot().unwrap();
    let window = fourth.into_iter().chain([new_picture_id, new_answer_id]).collect::<Vec<_>>();
    let disk = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_native_entry_compaction(&snapshot, "cannot hide the new picture", &fifth[0], &window, 60),
        Err(CompactionCommitError::UnsafeSummaryBoundary)
    ));
    assert_eq!(fs::read(journal.path()).unwrap(), disk);
}

/// Fixed native cuts can summarize a user prompt before its answer, then
/// summarize that retained Assistant on the next cut without losing raw IDs.
#[test]
fn native_split_cuts_reopen_and_accumulate_exact_sources_without_split_metadata() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.projected_compaction_snapshot().unwrap();
    let first_window = prefix_ids(&snapshot, 3);
    let bytes = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_projected_compaction(&snapshot, "split", &second[1], &first_window, 100),
        Err(CompactionCommitError::InvalidWindow)
    ));
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    journal
        .commit_native_projected_compaction(&snapshot, "first done; second pending", &second[1], &first_window, 100)
        .unwrap();
    let mut reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.entries(), journal.entries());
    let carried = reopened.projected_compaction_snapshot().unwrap();
    assert_eq!(carried.messages.len(), 1);
    assert_eq!(carried.messages[0].entry_id, second[1]);
    assert!(matches!(carried.messages[0].message, Message::Assistant(_)));
    assert_eq!(carried.previous_summary.unwrap().source_entry_ids.unwrap(), first_window);
    let third = turn(&mut reopened, "third");
    let snapshot = reopened.projected_compaction_snapshot().unwrap();
    let window = prefix_ids(&snapshot, 2);
    assert_eq!(window, [second[1].clone(), third[0].clone()]);
    reopened
        .commit_native_projected_compaction(&snapshot, "first and second done; third pending", &third[1], &window, 80)
        .unwrap();
    let reloaded = SessionJournal::open(reopened.path()).unwrap();
    assert_eq!(reloaded.compacted_context_projection().unwrap(), reopened.compacted_context_projection().unwrap());
    let snapshot = reloaded.projected_compaction_snapshot().unwrap();
    assert_eq!(snapshot.messages.len(), 1);
    assert_eq!(snapshot.messages[0].entry_id, third[1]);
    assert_eq!(
        snapshot.previous_summary.unwrap().source_entry_ids.unwrap(),
        first.into_iter().chain(second).chain([third[0].clone()]).collect::<Vec<_>>()
    );
    assert!(
        reloaded
            .entries()
            .iter()
            .filter(|entry| entry.kind == "compaction")
            .all(|entry| entry.raw.get("splitTurn").is_none())
    );
    // The new Assistant replay boundary always needs exact raw provenance;
    // imported User-boundary summaries retain their existing unknown-ID API.
    let last_id = reloaded.entries().last().unwrap().id.clone();
    let mut lines: Vec<Value> =
        fs::read_to_string(reloaded.path()).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    lines.iter_mut().find(|line| line["id"] == last_id).unwrap().as_object_mut().unwrap().remove("sourceEntryIds");
    fs::write(reloaded.path(), lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    assert_eq!(
        SessionJournal::open(reloaded.path()).unwrap().compacted_context_projection(),
        Err(CompactionProjectionError::SourceIdsMismatch { id: last_id })
    );
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
    let image =
        ImageContent { detail: None, compaction_frame: false, data: "YWJj".into(), mime_type: "image/png".into() };
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
        let split_window = prefix_ids(&snapshot, window.len() + 1);
        assert!(matches!(
            journal.commit_native_projected_compaction(&snapshot, "unsafe split", &kept[1], &split_window, 100),
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

/// The three fixed OMP compact-reset-boundary.test.ts input families, using
/// native Session IDs and checked commits rather than invented kept IDs for
/// the active summary. Cleared malformed summaries remain available raw.
#[test]
fn native_reset_boundaries_follow_fixed_compaction_input_families() {
    for family in ["raw-before-reset", "summary-before-reset", "summary-after-reset"] {
        let directory = tempfile::tempdir().unwrap();
        let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
        journal.append_model_change("fixture/retained-model").unwrap();
        let old = turn(&mut journal, "PRECLEAR");
        if family == "summary-before-reset" {
            // Fixed native family deliberately has an unresolvable old kept
            // ID. It must not poison the fresh post-clear context.
            journal.append_compaction("OLD SUMMARY", "kept-old", &old, 0).unwrap();
            turn(&mut journal, "MIDCLEAR");
            assert!(matches!(
                journal.compacted_context_projection(),
                Err(CompactionProjectionError::MissingKeptMessage { .. })
            ));
        }
        let raw_before = journal.entries().to_vec();
        let parent = journal.leaf_id().unwrap().to_owned();
        let reset = journal.append_reset_boundary().unwrap();
        let marker = journal.entries().last().unwrap();
        assert_eq!(marker.parent_id.as_deref(), Some(parent.as_str()));
        assert_eq!(marker.kind, "reset_boundary");
        let mut fields = marker.raw.as_object().unwrap().keys().map(String::as_str).collect::<Vec<_>>();
        fields.sort_unstable();
        assert_eq!(fields, ["id", "parentId", "timestamp", "type"]);
        let first = turn(&mut journal, "POST first");
        let second = turn(&mut journal, "POST second");
        let mut expected_sources = Vec::new();
        if family == "summary-after-reset" {
            let snapshot = journal.projected_compaction_snapshot().unwrap();
            journal.commit_projected_compaction(&snapshot, "KEEP SUMMARY", &second[0], &first, 100).unwrap();
            expected_sources.extend(first.clone());
            turn(&mut journal, "POST third");
        }
        let snapshot = journal.projected_compaction_snapshot().unwrap();
        assert_eq!(
            snapshot.previous_summary.as_ref().map(|summary| summary.summary.as_str()),
            (family == "summary-after-reset").then_some("KEEP SUMMARY")
        );
        let projected =
            serde_json::to_string(&snapshot.messages.iter().map(|source| &source.message).collect::<Vec<_>>()).unwrap();
        assert!(
            !projected.contains("PRECLEAR") && !projected.contains("MIDCLEAR") && !projected.contains("OLD SUMMARY")
        );
        if family == "summary-after-reset" {
            assert!(!projected.contains("POST first"));
        } else {
            assert!(projected.contains("POST first"));
            assert_eq!(journal.compaction_source_snapshot().unwrap().messages, snapshot.messages);
        }
        let split = snapshot.messages.last().unwrap().entry_id.clone();
        let window = prefix_ids(&snapshot, snapshot.messages.len() - 1);
        expected_sources.extend(window.clone());
        journal.commit_native_projected_compaction(&snapshot, "POST SUMMARY", &split, &window, 100).unwrap();
        let next = turn(&mut journal, "NEXT");
        let snapshot = journal.projected_compaction_snapshot().unwrap();
        let window = prefix_ids(&snapshot, 2);
        assert_eq!(window, [split, next[0].clone()]);
        expected_sources.extend(window.clone());
        journal.commit_native_projected_compaction(&snapshot, "UPDATED POST SUMMARY", &next[1], &window, 80).unwrap();
        let path = journal.path().to_path_buf();
        let bytes = fs::read(&path).unwrap();
        let reopened = SessionJournal::open(&path).unwrap();
        let snapshot = reopened.projected_compaction_snapshot().unwrap();
        assert_eq!(snapshot.messages.len(), 1);
        assert_eq!(snapshot.messages[0].entry_id, next[1]);
        assert_eq!(snapshot.previous_summary.unwrap().source_entry_ids.unwrap(), expected_sources);
        assert!(!expected_sources.contains(&reset));
        assert!(old.iter().all(|id| !expected_sources.contains(id)));
        assert_eq!(&reopened.entries()[..raw_before.len()], raw_before);
        assert_eq!(reopened.entries(), journal.entries());
        assert_eq!(reopened.branch().len(), reopened.entries().len(), "full transcript keeps pre-clear entries");
        assert_eq!(reopened.header(), journal.header());
        assert_eq!(reopened.title(), journal.title());
        assert_eq!(reopened.current_model().as_deref(), Some("fixture/retained-model"));
        assert_eq!(reopened.model_context().len(), 2);
        assert_eq!(reopened.model_context()[1], assistant("NEXT"));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn reset_boundaries_handle_empty_context_stale_summaries_and_failed_storage() {
    let mut memory =
        SessionJournal::in_memory(json!({"type":"session","version":3,"id":"memory-reset","timestamp":"t","cwd":"/"}))
            .unwrap();
    memory.append_reset_boundary().unwrap();
    memory.append_reset_boundary().unwrap();
    assert!(memory.build_context().is_empty());
    assert!(memory.model_context().is_empty());
    assert!(memory.projected_compaction_snapshot().unwrap().messages.is_empty());
    assert!(!memory.is_on_disk());
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let stale = journal.projected_compaction_snapshot().unwrap();
    let path = journal.path().to_path_buf();
    let bytes = fs::read(&path).unwrap();
    let entries = journal.entries().to_vec();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(journal.append_reset_boundary().is_err());
    assert_eq!(journal.entries(), entries);
    assert_eq!(journal.leaf_id(), Some(stale.leaf_id.as_str()));
    assert_eq!(journal.build_context().len(), 4, "failed clear preserves the active conversation");
    fs::remove_dir(&path).unwrap();
    fs::write(&path, &bytes).unwrap();
    journal.materialize().unwrap();
    journal.append_reset_boundary().unwrap();
    let cleared = journal.projected_compaction_snapshot().unwrap();
    assert!(cleared.previous_summary.is_none() && cleared.messages.is_empty());
    assert!(journal.compaction_source_snapshot().unwrap().messages.is_empty());
    let bytes = fs::read(&path).unwrap();
    assert!(matches!(
        journal.commit_native_projected_compaction(&stale, "stale summary", &second[1], &prefix_ids(&stale, 3), 100),
        Err(CompactionCommitError::StaleSnapshot)
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    journal.append_message(&user("discarded between two clears")).unwrap();
    journal.append_reset_boundary().unwrap();
    let kept = turn(&mut journal, "only active turn");
    let active = journal.projected_compaction_snapshot().unwrap();
    assert_eq!(prefix_ids(&active, 2), kept);
    assert!(first.iter().all(|id| !prefix_ids(&active, 2).contains(id)));
    let bytes = fs::read(&path).unwrap();
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.model_context(), [user("only active turn"), assistant("only active turn")]);
    assert_eq!(reopened.projected_compaction_snapshot().unwrap(), active);
    assert_eq!(reopened.entries(), journal.entries());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let invalid = journal.append_compaction("active summary with cleared sources", &kept[1], &first, 100).unwrap();
    assert_eq!(
        journal.compacted_context_projection(),
        Err(CompactionProjectionError::SourceIdsMismatch { id: invalid })
    );
    assert_eq!(
        journal.model_context(),
        [user("only active turn"), assistant("only active turn")],
        "invalid active compaction falls back only to the post-reset suffix"
    );
}

#[test]
fn reset_boundaries_preserve_raw_unknown_effects_without_recovering_cleared_calls() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.append_message(&user("old tool effects")).unwrap();
    journal.append_message(&call()).unwrap();
    let unknown = receipt(
        "call-1",
        Some(json!({
            "__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown"
        })),
    );
    let unknown_id = journal.append_message(&unknown).unwrap();
    let mut pending = call();
    let Message::Assistant(message) = &mut pending else { unreachable!() };
    let AssistantBlock::ToolCall(tool) = &mut message.content[0] else { unreachable!() };
    tool.id = "cleared-pending".into();
    let pending_id = journal.append_message(&pending).unwrap();
    journal.append_reset_boundary().unwrap();
    let bytes = fs::read(journal.path()).unwrap();
    let recovery = journal.recover_interrupted_tool_calls().unwrap();
    assert!(recovery.paired.is_empty() && recovery.unpaired_earlier.is_empty());
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    journal.append_message(&user("active tool")).unwrap();
    journal.append_message(&call()).unwrap();
    // A cleared result with this same call ID cannot answer the active call.
    let mut reopened = SessionJournal::open(journal.path()).unwrap();
    let recovery = reopened.recover_interrupted_tool_calls().unwrap();
    assert_eq!(recovery.paired, ["call-1"]);
    assert!(recovery.unpaired_earlier.is_empty());
    assert_eq!(reopened.entries().iter().find(|entry| entry.id == unknown_id).unwrap().message(), Some(unknown));
    assert_eq!(reopened.entries().iter().find(|entry| entry.id == pending_id).unwrap().message(), Some(pending));
    assert_eq!(
        reopened
            .entries()
            .iter()
            .filter(|entry| entry.raw.pointer("/message/toolCallId").and_then(Value::as_str) == Some("cleared-pending"))
            .count(),
        0
    );
    let kept = turn(&mut reopened, "kept");
    let snapshot = reopened.projected_compaction_snapshot().unwrap();
    let window = prefix_ids(&snapshot, 3);
    let bytes = fs::read(reopened.path()).unwrap();
    assert!(matches!(
        reopened.commit_projected_compaction(&snapshot, "cannot hide active unknown effects", &kept[0], &window, 100),
        Err(CompactionCommitError::UnsafeSummaryBoundary)
    ));
    assert_eq!(fs::read(reopened.path()).unwrap(), bytes);
    reopened.append_reset_boundary().unwrap();
    let first = turn(&mut reopened, "fresh after unknown effects were cleared");
    let kept = turn(&mut reopened, "fresh kept");
    let snapshot = reopened.projected_compaction_snapshot().unwrap();
    reopened.commit_projected_compaction(&snapshot, "fresh summary", &kept[0], &first, 100).unwrap();
    assert_eq!(
        reopened.projected_compaction_snapshot().unwrap().previous_summary.unwrap().source_entry_ids.unwrap(),
        first
    );
    assert!(reopened.entries().iter().any(|entry| entry.id == unknown_id));
}

#[test]
fn reset_boundaries_do_not_bypass_full_raw_parent_and_id_validation() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let old = turn(&mut journal, "old");
    journal.append_reset_boundary().unwrap();
    turn(&mut journal, "active");
    let path = journal.path().to_path_buf();
    let lines: Vec<Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for fault in ["duplicate-id", "missing-parent"] {
        let mut damaged = lines.clone();
        let index = damaged.iter().position(|line| line["id"] == old[0]).unwrap();
        if fault == "duplicate-id" {
            damaged[index]["id"] = json!(old[1]);
        } else {
            damaged[index]["parentId"] = json!("missing-before-reset");
        }
        fs::write(&path, damaged.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let bytes = fs::read(&path).unwrap();
        let reopened = SessionJournal::open(&path).unwrap();
        let expected = if fault == "duplicate-id" {
            CompactionSourceError::DuplicateEntryId { id: old[1].clone() }
        } else {
            CompactionSourceError::MissingEntry { id: "missing-before-reset".into() }
        };
        assert_eq!(reopened.compaction_source_snapshot(), Err(expected.clone()));
        assert_eq!(reopened.compacted_context_projection(), Err(CompactionProjectionError::Source(expected)));
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

fn native_fixture(entries: &[Value]) -> (tempfile::TempDir, SessionJournal) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("native.jsonl");
    let mut lines = vec![
        json!({"type":"session","version":3,"id":"native-entry-session","timestamp":"2026-10-02T00:00:00Z","cwd":"/"}),
    ];
    for (index, fields) in entries.iter().enumerate() {
        let mut raw = json!({"id":format!("raw{index}"),"parentId":if index == 0 { Value::Null } else { json!(format!("raw{}", index-1)) },"timestamp":"2026-10-02T00:00:00Z"});
        raw.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
        lines.push(raw);
    }
    fs::write(&path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    (directory, journal)
}

#[test]
fn native_entry_groups_preserve_metadata_kept_ids_split_cuts_and_reopen_sources() {
    for split in [false, true] {
        let mut raw = vec![
            json!({"type":"message","message":user("first")}),
            json!({"type":"message","message":assistant("first")}),
        ];
        if split {
            raw.push(json!({"type":"message","message":user("second")}));
        }
        let kept_index = raw.len();
        raw.extend([
            json!({"type":"custom","customType":"extension-state","data":{"retained":42}}),
            json!({"type":"service_tier_change","serviceTier":{"openai":"default"}}),
        ]);
        if !split {
            raw.push(json!({"type":"message","message":user("second")}));
        }
        raw.push(json!({"type":"message","message":assistant("second")}));
        let (_directory, mut journal) = native_fixture(&raw);
        let snapshot = journal.native_projected_compaction_snapshot().unwrap();
        assert_eq!(snapshot.entries.len(), raw.len());
        assert_eq!(snapshot.entries[kept_index].origin, NativeEntryOrigin::Metadata);
        assert!(
            snapshot.entries[kept_index].messages.is_empty()
                && snapshot.entries[kept_index].raw_token_message.is_none()
        );
        assert_eq!(snapshot.entries.iter().filter(|entry| entry.raw_token_message.is_some()).count(), 4);
        let window: Vec<String> = snapshot.entries[..kept_index].iter().map(|entry| entry.entry_id.clone()).collect();
        let kept_id = snapshot.entries[kept_index].entry_id.clone();
        journal
            .commit_native_entry_compaction(
                &snapshot,
                "first and optional pending second prompt",
                &kept_id,
                &window,
                100,
            )
            .unwrap();
        let context = journal.model_context();
        assert_eq!(context.len(), if split { 2 } else { 3 });
        assert_eq!(context.last().unwrap(), &assistant("second"));
        let next = turn(&mut journal, "next");
        let snapshot = journal.native_projected_compaction_snapshot().unwrap();
        assert_eq!(snapshot.entries[0].entry_id, kept_id);
        assert_eq!(snapshot.entries[0].origin, NativeEntryOrigin::Metadata);
        assert!(snapshot.entries.iter().any(|entry| entry.origin == NativeEntryOrigin::CompactionBoundary));
        let index = snapshot.entries.iter().position(|entry| entry.entry_id == next[0]).unwrap();
        let second_window: Vec<String> = snapshot.entries[..index]
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect();
        journal
            .commit_native_entry_compaction(&snapshot, "all previous turns done", &next[0], &second_window, 80)
            .unwrap();
        let bytes = fs::read(journal.path()).unwrap();
        let reopened = SessionJournal::open(journal.path()).unwrap();
        assert_eq!(reopened.entries(), journal.entries());
        assert_eq!(
            reopened.native_projected_compaction_snapshot().unwrap(),
            journal.native_projected_compaction_snapshot().unwrap()
        );
        let expected: Vec<String> = window.into_iter().chain(second_window).collect();
        assert_eq!(
            reopened
                .native_projected_compaction_snapshot()
                .unwrap()
                .previous_summary
                .unwrap()
                .source_entry_ids
                .unwrap(),
            expected
        );
        assert!(!expected.contains(&kept_id));
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
}

#[test]
fn native_entry_origins_keep_custom_branch_skill_steering_and_legacy_projection() {
    let notice = ara_session::LoopGuardNotice::thinking_loop().event_message();
    let custom =
        json!({"type":"custom_message","customType":"extension-prompt","content":"generic source","display":true});
    let legacy_custom = json!({"type":"message","message":{"role":"custom","customType":"legacy-prompt","content":"legacy custom","display":false,"timestamp":4}});
    let cases = [
        (custom, NativeEntryOrigin::CustomMessage),
        (
            json!({"type":"branch_summary","summary":"branch source","fromId":"prior-branch"}),
            NativeEntryOrigin::BranchSummary,
        ),
        (
            json!({"type":"message","message":{"role":"hookMessage","customType":"old-hook","content":"hook source","display":false,"timestamp":4}}),
            NativeEntryOrigin::HookMessage,
        ),
        (legacy_custom, NativeEntryOrigin::LegacyCustomMessage),
        (
            json!({"type":"message","message":{"role":"branchSummary","summary":"legacy branch","fromId":"prior-branch","timestamp":4}}),
            NativeEntryOrigin::LegacyBranchSummary,
        ),
        (
            json!({"type":"message","message":{"role":"compactionSummary","summary":"legacy summary","tokensBefore":100,"timestamp":4}}),
            NativeEntryOrigin::LegacyCompactionSummary,
        ),
        (
            json!({"type":"custom_message","customType":"skill-prompt","content":"user skill","display":true,"attribution":"user"}),
            NativeEntryOrigin::UserSkill,
        ),
        (
            json!({"type":"custom_message","customType":notice["customType"],"content":notice["content"],"display":false,"attribution":"agent"}),
            NativeEntryOrigin::LoopGuardNotice,
        ),
        (
            json!({"type":"custom_message","customType":"collab-prompt","content":[{"type":"text","text":"steer one"},{"type":"text","text":"steer two"}],"display":true,"attribution":"user"}),
            NativeEntryOrigin::SteeringUser,
        ),
        (
            json!({"type":"custom_message","customType":"skill-prompt","content":"agent skill","display":false,"attribution":"agent"}),
            NativeEntryOrigin::CustomMessage,
        ),
    ];
    let mut raw = Vec::new();
    for (entry, _) in &cases {
        raw.extend([entry.clone(), json!({"type":"message","message":assistant("done")})]);
    }
    let kept_index = raw.len();
    raw.extend([
        json!({"type":"message","message":user("kept")}),
        json!({"type":"message","message":assistant("kept")}),
    ]);
    let (_directory, mut journal) = native_fixture(&raw);
    let snapshot = journal.native_projected_compaction_snapshot().unwrap();
    for (index, (_, origin)) in cases.iter().enumerate() {
        let group = &snapshot.entries[index * 2];
        assert_eq!(group.origin, *origin);
        assert_eq!(group.messages.len(), 1);
        assert_eq!(group.raw_token_message.is_some(), raw[index * 2]["type"] == "message");
        match origin {
            NativeEntryOrigin::CustomMessage
            | NativeEntryOrigin::LoopGuardNotice
            | NativeEntryOrigin::HookMessage
            | NativeEntryOrigin::LegacyCustomMessage => assert!(matches!(group.messages[0], Message::Developer(_))),
            _ => assert!(matches!(group.messages[0], Message::User(_))),
        }
    }
    let Message::User(steering) = &snapshot.entries[16].messages[0] else { panic!("steering User fragment") };
    assert!(steering.content.plain_text().contains("</system-notice>\nsteer one\nsteer two"));
    let sources: Vec<String> = snapshot.entries[..kept_index].iter().map(|entry| entry.entry_id.clone()).collect();
    let kept_id = snapshot.entries[kept_index].entry_id.clone();
    let bytes = fs::read(journal.path()).unwrap();
    for fault in ["origin", "raw-token", "fragment", "order", "owner"] {
        let mut stale = snapshot.clone();
        match fault {
            "origin" => stale.entries[0].origin = NativeEntryOrigin::User,
            "raw-token" => stale.entries[0].raw_token_message = Some(user("fake raw budget")),
            "fragment" => stale.entries[0].messages[0] = user("fake projection"),
            "order" => stale.entries.swap(0, 1),
            _ => stale.session_id = "foreign".into(),
        }
        assert!(matches!(
            journal.commit_native_entry_compaction(&stale, "forged", &kept_id, &sources, 100),
            Err(CompactionCommitError::StaleSnapshot)
        ));
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
    journal
        .commit_native_entry_compaction(&snapshot, "custom and branch history completed", &kept_id, &sources, 100)
        .unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(
        reopened.native_projected_compaction_snapshot().unwrap().previous_summary.unwrap().source_entry_ids.unwrap(),
        sources
    );
    assert_eq!(reopened.entries(), journal.entries());
    assert_eq!(reopened.model_context().len(), 3);
}

#[test]
fn native_multi_fragments_stay_one_raw_group_and_images_cannot_be_hidden() {
    for kind in ["custom_message", "hookMessage"] {
        let payload = json!({"customType":"mixed-attachment","content":[{"type":"text","text":"one"},{"type":"image","data":"YWJj","mimeType":"image/png"},{"type":"text","text":"two"}],"display":false,"timestamp":4});
        let mixed = if kind == "custom_message" {
            let mut raw = payload.clone();
            raw.as_object_mut().unwrap().remove("timestamp");
            raw["type"] = json!(kind);
            raw
        } else {
            let mut message = payload;
            message["role"] = json!(kind);
            json!({"type":"message","message":message})
        };
        let hidden = json!({"type":"message","message":{"role":"bashExecution","command":"private","output":"excluded output","cancelled":false,"truncated":false,"timestamp":4,"excludeFromContext":true}});
        let raw = [
            json!({"type":"message","message":user("first")}),
            json!({"type":"message","message":assistant("first")}),
            hidden,
            mixed,
            json!({"type":"message","message":assistant("attachment received")}),
            json!({"type":"message","message":user("kept")}),
            json!({"type":"message","message":assistant("kept")}),
        ];
        let (_directory, mut journal) = native_fixture(&raw);
        let snapshot = journal.native_projected_compaction_snapshot().unwrap();
        assert!(snapshot.entries[2].messages.is_empty() && snapshot.entries[2].raw_token_message.is_some());
        let mixed = &snapshot.entries[3];
        assert_eq!(mixed.messages.len(), 2);
        assert_eq!(mixed.raw_token_message.is_some(), kind == "hookMessage");
        let Message::Developer(text) = &mixed.messages[0] else { panic!("text Developer") };
        assert_eq!(text.content.plain_text(), "one\ntwo");
        let Message::User(images) = &mixed.messages[1] else { panic!("images User") };
        assert_eq!(images.content.plain_text(), "Images attached to mixed-attachment.");
        assert!(journal.entries()[3].message().is_none(), "single-message API must not drop a fragment");
        assert!(matches!(
            journal.projected_compaction_snapshot(),
            Err(CompactionProjectionError::Source(CompactionSourceError::MultipleMessageProjection { .. }))
        ));
        let bytes = fs::read(journal.path()).unwrap();
        let unsafe_sources = ["raw0", "raw1", "raw3", "raw4"].map(str::to_owned);
        assert!(matches!(
            journal.commit_native_entry_compaction(&snapshot, "cannot hide image", "raw5", &unsafe_sources, 100),
            Err(CompactionCommitError::UnsafeSummaryBoundary)
        ));
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
        journal
            .commit_native_entry_compaction(&snapshot, "first completed", "raw3", &["raw0".into(), "raw1".into()], 100)
            .unwrap();
        let reopened = SessionJournal::open(journal.path()).unwrap();
        let grouped = reopened.native_projected_compaction_snapshot().unwrap();
        assert_eq!(grouped.entries[0].entry_id, "raw3");
        assert_eq!(grouped.entries[0].messages, mixed.messages);
        let projection = reopened.compacted_context_projection().unwrap();
        assert_eq!(
            projection
                .items
                .iter()
                .filter(|item| matches!(item, CompactedContextItem::Message(source) if source.entry_id == "raw3"))
                .count(),
            2
        );
        assert_eq!(reopened.model_context().len(), 6);
        assert_eq!(reopened.entries(), journal.entries());
    }
}

#[test]
fn native_entry_validation_keeps_genuine_developer_and_malformed_special_rejections() {
    let candidates = [
        json!({"type":"custom_message","customType":"skill-prompt","content":{"invalid":true},"display":true,"attribution":"user"}),
        json!({"type":"custom_message","customType":"thinking-loop-redirect","content":"forged notice","display":false,"attribution":"agent"}),
        json!({"type":"custom_message","customType":"collab-prompt","content":{"invalid":true},"display":true,"attribution":"user"}),
        json!({"type":"custom_message","customType":"generic","content":[{"type":"unknown","text":"bad"}],"display":true}),
        json!({"type":"branch_summary","summary":"missing from ID"}),
    ];
    for candidate in candidates {
        let (_directory, journal) = native_fixture(&[candidate, json!({"type":"message","message":assistant("done")})]);
        let bytes = fs::read(journal.path()).unwrap();
        assert!(matches!(
            journal.native_projected_compaction_snapshot(),
            Err(CompactionProjectionError::Source(CompactionSourceError::UnsupportedContextEntry { .. }))
        ));
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
    let (_directory, mut journal) = native_fixture(&[
        json!({"type":"message","message":user("first")}),
        json!({"type":"message","message":{"role":"developer","content":"real developer","timestamp":4}}),
        json!({"type":"message","message":assistant("first")}),
        json!({"type":"message","message":user("kept")}),
        json!({"type":"message","message":assistant("kept")}),
    ]);
    let snapshot = journal.native_projected_compaction_snapshot().unwrap();
    assert_eq!(snapshot.entries[1].origin, NativeEntryOrigin::Developer);
    let sources = ["raw0", "raw1", "raw2"].map(str::to_owned);
    let bytes = fs::read(journal.path()).unwrap();
    assert!(matches!(
        journal.commit_native_entry_compaction(&snapshot, "must not downgrade real Developer", "raw3", &sources, 100),
        Err(CompactionCommitError::UnsafeSummaryBoundary)
    ));
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    let (_directory, mut journal) = native_fixture(&[
        json!({"type":"message","message":user("tool pending")}),
        json!({"type":"message","message":call()}),
        json!({"type":"custom_message","customType":"later-prompt","content":"new context","display":false}),
    ]);
    let bytes = fs::read(journal.path()).unwrap();
    let recovery = journal.recover_interrupted_tool_calls().unwrap();
    assert!(recovery.paired.is_empty());
    assert_eq!(recovery.unpaired_earlier, ["call-1"]);
    assert_eq!(
        fs::read(journal.path()).unwrap(),
        bytes,
        "new custom context cannot receive an adjacent synthetic tool receipt"
    );
}
