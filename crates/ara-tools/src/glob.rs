//! `glob` tool (OMP `packages/coding-agent/src/tools/glob.ts`).
//!
//! Ported: glob/file/directory targets and semicolon lists (each its own walk
//! root), missing entries skipped with a note unless all are missing, `hidden`
//! and `gitignore` switches (both default true), `.git` always pruned,
//! `node_modules` pruned unless named, non-recursive patterns bounded by their
//! depth, newest-first ordering, `limit` (default and maximum 200) with the
//! result-limit notice, directories suffixed `/`, grouped `#` directory
//! headers, the root-directory refusal, the 5 s timeout notice, and the
//! upstream error texts.
//!
//! Not ported (open): internal URLs (`memory://` globs, path-backed schemes),
//! custom remote operations, streamed partial updates, the TUI renderer.

use crate::engine::{self, Budget, EngineError};
use crate::output::{Notice, truncate_head};
use crate::paths::{self, FindPattern};
use crate::{DEFAULT_MAX_BYTES, ToolContext};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use ara_walk::{self as walk, EntryKind};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 200;
pub const TIMEOUT: Duration = Duration::from_secs(5);

pub const DESCRIPTION: &str = "Globs files and directories with fast pattern matching.\n\n<instruction>\n- `path`: glob, file, or directory; separate targets with `;` (`src/**/*.ts; test/**/*.ts`).\n- `gitignore` defaults `true`. Set `false` for ignored files such as `.env*`, logs, or build output.\n- `hidden` defaults `true`; pair it with `gitignore: false` for ignored dotfiles.\n</instruction>\n\n<output>\nMatches are newest-first and grouped by directory; directories end in `/`.\n</output>";

/// Display paths with their mtimes (ms).
type Listing = Vec<(String, f64)>;

/// Upper bound on concurrent directory scans in one call.
const MAX_SCAN_THREADS: usize = 8;

