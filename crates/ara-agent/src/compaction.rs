//! Source-preserving input for a future explicit soft-compaction call.
//!
//! Follows fixed OMP `packages/agent/src/compaction/utils.ts` and
//! `compaction.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! ARA uses JSONL provenance, excludes private reasoning, and bounds input.
//! Native message cuts retain recent work and summarize split turn prefixes;
//! legacy whole-turn helpers remain separate. This never writes a Session.

use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{BuildHasher, Hasher};
use std::io::{self, Write};
use std::time::{Duration, Instant};

use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, CallOptions, Context, ContextRecoveryEvidence, JsonObject,
    Message, Model, ModelProvider, StopReason, ToolChoice, ToolResultMessage, Usage, UserBlock, UserContent,
    UserMessage,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::tokenizer::{
    EstimateMode, MessageCountOptions, ModelContentCount, count_message, count_model_fragments, count_text,
};
use ara_ai::retry_classification::{RetryClass, classify_retry};

#[path = "local_reduction.rs"]
pub mod local_reduction;

const TOOL_RESULT_MAX_CHARS: usize = 2_000;
const MAX_SUMMARY_INPUT_BYTES: usize = 1_000_000;
const MAX_SUMMARY_SOURCES: usize = 256;
const MAX_SUMMARY_OUTPUT_BYTES: usize = 1_000_000;
pub const MAX_SUMMARY_TOKENS: u64 = 16_384;
const DEFAULT_SUMMARY_INPUT_WINDOW: f64 = 200_000.0;

#[derive(Clone, Copy)]
pub struct SummarySource<'a> {
    pub entry_id: &'a str,
    pub message: &'a Message,
}

/// The raw Session entry kind, independently of its runtime model projection.
/// Hosts must derive this from the checked journal source, not the wire role.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeEntryOrigin {
    User,
    Assistant,
    ToolResult,
    Developer,
    BashExecution,
    HookMessage,
    FileMention,
    LegacyCustomMessage,
    LegacyBranchSummary,
    LegacyCompactionSummary,
    CustomMessage,
    UserSkill,
    SteeringUser,
    LoopGuardNotice,
    BranchSummary,
    Metadata,
    CompactionBoundary,
}

impl NativeEntryOrigin {
    fn is_raw_message(self) -> bool {
        matches!(
            self,
            Self::User
                | Self::Assistant
                | Self::ToolResult
                | Self::Developer
                | Self::BashExecution
                | Self::HookMessage
                | Self::FileMention
                | Self::LegacyCustomMessage
                | Self::LegacyBranchSummary
                | Self::LegacyCompactionSummary
        )
    }

    fn is_cut_point(self) -> bool {
        !matches!(
            self,
            Self::ToolResult
                | Self::Developer
                | Self::LegacyCustomMessage
                | Self::FileMention
                | Self::Metadata
                | Self::CompactionBoundary
        )
    }

    fn starts_turn(self) -> bool {
        matches!(
            self,
            Self::User
                | Self::BashExecution
                | Self::CustomMessage
                | Self::UserSkill
                | Self::SteeringUser
                | Self::LoopGuardNotice
                | Self::BranchSummary
        )
    }

    fn permits_historical_developer(self) -> bool {
        matches!(
            self,
            Self::HookMessage
                | Self::FileMention
                | Self::LegacyCustomMessage
                | Self::CustomMessage
                | Self::LoopGuardNotice
        )
    }
}

/// One real raw entry may project to zero, one, or several ordered messages.
/// Cuts and provenance never split a group or manufacture fragment entry IDs.
#[derive(Clone, Copy)]
pub struct NativeEntrySource<'a> {
    pub entry_id: &'a str,
    pub origin: NativeEntryOrigin,
    pub messages: &'a [Message],
    /// Estimate the original raw `type=message` once. Non-message entries
    /// contribute zero to fixed OMP's reverse keep budget.
    pub raw_message_tokens: usize,
}

/// A structural boundary before a user message. The preceding messages pass
/// completed-turn and receipt validation; this entry and later messages stay
/// raw. Prompt construction must still reject unsupported payloads or size.
/// The ID is caller-supplied until checked against a Session source snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WholeTurnCutCandidate {
    pub first_kept_index: usize,
    pub first_kept_entry_id: String,
}

/// A provisional cut selected using raw-message estimates. The retained count
/// is neither a provider-request count nor a context-fit verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WholeTurnCutSelection {
    pub candidate: WholeTurnCutCandidate,
    pub estimated_retained_raw_tokens: usize,
    pub estimated_retained_exceeds_target: bool,
}

/// Fixed OMP's cut can retain an assistant and its following tool results.
/// Indexes refer to messages in the legacy API and raw entry groups in the
/// native entry API. The host owns the current Session source proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeCompactionCut {
    pub first_kept_index: usize,
    pub first_kept_entry_id: String,
    pub turn_start_index: Option<usize>,
    pub history_end_index: usize,
    pub estimated_retained_raw_tokens: usize,
    pub estimated_retained_exceeds_target: bool,
}

