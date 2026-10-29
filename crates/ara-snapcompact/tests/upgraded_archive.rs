//! b91db66e scope/view/lazy-frame contracts, grouped by module behavior.
use ara_snapcompact::*;
use serde_json::{json, value::RawValue};
use std::{cell::RefCell, rc::Rc};

fn preserved(value: serde_json::Value) -> Archive {
    get_preserved_archive(Some(&json!({"snapcompact": value}))).unwrap()
}
fn frame(data: &str) -> serde_json::Value {
    json!({"data":data,"mimeType":"image/png","cols":64,"rows":40,"chars":10,"detail":"original"})
}
fn raw(value: serde_json::Value) -> Box<RawValue> {
    serde_json::value::to_raw_value(&value).unwrap()
}
fn preparation(archive: serde_json::Value) -> CompactionPreparationUtf16 {
    CompactionPreparationUtf16 {
        first_kept_entry_id: "kept".into(),
        messages_to_summarize: vec![raw(json!({"role":"user","content":"Continue."}))],
        turn_prefix_messages: vec![],
        tokens_before: 0.0,
        previous_summary: None,
        previous_preserve_data: Some(raw(
            json!({"other":"retain","openaiRemoteCompaction":{"old":1},"anthropicCompaction":{"old":2},"snapcompact":archive}),
        )),
        file_ops: create_file_ops(),
    }
}

#[test]
fn reasoning_scopes_views_and_recompaction() {
    observe_added_compact_and_view_contracts();
    for separator in ["\n", "█"] {
        let source = ["¶user: ask", "¶think: secret one", "", "secret two", "¶ai: answer", "¶think: final", "last"]
            .join(separator);
        assert_eq!(text_without_thinking(&source), ["¶user: ask", "¶ai: answer"].join(separator));
        let tool = format!(
            "¶call:read()\n<out>\n{DIM_ON}output\n¶think: printed label\n{DIM_OFF}</out>\n¶think: actual secret\n¶ai: answer"
        );
        let stripped = text_without_thinking(&tool);
        assert!(stripped.contains("¶think: printed label"));
        assert!(!stripped.contains("actual secret"));
    }
    let legends = "- `¶user:`, `¶think:`, `¶ai:`, `¶call:`: user, assistant reasoning, assistant reply, tool call.\n- `¶user:`, `¶think:`, `¶ai:`, and `¶call:` open user, assistant reasoning, assistant reply, and tool-call scopes.";
    let stripped = text_without_thinking(legends);
    assert!(!stripped.contains("¶think:"));
    assert!(!stripped.contains("assistant reasoning"));
    assert!(stripped.contains("`¶user:`, `¶ai:`, `¶call:`: user, assistant reply"));
    let lone = Utf16Text::from_units(vec![182, 97, 105, 58, 32, 0xd800]);
    assert_eq!(text_without_thinking_utf16(&lone), lone);
    let summary = "`¶user:`, `¶think:`, `¶ai:`, `¶call:`: user, assistant reasoning, assistant reply, tool call.";
    let text = "¶user: ask\n¶think: secret one\n\nsecret two\n¶ai: answer one\nfiller middle line\n¶ai: answer two";
    let head = &text[..text.find("secret two").unwrap() + 6];
    let tail = "¶ai: answer two";
    let source = json!({"frames":[frame("pixels")],"totalChars":100,"truncatedChars":5,"text":text,"textHead":head,"textTail":tail});
    let archive = preserved(source.clone());
    let details = CompactionDetails { read_files: vec!["src/a.ts".into()], modified_files: vec![] };
    let view = view_without_thinking(summary, &archive, Some(&details)).unwrap();
    assert!(view.archive.frames.is_empty());
    assert!(!view.summary.contains("assistant reasoning"));
    assert!(view.summary.contains("a.ts (Read)"));
    assert!(!view.archive.text.as_ref().unwrap().contains("secret"));
    assert_eq!(view.archive.text_tail.as_deref(), Some(tail));
    let variant =
        json!({"summary":"plain legend","archive":{"frames":[frame("clean")],"totalChars":10,"text":"¶ai: hi"}});
    let mut stored = source;
    stored["withoutThinking"] = variant.clone();
    let archive = preserved(stored.clone());
    let view = view_without_thinking(summary, &archive, None).unwrap();
    assert_eq!(view.summary, "plain legend");
    assert_eq!(view.archive.frames[0].data, "clean");
    for malformed in
        [json!({"summary":"","archive":variant["archive"]}), json!({"summary":"s","archive":{"frames":[]}})]
    {
        stored["withoutThinking"] = malformed;
        assert!(preserved(stored.clone()).without_thinking.is_none());
    }
    let legacy = json!({"frames":[frame("legacy")],"totalChars":100,"textHead":"¶user: ask\n¶ai: answer one","textTail":"secret continuation\n¶ai: answer two"});
    let view = view_without_thinking(summary, &preserved(legacy.clone()), None).unwrap();
    assert!(!view.archive.text.as_ref().unwrap().contains("secret"));
    let mut options = CompactionOptions { frame_size: Some(320), ..Default::default() };
    options.serialize.include_thinking = false;
    let result = compact_utf16(&preparation(legacy), &options).unwrap();
    assert!(!result.summary.contains("¶think:"));
    let preserve = result.preserve_data.as_ref().unwrap().get();
    assert!(!preserve.contains("openaiRemoteCompaction") && !preserve.contains("anthropicCompaction"));
    assert!(preserve.contains("retain") && !preserve.contains("secret"));
    options.serialize.include_thinking = true;
    let result = compact_utf16(&preparation(json!({"text":"¶think: keep me"})), &options).unwrap();
    assert!(result.summary.contains("¶think:"));
    assert!(result.preserve_data.unwrap().get().contains("keep me"));
    println!(
        "THINKING: raw+normalized multi-paragraph/dim/legends/lone/variant/malformed/failclosed/recompact/provider-reset PASS"
    );
}

