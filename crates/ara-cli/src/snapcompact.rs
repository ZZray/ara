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
use ara_rpc::WireValue;
use ara_session::{
    NativeMessageSource, NativeSnapcompactArchiveSource, NativeSnapcompactSnapshot, NativeSnapcompactSummary,
};
use ara_snapcompact::{
    self as snap, CompactionOptions, CompactionPreparationUtf16, CompactionResultUtf16, DeploymentTarget,
    FileOperations, NormalizeOptions, Shape, ShapeTarget,
};
use serde_json::{Value, value::RawValue};

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
    pub deployment: Option<DeploymentTarget>,
}

fn metadata_string(metadata: Option<&Value>, key: &str) -> Result<Option<String>> {
    match metadata.and_then(|metadata| metadata.get(key)) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => bail!("snapcompact model {key} must be a string"),
    }
}
fn normalize_anthropic_base_url(value: &str) -> Option<String> {
    let trimmed = ara_prompt::js::trim(value);
    if trimmed.is_empty() {
        return None;
    }
    let trimmed = trimmed.trim_end_matches('/');
    Some(trimmed.strip_suffix("/v1").unwrap_or(trimmed).into())
}
/// OMP anthropic-state.ts:19-41; only the Host observes process route overrides.
pub fn reaches_official_endpoint(model: &Model, metadata: Option<&Value>) -> bool {
    if metadata
        .and_then(|metadata| metadata.get("compat"))
        .and_then(|compat| compat.get("officialEndpoint"))
        .and_then(Value::as_bool)
        != Some(true)
    {
        return false;
    }
    if model.provider != "anthropic" || model.api != "anthropic-messages" {
        return true;
    }
    let foundry = std::env::var("CLAUDE_CODE_USE_FOUNDRY").ok().is_some_and(|value| {
        matches!(ara_prompt::js::trim(&value).to_lowercase().as_str(), "1" | "true" | "yes" | "on")
    });
    let foundry_url = foundry
        .then(|| std::env::var("FOUNDRY_BASE_URL").ok())
        .flatten()
        .and_then(|value| normalize_anthropic_base_url(&value));
    let configured = normalize_anthropic_base_url(&model.base_url);
    let effective = foundry_url
        .or_else(|| {
            if let Some(configured) = configured.as_ref()
                && !crate::model_policy::is_official_anthropic_api_url(Some(configured))
            {
                return Some(configured.clone());
            }
            std::env::var("ANTHROPIC_BASE_URL")
                .ok()
                .and_then(|value| normalize_anthropic_base_url(&value))
                .or(configured)
        })
        .unwrap_or_else(|| "https://api.anthropic.com".into());
    crate::model_policy::is_official_anthropic_api_url(Some(&effective))
}

/// Observe the selected model's built metadata without extending the shared wire Model.
pub fn deployment_target(model: &Model, metadata: Option<&Value>) -> Result<DeploymentTarget> {
    let catalog_id = metadata_string(metadata, "catalogModelId")?.unwrap_or_else(|| model.id.clone());
    let identity = match metadata.and_then(|metadata| metadata.get("identity")) {
        None | Some(Value::Null) => None,
        Some(Value::Object(identity)) => Some(snap::ModelIdentity {
            class: identity
                .get("class")
                .and_then(Value::as_str)
                .context("snapcompact model identity class is missing")?
                .into(),
            family: metadata_string(Some(&Value::Object(identity.clone())), "family")?,
            revision: metadata_string(Some(&Value::Object(identity.clone())), "revision")?,
        }),
        Some(_) => bail!("snapcompact model identity must be an object"),
    };
    let request_model_id =
        metadata_string(metadata, "requestModelId")?.or_else(|| (catalog_id != model.id).then(|| model.id.clone()));
    let unverified = metadata
        .and_then(|metadata| metadata.get("thinking"))
        .and_then(|thinking| thinking.get("unverified"))
        .and_then(Value::as_bool)
        == Some(true);
    let reasoning = !unverified
        && metadata.and_then(|metadata| metadata.get("reasoning")).and_then(Value::as_bool).unwrap_or(model.reasoning);
    Ok(DeploymentTarget {
        model: ShapeTarget {
            api: Some(model.api.clone()),
            id: Some(catalog_id),
            provider: Some(model.provider.clone()),
            identity,
        },
        provider_type: metadata_string(metadata, "providerType")?,
        request_model_id,
        reasoning,
        official_endpoint: reaches_official_endpoint(model, metadata),
    })
}

