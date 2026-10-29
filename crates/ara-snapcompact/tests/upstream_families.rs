//! Aggregated fixed-OMP module families, not an additional micro-test matrix.
//! Source: packages/snapcompact/test/snapcompact.test.ts at 596f2da (MIT).
//! Original native rasterizer tests remain in src/native.rs.
use ara_snapcompact::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::value::RawValue;
use serde_json::{Value, json};
use std::io::Cursor;

fn raw_json(source: &str) -> Box<RawValue> {
    RawValue::from_string(source.into()).unwrap()
}
fn exact_prep(source: &CompactionPreparation) -> CompactionPreparationUtf16 {
    CompactionPreparationUtf16 {
        first_kept_entry_id: source.first_kept_entry_id.clone(),
        messages_to_summarize: source
            .messages_to_summarize
            .iter()
            .map(|m| serde_json::value::to_raw_value(m).unwrap())
            .collect(),
        turn_prefix_messages: source
            .turn_prefix_messages
            .iter()
            .map(|m| serde_json::value::to_raw_value(m).unwrap())
            .collect(),
        tokens_before: source.tokens_before,
        previous_summary: source.previous_summary.clone().map(Utf16Text::from),
        previous_preserve_data: source
            .previous_preserve_data
            .as_ref()
            .map(|p| serde_json::value::to_raw_value(p).unwrap()),
        file_ops: source.file_ops.clone(),
    }
}
// The original scalar fixtures exercise the exact Host entry point. Their
// checked result projection is only a fixture convenience; separate boundary
// observations below retain lone units throughout archive/recompaction.
fn compact(source: &CompactionPreparation, options: &CompactionOptions) -> Result<CompactionResult> {
    let result = compact_utf16(&exact_prep(source), options)?;
    serde_json::from_str(&serde_json::to_string(&result).unwrap()).map_err(|e| Error::from_reason(e.to_string()))
}

fn user(text: impl Into<String>) -> Value {
    json!({"role":"user", "content":text.into()})
}
fn assistant(content: Value) -> Value {
    json!({"role":"assistant", "content":content})
}
fn tool(text: impl Into<String>) -> Value {
    json!({"role":"toolResult", "toolCallId":"call-1", "content":[{"type":"text","text":text.into()}], "isError":false})
}
fn prep(messages: Vec<Value>) -> CompactionPreparation {
    CompactionPreparation {
        first_kept_entry_id: "kept-1".into(),
        messages_to_summarize: messages,
        turn_prefix_messages: Vec::new(),
        tokens_before: 99000.0,
        previous_summary: None,
        previous_preserve_data: None,
        file_ops: create_file_ops(),
    }
}
fn target(api: &str, id: &str) -> ShapeTarget {
    ShapeTarget { api: Some(api.into()), id: (!id.is_empty()).then(|| id.into()), ..Default::default() }
}
fn named(name: &str) -> Shape {
    resolve_shape(None, Some(name)).unwrap()
}
fn size_options(shape: &Shape, size: u32) -> RenderManyOptions {
    RenderManyOptions { shape: Some(shape.clone()), frame_size: Some(size), ..Default::default() }
}
fn archive(result: &CompactionResult) -> Archive {
    get_preserved_archive(result.preserve_data.as_ref()).unwrap()
}
fn block_data(block: &HistoryBlock) -> &str {
    match block {
        HistoryBlock::Image { data, .. } => data,
        _ => panic!("image expected"),
    }
}
fn block_text(block: &HistoryBlock) -> &str {
    match block {
        HistoryBlock::Text { text } => text,
        _ => panic!("text expected"),
    }
}
fn rgb(data: &str) -> (usize, usize, Vec<u8>, u8) {
    let bytes = STANDARD.decode(data).unwrap();
    let color_type = bytes[25];
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    (info.width as usize, info.height as usize, pixels, color_type)
}
fn ink(pixels: &[u8], color: [u8; 3]) -> bool {
    pixels.chunks_exact(3).any(|p| p == color)
}

#[test]
fn upstream_normalize_and_scan_renderability_family() {
    observe_utf16_normalization_boundary();
    let cases = [
        ("a \t b   c", "a b c"),
        ("x → y ✓ “quoted” — em…", "x -> y v \"quoted\" - em..."),
        ("café größe", "café größe"),
        ("box │─┌ emoji 🎞", "box |-+ emoji"),
        ("a\n\n\tb   c\r\nd", "a█b c█d"),
        ("\n\nbody\n", "body"),
        ("  \n\t  ", ""),
        ("\u{1b}[31mred\u{1b}[0m plain", "red plain"),
        ("a\0b\u{7}c\u{200b}d\u{feff}e\u{301}f", "abcdef"),
        ("x \u{e}y\u{f} z", "x \u{e}y\u{f} z"),
        ("こんにちは", "こんにちは"),
        ("カタカナ", "カタカナ"),
        ("안녕하세요", "안녕하세요"),
        ("✅ pass ⚠️ warn ❌ fail 😄", "[OK] pass [WARN] warn [FAIL] fail"),
        ("✗ ✘", "x x"),
        ("x⁵", "x5"),
        ("ＨＥＬＬＯ", "HELLO"),
        ("ﬁle", "file"),
        ("step ① then ②", "step 1 then 2"),
        ("section Ⅻ", "section XII"),
        ("⅓ cup", "1/3 cup"),
        ("𝐇𝐞𝐥𝐥𝐨", "Hello"),
        ("™ ‹q› ′ ″ ⇐ ↑", "TM <q> ' \" <= ^"),
        ("emoji 🎞 \u{e000}", "emoji ?"),
    ];
    for (input, output) in cases {
        assert_eq!(normalize(input, NormalizeOptions::default()), output, "{input:?}");
    }
    for text in [
        "function hello() { return 'world'; }",
        "café résumé naïve",
        "const a = '你好世界';",
        "\u{1b}[31mhello \u{e} \u{f} \n\t world\u{1b}[0m",
    ] {
        assert_eq!(
            scan_renderability(text, NormalizeOptions::default()),
            Renderability { is_safe: true, unrenderable_ratio: 0.0 }
        );
    }
    assert_eq!(
        scan_renderability(&"\u{e000}".repeat(10), NormalizeOptions::default()),
        Renderability { is_safe: false, unrenderable_ratio: 1.0 }
    );
}

