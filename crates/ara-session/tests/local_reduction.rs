//! One module family for native raw maintenance and checked persistence.
use std::{fs, path::Path, sync::Arc};

use ara_ai::{AssistantBlock, AssistantMessage, Message, UserMessage};
use ara_session::{
    LoopGuardNotice, SessionJournal, SessionReductionAction as Action, SessionReductionEdit as Edit,
    SessionReductionError, SessionReductionSlot as Slot,
};
use serde_json::{Value, json};

fn row(kind: &str, id: &str, parent: Value, fields: Value) -> Value {
    let mut raw = json!({"type":kind,"id":id,"parentId":parent,"timestamp":"2026-10-02T00:00:00.000Z","opaqueEntry":{"keep":true}});
    raw.as_object_mut().unwrap().extend(fields.as_object().unwrap().clone());
    raw
}

fn message(id: &str, parent: &str, message: Value) -> Value {
    row("message", id, if parent.is_empty() { Value::Null } else { json!(parent) }, json!({"message":message}))
}

fn write_rows(path: &Path, header: &Value, rows: &[Value]) {
    let body = std::iter::once(header).chain(rows).map(|row| format!("{row}\n")).collect::<String>();
    fs::write(path, body).unwrap();
}

fn header() -> Value {
    json!({"type":"session","version":3,"id":"reduction-family","timestamp":"2026-10-02T00:00:00.000Z","cwd":"/"})
}

fn edit(id: &str, action: Action) -> Edit {
    Edit { entry_id: id.into(), action }
}

fn range(
    id: &str,
    slot: Slot,
    block: Option<usize>,
    text: &Arc<str>,
    start: usize,
    end: usize,
    replacement: &str,
) -> Edit {
    edit(
        id,
        Action::ReplaceTextRange {
            slot,
            block_index: block,
            start_utf16: text[..start].encode_utf16().count(),
            end_utf16: text[..end].encode_utf16().count(),
            expected_text: text.clone(),
            replacement: replacement.into(),
        },
    )
}

fn raw<'a>(journal: &'a SessionJournal, id: &str) -> &'a Value {
    &journal.entries().iter().find(|entry| entry.id == id).unwrap().raw
}

