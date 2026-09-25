//! Filesystem search engine behind `grep` and `glob` (OMP
//! `crates/pi-natives/src/grep.rs`, `glob.rs` and `glob_util.rs` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d; traversal in `ara-walk`).
//!
//! Matching uses the same ripgrep libraries as upstream: `grep-regex` first,
//! then PCRE2 (lookaround, backreferences), then a retry with stray
//! parentheses escaped, then a literal search. Directory searches stream the
//! walk in windows of 512 files and stop once the match budget is met; files
//! over 4 MB are searched over their first 4 MB after the walk.
//!
//! Differences: each window is searched sequentially (upstream searches a
//! window in parallel, then sorts it by path, so results are the same). The
//! matcher wrapper forwards the Rust engine's line terminator and candidate
//! search so single-line patterns use `grep-searcher`'s whole-buffer fast path
//! (same results; upstream searches line by line). The PCRE2 JIT switch is
//! `ARA_PCRE2_JIT` (upstream `OMP_PCRE2_JIT`).

use ara_walk::{self as walk, EntryKind, Visit, WalkEntry, WalkOptions};
use globset::{GlobBuilder, GlobMatcher};
use grep_matcher::{LineMatchKind, LineTerminator, Matcher};
use grep_pcre2::{RegexMatcher as PcreMatcher, RegexMatcherBuilder as PcreMatcherBuilder};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkContext, SinkContextKind, SinkMatch};
use std::borrow::Cow;
use std::fmt;
use std::io::{self, Read};
use std::path::Path;

/// PCRE2 JIT: `ARA_PCRE2_JIT=1` forces it on, `0`/`false` off. Unset, it is on
/// except on macOS, where PCRE2's JIT allocator can fault (upstream
/// `PCRE2_JIT_ENABLED`, read from `OMP_PCRE2_JIT` there).
static PCRE2_JIT_ENABLED: std::sync::LazyLock<bool> =
    std::sync::LazyLock::new(|| match std::env::var("ARA_PCRE2_JIT") {
        Ok(v) if !v.is_empty() => v != "0" && !v.eq_ignore_ascii_case("false"),
        _ => !cfg!(target_os = "macos"),
    });

/// Upstream `MAX_FILE_BYTES`: larger files are searched over this leading window.
pub const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub enum EngineError {
    /// Both regex engines rejected the pattern and the literal fallback failed.
    Regex(String),
    InvalidGlob(String),
    Timeout,
    Aborted,
    Io(String),
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EngineError::Regex(m) => write!(f, "Regex error: {m}"),
            EngineError::InvalidGlob(m) => write!(f, "Invalid glob pattern: {m}"),
            EngineError::Timeout => f.write_str("Aborted: Timeout"),
            EngineError::Aborted => f.write_str("Aborted"),
            EngineError::Io(m) => f.write_str(m),
        }
    }
}

pub use ara_walk::Budget;

impl From<ara_walk::WalkError> for EngineError {
    fn from(e: ara_walk::WalkError) -> Self {
        match e {
            ara_walk::WalkError::Timeout => EngineError::Timeout,
            ara_walk::WalkError::Aborted => EngineError::Aborted,
        }
    }
}

// ---------------------------------------------------------------------------
// Glob helpers (glob_util.rs)
// ---------------------------------------------------------------------------

fn fix_unclosed_braces(mut pattern: String) -> String {
    let opens = pattern.matches('{').count();
    let closes = pattern.matches('}').count();
    for _ in closes..opens {
        pattern.push('}');
    }
    pattern
}

fn is_exact_brace_union(pattern: &str) -> bool {
    pattern.len() >= 2
        && pattern.starts_with('{')
        && pattern.ends_with('}')
        && !pattern[1..pattern.len() - 1].is_empty()
        && !pattern[1..pattern.len() - 1].contains(['*', '?', '[', ']', '{', '}'])
}

/// Normalize separators, prefix `**/` for recursive simple patterns, close braces.
pub fn build_glob_pattern(glob: &str, recursive: bool) -> String {
    let normalized = glob.replace('\\', "/");
    let pattern = if !recursive
        || normalized.contains('/')
        || normalized.starts_with("**")
        || is_exact_brace_union(&normalized)
    {
        normalized
    } else {
        format!("**/{normalized}")
    };
    fix_unclosed_braces(pattern)
}