#[test]
fn lazy_frames_chronological_gaps_and_send_time_cap() {
    let preserve = raw(
        json!({"snapcompact":{"frames":[frame("old"), frame("missing"), frame("middle"), frame("huge"), frame("new")], "totalChars":50,"textHead":"head","textTail":"tail"}}),
    );
    let archive = get_preserved_archive_utf16(Some(&preserve)).unwrap().unwrap();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let reads = Rc::new(RefCell::new(Vec::new()));
    let blocks =
        history_blocks_with_resolver_utf16(&archive, HistoryBlockOptions { max_frame_data_bytes: Some(7) }, |data| {
            let data = data.to_scalar().unwrap();
            calls.borrow_mut().push(data.clone());
            if data == "missing" {
                return Ok(None);
            }
            let bytes = if data == "huge" { 100 } else { data.len() };
            let reads = reads.clone();
            Ok(Some(LazyFrameData {
                bytes,
                read: Box::new(move || {
                    assert_ne!(data, "huge");
                    reads.borrow_mut().push(data.clone());
                    Ok(Utf16Text::from(data))
                }),
            }))
        })
        .unwrap();
    assert_eq!(*calls.borrow(), ["new", "huge", "middle", "missing", "old"]);
    assert_eq!(*reads.borrow(), ["new", "old"]);
    let kinds: Vec<_> = blocks
        .iter()
        .map(|block| match block {
            HistoryBlockUtf16::Text { text } => text.to_scalar().unwrap(),
            HistoryBlockUtf16::Image { data, .. } => data.to_scalar().unwrap(),
        })
        .collect();
    assert_eq!(kinds[1], "old");
    assert!(kinds[2].contains("1 archived image frame unavailable here"));
    assert!(kinds[3].contains("2 archived image frames could not be included. 106 B"));
    assert_eq!(kinds[4], "new");
    assert!(kinds[0].contains("imaged middle below") && kinds[5].contains("imaged middle above"));
    let unresolved = preserved(json!({"frames":[frame("blob:sha256:malformed"), frame("valid")], "totalChars":20}));
    let blocks = history_blocks(&unresolved, HistoryBlockOptions::default());
    assert!(matches!(&blocks[0], HistoryBlock::Text { text } if text.contains("unavailable here")));
    assert!(matches!(&blocks[1], HistoryBlock::Image { data,.. } if data == "valid"));
    let pointer = blocks.as_ptr();
    let blocks = cap_frame_data_bytes(blocks, 100);
    assert_eq!(pointer, blocks.as_ptr());
    let blocks = cap_frame_data_bytes(blocks, 0);
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[1], HistoryBlock::Text { text } if text.contains("could not be included. 5 B")));
    let fitting = preserved(
        json!({"frames":[frame("aa"),frame("too big"),frame("bb")],"totalChars":30,"textHead":"H","textTail":"T"}),
    );
    let capped = cap_frame_data_bytes(history_blocks(&fitting, HistoryBlockOptions::default()), 4);
    assert!(matches!(&capped[1], HistoryBlock::Image { data,.. } if data == "aa"));
    assert!(matches!(&capped[2], HistoryBlock::Text { text } if text.contains("7 B")));
    assert!(matches!(&capped[3], HistoryBlock::Image { data,.. } if data == "bb"));
    println!(
        "LAZY: newest-first pricing/read-only-selected/mixed chronological gaps/unresolved-ref/identity/reclamp PASS"
    );
}

