// Fixed OMP 596f2da native input families from agent/test/{supersede-prune,shake}.test.ts
// and coding-agent/test/strip-images-from-message.test.ts. One family per test:
// these verify reducer decisions, while Session/Host tests exercise persistence.
use std::sync::Arc;

use ara_agent::compaction::local_reduction::*;
use ara_ai::JsonObject;
use serde_json::json;

fn entry(id: &str, role: ReductionRole, text: &str, tokens: usize) -> ReductionEntry {
    ReductionEntry {
        entry_id: id.into(),
        kind: ReductionEntryKind::Message(role),
        timestamp_ms: Some(1_000.0),
        raw_message_tokens: tokens,
        shake_entry_tokens: tokens,
        slots: vec![ReductionContentSlot {
            slot: RawContentSlot::MessageContent,
            content: ReductionContent::Blocks(vec![ReductionBlock::Text(text.into())]),
        }],
        tool_result: None,
        tool_calls: Vec::new(),
    }
}

fn pair(id: &str, path: &str, tokens: usize) -> Vec<ReductionEntry> {
    let mut call = entry(&format!("call-{id}"), ReductionRole::Assistant, "", 5);
    call.tool_calls.push(ReductionToolCall { id: id.into(), name: "read".into(), arguments: args(path) });
    call.slots[0].content = ReductionContent::Blocks(vec![ReductionBlock::Other]);
    let mut result = entry(id, ReductionRole::ToolResult, &"file content\n".repeat(50), tokens);
    result.tool_result = Some(ReductionToolResult {
        call_id: id.into(),
        tool_name: "read".into(),
        already_pruned: false,
        useless: false,
        is_error: false,
        source_type: None,
        source_value: None,
        host_protected: false,
    });
    vec![call, result]
}

fn args(path: &str) -> JsonObject {
    json!({"path": path}).as_object().unwrap().clone()
}

fn prune_config() -> PruneConfig {
    PruneConfig {
        protect_tokens: 1_000_000.0,
        minimum_savings: 0.0,
        protected_tools: Vec::new(),
        ..PruneConfig::default()
    }
}

fn supersede_config() -> SupersedePruneConfig {
    SupersedePruneConfig { protected_tools: Vec::new(), ..SupersedePruneConfig::default() }
}

fn shake_config() -> ShakeConfig {
    ShakeConfig {
        protect_tokens: 0.0,
        min_savings: 0.0,
        protected_tools: Vec::new(),
        fence_min_tokens: 50.0,
        keep_boundary_id: None,
    }
}

fn ids(plan: &ReductionPlan) -> Vec<&str> {
    plan.edits.iter().map(|edit| edit.entry_id.as_str()).collect()
}
fn region_ids(regions: &[ShakeRegion]) -> Vec<&str> {
    regions.iter().map(|region| region.entry_id.as_str()).collect()
}
fn characters(fragments: &[&str]) -> usize {
    fragments.iter().map(|text| text.encode_utf16().count()).sum()
}
fn fence() -> String {
    format!("```ts\n{}\n```", "const value = compute(alpha, beta);\n".repeat(20))
}

#[test]
fn native_read_selector_input_family() {
    for (path, expected) in [
        ("src/foo.ts", "src/foo.ts"),
        ("src/foo.ts:50-200", "src/foo.ts\u{0}50-200"),
        ("src/foo.ts:raw", "src/foo.ts\0raw"),
        ("src/foo.ts:conflicts", "src/foo.ts\0conflicts"),
        ("src/foo.ts:2-4:raw", "src/foo.ts\u{0}2-4:raw"),
        ("src/foo.ts:raw:2-4", "src/foo.ts\0raw:2-4"),
        ("src/foo.ts:5-16,960-973", "src/foo.ts\u{0}5-16,960-973"),
        ("src/foo.ts:50+150", "src/foo.ts\u{0}50+150"),
        ("C:\\src\\foo.ts:L50..L200", "C:\\src\\foo.ts\0L50..L200"),
        ("C:\\src\\foo.ts:50..", "C:\\src\\foo.ts\u{0}50.."),
        ("src/foo.ts:50-", "src/foo.ts\u{0}50-"),
        ("db.sqlite:users", "db.sqlite:users"),
        ("db.sqlite:users:42", "db.sqlite:users\u{0}42"),
        ("src/foo.ts:raw:conflicts", "src/foo.ts:raw\0conflicts"),
        ("src/foo.ts:50+", "src/foo.ts:50+"),
        ("src/foo.ts:50,", "src/foo.ts:50,"),
        ("src/foo.ts:l2-L4:RAW", "src/foo.ts\0l2-L4:RAW"),
    ] {
        assert_eq!(read_tool_supersede_key("read", &args(path)).as_deref(), Some(expected), "{path}");
    }
    for path in ["skill://react", "https://example.com/page", "x.ts:artifact://3", ""] {
        assert_eq!(read_tool_supersede_key("read", &args(path)), None);
    }
    assert_eq!(read_tool_supersede_key("bash", &args("src/foo.ts")), None);
    assert_eq!(read_tool_supersede_key("read", json!({"path": 42}).as_object().unwrap()), None);
    assert_eq!(read_tool_supersede_key("read", &JsonObject::new()), None);
}

