//! Preparation adapter for fixed OMP 596f2da local context maintenance (MIT).
//! Source: session-maintenance.ts:463-729, plan-mode/plan-protection.ts, and
//! agent/tokenizer.ts. This module does not commit history or bind runtimes.
//! Original raw JSON stays in the independent Session snapshot; only the
//! explicit selected slots are projected into Core DTOs.

use std::borrow::Cow;
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Write;

use anyhow::{Context, Result, bail};
use ara_agent::compaction::local_reduction::{self as core, *};
use ara_agent::tokenizer::{EstimateMode, IMAGE_TOKEN_ESTIMATE, count_fragments};
use ara_ai::Model;
use ara_ai::model_tokenizer::{ModelContentCount, ModelTokenizer, count_model_fragments};
use ara_session::{SessionReductionAction, SessionReductionEdit, SessionReductionSlot, SessionReductionSnapshot};
use serde_json::Value;

use crate::session_artifacts::SessionArtifacts;

/// Fixed packages/snapcompact/src/snapcompact.ts:475. Counting this payload
/// does not implement frame generation, rendering, archive recovery or fitting.
pub const FRAME_TOKEN_ESTIMATE: usize = 5_024;
pub const PRUNE_CACHE_WARM_SUFFIX_TOKENS: f64 = 8_000.0;
pub const PRUNE_IDLE_FLUSH_MS: f64 = 90.0 * 60_000.0;

#[derive(Clone, Debug, Default)]
pub struct HostReductionPolicy {
    /// Host-selected latest compaction boundary; None means the whole branch.
    pub keep_boundary_id: Option<String>,
    pub prefix_binding: bool,
    /// Current plan reference paths. Native local://PLAN.md is always protected.
    pub protected_read_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenEstimation {
    /// Exact selected-family content fragments, with estimated images/frames.
    /// This is not an exact raw/wire request total or a hard context-fit proof.
    ModelFamilyContent(ModelTokenizer),
    ApproximateUtf8Fragments,
    NotMeasured,
}

#[derive(Clone, Debug)]
pub struct PreparedReduction {
    pub snapshot: SessionReductionSnapshot,
    pub plan: ReductionPlan,
    pub edits: Vec<SessionReductionEdit>,
    /// Document order, so the Host can select entries before its usage anchor.
    pub regions: Vec<ShakeRegion>,
    /// Per-entry sum of max(0, original-region tokens - replacement tokens),
    /// as native anchored history accounting. Removed text only, not images.
    pub entry_tokens_freed: Vec<(String, usize)>,
    pub artifact_id: Option<String>,
    pub artifact_error: Option<String>,
    /// Native elide result: max(0, all original tokens - all replacements).
    /// Images/thinking return zero, matching their native receipts.
    pub tokens_freed: usize,
    pub token_estimation: TokenEstimation,
}

struct ModelCounter<'a> {
    model: &'a Model,
    overflowed: Cell<bool>,
}

impl<'a> ModelCounter<'a> {
    fn new(model: &'a Model) -> Self {
        Self { model, overflowed: Cell::new(false) }
    }
    fn measure<'b>(&self, fragments: impl IntoIterator<Item = &'b str>) -> Result<usize> {
        let fragments: Vec<&str> = fragments.into_iter().collect();
        match count_model_fragments(self.model, fragments.iter().copied()) {
            ModelContentCount::Exact(count) => usize::try_from(count).context("local reduction content count overflow"),
            ModelContentCount::UnknownTokenizer => Ok(count_fragments(fragments, EstimateMode::Approximate)),
            ModelContentCount::CountOverflow => bail!("local reduction content count overflow"),
        }
    }
    fn checked(&self) -> Result<()> {
        if self.overflowed.get() {
            bail!("local reduction content count overflow");
        }
        Ok(())
    }
    fn estimation(&self) -> TokenEstimation {
        self.model.tokenizer.map_or(TokenEstimation::ApproximateUtf8Fragments, TokenEstimation::ModelFamilyContent)
    }
}

