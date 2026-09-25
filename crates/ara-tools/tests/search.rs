//! `grep` and `glob` against real directory trees (OMP grep.ts / glob.ts
//! behaviors: output shape, pagination, selectors, ignore rules, errors).

use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::engine;
use ara_tools::glob::GlobTool;
use ara_tools::grep::GrepTool;
use ara_tools::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
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

fn put(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

fn age(root: &Path, rel: &str, secs_ago: u64) {
    let f = std::fs::File::options().write(true).open(root.join(rel)).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(secs_ago)).unwrap();
}

async fn grep(root: &Path, a: Value) -> Result<ToolOutput, ToolError> {
    GrepTool::new(plain_ctx(root)).execute("c", args(a), CancellationToken::new(), noop()).await
}

async fn glob(root: &Path, a: Value) -> Result<ToolOutput, ToolError> {
    GlobTool::new(plain_ctx(root)).execute("c", args(a), CancellationToken::new(), noop()).await
}

#[tokio::test]
async fn grep_groups_directory_matches_with_context() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    put(r, "src/lib.rs", "use std;\nfn alpha() {}\nfn beta() {}\n// end\n");
    put(r, "src/deep/mod.rs", "one\ntwo\nthree\nfour\nfive\nsix\nfn alpha_two() {}\n");
    put(r, "README.md", "alpha docs\n");
    let out = grep(r, json!({"pattern": "alpha"})).await.unwrap();
    assert_eq!(
        text(&out),
        "# README.md\n*1|alpha docs\n\n# src/\n## lib.rs\n 1|use std;\n*2|fn alpha() {}\n 3|fn beta() {}\n 4|// end\n\n## deep/\n### mod.rs\n 6|six\n*7|fn alpha_two() {}"
    );
    let details = out.details.unwrap();
    assert_eq!(details["matchCount"], 3);
    assert_eq!(details["fileCount"], 3);
    assert_eq!(details["truncated"], false);

    // Single file: no headers, `...` marks a gap between context windows.
    put(r, "one.txt", "hit\na\nb\nc\nd\ne\nf\nhit\n");
    let out = grep(r, json!({"pattern": "hit", "path": "one.txt"})).await.unwrap();
    assert_eq!(text(&out), "*1|hit\n 2|a\n 3|b\n 4|c\n...\n 7|f\n*8|hit");

    // Case switch, then no matches.
    assert_eq!(
        text(&grep(r, json!({"pattern": "ALPHA DOCS", "case": false})).await.unwrap()),
        "# README.md\n*1|alpha docs"
    );
    let none = grep(r, json!({"pattern": "zzz_nothing"})).await.unwrap();
    assert_eq!(text(&none), "No matches found");
}

#[tokio::test]
async fn grep_paginates_files_and_caps_hot_files() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..25 {
        put(r, &format!("f{i:02}.txt"), "needle\n");
    }
    let out = grep(r, json!({"pattern": "needle"})).await.unwrap();
    let t = text(&out);
    assert!(t.starts_with("# f00.txt\n*1|needle\n\n# f01.txt"), "{t}");
    assert!(
        t.ends_with("\n\nShowing files 1-20 of 25. Use skip=20 for the next page, or narrow paths/pattern."),
        "{t}"
    );
    assert_eq!(out.details.as_ref().unwrap()["fileLimitReached"], 20);
    let page2 = text(&grep(r, json!({"pattern": "needle", "skip": 20})).await.unwrap());
    assert!(page2.starts_with("# f20.txt") && page2.ends_with("# f24.txt\n*1|needle"), "{page2}");
    let past = text(&grep(r, json!({"pattern": "needle", "skip": 30})).await.unwrap());
    assert_eq!(past, "No more results (25 files total; skip=30 is past the end)");
    let null_skip = grep(r, json!({"pattern": "needle", "skip": null})).await.unwrap();
    assert!(text(&null_skip).contains("Showing files 1-20 of 25."));

    // A hot file is trimmed to 20 matches in a directory scope, and the file
    // total becomes a lower bound because the per-file fetch cap was hit.
    let hot = tempfile::tempdir().unwrap();
    put(hot.path(), "hot.txt", &"x\n".repeat(30));
    put(hot.path(), "cold.txt", "x\n");
    let out = grep(hot.path(), json!({"pattern": "x"})).await.unwrap();
    let details = out.details.unwrap();
    assert_eq!(details["perFileLimitReached"], 20);
    assert_eq!(details["fileMatches"], json!([{"path": "cold.txt", "count": 1}, {"path": "hot.txt", "count": 20}]));
    // Single-file scope keeps up to 200.
    let single = grep(hot.path(), json!({"pattern": "x", "path": "hot.txt"})).await.unwrap();
    assert_eq!(single.details.unwrap()["matchCount"], 30);
}

