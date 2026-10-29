use ara_ai::*;
use ara_session::*;
use serde_json::{Value, json};
use std::fs;

fn assistant(text: &str, calls: &[&str]) -> Message {
    let mut m = AssistantMessage::empty("openai-completions", "fake", "m");
    if !text.is_empty() {
        m.content.push(AssistantBlock::text(text));
    }
    for id in calls {
        m.content.push(AssistantBlock::ToolCall(ToolCall {
            id: id.to_string(),
            name: "bash".into(),
            arguments: JsonObject::new(),
            thought_signature: None,
            ..Default::default()
        }));
    }
    m.stop_reason = if calls.is_empty() { StopReason::Stop } else { StopReason::ToolUse };
    Message::Assistant(m)
}

fn result(id: &str) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: id.into(),
        tool_name: "bash".into(),
        content: vec![UserBlock::text("ok")],
        details: None,
        is_error: false,
        timestamp: 1,
        provider_metadata: None,
        useless: false,
    })
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn model_usage_keeps_initiating_branch_and_nullable_usage_without_chat_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let owner = journal.model_usage_owner();
    let mut usage = ModelUsageRecord {
        purpose: "auto-thinking".into(),
        role: Some("tiny".into()),
        api: "openai-completions".into(),
        provider: "fake".into(),
        model: "classifier".into(),
        usage: Usage { input: Some(9), ..Usage::unknown() },
        stop_reason: StopReason::Error,
        error_message: Some("transient upstream failure".into()),
    };
    let first = journal.append_model_usage(&usage, &owner).unwrap().unwrap();
    assert_eq!(journal.leaf_id(), Some(first.as_str()));
    assert!(!journal.path().exists(), "classifier metadata preserves lazy materialization");
    assert!(journal.build_context().unwrap().is_empty());
    let retry_owner = ModelUsageOwner { parent_id: Some(first.clone()), ..owner.clone() };
    let successor = journal.append_message(&assistant("successor branch", &[])).unwrap();
    let context = journal.build_context().unwrap();
    usage.stop_reason = StopReason::Stop;
    usage.error_message = None;
    let second = journal.append_model_usage(&usage, &retry_owner).unwrap().unwrap();
    assert_eq!(journal.leaf_id(), Some(successor.as_str()));
    assert_eq!(journal.build_context().unwrap(), context);
    let raw = lines(journal.path());
    let receipt = raw.iter().find(|entry| entry["id"] == second).unwrap();
    assert_eq!(receipt["type"], "model_usage");
    assert_eq!(receipt["parentId"], first);
    assert_eq!(receipt["purpose"], "auto-thinking");
    assert_eq!(receipt["role"], "tiny");
    assert_eq!(receipt["stopReason"], "stop");
    assert_eq!(receipt["usage"], json!({"input":9}));
    assert!(receipt.get("errorMessage").is_none());
    assert!(receipt.get("message").is_none());
    let before = fs::read(journal.path()).unwrap();
    for rejected in [
        ModelUsageOwner { session_id: "replacement-session".into(), ..retry_owner.clone() },
        ModelUsageOwner { parent_id: Some("missing-owner".into()), ..owner.clone() },
    ] {
        assert_eq!(journal.append_model_usage(&usage, &rejected).unwrap(), None);
        assert_eq!(fs::read(journal.path()).unwrap(), before);
        assert_eq!(journal.leaf_id(), Some(successor.as_str()));
    }
    let reopened = SessionJournal::open(journal.path()).unwrap();
    // Native reopen selects the last physical entry. Live owner restoration
    // above is deliberately separate from that existing loader contract.
    assert_eq!(reopened.model_usage_records().collect::<Vec<_>>().len(), 2);
}

#[test]
fn model_usage_failed_append_restores_owner_and_successor_before_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let owner = journal.model_usage_owner();
    let successor = journal.append_message(&assistant("materialize", &[])).unwrap();
    let receipt = ModelUsageRecord {
        purpose: "auto-thinking".into(),
        role: None,
        api: "openai-responses".into(),
        provider: "fake".into(),
        model: "classifier".into(),
        usage: Usage::unknown(),
        stop_reason: StopReason::Aborted,
        error_message: None,
    };
    let entries = journal.entries().to_vec();
    let saved = journal.path().with_extension("saved");
    fs::rename(journal.path(), &saved).unwrap();
    fs::create_dir(journal.path()).unwrap();
    assert!(journal.append_model_usage(&receipt, &owner).is_err());
    assert_eq!(journal.entries(), entries);
    assert_eq!(journal.leaf_id(), Some(successor.as_str()));
    fs::remove_dir(journal.path()).unwrap();
    fs::rename(&saved, journal.path()).unwrap();
    assert!(journal.append_model_usage(&receipt, &owner).unwrap().is_some());
    assert_eq!(journal.leaf_id(), Some(successor.as_str()));
    assert_eq!(SessionJournal::open(journal.path()).unwrap().model_usage_records().count(), 1);
}

#[test]
fn lazy_materialization_and_per_entry_append() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let (model, thinking) =
        j.append_model_and_thinking_change("fake/m", None, false, Some("high"), Some("auto")).unwrap();
    assert_eq!(j.leaf_id(), Some(thinking.as_str()));
    j.append_message(&Message::User(UserMessage::text("hi"))).unwrap();
    assert!(!j.path().exists(), "no file before an assistant message");
    j.append_message(&assistant("hello", &[])).unwrap();
    assert!(j.path().exists());
    let raw = fs::read(j.path()).unwrap();
    let first_line_len = raw.iter().position(|b| *b == b'\n').unwrap() + 1;
    assert_eq!(first_line_len, SESSION_TITLE_SLOT_BYTES, "fixed-width title slot");
    let l = lines(j.path());
    assert_eq!(l[0]["type"], json!("title"));
    assert_eq!(l[1]["type"], json!("session"));
    assert_eq!(l[1]["version"], json!(3));
    assert_eq!(l[1]["id"], json!(j.session_id()));
    assert_eq!(l.len(), 6);
    assert_eq!(l[2]["type"], json!("model_change"));
    assert_eq!(l[2]["id"], model);
    assert_eq!(l[2]["parentId"], Value::Null);
    assert_eq!(l[3]["parentId"], l[2]["id"]);
    assert_eq!(l[3]["type"], json!("thinking_level_change"));
    assert_eq!(l[3]["id"], thinking);
    assert_eq!(l[4]["parentId"], l[3]["id"]);
    assert_eq!(l[5]["parentId"], l[4]["id"]);
    assert_eq!(l[5]["id"].as_str().unwrap().len(), 8);
    // Every later entry is on disk as soon as append returns.
    j.append_message(&Message::User(UserMessage::text("more"))).unwrap();
    assert_eq!(lines(j.path()).len(), 7);

    // An unchanged thinking selection must not manufacture a null/null row,
    // including before the ordinary lazy materialization boundary.
    let mut model_only = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let (model, thinking) = model_only.append_model_and_optional_thinking_change("fake/m", None, false, None).unwrap();
    assert_eq!(thinking, None);
    assert_eq!(model_only.entries().len(), 1);
    assert_eq!(model_only.entries()[0].kind, "model_change");
    assert_eq!(model_only.leaf_id(), Some(model.as_str()));
    assert!(!model_only.path().exists());
}

