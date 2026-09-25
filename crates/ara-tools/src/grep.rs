//! `grep` tool (OMP `packages/coding-agent/src/tools/grep.ts`,
//! `match-line-format.ts`, `grouped-file-output.ts`).
//!
//! Ported: regex search with the upstream engine fallbacks, semicolon path
//! lists (plus comma and whitespace recovery), globs in paths, `file:N-M`
//! line-range filters (context outside the range dropped), case and gitignore
//! switches, 1 line of context before and 3 after, 512-column line cap,
//! file pagination (`skip`, 20 files per page, round-robin across files, 20
//! matches per file or 200 for a single file), grouped `#` directory headers,
//! `*N|line` match lines and ` N|line` context lines with `...` gaps, missing
//! path and oversized-file notes, 30 s timeout, and the upstream error texts.
//!
//! Not ported (open): internal URLs and archives as search targets, hashline
//! snapshot anchors (tied to the hashline edit tool), SSH approval tiers,
//! the TUI renderer.

use crate::engine::{self, Budget, EngineError, GrepMatch, GrepParams};
use crate::output::{Notice, truncate_head};
use crate::paths::{self, LineRange, SearchScope};
use crate::{DEFAULT_MAX_BYTES, ToolContext};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_FILE_LIMIT: usize = 20;
pub const MULTI_FILE_PER_FILE_MATCHES: usize = 20;
pub const SINGLE_FILE_MATCHES: usize = 200;
const INTERNAL_TOTAL_CAP: u64 = 2000;
/// Settings defaults `grep.contextBefore` / `grep.contextAfter`.
pub const CONTEXT_BEFORE: usize = 1;
pub const CONTEXT_AFTER: usize = 3;
/// `DEFAULT_MAX_COLUMN` (session/streaming-output.ts).
pub const MAX_COLUMN: usize = 512;
pub const TIMEOUT: Duration = Duration::from_secs(30);

pub const DESCRIPTION: &str = "Searches files: Rust regex, PCRE2 fallback.\n\n<instruction>\n- `path`: known files, directories, globs; roots `;`-separated.\n- Broad searches may time out → narrow scope or use `glob` first.\n- One-file line selector: `src/foo.ts:50-100`; never selects search root.\n- Literal `\\n` or `\\\\n` enables cross-line patterns.\n</instruction>\n\n<critical>\n- MUST use instead of shell `grep`/`rg`.\n</critical>";

struct PathSpec {
    original: String,
    clean: String,
    ranges: Option<Vec<LineRange>>,
}

fn parse_path_specs(ctx: &ToolContext, entries: &[String]) -> Result<Vec<PathSpec>, String> {
    let mut specs = Vec::new();
    for entry in entries {
        let strict = paths::split_path_and_sel(entry);
        let split = paths::split_path_and_sel_preferring_literal(ctx, entry);
        let literal = strict.1.is_some() && split.1.is_none();
        let mut clean = if literal { ctx.resolve(entry).to_string_lossy().into_owned() } else { entry.clone() };
        let mut ranges = None;
        if !literal && let Some(sel) = &split.1 {
            let Some(parsed) = paths::parse_line_ranges(sel)? else {
                return Err(format!(
                    "path entry \"{entry}\" — only line-range selectors like \":50-100\" are supported (no \":raw\"/\":conflicts\")"
                ));
            };
            if paths::has_glob_path_chars(&split.0) {
                return Err(format!("Line-range selector requires a single file, not a glob: {entry}"));
            }
            clean = split.0.clone();
            ranges = Some(parsed);
        }
        specs.push(PathSpec { original: entry.clone(), clean, ranges });
    }
    Ok(specs)
}

/// `lineRangeFetchCap`: widen the per-file fetch so filtering can still fill the cap.
fn line_range_fetch_cap(specs: &[PathSpec], keep: u64) -> u64 {
    let mut cap = 0u64;
    for r in specs.iter().filter_map(|s| s.ranges.as_ref()).flatten() {
        cap = cap.max(r.end.unwrap_or((r.start - 1).saturating_add(keep)));
    }
    cap.min(engine::MAX_FILE_BYTES)
}

