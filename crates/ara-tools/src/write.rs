//! `write` tool (subset of OMP `tools/write.ts`).
//!
//! Ported: create/overwrite, parent directory creation, `Successfully wrote N
//! bytes to <path>` (N counts UTF-16 units like upstream `content.length`),
//! chmod +x for shebang writes with a truthful changed-mode receipt.
//!
//! Hashline mode (edit tool in hashline mode): a `[path#TAG]` wrapper on
//! `path` is unwrapped, `LINE:` display prefixes (and a loose `[path#TAG]`
//! header line) copied from read output are stripped from `content` with a
//! note, and the result starts with a fresh `[path#TAG]` header recorded with
//! no seen lines (`maybeWriteSnapshotHeader`).
//!
//! Ordinary local writes also use the generated-file guard, read-selector and
//! URI-near-miss checks, pre-effect cancellation and Writing updates.
//! Archive WRITE uses fixed-source private readers and replacement encoders;
//! its executable evidence is recorded separately.
//! Not ported (open): complete
//! xd/SSH approval/dispatch and streaming preview/rendering. Ordinary file
//! writes consume the shared Host ACP/LSP/path/fallback ports; actual external
//! service installation and extension permission lifecycle remain Host obligations. Native Writing uses
//! a Write-specific acknowledgement before entering the filesystem effect.
//! The legacy no-update OS-input entry remains available to explicit callers.
//! Started byte writes retain their existing Native settlement owner; no
//! cancellation race or replay is added here.

use crate::ToolContext;
use crate::edit_host::{
    EditDeferredDiagnostics, EditDiagnosticsTransform, EditLspBatch, EditLspWriteRequest,
};
use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, atomic::{AtomicBool, Ordering}};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

pub const DEVICE_ONLY_DESCRIPTION:&str="Write JSON arguments to xd://<tool> to execute a mounted tool device. Read xd:// for available devices and xd://<tool> for full documentation. Filesystem writes are not available.";
pub const DEVICE_ONLY_REFUSAL:&str="This `write` tool is limited to the xd:// device transport: call it with path `xd://<tool>` and the device's JSON arguments in `content` (`read xd://` lists mounted devices). Active plan mode additionally permits local:// sandbox drafts. Filesystem writes are not available elsewhere.";

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

/// Fixed write.ts:332-363; every prefixed content row is eligible, independent of row numbers.
fn strip_write_content(content: &str) -> (String, bool) {
    let lines: Vec<String> = content.split('\n').map(str::to_string).collect();
    let cleaned = ara_edit::modes::hashline::prefixes::strip_hashline_prefixes(&lines).join("\n");
    if cleaned != content {
        return (cleaned, true);
    }
    let Some(header) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return (content.to_string(), false);
    };
    if !is_loose_hashline_header(&lines[header]) {
        return (content.to_string(), false);
    }
    let without: Vec<String> = lines[..header].iter().chain(&lines[header + 1..]).cloned().collect();
    let joined = without.join("\n");
    let cleaned = ara_edit::modes::hashline::prefixes::strip_hashline_prefixes(&without).join("\n");
    if cleaned == joined { (content.to_string(), false) } else { (cleaned, true) }
}

// Fixed OMP write.ts:114-210 and path-utils.ts:348-402 (MIT).
// Keep current registered hierarchical URI dispatch; this ordinary-path guard
// rejects URI near misses before they become local filesystem paths.
fn windows_absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'/' | b'\\')
}

fn assert_write_target_addressable(path: &str, ctx: &ToolContext) -> Result<(), ToolError> {
    let target = path.trim();
    let scheme = crate::internal_urls::hierarchical_scheme(target);
    let builtin_handler = |scheme: &str| {
        ["xd", "skill", "memory", "local", "vault"].iter().any(|known| scheme.eq_ignore_ascii_case(known))
    };
    if windows_absolute(target) {
        return Ok(());
    }
    if let Some(scheme) = scheme {
        // Actual registrations and explicit removed-route fences precede the
        // built-in fallback; removed routes retain their precise Host error.
        let handled = ctx.content_uri_port().is_some_and(|port| {
            matches!(port.route(&crate::internal_urls::split_content_url_selector(target).0), crate::ContentUriRoute::Registered { .. } | crate::ContentUriRoute::Removed)
        });
        if handled || builtin_handler(scheme) { return Ok(()); }
    }
    static XD_MISSING: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^xd/+(.*)$").unwrap());
    if let Some(captures) = XD_MISSING.captures(target) {
        return Err(ToolError(format!(
            "Unknown URI-like write target '{target}'. Did you mean 'xd://{}'? Prefix the path with './' to write it as a filesystem path.",
            &captures[1]
        )));
    }
    static URI_LIKE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(?i)^([a-z][a-z0-9+.-]*):/{1,2}(.*)$").unwrap());
    let Some(captures) = URI_LIKE.captures(target) else {
        return Ok(());
    };
    let scheme = captures[1].to_ascii_lowercase();
    if scheme == "conflict" {
        return Ok(());
    }
    let host_handler = crate::internal_urls::hierarchical_scheme(target).is_none()
        && ctx.content_uri_port().is_some_and(|port| {
            matches!(port.route(&crate::internal_urls::split_content_url_selector(&format!("{scheme}://{}", &captures[2])).0), crate::ContentUriRoute::Registered { .. })
        });
    let canonical = if builtin_handler(&scheme) || host_handler {
        Some(scheme.as_str())
    } else if matches!(scheme.as_str(), "dx" | "xdd" | "xdt") {
        Some("xd")
    } else { None };
    let suggestion = canonical.map_or_else(
        || " Tool devices use 'xd://<tool>'.".to_owned(),
        |scheme| format!(" Did you mean '{scheme}://{}'?", &captures[2]),
    );
    Err(ToolError(format!(
        "Unknown URI-like write target '{target}'.{suggestion} Prefix the path with './' to write it as a filesystem path."
    )))
}

