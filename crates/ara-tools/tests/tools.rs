#[path = "support/bun_image_codec.rs"]
mod bun_image_codec;
use ara_agent::{AgentTool, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::*;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

fn noop() -> UpdateFn {
    Arc::new(|_| {})
}

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain display (no edit tool exposed), as these read/write/bash cases assert.
fn plain(dir: &std::path::Path) -> ToolContext {
    ToolContext::new(dir).with_edit(ara_edit::EditMode::Hashline, false)
}

fn tools(dir: &std::path::Path) -> (read::ReadTool, write::WriteTool, bash::BashTool) {
    let ctx = plain(dir);
    (read::ReadTool { ctx: ctx.clone() }, write::WriteTool::new(ctx.clone()), bash::BashTool { ctx })
}

#[tokio::test]
async fn read_ranges_numbers_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    let (read, _, _) = tools(dir.path());
    std::fs::write(dir.path().join("a.txt"), "one\ntwo\nthree\nfour\nfive\n").unwrap();
    let r = |p: &str| {
        let read = &read;
        let p = p.to_string();
        async move { read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop()).await }
    };
    assert_eq!(text(&r("a.txt").await.unwrap()), "one\ntwo\nthree\nfour\nfive");
    assert_eq!(text(&r("a.txt:2-3").await.unwrap()), "one\ntwo\nthree\nfour\nfive");
    assert_eq!(text(&r("a.txt:-2").await.unwrap()), "three\nfour\nfive");
    assert_eq!(text(&r("a.txt:4+5").await.unwrap()), "three\nfour\nfive");
    assert_eq!(
        text(&r("a.txt:9").await.unwrap()),
        "Line 9 is beyond end of file (5 lines total). Use :1 to read from the start, or :5 to read the last line."
    );
    assert_eq!(r("missing.txt").await.unwrap_err().0, "File not found: missing.txt");

    let mut numbered_ctx = plain(dir.path());
    numbered_ctx.line_numbers = true;
    let numbered = read::ReadTool { ctx: numbered_ctx };
    let out =
        numbered.execute("c", args(json!({"path": "a.txt:2-3"})), CancellationToken::new(), noop()).await.unwrap();
    assert_eq!(text(&out), "1|one\n2|two\n3|three\n4|four\n5|five");
    let raw =
        numbered.execute("c", args(json!({"path": "a.txt:raw:2-2"})), CancellationToken::new(), noop()).await.unwrap();
    assert_eq!(text(&raw), "two", "fixed bounded raw body has no numbering or continuation notice");
    assert_eq!(raw.details.as_ref().unwrap()["truncation"]["truncatedBy"], json!("lines"));
    assert_eq!(raw.details.as_ref().unwrap()["truncation"]["outputLines"], json!(1));

    let big: String = (1..=5000).map(|i| format!("{i}\n")).collect();
    std::fs::write(dir.path().join("big.txt"), big).unwrap();
    // Pin this cap regression's original 3000-line premise independently of
    // fixed READ's default300 (covered by the production default-limit group).
    let capped = read::ReadTool { ctx: plain(dir.path()).with_read_default_limit(3000.0) };
    let out = capped.execute("c", args(json!({"path": "big.txt"})), CancellationToken::new(), noop()).await.unwrap();
    assert!(text(&out).ends_with("\n\n[2000 more lines in file. Use :3001 to continue]"));
    assert_eq!(out.details.unwrap()["truncation"]["truncatedBy"], json!("lines"));
    let wide = "x".repeat(1000) + "\n";
    std::fs::write(dir.path().join("wide.txt"), wide.repeat(100)).unwrap();
    // fixed max(50KiB, limit*512) retains this byte-cap premise at limit100.
    let capped = read::ReadTool { ctx: plain(dir.path()).with_read_default_limit(100.0) };
    let out = capped.execute("c", args(json!({"path": "wide.txt"})), CancellationToken::new(), noop()).await.unwrap();
    assert!(text(&out).len() <= DEFAULT_MAX_BYTES + 100);
    assert_eq!(out.details.unwrap()["truncation"]["truncatedBy"], json!("bytes"));
}

#[tokio::test]
async fn read_explicit_range_context_keeps_requested_bounds_and_raw_exact() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lines.txt"), (1..=12).map(|line| format!("line{line}\n")).collect::<String>())
        .unwrap();
    let mut ctx = plain(dir.path());
    ctx.line_numbers = true;
    let read = read::ReadTool { ctx };
    let r = |path: &str| {
        let read = &read;
        let path = path.to_owned();
        async move { read.execute("c", args(json!({"path":path})), CancellationToken::new(), noop()).await.unwrap() }
    };
    assert_eq!(
        text(&r("lines.txt:4-4").await),
        "3|line3\n4|line4\n5|line5\n6|line6\n7|line7\n\n[5 more lines in file. Use :8 to continue]"
    );
    assert_eq!(
        text(&r("lines.txt:1-1").await),
        "1|line1\n2|line2\n3|line3\n4|line4\n\n[8 more lines in file. Use :5 to continue]"
    );
    assert_eq!(text(&r("lines.txt:10-10").await), "9|line9\n10|line10\n11|line11\n12|line12");
    assert_eq!(text(&r("lines.txt:-2").await), "10|line10\n11|line11\n12|line12");
    assert!(text(&r("lines.txt:4").await).starts_with("3|line3\n4|line4\n5|line5"));
    assert_eq!(
        text(&r("lines.txt:13").await),
        "Line 13 is beyond end of file (12 lines total). Use :1 to read from the start, or :12 to read the last line."
    );
    let raw_result = r("lines.txt:raw:4-4").await;
    let raw = text(&raw_result);
    assert_eq!(raw, "line4", "fixed bounded raw body remains verbatim");
    assert_eq!(raw_result.details.as_ref().unwrap()["truncation"]["truncatedBy"], json!("lines"));
    assert_eq!(raw_result.details.as_ref().unwrap()["truncation"]["outputLines"], json!(1));
    assert!(!raw.contains("line3") && !raw.contains("line5"), "{raw}");

    let hash_ctx = ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Hashline, true);
    let hash_read = read::ReadTool { ctx: hash_ctx };
    let hash_out =
        hash_read.execute("c", args(json!({"path":"lines.txt:4-4"})), CancellationToken::new(), noop()).await.unwrap();
    let hash_text = text(&hash_out);
    assert!(hash_text.starts_with("[lines.txt#") && hash_text.contains("\n3:line3\n4:line4\n5:line5"), "{hash_text}");
    let key = ara_edit::path_policy::canonical_key(&dir.path().join("lines.txt"));
    assert_eq!(
        hash_read.ctx.edit_store.head(&key).unwrap().seen_lines.unwrap().as_ref(),
        &std::collections::BTreeSet::from([3, 4, 5, 6, 7])
    );
}