#[test]
fn native_supersede_tail_idle_selector_protection_family() {
    let key: &SupersedeKeyFn<'_> = &read_tool_supersede_key;
    for (paths, expected) in [
        (vec!["f.ts", "f.ts"], vec!["old"]),
        (vec!["f.ts:50-200", "f.ts:10-20"], vec![]),
        (vec!["f.ts:50-200", "f.ts:50-200"], vec!["old"]),
        (vec!["f.ts:50-200", "f.ts"], vec!["old"]),
        (vec!["f.ts", "f.ts:50-200"], vec![]),
    ] {
        let entries = [pair("old", paths[0], 500), pair("new", paths[1], 500)].concat();
        let before = entries.clone();
        let plan = plan_superseded_prune(&entries, &supersede_config(), Some(key), 2_000.0, 99).unwrap();
        assert_eq!(ids(&plan), expected, "{paths:?}");
        for edit in &plan.edits {
            assert_eq!(
                edit.action,
                ReductionAction::ReplaceToolResultContent { text: SUPERSEDED_NOTICE.into(), pruned_at_ms: 99 }
            );
        }
        assert_eq!(entries, before);
        assert_eq!(plan.expected_entries, before);
    }
    let base = [pair("old", "f.ts", 500), pair("mid", "f.ts", 500), pair("new", "f.ts", 500)].concat();
    for (case, expected) in [
        ("tail", vec!["old", "mid"]),
        ("warm", vec![]),
        ("idle", vec!["old", "mid"]),
        ("kept", vec!["mid"]),
        ("missing-boundary", vec!["old", "mid"]),
        ("pruned-superseder", vec!["old"]),
        ("protected", vec!["old"]),
        ("last-no-timestamp", vec![]),
        ("last-metadata", vec!["old", "mid"]),
    ] {
        let mut entries = base.clone();
        let mut config = supersede_config();
        let mut now = 2_000.0;
        match case {
            "warm" => config.suffix_token_limit = 0.0,
            "idle" => {
                config.suffix_token_limit = 0.0;
                now = 1_000.0 + config.idle_flush_ms;
            }
            "kept" => config.keep_boundary_id = Some("call-mid".into()),
            "missing-boundary" => config.keep_boundary_id = Some("absent".into()),
            "pruned-superseder" => entries[5].tool_result.as_mut().unwrap().already_pruned = true,
            "protected" => entries[3].tool_result.as_mut().unwrap().host_protected = true,
            "last-no-timestamp" => {
                config.suffix_token_limit = 0.0;
                now = 1_000.0 + config.idle_flush_ms;
                entries[5].timestamp_ms = None;
            }
            "last-metadata" => {
                config.suffix_token_limit = 0.0;
                now = 1_000.0 + config.idle_flush_ms;
                let mut metadata = entry("metadata", ReductionRole::User, "", 90_000);
                metadata.kind = ReductionEntryKind::Metadata;
                metadata.slots.clear();
                metadata.timestamp_ms = Some(now);
                entries.push(metadata);
            }
            _ => {}
        }
        let plan = plan_superseded_prune(&entries, &config, Some(key), now, 123).unwrap();
        assert_eq!(ids(&plan), expected, "{case}");
    }
    // Later raw assistant calls overwrite a duplicate ID, exactly as native Map.set.
    let mut duplicate = [pair("old", "f.ts", 500), pair("new", "other.ts", 500)].concat();
    let mut replacement = entry("duplicate-call", ReductionRole::Assistant, "", 5);
    replacement.tool_calls = vec![ReductionToolCall { id: "new".into(), name: "read".into(), arguments: args("f.ts") }];
    duplicate.push(replacement);
    assert_eq!(ids(&plan_superseded_prune(&duplicate, &supersede_config(), Some(key), 2_000.0, 123).unwrap()), ["old"]);
}