fn assert_not_read_selector_misfire(ctx: &ToolContext, target: &str, content: &str) -> Result<(), ToolError> {
    let segments = target.split(';').collect::<Vec<_>>();
    let list = segments.len() > 1
        && segments.iter().all(|segment| {
            let segment = segment.trim();
            !segment.is_empty() && crate::paths::split_path_and_sel(segment).1.is_some()
        });
    let single_selector = content.is_empty().then(|| crate::paths::split_path_and_sel(target).1).flatten();
    if (list || single_selector.is_some()) && crate::paths::probe_literal(ctx, target) == crate::paths::Probe::Missing {
        if list {
            return Err(ToolError(format!(
                "write target '{target}' is a semicolon-joined list of {} read-tool selectors, not a filesystem path — refusing to create it. write creates a single file; issue one read() per path to read these ranges (e.g. read({{ path: \"<one path>:<range>\" }})).",
                segments.len()
            )));
        }
        let selector = single_selector.expect("single selector checked");
        return Err(ToolError(format!(
            "write target '{target}' ends with a read-tool selector ':{selector}' and no such file exists — refusing to create a literal file by that name. If you meant to read it, use read({{ path: \"{target}\" }}). If you truly intend to create this file, pass its contents in `content` (a non-empty write is never blocked)."
        )));
    }
    Ok(())
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(ToolError("Aborted: Cancelled".into())) } else { Ok(()) }
}

/// Fixed render-utils.ts:714-729: replace a complete home prefix only.
fn shorten_write_path(ctx: &ToolContext, path: &str) -> String {
    let home = ctx
        .edit_host()
        .and_then(|host| host.path_policy().home_dir)
        .or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from));
    let Some(home) = home else {
        return path.to_owned();
    };
    let home = home.to_string_lossy();
    let windows_style = windows_absolute(&home) || home.starts_with(r"\\");
    let matches = if windows_style {
        path.to_lowercase().starts_with(&home.to_lowercase())
    } else {
        path.starts_with(home.as_ref())
    };
    if !home.is_empty()
        && matches
        && let Some(suffix) = path.get(home.len()..)
    {
        if suffix.is_empty() || suffix.starts_with(['/', '\\']) {
            return format!("~{}", suffix.replace('\\', "/"));
        }
    }
    path.to_owned()
}

fn write_progress(ctx: &ToolContext, content: &str, display: &str, resolved: Option<&Path>) -> ToolOutput {
    let details = resolved.map_or_else(|| json!({}), |path| json!({"resolvedPath": path.to_string_lossy()}));
    ToolOutput::text(format!(
        "Writing {} bytes to {}...",
        content.encode_utf16().count(),
        shorten_write_path(ctx, display)
    ))
    .with_details(details)
}

/// The Host acknowledges the original synchronous update Call before any
/// mkdir/write. A returned Graph Promise is ignored, as in fixed write.ts.
/// Graph values and the original failure carrier stay owned by the Host.
#[async_trait]
pub trait WriteProgress: Send + Sync {
    async fn acknowledge(&self, output: ToolOutput) -> Result<(), ToolError>;
    /// Native Host requests this at fixed write.ts:1288, before generated guard/progress.
    /// Only selected native batch scalars cross the admitted worker boundary.
    async fn prepare_batch(&self) -> Result<Option<EditLspBatch>, ToolError> {
        Ok(None)
    }
}

struct DirectWriteProgress(UpdateFn);
#[async_trait]
impl WriteProgress for DirectWriteProgress {
    async fn acknowledge(&self, output: ToolOutput) -> Result<(), ToolError> {
        (self.0)(output);
        Ok(())
    }
}

/// Only the explicit pre-effect boundary below may claim NotDispatched.
/// Other failures preserve the existing tool/Host settlement contract.
#[derive(Debug)]
pub enum WriteExecutionError {
    NotDispatched(ToolError),
    Tool(ToolError),
}
impl From<ToolError> for WriteExecutionError {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}
impl WriteExecutionError {
    pub fn into_tool_error(self) -> ToolError {
        match self {
            Self::NotDispatched(error) | Self::Tool(error) => error,
        }
    }
}

pub struct WriteTool {
    pub ctx: ToolContext,
    deferred_diagnostics: Option<Arc<EditDeferredDiagnostics>>,
    transform_diagnostics: Option<EditDiagnosticsTransform>,
}

#[async_trait]
impl AgentTool for WriteTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "write".into(),
            description: if {let policy=self.ctx.write_transport_policy();policy.device_only&&!policy.pending_full_description} {
                DEVICE_ONLY_DESCRIPTION.into()
            } else { "Creates or overwrites the file at `path` with `content`, creating parent directories. For ordinary files, replaces the whole file. Supports `.zip` (including ZIP-based `.jar`/`.war`/`.ear`/`.apk`), `.tar`, `.tar.gz`/`.tgz`, `.tar.zst`, and `.asar` archive entries via `archive.ext:path/inside/archive`; other archive formats cannot be written. Supports SQLite row operations via `db.sqlite:table` (insert), `db.sqlite:table:key` (update with JSON or JSON5 content, delete with empty content).".into()},
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path"},
                    "content": {"type": "string", "description": "Complete file content"}
                },
                "required": ["path", "content"],
                "additionalProperties": false
            }),
            ..Default::default()
        }
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let path = args.get("path").and_then(|value| value.as_str()).unwrap_or_default();
        let content = args.get("content").and_then(|value| value.as_str()).unwrap_or_default();
        self.execute_os_values_with_update(path, content, cancel, update).await
    }
}

