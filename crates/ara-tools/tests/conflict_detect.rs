//! Original conflict-detect.test.ts 56 contracts, fixed OMP 596f2da.
use ara_tools::conflict_detect::*;
use std::path::PathBuf;
fn entry() -> ConflictEntry {
    ConflictEntry {
        id: 1,
        absolute_path: PathBuf::from("/abs/a.ts"),
        display_path: "a.ts".into(),
        block: ConflictBlock {
            start_line: 2,
            separator_line: 4,
            end_line: 6,
            ours_lines: vec!["o".into()],
            theirs_lines: vec!["t".into()],
            ..Default::default()
        },
    }
}
fn strings(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|s| (*s).into()).collect()
}
fn two() -> (String, ConflictEntry) {
    let text = "before\n<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> feat\nafter\n".into();
    let mut e = entry();
    e.block.ours_label = Some("HEAD".into());
    e.block.theirs_label = Some("feat".into());
    e.block.ours_lines = strings(&["ours"]);
    e.block.theirs_lines = strings(&["theirs"]);
    (text, e)
}
fn body_fixture() -> (String, ConflictEntry) {
    let lines = [
        "function nextDelay(a) {",
        "<<<<<<< HEAD",
        "\tconst delay = BASE * 2 ** a;",
        "\treturn Math.min(delay, 10_000);",
        "=======",
        "\tconst d = B * 2 ** a;",
        "\treturn Math.min(d, 30_000);",
        ">>>>>>> tune",
        "}",
        "",
    ];
    let mut e = entry();
    e.block.separator_line = 5;
    e.block.end_line = 8;
    e.block.ours_label = Some("HEAD".into());
    e.block.theirs_label = Some("tune".into());
    e.block.ours_lines = strings(&lines[2..4]);
    e.block.theirs_lines = strings(&lines[5..7]);
    (lines.join("\n"), e)
}
fn run(line: u32) {
    match line {
        15 => {
            let b = scan_conflict_lines(
                &[
                    "line A",
                    "<<<<<<< HEAD",
                    "ours one",
                    "ours two",
                    "=======",
                    "theirs one",
                    ">>>>>>> feature/x",
                    "line Z",
                ],
                1,
            );
            assert_eq!(b.len(), 1);
            let b = &b[0];
            assert_eq!((b.start_line, b.separator_line, b.end_line), (2, 5, 7));
            assert_eq!(b.base_line, None);
            assert_eq!(b.ours_label.as_deref(), Some("HEAD"));
            assert_eq!(b.theirs_label.as_deref(), Some("feature/x"));
            assert_eq!(b.ours_lines, strings(&["ours one", "ours two"]));
            assert_eq!(b.theirs_lines, strings(&["theirs one"]));
        }
        39 => {
            let b = scan_conflict_lines(
                &[
                    "<<<<<<< HEAD",
                    "ours",
                    "||||||| merged common ancestor",
                    "base",
                    "=======",
                    "theirs",
                    ">>>>>>> branch",
                ],
                1,
            );
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].base_line, Some(3));
            assert_eq!(b[0].base_label.as_deref(), Some("merged common ancestor"));
            assert_eq!(b[0].base_lines, Some(strings(&["base"])));
            assert_eq!(b[0].ours_lines, strings(&["ours"]));
            assert_eq!(b[0].theirs_lines, strings(&["theirs"]));
        }
        52 => {
            let b = scan_conflict_lines(&["<<<<<<<", "o", "=======", "t", ">>>>>>>"], 100);
            assert_eq!(b.len(), 1);
            assert_eq!((b[0].start_line, b[0].separator_line, b[0].end_line), (100, 102, 104));
        }
        60 => {
            let b = scan_conflict_lines(
                &[
                    "<<<<<<< A",
                    "o1",
                    "=======",
                    "t1",
                    ">>>>>>> A",
                    "middle",
                    "<<<<<<< B",
                    "o2",
                    "=======",
                    "t2",
                    ">>>>>>> B",
                ],
                1,
            );
            assert_eq!(b.iter().map(|b| b.ours_label.as_deref()).collect::<Vec<_>>(), vec![Some("A"), Some("B")]);
        }
        68 => assert!(scan_conflict_lines(&["<<<<<<< HEAD", "ours", "=======", "theirs"], 1).is_empty()),
        73 => assert!(
            scan_conflict_lines(
                &[" <<<<<<< HEAD", " =======", " >>>>>>> branch", "<<<<<<<x", "========", ">>>>>>>x", "const a = 1;"],
                1
            )
            .is_empty()
        ),
        81 => {
            let b = scan_conflict_lines(&["<<<<<<<", "ours", "=======", "theirs", ">>>>>>>"], 1);
            assert_eq!(b.len(), 1);
            assert!(b[0].ours_label.is_none());
            assert!(b[0].theirs_label.is_none());
        }
        88 => {
            let b = scan_conflict_lines(
                &[
                    "<<<<<<< first",
                    "stale ours",
                    "<<<<<<< second",
                    "good ours",
                    "=======",
                    "good theirs",
                    ">>>>>>> end",
                ],
                1,
            );
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].ours_label.as_deref(), Some("second"));
            assert_eq!(b[0].ours_lines, strings(&["good ours"]));
        }
        98 => {
            let b = scan_conflict_lines(&["<<<<<<< HEAD\r", "ours\r", "=======\r", "theirs\r", ">>>>>>> feat\r"], 1);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].ours_label.as_deref(), Some("HEAD"));
            assert_eq!(b[0].theirs_label.as_deref(), Some("feat"));
            assert_eq!(b[0].ours_lines, strings(&["ours"]));
            assert_eq!(b[0].theirs_lines, strings(&["theirs"]));
        }
        109 => {
            let mut h = ConflictHistory::default();
            let mut e = entry();
            e.block.start_line = 10;
            e.block.separator_line = 12;
            e.block.end_line = 14;
            let a = h.register(e.absolute_path.clone(), e.display_path.clone(), e.block);
            let mut e = entry();
            e.absolute_path = "/abs/b.ts".into();
            e.display_path = "b.ts".into();
            e.block.start_line = 1;
            e.block.separator_line = 3;
            e.block.end_line = 5;
            e.block.ours_lines = strings(&["o2"]);
            e.block.theirs_lines = strings(&["t2"]);
            let b = h.register(e.absolute_path, e.display_path, e.block);
            assert_eq!((a.id, b.id), (1, 2));
            assert_eq!(h.get(1.0).unwrap().absolute_path, PathBuf::from("/abs/a.ts"));
            assert_eq!(h.get(2.0).unwrap().absolute_path, PathBuf::from("/abs/b.ts"));
            assert!(h.get(99.0).is_none());
        }
        136 => {
            let mut h = ConflictHistory::default();
            let mut e = entry();
            e.block.start_line = 10;
            e.block.separator_line = 12;
            e.block.end_line = 14;
            e.block.ours_lines = strings(&["old-ours"]);
            e.block.theirs_lines = strings(&["old-theirs"]);
            let a = h.register(e.absolute_path.clone(), e.display_path.clone(), e.block.clone());
            e.block.end_line = 16;
            e.block.ours_lines = strings(&["new-ours-1", "new-ours-2", "new-ours-3"]);
            e.block.theirs_lines = strings(&["new-theirs"]);
            let b = h.register(e.absolute_path, e.display_path, e.block);
            assert_eq!(a.id, b.id);
            assert_eq!(h.get(a.id as f64).unwrap().block.end_line, 16);
            assert_eq!(
                h.get(a.id as f64).unwrap().block.ours_lines,
                strings(&["new-ours-1", "new-ours-2", "new-ours-3"])
            );
        }
        161 => {
            let mut h = ConflictHistory::default();
            h.register("/abs/a.ts".into(), "a.ts".into(), entry().block);
            h.register("/abs/b.ts".into(), "b.ts".into(), entry().block);
            h.invalidate_path(&PathBuf::from("/abs/a.ts"));
            assert!(h.get(1.0).is_none());
            assert!(h.get(2.0).is_some());
        }
        188 => {
            for id in [1.0, 42.0] {
                assert_eq!(
                    parse_conflict_uri(&format!("conflict://{id}")).unwrap().unwrap(),
                    ParsedConflictUri { id: ConflictId::Number(id), scope: None, recovered_prefix: None }
                );
            }
        }
        193 => {
            for raw in ["src/foo.ts", "file:///abs/path", "conflict://"] {
                assert!(parse_conflict_uri(raw).unwrap().is_none());
            }
        }
        199 => {
            for (id, scope) in [(1.0, ConflictScope::Ours), (2.0, ConflictScope::Theirs), (3.0, ConflictScope::Base)] {
                let p = parse_conflict_uri(&format!("conflict://{id}/{}", scope.as_str())).unwrap().unwrap();
                assert_eq!(p.id, ConflictId::Number(id));
                assert_eq!(p.scope, Some(scope));
            }
        }
        205 => {
            for scope in ["both", "extras"] {
                assert!(
                    parse_conflict_uri(&format!("conflict://1/{scope}"))
                        .unwrap_err()
                        .0
                        .contains("scope must be one of")
                );
            }
        }
        210 => assert_eq!(
            parse_conflict_uri("conflict://*").unwrap().unwrap(),
            ParsedConflictUri { id: ConflictId::Wildcard, scope: None, recovered_prefix: None }
        ),
        214 => assert!(parse_conflict_uri("conflict://*/ours").unwrap_err().0.contains("wildcard")),
        218 => {
            for tail in ["0", "-1", "1.5", "abc", "1/extra"] {
                assert!(parse_conflict_uri(&format!("conflict://{tail}")).is_err());
            }
        }
        226 => {
            assert_eq!(
                parse_conflict_uri("src/foo.ts:conflict://3").unwrap().unwrap(),
                ParsedConflictUri {
                    id: ConflictId::Number(3.0),
                    scope: None,
                    recovered_prefix: Some("src/foo.ts".into())
                }
            );
            assert_eq!(
                parse_conflict_uri("packages/coding-agent/src/x.ts:conflict://*").unwrap().unwrap(),
                ParsedConflictUri {
                    id: ConflictId::Wildcard,
                    scope: None,
                    recovered_prefix: Some("packages/coding-agent/src/x.ts".into())
                }
            );
            assert_eq!(
                parse_conflict_uri("a.ts:conflict://2/theirs").unwrap().unwrap(),
                ParsedConflictUri {
                    id: ConflictId::Number(2.0),
                    scope: Some(ConflictScope::Theirs),
                    recovered_prefix: Some("a.ts".into())
                }
            );
        }
        242 => {
            for raw in ["conflict://1", "conflict://*"] {
                assert!(parse_conflict_uri(raw).unwrap().unwrap().recovered_prefix.is_none());
            }
        }
        274 | 279 | 284 | 289 | 295 | 300 | 304 | 312 => {
            let (file, e) = two();
            match line {
                274 => assert_eq!(splice_conflict(&file, &e, "resolved\n").unwrap().text, "before\nresolved\nafter\n"),
                279 => assert_eq!(
                    splice_conflict(&file, &e, "alpha\nbeta\n").unwrap().text,
                    "before\nalpha\nbeta\nafter\n"
                ),
                284 => assert_eq!(splice_conflict(&file, &e, "").unwrap().text, "before\n\nafter\n"),
                289 => assert_eq!(
                    splice_conflict(&format!("// new comment 1\n// new comment 2\n{file}"), &e, "resolved\n")
                        .unwrap()
                        .text,
                    "// new comment 1\n// new comment 2\nbefore\nresolved\nafter\n"
                ),
                295 => assert!(
                    splice_conflict("before\n// resolved by hand\nafter\n", &e, "x\n")
                        .unwrap_err()
                        .0
                        .contains("no longer present")
                ),
                300 => assert!(splice_conflict("short\n", &e, "x\n").unwrap_err().0.contains("no longer present")),
                304 => assert_eq!(
                    splice_conflict(&file.replace('\n', "\r\n"), &e, "alpha\nbeta\n").unwrap().text,
                    "before\r\nalpha\r\nbeta\r\nafter\r\n"
                ),
                312 => assert_eq!(
                    splice_conflict(
                        "before\r\n<<<<<<< HEAD\r\nours\r\n=======\r\ntheirs\r\n>>>>>>> feat",
                        &e,
                        "resolved"
                    )
                    .unwrap()
                    .text,
                    "before\r\nresolved"
                ),
                _ => unreachable!(),
            }
        }
        350 => {
            let lines = [
                "const queue = [];",
                "<<<<<<< HEAD",
                "export function scheduleTask(task, priority = 0) {",
                "\tif (dupe(task)) {",
                "\t\treturn;",
                "\t}",
                "=======",
                "export function enqueueTask(task) {",
                "\tif (queued.has(task.id)) {",
                "\t\treturn;",
                "\t}",
                ">>>>>>> feature",
                "\tqueue.push(task);",
                "}",
                "",
            ];
            let mut e = entry();
            e.block.separator_line = 7;
            e.block.end_line = 12;
            e.block.ours_label = Some("HEAD".into());
            e.block.theirs_label = Some("feature".into());
            e.block.ours_lines = strings(&lines[2..6]);
            e.block.theirs_lines = strings(&lines[7..11]);
            let replacement = [
                "export function scheduleTask(task, priority = 0) {",
                "\tif (queued.has(task.id)) {",
                "\t\treturn;",
                "\t}",
                "\tqueue.push(task);",
                "}",
            ]
            .join("\n");
            let r = splice_conflict(&lines.join("\n"), &e, &replacement).unwrap();
            assert_eq!((r.trimmed_trailing, r.trimmed_leading), (2, 0));
            assert_eq!(
                r.text,
                [
                    "const queue = [];",
                    "export function scheduleTask(task, priority = 0) {",
                    "\tif (queued.has(task.id)) {",
                    "\t\treturn;",
                    "\t}",
                    "\tqueue.push(task);",
                    "}",
                    ""
                ]
                .join("\n")
            );
        }
        399 | 456 => {
            let (file, e) = body_fixture();
            let replacement = "\tconst delay = BASE * 2 ** a;\n\treturn Math.min(delay, 30_000);\n}";
            let (file, ending) = if line == 456 { (file.replace('\n', "\r\n"), "\r\n") } else { (file, "\n") };
            let r = splice_conflict(&file, &e, replacement).unwrap();
            assert_eq!(r.trimmed_trailing, 1);
            assert_eq!(
                r.text,
                [
                    "function nextDelay(a) {",
                    "\tconst delay = BASE * 2 ** a;",
                    "\treturn Math.min(delay, 30_000);",
                    "}",
                    ""
                ]
                .join(ending)
            );
        }
        414 => {
            let mut e = entry();
            e.block.ours_label = Some("HEAD".into());
            e.block.theirs_label = Some("x".into());
            e.block.ours_lines = strings(&["a"]);
            e.block.theirs_lines = strings(&["b"]);
            let r = splice_conflict("start\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\ndone();\n", &e, "merged\ndone();")
                .unwrap();
            assert_eq!(r.trimmed_trailing, 0);
            assert_eq!(r.text, "start\nmerged\ndone();\ndone();\n");
        }
        430 => {
            let mut e = entry();
            e.block.start_line = 3;
            e.block.separator_line = 5;
            e.block.end_line = 7;
            e.block.ours_label = Some("HEAD".into());
            e.block.theirs_label = Some("x".into());
            e.block.ours_lines = strings(&["a"]);
            e.block.theirs_lines = strings(&["b"]);
            let r = splice_conflict(
                "// header\nconst queue = [];\n<<<<<<< HEAD\na\n=======\nb\n>>>>>>> x\ntail\n",
                &e,
                "// header\nconst queue = [];\nmerged",
            )
            .unwrap();
            assert_eq!(r.trimmed_leading, 2);
            assert_eq!(r.text, "// header\nconst queue = [];\nmerged\ntail\n");
        }
        496 | 502 | 516 | 531 | 537 | 543 | 549 => {
            let mut e = entry();
            e.block.start_line = 10;
            e.block.separator_line = 13;
            e.block.end_line = 15;
            e.block.ours_label = Some("HEAD".into());
            e.block.theirs_label = Some("feature/x".into());
            e.block.ours_lines = strings(&["ours-1", "ours-2"]);
            e.block.theirs_lines = strings(&["theirs-1"]);
            if line == 502 || line == 543 {
                e.block = ConflictBlock {
                    start_line: 20,
                    base_line: Some(22),
                    separator_line: 24,
                    end_line: 26,
                    ours_label: Some("HEAD".into()),
                    base_label: Some("common ancestor".into()),
                    theirs_label: Some("feat".into()),
                    ours_lines: strings(&["o"]),
                    base_lines: Some(strings(&["b"])),
                    theirs_lines: strings(&["t"]),
                };
            }
            match line {
                496 => assert_eq!(
                    render_conflict_region(&e, None).unwrap(),
                    (strings(&["<<<<<<< HEAD", "ours-1", "ours-2", "=======", "theirs-1", ">>>>>>> feature/x"]), 10)
                ),
                502 => assert_eq!(
                    render_conflict_region(&e, None).unwrap(),
                    (
                        strings(&["<<<<<<< HEAD", "o", "||||||| common ancestor", "b", "=======", "t", ">>>>>>> feat"]),
                        20
                    )
                ),
                516 => {
                    e.block.ours_label = None;
                    e.block.theirs_label = None;
                    let (rows, _) = render_conflict_region(&e, None).unwrap();
                    assert_eq!(rows.first().unwrap(), "<<<<<<<");
                    assert_eq!(rows.last().unwrap(), ">>>>>>>");
                }
                531 => assert_eq!(
                    render_conflict_region(&e, Some(ConflictScope::Ours)).unwrap(),
                    (strings(&["ours-1", "ours-2"]), 11)
                ),
                537 => assert_eq!(
                    render_conflict_region(&e, Some(ConflictScope::Theirs)).unwrap(),
                    (strings(&["theirs-1"]), 14)
                ),
                543 => {
                    assert_eq!(render_conflict_region(&e, Some(ConflictScope::Base)).unwrap(), (strings(&["b"]), 23))
                }
                549 => assert!(
                    render_conflict_region(&e, Some(ConflictScope::Base)).unwrap_err().0.contains("no base section")
                ),
                _ => unreachable!(),
            }
        }
        555 => assert_eq!(format_conflict_warning(&[], WarningOptions::default()), ""),
        559 => {
            let mut e = entry();
            e.id = 7;
            e.block.start_line = 12;
            e.block.separator_line = 14;
            e.block.end_line = 16;
            e.block.ours_label = Some("HEAD".into());
            e.block.theirs_label = Some("feature/x".into());
            e.block.ours_lines = strings(&["a", "b"]);
            e.block.theirs_lines = strings(&["c"]);
            let t = format_conflict_warning(&[e], WarningOptions::default());
            for needle in [
                "⚠ 1 unresolved conflict detected",
                "- ours = HEAD",
                "- theirs = feature/x",
                "──── #7  L12-16 ────",
                "<<< ours",
                "\na\n",
                "\nb\n",
                ">>> theirs",
                "\nc",
                "NOTICE: Inspect a block by reading `conflict://<N>`",
                "`write({ path: \"conflict://<N>\", content })`",
                "`write({ path: \"conflict://*\", content })`",
                "@ours",
                "@theirs",
            ] {
                assert!(t.contains(needle), "{needle}: {t}");
            }
            for needle in ["write({ path: \"conflict://7\"", "--- ours", "[conflict #7]"] {
                assert!(!t.contains(needle));
            }
        }
        592 => {
            let a = entry();
            let mut b = entry();
            b.id = 2;
            b.block.start_line = 20;
            b.block.separator_line = 22;
            b.block.end_line = 24;
            let t = format_conflict_warning(&[a, b], WarningOptions::default());
            for needle in ["⚠ 2 unresolved conflicts detected", "──── #1  L2-6 ────", "──── #2  L20-24 ────"]
            {
                assert!(t.contains(needle));
            }
        }
        601 | 619 | 630 => {
            let mut e = entry();
            e.id = 3;
            e.block.base_lines = Some(strings(&[if line == 601 {
                "o"
            } else if line == 619 {
                "t"
            } else {
                "b"
            }]));
            e.block.base_label = Some(if line == 630 { "common ancestor" } else { "ancestor" }.into());
            let t = format_conflict_warning(&[e], WarningOptions::default());
            match line {
                601 => {
                    let a = t.find("=== base ≡ ours").unwrap();
                    let b = t.find(">>> theirs").unwrap();
                    assert!(b > a);
                    assert_eq!(t[a + "=== base ≡ ours".len()..b].trim(), "");
                }
                619 => assert!(t.contains("=== base ≡ theirs")),
                630 => {
                    assert!(t.contains("- base = common ancestor"));
                    assert!(t.contains("=== base"));
                    assert!(!t.contains("=== base ≡"));
                    assert!(t.contains("\nb\n"));
                }
                _ => unreachable!(),
            }
        }
        645 => {
            let t = format_conflict_warning(&[entry()], WarningOptions::default());
            assert!(!t.contains("- ours ="));
            assert!(!t.contains("- theirs ="));
        }
        652 => {
            let mut e = entry();
            e.id = 6;
            e.block.ours_lines = (0..20).map(|n| format!("o{n}")).collect();
            let t = format_conflict_warning(&[e], WarningOptions::default());
            assert!(t.contains("\no0\n"));
            assert!(t.contains("\no5\n"));
            assert!(t.contains("… (14 more lines)"));
            assert!(!t.contains("\no6\n"));
        }
        671 | 675 | 679 | 684 | 688 | 694 | 698 => {
            let mut e = entry();
            e.block.ours_lines = strings(&["o1", "o2"]);
            e.block.theirs_lines = strings(&["t1"]);
            match line {
                671 => assert_eq!(expand_content_tokens("hand-written\nline\n", &e).unwrap(), "hand-written\nline\n"),
                675 => assert_eq!(expand_content_tokens("@ours", &e).unwrap(), "o1\no2"),
                679 => {
                    assert_eq!(expand_content_tokens("@theirs", &e).unwrap(), "t1");
                    assert_eq!(expand_content_tokens("@both", &e).unwrap(), "o1\no2\nt1");
                }
                684 => assert_eq!(
                    expand_content_tokens("// keep both\n@ours\n@theirs", &e).unwrap(),
                    "// keep both\no1\no2\nt1"
                ),
                688 => {
                    assert!(expand_content_tokens("@base", &e).is_err());
                    e.block.base_lines = Some(strings(&["b1"]));
                    e.block.ours_lines = strings(&["o"]);
                    e.block.theirs_lines = strings(&["t"]);
                    assert_eq!(expand_content_tokens("@base", &e).unwrap(), "b1");
                }
                694 => assert_eq!(expand_content_tokens("const x = '@ours';", &e).unwrap(), "const x = '@ours';"),
                698 => assert_eq!(expand_content_tokens("@ours\r\n@theirs", &e).unwrap(), "o1\no2\nt1"),
                _ => unreachable!(),
            }
        }
        _ => panic!("Unmapped upstream original line {line}"),
    }
}
// scanConflictLines > detects a 2-way conflict with correct line numbers and labels
#[test]
fn original_B_f279b87fe5() {
    run(15);
}
// scanConflictLines > detects a 3-way diff3 conflict with base section
#[test]
fn original_B_8111ebc4b1() {
    run(39);
}
// scanConflictLines > offsets line numbers by firstLineNumber
#[test]
fn original_B_2c017e3bf1() {
    run(52);
}
// scanConflictLines > returns multiple blocks in file order
#[test]
fn original_B_a822eb00c3() {
    run(60);
}
// scanConflictLines > ignores unclosed openers
#[test]
fn original_B_456090fb45() {
    run(68);
}
// scanConflictLines > ignores mis-shaped or indented marker lookalikes
#[test]
fn original_B_3752470afa() {
    run(73);
}
// scanConflictLines > accepts label-less markers
#[test]
fn original_B_29e85a9770() {
    run(81);
}
// scanConflictLines > treats a re-opened `<<<<<<<` as a fresh block
#[test]
fn original_B_ed8b5e9979() {
    run(88);
}
// scanConflictLines > detects conflicts in CRLF files and stores LF-normalized sections
#[test]
fn original_B_fe85a3fc01() {
    run(98);
}
// ConflictHistory > assigns monotonic ids and looks entries up by id
#[test]
fn original_B_d45b29e8e5() {
    run(109);
}
// ConflictHistory > dedupes registration by absolutePath+startLine and refreshes recorded body
#[test]
fn original_B_26bf0465e5() {
    run(136);
}
// ConflictHistory > invalidatePath drops entries scoped to one absolutePath
#[test]
fn original_B_bf7ebbdc15() {
    run(161);
}
// parseConflictUri > parses well-formed URIs
#[test]
fn original_B_8ae0f8a459() {
    run(188);
}
// parseConflictUri > returns null for non-conflict paths
#[test]
fn original_B_2121971a86() {
    run(193);
}
// parseConflictUri > parses an optional scope segment
#[test]
fn original_B_82eb8d86dd() {
    run(199);
}
// parseConflictUri > rejects unknown scope tokens
#[test]
fn original_B_5ea80793b2() {
    run(205);
}
// parseConflictUri > parses the bulk wildcard `conflict://*`
#[test]
fn original_B_0ccd6cbedf() {
    run(210);
}
// parseConflictUri > rejects a scope segment on the wildcard
#[test]
fn original_B_1496753449() {
    run(214);
}
// parseConflictUri > rejects malformed ids with a ToolError
#[test]
fn original_B_0ad77c4a2b() {
    run(218);
}
// parseConflictUri > recovers an erroneous `<file>:` prefix and surfaces it as `recoveredPrefix`
#[test]
fn original_B_8af179077b() {
    run(226);
}
// parseConflictUri > does not set `recoveredPrefix` on clean URIs
#[test]
fn original_B_a68a28a4c1() {
    run(242);
}
// spliceConflict > replaces the marker region with the chosen content
#[test]
fn original_B_74496369a5() {
    run(274);
}
// spliceConflict > accepts multi-line replacement
#[test]
fn original_B_1b641727af() {
    run(279);
}
// spliceConflict > accepts empty replacement
#[test]
fn original_B_b9325c6534() {
    run(284);
}
// spliceConflict > relocates the block when earlier lines have been added (line numbers shift)
#[test]
fn original_B_dd2a680865() {
    run(289);
}
// spliceConflict > rejects when the recorded marker block has been edited away
#[test]
fn original_B_e22fd48f2e() {
    run(295);
}
// spliceConflict > rejects when the file is shorter than the recorded region
#[test]
fn original_B_3f1fdb9830() {
    run(300);
}
// spliceConflict > splices CRLF files and preserves CRLF line endings
#[test]
fn original_B_8767bf571b() {
    run(304);
}
// spliceConflict > does not append \\r when the spliced region ends the file without a trailing newline
#[test]
fn original_B_95693fc908() {
    run(312);
}
// spliceConflict boundary-echo repair > drops a multi-line trailing echo of the context below the region
#[test]
fn original_B_f0b5846ef3() {
    run(350);
}
// spliceConflict boundary-echo repair > drops a single-line echo when it fixes the region's delimiter balance
#[test]
fn original_B_7c4b74d33c() {
    run(399);
}
// spliceConflict boundary-echo repair > keeps a single-line echo when the delimiter balance is already consistent
#[test]
fn original_B_7d0725a8ec() {
    run(414);
}
// spliceConflict boundary-echo repair > drops a multi-line leading echo of the context above the region
#[test]
fn original_B_04ea0264f4() {
    run(430);
}
// spliceConflict boundary-echo repair > repairs echoes in CRLF files without breaking EOL round-trip
#[test]
fn original_B_613a55bed0() {
    run(456);
}
// renderConflictRegion > returns full block with marker lines reconstructed from labels
#[test]
fn original_B_1be285abd6() {
    run(496);
}
// renderConflictRegion > includes the base section in a diff3 full block
#[test]
fn original_B_6255975a3b() {
    run(502);
}
// renderConflictRegion > omits the label when none was recorded
#[test]
fn original_B_293dc19e0b() {
    run(516);
}
// renderConflictRegion > returns just the ours body with the line number after `<<<<<<<`
#[test]
fn original_B_142667d581() {
    run(531);
}
// renderConflictRegion > returns just the theirs body with the line number after `=======`
#[test]
fn original_B_d7474dcda5() {
    run(537);
}
// renderConflictRegion > returns just the base body for a diff3 conflict
#[test]
fn original_B_fd56850f96() {
    run(543);
}
// renderConflictRegion > rejects `base` scope for a 2-way conflict
#[test]
fn original_B_d588334df8() {
    run(549);
}
// formatConflictWarning > emits empty string when no entries
#[test]
fn original_B_edc87c5c93() {
    run(555);
}
// formatConflictWarning > renders the compact diff-style block with labels aggregated at top
#[test]
fn original_B_523a9b0f01() {
    run(559);
}
// formatConflictWarning > pluralizes the summary count and emits one block per entry
#[test]
fn original_B_0590e7c8a2() {
    run(592);
}
// formatConflictWarning > collapses base ≡ ours by skipping the redundant body
#[test]
fn original_B_fe804ffb80() {
    run(601);
}
// formatConflictWarning > collapses base ≡ theirs the same way
#[test]
fn original_B_60e1836642() {
    run(619);
}
// formatConflictWarning > prints the base body when base differs from both sides
#[test]
fn original_B_9f9d117623() {
    run(630);
}
// formatConflictWarning > omits the ours/theirs label lines when no entry has labels
#[test]
fn original_B_d6536b763f() {
    run(645);
}
// formatConflictWarning > caps the body preview at PREVIEW_SIDE_LINES with a `… N more lines` footer
#[test]
fn original_B_8ad6708f82() {
    run(652);
}
// expandContentTokens > returns content unchanged when no tokens are present
#[test]
fn original_B_75c18a03d6() {
    run(671);
}
// expandContentTokens > expands a bare `@ours` token
#[test]
fn original_B_8180cb0684() {
    run(675);
}
// expandContentTokens > expands `@theirs` and `@both` line tokens
#[test]
fn original_B_4ef690c90c() {
    run(679);
}
// expandContentTokens > mixes tokens with literal lines
#[test]
fn original_B_553b3bd6e6() {
    run(684);
}
// expandContentTokens > expands `@base` only when the entry has a base section
#[test]
fn original_B_51e423ec7e() {
    run(688);
}
// expandContentTokens > leaves `@ours` inside a real code line literal (token must be the whole line)
#[test]
fn original_B_77b3651f0d() {
    run(694);
}
// expandContentTokens > handles CRLF input lines
#[test]
fn original_B_96a7ae3b1e() {
    run(698);
}
