//! Fixed OMP native fork paths, effective labels and in-memory ownership.

use ara_ai::{AssistantBlock, AssistantMessage, ImageContent, Message, UserBlock, UserContent, UserMessage};
use ara_session::{CompactedContextItem, CompactionSourceError, SessionJournal, UserSkillPrompt};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn header(cwd: &Path) -> Value {
    json!({"type":"session","version":3,"id":"host-owned-id","timestamp":"2026-01-01T00:00:00.000Z","cwd":cwd})
}

fn assistant() -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::text("Native fork answer."));
    Message::Assistant(message)
}

fn entry(id: &str, parent: Value, kind: &str, fields: Value) -> Value {
    let mut value = json!({"id":id,"parentId":parent,"type":kind,"timestamp":"2026-01-01T00:00:00.000Z"});
    value.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    value
}

fn open_fixture(path: &Path, header: Value, entries: Vec<Value>) -> SessionJournal {
    let mut values = vec![header];
    values.extend(entries);
    fs::write(path, values.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    SessionJournal::open(path).unwrap()
}

fn raw_entries(journal: &SessionJournal) -> Vec<Value> {
    journal.entries().iter().map(|entry| entry.raw.clone()).collect()
}

#[test]
fn in_memory_keeps_host_identity_native_ids_empty_users_images_and_skills_without_io() {
    let directory = tempfile::tempdir().unwrap();
    let supplied = header(&directory.path().join("no-directory-must-be-created"));
    let mut journal = SessionJournal::in_memory(supplied.clone()).unwrap();
    assert_eq!(journal.header(), &supplied);
    assert_eq!(journal.session_id(), "host-owned-id");
    assert!(!journal.is_persistent());
    assert!(journal.path().as_os_str().is_empty());
    let empty = journal.append_message(&Message::User(UserMessage::text(""))).unwrap();
    let image = Message::User(UserMessage {
        content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
            data: "aW1hZ2U=".into(),
            mime_type: "image/png".into(),
        })]),
        synthetic: None,
        timestamp: 1,
    });
    let image_id = journal.append_message(&image).unwrap();
    let skill = UserSkillPrompt::new(UserContent::Text("Owned Skill body".into()), Some(json!({"name":"fixture"})));
    let skill_id = journal.append_skill_prompt(&skill).unwrap();
    journal.append_message(&assistant()).unwrap();
    journal.set_session_name("Memory title", "user").unwrap();
    journal.materialize().unwrap();
    assert!(!journal.is_on_disk());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    assert_eq!(journal.entries()[0].id, empty);
    assert_eq!(journal.entries()[1].id, image_id);
    assert_eq!(journal.entries()[2].id, skill_id);
    assert_eq!(journal.entries()[1].parent_id.as_deref(), Some(empty.as_str()));
    assert_eq!(journal.entries()[2].parent_id.as_deref(), Some(image_id.as_str()));
    assert!(journal.entries().iter().all(|entry| entry.id.len() == 8));
    assert_eq!(journal.entries()[1].message(), Some(image));
    assert_eq!(journal.entries()[2].skill_prompt(), Some(skill));
    assert_eq!(journal.build_context().len(), 4);
    assert_eq!(journal.compaction_source_snapshot(), Err(CompactionSourceError::NotDurable));
}

#[test]
fn in_memory_rejects_invalid_headers_without_creating_a_path() {
    for supplied in
        [json!({}), json!({"type":"session","version":3,"id":""}), json!({"type":"session","version":2,"id":"id"})]
    {
        assert!(SessionJournal::in_memory(supplied).is_err());
    }
}