#[test]
fn native_age_useless_and_three_token_windows_family() {
    let key: &SupersedeKeyFn<'_> = &read_tool_supersede_key;
    let base = [pair("old", "f.ts", 500), pair("new", "f.ts", 500)].concat();
    let config = prune_config();
    assert!(plan_prune(&base, &config, None, 55).unwrap().edits.is_empty());
    let superseded = plan_prune(&base, &config, Some(key), 55).unwrap();
    assert_eq!(ids(&superseded), ["old"]);
    assert_eq!(superseded.estimated_tokens_saved, 500 - SUPERSEDED_NOTICE.encode_utf16().count().div_ceil(4));
    let mut age = config.clone();
    age.protect_tokens = 0.0;
    let all = plan_prune(&base, &age, None, 55).unwrap();
    assert_eq!(ids(&all), ["new", "old"]);
    assert!(all.edits.iter().all(|edit| matches!(&edit.action,
        ReductionAction::ReplaceToolResultContent { text, .. } if text == "[Output truncated - 500 tokens]")));
    for (case, expected) in [
        ("useless", 1),
        ("error", 0),
        ("tiny", 0),
        ("pruned", 0),
        ("protected", 0),
        ("useless-off", 0),
        ("warm", 0),
        ("kept", 0),
        ("savings", 0),
    ] {
        let mut entries = pair("flagged", "f.ts", 100);
        entries[1].tool_result.as_mut().unwrap().useless = true;
        let mut config = prune_config();
        match case {
            "error" => entries[1].tool_result.as_mut().unwrap().is_error = true,
            "tiny" => entries[1].raw_message_tokens = 3,
            "pruned" => entries[1].tool_result.as_mut().unwrap().already_pruned = true,
            "protected" => entries[1].tool_result.as_mut().unwrap().host_protected = true,
            "useless-off" => config.prune_useless = false,
            "warm" => {
                entries.push(entry("tail", ReductionRole::User, "huge", 10_000));
                config.cache_warm_suffix_tokens = Some(100.0);
            }
            "kept" => {
                entries.push(entry("tail", ReductionRole::User, "next", 10));
                config.keep_boundary_id = Some("tail".into());
            }
            "savings" => config.minimum_savings = 1_000.0,
            _ => {}
        }
        let plan = plan_prune(&entries, &config, None, 55).unwrap();
        assert_eq!(plan.counts.pruned_count, expected, "{case}");
        if expected == 1 {
            assert_eq!(
                plan.edits[0].action,
                ReductionAction::ReplaceToolResultContent { text: USELESS_NOTICE.into(), pruned_at_ms: 55 }
            );
        }
    }
    // A large user message counts for cache suffix, not for the ToolResult age window.
    let mut windows = pair("old", "f.ts", 100);
    windows.push(entry("user", ReductionRole::User, "large", 90_000));
    windows.extend(pair("new", "other.ts", 100));
    let mut age = prune_config();
    age.protect_tokens = 150.0;
    assert!(plan_prune(&windows, &age, None, 55).unwrap().edits.is_empty());
    age.protect_tokens = 100.0;
    assert_eq!(ids(&plan_prune(&windows, &age, None, 55).unwrap()), ["old"]);
    age.cache_warm_suffix_tokens = Some(200.0);
    assert!(plan_prune(&windows, &age, None, 55).unwrap().edits.is_empty());
    // Raw custom text is absent from prune suffix, present in shake's tail window.
    let mut custom = entry("custom", ReductionRole::User, "tail", 999_999);
    custom.kind = ReductionEntryKind::CustomMessage { custom_type: "hook".into() };
    custom.slots[0].slot = RawContentSlot::CustomContent;
    custom.shake_entry_tokens = 10_000;
    let entries = [pair("old", "f.ts", 100), pair("new", "f.ts", 100), vec![custom]].concat();
    let mut supersede = supersede_config();
    supersede.suffix_token_limit = 200.0;
    assert_eq!(ids(&plan_superseded_prune(&entries, &supersede, Some(key), 2_000.0, 55).unwrap()), ["old"]);
    let mut shake = shake_config();
    shake.protect_tokens = 8_000.0;
    assert_eq!(region_ids(&collect_shake_regions(&entries, &characters, &shake).unwrap()), ["old", "new"]);
    // Already-pruned tools still contribute to the age protection window.
    let mut count_pruned = [pair("old", "a", 100), pair("new", "b", 500)].concat();
    count_pruned[3].tool_result.as_mut().unwrap().already_pruned = true;
    age.cache_warm_suffix_tokens = None;
    age.protect_tokens = 300.0;
    assert_eq!(ids(&plan_prune(&count_pruned, &age, None, 55).unwrap()), ["old"]);
}