#[tokio::test]
async fn grep_path_lists_selectors_and_missing_entries() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    put(r, "src/a.rs", "k1\nk2\nk3\nk4\nk5\nk6\nk7\n");
    put(r, "tests/t.rs", "k in test\n");
    put(r, "other/o.rs", "k other\n");

    let t = text(&grep(r, json!({"pattern": "k", "path": "src; tests"})).await.unwrap());
    assert!(t.contains("# src/\n## a.rs") && t.contains("# tests/\n## t.rs") && !t.contains("other"), "{t}");

    // Line-range selector filters matches and drops context outside the range.
    let t = text(&grep(r, json!({"pattern": "k", "path": "src/a.rs:3-4"})).await.unwrap());
    assert_eq!(t, "*3|k3\n*4|k4");

    let t = text(&grep(r, json!({"pattern": "k", "path": "src; nope"})).await.unwrap());
    assert!(t.ends_with("\n\nSkipped missing paths: nope"), "{t}");

    // A real file whose name looks like a selector wins over the selector.
    put(r, "notes:1-2", "k literal\n");
    let t = text(&grep(r, json!({"pattern": "literal", "path": "notes:1-2"})).await.unwrap());
    assert_eq!(t, "*1|k literal");

    // Glob in path.
    let t = text(&grep(r, json!({"pattern": "k", "path": "**/t.rs"})).await.unwrap());
    assert_eq!(t, "# tests/\n## t.rs\n*1|k in test");
}

#[tokio::test]
async fn grep_errors_match_upstream_text() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    put(r, "src/a.rs", "a\n");
    let err = |a: Value| async move { grep(r, a).await.unwrap_err().0 };
    assert_eq!(err(json!({"pattern": "  "})).await, "Pattern must not be empty");
    assert_eq!(err(json!({"pattern": "a", "skip": -1})).await, "Skip must be a non-negative number");
    assert_eq!(err(json!({"pattern": "a", "path": "nope"})).await, "Path not found: nope");
    assert_eq!(err(json!({"pattern": "a", "path": "x; y"})).await, "Path not found: x, y");
    assert_eq!(
        err(json!({"pattern": "a", "path": "src:1-2"})).await,
        "Line-range selector requires a single file: src:1-2 is a directory"
    );
    assert_eq!(
        err(json!({"pattern": "a", "path": "src/*.rs:1-2"})).await,
        "Line-range selector requires a single file, not a glob: src/*.rs:1-2"
    );
    assert_eq!(
        err(json!({"pattern": "a", "path": "src/a.rs:raw"})).await,
        "path entry \"src/a.rs:raw\" — only line-range selectors like \":50-100\" are supported (no \":raw\"/\":conflicts\")"
    );
    assert_eq!(
        err(json!({"pattern": "a", "path": "src/a.rs:0"})).await,
        "Line selector 0 is invalid; lines are 1-indexed. Use :1."
    );
    assert_eq!(
        err(json!({"pattern": "a", "path": "https://example.com/x"})).await,
        "Cannot search external URL: https://example.com/x. Use `read` to fetch web content, then search the returned text."
    );
    // Stray parenthesis and lookaround still search instead of failing.
    put(r, "code.ts", "const p = fetchProvider(a);\nfoobar\n");
    assert_eq!(
        text(&grep(r, json!({"pattern": "fetchProvider(", "path": "code.ts"})).await.unwrap()),
        "*1|const p = fetchProvider(a);\n 2|foobar"
    );
    assert_eq!(
        text(&grep(r, json!({"pattern": "foo(?=bar)", "path": "code.ts"})).await.unwrap()),
        " 1|const p = fetchProvider(a);\n*2|foobar"
    );
}

