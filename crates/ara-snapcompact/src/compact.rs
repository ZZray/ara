//! Fixed OMP coherent re-rendering, text edges and HQ/LQ/HQ archive planning.
use crate::layout::{dim_open_utf16, doc_pages_utf16, paginate_cells_utf16};
use crate::text::{elide_data_urls_utf16, text_without_thinking_utf16};
use crate::{
    ArchiveUtf16, CompactionOptions, CompactionPreparation, CompactionPreparationUtf16, CompactionResult,
    CompactionResultUtf16, DIM_ON, Error, FileOperations, Frame, FrameUtf16, HQ_EDGE_FRAMES, NormalizeOptions, Result,
    Shape, Utf16Text,
};
use serde_json::value::RawValue;
use serde_json::{Map, Value, json};

struct Layout {
    frames: Vec<(Utf16Text, Shape)>,
    head: Utf16Text,
    tail: Utf16Text,
    kept: Utf16Text,
    truncated: usize,
}
fn planned(pages: &[Utf16Text], shape: &Shape) -> Vec<(Utf16Text, Shape)> {
    pages.iter().map(|text| (text.clone(), shape.clone())).collect()
}
fn slice_index(value: f64, length: usize) -> usize {
    if value.is_nan() {
        return 0;
    }
    let value = value.trunc();
    if value < 0.0 { (length as f64 + value).max(0.0) as usize } else { value.min(length as f64) as usize }
}
fn plan_archive(text: &Utf16Text, high: &Shape, low: &Shape, max_frames: f64) -> Layout {
    let cap_high = crate::geometry(high, None).capacity;
    let length = text.len();
    if length <= 2 * cap_high {
        return Layout {
            frames: Vec::new(),
            head: text.clone(),
            tail: Utf16Text::default(),
            kept: text.clone(),
            truncated: 0,
        };
    }
    let head = text.slice(0, cap_high);
    let tail = text.slice(length - cap_high, length);
    if max_frames < 1.0 {
        let truncated = length - head.len() - tail.len();
        return Layout {
            frames: Vec::new(),
            kept: Utf16Text::join(&[head.clone(), tail.clone()], ""),
            head,
            tail,
            truncated,
        };
    }
    let image = text.slice(cap_high, length - cap_high);
    if image.is_empty() {
        return Layout {
            frames: Vec::new(),
            head: text.clone(),
            tail: Utf16Text::default(),
            kept: text.clone(),
            truncated: 0,
        };
    }
    if high.columns == Some(2) {
        let pages = doc_pages_utf16(&image, crate::geometry(high, None), high.font != "silver");
        let mut kept = pages.clone();
        let mut truncated = 0;
        if pages.len() as f64 > max_frames {
            let end = slice_index(pages.len() as f64 - (max_frames - 1.0), pages.len());
            truncated = pages[1..end].iter().map(Utf16Text::len).sum();
            kept = pages[..1].iter().chain(&pages[end..]).cloned().collect();
        }
        let flat = Utf16Text::join(
            &kept
                .iter()
                .map(|s| Utf16Text::from_units(s.as_units().iter().map(|&u| if u == 10 { 32 } else { u }).collect()))
                .collect::<Vec<_>>(),
            " ",
        );
        return Layout {
            frames: planned(&kept, high),
            kept: Utf16Text::join(&[head.clone(), flat, tail.clone()], ""),
            head,
            tail,
            truncated,
        };
    }
    let high_geo = crate::geometry(high, None);
    let high_pages = paginate_cells_utf16(&image, cap_high, high_geo.cols, high.font != "silver");
    if high_pages.len() as f64 <= max_frames {
        return Layout { frames: planned(&high_pages, high), head, tail, kept: text.clone(), truncated: 0 };
    }
    let edge_frames =
        if max_frames.is_nan() { 0 } else { ((max_frames - 1.0) / 2.0).floor().min(HQ_EDGE_FRAMES as f64) as usize };
    let head_pages = &high_pages[..edge_frames];
    let tail_pages = if edge_frames > 0 { &high_pages[high_pages.len() - edge_frames..] } else { &[] };
    let image_head = Utf16Text::join(head_pages, "");
    let image_tail = Utf16Text::join(tail_pages, "");
    let middle_source = image.slice(image_head.len(), image.len() - image_tail.len());
    let low_geo = crate::geometry(low, None);
    let mut middle_pages = paginate_cells_utf16(&middle_source, low_geo.capacity, low_geo.cols, low.font != "silver");
    let budget = max_frames - 2.0 * edge_frames as f64;
    let mut truncated = 0;
    let mut middle = middle_source.clone();
    if middle_pages.len() as f64 > budget {
        let first_kept = slice_index(middle_pages.len() as f64 - budget, middle_pages.len());
        truncated = middle_pages[..first_kept].iter().map(Utf16Text::len).sum();
        middle = middle_source.slice(truncated, middle_source.len());
        middle_pages = middle_pages.split_off(first_kept);
    }
    let mut frames = planned(head_pages, high);
    frames.extend(planned(&middle_pages, low));
    frames.extend(planned(tail_pages, high));
    Layout {
        frames,
        kept: Utf16Text::join(&[head.clone(), image_head, middle, image_tail, tail.clone()], ""),
        head,
        tail,
        truncated,
    }
}
pub fn compact(preparation: &CompactionPreparation, options: &CompactionOptions) -> Result<CompactionResult> {
    compact_with_converter(preparation, options, Ok)
}

