//! `write` tool (subset of OMP `tools/write.ts`).
//!
//! Ported: create/overwrite, parent directory creation, `Successfully wrote N
//! bytes to <path>` (N counts UTF-16 units like upstream `content.length`),
//! chmod +x for new shebang files with the upstream notice.
//!
//! Not ported (open): archive and SQLite targets, hashline prefix stripping,
//! auto-generated-file guard, conflict detection, LSP diagnostics, ACP bridge,
//! plan-mode guard, streaming preview.

use crate::ToolContext;
use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub const EXECUTABLE_NOTICE: &str = "[Notice: Made executable via chmod +x]";

pub struct WriteTool {
    pub ctx: ToolContext,
}

#[async_trait]
impl AgentTool for WriteTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "write".into(),
            description: "Creates or overwrites the file at `path` with `content`, creating parent directories. Replaces the whole file.".into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"},
                    "content": {"type": "string", "description": "Complete file content"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
        }
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        _cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or_default();
        let abs = self.ctx.resolve(path);
        let display = self.ctx.display(&abs);
        if abs.is_dir() {
            return Err(ToolError(format!("Cannot write {display}: path is a directory")));
        }
        let existed = abs.exists();
        if let Some(parent) = abs.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError(format!("Cannot create {}: {e}", parent.display())))?;
        }
        tokio::fs::write(&abs, content.as_bytes())
            .await
            .map_err(|e| ToolError(format!("Cannot write {display}: {e}")))?;
        let mut made_executable = false;
        #[cfg(unix)]
        if !existed && content.starts_with("#!") {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = tokio::fs::metadata(&abs).await {
                let mode = meta.permissions().mode();
                if mode & 0o111 == 0 {
                    let _ = tokio::fs::set_permissions(&abs, std::fs::Permissions::from_mode(mode | 0o111)).await;
                    made_executable = true;
                }
            }
        }
        let _ = existed;
        let mut text = format!("Successfully wrote {} bytes to {display}", content.encode_utf16().count());
        if made_executable {
            text.push('\n');
            text.push_str(EXECUTABLE_NOTICE);
        }
        let mut details = json!({"resolvedPath": abs.to_string_lossy()});
        if made_executable {
            details["madeExecutable"] = json!(true);
        }
        Ok(ToolOutput::text(text).with_details(details))
    }
}
