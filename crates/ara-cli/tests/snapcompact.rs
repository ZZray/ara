//! Aggregated fixed-OMP Host seam families (596f2da, MIT).
//! Sources: agent-session-snapcompact-budget.test.ts and
//! agent-session-snapcompact-frame-dead-end.test.ts. Real RPC/event/dead-end
//! scheduling is covered by the supplied-binary runner, not mirrored here.
use ara_agent::tokenizer::{MessageCountOptions, count_messages};
use ara_ai::{AssistantBlock, AssistantMessage, Message, Model, UserMessage};
use ara_cli::snapcompact::*;
use ara_session::{NativeSnapcompactSnapshot, SessionJournal};
use ara_snapcompact::{self as snap, ShapeTarget};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn model() -> Model {
    Model {
        id: "claude-sonnet-4-5".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: String::new(),
        reasoning: false,
        max_tokens: Some(64_000),
        context_window: Some(200_000.0),
        tokenizer: None,
    }
}
fn policy() -> SnapcompactPolicy {
    SnapcompactPolicy { shape: "auto".into(), reserve_tokens: Some(16_384.0), non_message_tokens: 0, pending_tokens: 0 }
}
fn turn(journal: &mut SessionJournal, text: &str) -> [String; 2] {
    let mut answer = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-sonnet-4-5");
    answer.content.push(AssistantBlock::text(text));
    [
        journal.append_message(&Message::User(UserMessage::text(text))).unwrap(),
        journal.append_message(&Message::Assistant(answer)).unwrap(),
    ]
}
fn capacity(model: &Model) -> usize {
    snap::geometry(
        &snap::resolve_shape(Some(&ShapeTarget { api: Some(model.api.clone()), id: Some(model.id.clone()) }), None)
            .unwrap(),
        None,
    )
    .capacity
}
fn source_messages(snapshot: &NativeSnapcompactSnapshot) -> Vec<Message> {
    snapshot.projection.entries.iter().flat_map(|e| e.messages.clone()).collect()
}
fn archive(prepared: &PreparedSnapcompact) -> snap::Archive {
    snap::get_preserved_archive(prepared.result.preserve_data.as_ref()).unwrap()
}
fn seed_archive(path: &Path, frames: usize, before: &str, after: &str) {
    let message = |text: &str| serde_json::to_value(Message::User(UserMessage::text(text))).unwrap();
    let mut answer = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-sonnet-4-5");
    answer.content.push(AssistantBlock::text("settled answer"));
    let answer = serde_json::to_value(Message::Assistant(answer)).unwrap();
    let raw = |id: &str, parent: Value, message: Value| json!({"type":"message","id":id,"parentId":parent,"timestamp":"2026-10-02T00:00:00.000Z","message":message});
    let frames: Vec<_> = (0..frames).map(|i| json!({"data":STANDARD.encode(format!("stale-frame-{i}")),"mimeType":"image/png","cols":4,"rows":2,"chars":8})).collect();
    let mut entries = vec![
        raw("q0", Value::Null, message("consumed old question")),
        raw("a0", json!("q0"), answer.clone()),
        raw("q1", json!("a0"), message(before)),
        raw("a1", json!("q1"), answer.clone()),
        json!({"type":"compaction","id":"c1","parentId":"a1","timestamp":"2026-10-02T00:00:00.000Z","method":"snapcompact","summary":"Archived history onto stale snapcompact frames.","shortSummary":"stale snapcompact archive","firstKeptEntryId":"q1","tokensBefore":150000,"details":{"readFiles":["src/a.ts"],"modifiedFiles":["src/b.ts"]},"preserveData":{"snapcompact":{"frames":frames,"text":format!("HEAD sentinel. {}TAIL sentinel.", "Archived history line. ".repeat(200)),"totalChars":4600,"truncatedChars":0}}}),
    ];
    if !after.is_empty() {
        entries.push(raw("q2", json!("c1"), message(after)));
        entries.push(raw("a2", json!("q2"), answer));
    }
    let mut body = format!(
        "{}\n",
        json!({"type":"session","version":3,"id":"snapcompact-host-fixture","cwd":"/","timestamp":"2026-10-02T00:00:00.000Z"})
    );
    for entry in entries {
        body.push_str(&format!("{entry}\n"));
    }
    fs::write(path, body).unwrap();
}

