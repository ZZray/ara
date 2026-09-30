//! Fixed OMP SessionManager name cleaning, receipts and lazy title persistence.

use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
use ara_session::{CompactedContextItem, SESSION_TITLE_SLOT_BYTES, SessionJournal, normalize_session_name};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

fn assistant() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::text("Materialized title fixture."));
    Message::Assistant(message)
}

fn lines(path: &Path) -> Vec<Value> {
    fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

#[test]
fn cleaning_uses_ecmascript_trim_controls_and_only_ascii_space_collapse() {
    assert_eq!(
        normalize_session_name("\u{feff}\u{3000}alpha\0\t  beta\u{0085}\u{009f}🦀\n γ\u{00a0}"),
        "alpha beta 🦀 γ"
    );
    assert_eq!(normalize_session_name("\u{feff}A\u{2003}\u{2003}B\u{feff}"), "A\u{2003}\u{2003}B");
    assert_eq!(normalize_session_name("A\u{feff}B"), "A\u{feff}B");
    assert!(normalize_session_name("\u{feff}\0\n\u{007f}\u{0085}\u{009f}\u{2028}\u{2029}").is_empty());
}

#[test]
fn lazy_rename_records_title_change_header_and_source_without_creating_file() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let model_id = journal.append_model_change("fixture/model").unwrap();
    assert!(journal.set_session_name("  Generated\tname  ", "auto").unwrap());
    assert!(!journal.path().exists());
    assert!(!journal.is_on_disk());
    assert_eq!(journal.title().title, "Generated name");
    assert_eq!(journal.title().source.as_deref(), Some("auto"));
    assert_eq!(journal.header()["title"], "Generated name");
    assert_eq!(journal.header()["titleSource"], "auto");
    let first = journal.entries().last().unwrap().clone();
    assert_eq!(first.kind, "title_change");
    assert_eq!(first.parent_id.as_deref(), Some(model_id.as_str()));
    assert_eq!(first.raw["title"], "Generated name");
    assert_eq!(first.raw["source"], "auto");
    assert_eq!(first.raw["timestamp"], journal.title().updated_at);
    assert!(first.raw.get("previousTitle").is_none());
    assert!(journal.set_session_name("\u{feff} User\u{009f}chosen \u{feff}", "user").unwrap());
    let second = journal.entries().last().unwrap();
    assert_eq!(second.parent_id.as_deref(), Some(first.id.as_str()));
    assert_eq!(second.raw["previousTitle"], "Generated name");
    assert_eq!(second.raw["title"], "User chosen");
    assert_eq!(second.raw["source"], "user");
    assert!(!journal.path().exists(), "user naming also keeps a draft lazy");
    journal.append_message(&assistant()).unwrap();
    let raw = lines(journal.path());
    assert_eq!(raw.iter().filter(|entry| entry["type"] == "title_change").count(), 2);
    assert_eq!(raw[0]["title"], "User chosen");
    assert_eq!(raw[0]["source"], "user");
    assert_eq!(raw[1]["title"], "User chosen");
    assert_eq!(raw[1]["titleSource"], "user");
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.title(), journal.title());
    assert_eq!(reopened.header(), journal.header());
    assert_eq!(reopened.model_context(), vec![assistant_from(&raw)]);
}

fn assistant_from(entries: &[Value]) -> Message {
    serde_json::from_value(
        entries.iter().find(|entry| entry["message"]["role"] == "assistant").unwrap()["message"].clone(),
    )
    .unwrap()
}

#[test]
fn slotless_v3_header_restores_name_source_and_previous_title() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.jsonl");
    let header = json!({
        "type": "session",
        "version": 3,
        "id": "legacy-session",
        "title": "Legacy title",
        "titleSource": "user",
        "timestamp": "2026-01-01T00:00:00.000Z",
        "cwd": directory.path(),
    });
    let original = format!("{header}\n");
    fs::write(&path, &original).unwrap();

    let mut journal = SessionJournal::open(&path).unwrap();
    assert_eq!(journal.title().title, "Legacy title");
    assert_eq!(journal.title().source.as_deref(), Some("user"));
    assert_eq!(journal.title().updated_at, "2026-01-01T00:00:00.000Z");
    assert_eq!(journal.header(), &header);
    assert!(!journal.set_session_name("Must not replace a user name", "auto").unwrap());
    assert!(journal.entries().is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);

    assert!(journal.set_session_name("Renamed legacy title", "user").unwrap());
    assert_eq!(journal.entries().last().unwrap().raw["previousTitle"], "Legacy title");
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.title().title, "Renamed legacy title");
    assert_eq!(reopened.title().source.as_deref(), Some("user"));
    assert_eq!(reopened.entries().last().unwrap().raw["previousTitle"], "Legacy title");
}

