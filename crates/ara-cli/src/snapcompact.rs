//! Fixed OMP 596f2da local archive preparation and live-budget admission.
//! Source: session-maintenance.ts and packages/snapcompact (MIT).
//! This Host seam owns route capabilities and raw cuts; the neutral renderer
//! does not own credentials, Session identity or publication.
use anyhow::{Context as _, Result, bail};
use ara_agent::{
    compaction,
    tokenizer::{MessageCountOptions, Tokenizer},
};
use ara_ai::{Context, Message, Model};
use ara_session::{NativeSnapcompactSnapshot, NativeSnapcompactSummary};
use ara_snapcompact::{
    self as snap, CompactionOptions, CompactionPreparation, CompactionResult, FileOperations, NormalizeOptions,
    ShapeTarget,
};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct ManualCompactArgs {
    pub methods: Option<Vec<String>>,
    pub focus: Option<String>,
}

/// Fixed compact-modes.ts: unknown leading words remain focus text; explicit
/// local image archives reject instructions before any operation is aborted.
pub fn parse_manual_args(arguments: &str) -> Result<ManualCompactArgs> {
    let arguments = arguments.trim();
    if arguments.is_empty() {
        return Ok(ManualCompactArgs::default());
    }
    let (first, rest) = arguments.split_once(char::is_whitespace).unwrap_or((arguments, ""));
    let focus = rest.trim();
    let methods = match first.to_lowercase().as_str() {
        "soft" => vec!["soft".to_owned()],
        "remote" => vec!["remote".to_owned(), "soft".to_owned()],
        "snapcompact" => {
            if !focus.is_empty() {
                bail!(
                    "/compact snapcompact does not take focus instructions (it archives history without an LLM summary)"
                )
            }
            vec!["snapcompact".to_owned()]
        }
        _ => return Ok(ManualCompactArgs { methods: None, focus: Some(arguments.to_owned()) }),
    };
    Ok(ManualCompactArgs { methods: Some(methods), focus: (!focus.is_empty()).then(|| focus.to_owned()) })
}

#[derive(Clone, Debug)]
pub struct SnapcompactPolicy {
    pub shape: String,
    pub reserve_tokens: Option<f64>,
    pub non_message_tokens: usize,
    pub pending_tokens: usize,
}

pub struct PreparedSnapcompact {
    pub result: CompactionResult,
    pub summary: NativeSnapcompactSummary,
    pub window_source_entry_ids: Vec<String>,
    pub tokens_after: usize,
}

pub fn supports_images(metadata: Option<&Value>) -> bool {
    metadata
        .and_then(|value| value.get("input"))
        .and_then(Value::as_array)
        .is_some_and(|input| input.iter().any(|kind| kind == "image"))
}

/// Fixed resolveBudgetReserveTokens, including unset/default provenance.
pub fn prompt_budget(window: Option<f64>, reserve: Option<f64>) -> f64 {
    let Some(window) = window.filter(|value| value.is_finite() && *value > 0.0) else { return f64::INFINITY };
    let proportional = (window * 0.15).floor().max(1.0);
    let effective = (window * 0.15).floor().max(reserve.unwrap_or(16_384.0));
    let reserve = if reserve.is_none() && effective >= window - proportional || effective >= window {
        proportional
    } else {
        effective
    };
    (window - reserve).max(0.0)
}

fn hard_frame_cap(model: &Model) -> usize {
    snap::MAX_FRAMES_DEFAULT
        .min(snap::max_frames_for_data_budget(Some(snap::FRAME_DATA_BYTES_BUDGET)))
        .min(snap::provider_frame_budget(Some(&model.provider)))
}

fn edge_tokens(capacity: usize) -> f64 {
    (2.0 * capacity as f64 * 1.15 / 4.0).ceil() + 2000.0
}

fn archive_prompt_budget(window: Option<f64>, reserve: Option<f64>) -> f64 {
    let Some(window) = window.filter(|value| value.is_finite() && *value > 0.0) else { return f64::INFINITY };
    window - (window * 0.15).floor().max(reserve.unwrap_or(16_384.0))
}

