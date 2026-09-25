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
//! continuation notice. `.ipynb` files read as pi-edit's editable cell text
//! (`notebookToEditableText`), so their tags match what `edit` sees.
//!
//! Not ported (open): structural summaries and bracket context around
//! ranges, multi-range selectors, archives, SQLite, PDFs, URLs, internal
//! URIs, suffix path resolution, `:conflicts`, video, column caps, artifact
//! spill.

use crate::{DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, ToolContext, format_bytes};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{ImageContent, JsonObject, Tool, UserBlock};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;
use tokio_util::sync::CancellationToken;

const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_DIR_ENTRIES: usize = 500;
const SNIFF_BYTES: usize = 8192;
/// Bytes scanned past the emitted window to count remaining lines.
const MAX_SCAN_BYTES: u64 = 256 * 1024 * 1024;
const READ_DESCRIPTION: &str = "Read a local file or directory via `path`. Append a selector to `path` to read part of a file: `:50` (from line 50), `:50-200` (inclusive), `:50+150` (150 lines from 50), `:-60` (last 60 lines), `:raw` (verbatim), or a range with raw (`:raw:2-4`). Encode a literal `:` in a path as `%3A`. Directories return a listing; images return image content. Output is capped at 3000 lines / 50KB; follow the continuation notice to read more.";
/// Hashline mode adds the snapshot-header rule (`read.md`, `IS_HL_MODE`).
const READ_DESCRIPTION_HASHLINE: &str = "Read a local file or directory via `path`. Append a selector to `path` to read part of a file: `:50` (from line 50), `:50-200` (inclusive), `:50+150` (150 lines from 50), `:-60` (last 60 lines), `:raw` (verbatim, no anchors), or a range with raw (`:raw:2-4`). Encode a literal `:` in a path as `%3A`. Files return a `[path#TAG]` snapshot header plus `LINE:TEXT` numbered lines; copy `[FILENAME#TAG]` for anchored edits and NEVER fabricate the tag. Directories return a listing; images return image content. Output is capped at 3000 lines / 50KB; follow the continuation notice to read more.";
const SELECTOR_HELP: &str =
    "Use :N, :N-M, :N+K, :N- (open-ended), :-N (last N lines), :raw, or a range combined with raw (e.g. :raw:50-100).";

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
    pub raw: bool,
}

fn parse_range(sel: &str) -> Option<Range> {
    if let Some(n) = sel.strip_prefix('-') {
        return n.parse::<usize>().ok().filter(|n| *n > 0).map(Range::Tail);
    }
    if let Some((a, b)) = sel.split_once('+') {
        let start = a.parse::<usize>().ok().filter(|n| *n > 0)?;
        let count = b.parse::<usize>().ok().filter(|n| *n > 0)?;
        return Some(Range::From(start, Some(start + count - 1)));
    }
    if let Some((a, b)) = sel.split_once('-') {
        let start = a.parse::<usize>().ok().filter(|n| *n > 0)?;
        if b.is_empty() {
            return Some(Range::From(start, None));
        }
        let end = b.parse::<usize>().ok()?;
        return (end >= start).then_some(Range::From(start, Some(end)));
    }
    sel.parse::<usize>().ok().filter(|n| *n > 0).map(|n| Range::From(n, None))
}