#[test]
fn native_shake_tool_text_presets_and_protection_family() {
    let mut mixed = pair("mixed", "f.ts", 30_000).remove(1);
    mixed.slots[0].content = ReductionContent::Blocks(vec![
        ReductionBlock::Text(String::new()),
        ReductionBlock::Image,
        ReductionBlock::Text("alpha".into()),
        ReductionBlock::Other,
        ReductionBlock::Text("beta".into()),
    ]);
    // Distinguish native countTokens(fragments) from countTokens(fragments.join("\n")).
    let tokens = |fragments: &[&str]| fragments.len() * 100;
    let regions = collect_shake_regions(std::slice::from_ref(&mixed), &tokens, &shake_config()).unwrap();
    assert_eq!((regions[0].tokens, regions[0].original_text.as_str()), (200, "alpha\nbeta"));
    let plan = finalize_shake_plan(std::slice::from_ref(&mixed), &regions, &["[shaken]".into()], 88).unwrap();
    assert_eq!(
        plan.edits[0].action,
        ReductionAction::ElideToolResultText { text: "[shaken]".into(), pruned_at_ms: 88 }
    );
    assert_eq!(plan.counts.tool_results_dropped, 1);
    assert_eq!(plan.estimated_tokens_saved, 184);
    assert_eq!(plan.expected_entries[0], mixed);
    for (case, expected) in [
        ("manual-tail", vec!["old"]),
        ("rescue", vec!["old", "new"]),
        ("default", vec!["old"]),
        ("skill", vec!["old"]),
        ("artifact-read", vec!["old"]),
        ("artifact-details", vec!["old"]),
        ("useless", vec!["old", "new"]),
        ("error", vec!["old"]),
        ("pruned", vec!["old"]),
        ("kept", vec![]),
    ] {
        let mut entries = [pair("old", "a", 5_000), pair("new", "b", 30_000)].concat();
        entries[1].slots[0].content = ReductionContent::Blocks(vec![ReductionBlock::Text("old-result ".repeat(3_000))]);
        entries[3].slots[0].content =
            ReductionContent::Blocks(vec![ReductionBlock::Text("recent-result ".repeat(3_000))]);
        let mut config = ShakeConfig::aggressive();
        match case {
            "rescue" => config = ShakeConfig::rescue(),
            "default" => config = ShakeConfig::default(),
            "skill" => {
                config = ShakeConfig::rescue();
                entries[2].tool_calls[0].arguments = args("skill://react");
            }
            "artifact-read" => {
                config = ShakeConfig::rescue();
                entries[2].tool_calls[0].arguments = args("artifact://3");
            }
            "artifact-details" => {
                config = ShakeConfig::rescue();
                let result = entries[3].tool_result.as_mut().unwrap();
                result.source_type = Some("internal".into());
                result.source_value = Some("artifact://3".into());
            }
            "useless" => entries[3].tool_result.as_mut().unwrap().useless = true,
            "error" => {
                entries[3].tool_result.as_mut().unwrap().useless = true;
                entries[3].tool_result.as_mut().unwrap().is_error = true;
            }
            "pruned" => {
                config = ShakeConfig::rescue();
                entries[3].tool_result.as_mut().unwrap().already_pruned = true;
            }
            "kept" => config.keep_boundary_id = Some("call-new".into()),
            _ => {}
        }
        assert_eq!(region_ids(&collect_shake_regions(&entries, &characters, &config).unwrap()), expected, "{case}");
    }
    let mut config = shake_config();
    config.min_savings = 1_000.0;
    assert!(collect_shake_regions(&[mixed], &tokens, &config).unwrap().is_empty());
    assert_eq!(shake_placeholder(200, Some("42"), 0), "[shaken ~200 tokens — recover: artifact://42 (region 1)]");
    assert_eq!(shake_placeholder(200, None, 0), "[shaken ~200 tokens]");
    assert!(collect_shake_regions(&[], &characters, &ShakeConfig::aggressive()).unwrap().is_empty());
}

