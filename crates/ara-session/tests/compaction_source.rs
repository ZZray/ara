use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
use ara_session::{CompactionSourceError, SessionJournal};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fake", "m");
    message.content.push(AssistantBlock::text(text));
    Message::Assistant(message)
}

fn write_journal(path: &Path, entries: &[Value]) {
    let mut lines = vec![json!({"type":"session","version":3,"id":"session-1","timestamp":"t","cwd":"/"})];
    lines.extend_from_slice(entries);
    let content = lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n";
    fs::write(path, content).unwrap();
}

fn entry(kind: &str, id: &str, parent: Value, extra: Value) -> Value {
    let mut raw = json!({"type":kind,"id":id,"parentId":parent});
    raw.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    raw
}

fn assert_rejected_unchanged(entries: &[Value], expected: CompactionSourceError) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(&path, entries);
    let before = fs::read(&path).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.compaction_source_snapshot(), Err(expected));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1, "read must not create a backup");
}

#[test]
fn returns_durable_branch_messages_with_original_ids_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    assert_eq!(journal.compaction_source_snapshot(), Err(CompactionSourceError::NotDurable));
    journal.append_model_change("fake/m").unwrap();
    let question = user("question");
    let answer = assistant("answer");
    let question_id = journal.append_message(&question).unwrap();
    let answer_id = journal.append_message(&answer).unwrap();
    let path = journal.path().to_path_buf();
    let session_id = journal.session_id().to_owned();
    drop(journal);

    let journal = SessionJournal::open(&path).unwrap();
    let snapshot = journal.compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.session_id, session_id);
    assert_eq!(snapshot.leaf_id, answer_id);
    assert_eq!(snapshot.messages.iter().map(|m| m.entry_id.as_str()).collect::<Vec<_>>(), vec![question_id, answer_id]);
    assert_eq!(snapshot.messages.iter().map(|m| &m.message).collect::<Vec<_>>(), vec![&question, &answer]);
}

#[test]
fn follows_only_current_branch_but_rejects_global_duplicate_ids() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(
        &path,
        &[
            entry("message", "root", Value::Null, json!({"message": user("root")})),
            entry("message", "aside", json!("root"), json!({"message": user("aside")})),
            entry("message", "leaf", json!("root"), json!({"message": assistant("leaf")})),
        ],
    );
    let snapshot = SessionJournal::open(&path).unwrap().compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.messages.iter().map(|m| m.entry_id.as_str()).collect::<Vec<_>>(), vec!["root", "leaf"]);

    assert_rejected_unchanged(
        &[
            entry("message", "root", Value::Null, json!({"message": user("root")})),
            entry("message", "root", Value::Null, json!({"message": user("replacement")})),
            entry("message", "leaf", json!("root"), json!({"message": assistant("leaf")})),
        ],
        CompactionSourceError::DuplicateEntryId { id: "root".into() },
    );
}

#[test]
fn rejects_missing_forward_and_cycle_parents() {
    assert_rejected_unchanged(
        &[entry("message", "leaf", json!("missing"), json!({"message": user("x")}))],
        CompactionSourceError::MissingEntry { id: "missing".into() },
    );
    assert_rejected_unchanged(
        &[
            entry("message", "child", json!("later"), json!({"message": user("x")})),
            entry("message", "later", Value::Null, json!({"message": user("y")})),
            entry("message", "leaf", json!("child"), json!({"message": user("z")})),
        ],
        CompactionSourceError::ParentNotEarlier { child_id: "child".into(), parent_id: "later".into() },
    );
    assert_rejected_unchanged(
        &[entry("message", "self", json!("self"), json!({"message": user("x")}))],
        CompactionSourceError::ParentCycle { id: "self".into() },
    );
}

#[test]
fn rejects_invalid_raw_parent_ids_and_empty_ids() {
    let base = entry("message", "leaf", Value::Null, json!({"message": user("x")}));
    for invalid in [json!(4), json!({}), json!("")] {
        let mut changed = base.clone();
        changed["parentId"] = invalid;
        assert_rejected_unchanged(&[changed], CompactionSourceError::InvalidParentId { id: "leaf".into() });
    }
    let mut absent = base.clone();
    absent.as_object_mut().unwrap().remove("parentId");
    assert_rejected_unchanged(&[absent], CompactionSourceError::InvalidParentId { id: "leaf".into() });
    assert_rejected_unchanged(
        &[entry("message", "", Value::Null, json!({"message": user("x")}))],
        CompactionSourceError::EmptyEntryId,
    );
}

#[test]
fn rejects_undecodable_and_unported_context_entries() {
    assert_rejected_unchanged(
        &[entry("message", "bad", Value::Null, json!({"message":{"role":"user"}}))],
        CompactionSourceError::UndecodableMessage { id: "bad".into() },
    );
    for kind in ["compaction", "reset_boundary", "custom_message", "branch_summary", "future_kind"] {
        assert_rejected_unchanged(
            &[entry(kind, "special", Value::Null, json!({}))],
            CompactionSourceError::UnsupportedContextEntry { id: "special".into(), kind: kind.into() },
        );
    }
}

#[test]
fn rejects_unrepaired_journal_without_changing_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(&path, &[entry("message", "root", Value::Null, json!({"message": user("root")}))]);
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{broken").unwrap();
    let before = fs::read(&path).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.compaction_source_snapshot(), Err(CompactionSourceError::UnrepairedJournal));
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn rejects_lossy_utf8_source_even_when_json_still_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(&path, &[entry("message", "root", Value::Null, json!({"message": user("badx")}))]);
    let mut bytes = fs::read(&path).unwrap();
    let marker = bytes.windows(4).position(|window| window == b"badx").unwrap();
    bytes[marker + 3] = 0xff;
    fs::write(&path, &bytes).unwrap();

    let journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.report.malformed_records, 0);
    assert_eq!(journal.build_context().len(), 1, "ordinary tolerant read is unchanged");
    assert_eq!(journal.compaction_source_snapshot(), Err(CompactionSourceError::InvalidUtf8));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[test]
fn successful_rewrite_clears_current_damage_even_if_load_report_keeps_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    write_journal(&path, &[entry("message", "root", Value::Null, json!({"message": assistant("root")}))]);
    fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(b"{broken\n").unwrap();
    let mut journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.compaction_source_snapshot(), Err(CompactionSourceError::UnrepairedJournal));
    let next_id = journal.append_message(&user("next")).unwrap();
    assert_eq!(journal.report.malformed_records, 1);
    assert_eq!(journal.compaction_source_snapshot().unwrap().leaf_id, next_id);
}