#[test]
fn reopen_restores_branch_and_continues_chain() {
    native_journal_writer_reader_context_recovery_group();
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let primary = j.append_model_change("fake/m").unwrap();
    assert_eq!(j.current_thinking_level().as_deref(), Some("off"));
    assert_eq!(j.current_configured_thinking_level(), None);
    let msgs =
        vec![Message::User(UserMessage::text("q")), assistant("", &["c1"]), result("c1"), assistant("done", &[])];
    for m in &msgs {
        j.append_message(m).unwrap();
    }
    let leaf = j.leaf_id().unwrap().to_string();
    let path = j.path().to_path_buf();
    drop(j);
    let mut r = SessionJournal::open(&path).unwrap();
    assert_eq!(r.report, LoadReport::default());
    assert_eq!(r.build_context().unwrap(), msgs);
    assert_eq!(r.current_model().unwrap().as_deref(), Some("fake/m"));
    assert_eq!(r.leaf_id(), Some(leaf.as_str()));
    r.append_message(&Message::User(UserMessage::text("next"))).unwrap();
    let l = lines(&path);
    assert_eq!(l.last().unwrap()["parentId"], json!(leaf));
    assert_eq!(latest_session(dir.path()).unwrap(), path);
    let original_leaf = r.leaf_id().unwrap().to_owned();
    let original_context = r.build_context().unwrap();

    // Native roles own restoration; the historical fallback flag alone does
    // not make a temporary selection ephemeral.
    r.append_model_change_with_role("fake/temporary", Some("temporary"), true).unwrap();
    assert_eq!(r.restorable_model_candidates().unwrap(), vec!["fake/temporary", "fake/m"]);
    let before_fallback = r.leaf_id().unwrap().to_owned();
    let (fallback, thinking) = r
        .append_model_and_thinking_change(
            "fake/fallback",
            Some(EPHEMERAL_MODEL_CHANGE_ROLE),
            false,
            Some("high"),
            Some("auto"),
        )
        .unwrap();
    assert_eq!(r.leaf_id(), Some(thinking.as_str()));
    assert_eq!(
        r.entries().iter().find(|entry| entry.id == fallback).unwrap().parent_id.as_deref(),
        Some(before_fallback.as_str())
    );
    assert_eq!(r.entries().last().unwrap().parent_id.as_deref(), Some(fallback.as_str()));
    let mut served = assistant("fallback completed", &[]);
    if let Message::Assistant(message) = &mut served {
        message.model = "fallback".into();
    }
    r.append_message(&served).unwrap();
    assert_eq!(r.restorable_model_candidates().unwrap(), vec!["fake/m"]);
    assert_eq!(r.current_model().unwrap().as_deref(), Some("fake/m"));
    let receipt = &r.entries().iter().find(|entry| entry.id == fallback).unwrap().raw;
    assert_eq!(receipt["role"], EPHEMERAL_MODEL_CHANGE_ROLE);
    assert_eq!(receipt["resolvedModelIsFallback"], false);
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries(), r.entries());
    assert_eq!(reopened.build_context().unwrap(), r.build_context().unwrap());
    assert_eq!(reopened.current_model().unwrap().as_deref(), Some("fake/m"));
    assert_eq!(reopened.current_thinking_level().as_deref(), Some("high"));
    assert_eq!(reopened.current_configured_thinking_level().as_deref(), Some("auto"));

    // Reapplying the same effective thinking level produces only the model
    // receipt. Existing effective/configured history survives restart intact.
    let previous_leaf = r.leaf_id().unwrap().to_owned();
    let previous_len = r.entries().len();
    let thinking_count = r.entries().iter().filter(|entry| entry.kind == "thinking_level_change").count();
    let (model, thinking) = r
        .append_model_and_optional_thinking_change(
            "fake/unchanged-thinking",
            Some(EPHEMERAL_MODEL_CHANGE_ROLE),
            true,
            None,
        )
        .unwrap();
    assert_eq!(thinking, None);
    assert_eq!(r.entries().len(), previous_len + 1);
    assert_eq!(r.entries().last().unwrap().parent_id.as_deref(), Some(previous_leaf.as_str()));
    assert_eq!(r.leaf_id(), Some(model.as_str()));
    let reopened = SessionJournal::open(&path).unwrap();
    assert_eq!(reopened.entries(), r.entries());
    assert_eq!(reopened.entries().iter().filter(|entry| entry.kind == "thinking_level_change").count(), thinking_count);
    assert_eq!(reopened.current_thinking_level().as_deref(), Some("high"));
    assert_eq!(reopened.current_configured_thinking_level().as_deref(), Some("auto"));

    // Null effective levels become off; absent configured selectors preserve
    // the concrete level. Both nulls are written, rather than omitted.
    r.append_thinking_level_change(None, Some("auto")).unwrap();
    assert_eq!(r.current_thinking_level().as_deref(), Some("off"));
    assert_eq!(r.current_configured_thinking_level().as_deref(), Some("auto"));
    r.append_thinking_level_change(Some("low"), None).unwrap();
    assert_eq!(r.current_thinking_level().as_deref(), Some("low"));
    assert_eq!(r.current_configured_thinking_level().as_deref(), Some("low"));

    // Import older records in this same file, retaining every message and
    // branch link. Null/missing role means default; legacy assistant inference
    // applies only while no nonempty explicit default has been recorded.
    let persisted = fs::read_to_string(&path).unwrap();
    let mut imported = lines(&path);
    let primary_index = imported.iter().position(|raw| raw["id"] == primary).unwrap();
    imported[primary_index]["role"] = Value::Null;
    imported.last_mut().unwrap().as_object_mut().unwrap().remove("configured");
    let write_imported = |records: &[Value]| {
        fs::write(&path, records.iter().map(Value::to_string).collect::<Vec<_>>().join("\n") + "\n").unwrap();
    };
    write_imported(&imported);
    let legacy = SessionJournal::open(&path).unwrap();
    assert_eq!(legacy.current_model().unwrap().as_deref(), Some("fake/m"));
    assert_eq!(legacy.current_configured_thinking_level().as_deref(), Some("low"));
    for model in [Some(Value::Null), Some(json!("")), None] {
        match model {
            Some(model) => imported[primary_index]["model"] = model,
            None => {
                imported[primary_index].as_object_mut().unwrap().remove("model");
            }
        }
        write_imported(&imported);
        let legacy = SessionJournal::open(&path).unwrap();
        assert_eq!(legacy.current_model().unwrap().as_deref(), Some("fake/fallback"));
        assert_eq!(legacy.build_context().unwrap(), r.build_context().unwrap());
    }
    imported.last_mut().unwrap()["thinkingLevel"] = Value::Null;
    write_imported(&imported);
    let legacy = SessionJournal::open(&path).unwrap();
    assert_eq!(legacy.current_thinking_level().as_deref(), Some("off"));
    assert_eq!(legacy.current_configured_thinking_level(), None);
    imported.last_mut().unwrap().as_object_mut().unwrap().remove("thinkingLevel");
    write_imported(&imported);
    assert_eq!(SessionJournal::open(&path).unwrap().current_thinking_level().as_deref(), Some("off"));
    fs::write(&path, persisted).unwrap();

    r.append_thinking_level_change(None, None).unwrap();
    assert_eq!(r.entries().last().unwrap().raw["thinkingLevel"], Value::Null);
    assert_eq!(r.entries().last().unwrap().raw["configured"], Value::Null);
    r.append_model_change_with_role("fake/replaced", Some("default"), true).unwrap();
    r.append_model_change_with_role("fake/second-fallback", Some(EPHEMERAL_MODEL_CHANGE_ROLE), true).unwrap();
    assert_eq!(r.current_model().unwrap().as_deref(), Some("fake/replaced"));
    r.append_model_change_with_role("fake/replaced", Some("temporary"), false).unwrap();
    assert_eq!(r.restorable_model_candidates().unwrap(), vec!["fake/replaced"], "equal role/default are tried once");
    r.append_model_change_with_role("fake/empty-role", Some(""), false).unwrap();
    assert_eq!(r.current_model().unwrap().as_deref(), Some("fake/replaced"));

    // A different active branch restores only its ancestors; off-path role
    // and thinking receipts remain present in the raw journal.
    let persisted = fs::read_to_string(&path).unwrap();
    let branch = json!({"type":"label","id":"branch01","parentId":original_leaf,"timestamp":"t",
        "targetId":original_leaf,"label":"original branch"});
    fs::write(&path, format!("{persisted}{branch}\n")).unwrap();
    let branched = SessionJournal::open(&path).unwrap();
    assert_eq!(branched.current_model().unwrap().as_deref(), Some("fake/m"));
    assert_eq!(branched.current_thinking_level().as_deref(), Some("off"));
    assert_eq!(branched.build_context().unwrap(), original_context);
    assert_eq!(branched.entries().len(), r.entries().len() + 1);
    fs::write(&path, persisted).unwrap();
}

