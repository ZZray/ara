//! Grouped native remote families from fixed OMP remote-compaction.test.ts
//! (596f2da, MIT): iterative replay, #6343 re-expansion and durable provenance.
use ara_ai::{AssistantBlock, AssistantMessage, Message, Model, UserMessage};
use ara_session::{
    CompactedContextItem, CompactionCommitError, NativeRemoteError, NativeRemoteSnapshot, NativeRemoteSummary,
    SessionJournal,
};
use serde_json::{Value, json};
use std::fs;

fn model(provider: &str, id: &str) -> Model {
    Model {
        id: id.into(),
        api: "openai-responses".into(),
        provider: provider.into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn turn(journal: &mut SessionJournal, text: &str) -> [String; 2] {
    let mut assistant = AssistantMessage::empty("openai-responses", "openai", "old-model");
    assistant.content.push(AssistantBlock::text(text));
    [
        journal.append_message(&Message::User(UserMessage::text(text))).unwrap(),
        journal.append_message(&Message::Assistant(assistant)).unwrap(),
    ]
}

fn prepared(version: &str, retained: &str) -> NativeRemoteSummary {
    let item = json!({"type":"compaction","encrypted_content":format!("opaque-{version}")});
    let history = json!([
        item,
        {"type":"message","role":"user","content":[{"type":"input_text","text":retained}]},
    ]);
    let payload = if version == "v2" {
        json!({"version":"v2","provider":"openai","replacementHistory":history})
    } else {
        json!({"provider":"openai","replacementHistory":history,"compactionItem":item})
    };
    NativeRemoteSummary {
        summary: "Remote compaction preserved provider-native history for this session.\n<files>\nsource.txt (Read)\nresult.json (Write)\n</files>".into(),
        short_summary: Some(String::new()),
        preserve_data: json!({"openaiRemoteCompaction":payload}),
        read_files: vec!["source.txt".into()], modified_files: vec!["result.json".into()],
    }
}

fn window(snapshot: &NativeRemoteSnapshot, first_kept: &str) -> Vec<String> {
    let kept = snapshot.projection.entries.iter().position(|entry| entry.entry_id == first_kept).unwrap();
    snapshot.projection.entries[..kept]
        .iter()
        .filter(|entry| !entry.messages.is_empty())
        .map(|entry| entry.entry_id.clone())
        .collect()
}

fn native_summary(journal: &SessionJournal, active: &Model) -> ara_session::CompactionSummaryView {
    let projection = journal.compacted_context_projection_for_route(active, true).unwrap();
    let CompactedContextItem::Summary(summary) = &projection.items[0] else { panic!("native summary") };
    summary.as_ref().clone()
}

fn comparable_context(journal: &SessionJournal, active: &Model, available: bool) -> Vec<Message> {
    let projection = journal.compacted_context_projection_for_route(active, available).unwrap();
    let mut context = journal.model_context_for_route(active, available);
    if matches!(projection.items.first(), Some(CompactedContextItem::Summary(summary)) if summary.remote.is_none()) {
        // A text summary is a freshly derived lower-trust User projection.
        // Only its synthetic timestamp varies between calls; retain every
        // original kept message, raw timestamp and native carrier unchanged.
        let Some(Message::User(summary)) = context.first_mut() else { panic!("derived text summary") };
        summary.timestamp = 0;
    }
    context
}

#[test]
fn native_v1_v2_replay_same_provider_different_model_reopen_and_recent_tail_are_exact() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let active = model("openai", "model-a");
    let first = turn(&mut journal, "ORIGINAL ALPHA port 4242");
    let metadata = journal.append_model_change("openai/model-a").unwrap();
    let second = turn(&mut journal, "RECENT SECOND TURN");
    let raw_before = journal.entries().to_vec();
    let header = journal.header().clone();
    let path = journal.path().to_path_buf();
    let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
    let result = prepared("v1", "RECENT SECOND TURN");
    journal.commit_native_entry_remote(&snapshot, &result, &metadata, &first, &second[1], 100).unwrap();
    assert_eq!(&journal.entries()[..raw_before.len()], raw_before);
    assert_eq!(journal.header(), &header);
    assert_eq!(journal.entries().last().unwrap().raw["preserveData"], result.preserve_data);
    let summary = native_summary(&journal, &active);
    assert_eq!(summary.short_summary.as_deref(), Some(""));
    assert_eq!(summary.source_entry_ids.as_ref().unwrap(), &first);
    assert_eq!(summary.remote.as_ref().unwrap().replay_through_entry_id, second[1]);
    assert_eq!(summary.file_details.unwrap().modified_files, result.modified_files);
    let context = journal.model_context_for_route(&active, true);
    assert_eq!(context.len(), 1, "recent history is already covered by the replay carrier");
    let carrier = context[0].as_assistant().unwrap();
    assert!(carrier.content.is_empty() && carrier.usage.is_unknown() && carrier.response_id.is_none());
    assert_eq!(carrier.provider_payload.as_ref().unwrap()["dt"], false);
    assert_eq!(
        carrier.provider_payload.as_ref().unwrap()["items"],
        result.preserve_data["openaiRemoteCompaction"]["replacementHistory"]
    );
    let mut reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries(), journal.entries());
    assert_eq!(reopened.model_context_for_route(&active, true), context);
    let changed_model = model("openai", "model-b");
    let changed = reopened.model_context_for_route(&changed_model, true);
    assert_eq!(changed[0].as_assistant().unwrap().model, "model-b");
    assert_eq!(changed[0].as_assistant().unwrap().provider_payload, carrier.provider_payload);
    let third = turn(&mut reopened, "THIRD NEW TURN");
    let fourth = turn(&mut reopened, "FOURTH NEW TURN");
    let context = reopened.model_context_for_route(&changed_model, true);
    assert_eq!(context.len(), 5);
    let snapshot = reopened.native_remote_snapshot(&changed_model, true).unwrap();
    assert_eq!(
        snapshot
            .projection
            .entries
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect::<Vec<_>>(),
        third.iter().chain(&fourth).cloned().collect::<Vec<_>>()
    );
    let result = prepared("v2", "FOURTH NEW TURN");
    reopened
        .commit_native_entry_remote(&snapshot, &result, &fourth[0], &window(&snapshot, &fourth[0]), &fourth[1], 80)
        .unwrap();
    let summary = native_summary(&reopened, &changed_model);
    assert_eq!(summary.source_entry_ids.unwrap(), first.into_iter().chain(second).chain(third).collect::<Vec<_>>());
    assert_eq!(reopened.model_context_for_route(&changed_model, true).len(), 1);
    assert!(reopened.native_remote_snapshot(&changed_model, true).unwrap().projection.entries.is_empty());
    assert_eq!(
        SessionJournal::open(&path).unwrap().model_context_for_route(&changed_model, true),
        reopened.model_context_for_route(&changed_model, true)
    );
}