/// Deepest component count a non-recursive pattern can match.
pub fn walk_depth_bound(pattern: &str) -> Option<usize> {
    if pattern.contains("**") || pattern.contains('{') {
        return None;
    }
    Some(pattern.split('/').filter(|s| !s.is_empty()).count().max(1))
}

pub fn compile_glob(pattern: &str) -> Result<GlobMatcher, EngineError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| EngineError::InvalidGlob(e.to_string()))
}

// ---------------------------------------------------------------------------
// Glob (glob.rs, sortByMtime mode as the glob tool uses it)
// ---------------------------------------------------------------------------

pub struct GlobRequest<'a> {
    pub root: &'a Path,
    pub pattern: &'a str,
    pub recursive: bool,
    pub include_hidden: bool,
    pub use_gitignore: bool,
    pub max_results: usize,
}

/// A glob match with the lstat mtime used for ranking.
#[derive(Clone, Debug)]
pub struct GlobEntry {
    pub relative: String,
    pub kind: EntryKind,
    pub mtime_ms: f64,
}

/// Entries matching the pattern, newest first (ties by path), capped.
pub fn glob(req: &GlobRequest<'_>, budget: &Budget) -> Result<Vec<GlobEntry>, EngineError> {
    let pattern = req.pattern.trim();
    let pattern = if pattern.is_empty() { "*" } else { pattern };
    let walk_pattern = build_glob_pattern(pattern, req.recursive);
    let matcher = compile_glob(&walk_pattern)?;
    if req.max_results == 0 {
        return Ok(Vec::new());
    }
    let opts = WalkOptions {
        include_hidden: req.include_hidden,
        use_gitignore: req.use_gitignore,
        skip_node_modules: !pattern.contains("node_modules"),
        max_depth: walk_depth_bound(&walk_pattern).unwrap_or(usize::MAX),
    };
    let mut matches = Vec::new();
    walk::walk(req.root, opts, budget, |e| {
        if matcher.is_match(&e.relative) {
            // Only matched entries are stat'ed.
            let mtime_ms = std::fs::symlink_metadata(&e.path).map_or(0.0, |m| walk::mtime_ms(&m));
            matches.push(GlobEntry { relative: e.relative, kind: e.kind, mtime_ms });
        }
        Visit::Continue
    })?;
    matches.sort_by(|a, b| b.mtime_ms.total_cmp(&a.mtime_ms).then_with(|| a.relative.cmp(&b.relative)));
    matches.truncate(req.max_results);
    Ok(matches)
}

// ---------------------------------------------------------------------------
// Regex matcher construction
// ---------------------------------------------------------------------------

pub enum CompiledMatcher {
    Rust(RegexMatcher),
    Pcre(PcreMatcher),
}

#[derive(Debug)]
pub enum CompiledMatcherError {
    Rust(grep_matcher::NoError),
    Pcre(grep_pcre2::Error),
}

impl fmt::Display for CompiledMatcherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rust(e) => e.fmt(f),
            Self::Pcre(e) => e.fmt(f),
        }
    }
}

impl Matcher for CompiledMatcher {
    type Captures = grep_matcher::NoCaptures;
    type Error = CompiledMatcherError;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<grep_matcher::Match>, Self::Error> {
        match self {
            Self::Rust(m) => m.find_at(haystack, at).map_err(CompiledMatcherError::Rust),
            Self::Pcre(m) => m.find_at(haystack, at).map_err(CompiledMatcherError::Pcre),
        }
    }

    fn new_captures(&self) -> Result<Self::Captures, Self::Error> {
        Ok(grep_matcher::NoCaptures::new())
    }

    // `non_matching_bytes` is deliberately not forwarded: it would switch a
    // multi-line search of a newline-free regex to line-by-line matching.
    fn line_terminator(&self) -> Option<LineTerminator> {
        match self {
            Self::Rust(m) => m.line_terminator(),
            Self::Pcre(_) => None,
        }
    }

