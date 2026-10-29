//! Workspace path checks of the Capability_Test_Host tools (requirement 2.3).
//!
//! Every path a model passes to a `ws_*` tool goes through [`confine`] before
//! the Core implementation sees it. A rejection is a known tool error
//! (`path_outside_workspace`), never an unknown effect.

use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use ara_agent_lite_harness::core::{
    local_tools,
    rpi_agent::{types::TextContentOrImage, AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::Tool,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    digest::refuse_link,
    workspace::{canonical, link_count},
};

/// What the tool will do with the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Intent {
    /// `ws_read_file`, `ws_list_dir`.
    ReadExisting,
    /// `ws_write_file`: the target may not exist yet.
    WriteMaybeNew,
    /// `ws_edit_file`.
    EditExisting,
    /// Root directory of `ws_search_files`.
    SearchRoot,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PathRejection {
    Empty,
    /// Lexically or after resolving links, the path is not under the root.
    OutsideWorkspace {
        resolved: PathBuf,
    },
    /// The target is a link (or a file with several hard links) that could
    /// alias a file outside the workspace.
    LinkComponent {
        path: PathBuf,
    },
}

impl PathRejection {
    /// Error code returned to the model.
    pub fn code(&self) -> &'static str {
        match self {
            PathRejection::Empty => "empty_path",
            PathRejection::OutsideWorkspace { .. } | PathRejection::LinkComponent { .. } => "path_outside_workspace",
        }
    }
}

/// Join `raw` to `root` when relative and remove `.` and `..` lexically.
fn normalize(root: &Path, raw: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in root.join(raw).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // Never pops a prefix or root; `C:\..` stays `C:\`.
                if matches!(normalized.components().next_back(), Some(Component::Normal(_))) {
                    normalized.pop();
                }
            }
            other => normalized.push(other),
        }
    }
    normalized
}

/// Resolve `raw` against the canonical workspace `root`. Returns the path to
/// hand to the Core tool: the canonical target when it exists, otherwise the
/// normalized path whose deepest existing ancestor is inside `root`.
pub fn confine(root: &Path, raw: &str, intent: Intent) -> Result<PathBuf, PathRejection> {
    if raw.trim().is_empty() {
        return Err(PathRejection::Empty);
    }
    let normalized = normalize(root, raw);
    let outside = || PathRejection::OutsideWorkspace { resolved: normalized.clone() };
    // The deepest existing ancestor (the target itself when it exists) must be
    // inside the root once junctions and symbolic links resolve. Comparing the
    // canonical form also accepts an absolute path that differs only in case.
    let mut existing = normalized.as_path();
    while fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().ok_or_else(outside)?;
    }
    let resolved = canonical(existing).map_err(|_| outside())?;
    if !resolved.starts_with(root) {
        return Err(PathRejection::OutsideWorkspace { resolved });
    }
    if existing != normalized.as_path() {
        // Only plain names remain after normalization.
        let rest = normalized.strip_prefix(existing).map_err(|_| outside())?;
        return Ok(resolved.join(rest));
    }
    let metadata = fs::symlink_metadata(&normalized)
        .map_err(|_| PathRejection::OutsideWorkspace { resolved: normalized.clone() })?;
    let is_link = refuse_link(&normalized, &metadata).is_err();
    let aliased = metadata.is_file() && link_count(&normalized).map_or(true, |links| links != 1);
    match intent {
        // Writing through or replacing a link, or into a hard-linked file, could change a file outside.
        Intent::WriteMaybeNew | Intent::EditExisting | Intent::ReadExisting if is_link || aliased => {
            Err(PathRejection::LinkComponent { path: normalized })
        }
        _ => Ok(resolved),
    }
}

/// Whole-file write and edit counts of one task (requirement 20.4). A write
/// is counted once it succeeded, as `write_existing` when its target existed
/// before the call and `write_new` otherwise. Every `ws_edit_file` call is
/// either `edit_ok` or `edit_rejected` (path rejection, a stale SHA or an
/// inapplicable block included); without the P2-e switch there is no edit
/// tool, so both stay 0.
#[derive(Debug, Default)]
pub struct WriteCounters {
    write_existing: AtomicU64,
    write_new: AtomicU64,
    edit_ok: AtomicU64,
    edit_rejected: AtomicU64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteCounts {
    pub write_existing: u64,
    pub write_new: u64,
    pub edit_ok: u64,
    pub edit_rejected: u64,
}

impl WriteCounters {
    pub fn snapshot(&self) -> WriteCounts {
        WriteCounts {
            write_existing: self.write_existing.load(Ordering::SeqCst),
            write_new: self.write_new.load(Ordering::SeqCst),
            edit_ok: self.edit_ok.load(Ordering::SeqCst),
            edit_rejected: self.edit_rejected.load(Ordering::SeqCst),
        }
    }

    fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::SeqCst);
    }
}

/// `source` of the structured unknown-effect status the wrappers return.
pub const DELEGATE_SOURCE: &str = "capability_host_delegate";

/// First-line phrases of Core's builtin effect-tool errors that mean the
/// effects are unknown. Mirrors Core `effect_status` (crate-private there);
/// Core only classifies its own builtin names, not these `ws_*` wrappers.
const UNKNOWN_EFFECT_PHRASES: [&str; 4] = [
    "effects_unknown:",
    "partial effects may remain",
    "effects may be unknown",
    "do not replay the command without reconciling its effects",
];

/// P2-e sentence appended to the `ws_write_file` description (20.2), worded
/// as in the design.
pub const WRITE_NEW_ONLY: &str = "只用于创建尚不存在的文件，修改已有文件请用 ws_edit_file";

const WORKSPACE_NOTE: &str = "Paths are relative to the task workspace root; absolute paths are accepted only inside \
                              it, and any path that resolves outside the workspace is rejected with \
                              path_outside_workspace.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WsKind {
    Read,
    List,
    Search,
    Write,
    Edit,
}

impl WsKind {
    fn of(name: &str) -> Option<(Self, &'static str)> {
        Some(match name {
            "ws_read_file" => (Self::Read, "read_file"),
            "ws_list_dir" => (Self::List, "list_dir"),
            "ws_search_files" => (Self::Search, "search_files"),
            "ws_write_file" => (Self::Write, "write_file"),
            "ws_edit_file" => (Self::Edit, "edit_file"),
            _ => return None,
        })
    }

    fn intent(self) -> Intent {
        match self {
            Self::Read | Self::List => Intent::ReadExisting,
            Self::Search => Intent::SearchRoot,
            Self::Write => Intent::WriteMaybeNew,
            Self::Edit => Intent::EditExisting,
        }
    }

    fn read_only(self) -> bool {
        matches!(self, Self::Read | Self::List | Self::Search)
    }
}

/// A `ws_*` tool: confines the path argument, then delegates to the Core
/// builtin of the same name created by `local_tools::tools(workspace.root)`.
pub struct WsTool {
    kind: WsKind,
    schema: Tool,
    inner: Arc<dyn AgentTool>,
    root: PathBuf,
    counters: Arc<WriteCounters>,
}

/// Names of the `ws_*` wrappers.
pub const WS_TOOLS: [&str; 5] = ["ws_read_file", "ws_list_dir", "ws_search_files", "ws_write_file", "ws_edit_file"];

/// Whether `name` is a read-only `ws_*` wrapper (the Plan-mode tool set).
pub fn is_read_only(name: &str) -> bool {
    WsKind::of(name).is_some_and(|(kind, _)| kind.read_only())
}

/// Build the wrapper `name` over the Core builtins of `root` (canonical).
/// `edit_file_available` is the P2-e switch: it adds `ws_edit_file` and
/// restricts the `ws_write_file` description to new files (20.2).
pub fn ws_tool(
    name: &str,
    root: &Path,
    counters: Arc<WriteCounters>,
    edit_file_available: bool,
) -> Option<Arc<dyn AgentTool>> {
    let (kind, core_name) = WsKind::of(name)?;
    if kind == WsKind::Edit && !edit_file_available {
        return None;
    }
    let inner = local_tools::tools(root.to_owned()).into_iter().find(|tool| tool.schema().name == core_name)?;
    let original = inner.schema();
    let kept: Vec<&str> = original
        .description
        .split_inclusive(". ")
        .filter(|sentence| !sentence.to_lowercase().contains("absolute paths are allowed"))
        .collect();
    let mut description = format!("{} {WORKSPACE_NOTE}", kept.concat().trim_end());
    if kind == WsKind::Write && edit_file_available {
        description.push(' ');
        description.push_str(WRITE_NEW_ONLY);
    }
    let schema = Tool {
        name: name.to_owned(),
        description,
        parameters: original.parameters.clone(),
        constrained_sampling: original.constrained_sampling.clone(),
    };
    Some(Arc::new(WsTool { kind, schema, inner, root: root.to_owned(), counters }))
}