/// Convert the concatenated Host message arrays exactly once before serializing.
/// The fixed OMP callback accepts generic Host messages; this Rust boundary
/// carries JSON values and returns the existing package error unchanged.
pub fn compact_with_converter(
    preparation: &CompactionPreparation,
    options: &CompactionOptions,
    converter: impl FnOnce(Vec<Value>) -> Result<Vec<Value>>,
) -> Result<CompactionResult> {
    if preparation.first_kept_entry_id.is_empty() {
        return Err(Error::from_reason("First kept entry has no ID - session may need migration"));
    }
    let messages: Vec<_> =
        preparation.messages_to_summarize.iter().chain(&preparation.turn_prefix_messages).cloned().collect();
    let llm_messages = converter(messages)?;
    let serialized = crate::serialize_conversation_utf16(&llm_messages, &options.serialize);
    let previous_preserve = preparation
        .previous_preserve_data
        .as_ref()
        .map(serde_json::value::to_raw_value)
        .transpose()
        .map_err(|e| Error::from_reason(e.to_string()))?;
    let summary = preparation.previous_summary.clone().map(Utf16Text::from);
    let previous = crate::get_preserved_archive_utf16(previous_preserve.as_deref())?;
    let result = compact_core(
        serialized,
        CorePreparation {
            first_kept_entry_id: &preparation.first_kept_entry_id,
            tokens_before: preparation.tokens_before,
            previous_summary: summary.as_ref(),
            previous_preserve_data: previous_preserve.as_deref(),
            file_ops: &preparation.file_ops,
        },
        previous,
        options,
    )?;
    // Value cannot represent lone UTF16 units. This compatibility boundary
    // checks the exact result; Hosts use compact_utf16 for persistence.
    let preserve_data =
        result.preserve_data.as_deref().map(|raw| serde_json::from_str(raw.get())).transpose().map_err(|e| {
            Error::from_reason(format!("snapcompact scalar result cannot represent exact archive: {e}"))
        })?;
    Ok(CompactionResult {
        summary: result.summary,
        short_summary: result.short_summary,
        first_kept_entry_id: result.first_kept_entry_id,
        tokens_before: result.tokens_before,
        details: result.details,
        preserve_data,
    })
}

pub fn compact_utf16(
    preparation: &CompactionPreparationUtf16,
    options: &CompactionOptions,
) -> Result<CompactionResultUtf16> {
    compact_with_converter_utf16(preparation, options, Ok)
}