    fn find_candidate_line(&self, haystack: &[u8]) -> Result<Option<LineMatchKind>, Self::Error> {
        match self {
            Self::Rust(m) => m.find_candidate_line(haystack).map_err(CompiledMatcherError::Rust),
            Self::Pcre(m) => m.find_candidate_line(haystack).map_err(CompiledMatcherError::Pcre),
        }
    }

    fn shortest_match_at(&self, haystack: &[u8], at: usize) -> Result<Option<usize>, Self::Error> {
        match self {
            Self::Rust(m) => m.shortest_match_at(haystack, at).map_err(CompiledMatcherError::Rust),
            Self::Pcre(m) => m.shortest_match_at(haystack, at).map_err(CompiledMatcherError::Pcre),
        }
    }
}

/// `{N}`, `{N,}`, `{N,M}` starting at `bytes[start] == b'{'`; index of `}`.
fn find_valid_repetition(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if i >= bytes.len() || !bytes[i].is_ascii_digit() {
        return None;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    match bytes.get(i) {
        Some(b'}') => return Some(i),
        Some(b',') => {}
        _ => return None,
    }
    i += 1;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    (bytes.get(i) == Some(&b'}')).then_some(i)
}

/// Escape braces that cannot be repetition quantifiers (`${platform}`, `a{b}`).
pub fn sanitize_braces(pattern: &str) -> Cow<'_, str> {
    let bytes = pattern.as_bytes();
    if !bytes.contains(&b'{') && !bytes.contains(&b'}') {
        return Cow::Borrowed(pattern);
    }
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut modified = false;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            out.push('\\');
            i += 1;
            let ch = pattern[i..].chars().next().expect("in bounds");
            out.push(ch);
            i += ch.len_utf8();
            if matches!(ch, 'p' | 'P' | 'x' | 'u') && bytes.get(i) == Some(&b'{') {
                match bytes[i..].iter().position(|b| *b == b'}') {
                    Some(off) => {
                        out.push_str(&pattern[i..=i + off]);
                        i += off + 1;
                    }
                    None => {
                        out.push_str(&pattern[i..]);
                        i = bytes.len();
                    }
                }
            }
            continue;
        }
        if bytes[i] == b'{' {
            if let Some(end) = find_valid_repetition(bytes, i) {
                out.push_str(&pattern[i..=end]);
                i = end + 1;
            } else {
                out.push_str("\\{");
                i += 1;
                modified = true;
            }
            continue;
        }
        if bytes[i] == b'}' {
            out.push_str("\\}");
            i += 1;
            modified = true;
            continue;
        }
        let ch = pattern[i..].chars().next().expect("in bounds");
        out.push(ch);
        i += ch.len_utf8();
    }
    if modified { Cow::Owned(out) } else { Cow::Borrowed(pattern) }
}

fn escape_unescaped_parentheses(pattern: &str) -> Cow<'_, str> {
    if !pattern.contains(['(', ')']) {
        return Cow::Borrowed(pattern);
    }
    let mut out = String::with_capacity(pattern.len() + 4);
    let mut modified = false;
    let mut chars = pattern.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            out.push('\\');
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if matches!(ch, '(' | ')') {
            out.push('\\');
            modified = true;
        }
        out.push(ch);
    }
    if modified { Cow::Owned(out) } else { Cow::Borrowed(pattern) }
}

fn build_regex(pattern: &str, ignore_case: bool, multiline: bool) -> Result<RegexMatcher, grep_regex::Error> {
    let build = |line_terminated: bool| {
        let mut b = RegexMatcherBuilder::new();
        b.case_insensitive(ignore_case).multi_line(multiline);
        if line_terminated {
            b.line_terminator(Some(b'\n'));
        }
        b.build(pattern)
    };
    if !multiline && let Ok(m) = build(true) {
        return Ok(m);
    }
    build(false)
}

fn build_pcre(pattern: &str, ignore_case: bool, multiline: bool) -> Result<PcreMatcher, grep_pcre2::Error> {
    PcreMatcherBuilder::new()
        .caseless(ignore_case)
        .multi_line(multiline)
        .utf(true)
        .ucp(true)
        .jit_if_available(*PCRE2_JIT_ENABLED)
        .build(pattern)
}