#[test]
fn foreign_disabled_and_unsupported_routes_expand_sources_and_keep_earlier_readable_boundary() {
    for previous_text in [false, true] {
        let mut journal = SessionJournal::in_memory(
            json!({"type":"session","version":3,"id":format!("route-{previous_text}"),"timestamp":"t","cwd":"/"}),
        )
        .unwrap();
        let first = turn(&mut journal, "ORIGINAL FIRST");
        let second = turn(&mut journal, "ORIGINAL ALPHA port 4242");
        if previous_text {
            let snapshot = journal.native_projected_compaction_snapshot().unwrap();
            journal
                .commit_native_entry_compaction(&snapshot, "portable original first summary", &second[0], &first, 200)
                .unwrap();
        }
        let third = turn(&mut journal, "retained third");
        let active = model("openai", "model-a");
        let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
        let ids = window(&snapshot, &third[0]);
        journal
            .commit_native_entry_remote(&snapshot, &prepared("v1", "retained third"), &third[0], &ids, &third[1], 150)
            .unwrap();
        let fourth = turn(&mut journal, "fourth kept");
        for (route, available) in [
            (model("foreign", "model-f"), true),
            (active.clone(), false),
            (Model { api: "openai-completions".into(), ..active.clone() }, true),
        ] {
            let snapshot = journal.native_projected_compaction_snapshot_for_route(&route, available).unwrap();
            let expanded =
                serde_json::to_string(&snapshot.entries.iter().flat_map(|entry| &entry.messages).collect::<Vec<_>>())
                    .unwrap();
            assert!(expanded.contains("ORIGINAL ALPHA port 4242"));
            assert_eq!(snapshot.previous_summary.is_some(), previous_text);
            assert!(!snapshot.previous_summary.as_ref().is_some_and(|summary| summary.remote.is_some()));
            assert_eq!(expanded.contains("ORIGINAL FIRST"), !previous_text);
            assert!(
                !serde_json::to_string(&journal.model_context_for_route(&route, available))
                    .unwrap()
                    .contains("Remote compaction preserved provider-native history")
            );
        }
        assert_eq!(
            comparable_context(&journal, &active, false),
            comparable_context(&journal, &model("foreign", "model-f"), true)
        );
        let snapshot = journal.native_projected_compaction_snapshot().unwrap();
        let kept = snapshot.entries.iter().position(|entry| entry.entry_id == fourth[0]).unwrap();
        let consumed: Vec<String> = snapshot.entries[..kept]
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect();
        let remote_snapshot = journal.native_remote_snapshot(&active, true).unwrap();
        let portable = ara_session::NativeHandoffSummary {
            summary: "portable ALPHA 4242 after native replay".into(),
            read_files: vec!["source.txt".into()],
            modified_files: vec!["result.json".into()],
        };
        journal
            .commit_native_entry_remote_text(&remote_snapshot, &snapshot, &portable, "", &fourth[0], &consumed, 100)
            .unwrap();
        let summary = native_summary(&journal, &active);
        assert_eq!(summary.method, "remote");
        assert_eq!(summary.short_summary.as_deref(), Some(""));
        assert!(summary.remote.is_none());
        assert_eq!(summary.source_entry_ids.unwrap(), first.into_iter().chain(second).chain(third).collect::<Vec<_>>());
        assert_eq!(comparable_context(&journal, &active, true), comparable_context(&journal, &active, false));
        assert!(journal.entries().last().unwrap().raw.get("preserveData").is_none());
        let fifth = turn(&mut journal, "fifth kept");
        let snapshot = journal.native_projected_compaction_snapshot().unwrap();
        journal
            .commit_native_entry_compaction(&snapshot, "soft portable after generic remote", &fifth[0], &fourth, 80)
            .unwrap();
        let summary = native_summary(&journal, &active);
        assert_eq!(summary.method, "soft");
        assert!(summary.remote.is_none() && summary.short_summary.is_none());
    }
}

