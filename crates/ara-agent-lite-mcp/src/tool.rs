//! `McpTool`: one server tool as a Core `AgentTool` (requirements 42.5, 42.6).
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use ara_agent_lite_core::{
    rpi_agent::{AgentError, AgentTool, AgentToolResult, TextContentOrImage, ToolResultPartial},
    rpi_ai::{ImageContent, Schema, Tool},
};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{client::Client, transport::CallError};

/// Error text longer than this keeps its head and tail.
pub const ERROR_LIMIT: usize = 11_000;
/// Characters kept at each end of an over-long error.
pub const ERROR_KEEP: usize = 5_000;
/// Success text longer than this is saved to `results_dir` or truncated.
pub const SUCCESS_LIMIT: usize = 50_000;
/// Excerpt returned with a saved full result.
pub const EXCERPT: usize = 2_000;

/// `isError` text: head and tail with the omitted character count.
pub fn truncate_error(text: &str) -> String {
    let count = text.chars().count();
    if count <= ERROR_LIMIT {
        return text.to_string();
    }
    let head: String = text.chars().take(ERROR_KEEP).collect();
    let tail: String = text.chars().skip(count - ERROR_KEEP).collect();
    format!("{head}\n\n[... {} characters omitted ...]\n\n{tail}", count - 2 * ERROR_KEEP)
}

fn image_note(mime: &str, data: &str) -> String {
    match base64::engine::general_purpose::STANDARD.decode(data) {
        Ok(bytes) => format!("[image: {mime}, {} bytes]", bytes.len()),
        Err(_) => format!("[image: {mime}, invalid base64 data omitted]"),
    }
}

/// Convert MCP `content` items: `text` → text, valid `image` → image (or a
/// MIME/byte-count note when `images` is false), anything else → JSON text.
fn content_parts(items: &[Value], images: bool) -> (Vec<String>, Vec<ImageContent>) {
    let mut texts = Vec::new();
    let mut pictures = Vec::new();
    for item in items {
        let kind = item.get("type").and_then(Value::as_str);
        match (kind, item.get("text").and_then(Value::as_str)) {
            (Some("text"), Some(text)) => texts.push(text.to_string()),
            _ => match (kind, item.get("data").and_then(Value::as_str), item.get("mimeType").and_then(Value::as_str)) {
                (Some("image"), Some(data), Some(mime)) => {
                    let valid = base64::engine::general_purpose::STANDARD.decode(data).is_ok();
                    if images && valid {
                        pictures.push(ImageContent {
                            kind: Default::default(),
                            data: data.to_string(),
                            mime_type: mime.to_string(),
                        });
                    } else {
                        texts.push(image_note(mime, data));
                    }
                }
                _ => texts.push(item.to_string()),
            },
        }
    }
    (texts, pictures)
}

/// Convert a `tools/call` (or `resources/read`) result. `Err` is the tool
/// error text for `isError: true`; `Ok` carries content and details.
pub fn convert_result(
    server: &str,
    tool: &str,
    call_id: &str,
    result: &Value,
    results_dir: Option<&Path>,
) -> Result<AgentToolResult, String> {
    let items = result.get("content").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let structured = result.get("structuredContent").filter(|value| !value.is_null());
    let failed = result.get("isError").and_then(Value::as_bool) == Some(true);
    let (texts, images) = content_parts(items, !failed);
    let mut text = texts.join("\n");
    if failed {
        if let Some(structured) = structured {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&format!("structuredContent: {structured}"));
        }
        if text.is_empty() {
            text = "MCP tool reported an error without text.".into();
        }
        return Err(truncate_error(&text));
    }
    if text.is_empty() && images.is_empty() {
        text = structured.map_or_else(|| "(MCP tool returned no content)".to_string(), Value::to_string);
    }
    let mut details = json!({"server": server, "tool": tool});
    if let Some(structured) = structured {
        details["structuredContent"] = structured.clone();
    }
    let count = text.chars().count();
    if count > SUCCESS_LIMIT {
        details["original_chars"] = json!(count);
        let saved = results_dir.and_then(|dir| {
            let name = format!(
                "mcp-{}-{}.txt",
                crate::names::normalize(&crate::text::bounded(call_id, 64)),
                ara_agent_lite_core::fresh_id()
            );
            let path = dir.join(name);
            std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, &text)).ok().map(|()| path)
        });
        let excerpt_len = if saved.is_some() { EXCERPT } else { SUCCESS_LIMIT };
        let excerpt: String = text.chars().take(excerpt_len).collect();
        text = match saved {
            Some(path) => {
                details["full_result_path"] = json!(path.display().to_string());
                format!(
                    "MCP result has {count} characters; the full text is saved to {}. First {EXCERPT} characters:\n{excerpt}",
                    path.display()
                )
            }
            None => {
                details["truncated"] = json!(true);
                format!("{excerpt}\n\n[... result truncated: {} characters omitted ...]", count - SUCCESS_LIMIT)
            }
        };
    }
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(TextContentOrImage::text(text));
    }
    content.extend(images.into_iter().map(TextContentOrImage::Image));
    Ok(AgentToolResult { content, details, ..Default::default() })
}

