use ara_ai::{
    AssistantBlock, AssistantMessage, ImageContent, JsonObject, Message, StopReason, ToolCall, UserBlock, UserContent,
    UserMessage,
};
use ara_session::{
    CompactedContextItem, CompactionProjectionError, CompactionSourceError, Recovery, SessionJournal, UserSkillPrompt,
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const TIMESTAMP: i64 = 1_790_467_200_123;

fn skill(text: &str) -> UserSkillPrompt {
    UserSkillPrompt {
        content: UserContent::Text(text.into()),
        details: Some(
            json!({"name":"review","path":"/skills/review/SKILL.md","args":"target","lineCount":3,"originalInput":"/skill:review target"}),
        ),
        timestamp: TIMESTAMP,
    }
}

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

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn write_journal(path: &Path, entries: &[Value]) {
    let mut lines = vec![json!({"type":"session","version":3,"id":"session-1","timestamp":"t","cwd":"/"})];
    lines.extend_from_slice(entries);
    fs::write(path, lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
}

#[test]
fn persists_one_custom_entry_and_reopens_without_skill_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let prompt = skill("expanded Skill body");
    let expected = prompt.model_message();
    let answer = assistant("done");
    assert_eq!(prompt.event_message()["role"], "custom");
    assert_eq!(prompt.event_message()["customType"], "skill-prompt");
    assert_eq!(prompt.event_message()["timestamp"], TIMESTAMP);
    let id = journal.append_skill_prompt(&prompt).unwrap();
    journal.append_message(&answer).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);

    let raw = lines(&path);
    let custom: Vec<&Value> = raw.iter().filter(|entry| entry["type"] == "custom_message").collect();
    assert_eq!(custom.len(), 1);
    assert_eq!(raw.iter().filter(|entry| entry["type"] == "message" && entry["message"]["role"] == "user").count(), 0);
    assert_eq!(custom[0]["id"], id);
    assert_eq!(custom[0]["customType"], "skill-prompt");
    assert_eq!(custom[0]["content"], "expanded Skill body");
    assert_eq!(custom[0]["display"], true);
    assert_eq!(custom[0]["attribution"], "user");
    assert_eq!(custom[0]["details"], prompt.details.clone().unwrap());
    assert_eq!(custom[0]["timestamp"], "2026-09-27T00:00:00.123Z");

    // Projection depends on the recorded payload, never on a live SKILL.md.
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.branch()[0].skill_prompt(), Some(prompt.clone()));
    assert!(reopened.branch()[1].skill_prompt().is_none());
    assert_eq!(reopened.build_context(), vec![expected.clone(), answer]);
    assert_eq!(reopened.model_context(), reopened.build_context());
    let snapshot = reopened.compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.messages[0].entry_id, id);
    assert_eq!(snapshot.messages[0].message, expected);
    let Message::User(projected) = &snapshot.messages[0].message else { panic!("skill projects as user") };
    assert!(
        matches!(&projected.content, UserContent::Blocks(blocks) if matches!(&blocks[..], [UserBlock::Text(text)] if text.text == "expanded Skill body"))
    );
}

#[test]
fn block_content_and_images_remain_typed_without_details_in_model() {
    let content = UserContent::Blocks(vec![
        UserBlock::text("instructions"),
        UserBlock::Image(ImageContent { data: "aGVsbG8=".into(), mime_type: "image/png".into() }),
    ]);
    let prompt = UserSkillPrompt {
        content: content.clone(),
        details: Some(json!({"private":"metadata"})),
        timestamp: TIMESTAMP,
    };
    let model = prompt.model_message();
    assert!(matches!(&model, Message::User(user) if user.content == content));
    assert!(!serde_json::to_string(&model).unwrap().contains("private"));

    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_skill_prompt(&prompt).unwrap();
    journal.append_message(&assistant("response")).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.build_context()[0], model);
    assert_eq!(reopened.compaction_source_snapshot().unwrap().messages[0].message, model);
}

#[test]
fn image_bearing_skill_is_not_summarized_by_text_only_compaction() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let prompt = UserSkillPrompt {
        content: UserContent::Blocks(vec![
            UserBlock::text("inspect image"),
            UserBlock::Image(ImageContent { data: "aGVsbG8=".into(), mime_type: "image/png".into() }),
        ]),
        details: None,
        timestamp: TIMESTAMP,
    };
    let skill_id = journal.append_skill_prompt(&prompt).unwrap();
    let answer = journal.append_message(&assistant("observed")).unwrap();
    let kept = journal.append_message(&user("next turn")).unwrap();
    journal.append_message(&assistant("done")).unwrap();
    journal.append_compaction("summary", &kept, &[skill_id, answer], 100).unwrap();
    assert!(matches!(
        journal.compacted_context_projection(),
        Err(CompactionProjectionError::UnsafeSummaryBoundary { .. })
    ));
}

#[test]
fn custom_skill_can_be_first_kept_message_with_real_source_ids() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let first = journal.append_message(&user("first")).unwrap();
    let first_answer = journal.append_message(&assistant("answered")).unwrap();
    let kept = journal.append_skill_prompt(&skill("kept Skill")).unwrap();
    let kept_answer = journal.append_message(&assistant("done")).unwrap();
    journal.append_compaction("summary", &kept, &[first.clone(), first_answer.clone()], 100).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    let projection = reopened.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 3);
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("summary") };
    assert_eq!(summary.first_kept_entry_id, kept);
    assert_eq!(summary.source_entry_ids.as_ref().unwrap(), &[first, first_answer]);
    let CompactedContextItem::Message(message) = &projection.items[1] else { panic!("kept Skill") };
    assert_eq!(message.entry_id, kept);
    let CompactedContextItem::Message(message) = &projection.items[2] else { panic!("kept answer") };
    assert_eq!(message.entry_id, kept_answer);
    assert_eq!(reopened.model_context().len(), 3);
    assert_eq!(
        reopened.branch().iter().find(|entry| entry.id == kept).unwrap().raw["details"]["originalInput"],
        "/skill:review target"
    );
}