#[test]
fn torn_tail_is_backed_up_then_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&assistant("seed", &[])).unwrap();
    let path = j.path().to_path_buf();
    drop(j);
    let torn = r#"{"type":"message","id":"torn","message":{"role":"user","content":"lost"#;
    let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
    std::io::Write::write_all(&mut f, torn.as_bytes()).unwrap();
    drop(f);
    let original = fs::read(&path).unwrap();

    let mut r = SessionJournal::open(&path).unwrap();
    assert_eq!(r.report.malformed_records, 1);
    assert_eq!(fs::read(&path).unwrap(), original, "open alone never rewrites");
    r.append_message(&Message::User(UserMessage::text("after resume"))).unwrap();
    let kinds: Vec<String> = lines(&path).iter().skip(1).map(|v| v["type"].as_str().unwrap().to_string()).collect();
    assert_eq!(kinds, vec!["session", "message", "message"]);
    let backup = r.report.backup.clone().expect("backup written");
    assert_eq!(fs::read(&backup).unwrap(), original, "torn bytes preserved");
    // A further append takes the fast path again.
    r.append_message(&Message::User(UserMessage::text("again"))).unwrap();
    assert_eq!(lines(&path).len(), 5);
}

#[test]
fn corrupt_header_is_rejected_without_touching_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.jsonl");
    let original = "{broken header\n{\"type\":\"message\",\"id\":\"m1\",\"parentId\":null,\"timestamp\":\"t\",\"message\":{\"role\":\"user\",\"content\":\"recover me\",\"timestamp\":0}}\n";
    fs::write(&path, original).unwrap();
    let err = SessionJournal::open(&path).err().expect("rejected");
    assert!(err.to_string().ends_with(CORRUPT_HEADER_MESSAGE), "{err}");
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::write(&path, "{\"type\":\"message\",\"id\":\"x\"}\n").unwrap();
    assert!(SessionJournal::open(&path).is_err());
}

#[test]
fn interrupted_tool_calls_get_unknown_effect_results() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&Message::User(UserMessage::text("q"))).unwrap();
    j.append_message(&assistant("", &["c1", "c2"])).unwrap();
    j.append_message(&result("c1")).unwrap();
    let path = j.path().to_path_buf();
    drop(j); // crash while c2 was running

    let mut r = SessionJournal::open(&path).unwrap();
    assert_eq!(
        r.recover_interrupted_tool_calls().unwrap(),
        Recovery { paired: vec!["c2".into()], unpaired_earlier: vec![] }
    );
    let ctx = r.build_context().unwrap();
    let Message::ToolResult(last) = ctx.last().unwrap() else { panic!() };
    assert_eq!(last.tool_call_id, "c2");
    assert!(last.is_error);
    assert_eq!(last.details.as_ref().unwrap()["source"], json!("interrupted_unknown_effect"));
    assert!(matches!(&last.content[0], UserBlock::Text(t) if t.text == UNKNOWN_EFFECT_TEXT));
    assert_eq!(r.recover_interrupted_tool_calls().unwrap(), Recovery::default(), "idempotent");
    drop(r);
    assert_eq!(SessionJournal::open(&path).unwrap().build_context().unwrap().len(), 4, "recovery is journaled");
}

#[test]
fn unknown_entry_types_survive_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&assistant("seed", &[])).unwrap();
    let path = j.path().to_path_buf();
    let leaf = j.leaf_id().unwrap().to_string();
    drop(j);
    let label = json!({"type": "label", "id": "lbl00001", "parentId": leaf, "timestamp": "t", "targetId": leaf, "label": "keep"});
    let mut body = fs::read_to_string(&path).unwrap();
    body.push_str(&format!("{label}\nnot json\n"));
    fs::write(&path, body).unwrap();
    let mut r = SessionJournal::open(&path).unwrap();
    r.append_message(&Message::User(UserMessage::text("x"))).unwrap();
    let l = lines(&path);
    assert!(l.contains(&label), "foreign entry preserved verbatim");
    assert_eq!(r.build_context().unwrap().len(), 2, "label is not model context");
}

#[test]
fn title_slot_is_fixed_width_even_for_long_titles() {
    let slot = serialize_title_slot(&TitleSlot {
        title: "长标题".repeat(200),
        source: Some("user".into()),
        updated_at: "2026-09-25T00:00:00.000Z".into(),
    });
    assert_eq!(slot.len(), SESSION_TITLE_SLOT_BYTES);
    let v: Value = serde_json::from_str(slot.trim_end()).unwrap();
    assert_eq!(v["type"], json!("title"));
    assert!(v["title"].as_str().unwrap().starts_with("长标题"));
}

fn write_header_file(path: &std::path::Path, header: Value, rest: &[Value]) {
    let mut body = format!("{header}\n");
    for v in rest {
        body.push_str(&format!("{v}\n"));
    }
    fs::write(path, body).unwrap();
}