#[tokio::test]
async fn grep_honours_ignore_rules_binary_and_columns() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    put(r, ".git/config", "token\n");
    put(r, ".gitignore", "build/\n");
    put(r, "build/out.txt", "token\n");
    put(r, "node_modules/pkg/index.js", "token\n");
    put(r, ".hidden.txt", "token\n");
    put(r, "src/keep.txt", "token\n");
    std::fs::write(r.join("bin.dat"), b"token\0\x01\x02").unwrap();
    let files = |o: &ToolOutput| o.details.as_ref().unwrap()["files"].clone();
    let out = grep(r, json!({"pattern": "token"})).await.unwrap();
    assert_eq!(files(&out), json!([".hidden.txt", "src/keep.txt"]), "{}", text(&out));
    let out = grep(r, json!({"pattern": "token", "gitignore": false})).await.unwrap();
    assert_eq!(files(&out), json!([".hidden.txt", "build/out.txt", "src/keep.txt"]));
    let out = grep(r, json!({"pattern": "token", "path": "node_modules/**/*.js"})).await.unwrap();
    assert_eq!(files(&out), json!(["node_modules/pkg/index.js"]));

    // Parent .gitignore applies when searching a subdirectory of the repo.
    put(r, "src/build/deep.txt", "token\n");
    let out = grep(r, json!({"pattern": "token", "path": "src"})).await.unwrap();
    assert_eq!(files(&out), json!(["src/keep.txt"]));

    let long = format!("start{}\n", "y".repeat(600));
    put(r, "long.txt", &long);
    let out = grep(r, json!({"pattern": "start", "path": "long.txt"})).await.unwrap();
    let t = text(&out);
    assert!(t.starts_with(&format!("*1|start{}...", "y".repeat(504))), "{t}");
    assert!(t.ends_with("\n\n[Some lines truncated to 512 chars]"), "{t}");

    // Cross-line pattern.
    put(r, "ml.txt", "first\nsecond\n");
    let t = text(&grep(r, json!({"pattern": "first\\nsecond", "path": "ml.txt"})).await.unwrap());
    assert_eq!(t, "*1|first\nsecond");
}

#[tokio::test]
async fn grep_notes_oversized_targets_and_honours_budget() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    let mut big = "early\n".to_string();
    big.push_str(&"z".repeat(5 * 1024 * 1024));
    big.push_str("\nlate\n");
    put(r, "big.log", &big);
    let t = text(&grep(r, json!({"pattern": "early|late", "path": "big.log"})).await.unwrap());
    assert!(t.starts_with("*1|early"), "{}", &t[..80]);
    assert!(!t.contains("late\n") && t.ends_with("Searched only the first 4MB of large files (matches past the 4MB window are not shown; use `read` for the rest): big.log"), "{}", &t[t.len() - 200..]);

    let tool = GrepTool { ctx: plain_ctx(r), timeout: Duration::ZERO };
    let e = tool.execute("c", args(json!({"pattern": "early"})), CancellationToken::new(), noop()).await.unwrap_err();
    assert_eq!(e.0, "Grep timed out after 0s; narrow paths or pattern, or scope with `glob` first");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let e = GrepTool::new(plain_ctx(r)).execute("c", args(json!({"pattern": "early"})), cancel, noop()).await;
    assert_eq!(e.unwrap_err().0, "Grep was aborted");
}