#[test]
fn budget_family_sizes_full_projection_and_separates_regular_from_rescue_floors() {
    let mut model = model();
    let mut policy = policy();
    policy.non_message_tokens = 1000;
    policy.pending_tokens = 500;
    let edge = capacity(&model);
    let cap = regular_max_frames(&model, &policy, 100_000, edge);
    assert!(cap > 0 && cap < snap::MAX_FRAMES_DEFAULT && cap <= snap::max_frames_for_data_budget(None));
    // Original upstream observable acceptance: full prompt includes both text
    // edges plus template overhead, not only frameCount * frame estimate.
    let full = 101_500 + cap * snap::FRAME_TOKEN_ESTIMATE + (2 * edge).div_ceil(4) + 2000;
    assert!(full as f64 <= prompt_budget(model.context_window, policy.reserve_tokens));
    let empty = SnapcompactPolicy { non_message_tokens: 0, pending_tokens: 0, ..policy.clone() };
    assert_eq!(
        regular_max_frames(&model, &empty, 168_500, edge),
        1,
        "positive 1500-token residual must attempt a text-only archive"
    );
    assert_eq!(regular_max_frames(&model, &empty, 170_000, edge), 0);
    assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 40_000, edge), 0, "rescue has no min-one exception");
    assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 100_000, edge), 0);
    for window in [None, Some(0.0)] {
        model.context_window = window;
        assert_eq!(regular_max_frames(&model, &empty, 0, edge), snap::max_frames_for_data_budget(None));
        assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 0, edge), snap::max_frames_for_data_budget(None));
    }
    model.context_window = Some(500_000.0);
    model.provider = "ramp".into();
    assert_eq!(regular_max_frames(&model, &empty, 0, edge), snap::DEFAULT_PROVIDER_IMAGE_BUDGET);
    assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 0, edge), snap::DEFAULT_PROVIDER_IMAGE_BUDGET);
    assert!(supports_images(Some(&json!({"input":["text","image"]}))));
    assert!(!supports_images(Some(&json!({"input":["text"]}))));
    assert!(!supports_images(None));
    assert_eq!(parse_manual_args("snapcompact").unwrap().methods, Some(vec!["snapcompact".into()]));
    assert!(parse_manual_args("snapcompact focus text").is_err());
}

#[test]
fn ordinary_preparation_family_runs_real_local_archive_and_checked_publication() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let mut journal = SessionJournal::create_with_blob_directory(dir.path(), dir.path(), &blobs).unwrap();
    let filler = "the quick brown fox jumps over the lazy dog. ".repeat(64);
    for i in 0..64 {
        turn(&mut journal, &format!("turn {i}: {filler}"));
    }
    journal.append_message(&Message::User(UserMessage::text("x".repeat(400_000)))).unwrap();
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let before = journal.entries().to_vec();
    let model = model();
    let mut policy = policy();
    policy.non_message_tokens = 1000;
    policy.pending_tokens = 500;
    let prepared = prepare_snapcompact(&snapshot, &model, 4000, 200_000, &policy).unwrap().unwrap();
    assert!(!prepared.window_source_entry_ids.is_empty());
    assert!(prepared.tokens_after as f64 <= prompt_budget(model.context_window, policy.reserve_tokens));
    let a = archive(&prepared);
    assert!(a.frames.len() <= snap::max_frames_for_data_budget(None));
    assert!(snap::frame_data_bytes(&a.frames) <= snap::FRAME_DATA_BYTES_BUDGET);
    let id = journal
        .commit_native_entry_snapcompact(
            &snapshot,
            &prepared.summary,
            &prepared.result.first_kept_entry_id,
            &prepared.window_source_entry_ids,
            200_000,
        )
        .unwrap();
    assert_eq!(&journal.entries()[..before.len()], before);
    assert_eq!(journal.entries().last().unwrap().id, id);
    let reopened = SessionJournal::open_with_blob_directory(journal.path(), &blobs).unwrap();
    assert_eq!(reopened.model_context(), journal.model_context());
    let actual_projection = count_messages(&reopened.model_context(), MessageCountOptions::default())
        + policy.non_message_tokens
        + policy.pending_tokens;
    assert_eq!(
        actual_projection, prepared.tokens_after,
        "accepted budget is the same complete rebuilt Session projection"
    );
}

