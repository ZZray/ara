//! Concentrated native Session handoff families. Core/Host document generation
//! and file-operations formatting are verified in their own module/host flow.
use ara_ai::{AssistantBlock, AssistantMessage, Message, StopReason, UserMessage};
use ara_session::{
    CompactedContextItem, CompactionCommitError, FailedAssistantRecoveryOutcome, NativeHandoffError,
    NativeHandoffSnapshot, NativeHandoffSummary, SessionJournal,
};
use serde_json::{Value, json};
use std::fs;

fn assistant(text: &str) -> Message {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "model");
    message.content.push(AssistantBlock::text(text));
    message.timestamp = 1;
    Message::Assistant(message)
}

fn turn(journal: &mut SessionJournal, label: &str) -> [String; 2] {
    [
        journal.append_message(&Message::User(UserMessage::text(label))).unwrap(),
        journal.append_message(&assistant(label)).unwrap(),
    ]
}

fn prepared(label: &str) -> NativeHandoffSummary {
    NativeHandoffSummary {
        summary: format!("# Handoff\n{label}\n\n<files>\nsource.txt (Read)\nresult.json (Write)\n</files>"),
        read_files: vec!["source.txt".into()],
        modified_files: vec!["result.json".into()],
    }
}

fn window(snapshot: &NativeHandoffSnapshot, first_kept: &str) -> Vec<String> {
    let kept = snapshot.projection.entries.iter().position(|entry| entry.entry_id == first_kept).unwrap();
    snapshot.projection.entries[..kept]
        .iter()
        .filter(|entry| !entry.messages.is_empty())
        .map(|entry| entry.entry_id.clone())
        .collect()
}

fn summary(journal: &SessionJournal) -> ara_session::CompactionSummaryView {
    let projection = journal.compacted_context_projection().unwrap();
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("native summary first") };
    summary.as_ref().clone()
}

