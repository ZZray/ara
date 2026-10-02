//! Grouped fixed-OMP archive families: native commit/reopen, legacy rescue,
//! image storage/healing and checked failure publication (596f2da, MIT).
use ara_ai::{AssistantBlock, AssistantMessage, Message, UserBlock, UserContent, UserMessage};
use ara_session::{
    CompactedContextItem, CompactionCommitError, NativeSnapcompactError, NativeSnapcompactSummary, SessionJournal,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::fs;

fn turn(journal: &mut SessionJournal, text: &str) -> [String; 2] {
    let mut answer = AssistantMessage::empty("openai-completions", "fixture", "m");
    answer.content.push(AssistantBlock::text(text));
    [
        journal.append_message(&Message::User(UserMessage::text(text))).unwrap(),
        journal.append_message(&Message::Assistant(answer)).unwrap(),
    ]
}

fn prepared(data: &str) -> NativeSnapcompactSummary {
    NativeSnapcompactSummary {
        summary: "History kept as archive.".into(),
        short_summary: Some(String::new()),
        preserve_data: Some(json!({"snapcompact":{
            "frames":[{"data":data,"mimeType":"image/png","cols":80,"rows":20,"chars":1600,"detail":"original"}],
            "totalChars":1620,"truncatedChars":0,"text":"HEAD ¶u: full imaged middle TAIL",
            "textHead":"HEAD","textTail":"TAIL"}})),
        read_files: vec!["source.rs".into()],
        modified_files: vec!["result.json".into()],
    }
}

fn summary(journal: &SessionJournal) -> ara_session::CompactionSummaryView {
    let projection = journal.compacted_context_projection().unwrap();
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("archive first") };
    summary.as_ref().clone()
}

fn write_entries(path: &std::path::Path, entries: &[Value]) {
    let header = json!({"type":"session","version":3,"id":"imported","cwd":"/","timestamp":"2026-10-02T00:00:00.000Z"});
    let mut body = format!("{header}\n");
    for entry in entries {
        body.push_str(&format!("{entry}\n"));
    }
    fs::write(path, body).unwrap();
}

fn fixture_entries(preserve: Value) -> Vec<Value> {
    let user = |text| serde_json::to_value(Message::User(UserMessage::text(text))).unwrap();
    let mut answer = AssistantMessage::empty("openai-completions", "fixture", "m");
    answer.content.push(AssistantBlock::text("answer"));
    let answer = serde_json::to_value(Message::Assistant(answer)).unwrap();
    let raw = |id: &str, parent: Value, message: Value| {
        json!({"type":"message","id":id,"parentId":parent,
        "timestamp":"2026-10-02T00:00:00.000Z","message":message})
    };
    vec![
        raw("q1", Value::Null, user("old question")),
        raw("a1", json!("q1"), answer.clone()),
        raw("q2", json!("a1"), user("retained question")),
        raw("a2", json!("q2"), answer),
        json!({"type":"compaction","id":"c1","parentId":"a2","timestamp":"2026-10-02T00:00:00.000Z",
            "method":"snapcompact","summary":"Imported archive","firstKeptEntryId":"q2","tokensBefore":200,
            "preserveData":preserve}),
    ]
}

