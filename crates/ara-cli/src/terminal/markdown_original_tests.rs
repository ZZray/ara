//! Fixed 596f2da original assertions; grouped to share one build invocation.
use super::*;
#[test]
fn source_osc8_normalization_precedes_storage_equality_and_cache_identity() {
    let theme = Arc::new(Theme::dark());
    let st = WireString::from("\x1b]8;;https://example.test\x1b\\label\x1b]8;;\x1b\\");
    let bel = WireString::from("\x1b]8;;https://example.test\x07label\x1b]8;;\x07");
    let mut md = MarkdownComponent::new(st, theme.clone());
    assert_eq!(md.source.units(), bel.units());
    let first = md.render(80);
    assert!(!md.set_text(bel.clone()));
    assert!(Arc::ptr_eq(&first, &md.render(80)));
    let mut other = MarkdownComponent::new(bel, theme.clone());
    assert!(Arc::ptr_eq(&first, &other.render(80)));
    let isolated = WireString::from_units(vec![0xd800, 27, 93, 56, 59, 59, 27, 92, 0xdc00]);
    let wire = MarkdownComponent::new(isolated, theme);
    assert_eq!(wire.source.units(), &[0xd800, 27, 93, 56, 59, 59, 7, 0xdc00]);
    // Regression for review v4: lossy display must not merge distinct original
    // UTF16 source owners in L2; repeated identical original units still reuse.
    let theme = Arc::new(Theme::dark());
    let first = MarkdownComponent::new(WireString::from_units(vec![0xd800]), theme.clone()).render(80);
    let different = MarkdownComponent::new(WireString::from_units(vec![0xd801]), theme.clone()).render(80);
    let repeated = MarkdownComponent::new(WireString::from_units(vec![0xd800]), theme).render(80);
    assert!(!Arc::ptr_eq(&first, &different), "distinct original unpaired UTF16 source cache identities");
    assert!(Arc::ptr_eq(&first, &repeated), "identical original unpaired UTF16 source reuses L2");
}
fn plain(s: &str) -> String {
    regex::Regex::new(r"\x1b\]8;[^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;:]*m").unwrap().replace_all(s, "").into_owned()
}

