//! Fixed OMP session/session-context.test.ts original three archive modes,
//! plus their production message timeline contracts (596f2da, MIT).
use ara_ai::{AssistantBlock, AssistantMessage, Message};
use ara_rpc::WireValue as W;
use ara_session::{
    SessionJournal,
    context_view::{SummaryViewPolicy, TranscriptViewMode, TranscriptViewOptions, compaction_summary_dto},
};
use serde_json::{Value, json};
use std::fs;

const TIMESTAMP: &str = "2026-07-09T00:00:00.000Z";
fn row(kind: &str, id: &str, parent: Value, extra: Value) -> Value {
    let mut value = json!({"type":kind,"id":id,"parentId":parent,"timestamp":TIMESTAMP});
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    value
}
fn user(id: &str, parent: Value, text: &str) -> Value {
    row(
        "message",
        id,
        parent,
        json!({"message":{"role":"user","content":[{"type":"text","text":text}],"timestamp":if id == "m2" { 2 } else { 1 }}}),
    )
}
fn assistant(id: &str, parent: Value, model: &str, content: Value) -> Value {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", model);
    message.content.push(AssistantBlock::text("placeholder"));
    let mut body = serde_json::to_value(Message::Assistant(message)).unwrap();
    body["content"] = content;
    row("message", id, parent, json!({"message":body}))
}
fn compact(id: &str, parent: Value, kept: &str, data: &str) -> Value {
    row(
        "compaction",
        id,
        parent,
        json!({"summary":"summary","firstKeptEntryId":kept,"tokensBefore":123,
    "shortSummary":"brief","tokensAfter":45,"warning":"ACTIVE warning","method":"snapcompact",
    "preserveData":{"snapcompact":{"frames":[{"data":data,"mimeType":"image/png","cols":10,"rows":10,"chars":100}],
    "totalChars":100,"truncatedChars":0,"textHead":"head","textTail":"tail"}}}),
    )
}
fn open(entries: &[Value]) -> (tempfile::TempDir, SessionJournal) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fixture.jsonl");
    let header = json!({"type":"session","version":3,"id":"view-fixture","cwd":"/","timestamp":TIMESTAMP});
    let mut text = format!("{header}\n");
    for row in entries {
        text.push_str(&format!("{row}\n"));
    }
    fs::write(&path, text).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    (dir, journal)
}
fn options(mode: TranscriptViewMode, keep: bool) -> TranscriptViewOptions {
    TranscriptViewOptions { mode, keep_dangling_tool_calls: keep }
}
fn ids(view: &ara_session::context_view::TranscriptView) -> Vec<&str> {
    view.messages.iter().map(|message| message.entry_id.as_str()).collect()
}
fn scalar(body: &W) -> Value {
    ara_session::native_checked_serde(body).unwrap()
}

