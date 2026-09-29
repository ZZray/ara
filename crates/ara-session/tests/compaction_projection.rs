use ara_ai::{
    AssistantBlock, AssistantMessage, DeveloperMessage, JsonObject, Message, StopReason, ToolCall, ToolResultMessage,
    UserBlock, UserContent, UserMessage,
};
use ara_session::{CompactedContextItem, CompactionProjectionError, CompactionSourceError, SessionJournal};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fake", "m");
    message.content.push(AssistantBlock::text(text));
    Message::Assistant(message)
}

fn tool_call(id: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fake", "m");
    message.content.push(AssistantBlock::ToolCall(ToolCall {
        id: id.into(),
        name: "bash".into(),
        arguments: JsonObject::new(),
        thought_signature: None,
    }));
    message.stop_reason = StopReason::ToolUse;
    Message::Assistant(message)
}

fn tool_result(id: &str, details: Option<Value>) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "bash".into(),
        content: vec![UserBlock::text("ok")],
        details,
        is_error: false,
        timestamp: 1,
    })
}

fn entry(kind: &str, id: &str, parent: Value, extra: Value) -> Value {
    let mut raw = json!({"type":kind,"id":id,"parentId":parent,"timestamp":"2026-09-27T00:00:00.000Z"});
    raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    raw
}

fn message(id: &str, parent: Value, value: Message) -> Value {
    entry("message", id, parent, json!({"message":value}))
}

fn compaction(id: &str, parent: Value, first_kept: &str, sources: Value) -> Value {
    let mut extra = json!({
        "summary":"Earlier work completed.",
        "firstKeptEntryId":first_kept,
        "tokensBefore":100,
        "method":"soft",
    });
    if !sources.is_null() {
        extra["sourceEntryIds"] = sources;
    }
    entry("compaction", id, parent, extra)
}

fn write_journal(path: &Path, entries: &[Value]) {
    let mut lines = vec![json!({"type":"session","version":3,"id":"session-1","timestamp":"t","cwd":"/"})];
    lines.extend_from_slice(entries);
    fs::write(path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
}

fn base_entries() -> Vec<Value> {
    vec![
        message("q1", Value::Null, user("question one")),
        message("a1", json!("q1"), assistant("answer one")),
        message("q2", json!("a1"), user("question two")),
        message("a2", json!("q2"), assistant("answer two")),
    ]
}

fn projected_ids(journal: &SessionJournal) -> Vec<String> {
    journal
        .compacted_context_projection()
        .unwrap()
        .items
        .iter()
        .map(|item| match item {
            CompactedContextItem::Summary(summary) => format!("summary:{}", summary.entry_id),
            CompactedContextItem::Message(message) => message.entry_id.clone(),
        })
        .collect()
}

#[test]
fn summary_precedes_kept_and_later_messages_after_restart_without_erasing_raw_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut entries = base_entries();
    entries.push(compaction("c1", json!("a2"), "q2", json!(["q1", "a1"])));
    entries.push(message("q3", json!("c1"), user("question three")));
    entries.push(message("a3", json!("q3"), assistant("answer three")));
    write_journal(&path, &entries);

    let journal = SessionJournal::open(&path).unwrap();
    let original = fs::read(&path).unwrap();
    let projection = journal.compacted_context_projection().unwrap();
    assert_eq!(projection.session_id, "session-1");
    assert_eq!(projection.leaf_id, "a3");
    assert_eq!(projected_ids(&journal), ["summary:c1", "q2", "a2", "q3", "a3"]);
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("summary first") };
    assert_eq!(summary.summary, "Earlier work completed.");
    assert_eq!(summary.first_kept_entry_id, "q2");
    assert_eq!(summary.source_entry_ids.as_ref().unwrap(), &["q1", "a1"]);
    assert_eq!(summary.tokens_before, 100);
    assert_eq!(journal.branch().len(), 7, "raw entries remain available");
    assert_eq!(journal.build_context().len(), 6, "existing tolerant projection is unchanged");
    assert_eq!(
        journal.compaction_source_snapshot(),
        Err(CompactionSourceError::UnsupportedContextEntry { id: "c1".into(), kind: "compaction".into() })
    );
    assert_eq!(fs::read(&path).unwrap(), original, "projection is read-only");
}

