//! Fixed OMP persisted archive admission and request-time image byte selection.
use crate::{Archive, Frame, HistoryBlock, HistoryBlockOptions, NEWLINE_GLYPH, PRESERVE_KEY};
use crate::{ArchiveUtf16, FrameUtf16, HistoryBlockUtf16, Result, Utf16Text};
use ara_prompt::js::utf16_len;
use serde_json::Value;
use serde_json::value::RawValue;

fn raw_number(value: &RawValue, name: &str) -> Result<Option<f64>> {
    Ok(crate::raw_json::field(value, name)?.and_then(|v| v.get().trim().parse::<f64>().ok()))
}
fn raw_text(value: &RawValue, name: &str) -> Result<Option<Utf16Text>> {
    crate::raw_json::field(value, name)?.as_deref().map(crate::raw_json::decode_string).transpose().map(Option::flatten)
}
pub fn get_preserved_archive_utf16(preserve: Option<&RawValue>) -> Result<Option<ArchiveUtf16>> {
    let Some(preserve) = preserve else {
        return Ok(None);
    };
    let Some(candidate) = crate::raw_json::field(preserve, PRESERVE_KEY)? else {
        return Ok(None);
    };
    let Some(mut archive) = validate_archive(&candidate)? else {
        return Ok(None);
    };
    if let Some(view) = crate::raw_json::field(&candidate, "withoutThinking")? {
        if let (Some(summary), Some(raw_archive)) =
            (raw_text(&view, "summary")?, crate::raw_json::field(&view, "archive")?)
        {
            if !summary.is_empty()
                && let Some(variant) = validate_archive(&raw_archive)?
            {
                archive.without_thinking = Some(Box::new(crate::ArchiveViewUtf16 { summary, archive: variant }));
            }
        }
    }
    Ok(Some(archive))
}