impl WriteTool {
    /// Share the admitted Session owner; resolve LSP leaves at each write and
    /// diagnostics delivery, including handles retained by in-flight fetches.
    pub fn new(ctx: ToolContext) -> Self {
        let host = ctx.edit_host();
        let transform_diagnostics = host.as_ref().and_then(|owner| {
            let ledger = owner.diagnostics_host()?;
            Some(Arc::new(move |path: &Path, result| ledger.reduce_diagnostics(path, result)) as EditDiagnosticsTransform)
        });
        let deferred_diagnostics = host.as_ref().and_then(|owner| {
            let ledger = owner.deferred_diagnostics_host()?;
            let owner = owner.clone();
            Some(EditDeferredDiagnostics::with_live_dedup(ledger, Arc::new(move || owner.write_lsp_options().deduplicate_diagnostics)))
        });
        Self { ctx, deferred_diagnostics, transform_diagnostics }
    }

    /// Checks the current transport grant before any resolver/handler or effect.
    /// Shared by typed and Native consumers; metadata preview cannot relax it.
    pub fn require_write_transport_target(&self,raw_path:&str)->Result<(),ToolError> {
        if !self.ctx.write_transport_policy().device_only{return Ok(())}
        let unwrapped=ara_edit::path_policy::unwrap_hashline_header_path(raw_path);
        let (target,_)=crate::internal_urls::split_content_url_selector(unwrapped);
        let xd=target.get(..5).is_some_and(|prefix|prefix.eq_ignore_ascii_case("xd://"));
        let local=self.ctx.edit_host().is_some_and(|host|{
            let state=host.path_policy();
            let policy=state.file_policy(&self.ctx.cwd);
            state.plan_active&&policy.resolve(&target, &crate::edit_urls::native_file_answers(&state,&target)).is_ok_and(|resolved|policy.enforce_write(&resolved.absolute.to_string_lossy(),ara_edit::FileOp::Create,None,&std::collections::HashMap::new()).is_ok())
        });
        let coordination = self.ctx.content_uri_port().is_some_and(|port| {
            matches!(port.route(&target), crate::ContentUriRoute::Registered { writable: true }) && port.write_policy(&target).coordination()
        });
        if xd||local||coordination{Ok(())}else{Err(ToolError(DEVICE_ONLY_REFUSAL.into())) }
    }

    pub async fn execute_os_values_with_request(
        &self,
        path: &str,
        content: &str,
        batch: Option<EditLspBatch>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_values_with_request_and_progress(
            path,
            content,
            batch,
            cancel,
            Arc::new(DirectWriteProgress(Arc::new(|_| {}))),
            None,
        )
        .await
        .map_err(WriteExecutionError::into_tool_error)
    }

    /// Shared existing filesystem/URI action body. Only these OS inputs are scalar;
    /// actual callers retain their original arguments and current capability receipt.
    pub async fn execute_os_values(
        &self,
        path: &str,
        content: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_values_with_update(path, content, cancel, Arc::new(|_| {})).await
    }

    /// Typed calls keep their direct UpdateFn through the same action body.
    pub async fn execute_os_values_with_update(
        &self,
        path: &str,
        content: &str,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_values_with_progress(path, content, cancel, Arc::new(DirectWriteProgress(update)))
            .await
            .map_err(WriteExecutionError::into_tool_error)
    }

    pub async fn execute_os_values_with_progress(
        &self,
        path: &str,
        content: &str,
        cancel: CancellationToken,
        progress: Arc<dyn WriteProgress>,
    ) -> Result<ToolOutput, WriteExecutionError> {
        self.execute_os_values_with_request_and_progress(path, content, None, cancel, progress,None).await
    }

    pub async fn execute_os_values_selected(&self,path:&str,content:&str,cancel:CancellationToken,receipt:crate::ContentUriWriteReceipt)->Result<ToolOutput,WriteExecutionError>{
        self.execute_os_values_with_receipt(path,content,cancel,Arc::new(DirectWriteProgress(Arc::new(|_|{}))),receipt).await
    }
    pub async fn execute_os_values_with_receipt(&self,path:&str,content:&str,cancel:CancellationToken,progress:Arc<dyn WriteProgress>,receipt:crate::ContentUriWriteReceipt)->Result<ToolOutput,WriteExecutionError> {
        self.execute_os_values_with_request_and_progress(path,content,None,cancel,progress,Some(receipt)).await
    }