impl ReductionTokenizer for ModelCounter<'_> {
    fn count_fragments(&self, fragments: &[&str]) -> usize {
        match self.measure(fragments.iter().copied()) {
            Ok(count) => count,
            Err(_) => {
                self.overflowed.set(true);
                0
            }
        }
    }
}

fn raw_content(value: Option<&Value>) -> Result<ReductionContent> {
    match value {
        None => Ok(ReductionContent::Absent),
        Some(Value::String(text)) => Ok(ReductionContent::String(text.clone())),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| {
                Ok(match block.get("type").and_then(Value::as_str) {
                    Some("text") => ReductionBlock::Text(
                        block
                            .get("text")
                            .and_then(Value::as_str)
                            .context("original text block has no string text")?
                            .to_owned(),
                    ),
                    Some("image") => ReductionBlock::Image,
                    Some("thinking") => ReductionBlock::Thinking,
                    Some("redactedThinking") => ReductionBlock::RedactedThinking,
                    _ => ReductionBlock::Other,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(ReductionContent::Blocks),
        _ => bail!("original reduction content is neither string nor array"),
    }
}

fn role(value: &str) -> ReductionRole {
    match value {
        "user" => ReductionRole::User,
        "developer" => ReductionRole::Developer,
        "assistant" => ReductionRole::Assistant,
        "toolResult" => ReductionRole::ToolResult,
        "custom" => ReductionRole::Custom,
        "hookMessage" => ReductionRole::HookMessage,
        "fileMention" => ReductionRole::FileMention,
        value => ReductionRole::Other(value.into()),
    }
}

fn text_fragments<'a>(value: &'a Value, include_empty: bool, fragments: &mut Vec<Cow<'a, str>>) {
    if let Some(text) = value.as_str() {
        fragments.push(Cow::Borrowed(text));
    } else if let Some(blocks) = value.as_array() {
        for block in blocks {
            if block["type"] == "text"
                && let Some(text) = block["text"].as_str()
                && (include_empty || !text.is_empty())
            {
                fragments.push(Cow::Borrowed(text));
            }
        }
    }
}

/// Raw native message counting, independently of model-visible projections.
/// ARA's Developer wire role uses the User text recipe; native Custom and
/// FileMention keep the fixed tokenizer default-zero contribution.
fn raw_message_tokens(message: &Value, counter: &ModelCounter<'_>) -> Result<usize> {
    let mut fragments: Vec<Cow<'_, str>> = Vec::new();
    let mut extra: usize = 0;
    match message["role"].as_str() {
        Some("user" | "developer") => text_fragments(&message["content"], false, &mut fragments),
        Some("assistant") => {
            if let Some(blocks) = message["content"].as_array() {
                for block in blocks {
                    let fields: &[&str] = match block["type"].as_str() {
                        Some("text") => &["text"],
                        Some("thinking") => &["thinking", "thinkingSignature"],
                        Some("redactedThinking") => &["data"],
                        Some("toolCall") => &["name"],
                        _ => &[],
                    };
                    for field in fields {
                        if let Some(text) = block[*field].as_str()
                            && (*field != "thinkingSignature" || !text.is_empty())
                        {
                            fragments.push(Cow::Borrowed(text));
                        }
                    }
                    if block["type"] == "toolCall" {
                        fragments.push(Cow::Owned(serde_json::to_string(&block["arguments"])?));
                    } else if block["type"] == "anthropicServerTool" {
                        fragments.push(Cow::Owned(serde_json::to_string(&block["block"])?));
                    }
                }
            }
        }
        Some("hookMessage" | "toolResult") => {
            text_fragments(&message["content"], false, &mut fragments);
            if let Some(blocks) = message["content"].as_array() {
                extra =
                    blocks.iter().filter(|block| block["type"] == "image").count().saturating_mul(IMAGE_TOKEN_ESTIMATE);
            }
        }
        Some("bashExecution") => {
            for field in ["command", "output"] {
                if let Some(text) = message[field].as_str() {
                    fragments.push(Cow::Borrowed(text));
                }
            }
        }
        Some("branchSummary" | "compactionSummary") => {
            if let Some(summary) = message["summary"].as_str() {
                fragments.push(Cow::Borrowed(summary));
            }
            if message["role"] == "compactionSummary" {
                if let Some(blocks) = message["blocks"].as_array() {
                    for block in blocks {
                        if block["type"] == "text" {
                            if let Some(text) = block["text"].as_str() {
                                fragments.push(Cow::Borrowed(text));
                            }
                        } else {
                            extra = extra.saturating_add(FRAME_TOKEN_ESTIMATE);
                        }
                    }
                } else if let Some(images) = message["images"].as_array() {
                    extra = images.len().saturating_mul(FRAME_TOKEN_ESTIMATE);
                }
            }
        }
        _ => {}
    }
    if fragments.is_empty() {
        return Ok(extra);
    }
    Ok(extra.saturating_add(counter.measure(fragments.iter().map(|fragment| fragment.as_ref()))?))
}

fn normalize_plan_path(path: &str) -> Cow<'_, str> {
    if let Some(rest) = path.strip_prefix("local:/")
        && !rest.starts_with('/')
    {
        Cow::Owned(format!("local://{rest}"))
    } else {
        Cow::Borrowed(path)
    }
}

fn targets_plan(path: &str, target: &str) -> bool {
    let path = normalize_plan_path(path);
    let target = normalize_plan_path(target);
    path == target || path.strip_prefix(target.as_ref()).is_some_and(|suffix| suffix.starts_with(':'))
}

fn json_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|value| value != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn map_entries(
    snapshot: &SessionReductionSnapshot,
    counter: Option<&ModelCounter<'_>>,
    policy: &HostReductionPolicy,
) -> Result<Vec<ReductionEntry>> {
    let mut entries = Vec::with_capacity(snapshot.entries.len());
    for entry in &snapshot.entries {
        let raw = &entry.raw;
        if raw["type"].as_str() != Some(entry.kind.as_str()) || raw["id"].as_str() != Some(entry.id.as_str()) {
            bail!("raw reduction entry identity disagrees with its Session receipt: {}", entry.id);
        }
        let mut reduced = ReductionEntry {
            entry_id: entry.id.clone(),
            kind: ReductionEntryKind::Metadata,
            timestamp_ms: None,
            raw_message_tokens: 0,
            shake_entry_tokens: 0,
            slots: Vec::new(),
            tool_result: None,
            tool_calls: Vec::new(),
        };
        if entry.kind == "message" {
            let message = raw
                .get("message")
                .filter(|message| message.is_object())
                .context("raw entry has no original message object")?;
            let role_name = message["role"].as_str().context("raw message has no string role")?;
            reduced.kind = ReductionEntryKind::Message(role(role_name));
            reduced.timestamp_ms = message["timestamp"].as_f64();
            if let Some(counter) = counter {
                reduced.raw_message_tokens = raw_message_tokens(message, counter)?;
            }
            reduced.shake_entry_tokens = reduced.raw_message_tokens;
            if role_name == "fileMention" {
                if let Some(files) = message["files"].as_array() {
                    for (index, file) in files.iter().enumerate() {
                        // Native checks the raw attribute's truthiness, without
                        // normalizing or decoding its opaque image payload.
                        if file.get("image").is_some_and(json_truthy) {
                            reduced.slots.push(ReductionContentSlot {
                                slot: RawContentSlot::FileMentionImage(index),
                                content: ReductionContent::Blocks(vec![ReductionBlock::Image]),
                            });
                        }
                    }
                }
            } else if message.get("content").is_some() {
                reduced.slots.push(ReductionContentSlot {
                    slot: RawContentSlot::MessageContent,
                    content: raw_content(message.get("content"))?,
                });
            }
            if role_name == "assistant"
                && let Some(blocks) = message["content"].as_array()
            {
                for block in blocks.iter().filter(|block| block["type"] == "toolCall") {
                    reduced.tool_calls.push(ReductionToolCall {
                        id: block["id"].as_str().context("raw tool call has no string ID")?.into(),
                        name: block["name"].as_str().context("raw tool call has no string name")?.into(),
                        arguments: block["arguments"]
                            .as_object()
                            .context("raw tool call arguments are not an object")?
                            .clone(),
                    });
                }
            }
            if role_name == "toolResult" {
                reduced.tool_result = Some(ReductionToolResult {
                    call_id: message["toolCallId"].as_str().context("raw tool result has no call ID")?.into(),
                    tool_name: message["toolName"].as_str().context("raw tool result has no tool name")?.into(),
                    already_pruned: message.get("prunedAt").is_some(),
                    useless: message["useless"] == true,
                    is_error: message["isError"] == true,
                    source_type: message
                        .pointer("/details/meta/source/type")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    source_value: message
                        .pointer("/details/meta/source/value")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    host_protected: false,
                });
                if let Some(images) = message.pointer("/details/images").filter(|images| images.is_array()) {
                    reduced.slots.push(ReductionContentSlot {
                        slot: RawContentSlot::ToolResultDetailsImages,
                        // details.images is opaque metadata, not a model content
                        // array. Native only recognizes the image discriminant.
                        content: ReductionContent::Blocks(
                            images
                                .as_array()
                                .expect("filtered array")
                                .iter()
                                .map(|value| {
                                    if value["type"] == "image" { ReductionBlock::Image } else { ReductionBlock::Other }
                                })
                                .collect(),
                        ),
                    });
                }
            }
        } else if entry.kind == "custom_message" {
            reduced.kind = ReductionEntryKind::CustomMessage {
                custom_type: raw["customType"].as_str().context("raw custom message has no string customType")?.into(),
            };
            reduced.slots.push(ReductionContentSlot {
                slot: RawContentSlot::CustomContent,
                content: raw_content(raw.get("content"))?,
            });
            if let Some(counter) = counter {
                let mut fragments = Vec::new();
                text_fragments(&raw["content"], true, &mut fragments);
                // Native custom string empty is zero even under exact families;
                // array text fragments, including empty ones, retain countTokens.
                if raw["content"].as_str() == Some("") {
                    fragments.clear();
                }
                if !fragments.is_empty() {
                    reduced.shake_entry_tokens = counter.measure(fragments.iter().map(|fragment| fragment.as_ref()))?;
                }
            }
        } else if entry.kind == "compaction" {
            reduced.kind = ReductionEntryKind::CompactionBoundary;
        }
        entries.push(reduced);
    }
    // Later duplicate calls override earlier ones, even if the newer call is
    // non-read. Protection is based on the paired original assistant call.
    let mut read_paths = HashMap::new();
    for call in entries.iter().flat_map(|entry| &entry.tool_calls) {
        read_paths.insert(
            call.id.clone(),
            (call.name == "read").then(|| call.arguments.get("path")?.as_str().map(str::to_owned)).flatten(),
        );
    }
    for result in entries.iter_mut().filter_map(|entry| entry.tool_result.as_mut()) {
        if result.tool_name == "read"
            && let Some(Some(path)) = read_paths.get(&result.call_id)
        {
            result.host_protected = targets_plan(path, "local://PLAN.md")
                || policy.protected_read_paths.iter().any(|target| targets_plan(path, target));
        }
    }
    Ok(entries)
}

pub fn raw_entries(
    snapshot: &SessionReductionSnapshot,
    model: &Model,
    policy: &HostReductionPolicy,
) -> Result<Vec<ReductionEntry>> {
    map_entries(snapshot, Some(&ModelCounter::new(model)), policy)
}

fn session_slot(slot: &RawContentSlot) -> SessionReductionSlot {
    match slot {
        RawContentSlot::MessageContent => SessionReductionSlot::MessageContent,
        RawContentSlot::CustomContent => SessionReductionSlot::CustomContent,
        RawContentSlot::ToolResultDetailsImages => SessionReductionSlot::ToolResultDetailsImages,
        RawContentSlot::FileMentionImage(index) => SessionReductionSlot::FileMentionImage(*index),
    }
}

pub fn session_edits(plan: &ReductionPlan) -> Vec<SessionReductionEdit> {
    plan.edits
        .iter()
        .map(|edit| SessionReductionEdit {
            entry_id: edit.entry_id.clone(),
            action: match &edit.action {
                ReductionAction::ReplaceToolResultContent { text, pruned_at_ms } => {
                    SessionReductionAction::ReplaceToolResultContent { text: text.clone(), pruned_at_ms: *pruned_at_ms }
                }
                ReductionAction::ElideToolResultText { text, pruned_at_ms } => {
                    SessionReductionAction::ElideToolResultText { text: text.clone(), pruned_at_ms: *pruned_at_ms }
                }
                ReductionAction::ReplaceTextRange {
                    slot,
                    block_index,
                    start_utf16,
                    end_utf16,
                    expected_text,
                    replacement,
                } => SessionReductionAction::ReplaceTextRange {
                    slot: session_slot(slot),
                    block_index: *block_index,
                    start_utf16: *start_utf16,
                    end_utf16: *end_utf16,
                    expected_text: expected_text.clone(),
                    replacement: replacement.clone(),
                },
                ReductionAction::DropBlocks { slot, indexes } => {
                    SessionReductionAction::DropBlocks { slot: session_slot(slot), indexes: indexes.clone() }
                }
                ReductionAction::DropImages { slot, indexes, placeholder_if_empty } => {
                    SessionReductionAction::DropImages {
                        slot: session_slot(slot),
                        indexes: indexes.clone(),
                        placeholder_if_empty: *placeholder_if_empty,
                    }
                }
            },
        })
        .collect()
}

fn prepared(
    snapshot: SessionReductionSnapshot,
    plan: ReductionPlan,
    token_estimation: TokenEstimation,
) -> PreparedReduction {
    let edits = session_edits(&plan);
    PreparedReduction {
        snapshot,
        tokens_freed: plan.estimated_tokens_saved,
        plan,
        edits,
        regions: Vec::new(),
        entry_tokens_freed: Vec::new(),
        artifact_id: None,
        artifact_error: None,
        token_estimation,
    }
}

pub fn prepare_prune(
    snapshot: SessionReductionSnapshot,
    model: &Model,
    config: &PruneConfig,
    policy: &HostReductionPolicy,
    stamp_ms: i64,
) -> Result<PreparedReduction> {
    let counter = ModelCounter::new(model);
    let entries = map_entries(&snapshot, Some(&counter), policy)?;
    let mut config = config.clone();
    config.keep_boundary_id.clone_from(&policy.keep_boundary_id);
    config.cache_warm_suffix_tokens = Some(if policy.prefix_binding {
        0.0
    } else {
        config.cache_warm_suffix_tokens.unwrap_or(PRUNE_CACHE_WARM_SUFFIX_TOKENS)
    });
    let plan = core::plan_prune(&entries, &config, None, stamp_ms)?;
    Ok(prepared(snapshot, plan, counter.estimation()))
}

pub fn prepare_stale(
    snapshot: SessionReductionSnapshot,
    model: &Model,
    config: &SupersedePruneConfig,
    supersede_reads: bool,
    policy: &HostReductionPolicy,
    now_ms: f64,
    stamp_ms: i64,
) -> Result<PreparedReduction> {
    let counter = ModelCounter::new(model);
    let entries = map_entries(&snapshot, Some(&counter), policy)?;
    let mut config = config.clone();
    config.keep_boundary_id.clone_from(&policy.keep_boundary_id);
    if policy.prefix_binding {
        config.suffix_token_limit = 0.0;
    }
    let key: &SupersedeKeyFn<'_> = &read_tool_supersede_key;
    let plan = core::plan_superseded_prune(&entries, &config, supersede_reads.then_some(key), now_ms, stamp_ms)?;
    Ok(prepared(snapshot, plan, counter.estimation()))
}

pub async fn prepare_shake(
    snapshot: SessionReductionSnapshot,
    model: &Model,
    config: &ShakeConfig,
    policy: &HostReductionPolicy,
    artifacts: &SessionArtifacts,
    stamp_ms: i64,
) -> Result<PreparedReduction> {
    let counter = ModelCounter::new(model);
    let entries = map_entries(&snapshot, Some(&counter), policy)?;
    let mut config = config.clone();
    config.keep_boundary_id.clone_from(&policy.keep_boundary_id);
    let regions = core::collect_shake_regions(&entries, &counter, &config)?;
    counter.checked()?;
    if regions.is_empty() {
        let plan = core::finalize_shake_plan(&entries, &[], &[], stamp_ms)?;
        return Ok(prepared(snapshot, plan, counter.estimation()));
    }
    let mut document = String::new();
    for (index, region) in regions.iter().enumerate() {
        if index > 0 {
            document.push('\n');
        }
        writeln!(
            &mut document,
            "### region {} ({}, ~{} tok)\n\n{}",
            index + 1,
            region.label,
            region.tokens,
            region.original_text
        )?;
    }
    let (artifact_id, artifact_error) = match artifacts.save(&document, "shake").await {
        Ok(id) => (Some(id), None),
        Err(error) => (None, Some(format!("{error:#}"))),
    };
    let replacements: Vec<String> = regions
        .iter()
        .enumerate()
        .map(|(index, region)| core::shake_placeholder(region.tokens, artifact_id.as_deref(), index))
        .collect();
    let mut original_tokens: usize = 0;
    let mut replacement_tokens: usize = 0;
    let mut entry_tokens_freed: Vec<(String, usize)> = Vec::new();
    for (region, replacement) in regions.iter().zip(&replacements) {
        let replacement_count = if replacement.is_empty() { 0 } else { counter.measure([replacement.as_str()])? };
        original_tokens = original_tokens.saturating_add(region.tokens);
        replacement_tokens = replacement_tokens.saturating_add(replacement_count);
        let removed = region.tokens.saturating_sub(replacement_count);
        if let Some(entry) = entry_tokens_freed.last_mut().filter(|entry| entry.0 == region.entry_id) {
            entry.1 = entry.1.saturating_add(removed);
        } else {
            entry_tokens_freed.push((region.entry_id.clone(), removed));
        }
    }
    let plan = core::finalize_shake_plan(&entries, &regions, &replacements, stamp_ms)?;
    let mut prepared = prepared(snapshot, plan, counter.estimation());
    prepared.tokens_freed = original_tokens.saturating_sub(replacement_tokens);
    prepared.regions = regions;
    prepared.entry_tokens_freed = entry_tokens_freed;
    prepared.artifact_id = artifact_id;
    prepared.artifact_error = artifact_error;
    Ok(prepared)
}

pub fn prepare_images(snapshot: SessionReductionSnapshot, _model: &Model) -> Result<PreparedReduction> {
    prepare_images_after_boundary(snapshot, _model, None)
}

/// Native replay has already absorbed older raw history. Keep that source
/// evidence intact while reducing only the represented tail after its carrier.
pub fn prepare_images_after_boundary(
    snapshot: SessionReductionSnapshot,
    _model: &Model,
    boundary: Option<&str>,
) -> Result<PreparedReduction> {
    let entries = map_entries(&snapshot, None, &HostReductionPolicy::default())?;
    let start = boundary
        .map(|id| {
            entries.iter().position(|entry| entry.entry_id == id).context("native replay reduction boundary is absent")
        })
        .transpose()?
        .unwrap_or(0);
    let plan = core::plan_drop_images(&entries[start..])?;
    Ok(prepared(snapshot, plan, TokenEstimation::NotMeasured))
}

pub fn prepare_thinking(snapshot: SessionReductionSnapshot, _model: &Model) -> Result<PreparedReduction> {
    prepare_thinking_after_boundary(snapshot, _model, None)
}

pub fn prepare_thinking_after_boundary(
    snapshot: SessionReductionSnapshot,
    _model: &Model,
    boundary: Option<&str>,
) -> Result<PreparedReduction> {
    let entries = map_entries(&snapshot, None, &HostReductionPolicy::default())?;
    let start = boundary
        .map(|id| {
            entries.iter().position(|entry| entry.entry_id == id).context("native replay reduction boundary is absent")
        })
        .transpose()?
        .unwrap_or(0);
    let plan = core::plan_drop_thinking(&entries[start..])?;
    Ok(prepared(snapshot, plan, TokenEstimation::NotMeasured))
}