#[test]
fn fixed_session_context_three_mode_views_and_timeline_family() {
    // Exact original compactedEntries + original observations, not a model
    // converter substituted for a display transcript.
    let mut original = compact("c1", json!("m1"), "m1", "base64-frame");
    for field in ["shortSummary", "tokensAfter", "warning", "method"] {
        original.as_object_mut().unwrap().remove(field);
    }
    let (dir, journal) =
        open(&[user("m1", Value::Null, "before compaction"), original, user("m2", json!("c1"), "after compaction")]);
    let bytes = fs::read(journal.path()).unwrap();
    let raw = journal.entries().to_vec();
    let full = journal.transcript_view(options(TranscriptViewMode::Full, false)).unwrap();
    let collapsed = journal.transcript_view(options(TranscriptViewMode::Collapsed, false)).unwrap();
    assert_eq!(ids(&full), ["m1", "c1", "m2"]);
    assert_eq!(ids(&collapsed), ["m1", "c1", "m2"]);
    for view in [&full, &collapsed] {
        for (index, text, timestamp) in [(0, "before compaction", 1), (2, "after compaction", 2)] {
            assert_eq!(
                scalar(&view.messages[index].body),
                json!({
                    "role":"user", "content":[{"type":"text","text":text}], "timestamp":timestamp
                }),
                "whole original user body, not only its ID"
            );
        }
    }
    let entry = journal.entries().iter().find(|entry| entry.id == "c1").unwrap();
    let provider = compaction_summary_dto(entry, SummaryViewPolicy::Provider, true, None).unwrap();
    for (mode, body) in [("full", &full.messages[1].body), ("provider", &provider)] {
        let dto = scalar(body);
        assert_eq!(dto["timestamp"], 1783555200000_i64);
        assert_eq!(dto["tokensBefore"], 123);
        assert_eq!(dto["summary"], "summary");
        assert_eq!(
            dto["images"].as_array().unwrap().iter().map(|image| image["data"].as_str().unwrap()).collect::<Vec<_>>(),
            ["base64-frame"],
            "{mode}"
        );
        assert_eq!(
            dto["blocks"].as_array().unwrap().iter().map(|block| block["type"].as_str().unwrap()).collect::<Vec<_>>(),
            ["text", "image", "text"],
            "{mode}"
        );
        assert!(dto["images"][0].get("compactionFrame").is_none());
        assert!(dto.get("method").is_none(), "fixed optional method stays absent");
        assert!(dto.get("preserveData").is_none());
    }
    let dto = scalar(&collapsed.messages[1].body);
    assert!(dto.get("images").is_none());
    assert!(dto.get("blocks").is_none());
    assert_eq!(full.cache_miss_explained_at.len(), full.messages.len());
    assert_eq!(collapsed.cache_miss_explained_at.len(), collapsed.messages.len());
    assert_eq!(journal.entries(), raw);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    drop(dir);

    let orphan = json!({"type":"toolCall","id":"orphan","name":"bash","arguments":{"command":"sleep 60"}});
    let rows = vec![
        user("m1", Value::Null, "old input"),
        assistant(
            "a1",
            json!("m1"),
            "m",
            json!([
            {"type":"thinking","thinking":"visible reasoning","thinkingSignature":"signed"},
            {"type":"redactedThinking","data":"encrypted"},orphan.clone(),{"type":"text","text":"visible text"}]),
        ),
        compact("c1", json!("a1"), "m1", "OLD image"),
        user("m2", json!("c1"), "kept input"),
        compact("c2", json!("m2"), "m2", "ACTIVE image"),
        assistant("a2", json!("c2"), "m", json!([orphan])),
    ];
    let (_dir, journal) = open(&rows);
    let bytes = fs::read(journal.path()).unwrap();
    let raw = journal.entries().to_vec();
    let full = journal.transcript_view(options(TranscriptViewMode::Full, false)).unwrap();
    assert_eq!(ids(&full), ["m1", "a1", "c1", "m2", "c2", "a2"]);
    let old = scalar(&full.messages[2].body);
    assert_eq!(old["summary"], "[Superseded compaction summary elided after a newer compaction]");
    assert_eq!(old["shortSummary"], "Superseded compaction elided");
    assert!(old.get("blocks").is_none());
    assert!(old.get("images").is_none());
    let active = scalar(&full.messages[4].body);
    assert_eq!(active["images"][0]["data"], "ACTIVE image");
    for field in ["shortSummary", "tokensBefore", "tokensAfter", "method", "warning", "timestamp"] {
        assert!(active.get(field).is_some(), "{field}");
    }
    let rewritten = scalar(&full.messages[1].body);
    assert_eq!(rewritten["strippedToolCalls"], 1);
    assert_eq!(rewritten["content"].as_array().unwrap().len(), 2);
    assert!(rewritten["content"][0].get("thinkingSignature").is_none());
    let empty = scalar(&full.messages[5].body);
    assert_eq!(empty["content"], json!([]));
    assert_eq!(empty["strippedToolCalls"], 1);
    assert_eq!(full.cache_miss_explained_at, [false, false, false, false, false, true]);
    let collapsed = journal.transcript_view(options(TranscriptViewMode::Collapsed, false)).unwrap();
    assert_eq!(ids(&collapsed), ["m2", "c2", "a2"]);
    assert_eq!(collapsed.cache_miss_explained_at, [false, false, true]);
    assert!(scalar(&collapsed.messages[1].body).get("blocks").is_none());
    let pending = journal.transcript_view(options(TranscriptViewMode::Collapsed, true)).unwrap();
    let empty = scalar(&pending.messages[2].body);
    assert_eq!(empty["content"].as_array().unwrap().len(), 1);
    assert!(empty.get("strippedToolCalls").is_none());
    assert_eq!(journal.entries(), raw);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);

    // Full export ignores /clear emission trimming; collapsed rebuild starts
    // after it. Plan/model transitions explain only the next assistant cache.
    let mut rows = rows;
    rows.extend([
        row("reset_boundary", "reset", json!("a2"), json!({})),
        user("m3", json!("reset"), "new input"),
        row("mode_change", "plan", json!("m3"), json!({"mode":"plan"})),
        assistant("a3", json!("plan"), "new-model", json!([])),
        assistant("a4", json!("a3"), "new-model", json!([])),
    ]);
    let (_dir, journal) = open(&rows);
    let full = journal.transcript_view(options(TranscriptViewMode::Full, false)).unwrap();
    assert_eq!(ids(&full), ["m1", "a1", "c1", "m2", "c2", "a2", "m3", "a3", "a4"]);
    assert_eq!(&full.cache_miss_explained_at[7..], [true, false]);
    let collapsed = journal.transcript_view(options(TranscriptViewMode::Collapsed, false)).unwrap();
    assert_eq!(ids(&collapsed), ["m3", "a3", "a4"]);
    assert_eq!(collapsed.cache_miss_explained_at, [false, true, false]);

    // Exact UTF16 archive data stays exact in the display DTO, and the public
    // scalar boundary refuses it instead of silently replacing code units.
    let (_dir, journal) = open(&[user("m1", Value::Null, "input"), compact("c1", json!("m1"), "m1", "AQI=")]);
    let path = journal.path().to_path_buf();
    let text = fs::read_to_string(&path).unwrap().replace("\"textHead\":\"head\"", "\"textHead\":\"head \\ud800\"");
    fs::write(&path, text).unwrap();
    let journal = SessionJournal::open(&path).unwrap();
    let full = journal.transcript_view(options(TranscriptViewMode::Full, false)).unwrap();
    assert!(full.messages[1].body.stringify().contains("\\ud800"));
    assert!(ara_session::native_checked_serde(&full.messages[1].body).is_err());

    // Fixed custom content admission/normalization belongs to display views,
    // independently of provider UserContent decoding. Empty branches vanish.
    let rows = [
        row(
            "custom_message",
            "valid-custom",
            Value::Null,
            json!({"customType":"","content":"visible custom","display":1,"attribution":"unknown","details":{"marker":"kept"}}),
        ),
        row(
            "custom_message",
            "array-custom",
            json!("valid-custom"),
            json!({"customType":"array","content":[{"unexpected":"retained display array"}],"display":true}),
        ),
        row(
            "custom_message",
            "invalid-custom",
            json!("array-custom"),
            json!({"customType":"invalid","content":{"not":"string or array"},"display":true}),
        ),
        row("branch_summary", "empty-branch", json!("invalid-custom"), json!({"summary":"","fromId":"m1"})),
        row(
            "branch_summary",
            "truthy-branch",
            json!("empty-branch"),
            json!({"summary":"kept branch summary","fromId":"m1"}),
        ),
    ];
    let (_dir, journal) = open(&rows);
    let bytes = fs::read(journal.path()).unwrap();
    let raw = journal.entries().to_vec();
    for mode in [TranscriptViewMode::Full, TranscriptViewMode::Collapsed] {
        let view = journal.transcript_view(options(mode, false)).unwrap();
        assert_eq!(ids(&view), ["valid-custom", "array-custom", "truthy-branch"]);
        let custom = scalar(&view.messages[0].body);
        assert_eq!(custom["role"], "custom");
        assert_eq!(custom["customType"], "custom-message");
        assert_eq!(custom["content"], "visible custom");
        assert_eq!(custom["display"], false);
        assert_eq!(custom["attribution"], "agent");
        assert_eq!(custom["details"]["marker"], "kept");
        let array = scalar(&view.messages[1].body);
        assert_eq!(array["content"], json!([{"unexpected":"retained display array"}]));
        assert_eq!(array["display"], true);
        assert!(array.get("attribution").is_none(), "absent attribution stays absent");
        assert_eq!(scalar(&view.messages[2].body)["role"], "branchSummary");
        assert_eq!(view.cache_miss_explained_at, [false, false, false]);
    }
    assert_eq!(journal.entries(), raw);
    assert_eq!(fs::read(journal.path()).unwrap(), bytes);
}