    async fn execute_os_values_with_request_and_progress(
        &self,
        path: &str,
        content: &str,
        batch: Option<EditLspBatch>,
        cancel: CancellationToken,
        progress: Arc<dyn WriteProgress>,
        receipt:Option<crate::ContentUriWriteReceipt>,
    ) -> Result<ToolOutput, WriteExecutionError> {
        if let Some(receipt)=&receipt{receipt.check(&self.ctx,path).map_err(WriteExecutionError::NotDispatched)?;}
        check_cancelled(&cancel).map_err(WriteExecutionError::NotDispatched)?;
        self.require_write_transport_target(path).map_err(WriteExecutionError::NotDispatched)?;
        if let Some(device) = crate::resolve::resolution_device(path) {
            if cancel.is_cancelled() {
                return Err(WriteExecutionError::NotDispatched(ToolError("Aborted: Cancelled".into())));
            }
            return self.ctx.pending_resolution.dispatch_in_context(device, content, &self.ctx).await.map_err(WriteExecutionError::Tool);
        }
        let hashlines = self.ctx.hashlines();
        let raw_path = path;
        let path = ara_edit::path_policy::unwrap_hashline_header_path(raw_path);
        assert_write_target_addressable(path, &self.ctx)?;
        let filesystem_url = path.starts_with("local:") || path.starts_with("vault:");
        let mut filesystem_target = None;
        if !windows_absolute(path)
            && let Some(scheme) = crate::internal_urls::hierarchical_scheme(path)
        {
            let (url, selector) = crate::internal_urls::split_content_url_selector(path);
            if let Some(sel) = selector
                && !sel.eq_ignore_ascii_case("raw")
                && !sel.eq_ignore_ascii_case("conflicts")
            {
                return Err(ToolError(format!(
                    "write does not accept the trailing selector \":{sel}\" — it writes a whole file. Remove \":{sel}\", or if the filename truly ends with it, percent-encode the \":\" as %3A."
                )).into());
            }
            if let Some(port) = self.ctx.content_uri_port() {
                match port.route(&url) {
                    crate::ContentUriRoute::Registered { writable: true } => {
                        if cancel.is_cancelled() {
                            return Err(WriteExecutionError::NotDispatched(ToolError(format!(
                                "Host URI write for {url} was aborted"
                            ))));
                        }
                        let selection=port.write_selection(&url);
                        let policy=selection.policy;
                        let raw_content = content;
                        let (content, stripped) =
                            if hashlines && policy.payload != crate::ContentUriWritePayload::Verbatim { strip_write_content(raw_content) } else { (raw_content.to_owned(), false) };
                        progress
                            .acknowledge(write_progress(&self.ctx, &content, &url, None))
                            .await
                            .map_err(WriteExecutionError::NotDispatched)?;
                        check_cancelled(&cancel).map_err(WriteExecutionError::NotDispatched)?;
                        // An awaited progress/approval may replace the live owner.
                        let current = self.ctx.content_uri_port().ok_or_else(|| WriteExecutionError::NotDispatched(ToolError("URI owner removed before dispatch".into())))?;
                        if !std::sync::Arc::ptr_eq(&port, &current)
                            || current.route(&url) != (crate::ContentUriRoute::Registered { writable: true })
                            || current.write_selection(&url) != selection {
                            return Err(WriteExecutionError::NotDispatched(ToolError("URI owner or write policy changed before dispatch; nothing sent".into())));
                        }
                        self.require_write_transport_target(path).map_err(WriteExecutionError::NotDispatched)?;
                        if let Some(receipt)=&receipt{receipt.check(&self.ctx,path).map_err(WriteExecutionError::NotDispatched)?;}
                        if let Some(result) = current.write_result_selected(&url, &content, &selection,cancel).await.map_err(|error|match error{crate::ContentUriWriteDispatchError::NotDispatched(error)=>WriteExecutionError::NotDispatched(error),crate::ContentUriWriteDispatchError::Effect(error)=>WriteExecutionError::Tool(error)})? {
                            return Ok(result);
                        }
                        let mut text = format!("Successfully wrote {} bytes to {url}", content.encode_utf16().count());
                        if stripped {
                            text.push('\n');
                            text.push_str(STRIPPED_NOTE);
                        }
                        return Ok(ToolOutput::text(text).with_details(json!({})));
                    }
                    crate::ContentUriRoute::Registered { writable: false } => {
                        return Err(ToolError(format!(
                            "{}:// URLs are read-only for write; use the protocol-specific tool for mutations.",
                            scheme.to_ascii_lowercase()
                        ))
                        .into());
                    }
                    crate::ContentUriRoute::Removed => {
                        return Err(ToolError(format!("Unknown protocol: {}://", scheme.to_ascii_lowercase())).into());
                    }
                    crate::ContentUriRoute::Unregistered => {}
                }
            }
            // Registered and Removed Host routes retain precedence over the
            // fixed builtin read-only memory protocol.
            if scheme.eq_ignore_ascii_case("memory") {
                return Err(ToolError(
                    "memory:// URLs are read-only for write; use the protocol-specific tool for mutations.".into(),
                )
                .into());
            }
            if !crate::internal_urls::is_internal_url(path)
                && !filesystem_url
                && !scheme.eq_ignore_ascii_case("conflict")
            {
                return Err(ToolError(format!("Unknown protocol: {}://", scheme.to_ascii_lowercase())).into());
            }
            if filesystem_url {
                filesystem_target = Some(url);
            }
        }
        if let Some(uri) = crate::conflict_detect::parse_conflict_uri(path)? {
            if let Some(scope) = uri.scope {
                return Err(ToolError(format!("Conflict URI scope '/{}' is read-only — read `conflict://{}/{}` to inspect that side. To write, drop the scope (`conflict://{}`) and put the chosen content (or shorthand like `@{}`) in `content`.", scope.as_str(), uri.id, scope.as_str(), uri.id, scope.as_str())).into());
            }
            let (clean, stripped) = if hashlines { strip_write_content(content) } else { (content.to_owned(), false) };
            progress
                .acknowledge(write_progress(&self.ctx, &clean, path, None))
                .await
                .map_err(WriteExecutionError::NotDispatched)?;
            let mut result = match uri.id {
                crate::conflict_detect::ConflictId::Number(id) => {
                    self.resolve_conflict_by_id(id, &clean, stripped).await?
                }
                crate::conflict_detect::ConflictId::Wildcard => {
                    self.resolve_all_conflicts(&clean, stripped, content).await?
                }
            };
            if let Some(prefix) = uri.recovered_prefix {
                if let Some(ara_ai::UserBlock::Text(block)) =
                    result.content.iter_mut().find(|block| matches!(block, ara_ai::UserBlock::Text(_)))
                {
                    block.text.push_str(&format!("\nNote: stripped erroneous '{prefix}:' prefix from path; conflict URIs are global (use `conflict://{}`, not `<file>:conflict://{}`).", uri.id, uri.id));
                }
            }
            return Ok(result);
        }
        // Registered Host URI handlers above retain precedence. Only the native
        // local/vault filesystem routes may proceed to the common path policy.
        let path = filesystem_target.as_deref().unwrap_or(path);
        if crate::internal_urls::is_internal_url(path) && !filesystem_url {
            let (_, selector) = crate::internal_urls::split_skill_url_selector(path);
            if let Some(sel) = selector
                && !sel.eq_ignore_ascii_case("raw")
                && !sel.eq_ignore_ascii_case("conflicts")
            {
                return Err(ToolError(format!(
                    "write does not accept the trailing selector \":{sel}\" — it writes a whole file. Remove \":{sel}\", or if the filename truly ends with it, percent-encode the \":\" as %3A."
                )).into());
            }
            return Err(ToolError(
                "skill:// URLs are read-only for write; use the protocol-specific tool for mutations.".into(),
            )
            .into());
        }
        let raw_content = content;
        let (content, stripped) =
            if hashlines { strip_write_content(raw_content) } else { (raw_content.to_string(), false) };
        let content = content.as_str();
        if let Some(archive) = crate::write_archive::resolve(path, |outer| {
            let host = self.ctx.edit_host();
            let policy = host.as_ref().map(|host| host.path_policy()).unwrap_or_default().file_policy(&self.ctx.cwd);
            if host.is_some() || filesystem_url {
                policy.resolve(outer, &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),outer)).map(|resolved| resolved.absolute).map_err(|error| ToolError(error.to_string()))
            } else {
                Ok(self.ctx.resolve(outer))
            }
        })? {
            let host = self.ctx.edit_host();
            let policy = host.as_ref().map(|host| host.path_policy()).unwrap_or_default().file_policy(&self.ctx.cwd);
            policy
                .enforce_write(
                    &archive.authored,
                    if archive.exists { ara_edit::FileOp::Update } else { ara_edit::FileOp::Create },
                    None,
                    &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),&archive.authored),
                )
                .map_err(|error| ToolError(error.to_string()))?;
            let display = format!("{}:{}", self.ctx.display(&archive.absolute), archive.display_member());
            progress
                .acknowledge(write_progress(&self.ctx, content, &display, Some(&archive.absolute)))
                .await
                .map_err(WriteExecutionError::NotDispatched)?;
            check_cancelled(&cancel).map_err(WriteExecutionError::NotDispatched)?;
            crate::write_archive::rewrite(&archive, content).await?;
            if let Some(host) = &host {
                host.invalidate_after_write(&archive.absolute);
            } else {
                crate::engine::invalidate_fs_scan_cache(Some(&archive.absolute));
            }
            let mut text = format!("Successfully wrote {} bytes to {display}", content.encode_utf16().count());
            if stripped {
                text.push('\n');
                text.push_str(STRIPPED_NOTE);
            }
            return Ok(ToolOutput::text(text).with_details(json!({"resolvedPath":archive.absolute.to_string_lossy()})));
        }
        let host = self.ctx.edit_host();
        let policy = host.as_ref().map(|host| host.path_policy()).unwrap_or_default().file_policy(&self.ctx.cwd);
        // Fixed write.ts:1267-1283. The final archive graft precedes this
        // branch; neither compound target reaches the ordinary selector guard.
        if let Some(target) = crate::write_sqlite::resolve(path, |database| {
            if host.is_some() || filesystem_url {
                policy.resolve(database, &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),database)).map(|resolved| resolved.absolute).map_err(|error| ToolError(error.to_string()))
            } else {
                Ok(self.ctx.resolve(database))
            }
        })? {
            policy
                .enforce_write(&target.database_path, ara_edit::FileOp::Update, None, &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),&target.database_path))
                .map_err(|error| ToolError(error.to_string()))?;
            progress
                .acknowledge(write_progress(&self.ctx, content, path, Some(&target.absolute)))
                .await
                .map_err(WriteExecutionError::NotDispatched)?;
            check_cancelled(&cancel).map_err(WriteExecutionError::NotDispatched)?;
            let mut text = crate::write_sqlite::execute(path, content, &target)?;
            if let Some(host) = &host {
                host.invalidate_after_write(&target.absolute);
            } else {
                crate::engine::invalidate_fs_scan_cache(Some(&target.absolute));
            }
            if stripped {
                text.push('\n');
                text.push_str(STRIPPED_NOTE);
            }
            return Ok(ToolOutput::text(text).with_details(json!({
                "resolvedPath":target.absolute.to_string_lossy(),
                "meta":{"source":{"type":"path","value":target.absolute.to_string_lossy()}}
            })));
        }
        assert_not_read_selector_misfire(&self.ctx, path, content)?;
        policy.enforce_write(path, ara_edit::FileOp::Create, None, &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),path)).map_err(|error| ToolError(error.to_string()))?;
        let abs = if host.is_some() || filesystem_url {
            policy.resolve(path, &crate::edit_urls::native_file_answers(&host.as_ref().map(|host|host.path_policy()).unwrap_or_default(),path)).map_err(|error| ToolError(error.to_string()))?.absolute
        } else {
            self.ctx.resolve(path)
        };
        // Fixed position: special/URI dispatch and selector/plan/resolve precede
        // the original native Getter; generated guard and Writing follow it.
        let batch = match batch {
            Some(batch) => Some(batch),
            None => progress.prepare_batch().await.map_err(WriteExecutionError::NotDispatched)?,
        };
        let display = self.ctx.display(&abs);
        if abs.is_dir() {
            return Err(ToolError(format!("Cannot write {display}: path is a directory")).into());
        }
        if abs.exists()
            && let Some(message) = self.auto_generated_message(&abs, path).await
        {
            return Err(ToolError(message).into());
        }
        progress
            .acknowledge(write_progress(&self.ctx, content, &display, Some(&abs)))
            .await
            .map_err(WriteExecutionError::NotDispatched)?;
        // A cancelled typed call (or a callback that cancels it) cannot enter
        // mkdir/write. Once admitted, the original fs future stays owned until
        // settlement; dropping it cannot prove an absent filesystem effect.
        check_cancelled(&cancel).map_err(WriteExecutionError::NotDispatched)?;
        let fallbacks = host.as_ref().and_then(|host| host.file_mutation_fallbacks());
        let session_id = host.as_ref().and_then(|host| host.file_mutation_session_id());
        let bridge = if let Some(host) = &host {
            crate::edit_host::route_write_through_bridge(
                host.as_ref(),
                &self.ctx.cwd,
                path,
                &abs,
                content,
                Some(cancel.clone()),
                self.deferred_diagnostics.as_ref(),
            )
            .await
            .map_err(|failure| {
                let error = ToolError(format!(
                    "Cannot write {display}: {}{}",
                    failure.error,
                    if failure.admitted { "; ACP persisted outcome unknown" } else { "" }
                ));
                if failure.admitted {
                    WriteExecutionError::Tool(error)
                } else {
                    WriteExecutionError::NotDispatched(error)
                }
            })?
        } else {
            None
        };
        // ACP reports requested clean UTF8 bytes even when client readback drifted.
        // Ordinary writes report the supplier's persisted final UTF8 bytes.
        let bridge_written = bridge.is_some();
        let enabled = host.as_ref().is_some_and(|host| host.enable_lsp());
        let lsp = host.as_ref().filter(|_| enabled).and_then(|host| host.lsp_writethrough());
        let mut lsp_options = host.as_ref().map(|host| host.write_lsp_options()).unwrap_or_default();
        lsp_options.enable_format &= enabled;
        lsp_options.enable_diagnostics &= enabled;
        let began = Arc::new(AtomicBool::new(false));
        let (verified_content, diagnostics) = if let Some(written) = bridge {
            (Some(written), None)
        } else {
            let result = if let Some(lsp) = &lsp {
                lsp.write_through_result(EditLspWriteRequest {
                    cwd: &self.ctx.cwd,
                    absolute_path: &abs,
                    content,
                    options: lsp_options,
                    batch: batch.clone(),
                    cancel: Some(cancel.clone()),
                    get_deferred: self.deferred_diagnostics.as_ref().filter(|_| lsp_options.enable_diagnostics).map(|owner| owner.for_all_paths_tracking(began.clone())),
                    transform_diagnostics: if lsp_options.enable_diagnostics && lsp_options.deduplicate_diagnostics { self.transform_diagnostics.clone() } else { None },
                    file_fallbacks: fallbacks.clone(),
                    mutation_session_id: session_id.clone(),
                })
                .await
                .map_err(|error| ToolError(format!("Cannot write {display}: {error}; LSP persisted outcome unknown")))?
            } else {
                if let Some(registry) = &fallbacks {
                    if let Some(parent) = abs.parent() {
                        registry
                            .mkdir_allowing_fallback(parent)
                            .map_err(|error| ToolError(format!("Cannot create {}: {error}", parent.display())))?;
                    }
                    registry.write(&abs, content, session_id.as_deref()).await.map_err(|error| {
                        let mut message = format!("Cannot write {display}: {}", error.original());
                        if let Some(cause) = error.boundary_cause() {
                            message.push_str(&format!("; recovered permission boundary: {cause}"));
                        }
                        ToolError(message)
                    })?;
                } else {
                    if let Some(parent) = abs.parent() {
                        tokio::fs::create_dir_all(parent)
                            .await
                            .map_err(|error| ToolError(format!("Cannot create {}: {error}", parent.display())))?;
                    }
                    tokio::fs::write(&abs, content.as_bytes())
                        .await
                        .map_err(|error| ToolError(format!("Cannot write {display}: {error}")))?;
                }
                crate::edit_host::EditLspWritethroughResult::default()
            };
            if let Some(host) = &host {
                host.invalidate_after_write(&abs);
                if !began.load(Ordering::Acquire) {
                    host.bump_file_mutation_version(&abs);
                }
            }
            (result.final_content, result.diagnostics)
        };
        // Target LSP/ACP final bytes drive snapshot tags and chmod alike.
        let persisted_content = verified_content.as_deref().unwrap_or(content);
        #[cfg(unix)]
        let mut made_executable = false;
        #[cfg(not(unix))]
        let made_executable = false;
        #[cfg(unix)]
        if persisted_content.starts_with("#!") {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = tokio::fs::metadata(&abs).await {
                let mode = meta.permissions().mode() & 0o7777;
                let new_mode = mode | 0o111;
                if new_mode != mode
                    && tokio::fs::set_permissions(&abs, std::fs::Permissions::from_mode(new_mode)).await.is_ok()
                {
                    made_executable = true;
                }
            }
        }
        let receipt_bytes = if bridge_written { content.len() } else { persisted_content.len() };
        let mut text = format!("Successfully wrote {receipt_bytes} bytes to {display}");
        if hashlines {
            // A write shows no numbered lines, so the tag carries no seen lines.
            // The tag hashes what the edit engine will see: BOM-stripped text,
            // or a notebook's editable cells.
            let key = ara_edit::path_policy::canonical_key(&abs);
            let seen_text = if ara_edit::notebook::is_notebook_path(&abs) {
                ara_edit::notebook::notebook_to_editable_text(persisted_content, &display)
                    .unwrap_or_else(|_| content.to_string())
            } else {
                ara_edit::text::strip_bom(persisted_content).1.to_string()
            };
            let tag = self.ctx.edit_store.record(&key, &ara_edit::text::normalize_to_lf(&seen_text), Some(&[]));
            let header_path = crate::paths::format_path_relative_to_cwd(&abs, &self.ctx.cwd, false);
            text = format!("{}\n{text}", ara_edit::modes::hashline::format::format_hashline_header(&header_path, &tag));
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
        if let Some(diagnostics) = diagnostics {
            if diagnostics
                .get("messages")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|messages| !messages.is_empty())
            {
                details["meta"] = json!({"diagnostics": {
                    "summary": diagnostics.get("summary").cloned().unwrap_or(json!("")),
                    "messages": diagnostics["messages"],
                }});
            }
            details["diagnostics"] = diagnostics;
        }
        Ok(ToolOutput::text(text).with_details(details))
    }

    /// Fixed standalone guard + pi-natives edit.rs:663-694: read at most
    /// 1024 bytes; any open/read failure returns no rejection. The detector's
    /// filename rule also runs only after a successful readable prefix.
    async fn auto_generated_message(&self, absolute: &Path, display: &str) -> Option<String> {
        if !self.ctx.edit_settings().block_auto_generated {
            return None;
        }
        let mut file = tokio::fs::File::open(absolute).await.ok()?;
        let mut head = [0u8; 1024];
        let mut read = 0;
        while read < head.len() {
            let count = file.read(&mut head[read..]).await.ok()?;
            if count == 0 {
                break;
            }
            read += count;
        }
        let policy = ara_edit::PathPolicy {
            cwd: PathBuf::new(),
            home_dir: PathBuf::new(),
            url_schemes: Vec::new(),
            url_alias_schemes: Vec::new(),
            plan_writable_roots: Vec::new(),
            plan_active: false,
            block_auto_generated: true,
        };
        policy.auto_generated_message(display, &head[..read])
    }
}