#[test]
fn persistent_fork_keeps_native_custom_and_compaction_receipts_without_mutating_source() {
    let directory = tempfile::tempdir().unwrap();
    let mut source = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = source.append_message(&Message::User(UserMessage::text("Earlier request"))).unwrap();
    let answer = source.append_message(&assistant()).unwrap();
    let kept = source.append_message(&Message::User(UserMessage::text("Keep request"))).unwrap();
    source.append_message(&assistant()).unwrap();
    let summary = source.append_compaction("Earlier work completed", &kept, &[first, answer], 100).unwrap();
    let skill = UserSkillPrompt::new(UserContent::Text("Original Skill body".into()), Some(json!({"source":"codex"})));
    let leaf = source.append_skill_prompt(&skill).unwrap();
    let retained = raw_entries(&source);
    source.append_message(&Message::User(UserMessage::text("Selected user excluded"))).unwrap();
    source.set_session_name("Current title", "user").unwrap();
    let source_bytes = fs::read(source.path()).unwrap();
    let source_id = source.session_id().to_owned();
    let source_leaf = source.leaf_id().unwrap().to_owned();

    let target_directory = directory.path().join("forks");
    let mut fork = source.fork_at(Some(&leaf), Some(&target_directory)).unwrap();
    assert!(fork.is_persistent() && fork.is_on_disk());
    assert_ne!(fork.session_id(), source_id);
    assert_ne!(fork.path(), source.path());
    assert_eq!(fork.header()["parentSession"], json!(source.path()));
    assert_eq!(fork.title().title, "Current title");
    assert_eq!(fork.title().source.as_deref(), Some("user"));
    assert_eq!(raw_entries(&fork), retained);
    assert_eq!(fork.leaf_id(), Some(leaf.as_str()));
    assert_eq!(fork.entries().last().unwrap().skill_prompt(), Some(skill));
    let projection = fork.compacted_context_projection().unwrap();
    assert!(matches!(&projection.items[0], CompactedContextItem::Summary(view) if view.entry_id == summary));
    assert_eq!(projection.items.len(), 4);
    let follow_up = fork.append_message(&Message::User(UserMessage::text("New fork request"))).unwrap();
    assert_eq!(fork.entries().last().unwrap().parent_id.as_deref(), Some(leaf.as_str()));
    assert_eq!(SessionJournal::open(fork.path()).unwrap().leaf_id(), Some(follow_up.as_str()));
    assert_eq!(fs::read(source.path()).unwrap(), source_bytes);
    assert_eq!(source.session_id(), source_id);
    assert_eq!(source.leaf_id(), Some(source_leaf.as_str()));
}

#[test]
fn fork_resolves_off_path_labels_removals_and_map_insertion_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("labels.jsonl");
    let mut supplied = header(directory.path());
    supplied["additionalDirectories"] = json!(["extra-workspace"]);
    supplied["title"] = json!("Named branch");
    supplied["titleSource"] = json!("user");
    let user =
        |id: &str, parent: Value| entry(id, parent, "message", json!({"message":Message::User(UserMessage::text(id))}));
    let label = |id: &str, target: &str, value: Value| {
        entry(id, json!("u4"), "label", json!({"targetId":target,"label":value}))
    };
    let source = open_fixture(
        &path,
        supplied,
        vec![
            user("u1", Value::Null),
            user("u2", json!("u1")),
            user("u3", json!("u2")),
            user("u4", json!("u3")),
            label("l1", "u2", json!("second")),
            label("l2", "u1", json!("first")),
            label("l3", "u3", json!("third")),
            label("l4", "u2", Value::Null),
            label("l5", "u2", json!("second reinserted")),
            label("l6", "u1", json!("first updated")),
            label("l7", "u4", json!("outside retained path")),
            label("l8", "u3", json!("")),
            entry("l9", json!("u4"), "label", json!({"targetId":"u3"})),
        ],
    );
    let before = fs::read(&path).unwrap();
    let fork = source.fork_at(Some("u3"), None).unwrap();
    assert!(!fork.is_persistent() && !fork.is_on_disk());
    assert!(fork.header().get("parentSession").is_none());
    assert_eq!(fork.header()["additionalDirectories"], json!(["extra-workspace"]));
    assert_eq!(fork.title().title, "Named branch");
    let labels: Vec<_> = fork.entries().iter().filter(|entry| entry.kind == "label").collect();
    assert_eq!(labels.len(), 2);
    assert_eq!(labels[0].raw["targetId"], "u1");
    assert_eq!(labels[0].raw["label"], "first updated");
    assert_eq!(labels[1].raw["targetId"], "u2");
    assert_eq!(labels[1].raw["label"], "second reinserted");
    assert_eq!(labels[0].parent_id.as_deref(), Some("u3"));
    assert_eq!(labels[1].parent_id.as_deref(), Some(labels[0].id.as_str()));
    assert!(labels.iter().all(|entry| !source.entries().iter().any(|old| old.id == entry.id)));
    assert_eq!(fork.build_context().len(), 3);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn root_fork_is_empty_except_inherited_name_action_and_has_no_previous_title() {
    let directory = tempfile::tempdir().unwrap();
    let mut supplied = header(directory.path());
    supplied["additionalDirectories"] = json!(["extra-workspace"]);
    let mut source = SessionJournal::in_memory(supplied).unwrap();
    source.append_message(&Message::User(UserMessage::text("Old root"))).unwrap();
    source.set_session_name("Root fork title", "user").unwrap();
    let before = raw_entries(&source);
    let fork = source.fork_at(None, None).unwrap();
    assert_ne!(fork.session_id(), source.session_id());
    assert!(fork.build_context().is_empty());
    assert_eq!(fork.entries().len(), 1);
    let title = &fork.entries()[0];
    assert_eq!(title.kind, "title_change");
    assert_eq!(title.parent_id, None);
    assert_eq!(title.raw["title"], "Root fork title");
    assert_eq!(title.raw["source"], "user");
    assert!(title.raw.get("previousTitle").is_none());
    assert!(fork.header().get("additionalDirectories").is_none(), "fixed root newSession resets workspace options");
    assert!(fork.header().get("parentSession").is_none());
    assert_eq!(raw_entries(&source), before);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    let unnamed = SessionJournal::in_memory(header(directory.path())).unwrap();
    assert!(unnamed.fork_at(None, None).unwrap().entries().is_empty());
    let disk_root = unnamed.fork_at(None, Some(directory.path())).unwrap();
    assert!(disk_root.is_on_disk());
    assert!(disk_root.header().get("parentSession").is_none(), "a memory source has no phantom parent file");
}