#[test]
fn unsupported_version_is_rejected_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v1.jsonl");
    write_header_file(
        &path,
        json!({"type": "session", "id": "x", "timestamp": "t", "cwd": "/"}),
        &[json!({"type": "message", "timestamp": "t", "message": {"role": "user", "content": "old", "timestamp": 0}})],
    );
    let before = fs::read(&path).unwrap();
    let err = SessionJournal::open(&path).err().unwrap().to_string();
    assert!(err.contains("unsupported session version 1"), "{err}");
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn missing_parent_stops_the_walk_instead_of_failing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let msg = |id: &str, parent: Value, text: &str| json!({"type": "message", "id": id, "parentId": parent, "timestamp": "t", "message": {"role": "user", "content": text, "timestamp": 0}});
    let mut body = format!("{}\n", json!({"type": "session", "version": 3, "id": "s", "timestamp": "t", "cwd": "/"}));
    body.push_str(&format!("{}\n", msg("a", Value::Null, "first")));
    body.push_str("{\"type\":\"message\",\"id\":\"b\",\"parentId\":\"a\",\"message\":\"\\ud83d broken\n");
    body.push_str(&format!("{}\n", msg("c", json!("b"), "after gap")));
    fs::write(&path, body).unwrap();
    let j = SessionJournal::open(&path).unwrap();
    assert_eq!(j.report.malformed_records, 1);
    let ctx = j.build_context().unwrap();
    assert_eq!(ctx.len(), 1);
    assert!(matches!(&ctx[0], Message::User(u) if u.content.plain_text() == "after gap"));
}

#[test]
fn title_slot_source_survives_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let slot = serialize_title_slot(&TitleSlot {
        title: "My fix".into(),
        source: Some("user".into()),
        updated_at: "2026-09-20T00:00:00.000Z".into(),
    });
    let header =
        json!({"type": "session", "version": 3, "id": "s", "timestamp": "2026-09-01T00:00:00.000Z", "cwd": "/"});
    fs::write(&path, format!("{slot}{header}\nnot json\n")).unwrap();
    let mut j = SessionJournal::open(&path).unwrap();
    j.append_message(&Message::User(UserMessage::text("x"))).unwrap();
    let first: Value = serde_json::from_str(fs::read_to_string(&path).unwrap().lines().next().unwrap()).unwrap();
    assert_eq!(first["source"], json!("user"));
    assert_eq!(first["updatedAt"], json!("2026-09-20T00:00:00.000Z"));
    assert_eq!(first["title"], json!("My fix"));
}

#[test]
fn recovery_uses_raw_entries_and_reports_non_adjacent_gaps() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&assistant("", &["c1"])).unwrap();
    let path = j.path().to_path_buf();
    drop(j);
    // A newer writer's tool result ARA cannot decode (unknown content block).
    let mut body = fs::read_to_string(&path).unwrap();
    let leaf = body.lines().last().map(|l| serde_json::from_str::<Value>(l).unwrap()["id"].clone()).unwrap();
    body.push_str(&format!("{}\n", json!({"type": "message", "id": "r0000001", "parentId": leaf, "timestamp": "t", "message": {"role": "toolResult", "toolCallId": "c1", "toolName": "bash", "content": [{"type": "hologram"}], "isError": false, "timestamp": 1}})));
    fs::write(&path, body).unwrap();
    let mut r = SessionJournal::open(&path).unwrap();
    assert_eq!(r.undecodable_messages(), 1);
    assert_eq!(
        r.recover_interrupted_tool_calls().unwrap(),
        Recovery::default(),
        "undecodable result still counts as answered"
    );

    // Unpaired call followed by a user message cannot be paired adjacently.
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&assistant("", &["c9"])).unwrap();
    j.append_message(&Message::User(UserMessage::text("queued"))).unwrap();
    let before = j.entries().len();
    let rec = j.recover_interrupted_tool_calls().unwrap();
    assert_eq!(rec, Recovery { paired: vec![], unpaired_earlier: vec!["c9".into()] });
    assert_eq!(j.entries().len(), before, "nothing appended out of place");
}

#[test]
fn duplicate_ids_resolve_to_the_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.jsonl");
    let m = |text: &str| json!({"type": "message", "id": "dup00001", "parentId": null, "timestamp": "t", "message": {"role": "user", "content": text, "timestamp": 0}});
    write_header_file(
        &path,
        json!({"type": "session", "version": 3, "id": "s", "timestamp": "t", "cwd": "/"}),
        &[m("first"), m("second")],
    );
    let j = SessionJournal::open(&path).unwrap();
    assert_eq!(j.report.malformed_records, 0);
    assert_eq!(j.entries().len(), 2, "both lines kept for rewrite fidelity");
    let ctx = j.build_context().unwrap();
    assert!(matches!(&ctx[..], [Message::User(u)] if u.content.plain_text() == "second"));
}

#[test]
fn failed_append_is_rolled_back_and_retried_by_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_message(&assistant("seed", &[])).unwrap();
    let path = j.path().to_path_buf();
    let (len, leaf) = (j.entries().len(), j.leaf_id().unwrap().to_string());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap(); // appends now fail
    assert!(j.append_message(&Message::User(UserMessage::text("lost"))).is_err());
    assert!(j.append_model_change_with_role("fake/fallback", Some(EPHEMERAL_MODEL_CHANGE_ROLE), false).is_err());
    assert!(j.append_thinking_level_change(Some("high"), Some("auto")).is_err());
    let previous_entries = j.entries().to_vec();
    let previous_model = j.current_model().unwrap();
    let error = j
        .append_model_and_thinking_change("fake/replaced", Some("default"), true, Some("high"), Some("auto"))
        .unwrap_err();
    assert!(!error.history_published(), "neither member of the pair was published");
    assert_eq!(j.entries(), previous_entries);
    assert_eq!(j.current_model().unwrap(), previous_model);
    assert_eq!(j.current_configured_thinking_level(), None);
    let error =
        j.append_model_and_optional_thinking_change("fake/model-only", Some("default"), true, None).unwrap_err();
    assert!(!error.history_published());
    assert_eq!(j.entries(), previous_entries);
    assert_eq!(j.current_model().unwrap(), previous_model);
    assert_eq!(j.entries().len(), len, "Err means not recorded");
    assert_eq!(j.leaf_id(), Some(leaf.as_str()));
    assert_eq!(j.current_thinking_level().as_deref(), Some("off"));
    fs::remove_dir(&path).unwrap();
    j.append_message(&Message::User(UserMessage::text("kept"))).unwrap();
    let l = lines(&path);
    assert_eq!(l.len(), 4, "full rewrite restored seed + kept");
    assert_eq!(l[3]["parentId"], json!(leaf));
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "no temp files leaked");
}

// Append to the existing ara-session/tests/journal.rs coarse family.
// Draft only: Root integrates/runs on the delivered code with Set.size and
// String.endsWith dependencies. Fake blob storage is not real-host evidence.
mod actual_persistence_source_observations {
    use ara_rpc::value_heap::{
        CapabilityId, NoRuntime, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime,
        ValueSnapshot,
    };
    use ara_rpc::{WireString, WireValue};
    use ara_session::actual_persistence::{
        ActualPersistenceBlobs, ActualPersistenceModule, MAX_PERSIST_CHARS, PersistenceBufferEncoding,
        TRUNCATION_NOTICE, buffer_from_wire_string,
    };
    use std::sync::{Arc, Mutex, Weak};