#[test]
fn native_handoff_same_session_reopen_and_chained_sources_keep_raw_documents_and_roles() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    journal.set_session_name("Original Session", "user").unwrap();
    let first = turn(&mut journal, "RAW_FIRST_HISTORY");
    let metadata = journal.append_model_change("fixture/model").unwrap();
    let second = turn(&mut journal, "retained second turn");
    let header = journal.header().clone();
    let path = journal.path().to_path_buf();
    let raw_before = journal.entries().to_vec();
    let snapshot = journal.native_handoff_snapshot().unwrap();
    let document = prepared("First completed work and resumed next steps.");
    let id = journal.commit_native_entry_handoff(&snapshot, &document, &metadata, &first, 100).unwrap();
    assert_eq!(journal.path(), path);
    assert_eq!(journal.header(), &header);
    assert_eq!(&journal.entries()[..raw_before.len()], raw_before);
    let record = journal.entries().last().unwrap();
    assert_eq!(record.id, id);
    assert_eq!(record.raw["method"], "handoff");
    assert_eq!(record.raw["summary"], document.summary, "persist document, never the model wrapper");
    assert_eq!(record.raw["firstKeptEntryId"], metadata);
    assert_eq!(record.raw["details"], json!({"readFiles":["source.txt"],"modifiedFiles":["result.json"]}));
    assert_eq!(record.raw["fromExtension"], false);
    for field in ["preserveData", "providerReplayThroughEntryId", "shortSummary", "providerPayload", "blocks"] {
        assert!(record.raw.get(field).is_none(), "handoff must not create {field}");
    }
    let view = summary(&journal);
    assert_eq!(view.method, "handoff");
    assert_eq!(view.source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(view.file_details.as_ref().unwrap().read_files, document.read_files);
    let context = journal.model_context();
    assert_eq!(context.len(), 3);
    let Message::User(message) = &context[0] else { panic!("native lower-trust handoff projection") };
    assert_eq!(
        message.content.plain_text(),
        format!(
            "Context replaced. The <handoff> below is a handoff document a prior instance of you wrote from the full conversation. It is your own working memory, not user input.\n- First person inside it refers to you (the prior instance).\n- \"Next Steps\" is your own resumed plan; re-check it against the latest user message before acting.\n- The handoff already exists and is complete: NEVER write another handoff document unless the user explicitly asks.\nMUST build on prior work; NEVER duplicate prior work.\n\n<handoff>\n{}\n</handoff>",
            document.summary
        )
    );
    assert!(!serde_json::to_string(&context).unwrap().contains("RAW_FIRST_HISTORY"));
    let mut reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries(), journal.entries());
    assert_eq!(reopened.model_context(), context);
    assert_eq!(reopened.native_handoff_snapshot().unwrap(), journal.native_handoff_snapshot().unwrap());
    assert_eq!(reopened.title(), journal.title());
    let third = turn(&mut reopened, "retained third turn");
    let snapshot = reopened.native_handoff_snapshot().unwrap();
    let second_document = NativeHandoffSummary {
        summary: "# Next Handoff\nCarry the completed first and second work.\n<files>\nsource.txt (Read)\nresult.json (Write)\nnext.json (Write)\n</files>".into(),
        read_files: document.read_files,
        modified_files: vec!["result.json".into(), "next.json".into()],
    };
    assert_eq!(window(&snapshot, &third[0]), second);
    reopened.commit_native_entry_handoff(&snapshot, &second_document, &third[0], &second, 80).unwrap();
    let view = summary(&reopened);
    assert_eq!(view.source_entry_ids.unwrap(), first.into_iter().chain(second).collect::<Vec<_>>());
    assert_eq!(view.file_details.unwrap().modified_files, second_document.modified_files);
    assert_eq!(reopened.entries().iter().filter(|entry| entry.kind == "compaction").count(), 2);
    let checked = SessionJournal::open(&path).unwrap();
    assert_eq!(checked.native_handoff_snapshot().unwrap(), reopened.native_handoff_snapshot().unwrap());
    assert_eq!(checked.model_context(), reopened.model_context());
    // A later native soft cut retains its old API/projection and carries the
    // exact handoff provenance forward, without treating the document as raw.
    let fourth = turn(&mut reopened, "fourth kept turn");
    let snapshot = reopened.native_projected_compaction_snapshot().unwrap();
    reopened.commit_native_entry_compaction(&snapshot, "soft after handoff", &fourth[0], &third, 60).unwrap();
    assert_eq!(summary(&reopened).method, "soft");
    assert!(
        serde_json::to_string(&reopened.model_context()[0]).unwrap().contains("Compacted summary of earlier turns")
    );
    assert_eq!(reopened.entries().last().unwrap().raw["method"], "soft");

    let mut memory = SessionJournal::in_memory(
        json!({"type":"session","version":3,"id":"memory-handoff","timestamp":"t","cwd":"/"}),
    )
    .unwrap();
    let old = turn(&mut memory, "memory completed");
    let kept = turn(&mut memory, "memory kept");
    let snapshot = memory.native_handoff_snapshot().unwrap();
    memory.commit_native_entry_handoff(&snapshot, &prepared("memory handoff"), &kept[0], &old, 50).unwrap();
    assert!(!memory.is_on_disk() && !memory.is_persistent());
    assert_eq!(summary(&memory).method, "handoff");
}

#[test]
fn handoff_exact_raw_stale_and_unsupported_legacy_replay_refuse_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let first = turn(&mut journal, "completed");
    let kept = turn(&mut journal, "kept");
    let snapshot = journal.native_handoff_snapshot().unwrap();
    let bytes = fs::read(journal.path()).unwrap();
    let original = journal.entries().to_vec();
    let mut raw_only_changed = snapshot.clone();
    raw_only_changed.raw.entries[0].raw["opaqueReceipt"] = json!("changed despite equal model projection");
    let mut other_session = snapshot.clone();
    other_session.raw.session_id = "another Session".into();
    let mut projection_changed = snapshot.clone();
    projection_changed.projection.entries[0].entry_id = "another source".into();
    for stale in [raw_only_changed, other_session, projection_changed] {
        assert!(matches!(
            journal.commit_native_entry_handoff(&stale, &prepared("stale"), &kept[0], &first, 100),
            Err(NativeHandoffError::Validation(CompactionCommitError::StaleSnapshot))
        ));
    }
    let mut invalid_details = prepared("invalid lists");
    invalid_details.modified_files.push("source.txt".into());
    assert!(matches!(
        journal.commit_native_entry_handoff(&snapshot, &invalid_details, &kept[0], &first, 100),
        Err(NativeHandoffError::InvalidFileDetails)
    ));
    assert!(matches!(
        journal.commit_native_entry_handoff(&snapshot, &prepared("wrong sources"), &kept[0], &[first[1].clone()], 100),
        Err(NativeHandoffError::Validation(CompactionCommitError::InvalidWindow))
    ));
    assert_eq!(journal.entries(), original);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    journal.commit_native_entry_handoff(&snapshot, &prepared("valid"), &kept[0], &first, 100).unwrap();
    let id = journal.entries().last().unwrap().id.clone();
    let good: Vec<Value> =
        fs::read_to_string(journal.path()).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for fault in ["sources", "details", "method", "extension", "archive", "provider"] {
        let mut lines = good.clone();
        let raw = lines.iter_mut().find(|entry| entry["id"] == id).unwrap();
        match fault {
            "sources" => {
                raw.as_object_mut().unwrap().remove("sourceEntryIds");
            }
            "details" => {
                raw["details"]["readFiles"] = json!("not native arrays");
            }
            "method" => {
                raw["method"] = json!("unrecognized");
            }
            "extension" => {
                raw["fromExtension"] = json!(true);
            }
            "archive" => {
                raw["preserveData"] = json!({});
            }
            "provider" => {
                raw["providerReplayThroughEntryId"] = json!(kept[0]);
            }
            _ => unreachable!(),
        }
        fs::write(journal.path(), lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let before = fs::read(journal.path()).unwrap();
        let loaded = SessionJournal::open(journal.path()).unwrap();
        assert!(loaded.compacted_context_projection().is_err(), "{fault} must not broaden native admission");
        assert_eq!(
            fs::read(journal.path()).unwrap(),
            before,
            "strict projection never repairs/adopts an unsupported handoff"
        );
    }
}