impl WriteTool {
    /// Fixed writethrough.ts:62-71 ignores its AbortSignal and delegates directly
    /// to the established permission fallback. It never formats via LSP/ACP.
    async fn persist_conflict_content(&self, path: &Path, content: &str) -> Result<(), ToolError> {
        let host = self.ctx.edit_host();
        if let Some(fallback) = host.as_ref().and_then(|host| host.file_mutation_fallbacks()) {
            let session = host.as_ref().and_then(|host| host.file_mutation_session_id());
            fallback
                .write(path, content, session.as_deref())
                .await
                .map_err(|error| ToolError(error.original().to_string()))?;
        } else {
            tokio::fs::write(path, content.as_bytes()).await.map_err(|error| ToolError(error.to_string()))?;
        }
        if let Some(host) = host {
            host.invalidate_after_write(path);
            host.bump_file_mutation_version(path);
        }
        self.ctx.edit_store.invalidate(&ara_edit::path_policy::canonical_key(path));
        Ok(())
    }
    fn conflict_snapshot_header(&self, path: &Path, content: &str) -> Option<String> {
        if !self.ctx.hashlines() {
            return None;
        }
        let text = if ara_edit::notebook::is_notebook_path(path) {
            ara_edit::notebook::notebook_to_editable_text(content, &self.ctx.display(path))
                .unwrap_or_else(|_| content.to_owned())
        } else {
            ara_edit::text::strip_bom(content).1.to_owned()
        };
        let tag = self.ctx.edit_store.record(
            &ara_edit::path_policy::canonical_key(path),
            &ara_edit::text::normalize_to_lf(&text),
            Some(&[]),
        );
        Some(ara_edit::modes::hashline::format::format_hashline_header(&self.ctx.hashline_display(path), &tag))
    }
    async fn conflict_text(path: &Path) -> Result<String, ToolError> {
        tokio::fs::read(path)
            .await
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .map_err(|e| ToolError(e.to_string()))
    }
    async fn resolve_conflict_by_id(&self, id: f64, content: &str, stripped: bool) -> Result<ToolOutput, ToolError> {
        let entry = self.ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner()).get(id)
            .ok_or_else(|| ToolError(format!("Conflict #{id} not found. Conflict ids are registered when `read` surfaces a marker block; re-read the file to get a current id.")))?;
        if !entry.absolute_path.exists() {
            return Err(ToolError(format!("Conflict #{} target '{}' no longer exists.", entry.id, entry.display_path)));
        }
        let expanded = crate::conflict_detect::expand_content_tokens(content, &entry)?;
        let original = Self::conflict_text(&entry.absolute_path).await?;
        let splice = crate::conflict_detect::splice_conflict(&original, &entry, &expanded)?;
        self.persist_conflict_content(&entry.absolute_path, &splice.text).await?;
        {
            let mut history = self.ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner());
            history.invalidate(entry.id);
            for other in history.entries() {
                if other.absolute_path == entry.absolute_path
                    && crate::conflict_detect::conflict_regions_equal(&other.block, &entry.block)
                    && !crate::conflict_detect::conflict_region_present(&splice.text, &other.block)
                {
                    history.invalidate(other.id);
                }
            }
        }
        let range = if entry.block.start_line == entry.block.end_line {
            format!("line {}", entry.block.start_line)
        } else {
            format!("lines {}–{}", entry.block.start_line, entry.block.end_line)
        };
        let summary = format!("Resolved conflict #{} at {range} in {}.", entry.id, entry.display_path);
        let mut text = self
            .conflict_snapshot_header(&entry.absolute_path, &splice.text)
            .map_or_else(|| summary.clone(), |header| format!("{header}\n{summary}"));
        if stripped {
            text.push_str(&format!("\n{STRIPPED_NOTE}"));
        }
        let trimmed = splice.trimmed_leading + splice.trimmed_trailing;
        if trimmed > 0 {
            text.push_str(&format!("\nNote: dropped {trimmed} content line(s) that duplicated the code adjacent to the conflict region — writes replace only the marker block; surrounding lines stay in place."));
        }
        Ok(ToolOutput::text(text).with_details(json!({"resolvedPath":entry.absolute_path.to_string_lossy()})))
    }
    async fn resolve_all_conflicts(&self, content: &str, stripped: bool, raw: &str) -> Result<ToolOutput, ToolError> {
        let entries = self.ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner()).entries();
        if entries.is_empty() {
            return Err(ToolError("`conflict://*` has nothing to resolve — no conflicts are currently registered. Re-read the file(s) with conflicts first.".into()));
        }
        let directives = crate::conflict_detect::resolve_bulk_directives(raw, content)?;
        if let Some(directives) = &directives {
            let unknown = directives
                .iter()
                .filter(|(id, _)| !entries.iter().any(|entry| entry.id as f64 == *id))
                .map(|(id, _)| format!("#{id}"))
                .collect::<Vec<_>>();
            if !unknown.is_empty() {
                return Err(ToolError(format!(
                    "Bulk directive references unknown conflict id(s) {}. Currently registered: {}.",
                    unknown.join(", "),
                    entries.iter().map(|e| format!("#{}", e.id)).collect::<Vec<_>>().join(", ")
                )));
            }
        }
        let selected = entries
            .iter()
            .filter(|entry| directives.as_ref().is_none_or(|map| map.iter().any(|(id, _)| *id == entry.id as f64)))
            .cloned()
            .collect::<Vec<_>>();
        // Insertion order matches the upstream Map, not pathname order.
        let mut files: Vec<(PathBuf, Vec<crate::conflict_detect::ConflictEntry>)> = Vec::new();
        for entry in &selected {
            if let Some((_, members)) = files.iter_mut().find(|(path, _)| path == &entry.absolute_path) {
                members.push(entry.clone());
            } else {
                files.push((entry.absolute_path.clone(), vec![entry.clone()]));
            }
        }
        let mut succeeded: Vec<(String, usize, Option<String>)> = Vec::new();
        let mut failed: Vec<(String, usize, String)> = Vec::new();
        let mut resolved_count = 0;
        let mut echo_trimmed = 0;
        for (path, mut members) in files {
            let display = members[0].display_path.clone();
            if !path.exists() {
                failed.push((display, members.len(), "file no longer exists".into()));
                continue;
            }
            members.sort_by(|a, b| b.block.start_line.cmp(&a.block.start_line));
            let mut text = match Self::conflict_text(&path).await {
                Ok(text) => text,
                Err(e) => {
                    failed.push((display, members.len(), e.0));
                    continue;
                }
            };
            let mut resolved: Vec<crate::conflict_detect::ConflictEntry> = Vec::new();
            let mut stale = Vec::new();
            let mut failure = None;
            for entry in &members {
                let replacement = directives
                    .as_ref()
                    .and_then(|map| map.iter().find(|(id, _)| *id == entry.id as f64).map(|(_, value)| value.as_str()))
                    .unwrap_or(content);
                let result = crate::conflict_detect::expand_content_tokens(replacement, entry)
                    .and_then(|expanded| crate::conflict_detect::splice_conflict(&text, entry, &expanded));
                match result {
                    Ok(splice) => {
                        text = splice.text;
                        echo_trimmed += splice.trimmed_leading + splice.trimmed_trailing;
                        resolved.push(entry.clone());
                    }
                    Err(error) => {
                        if resolved
                            .iter()
                            .any(|done| crate::conflict_detect::conflict_regions_equal(&done.block, &entry.block))
                        {
                            stale.push(entry.id);
                            continue;
                        }
                        failure = Some(error.0);
                        break;
                    }
                }
            }
            if let Some(error) = failure {
                failed.push((display, members.len(), error));
                continue;
            }
            // One persisted effect only after every selected splice succeeded for
            // this file. The existing Native caller owns started-effect settlement.
            self.persist_conflict_content(&path, &text).await?;
            {
                let mut history = self.ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner());
                for entry in &resolved {
                    history.invalidate(entry.id);
                }
                for id in stale {
                    history.invalidate(id);
                }
            }
            resolved_count += resolved.len();
            succeeded.push((display, resolved.len(), self.conflict_snapshot_header(&path, &text)));
        }
        let word = |n| if n == 1 { "conflict" } else { "conflicts" };
        let file_word = |n| if n == 1 { "file" } else { "files" };
        let mut lines = Vec::new();
        if !succeeded.is_empty() {
            lines.push(format!(
                "Resolved {resolved_count} {} across {} {}:",
                word(resolved_count),
                succeeded.len(),
                file_word(succeeded.len())
            ));
            for (path, count, _) in &succeeded {
                lines.push(format!("  {path}: {count} {}", word(*count)));
            }
        }
        if directives.is_some() && selected.len() < entries.len() {
            let remaining = entries
                .iter()
                .filter(|entry| !selected.iter().any(|other| other.id == entry.id))
                .map(|e| format!("#{}", e.id))
                .collect::<Vec<_>>();
            lines.push(format!(
                "Directive mode: {} unlisted {} still registered ({}).",
                remaining.len(),
                word(remaining.len()),
                remaining.join(", ")
            ));
        }
        if echo_trimmed > 0 {
            lines.push(format!("Note: dropped {echo_trimmed} content line(s) that duplicated code adjacent to conflict regions — writes replace only the marker block; surrounding lines stay in place."));
        }
        if !failed.is_empty() {
            lines.push(format!(
                "Failed to resolve {} {} — registered entries left intact for retry:",
                failed.len(),
                file_word(failed.len())
            ));
            for (path, count, error) in &failed {
                lines.push(format!("  {path}: {count} {} ({error})", word(*count)));
            }
        }
        let headers = succeeded.iter().filter_map(|(_, _, header)| header.as_ref()).collect::<Vec<_>>();
        if !headers.is_empty() {
            lines.push("Snapshots:".into());
            for header in headers {
                lines.push(format!("  {header}"));
            }
        }
        if stripped && directives.is_none() {
            lines.push(STRIPPED_NOTE.into());
        }
        let text = lines.join("\n");
        if !failed.is_empty() && succeeded.is_empty() {
            return Err(ToolError(text));
        }
        let mut output = ToolOutput::text(text).with_details(json!({}));
        output.is_error = !failed.is_empty();
        Ok(output)
    }
}