/// Ordinary admission deliberately permits a one-frame cap to try a cheaper
/// text-only layout. Actual byte/window/reduction checks still follow render.
pub fn regular_max_frames(
    model: &Model,
    policy: &SnapcompactPolicy,
    kept_tokens: usize,
    edge_capacity: usize,
) -> usize {
    if model.context_window.is_none_or(|window| !window.is_finite() || window <= 0.0) {
        return hard_frame_cap(model);
    }
    let budget = archive_prompt_budget(model.context_window, policy.reserve_tokens);
    let base = policy.non_message_tokens.saturating_add(policy.pending_tokens).saturating_add(kept_tokens) as f64;
    if base >= budget {
        return 0;
    }
    let frame_budget = budget - base - edge_tokens(edge_capacity);
    if frame_budget < snap::FRAME_TOKEN_ESTIMATE as f64 {
        return 1;
    }
    ((frame_budget / snap::FRAME_TOKEN_ESTIMATE as f64).floor() as usize).min(hard_frame_cap(model))
}

/// Rescue has no min-one exception: if the kept tail exhausts headroom, avoid
/// appending a new archive barrier in front of the real oversized tail.
pub fn rescue_max_frames(
    model: &Model,
    policy: &SnapcompactPolicy,
    threshold: f64,
    kept_tokens: usize,
    edge_capacity: usize,
) -> usize {
    if model.context_window.is_none_or(|window| !window.is_finite() || window <= 0.0) {
        return hard_frame_cap(model);
    }
    let frame_budget = (threshold * 0.8).floor()
        - policy.non_message_tokens as f64
        - policy.pending_tokens as f64
        - kept_tokens as f64
        - edge_tokens(edge_capacity);
    if frame_budget < snap::FRAME_TOKEN_ESTIMATE as f64 {
        return 0;
    }
    ((frame_budget / snap::FRAME_TOKEN_ESTIMATE as f64).floor() as usize).min(hard_frame_cap(model))
}

fn target(model: &Model) -> ShapeTarget {
    ShapeTarget { api: Some(model.api.clone()), id: Some(model.id.clone()) }
}

fn summary(result: &CompactionResult) -> NativeSnapcompactSummary {
    NativeSnapcompactSummary {
        summary: result.summary.clone(),
        short_summary: result.short_summary.clone(),
        preserve_data: result.preserve_data.clone(),
        read_files: result.details.as_ref().map_or_else(Vec::new, |details| details.read_files.clone()),
        modified_files: result.details.as_ref().map_or_else(Vec::new, |details| details.modified_files.clone()),
    }
}

fn result_tokens(
    tokenizer: &Tokenizer,
    result: &CompactionResult,
    kept: &[Message],
    non_message: usize,
    pending: usize,
    options: MessageCountOptions,
) -> usize {
    let message = ara_session::snapcompact::model_message_from_summary(&result.summary, result.preserve_data.as_ref());
    non_message
        .saturating_add(pending)
        .saturating_add(tokenizer.count_messages(kept, options))
        .saturating_add(tokenizer.count_message(&message, options))
}

fn files(snapshot: &NativeSnapcompactSnapshot, prefix: usize) -> FileOperations {
    let previous = snapshot.projection.previous_summary.as_ref().and_then(|summary| summary.file_details.as_ref()).map(
        |details| ara_agent::handoff::HandoffFileDetails {
            read_files: details.read_files.clone(),
            modified_files: details.modified_files.clone(),
        },
    );
    let context = Context {
        messages: snapshot.projection.entries[..prefix].iter().flat_map(|entry| entry.messages.clone()).collect(),
        ..Context::default()
    };
    let details = ara_agent::handoff::prepare_handoff_summary("", &context, previous.as_ref());
    FileOperations {
        read: details.read_files.into_iter().collect(),
        edited: details.modified_files.into_iter().collect(),
        ..Default::default()
    }
}

fn messages(snapshot: &NativeSnapcompactSnapshot, start: usize, end: usize) -> Result<Vec<Value>> {
    snapshot.projection.entries[start..end]
        .iter()
        .flat_map(|entry| &entry.messages)
        .map(|message| serde_json::to_value(message).context("encoding snapcompact source message"))
        .collect()
}

