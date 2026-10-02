//! Fixed OMP persisted archive admission and request-time image byte selection.
use crate::{Archive, Frame, HistoryBlock, HistoryBlockOptions, NEWLINE_GLYPH, PRESERVE_KEY};
use ara_prompt::js::{truthy, utf16_len};
use serde_json::Value;

pub fn get_preserved_archive(preserve: Option<&Value>) -> Option<Archive> {
    let value = preserve?.get(PRESERVE_KEY)?;
    if !value.is_object() && !value.is_array() {
        return None;
    }
    let frames = value
        .get("frames")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|frame| {
            if frame.get("data").and_then(Value::as_str).is_none_or(str::is_empty) {
                return None;
            }
            serde_json::from_value::<Frame>(frame.clone()).ok()
        })
        .collect::<Vec<_>>();
    let nonempty = |key| value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let text = nonempty("text");
    let text_head = nonempty("textHead");
    let text_tail = nonempty("textTail");
    if frames.is_empty() && text.is_none() && text_head.is_none() && text_tail.is_none() {
        return None;
    }
    Some(Archive {
        frames,
        total_chars: value.get("totalChars").and_then(Value::as_f64).unwrap_or(0.0),
        truncated_chars: value.get("truncatedChars").and_then(Value::as_f64).unwrap_or(0.0),
        text,
        text_head,
        text_tail,
    })
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
    let text = archive.text.clone().unwrap_or_else(|| {
        [&archive.text_head, &archive.text_tail]
            .into_iter()
            .filter_map(|s| s.as_deref())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(&NEWLINE_GLYPH.to_string())
    });
    (!text.is_empty()).then(|| crate::text::elide_data_urls(&crate::text::to_plain_text(&text), true))
}
pub fn renderability_probe_text(
    serialized: &str,
    previous_preserve: Option<&Value>,
    previous_summary: Option<&str>,
) -> String {
    let previous = get_preserved_archive(previous_preserve).and_then(|a| archive_source_text(&a)).unwrap_or_default();
    if !previous.is_empty() {
        format!("{previous}{NEWLINE_GLYPH}{serialized}")
    } else if let Some(summary) = previous_summary.filter(|s| !s.is_empty()) {
        format!("{summary}{NEWLINE_GLYPH}{serialized}")
    } else {
        serialized.into()
    }
}
pub fn frame_data_bytes(frames: &[Frame]) -> usize {
    frames.iter().map(|f| utf16_len(&f.data)).sum()
}
pub fn images(archive: &Archive) -> Vec<HistoryBlock> {
    archive
        .frames
        .iter()
        .map(|frame| HistoryBlock::Image {
            data: frame.data.clone(),
            mime_type: frame.mime_type.clone(),
            detail: frame.metadata.get("detail").filter(|v| truthy(v)).cloned(),
        })
        .collect()
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
    format!(
        "-------------- snapcompact image middle omitted\n{} archived image frame{} ({} base64) exceeded the per-request snapcompact payload budget. The compacted summary and visible text edges remain available.\n--------------",
        comma_number(frames as f64),
        if frames == 1 { "" } else { "s" },
        format_bytes(bytes)
    )
}
pub fn history_blocks(archive: &Archive, options: HistoryBlockOptions) -> Vec<HistoryBlock> {
    let (mut omitted_frames, mut omitted_bytes) = (0, 0);
    let mut budgeted = archive.clone();
    if let Some(budget) = options.max_frame_data_bytes {
        let mut used = 0;
        let mut kept = Vec::new();
        for frame in archive.frames.iter().rev() {
            let bytes = utf16_len(&frame.data);
            if bytes > budget.saturating_sub(used) {
                omitted_frames += 1;
                omitted_bytes += bytes;
                continue;
            }
            used += bytes;
            kept.push(frame.clone());
        }
        kept.reverse();
        budgeted.frames = kept;
    }
    let selected_images = images(&budgeted);
    let has_images = !selected_images.is_empty();
    let omitted = omitted_frames > 0;
    let mut blocks = Vec::new();
    if let Some(head) = archive.text_head.as_deref().filter(|s| !s.is_empty()) {
        let suffix = if has_images {
            "\n-------------- imaged middle below\n".into()
        } else if omitted {
            format!("\n{}\n", omitted_notice(omitted_frames, omitted_bytes))
        } else {
            String::new()
        };
        blocks.push(HistoryBlock::Text {
            text: crate::text::elide_data_urls(&crate::text::to_plain_text(head), true) + &suffix,
        });
    } else if omitted && !has_images {
        blocks.push(HistoryBlock::Text { text: omitted_notice(omitted_frames, omitted_bytes) });
    }
    if has_images && omitted {
        blocks.push(HistoryBlock::Text { text: omitted_notice(omitted_frames, omitted_bytes) });
    }
    blocks.extend(selected_images);
    if let Some(tail) = archive.text_tail.as_deref().filter(|s| !s.is_empty()) {
        let prefix = if has_images {
            "-------------- imaged middle above\n"
        } else if archive.truncated_chars > 0.0 || omitted {
            "\n-------------- middle history omitted above\n"
        } else {
            ""
        };
        let text = format!("{prefix}{}", crate::text::elide_data_urls(&crate::text::to_plain_text(tail), true));
        if let Some(HistoryBlock::Text { text: last }) = blocks.last_mut() {
            last.push_str(&text);
        } else {
            blocks.push(HistoryBlock::Text { text });
        }
    }
    blocks
}
