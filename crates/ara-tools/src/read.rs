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
//! native URIs other than `skill://`, suffix path resolution, `:conflicts`, video,
//! column caps, artifact spill.
//!
//! `skill://` (`internal_urls`): resolved to the skill file, then read with
//! the same selectors, as an immutable resource without result limits.

// MIT License
// Copyright (c) 2025 Mario Zechner
// Copyright (c) 2025-2026 Can Bölük
// Copyright (c) 2026 Stencil Labs, Inc.
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use crate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, ToolContext};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool, UserBlock};
use async_trait::async_trait;
use icu_collator::{Collator, CollatorBorrowed, options::CollatorOptions};
use icu_locale_core::Locale;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Fixed utils/format.ts formatBytes for the READ size domain.
/// JS first represents byte counts as binary64. Round that exact quotient to
/// one decimal, choosing the larger decimal integer on a tie; Rust's default
/// float formatting instead chooses the even integer. Keep shared WRITE intact.
pub(crate) fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes}B");
    }
    let (divisor, suffix) = if bytes < 1024 * 1024 {
        (1024.0, "KB")
    } else if bytes < 1024 * 1024 * 1024 {
        (1048576.0, "MB")
    } else {
        (1073741824.0, "GB")
    };
    let bits = (bytes as f64 / divisor).to_bits();
    let mantissa = (bits & ((1u64 << 52) - 1)) | (1u64 << 52);
    // Quotients are finite and in [1, 2^34], so shift is in [18, 52].
    // The 53-bit mantissa times ten plus half a denominator fits u128.
    let shift = 1075 - ((bits >> 52) & 0x7ff);
    let denominator = 1u128 << shift;
    let tenths = (u128::from(mantissa) * 10 + denominator / 2) / denominator;
    format!("{}.{}{suffix}", tenths / 10, tenths % 10)
}

#[path = "read_archive.rs"]
mod archive;
#[path = "read_in_memory_wire.rs"]
mod in_memory_wire;

fn skill_directory_collator() -> Result<CollatorBorrowed<'static>, ToolError> {
    // Bun 1.4.0 resolves the unspecified locale to en-US on Linux even with
    // LANG/LC_ALL set to zh_CN.UTF-8. On Windows it follows the OS locale.
    #[cfg(target_os = "linux")]
    let locale_name = "en-US".to_owned();
    #[cfg(not(target_os = "linux"))]
    let locale_name = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
    let locale: Locale = locale_name.parse().map_err(|e| {
        ToolError(format!(
            "Cannot parse host locale {locale_name} for skill directory: {e}"
        ))
    })?;
    Collator::try_new(locale.into(), CollatorOptions::default()).map_err(|e| {
        ToolError(format!(
            "Cannot sort skill directory for locale {locale_name}: {e}"
        ))
    })
}

