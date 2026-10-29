//! Aggregated fixed-OMP Host seam families (596f2da, MIT).
//! Sources: agent-session-snapcompact-budget.test.ts and
//! agent-session-snapcompact-frame-dead-end.test.ts. Real RPC/event/dead-end
//! scheduling is covered by the supplied-binary runner, not mirrored here.
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
    SnapcompactPolicy {
        shape: "auto".into(),
        reserve_tokens: Some(16_384.0),
        non_message_tokens: 0,
        pending_tokens: 0,
        deployment: None,
    }
}
fn turn(journal: &mut SessionJournal, text: &str) -> [String; 2] {
    let mut answer = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-sonnet-4-5");
    answer.content.push(AssistantBlock::text(text));
    [
        journal.append_message(&Message::User(UserMessage::text(text))).unwrap(),
        journal.append_message(&Message::Assistant(answer)).unwrap(),
    ]
}
fn shape(model: &Model) -> snap::Shape {
    snap::resolve_shape(
        Some(&ShapeTarget {
            api: Some(model.api.clone()),
            id: Some(model.id.clone()),
            provider: Some(model.provider.clone()),
            ..Default::default()
        }),
        None,
    )
    .unwrap()
}
fn projected_tokens(journal: &SessionJournal, model: &Model) -> usize {
    let projector = ara_cli::agent_message_projection::AgentMessageProjector::default();
    projector.set_donor(model, None, &Default::default(), true);
    let context = projector.raw_context(journal, model, false, true).unwrap();
    projector.refresh_checked(journal, &context, model).unwrap();
    projector.count_current(&context, model, Default::default()).unwrap()
}
fn source_messages(snapshot: &NativeSnapcompactSnapshot) -> Vec<Message> {
    snapshot.projection.entries.iter().flat_map(|e| e.messages.clone()).collect()
}
fn archive(prepared: &PreparedSnapcompact) -> snap::Archive {
    snap::get_preserved_archive(Some(
        &serde_json::from_str::<Value>(prepared.result.preserve_data.as_ref().unwrap().get()).unwrap(),
    ))
    .unwrap()
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
    let shape = shape(&model);
    let edge = snap::geometry(&shape, None).capacity;
    let cap = regular_max_frames(&model, &policy, 100_000, &shape).unwrap();
    assert!(
        cap > 0
            && cap < snap::MAX_FRAMES_DEFAULT
            && cap <= snap::max_frames_for_data_budget(&shape, snap::FRAME_DATA_BYTES_BUDGET)
    );
    // Original upstream observable acceptance: full prompt includes both text
    // edges plus template overhead, not only frameCount * frame estimate.
    let full = 101_500 + cap * snap::planned_frame_tokens(&shape) as usize + (2 * edge).div_ceil(4) + 2000;
    assert!(full as f64 <= prompt_budget(model.context_window, policy.reserve_tokens));
    let empty = SnapcompactPolicy { non_message_tokens: 0, pending_tokens: 0, ..policy.clone() };
    assert_eq!(
        regular_max_frames(&model, &empty, 168_500, &shape).unwrap(),
        1,
        "positive 1500-token residual must attempt a text-only archive"
    );
    assert_eq!(regular_max_frames(&model, &empty, 170_000, &shape).unwrap(), 0);
    assert_eq!(
        rescue_max_frames(&model, &empty, 60_000.0, 40_000, &shape).unwrap(),
        0,
        "rescue has no min-one exception"
    );
    assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 100_000, &shape).unwrap(), 0);
    for window in [None, Some(0.0)] {
        model.context_window = window;
        assert_eq!(
            regular_max_frames(&model, &empty, 0, &shape).unwrap(),
            snap::max_frames_for_data_budget(&shape, snap::FRAME_DATA_BYTES_BUDGET)
        );
        assert_eq!(
            rescue_max_frames(&model, &empty, 60_000.0, 0, &shape).unwrap(),
            snap::max_frames_for_data_budget(&shape, snap::FRAME_DATA_BYTES_BUDGET)
        );
    }
    model.context_window = Some(500_000.0);
    model.provider = "ramp".into();
    assert_eq!(regular_max_frames(&model, &empty, 0, &shape).unwrap(), snap::DEFAULT_PROVIDER_IMAGE_BUDGET);
    assert_eq!(rescue_max_frames(&model, &empty, 60_000.0, 0, &shape).unwrap(), snap::DEFAULT_PROVIDER_IMAGE_BUDGET);
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
    assert!(a.frames.len() <= snap::max_frames_for_data_budget(&shape(&model), snap::FRAME_DATA_BYTES_BUDGET));
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
    assert_eq!(reopened.model_context().unwrap(), journal.model_context().unwrap());
    let actual_projection = projected_tokens(&reopened, &model) + policy.non_message_tokens + policy.pending_tokens;
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
    let projected = reopened.model_context().unwrap();
    assert_eq!(&projected[1..], kept.as_slice(), "all raw kept entries before and after append point survive rescue");
    assert_eq!(
        projected_tokens(&reopened, &model()) + policy.non_message_tokens + policy.pending_tokens,
        prepared.tokens_after
    );
    assert!(prepared.tokens_after < 48_000);
    assert_eq!(journal.entries().iter().filter(|e| e.kind == "compaction").count(), 2);
    exact_graph_archive_reopen_history_and_rescue();
}