#[tokio::test]
async fn glob_lists_newest_first_grouped_with_dirs() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    put(r, "src/a.ts", "");
    put(r, "src/nested/b.ts", "");
    put(r, "c.ts", "");
    put(r, "src/tests/x.txt", "");
    age(r, "src/a.ts", 300);
    age(r, "src/nested/b.ts", 200);
    age(r, "c.ts", 100);
    let out = glob(r, json!({"path": "*.ts"})).await.unwrap();
    // A directory's own files precede its subdirectories in the grouped text.
    assert_eq!(text(&out), "c.ts\n# src/\na.ts\n## nested/\nb.ts");
    assert_eq!(out.details.unwrap()["files"], json!(["c.ts", "src/nested/b.ts", "src/a.ts"]));

    // Directory matches end in `/`; `dir/*` does not recurse.
    let out = glob(r, json!({"path": "**/tests"})).await.unwrap();
    assert_eq!(out.details.unwrap()["files"], json!(["src/tests/"]));
    let out = glob(r, json!({"path": "src/*.ts"})).await.unwrap();
    assert_eq!(out.details.unwrap()["files"], json!(["src/a.ts"]));

    // A plain file and a plain directory.
    assert_eq!(text(&glob(r, json!({"path": "c.ts"})).await.unwrap()), "c.ts");
    let out = glob(r, json!({"path": "src/nested"})).await.unwrap();
    assert_eq!(out.details.unwrap()["files"], json!(["src/nested/b.ts"]));

    // Limit and its notice.
    let out = glob(r, json!({"path": "**/*.ts", "limit": 2})).await.unwrap();
    assert!(text(&out).ends_with("\n\n[2 results limit reached. Use limit=4 for more]"), "{}", text(&out));
    assert_eq!(out.details.unwrap()["fileCount"], 2);
}

#[tokio::test]
async fn glob_ignore_rules_and_errors() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    put(r, ".gitignore", ".env*\n");
    put(r, ".env.local", "");
    put(r, ".tool.json", "");
    put(r, "node_modules/p/i.js", "");
    put(r, "a.js", "");
    let files = |o: ToolOutput| o.details.unwrap()["files"].clone();
    let sorted = |v: Value| {
        let mut v: Vec<String> = v.as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
        v.sort();
        v
    };
    // `*` gains `**/`; `.git` and `node_modules` are pruned; `.env*` is gitignored.
    assert_eq!(sorted(files(glob(r, json!({"path": "*"})).await.unwrap())), [".gitignore", ".tool.json", "a.js"]);
    assert_eq!(
        sorted(files(glob(r, json!({"path": "*", "gitignore": false})).await.unwrap())),
        [".env.local", ".gitignore", ".tool.json", "a.js"]
    );
    assert_eq!(sorted(files(glob(r, json!({"path": "*", "hidden": false})).await.unwrap())), ["a.js"]);
    assert_eq!(files(glob(r, json!({"path": "node_modules/**/*.js"})).await.unwrap()), json!(["node_modules/p/i.js"]));

    let err = |a: Value| async move { glob(r, a).await.unwrap_err().0 };
    assert_eq!(err(json!({"path": "/"})).await, "Searching from root directory '/' is not allowed");
    assert_eq!(err(json!({"path": "*", "limit": 0})).await, "Limit must be a positive number");
    assert_eq!(err(json!({"path": "nope"})).await, "Path not found: nope");
    assert_eq!(err(json!({"path": "x; y"})).await, "Path not found: x, y");
    let not_dir = err(json!({"path": "a.js/*.rs"})).await;
    assert_eq!(not_dir, format!("Path is not a directory: {}", r.join("a.js").display()));
    assert_eq!(text(&glob(r, json!({"path": "*.zzz"})).await.unwrap()), "No files found matching pattern");
    let t = text(&glob(r, json!({"path": "a.js; missing"})).await.unwrap());
    assert_eq!(t, "a.js\n\nSkipped missing paths: missing");

    let expired = GlobTool { ctx: plain_ctx(r), timeout: Duration::ZERO };
    let t = text(&expired.execute("c", args(json!({"path": "**/*"})), CancellationToken::new(), noop()).await.unwrap());
    assert!(
        t.starts_with(
            "Glob timed out after 0s before finding any matches — the scan is incomplete, NOT proof of absence."
        ),
        "{t}"
    );
}