#[tokio::test]
async fn read_multiple_ranges_keep_exact_lines_and_edit_provenance() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("lines.txt"), (1..=12).map(|n| format!("line{n}\n")).collect::<String>()).unwrap();
    let mut ctx = plain(dir.path());
    ctx.line_numbers = true;
    let read = read::ReadTool { ctx };
    let run = |path: &str| {
        let read = &read;
        let path = path.to_owned();
        async move { read.execute("c", args(json!({"path": path})), CancellationToken::new(), noop()).await.unwrap() }
    };
    assert_eq!(
        text(&run("lines.txt:9-9,3-4,4-5,20-22").await),
        "3|line3\n4|line4\n5|line5\n…\n9|line9\n[Range 20-22 is beyond end of file (12 lines total); skipped]"
    );
    assert_eq!(text(&run("lines.txt:raw:3-3,9-9").await), "line3\n\n…\n\nline9");

    let hash_read = read::ReadTool { ctx: ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Hashline, true) };
    let out = hash_read
        .execute("c", args(json!({"path":"lines.txt:3-4,9-9"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    let body = text(&out);
    assert!(body.starts_with("[lines.txt#") && body.contains("\n3:line3\n4:line4\n…\n9:line9"), "{body}");
    let key = ara_edit::path_policy::canonical_key(&dir.path().join("lines.txt"));
    assert_eq!(
        hash_read.ctx.edit_store.head(&key).unwrap().seen_lines.unwrap().as_ref(),
        &std::collections::BTreeSet::from([3, 4, 9])
    );

    let raw = hash_read
        .execute("c", args(json!({"path":"lines.txt:raw:2-2,8-8"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert_eq!(text(&raw), "line2\n\n…\n\nline8");
    assert_eq!(
        hash_read.ctx.edit_store.head(&key).unwrap().seen_lines.unwrap().as_ref(),
        &std::collections::BTreeSet::from([2, 3, 4, 8, 9])
    );
}

#[tokio::test]
async fn read_disjoint_ranges_include_syntax_boundaries_and_record_visible_anchors() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("blocks.ts"),
        [
            "function first() {",
            "  const a = 1;",
            "  const b = 2;",
            "  return a + b;",
            "}",
            "function second() {",
            "  const c = 3;",
            "  return c;",
            "}",
            "after();",
        ]
        .join("\n"),
    )
    .unwrap();
    let read = read::ReadTool { ctx: ToolContext::new(dir.path()).with_edit(ara_edit::EditMode::Hashline, true) };
    let out =
        read.execute("c", args(json!({"path":"blocks.ts:1-1,6-6"})), CancellationToken::new(), noop()).await.unwrap();
    let body = text(&out);
    assert!(body.contains("\n1:function first() {\n…\n5:}\n6:function second() {\n…\n9:}"), "{body}");
    assert!(!body.contains("2:  const a") && !body.contains("8:  return c"), "{body}");
    let key = ara_edit::path_policy::canonical_key(&dir.path().join("blocks.ts"));
    assert_eq!(
        read.ctx.edit_store.head(&key).unwrap().seen_lines.unwrap().as_ref(),
        &std::collections::BTreeSet::from([1, 5, 6, 9])
    );
}

#[tokio::test]
async fn read_disjoint_ranges_use_python_ast_and_keep_selected_lines_ahead_of_context() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("module.py"),
        [
            "def greet(name):",
            "    a = 1",
            "    b = 2",
            "    c = 3",
            "    d = 4",
            "    e = 5",
            "    f = 6",
            "    g = 7",
            "    return a + b + c + d + e + f + g + len(name)",
            "trailing = 1",
            "other = 2",
        ]
        .join("\n"),
    )
    .unwrap();
    let mut ctx = plain(dir.path());
    ctx.line_numbers = true;
    let read = read::ReadTool { ctx };
    let run = |path: &str| {
        let path = path.to_owned();
        let read = &read;
        async move { read.execute("c", args(json!({"path":path})), CancellationToken::new(), noop()).await.unwrap() }
    };
    let py = text(&run("module.py:1-1,11-11").await);
    assert!(
        py.contains("1|def greet(name):\n…\n9|    return a + b + c + d + e + f + g + len(name)\n…\n11|other = 2"),
        "{py}"
    );
    assert!(!py.contains("8|    g = 7") && !py.contains("10|trailing"), "{py}");

    let large_context = "x".repeat(DEFAULT_MAX_BYTES + 10);
    std::fs::write(
        dir.path().join("budget.ts"),
        format!("function first() {{\n  first();\n}} // {large_context}\nfunction second() {{\n  second();\n}}\n"),
    )
    .unwrap();
    let bounded = text(&run("budget.ts:1-1,4-4").await);
    assert!(bounded.contains("1|function first() {") && bounded.contains("4|function second() {"), "{bounded}");
    assert!(bounded.contains("6|}") && !bounded.contains(&large_context), "{bounded}");
    assert!(bounded.len() <= DEFAULT_MAX_BYTES, "{}", bounded.len());
    assert_eq!(text(&run("budget.ts:raw:1-1,4-4").await), "function first() {\n\n…\n\nfunction second() {");

    let near_limit_context = "x".repeat(DEFAULT_MAX_BYTES - 70);
    std::fs::write(
        dir.path().join("notices.ts"),
        format!("function first() {{\n  first();\n}} // {near_limit_context}\nfunction second() {{\n  second();\n}}\n"),
    )
    .unwrap();
    let with_notice = text(&run("notices.ts:1-1,4-4,100-100").await);
    assert!(
        with_notice.contains("[Range 100-100 is beyond end of file (6 lines total); skipped]"),
        "{}",
        &with_notice[with_notice.len().saturating_sub(200)..]
    );

    std::fs::write(
        dir.path().join("large.ts"),
        format!("function outer() {{\n// {}\n  inner();\n}}\n", "y".repeat(4 * 1024 * 1024)),
    )
    .unwrap();
    let streamed = text(&run("large.ts:1-1,3-3").await);
    assert_eq!(streamed, "1|function outer() {\n…\n3|  inner();");
}

#[tokio::test]
async fn read_multiple_ranges_raw_eof_and_limits() {
    let dir = tempfile::tempdir().unwrap();
    // Test source byte/line budgets independently from display-only column caps.
    let read = read::ReadTool { ctx: plain(dir.path()).with_read_output_max_columns(0.0) };
    std::fs::write(dir.path().join("crlf.txt"), "one\r\ntwo\r\nthree\r\n").unwrap();
    let run = |path: &str| {
        let read = &read;
        let path = path.to_owned();
        async move { read.execute("c", args(json!({"path":path})), CancellationToken::new(), noop()).await.unwrap() }
    };
    assert_eq!(text(&run("crlf.txt:raw:1-1,3-3").await), "one\r\n\n…\n\nthree\r");
    assert_eq!(text(&run("crlf.txt:raw:1-1,4-4").await), "one\r\n\n…\n\n");
    assert_eq!(
        text(&run("crlf.txt:1-1,10-12").await),
        "one\n[Range 10-12 is beyond end of file (3 lines total); skipped]"
    );
    let huge = (1..=5000).map(|n| format!("line{n}\n")).collect::<String>();
    std::fs::write(dir.path().join("huge.txt"), huge).unwrap();
    let out = run("huge.txt:1-3500,4000-4500").await;
    // Fixed read.ts #readLocalFileMultiRange caps each span independently;
    // buffered nonraw spans have no aggregate byte/line truncation receipt.
    let first = (1..=3000).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
    let second = (4000..=4500).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
    assert_eq!(text(&out), format!("{first}\n…\n{second}"));
    assert!(out.details.as_ref().unwrap().get("truncation").is_none());
    std::fs::write(dir.path().join("wide.txt"), format!("{}\nsecond\nthird\n", "x".repeat(DEFAULT_MAX_BYTES + 1)))
        .unwrap();
    let too_wide = run("wide.txt:1-1,3-3").await;
    assert_eq!(text(&too_wide), format!("{}\n…\nthird", "x".repeat(DEFAULT_MAX_BYTES + 1)));
    assert!(too_wide.details.as_ref().unwrap().get("truncation").is_none());
    assert_eq!(text(&run("wide.txt:raw:3-3").await), "third");
    std::fs::write(
        dir.path().join("wide_later.txt"),
        format!("first\nsecond\n{}\nfour\nfifth\n", "x".repeat(DEFAULT_MAX_BYTES + 1)),
    )
    .unwrap();
    let later = run("wide_later.txt:1-1,3-3,5-5").await;
    assert_eq!(text(&later), format!("first\n…\n{}\n…\nfifth", "x".repeat(DEFAULT_MAX_BYTES + 1)));
    assert!(later.details.as_ref().unwrap().get("truncation").is_none());
    std::fs::write(dir.path().join("budget.txt"), format!("{}\nsecond\nthird\n", "x".repeat(DEFAULT_MAX_BYTES - 5)))
        .unwrap();
    let budget = run("budget.txt:1-1,3-3").await;
    assert_eq!(text(&budget), format!("{}\n…\nthird", "x".repeat(DEFAULT_MAX_BYTES - 5)));
    assert!(budget.details.as_ref().unwrap().get("truncation").is_none());
    let many = (100..=8100).step_by(2).map(|n| format!("{n}-{n}")).collect::<Vec<_>>().join(",");
    let notices = run(&format!("crlf.txt:{many}")).await;
    let expected_notices = (100..=8100).step_by(2)
        .map(|n| format!("[Range {n}-{n} is beyond end of file (3 lines total); skipped]"))
        .collect::<Vec<_>>().join("\n");
    assert_eq!(text(&notices), expected_notices);
    assert!(notices.details.as_ref().unwrap().get("truncation").is_none());
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let err =
        read.execute("c", args(json!({"path":"sub:1-1,3-3"})), CancellationToken::new(), noop()).await.unwrap_err();
    assert!(err.0.contains("Multi-range line selectors are not supported for directory listings"), "{err:?}");
}

#[tokio::test]
async fn read_explicit_range_reaches_requested_line_when_leading_context_exhausts_byte_limit() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = plain(dir.path());
    ctx.line_numbers = true;
    let read = read::ReadTool { ctx };
    for (name, lead) in
        [("oversized.txt", "x".repeat(DEFAULT_MAX_BYTES + 1)), ("nearly_full.txt", "x".repeat(DEFAULT_MAX_BYTES - 10))]
    {
        std::fs::write(dir.path().join(name), format!("{lead}\ntarget line\nnext line\n")).unwrap();
        let out = read
            .execute("c", args(json!({"path":format!("{name}:2-2")})), CancellationToken::new(), noop())
            .await
            .unwrap();
        assert!(
            text(&out).starts_with("2|target line\n3|next line"),
            "{name}: {}",
            &text(&out)[..text(&out).len().min(120)]
        );
    }
}

#[tokio::test]
async fn read_directories_binaries_images() {
    let dir = tempfile::tempdir().unwrap();
    let (read, _, _) = tools(dir.path());
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    std::fs::write(dir.path().join("b.bin"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    std::fs::write(dir.path().join("p.png"), [137u8, 80, 78, 71]).unwrap();
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());
    // Adapt this old bespoke flat-list expectation to fixed workspace-tree READ profile.
    // Keep the original invalid 4-byte PNG fixture; genuine image loading now
    // rejects it through the binary text route, rather than attaching it.
    let listing = text(&r(".").await.unwrap());
    let lines: Vec<_> = listing.lines().collect();
    assert_eq!(lines.first().copied(), Some("."));
    assert_eq!(lines.len(), 4);
    for entry in ["  - b.bin", "  - p.png", "  - sub/"] {
        assert_eq!(lines.iter().filter(|line| line.starts_with(entry)).count(), 1, "{listing}");
    }
    assert!(
        text(&r("b.bin").await.unwrap()).starts_with("[Cannot read binary file 'b.bin' (6B); not valid UTF-8 text.")
    );
    let invalid = r("p.png").await.unwrap();
    assert!(text(&invalid).starts_with("[Cannot read binary file 'p.png' (4B); not valid UTF-8 text."));
    assert!(!invalid.content.iter().any(|block| matches!(block, UserBlock::Image(_))));

    // Fixed read-local-image.test.ts tiny PNG; actual Bun decoder, no fake codec.
    use base64::Engine;
    const TINY_PNG: &str =
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    std::fs::write(dir.path().join("p.png"), base64::engine::general_purpose::STANDARD.decode(TINY_PNG).unwrap())
        .unwrap();
    let real_image = read::ReadTool {
        ctx: plain(dir.path()).with_read_auto_resize_images(false)
            .with_read_image_codec(Arc::new(bun_image_codec::NativeBunCodec::new())),
    };
    let img = real_image.execute("c", args(json!({"path":"p.png"})), CancellationToken::new(), noop()).await.unwrap();
    assert!(matches!(&img.content[1], UserBlock::Image(i) if i.mime_type == "image/png" && i.data == TINY_PNG));
}

#[tokio::test]
async fn write_creates_dirs_counts_utf16_and_marks_shebang() {
    // Reenter this existing coarse family in an isolated process: HOME and the
    // Windows drive-relative cwd cannot race with other integration tests.
    if let Some(cwd) = std::env::var_os("ARA_ARCHIVE_HOME_WITNESS_CWD") {
        let cwd = std::path::PathBuf::from(cwd);
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let (_, write, _) = tools(&cwd);
        let before = std::fs::read(cwd.join("home.zip")).unwrap();
        let request = ara_tools::engine::GlobRequest {
            root: &home,
            pattern: "*",
            recursive: true,
            include_hidden: true,
            use_gitignore: false,
            max_results: usize::MAX,
        };
        let budget = ara_tools::engine::Budget {
            cancel: CancellationToken::new(),
            deadline: Instant::now() + Duration::from_secs(3),
        };
        assert!(!ara_tools::engine::glob_with_cache(&request, &budget, true).unwrap().is_empty());
        std::fs::write(home.join("late-scan-sentinel.txt"), b"outside writer").unwrap();
        assert!(
            !ara_tools::engine::glob_with_cache(&request, &budget, true)
                .unwrap()
                .iter()
                .any(|entry| entry.relative == "late-scan-sentinel.txt")
        );
        let out = write
            .execute(
                "home",
                args(json!({"path":"~/home.zip:new","content":"home-only"})),
                CancellationToken::new(),
                noop(),
            )
            .await
            .unwrap();
        assert_eq!(out.details.unwrap()["resolvedPath"], json!(home.join("home.zip").to_string_lossy()));
        assert_eq!(std::fs::read(cwd.join("home.zip")).unwrap(), before);
        assert!(
            ara_tools::engine::glob_with_cache(&request, &budget, true)
                .unwrap()
                .iter()
                .any(|entry| entry.relative == "late-scan-sentinel.txt")
        );
        #[cfg(windows)]
        {
            let original = std::fs::read(cwd.join("drive.asar")).unwrap();
            let rejected = write
                .execute(
                    "drive",
                    args(json!({"path":"drive.asar:new","content":"x"})),
                    CancellationToken::new(),
                    noop(),
                )
                .await
                .unwrap_err();
            assert!(rejected.0.contains("Unpacked ASAR file"), "{}", rejected.0);
            assert_eq!(std::fs::read(cwd.join("drive.asar")).unwrap(), original);
            assert_eq!(std::fs::read(home.join("probe.bin")).unwrap(), b"external");
            assert!(!cwd.join(format!("drive.asar.tmp-{}", std::process::id())).exists());
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_, write, _) = tools(dir.path());
    let w =
        |p: &str, c: &str| write.execute("c", args(json!({"path": p, "content": c})), CancellationToken::new(), noop());
    let out = w("deep/nested/x.txt", "héllo😀").await.unwrap();
    assert_eq!(
        text(&out),
        format!(
            "Successfully wrote 10 bytes to {}",
            std::path::Path::new("deep").join("nested").join("x.txt").display()
        )
    );
    assert_eq!(std::fs::read_to_string(dir.path().join("deep/nested/x.txt")).unwrap(), "héllo😀");
    let out = w("run.sh", "#!/bin/sh\necho hi\n").await.unwrap();
    #[cfg(unix)]
    assert_eq!(text(&out), format!("Successfully wrote 18 bytes to run.sh\n{}", write::EXECUTABLE_NOTICE));
    #[cfg(not(unix))]
    assert_eq!(text(&out), "Successfully wrote 18 bytes to run.sh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.path().join("run.sh")).unwrap().permissions().mode() & 0o111, 0o111);
        assert_eq!(out.details.as_ref().unwrap()["madeExecutable"], json!(true));
        assert!(text(&out).contains("[Notice: Made executable via chmod +x]"));

        let data = w("shebang-data.txt", "no shebang here\n").await.unwrap();
        assert_eq!(std::fs::metadata(dir.path().join("shebang-data.txt")).unwrap().permissions().mode() & 0o111, 0);
        assert!(data.details.as_ref().unwrap().get("madeExecutable").is_none());
        std::fs::write(dir.path().join("preexec.sh"), "#!/bin/sh\nold\n").unwrap();
        std::fs::set_permissions(dir.path().join("preexec.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        let preexec = w("preexec.sh", "#!/usr/bin/env python3\nprint('hi')\n").await.unwrap();
        assert_eq!(std::fs::metadata(dir.path().join("preexec.sh")).unwrap().permissions().mode() & 0o111, 0o111);
        assert!(preexec.details.as_ref().unwrap().get("madeExecutable").is_none());
    }
    let again = w("run.sh", "#!/bin/sh\necho changed\n").await.unwrap();
    assert!(!text(&again).contains("chmod"), "existing file keeps its mode");
    assert!(w("deep", "x").await.unwrap_err().0.contains("is a directory"));

    // Fixed WRITE ordinary-file behavior in the original filesystem family.
    let generated = "// Code generated by sqlc. DO NOT EDIT.\nexport const original = 1;\n";
    std::fs::write(dir.path().join("guard.ts"), generated).unwrap();
    let rejected = w("guard.ts", "replacement").await.unwrap_err();
    assert!(rejected.0.contains("Cannot modify auto-generated file"), "{}", rejected.0);
    assert_eq!(std::fs::read_to_string(dir.path().join("guard.ts")).unwrap(), generated);
    // Fixed auto-generated-guard.test.ts source shapes stay in this existing
    // WRITE filesystem family (MIT, OMP 596f2da). Image-default ID belongs to
    // the presentation family, not a second assertion or inventory row here.
    for (name, body) in [
        ("canonical.ts", "// Code generated by sqlc. DO NOT EDIT.\n\nexport const foo = 1;"),
        ("marker.ts", "// @generated\n\nexport const foo = 1;"),
        ("test.py", "# Generated by buf\n\nvalue = 1"),
        ("query.sql", "-- generated by sqlc\n\nselect 1;"),
        ("block.ts", "/*\n * Code generated by mockery. DO NOT EDIT.\n */\nexport const foo = 1;"),
        ("db.ts", "/**\n * This file was generated by kysely-codegen.\n * Please do not edit it manually.\n */\n\nexport interface Database {}"),
        ("global.ts", generated),
        ("service.ts", "// Code generated by sqlc. DO NOT EDIT.\nexport const foo = 1;"),
        ("cursor-proto.ts", "// @generated by packages/catalog/scripts/proto-parser.ts - DO NOT EDIT\n"),
        ("devin-proto.ts", "// @generated by packages/catalog/scripts/proto-parser.ts - DO NOT EDIT\n"),
    ] {
        std::fs::write(dir.path().join(name), body).unwrap();
        let error = w(name, "must not replace").await.unwrap_err();
        assert!(error.0.contains("Cannot modify auto-generated file"), "{name}: {}", error.0);
        assert_eq!(std::fs::read_to_string(dir.path().join(name)).unwrap(), body, "{name}");
    }
    let normal_body = "// Regular source file\nexport const foo = 1;";
    std::fs::write(dir.path().join("normal.ts"), normal_body).unwrap();
    w("normal.ts", "authored\n").await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("normal.ts")).unwrap(), "authored\n");
    w("does-not-exist.ts", "new source\n").await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("does-not-exist.ts")).unwrap(), "new source\n");
    // New files do not run an overwrite guard; disabling the Host policy is
    // the original setting, not a bypass created by WRITE.
    assert!(!w("fresh.pb.go", "package fresh\n").await.unwrap().is_error);
    let unguarded = write::WriteTool::new(
        plain(dir.path()).with_edit_settings(edit::EditSettings { block_auto_generated: false, ..Default::default() }),
    );
    unguarded
        .execute("unguarded", args(json!({"path":"guard.ts","content":"authored"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("guard.ts")).unwrap(), "authored");
    std::fs::write(dir.path().join("permissive.ts"), generated).unwrap();
    unguarded
        .execute("isolated-policy", args(json!({"path":"permissive.ts","content":"authored"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("permissive.ts")).unwrap(), "authored");

    // Original seven ordinary selector rows; the two archive rows remain
    // owned by the archive implementation and inspect real container bytes.
    let selector_root = dir.path().join("original-selectors");
    std::fs::create_dir_all(selector_root.join("src/components")).unwrap();
    std::fs::write(selector_root.join("src/components/LoraSelector.tsx"), "export const x = 1;\n").unwrap();
    let selector_write = write::WriteTool::new(plain(&selector_root));
    let literal = "src/components/LoraSelector.tsx:1-260:raw";
    let selector_error = selector_write
        .execute("selector-empty", args(json!({"path":literal,"content":""})), CancellationToken::new(), noop())
        .await
        .unwrap_err();
    assert!(selector_error.0.contains("read-tool selector ':1-260:raw'"), "{}", selector_error.0);
    assert!(selector_error.0.contains(&format!("read({{ path: \"{literal}\" }})")), "{}", selector_error.0);
    assert!(!selector_root.join(literal).exists());
    let joined = "a.txt:1-2;b/c.txt:3-4";
    let joined_error = selector_write
        .execute("selector-list", args(json!({"path":joined,"content":"{}"})), CancellationToken::new(), noop())
        .await
        .unwrap_err();
    assert!(joined_error.0.contains("semicolon-joined list of 2 read-tool selectors"));
    assert_eq!(std::fs::read_dir(&selector_root).unwrap().map(|e| e.unwrap().file_name()).collect::<Vec<_>>(), vec![std::ffi::OsString::from("src")]);
    for (name, body) in [("src/empty.txt", ""), ("notes;draft.txt", "hi")] {
        let result = selector_write
            .execute("ordinary-selector-shape", args(json!({"path":name,"content":body})), CancellationToken::new(), noop())
            .await
            .unwrap();
        assert!(!result.is_error);
        assert_eq!(std::fs::read_to_string(selector_root.join(name)).unwrap(), body);
    }
    #[cfg(unix)]
    {
        let created = selector_write
            .execute("intentional-colon", args(json!({"path":literal,"content":"hi"})), CancellationToken::new(), noop())
            .await
            .unwrap();
        assert!(!created.is_error);
        assert_eq!(std::fs::read_to_string(selector_root.join(literal)).unwrap(), "hi");
        for (name, replacement) in [("log:1-5", ""), ("report:1-2;archive:3-4", "new")] {
            std::fs::write(selector_root.join(name), "old").unwrap();
            let result = selector_write
                .execute("existing-colon", args(json!({"path":name,"content":replacement})), CancellationToken::new(), noop())
                .await
                .unwrap();
            assert!(!result.is_error);
            assert_eq!(std::fs::read_to_string(selector_root.join(name)).unwrap(), replacement);
        }
    }

    for (path, content, message) in [
        ("read-misfire.txt:1-2:raw", "", "read-tool selector"),
        ("read-misfire.txt:conflicts", "", "read-tool selector"),
        ("first.txt:1-2;second/path.txt:3-4", "not empty", "semicolon-joined list"),
    ] {
        assert!(w(path, content).await.unwrap_err().0.contains(message));
        assert!(!dir.path().join(path).exists());
    }
    assert!(!dir.path().join("first.txt:1-2;second").exists());
    #[cfg(unix)]
    {
        // Existing literal colon paths win, including a dangling symlink;
        // non-empty content is the fixed single-selector escape hatch.
        let literal = "literal.txt:1-2";
        std::fs::write(dir.path().join(literal), "before").unwrap();
        w(literal, "").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join(literal)).unwrap(), "");
        w("authored.txt:raw", "intentional").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("authored.txt:raw")).unwrap(), "intentional");
        std::os::unix::fs::symlink("dangling-target.txt", dir.path().join("dangling.txt:raw")).unwrap();
        w("dangling.txt:raw", "").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.path().join("dangling-target.txt")).unwrap(), "");

        use std::os::unix::fs::PermissionsExt;
        let path = dir.path().join("existing.sh");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let changed = w("existing.sh", "#!/bin/sh\ntrue\n").await.unwrap();
        assert_eq!(changed.details.as_ref().unwrap()["madeExecutable"], json!(true));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o751);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o740)).unwrap();
        assert!(text(&w("existing.sh", "#!/bin/sh\ntrue\n").await.unwrap()).contains(write::EXECUTABLE_NOTICE));
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777, 0o751);
    }
    for (path, suggestion) in [("xd/echo", "xd://echo"), ("dx:/echo", "xd://echo"), ("skill:/book", "skill://book")] {
        let rejected = w(path, "payload").await.unwrap_err();
        assert!(rejected.0.contains("Unknown URI-like write target") && rejected.0.contains(suggestion));
    }
    w("./xd/literal.txt", "ordinary").await.unwrap();
    assert_eq!(std::fs::read_to_string(dir.path().join("xd/literal.txt")).unwrap(), "ordinary");
    let absolute = dir.path().join("absolute.txt");
    w(&absolute.to_string_lossy(), "absolute").await.unwrap();
    assert_eq!(std::fs::read_to_string(absolute).unwrap(), "absolute");

    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    let update: UpdateFn = Arc::new(move |partial| captured.lock().unwrap().push(partial));
    let error = write
        .execute("pre-cancel", args(json!({"path":"cancelled/a.txt","content":"never"})), cancelled, update)
        .await
        .unwrap_err();
    assert!(error.0.contains("Cancelled"));
    assert!(!dir.path().join("cancelled").exists());
    assert!(updates.lock().unwrap().is_empty());

    let updates = Arc::new(Mutex::new(Vec::new()));
    let captured = updates.clone();
    let target = dir.path().join("progress.txt");
    let before_effect = target.clone();
    let update: UpdateFn = Arc::new(move |partial| {
        assert!(!before_effect.exists(), "Writing receipt precedes the filesystem write");
        captured.lock().unwrap().push(partial);
    });
    write
        .execute("progress", args(json!({"path":"progress.txt","content":"hé😀"})), CancellationToken::new(), update)
        .await
        .unwrap();
    let updates = updates.lock().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(text(&updates[0]), "Writing 4 bytes to progress.txt...");
    assert_eq!(updates[0].details.as_ref().unwrap()["resolvedPath"], json!(target.to_string_lossy()));
    assert_eq!(std::fs::read_to_string(target).unwrap(), "hé😀");
    drop(updates);

    // B-eda095646a: fixed original progress payload, result and whole
    // snapshot, alongside the existing UTF16/before-effect observation.
    let progress_root = dir.path().join("original-acp-progress");
    let progress_target = progress_root.join("progress.txt");
    let progress_write = write::WriteTool::new(ToolContext::new(&progress_root));
    let progress_updates = Arc::new(Mutex::new(Vec::new()));
    let progress_captured = progress_updates.clone();
    let progress_update: UpdateFn = Arc::new(move |partial| progress_captured.lock().unwrap().push(partial));
    let progress_content = "bridge write content\n";
    let progress_result = progress_write
        .execute("call-progress", args(json!({"path":progress_target,"content":progress_content})), CancellationToken::new(), progress_update)
        .await
        .unwrap();
    let progress_updates = progress_updates.lock().unwrap();
    assert_eq!(progress_updates.len(), 1);
    let progress_count = progress_content.encode_utf16().count();
    assert_eq!(serde_json::to_value(&progress_updates[0].content).unwrap(), json!([{"type":"text","text":format!("Writing {progress_count} bytes to progress.txt...")}]));
    assert_eq!(progress_updates[0].details.as_ref().unwrap(), &json!({"resolvedPath":progress_target}));
    assert!(text(&progress_result).contains(&format!("Successfully wrote {progress_count} bytes to progress.txt")));
    drop(progress_updates);

    let cancelled = CancellationToken::new();
    let cancel_on_update = cancelled.clone();
    let update: UpdateFn = Arc::new(move |_| cancel_on_update.cancel());
    assert!(
        write
            .execute(
                "cancel-on-progress",
                args(json!({"path":"callback-cancel/a.txt","content":"never"})),
                cancelled,
                update
            )
            .await
            .unwrap_err()
            .0
            .contains("Cancelled")
    );
    assert!(!dir.path().join("callback-cancel").exists());

    // A direct callback that throws cannot have admitted mkdir/write. The
    // separate task catches its Rust unwind; this is not a Native-ack claim.
    let throwing = write::WriteTool::new(plain(dir.path()));
    let target = dir.path().join("callback-throw");
    let joined = tokio::spawn(async move {
        throwing
            .execute(
                "callback-throw",
                args(json!({"path":"callback-throw/a.txt","content":"never"})),
                CancellationToken::new(),
                Arc::new(|_| panic!("direct WRITE callback rejected")),
            )
            .await
    })
    .await;
    assert!(joined.unwrap_err().is_panic());
    assert!(!target.exists());

    // One existing WRITE group also exercises the Host acknowledgement port.
    // The native worker may proceed only after this reply, and every refusal,
    // channel close or callback-triggered cancellation is known pre-effect.
    struct AwaitedProgress {
        target: std::path::PathBuf,
        observed: Mutex<Option<ToolOutput>>,
        entered: tokio::sync::Notify,
        reply: Mutex<Option<tokio::sync::oneshot::Receiver<Result<(), ara_agent::ToolError>>>>,
    }
    #[async_trait::async_trait]
    impl write::WriteProgress for AwaitedProgress {
        async fn acknowledge(&self, output: ToolOutput) -> Result<(), ara_agent::ToolError> {
            assert!(!self.target.parent().unwrap().exists(), "ack request precedes mkdir");
            assert!(!self.target.exists(), "ack request precedes write");
            *self.observed.lock().unwrap() = Some(output);
            let reply = self.reply.lock().unwrap().take().unwrap();
            self.entered.notify_one();
            reply.await.unwrap_or_else(|_| Err(ara_agent::ToolError("ack closed".into())))
        }
    }
    for mode in ["success", "refusal", "closed", "cancel-plus-success"] {
        let relative = format!("awaited-{mode}/file.txt");
        let target = dir.path().join(format!("awaited-{mode}")).join("file.txt");
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let progress = Arc::new(AwaitedProgress {
            target: target.clone(),
            observed: Mutex::new(None),
            entered: tokio::sync::Notify::new(),
            reply: Mutex::new(Some(receiver)),
        });
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let tool = write::WriteTool::new(plain(dir.path()));
        let progress_worker = progress.clone();
        let worker = tokio::spawn(async move {
            tool.execute_os_values_with_progress(&relative, "hé😀", cancel, progress_worker).await
        });
        tokio::time::timeout(Duration::from_secs(3), progress.entered.notified()).await.unwrap();
        assert!(!target.parent().unwrap().exists());
        let update = progress.observed.lock().unwrap().clone().unwrap();
        assert_eq!(
            text(&update),
            format!(
                "Writing 4 bytes to {}...",
                std::path::Path::new(&format!("awaited-{mode}")).join("file.txt").display()
            )
        );
        assert_eq!(update.details.as_ref().unwrap()["resolvedPath"], json!(target.to_string_lossy()));
        match mode {
            "refusal" => sender.send(Err(ara_agent::ToolError("original callback refused".into()))).unwrap(),
            "closed" => drop(sender),
            "cancel-plus-success" => {
                stop.cancel();
                sender.send(Ok(())).unwrap();
            }
            _ => sender.send(Ok(())).unwrap(),
        }
        let result = tokio::time::timeout(Duration::from_secs(3), worker).await.unwrap().unwrap();
        if mode == "success" {
            assert!(!result.unwrap().is_error);
            assert_eq!(std::fs::read_to_string(&target).unwrap(), "hé😀");
        } else {
            assert!(matches!(result, Err(write::WriteExecutionError::NotDispatched(_))));
            assert!(!target.parent().unwrap().exists());
            assert!(!target.exists());
        }
    }

    // Complete fixed archive WRITE observations stay inside the existing
    // filesystem family; there is no additional test entry or acceptance ID.
    // B-f5136d8199 and B-f89356c6e4 retain their original selector contracts.
    use base64::Engine as _;
    use std::collections::HashMap;
    use std::io::{Read as _, Write as _};
    #[derive(Clone)]
    struct ZipItem {
        name: Vec<u8>,
        data: Vec<u8>,
        payload: Vec<u8>,
        method: u16,
        flags: u16,
        mode: u32,
        extra: Vec<u8>,
    }
    fn item(name: &str, data: &[u8]) -> ZipItem {
        ZipItem {
            name: name.as_bytes().to_vec(),
            data: data.to_vec(),
            payload: data.to_vec(),
            method: 0,
            flags: 0x800,
            mode: 0o100644,
            extra: Vec::new(),
        }
    }
    fn put16(b: &mut [u8], o: usize, n: u16) {
        b[o..o + 2].copy_from_slice(&n.to_le_bytes());
    }
    fn put32(b: &mut [u8], o: usize, n: u32) {
        b[o..o + 4].copy_from_slice(&n.to_le_bytes());
    }
    fn get16(b: &[u8], o: usize) -> usize {
        u16::from_le_bytes(b[o..o + 2].try_into().unwrap()) as usize
    }
    fn get32(b: &[u8], o: usize) -> usize {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as usize
    }
    fn zip_fixture(items: &[ZipItem]) -> Vec<u8> {
        let mut local = Vec::new();
        let mut central = Vec::new();
        for item in items {
            let offset = local.len();
            let crc = crc32fast::hash(&item.data);
            let mut h = vec![0; 30];
            put32(&mut h, 0, 0x04034b50);
            put16(&mut h, 4, 20);
            put16(&mut h, 6, item.flags);
            put16(&mut h, 8, item.method);
            put32(&mut h, 14, crc);
            put32(&mut h, 18, item.payload.len() as u32);
            put32(&mut h, 22, item.data.len() as u32);
            put16(&mut h, 26, item.name.len() as u16);
            local.extend(h);
            local.extend(&item.name);
            local.extend(&item.payload);
            let mut h = vec![0; 46];
            put32(&mut h, 0, 0x02014b50);
            put16(&mut h, 4, (3 << 8) | 20);
            put16(&mut h, 6, 20);
            put16(&mut h, 8, item.flags);
            put16(&mut h, 10, item.method);
            put32(&mut h, 16, crc);
            put32(&mut h, 20, item.payload.len() as u32);
            put32(&mut h, 24, item.data.len() as u32);
            put16(&mut h, 28, item.name.len() as u16);
            put16(&mut h, 30, item.extra.len() as u16);
            put32(&mut h, 38, item.mode << 16);
            put32(&mut h, 42, offset as u32);
            central.extend(h);
            central.extend(&item.name);
            central.extend(&item.extra);
        }
        let offset = local.len();
        let size = central.len();
        local.extend(central);
        let mut h = vec![0; 22];
        put32(&mut h, 0, 0x06054b50);
        put16(&mut h, 8, items.len() as u16);
        put16(&mut h, 10, items.len() as u16);
        put32(&mut h, 12, size as u32);
        put32(&mut h, 16, offset as u32);
        local.extend(h);
        local
    }
    fn zip_files(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut offset = 0;
        let mut files = Vec::new();
        while get32(bytes, offset) == 0x04034b50 {
            let method = get16(bytes, offset + 8);
            let compressed = get32(bytes, offset + 18);
            let size = get32(bytes, offset + 22);
            let name_len = get16(bytes, offset + 26);
            let name = String::from_utf8(bytes[offset + 30..offset + 30 + name_len].to_vec()).unwrap();
            let start = offset + 30 + name_len + get16(bytes, offset + 28);
            let payload = &bytes[start..start + compressed];
            let mut data = Vec::new();
            if method == 0 {
                data.extend(payload);
            } else {
                assert_eq!(method, 8);
                flate2::read::DeflateDecoder::new(payload).read_to_end(&mut data).unwrap();
            }
            assert_eq!(data.len(), size);
            assert_eq!(crc32fast::hash(&data), get32(bytes, offset + 14) as u32);
            files.push((name, data));
            offset = start + compressed;
        }
        files
    }
    fn tar_record(name: &str, data: &[u8], kind: u8, target: &str) -> Vec<u8> {
        let mut h = vec![0; 512];
        assert!(name.len() <= 100 && target.len() <= 100);
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[157..157 + target.len()].copy_from_slice(target.as_bytes());
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        for (offset, len, n) in [(100, 8, 0o644), (108, 8, 0), (116, 8, 0), (124, 12, data.len()), (136, 12, 0)] {
            let digits = format!("{n:0width$o}", width = len - 1);
            h[offset..offset + len - 1].copy_from_slice(digits.as_bytes());
        }
        h[148..156].fill(32);
        let sum: usize = h.iter().map(|&b| b as usize).sum();
        h[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
        h[154] = 0;
        h[155] = 32;
        h.extend(data);
        h.resize(h.len() + (512 - data.len() % 512) % 512, 0);
        h
    }
    fn tar_files(bytes: &[u8]) -> HashMap<String, Vec<u8>> {
        let mut out = HashMap::new();
        let mut offset = 0;
        let mut pax = None;
        while bytes[offset..offset + 512].iter().any(|&b| b != 0) {
            let h = &bytes[offset..offset + 512];
            let s = |b: &[u8]| String::from_utf8(b.iter().copied().take_while(|&b| b != 0).collect()).unwrap();
            let mut name = s(&h[..100]);
            let prefix = s(&h[345..500]);
            if !prefix.is_empty() {
                name = format!("{prefix}/{name}");
            }
            let size = usize::from_str_radix(s(&h[124..136]).trim(), 8).unwrap();
            let data = &bytes[offset + 512..offset + 512 + size];
            if h[156] == b'x' {
                let record = String::from_utf8(data.to_vec()).unwrap();
                pax = record.split_once(" path=").map(|(_, name)| name.trim_end_matches('\n').to_owned());
            } else {
                if let Some(path) = pax.take() {
                    name = path;
                }
                if h[156] != b'5' {
                    out.insert(name, data.to_vec());
                }
            }
            offset += 512 + size.div_ceil(512) * 512;
        }
        out
    }
    fn asar_fixture(json: &str, payload: &[u8]) -> Vec<u8> {
        let padded = json.len().div_ceil(4) * 4;
        let mut bytes = vec![0; 16 + padded];
        put32(&mut bytes, 0, 4);
        put32(&mut bytes, 4, (8 + padded) as u32);
        put32(&mut bytes, 8, (4 + padded) as u32);
        put32(&mut bytes, 12, json.len() as u32);
        bytes[16..16 + json.len()].copy_from_slice(json.as_bytes());
        bytes.extend(payload);
        bytes
    }
    fn asar_files(bytes: &[u8]) -> HashMap<ara_rpc::WireString, Vec<u8>> {
        use ara_rpc::{WireString, WireValue};
        let start = 8 + get32(bytes, 4);
        let header = WireValue::parse(std::str::from_utf8(&bytes[16..16 + get32(bytes, 12)]).unwrap()).unwrap();
        let mut work = vec![(WireString::from(""), header.get("files").unwrap())];
        let mut out = HashMap::new();
        while let Some((parent, files)) = work.pop() {
            for (name, node) in files.entries().unwrap() {
                let mut path = parent.units().to_vec();
                if !path.is_empty() {
                    path.push(47);
                }
                path.extend_from_slice(name.units());
                let path = WireString::from_units(path);
                if let Some(files) = node.get("files") {
                    work.push((path, files));
                } else {
                    let size = node.get("size").unwrap().as_number().unwrap() as usize;
                    let offset =
                        node.get("offset").unwrap().as_string().unwrap().to_utf8().unwrap().parse::<usize>().unwrap();
                    out.insert(path, bytes[start + offset..start + offset + size].to_vec());
                }
            }
        }
        out
    }
    fn unpack(extension: &str, bytes: &[u8]) -> HashMap<String, Vec<u8>> {
        match extension {
            "zip" => zip_files(bytes).into_iter().collect(),
            "tar" => tar_files(bytes),
            "tgz" => {
                let mut tar = Vec::new();
                flate2::read::MultiGzDecoder::new(bytes).read_to_end(&mut tar).unwrap();
                tar_files(&tar)
            }
            "tzst" => tar_files(&zstd::stream::decode_all(bytes).unwrap()),
            "asar" => asar_files(bytes).into_iter().map(|(name, data)| (name.to_utf8().unwrap(), data)).collect(),
            _ => unreachable!(),
        }
    }
    let binary = b"\0\xffkept\r\n";
    for extension in ["zip", "tar", "tgz", "tzst", "asar"] {
        let target = format!("archives/fresh.{extension}");
        let out = w(&format!("{target}:./nest\\a.txt"), "hé😀").await.unwrap();
        assert_eq!(
            text(&out),
            format!(
                "Successfully wrote 4 bytes to {}:nest/a.txt",
                std::path::Path::new("archives").join(format!("fresh.{extension}")).display()
            )
        );
        assert_eq!(out.details.unwrap()["resolvedPath"], json!(dir.path().join("archives").join(format!("fresh.{extension}")).to_string_lossy()));
        let fixture = match extension {
            "zip" => zip_fixture(&[item("old.bin", binary)]),
            "asar" => asar_fixture("{\"files\":{\"old.bin\":{\"size\":8,\"offset\":\"0\"}}}", binary),
            _ => {
                let mut tar = tar_record("old.bin", binary, b'0', "");
                tar.resize(tar.len() + 1024, 0);
                if extension == "tgz" {
                    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                    encoder.write_all(&tar).unwrap();
                    encoder.finish().unwrap()
                } else if extension == "tzst" {
                    zstd::stream::encode_all(tar.as_slice(), 0).unwrap()
                } else {
                    tar
                }
            }
        };
        std::fs::write(dir.path().join(&target), fixture).unwrap();
        w(&format!("{target}:new.txt"), "authored").await.unwrap();
        let members = unpack(extension, &std::fs::read(dir.path().join(&target)).unwrap());
        assert_eq!(members["old.bin"], binary);
        assert_eq!(members["new.txt"], b"authored");
        w(&format!("{target}:old.bin"), "replace").await.unwrap();
        let members = unpack(extension, &std::fs::read(dir.path().join(&target)).unwrap());
        assert_eq!(members["old.bin"], b"replace");
        assert_eq!(members["new.txt"], b"authored");
    }
    let mut zip64_member = item("old", binary);
    let mut extra = vec![0; 28];
    put16(&mut extra, 0, 1);
    put16(&mut extra, 2, 24);
    put32(&mut extra, 4, binary.len() as u32);
    put32(&mut extra, 12, binary.len() as u32);
    zip64_member.extra = extra;
    let mut bytes = zip_fixture(&[zip64_member]);
    let old_end = bytes.len() - 22;
    let central = get32(&bytes, old_end + 16);
    let central_size = get32(&bytes, old_end + 12);
    for field in [20, 24, 42] {
        put32(&mut bytes, central + field, u32::MAX);
    }
    let mut records = vec![0; 76];
    put32(&mut records, 0, 0x06064b50);
    put32(&mut records, 4, 44);
    put16(&mut records, 12, 45);
    put16(&mut records, 14, 45);
    put32(&mut records, 24, 1);
    put32(&mut records, 32, 1);
    put32(&mut records, 40, central_size as u32);
    put32(&mut records, 48, central as u32);
    put32(&mut records, 56, 0x07064b50);
    put32(&mut records, 64, old_end as u32);
    put32(&mut records, 72, 1);
    bytes.splice(old_end..old_end, records);
    std::fs::write(dir.path().join("zip64.zip"), bytes).unwrap();
    w("zip64.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("zip64.zip")).unwrap())[0].1, binary);
    {
        let items = (0..65534).map(|n| item(&format!("f{n:05}"), b"")).collect::<Vec<_>>();
        std::fs::write(dir.path().join("count.zip"), zip_fixture(&items)).unwrap();
        drop(items);
        w("count.zip:last", "x").await.unwrap();
        let output = std::fs::read(dir.path().join("count.zip")).unwrap();
        assert_eq!(get16(&output, output.len() - 14), 65535);
        assert_eq!(get32(&output, output.len() - 98), 0x06064b50);
        assert_eq!(get32(&output, output.len() - 98 + 32), 65535);
    }
    // B-f5136d8199/B-f89356c6e4: original archive, literal member and
    // selector suffix; observe full container and member bytes.
    let selector = dir.path().join("bundle.zip");
    let bytes = zip_fixture(&[item("src/foo.ts", b"export const x = 1;\n")]);
    std::fs::write(&selector, &bytes).unwrap();
    let selector_target = "bundle.zip:src/foo.ts:1-20:raw";
    let selector_error = w(selector_target, "").await.unwrap_err();
    assert!(selector_error.0.contains("read-tool selector ':1-20:raw'"), "{}", selector_error.0);
    assert!(selector_error.0.contains(&format!("read({{ path: \"{selector_target}\" }})")), "{}", selector_error.0);
    let unchanged = std::fs::read(&selector).unwrap();
    assert_eq!(unchanged, bytes);
    let unchanged_members = zip_files(&unchanged);
    assert!(unchanged_members.iter().any(|(name, _)| name == "src/foo.ts"));
    assert!(!unchanged_members.iter().any(|(name, _)| name == "src/foo.ts:1-20:raw"));
    assert!(!dir.path().join(format!("bundle.zip.tmp-{}", std::process::id())).exists());
    std::fs::write(&selector, zip_fixture(&[item("src/foo.ts:1-20:raw", b"old")])).unwrap();
    let selector_result = w(selector_target, "").await.unwrap();
    assert!(!selector_result.is_error);
    assert_eq!(zip_files(&std::fs::read(&selector).unwrap()), vec![("src/foo.ts:1-20:raw".into(), Vec::<u8>::new())]);
    for (member, message) in [
        ("", "inside the archive"),
        ("dir/", "not a directory"),
        ("a/../b", "cannot contain '..'"),
        ("././", "not a directory"),
    ] {
        assert!(w(&format!("no.zip:{member}"), "x").await.unwrap_err().0.contains(message));
        assert!(!dir.path().join("no.zip").exists());
    }
    for (extension, format) in [
        ("RAR", "rar"),
        ("cbr", "rar"),
        ("7z", "7z"),
        ("tar.bz2", "tar.bz2"),
        ("tar.xz", "tar.xz"),
        ("tar.Z", "tar.Z"),
        ("gz", "gz"),
        ("bz2", "bz2"),
        ("xz", "xz"),
        ("zst", "zst"),
        ("Z", "Z"),
        ("lzma", "lzma"),
        ("iso", "iso"),
        ("cab", "cab"),
        ("cpio", "cpio"),
        ("rpm", "rpm"),
        ("ar", "ar"),
        ("a", "ar"),
        ("lib", "ar"),
        ("deb", "deb"),
        ("lzh", "lzh"),
        ("lha", "lzh"),
        ("arj", "arj"),
    ] {
        assert_eq!(
            w(&format!("readonly/new.{extension}:file"), "x").await.unwrap_err().0,
            format!("Writing entries inside {format} archives is not supported (read-only format).")
        );
        assert!(!dir.path().join("readonly").exists());
    }
    for extension in ["jar", "war", "ear", "apk", "whl", "ipa", "xpi", "vsix", "nupkg", "cbz"] {
        let name = format!("alias.{extension}");
        w(&format!("{name}:member"), "alias").await.unwrap();
        assert_eq!(
            zip_files(&std::fs::read(dir.path().join(name)).unwrap()),
            vec![("member".into(), b"alias".to_vec())]
        );
    }
    let method_data=base64::engine::general_purpose::STANDARD.decode("Y29kZWMA/3BheWxvYWQtQVJBLTU5NmYyZGFjb2RlYwD/cGF5bG9hZC1BUkEtNTk2ZjJkYWNvZGVjAP9wYXlsb2FkLUFSQS01OTZmMmRhY29kZWMA/3BheWxvYWQtQVJBLTU5NmYyZGE=").unwrap();
    // Python stdlib bz2/lzma produced these protocol fixtures independently;
    // methods20/93 use the native zstd writer. These are member payloads.
    for method in [0, 8, 12, 14, 20, 93, 95] {
        let payload=match method{0=>method_data.clone(),8=>{let mut encoder=flate2::write::DeflateEncoder::new(Vec::new(),flate2::Compression::default());encoder.write_all(&method_data).unwrap();encoder.finish().unwrap()},12=>base64::engine::general_purpose::STANDARD.decode("QlpoOTFBWSZTWTSLdQkAAAVfgMAAAAICACAAEAAuBMAgAACgACKabTSPQhAAAIkFnVLyHbusOfF3JFOFCQNIt1CQQlpoOTFBWSZTWWqAfGcAAAvfgMAAAAITICAAEAAvBMAgAACgAFRXqNMI0aZBFUMnoI9EXYcPgVbhZzlowwEumWB9CjVkNu3QoMKHxdyRThQkGqAfGcA=").unwrap(),14=>base64::engine::general_purpose::STANDARD.decode("CQQFAF0AAIAAADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/z9kQP///B/wAA==").unwrap(),20|93=>zstd::stream::encode_all(method_data.as_slice(),0).unwrap(),95=>base64::engine::general_purpose::STANDARD.decode("/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla").unwrap(),_=>unreachable!()};
        let name = format!("method-{method}.zip");
        let mut fixture = item("old.bin", &method_data);
        fixture.method = method;
        fixture.payload = payload;
        let bytes = zip_fixture(&[fixture.clone()]);
        std::fs::write(dir.path().join(&name), &bytes).unwrap();
        w(&format!("{name}:new"), "yes").await.unwrap();
        assert_eq!(
            zip_files(&std::fs::read(dir.path().join(&name)).unwrap())[0],
            ("old.bin".into(), method_data.clone())
        );
        if method == 14 {
            fixture.payload[2] = 4;
        } else if method == 8 {
            fixture.payload[0] |= 6;
        } else {
            fixture.payload[0] ^= 0x80;
        }
        let failed = zip_fixture(&[fixture]);
        std::fs::write(dir.path().join(&name), &failed).unwrap();
        assert!(w(&format!("{name}:new"), "never").await.is_err(), "corrupt codec {method}");
        assert_eq!(std::fs::read(dir.path().join(&name)).unwrap(), failed);
        assert!(!dir.path().join(format!("{name}.tmp-{}", std::process::id())).exists());
    }
    // The same raw stream needs an exact distance4098. Declaring4097 must fail
    // even though a decoder allocator could round its storage upward to4112.
    let mut seed = 1044u32;
    let mut history = Vec::new();
    for _ in 0..4098 {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        history.push(seed as u8);
    }
    let expected = [history.as_slice(), history.as_slice()].concat();
    let raw=base64::engine::general_purpose::STANDARD.decode("AHqExzG/vVpubjFgIJeE1Y5jou2jSp/k6d8Rui4hheu1bjbA9VnglA+LHOdD4mREdwdNg5IH7iM4XhxCeODez0JinF0ucPd51SEqqO7fCnJiTqqhJy/myNvQHt0Y7P5cVrkt4mm661RYayZDoCKlLuRHdOBjshyxS8bBBu8P6ZMD4IB0dhQf5vzs8x2tdZRLW6I6A+7QCLyP20bqsBmk6SPRzgfNajgmizVqOOIp2ZD2TSplrNxZhRVK8NxXZUF1Ll8QCP0jlm1CRh08aCm7ZD4QuSBbaeVVJ7ixxuPS6hUKwpgezmN04DDzFY5cYqKXDrba0PT2I8AThmpSVxbP51Mt9B1FAe9WVYzpg8+VLYRtAZZEycsBjgZAMI2/G2NIQARslYbdmGELiDdGiS+JVS9Z+p8hBFH24jHM5HqK/X3e8ahnY8s0V4m/mshlgaqP5Lvf5rWuNki9DinvZNWvL9d59XVr+yBMah+a481svhvj064QuKhIZov7Uam0019y4+TBCibJ1iYpnh4C2Vn4Fj7zSqUvI/iYpFCcvUMRZkGXe3Puhli31rn3gzj4jT5Mfjk1/6SLs1B/VFobOd8hxlwuYIGxpY9hj6LmIlibbRmIeA+NHKQm6PH/QbVe0PAaoRONqMX7ih7w+fXj2VHdjDiyWcvCd5JlOJwH/718qnLxF/Y/zvmw/woRqB8rne8cmSWDB3Q5/nGths6265n4ODV7xBPEmT4eS12OsySJQ2YVu8uhoaLJJAj6dTGWXojJCogKdHjDRkLyWlYcpepDYXpB3+RBf/J2zrZPNdgx2rH78l9rJXeVd+8RxFozgA25FgqYCKxpGm3sSmRuyx01PUmsZ6rwlIe0yNjsjUSkyZkPtxS61xeQ9pTSLzoUxD9BpnE9pvYuqdbbxlbGTQiMQblr/XvmeXblCk+cZ9U6J+Q6KQrOv0hSr6p6WeHuhmfxJFGRpSiS6FvN24GaqiOs+7ogo0eygnCfZ1g8MFk6QsurXBMVg0GlkhgLELdHjBYlKNpftQ1sIF1dMIyjaQs3c4n5ZkJ+MBCjGP7GE+NPG/wXEZ4+XcVAkpqy3IXLE113Leus/GKQqzIXyOoX92HvkJnxZYfFUtkUWFSVIei1mv8y5OuuhFugHTvQtZG09zIhrvepdacfzkcaAUjMmMvBNmaay/S8YFQcvXD417BPYUe5pNmvV66euC74Kc6LNKJs0jYVkD6X74jTPwwr4KYOc65QoBM1fNtj8LYhqKuF94BdDegyD2kd+/7eHvaj0e09GWcDQdIXw+9c2bVDOM5GP2YeSc5HWhI0lbCnVNRXDSRJBpcxNHc0v+/5RJd6PVUjIYg8h94EVxcSQNgLAZHA2TTHcG2H9mmhYYrpSMXLxciWqvkmF3YEWH601sJbU9joT9cl4M6cRMYE5tf1ZqMMUyQbV/t+6SWcPCwoHAVZDq3zaqZ1SX/jpeWCI8S7IgfQYg8WgD25ibrT98toY614OBTZvYlTSVzamDidLEh0SLPtrVthM15NgigEAPRETFjDvNwtVBpgOBINeauqid6bR7KJQ8Cct/DrJJNy5uYe8CeUNB171D5PnszJe3JaOFP1LkSTTWPC0v2w3xpYm1sE9NsVtbTkOqAspDimAfdzMOSJwS7xk4fXy8bshfdK3AxvIiZUtqGo1PUeSp6htSKWWbRaHf1vzpNva5ff5AHwKHzmfduf3AyCQOKE991d9DyXdkOcY3ReX9be0RF/3KhWKiQ0E8+x7WSbLDQumAabZfreX9akVQA6xuyz65CUu2VWfY5pV9jvRL032TYaou/jtJsNZqqzyCFbCTJvADKC5pefm0fOPntJQl974pmo0zRqStDyYvzN7z+tFMabUsa9Z3C6/h8vIfQFi02kCh6nwLwDLqCbyGSmuOJ/XhD+XD+jExzoczfbII2ApsQPOFpRKjeJC/UUrJ7CBCmg+JSTG+/Ms3lhj3iUmf2N5b/pH4FdlfgcOoUe+QiIiYPk7JabVQii/Sziv7o5tD6F5QYZlCJNt86ROsO7efgEIWBU7ZPGi+At03ZdF1hXY6RuCFMU2mbgI2ihozQe7NJ/JQCeonP9E2YzK6+Bi0idMB9+TXPZhFQpu3i7z/GBdzYgv5oo7do5zm71qTfYMsJYBR9k4Grrad0wrgGpzdYmg6oUH9E3UXCCrbC0GV5GV1XdiPGT2Y6a8lET//jaba8SuDt1/GUtC67d9qpZibyeZV6gPxTTGxtPof8TqgJMMUgD03IsWdzs3TGI1TSqsLUceW/3IcSFVKq6aH1YOrd03CG81xHnJMNm4HiGkX7B92755LPmAlHEqcWNBvw35QYzq8LsnENuBVMqxF5G4/10Smgu4yjNauF7fW0g81sXMeUPX7gn4EIZTIRKz2cZL/xVDm4HawtCCTrVEimHP9b+Q6CTUfMajpwVkVmiB++uSf9IDSowWThF69FCXXPWz82GVIM4u3gjzjwnUO1ue5VGAz7ZHKF4uAWkHvrfAIvSPwK9Hv0Ul9MzgmDdG7FrvBwlhfLU99KiY/jO0SCFCnzX5OZa+xE2/43jSa7OtHy0JXQhDMXiVzKtUBR/MA0eNFJ/1IMJu/s4Hu47pcLQCCbKeadQzCiL2+K/LyOjDSn2kAeDbauB8X9526myonNDRWy83k3h8LmDxBx8pGSSSn0hEkSgb+7g5zcbaeS9ejQXbYMvG92cUFtr8A3y3R08JYJqCLG5ROqW2/H2iq2FCsk5PC29Fb3ltYBKRsqGF7Y/TwwPUDf55miJ+nR3pJTLLYmLQZv4h6EWPK/+MrG45SwYk4F6uguoCBMeOg5P63tVwW22zLGQnXcawPVM48UHt3PDwmnkYhJk2ZqvpBDMWA56oSACyd4svxa2wiOVoxCbKR5S1wBJ5VTUG0VDiz/km6Vuq/pgjhBMoGQQDPrsh4w+BvCS7di2WUOtY4shw0wqVptAbF2FiUkNgGHL7DADWFLXzH/3XaVTz4MeGyc9obqjVZJZZ4MVwZN+/kBGa2beDpqS6/JliGQsFFzT3kRzIlHBN7jM0l1fch4I2Np9B9uTVPxCpQrJaYhIgLbF57RisMQNFze78gHQ8qoe4CuWAnpmgylHraRbQKZ1AIcr23B1/yWknJlSgdXNwYav2VEyz95+f0uBfj4BDdVLxSgZBsQWbCbKvsfu+1KSB94BgvZLOhEmgh93DHBxjExYIPh4EDhzTULShbyEJgs8tnjt6z9b6cBfiK/ipY8GKrQ67iZTCoqIMZLpaPtAes3ijYSr8yDwmEa/jcEeSAyWAlIhMeIyrOK2WVkrbC+9bB5AQT1m6AsE6KQtM0IjvAGxxWPCtwL23V9VzssEwGpFvC2RrSsuTaeg6YDm5wgIUMYTPyGrRgg2p4yOC8OcVkZcR47kV6xEVNfMY2uTXGjyO2D+Gm4deZ/KTvEPgGBVwsHJ+0g4p7+G7UnUBKfp0exjM9agsu0PpMSFUon3U0w2fg47CRNwN+Eba6kSpoh+TYgTGGHMvpHzksGwqvofkuDBhx3P0P2NtbINebIeJKk0SurtUzajZaqfe4zHfu4hy8lyPlToE8ucKEFe1QehfOdcdDekymMFkXS95eqk2UMSH24tRHLTdohcJeABTqL0oh6o2Rx5XZJfO5LotuC4ZeauEJzIMSN/DNwtHmVyA0igIxHNiKooEbjMM/itJdi/PLeJM9ReXKHzR4yS47tEzoVDiU6Z6DJvu3E6VaWXovhlBLXjQkuhKntLVIXPI2+MfKbhlQNoa5IWOihii3SiZnaoCwH8IOA3WiTIdeXJdkyxXj6aF66R38oV3bPZ2GxxZ2eany/Q8ulrl7F8csdHkV28iifb81C9n5FYFToPF/y5Lm7w4EsYUU7txzIx+LXCNEwrCTms+m7gbDWZUAy1HNsD+CPjL2Qna6pO+6eIlfgHIhjMekiNXzgdHdB8WdNTH1sNvkcDq9FPkr3tpPWxpaXiDIsnyvJbyDmgSc228ZtvKGXwihlXkV7ggSYY5SqiOU9GtolyXTRX0vB7f5p5BIixGEUXr1D6CBcUwcQb4kjMaTGa07oyHik4Gv4cSAhmhkwzyDw3Bs5DPg5MO/3UtAS1ubPu1J1YRAIolORrg7HLo99kghYGsBThuV/+zcbWE/M4bb4Fxl8AJGi2gSQk0fwbshMjDIJWRfl3uyPKq/sLRU8HxkW40aGcrUHU52XlFJn9ScW1y/cXy7Ws2WhHvkX5nkAftXcEKnZLEGPzs+hoHKEBwd6X6U9Mo25AEBBp1vpX6NM0oEEkWh/lAIA3yJb79XFg2UEOoZN08Nc4DDbavD5y8F1kSr4HAGn23hW87kfPIV6clWcLjGWwLyKhI23MAAMWTkNZVyDt3+SI/12nlu4dgtU4/hvqQvo3VHcgLEitHwT0FVyMG6V3XIHLOW+CZ4vfaBSo5Py4pwljjroHHdQ5ncEXsi1zcdLA5fKQHJpq9nckeifShqyT2FwpMSZviRKTR/zSeK3lXPgQ0p3PRbRLr9f/9c5MUdudluZjkSous6b2TCTXDFAlis0pUj6RLuNK0WkA0myvs8nXZprxudK8fnwMueSvFXvjnIkBID8hyPf+NnOvpT4QQAVHrAwiCwgnFPzQbBnBYApIpDawUvaKRiUx+FK0VWnPmhTdaIR0ObG41Iy1PlhfCZywv4ALi2cJTmPZaElQ6opqTrbUgcUL608xuykXc1KcvlEPcS0VSm+IRep1f/h0CyGA/5jQnhZ6XjMzsMI87PmmkKEmhXyCYPdRei7/N3B+lo91/JTJTaiUPXiiu+4kXU1k7eM8tNVSY0tGtSlWz8VMaXStVbQRh1oRGfFCRPrGhyrAewBBG1t9IXQxSryyyq4p8j8Q5r3BbxsPdViOF3Kw7Vt09lzGr5gjv+toD1OURRmHRiFDPlYDas8ATvQpT/fNVre/VFPi8AkLSNAVLLw+ptCHhn1uTZLb4vpH9lSzZbdrT8HGm+Q6Lh2pqicTWXdD7Oau8i/wPaAp3WjTLnmcBhbOXY9vvXrdwTMipEFssz9M3EGgEIDxBAACcNsAxH8KxycsiXGRpyB71fJpYYlnLbONs7WF7ht47CWvK3/ocsLWXCmpHvIA1ySrqTt8TabYRn6kvmdNhl7gtDJminUBBoY6Gz9gDf6Nw8oPtmlZhex56x8T9TOj1cLz86AcaNEUsqo67ivSlEBKYbhz2ar62w8tSbHG694WHgFWO5EFyocvx07Pocns5bJOw0QAQ94VpwwYyVePBBEsmDg7jZ8JR4zsekeqyOsrv/0p3Q3bHPiCmqP7qMyaliPXWVXNBUxRickLwlyUWjpXi9gFhp2crdZ1C6OSjQew8bk8VKNMOxRxxhUUgEc+YvJfBvhRTIOVbc6nWaxoePk/NKMWt53Va/NHBjfhW9clWVMpXybZ+gUb1AYSyHFjPNeaVeFtBsOaiqrQSPbX4L6Jo/xKPGzyy4rhglJqJi6RZ5ENDt6spN2QgPpE/zd8k3N1RuhhJIzywrrIRXWsxAr82C7WWENY6dOOdxiQHuyqdC1EfwF0s6zSWTQLnkuNdvBttgMkafIb///JiYAA").unwrap();
    for dictionary in [4098u32, 4097] {
        let mut fixture = item("old.bin", &expected);
        fixture.method = 14;
        fixture.payload = vec![9, 4, 5, 0, 93];
        fixture.payload.extend(dictionary.to_le_bytes());
        fixture.payload.extend(&raw);
        let bytes = zip_fixture(&[fixture]);
        std::fs::write(dir.path().join("distance.zip"), &bytes).unwrap();
        let result = w("distance.zip:new", "x").await;
        if dictionary == 4098 {
            result.unwrap();
            assert_eq!(zip_files(&std::fs::read(dir.path().join("distance.zip")).unwrap())[0].1, expected);
        } else {
            assert_eq!(result.unwrap_err().0, "Invalid LZMA match distance 4098 with 4098 bytes available");
            assert_eq!(std::fs::read(dir.path().join("distance.zip")).unwrap(), bytes);
            assert!(!dir.path().join(format!("distance.zip.tmp-{}", std::process::id())).exists());
        }
    }
    // XZ retains every fixed check and pre-filter, concatenation, index-size
    // validation, and wide dictionary admission before materializing a ZIP.
    for (check, encoded) in [
        (
            0,
            "/Td6WFoAAAD/EtlBAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAAAAATZoNM3z6gZynnoBAAAAAABZWg==",
        ),
        (
            1,
            "/Td6WFoAAAFpIt42AgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABZBAnwAAE6aDiCRkaQQpkNAQAAAAABWVo=",
        ),
        (
            4,
            "/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        ),
        (
            10,
            "/Td6WFoAAArh+wyhAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABT4615+lBCckTetiUnGF3pNl9UTOFglKLnjzZSh8CLywABVmiTpg6PGJtLmgEAAAAAClla",
        ),
    ] {
        let mut fixture = item("old.bin", &method_data);
        fixture.method = 95;
        fixture.payload = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        std::fs::write(dir.path().join("xz.zip"), zip_fixture(&[fixture.clone()])).unwrap();
        w("xz.zip:new", "x").await.unwrap();
        assert_eq!(zip_files(&std::fs::read(dir.path().join("xz.zip")).unwrap())[0].1, method_data);
        if check != 0 {
            let len = fixture.payload.len();
            let index = len - 12 - (get32(&fixture.payload, len - 8) + 1) * 4;
            let check_size = match check {
                1 => 4,
                4 => 8,
                _ => 32,
            };
            fixture.payload[index - check_size] ^= 1;
            let bytes = zip_fixture(&[fixture]);
            std::fs::write(dir.path().join("xz.zip"), &bytes).unwrap();
            assert!(w("xz.zip:new", "x").await.unwrap_err().0.contains("block"));
            assert_eq!(std::fs::read(dir.path().join("xz.zip")).unwrap(), bytes);
        }
    }
    for encoded in [
        "/Td6WFoAAATm1rRGAgEDAQIhAQCjXRmw4ABnACZdADGbyKlfiCjnOMnba5qBCXPVlHhL2zT4o3amxrlrwMg/bJ7i6oAAAAAAYc8NajhvnpYAAUJoxnGgoR+2830BAAAAAARZWg==",
        "/Td6WFoAAATm1rRGAgEEACEBAADaM60D4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        "/Td6WFoAAATm1rRGAgEFACEBAAB/4PHI4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        "/Td6WFoAAATm1rRGAgEGACEBAADRkmVO4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        "/Td6WFoAAATm1rRGAgEHACEBAAB0QTmF4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        "/Td6WFoAAATm1rRGAgEIACEBAACh82904ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
        "/Td6WFoAAATm1rRGAgEJACEBAAAEIDO/4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla",
    ] {
        let mut fixture = item("old.bin", &method_data);
        fixture.method = 95;
        fixture.payload = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        std::fs::write(dir.path().join("xz.zip"), zip_fixture(&[fixture])).unwrap();
        w("xz.zip:new", "x").await.unwrap();
        assert_eq!(zip_files(&std::fs::read(dir.path().join("xz.zip")).unwrap())[0].1, method_data);
    }
    let mut wide = item("old.bin", &method_data);
    wide.method = 95;
    wide.payload=base64::engine::general_purpose::STANDARD.decode("/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla").unwrap();
    wide.payload[16] = 40;
    let crc = crc32fast::hash(&wide.payload[12..20]);
    put32(&mut wide.payload, 20, crc);
    std::fs::write(dir.path().join("xz.zip"), zip_fixture(&[wide])).unwrap();
    w("xz.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("xz.zip")).unwrap())[0].1, method_data);
    let mut concat = item("old.bin", &[method_data.as_slice(), method_data.as_slice()].concat());
    concat.method = 95;
    let stream=base64::engine::general_purpose::STANDARD.decode("/Td6WFoAAATm1rRGAgAhARYAAAB0L+Wj4ABnACJdADGbyLBWfekoyDVuoJ96YHIrZ8ePaymGLWxcPL+v/zcmAAAAAABhzw1qOG+elgABPmg8RyoiH7bzfQEAAAAABFla").unwrap();
    concat.payload = [stream.as_slice(), &[0, 0, 0, 0], stream.as_slice()].concat();
    std::fs::write(dir.path().join("xz.zip"), zip_fixture(&[concat.clone()])).unwrap();
    w("xz.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("xz.zip")).unwrap())[0].1, concat.data);

    for (field, value, message) in [
        ("method", 77, "Unsupported ZIP compression method 77"),
        ("flags", 1, "Encrypted ZIP member"),
        ("size", 64 * 1024 * 1024 + 1, "too large to extract"),
    ] {
        let mut bytes = zip_fixture(&[item("old", binary)]);
        let central = 30 + 3 + binary.len();
        match field {
            "method" => put16(&mut bytes, central + 10, value as u16),
            "flags" => put16(&mut bytes, central + 8, value as u16),
            _ => put32(&mut bytes, central + 24, value),
        }
        std::fs::write(dir.path().join("fault.zip"), &bytes).unwrap();
        assert!(w("fault.zip:new", "x").await.unwrap_err().0.contains(message));
        assert_eq!(std::fs::read(dir.path().join("fault.zip")).unwrap(), bytes);
    }
    let mut bytes = zip_fixture(&[item("old", binary)]);
    put16(&mut bytes, 8, 8);
    std::fs::write(dir.path().join("fault.zip"), &bytes).unwrap();
    assert!(w("fault.zip:new", "x").await.unwrap_err().0.contains("compression methods disagree"));
    let mut bytes = zip_fixture(&[item("old", binary)]);
    let len = bytes.len();
    put32(&mut bytes, len - 10, 64 * 1024 * 1024 + 1);
    std::fs::write(dir.path().join("fault.zip"), &bytes).unwrap();
    assert!(w("fault.zip:new", "x").await.unwrap_err().0.contains("ZIP central directory is too large"));
    let mut prefix = b"self-extracting prefix".to_vec();
    prefix.extend(zip_fixture(&[item("old", binary)]));
    std::fs::write(dir.path().join("prefix.zip"), prefix).unwrap();
    w("prefix.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("prefix.zip")).unwrap())[0], ("old".into(), binary.to_vec()));
    for (valid_crc, valid_utf8, expected) in
        [(true, true, "named.bin"), (true, false, "header.bin"), (false, true, "header.bin")]
    {
        let mut entry = item("header.bin", binary);
        let mut extra = vec![0; 9];
        put16(&mut extra, 0, 0x7075);
        let name = if valid_utf8 { b"named.bin".to_vec() } else { vec![0xff] };
        put16(&mut extra, 2, (5 + name.len()) as u16);
        extra[4] = 1;
        put32(&mut extra, 5, if valid_crc { crc32fast::hash(&entry.name) } else { 0 });
        extra.extend(name);
        entry.extra = extra;
        std::fs::write(dir.path().join("unicode.zip"), zip_fixture(&[entry])).unwrap();
        w("unicode.zip:new", "x").await.unwrap();
        assert_eq!(zip_files(&std::fs::read(dir.path().join("unicode.zip")).unwrap())[0].0, expected);
    }
    let mut legacy = item("ignored", binary);
    legacy.name = vec![0x80, b'.', b'b', b'i', b'n'];
    legacy.flags = 0;
    std::fs::write(dir.path().join("legacy.zip"), zip_fixture(&[legacy])).unwrap();
    w("legacy.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("legacy.zip")).unwrap())[0].0, "€.bin");
    for (suffix, size, message) in [("zip", 4097, "Archive member path exceeds 4096 bytes")] {
        let bytes = zip_fixture(&[item(&"p".repeat(size), b"x")]);
        std::fs::write(dir.path().join(format!("limit.{suffix}")), &bytes).unwrap();
        assert_eq!(w(&format!("limit.{suffix}:new"), "x").await.unwrap_err().0, message);
        assert_eq!(std::fs::read(dir.path().join(format!("limit.{suffix}"))).unwrap(), bytes);
    }
    // Directory aliases remain an error for ZIP/ASAR, while TAR directory
    // aliases are indexed directories and are skipped by materialization.
    let mut link = item("alias", b"old");
    link.mode = 0o120777;
    std::fs::write(dir.path().join("links.zip"), zip_fixture(&[item("old", binary), link.clone()])).unwrap();
    w("links.zip:new", "x").await.unwrap();
    assert_eq!(zip_files(&std::fs::read(dir.path().join("links.zip")).unwrap())[1], ("alias".into(), binary.to_vec()));
    link.data = b"folder".to_vec();
    link.payload = link.data.clone();
    std::fs::write(dir.path().join("links.zip"), zip_fixture(&[item("folder/", b""), link])).unwrap();
    assert!(w("links.zip:new", "x").await.unwrap_err().0.contains("is a directory"));
    let mut tar = tar_record("folder/", b"", b'5', "");
    tar.extend(tar_record("folder/old", binary, b'0', ""));
    tar.extend(tar_record("alias", b"", b'2', "folder"));
    tar.extend(tar_record("hard", b"", b'1', "folder/old"));
    tar.resize(tar.len() + 1024, 0);
    std::fs::write(dir.path().join("links.tar"), tar).unwrap();
    w("links.tar:new", "x").await.unwrap();
    let files = tar_files(&std::fs::read(dir.path().join("links.tar")).unwrap());
    assert!(!files.contains_key("alias"));
    assert_eq!(files["hard"], binary);
    for (kind, target, message) in [
        (b'1', "missing", "targets missing member"),
        (b'2', "missing", "cannot be materialized"),
        (b'S', "", "is a sparse file"),
    ] {
        let mut bytes = tar_record("bad", b"", kind, target);
        bytes.resize(bytes.len() + 1024, 0);
        std::fs::write(dir.path().join("bad.tar"), &bytes).unwrap();
        assert!(w("bad.tar:new", "x").await.unwrap_err().0.contains(message));
        assert_eq!(std::fs::read(dir.path().join("bad.tar")).unwrap(), bytes);
    }
    let mut oldgnu = tar_record("hard", b"", b'1', "old");
    oldgnu.extend(tar_record("old", binary, b'0', ""));
    oldgnu.extend(tar_record("././@LongLink", b"Rename old to renamed/\n", b'N', ""));
    oldgnu.resize(oldgnu.len() + 1024, 0);
    std::fs::write(dir.path().join("oldgnu.tar"), oldgnu).unwrap();
    w("oldgnu.tar:new", "x").await.unwrap();
    let members = tar_files(&std::fs::read(dir.path().join("oldgnu.tar")).unwrap());
    assert!(!members.contains_key("old"));
    assert_eq!(members["renamed"], binary);
    assert_eq!(members["hard"], binary);
    let mut signed = tar_record("old", binary, b'0', "");
    signed[0] = 0xff;
    signed[124..136].fill(0);
    signed[124] = 0x80;
    signed[135] = binary.len() as u8;
    signed[148..156].fill(32);
    let sum: i64 = signed[..512].iter().map(|&b| (b as i8) as i64).sum();
    signed[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    signed[154] = 0;
    signed[155] = 32;
    signed.resize(signed.len() + 1024, 0);
    std::fs::write(dir.path().join("signed.tar"), signed).unwrap();
    w("signed.tar:new", "x").await.unwrap();
    assert_eq!(tar_files(&std::fs::read(dir.path().join("signed.tar")).unwrap())["�ld"], binary);
    for (name, payload) in [("pseudo", binary.to_vec()), ("emptyname", tar_record("", b"", b'0', ""))] {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&payload).unwrap();
        let compressed = encoder.finish().unwrap();
        let path = format!("{name}.tgz");
        std::fs::write(dir.path().join(&path), compressed).unwrap();
        w(&format!("{path}:new"), "x").await.unwrap();
        assert_eq!(unpack("tgz", &std::fs::read(dir.path().join(path)).unwrap())[name], payload);
    }
    let long = "q".repeat(180);
    w(&format!("archives/fresh.tar:{long}"), "long").await.unwrap();
    assert_eq!(tar_files(&std::fs::read(dir.path().join("archives/fresh.tar")).unwrap())[&long], b"long");
    let asar = asar_fixture(
        "{\"files\":{\"\\ud800\":{\"size\":1,\"offset\":\"0\"},\"\\ud801\":{\"size\":1,\"offset\":\"1\"}}}",
        b"AB",
    );
    std::fs::write(dir.path().join("units.asar"), asar).unwrap();
    w("units.asar:new", "C").await.unwrap();
    let files = asar_files(&std::fs::read(dir.path().join("units.asar")).unwrap());
    assert_eq!(files[&ara_rpc::WireString::from_units(vec![0xd800])], b"A");
    assert_eq!(files[&ara_rpc::WireString::from_units(vec![0xd801])], b"B");
    let mut deep = "{\"size\":1,\"offset\":\"0\"}".to_owned();
    for _ in 0..1024 {
        deep = format!("{{\"files\":{{\"a\":{deep}}}}}");
    }
    std::fs::write(dir.path().join("deep.asar"), asar_fixture(&deep, b"D")).unwrap();
    w("deep.asar:new", "x").await.unwrap();
    assert_eq!(asar_files(&std::fs::read(dir.path().join("deep.asar")).unwrap()).len(), 2);
    for depth in [40, 41] {
        let mut files = serde_json::Map::new();
        files.insert("leaf".into(), json!({"size":1,"offset":"0"}));
        for i in 0..depth {
            files.insert(format!("a{i}"), json!({"link":if i+1==depth{"leaf".into()}else{format!("a{}",i+1)}}));
        }
        let bytes = asar_fixture(&json!({"files":files}).to_string(), b"L");
        std::fs::write(dir.path().join("depth.asar"), &bytes).unwrap();
        let result = w("depth.asar:new", "x").await;
        if depth == 40 {
            result.unwrap();
            assert_eq!(asar_files(&std::fs::read(dir.path().join("depth.asar")).unwrap()).len(), 42);
        } else {
            assert!(result.unwrap_err().0.contains("cyclic symlink"));
            assert_eq!(std::fs::read(dir.path().join("depth.asar")).unwrap(), bytes);
        }
    }
    let invalid = asar_fixture("{\"files\":{\"bad\":{\"size\":1,\"offset\":\"0\",\"executable\":0}}}", b"x");
    std::fs::write(dir.path().join("bad.asar"), invalid).unwrap();
    assert!(w("bad.asar:new", "x").await.unwrap_err().0.contains("file 'bad' has a non-boolean executable flag"));
    use sha2::Digest as _;
    let hash = format!("{:x}", sha2::Sha256::digest(binary));
    let header=json!({"files":{"old":{"size":binary.len(),"offset":"0","integrity":{"algorithm":"SHA256","hash":hash,"blockSize":64,"blocks":[hash]}}}}).to_string();
    std::fs::write(dir.path().join("integrity.asar"), asar_fixture(&header, binary)).unwrap();
    w("integrity.asar:new", "x").await.unwrap();
    let bad = asar_fixture(&header, b"\0\xfffail\r\n");
    std::fs::write(dir.path().join("integrity.asar"), &bad).unwrap();
    assert!(w("integrity.asar:new", "x").await.unwrap_err().0.contains("SHA256 integrity verification"));
    assert_eq!(std::fs::read(dir.path().join("integrity.asar")).unwrap(), bad);
    std::fs::write(
        dir.path().join("unpacked.asar"),
        asar_fixture("{\"files\":{\"old\":{\"size\":8,\"unpacked\":true}}}", b""),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("unpacked.asar.unpacked")).unwrap();
    std::fs::write(dir.path().join("unpacked.asar.unpacked/old"), binary).unwrap();
    w("unpacked.asar:new", "x").await.unwrap();
    assert_eq!(
        asar_files(&std::fs::read(dir.path().join("unpacked.asar")).unwrap())[&ara_rpc::WireString::from("old")],
        binary
    );
    let home = tempfile::tempdir().unwrap();
    let cwd_zip = zip_fixture(&[item("old", b"cwd-only")]);
    std::fs::write(dir.path().join("home.zip"), &cwd_zip).unwrap();
    std::fs::write(home.path().join("home.zip"), zip_fixture(&[item("old", b"home-original")])).unwrap();
    #[cfg(windows)]
    {
        let drive = home.path().to_string_lossy();
        assert_eq!(drive.as_bytes()[1], b':', "drive-relative witness requires a local drive temp root");
        let name = format!("{}:probe.bin", &drive[..1]);
        let header = json!({"files":{name:{"size":8,"unpacked":true}}}).to_string();
        std::fs::write(dir.path().join("drive.asar"), asar_fixture(&header, b"")).unwrap();
        std::fs::create_dir(dir.path().join("drive.asar.unpacked")).unwrap();
        std::fs::write(home.path().join("probe.bin"), b"external").unwrap();
    }
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .arg("--exact")
        .arg("write_creates_dirs_counts_utf16_and_marks_shebang")
        .arg("--nocapture")
        .env("HOME", home.path())
        .env("ARA_ARCHIVE_HOME_WITNESS_CWD", dir.path())
        .current_dir(home.path())
        .kill_on_drop(true);
    let child = tokio::time::timeout(Duration::from_secs(15), child.output())
        .await
        .expect("bounded archive HOME/scan/drive witness timed out")
        .unwrap();
    assert!(
        child.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(std::fs::read(dir.path().join("home.zip")).unwrap(), cwd_zip);
    assert_eq!(
        zip_files(&std::fs::read(home.path().join("home.zip")).unwrap()),
        vec![("old".into(), b"home-original".to_vec()), ("new".into(), b"home-only".to_vec())]
    );
    let tmp = dir.path().join(format!("literal.zip.tmp-{}", std::process::id()));
    std::fs::create_dir(&tmp).unwrap();
    std::fs::write(tmp.join("owned"), b"keep").unwrap();
    let original = std::fs::read(&selector).unwrap();
    assert!(w("literal.zip:new", "x").await.is_err());
    assert_eq!(std::fs::read(&selector).unwrap(), original);
    assert_eq!(std::fs::read(tmp.join("owned")).unwrap(), b"keep");
    std::fs::remove_dir_all(&tmp).unwrap();
    let rename_target = dir.path().join("rename.zip");
    let obstruction = rename_target.clone();
    let update: UpdateFn = Arc::new(move |_| std::fs::create_dir(&obstruction).unwrap());
    assert!(
        write
            .execute(
                "rename-fail",
                args(json!({"path":"rename.zip:new","content":"x"})),
                CancellationToken::new(),
                update
            )
            .await
            .is_err()
    );
    assert!(rename_target.is_dir());
    assert!(!dir.path().join(format!("rename.zip.tmp-{}", std::process::id())).exists());
    let cancelled = CancellationToken::new();
    let stop = cancelled.clone();
    assert!(
        write
            .execute(
                "archive-cancel",
                args(json!({"path":"archive-cancel/new.zip:member","content":"x"})),
                cancelled,
                Arc::new(move |_| stop.cancel())
            )
            .await
            .unwrap_err()
            .0
            .contains("Cancelled")
    );
    assert!(!dir.path().join("archive-cancel").exists());
    for filter in [10, 11] {
        let mut entry = item("old.bin", &[0; 64]);
        entry.method = 95;
        entry.payload = base64::engine::general_purpose::STANDARD
            .decode("/Td6WFoAAATm1rRGAgEEACEBAADaM60D4AA/AAZdAABuWEaYAAAAAAIkMBalelTeAAEiQJuy6PEftvN9AQAAAAAEWVo=")
            .unwrap();
        entry.payload[14] = filter;
        let crc = crc32fast::hash(&entry.payload[12..20]);
        put32(&mut entry.payload, 20, crc);
        std::fs::write(dir.path().join("xz.zip"), zip_fixture(&[entry])).unwrap();
        w("xz.zip:new", "x").await.unwrap();
        assert_eq!(zip_files(&std::fs::read(dir.path().join("xz.zip")).unwrap())[0].1, vec![0; 64]);
    }
    // Eight actual AUIPC bytes enter the differing fixed OMP predicate; check0
    // exercises the ZIP CRC and check1 also exercises the XZ CRC32.
    for encoded in [
        "/Td6WFoAAAD/EtlBAgELACEBAAAPgfvyAQAHlwAAAAOAAAAAAAEYCEClRqwGcp56AQAAAAAAWVo=",
        "/Td6WFoAAAFpIt42AgELACEBAAAPgfvyAQAHlwAAAAOAAAAA79bnswABHAhEYCrIkEKZDQEAAAAAAVla",
    ] {
        let expected = [0x97, 0, 0, 0, 3, 0x80, 0, 0];
        let mut fixture = item("riscv.bin", &expected);
        fixture.method = 95;
        fixture.payload = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        std::fs::write(dir.path().join("riscv.zip"), zip_fixture(&[fixture])).unwrap();
        w("riscv.zip:new", "x").await.unwrap();
        assert_eq!(zip_files(&std::fs::read(dir.path().join("riscv.zip")).unwrap())[0].1, expected);
    }
    struct ArchiveProgress {
        root: std::path::PathBuf,
        fail: bool,
        seen: Mutex<Vec<ToolOutput>>,
    }
    #[async_trait::async_trait]
    impl write::WriteProgress for ArchiveProgress {
        async fn prepare_batch(&self) -> Result<Option<edit_host::EditLspBatch>, ara_agent::ToolError> {
            panic!("archive precedes native batch Getter")
        }
        async fn acknowledge(&self, output: ToolOutput) -> Result<(), ara_agent::ToolError> {
            assert!(!self.root.exists());
            self.seen.lock().unwrap().push(output);
            if self.fail { Err(ara_agent::ToolError("original archive progress refusal".into())) } else { Ok(()) }
        }
    }
    for fail in [false, true] {
        let root = dir.path().join(format!("archive-ack-{fail}"));
        let progress = Arc::new(ArchiveProgress { root: root.clone(), fail, seen: Mutex::new(Vec::new()) });
        let out = write
            .execute_os_values_with_progress(
                &format!("archive-ack-{fail}/file.zip:member"),
                "hé😀",
                CancellationToken::new(),
                progress.clone(),
            )
            .await;
        let seen = progress.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].details.as_ref().unwrap()["resolvedPath"], json!(root.join("file.zip").to_string_lossy()));
        if fail {
            assert!(matches!(out, Err(write::WriteExecutionError::NotDispatched(_))));
            assert!(!root.exists());
        } else {
            assert!(!out.unwrap().is_error);
            assert_eq!(zip_files(&std::fs::read(root.join("file.zip")).unwrap())[0].1, "hé😀".as_bytes());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let path = dir.path().join("locked.zip");
        let original = zip_fixture(&[item("old", binary)]);
        std::fs::write(&path, &original).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        assert!(w("locked.zip:new", "x").await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!dir.path().join(format!("locked.zip.tmp-{}", std::process::id())).exists());
        drop(held);
    }
    #[cfg(unix)]
    {
        let target = dir.path().join("extensionless");
        let mut tar = tar_record("old", binary, b'0', "");
        tar.resize(tar.len() + 1024, 0);
        std::fs::write(&target, tar).unwrap();
        std::os::unix::fs::symlink("extensionless", dir.path().join("outer.zip")).unwrap();
        w("outer.zip:new", "x").await.unwrap();
        assert!(std::fs::symlink_metadata(dir.path().join("outer.zip")).unwrap().file_type().is_symlink());
        assert_eq!(tar_files(&std::fs::read(target).unwrap())["old"], binary);
    }
    struct ArchiveHost {
        policy: edit_host::EditHostPathPolicy,
        invalidated: Mutex<Vec<std::path::PathBuf>>,
        versions: Mutex<usize>,
    }
    impl edit_host::EditHostPort for ArchiveHost {
        fn path_policy(&self) -> edit_host::EditHostPathPolicy {
            self.policy.clone()
        }
        fn invalidate_after_write(&self, path: &std::path::Path) {
            self.invalidated.lock().unwrap().push(path.to_owned());
        }
        fn bump_file_mutation_version(&self, _: &std::path::Path) {
            *self.versions.lock().unwrap() += 1;
        }
    }
    let host = Arc::new(ArchiveHost {
        policy: Default::default(),
        invalidated: Mutex::new(Vec::new()),
        versions: Mutex::new(0),
    });
    let tool = write::WriteTool::new(plain(dir.path()).with_edit_host(host.clone()));
    tool.execute("archive-host", args(json!({"path":"host.zip:a","content":"x"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert_eq!(*host.invalidated.lock().unwrap(), vec![dir.path().join("host.zip")]);
    assert_eq!(*host.versions.lock().unwrap(), 0);
    let plan = Arc::new(ArchiveHost {
        policy: edit_host::EditHostPathPolicy { plan_active: true, ..Default::default() },
        invalidated: Mutex::new(Vec::new()),
        versions: Mutex::new(0),
    });
    let tool = write::WriteTool::new(plain(dir.path()).with_edit_host(plan));
    assert!(
        tool.execute(
            "archive-plan",
            args(json!({"path":"plan-new.zip:a","content":"x"})),
            CancellationToken::new(),
            noop()
        )
        .await
        .unwrap_err()
        .0
        .contains("read-only")
    );
    assert!(!dir.path().join("plan-new.zip").exists());
}

#[tokio::test]
async fn bash_success_error_env_cwd() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let (_, _, bash) = tools(dir.path());
    let b = |v: Value| bash.execute("c", args(v), CancellationToken::new(), noop());
    let out = b(json!({"command": "echo out; echo err 1>&2"})).await.unwrap();
    assert!(!out.is_error);
    let t = text(&out);
    assert!(t.contains("out") && t.contains("err"), "{t}");
    assert_eq!(out.details.as_ref().unwrap()["timeoutSeconds"], json!(300));
    let out = b(json!({"command": "echo nope; exit 3"})).await.unwrap();
    assert!(out.is_error);
    assert_eq!(text(&out), "nope\n\nCommand exited with code 3");
    assert_eq!(out.details.unwrap()["exitCode"], json!(3));
    assert_eq!(text(&b(json!({"command": "true"})).await.unwrap()), "(no output)");
    let out =
        b(json!({"command": "echo $ARA_X $PAGER $GIT_TERMINAL_PROMPT; pwd", "env": {"ARA_X": "set"}, "cwd": "sub"}))
            .await
            .unwrap();
    let t = text(&out);
    assert!(t.starts_with("set cat 0\n") && t.ends_with("/sub"), "{t}");
    assert!(
        b(json!({"command": "true", "cwd": "nope"}))
            .await
            .unwrap_err()
            .0
            .starts_with("Working directory does not exist")
    );
    let out = b(json!({"command": "seq 1 20000"})).await.unwrap();
    let t = text(&out);
    assert!(t.ends_with("of 20000]") && t.contains("\n20000\n"), "tail kept");
}

#[tokio::test]
async fn bash_timeout_kills_process_group() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let marker = dir.path().join("survived");
    let started = Instant::now();
    let cmd = "(sleep 3; touch survived) & echo started; wait";
    let out =
        bash.execute("c", args(json!({"command": cmd, "timeout": 1})), CancellationToken::new(), noop()).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(2800), "{:?}", started.elapsed());
    assert!(out.is_error);
    assert_eq!(text(&out), "started\n\n[Command timed out after 1 seconds]");
    assert_eq!(out.details.unwrap()["timedOut"], json!(true));
    tokio::time::sleep(Duration::from_millis(3000)).await;
    assert!(!marker.exists(), "background child in the group was killed");
}

#[tokio::test]
async fn bash_cancel_aborts_and_streams_updates() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let marker = dir.path().join("after-cancel");
    let updates = Arc::new(Mutex::new(Vec::<String>::new()));
    let u2 = updates.clone();
    let update: UpdateFn = Arc::new(move |o| u2.lock().unwrap().push(text(&o)));
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let u3 = updates.clone();
    tokio::spawn(async move {
        // Cancel only after a live partial update has been observed.
        loop {
            if u3.lock().unwrap().iter().any(|t| t.contains("tick 1")) {
                c2.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let started = Instant::now();
    let err = bash
        .execute(
            "c",
            args(json!({"command": "(sleep 2; touch after-cancel) & for i in 1 2 3 4 5 6 7 8 9; do echo tick $i; sleep 1; done"})),
            cancel,
            update,
        )
        .await
        .unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(4));
    assert!(err.0.starts_with("tick 1") && err.0.ends_with("[Command aborted]"), "{}", err.0);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "background child continued after cancellation");
}

#[tokio::test]
async fn builtin_tool_set() {
    let names: Vec<String> = builtin_tools(ToolContext::new(".")).iter().map(|t| t.definition().name).collect();
    assert_eq!(names, vec!["read", "write", "edit", "bash", "grep", "glob"]);
}

#[tokio::test]
async fn bash_keeps_stream_order_and_reaps_background_children() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let b = |v: Value| bash.execute("c", args(v), CancellationToken::new(), noop());
    let out = b(json!({"command": "printf 'a\\n'; printf 'b\\n' >&2; printf 'c\\n'"})).await.unwrap();
    assert_eq!(text(&out), "a\nb\nc");
    let marker = dir.path().join("bg-survived");
    let started = Instant::now();
    let out = b(json!({"command": "(sleep 2; touch bg-survived) & echo hi"})).await.unwrap();
    assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
    assert_eq!(text(&out), "hi");
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "background child killed with the call");
    let out = b(json!({"command": "kill -9 $$"})).await.unwrap();
    assert!(out.is_error);
    #[cfg(unix)]
    let signal_exit = 137;
    #[cfg(windows)]
    let signal_exit = 2304; // Git Bash returns the raw Windows exit code (9 << 8).
    assert!(text(&out).ends_with(&format!("Command exited with code {signal_exit}")), "{}", text(&out));
    let out = b(json!({"command": "head -c 30000000 /dev/zero | tr '\\0' 'x' | fold -w 100"})).await.unwrap();
    assert!(text(&out).len() < DEFAULT_MAX_BYTES + 200);
    assert!(text(&out).ends_with("of 300000]"), "{}", &text(&out)[text(&out).len() - 60..]);
}

#[tokio::test]
async fn dropping_a_bash_call_kills_its_group() {
    let dir = tempfile::tempdir().unwrap();
    let (_, _, bash) = tools(dir.path());
    let marker = dir.path().join("after-drop");
    let cmd = "sleep 2; touch after-drop";
    let r = tokio::time::timeout(
        Duration::from_millis(300),
        bash.execute("c", args(json!({"command": cmd})), CancellationToken::new(), noop()),
    )
    .await;
    assert!(r.is_err());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!marker.exists(), "group killed by the drop guard");
}

#[tokio::test]
async fn read_bounds_and_robustness() {
    let dir = tempfile::tempdir().unwrap();
    // Keep the legacy >1000-row lossy-decode case explicit, independent of
    // the fixed ordinary default300. The 50KiB case uses its own100-row budget.
    let read = read::ReadTool { ctx: plain(dir.path()).with_read_default_limit(3000.0) };
    let capped = read::ReadTool { ctx: plain(dir.path()).with_read_default_limit(100.0) };
    let r = |p: &str| read.execute("c", args(json!({"path": p})), CancellationToken::new(), noop());
    std::fs::write(dir.path().join("one.txt"), "y".repeat(200_000) + "\nsecond\n").unwrap();
    let out = text(&capped.execute("c", args(json!({"path":"one.txt"})), CancellationToken::new(), noop()).await.unwrap());
    assert!(out.len() < DEFAULT_MAX_BYTES + 300, "{}", out.len());
    assert!(
        out.contains("[Line 1 is 195.3KB, exceeds 50.0KB limit. Showing its first 50.0KB.]"),
        "{}",
        &out[out.len() - 200..]
    );
    assert!(out.ends_with("[1 more lines in file. Use :2 to continue]"));
    let mut late = "ok ok ok ok ok ok ok\n".repeat(1000).into_bytes();
    late.extend_from_slice(&[0xE9, b'\n']);
    std::fs::write(dir.path().join("lat.txt"), late).unwrap();
    let out = text(&r("lat.txt").await.unwrap());
    assert!(out.ends_with("\u{FFFD}"), "late invalid byte decoded lossily");
    assert!(r("one.txt:0").await.unwrap_err().0.starts_with("Invalid selector ':0' on 'one.txt'"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("nowhere"), dir.path().join("log:1-2")).unwrap();
        assert_eq!(r("log:1-2").await.unwrap_err().0, "File not found: log:1-2", "dangling symlink is a literal path");
        let fifo = dir.path().join("pipe");
        let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o644) }, 0);
        assert_eq!(r("pipe").await.unwrap_err().0, "Cannot read pipe: not a regular file");
    }
    let ctx = ToolContext::new(dir.path());
    let expected_home = std::env::var_os("HOME")
        .map(|home| std::path::PathBuf::from(home).join("y"))
        .unwrap_or_else(|| dir.path().join("~/y"));
    assert_eq!(ctx.resolve("~/x/../y"), expected_home);
    assert_eq!(ctx.resolve("a/./b/../c"), dir.path().join("a/c"));
}

// Fixed OMP 596f2da read-directory-range.test.ts: complete original five cases.
#[tokio::test]
async fn original_read_directory_range_five_cases() {
    let dir = tempfile::tempdir().unwrap();
    for i in 1..=60 {
        std::fs::write(dir.path().join(format!("file-{i:03}.txt")), "").unwrap();
    }
    let tool = read::ReadTool { ctx: plain(dir.path()) };
    let read = |suffix: &str| {
        tool.execute(
            "dir",
            args(json!({"path": format!("{}{suffix}", dir.path().display())})),
            CancellationToken::new(),
            noop(),
        )
    };
    let full = read("").await.unwrap(); // B-0dab2f7f25
    let full_text = text(&full);
    assert_eq!(full.details.unwrap()["isDirectory"], true);
    assert!(full_text.contains("file-001.txt") && full_text.contains("file-060.txt"));
    let slice = text(&read(":30-40").await.unwrap()); // B-40d8af0689
    assert!(slice.lines().filter(|line| line.contains("file-")).count() <= 11);
    assert!(slice.contains("more lines in listing") && slice.contains("Use :41 to continue"));
    let offset = text(&read(":30").await.unwrap()); // B-8a1e0c9257
    assert!(offset.lines().filter(|line| line.contains("file-")).count() > 10);
    assert!(!offset.contains("Use :"));
    let tail = text(&read(":-5").await.unwrap()); // B-78592f8d6e
    let expected = full_text.split('\n').collect::<Vec<_>>();
    assert_eq!(tail, expected[expected.len() - 5..].join("\n"));
    assert!(!tail.contains("Use :"));
    let beyond = text(&read(":9999").await.unwrap()); // B-7fd3ffb60d
    assert!(beyond.contains("Line 9999 is beyond end of listing") && beyond.contains("lines total"));
    // B-1c74a4ddc3 (read-multi-range original): directory multi selector rejection.
    let error = tool
        .execute(
            "dir-multi",
            args(json!({"path":format!("{}:1-2,5-6",dir.path().display())})),
            CancellationToken::new(),
            noop(),
        )
        .await
        .unwrap_err();
    assert!(error.0.contains("Multi-range line selectors are not supported for directory listings"));

    // Production-tree dependency coverage: root stays uncapped, children keep
    // recent11 + oldest, hidden files remain, build/cache paths are excluded.
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    for i in 1..=14 {
        std::fs::write(sub.join(format!("child-{i:02}.txt")), "").unwrap();
    }
    let oldest = sub.join("oldest.txt");
    std::fs::write(&oldest, "").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&oldest)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(10))
        .unwrap();
    std::fs::create_dir_all(dir.path().join("depth/inside")).unwrap();
    std::fs::write(dir.path().join("depth/inside/deeper.txt"), "").unwrap();
    for excluded in ["target", ".cache"] {
        std::fs::create_dir(dir.path().join(excluded)).unwrap();
        std::fs::write(dir.path().join(excluded).join("excluded.txt"), "").unwrap();
    }
    std::fs::write(dir.path().join(".DS_Store"), "").unwrap();
    std::fs::write(dir.path().join(".hidden"), "").unwrap();
    let full = read("").await.unwrap();
    let listing = text(&full);
    assert!(listing.starts_with(".\n"));
    assert!(listing.contains("- … 3 more") && listing.contains("oldest.txt"));
    assert!(listing.contains(".hidden") && listing.contains("inside/"));
    assert!(!listing.contains("deeper.txt") && !listing.contains("excluded.txt") && !listing.contains(".DS_Store"));
    let details = full.details.unwrap();
    assert_eq!(details["meta"]["source"], json!({"type":"path","value":dir.path().to_string_lossy()}));
    assert_eq!(details["meta"]["limits"]["resultLimit"], json!({"reached":1,"suggestion":2}));
    assert!(details["meta"].get("resultLimit").is_none());
    let beyond = read(":9999").await.unwrap();
    let details = beyond.details.unwrap();
    assert_eq!(details["meta"]["source"], json!({"type":"path","value":dir.path().to_string_lossy()}));
    assert!(details["meta"].get("limits").is_none());
}