fn exact_graph_archive_reopen_history_and_rescue() {
    use ara_cli::{actual_session::GraphJournalEncoder, agent_message_projection::AgentMessageProjector};
    use ara_rpc::{WireValue, value_heap::ValueSnapshot};
    use std::sync::Arc;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("exact-source.jsonl");
    let blobs = dir.path().join("blobs");
    seed_archive(&path, 16, "kept before archive sentinel", "kept after archive sentinel");
    let source = fs::read_to_string(&path)
        .unwrap()
        .replace("\"preserveData\":{", r#""preserveData":{"audit":"\udc00","n":-0,"#)
        .replace("HEAD sentinel.", r"HEAD \ud800 sentinel.")
        .replace(r#""snapcompact":{"#, r#""snapcompact":{"textHead":"HEAD \ud800 sentinel.","#);
    fs::write(&path, source).unwrap();
    let mut journal = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    let realm = ara_cli::native_realm::NativeRealm::prepare().unwrap();
    let encoder = Arc::new(GraphJournalEncoder::new(realm.values.clone(), None).unwrap());
    let projector = AgentMessageProjector::default();
    projector.bind_native_session(&mut journal, encoder.clone()).unwrap();
    let model = model();
    projector.set_donor(&model, Some(&json!({"input":["text","image"]})), &Default::default(), true);
    let context = projector.raw_context(&journal, &model, false, true).unwrap();
    projector.refresh_checked(&journal, &context, &model).unwrap();
    assert!(projector.count_current(&context, &model, Default::default()).unwrap() > 0);
    assert!(
        projector.estimate_current(&context, &model).is_err(),
        "scalar adapter cannot represent lone archive units"
    );
    let history = projector.convert_current_actual(&context, &model, &realm.values).unwrap();
    assert!(
        history.iter().any(|root| {
            let source = ara_session::NativeMessageSource::live(root.clone(), encoder.clone()).observe().unwrap();
            source.stringify().contains(r"\ud800")
        }),
        "exact model history retains the original archive edge"
    );
    let receipts = projector.current_body_receipts(&context).unwrap();
    assert_eq!(receipts, projector.current_body_receipts(&context).unwrap());
    assert!(projector.reduction_receipts(&context).is_ok());

    let heap = realm.values.heap();
    let summary = context.iter().find(|message| {
        let ara_agent::AgentMessageBody::Graph(root) = message.body_snapshot() else { return false };
        let role = heap.get(&root, &"role".into(), realm.values.runtime()).unwrap();
        matches!(heap.snapshot(&role).unwrap(), ValueSnapshot::String(role) if role.equals_ascii("compactionSummary"))
    }).unwrap();
    let ara_agent::AgentMessageBody::Graph(root) = summary.body_snapshot() else { unreachable!() };
    let preserve = heap.get(&root, &"preserveData".into(), realm.values.runtime()).unwrap();
    heap.define_own_data(&preserve, "n".into(), heap.number(0.0).unwrap(), realm.values.runtime()).unwrap();
    assert_ne!(
        receipts,
        projector.current_body_receipts(&context).unwrap(),
        "Number bits are part of current-source receipts"
    );
    assert!(
        projector.count_current(&context, &model, Default::default()).is_err(),
        "nested source mutation cannot reuse a grant"
    );
    assert!(projector.convert_current_actual(&context, &model, &realm.values).is_err());
    heap.define_own_data(&preserve, "n".into(), heap.number(-0.0).unwrap(), realm.values.runtime()).unwrap();
    summary.invalidate_derived();
    projector.refresh_checked(&journal, &context, &model).unwrap();
    assert!(projector.count_current(&context, &model, Default::default()).is_ok());
    assert!(projector.convert_current_actual(&context, &model, &realm.values).is_ok());

    let snapshot = journal.native_snapcompact_snapshot().unwrap();
    let prepared = prepare_frame_rescue(&snapshot, &model, &policy(), 60_000.0).unwrap().unwrap();
    journal.commit_native_snapcompact_rescue(&snapshot, &prepared.summary, 150_000).unwrap();
    let mut reopened = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    let next_realm = ara_cli::native_realm::NativeRealm::prepare().unwrap();
    let next_encoder = Arc::new(GraphJournalEncoder::new(next_realm.values.clone(), None).unwrap());
    let next_projector = AgentMessageProjector::default();
    next_projector.bind_native_session(&mut reopened, next_encoder).unwrap();
    let next = next_projector.raw_context(&reopened, &model, false, true).unwrap();
    assert!(next_projector.count_current(&next, &model, Default::default()).unwrap() > 0);
    assert!(next_projector.convert_current_actual(&next, &model, &next_realm.values).is_ok());
    let owner = reopened.entries().last().unwrap().snapcompact_source.as_ref().unwrap().observe();
    assert!(matches!(owner.get("audit"), Some(WireValue::String(value)) if value.units() == [0xdc00]));
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

#[test]
fn catalog_deployment_real_png_and_route_cache_family() {
    const CHILD_CASE: &str = "ARA_SNAPCOMPACT_CATALOG_ROUTE_CASE";
    if let Ok(case) = std::env::var(CHILD_CASE) {
        let mut anthropic = model();
        anthropic.id = "claude-opus-5-5".into();
        anthropic.base_url = "https://api.anthropic.com/v1".into();
        let metadata = json!({"compat":{"officialEndpoint":true}});
        let deployment = deployment_target(&anthropic, Some(&metadata)).unwrap();
        let official = case == "official";
        assert_eq!(deployment.official_endpoint, official, "{case}");
        assert_eq!(
            snap::frame_data_bytes_budget(Some(&deployment)).unwrap(),
            if official { 12_000_000 } else { 3_000_000 }
        );
        assert_eq!(snap::provider_image_budget(Some(&deployment)).unwrap(), if official { 600 } else { 90 });
        let mut codex = anthropic.clone();
        codex.api = "openai-codex-responses".into();
        codex.provider = "openai-codex".into();
        codex.id = "gpt-6.1-sol".into();
        let codex = deployment_target(&codex, Some(&metadata)).unwrap();
        assert!(codex.official_endpoint);
        assert_eq!(snap::frame_data_bytes_budget(Some(&codex)).unwrap(), 12_000_000);
        assert_eq!(snap::provider_image_budget(Some(&codex)).unwrap(), 2000);
        println!("route case {case}: official={official}, Anthropic and Codex receipts checked");
        return;
    }
    for case in ["official", "anthropic-env", "foundry-env"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "catalog_deployment_real_png_and_route_cache_family", "--nocapture"])
            .env(CHILD_CASE, case)
            .env_remove("ANTHROPIC_BASE_URL")
            .env_remove("CLAUDE_CODE_USE_FOUNDRY")
            .env_remove("FOUNDRY_BASE_URL");
        if case == "anthropic-env" {
            command.env("ANTHROPIC_BASE_URL", "https://proxy.example/v1");
        }
        if case == "foundry-env" {
            command.env("CLAUDE_CODE_USE_FOUNDRY", "yes").env("FOUNDRY_BASE_URL", "https://foundry.example/v1");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut host_model = model();
    host_model.api = "openai-codex-responses".into();
    host_model.provider = "openai-codex".into();
    host_model.id = "wire-only-id".into();
    let metadata = json!({"catalogModelId":"gpt-6.1-sol","requestModelId":"wire-only-id","providerType":"openai-codex",
        "identity":{"class":"openai","family":"sol","revision":"6.1.0"},"compat":{"officialEndpoint":true},"input":["text","image"]});
    let built = deployment_target(&host_model, Some(&metadata)).unwrap();
    assert_eq!(built.model.id.as_deref(), Some("gpt-6.1-sol"));
    assert_eq!(built.request_model_id.as_deref(), Some("wire-only-id"));
    assert_eq!(built.provider_type.as_deref(), Some("openai-codex"));
    assert_eq!(built.model.identity.as_ref().unwrap().revision.as_deref(), Some("6.1.0"));
    let billing = snap::frame_billing(Some(&built.model)).unwrap();
    let shape = snap::resolve_shape(Some(&built.model), Some("8on22-bw")).unwrap();
    let frame = snap::render("real pixels", &shape, Some(100)).unwrap();
    let raw = ara_rpc::WireValue::parse(
        &json!({"role":"user","content":[{"type":"image","data":frame.data,
        "mimeType":"image/png","compactionFrame":true}]})
        .to_string(),
    )
    .unwrap();
    let source = ara_session::NativeMessageSource::persisted(raw);
    let tokenizer = ara_cli::context_budget::tokenizer(&host_model);
    assert_eq!(
        model_source_tokens_with_billing(&tokenizer, &source, &billing).unwrap(),
        10,
        "100x64 real PNG charges pixels rather than its 1568px planning square"
    );
    let malformed = ara_session::NativeMessageSource::persisted(
        ara_rpc::WireValue::parse(
            r#"{"role":"user","content":[{"type":"image","data":"bad","compactionFrame":true}]}"#,
        )
        .unwrap(),
    );
    assert_eq!(model_source_tokens_with_billing(&tokenizer, &malformed, &billing).unwrap(), 5024);
    route_cache_rebuilds_archive_after_deployment_budget_change(&host_model, &metadata, &frame.data);
}

fn route_cache_rebuilds_archive_after_deployment_budget_change(model: &Model, metadata: &Value, png: &str) {
    use ara_cli::{actual_session::GraphJournalEncoder, agent_message_projection::AgentMessageProjector};
    use std::sync::Arc;
    // A valid PNG ancillary tEXt chunk grows payload bytes without changing its pixels.
    let original = STANDARD.decode(png).unwrap();
    let mut chunk = b"tEXt".to_vec();
    chunk.extend_from_slice(b"fixture\0");
    chunk.resize(1_000_004, b'x');
    let mut crc = 0xffff_ffff_u32;
    for byte in &chunk {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb8_8320 } else { 0 };
        }
    }
    let mut large = original[..original.len() - 12].to_vec();
    large.extend_from_slice(&((chunk.len() - 4) as u32).to_be_bytes());
    large.extend(chunk);
    large.extend_from_slice(&(!crc).to_be_bytes());
    large.extend_from_slice(&original[original.len() - 12..]);
    let data = STANDARD.encode(large);
    assert!(data.len() * 2 < 3_000_000 && data.len() * 3 > 3_000_000);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("route-cache.jsonl");
    let blobs = dir.path().join("blobs");
    seed_archive(&path, 3, "kept source", "after archive");
    let mut rows: Vec<Value> =
        fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for row in &mut rows {
        if row["id"] == "c1" {
            row["preserveData"]["snapcompact"]["frames"] = Value::Array(
                (0..3)
                    .map(|_| {
                        json!({"data":data,"mimeType":"image/png",
            "cols":12,"rows":1,"chars":11,"font":"8x13","variant":"bw"})
                    })
                    .collect(),
            );
        }
    }
    fs::write(&path, rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    let mut journal = SessionJournal::open_with_blob_directory(&path, &blobs).unwrap();
    let realm = ara_cli::native_realm::NativeRealm::prepare().unwrap();
    let encoder = Arc::new(GraphJournalEncoder::new(realm.values.clone(), None).unwrap());
    let projector = AgentMessageProjector::default();
    projector.bind_native_session(&mut journal, encoder.clone()).unwrap();
    projector.set_donor(model, Some(metadata), &Default::default(), true);
    let context = projector.raw_context(&journal, model, false, true).unwrap();
    projector.refresh_checked(&journal, &context, model).unwrap();
    let receipts = projector.current_body_receipts(&context).unwrap();
    let count = |projector: &AgentMessageProjector| {
        projector
            .convert_current_actual(&context, model, &realm.values)
            .unwrap()
            .into_iter()
            .map(|root| {
                let body = ara_session::NativeMessageSource::live(root, encoder.clone()).observe().unwrap();
                body.get("content").and_then(ara_rpc::WireValue::as_array).map_or(0, |blocks| {
                    blocks
                        .iter()
                        .filter(|block| {
                            block
                                .get("type")
                                .and_then(ara_rpc::WireValue::as_string)
                                .is_some_and(|kind| kind.equals_ascii("image"))
                        })
                        .count()
                })
            })
            .sum::<usize>()
    };
    assert_eq!(count(&projector), 3);
    let mut proxy = metadata.clone();
    proxy["compat"]["officialEndpoint"] = json!(false);
    projector.set_donor(model, Some(&proxy), &Default::default(), true);
    assert!(
        projector.convert_current_actual(&context, model, &realm.values).is_err(),
        "old larger-budget grant must not match"
    );
    projector.refresh_checked(&journal, &context, model).unwrap();
    assert_eq!(count(&projector), 2, "current three MB policy keeps only newest fitting PNGs");
    assert_eq!(projector.current_body_receipts(&context).unwrap(), receipts, "route views preserve canonical records");
}