/// Restricted JSON message facade with the same validation/conversion order.
pub fn compact_with_converter_utf16(
    preparation: &CompactionPreparationUtf16,
    options: &CompactionOptions,
    converter: impl FnOnce(Vec<Box<RawValue>>) -> Result<Vec<Box<RawValue>>>,
) -> Result<CompactionResultUtf16> {
    if preparation.first_kept_entry_id.is_empty() {
        return Err(Error::from_reason("First kept entry has no ID - session may need migration"));
    }
    let messages = preparation.messages_to_summarize.iter().chain(&preparation.turn_prefix_messages).cloned().collect();
    let converted = converter(messages)?;
    let serialized = crate::serialize_conversation_raw_utf16(&converted, &options.serialize)?;
    let previous = crate::get_preserved_archive_utf16(preparation.previous_preserve_data.as_deref())?;
    compact_core(
        serialized,
        CorePreparation {
            first_kept_entry_id: &preparation.first_kept_entry_id,
            tokens_before: preparation.tokens_before,
            previous_summary: preparation.previous_summary.as_ref(),
            previous_preserve_data: preparation.previous_preserve_data.as_deref(),
            file_ops: &preparation.file_ops,
        },
        previous,
        options,
    )
}

struct CorePreparation<'a> {
    first_kept_entry_id: &'a str,
    tokens_before: f64,
    previous_summary: Option<&'a Utf16Text>,
    previous_preserve_data: Option<&'a RawValue>,
    file_ops: &'a FileOperations,
}
fn compact_core(
    serialized: Utf16Text,
    preparation: CorePreparation<'_>,
    previous: Option<ArchiveUtf16>,
    options: &CompactionOptions,
) -> Result<CompactionResultUtf16> {
    let previous_raw = previous
        .as_ref()
        .map(|a| {
            if !options.serialize.include_thinking {
                return crate::archive::source_for_reader_without_thinking(a);
            }
            a.text.clone().unwrap_or_else(|| {
                Utf16Text::join(
                    &a.text_head.iter().chain(&a.text_tail).filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>(),
                    &crate::NEWLINE_GLYPH.to_string(),
                )
            })
        })
        .unwrap_or_default();
    let healed = elide_data_urls_utf16(&previous_raw, true);
    let previous_text = if !options.serialize.include_thinking && !healed.is_empty() {
        text_without_thinking_utf16(&healed)
    } else {
        healed
    };
    let has_previous = !previous_text.is_empty();
    let included_summary = !has_previous && preparation.previous_summary.as_ref().is_some_and(|s| !s.is_empty());
    let probe = crate::renderability_probe_text_utf16(&serialized, previous.as_ref(), preparation.previous_summary);
    let mut high = match options.shape.as_ref() {
        Some(s) => s.clone(),
        None => crate::resolve_shape_for_text_utf16(&probe, options.model.as_ref(), None)?,
    };
    if let Some(size) = options.frame_size {
        high.frame_size = f64::from(size);
    }
    let low = crate::shape::dense_companion(&high, options.model.as_ref().and_then(|m| m.api.as_deref()));
    let geo = crate::geometry(&high, None);
    let max_frames =
        options.max_frames.unwrap_or(crate::MAX_FRAMES_DEFAULT as f64).clamp(1.0, crate::MAX_FRAMES_DEFAULT as f64);
    let mut text = Utf16Text::from(crate::normalize_utf16(
        &serialized,
        NormalizeOptions { shape: Some(high.clone()), ..Default::default() },
    ));
    if included_summary && let Some(summary) = preparation.previous_summary {
        let summary =
            if options.serialize.include_thinking { summary.clone() } else { text_without_thinking_utf16(summary) };
        let head = Utf16Text::from(format!(
            "[Summary of earlier history] {}",
            crate::normalize_utf16(&summary, NormalizeOptions { shape: Some(high.clone()), ..Default::default() })
        ));
        text = if text.is_empty() { head } else { Utf16Text::join(&[head, text], " [Recent conversation] ") };
    }
    if has_previous {
        text = if text.is_empty() {
            previous_text
        } else {
            Utf16Text::join(&[previous_text, text], &crate::NEWLINE_GLYPH.to_string())
        };
    }
    text = elide_data_urls_utf16(&text, false);
    let layout = plan_archive(&text, &high, &low, max_frames);
    let truncated = previous.as_ref().map(|a| a.truncated_chars).unwrap_or(0.0) + layout.truncated as f64;
    let mut dim = dim_open_utf16(&layout.head);
    let mut jobs = Vec::new();
    for (page, shape) in &layout.frames {
        let mut text =
            if dim { Utf16Text::join(&[Utf16Text::from(DIM_ON.to_string()), page.clone()], "") } else { page.clone() };
        dim = dim_open_utf16(&text);
        if shape.stopword_dim == Some(true) {
            text = crate::text::dim_stopwords_utf16(&text);
        }
        jobs.push((text, shape.clone()));
    }
    let tail = if layout.tail.is_empty() {
        Utf16Text::default()
    } else if dim {
        Utf16Text::join(&[Utf16Text::from(DIM_ON.to_string()), layout.tail.clone()], "")
    } else {
        layout.tail.clone()
    };
    let text_chars = layout.head.len() + tail.len();
    let rendered = crate::layout::render_pages_utf16(&jobs, None)?;
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
        let frame = Frame {
            data: rendered.data,
            mime_type: "image/png".into(),
            cols: rendered.cols as f64,
            rows: rendered.rows as f64,
            chars: rendered.chars as f64,
            metadata,
        };
        frames.push(FrameUtf16 {
            data: Utf16Text::from(frame.data.as_str()),
            mime_type: Utf16Text::from(frame.mime_type.as_str()),
            cols: frame.cols,
            rows: frame.rows,
            chars: frame.chars,
            source: serde_json::value::to_raw_value(&frame).map_err(|e| Error::from_reason(e.to_string()))?,
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
        ara_prompt::render(include_str!("prompts/snapcompact-summary.md"), &json!({ "frameCount": frames.len(), "multipleFrames": frames.len() > 1, "docColumns": high.columns == Some(2), "cols": summary_cols, "rows": geo.rows, "sentenceInk": high.variant == "sent", "stopwordDimmed": high.stopword_dim == Some(true), "lineRepeated": high.line_repeat > 1.0, "truncatedChars": truncated, "includedPreviousSummary": included_summary, "includeThinking": options.serialize.include_thinking, "files": if files.is_empty() { Value::Null } else { Value::String(files) } })).map_err(|error| Error::from_reason(error.to_string()))?
    };
    let kept = if !layout.kept.is_empty() && !layout.tail.is_empty() {
        Utf16Text::join(&[layout.kept.slice(0, layout.kept.len() - layout.tail.len()), tail.clone()], "")
    } else {
        layout.kept
    };
    let frame_count = frames.len();
    let archive = ArchiveUtf16 {
        frames,
        total_chars,
        truncated_chars: truncated,
        text: (!kept.is_empty()).then_some(kept),
        text_head: (!layout.head.is_empty()).then_some(layout.head),
        text_tail: (!tail.is_empty()).then_some(tail),
        without_thinking: None,
    };
    let preserve = crate::archive::replace_archive_raw(preparation.previous_preserve_data, &archive)?;
    let text_note = if text_chars > 0 {
        format!(" (+{} chars as text)", crate::archive::comma_number(text_chars as f64))
    } else {
        String::new()
    };
    Ok(CompactionResultUtf16 {
        summary,
        short_summary: Some(format!(
            "Archived {} chars of history onto {frame_count} snapcompact frame{}{text_note}",
            crate::archive::comma_number(total_chars),
            if frame_count == 1 { "" } else { "s" }
        )),
        first_kept_entry_id: preparation.first_kept_entry_id.to_owned(),
        tokens_before: preparation.tokens_before,
        details: Some(details),
        preserve_data: Some(preserve),
    })
}
