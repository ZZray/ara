//! Complete fixed read-summary.test.ts original16 bodies/fixtures.
//! MIT / Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
use ara_agent::{AgentTool, ToolOutput};
use ara_ai::UserBlock;
use ara_tools::{ToolContext, read::ReadTool, read_summary::SummaryPolicy};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;
const TWO: &str = "export function alpha(value: string): string {\n\tconst clean = value.trim();\n\tconst label = clean || 'alpha';\n\treturn label.toUpperCase();\n}\n\nexport function beta(): number {\n\tconst one = 1;\n\tconst two = 2;\n\treturn one + two;\n}\n";
const ONE: &str =
    "export function alpha(): string {\n\tconst clean = 'alpha';\n\treturn clean;\n}\n";
const MARKDOWN: &str = "# Heading\n\nIntro line.\n\n```ts\nexport function alpha(): string {\n\tconst clean = 'alpha';\n\treturn clean;\n}\n```\n\nMore prose.\n";
fn legacy() -> SummaryPolicy {
    SummaryPolicy {
        min_total_lines: 0.0,
        unfold_until: Some(0.0),
        unfold_limit: Some(0.0),
        ..Default::default()
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    ctx: ToolContext,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path()).with_read_summary_policy(legacy());
        Self { dir, ctx }
    }
    fn write(&self, path: &str, content: &str) -> PathBuf {
        let path = self.dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }
}
async fn read(ctx: &ToolContext, path: impl AsRef<Path>) -> ToolOutput {
    ReadTool { ctx: ctx.clone() }
        .execute(
            "read-summary",
            json!({"path":path.as_ref().to_string_lossy()})
                .as_object()
                .unwrap()
                .clone(),
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .unwrap()
}
fn text(result: &ToolOutput) -> String {
    result
        .content
        .iter()
        .filter_map(|content| {
            if let UserBlock::Text(content) = content {
                Some(content.text.as_str())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn details(result: &ToolOutput) -> &Value {
    result.details.as_ref().unwrap()
}
fn matches(pattern: &str, text: &str) {
    assert!(regex::Regex::new(pattern).unwrap().is_match(text), "{text}");
}
fn absent_summary(result: &ToolOutput) {
    assert!(details(result).get("summary").is_none());
}

#[tokio::test]
async fn original_read_summary_typescript() {
    // B-3e364e6a16
    let f = Fixture::new();
    let path = f.write("src/fixture.ts", TWO);
    let output = read(&f.ctx, &path).await;
    let text = text(&output);
    matches(
        r"^\[src/fixture\.ts#[0-9A-F]{4}\]$",
        text.lines().next().unwrap(),
    );
    assert!(text.contains("export function alpha(value: string): string { … }"));
    assert!(text.contains("export function beta(): number { … }"));
    assert!(!text.contains("const clean = value.trim()"));
    assert_eq!(details(&output)["summary"]["elidedSpans"], 2);
}
#[tokio::test]
async fn original_read_summary_resolved_suffix() {
    // B-488dd21427
    let f = Fixture::new();
    f.write("project/src/fixture.ts", TWO);
    let output = read(&f.ctx, "src/fixture.ts").await;
    let body = text(&output);
    assert_eq!(
        details(&output)["suffixResolution"]["to"],
        "project/src/fixture.ts"
    );
    assert!(body.contains("with project/src/fixture.ts:1-5,7-11]"));
    assert!(!body.contains("with src/fixture.ts:"));
}
#[tokio::test]
async fn original_read_summary_markdown_prose() {
    // B-46a8ef311e
    let f = Fixture::new();
    let path = f.write("fixture.md", MARKDOWN);
    let default = read(&f.ctx, &path).await;
    assert!(text(&default).contains("const clean = 'alpha';"));
    absent_summary(&default);
    let prose = f.ctx.clone().with_read_summary_policy(SummaryPolicy {
        prose: true,
        ..legacy()
    });
    let output = read(&prose, &path).await;
    assert!(!text(&output).contains("const clean = 'alpha';"));
    assert_eq!(details(&output)["summary"]["elidedSpans"], 1);
}
#[tokio::test]
async fn original_read_summary_markdown_preview_extensions() {
    // B-70febd13d2: preserve all six original extensions and all assertions.
    let f = Fixture::new();
    let preview = f.ctx.clone().with_read_render_markdown(true);
    for ext in ["md", "markdown", "mdx", "mdc", "mkd", "mdown"] {
        let path = f.write(
            &format!("fixture.{ext}"),
            "# Heading\n\nSome **bold** text.\n",
        );
        let default = read(&f.ctx, &path).await;
        assert!(details(&default).get("contentType").is_none());
        let output = read(&preview, &path).await;
        let body = text(&output);
        assert_eq!(details(&output)["contentType"], "text/markdown");
        assert_eq!(
            details(&output)["displayContent"]["text"],
            "# Heading\n\nSome **bold** text."
        );
        matches(
            &format!(r"^\[fixture\.{ext}#[0-9A-F]{{4}}\]$"),
            body.lines().next().unwrap(),
        );
        assert!(body.contains("1:# Heading"));
        assert!(body.contains("3:Some **bold** text."));
    }
}
#[tokio::test]
async fn original_read_summary_mdx_verbatim() {
    // B-446fd26e97
    let f = Fixture::new();
    let path = f.write("fixture.mdx", MARKDOWN);
    let output = read(&f.ctx, &path).await;
    assert!(text(&output).contains("const clean = 'alpha';"));
    absent_summary(&output);
}
#[tokio::test]
async fn original_read_summary_no_truncation() {
    // B-a812b83caf: actual production read.defaultLimit=10, not a fixture-only flag.
    let f = Fixture::new();
    let source=(0..20).map(|index|format!("export function fn{index}(): number {{\n\tconst one = {index};\n\tconst two = {};\n\treturn one + two;\n}}",index+1)).collect::<Vec<_>>().join("\n\n");
    let path = f.write("many.ts", &(source + "\n"));
    let ctx = f.ctx.clone().with_read_default_limit(10.0);
    let output = read(&ctx, &path).await;
    let body = text(&output);
    assert!(body.contains("export function fn19(): number {"));
    assert!(!body.contains("[Showing lines"));
    assert!(details(&output).get("truncation").is_none());
    assert_eq!(details(&output)["summary"]["elidedSpans"], 20);
}
#[tokio::test]
async fn original_read_summary_explicit_range() {
    // B-6d31ffffb4
    let f = Fixture::new();
    let path = f.write("fixture.ts", ONE);
    let output = read(&f.ctx, format!("{}:1-9999", path.display())).await;
    let body = text(&output);
    assert!(body.contains("const clean = 'alpha';"));
    assert!(!body.contains('…'));
    absent_summary(&output);
}
#[tokio::test]
async fn original_read_summary_raw() {
    // B-b96966a904
    let f = Fixture::new();
    let path = f.write("fixture.ts", "export const value = 1;\n");
    let output = read(&f.ctx, format!("{}:raw", path.display())).await;
    let body = text(&output);
    assert_eq!(body, "export const value = 1;\n");
    assert!(!regex::Regex::new(r"^1[a-z]{2}\|").unwrap().is_match(&body));
}
#[tokio::test]
async fn original_read_summary_compound_raw() {
    // B-db743d73e8
    let f = Fixture::new();
    let path = f.write("compound.ts", "alpha\nbeta\ngamma\ndelta\nepsilon\n");
    let first = read(&f.ctx, format!("{}:2-4:raw", path.display())).await;
    let body = text(&first);
    for line in ["beta", "gamma", "delta"] {
        assert!(body.contains(line));
    }
    assert!(
        !regex::Regex::new(r"(?m)^\s*\d+[a-z]{2}\|")
            .unwrap()
            .is_match(&body)
    );
    assert!(!regex::Regex::new(r"(?m)^\s*\d+\|").unwrap().is_match(&body));
    let second = read(&f.ctx, format!("{}:raw:2-4", path.display())).await;
    assert_eq!(text(&second), body);
}
#[tokio::test]
async fn original_read_summary_disabled_or_parse_failure() {
    // B-ca65912949
    let f = Fixture::new();
    let valid = f.write("valid.ts", ONE);
    let broken = f.write("broken.ts", "export function broken( {\n");
    let disabled = f.ctx.clone().with_read_summary_policy(SummaryPolicy {
        enabled: false,
        ..legacy()
    });
    let output = read(&disabled, &valid).await;
    assert!(text(&output).contains("const clean = 'alpha';"));
    absent_summary(&output);
    let output = read(&f.ctx, &broken).await;
    assert!(text(&output).contains("export function broken( {"));
    absent_summary(&output);
}
#[tokio::test]
async fn original_read_summary_sqlite_colon() {
    // B-55a95add86: actual SQLite process/library owner; fixture unchanged.
    let f = Fixture::new();
    let path = f.dir.path().join("data.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL); INSERT INTO users (id, name) VALUES (42, 'Ada');").unwrap();
    }
    let output = read(&f.ctx, format!("{}:users:42", path.display())).await;
    let body = text(&output);
    assert!(body.contains("id: 42"));
    assert!(body.contains("name: Ada"));
}
#[tokio::test]
async fn original_read_summary_brace_merge() {
    // B-503c69f72d
    let f = Fixture::new();
    let path=f.write("merge.ts","export function stripNewLinePrefixes(lines: string[]): string[] {\n\tconst out: string[] = [];\n\tfor (const line of lines) {\n\t\tout.push(line.replace(/^\\n+/, ''));\n\t}\n\treturn out;\n}\n");
    let output = read(&f.ctx, &path).await;
    let body = text(&output);
    assert!(body.contains("export function stripNewLinePrefixes(lines: string[]): string[] { … }"));
    assert!(!body.contains("\n…\n"));
    matches(r"\b1-7:export function stripNewLinePrefixes", &body);
    assert_eq!(details(&output)["summary"]["elidedSpans"], 1);
}
#[tokio::test]
async fn original_read_summary_trailing_punctuation() {
    // B-588bdfc7ba
    let f = Fixture::new();
    let path = f.write(
        "object.ts",
        "export const config = {\n\talpha: 1,\n\tbeta: 2,\n\tgamma: 3,\n\tdelta: 4,\n};\n",
    );
    let output = read(&f.ctx, &path).await;
    let body = text(&output);
    assert!(body.contains("export const config = { … };"));
    assert!(!body.contains("\n…\n"));
}
#[tokio::test]
async fn original_read_summary_python_no_merge() {
    // B-ac32240deb
    let f = Fixture::new();
    let path=f.write("fixture.py","def greet(name: str) -> str:\n    clean = name.strip()\n    label = clean or 'world'\n    upper = label.upper()\n    return f'hello {upper}'\n");
    let output = read(&f.ctx, &path).await;
    let body = text(&output);
    assert!(body.contains("def greet(name: str) -> str:"));
    assert!(body.contains("\n…\n"));
    assert!(!body.contains(" … "));
}
#[tokio::test]
async fn original_read_summary_footer_last() {
    // B-91432c3330
    let f = Fixture::new();
    let path = f.write("footer.ts", TWO);
    let output = read(&f.ctx, &path).await;
    let body = text(&output);
    assert_eq!(details(&output)["summary"]["elidedSpans"], 2);
    assert!(details(&output)["summary"]["elidedLines"].as_u64().unwrap() > 0);
    assert!(body.contains("ln elided"));
    assert!(body.contains("footer.ts:1-5,7-11"));
    assert!(!body.contains("footer.ts:raw"));
    assert!(!body.contains("footer.ts:1-9999"));
    assert!(body.trim_end().ends_with(']'));
}
#[tokio::test]
async fn original_read_summary_no_footer() {
    // B-df9597b5f2
    let f = Fixture::new();
    let path = f.write("noelide.ts", "export const x = 1;\n");
    let output = read(&f.ctx, &path).await;
    let body = text(&output);
    assert!(!body.contains("elided regions"));
    assert!(!body.contains(":raw"));
    absent_summary(&output);
}

// Production supplement: source selection order and real captured limit values.
// Controlled ACP fault fixtures are bounded; no assertion accepts actual ACP/UI.
struct SummaryBridge {
    calls: std::sync::atomic::AtomicUsize,
    replies: std::sync::Mutex<std::collections::VecDeque<Result<String, String>>>,
}
impl SummaryBridge {
    fn new(replies: Vec<Result<String, String>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Default::default(),
            replies: std::sync::Mutex::new(replies.into()),
        })
    }
    fn count(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}
#[async_trait::async_trait]
impl ara_tools::edit_host::EditClientBridge for SummaryBridge {
    fn can_write_text_file(&self) -> bool {
        false
    }
    fn can_read_text_file(&self) -> bool {
        true
    }
    async fn read_text_file(&self, _path: &Path) -> std::io::Result<String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra ACP acquisition")
            .map_err(std::io::Error::other)
    }
    async fn write_text_file(&self, _path: &Path, _content: &str) -> std::io::Result<()> {
        unreachable!("READ never writes ACP")
    }
}
struct SummaryHost(Arc<SummaryBridge>);
impl ara_tools::edit_host::EditHostPort for SummaryHost {
    fn client_bridge(&self) -> Option<Arc<dyn ara_tools::edit_host::EditClientBridge>> {
        Some(self.0.clone())
    }
}
#[tokio::test]
async fn read_summary_production_fresh_acp_failure_gates_and_disk_snapshot() {
    let f = Fixture::new();
    let path = f.write("acp.ts", &TWO.replace("alpha", "disk"));
    let bridge = SummaryBridge::new(vec![Ok(TWO.into())]);
    let ctx = f
        .ctx
        .clone()
        .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
    let output = read(&ctx, &path).await;
    assert_eq!(bridge.count(), 1);
    assert!(text(&output).contains("alpha"));
    assert!(details(&output).get("summary").is_some());
    let normalized = TWO.replace("alpha", "disk");
    let tag = ara_edit::store::file_hash(&normalized);
    assert!(text(&output).starts_with(&format!("[acp.ts#{tag}]\n")));
    let key = ara_edit::path_policy::canonical_key(&path);
    assert_eq!(
        &*ctx.edit_store.by_hash(&key, &tag).unwrap().text,
        normalized
    );
    // Failed/unusable Summary obtains a SECOND fresh volatile ACP snapshot.
    let second = "SECOND1\nSECOND2\nSECOND3\nSECOND4\n";
    for first in [
        Ok("export function broken( {\n".into()),
        Ok("export const x = 1;\n".into()),
    ] {
        let bridge = SummaryBridge::new(vec![first, Ok(second.into())]);
        let ctx = f
            .ctx
            .clone()
            .with_read_default_limit(1.0)
            .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
        let output = read(&ctx, &path).await;
        assert_eq!(bridge.count(), 2);
        assert!(text(&output).contains("SECOND4"));
        absent_summary(&output);
    }
    // Bridge failure inside Summary uses disk and can succeed without retry.
    let bridge = SummaryBridge::new(vec![Err("summary bridge failed".into())]);
    let ctx = f
        .ctx
        .clone()
        .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
    assert!(details(&read(&ctx, &path).await).get("summary").is_some());
    assert_eq!(bridge.count(), 1);
    // Unusable first ACP then ordinary ACP fault falls back to ordinary disk.
    let bridge = SummaryBridge::new(vec![
        Ok("export const x = 1;\n".into()),
        Err("second failed".into()),
    ]);
    let ctx = f
        .ctx
        .clone()
        .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
    let output = read(&ctx, &path).await;
    assert_eq!(bridge.count(), 2);
    absent_summary(&output);
    assert!(text(&output).contains("const label = clean || 'disk'"));
    let md = f.write("gated.md", TWO);
    let large = f.write("gated.ts", &format!("// {}\n", "x".repeat(2 * 1024 * 1024)));
    for (name, disabled) in [
        (path.to_string_lossy().to_string(), true),
        (md.to_string_lossy().to_string(), false),
        (format!("{}:1-2", path.display()), false),
        (large.to_string_lossy().to_string(), false),
    ] {
        let bridge = SummaryBridge::new(vec![Ok(second.into())]);
        let ctx = f
            .ctx
            .clone()
            .with_read_summary_policy(SummaryPolicy {
                enabled: !disabled,
                ..legacy()
            })
            .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
        let output = read(&ctx, name).await;
        assert_eq!(bridge.count(), 1);
        absent_summary(&output);
    }
    // The disk sniff precedes Summary ACP, exactly as the fixed plain route.
    let binary = f.dir.path().join("binary.ts");
    std::fs::write(&binary, [0u8, 1, 2]).unwrap();
    let bridge = SummaryBridge::new(vec![]);
    let ctx = f
        .ctx
        .clone()
        .with_edit_host(Arc::new(SummaryHost(bridge.clone())));
    assert!(text(&read(&ctx, &binary).await).contains("Cannot read binary"));
    assert_eq!(bridge.count(), 0);
}
#[tokio::test]
async fn read_default_limit_actual_disk_fractional_nan_selector_owners() {
    let f = Fixture::new();
    let path = f.write(
        "limits.txt",
        &(1..=400).map(|n| format!("L{n}\n")).collect::<String>(),
    );
    let disabled = SummaryPolicy {
        enabled: false,
        ..legacy()
    };
    for (limit, last) in [
        (300.0, 300),
        (-1.0, 1),
        (2.5, 3),
        (4000.0, 400),
        (f64::NAN, 400),
    ] {
        let ctx = f
            .ctx
            .clone()
            .with_read_summary_policy(disabled)
            .with_read_default_limit(limit);
        let body = text(&read(&ctx, &path).await);
        assert!(body.contains(&format!("{last}:L{last}")));
        if last < 400 {
            assert!(!body.contains(&format!("{}:L{}", last + 1, last + 1)));
        }
    }
    let ctx = f
        .ctx
        .clone()
        .with_read_summary_policy(disabled)
        .with_read_default_limit(2.5);
    let body = text(&read(&ctx, format!("{}:raw:10-", path.display())).await);
    assert!(body.contains("L12"));
    assert!(!body.contains("L13\n"));
    let body = text(&read(&ctx, format!("{}:raw:10-14", path.display())).await);
    assert!(body.contains("L14"));
    assert!(!body.contains("L15\n"));
    // Buffered nonraw multi Array.slice floors 2.5; raw collector counts ceil.
    let body = text(&read(&ctx, format!("{}:1-1,10-", path.display())).await);
    assert!(body.contains("10:L10"));
    assert!(body.contains("11:L11"));
    assert!(!body.contains("12:L12"));
    let body = text(&read(&ctx, format!("{}:raw:1-1,10-", path.display())).await);
    assert!(body.contains("L12"));
    assert!(!body.contains("L13\n"));
    let nan = ctx.with_read_default_limit(f64::NAN);
    let body = text(&read(&nan, format!("{}:1-1,10-", path.display())).await);
    assert!(body.contains("1:L1"));
    assert!(!body.contains("10:L10"));

    // Fixed read-single-pass.test.ts: raw byte lengths must survive UTF-8
    // replacement expansion past the 8KiB binary sniff. Preserve its 60KiB
    // fixture and 512 stray bytes, not a decoded-length substitute.
    let source_ctx = f.ctx.clone().with_read_summary_policy(disabled);
    let invalid = f.dir.path().join("invalid-utf8.txt");
    let mut bytes = vec![b'z'; 60 * 1024 - 512];
    bytes.extend(std::iter::repeat_n(0xff, 512));
    bytes.extend_from_slice(b"\ntail\n");
    std::fs::write(&invalid, bytes).unwrap();
    let body = text(&read(&source_ctx, format!("{}:1-1", invalid.display())).await);
    assert!(body.contains("[Line 1 is 60.0KB, exceeds 50.0KB limit"));
    assert!(!body.contains("61.0KB"));

    // Source budget includes only source rows and separators, never prefixes.
    // Exactly 50KiB therefore fits even with the hashline prefix present.
    let boundary = f.write("boundary.txt", &format!("{}\ntail\n", "z".repeat(50 * 1024)));
    let result = read(&source_ctx, format!("{}:1-1", boundary.display())).await;
    assert!(!text(&result).contains("exceeds 50.0KB"));
    assert!(text(&result).contains("1:"));

    // Single rows retain raw BOM/CR while the editable snapshot is stripped
    // and normalized. Nonraw multi remains the normalized fullLines owner.
    let views = f.write("views.txt", "\u{feff}alpha\r\nbeta\r\ngamma\r\n");
    let single = text(&read(&source_ctx, &views).await);
    assert!(single.contains("1:\u{feff}alpha\r\n2:beta\r"));
    let normalized = "alpha\nbeta\ngamma\n";
    let snapshot = source_ctx.edit_store.by_content(&ara_edit::path_policy::canonical_key(&views), normalized);
    assert_eq!(snapshot.unwrap().text.as_ref(), normalized);
    // Adjacent 1-1,2-2 merges to a SINGLE range upstream. Keep a gap so this
    // observation actually exercises the nonraw multi normalized-text owner.
    let multi = text(&read(&source_ctx, format!("{}:1-1,3-3", views.display())).await);
    assert!(multi.contains("1:alpha"));
    assert!(!multi.contains('\u{feff}'));
    assert!(!multi.contains('\r'));
}
