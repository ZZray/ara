//! Fixed OMP conflict-detect.ts at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Copyright (c) OMP contributors. MIT licensed, see upstream license notices.
use ara_agent::ToolError;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConflictBlock {
    pub start_line: usize,
    pub separator_line: usize,
    pub end_line: usize,
    pub base_line: Option<usize>,
    pub ours_label: Option<String>,
    pub base_label: Option<String>,
    pub theirs_label: Option<String>,
    pub ours_lines: Vec<String>,
    pub base_lines: Option<Vec<String>>,
    pub theirs_lines: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictEntry {
    pub id: u64,
    pub absolute_path: PathBuf,
    pub display_path: String,
    pub block: ConflictBlock,
}
fn strip_cr(line: &str) -> &str {
    line.strip_suffix('\r').unwrap_or(line)
}
fn marker<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let tail = line.strip_prefix(prefix)?;
    if tail.is_empty() { Some("") } else { tail.strip_prefix(' ') }
}
fn label(raw: &str) -> Option<String> {
    (!raw.is_empty()).then(|| raw.to_owned())
}
pub fn scan_conflict_lines<S: AsRef<str>>(lines: &[S], first_line: usize) -> Vec<ConflictBlock> {
    let mut blocks = Vec::new();
    let mut partial: Option<ConflictBlock> = None;
    let mut phase = 0;
    for (i, line) in lines.iter().enumerate() {
        let line = strip_cr(line.as_ref());
        let ln = first_line + i;
        if let Some(ours) = marker(line, "<<<<<<<") {
            partial = Some(ConflictBlock { start_line: ln, ours_label: label(ours), ..Default::default() });
            phase = 1;
            continue;
        }
        let Some(block) = partial.as_mut() else {
            continue;
        };
        if let Some(base) = marker(line, "|||||||") {
            if phase != 1 {
                partial = None;
                phase = 0;
                continue;
            }
            block.base_line = Some(ln);
            block.base_label = label(base);
            block.base_lines = Some(Vec::new());
            phase = 2;
            continue;
        }
        if line == "=======" {
            if phase == 1 || phase == 2 {
                block.separator_line = ln;
                phase = 3;
            } else {
                partial = None;
                phase = 0;
            }
            continue;
        }
        if let Some(theirs) = marker(line, ">>>>>>>") {
            if phase == 3 {
                block.end_line = ln;
                block.theirs_label = label(theirs);
                blocks.push(partial.take().unwrap());
            } else {
                partial = None;
            }
            phase = 0;
            continue;
        }
        match phase {
            1 => block.ours_lines.push(line.to_owned()),
            2 => block.base_lines.as_mut().unwrap().push(line.to_owned()),
            3 => block.theirs_lines.push(line.to_owned()),
            _ => {}
        }
    }
    blocks
}
pub const SCAN_FILE_MAX_BYTES: u64 = 10 * 1024 * 1024;
pub fn scan_file_for_conflicts(path: &Path, max_bytes: u64) -> Result<(Vec<ConflictBlock>, bool), ToolError> {
    let file = std::fs::File::open(path).map_err(|e| ToolError(e.to_string()))?;
    let truncated = file.metadata().map_err(|e| ToolError(e.to_string()))?.len() > max_bytes;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes).map_err(|e| ToolError(e.to_string()))?;
    let text = String::from_utf8_lossy(&bytes);
    // Fixed TextDecoder UTF-8 consumes one leading BOM.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Ok((scan_conflict_lines(&text.split('\n').collect::<Vec<_>>(), 1), truncated))
}
#[derive(Debug)]
pub struct ConflictHistory {
    next_id: u64,
    entries: BTreeMap<u64, ConflictEntry>,
}
impl Default for ConflictHistory {
    fn default() -> Self {
        Self { next_id: 1, entries: BTreeMap::new() }
    }
}
impl ConflictHistory {
    pub fn register(&mut self, absolute_path: PathBuf, display_path: String, block: ConflictBlock) -> ConflictEntry {
        let old = self
            .entries
            .values()
            .find(|entry| entry.absolute_path == absolute_path && entry.block.start_line == block.start_line)
            .map(|e| e.id);
        let id = old.unwrap_or_else(|| {
            let id = self.next_id;
            self.next_id += 1;
            id
        });
        let entry = ConflictEntry { id, absolute_path, display_path, block };
        self.entries.insert(id, entry.clone());
        entry
    }
    pub fn get(&self, id: f64) -> Option<ConflictEntry> {
        self.entries.values().find(|e| e.id as f64 == id).cloned()
    }
    pub fn entries(&self) -> Vec<ConflictEntry> {
        self.entries.values().cloned().collect()
    }
    pub fn invalidate(&mut self, id: u64) {
        self.entries.remove(&id);
    }
    pub fn invalidate_path(&mut self, path: &Path) {
        self.entries.retain(|_, e| e.absolute_path != path);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictScope {
    Ours,
    Theirs,
    Base,
}
impl ConflictScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ours => "ours",
            Self::Theirs => "theirs",
            Self::Base => "base",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ConflictId {
    Number(f64),
    Wildcard,
}
impl std::fmt::Display for ConflictId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::Wildcard => f.write_str("*"),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedConflictUri {
    pub id: ConflictId,
    pub scope: Option<ConflictScope>,
    pub recovered_prefix: Option<String>,
}
pub fn parse_conflict_uri(raw: &str) -> Result<Option<ParsedConflictUri>, ToolError> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^(?:(.+):)?conflict://(.+)$").unwrap());
    let Some(c) = RE.captures(raw) else {
        return Ok(None);
    };
    let prefix = c.get(1).map(|s| s.as_str().to_owned());
    let tail = &c[2];
    let (id, scope) = tail.split_once('/').map_or((tail, None), |(id, scope)| (id, Some(scope)));
    if id == "*" {
        if let Some(scope) = scope {
            return Err(ToolError(format!(
                "Invalid conflict URI '{raw}': wildcard 'conflict://*' does not accept a scope segment. Drop '/{scope}' or use a numeric id."
            )));
        }
        return Ok(Some(ParsedConflictUri { id: ConflictId::Wildcard, scope: None, recovered_prefix: prefix }));
    }
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ToolError(format!(
            "Invalid conflict URI '{raw}': must be 'conflict://<N>', 'conflict://<N>/<scope>', or 'conflict://*' where N is a positive integer surfaced by a prior `read`."
        )));
    }
    // JS parseInt returns a Number, including large finite integral values.
    let number = id.parse::<f64>().unwrap_or(f64::INFINITY);
    if !number.is_finite() || number < 1.0 {
        return Err(ToolError(format!("Invalid conflict URI '{raw}': id must be ≥ 1.")));
    }
    let scope = match scope {
        None => None,
        Some("ours") => Some(ConflictScope::Ours),
        Some("theirs") => Some(ConflictScope::Theirs),
        Some("base") => Some(ConflictScope::Base),
        _ => {
            return Err(ToolError(format!(
                "Invalid conflict URI '{raw}': scope must be one of 'ours', 'theirs', 'base', or omitted (e.g. 'conflict://{number}/theirs')."
            )));
        }
    };
    Ok(Some(ParsedConflictUri { id: ConflictId::Number(number), scope, recovered_prefix: prefix }))
}
fn marker_line(prefix: &str, label: &Option<String>) -> String {
    label.as_ref().filter(|s| !s.is_empty()).map_or_else(|| prefix.to_owned(), |s| format!("{prefix} {s}"))
}
pub fn recorded_region(block: &ConflictBlock) -> Vec<String> {
    let mut out = vec![marker_line("<<<<<<<", &block.ours_label)];
    out.extend(block.ours_lines.clone());
    if let Some(base) = &block.base_lines {
        out.push(marker_line("|||||||", &block.base_label));
        out.extend(base.clone());
    }
    out.push("=======".to_owned());
    out.extend(block.theirs_lines.clone());
    out.push(marker_line(">>>>>>>", &block.theirs_label));
    out
}
pub fn conflict_regions_equal(a: &ConflictBlock, b: &ConflictBlock) -> bool {
    recorded_region(a) == recorded_region(b)
}
pub fn conflict_region_present(content: &str, entry: &ConflictBlock) -> bool {
    content.replace("\r\n", "\n").contains(&recorded_region(entry).join("\n"))
}
#[derive(Debug, PartialEq, Eq)]
pub struct ConflictSplice {
    pub text: String,
    pub trimmed_leading: usize,
    pub trimmed_trailing: usize,
}
fn balance(lines: &[String]) -> i64 {
    lines
        .iter()
        .flat_map(|s| s.chars())
        .map(|c| match c {
            '{' | '(' | '[' => 1,
            '}' | ')' | ']' => -1,
            _ => 0,
        })
        .sum()
}
pub fn splice_conflict(original: &str, entry: &ConflictEntry, replacement: &str) -> Result<ConflictSplice, ToolError> {
    let file = original.split('\n').collect::<Vec<_>>();
    let expected = recorded_region(&entry.block);
    let preferred = entry.block.start_line.saturating_sub(1);
    let matches = |at: usize| {
        at + expected.len() <= file.len() && expected.iter().enumerate().all(|(i, line)| strip_cr(file[at + i]) == line)
    };
    let at = if matches(preferred) {
        Some(preferred)
    } else {
        (0..=file.len().saturating_sub(expected.len())).filter(|&i| matches(i)).min_by_key(|i| i.abs_diff(preferred))
    };
    let Some(at) = at else {
        return Err(ToolError(format!(
            "Conflict #{} no longer present in '{}': the recorded marker block can't be located. The file changed since the conflict was registered — re-read it to re-register conflicts.",
            entry.id, entry.display_path
        )));
    };
    let end = at + expected.len() - 1;
    let trimmed = replacement.strip_suffix("\r\n").or_else(|| replacement.strip_suffix('\n')).unwrap_or(replacement);
    let mut lines = trimmed.split('\n').map(|s| strip_cr(s).to_owned()).collect::<Vec<_>>();
    let ours = balance(&entry.block.ours_lines);
    let expected_balance = (ours == balance(&entry.block.theirs_lines)).then_some(ours);
    let justifies = |all: &[String], without: &[String]| {
        expected_balance.is_some_and(|n| balance(all) != n && balance(without) == n)
    };
    let after = file[end + 1..].iter().take(12).map(|s| strip_cr(s)).collect::<Vec<_>>();
    let mut trailing = 0;
    for k in (1..=after.len().min(lines.len().saturating_sub(1))).rev() {
        if after[..k].iter().zip(&lines[lines.len() - k..]).any(|(a, b)| *a != b) {
            continue;
        }
        if k >= 2 || justifies(&lines, &lines[..lines.len() - 1]) {
            trailing = k;
            lines.truncate(lines.len() - k);
        }
        break;
    }
    let before = file[at.saturating_sub(12)..at].iter().map(|s| strip_cr(s)).collect::<Vec<_>>();
    let mut leading = 0;
    for k in (1..=before.len().min(lines.len().saturating_sub(1))).rev() {
        if before[before.len() - k..].iter().zip(&lines[..k]).any(|(a, b)| *a != b) {
            continue;
        }
        if k >= 2 || justifies(&lines, &lines[1..]) {
            leading = k;
            lines.drain(..k);
        }
        break;
    }
    if file[at].ends_with('\r') {
        let following = end + 1 < file.len();
        let last = lines.len() - 1;
        for (i, line) in lines.iter_mut().enumerate() {
            if i < last || following {
                line.push('\r');
            }
        }
    }
    let mut next = file[..at].iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    next.extend(lines);
    next.extend(file[end + 1..].iter().map(|s| (*s).to_owned()));
    Ok(ConflictSplice { text: next.join("\n"), trimmed_leading: leading, trimmed_trailing: trailing })
}
pub fn expand_content_tokens(content: &str, entry: &ConflictEntry) -> Result<String, ToolError> {
    let mut out = Vec::new();
    for raw in content.split('\n') {
        match strip_cr(raw) {
            "@ours" => out.extend(entry.block.ours_lines.clone()),
            "@theirs" => out.extend(entry.block.theirs_lines.clone()),
            "@both" => {
                out.extend(entry.block.ours_lines.clone());
                out.extend(entry.block.theirs_lines.clone());
            }
            "@base" => {
                let base = entry.block.base_lines.as_ref().ok_or_else(|| {
                    ToolError(format!(
                        "Conflict #{} has no base section (2-way merge). `@base` is only valid for diff3 conflicts.",
                        entry.id
                    ))
                })?;
                out.extend(base.clone());
            }
            _ => out.push(raw.to_owned()),
        }
    }
    Ok(out.join("\n"))
}
pub fn render_conflict_region(
    entry: &ConflictEntry,
    scope: Option<ConflictScope>,
) -> Result<(Vec<String>, usize), ToolError> {
    let block = &entry.block;
    match scope {
        None => Ok((recorded_region(block), block.start_line)),
        Some(ConflictScope::Ours) => Ok((block.ours_lines.clone(), block.start_line + 1)),
        Some(ConflictScope::Theirs) => Ok((block.theirs_lines.clone(), block.separator_line + 1)),
        Some(ConflictScope::Base) => {
            let (Some(base), Some(line)) = (&block.base_lines, block.base_line) else {
                return Err(ToolError(format!(
                    "Conflict #{} has no base section (2-way merge). 'conflict://{}/base' is only valid for diff3 conflicts.",
                    entry.id, entry.id
                )));
            };
            Ok((base.clone(), line + 1))
        }
    }
}
fn labels(out: &mut Vec<String>, entries: &[ConflictEntry]) {
    let pick = |get: fn(&ConflictBlock) -> Option<&String>| {
        entries.iter().find_map(|e| get(&e.block).filter(|s| !s.trim().is_empty())).cloned()
    };
    if let Some(label) = pick(|b| b.ours_label.as_ref()) {
        out.push(format!("- ours = {label}"));
    }
    if let Some(label) = pick(|b| b.theirs_label.as_ref()) {
        out.push(format!("- theirs = {label}"));
    }
    if entries.iter().any(|e| e.block.base_lines.is_some()) {
        out.push(format!(
            "- base = {}",
            pick(|b| b.base_lines.as_ref().and(b.base_label.as_ref())).unwrap_or_else(|| "(no label)".into())
        ));
    }
}
fn range(block: &ConflictBlock) -> String {
    if block.start_line == block.end_line {
        format!("L{}", block.start_line)
    } else {
        format!("L{}-{}", block.start_line, block.end_line)
    }
}
fn body(out: &mut Vec<String>, lines: &[String]) {
    if lines.is_empty() {
        out.push("(empty)".into());
        return;
    }
    out.extend(lines.iter().take(6).cloned());
    if lines.len() > 6 {
        let hidden = lines.len() - 6;
        out.push(format!("… ({hidden} more line{})", if hidden == 1 { "" } else { "s" }));
    }
}
#[derive(Default)]
pub struct WarningOptions<'a> {
    pub total_in_file: Option<usize>,
    pub display_path: Option<&'a str>,
    pub scan_truncated: bool,
}
const CAP_NOTICE: &str =
    "- note: file scan hit the byte cap; additional conflicts may exist beyond the scanned prefix.";
