//! Source-preserving input for a future explicit soft-compaction call.
//!
//! Follows fixed OMP `packages/agent/src/compaction/utils.ts` and
//! `compaction.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! ARA uses JSONL provenance, excludes private reasoning, and bounds input.
//! This module plans structural whole-turn cuts and a provisional recent-message
//! target, but does not write a Session compaction or change model context.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::time::Instant;

use ara_ai::{
    AssistantBlock, AssistantMessage, AssistantMessageEvent, CallOptions, Context, JsonObject, Message, Model,
    ModelProvider, StopReason, ToolChoice, ToolResultMessage, Usage, UserBlock, UserContent, UserMessage,
};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::tokenizer::{MessageCountOptions, count_message};

const TOOL_RESULT_MAX_CHARS: usize = 2_000;
const MAX_SUMMARY_INPUT_BYTES: usize = 1_000_000;
const MAX_SUMMARY_SOURCES: usize = 256;
const MAX_SUMMARY_OUTPUT_BYTES: usize = 1_000_000;

#[derive(Clone, Copy)]
pub struct SummarySource<'a> {
    pub entry_id: &'a str,
    pub message: &'a Message,
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
        }
    }
}

impl std::error::Error for SummaryInputError {}

/// A one-shot request prepared from source-tagged, lower-trust history. This
/// is not a context-fit claim; callers must enforce model limits and only
/// accept a completed, nonempty summary before committing any rewrite.
pub struct SummaryPrompt {
    pub system_prompt: &'static str,
    pub user_prompt: String,
}

const SUMMARIZATION_SYSTEM_PROMPT: &str = include_str!("../prompts/summarization-system.md");
const SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compaction-summary.md");
const UPDATE_SUMMARIZATION_PROMPT: &str = include_str!("../prompts/compaction-update-summary.md");

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
    if !pending.is_empty() || ends_with_user {
        return Err(SummaryInputError::UnfinishedTurn);
    }
    Ok(())
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
#[serde(tag = "role", rename_all = "snake_case")]
enum SummaryEntry<'a> {
    User {
        entry_id: &'a str,
        content: String,
    },
    Developer {
        entry_id: &'a str,
        content: String,
    },
    Assistant {
        entry_id: &'a str,
        blocks: Vec<SummaryBlock<'a>>,
        stop_reason: &'a str,
    },
    ToolResult {
        entry_id: &'a str,
        tool_call_id: &'a str,
        tool_name: &'a str,
        content: String,
        is_error: bool,
        unknown_effect: bool,
    },
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
        let entry = match source.message {
            Message::User(user) => {
                SummaryEntry::User { entry_id: source.entry_id, content: text_content(&user.content)? }
            }
            Message::Developer(developer) => {
                SummaryEntry::Developer { entry_id: source.entry_id, content: text_content(&developer.content)? }
            }
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
                SummaryEntry::Assistant {
                    entry_id: source.entry_id,
                    blocks,
                    stop_reason: assistant.stop_reason.as_str(),
                }
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
                SummaryEntry::ToolResult {
                    entry_id: source.entry_id,
                    tool_call_id: &result.tool_call_id,
                    tool_name: &result.tool_name,
                    content: truncate_tool_result(&content),
                    is_error: result.is_error,
                    unknown_effect,
                }
            }
        };
        let remaining = MAX_SUMMARY_INPUT_BYTES.saturating_sub(output.len());
        let mut buffer = BoundedBuffer { bytes: Vec::new(), max: remaining };
        serde_json::to_writer(&mut buffer, &entry).map_err(|_| SummaryInputError::TooLarge)?;
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
    /// IDs for this summary window only. A previous summary has separate
    /// provenance that the Session owner must carry forward when updating it.
    pub window_source_entry_ids: Vec<String>,
    pub model_id: String,
    pub response_id: Option<String>,
    pub usage: Usage,
    pub duration_ms: Option<u64>,
    pub ttft_ms: Option<u64>,
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
    }
}

fn rejected_terminal(kind: SummaryCallErrorKind, message: &AssistantMessage) -> SummaryCallError {
    let mut failure = rejected(kind, Some(message.usage.clone()));
    failure.provider_status = message.error_status;
    failure.stop_reason = Some(message.stop_reason);
    failure.provider_message = message.error_message.as_deref().map(|text| text.chars().take(512).collect());
    failure
}