#[test]
fn upstream_shape_resolution_and_guard_family() {
    // B-7a12a03e2d: forced geometry retains provider billing and detail.
    let anthropic = target("anthropic-messages", "");
    let openai = target("openai-responses", "");
    assert_eq!(resolve_shape(Some(&anthropic), Some("auto")).unwrap(), resolve_shape(Some(&anthropic), None).unwrap());
    let dense = resolve_shape(Some(&anthropic), Some("6x6u-sent")).unwrap();
    assert_eq!(
        (dense.cell_width, dense.variant.as_str(), dense.frame_token_estimate, dense.image_detail.as_deref()),
        (6.0, "sent", resolve_shape(Some(&anthropic), None).unwrap().frame_token_estimate, None)
    );
    let repeated = resolve_shape(Some(&openai), Some("8x8r-bw")).unwrap();
    assert_eq!(
        (repeated.line_repeat, repeated.frame_token_estimate, repeated.image_detail.as_deref()),
        (2.0, resolve_shape(Some(&openai), None).unwrap().frame_token_estimate, Some("original"))
    );
    let google_legacy = resolve_shape(Some(&target("google-generative-ai", "")), Some("5x8-bw")).unwrap();
    assert_eq!((google_legacy.frame_size, google_legacy.frame_token_estimate), (2576.0, 1120.0));
    assert_eq!(resolve_shape(Some(&anthropic), Some("5x8-bw")).unwrap().frame_token_estimate, 4761.0);
    for (api, height, width, bill, detail) in [
        ("anthropic-messages", 16.0, 11.0, 3136.0, None),
        ("openai-responses", 22.0, 8.0, 2882.0, Some("original")),
        ("azure-openai-responses", 22.0, 8.0, 2882.0, Some("original")),
        ("google-generative-ai", 22.0, 8.0, 1120.0, None),
    ] {
        let s = resolve_shape(Some(&target(api, "")), None).unwrap();
        assert_eq!(
            (s.cell_height, s.cell_width, s.frame_token_estimate, s.image_detail.as_deref()),
            (height, width, bill, detail)
        );
    }
    assert_eq!(resolve_shape(None, None).unwrap(), resolve_shape(Some(&target("future-api", "")), None).unwrap());
    let fallback = resolve_shape(Some(&target("future-api", "")), None).unwrap();
    assert_eq!(fallback.cell_height, 22.0);
    assert_eq!(fallback.variant, "bw");
    for id in [
        "anthropic/claude-fable-5",
        "claude-fable-5@20250929",
        "claude-opus-4-8",
        "anthropic--claude-4.8-opus",
        "claude-opus-4-10",
        "anthropic/claude-fable-latest",
        "~anthropic/claude-fable-latest",
        "claude-opus-5",
        "claude-opus-6",
        "CLAUDE-OPUS-5",
        "claude-opus-5-11",
    ] {
        assert_eq!(resolve_shape(Some(&target("anthropic-messages", id)), None).unwrap().frame_size, 1932.0, "{id}");
    }
    for id in ["claude-opus-4-6", "claude-3-5-sonnet"] {
        assert_eq!(resolve_shape(Some(&target("anthropic-messages", id)), None).unwrap().frame_size, 1568.0);
    }
    let routed = resolve_shape(Some(&target("openai-completions", "anthropic/claude-fable-5")), None).unwrap();
    assert_eq!(
        (
            routed.font.as_str(),
            routed.cell_width,
            routed.frame_size,
            routed.frame_token_estimate,
            routed.image_detail.as_deref()
        ),
        ("8x13", 11.0, 1932.0, 4761.0, Some("original"))
    );
    let vertex = resolve_shape(Some(&target("google-vertex", "claude-fable-5@20250929")), None).unwrap();
    assert_eq!(vertex.font, "8x13");
    assert_eq!((vertex.cell_width, vertex.frame_size, vertex.frame_token_estimate), (11.0, 1932.0, 4761.0));
    let gemini = resolve_shape(Some(&target("google-generative-ai", "gemini-3.5-flash")), None).unwrap();
    assert_eq!(gemini.columns, None);
    assert_eq!((gemini.cell_height, gemini.frame_size, gemini.frame_token_estimate), (22.0, 2048.0, 1120.0));
    assert_eq!(
        resolve_shape(Some(&target("openai-completions", "moonshotai/kimi-k2.6")), None).unwrap(),
        resolve_shape(Some(&target("openai-completions", "")), Some("8on22-bw")).unwrap()
    );
    assert_eq!(
        resolve_shape(Some(&target("openai-completions", "z-ai/glm-4.6v")), None).unwrap(),
        resolve_shape(Some(&target("openai-completions", "")), Some("8on16-bw")).unwrap()
    );
    assert!(ideal_shape_variant("qwen/qwen3-vl").is_none());
    for name in SHAPE_VARIANT_NAMES {
        let shape = named(name);
        assert!(is_shape_variant_name(name));
        assert!(is_shape(&serde_json::to_value(shape).unwrap()));
        assert!(is_shape(&serde_json::to_value(resolve_shape(Some(&openai), Some(name)).unwrap()).unwrap()));
    }
    // B-73398e9531, B-eb97d9eaa5: catalog and complete guard negatives.
    assert!(!is_shape_variant_name("auto") && !is_shape_variant_name("6x10-sent"));
    let mut guard = serde_json::to_value(resolve_shape(Some(&openai), None).unwrap()).unwrap();
    assert!(is_shape(&guard));
    let original_width = guard["cellWidth"].clone();
    guard["cellWidth"] = json!(0);
    assert!(!is_shape(&guard));
    guard["cellWidth"] = original_width;
    guard["variant"] = json!("color");
    assert!(!is_shape(&guard));
    guard["variant"] = json!("bw");
    guard["imageDetail"] = json!("original");
    assert!(is_shape(&guard));
    let mut malformed = serde_json::to_value(named("doc-8on16-sent-dim")).unwrap();
    // B-1e6f80cc2a, B-60a2de2f0e: original six winners and allowed one column.
    for name in ["6x12-dim", "8x13-bw", "8on16-bw", "doc-8on16-bw", "doc-8on16-sent", "doc-8on16-sent-dim"] {
        assert!(is_shape_variant_name(name) && SHAPE_VARIANT_NAMES.contains(&name));
        let shape = named(name);
        assert!(is_shape(&serde_json::to_value(&shape).unwrap()));
        assert_eq!(shape.frame_size, 1568.0);
    }
    let mut one_column = malformed.clone();
    one_column["columns"] = json!(1);
    assert!(is_shape(&one_column));
    for (key, bad) in
        [("columns", json!(3)), ("stretch", json!("no")), ("stopwordDim", json!(1)), ("font", json!("9x9"))]
    {
        let old = malformed[key].clone();
        malformed[key] = bad;
        assert!(!is_shape(&malformed));
        malformed[key] = old;
    }
    let cjk = "你好世界测试文本数据记录";
    // B-a13b1873d8 and B-e58ac15022 retain original multilingual/API inputs.
    let multilingual = "こんにちは 你好 안녕 世界 한국어";
    let silver = resolve_shape(Some(&anthropic), Some("silver16-bw")).unwrap();
    assert!(scan_renderability(multilingual, NormalizeOptions::default()).is_safe);
    assert!(
        scan_renderability(multilingual, NormalizeOptions { shape: Some(silver.clone()), ..Default::default() })
            .is_safe
    );
    assert_eq!(normalize(multilingual, NormalizeOptions::default()), multilingual);
    assert_eq!(
        normalize(multilingual, NormalizeOptions { shape: Some(silver.clone()), ..Default::default() }),
        multilingual
    );
    assert_eq!(resolve_shape_for_text(multilingual, Some(&anthropic), Some("auto")).unwrap(), silver);
    let gpt = target("openai-responses", "gpt-5.5");
    let ascii = resolve_shape_for_text(
        "function render(value: string) { return value.trim().toLowerCase(); }",
        Some(&gpt),
        Some("auto"),
    )
    .unwrap();
    assert_eq!(ascii, resolve_shape(Some(&gpt), Some("auto")).unwrap());
    assert_ne!(ascii.font, "silver");
    assert_eq!(resolve_shape_for_text(cjk, Some(&target("anthropic-messages", "")), None).unwrap().font, "silver");
    assert_eq!(
        resolve_shape_for_text(
            "ASCII-heavy transcript with a 你好 footnote",
            Some(&target("anthropic-messages", "")),
            None
        )
        .unwrap()
        .font,
        "8x13"
    );
    assert_eq!(resolve_shape_for_text(cjk, None, Some("8on16-bw")).unwrap().font, "8x13");
    assert!(
        !scan_renderability(
            "\u{31350}".repeat(12).as_str(),
            NormalizeOptions { shape: Some(named("silver16-bw")), ..Default::default() }
        )
        .is_safe
    );
    assert_eq!(
        scan_renderability(
            &"\u{31350}".repeat(12),
            NormalizeOptions { shape: Some(named("silver16-bw")), ..Default::default() }
        )
        .unrenderable_ratio,
        1.0
    );
}

