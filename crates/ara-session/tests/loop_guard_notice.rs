//! Whole-module fixed notice provenance, original branch and restart scenario.
use ara_ai::{AssistantBlock, AssistantMessage, Message, StopReason, ThinkingContent, UserMessage};
use ara_session::{LoopGuardNotice, SessionJournal};
use serde_json::json;

#[test]
fn fixed_agent_notices_reopen_project_to_developer_and_preserve_compaction_sources() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_message(&Message::User(UserMessage::text("original request"))).unwrap();
    let mut aborted = AssistantMessage::empty("openai-completions", "fixture", "google/gemini-3.5-flash");
    aborted.stop_reason = StopReason::Aborted;
    aborted
        .content
        .push(AssistantBlock::Thinking(ThinkingContent { thinking: "**Planning**".into(), thinking_signature: None }));
    let aborted_id = journal.append_message(&Message::Assistant(aborted)).unwrap();
    let reminder = LoopGuardNotice::gemini_headers(36);
    let reminder_id = journal.append_gemini_reminder_after_aborted(&reminder, &aborted_id).unwrap();
    let redirect = LoopGuardNotice::thinking_loop();
    let first = journal.append_loop_guard_notice(&redirect).unwrap();
    let second = journal.append_loop_guard_notice(&redirect).unwrap();
    assert_ne!(first, second, "Two actual retries retain two independent receipts");
    let tool_loop = LoopGuardNotice::tool_call_loop(&ara_ai::tool_call_loop_guard::RepeatedToolCallDetection {
        kind: "repeated_tool_call",
        tool_name: "bash".into(),
        count: 2.0,
        arguments_summary: "{\"command\":\"pytest -q\"}".into(),
        result_summary: "1263 passed, 4 skipped".into(),
    })
    .unwrap();
    let tool_loop_id = journal.append_loop_guard_notice(&tool_loop).unwrap();
    assert!(
        !tool_loop.event_message()["content"].as_str().unwrap().ends_with('\n'),
        "Fixed prompt.render formats the redirect"
    );
    assert_eq!(LoopGuardNotice::from_event_message(&tool_loop.event_message()), Some(tool_loop));
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.header()["id"], journal.header()["id"]);
    assert!(reopened.entries().iter().any(|entry| entry.id == aborted_id), "raw interruption receipt is retained");
    assert!(
        !reopened.branch().iter().any(|entry| entry.id == aborted_id),
        "only the interrupted active leaf is bypassed"
    );
    let notice_ids = [reminder_id, first, second, tool_loop_id];
    let snapshot = reopened.compaction_source_snapshot().unwrap();
    for id in &notice_ids {
        let entry = reopened.entries().iter().find(|entry| &entry.id == id).unwrap();
        let native = entry.loop_guard_notice().unwrap().event_message();
        assert_eq!(native["role"], "custom");
        assert_eq!(native["display"], false);
        assert_eq!(native["attribution"], "agent");
        assert!(matches!(entry.message(), Some(Message::Developer(_))));
        assert!(
            snapshot
                .messages
                .iter()
                .any(|source| &source.entry_id == id && matches!(source.message, Message::Developer(_)))
        );
    }
    assert_eq!(reopened.undecodable_messages(), 0);

    // A model-authored string or forged custom receipt must not acquire the
    // fixed trusted Developer projection merely by naming the custom type.
    let source = std::fs::read_to_string(journal.path()).unwrap();
    let mut rows =
        source.lines().map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()).collect::<Vec<_>>();
    let notice = rows.iter_mut().find(|row| row["id"] == json!(notice_ids[3])).unwrap();
    notice["content"] = json!("untrusted captured error text");
    std::fs::write(journal.path(), rows.iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
    let forged = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(forged.undecodable_messages(), 1);
    assert!(forged.compaction_source_snapshot().is_err());
}
