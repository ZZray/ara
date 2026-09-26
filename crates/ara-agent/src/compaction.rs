//! Source-preserving input for a future explicit soft-compaction call.
//!
//! Follows fixed OMP `packages/agent/src/compaction/utils.ts` and
//! `compaction.ts` at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! ARA uses JSONL provenance, excludes private reasoning, and bounds input.
//! This module does not choose a cut point, call a model or change context.

use std::io::{self, Write};

use ara_ai::{AssistantBlock, JsonObject, Message, UserBlock, UserContent};
use serde::Serialize;

const TOOL_RESULT_MAX_CHARS: usize = 2_000;
const MAX_SUMMARY_INPUT_BYTES: usize = 1_000_000;
const MAX_SUMMARY_SOURCES: usize = 256;

#[derive(Clone, Copy)]
pub struct SummarySource<'a> {
    pub entry_id: &'a str,
    pub message: &'a Message,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryInputError {
    EmptySources,
    InvalidSourceId,
    UnsupportedImage,
    TooManySources,
    TooLarge,
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
                let unknown_effect = result.details.as_ref().is_some_and(|details| {
                    details.get("__synthetic").and_then(serde_json::Value::as_bool) == Some(true)
                        && details.get("source").and_then(serde_json::Value::as_str)
                            == Some("interrupted_unknown_effect")
                        && details.get("executed").and_then(serde_json::Value::as_str) == Some("unknown")
                });
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
    if sources.is_empty() {
        return Err(SummaryInputError::EmptySources);
    }
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
    if user_prompt.len() > MAX_SUMMARY_INPUT_BYTES {
        return Err(SummaryInputError::TooLarge);
    }
    Ok(SummaryPrompt { system_prompt: SUMMARIZATION_SYSTEM_PROMPT, user_prompt })
}