#[test]
fn native_shake_fence_xml_multi_range_and_snapshot_family() {
    let first = fence();
    let second = format!("<example attr='value'>\n{}\n  </example>", "payload\n".repeat(40));
    let text = format!("😀 head\n{first}\nmiddle\n{second}\ntail");
    let mut source = entry("source", ReductionRole::Assistant, "", 1_000);
    source.slots[0].content = ReductionContent::Blocks(vec![ReductionBlock::Other, ReductionBlock::Text(text.clone())]);
    source.tool_calls.push(ReductionToolCall { id: "call".into(), name: "read".into(), arguments: args("x") });
    let regions = collect_shake_regions(std::slice::from_ref(&source), &characters, &shake_config()).unwrap();
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].original_text, first);
    assert_eq!(regions[1].original_text, second);
    let ShakeRegionKind::Block { block_index, start_utf16, expected_text: a, .. } = &regions[0].kind else {
        panic!();
    };
    let ShakeRegionKind::Block { expected_text: b, .. } = &regions[1].kind else {
        panic!();
    };
    assert_eq!(*block_index, Some(1));
    assert_eq!(*start_utf16, 8);
    assert!(Arc::ptr_eq(a, b), "one shared source text for all ranges in the slot");
    let plan = finalize_shake_plan(std::slice::from_ref(&source), &regions, &["[A]".into(), "[B]".into()], 99).unwrap();
    assert_eq!(plan.counts.blocks_dropped, 2);
    let mut rewritten = text.clone();
    for edit in &plan.edits {
        let ReductionAction::ReplaceTextRange { start_utf16, end_utf16, expected_text, replacement, .. } = &edit.action
        else {
            panic!();
        };
        assert_eq!(expected_text.as_ref(), text);
        let range = utf16_byte_range(&rewritten, *start_utf16, *end_utf16).unwrap();
        rewritten.replace_range(range, replacement);
    }
    assert_eq!(rewritten, "😀 head\n[A]\nmiddle\n[B]\ntail");
    for (case, snippet, expected) in [
        ("mixed-fences", "  ```ts\nlarge payload\n ~~~", 1),
        ("nested-xml", "<outer>\n<inner>\nx\n</inner>\n</outer>", 1),
        ("xml-fence-overlap", "<outer>\n```\nx\n```\n</outer>", 1),
        ("xml-inside-fence", "```\n<tag>\nx\n</tag>\n```", 1),
        // Fixed JS regex has no multiline flag: '$' does not admit a final
        // line terminator in the tag line retained by split('\n').
        ("xml-crlf", "<tag>\r\nx\r\n</tag>\r\n", 0),
        ("xml-line-separator", "<tag>\u{2028}\nx\n</tag>\u{2028}", 0),
        ("xml-paragraph-separator", "<tag>\u{2029}\nx\n</tag>\u{2029}", 0),
        ("unclosed-fence", "```\nx", 0),
        ("unclosed-xml", "<tag>\nx", 0),
        ("indented-opening", " <tag>\nx\n</tag>", 0),
        ("uppercase", "<Tag>\nx\n</Tag>", 0),
        ("mismatched-top", "<outer>\n<inner>\nx\n</outer>", 0),
    ] {
        let input = entry(case, ReductionRole::User, snippet, 100);
        let mut config = shake_config();
        config.fence_min_tokens = 0.0;
        let regions = collect_shake_regions(&[input], &characters, &config).unwrap();
        assert_eq!(regions.len(), expected, "{case}");
        if expected == 1 {
            assert_eq!(regions[0].original_text, snippet);
        }
    }
    for role in [
        ReductionRole::User,
        ReductionRole::Developer,
        ReductionRole::Assistant,
        ReductionRole::Custom,
        ReductionRole::HookMessage,
    ] {
        let expected =
            usize::from(matches!(role, ReductionRole::User | ReductionRole::Developer | ReductionRole::Assistant));
        let input = entry("role", role, &first, 1_000);
        assert_eq!(collect_shake_regions(&[input], &characters, &shake_config()).unwrap().len(), expected);
    }
    let mut custom = entry("custom", ReductionRole::User, &first, 1_000);
    custom.kind = ReductionEntryKind::CustomMessage { custom_type: "note".into() };
    custom.slots[0].slot = RawContentSlot::CustomContent;
    custom.slots[0].content = ReductionContent::String(first);
    let custom_regions = collect_shake_regions(&[custom], &characters, &shake_config()).unwrap();
    assert_eq!(custom_regions[0].label, "note");
    assert!(matches!(
        custom_regions[0].kind,
        ShakeRegionKind::Block { slot: RawContentSlot::CustomContent, block_index: None, .. }
    ));
    let mut changed = source.clone();
    changed.slots[0].content = ReductionContent::Blocks(vec![ReductionBlock::Text("changed".into())]);
    assert!(finalize_shake_plan(&[changed], &regions, &["a".into(), "b".into()], 99).is_err());
    assert_eq!(
        finalize_shake_plan(std::slice::from_ref(&source), &regions, &[], 99).unwrap_err(),
        ReductionError::InvalidReplacementCount
    );
    let mut invalid = regions.clone();
    let ShakeRegionKind::Block { start_utf16, .. } = &mut invalid[0].kind else {
        panic!();
    };
    *start_utf16 = 1;
    assert_eq!(
        finalize_shake_plan(std::slice::from_ref(&source), &invalid, &["a".into(), "b".into()], 99).unwrap_err(),
        ReductionError::InvalidUtf16Range
    );
    let overlap = vec![regions[0].clone(), regions[0].clone()];
    assert_eq!(
        finalize_shake_plan(&[source], &overlap, &["a".into(), "b".into()], 99).unwrap_err(),
        ReductionError::StaleRegion
    );
}

