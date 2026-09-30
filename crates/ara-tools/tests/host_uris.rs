//! Actual read/write entrypoints with a controlled host content port. Fixed OMP
//! 596f2da: host-uris.ts, read.ts:2294-2375, write.ts:1109-1217 and
//! path-utils.ts:473-525. Arbitrary schemes retain selector-shaped suffixes;
//! predefined schemes (including an explicit skill override) peel selectors.

use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, UserBlock};
use ara_tools::internal_urls::{SkillRef, hierarchical_scheme};
use ara_tools::{ContentUriPort, ContentUriRoute, ToolContext, UriResource, read::ReadTool, write::WriteTool};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct HostPort {
    routes: Mutex<HashMap<String, ContentUriRoute>>,
    resource: Mutex<Option<UriResource>>,
    reads: Mutex<Vec<String>>,
    writes: Mutex<Vec<(String, String)>>,
}

impl HostPort {
    fn new(scheme: &str, writable: bool, content: &str, immutable: bool) -> Arc<Self> {
        let port = Arc::new(Self::default());
        port.routes.lock().unwrap().insert(scheme.to_owned(), ContentUriRoute::Registered { writable });
        *port.resource.lock().unwrap() = Some(UriResource {
            content: content.to_owned(),
            content_type: "text/markdown".into(),
            notes: vec!["host note retained by the resource".into()],
            immutable,
        });
        port
    }
}

#[async_trait]
impl ContentUriPort for HostPort {
    fn route(&self, url: &str) -> ContentUriRoute {
        hierarchical_scheme(url)
            .and_then(|scheme| self.routes.lock().unwrap().get(&scheme.to_ascii_lowercase()).copied())
            .unwrap_or(ContentUriRoute::Unregistered)
    }

    async fn read(&self, url: &str, _cancel: CancellationToken) -> Result<UriResource, ToolError> {
        self.reads.lock().unwrap().push(url.to_owned());
        Ok(self.resource.lock().unwrap().clone().unwrap())
    }

    async fn write(&self, url: &str, content: &str, _cancel: CancellationToken) -> Result<(), ToolError> {
        self.writes.lock().unwrap().push((url.to_owned(), content.to_owned()));
        Ok(())
    }
}

fn args(value: Value) -> JsonObject {
    value.as_object().unwrap().clone()
}

fn noop() -> UpdateFn {
    Arc::new(|_| {})
}

fn text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn read(ctx: ToolContext, path: &str) -> Result<ToolOutput, ToolError> {
    ReadTool { ctx }.execute("read-call", args(json!({"path":path})), CancellationToken::new(), noop()).await
}

async fn write(ctx: ToolContext, path: &str, content: &str) -> Result<ToolOutput, ToolError> {
    WriteTool { ctx }
        .execute("write-call", args(json!({"path":path,"content":content})), CancellationToken::new(), noop())
        .await
}

