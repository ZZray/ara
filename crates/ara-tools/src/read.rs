//! `read` tool (subset of OMP `tools/read.ts` + `read-format.ts`).
//!
//! Ported: inline selectors `:N`, `:N-`, `:N-M`, `:N+K`, `:-N`, `:raw` (and a
//! range combined with raw), `%3A` escapes, `Invalid selector` errors, a
//! literal existing path (lstat probe) winning over selector parsing,
//! beyond-EOF message, head truncation at 3000 lines / 50 KB with a
//! continuation notice, the `not scanned to EOF` notice for huge files, a
//! bounded preview when a single line exceeds the byte cap, optional `N|text`
//! line numbers, directory listing, image files as image content, a binary
//! sniff of the first 8 KB only, `File not found`, non-regular files rejected.
//! Files are streamed on a blocking thread that honours cancellation.
//!
//! Hashline mode (edit tool in hashline mode): a `[path#TAG]` header plus
//! `N:text` rows; the tag hashes the same in-memory text that is displayed,
//! and the shown lines are recorded as seen. Files over the 4 MB snapshot cap
//! or not valid UTF-8 get display-only `N|text` rows and no tag (upstream
//! prints `N:` rows without a header there). A first line over the byte cap
//! gets the upstream "Hashline output requires full lines" message plus the
//! continuation notice. `.ipynb` files read as ara-edit's editable cell text
//! (`notebookToEditableText`), so their tags match what `edit` sees.
//!
//! Not ported (open): structural summaries and bracket context around
//! ranges, archives, SQLite, PDFs, URLs, internal
//! URIs other than `skill://`, suffix path resolution, `:conflicts`, video,
//! column caps, artifact spill.
//!
//! `skill://` (`internal_urls`): resolved to the skill file, then read with
//! the same selectors, as an immutable resource without result limits.

use crate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, ToolContext, format_bytes};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{ImageContent, JsonObject, Tool, UserBlock};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;
use tokio_util::sync::CancellationToken;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_DIR_ENTRIES: usize = 500;
const SNIFF_BYTES: usize = 8192;
/// Bytes scanned past the emitted window to count remaining lines.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const READ_DESCRIPTION: &str = "Read a local file or directory via `path`. Append a selector to `path` to read part of a file: `:50` (from line 50), `:50-200` (inclusive), `:50+150` (150 lines from 50), `:5-7,20-24` (separate ranges), `:-60` (last 60 lines), `:raw` (verbatim), or a range with raw (`:raw:2-4`). Encode a literal `:` in a path as `%3A`. Directories return a listing; images return image content. Output is capped at 3000 lines / 50KB; follow the continuation notice to read more.";
/// Hashline mode adds the snapshot-header rule (`read.md`, `IS_HL_MODE`).
const READ_DESCRIPTION_HASHLINE: &str = "Read a local file or directory via `path`. Append a selector to `path` to read part of a file: `:50` (from line 50), `:50-200` (inclusive), `:50+150` (150 lines from 50), `:5-7,20-24` (separate ranges), `:-60` (last 60 lines), `:raw` (verbatim, no anchors), or a range with raw (`:raw:2-4`). Encode a literal `:` in a path as `%3A`. Files return a `[path#TAG]` snapshot header plus `LINE:TEXT` numbered lines; copy `[FILENAME#TAG]` for anchored edits and NEVER fabricate the tag. Directories return a listing; images return image content. Output is capped at 3000 lines / 50KB; follow the continuation notice to read more.";
const SELECTOR_HELP: &str = "Use :N, :N-M, :N+K, :N- (open-ended), comma-separated ranges, :-N (last N lines), :raw, or a range combined with raw (e.g. :raw:50-100).";

#[derive(Debug, Clone, PartialEq)]
pub enum Range {
    /// 1-based inclusive start, optional inclusive end.
    From(usize, Option<usize>),
    /// Last N lines.
    Tail(usize),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Selector {
    pub range: Option<Range>,
    pub multi_ranges: Vec<Range>,
    pub raw: bool,
}

fn parse_range(sel: &str) -> Option<Range> {
    let normalized = sel.replace("..", "-");
    if let Some(n) = normalized.strip_prefix('-') {
        return n.parse::<usize>().ok().filter(|n| *n > 0).map(Range::Tail);
    }
    let sel = normalized.strip_prefix(['L', 'l']).unwrap_or(&normalized);
    let number = |part: &str| part.strip_prefix(['L', 'l']).unwrap_or(part).parse::<usize>().ok().filter(|n| *n > 0);
    if let Some((a, b)) = sel.split_once('+') {
        let start = number(a)?;
        let count = number(b)?;
        return start.checked_add(count - 1).map(|end| Range::From(start, Some(end)));
    }
    if let Some((a, b)) = sel.split_once('-') {
        let start = number(a)?;
        if b.is_empty() {
            return Some(Range::From(start, None));
        }
        let end = number(b)?;
        return (end >= start).then_some(Range::From(start, Some(end)));
    }
    number(sel).map(|n| Range::From(n, None))
}

fn looks_like_selector(sel: &str) -> bool {
    !sel.is_empty() && sel.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | ',' | '.' | 'L' | 'l'))
}