#[test]
fn latest_compaction_supersedes_earlier_summary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut entries = base_entries();
    entries.push(compaction("c1", json!("a2"), "q2", json!(["q1", "a1"])));
    entries.push(message("q3", json!("c1"), user("question three")));
    entries.push(message("a3", json!("q3"), assistant("answer three")));
    entries.push(compaction("c2", json!("a3"), "q3", json!(["q1", "a1", "q2", "a2"])));
    entries.push(message("q4", json!("c2"), user("question four")));
    write_journal(&path, &entries);

    let journal = SessionJournal::open(&path).unwrap();
    assert_eq!(projected_ids(&journal), ["summary:c2", "q3", "a3", "q4"]);
    assert_eq!(journal.branch().len(), 9);
}

#[test]
fn no_compaction_projects_all_messages_and_legacy_summary_has_unknown_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut entries = base_entries();
    write_journal(&path, &entries);
    assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["q1", "a1", "q2", "a2"]);
    entries.push(compaction("c1", json!("a2"), "q2", Value::Null));
    write_journal(&path, &entries);
    let projection = SessionJournal::open(&path).unwrap().compacted_context_projection().unwrap();
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("summary first") };
    assert_eq!(summary.source_entry_ids, None);
}

#[test]
fn malformed_compactions_and_off_branch_kept_ids_fail_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut entries = base_entries();
    entries.push(message("aside", json!("a1"), user("off branch")));
    let good = compaction("c1", json!("a2"), "q2", json!(["q1", "a1"]));
    let cases = [
        (
            compaction("c1", json!("a2"), "aside", json!(["q1", "a1"])),
            CompactionProjectionError::MissingKeptMessage { id: "c1".into() },
        ),
        (
            compaction("c1", json!("a2"), "missing", json!(["q1", "a1"])),
            CompactionProjectionError::MissingKeptMessage { id: "c1".into() },
        ),
        (
            compaction("c1", json!("a2"), "a1", json!(["q1"])),
            CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() },
        ),
        (
            compaction("c1", json!("a2"), "q2", json!(["a1", "q1"])),
            CompactionProjectionError::SourceIdsMismatch { id: "c1".into() },
        ),
        (
            compaction("c1", json!("a2"), "q2", json!(["q1", "q1"])),
            CompactionProjectionError::SourceIdsMismatch { id: "c1".into() },
        ),
        (
            compaction("c1", json!("a2"), "q2", json!(["q1", 42])),
            CompactionProjectionError::InvalidField { id: "c1".into(), field: "sourceEntryIds" },
        ),
        (
            {
                let mut e = good.clone();
                e["summary"] = json!("   ");
                e
            },
            CompactionProjectionError::InvalidField { id: "c1".into(), field: "summary" },
        ),
        (
            {
                let mut e = good.clone();
                e["firstKeptEntryId"] = json!(4);
                e
            },
            CompactionProjectionError::InvalidField { id: "c1".into(), field: "firstKeptEntryId" },
        ),
        (
            {
                let mut e = good.clone();
                e["tokensBefore"] = json!(-1);
                e
            },
            CompactionProjectionError::InvalidField { id: "c1".into(), field: "tokensBefore" },
        ),
        (
            {
                let mut e = good.clone();
                e["method"] = json!("remote");
                e
            },
            CompactionProjectionError::UnsupportedMethod { id: "c1".into(), method: "remote".into() },
        ),
        (
            {
                let mut e = good.clone();
                e["preserveData"] = json!({"openaiRemoteCompaction":{"replacementHistory":[]}});
                e
            },
            CompactionProjectionError::UnsupportedReplayData { id: "c1".into() },
        ),
        (
            {
                let mut e = good.clone();
                e["providerReplayThroughEntryId"] = json!("a1");
                e
            },
            CompactionProjectionError::UnsupportedReplayData { id: "c1".into() },
        ),
    ];
    for (candidate, expected) in cases {
        let mut case_entries = entries.clone();
        case_entries.push(candidate);
        write_journal(&path, &case_entries);
        let before = fs::read(&path).unwrap();
        assert_eq!(SessionJournal::open(&path).unwrap().compacted_context_projection(), Err(expected));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[test]
fn strict_projection_rejects_undecodable_message_and_unknown_context_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut entries = base_entries();
    entries[0]["message"] = json!({"role":"user"});
    write_journal(&path, &entries);
    assert_eq!(
        SessionJournal::open(&path).unwrap().compacted_context_projection(),
        Err(CompactionProjectionError::Source(CompactionSourceError::UndecodableMessage { id: "q1".into() }))
    );

    let mut entries = base_entries();
    entries.push(entry("custom_message", "x", json!("a2"), json!({})));
    write_journal(&path, &entries);
    assert_eq!(
        SessionJournal::open(&path).unwrap().compacted_context_projection(),
        Err(CompactionProjectionError::Source(CompactionSourceError::UnsupportedContextEntry {
            id: "x".into(),
            kind: "custom_message".into(),
        }))
    );
}

