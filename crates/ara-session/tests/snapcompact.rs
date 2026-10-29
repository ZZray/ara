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
        preserve_source: None,
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

fn exact_prepared(data: &str) -> NativeSnapcompactSummary {
    let data = serde_json::to_string(data).unwrap();
    let raw = serde_json::value::RawValue::from_string(format!(
        r#"{{"extraMeta":"\udfff","snapcompact":{{
        "frames":[{{"data":{data},"mimeType":"image/png","cols":80,"rows":20,"chars":1600,
            "detail":"original","opaque":"\ud800"}}],"totalChars":1620,"truncatedChars":0,
        "text":"HEAD \ud800 full imaged middle \udc00 TAIL","textHead":"HEAD \ud800","textTail":"\udc00 TAIL"}}}}"#
    ))
    .unwrap();
    NativeSnapcompactSummary {
        summary: "Exact history kept as archive.".into(),
        short_summary: Some(String::new()),
        preserve_data: None,
        preserve_source: Some(ara_session::NativeSnapcompactArchiveSource::from_raw(&raw).unwrap()),
        read_files: vec!["source.rs".into()],
        modified_files: vec!["result.json".into()],
    }
}

fn write_exact_import(path: &std::path::Path, prepared: &NativeSnapcompactSummary) {
    write_entries(path, &fixture_entries(Value::Null));
    let body = fs::read_to_string(path).unwrap();
    let lines = body.lines().collect::<Vec<_>>();
    let mut last = ara_rpc::WireValue::parse(lines.last().unwrap()).unwrap();
    last.insert(
        "preserveData",
        ara_rpc::WireValue::parse(prepared.preserve_source.as_ref().unwrap().exact_preserve_json().unwrap().get())
            .unwrap(),
    );
    let mut body = lines[..lines.len() - 1].join("\n");
    body.push('\n');
    body.push_str(&last.stringify());
    body.push('\n');
    fs::write(path, body).unwrap();
}