fn parse_ranges(sel: &str) -> Option<Vec<Range>> {
    let mut ranges = Vec::new();
    for chunk in sel.split(',') {
        let range = parse_range(chunk)?;
        if matches!(range, Range::Tail(_)) && sel.contains(',') {
            return None;
        }
        ranges.push(range);
    }
    if ranges.len() < 2 {
        return Some(ranges);
    }
    ranges.sort_by_key(|range| match range {
        Range::From(start, _) => *start,
        Range::Tail(_) => unreachable!(),
    });
    let mut merged: Vec<Range> = Vec::with_capacity(ranges.len());
    for range in ranges {
        if let (Some(Range::From(_, last_end)), Range::From(start, next_end)) = (merged.last_mut(), &range) {
            if last_end.is_none() {
                continue;
            }
            if *start <= last_end.unwrap().saturating_add(1) {
                if next_end.is_none_or(|next| next > last_end.unwrap()) {
                    *last_end = *next_end;
                }
                continue;
            }
        }
        merged.push(range);
    }
    Some(merged)
}

/// Split `path[:sel][:sel]` into a path and selector. A literal existing path
/// wins; `%3A` decodes to `:` after splitting. `Err` for a selector-shaped
/// suffix that is not valid (`:0`, `:9-3`, `:-0`).
pub fn split_selector(input: &str, exists: impl Fn(&str) -> bool) -> Result<(String, Selector), String> {
    let decode = |s: &str| s.replace("%3A", ":").replace("%3a", ":");
    if exists(&decode(input)) {
        return Ok((decode(input), Selector::default()));
    }
    let mut path = input.to_string();
    let mut selector = Selector::default();
    for _ in 0..2 {
        let Some((head, tail)) = path.rsplit_once(':') else { break };
        if head.is_empty() {
            break;
        }
        if tail == "raw" && !selector.raw {
            selector.raw = true;
        } else if selector.range.is_none() && selector.multi_ranges.is_empty() && looks_like_selector(tail) {
            match parse_ranges(tail) {
                Some(mut ranges) if ranges.len() == 1 => selector.range = ranges.pop(),
                Some(ranges) => selector.multi_ranges = ranges,
                None => return Err(format!("Invalid selector ':{tail}' on '{}'. {SELECTOR_HELP}", decode(head))),
            }
        } else {
            break;
        }
        path = head.to_string();
    }
    Ok((decode(&path), selector))
}

/// lstat-based probe: a dangling symlink or an unreadable entry still counts
/// as an existing literal path, so it is never reinterpreted as a selector.
/// As upstream `probeLiteralPathExists`, a missing entry, a non-directory
/// parent, or a name the OS cannot hold (too long; invalid on Windows) is not
/// an entry.
fn path_exists(p: &Path) -> bool {
    use std::io::ErrorKind;
    match std::fs::symlink_metadata(p) {
        Ok(_) => true,
        Err(e) => !matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory | ErrorKind::InvalidFilename),
    }
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// True when the first 8 KB contain NUL or invalid UTF-8 (a sequence cut at
/// the sniff boundary is not invalid).
fn sniff_binary(head: &[u8]) -> bool {
    if head.contains(&0) {
        return true;
    }
    match std::str::from_utf8(head) {
        Ok(_) => false,
        Err(e) => e.error_len().is_some(),
    }
}