/// Run `f` over `items` on a bounded set of scoped threads (upstream runs the
/// targets concurrently on its native pool). Falls back to the calling thread
/// when no worker thread can be spawned.
fn scan_concurrently<T: Send>(items: &[usize], f: impl Fn(usize) -> T + Sync) -> Vec<(usize, T)> {
    if items.len() <= 1 {
        return items.iter().map(|&i| (i, f(i))).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let out = std::sync::Mutex::new(Vec::with_capacity(items.len()));
    let work = || {
        loop {
            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let Some(&i) = items.get(k) else { break };
            let r = f(i);
            out.lock().unwrap_or_else(|e| e.into_inner()).push((i, r));
        }
    };
    let workers = items.len().min(std::thread::available_parallelism().map_or(4, |n| n.get())).min(MAX_SCAN_THREADS);
    std::thread::scope(|scope| {
        let spawned = (0..workers).filter(|_| std::thread::Builder::new().spawn_scoped(scope, work).is_ok()).count();
        if spawned == 0 {
            work();
        }
    });
    out.into_inner().unwrap_or_else(|e| e.into_inner())
}

struct Target {
    search_path: PathBuf,
    glob: String,
    has_glob: bool,
}

pub struct GlobTool {
    pub ctx: ToolContext,
    /// Scan budget; upstream `DEFAULT_GLOB_TIMEOUT_MS`.
    pub timeout: Duration,
}

impl GlobTool {
    pub fn new(ctx: ToolContext) -> Self {
        GlobTool { ctx, timeout: TIMEOUT }
    }

    fn execute_blocking(&self, args: &JsonObject, cancel: &CancellationToken) -> Result<ToolOutput, ToolError> {
        let ctx = &self.ctx;
        let err = |s: String| ToolError(s);
        let mut inputs = paths::to_path_list(args.get("path").and_then(Value::as_str));
        if inputs.is_empty() {
            inputs.push(".".into());
        }
        let raw = paths::expand_delimited_entries(ctx, &inputs, paths::Splitter::Find).map_err(err)?;
        let patterns: Vec<String> =
            raw.iter().map(|p| paths::normalize_path_like_input(p).replace('\\', "/")).collect();
        if patterns.iter().any(|p| !p.is_empty() && p.chars().all(|c| c == '/')) {
            return Err(ToolError("Searching from root directory '/' is not allowed".into()));
        }
        if patterns.iter().any(String::is_empty) {
            return Err(ToolError("`path` must contain non-empty globs or paths".into()));
        }
        let (effective, missing) = if patterns.len() > 1 {
            let (valid, missing) = paths::partition_existing(ctx, &patterns, paths::Splitter::Find).map_err(err)?;
            if valid.is_empty() {
                return Err(ToolError(format!("Path not found: {}", missing.join(", "))));
            }
            (valid, missing)
        } else {
            (patterns, Vec::new())
        };
        let mut unique: Vec<String> = Vec::new();
        for p in effective {
            if !unique.contains(&p) {
                unique.push(p);
            }
        }
        let is_single = unique.len() <= 1;
        let to_target = |p: &str| {
            let FindPattern { base, glob, has_glob } = paths::parse_find_pattern(p);
            Target { search_path: paths::resolve_to_cwd(ctx, &base), glob, has_glob }
        };
        let targets: Vec<Target> = unique.iter().map(|p| to_target(p)).collect();
        let scope_path = if is_single {
            paths::format_path_relative_to_cwd(&targets[0].search_path, &ctx.cwd, false)
        } else {
            unique
                .iter()
                .map(|i| {
                    let slash = i.ends_with('/');
                    paths::format_path_relative_to_cwd(&paths::resolve_to_cwd(ctx, i), &ctx.cwd, slash)
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        if targets.iter().any(|t| t.search_path == std::path::Path::new("/")) {
            return Err(ToolError("Searching from root directory '/' is not allowed".into()));
        }
        let limit = match args.get("limit") {
            None | Some(Value::Null) => DEFAULT_LIMIT as f64,
            Some(v) => v.as_f64().unwrap_or(f64::NAN),
        };
        if !limit.is_finite() || limit <= 0.0 {
            return Err(ToolError("Limit must be a positive number".into()));
        }
        let limit = (limit.floor() as usize).clamp(1, MAX_LIMIT);
        let include_hidden = args.get("hidden").and_then(Value::as_bool).unwrap_or(true);
        let use_gitignore = args.get("gitignore").and_then(Value::as_bool).unwrap_or(true);
        let budget = Budget { cancel: cancel.clone(), deadline: Instant::now() + self.timeout };

        // Stat every target first (upstream prepares all targets before scanning).
        let mut prepared: Vec<Option<Listing>> = Vec::new();
        for t in &targets {
            let meta = match std::fs::metadata(&t.search_path) {
                Ok(m) => m,
                Err(e) if paths::is_missing(&e) => {
                    if is_single {
                        return Err(ToolError(format!("Path not found: {scope_path}")));
                    }
                    prepared.push(Some(Vec::new()));
                    continue;
                }
                Err(e) => return Err(ToolError(format!("Cannot access {}: {e}", t.search_path.display()))),
            };
            if !t.has_glob && meta.is_file() {
                let display = paths::format_path_relative_to_cwd(&t.search_path, &ctx.cwd, false);
                prepared.push(Some(vec![(display, walk::mtime_ms(&meta))]));
            } else if !meta.is_dir() {
                if is_single {
                    return Err(ToolError(format!("Path is not a directory: {}", t.search_path.display())));
                }
                prepared.push(Some(Vec::new()));
            } else {
                prepared.push(None);
            }
        }
        let scan = |i: usize| -> Result<Listing, EngineError> {
            let t = &targets[i];
            let req = engine::GlobRequest {
                root: &t.search_path,
                pattern: &t.glob,
                recursive: false,
                include_hidden,
                use_gitignore,
                max_results: limit,
            };
            let entries = engine::glob(&req, &budget)?;
            Ok(entries
                .into_iter()
                .map(|e| {
                    let abs = t.search_path.join(&e.relative);
                    (paths::format_path_relative_to_cwd(&abs, &ctx.cwd, e.kind == EntryKind::Dir), e.mtime_ms)
                })
                .collect())
        };
        let pending: Vec<usize> = (0..targets.len()).filter(|i| prepared[*i].is_none()).collect();
        let mut scans: Vec<Option<Result<Listing, EngineError>>> = (0..targets.len()).map(|_| None).collect();
        for (i, r) in scan_concurrently(&pending, scan) {
            scans[i] = Some(r);
        }
        // Per target, in order: (walked, listing). On timeout upstream returns
        // only what its walks streamed, not the directly named files.
        let mut groups: Vec<(bool, Listing)> = Vec::new();
        let mut timed_out = false;
        for (direct, scan) in prepared.into_iter().zip(scans) {
            match (direct, scan) {
                (Some(list), _) => groups.push((false, list)),
                (None, Some(Ok(list))) => groups.push((true, list)),
                (None, Some(Err(EngineError::Timeout))) => timed_out = true,
                (None, Some(Err(EngineError::Aborted))) => return Err(ToolError("Glob was aborted".into())),
                (None, Some(Err(e))) => return Err(ToolError(e.to_string())),
                (None, None) => {}
            }
        }
        let mut merged: Listing =
            groups.into_iter().filter(|(walked, _)| *walked || !timed_out).flat_map(|(_, list)| list).collect();
        let mut seen = std::collections::HashSet::new();
        merged.retain(|(p, _)| seen.insert(p.clone()));
        merged.sort_by(|a, b| b.1.total_cmp(&a.1));
        let files: Vec<String> = merged.into_iter().map(|(p, _)| p).collect();

        let notice = timed_out.then(|| {
            let ms = self.timeout.as_millis();
            let secs = if ms.is_multiple_of(1000) { (ms / 1000).to_string() } else { format!("{:.1}", ms as f64 / 1000.0) };
            if files.is_empty() {
                format!(
                    "Glob timed out after {secs}s before finding any matches — the scan is incomplete, NOT proof of absence. The walk is bounded by directory size, not pattern width; scope the search to a deeper directory (e.g. `sub/dir/*.ext` instead of `*.ext` at a huge root)."
                )
            } else {
                format!(
                    "glob timed out after {secs}s; returning {} partial matches — results are incomplete, scope to a deeper directory instead of retrying blindly",
                    files.len()
                )
            }
        });
        let missing_note = (!missing.is_empty()).then(|| format!("Skipped missing paths: {}", missing.join(", ")));
        let mut details = json!({"scopePath": scope_path, "cwd": ctx.cwd.to_string_lossy(), "files": []});
        if !missing.is_empty() {
            details["missingPaths"] = json!(missing);
        }
        if files.is_empty() {
            let mut parts = Vec::new();
            if !timed_out {
                parts.push("No files found matching pattern".to_string());
            }
            parts.extend(notice);
            parts.extend(missing_note);
            details["fileCount"] = json!(0);
            details["truncated"] = json!(timed_out);
            return Ok(ToolOutput::text(parts.join("\n")).with_details(details));
        }
        let limit_reached = files.len() >= limit;
        let limited: Vec<String> = files.into_iter().take(limit).collect();
        let mut raw_output = paths::format_grouped_paths(&limited);
        let trailing: Vec<String> = notice.into_iter().chain(missing_note).collect();
        if !trailing.is_empty() {
            raw_output = format!("{raw_output}\n\n{}", trailing.join("\n"));
        }
        let truncation = truncate_head(&raw_output, usize::MAX, DEFAULT_MAX_BYTES);
        let mut n = Notice::default();
        n.truncation(&truncation);
        if limit_reached {
            n.result_limit(limit);
        }
        details["fileCount"] = json!(limited.len());
        details["files"] = json!(limited);
        details["truncated"] = json!(timed_out || limit_reached || truncation.truncated);
        if limit_reached {
            details["resultLimitReached"] = json!(limit);
        }
        if truncation.truncated {
            details["truncation"] = truncation.details();
        }
        Ok(ToolOutput::text(format!("{}{}", truncation.content, n.render())).with_details(details))
    }
}

#[async_trait]
impl AgentTool for GlobTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "glob".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "glob, file, or directory to search — a single path or a semicolon-delimited list (\"src/**/*.ts; test/**/*.ts\"). Omitted -> searches the workspace root (\".\")"},
                    "hidden": {"type": "boolean", "description": "include hidden files"},
                    "gitignore": {"type": "boolean", "description": "respect gitignore"},
                    "limit": {"type": "number", "description": "max results"}
                },
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let tool = GlobTool { ctx: self.ctx.clone(), timeout: self.timeout };
        crate::run_blocking("Glob", cancel, move |work| tool.execute_blocking(&args, &work)).await
    }
}