    #[derive(Default)]
    struct Blobs {
        writes: Mutex<Vec<(Vec<u8>, Option<WireString>)>>,
        fail: bool,
    }
    impl ActualPersistenceBlobs for Blobs {
        fn buffer_from(
            &self,
            heap: &Arc<ValueHeap>,
            data: &ValueHandle,
            encoding: PersistenceBufferEncoding,
            _runtime: &dyn ValueRuntime,
        ) -> Result<Vec<u8>, ValueError> {
            buffer_from_wire_string(heap, data, encoding)
        }
        fn put_sync(&self, bytes: &[u8], extension: Option<&WireString>) -> Result<WireString, ValueError> {
            if self.fail {
                return Err(ValueError::TypeError("controlled native blob write failure".into()));
            }
            use ring::digest::{SHA256, digest};
            let hash = digest(&SHA256, bytes).as_ref().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            self.writes.lock().unwrap().push((bytes.to_vec(), extension.cloned()));
            Ok(format!("blob:sha256:{hash}").into())
        }
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn own(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, member: ValueHandle) {
        heap.define_own_data(value, key.into(), member, &NoRuntime).unwrap();
    }
    fn text(heap: &Arc<ValueHeap>, value: &ValueHandle) -> WireString {
        match heap.snapshot(value).unwrap() {
            ValueSnapshot::String(value) => value,
            _ => panic!("expected original string"),
        }
    }
    fn object(heap: &Arc<ValueHeap>, fields: Vec<(&str, ValueHandle)>) -> ValueHandle {
        let value = heap.object().unwrap();
        for (key, member) in fields {
            own(heap, &value, key, member);
        }
        value
    }
    fn string(heap: &Arc<ValueHeap>, value: &str) -> ValueHandle {
        heap.string(value.into()).unwrap()
    }

    struct Capture {
        heap: Weak<ValueHeap>,
        originals: Vec<ValueHandle>,
        saved: Mutex<Vec<(u64, ValueHandle)>>,
    }
    impl ValueRuntime for Capture {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.resolve_call(capability, receiver, arguments, self)
                }
                other => Err(ValueError::MissingCapability(other.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            args: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if !(1..=3).contains(&capability.0) {
                return Err(ValueError::MissingCapability(capability));
            }
            self.saved.lock().unwrap().push((capability.0, args[0].clone()));
            self.heap
                .upgrade()
                .unwrap()
                .call(&self.originals[(capability.0 - 1) as usize], receiver, args, runtime)
                .map(RuntimeReply::Value)
        }
    }