#[tokio::test]
async fn parent_ignore_rules_follow_upstream_anchoring_and_precedence() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    put(r, ".gitignore", "/pkg/dist/\npkg/gen.txt\n*.log\n");
    put(r, ".ignore", "!keep.log\n");
    put(r, "pkg/dist/a.txt", "token\n");
    put(r, "pkg/gen.txt", "token\n");
    put(r, "pkg/src.txt", "token\n");
    put(r, "pkg/keep.log", "token\n");
    put(r, "pkg/drop.log", "token\n");
    let files = |o: ToolOutput| o.details.unwrap()["files"].clone();
    // Anchored parent rules apply below a subdirectory root; `.ignore` wins over `.gitignore`.
    let from_root = files(grep(r, json!({"pattern": "token"})).await.unwrap());
    assert_eq!(from_root, json!(["pkg/keep.log", "pkg/src.txt"]));
    assert_eq!(files(grep(r, json!({"pattern": "token", "path": "pkg"})).await.unwrap()), from_root);
    let sub = GrepTool::new(plain_ctx(r.join("pkg")));
    let out = sub.execute("c", args(json!({"pattern": "token"})), CancellationToken::new(), noop()).await.unwrap();
    assert_eq!(files(out), json!(["keep.log", "src.txt"]));
    let listed = files(glob(r, json!({"path": "pkg/**/*"})).await.unwrap());
    let mut listed: Vec<&str> = listed.as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    listed.sort();
    assert_eq!(listed, ["pkg/keep.log", "pkg/src.txt"]);

    // A parent rule that would hide the walk root itself is dropped for that walk.
    put(r, ".gitignore", "/pkg/dist/\npkg/gen.txt\n*.log\nignored_root/\n");
    put(r, "ignored_root/x.txt", "token\n");
    assert_eq!(
        files(grep(r, json!({"pattern": "token", "path": "ignored_root"})).await.unwrap()),
        json!(["ignored_root/x.txt"])
    );

    // `.jj` marks a repository root too.
    let jj = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(jj.path().join(".jj")).unwrap();
    put(jj.path(), ".gitignore", "*.gen\n");
    put(jj.path(), "sub/a.gen", "token\n");
    put(jj.path(), "sub/b.txt", "token\n");
    let out = grep(jj.path(), json!({"pattern": "token", "path": "sub"})).await.unwrap();
    assert_eq!(files(out), json!(["sub/b.txt"]));
}

#[tokio::test]
async fn review_regressions() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    // A failed search of a named file is an error, not "no matches".
    put(r, "a.txt", &format!("{}b\n", "a".repeat(40)));
    let e = grep(r, json!({"pattern": "^(a|a)*\\1$", "path": "a.txt"})).await.unwrap_err();
    assert!(e.0.starts_with("Search failed: "), "{}", e.0);
    // Non-ASCII digits are not a selector.
    put(r, "notes", "k\n");
    let e = grep(r, json!({"pattern": "k", "path": "notes:٣"})).await.unwrap_err();
    assert_eq!(e.0, "Path not found: notes:٣");
    // Zero-result details keep `files` and omit empty `missingPaths`.
    let details = grep(r, json!({"pattern": "zzz"})).await.unwrap().details.unwrap();
    assert_eq!(details["files"], json!([]));
    assert!(details.get("missingPaths").is_none());
    let details = glob(r, json!({"path": "*.zzz"})).await.unwrap().details.unwrap();
    assert_eq!(details["files"], json!([]));
    assert!(details.get("missingPaths").is_none());
    // Oversized note uses a cwd-relative path even outside cwd.
    let mut big = "early\n".to_string();
    big.push_str(&"z".repeat(5 * 1024 * 1024));
    put(r, "other/big.log", &big);
    std::fs::create_dir_all(r.join("work")).unwrap();
    let tool = GrepTool::new(plain_ctx(r.join("work")));
    let out = tool
        .execute("c", args(json!({"pattern": "early", "path": "../other/big.log"})), CancellationToken::new(), noop())
        .await
        .unwrap();
    assert!(text(&out).ends_with("use `read` for the rest): ../other/big.log"), "{}", text(&out));
    // A relative host cwd is resolved against the process directory.
    let here = GrepTool::new(plain_ctx("."));
    let out = here
        .execute(
            "c",
            args(json!({"pattern": "pub fn builtin_tools", "path": "src/lib.rs"})),
            CancellationToken::new(),
            noop(),
        )
        .await
        .unwrap();
    assert!(text(&out).contains("|pub fn builtin_tools"), "{}", text(&out));
}