fn looks_like_selector(sel: &str) -> bool {
    !sel.is_empty() && sel.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+')
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
        } else if selector.range.is_none() && looks_like_selector(tail) {
            match parse_range(tail) {
                Some(r) => selector.range = Some(r),
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
fn path_exists(p: &Path) -> bool {
    match std::fs::symlink_metadata(p) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
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

/// Stream the requested line window from `reader` (a file or in-memory
/// bytes of `size`); blocking, checks `cancel` while scanning.
fn read_window<R: BufRead + Seek>(
    mut reader: R,
    size: u64,
    display: &str,
    sel: &Selector,
    numbering: Numbering,
    cancel: &CancellationToken,
) -> Result<Window, String> {
    let io = |e: std::io::Error| format!("Cannot read {display}: {e}");
    let mut head = vec![0u8; SNIFF_BYTES];
    let n = read_up_to(&mut reader, &mut head).map_err(io)?;
    head.truncate(n);
    if !sel.raw && sniff_binary(&head) {
        return Ok(Window {
            text: format!(
                "[Cannot read binary file '{display}' ({}); not valid UTF-8 text. Use ':raw' to read bytes verbatim.]",
                format_bytes(size)
            ),
            details: json!({"fileSize": size}),
            emitted: 0,
            start: 1,
            oversized_first_line: None,
        });
    }
    reader.rewind().map_err(io)?;
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
    let (start, end) = match (&sel.range, known_total) {
        (None, _) => (1usize, None),
        (Some(Range::Tail(n)), Some(total)) => (total.saturating_sub(*n) + 1, None),
        (Some(Range::Tail(_)), None) => unreachable!(),
        (Some(Range::From(s, e)), _) => (*s, *e),
    };

    let mut out = String::new();
    let mut emitted = 0usize;
    let mut line_no = 0usize;
    let mut truncated_by: Option<&str> = None;
    let mut oversized_line: Option<(usize, usize)> = None;
    let mut buf = Vec::new();
    let mut scanned_after = 0u64;
    let mut reached_eof = false;
    loop {
        buf.clear();
        let k = reader.read_until(b'\n', &mut buf).map_err(io)?;
        if k == 0 {
            reached_eof = true;
            break;
        }
        line_no += 1;
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
        let line = line.strip_suffix('\r').filter(|_| !sel.raw).unwrap_or(line);
        let rendered = match numbering {
            _ if sel.raw => line.to_string(),
            Numbering::Pipe => format!("{line_no}|{line}"),
            Numbering::Hashline => format!("{line_no}:{line}"),
            Numbering::None => line.to_string(),
        };
        if emitted >= DEFAULT_MAX_LINES {
            truncated_by = Some("lines");
            continue;
        }
        if out.len() + rendered.len() + 1 > DEFAULT_MAX_BYTES {
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
    if emitted == 0 {
        let Some(total) = total else { return Err(format!("Cannot read {display}: scan budget exceeded")) };
        if total == 0 && sel.range.is_none() {
            return Ok(Window {
                text: String::new(),
                details: json!({"totalLines": 0, "fileSize": size}),
                emitted: 0,
                start,
                oversized_first_line: None,
            });
        }
        let suggestion = if total == 0 {
            "The file is empty.".to_string()
        } else {
            format!("Use :1 to read from the start, or :{total} to read the last line.")
        };
        return Ok(Window {
            text: format!("Line {start} is beyond end of file ({total} lines total). {suggestion}"),
            details: json!({"totalLines": total, "fileSize": size}),
            emitted: 0,
            start,
            oversized_first_line: None,
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
                "\n\n[{} more lines in file. Use :{} to continue]",
                t - last_shown,
                last_shown + 1
            )),
            None => out.push_str(&format!(
                "\n\n[More lines in file ({} total; not scanned to EOF). Use :{} to continue]",
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
    Ok(Window { text: out, details, emitted, start, oversized_first_line: oversized_line })
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
        let (path, sel) = split_selector(input, |p| path_exists(&self.ctx.resolve(p))).map_err(ToolError)?;
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
            let mut names = Vec::new();
            let mut rd =
                tokio::fs::read_dir(&abs).await.map_err(|e| ToolError(format!("Cannot list {display}: {e}")))?;
            while let Ok(Some(entry)) = rd.next_entry().await {
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                names.push(format!("{}{}", entry.file_name().to_string_lossy(), if is_dir { "/" } else { "" }));
            }
            names.sort();
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
        if !sel.raw
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
        let hashlines = self.ctx.hashlines() && !sel.raw;
        let notebook = !sel.raw && abs.extension().is_some_and(|e| e.eq_ignore_ascii_case("ipynb"));
        let raw = sel.raw;
        let line_numbers = self.ctx.line_numbers;
        let store = self.ctx.edit_store.clone();
        let ctx = self.ctx.clone();
        let (abs2, display2, cancel2) = (abs.clone(), display.clone(), cancel.clone());
        let file_size = meta.len();
        let (window, text) = tokio::task::spawn_blocking(move || -> Result<(Window, String), String> {
            let io = |e: std::io::Error| format!("Cannot read {display2}: {e}");
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
                    pi_edit::notebook::notebook_to_editable_text(&json, &display2).map_err(|e| e.to_string())?;
                Some(pi_edit::text::normalize_to_lf(&cells).into_owned())
            } else if hashlines && file_size <= pi_edit::store::MAX_SNAPSHOT_FILE_BYTES {
                match String::from_utf8(std::fs::read(&abs2).map_err(io)?) {
                    Ok(text) => Some(pi_edit::text::normalize_to_lf(pi_edit::text::strip_bom(&text).1).into_owned()),
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
            let window = match (&editable, &undecodable) {
                (Some(text), _) => read_window(
                    std::io::Cursor::new(text.as_bytes()),
                    text.len() as u64,
                    &display2,
                    &sel,
                    numbering,
                    &cancel2,
                )?,
                (None, Some(bytes)) => read_window(
                    std::io::Cursor::new(bytes.as_slice()),
                    bytes.len() as u64,
                    &display2,
                    &sel,
                    numbering,
                    &cancel2,
                )?,
                (None, None) => {
                    let file = std::fs::File::open(&abs2).map_err(io)?;
                    let size = file.metadata().map_err(io)?.len();
                    read_window(BufReader::with_capacity(64 * 1024, file), size, &display2, &sel, numbering, &cancel2)?
                }
            };
            let mut text = window.text.clone();
            if window.emitted > 0 && window.oversized_first_line.is_none() {
                let key = pi_edit::path_policy::canonical_key(&abs2);
                if let (true, Some(content)) = (taggable, &editable) {
                    let tag = store.record(&key, content, None);
                    let header =
                        pi_edit::modes::hashline::format::format_hashline_header(&ctx.hashline_display(&abs2), &tag);
                    text = format!("{header}\n{text}");
                    let seen = pi_edit::store::seen_lines_from_body(&text);
                    if !seen.is_empty() {
                        store.record_seen_lines(&key, &tag, &seen);
                    }
                } else if raw {
                    // A raw read has no header, but records the range it showed so
                    // a same-content hashline tag inherits its provenance.
                    let seen: Vec<u32> = (window.start..window.start + window.emitted).map(|n| n as u32).collect();
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
        Ok(ToolOutput::text(text).with_details(details))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors() {
        let none = |_: &str| false;
        let ok = |s: &str| split_selector(s, none).unwrap();
        assert_eq!(ok("a.txt:5-9"), ("a.txt".into(), Selector { range: Some(Range::From(5, Some(9))), raw: false }));
        assert_eq!(ok("a.txt:5+3").1.range, Some(Range::From(5, Some(7))));
        assert_eq!(ok("a.txt:-4").1.range, Some(Range::Tail(4)));
        assert_eq!(ok("a.txt:7-").1.range, Some(Range::From(7, None)));
        assert_eq!(ok("a.txt:raw:2-4"), ("a.txt".into(), Selector { range: Some(Range::From(2, Some(4))), raw: true }));
        assert_eq!(ok("a.txt:2-4:raw").1, Selector { range: Some(Range::From(2, Some(4))), raw: true });
        assert_eq!(ok("dir/a%3Ab.txt").0, "dir/a:b.txt");
        assert_eq!(ok("weird:name"), ("weird:name".into(), Selector::default()));
        assert_eq!(split_selector("x:1", |p| p == "x:1").unwrap(), ("x:1".into(), Selector::default()));
        for bad in ["a.txt:0", "a.txt:9-3", "a.txt:-0", "a.txt:1+0"] {
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
}