#[test]
fn raw_slots_full_branch_and_provider_projections_reopen_without_losing_receipts() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("raw.jsonl");
    let image = json!({"type":"image","data":"eA==","mimeType":"image/png","opaqueBlock":"keep"});
    let text: Arc<str> = "😀before\n```rust\nfirst\n```\nbetween\n<x>\nsecond\n</x>\nafter".into();
    let mut assistant = serde_json::to_value(AssistantMessage::empty("openai-completions", "fixture", "m")).unwrap();
    assistant["role"] = json!("assistant");
    assistant["content"] = json!([
        {"type":"thinking","thinking":"private","opaqueBlock":"keep"},
        {"type":"redactedThinking","data":"opaque"}, {"type":"text","text":"answer","textSignature":"keep"}, image,
    ]);
    assistant["usage"]["input"] = json!(200);
    let tool = |content: Value| {
        json!({"role":"toolResult","toolCallId":"call","toolName":"bash","isError":false,"timestamp":1,
        "content":content,"useless":false,"opaqueMessage":{"keep":true},"details":{"images":[image, null, {"opaque":true}],"keep":17}})
    };
    let rows = vec![
        message("user", "", json!({"role":"user","content":[image],"timestamp":1})),
        message("assistant", "user", assistant),
        message("prune", "assistant", tool(json!([{ "type":"text","text":"full" }, image]))),
        message(
            "elide",
            "prune",
            tool(json!([
                {"type":"text","text":"","opaque":"discard"}, image,
                {"type":"text","text":"first","textSignature":"discard"}, {"type":"text","text":"last"},
            ])),
        ),
        row(
            "custom_message",
            "custom",
            json!("elide"),
            json!({"customType":"host-note","content":[
            {"type":"text","text":text.as_ref(),"textSignature":"keep"}, image],"display":true,"attribution":"agent","details":{"keep":true}}),
        ),
        row(
            "custom_message",
            "steer",
            json!("custom"),
            json!({"customType":"collab-prompt","content":text.as_ref(),"display":false,"attribution":"user"}),
        ),
        message(
            "file",
            "steer",
            json!({"role":"fileMention","files":[{"path":"keep","image":image,"opaque":23},
            {"path":"notes.md","content":"original text"}],"timestamp":1}),
        ),
        message(
            "summary",
            "file",
            json!({"role":"compactionSummary","summary":"keep","tokensBefore":99,"images":[image],"timestamp":1}),
        ),
        message("offbranch", "user", json!({"role":"user","content":[image],"timestamp":1})),
        row("reset_boundary", "reset", json!("summary"), json!({})),
        message("current", "reset", json!({"role":"user","content":"current request","timestamp":1})),
    ];
    write_rows(&path, &header(), &rows);
    let mut journal = SessionJournal::open(&path).unwrap();
    let snapshot = journal.raw_reduction_snapshot().unwrap();
    assert!(snapshot.entries.iter().any(|entry| entry.id == "user"), "image shake scans before reset");
    assert!(!snapshot.entries.iter().any(|entry| entry.id == "offbranch"));
    let original_file = journal.entries().iter().find(|entry| entry.id == "file").unwrap();
    let fragments = original_file.model_messages().unwrap();
    assert!(matches!(fragments.as_slice(), [Message::Developer(_), Message::User(_)]));
    let first_start = text.find("```rust").unwrap();
    let first_end = text.find("```\nbetween").unwrap() + 3;
    let second_start = text.find("<x>").unwrap();
    let second_end = text.find("</x>").unwrap() + 4;
    let edits = vec![
        edit("user", Action::DropImages { slot: Slot::MessageContent, indexes: vec![0], placeholder_if_empty: true }),
        edit("assistant", Action::DropBlocks { slot: Slot::MessageContent, indexes: vec![0, 1] }),
        edit(
            "assistant",
            Action::RecordAnchoredHistoryRewrite {
                tokens_removed: 100,
                snapshot_if_missing: Some(
                    json!({"promptTokens":200,"nonMessageTokens":20,"compactionEpoch":0,"opaqueSnapshot":"keep"}),
                ),
            },
        ),
        edit("prune", Action::ReplaceToolResultContent { text: "[Output truncated]".into(), pruned_at_ms: 7 }),
        edit("elide", Action::ElideToolResultText { text: "[shaken ~800 tokens]".into(), pruned_at_ms: 8 }),
        edit(
            "elide",
            Action::DropImages { slot: Slot::ToolResultDetailsImages, indexes: vec![0], placeholder_if_empty: false },
        ),
        range("custom", Slot::CustomContent, Some(0), &text, first_start, first_end, "[shaken ~400 tokens]"),
        range("custom", Slot::CustomContent, Some(0), &text, second_start, second_end, "[shaken ~500 tokens]"),
        range("steer", Slot::CustomContent, None, &text, second_start, second_end, "[shaken ~500 tokens]"),
        edit(
            "file",
            Action::DropImages { slot: Slot::FileMentionImage(0), indexes: vec![0], placeholder_if_empty: false },
        ),
    ];
    let receipt = journal.commit_reduction(&snapshot, &edits).unwrap();
    assert_eq!(receipt.changed_entry_ids, ["user", "assistant", "prune", "elide", "custom", "steer", "file"]);
    assert_eq!(receipt.leaf_id.as_deref(), Some("current"));
    for (before, after) in rows.iter().zip(journal.entries()) {
        assert_eq!(before["id"], after.raw["id"]);
        assert_eq!(before["parentId"], after.raw["parentId"]);
        assert_eq!(before["opaqueEntry"], after.raw["opaqueEntry"]);
    }
    assert_eq!(raw(&journal, "user")["message"]["content"], json!([{"type":"text","text":"[image removed]"}]));
    assert_eq!(
        raw(&journal, "assistant")["message"]["content"],
        json!([{ "type":"text","text":"answer","textSignature":"keep" },image])
    );
    assert_eq!(raw(&journal, "prune")["message"]["details"], rows[2]["message"]["details"]);
    assert_eq!(
        raw(&journal, "elide")["message"]["content"],
        json!([image,{"type":"text","text":"[shaken ~800 tokens]"}])
    );
    assert_eq!(raw(&journal, "elide")["message"]["details"]["images"], json!([null,{"opaque":true}]));
    assert_eq!(
        raw(&journal, "file")["message"]["files"],
        json!([{"path":"keep","opaque":23},{"path":"notes.md","content":"original text"}])
    );
    assert_eq!(raw(&journal, "assistant")["message"]["usage"], rows[1]["message"]["usage"]);
    assert_eq!(
        raw(&journal, "assistant")["message"]["contextSnapshot"],
        json!({"promptTokens":200,"nonMessageTokens":20,
        "compactionEpoch":0,"opaqueSnapshot":"keep","historyRewriteTokensRemoved":100})
    );
    let file = journal.entries().iter().find(|entry| entry.id == "file").unwrap();
    let Some(Message::Developer(file_context)) = file.message() else {
        panic!("shaken file mentions move to Developer");
    };
    assert_eq!(
        file_context.content.plain_text(),
        "<file path=\"keep\">\n</file>\n<file path=\"notes.md\">\noriginal text\n</file>"
    );
    let mut empty = file.clone();
    empty.raw["message"]["files"] = json!([]);
    assert_eq!(empty.model_messages(), Some(Vec::new()));
    assert_eq!(raw(&journal, "summary"), &rows[7], "native image shake leaves legacy summaries untouched");
    assert_eq!(raw(&journal, "offbranch"), &rows[8]);
    let custom = journal.entries().iter().find(|entry| entry.id == "custom").unwrap();
    assert_eq!(custom.raw["content"][0]["textSignature"], "keep");
    assert!(!custom.raw["content"][0]["text"].as_str().unwrap().contains("Images attached"));
    assert_eq!(custom.model_messages().unwrap().len(), 2);
    let steering = journal.entries().iter().find(|entry| entry.id == "steer").unwrap();
    assert!(!steering.raw["content"].as_str().unwrap().contains("User interjection"));
    let Some(Message::User(projected)) = steering.message() else {
        panic!("steering retains its user projection");
    };
    assert!(projected.content.plain_text().contains("User interjection"));
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries(), journal.entries());
    assert_eq!(reopened.raw_reduction_snapshot().unwrap(), journal.raw_reduction_snapshot().unwrap());
    assert_eq!(reopened.model_context(), journal.model_context());
    let second = journal.raw_reduction_snapshot().unwrap();
    journal
        .commit_reduction(
            &second,
            &[edit(
                "assistant",
                Action::RecordAnchoredHistoryRewrite { tokens_removed: 60, snapshot_if_missing: None },
            )],
        )
        .unwrap();
    let twice = SessionJournal::open(&path).unwrap();
    assert_eq!(raw(&twice, "assistant")["message"]["contextSnapshot"]["historyRewriteTokensRemoved"], 160);
    assert_eq!(raw(&twice, "assistant")["message"]["contextSnapshot"]["opaqueSnapshot"], "keep");
    let mut active_file = raw(&twice, "file").clone();
    active_file["parentId"] = Value::Null;
    let active_path = directory.path().join("active-file.jsonl");
    write_rows(&active_path, &header(), &[active_file]);
    let active = SessionJournal::open(&active_path).unwrap();
    assert_eq!(
        active.native_projected_compaction_snapshot().unwrap().entries[0].origin,
        ara_session::NativeEntryOrigin::FileMention
    );

    // Existing already-pruned native receipts are normalized only when sent.
    for (content, expected) in [
        (
            json!([image,{"type":"text","text":"a"},{"type":"text","text":"b"}]),
            json!([image,{"type":"text","text":"ab"}]),
        ),
        (json!([image]), json!([{"type":"text","text":"[Output truncated]"},image])),
    ] {
        let mut result = tool(content.clone());
        result["prunedAt"] = Value::Null;
        let mut entry = journal.entries().iter().find(|entry| entry.id == "elide").unwrap().clone();
        entry.raw["message"] = result;
        let Some(Message::ToolResult(projected)) = entry.message() else {
            panic!("pruned tool result");
        };
        let mut actual = serde_json::to_value(projected.content).unwrap();
        for block in actual.as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("opaqueBlock");
        }
        let mut expected = expected;
        for block in expected.as_array_mut().unwrap() {
            block.as_object_mut().unwrap().remove("opaqueBlock");
        }
        assert_eq!(actual, expected);
        assert_eq!(entry.raw["message"]["content"], content);
    }

    // Tiered local rescue keeps the exact failed receipt off branch. The
    // first rewrite and its durable selected leaf publish atomically; the
    // second layer reuses that marker and rebases only original raw slots.
    let mut rescue = SessionJournal::create(&directory.path().join("rescue"), directory.path()).unwrap();
    let request = rescue.append_message(&Message::User(UserMessage::text("before"))).unwrap();
    let mut completed = AssistantMessage::empty("openai-completions", "fixture", "m");
    completed.content.push(AssistantBlock::Thinking(ara_ai::ThinkingContent {
        thinking: "old thought".into(),
        thinking_signature: None,
    }));
    completed.content.push(AssistantBlock::text("settled"));
    let settled = rescue.append_message(&Message::Assistant(completed)).unwrap();
    let mut failure = AssistantMessage::empty("openai-completions", "fixture", "m");
    failure.stop_reason = ara_ai::StopReason::Error;
    failure.content.push(AssistantBlock::text("failed output must not revive"));
    let failed = rescue.append_message(&Message::Assistant(failure.clone())).unwrap();
    let original_failed = raw(&rescue, &failed).clone();
    let mut owner = rescue.begin_failed_assistant_recovery(&failed, &failed, &failure).unwrap();
    let source = rescue.raw_reduction_snapshot().unwrap();
    let before: Arc<str> = "before".into();
    let receipt = rescue
        .commit_reduction_during_recovery(
            &source,
            &[range(&request, Slot::MessageContent, None, &before, 0, 6, "after")],
            &mut owner,
        )
        .unwrap();
    assert_eq!(receipt.changed_entry_ids.as_slice(), std::slice::from_ref(&request));
    assert!(owner.local_history_rewritten());
    let marker = receipt.leaf_id.unwrap();
    assert_eq!(raw(&rescue, &failed), &original_failed);
    let restarted = SessionJournal::open(rescue.path()).unwrap();
    assert_eq!(restarted.leaf_id(), Some(marker.as_str()));
    assert_eq!(restarted.model_context(), rescue.model_context());
    assert!(!restarted.branch().iter().any(|entry| entry.id == failed));
    let source = rescue.raw_reduction_snapshot().unwrap();
    let again = rescue
        .commit_reduction_during_recovery(
            &source,
            &[edit(&settled, Action::DropBlocks { slot: Slot::MessageContent, indexes: vec![0] })],
            &mut owner,
        )
        .unwrap();
    assert_eq!(again.leaf_id.as_deref(), Some(marker.as_str()));
    assert_eq!(
        rescue
            .entries()
            .iter()
            .filter(|entry| entry.raw.pointer("/details/discardedEntryId") == Some(&json!(failed)))
            .count(),
        1
    );
    assert_eq!(
        rescue.finish_failed_assistant_recovery(owner, true).unwrap(),
        ara_session::FailedAssistantRecoveryOutcome::Committed
    );
    assert_eq!(SessionJournal::open(rescue.path()).unwrap().entries(), rescue.entries());
}

