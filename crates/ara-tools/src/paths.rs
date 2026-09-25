//! Path lists, line-range selectors and search scopes shared by `grep` and
//! `glob` (OMP `packages/coding-agent/src/tools/path-utils.ts`,
//! `tools/file-recorder.ts` `formatResultPath`, and
//! `packages/utils/src/path-tree.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! Not ported: internal URLs (`skill://`, `memory://`, `artifact://` …), archive
//! members, `file://` stripping, fuzzy URL spellings, and the macOS/NFD/curly
//! quote path variants of `resolveReadPath`. An unregistered `scheme://` entry
//! is an ordinary path here, as it is upstream when no protocol handles it.

use crate::ToolContext;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub fn has_glob_path_chars(s: &str) -> bool {
    s.contains(['*', '?', '[', '{'])
}

/// Trim and strip one pair of surrounding double quotes (`normalizePathLikeInput`).
pub fn normalize_path_like_input(input: &str) -> String {
    let t = input.trim();
    if t.len() > 1 && t.starts_with('"') && t.ends_with('"') { t[1..t.len() - 1].to_string() } else { t.to_string() }
}

/// A single string, or a JSON-encoded string array (`toPathList`).
pub fn to_path_list(input: Option<&str>) -> Vec<String> {
    let Some(input) = input else { return Vec::new() };
    let trimmed = input.trim();
    if trimmed.starts_with('[')
        && trimmed.ends_with(']')
        && let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(trimmed)
        && items.iter().all(|v| v.is_string())
    {
        return items.into_iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    }
    vec![input.to_string()]
}

/// `resolveToCwd`: a slash-only path means the working directory.
pub fn resolve_to_cwd(ctx: &ToolContext, p: &str) -> PathBuf {
    if !p.is_empty() && p.chars().all(|c| c == '/') { ctx.cwd.clone() } else { ctx.resolve(p) }
}

/// `formatPathRelativeToCwd`: cwd-relative inside cwd (`.` for cwd itself),
/// absolute outside; `/` separators; optional trailing slash.
pub fn format_path_relative_to_cwd(path: &Path, cwd: &Path, trailing_slash: bool) -> String {
    let path = crate::normalize(path);
    let mut display = match path.strip_prefix(cwd) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".to_string(),
        Ok(rel) => rel.to_string_lossy().into_owned(),
        Err(_) => path.to_string_lossy().into_owned(),
    }
    .replace('\\', "/");
    if trailing_slash && display != "." && !display.ends_with('/') {
        display.push('/');
    }
    display
}

/// Node `path.relative(from, to)` for normalized absolute paths (`..` steps
/// outside `from`; empty when equal).
pub fn relative_path(from: &Path, to: &Path) -> String {
    let (from, to) = (crate::normalize(from), crate::normalize(to));
    let a: Vec<_> = from.components().collect();
    let b: Vec<_> = to.components().collect();
    let shared = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); a.len() - shared];
    parts.extend(b[shared..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

/// `formatResultPath`: a match path under a directory scope, or the scope file itself.
pub fn format_result_path(match_path: &str, is_directory: bool, base: &Path, cwd: &Path) -> String {
    if is_directory {
        let clean = match_path.strip_prefix('/').unwrap_or(match_path);
        format_path_relative_to_cwd(&base.join(clean), cwd, false)
    } else {
        format_path_relative_to_cwd(base, cwd, false)
    }
}

// ---------------------------------------------------------------------------
// Line ranges
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineRange {
    pub start: u64,
    /// Inclusive; `None` runs to end of file.
    pub end: Option<u64>,
}

static LINE_RANGE_CHUNK_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)^L?([0-9]+)(?:(\.\.|[-+])L?([0-9]+)?)?$").unwrap());
const RANGE_CHUNK_SRC: &str = r"L?[0-9]+(?:(?:[-+]|\.\.)L?[0-9]+|-|\.\.)?";
static FILE_LINE_RANGE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(&format!(r"(?i)^(?:{RANGE_CHUNK_SRC}(?:,{RANGE_CHUNK_SRC})*|-[0-9]+|raw|conflicts|img)$"))
        .unwrap()
});
static FILE_LINE_RANGE_ONLY_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(&format!(r"(?i)^(?:{RANGE_CHUNK_SRC}(?:,{RANGE_CHUNK_SRC})*|-[0-9]+)$")).unwrap()
});