#[test]
fn materialized_rename_commits_slot_header_and_one_receipt_before_returning() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.append_message(&assistant()).unwrap();
    let before = lines(journal.path());
    let before_leaf = journal.leaf_id().unwrap().to_owned();
    assert!(journal.set_session_name("Release 🦀 name", "user").unwrap());
    let bytes = fs::read(journal.path()).unwrap();
    assert_eq!(bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1, SESSION_TITLE_SLOT_BYTES);
    let raw = lines(journal.path());
    assert_eq!(raw.len(), before.len() + 1);
    assert_eq!(&raw[2..raw.len() - 1], &before[2..], "history receipts remain exact");
    assert_eq!(raw[0]["title"], "Release 🦀 name");
    assert_eq!(raw[0]["source"], "user");
    assert_eq!(raw[1]["title"], "Release 🦀 name");
    assert_eq!(raw[1]["titleSource"], "user");
    let change = raw.last().unwrap();
    assert_eq!(change["type"], "title_change");
    assert_eq!(change["parentId"], before_leaf);
    assert_eq!(change["title"], "Release 🦀 name");
    assert_eq!(change["source"], "user");
    assert_eq!(change["timestamp"], raw[0]["updatedAt"]);
    assert!(change.get("previousTitle").is_none());
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.title(), journal.title());
    assert_eq!(reopened.header(), journal.header());
    assert_eq!(reopened.leaf_id(), journal.leaf_id());
}

#[test]
fn empty_and_auto_after_user_leave_public_and_durable_state_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.materialize().unwrap();
    assert!(journal.set_session_name("Explicit name", "user").unwrap());
    let before_title = journal.title().clone();
    let before_header = journal.header().clone();
    let before_entries: Vec<_> = journal.entries().iter().map(|entry| entry.raw.clone()).collect();
    let before_leaf = journal.leaf_id().unwrap().to_owned();
    let before_file = fs::read(journal.path()).unwrap();
    for (name, source) in [("\u{feff}\n\u{009f}\u{3000}", "user"), ("Automatic must not replace explicit", "auto")] {
        assert!(!journal.set_session_name(name, source).unwrap());
        assert_eq!(journal.title(), &before_title);
        assert_eq!(journal.header(), &before_header);
        assert_eq!(journal.entries().iter().map(|entry| entry.raw.clone()).collect::<Vec<_>>(), before_entries);
        assert_eq!(journal.leaf_id(), Some(before_leaf.as_str()));
        assert_eq!(fs::read(journal.path()).unwrap(), before_file);
    }
    // Fixed OMP records a second explicit rename even if its cleaned text is
    // equal; this is a user action receipt, not a text-difference notification.
    assert!(journal.set_session_name(" Explicit name ", "user").unwrap());
    assert_eq!(journal.entries().len(), before_entries.len() + 1);
    assert_eq!(journal.entries().last().unwrap().raw["previousTitle"], "Explicit name");
}

#[test]
fn rewrite_failure_keeps_previous_title_header_leaf_and_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.materialize().unwrap();
    journal.set_session_name("Durable original", "user").unwrap();
    let before_title = journal.title().clone();
    let before_header = journal.header().clone();
    let before_entries: Vec<_> = journal.entries().iter().map(|entry| entry.raw.clone()).collect();
    let before_leaf = journal.leaf_id().unwrap().to_owned();
    let before_file = fs::read(journal.path()).unwrap();
    fs::remove_file(journal.path()).unwrap();
    fs::create_dir(journal.path()).unwrap();
    assert!(journal.set_session_name("Must not publish failed name", "user").is_err());
    assert_eq!(journal.title(), &before_title);
    assert_eq!(journal.header(), &before_header);
    assert_eq!(journal.entries().iter().map(|entry| entry.raw.clone()).collect::<Vec<_>>(), before_entries);
    assert_eq!(journal.leaf_id(), Some(before_leaf.as_str()));
    assert!(journal.path().is_dir());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1, "failed rewrite removes its temporary file");
    fs::remove_dir(journal.path()).unwrap();
    fs::write(journal.path(), before_file).unwrap();
    assert!(journal.set_session_name("Recovered name", "user").unwrap());
    let raw = lines(journal.path());
    assert_eq!(raw.last().unwrap()["previousTitle"], "Durable original");
    assert!(!serde_json::to_string(&raw).unwrap().contains("Must not publish failed name"));
    assert_eq!(SessionJournal::open(journal.path()).unwrap().title().title, "Recovered name");
}