#[test]
fn refuses_to_hide_developer_unfinished_or_unknown_effect_turns_but_accepts_completed_tool_cycle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let developer = Message::Developer(DeveloperMessage {
        content: UserContent::Text("priority instruction".into()),
        timestamp: 1,
    });
    let cases = [
        vec![
            message("q1", Value::Null, user("q1")),
            message("a1", json!("q1"), assistant("a1")),
            message("d1", json!("a1"), developer),
            message("q2", json!("d1"), user("q2")),
            message("a2", json!("q2"), assistant("a2")),
            compaction("c1", json!("a2"), "q2", json!(["q1", "a1", "d1"])),
        ],
        vec![
            message("q1", Value::Null, user("q1")),
            message("a1", json!("q1"), tool_call("t1")),
            message("q2", json!("a1"), user("q2")),
            message("a2", json!("q2"), assistant("a2")),
            compaction("c1", json!("a2"), "q2", json!(["q1", "a1"])),
        ],
        vec![
            message("q1", Value::Null, user("q1")),
            message("a1", json!("q1"), tool_call("t1")),
            message(
                "r1",
                json!("a1"),
                tool_result(
                    "t1",
                    Some(json!({"__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown"})),
                ),
            ),
            message("a2", json!("r1"), assistant("a2")),
            message("q2", json!("a2"), user("q2")),
            message("a3", json!("q2"), assistant("a3")),
            compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1", "a2"])),
        ],
    ];
    for entries in cases {
        write_journal(&path, &entries);
        assert_eq!(
            SessionJournal::open(&path).unwrap().compacted_context_projection(),
            Err(CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() })
        );
    }

    let entries = vec![
        message("q1", Value::Null, user("q1")),
        message("a1", json!("q1"), tool_call("t1")),
        message("r1", json!("a1"), tool_result("t1", None)),
        message("a2", json!("r1"), assistant("a2")),
        message("q2", json!("a2"), user("q2")),
        message("a3", json!("q2"), assistant("a3")),
        compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1", "a2"])),
    ];
    write_journal(&path, &entries);
    assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["summary:c1", "q2", "a3"]);
}

#[test]
fn projection_rejects_panic_receipts_but_keeps_completed_failures() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let mut receipt = tool_result("t1", Some(json!({"panicked": true})));
    let Message::ToolResult(result) = &mut receipt else { unreachable!() };
    result.is_error = true;
    let entries = vec![
        message("q1", Value::Null, user("run a command")),
        message("a1", json!("q1"), tool_call("t1")),
        message("r1", json!("a1"), receipt),
        message("a2", json!("r1"), assistant("continued")),
        message("q2", json!("a2"), user("next question")),
        message("a3", json!("q2"), assistant("answer")),
        compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1", "a2"])),
    ];
    write_journal(&path, &entries);
    assert_eq!(
        SessionJournal::open(&path).unwrap().compacted_context_projection(),
        Err(CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() })
    );

    let mut receipt = tool_result("t1", Some(json!({"exitCode": 1})));
    let Message::ToolResult(result) = &mut receipt else { unreachable!() };
    result.is_error = true;
    let entries = vec![
        message("q1", Value::Null, user("run a command")),
        message("a1", json!("q1"), tool_call("t1")),
        message("r1", json!("a1"), receipt),
        message("a2", json!("r1"), assistant("continued")),
        message("q2", json!("a2"), user("next question")),
        message("a3", json!("q2"), assistant("answer")),
        compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1", "a2"])),
    ];
    write_journal(&path, &entries);
    assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["summary:c1", "q2", "a3"]);
}