/// Native one-shot budget; None in SummaryOptions explicitly disables it when
/// the caller owns an outer compaction retry budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SummaryRetryPolicy {
    pub max_attempts: usize,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for SummaryRetryPolicy {
    fn default() -> Self {
        Self { max_attempts: 3, base_delay_ms: 500, max_delay_ms: 30_000 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SummaryOptions {
    pub oneshot_retry: Option<SummaryRetryPolicy>,
    /// Caller output cap applied independently to history and turn prefix.
    pub max_tokens: Option<u64>,
}

impl Default for SummaryOptions {
    fn default() -> Self {
        Self { oneshot_retry: Some(SummaryRetryPolicy::default()), max_tokens: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryInputError {
    EmptySources,
    InvalidSourceId,
    UnsupportedImage,
    TooManySources,
    TooLarge,
    DuplicateSourceId,
    DeveloperInSummary,
    UnfinishedTurn,
    UnpairedToolResult,
    UnknownToolEffect,
    NoLeadingPrompt,
    InvalidEntryProjection,
}

impl std::fmt::Display for SummaryInputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptySources => f.write_str("compaction has no source messages to summarize"),
            Self::InvalidSourceId => f.write_str("compaction source has an invalid entry ID"),
            Self::UnsupportedImage => {
                f.write_str("compaction input contains an image that this summarizer cannot preserve")
            }
            Self::TooManySources => f.write_str("compaction has too many source messages for one summary request"),
            Self::TooLarge => f.write_str("compaction input is too large for one summary request"),
            Self::DuplicateSourceId => f.write_str("compaction source entry IDs must be unique"),
            Self::DeveloperInSummary => f.write_str("compaction cannot lower a developer message into summary text"),
            Self::UnfinishedTurn => {
                f.write_str("compaction source span ends with an unanswered prompt or a tool call without its result")
            }
            Self::UnpairedToolResult => f.write_str("compaction source span has an unmatched tool result"),
            Self::UnknownToolEffect => {
                f.write_str("compaction cannot hide a tool call whose execution effect is unknown")
            }
            Self::NoLeadingPrompt => f.write_str("compaction source span does not start with a user prompt"),
            Self::InvalidEntryProjection => f.write_str("compaction raw entry kind and model projection do not match"),
        }
    }
}

impl std::error::Error for SummaryInputError {}

/// A one-shot request prepared from source-tagged, lower-trust history. This
/// is not a context-fit claim; callers must enforce model limits and only
/// accept a completed, nonempty summary before committing any rewrite.
#[derive(Clone)]
pub struct SummaryPrompt {
    pub system_prompt: &'static str,
    pub user_prompt: String,
}

const SUMMARIZATION_SYSTEM_PROMPT: &str = include_str!("../prompts/summarization-system.md");
const SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compaction-summary.md");
const UPDATE_SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compaction-update-summary.md");
const TURN_PREFIX_SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compaction-turn-prefix.md");

/// Escape harness-owned tags in untrusted text. Each candidate scans only to
/// the next delimiter, so malformed input cannot cause quadratic work.
pub fn escape_summary_boundary_tags(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        output.push_str(&rest[..start]);
        let candidate = &rest[start..];
        let next_delimiter = candidate[1..].find(['<', '>']).map(|offset| offset + 1);
        if let Some(end) = next_delimiter.filter(|end| candidate.as_bytes()[*end] == b'>') {
            let tag = &candidate[..=end];
            let inner = tag[1..tag.len() - 1].trim();
            let name = inner.strip_prefix('/').unwrap_or(inner).trim();
            if name.eq_ignore_ascii_case("conversation") || name.eq_ignore_ascii_case("previous-summary") {
                output.push_str("&lt;");
                output.push_str(&tag[1..]);
                rest = &candidate[end + 1..];
                continue;
            }
        }
        output.push('<');
        rest = &candidate[1..];
    }
    output.push_str(rest);
    output
}

fn text_content(content: &UserContent) -> Result<String, SummaryInputError> {
    match content {
        UserContent::Text(text) => {
            if text.len() > MAX_SUMMARY_INPUT_BYTES {
                return Err(SummaryInputError::TooLarge);
            }
            Ok(text.clone())
        }
        UserContent::Blocks(blocks) => {
            let mut text = String::new();
            for block in blocks {
                match block {
                    UserBlock::Text(part) => {
                        if part.text.len() > MAX_SUMMARY_INPUT_BYTES.saturating_sub(text.len()) {
                            return Err(SummaryInputError::TooLarge);
                        }
                        text.push_str(&part.text);
                    }
                    UserBlock::Image(_) => return Err(SummaryInputError::UnsupportedImage),
                }
            }
            Ok(text)
        }
    }
}

fn truncate_tool_result(text: &str) -> String {
    let mut chars = text.chars();
    let kept: String = chars.by_ref().take(TOOL_RESULT_MAX_CHARS).collect();
    let omitted = chars.count();
    if omitted == 0 { kept } else { format!("{kept}\n\n[... {omitted} more characters truncated]") }
}

fn has_unknown_tool_effect(result: &ToolResultMessage) -> bool {
    result.details.as_ref().is_some_and(|details| {
        details.get("panicked").and_then(serde_json::Value::as_bool) == Some(true)
            || (details.get("__synthetic").and_then(serde_json::Value::as_bool) == Some(true)
                && details.get("source").and_then(serde_json::Value::as_str) == Some("interrupted_unknown_effect")
                && details.get("executed").and_then(serde_json::Value::as_str) == Some("unknown"))
    })
}

/// Check that a proposed span pairs every tool call with its result, does not
/// end with an unanswered prompt, and does not hide an unknown-effect tool
/// call. As in fixed OMP `findValidCutPoints`, a turn that ended with an
/// aborted, errored or length-stopped assistant, or after tool results when a
/// host budget stopped the loop, is still summarized: the agent loop pairs
/// the calls such an assistant did not run with a synthetic `executed: false`
/// result, and a call it blocked, failed to validate or skipped after a cancel
/// with an ordinary error result. A call that was running when the user
/// cancelled it keeps the tool's own error result (for Bash, the partial
/// output and `[Command aborted]`, or `Command aborted` with no output) and is
/// summarized like any failed call, as in OMP. A Bash call that timed out ends
/// the same way (the tool kills its process group) and keeps its own result
/// with `[Command timed out after N seconds]`, so it is summarized too.
/// Panicked and interrupted-unknown results are refused, which is stricter
/// than OMP. The Session owner must still prove IDs and messages correspond to
/// the current branch and commit against that branch's leaf.
pub fn validate_completed_summary_span(sources: &[SummarySource<'_>]) -> Result<(), SummaryInputError> {
    validate_summary_span(sources, false)
}

fn validate_summary_span(
    sources: &[SummarySource<'_>],
    allow_unanswered_prompt: bool,
) -> Result<(), SummaryInputError> {
    if sources.is_empty() {
        return Err(SummaryInputError::EmptySources);
    }
    if sources.len() > MAX_SUMMARY_SOURCES {
        return Err(SummaryInputError::TooManySources);
    }
    let mut seen_ids = HashSet::new();
    let mut pending: HashMap<&str, &str> = HashMap::new();
    let mut ends_with_user = false;
    for source in sources {
        if source.entry_id.is_empty() || !source.entry_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(SummaryInputError::InvalidSourceId);
        }
        if !seen_ids.insert(source.entry_id) {
            return Err(SummaryInputError::DuplicateSourceId);
        }
        match source.message {
            Message::User(_) => {
                if !pending.is_empty() {
                    return Err(SummaryInputError::UnfinishedTurn);
                }
                ends_with_user = true;
            }
            Message::Developer(_) => return Err(SummaryInputError::DeveloperInSummary),
            Message::Assistant(assistant) => {
                if !pending.is_empty() {
                    return Err(SummaryInputError::UnfinishedTurn);
                }
                ends_with_user = false;
                for call in assistant.tool_calls() {
                    if pending.insert(&call.id, &call.name).is_some() {
                        return Err(SummaryInputError::UnfinishedTurn);
                    }
                }
            }
            Message::ToolResult(result) => {
                if has_unknown_tool_effect(result) {
                    return Err(SummaryInputError::UnknownToolEffect);
                }
                match pending.remove(result.tool_call_id.as_str()) {
                    Some(name) if name == result.tool_name => {}
                    _ => return Err(SummaryInputError::UnpairedToolResult),
                }
                ends_with_user = false;
            }
        }
    }
    if !pending.is_empty() || (ends_with_user && !allow_unanswered_prompt) {
        return Err(SummaryInputError::UnfinishedTurn);
    }
    Ok(())
}

fn native_cut_at(
    sources: &[SummarySource<'_>],
    index: usize,
    previous_summary: Option<&str>,
) -> Result<(Option<usize>, usize), SummaryInputError> {
    let Some(first) = sources.first() else { return Err(SummaryInputError::EmptySources) };
    if !matches!(first.message, Message::User(_))
        && !(matches!(first.message, Message::Assistant(_))
            && previous_summary.is_some_and(|summary| !summary.trim().is_empty()))
    {
        return Err(if matches!(first.message, Message::Developer(_)) {
            SummaryInputError::DeveloperInSummary
        } else {
            SummaryInputError::NoLeadingPrompt
        });
    }
    let Some(kept) = sources.get(index) else { return Err(SummaryInputError::InvalidSourceId) };
    let split = matches!(kept.message, Message::Assistant(_));
    if index == 0 || !matches!(kept.message, Message::User(_) | Message::Assistant(_)) {
        return Err(SummaryInputError::EmptySources);
    }
    // Validate the complete discarded prefix together, so a history/prefix
    // split never makes an orphan tool result look safe. A user-only prefix is
    // intentional: the retained assistant answers that very user request.
    validate_summary_span(&sources[..index], split)?;
    let mut ids = HashSet::new();
    for source in &sources[..=index] {
        if source.entry_id.is_empty() || !source.entry_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(SummaryInputError::InvalidSourceId);
        }
        if !ids.insert(source.entry_id) {
            return Err(SummaryInputError::DuplicateSourceId);
        }
    }
    if split {
        let turn_start = (0..index).rev().find(|&i| matches!(sources[i].message, Message::User(_))).unwrap_or(0);
        Ok((Some(turn_start), turn_start))
    } else {
        Ok((None, index))
    }
}

/// Message-only port of fixed compaction.ts:499. Walk backward until the
/// accumulated estimate reaches the target, then select the first user or
/// assistant boundary at or after that index. With no later valid boundary,
/// native retains its first boundary; that no-op returns None here. This is
/// deliberately not a suffix-at-most-target search. The host must still prove
/// the exact source snapshot and map raw non-message entry boundaries.
pub fn select_native_compaction_cut(
    sources: &[SummarySource<'_>],
    keep_recent_tokens: usize,
    previous_summary: Option<&str>,
) -> Result<Option<NativeCompactionCut>, SummaryInputError> {
    let cut_points: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| {
            matches!(source.message, Message::User(_) | Message::Assistant(_)).then_some(index)
        })
        .collect();
    let Some(&first_cut) = cut_points.first() else { return Ok(None) };
    let mut suffix_tokens = vec![0usize; sources.len() + 1];
    let mut cut_index = first_cut;
    for index in (0..sources.len()).rev() {
        suffix_tokens[index] = suffix_tokens[index + 1]
            .saturating_add(count_message(sources[index].message, MessageCountOptions::default()));
    }
    for index in (0..sources.len()).rev() {
        if suffix_tokens[index] >= keep_recent_tokens {
            if let Some(&valid) = cut_points.iter().find(|&&valid| valid >= index) {
                cut_index = valid;
            }
            break;
        }
    }
    if cut_index == 0 {
        return Ok(None);
    }
    let (turn_start_index, history_end_index) = native_cut_at(sources, cut_index, previous_summary)?;
    // Check payload support without treating the unanswered turn prefix as a
    // completed whole turn or exposing the retained tool cycle to summarization.
    serialize_sources_for_summary(&sources[..cut_index])?;
    let estimated_retained_raw_tokens = suffix_tokens[cut_index];
    Ok(Some(NativeCompactionCut {
        first_kept_index: cut_index,
        first_kept_entry_id: sources[cut_index].entry_id.to_owned(),
        turn_start_index,
        history_end_index,
        estimated_retained_raw_tokens,
        estimated_retained_exceeds_target: estimated_retained_raw_tokens > keep_recent_tokens,
    }))
}

fn validate_native_entry_sources(sources: &[NativeEntrySource<'_>]) -> Result<(), SummaryInputError> {
    let mut ids = HashSet::new();
    for source in sources {
        if source.entry_id.is_empty() || !source.entry_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(SummaryInputError::InvalidSourceId);
        }
        if !ids.insert(source.entry_id) {
            return Err(SummaryInputError::DuplicateSourceId);
        }
        let valid = match source.origin {
            NativeEntryOrigin::User
            | NativeEntryOrigin::UserSkill
            | NativeEntryOrigin::SteeringUser
            | NativeEntryOrigin::LegacyBranchSummary
            | NativeEntryOrigin::LegacyCompactionSummary
            | NativeEntryOrigin::BranchSummary => {
                matches!(source.messages, [Message::User(_)])
            }
            NativeEntryOrigin::Assistant => matches!(source.messages, [Message::Assistant(_)]),
            NativeEntryOrigin::ToolResult => matches!(source.messages, [Message::ToolResult(_)]),
            NativeEntryOrigin::Developer | NativeEntryOrigin::LoopGuardNotice => {
                matches!(source.messages, [Message::Developer(_)])
            }
            NativeEntryOrigin::BashExecution => {
                matches!(source.messages, [] | [Message::User(_)])
            }
            NativeEntryOrigin::FileMention => matches!(
                source.messages,
                [] | [Message::Developer(_)] | [Message::User(_)] | [Message::Developer(_), Message::User(_)]
            ),
            NativeEntryOrigin::CustomMessage
            | NativeEntryOrigin::HookMessage
            | NativeEntryOrigin::LegacyCustomMessage => {
                let text_only = |content: &UserContent| match content {
                    UserContent::Text(_) => true,
                    UserContent::Blocks(blocks) => blocks.iter().all(|block| matches!(block, UserBlock::Text(_))),
                };
                let has_image = |content: &UserContent| {
                    matches!(content,
                    UserContent::Blocks(blocks) if blocks.iter().any(|block| matches!(block, UserBlock::Image(_))))
                };
                match source.messages {
                    [Message::Developer(developer)] => text_only(&developer.content),
                    [Message::User(user)] => {
                        source.origin == NativeEntryOrigin::LegacyCustomMessage || has_image(&user.content)
                    }
                    [Message::Developer(developer), Message::User(user)] => {
                        text_only(&developer.content) && has_image(&user.content)
                    }
                    _ => false,
                }
            }
            NativeEntryOrigin::Metadata | NativeEntryOrigin::CompactionBoundary => source.messages.is_empty(),
        };
        if !valid || (!source.origin.is_raw_message() && source.raw_message_tokens != 0) {
            return Err(SummaryInputError::InvalidEntryProjection);
        }
    }
    Ok(())
}

fn validate_native_summary_span(
    sources: &[NativeEntrySource<'_>],
    allow_unanswered_prompt: bool,
) -> Result<(), SummaryInputError> {
    validate_native_entry_sources(sources)?;
    let context_sources = sources.iter().filter(|source| !source.messages.is_empty()).count();
    if context_sources == 0 {
        return Err(SummaryInputError::EmptySources);
    }
    if context_sources > MAX_SUMMARY_SOURCES {
        return Err(SummaryInputError::TooManySources);
    }
    let mut pending: HashMap<&str, &str> = HashMap::new();
    let mut ends_with_prompt = false;
    for source in sources {
        for message in source.messages {
            match message {
                Message::User(_) => {
                    if !pending.is_empty() {
                        return Err(SummaryInputError::UnfinishedTurn);
                    }
                    ends_with_prompt |= matches!(
                        source.origin,
                        NativeEntryOrigin::User
                            | NativeEntryOrigin::UserSkill
                            | NativeEntryOrigin::SteeringUser
                            | NativeEntryOrigin::BashExecution
                            | NativeEntryOrigin::LegacyCustomMessage
                    );
                }
                Message::Developer(_) => {
                    if !source.origin.permits_historical_developer() {
                        return Err(SummaryInputError::DeveloperInSummary);
                    }
                    if !pending.is_empty() {
                        return Err(SummaryInputError::UnfinishedTurn);
                    }
                    // Historical notes retain their Developer wire role but
                    // cannot answer an earlier real user request.
                }
                Message::Assistant(assistant) => {
                    if !pending.is_empty() {
                        return Err(SummaryInputError::UnfinishedTurn);
                    }
                    ends_with_prompt = false;
                    for call in assistant.tool_calls() {
                        if pending.insert(&call.id, &call.name).is_some() {
                            return Err(SummaryInputError::UnfinishedTurn);
                        }
                    }
                }
                Message::ToolResult(result) => {
                    if has_unknown_tool_effect(result) {
                        return Err(SummaryInputError::UnknownToolEffect);
                    }
                    match pending.remove(result.tool_call_id.as_str()) {
                        Some(name) if name == result.tool_name => {}
                        _ => return Err(SummaryInputError::UnpairedToolResult),
                    }
                    ends_with_prompt = false;
                }
            }
        }
    }
    if !pending.is_empty() || (ends_with_prompt && !allow_unanswered_prompt) {
        return Err(SummaryInputError::UnfinishedTurn);
    }
    Ok(())
}

fn native_entry_cut_at(
    sources: &[NativeEntrySource<'_>],
    index: usize,
    previous_summary: Option<&str>,
) -> Result<(Option<usize>, usize), SummaryInputError> {
    validate_native_entry_sources(sources)?;
    let Some(first) = sources.iter().find(|source| !source.messages.is_empty()) else {
        return Err(SummaryInputError::EmptySources);
    };
    if first.origin == NativeEntryOrigin::Assistant
        && !previous_summary.is_some_and(|summary| !summary.trim().is_empty())
    {
        return Err(SummaryInputError::NoLeadingPrompt);
    }
    let Some(kept) = sources.get(index) else { return Err(SummaryInputError::InvalidSourceId) };
    if index == 0 {
        return Err(SummaryInputError::EmptySources);
    }
    let previous = sources[index - 1].origin;
    if !previous.is_raw_message() && previous != NativeEntryOrigin::CompactionBoundary {
        return Err(SummaryInputError::InvalidSourceId);
    }
    let mut native_boundary = false;
    for source in &sources[index..] {
        if source.origin.is_cut_point() {
            native_boundary = true;
            break;
        }
        if source.origin.is_raw_message() || source.origin == NativeEntryOrigin::CompactionBoundary {
            break;
        }
    }
    if !native_boundary {
        return Err(SummaryInputError::InvalidSourceId);
    }
    let turn_start = if kept.origin == NativeEntryOrigin::User {
        None
    } else {
        (0..=index).rev().find(|&i| sources[i].origin.starts_turn())
    };
    validate_native_summary_span(&sources[..index], kept.origin != NativeEntryOrigin::User)?;
    Ok((turn_start, turn_start.unwrap_or(index)))
}

/// Port fixed `findValidCutPoints` / `findCutPoint` over raw entry groups.
/// Only raw message entries trigger the reverse token threshold. Backtracking
/// includes adjacent raw metadata/custom/branch entries and stops at any raw
/// message or compaction barrier. The final raw origin decides turn splitting,
/// independently of the runtime Developer/User projection of custom content.
pub fn select_native_entry_compaction_cut(
    sources: &[NativeEntrySource<'_>],
    keep_recent_tokens: usize,
    previous_summary: Option<&str>,
) -> Result<Option<NativeCompactionCut>, SummaryInputError> {
    select_native_entry_cut(sources, keep_recent_tokens, previous_summary, true)
}

/// Native remote encoders preserve images and provider replay. Select the
/// identical structural raw cut without applying a text summarizer's codec.
pub fn select_native_remote_compaction_cut(
    sources: &[NativeEntrySource<'_>],
    keep_recent_tokens: usize,
    previous_summary: Option<&str>,
) -> Result<Option<NativeCompactionCut>, SummaryInputError> {
    select_native_entry_cut(sources, keep_recent_tokens, previous_summary, false)
}

fn select_native_entry_cut(
    sources: &[NativeEntrySource<'_>],
    keep_recent_tokens: usize,
    previous_summary: Option<&str>,
    text_summary: bool,
) -> Result<Option<NativeCompactionCut>, SummaryInputError> {
    validate_native_entry_sources(sources)?;
    let cut_points: Vec<usize> = sources
        .iter()
        .enumerate()
        .filter_map(|(index, source)| source.origin.is_cut_point().then_some(index))
        .collect();
    let Some(&first_cut) = cut_points.first() else { return Ok(None) };
    let mut suffix_tokens = vec![0usize; sources.len() + 1];
    for index in (0..sources.len()).rev() {
        suffix_tokens[index] = suffix_tokens[index + 1].saturating_add(if sources[index].origin.is_raw_message() {
            sources[index].raw_message_tokens
        } else {
            0
        });
    }
    let mut cut_index = first_cut;
    for index in (0..sources.len()).rev() {
        if !sources[index].origin.is_raw_message() {
            continue;
        }
        if suffix_tokens[index] >= keep_recent_tokens {
            if let Some(&valid) = cut_points.iter().find(|&&valid| valid >= index) {
                cut_index = valid;
            }
            break;
        }
    }
    while cut_index > 0 {
        let previous = sources[cut_index - 1].origin;
        if previous.is_raw_message() || previous == NativeEntryOrigin::CompactionBoundary {
            break;
        }
        cut_index -= 1;
    }
    if cut_index == 0 || sources[..cut_index].iter().all(|source| source.messages.is_empty()) {
        return Ok(None);
    }
    let (turn_start_index, history_end_index) = native_entry_cut_at(sources, cut_index, previous_summary)?;
    if text_summary {
        serialize_native_entry_sources_for_summary(&sources[..cut_index])?;
    }
    let estimated_retained_raw_tokens = suffix_tokens[cut_index];
    Ok(Some(NativeCompactionCut {
        first_kept_index: cut_index,
        first_kept_entry_id: sources[cut_index].entry_id.to_owned(),
        turn_start_index,
        history_end_index,
        estimated_retained_raw_tokens,
        estimated_retained_exceeds_target: estimated_retained_raw_tokens > keep_recent_tokens,
    }))
}

/// Enumerate structurally safe, message-only, whole-turn cuts. Unlike fixed
/// OMP's split-turn cut, these never hide a partial tool cycle or turn prefix.
/// A developer message in the summarized prefix would lose its priority when
/// converted to summary text, so no cut after it is offered. A caller must
/// still build the summary prompt to check image/size support. This does not
/// choose a budget or prove the IDs match the active Session branch.
pub fn whole_turn_cut_candidates(sources: &[SummarySource<'_>]) -> Vec<WholeTurnCutCandidate> {
    if !sources.first().is_some_and(|source| matches!(source.message, Message::User(_))) {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let mut seen_ids = HashSet::new();
    seen_ids.insert(sources[0].entry_id);
    for index in 1..sources.len().min(MAX_SUMMARY_SOURCES + 1) {
        if matches!(sources[index - 1].message, Message::Developer(_)) {
            break;
        }
        let source = &sources[index];
        if matches!(source.message, Message::User(_))
            && !source.entry_id.is_empty()
            && source.entry_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            && !seen_ids.contains(source.entry_id)
            && validate_completed_summary_span(&sources[..index]).is_ok()
        {
            candidates.push(WholeTurnCutCandidate {
                first_kept_index: index,
                first_kept_entry_id: source.entry_id.to_owned(),
            });
        }
        seen_ids.insert(source.entry_id);
    }
    candidates
}

/// Why `whole_turn_cut_candidates` offers no cut, so a host can say so
/// instead of calling the history small. `None` when a cut exists.
/// `EmptySources` means no later prompt can start a kept tail, and
/// `TooManySources` that the only later prompts lie past the source limit.
/// Otherwise it is the first validation error of the longest prefix a cut
/// could summarize, so an early blocker is named even in a long history.
pub fn explain_no_whole_turn_cut(sources: &[SummarySource<'_>]) -> Option<SummaryInputError> {
    if !whole_turn_cut_candidates(sources).is_empty() {
        return None;
    }
    match sources.first().map(|source| source.message) {
        None => return Some(SummaryInputError::EmptySources),
        Some(Message::User(_)) => {}
        Some(Message::Developer(_)) => return Some(SummaryInputError::DeveloperInSummary),
        Some(_) => return Some(SummaryInputError::NoLeadingPrompt),
    }
    let window = sources.len().min(MAX_SUMMARY_SOURCES + 1);
    let Some(latest_prompt) = (1..window).rev().find(|&i| matches!(sources[i].message, Message::User(_))) else {
        let later_prompt = sources[window..].iter().any(|source| matches!(source.message, Message::User(_)));
        return Some(if later_prompt { SummaryInputError::TooManySources } else { SummaryInputError::EmptySources });
    };
    Some(validate_completed_summary_span(&sources[..latest_prompt]).err().unwrap_or(SummaryInputError::InvalidSourceId))
}

/// Keep as much recent raw history as the approximate target permits, using
/// only complete-turn candidates. If the newest turn alone exceeds the target,
/// keep that turn and report the overshoot. No cut is useful when the entire
/// history already meets the target. The chosen summary prefix must be
/// serializable. If it is not, earlier boundaries are tried so unsupported
/// content remains raw, even when that exceeds the estimated target. Callers
/// must still verify Session IDs, branch leaf, provider
/// request size and the final summary before any write or model replay.
pub fn select_whole_turn_cut(
    sources: &[SummarySource<'_>],
    keep_recent_tokens: usize,
    previous_summary: Option<&str>,
) -> Result<Option<WholeTurnCutSelection>, SummaryInputError> {
    let candidates = whole_turn_cut_candidates(sources);
    if candidates.is_empty() {
        return Ok(None);
    }
    let mut suffix_tokens = vec![0usize; sources.len() + 1];
    for index in (0..sources.len()).rev() {
        suffix_tokens[index] = suffix_tokens[index + 1]
            .saturating_add(count_message(sources[index].message, MessageCountOptions::default()));
    }
    if suffix_tokens[0] <= keep_recent_tokens {
        return Ok(None);
    }
    let selected_position = candidates
        .iter()
        .position(|candidate| suffix_tokens[candidate.first_kept_index] <= keep_recent_tokens)
        .unwrap_or(candidates.len() - 1);
    let mut prompt_error = None;
    for candidate in candidates[..=selected_position].iter().rev() {
        match build_summary_prompt(&sources[..candidate.first_kept_index], previous_summary) {
            Ok(_) => {
                let estimated_retained_raw_tokens = suffix_tokens[candidate.first_kept_index];
                return Ok(Some(WholeTurnCutSelection {
                    candidate: candidate.clone(),
                    estimated_retained_raw_tokens,
                    estimated_retained_exceeds_target: estimated_retained_raw_tokens > keep_recent_tokens,
                }));
            }
            Err(error @ (SummaryInputError::UnsupportedImage | SummaryInputError::TooLarge)) => {
                prompt_error.get_or_insert(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(prompt_error.expect("candidate prompt was attempted"))
}

#[derive(Serialize)]
struct SummaryEntry<'a> {
    entry_id: &'a str,
    #[serde(flatten)]
    message: SummaryMessage<'a>,
}

#[derive(Serialize)]
#[serde(tag = "role", rename_all = "snake_case")]
enum SummaryMessage<'a> {
    User { content: String },
    Developer { content: String },
    Assistant { blocks: Vec<SummaryBlock<'a>>, stop_reason: &'a str },
    ToolResult { tool_call_id: &'a str, tool_name: &'a str, content: String, is_error: bool, unknown_effect: bool },
}

#[derive(Serialize)]
struct NativeSummaryEntry<'a> {
    entry_id: &'a str,
    origin: NativeEntryOrigin,
    messages: Vec<SummaryMessage<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SummaryBlock<'a> {
    Text { text: &'a str },
    ToolCall { id: &'a str, name: &'a str, arguments: &'a JsonObject },
}

struct BoundedBuffer {
    bytes: Vec<u8>,
    max: usize,
}

impl Write for BoundedBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.max.saturating_sub(self.bytes.len()) {
            return Err(io::Error::other("summary input size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialize journal messages as JSONL lower-trust data. Source IDs and roles
/// are separate fields, so message content cannot impersonate another entry.
/// The caller must persist exact source IDs alongside any accepted summary.
/// This only serializes; it does not prove that turns or tool receipts are
/// complete enough to compact. Images fail explicitly. Thinking is excluded.
pub fn serialize_sources_for_summary(sources: &[SummarySource<'_>]) -> Result<String, SummaryInputError> {
    if sources.len() > MAX_SUMMARY_SOURCES {
        return Err(SummaryInputError::TooManySources);
    }
    let mut output = String::new();
    for source in sources {
        if source.entry_id.is_empty() || !source.entry_id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err(SummaryInputError::InvalidSourceId);
        }
        let entry = SummaryEntry { entry_id: source.entry_id, message: summary_message(source.message)? };
        append_summary_record(&mut output, &entry)?;
    }
    Ok(output)
}

fn summary_message(message: &Message) -> Result<SummaryMessage<'_>, SummaryInputError> {
    Ok(match message {
        Message::User(user) => SummaryMessage::User { content: text_content(&user.content)? },
        Message::Developer(developer) => SummaryMessage::Developer { content: text_content(&developer.content)? },
        Message::Assistant(assistant) => {
            let mut blocks = Vec::new();
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => blocks.push(SummaryBlock::Text { text: &text.text }),
                    AssistantBlock::ToolCall(call) => blocks.push(SummaryBlock::ToolCall {
                        id: &call.id,
                        name: &call.name,
                        arguments: &call.arguments,
                    }),
                    AssistantBlock::Image(_) => return Err(SummaryInputError::UnsupportedImage),
                    AssistantBlock::Thinking(_) | AssistantBlock::RedactedThinking { .. } => {}
                }
            }
            SummaryMessage::Assistant { blocks, stop_reason: assistant.stop_reason.as_str() }
        }
        Message::ToolResult(result) => {
            let mut content = String::new();
            for block in &result.content {
                match block {
                    UserBlock::Text(part) => {
                        if part.text.len() > MAX_SUMMARY_INPUT_BYTES.saturating_sub(content.len()) {
                            return Err(SummaryInputError::TooLarge);
                        }
                        content.push_str(&part.text);
                    }
                    UserBlock::Image(_) => return Err(SummaryInputError::UnsupportedImage),
                }
            }
            let unknown_effect = has_unknown_tool_effect(result);
            SummaryMessage::ToolResult {
                tool_call_id: &result.tool_call_id,
                tool_name: &result.tool_name,
                content: truncate_tool_result(&content),
                is_error: result.is_error,
                unknown_effect,
            }
        }
    })
}

fn append_summary_record(output: &mut String, entry: &impl Serialize) -> Result<(), SummaryInputError> {
    let remaining = MAX_SUMMARY_INPUT_BYTES.saturating_sub(output.len());
    let mut buffer = BoundedBuffer { bytes: Vec::new(), max: remaining };
    serde_json::to_writer(&mut buffer, entry).map_err(|_| SummaryInputError::TooLarge)?;
    let line = String::from_utf8(buffer.bytes).expect("JSON is UTF-8");
    let escaped = escape_summary_boundary_tags(&line);
    let separator = usize::from(!output.is_empty());
    if escaped.len() + separator > remaining {
        return Err(SummaryInputError::TooLarge);
    }
    if separator != 0 {
        output.push('\n');
    }
    output.push_str(&escaped);
    Ok(())
}

/// Serialize one nested JSONL record per context-bearing raw entry. Metadata
/// and excluded Bash have no prompt record or summary provenance. Every
/// projection fragment remains ordered inside its real source group; private
/// reasoning is omitted and any image fails before a provider call.
pub fn serialize_native_entry_sources_for_summary(
    sources: &[NativeEntrySource<'_>],
) -> Result<String, SummaryInputError> {
    validate_native_entry_sources(sources)?;
    if sources.iter().filter(|source| !source.messages.is_empty()).count() > MAX_SUMMARY_SOURCES {
        return Err(SummaryInputError::TooManySources);
    }
    let mut output = String::new();
    for source in sources.iter().filter(|source| !source.messages.is_empty()) {
        if source.messages.iter().any(|message| matches!(message, Message::Developer(_)))
            && !source.origin.permits_historical_developer()
        {
            return Err(SummaryInputError::DeveloperInSummary);
        }
        let messages = source.messages.iter().map(summary_message).collect::<Result<Vec<_>, _>>()?;
        let entry = NativeSummaryEntry { entry_id: source.entry_id, origin: source.origin, messages };
        append_summary_record(&mut output, &entry)?;
    }
    Ok(output)
}

/// Assemble fixed OMP summary boundaries and initial/update instruction.
/// Previous derived summaries remain lower-trust input.
pub fn build_summary_prompt(
    sources: &[SummarySource<'_>],
    previous_summary: Option<&str>,
) -> Result<SummaryPrompt, SummaryInputError> {
    build_summary_prompt_with_instructions(sources, previous_summary, None)
}

/// Fixed OMP appends a nonempty custom instruction after the initial/update
/// instruction, outside both lower-trust source boundaries. Its bytes remain
/// part of the same bounded summary request.
pub fn build_summary_prompt_with_instructions(
    sources: &[SummarySource<'_>],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<SummaryPrompt, SummaryInputError> {
    validate_completed_summary_span(sources)?;
    let conversation = serialize_sources_for_summary(sources)?;
    build_summary_prompt_from_conversation(&conversation, previous_summary, custom_instructions)
}

// Folding windows may split a tool cycle. Only the complete source span is
// validated; each window is serialized lower-trust data, never a live tool call.
fn build_summary_prompt_from_conversation(
    conversation: &str,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
) -> Result<SummaryPrompt, SummaryInputError> {
    let previous = previous_summary.filter(|s| !s.trim().is_empty());
    if previous.is_some_and(|s| s.len() > MAX_SUMMARY_INPUT_BYTES.saturating_sub(conversation.len())) {
        return Err(SummaryInputError::TooLarge);
    }
    let mut user_prompt = format!("<conversation>\n{conversation}\n</conversation>\n\n");
    if let Some(previous) = previous {
        user_prompt.push_str("<previous-summary>\n");
        user_prompt.push_str(&escape_summary_boundary_tags(previous));
        user_prompt.push_str("\n</previous-summary>\n\n");
    }
    user_prompt.push_str(if previous.is_some() { UPDATE_SUMMARIZATION_PROMPT } else { SUMMARIZATION_PROMPT });
    if let Some(instructions) = custom_instructions.filter(|instructions| !instructions.is_empty()) {
        const PREFIX: &str = "\n\nAdditional focus: ";
        if user_prompt.len().saturating_add(PREFIX.len()).saturating_add(instructions.len()) > MAX_SUMMARY_INPUT_BYTES {
            return Err(SummaryInputError::TooLarge);
        }
        user_prompt.push_str(PREFIX);
        user_prompt.push_str(instructions);
    }
    if user_prompt.len() > MAX_SUMMARY_INPUT_BYTES {
        return Err(SummaryInputError::TooLarge);
    }
    Ok(SummaryPrompt { system_prompt: SUMMARIZATION_SYSTEM_PROMPT, user_prompt })
}

/// A completed, visible summary. Raw assistant blocks (including private
/// reasoning) are deliberately not returned to the Session owner.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptedSummary {
    pub text: String,
    /// Actual final summary terminal; Length may yield usable visible text.
    /// This does not turn a length-stopped primary Agent Run into success.
    pub terminal_reason: StopReason,
    /// IDs for this summary window only. A previous summary has separate
    /// provenance that the Session owner must carry forward when updating it.
    pub window_source_entry_ids: Vec<String>,
    pub model_id: String,
    pub response_id: Option<String>,
    /// Legacy folds expose their final request usage. The native cut API sums
    /// actual invocation buckets only when every invocation reports that bucket.
    /// Every request, including rejected attempts, is in `invocations`.
    pub usage: Usage,
    pub duration_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
    pub invocations: Vec<SummaryInvocationReceipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryCallErrorKind {
    InvalidInput(SummaryInputError),
    InvalidMaxTokens,
    Cancelled,
    Deadline,
    StreamEndedWithoutTerminal,
    ProviderError,
    IncompleteResponse,
    UnexpectedToolCall,
    UnsupportedResponseImage,
    EmptySummary,
    SummaryTooLarge,
    OutputBudgetExceeded,
}

/// `usage` is present only when a terminal model message was received. Its
/// optional token fields retain provider unknowns rather than becoming zero.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryCallError {
    pub kind: SummaryCallErrorKind,
    pub usage: Option<Box<Usage>>,
    pub provider_status: Option<u16>,
    /// Stop reason of the terminal model message, when one was received.
    pub stop_reason: Option<StopReason>,
    /// Bounded provider diagnostic. Callers must still redact it before logs
    /// or user display because an upstream may echo request content.
    pub provider_message: Option<String>,
    pub invocations: Vec<SummaryInvocationReceipt>,
}

/// One actual summary provider invocation. Unknown usage stays unknown; these
/// receipts must not be mistaken for a locally priced or aggregated cost.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryInvocationReceipt {
    pub window_source_entry_ids: Vec<String>,
    pub usage: Option<Usage>,
    pub response_id: Option<String>,
    pub stop_reason: Option<StopReason>,
    pub provider_status: Option<u16>,
    pub error_kind: Option<SummaryCallErrorKind>,
    /// Classified from the complete terminal message before diagnostics truncate.
    pub classification: Option<RetryClass>,
    /// Fixed-prompt retry gate assessed on the complete terminal, separate
    /// from context-rewrite eligibility (ordinary usage admission may wait).
    pub oneshot_retry_eligible: bool,
    /// Maximum finite provider header/text hint before diagnostic truncation.
    pub oneshot_retry_wait_ms: Option<f64>,
    pub overflow_replanned: bool,
}

impl std::fmt::Display for SummaryCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "summary call failed: {:?}", self.kind)
    }
}

impl std::error::Error for SummaryCallError {}

fn rejected(kind: SummaryCallErrorKind, usage: Option<Usage>) -> SummaryCallError {
    SummaryCallError {
        kind,
        usage: usage.map(Box::new),
        provider_status: None,
        stop_reason: None,
        provider_message: None,
        invocations: Vec::new(),
    }
}

fn summary_oneshot_retry_eligible(message: &AssistantMessage, actual_api: &str) -> bool {
    let class = classify_retry(message, actual_api);
    let wire_veto = matches!(
        message.terminal_context_recovery,
        Some(ContextRecoveryEvidence::NativeOutput | ContextRecoveryEvidence::NativeValidation)
    ) || message.failure_evidence.as_ref().is_some_and(|evidence| {
        matches!(
            evidence.context_recovery,
            Some(ContextRecoveryEvidence::NativeOutput | ContextRecoveryEvidence::NativeValidation)
        )
    });
    class.retriable
        && !class.abort
        && !class.overflow
        && !class.replay_blocked
        && !wire_veto
        && message.tool_calls().next().is_none()
}

fn summary_oneshot_retry_wait(message: &AssistantMessage, actual_api: &str) -> Option<f64> {
    let header = message
        .failure_evidence
        .as_ref()
        .and_then(|evidence| evidence.wait_ms)
        .filter(|delay| delay.is_finite() && *delay >= 0.0);
    // Existing classification prefers its header evidence over text. One-shot
    // native policy takes the larger hint, so independently classify the text.
    let mut text_only = message.clone();
    text_only.failure_evidence = None;
    let extracted = classify_retry(&text_only, actual_api).wait_ms;
    let suffix = message.error_message.as_deref().unwrap_or("").split_whitespace().find_map(|part| {
        let (name, value) = part.split_once('=')?;
        if !name.eq_ignore_ascii_case("retry-after-ms") {
            return None;
        }
        let delay = value.parse::<f64>().ok()?;
        (delay.is_finite() && delay > 0.0).then_some(delay.ceil())
    });
    let text = match (extracted, suffix) {
        (Some(extracted), Some(suffix)) => Some(extracted.max(suffix)),
        (Some(delay), None) | (None, Some(delay)) => Some(delay),
        (None, None) => None,
    };
    match (header, text) {
        (Some(header), Some(text)) => Some(header.max(text)),
        (Some(delay), None) | (None, Some(delay)) => Some(delay),
        (None, None) => None,
    }
}

fn rejected_terminal(kind: SummaryCallErrorKind, message: &AssistantMessage, actual_api: &str) -> SummaryCallError {
    let mut failure = rejected(kind, Some(message.usage.clone()));
    failure.provider_status = message.error_status;
    failure.stop_reason = Some(message.stop_reason);
    failure.provider_message = message.error_message.as_deref().map(|text| text.chars().take(512).collect());
    failure.invocations.push(SummaryInvocationReceipt {
        window_source_entry_ids: Vec::new(),
        usage: Some(message.usage.clone()),
        response_id: message.response_id.clone(),
        stop_reason: Some(message.stop_reason),
        provider_status: message.error_status,
        error_kind: Some(kind),
        classification: Some(classify_retry(message, actual_api)),
        oneshot_retry_eligible: summary_oneshot_retry_eligible(message, actual_api),
        oneshot_retry_wait_ms: summary_oneshot_retry_wait(message, actual_api),
        overflow_replanned: false,
    });
    failure
}

fn rejected_terminal_event(
    kind: SummaryCallErrorKind,
    reason: StopReason,
    message: &AssistantMessage,
    actual_api: &str,
) -> SummaryCallError {
    let mut failure = rejected_terminal(kind, message, actual_api);
    failure.stop_reason = Some(reason);
    failure.invocations.last_mut().expect("terminal invocation").stop_reason = Some(reason);
    failure
}

fn rejected_invocation(kind: SummaryCallErrorKind, usage: Option<Usage>) -> SummaryCallError {
    let mut failure = rejected(kind, usage);
    failure.invocations.push(SummaryInvocationReceipt {
        window_source_entry_ids: Vec::new(),
        usage: failure.usage.as_deref().cloned(),
        response_id: None,
        stop_reason: None,
        provider_status: None,
        error_kind: Some(kind),
        classification: None,
        oneshot_retry_eligible: false,
        oneshot_retry_wait_ms: None,
        overflow_replanned: false,
    });
    failure
}

fn rejected_after_accept(kind: SummaryCallErrorKind, accepted: AcceptedSummary) -> SummaryCallError {
    let mut failure = rejected_invocation(kind, Some(accepted.usage));
    failure.stop_reason = Some(accepted.terminal_reason);
    let receipt = failure.invocations.last_mut().expect("completed invocation");
    receipt.stop_reason = Some(accepted.terminal_reason);
    receipt.response_id = accepted.response_id;
    failure
}

fn accept_summary_response(
    window_source_entry_ids: Vec<String>,
    model: &Model,
    reason: StopReason,
    message: AssistantMessage,
    saw_tool_call_event: bool,
) -> Result<AcceptedSummary, SummaryCallError> {
    let length_without_content_proof = reason == StopReason::Length
        && matches!(model.api.as_str(), "openai-responses" | "openai-codex-responses")
        && message.terminal_context_recovery != Some(ContextRecoveryEvidence::ContentOnly);
    if reason != message.stop_reason
        || !matches!(reason, StopReason::Stop | StopReason::Length)
        || length_without_content_proof
        || message.error_message.is_some()
        || message.error_status.is_some()
    {
        return Err(rejected_terminal_event(SummaryCallErrorKind::IncompleteResponse, reason, &message, &model.api));
    }
    if saw_tool_call_event || message.tool_calls().next().is_some() {
        return Err(rejected_terminal(SummaryCallErrorKind::UnexpectedToolCall, &message, &model.api));
    }
    let mut text = String::new();
    let mut first_text = true;
    for block in &message.content {
        match block {
            AssistantBlock::Text(part) => {
                let separator = usize::from(!first_text);
                if part.text.len() > MAX_SUMMARY_OUTPUT_BYTES.saturating_sub(text.len()).saturating_sub(separator) {
                    return Err(rejected_terminal(SummaryCallErrorKind::SummaryTooLarge, &message, &model.api));
                }
                if separator != 0 {
                    text.push('\n');
                }
                text.push_str(&part.text);
                first_text = false;
            }
            AssistantBlock::ToolCall(_) => {
                return Err(rejected_terminal(SummaryCallErrorKind::UnexpectedToolCall, &message, &model.api));
            }
            AssistantBlock::Image(_) => {
                return Err(rejected_terminal(SummaryCallErrorKind::UnsupportedResponseImage, &message, &model.api));
            }
            AssistantBlock::Thinking(_) | AssistantBlock::RedactedThinking { .. } => {}
        }
    }
    if text.trim().is_empty() {
        return Err(rejected_terminal(SummaryCallErrorKind::EmptySummary, &message, &model.api));
    }
    Ok(AcceptedSummary {
        text,
        terminal_reason: reason,
        window_source_entry_ids,
        model_id: model.id.clone(),
        response_id: message.response_id,
        usage: message.usage,
        duration_ms: message.duration,
        ttft_ms: message.ttft,
        invocations: Vec::new(),
    })
}

/// Native output budget uses the raw reserve, not the context-fit reserve.
/// Fixed `compaction.ts:1543,857` defaults to 16384 and caps at 16384.
pub fn summary_output_budget_tokens(raw_reserve: Option<f64>) -> Result<u64, SummaryCallError> {
    summary_budget_tokens(raw_reserve, 0.8)
}

fn summary_budget_tokens(raw_reserve: Option<f64>, fraction: f64) -> Result<u64, SummaryCallError> {
    let reserve = raw_reserve.unwrap_or(MAX_SUMMARY_TOKENS as f64);
    if !reserve.is_finite() || reserve < 0.0 {
        return Err(rejected(SummaryCallErrorKind::InvalidMaxTokens, None));
    }
    let output = (fraction * reserve).floor().min(MAX_SUMMARY_TOKENS as f64) as u64;
    if output == 0 {
        return Err(rejected(SummaryCallErrorKind::InvalidMaxTokens, None));
    }
    Ok(output)
}

fn summary_input_floor(model: &Model) -> u64 {
    let window = model
        .context_window
        .filter(|window| window.is_finite() && *window > 0.0)
        .unwrap_or(DEFAULT_SUMMARY_INPUT_WINDOW);
    (window / 8.0).floor().max(1024.0).min(MAX_SUMMARY_TOKENS as f64) as u64
}

fn summary_input_budget(model: &Model, max_output_tokens: u64) -> u64 {
    let window = model
        .context_window
        .filter(|window| window.is_finite() && *window > 0.0)
        .unwrap_or(DEFAULT_SUMMARY_INPUT_WINDOW);
    // Signed floating arithmetic reproduces native subtraction before the floor
    // is applied, without unsigned underflow on small advertised windows.
    ((window * 0.8).floor() - max_output_tokens as f64 - MAX_SUMMARY_TOKENS as f64)
        .max(summary_input_floor(model) as f64) as u64
}

fn summary_text_tokens(model: &Model, text: &str) -> u64 {
    match count_model_fragments(model, [text]) {
        ModelContentCount::Exact(count) => count,
        ModelContentCount::UnknownTokenizer => count_text(text, EstimateMode::Approximate) as u64,
        ModelContentCount::CountOverflow => u64::MAX,
    }
}

struct SummaryWindow {
    start: usize,
    end: usize,
    budget_tokens: u64,
    text: Option<String>,
}

// The fold/retry engine is shared by legacy message sources and native raw
// entry groups. Only input serialization, window boundaries and ID extraction
// vary; provider calls, cancellation and actual receipts have one owner.
#[derive(Clone, Copy)]
enum SummaryHistorySources<'a> {
    Messages(&'a [SummarySource<'a>]),
    NativeEntries(&'a [NativeEntrySource<'a>]),
}

impl SummaryHistorySources<'_> {
    fn len(self) -> usize {
        match self {
            Self::Messages(sources) => sources.len(),
            Self::NativeEntries(sources) => sources.len(),
        }
    }

    fn has_context(self) -> bool {
        match self {
            Self::Messages(sources) => !sources.is_empty(),
            Self::NativeEntries(sources) => sources.iter().any(|source| !source.messages.is_empty()),
        }
    }

    fn serialize(self, start: usize, end: usize) -> Result<String, SummaryInputError> {
        match self {
            Self::Messages(sources) => serialize_sources_for_summary(&sources[start..end]),
            Self::NativeEntries(sources) => serialize_native_entry_sources_for_summary(&sources[start..end]),
        }
    }

    fn ids(self, start: usize, end: usize) -> Vec<String> {
        match self {
            Self::Messages(sources) => sources[start..end].iter().map(|source| source.entry_id.to_owned()).collect(),
            Self::NativeEntries(sources) => sources[start..end]
                .iter()
                .filter(|source| !source.messages.is_empty())
                .map(|source| source.entry_id.to_owned())
                .collect(),
        }
    }
}

fn plan_summary_windows(
    sources: SummaryHistorySources<'_>,
    model: &Model,
    budget_tokens: u64,
    start: usize,
    end: usize,
) -> Result<Vec<SummaryWindow>, SummaryInputError> {
    let mut windows = Vec::new();
    let mut first = start;
    let mut current_tokens = 0u64;
    for index in start..end {
        let text = sources.serialize(index, index + 1)?;
        if text.is_empty() {
            continue;
        }
        let tokens = summary_text_tokens(model, &text);
        if current_tokens > 0 && current_tokens.saturating_add(tokens) > budget_tokens {
            windows.push(SummaryWindow { start: first, end: index, budget_tokens, text: None });
            first = index;
            current_tokens = 0;
        }
        current_tokens = current_tokens.saturating_add(tokens);
    }
    if first < end {
        windows.push(SummaryWindow { start: first, end, budget_tokens, text: None });
    }
    Ok(windows)
}

fn clamp_summary_conversation(text: &str, budget_tokens: u64, tokens: u64) -> String {
    if tokens <= budget_tokens {
        return text.to_owned();
    }
    // Native slices JS UTF-16 characters. Keep complete Rust scalars so a
    // surrogate pair/UTF-8 sequence is never split at that same prefix boundary.
    let characters = text.encode_utf16().count();
    let keep = ((characters as f64 * budget_tokens as f64 * 0.95) / tokens as f64).floor().max(1024.0) as usize;
    if keep >= characters {
        return text.to_owned();
    }
    let mut kept = 0;
    let end = text
        .char_indices()
        .find_map(|(index, character)| {
            let next = kept + character.len_utf16();
            if next > keep {
                Some(index)
            } else {
                kept = next;
                None
            }
        })
        .unwrap_or(text.len());
    format!("{}\n\n[... {} more characters truncated]", &text[..end], characters - kept)
}

/// Fold bounded source history through native model-window planning. Only a
/// fully accepted fold returns a summary; this layer never executes tools or
/// edits a journal. Local content counts plan calls, not a provider fit proof.
/// The host must still validate source IDs against its current Session branch.
pub async fn summarize_sources(
    sources: &[SummarySource<'_>],
    previous_summary: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    max_output_tokens: u64,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    summarize_sources_with_instructions(
        sources,
        previous_summary,
        None,
        model,
        provider,
        max_output_tokens,
        deadline,
        cancel,
    )
    .await
}

/// The same bounded, no-tools summary fold with fixed OMP's optional focus.
/// The explicit max-output argument remains a caller override. Native host
/// policy can obtain its default from `summary_output_budget_tokens`.
#[allow(clippy::too_many_arguments)]
pub async fn summarize_sources_with_instructions(
    sources: &[SummarySource<'_>],
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    max_output_tokens: u64,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    validate_completed_summary_span(sources)
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    summarize_history(
        SummaryHistorySources::Messages(sources),
        previous_summary,
        custom_instructions,
        model,
        provider,
        max_output_tokens,
        SummaryOptions { oneshot_retry: None, max_tokens: None },
        false,
        deadline,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn summarize_history(
    sources: SummaryHistorySources<'_>,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    max_output_tokens: u64,
    options: SummaryOptions,
    native_acceptance: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    let conversation = sources
        .serialize(0, sources.len())
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    if max_output_tokens == 0 {
        return Err(rejected(SummaryCallErrorKind::InvalidMaxTokens, None));
    }
    let budget = summary_input_budget(model, max_output_tokens);
    let mut pending = if summary_text_tokens(model, &conversation) <= budget {
        VecDeque::from([SummaryWindow {
            start: 0,
            end: sources.len(),
            budget_tokens: budget,
            text: Some(conversation),
        }])
    } else {
        plan_summary_windows(sources, model, budget, 0, sources.len())
            .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?
            .into()
    };
    let mut carried_summary = previous_summary.map(str::to_owned);
    let mut final_summary = None;
    let mut invocations = Vec::new();
    while let Some(window) = pending.pop_front() {
        let text = match window.text {
            Some(text) => text,
            None => sources
                .serialize(window.start, window.end)
                .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?,
        };
        let tokens = summary_text_tokens(model, &text);
        let conversation = clamp_summary_conversation(&text, window.budget_tokens, tokens);
        let prompt = match build_summary_prompt_from_conversation(
            &conversation,
            carried_summary.as_deref(),
            custom_instructions,
        ) {
            Ok(prompt) => prompt,
            Err(error) => {
                let mut failure = rejected(SummaryCallErrorKind::InvalidInput(error), None);
                failure.invocations = invocations;
                return Err(failure);
            }
        };
        let ids = sources.ids(window.start, window.end);
        match summarize_window_with_retry(
            prompt,
            model,
            provider,
            max_output_tokens,
            options,
            native_acceptance,
            deadline,
            cancel,
        )
        .await
        {
            Ok(mut accepted) => {
                for receipt in &mut accepted.invocations {
                    receipt.window_source_entry_ids = ids.clone();
                }
                invocations.append(&mut accepted.invocations);
                invocations.push(SummaryInvocationReceipt {
                    window_source_entry_ids: ids,
                    usage: Some(accepted.usage.clone()),
                    response_id: accepted.response_id.clone(),
                    stop_reason: Some(accepted.terminal_reason),
                    provider_status: None,
                    error_kind: None,
                    classification: None,
                    oneshot_retry_eligible: false,
                    oneshot_retry_wait_ms: None,
                    overflow_replanned: false,
                });
                carried_summary = Some(accepted.text.clone());
                accepted.window_source_entry_ids = sources.ids(0, sources.len());
                final_summary = Some(accepted);
            }
            Err(mut failure) => {
                if failure.invocations.is_empty() {
                    // Pre-cancel/deadline opens no provider request.
                    if matches!(failure.kind, SummaryCallErrorKind::Cancelled | SummaryCallErrorKind::Deadline)
                        && (cancel.is_cancelled() || Instant::now() >= deadline)
                        && failure.usage.is_none()
                    {
                        failure.invocations = invocations;
                        return Err(failure);
                    }
                    failure.invocations.push(SummaryInvocationReceipt {
                        window_source_entry_ids: Vec::new(),
                        usage: failure.usage.as_deref().cloned(),
                        response_id: None,
                        stop_reason: failure.stop_reason,
                        provider_status: failure.provider_status,
                        error_kind: Some(failure.kind),
                        classification: None,
                        oneshot_retry_eligible: false,
                        oneshot_retry_wait_ms: None,
                        overflow_replanned: false,
                    });
                }
                let halved = window.budget_tokens.min(tokens) / 2;
                for receipt in &mut failure.invocations {
                    receipt.window_source_entry_ids = ids.clone();
                }
                let receipt = failure.invocations.last_mut().expect("one actual invocation receipt");
                let replan =
                    receipt.classification.is_some_and(|class| class.overflow && !class.context_recovery_blocked)
                        && matches!(
                            failure.kind,
                            SummaryCallErrorKind::ProviderError | SummaryCallErrorKind::IncompleteResponse
                        )
                        && !cancel.is_cancelled()
                        && Instant::now() < deadline
                        && halved >= summary_input_floor(model);
                receipt.overflow_replanned = replan;
                invocations.append(&mut failure.invocations);
                if !replan {
                    failure.invocations = invocations;
                    return Err(failure);
                }
                let smaller = match plan_summary_windows(sources, model, halved, window.start, window.end) {
                    Ok(smaller) => smaller,
                    Err(error) => {
                        let mut failure = rejected(SummaryCallErrorKind::InvalidInput(error), None);
                        failure.invocations = invocations;
                        return Err(failure);
                    }
                };
                for smaller in smaller.into_iter().rev() {
                    pending.push_front(smaller);
                }
            }
        }
    }
    let mut accepted = final_summary.expect("validated sources produce at least one summary window");
    accepted.invocations = invocations;
    Ok(accepted)
}

fn accepted_invocation(accepted: &AcceptedSummary, ids: Vec<String>) -> SummaryInvocationReceipt {
    SummaryInvocationReceipt {
        window_source_entry_ids: ids,
        usage: Some(accepted.usage.clone()),
        response_id: accepted.response_id.clone(),
        stop_reason: Some(accepted.terminal_reason),
        provider_status: None,
        error_kind: None,
        classification: None,
        oneshot_retry_eligible: false,
        oneshot_retry_wait_ms: None,
        overflow_replanned: false,
    }
}

fn aggregate_summary_usage(receipts: &[SummaryInvocationReceipt]) -> Usage {
    if receipts.is_empty() {
        return Usage::unknown();
    }
    let sum = |bucket: fn(&Usage) -> Option<u64>| {
        receipts.iter().try_fold(0u64, |total, receipt| total.checked_add(bucket(receipt.usage.as_ref()?)?))
    };
    let cost = receipts.iter().try_fold(ara_ai::Cost::default(), |mut total, receipt| {
        let cost = receipt.usage.as_ref()?.cost.as_ref()?;
        total.input += cost.input;
        total.output += cost.output;
        total.cache_read += cost.cache_read;
        total.cache_write += cost.cache_write;
        total.total += cost.total;
        (total.input.is_finite()
            && total.output.is_finite()
            && total.cache_read.is_finite()
            && total.cache_write.is_finite()
            && total.total.is_finite())
        .then_some(total)
    });
    Usage {
        input: sum(|usage| usage.input),
        output: sum(|usage| usage.output),
        cache_read: sum(|usage| usage.cache_read),
        cache_write: sum(|usage| usage.cache_write),
        total_tokens: sum(|usage| usage.total_tokens),
        reasoning_tokens: sum(|usage| usage.reasoning_tokens),
        cost,
    }
}

/// Native split-turn compaction over a host-pinned message snapshot. History
/// uses the existing bounded fold and previous-summary/custom focus; the
/// independent turn-prefix call uses the exact upstream prompt and half the
/// raw reserve. All calls share cancellation/deadline, and any branch failure
/// discards every partial summary while retaining actual invocation receipts.
/// This never commits a Session entry or proves the raw entry adapter surface.
#[allow(clippy::too_many_arguments)]
pub async fn summarize_compaction_cut(
    sources: &[SummarySource<'_>],
    cut: &NativeCompactionCut,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    raw_reserve: Option<f64>,
    options: SummaryOptions,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    let (turn_start, history_end) = native_cut_at(sources, cut.first_kept_index, previous_summary)
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    if sources[cut.first_kept_index].entry_id != cut.first_kept_entry_id
        || cut.turn_start_index != turn_start
        || cut.history_end_index != history_end
    {
        return Err(rejected(SummaryCallErrorKind::InvalidInput(SummaryInputError::InvalidSourceId), None));
    }
    let history_sources = &sources[..history_end];
    let prefix_sources = &sources[history_end..cut.first_kept_index];
    let prefix_conversation = if prefix_sources.is_empty() {
        None
    } else {
        validate_summary_span(prefix_sources, true)
            .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
        Some(
            serialize_sources_for_summary(prefix_sources)
                .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?,
        )
    };
    summarize_prepared_compaction_cut(
        SummaryHistorySources::Messages(history_sources),
        prefix_conversation,
        prefix_sources.iter().map(|source| source.entry_id.to_owned()).collect(),
        sources[..cut.first_kept_index].iter().map(|source| source.entry_id.to_owned()).collect(),
        previous_summary,
        custom_instructions,
        model,
        provider,
        raw_reserve,
        options,
        deadline,
        cancel,
    )
    .await
}

/// Summarize a native raw cut without dropping or duplicating projection
/// fragments. Metadata has no summary record; custom/hook/LoopGuard historical
/// content retains its checked origin beside its runtime Developer role. This
/// uses the same fold, split-prefix, retry and receipt engine as the legacy API.
#[allow(clippy::too_many_arguments)]
pub async fn summarize_native_entry_compaction_cut(
    sources: &[NativeEntrySource<'_>],
    cut: &NativeCompactionCut,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    raw_reserve: Option<f64>,
    options: SummaryOptions,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    let (turn_start, history_end) = native_entry_cut_at(sources, cut.first_kept_index, previous_summary)
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    if sources[cut.first_kept_index].entry_id != cut.first_kept_entry_id
        || cut.turn_start_index != turn_start
        || cut.history_end_index != history_end
    {
        return Err(rejected(SummaryCallErrorKind::InvalidInput(SummaryInputError::InvalidSourceId), None));
    }
    // Validate every discarded fragment before opening either independent
    // request, including images in history while the prefix itself is text.
    serialize_native_entry_sources_for_summary(&sources[..cut.first_kept_index])
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    let history_sources = SummaryHistorySources::NativeEntries(&sources[..history_end]);
    let prefix_sources = &sources[history_end..cut.first_kept_index];
    let prefix_conversation = if prefix_sources.iter().all(|source| source.messages.is_empty()) {
        None
    } else {
        validate_native_summary_span(prefix_sources, true)
            .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
        Some(
            serialize_native_entry_sources_for_summary(prefix_sources)
                .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?,
        )
    };
    summarize_prepared_compaction_cut(
        history_sources,
        prefix_conversation,
        SummaryHistorySources::NativeEntries(prefix_sources).ids(0, prefix_sources.len()),
        SummaryHistorySources::NativeEntries(sources).ids(0, cut.first_kept_index),
        previous_summary,
        custom_instructions,
        model,
        provider,
        raw_reserve,
        options,
        deadline,
        cancel,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn summarize_prepared_compaction_cut(
    history_sources: SummaryHistorySources<'_>,
    prefix_conversation: Option<String>,
    prefix_source_entry_ids: Vec<String>,
    window_source_entry_ids: Vec<String>,
    previous_summary: Option<&str>,
    custom_instructions: Option<&str>,
    model: &Model,
    provider: &dyn ModelProvider,
    raw_reserve: Option<f64>,
    options: SummaryOptions,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    if options.max_tokens == Some(0) {
        return Err(rejected(SummaryCallErrorKind::InvalidMaxTokens, None));
    }
    let history_budget = summary_output_budget_tokens(raw_reserve)?.min(options.max_tokens.unwrap_or(u64::MAX));
    let prefix_budget = if prefix_conversation.is_none() {
        0
    } else {
        summary_budget_tokens(raw_reserve, 0.5)?.min(options.max_tokens.unwrap_or(u64::MAX))
    };
    let prefix_prompt = if let Some(conversation) = prefix_conversation {
        let user_prompt =
            format!("<conversation>\n{conversation}\n</conversation>\n\n{TURN_PREFIX_SUMMARIZATION_PROMPT}");
        if user_prompt.len() > MAX_SUMMARY_INPUT_BYTES {
            return Err(rejected(SummaryCallErrorKind::InvalidInput(SummaryInputError::TooLarge), None));
        }
        Some(SummaryPrompt { system_prompt: SUMMARIZATION_SYSTEM_PROMPT, user_prompt })
    } else {
        None
    };
    let has_history =
        history_sources.has_context() || previous_summary.is_some_and(|summary| !summary.trim().is_empty());
    let operation_cancel = cancel.child_token();
    let _operation_guard = operation_cancel.clone().drop_guard();
    let history_future = async {
        let result = if has_history {
            summarize_history(
                history_sources,
                previous_summary,
                custom_instructions,
                model,
                provider,
                history_budget,
                options,
                true,
                deadline,
                &operation_cancel,
            )
            .await
            .map(Some)
        } else {
            Ok(None)
        };
        if result.is_err() {
            operation_cancel.cancel();
        }
        result
    };
    let prefix_future = async {
        let result = if let Some(prompt) = prefix_prompt {
            let ids = prefix_source_entry_ids;
            match summarize_window_with_retry(
                prompt,
                model,
                provider,
                prefix_budget,
                options,
                true,
                deadline,
                &operation_cancel,
            )
            .await
            {
                Ok(mut accepted) => {
                    for receipt in &mut accepted.invocations {
                        receipt.window_source_entry_ids = ids.clone();
                    }
                    let receipt = accepted_invocation(&accepted, ids.clone());
                    accepted.invocations.push(receipt);
                    accepted.window_source_entry_ids = ids;
                    Ok(Some(accepted))
                }
                Err(mut failure) => {
                    for receipt in &mut failure.invocations {
                        receipt.window_source_entry_ids = ids.clone();
                    }
                    Err(failure)
                }
            }
        } else {
            Ok(None)
        };
        if result.is_err() {
            operation_cancel.cancel();
        }
        result
    };
    let (history_result, prefix_result) = tokio::join!(history_future, prefix_future);
    let mut receipts = Vec::new();
    for result in [&history_result, &prefix_result] {
        match result {
            Ok(Some(accepted)) => receipts.extend(accepted.invocations.iter().cloned()),
            Err(failure) => receipts.extend(failure.invocations.iter().cloned()),
            Ok(None) => {}
        }
    }
    // Prefer the original branch error over its sibling's cancellation receipt.
    let failure = [&history_result, &prefix_result]
        .into_iter()
        .filter_map(|result| result.as_ref().err())
        .find(|failure| failure.kind != SummaryCallErrorKind::Cancelled)
        .or_else(|| history_result.as_ref().err())
        .or_else(|| prefix_result.as_ref().err());
    if let Some(failure) = failure {
        let mut failure = failure.clone();
        failure.usage = (!receipts.is_empty()).then(|| Box::new(aggregate_summary_usage(&receipts)));
        failure.invocations = receipts;
        return Err(failure);
    }
    let history = history_result.expect("branch failures handled");
    let prefix = prefix_result.expect("branch failures handled");
    let mut accepted = match (history, prefix) {
        (Some(history), Some(mut prefix)) => {
            prefix.text = format!("{}\n\n---\n\n**Turn Context (split turn):**\n\n{}", history.text, prefix.text);
            if history.terminal_reason == StopReason::Length {
                prefix.terminal_reason = StopReason::Length;
            }
            prefix.response_id = None;
            prefix.duration_ms = None;
            prefix.ttft_ms = None;
            prefix
        }
        (None, Some(mut prefix)) => {
            prefix.text = format!("No prior history.\n\n---\n\n**Turn Context (split turn):**\n\n{}", prefix.text);
            prefix
        }
        (Some(history), None) => history,
        (None, None) => {
            return Err(rejected(SummaryCallErrorKind::InvalidInput(SummaryInputError::EmptySources), None));
        }
    };
    accepted.window_source_entry_ids = window_source_entry_ids;
    accepted.usage = aggregate_summary_usage(&receipts);
    accepted.invocations = receipts;
    if cancel.is_cancelled() || Instant::now() >= deadline {
        let mut failure = rejected(
            if cancel.is_cancelled() { SummaryCallErrorKind::Cancelled } else { SummaryCallErrorKind::Deadline },
            Some(accepted.usage),
        );
        failure.invocations = accepted.invocations;
        return Err(failure);
    }
    Ok(accepted)
}

#[allow(clippy::too_many_arguments)]
async fn summarize_window_with_retry(
    prompt: SummaryPrompt,
    model: &Model,
    provider: &dyn ModelProvider,
    max_output_tokens: u64,
    options: SummaryOptions,
    native_acceptance: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    let mut invocations = Vec::new();
    let mut attempt = 1usize;
    loop {
        match summarize_window(prompt.clone(), model, provider, max_output_tokens, native_acceptance, deadline, cancel)
            .await
        {
            Ok(mut accepted) => {
                accepted.invocations = invocations;
                return Ok(accepted);
            }
            Err(mut failure) => {
                let receipt = failure.invocations.last();
                let policy = options.oneshot_retry.filter(|policy| attempt < policy.max_attempts.max(1));
                let eligible = receipt.is_some_and(|receipt| receipt.oneshot_retry_eligible)
                    && matches!(
                        failure.kind,
                        SummaryCallErrorKind::ProviderError | SummaryCallErrorKind::IncompleteResponse
                    )
                    && !cancel.is_cancelled()
                    && Instant::now() < deadline;
                let wait = receipt.and_then(|receipt| receipt.oneshot_retry_wait_ms);
                invocations.append(&mut failure.invocations);
                let Some(policy) = policy.filter(|_| eligible) else {
                    failure.invocations = invocations;
                    return Err(failure);
                };
                if wait.is_some_and(|wait| wait > policy.max_delay_ms as f64) {
                    failure.invocations = invocations;
                    return Err(failure);
                }
                // RandomState is already independently seeded by std; no new
                // RNG dependency or synchronized timestamp jitter is needed.
                let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
                hasher.write_usize(attempt);
                let fraction = (hasher.finish() as u32) as f64 / u32::MAX as f64;
                let growth = (policy.base_delay_ms as f64 * 2f64.powi((attempt - 1).min(32) as i32)).min(8_000.0);
                let backoff = (growth * (0.75 + fraction * 0.25)).round();
                let delay_ms = wait.unwrap_or(0.0).max(backoff).min(policy.max_delay_ms as f64);
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let mut cancelled = rejected(SummaryCallErrorKind::Cancelled, None);
                        cancelled.invocations = invocations;
                        return Err(cancelled);
                    }
                    _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
                        let mut expired = rejected(SummaryCallErrorKind::Deadline, None);
                        expired.invocations = invocations;
                        return Err(expired);
                    }
                    _ = tokio::time::sleep(Duration::from_secs_f64(delay_ms / 1000.0)) => {}
                }
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

async fn summarize_window(
    prompt: SummaryPrompt,
    model: &Model,
    provider: &dyn ModelProvider,
    max_output_tokens: u64,
    native_acceptance: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<AcceptedSummary, SummaryCallError> {
    if cancel.is_cancelled() {
        return Err(rejected(SummaryCallErrorKind::Cancelled, None));
    }
    if Instant::now() >= deadline {
        return Err(rejected(SummaryCallErrorKind::Deadline, None));
    }

    let context = Context {
        system_prompt: vec![prompt.system_prompt.to_owned()],
        messages: vec![Message::User(UserMessage::text(prompt.user_prompt))],
        tools: Some(Vec::new()),
    };
    let provider_cancel = cancel.child_token();
    // A dropped summary future must stop the provider's spawned HTTP task.
    let _provider_guard = provider_cancel.clone().drop_guard();
    let mut events = provider.stream(
        model,
        &context,
        CallOptions {
            cancel: provider_cancel.clone(),
            tool_choice: Some(ToolChoice::None),
            max_tokens: Some(max_output_tokens),
            temperature: None,
            loop_guard: None,
            on_response: None,
        },
    );
    let deadline = tokio::time::Instant::from_std(deadline);
    let timeout = tokio::time::sleep_until(deadline);
    tokio::pin!(timeout);
    let mut saw_tool_call_event = false;
    loop {
        let event = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                provider_cancel.cancel();
                return Err(rejected_invocation(SummaryCallErrorKind::Cancelled, None));
            }
            _ = &mut timeout => {
                provider_cancel.cancel();
                return Err(rejected_invocation(SummaryCallErrorKind::Deadline, None));
            }
            event = events.recv() => event,
        };
        match event {
            Some(AssistantMessageEvent::Done { reason, message }) => {
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected_terminal_event(SummaryCallErrorKind::Cancelled, reason, &message, &model.api));
                }
                if Instant::now() >= deadline.into_std() {
                    provider_cancel.cancel();
                    return Err(rejected_terminal_event(SummaryCallErrorKind::Deadline, reason, &message, &model.api));
                }
                if native_acceptance
                    && (matches!(
                        message.terminal_context_recovery,
                        Some(ContextRecoveryEvidence::NativeOutput | ContextRecoveryEvidence::NativeValidation)
                    ) || message.failure_evidence.as_ref().is_some_and(|evidence| {
                        matches!(
                            evidence.context_recovery,
                            Some(ContextRecoveryEvidence::NativeOutput | ContextRecoveryEvidence::NativeValidation)
                        )
                    }))
                {
                    return Err(rejected_terminal_event(
                        SummaryCallErrorKind::IncompleteResponse,
                        reason,
                        &message,
                        &model.api,
                    ));
                }
                let mut result = accept_summary_response(Vec::new(), model, reason, message, saw_tool_call_event);
                if saw_tool_call_event && let Err(failure) = &mut result {
                    for receipt in &mut failure.invocations {
                        receipt.oneshot_retry_eligible = false;
                    }
                }
                let accepted = result?;
                if native_acceptance
                    && model.api == "openai-codex-responses"
                    && (accepted.usage.output.is_some_and(|output| output > max_output_tokens)
                        || count_text(&accepted.text, EstimateMode::Approximate) as u64 > max_output_tokens)
                {
                    return Err(rejected_after_accept(SummaryCallErrorKind::OutputBudgetExceeded, accepted));
                }
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected_after_accept(SummaryCallErrorKind::Cancelled, accepted));
                }
                if Instant::now() >= deadline.into_std() {
                    provider_cancel.cancel();
                    return Err(rejected_after_accept(SummaryCallErrorKind::Deadline, accepted));
                }
                return Ok(accepted);
            }
            Some(AssistantMessageEvent::Error { error, reason }) => {
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected_terminal_event(SummaryCallErrorKind::Cancelled, reason, &error, &model.api));
                }
                let mut failure =
                    rejected_terminal_event(SummaryCallErrorKind::ProviderError, reason, &error, &model.api);
                if saw_tool_call_event {
                    for receipt in &mut failure.invocations {
                        receipt.oneshot_retry_eligible = false;
                    }
                }
                return Err(failure);
            }
            Some(
                AssistantMessageEvent::ToolcallStart { .. }
                | AssistantMessageEvent::ToolcallDelta { .. }
                | AssistantMessageEvent::ToolcallEnd { .. },
            ) => saw_tool_call_event = true,
            Some(_) => {}
            None => return Err(rejected_invocation(SummaryCallErrorKind::StreamEndedWithoutTerminal, None)),
        }
    }
}