/// One `N`, `N-M`, `N-`, `N+K`, `N..M` chunk (`parseLineRangeChunk`).
/// `Ok(None)` when the chunk is not range-shaped; `Err` for invalid bounds.
pub fn parse_line_range_chunk(sel: &str) -> Result<Option<LineRange>, String> {
    let Some(caps) = LINE_RANGE_CHUNK_RE.captures(sel) else { return Ok(None) };
    // Digit runs beyond u64 saturate; such a line never exists.
    let num = |s: &str| s.parse::<u64>().unwrap_or(u64::MAX);
    let start = num(&caps[1]);
    if start < 1 {
        return Err("Line selector 0 is invalid; lines are 1-indexed. Use :1.".into());
    }
    let sep = caps.get(2).map(|m| if m.as_str() == ".." { "-" } else { m.as_str() });
    let rhs = caps.get(3).map(|m| num(m.as_str()));
    let end = match sep {
        Some("+") => match rhs {
            Some(k) if k >= 1 => Some(start.saturating_add(k - 1)),
            _ => return Err(format!("Invalid range {start}+{}: count must be >= 1.", rhs.unwrap_or(0))),
        },
        Some("-") => match rhs {
            Some(e) if e < start => return Err(format!("Invalid range {start}-{e}: end must be >= start.")),
            other => other,
        },
        _ => None,
    };
    Ok(Some(LineRange { start, end }))
}

/// Comma-separated ranges, sorted and merged (`parseLineRanges`).
pub fn parse_line_ranges(sel: &str) -> Result<Option<Vec<LineRange>>, String> {
    let mut parsed = Vec::new();
    for chunk in sel.split(',') {
        match parse_line_range_chunk(chunk)? {
            Some(r) => parsed.push(r),
            None => return Ok(None),
        }
    }
    parsed.sort_by_key(|r| r.start);
    let mut merged: Vec<LineRange> = Vec::new();
    for current in parsed {
        let Some(last) = merged.last_mut() else {
            merged.push(current);
            continue;
        };
        let Some(last_end) = last.end else { continue };
        if current.start <= last_end.saturating_add(1) {
            if current.end.is_none_or(|e| e > last_end) {
                last.end = current.end;
            }
            continue;
        }
        merged.push(current);
    }
    Ok(Some(merged))
}

pub fn is_line_in_ranges(line: u64, ranges: &[LineRange]) -> bool {
    ranges.iter().any(|r| line >= r.start && r.end.is_none_or(|e| line <= e))
}

/// Peel a trailing `:sel` (or `:range:raw` / `:raw:range`) selector (`splitPathAndSel`).
pub fn split_path_and_sel(raw: &str) -> (String, Option<String>) {
    let Some(colon) = raw.rfind(':').filter(|&c| c > 0) else { return (raw.to_string(), None) };
    let candidate = &raw[colon + 1..];
    if !FILE_LINE_RANGE_RE.is_match(candidate) {
        return (raw.to_string(), None);
    }
    let mut base = &raw[..colon];
    let mut sel = candidate.to_string();
    if let Some(inner) = base.rfind(':').filter(|&c| c > 0) {
        let inner_candidate = &base[inner + 1..];
        let is_raw = |s: &str| s.eq_ignore_ascii_case("raw");
        let inner_range = FILE_LINE_RANGE_ONLY_RE.is_match(inner_candidate);
        let outer_range = FILE_LINE_RANGE_ONLY_RE.is_match(candidate);
        if (is_raw(inner_candidate) && outer_range) || (inner_range && is_raw(candidate)) {
            sel = format!("{inner_candidate}:{candidate}");
            base = &base[..inner];
        }
    }
    (base.to_string(), Some(sel))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Exists,
    Missing,
    Unknown,
}

/// lstat probe: dangling symlinks and unreadable entries still count as present
/// (`probeLiteralPathExists`).
pub fn probe_literal(ctx: &ToolContext, p: &str) -> Probe {
    match std::fs::symlink_metadata(ctx.resolve(p)) {
        Ok(_) => Probe::Exists,
        Err(e) if is_missing(&e) => Probe::Missing,
        Err(_) => Probe::Unknown,
    }
}

/// ENOENT, ENOTDIR and ENAMETOOLONG can never name an existing entry.
pub fn is_missing(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
        || e.raw_os_error() == Some(libc::ENAMETOOLONG)
        || e.raw_os_error() == Some(libc::ENOTDIR)
}