#[test]
fn text_only_preparation_family_retains_the_min_one_attempt() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    journal.append_message(&Message::User(UserMessage::text("Run the suite."))).unwrap();
    let call: Message = serde_json::from_value(json!({"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"bash","arguments":{"command":"run tests"}}],"api":"anthropic-messages","provider":"anthropic","model":"claude-sonnet-4-5","stopReason":"stop","timestamp":0,"usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}}})).unwrap();
    journal.append_message(&call).unwrap();
    let result: Message = serde_json::from_value(json!({"role":"toolResult","toolCallId":"call-1","toolName":"bash","content":[{"type":"text","text":"x".repeat(50_000)}],"isError":false,"timestamp":0})).unwrap();
    journal.append_message(&result).unwrap();
    journal.append_message(&Message::User(UserMessage::text("y".repeat(168_500 * 4)))).unwrap();
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let model = model();
    let policy = policy();
    let prepared = prepare_snapcompact(&snapshot, &model, 4000, 190_000, &policy).unwrap().unwrap();
    let archive = archive(&prepared);
    assert!(archive.frames.is_empty());
    assert!(archive.text.as_deref().unwrap().contains("ch elided"));
    assert!(prepared.tokens_after < 170_000);
}

#[test]
fn rescue_family_keeps_both_sides_of_the_archive_in_the_same_complete_projection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.jsonl");
    let blobs = dir.path().join("blobs");
    seed_archive(&path, 16, "kept before archive sentinel", "kept after archive sentinel");
    let mut journal = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let before = journal.entries().to_vec();
    let kept = source_messages(&snapshot);
    assert_eq!(kept.len(), 4);
    let mut policy = policy();
    policy.non_message_tokens = 500;
    policy.pending_tokens = 200;
    let prepared = prepare_frame_rescue(&snapshot, &model(), &policy, 60_000.0).unwrap().unwrap();
    assert!(archive(&prepared).frames.len() < 16);
    assert!(prepared.window_source_entry_ids.is_empty());
    assert_eq!(prepared.result.first_kept_entry_id, "q1");
    assert_eq!(prepared.summary.read_files, vec!["src/a.ts"]);
    assert_eq!(prepared.summary.modified_files, vec!["src/b.ts"]);
    let rescued = journal.commit_native_snapcompact_rescue(&snapshot, &prepared.summary, 150_000).unwrap();
    assert_eq!(&journal.entries()[..before.len()], before);
    assert_eq!(journal.entries().last().unwrap().raw["archiveSourceEntryId"], "c1");
    assert_eq!(journal.entries().last().unwrap().id, rescued);
    let reopened = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    let projected = reopened.model_context();
    assert_eq!(&projected[1..], kept.as_slice(), "all raw kept entries before and after append point survive rescue");
    assert_eq!(
        count_messages(&projected, Default::default()) + policy.non_message_tokens + policy.pending_tokens,
        prepared.tokens_after
    );
    assert!(prepared.tokens_after < 48_000);
    assert_eq!(journal.entries().iter().filter(|e| e.kind == "compaction").count(), 2);
}

#[test]
fn rescue_no_headroom_and_minimum_archive_family_leaves_the_real_tail_available() {
    let dir = tempfile::tempdir().unwrap();
    let blobs = dir.path().join("blobs");
    let model = model();
    let policy = policy();
    for (label, frames, before, after) in [
        ("post-kept", 16, "hello".into(), "y".repeat(160_000)),
        ("pre-kept", 16, "y".repeat(160_000), "".into()),
        ("oversized-tail", 16, "hello".into(), "x".repeat(400_000)),
        ("minimum-frame", 1, "hello".into(), "".into()),
    ] {
        let path = dir.path().join(format!("{label}.jsonl"));
        seed_archive(&path, frames, &before, &after);
        let journal = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
        let snapshot = journal.native_snapcompact_snapshot().unwrap();
        let original = fs::read(&path).unwrap();
        assert!(prepare_frame_rescue(&snapshot, &model, &policy, 60_000.0).unwrap().is_none(), "{label}");
        assert_eq!(
            fs::read(&path).unwrap(),
            original,
            "a failed rescue does not append a barrier over the retained tail"
        );
        if !after.is_empty() {
            assert_eq!(journal.entries().last().unwrap().kind, "message");
        }
    }
}