#[test]
fn upstream_render_and_render_many_family() {
    let legacy = named("5x8-sent");
    assert_eq!(geometry(&legacy, Some(320)), Geometry { cols: 64, rows: 40, capacity: 2560 });
    let frame = render("First sentence here. Second one differs.", &legacy, Some(320)).unwrap();
    assert_eq!((frame.cols, frame.rows, frame.chars), (64, 40, 40));
    let (w, h, pixels, color) = rgb(&frame.data);
    assert_eq!((w, h, color), (320, 64, 3)); // b91db66e: 64px minimum.
    assert!(ink(&pixels, [109, 2, 2]) && ink(&pixels, [109, 53, 2]));
    assert!(!ink(&pixels, [24, 109, 2]));
    let repeated = named("8x8r-bw");
    assert_eq!(geometry(&repeated, Some(320)), Geometry { cols: 40, rows: 20, capacity: 800 });
    let (_, _, p, color) = rgb(&render("Hello world. Again.", &repeated, Some(320)).unwrap().data);
    assert_eq!(color, 3);
    assert!(ink(&p, [0, 0, 0]) && ink(&p, [255, 247, 194]));
    assert!(!ink(&p, [109, 2, 2]));
    let tracked = resolve_shape(Some(&target("anthropic-messages", "")), None).unwrap();
    assert_eq!(geometry(&tracked, Some(320)), Geometry { cols: 29, rows: 20, capacity: 580 });
    let blocks = render_many("Reading the films of the archive. Again.", size_options(&tracked, 320)).unwrap();
    let (_, _, p, _) = rgb(block_data(&blocks[0]));
    assert!(ink(&p, [0, 0, 0]));
    assert!(!ink(&p, [128, 128, 128]) && !ink(&p, [255, 247, 194]) && !ink(&p, [109, 2, 2]));
    let dim = named("6x12-dim");
    let blocks = render_many("Reading the films of the archive. Again.", size_options(&dim, 320)).unwrap();
    let (_, _, p, _) = rgb(block_data(&blocks[0]));
    assert!(ink(&p, [0, 0, 0]) && ink(&p, [128, 128, 128]));
    let stretched = render("Hello world.", &named("6x6u-sent"), Some(320)).unwrap();
    assert_eq!(stretched.cols, 53); // B-aea85eb5f9
    assert_eq!(rgb(&stretched.data).3, 2);
    let silver = named("silver16-bw");
    let frame = render("你好안녕", &silver, Some(64)).unwrap();
    assert_eq!(frame.cols, 4); // B-a7b3634cd3: Silver uses 16px cells.
    assert_eq!(frame.chars, 4);
    let (w, h, _, color) = rgb(&frame.data);
    assert_eq!((w, h, color), (64, 64, 2)); // Same floor for Silver.
    let bitmap = named("8on16-bw");
    let frame = render("你", &bitmap, Some(64)).unwrap();
    assert_eq!(frame.chars, 1);
    let (w, h, p, color) = rgb(&frame.data);
    assert_eq!(color, 3); // B-70c26ef9f1
    assert!((0..h).any(|y| (8..16).any(|x| p[(y * w + x) * 3..(y * w + x) * 3 + 3] == [0, 0, 0])));
    let frame = render(&"x".repeat(3060), &legacy, Some(320)).unwrap();
    assert_eq!(frame.chars, 2560);
    let newline = render("a█b", &legacy, Some(320)).unwrap();
    assert_eq!(newline.chars, 3); // B-3e9442822e
    let (_, _, p, _) = rgb(&newline.data);
    for y in 0..8 {
        for x in 5..10 {
            assert_eq!(&p[(y * 320 + x) * 3..(y * 320 + x) * 3 + 3], &[0, 0, 0]);
        }
    }
    for text in ["", "  \n\t  "] {
        assert!(render_many(text, size_options(&tracked, 320)).unwrap().is_empty());
        assert_eq!(frames(text, size_options(&tracked, 320)), 0);
    }
    let cap = geometry(&tracked, Some(320)).capacity;
    // B-dcf6632e5b: short text is exactly one PNG image.
    let short = render_many("hello", size_options(&tracked, 320)).unwrap();
    assert_eq!(short.len(), 1);
    assert!(matches!(&short[0], HistoryBlock::Image { mime_type, .. } if mime_type == "image/png"));
    let text = "x".repeat(cap * 2 + 10);
    assert_eq!(render_many(&text, size_options(&tracked, 320)).unwrap().len(), 3);
    assert_eq!(frames(&text, size_options(&tracked, 320)), 3);
    let cap = geometry(&bitmap, Some(64)).capacity;
    assert_eq!(frames(&"你".repeat(cap / 2), size_options(&bitmap, 64)), 1);
    assert_eq!(frames(&"你".repeat(cap / 2 + 1), size_options(&bitmap, 64)), 2);
    assert_eq!(frames(&"a".repeat(cap), size_options(&bitmap, 64)), 1);
    let cap = geometry(&silver, Some(64)).capacity;
    assert_eq!(frames(&"你".repeat(cap), size_options(&silver, 64)), 1);
    let openai = resolve_shape(Some(&target("openai-responses", "")), None).unwrap();
    let cap = geometry(&openai, Some(320)).capacity;
    let mut options = size_options(&openai, 320);
    options.max_frames = Some(2.0);
    let blocks = render_many(&"x".repeat(cap * 3), options).unwrap();
    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], HistoryBlock::Image { detail: Some(v), .. } if v == "original"));
    let no_detail = render_many("hello", size_options(&tracked, 320)).unwrap();
    assert!(matches!(&no_detail[0], HistoryBlock::Image { detail: None, .. })); // B-a3b7a013c4
}

#[test]
fn upstream_serializer_family() {
    observe_utf16_serializer_boundaries();
    let default = SerializeOptions::default();
    let plain = SerializeOptions { dim_tool_results: false, ..default };
    let out = serialize_conversation(&[tool(format!("HEAD-{}-TAIL", "x".repeat(5000)))], &default);
    assert!(out.contains("HEAD-") && out.contains("[…3010ch elided…]") && out.ends_with("-TAIL\u{f}\n</out>"));
    let tight = SerializeOptions { tool_result_max_chars: 10.0, truncate_head_ratio: 0.5, ..default };
    assert!(serialize_conversation(&[tool("a".repeat(100))], &tight).contains("[…90ch elided…]"));
    assert!(
        serialize_conversation(
            &[tool("a".repeat(100))],
            &SerializeOptions { tool_result_max_chars: f64::INFINITY, ..default }
        )
        .contains(&"a".repeat(100))
    );
    let out = serialize_conversation(
        &[assistant(
            json!([{"type":"toolCall","id":"c1","name":"write","arguments":{"path":"a.ts","content":"y".repeat(3000)}}]),
        )],
        &default,
    );
    assert!(out.contains("write(path=\"a.ts\", content=") && out.contains("[…2502ch elided…]"));
    let mut args = serde_json::Map::new();
    for i in 0..10 {
        args.insert(format!("arg{i}"), json!("z".repeat(400)));
    }
    let out = serialize_conversation(
        &[assistant(json!([{"type":"toolCall","id":"c1","name":"tool","arguments":args}]))],
        &default,
    );
    assert!(out.contains("arg0=") && out.contains("ch elided") && out.len() < 2200);
    assert_eq!(
        serialize_conversation(&[user("do the thing"), assistant(json!([{"type":"text","text":"done"}]))], &default),
        "¶user:do the thing\n\n¶ai:done"
    );
    let call = assistant(
        json!([{"type":"toolCall","id":"call-1","name":"bash","arguments":{"i":"Running tests","command":"bun test"}}]),
    );
    assert_eq!(
        serialize_conversation(&[call, tool("3 pass")], &plain),
        "¶call:bash(command=\"bun test\")//Running tests\n<out>\n3 pass\n</out>"
    );
    let call = assistant(
        json!([{"type":"toolCall","id":"c1","name":"bash","arguments":{"i":"raw arg","command":"ls"},"intent":"Derived\nintent  line"}]),
    );
    let out = serialize_conversation(&[call], &default);
    assert!(out.contains("//Derived intent line") && !out.contains("raw arg") && !out.contains("i="));
    let thought =
        assistant(json!([{"type":"thinking","thinking":"weigh options"},{"type":"text","text":"the answer"}]));
    assert_eq!(
        serialize_conversation(std::slice::from_ref(&thought), &default),
        "¶think:weigh options\n\n¶ai:the answer"
    );
    assert_eq!(
        serialize_conversation(&[thought], &SerializeOptions { include_thinking: false, ..default }),
        "¶ai:the answer"
    );
    let thought = assistant(
        json!([{"type":"thinking","thinking":"plan first"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"a.ts"}}]),
    );
    assert_eq!(serialize_conversation(&[thought], &default), "¶think:plan first\n\n¶call:read(path=\"a.ts\")");
    assert_eq!(serialize_conversation(&[tool("ok")], &plain), "¶call:\n<out>\nok\n</out>");
    let mixed = assistant(
        json!([{"type":"text","text":"before"},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"a.ts"}},{"type":"text","text":"after"}]),
    );
    assert_eq!(
        serialize_conversation(&[mixed.clone(), tool("file body")], &plain),
        "¶ai:before\n\n¶call:read(path=\"a.ts\")\n<out>\nfile body\n</out>\n\n¶ai:after"
    );
    let mut useless = tool("No matches found");
    useless["useless"] = json!(true);
    assert_eq!(serialize_conversation(&[mixed, useless], &default), "¶ai:before\nafter");
    let blank = assistant(
        json!([{"type":"thinking","thinking":"   "},{"type":"text","text":""},{"type":"toolCall","id":"call-1","name":"read","arguments":{"path":"a.ts"}}]),
    );
    let blank_output = serialize_conversation(&[blank, tool("body")], &plain);
    assert!(!blank_output.contains("¶ai:")); // B-a746d3b29d
    assert_eq!(blank_output, "¶call:read(path=\"a.ts\")\n<out>\nbody\n</out>");
    // B-fd45bfbea8: keep/drop pairs coexist in the same assistant message.
    let pairs = assistant(json!([
        {"type":"toolCall","id":"c-keep","name":"search","arguments":{"pattern":"alpha"}},
        {"type":"toolCall","id":"c-drop","name":"search","arguments":{"pattern":"zzz_nothing"}}
    ]));
    let mut kept = tool("alpha match found");
    kept["toolCallId"] = json!("c-keep");
    let mut dropped = tool("No matches found");
    dropped["toolCallId"] = json!("c-drop");
    dropped["useless"] = json!(true);
    let paired = serialize_conversation(&[pairs, kept, dropped], &default);
    assert!(paired.contains("pattern=\"alpha\"") && paired.contains("alpha match found"));
    assert!(!paired.contains("zzz_nothing") && !paired.contains("No matches found"));
    let out = serialize_conversation(&[user("hello \u{e}world"), tool("ok")], &default);
    assert!(out.contains("¶user:hello world") && out.contains("<out>\n\u{e}ok\u{f}\n</out>"));
    assert_eq!(
        serialize_conversation(
            &[
                user("hello"),
                user("world"),
                assistant(json!([{"type":"text","text":"hi"}])),
                assistant(json!([{"type":"text","text":"there"}]))
            ],
            &default
        ),
        "¶user:hello\nworld\n\n¶ai:hi\nthere"
    );
    assert_eq!(
        serialize_conversation(
            &[assistant(
                json!([{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"a.ts"}},{"type":"toolCall","id":"c2","name":"read","arguments":{"path":"b.ts"}}])
            )],
            &default
        ),
        "¶call:read(path=\"a.ts\")\nread(path=\"b.ts\")"
    );
}

