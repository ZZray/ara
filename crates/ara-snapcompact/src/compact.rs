//! Fixed OMP coherent re-rendering, text edges and HQ/LQ/HQ archive planning.
use crate::layout::{dim_open, doc_pages, paginate_cells};
use crate::text::{elide_data_urls, utf16_slice};
use crate::{
    Archive, CompactionOptions, CompactionPreparation, CompactionResult, DIM_ON, Error, Frame, HQ_EDGE_FRAMES,
    NormalizeOptions, Result, Shape,
};
use ara_prompt::js::utf16_len;
use serde_json::{Map, Value, json};

struct Layout {
    frames: Vec<(String, Shape)>,
    head: String,
    tail: String,
    kept: String,
    truncated: usize,
}
fn planned(pages: &[String], shape: &Shape) -> Vec<(String, Shape)> {
    pages.iter().map(|text| (text.clone(), shape.clone())).collect()
}
fn slice_index(value: f64, length: usize) -> usize {
    if value.is_nan() {
        return 0;
    }
    let value = value.trunc();
    if value < 0.0 { (length as f64 + value).max(0.0) as usize } else { value.min(length as f64) as usize }
}
fn plan_archive(text: &str, high: &Shape, low: &Shape, max_frames: f64) -> Layout {
    let cap_high = crate::geometry(high, None).capacity;
    let length = utf16_len(text);
    if length <= 2 * cap_high {
        return Layout { frames: Vec::new(), head: text.into(), tail: String::new(), kept: text.into(), truncated: 0 };
    }
    let head = utf16_slice(text, 0, cap_high);
    let tail = utf16_slice(text, length - cap_high, length);
    if max_frames < 1.0 {
        let truncated = length - utf16_len(&head) - utf16_len(&tail);
        return Layout { frames: Vec::new(), kept: format!("{head}{tail}"), head, tail, truncated };
    }
    let image = utf16_slice(text, cap_high, length - cap_high);
    if image.is_empty() {
        return Layout { frames: Vec::new(), head: text.into(), tail: String::new(), kept: text.into(), truncated: 0 };
    }
    if high.columns == Some(2) {
        let pages = doc_pages(&image, crate::geometry(high, None), high.font != "silver");
        let mut kept = pages.clone();
        let mut truncated = 0;
        if pages.len() as f64 > max_frames {
            let end = slice_index(pages.len() as f64 - (max_frames - 1.0), pages.len());
            truncated = pages[1..end].iter().map(|s| utf16_len(s)).sum();
            kept = pages[..1].iter().chain(&pages[end..]).cloned().collect();
        }
        let flat = kept.iter().map(|s| s.replace('\n', " ")).collect::<Vec<_>>().join(" ");
        return Layout { frames: planned(&kept, high), kept: format!("{head}{flat}{tail}"), head, tail, truncated };
    }
    let high_geo = crate::geometry(high, None);
    let high_pages = paginate_cells(&image, cap_high, high_geo.cols, high.font != "silver");
    if high_pages.len() as f64 <= max_frames {
        return Layout { frames: planned(&high_pages, high), head, tail, kept: text.into(), truncated: 0 };
    }
    let edge_frames =
        if max_frames.is_nan() { 0 } else { ((max_frames - 1.0) / 2.0).floor().min(HQ_EDGE_FRAMES as f64) as usize };
    let head_pages = &high_pages[..edge_frames];
    let tail_pages = if edge_frames > 0 { &high_pages[high_pages.len() - edge_frames..] } else { &[] };
    let image_head = head_pages.concat();
    let image_tail = tail_pages.concat();
    let middle_source = utf16_slice(&image, utf16_len(&image_head), utf16_len(&image) - utf16_len(&image_tail));
    let low_geo = crate::geometry(low, None);
    let mut middle_pages = paginate_cells(&middle_source, low_geo.capacity, low_geo.cols, low.font != "silver");
    let budget = max_frames - 2.0 * edge_frames as f64;
    let mut truncated = 0;
    let mut middle = middle_source.clone();
    if middle_pages.len() as f64 > budget {
        let first_kept = slice_index(middle_pages.len() as f64 - budget, middle_pages.len());
        truncated = utf16_len(&middle_pages[..first_kept].concat());
        middle = utf16_slice(&middle_source, truncated, utf16_len(&middle_source));
        middle_pages = middle_pages.split_off(first_kept);
    }
    let mut frames = planned(head_pages, high);
    frames.extend(planned(&middle_pages, low));
    frames.extend(planned(tail_pages, high));
    Layout { frames, kept: format!("{head}{image_head}{middle}{image_tail}{tail}"), head, tail, truncated }
}
fn strip_thinking_sections(text: &str) -> String {
    text.split(crate::NEWLINE_GLYPH)
        .map(|segment| {
            let mut sections = Vec::new();
            let mut start = 0;
            for (at, _) in segment.match_indices("\n\n") {
                if ["¶user:", "¶think:", "¶ai:", "¶call:"].iter().any(|prefix| segment[at + 2..].starts_with(prefix))
                {
                    sections.push(&segment[start..at]);
                    start = at + 2;
                }
            }
            sections.push(&segment[start..]);
            sections.into_iter().filter(|s| !s.starts_with("¶think:")).collect::<Vec<_>>().join("\n\n")
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(&crate::NEWLINE_GLYPH.to_string())
}
pub fn compact(preparation: &CompactionPreparation, options: &CompactionOptions) -> Result<CompactionResult> {
    if preparation.first_kept_entry_id.is_empty() {
        return Err(Error::from_reason("First kept entry has no ID - session may need migration"));
    }
    let messages: Vec<_> =
        preparation.messages_to_summarize.iter().chain(&preparation.turn_prefix_messages).cloned().collect();
    let serialized = crate::serialize_conversation(&messages, &options.serialize);
    let previous = crate::get_preserved_archive(preparation.previous_preserve_data.as_ref());
    let previous_raw = previous
        .as_ref()
        .map(|a| {
            a.text.clone().unwrap_or_else(|| {
                [&a.text_head, &a.text_tail]
                    .into_iter()
                    .filter_map(|s| s.as_deref())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(&crate::NEWLINE_GLYPH.to_string())
            })
        })
        .unwrap_or_default();
    let healed = elide_data_urls(&previous_raw, true);
    let previous_text = if !options.serialize.include_thinking && !healed.is_empty() {
        strip_thinking_sections(&healed)
    } else {
        healed
    };
    let has_previous = !previous_text.is_empty();
    let included_summary = !has_previous && preparation.previous_summary.as_ref().is_some_and(|s| !s.is_empty());
    let probe = crate::renderability_probe_text(
        &serialized,
        preparation.previous_preserve_data.as_ref(),
        preparation.previous_summary.as_deref(),
    );
    let mut high = match options.shape.as_ref() {
        Some(s) => s.clone(),
        None => crate::resolve_shape_for_text(&probe, options.model.as_ref(), None)?,
    };
    if let Some(size) = options.frame_size {
        high.frame_size = f64::from(size);
    }
    let low = crate::shape::dense_companion(&high, options.model.as_ref().and_then(|m| m.api.as_deref()));
    let geo = crate::geometry(&high, None);
    let max_frames =
        options.max_frames.unwrap_or(crate::MAX_FRAMES_DEFAULT as f64).clamp(1.0, crate::MAX_FRAMES_DEFAULT as f64);
    let mut text = crate::normalize(&serialized, NormalizeOptions { shape: Some(high.clone()), ..Default::default() });
    if included_summary && let Some(summary) = &preparation.previous_summary {
        let head = format!(
            "[Summary of earlier history] {}",
            crate::normalize(summary, NormalizeOptions { shape: Some(high.clone()), ..Default::default() })
        );
        text = if text.is_empty() { head } else { format!("{head} [Recent conversation] {text}") };
    }
    if has_previous {
        text = if text.is_empty() { previous_text } else { format!("{previous_text}{}{text}", crate::NEWLINE_GLYPH) };
    }
    text = elide_data_urls(&text, false);
    let layout = plan_archive(&text, &high, &low, max_frames);
    let truncated = previous.as_ref().map(|a| a.truncated_chars).unwrap_or(0.0) + layout.truncated as f64;
    let mut dim = dim_open(&layout.head);
    let mut jobs = Vec::new();
    for (page, shape) in &layout.frames {
        let mut text = if dim { format!("{DIM_ON}{page}") } else { page.clone() };
        dim = dim_open(&text);
        if shape.stopword_dim == Some(true) {
            text = crate::dim_stopwords(&text);
        }
        jobs.push((text, shape.clone()));
    }
    let tail = if layout.tail.is_empty() {
        String::new()
    } else if dim {
        format!("{DIM_ON}{}", layout.tail)
    } else {
        layout.tail.clone()
    };
    let text_chars = utf16_len(&layout.head) + utf16_len(&tail);
    let rendered = crate::layout::render_pages(&jobs, None)?;
    let mut frames = Vec::with_capacity(rendered.len());
    for (rendered, (_, shape)) in rendered.into_iter().zip(&jobs) {
        let mut metadata = Map::new();
        metadata.insert("font".into(), json!(shape.font));
        metadata.insert("variant".into(), json!(shape.variant));
        metadata.insert("lineRepeat".into(), json!(shape.line_repeat));
        if shape.columns == Some(2) {
            metadata.insert("columns".into(), json!(2));
        }
        if shape.stopword_dim == Some(true) {
            metadata.insert("stopwordDim".into(), json!(true));
        }
        if let Some(detail) = &shape.image_detail {
            metadata.insert("detail".into(), json!(detail));
        }
        frames.push(Frame {
            data: rendered.data,
            mime_type: "image/png".into(),
            cols: rendered.cols as f64,
            rows: rendered.rows as f64,
            chars: rendered.chars as f64,
            metadata,
        });
    }
    let total_chars = frames.iter().map(|f| f.chars).sum::<f64>() + text_chars as f64;
    let mut cols = Vec::new();
    for frame in &frames {
        if !cols.contains(&frame.cols) {
            cols.push(frame.cols);
        }
    }
    let summary_cols = if cols.is_empty() {
        geo.cols.to_string()
    } else {
        cols.iter().map(|v| ara_prompt::js::f64_to_string(*v)).collect::<Vec<_>>().join(" or ")
    };
    let details = crate::compute_file_lists(&preparation.file_ops);
    let files =
        crate::files::format_file_list(&details.read_files, &details.modified_files, Some(&preparation.file_ops.read));
    let summary = if frames.is_empty() && layout.head.is_empty() && tail.is_empty() && files.is_empty() {
        "No prior history.".into()
    } else {
        ara_prompt::render(include_str!("prompts/snapcompact-summary.md"), &json!({ "frameCount": frames.len(), "multipleFrames": frames.len() > 1, "docColumns": high.columns == Some(2), "cols": summary_cols, "rows": geo.rows, "sentenceInk": high.variant == "sent", "stopwordDimmed": high.stopword_dim == Some(true), "lineRepeated": high.line_repeat > 1.0, "truncatedChars": truncated, "includedPreviousSummary": included_summary, "files": if files.is_empty() { Value::Null } else { Value::String(files) } })).map_err(|error| Error::from_reason(error.to_string()))?
    };
    let kept = if !layout.kept.is_empty() && !layout.tail.is_empty() {
        format!("{}{}", utf16_slice(&layout.kept, 0, utf16_len(&layout.kept) - utf16_len(&layout.tail)), tail)
    } else {
        layout.kept
    };
    let frame_count = frames.len();
    let archive = Archive {
        frames,
        total_chars,
        truncated_chars: truncated,
        text: (!kept.is_empty()).then_some(kept),
        text_head: (!layout.head.is_empty()).then_some(layout.head),
        text_tail: (!tail.is_empty()).then_some(tail),
    };
    let mut preserve = crate::archive::strip_key(preparation.previous_preserve_data.as_ref(), "openaiRemoteCompaction")
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    preserve.insert(
        crate::PRESERVE_KEY.into(),
        serde_json::to_value(archive).map_err(|error| Error::from_reason(error.to_string()))?,
    );
    let text_note = if text_chars > 0 {
        format!(" (+{} chars as text)", crate::archive::comma_number(text_chars as f64))
    } else {
        String::new()
    };
    Ok(CompactionResult {
        summary,
        short_summary: Some(format!(
            "Archived {} chars of history onto {frame_count} snapcompact frame{}{text_note}",
            crate::archive::comma_number(total_chars),
            if frame_count == 1 { "" } else { "s" }
        )),
        first_kept_entry_id: preparation.first_kept_entry_id.clone(),
        tokens_before: preparation.tokens_before,
        details: Some(details),
        preserve_data: Some(Value::Object(preserve)),
    })
}
