//! Original fixed EDIT renderer observations grouped as 31 + 8 + 2.
//! OMP 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; repository notices).
//! These source fixtures are not physical terminal or whole TUI acceptance.
use super::{
    diff,
    edit_renderer::{EditComponent, is_edit_renderer, renderer, streaming_fallback},
    render_utils::{sanitize, width},
    theme::Theme,
    tool_execution::ToolExecutionComponent,
    types::*,
};
use ara_edit::{
    EditMode,
    diff_string::{BlockContextSource, generate_diff_string},
};
use ara_rpc::WireString;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn args(value: serde_json::Value) -> EditRenderArgs {
    super::event_controller::args_from_json(&value)
}
fn options(partial: bool, expanded: bool) -> RenderOptions {
    RenderOptions { is_partial: partial, expanded, spinner_frame: partial.then_some(0) }
}
fn context(mode: EditMode) -> EditRenderContext {
    EditRenderContext { edit_mode: Some(mode), ..Default::default() }
}
fn call(value: serde_json::Value, mode: EditMode, expanded: bool) -> EditComponent {
    EditComponent::call(args(value), options(true, expanded), context(mode), Arc::new(Theme::dark()))
}
fn complete(path: &str, diff: &str, expanded: bool) -> EditComponent {
    EditComponent::result(
        EditResult {
            content: vec![format!("Updated {path}").into()],
            details: Some(EditDetails {
                diff: WireText(diff.into()),
                op: Some(Operation::Update),
                ..Default::default()
            }),
            ..Default::default()
        },
        args(serde_json::json!({"file_path":path})),
        options(false, expanded),
        context(EditMode::Hashline),
        Arc::new(Theme::dark()),
    )
}
fn rows(component: &mut EditComponent, columns: usize, viewport_rows: usize) -> Vec<String> {
    component.render(columns, &FrameContext { columns, rows: viewport_rows, ..Default::default() })
}
fn stripped(rows: &[String]) -> String {
    rows.iter().map(|s| sanitize(&WireString::from(s.as_str()))).collect::<Vec<_>>().join("\n")
}
fn render(component: &mut EditComponent, columns: usize) -> String {
    stripped(&rows(component, columns, 30))
}
fn tool(name: &str, value: serde_json::Value, mode: EditMode) -> ToolExecutionComponent {
    ToolExecutionComponent::new("call-1".into(), name.into(), args(value), mode, Arc::new(Theme::dark()))
}
fn tool_render(component: &mut ToolExecutionComponent, columns: usize) -> String {
    stripped(&component.render(columns, &FrameContext { columns, ..Default::default() }, 0))
}
fn preview(path: &str, old: &str, new: &str) -> PerFilePreview {
    let generated =
        generate_diff_string(old, new, None, &BlockContextSource { path: Some(path), ..Default::default() });
    PerFilePreview {
        path: WireText(path.into()),
        diff: Some(generated.diff.into()),
        first_changed_line: generated.first_changed_line,
        ..Default::default()
    }
}
fn contains(text: &str, expected: &str) {
    assert!(text.contains(expected), "expected {expected:?} in:\n{text}");
}