const SNIFF_BYTES: usize = 8192;
/// Bytes scanned past an ordinary file window to count remaining lines.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
// Fixed OMP596f2da prompts/tools/read.md, rendered through the existing prompt engine.
const READ_DESCRIPTION: &str = include_str!("read_description.md");
fn read_description(ctx: &ToolContext) -> String {
    let hashline = ctx.hashlines();
    let limit = serde_json::Number::from_f64(ctx.read_default_limit())
        .map(|number| ara_prompt::js::number_to_string(&number))
        .unwrap_or_else(|| "NaN".into());
    ara_prompt::render(
        READ_DESCRIPTION,
        &json!({
            "DEFAULT_LIMIT": limit,
            "DEFAULT_MAX_LINES": DEFAULT_MAX_LINES.to_string(),
            "IS_HL_MODE": hashline,
            "IS_LINE_NUMBER_MODE": !hashline && ctx.line_numbers,
            "INSPECT_IMAGE_ENABLED": ctx.read_image_runtime().inspect_image_active,
        }),
    )
    .expect("fixed OMP READ description template must render")
}
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
    let number = |part: &str| {
        part.strip_prefix(['L', 'l'])
            .unwrap_or(part)
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
    };
    if let Some((a, b)) = sel.split_once('+') {
        let start = number(a)?;
        let count = number(b)?;
        return start
            .checked_add(count - 1)
            .map(|end| Range::From(start, Some(end)));
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
    !sel.is_empty()
        && sel
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | ',' | '.' | 'L' | 'l'))
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
        if let (Some(Range::From(_, last_end)), Range::From(start, next_end)) =
            (merged.last_mut(), &range)
        {
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
pub fn split_selector(
    input: &str,
    exists: impl Fn(&str) -> bool,
) -> Result<(String, Selector), String> {
    let decode = |s: &str| s.replace("%3A", ":").replace("%3a", ":");
    if exists(&decode(input)) {
        return Ok((decode(input), Selector::default()));
    }
    let mut path = input.to_string();
    let mut selector = Selector::default();
    for _ in 0..2 {
        let Some((head, tail)) = path.rsplit_once(':') else {
            break;
        };
        if head.is_empty() {
            break;
        }
        if tail == "raw" && !selector.raw {
            selector.raw = true;
        } else if selector.range.is_none()
            && selector.multi_ranges.is_empty()
            && looks_like_selector(tail)
        {
            match parse_ranges(tail) {
                Some(mut ranges) if ranges.len() == 1 => selector.range = ranges.pop(),
                Some(ranges) => selector.multi_ranges = ranges,
                None => {
                    return Err(format!(
                        "Invalid selector ':{tail}' on '{}'. {SELECTOR_HELP}",
                        decode(head)
                    ));
                }
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
        Err(e) => !matches!(
            e.kind(),
            ErrorKind::NotFound | ErrorKind::NotADirectory | ErrorKind::InvalidFilename
        ),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowOrigin {
    BinaryRefusal,
    SingleRangeEof,
    Complete,
    MultiRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SuffixNoticeDisposition {
    DetailsOnly,
    PrefixFirstText,
    Omit,
}

struct PlainReadReceipt {
    output: ToolOutput,
    suffix_notice: SuffixNoticeDisposition,
}

impl PlainReadReceipt {
    fn project(mut self, suffix: Option<(String, String)>) -> ToolOutput {
        if self.suffix_notice == SuffixNoticeDisposition::Omit {
            return self.output;
        }
        let Some((from, to)) = suffix else {
            return self.output;
        };
        let details = self.output.details.get_or_insert_with(|| json!({}));
        details["suffixResolution"] = json!({"from":from,"to":to});
        if self.suffix_notice == SuffixNoticeDisposition::PrefixFirstText {
            let notice = format!("[Path '{from}' not found; resolved to '{to}' via suffix match]");
            if let Some(text) = self.output.content.iter_mut().find_map(|block| {
                if let UserBlock::Text(text) = block {
                    Some(text)
                } else {
                    None
                }
            }) {
                text.text = format!("{notice}\n{}", text.text);
            } else {
                self.output.content.insert(0, UserBlock::text(notice));
            }
        }
        self.output
    }
}

enum ReadDispatch {
    Completed(ToolOutput),
    PendingPlanAlias(PendingPlanAlias),
    Delimited(Vec<String>),
    SvgImage {
        path: String,
        suffix: Option<(String, String)>,
    },
    PdfImage {
        path: String,
        page: u64,
        suffix: Option<(String, String)>,
    },
    Video {
        path: String,
        selector: Option<String>,
        suffix: Option<(String, String)>,
    },
    Plain {
        path: String,
        selector: Selector,
        suffix: Option<(String, String)>,
    },
}

struct BufferedPlainText {
    bytes: Vec<u8>,
    stripped: String,
    normalized: String,
}

struct Window {
    origin: WindowOrigin,
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
    /// Complete undecorated emitted rows; gaps never invent closed conflicts.
    conflict_lines: Vec<(usize, String)>,
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
    /// In-memory ACP buffers retain the file owner in continuation messages.
    entity_label: Option<&'a str>,
    /// Only `skill://` resources bypass the normal result caps. In-memory
    /// host resources are text but remain capped (OMP read.ts:2373).
    ignore_result_limits: bool,
    block_context: Option<(&'a str, &'a Path)>,
    /// Ordinary disk default; ACP/resources retain only their selector limit.
    default_limit: Option<f64>,
}

/// Stream the requested line window from `reader` (a file or in-memory
/// bytes of `size`); blocking, checks `cancel` while scanning.
fn read_window<R: BufRead + Seek>(
    reader: R,
    size: u64,
    display: &str,
    sel: &Selector,
    render: ReadRender<'_>,
    cancel: &CancellationToken,
) -> Result<Window, String> {
    read_window_with_scan_limit::<_, MAX_SCAN_BYTES>(reader, size, display, sel, render, cancel)
}

fn read_window_with_scan_limit<R: BufRead + Seek, const SCAN_LIMIT: u64>(
    mut reader: R,
    size: u64,
    display: &str,
    sel: &Selector,
    render: ReadRender<'_>,
    cancel: &CancellationToken,
) -> Result<Window, String> {
    let ReadRender {
        numbering,
        text_resource,
        ..
    } = render;
    let entity = render
        .entity_label
        .unwrap_or(if text_resource { "resource" } else { "file" });
    let limits = !render.ignore_result_limits;
    // Buffered plain disk collectors budget original source bytes, before lossy decoding
    // or line prefixes. LF separates lines; CR and BOM remain source content.
    let source_bytes =
        render.default_limit.is_some() && size <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES;
    let io = |e: std::io::Error| format!("Cannot read {display}: {e}");
    let mut head = vec![0u8; SNIFF_BYTES];
    let n = read_up_to(&mut reader, &mut head).map_err(io)?;
    head.truncate(n);
    if !sel.raw && !text_resource && sniff_binary(&head) {
        return Ok(Window {
            origin: WindowOrigin::BinaryRefusal,
            text: format!(
                "[Cannot read binary file '{display}' ({}); not valid UTF-8 text. Use ':raw' to read bytes verbatim.]",
                format_bytes(size)
            ),
            details: json!({"fileSize": size}),
            emitted: 0,
            start: 1,
            oversized_first_line: None,
            raw_seen_lines: None,
            conflict_lines: Vec::new(),
        });
    }
    reader.rewind().map_err(io)?;
    if !sel.multi_ranges.is_empty() {
        return read_multi_window::<_, SCAN_LIMIT>(reader, size, display, sel, render, cancel);
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
            if !text_resource && scanned > SCAN_LIMIT {
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
    let mut start = if !sel.raw && requested_start > 1 {
        requested_start - 1
    } else {
        requested_start
    };
    let end = requested_end.map(|end| if sel.raw { end } else { end.saturating_add(3) });

    // Fixed ordinary disk collector compares its integer count to the original
    // binary64 bound. Fractional settings therefore collect ceil, and NaN does
    // not become an arbitrary integer/default. ACP/resource builders skip this.
    let selected_budget = render.default_limit.and_then(|default| match sel.range {
        None => Some(default),
        Some(Range::From(begin, end)) => {
            let effective = end.map_or(default, |end| {
                end.saturating_sub(begin).saturating_add(1) as f64
            });
            Some(
                effective
                    + if !sel.raw && begin > 1 { 1.0 } else { 0.0 }
                    + if !sel.raw && end.is_some() { 3.0 } else { 0.0 },
            )
        }
        Some(Range::Tail(_)) => None,
    });
    let max_collect_lines = selected_budget.map_or(DEFAULT_MAX_LINES as f64, |limit| {
        if limit.is_nan() {
            f64::NAN
        } else {
            limit.min(DEFAULT_MAX_LINES as f64)
        }
    });
    let max_collect_bytes = selected_budget.map_or(DEFAULT_MAX_BYTES as f64, |_| {
        if max_collect_lines.is_nan() {
            f64::NAN
        } else {
            (max_collect_lines * 512.0).max(DEFAULT_MAX_BYTES as f64)
        }
    });
    let byte_preview_cap = if max_collect_bytes.is_nan() {
        usize::MAX
    } else {
        max_collect_bytes as usize
    };

    let mut out = String::new();
    let mut resource_bytes = 0usize;
    let mut conflict_lines = Vec::new();
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
        let in_window =
            line_no >= start && end.is_none_or(|e| line_no <= e) && truncated_by.is_none();
        if !in_window {
            if line_no > start || truncated_by.is_some() {
                scanned_after += k as u64;
                if !text_resource && scanned_after > SCAN_LIMIT {
                    break;
                }
            }
            continue;
        }
        let raw_line = String::from_utf8_lossy(&buf);
        let line = raw_line.strip_suffix('\n').unwrap_or(&raw_line);
        let line = line
            .strip_suffix('\r')
            .filter(|_| !source_bytes && !sel.raw && !text_resource)
            .unwrap_or(line);
        let rendered = match numbering {
            _ if sel.raw => line.to_string(),
            Numbering::Pipe => format!("{line_no}|{line}"),
            Numbering::Hashline => format!("{line_no}:{line}"),
            Numbering::None => line.to_string(),
        };
        // OMP's in-memory builder truncates selected text before adding line
        // display prefixes. Preserve the existing filesystem budget here.
        let line_bytes = if source_bytes {
            buf.len() - usize::from(buf.last() == Some(&b'\n'))
        } else if text_resource {
            line.len()
        } else {
            rendered.len() + 1
        };
        let projected_bytes = if source_bytes || text_resource {
            resource_bytes + usize::from(emitted > 0) + line_bytes
        } else {
            out.len() + line_bytes
        };
        if limits && line_no < requested_start && line_bytes as f64 > max_collect_bytes {
            // Context must not consume the entire budget before the requested line.
            start = requested_start;
            continue;
        }
        if limits
            && line_no == requested_start
            && emitted > 0
            && projected_bytes as f64 > max_collect_bytes
        {
            out.clear();
            conflict_lines.clear();
            emitted = 0;
            resource_bytes = 0;
            start = requested_start;
        }
        if limits && emitted as f64 >= max_collect_lines {
            truncated_by = Some("lines");
            continue;
        }
        let projected_bytes = if source_bytes || text_resource {
            resource_bytes + usize::from(emitted > 0) + line_bytes
        } else {
            out.len() + line_bytes
        };
        if limits && projected_bytes as f64 > max_collect_bytes {
            if emitted == 0 {
                // A single line larger than the cap: bounded preview (OMP firstLineExceedsLimit).
                if source_bytes {
                    let original = buf.strip_suffix(b"\n").unwrap_or(&buf);
                    let preview =
                        String::from_utf8_lossy(&original[..original.len().min(byte_preview_cap)]);
                    let preview = match numbering {
                        _ if sel.raw => preview.into_owned(),
                        Numbering::Pipe => format!("{line_no}|{preview}"),
                        Numbering::Hashline => format!("{line_no}:{preview}"),
                        Numbering::None => preview.into_owned(),
                    };
                    out.push_str(&preview);
                } else if text_resource {
                    // In-memory resources budget the source before adding
                    // display prefixes (fixed read-format.ts firstLineExceedsLimit).
                    let snippet = cut_at_char_boundary(line, byte_preview_cap);
                    let preview = match numbering {
                        _ if sel.raw => snippet.to_owned(),
                        Numbering::Pipe => format!("{line_no}|{snippet}"),
                        Numbering::Hashline => format!("{line_no}:{snippet}"),
                        Numbering::None => snippet.to_owned(),
                    };
                    out.push_str(&preview);
                } else {
                    out.push_str(cut_at_char_boundary(&rendered, byte_preview_cap));
                }
                emitted = 1;
                oversized_line = Some((
                    line_no,
                    if source_bytes {
                        line_bytes
                    } else if text_resource {
                        line.len()
                    } else {
                        buf.len()
                    },
                ));
            }
            truncated_by = Some("bytes");
            continue;
        }
        if emitted > 0 {
            out.push('\n');
        }
        out.push_str(&rendered);
        conflict_lines.push((line_no, line.to_owned()));
        resource_bytes = projected_bytes;
        emitted += 1;
    }
    let total = if reached_eof { Some(line_no) } else { None };
    if !requested_seen && requested_start > line_no {
        let Some(total) = total else {
            return Err(format!("Cannot read {display}: scan budget exceeded"));
        };
        let suggestion = if total == 0 {
            format!("The {entity} is empty.")
        } else {
            format!("Use :1 to read from the start, or :{total} to read the last line.")
        };
        return Ok(Window {
            origin: WindowOrigin::SingleRangeEof,
            text: format!(
                "Line {requested_start} is beyond end of {} ({total} lines total). {suggestion}",
                entity
            ),
            details: json!({"totalLines": total, "fileSize": size}),
            emitted: 0,
            start: requested_start,
            oversized_first_line: None,
            raw_seen_lines: None,
            conflict_lines: Vec::new(),
        });
    }
    if emitted == 0 {
        let Some(total) = total else {
            return Err(format!("Cannot read {display}: scan budget exceeded"));
        };
        if total == 0 && sel.range.is_none() {
            return Ok(Window {
                origin: WindowOrigin::SingleRangeEof,
                text: if text_resource {
                    format!(
                        "Line 1 is beyond end of {entity} (0 lines total). The {entity} is empty."
                    )
                } else {
                    String::new()
                },
                details: json!({"totalLines": 0, "fileSize": size}),
                emitted: 0,
                start,
                oversized_first_line: None,
                raw_seen_lines: None,
                conflict_lines: Vec::new(),
            });
        }
        let suggestion = if total == 0 {
            format!("The {entity} is empty.")
        } else {
            format!("Use :1 to read from the start, or :{total} to read the last line.")
        };
        return Ok(Window {
            origin: WindowOrigin::SingleRangeEof,
            text: format!(
                "Line {start} is beyond end of {} ({total} lines total). {suggestion}",
                entity
            ),
            details: json!({"totalLines": total, "fileSize": size}),
            emitted: 0,
            start,
            oversized_first_line: None,
            raw_seen_lines: None,
            conflict_lines: Vec::new(),
        });
    }
    let last_shown = start + emitted - 1;
    let hashline_oversized =
        numbering == Numbering::Hashline && !sel.raw && oversized_line.is_some();
    if let Some((n, bytes)) = oversized_line.filter(|_| hashline_oversized) {
        // Hashline anchors need whole lines: no editable preview (upstream
        // text); the continuation notice below still names the next line.
        out = format!(
            "[Line {n} is {}, exceeds {} limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]",
            format_bytes(bytes as u64),
            format_bytes(byte_preview_cap as u64)
        );
    } else if let Some((n, bytes)) = oversized_line {
        out.push_str(&format!(
            "\n\n[Line {n} is {}, exceeds {} limit. Showing its first {}.]",
            format_bytes(bytes as u64),
            format_bytes(byte_preview_cap as u64),
            format_bytes(byte_preview_cap as u64)
        ));
    }
    let more_after = match total {
        Some(t) => t > last_shown,
        None => true,
    };
    // Fixed ordinary-disk raw bounded ranges take the truncation-metadata
    // branch, leaving the body verbatim. In-memory resource ranges use a
    // different builder and retain their existing continuation notice.
    let raw_range_truncated = !text_resource
        && sel.raw
        && matches!(sel.range, Some(Range::From(_, Some(_))))
        && total.is_some()
        && more_after
        && truncated_by.is_none()
        && oversized_line.is_none();
    if !raw_range_truncated
        && more_after
        && (truncated_by.is_some() || sel.range.is_some() || total.is_none())
    {
        match total {
            Some(t) => out.push_str(&format!(
                "\n\n[{} more lines in {}. Use :{} to continue]",
                t - last_shown,
                entity,
                last_shown + 1
            )),
            None => out.push_str(&format!(
                "\n\n[More lines in {} ({} total; not scanned to EOF). Use :{} to continue]",
                entity,
                format_bytes(size),
                last_shown + 1
            )),
        }
    }
    let mut details = json!({"fileSize": size});
    if let Some(t) = total {
        details["totalLines"] = json!(t);
    }
    if raw_range_truncated {
        details["truncation"] = json!({
            "content": out, "truncated": true, "truncatedBy": "lines",
            "totalLines": total.unwrap().saturating_sub(start - 1),
            "totalBytes": if source_bytes {resource_bytes} else {out.len()}, "outputLines": emitted,
            "outputBytes": if source_bytes {resource_bytes} else {out.len()},
            "lastLinePartial": false, "firstLineExceedsLimit": false,
        });
    }
    if let Some(by) = truncated_by {
        let output_lines = if hashline_oversized { 0 } else { emitted };
        details["truncation"] =
            json!({"truncated": true, "truncatedBy": by, "outputLines": output_lines});
    }
    Ok(Window {
        origin: WindowOrigin::Complete,
        text: out,
        details,
        emitted,
        start,
        oversized_first_line: oversized_line,
        raw_seen_lines: None,
        conflict_lines,
    })
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
    path: Option<&Path>,
    selected: &[u32],
    numbering: Numbering,
    max_bytes: usize,
    max_lines: usize,
) -> Option<(String, Vec<u32>)> {
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
    let path = path.and_then(Path::to_str);
    let source = ara_edit::diff_string::BlockContextSource {
        path,
        lang: None,
        streaming: false,
    };
    let mut added = false;
    for (line, content) in
        ara_edit::diff_string::find_block_context_lines(&lines, selected, &source)
    {
        if rows.contains_key(&line)
            || line == 0
            || line as usize > lines.len()
            || rows.len() >= max_lines
        {
            continue;
        }
        let row = numbered_multi_row(line, &content, numbering);
        let before = rows.range(..line).next_back().map(|(&number, _)| number);
        let after = rows.range(line..).next().map(|(&number, _)| number);
        let old_separator = before
            .zip(after)
            .map_or(0, |(a, b)| multi_separator(a, b).len());
        let new_separators = before.map_or(0, |a| multi_separator(a, line).len())
            + after.map_or(0, |b| multi_separator(line, b).len());
        let projected = used_bytes
            .saturating_add(row.len())
            .saturating_add(new_separators)
            .saturating_sub(old_separator);
        if projected > max_bytes {
            continue;
        }
        rows.insert(line, row);
        used_bytes = projected;
        added = true;
    }
    added.then(|| (render_multi_rows(&rows), rows.keys().copied().collect()))
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
    let ReadRender {
        numbering,
        entity_label,
        text_resource,
        ignore_result_limits,
        block_context,
        default_limit,
    } = render;
    // Fixed disk multi ranges have a per-span limit, not a combined shared
    // 3000-line/50KiB cap. ACP/internal retain their distinct builder owner.
    let ignore_result_limits = ignore_result_limits || default_limit.is_some();
    let mut span_bytes = 0usize;
    let mut span_emitted = 0usize;
    let mut span_byte_stopped = false;
    let io = |e: std::io::Error| format!("Cannot read {display}: {e}");
    let entity = entity_label.unwrap_or(if text_resource { "resource" } else { "file" });
    let mut out = String::new();
    let mut seen = Vec::new();
    let mut conflict_lines = Vec::new();
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
            span_bytes = 0;
            span_emitted = 0;
            span_byte_stopped = false;
        }
        if let Some(Range::From(start, end)) = sel.multi_ranges.get(range_index)
            && total >= *start
            && end.is_none_or(|end| total <= end)
            && default_limit.is_none_or(|default| {
                let requested = end.map_or(default, |end| {
                    end.saturating_sub(*start).saturating_add(1) as f64
                });
                let bounded = if requested.is_nan() {
                    f64::NAN
                } else {
                    requested.min(DEFAULT_MAX_LINES as f64)
                };
                if !sel.raw && !text_resource && block_context.is_some() {
                    // Array.slice(ToIntegerOrInfinity): NaN is zero; fractions floor.
                    !bounded.is_nan() && total.saturating_sub(*start) < bounded.floor() as usize
                } else {
                    // collectLineWindowFromBuffer/streamLinesFromFile counters.
                    !((total.saturating_sub(*start) as f64) >= bounded)
                }
            })
            && truncated_by.is_none()
        {
            let raw_line = String::from_utf8_lossy(&buf);
            let line = raw_line.strip_suffix('\n').unwrap_or(&raw_line);
            let line = line
                .strip_suffix('\r')
                .filter(|_| !sel.raw && !text_resource)
                .unwrap_or(line);
            if let Some(default) = default_limit {
                if sel.raw || block_context.is_none() {
                    // Buffered nonraw uses Array.slice with no byte collector;
                    // raw/stream collect source bytes independently per span.
                    let requested = end.map_or(default, |end| {
                        end.saturating_sub(*start).saturating_add(1) as f64
                    });
                    let max_lines = if requested.is_nan() {
                        f64::NAN
                    } else {
                        requested.min(DEFAULT_MAX_LINES as f64)
                    };
                    let max_bytes = if max_lines.is_nan() {
                        f64::NAN
                    } else {
                        (max_lines * 512.0).max(DEFAULT_MAX_BYTES as f64)
                    };
                    let projected = span_bytes + usize::from(span_emitted > 0) + line.len();
                    if span_byte_stopped || projected as f64 > max_bytes {
                        span_byte_stopped = true;
                        if bytes == 0 {
                            break;
                        }
                        continue;
                    }
                    span_bytes = projected;
                    span_emitted += 1;
                }
            }
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
            if !ignore_result_limits && seen.len() >= DEFAULT_MAX_LINES {
                truncated_by = Some("lines");
            } else if !ignore_result_limits && out.len() + extra > DEFAULT_MAX_BYTES {
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
                conflict_lines.push((total, line.to_owned()));
                seen.push(total as u32);
                last_emitted = total;
                prior_range = Some(range_index);
            }
        }
        if bytes == 0 {
            break;
        }
        // A late requested span is still reachable, as with a single-range
        // read. Only ordinary files bound the post-selection scan; immutable
        // Skill resources have already been loaded in full, as in fixed OMP.
        if truncated_by.is_some() || range_index >= sel.multi_ranges.len() {
            scanned_after += bytes as u64;
            if !text_resource && scanned_after > POST_SCAN_LIMIT {
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
                let notice = format!(
                    "[Range {bound} is beyond end of {entity} ({total} lines total); skipped]"
                );
                if ignore_result_limits
                    || out.len()
                        + notices.len()
                        + usize::from(!out.is_empty() || !notices.is_empty())
                        + notice.len()
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
            notices.push_str(&format!(
                "\n[{omitted} additional out-of-bounds ranges omitted]"
            ));
        }
    }
    if !sel.raw
        && let Some((full_text, path)) = block_context
    {
        if cancel.is_cancelled() {
            return Err(format!("Read of {display} was aborted"));
        }
        let (max_bytes, max_lines) = if ignore_result_limits {
            (usize::MAX, usize::MAX)
        } else {
            (
                DEFAULT_MAX_BYTES.saturating_sub(notices.len()),
                DEFAULT_MAX_LINES,
            )
        };
        if let Some((rendered, context_lines)) = multi_block_context(
            full_text,
            Some(path),
            &seen,
            numbering,
            max_bytes,
            max_lines,
        ) {
            out = rendered;
            emitted = context_lines.len();
            let full_lines = full_text.split_terminator('\n').collect::<Vec<_>>();
            conflict_lines = context_lines
                .iter()
                .map(|&n| (n as usize, full_lines[n as usize - 1].to_owned()))
                .collect();
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
                Range::From(start, end) if end.is_none_or(|end| end >= after) => {
                    Some((*start).max(after))
                }
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
        details["truncation"] =
            json!({"truncated": true, "truncatedBy": by, "outputLines": emitted});
    }
    Ok(Window {
        origin: WindowOrigin::MultiRange,
        text: out,
        details,
        emitted,
        start: seen.first().copied().unwrap_or(1) as usize,
        oversized_first_line: None,
        raw_seen_lines: sel.raw.then_some(seen),
        conflict_lines,
    })
}

/// Fixed truncateLine uses UTF16 length, then slice truncation toward zero.
/// String cannot retain a sliced lone surrogate; that full-READ boundary stays
/// explicitly unsupported rather than silently counting Unicode scalars.
fn column_display_line(line: &str, columns: f64) -> Result<(String, bool), String> {
    if columns <= 0.0 || !columns.is_finite() || line.encode_utf16().count() as f64 <= columns {
        return Ok((line.to_owned(), false));
    }
    let units = line
        .encode_utf16()
        .take(columns.trunc() as usize)
        .collect::<Vec<_>>();
    let prefix=String::from_utf16(&units).map_err(|_|"READ column cap splits a UTF-16 surrogate pair; this Host String transport cannot preserve the fixed output. Set tools.outputMaxColumns to 0 or a boundary that keeps the pair.".to_owned())?;
    Ok((format!("{prefix}…"), true))
}

/// Source rows, source budget and notices are immutable; only clone the body.
fn cap_window_display(
    window: &mut Window,
    numbering: Numbering,
    columns: f64,
) -> Result<(), String> {
    if columns <= 0.0
        || !columns.is_finite()
        || window.conflict_lines.is_empty()
        || window.oversized_first_line.is_some()
    {
        return Ok(());
    }
    let original = window
        .conflict_lines
        .iter()
        .map(|(line, text)| {
            (
                *line as u32,
                numbered_multi_row(*line as u32, text, numbering),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let original_body = render_multi_rows(&original);
    let mut changed = false;
    let mut display = BTreeMap::new();
    for (line, text) in &window.conflict_lines {
        let (text, truncated) = column_display_line(text, columns)?;
        changed |= truncated;
        display.insert(
            *line as u32,
            numbered_multi_row(*line as u32, &text, numbering),
        );
    }
    if !changed {
        return Ok(());
    }
    let suffix=window.text.strip_prefix(&original_body).ok_or_else(||"Cannot apply READ display column cap: source row body does not match its recorded window".to_owned())?;
    window.text = format!("{}{suffix}", render_multi_rows(&display));
    window.details["meta"]["limits"]["columnTruncated"] =
        json!({"maxColumn":ara_prompt::js::number(columns)});
    Ok(())
}

pub struct ReadTool {
    pub ctx: ToolContext,
}

/// A Host may retain each part's original receiver/execute and result Graph.
/// The Core reports the authoritative parts at the fixed routing position;
/// the Host owns invocation, continuation, aggregation and original failures.
#[async_trait]
pub trait ReadDelimitedDispatcher: Send + Sync {
    async fn read_parts(
        &self,
        parts: Vec<String>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError>;
    /// Only owned native values cross this issuing READ effect. The graph
    /// driver consumes its exact receipt before adopting the typed projection.
    async fn publish_in_memory_output(&self, _output: ara_rpc::WireValue) -> Result<(), ToolError> {
        Ok(())
    }
    /// Fixed profiler optional-field failures are TypeError, not ToolError.
    async fn publish_profile_type_error(&self, _message: String) -> Result<(), ToolError> {
        Ok(())
    }
}

/// Live current plan reference snapshot issued by the Session Host. Absence
/// is authoritative; lookup never creates artifacts or initiates approval.
pub enum ReadPlanReferenceSnapshot {
    Absent,
    Path(std::path::PathBuf),
    Uri {
        authored: String,
        native_local_fallback: Option<std::path::PathBuf>,
    },
}
pub trait ReadPlanReferencePort: Send + Sync {
    fn read_reference_path(&self) -> Option<std::path::PathBuf>;
    fn read_reference_snapshot(&self) -> Result<ReadPlanReferenceSnapshot, ToolError> {
        Ok(self.read_reference_path().map_or(
            ReadPlanReferenceSnapshot::Absent,
            ReadPlanReferenceSnapshot::Path,
        ))
    }
}
enum PlanAliasTail {
    Pdf(u64),
    Svg,
    Video(Option<String>),
    Plain(Selector),
}
struct PendingPlanAlias {
    path: String,
    absolute: std::path::PathBuf,
    authored: String,
    suffix: Option<(String, String)>,
    reference: ReadPlanReferenceSnapshot,
    port: Option<Arc<dyn crate::ContentUriPort>>,
    tail: PlanAliasTail,
}
enum ReadPathRecovery {
    Continue,
    Delimited(Vec<String>),
    Pending(
        ReadPlanReferenceSnapshot,
        Option<Arc<dyn crate::ContentUriPort>>,
    ),
}
fn read_missing_split(ctx: &ToolContext, authored: &str) -> Result<Option<Vec<String>>, ToolError> {
    let routed = |entry: &str| read_path_is_routed(ctx, entry);
    crate::paths::split_delimited_entry_with_routed(
        ctx,
        authored,
        crate::paths::Splitter::Search,
        Some(&routed),
    )
    .map_err(ToolError)
}
fn finish_plan_alias_tail(
    ctx: &ToolContext,
    path: String,
    suffix: Option<(String, String)>,
    tail: PlanAliasTail,
) -> Result<ReadDispatch, ToolError> {
    Ok(match tail {
        PlanAliasTail::Pdf(page) => ReadDispatch::PdfImage { path, page, suffix },
        PlanAliasTail::Svg => {
            let absolute = ctx.resolve(&path);
            if !absolute.extension().is_some_and(|extension| {
                extension.eq_ignore_ascii_case("svg") || extension.eq_ignore_ascii_case("svgz")
            }) {
                return Err(ToolError(
                    "The ':img' selector only supports local .svg and .svgz files.".into(),
                ));
            }
            ReadDispatch::SvgImage { path, suffix }
        }
        PlanAliasTail::Video(selector) => {
            if std::fs::metadata(ctx.resolve(&path)).is_ok_and(|meta| meta.is_dir()) {
                let selector = if selector.as_deref().is_none_or(|value| {
                    crate::read_video::parse_video_selector(Some(value)).is_some()
                }) {
                    Selector::default()
                } else {
                    content_selector(selector.as_deref())?
                };
                ReadDispatch::Plain {
                    path,
                    selector,
                    suffix,
                }
            } else {
                ReadDispatch::Video {
                    path,
                    selector,
                    suffix,
                }
            }
        }
        PlanAliasTail::Plain(selector) => ReadDispatch::Plain {
            path,
            selector,
            suffix,
        },
    })
}
fn read_recovery_dispatch(
    ctx: &ToolContext,
    authored: &str,
    path: String,
    suffix: Option<(String, String)>,
    tail: PlanAliasTail,
    recovery: ReadPathRecovery,
) -> Result<ReadDispatch, ToolError> {
    match recovery {
        ReadPathRecovery::Continue => finish_plan_alias_tail(ctx, path, suffix, tail),
        ReadPathRecovery::Delimited(parts) => Ok(ReadDispatch::Delimited(parts)),
        ReadPathRecovery::Pending(reference, port) => {
            Ok(ReadDispatch::PendingPlanAlias(PendingPlanAlias {
                absolute: ctx.resolve(&path),
                path,
                authored: authored.into(),
                suffix,
                reference,
                port,
                tail,
            }))
        }
    }
}

fn read_path_is_routed(ctx: &ToolContext, entry: &str) -> bool {
    if let Some(port) = ctx.content_uri_port() {
        match port.route(&crate::internal_urls::split_content_url_selector(entry).0) {
            crate::ContentUriRoute::Registered { .. } => return true,
            crate::ContentUriRoute::Removed => return false,
            crate::ContentUriRoute::Unregistered => {}
        }
    }
    crate::internal_urls::is_internal_url(entry)
}

/// Called only after literal/archive/SQLite and media classification. Share
/// the original READ's memo: explicit semicolon scope precedes suffix lookup;
/// other delimiter recovery follows a suffix miss (fixed read.ts1402-1447).
fn recover_missing_read_path(
    ctx: &ToolContext,
    authored: &str,
    path: &mut String,
    suffix: &mut Option<(String, String)>,
    memo: &mut crate::read_sqlite::ScopedSuffixMemo<'_>,
) -> Result<ReadPathRecovery, ToolError> {
    let absolute = ctx.resolve(path);
    if !std::fs::metadata(&absolute).is_err_and(|error| crate::paths::is_missing(&error)) {
        return Ok(ReadPathRecovery::Continue);
    }
    if authored.contains(';')
        && let Some(parts) = read_missing_split(ctx, authored)?
    {
        return Ok(ReadPathRecovery::Delimited(parts));
    }
    let remote = ctx
        .read_remote_mount_root()
        .is_some_and(|root| absolute.starts_with(root) && absolute != root);
    if !remote
        && let Some((found, display)) = memo.find(path)?
        && std::fs::metadata(&found).is_ok()
    {
        let resolved = ctx.resolve(&display).to_string_lossy().into_owned();
        *suffix = Some((std::mem::replace(path, resolved), display));
        return Ok(ReadPathRecovery::Continue);
    }
    // Fixed b91db66e read1141: single getter BEFORE parent qualification; only
    // the later async locator is caught. No mutable reference is read on resume.
    let reference = match ctx.read_plan_reference_port() {
        Some(port) => port.read_reference_snapshot()?,
        None => ReadPlanReferenceSnapshot::Absent,
    };
    if absolute.parent() == Some(ctx.cwd.as_path()) {
        match reference {
            ReadPlanReferenceSnapshot::Path(reference) => {
                if absolute.file_name() == reference.file_name()
                    && std::fs::metadata(&reference).is_ok()
                {
                    *path = reference.to_string_lossy().into_owned();
                    return Ok(ReadPathRecovery::Continue);
                }
            }
            ReadPlanReferenceSnapshot::Uri {
                authored,
                native_local_fallback,
            } => {
                // Fixed read1141-1142: an empty current reference is absent.
                if !authored.is_empty() {
                    return Ok(ReadPathRecovery::Pending(
                        ReadPlanReferenceSnapshot::Uri {
                            authored,
                            native_local_fallback,
                        },
                        ctx.content_uri_port(),
                    ));
                }
            }
            ReadPlanReferenceSnapshot::Absent => {}
        }
    }
    Ok(read_missing_split(ctx, authored)?
        .map_or(ReadPathRecovery::Continue, ReadPathRecovery::Delimited))
}

fn content_selector(selector: Option<&str>) -> Result<Selector, ToolError> {
    let Some(selector) = selector else {
        return Ok(Selector::default());
    };
    if selector.eq_ignore_ascii_case("raw") {
        return Ok(Selector {
            raw: true,
            ..Selector::default()
        });
    }
    if selector.eq_ignore_ascii_case("conflicts") {
        // In-memory resources do not have the filesystem conflict handler;
        // upstream's in-memory builder treats this as a whole-resource read.
        return Ok(Selector::default());
    }
    if selector.eq_ignore_ascii_case("img") {
        return Err(ToolError(
            "The ':img' selector only supports local .svg and .svgz files.".into(),
        ));
    }
    let chunks: Vec<&str> = selector.split(':').collect();
    let (range, raw) = match chunks.as_slice() {
        [range] => (*range, false),
        [raw, range] if raw.eq_ignore_ascii_case("raw") => (*range, true),
        [range, raw] if raw.eq_ignore_ascii_case("raw") => (*range, true),
        _ => ("", false),
    };
    let Some(mut ranges) = parse_ranges(range).filter(|ranges| !ranges.is_empty()) else {
        return Err(ToolError(format!(
            "Invalid selector ':{selector}'. Use :N, :N-M, :N+K, :N- (open-ended), :-N (last N lines), a comma-separated list of ranges, :raw, :img for SVG rendering, or a range combined with raw (e.g. :raw:50-100)."
        )));
    };
    Ok(if ranges.len() == 1 {
        Selector {
            range: ranges.pop(),
            multi_ranges: vec![],
            raw,
        }
    } else {
        Selector {
            range: None,
            multi_ranges: ranges,
            raw,
        }
    })
}

impl ReadTool {
    async fn read_content_uri(
        &self,
        port: std::sync::Arc<dyn crate::ContentUriPort>,
        input: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let (url, selector) = crate::internal_urls::split_content_url_selector(input);
        if cancel.is_cancelled() {
            return Err(ToolError(format!("Host URI read for {url} was aborted")));
        }
        let file_source = port.read_file(&url, cancel.clone()).await?;
        if crate::internal_urls::hierarchical_scheme(&url)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("local"))
            && let Some(source) = &file_source
        {
            let svg = selector
                .as_deref()
                .is_some_and(|selector| selector.eq_ignore_ascii_case("img"));
            if svg {
                if !source.path.extension().is_some_and(|extension| {
                    extension.eq_ignore_ascii_case("svg") || extension.eq_ignore_ascii_case("svgz")
                }) {
                    return Err(ToolError(
                        "The ':img' selector only supports local .svg and .svgz files.".into(),
                    ));
                }
                let inspect = format!("{}:img", self.ctx.display(&source.path));
                return self
                    .read_image_content(
                        &source.path,
                        crate::read_image::ReadImageKind::Svg,
                        None,
                        &inspect,
                        cancel,
                    )
                    .await?
                    .ok_or_else(|| {
                        ToolError("The ':img' selector only supports .svg and .svgz files.".into())
                    });
            }
            if crate::read_video::is_video_path(&source.path.to_string_lossy()) {
                return self
                    .read_video_content(&source.path, selector.as_deref(), cancel)
                    .await;
            }
            let image_selector = content_selector(selector.as_deref())?;
            if !image_selector.raw
                && let Some(metadata) = crate::read_image::read_image_metadata(&source.path).await?
            {
                if !image_selector.multi_ranges.is_empty() {
                    return Err(ToolError(format!(
                        "Multiple line ranges require a text file: {} is an image",
                        self.ctx.display(&source.path)
                    )));
                }
                return self
                    .read_image_content(
                        &source.path,
                        crate::read_image::ReadImageKind::Raster,
                        Some(&metadata.mime_type),
                        &self.ctx.display(&source.path),
                        cancel,
                    )
                    .await?
                    .ok_or_else(|| {
                        ToolError("Read image file failed: unsupported image format.".into())
                    });
            }
        }
        let sel = content_selector(selector.as_deref())?;
        let (resource, source_path) = match file_source {
            Some(source) if source.immutable => {
                return self.read_file_content_uri(source, url, sel, cancel).await;
            }
            Some(source) => {
                let bytes = tokio::fs::read(&source.path)
                    .await
                    .map_err(|error| ToolError(format!("Cannot read {url}: {error}")))?;
                (
                    crate::UriResource {
                        content: String::from_utf8_lossy(&bytes).into_owned(),
                        content_type: source.content_type,
                        notes: Vec::new(),
                        immutable: false,
                    },
                    Some(source.path),
                )
            }
            None => (port.read(&url, cancel.clone()).await?, None),
        };
        // Fixed multi-range in-memory reads do not truncate either; the
        // ordinary single-range resource caps remain scheme-specific.
        let ignore_result_limits = !sel.multi_ranges.is_empty()
            || crate::internal_urls::hierarchical_scheme(&url)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("skill"));
        let numbering = if sel.raw {
            Numbering::None
        } else if self.ctx.hashlines() && !resource.immutable {
            // A mutable URI receives hashline rows. Only a native authorized
            // sourcePath below can also mint a tag and an EditStore snapshot.
            Numbering::Hashline
        } else if self.ctx.line_numbers {
            Numbering::Pipe
        } else {
            Numbering::None
        };
        let display = url.clone();
        let ctx = self.ctx.clone();
        crate::run_blocking("Read", cancel, move |work| {
            if work.is_cancelled() {
                return Err(ToolError(format!("Read of {url} was aborted")));
            }
            let mut content = resource.content;
            if source_path.is_some() && numbering == Numbering::Hashline {
                content = ara_edit::text::normalize_to_lf(ara_edit::text::strip_bom(&content).1)
                    .into_owned();
            }
            let size = content.len() as u64;
            // Raw addresses `split("\n")`, including the final empty segment.
            if sel.raw
                && sel.multi_ranges.is_empty()
                && (content.is_empty() || content.ends_with('\n'))
            {
                content.push('\n');
            }
            let mut window = read_window(
                std::io::Cursor::new(content.as_bytes()),
                size,
                &display,
                &sel,
                ReadRender {
                    default_limit: None,
                    numbering,
                    entity_label: None,
                    text_resource: true,
                    ignore_result_limits,
                    block_context: None,
                },
                &work,
            )
            .map_err(ToolError)?;
            let lines: Vec<&str> = content.split_terminator('\n').collect();
            let mut display_content = resource_display_content(&lines, &sel, &window, numbering);
            if !sel.raw
                && window.oversized_first_line.is_none()
                && let Some((_, _, numbers)) = &display_content
            {
                let selected: Vec<u32> = numbers
                    .iter()
                    .filter_map(|line| line.as_u64().map(|line| line as u32))
                    .collect();
                // Native paths use syntax context; an ordinary Host resource
                // keeps the lexical bracket fallback without a file identity.
                if let Some((rendered, context_lines)) = multi_block_context(
                    &content,
                    source_path.as_deref(),
                    &selected,
                    numbering,
                    usize::MAX,
                    usize::MAX,
                ) {
                    let original_rows = selected
                        .iter()
                        .map(|&line| {
                            (
                                line,
                                numbered_multi_row(line, lines[line as usize - 1], numbering),
                            )
                        })
                        .collect();
                    let original_body = render_multi_rows(&original_rows);
                    if let Some(notices) = window.text.strip_prefix(&original_body) {
                        window.text = format!("{rendered}{notices}");
                        display_content = resource_rows_display(&lines, &context_lines);
                    }
                }
                if work.is_cancelled() {
                    return Err(ToolError(format!("Read of {url} was aborted")));
                }
            }
            let mut details = window.details;
            // Fixed host resources supply contentType, not a backing file.
            details
                .as_object_mut()
                .expect("read details object")
                .remove("fileSize");
            details["contentType"] = json!(resource.content_type);
            details["meta"] = json!({"source": {"type": "internal", "value": url}});
            if let Some(path) = &source_path {
                details["resolvedPath"] = json!(path.to_string_lossy());
                if window.emitted > 0 && window.oversized_first_line.is_none() {
                    if numbering == Numbering::Hashline
                        && content.len() as u64 <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES
                    {
                        let key = ara_edit::path_policy::canonical_key(path);
                        let tag = ctx.edit_store.record(&key, &content, None);
                        let header = ara_edit::modes::hashline::format::format_hashline_header(
                            &ctx.hashline_display(path),
                            &tag,
                        );
                        window.text = format!("{header}\n{}", window.text);
                        let seen = ara_edit::store::seen_lines_from_body(&window.text);
                        if !seen.is_empty() {
                            ctx.edit_store.record_seen_lines(&key, &tag, &seen);
                        }
                    } else if sel.raw {
                        let seen = window.raw_seen_lines.clone().unwrap_or_else(|| {
                            (window.start..window.start + window.emitted)
                                .map(|n| n as u32)
                                .collect()
                        });
                        ctx.edit_store.record_file(path, Some(&seen));
                    }
                }
            }
            // The host renderer receives the undecorated content separately
            // from numbered model rows (OMP read-format.ts `displayContent`).
            if let Some((start, plain, numbers)) = display_content {
                details["displayContent"] =
                    json!({"text": plain, "startLine": start, "lineNumbers": numbers});
            }
            // Resource notes stay on the port resource; fixed read.ts does not
            // project them into the tool's ReadToolDetails.
            Ok(ToolOutput::text(window.text).with_details(details))
        })
        .await
    }

    async fn read_file_content_uri(
        &self,
        source: crate::UriFileResource,
        url: String,
        selector: Selector,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let line_numbers = self.ctx.line_numbers;
        crate::run_blocking("Read artifact", cancel, move |work| {
            if work.is_cancelled() {
                return Err(ToolError(format!("Read of {url} was aborted")));
            }
            let file = std::fs::File::open(&source.path)
                .map_err(|error| ToolError(format!("Cannot read {url}: {error}")))?;
            let meta = file.metadata().map_err(|error| ToolError(format!("Cannot read {url}: {error}")))?;
            if meta.is_dir() {
                return Err(ToolError(format!("Artifact resolved to a directory, not a file: {url}")));
            }
            let artifact = crate::internal_urls::hierarchical_scheme(&url)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("artifact"));
            if artifact && selector.raw && selector.range.is_none() && selector.multi_ranges.is_empty()
                && meta.len() > DEFAULT_MAX_BYTES as u64 {
                let text = raw_artifact_blocked_notice(&source.path, &url, meta.len());
                return Ok(ToolOutput::text(text).with_details(json!({
                    "resolvedPath": source.path.to_string_lossy(), "contentType": source.content_type,
                    "meta": {"source": {"type": "internal", "value": url}},
                })));
            }
            if !artifact && selector.range.is_none() && selector.multi_ranges.is_empty() && !selector.raw
                && source.max_inline_bytes.is_some_and(|limit| meta.len() > limit)
            {
                return Err(ToolError(format!(
                    "Artifact is {} bytes; full internal resolution is blocked. Use read selectors such as {url}:1-3000 or {url}:raw:1-3000, and use the artifact file path for search/copy workflows: {}",
                    meta.len(), source.path.display()
                )));
            }
            let numbering = if selector.raw || !line_numbers { Numbering::None } else { Numbering::Pipe };
            let window = read_window(
                std::io::BufReader::new(file), meta.len(), &url, &selector,
                ReadRender { default_limit: None, numbering, entity_label: None, text_resource: false, ignore_result_limits: !selector.multi_ranges.is_empty(), block_context: None },
                &work,
            ).map_err(ToolError)?;
            if work.is_cancelled() {
                return Err(ToolError(format!("Read of {url} was aborted")));
            }
            let mut details = window.details;
            details["contentType"] = json!(source.content_type);
            details["resolvedPath"] = json!(source.path.to_string_lossy());
            details["meta"] = json!({"source":{"type":"internal","value":url}});
            let mut text = window.text;
            if artifact && !selector.raw && meta.len() > DEFAULT_MAX_BYTES as u64 {
                text.push_str(&format!("\n\nArtifact storage: {} ({}). Use {url}:N-M to page, {url}:raw:N-M for verbatim chunks, and the artifact file path for search/copy workflows.", shorten_artifact_path(&source.path), format_bytes(meta.len())));
            }
            Ok(ToolOutput::text(text).with_details(details))
        }).await
    }
}

fn shorten_artifact_path_under_home(
    path: &std::path::Path,
    home: Option<&std::path::Path>,
) -> String {
    if let Some(home) = home
        && let Ok(relative) = path.strip_prefix(home)
    {
        if relative.as_os_str().is_empty() {
            return "~".into();
        }
        return format!("~{}{}", std::path::MAIN_SEPARATOR, relative.display());
    }
    path.to_string_lossy().into_owned()
}

fn shorten_artifact_path(path: &std::path::Path) -> String {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from);
    shorten_artifact_path_under_home(path, home.as_deref())
}

fn raw_artifact_blocked_notice(path: &std::path::Path, url: &str, size: u64) -> String {
    format!(
        "Unbounded raw read blocked for {url} ({}). Reading the whole artifact verbatim can exhaust memory. Use {url}:raw:1-3000 for bounded verbatim chunks, {url}:1-3000 for numbered exploration, and the artifact file path for search/copy workflows: {}",
        format_bytes(size),
        shorten_artifact_path(path)
    )
}

// Fixed workspace-tree.ts buildDirectoryTree READ profile only: depth2,
// child12, unlimited root and rendered lines. Reuse the uncached metadata walk;
// this adapter does not port the system-prompt tree or the native parallel pool.
fn read_directory_tree(root: &std::path::Path) -> Result<(String, bool), ToolError> {
    use ara_walk::{CollectOptions, EntryKind, FollowLinks, WalkDetail, WalkOptions};
    use std::collections::HashMap;
    const EXCLUDED: &[&str] = &[
        "node_modules",
        ".git",
        ".next",
        "dist",
        "build",
        "target",
        ".venv",
        ".cache",
        ".turbo",
        ".parcel-cache",
        "coverage",
    ];
    let options = CollectOptions {
        walk: WalkOptions {
            include_hidden: true,
            use_gitignore: false,
            skip_node_modules: true,
            max_depth: 2,
        },
        detail: WalkDetail::Full,
        follow_links: FollowLinks::Never,
        cache: false,
    };
    // The fixed deterministic READ directory call supplies no abort or timeout.
    let budget = ara_walk::Budget {
        cancel: CancellationToken::new(),
        deadline: std::time::Instant::now()
            + std::time::Duration::from_secs(100 * 365 * 24 * 60 * 60),
    };
    // Native listWorkspace resolves a symlinked root before FollowLinks::Never.
    let scan_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let mut entries = match ara_walk::collect_entries(&scan_root, options, &budget) {
        Ok(scan) => scan.entries,
        Err(_) => return Ok(("(empty directory)".into(), false)),
    };
    entries.retain(|entry| {
        let components: Vec<_> = entry.relative.split('/').collect();
        !components.iter().enumerate().any(|(i, name)| {
            *name == ".DS_Store"
                || (EXCLUDED.contains(name)
                    && (i + 1 < components.len() || entry.kind == EntryKind::Dir))
        })
    });
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    entries.dedup_by(|a, b| a.relative == b.relative);
    let mut truncated = entries.len() > 100_000;
    entries.truncate(100_000);
    let mut by_parent: HashMap<String, Vec<ara_walk::CollectedEntry>> = HashMap::new();
    for entry in entries {
        let parent = entry
            .relative
            .rsplit_once('/')
            .map_or("", |(parent, _)| parent)
            .to_owned();
        by_parent.entry(parent).or_default().push(entry);
    }
    let collator = skill_directory_collator()?;
    for entries in by_parent.values_mut() {
        entries.sort_by(|a, b| {
            b.mtime_ms
                .unwrap_or(0.0)
                .total_cmp(&a.mtime_ms.unwrap_or(0.0))
                .then_with(|| {
                    collator.compare(
                        a.relative.rsplit('/').next().unwrap_or(""),
                        b.relative.rsplit('/').next().unwrap_or(""),
                    )
                })
        });
    }
    struct TreeLine {
        label: String,
        size: String,
        age: String,
    }
    fn age(seconds: u64) -> String {
        if seconds == 0 {
            return String::new();
        }
        for (unit, scale) in [
            ("mo", 30 * 24 * 60 * 60),
            ("w", 7 * 24 * 60 * 60),
            ("d", 24 * 60 * 60),
            ("h", 60 * 60),
            ("m", 60),
        ] {
            if seconds >= scale {
                return format!("{}{unit} ago", seconds / scale);
            }
        }
        "just now".into()
    }
    fn render(
        parent: &str,
        depth: usize,
        buckets: &HashMap<String, Vec<ara_walk::CollectedEntry>>,
        now_ms: f64,
        lines: &mut Vec<TreeLine>,
        truncated: &mut bool,
    ) {
        let Some(entries) = buckets.get(parent) else {
            return;
        };
        let limit = if depth == 1 { entries.len() } else { 12 };
        let dropped = entries.len().saturating_sub(limit);
        if dropped > 0 {
            *truncated = true;
        }
        let indexes: Vec<_> = if dropped > 0 {
            (0..11).chain(std::iter::once(entries.len() - 1)).collect()
        } else {
            (0..entries.len()).collect()
        };
        for (slot, index) in indexes.into_iter().enumerate() {
            if dropped > 0 && slot == 11 {
                lines.push(TreeLine {
                    label: format!("{}- … {dropped} more", "  ".repeat(depth)),
                    size: String::new(),
                    age: String::new(),
                });
            }
            let entry = &entries[index];
            let name = entry.relative.rsplit('/').next().unwrap_or("");
            let is_dir = entry.kind == EntryKind::Dir;
            let label = format!(
                "{}- {name}{}",
                "  ".repeat(depth),
                if is_dir { "/" } else { "" }
            );
            let seconds =
                ((now_ms - entry.mtime_ms.unwrap_or(0.0)).max(0.0) / 1000.0).floor() as u64;
            lines.push(TreeLine {
                label,
                size: if is_dir {
                    String::new()
                } else {
                    format_bytes(entry.size.unwrap_or(0))
                },
                age: age(seconds),
            });
            if is_dir {
                render(
                    &entry.relative,
                    depth + 1,
                    buckets,
                    now_ms,
                    lines,
                    truncated,
                );
            }
        }
    }
    let mut lines = vec![TreeLine {
        label: ".".into(),
        size: String::new(),
        age: String::new(),
    }];
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1000.0;
    render("", 1, &by_parent, now, &mut lines, &mut truncated);
    if lines.len() <= 1 {
        return Ok(("(empty directory)".into(), truncated));
    }
    let width = lines
        .iter()
        .map(|line| line.label.encode_utf16().count())
        .max()
        .unwrap_or(0)
        + 2;
    let text = lines
        .into_iter()
        .map(|line| {
            if line.age.is_empty() {
                return line.label;
            }
            format!(
                "{}{}{:<8}  {:<4}",
                line.label,
                " ".repeat(width - line.label.encode_utf16().count()),
                line.size,
                line.age
            )
            .trim_end()
            .to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok((text, truncated))
}

fn directory_listing_window(listing: &str, sel: &Selector) -> String {
    let lines: Vec<_> = listing.split('\n').collect();
    let (start, end) = match sel.range {
        Some(Range::From(start, end)) => (
            start.saturating_sub(1),
            end.unwrap_or(lines.len()).min(lines.len()),
        ),
        Some(Range::Tail(count)) => (lines.len().saturating_sub(count), lines.len()),
        None => return cut_at_char_boundary(listing, DEFAULT_MAX_BYTES).to_owned(),
    };
    if start >= lines.len() {
        return format!(
            "Line {} is beyond end of listing ({} lines total). Use :1 to read from the start, or :{} to read the last line.",
            start + 1,
            lines.len(),
            lines.len()
        );
    }
    let mut text = lines[start..end].join("\n");
    if end < lines.len() {
        text.push_str(&format!(
            "\n\n[{} more lines in listing. Use :{} to continue]",
            lines.len() - end,
            end + 1
        ));
    }
    text
}

fn resource_display_content(
    lines: &[&str],
    sel: &Selector,
    window: &Window,
    numbering: Numbering,
) -> Option<(usize, String, Vec<Value>)> {
    if let Some((line, _)) = window.oversized_first_line {
        if numbering == Numbering::Hashline {
            return None;
        }
        let snippet = cut_at_char_boundary(lines.get(line - 1)?, DEFAULT_MAX_BYTES);
        return Some((line, snippet.to_owned(), vec![json!(line)]));
    }
    if window.emitted == 0 {
        return None;
    }
    if sel.multi_ranges.is_empty() {
        let selected = lines.get(window.start - 1..window.start - 1 + window.emitted)?;
        let plain = selected.join("\n");
        let numbers = if sel.raw && plain.is_empty() {
            vec![]
        } else {
            (window.start..window.start + window.emitted)
                .map(|line| json!(line))
                .collect()
        };
        return Some((window.start, plain, numbers));
    }
    // Fixed raw multi-range results omit displayContent.
    if sel.raw {
        return None;
    }
    let mut selected = Vec::new();
    for range in &sel.multi_ranges {
        let Range::From(start, end) = range else {
            continue;
        };
        for line in *start..=end.unwrap_or(lines.len()).min(lines.len()) {
            selected.push(line as u32);
        }
    }
    resource_rows_display(lines, &selected)
}

fn resource_rows_display(lines: &[&str], selected: &[u32]) -> Option<(usize, String, Vec<Value>)> {
    let first = *selected.first()? as usize;
    let mut plain = String::new();
    let mut numbers = Vec::new();
    let mut previous = None;
    for &line in selected {
        if let Some(before) = previous {
            if line == before + 1 {
                plain.push('\n');
            } else {
                plain.push_str("\n…\n");
                numbers.push(Value::Null);
            }
        }
        plain.push_str(lines.get(line.checked_sub(1)? as usize)?);
        numbers.push(json!(line));
        previous = Some(line);
    }
    Some((first, plain, numbers))
}

#[async_trait]
impl AgentTool for ReadTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "read".into(),
            description: read_description(&self.ctx),
            parameters: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Local path. Inline selectors are supported."}},
                "required": ["path"],
                "additionalProperties": false
            }),
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let path = args
            .get("path")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        self.execute_os_path(path, cancel).await
    }
}

impl ReadTool {
    /// Shared existing filesystem/URI action body; Graph Host inputs retain their original owner.
    /// `path` is the explicit existing OS/string boundary, not a normalized transcript.
    pub async fn execute_os_path(
        &self,
        path: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_path_with_delimited_dispatcher(path, cancel, None)
            .await
    }

    pub async fn execute_os_path_with_delimited_dispatcher(
        &self,
        path: &str,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_os_path_with_selector(path, None, cancel, dispatcher)
            .await
    }

    /// Native uses its authentic extra router before local:// promotion. A
    /// removed Core route does not silently restore the native skill handler.
    pub fn routed_delimited_parts(
        &self,
        path: &str,
        extra_routed: &dyn Fn(&str) -> bool,
    ) -> Result<Option<Vec<String>>, ToolError> {
        let routed = |entry: &str| read_path_is_routed(&self.ctx, entry) || extra_routed(entry);
        if !routed(path) {
            return Ok(None);
        }
        crate::paths::split_delimited_entry_with_routed(
            &self.ctx,
            path,
            crate::paths::Splitter::Search,
            Some(&routed),
        )
        .map_err(ToolError)
    }

    async fn read_delimited_parts(
        &self,
        parts: Vec<String>,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError("Read was aborted".into()));
        }
        if let Some(dispatcher) = dispatcher {
            return dispatcher.read_parts(parts, cancel).await;
        }
        let notice = format!(
            "Note: interpreted as {} paths: {}",
            parts.len(),
            parts.join(", ")
        );
        let mut notes = vec![notice.clone()];
        let mut content = Vec::new();
        let mut display_read_targets = Vec::new();
        let mut pending_text = notice;
        for part in parts {
            if cancel.is_cancelled() {
                return Err(ToolError("Read was aborted".into()));
            }
            let arguments = json!({"path":part}).as_object().unwrap().clone();
            // async_trait supplies the boxed recursion boundary and invokes
            // the existing complete Core execute for every part.
            let result = self
                .execute(
                    "read-delimited-part",
                    arguments,
                    cancel.clone(),
                    Arc::new(|_| {}),
                )
                .await;
            match result {
                Ok(result) => {
                    if cancel.is_cancelled() {
                        return Err(ToolError("Read was aborted".into()));
                    }
                    let display = result
                        .details
                        .as_ref()
                        .and_then(|details| details.get("suffixResolution"))
                        .and_then(|suffix| suffix.get("to"))
                        .and_then(Value::as_str)
                        .unwrap_or(&part);
                    display_read_targets.push(display.to_owned());
                    for block in result.content {
                        if let UserBlock::Text(text) = block {
                            if !pending_text.is_empty() {
                                pending_text.push_str("\n\n");
                            }
                            pending_text.push_str(&text.text);
                        } else {
                            if !pending_text.is_empty() {
                                content.push(UserBlock::text(std::mem::take(&mut pending_text)));
                            }
                            content.push(block);
                        }
                    }
                }
                Err(error) => {
                    if cancel.is_cancelled() {
                        return Err(error);
                    }
                    let note = format!("Could not read {part}: {}", error.0);
                    notes.push(note.clone());
                    display_read_targets.push(part);
                    if !pending_text.is_empty() {
                        pending_text.push_str("\n\n");
                    }
                    pending_text.push_str(&format!("[{note}]"));
                }
            }
        }
        if !pending_text.is_empty() {
            content.push(UserBlock::text(pending_text));
        }
        Ok(ToolOutput {
            content,
            details: Some(json!({"notes":notes,"displayReadTargets":display_read_targets})),
            is_error: false,
            provider_metadata: None,
            useless: false,
        })
    }

    /// Fixed local:// file promotion retains its selector separately; a literal
    /// sibling filename must not shadow the URL's selected range/raw semantics.
    pub async fn execute_promoted_os_path(
        &self,
        path: &str,
        selector: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        self.execute_promoted_os_path_with_delimited_dispatcher(path, selector, cancel, None)
            .await
    }

    pub async fn execute_promoted_os_path_with_delimited_dispatcher(
        &self,
        path: &str,
        selector: Option<&str>,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        if selector.is_some_and(|selector| selector.eq_ignore_ascii_case("conflicts")) {
            return self.read_file_conflicts(path, cancel).await;
        }
        self.execute_os_path_with_selector(path, Some(selector), cancel, dispatcher)
            .await
    }

    async fn execute_os_path_with_selector(
        &self,
        path: &str,
        promoted_selector: Option<Option<&str>>,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        let snapshot = self.ctx.try_edit_runtime_policy()?;
        let action = Self {
            ctx: self.ctx.clone().with_edit_runtime_snapshot(snapshot),
        };
        action
            .execute_os_path_with_selector_snapshot(path, promoted_selector, cancel, dispatcher)
            .await
    }
    async fn execute_os_path_with_selector_snapshot(
        &self,
        path: &str,
        promoted_selector: Option<Option<&str>>,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        let input = path;
        if let Some(uri) = crate::conflict_detect::parse_conflict_uri(input)? {
            return self.read_registered_conflict(uri).await;
        }
        if !path_exists(&self.ctx.resolve(input))
            && let Some((target, selector)) = input.rsplit_once(':')
            && selector.eq_ignore_ascii_case("conflicts")
            && crate::internal_urls::hierarchical_scheme(target).is_none()
        {
            return self.read_file_conflicts(target, cancel).await;
        }
        if let Some(parts) = self.routed_delimited_parts(input, &|_| false)? {
            return self.read_delimited_parts(parts, cancel, dispatcher).await;
        }
        if let Some(scheme) = crate::internal_urls::hierarchical_scheme(input) {
            if let Some(port) = self.ctx.content_uri_port() {
                match port.route(&crate::internal_urls::split_content_url_selector(input).0) {
                    crate::ContentUriRoute::Registered { .. } => {
                        return self.read_content_uri(port, input, cancel).await;
                    }
                    crate::ContentUriRoute::Removed => {
                        return Err(ToolError(format!(
                            "Unknown protocol: {}://",
                            scheme.to_ascii_lowercase()
                        )));
                    }
                    crate::ContentUriRoute::Unregistered => {}
                }
            }
            if !crate::internal_urls::is_internal_url(input) {
                return Err(ToolError(format!(
                    "Unknown protocol: {}://",
                    scheme.to_ascii_lowercase()
                )));
            }
        }
        // Internal URLs (`skill://…`) resolve to a file first; selectors apply
        // to it. A skill resource is immutable and exempt from result limits
        // (upstream `immutable`, `ignoreResultLimits: scheme === "skill"`):
        // no hashline tags, no notebook projection, no line/byte truncation.
        let internal = crate::internal_urls::is_internal_url(input);
        if internal {
            let exists = |p: &str| {
                self.ctx
                    .resolve_internal_url(p)
                    .is_ok_and(|abs| path_exists(&abs))
            };
            let (url, sel) = split_selector(input, exists).map_err(ToolError)?;
            let target = self.ctx.resolve_internal_url(&url).map_err(ToolError)?;
            let mut output = self
                .read_resolved_plain(
                    target.to_string_lossy().into_owned(),
                    sel,
                    internal,
                    cancel,
                    dispatcher,
                )
                .await?;
            let details = output.details.get_or_insert_with(|| json!({}));
            // Fixed read.ts2361-2375: the physical sourcePath belongs to
            // resolvedPath; sourceInternal(url) owns the result provenance.
            details["meta"]["source"] = json!({"type":"internal","value":url});
            return Ok(output);
        }
        let raw_is_literal = if promoted_selector.is_some() {
            input.contains(':') && path_exists(&self.ctx.resolve(input))
        } else {
            crate::paths::split_path_and_sel(input).1.is_some()
                && path_exists(&self.ctx.resolve(input))
        };
        let ctx = self.ctx.clone();
        let authored = input.to_owned();
        let work = cancel.clone();
        let promoted = promoted_selector.map(|selector| selector.map(str::to_owned));
        let mut dispatch =
            tokio::task::spawn_blocking(move || -> Result<ReadDispatch, ToolError> {
                let mut memo = crate::read_sqlite::ScopedSuffixMemo::new(&ctx, &work);
                if !raw_is_literal {
                    if let Some(target) = archive::resolve(&authored, &mut memo)? {
                        let selector = promoted.as_ref().map(|selector| selector.as_deref());
                        return archive::read(&ctx, &authored, target, selector, &work)
                            .map(ReadDispatch::Completed);
                    }
                    if let Some(target) = memo.resolve(&authored)? {
                        return crate::read_sqlite::read(&ctx, &target, &work)
                            .map(ReadDispatch::Completed);
                    }
                }
                if !raw_is_literal
                    && promoted.is_none()
                    && crate::paths::split_path_and_sel(&authored).1.is_none()
                    && std::fs::symlink_metadata(ctx.resolve(&authored)).is_err_and(|error| {
                        matches!(
                            error.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                        ) || cfg!(windows) && error.raw_os_error() == Some(123)
                    })
                    && let Some((mut path, page)) =
                        crate::read_pdf_host::split_pdf_image_path(&authored)
                {
                    let mut suffix = None;
                    let recovery = recover_missing_read_path(
                        &ctx,
                        &authored,
                        &mut path,
                        &mut suffix,
                        &mut memo,
                    )?;
                    return read_recovery_dispatch(
                        &ctx,
                        &authored,
                        path,
                        suffix,
                        PlanAliasTail::Pdf(page),
                        recovery,
                    );
                }
                // Fixed read.ts1402-1447 resolves missing/list scope before the
                // image effect at1518-1527; retain literal and container priority.
                if !raw_is_literal {
                    let (mut path, selector) = if let Some(selector) = &promoted {
                        (authored.clone(), selector.clone())
                    } else {
                        crate::paths::split_path_and_sel(&authored)
                    };
                    if selector
                        .as_deref()
                        .is_some_and(|selector| selector.eq_ignore_ascii_case("img"))
                    {
                        let mut suffix = None;
                        let recovery = recover_missing_read_path(
                            &ctx,
                            &authored,
                            &mut path,
                            &mut suffix,
                            &mut memo,
                        )?;
                        return read_recovery_dispatch(
                            &ctx,
                            &authored,
                            path,
                            suffix,
                            PlanAliasTail::Svg,
                            recovery,
                        );
                    }
                }
                // Preserve the raw media suffix before the generic line parser.
                // Literal colon names and archive/SQLite/PDF routing retain priority.
                let (mut video_path, video_selector) = if let Some(promoted) = &promoted {
                    (authored.clone(), promoted.clone())
                } else {
                    let literal =
                        crate::paths::split_path_and_sel_preferring_literal(&ctx, &authored);
                    match crate::read_video::split_video_read_target(&authored) {
                        Some((base, selector))
                            if crate::paths::probe_literal(&ctx, &base)
                                != crate::paths::Probe::Missing
                                && crate::paths::probe_literal(&ctx, &authored)
                                    != crate::paths::Probe::Exists =>
                        {
                            (base, Some(selector))
                        }
                        _ => literal,
                    }
                };
                if crate::read_video::is_video_path(&video_path) {
                    // Fixed invalid line-shaped selectors remain valid line parsing
                    // first, then the media reader supplies its video-specific hint.
                    if video_selector.as_deref().is_some_and(|value| {
                        crate::read_video::parse_video_selector(Some(value)).is_none()
                    }) {
                        content_selector(video_selector.as_deref())?;
                    }
                    let mut suffix = None;
                    let recovery = recover_missing_read_path(
                        &ctx,
                        &authored,
                        &mut video_path,
                        &mut suffix,
                        &mut memo,
                    )?;
                    return read_recovery_dispatch(
                        &ctx,
                        &authored,
                        video_path,
                        suffix,
                        PlanAliasTail::Video(video_selector),
                        recovery,
                    );
                }
                let (mut path, selector) = if let Some(promoted) = &promoted {
                    (authored.clone(), content_selector(promoted.as_deref())?)
                } else {
                    split_selector(&authored, |p| path_exists(&ctx.resolve(p)))
                        .map_err(ToolError)?
                };
                let mut suffix = None;
                let recovery =
                    recover_missing_read_path(&ctx, &authored, &mut path, &mut suffix, &mut memo)?;
                read_recovery_dispatch(
                    &ctx,
                    &authored,
                    path,
                    suffix,
                    PlanAliasTail::Plain(selector),
                    recovery,
                )
            })
            .await
            .map_err(|error| ToolError(format!("Cannot read: {error}")))??;
        // Suspend once at the terminal alias cutpoint, without restarting its worker.
        while let ReadDispatch::PendingPlanAlias(pending) = dispatch {
            let located = if let ReadPlanReferenceSnapshot::Uri {
                authored,
                native_local_fallback,
            } = &pending.reference
            {
                let normalized =
                    crate::internal_urls::normalize_plan_uri(pending.port.as_deref(), authored);
                let bare = crate::internal_urls::split_content_url_selector(&normalized).0;
                let route = pending
                    .port
                    .as_ref()
                    .map_or(crate::ContentUriRoute::Unregistered, |port| {
                        port.route(&bare)
                    });
                match route {
                    crate::ContentUriRoute::Registered { .. } => pending
                        .port
                        .as_ref()
                        .unwrap()
                        .search_path_only(&bare, cancel.clone())
                        .await
                        .ok()
                        .flatten(),
                    crate::ContentUriRoute::Removed => None,
                    crate::ContentUriRoute::Unregistered => {
                        if crate::internal_urls::hierarchical_scheme(&normalized)
                            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("local"))
                        {
                            native_local_fallback.clone()
                        } else {
                            None
                        }
                    }
                }
            } else {
                None
            };
            let ctx = self.ctx.clone();
            dispatch = tokio::task::spawn_blocking(move || {
                let mut path = pending.path;
                let recovered = located.filter(|reference| {
                    pending.absolute.file_name() == reference.file_name()
                        && std::fs::metadata(reference).is_ok()
                });
                if let Some(reference) = recovered {
                    path = reference.to_string_lossy().into_owned();
                } else if let Some(parts) = read_missing_split(&ctx, &pending.authored)? {
                    return Ok(ReadDispatch::Delimited(parts));
                }
                finish_plan_alias_tail(&ctx, path, pending.suffix, pending.tail)
            })
            .await
            .map_err(|error| ToolError(format!("Cannot read: {error}")))??;
        }
        match dispatch {
            ReadDispatch::PendingPlanAlias(_) => {
                unreachable!("resumed tails do not issue another locator")
            }
            ReadDispatch::Completed(output) => Ok(output),
            ReadDispatch::Delimited(parts) => {
                self.read_delimited_parts(parts, cancel, dispatcher).await
            }
            ReadDispatch::SvgImage { path, suffix } => {
                let absolute = self.ctx.resolve(&path);
                let inspect = format!("{}:img", self.ctx.display(&absolute));
                let output = self
                    .read_image_content(
                        &absolute,
                        crate::read_image::ReadImageKind::Svg,
                        None,
                        &inspect,
                        cancel,
                    )
                    .await?
                    .ok_or_else(|| {
                        ToolError("The ':img' selector only supports .svg and .svgz files.".into())
                    })?;
                Ok(PlainReadReceipt {
                    output,
                    suffix_notice: SuffixNoticeDisposition::PrefixFirstText,
                }
                .project(suffix))
            }
            ReadDispatch::PdfImage { path, page, suffix } => {
                let output = self.read_pdf_image(path, page, cancel).await?;
                Ok(PlainReadReceipt {
                    output,
                    suffix_notice: SuffixNoticeDisposition::PrefixFirstText,
                }
                .project(suffix))
            }
            ReadDispatch::Video {
                path,
                selector,
                suffix,
            } => {
                let output = self
                    .read_video_content(&self.ctx.resolve(&path), selector.as_deref(), cancel)
                    .await?;
                Ok(PlainReadReceipt {
                    output,
                    suffix_notice: SuffixNoticeDisposition::PrefixFirstText,
                }
                .project(suffix))
            }
            ReadDispatch::Plain {
                path,
                selector,
                suffix,
            } => Ok(self
                .read_resolved_plain_receipt(path, selector, false, cancel, dispatcher)
                .await?
                .project(suffix)),
        }
    }

    async fn read_pdf_image(
        &self,
        path: String,
        page: u64,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let abs = self.ctx.resolve(&path);
        let display = self.ctx.display(&abs);
        let meta = tokio::fs::metadata(&abs).await.map_err(|error| {
            ToolError(if error.kind() == std::io::ErrorKind::NotFound {
                format!("File not found: {display}")
            } else {
                format!("Cannot read {display}: {error}")
            })
        })?;
        if !meta.is_file() {
            return Err(ToolError(format!(
                "Cannot read {display}: not a regular file"
            )));
        }
        let port = self.ctx.pdf_screenshot_port().ok_or_else(|| {
            ToolError("Host shared Chromium PDF renderer is not installed".into())
        })?;
        if cancel.is_cancelled() {
            return Err(ToolError("Read PDF was aborted".into()));
        }
        let screenshot = port.render_pdf_page(&abs, page, cancel.clone()).await?;
        if cancel.is_cancelled() {
            return Err(ToolError("Read PDF was aborted".into()));
        }
        // Reload the saved artifact. Renderer bytes/dimensions may describe
        // stage one and cannot stand for the full-resolution saved file.
        let mut output = self
            .read_image_content(
                &screenshot.path,
                crate::read_image::ReadImageKind::Raster,
                None,
                &self.ctx.display(&screenshot.path),
                cancel,
            )
            .await?
            .ok_or_else(|| ToolError("Read image file failed: unsupported image format.".into()))?;
        if let Some(details) = output.details.as_mut() {
            details["resolvedPath"] = json!(abs.to_string_lossy());
            details["fileSize"] = json!(meta.len());
        }
        Ok(output)
    }

    /// OS boundary only. Native UTF16 throws must call read_video_file and
    /// propagate VideoError.message directly; that caller hook is separately gated.
    async fn read_video_content(
        &self,
        path: &Path,
        selector: Option<&str>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        let display = self.ctx.display(path);
        let metadata = tokio::fs::metadata(path).await.map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ToolError(format!("File not found: {display}"))
            } else {
                ToolError(format!("Cannot read {display}: {error}"))
            }
        })?;
        if !metadata.is_file() {
            return Err(ToolError(format!(
                "Cannot read {display}: not a regular file"
            )));
        }
        let runtime = self
            .ctx
            .read_video_runtime()
            .ok_or_else(|| ToolError("Host video media runtime is not installed".into()))?;
        let policy = self.ctx.read_image_policy();
        let codec = self.ctx.read_image_codec();
        crate::read_video::read_video_file(
            &runtime,
            path,
            &display,
            selector,
            metadata.len(),
            self.ctx.read_auto_resize_images(),
            policy.as_deref(),
            codec.as_deref(),
            cancel,
        )
        .await
        .map_err(crate::read_video::VideoError::into_os_tool_error)
    }

    async fn read_image_content(
        &self,
        path: &Path,
        kind: crate::read_image::ReadImageKind,
        detected_mime: Option<&str>,
        inspect_path: &str,
        cancel: CancellationToken,
    ) -> Result<Option<ToolOutput>, ToolError> {
        let policy = self.ctx.read_image_policy();
        let codec = self.ctx.read_image_codec();
        let loaded = crate::read_image::load_read_image(
            path,
            kind,
            detected_mime,
            inspect_path,
            self.ctx.read_auto_resize_images(),
            policy.as_deref(),
            codec.as_deref(),
            cancel,
        )
        .await?;
        Ok(loaded.map(|loaded| ToolOutput {
            content: loaded.content,
            details: Some(json!({"resolvedPath":path.to_string_lossy(),"contentType":loaded.content_type,"fileSize":loaded.file_size,
                "meta":{"source":{"type":"path","value":loaded.source_path.to_string_lossy()}}})),
            is_error:false,provider_metadata:None,useless:false,
        }))
    }

    async fn read_resolved_plain(
        &self,
        path: String,
        sel: Selector,
        internal: bool,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<ToolOutput, ToolError> {
        Ok(self
            .read_resolved_plain_receipt(path, sel, internal, cancel, dispatcher)
            .await?
            .project(None))
    }

    async fn read_resolved_plain_receipt(
        &self,
        path: String,
        sel: Selector,
        internal: bool,
        cancel: CancellationToken,
        dispatcher: Option<Arc<dyn ReadDelimitedDispatcher>>,
    ) -> Result<PlainReadReceipt, ToolError> {
        let mut suffix_notice = SuffixNoticeDisposition::PrefixFirstText;
        let output = async {
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
                return Err(ToolError("Multi-range line selectors are not supported for directory listings.".into()));
            }
            if !internal {
                let tree_path = abs.clone();
                let (listing, tree_truncated) = tokio::task::spawn_blocking(move || read_directory_tree(&tree_path))
                    .await
                    .map_err(|error| ToolError(format!("Cannot read directory: {error}")))??;
                let mut details = json!({"resolvedPath":resolved,"isDirectory":true,
                    "meta":{"source":{"type":"path","value":abs.to_string_lossy()}}});
                // Fixed read.ts returns a beyond-end notice before adding tree limits.
                let selector_beyond =
                    matches!(sel.range, Some(Range::From(start, _)) if start > listing.split('\n').count());
                let text = if sel.range.is_some() {
                    directory_listing_window(&listing, &sel)
                } else {
                    let truncated = crate::output::truncate_head(&listing, usize::MAX, DEFAULT_MAX_BYTES);
                    let mut text = truncated.content.clone();
                    if truncated.truncated {
                        details["truncation"] = truncated.details();
                        details["meta"]["truncation"] = json!({
                            "direction":"head", "truncatedBy":if truncated.by_bytes {"bytes"} else {"lines"},
                            "totalLines":truncated.total_lines, "totalBytes":truncated.total_bytes,
                            "outputLines":truncated.output_lines, "outputBytes":truncated.output_bytes,
                            "shownRange":{"start":1,"end":truncated.output_lines},
                            "nextOffset":truncated.output_lines+1,
                        });
                        let mut notice = crate::output::Notice::default();
                        notice.truncation(&truncated);
                        text.push_str(&notice.render());
                    }
                    text
                };
                if tree_truncated && !selector_beyond {
                    details["meta"]["limits"]["resultLimit"] = json!({"reached":1,"suggestion":2});
                }
                suffix_notice = SuffixNoticeDisposition::DetailsOnly; // ARA private receipt tag
                return Ok(ToolOutput::text(text).with_details(details));
            }
            let mut entries = Vec::new();
            let mut rd =
                tokio::fs::read_dir(&abs).await.map_err(|e| ToolError(format!("Cannot list {display}: {e}")))?;
            loop {
                let entry = match rd.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(e) if internal => {
                        return Err(ToolError(format!("Cannot list {display}: {e}")));
                    }
                    Err(_) => break,
                };
                let is_dir = match entry.file_type().await {
                    Ok(file_type) => file_type.is_dir(),
                    Err(e) if internal => {
                        return Err(ToolError(format!("Cannot list {display}: {e}")));
                    }
                    Err(_) => false,
                };
                entries.push((is_dir, entry.file_name().to_string_lossy().into_owned()));
            }
            if internal {
                let collator = skill_directory_collator()?;
                entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| collator.compare(&a.1, &b.1)));
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
                    ReadRender { default_limit: None,
                        numbering,
                        entity_label: None,
                        text_resource: true,
                        ignore_result_limits: true,
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
                suffix_notice = SuffixNoticeDisposition::DetailsOnly; // ARA private receipt tag
                return Ok(ToolOutput::text(window.text).with_details(details));
            }
        }
        if !meta.is_file() {
            return Err(ToolError(format!("Cannot read {display}: not a regular file")));
        }
        if !internal && !sel.raw && let Some(metadata) = crate::read_image::read_image_metadata(&abs).await? {
            if !sel.multi_ranges.is_empty() {
                return Err(ToolError(format!("Multiple line ranges require a text file: {display} is an image")));
            }
            return self.read_image_content(&abs, crate::read_image::ReadImageKind::Raster,
                Some(&metadata.mime_type), &display, cancel).await?
                .ok_or_else(|| ToolError(format!("Read image file [{}] failed: unsupported image format.",metadata.mime_type)));
        }
        // Fixed read.ts1490-1507: no image MIME, non-raw, <=32MiB;
        // sample is checked first. A parser null falls through to disk text,
        // but optional-field TypeError does not become a false null fallback.
        let profile_path = abs.to_string_lossy();
        let sample_profile = crate::read_profile::is_sample_profile_path(&profile_path);
        let cpu_profile = crate::read_profile::is_cpu_profile_path(&profile_path);
        if !internal && !sel.raw && meta.len() <= 32 * 1024 * 1024 && (sample_profile || cpu_profile) {
            let bytes = tokio::fs::read(&abs).await
                .map_err(|error| ToolError(format!("Cannot read {display}: {error}")))?;
            // Fixed Bun.file.text boundary: UTF8 BOM removal/replacement,
            // preserving CRLF. Pure renderers receive this decoded text once.
            let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
            let decoded = String::from_utf8_lossy(bytes);
            let rendered = if sample_profile {
                Ok(crate::read_profile::render_sample_profile(&decoded))
            } else { crate::read_profile::render_cpu_profile(&decoded) };
            let rendered = match rendered {
                Ok(rendered) => rendered,
                Err(error) => {
                    let message = error.message.to_utf8().map_err(|_| ToolError("Profiler error crossed an unsupported UTF16 native boundary".into()))?;
                    if error.kind == "TypeError" && let Some(publisher) = &dispatcher {
                        publisher.publish_profile_type_error(message.clone()).await?;
                    }
                    return Err(ToolError(message));
                }
            };
            if let Some(rendered) = rendered.filter(|text| !text.is_empty()) {
                let receipt = in_memory_wire::build(&self.ctx, &abs, &sel, rendered, &cancel,
                    in_memory_wire::MemoryOptions { entity: "profile summary", markdown: false, abort: None })?;
                let output = receipt.legacy();
                if let Some(publisher) = &dispatcher { publisher.publish_in_memory_output(receipt.into_wire()).await?; }
                suffix_notice = SuffixNoticeDisposition::Omit;
                return Ok(output);
            }
        }
        if !internal && abs.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")) {
            let ctx = self.ctx.clone();
            return crate::run_blocking("Read PDF", cancel, move |work| {
                let bytes = std::fs::read(&abs).map_err(|error| ToolError(format!("Cannot read {display}: {error}")))?;
                if work.is_cancelled() { return Err(ToolError("Read PDF was aborted".into())); }
                let cache = ctx.document_cache();
                let conversion = crate::read_document_cache::convert_buffer_with_cache(
                    cache.as_deref(), &bytes, ".pdf", &work, true,
                    |bytes, _stream| crate::read_document::convert_pdf_with(bytes, ctx.pdf_adapter().as_ref())
                        .map(|result| result.markdown),
                );
                if work.is_cancelled() { return Err(ToolError("Read PDF was aborted".into())); }
                let conversion = conversion?;
                let content = if conversion.ok { conversion.content } else {
                    let error = conversion.error.expect("Failed document conversion has an error");
                    return Ok(ToolOutput::text(format!("[Cannot read .pdf file: {error}]")).with_details(json!({
                        "resolvedPath":abs.to_string_lossy(), "meta":{"source":{"type":"path","value":abs.to_string_lossy()}}
                    })));
                };
                build_document_output(&ctx, &abs, &display, &sel, &content, &work)
            }).await;
        }
        let hashlines = self.ctx.hashlines() && !sel.raw && !internal;
        let notebook = !sel.raw && !internal && abs.extension().is_some_and(|e| e.eq_ignore_ascii_case("ipynb"));
        let summary_allowed = !internal && !notebook && self.ctx.read_summary_policy().permits(
            &abs,!sel.raw && sel.range.is_none() && sel.multi_ranges.is_empty());
        let mut disk_buffer = None;
        if !internal && !notebook {
            let source_path = abs.clone();let raw_selector = sel.raw;
            let buffered = meta.len() <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES;
            let (binary,buffer) = tokio::task::spawn_blocking(move || -> std::io::Result<_> {
                let bytes = if buffered {std::fs::read(&source_path)?} else if raw_selector {Vec::new()} else {
                    let mut head = Vec::new();std::fs::File::open(&source_path)?.take(SNIFF_BYTES as u64).read_to_end(&mut head)?;head
                };
                let binary = !raw_selector && sniff_binary(&bytes[..bytes.len().min(SNIFF_BYTES)]);
                let buffer = if buffered && !binary {
                    let decoded = String::from_utf8_lossy(&bytes);
                    let stripped = ara_edit::text::strip_bom(&decoded).1.to_owned();
                    let normalized = ara_edit::text::normalize_to_lf(&stripped).into_owned();
                    Some(BufferedPlainText {bytes,stripped,normalized})
                } else {None};
                Ok((binary,buffer))
            }).await.map_err(|e|ToolError(format!("Cannot read {display}: {e}")))?
                .map_err(|e|ToolError(format!("Cannot read {display}: {e}")))?;
            if binary {return Ok(ToolOutput::text(format!(
                "[Cannot read binary file '{display}' ({}); not valid UTF-8 text. Use ':raw' to read bytes verbatim.]",format_bytes(meta.len())))
                .with_details(json!({"resolvedPath":resolved,"meta":{"source":{"type":"path","value":abs.to_string_lossy()}}})));}
            disk_buffer = buffer;
        }
        if summary_allowed {
            if let Some(summary) = crate::read_summary::try_summarize(&self.ctx,&abs,meta.len(),&cancel,
                disk_buffer.as_ref().map(|buffer|buffer.stripped.clone())).await {
                let summary_display = crate::paths::format_path_relative_to_cwd(&abs,&self.ctx.cwd,false);
                let mut output = crate::read_summary::summary_output(&self.ctx,&abs,&summary_display,&summary,
                    disk_buffer.as_ref().map(|buffer|buffer.normalized.as_str()))?;
                output.details.as_mut().expect("Summary details")["meta"]["source"] = json!({"type":"path","value":abs.to_string_lossy()});
                return Ok(output);
            }
        }
        // Fixed read-summary.ts routes complete text through ACP first and
        // falls back to disk on read failure. No selector is sent to the client.
        let bridge_text = if !internal && !notebook {
            if let Some(bridge) =
                self.ctx.edit_host().and_then(|host| host.client_bridge()).filter(|bridge| bridge.can_read_text_file())
            {
                bridge.read_text_file(&abs).await.ok()
            } else {
                None
            }
        } else {
            None
        };
        let bridge_read = bridge_text.is_some();
        let raw = sel.raw;
        let line_numbers = self.ctx.line_numbers;
        let store = self.ctx.edit_store.clone();
        let ctx = self.ctx.clone();
        // Ordinary deterministic file windows are non-abortable upstream;
        // internal resources and non-text routes keep their existing token.
        let window_cancel = if internal || notebook { cancel.clone() } else { CancellationToken::new() };
        let (abs2, display2, cancel2) = (abs.clone(), display.clone(), window_cancel);
        let file_size = meta.len();
        let (window, text) = tokio::task::spawn_blocking(move || -> Result<(Window, String), String> {
            let io = |e: std::io::Error| format!("Cannot read {display2}: {e}");
            // SkillProtocolHandler creates an in-memory UTF-8 text resource
            // for every regular file, including images and invalid UTF-8.
            let resource_text = if let Some(text) = &bridge_text {
                let mut content = text.clone();
                let size = content.len() as u64;
                if raw && sel.multi_ranges.is_empty() && (content.is_empty() || content.ends_with('\n')) {
                    content.push('\n');
                }
                Some((content, size))
            } else if internal {
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
            if raw && let Some(buffer) = &disk_buffer {
                let mut bytes = buffer.bytes.clone();
                if sel.multi_ranges.is_empty() && (bytes.is_empty() || bytes.ends_with(b"\n")) {bytes.push(b'\n');}
                undecodable = Some(bytes);
            }
            let editable: Option<String> = if let Some(text) = &bridge_text {
                (!raw && text.len() as u64 <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES)
                    .then(|| ara_edit::text::normalize_to_lf(ara_edit::text::strip_bom(text).1).into_owned())
            } else if !raw && let Some(buffer) = &disk_buffer {
                Some(buffer.normalized.clone())
            } else if notebook {
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
            let mut resource_text = resource_text;
            if bridge_text.is_some()
                && !raw
                && let Some(text) = &editable
            {
                resource_text = Some((text.clone(), text.len() as u64));
            }
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
                    ReadRender { default_limit: None,
                        numbering,
                        entity_label: bridge_text.as_ref().map(|_| "file"),
                        text_resource: true,
                        ignore_result_limits: internal || !sel.multi_ranges.is_empty(),
                        block_context: (!raw && !sel.multi_ranges.is_empty())
                            .then_some((content.as_str(), abs2.as_path())),
                    },
                    &cancel2,
                )?
            } else if sel.multi_ranges.is_empty() && let Some(buffer) = &disk_buffer {
                // Single disk windows retain the bytes used by fixed
                // collectLineWindowFromBuffer. Snapshot/bracket views remain
                // separately normalized, and nonraw multi uses that text view.
                let bytes = undecodable.as_ref().unwrap_or(&buffer.bytes);
                read_window(
                    std::io::Cursor::new(bytes.as_slice()),
                    file_size,
                    &display2,
                    &sel,
                    ReadRender { default_limit: Some(ctx.read_default_limit()),
                        numbering,
                        entity_label: None,
                        text_resource: false,
                        ignore_result_limits: false,
                        block_context: None,
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
                        ReadRender { default_limit: (!notebook).then(||ctx.read_default_limit()),
                            numbering,
                            entity_label: None,
                            text_resource: false,
                            ignore_result_limits: false,
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
                        ReadRender { default_limit: (!notebook).then(||ctx.read_default_limit()),
                            numbering,
                            entity_label: None,
                            text_resource: false,
                            ignore_result_limits: false,
                            block_context: None,
                        },
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
                            ReadRender { default_limit: (!notebook).then(||ctx.read_default_limit()),
                                numbering,
                                entity_label: None,
                                text_resource: false,
                                ignore_result_limits: false,
                                block_context: None,
                            },
                            &cancel2,
                        )?
                    }
                }
            };
            let mut window = window;
            // Fixed ACP/internal builders return before the ordinary disk cap.
            if !raw && !internal && bridge_text.is_none() {
                cap_window_display(&mut window,numbering,ctx.read_output_max_columns())?;
                if window.emitted > 0 && window.oversized_first_line.is_none() {
                    let mut plain = String::new();let mut numbers = Vec::new();let mut previous = None;
                    for (line,source) in &window.conflict_lines {
                        if let Some(before) = previous {
                            if *line == before + 1 {plain.push('\n');} else {plain.push_str("\n…\n");numbers.push(Value::Null);}
                        }
                        let display = column_display_line(source,ctx.read_output_max_columns())?.0;
                        plain.push_str(&display);numbers.push(json!(line));previous=Some(*line);
                    }
                    if let Some((start,_)) = window.conflict_lines.first() {
                        window.details["displayContent"] = json!({"text":plain,"startLine":start,"lineNumbers":numbers});
                    }
                }
            }
            if bridge_text.is_some()
                && let Some((content, _)) = &resource_text
            {
                let lines = content.split_terminator('\n').collect::<Vec<_>>();
                if let Some((start, plain, numbers)) = resource_display_content(&lines, &sel, &window, numbering) {
                    window.details["displayContent"] = json!({"text":plain,"startLine":start,"lineNumbers":numbers});
                }
            }
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
                    if let Some(text) = &bridge_text {
                        let normalized = ara_edit::text::normalize_to_lf(ara_edit::text::strip_bom(text).1);
                        if normalized.len() as u64 <= ara_edit::store::MAX_SNAPSHOT_FILE_BYTES {
                            store.record(&key, &normalized, Some(&seen));
                        }
                    } else if let Some(buffer) = &disk_buffer {
                        store.record(&key,&buffer.normalized,Some(&seen));
                    } else {
                        store.record_file(&abs2, Some(&seen));
                    }
                }
            }
            // Fixed read.ts:1962 scans the rows already emitted, without extra
            // I/O on clean files. A disjoint gap cannot close a synthetic block.
            if !internal && !notebook && window.oversized_first_line.is_none() {
                let mut blocks = Vec::new();
                let mut begin = 0;
                while begin < window.conflict_lines.len() {
                    let mut end = begin + 1;
                    while end < window.conflict_lines.len()
                        && window.conflict_lines[end].0 == window.conflict_lines[end - 1].0 + 1
                    {
                        end += 1;
                    }
                    let lines =
                        window.conflict_lines[begin..end].iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>();
                    blocks.extend(crate::conflict_detect::scan_conflict_lines(&lines, window.conflict_lines[begin].0));
                    begin = end;
                }
                if !blocks.is_empty() {
                    let entries = {
                        let mut history = ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner());
                        blocks
                            .into_iter()
                            .map(|block| history.register(abs2.clone(), display2.clone(), block))
                            .collect::<Vec<_>>()
                    };
                    let (total, truncated) = crate::conflict_detect::scan_file_for_conflicts(
                        &abs2,
                        crate::conflict_detect::SCAN_FILE_MAX_BYTES,
                    )
                    .map_or((entries.len(), false), |(blocks, truncated)| (entries.len().max(blocks.len()), truncated));
                    text.push_str(&crate::conflict_detect::format_conflict_warning(
                        &entries,
                        crate::conflict_detect::WarningOptions {
                            total_in_file: Some(total),
                            display_path: Some(&display2),
                            scan_truncated: truncated,
                        },
                    ));
                    window.details["conflictCount"] = json!(entries.len());
                }
            }
            Ok((window, text))
        })
        .await
        .map_err(|e| ToolError(format!("Cannot read {display}: {e}")))?
        .map_err(ToolError)?;
        if !internal && !notebook && !bridge_read && window.origin == WindowOrigin::SingleRangeEof {
            suffix_notice = SuffixNoticeDisposition::DetailsOnly;
        } // ARA private receipt EOF tag
        let mut details = window.details;
        details["resolvedPath"] = resolved;
        // The on-disk size, not the size of a notebook's cell projection.
        if bridge_read {
            details.as_object_mut().expect("read details object").remove("fileSize");
        } else {
            details["fileSize"] = json!(meta.len());
        }
        if internal {
            let markdown = abs.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
                ["md", "markdown", "mdx", "mdc", "mkd", "mdown"]
                    .iter()
                    .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            });
            details["contentType"] = json!(if markdown { "text/markdown" } else { "text/plain" });
        }
        if !internal && self.ctx.read_render_markdown() && crate::read_summary::is_markdown_path(&abs) {
            details["contentType"] = json!("text/markdown");
        }
        Ok(ToolOutput::text(text).with_details(details))
        }.await?;
        Ok(PlainReadReceipt {
            output,
            suffix_notice,
        })
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
        assert!(
            !path_exists(&file.join("child")),
            "a file cannot be a parent"
        );
        assert!(
            !path_exists(&dir.path().join("a".repeat(300))),
            "name past the OS limit"
        );
    }

    #[test]
    fn selectors() {
        let none = |_: &str| false;
        let ok = |s: &str| split_selector(s, none).unwrap();
        assert_eq!(
            ok("a.txt:5-9"),
            (
                "a.txt".into(),
                Selector {
                    range: Some(Range::From(5, Some(9))),
                    multi_ranges: vec![],
                    raw: false
                }
            )
        );
        assert_eq!(ok("a.txt:5+3").1.range, Some(Range::From(5, Some(7))));
        assert_eq!(ok("a.txt:-4").1.range, Some(Range::Tail(4)));
        assert_eq!(ok("a.txt:7-").1.range, Some(Range::From(7, None)));
        assert_eq!(
            ok("a.txt:raw:2-4"),
            (
                "a.txt".into(),
                Selector {
                    range: Some(Range::From(2, Some(4))),
                    multi_ranges: vec![],
                    raw: true
                }
            )
        );
        assert_eq!(
            ok("a.txt:2-4:raw").1,
            Selector {
                range: Some(Range::From(2, Some(4))),
                multi_ranges: vec![],
                raw: true
            }
        );
        assert_eq!(
            ok("a.txt:9,3-4,4-5").1.multi_ranges,
            vec![Range::From(3, Some(5)), Range::From(9, None)]
        );
        assert_eq!(ok("a.txt:5-7,1-4").1.range, Some(Range::From(1, Some(7))));
        assert_eq!(
            ok("a.txt:2-3,8-").1.multi_ranges,
            vec![Range::From(2, Some(3)), Range::From(8, None)]
        );
        assert_eq!(
            ok("a.txt:L2..L3,L8+L2").1.multi_ranges,
            vec![Range::From(2, Some(3)), Range::From(8, Some(9))]
        );
        assert_eq!(ok("dir/a%3Ab.txt").0, "dir/a:b.txt");
        assert_eq!(ok("weird:name"), ("weird:name".into(), Selector::default()));
        assert_eq!(
            split_selector("x:1", |p| p == "x:1").unwrap(),
            ("x:1".into(), Selector::default())
        );
        assert_eq!(
            split_selector("x:1-2,4", |p| p == "x:1-2,4").unwrap(),
            ("x:1-2,4".into(), Selector::default())
        );
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
            assert!(
                err.starts_with("Invalid selector ':") && err.contains("on 'a.txt'"),
                "{err}"
            );
        }
    }

    #[test]
    fn binary_sniff_is_limited_to_the_head() {
        assert!(sniff_binary(&[0, 1, 2]));
        assert!(sniff_binary(&[0xff, b'a']));
        let mut cut = "é".repeat(10).into_bytes();
        cut.pop();
        assert!(
            !sniff_binary(&cut),
            "sequence cut at the sniff boundary is not binary"
        );
    }

    #[test]
    fn skill_resource_tail_and_line_totals_ignore_file_scan_budget() {
        let content = (1..=60)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let run = |range, text_resource| {
            read_window_with_scan_limit::<_, 8>(
                std::io::Cursor::new(content.as_bytes()),
                content.len() as u64,
                "lines.txt",
                &Selector {
                    range,
                    multi_ranges: vec![],
                    raw: true,
                },
                ReadRender {
                    default_limit: None,
                    numbering: Numbering::None,
                    entity_label: None,
                    text_resource,
                    ignore_result_limits: text_resource,
                    block_context: None,
                },
                &CancellationToken::new(),
            )
        };
        let tail = run(Some(Range::Tail(2)), true).unwrap();
        assert_eq!(tail.text, "line59\nline60");
        assert_eq!(tail.details["totalLines"], 60);
        let ordinary_tail = run(Some(Range::Tail(2)), false).err().unwrap();
        assert!(
            ordinary_tail.contains("too large to count lines for a tail selector"),
            "{ordinary_tail}"
        );

        let head = run(Some(Range::From(1, Some(1))), true).unwrap();
        assert_eq!(
            head.text,
            "line1\n\n[59 more lines in resource. Use :2 to continue]"
        );
        assert_eq!(head.details["totalLines"], 60);
        let ordinary_head = run(Some(Range::From(1, Some(1))), false).unwrap();
        assert!(
            ordinary_head.text.contains("not scanned to EOF"),
            "{}",
            ordinary_head.text
        );
        assert!(ordinary_head.details["totalLines"].is_null());
    }

    #[test]
    fn skill_resource_multi_range_counts_to_eof_beyond_file_scan_budget() {
        let content = (1..=60)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let sel = Selector {
            range: None,
            multi_ranges: vec![Range::From(3, Some(3)), Range::From(5, Some(5))],
            raw: true,
        };
        for text_resource in [true, false] {
            let window = read_window_with_scan_limit::<_, 8>(
                std::io::Cursor::new(content.as_bytes()),
                content.len() as u64,
                "lines.txt",
                &sel,
                ReadRender {
                    default_limit: None,
                    numbering: Numbering::None,
                    entity_label: None,
                    text_resource,
                    ignore_result_limits: text_resource,
                    block_context: None,
                },
                &CancellationToken::new(),
            )
            .unwrap();
            assert_eq!(window.text, "line3\n\n…\n\nline5");
            assert_eq!(window.raw_seen_lines, Some(vec![3, 5]));
            if text_resource {
                assert_eq!(window.details["totalLines"], 60);
            } else {
                assert!(window.details["totalLines"].is_null());
            }
        }
    }

    #[test]
    fn unbounded_skill_resource_scans_still_honor_cancellation() {
        let content = "line\n".repeat(5000);
        let cancel = CancellationToken::new();
        cancel.cancel();
        for sel in [
            Selector {
                range: Some(Range::Tail(2)),
                multi_ranges: vec![],
                raw: true,
            },
            Selector {
                range: Some(Range::From(1, Some(1))),
                multi_ranges: vec![],
                raw: true,
            },
            Selector {
                range: None,
                multi_ranges: vec![Range::From(1, Some(1)), Range::From(3, Some(3))],
                raw: true,
            },
        ] {
            let error = read_window_with_scan_limit::<_, 8>(
                std::io::Cursor::new(content.as_bytes()),
                content.len() as u64,
                "large-resource.txt",
                &sel,
                ReadRender {
                    default_limit: None,
                    numbering: Numbering::None,
                    entity_label: None,
                    text_resource: true,
                    ignore_result_limits: true,
                    block_context: None,
                },
                &cancel,
            )
            .err()
            .unwrap();
            assert_eq!(error, "Read of large-resource.txt was aborted");
        }
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
            ReadRender {
                default_limit: None,
                numbering: Numbering::Pipe,
                entity_label: None,
                text_resource: false,
                ignore_result_limits: false,
                block_context: None,
            },
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(
            window.text.starts_with("30|line30\n…\n50|line50"),
            "{}",
            window.text
        );
        assert!(
            !window.text.contains("continue"),
            "all selected ranges were complete: {}",
            window.text
        );
        assert!(window.details["totalLines"].is_null());
    }

    #[test]
    fn cancelled_buffered_multi_range_does_not_return_block_context() {
        let content = "function first() {\n  return 1;\n}\nfunction second() {\n  return 2;\n}\n";
        let sel = Selector {
            range: None,
            multi_ranges: vec![Range::From(1, Some(1)), Range::From(4, Some(4))],
            raw: false,
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let error = read_multi_window::<_, MAX_SCAN_BYTES>(
            std::io::Cursor::new(content.as_bytes()),
            content.len() as u64,
            "blocks.ts",
            &sel,
            ReadRender {
                default_limit: None,
                numbering: Numbering::Pipe,
                entity_label: None,
                text_resource: false,
                ignore_result_limits: false,
                block_context: Some((content, Path::new("blocks.ts"))),
            },
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error, "Read of blocks.ts was aborted");
    }
}