/// `formatMatchLine` (plain mode): `*N|line` for matches, ` N|line` for context.
pub fn format_match_line(line_number: u64, line: &str, is_match: bool) -> String {
    format!("{}{line_number}|{line}", if is_match { '*' } else { ' ' })
}

fn engine_error(e: EngineError, timeout: Duration) -> ToolError {
    match e {
        EngineError::Regex(m) => ToolError(format!("Invalid regex: {m}")),
        EngineError::Timeout => ToolError(format!(
            "Grep timed out after {}s; narrow paths or pattern, or scope with `glob` first",
            timeout.as_secs()
        )),
        EngineError::Aborted => ToolError("Grep was aborted".into()),
        other => ToolError(other.to_string()),
    }
}

struct Search {
    matches: Vec<GrepMatch>,
    limit_reached: bool,
    skipped_oversized: u64,
}

fn run_search(pattern: &str, scope: &SearchScope, params: &GrepParams, budget: &Budget) -> Result<Search, EngineError> {
    let matcher = engine::build_matcher(pattern, params.ignore_case, params.multiline)?;
    let targets: Option<Vec<(PathBuf, Option<String>)>> = if let Some(files) = &scope.exact_file_paths {
        Some(files.iter().map(|f| (f.clone(), None)).collect())
    } else {
        scope.multi_targets.as_ref().map(|t| t.iter().map(|t| (t.base.clone(), t.glob.clone())).collect())
    };
    let Some(targets) = targets else {
        let r = engine::grep(&matcher, &scope.search_path, scope.glob.as_deref(), params, budget)?;
        return Ok(Search {
            matches: r.matches,
            limit_reached: r.limit_reached,
            skipped_oversized: r.skipped_oversized,
        });
    };
    let mut out = Search { matches: Vec::new(), limit_reached: false, skipped_oversized: 0 };
    let mut seen = HashSet::new();
    for (base, glob) in targets {
        let r = engine::grep(&matcher, &base, glob.as_deref(), params, budget)?;
        out.skipped_oversized += r.skipped_oversized;
        out.limit_reached |= r.limit_reached;
        for m in r.matches {
            let abs = crate::normalize(&base.join(&m.path));
            // Overlapping targets surface the same line twice; keep the first.
            if !seen.insert((abs.clone(), m.line_number)) {
                continue;
            }
            let rel = abs.strip_prefix(&scope.search_path).map(|p| p.to_string_lossy().replace('\\', "/"));
            let path = rel.unwrap_or_else(|_| abs.to_string_lossy().into_owned());
            out.matches.push(GrepMatch { path, ..m });
        }
    }
    Ok(out)
}

fn match_abs(match_path: &str, search_path: &Path) -> PathBuf {
    if match_path.is_empty() { search_path.to_path_buf() } else { crate::normalize(&search_path.join(match_path)) }
}

pub struct GrepTool {
    pub ctx: ToolContext,
    /// Wall-clock budget per call; upstream `SEARCH_GREP_TIMEOUT_MS`.
    pub timeout: Duration,
}

impl GrepTool {
    pub fn new(ctx: ToolContext) -> Self {
        GrepTool { ctx, timeout: TIMEOUT }
    }