// Supplement to the existing reasoning coarse bank: exercise compact/view
// consumers of the changed helpers with the target source's observations.
fn observe_added_compact_and_view_contracts() {
    let old_and_new_legends = "- `¶user:`, `¶think:`, `¶ai:`, `¶call:`: user, assistant reasoning, assistant reply, tool call.\n- `¶user:`, `¶think:`, `¶ai:`, and `¶call:` open user, assistant reasoning, assistant reply, and tool-call scopes.\n¶user: hi\n¶think: hidden\n¶ai: shown";
    assert_eq!(
        text_without_thinking(old_and_new_legends),
        "- `¶user:`, `¶ai:`, `¶call:`: user, assistant reply, tool call.\n- `¶user:`, `¶ai:`, and `¶call:` open user, assistant reply, and tool-call scopes.\n¶user: hi\n¶ai: shown"
    );
    let mut options = CompactionOptions { frame_size: Some(320), ..Default::default() };
    // Conditional legends must also work for a file-only preamble, independent
    // of whether a thinking block actually occurs in the fresh input.
    let mut file_only = preparation(json!({"text":"unused"}));
    file_only.previous_preserve_data = None;
    file_only.messages_to_summarize.clear();
    file_only.file_ops.read.insert("src/auth.ts".into());
    for include in [false, true] {
        options.serialize.include_thinking = include;
        let result = compact_utf16(&file_only, &options).unwrap();
        assert!(result.summary.contains("`¶user:`") && result.summary.contains("`¶call:`"));
        assert_eq!(result.summary.contains("`¶think:`"), include);
    }
    options.serialize.include_thinking = true;
    let mut initial = preparation(json!({"text":"unused"}));
    initial.previous_preserve_data = None;
    initial.messages_to_summarize = vec![
        raw(json!({"role":"user","content":"Investigate the flaky auth test."})),
        raw(
            json!({"role":"assistant","content":[{"type":"thinking","thinking":"First reasoning paragraph.\n\nSecond reasoning paragraph.\nThird reasoning line."},{"type":"text","text":"The token clock is skewed."}]}),
        ),
    ];
    let first = compact_utf16(&initial, &options).unwrap();
    assert!(
        get_preserved_archive_utf16(first.preserve_data.as_deref())
            .unwrap()
            .unwrap()
            .text
            .unwrap()
            .to_scalar()
            .unwrap()
            .contains("¶think:")
    );
    options.serialize.include_thinking = false;
    let mut next = preparation(json!({"text":"unused"}));
    next.previous_preserve_data = first.preserve_data;
    let second = compact_utf16(&next, &options).unwrap();
    let source = get_preserved_archive_utf16(second.preserve_data.as_deref())
        .unwrap()
        .unwrap()
        .text
        .unwrap()
        .to_scalar()
        .unwrap();
    assert!(!source.contains("¶think:") && !source.contains("reasoning"));
    assert!(source.contains("Investigate the flaky auth test.") && source.contains("The token clock is skewed."));
    let legacy_text = "¶user: Investigate the flaky auth test.\n¶think: First reasoning paragraph.\n\nSecond reasoning paragraph.\n¶ai: The token clock is skewed.";
    let second =
        compact_utf16(&preparation(json!({"frames":[],"totalChars":legacy_text.len(),"text":legacy_text})), &options)
            .unwrap();
    let source = get_preserved_archive_utf16(second.preserve_data.as_deref())
        .unwrap()
        .unwrap()
        .text
        .unwrap()
        .to_scalar()
        .unwrap();
    assert!(!source.contains("reasoning"));
    assert!(source.contains("Investigate the flaky auth test.") && source.contains("The token clock is skewed."));
    // Serialize a genuine tool result's DIM scope, then recompact it. This
    // exercises the helper's real archive/normalization consumer.
    options.serialize.include_thinking = true;
    initial.messages_to_summarize = vec![
        raw(
            json!({"role":"assistant","content":[{"type":"toolCall","id":"call-1","name":"bash","arguments":{"command":"cat dump.txt"}}]}),
        ),
        raw(
            json!({"role":"toolResult","toolCallId":"call-1","content":[{"type":"text","text":"dump start\n¶think: dumped line\ndump end"}]}),
        ),
        raw(json!({"role":"assistant","content":[{"type":"text","text":"Read the dump."}]})),
    ];
    let first = compact_utf16(&initial, &options).unwrap();
    next.previous_preserve_data = first.preserve_data;
    options.serialize.include_thinking = false;
    let second = compact_utf16(&next, &options).unwrap();
    let source = get_preserved_archive_utf16(second.preserve_data.as_deref())
        .unwrap()
        .unwrap()
        .text
        .unwrap()
        .to_scalar()
        .unwrap();
    assert!(source.contains("¶think: dumped line") && source.contains("dump end") && source.contains("Read the dump."));
    // An old summary legend lives mid-line after normalization, where it is
    // not a scope boundary; recompaction must nevertheless scrub the legend.
    let legend = "- `¶user:`, `¶think:`, `¶ai:`, `¶call:`: user, assistant reasoning, assistant reply, tool call.";
    let source = format!("¶user: 41822 Resume prior conversation.█{legend}█¶ai: done");
    let second =
        compact_utf16(&preparation(json!({"frames":[],"totalChars":source.len(),"text":source})), &options).unwrap();
    let source = get_preserved_archive_utf16(second.preserve_data.as_deref())
        .unwrap()
        .unwrap()
        .text
        .unwrap()
        .to_scalar()
        .unwrap();
    assert!(!source.contains("¶think:") && !source.contains("assistant reasoning"));
    assert!(source.contains("`¶user:`, `¶ai:`, `¶call:`: user, assistant reply, tool call."));
    let summary = "Archived transcript scopes:\n- `¶user:`, `¶think:`, `¶ai:`, `¶call:`: user, assistant reasoning, assistant reply, tool call.\nHISTORY";
    let text = "¶user: ask\n¶think: secret one\n\nsecret two\n¶ai: answer one\nfiller middle line\n¶ai: answer two";
    let head = &text[..text.find("secret two").unwrap() + 6];
    let tail = "¶ai: answer two";
    let archive = preserved(
        json!({"frames":[frame("pixels")],"totalChars":100,"truncatedChars":5,"text":text,"textHead":head,"textTail":tail}),
    );
    let details = CompactionDetails { read_files: vec!["src/a.ts".into()], modified_files: vec![] };
    let view = view_without_thinking(summary, &archive, Some(&details)).unwrap();
    let stripped = "¶user: ask\n¶ai: answer one\nfiller middle line\n¶ai: answer two";
    assert!(view.archive.text_head.as_deref().unwrap().starts_with("¶user: ask\n¶ai: answer one"));
    assert_eq!(
        view.archive.truncated_chars,
        5.0 + (stripped.encode_utf16().count() - head.encode_utf16().count() - tail.encode_utf16().count()) as f64
    );
    let sent = format!(
        "{}\n{}",
        view.summary,
        history_blocks(&view.archive, HistoryBlockOptions::default())
            .iter()
            .map(|block| match block {
                HistoryBlock::Text { text } => text.as_str(),
                _ => panic!("fail-closed image"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(!sent.contains("secret") && !sent.contains("¶think:") && !sent.contains("assistant reasoning"));
    assert!(
        sent.contains("`¶user:`, `¶ai:`, `¶call:`: user, assistant reply, tool call.") && sent.contains("a.ts (Read)")
    );
    let original = view_without_thinking("no scopes", &archive, None).unwrap();
    assert_eq!(original.summary, "no scopes");
    assert_eq!(original.archive, archive);
    let legacy = json!({"frames":[frame("legacy")],"totalChars":100,"textHead":"¶user: ask\n¶ai: answer one","textTail":"secret continuation\n¶ai: answer two"});
    let result = compact_utf16(&preparation(legacy.clone()), &options).unwrap();
    let source = get_preserved_archive_utf16(result.preserve_data.as_deref())
        .unwrap()
        .unwrap()
        .text
        .unwrap()
        .to_scalar()
        .unwrap();
    assert!(!source.contains("secret") && source.contains("answer two"));
    let view = view_without_thinking(summary, &preserved(legacy), None).unwrap();
    let text = history_blocks(&view.archive, HistoryBlockOptions::default())
        .iter()
        .map(|block| match block {
            HistoryBlock::Text { text } => text.clone(),
            _ => panic!("legacy fail-closed image"),
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains("secret") && text.contains("answer two"));
}