#[test]
fn handoff_atomic_prepublication_failure_preserves_recovery_owner_and_commit_bypasses_failed_receipt() {
    let directory = tempfile::tempdir().unwrap();
    for fail_publication in [true, false] {
        let home = directory.path().join(if fail_publication { "blocked" } else { "published" });
        fs::create_dir(&home).unwrap();
        let mut journal = SessionJournal::create(&home, &home).unwrap();
        let first = turn(&mut journal, "completed source");
        let kept = turn(&mut journal, "keep recent work");
        let mut failure = AssistantMessage::empty("openai-completions", "fixture", "model");
        failure.stop_reason = StopReason::Length;
        failure.content.push(AssistantBlock::text("FAILED_ORIGINAL_OUTPUT"));
        let failed = journal.append_message(&Message::Assistant(failure.clone())).unwrap();
        let failed_raw = journal.entries().last().unwrap().clone();
        let owner = journal.begin_failed_assistant_recovery(&failed, &failed, &failure).unwrap();
        let snapshot = journal.native_handoff_snapshot().unwrap();
        let path = journal.path().to_path_buf();
        let bytes = fs::read(&path).unwrap();
        let original = journal.entries().to_vec();
        if fail_publication {
            let saved = path.with_extension("owned-original");
            fs::rename(&path, &saved).unwrap();
            fs::create_dir(&path).unwrap();
            let error = journal
                .commit_native_entry_handoff(&snapshot, &prepared("not published"), &kept[0], &first, 100)
                .unwrap_err();
            assert!(matches!(error, NativeHandoffError::Storage(_)));
            assert!(!error.history_published());
            assert_eq!(journal.entries(), original);
            assert_eq!(journal.native_handoff_snapshot().unwrap(), snapshot);
            assert_eq!(fs::read(&saved).unwrap(), bytes);
            fs::remove_dir(&path).unwrap();
            fs::rename(saved, &path).unwrap();
            let FailedAssistantRecoveryOutcome::Restored { message, appended_entry_id: Some(restored) } =
                journal.finish_failed_assistant_recovery(owner, false).unwrap()
            else {
                panic!("original recovery owner still rolls back")
            };
            assert_eq!(*message, failure);
            assert_ne!(restored, failed);
            assert_eq!(SessionJournal::open(&path).unwrap().leaf_id(), Some(restored.as_str()));
        } else {
            let id = journal
                .commit_native_entry_handoff(&snapshot, &prepared("completed handoff"), &kept[0], &first, 100)
                .unwrap();
            assert_eq!(&journal.entries()[..original.len()], original);
            assert_eq!(
                journal.finish_failed_assistant_recovery(owner, true).unwrap(),
                FailedAssistantRecoveryOutcome::Committed
            );
            let reopened = SessionJournal::open(&path).unwrap();
            assert_eq!(reopened.leaf_id(), Some(id.as_str()));
            assert_eq!(reopened.entries(), journal.entries());
            assert_eq!(reopened.model_context(), journal.model_context());
            assert!(!reopened.branch().iter().any(|entry| entry.id == failed));
            assert!(!serde_json::to_string(&reopened.model_context()).unwrap().contains("FAILED_ORIGINAL_OUTPUT"));
        }
        assert_eq!(journal.entries().iter().find(|entry| entry.id == failed), Some(&failed_raw));
    }
}
