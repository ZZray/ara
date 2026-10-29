//! Fixed 596f2da original display layout, including nonliteral assertions.
use super::*;
fn plain(s: &str) -> String {
    regex::Regex::new(r"\x1b\[[0-9;:]*m").unwrap().replace_all(s, "").into_owned()
}
#[test]
fn original_display_layout_46_ids() {
    let fixtures: serde_json::Value = serde_json::from_str(include_str!("markdown_math_layout_original.json")).unwrap();
    for f in fixtures.as_array().unwrap() {
        let id = f["id"].as_str().unwrap();
        let mut actual = latex_to_block(f["source"].as_str().unwrap());
        if f["strip_ansi"].as_bool().unwrap() {
            actual = actual.iter().map(|l| plain(l)).collect();
        }
        if f["trim_end"].as_bool().unwrap() {
            actual = actual.iter().map(|l| l.trim_end().to_owned()).collect();
        }
        let expected: Vec<String> = if let Some(other) = f["equivalent"].as_str() {
            latex_to_block(other)
        } else {
            f["expected"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect()
        };
        assert_eq!(actual, expected, "{id} source {:?}", f["source"]);
    }
    let lines = latex_to_block(r"\begin{alignat}{2} a &= \frac{1}{2} \end{alignat}");
    assert!(!lines.join("\n").contains("{2}"), "B-163fa8996e preamble");
    assert!(lines.iter().any(|l| l.contains("───")), "B-163fa8996e bar");
    assert_eq!(lines[0].trim(), "1", "B-163fa8996e numerator");
    let lines = latex_to_block(r"\begin{aligned} y &= \frac{1}{2} \\ z &= \frac{3}{4} \end{aligned}");
    assert_eq!(lines.len(), 6, "B-c51b725dbd");
    assert!(
        lines[1].contains("───")
            && lines[4].contains("───")
            && plain(&lines[0]).contains('1')
            && plain(&lines[5]).contains('4'),
        "B-c51b725dbd rows"
    );
    let lines = latex_to_block(r"\frac{\textcolor{red}{a}}{b}");
    assert_eq!(plain(&lines[1]), "───", "B-311f4671ee bar");
    assert_eq!(lines.iter().map(|l| plain(l)).collect::<Vec<_>>(), vec![" a ", "───", " b "], "B-311f4671ee");
    assert!(lines[0].contains('\x1b'), "B-311f4671ee color");
    let lines = latex_to_block(r"f(x) = \begin{cases} x & x > 0 \\ 0 & \text{otherwise} \end{cases}");
    assert_eq!(
        lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>(),
        vec!["       ⎧ x  x > 0", "f(x) = ⎨", "       ⎩ 0  otherwise"],
        "B-6927f50e30"
    );
    let lines = latex_to_block(r"\lim_{x \to 0} \frac{\sin x}{x}");
    assert!(lines[1].contains("lim") && lines[2].contains("x → 0") && lines[1].contains("───"), "B-de86eb6b44");
    let lines = latex_to_block(r"\left\{ x \middle| \frac{x}{2} \in \mathbb{Z} \right\}");
    assert_eq!(lines.len(), 3, "B-4ae8d65104");
    assert!(lines[1].starts_with('⎨') && lines[1].contains('│') && lines[1].ends_with('⎬'), "B-4ae8d65104 delimiters");
    let lines = latex_to_block(r"\color{red} x + \frac{a}{b}");
    assert_eq!(
        lines.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>(),
        vec!["      a", " x + ───", "      b"],
        "B-da15a09051"
    );
    assert!(lines[0].contains("\x1b[38;") && lines[1].contains("\x1b[38;"), "B-da15a09051 color");
    let lines = latex_to_block(r"\textcolor{red}{\frac{a}{b}}");
    assert_eq!(lines.iter().map(|l| plain(l)).collect::<Vec<_>>(), vec![" a ", "───", " b "], "B-f6a4f382e2");
    assert!(lines[1].contains("\x1b[38;"), "B-f6a4f382e2 bar color");
    let lines = latex_to_block(r"\textbf{\frac{a}{b}}");
    assert_eq!(lines.iter().map(|l| plain(l)).collect::<Vec<_>>(), vec![" a ", "───", " b "], "B-ab125ef73b");
    assert!(lines[0].contains("\x1b[1m") && lines[2].contains("\x1b[1m"), "B-ab125ef73b font");
    let lines = latex_to_block(r"\textcolor{red}{\sum_{i=1}^n}");
    assert_eq!(
        lines.iter().map(|l| plain(l).trim_end().to_owned()).collect::<Vec<_>>(),
        vec![" n", " ∑", "i=1"],
        "B-e02d2c382f"
    );
}
#[test]
fn original_math_delimiter_12_ids() {
    for (source, opener, display, end) in [
        ("$x^2$ tail", "$", false, 5),
        ("$$x^2$$ tail", "$$", true, 7),
        (r"\(x^2\) tail", r"\(", false, 7),
        (r"\[x^2\] tail", r"\[", true, 7),
    ] {
        assert_eq!(
            math_span_at(source, 0, 0),
            Some(MathSpan { opener, display, end, body: "x^2".into() }),
            "B-b87c423098"
        );
    }
    assert_eq!(math_span_at("$$\na & b \\\\ c & d\n$$", 0, 0).unwrap().body, "\na & b \\\\ c & d\n", "B-c5615ba810");
    assert_eq!(math_span_at(r"\( x \)", 0, 0).unwrap().body, " x ", "B-c5615ba810");
    for s in ["$20 and $30 total", "$x $", "$ x$", "$x\ny$"] {
        assert!(math_span_at(s, 0, 0).is_none(), "B-3ceff149e0 {s:?}");
    }
    assert_eq!(
        math_span_at(r"$cost = \$20$", 0, 0),
        Some(MathSpan { opener: "$", display: false, end: 13, body: r"cost = \$20".into() }),
        "B-3ceff149e0 escaped currency"
    );
    for s in ["$unfinished", r"\(unfinished", r"\[unfinished", "$$ $$"] {
        assert!(math_span_at(s, 0, 0).is_none(), "B-02883bb298 {s:?}");
    }
    assert_eq!(math_span_at(r"\(\)", 0, 0).unwrap().end, 4, "B-02883bb298 empty unambiguous");
    for (s, at) in [(r"\$x$", 1), (r"\\(x\)", 1)] {
        assert!(math_span_at(s, at, 0).is_none(), "B-be08fe9550");
    }
    for (s, at, from) in [(r"\\$x$", 2, 0), (r"\\\(x\)", 2, 0), (r"\$x$", 1, 1)] {
        assert_eq!(math_span_at(s, at, from).unwrap().body, "x", "B-be08fe9550 live opener");
    }
    assert_eq!(
        math_span_at(r"\(a \\) b\)", 0, 0),
        Some(MathSpan { opener: r"\(", display: false, end: 11, body: r"a \\) b".into() }),
        "B-de1192d86f"
    );
    assert_eq!(math_span_at(r"\[a \\] b\]", 0, 0).unwrap().body, r"a \\] b", "B-de1192d86f bracket");
    assert_eq!(math_span_at(r"$$a \$$ b$$", 0, 0).unwrap().body, r"a \$$ b", "B-de1192d86f dollar");
    assert!(math_span_at(r"$$\$$", 0, 0).is_none(), "B-de1192d86f unclosed");
    assert_eq!(math_span_at(r"\(a\\\)", 0, 0).unwrap().body, r"a\\", "B-de1192d86f even");
    for (s, expected) in
        [("$$x$$", Some("$$")), ("$x$", Some("$")), (r"\[x\]", Some(r"\[")), ("x$", None), (r"\frac{1}{2}", None)]
    {
        assert_eq!(math_opener_at(s, 0), expected, "B-a08078f417");
    }
    for (s, from, expected) in [
        ("no math here", 0, None),
        (r"prose \(x\) and $y$", 0, Some(6)),
        (r"prose \(x\) and $y$", 7, Some(16)),
        (r"cost \$20", 0, Some(6)),
    ] {
        assert_eq!(math_start_index(s, from), expected, "B-3c4bc2900f");
    }
    let source = "  $$\n\\begin{pmatrix}a & b \\\\ c & d\\end{pmatrix}\n  $$\nnext";
    assert_eq!(
        math_block_at(source),
        Some(MathBlock {
            raw: "  $$\n\\begin{pmatrix}a & b \\\\ c & d\\end{pmatrix}\n  $$\n".into(),
            body: r"\begin{pmatrix}a & b \\ c & d\end{pmatrix}".into()
        }),
        "B-39d65b1e19"
    );
    assert_eq!(math_block_at("$$\na\n\nb\n$$\n").unwrap().body, "a\n\nb", "B-28e10864e5");
    for s in ["$$\r\nx\r\n$$\r\n", "\\[\r\nx\r\n\\]\r\n"] {
        assert_eq!(math_block_at(s), Some(MathBlock { raw: s.into(), body: "x".into() }), "B-c2d81f4e99");
    }
    for s in ["$$\nunclosed\n", "$$\n \n$$\n", "$$ x^2 $$\n", "    $$\nx\n    $$\n"] {
        assert!(math_block_at(s).is_none(), "B-0be0d274fb {s:?}");
    }
}
#[test]
fn original_latex_unicode_22_ids() {
    let red = "\x1b[38;2;255;0;0m";
    let blue = "\x1b[38;2;0;0;255m";
    let yellow = "\x1b[48;2;255;255;0m";
    let reset = "\x1b[39m";
    for (s, c) in [
        (r"\textcolor{red}{x}", red),
        (r"\textcolor[HTML]{C5FFD6}{x}", "\x1b[38;2;197;255;214m"),
        (r"\textcolor[RGB]{128,64,32}{x}", "\x1b[38;2;128;64;32m"),
        (r"\textcolor{red!50!blue}{x}", "\x1b[38;2;128;0;128m"),
    ] {
        assert_eq!(latex_to_unicode(s), format!("{c}x{reset}"), "B-7d80f35fe5 {s}");
    }
    assert_eq!(latex_to_unicode(r"{\color{red}x}y"), format!("{red}x{reset}y"), "B-c9b1339cbe");
    assert_eq!(
        latex_to_unicode(r"{\color{red}a {\color{blue}b} c} d"),
        format!("{red}a {blue}b{red} c{reset} d"),
        "B-c9b1339cbe nested"
    );
    assert_eq!(latex_to_unicode(r"\colorbox{yellow}{x}"), format!("{yellow}x\x1b[49m"), "B-8700baafc5");
    assert_eq!(
        latex_to_unicode(r"\fcolorbox{red}{yellow}{x}"),
        format!("{red}[{reset}{yellow}x\x1b[49m{red}]{reset}"),
        "B-8700baafc5 frame"
    );
    // Independent oracle: Bun 1.4.0 executable SHA 627d2e4775c24bdedee2cd7ccc18dcadae061e5345274ab6e3c4c797927bfb8f;
    // Root executed Bun.color('#C5FFD6','ansi-256'), yielding ESC[38;5;194m.
    assert_eq!(
        latex_to_unicode_with_color(r"\textcolor{#C5FFD6}{x}", false),
        "\x1b[38;5;194mx\x1b[39m",
        "B-7c0a84a695"
    );
    let out = render_math_in_text(r"\textcolor{red}{alert}");
    assert_eq!(plain(&out), "alert", "B-b8b85f543c");
    assert!(out.contains(red), "B-b8b85f543c color");
    let out = latex_to_unicode(r"\textcolor{red}{\textbf{strongest}}");
    assert_eq!(plain(&out), "strongest", "B-cb3201da36");
    assert!(out.contains(red) && out.contains("\x1b[1m"), "B-cb3201da36 color/style");
    assert!(!out.chars().any(|c| ('\u{1d400}'..='\u{1d7ff}').contains(&c)), "B-cb3201da36 no glyph transform");
    for (cmd, on, off) in [("textbf", 1, 22), ("textit", 3, 23), ("textsl", 3, 23), ("emph", 3, 23)] {
        assert_eq!(latex_to_unicode(&format!("\\{cmd}{{x}}")), format!("\x1b[{on}mx\x1b[{off}m"), "B-0b38fd9fc1 {cmd}");
    }
    let out = latex_to_unicode(r"\textbf{outer \textbf{inner} outer}");
    assert_eq!(plain(&out), "outer inner outer", "B-5baba87174");
    assert!(out.contains("\x1b[22m\x1b[1m outer"), "B-5baba87174 reopen");
    for (s, e) in [
        (r"\hat{\textbf{x}}", "\x1b[1mx̂\x1b[22m"),
        (r"\frac{\textbf{a}}{b}", "\x1b[1ma\x1b[22m/b"),
        (r"\sqrt{\textbf{x}}", "√\x1b[1mx\x1b[22m"),
    ] {
        assert_eq!(latex_to_unicode(s), e, "B-2c4e734917");
    }
    assert_eq!(latex_to_unicode(r"\boxed{\textcolor{red}{x}}"), format!("[{red}x{reset}]"), "B-12af4197ac");
    assert_eq!(latex_to_unicode(r"\pmod{\textbf{x}}"), "(mod \x1b[1mx\x1b[22m)", "B-12af4197ac mod");
    assert_eq!(latex_to_unicode(r"x_{\textit{word}}"), "x_(\x1b[3mword\x1b[23m)", "B-a9660d31c9");
    assert_eq!(latex_to_unicode(r"x^{\textbf{foo!}}"), "x^(\x1b[1mfoo!\x1b[22m)", "B-a9660d31c9 power");
    let bold = latex_to_unicode(r"\textbf{A\textmd{B}C}");
    let italic = latex_to_unicode(r"\textit{A\textup{B}C}");
    assert_eq!(plain(&bold), "ABC", "B-3fbc459e7f");
    assert_eq!(plain(&italic), "ABC", "B-3fbc459e7f");
    assert_eq!(bold, "\x1b[1mA\x1b[22mB\x1b[1mC\x1b[22m", "B-3fbc459e7f bold");
    assert_eq!(italic, "\x1b[3mA\x1b[23mB\x1b[3mC\x1b[23m", "B-3fbc459e7f italic");
    assert_eq!(latex_to_unicode(r"\texttt{x}\textsf{y}"), "xy", "B-f507dad3f3");
    for env in ["cases", "align", "align*", "pmatrix", "array", "gather", "equation*", "aligned"] {
        assert!(is_bare_math_environment(env), "B-333834bafb {env}");
    }
    for env in ["tabular", "tabular*", "itemize", "enumerate", "verbatim", "document", "figure"] {
        assert!(!is_bare_math_environment(env), "B-a00715fa58 {env}");
    }
    assert_eq!(latex_to_unicode(r"\subseteqq"), "⫅", "B-97fb2d2e5d");
    assert_eq!(latex_to_unicode(r"\supseteqq"), "⫆", "B-97fb2d2e5d");
    assert_eq!(latex_to_unicode(r"\frac{1}{2}"), "½", "B-463c268223");
    let out = render_math_in_text("\\begin{align}\na &= b + c \\\\\nx &= y\n\\end{align}");
    assert!(!out.contains(r"\begin{align}") && out.contains('='), "B-0af22d6b25");
    let out = render_math_in_text("f(x) =\n\\begin{cases}\n1 & x > 0 \\\\\n0 & x \\le 0\n\\end{cases}");
    assert!(!out.contains(r"\begin{cases}") && out.contains("f(x) =") && !out.contains("f(x) =\n"), "B-9db6d0d451");
    let verbatim = "\\begin{verbatim}\n\\frac{a}{b}\n\\end{verbatim}";
    assert_eq!(render_math_in_text(verbatim), verbatim, "B-4f024c93d3");
    let list = "Here:\n\\begin{itemize}\n\\item first\n\\item second\n\\end{itemize}\nDone.";
    assert_eq!(render_math_in_text(list), list, "B-2669482e79");
    assert_eq!(render_math_in_text("I paid \\$5 for it."), "I paid $5 for it.", "B-ebda9befd1");
}