#[test]
fn checked_invalid_or_stale_plans_leave_file_and_memory_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let id = journal.append_message(&Message::User(UserMessage::text("😀source"))).unwrap();
    let mut assistant = AssistantMessage::empty("openai-completions", "fixture", "m");
    assistant.content.push(AssistantBlock::text("done"));
    journal.append_message(&Message::Assistant(assistant)).unwrap();
    let snapshot = journal.raw_reduction_snapshot().unwrap();
    let text: Arc<str> = "😀source".into();
    let valid = range(&id, Slot::MessageContent, None, &text, 0, text.len(), "replacement");
    let mut half_surrogate = valid.clone();
    if let Action::ReplaceTextRange { start_utf16, .. } = &mut half_surrogate.action {
        *start_utf16 = 1;
    }
    let invalid = [
        vec![edit("foreign", valid.action.clone())],
        vec![half_surrogate],
        vec![valid.clone(), valid.clone()],
        vec![edit(&id, Action::DropBlocks { slot: Slot::MessageContent, indexes: vec![0] })],
        vec![edit(&id, Action::RecordAnchoredHistoryRewrite { tokens_removed: 1, snapshot_if_missing: None })],
    ];
    let bytes = fs::read(journal.path()).unwrap();
    let entries = journal.entries().to_vec();
    for edits in invalid {
        assert!(matches!(journal.commit_reduction(&snapshot, &edits), Err(SessionReductionError::InvalidEdit { .. })));
        assert_eq!(journal.entries(), entries);
        assert_eq!(fs::read(journal.path()).unwrap(), bytes);
    }
    journal.append_model_change("fixture/model").unwrap();
    let after_append = fs::read(journal.path()).unwrap();
    assert!(matches!(journal.commit_reduction(&snapshot, &[valid]), Err(SessionReductionError::StaleSnapshot)));
    assert_eq!(fs::read(journal.path()).unwrap(), after_append);
}