fn engine_params() -> engine::GrepParams {
    engine::GrepParams {
        ignore_case: false,
        multiline: false,
        include_hidden: true,
        use_gitignore: true,
        max_count: Some(2000),
        max_count_per_file: Some(21),
        context_before: 1,
        context_after: 3,
        max_columns: Some(512),
    }
}

fn budget() -> engine::Budget {
    engine::Budget { cancel: CancellationToken::new(), deadline: std::time::Instant::now() + Duration::from_secs(30) }
}

#[tokio::test]
async fn windowed_search_stops_at_the_budget_and_defers_oversized_files() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for i in 0..2100 {
        put(r, &format!("f{i:04}.txt"), "needle\n");
    }
    let mut big = "needle\n".to_string();
    big.push_str(&"z".repeat(5 * 1024 * 1024));
    put(r, "0big.log", &big);
    let m = engine::build_matcher("needle", false, false).unwrap();
    let out = engine::grep(&m, r, None, &engine_params(), &budget()).unwrap();
    // Four 512-file windows (the first holds the deferred `0big.log`) search
    // 2047 files, reaching >= 2000 matches; the walk stops there and the
    // deferred oversized file is never searched.
    assert_eq!(out.files_searched, 2047);
    assert_eq!(out.matches.len(), 2000);
    assert!(out.limit_reached);
    assert!(out.matches.iter().all(|m| m.path != "0big.log"));
    let t = text(&grep(r, json!({"pattern": "needle"})).await.unwrap());
    assert!(t.ends_with("Showing files 1-20 of 2000+. Use skip=20 for the next page, or narrow paths/pattern."), "{t}");

    // Under budget, oversized files follow the others regardless of path order.
    let small = tempfile::tempdir().unwrap();
    put(small.path(), "a_big.log", &big);
    put(small.path(), "z.txt", "needle\n");
    let out = engine::grep(&m, small.path(), None, &engine_params(), &budget()).unwrap();
    let order: Vec<&str> = out.matches.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(order, ["z.txt", "a_big.log"]);
    let details = grep(small.path(), json!({"pattern": "needle"})).await.unwrap().details.unwrap();
    assert_eq!(details["files"], json!(["z.txt", "a_big.log"]));
}

#[tokio::test]
async fn special_files_symlinked_roots_and_multiline_merging() {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    put(r, "a.txt", "x\n");
    let status = std::process::Command::new("mkfifo").arg(r.join("pipe")).status().unwrap();
    assert!(status.success());
    let files = |o: ToolOutput| o.details.unwrap()["files"].clone();
    assert_eq!(files(glob(r, json!({"path": "*"})).await.unwrap()), json!(["a.txt"]), "FIFOs are never listed");

    // A symlinked root walks the real directory with its repository's ignore chain.
    let repo = r.join("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    put(&repo, ".gitignore", "*.gen\n");
    put(&repo, "sub/a.gen", "token\n");
    put(&repo, "sub/b.txt", "token\n");
    std::fs::create_dir_all(r.join("outside")).unwrap();
    std::os::unix::fs::symlink(repo.join("sub"), r.join("outside/link")).unwrap();
    assert_eq!(files(glob(r, json!({"path": "outside/link"})).await.unwrap()), json!(["outside/link/b.txt"]));
    assert_eq!(
        files(grep(r, json!({"pattern": "token", "path": "outside/link"})).await.unwrap()),
        json!(["outside/link/b.txt"])
    );

    // A multi-line pattern that cannot match a newline still merges adjacent
    // matching lines into one match, as upstream's multi-line search does.
    put(r, "c.txt", "printf(\"a\\n\");\nprintf(\"b\\n\");\nprintf(\"c\\n\");\n");
    let t = text(&grep(r, json!({"pattern": "\\\\n", "path": "c.txt"})).await.unwrap());
    assert_eq!(t, "*1|printf(\"a\\n\");\nprintf(\"b\\n\");\nprintf(\"c\\n\");");
}

/// Plain display (no edit tool exposed): these cases assert `*N|line` rows.
fn plain_ctx(dir: impl Into<std::path::PathBuf>) -> ToolContext {
    ToolContext::new(dir).with_edit(pi_edit::EditMode::Hashline, false)
}
