//! Canonical in-memory READ receipt (fixed OMP read-format.ts314-598).
//! MIT / Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Profiler summaries share this one exact Core/Native selector/receipt algorithm.
//! The existing PDF document builder is retained unchanged.
//! Wire units survive selection and display; conversion happens only at the
//! original Buffer/native snapshot and the legacy public String boundaries.
use super::*;
use ara_rpc::{WireString, WireValue};

pub(super) struct MemoryOptions {
    pub entity: &'static str,
    pub markdown: bool,
    pub abort: Option<&'static str>,
}
pub(super) struct MemoryReceipt {
    text: WireString,
    details: WireValue,
}
impl MemoryReceipt {
    pub(super) fn legacy(&self) -> ToolOutput {
        // A projection of the exact receipt, never an input to its selectors.
        ToolOutput::text(String::from_utf16_lossy(self.text.units()))
            .with_details(legacy_value(&self.details))
    }
    pub(super) fn into_wire(self) -> WireValue {
        object(vec![("content", WireValue::Array(vec![object(vec![
            ("type", string("text")), ("text", WireValue::String(self.text)),
        ])])), ("details", self.details)])
    }
}
fn object(items: Vec<(&str, WireValue)>) -> WireValue {
    WireValue::Object(items.into_iter().map(|(key, value)| (key.into(), value)).collect())
}
fn string(value: &str) -> WireValue { WireValue::String(value.into()) }
fn number(value: usize) -> WireValue { WireValue::Number(value as f64) }
fn put(target: &mut WireValue, key: &str, value: WireValue) {
    let WireValue::Object(items) = target else { unreachable!("READ details object") };
    if let Some((_, existing)) = items.iter_mut().find(|(name, _)| name.equals_ascii(key)) { *existing = value; }
    else { items.push((key.into(), value)); }
}
fn legacy_value(value: &WireValue) -> Value {
    match value {
        WireValue::Null => Value::Null,
        WireValue::Bool(value) => json!(value),
        WireValue::Number(value) => json!(value),
        WireValue::String(value) => json!(String::from_utf16_lossy(value.units())),
        WireValue::Array(values) => Value::Array(values.iter().map(legacy_value).collect()),
        WireValue::Object(values) => Value::Object(values.iter().map(|(key, value)|
            (String::from_utf16_lossy(key.units()), legacy_value(value))).collect()),
    }
}
fn append(target: &mut WireString, value: &WireString) {
    let mut units = target.units().to_vec();
    units.extend_from_slice(value.units());
    *target = WireString::from_units(units);
}
fn joined(values: &[WireString], separator: &str) -> WireString {
    let mut units = Vec::new();
    for (index, value) in values.iter().enumerate() {
        if index != 0 { units.extend(separator.encode_utf16()); }
        units.extend_from_slice(value.units());
    }
    WireString::from_units(units)
}
fn lines(source: &WireString, raw: bool) -> Vec<WireString> {
    let mut lines = source.units().split(|unit| *unit == 10)
        .map(|units| WireString::from_units(units.to_vec())).collect::<Vec<_>>();
    if !raw && lines.last().is_some_and(WireString::is_empty) { lines.pop(); }
    lines
}
fn count_text_lines(source: &WireString) -> usize {
    if source.is_empty() { 0 } else { 1 + source.units().iter().filter(|unit| **unit == 10).count() }
}
fn buffer_bytes(source: &WireString) -> usize {
    // Buffer.byteLength UTF8 replaces lone units. Do not use the replacement
    // string as a canonical row or selector input.
    String::from_utf16_lossy(source.units()).len()
}
fn snapshot_string(source: &WireString) -> String {
    // Installed fixed-version NAPI observed lone-unit replacement; keep this boundary
    // separate from model/display units and unchanged public EditStore state.
    let native = String::from_utf16_lossy(source.units());
    ara_edit::text::normalize_to_lf(ara_edit::text::strip_bom(&native).1).into_owned()
}
fn numbered(line: usize, source: &WireString, mode: Numbering) -> WireString {
    let mut text = match mode {
        Numbering::None => WireString::from(""),
        Numbering::Pipe => format!("{line}|").into(),
        Numbering::Hashline => format!("{line}:").into(),
    };
    append(&mut text, source);
    text
}
fn row_receipt(all: &[WireString], selected: &[usize], mode: Numbering) -> (WireString, WireValue) {
    let mut model = Vec::new(); let mut display = Vec::new(); let mut numbers = Vec::new();
    let mut previous = None;
    for &line in selected {
        if previous.is_some_and(|before| line != before + 1) {
            model.push(WireString::from("…")); display.push(WireString::from("…")); numbers.push(WireValue::Null);
        }
        model.push(numbered(line, &all[line - 1], mode));
        display.push(all[line - 1].clone()); numbers.push(number(line)); previous = Some(line);
    }
    (joined(&model, "\n"), object(vec![("text", WireValue::String(joined(&display, "\n"))),
        ("startLine", number(selected.first().copied().unwrap_or(1))), ("lineNumbers", WireValue::Array(numbers))]))
}
fn with_context(source: &WireString, all: &[WireString], selected: &[usize], path: &Path) -> Vec<usize> {
    // Select exact units first. Existing native syntax/lexical context is only
    // asked for line identities; its returned String never replaces a row.
    let native_lines = all.iter().map(|line| String::from_utf16_lossy(line.units())).collect::<Vec<_>>();
    let refs = native_lines.iter().map(String::as_str).collect::<Vec<_>>();
    let selected_u32 = selected.iter().map(|line| *line as u32).collect::<Vec<_>>();
    let source_hint = ara_edit::diff_string::BlockContextSource { path: path.to_str(), lang: None, streaming: false };
    let mut rows = selected.iter().copied().collect::<std::collections::BTreeSet<_>>();
    for (line, _) in ara_edit::diff_string::find_block_context_lines(&refs, &selected_u32, &source_hint) {
        if line > 0 && line as usize <= all.len() { rows.insert(line as usize); }
    }
    let _ = source; // Exact full source is retained by the caller/snapshot owner.
    rows.into_iter().collect()
}
struct Truncated {
    content: WireString, by: Option<&'static str>, total_lines: usize, total_bytes: usize,
    output_lines: usize, output_bytes: usize, first_exceeds: bool,
}
fn head(source: &WireString) -> Truncated {
    let total_lines = 1 + source.units().iter().filter(|unit| **unit == 10).count();
    let total_bytes = buffer_bytes(source);
    if total_lines <= DEFAULT_MAX_LINES && total_bytes <= DEFAULT_MAX_BYTES {
        return Truncated { content: source.clone(), by: None, total_lines, total_bytes,
            output_lines: total_lines, output_bytes: total_bytes, first_exceeds: false };
    }
    let raw_lines = lines(source, true);
    let mut kept = Vec::new(); let mut bytes: usize = 0; let mut by = "lines";
    for line in raw_lines {
        if kept.len() >= DEFAULT_MAX_LINES { break; }
        let extra = usize::from(!kept.is_empty()) + buffer_bytes(&line);
        if bytes.saturating_add(extra) > DEFAULT_MAX_BYTES { by = "bytes"; break; }
        bytes += extra; kept.push(line);
    }
    let first_exceeds = kept.is_empty() && by == "bytes";
    Truncated { content: joined(&kept, "\n"), by: Some(by), total_lines, total_bytes,
        output_lines: kept.len(), output_bytes: bytes, first_exceeds }
}
fn truncation_details(truncation: &Truncated) -> WireValue {
    object(vec![("content", WireValue::String(truncation.content.clone())), ("truncated", WireValue::Bool(true)),
        ("truncatedBy", string(truncation.by.unwrap())), ("totalLines", number(truncation.total_lines)),
        ("totalBytes", number(truncation.total_bytes)), ("outputLines", number(truncation.output_lines)),
        ("outputBytes", number(truncation.output_bytes)), ("lastLinePartial", WireValue::Bool(false)),
        ("firstLineExceedsLimit", WireValue::Bool(truncation.first_exceeds))])
}
fn truncation_meta(truncation: &Truncated, start: usize, total: usize) -> WireValue {
    object(vec![("direction", string("head")), ("truncatedBy", string(truncation.by.unwrap())),
        ("totalLines", number(total)), ("totalBytes", number(truncation.total_bytes)),
        ("outputLines", number(truncation.output_lines)), ("outputBytes", number(truncation.output_bytes)),
        ("shownRange", object(vec![("start", number(start)), ("end", WireValue::Number(start as f64 + truncation.output_lines as f64 - 1.0))])),
        ("nextOffset", number(start + truncation.output_lines))])
}
fn raw_receipt(text: &WireString, start: usize) -> WireValue {
    let count = count_text_lines(text);
    object(vec![("text", WireValue::String(text.clone())), ("startLine", number(start)),
        ("lineNumbers", WireValue::Array((start..start + count).map(number).collect()))])
}
fn check_abort(cancel: &CancellationToken, options: &MemoryOptions) -> Result<(), ToolError> {
    if let Some(message) = options.abort && cancel.is_cancelled() { return Err(ToolError(message.into())); }
    Ok(())
}
pub(super) fn build(
    ctx: &ToolContext, path: &Path, sel: &Selector, source: WireString,
    cancel: &CancellationToken, options: MemoryOptions,
) -> Result<MemoryReceipt, ToolError> {
    check_abort(cancel, &options)?;
    let all = lines(&source, sel.raw); let total = all.len();
    let mode = if sel.raw { Numbering::None } else if ctx.hashlines() { Numbering::Hashline }
        else if ctx.line_numbers { Numbering::Pipe } else { Numbering::None };
    let path_text = path.to_string_lossy();
    let mut meta = object(vec![("source", object(vec![("type", string("path")), ("value", string(&path_text))]))]);
    let mut details = object(vec![("totalLines", number(total)), ("resolvedPath", string(&path_text))]);
    if options.markdown { put(&mut details, "contentType", string("text/markdown")); }
    let key = ara_edit::path_policy::canonical_key(path);
    let mut seen = Vec::new(); let mut text;
    let mut header = None;
    // Native snapshot boundary is independent of the exact model/display units.
    let mut hash_context = || {
        let tag = ctx.edit_store.record(&key, &snapshot_string(&source), None);
        (tag.clone(), ara_edit::modes::hashline::format::format_hashline_header(&ctx.hashline_display(path), &tag))
    };
    if !sel.multi_ranges.is_empty() {
        if mode == Numbering::Hashline { header = Some(hash_context()); }
        let mut selected = Vec::new(); let mut raw_parts = Vec::new(); let mut notices = Vec::new();
        for range in &sel.multi_ranges {
            let Range::From(start, end) = range else { unreachable!("multi-range tails are rejected by parser") };
            if *start > total {
                let bound = end.map_or_else(|| start.to_string(), |end| format!("{start}-{end}"));
                notices.push(format!("[Range {bound} is beyond end of {} ({total} lines total); skipped]", options.entity));
                continue;
            }
            let end = end.unwrap_or(total).min(total);
            selected.extend(*start..=end);
            if sel.raw { raw_parts.push(joined(&all[*start - 1..end], "\n")); }
        }
        if sel.raw { text = joined(&raw_parts, "\n\n…\n\n"); seen = selected; }
        else if selected.is_empty() { text = "".into(); }
        else {
            seen = with_context(&source, &all, &selected, path);
            let (rendered, display) = row_receipt(&all, &seen, mode); text = rendered;
            put(&mut details, "displayContent", display);
            if let Some((_, header)) = &header { let mut value: WireString = format!("{header}\n").into(); append(&mut value, &text); text = value; }
        }
        for notice in notices { if !text.is_empty() { text.append_str("\n"); } text.append_str(&notice); }
    } else {
        let (requested_start, limit, explicit_offset) = match &sel.range {
            None => (0, None, false),
            Some(Range::From(start, end)) => (*start - 1, end.map(|end| end - *start + 1), true),
            Some(Range::Tail(count)) => (total.saturating_sub(*count), Some((*count).min(total)), true),
        };
        if requested_start >= total {
            let suggestion = if total == 0 { format!("The {} is empty.", options.entity) }
                else { format!("Use :1 to read from the start, or :{total} to read the last line.") };
            text = format!("Line {} is beyond end of {} ({total} lines total). {suggestion}", requested_start + 1, options.entity).into();
        } else {
            let requested_end = limit.map_or(total, |limit| requested_start.saturating_add(limit).min(total));
            let start = if !sel.raw && explicit_offset { requested_start.saturating_sub(1) } else { requested_start };
            let end = if !sel.raw && limit.is_some() { requested_end.saturating_add(3).min(total) } else { requested_end };
            let selected = joined(&all[start..end], "\n"); let truncation = head(&selected);
            if mode == Numbering::Hashline { header = Some(hash_context()); }
            if truncation.first_exceeds {
                let line = &all[start]; let native = String::from_utf16_lossy(line.units());
                let snippet = WireString::from(cut_at_char_boundary(&native, DEFAULT_MAX_BYTES));
                if mode == Numbering::Hashline {
                    text = format!("[Line {} is {}, exceeds {} limit. Hashline output requires full lines; cannot emit an editable numbered preview for a truncated line.]",
                        start + 1, format_bytes(buffer_bytes(line) as u64), format_bytes(DEFAULT_MAX_BYTES as u64)).into();
                } else {
                    text = numbered(start + 1, &snippet, mode);
                    put(&mut details, "displayContent", raw_receipt(&snippet, start + 1));
                }
                if snippet.is_empty() {
                    text = format!("[Line {} is {}, exceeds {} limit. Unable to display a valid UTF-8 snippet.]", start + 1,
                        format_bytes(buffer_bytes(line) as u64), format_bytes(DEFAULT_MAX_BYTES as u64)).into();
                }
            } else if sel.raw {
                text = truncation.content.clone();
                put(&mut details, "displayContent", raw_receipt(&text, start + 1));
                let count = if truncation.by.is_some() { truncation.output_lines } else { end - start };
                seen.extend(start + 1..start + 1 + count);
            } else {
                let last = if truncation.by.is_some() { start + truncation.output_lines } else { end };
                let selected = (start + 1..=last).collect::<Vec<_>>();
                seen = with_context(&source, &all, &selected, path);
                let (rendered, display) = row_receipt(&all, &seen, mode); text = rendered;
                put(&mut details, "displayContent", display);
                if let Some((_, header)) = &header { let mut value: WireString = format!("{header}\n").into(); append(&mut value, &text); text = value; }
            }
            if truncation.by.is_some() {
                put(&mut details, "truncation", truncation_details(&truncation));
                put(&mut meta, "truncation", truncation_meta(&truncation, start + 1, total));
            } else if limit.is_some() && end < total {
                text.append_str(&format!("\n\n[{} more lines in {}. Use :{} to continue]", total - end, options.entity, end + 1));
            }
        }
    }
    if mode == Numbering::Hashline && let Some((tag, _)) = &header && !seen.is_empty() {
        ctx.edit_store.record_seen_lines(&key, tag, &seen.iter().map(|line| *line as u32).collect::<Vec<_>>());
    } else if sel.raw && !seen.is_empty() {
        ctx.edit_store.record(&key, &snapshot_string(&source), Some(&seen.iter().map(|line| *line as u32).collect::<Vec<_>>()));
    }
    put(&mut details, "meta", meta);
    check_abort(cancel, &options)?;
    Ok(MemoryReceipt { text, details })
}