pub fn build_matcher(pattern: &str, ignore_case: bool, multiline: bool) -> Result<CompiledMatcher, EngineError> {
    let sanitized = sanitize_braces(pattern);
    let err = match build_regex(&sanitized, ignore_case, multiline) {
        Ok(m) => return Ok(CompiledMatcher::Rust(m)),
        Err(e) => e,
    };
    if let Ok(m) = build_pcre(&sanitized, ignore_case, multiline) {
        return Ok(CompiledMatcher::Pcre(m));
    }
    let message = err.to_string();
    if message.contains("unclosed group") || message.contains("unopened group") {
        let escaped = escape_unescaped_parentheses(&sanitized);
        if escaped != sanitized {
            if let Ok(m) = build_regex(&escaped, ignore_case, multiline) {
                return Ok(CompiledMatcher::Rust(m));
            }
            if let Ok(m) = build_pcre(&escaped, ignore_case, multiline) {
                return Ok(CompiledMatcher::Pcre(m));
            }
        }
    }
    build_regex(&regex::escape(pattern), ignore_case, multiline)
        .map(CompiledMatcher::Rust)
        .map_err(|_| EngineError::Regex(message))
}

// ---------------------------------------------------------------------------
// Match collection
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ContextLine {
    pub line_number: u64,
    pub line: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GrepMatch {
    /// Relative to a directory scope; the absolute file path for a file scope.
    pub path: String,
    pub line_number: u64,
    pub line: String,
    pub context_before: Vec<ContextLine>,
    pub context_after: Vec<ContextLine>,
    pub truncated: bool,
}

struct Collected {
    line_number: u64,
    line: String,
    context_before: Vec<ContextLine>,
    context_after: Vec<ContextLine>,
    truncated: bool,
}

struct Collector {
    matches: Vec<Collected>,
    match_count: u64,
    collected: u64,
    max_count: Option<u64>,
    limit_reached: bool,
    max_columns: Option<usize>,
    before: Vec<ContextLine>,
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Byte-length column cap with a `...` marker (native `truncate_line`).
fn truncate_line(line: String, max_columns: Option<usize>) -> (String, bool) {
    match max_columns {
        Some(max) if line.len() > max => {
            let cut = floor_char_boundary(&line, max.saturating_sub(3));
            (format!("{}...", &line[..cut]), true)
        }
        _ => (line, false),
    }
}

fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

impl Sink for Collector {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
        self.match_count += 1;
        if self.limit_reached {
            return Ok(false);
        }
        let (line, truncated) = truncate_line(trimmed(mat.bytes()), self.max_columns);
        self.matches.push(Collected {
            line_number: mat.line_number().unwrap_or(0),
            line,
            context_before: std::mem::take(&mut self.before),
            context_after: Vec::new(),
            truncated,
        });
        self.collected += 1;
        if self.max_count.is_some_and(|max| self.collected >= max) {
            self.limit_reached = true;
        }
        Ok(true)
    }

    fn context(&mut self, _: &Searcher, ctx: &SinkContext<'_>) -> Result<bool, io::Error> {
        let (line, _) = truncate_line(trimmed(ctx.bytes()), self.max_columns);
        let c = ContextLine { line_number: ctx.line_number().unwrap_or(0), line };
        match ctx.kind() {
            SinkContextKind::Before => self.before.push(c),
            SinkContextKind::After => {
                if let Some(last) = self.matches.last_mut() {
                    last.context_after.push(c);
                }
            }
            SinkContextKind::Other => {}
        }
        Ok(true)
    }
}

#[derive(Clone, Copy)]
pub struct GrepParams {
    pub ignore_case: bool,
    pub multiline: bool,
    pub include_hidden: bool,
    pub use_gitignore: bool,
    pub max_count: Option<u64>,
    pub max_count_per_file: Option<u64>,
    pub context_before: usize,
    pub context_after: usize,
    pub max_columns: Option<usize>,
}

#[derive(Debug, Default)]
pub struct GrepOutcome {
    pub matches: Vec<GrepMatch>,
    pub total_matches: u64,
    pub files_searched: u64,
    pub limit_reached: bool,
    pub skipped_oversized: u64,
}

/// Upstream `GREP_STREAM_WINDOW`.
const STREAM_WINDOW: usize = 512;

struct FileSearch {
    matches: Vec<Collected>,
    match_count: u64,
    collected: u64,
    limit_reached: bool,
}

enum ReadOutcome {
    Read,
    Oversized,
    /// Missing, permission denied or not a regular file.
    Skipped,
    /// Any other I/O error; never counted as a skipped large file.
    Failed,
}

/// One searcher and read buffer reused across the files of a call.
struct Worker<'a> {
    matcher: &'a CompiledMatcher,
    params: &'a GrepParams,
    searcher: Searcher,
    buffer: Vec<u8>,
}