fn deployment(model: &Model, policy: &SnapcompactPolicy) -> DeploymentTarget {
    policy.deployment.clone().unwrap_or_else(|| DeploymentTarget {
        model: ShapeTarget {
            api: Some(model.api.clone()),
            id: Some(model.id.clone()),
            provider: Some(model.provider.clone()),
            identity: None,
        },
        reasoning: model.reasoning,
        ..Default::default()
    })
}

pub struct PreparedSnapcompact {
    pub result: CompactionResultUtf16,
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

fn hard_frame_cap(model: &Model, policy: &SnapcompactPolicy, shape: &Shape) -> Result<usize> {
    let deployment = deployment(model, policy);
    Ok(snap::MAX_FRAMES_DEFAULT
        .min(snap::max_frames_for_data_budget(shape, snap::frame_data_bytes_budget(Some(&deployment))?))
        .min(snap::provider_frame_budget(Some(&deployment))?))
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
    shape: &Shape,
) -> Result<usize> {
    if model.context_window.is_none_or(|window| !window.is_finite() || window <= 0.0) {
        return hard_frame_cap(model, policy, shape);
    }
    let budget = archive_prompt_budget(model.context_window, policy.reserve_tokens);
    let base = policy.non_message_tokens.saturating_add(policy.pending_tokens).saturating_add(kept_tokens) as f64;
    if base >= budget {
        return Ok(0);
    }
    let frame_budget = budget - base - edge_tokens(snap::geometry(shape, None).capacity);
    let frame_cost = snap::planned_frame_tokens(shape);
    if frame_budget < frame_cost {
        return Ok(1);
    }
    Ok(((frame_budget / frame_cost).floor() as usize).min(hard_frame_cap(model, policy, shape)?))
}

/// Rescue has no min-one exception: if the kept tail exhausts headroom, avoid
/// appending a new archive barrier in front of the real oversized tail.
pub fn rescue_max_frames(
    model: &Model,
    policy: &SnapcompactPolicy,
    threshold: f64,
    kept_tokens: usize,
    shape: &Shape,
) -> Result<usize> {
    if model.context_window.is_none_or(|window| !window.is_finite() || window <= 0.0) {
        return hard_frame_cap(model, policy, shape);
    }
    let frame_budget = (threshold * 0.8).floor()
        - policy.non_message_tokens as f64
        - policy.pending_tokens as f64
        - kept_tokens as f64
        - edge_tokens(snap::geometry(shape, None).capacity);
    let frame_cost = snap::planned_frame_tokens(shape);
    if frame_budget < frame_cost {
        return Ok(0);
    }
    Ok(((frame_budget / frame_cost).floor() as usize).min(hard_frame_cap(model, policy, shape)?))
}

fn target(model: &Model, policy: &SnapcompactPolicy) -> ShapeTarget {
    deployment(model, policy).model
}

fn summary(result: &CompactionResultUtf16) -> Result<NativeSnapcompactSummary> {
    Ok(NativeSnapcompactSummary {
        summary: result.summary.clone(),
        short_summary: result.short_summary.clone(),
        // A scalar mirror exists only when the canonical source is representable.
        preserve_data: result.preserve_data.as_deref().and_then(|raw| serde_json::from_str(raw.get()).ok()),
        preserve_source: result.preserve_data.as_deref().map(NativeSnapcompactArchiveSource::from_raw).transpose()?,
        read_files: result.details.as_ref().map_or_else(Vec::new, |details| details.read_files.clone()),
        modified_files: result.details.as_ref().map_or_else(Vec::new, |details| details.modified_files.clone()),
    })
}

/// Count the selected source without rebuilding its text as a scalar transcript.
/// Native token encodings consume the original UTF16 fragments; only the byte
/// estimator measures replacement UTF8 byte lengths for lone units.
pub fn model_source_tokens(tokenizer: &Tokenizer, source: &NativeMessageSource) -> Result<usize> {
    model_source_tokens_with_billing(tokenizer, source, &snap::FrameBilling::default())
}

/// OMP agent/tokenizer.ts:550-552: measurable pixels use the active reader's
/// price; undecodable/missing dimensions use the conservative ceiling.
pub fn model_source_tokens_with_billing(
    tokenizer: &Tokenizer,
    source: &NativeMessageSource,
    billing: &snap::FrameBilling,
) -> Result<usize> {
    let body = source.observe()?;
    let content = body.get("content").ok_or_else(|| anyhow::anyhow!("archive model source has no content"))?;
    let mut fragments = Vec::new();
    let mut extra = 0usize;
    if let Some(text) = content.as_string() {
        fragments.push(text.units().to_vec());
    } else {
        let blocks = content.as_array().ok_or_else(|| anyhow::anyhow!("invalid archive model content"))?;
        for block in blocks {
            let kind = block.get("type").and_then(WireValue::as_string);
            if kind.is_some_and(|kind| kind.equals_ascii("text")) {
                let text = block
                    .get("text")
                    .and_then(WireValue::as_string)
                    .ok_or_else(|| anyhow::anyhow!("invalid archive model text"))?;
                if !text.units().is_empty() {
                    fragments.push(text.units().to_vec());
                }
            } else if kind.is_some_and(|kind| kind.equals_ascii("image")) {
                if matches!(block.get("compactionFrame"), Some(WireValue::Bool(true))) {
                    let size = block.get("data").and_then(WireValue::as_string).and_then(frame_image_size);
                    let tokens =
                        size.map_or(snap::FRAME_TOKEN_ESTIMATE as f64, |size| snap::frame_tokens(billing, size));
                    extra = extra.saturating_add(tokens as usize);
                }
            } else {
                bail!("unsupported archive model content block")
            }
        }
    }
    Ok(tokenizer
        .count_utf16_fragments(fragments.iter().map(Vec::as_slice), ara_agent::tokenizer::TokenCountMode::Approximate)
        .saturating_add(extra))
}

fn frame_image_size(data: &ara_rpc::WireString) -> Option<snap::ImageSize> {
    // Same 64KiB decoded-header bound as agent/image-tokens.ts. The forgiving
    // base64 observation mirrors Buffer.from, never an ingress validator.
    const HEADER_BASE64_UNITS: usize = 4 * (64 * 1024_usize).div_ceil(3);
    let (mut bytes, mut accumulator, mut bits) = (Vec::new(), 0_u32, 0_u32);
    for &unit in data.units().iter().take(HEADER_BASE64_UNITS) {
        let value = match unit {
            65..=90 => unit - 65,
            97..=122 => unit - 97 + 26,
            48..=57 => unit - 48 + 52,
            43 | 45 => 62,
            47 | 95 => 63,
            61 => break,
            _ => continue,
        };
        accumulator = (accumulator << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            bytes.push((accumulator >> bits) as u8);
        }
    }
    let metadata = ara_tools::read_image::parse_image_metadata(&bytes)?;
    let (width, height) = (metadata.width?, metadata.height?);
    (width > 0 && height > 0).then_some(snap::ImageSize { width: f64::from(width), height: f64::from(height) })
}

fn result_tokens(
    tokenizer: &Tokenizer,
    result: &CompactionResultUtf16,
    kept: &[Message],
    non_message: usize,
    pending: usize,
    options: MessageCountOptions,
    billing: &snap::FrameBilling,
    history_options: snap::HistoryBlockOptions,
    include_thinking: bool,
) -> Result<usize> {
    let source = ara_session::snapcompact::model_source_from_summary_with_blob_store(
        &result.summary,
        result.preserve_data.as_deref(),
        None,
        Some(history_options),
        include_thinking,
        result.details.as_ref(),
    )?;
    Ok(non_message
        .saturating_add(pending)
        .saturating_add(tokenizer.count_messages(kept, options))
        .saturating_add(model_source_tokens_with_billing(tokenizer, &source, billing)?))
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

fn messages(snapshot: &NativeSnapcompactSnapshot, start: usize, end: usize) -> Result<Vec<Box<RawValue>>> {
    let mut messages = Vec::new();
    for projected in &snapshot.projection.entries[start..end] {
        // Preserve known native model bodies at the existing snapshot boundary.
        // Extension-role conversion still uses its explicitly checked fragments.
        let raw = snapshot
            .raw
            .entries
            .iter()
            .find(|entry| entry.id == projected.entry_id)
            .and_then(|entry| entry.native_message.as_ref());
        if projected.messages.len() == 1
            && let Some(raw) = raw
        {
            let body = raw.observe()?;
            if body.get("prunedAt").is_none()
                && body
                    .get("role")
                    .and_then(WireValue::as_string)
                    .is_some_and(|role| ["user", "assistant", "toolResult"].iter().any(|kind| role.equals_ascii(kind)))
            {
                messages.push(RawValue::from_string(body.stringify())?);
                continue;
            }
        }
        for message in &projected.messages {
            messages
                .push(serde_json::value::to_raw_value(message).context("encoding scalar snapcompact source fragment")?);
        }
    }
    Ok(messages)
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
    let preparation = CompactionPreparationUtf16 {
        first_kept_entry_id: cut.first_kept_entry_id.clone(),
        messages_to_summarize: messages(snapshot, 0, cut.history_end_index)?,
        turn_prefix_messages: messages(
            snapshot,
            cut.turn_start_index.unwrap_or(cut.first_kept_index),
            cut.first_kept_index,
        )?,
        tokens_before: tokens_before as f64,
        previous_summary: previous.map(|summary| snap::Utf16Text::from(summary.summary.as_str())),
        previous_preserve_data: previous.map(|summary| summary.exact_preserve_json()).transpose()?.flatten(),
        file_ops: files(snapshot, cut.first_kept_index),
    };
    let model_target = target(model, policy);
    let deployment = deployment(model, policy);
    let include_thinking = snap::reader_includes_thinking(&deployment)?;
    let byte_budget = snap::frame_data_bytes_budget(Some(&deployment))?;
    let history_options = snap::HistoryBlockOptions { max_frame_data_bytes: Some(byte_budget), ..Default::default() };
    let billing = snap::frame_billing(Some(&model_target))?;
    let serialize = snap::SerializeOptions { include_thinking, ..Default::default() };
    let mut source_messages = preparation.messages_to_summarize.clone();
    source_messages.extend(preparation.turn_prefix_messages.clone());
    let text = snap::serialize_conversation_raw_utf16(&source_messages, &serialize)?;
    let prior_archive = snap::get_preserved_archive_utf16(preparation.previous_preserve_data.as_deref())?;
    let probe =
        snap::renderability_probe_text_utf16(&text, prior_archive.as_ref(), preparation.previous_summary.as_ref());
    let shape = snap::resolve_shape_for_text_utf16(&probe, Some(&model_target), Some(&policy.shape))?;
    let scan =
        snap::scan_renderability_utf16(&probe, NormalizeOptions { shape: Some(shape.clone()), ..Default::default() });
    if !scan.is_safe {
        bail!("snapcompact font cannot represent {:.1}% of the archive", scan.unrenderable_ratio * 100.0)
    }
    let kept: Vec<_> =
        projection.entries[cut.first_kept_index..].iter().flat_map(|entry| entry.messages.clone()).collect();
    let default_shape = snap::resolve_shape(Some(&model_target), Some(&policy.shape))?;
    let max_frames =
        regular_max_frames(model, policy, tokenizer.count_messages(&kept, Default::default()), &default_shape)?;
    if max_frames == 0 {
        bail!("snapcompact kept history already exhausts the prompt budget")
    }
    let result = snap::compact_utf16(
        &preparation,
        &CompactionOptions {
            model: Some(model_target),
            shape: (policy.shape != "auto").then_some(shape),
            max_frames: Some(max_frames as f64),
            serialize,
            ..Default::default()
        },
    )?;
    if snap::get_preserved_archive_utf16(result.preserve_data.as_deref())?
        .is_some_and(|archive| snap::frame_data_bytes_utf16(&archive.frames) > byte_budget)
    {
        bail!("snapcompact exceeds the standing image payload budget")
    }
    let tokens_after = result_tokens(
        &tokenizer,
        &result,
        &kept,
        policy.non_message_tokens,
        policy.pending_tokens,
        Default::default(),
        &billing,
        history_options,
        include_thinking,
    )?;
    let reduction_tokens = result_tokens(
        &tokenizer,
        &result,
        &kept,
        policy.non_message_tokens,
        0,
        MessageCountOptions { exclude_encrypted_reasoning: true },
        &billing,
        history_options,
        include_thinking,
    )?;
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
        summary: summary(&result)?,
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
    let Some(archive) = previous.exact_archive()? else { return Ok(None) };
    if archive.frames.len() <= 1 {
        return Ok(None);
    }
    let Some(text) = snap::archive_source_text_utf16(&archive).filter(|text| !text.is_empty()) else { return Ok(None) };
    let kept: Vec<_> = snapshot.projection.entries.iter().flat_map(|entry| entry.messages.clone()).collect();
    let model_target = target(model, policy);
    let deployment = deployment(model, policy);
    let include_thinking = snap::reader_includes_thinking(&deployment)?;
    let byte_budget = snap::frame_data_bytes_budget(Some(&deployment))?;
    let history_options = snap::HistoryBlockOptions { max_frame_data_bytes: Some(byte_budget), ..Default::default() };
    let billing = snap::frame_billing(Some(&model_target))?;
    let default_shape = snap::resolve_shape(Some(&model_target), Some(&policy.shape))?;
    let max_frames = rescue_max_frames(
        model,
        policy,
        threshold,
        tokenizer.count_messages(&kept, Default::default()),
        &default_shape,
    )?;
    if max_frames == 0 || max_frames >= archive.frames.len() {
        return Ok(None);
    }
    let shape = snap::resolve_shape_for_text_utf16(&text, Some(&model_target), Some(&policy.shape))?;
    let file_ops = previous.file_details.as_ref().map_or_else(FileOperations::default, |details| FileOperations {
        read: details.read_files.iter().cloned().collect(),
        edited: details.modified_files.iter().cloned().collect(),
        ..Default::default()
    });
    let result = snap::compact_utf16(
        &CompactionPreparationUtf16 {
            first_kept_entry_id: previous.first_kept_entry_id.clone(),
            messages_to_summarize: Vec::new(),
            turn_prefix_messages: Vec::new(),
            tokens_before: previous.tokens_before as f64,
            previous_summary: Some(snap::Utf16Text::from(previous.summary.as_str())),
            previous_preserve_data: previous.exact_preserve_json()?,
            file_ops,
        },
        &CompactionOptions {
            model: Some(model_target),
            shape: (policy.shape != "auto").then_some(shape),
            max_frames: Some(max_frames as f64),
            serialize: snap::SerializeOptions { include_thinking, ..Default::default() },
            ..Default::default()
        },
    )?;
    let Some(rebuilt) = snap::get_preserved_archive_utf16(result.preserve_data.as_deref())? else { return Ok(None) };
    if rebuilt.frames.len() >= archive.frames.len() {
        return Ok(None);
    }
    if snap::frame_data_bytes_utf16(&rebuilt.frames) > byte_budget {
        return Ok(None);
    }
    let tokens_after = result_tokens(
        &tokenizer,
        &result,
        &kept,
        policy.non_message_tokens,
        policy.pending_tokens,
        Default::default(),
        &billing,
        history_options,
        include_thinking,
    )?;
    Ok(Some(PreparedSnapcompact {
        summary: summary(&result)?,
        result,
        window_source_entry_ids: Vec::new(),
        tokens_after,
    }))
}