/// Fixed original preview15/cache8/image1; same existing coarse entrance.
/// This controlled component/Graph evidence is not physical terminal acceptance.
fn observe_original_write24() {
    use super::cache_marker::{self, CacheInvalidation};
    use super::graph_renderer::DisplayGraph;
    use super::render_utils::{TerminalPresentationSettings, resolve_image_options};
    use super::write_renderer::{WriteComponent, WriteResult};
    use ara_agent::{AgentEvent, SessionValues};
    use ara_rpc::value_heap::{NoRuntime, ValueHeap};
    let theme = Arc::new(Theme::dark());
    let frame = FrameContext { columns: 120, ..Default::default() };
    let mut opts = options(true, false);
    fn pending(text: &str) -> WriteComponent {
        let mut c = WriteComponent::default();
        c.update_args(super::write_renderer::args_from_json(&serde_json::json!({"path":"/tmp/inc.ts","content":text})));
        c
    }
    fn has(text: &str, n: usize) -> bool {
        text.contains(&format!("line {n} "))
            || text.contains(&format!("line {n}\n"))
            || text.lines().any(|l| l.trim_end_matches([' ', '│']).ends_with(&format!("line {n}")))
    }
    let lines = |count: usize| (1..=count).map(|n| format!("line {n}")).collect::<Vec<_>>().join("\n");
    // B-affdab4dba shared render state through 5→12→13→25→40.
    let mut c = pending("");
    for n in [5, 12, 13, 25, 40] {
        c.update_args(super::write_renderer::args_from_json(
            &serde_json::json!({"path":"/tmp/inc.ts","content":lines(n)}),
        ));
        let text = stripped(&c.render(120, &frame, opts, &theme));
        assert!(has(&text, n));
        let start = n.saturating_sub(12);
        if start > 0 {
            assert!(!has(&text, start));
            assert!(has(&text, start + 1));
            contains(&text, &format!("{start} earlier line"));
        } else {
            assert!(has(&text, 1));
            excludes(&text, "earlier line");
        }
    }
    // B-5e7a041ffd split-based reference size battery; same state is reused.
    let mut c = pending("");
    for n in [1, 2, 3, 11, 12, 13, 40, 41] {
        let content = lines(n);
        c.update_args(super::write_renderer::args_from_json(
            &serde_json::json!({"path":"/tmp/inc.ts","content":content}),
        ));
        let text = stripped(&c.render(120, &frame, opts, &theme));
        for row in content.replace('\r', "").split('\n').skip(n.saturating_sub(12)) {
            contains(&text, row);
        }
        if n > 12 {
            let hidden = n - 12;
            contains(&text, &format!("… ({hidden} earlier line{})", if hidden == 1 { "" } else { "s" }));
        }
    }
    // B-8585e8783f: instrument the actual guard length, not a full payload hash.
    let first = vec!["x".repeat(64); 2000].join("\n");
    let mut c = pending(&first);
    c.render(120, &frame, opts, &theme);
    c.update_args(super::write_renderer::args_from_json(
        &serde_json::json!({"path":"/tmp/inc.ts","content":format!("{first}\nlast")}),
    ));
    c.render(120, &frame, opts, &theme);
    assert!(c.max_append_guard_units() <= 64);
    assert!(c.max_append_guard_units() < first.len());
    // B-9c30ea9550 CRLF tail and exact original line numbers.
    let mut c = pending(&lines(20).replace('\n', "\r\n"));
    let text = stripped(&c.render(120, &frame, opts, &theme));
    excludes(&text, "\r");
    contains(&text, "… (8 earlier lines)");
    assert!(!has(&text, 8));
    assert!(has(&text, 9));
    assert!(has(&text, 20));
    // B-0110bc2cd1 trailing newline contributes final empty row14.
    let mut c = pending(&format!("{}\n", lines(13)));
    let text = stripped(&c.render(120, &frame, opts, &theme));
    contains(&text, "… (2 earlier lines)");
    assert!(has(&text, 13));
    assert!(!has(&text, 2));
    // B-de7376b082 carriage-return-only has the same normalized empty frame.
    assert_eq!(pending("").render(120, &frame, opts, &theme), pending("\r\r").render(120, &frame, opts, &theme));
    // B-9addda8892 restarted longer nonappend stream resets count/content.
    let mut c = pending("alpha 1\nalpha 2");
    c.render(120, &frame, opts, &theme);
    c.update_args(super::write_renderer::args_from_json(
        &serde_json::json!({"path":"/tmp/inc.ts","content":format!("beta {}\nbeta 2","x".repeat(100))}),
    ));
    let text = stripped(&c.render(120, &frame, opts, &theme));
    excludes(&text, "earlier line");
    contains(&text, "beta");
    excludes(&text, "alpha");
    // B-6363ecfef3 append across unfinished CR retains lines1..4.
    let mut c = pending("line 1\r\nline 2\r");
    c.render(120, &frame, opts, &theme);
    c.update_args(super::write_renderer::args_from_json(
        &serde_json::json!({"path":"/tmp/inc.ts","content":"line 1\r\nline 2\r\nline 3\r\nline 4"}),
    ));
    let text = stripped(&c.render(120, &frame, opts, &theme));
    assert!(has(&text, 1));
    assert!(has(&text, 4));
    excludes(&text, "earlier line");
    // B-22ed12b19e Ctrl+O lifts pending last12 cap; full head and tail.
    let mut c = pending(&lines(40));
    let collapsed = c.render(80, &frame, opts, &theme);
    let text = stripped(&collapsed);
    assert!(has(&text, 40));
    assert!(!has(&text, 1));
    contains(&text, "earlier line");
    opts.expanded = true;
    let expanded = c.render(80, &frame, opts, &theme);
    let text = stripped(&expanded);
    assert!(has(&text, 1));
    assert!(has(&text, 40));
    excludes(&text, "earlier line");
    assert!(expanded.len() > collapsed.len());
    opts.expanded = false;
    // B-c4b28b8710 short pending content stays uncapped.
    let text = stripped(&pending(&lines(4)).render(80, &frame, opts, &theme));
    assert!(has(&text, 1));
    assert!(has(&text, 4));
    excludes(&text, "earlier line");
    // B-e4c5d04807 same body highlights once across widths and spinner frames.
    let mut c = pending("const a = 1;\nconst b = 2;");
    c.render(80, &frame, opts, &theme);
    c.render(120, &frame, opts, &theme);
    assert_eq!(c.highlight_calls, 1);
    opts.spinner_frame = Some(1);
    c.render(120, &frame, opts, &theme);
    assert_eq!(c.highlight_calls, 1);

    let heap = ValueHeap::new().unwrap();
    let values = SessionValues::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
    let g = DisplayGraph::base(&values);
    // B-ab61bf0142/B-a7aa176dcb use the original same-domain Array, not JSON.
    let root = heap
        .import_serde(
            &serde_json::json!({"path":"/tmp/runtime-content.ts","content":["object first\r\nobject second"]}),
        )
        .unwrap();
    let mut c = WriteComponent::default();
    c.update_args(super::write_renderer::args_from_graph(&g, &root).unwrap());
    opts.expanded = true;
    let text = stripped(&c.render(120, &frame, opts, &theme));
    contains(&text, "object first");
    contains(&text, "object second");
    contains(&text, "2 object second");
    excludes(&text, "\r");
    let root = heap
        .import_serde(
            &serde_json::json!({"path":"/tmp/runtime-content.ts","content":["merged first\r\nmerged second"]}),
        )
        .unwrap();
    c.update_args(super::write_renderer::args_from_graph(&g, &root).unwrap());
    c.update_result(
        WriteResult {
            text: WireText("Wrote /tmp/runtime-content.ts".into()),
            resolved_path: Some("/tmp/runtime-content.ts".into()),
            ..Default::default()
        },
        true,
    );
    opts.is_partial = false;
    let text = stripped(&c.render(120, &frame, opts, &theme));
    contains(&text, "merged first");
    contains(&text, "merged second");
    contains(&text, "2 lines");
    contains(&text, "2 merged second");
    excludes(&text, "\r");
    // B-9c7be8c9a1 partial progress excludes diagnostics and clips tabs/long tail.
    let progress = format!("Writing 12 bytes to tab\tpath/{}UNTRUNCATED_TAIL_SENTINEL.ts...", "segment/".repeat(20));
    let mut c = pending("const x = 1;");
    c.update_result(
        WriteResult {
            text: WireText(progress.into()),
            diagnostics: Some(Diagnostics {
                errored: true,
                summary: WireText("1 error".into()),
                messages: vec!["diagnostic sentinel".into()],
            }),
            ..Default::default()
        },
        false,
    );
    opts.expanded = false;
    opts.is_partial = true;
    let text = stripped(&c.render(100, &frame, opts, &theme));
    contains(&text, "Writing 12 bytes to tab");
    excludes(&text, "\t");
    excludes(&text, "UNTRUNCATED_TAIL_SENTINEL");
    excludes(&text, "diagnostic sentinel");
    // B-78bde384cf raw buffer Symbol grows without provider args replacement.
    let raw = serde_json::json!({"role":"assistant","content":[{"type":"toolCall","name":"write","id":"w","arguments":{"path":"/tmp/foo.ts","content":"line before throttle"}}],"usage":{},"api":"openai-completions","provider":"fixture","model":"fixture","stopReason":"stop","timestamp":0});
    let partial = heap.import_serde(&raw).unwrap();
    let content = g.get(&partial, "content").unwrap();
    let block = g.get(&content, "0").unwrap();
    let key = ara_ai::graph_values::streaming_partial_json_key(&heap).unwrap();
    heap.define_own_data(
        &block,
        key.clone(),
        heap.string("{\"path\":\"/tmp/foo.ts\",\"content\":\"line before throttle\\nGROWN_TAIL_SENTINEL".into())
            .unwrap(),
        g.runtime(),
    )
    .unwrap();
    let mut controller = super::event_controller::EventController::new(theme.clone(), EditMode::Hashline);
    let generation = controller.begin_run(EditMode::Hashline);
    let event = AgentEvent::ActualMessageUpdate {
        message: ara_agent::AgentMessage::from_raw(raw.clone(), None).unwrap(),
        event: ara_ai::AssistantMessageEvent::Start { partial: partial.clone() },
    };
    controller.consume(generation, &event, Some(&values)).unwrap();
    controller.render_blocks(&frame, 0);
    let tool = controller.tools.get_mut(&WireString::from("w")).unwrap();
    contains(&stripped(&tool.render(100, &frame, 0)), "GROWN_TAIL_SENTINEL");
    heap.define_own_data(
        &block,
        key,
        heap.string(
            "{\"path\":\"/tmp/foo.ts\",\"content\":\"line before throttle\\nGROWN_TAIL_SENTINEL\\nSAME_ARGS_NEW_RAW"
                .into(),
        )
        .unwrap(),
        g.runtime(),
    )
    .unwrap();
    controller.consume(generation, &event, Some(&values)).unwrap();
    controller.toggle_expanded_in(Some(&g));
    controller.invalidate_in(Some(&g));
    let tool = controller.tools.get_mut(&WireString::from("w")).unwrap();
    contains(&stripped(&tool.render(100, &frame, 0)), "SAME_ARGS_NEW_RAW");
    // Same actual WRITE identity receives partial then final; no EDIT parser.
    let result=heap.import_serde(&serde_json::json!({"content":[{"type":"text","text":"Writing..."}],"details":{"resolvedPath":"/tmp/foo.ts","diagnostics":{"errored":true,"summary":"1 error","messages":["diagnostic sentinel"]}}})).unwrap();
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionUpdate {
                tool_call_id: "w".into(),
                tool_name: "write".into(),
                partial: result.clone(),
                args: g.get(&block, "arguments").unwrap(),
            },
            Some(&values),
        )
        .unwrap();
    let tool = controller.tools.get_mut(&WireString::from("w")).unwrap();
    excludes(&stripped(&tool.render(100, &frame, 0)), "diagnostic sentinel");
    controller
        .consume(
            generation,
            &AgentEvent::ActualToolExecutionEnd {
                tool_call_id: "w".into(),
                tool_name: "write".into(),
                result,
                is_error: false,
            },
            Some(&values),
        )
        .unwrap();
    assert!(!controller.tools.get(&WireString::from("w")).unwrap().component.options.is_partial);

    fn usage(read: u64, write: u64, input: u64) -> ara_ai::Usage {
        ara_ai::Usage { cache_read: Some(read), cache_write: Some(write), input: Some(input), ..Default::default() }
    }
    // Original cache detector seven assertions (B-84937214a5..B-626e4bde24).
    assert_eq!(cache_marker::detect(None, &usage(0, 50_000, 2)), None);
    assert_eq!(
        cache_marker::detect(Some(&usage(49_837, 980, 0)), &usage(0, 50_900, 99)),
        Some(CacheInvalidation { reprocessed_tokens: 50_999 })
    );
    assert_eq!(cache_marker::detect(Some(&usage(0, 50_900, 99)), &usage(0, 51_113, 16)), None);
    assert_eq!(cache_marker::detect(Some(&usage(50_900, 980, 0)), &usage(50_900, 3459, 2)), None);
    assert_eq!(cache_marker::detect(Some(&usage(40_789, 0, 1069)), &usage(0, 0, 43_102)), None);
    assert_eq!(cache_marker::detect(Some(&usage(0, 0, 500)), &usage(0, 0, 600)), None);
    assert_eq!(cache_marker::detect(Some(&usage(40_000, 1000, 0)), &usage(0, 0, 12)), None);
    // B-2217bbb7b4 exact slim three-row shape at80.
    let rows = cache_marker::render(&CacheInvalidation { reprocessed_tokens: 50_999 }, 80, &theme);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0], "");
    assert_eq!(rows[2], "");
    assert!(width(&rows[1]) > 0);
    assert!(width(&rows[1]) < 80);
    // Unknown usage stays unknown; disabled marker still records warm prefix.
    assert_eq!(cache_marker::detect(Some(&usage(49_837, 980, 0)), &ara_ai::Usage::unknown()), None);
    let mut prev = None;
    assert_eq!(cache_marker::observe(&mut prev, usage(50_000, 1000, 0), &Default::default()), None);
    assert_eq!(
        cache_marker::observe(
            &mut prev,
            usage(0, 50_900, 99),
            &TerminalPresentationSettings { cache_miss_marker: true, ..Default::default() }
        ),
        Some(CacheInvalidation { reprocessed_tokens: 50_999 })
    );
    // B-8812fd86aa uninitialized settings schema100/positive-height.
    let image = resolve_image_options(None, None);
    assert_eq!(image.max_width_cells, 100.0);
    assert!(image.max_height_cells.unwrap() > 0.0);
    assert_eq!(image.max_height_cells, Some(20.0));
    assert_eq!(resolve_image_options(None, Some(30)).max_height_cells, Some(18.0));
    assert_eq!(
        resolve_image_options(Some(&TerminalPresentationSettings { image_rows: 0.0, ..Default::default() }), None)
            .max_height_cells,
        None
    );
    // SOURCE POST P1: authoritative failures never display success suffixes.
    let mut failed = pending("one\ntwo");
    failed.update_result(
        WriteResult {
            error: true,
            made_executable: true,
            text: WireText("failure sentinel".into()),
            ..Default::default()
        },
        false,
    );
    let failed = stripped(&failed.render(120, &frame, options(false, false), &theme));
    contains(&failed, "failure sentinel");
    excludes(&failed, "2 lines");
    excludes(&failed, "made executable!");
    // P3: fixed permissive content extractor, full JS whitespace, UTF16 and
    // repeated content fields. Path is owned by the fresh parse, not extraction.
    for (raw, expected) in [
        (r#"{"content":"a\u12Xz"}"#, "au12Xz"),
        ("{\"content\"\u{FEFF}:\u{A0}\"space\"}", "space"),
        (r#"{"content":"first","content":""}"#, ""),
        (r#"{"content":"first","content":"\qtail"}"#, "qtail"),
    ] {
        let full = heap.import_serde(&serde_json::json!({"path":"provider-path","content":"stale"})).unwrap();
        let decoded = super::write_renderer::decode_graph_args(&g, &full, &raw.into()).unwrap();
        assert_eq!(g.string(&g.get(&decoded, "content").unwrap()).unwrap(), WireString::from(expected));
        assert_eq!(g.string(&g.get(&decoded, "path").unwrap()).unwrap(), WireString::from("provider-path"));
    }
    let decoded =
        super::write_renderer::decode_graph_args(&g, &heap.object().unwrap(), &r#"{"content":"\uD800"}"#.into())
            .unwrap();
    assert_eq!(g.string(&g.get(&decoded, "content").unwrap()).unwrap().units(), &[0xd800]);
    // P4: exact binary64 decimal tie and the non-tie erased by naive n*10.
    contains(&stripped(&cache_marker::render(&CacheInvalidation { reprocessed_tokens: 2250 }, 80, &theme)), "2.3K");
    contains(&stripped(&cache_marker::render(&CacheInvalidation { reprocessed_tokens: 2550 }, 80, &theme)), "2.5K");
    // P2: actual message update/end associates the marker with its assistant
    // segment, before its tool card, even though that card was inserted early.
    let mut controller = super::event_controller::EventController::new(theme.clone(), EditMode::Hashline);
    controller.set_presentation(TerminalPresentationSettings { cache_miss_marker: true, ..Default::default() });
    let generation = controller.begin_run(EditMode::Hashline);
    let mut warm = raw.clone();
    warm["content"] = serde_json::json!([]);
    warm["usage"] = serde_json::json!({"input":0,"cacheRead":50000,"cacheWrite":1000});
    controller
        .consume(
            generation,
            &AgentEvent::MessageEnd {
                message: ara_agent::AgentMessage::from_graph(heap.import_serde(&warm).unwrap(), None).unwrap(),
            },
            Some(&values),
        )
        .unwrap();
    let mut cold = raw.clone();
    cold["content"] = serde_json::json!([
        {"type":"text","text":"before tool"},
        {"type":"toolCall","name":"write","id":"anchored","arguments":{"path":"a.ts","content":"x"}}]);
    cold["usage"] = serde_json::json!({"input":99,"cacheRead":0,"cacheWrite":50900});
    let partial = heap.import_serde(&cold).unwrap();
    controller
        .consume(
            generation,
            &AgentEvent::ActualMessageUpdate {
                message: ara_agent::AgentMessage::from_graph(partial.clone(), None).unwrap(),
                event: ara_ai::AssistantMessageEvent::Start { partial: partial.clone() },
            },
            Some(&values),
        )
        .unwrap();
    controller
        .consume(
            generation,
            &AgentEvent::MessageEnd { message: ara_agent::AgentMessage::from_graph(partial, None).unwrap() },
            Some(&values),
        )
        .unwrap();
    let keys = controller.transcript.block_keys();
    let marker = keys.iter().position(|k| matches!(k, super::transcript::BlockKey::CacheMarker(_))).unwrap();
    let tool = keys
        .iter()
        .position(|k| matches!(k,super::transcript::BlockKey::Tool(_,id) if id.equals_ascii("anchored")))
        .unwrap();
    assert!(marker < tool, "assistant marker must precede its early tool card: {keys:?}");
    // Existing coarse group: fixed 30fps/min3/ceil(backlog/8), geometric
    // parse throttle, stable repeated paints and the actual final flush path.
    let now = std::time::Instant::now();
    let tick = std::time::Duration::from_nanos(1_000_000_000 / 30);
    let root = heap.import_serde(&serde_json::json!({"path":"a.ts","content":"stale"})).unwrap();
    let block = heap.object().unwrap();
    let raw_key = ara_ai::graph_values::streaming_partial_json_key(&heap).unwrap();
    let prefix = format!("{{\"path\":\"a.ts\",\"content\":\"{}", "x".repeat(32768));
    heap.define_own_data(&block, raw_key.clone(), heap.string(prefix.as_str().into()).unwrap(), g.runtime()).unwrap();
    let mut paced = WriteComponent::default();
    paced.raw_block = Some(block.clone());
    paced.rebuild_live(&g, &root, now, true).unwrap();
    let initial = paced.reveal_observation();
    assert_eq!(initial, (prefix.len(), 1));
    let grown = format!("{prefix}{}", "y".repeat(64));
    heap.define_own_data(&block, raw_key.clone(), heap.string(grown.as_str().into()).unwrap(), g.runtime()).unwrap();
    paced.rebuild_live(&g, &root, now, true).unwrap();
    assert_eq!(paced.reveal_observation(), initial);
    assert_eq!(paced.reveal_deadline(), Some(now + tick));
    paced.advance_reveal(&g, &root, now + tick - std::time::Duration::from_nanos(1)).unwrap();
    assert_eq!(paced.reveal_observation(), initial);
    paced.advance_reveal(&g, &root, now + tick).unwrap();
    // Growth floor is 1024 at this size, rather than a fixed 256-byte gate.
    assert_eq!(paced.reveal_observation(), (prefix.len() + 8, 1));
    assert_eq!(paced.args.content.0.len(), 32768 + 8);
    assert_eq!(paced.reveal_deadline(), Some(now + tick + tick));
    // Turning smoothing off snaps received text but retains the same parse gate.
    paced.rebuild_live(&g, &root, now + tick, false).unwrap();
    assert_eq!(paced.reveal_observation(), (grown.len(), 1));
    assert_eq!(paced.reveal_deadline(), None);
    let regrown = format!("{grown}zzz");
    heap.define_own_data(&block, raw_key.clone(), heap.string(regrown.as_str().into()).unwrap(), g.runtime()).unwrap();
    paced.rebuild_live(&g, &root, now + tick, true).unwrap();
    assert_eq!(paced.reveal_observation(), (grown.len(), 1));
    paced.advance_reveal(&g, &root, now + tick + tick).unwrap();
    assert_eq!(paced.reveal_observation(), (regrown.len(), 1));
    // Normalized full content and highlight output are reused on repeated paint.
    let expanded = options(true, true);
    paced.render(80, &frame, expanded, &theme);
    paced.render(120, &frame, expanded, &theme);
    assert_eq!(paced.normalization_calls, 1);
    assert_eq!(paced.highlight_calls, 1);
    paced.update_args(paced.args.clone());
    paced.render(80, &frame, expanded, &theme);
    assert_eq!(paced.normalization_calls, 1);
    paced.update_result(WriteResult::default(), true);
    paced.render(80, &frame, options(false, true), &theme);
    paced.render(120, &frame, options(false, true), &theme);
    assert_eq!(paced.normalization_calls, 2);
    // A reveal boundary landing on a high surrogate includes its low partner.
    let short = format!("{{\"path\":\"a.ts\",\"content\":\"{}", "x".repeat(512));
    heap.define_own_data(&block, raw_key.clone(), heap.string(short.as_str().into()).unwrap(), g.runtime()).unwrap();
    let mut unicode = WriteComponent::default();
    unicode.raw_block = Some(block.clone());
    unicode.rebuild_live(&g, &root, now, true).unwrap();
    let emoji = format!("{short}ab😀xyz");
    heap.define_own_data(&block, raw_key.clone(), heap.string(emoji.as_str().into()).unwrap(), g.runtime()).unwrap();
    unicode.rebuild_live(&g, &root, now, true).unwrap();
    unicode.advance_reveal(&g, &root, now + tick).unwrap();
    assert_eq!(unicode.reveal_observation(), (short.len() + 4, 1));
    assert!(unicode.args.content.0.units().ends_with(&[0xd83d, 0xde00]));
    // Escape state also survives separate prefixes with no full reparse.
    let escaped = format!("{short}\\uD8");
    heap.define_own_data(&block, raw_key.clone(), heap.string(escaped.as_str().into()).unwrap(), g.runtime()).unwrap();
    let mut escape = WriteComponent::default();
    escape.raw_block = Some(block.clone());
    escape.rebuild_live(&g, &root, now, true).unwrap();
    heap.define_own_data(&block, raw_key.clone(), heap.string(format!("{escaped}00x").into()).unwrap(), g.runtime())
        .unwrap();
    escape.rebuild_live(&g, &root, now, true).unwrap();
    escape.advance_reveal(&g, &root, now + tick).unwrap();
    assert!(escape.args.content.0.units().ends_with(&[0xd800, 120]));
    assert_eq!(escape.reveal_observation().1, 1);
    // Actual cumulative update -> same Graph final MessageEnd flushes the
    // unrevealed tail, without changing the provider's stale arguments object.
    let partial = heap.import_serde(&raw).unwrap();
    let block = g.get(&g.get(&partial, "content").unwrap(), "0").unwrap();
    heap.define_own_data(&block, raw_key.clone(), heap.string(short.as_str().into()).unwrap(), g.runtime()).unwrap();
    let message = ara_agent::AgentMessage::from_graph(partial.clone(), None).unwrap();
    let event = AgentEvent::ActualMessageUpdate {
        message: message.clone(),
        event: ara_ai::AssistantMessageEvent::Start { partial },
    };
    let mut controller = super::event_controller::EventController::new(theme.clone(), EditMode::Hashline);
    let generation = controller.begin_run(EditMode::Hashline);
    controller.sync_presentation(0, now);
    controller.consume(generation, &event, Some(&values)).unwrap();
    heap.define_own_data(
        &block,
        raw_key,
        heap.string(format!("{short}FINAL_FLUSH_SENTINEL").into()).unwrap(),
        g.runtime(),
    )
    .unwrap();
    controller.consume(generation, &event, Some(&values)).unwrap();
    assert!(
        !controller.tools[&WireString::from("w")]
            .write
            .as_ref()
            .unwrap()
            .args
            .content
            .0
            .to_utf8()
            .unwrap()
            .contains("FINAL_FLUSH_SENTINEL")
    );
    controller.consume(generation, &AgentEvent::MessageEnd { message }, Some(&values)).unwrap();
    let flushed = &controller.tools[&WireString::from("w")];
    assert!(flushed.write.as_ref().unwrap().args.content.0.to_utf8().unwrap().ends_with("FINAL_FLUSH_SENTINEL"));
    assert_eq!(flushed.reveal_deadline(), None);
    assert!(flushed.args_complete);
    assert_eq!(
        g.string(&g.get(&g.get(&block, "arguments").unwrap(), "content").unwrap()).unwrap(),
        WireString::from("line before throttle")
    );
    // S1 actual controller: a failed content getter or String callback is an
    // effect receipt, not a reason to replay the same root on final completion.
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError, ValueHandle,
        ValueRuntime,
    };
    struct WriteEffects {
        failed: AtomicUsize,
        recovered: AtomicUsize,
        reason: ValueHandle,
        text: ValueHandle,
    }
    impl ValueRuntime for WriteEffects {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(7855), .. } => {
                    self.failed.fetch_add(1, Ordering::SeqCst);
                    Err(ValueError::Thrown(self.reason.clone()))
                }
                RuntimeRequest::Call { capability: CapabilityId(7856), .. } => {
                    self.recovered.fetch_add(1, Ordering::SeqCst);
                    Ok(RuntimeReply::Value(self.text.clone()))
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
    }
    for getter in [true, false] {
        for recover in [false, true] {
            let heap = ValueHeap::new().unwrap();
            let runtime = Arc::new(WriteEffects {
                failed: AtomicUsize::new(0),
                recovered: AtomicUsize::new(0),
                reason: heap.object().unwrap(),
                text: heap.string("recovered content".into()).unwrap(),
            });
            let values = SessionValues::new(heap.clone(), runtime.clone()).unwrap();
            let graph = DisplayGraph::base(&values);
            let partial = heap.import_serde(&raw).unwrap();
            let block = graph.get(&graph.get(&partial, "content").unwrap(), "0").unwrap();
            let failed_args = graph.get(&block, "arguments").unwrap();
            if getter {
                heap.define_own(
                    &failed_args,
                    "content".into(),
                    PropertyDescriptor::Accessor {
                        get: Some(CallableTarget::Host(CapabilityId(7855))),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    graph.runtime(),
                )
                .unwrap();
            } else {
                let content = heap.object().unwrap();
                heap.define_own_data(
                    &content,
                    "toString".into(),
                    heap.function(CapabilityId(7855)).unwrap(),
                    graph.runtime(),
                )
                .unwrap();
                heap.define_own_data(&failed_args, "content".into(), content, graph.runtime()).unwrap();
            }
            let message = ara_agent::AgentMessage::from_graph(partial.clone(), None).unwrap();
            let event = AgentEvent::ActualMessageUpdate {
                message: message.clone(),
                event: ara_ai::AssistantMessageEvent::Start { partial },
            };
            let mut controller = super::event_controller::EventController::new(theme.clone(), EditMode::Hashline);
            let generation = controller.begin_run(EditMode::Hashline);
            controller.consume(generation, &event, Some(&values)).unwrap();
            assert_eq!(runtime.failed.load(Ordering::SeqCst), 1, "one initial getter/String effect");
            let receipts = controller.take_display_errors();
            assert_eq!(receipts.len(), 1);
            assert!(matches!(&receipts[0].1,ValueError::Thrown(reason) if reason.same_identity(&runtime.reason)));
            // A repeated cumulative event has no new authoritative args root.
            controller.consume(generation, &event, Some(&values)).unwrap();
            assert_eq!(runtime.failed.load(Ordering::SeqCst), 1);
            controller.consume(generation, &AgentEvent::MessageEnd { message }, Some(&values)).unwrap();
            let id = WireString::from("w");
            let component = &controller.tools[&id];
            assert!(component.constructor_failed && component.args_complete);
            assert!(component.graph_args_root.as_ref().unwrap().same_identity(&failed_args));
            assert!(component.write.as_ref().unwrap().raw_block.is_none());
            assert_eq!(component.reveal_deadline(), None);
            assert!(controller.take_display_errors().is_empty());
            controller
                .tools
                .get_mut(&id)
                .unwrap()
                .render_in(&graph, 120, &frame, 1, usize::MAX, now + std::time::Duration::from_secs(1))
                .unwrap();
            assert_eq!(
                runtime.failed.load(Ordering::SeqCst),
                1,
                "same Graph end/timer paint cannot replay the failed input"
            );
            if recover {
                // A different authoritative args identity can construct once.
                let final_root = heap.import_serde(&raw).unwrap();
                let final_block = graph.get(&graph.get(&final_root, "content").unwrap(), "0").unwrap();
                let final_args = graph.get(&final_block, "arguments").unwrap();
                heap.define_own(
                    &final_args,
                    "content".into(),
                    PropertyDescriptor::Accessor {
                        get: Some(CallableTarget::Host(CapabilityId(7856))),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    graph.runtime(),
                )
                .unwrap();
                controller
                    .consume(
                        generation,
                        &AgentEvent::MessageEnd {
                            message: ara_agent::AgentMessage::from_graph(final_root, None).unwrap(),
                        },
                        Some(&values),
                    )
                    .unwrap();
                let component = &controller.tools[&id];
                assert!(!component.constructor_failed);
                assert!(component.graph_args_root.as_ref().unwrap().same_identity(&final_args));
                assert_eq!(component.write.as_ref().unwrap().args.content.0, WireString::from("recovered content"));
                assert_eq!(runtime.recovered.load(Ordering::SeqCst), 1, "final WRITE upsert must not construct twice");
                assert!(controller.take_display_errors().is_empty());
            }
            controller.seal_run();
            controller
                .tools
                .get_mut(&id)
                .unwrap()
                .render_in(&graph, 120, &frame, 2, usize::MAX, now + std::time::Duration::from_secs(2))
                .unwrap();
            assert_eq!(
                runtime.failed.load(Ordering::SeqCst),
                1,
                "seal/paint retains the original failure effect count"
            );
            assert_eq!(runtime.recovered.load(Ordering::SeqCst), usize::from(recover));
            assert!(controller.tools[&id].sealed);
            assert_eq!(controller.tools[&id].reveal_deadline(), None);
            assert_eq!(receipts.len(), 1, "the original independent error receipt remains available");
        }
    }
}
fn excludes(text: &str, unexpected: &str) {
    assert!(!text.contains(unexpected), "unexpected {unexpected:?} in:\n{text}");
}

#[test]
fn edit_renderer_original_31_observations() {
    // Independent helpers retain strict assertions and report all failures in
    // one original coarse family, avoiding a new build for each hidden fixture.
    let helpers: [(&str, fn()); 7] = [
        ("WRITE original24 and actual event wiring", observe_original_write24),
        ("prompt races", super::driver::observe_passive_prompt_races),
        ("retained callbacks", super::graph_renderer::observe_retained_dynamic_callbacks),
        ("display queue and Session", super::evaluation::observe_display_queue_and_session_callbacks),
        ("graph admission", super::event_controller::observe_graph_admission),
        ("links and digits", observe_link_and_ascii_digit_contracts),
        ("terminal Unicode", observe_fixed_terminal_unicode_contracts),
    ];
    let mut failures = Vec::new();
    for (name, observe) in helpers {
        if std::panic::catch_unwind(observe).is_err() {
            failures.push(name);
        }
    }
    let original_observations = std::panic::catch_unwind(|| {
        // 01 shows the target path from partial JSON while edit args stream
        let mut c = call(
            serde_json::json!({"__partialJson":"{\"path\":\"packages/coding-agent/src/edit/renderer.ts\",\"old_string\":\"before"}),
            EditMode::Replace,
            false,
        );
        contains(&render(&mut c, 160), "packages/coding-agent/src/edit/renderer.ts");
        // 02 windows the expanded streaming diff to the viewport tail
        let make_diff = |count: usize| {
            (0..count)
                .map(|i| if i == 0 { "-head-line-1".to_owned() } else { format!("+tail-line-{}", i + 1) })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let make_preview = |diff: String, expanded: bool| {
            let mut c = call(
                serde_json::json!({"file_path":"/tmp/preview.ts","previewDiff":diff}),
                EditMode::Replace,
                expanded,
            );
            stripped(&rows(&mut c, 200, 50))
        };
        let collapsed = make_preview(make_diff(20), false);
        contains(&collapsed, "tail-line-20");
        excludes(&collapsed, "head-line-1");
        contains(&collapsed, "content above");
        contains(&collapsed, "(preview)");
        let expanded = make_preview(make_diff(20), true);
        contains(&expanded, "head-line-1");
        contains(&expanded, "tail-line-20");
        excludes(&expanded, "content above");
        excludes(&expanded, "(preview)");
        let tall = make_preview(make_diff(40), true);
        contains(&tall, "tail-line-40");
        excludes(&tall, "head-line-1");
        contains(&tall, "content above");
        // 03 does not report a leading blank line as hidden content
        let mut c = call(
            serde_json::json!({"file_path":"/tmp/leading-blank.ts","previewDiff":"\n+1|first-added\n+2|second-added"}),
            EditMode::Replace,
            false,
        );
        let text = render(&mut c, 200);
        contains(&text, "first-added");
        contains(&text, "second-added");
        excludes(&text, "content above");
        // 04 uses a count-free marker for a discarded streaming prefix
        let mut lines = vec!["@@ -1,10000 +1,12 @@".to_owned()];
        lines.extend((1..=10_000).map(|n| format!("-hidden-line-{n}")));
        lines.extend((1..=12).map(|n| format!("+visible-tail-{n}")));
        lines.extend([String::new(), String::new()]);
        let mut c = call(
            serde_json::json!({"file_path":"/tmp/large-preview.ts","previewDiff":lines.join("\n")}),
            EditMode::Replace,
            false,
        );
        let text = render(&mut c, 200);
        contains(&text, "content above");
        contains(&text, "visible-tail-12");
        excludes(&text, "hidden-line-10000");
        excludes(&text, "more hunks");
        excludes(&text, "more lines above");
        // 05 uses hashline input headers for streaming call path without apply_patch errors
        let mut c = call(
            serde_json::json!({"input":"[packages/coding-agent/src/edit/renderer.ts]\nPUT >$:\n+// preview"}),
            EditMode::Hashline,
            false,
        );
        let text = render(&mut c, 160);
        contains(&text, "packages/coding-agent/src/edit/renderer.ts");
        excludes(&text, "The first line of the patch must be");
        // 06 keeps the path observation with target sloppy *** Edit File/Find syntax
        let mut c = call(
            serde_json::json!({"input":"*** Edit File: src/engine/disk.rs\n*** Find\nfn parse_disk_ref("}),
            EditMode::Sloppy,
            false,
        );
        contains(&render(&mut c, 160), "src/engine/disk.rs");
        // 07 keeps the extra-file count observation with target sloppy section syntax
        let mut c = call(
            serde_json::json!({"input":"*** Edit File: a.ts\n*** Find\nfoo\n*** Edit File: b.ts\n*** Find\nbar"}),
            EditMode::Sloppy,
            false,
        );
        let text = render(&mut c, 160);
        contains(&text, "a.ts");
        contains(&text, "(+1 more)");
        // 08 shows hashline envelope target path while preview diff is not computable yet
        let mut c = tool(
            "edit",
            serde_json::json!({"input":"*** Begin Patch\n[crates/pi-natives/src/shell.rs]\nPUT >$:\n+pub fn streaming_preview() {"}),
            EditMode::Hashline,
        );
        let text = tool_render(&mut c, 160);
        contains(&text, "crates/pi-natives/src/shell.rs");
        for no in ["PUT >$:", "+pub fn streaming_preview() {", "*** Begin Patch"] {
            excludes(&text, no);
        }
        // 09 recognizes compact and quoted hashline input headers
        for (input, path, expanded) in [
            ("[foo bar.ts]\nPUT <1:\n+// preview", "foo bar.ts", true),
            ("['baz qux.ts']\nPUT <1:\n+// preview", "baz qux.ts", false),
        ] {
            let mut c = call(serde_json::json!({"input":input}), EditMode::Hashline, expanded);
            contains(&render(&mut c, 160), path);
        }
        // 10 strips bracket delimiters from hashline input headers
        for (input, path) in [
            (
                "[packages/coding-agent/src/slash-commands/builtin-registry.ts]\nPUT <1:\n+// preview",
                "packages/coding-agent/src/slash-commands/builtin-registry.ts",
            ),
            ("[a/b/c.ts\nPUT <1:\n+// preview", "a/b/c.ts"),
        ] {
            let mut c = call(serde_json::json!({"input":input}), EditMode::Hashline, true);
            let text = render(&mut c, 160);
            contains(&text, path);
            excludes(&text, &format!("[{path}"));
        }
        // 11 uses hashline input headers for completed single-file result path
        let mut c = complete("", "+1|// preview", false);
        c.args =
            args(serde_json::json!({"input":"[packages/coding-agent/src/edit/renderer.ts]\nPUT >$:\n+// preview"}));
        contains(&render(&mut c, 160), "packages/coding-agent/src/edit/renderer.ts");
        // 12 omits changed-line suffixes from completed edit headers and middle-elides long paths
        let relative = "packages/coding-agent/test/modes/components/transcript-container.test.ts";
        let mut c = complete(relative, "+1│const value = 2;", false);
        let details = c.result.as_mut().unwrap().details.as_mut().unwrap();
        details.first_changed_line = Some(251);
        details.path = Some(format!("/tmp/project/{relative}").into());
        let wide = stripped(&rows(&mut c, 160, 30)[..1]);
        contains(&wide, relative);
        excludes(&wide, ":251");
        let narrow = stripped(&rows(&mut c, 72, 30)[..1]);
        contains(&narrow, "…");
        contains(&narrow, "container.test.ts");
        excludes(&narrow, ":251");
        // 13 caches completed diff rendering across stable frame renders
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let mut c = complete("src/example.ts", "+1│const value = 1;", false);
        c.context.render_diff = Some(Arc::new(move |text, _| {
            count.fetch_add(1, Ordering::Relaxed);
            text.to_owned()
        }));
        c.render(160, &FrameContext::default());
        c.render(120, &FrameContext::default());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(c.theme.stats_colors.load(Ordering::Relaxed), 1);
        c.options.expanded = true;
        c.render(120, &FrameContext::default());
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        assert_eq!(c.theme.stats_colors.load(Ordering::Relaxed), 1);
        // 14 computes hashline preview once a single-line edit finishes streaming
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.ts");
        let old = "export const a = 1;\nexport const b = 2;\nexport const c = 3;\n";
        std::fs::write(&path, old).unwrap();
        let new = old.replace("export const b = 2;", "export const b = 22;");
        let path = path.to_string_lossy();
        let mut c = tool(
            "edit",
            serde_json::json!({"input":"[memory.ts#ABCD]\nPUT 2-2:\n+export const b = 22;"}),
            EditMode::Hashline,
        );
        c.update_stream_preview(vec![preview(&path, old, &new)], false);
        let text = tool_render(&mut c, 160);
        contains(&text, "export const b = 22;");
        excludes(&text, "No changes would be made");
        // 15 renders raw custom hashline input carried only in partialJson
        let old = "export const a = 1;\nexport const b = 2;\n";
        std::fs::write(path.as_ref(), old).unwrap();
        let new = old.replace("export const b = 2;", "export const b = 22;");
        let mut c = tool(
            "edit",
            serde_json::json!({"__partialJson":"[memory.ts#ABCD]\nPUT 2-2:\n+export const b = 22;\n"}),
            EditMode::Hashline,
        );
        c.update_stream_preview(vec![preview(&path, old, &new)], true);
        contains(&tool_render(&mut c, 160), "export const b = 22;");
        // 16 renders native apply_patch preview batches
        let mut c = tool("edit", serde_json::json!({"input":"*** Begin Patch"}), EditMode::ApplyPatch);
        c.update_stream_preview(vec![preview("src/demo.ts", "const value = 1;\n", "const value = 2;\n")], true);
        contains(&tool_render(&mut c, 160), "const value = 2;");
        // 17 uses raw replacement only as replace-mode streaming fallback
        let replacement = args(serde_json::json!({"new_string":"plain streamed text"}));
        contains(
            &sanitize(&streaming_fallback(EditMode::Replace, &replacement, &Theme::dark()).as_str().into()),
            "plain streamed text",
        );
        assert_eq!(streaming_fallback(EditMode::Patch, &replacement, &Theme::dark()), "");
        assert_eq!(
            streaming_fallback(
                EditMode::Hashline,
                &args(serde_json::json!({"input":"plain streamed text"})),
                &Theme::dark()
            ),
            ""
        );
        // 18 uses supplied theme when injected diff renderer unavailable
        let dark = Theme::dark();
        let light = Arc::new(Theme::light());
        assert_ne!(light.fg("toolDiffAdded", "COLOR"), dark.fg("toolDiffAdded", "COLOR"));
        let mut c = complete("demo.ts", "-1|const oldValue = 1;\n\n+2|const newValue = 2;", true);
        c.theme = light.clone();
        let output = rows(&mut c, 160, 30);
        let removed = output.iter().find(|r| r.contains("oldValue")).unwrap();
        let added = output.iter().find(|r| r.contains("newValue")).unwrap();
        contains(removed, light.ansi("toolDiffRemoved"));
        contains(added, light.ansi("toolDiffAdded"));
        // 19 inline stats; no separate metadata/stats row
        let mut c = complete("demo.go", " 115│ ctx\n-116│ old\n+117│ new one\n+118│ new two", false);
        let text = render(&mut c, 160);
        let lines = text.lines().collect::<Vec<_>>();
        contains(lines[0], "demo.go");
        contains(lines[0], "+2");
        contains(lines[0], "-1");
        contains(lines[0], "+2/-1");
        contains(lines[1], "115│ ctx");
        assert_eq!(lines.iter().filter(|r| r.contains("+2/-1")).count(), 1);
        // 20 bounds one oversized completed change hunk
        let diff = (0..1_000).map(|n| format!("+{}│line {n}", n + 1)).collect::<Vec<_>>().join("\n");
        let mut c = complete("demo.ts", &diff, false);
        let text = render(&mut c, 160);
        assert_eq!(text.lines().filter(|r| r.contains("│line ")).count(), 40);
        contains(&text, "+40│line 39");
        excludes(&text, "+41│line 40");
        contains(&text, "960 more lines");
        // 21 bounds completed collapsed diff by rendered rows
        let tail = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".repeat(4);
        let diff = (1..=12)
            .map(|n| format!("-{n}|ROW_{n:02}={tail}\n+{n}|ROW_{n:02}=changed-{tail}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut c = complete("long-lines.txt", &diff, false);
        let output = rows(&mut c, 120, 30);
        assert_eq!(output.len(), 43);
        contains(&stripped(&output), "more lines");
        // 22 completed gutters use zero inherited frame padding
        let mut c = complete("demo.ts", "+1│const renamedIdentifier = computeValueFromSomeVeryLongInputName();", false);
        let output = rows(&mut c, 48, 30).into_iter().map(|r| sanitize(&r.as_str().into())).collect::<Vec<_>>();
        assert!(output.iter().all(|r| width(r) == 48));
        assert!(output[1].starts_with("│+1│"));
        assert!(!output[1].starts_with("│ +1│"));
        // 23 first-file no-change preview cannot leak into multi-file delete result
        let paths = ["scripts/a.ts", "scripts/a.user.md", "scripts/a.system.md"];
        let mut c = complete("", "", false);
        c.result.as_mut().unwrap().details.as_mut().unwrap().per_file_results = Some(
            paths
                .iter()
                .map(|path| EditDetails {
                    path: Some((*path).into()),
                    op: Some(Operation::Delete),
                    old_text: Some("x\n".into()),
                    ..Default::default()
                })
                .collect(),
        );
        c.context.edit_diff_preview = Some(EditPreview::Error("No changes would be made to scripts/a.ts.".into()));
        let text = render(&mut c, 160);
        contains(&text, "Delete");
        excludes(&text, "No changes would be made");
        for path in paths {
            contains(&text, path);
        }
        // 24 move-only uses source → destination, without diff body
        let mut c = complete("", "", false);
        c.args = args(serde_json::json!({"input":"[a.ts#1a2b]\nMV b.ts"}));
        let d = c.result.as_mut().unwrap().details.as_mut().unwrap();
        d.path = Some("b.ts".into());
        d.move_to = Some("b.ts".into());
        d.source_path = Some("a.ts".into());
        c.context.edit_diff_preview = Some(EditPreview::Error("No changes would be made to other.ts.".into()));
        let header = stripped(&rows(&mut c, 160, 30)[..1]);
        for part in ["a.ts", "b.ts", "→"] {
            contains(&header, part);
        }
        excludes(&render(&mut c, 160), "No changes");
        // 25 genuine no-op uses its own result path
        let mut c = complete("scripts/real.ts", "", false);
        c.result.as_mut().unwrap().details.as_mut().unwrap().path = Some("scripts/real.ts".into());
        c.context.edit_diff_preview = Some(EditPreview::Error("No changes would be made to scripts/WRONG.ts.".into()));
        let text = render(&mut c, 160);
        contains(&text, "No changes were made");
        contains(&text, "scripts/real.ts");
        excludes(&text, "WRONG");
        // 26 added continuation rows remain inside blanked dedup gutter
        let old = "    the previous synopsis paragraph rambled across quarterly reconciliation notes enumerating every provisional ledger amendment the archival committee had deferred pending review by the regional custodians during the extended winter recess of the auditing season";
        let new = "    the revised synopsis paragraph now catalogues seasonal festival logistics enumerating lantern shipments drum rehearsals and ribbon inventories that the parade stewards confirmed before dawn, closing with the zephyrQuota tally and the marbledFinale banner";
        let output = replacement_rows(old.into(), new.into(), 100);
        let plain = output.iter().map(|r| sanitize(&r.as_str().into())).collect::<Vec<_>>();
        let tails =
            plain.iter().filter(|r| r.contains("zephyrQuota") || r.contains("marbledFinale")).collect::<Vec<_>>();
        assert!(!tails.is_empty());
        let tail_pattern = regex::Regex::new(r"^│\s+│").unwrap();
        assert!(tails.iter().all(|r| tail_pattern.is_match(r)));
        let body_pattern = regex::Regex::new(r"^│\s*[+-]?\s*\d*│").unwrap();
        assert!(plain[1..plain.len() - 1].iter().all(|r| body_pattern.is_match(r)));
        // 27 ill-formed UTF16 replacement keeps native code-unit input
        let wire = |surrogate| {
            let mut units = "alpha ".encode_utf16().collect::<Vec<_>>();
            units.push(surrogate);
            units.extend(" beta".encode_utf16());
            WireString::from_units(units)
        };
        let plain = stripped(&replacement_rows(wire(0xd800), wire(0xd801), 100));
        contains(&plain, "-42");
        contains(&plain, "+");
        // 28 inverse closes at every wrapped row end
        let output=replacement_rows("    stanza recounts venerable chronicle passages spanning bygone dynasties whose archivists engraved ledgers onto vellum scrolls".into(),"    stanza celebrates luminous festival processions winding through lantern boulevards while drummers herald jubilant choruses beneath cascading ribbons and fireworks".into(),100);
        assert!(output.iter().any(|r| r.contains("│\x1b[7m")), "inverse span must cross wrap");
        assert!(output.iter().all(|r| !inverse_active_at_end(r)));
        // 29/30/31 error prose with bare |, indented |, or digit-only | is generic
        for (prefix, text, pattern) in [
            (
                "| ",
                "| pipe-leading diagnostic output that is quite long and should certainly wrap at the render width because it keeps going on and on with more words than fit in one row of the frame",
                r"^│\| ",
            ),
            (
                "   | ",
                "   | indented bare-pipe diagnostic output that is quite long and should certainly wrap at the render width because it keeps going on and on with more words than fit in one row of the frame",
                r"^│\s+\| ",
            ),
            (
                "123| ",
                "123| numbered pipe-leading diagnostic output that is quite long and should certainly wrap at the render width because it keeps going on and on with more words than fit in one row of the frame",
                r"^│123\| ",
            ),
        ] {
            let mut c = complete("demo.ts", "", true);
            c.result.as_mut().unwrap().is_error = Some(true);
            c.result.as_mut().unwrap().content = vec![text.into()];
            let output = rows(&mut c, 100, 30).iter().map(|r| sanitize(&r.as_str().into())).collect::<Vec<_>>();
            let body = &output[1..output.len() - 1];
            assert!(body.len() >= 2, "{prefix}");
            assert!(regex::Regex::new(pattern).unwrap().is_match(&body[0]));
            let phantom = regex::Regex::new(r"^│\s*\|").unwrap();
            assert!(body[1..].iter().all(|r| !phantom.is_match(r)));
        }
    });
    if original_observations.is_err() {
        failures.push("original 31 observations");
    }
    assert!(failures.is_empty(), "EDIT source family failures: {failures:?}");
}

fn replacement_rows(old: WireString, new: WireString, columns: usize) -> Vec<String> {
    let mut units = "-42|".encode_utf16().collect::<Vec<_>>();
    units.extend_from_slice(old.units());
    units.extend("\n+42|".encode_utf16());
    units.extend_from_slice(new.units());
    let mut c = complete("demo.ts", "", true);
    c.result.as_mut().unwrap().details.as_mut().unwrap().diff = WireText(WireString::from_units(units));
    rows(&mut c, columns, 30)
}
fn inverse_active_at_end(row: &str) -> bool {
    let re = regex::Regex::new(r"\x1b\[([0-9;]*)m").unwrap();
    let mut inverse = false;
    for cap in re.captures_iter(row) {
        let p = cap[1].split(';').collect::<Vec<_>>();
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                "38" | "48" => {
                    i += match p.get(i + 1) {
                        Some(&"2") => 4,
                        Some(&"5") => 2,
                        _ => 0,
                    }
                }
                "" | "0" => inverse = false,
                "7" => inverse = true,
                "27" => inverse = false,
                _ => {}
            }
            i += 1;
        }
    }
    inverse
}

#[test]
fn apply_patch_renderer_original_8_observations() {
    // 01 renderer identity aliases edit and apply_patch
    assert!(std::ptr::eq(renderer(&"edit".into()).unwrap(), renderer(&"apply_patch".into()).unwrap()));
    assert!(is_edit_renderer(&"apply_patch".into()));
    // 02 apply_patch result goes through EDIT UI
    let mut c = tool(
        "apply_patch",
        serde_json::json!({"input":"*** Begin Patch\n*** Update File: src/demo.ts\n@@\n-old\n+new\n*** End Patch"}),
        EditMode::Hashline,
    );
    c.update_result(
        EditResult {
            content: vec!["".into()],
            details: Some(EditDetails {
                path: Some("src/demo.ts".into()),
                op: Some(Operation::Update),
                diff: WireText("@@\n-old\n+new".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        false,
    );
    let text = tool_render(&mut c, 140);
    contains(&text, "src/demo.ts");
    contains(&text, "+new");
    excludes(&text, "(no output)");
    // 03 derives call path, operation and count from envelope
    let mut c = call(
        serde_json::json!({"input":"*** Begin Patch\n*** Update File: src/first.ts\n@@\n-before\n+after\n*** Add File: src/new.ts\n+hello\n*** End Patch"}),
        EditMode::ApplyPatch,
        false,
    );
    let text = render(&mut c, 160);
    for part in ["src/first.ts", "Edit", "(+1 more)"] {
        contains(&text, part);
    }
    // 04 missing end-marker suppressed while input streams
    let mut c = call(
        serde_json::json!({"input":"*** Begin Patch\n*** Update File: src/streaming.ts\n@@\n-before\n+after"}),
        EditMode::ApplyPatch,
        false,
    );
    let text = render(&mut c, 160);
    contains(&text, "src/streaming.ts");
    excludes(&text, "The last line of the patch must be");
    // 05 malformed hunk has parse error beside resolved path
    let mut c = tool(
        "apply_patch",
        serde_json::json!({"input":"*** Begin Patch\n*** Update File: src/bad.ts\n*** End Patch"}),
        EditMode::ApplyPatch,
    );
    c.update_stream_preview(
        vec![PerFilePreview {
            path: WireText("".into()),
            error: Some("Line 3: Update file hunk for path 'src/bad.ts' is empty".into()),
            ..Default::default()
        }],
        false,
    );
    let text = tool_render(&mut c, 160);
    contains(&text, "src/bad.ts");
    contains(&text, "is empty");
    // 06 previews remain after args complete
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("preview.ts"), "const value = 1;\n").unwrap();
    let input = "*** Begin Patch\n*** Update File: preview.ts\n@@\n-const value = 1;\n+const value = 2;\n*** End Patch";
    let mut c = tool("apply_patch", serde_json::json!({"input":input}), EditMode::ApplyPatch);
    excludes(&tool_render(&mut c, 160), "(preview)");
    c.update_stream_preview(
        vec![PerFilePreview {
            path: WireText("preview.ts".into()),
            diff: Some(" 1|const value = 1;\n-1|const value = 1;\n+1|const value = 2;".into()),
            first_changed_line: Some(1),
            ..Default::default()
        }],
        false,
    );
    c.set_args_complete();
    let text = tool_render(&mut c, 160);
    contains(&text, "(preview)");
    contains(&text, "const value = 2;");
    // 07 immediate argument refresh is synchronous, without timer scheduling
    let mut c = tool("apply_patch", serde_json::json!({"input":""}), EditMode::ApplyPatch);
    let before = c.revision;
    c.update_args(args(serde_json::json!({"input":input})));
    assert_eq!(c.revision, before + 1);
    contains(&tool_render(&mut c, 160), "preview.ts");
    // 08 separators align with original mixed line-number widths
    let mut c =
        tool("edit", serde_json::json!({"path":"packages/coding-agent/src/tools/image-gen.ts"}), EditMode::Hashline);
    c.update_result(EditResult{content:vec!["".into()],details:Some(EditDetails{path:Some("packages/coding-agent/src/tools/image-gen.ts".into()),op:Some(Operation::Update),diff:WireText(" 10|}\n+11|import { CODEX_INSTRUCTIONS } from \"@oh-my-pi/pi-ai/providers/openai-codex-responses\";\n 12|\t$env,\n 228|\toutput_format: typeof OPENAI_IMAGE_OUTPUT_FORMAT;\n+235|\tinstructions?: string;\n 234|\tinput: Array<{ role: \"user\"; content: OpenAIInputContent[] }>;".into()),..Default::default()}),..Default::default()},false);
    let text = tool_render(&mut c, 220);
    for part in ["  10│}", " +11│import", " 228│", "+235│", " 234│"] {
        contains(&text, part);
    }
}

// Invoked by the existing 31-observation family, never a separate test entrance.
fn observe_link_and_ascii_digit_contracts() {
    if cfg!(windows) {
        for (path, expected) in [
            (r"\\server\share\a.ts", "file://server/share/a.ts?line=42"),
            (r"\\SERVER\share\a b#%?é.ts", "file://server/share/a%20b%23%25%3F%C3%A9.ts?line=42"),
        ] {
            let link = super::hyperlink::file_hyperlink(path, "anchor", Some(42), true);
            contains(&link, &format!(";{expected}\x1b\\anchor\x1b]8;;\x1b\\"));
            excludes(&link, "file:////");
            assert_eq!(super::hyperlink::file_hyperlink(path, "anchor", None, false), "anchor");
        }
    }
    let theme = Theme::dark();
    for digit in ["١", "१", "１"] {
        for source in
            [format!("+{digit}|unicode number stays content"), format!("+{digit} unicode legacy stays content")]
        {
            let rendered = diff::render_diff(&source.as_str().into(), None, &theme);
            let plain = sanitize(&rendered.as_str().into());
            contains(&plain, &source);
            excludes(&plain, "│");
        }
        for separator in ['|', '│'] {
            let source = format!("+{digit}{separator}{}", "ordinary_body_".repeat(8));
            let wrapped = super::edit_renderer::wrap_edit_line(&source, 24);
            assert!(wrapped.len() > 1);
            assert!(
                wrapped.iter().skip(1).all(|line| !line.contains(separator)),
                "Unicode digit is not an ASCII gutter: {wrapped:?}"
            );
        }
    }
    let ascii = diff::render_diff(&"+12|ASCII canonical row".into(), None, &theme);
    contains(&sanitize(&ascii.as_str().into()), "12│");
    for separator in ['|', '│'] {
        let wrapped = super::edit_renderer::wrap_edit_line(&format!("+12{separator}{}", "ascii_body_".repeat(8)), 24);
        assert!(wrapped.len() > 1);
        assert!(wrapped.iter().skip(1).all(|line| line.contains(separator)), "ASCII gutter is retained: {wrapped:?}");
    }
}

fn observe_fixed_terminal_unicode_contracts() {
    use super::unicode::{graphemes_str, width_char, width_str};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    // Fixed xutf tests/width.rs and scalar state contracts, within the original
    // renderer group. These observations do not replace full TUI acceptance.
    for (text, expected) in [
        ("", 0),
        ("a\tb\r\n", 2),
        ("界", 2),
        ("e\u{301}", 1),
        // Fixed width.rs commits non-promotable ASCII before the ZWJ base.
        // This legal string has one grapheme but flat-scanner width two.
        ("a\u{200d}\u{0903}", 2),
        ("ab\u{200d}\u{0903}", 3),
        // A promotable digit is held back by that same original ASCII path.
        ("0\u{200d}\u{0903}", 1),
        ("\u{3164}", 0),
        ("⚠\u{fe0e}", 1),
        ("⚠\u{fe0f}", 2),
        ("😀\u{fe0e}", 2),
        ("0\u{20e3}", 2),
        ("👨‍👩‍👧‍👦", 2),
        ("👍🏽", 2),
        ("0🏽", 3),
        ("🇺🇸🇯🇵", 4),
        ("\u{1100}\u{1161}\u{11a8}", 2),
        ("\u{200b}\u{200d}\u{301}", 0),
    ] {
        assert_eq!(width_str(text), expected, "fixed terminal cells for {text:?}");
    }
    for (character, expected) in [('a', 1), ('界', 2), ('\u{301}', 0), ('\0', 0), ('\t', 0), ('\u{200d}', 0)] {
        assert_eq!(width_char(character), expected);
    }
    let boundary_case = "a\u{200d}\u{0903}";
    assert_eq!(graphemes_str(boundary_case).collect::<Vec<_>>(), [boundary_case]);
    assert_eq!(super::text::visible_width(&boundary_case.encode_utf16().collect::<Vec<_>>(), 4), 2);
    let sample = "界e\u{301}⚠️👍🏽🇺🇸\u{1100}\u{1161}\u{11a8}\r\nx";
    let clusters = graphemes_str(sample).collect::<Vec<_>>();
    assert_eq!(clusters, ["界", "e\u{301}", "⚠️", "👍🏽", "🇺🇸", "\u{1100}\u{1161}\u{11a8}", "\r\n", "x"]);
    let mut offset = 0;
    for cluster in clusters {
        assert!(sample.is_char_boundary(offset));
        assert!(std::ptr::eq(cluster.as_ptr(), sample[offset..].as_ptr()));
        offset += cluster.len();
    }
    assert_eq!(offset, sample.len());
    assert_eq!(graphemes_str("\u{915}\u{94d}\u{937}").collect::<Vec<_>>(), ["\u{915}\u{94d}\u{937}"]);
    assert_eq!(graphemes_str("\u{600}1").collect::<Vec<_>>(), ["\u{600}1"]);
    assert_eq!(graphemes_str("🇺🇸🇨").collect::<Vec<_>>(), ["🇺🇸", "🇨"]);

    // Input consumes the same borrowed boundaries for whole-cluster movement
    // and deletion, including non-BMP ZWJ sequences.
    let mut input = super::input::InputBuffer::default();
    input.insert("ae\u{301}👩‍💻");
    input.key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), true);
    assert_eq!(input.cursor, "ae\u{301}".len());
    input.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE), true);
    assert_eq!(input.draft, "a👩‍💻");
    assert_eq!(input.cursor, 1);
    input.key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE), true);
    assert_eq!(input.draft, "a");
    input.key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE), true);
    input.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE), true);
    assert_eq!(input.cursor, 1);
}