#[tokio::test]
async fn mutable_host_content_has_numbered_rows_without_a_fake_path_or_file_tag() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("db", true, "标题😀\n次行\r\nlast\n", false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let output = read(ctx, "DB://资料/文档:raw:2-3").await.unwrap();
    // The arbitrary scheme is opaque to the selector parser, exactly upstream.
    assert_eq!(*port.reads.lock().unwrap(), ["DB://资料/文档:raw:2-3"]);
    assert_eq!(text(&output), "1:标题😀\n2:次行\r\n3:last");
    let details = output.details.unwrap();
    assert_eq!(details["contentType"], "text/markdown");
    assert_eq!(details["totalLines"], 3);
    assert_eq!(details["displayContent"], json!({"text":"标题😀\n次行\r\nlast","startLine":1,"lineNumbers":[1,2,3]}));
    assert_eq!(details["meta"]["source"], json!({"type":"internal","value":"DB://资料/文档:raw:2-3"}));
    assert!(details.get("resolvedPath").is_none());
    assert!(details.get("fileSize").is_none());
    assert!(details.get("notes").is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn immutable_oversized_first_line_budgets_content_before_pipe_numbering() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("db", false, &"x".repeat(ara_tools::DEFAULT_MAX_BYTES + 1), true);
    let mut ctx = ToolContext::new(dir.path()).with_uri_port(port);
    ctx.line_numbers = true;
    let output = read(ctx, "db://large").await.unwrap();
    let displayed = format!("1|{}", "x".repeat(ara_tools::DEFAULT_MAX_BYTES));
    assert_eq!(text(&output).lines().next().unwrap(), displayed);
    assert_eq!(output.details.as_ref().unwrap()["displayContent"]["text"], "x".repeat(ara_tools::DEFAULT_MAX_BYTES));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn empty_nonraw_resource_line_keeps_its_display_line_number() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("skill", false, "\n", false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let nonraw = read(ctx.clone(), "skill://empty").await.unwrap();
    assert_eq!(text(&nonraw), "1:");
    assert_eq!(nonraw.details.as_ref().unwrap()["displayContent"]["lineNumbers"], json!([1]));
    port.resource.lock().unwrap().as_mut().unwrap().content.clear();
    let raw = read(ctx, "skill://empty:raw").await.unwrap();
    assert_eq!(text(&raw), "");
    assert_eq!(raw.details.as_ref().unwrap()["displayContent"]["lineNumbers"], json!([]));
}

#[tokio::test]
async fn immutable_host_reads_suppress_hashlines_but_keep_requested_pipe_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("notes", false, "a\nb", true);
    let mut ctx = ToolContext::new(dir.path()).with_uri_port(port);
    assert_eq!(text(&read(ctx.clone(), "notes://one").await.unwrap()), "a\nb");
    ctx.line_numbers = true;
    assert_eq!(text(&read(ctx, "notes://one").await.unwrap()), "1|a\n2|b");
}

#[tokio::test]
async fn explicit_skill_override_uses_selectors_and_removal_does_not_restore_native_file() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("SKILL.md");
    std::fs::write(&file, "native skill content").unwrap();
    let port = HostPort::new("skill", true, "host first\n第二😀\r\nthird\n", false);
    let ctx = ToolContext::new(dir.path())
        .with_skills(vec![SkillRef { name: "demo".into(), file_path: file, base_dir: dir.path().into() }])
        .with_uri_port(port.clone());
    let raw = read(ctx.clone(), "skill://demo:RAW:2-3").await.unwrap();
    assert_eq!(text(&raw), "第二😀\r\nthird\n\n[1 more lines in resource. Use :4 to continue]");
    assert_eq!(port.reads.lock().unwrap().as_slice(), ["skill://demo"]);
    let multi = read(ctx.clone(), "skill://demo:raw:1-1,3-3").await.unwrap();
    assert_eq!(text(&multi), "host first\n\n…\n\nthird");
    for malformed in ["skill://demo:raw:raw", "skill://demo:raw:-5-2", "skill://demo:0"] {
        assert!(read(ctx.clone(), malformed).await.unwrap_err().0.starts_with("Invalid selector ':"));
    }
    assert_eq!(port.reads.lock().unwrap().len(), 2, "invalid selectors fail before host reads");
    port.routes.lock().unwrap().insert("skill".into(), ContentUriRoute::Removed);
    assert_eq!(read(ctx.clone(), "skill://demo").await.unwrap_err().0, "Unknown protocol: skill://");
    assert_eq!(write(ctx, "skill://demo", "overwrite").await.unwrap_err().0, "Unknown protocol: skill://");
    assert!(port.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn custom_host_resources_remain_capped_and_only_skill_is_exempt() {
    let dir = tempfile::tempdir().unwrap();
    let content = (1..=3005).map(|n| format!("line{n}")).collect::<Vec<_>>().join("\n");
    let port = HostPort::new("db", false, &content, true);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let capped = read(ctx.clone(), "db://big").await.unwrap();
    assert!(text(&capped).contains("[5 more lines in resource. Use :3001 to continue]"));
    assert!(!text(&capped).contains("line3001\n"));
    assert_eq!(capped.details.unwrap()["truncation"]["truncatedBy"], "lines");
    port.routes.lock().unwrap().insert("skill".into(), ContentUriRoute::Registered { writable: false });
    let uncapped = read(ctx, "skill://big").await.unwrap();
    assert_eq!(text(&uncapped), content);
    assert!(uncapped.details.unwrap().get("truncation").is_none());
}

#[tokio::test]
async fn byte_cap_does_not_emit_a_mutable_partial_hashline() {
    let dir = tempfile::tempdir().unwrap();
    let content = "字".repeat(ara_tools::DEFAULT_MAX_BYTES / 3 + 4);
    let port = HostPort::new("db", false, &content, false);
    let output = read(ToolContext::new(dir.path()).with_uri_port(port), "db://huge").await.unwrap();
    let body = text(&output);
    assert!(body.contains("Hashline output requires full lines"), "{body}");
    assert!(!body.contains("1:字"));
    assert_eq!(output.details.unwrap()["truncation"]["truncatedBy"], "bytes");
}

#[tokio::test]
async fn resource_byte_budget_is_applied_before_numbering_and_multi_ranges_remain_uncapped() {
    let dir = tempfile::tempdir().unwrap();
    let content = "x".repeat(ara_tools::DEFAULT_MAX_BYTES);
    let port = HostPort::new("memory", false, &content, false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let exact_cap = read(ctx.clone(), "memory://exact").await.unwrap();
    assert_eq!(text(&exact_cap), format!("1:{content}"));
    assert!(exact_cap.details.unwrap().get("truncation").is_none());
    let lines = (1..=3006).map(|n| format!("row{n}")).collect::<Vec<_>>().join("\n");
    port.resource.lock().unwrap().as_mut().unwrap().content = lines;
    let multi = read(ctx, "memory://many:1-3001,3003-3005").await.unwrap();
    assert!(text(&multi).contains("3001:row3001\n…\n3003:row3003"));
    assert!(text(&multi).ends_with("3005:row3005"));
    assert!(multi.details.unwrap().get("truncation").is_none());
}

#[tokio::test]
async fn host_multi_range_lexical_boundaries_match_model_rows_and_display_content() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("skill", false, "{\na\nb\nc\nd\n}\n", false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port);
    let output = read(ctx.clone(), "skill://doc:1-1,3-3").await.unwrap();
    assert_eq!(text(&output), "1:{\n…\n3:b\n…\n6:}");
    assert_eq!(
        output.details.as_ref().unwrap()["displayContent"],
        json!({"text":"{\n…\nb\n…\n}","startLine":1,"lineNumbers":[1,null,3,null,6]})
    );
    assert!(output.details.as_ref().unwrap().get("resolvedPath").is_none());
    let raw = read(ctx, "skill://doc:raw:1-1,3-3").await.unwrap();
    assert_eq!(text(&raw), "{\n\n…\n\nb");
    assert!(raw.details.unwrap().get("displayContent").is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn host_single_range_keeps_padding_notice_and_adds_lexical_boundary_without_anchors() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("skill", false, "{\na\nb\nc\nd\n}\n", true);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let output = read(ctx.clone(), "skill://doc:1-1").await.unwrap();
    assert_eq!(text(&output), "{\na\nb\nc\n…\n}\n\n[2 more lines in resource. Use :5 to continue]");
    assert_eq!(
        output.details.as_ref().unwrap()["displayContent"],
        json!({"text":"{\na\nb\nc\n…\n}","startLine":1,"lineNumbers":[1,2,3,4,null,6]})
    );
    let raw = read(ctx.clone(), "skill://doc:raw:1-1").await.unwrap();
    assert_eq!(text(&raw), "{\n\n[6 more lines in resource. Use :2 to continue]");
    assert_eq!(raw.details.unwrap()["displayContent"], json!({"text":"{","startLine":1,"lineNumbers":[1]}));
    port.resource.lock().unwrap().as_mut().unwrap().content = "a\nb\nc\nd\ne\nf\ng\n".into();
    let plain = read(ctx, "skill://doc:3-3").await.unwrap();
    assert_eq!(text(&plain), "b\nc\nd\ne\nf\n\n[1 more lines in resource. Use :7 to continue]");
    assert_eq!(
        plain.details.unwrap()["displayContent"],
        json!({"text":"b\nc\nd\ne\nf","startLine":2,"lineNumbers":[2,3,4,5,6]})
    );
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn host_write_receives_complete_stripped_content_and_counts_utf16_units() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("skill", true, "", false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    let output = write(ctx.clone(), "[skill://draft:raw#ABCD]", "[skill://draft#ABCD]\n1:标题😀\n2:end").await.unwrap();
    assert_eq!(*port.writes.lock().unwrap(), [("skill://draft".into(), "标题😀\nend".into())]);
    assert_eq!(
        text(&output),
        "Successfully wrote 8 bytes to skill://draft\nNote: auto-stripped hashline display prefixes from content before writing."
    );
    assert_eq!(output.details, Some(json!({})));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    for path in ["skill://draft:1-3", "skill://draft:raw:1-3", "skill://draft:-3-2"] {
        assert!(
            write(ctx.clone(), path, "replacement")
                .await
                .unwrap_err()
                .0
                .starts_with("write does not accept the trailing selector")
        );
    }
    assert_eq!(port.writes.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn readonly_and_unknown_uri_writes_never_fall_back_to_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let port = HostPort::new("db", false, "", false);
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    assert_eq!(
        write(ctx.clone(), "db://table:raw", "row").await.unwrap_err().0,
        "db:// URLs are read-only for write; use the protocol-specific tool for mutations."
    );
    assert_eq!(read(ctx.clone(), "unknown://table").await.unwrap_err().0, "Unknown protocol: unknown://");
    assert_eq!(write(ctx, "unknown://table", "row").await.unwrap_err().0, "Unknown protocol: unknown://");
    assert!(port.writes.lock().unwrap().is_empty());
    assert!(port.reads.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn installing_port_updates_existing_tool_clones_but_not_other_contexts() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext::new(dir.path());
    let read_tool = ReadTool { ctx: ctx.clone() };
    let separate_ctx = ToolContext::new(dir.path());
    let port = HostPort::new("db", true, "late installed", true);
    ctx.set_uri_port(port);
    let output =
        read_tool.execute("late", args(json!({"path":"db://one"})), CancellationToken::new(), noop()).await.unwrap();
    assert_eq!(text(&output), "late installed");
    assert!(separate_ctx.content_uri_port().is_none());
    assert_eq!(read(separate_ctx, "db://one").await.unwrap_err().0, "Unknown protocol: db://");
}

#[derive(Default)]
struct WaitingPort {
    started: tokio::sync::Notify,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl ContentUriPort for WaitingPort {
    fn route(&self, _: &str) -> ContentUriRoute {
        ContentUriRoute::Registered { writable: true }
    }
    async fn read(&self, url: &str, cancel: CancellationToken) -> Result<UriResource, ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.notify_one();
        cancel.cancelled().await;
        Err(ToolError(format!("Host URI read for {url} was aborted")))
    }
    async fn write(&self, url: &str, _: &str, cancel: CancellationToken) -> Result<(), ToolError> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.started.notify_one();
        cancel.cancelled().await;
        Err(ToolError(format!("Host URI write for {url} was aborted")))
    }
}

#[tokio::test]
async fn cancellation_before_dispatch_and_while_waiting_reaches_the_host_port() {
    let dir = tempfile::tempdir().unwrap();
    let port = Arc::new(WaitingPort::default());
    let ctx = ToolContext::new(dir.path()).with_uri_port(port.clone());
    for tool in [Arc::new(ReadTool { ctx: ctx.clone() }) as Arc<dyn AgentTool>, Arc::new(WriteTool { ctx })] {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error =
            tool.execute("before", args(json!({"path":"db://wait","content":"x"})), cancel, noop()).await.unwrap_err();
        assert!(error.0.ends_with("was aborted"));
        assert_eq!(port.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        let cancel = CancellationToken::new();
        let execution = tool.execute("during", args(json!({"path":"db://wait","content":"x"})), cancel.clone(), noop());
        tokio::pin!(execution);
        tokio::select! {
            _ = port.started.notified() => {}
            result = &mut execution => panic!("host must wait: {result:?}"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("host not called"),
        }
        cancel.cancel();
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), execution).await.unwrap();
        assert!(result.unwrap_err().0.ends_with("was aborted"));
        port.calls.store(0, std::sync::atomic::Ordering::SeqCst);
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
