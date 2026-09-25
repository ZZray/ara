//! Rust ports of upstream search-tool cases (OMP
//! `packages/coding-agent/test/tools/{grep-path-lists,multi-grep-path,
//! multi-path-missing}.test.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d).
//! Upstream runs these in hashline display mode (`## grep.txt#TAG`,
//! `[file#TAG]`); ARA has no hashline edit tool yet, so the assertions use the
//! plain display mode and otherwise keep the upstream expectations.

use ara_agent::{AgentTool, ToolOutput};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::ToolContext;
use ara_tools::glob::GlobTool;
use ara_tools::grep::GrepTool;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn text(o: &ToolOutput) -> String {
    o.content
        .iter()
        .filter_map(|b| if let UserBlock::Text(t) = b { Some(t.text.clone()) } else { None })
        .collect::<Vec<_>>()
        .join("\n")
}

fn args(v: Value) -> JsonObject {
    v.as_object().unwrap().clone()
}

async fn grep(cwd: &Path, a: Value) -> ToolOutput {
    GrepTool::new(plain_ctx(cwd)).execute("c", args(a), CancellationToken::new(), Arc::new(|_| {})).await.unwrap()
}

async fn glob(cwd: &Path, a: Value) -> ToolOutput {
    GlobTool::new(plain_ctx(cwd)).execute("c", args(a), CancellationToken::new(), Arc::new(|_| {})).await.unwrap()
}

/// `createSearchFixture`.
fn fixture() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    for t in ["apps", "packages", "phases", "other"] {
        std::fs::create_dir_all(r.join(t)).unwrap();
        std::fs::write(r.join(t).join("grep.txt"), format!("shared-needle {t}\n")).unwrap();
    }
    for t in ["apps", "packages", "phases"] {
        std::fs::write(r.join(t).join("ast.ts"), format!("const providerOptions = {{}};\nlegacyWrap({t}Value);\n"))
            .unwrap();
    }
    std::fs::create_dir_all(r.join("folder with spaces")).unwrap();
    std::fs::write(r.join("folder with spaces/note.txt"), "space-needle\n").unwrap();
    d
}

fn d(o: &ToolOutput) -> Value {
    o.details.clone().unwrap()
}

// B-05450bf5bf, B-11eb8cdfe7, B-fad0ff768e
#[tokio::test]
async fn semicolon_lists_render_only_headings_with_children() {
    let f = fixture();
    let out = grep(f.path(), json!({"pattern": "shared-needle", "path": "apps/; packages/; phases/"})).await;
    let t = text(&out);
    for dir in ["apps", "packages", "phases"] {
        assert!(t.contains(&format!("# {dir}/\n## grep.txt\n*1|shared-needle {dir}")), "{t}");
    }
    assert!(!t.contains("other"));
    assert_eq!(d(&out)["fileCount"], 3);
    assert_eq!(d(&out)["scopePath"], "apps/, packages/, phases/");
    let lines: Vec<&str> = t.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with('#') {
            assert!(lines.get(i + 1).is_some_and(|n| !n.trim().is_empty()), "heading {line} has children");
        }
    }
}

// B-ae974f17f7, B-ef43cd2aa4, B-696defef59
#[tokio::test]
async fn json_array_strings_and_empty_arrays() {
    let f = fixture();
    let out =
        grep(f.path(), json!({"pattern": "shared-needle", "path": "[\"apps/\", \"packages/\", \"phases/\"]"})).await;
    assert_eq!(d(&out)["fileCount"], 3);
    assert_eq!(d(&out)["scopePath"], "apps/, packages/, phases/");
    let out = grep(f.path(), json!({"pattern": "space-needle", "path": "[]"})).await;
    assert!(text(&out).contains("space-needle"));
    let out = grep(f.path(), json!({"pattern": "space-needle"})).await;
    assert!(text(&out).contains("space-needle"));
}