fn read_up_to(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

struct Window {
    text: String,
    details: Value,
    /// Lines emitted (0 for binary, empty and beyond-EOF results).
    emitted: usize,
    /// First emitted line number.
    start: usize,
    /// Line number and byte size of a first line over the byte cap.
    oversized_first_line: Option<(usize, usize)>,
    /// Exact raw lines displayed by a disjoint multi-range read.
    raw_seen_lines: Option<Vec<u32>>,
}

fn cut_at_char_boundary(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Line prefix style for a read window.
#[derive(Clone, Copy, PartialEq)]
enum Numbering {
    None,
    /// `N|text` (`readLineNumbers`).
    Pipe,
    /// `N:text` (hashline anchors).
    Hashline,
}

#[derive(Clone, Copy)]
struct ReadRender<'a> {
    numbering: Numbering,
    text_resource: bool,
    block_context: Option<(&'a str, &'a Path)>,
}

/// Stream the requested line window from `reader` (a file or in-memory
/// bytes of `size`); blocking, checks `cancel` while scanning.
fn read_window<R: BufRead + Seek>(
    mut reader: R,
    size: u64,
    display: &str,
    sel: &Selector,
    render: ReadRender<'_>,
    cancel: &CancellationToken,
) -> Result<Window, String> {
    let ReadRender { numbering, text_resource, .. } = render;
    let limits = !text_resource;
    let io = |e: std::io::Error| format!("Cannot read {display}: {e}");
    let mut head = vec![0u8; SNIFF_BYTES];
    let n = read_up_to(&mut reader, &mut head).map_err(io)?;
    head.truncate(n);
    if !sel.raw && !text_resource && sniff_binary(&head) {
        return Ok(Window {
            text: format!(
                "[Cannot read binary file '{display}' ({}); not valid UTF-8 text. Use ':raw' to read bytes verbatim.]",
                format_bytes(size)
            ),
            details: json!({"fileSize": size}),
            emitted: 0,
            start: 1,
            oversized_first_line: None,
            raw_seen_lines: None,
        });
    }
    reader.rewind().map_err(io)?;
    if !sel.multi_ranges.is_empty() {
        return read_multi_window::<_, MAX_SCAN_BYTES>(reader, size, display, sel, render, cancel);
    }
    let aborted = || format!("Read of {display} was aborted");

    // Tail selectors need the total line count first.
    let mut known_total: Option<usize> = None;
    if matches!(sel.range, Some(Range::Tail(_))) {
        let mut count = 0usize;
        let mut buf = Vec::new();
        let mut scanned = 0u64;
        loop {
            buf.clear();
            let k = reader.read_until(b'\n', &mut buf).map_err(io)?;
            if k == 0 {
                break;
            }
            count += 1;
            scanned += k as u64;
            if scanned > MAX_SCAN_BYTES {
                return Err(format!(
                    "{display} is too large to count lines for a tail selector ({})",
                    format_bytes(size)
                ));
            }
            if count.is_multiple_of(4096) && cancel.is_cancelled() {
                return Err(aborted());
            }
        }
        known_total = Some(count);
        reader.rewind().map_err(io)?;
    }
    let (requested_start, requested_end) = match (&sel.range, known_total) {
        (None, _) => (1usize, None),
        (Some(Range::Tail(n)), Some(total)) => (total.saturating_sub(*n) + 1, None),
        (Some(Range::Tail(_)), None) => unreachable!(),
        (Some(Range::From(s, e)), _) => (*s, *e),
    };
    let mut start = if !sel.raw && requested_start > 1 { requested_start - 1 } else { requested_start };
    let end = requested_end.map(|end| if sel.raw { end } else { end.saturating_add(3) });

    let mut out = String::new();
    let mut emitted = 0usize;
    let mut line_no = 0usize;
    let mut truncated_by: Option<&str> = None;
    let mut oversized_line: Option<(usize, usize)> = None;
    let mut buf = Vec::new();
    let mut scanned_after = 0u64;
    let mut reached_eof = false;
    let mut requested_seen = false;
    loop {
        buf.clear();
        let k = reader.read_until(b'\n', &mut buf).map_err(io)?;
        if k == 0 {
            reached_eof = true;
            break;
        }
        line_no += 1;
        if line_no == requested_start {
            requested_seen = true;
        }
        if line_no.is_multiple_of(4096) && cancel.is_cancelled() {
            return Err(aborted());
        }
        let in_window = line_no >= start && end.is_none_or(|e| line_no <= e) && truncated_by.is_none();
        if !in_window {
            if line_no > start || truncated_by.is_some() {
                scanned_after += k as u64;
                if scanned_after > MAX_SCAN_BYTES {
                    break;
                }
            }
            continue;
        }
        let raw_line = String::from_utf8_lossy(&buf);
        let line = raw_line.strip_suffix('\n').unwrap_or(&raw_line);
        let line = line.strip_suffix('\r').filter(|_| !sel.raw && !text_resource).unwrap_or(line);
        let rendered = match numbering {
            _ if sel.raw => line.to_string(),
            Numbering::Pipe => format!("{line_no}|{line}"),
            Numbering::Hashline => format!("{line_no}:{line}"),
            Numbering::None => line.to_string(),
        };
        if limits && line_no < requested_start && rendered.len() + 1 > DEFAULT_MAX_BYTES {
            // Context must not consume the entire budget before the requested line.
            start = requested_start;
            continue;
        }
        if limits && line_no == requested_start && emitted > 0 && out.len() + rendered.len() + 1 > DEFAULT_MAX_BYTES {
            out.clear();
            emitted = 0;
            start = requested_start;
        }
        if limits && emitted >= DEFAULT_MAX_LINES {
            truncated_by = Some("lines");
            continue;
        }
        if limits && out.len() + rendered.len() + 1 > DEFAULT_MAX_BYTES {
            if emitted == 0 {
                // A single line larger than the cap: bounded preview (OMP firstLineExceedsLimit).
                out.push_str(cut_at_char_boundary(&rendered, DEFAULT_MAX_BYTES));
                emitted = 1;
                oversized_line = Some((line_no, buf.len()));
            }
            truncated_by = Some("bytes");
            continue;
        }
        if emitted > 0 {
            out.push('\n');
        }
        out.push_str(&rendered);
        emitted += 1;
    }
    let total = if reached_eof { Some(line_no) } else { None };
    if !requested_seen && requested_start > line_no {
        let Some(total) = total else { return Err(format!("Cannot read {display}: scan budget exceeded")) };
        let suggestion = if total == 0 {
            if text_resource { "The resource is empty." } else { "The file is empty." }.to_string()
        } else {
            format!("Use :1 to read from the start, or :{total} to read the last line.")
        };
        return Ok(Window {
            text: format!(
                "Line {requested_start} is beyond end of {} ({total} lines total). {suggestion}",
                if text_resource { "resource" } else { "file" }
            ),
            details: json!({"totalLines": total, "fileSize": size}),
            emitted: 0,
            start: requested_start,
            oversized_first_line: None,
            raw_seen_lines: None,
        });
    }
    if emitted == 0 {
        let Some(total) = total else { return Err(format!("Cannot read {display}: scan budget exceeded")) };
        if total == 0 && sel.range.is_none() {
            return Ok(Window {
                text: if text_resource {
                    "Line 1 is beyond end of resource (0 lines total). The resource is empty.".into()
                } else {
                    String::new()
                },
                details: json!({"totalLines": 0, "fileSize": size}),
                emitted: 0,
                start,
                oversized_first_line: None,
                raw_seen_lines: None,
            });
        }
        let suggestion = if total == 0 {
            if text_resource { "The resource is empty." } else { "The file is empty." }.to_string()
        } else {
            format!("Use :1 to read from the start, or :{total} to read the last line.")
        };
        return Ok(Window {
            text: format!(
                "Line {start} is beyond end of {} ({total} lines total). {suggestion}",
                if text_resource { "resource" } else { "file" }
            ),
            details: json!({"totalLines": total, "fileSize": size}),
            emitted: 0,
            start,
            oversized_first_line: None,
            raw_seen_lines: None,
        });
    }
    let last_shown = start + emitted - 1;
    let hashline_oversized = numbering == Numbering::Hashline && !sel.raw && oversized_line.is_some();
    if let Some((n, bytes)) = oversized_line.filter(|_| hashline_oversized) {
        // Hashline anchors need whole lines: no editable preview (upstream
        // text); the continuation notice below still names the next line.
        out = format!(
            "[Line {n} is {}, exceeds {} limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]",
            format_bytes(bytes as u64),
            format_bytes(DEFAULT_MAX_BYTES as u64)
        );
    } else if let Some((n, bytes)) = oversized_line {
        out.push_str(&format!(
            "\n\n[Line {n} is {}, exceeds {} limit. Showing its first {}.]",
            format_bytes(bytes as u64),
            format_bytes(DEFAULT_MAX_BYTES as u64),
            format_bytes(DEFAULT_MAX_BYTES as u64)
        ));
    }
    let more_after = match total {
        Some(t) => t > last_shown,
        None => true,
    };
    if more_after && (truncated_by.is_some() || sel.range.is_some() || total.is_none()) {
        match total {
            Some(t) => out.push_str(&format!(
                "\n\n[{} more lines in {}. Use :{} to continue]",
                t - last_shown,
                if text_resource { "resource" } else { "file" },
                last_shown + 1
            )),
            None => out.push_str(&format!(
                "\n\n[More lines in {} ({} total; not scanned to EOF). Use :{} to continue]",
                if text_resource { "resource" } else { "file" },
                format_bytes(size),
                last_shown + 1
            )),
        }
    }
    let mut details = json!({"fileSize": size});
    if let Some(t) = total {
        details["totalLines"] = json!(t);
    }
    if let Some(by) = truncated_by {
        let output_lines = if hashline_oversized { 0 } else { emitted };
        details["truncation"] = json!({"truncated": true, "truncatedBy": by, "outputLines": output_lines});
    }
    Ok(Window { text: out, details, emitted, start, oversized_first_line: oversized_line, raw_seen_lines: None })
}

fn numbered_multi_row(line: u32, text: &str, numbering: Numbering) -> String {
    match numbering {
        Numbering::Pipe => format!("{line}|{text}"),
        Numbering::Hashline => format!("{line}:{text}"),
        Numbering::None => text.to_owned(),
    }
}

fn multi_separator(before: u32, after: u32) -> &'static str {
    if after == before + 1 { "\n" } else { "\n…\n" }
}