/// Synchronous preparation is run in the Host's joined blocking worker. No
/// Session changes occur until the caller rechecks cancellation and snapshot.
pub fn prepare_snapcompact(
    snapshot: &NativeSnapcompactSnapshot,
    model: &Model,
    keep_tokens: usize,
    tokens_before: u64,
    policy: &SnapcompactPolicy,
) -> Result<Option<PreparedSnapcompact>> {
    let tokenizer = crate::context_budget::tokenizer(model);
    let projection = &snapshot.projection;
    let sources = crate::native_compaction::sources_for_model(&projection.entries, model);
    let previous = projection.previous_summary.as_ref();
    // Native structural cuts support archived images; the soft-summary codec
    // would reject them before snapcompact's own serializer can handle them.
    let Some(cut) = compaction::select_native_remote_compaction_cut(
        &sources,
        keep_tokens,
        previous.map(|entry| entry.summary.as_str()),
    )
    .map_err(|error| anyhow::anyhow!("{error}"))?
    else {
        return Ok(None);
    };
    if cut.first_kept_index == 0 {
        return Ok(None);
    }
    let preparation = CompactionPreparation {
        first_kept_entry_id: cut.first_kept_entry_id.clone(),
        messages_to_summarize: messages(snapshot, 0, cut.history_end_index)?,
        turn_prefix_messages: messages(
            snapshot,
            cut.turn_start_index.unwrap_or(cut.first_kept_index),
            cut.first_kept_index,
        )?,
        tokens_before: tokens_before as f64,
        previous_summary: previous.map(|summary| summary.summary.clone()),
        previous_preserve_data: previous.and_then(|summary| summary.preserve_data.clone()),
        file_ops: files(snapshot, cut.first_kept_index),
    };
    let model_target = target(model);
    let include_thinking = crate::model_identity::preferred_dialect(&model.id) != "anthropic";
    let serialize = snap::SerializeOptions { include_thinking, ..Default::default() };
    let mut source_messages = preparation.messages_to_summarize.clone();
    source_messages.extend(preparation.turn_prefix_messages.clone());
    let text = snap::serialize_conversation(&source_messages, &serialize);
    let probe = snap::renderability_probe_text(
        &text,
        preparation.previous_preserve_data.as_ref(),
        preparation.previous_summary.as_deref(),
    );
    let shape = snap::resolve_shape_for_text(&probe, Some(&model_target), Some(&policy.shape))?;
    let scan = snap::scan_renderability(&probe, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() });
    if !scan.is_safe {
        bail!("snapcompact font cannot represent {:.1}% of the archive", scan.unrenderable_ratio * 100.0)
    }
    let kept: Vec<_> =
        projection.entries[cut.first_kept_index..].iter().flat_map(|entry| entry.messages.clone()).collect();
    let default_shape = snap::resolve_shape(Some(&model_target), Some(&policy.shape))?;
    let max_frames = regular_max_frames(
        model,
        policy,
        tokenizer.count_messages(&kept, Default::default()),
        snap::geometry(&default_shape, None).capacity,
    );
    if max_frames == 0 {
        bail!("snapcompact kept history already exhausts the prompt budget")
    }
    let result = snap::compact(
        &preparation,
        &CompactionOptions {
            model: Some(model_target),
            shape: (policy.shape != "auto").then_some(shape),
            max_frames: Some(max_frames as f64),
            serialize,
            ..Default::default()
        },
    )?;
    if snap::get_preserved_archive(result.preserve_data.as_ref())
        .is_some_and(|archive| snap::frame_data_bytes(&archive.frames) > snap::FRAME_DATA_BYTES_BUDGET)
    {
        bail!("snapcompact exceeds the standing image payload budget")
    }
    let tokens_after =
        result_tokens(&tokenizer, &result, &kept, policy.non_message_tokens, policy.pending_tokens, Default::default());
    let reduction_tokens = result_tokens(
        &tokenizer,
        &result,
        &kept,
        policy.non_message_tokens,
        0,
        MessageCountOptions { exclude_encrypted_reasoning: true },
    );
    let baseline_messages: Vec<_> = projection.entries.iter().flat_map(|entry| entry.messages.clone()).collect();
    let baseline = policy.non_message_tokens.saturating_add(
        tokenizer.count_messages(&baseline_messages, MessageCountOptions { exclude_encrypted_reasoning: true }),
    );
    if reduction_tokens >= baseline {
        bail!("snapcompact would not reduce context")
    }
    if tokens_after as f64 > archive_prompt_budget(model.context_window, policy.reserve_tokens) {
        bail!("snapcompact projected context still exceeds the prompt budget")
    }
    Ok(Some(PreparedSnapcompact {
        summary: summary(&result),
        result,
        window_source_entry_ids: projection.entries[..cut.first_kept_index]
            .iter()
            .filter(|entry| !entry.messages.is_empty())
            .map(|entry| entry.entry_id.clone())
            .collect(),
        tokens_after,
    }))
}

