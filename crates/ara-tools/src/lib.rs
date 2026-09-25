//! Built-in tools (OMP `packages/coding-agent/src/tools/{read,write,bash,grep,glob}.ts`,
//! `exec/`, `session/streaming-output.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! These tools perform real file and process effects. They are not a
//! sandbox: the host decides which calls may run (`LoopHooks::before_tool_call`)
//! and which directory is the working root.

pub mod bash;
pub mod edit;
pub mod engine;
pub mod glob;
pub mod grep;
pub mod output;
pub mod paths;
pub mod read;
pub mod write;

use ara_agent::AgentTool;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Upstream `DEFAULT_MAX_LINES` / `DEFAULT_MAX_BYTES` (session/streaming-output.ts).
pub const DEFAULT_MAX_LINES: usize = 3000;
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;

/// Shared tool settings chosen by the host.
#[derive(Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    /// `N|text` line prefixes on reads (OMP `readLineNumbers`, default off).
    pub line_numbers: bool,
    /// Edit mode of the `edit` tool (OMP `edit.mode`, default hashline).
    edit_mode: pi_edit::EditMode,
    /// Whether the host exposes the `edit` tool (OMP `hasEditTool`, default true).
    edit_enabled: bool,
    /// File snapshots shared by `read`, `grep` and `edit` for one session
    /// (OMP `getEditStore(session)`): tags, seen lines, clipboard registers.
    pub edit_store: pi_edit::EditStore,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("line_numbers", &self.line_numbers)
            .field("edit_mode", &self.edit_mode)
            .field("edit_enabled", &self.edit_enabled)
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    /// A relative `cwd` is made absolute against the process directory, so
    /// `.` names a real directory rather than an empty path.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let cwd = cwd.into();
        let cwd = std::path::absolute(&cwd).unwrap_or(cwd);
        ToolContext {
            cwd: normalize(&cwd),
            line_numbers: false,
            edit_mode: pi_edit::EditMode::Hashline,
            edit_enabled: true,
            edit_store: pi_edit::EditStore::new(),
        }
    }

    /// Select the edit mode and whether the `edit` tool is exposed.
    pub fn with_edit(mut self, mode: pi_edit::EditMode, edit_enabled: bool) -> Self {
        self.edit_mode = mode;
        self.edit_enabled = edit_enabled;
        self
    }

    pub fn edit_mode(&self) -> pi_edit::EditMode {
        self.edit_mode
    }

    /// Hashline display for `read`, `grep` and `write`: `[path#TAG]` headers
    /// and `N:text` rows (OMP `resolveFileDisplayMode`: the edit tool is
    /// exposed in hashline mode).
    pub fn hashlines(&self) -> bool {
        self.edit_enabled && self.edit_mode == pi_edit::EditMode::Hashline
    }

    /// Header path for a hashline read (`formatReadHashlineHeader`): cwd-relative
    /// inside cwd, otherwise absolute with the home directory shortened to `~`.
    pub fn hashline_display(&self, abs: &Path) -> String {
        let display = paths::format_path_relative_to_cwd(abs, &self.cwd, false);
        if !Path::new(&display).is_absolute() {
            return display;
        }
        match std::env::var_os("HOME").map(PathBuf::from) {
            Some(home) if !home.as_os_str().is_empty() => match abs.strip_prefix(&home) {
                Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
                Ok(rest) => format!("~/{}", rest.to_string_lossy()),
                Err(_) => display,
            },
            _ => display,
        }
    }

    /// Resolve a model-supplied path: `~` expands to `$HOME`, relative paths
    /// join the working directory, and `.`/`..` are normalized lexically
    /// (OMP `resolveToCwd`/`expandPath`).
    pub fn resolve(&self, path: &str) -> PathBuf {
        let expanded = if path == "~" {
            std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(path))
        } else if let Some(rest) = path.strip_prefix("~/") {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or_else(|| PathBuf::from(path))
        } else {
            PathBuf::from(path)
        };
        let joined = if expanded.is_absolute() { expanded } else { self.cwd.join(expanded) };
        normalize(&joined)
    }

    /// Path shown back to the model: relative to cwd when inside it.
    pub fn display(&self, path: &Path) -> String {
        path.strip_prefix(&self.cwd)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned())
    }
}

/// Lexical normalization of `.` and `..` (no filesystem access, no symlink resolution).
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// `read`, `write`, `edit`, `bash`, `grep` and `glob` bound to one working
/// directory and one snapshot store.
pub fn builtin_tools(ctx: ToolContext) -> Vec<Arc<dyn AgentTool>> {
    vec![
        Arc::new(read::ReadTool { ctx: ctx.clone() }),
        Arc::new(write::WriteTool { ctx: ctx.clone() }),
        Arc::new(edit::EditTool::new(ctx.clone())),
        Arc::new(bash::BashTool { ctx: ctx.clone() }),
        Arc::new(grep::GrepTool::new(ctx.clone())),
        Arc::new(glob::GlobTool::new(ctx)),
    ]
}

/// Run blocking tool work on the blocking pool. The work gets a child of
/// `cancel`; aborting the run, or dropping the returned future (a host-side
/// timeout), cancels it, so abandoned filesystem scans stop promptly.
pub(crate) async fn run_blocking<F>(
    label: &'static str,
    cancel: tokio_util::sync::CancellationToken,
    f: F,
) -> Result<ara_agent::ToolOutput, ara_agent::ToolError>
where
    F: FnOnce(tokio_util::sync::CancellationToken) -> Result<ara_agent::ToolOutput, ara_agent::ToolError>
        + Send
        + 'static,
{
    let work = cancel.child_token();
    let _stop_on_drop = work.clone().drop_guard();
    let job = tokio::task::spawn_blocking(move || f(work));
    tokio::select! {
        r = job => r.map_err(|e| ara_agent::ToolError(format!("{label} failed: {e}")))?,
        _ = cancel.cancelled() => Err(ara_agent::ToolError(format!("{label} was aborted"))),
    }
}

/// Human-readable byte size (OMP `formatBytes`).
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}