fn render_multi_rows(rows: &BTreeMap<u32, String>) -> String {
    let mut out = String::new();
    let mut previous = None;
    for (&line, row) in rows {
        if let Some(before) = previous {
            out.push_str(multi_separator(before, line));
        }
        out.push_str(row);
        previous = Some(line);
    }
    out
}

/// Add pinned OMP's AST/lexical boundary rows without displacing requested
/// lines from ARA's bounded result. Only complete, buffered UTF-8 files use it.
fn multi_block_context(
    text: &str,
    path: &Path,
    selected: &[u32],
    numbering: Numbering,
    max_bytes: usize,
    max_lines: usize,
) -> Option<(String, usize)> {
    if selected.is_empty() {
        return None;
    }
    // Internal text resources preserve CRLF verbatim; `str::lines` would
    // silently strip their CR bytes from selected and context rows.
    let lines: Vec<&str> = text.split_terminator('\n').collect();
    let mut rows = BTreeMap::new();
    for &line in selected {
        let source = lines.get(line.checked_sub(1)? as usize)?;
        rows.insert(line, numbered_multi_row(line, source, numbering));
    }
    let mut used_bytes = render_multi_rows(&rows).len();
    let path = path.to_str();
    let source = ara_edit::diff_string::BlockContextSource { path, lang: None };
    let mut added = false;
    for (line, content) in ara_edit::diff_string::find_block_context_lines(&lines, selected, &source) {
        if rows.contains_key(&line) || line == 0 || line as usize > lines.len() || rows.len() >= max_lines {
            continue;
        }
        let row = numbered_multi_row(line, &content, numbering);
        let before = rows.range(..line).next_back().map(|(&number, _)| number);
        let after = rows.range(line..).next().map(|(&number, _)| number);
        let old_separator = before.zip(after).map_or(0, |(a, b)| multi_separator(a, b).len());
        let new_separators =
            before.map_or(0, |a| multi_separator(a, line).len()) + after.map_or(0, |b| multi_separator(line, b).len());
        let projected =
            used_bytes.saturating_add(row.len()).saturating_add(new_separators).saturating_sub(old_separator);
        if projected > max_bytes {
            continue;
        }
        rows.insert(line, row);
        used_bytes = projected;
        added = true;
    }
    added.then(|| (render_multi_rows(&rows), rows.len()))
}