fn validate_archive(candidate: &RawValue) -> Result<Option<ArchiveUtf16>> {
    if crate::raw_json::object_members(&candidate)?.is_none() {
        return Ok(None);
    }
    let mut frames = Vec::new();
    if let Some(raw_frames) = crate::raw_json::field(&candidate, "frames")? {
        for source in crate::raw_json::array_values(&raw_frames)?.unwrap_or_default() {
            let (Some(data), Some(mime_type), Some(cols), Some(rows), Some(chars)) = (
                raw_text(&source, "data")?,
                raw_text(&source, "mimeType")?,
                raw_number(&source, "cols")?,
                raw_number(&source, "rows")?,
                raw_number(&source, "chars")?,
            ) else {
                continue;
            };
            if data.is_empty() {
                continue;
            }
            frames.push(FrameUtf16 { data, mime_type, cols, rows, chars, source });
        }
    }
    let text = raw_text(&candidate, "text")?.filter(|t| !t.is_empty());
    let text_head = raw_text(&candidate, "textHead")?.filter(|t| !t.is_empty());
    let text_tail = raw_text(&candidate, "textTail")?.filter(|t| !t.is_empty());
    if frames.is_empty() && text.is_none() && text_head.is_none() && text_tail.is_none() {
        return Ok(None);
    }
    Ok(Some(ArchiveUtf16 {
        frames,
        total_chars: raw_number(&candidate, "totalChars")?.unwrap_or(0.0),
        truncated_chars: raw_number(&candidate, "truncatedChars")?.unwrap_or(0.0),
        text,
        text_head,
        text_tail,
        without_thinking: None,
    }))
}
pub fn archive_source_text_utf16(archive: &ArchiveUtf16) -> Option<Utf16Text> {
    let text = archive.text.clone().unwrap_or_else(|| {
        Utf16Text::join(
            &archive.text_head.iter().chain(&archive.text_tail).filter(|t| !t.is_empty()).cloned().collect::<Vec<_>>(),
            &NEWLINE_GLYPH.to_string(),
        )
    });
    (!text.is_empty()).then(|| crate::text::elide_data_urls_utf16(&crate::text::to_plain_text_utf16(&text), true))
}
pub fn renderability_probe_text_utf16(
    serialized: &Utf16Text,
    previous: Option<&ArchiveUtf16>,
    summary: Option<&Utf16Text>,
) -> Utf16Text {
    let prior = previous
        .and_then(archive_source_text_utf16)
        .filter(|t| !t.is_empty())
        .or_else(|| summary.filter(|t| !t.is_empty()).cloned());
    match prior {
        Some(mut text) => {
            text.push_str(&NEWLINE_GLYPH.to_string());
            text.append(serialized);
            text
        }
        None => serialized.clone(),
    }
}
pub(crate) fn archive_raw(archive: &ArchiveUtf16) -> Result<Box<RawValue>> {
    let mut members = vec![
        format!("\"frames\":[{}]", archive.frames.iter().map(|f| f.source.get()).collect::<Vec<_>>().join(",")),
        format!(
            "\"totalChars\":{}",
            serde_json::to_string(&archive.total_chars).map_err(|e| crate::Error::from_reason(e.to_string()))?
        ),
        format!(
            "\"truncatedChars\":{}",
            serde_json::to_string(&archive.truncated_chars).map_err(|e| crate::Error::from_reason(e.to_string()))?
        ),
    ];
    for (key, text) in [("text", &archive.text), ("textHead", &archive.text_head), ("textTail", &archive.text_tail)] {
        if let Some(text) = text {
            members.push(format!("\"{key}\":{}", crate::raw_json::encode_string(text)));
        }
    }
    if let Some(view) = &archive.without_thinking {
        members.push(format!(
            "\"withoutThinking\":{{\"summary\":{},\"archive\":{}}}",
            crate::raw_json::encode_string(&view.summary),
            archive_raw(&view.archive)?.get()
        ));
    }
    crate::raw_json::raw(&format!("{{{}}}", members.join(",")))
}
pub(crate) fn replace_archive_raw(previous: Option<&RawValue>, archive: &ArchiveUtf16) -> Result<Box<RawValue>> {
    let mut members = Vec::new();
    if let Some(previous) = previous {
        for member in crate::raw_json::entries(previous)?.unwrap_or_default() {
            if [PRESERVE_KEY, "openaiRemoteCompaction", "anthropicCompaction"]
                .iter()
                .any(|key| member.key == Utf16Text::from(*key))
            {
                continue;
            }
            members.push(format!("{}:{}", member.key_json, member.value.get()));
        }
    }
    members.push(format!("\"{PRESERVE_KEY}\":{}", archive_raw(archive)?.get()));
    crate::raw_json::raw(&format!("{{{}}}", members.join(",")))
}
pub fn strip_preserved_archive_utf16(preserve: Option<&RawValue>) -> Result<Option<Box<RawValue>>> {
    let Some(preserve) = preserve else {
        return Ok(None);
    };
    let Some(members) = crate::raw_json::object_members(preserve)? else {
        return Ok(Some(crate::raw_json::raw(preserve.get())?));
    };
    if !members.iter().any(|m| m.key == Utf16Text::from(PRESERVE_KEY)) {
        return Ok(Some(crate::raw_json::raw(preserve.get())?));
    }
    let kept: Vec<_> = crate::raw_json::entries(preserve)?
        .unwrap_or_default()
        .into_iter()
        .filter(|m| m.key != Utf16Text::from(PRESERVE_KEY))
        .map(|m| format!("{}:{}", m.key_json, m.value.get()))
        .collect();
    if kept.is_empty() { Ok(None) } else { crate::raw_json::raw(&format!("{{{}}}", kept.join(","))).map(Some) }
}