fn accept_summary_response(
    window_source_entry_ids: Vec<String>,
    model: &Model,
    reason: StopReason,
    message: AssistantMessage,
    saw_tool_call_event: bool,
) -> Result<AcceptedSummary, SummaryCallError> {
    if reason != StopReason::Stop
        || message.stop_reason != StopReason::Stop
        || message.error_message.is_some()
        || message.error_status.is_some()
    {
        let mut failure = rejected_terminal(SummaryCallErrorKind::IncompleteResponse, &message);
        if reason != StopReason::Stop {
            failure.stop_reason = Some(reason);
        }
        return Err(failure);
    }
    if saw_tool_call_event || message.tool_calls().next().is_some() {
        return Err(rejected_terminal(SummaryCallErrorKind::UnexpectedToolCall, &message));
    }
    let mut text = String::new();
    let mut first_text = true;
    for block in &message.content {
        match block {
            AssistantBlock::Text(part) => {
                let separator = usize::from(!first_text);
                if part.text.len() > MAX_SUMMARY_OUTPUT_BYTES.saturating_sub(text.len()).saturating_sub(separator) {
                    return Err(rejected_terminal(SummaryCallErrorKind::SummaryTooLarge, &message));
                }
                if separator != 0 {
                    text.push('\n');
                }
                text.push_str(&part.text);
                first_text = false;
            }
            AssistantBlock::ToolCall(_) => {
                return Err(rejected_terminal(SummaryCallErrorKind::UnexpectedToolCall, &message));
            }
            AssistantBlock::Image(_) => {
                return Err(rejected_terminal(SummaryCallErrorKind::UnsupportedResponseImage, &message));
            }
            AssistantBlock::Thinking(_) | AssistantBlock::RedactedThinking { .. } => {}
        }
    }
    if text.trim().is_empty() {
        return Err(rejected_terminal(SummaryCallErrorKind::EmptySummary, &message));
    }
    Ok(AcceptedSummary {
        text,
        window_source_entry_ids,
        model_id: model.id.clone(),
        response_id: message.response_id,
        usage: message.usage,
        duration_ms: message.duration,
        ttft_ms: message.ttft,
    })
}

/// Make one bounded logical summary call. This layer never runs tools, retries,
/// edits the journal, or treats partial output as accepted. The provider may
/// retry HTTP before streaming according to its own configuration. The host must
/// choose a model/window that fits the prepared prompt and must validate
/// source IDs against its Session branch before committing the derived text.
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

/// The same bounded, no-tools summary call with fixed OMP's optional focus.
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
    let prompt = build_summary_prompt_with_instructions(sources, previous_summary, custom_instructions)
        .map_err(|error| rejected(SummaryCallErrorKind::InvalidInput(error), None))?;
    if max_output_tokens == 0 {
        return Err(rejected(SummaryCallErrorKind::InvalidMaxTokens, None));
    }
    if cancel.is_cancelled() {
        return Err(rejected(SummaryCallErrorKind::Cancelled, None));
    }
    if Instant::now() >= deadline {
        return Err(rejected(SummaryCallErrorKind::Deadline, None));
    }

    let window_source_entry_ids = sources.iter().map(|source| source.entry_id.to_owned()).collect();
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
                return Err(rejected(SummaryCallErrorKind::Cancelled, None));
            }
            _ = &mut timeout => {
                provider_cancel.cancel();
                return Err(rejected(SummaryCallErrorKind::Deadline, None));
            }
            event = events.recv() => event,
        };
        match event {
            Some(AssistantMessageEvent::Done { reason, message }) => {
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected(SummaryCallErrorKind::Cancelled, Some(message.usage)));
                }
                if Instant::now() >= deadline.into_std() {
                    provider_cancel.cancel();
                    return Err(rejected(SummaryCallErrorKind::Deadline, Some(message.usage)));
                }
                let accepted =
                    accept_summary_response(window_source_entry_ids, model, reason, message, saw_tool_call_event)?;
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected(SummaryCallErrorKind::Cancelled, Some(accepted.usage)));
                }
                if Instant::now() >= deadline.into_std() {
                    provider_cancel.cancel();
                    return Err(rejected(SummaryCallErrorKind::Deadline, Some(accepted.usage)));
                }
                return Ok(accepted);
            }
            Some(AssistantMessageEvent::Error { error, .. }) => {
                if cancel.is_cancelled() {
                    provider_cancel.cancel();
                    return Err(rejected_terminal(SummaryCallErrorKind::Cancelled, &error));
                }
                return Err(rejected_terminal(SummaryCallErrorKind::ProviderError, &error));
            }
            Some(
                AssistantMessageEvent::ToolcallStart { .. }
                | AssistantMessageEvent::ToolcallDelta { .. }
                | AssistantMessageEvent::ToolcallEnd { .. },
            ) => saw_tool_call_event = true,
            Some(_) => {}
            None => return Err(rejected(SummaryCallErrorKind::StreamEndedWithoutTerminal, None)),
        }
    }
}