#[test]
fn exact_raw_cut_replay_through_and_native_details_refuse_invalid_publication_without_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let active = model("openai", "model-a");
    let first = turn(&mut journal, "completed");
    let kept = turn(&mut journal, "retained");
    let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
    let bytes = fs::read(journal.path()).unwrap();
    let entries = journal.entries().to_vec();
    let mut stale = snapshot.clone();
    stale.raw.entries[0].raw["opaqueReceipt"] = json!("different raw despite equal model text");
    assert!(matches!(
        journal.commit_native_entry_remote(&stale, &prepared("v1", "retained"), &kept[0], &first, &kept[1], 100),
        Err(NativeRemoteError::Validation(CompactionCommitError::StaleSnapshot))
    ));
    for through in [&first[1], &kept[0], "another-session-id"] {
        assert!(matches!(
            journal.commit_native_entry_remote(&snapshot, &prepared("v1", "retained"), &kept[0], &first, through, 100),
            Err(NativeRemoteError::InvalidReplayThrough)
        ));
    }
    let mut invalid = prepared("v1", "retained");
    invalid.modified_files.push("source.txt".into());
    assert!(matches!(
        journal.commit_native_entry_remote(&snapshot, &invalid, &kept[0], &first, &kept[1], 100),
        Err(NativeRemoteError::InvalidFileDetails)
    ));
    invalid = prepared("v1", "retained");
    invalid.preserve_data["openaiRemoteCompaction"]["provider"] = json!("foreign");
    assert!(matches!(
        journal.commit_native_entry_remote(&snapshot, &invalid, &kept[0], &first, &kept[1], 100),
        Err(NativeRemoteError::InvalidReplay)
    ));
    assert_eq!(journal.entries(), entries);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    journal
        .commit_native_entry_remote(&snapshot, &prepared("v1", "retained"), &kept[0], &first, &kept[1], 100)
        .unwrap();
    let id = journal.entries().last().unwrap().id.clone();
    let good: Vec<Value> =
        fs::read_to_string(journal.path()).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for fault in ["sources", "through", "details", "preserve", "extension"] {
        let mut lines = good.clone();
        let record = lines.iter_mut().find(|entry| entry["id"] == id).unwrap();
        match fault {
            "sources" => {
                record.as_object_mut().unwrap().remove("sourceEntryIds");
            }
            "through" => record["providerReplayThroughEntryId"] = json!(kept[0]),
            "details" => record["details"]["modifiedFiles"] = json!(["source.txt"]),
            "preserve" => record["preserveData"] = json!({}),
            "extension" => record["fromExtension"] = json!(true),
            _ => unreachable!(),
        }
        fs::write(journal.path(), lines.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
        let loaded = SessionJournal::open(journal.path()).unwrap();
        assert!(loaded.compacted_context_projection_for_route(&active, true).is_err(), "{fault}");
    }
}