#[test]
fn upstream_dim_wrap_doc_and_file_operations_family() {
    assert_eq!(dim_stopwords("the cat sat on a mat"), "\u{e}the\u{f} cat sat \u{e}on\u{f} \u{e}a\u{f} mat");
    assert_eq!(dim_stopwords("The end."), "\u{e}The\u{f} end.");
    assert_eq!(dim_stopwords("theory"), "theory");
    assert_eq!(
        dim_stopwords("alpha \u{e}the tool output\u{f} and omega"),
        "alpha \u{e}the tool output\u{f} \u{e}and\u{f} omega"
    );
    assert_eq!(dim_stopwords("\u{e}so it goes"), "\u{e}so it goes");
    let text = "this is the content of a frame";
    assert_ne!(dim_stopwords(text), text);
    assert_eq!(strip_dim_markers(&dim_stopwords(text)), text);
    for (text, width, expected) in [
        ("aa bb cc dd", 5, vec!["aa bb", "cc dd"]),
        ("one two three", 8, vec!["one two", "three"]),
        ("abcd", 4, vec!["abcd"]),
        ("abcdefghij", 4, vec!["abcd", "efgh", "ij"]),
        ("xx abcdefghij yy", 4, vec!["xx", "abcd", "efgh", "ij", "yy"]),
    ] {
        assert_eq!(wrap(text, width, false), expected);
    }
    assert!(wrap("", 10, false).is_empty());
    let doc = named("doc-8on16-bw");
    assert_eq!(frames("", size_options(&doc, 160)), 0); // B-1a0bceaef2
    assert_eq!(geometry(&doc, None), Geometry { cols: 96, rows: 98, capacity: 18816 });
    assert_eq!(geometry(&doc, Some(160)), Geometry { cols: 8, rows: 10, capacity: 160 });
    for (count, expected) in [(60, 1), (61, 2)] {
        assert_eq!(frames(&vec!["ab"; count].join(" "), size_options(&doc, 160)), expected);
    }
    let mut ops = create_file_ops();
    ops.read.extend(["src/read-only.ts", "artifact://7"].map(str::to_owned));
    ops.edited.extend(["src/edited.ts", "conflict://1"].map(str::to_owned));
    ops.written.insert("local://ctx.md".into());
    assert_eq!(
        compute_file_lists(&ops),
        CompactionDetails { read_files: vec!["src/read-only.ts".into()], modified_files: vec!["src/edited.ts".into()] }
    );
    let updated =
        upsert_file_operations("body\n<read-files>old</read-files>", &["src/a.ts".into()], &["src/b.ts".into()], None);
    assert!(
        updated.contains("body")
            && updated.contains("a.ts (Read)")
            && updated.contains("b.ts (Write)")
            && !updated.contains("old")
    );
}

#[test]
fn upstream_archive_helpers_and_budgets_family() {
    observe_utf16_archive_contract();
    // B-0f20617fa8: detail presence is per-frame, including wire omission.
    let detail_archive: Archive = serde_json::from_value(json!({"frames":[
        {"data":"ZmFrZQ==","mimeType":"image/png","cols":10,"rows":10,"chars":5,"detail":"original"},
        {"data":"ZmFrZTI=","mimeType":"image/png","cols":10,"rows":10,"chars":5}
    ],"totalChars":10,"truncatedChars":0}))
    .unwrap();
    let detail_images = images(&detail_archive);
    assert_eq!(serde_json::to_value(&detail_images[0]).unwrap()["detail"], "original");
    assert!(serde_json::to_value(&detail_images[1]).unwrap().get("detail").is_none());
    for value in [None, Some(json!({"snapcompact":"nope"})), Some(json!({"snapcompact":{"frames":[]}}))] {
        assert!(get_preserved_archive(value.as_ref()).is_none());
    }
    let frame = json!({"data":"ZmFrZQ==","mimeType":"image/png","cols":64,"rows":40,"chars":10,"detail":"original"});
    for value in [
        json!({"frames":[frame.clone()],"totalChars":10,"truncatedChars":0}),
        json!({"frames":[],"totalChars":21,"truncatedChars":0,"text":"older history newer history","textHead":"older history newer history"}),
        json!({"frames":[frame.clone()],"totalChars":10,"truncatedChars":0,"textTail":"newest unframed history"}),
    ] {
        let archive = get_preserved_archive(Some(&json!({"snapcompact":value.clone()}))).unwrap();
        // OMP's toEqual compares JavaScript Number values, not JSON integer /
        // floating-point literal spellings. Both 64 and 64.0 are the same Number.
        assert_eq!(archive, serde_json::from_value::<Archive>(value).unwrap());
    }
    assert!(strip_preserved_archive(None).is_none());
    assert_eq!(strip_preserved_archive(Some(&json!({"other":"keep-me"}))), Some(json!({"other":"keep-me"})));
    assert_eq!(
        strip_preserved_archive(Some(&json!({"snapcompact":{},"other":"keep-me"}))),
        Some(json!({"other":"keep-me"}))
    );
    assert!(strip_preserved_archive(Some(&json!({"snapcompact":{}}))).is_none());
    let archive: Archive = serde_json::from_value(json!({"frames":[frame],"totalChars":40,"truncatedChars":0,"text":"head middle tail","textHead":"head text","textTail":"tail text"})).unwrap();
    let blocks = history_blocks(&archive, HistoryBlockOptions::default());
    assert_eq!(blocks.len(), 3);
    assert!(block_text(&blocks[0]).contains("head text"));
    assert_eq!(block_data(&blocks[1]), "ZmFrZQ==");
    assert!(block_text(&blocks[2]).contains("tail text"));
    assert!(matches!(&blocks[1], HistoryBlock::Image { detail: Some(v), .. } if v == "original"));
    for (provider, images, cap) in [
        (Some("openrouter"), 90, 80),
        (Some("umans"), 10, 10),
        (None, 5, 5),
        (Some("future-router"), 5, 5),
        (Some("openai-codex"), 200, 80),
        (Some("anthropic"), 90, 80),
    ] {
        let deployment = provider.map(|provider| DeploymentTarget {
            model: ShapeTarget { provider: Some(provider.into()), ..Default::default() },
            ..Default::default()
        });
        assert_eq!(provider_image_budget(deployment.as_ref()).unwrap(), images);
        assert_eq!(provider_frame_budget(deployment.as_ref()).unwrap(), cap);
    }
    assert_eq!(max_frames_for_data_budget(&named("5x8-bw"), FRAME_DATA_BYTES_BUDGET), 17);
    assert_eq!(frame_data_bytes(&archive.frames), 8);
}

