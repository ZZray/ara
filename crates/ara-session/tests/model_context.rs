//! V1-COMPACT: `model_context` after `append_compaction` — summary projection,
//! raw history retention, reopen stability, and fallback on invalid entries.

use ara_ai::{AssistantBlock, AssistantMessage, Message, StopReason, UserMessage};
use ara_session::SessionJournal;
use serde_json::{Value, json};
use std::fs;

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fake", "m");
    message.content.push(AssistantBlock::text(text));
    message.stop_reason = StopReason::Stop;
    Message::Assistant(message)
}

fn seed_compacted() -> (tempfile::TempDir, std::path::PathBuf, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_model_change("fake/m").unwrap();
    let q1 = journal.append_message(&user("question one")).unwrap();
    let a1 = journal.append_message(&assistant("answer one")).unwrap();
    let q2 = journal.append_message(&user("question two")).unwrap();
    let a2 = journal.append_message(&assistant("answer two")).unwrap();
    journal.append_compaction("Earlier work completed.", &q2, &[q1.clone(), a1.clone()], 4).unwrap();
    let path = journal.path().to_path_buf();
    (dir, path, vec![q1, a1, q2, a2])
}

fn as_user_text(message: &Message) -> Option<&str> {
    match message {
        Message::User(user) => match &user.content {
            ara_ai::UserContent::Text(text) => Some(text.as_str()),
            ara_ai::UserContent::Blocks(_) => None,
        },
        _ => None,
    }
}

fn assistant_text(message: &Message) -> String {
    match message {
        Message::Assistant(assistant) => assistant
            .content
            .iter()
            .filter_map(|block| match block {
                AssistantBlock::Text(text) => Some(text.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

#[test]
fn model_context_replaces_the_summarized_prefix_with_a_summary_user_message() {
    let (_dir, path, ids) = seed_compacted();
    let journal = SessionJournal::open(&path).unwrap();
    let context = journal.model_context();
    assert_eq!(context.len(), 3, "summary + kept user + kept assistant");
    let summary = as_user_text(&context[0]).expect("summary is a user message");
    assert!(summary.starts_with("[Compacted summary of earlier turns"), "{summary}");
    assert!(summary.contains("Earlier work completed."), "{summary}");
    assert_eq!(as_user_text(&context[1]), Some("question two"));
    assert_eq!(assistant_text(&context[2]), "answer two");
    // The summarized raw messages are gone from the projection.
    let texts: Vec<String> = context.iter().filter_map(|message| as_user_text(message).map(str::to_string)).collect();
    assert!(!texts.iter().any(|t| t.contains("question one")), "{texts:?}");
    let answers: Vec<String> = context.iter().map(assistant_text).collect();
    assert!(!answers.iter().any(|t| t.contains("answer one")), "{answers:?}");
    // firstKeptEntryId points at the kept user entry.
    let entries = fs::read_to_string(&path).unwrap();
    let compaction_line =
        entries.lines().find(|line| line.contains("\"type\":\"compaction\"")).expect("compaction entry");
    let value: Value = serde_json::from_str(compaction_line).unwrap();
    assert_eq!(value["firstKeptEntryId"], json!(ids[2]));
    assert_eq!(value["method"], json!("soft"));
}

#[test]
fn raw_summarized_entries_stay_in_the_journal_file() {
    let (_dir, path, ids) = seed_compacted();
    let before = fs::read_to_string(&path).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    let _ = journal.model_context();
    assert_eq!(fs::read_to_string(&path).unwrap(), before, "model_context is read-only");
    for id in &ids {
        assert!(before.contains(&format!("\"id\":\"{id}\"")), "entry {id} remains in the file");
    }
    assert!(before.contains("question one") && before.contains("answer one"), "raw text remains");
    assert!(before.contains("question two") && before.contains("answer two"), "kept text remains");
}

#[test]
fn reopen_yields_the_same_model_context() {
    let (_dir, path, _) = seed_compacted();
    let before = SessionJournal::open(&path).unwrap().model_context();
    let after = SessionJournal::open(&path).unwrap().model_context();
    // The synthesized summary user message gets a fresh timestamp; compare content.
    let texts = |context: &[Message]| -> Vec<(String, String)> {
        context
            .iter()
            .map(|message| {
                (
                    message.role().to_string(),
                    match message {
                        Message::User(user) => match &user.content {
                            ara_ai::UserContent::Text(text) => text.clone(),
                            ara_ai::UserContent::Blocks(_) => String::from("<blocks>"),
                        },
                        Message::Assistant(_) => assistant_text(message),
                        Message::ToolResult(_) => String::new(),
                        Message::Developer(_) => String::new(),
                    },
                )
            })
            .collect()
    };
    assert_eq!(texts(&before), texts(&after));
    assert_eq!(before.len(), after.len());
    assert_eq!(before.len(), 3);
}

#[test]
fn missing_first_kept_id_falls_back_to_build_context() {
    let (_dir, path, _) = seed_compacted();
    let mut text = fs::read_to_string(&path).unwrap();
    text = text.replace("firstKeptEntryId\":\"", "firstKeptEntryId\":\"missing-");
    fs::write(&path, text).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    let model = journal.model_context();
    let raw = journal.build_context();
    assert_eq!(model, raw, "invalid projection falls back to raw history");
    assert_eq!(raw.len(), 4, "user, assistant, user, assistant still present");
    assert!(raw.iter().any(|m| as_user_text(m) == Some("question one")));
}

#[test]
fn non_user_first_kept_falls_back_to_build_context() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_model_change("fake/m").unwrap();
    let q1 = journal.append_message(&user("question one")).unwrap();
    let a1 = journal.append_message(&assistant("answer one")).unwrap();
    // firstKeptEntryId is the assistant message, not a user message.
    journal.append_compaction("Earlier work completed.", &a1, std::slice::from_ref(&q1), 2).unwrap();
    let path = journal.path().to_path_buf();
    drop(journal);
    let journal = SessionJournal::open(&path).unwrap();
    let model = journal.model_context();
    let raw = journal.build_context();
    assert_eq!(model, raw, "non-user firstKeptEntryId is rejected and falls back");
    assert_eq!(raw.len(), 2);
}