/// The Session guard agrees with the Agent's span rule: turns that ended
/// aborted, errored or length-stopped, or after tool results, are summarized
/// (fixed OMP `findValidCutPoints`); an unanswered prompt is not.
#[test]
fn projection_summarizes_aborted_errored_and_budget_stopped_turns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let stopped = |reason: StopReason| {
        let Message::Assistant(mut message) = assistant("") else { unreachable!() };
        message.content.clear();
        message.stop_reason = reason;
        Message::Assistant(message)
    };
    for reason in [StopReason::Aborted, StopReason::Error, StopReason::Length] {
        let entries = vec![
            message("q1", Value::Null, user("run slow")),
            message("a1", json!("q1"), tool_call("t1")),
            message("r1", json!("a1"), tool_result("t1", None)),
            message("a2", json!("r1"), stopped(reason)),
            message("q2", json!("a2"), user("next")),
            message("a3", json!("q2"), assistant("answer")),
            compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1", "a2"])),
        ];
        write_journal(&path, &entries);
        assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["summary:c1", "q2", "a3"], "{reason:?}");
    }

    let budget_stopped = vec![
        message("q1", Value::Null, user("run")),
        message("a1", json!("q1"), tool_call("t1")),
        message("r1", json!("a1"), tool_result("t1", None)),
        message("q2", json!("r1"), user("next")),
        message("a3", json!("q2"), assistant("answer")),
        compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1"])),
    ];
    write_journal(&path, &budget_stopped);
    assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["summary:c1", "q2", "a3"]);

    // A call the loop did not run after an aborted or length stop is paired
    // with a synthetic `executed: false` result, as in the Agent rule.
    for (reason, source) in
        [(StopReason::Aborted, "assistant_stop_aborted"), (StopReason::Length, "assistant_stop_length")]
    {
        let Message::Assistant(mut stopped_call) = tool_call("t1") else { unreachable!() };
        stopped_call.stop_reason = reason;
        let paired = vec![
            message("q1", Value::Null, user("run")),
            message("a1", json!("q1"), Message::Assistant(stopped_call.clone())),
            message(
                "r1",
                json!("a1"),
                tool_result("t1", Some(json!({"__synthetic": true, "source": source, "executed": false}))),
            ),
            message("q2", json!("r1"), user("next")),
            message("a3", json!("q2"), assistant("answer")),
            compaction("c1", json!("a3"), "q2", json!(["q1", "a1", "r1"])),
        ];
        write_journal(&path, &paired);
        assert_eq!(projected_ids(&SessionJournal::open(&path).unwrap()), ["summary:c1", "q2", "a3"], "{reason:?}");

        let unpaired = vec![
            message("q1", Value::Null, user("run")),
            message("a1", json!("q1"), Message::Assistant(stopped_call)),
            message("q2", json!("a1"), user("next")),
            message("a3", json!("q2"), assistant("answer")),
            compaction("c1", json!("a3"), "q2", json!(["q1", "a1"])),
        ];
        write_journal(&path, &unpaired);
        assert_eq!(
            SessionJournal::open(&path).unwrap().compacted_context_projection(),
            Err(CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() }),
            "{reason:?}"
        );
    }

    let unanswered = vec![
        message("q1", Value::Null, user("never answered")),
        message("q2", json!("q1"), user("next")),
        message("a3", json!("q2"), assistant("answer")),
        compaction("c1", json!("a3"), "q2", json!(["q1"])),
    ];
    write_journal(&path, &unanswered);
    assert_eq!(
        SessionJournal::open(&path).unwrap().compacted_context_projection(),
        Err(CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() })
    );
}
