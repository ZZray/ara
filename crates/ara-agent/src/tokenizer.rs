//! Local token estimates for the message forms currently supported by ARA.
//!
//! The fragment and message rules follow OMP `packages/agent/src/tokenizer.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see
//! THIRD_PARTY_NOTICES.md). The separate Claude content counter is selected
//! through host-owned model metadata; other families remain unimplemented.
//! These estimates are for sizing and display, not a context-limit gate.
//! Raw byte length is not a universal token upper bound: pinned Claude
//! fixtures contain content counts larger than their UTF-8 byte lengths.

pub use ara_ai::model_tokenizer::{ModelContentCount, count_model_fragments};
use ara_ai::{AssistantBlock, Message, UserBlock, UserContent};

/// OMP's fixed tool-result image estimate, also used for ARA assistant images.
pub const IMAGE_TOKEN_ESTIMATE: usize = 1200;
/// Fixed snapcompact estimate, only for images attached to derived summaries.
pub const SNAPCOMPACT_FRAME_TOKEN_ESTIMATE: usize = 5024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstimateMode {
    /// Sum `ceil(UTF-8 bytes / 4)` separately for each fragment.
    Approximate,
    /// Sum raw UTF-8 byte lengths. This is a size observation, not a token bound.
    RawUtf8Bytes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageCountOptions {
    /// Exclude opaque reasoning bytes from a future compaction floor estimate.
    pub exclude_encrypted_reasoning: bool,
}

pub fn count_text(text: &str, mode: EstimateMode) -> usize {
    match mode {
        EstimateMode::Approximate => text.len().saturating_add(3) / 4,
        EstimateMode::RawUtf8Bytes => text.len(),
    }
}

pub fn count_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>, mode: EstimateMode) -> usize {
    fragments.into_iter().fold(0usize, |sum, fragment| sum.saturating_add(count_text(fragment, mode)))
}

fn count_user_content(content: &UserContent) -> usize {
    match content {
        UserContent::Text(text) => count_text(text, EstimateMode::Approximate),
        // OMP counts text blocks in user content. Its image estimate applies to
        // tool results, so user images are intentionally omitted here too.
        UserContent::Blocks(blocks) => blocks.iter().fold(0usize, |sum, block| match block {
            UserBlock::Text(text) => sum.saturating_add(count_text(&text.text, EstimateMode::Approximate)),
            UserBlock::Image(image) if image.compaction_frame => sum.saturating_add(SNAPCOMPACT_FRAME_TOKEN_ESTIMATE),
            UserBlock::Image(_) => sum,
        }),
    }
}

/// Estimate one raw settled message, not its provider-transformed wire replay.
/// For example, the current OpenAI adapter omits assistant reasoning on replay.
/// This has no memoized state, so edits and clones cannot retain stale counts.
pub fn count_message(message: &Message, options: MessageCountOptions) -> usize {
    match message {
        Message::User(user) => count_user_content(&user.content),
        // Developer messages are an ARA wire role absent from this OMP union;
        // their content has the same representation as a user message.
        Message::Developer(developer) => count_user_content(&developer.content),
        Message::Assistant(assistant) => assistant.content.iter().fold(0usize, |sum, block| {
            let count = match block {
                AssistantBlock::Text(text) => count_text(&text.text, EstimateMode::Approximate),
                AssistantBlock::Thinking(thinking) => {
                    let visible = count_text(&thinking.thinking, EstimateMode::Approximate);
                    if options.exclude_encrypted_reasoning {
                        visible
                    } else {
                        visible.saturating_add(
                            thinking
                                .thinking_signature
                                .as_deref()
                                .map_or(0, |signature| count_text(signature, EstimateMode::Approximate)),
                        )
                    }
                }
                AssistantBlock::RedactedThinking { data } => {
                    if options.exclude_encrypted_reasoning {
                        0
                    } else {
                        count_text(data, EstimateMode::Approximate)
                    }
                }
                AssistantBlock::ToolCall(call) => {
                    count_text(&call.name, EstimateMode::Approximate).saturating_add(count_text(
                        &serde_json::to_string(&call.arguments).unwrap_or_else(|_| "null".to_owned()),
                        EstimateMode::Approximate,
                    ))
                }
                // Rust's assistant image variant is not present in OMP's
                // AgentMessage union; apply its tool-result image estimate.
                AssistantBlock::Image(_) => IMAGE_TOKEN_ESTIMATE,
            };
            sum.saturating_add(count)
        }),
        Message::ToolResult(result) => result.content.iter().fold(0usize, |sum, block| {
            let count = match block {
                UserBlock::Text(text) => count_text(&text.text, EstimateMode::Approximate),
                UserBlock::Image(_) => IMAGE_TOKEN_ESTIMATE,
            };
            sum.saturating_add(count)
        }),
    }
}

pub fn count_messages(messages: &[Message], options: MessageCountOptions) -> usize {
    messages.iter().fold(0usize, |sum, message| sum.saturating_add(count_message(message, options)))
}