#[test]
fn native_remote_branch_and_reset_do_not_resurrect_or_cover_other_raw_windows() {
    let mut journal =
        SessionJournal::in_memory(json!({"type":"session","version":3,"id":"reset-remote","timestamp":"t","cwd":"/"}))
            .unwrap();
    let active = model("openai", "model-a");
    let first = turn(&mut journal, "before remote");
    let kept = turn(&mut journal, "old recent");
    let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
    journal
        .commit_native_entry_remote(&snapshot, &prepared("v1", "old recent"), &kept[0], &first, &kept[1], 100)
        .unwrap();
    let fork = journal.fork_at(Some(&first[1]), None).unwrap();
    assert!(fork.native_projected_compaction_snapshot_for_route(&active, true).unwrap().previous_summary.is_none());
    assert_eq!(fork.model_context_for_route(&active, true).len(), 2);
    journal.append_reset_boundary().unwrap();
    assert!(journal.model_context_for_route(&active, true).is_empty());
    let fresh = turn(&mut journal, "fresh complete");
    let next = turn(&mut journal, "fresh retained");
    let snapshot = journal.native_remote_snapshot(&active, true).unwrap();
    assert!(snapshot.projection.previous_summary.is_none());
    assert!(matches!(
        journal.commit_native_entry_remote(
            &snapshot,
            &prepared("v1", "fresh retained"),
            &next[0],
            &fresh,
            &kept[1],
            90
        ),
        Err(NativeRemoteError::InvalidReplayThrough)
    ));
    journal
        .commit_native_entry_remote(&snapshot, &prepared("v1", "fresh retained"), &next[0], &fresh, &next[1], 90)
        .unwrap();
    assert_eq!(native_summary(&journal, &active).source_entry_ids.unwrap(), fresh);
    assert_eq!(journal.model_context_for_route(&active, true).len(), 1);
    assert!(!serde_json::to_string(&journal.model_context()).unwrap().contains("old recent"));
}