pub fn get_preserved_archive(preserve: Option<&Value>) -> Option<Archive> {
    let raw = serde_json::value::to_raw_value(preserve?).ok()?;
    let archive = get_preserved_archive_utf16(Some(&raw)).ok()??;
    serde_json::from_str(archive_raw(&archive).ok()?.get()).ok()
}
pub fn strip_preserved_archive(preserve: Option<&Value>) -> Option<Value> {
    strip_key(preserve, PRESERVE_KEY)
}
pub(crate) fn strip_key(preserve: Option<&Value>, key: &str) -> Option<Value> {
    let preserve = preserve?;
    let Some(map) = preserve.as_object() else {
        return Some(preserve.clone());
    };
    if !map.contains_key(key) {
        return Some(preserve.clone());
    }
    let mut rest = map.clone();
    rest.remove(key);
    (!rest.is_empty()).then_some(Value::Object(rest))
}
pub fn archive_source_text(archive: &Archive) -> Option<String> {
    archive_source_text_utf16(&scalar_archive(archive))
        .map(|t| t.to_scalar().expect("scalar archive transforms preserve scalar text"))
}
pub fn renderability_probe_text(
    serialized: &str,
    previous_preserve: Option<&Value>,
    previous_summary: Option<&str>,
) -> String {
    let prior = get_preserved_archive(previous_preserve).map(|a| scalar_archive(&a));
    let summary = previous_summary.map(Utf16Text::from);
    renderability_probe_text_utf16(&Utf16Text::from(serialized), prior.as_ref(), summary.as_ref())
        .to_scalar()
        .expect("scalar probe transforms preserve scalar text")
}
pub fn frame_data_bytes(frames: &[Frame]) -> usize {
    frames.iter().map(|f| utf16_len(&f.data)).sum()
}
pub fn frame_data_bytes_utf16(frames: &[FrameUtf16]) -> usize {
    frames.iter().map(|f| f.data.len()).sum()
}
pub fn images(archive: &Archive) -> Vec<HistoryBlock> {
    scalar_blocks(images_utf16(&scalar_archive(archive)).expect("validated scalar frame"))
}

fn scalar_archive(archive: &Archive) -> ArchiveUtf16 {
    ArchiveUtf16 {
        frames: archive
            .frames
            .iter()
            .map(|f| FrameUtf16 {
                data: Utf16Text::from(f.data.as_str()),
                mime_type: Utf16Text::from(f.mime_type.as_str()),
                cols: f.cols,
                rows: f.rows,
                chars: f.chars,
                source: serde_json::value::to_raw_value(f).expect("scalar frame serializes"),
            })
            .collect(),
        total_chars: archive.total_chars,
        truncated_chars: archive.truncated_chars,
        text: archive.text.clone().map(Utf16Text::from),
        text_head: archive.text_head.clone().map(Utf16Text::from),
        text_tail: archive.text_tail.clone().map(Utf16Text::from),
        without_thinking: archive.without_thinking.as_ref().map(|view| {
            Box::new(crate::ArchiveViewUtf16 {
                summary: Utf16Text::from(view.summary.as_str()),
                archive: scalar_archive(&view.archive),
            })
        }),
    }
}
fn scalar_blocks(blocks: Vec<HistoryBlockUtf16>) -> Vec<HistoryBlock> {
    blocks
        .into_iter()
        .map(|b| match b {
            HistoryBlockUtf16::Text { text } => {
                HistoryBlock::Text { text: text.to_scalar().expect("scalar archive history text") }
            }
            HistoryBlockUtf16::Image { data, mime_type, detail } => HistoryBlock::Image {
                data: data.to_scalar().expect("scalar frame data"),
                mime_type: mime_type.to_scalar().expect("scalar frame mime"),
                detail: detail.map(|v| serde_json::from_str(v.get()).expect("scalar frame metadata")),
            },
        })
        .collect()
}
pub fn images_utf16(archive: &ArchiveUtf16) -> Result<Vec<HistoryBlockUtf16>> {
    archive
        .frames
        .iter()
        .map(|f| {
            let detail = crate::raw_json::field(&f.source, "detail")?.filter(|v| raw_truthy(v));
            Ok(HistoryBlockUtf16::Image { data: f.data.clone(), mime_type: f.mime_type.clone(), detail })
        })
        .collect()
}
fn raw_truthy(value: &RawValue) -> bool {
    let raw = value.get().trim();
    if matches!(raw, "false" | "null") {
        return false;
    }
    if raw.starts_with('"') {
        return crate::raw_json::decode_string(value).ok().flatten().is_some_and(|t| !t.is_empty());
    }
    if let Ok(number) = raw.parse::<f64>() {
        return number != 0.0 && !number.is_nan();
    }
    true
}
pub(crate) fn comma_number(number: f64) -> String {
    let raw = if number.is_finite() {
        if number.fract() == 0.0 {
            format!("{number:.0}")
        } else {
            format!("{number:.3}").trim_end_matches('0').to_owned()
        }
    } else {
        ara_prompt::js::f64_to_string(number)
    };
    let (whole, fraction) = raw.split_once('.').unwrap_or((&raw, ""));
    let sign = if whole.starts_with('-') { "-" } else { "" };
    let whole = whole.trim_start_matches('-');
    let mut result = String::from(sign);
    for (at, ch) in whole.chars().enumerate() {
        if at > 0 && (whole.len() - at).is_multiple_of(3) {
            result.push(',');
        }
        result.push(ch);
    }
    if !fraction.is_empty() {
        result.push('.');
        result.push_str(fraction);
    }
    result
}
fn format_bytes(bytes: usize) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1_000_000.0)
    } else if bytes >= 1_000 {
        format!("{:.1} KB", bytes as f64 / 1_000.0)
    } else {
        format!("{bytes} B")
    }
}
fn omitted_notice(frames: usize, bytes: usize) -> String {
    let budget_note = if bytes > 0 {
        format!(" {} of base64 exceeded the per-request snapcompact payload budget.", format_bytes(bytes))
    } else {
        String::new()
    };
    format!(
        "-------------- snapcompact image middle omitted\n{} archived image frame{} could not be included.{} The compacted summary and visible text edges remain available.\n--------------",
        comma_number(frames as f64),
        if frames == 1 { "" } else { "s" },
        budget_note
    )
}
pub fn history_blocks(archive: &Archive, options: HistoryBlockOptions) -> Vec<HistoryBlock> {
    scalar_blocks(history_blocks_utf16(&scalar_archive(archive), options).expect("validated scalar archive"))
}
pub fn history_blocks_utf16(archive: &ArchiveUtf16, options: HistoryBlockOptions) -> Result<Vec<HistoryBlockUtf16>> {
    history_blocks_core(archive, options, None)
}

