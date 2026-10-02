//! Concentrated fixed OMP native families: source/window mapping, artifact
//! offload/anchor estimates, and checked raw images/thinking persistence.
use std::sync::Arc;

use ara_agent::compaction::local_reduction::{
    PruneConfig, ReductionAction, ReductionEntryKind, ShakeConfig, SupersedePruneConfig,
};
use ara_ai::{Model, model_tokenizer::ModelTokenizer};
use ara_cli::local_reduction::*;
use ara_cli::session_artifacts::{ArtifactUriRouter, SessionArtifacts};
use ara_session::{Entry, SessionJournal, SessionReductionAction, SessionReductionSnapshot};
use ara_tools::ContentUriPort;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn model() -> Model {
    Model {
        id: "fixture".into(),
        api: "openai-completions".into(),
        provider: "fixture".into(),
        base_url: String::new(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn snapshot(raws: Vec<Value>) -> SessionReductionSnapshot {
    let entries: Vec<Entry> = raws
        .into_iter()
        .enumerate()
        .map(|(index, mut raw)| {
            let id = format!("e{index}");
            let parent_id = index.checked_sub(1).map(|index| format!("e{index}"));
            raw["id"] = json!(id);
            raw["parentId"] = json!(parent_id);
            raw["timestamp"] = json!("2026-10-02T00:00:00.000Z");
            Entry { id, parent_id, kind: raw["type"].as_str().unwrap().into(), raw }
        })
        .collect();
    SessionReductionSnapshot {
        session_id: "fixture".into(),
        leaf_id: entries.last().map(|entry| entry.id.clone()),
        entries,
    }
}

fn message(mut value: Value) -> Value {
    if value["role"] == "assistant" {
        value["provider"] = json!("fixture");
        value["model"] = json!("fixture");
        value["api"] = json!("openai-completions");
        value["stopReason"] = json!("stop");
        value["usage"] = json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"totalTokens":0,
            "cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}});
    }
    json!({"type":"message","message":value,"unknownEntry":{"receipt":"retained"}})
}
fn call(id: &str, path: &str) -> Value {
    message(
        json!({"role":"assistant","content":[{"type":"toolCall","id":id,"name":"read","arguments":{"path":path}}],"timestamp":1000}),
    )
}
fn result(id: &str, text: &str) -> Value {
    message(
        json!({"role":"toolResult","toolCallId":id,"toolName":"read","content":[{"type":"text","text":text}],"isError":false,"timestamp":1000}),
    )
}
fn fence() -> String {
    format!("```ts\n{}\n```", "const value = compute(alpha, beta);\n".repeat(80))
}

#[test]
fn raw_source_and_host_window_family() {
    let source = snapshot(vec![
        message(
            json!({"role":"user","content":[{"type":"text","text":"abcd"},{"type":"image","data":"opaque"}],"timestamp":1000}),
        ),
        message(
            json!({"role":"bashExecution","command":"cmd","output":"abcdefg","excludeFromContext":true,"timestamp":1000}),
        ),
        json!({"type":"custom_message","customType":"note","content":"abcdef"}),
        json!({"type":"custom","customType":"opaque","data":{"text":"x".repeat(20000)}}),
        message(
            json!({"role":"hookMessage","content":[{"type":"text","text":"abcde"},{"type":"image","data":"opaque"}],"timestamp":1000}),
        ),
        message(json!({"role":"custom","content":"abcdef","timestamp":1000})),
        message(
            json!({"role":"compactionSummary","summary":"abcd","images":[{"type":"image"},{"type":"image"}],"timestamp":1000}),
        ),
        message(
            json!({"role":"fileMention","files":[{"path":"x.png","image":{"type":"image","data":"opaque"}}],"timestamp":1000}),
        ),
    ]);
    let before = source.clone();
    let mapped = raw_entries(&source, &model(), &HostReductionPolicy::default()).unwrap();
    assert_eq!(
        mapped.iter().map(|entry| (entry.raw_message_tokens, entry.shake_entry_tokens)).collect::<Vec<_>>(),
        [(1, 1), (3, 3), (0, 2), (0, 0), (1202, 1202), (0, 0), (10049, 10049), (0, 0)]
    );
    assert!(matches!(mapped[2].kind, ReductionEntryKind::CustomMessage { .. }));
    assert_eq!(source, before, "raw opaque data is never round-tripped through model types");
    let content = "file content\n".repeat(200);
    let reads =
        snapshot(vec![call("old", "f.ts"), result("old", &content), call("new", "f.ts"), result("new", &content)]);
    let config = SupersedePruneConfig { idle_flush_ms: PRUNE_IDLE_FLUSH_MS, ..SupersedePruneConfig::default() };
    for (prefix_binding, idle, boundary, expected) in
        [(false, false, None, 1), (true, false, None, 0), (true, true, None, 1), (false, true, Some("e2"), 0)]
    {
        let policy = HostReductionPolicy {
            prefix_binding,
            keep_boundary_id: boundary.map(str::to_owned),
            ..HostReductionPolicy::default()
        };
        let now = if idle { 1000.0 + PRUNE_IDLE_FLUSH_MS } else { 2000.0 };
        let prepared = prepare_stale(reads.clone(), &model(), &config, true, &policy, now, 50).unwrap();
        assert_eq!(prepared.edits.len(), expected);
        assert_eq!(prepared.token_estimation, TokenEstimation::ApproximateUtf8Fragments);
    }
    let plan_reads = snapshot(vec![
        call("alias", "local:/PLAN.md:raw"),
        result("alias", &content),
        call("plan", "local://my-plan.md:1-10"),
        result("plan", &content),
        call("file", "normal.ts"),
        result("file", &content),
    ]);
    let policy = HostReductionPolicy {
        protected_read_paths: vec!["local:/my-plan.md".into()],
        ..HostReductionPolicy::default()
    };
    let config = PruneConfig { protect_tokens: 0.0, minimum_savings: 0.0, ..PruneConfig::default() };
    let prepared = prepare_prune(plan_reads, &model(), &config, &policy, 50).unwrap();
    assert_eq!(prepared.edits.iter().map(|edit| edit.entry_id.as_str()).collect::<Vec<_>>(), ["e5"]);
    let mut pruned = reads;
    pruned.entries[3].raw["message"]["prunedAt"] = Value::Null;
    assert!(
        prepare_stale(
            pruned,
            &model(),
            &SupersedePruneConfig::default(),
            true,
            &HostReductionPolicy::default(),
            2000.0,
            50
        )
        .unwrap()
        .edits
        .is_empty()
    );
    let mut exact_model = model();
    exact_model.tokenizer = Some(ModelTokenizer::ClaudeV3);
    let exact = raw_entries(
        &snapshot(vec![message(json!({"role":"user","content":"ξ","timestamp":1000}))]),
        &exact_model,
        &HostReductionPolicy::default(),
    )
    .unwrap();
    assert_eq!(exact[0].raw_message_tokens, 3);
}

#[tokio::test]
async fn native_artifact_and_anchor_family() {
    let text = format!("😀 head\n{}\nmiddle\n{}\ntail", fence(), fence());
    let mut tool = result("read", &"original result\n".repeat(200));
    tool["message"]["content"]
        .as_array_mut()
        .unwrap()
        .extend([json!({"type":"image","data":"opaque"}), json!({"type":"text","text":""})]);
    let source = snapshot(vec![
        tool,
        message(json!({"role":"assistant","content":[{"type":"text","text":text}],"timestamp":1000})),
    ]);
    let config = ShakeConfig::rescue();
    let selected = model();
    for storage in ["memory", "persistent", "write-failure"] {
        let temporary = tempfile::tempdir().unwrap();
        let journal = if storage == "memory" {
            SessionJournal::in_memory(json!({"type":"session","version":3,"id":"fixture"})).unwrap()
        } else {
            SessionJournal::create(temporary.path(), temporary.path()).unwrap()
        };
        let store = SessionArtifacts::for_journal(&journal);
        if storage == "write-failure" {
            std::fs::write(store.directory().unwrap(), b"blocked directory").unwrap();
        }
        let prepared = prepare_shake(source.clone(), &selected, &config, &HostReductionPolicy::default(), &store, 55)
            .await
            .unwrap();
        assert_eq!(prepared.snapshot, source);
        assert_eq!((prepared.plan.counts.tool_results_dropped, prepared.plan.counts.blocks_dropped), (1, 2));
        assert_eq!(prepared.entry_tokens_freed.len(), 2);
        assert!(prepared.entry_tokens_freed.iter().all(|(_, tokens)| *tokens > 0));
        assert_eq!(prepared.entry_tokens_freed.iter().map(|(_, tokens)| tokens).sum::<usize>(), prepared.tokens_freed);
        let replacements: Vec<&str> = prepared
            .plan
            .edits
            .iter()
            .map(|edit| match &edit.action {
                ReductionAction::ElideToolResultText { text, .. } => text.as_str(),
                ReductionAction::ReplaceTextRange { replacement, .. } => replacement.as_str(),
                _ => unreachable!(),
            })
            .collect();
        if storage == "write-failure" {
            assert!(prepared.artifact_id.is_none());
            assert!(prepared.artifact_error.is_some());
            assert!(replacements.iter().all(|replacement| !replacement.contains("recover:")));
        } else {
            assert_eq!(prepared.artifact_id.as_deref(), Some("0"));
            assert!(prepared.artifact_error.is_none());
            assert!(replacements.iter().all(|replacement| replacement.contains("artifact://0")));
            let router = ArtifactUriRouter::new(store.clone(), None);
            if storage == "memory" {
                let error = router.read("artifact://0", CancellationToken::new()).await.unwrap_err();
                assert!(
                    error.0.contains("No session"),
                    "fixed native resolver requires a persisted artifact directory"
                );
            } else {
                let recovered = router.read("artifact://0", CancellationToken::new()).await.unwrap();
                let expected = prepared
                    .regions
                    .iter()
                    .enumerate()
                    .map(|(index, region)| {
                        format!(
                            "### region {} ({}, ~{} tok)\n\n{}\n",
                            index + 1,
                            region.label,
                            region.tokens,
                            region.original_text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                assert_eq!(recovered.content, expected);
            }
            let next = store.save("next artifact", "test").await.unwrap();
            assert_eq!(next, "1", "one artifact per whole region batch");
        }
        for (core, session) in prepared.plan.edits.iter().zip(&prepared.edits) {
            if let (
                ReductionAction::ReplaceTextRange { expected_text: a, .. },
                SessionReductionAction::ReplaceTextRange { expected_text: b, .. },
            ) = (&core.action, &session.action)
            {
                assert!(Arc::ptr_eq(a, b), "Session edit shares the exact Core original-text allocation");
            }
        }
    }
}

#[test]
fn raw_images_thinking_and_opaque_persistence_family() {
    let image = json!({"type":"image","data":"iVBORw0KGgo","mimeType":"image/png","opaqueImage":"kept if unselected"});
    let source = snapshot(vec![
        message(json!({"role":"user","content":[image.clone()],"timestamp":1000,"unknownMessage":"keep"})),
        message(
            json!({"role":"toolResult","toolCallId":"tool","toolName":"generate_image","content":[{"type":"text","text":"generated","opaqueText":"keep"},image.clone()],
            "details":{"images":[image.clone(),{"type":"text","text":42,"opaque":"keep"}],"imageCount":2,"opaqueDetails":"keep"},"isError":false,"timestamp":1000}),
        ),
        message(
            json!({"role":"fileMention","files":[{"path":"a.png","content":"","image":image.clone(),"byteSize":1024}],"timestamp":1000}),
        ),
        message(
            json!({"role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"redactedThinking","data":"opaque"},
            {"type":"toolCall","id":"tool","name":"generate_image","arguments":{},"opaqueCall":"keep"},image.clone()],"timestamp":1000,"opaqueAssistant":"keep"}),
        ),
        json!({"type":"custom_message","customType":"note","content":[image],"display":true,"opaqueCustom":"keep"}),
        json!({"type":"custom","customType":"future","data":{"opaque":[1,2,3]}}),
    ]);
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("fixture.jsonl");
    let header =
        json!({"type":"session","version":3,"id":"fixture","timestamp":"2026-10-02T00:00:00Z","cwd":temporary.path()});
    let data = std::iter::once(header)
        .chain(source.entries.iter().map(|entry| entry.raw.clone()))
        .map(|value| serde_json::to_string(&value).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, data).unwrap();
    let mut journal = SessionJournal::open(&path).unwrap();
    let pinned = journal.raw_reduction_snapshot().unwrap();
    let images = prepare_images(pinned.clone(), &model()).unwrap();
    assert_eq!(images.plan.counts.images_dropped, 5);
    journal.commit_reduction(&images.snapshot, &images.edits).unwrap();
    let thinking = prepare_thinking(journal.raw_reduction_snapshot().unwrap(), &model()).unwrap();
    assert_eq!(thinking.plan.counts.thinking_blocks_dropped, 2);
    journal.commit_reduction(&thinking.snapshot, &thinking.edits).unwrap();
    let reopened = SessionJournal::open(&path).unwrap().raw_reduction_snapshot().unwrap();
    assert_eq!(
        reopened.entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(),
        pinned.entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>()
    );
    let raw: Vec<&Value> = reopened.entries.iter().map(|entry| &entry.raw).collect();
    assert_eq!(raw[0]["message"]["content"], json!([{"type":"text","text":"[image removed]"}]));
    assert_eq!(raw[0]["message"]["unknownMessage"], "keep");
    assert_eq!(raw[1]["message"]["content"][0]["opaqueText"], "keep");
    assert_eq!(raw[1]["message"]["details"]["images"], json!([{"type":"text","text":42,"opaque":"keep"}]));
    assert_eq!(raw[1]["message"]["details"]["imageCount"], 2);
    assert!(raw[2]["message"]["files"][0].get("image").is_none());
    assert_eq!(raw[2]["message"]["files"][0]["byteSize"], 1024);
    assert_eq!(raw[3]["message"]["content"][0]["opaqueCall"], "keep");
    assert_eq!(raw[3]["message"]["content"][1]["opaqueImage"], "kept if unselected");
    assert_eq!(raw[4]["content"], json!([{"type":"text","text":"[image removed]"}]));
    assert_eq!(raw[5], &pinned.entries[5].raw);
    assert_eq!(images.tokens_freed, 0);
    assert_eq!(thinking.tokens_freed, 0);
}