#[test]
fn native_archive_reopen_rescue_and_fork_keep_images_sources_and_text_migration() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let mut journal = SessionJournal::create_with_blob_directory(dir.path(), dir.path(), &blobs).unwrap();
    let first_user = journal
        .append_message(&Message::User(UserMessage {
            content: UserContent::Blocks(vec![
                UserBlock::text("first port 4242"),
                UserBlock::Image(ara_ai::ImageContent {
                    data: "AQI=".into(),
                    mime_type: "image/png".into(),
                    detail: None,
                    compaction_frame: false,
                }),
            ]),
            synthetic: None,
            timestamp: 0,
        }))
        .unwrap();
    let mut first_answer = AssistantMessage::empty("openai-completions", "fixture", "m");
    first_answer.content.push(AssistantBlock::text("first answer"));
    let first = [first_user, journal.append_message(&Message::Assistant(first_answer)).unwrap()];
    let second = turn(&mut journal, "second retained");
    let before = journal.entries().to_vec();
    // Larger than OMP's old generic string cap: never truncate frame base64.
    let bytes = vec![42u8; 400_000];
    let data = STANDARD.encode(&bytes);
    let prepared = prepared(&data);
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let id = journal.commit_native_entry_snapcompact(&snapshot, &prepared, &second[0], &first, 200).unwrap();
    assert_eq!(&journal.entries()[..before.len()], before);
    let disk = fs::read_to_string(journal.path()).unwrap();
    let persisted: Value = serde_json::from_str(disk.lines().last().unwrap()).unwrap();
    let reference = persisted["preserveData"]["snapcompact"]["frames"][0]["data"].as_str().unwrap();
    let hash = ara_session::blob::parse_blob_ref(reference).unwrap();
    assert_eq!(fs::read(blobs.join(hash)).unwrap(), bytes);
    assert!(blobs.join(format!("{hash}.png")).exists());
    assert_eq!(journal.entries().last().unwrap().raw["preserveData"], prepared.preserve_data.clone().unwrap());
    let mut reopened = SessionJournal::open_with_blob_directory(journal.path(), &blobs).unwrap();
    assert_eq!(reopened.entries(), journal.entries());
    assert!(reopened.report.blob_warnings.is_empty());
    let view = summary(&reopened);
    assert_eq!(view.entry_id, id);
    assert_eq!(view.source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(view.short_summary.as_deref(), Some(""));
    assert!(view.previous_summary_for_text_compaction().contains("Previous snapcompact archive source text:\n\n"));
    assert!(view.archive_migration_text().unwrap().contains("full imaged middle"));
    assert_eq!(view.preserve_data_without_archive(), None);
    let context = reopened.model_context();
    let Message::User(archive) = &context[0] else { panic!("archive projection") };
    let UserContent::Blocks(blocks) = &archive.content else { panic!("archive blocks") };
    let image = blocks
        .iter()
        .find_map(|block| match block {
            UserBlock::Image(image) => Some(image),
            _ => None,
        })
        .unwrap();
    assert_eq!(image.data, data);
    assert_eq!(image.detail, Some(json!("original")));
    assert!(image.compaction_frame);
    assert_eq!(reopened.model_context(), context, "derived timestamp is stable");
    let rescue = reopened.native_snapcompact_snapshot().unwrap();
    let rescued_id = reopened.commit_native_snapcompact_rescue(&rescue, &prepared, 100).unwrap();
    reopened.stamp_native_snapcompact_warning(&rescued_id, "No headroom").unwrap();
    let reopened = SessionJournal::open_with_blob_directory(reopened.path(), &blobs).unwrap();
    assert_eq!(summary(&reopened).source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(summary(&reopened).first_kept_entry_id, second[0]);
    assert_eq!(reopened.entries().last().unwrap().raw["archiveSourceEntryId"], id);
    assert_eq!(reopened.entries().last().unwrap().raw["warning"], "No headroom");
    let rescued_context = reopened.model_context();
    let fork_dir = dir.path().join("fork");
    let fork = reopened.fork_at(reopened.leaf_id(), Some(&fork_dir)).unwrap();
    assert_eq!(fork.blob_directory(), Some(blobs.as_path()));
    assert_eq!(SessionJournal::open_with_blob_directory(fork.path(), &blobs).unwrap().model_context(), rescued_context);
}

#[test]
fn legacy_archive_without_ara_sources_is_readable_and_rescue_keeps_unknown_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("import.jsonl");
    let prepared = prepared("AQI=");
    write_entries(&path, &fixture_entries(prepared.preserve_data.clone().unwrap()));
    let mut journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
    assert_eq!(summary(&journal).source_entry_ids, None);
    let original = journal.entries().to_vec();
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    journal.commit_native_snapcompact_rescue(&snapshot, &prepared, 100).unwrap();
    assert_eq!(&journal.entries()[..original.len()], original);
    let raw = &journal.entries().last().unwrap().raw;
    assert!(raw.get("sourceEntryIds").is_none(), "never manufacture a verified empty/source list");
    assert_eq!(raw["archiveSourceEntryId"], "c1");
    let reopened = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
    assert_eq!(summary(&reopened).source_entry_ids, None);
    // A known exact new window does not establish the imported archive's facts.
    let snapshot = reopened.native_snapcompact_snapshot().unwrap();
    let mut reopened = reopened;
    assert!(matches!(
        reopened.commit_native_entry_snapcompact(&snapshot, &prepared, "a2", &["q2".into()], 100),
        Err(NativeSnapcompactError::Validation(CompactionCommitError::MissingPreviousSources))
    ));
}