#[test]
fn reserved_notice_reduction_reopens_public_views_without_resurrection_and_rejects_tampering() {
    let directory = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(directory.path(), directory.path()).unwrap();
    let notices = [
        LoopGuardNotice::thinking_loop(),
        LoopGuardNotice::gemini_headers(36),
        LoopGuardNotice::tool_call_loop(&ara_ai::tool_call_loop_guard::RepeatedToolCallDetection {
            kind: "repeated_tool_call",
            tool_name: "bash".into(),
            count: 2.0,
            arguments_summary: "😀same".into(),
            result_summary: "same".into(),
        })
        .unwrap(),
    ];
    let ids: Vec<_> = notices.iter().map(|notice| journal.append_loop_guard_notice(notice).unwrap()).collect();
    journal.materialize().unwrap();
    let snapshot = journal.raw_reduction_snapshot().unwrap();
    let edits: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let text: Arc<str> = raw(&journal, id)["content"].as_str().unwrap().into();
            let end = text.find("</system-interrupt>").unwrap() + "</system-interrupt>".len();
            range(
                id,
                Slot::CustomContent,
                None,
                &text,
                0,
                end,
                &format!("[shaken ~800 tokens — recover: artifact://0 (region {})]", index + 1),
            )
        })
        .collect();
    journal.commit_reduction(&snapshot, &edits).unwrap();
    let reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(journal.entries(), reopened.entries());
    for id in &ids {
        let entry = reopened.entries().iter().find(|entry| &entry.id == id).unwrap();
        let actual = entry.raw["content"].as_str().unwrap();
        assert!(actual.starts_with("[shaken ~800 tokens"));
        let notice = entry.loop_guard_notice().unwrap();
        assert_eq!(notice.event_message()["content"], actual);
        let Message::Developer(model) = notice.model_message() else {
            panic!("trusted reduced notice");
        };
        assert_eq!(model.content.plain_text(), actual);
        assert_eq!(entry.message(), Some(notice.model_message()));
        assert!(notice.event_message().get("araNoticeReduction").is_none(), "original stays outside public view");
        assert!(
            LoopGuardNotice::from_event_message(&notice.event_message()).is_none(),
            "live input stays original-native-only"
        );
    }
    assert_eq!(reopened.undecodable_messages(), 0);
    assert!(reopened.native_projected_compaction_snapshot().is_ok());
    let restored_notice = reopened.entries()[0].loop_guard_notice().unwrap();
    let copy = journal.append_loop_guard_notice(&restored_notice).unwrap();
    let again = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(raw(&again, &copy)["content"], raw(&reopened, &ids[0])["content"]);
    assert!(again.entries().last().unwrap().loop_guard_notice().is_some());

    let rows: Vec<_> = reopened.entries().iter().map(|entry| entry.raw.clone()).collect();
    let mut tampered = Vec::new();
    let mut missing = rows[0].clone();
    missing.as_object_mut().unwrap().remove("araNoticeReduction");
    tampered.push(missing);
    let mut version = rows[0].clone();
    version["araNoticeReduction"]["version"] = json!(2);
    tampered.push(version);
    let mut original = rows[0].clone();
    original["araNoticeReduction"]["originalContent"] = json!("arbitrary captured text");
    tampered.push(original);
    let mut current = rows[0].clone();
    current["content"] = json!("arbitrary current text");
    tampered.push(current);
    let mut partial = rows[0].clone();
    partial["araNoticeReduction"]["rounds"][0][0]["startUtf16"] = json!(1);
    tampered.push(partial);
    let mut replacement = rows[0].clone();
    replacement["araNoticeReduction"]["rounds"][0][0]["replacement"] = json!("untrusted developer instruction");
    tampered.push(replacement);
    let mut url = rows[0].clone();
    url["araNoticeReduction"]["rounds"][0][0]["replacement"] =
        json!("[shaken ~800 tokens — recover: https://example.invalid (region 1)]");
    tampered.push(url);
    for damaged in tampered {
        let mut rows = rows.clone();
        rows[0] = damaged;
        let path = directory.path().join("tampered.jsonl");
        write_rows(&path, reopened.header(), &rows);
        let damaged = SessionJournal::open(&path).unwrap();
        assert!(damaged.entries()[0].loop_guard_notice().is_none());
        assert!(damaged.entries()[0].message().is_none());
        assert!(damaged.native_projected_compaction_snapshot().is_err());
    }
}