#[test]
fn multibyte_name_keeps_full_receipt_and_valid_fixed_width_slot() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.materialize().unwrap();
    let title = "会议 🦀 ".repeat(80);
    let title = title.trim_end();
    assert!(journal.set_session_name(title, "user").unwrap());
    assert_eq!(journal.title().title, title);
    let bytes = fs::read(journal.path()).unwrap();
    let first_line_end = bytes.iter().position(|byte| *byte == b'\n').unwrap() + 1;
    assert_eq!(first_line_end, SESSION_TITLE_SLOT_BYTES);
    std::str::from_utf8(&bytes[..first_line_end]).expect("slot truncation must not split UTF-8");
    let raw = lines(journal.path());
    let slot_title = raw[0]["title"].as_str().unwrap();
    assert!(!slot_title.is_empty() && slot_title.len() < title.len());
    assert!(title.starts_with(slot_title));
    assert_eq!(raw[1]["title"], title);
    assert_eq!(raw.last().unwrap()["title"], title);
    // The fixed title slot is intentionally abbreviated. Reopened display
    // uses the slot while the native header/change receipt retain full text.
    assert_eq!(SessionJournal::open(journal.path()).unwrap().title().title, slot_title);
}

#[test]
fn rename_receipts_after_reopen_do_not_block_compaction_or_enter_model_context() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = journal.append_message(&Message::User(UserMessage::text("Earlier request"))).unwrap();
    let first_answer = journal.append_message(&assistant()).unwrap();
    journal.set_session_name("Name between turns", "user").unwrap();
    let kept = journal.append_message(&Message::User(UserMessage::text("Keep this request"))).unwrap();
    let kept_answer = journal.append_message(&assistant()).unwrap();
    journal.set_session_name("Name after last answer", "user").unwrap();
    let path = journal.path().to_path_buf();
    let title_leaf = journal.leaf_id().unwrap().to_owned();
    drop(journal);

    let mut reopened = SessionJournal::open(&path).unwrap();
    let snapshot = reopened.compaction_source_snapshot().unwrap();
    assert_eq!(snapshot.leaf_id, title_leaf, "the actual metadata leaf still guards the compaction source snapshot");
    assert_eq!(
        snapshot.messages.iter().map(|message| message.entry_id.as_str()).collect::<Vec<_>>(),
        [first.as_str(), first_answer.as_str(), kept.as_str(), kept_answer.as_str()]
    );
    let projection = reopened.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 4);
    assert!(projection.items.iter().all(|item| matches!(item, CompactedContextItem::Message(_))));

    let sources = [first.clone(), first_answer.clone()];
    let compaction = reopened.append_compaction("Earlier request completed.", &kept, &sources, 100).unwrap();
    reopened.set_session_name("Name after compaction", "user").unwrap();
    drop(reopened);
    let reopened = SessionJournal::open(&path).unwrap();
    let projection = reopened.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 3);
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("summary must remain first") };
    assert_eq!(summary.entry_id, compaction);
    assert_eq!(summary.first_kept_entry_id, kept);
    assert_eq!(summary.source_entry_ids.as_ref().unwrap(), &sources);
    let kept_ids: Vec<_> = projection.items[1..]
        .iter()
        .map(|item| match item {
            CompactedContextItem::Message(message) => message.entry_id.as_str(),
            CompactedContextItem::Summary(_) => panic!("only one summary"),
        })
        .collect();
    assert_eq!(kept_ids, [kept.as_str(), kept_answer.as_str()]);
    assert_eq!(reopened.model_context().len(), 3);
    assert!(!serde_json::to_string(&reopened.model_context()).unwrap().contains("Name after"));
    assert_eq!(
        reopened.entries().iter().filter(|entry| entry.kind == "title_change").count(),
        3,
        "audit receipts remain native history"
    );
    assert_eq!(reopened.title().title, "Name after compaction");
}