/// `splitPathAndSelPreferringLiteral`: an existing (or unprobeable) literal path keeps its colon.
pub fn split_path_and_sel_preferring_literal(ctx: &ToolContext, raw: &str) -> (String, Option<String>) {
    let strict = split_path_and_sel(raw);
    if strict.1.is_none() || probe_literal(ctx, raw) != Probe::Missing {
        return (raw.to_string(), None);
    }
    strict
}

// ---------------------------------------------------------------------------
// Search and find patterns
// ---------------------------------------------------------------------------

/// `parseSearchPath`: literal base directory plus the glob tail, if any.
pub fn parse_search_path(p: &str) -> (String, Option<String>) {
    let normalized = p.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').collect();
    match segments.iter().position(|s| has_glob_path_chars(s)) {
        None => (normalized, None),
        Some(0) => (".".into(), Some(normalized)),
        Some(i) => (segments[..i].join("/"), Some(segments[i..].join("/"))),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FindPattern {
    pub base: String,
    pub glob: String,
    pub has_glob: bool,
}

/// `parseFindPattern`: a leading glob gains `**/`; a plain path means `**/*`.
pub fn parse_find_pattern(p: &str) -> FindPattern {
    let normalized = p.replace('\\', "/");
    let segments: Vec<&str> = normalized.split('/').collect();
    match segments.iter().position(|s| has_glob_path_chars(s)) {
        None => FindPattern { base: normalized, glob: "**/*".into(), has_glob: false },
        Some(0) => FindPattern {
            base: ".".into(),
            glob: if normalized.starts_with("**/") { normalized.clone() } else { format!("**/{normalized}") },
            has_glob: true,
        },
        Some(i) => FindPattern { base: segments[..i].join("/"), glob: segments[i..].join("/"), has_glob: true },
    }
}

#[derive(Clone, Copy)]
pub enum Splitter {
    Search,
    Find,
}

impl Splitter {
    fn base(self, p: &str) -> String {
        match self {
            Splitter::Search => parse_search_path(p).0,
            Splitter::Find => parse_find_pattern(p).base,
        }
    }
}

fn stat_exists(ctx: &ToolContext, base: &str) -> Result<bool, String> {
    match std::fs::metadata(resolve_to_cwd(ctx, base)) {
        Ok(_) => Ok(true),
        Err(e) if is_missing(&e) => Ok(false),
        Err(e) => Err(format!("Cannot access {base}: {e}")),
    }
}

fn delimited_part_resolves(ctx: &ToolContext, entry: &str, splitter: Splitter) -> Result<bool, String> {
    let peeled = split_path_and_sel(entry).0;
    stat_exists(ctx, &splitter.base(&peeled))
}

#[derive(Clone, Copy, PartialEq)]
enum SplitMode {
    Comma,
    Semicolon,
    Whitespace,
    Mixed,
}

fn is_separator(ch: char, mode: SplitMode) -> bool {
    match mode {
        SplitMode::Comma => ch == ',',
        SplitMode::Semicolon => ch == ';',
        SplitMode::Whitespace => ch.is_whitespace(),
        SplitMode::Mixed => ch == ',' || ch == ';' || ch.is_whitespace(),
    }
}

/// Split outside `{…}` groups, skipping `\`-escaped characters.
fn split_top_level(entry: &str, sep: impl Fn(char) -> bool) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut chars = entry.char_indices();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '\\' => {
                chars.next();
            }
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            c if depth == 0 && sep(c) => {
                parts.push(&entry[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&entry[start..]);
    parts
}

#[derive(Clone, Copy, PartialEq)]
enum Requirement {
    All,
    Some,
    None,
}

fn try_split(
    ctx: &ToolContext,
    entry: &str,
    splitter: Splitter,
    mode: SplitMode,
    req: Requirement,
) -> Result<Option<Vec<String>>, String> {
    let raw_parts = split_top_level(entry, |c| is_separator(c, mode));
    if raw_parts.len() < 2 {
        return Ok(None);
    }
    let parts: Vec<String> = raw_parts.iter().map(|p| normalize_path_like_input(p)).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Ok(None);
    }
    if req != Requirement::None {
        let mut resolved = Vec::with_capacity(parts.len());
        for part in &parts {
            resolved.push(delimited_part_resolves(ctx, part, splitter)?);
        }
        let ok = if req == Requirement::All { resolved.iter().all(|b| *b) } else { resolved.iter().any(|b| *b) };
        if !ok {
            return Ok(None);
        }
    }
    Ok(Some(parts))
}

/// Split one entry that flattened several targets (`splitDelimitedPathEntry`).
/// Semicolons always split; commas need one existing part; whitespace needs all.
/// An existing literal path is never split.
pub fn split_delimited_entry(
    ctx: &ToolContext,
    entry: &str,
    splitter: Splitter,
) -> Result<Option<Vec<String>>, String> {
    let entry = normalize_path_like_input(entry);
    if split_top_level(&entry, |c| is_separator(c, SplitMode::Mixed)).len() < 2 {
        return Ok(None);
    }
    if probe_literal(ctx, &entry) != Probe::Missing {
        return Ok(None);
    }
    let peeled = split_path_and_sel(&entry).0;
    if !has_glob_path_chars(&peeled) && delimited_part_resolves(ctx, &entry, splitter)? {
        return Ok(None);
    }
    for (mode, req) in [
        (SplitMode::Semicolon, Requirement::None),
        (SplitMode::Comma, Requirement::Some),
        (SplitMode::Whitespace, Requirement::All),
        (SplitMode::Mixed, Requirement::All),
    ] {
        if let Some(parts) = try_split(ctx, &entry, splitter, mode, req)? {
            return Ok(Some(parts));
        }
    }
    Ok(None)
}

pub fn expand_delimited_entries(
    ctx: &ToolContext,
    entries: &[String],
    splitter: Splitter,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for entry in entries {
        let normalized = normalize_path_like_input(entry);
        match split_delimited_entry(ctx, &normalized, splitter)? {
            Some(parts) => out.extend(parts),
            None => out.push(normalized),
        }
    }
    Ok(out)
}

/// Existing vs missing entries by base path (`partitionExistingPaths`).
pub fn partition_existing(
    ctx: &ToolContext,
    items: &[String],
    splitter: Splitter,
) -> Result<(Vec<String>, Vec<String>), String> {
    let (mut valid, mut missing) = (Vec::new(), Vec::new());
    for item in items {
        if stat_exists(ctx, &splitter.base(item))? { valid.push(item.clone()) } else { missing.push(item.clone()) }
    }
    Ok((valid, missing))
}

// ---------------------------------------------------------------------------
// Grep search scope (`resolveToolSearchScope` with surfaceExactFilePaths and
// fanOutFileTargets, as grep calls it)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct SearchTarget {
    pub base: PathBuf,
    pub glob: Option<String>,
}

#[derive(Clone, Debug)]
pub struct SearchScope {
    pub search_path: PathBuf,
    pub scope_path: String,
    pub glob: Option<String>,
    pub is_directory: bool,
    pub multi_targets: Option<Vec<SearchTarget>>,
    pub exact_file_paths: Option<Vec<PathBuf>>,
    pub missing_paths: Vec<String>,
}

/// `parseSearchPathPreferringLiteral`: `apps/[id]/page.tsx` stays literal when it exists.
fn parse_search_path_preferring_literal(ctx: &ToolContext, p: &str) -> (String, Option<String>) {
    if has_glob_path_chars(p) && std::fs::metadata(resolve_to_cwd(ctx, p)).is_ok() {
        return (p.replace('\\', "/"), None);
    }
    parse_search_path(p)
}

fn common_base(paths: &[PathBuf]) -> PathBuf {
    let mut common: Vec<_> = paths[0].components().collect();
    for p in &paths[1..] {
        let shared = common.iter().zip(p.components()).take_while(|(a, b)| **a == *b).count();
        common.truncate(shared);
    }
    if common.is_empty() { PathBuf::from("/") } else { common.iter().collect() }
}

fn rel_posix(from: &Path, to: &Path) -> String {
    let rel = to.strip_prefix(from).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
    if rel.is_empty() { ".".into() } else { rel }
}

fn join_relative_glob(base: &str, glob: &str) -> String {
    let glob = glob.replace('\\', "/");
    let glob = glob.trim_start_matches('/');
    if base.is_empty() || base == "." { glob.to_string() } else { format!("{}/{glob}", base.trim_end_matches('/')) }
}

fn brace_union(patterns: &[String]) -> Option<String> {
    let mut unique: Vec<String> = Vec::new();
    for p in patterns {
        let p = p.replace('\\', "/").trim().to_string();
        if !p.is_empty() && !unique.contains(&p) {
            unique.push(p);
        }
    }
    match unique.len() {
        0 => None,
        1 => unique.pop(),
        _ => Some(format!("{{{}}}", unique.join(","))),
    }
}

fn scope_display(ctx: &ToolContext, items: &[String]) -> String {
    items
        .iter()
        .map(|item| {
            let slash = item.ends_with('/') || item.ends_with('\\');
            format_path_relative_to_cwd(&resolve_to_cwd(ctx, item), &ctx.cwd, slash)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

static EXTERNAL_URL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)^(?:https?|ftp|ws|wss)://").unwrap());

pub fn resolve_search_scope(ctx: &ToolContext, inputs: &[String]) -> Result<SearchScope, String> {
    const EMPTY: &str = "Search scope entries must be non-empty paths or globs";
    let normalized: Vec<String> = inputs.iter().map(|p| normalize_path_like_input(p)).collect();
    if normalized.iter().any(String::is_empty) {
        return Err(EMPTY.into());
    }
    let raw_paths = expand_delimited_entries(ctx, &normalized, Splitter::Search)?;
    if raw_paths.iter().any(String::is_empty) {
        return Err(EMPTY.into());
    }
    if let Some(url) = raw_paths.iter().find(|p| EXTERNAL_URL_RE.is_match(p)) {
        return Err(format!(
            "Cannot search external URL: {url}. Use `read` to fetch web content, then search the returned text."
        ));
    }
    let (effective, missing_paths) = if raw_paths.len() > 1 {
        let (valid, missing) = partition_existing(ctx, &raw_paths, Splitter::Search)?;
        if valid.is_empty() {
            return Err(format!("Path not found: {}", missing.join(", ")));
        }
        (valid, missing)
    } else {
        (raw_paths.clone(), Vec::new())
    };

    let (search_path, scope_path, glob, multi_targets, exact_file_paths);
    if effective.len() == 1 {
        let (base, g) = parse_search_path_preferring_literal(ctx, &effective[0]);
        search_path = resolve_to_cwd(ctx, &base);
        glob = g;
        scope_path = format_path_relative_to_cwd(&search_path, &ctx.cwd, false);
        multi_targets = None;
        exact_file_paths = None;
    } else {
        let mut unique: Vec<String> = Vec::new();
        for p in &effective {
            if !unique.contains(p) {
                unique.push(p.clone());
            }
        }
        struct Item {
            glob: Option<String>,
            abs: PathBuf,
            is_file: bool,
            is_dir: bool,
        }
        let mut items = Vec::new();
        for raw in &unique {
            let (base, g) = parse_search_path_preferring_literal(ctx, raw);
            let abs = resolve_to_cwd(ctx, &base);
            let meta = std::fs::metadata(&abs).map_err(|e| format!("Path not found: {raw} ({e})"))?;
            items.push(Item { glob: g, abs, is_file: meta.is_file(), is_dir: meta.is_dir() });
        }
        let all_exact_files = items.iter().all(|i| i.glob.is_none() && i.is_file);
        let base = common_base(&items.iter().map(|i| i.abs.clone()).collect::<Vec<_>>());
        let combined: Vec<String> = items
            .iter()
            .map(|i| {
                let rel = rel_posix(&base, &i.abs);
                if let Some(g) = &i.glob {
                    join_relative_glob(&rel, g)
                } else if i.is_dir {
                    join_relative_glob(&rel, "**/*")
                } else if rel == "." {
                    i.abs.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                } else {
                    rel
                }
            })
            .collect();
        let common_is_requested = items.iter().any(|i| i.abs == base);
        let demotes_file = !all_exact_files && items.iter().any(|i| i.glob.is_none() && i.is_file);
        let targets = (items.len() > 1 && (!common_is_requested || demotes_file))
            .then(|| items.iter().map(|i| SearchTarget { base: i.abs.clone(), glob: i.glob.clone() }).collect());
        exact_file_paths = all_exact_files.then(|| items.iter().map(|i| i.abs.clone()).collect::<Vec<_>>());
        glob = if exact_file_paths.is_some() || targets.is_some() { None } else { brace_union(&combined) };
        multi_targets = targets;
        search_path = base;
        scope_path = scope_display(ctx, &unique);
    }
    let is_directory = match std::fs::metadata(&search_path) {
        Ok(m) => m.is_dir(),
        Err(_) => {
            let hint = if raw_paths.len() > 1 { " (`path` list entries must each exist relative to cwd)" } else { "" };
            return Err(format!("Path not found: {scope_path}{hint}"));
        }
    };
    Ok(SearchScope { search_path, scope_path, glob, is_directory, multi_targets, exact_file_paths, missing_paths })
}

// ---------------------------------------------------------------------------
// Grouped path trees (`buildPathTree`, `walkPathTree`, `formatGroupedPaths`)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Node {
    files: Vec<(String, String)>,
    subdirs: Vec<(String, Node)>,
}

impl Node {
    fn child(&mut self, name: &str) -> &mut Node {
        let idx = match self.subdirs.iter().position(|(n, _)| n == name) {
            Some(i) => i,
            None => {
                self.subdirs.push((name.to_string(), Node::default()));
                self.subdirs.len() - 1
            }
        };
        &mut self.subdirs[idx].1
    }
}

static URL_LIKE_RE: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://").unwrap());

fn build_tree(entries: &[(String, bool, String)]) -> Node {
    let mut root = Node::default();
    for (raw, is_dir, key) in entries {
        let normalized = raw.replace('\\', "/");
        if URL_LIKE_RE.is_match(&normalized) {
            if !root.files.iter().any(|(n, _)| *n == normalized) {
                root.files.push((normalized, key.clone()));
            }
            continue;
        }
        let trimmed = normalized.strip_suffix('/').unwrap_or(&normalized);
        if trimmed.is_empty() {
            continue;
        }
        let segments: Vec<&str> = trimmed.split('/').collect();
        let dir_count = if *is_dir { segments.len() } else { segments.len() - 1 };
        let mut node = &mut root;
        for seg in &segments[..dir_count] {
            node = node.child(seg);
        }
        if !is_dir {
            let name = segments[segments.len() - 1].to_string();
            if !node.files.iter().any(|(n, _)| *n == name) {
                node.files.push((name, key.clone()));
            }
        }
    }
    root
}

/// Tree walk events: `(is_dir, depth, name, key)`.
fn walk_tree(node: &Node, depth: usize, out: &mut Vec<(bool, usize, String, String)>) {
    for (name, key) in &node.files {
        out.push((false, depth, name.clone(), key.clone()));
    }
    for (name, child) in &node.subdirs {
        let mut parts = vec![name.clone()];
        let mut dir = child;
        while dir.files.is_empty() && dir.subdirs.len() == 1 {
            parts.push(dir.subdirs[0].0.clone());
            dir = &dir.subdirs[0].1;
        }
        out.push((true, depth, parts.join("/"), String::new()));
        walk_tree(dir, depth + 1, out);
    }
}

/// Find-style listing: `# dir/` headers (one `#` per level), bare file names.
pub fn format_grouped_paths(paths: &[String]) -> String {
    if paths.is_empty() {
        return String::new();
    }
    let entries: Vec<(String, bool, String)> = paths.iter().map(|p| (p.clone(), p.ends_with('/'), p.clone())).collect();
    let mut events = Vec::new();
    walk_tree(&build_tree(&entries), 0, &mut events);
    events
        .into_iter()
        .map(|(is_dir, depth, name, _)| if is_dir { format!("{} {name}/", "#".repeat(depth + 1)) } else { name })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Grep-style grouped output (`formatGroupedFiles`): a header per file with its
/// body; a blank line before each directory header and each root-level file.
/// `body` returns the file's lines and a header suffix (a hashline `#TAG`).
/// Files whose body is empty are omitted.
pub fn format_grouped_files(files: &[String], mut body: impl FnMut(&str) -> (Vec<String>, String)) -> Vec<String> {
    let mut sections: Vec<(String, Vec<String>, String)> = Vec::new();
    for f in files {
        if sections.iter().any(|(k, _, _)| k == f) {
            continue;
        }
        let (lines, suffix) = body(f);
        if !lines.is_empty() {
            sections.push((f.clone(), lines, suffix));
        }
    }
    let entries: Vec<(String, bool, String)> = sections.iter().map(|(k, _, _)| (k.clone(), false, k.clone())).collect();
    let mut events = Vec::new();
    walk_tree(&build_tree(&entries), 0, &mut events);
    let mut out = Vec::new();
    let mut emitted = false;
    for (is_dir, depth, name, key) in events {
        let hashes = "#".repeat(depth + 1);
        if emitted && (depth == 0 || is_dir) {
            out.push(String::new());
        }
        emitted = true;
        if is_dir {
            out.push(format!("{hashes} {name}/"));
            continue;
        }
        let section = sections.iter().find(|(k, _, _)| *k == key);
        out.push(format!("{hashes} {name}{}", section.map_or("", |(_, _, suffix)| suffix.as_str())));
        if let Some((_, lines, _)) = section {
            out.extend(lines.iter().cloned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_ranges_parse_merge_and_reject() {
        let r = |s| parse_line_ranges(s).unwrap().unwrap();
        assert_eq!(r("5-9"), vec![LineRange { start: 5, end: Some(9) }]);
        assert_eq!(r("L5..L9"), vec![LineRange { start: 5, end: Some(9) }]);
        assert_eq!(r("5+3"), vec![LineRange { start: 5, end: Some(7) }]);
        assert_eq!(r("7-"), vec![LineRange { start: 7, end: None }]);
        assert_eq!(
            r("20-30,1-5,4-8"),
            vec![LineRange { start: 1, end: Some(8) }, LineRange { start: 20, end: Some(30) }]
        );
        assert_eq!(r("3,1-"), vec![LineRange { start: 1, end: None }]);
        assert_eq!(parse_line_ranges("raw").unwrap(), None);
        assert_eq!(parse_line_ranges("-5").unwrap(), None);
        assert_eq!(parse_line_ranges("0").unwrap_err(), "Line selector 0 is invalid; lines are 1-indexed. Use :1.");
        assert_eq!(parse_line_ranges("9-3").unwrap_err(), "Invalid range 9-3: end must be >= start.");
        assert_eq!(parse_line_ranges("4+0").unwrap_err(), "Invalid range 4+0: count must be >= 1.");
        assert!(is_line_in_ranges(8, &r("1-3,8")) && !is_line_in_ranges(5, &r("1-3,8-9")));
    }

    #[test]
    fn selector_split() {
        assert_eq!(split_path_and_sel("src/a.ts:50-100"), ("src/a.ts".into(), Some("50-100".into())));
        assert_eq!(split_path_and_sel("a.ts:1-2:raw"), ("a.ts".into(), Some("1-2:raw".into())));
        assert_eq!(split_path_and_sel("a.ts:raw:-60"), ("a.ts".into(), Some("raw:-60".into())));
        assert_eq!(split_path_and_sel("C:thing"), ("C:thing".into(), None));
        assert_eq!(split_path_and_sel(":5"), (":5".into(), None));
    }

    #[test]
    fn find_and_search_patterns() {
        assert_eq!(parse_find_pattern("src/app/**/*.tsx").base, "src/app");
        assert_eq!(parse_find_pattern("*.ts").glob, "**/*.ts");
        assert_eq!(parse_find_pattern("**/*.json").glob, "**/*.json");
        assert_eq!(
            parse_find_pattern("src/app"),
            FindPattern { base: "src/app".into(), glob: "**/*".into(), has_glob: false }
        );
        assert_eq!(parse_search_path("src/*.rs"), ("src".into(), Some("*.rs".into())));
        assert_eq!(parse_search_path("*.rs"), (".".into(), Some("*.rs".into())));
        assert_eq!(parse_search_path("src/lib.rs"), ("src/lib.rs".into(), None));
    }

    #[test]
    fn top_level_split_respects_braces_and_escapes() {
        assert_eq!(split_top_level("a;{b;c};d", |c| c == ';'), vec!["a", "{b;c}", "d"]);
        assert_eq!(split_top_level(r"a\;b;c", |c| c == ';'), vec![r"a\;b", "c"]);
        assert_eq!(to_path_list(Some(r#"["a", "b"]"#)), vec!["a", "b"]);
        assert_eq!(to_path_list(Some("[x]")), vec!["[x]"]);
        assert_eq!(normalize_path_like_input(" \"a b\" "), "a b");
    }

    #[test]
    fn grouped_paths_fold_single_child_chains() {
        let paths: Vec<String> = ["packages/pkg/src/a.ts", "packages/pkg/src/nested/b.ts", "top.md", "dir/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(format_grouped_paths(&paths), "top.md\n# packages/pkg/src/\na.ts\n## nested/\nb.ts\n# dir/");
        let files: Vec<String> = ["src/a.rs", "src/b/c.rs", "root.txt"].iter().map(|s| s.to_string()).collect();
        let out = format_grouped_files(&files, |f| (vec![format!("*1|{f}")], String::new()));
        assert_eq!(
            out.join("\n"),
            "# root.txt\n*1|root.txt\n\n# src/\n## a.rs\n*1|src/a.rs\n\n## b/\n### c.rs\n*1|src/b/c.rs"
        );
        // Upstream B-499e4bca94: skipped files and their now-empty directories vanish.
        let skipped = format_grouped_files(&files, |f| {
            (if f.starts_with("src/b") { vec![] } else { vec![f.to_string()] }, String::new())
        });
        assert_eq!(skipped.join("\n"), "# root.txt\nroot.txt\n\n# src/\n## a.rs\nsrc/a.rs");
        let abs: Vec<String> = ["/tmp/x/a.rs", "/tmp/x/b.rs"].iter().map(|s| s.to_string()).collect();
        assert_eq!(format_grouped_paths(&abs), "# /tmp/x/\na.rs\nb.rs");
    }

    /// Upstream `glob-validate-paths.test.ts` "delimited path expansion"
    /// (B-f25c0e5ce8, B-df90f9b8b2, B-892f3e95bf, B-8f1d4c9081, B-3259ad16cf,
    /// B-082c5721e3, B-60ed7dd2a0, B-4417c7d548, B-b843a033ee).
    #[test]
    fn delimited_path_expansion() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        for d in ["apps", "packages", "src", "folder with spaces"] {
            std::fs::create_dir_all(r.join(d)).unwrap();
        }
        std::fs::write(r.join("apps/a.txt"), "apps\n").unwrap();
        std::fs::write(r.join("packages/b.txt"), "packages\n").unwrap();
        std::fs::write(r.join("folder with spaces/file.txt"), "spaces\n").unwrap();
        let ctx = ToolContext::new(r);
        let split = |e: &str| split_delimited_entry(&ctx, e, Splitter::Search).unwrap();
        let v = |xs: &[&str]| Some(xs.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let both = v(&["apps/a.txt", "packages/b.txt"]);
        assert_eq!(split("apps/a.txt, packages/b.txt"), both);
        assert_eq!(split("apps/a.txt;packages/b.txt"), both);
        assert_eq!(split("apps/a.txt packages/b.txt"), both);
        assert_eq!(split("folder with spaces/file.txt"), None);
        assert_eq!(split("src/{a,b}.txt"), None);
        assert_eq!(split("src/{a,b}.txt, packages/b.txt"), v(&["src/{a,b}.txt", "packages/b.txt"]));
        assert_eq!(split("apps/a.txt\\,packages/b.txt"), None);
        assert_eq!(split("apps/a.txt\\;packages/b.txt"), None);
        assert_eq!(split("folder\\ with\\ spaces/file.txt packages/b.txt"), None);
        assert_eq!(split("missing.txt, packages/b.txt"), v(&["missing.txt", "packages/b.txt"]));
        assert_eq!(split("missing.txt;packages/b.txt"), v(&["missing.txt", "packages/b.txt"]));
        assert_eq!(split("missing.txt packages/b.txt"), None);
        let expand = |e: &str, s| expand_delimited_entries(&ctx, &[e.to_string()], s).unwrap();
        assert_eq!(expand("apps/a.txt,", Splitter::Search), ["apps/a.txt"]);
        assert_eq!(expand("apps/**/*.txt, packages/**/*.txt", Splitter::Find), ["apps/**/*.txt", "packages/**/*.txt"]);
        let names: Vec<String> = (0..20).map(|i| format!("enametoolong-probe-{i:02}.txt")).collect();
        for n in &names {
            std::fs::write(r.join(n), "needle\n").unwrap();
        }
        let joined = names.join("; ");
        assert!(joined.len() > 255);
        assert_eq!(split(&joined), Some(names.clone()));
        assert_eq!(
            parse_find_pattern("apps\\**\\*.txt"),
            FindPattern { base: "apps".into(), glob: "**/*.txt".into(), has_glob: true }
        );
        assert_eq!(parse_search_path("apps\\*.txt"), ("apps".into(), Some("*.txt".into())));
    }
}