pub fn history_blocks_with_resolver_utf16<'a>(
    archive: &ArchiveUtf16,
    options: HistoryBlockOptions,
    mut resolver: impl FnMut(&Utf16Text) -> Result<Option<crate::LazyFrameData<'a>>>,
) -> Result<Vec<HistoryBlockUtf16>> {
    history_blocks_core(archive, options, Some(&mut resolver))
}

enum FrameSlot {
    Frame(usize, Option<Utf16Text>),
    Unavailable,
    Budget(usize),
}

type FrameResolver<'a, 'r> = dyn FnMut(&Utf16Text) -> Result<Option<crate::LazyFrameData<'a>>> + 'r;

fn history_blocks_core(
    archive: &ArchiveUtf16,
    options: HistoryBlockOptions,
    mut resolver: Option<&mut FrameResolver<'_, '_>>,
) -> Result<Vec<HistoryBlockUtf16>> {
    let blob_prefix = Utf16Text::from("blob:sha256:");
    let mut used = 0usize;
    let mut slots = Vec::with_capacity(archive.frames.len());
    for (index, frame) in archive.frames.iter().enumerate().rev() {
        let has_resolver = resolver.is_some();
        let lazy = match resolver.as_mut() {
            Some(resolve) => resolve(&frame.data)?,
            None => None,
        };
        if lazy.is_none() && (has_resolver || frame.data.as_units().starts_with(blob_prefix.as_units())) {
            slots.push(FrameSlot::Unavailable);
            continue;
        }
        let bytes = lazy.as_ref().map_or(frame.data.len(), |payload| payload.bytes);
        if options.max_frame_data_bytes.is_some_and(|budget| bytes > budget.saturating_sub(used)) {
            slots.push(FrameSlot::Budget(bytes));
            continue;
        }
        used = used.saturating_add(bytes);
        slots.push(FrameSlot::Frame(index, lazy.map(|payload| (payload.read)()).transpose()?));
    }
    slots.reverse();
    let mut middle = Vec::new();
    let (mut pending_gap, mut pending_frames, mut pending_bytes) = (None, 0usize, 0usize);
    let flush = |middle: &mut Vec<HistoryBlockUtf16>, gap: &mut Option<bool>, count: &mut usize, bytes: &mut usize| {
        if let Some(unavailable) = gap.take() {
            let notice = if unavailable {
                format!(
                    "-------------- {} archived image frame{} unavailable here --------------",
                    comma_number(*count as f64),
                    if *count == 1 { "" } else { "s" }
                )
            } else {
                omitted_notice(*count, *bytes)
            };
            middle.push(HistoryBlockUtf16::Text { text: Utf16Text::from(notice) });
            *count = 0;
            *bytes = 0;
        }
    };
    for slot in slots {
        if let FrameSlot::Frame(index, data) = slot {
            flush(&mut middle, &mut pending_gap, &mut pending_frames, &mut pending_bytes);
            let frame = &archive.frames[index];
            middle.push(HistoryBlockUtf16::Image {
                data: data.unwrap_or_else(|| frame.data.clone()),
                mime_type: frame.mime_type.clone(),
                detail: crate::raw_json::field(&frame.source, "detail")?.filter(|value| raw_truthy(value)),
            });
        } else {
            let unavailable = matches!(slot, FrameSlot::Unavailable);
            if pending_gap.is_some_and(|gap| gap != unavailable) {
                flush(&mut middle, &mut pending_gap, &mut pending_frames, &mut pending_bytes);
            }
            pending_gap = Some(unavailable);
            pending_frames += 1;
            if let FrameSlot::Budget(bytes) = slot {
                pending_bytes = pending_bytes.saturating_add(bytes);
            }
        }
    }
    flush(&mut middle, &mut pending_gap, &mut pending_frames, &mut pending_bytes);
    let has_images = middle.iter().any(|block| matches!(block, HistoryBlockUtf16::Image { .. }));
    let omitted = middle.iter().any(|block| matches!(block, HistoryBlockUtf16::Text { .. }));
    let mut blocks = Vec::new();
    if let Some(head) = archive.text_head.as_ref().filter(|s| !s.is_empty()) {
        let suffix = if has_images { "\n-------------- imaged middle below\n".into() } else { String::new() };
        let mut text = crate::text::elide_data_urls_utf16(&crate::text::to_plain_text_utf16(head), true);
        text.push_str(&suffix);
        blocks.push(HistoryBlockUtf16::Text { text });
    }
    blocks.extend(middle);
    if let Some(tail) = archive.text_tail.as_ref().filter(|s| !s.is_empty()) {
        let prefix = if has_images {
            "-------------- imaged middle above\n"
        } else if archive.truncated_chars > 0.0 || omitted {
            "\n-------------- middle history omitted above\n"
        } else {
            ""
        };
        let mut text = Utf16Text::from(prefix);
        text.append(&crate::text::elide_data_urls_utf16(&crate::text::to_plain_text_utf16(tail), true));
        if let Some(HistoryBlockUtf16::Text { text: last }) = blocks.last_mut() {
            last.append(&text);
        } else {
            blocks.push(HistoryBlockUtf16::Text { text });
        }
    }
    Ok(blocks)
}

