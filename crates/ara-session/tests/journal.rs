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
    })
}

fn lines(path: &std::path::Path) -> Vec<Value> {
    fs::read_to_string(path).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

#[test]
fn lazy_materialization_and_per_entry_append() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_model_change("fake/m").unwrap();
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
    assert_eq!(l.len(), 5);
    assert_eq!(l[2]["type"], json!("model_change"));
    assert_eq!(l[2]["parentId"], Value::Null);
    assert_eq!(l[3]["parentId"], l[2]["id"]);
    assert_eq!(l[4]["parentId"], l[3]["id"]);
    assert_eq!(l[4]["id"].as_str().unwrap().len(), 8);
    // Every later entry is on disk as soon as append returns.
    j.append_message(&Message::User(UserMessage::text("more"))).unwrap();
    assert_eq!(lines(j.path()).len(), 6);
}

#[test]
fn reopen_restores_branch_and_continues_chain() {
    let dir = tempfile::tempdir().unwrap();
    let mut j = SessionJournal::create(dir.path(), dir.path()).unwrap();
    j.append_model_change("fake/m").unwrap();
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
    assert_eq!(r.build_context(), msgs);
    assert_eq!(r.current_model().as_deref(), Some("fake/m"));
    assert_eq!(r.leaf_id(), Some(leaf.as_str()));
    r.append_message(&Message::User(UserMessage::text("next"))).unwrap();
    let l = lines(&path);
    assert_eq!(l.last().unwrap()["parentId"], json!(leaf));
    assert_eq!(latest_session(dir.path()).unwrap(), path);
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
        Recovery { paired: vec!["c2".to_string()], unpaired_earlier: vec![] }
    );
    let ctx = r.build_context();
    let Message::ToolResult(last) = ctx.last().unwrap() else { panic!() };
    assert_eq!(last.tool_call_id, "c2");
    assert!(last.is_error);
    assert_eq!(last.details.as_ref().unwrap()["source"], json!("interrupted_unknown_effect"));
    assert!(matches!(&last.content[0], UserBlock::Text(t) if t.text == UNKNOWN_EFFECT_TEXT));
    assert_eq!(r.recover_interrupted_tool_calls().unwrap(), Recovery::default(), "idempotent");
    drop(r);
    assert_eq!(SessionJournal::open(&path).unwrap().build_context().len(), 4, "recovery is journaled");
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
    assert_eq!(r.build_context().len(), 2, "label is not model context");
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
    let ctx = j.build_context();
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
    let ctx = j.build_context();
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
    assert_eq!(j.entries().len(), len, "Err means not recorded");
    assert_eq!(j.leaf_id(), Some(leaf.as_str()));
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