// B-863ebc2f69, B-9e04ef3f90, B-b04c0bd414
#[tokio::test]
async fn delimited_entries_and_missing_peers() {
    let f = fixture();
    for entry in
        ["apps/grep.txt, packages/grep.txt", "apps/grep.txt;packages/grep.txt", "apps/grep.txt packages/grep.txt"]
    {
        let out = grep(f.path(), json!({"pattern": "shared-needle", "path": entry})).await;
        assert_eq!(d(&out)["fileCount"], 2, "{entry}: {}", text(&out));
        assert!(!text(&out).contains("other"));
    }
    let out = grep(f.path(), json!({"pattern": "shared-needle", "path": "missing.txt, packages/grep.txt"})).await;
    let t = text(&out);
    assert!(t.contains("Skipped missing paths: missing.txt") && !t.contains("apps"), "{t}");
    assert_eq!(d(&out)["fileCount"], 1);
    assert_eq!(d(&out)["missingPaths"], json!(["missing.txt"]));
}

// B-d90ac5205b, B-7037613205, B-fc67c53e51, B-a1801f60bc
#[tokio::test]
async fn spaces_quotes_absolute_and_bracketed_literals() {
    let f = fixture();
    let out = grep(f.path(), json!({"pattern": "space-needle", "path": "folder with spaces/"})).await;
    assert!(text(&out).contains("note.txt"));
    assert_eq!(d(&out)["fileCount"], 1);
    assert_eq!(d(&out)["scopePath"], "folder with spaces");
    let out = grep(f.path(), json!({"pattern": "shared-needle", "path": "\"packages/\""})).await;
    assert!(text(&out).contains("grep.txt") && !text(&out).contains("other"));
    assert_eq!(d(&out)["scopePath"], "packages");
    let abs = f.path().join("apps");
    let out = grep(f.path(), json!({"pattern": "shared-needle", "path": abs.to_str().unwrap()})).await;
    let t = text(&out);
    assert!(t.starts_with("# apps/\n## grep.txt\n") && !t.contains(f.path().to_str().unwrap()), "{t}");
    assert_eq!(d(&out)["scopePath"], "apps");

    std::fs::create_dir_all(f.path().join("apps/[id]")).unwrap();
    std::fs::write(f.path().join("apps/[id]/page.tsx"), "bracket-needle\n").unwrap();
    for p in ["apps/[id]/page.tsx", "apps/[id]"] {
        assert!(
            text(&grep(f.path(), json!({"pattern": "bracket-needle", "path": p})).await).contains("bracket-needle")
        );
    }
}

// B-a532b06227, B-c3f4a53082 (upstream sets context 1/1; defaults here are 1/3)
#[tokio::test]
async fn explicit_files_stay_exact_and_gutters_mark_matches() {
    let f = tempfile::tempdir().unwrap();
    let r = f.path();
    std::fs::create_dir_all(r.join("nested")).unwrap();
    for (p, body) in [
        ("alpha.txt", "exact-needle alpha\n"),
        ("beta.txt", "exact-needle beta\n"),
        ("nested/alpha.txt", "exact-needle nested alpha\n"),
        ("nested/beta.txt", "exact-needle nested beta\n"),
    ] {
        std::fs::write(r.join(p), body).unwrap();
    }
    let out = grep(r, json!({"pattern": "exact-needle", "path": "alpha.txt; beta.txt"})).await;
    assert_eq!(text(&out), "# alpha.txt\n*1|exact-needle alpha\n\n# beta.txt\n*1|exact-needle beta");
    assert_eq!(d(&out)["scopePath"], "alpha.txt, beta.txt");
    std::fs::write(r.join("context.txt"), "#if FLAG\nneedle\n#endif\n").unwrap();
    assert_eq!(
        text(&grep(r, json!({"pattern": "needle", "path": "context.txt"})).await),
        " 1|#if FLAG\n*2|needle\n 3|#endif"
    );
}