fn cap_blocks<B>(
    blocks: Vec<B>,
    budget: usize,
    image_bytes: impl Fn(&B) -> Option<usize>,
    notice: impl Fn(usize, usize) -> B,
) -> Vec<B> {
    let mut kept = vec![true; blocks.len()];
    let (mut used, mut dropped) = (0usize, false);
    for (index, block) in blocks.iter().enumerate().rev() {
        if let Some(bytes) = image_bytes(block) {
            if bytes > budget.saturating_sub(used) {
                kept[index] = false;
                dropped = true;
            } else {
                used = used.saturating_add(bytes);
            }
        }
    }
    if !dropped {
        return blocks;
    }
    let mut capped = Vec::new();
    let (mut omitted_frames, mut omitted_bytes) = (0, 0);
    for (index, block) in blocks.into_iter().enumerate() {
        if !kept[index] {
            omitted_frames += 1;
            omitted_bytes += image_bytes(&block).expect("only image blocks dropped");
            continue;
        }
        if omitted_frames > 0 {
            capped.push(notice(omitted_frames, omitted_bytes));
            omitted_frames = 0;
            omitted_bytes = 0;
        }
        capped.push(block);
    }
    if omitted_frames > 0 {
        capped.push(notice(omitted_frames, omitted_bytes));
    }
    capped
}

pub fn cap_frame_data_bytes(blocks: Vec<HistoryBlock>, max_frame_data_bytes: usize) -> Vec<HistoryBlock> {
    cap_blocks(
        blocks,
        max_frame_data_bytes,
        |block| match block {
            HistoryBlock::Image { data, .. } => Some(utf16_len(data)),
            _ => None,
        },
        |frames, bytes| HistoryBlock::Text { text: omitted_notice(frames, bytes) },
    )
}