impl ReadTool {
    async fn read_registered_conflict(
        &self,
        uri: crate::conflict_detect::ParsedConflictUri,
    ) -> Result<ToolOutput, ToolError> {
        let crate::conflict_detect::ConflictId::Number(id) = uri.id else {
            return Err(ToolError("Reading `conflict://*` is not supported — wildcards are write-only. Use the `<path>:conflicts` read selector for the full list of conflicts in a file, or read `conflict://<N>` to inspect a single block.".into()));
        };
        let entry = self.ctx.conflict_history.lock().unwrap_or_else(|e| e.into_inner()).get(id)
            .ok_or_else(|| ToolError(format!("Conflict #{id} not found. Conflict ids are registered when `read` surfaces a marker block; re-read the file to get a current id.")))?;
        let (lines, start) = crate::conflict_detect::render_conflict_region(&entry, uri.scope)?;
        let raw = lines.join("\n");
        let numbering = if self.ctx.hashlines() {
            Numbering::Hashline
        } else if self.ctx.line_numbers {
            Numbering::Pipe
        } else {
            Numbering::None
        };
        let mut text = lines
            .iter()
            .enumerate()
            .map(|(i, line)| numbered_multi_row((start + i) as u32, line, numbering))
            .collect::<Vec<_>>()
            .join("\n");
        if self.ctx.hashlines()
            && let Some(tag) = self.ctx.edit_store.record_file(&entry.absolute_path, None)
        {
            text = format!(
                "{}\n{text}",
                ara_edit::modes::hashline::format::format_hashline_header(
                    &self.ctx.hashline_display(&entry.absolute_path),
                    &tag
                )
            );
        }
        Ok(ToolOutput::text(text).with_details(json!({"resolvedPath": entry.absolute_path.to_string_lossy(), "displayContent": {"text":raw, "startLine":start}})))
    }
    async fn read_file_conflicts(
        &self,
        target: &str,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError("Aborted: Cancelled".into()));
        }
        let absolute = self.ctx.resolve(target);
        let display = self.ctx.display(&absolute);
        let ctx = self.ctx.clone();
        crate::run_blocking("Read conflicts", cancel, move |work| {
            if work.is_cancelled() {
                return Err(ToolError("Aborted: Cancelled".into()));
            }
            let (blocks, truncated) = crate::conflict_detect::scan_file_for_conflicts(
                &absolute,
                crate::conflict_detect::SCAN_FILE_MAX_BYTES,
            )?;
            let entries = {
                let mut history = ctx
                    .conflict_history
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                blocks
                    .into_iter()
                    .map(|block| history.register(absolute.clone(), display.clone(), block))
                    .collect::<Vec<_>>()
            };
            let text = if entries.is_empty() {
                format!("No unresolved git merge conflicts in {display}.")
            } else {
                crate::conflict_detect::format_conflict_summary(&entries, &display, truncated)
            };
            Ok(ToolOutput::text(text).with_details(
                json!({"resolvedPath": absolute.to_string_lossy(), "conflictCount":entries.len()}),
            ))
        })
        .await
    }
}

