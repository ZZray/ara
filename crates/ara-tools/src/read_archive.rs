//! Fixed OMP596f2da tools/read-archive.ts immutable archive READ wrapper.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! SOURCE prepared: registry24 joined; Root Host composition/execution pending.
use super::*;
use crate::read_sqlite::ScopedSuffixMemo;
use crate::write_archive::{ReadArchive, read_archive_path_candidates};
use ara_rpc::WireString;

pub(super) struct ResolvedArchivePath {
    absolute: std::path::PathBuf,
    sub_path: String,
    suffix: Option<(String, String)>,
}
fn missing(error: &std::io::Error) -> bool {
    matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory)
}
pub(super) fn resolve(
    authored: &str,
    memo: &mut ScopedSuffixMemo<'_>,
) -> Result<Option<ResolvedArchivePath>, ToolError> {
    let ctx = memo.context();
    for candidate in read_archive_path_candidates(authored) {
        let mut absolute = ctx.resolve(&candidate.archive_path);
        match std::fs::metadata(&absolute) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) => {
                return Ok(Some(ResolvedArchivePath {
                    absolute,
                    sub_path: if candidate.archive_path == authored { String::new() } else { candidate.sub_path },
                    suffix: None,
                }));
            }
            Err(error) if !missing(&error) => continue,
            Err(_) => {}
        }
        if ctx.read_remote_mount_root().is_some_and(|root| absolute.starts_with(root) && absolute != root) {
            continue;
        }
        let Some((found, display)) = memo.find(&candidate.archive_path)? else {
            continue;
        };
        match std::fs::metadata(&found) {
            Ok(meta) if meta.is_dir() => continue,
            Ok(_) => {
                // Fixed suffix resolution publishes cwd + authored relative
                // spelling, while found remains the native stat target above.
                absolute = ctx.resolve(&display);
                return Ok(Some(ResolvedArchivePath {
                    absolute,
                    sub_path: if candidate.archive_path == authored { String::new() } else { candidate.sub_path },
                    suffix: Some((candidate.archive_path, display)),
                }));
            }
            Err(error) if missing(&error) => {}
            Err(error) => return Err(ToolError(error.to_string())),
        }
    }
    Ok(None)
}
fn aborted(cancel: &CancellationToken) -> Result<(), ToolError> {
    if cancel.is_cancelled() { Err(ToolError("Operation aborted".into())) } else { Ok(()) }
}
fn prepend(text: String, suffix: &Option<(String, String)>) -> String {
    match suffix {
        None => text,
        Some((from, to)) => {
            let notice = format!("[Path '{from}' not found; resolved to '{to}' via suffix match]");
            if text.is_empty() { notice } else { format!("{notice}\n{text}") }
        }
    }
}
// Fixed parseSel differs from local content_selector: image/conflicts and
// unrecognized bare selectors address a whole in-memory member. Recognized
// malformed compounds reject; arbitrary member names are never selectors.
fn parse_archive_selector(selector: Option<&str>) -> Result<(Selector, bool), ToolError> {
    let Some(selector) = selector.filter(|s| !s.is_empty()) else {
        return Ok((Selector::default(), false));
    };
    if selector.eq_ignore_ascii_case("img") || selector.eq_ignore_ascii_case("conflicts") {
        return Ok((Selector::default(), true));
    }
    if selector.eq_ignore_ascii_case("raw") {
        return Ok((Selector { raw: true, ..Selector::default() }, true));
    }
    if let Ok(parsed) = content_selector(Some(selector)) {
        return Ok((parsed, true));
    }
    let chunks: Vec<_> = selector.split(':').collect();
    let read_like = |chunk: &str| {
        if ["raw", "conflicts", "img"].iter().any(|word| chunk.eq_ignore_ascii_case(word))
            || parse_ranges(chunk).is_some()
        {
            return true;
        }
        let Some(tail) = chunk.strip_prefix('-') else {
            return false;
        };
        let parts: Vec<_> = tail.split(['-', '+']).collect();
        (1..=2).contains(&parts.len())
            && parts.iter().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    };
    if chunks.len() > 1 && chunks.iter().all(|chunk| read_like(chunk)) {
        return content_selector(Some(selector)).map(|parsed| (parsed, true));
    }
    Ok((Selector::default(), false))
}
fn archive_directory(
    archive: &ReadArchive,
    absolute: &Path,
    sub_path: &WireString,
    sel: &Selector,
    mut details: Value,
    suffix: &Option<(String, String)>,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let collator = skill_directory_collator()?;
    let all = archive.list_directory(sub_path, |left, right| {
        let left = String::from_utf16_lossy(left.units()).to_lowercase();
        let right = String::from_utf16_lossy(right.units()).to_lowercase();
        collator.compare(&left, &right)
    })?;
    let (start, limit) = match sel.range {
        Some(Range::From(start, end)) => (start.saturating_sub(1), end.map(|end| end - start + 1)),
        Some(Range::Tail(count)) => (all.len().saturating_sub(count), Some(count.min(all.len()).max(1))),
        None => (0, None),
    };
    let limit = limit.unwrap_or(500);
    let available = all.len().saturating_sub(start);
    let mut lines = Vec::new();
    for entry in all.iter().skip(start).take(limit) {
        aborted(cancel)?;
        let name = String::from_utf16_lossy(entry.name.units());
        lines.push(if entry.node.directory {
            format!("{name}/")
        } else if entry.node.size > 0 {
            format!("{name} ({})", format_bytes(entry.node.size))
        } else {
            name
        });
    }
    let text = prepend(if lines.is_empty() { "(empty archive directory)".into() } else { lines.join("\n") }, suffix);
    let truncation = crate::output::truncate_head(&text, usize::MAX, DEFAULT_MAX_BYTES);
    details["isDirectory"] = json!(true);
    details["meta"]["source"] = json!({"type":"path","value":absolute.to_string_lossy()});
    if available >= limit {
        details["meta"]["limits"]["resultLimit"] = json!({"reached":limit,"suggestion":limit.saturating_mul(2)});
    }
    let mut text = truncation.content.clone();
    if truncation.truncated {
        details["truncation"] = truncation.details();
        details["meta"]["truncation"] = json!({
            "direction":"head","truncatedBy":if truncation.by_bytes {"bytes"} else {"lines"},
            "totalLines":truncation.total_lines,"totalBytes":truncation.total_bytes,
            "outputLines":truncation.output_lines,"outputBytes":truncation.output_bytes,
        });
        let mut notice = crate::output::Notice::default();
        notice.truncation(&truncation);
        text.push_str(&notice.render());
    }
    Ok(ToolOutput::text(text).with_details(details))
}
pub(super) fn read(
    ctx: &ToolContext,
    authored: &str,
    resolved: ResolvedArchivePath,
    promoted: Option<Option<&str>>,
    cancel: &CancellationToken,
) -> Result<ToolOutput, ToolError> {
    let (sub_path, selector) = if let Some(selector) = promoted {
        (resolved.sub_path.clone(), selector.map(str::to_owned))
    } else {
        crate::paths::split_path_and_sel(&resolved.sub_path)
    };
    let (mut sel, _) = parse_archive_selector(selector.as_deref())?;
    aborted(cancel)?;
    let archive = ReadArchive::open(&resolved.absolute, ctx)?;
    aborted(cancel)?;
    let mut details = json!({"resolvedPath":resolved.absolute.to_string_lossy()});
    if let Some((from, to)) = &resolved.suffix {
        details["suffixResolution"] = json!({"from":from,"to":to});
    }
    // Fixed Markdown metadata uses the member, before root-selector fallback,
    // and respects read.renderMarkdown default false.
    if ctx.read_render_markdown()
        && Path::new(&sub_path).extension().and_then(|ext| ext.to_str()).is_some_and(|ext| {
            ["md", "markdown", "mdx", "mdc", "mkd", "mdown"].iter().any(|candidate| ext.eq_ignore_ascii_case(candidate))
        })
    {
        details["contentType"] = json!("text/markdown");
    }
    let mut sub_path = WireString::from(sub_path);
    let mut node = archive.get_node(&sub_path)?;
    if node.is_none() && !sub_path.is_empty() {
        let (whole_sel, recognized) = parse_archive_selector(Some(&String::from_utf16_lossy(sub_path.units())))?;
        if recognized {
            sub_path = WireString::from("");
            node = archive.get_node(&sub_path)?;
            sel = whole_sel;
        }
    }
    let node = node.ok_or_else(|| ToolError(format!("Path '{authored}' not found inside archive")))?;
    if node.directory {
        if !sel.multi_ranges.is_empty() {
            return Err(ToolError(
                "Multi-range line selectors are not supported for archive directory listings.".into(),
            ));
        }
        return archive_directory(&archive, &resolved.absolute, &sub_path, &sel, details, &resolved.suffix, cancel);
    }
    let member = archive.read_file(&sub_path)?;
    // Decode the whole selected member, never only a header/sniff window.
    let text = if member.bytes.contains(&0) { None } else { std::str::from_utf8(&member.bytes).ok() };
    let Some(text) = text else {
        details["meta"]["source"] = json!({"type":"path","value":resolved.absolute.to_string_lossy()});
        let name = String::from_utf16_lossy(member.node.path.units());
        return Ok(ToolOutput::text(prepend(
            format!("[Cannot read binary archive entry '{name}' ({})]", format_bytes(member.node.size)),
            &resolved.suffix,
        ))
        .with_details(details));
    };
    // TextDecoder strips a UTF8 BOM; in-memory raw preserves CR/newline bytes.
    let mut text = text.strip_prefix('\u{feff}').unwrap_or(text).to_owned();
    let size = text.len() as u64;
    if sel.raw && sel.multi_ranges.is_empty() && (text.is_empty() || text.ends_with('\n')) {
        text.push('\n');
    }
    let numbering = if ctx.line_numbers && !sel.raw { Numbering::Pipe } else { Numbering::None };
    let uncancelled = CancellationToken::new();
    let mut window = read_window(
        std::io::Cursor::new(text.as_bytes()),
        size,
        authored,
        &sel,
        ReadRender {
            numbering,
            entity_label: Some("archive entry"),
            text_resource: true,
            ignore_result_limits: !sel.multi_ranges.is_empty(),
            block_context: (!sel.raw && !sel.multi_ranges.is_empty())
                .then_some((text.as_str(), Path::new(&String::from_utf16_lossy(sub_path.units())))),
            default_limit: None,
        },
        &uncancelled,
    )
    .map_err(ToolError)?;
    let lines = text.split_terminator('\n').collect::<Vec<_>>();
    if let Some((start, plain, numbers)) = resource_display_content(&lines, &sel, &window, numbering) {
        details["displayContent"] = json!({"text":plain,"startLine":start,"lineNumbers":numbers});
    }
    window.details.as_object_mut().expect("window details").remove("fileSize");
    for (key, value) in window.details.as_object().expect("window details") {
        details[key] = value.clone();
    }
    details["meta"]["source"] = json!({"type":"path","value":resolved.absolute.to_string_lossy()});
    Ok(ToolOutput::text(prepend(window.text, &resolved.suffix)).with_details(details))
}