// B-ca1788ea7b, B-211edc8556, B-4d2dab617d, B-af0141b2fc
#[tokio::test]
async fn walker_pruned_targets_overlaps_and_unrelated_trees() {
    let repo = tempfile::tempdir().unwrap();
    let r = repo.path();
    std::fs::create_dir_all(r.join(".git")).unwrap();
    std::fs::write(r.join(".git/config"), "[push]\n\tfollowTags = true\n").unwrap();
    let out = grep(r, json!({"pattern": "followTags", "path": ".; .git/config"})).await;
    assert!(text(&out).contains("followTags = true"));
    assert_eq!(d(&out)["matchCount"], 1);
    assert_eq!(d(&out)["files"], json!([".git/config"]));

    std::fs::create_dir_all(r.join("src")).unwrap();
    std::fs::write(r.join("src/a.ts"), "needle-dup\n").unwrap();
    let out = grep(r, json!({"pattern": "needle-dup", "path": ".; src/a.ts"})).await;
    assert_eq!(d(&out)["matchCount"], 1);

    let a = tempfile::tempdir_in("/tmp").unwrap();
    let b = tempfile::tempdir_in("/var/tmp").unwrap();
    std::fs::write(a.path().join("alpha.txt"), "shared-needle alpha\n").unwrap();
    std::fs::write(b.path().join("beta.txt"), "shared-needle beta\n").unwrap();
    let started = std::time::Instant::now();
    let path = format!("{}; {}", a.path().display(), b.path().display());
    let out = grep(r, json!({"pattern": "shared-needle", "path": path})).await;
    assert!(started.elapsed().as_secs() < 5, "scan was not rooted at /");
    assert!(text(&out).contains("shared-needle alpha") && text(&out).contains("shared-needle beta"));
    assert_eq!(d(&out)["matchCount"], 2);
}

// B-1a9bab36e6, B-0b3f0ad599, B-be57835ea0, B-5cf487afd0, B-b901f5e49f, B-e193a222e6, B-4fbe802318, B-eaedd5f8b0
#[tokio::test]
async fn glob_path_lists_quotes_and_outside_cwd() {
    let f = fixture();
    let r = f.path();
    let out = glob(r, json!({"path": "apps/*.txt; packages/*.txt"})).await;
    let mut files: Vec<String> = serde_json::from_value(d(&out)["files"].clone()).unwrap();
    files.sort();
    assert_eq!(files, ["apps/grep.txt", "packages/grep.txt"]);
    let out = glob(r, json!({"path": "apps/**/*.txt, packages/**/*.txt"})).await;
    assert_eq!(d(&out)["fileCount"], 2);
    let out = glob(r, json!({"path": "missing.txt, packages/grep.txt"})).await;
    assert!(
        text(&out).starts_with("# packages/\ngrep.txt") && text(&out).contains("Skipped missing paths: missing.txt")
    );
    assert_eq!(d(&out)["files"], json!(["packages/grep.txt"]));
    assert_eq!(d(&out)["missingPaths"], json!(["missing.txt"]));
    let out = glob(r, json!({"path": "folder with spaces/"})).await;
    assert_eq!(text(&out), "# folder with spaces/\nnote.txt");
    assert_eq!(d(&out)["scopePath"], "folder with spaces");
    let out = glob(r, json!({"path": "\"packages/\""})).await;
    assert_eq!(d(&out)["fileCount"], 2);
    assert_eq!(d(&out)["scopePath"], "packages");

    let outside = tempfile::tempdir_in(r.parent().unwrap()).unwrap();
    std::fs::write(outside.path().join("outside.txt"), "outside\n").unwrap();
    let out = glob(r, json!({"path": outside.path().to_str().unwrap()})).await;
    let o = outside.path().to_str().unwrap();
    assert_eq!(text(&out), format!("# {o}/\noutside.txt"));
    assert_eq!(d(&out)["files"], json!([format!("{o}/outside.txt")]));
    assert_eq!(d(&out)["scopePath"], o);
    for root in ["/", "//"] {
        let e = GlobTool::new(plain_ctx(r))
            .execute("c", args(json!({"path": root})), CancellationToken::new(), Arc::new(|_| {}))
            .await
            .unwrap_err();
        assert_eq!(e.0, "Searching from root directory '/' is not allowed");
    }
}

/// Plain display (no edit tool exposed): these cases assert `*N|line` rows.
fn plain_ctx(dir: impl Into<std::path::PathBuf>) -> ToolContext {
    ToolContext::new(dir).with_edit(pi_edit::EditMode::Hashline, false)
}