#[test]
fn structured_preview_original_2_observations() {
    let render_files = |files: Vec<PerFilePreview>| {
        let mut c = EditComponent::call(
            EditRenderArgs::default(),
            options(true, false),
            EditRenderContext {
                edit_mode: Some(EditMode::ApplyPatch),
                per_file_diff_preview: Some(files),
                ..Default::default()
            },
            Arc::new(Theme::dark()),
        );
        render(&mut c, 160)
    };
    // 01 each structured file gets its own header and diff
    let text = render_files(vec![
        PerFilePreview {
            path: WireText("src/a.ts".into()),
            diff: Some("@@ -1,1 +1,1 @@\n-1|old a\n+1|new a".into()),
            first_changed_line: Some(1),
            ..Default::default()
        },
        PerFilePreview {
            path: WireText("src/b.ts".into()),
            diff: Some("@@ -2,1 +2,1 @@\n-2|old b\n+2|new b".into()),
            first_changed_line: Some(2),
            ..Default::default()
        },
    ]);
    for part in ["src/a.ts", "new a", "src/b.ts", "new b"] {
        contains(&text, part);
    }
    // 02 sibling error does not erase successful diff
    let text = render_files(vec![
        PerFilePreview {
            path: WireText("src/good.ts".into()),
            diff: Some("@@ -1,1 +1,1 @@\n-1|old\n+1|new".into()),
            first_changed_line: Some(1),
            ..Default::default()
        },
        PerFilePreview {
            path: WireText("src/missing.ts".into()),
            error: Some("File not found: src/missing.ts".into()),
            ..Default::default()
        },
    ]);
    for part in ["src/good.ts", "new", "src/missing.ts", "File not found: src/missing.ts"] {
        contains(&text, part);
    }
}