/// Structured unknown-effect result (requirement 22 contract).
pub fn unknown_effect(server: &str, tool: &str, source: &str, what: &str) -> AgentToolResult {
    let mut result = AgentToolResult::text(format!(
        "MCP call {tool} on server {server} {what}; the server may already have executed it. External effects are unknown: inspect the actual state before any retry."
    ));
    result.details = json!({"effects_unknown": true, "source": source, "server": server, "tool": tool});
    result
}

/// Map a transport failure. Interrupted calls of non-read-only tools carry
/// structured unknown effects; nothing is retried.
pub(crate) fn interrupted(
    server: &str,
    tool: &str,
    read_only: bool,
    timeout: Duration,
    error: CallError,
) -> Result<AgentToolResult, AgentError> {
    let (source, what) = match &error {
        CallError::Timeout => ("mcp_call_timeout", format!("timed out after {} s", timeout.as_secs_f64())),
        CallError::Cancelled => ("mcp_call_cancelled", "was cancelled (notifications/cancelled sent)".to_string()),
        CallError::Lost(reason) => ("mcp_connection_lost", format!("lost its connection before the reply ({reason})")),
        CallError::NotSent(_) | CallError::Rejected(_) | CallError::Protocol(_) => {
            return Err(AgentError::Tool(format!("MCP call {tool} on server {server} failed: {error}")))
        }
    };
    if read_only {
        return Err(AgentError::Tool(format!(
            "MCP call {tool} on server {server} {what}; the tool is read-only, so no external effects."
        )));
    }
    Ok(unknown_effect(server, tool, source, &what))
}

pub(crate) struct McpTool {
    pub(crate) schema: Tool,
    pub(crate) server: String,
    pub(crate) tool: String,
    pub(crate) read_only: bool,
    pub(crate) client: Arc<Client>,
    pub(crate) call_timeout: Duration,
    pub(crate) results_dir: Option<PathBuf>,
}

#[async_trait]
impl AgentTool for McpTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        &self.schema.name
    }

    async fn execute(
        &self,
        call_id: &str,
        params: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let arguments = if params.is_null() { json!({}) } else { params };
        if !arguments.is_object() {
            return Err(AgentError::Tool("MCP tool arguments must be an object".into()));
        }
        let request = json!({"name": self.tool, "arguments": arguments});
        match self.client.request("tools/call", request, self.call_timeout, &signal).await {
            Ok(result) => convert_result(&self.server, &self.tool, call_id, &result, self.results_dir.as_deref())
                .map_err(AgentError::Tool),
            Err(error) => interrupted(&self.server, &self.tool, self.read_only, self.call_timeout, error),
        }
    }
}

pub(crate) fn schema(name: String, description: String, parameters: Value) -> Tool {
    Tool { name, description, parameters: Schema(parameters), constrained_sampling: None }
}