#[test]
fn legacy_broken_frames_heal_to_retained_text_and_missing_refs_remain_observable() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("legacy.jsonl");
    let mut result = prepared("AQI=\n\n[Session persistence truncated large content]");
    write_entries(&path, &fixture_entries(result.preserve_data.clone().unwrap()));
    let journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
    assert!(summary(&journal).archive.unwrap().frames.is_empty());
    let context = journal.model_context();
    let Message::User(user) = &context[0] else { panic!("archive summary") };
    let UserContent::Blocks(blocks) = &user.content else { panic!("text archive blocks") };
    assert!(blocks.iter().all(|block| !matches!(block, UserBlock::Image(_))));
    let mut preserve = result.preserve_data.take().unwrap();
    preserve["snapcompact"].as_object_mut().unwrap().remove("text");
    write_entries(&path, &fixture_entries(preserve));
    let journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
    assert!(summary(&journal).archive.unwrap().frames.is_empty());
    for data in [format!("blob:sha256:{}", "a".repeat(64)), "blob:sha256:../../outside".into()] {
        write_entries(&path, &fixture_entries(prepared(&data).preserve_data.unwrap()));
        let journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
        assert_eq!(summary(&journal).archive.unwrap().frames[0].data, data);
        assert_eq!(journal.report.blob_warnings.len(), 1);
    }
}

#[test]
fn stale_sources_invalid_remote_mix_and_blob_io_failure_cannot_publish_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blocked-blobs");
    let mut journal = SessionJournal::create_with_blob_directory(dir.path(), dir.path(), &blobs).unwrap();
    let first = turn(&mut journal, "first");
    let second = turn(&mut journal, "second");
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let before = journal.entries().to_vec();
    let disk = fs::read(journal.path()).unwrap();
    fs::write(&blobs, "not a directory").unwrap();
    let prepared = prepared(&STANDARD.encode(vec![42; 2000]));
    assert!(matches!(
        journal.commit_native_entry_snapcompact(&snapshot, &prepared, &second[0], &first, 200),
        Err(NativeSnapcompactError::Storage(_))
    ));
    assert_eq!(journal.entries(), before);
    assert_eq!(fs::read(journal.path()).unwrap(), disk);
    fs::remove_file(&blobs).unwrap();
    let canonical = ara_session::BlobStore::new(&blobs).put(&[42u8; 2000], None).unwrap().path;
    fs::remove_file(&canonical).unwrap();
    fs::create_dir(&canonical).unwrap();
    assert!(
        matches!(
            journal.commit_native_entry_snapcompact(&snapshot, &prepared, &second[0], &first, 200),
            Err(NativeSnapcompactError::Storage(_))
        ),
        "an unreadable canonical directory must fail before JSONL publication"
    );
    assert_eq!(journal.entries(), before);
    assert_eq!(fs::read(journal.path()).unwrap(), disk);
    let mut invalid = prepared.clone();
    invalid.preserve_data.as_mut().unwrap()["openaiRemoteCompaction"] = json!({"opaque":"old"});
    assert!(matches!(
        journal.commit_native_entry_snapcompact(&snapshot, &invalid, &second[0], &first, 200),
        Err(NativeSnapcompactError::InvalidArchive)
    ));
    journal.append_model_change("fixture/new").unwrap();
    assert!(matches!(
        journal.commit_native_entry_snapcompact(&snapshot, &prepared, &second[0], &first, 200),
        Err(NativeSnapcompactError::Validation(CompactionCommitError::StaleSnapshot))
    ));
}