fn exact_archive_json_publish_reopen_history_and_recompact() {
    use ara_rpc::WireValue as W;
    use ara_session::{AgentEntrySource, AgentMessageBody};
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let mut journal = SessionJournal::create_with_blob_directory(dir.path(), dir.path(), &blobs).unwrap();
    let first = turn(&mut journal, "first exact");
    let second = turn(&mut journal, "second exact retained");
    let original = journal.entries().to_vec();
    let data = STANDARD.encode(vec![42; 2000]);
    let prepared = exact_prepared(&data);
    assert!(prepared.preserve_source.as_ref().unwrap().checked_preserve().is_err());
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let archive_id = journal.commit_native_entry_snapcompact(&snapshot, &prepared, &second[0], &first, 200).unwrap();
    assert_eq!(&journal.entries()[..original.len()], original);
    let disk = fs::read_to_string(journal.path()).unwrap();
    let persisted = W::parse(disk.lines().last().unwrap()).unwrap();
    let preserve = persisted.get("preserveData").unwrap();
    let archive = preserve.get("snapcompact").unwrap();
    assert_eq!(archive.get("textHead").unwrap().as_string().unwrap().units(), &[72, 69, 65, 68, 32, 0xd800]);
    let frame = &archive.get("frames").unwrap().as_array().unwrap()[0];
    assert_eq!(frame.get("opaque").unwrap().as_string().unwrap().units(), &[0xd800]);
    let reference = frame.get("data").unwrap().as_string().unwrap().to_utf8().unwrap();
    assert!(ara_session::blob::parse_blob_ref(&reference).is_some(), "exact owner uses shared blob externalization");
    let mut reopened = SessionJournal::open_with_blob_directory(journal.path(), &blobs).unwrap();
    assert_eq!(reopened.report.malformed_records, 0, "lone archive is a valid journal line");
    assert_eq!(&reopened.entries()[..original.len()], original);
    assert_eq!(reopened.entries().last().unwrap().id, archive_id);
    assert!(reopened.report.blob_warnings.is_empty());
    let view = summary(&reopened);
    assert!(view.archive.is_none(), "scalar archive cannot represent this exact source");
    let exact = view.exact_archive().unwrap().unwrap();
    assert_eq!(exact.frames[0].data.to_scalar().unwrap(), reference);
    assert!(exact.text.as_ref().unwrap().as_units().contains(&0xd800));
    assert!(view.previous_summary_for_text_compaction_utf16().unwrap().as_units().contains(&0xdc00));
    assert!(reopened.model_context().is_err(), "legacy typed bridge explicitly rejects lone archive");
    let active = ara_ai::Model {
        id: "m".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let raw_context = reopened.raw_agent_context_for_route_with_images(&active, false, true).unwrap();
    let AgentMessageBody::Native(source) = &raw_context[0].body else { panic!("exact summary body") };
    assert!(matches!(&raw_context[0].source, AgentEntrySource::Snapcompact { .. }));
    assert!(raw_context[0].checked_projection.is_none(), "no full-scalar mirror proof for native archive");
    let current = source.observe().unwrap();
    let model =
        reopened.checked_snapcompact_source_for_route(&current, &archive_id, &active, false, true).unwrap().unwrap();
    let model = model.observe().unwrap();
    assert!(model.get("role").unwrap().as_string().unwrap().equals_ascii("user"));
    let content = model.get("content").unwrap().as_array().unwrap();
    assert!(
        content.iter().any(|block| {
            block.get("text").and_then(W::as_string).is_some_and(|text| text.units().contains(&0xd800))
        })
    );
    assert!(content.iter().any(|block| matches!(block.get("compactionFrame"), Some(W::Bool(true)))));
    let edited = ara_session::NativeSnapcompactArchiveSource::from_wire(
        ara_session::native_wire_from_serde(&json!({"snapcompact":{"text":"edited"}})).unwrap(),
    );
    // Mutating current preserve text loses the grant even with the original ID.
    let mut changed = W::parse(&current.stringify()).unwrap();
    changed.insert("preserveData", W::parse(edited.exact_preserve_json().unwrap().get()).unwrap());
    assert!(
        reopened.checked_snapcompact_source_for_route(&changed, &archive_id, &active, false, true).unwrap().is_none()
    );
    assert!(
        reopened
            .checked_snapcompact_source_for_route(&current, "wrong-source", &active, false, true)
            .unwrap()
            .is_none()
    );
    let rescue = reopened.native_snapcompact_snapshot().unwrap();
    let rescued = reopened.commit_native_snapcompact_rescue(&rescue, &prepared, 100).unwrap();
    reopened.stamp_native_snapcompact_warning(&rescued, "No headroom exact").unwrap();
    let mut reopened = SessionJournal::open_with_blob_directory(reopened.path(), &blobs).unwrap();
    assert_eq!(summary(&reopened).preserve_source, exact_prepared(&reference).preserve_source);
    assert_eq!(reopened.entries().last().unwrap().raw["warning"], "No headroom exact");
    let mut stale = reopened.native_snapcompact_snapshot().unwrap();
    stale.raw.entries.last_mut().unwrap().snapcompact_source = Some(edited);
    assert!(
        matches!(
            reopened.commit_native_snapcompact_rescue(&stale, &prepared, 100),
            Err(NativeSnapcompactError::Validation(CompactionCommitError::StaleSnapshot))
        ),
        "raw snapshot equality includes the exact archive owner"
    );
    let third = turn(&mut reopened, "third kept exact");
    let snapshot = reopened.native_projected_compaction_snapshot().unwrap();
    reopened.commit_native_entry_compaction(&snapshot, "text migrated", &third[0], &second, 100).unwrap();
    let soft = summary(&reopened);
    assert!(!soft.has_archive());
    assert_eq!(
        soft.preserve_source.as_ref().unwrap().observe().get("extraMeta").unwrap().as_string().unwrap().units(),
        &[0xdfff]
    );
    let soft_body = reopened.entries().last().unwrap().compaction_summary_source().unwrap().observe().unwrap();
    assert!(
        reopened
            .checked_snapcompact_source_for_route(&soft_body, &soft.entry_id, &active, false, true)
            .unwrap()
            .is_some(),
        "migrated unknown exact metadata keeps source equality without becoming model archive data"
    );
    let fourth = turn(&mut reopened, "fourth retained after migration");
    let snapshot = reopened.native_snapcompact_snapshot().unwrap();
    // Exact owner is republished through the same source-validated compaction
    // transaction after a text migration, without inventing source provenance.
    reopened.commit_native_entry_snapcompact(&snapshot, &prepared, &fourth[0], &third, 100).unwrap();
    let reopened = SessionJournal::open_with_blob_directory(reopened.path(), &blobs).unwrap();
    assert_eq!(summary(&reopened).preserve_source, exact_prepared(&reference).preserve_source);
    assert_eq!(reopened.report.malformed_records, 0);
}

#[test]
fn native_archive_reopen_rescue_and_fork_keep_images_sources_and_text_migration() {
    exact_archive_json_publish_reopen_history_and_recompact();
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
            provider_payload: None,
            provider_payload_origin: None,
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
    assert_eq!(&reopened.entries()[..before.len()], before);
    let mut expected = journal.entries().last().unwrap().clone();
    let mut expected_preserve = prepared.preserve_data.clone().unwrap();
    expected_preserve["snapcompact"]["frames"][0]["data"] = json!(reference);
    expected.raw["preserveData"] = expected_preserve.clone();
    expected.snapcompact_source = Some(ara_session::NativeSnapcompactArchiveSource::from_wire(
        ara_session::native_wire_from_serde(&expected_preserve).unwrap(),
    ));
    assert_eq!(reopened.entries().last().unwrap(), &expected);
    assert!(reopened.report.blob_warnings.is_empty());
    let view = summary(&reopened);
    assert_eq!(view.entry_id, id);
    assert_eq!(view.source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(view.short_summary.as_deref(), Some(""));
    assert!(view.previous_summary_for_text_compaction().contains("Previous snapcompact archive source text:\n\n"));
    assert!(view.archive_migration_text().unwrap().contains("full imaged middle"));
    assert_eq!(view.preserve_data_without_archive(), None);
    let context = reopened.model_context().unwrap();
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
    assert_eq!(reopened.model_context().unwrap(), context, "derived timestamp is stable");
    let rescue = reopened.native_snapcompact_snapshot().unwrap();
    let rescued_id = reopened.commit_native_snapcompact_rescue(&rescue, &prepared, 100).unwrap();
    reopened.stamp_native_snapcompact_warning(&rescued_id, "No headroom").unwrap();
    let reopened = SessionJournal::open_with_blob_directory(reopened.path(), &blobs).unwrap();
    assert_eq!(summary(&reopened).source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(summary(&reopened).first_kept_entry_id, second[0]);
    assert_eq!(reopened.entries().last().unwrap().raw["archiveSourceEntryId"], id);
    assert_eq!(reopened.entries().last().unwrap().raw["warning"], "No headroom");
    let rescued_context = reopened.model_context().unwrap();
    let fork_dir = dir.path().join("fork");
    let fork = reopened.fork_at(reopened.leaf_id(), Some(&fork_dir)).unwrap();
    assert_eq!(fork.blob_directory(), Some(blobs.as_path()));
    assert_eq!(
        SessionJournal::open_with_blob_directory(fork.path(), &blobs).unwrap().model_context().unwrap(),
        rescued_context
    );
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
    let context = journal.model_context().unwrap();
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
        assert!(journal.report.blob_warnings.is_empty(), "archive resolution is deferred to prompt admission");
        let view = summary(&journal);
        let text = ara_session::snapcompact::model_source_from_summary(
            &view.summary,
            view.exact_preserve_json().unwrap().as_deref(),
        )
        .unwrap()
        .observe()
        .unwrap()
        .stringify();
        assert!(text.contains("archived image frame unavailable here"));
    }
    let exact = exact_prepared("AQI=\n\n[Session persistence truncated large content]");
    write_exact_import(&path, &exact);
    let journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
    let archive = summary(&journal).exact_archive().unwrap().unwrap();
    assert!(archive.frames.is_empty(), "legacy exact frame healing retains source text");
    assert!(archive.text_head.unwrap().as_units().contains(&0xd800));
    assert_eq!(journal.report.malformed_records, 0);
    for data in [format!("blob:sha256:{}", "a".repeat(64)), "blob:sha256:../../outside".into()] {
        write_exact_import(&path, &exact_prepared(&data));
        let journal = SessionJournal::open_with_blob_directory(&path, &dir.path().join("blobs")).unwrap();
        assert_eq!(summary(&journal).exact_archive().unwrap().unwrap().frames[0].data.to_scalar().unwrap(), data);
        assert!(journal.report.blob_warnings.is_empty(), "archive resolution is deferred to prompt admission");
        let view = summary(&journal);
        let text = ara_session::snapcompact::model_source_from_summary(
            &view.summary,
            view.exact_preserve_json().unwrap().as_deref(),
        )
        .unwrap()
        .observe()
        .unwrap()
        .stringify();
        assert!(text.contains("archived image frame unavailable here"));
        assert_eq!(journal.report.malformed_records, 0);
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
    let exact = exact_prepared(&STANDARD.encode(vec![42; 2000]));
    assert!(
        matches!(
            journal.commit_native_entry_snapcompact(&snapshot, &exact, &second[0], &first, 200),
            Err(NativeSnapcompactError::Storage(_))
        ),
        "exact blob IO failure precedes journal publication"
    );
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

// Newly ported fixed source-backed fixtures, not a previously executed family:
// session-manager/build-context.test.ts:314-483, 596f2da (MIT).
// Model request blocks exercise both public Session converters. Transcript
// assertions exercise the existing exact archive/history owner; the Host's
// transcript DTO blocks/images remain a separate unaccepted interface gate.
#[test]
fn fixed_session_context_archive_request_and_transcript_source_family() {
    use ara_rpc::WireValue as W;
    use ara_session::AgentMessageBody;
    use ara_snapcompact::{HistoryBlockOptions, HistoryBlockUtf16};
    let active = ara_ai::Model {
        api: "openai-completions".into(),
        provider: "fixture".into(),
        id: "m".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let edge_frame = "o".repeat(ara_snapcompact::FRAME_DATA_BYTES_BUDGET.div_ceil(2) + 1);
    let newest_frame = "n".repeat(edge_frame.len());
    let large_text = |prefix: &str| format!("{prefix}\n{}", "archived history ".repeat(22_000));
    let frame = |data: String, index: usize, current: bool| {
        let mut value = json!({"data":data,"mimeType":"image/png","cols":64,"rows":40,"chars":1000+index});
        if current {
            value["font"] = json!("8x13");
            value["variant"] = json!("bw");
            value["lineRepeat"] = json!(1);
        }
        value
    };
    let legacy_text = large_text("Issue #4470 legacy archive source");
    let current_text = large_text("Current archive source");
    let small_text = large_text("Large legacy text-only counter");
    let cases = [
        (
            "payload-cap",
            "Snapcompact summary",
            json!({"snapcompact":{
            "frames":[
                {"data":edge_frame,"mimeType":"image/png","cols":10,"rows":10,"chars":10},
                {"data":newest_frame,"mimeType":"image/png","cols":10,"rows":10,"chars":10}],
            "totalChars":20,"truncatedChars":0,"textHead":"old edge","textTail":"new edge"}}),
            "first",
            "response",
            "after compact",
            1,
            2,
        ),
        (
            "legacy-oversized",
            "Legacy snapcompact summary",
            json!({"snapcompact":{
            "frames":(0..17).map(|index| frame("A".repeat(100_000),index,false)).collect::<Vec<_>>(),
            "totalChars":legacy_text.encode_utf16().count(),"truncatedChars":1_500_000,"text":legacy_text,
            "textHead":"oldest retained snapcompact archive text","textTail":"newest retained snapcompact archive text"}}),
            "before compact",
            "archived response",
            "after resume",
            0,
            17,
        ),
        (
            "current-oversized",
            "Current snapcompact summary",
            json!({"snapcompact":{
            "frames":(0..17).map(|index| frame("A".repeat(100_000),index,true)).collect::<Vec<_>>(),
            "totalChars":current_text.encode_utf16().count(),"truncatedChars":1_500_000,"text":current_text,
            "textHead":"current oldest retained text","textTail":"current newest retained text"}}),
            "before compact",
            "archived response",
            "after resume",
            17,
            17,
        ),
        (
            "legacy-small",
            "Small legacy snapcompact summary",
            json!({"snapcompact":{
            "frames":(0..2).map(|index| frame("A".repeat(1000),index,false)).collect::<Vec<_>>(),
            "totalChars":small_text.encode_utf16().count(),"truncatedChars":1_500_000,"text":small_text,
            "textHead":"small legacy oldest retained text","textTail":"small legacy newest retained text"}}),
            "before compact",
            "archived response",
            "after resume",
            2,
            2,
        ),
    ];
    for (name, summary_text, preserve, first, response, last, request_images, transcript_images) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(format!("{name}.jsonl"));
        let user = |text| serde_json::to_value(Message::User(UserMessage::text(text))).unwrap();
        let mut answer = AssistantMessage::empty("openai-completions", "fixture", "m");
        answer.content.push(AssistantBlock::text(response));
        let message = |id: &str, parent: Value, body: Value| {
            json!({"type":"message","id":id,"parentId":parent,
            "timestamp":"2026-10-02T00:00:00.000Z","message":body})
        };
        write_entries(
            &path,
            &[
                message("1", Value::Null, user(first)),
                message("2", json!("1"), serde_json::to_value(Message::Assistant(answer)).unwrap()),
                json!({"type":"compaction","id":"3","parentId":"2","timestamp":"2026-10-02T00:00:00.000Z",
                "method":"snapcompact","summary":summary_text,"firstKeptEntryId":"1","tokensBefore":100,
                "preserveData":preserve}),
                message("4", json!("3"), user(last)),
            ],
        );
        let journal = SessionJournal::open(&path).unwrap();
        let raw = journal.raw_agent_context_for_route_with_images(&active, false, true).unwrap();
        let roles = raw
            .iter()
            .map(|message| match &message.body {
                AgentMessageBody::Native(source) => {
                    source.observe().unwrap().get("role").unwrap().as_string().unwrap().to_utf8().unwrap()
                }
                AgentMessageBody::Legacy(value) => value["role"].as_str().unwrap().to_owned(),
            })
            .collect::<Vec<_>>();
        assert_eq!(roles, ["compactionSummary", "user", "assistant", "user"], "{name}");
        let AgentMessageBody::Native(source) = &raw[0].body else { panic!("exact summary source") };
        let body = source.observe().unwrap();
        assert_eq!(body.get("summary").unwrap().as_string().unwrap().to_utf8().unwrap(), summary_text);
        let projected =
            journal.checked_snapcompact_source_for_route(&body, "3", &active, false, true).unwrap().unwrap();
        let native = projected.observe().unwrap();
        let blocks = native.get("content").unwrap().as_array().unwrap();
        let is_image =
            |block: &W| block.get("type").and_then(W::as_string).is_some_and(|kind| kind.equals_ascii("image"));
        assert_eq!(blocks.iter().filter(|block| is_image(block)).count(), request_images, "{name} native request");
        let typed = journal.model_context().unwrap();
        let Message::User(user) = &typed[0] else { panic!("provider request summary converts to User") };
        let UserContent::Blocks(typed_blocks) = &user.content else { panic!("provider request blocks") };
        assert_eq!(
            typed_blocks.iter().filter(|block| matches!(block, UserBlock::Image(_))).count(),
            request_images,
            "{name} typed request"
        );
        let view = summary(&journal);
        let archive = view.exact_archive().unwrap().unwrap();
        let transcript = ara_snapcompact::history_blocks_utf16(&archive, HistoryBlockOptions::default()).unwrap();
        assert_eq!(
            transcript.iter().filter(|block| matches!(block, HistoryBlockUtf16::Image { .. })).count(),
            transcript_images,
            "{name} exact transcript source history; not Host transcript DTO acceptance"
        );
        if name == "payload-cap" {
            let image_index = blocks.iter().position(is_image).unwrap();
            assert_eq!(blocks[image_index].get("data").unwrap().as_string().unwrap().to_utf8().unwrap(), newest_frame);
            let notice_index = blocks
                .iter()
                .position(|block| {
                    block
                        .get("text")
                        .and_then(W::as_string)
                        .is_some_and(|text| text.to_utf8().unwrap().contains("image middle omitted"))
                })
                .unwrap();
            assert!(notice_index < image_index, "old omission notice precedes newer kept frame");
            let UserBlock::Image(image) =
                typed_blocks.iter().find(|block| matches!(block, UserBlock::Image(_))).unwrap()
            else {
                unreachable!()
            };
            assert_eq!(image.data, newest_frame);
            let typed_notice = typed_blocks
                .iter()
                .position(
                    |block| matches!(block,UserBlock::Text(content) if content.text.contains("image middle omitted")),
                )
                .unwrap();
            let typed_image = typed_blocks.iter().position(|block| matches!(block, UserBlock::Image(_))).unwrap();
            assert!(typed_notice < typed_image);
        } else if name == "legacy-oversized" {
            for edge in ["oldest retained snapcompact", "newest retained snapcompact"] {
                assert!(blocks.iter().any(|block| {
                    block.get("text").and_then(W::as_string).is_some_and(|text| text.to_utf8().unwrap().contains(edge))
                }));
                assert!(
                    typed_blocks
                        .iter()
                        .any(|block| matches!(block,UserBlock::Text(content) if content.text.contains(edge)))
                );
            }
        }
        assert_eq!(
            journal.entries().iter().find(|entry| entry.id == "3").unwrap().checked_raw().unwrap()["preserveData"],
            preserve,
            "request policy never changes retained archive metadata/images"
        );
    }
}

// A retained-all legacy archive is readable only with valid archive data and
// unknown source provenance; it cannot turn explicit bad sources into a grant.
#[test]
fn retained_all_archive_read_rejects_missing_archive_and_explicit_source_ids_without_writes() {
    use ara_session::CompactionProjectionError;

    for case in ["no-archive", "null-sources", "empty-sources"] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("{case}.jsonl"));
        let mut entries = fixture_entries(prepared("AQI=").preserve_data.unwrap());
        let compaction = entries.last_mut().unwrap();
        compaction["firstKeptEntryId"] = json!("q1");
        match case {
            "no-archive" => {
                compaction.as_object_mut().unwrap().remove("preserveData");
            }
            "null-sources" => compaction["sourceEntryIds"] = Value::Null,
            "empty-sources" => compaction["sourceEntryIds"] = json!([]),
            _ => unreachable!(),
        }
        write_entries(&path, &entries);
        let journal = SessionJournal::open(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let raw_entries = journal.entries().to_vec();
        let leaf = journal.leaf_id().map(str::to_owned);
        assert_eq!(
            journal.compacted_context_projection(),
            Err(CompactionProjectionError::UnsafeSummaryBoundary { id: "c1".into() }),
            "{case} cannot admit a retained-all archive"
        );
        assert_eq!(journal.entries(), raw_entries, "{case} leaves raw memory unchanged");
        assert_eq!(journal.leaf_id(), leaf.as_deref(), "{case} leaves the active leaf unchanged");
        assert_eq!(fs::read(&path).unwrap(), bytes, "{case} leaves journal bytes unchanged");
    }
}

#[test]
fn lazy_blob_reopen_budget_missing_and_without_thinking_family() {
    use ara_rpc::WireValue as W;
    use ara_session::{AgentMessageBody, BlobStore};
    use ara_snapcompact::{HistoryBlockOptions, Utf16Text};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("lazy.jsonl");
    let blobs = directory.path().join("blobs");
    let store = BlobStore::new(&blobs);
    let old = store.put(&[1, 2], None).unwrap();
    let newest = store.put(&[3, 4], None).unwrap();
    let big = store.put(&vec![9; 3000], None).unwrap();
    let variant = store.put(&[5, 6], None).unwrap();
    let ordinary = store.put(&vec![7; 1500], None).unwrap();
    let missing = format!("blob:sha256:{}", "a".repeat(64));
    let frame = |data: &str| {
        json!({"data":data,"mimeType":"image/png","cols":1,"rows":1,
        "chars":1,"font":"8x8","variant":"bw","opaque":"preserved-frame-meta"})
    };
    let preserve = json!({"ownerMeta":{"unknown":"preserved"},"snapcompact":{
        "frames":[frame(&old.reference),frame(&missing),frame(&big.reference),frame(&newest.reference)],
        "text":"¶user: visible ¶think: secret ¶ai: answer", "textHead":"HEAD", "textTail":"TAIL",
        "totalChars":40,"truncatedChars":0,
        "withoutThinking":{"summary":"Stored safe view", "archive":{
            "frames":[frame(&variant.reference)],"text":"¶user: visible ¶ai: answer",
            "textHead":"SAFE HEAD","textTail":"SAFE TAIL","totalChars":30,"truncatedChars":0}}}});
    let mut entries = fixture_entries(preserve.clone());
    entries[2]["message"]["content"] = json!([{"type":"text","text":"retained question"},
        {"type":"image","data":ordinary.reference,"mimeType":"image/png"}]);
    write_entries(&path, &entries);
    let disk_before = fs::read(&path).unwrap();
    #[cfg(windows)]
    let locked = {
        use std::os::windows::fs::OpenOptionsExt;
        let file = fs::OpenOptions::new().read(true).share_mode(0).open(&big.path).unwrap();
        assert!(fs::read(&big.path).is_err(), "deny-share fixture must reject a physical payload read");
        file
    };
    let journal = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    assert!(journal.report.blob_warnings.is_empty(), "frame refs are not eagerly hydrated");
    let source = summary(&journal).preserve_source.unwrap();
    assert!(source.observe().deep_equal(&ara_session::native_wire_from_serde(&preserve).unwrap()));
    let q2 = journal.entries().iter().find(|entry| entry.id == "q2").unwrap();
    let ordinary_messages = q2.model_messages().unwrap();
    let ordinary_json = serde_json::to_value(&ordinary_messages).unwrap();
    assert!(
        ordinary_json.to_string().contains(&STANDARD.encode(vec![7; 1500])),
        "ordinary user image hydration remains unchanged"
    );
    let model = ara_ai::Model {
        id: "fixture".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let raw = journal.raw_agent_context_for_route_with_images(&model, false, true).unwrap();
    let AgentMessageBody::Native(current) = &raw[0].body else { panic!("exact original archive") };
    let current = current.observe().unwrap();
    let projected = journal
        .checked_snapcompact_source_for_route_with_options(
            &current,
            "c1",
            &model,
            false,
            true,
            HistoryBlockOptions { max_frame_data_bytes: Some(8) },
            true,
        )
        .unwrap()
        .unwrap()
        .observe()
        .unwrap();
    let content = projected.get("content").unwrap().as_array().unwrap();
    let image_data = content
        .iter()
        .filter(|block| block.get("type").and_then(W::as_string).is_some_and(|value| value.equals_ascii("image")))
        .map(|block| block.get("data").unwrap().as_string().unwrap().to_utf8().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(image_data, vec![STANDARD.encode([1, 2]), STANDARD.encode([3, 4])]);
    let projected_text = projected.stringify();
    let unavailable = projected_text.find("archived image frame unavailable here").unwrap();
    let omitted = projected_text.find("snapcompact image middle omitted").unwrap();
    assert!(unavailable < omitted, "missing and budget gaps retain source chronology");
    assert!(!projected_text.contains("blob:sha256:"));
    // A smaller request route budget is applied anew, without rewriting the
    // archive or borrowing the previous route's materialized images.
    let smaller = journal
        .checked_snapcompact_source_for_route_with_options(
            &current,
            "c1",
            &model,
            false,
            true,
            HistoryBlockOptions { max_frame_data_bytes: Some(4) },
            true,
        )
        .unwrap()
        .unwrap()
        .observe()
        .unwrap();
    let small_images = smaller
        .get("content")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .filter(|block| block.get("type").and_then(W::as_string).is_some_and(|s| s.equals_ascii("image")))
        .count();
    assert_eq!(small_images, 1);
    let safe = journal
        .checked_snapcompact_source_for_route_with_options(
            &current,
            "c1",
            &model,
            false,
            true,
            HistoryBlockOptions { max_frame_data_bytes: Some(8) },
            false,
        )
        .unwrap()
        .unwrap()
        .observe()
        .unwrap();
    assert!(safe.stringify().contains("Stored safe view"));
    assert!(!safe.stringify().contains("secret"));
    assert!(safe.stringify().contains(&STANDARD.encode([5, 6])));
    assert!(summary(&journal).preserve_source.unwrap().observe().deep_equal(source.observe()));
    assert_eq!(fs::read(&path).unwrap(), disk_before, "projection cannot rewrite the source archive");
    #[cfg(windows)]
    drop(locked);
    // Fixed upstream read() returns its ref if the blob vanishes after stat;
    // the normal pre-stat missing case below is still an unavailable gap.
    let transient = store.put(&[21, 22, 23], None).unwrap();
    let lazy = store.lazy_frame_data_utf16(&Utf16Text::from(transient.reference.clone())).unwrap().unwrap();
    assert_eq!(lazy.bytes, 4);
    fs::remove_file(&transient.path).unwrap();
    assert_eq!((lazy.read)().unwrap().to_scalar().unwrap(), transient.reference);
    assert!(store.lazy_frame_data_utf16(&Utf16Text::from(transient.reference)).unwrap().is_none());
    assert!(store.lazy_frame_data_utf16(&Utf16Text::from("blob:sha256:../../outside")).unwrap().is_none());
    // Explicitly supplied alternate views share the same publish-before-ref
    // persistence path. This does not claim automatic CTX view re-rendering.
    let save_directory = directory.path().join("save");
    let mut saved = SessionJournal::create_with_blob_directory(&save_directory, directory.path(), &blobs).unwrap();
    let discarded = turn(&mut saved, "old saved turn");
    let kept = turn(&mut saved, "kept saved turn");
    let alternate_data = STANDARD.encode(vec![10; 1500]);
    let mut saved_result = prepared(&alternate_data);
    saved_result.preserve_data.as_mut().unwrap()["snapcompact"]["withoutThinking"] = json!({
        "summary":"Saved safe variant", "archive": {"frames":[frame(&alternate_data)],
        "totalChars":1,"truncatedChars":0,"textHead":"saved safe head"}});
    let snapshot = saved.native_snapcompact_snapshot().unwrap();
    saved.commit_native_entry_snapcompact(&snapshot, &saved_result, &kept[0], &discarded, 100).unwrap();
    let loaded = SessionJournal::open_with_blob_directory(saved.path(), &blobs).unwrap();
    let archive = summary(&loaded).exact_archive().unwrap().unwrap();
    let alternate_ref = archive.without_thinking.unwrap().archive.frames[0].data.to_scalar().unwrap();
    let alternate_hash = ara_session::blob::parse_blob_ref(&alternate_ref).unwrap();
    assert_eq!(store.get(alternate_hash).unwrap().unwrap(), vec![10; 1500]);
    assert_eq!(archive.frames[0].data.to_scalar().unwrap(), alternate_ref);
}