#[cfg(test)]
mod fixed_read_size_tests {
    #[test]
    fn fixed_format_bytes_number_domain() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/read-format-bytes-fixed.json"
        ))
        .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let bytes = case["bytes"].as_str().unwrap().parse::<u64>().unwrap();
            assert_eq!(
                super::format_bytes(bytes),
                case["formatted"].as_str().unwrap(),
                "bytes={bytes}"
            );
        }
    }
}

/// Fixed read-format in-memory document owner, reused window/context primitives.
/// Unlike ordinary disk reads, document selectors address converted Markdown.
fn build_document_output(
    ctx: &ToolContext,
    path: &Path,
    display: &str,
    sel: &Selector,
    source: &str,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    if cancel.is_cancelled() {
        return Err(ToolError("Read PDF was aborted".into()));
    }
    let numbering = if sel.raw {
        Numbering::None
    } else if ctx.hashlines() {
        Numbering::Hashline
    } else if ctx.line_numbers {
        Numbering::Pipe
    } else {
        Numbering::None
    };
    let size = source.len() as u64;
    let mut content = source.to_owned();
    // Raw in-memory selectors retain split("\n")'s final empty address.
    if sel.raw && sel.multi_ranges.is_empty() && (content.is_empty() || content.ends_with('\n')) {
        content.push('\n');
    }
    let mut window = read_window(
        std::io::Cursor::new(content.as_bytes()),
        size,
        display,
        sel,
        ReadRender {
            default_limit: None,
            numbering,
            entity_label: Some("document"),
            text_resource: true,
            ignore_result_limits: !sel.multi_ranges.is_empty(),
            block_context: (!sel.raw && !sel.multi_ranges.is_empty()).then_some((source, path)),
        },
        cancel,
    )
    .map_err(ToolError)?;
    let lines = if sel.raw {
        source.split('\n').collect::<Vec<_>>()
    } else {
        source.split_terminator('\n').collect::<Vec<_>>()
    };
    if !sel.raw && sel.multi_ranges.is_empty() && window.oversized_first_line.is_none() {
        let selected = window
            .conflict_lines
            .iter()
            .map(|(line, _)| *line as u32)
            .collect::<Vec<_>>();
        if let Some((rendered, context_lines)) = multi_block_context(
            source,
            Some(path),
            &selected,
            numbering,
            usize::MAX,
            usize::MAX,
        ) {
            let body = window
                .conflict_lines
                .iter()
                .map(|(line, text)| {
                    (
                        *line as u32,
                        numbered_multi_row(*line as u32, text, numbering),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            let original_body = render_multi_rows(&body);
            if let Some(notices) = window.text.strip_prefix(&original_body) {
                window.text = format!("{rendered}{notices}");
                window.conflict_lines = context_lines
                    .iter()
                    .map(|line| (*line as usize, lines[*line as usize - 1].to_owned()))
                    .collect();
            }
        }
    }
    let display_content = if !sel.raw && window.oversized_first_line.is_none() {
        let selected = window
            .conflict_lines
            .iter()
            .map(|(line, _)| *line as u32)
            .collect::<Vec<_>>();
        resource_rows_display(&lines, &selected)
    } else {
        resource_display_content(&lines, sel, &window, numbering)
    };
    if let Some((start, plain, numbers)) = display_content {
        window.details["displayContent"] =
            json!({"text":plain,"startLine":start,"lineNumbers":numbers});
    }
    window
        .details
        .as_object_mut()
        .expect("read details object")
        .remove("fileSize");
    window.details["resolvedPath"] = json!(path.to_string_lossy());
    window.details["meta"] = json!({"source":{"type":"path","value":path.to_string_lossy()}});
    if ctx.read_render_markdown() {
        window.details["contentType"] = json!("text/markdown");
    }
    if window.emitted > 0 && window.oversized_first_line.is_none() {
        let key = ara_edit::path_policy::canonical_key(path);
        if numbering == Numbering::Hashline {
            let normalized = ara_edit::text::normalize_to_lf(source);
            let tag = ctx.edit_store.record(&key, &normalized, None);
            let header = ara_edit::modes::hashline::format::format_hashline_header(
                &ctx.hashline_display(path),
                &tag,
            );
            window.text = format!("{header}\n{}", window.text);
            let seen = ara_edit::store::seen_lines_from_body(&window.text);
            if !seen.is_empty() {
                ctx.edit_store.record_seen_lines(&key, &tag, &seen);
            }
        } else if sel.raw {
            let seen = window.raw_seen_lines.clone().unwrap_or_else(|| {
                (window.start..window.start + window.emitted)
                    .map(|line| line as u32)
                    .collect()
            });
            // Provenance is from converted text, not unrelated original PDF bytes.
            let normalized = ara_edit::text::normalize_to_lf(source);
            ctx.edit_store.record(&key, &normalized, Some(&seen));
        }
    }
    Ok(ToolOutput::text(window.text).with_details(window.details))
}
