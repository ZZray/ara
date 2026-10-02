//! Aggregated fixed-OMP module families, not an additional micro-test matrix.
//! Source: packages/snapcompact/test/snapcompact.test.ts at 596f2da (MIT).
//! Original native rasterizer tests remain in src/native.rs.
use ara_snapcompact::*;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::io::Cursor;

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
    ShapeTarget { api: Some(api.into()), id: (!id.is_empty()).then(|| id.into()) }
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
    for (api, height, width, bill, detail) in [
        ("anthropic-messages", 16.0, 11.0, 3293.0, None),
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
        ("8x13", 11.0, 1932.0, (61.0_f64.powi(2) * 1.2).ceil(), Some("original"))
    );
    let vertex = resolve_shape(Some(&target("google-vertex", "claude-fable-5@20250929")), None).unwrap();
    assert_eq!((vertex.cell_width, vertex.frame_size, vertex.frame_token_estimate), (11.0, 1932.0, 1120.0));
    let gemini = resolve_shape(Some(&target("google-generative-ai", "gemini-3.5-flash")), None).unwrap();
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
    }
    let mut malformed = serde_json::to_value(named("doc-8on16-sent-dim")).unwrap();
    for (key, bad) in
        [("columns", json!(3)), ("stretch", json!("no")), ("stopwordDim", json!(1)), ("font", json!("9x9"))]
    {
        let old = malformed[key].clone();
        malformed[key] = bad;
        assert!(!is_shape(&malformed));
        malformed[key] = old;
    }
    let cjk = "你好世界测试文本数据记录";
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
}

#[test]
fn upstream_render_and_render_many_family() {
    let legacy = named("5x8-sent");
    assert_eq!(geometry(&legacy, Some(320)), Geometry { cols: 64, rows: 40, capacity: 2560 });
    let frame = render("First sentence here. Second one differs.", &legacy, Some(320)).unwrap();
    assert_eq!((frame.cols, frame.rows, frame.chars), (64, 40, 40));
    let (w, h, pixels, color) = rgb(&frame.data);
    assert_eq!((w, h, color), (320, 8, 3));
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
    assert_eq!(rgb(&render("Hello world.", &named("6x6u-sent"), Some(320)).unwrap().data).3, 2);
    let silver = named("silver16-bw");
    let frame = render("你好안녕", &silver, Some(64)).unwrap();
    assert_eq!(frame.chars, 4);
    let (w, h, _, color) = rgb(&frame.data);
    assert_eq!((w, h, color), (64, 16, 2));
    let bitmap = named("8on16-bw");
    let frame = render("你", &bitmap, Some(64)).unwrap();
    assert_eq!(frame.chars, 1);
    let (w, h, p, _) = rgb(&frame.data);
    assert!((0..h).any(|y| (8..16).any(|x| p[(y * w + x) * 3..(y * w + x) * 3 + 3] == [0, 0, 0])));
    let frame = render(&"x".repeat(3060), &legacy, Some(320)).unwrap();
    assert_eq!(frame.chars, 2560);
    let (_, _, p, _) = rgb(&render("a█b", &legacy, Some(320)).unwrap().data);
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
}

#[test]
fn upstream_serializer_family() {
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
    assert_eq!(
        serialize_conversation(&[blank, tool("body")], &plain),
        "¶call:read(path=\"a.ts\")\n<out>\nbody\n</out>"
    );
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
        assert_eq!(provider_image_budget(provider), images);
        assert_eq!(provider_frame_budget(provider), cap);
    }
    assert_eq!(max_frames_for_data_budget(None), 17);
    assert_eq!(frame_data_bytes(&archive.frames), 8);
}

#[test]
fn upstream_compact_and_recompaction_family() {
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
    assert_eq!(history_blocks(&a, HistoryBlockOptions::default()).len(), 1);
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
        let out = serialize_conversation(&[tool(text)], &options);
        assert!(out.contains(&placeholder) && !out.contains(";base64,"));
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
    let prose = "e.g. 'data:image/png;base64,abc' is the expected shape";
    assert!(serialize_conversation(&[tool(prose)], &options).contains(prose));
    for (url, mime) in [
        (format!("data:image/svg+xml;charset=utf-8;base64,{payload}"), "image/svg+xml;charset=utf-8"),
        (format!("DATA:IMAGE/PNG;BASE64,{payload}"), "IMAGE/PNG"),
    ] {
        let out = serialize_conversation(&[tool(format!("see {url} end"))], &options);
        assert!(out.contains(&format!("[data URL omitted: {mime}, {} base64 chars]", payload.len())));
    }
    let compact_options = CompactionOptions { frame_size: Some(320), max_frames: Some(3.0), ..Default::default() };
    let result = compact(
        &prep(vec![user(format!("{}![img]({url}){}", "context ".repeat(400), " more".repeat(400)))]),
        &compact_options,
    )
    .unwrap();
    assert!(!archive(&result).text.unwrap().contains(";base64,"));
    let old = format!(
        "earlier work ![a]({url}) then data:image/png;base64,{} [...900ch elided...] {} and data:image/webp;base64, [...123ch elided...] QUFB plus a bare cut data:image/jpeg;base64,",
        "QUFB".repeat(50),
        "QUFB".repeat(10)
    );
    let mut legacy = prep(vec![user("Next turn after legacy archive.")]);
    legacy.previous_preserve_data = Some(
        json!({"snapcompact":{"frames":[],"totalChars":old.len(),"truncatedChars":0,"textHead":old,"textTail":"recent tail"}}),
    );
    let text = archive(&compact(&legacy, &compact_options).unwrap()).text.unwrap();
    assert!(!text.contains(";base64,") && text.contains("data URL omitted: image/webp"));
    for head in [
        "history ![x](data:image/svg+xml;base64,".into(),
        "history ![x](data:image/svg+xml;base64,Q".into(),
        format!("history ![x](data:image/svg+xml;base64,{}QUL", "QUFB".repeat(9)),
        "data:image/webp;base64, [...900ch elided...] QUFB rest".into(),
        "data:image/svg+xml;charset=utf-8;base64,Q".into(),
    ] {
        let archive = Archive {
            text_head: Some(head),
            text_tail: Some("tail".into()),
            truncated_chars: 5.0,
            ..Default::default()
        };
        let blocks = history_blocks(&archive, HistoryBlockOptions::default());
        let text = blocks.iter().map(block_text).collect::<Vec<_>>().join("\n");
        assert!(!text.contains(";base64,") && text.contains("data URL omitted:"));
    }
    let archive = Archive {
        text_head: Some("data:image/png;base64,QUFB [...900ch elided...] QUFB".into()),
        ..Default::default()
    };
    assert_eq!(archive_source_text(&archive).unwrap(), "[data URL omitted: image/png, 908 base64 chars]");
    let out = serialize_conversation(&[tool(format!("{};base64,", "[".repeat(80000)))], &options);
    assert!(out.contains("[…78008ch elided…]"));
}