fn reports_unknown_effects(message: &str) -> bool {
    let first = message.lines().next().unwrap_or("");
    UNKNOWN_EFFECT_PHRASES.iter().any(|phrase| first.contains(phrase))
}

#[async_trait]
impl AgentTool for WsTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        &self.schema.name
    }

    async fn execute(
        &self,
        tool_call_id: &str,
        params: Value,
        signal: CancellationToken,
        on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let edit = self.kind == WsKind::Edit;
        let reject = |error: AgentError| {
            if edit {
                WriteCounters::bump(&self.counters.edit_rejected);
            }
            Err(error)
        };
        let mut params = params;
        let raw = match params.get("path") {
            Some(Value::String(raw)) => raw.clone(),
            None if self.kind == WsKind::Search => ".".to_owned(),
            _ => return reject(AgentError::Tool("missing string argument: path".into())),
        };
        let target = match confine(&self.root, &raw, self.kind.intent()) {
            Ok(target) => target,
            // Only the model's own input is echoed: a resolved target could
            // name a file outside the workspace.
            Err(rejection) => {
                return reject(AgentError::Tool(json!({"error": rejection.code(), "path": raw}).to_string()))
            }
        };
        let existed = target.exists();
        if let Some(object) = params.as_object_mut() {
            object.insert("path".into(), Value::String(target.to_string_lossy().into_owned()));
            // Without any versioned option Core takes its legacy edit path,
            // whose receipt has no SHA; `dry_run=false` selects the versioned
            // edit (same commit semantics) so every result names the new SHA (20.5).
            if edit && !["expected_sha256", "additional_edits", "dry_run"].iter().any(|key| object.contains_key(*key)) {
                object.insert("dry_run".into(), Value::Bool(false));
            }
        }
        match self.inner.execute(tool_call_id, params, signal, on_update).await {
            Ok(mut result) => {
                if self.kind == WsKind::Search {
                    // The Core report echoes the query. The model already has
                    // it, and an echoed Oracle feature string would read as a
                    // leak in the post-run scan (2.7), so the echo is dropped.
                    for content in &mut result.content {
                        if let TextContentOrImage::Text(block) = content {
                            if let Ok(Value::Object(mut report)) = serde_json::from_str::<Value>(&block.text) {
                                if report.remove("query").is_some() {
                                    block.text = Value::Object(report).to_string();
                                }
                            }
                        }
                    }
                }
                match self.kind {
                    WsKind::Write if existed => WriteCounters::bump(&self.counters.write_existing),
                    WsKind::Write => WriteCounters::bump(&self.counters.write_new),
                    WsKind::Edit => WriteCounters::bump(&self.counters.edit_ok),
                    _ => {}
                }
                Ok(result)
            }
            Err(AgentError::Tool(message)) if reports_unknown_effects(&message) => {
                if edit {
                    WriteCounters::bump(&self.counters.edit_rejected);
                }
                let mut result = AgentToolResult::text(format!("tool failed: {message}"));
                result.details = json!({"effects_unknown": true, "source": DELEGATE_SOURCE});
                Ok(result)
            }
            Err(error) => reject(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_escapes_and_foreign_roots_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical(dir.path()).unwrap().join("ws");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/a.cpp"), b"int a;\n").unwrap();
        assert_eq!(confine(&root, "src/a.cpp", Intent::ReadExisting).unwrap(), root.join("src").join("a.cpp"));
        assert_eq!(
            confine(&root, "./src/../src/./a.cpp", Intent::EditExisting).unwrap(),
            root.join("src").join("a.cpp")
        );
        assert_eq!(
            confine(&root, "new/dir/b.cpp", Intent::WriteMaybeNew).unwrap(),
            root.join("new").join("dir").join("b.cpp")
        );
        assert_eq!(confine(&root, ".", Intent::SearchRoot).unwrap(), root);
        for raw in ["..", "../ws-other/x", "src/../../x", "/etc/passwd", r"C:\Windows\win.ini", r"\x", "C:x"] {
            assert!(
                matches!(confine(&root, raw, Intent::ReadExisting), Err(PathRejection::OutsideWorkspace { .. })),
                "{raw}"
            );
        }
        assert_eq!(confine(&root, "  ", Intent::ReadExisting), Err(PathRejection::Empty));
        assert_eq!(PathRejection::Empty.code(), "empty_path");
    }
}