impl<'a> Worker<'a> {
    fn new(matcher: &'a CompiledMatcher, params: &'a GrepParams) -> Self {
        let searcher = SearcherBuilder::new()
            .binary_detection(BinaryDetection::quit(b'\x00'))
            .line_number(true)
            .multi_line(params.multiline)
            .before_context(params.context_before)
            .after_context(params.context_after)
            .build();
        Worker { matcher, params, searcher, buffer: Vec::new() }
    }

    /// Whole regular file when at most 4 MB, else only its first 4 MB when
    /// `prefix` is set; non-regular or unreadable files are skipped.
    fn read(&mut self, path: &Path, prefix: bool) -> ReadOutcome {
        self.buffer.clear();
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied) => {
                return ReadOutcome::Skipped;
            }
            Err(_) => return ReadOutcome::Failed,
        };
        let Ok(meta) = file.metadata() else { return ReadOutcome::Failed };
        if !meta.is_file() {
            return ReadOutcome::Skipped;
        }
        if meta.len() > MAX_FILE_BYTES && !prefix {
            return ReadOutcome::Oversized;
        }
        match file.take(MAX_FILE_BYTES + 1).read_to_end(&mut self.buffer) {
            Ok(_) if self.buffer.len() as u64 > MAX_FILE_BYTES => {
                if prefix {
                    self.buffer.truncate(MAX_FILE_BYTES as usize);
                    ReadOutcome::Read
                } else {
                    ReadOutcome::Oversized
                }
            }
            Ok(_) => ReadOutcome::Read,
            Err(_) => ReadOutcome::Failed,
        }
    }

    fn search(&mut self, max_count: Option<u64>) -> Result<FileSearch, io::Error> {
        let mut c = Collector {
            matches: Vec::new(),
            match_count: 0,
            collected: 0,
            max_count,
            limit_reached: false,
            max_columns: self.params.max_columns,
            before: Vec::new(),
        };
        self.searcher.search_slice(self.matcher, &self.buffer, &mut c)?;
        Ok(FileSearch {
            matches: c.matches,
            match_count: c.match_count,
            collected: c.collected,
            limit_reached: c.limit_reached,
        })
    }

    /// Directory-walk search: a searcher error counts as no matches (upstream).
    fn search_lenient(&mut self, max_count: Option<u64>) -> FileSearch {
        self.search(max_count).unwrap_or(FileSearch {
            matches: Vec::new(),
            match_count: 0,
            collected: 0,
            limit_reached: false,
        })
    }
}

fn to_match(path: String, c: Collected) -> GrepMatch {
    GrepMatch {
        path,
        line_number: c.line_number,
        line: c.line,
        context_before: c.context_before,
        context_after: c.context_after,
        truncated: c.truncated,
    }
}

/// Walk state for the windowed directory search.
struct Pass<'a> {
    worker: Worker<'a>,
    per_file: Option<u64>,
    results: Vec<(String, FileSearch)>,
    deferred: Vec<WalkEntry>,
    emitted: u64,
    files_searched: u64,
    skipped_oversized: u64,
}