    #[test]
    fn actual_persistence_original_source_preparation_and_native_line_observations() {
        let heap = ValueHeap::new().unwrap();
        let module = ActualPersistenceModule::new(heap.clone()).unwrap();
        let blobs = Blobs::default();
        let small = object(&heap, vec![("type", string(&heap, "custom")), ("value", string(&heap, "small"))]);
        assert!(module.prepare_entry(&small, &blobs, &NoRuntime, Some(128)).unwrap().same_identity(&small));
        assert!(module.prepare_entry(&small, &blobs, &NoRuntime, None).unwrap().same_identity(&small));
        let mut nested = string(&heap, "finite leaf");
        for _ in 0..256 {
            nested = object(&heap, vec![("nested", nested)]);
        }
        assert!(module.prepare_entry(&nested, &blobs, &NoRuntime, None).unwrap().same_identity(&nested));
        assert!(matches!(module.prepare_entry(&nested, &blobs, &NoRuntime, Some(12)), Err(ValueError::RangeError(_))));

        // UTF16 prefix truncation drops a high surrogate at the cut; the live
        // source, signed fields, aliases and unpaired units stay canonical.
        let notice_units = TRUNCATION_NOTICE.encode_utf16().count();
        let prefix = MAX_PERSIST_CHARS - notice_units;
        let mut units = vec![u16::from(b'x'); MAX_PERSIST_CHARS + 2];
        units[prefix - 1] = 0xd83d;
        units[prefix] = 0xde00;
        let large = heap.string(WireString::from_units(units.clone())).unwrap();
        let lone = heap.string(WireString::from_units(vec![0xd800])).unwrap();
        let details = object(
            &heap,
            vec![
                ("content", large.clone()),
                ("lineCount", heap.number(999.0).unwrap()),
                ("jsonlEvents", heap.array(vec![string(&heap, "transient")]).unwrap()),
                ("lone", lone.clone()),
            ],
        );
        let signed = object(
            &heap,
            vec![
                ("type", string(&heap, "text")),
                ("text", large.clone()),
                ("textSignature", string(&heap, "signature")),
            ],
        );
        let encrypted = object(&heap, vec![("type", string(&heap, "reasoning")), ("encrypted_content", large.clone())]);
        let valid_server = object(
            &heap,
            vec![
                ("type", string(&heap, "anthropicServerTool")),
                (
                    "block",
                    object(
                        &heap,
                        vec![
                            ("type", string(&heap, "web_search_tool_result")),
                            ("tool_use_id", string(&heap, "id")),
                            ("content", large.clone()),
                        ],
                    ),
                ),
            ],
        );
        let invalid_server = object(
            &heap,
            vec![
                ("type", string(&heap, "anthropicServerTool")),
                ("block", object(&heap, vec![("type", string(&heap, "future")), ("content", large.clone())])),
            ],
        );
        let source = object(
            &heap,
            vec![
                ("type", string(&heap, "custom")),
                ("details", details.clone()),
                ("signed", signed.clone()),
                ("encrypted", encrypted.clone()),
                ("server", valid_server.clone()),
                ("unknown", invalid_server.clone()),
                ("alias", details.clone()),
                ("big", heap.bigint_decimal("9007199254740993").unwrap()),
            ],
        );
        let prepared = module.prepare_entry(&source, &blobs, &NoRuntime, Some(128)).unwrap();
        assert!(!prepared.same_identity(&source));
        assert!(get(&heap, &prepared, "signed").same_identity(&signed));
        assert!(get(&heap, &prepared, "encrypted").same_identity(&encrypted));
        assert!(get(&heap, &prepared, "server").same_identity(&valid_server));
        let prepared_details = get(&heap, &prepared, "details");
        assert!(!heap.has_own(&prepared_details, &"jsonlEvents".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&details, &"jsonlEvents".into(), &NoRuntime).unwrap());
        assert_eq!(text(&heap, &large).units(), units);
        let truncated = text(&heap, &get(&heap, &prepared_details, "content"));
        assert_eq!(truncated.len(), MAX_PERSIST_CHARS - 1);
        assert_eq!(truncated.units()[prefix - 2], u16::from(b'x'));
        assert!(truncated.units().ends_with(&TRUNCATION_NOTICE.encode_utf16().collect::<Vec<_>>()));
        assert!(get(&heap, &prepared_details, "lone").same_identity(&lone));
        assert!(matches!(
            heap.snapshot(&get(&heap, &prepared_details, "lineCount")).unwrap(),
            ValueSnapshot::Number(3.0)
        ));
        let unknown = get(&heap, &get(&heap, &prepared, "unknown"), "block");
        assert!(text(&heap, &get(&heap, &unknown, "content")).len() < units.len());
        let line = module.line_for(&source, &blobs, &NoRuntime, Some(128)).unwrap();
        let wire = line.to_utf8().unwrap();
        assert!(wire.contains("\\ud800"));
        assert!(wire.contains("\"big\":\"9007199254740993\""));
        assert!(wire.ends_with('\n'));
        assert!(WireValue::parse(wire.trim_end_matches('\n')).is_ok());

        // The encrypted payload copy is authoritative. An uncovered encrypted
        // signature cannot be dropped merely because its id happens to match.
        let reasoning = object(
            &heap,
            vec![
                ("type", string(&heap, "reasoning")),
                ("id", string(&heap, "r1")),
                ("encrypted_content", string(&heap, "cipher")),
            ],
        );
        let block = object(
            &heap,
            vec![
                ("type", string(&heap, "thinking")),
                ("thinking", string(&heap, "thought")),
                ("thinkingSignature", string(&heap, r#"{"type":"reasoning","id":"r1","encrypted_content":"cipher"}"#)),
            ],
        );
        let uncovered = object(
            &heap,
            vec![
                ("type", string(&heap, "thinking")),
                ("thinking", string(&heap, "keep")),
                (
                    "thinkingSignature",
                    string(&heap, r#"{"type":"reasoning","id":"r1","encrypted_content":"not-covered"}"#),
                ),
            ],
        );
        let message = object(
            &heap,
            vec![
                ("role", string(&heap, "assistant")),
                ("content", heap.array(vec![block.clone(), uncovered.clone()]).unwrap()),
                (
                    "providerPayload",
                    object(
                        &heap,
                        vec![
                            ("type", string(&heap, "openaiResponsesHistory")),
                            ("items", heap.array(vec![reasoning]).unwrap()),
                        ],
                    ),
                ),
            ],
        );
        let entry = object(&heap, vec![("type", string(&heap, "message")), ("message", message.clone())]);
        let prepared = module.prepare_entry(&entry, &blobs, &NoRuntime, Some(128)).unwrap();
        let content = get(&heap, &get(&heap, &prepared, "message"), "content");
        assert!(matches!(
            heap.snapshot(&get(&heap, &get(&heap, &content, "0"), "thinkingSignature")).unwrap(),
            ValueSnapshot::Undefined
        ));
        assert!(get(&heap, &content, "1").same_identity(&uncovered));
        assert!(matches!(heap.snapshot(&get(&heap, &block, "thinkingSignature")).unwrap(), ValueSnapshot::String(_)));
        let persisted = module.line_for(&entry, &blobs, &NoRuntime, Some(128)).unwrap().to_utf8().unwrap();
        assert_eq!(persisted.matches("thinkingSignature").count(), 1);

        // Content/images/frames/generation/base64 and complete image URLs share
        // the Host blob boundary; refs are never emitted after a failed write.
        let data = string(&heap, &"-_8 \n".repeat(300));
        let image = object(
            &heap,
            vec![("type", string(&heap, "image")), ("data", data.clone()), ("mimeType", string(&heap, "image/png"))],
        );
        let image_source = object(
            &heap,
            vec![
                ("type", string(&heap, "custom")),
                ("content", heap.array(vec![image.clone()]).unwrap()),
                (
                    "images",
                    heap.array(vec![object(
                        &heap,
                        vec![("data", data.clone()), ("mimeType", string(&heap, "image/jpeg"))],
                    )])
                    .unwrap(),
                ),
                (
                    "frames",
                    heap.array(vec![object(
                        &heap,
                        vec![("data", data.clone()), ("mimeType", string(&heap, "image/webp"))],
                    )])
                    .unwrap(),
                ),
                ("generation", object(&heap, vec![("type", string(&heap, "image_generation_call")), ("result", data)])),
                ("image_url", string(&heap, "data:image/png;base64,AA==")),
            ],
        );
        let prepared = module.prepare_entry(&image_source, &blobs, &NoRuntime, Some(128)).unwrap();
        assert!(
            text(&heap, &get(&heap, &get(&heap, &get(&heap, &prepared, "content"), "0"), "data"))
                .to_utf8()
                .unwrap()
                .starts_with("blob:sha256:")
        );
        assert_eq!(blobs.writes.lock().unwrap().len(), 5);
        assert_eq!(blobs.writes.lock().unwrap()[0].1, Some(WireString::from("png")));
        assert_eq!(blobs.writes.lock().unwrap()[4].0, b"data:image/png;base64,AA==");
        assert!(text(&heap, &get(&heap, &image, "data")).len() >= 1024);
        let failing = Blobs { fail: true, ..Default::default() };
        assert!(matches!(
            module.prepare_entry(&image_source, &failing, &NoRuntime, Some(128)),
            Err(ValueError::TypeError(_))
        ));
        assert!(failing.writes.lock().unwrap().is_empty());

        let cycle = object(&heap, vec![("type", string(&heap, "custom"))]);
        own(&heap, &cycle, "cycle", cycle.clone());
        assert!(matches!(module.prepare_entry(&cycle, &blobs, &NoRuntime, Some(12)), Err(ValueError::RangeError(_))));
        assert_eq!(
            buffer_from_wire_string(&heap, &lone, PersistenceBufferEncoding::Utf8).unwrap(),
            "\u{fffd}".as_bytes()
        );

        // Native overrides can retain all five authored callbacks. Each is a
        // real compiled function callable after the original runtime is gone.
        let prototype = heap.array_prototype();
        let names = ["some", "map", "find"];
        let originals = names.iter().map(|name| get(&heap, &prototype, name)).collect::<Vec<_>>();
        let runtime =
            Capture { heap: Arc::downgrade(&heap), originals: originals.clone(), saved: Mutex::new(Vec::new()) };
        for (index, name) in names.iter().enumerate() {
            own(&heap, &prototype, name, heap.function(CapabilityId(index as u64 + 1)).unwrap());
        }
        module.prepare_entry(&entry, &blobs, &runtime, Some(128)).unwrap();
        let count_source = object(
            &heap,
            vec![
                ("type", string(&heap, "custom")),
                ("content", string(&heap, "a\nb")),
                ("lineCount", heap.number(99.0).unwrap()),
                ("jsonlEvents", heap.array(vec![]).unwrap()),
            ],
        );
        module.prepare_entry(&count_source, &blobs, &runtime, Some(128)).unwrap();
        for (name, original) in names.iter().zip(originals) {
            own(&heap, &prototype, name, original);
        }
        let saved = std::mem::take(&mut *runtime.saved.lock().unwrap());
        drop(runtime);
        assert_eq!(saved.len(), 5);
        let call = |function: &ValueHandle, argument: &ValueHandle| {
            heap.call(function, &heap.undefined().unwrap(), std::slice::from_ref(argument), &NoRuntime).unwrap()
        };
        assert!(matches!(heap.snapshot(&call(&saved[0].1, &block)).unwrap(), ValueSnapshot::Boolean(true)));
        assert!(matches!(
            heap.snapshot(&get(&heap, &call(&saved[1].1, &block), "thinkingSignature")).unwrap(),
            ValueSnapshot::Undefined
        ));
        let pair = heap.array(vec![string(&heap, "content"), string(&heap, "ignored")]).unwrap();
        assert!(matches!(heap.snapshot(&call(&saved[2].1, &pair)).unwrap(), ValueSnapshot::Boolean(true)));
        assert!(matches!(heap.snapshot(&call(&saved[3].1, &pair)).unwrap(), ValueSnapshot::Boolean(false)));
        let pair = heap.array(vec![string(&heap, "lineCount"), heap.number(-1.0).unwrap()]).unwrap();
        assert!(matches!(
            heap.snapshot(&get(&heap, &call(&saved[4].1, &pair), "1")).unwrap(),
            ValueSnapshot::Number(2.0)
        ));
        for (_, function) in &saved {
            assert!(matches!(heap.snapshot(&get(&heap, function, "length")).unwrap(), ValueSnapshot::Number(1.0)));
            assert!(text(&heap, &get(&heap, function, "name")).is_empty());
        }
    }
}

// Joined native journal assertions in the existing reopen family. This is a
// deterministic Host adapter fixture, not a production process/model receipt.
fn native_journal_writer_reader_context_recovery_group() {
    use ara_rpc::value_heap::{
        NoRuntime, StringifyMode, ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    };
    use ara_rpc::{WireString as S, WireValue as W};
    use ara_session::actual_persistence::{
        ActualPersistenceBlobs, ActualPersistenceModule, PersistenceBufferEncoding, buffer_from_wire_string,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    struct Blobs;
    impl ActualPersistenceBlobs for Blobs {
        fn buffer_from(
            &self,
            heap: &Arc<ValueHeap>,
            data: &ValueHandle,
            encoding: PersistenceBufferEncoding,
            _runtime: &dyn ValueRuntime,
        ) -> std::result::Result<Vec<u8>, ValueError> {
            buffer_from_wire_string(heap, data, encoding)
        }
        fn put_sync(&self, _bytes: &[u8], _extension: Option<&S>) -> std::result::Result<S, ValueError> {
            Err(ValueError::UnsupportedOperation("fixture has no image blob publication"))
        }
    }
    struct Encoder {
        module: ActualPersistenceModule,
        fail: AtomicBool,
    }
    fn error(message: impl ToString) -> std::io::Error {
        std::io::Error::other(message.to_string())
    }
    impl NativeJournalEncoder for Encoder {
        fn encode_entry(&self, envelope: &Value, body: &ValueHandle, _blobs: Option<&BlobStore>) -> std::io::Result<S> {
            if self.fail.load(Ordering::SeqCst) {
                return Err(error("controlled native encoder failure"));
            }
            let heap = self.module.heap();
            let entry = heap.import_serde(envelope).map_err(error)?;
            heap.define_own_data(&entry, "message".into(), body.clone(), &NoRuntime).map_err(error)?;
            self.module.line_for(&entry, &Blobs, &NoRuntime, Some(1000)).map_err(error)
        }
        fn observe_body(&self, body: &ValueHandle) -> std::io::Result<W> {
            // This checked projection fixture contains only literal data. Real
            // Host current observations must retain native numbers/capabilities.
            let wire = self
                .module
                .heap()
                .stringify(body, StringifyMode::BigIntReplacer, None, &NoRuntime)
                .map_err(error)?
                .ok_or_else(|| error("fixture body was omitted"))?;
            W::parse_units(wire.units()).map_err(error)
        }
        fn observe_field(&self, body: &ValueHandle, path: &[&str]) -> std::io::Result<Option<W>> {
            let heap = self.module.heap();
            let mut value = body.clone();
            for key in path {
                value = heap.get(&value, &(*key).into(), &NoRuntime).map_err(error)?;
            }
            Ok(match heap.snapshot(&value).map_err(error)? {
                ValueSnapshot::Undefined => None,
                ValueSnapshot::Null => Some(W::Null),
                ValueSnapshot::Boolean(value) => Some(W::Bool(value)),
                ValueSnapshot::Number(value) => Some(W::Number(value)),
                ValueSnapshot::String(value) => Some(W::String(value)),
                _ => return Err(error("fixture field requires an explicit compound observation")),
            })
        }
        fn observe_tool_calls(&self, body: &ValueHandle) -> std::io::Result<Vec<(S, S)>> {
            let heap = self.module.heap();
            let content = heap.get(body, &"content".into(), &NoRuntime).map_err(error)?;
            if !matches!(heap.snapshot(&content).map_err(error)?, ValueSnapshot::Array) {
                return Ok(Vec::new());
            }
            let length = heap.get(&content, &"length".into(), &NoRuntime).map_err(error)?;
            let ValueSnapshot::Number(length) = heap.snapshot(&length).map_err(error)? else {
                return Err(error("array length"));
            };
            let mut calls = Vec::new();
            for index in 0..length as usize {
                let call = heap.get(&content, &index.to_string().as_str().into(), &NoRuntime).map_err(error)?;
                let kind = heap.get(&call, &"type".into(), &NoRuntime).map_err(error)?;
                if !matches!(heap.snapshot(&kind).map_err(error)?,ValueSnapshot::String(kind) if kind.equals_ascii("toolCall"))
                {
                    continue;
                }
                let id = heap.get(&call, &"id".into(), &NoRuntime).map_err(error)?;
                let name = heap.get(&call, &"name".into(), &NoRuntime).map_err(error)?;
                let (ValueSnapshot::String(id), ValueSnapshot::String(name)) =
                    (heap.snapshot(&id).map_err(error)?, heap.snapshot(&name).map_err(error)?)
                else {
                    return Err(error("exact call identity"));
                };
                calls.push((id, name));
            }
            Ok(calls)
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let heap = ValueHeap::new().unwrap();
    let encoder =
        Arc::new(Encoder { module: ActualPersistenceModule::new(heap.clone()).unwrap(), fail: AtomicBool::new(false) });
    let original_units = vec![0x0061, 0xd800, 0x0062];
    let user = heap
        .import_strict(W::object(vec![
            ("role", W::String("user".into())),
            ("content", W::String("native source".into())),
            ("timestamp", W::Number(1.0)),
            ("application", W::String(S::from_units(original_units.clone()))),
        ]))
        .unwrap();
    let assistant_wire =
        W::parse(&serde_json::to_value(assistant("native continuation", &[])).unwrap().to_string()).unwrap();
    let answer = heap.import_strict(assistant_wire).unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let user_id = journal.append_native_message(NativeMessageSource::live(user.clone(), encoder.clone())).unwrap();
    assert!(!journal.is_on_disk(), "same lazy materialization owner");
    let answer_id = journal.append_native_message(NativeMessageSource::live(answer.clone(), encoder.clone())).unwrap();
    assert_eq!(journal.entries()[1].parent_id.as_deref(), Some(user_id.as_str()));
    assert_eq!(journal.leaf_id(), Some(answer_id.as_str()));
    assert!(journal.build_context().is_err(), "legacy context explicitly rejects unrepresentable source");
    let model = Model {
        id: "m".into(),
        api: "openai-completions".into(),
        provider: "fake".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let raw = journal.raw_agent_context_for_route(&model, false).unwrap();
    let AgentMessageBody::Native(body) = &raw[0].body else { panic!("native canonical body") };
    assert!(body.live_body().unwrap().same_identity(&user));
    // A fresh logical Session cannot reuse Live handles from its parent fork.
    // Exercise the explicit durable boundary with exact unpaired UTF16 intact.
    let original_file = std::fs::read(journal.path()).unwrap();
    let original_entries = journal.entries().to_vec();
    let mut fork = journal.fork_at(Some(&answer_id), None).unwrap();
    let fork_heap = ValueHeap::new().unwrap();
    let fork_encoder = Arc::new(Encoder {
        module: ActualPersistenceModule::new(fork_heap.clone()).unwrap(),
        fail: AtomicBool::new(false),
    });
    assert!(fork.bind_native_realm(&fork_heap, fork_encoder.clone()).is_err());
    let before_fork = fork.entries().to_vec();
    encoder.fail.store(true, Ordering::SeqCst);
    assert!(fork.prepare_native_fork_for_realm().is_err());
    assert_eq!(fork.entries(), before_fork, "failed capture preserves candidate sources");
    encoder.fail.store(false, Ordering::SeqCst);
    fork.prepare_native_fork_for_realm().unwrap();
    fork.bind_native_realm(&fork_heap, fork_encoder).unwrap();
    assert_eq!(fork.leaf_id(), Some(answer_id.as_str()));
    let fork_user = fork.entries().iter().find(|entry| entry.id == user_id).unwrap();
    assert_eq!(
        fork_user.observed_message_field(&["application"]).unwrap().unwrap().as_string().unwrap().units(),
        original_units
    );
    assert!(!fork_user.native_message.as_ref().unwrap().live_body().unwrap().same_identity(&user));
    assert_eq!(journal.entries(), original_entries);
    assert_eq!(std::fs::read(journal.path()).unwrap(), original_file);
    let mut reopened = SessionJournal::open(journal.path()).unwrap();
    assert_eq!(reopened.report.malformed_records, 0, "native UTF16 body is not corrupt JSON");
    assert!(reopened.entries()[0].raw.get("message").is_none(), "no fake/null checked mirror");
    assert_eq!(
        reopened.entries()[0].observed_message_field(&["application"]).unwrap().unwrap().as_string().unwrap().units(),
        original_units
    );
    assert!(reopened.entries()[0].message().is_err());
    assert_eq!(reopened.current_model().unwrap().as_deref(), Some("fake/m"));
    let next_heap = ValueHeap::new().unwrap();
    let next_encoder = Arc::new(Encoder {
        module: ActualPersistenceModule::new(next_heap.clone()).unwrap(),
        fail: AtomicBool::new(false),
    });
    reopened.bind_native_realm(&next_heap, next_encoder.clone()).unwrap();
    let rebound = reopened.entries()[0].native_message.as_ref().unwrap().live_body().unwrap();
    assert!(!rebound.same_identity(&user), "fresh realm, no pointer codec");
    assert_eq!(
        reopened.entries()[0].observed_message_field(&["application"]).unwrap().unwrap().as_string().unwrap().units(),
        original_units
    );
    let before = reopened.entries().to_vec();
    let leaf = reopened.leaf_id().unwrap().to_owned();
    next_encoder.fail.store(true, Ordering::SeqCst);
    let body = next_heap.import_serde(&serde_json::to_value(assistant("failure", &[])).unwrap()).unwrap();
    assert!(reopened.append_native_message(NativeMessageSource::live(body, next_encoder.clone())).is_err());
    assert_eq!(reopened.entries(), before);
    assert_eq!(reopened.leaf_id(), Some(leaf.as_str()));
    next_encoder.fail.store(false, Ordering::SeqCst);
    let mut pending = W::parse(&serde_json::to_value(assistant("", &["pending"])).unwrap().to_string()).unwrap();
    let W::Object(fields) = &mut pending else { unreachable!() };
    let content = fields.iter_mut().find(|(key, _)| key.equals_ascii("content")).unwrap();
    let W::Array(blocks) = &mut content.1 else { unreachable!() };
    let exact_id = S::from_units(vec![0x0070, 0xdfff]);
    blocks[0].insert("id", W::String(exact_id.clone()));
    let pending_body = next_heap.import_strict(pending).unwrap();
    reopened.append_native_message(NativeMessageSource::live(pending_body, next_encoder)).unwrap();
    let recovery = reopened.recover_interrupted_tool_calls().unwrap();
    assert_eq!(recovery.paired, vec![exact_id.clone()]);
    assert!(recovery.unpaired_earlier.is_empty());
    let last = reopened.entries().last().unwrap();
    assert_eq!(last.observed_message_field(&["toolCallId"]).unwrap().unwrap().as_string(), Some(&exact_id));
    assert!(
        last.observed_message_field(&["details", "executed"])
            .unwrap()
            .unwrap()
            .as_string()
            .unwrap()
            .equals_ascii("unknown")
    );
    let reopened_path = reopened.path().to_owned();
    let mut final_journal = SessionJournal::open(&reopened_path).unwrap();
    let count = final_journal.entries().len();
    assert_eq!(final_journal.recover_interrupted_tool_calls().unwrap(), Recovery::default());
    assert_eq!(final_journal.entries().len(), count, "Unknown is answered evidence, never replay");
    assert_eq!(
        final_journal.entries().last().unwrap().observed_message_field(&["toolCallId"]).unwrap().unwrap().as_string(),
        Some(&exact_id)
    );
    // Raw Number observations retain bits independently of JSON.stringify's
    // durable normalization of -0; checked legacy decoding never substitutes 0.
    let negative_zero = NativeMessageSource::persisted(W::object(vec![
        ("role", W::String("extension".into())),
        ("number", W::Number(-0.0)),
    ]));
    assert_eq!(
        negative_zero.observe_field(&["number"]).unwrap().unwrap().as_number().unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
    // Native blob reads preserve unrelated UTF16 and adopt no candidate after
    // IO failure. Image-position resolution reuses the existing global store.
    let blobs = dir.path().join("native-blobs");
    let store = BlobStore::new(&blobs);
    let blob = store.put(&[0, 1, 2], Some("png")).unwrap();
    let image_body = W::object(vec![
        ("role", W::String("user".into())),
        ("timestamp", W::Number(1.0)),
        (
            "content",
            W::Array(vec![W::object(vec![
                ("type", W::String("image".into())),
                ("data", W::String(blob.reference.into())),
                ("mimeType", W::String("image/png".into())),
            ])]),
        ),
        ("application", W::String(S::from_units(original_units.clone()))),
    ]);
    let mut images = SessionJournal::create(&dir.path().join("image-session"), dir.path()).unwrap();
    images.append_native_message(NativeMessageSource::persisted(image_body)).unwrap();
    images.bind_blob_directory(&blobs).unwrap();
    let body = images.entries()[0].observed_message_body().unwrap().unwrap();
    assert!(
        body.get("content").unwrap().as_array().unwrap()[0]
            .get("data")
            .unwrap()
            .as_string()
            .unwrap()
            .equals_ascii("AAEC")
    );
    assert_eq!(body.get("application").unwrap().as_string().unwrap().units(), original_units);
    let bad = store.put(&[3, 4, 5], Some("png")).unwrap();
    let bad_ref = bad.reference;
    fs::remove_file(&bad.path).unwrap();
    fs::create_dir(&bad.path).unwrap();
    let broken = W::object(vec![
        ("role", W::String("user".into())),
        (
            "content",
            W::Array(vec![W::object(vec![("type", W::String("image".into())), ("data", W::String(bad_ref.into()))])]),
        ),
    ]);
    images.append_native_message(NativeMessageSource::persisted(broken)).unwrap();
    let original = images.entries().to_vec();
    assert!(images.bind_blob_directory(&blobs).is_err());
    assert_eq!(images.entries(), original, "all native resolutions precede view adoption");
}