#[test]
fn unknown_leaf_fails_before_io_and_tolerant_path_walk_keeps_last_duplicate_missing_parents_and_cycles() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tolerant.jsonl");
    let source = open_fixture(
        &path,
        header(directory.path()),
        vec![
            entry("duplicate", Value::Null, "custom", json!({"value":"old"})),
            entry("duplicate", json!("missing"), "custom", json!({"value":"last"})),
            entry("cycle-a", json!("cycle-b"), "custom", json!({})),
            entry("cycle-b", json!("cycle-a"), "custom", json!({})),
        ],
    );
    let before = fs::read(&path).unwrap();
    let untouched = directory.path().join("not-created");
    assert!(
        source
            .fork_at(Some("unknown"), Some(&untouched))
            .err()
            .unwrap()
            .to_string()
            .contains("Entry unknown not found")
    );
    assert!(!untouched.exists());
    let duplicate = source.fork_at(Some("duplicate"), None).unwrap();
    assert_eq!(duplicate.entries().len(), 1);
    assert_eq!(duplicate.entries()[0].raw["value"], "last");
    assert_eq!(duplicate.entries()[0].parent_id.as_deref(), Some("missing"));
    let cycle = source.fork_at(Some("cycle-a"), None).unwrap();
    assert_eq!(cycle.entries().iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), ["cycle-b", "cycle-a"]);
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn failed_target_persistence_keeps_source_identity_history_leaf_and_file_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut source = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let user = source.append_message(&Message::User(UserMessage::text("Keep source"))).unwrap();
    source.append_message(&assistant()).unwrap();
    let original = fs::read(source.path()).unwrap();
    let original_header = source.header().clone();
    let original_entries = raw_entries(&source);
    let original_leaf = source.leaf_id().unwrap().to_owned();
    let blocked_directory = directory.path().join("occupied-by-file");
    fs::write(&blocked_directory, "sentinel").unwrap();
    assert!(source.fork_at(Some(&user), Some(&blocked_directory)).is_err());
    assert_eq!(source.header(), &original_header);
    assert_eq!(raw_entries(&source), original_entries);
    assert_eq!(source.leaf_id(), Some(original_leaf.as_str()));
    assert_eq!(fs::read(source.path()).unwrap(), original);
    assert_eq!(fs::read_to_string(blocked_directory).unwrap(), "sentinel");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2, "failed fork creates no temporary journal");
}

#[test]
fn memory_fork_projects_compaction_without_claiming_a_durable_source_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let mut source = SessionJournal::in_memory(header(directory.path())).unwrap();
    let first = source.append_message(&Message::User(UserMessage::text("Summarize request"))).unwrap();
    let answer = source.append_message(&assistant()).unwrap();
    let kept = source.append_message(&Message::User(UserMessage::text("Keep request"))).unwrap();
    source.append_message(&assistant()).unwrap();
    let summary = source.append_compaction("Earlier work completed", &kept, &[first, answer], 100).unwrap();
    let fork = source.fork_at(Some(&summary), None).unwrap();
    let projection = fork.compacted_context_projection().unwrap();
    assert_eq!(projection.items.len(), 3);
    assert!(matches!(&projection.items[0], CompactedContextItem::Summary(view) if view.entry_id == summary));
    assert_eq!(fork.model_context().len(), 3);
    assert_eq!(fork.compaction_source_snapshot(), Err(CompactionSourceError::NotDurable));
    assert!(!fork.is_on_disk());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}