/// Multi-range reads use exact spans, unlike single-range reads with 1/3-line
/// padding. Keep one forward scan and a bounded output buffer for large files.
fn read_multi_window<R: BufRead, const POST_SCAN_LIMIT: u64>(
    mut reader: R,
    size: u64,
    display: &str,
    sel: &Selector,
    render: ReadRender<'_>,
    cancel: &CancellationToken,
) -> Result<Window, String> {
    let ReadRender { numbering, text_resource, block_context } = render;
    let io = |e: std::io::Error| format!("Cannot read {display}: {e}");
    let entity = if text_resource { "resource" } else { "file" };
    let mut out = String::new();
    let mut seen = Vec::new();
    let mut buf = Vec::new();
    let mut total = 0usize;
    let mut scanned_after = 0u64;
    let mut range_index = 0usize;
    let mut prior_range: Option<usize> = None;
    let mut truncated_by: Option<&str> = None;
    let mut last_emitted = 0usize;
    let mut oversized_line = None;
    let mut ended_with_newline = false;
    let mut reached_eof = false;
    loop {
        buf.clear();
        let bytes = reader.read_until(b'\n', &mut buf).map_err(io)?;
        if bytes == 0 {
            reached_eof = true;
            if !sel.raw || (!ended_with_newline && total != 0) || total == usize::MAX {
                break;
            }
            // Raw selectors address the final empty `split("\n")` segment.
        } else {
            ended_with_newline = buf.last() == Some(&b'\n');
        }
        total += 1;
        if total.is_multiple_of(4096) && cancel.is_cancelled() {
            return Err(format!("Read of {display} was aborted"));
        }
        while let Some(Range::From(_, Some(end))) = sel.multi_ranges.get(range_index) {
            if total <= *end {
                break;
            }
            range_index += 1;
        }
        if let Some(Range::From(start, end)) = sel.multi_ranges.get(range_index)
            && total >= *start
            && end.is_none_or(|end| total <= end)
            && truncated_by.is_none()
        {
            let raw_line = String::from_utf8_lossy(&buf);
            let line = raw_line.strip_suffix('\n').unwrap_or(&raw_line);
            let line = line.strip_suffix('\r').filter(|_| !sel.raw && !text_resource).unwrap_or(line);
            let rendered = match numbering {
                _ if sel.raw => line.to_string(),
                Numbering::Pipe => format!("{total}|{line}"),
                Numbering::Hashline => format!("{total}:{line}"),
                Numbering::None => line.to_string(),
            };
            let separator = if seen.is_empty() {
                ""
            } else if prior_range != Some(range_index) {
                if sel.raw { "\n\n…\n\n" } else { "\n…\n" }
            } else {
                "\n"
            };
            let extra = separator.len() + rendered.len();
            if !text_resource && seen.len() >= DEFAULT_MAX_LINES {
                truncated_by = Some("lines");
            } else if !text_resource && out.len() + extra > DEFAULT_MAX_BYTES {
                truncated_by = Some("bytes");
                if rendered.len() > DEFAULT_MAX_BYTES {
                    oversized_line = Some(total);
                }
                if seen.is_empty() {
                    let preview = cut_at_char_boundary(&rendered, DEFAULT_MAX_BYTES);
                    out = if numbering == Numbering::Hashline && !sel.raw {
                        format!(
                            "[Line {total} exceeds {} limit. Hashline output requires full lines.]",
                            format_bytes(DEFAULT_MAX_BYTES as u64)
                        )
                    } else {
                        preview.to_string()
                    };
                }
            } else {
                out.push_str(separator);
                out.push_str(&rendered);
                seen.push(total as u32);
                last_emitted = total;
                prior_range = Some(range_index);
            }
        }
        if bytes == 0 {
            break;
        }
        // A late requested span is still reachable, as with a single-range
        // read. Bound only the post-selection scan for EOF/total-line notes.
        if truncated_by.is_some() || range_index >= sel.multi_ranges.len() {
            scanned_after += bytes as u64;
            if scanned_after > POST_SCAN_LIMIT {
                break;
            }
        }
    }
    if !reached_eof && seen.is_empty() && truncated_by.is_none() {
        return Err(format!("Cannot read {display}: scan budget exceeded"));
    }
    let selection_truncated = truncated_by.is_some();
    let mut emitted = seen.len();
    // Preserve notices that fit after the selected rows before spending the
    // remaining budget on optional block context.
    let mut notices = String::new();
    if reached_eof {
        let mut omitted = 0usize;
        for range in &sel.multi_ranges {
            if let Range::From(start, end) = range
                && *start > total
            {
                let bound = end.map_or_else(|| start.to_string(), |end| format!("{start}-{end}"));
                let notice = format!("[Range {bound} is beyond end of {entity} ({total} lines total); skipped]");
                if text_resource
                    || out.len() + notices.len() + usize::from(!out.is_empty() || !notices.is_empty()) + notice.len()
                        <= DEFAULT_MAX_BYTES
                {
                    if !out.is_empty() || !notices.is_empty() {
                        notices.push('\n');
                    }
                    notices.push_str(&notice);
                } else {
                    omitted += 1;
                }
            }
        }
        if omitted > 0 {
            truncated_by.get_or_insert("bytes");
            notices.push_str(&format!("\n[{omitted} additional out-of-bounds ranges omitted]"));
        }
    }
    if !sel.raw
        && let Some((full_text, path)) = block_context
    {
        if cancel.is_cancelled() {
            return Err(format!("Read of {display} was aborted"));
        }
        let (max_bytes, max_lines) = if text_resource {
            (usize::MAX, usize::MAX)
        } else {
            (DEFAULT_MAX_BYTES.saturating_sub(notices.len()), DEFAULT_MAX_LINES)
        };
        if let Some((rendered, count)) = multi_block_context(full_text, path, &seen, numbering, max_bytes, max_lines) {
            out = rendered;
            emitted = count;
        }
        if cancel.is_cancelled() {
            return Err(format!("Read of {display} was aborted"));
        }
    }
    out.push_str(&notices);
    if selection_truncated {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let after = oversized_line.unwrap_or(last_emitted).saturating_add(1);
        let next = sel
            .multi_ranges
            .iter()
            .find_map(|range| match range {
                Range::From(start, end) if end.is_none_or(|end| end >= after) => Some((*start).max(after)),
                _ => None,
            })
            .unwrap_or(after);
        out.push_str(&format!("[Read limit reached; use :{next} to continue]"));
    }
    let mut details = json!({"fileSize": size});
    if reached_eof {
        details["totalLines"] = json!(total);
    }
    if let Some(by) = truncated_by {
        details["truncation"] = json!({"truncated": true, "truncatedBy": by, "outputLines": emitted});
    }
    Ok(Window {
        text: out,
        details,
        emitted,
        start: seen.first().copied().unwrap_or(1) as usize,
        oversized_first_line: None,
        raw_seen_lines: sel.raw.then_some(seen),
    })
}