// Fixed original read-fs-not-abortable.test.ts; keep conflict cancellation boundary.
#[tokio::test]
async fn original_read_fs_not_abortable_four_cases() {
    let dir = tempfile::tempdir().unwrap();
    let tool = read::ReadTool { ctx: ToolContext::new(dir.path()) };
    let content = (1..=40).map(|i| format!("line-{i}")).collect::<Vec<_>>().join("\n");
    std::fs::write(dir.path().join("range.txt"), &content).unwrap();
    std::fs::write(dir.path().join("multi.txt"), &content).unwrap();
    for i in 1..=5 {
        std::fs::write(dir.path().join(format!("f-{i}.txt")), "").unwrap();
    }
    let token = CancellationToken::new();
    token.cancel();
    // B-6b1ef28b4f
    let single = tool.execute("single", args(json!({"path":"range.txt:20-22"})), token.clone(), noop()).await.unwrap();
    assert!(!single.is_error && text(&single).contains("line-20") && text(&single).contains("line-22"));
    // B-599bb65be2
    let multi =
        tool.execute("multi", args(json!({"path":"multi.txt:2-3,30-31"})), token.clone(), noop()).await.unwrap();
    assert!(
        !multi.is_error
            && text(&multi).contains("line-2")
            && text(&multi).contains("line-30")
            && text(&multi).contains("line-31")
    );
    // B-5ec92716d4
    let listing = tool.execute("listing", args(json!({"path":dir.path()})), token.clone(), noop()).await.unwrap();
    assert!(!listing.is_error && listing.details.as_ref().unwrap()["isDirectory"] == true);
    assert!(text(&listing).contains("f-1.txt") && text(&listing).contains("f-5.txt"));
    // B-43ec8c3a7f
    std::fs::write(dir.path().join("conflicted.txt"), "hello\nworld\n").unwrap();
    let error =
        tool.execute("conflicts", args(json!({"path":"conflicted.txt:conflicts"})), token, noop()).await.unwrap_err();
    assert_eq!(error.0, "Aborted: Cancelled");
}