pub fn cap_frame_data_bytes_utf16(
    blocks: Vec<HistoryBlockUtf16>,
    max_frame_data_bytes: usize,
) -> Vec<HistoryBlockUtf16> {
    cap_blocks(
        blocks,
        max_frame_data_bytes,
        |block| match block {
            HistoryBlockUtf16::Image { data, .. } => Some(data.len()),
            _ => None,
        },
        |frames, bytes| HistoryBlockUtf16::Text { text: Utf16Text::from(omitted_notice(frames, bytes)) },
    )
}

pub(crate) fn source_for_reader_without_thinking(archive: &ArchiveUtf16) -> Utf16Text {
    if let Some(text) = archive.text.as_ref().filter(|text| !text.is_empty()) {
        return text.clone();
    }
    let tail = archive.text_tail.clone().unwrap_or_default();
    let tail = crate::text::section_starts_utf16(&tail)
        .first()
        .map(|&start| tail.slice(start, tail.len()))
        .unwrap_or_default();
    Utf16Text::join(
        &archive.text_head.iter().cloned().chain((!tail.is_empty()).then_some(tail)).collect::<Vec<_>>(),
        &NEWLINE_GLYPH.to_string(),
    )
}

pub fn view_without_thinking_utf16(
    summary: &Utf16Text,
    archive: &ArchiveUtf16,
    details: Option<&crate::CompactionDetails>,
) -> Result<crate::ArchiveViewUtf16> {
    if let Some(view) = &archive.without_thinking {
        return Ok((**view).clone());
    }
    if !summary.as_units().windows(7).any(|units| units == [182, 116, 104, 105, 110, 107, 58]) {
        return Ok(crate::ArchiveViewUtf16 { summary: summary.clone(), archive: archive.clone() });
    }
    let text = crate::text::text_without_thinking_utf16(&source_for_reader_without_thinking(archive));
    let head_chars = archive.text_head.as_ref().map_or(0, Utf16Text::len);
    let tail_chars = archive.text_tail.as_ref().map_or(0, Utf16Text::len);
    let fits = head_chars + tail_chars >= text.len();
    let head = if fits { text.clone() } else { text.slice(0, head_chars) };
    let tail =
        if fits || tail_chars == 0 { Utf16Text::default() } else { text.slice(text.len() - tail_chars, text.len()) };
    let truncated = archive.truncated_chars + if fits { 0.0 } else { (text.len() - head_chars - tail_chars) as f64 };
    let files = details
        .map(|details| crate::files::format_file_list(&details.read_files, &details.modified_files, None))
        .unwrap_or_default();
    let summary = if head.is_empty() && files.is_empty() {
        "No prior history.".to_owned()
    } else {
        ara_prompt::render(include_str!("prompts/snapcompact-summary.md"), &serde_json::json!({"frameCount":0,"truncatedChars":truncated,"includedPreviousSummary":text.as_units().starts_with(Utf16Text::from("[Summary of earlier history]").as_units()),"files":if files.is_empty() { serde_json::Value::Null } else { serde_json::Value::String(files) },"includeThinking":false})).map_err(|error| crate::Error::from_reason(error.to_string()))?
    };
    Ok(crate::ArchiveViewUtf16 {
        summary: Utf16Text::from(summary),
        archive: ArchiveUtf16 {
            frames: Vec::new(),
            total_chars: (head.len() + tail.len()) as f64,
            truncated_chars: truncated,
            text: (!text.is_empty()).then_some(text),
            text_head: (!head.is_empty()).then_some(head),
            text_tail: (!tail.is_empty()).then_some(tail),
            without_thinking: None,
        },
    })
}

pub fn view_without_thinking(
    summary: &str,
    archive: &Archive,
    details: Option<&crate::CompactionDetails>,
) -> Result<crate::ArchiveView> {
    let view = view_without_thinking_utf16(&Utf16Text::from(summary), &scalar_archive(archive), details)?;
    Ok(crate::ArchiveView {
        summary: view.summary.to_scalar().map_err(|error| crate::Error::from_reason(error.to_string()))?,
        archive: serde_json::from_str(archive_raw(&view.archive)?.get())
            .map_err(|error| crate::Error::from_reason(error.to_string()))?,
    })
}