#[test]
fn native_images_and_thinking_raw_slot_family() {
    let mut entries = Vec::new();
    for (index, role) in [
        ReductionRole::User,
        ReductionRole::Developer,
        ReductionRole::Custom,
        ReductionRole::HookMessage,
        ReductionRole::Assistant,
        ReductionRole::Other("branchSummary".into()),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = entry(&format!("role-{index}"), role, "", 100);
        input.slots[0].content = ReductionContent::Blocks(vec![ReductionBlock::Image]);
        entries.push(input);
    }
    let mut mixed = pair("mixed", "f", 100).remove(1);
    mixed.slots[0].content =
        ReductionContent::Blocks(vec![ReductionBlock::Text("generated".into()), ReductionBlock::Image]);
    mixed.slots.push(ReductionContentSlot {
        slot: RawContentSlot::ToolResultDetailsImages,
        content: ReductionContent::Blocks(vec![ReductionBlock::Image, ReductionBlock::Other, ReductionBlock::Image]),
    });
    entries.push(mixed);
    let mut files = entry("files", ReductionRole::FileMention, "", 100);
    files.slots = vec![ReductionContentSlot {
        slot: RawContentSlot::FileMentionImage(1),
        content: ReductionContent::Blocks(vec![ReductionBlock::Image]),
    }];
    entries.push(files);
    let mut custom = entry("raw-custom", ReductionRole::User, "", 100);
    custom.kind = ReductionEntryKind::CustomMessage { custom_type: "note".into() };
    custom.slots[0] = ReductionContentSlot {
        slot: RawContentSlot::CustomContent,
        content: ReductionContent::Blocks(vec![ReductionBlock::Image]),
    };
    entries.push(custom);
    let plan = plan_drop_images(&entries).unwrap();
    assert_eq!(plan.counts.images_dropped, 9);
    assert_eq!(ids(&plan), ["role-0", "role-1", "role-2", "role-3", "mixed", "mixed", "files", "raw-custom"]);
    assert!(plan.edits.iter().all(|edit| match &edit.action {
        ReductionAction::DropImages { slot, placeholder_if_empty, .. } =>
            *placeholder_if_empty == matches!(slot, RawContentSlot::MessageContent | RawContentSlot::CustomContent),
        _ => false,
    }));
    let mut assistant = entry("thinking", ReductionRole::Assistant, "", 100);
    assistant.slots[0].content =
        ReductionContent::Blocks(vec![ReductionBlock::Thinking, ReductionBlock::RedactedThinking]);
    let mut user = assistant.clone();
    user.entry_id = "user-thinking".into();
    user.kind = ReductionEntryKind::Message(ReductionRole::User);
    let thinking = plan_drop_thinking(&[assistant, user]).unwrap();
    assert_eq!(thinking.counts.thinking_blocks_dropped, 2);
    assert_eq!(ids(&thinking), ["thinking"]);
    assert_eq!(
        thinking.edits[0].action,
        ReductionAction::DropBlocks { slot: RawContentSlot::MessageContent, indexes: vec![0, 1] }
    );
    assert_eq!(utf16_byte_range("😀x", 1, 2), Err(ReductionError::InvalidUtf16Range));
    assert_eq!(utf16_byte_range("😀x", 0, 2).unwrap(), 0..4);
    assert_eq!(utf16_byte_range("x", 2, 1), Err(ReductionError::InvalidUtf16Range));
}