struct ReadBufferBridge {
    content: String,
    enabled: bool,
    fail: bool,
    reads: Mutex<Vec<std::path::PathBuf>>,
}
#[async_trait::async_trait]
impl edit_host::EditClientBridge for ReadBufferBridge {
    fn can_write_text_file(&self) -> bool {
        false
    }
    fn can_read_text_file(&self) -> bool {
        self.enabled
    }
    async fn read_text_file(&self, path: &std::path::Path) -> std::io::Result<String> {
        self.reads.lock().unwrap().push(path.to_owned());
        if self.fail {
            return Err(std::io::Error::other("editor read failure"));
        }
        Ok(self.content.clone())
    }
    async fn write_text_file(&self, _: &std::path::Path, _: &str) -> std::io::Result<()> {
        panic!("read-only bridge")
    }
}
struct ReadBufferHost(Arc<ReadBufferBridge>);
impl edit_host::EditHostPort for ReadBufferHost {
    fn client_bridge(&self) -> Option<Arc<dyn edit_host::EditClientBridge>> {
        Some(self.0.clone())
    }
}
#[tokio::test]
async fn original_read_acp_fs_two_cases_and_source_snapshot_controls() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = Arc::new(ReadBufferBridge {
        content: "// content from editor buffer\nexport function greet() { return 'bridge'; }\n".into(),
        enabled: true,
        fail: false,
        reads: Mutex::new(vec![]),
    });
    // This exact-one-call supplemental check owns the ordinary ACP route.
    // Summary's failed attempt plus fresh ACP acquisition is covered separately.
    let ctx = ToolContext::new(dir.path())
        .with_read_summary_policy(read_summary::SummaryPolicy { enabled: false, ..Default::default() })
        .with_edit_host(Arc::new(ReadBufferHost(bridge.clone())));
    let tool = read::ReadTool { ctx: ctx.clone() };
    let path = dir.path().join("example.ts");
    std::fs::write(&path, "export function greet() { return 'disk'; }\n").unwrap();
    // B-d05f349070: successful full-buffer ownership; disk content is never rendered.
    let output = tool.execute("plain", args(json!({"path":path})), CancellationToken::new(), noop()).await.unwrap();
    assert!(text(&output).contains("content from editor buffer"));
    assert!(!text(&output).contains("return 'disk'"));
    assert_eq!(bridge.reads.lock().unwrap().as_slice(), &[path.clone()]);
    let key = ara_edit::path_policy::canonical_key(&path);
    assert_eq!(ctx.edit_store.head(&key).unwrap().text.as_ref(), bridge.content);
    // B-9f4b0eba55: the bridge API only accepts a path, so selectors cannot run twice.
    std::fs::write(dir.path().join("range.txt"), "disk one\ndisk two\ndisk three\n").unwrap();
    let range_bridge = Arc::new(ReadBufferBridge {
        content: "bridge one\nbridge two\nbridge three\n".into(),
        enabled: true,
        fail: false,
        reads: Mutex::new(vec![]),
    });
    let range_tool =
        read::ReadTool { ctx: plain(dir.path()).with_edit_host(Arc::new(ReadBufferHost(range_bridge.clone()))) };
    let range = range_tool
        .execute("range", args(json!({"path":"range.txt:2+1"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert!(text(&range).contains("bridge two"));
    assert!(!text(&range).contains("Line 2 is beyond end") && !text(&range).contains("disk two"));
    assert_eq!(range_bridge.reads.lock().unwrap().len(), 1);

    // B-b1ca794128: retain all original multi-range buffer assertions.
    std::fs::write(dir.path().join("disk.txt"), "disk one\ndisk two\ndisk three\ndisk four\ndisk five\n").unwrap();
    let multi_bridge = Arc::new(ReadBufferBridge {
        content: "bridge one\nbridge two\nbridge three\nbridge four\nbridge five\n".into(),
        enabled: true,
        fail: false,
        reads: Mutex::new(vec![]),
    });
    let multi_ctx = ToolContext::new(dir.path()).with_edit_host(Arc::new(ReadBufferHost(multi_bridge)));
    let multi = read::ReadTool { ctx: multi_ctx }
        .execute("multi", args(json!({"path":"disk.txt:1-2,4-5"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    let shown = text(&multi);
    for expected in ["bridge one", "bridge two", "bridge four", "bridge five"] {
        assert!(shown.contains(expected));
    }
    assert!(!shown.contains("bridge three") && !shown.contains("disk one"));
    // B-ae3db8007d: actual disk CUT2 from a bridge-sourced returned tag.
    let blank_text = "first\n\nlast\n";
    std::fs::write(dir.path().join("bridge.txt"), blank_text).unwrap();
    let blank_bridge = Arc::new(ReadBufferBridge {
        content: blank_text.into(),
        enabled: true,
        fail: false,
        reads: Mutex::new(vec![]),
    });
    let blank_ctx = ToolContext::new(dir.path()).with_edit_host(Arc::new(ReadBufferHost(blank_bridge)));
    let blank = read::ReadTool { ctx: blank_ctx.clone() }
        .execute("blank", args(json!({"path":"bridge.txt:1-2,3-3"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    let shown = text(&blank);
    let header = shown.lines().next().unwrap();
    assert!(regex::Regex::new(r"^\[bridge\.txt#[0-9A-F]{4}\]$").unwrap().is_match(header), "{shown}");
    assert!(shown.contains("1:first\n2:") && !shown.contains("\n4:"));
    let edited = edit::EditTool::new(blank_ctx)
        .execute("blank-edit", args(json!({"input":format!("{header}\nCUT 2")})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert!(!edited.is_error, "{}", text(&edited));
    assert_eq!(std::fs::read_to_string(dir.path().join("bridge.txt")).unwrap(), "first\nlast\n");
    for (enabled, fail) in [(false, false), (true, true)] {
        let off =
            Arc::new(ReadBufferBridge { content: "wrong source".into(), enabled, fail, reads: Mutex::new(vec![]) });
        let tool = read::ReadTool { ctx: plain(dir.path()).with_edit_host(Arc::new(ReadBufferHost(off.clone()))) };
        let output = tool
            .execute("fallback", args(json!({"path":"range.txt:raw:2-2"})), CancellationToken::new(), noop())
            .await
            .unwrap();
        assert_eq!(text(&output), "disk two", "disabled or failed ACP bridge falls back to ordinary disk raw semantics");
        assert_eq!(output.details.as_ref().unwrap()["truncation"]["truncatedBy"], json!("lines"));
        assert_eq!(output.details.as_ref().unwrap()["truncation"]["outputLines"], json!(1));
        assert_eq!(off.reads.lock().unwrap().len(), usize::from(enabled));
    }
}

// Fixed workspace-tree.ts uses utils/format.ts for actual file metadata.
#[tokio::test]
async fn read_directory_format_bytes_rounding_matches_fixed_utils() {
    let dir = tempfile::tempdir().unwrap();
    for (name, size) in [("tie-kb.bin", 1280), ("tie-mb.bin", 1310720)] {
        let file = std::fs::File::create(dir.path().join(name)).unwrap();
        file.set_len(size).unwrap();
        // Tree rows show size only when their modification age is nonzero.
        let modified = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        file.set_times(std::fs::FileTimes::new().set_modified(modified)).unwrap();
    }
    let tool = read::ReadTool { ctx: ToolContext::new(dir.path()) };
    let result = tool.execute("size", args(json!({"path":dir.path()})), CancellationToken::new(), noop()).await.unwrap();
    let body = text(&result);
    assert!(!result.is_error);
    for (name, size) in [("tie-kb.bin", "1.3KB"), ("tie-mb.bin", "1.3MB")] {
        let row = body.lines().find(|row| row.contains(name)).unwrap();
        assert!(row.contains(size), "{row}");
    }
}

// Fixed read-guidance.test.ts B-5680c8ed9b and read.ts renderDescription flags.
#[test]
fn read_guidance_and_live_description_group() {
    struct Policy(Arc<Mutex<bool>>);
    impl ReadImagePolicyPort for Policy {
        fn current_state(&self) -> ReadImageRuntime {
            ReadImageRuntime { inspect_image_active: *self.0.lock().unwrap(), ..Default::default() }
        }
    }
    for hashline in [false, true] {
        let active = Arc::new(Mutex::new(false));
        let tool = read::ReadTool { ctx: ToolContext::new(".")
            .with_edit(ara_edit::EditMode::Hashline, hashline)
            .with_read_image_policy(Arc::new(Policy(active.clone()))) };
        for inspect in [false, true, false] {
            *active.lock().unwrap() = inspect;
            let description = tool.definition().description;
            for phrase in ["searchable with `grep`", "use `bash` with a remote SSH command", "`sshfs`"] {
                assert!(description.contains(phrase), "{phrase}: {description}");
            }
            for retired in ["`search`", "`ssh` tool", "{{"] {
                assert!(!description.contains(retired), "{retired}: {description}");
            }
            assert_eq!(description.contains("`[foo.ts#1A2B]` snapshot header"), hashline);
            assert_eq!(description.contains("Images → metadata; call `inspect_image`"), inspect);
            assert_eq!(description.contains("Images → decoded inline"), !inspect);
            assert!(description.contains("`:raw` → untouched HTML"));
            assert!(description.contains("Summary footer names elided ranges?"));
        }
    }
}

// Fixed OMP596f2da read.ts763-809/1287-1290/1402-1447 and
// path-utils.ts868-1014. One grouped entry per routing/aggregate/fault boundary.
// Native graph invocation and original failure carriers have separate Host tests.
#[tokio::test]
async fn read_delimited_core_routing_group() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "A\n").unwrap();
    std::fs::write(dir.path().join("b.txt"), "B\n").unwrap();
    // This routing group exercises actual ZIP creation/READ, not Host TZ.
    // Its narrowly controlled calendar import supplies a valid fixed instant.
    struct RoutingArchiveDate;
    impl ArchiveDatePort for RoutingArchiveDate {
        fn to_utc_ms(&self, _: ArchiveLocalDate) -> Result<f64, ara_agent::ToolError> {
            Ok(315_532_800_000.0) // 1980-01-01T00:00:00Z; fixture calendar only
        }
    }
    let ctx = plain(dir.path()).with_archive_date_port(Arc::new(RoutingArchiveDate));
    let read = read::ReadTool { ctx: ctx.clone() };
    let write = write::WriteTool::new(ctx);
    let directory_cancel = CancellationToken::new(); directory_cancel.cancel();
    let directory = read.execute_os_path(".", directory_cancel).await.unwrap();
    assert!(!text(&directory).is_empty(), "fixed directory READ ignores scan cancellation");
    let run = |path: &str| {
        let read = &read;
        let path = path.to_owned();
        async move { read.execute_os_path(&path, CancellationToken::new()).await }
    };
    let output = run("a.txt; b.txt").await.unwrap();
    assert_eq!(output.content, vec![UserBlock::text("Note: interpreted as 2 paths: a.txt, b.txt\n\nA\n\nB")]);
    assert_eq!(output.details, Some(json!({"notes":["Note: interpreted as 2 paths: a.txt, b.txt"],
        "displayReadTargets":["a.txt","b.txt"]})));
    assert!(!output.is_error && !output.useless && output.provider_metadata.is_none());
    for entry in ["a.txt, b.txt", "a.txt b.txt", "a.txt, b.txt a.txt"] {
        let parts = paths::split_delimited_entry(&read.ctx, entry, paths::Splitter::Search).unwrap().unwrap();
        let output = run(entry).await.unwrap();
        assert_eq!(output.details.as_ref().unwrap()["displayReadTargets"], json!(parts));
    }
    let output = run(";;; a.txt ;; ").await.unwrap();
    assert_eq!(text(&output), "Note: interpreted as 1 paths: a.txt\n\nA");
    let output = run("missing.txt; absent.txt").await.unwrap();
    assert_eq!(text(&output), "Note: interpreted as 2 paths: missing.txt, absent.txt\n\n[Could not read missing.txt: File not found: missing.txt]\n\n[Could not read absent.txt: File not found: absent.txt]");
    assert_eq!(output.details.as_ref().unwrap()["notes"].as_array().unwrap().len(), 3);
    assert!(run("missing.txt, absent.txt").await.is_err(), "comma requires some resolving part");
    assert!(run("a.txt absent.txt").await.is_err(), "whitespace requires all parts");
    assert_eq!(paths::split_delimited_entry(&read.ctx, r"a.txt\;b.txt", paths::Splitter::Search).unwrap(), None);
    let not_routed = |_: &str| false;
    assert_eq!(paths::split_delimited_entry_with_routed(&read.ctx, "\u{feff}a.txt\u{feff}b.txt\u{feff}",
        paths::Splitter::Search, Some(&not_routed)).unwrap(), Some(vec!["a.txt".into(),"b.txt".into()]),
        "fixed JavaScript whitespace includes BOM");
    assert_eq!(paths::split_delimited_entry_with_routed(&read.ctx, "a.txt\u{0085}b.txt",
        paths::Splitter::Search, Some(&not_routed)).unwrap(), None, "JavaScript whitespace excludes NEL");
    std::fs::write(dir.path().join("brace{a;b}.txt"), "BRACE").unwrap();
    assert_eq!(text(&run("brace{a;b}.txt; b.txt").await.unwrap()),
        "Note: interpreted as 2 paths: brace{a;b}.txt, b.txt\n\nBRACE\n\nB");
    std::fs::write(dir.path().join("a.txt;b.txt"), "LITERAL").unwrap();
    let literal = run("a.txt;b.txt").await.unwrap();
    assert_eq!(text(&literal), "LITERAL");
    assert!(literal.details.as_ref().unwrap().get("displayReadTargets").is_none());
    #[cfg(unix)] {
        std::fs::write(dir.path().join("a.txt;b.txt:1-2"), "COLON_LITERAL").unwrap();
        assert_eq!(text(&run("a.txt;b.txt:1-2").await.unwrap()), "COLON_LITERAL");
    }

    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/a.txt, b.txt"), "COMMA_SUFFIX").unwrap();
    let suffix = run("a.txt, b.txt").await.unwrap();
    assert!(text(&suffix).contains("COMMA_SUFFIX"));
    assert_eq!(suffix.details.as_ref().unwrap()["suffixResolution"]["to"], json!("nested/a.txt, b.txt"));
    std::fs::write(dir.path().join("nested/a.txt; b.txt"), "SEMICOLON_SUFFIX").unwrap();
    assert_eq!(text(&run("a.txt; b.txt").await.unwrap()),
        "Note: interpreted as 2 paths: a.txt, b.txt\n\nA\n\nB", "explicit list precedes suffix recovery");
    std::fs::write(dir.path().join("nested/c.txt"), "C").unwrap();
    let suffix_part = run("a.txt; c.txt").await.unwrap();
    assert_eq!(suffix_part.details.as_ref().unwrap()["displayReadTargets"], json!(["a.txt","nested/c.txt"]));

    // Actual existing archive and SQLite resources containing delimiters win.
    write.execute("setup", args(json!({"path":"bundle;copy.zip:inside.txt","content":"ARCHIVE"})),
        CancellationToken::new(), noop()).await.unwrap();
    let archive = run("bundle;copy.zip:inside.txt").await.unwrap();
    assert_eq!(text(&archive), "ARCHIVE");
    assert!(archive.details.as_ref().unwrap().get("displayReadTargets").is_none());
    let db = rusqlite::Connection::open(dir.path().join("data;copy.db")).unwrap();
    db.execute_batch("CREATE TABLE samples(value TEXT); INSERT INTO samples VALUES ('SQLITE');").unwrap();
    drop(db);
    let sqlite = run("data;copy.db:samples").await.unwrap();
    assert!(text(&sqlite).contains("SQLITE"));
    assert!(sqlite.details.as_ref().unwrap().get("displayReadTargets").is_none());

    // Media classification must still give a missing explicit list to the seam.
    for entry in ["a.txt;missing.mp4:5", "a.txt;missing.pdf:img", "a.txt;missing.svg:img"] {
        let output = run(entry).await.unwrap();
        assert_eq!(output.details.as_ref().unwrap()["displayReadTargets"].as_array().unwrap().len(), 2, "{entry}");
        assert!(text(&output).contains("\n\nA\n\n[Could not read missing."), "{entry}: {}", text(&output));
    }
    assert_eq!(run("a.txt:img").await.unwrap_err().0, "The ':img' selector only supports local .svg and .svgz files.");
    assert_eq!(run("unknown://a;unknown://b").await.unwrap_err().0, "Unknown protocol: unknown://");
}

#[derive(Default)]
struct DelimitedCorePort {
    calls: Mutex<Vec<String>>,
    cancel_at: Mutex<Option<String>>,
}

#[async_trait::async_trait]
impl ContentUriPort for DelimitedCorePort {
    fn route(&self, url: &str) -> ContentUriRoute {
        if url.starts_with("removed://") { ContentUriRoute::Removed }
        else if url.starts_with("delim://") { ContentUriRoute::Registered { writable:false } }
        else { ContentUriRoute::Unregistered }
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ara_agent::ToolError> {
        self.calls.lock().unwrap().push(url.to_owned());
        if self.cancel_at.lock().unwrap().as_deref() == Some(url) {
            cancel.cancel();
            return Err(ara_agent::ToolError("original cancelled child".into()));
        }
        if url == "delim://fail" { return Err(ara_agent::ToolError("ordinary child failure".into())); }
        Ok(UriResource {content:url.to_owned(),content_type:"text/plain".into(),notes:vec![],immutable:true})
    }
    async fn write(&self, _: &str, _: &str, _: CancellationToken) -> Result<(), ara_agent::ToolError> {
        unreachable!("READ must never invoke write")
    }
}

struct DelimitedCoreDispatcher(Mutex<Vec<Vec<String>>>);
#[async_trait::async_trait]
impl read::ReadDelimitedDispatcher for DelimitedCoreDispatcher {
    async fn read_parts(&self, parts: Vec<String>, _: CancellationToken) -> Result<ToolOutput, ara_agent::ToolError> {
        self.0.lock().unwrap().push(parts);
        Ok(ToolOutput::text("native parts ack"))
    }
}

#[tokio::test]
async fn read_delimited_core_routed_dispatch_and_cancel_group() {
    let dir = tempfile::tempdir().unwrap();
    let port = Arc::new(DelimitedCorePort::default());
    let read = read::ReadTool {ctx:plain(dir.path()).with_uri_port(port.clone())};
    assert_eq!(read.routed_delimited_parts("delim://a;delim://b", &|_| false).unwrap(),
        Some(vec!["delim://a".into(),"delim://b".into()]));
    for entry in ["delim://a;missing.txt", "delim://a,delim://b", "delim://a delim://b", "removed://a;removed://b"] {
        assert_eq!(read.routed_delimited_parts(entry, &|_| false).unwrap(), None, "{entry}");
    }
    assert_eq!(read.routed_delimited_parts("local://a;local://b", &|entry| entry.starts_with("local://")).unwrap(),
        Some(vec!["local://a".into(),"local://b".into()]));
    assert_eq!(paths::split_delimited_entry(&read.ctx, "local://a;local://b", paths::Splitter::Search).unwrap(), None,
        "default grep/glob parser retains existing behavior");
    let output = read.execute_os_path("delim://a;delim://fail;delim://b", CancellationToken::new()).await.unwrap();
    assert_eq!(text(&output), "Note: interpreted as 3 paths: delim://a, delim://fail, delim://b\n\ndelim://a\n\n[Could not read delim://fail: ordinary child failure]\n\ndelim://b");
    assert_eq!(output.details.as_ref().unwrap()["displayReadTargets"], json!(["delim://a","delim://fail","delim://b"]));
    assert_eq!(*port.calls.lock().unwrap(), ["delim://a","delim://fail","delim://b"]);

    // Internal resolvability is consulted before part filesystem stat.
    let routed = |entry: &str| entry.starts_with("delim://");
    assert_eq!(paths::split_delimited_entry_with_routed(&read.ctx, "missing.txt,delim://a", paths::Splitter::Search, Some(&routed)).unwrap(),
        None, "registered custom URI is not a lexical internal path");
    assert_eq!(paths::split_delimited_entry_with_routed(&read.ctx, "missing.txt,local://a", paths::Splitter::Search, Some(&routed)).unwrap(),
        Some(vec!["missing.txt".into(),"local://a".into()]), "lexical internal part resolves without a routed predicate hit");
    for (part, internal) in [("local:/a",true),("@local:/a",true),("@agent://a",true),("ssh://a",true),("@ssh://a",false),("@vault://a",false),("LOCAL://a",false)] {
        let entry = format!("missing.txt,{part}");
        assert_eq!(paths::split_delimited_entry_with_routed(&read.ctx, &entry, paths::Splitter::Search, Some(&routed)).unwrap().is_some(), internal, "{entry}");
    }
    let dispatcher = Arc::new(DelimitedCoreDispatcher(Mutex::new(vec![])));
    let prior_calls = port.calls.lock().unwrap().len();
    assert_eq!(text(&read.execute_os_path_with_delimited_dispatcher("delim://a;delim://b", CancellationToken::new(), Some(dispatcher.clone())).await.unwrap()), "native parts ack");
    assert_eq!(text(&read.execute_promoted_os_path_with_delimited_dispatcher("a.txt;b.txt", None, CancellationToken::new(), Some(dispatcher.clone())).await.unwrap()), "native parts ack");
    assert_eq!(port.calls.lock().unwrap().len(), prior_calls, "worker seam must not execute any child");
    assert_eq!(*dispatcher.0.lock().unwrap(), vec![vec!["delim://a".to_owned(),"delim://b".to_owned()],vec!["a.txt".to_owned(),"b.txt".to_owned()]]);

    port.calls.lock().unwrap().clear();
    *port.cancel_at.lock().unwrap() = Some("delim://cancel".into());
    let error = read.execute_os_path("delim://a;delim://cancel;delim://later", CancellationToken::new()).await.unwrap_err();
    assert_eq!(error.0, "original cancelled child");
    assert_eq!(*port.calls.lock().unwrap(), ["delim://a","delim://cancel"]);
    port.calls.lock().unwrap().clear();
    let cancelled = CancellationToken::new(); cancelled.cancel();
    assert_eq!(read.execute_os_path("delim://a;delim://b", cancelled).await.unwrap_err().0, "Read was aborted");
    assert!(port.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn read_delimited_core_text_image_order_group() {
    use base64::Engine;
    const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("before.txt"), "BEFORE").unwrap();
    std::fs::write(dir.path().join("after.txt"), "AFTER").unwrap();
    std::fs::write(dir.path().join("p.png"), base64::engine::general_purpose::STANDARD.decode(TINY_PNG).unwrap()).unwrap();
    let read = read::ReadTool {ctx:plain(dir.path()).with_read_auto_resize_images(false)
        .with_read_image_codec(Arc::new(bun_image_codec::NativeBunCodec::new()))};
    let image = read.execute_os_path("p.png", CancellationToken::new()).await.unwrap();
    let output = read.execute_os_path("before.txt;p.png;after.txt", CancellationToken::new()).await.unwrap();
    assert_eq!(output.content.len(), 3);
    assert_eq!(output.content[0], UserBlock::text(format!("Note: interpreted as 3 paths: before.txt, p.png, after.txt\n\nBEFORE\n\n{}", text(&image))));
    assert_eq!(output.content[1], image.content[1]);
    assert_eq!(output.content[2], UserBlock::text("AFTER"));
    assert_eq!(output.details, Some(json!({"notes":["Note: interpreted as 3 paths: before.txt, p.png, after.txt"],
        "displayReadTargets":["before.txt","p.png","after.txt"]})));
}