#[test]
fn original_three_utf16_scanner_ids() {
    let mut fixtures: Vec<String> = [
        "",
        "plain prose with no candidates at all",
        "$x$ math first",
        "prose then $inline$ math",
        "prose then \\(paren\\) math",
        "prose then \\[bracket\\] math",
        "\\( before $ dollar",
        "$ before \\( paren",
        "backslash only \\ then ( apart",
        "ends with backslash \\",
        "ends with dollar $",
        "www.example.com leading",
        "see www.example.com mid-string",
        "see WWW.EXAMPLE.COM upper",
        "mixed WwW.case.com scan",
        "http://example.com leading",
        "prose http://example.com mid",
        "prose HTTPS://EXAMPLE.COM upper",
        "HtTpS://mixed.example",
        "ftp://files.example mid ftp",
        "prose FTP://FILES.EXAMPLE",
        "ftps:// is not ftp:// until here ftp://x",
        "wwww.overlap.example",
        "hhttp://overlap.example",
        "http:/ missing slash then https://real.example",
        "www without dot www. with dot",
        "w h f teaser chars but no scheme",
        "user@example.com email",
        "prose user.name+tag@example.co.uk",
        "trailing at sign only@ ",
        "@leading-at no local part",
        "short@x",
        "dots...and+plus_under-score@host.tld",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    fixtures.push("a".repeat(400));
    fixtures.push(format!("{}@example.com", "a".repeat(400)));
    let math_re = regex::Regex::new(r"\$|\\\(|\\\[").unwrap();
    let scheme_re = regex::Regex::new(r"(?i)www\.|https?://|ftp://").unwrap();
    let gfm_re = regress::Regex::new(r"^((?:[hH][tT][tT][pP][sS]?|[fF][tT][pP]):\/\/|www\.)(?:[a-zA-Z0-9-]+\.?)+[^\s<]*|^[A-Za-z0-9._+-]+(@)[a-zA-Z0-9-_]+(?:\.[a-zA-Z0-9-_]*[a-zA-Z0-9])+(?![-_])").unwrap();
    for src in &fixtures {
        assert_eq!(
            markdown_math::math_start_index(src, 0),
            math_re.find(src).map(|m| src[..m.start()].encode_utf16().count()),
            "B-e54eed2d3f {src:?}"
        );
        assert_eq!(
            autolink_scheme_scan_index(&WireString::from(src.clone())),
            scheme_re.find(src).map(|m| src[..m.start()].encode_utf16().count()),
            "B-38ff62c1ac {src:?}"
        );
        if gfm_re.find(src).is_some() {
            assert!(url_token_possible(&WireString::from(src.clone())), "B-6575d7ff5d {src:?}");
        }
    }
    assert!(!url_token_possible(&WireString::from("plain prose, nothing linkable here")), "B-6575d7ff5d prose gate");
    assert!(!url_token_possible(&WireString::from("@leading-at no local part")), "B-6575d7ff5d local gate");
}

#[test]
fn original_eight_static_cache_and_state_ids() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    clear_render_cache();
    let theme = Arc::new(Theme::dark());
    let calls = Arc::new(AtomicUsize::new(0));
    let spy = calls.clone();
    let options = MarkdownOptions {
        highlighter: Some(Arc::new(move |code, _lang| {
            spy.fetch_add(1, Ordering::SeqCst);
            vec![code.to_owned()]
        })),
        ..Default::default()
    };
    let text = WireString::from("```js\nconst x = 1;\n```");
    let mut md1 = MarkdownComponent::new(text.clone(), theme.clone());
    md1.set_options(options.clone());
    let lines1 = md1.render(80);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "B-d7348e4e9f cold highlight");
    let mut md2 = MarkdownComponent::new(text, theme.clone());
    md2.set_options(options);
    let lines2 = md2.render(80);
    assert_eq!(calls.load(Ordering::SeqCst), 1, "B-d7348e4e9f L2 highlight");
    assert_eq!(lines2, lines1, "B-d7348e4e9f bytes");

    clear_render_cache();
    let mut md = MarkdownComponent::new(WireString::from("Cache identity sentinel"), theme.clone());
    let first = md.render(80);
    assert!(Arc::ptr_eq(&md.render(80), &first), "B-c9ae39b65a L1");
    let mut l2 = MarkdownComponent::new(WireString::from("Cache identity sentinel"), theme.clone());
    assert!(Arc::ptr_eq(&l2.render(80), &first), "B-c9ae39b65a L2");

    clear_render_cache();
    let mut md = MarkdownComponent::new(WireString::from("Question text"), theme.clone());
    md.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    let first = md.render(40);
    assert!(Arc::ptr_eq(&md.render(40), &first), "B-42a4740770 repeat one");
    assert!(Arc::ptr_eq(&md.render(40), &first), "B-42a4740770 repeat two");

    clear_render_cache();
    let mut a = MarkdownComponent::new(WireString::from("Shared markdown body"), theme.clone());
    let mut b = MarkdownComponent::new(WireString::from("Shared markdown body"), theme.clone());
    a.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    b.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    assert!(Arc::ptr_eq(&a.render(40), &b.render(40)), "B-1034584183 L2 arrays");

    clear_render_cache();
    let paragraph = format!("cache-budget sentinel {}", "x".repeat(120));
    let large = (0..1400).map(|i| format!("Paragraph {i}: {paragraph}")).collect::<Vec<_>>().join("\n\n");
    let mut first = MarkdownComponent::new(WireString::from(large.clone()), theme.clone());
    let mut second = MarkdownComponent::new(WireString::from(large), theme.clone());
    let a = first.render(80);
    let b = second.render(80);
    assert_eq!(a, b, "B-568e3affd4 oversized bytes");
    assert!(!Arc::ptr_eq(&a, &b), "B-568e3affd4 oversized references");

    clear_render_cache();
    let mut md = MarkdownComponent::new(WireString::from("Before edit"), theme.clone());
    md.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    let before = md.render(40);
    assert!(before.iter().any(|l| plain(l).contains("Before edit")), "B-c016cb0ce5 before");
    md.set_text(WireString::from("After edit"));
    let after = md.render(40);
    assert!(!Arc::ptr_eq(&before, &after), "B-c016cb0ce5 edited reference");
    assert!(after.iter().any(|l| plain(l).contains("After edit")), "B-c016cb0ce5 after");
    assert!(!after.iter().any(|l| plain(l).contains("Before edit")), "B-c016cb0ce5 old absent");
    assert!(Arc::ptr_eq(&md.render(40), &after), "B-c016cb0ce5 stable edited reference");

    clear_render_cache();
    let mut md = MarkdownComponent::new(WireString::from("streamed content"), theme.clone());
    md.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    let before = md.render(40);
    assert!(!md.set_text(WireString::from("streamed content")), "B-30ba784329 unchanged result");
    assert!(Arc::ptr_eq(&md.render(40), &before), "B-30ba784329 unchanged cache");
    assert!(md.set_text(WireString::from("new content")), "B-30ba784329 changed result");
    assert!(!Arc::ptr_eq(&md.render(40), &before), "B-30ba784329 invalidated cache");

    clear_render_cache();
    let mut md = MarkdownComponent::new(WireString::from("Width sentinel content"), theme);
    md.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    let narrow = md.render(30);
    let wide = md.render(60);
    assert!(!Arc::ptr_eq(&narrow, &wide), "B-b046370a2f width references");
    assert!(narrow.iter().all(|l| render_utils::width(l) <= 30), "B-b046370a2f narrow widths");
    assert!(wide.iter().all(|l| render_utils::width(l) <= 60), "B-b046370a2f wide widths");
    clear_render_cache();
}
#[test]
fn original_inline_display_and_nul_ids() {
    let theme = Arc::new(Theme::dark());
    let input = WireString::from("\x1b]8;;file:///tmp/example.ts\x1b\\`example.ts`\x1b]8;;\x1b\\");
    let output = render_inline_markdown(Some(&input), theme.clone(), None);
    assert!(output.contains("\x1b]8;;file:///tmp/example.ts\x07"), "B-4268fb7730 OSC8");
    assert_eq!(plain(&output), "example.ts", "B-4268fb7730 plain");
    let ordered = WireString::from("1. Review against a base branch (PR Style)");
    assert_eq!(
        plain(&render_inline_markdown(Some(&ordered), theme.clone(), None)),
        "1. Review against a base branch (PR Style)",
        "B-a53947ec4f"
    );
    assert_eq!(render_inline_markdown(None, theme.clone(), None), "", "B-61e9af0c1e");
    assert_eq!(render_inline_markdown(None, theme.clone(), Some(&|s| format!("[{s}]"))), "[]", "B-761b0c56f6");
    let file = "\x1b]8;;file:///tmp/DisplayTypeEnum.java\x1b\\`DisplayTypeEnum.java`\x1b]8;;\x1b\\";
    let source = WireString::from(format!(
        "| Module | Local file | Change |\n| --- | --- | --- |\n| common | {file} | Added display type |"
    ));
    let lines = MarkdownComponent::new(source, theme.clone()).render(80);
    let output = lines.join("\n");
    assert!(output.contains("\x1b]8;;file:///tmp/DisplayTypeEnum.java\x07"), "B-e808bc83fd OSC8");
    assert!(plain(&output).contains("DisplayTypeEnum.java"), "B-e808bc83fd visible");
    assert!(!plain(&output).contains("`DisplayTypeEnum.java`"), "B-e808bc83fd code span");
    for line in lines.iter() {
        assert!(render_utils::width(line) <= 80, "B-e808bc83fd width: {line:?}");
    }
    let mut nul = MarkdownComponent::new(WireString::from("before\0after"), theme);
    nul.set_options(MarkdownOptions { padding_x: 1, ..Default::default() });
    assert_eq!(
        nul.render(80).iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>(),
        vec![" before\0after"],
        "B-8dda02936d"
    );
}
fn rendered(source: &str, width: usize) -> Vec<String> {
    MarkdownComponent::new(WireString::from(source), Arc::new(Theme::dark()))
        .render(width)
        .iter()
        .map(|l| plain(l).trim_end().to_owned())
        .filter(|l| !l.is_empty())
        .collect()
}
#[test]
fn original_markdown_math_fourteen_ids() {
    assert_eq!(rendered(r"the area is $A = \pi r^2$ exactly", 100)[0], "the area is A = π r² exactly", "B-8da8a730da");
    assert_eq!(rendered("energy $x_i^2 + y_j^2$ done", 100)[0], "energy xᵢ² + yⱼ² done", "B-f1741fa6a3");
    let lines = rendered("$$\n\\begin{bmatrix} a & b \\\\ c & d \\end{bmatrix}\n$$", 100);
    assert_eq!(lines.len(), 3, "B-fefe085f4a");
    assert!(lines[0].starts_with('⎡') && lines.last().unwrap().ends_with('⎦'), "B-fefe085f4a brackets");
    assert_eq!(
        lines.join("").chars().filter(|c| !c.is_whitespace() && !"⎡⎤⎢⎥⎣⎦".contains(*c)).collect::<String>(),
        "abcd",
        "B-fefe085f4a content"
    );
    let lines = rendered("\\[\nx = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}\n\\]", 100);
    let row = lines.iter().position(|l| l.contains("x =")).unwrap();
    assert!(row > 1, "B-7e155fddde");
    assert!(
        lines[row].contains("───")
            && lines[row - 1].contains("-b ± ╲│ b² - 4ac")
            && lines[row - 2].contains('┌')
            && lines[row + 1].contains("2a"),
        "B-7e155fddde layout"
    );
    for (id, source) in
        [("B-989af88073", "Here:\n\n$$\\frac{a+b}{c}$$\n\nDone."), ("B-29b1c446f8", "$$\n\\frac{a+b}{c}\n$$")]
    {
        let lines = rendered(source, 100);
        let row = lines.iter().position(|l| l.contains('─')).unwrap();
        assert!(row > 0 && lines[row - 1].contains("a+b") && lines[row + 1].contains('c'), "{id}");
    }
    let lines = rendered("- result:\n\n  $$\n  \\begin{bmatrix} a \\\\ b \\end{bmatrix}\n  $$", 100);
    let open = lines.iter().position(|l| l.contains("⎡ a")).unwrap();
    let close = lines.iter().position(|l| l.contains("b ⎦")).unwrap();
    assert!(close > open, "B-a17f29698e");
    assert_eq!(
        rendered("use `$x^2$` literally and $y^2$ as math", 100)[0],
        "use $x^2$ literally and y² as math",
        "B-77608bee9d"
    );
    assert!(rendered("```\n$a$ and $b$\n```", 100).iter().any(|l| l.contains("$a$ and $b$")), "B-a984a99843");
    assert_eq!(rendered("it costs $5 and $10 total", 100)[0], "it costs $5 and $10 total", "B-dd55eaac8f");
    assert_eq!(rendered(r"prose \(x \\) y\) end", 100)[0], "prose x  ) y end", "B-723cd773a7 paren");
    assert_eq!(rendered(r"$$a \$$ b$$", 100)[0], "a $ b", "B-723cd773a7 dollar");
    for (id, input, expected) in
        [("B-b4ce2823f7", "energy $E=mc^2$ here", "energy E=mc² here"), ("B-32f4fbb6c0", "$$E = mc^2$$", "E = mc²")]
    {
        assert_eq!(
            plain(&render_inline_markdown(Some(&WireString::from(input)), Arc::new(Theme::dark()), None)),
            expected,
            "{id}"
        );
    }
    let lines = rendered("\\begin{equation}\nx = \\frac{a+b}{c}\n\\end{equation}", 100);
    let row = lines.iter().position(|l| l.contains('─')).unwrap();
    assert!(
        lines[row].contains("x =") && lines[row - 1].contains("a+b") && lines[row + 1].contains('c'),
        "B-2899978941"
    );
}
fn tree_raw(source: &str, width: usize) -> Arc<Vec<String>> {
    let (theme, options) = super::capture_tests::fixed_default_markdown_theme();
    let mut component = MarkdownComponent::new(WireString::from(source), theme);
    component.set_options(options);
    component.render(width)
}
fn tree_plain(source: &str, width: usize) -> Vec<String> {
    tree_raw(source, width).iter().map(|l| plain(l).trim_end().to_owned()).collect()
}
#[test]
fn original_tree_hanging_wrap_sixteen_ids() {
    let node = "가나다라 마바사아 자차카타 파하가나 다라마바 사자차카";
    let raw = tree_raw(&format!("├── {node}"), 40);
    let rows = raw.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>();
    assert!(rows.len() >= 2 && rows[0].starts_with("├── 가나다라"), "B-9ef1583263");
    for row in &rows[1..] {
        assert!(row.starts_with("│   ") && row.chars().nth(4) != Some(' '), "B-9ef1583263 indent");
    }
    for row in raw.iter() {
        assert!(render_utils::width(row) <= 40, "B-9ef1583263 width");
    }
    let joined = std::iter::once(rows[0].chars().skip(4).collect::<String>())
        .chain(rows[1..].iter().map(|l| l.chars().skip(4).collect::<String>()))
        .collect::<String>()
        .replace(' ', "");
    assert_eq!(joined, node.replace(' ', ""), "B-9ef1583263 lossless");
    for (id, source, start, hang, offset) in [
        ("B-3ec24bc551", "│   └── delta echo foxtrot golf hotel india juliet", "│   └── delta", "│       ", 8),
        ("B-207826099c", "└── alpha bravo charlie delta echo foxtrot golf hotel", "└── alpha", "    ", 4),
        ("B-a3ff4674f7", "╰── alpha bravo charlie delta echo foxtrot golf hotel", "╰── alpha", "    ", 4),
    ] {
        let rows = tree_plain(source, 40);
        assert!(rows.len() >= 2 && rows[0].starts_with(start), "{id}");
        for row in &rows[1..] {
            assert!(row.starts_with(hang) && row.chars().nth(offset) != Some(' '), "{id} indent");
            if offset == 4 {
                assert!(!row.contains('│'), "{id} releases rail");
            }
        }
    }
    for (id, text) in [
        ("B-073d421729", "alpha bravo charlie delta echo foxtrot golf hotel india juliet kilo lima"),
        ("B-ff23fa2629", "── alpha bravo charlie delta echo foxtrot golf hotel india"),
        ("B-2a1ea2a923", "│ is the Unicode vertical box drawing glyph used for rails in terminal trees"),
        ("B-dc7630f3e8", "├ marks a branch point in a tree diagram and has no horizontal connector here"),
    ] {
        let rows = tree_plain(text, 40);
        assert_eq!(
            rows,
            render_utils::wrap(text, 40).iter().map(|l| l.trim_end().to_owned()).collect::<Vec<_>>(),
            "{id} plain wrapper"
        );
        assert!(rows.len() >= 2, "{id} overflow");
        for row in &rows[1..] {
            assert!(!row.starts_with([' ', '│']), "{id} flush");
        }
    }
    assert_eq!(
        tree_plain("├── alpha\n│   └── beta\n└── gamma", 40),
        vec!["├── alpha", "│   └── beta", "└── gamma"],
        "B-cd45c1d332"
    );
    let code = "├── alpha bravo charlie delta echo foxtrot golf hotel india";
    let raw = tree_raw(&format!("```\n{code}\n```"), 40);
    let rows = raw.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>();
    assert_eq!(rows[0], "```", "B-7759953bbb open");
    assert_eq!(rows.last().unwrap(), "```", "B-7759953bbb close");
    let row = rows.iter().position(|l| l.contains("├──")).unwrap();
    assert!(row > 0 && row + 1 < rows.len() - 1, "B-7759953bbb wrapped");
    assert!(!rows[row + 1].is_empty() && !rows[row + 1].starts_with([' ', '│']), "B-7759953bbb flush");
    for row in raw.iter() {
        assert!(render_utils::width(row) <= 40, "B-7759953bbb width");
    }
    let raw = tree_raw("├── aaaa bbbb **cccc dddd eeee ffff gggg hhhh**", 40);
    let rows = raw.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>();
    assert!(rows.len() >= 2 && rows[1].starts_with("│   hhhh"), "B-f7a2c5d164");
    assert!(raw[1].find("\x1b[1m").unwrap() < raw[1].find("hhhh").unwrap(), "B-f7a2c5d164 bold carry");
    let rows = tree_plain("> ├── alpha bravo charlie delta echo foxtrot golf hotel india", 40);
    assert!(rows.len() >= 2 && rows[0].starts_with("│ ├── alpha"), "B-b36dc2f03e");
    let continuation = rows[1..].iter().filter(|l| !l.is_empty()).collect::<Vec<_>>();
    assert!(!continuation.is_empty(), "B-b36dc2f03e continuation");
    for row in continuation {
        assert!(row.starts_with("│ │   ") && row.chars().nth(6) != Some(' '), "B-b36dc2f03e hang");
    }
    let text = "├── alpha bravo charlie delta echo foxtrot golf hotel india";
    let rows = tree_plain(text, 10);
    assert_eq!(
        rows,
        render_utils::wrap(text, 10).iter().map(|l| l.trim_end().to_owned()).collect::<Vec<_>>(),
        "B-4df413ddcf fallback"
    );
    assert!(rows.len() >= 2, "B-4df413ddcf overflow");
    for row in &rows[1..] {
        assert!(!row.starts_with([' ', '│']), "B-4df413ddcf flush");
    }
    let rows = tree_plain(text, 12);
    assert_eq!(rows[0], "├── alpha", "B-4df413ddcf boundary");
    assert!(rows.len() >= 2, "B-4df413ddcf hangs");
    for row in &rows[1..] {
        assert!(row.starts_with("│   ") && row.chars().nth(4) != Some(' '), "B-4df413ddcf hang");
    }
    let mut md = MarkdownComponent::new(
        WireString::from("aaa **bold**\n├── alpha bravo charlie delta echo foxtrot golf hotel india"),
        Arc::new(Theme::dark()),
    );
    md.set_options(MarkdownOptions {
        default_style: DefaultTextStyle {
            color: Some(Arc::new(|s| format!("\x1b[31m{s}\x1b[39m"))),
            ..Default::default()
        },
        ..Default::default()
    });
    let raw = md.render(40);
    let rows = raw.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>();
    assert_eq!(rows.len(), 3, "B-55439c0951");
    assert!(rows[1].starts_with("├── alpha") && rows[2].starts_with("│   foxtrot"), "B-55439c0951 layout");
    let sgr = regex::Regex::new(r"\x1b\[[0-9;:]*m").unwrap();
    assert_eq!(sgr.find_iter(raw[0].trim_end()).last().unwrap().as_str(), "\x1b[31m", "B-55439c0951 row0 reopen");
    let hang = raw[2].find("│   ").unwrap();
    assert!(hang > 0, "B-55439c0951 replay");
    let replay = &raw[2][..hang];
    assert_eq!(sgr.replace_all(replay, ""), "", "B-55439c0951 zero cells");
    assert!(replay.contains("\x1b[1m"), "B-55439c0951 bold history");
    assert_eq!(sgr.find_iter(replay).last().unwrap().as_str(), "\x1b[31m", "B-55439c0951 color carry");
    let doc = "├── alpha bravo charlie delta echo foxtrot golf\n└── hotel india juliet kilo lima mike november";
    let theme = Arc::new(Theme::dark());
    let mut md = MarkdownComponent::new(WireString::from(doc), theme.clone());
    let first = md.render(44);
    assert_eq!(
        first.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>(),
        vec![
            "├── alpha bravo charlie delta echo foxtrot",
            "│   golf",
            "└── hotel india juliet kilo lima mike",
            "    november"
        ],
        "B-b155415277 narrow"
    );
    assert_eq!(
        md.render(80).iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>(),
        vec!["├── alpha bravo charlie delta echo foxtrot golf", "└── hotel india juliet kilo lima mike november"],
        "B-b155415277 wide"
    );
    assert_eq!(*md.render(44), *first, "B-b155415277 roundtrip");
    assert_eq!(*MarkdownComponent::new(WireString::from(doc), theme).render(44), *first, "B-b155415277 cold");
    let raw = tree_raw(
        "aaa \x1b[1mbold\x1b[0m\x1b[3mrest and filler\n├── alpha bravo charlie delta echo foxtrot golf hotel india",
        40,
    );
    let rows = raw.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>();
    assert_eq!(rows.len(), 3, "B-757625241c");
    assert!(rows[1].starts_with("├── alpha") && rows[2].starts_with("│   foxtrot"), "B-757625241c layout");
    for row in &raw[1..] {
        assert!(!row.contains("\x1b[1m") && !row.contains("\x1b[0m"), "B-757625241c dead style");
    }
    assert!(raw[1].starts_with("\x1b[3m├── "), "B-757625241c italic carry");
    let hang = raw[2].find("│   ").unwrap();
    assert!(hang > 0, "B-757625241c hang");
    assert_eq!(&raw[2][..hang], "\x1b[3m", "B-757625241c only live style");
}