#[test]
fn upstream_compact_and_recompaction_family() {
    observe_utf16_compaction_contract();
    observe_compact_converter_contract();
    let mut options = CompactionOptions { frame_size: Some(320), ..Default::default() };
    let mut preparation = prep(vec![
        user("Fix the login bug. The token expires too early!"),
        assistant(json!([{"type":"text","text":"Fixed the TTL comparison in src/login.ts."}])),
    ]);
    preparation.file_ops.read.insert("src/auth.ts".into());
    preparation.file_ops.edited.insert("src/login.ts".into());
    let small = compact(&preparation, &options).unwrap();
    let a = archive(&small);
    assert!(a.frames.is_empty() && a.text_tail.is_none() && a.truncated_chars == 0.0);
    assert!(small.summary.contains("FILES\n===================\n# src/\nauth.ts (Read)\nlogin.ts (Write)"));
    for heading in ["HISTORY", "`¶user:`", "`¶call:`", "`¶call:name(args)//intent`"] {
        assert!(small.summary.contains(heading)); // B-9b227d69ec
    }
    assert_eq!(history_blocks(&a, HistoryBlockOptions::default()).len(), 1);
    assert!(matches!(&history_blocks(&a, HistoryBlockOptions::default())[0], HistoryBlock::Text { .. }));
    options.max_frames = Some(2.0);
    let dimmed = compact(&prep(vec![user("Run the suite."), tool("FAIL ".repeat(330))]), &options).unwrap();
    let (_, _, p, _) = rgb(&archive(&dimmed).frames[0].data);
    assert!(ink(&p, [128, 128, 128]));
    options.serialize.dim_tool_results = false;
    let loud = compact(&prep(vec![user("Run."), tool("all good ".repeat(200))]), &options).unwrap();
    let (_, _, p, _) = rgb(&archive(&loud).frames[0].data);
    assert!(!ink(&p, [128, 128, 128]));
    options.serialize.dim_tool_results = true;
    options.max_frames = Some(5.0);
    let first = compact(
        &prep(vec![user(format!(
            "ORIGINAL BEGINNING SENTINEL. {}TAIL sentinel QQZZ.",
            "Important fact number one. ".repeat(400)
        ))]),
        &options,
    )
    .unwrap();
    let a = archive(&first);
    assert_eq!(a.frames.len(), 5);
    assert!(
        a.text_head.as_deref().unwrap().contains("ORIGINAL BEGINNING SENTINEL.")
            && a.text_tail.as_deref().unwrap().contains("TAIL sentinel QQZZ.")
    );
    let blocks = history_blocks(&a, HistoryBlockOptions::default());
    assert_eq!(blocks.iter().filter(|block| matches!(block, HistoryBlock::Image { .. })).count(), 5); // B-92b4fa68de
    assert!(
        block_text(&blocks[0]).contains("imaged middle below")
            && block_text(blocks.last().unwrap()).contains("imaged middle above")
    );
    let mut next = prep(vec![user("A short follow-up turn.")]);
    next.previous_summary = Some(first.summary.clone());
    next.previous_preserve_data = first.preserve_data.clone();
    let second = compact(&next, &options).unwrap();
    let a = archive(&second);
    assert_eq!(a.frames.len(), 5);
    assert!(
        a.text.as_deref().unwrap().contains("A short follow-up turn.")
            && a.text_head.as_deref().unwrap().contains("ORIGINAL BEGINNING SENTINEL.")
    );
    assert!(a.text.as_deref().unwrap().contains("ORIGINAL BEGINNING SENTINEL.")); // B-bd6dcefc46
    assert!(a.text_tail.as_deref().or(a.text_head.as_deref()).unwrap().contains("A short follow-up turn."));
    // B-a82aab6efb, B-bd6dcefc46: exact original first-turn inputs, including
    // the chronological follow-up at the retained text edge.
    for source in [
        "A long first turn. ".repeat(500),
        format!("ORIGINAL BEGINNING SENTINEL. {}", "A long first turn. ".repeat(500)),
    ] {
        let original = compact(&prep(vec![user(&source)]), &options).unwrap();
        let mut follow = prep(vec![user("A short follow-up turn.")]);
        follow.previous_summary = Some(original.summary);
        follow.previous_preserve_data = original.preserve_data;
        let rerendered = archive(&compact(&follow, &options).unwrap());
        // Only B-a82aab6efb specifies five frames. B-bd6dcefc46 checks
        // chronological text continuity; the fixed source produces four.
        if !source.starts_with("ORIGINAL") {
            assert_eq!(rerendered.frames.len(), 5, "B-a82aab6efb original first-turn fixture");
        }
        assert!(rerendered.text.as_deref().unwrap().contains("A short follow-up turn."));
        assert!(
            rerendered
                .text_tail
                .as_deref()
                .or(rerendered.text_head.as_deref())
                .unwrap()
                .contains("A short follow-up turn.")
        );
        if source.starts_with("ORIGINAL") {
            assert!(rerendered.text.as_deref().unwrap().contains("ORIGINAL BEGINNING SENTINEL."));
            assert!(rerendered.text_head.as_deref().unwrap().contains("ORIGINAL BEGINNING SENTINEL."));
        }
    }
    options.model = Some(target("anthropic-messages", ""));
    options.max_frames = Some(7.0);
    let hq = compact(
        &prep(vec![user(format!("HEAD sentinel. {}TAIL sentinel.", "Important fact number one. ".repeat(1000)))]),
        &options,
    )
    .unwrap();
    let a = archive(&hq);
    assert_eq!(a.frames.len(), 7);
    let hi_cols = geometry(&resolve_shape(options.model.as_ref(), None).unwrap(), Some(320)).cols as f64;
    assert!(a.frames[..3].iter().chain(&a.frames[4..]).all(|f| f.cols == hi_cols));
    assert!(a.frames[3].cols > hi_cols);
    assert!(hq.summary.contains(&format!("{} or {} characters wide", hi_cols, a.frames[3].cols)));
    let mut shrink = prep(Vec::new());
    shrink.previous_summary = Some(hq.summary.clone());
    shrink.previous_preserve_data = hq.preserve_data.clone();
    options.max_frames = Some(3.0);
    let shrunk = archive(&compact(&shrink, &options).unwrap());
    assert!(!shrunk.frames.is_empty() && shrunk.frames.len() <= 3);
    assert!(shrunk.text_head.as_deref().unwrap().contains("HEAD sentinel."));
    let silver = compact(
        &prep(vec![user("你好世界".repeat(200))]),
        &CompactionOptions {
            shape: Some(named("silver16-bw")),
            frame_size: Some(64),
            max_frames: Some(1.0),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!archive(&silver).frames.is_empty()); // B-05f97f733c
    assert!(archive(&silver).frames.iter().all(|f| f.metadata["font"] == "silver"));
    let mut legacy = prep(vec![user("New work after a legacy archive.")]);
    legacy.previous_summary = Some("Legacy beginning summary: user approved PLAN.md and started auth work.".into());
    legacy.previous_preserve_data = Some(
        json!({"snapcompact":{"frames":[{"data":"bGVnYWN5LWZyYW1l","mimeType":"image/png","cols":1,"rows":1,"chars":12}],"totalChars":12,"truncatedChars":0}}),
    );
    let result = compact(&legacy, &options).unwrap();
    assert!(
        archive(&result).text.unwrap().contains("Legacy beginning summary")
            && result.summary.contains("condensed digest of still-older context")
    );
    legacy.previous_preserve_data = None;
    let result = compact(&legacy, &options).unwrap();
    assert!(result.summary.contains("condensed digest of still-older context"));
    let mut preserve = second.preserve_data.unwrap();
    preserve["openaiRemoteCompaction"] = json!({"provider":"openai","replacementHistory":[]});
    preserve["appKey"] = json!("kept");
    next.previous_preserve_data = Some(preserve);
    let result = compact(&next, &options).unwrap();
    assert!(!result.summary.contains("[Summary of earlier history]")); // B-3402faf767
    assert!(result.preserve_data.as_ref().unwrap().get("openaiRemoteCompaction").is_none());
    assert_eq!(result.preserve_data.unwrap()["appKey"], "kept");
    let thought = compact(&prep(vec![user("Investigate flaky auth."), assistant(json!([{"type":"thinking","thinking":"legacy reasoning text"},{"type":"text","text":"The token clock is skewed."}]))]), &options).unwrap();
    let mut follow = prep(vec![user("Another turn.")]);
    follow.previous_preserve_data = thought.preserve_data.clone();
    assert!(archive(&compact(&follow, &options).unwrap()).text.unwrap().contains("¶think:"));
    options.serialize.include_thinking = false;
    let healed = archive(&compact(&follow, &options).unwrap()).text.unwrap();
    assert!(
        !healed.contains("¶think:")
            && !healed.contains("legacy reasoning text")
            && healed.contains("Investigate flaky auth.")
            && healed.contains("The token clock is skewed.")
    );
}

fn observe_utf16_archive_contract() {
    let frame_json = r#"{"data":"D\ud800","mimeType":"M\udc00","cols":1,"rows":2,"chars":3,"detail":"\ud800","future":{"text":"\udc00"}}"#;
    let preserve = raw_json(&format!(
        r#"{{"snapcompact":{{"frames":[{frame_json}],"totalChars":7,"truncatedChars":2,"textHead":"H\ud800","textTail":"\udc00T"}},"unknown":{{ "raw" : "\ud800" }}}}"#
    ));
    let exact = get_preserved_archive_utf16(Some(&preserve)).unwrap().unwrap();
    assert_eq!(exact.frames.len(), 1);
    assert_eq!(exact.frames[0].source.get(), frame_json);
    assert_eq!(exact.frames[0].data.as_units(), &[b'D' as u16, 0xd800]);
    assert_eq!(frame_data_bytes_utf16(&exact.frames), 2);
    assert_eq!(archive_source_text_utf16(&exact).unwrap().as_units(), &[b'H' as u16, 0xd800, 10, 0xdc00, b'T' as u16]);
    let blocks = history_blocks_utf16(&exact, HistoryBlockOptions::default()).unwrap();
    assert_eq!(blocks.len(), 3);
    match &blocks[1] {
        HistoryBlockUtf16::Image { data, mime_type, detail } => {
            assert_eq!(data.as_units(), &[b'D' as u16, 0xd800]);
            assert_eq!(mime_type.as_units(), &[b'M' as u16, 0xdc00]);
            assert_eq!(detail.as_ref().unwrap().get(), r#""\ud800""#);
        }
        _ => panic!("exact image block expected"),
    }
    let stripped = strip_preserved_archive_utf16(Some(&preserve)).unwrap().unwrap();
    assert!(stripped.get().contains(r#"{ "raw" : "\ud800" }"#));
    assert!(!stripped.get().contains("snapcompact"));
    let omitted = history_blocks_utf16(&exact, HistoryBlockOptions { max_frame_data_bytes: Some(1) }).unwrap();
    // b91db66e keeps each gap at its chronological position: the head remains
    // its own block; the notice and tail share the second text block.
    assert_eq!(omitted.len(), 2);
    let texts = omitted
        .iter()
        .map(|block| match block {
            HistoryBlockUtf16::Text { text } => text.clone(),
            _ => panic!("budgeted text block expected"),
        })
        .collect::<Vec<_>>();
    let text = Utf16Text::join(&texts, "\n");
    assert!(text.as_units().contains(&0xd800) && text.as_units().contains(&0xdc00));
    assert!(texts[1].to_lossy_string().contains("1 archived image frame could not be included. 2 B of base64"));
}

fn observe_utf16_compaction_contract() {
    let options = CompactionOptions {
        frame_size: Some(64),
        shape: Some(named("silver16-bw")),
        max_frames: Some(1.0),
        ..Default::default()
    };
    let mut prior = exact_prep(&prep(vec![user("next")]));
    let unknown_span = r#"{ "raw" : "\ud800", "nested": [1,{"text":"\udc00"}] }"#;
    prior.previous_preserve_data = Some(raw_json(&format!(
        r#"{{"appKey":{unknown_span},"openaiRemoteCompaction":{{"old":true}},"snapcompact":{{"frames":[],"totalChars":3,"truncatedChars":0,"text":"H\ud800T","textHead":"H\ud800T"}}}}"#
    )));
    let first = compact_utf16(&prior, &options).unwrap();
    let raw_preserve = first.preserve_data.as_ref().unwrap();
    assert!(raw_preserve.get().contains(unknown_span));
    assert!(!raw_preserve.get().contains("openaiRemoteCompaction"));
    let archive = get_preserved_archive_utf16(Some(raw_preserve)).unwrap().unwrap();
    let mut expected = vec![b'H' as u16, 0xd800, b'T' as u16, 0x2588];
    expected.extend("¶user:next".encode_utf16());
    assert_eq!(archive.text.as_ref().unwrap().as_units(), expected);
    let mut follow = exact_prep(&prep(vec![user("again")]));
    follow.previous_preserve_data = first.preserve_data.clone();
    let next = compact_utf16(&follow, &options).unwrap();
    let archive = get_preserved_archive_utf16(next.preserve_data.as_deref()).unwrap().unwrap();
    expected.push(0x2588);
    expected.extend("¶user:again".encode_utf16());
    assert_eq!(archive.text.as_ref().unwrap().as_units(), expected);
    assert!(next.preserve_data.as_ref().unwrap().get().contains(unknown_span));

    // A legal supplementary scalar survives fresh Silver normalization; an
    // archive UTF16 edge then splits the pair without replacing either unit.
    let source = format!("{}\u{10000}{}", "a".repeat(9), "b".repeat(40));
    let fresh = exact_prep(&prep(vec![user(&source)]));
    let fresh_options = CompactionOptions { max_frames: Some(3.0), ..options.clone() };
    let fresh_result = compact_utf16(&fresh, &fresh_options).unwrap();
    let fresh_archive = get_preserved_archive_utf16(fresh_result.preserve_data.as_deref()).unwrap().unwrap();
    assert_eq!(fresh_archive.text_head.as_ref().unwrap().as_units().last(), Some(&0xd800));
    assert_eq!(fresh_archive.text_head.as_ref().unwrap().len(), 16);
    let mut full_expected: Vec<_> = "¶user:".encode_utf16().collect();
    full_expected.extend(source.encode_utf16());
    assert_eq!(fresh_archive.text.as_ref().unwrap().as_units(), full_expected);
    let scalar_failure = ara_snapcompact::compact(&prep(vec![user(source)]), &fresh_options).unwrap_err();
    assert!(scalar_failure.0.contains("scalar result cannot represent exact archive"));

    let mut raw = exact_prep(&prep(Vec::new()));
    raw.messages_to_summarize = vec![raw_json(r#"{"tag":"history","text":"H\ud800"}"#)];
    raw.turn_prefix_messages = vec![raw_json(r#"{"tag":"prefix","text":"P\udc00"}"#)];
    let combined: Vec<_> =
        raw.messages_to_summarize.iter().chain(&raw.turn_prefix_messages).map(|m| m.get().to_owned()).collect();
    let calls = std::cell::Cell::new(0);
    let converted = compact_with_converter_utf16(&raw, &options, |messages| {
        calls.set(calls.get() + 1);
        assert_eq!(messages.iter().map(|m| m.get().to_owned()).collect::<Vec<_>>(), combined);
        Ok(vec![raw_json(r#"{"role":"user","content":"converted"}"#)])
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    assert!(
        get_preserved_archive_utf16(converted.preserve_data.as_deref())
            .unwrap()
            .unwrap()
            .text
            .unwrap()
            .to_scalar()
            .unwrap()
            .contains("converted")
    );
    let failure = Error::from_reason("exact converter failure");
    assert_eq!(compact_with_converter_utf16(&raw, &options, |_| Err(failure.clone())).unwrap_err(), failure);
    raw.first_kept_entry_id.clear();
    assert_eq!(
        compact_with_converter_utf16(&raw, &options, |_| panic!("ID precedes exact conversion")).unwrap_err().0,
        "First kept entry has no ID - session may need migration"
    );
}

fn observe_utf16_normalization_boundary() {
    let barrier = Utf16Text::from_units(vec![b'a' as u16, 10, 0xd800, 10, b'b' as u16]);
    assert_eq!(normalize_utf16(&barrier, NormalizeOptions::default()), "a██b");
    let lone = Utf16Text::from_units(vec![0xd800]);
    assert!(lone.to_scalar().is_err());
    assert_eq!(normalize_utf16(&lone, NormalizeOptions::default()), "");
    let replacement = Utf16Text::from("\u{fffd}");
    assert_ne!(lone.as_units(), replacement.as_units());
    let rendered_replacement = normalize_utf16(&replacement, NormalizeOptions::default());
    assert_eq!(rendered_replacement, normalize("\u{fffd}", NormalizeOptions::default()));
    assert!(!rendered_replacement.is_empty());
    assert_eq!(
        scan_renderability_utf16(&replacement, NormalizeOptions::default()),
        scan_renderability("\u{fffd}", NormalizeOptions::default())
    );
    // U+1D400 becomes a scalar only after the source's ANSI/Cf removal.
    // NFKD must observe that reformed pair and produce the ordinary letter A.
    for between in [vec![0x200b], "\u{1b}[31m".encode_utf16().collect()] {
        let mut units = vec![0xd835];
        units.extend(between);
        units.push(0xdc00);
        assert_eq!(normalize_utf16(&Utf16Text::from_units(units), NormalizeOptions::default()), "A");
    }
}

fn observe_utf16_serializer_boundaries() {
    let options = SerializeOptions::default();
    let deep_argument = format!("{}null{}", "[".repeat(140), "]".repeat(140));
    let deep = raw_json(&format!(
        r#"{{"role":"assistant","content":[{{"type":"toolCall","id":"deep","name":"keep","arguments":{{"value":{deep_argument}}}}}]}}"#
    ));
    assert_eq!(
        serialize_conversation_raw_utf16(&[deep], &options).unwrap().to_scalar().unwrap(),
        format!("¶call:keep(value={deep_argument})")
    );
    let raw =
        serialize_conversation_raw_utf16(&[raw_json(r#"{"role":"user","content":"A\ud800B"}"#)], &options).unwrap();
    let mut raw_expected: Vec<_> = "¶user:A".encode_utf16().collect();
    raw_expected.push(0xd800);
    raw_expected.push(b'B' as u16);
    assert_eq!(raw.as_units(), raw_expected);
    let args = raw_json(
        r#"{"role":"assistant","content":[{"type":"toolCall","id":"call","name":"write","arguments":{"10":"ten","2":"two","name":"first","name":"second","lone":"\ud800","n":-0}}]}"#,
    );
    assert_eq!(
        serialize_conversation_raw_utf16(&[args], &options).unwrap().to_scalar().unwrap(),
        r#"¶call:write(2="two", 10="ten", name="second", lone="\ud800", n=0)"#
    );
    let ids = vec![
        raw_json(
            r#"{"role":"assistant","content":[{"type":"toolCall","id":"\ud800","name":"high","arguments":{}},{"type":"toolCall","id":"\udc00","name":"low","arguments":{}}]}"#,
        ),
        raw_json(r#"{"role":"toolResult","toolCallId":"\ud800","content":[{"type":"text","text":"HIGH"}]}"#),
        raw_json(r#"{"role":"toolResult","toolCallId":"\udc00","content":[{"type":"text","text":"LOW"}]}"#),
    ];
    let paired = serialize_conversation_raw_utf16(&ids, &SerializeOptions { dim_tool_results: false, ..options })
        .unwrap()
        .to_scalar()
        .unwrap();
    assert_eq!(paired, "¶call:high()\n<out>\nHIGH\n</out>\nlow()\n<out>\nLOW\n</out>");
    for (input, body) in [
        (format!("{}😀{}", "a".repeat(1199), "b".repeat(2500)), {
            let mut units: Vec<_> = "a".repeat(1199).encode_utf16().collect();
            units.push(0xd83d);
            units.extend(" […1701ch elided…] ".encode_utf16());
            units.extend("b".repeat(800).encode_utf16());
            units
        }),
        (format!("{}😀{}", "a".repeat(2500), "b".repeat(799)), {
            let mut units: Vec<_> = "a".repeat(1200).encode_utf16().collect();
            units.extend(" […1301ch elided…] ".encode_utf16());
            units.push(0xde00);
            units.extend("b".repeat(799).encode_utf16());
            units
        }),
    ] {
        let mut expected: Vec<_> = "¶call:\n<out>\n\u{e}".encode_utf16().collect();
        expected.extend(body);
        expected.extend("\u{f}\n</out>".encode_utf16());
        let exact = serialize_conversation_utf16(&[tool(input)], &options);
        assert_eq!(exact.as_units(), expected);
        assert!(exact.to_scalar().is_err());
        assert!(!exact.as_units().contains(&0xfffd));
    }
    let value = format!("aaaa😀{}", "b".repeat(20));
    let source = assistant(json!([{"type":"toolCall","id":"nested","name":"write","arguments":{"content":value}}]));
    let nested_options = SerializeOptions { tool_arg_max_chars: 10.0, tool_call_max_chars: 30.0, ..options };
    // Source arithmetic: JSON-string length 28, inner head 6/tail 4;
    // content= plus the result is 35 units, outer head 18/tail 12.
    let mut inner: Vec<_> = "\"aaaa".encode_utf16().collect();
    inner.push(0xd83d);
    inner.extend(" […18ch elided…] bbb\"".encode_utf16());
    let mut args: Vec<_> = "content=".encode_utf16().collect();
    args.extend(inner);
    assert_eq!(args.len(), 35);
    let mut expected: Vec<_> = "¶call:write(".encode_utf16().collect();
    expected.extend_from_slice(&args[..18]);
    expected.extend(" […5ch elided…] ".encode_utf16());
    expected.extend_from_slice(&args[args.len() - 12..]);
    expected.push(b')' as u16);
    let exact = serialize_conversation_utf16(&[source], &nested_options);
    assert_eq!(exact.as_units(), expected);
    assert!(exact.as_units().contains(&0xd83d));
    assert!(!exact.as_units().contains(&0xfffd));
}

fn observe_compact_converter_contract() {
    let options = CompactionOptions { frame_size: Some(320), ..Default::default() };
    let mut raw = prep(vec![json!({"hostTag":"history-one","body":"converted history"})]);
    raw.turn_prefix_messages =
        vec![json!({"hostTag":"prefix-two","body":"converted prefix"}), json!({"hostTag":"omit"})];
    let combined: Vec<_> = raw.messages_to_summarize.iter().chain(&raw.turn_prefix_messages).cloned().collect();
    let calls = std::cell::Cell::new(0);
    let converted = compact_with_converter(&raw, &options, |messages| {
        calls.set(calls.get() + 1);
        assert_eq!(messages, combined, "one ordered concatenate precedes conversion");
        Ok(messages.into_iter().filter_map(|message| message["body"].as_str().map(user)).collect())
    })
    .unwrap();
    assert_eq!(calls.get(), 1);
    let text = archive(&converted).text.unwrap();
    assert!(text.find("converted history").unwrap() < text.find("converted prefix").unwrap());
    assert!(!text.contains("hostTag") && !text.contains("omit"));
    let mut model_messages = prep(vec![user("converted history")]);
    model_messages.turn_prefix_messages = vec![user("converted prefix")];
    assert_eq!(converted, compact(&model_messages, &options).unwrap());
    assert_eq!(
        compact(&model_messages, &options).unwrap(),
        compact_with_converter(&model_messages, &options, Ok).unwrap()
    );
    let failure = Error::from_reason("synthetic Host converter failure");
    let failed = compact_with_converter(&raw, &options, |messages| {
        calls.set(calls.get() + 1);
        assert_eq!(messages, combined);
        Err(failure.clone())
    })
    .unwrap_err();
    assert_eq!(failed, failure);
    assert_eq!(calls.get(), 2);
    raw.first_kept_entry_id.clear();
    let missing = compact_with_converter(&raw, &options, |_| panic!("missing ID must fail before invoking converter"))
        .unwrap_err();
    assert_eq!(missing.0, "First kept entry has no ID - session may need migration");
    assert_eq!(calls.get(), 2);
}

#[test]
fn upstream_atomic_data_url_healing_family() {
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"16\" height=\"16\"><desc>{}</desc></svg>",
        "A".repeat(6000)
    );
    let payload = STANDARD.encode(svg);
    let url = format!("data:image/svg+xml;base64,{payload}");
    let placeholder = format!("[data URL omitted: image/svg+xml, {} base64 chars]", payload.len());
    let options = SerializeOptions::default();
    for text in [format!("Reader output:\n\n![inline SVG]({url})\ntrailing text"), format!("see {url} end")] {
        let has_trailing_text = text.contains("trailing text");
        let out = serialize_conversation(&[tool(text)], &options);
        assert!(out.contains(&placeholder) && !out.contains(";base64,"));
        if has_trailing_text {
            assert!(out.contains("trailing text"));
        } // B-77a78c4e9c
    }
    for pad in [0, 600, 1150, 1199, 4000, 6500, 7400] {
        let text = format!("{} ![img]({url}) {}", "p".repeat(pad), "s".repeat(7600 - pad));
        let out = normalize(&serialize_conversation(&[tool(text)], &options), NormalizeOptions::default());
        assert!(!out.contains(";base64,"));
    }
    let out = serialize_conversation(
        &[assistant(
            json!([{"type":"toolCall","id":"c1","name":"write","arguments":{"path":"a.html","content":format!("<img src=\"{url}\">")}}]),
        )],
        &options,
    );
    assert!(out.contains("data URL omitted: image/svg+xml") && !out.contains(";base64,"));
    let gif = "R0lGODlhAQABAAAAACH5BAEKAAEALAAAAAABAAEAAAICTAEAOw==";
    let out = serialize_conversation(&[tool(format!("const BLANK = \"data:image/gif;base64,{gif}\";"))], &options);
    assert!(out.contains(&format!("[data URL omitted: image/gif, {} base64 chars]", gif.len())));
    assert!(!out.to_ascii_lowercase().contains(";base64,")); // B-b2cafc930a
    let prose = "e.g. 'data:image/png;base64,abc' is the expected shape";
    let prose_output = serialize_conversation(&[tool(prose)], &options);
    assert!(prose_output.contains(prose));
    assert!(!prose_output.contains("data URL omitted")); // B-06a7769852
    for (url, mime) in [
        (format!("data:image/svg+xml;charset=utf-8;base64,{payload}"), "image/svg+xml;charset=utf-8"),
        (format!("DATA:IMAGE/PNG;BASE64,{payload}"), "IMAGE/PNG"),
    ] {
        let out = serialize_conversation(&[tool(format!("see {url} end"))], &options);
        assert!(out.contains(&format!("[data URL omitted: {mime}, {} base64 chars]", payload.len())));
        assert!(!out.to_ascii_lowercase().contains(";base64,")); // B-5d6e6bd14f / B-048cfe045d
    }
    let compact_options = CompactionOptions { frame_size: Some(320), max_frames: Some(3.0), ..Default::default() };
    let result = compact(
        &prep(vec![user(format!("{}![img]({url}){}", "context ".repeat(400), " more".repeat(400)))]),
        &compact_options,
    )
    .unwrap();
    let projection = archive(&result);
    let combined = [projection.text.as_deref(), projection.text_head.as_deref(), projection.text_tail.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!combined.contains(";base64,")); // B-5c6fa40d24: all retained text surfaces.
    let old = format!(
        "earlier work ![a]({url}) then data:image/png;base64,{} [...900ch elided...] {} and data:image/webp;base64, [...123ch elided...] QUFB plus a bare cut data:image/jpeg;base64,",
        "QUFB".repeat(50),
        "QUFB".repeat(10)
    );
    let mut legacy = prep(vec![user("Next turn after legacy archive.")]);
    legacy.previous_preserve_data = Some(
        json!({"snapcompact":{"frames":[],"totalChars":old.len(),"truncatedChars":0,"textHead":old,"textTail":"recent tail"}}),
    );
    let healed = archive(&compact(&legacy, &compact_options).unwrap());
    let text = [healed.text.as_deref(), healed.text_head.as_deref(), healed.text_tail.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!text.contains(";base64,") && text.contains("data URL omitted: image/webp"));
    assert!(text.contains(&placeholder)); // B-811162d0a9: original SVG marker and full projection.
    for (head, mime, retain) in [
        ("history ![x](data:image/svg+xml;base64,".into(), "image/svg+xml", None),
        ("history ![x](data:image/svg+xml;base64,Q".into(), "image/svg+xml", None),
        (format!("history ![x](data:image/svg+xml;base64,{}QUL", "QUFB".repeat(9)), "image/svg+xml", None),
        ("data:image/webp;base64, [...900ch elided...] QUFB rest".into(), "image/webp", Some(" rest")),
        ("data:image/svg+xml;charset=utf-8;base64,Q".into(), "image/svg+xml;charset=utf-8", None),
    ] {
        let archive = Archive {
            text_head: Some(head),
            text_tail: Some("tail".into()),
            truncated_chars: 5.0,
            ..Default::default()
        };
        let blocks = history_blocks(&archive, HistoryBlockOptions::default());
        let text = blocks.iter().map(block_text).collect::<Vec<_>>().join("\n");
        assert!(!text.contains(";base64,") && text.contains(&format!("data URL omitted: {mime}"))); // B-39d8f5caec
        if let Some(retain) = retain {
            assert!(text.contains(retain) && !text.contains("ch elided"));
        }
    }
    let archive = Archive {
        text_head: Some("data:image/png;base64,QUFB [...900ch elided...] QUFB".into()),
        ..Default::default()
    };
    assert_eq!(archive_source_text(&archive).unwrap(), "[data URL omitted: image/png, 908 base64 chars]");
    let started = std::time::Instant::now();
    let out = serialize_conversation(&[tool(format!("{};base64,", "[".repeat(80000)))], &options);
    assert!(started.elapsed() < std::time::Duration::from_millis(2000)); // B-bd4fa26bc0: original timeout.
    assert!(out.contains("[…78008ch elided…]"));
    assert!(out.contains(&format!(
        "{DIM_ON}{} […78008ch elided…] {};base64,{DIM_OFF}",
        "[".repeat(1200),
        "[".repeat(792)
    )));
}