#[test]
fn summarized_skill_id_is_required_exactly_in_source_list() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let skill_id = journal.append_skill_prompt(&skill("first Skill")).unwrap();
    let answer = journal.append_message(&assistant("answered")).unwrap();
    let kept = journal.append_message(&user("later")).unwrap();
    journal.append_message(&assistant("done")).unwrap();
    let snapshot = journal.compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.messages[0].entry_id, skill_id);
    journal.append_compaction("summary", &kept, &[skill_id.clone(), answer.clone()], 100).unwrap();
    assert!(journal.compacted_context_projection().is_ok());

    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_skill_prompt(&skill("first Skill")).unwrap();
    let answer = journal.append_message(&assistant("answered")).unwrap();
    let kept = journal.append_message(&user("later")).unwrap();
    journal.append_message(&assistant("done")).unwrap();
    journal.append_compaction("summary", &kept, &[answer], 100).unwrap();
    assert!(matches!(journal.compacted_context_projection(), Err(CompactionProjectionError::SourceIdsMismatch { .. })));
}

#[test]
fn unknown_and_agent_attributed_custom_entries_stay_unsupported() {
    for (custom_type, attribution) in [("other", "user"), ("skill-prompt", "agent")] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        write_journal(
            &path,
            &[
                json!({"type":"custom_message","id":"custom","parentId":null,"timestamp":"2026-09-27T00:00:00.123Z","customType":custom_type,"content":"x","display":true,"attribution":attribution}),
                json!({"type":"message","id":"answer","parentId":"custom","timestamp":"2026-09-27T00:00:01.000Z","message":assistant("done")}),
            ],
        );
        let journal = SessionJournal::open(&path).unwrap();
        assert!(journal.branch()[0].skill_prompt().is_none());
        assert_eq!(journal.build_context().len(), 1);
        assert_eq!(journal.undecodable_messages(), 0);
        assert_eq!(
            journal.compaction_source_snapshot(),
            Err(CompactionSourceError::UnsupportedContextEntry { id: "custom".into(), kind: "custom_message".into() })
        );
        assert!(matches!(
            journal.compacted_context_projection(),
            Err(CompactionProjectionError::Source(CompactionSourceError::UnsupportedContextEntry { .. }))
        ));
    }
}

#[test]
fn historical_skill_content_does_not_depend_on_details_or_current_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(
        &path,
        &[
            json!({"type":"custom_message","id":"custom","parentId":null,"timestamp":"2026-09-27T00:00:00.123Z","customType":"skill-prompt","content":"saved body","display":false,"attribution":"user","details":{"oldShape":42}}),
            json!({"type":"message","id":"answer","parentId":"custom","timestamp":"2026-09-27T00:00:01.000Z","message":assistant("done")}),
        ],
    );
    let journal = SessionJournal::open(&path).unwrap();
    let snapshot = journal.compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.messages[0].entry_id, "custom");
    assert!(
        matches!(&snapshot.messages[0].message, Message::User(user) if user.timestamp == TIMESTAMP && user.content.plain_text() == "saved body")
    );

    let mut invalid = lines(&path);
    invalid[1]["timestamp"] = json!("invalid");
    write_journal(&path, &invalid[1..]);
    let journal = SessionJournal::open(&path).unwrap();
    assert!(journal.branch()[0].skill_prompt().is_none());
    assert!(journal.build_context().iter().all(|message| !matches!(message, Message::User(_))));
    assert_eq!(journal.undecodable_messages(), 1, "the host can warn about the omitted Skill");
    assert!(
        matches!(journal.compaction_source_snapshot(), Err(CompactionSourceError::UnsupportedContextEntry { id, .. }) if id == "custom")
    );
}

#[test]
fn recovery_does_not_pair_unknown_tool_effect_across_user_skill() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_message(&user("first")).unwrap();
    journal.append_message(&tool_call("call-1")).unwrap();
    let skill_id = journal.append_skill_prompt(&skill("next turn")).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);

    let mut reopened = SessionJournal::open(&path).unwrap();
    let before = fs::read(&path).unwrap();
    assert_eq!(
        reopened.recover_interrupted_tool_calls().unwrap(),
        Recovery { paired: vec![], unpaired_earlier: vec!["call-1".into()] }
    );
    assert_eq!(fs::read(&path).unwrap(), before, "no synthetic result appended after the Skill");
    assert_eq!(reopened.leaf_id(), Some(skill_id.as_str()));
}

#[test]
fn recovery_keeps_malformed_user_skill_as_a_raw_turn_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(
        &path,
        &[
            json!({"type":"message","id":"user","parentId":null,"timestamp":"2026-09-27T00:00:00.000Z","message":user("first")}),
            json!({"type":"message","id":"assistant","parentId":"user","timestamp":"2026-09-27T00:00:01.000Z","message":tool_call("call-1")}),
            json!({"type":"custom_message","id":"custom","parentId":"assistant","timestamp":"invalid","customType":"skill-prompt","content":"saved body","display":true,"attribution":"user"}),
        ],
    );
    let before = fs::read(&path).unwrap();
    let mut journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.undecodable_messages(), 1);
    assert_eq!(
        journal.recover_interrupted_tool_calls().unwrap(),
        Recovery { paired: vec![], unpaired_earlier: vec!["call-1".into()] }
    );
    assert_eq!(fs::read(&path).unwrap(), before);
}