impl Pass<'_> {
    /// Search one window of candidates and append its path-sorted results.
    fn flush(&mut self, window: &mut Vec<WalkEntry>, budget: &Budget) -> Result<(), EngineError> {
        let mut found = Vec::new();
        for file in window.drain(..) {
            budget.check()?;
            match self.worker.read(&file.path, false) {
                ReadOutcome::Read => {}
                ReadOutcome::Oversized => {
                    self.deferred.push(file);
                    continue;
                }
                ReadOutcome::Skipped | ReadOutcome::Failed => continue,
            }
            self.files_searched += 1;
            let search = self.worker.search_lenient(self.per_file);
            if search.match_count > 0 {
                found.push((file.relative, search));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        self.emitted += found.iter().map(|(_, s)| s.matches.len() as u64).sum::<u64>();
        self.results.extend(found);
        Ok(())
    }
}

/// Search a file or a directory tree (`grep_sync`, content mode, offset 0).
pub fn grep(
    matcher: &CompiledMatcher,
    path: &Path,
    glob: Option<&str>,
    p: &GrepParams,
    budget: &Budget,
) -> Result<GrepOutcome, EngineError> {
    let meta = std::fs::metadata(path).map_err(|e| EngineError::Io(format!("Path not found: {e}")))?;
    let glob = glob.map(str::trim).filter(|g| !g.is_empty());
    let compiled_glob = glob.map(|g| compile_glob(&build_glob_pattern(g, true))).transpose()?;
    let mut worker = Worker::new(matcher, p);
    if meta.is_file() {
        budget.check()?;
        match worker.read(path, false) {
            ReadOutcome::Read => {}
            ReadOutcome::Oversized => {
                if !matches!(worker.read(path, true), ReadOutcome::Read) {
                    return Ok(GrepOutcome { skipped_oversized: 1, ..Default::default() });
                }
            }
            ReadOutcome::Skipped | ReadOutcome::Failed => return Ok(GrepOutcome::default()),
        }
        // An explicitly named file reports a failed search instead of "no matches".
        let search = worker.search(p.max_count).map_err(|e| EngineError::Io(format!("Search failed: {e}")))?;
        let limit_reached = search.limit_reached || p.max_count.is_some_and(|m| search.collected >= m);
        let path_string = path.to_string_lossy().into_owned();
        return Ok(GrepOutcome {
            total_matches: search.match_count,
            matches: search.matches.into_iter().map(|c| to_match(path_string.clone(), c)).collect(),
            files_searched: 1,
            limit_reached,
            skipped_oversized: 0,
        });
    }
    if !meta.is_dir() {
        return Ok(GrepOutcome::default());
    }
    let opts = WalkOptions {
        include_hidden: p.include_hidden,
        use_gitignore: p.use_gitignore,
        skip_node_modules: !glob.is_some_and(|g| g.contains("node_modules")),
        max_depth: usize::MAX,
    };
    let per_file = match (p.max_count, p.max_count_per_file) {
        (Some(g), Some(f)) => Some(g.min(f)),
        (g, f) => g.or(f),
    };
    let stop_after = p.max_count.filter(|m| *m > 0).unwrap_or(u64::MAX);
    let mut pass = Pass {
        worker,
        per_file,
        results: Vec::new(),
        deferred: Vec::new(),
        emitted: 0,
        files_searched: 0,
        skipped_oversized: 0,
    };
    let mut window: Vec<WalkEntry> = Vec::with_capacity(STREAM_WINDOW);
    let mut failure = None;
    walk::walk(path, opts, budget, |e| {
        if e.kind != EntryKind::File || !compiled_glob.as_ref().is_none_or(|g| g.is_match(&e.relative)) {
            return Visit::Continue;
        }
        window.push(e);
        if window.len() == STREAM_WINDOW {
            if let Err(err) = pass.flush(&mut window, budget) {
                failure = Some(err);
                return Visit::Stop;
            }
            if pass.emitted >= stop_after {
                return Visit::Stop;
            }
        }
        Visit::Continue
    })?;
    if let Some(err) = failure {
        return Err(err);
    }
    if pass.emitted < stop_after {
        pass.flush(&mut window, budget)?;
    }
    if pass.emitted < stop_after && !pass.deferred.is_empty() {
        let mut deferred = std::mem::take(&mut pass.deferred);
        deferred.sort_by(|a, b| a.relative.cmp(&b.relative));
        let mut found = Vec::new();
        for file in deferred {
            budget.check()?;
            if pass.emitted >= stop_after {
                break;
            }
            match pass.worker.read(&file.path, true) {
                ReadOutcome::Read => {}
                ReadOutcome::Failed => continue,
                _ => {
                    pass.skipped_oversized += 1;
                    continue;
                }
            }
            pass.files_searched += 1;
            let search = pass.worker.search_lenient(per_file);
            if search.match_count > 0 {
                pass.emitted += search.collected;
                found.push((file.relative, search));
            }
        }
        found.sort_by(|a, b| a.0.cmp(&b.0));
        pass.results.extend(found);
    }

    // aggregate_parallel_results (content mode, offset 0)
    let mut out = GrepOutcome {
        files_searched: pass.files_searched,
        skipped_oversized: pass.skipped_oversized,
        ..Default::default()
    };
    let mut taken = 0u64;
    for (relative, search) in pass.results {
        out.total_matches += search.match_count;
        for c in search.matches {
            if p.max_count.is_some_and(|m| taken >= m) {
                out.limit_reached = true;
                break;
            }
            out.matches.push(to_match(relative.clone(), c));
            taken += 1;
        }
        if search.limit_reached {
            out.limit_reached = true;
        }
    }
    if p.max_count.is_some_and(|m| taken >= m) {
        out.limit_reached = true;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_patterns_follow_upstream_normalization() {
        assert_eq!(build_glob_pattern("*.ts", true), "**/*.ts");
        assert_eq!(build_glob_pattern("src/*.ts", true), "src/*.ts");
        assert_eq!(build_glob_pattern("*.ts", false), "*.ts");
        assert_eq!(build_glob_pattern("src\\**\\*.ts", true), "src/**/*.ts");
        assert_eq!(build_glob_pattern("*.{ts,tsx,js", true), "**/*.{ts,tsx,js}");
        assert_eq!(build_glob_pattern("{alpha.txt,beta.txt}", true), "{alpha.txt,beta.txt}");
        assert_eq!(build_glob_pattern("{*.ts,*.tsx}", true), "**/{*.ts,*.tsx}");
        assert_eq!(walk_depth_bound("dir/*.ts"), Some(2));
        assert_eq!(walk_depth_bound("*"), Some(1));
        assert_eq!(walk_depth_bound("src/**/*.ts"), None);
        assert_eq!(walk_depth_bound("{a/b,c}/d.txt"), None);
    }

    #[test]
    fn brace_sanitizing_keeps_quantifiers_and_escapes() {
        assert_eq!(sanitize_braces("${platform}"), "$\\{platform\\}");
        assert_eq!(sanitize_braces("a{2,3}"), "a{2,3}");
        assert_eq!(sanitize_braces(r"\p{L}+{x}"), r"\p{L}+\{x\}");
        assert_eq!(sanitize_braces(r"\x{41}"), r"\x{41}");
        assert!(matches!(sanitize_braces("plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn matcher_fallback_chain() {
        let hit = |pat: &str, hay: &str| build_matcher(pat, false, false).unwrap().is_match(hay.as_bytes()).unwrap();
        assert!(hit("fetchProvider(", "x = fetchProvider(a)"), "stray paren escaped");
        assert!(hit(r"foo(?=bar)", "foobar"), "lookahead via PCRE2");
        assert!(!hit(r"foo(?=bar)", "foobaz"));
        assert!(hit(r"(a)\1", "aa"), "backreference via PCRE2");
        assert!(hit("a{b}", "xa{b}y"), "non-quantifier braces escaped");
        assert!(!hit("${platform}", "path/${platform}/x"), "`$` stays an anchor, as upstream");
        assert!(hit("[unclosed", "x [unclosed y"), "literal fallback");
        assert!(build_matcher("ABC", true, false).unwrap().is_match(b"xabcx").unwrap());
    }

    #[test]
    fn column_truncation_respects_char_boundaries() {
        let (s, t) = truncate_line("é".repeat(10), Some(8));
        assert!(t && s == "éé...", "{s}");
        assert_eq!(truncate_line("short".into(), Some(8)), ("short".into(), false));
    }
}