const NOTICE: &str = "NOTICE: Inspect a block by reading `conflict://<N>` (add `/ours` / `/theirs` / `/base` to render a single side). Resolve with `write({ path: \"conflict://<N>\", content })`, or bulk-resolve every registered conflict with `write({ path: \"conflict://*\", content })`. Writes replace ONLY the marker block (markers + all sides) — never repeat the lines before/after it; they stay in place.";
const TOKENS_NOTICE: &str = "`content` shorthand: a line that is exactly `@ours` / `@theirs` / `@base` / `@both` expands to that recorded section. `@both` is ours-then-theirs with no separator — only for additive conflicts where each side adds something different; NEVER for competing edits of the same lines (pick a side or write the combined text). Lines that are not a token pass through verbatim, so `\"// keep both\\n@ours\\n@theirs\"` literally writes the comment, then ours, then theirs.";
const DIRECTIVES_NOTICE: &str = "Per-id bulk: `write({ path: \"conflict://*\", content: \"1: @ours\\n2: @theirs\\n…\" })` resolves each listed id with that side in ONE call — the cheapest way through many pick-one conflicts; unlisted ids stay registered.";
const FAITHFUL_NOTICE: &str = "Resolve each block faithfully: keep one side (`@ours`/`@theirs`), or combine them when both intents apply — never invent content beyond the recorded sides, and never stack both sides of competing edits. Resolve several conflicts in a single turn by issuing multiple `write` calls at once; ids stay valid as earlier blocks are resolved.";
pub fn format_conflict_warning(entries: &[ConflictEntry], options: WarningOptions<'_>) -> String {
    if entries.is_empty() {
        return String::new();
    }
    let total = options.total_in_file.unwrap_or(entries.len());
    let word = if total == 1 { "conflict" } else { "conflicts" };
    let mut out = vec![String::new()];
    if total > entries.len() {
        out.push(format!(
            "⚠ {} of {total} unresolved {word} visible in this window (read `{}:conflicts` for the full list).",
            entries.len(),
            options.display_path.unwrap_or("<file>")
        ));
    } else {
        out.push(format!("⚠ {total} unresolved {word} detected"));
    }
    if options.scan_truncated {
        out.push(CAP_NOTICE.into());
    }
    labels(&mut out, entries);
    out.extend([NOTICE, TOKENS_NOTICE, DIRECTIVES_NOTICE, FAITHFUL_NOTICE].map(str::to_owned));
    for entry in entries {
        let b = &entry.block;
        out.push(String::new());
        out.push(format!("──── #{}  {} ────", entry.id, range(b)));
        out.push("<<< ours".into());
        body(&mut out, &b.ours_lines);
        if let Some(base) = &b.base_lines {
            if base == &b.ours_lines {
                out.push("=== base ≡ ours".into());
            } else if base == &b.theirs_lines {
                out.push("=== base ≡ theirs".into());
            } else {
                out.push("=== base".into());
                body(&mut out, base);
            }
        }
        if b.theirs_lines == b.ours_lines {
            out.push(">>> theirs ≡ ours".into());
        } else {
            out.push(">>> theirs".into());
            body(&mut out, &b.theirs_lines);
        }
    }
    out.join("\n")
}
pub fn format_conflict_summary(entries: &[ConflictEntry], display_path: &str, scan_truncated: bool) -> String {
    let total = entries.len();
    let mut out = vec![format!(
        "⚠ {total} unresolved {} in {}",
        if total == 1 { "conflict" } else { "conflicts" },
        if display_path.is_empty() { "<file>" } else { display_path }
    )];
    if scan_truncated {
        out.push(CAP_NOTICE.into());
    }
    labels(&mut out, entries);
    out.push("NOTICE: Bulk-resolve with `write({ path: \"conflict://*\", content })`, or address a single block with `write({ path: \"conflict://<N>\", content })`. Inspect a block by reading `conflict://<N>` (add `/ours` / `/theirs` / `/base` for a single side).".into());
    out.push("`content` shorthand: `@ours` / `@theirs` / `@base` / `@both` lines expand to the recorded sections; `@both` = ours-then-theirs (additive conflicts only — never for competing edits of the same lines). Per-id bulk: content of `<id>: @side` lines (e.g. \"1: @ours\\n2: @theirs\") resolves each listed id in one call. Non-token lines pass through verbatim. Writes replace ONLY the marker block — never repeat the surrounding lines. Keep one side or combine faithfully; never invent content beyond the recorded sides.".into());
    out.push(String::new());
    let width = entries.last().map_or(1, |e| e.id.to_string().len());
    for e in entries {
        out.push(format!(
            "#{:>width$}  {}{}",
            e.id,
            range(&e.block),
            if e.block.base_lines.is_some() { "  (3-way)" } else { "" }
        ));
    }
    out.join("\n")
}
pub fn parse_bulk_directives(content: &str) -> Result<Option<Vec<(f64, String)>>, ToolError> {
    static RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^#?([0-9]+)\s*[:=]\s*(@ours|@theirs|@base|@both)$").unwrap());
    static HEAD: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^#?[0-9]+\s*[:=]").unwrap());
    let mut map = Vec::new();
    let mut stray = Vec::new();
    for raw in content.split('\n') {
        let line = raw.trim_matches(|c: char| c == '\u{feff}' || matches!(c, '\u{0009}'..='\u{000d}' | '\u{0020}' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}'));
        if line.is_empty() {
            continue;
        }
        let Some(c) = RE.captures(line) else {
            stray.push(line);
            continue;
        };
        let id = c[1].parse::<f64>().unwrap_or(f64::INFINITY);
        if map.iter().any(|(key, _)| *key == id) {
            return Err(ToolError(format!("Bulk directive lists conflict #{id} twice — each id may appear once.")));
        }
        map.push((id, c[2].to_owned()));
    }
    if map.is_empty() {
        return Ok(None);
    }
    if let Some(sample) = stray.first() {
        let hint = if HEAD.is_match(sample) {
            "Per-id bulk only accepts the tokens @ours/@theirs/@base/@both — one side per id, single line. "
        } else {
            ""
        };
        let shown = if sample.encode_utf16().count() > 60 {
            format!("{}…", sample.chars().take(57).collect::<String>())
        } else {
            sample.to_string()
        };
        return Err(ToolError(format!(
            "Malformed `conflict://*` per-id block: {} line(s) are not `<id>: @side` directives (first: `{shown}`). {hint}Literal or multi-line replacement content isn't supported in a per-id block — resolve those blocks with individual `write({{ path: \"conflict://<N>\", content }})` calls (you can issue several at once). For a pure pick-a-side pass, make every non-empty line `<id>: @ours` (or @theirs/@base/@both).",
            stray.len()
        )));
    }
    Ok(Some(map))
}
pub fn resolve_bulk_directives(raw: &str, stripped: &str) -> Result<Option<Vec<(f64, String)>>, ToolError> {
    if raw == stripped {
        return parse_bulk_directives(raw);
    }
    match parse_bulk_directives(raw) {
        Err(error) => match parse_bulk_directives(stripped) {
            Ok(Some(map)) => Ok(Some(map)),
            _ => Err(error),
        },
        Ok(Some(map)) => Ok(Some(map)),
        Ok(None) => parse_bulk_directives(stripped),
    }
}