/// Re-render only the currently retained archive. Every projected raw message
/// is kept: this includes the region before the archive's append point and the
/// region after it. Source identities and the original kept cut do not change.
pub fn prepare_frame_rescue(
    snapshot: &NativeSnapcompactSnapshot,
    model: &Model,
    policy: &SnapcompactPolicy,
    threshold: f64,
) -> Result<Option<PreparedSnapcompact>> {
    let tokenizer = crate::context_budget::tokenizer(model);
    let Some(previous) = snapshot.projection.previous_summary.as_ref() else { return Ok(None) };
    let Some(archive) = previous.archive.as_ref() else { return Ok(None) };
    if archive.frames.len() <= 1 {
        return Ok(None);
    }
    let Some(text) = snap::archive_source_text(archive).filter(|text| !text.is_empty()) else { return Ok(None) };
    let kept: Vec<_> = snapshot.projection.entries.iter().flat_map(|entry| entry.messages.clone()).collect();
    let model_target = target(model);
    let default_shape = snap::resolve_shape(Some(&model_target), Some(&policy.shape))?;
    let max_frames = rescue_max_frames(
        model,
        policy,
        threshold,
        tokenizer.count_messages(&kept, Default::default()),
        snap::geometry(&default_shape, None).capacity,
    );
    if max_frames == 0 || max_frames >= archive.frames.len() {
        return Ok(None);
    }
    let shape = snap::resolve_shape_for_text(&text, Some(&model_target), Some(&policy.shape))?;
    let file_ops = previous.file_details.as_ref().map_or_else(FileOperations::default, |details| FileOperations {
        read: details.read_files.iter().cloned().collect(),
        edited: details.modified_files.iter().cloned().collect(),
        ..Default::default()
    });
    let result = snap::compact(
        &CompactionPreparation {
            first_kept_entry_id: previous.first_kept_entry_id.clone(),
            messages_to_summarize: Vec::new(),
            turn_prefix_messages: Vec::new(),
            tokens_before: previous.tokens_before as f64,
            previous_summary: Some(previous.summary.clone()),
            previous_preserve_data: previous.preserve_data.clone(),
            file_ops,
        },
        &CompactionOptions {
            model: Some(model_target),
            shape: (policy.shape != "auto").then_some(shape),
            max_frames: Some(max_frames as f64),
            ..Default::default()
        },
    )?;
    let Some(rebuilt) = snap::get_preserved_archive(result.preserve_data.as_ref()) else { return Ok(None) };
    if rebuilt.frames.len() >= archive.frames.len() {
        return Ok(None);
    }
    if snap::frame_data_bytes(&rebuilt.frames) > snap::FRAME_DATA_BYTES_BUDGET {
        return Ok(None);
    }
    let tokens_after =
        result_tokens(&tokenizer, &result, &kept, policy.non_message_tokens, policy.pending_tokens, Default::default());
    Ok(Some(PreparedSnapcompact {
        summary: summary(&result),
        result,
        window_source_entry_ids: Vec::new(),
        tokens_after,
    }))
}
