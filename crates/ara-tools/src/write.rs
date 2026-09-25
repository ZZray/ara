//! `write` tool (subset of OMP `tools/write.ts`).
//!
//! Ported: create/overwrite, parent directory creation, `Successfully wrote N
//! bytes to <path>` (N counts UTF-16 units like upstream `content.length`),
//! chmod +x for new shebang files with the upstream notice.
//!
//! Hashline mode (edit tool in hashline mode): a `[path#TAG]` wrapper on
//! `path` is unwrapped, `LINE:` display prefixes (and a loose `[path#TAG]`
//! header line) copied from read output are stripped from `content` with a
//! note, and the result starts with a fresh `[path#TAG]` header recorded with
//! no seen lines (`maybeWriteSnapshotHeader`). ARA difference: stripping also
//! requires consecutive row numbers (see `looks_like_read_rows`).
//!
//! Not ported (open): archive and SQLite targets, auto-generated-file guard,
//! conflict detection, LSP diagnostics, ACP bridge, plan-mode guard,
//! streaming preview.

use crate::ToolContext;
use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::json;
use tokio_util::sync::CancellationToken;

pub const EXECUTABLE_NOTICE: &str = "[Notice: Made executable via chmod +x]";
pub const STRIPPED_NOTE: &str = "Note: auto-stripped hashline display prefixes from content before writing.";

/// `LOOSE_HASHLINE_HEADER_RE`: a `[path#…]` line.
fn is_loose_hashline_header(line: &str) -> bool {
    let t = line.trim();
    t.len() > 2
        && t.starts_with('[')
        && t.ends_with(']')
        && t[1..t.len() - 1].split_once('#').is_some_and(|(path, tag)| {
            !path.is_empty() && !path.contains(['#', '\r', '\n']) && !tag.contains([' ', '\t', '\r', '\n'])
        })
}

/// Leading line number of a `LINE:`/`LINE|` display row.
fn row_number(line: &str) -> Option<u64> {
    static ROW: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^\s*(?:(?:>>>|>>)\s*)?(?:[+*-]\s*)?([0-9]+)[:|]").expect("valid regex")
    });
    ROW.captures(line).and_then(|c| c[1].parse().ok())
}

/// ARA data-safety gate on top of upstream's rule (every content row carries a
/// prefix): the prefixes must also be consecutive, as `read` output is, and a
/// lone row must be line 1. YAML like `200: OK` / `404: Not Found` or a single
/// `10:30 standup` line is written as authored.
fn looks_like_read_rows(lines: &[String]) -> bool {
    let numbers: Vec<u64> = lines
        .iter()
        .filter(|l| !l.trim().is_empty() && !is_loose_hashline_header(l))
        .filter(|l| !pi_edit::modes::hashline::prefixes::is_read_metadata_line(l))
        .filter_map(|l| row_number(l))
        .collect();
    match numbers.as_slice() {
        [] => false,
        [only] => *only == 1,
        many => many.windows(2).all(|w| w[1] == w[0] + 1),
    }
}

/// `stripWriteContentWithPotentialLooseHeader`, gated by [`looks_like_read_rows`].
fn strip_write_content(content: &str) -> (String, bool) {
    let lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    if !looks_like_read_rows(&lines) {
        return (content.to_string(), false);
    }
    let cleaned = pi_edit::modes::hashline::prefixes::strip_hashline_prefixes(&lines).join("\n");
    if cleaned != content {
        return (cleaned, true);
    }
    let Some(header) = lines.iter().position(|l| !l.trim().is_empty()) else { return (content.to_string(), false) };
    if !is_loose_hashline_header(&lines[header]) {
        return (content.to_string(), false);
    }
    let without: Vec<String> = lines[..header].iter().chain(&lines[header + 1..]).cloned().collect();
    let joined = without.join("\n");
    let cleaned = pi_edit::modes::hashline::prefixes::strip_hashline_prefixes(&without).join("\n");
    if cleaned == joined { (content.to_string(), false) } else { (cleaned, true) }
}

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
        let hashlines = self.ctx.hashlines();
        let raw_path = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        let path = pi_edit::path_policy::unwrap_hashline_header_path(raw_path);
        let raw_content = args.get("content").and_then(|v| v.as_str()).unwrap_or_default();
        let (content, stripped) =
            if hashlines { strip_write_content(raw_content) } else { (raw_content.to_string(), false) };
        let content = content.as_str();
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
        if hashlines {
            // A write shows no numbered lines, so the tag carries no seen lines.
            // The tag hashes what the edit engine will see: BOM-stripped text,
            // or a notebook's editable cells.
            let key = pi_edit::path_policy::canonical_key(&abs);
            let seen_text = if pi_edit::notebook::is_notebook_path(&abs) {
                pi_edit::notebook::notebook_to_editable_text(content, &display).unwrap_or_else(|_| content.to_string())
            } else {
                pi_edit::text::strip_bom(content).1.to_string()
            };
            let tag = self.ctx.edit_store.record(&key, &pi_edit::text::normalize_to_lf(&seen_text), Some(&[]));
            let header_path = crate::paths::format_path_relative_to_cwd(&abs, &self.ctx.cwd, false);
            text = format!("{}\n{text}", pi_edit::modes::hashline::format::format_hashline_header(&header_path, &tag));
        }
        if stripped {
            text.push('\n');
            text.push_str(STRIPPED_NOTE);
        }
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