    fn execute_blocking(&self, args: &JsonObject, cancel: &CancellationToken) -> Result<ToolOutput, ToolError> {
        let ctx = &self.ctx;
        let pattern = args.get("pattern").and_then(Value::as_str).unwrap_or_default().to_string();
        if pattern.trim().is_empty() {
            return Err(ToolError("Pattern must not be empty".into()));
        }
        let skip = match args.get("skip") {
            None | Some(Value::Null) => 0usize,
            Some(v) => match v.as_f64() {
                Some(f) if f.is_finite() && f >= 0.0 => f.floor() as usize,
                _ => return Err(ToolError("Skip must be a non-negative number".into())),
            },
        };
        let raw_path = args.get("path").and_then(Value::as_str);
        let mut entries = paths::to_path_list(raw_path);
        if entries.is_empty() {
            entries.push(".".into());
        }
        let entries = paths::expand_delimited_entries(ctx, &entries, paths::Splitter::Search).map_err(ToolError)?;
        let specs = parse_path_specs(ctx, &entries).map_err(ToolError)?;
        let searchable: Vec<String> = specs.iter().map(|s| s.clean.clone()).collect();
        let scope = paths::resolve_search_scope(ctx, &searchable).map_err(ToolError)?;

        let mut ranges_by_abs: HashMap<PathBuf, Vec<LineRange>> = HashMap::new();
        for spec in &specs {
            let Some(ranges) = &spec.ranges else { continue };
            let abs = ctx.resolve(&spec.clean);
            match std::fs::metadata(&abs) {
                Err(_) => return Err(ToolError(format!("Path not found for line-range selector: {}", spec.original))),
                Ok(m) if !m.is_file() => {
                    return Err(ToolError(format!(
                        "Line-range selector requires a single file: {} is a directory",
                        spec.original
                    )));
                }
                Ok(_) => ranges_by_abs.entry(abs).or_default().extend(ranges.iter().copied()),
            }
        }

        let case_sensitive = args.get("case").and_then(Value::as_bool).unwrap_or(true);
        let use_gitignore = args.get("gitignore").and_then(Value::as_bool).unwrap_or(true);
        let multiline = pattern.contains('\n') || pattern.contains("\\n");
        let is_multi_scope = scope.is_directory || scope.exact_file_paths.is_some() || scope.multi_targets.is_some();
        let per_file_cap = if is_multi_scope { MULTI_FILE_PER_FILE_MATCHES } else { SINGLE_FILE_MATCHES };
        let has_ranges = specs.iter().any(|s| s.ranges.is_some());
        let keep = per_file_cap as u64 + 1;
        let per_file_fetch = if has_ranges { keep.max(line_range_fetch_cap(&specs, keep)) } else { keep };
        let max_count =
            if has_ranges { INTERNAL_TOTAL_CAP.div_ceil(keep) * per_file_fetch } else { INTERNAL_TOTAL_CAP };
        let params = GrepParams {
            ignore_case: !case_sensitive,
            multiline,
            include_hidden: true,
            use_gitignore,
            max_count: Some(max_count),
            max_count_per_file: Some(per_file_fetch),
            context_before: CONTEXT_BEFORE,
            context_after: CONTEXT_AFTER,
            max_columns: Some(MAX_COLUMN),
        };
        let budget = Budget { cancel: cancel.clone(), deadline: Instant::now() + self.timeout };
        let mut result = run_search(&pattern, &scope, &params, &budget).map_err(|e| engine_error(e, self.timeout))?;

        if !ranges_by_abs.is_empty() {
            result.matches.retain_mut(|m| {
                let Some(ranges) = ranges_by_abs.get(&match_abs(&m.path, &scope.search_path)) else { return true };
                if !paths::is_line_in_ranges(m.line_number, ranges) {
                    return false;
                }
                // Context outside the allowed ranges would leak excluded lines.
                m.context_before.retain(|c| paths::is_line_in_ranges(c.line_number, ranges));
                m.context_after.retain(|c| paths::is_line_in_ranges(c.line_number, ranges));
                true
            });
        }

        let format_path = |p: &str| paths::format_result_path(p, scope.is_directory, &scope.search_path, &ctx.cwd);

        // Group by file in encounter order; trim hot files before windowing.
        let mut file_order: Vec<String> = Vec::new();
        let mut by_path: HashMap<String, Vec<GrepMatch>> = HashMap::new();
        for m in result.matches {
            if !by_path.contains_key(&m.path) {
                file_order.push(m.path.clone());
            }
            by_path.entry(m.path.clone()).or_default().push(m);
        }
        let mut per_file_limit_reached = false;
        for list in by_path.values_mut() {
            if list.len() > per_file_cap {
                per_file_limit_reached = true;
                list.truncate(per_file_cap);
            }
        }
        let total_files = file_order.len();
        let total_label = if result.limit_reached { format!("{total_files}+") } else { total_files.to_string() };
        let can_paginate = is_multi_scope;
        let skip_files = if can_paginate { skip.min(total_files) } else { 0 };
        let window: Vec<String> = if can_paginate {
            file_order.iter().skip(skip_files).take(DEFAULT_FILE_LIMIT).cloned().collect()
        } else {
            file_order.clone()
        };
        let file_limit_reached = can_paginate && total_files > skip_files + DEFAULT_FILE_LIMIT;

        // Round-robin across the window for diversity.
        let mut selected: Vec<GrepMatch> = Vec::new();
        let mut lists: Vec<std::vec::IntoIter<GrepMatch>> =
            window.iter().map(|f| by_path.remove(f).unwrap_or_default().into_iter()).collect();
        loop {
            let mut any = false;
            for it in lists.iter_mut() {
                if let Some(m) = it.next() {
                    selected.push(m);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
        let next_skip = skip_files + window.len();
        let limit_message = file_limit_reached.then(|| {
            format!(
                "Showing files {}-{next_skip} of {total_label}. Use skip={next_skip} for the next page, or narrow paths/pattern.",
                skip_files + 1
            )
        });

        // Explicit file targets past the 4 MB window.
        let mut explicit: Vec<PathBuf> = Vec::new();
        if let Some(files) = &scope.exact_file_paths {
            explicit.extend(files.iter().cloned());
        } else if !scope.is_directory && scope.multi_targets.is_none() {
            explicit.push(scope.search_path.clone());
        }
        let oversized: Vec<String> = explicit
            .iter()
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() > engine::MAX_FILE_BYTES))
            .map(|p| {
                let rel = paths::relative_path(&ctx.cwd, p);
                if rel.is_empty() { p.to_string_lossy().into_owned() } else { rel }
            })
            .collect();
        let limit_mb = engine::MAX_FILE_BYTES / (1024 * 1024);
        let oversized_note = (!oversized.is_empty()).then(|| {
            format!(
                "Searched only the first {limit_mb}MB of large files (matches past the {limit_mb}MB window are not shown; use `read` for the rest): {}",
                oversized.join(", ")
            )
        });
        let oversized_scan_note = (oversized_note.is_none() && result.skipped_oversized > 0).then(|| {
            format!("Skipped {} unreadable large file(s); target them directly with `read`", result.skipped_oversized)
        });
        let missing_note = (!scope.missing_paths.is_empty())
            .then(|| format!("Skipped missing paths: {}", scope.missing_paths.join(", ")));
        let warning: Vec<String> = [missing_note, oversized_note, oversized_scan_note].into_iter().flatten().collect();
        let warning = (!warning.is_empty()).then(|| warning.join("\n"));

        let mut base_details = json!({
            "scopePath": scope.scope_path,
            "searchPath": scope.search_path.to_string_lossy(),
            "cwd": ctx.cwd.to_string_lossy(),
        });
        if !scope.missing_paths.is_empty() {
            base_details["missingPaths"] = json!(scope.missing_paths);
        }
        if selected.is_empty() {
            let past_end = can_paginate && skip > 0 && total_files > 0 && skip_files >= total_files;
            let mut text = if past_end {
                format!("No more results ({total_label} files total; skip={skip} is past the end)")
            } else {
                "No matches found".to_string()
            };
            if let Some(w) = &warning {
                text.push('\n');
                text.push_str(w);
            }
            let mut details = base_details;
            details["matchCount"] = json!(0);
            details["fileCount"] = json!(0);
            details["files"] = json!([]);
            details["truncated"] = json!(false);
            return Ok(ToolOutput::text(text).with_details(details));
        }

        let mut file_list: Vec<String> = Vec::new();
        let mut by_file: HashMap<String, Vec<GrepMatch>> = HashMap::new();
        let mut lines_truncated = false;
        for m in selected {
            let rel = format_path(&m.path);
            if !by_file.contains_key(&rel) {
                file_list.push(rel.clone());
            }
            lines_truncated |= m.truncated;
            by_file.entry(rel).or_default().push(m);
        }
        let match_count: usize = by_file.values().map(Vec::len).sum();
        let render = |rel: &str| -> Vec<String> {
            let mut out = Vec::new();
            let mut last: Option<u64> = None;
            let mut push = |n: u64, line: &str, is_match: bool, out: &mut Vec<String>| {
                if last.is_some_and(|l| n > l + 1) {
                    out.push("...".into());
                }
                out.push(format_match_line(n, line, is_match));
                last = Some(n);
            };
            for m in by_file.get(rel).map(Vec::as_slice).unwrap_or_default() {
                for c in &m.context_before {
                    push(c.line_number, &c.line, false, &mut out);
                }
                push(m.line_number, &m.line, true, &mut out);
                for c in &m.context_after {
                    push(c.line_number, &c.line, false, &mut out);
                }
            }
            out
        };
        let mut output: Vec<String> = Vec::new();
        if scope.is_directory || is_multi_scope {
            output = paths::format_grouped_files(&file_list, render);
        } else {
            for rel in &file_list {
                let body = render(rel);
                if body.is_empty() {
                    continue;
                }
                if !output.is_empty() {
                    output.push(String::new());
                }
                output.extend(body);
            }
        }
        if let Some(m) = &limit_message {
            output.push(String::new());
            output.push(m.clone());
        }
        if let Some(w) = &warning {
            output.push(String::new());
            output.push(w.clone());
        }
        let truncation = truncate_head(&output.join("\n"), usize::MAX, DEFAULT_MAX_BYTES);
        let mut notice = Notice::default();
        notice.truncation(&truncation);
        if lines_truncated {
            notice.column_max(MAX_COLUMN);
        }
        let truncated = file_limit_reached
            || per_file_limit_reached
            || result.limit_reached
            || truncation.truncated
            || lines_truncated;
        let mut details = base_details;
        details["matchCount"] = json!(match_count);
        details["fileCount"] = json!(file_list.len());
        details["files"] = json!(file_list);
        details["fileMatches"] =
            json!(file_list.iter().map(|f| json!({"path": f, "count": by_file[f].len()})).collect::<Vec<_>>());
        details["truncated"] = json!(truncated);
        if file_limit_reached {
            details["fileLimitReached"] = json!(DEFAULT_FILE_LIMIT);
        }
        if per_file_limit_reached {
            details["perFileLimitReached"] = json!(per_file_cap);
        }
        if lines_truncated {
            details["linesTruncated"] = json!(true);
        }
        if truncation.truncated {
            details["truncation"] = truncation.details();
        }
        Ok(ToolOutput::text(format!("{}{}", truncation.content, notice.render())).with_details(details))
    }
}

#[async_trait]
impl AgentTool for GrepTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "grep".into(),
            description: DESCRIPTION.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "regex pattern"},
                    "path": {"type": "string", "description": "file, directory, glob, or \"<file>:<lines>\" selector to search; pass several as a semicolon-delimited list (\"src; tests\"). Omitted -> searches the workspace root (\".\")"},
                    "case": {"type": "boolean", "description": "case-sensitive search"},
                    "gitignore": {"type": "boolean", "description": "respect gitignore"},
                    "skip": {"type": ["number", "null"], "description": "files to skip before collecting results — use to paginate when the prior call hit the file limit"}
                },
                "required": ["pattern"],
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
        let tool = GrepTool { ctx: self.ctx.clone(), timeout: self.timeout };
        crate::run_blocking("Grep", cancel, move |work| tool.execute_blocking(&args, &work)).await
    }
}