pub struct ReadTool {
    pub ctx: ToolContext,
}

#[async_trait]
impl AgentTool for ReadTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "read".into(),
            description: if self.ctx.hashlines() { READ_DESCRIPTION_HASHLINE.into() } else { READ_DESCRIPTION.into() },
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Local path. Inline selectors are supported."}},
                "required": ["path"],
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
        let input = args.get("path").and_then(|v| v.as_str()).unwrap_or_default();
        // Internal URLs (`skill://…`) resolve to a file first; selectors apply
        // to it. A skill resource is immutable and exempt from result limits
        // (upstream `immutable`, `ignoreResultLimits: scheme === "skill"`):
        // no hashline tags, no notebook projection, no line/byte truncation.
        let internal = crate::internal_urls::is_internal_url(input);
        let (path, sel) = if internal {
            let exists = |p: &str| self.ctx.resolve_internal_url(p).is_ok_and(|abs| path_exists(&abs));
            let (url, sel) = split_selector(input, exists).map_err(ToolError)?;
            let target = self.ctx.resolve_internal_url(&url).map_err(ToolError)?;
            (target.to_string_lossy().into_owned(), sel)
        } else {
            split_selector(input, |p| path_exists(&self.ctx.resolve(p))).map_err(ToolError)?
        };
        let abs = self.ctx.resolve(&path);
        let display = self.ctx.display(&abs);
        let meta = match tokio::fs::metadata(&abs).await {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ToolError(format!("File not found: {display}")));
            }
            Err(e) => return Err(ToolError(format!("Cannot read {display}: {e}"))),
        };
        let resolved = json!(abs.to_string_lossy());
        if meta.is_dir() {
            if !sel.multi_ranges.is_empty() && !internal {
                return Err(ToolError(format!("Multiple line ranges require a text file: {display} is a directory")));
            }
            let mut entries = Vec::new();
            let mut rd =
                tokio::fs::read_dir(&abs).await.map_err(|e| ToolError(format!("Cannot list {display}: {e}")))?;
            while let Ok(Some(entry)) = rd.next_entry().await {
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                entries.push((is_dir, entry.file_name().to_string_lossy().into_owned()));
            }
            if internal {
                entries.sort_by(|a, b| {
                    b.0.cmp(&a.0).then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase())).then_with(|| a.1.cmp(&b.1))
                });
            } else {
                entries.sort_by(|a, b| a.1.cmp(&b.1));
            }
            let names: Vec<String> =
                entries.into_iter().map(|(is_dir, name)| format!("{name}{}", if is_dir { "/" } else { "" })).collect();
            if internal {
                let mut content = if names.is_empty() { "(empty directory)".into() } else { names.join("\n") };
                let size = content.len() as u64;
                if sel.raw && sel.multi_ranges.is_empty() && (content.is_empty() || content.ends_with('\n')) {
                    content.push('\n');
                }
                let numbering = if self.ctx.line_numbers && !sel.raw { Numbering::Pipe } else { Numbering::None };
                let window = read_window(
                    std::io::Cursor::new(content.as_bytes()),
                    size,
                    &display,
                    &sel,
                    ReadRender {
                        numbering,
                        text_resource: true,
                        block_context: (!sel.raw && !sel.multi_ranges.is_empty())
                            .then_some((content.as_str(), abs.as_path())),
                    },
                    &cancel,
                )
                .map_err(ToolError)?;
                let mut details = window.details;
                details["resolvedPath"] = resolved;
                details["isDirectory"] = json!(true);
                details["contentType"] = json!("text/plain");
                return Ok(ToolOutput::text(window.text).with_details(details));
            }
            let total = names.len();
            let mut text = names.iter().take(MAX_DIR_ENTRIES).cloned().collect::<Vec<_>>().join("\n");
            if total == 0 {
                text = "(empty directory)".into();
            } else if total > MAX_DIR_ENTRIES {
                text.push_str(&format!("\n\n[{} more entries in listing]", total - MAX_DIR_ENTRIES));
            }
            return Ok(ToolOutput::text(text).with_details(json!({"resolvedPath": resolved, "isDirectory": true})));
        }
        if !meta.is_file() {
            return Err(ToolError(format!("Cannot read {display}: not a regular file")));
        }
        if !sel.multi_ranges.is_empty() && !internal && image_mime(&abs).is_some() && !sel.raw {
            return Err(ToolError(format!("Multiple line ranges require a text file: {display} is an image")));
        }
        if !internal
            && !sel.raw
            && let Some(mime) = image_mime(&abs)
        {
            if meta.len() > MAX_IMAGE_BYTES {
                return Err(ToolError(format!(
                    "Image {display} is {}, exceeds the {} limit",
                    format_bytes(meta.len()),
                    format_bytes(MAX_IMAGE_BYTES)
                )));
            }
            let bytes = tokio::fs::read(&abs).await.map_err(|e| ToolError(format!("Cannot read {display}: {e}")))?;
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            return Ok(ToolOutput {
                content: vec![
                    UserBlock::text(format!("Read image file [{mime}] {display} ({})", format_bytes(meta.len()))),
                    UserBlock::Image(ImageContent { data, mime_type: mime.into() }),
                ],
                details: Some(json!({"resolvedPath": resolved, "fileSize": meta.len()})),
                is_error: false,
            });
        }
        let hashlines = self.ctx.hashlines() && !sel.raw && !internal;
        let notebook = !sel.raw && !internal && abs.extension().is_some_and(|e| e.eq_ignore_ascii_case("ipynb"));
        let raw = sel.raw;
        let line_numbers = self.ctx.line_numbers;
        let store = self.ctx.edit_store.clone();
        let ctx = self.ctx.clone();
        let (abs2, display2, cancel2) = (abs.clone(), display.clone(), cancel.clone());
        let file_size = meta.len();
        let (window, text) = tokio::task::spawn_blocking(move || -> Result<(Window, String), String> {
            let io = |e: std::io::Error| format!("Cannot read {display2}: {e}");
            // SkillProtocolHandler creates an in-memory UTF-8 text resource
            // for every regular file, including images and invalid UTF-8.
            let resource_text = if internal {
                let bytes = std::fs::read(&abs2).map_err(io)?;
                let mut content = String::from_utf8_lossy(&bytes).into_owned();
                let size = content.len() as u64;
                // Raw selectors address `text.split("\n")`, including the
                // final empty segment of an empty or newline-terminated file.
                if raw && sel.multi_ranges.is_empty() && (content.is_empty() || content.ends_with('\n')) {
                    content.push('\n');
                }
                Some((content, size))
            } else {
                None
            };
            // The text a tag is minted from is exactly the text displayed:
            // notebooks as editable cells, files up to the snapshot cap from
            // one in-memory read. Without such text (over 4 MB, or not UTF-8)
            // rows use display-only `N|` numbering and carry no tag.
            // Editable text is BOM-stripped and LF-normalized exactly as the
            // edit engine sees it, so displayed line numbers are its numbers.
            let mut undecodable: Option<Vec<u8>> = None;
            let editable: Option<String> = if notebook {
                let json = std::fs::read_to_string(&abs2).map_err(io)?;
                let cells =
                    ara_edit::notebook::notebook_to_editable_text(&json, &display2).map_err(|e| e.to_string())?;
                Some(ara_edit::text::normalize_to_lf(&cells).into_owned())
            } else if (hashlines || (!raw && !sel.multi_ranges.is_empty()))
                && file_size <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES
            {
                match String::from_utf8(std::fs::read(&abs2).map_err(io)?) {
                    Ok(text) => Some(ara_edit::text::normalize_to_lf(ara_edit::text::strip_bom(&text).1).into_owned()),
                    Err(e) => {
                        undecodable = Some(e.into_bytes());
                        None
                    }
                }
            } else {
                None
            };
            let taggable = hashlines && editable.is_some();
            let numbering = if taggable {
                Numbering::Hashline
            } else if hashlines || line_numbers {
                Numbering::Pipe
            } else {
                Numbering::None
            };
            let window = if let Some((content, size)) = &resource_text {
                read_window(
                    std::io::Cursor::new(content.as_bytes()),
                    *size,
                    &display2,
                    &sel,
                    ReadRender {
                        numbering,
                        text_resource: true,
                        block_context: (!raw && !sel.multi_ranges.is_empty())
                            .then_some((content.as_str(), abs2.as_path())),
                    },
                    &cancel2,
                )?
            } else {
                match (&editable, &undecodable) {
                    (Some(text), _) => read_window(
                        std::io::Cursor::new(text.as_bytes()),
                        text.len() as u64,
                        &display2,
                        &sel,
                        ReadRender {
                            numbering,
                            text_resource: false,
                            block_context: (!notebook
                                && !raw
                                && !sel.multi_ranges.is_empty()
                                && text.len() as u64 <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES)
                                .then_some((text.as_str(), abs2.as_path())),
                        },
                        &cancel2,
                    )?,
                    (None, Some(bytes)) => read_window(
                        std::io::Cursor::new(bytes.as_slice()),
                        bytes.len() as u64,
                        &display2,
                        &sel,
                        ReadRender { numbering, text_resource: false, block_context: None },
                        &cancel2,
                    )?,
                    (None, None) => {
                        let file = std::fs::File::open(&abs2).map_err(io)?;
                        let size = file.metadata().map_err(io)?.len();
                        read_window(
                            BufReader::with_capacity(64 * 1024, file),
                            size,
                            &display2,
                            &sel,
                            ReadRender { numbering, text_resource: false, block_context: None },
                            &cancel2,
                        )?
                    }
                }
            };
            let mut text = window.text.clone();
            if window.emitted > 0 && window.oversized_first_line.is_none() {
                let key = ara_edit::path_policy::canonical_key(&abs2);
                if let (true, Some(content)) = (taggable, &editable) {
                    let tag = store.record(&key, content, None);
                    let header =
                        ara_edit::modes::hashline::format::format_hashline_header(&ctx.hashline_display(&abs2), &tag);
                    text = format!("{header}\n{text}");
                    let seen = ara_edit::store::seen_lines_from_body(&text);
                    if !seen.is_empty() {
                        store.record_seen_lines(&key, &tag, &seen);
                    }
                } else if raw && !internal {
                    // A raw read has no header, but records the range it showed so
                    // a same-content hashline tag inherits its provenance.
                    let seen: Vec<u32> = window
                        .raw_seen_lines
                        .clone()
                        .unwrap_or_else(|| (window.start..window.start + window.emitted).map(|n| n as u32).collect());
                    store.record_file(&abs2, Some(&seen));
                }
            }
            Ok((window, text))
        })
        .await
        .map_err(|e| ToolError(format!("Cannot read {display}: {e}")))?
        .map_err(ToolError)?;
        let mut details = window.details;
        details["resolvedPath"] = resolved;
        // The on-disk size, not the size of a notebook's cell projection.
        details["fileSize"] = json!(meta.len());
        if internal {
            let markdown = abs.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
                ["md", "markdown", "mdx", "mdc", "mkd", "mdown"]
                    .iter()
                    .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            });
            details["contentType"] = json!(if markdown { "text/markdown" } else { "text/plain" });
        }
        Ok(ToolOutput::text(text).with_details(details))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_probe_treats_unnameable_paths_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(path_exists(&file));
        assert!(!path_exists(&dir.path().join("missing.txt")));
        assert!(!path_exists(&file.join("child")), "a file cannot be a parent");
        assert!(!path_exists(&dir.path().join("a".repeat(300))), "name past the OS limit");
    }

    #[test]
    fn selectors() {
        let none = |_: &str| false;
        let ok = |s: &str| split_selector(s, none).unwrap();
        assert_eq!(
            ok("a.txt:5-9"),
            ("a.txt".into(), Selector { range: Some(Range::From(5, Some(9))), multi_ranges: vec![], raw: false })
        );
        assert_eq!(ok("a.txt:5+3").1.range, Some(Range::From(5, Some(7))));
        assert_eq!(ok("a.txt:-4").1.range, Some(Range::Tail(4)));
        assert_eq!(ok("a.txt:7-").1.range, Some(Range::From(7, None)));
        assert_eq!(
            ok("a.txt:raw:2-4"),
            ("a.txt".into(), Selector { range: Some(Range::From(2, Some(4))), multi_ranges: vec![], raw: true })
        );
        assert_eq!(
            ok("a.txt:2-4:raw").1,
            Selector { range: Some(Range::From(2, Some(4))), multi_ranges: vec![], raw: true }
        );
        assert_eq!(ok("a.txt:9,3-4,4-5").1.multi_ranges, vec![Range::From(3, Some(5)), Range::From(9, None)]);
        assert_eq!(ok("a.txt:5-7,1-4").1.range, Some(Range::From(1, Some(7))));
        assert_eq!(ok("a.txt:2-3,8-").1.multi_ranges, vec![Range::From(2, Some(3)), Range::From(8, None)]);
        assert_eq!(ok("a.txt:L2..L3,L8+L2").1.multi_ranges, vec![Range::From(2, Some(3)), Range::From(8, Some(9))]);
        assert_eq!(ok("dir/a%3Ab.txt").0, "dir/a:b.txt");
        assert_eq!(ok("weird:name"), ("weird:name".into(), Selector::default()));
        assert_eq!(split_selector("x:1", |p| p == "x:1").unwrap(), ("x:1".into(), Selector::default()));
        assert_eq!(split_selector("x:1-2,4", |p| p == "x:1-2,4").unwrap(), ("x:1-2,4".into(), Selector::default()));
        for bad in [
            "a.txt:0",
            "a.txt:9-3",
            "a.txt:-0",
            "a.txt:1+0",
            "a.txt:1,,3",
            "a.txt:1,-2",
            "a.txt:2+18446744073709551615",
        ] {
            let err = split_selector(bad, none).unwrap_err();
            assert!(err.starts_with("Invalid selector ':") && err.contains("on 'a.txt'"), "{err}");
        }
    }

    #[test]
    fn binary_sniff_is_limited_to_the_head() {
        assert!(sniff_binary(&[0, 1, 2]));
        assert!(sniff_binary(&[0xff, b'a']));
        let mut cut = "é".repeat(10).into_bytes();
        cut.pop();
        assert!(!sniff_binary(&cut), "sequence cut at the sniff boundary is not binary");
    }

    #[test]
    fn multi_range_scan_budget_starts_after_requested_lines() {
        let content = (1..=60).map(|n| format!("line{n}\n")).collect::<String>();
        let sel = Selector {
            range: None,
            multi_ranges: vec![Range::From(30, Some(30)), Range::From(50, Some(50))],
            raw: false,
        };
        let window = read_multi_window::<_, 8>(
            std::io::Cursor::new(content.as_bytes()),
            content.len() as u64,
            "lines.txt",
            &sel,
            ReadRender { numbering: Numbering::Pipe, text_resource: false, block_context: None },
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(window.text.starts_with("30|line30\n…\n50|line50"), "{}", window.text);
        assert!(!window.text.contains("continue"), "all selected ranges were complete: {}", window.text);
        assert!(window.details["totalLines"].is_null());
    }

    #[test]
    fn cancelled_buffered_multi_range_does_not_return_block_context() {
        let content = "function first() {\n  return 1;\n}\nfunction second() {\n  return 2;\n}\n";
        let sel =
            Selector { range: None, multi_ranges: vec![Range::From(1, Some(1)), Range::From(4, Some(4))], raw: false };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = read_multi_window::<_, MAX_SCAN_BYTES>(
            std::io::Cursor::new(content.as_bytes()),
            content.len() as u64,
            "blocks.ts",
            &sel,
            ReadRender {
                numbering: Numbering::Pipe,
                text_resource: false,
                block_context: Some((content, Path::new("blocks.ts"))),
            },
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error, "Read of blocks.ts was aborted");
    }
}
