//! All 73 fixed OMP streaming originals. No original ID/input/assertion removed.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU8, Ordering};
static GROUP_LOCK: Mutex<()> = Mutex::new(());
fn plain(text: &str) -> String {
    regex::Regex::new(r"\x1b\]8;[^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;:]*m")
        .unwrap()
        .replace_all(text, "")
        .into_owned()
}
fn base() -> (Arc<Theme>, MarkdownOptions) {
    super::capture_tests::fixed_default_markdown_theme()
}
fn make(text: WireString, theme: &Arc<Theme>, options: &MarkdownOptions) -> MarkdownComponent {
    let mut md = MarkdownComponent::new(text, theme.clone());
    md.set_options(options.clone());
    md
}
fn cold(
    text: WireString,
    width: usize,
    theme: &Arc<Theme>,
    options: &MarkdownOptions,
    transient: bool,
) -> Arc<Vec<String>> {
    clear_render_cache();
    let mut md = make(text, theme, options);
    md.set_transient_render_cache(transient);
    let rows = md.render(width);
    clear_render_cache();
    rows
}
fn reveal(
    full: &str,
    width: usize,
    step: usize,
    transient: bool,
    theme: &Arc<Theme>,
    options: &MarkdownOptions,
    md: &mut MarkdownComponent,
    id: &str,
) {
    md.set_transient_render_cache(transient);
    let units: Vec<u16> = full.encode_utf16().collect();
    for len in (1..=units.len()).step_by(step) {
        let part = WireString::from_units(units[..len].to_vec());
        clear_render_cache();
        md.set_text(part.clone());
        assert_eq!(md.render(width).as_ref(), cold(part, width, theme, options, false).as_ref(), "{id} frame {len}");
    }
    clear_render_cache();
    let full = WireString::from(full);
    md.set_text(full.clone());
    assert_eq!(md.render(width).as_ref(), cold(full, width, theme, options, false).as_ref(), "{id} final frame");
}
fn text_var<'a>(case: &'a Value, name: &str) -> &'a str {
    case["variables"][name].as_str().unwrap_or_else(|| panic!("unresolved original variable {name} {}", case["id"]))
}
fn render_equal(
    md: &mut MarkdownComponent,
    text: &str,
    width: usize,
    theme: &Arc<Theme>,
    options: &MarkdownOptions,
    transient_cold: bool,
    id: &str,
) {
    clear_render_cache();
    md.set_text(text.into());
    assert_eq!(md.render(width).as_ref(), cold(text.into(), width, theme, options, transient_cold).as_ref(), "{id}");
}
fn helper_cases(case: &Value, theme: &Arc<Theme>, base: &MarkdownOptions) {
    let id = case["id"].as_str().unwrap();
    for helper in case["helpers"].as_array().unwrap() {
        let args = helper["args"].as_array().unwrap();
        match helper["name"].as_str().unwrap() {
            "assertIdenticalGrowth" | "assertIdenticalGrowthTransient" => {
                let width = args.get(1).and_then(Value::as_u64).unwrap_or(60) as usize;
                let step = args.get(2).and_then(Value::as_u64).unwrap_or(13) as usize;
                let mut md = make("".into(), theme, base);
                reveal(
                    args[0].as_str().unwrap(),
                    width,
                    step,
                    helper["name"] == "assertIdenticalGrowthTransient",
                    theme,
                    base,
                    &mut md,
                    id,
                );
            }
            "assertByteIdentityEveryFrame" => {
                let opts = args.get(1).cloned().unwrap_or(Value::Null);
                let width = opts["width"].as_u64().unwrap_or(60) as usize;
                let step = opts["step"].as_u64().unwrap_or(1) as usize;
                let mut options = base.clone();
                if opts["defaultTextStyle"]["italic"] == true {
                    options.default_style.italic = true;
                    options.default_style.color = Some(Arc::new(|text| format!("\x1b[35m{text}\x1b[39m")));
                }
                let background = Arc::new(AtomicU8::new(52));
                if opts["defaultTextStyle"].get("bgColor").is_some() {
                    let state = background.clone();
                    options.default_style.bg_color = Some(Arc::new(move |text| {
                        format!("\x1b[48;5;{}m{text}\x1b[0m", state.load(Ordering::Relaxed))
                    }));
                }
                let mut md = make("".into(), theme, &options);
                let full = args[0].as_str().unwrap();
                reveal(full, width, step, true, theme, &options, &mut md, id);
                // Original B-db866680f1 mutates the same style object after
                // reveal; the same closure identity now probes different bytes.
                if opts["defaultTextStyle"].get("bgColor").is_some() {
                    background.store(22, Ordering::Relaxed);
                    render_equal(&mut md, &format!("{full} and more"), 20, theme, &options, false, id);
                }
            }
            "assertSpliceParts" => {
                let width = args.get(2).and_then(Value::as_u64).unwrap_or(60) as usize;
                let mut md = make("".into(), theme, base);
                md.set_transient_render_cache(true);
                let mut out = Arc::new(Vec::new());
                for frame in args[0].as_array().unwrap() {
                    clear_render_cache();
                    md.set_text(frame.as_str().unwrap().into());
                    out = md.render(width);
                }
                assert_eq!(
                    out.as_ref(),
                    cold(args[1].as_str().unwrap().into(), width, theme, base, false).as_ref(),
                    "{id} last frame"
                );
                clear_render_cache();
            }
            "assertChunkedGrowth" => {
                let width = args.get(1).and_then(Value::as_u64).unwrap_or(60) as usize;
                let mut md = make("".into(), theme, base);
                let mut text = String::new();
                for chunk in args[0].as_array().unwrap() {
                    text.push_str(chunk.as_str().unwrap());
                    render_equal(&mut md, &text, width, theme, base, false, id);
                }
            }
            name => panic!("untranslated original helper {name} {id}"),
        }
    }
}
#[derive(Clone)]
struct TestHighlightStream {
    pushes: Arc<Mutex<Vec<String>>>,
    consumed: String,
}
impl MarkdownHighlightSession for TestHighlightStream {
    fn push(&mut self, chunk: &str) -> String {
        self.pushes.lock().unwrap().push(chunk.into());
        self.consumed.push_str(chunk);
        let mut parts = chunk.split('\n').peekable();
        let mut out = String::new();
        while let Some(line) = parts.next() {
            if parts.peek().is_some() {
                out.push_str(&format!("S<{line}>\n"));
            } else {
                out.push_str(line);
            }
        }
        out
    }
    fn frame_copy(&self) -> Box<dyn MarkdownHighlightSession> {
        Box::new(self.clone())
    }
}
fn custom_case(case: &Value, theme: &Arc<Theme>, base: &MarkdownOptions) {
    let id = case["id"].as_str().unwrap();
    let mut md = make("".into(), theme, base);
    match id {
        "B-7a289e77ce" | "B-5cd6baf925" | "B-57c11dfb9d" => {
            let full = text_var(case, "full");
            md.set_transient_render_cache(true);
            for len in (1..=full.encode_utf16().count()).step_by(3) {
                clear_render_cache();
                md.set_text(WireString::from_units(full.encode_utf16().take(len).collect()));
                md.render(if id == "B-5cd6baf925" { 80 } else { 60 });
            }
            if id == "B-7a289e77ce" {
                md.set_transient_render_cache(false);
            } else if id == "B-57c11dfb9d" {
                md.set_transient_render_cache(false);
                md.set_transient_render_cache(true);
            }
            render_equal(&mut md, full, if id == "B-5cd6baf925" { 40 } else { 60 }, theme, base, false, id);
        }
        "B-e22a72c39a" => {
            md.set_text("hello world".into());
            md.set_transient_render_cache(true);
            clear_render_cache();
            assert_eq!(md.render(60).as_ref(), cold("hello world".into(), 60, theme, base, false).as_ref(), "{id}");
        }
        "B-a57c389ade" => {
            for line in ["hello world", "a b c", "the quick brown fox", "plain text here", "foo", "x y z"] {
                assert!(!markdown_stream::fast_line_start_hazard(line), "{id} {line}");
            }
        }
        "B-8953c00fec" => {
            for line in ["# h", "> q", "1. x", "- a", "+ a", "---", "***", "═══", "───", "==="] {
                assert!(markdown_stream::fast_line_start_hazard(line), "{id} {line}");
            }
        }
        "B-5ed4c45377" => assert!(markdown_stream::fast_line_start_hazard("[label]: https://x"), "{id}"),
        "B-81ce4b1b96" => {
            let prose = text_var(case, "prose");
            md.set_transient_render_cache(true);
            markdown_stream::reset_fast_tail_splices();
            for len in 1..=prose.encode_utf16().count() {
                clear_render_cache();
                md.set_text(WireString::from_units(prose.encode_utf16().take(len).collect()));
                md.render(60);
            }
            assert!(markdown_stream::fast_tail_splices() > 0, "{id} actual row splice");
            markdown_stream::reset_fast_tail_splices();
        }
        "B-de07b8dd9d" => {
            let mixed = text_var(case, "MIXED");
            for len in (1..=mixed.encode_utf16().count()).step_by(41) {
                clear_render_cache();
                md.set_text(WireString::from_units(mixed.encode_utf16().take(len).collect()));
                md.render(80);
            }
            render_equal(&mut md, mixed, 40, theme, base, false, id);
            clear_render_cache();
            assert_eq!(md.render(100).as_ref(), cold(mixed.into(), 100, theme, base, false).as_ref(), "{id} wide");
        }
        "B-4b9be5b151" | "B-c3c14e0b0b" => {
            if id == "B-c3c14e0b0b" {
                md.set_transient_render_cache(true);
            }
            clear_render_cache();
            md.set_text("# First document\n\nOriginal body paragraph one.\n\nOriginal body paragraph two.\n".into());
            md.render(60);
            let next = if id == "B-c3c14e0b0b" {
                "a flat replacement with no double newline at all"
            } else {
                "## Different\n\nCompletely new content replacing the old buffer entirely.\n"
            };
            render_equal(&mut md, next, 60, theme, base, false, id);
        }
        "B-a5f43bee42" => {
            let crlf = text_var(case, "crlf");
            for len in (1..=crlf.encode_utf16().count()).step_by(11) {
                let part = WireString::from_units(crlf.encode_utf16().take(len).collect());
                clear_render_cache();
                md.set_text(part.clone());
                assert_eq!(md.render(60).as_ref(), cold(part, 60, theme, base, false).as_ref(), "{id} frame{len}");
            }
        }
        "B-600594c2a7" => {
            md.set_transient_render_cache(true);
            render_equal(&mut md, text_var(case, "doc"), 60, theme, base, false, id);
            assert!(!md.last_render_stable_text().is_empty(), "supplemental fixed stable boundary actual owner");
        }
        "B-d654fe81a5" => {
            md.set_transient_render_cache(true);
            let doc = text_var(case, "doc");
            for len in 1..=doc.encode_utf16().count() {
                let part = WireString::from_units(doc.encode_utf16().take(len).collect());
                clear_render_cache();
                md.set_text(part.clone());
                assert_eq!(md.render(60).as_ref(), cold(part, 60, theme, base, false).as_ref(), "{id} frame{len}");
                assert!(md.last_render_stable_text().is_empty(), "supplemental whole-list owner cannot freeze");
            }
        }
        "B-23ebb3b936" => {
            let mut grown = text_var(case, "crlfFence").to_owned();
            clear_render_cache();
            md.set_text(grown.clone().into());
            md.render(60);
            md.set_transient_render_cache(true);
            for suffix in [" tail-a", " tail-b", " tail-c"] {
                grown.push_str(suffix);
                render_equal(&mut md, &grown, 60, theme, base, true, id);
            }
        }
        "B-1688bc9a84" => {
            let full = text_var(case, "full");
            md.set_text(full.into());
            let truncated = WireString::from_units(full.encode_utf16().take(12).collect());
            clear_render_cache();
            md.set_text(truncated.clone());
            assert_eq!(
                md.render(60).as_ref(),
                cold(truncated.clone(), 60, theme, base, false).as_ref(),
                "{id} truncation"
            );
            let grown = format!("{}|\x1b\\", String::from_utf16_lossy(truncated.units()));
            render_equal(&mut md, &grown, 60, theme, base, false, id);
        }
        "B-01788d173a" | "B-65c8f05d88" => {
            clear_render_cache();
            let calls = Arc::new(AtomicU8::new(0));
            let count = calls.clone();
            let mut options = base.clone();
            options.highlighter = Some(Arc::new(move |_, _| {
                count.fetch_add(1, Ordering::Relaxed);
                vec!["HIGHLIGHTED".into()]
            }));
            md = make("".into(), theme, &options);
            md.set_text(
                if id == "B-01788d173a" {
                    "```ts\nconst streamed = true;\n"
                } else {
                    "```ts\nconst streamed = true;\n```"
                }
                .into(),
            );
            md.set_transient_render_cache(true);
            let output = plain(&md.render(80).join("\n"));
            if id == "B-01788d173a" {
                assert_eq!(calls.load(Ordering::Relaxed), 0, "{id}");
                assert!(output.contains("const streamed = true;"), "{id}");
                assert!(!output.contains("HIGHLIGHTED"), "{id}");
            } else {
                assert_eq!(calls.load(Ordering::Relaxed), 1, "{id}");
                assert!(output.contains("HIGHLIGHTED"), "{id}");
                md.set_transient_render_cache(false);
                assert!(plain(&md.render(80).join("\n")).contains("HIGHLIGHTED"), "{id}");
            }
        }
        "B-6ff22f0340" => {
            clear_render_cache();
            let pushes = Arc::new(Mutex::new(Vec::new()));
            let receipts = pushes.clone();
            let mut options = base.clone();
            options.highlighter = Some(Arc::new(|code, _| code.split('\n').map(|line| format!("F<{line}>")).collect()));
            options.highlight_stream_factory = Some(Arc::new(move |lang| {
                Ok(if lang == Some("python") {
                    Some(Box::new(TestHighlightStream { pushes: receipts.clone(), consumed: String::new() }))
                } else {
                    None
                })
            }));
            md = make("```python\ndef f():\n    x = 1".into(), theme, &options);
            md.set_transient_render_cache(true);
            let first = plain(&md.render(80).join("\n"));
            assert!(first.contains("S<def f():>"), "{id}");
            assert!(first.contains("    x = 1"), "{id}");
            assert!(!first.contains("S<    x = 1"), "{id}");
            assert_eq!(*pushes.lock().unwrap(), vec!["def f():\n"], "{id}");
            md.set_text("```python\ndef f():\n    x = 1\n    return x".into());
            let second = plain(&md.render(80).join("\n"));
            assert!(second.contains("S<def f():>"), "{id}");
            assert!(second.contains("S<    x = 1>"), "{id}");
            assert!(second.contains("    return x"), "{id}");
            assert_eq!(*pushes.lock().unwrap(), vec!["def f():\n", "    x = 1\n"], "{id}");
            md.set_text("```python\ndef f():\n    x = 1\n    return x\n```".into());
            let closed = plain(&md.render(80).join("\n"));
            assert!(closed.contains("F<def f():>"), "{id}");
            assert_eq!(*pushes.lock().unwrap(), vec!["def f():\n", "    x = 1\n"], "{id}");
        }
        "B-aa66406e0e" | "B-6b88f0d693" => {
            clear_render_cache();
            let mut options = base.clone();
            options.highlighter = Some(Arc::new(|code, _| vec![format!("F<{code}>")]));
            options.highlight_stream_factory = Some(if id == "B-aa66406e0e" {
                Arc::new(|_| Ok(None))
            } else {
                Arc::new(|_| panic!("undefined is not a constructor"))
            });
            md = make(
                if id == "B-aa66406e0e" {
                    "```someunknownlang\nplain text line\nmore"
                } else {
                    "```lua\nlocal x = 1\nmore"
                }
                .into(),
                theme,
                &options,
            );
            md.set_transient_render_cache(true);
            let output = plain(&md.render(80).join("\n"));
            assert!(output.contains(if id == "B-aa66406e0e" { "plain text line" } else { "local x = 1" }), "{id}");
            assert!(!output.contains("S<"), "{id}");
            assert!(!output.contains("F<"), "{id}");
        }
        _ => panic!("untranslated original custom case {id}"),
    }
}
fn run_group(source_file: &str, expected: usize) {
    let _serial = GROUP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fixture: Value = serde_json::from_str(include_str!("markdown_stream_original_cases.json")).unwrap();
    let (theme, options) = base();
    let mut visited = Vec::new();
    let mut failures = Vec::new();
    for case in fixture["originals"].as_array().unwrap().iter().filter(|case| case["file"] == source_file) {
        let id = case["id"].as_str().unwrap();
        visited.push(id.to_owned());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if case["helpers"].as_array().unwrap().is_empty() {
                custom_case(case, &theme, &options)
            } else {
                helper_cases(case, &theme, &options)
            }
        }));
        if let Err(error) = result {
            let message = error
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| error.downcast_ref::<&str>().map(|s| (*s).into()))
                .unwrap_or_else(|| "nonstring panic".into());
            failures.push(json!({"id":id,"failure":message}));
        }
    }
    let receipt = json!({"fixedCommit":fixture["fixedCommit"],"sourceFile":source_file,"originalIDs":visited,"originalCount":visited.len(),"failures":failures,"boundary":"Real Rust streaming original actions; writer1 and full TUI acceptance separate."});
    eprintln!("ARA_TUI_STREAM_ORIGINAL_RECEIPT {receipt}");
    assert_eq!(visited.len(), expected, "full original source group count");
    assert!(failures.is_empty(), "original streaming failures: {failures:?}");
}
#[test]
fn complete_original_bplus36() {
    run_group("packages/tui/test/markdown-bplus-fast-path.test.ts", 36);
}
#[test]
fn complete_original_incremental32() {
    run_group("packages/tui/test/markdown-incremental-lex.test.ts", 32);
}
#[test]
fn complete_original_fence_highlight5() {
    run_group("packages/tui/test/markdown.test.ts", 5);
    // Supplemental Host frame-copy invariant. Rejected scratch rendering must
    // not close the retained native parser's multiline string scope.
    let _serial = GROUP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    use super::super::highlight::{Color, HighlightColors, HighlightStream, highlight_code};
    let c = Color::new("\x1b[32m").unwrap();
    let colors = HighlightColors {
        comment: c.clone(),
        keyword: c.clone(),
        function: c.clone(),
        variable: c.clone(),
        string: c.clone(),
        number: c.clone(),
        r#type: c.clone(),
        operator: c.clone(),
        punctuation: c,
        inserted: None,
        deleted: None,
    };
    let (theme, mut options) = base();
    let palette = colors.clone();
    options.highlighter = Some(Arc::new(move |code, lang| {
        highlight_code(code, lang, palette.clone()).split('\n').map(str::to_owned).collect()
    }));
    options.highlight_stream_factory = Some(Arc::new(move |lang| {
        let native = HighlightStream::new(lang, colors.clone());
        Ok(native.supported().then(|| Box::new(native) as Box<dyn MarkdownHighlightSession>))
    }));
    let mut retained = make("```python\ntext = '''\ninside".into(), &theme, &options);
    retained.set_transient_render_cache(true);
    retained.render(80);
    let mut rejected = retained.frame_copy();
    rejected.set_text("```python\ntext = '''\ninside'''\nnew plain".into());
    rejected.render(80);
    retained.set_text("```python\ntext = '''\ninside\nstill".into());
    let actual = retained.render(80);
    let expected = cold("```python\ntext = '''\ninside\nstill".into(), 80, &theme, &options, true);
    assert_eq!(actual.as_ref(), expected.as_ref(), "supplemental rejected frame keeps native parser state independent");
}
