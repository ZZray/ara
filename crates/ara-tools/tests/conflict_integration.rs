//! Original conflict-integration.test.ts 37 contracts, fixed OMP 596f2da.
use ara_agent::{ToolError, ToolOutput};
use ara_ai::UserBlock;
use ara_tools::{ToolContext, read::ReadTool, write::WriteTool};
use tokio_util::sync::CancellationToken;
const TWO: &str = "line 1\n<<<<<<< HEAD\noldApi(x)\n=======\nnewApi(x)\n>>>>>>> feature/x\nline N\n";
const THREE: &str =
    "head\n<<<<<<< HEAD\nours body\n||||||| common ancestor\nbase body\n=======\ntheirs body\n>>>>>>> feat\ntail\n";
const BLOCKS: &str =
    "<<<<<<< A\na-ours\n=======\na-theirs\n>>>>>>> A\nmiddle\n<<<<<<< B\nb-ours\n=======\nb-theirs\n>>>>>>> B\ntail\n";
fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|c| if let UserBlock::Text(t) = c { Some(t.text.as_str()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}
async fn read(tool: &ReadTool, path: &str) -> Result<ToolOutput, ToolError> {
    tool.execute_os_path(path, CancellationToken::new()).await
}
async fn write(tool: &WriteTool, path: &str, content: &str) -> Result<ToolOutput, ToolError> {
    tool.execute_os_values(path, content, CancellationToken::new()).await
}
fn has(ctx: &ToolContext, id: f64) -> bool {
    ctx.conflict_history.lock().unwrap().get(id).is_some()
}
fn expect_err(result: Result<ToolOutput, ToolError>, needle: &str) {
    assert!(result.unwrap_err().0.contains(needle), "Expected {needle}");
}
async fn run(line: u32) {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let reader = ReadTool { ctx: ctx.clone() };
    let writer = WriteTool::new(ctx.clone());
    let name = match line {
        119 => "three.ts",
        132 => "two-blocks.ts",
        146 => "clean.ts",
        160 => "stable.ts",
        172 => "full.ts",
        190 => "theirs.ts",
        206 => "base.ts",
        220 => "no-base.ts",
        245 => "many.ts",
        263 => "diff3.ts",
        274 => "clean-conflicts.ts",
        285 => "wide.ts",
        335 => "echo.ts",
        369 => "prefix.ts",
        390 => "bulk-prefix.ts",
        409 => "directives.ts",
        429 => "directives-subset.ts",
        451 => "directives-bad.ts",
        466 => "directives-mixed.ts",
        490 => "directives-badtoken.ts",
        506 => "directives-literal.ts",
        523 => "two.ts",
        548 => "tokens.ts",
        562 => "both.ts",
        576 => "nobase.ts",
        590 => "nope.ts",
        620 => "scoped.ts",
        635 => "stale.ts",
        655 => "hashed.ts",
        710 => "shift.ts",
        _ => "foo.ts",
    };
    let original = match line {
        119 | 206 | 263 => THREE,
        132 | 245 | 285 | 409 | 429 | 466 | 490 | 523 => BLOCKS,
        146 | 274 => "const a = 1;\nconst b = 2;\n",
        335 => "function f() {\n<<<<<<< HEAD\n\tours();\n=======\n\ttheirs();\n>>>>>>> feature/x\n\tdone();\n}\n",
        _ => TWO,
    };
    let file = dir.path().join(name);
    if !matches!(line, 231 | 238 | 605 | 702) {
        std::fs::write(&file, original).unwrap();
    }
    let disk = || std::fs::read_to_string(&file).unwrap();
    match line {
        89 => {
            let out = read(&reader, name).await.unwrap();
            let t = text(&out);
            for s in [
                "<<<<<<< HEAD",
                "oldApi(x)",
                ">>>>>>> feature/x",
                "⚠",
                "⚠ 1 unresolved conflict detected",
                "- ours = HEAD",
                "- theirs = feature/x",
                "──── #1  L2-6 ────",
                "<<< ours",
                ">>> theirs",
                "NOTICE: Inspect a block by reading `conflict://<N>`",
                "`write({ path: \"conflict://<N>\", content })`",
                "`write({ path: \"conflict://*\", content })`",
                "@ours",
            ] {
                assert!(t.contains(s), "{s}: {t}");
            }
            assert_eq!(ctx.conflict_history.lock().unwrap().get(1.0).unwrap().absolute_path, file);
        }
        119 => {
            let t = text(&read(&reader, name).await.unwrap());
            assert!(t.contains("- base = common ancestor"));
            assert!(t.contains("=== base"));
            assert_eq!(
                ctx.conflict_history.lock().unwrap().get(1.0).unwrap().block.base_lines,
                Some(vec!["base body".into()])
            );
        }
        132 => {
            let t = text(&read(&reader, name).await.unwrap());
            assert!(t.contains("──── #1  L1-5 ────"));
            assert!(t.contains("──── #2  L7-11 ────"));
            let h = ctx.conflict_history.lock().unwrap();
            assert_eq!(h.get(1.0).unwrap().block.ours_lines, vec!["a-ours"]);
            assert_eq!(h.get(2.0).unwrap().block.ours_lines, vec!["b-ours"]);
        }
        146 => {
            let t = text(&read(&reader, name).await.unwrap());
            assert!(t.contains("const a = 1;"));
            assert!(!t.contains("conflict://"));
            assert!(!t.contains('⚠'));
            assert!(!has(&ctx, 1.0));
        }
        160 => {
            read(&reader, name).await.unwrap();
            read(&reader, name).await.unwrap();
            assert!(has(&ctx, 1.0));
            assert!(!has(&ctx, 2.0));
        }
        172 => {
            read(&reader, name).await.unwrap();
            let t = text(&read(&reader, "conflict://1").await.unwrap());
            for s in ["<<<<<<< HEAD", "oldApi(x)", "=======", "newApi(x)", ">>>>>>> feature/x"] {
                assert!(t.contains(s));
            }
            assert!(!t.contains('⚠'));
        }
        190 => {
            read(&reader, name).await.unwrap();
            let t = text(&read(&reader, "conflict://1/theirs").await.unwrap());
            assert!(t.contains("newApi(x)"));
            for s in ["<<<<<<<", "=======", ">>>>>>>", "oldApi(x)"] {
                assert!(!t.contains(s));
            }
        }
        206 => {
            read(&reader, name).await.unwrap();
            let t = text(&read(&reader, "conflict://1/base").await.unwrap());
            assert!(t.contains("base body"));
            assert!(!t.contains("ours body"));
            assert!(!t.contains("theirs body"));
        }
        220 => {
            read(&reader, name).await.unwrap();
            expect_err(read(&reader, "conflict://1/base").await, "no base section");
        }
        231 => expect_err(read(&reader, "conflict://99").await, "Conflict #99 not found"),
        238 => expect_err(read(&reader, "conflict://*").await, "wildcards are write-only"),
        245 => {
            let t = text(&read(&reader, &format!("{name}:conflicts")).await.unwrap());
            assert!(t.contains("2 unresolved conflicts in many.ts"));
            assert!(t.contains("#1  L1-5"));
            assert!(t.contains("#2  L7-11"));
            assert!(!t.contains("a-ours"));
            assert!(has(&ctx, 1.0));
            assert!(has(&ctx, 2.0));
        }
        263 => {
            let t = text(&read(&reader, &format!("{name}:conflicts")).await.unwrap());
            assert!(t.contains("#1  L2-8  (3-way)"));
        }
        274 => assert!(
            text(&read(&reader, &format!("{name}:conflicts")).await.unwrap())
                .contains("No unresolved git merge conflicts")
        ),
        285 => {
            let t = text(&read(&reader, &format!("{name}:1-5")).await.unwrap());
            assert!(t.contains("1 of 2 unresolved"));
            assert!(t.contains("read `wide.ts:conflicts`"));
            // TextDecoder parity: a UTF-8 BOM at the first opener must not hide it.
            std::fs::write(&file, format!("\u{feff}{BLOCKS}")).unwrap();
            let t = text(&read(&reader, &format!("{name}:conflicts")).await.unwrap());
            assert!(t.contains("2 unresolved conflicts in wide.ts"));
            assert!(t.contains("#1  L1-5"));
            assert!(t.contains("#2  L7-11"));
        }
        315 => {
            read(&reader, name).await.unwrap();
            assert!(
                text(&write(&writer, "conflict://1", "newApi(x);\n").await.unwrap()).contains("Resolved conflict #1")
            );
            assert_eq!(disk(), "line 1\nnewApi(x);\nline N\n");
            assert!(!has(&ctx, 1.0));
        }
        335 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "conflict://1", "\tours();\n\ttheirs();\n\tdone();\n}\n").await.unwrap());
            assert!(t.contains("Resolved conflict #1"));
            assert!(t.contains("dropped 2 content line(s)"));
            assert_eq!(disk(), "function f() {\n\tours();\n\ttheirs();\n\tdone();\n}\n");
        }
        369 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "prefix.ts:conflict://1", "@theirs").await.unwrap());
            assert!(t.contains("Resolved conflict #1"));
            assert!(t.contains("stripped erroneous 'prefix.ts:' prefix"));
            assert_eq!(disk(), "line 1\nnewApi(x)\nline N\n");
        }
        390 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "bulk-prefix.ts:conflict://*", "@ours").await.unwrap());
            assert!(t.contains("Resolved 1 conflict"));
            assert!(t.contains("stripped erroneous 'bulk-prefix.ts:' prefix"));
            assert_eq!(disk(), "line 1\noldApi(x)\nline N\n");
        }
        409 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "conflict://*", "1: @ours\n2: @theirs\n").await.unwrap());
            assert!(t.contains("Resolved 2 conflicts"));
            assert_eq!(disk(), "a-ours\nmiddle\nb-theirs\ntail\n");
            assert!(!has(&ctx, 1.0));
            assert!(!has(&ctx, 2.0));
        }
        429 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "conflict://*", "2: @ours").await.unwrap());
            assert!(t.contains("Resolved 1 conflict"));
            assert!(t.contains("still registered (#1)"));
            assert_eq!(disk(), "<<<<<<< A\na-ours\n=======\na-theirs\n>>>>>>> A\nmiddle\nb-ours\ntail\n");
            assert!(has(&ctx, 1.0));
        }
        451 => {
            read(&reader, name).await.unwrap();
            expect_err(write(&writer, "conflict://*", "1: @ours\n7: @theirs").await, "unknown conflict id(s) #7");
        }
        466 => {
            read(&reader, name).await.unwrap();
            expect_err(
                write(&writer, "conflict://*", "1: @ours\n2: combined line A\ncombined line B\n").await,
                "Malformed `conflict://*` per-id block",
            );
            assert_eq!(disk(), original);
            assert!(has(&ctx, 1.0));
            assert!(has(&ctx, 2.0));
        }
        490 => {
            read(&reader, name).await.unwrap();
            expect_err(
                write(&writer, "conflict://*", "1: @ours\n2: @mine").await,
                "only accepts the tokens @ours/@theirs/@base/@both",
            );
            assert_eq!(disk(), BLOCKS);
        }
        506 => {
            read(&reader, name).await.unwrap();
            assert!(
                text(&write(&writer, "conflict://*", "resolvedApi(x)\n").await.unwrap())
                    .contains("Resolved 1 conflict")
            );
            assert_eq!(disk(), "line 1\nresolvedApi(x)\nline N\n");
        }
        523 => {
            read(&reader, name).await.unwrap();
            write(&writer, "conflict://2", "B-resolved\n").await.unwrap();
            write(&writer, "conflict://1", "A-resolved\n").await.unwrap();
            assert_eq!(disk(), "A-resolved\nmiddle\nB-resolved\ntail\n");
        }
        548 => {
            read(&reader, name).await.unwrap();
            write(&writer, "conflict://1", "@theirs").await.unwrap();
            assert_eq!(disk(), "line 1\nnewApi(x)\nline N\n");
        }
        562 => {
            read(&reader, name).await.unwrap();
            write(&writer, "conflict://1", "@both").await.unwrap();
            assert_eq!(disk(), "line 1\noldApi(x)\nnewApi(x)\nline N\n");
        }
        576 => {
            read(&reader, name).await.unwrap();
            expect_err(write(&writer, "conflict://1", "@base").await, "no base section");
            assert_eq!(disk(), TWO);
        }
        590 => {
            expect_err(write(&writer, "conflict://99", "x\n").await, "Conflict #99 not found");
            assert_eq!(disk(), TWO);
        }
        605 => {
            for raw in ["conflict://0", "conflict://-1", "conflict://1.5"] {
                expect_err(write(&writer, raw, "x").await, "Invalid conflict URI");
            }
        }
        620 => {
            read(&reader, name).await.unwrap();
            expect_err(write(&writer, "conflict://1/theirs", "x").await, "read-only");
            assert_eq!(disk(), TWO);
        }
        635 => {
            read(&reader, name).await.unwrap();
            std::fs::write(&file, "line 1\nresolved by hand\nline N\n").unwrap();
            let err = write(&writer, "conflict://1", "agent-pick\n").await.unwrap_err().0.to_lowercase();
            assert!(err.contains("stale") || err.contains("outside the current file") || err.contains("no longer"));
            assert_eq!(disk(), "line 1\nresolved by hand\nline N\n");
        }
        655 => {
            read(&reader, name).await.unwrap();
            let t = text(&write(&writer, "conflict://1", "[hashed.ts#1a2b]\n42:cleanline\n").await.unwrap());
            assert!(t.contains("auto-stripped hashline display prefixes"));
            assert_eq!(disk(), "line 1\ncleanline\nline N\n");
        }
        672 => {
            let a = dir.path().join("bulkA.ts");
            let b = dir.path().join("bulkB.ts");
            std::fs::write(&a, BLOCKS).unwrap();
            std::fs::write(&b, TWO).unwrap();
            read(&reader, "bulkA.ts:conflicts").await.unwrap();
            read(&reader, "bulkB.ts:conflicts").await.unwrap();
            assert_eq!(ctx.conflict_history.lock().unwrap().entries().len(), 3);
            let t = text(&write(&writer, "conflict://*", "@theirs").await.unwrap());
            assert!(t.contains("Resolved 3 conflicts across 2 files"));
            assert!(t.contains("bulkA.ts: 2 conflicts"));
            assert!(t.contains("bulkB.ts: 1 conflict"));
            assert_eq!(std::fs::read_to_string(a).unwrap(), "a-theirs\nmiddle\nb-theirs\ntail\n");
            assert_eq!(std::fs::read_to_string(b).unwrap(), "line 1\nnewApi(x)\nline N\n");
            assert!(ctx.conflict_history.lock().unwrap().entries().is_empty());
        }
        702 => expect_err(write(&writer, "conflict://*", "@ours").await, "nothing to resolve"),
        710 => {
            read(&reader, name).await.unwrap();
            std::fs::write(&file, format!("// extra line\n// another extra\n{original}")).unwrap();
            assert!(text(&write(&writer, "conflict://1", "@theirs").await.unwrap()).contains("Resolved conflict #1"));
            assert_eq!(disk(), "// extra line\n// another extra\nline 1\nnewApi(x)\nline N\n");
        }
        _ => panic!("Unmapped original upstream line {line}"),
    }
}
// read surfaces conflicts as a warning footer > returns file content and appends a conflict warning with id 1
#[tokio::test]
async fn original_B_3ec1e18b67() {
    run(89).await;
}
// read surfaces conflicts as a warning footer > registers diff3 conflicts with base section
#[tokio::test]
async fn original_B_b17d8db5ba() {
    run(119).await;
}
// read surfaces conflicts as a warning footer > registers each block with its own id when several appear in one window
#[tokio::test]
async fn original_B_78cc77a3be() {
    run(132).await;
}
// read surfaces conflicts as a warning footer > emits no warning on clean files and does not touch the history
#[tokio::test]
async fn original_B_45428e16dd() {
    run(146).await;
}
// read surfaces conflicts as a warning footer > re-reading the same file reuses the existing id rather than inflating
#[tokio::test]
async fn original_B_084aabdef5() {
    run(160).await;
}
// read surfaces conflicts as a warning footer > renders the full conflict block via reads of `conflict://<N>`
#[tokio::test]
async fn original_B_c3424c8527() {
    run(172).await;
}
// read surfaces conflicts as a warning footer > renders only the theirs body via reads of `conflict://<N>/theirs`
#[tokio::test]
async fn original_B_af6daf9807() {
    run(190).await;
}
// read surfaces conflicts as a warning footer > renders the base body for a diff3 conflict via `/base`
#[tokio::test]
async fn original_B_4772c73092() {
    run(206).await;
}
// read surfaces conflicts as a warning footer > rejects `/base` on a 2-way conflict with a clear error
#[tokio::test]
async fn original_B_19aeedf9a3() {
    run(220).await;
}
// read surfaces conflicts as a warning footer > errors clearly when the conflict id is unknown
#[tokio::test]
async fn original_B_5b5f824060() {
    run(231).await;
}
// read surfaces conflicts as a warning footer > rejects reads of `conflict://*` (wildcard is write-only)
#[tokio::test]
async fn original_B_4a13ca0760() {
    run(238).await;
}
// read surfaces conflicts as a warning footer > the `<path>:conflicts` read selector lists every conflict in the file with stable ids
#[tokio::test]
async fn original_B_2dec298132() {
    run(245).await;
}
// read surfaces conflicts as a warning footer > `:conflicts` marks 3-way blocks distinctly
#[tokio::test]
async fn original_B_81abb295dc() {
    run(263).await;
}
// read surfaces conflicts as a warning footer > `:conflicts` on a clean file says so explicitly
#[tokio::test]
async fn original_B_2d086bb01f() {
    run(274).await;
}
// read surfaces conflicts as a warning footer > window-mode warning shows visible-of-total when window misses some conflicts
#[tokio::test]
async fn original_B_a6bfa26c34() {
    run(285).await;
}
// write resolves conflicts via conflict://N > splices the registered region with the supplied content
#[tokio::test]
async fn original_B_511519c501() {
    run(315).await;
}
// write resolves conflicts via conflict://N > drops trailing lines that echo the context below the region and notes the repair
#[tokio::test]
async fn original_B_51288dc321() {
    run(335).await;
}
// write resolves conflicts via conflict://N > auto-recovers a `<file>:conflict://N` path and resolves the conflict
#[tokio::test]
async fn original_B_c98927b3f1() {
    run(369).await;
}
// write resolves conflicts via conflict://N > auto-recovers a `<file>:conflict://*` path and bulk-resolves
#[tokio::test]
async fn original_B_9e0f987ea1() {
    run(390).await;
}
// write resolves conflicts via conflict://N > resolves per-id bulk directives in one call, leaving unlisted ids registered
#[tokio::test]
async fn original_B_286135b754() {
    run(409).await;
}
// write resolves conflicts via conflict://N > directive mode resolves a subset and reports the ids left registered
#[tokio::test]
async fn original_B_6f27258848() {
    run(429).await;
}
// write resolves conflicts via conflict://N > rejects directives referencing unknown ids
#[tokio::test]
async fn original_B_a9ae4e60c8() {
    run(451).await;
}
// write resolves conflicts via conflict://N > rejects a per-id block that mixes directives with literal content instead of leaking it
#[tokio::test]
async fn original_B_385a2f8120() {
    run(466).await;
}
// write resolves conflicts via conflict://N > rejects a per-id line whose value is not a recognized @side token
#[tokio::test]
async fn original_B_0d8fb7bd71() {
    run(490).await;
}
// write resolves conflicts via conflict://N > still treats pure-literal content as a uniform bulk write (no false directive rejection)
#[tokio::test]
async fn original_B_8ace69e611() {
    run(506).await;
}
// write resolves conflicts via conflict://N > can resolve two blocks in the same file by id, in either order
#[tokio::test]
async fn original_B_02adff98cf() {
    run(523).await;
}
// write resolves conflicts via conflict://N > accepts `@ours`/`@theirs`/`@both` content tokens as shorthand
#[tokio::test]
async fn original_B_1f6c32307e() {
    run(548).await;
}
// write resolves conflicts via conflict://N > expands `@both` to ours then theirs without re-typing either side
#[tokio::test]
async fn original_B_3d9ebc90ae() {
    run(562).await;
}
// write resolves conflicts via conflict://N > rejects `@base` for a 2-way conflict with a clear error
#[tokio::test]
async fn original_B_bafa1e9b32() {
    run(576).await;
}
// write resolves conflicts via conflict://N > errors clearly when the id is unknown
#[tokio::test]
async fn original_B_a58b44af08() {
    run(590).await;
}
// write resolves conflicts via conflict://N > errors clearly when the URI itself is malformed
#[tokio::test]
async fn original_B_c5e08eb5e1() {
    run(605).await;
}
// write resolves conflicts via conflict://N > rejects scoped conflict URIs on write (read-only)
#[tokio::test]
async fn original_B_da53545f06() {
    run(620).await;
}
// write resolves conflicts via conflict://N > rejects stale resolutions when the file changed out of band
#[tokio::test]
async fn original_B_c74fbabb89() {
    run(635).await;
}
// write resolves conflicts via conflict://N > strips hashline display prefixes from replacement content when hashline mode is active
#[tokio::test]
async fn original_B_fad13d1552() {
    run(655).await;
}
// write resolves conflicts via conflict://N > `write conflict://*` bulk-resolves every registered conflict, per-entry token expansion
#[tokio::test]
async fn original_B_d98ed93a5a() {
    run(672).await;
}
// write resolves conflicts via conflict://N > `write conflict://*` errors when no conflicts are registered
#[tokio::test]
async fn original_B_87eda46261() {
    run(702).await;
}
// write resolves conflicts via conflict://N > splice relocates when line numbers shift out of band
#[tokio::test]
async fn original_B_2564a6c5b9() {
    run(710).await;
}
