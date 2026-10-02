//! Immutable model-aware counts for the message forms currently supported by ARA.
//!
//! The fragment and message rules follow OMP `packages/agent/src/tokenizer.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d (MIT; see
//! THIRD_PARTY_NOTICES.md). Host-owned catalog metadata selects the encoding.
//! Counts cover local content, not provider-transformed replay or wire framing.
//! Raw byte length is not a universal token upper bound: pinned Claude
//! fixtures contain content counts larger than their UTF-8 byte lengths.

pub use ara_ai::model_tokenizer::{ModelContentCount, count_model_fragments};
use ara_ai::model_tokenizer::{count_default_fragments, count_family_fragments};
use ara_ai::{AssistantBlock, Message, Model, ModelTokenizer, UserBlock, UserContent};
use std::borrow::Cow;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCountMode {
    /// Always use the selected family, or O200kBase for an unknown model.
    Strict,
    /// Prefer native counts under the captured policy, otherwise bytes / 4.
    Approximate,
    /// Prefer native counts under the captured policy, otherwise raw bytes.
    /// The name follows OMP; raw bytes are not a universal token upper bound.
    UpperBound,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenizerPolicy {
    /// Mirror OMP's test environment without relying on library `cfg(test)`.
    pub test_environment: bool,
    /// Use O200kBase for unknown models outside the test environment.
    pub accurate_unknown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenCountError {
    CountOverflow,
    NativeEncodingUnavailable,
}

impl std::fmt::Display for TokenCountError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::CountOverflow => "token count overflow",
            Self::NativeEncodingUnavailable => "selected native token encoding is unavailable",
        })
    }
}

impl std::error::Error for TokenCountError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TokenBudgetCheck {
    pub fits: bool,
    pub tokens: usize,
    /// True only when the strict native count completed without overflow.
    pub exact: bool,
}

/// Captures model metadata and policy at construction. Core never reads Host
/// environment variables. Messages are freshly counted because ARA's mutable
/// values have no stable identity/version protocol for OMP's weak cache.
#[derive(Clone, Copy, Debug)]
pub struct Tokenizer {
    encoding: Option<ModelTokenizer>,
    policy: TokenizerPolicy,
}

impl Tokenizer {
    pub fn for_model(model: &Model) -> Self {
        Self::with_policy(Some(model), TokenizerPolicy::default())
    }

    pub fn with_policy(model: Option<&Model>, policy: TokenizerPolicy) -> Self {
        Self { encoding: model.and_then(|model| model.tokenizer), policy }
    }

    pub fn encoding(&self) -> Option<ModelTokenizer> {
        self.encoding
    }

    pub fn uses_native(&self, mode: TokenCountMode) -> bool {
        mode == TokenCountMode::Strict
            || !self.policy.test_environment && (self.encoding.is_some() || self.policy.accurate_unknown)
    }

    pub fn count_fragments_checked<'a>(
        &self,
        fragments: impl IntoIterator<Item = &'a str>,
        mode: TokenCountMode,
    ) -> Result<usize, TokenCountError> {
        if self.uses_native(mode) {
            let count = match self.encoding {
                Some(family) => count_family_fragments(family, fragments),
                None => count_default_fragments(fragments),
            };
            return match count {
                ModelContentCount::Exact(tokens) => usize::try_from(tokens).map_err(|_| TokenCountError::CountOverflow),
                ModelContentCount::CountOverflow => Err(TokenCountError::CountOverflow),
                ModelContentCount::UnknownTokenizer => Err(TokenCountError::NativeEncodingUnavailable),
            };
        }
        let estimate = match mode {
            TokenCountMode::Approximate => EstimateMode::Approximate,
            TokenCountMode::UpperBound => EstimateMode::RawUtf8Bytes,
            TokenCountMode::Strict => unreachable!("strict counts always use a native encoding"),
        };
        checked_total(fragments.into_iter().map(|fragment| count_text(fragment, estimate)))
    }

    /// Saturation preserves an explicit over-budget signal; native failures
    /// never silently switch to an approximate count.
    pub fn count_fragments<'a>(&self, fragments: impl IntoIterator<Item = &'a str>, mode: TokenCountMode) -> usize {
        self.count_fragments_checked(fragments, mode).unwrap_or(usize::MAX)
    }

    pub fn count_text(&self, text: &str, mode: TokenCountMode) -> usize {
        self.count_fragments([text], mode)
    }

    /// Always measure strictly: fixed OMP's raw-byte shortcut is unsound for
    /// its Claude counter (for example, two-byte `ξ` counts as three).
    pub fn check_token_budget<'a>(
        &self,
        fragments: impl IntoIterator<Item = &'a str>,
        budget: usize,
    ) -> TokenBudgetCheck {
        match self.count_fragments_checked(fragments, TokenCountMode::Strict) {
            Ok(tokens) => TokenBudgetCheck { fits: tokens <= budget, tokens, exact: true },
            Err(_) => TokenBudgetCheck { fits: false, tokens: usize::MAX, exact: false },
        }
    }

    pub fn count_message_checked(
        &self,
        message: &Message,
        options: MessageCountOptions,
    ) -> Result<usize, TokenCountError> {
        let parts = message_parts(message, options);
        let text = self.count_fragments_checked(
            parts.fragments.iter().map(|fragment| fragment.as_ref()),
            TokenCountMode::Approximate,
        )?;
        checked_total([text, parts.extra?])
    }

    pub fn count_message(&self, message: &Message, options: MessageCountOptions) -> usize {
        self.count_message_checked(message, options).unwrap_or(usize::MAX)
    }

    pub fn count_messages(&self, messages: &[Message], options: MessageCountOptions) -> usize {
        messages.iter().fold(0usize, |sum, message| sum.saturating_add(self.count_message(message, options)))
    }
}

fn checked_total(counts: impl IntoIterator<Item = usize>) -> Result<usize, TokenCountError> {
    counts.into_iter().try_fold(0usize, |sum, count| sum.checked_add(count).ok_or(TokenCountError::CountOverflow))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessageCountOptions {
    /// Exclude opaque reasoning bytes from a future compaction floor estimate.
    pub exclude_encrypted_reasoning: bool,
}

pub fn count_text(text: &str, mode: EstimateMode) -> usize {
    match mode {
        EstimateMode::Approximate => text.len().div_ceil(4),
        EstimateMode::RawUtf8Bytes => text.len(),
    }
}

pub fn count_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>, mode: EstimateMode) -> usize {
    fragments.into_iter().fold(0usize, |sum, fragment| sum.saturating_add(count_text(fragment, mode)))
}

struct MessageParts<'a> {
    fragments: Vec<Cow<'a, str>>,
    extra: Result<usize, TokenCountError>,
}

impl MessageParts<'_> {
    fn add_extra(&mut self, tokens: usize) {
        self.extra = self.extra.and_then(|extra| checked_total([extra, tokens]));
    }
}

fn user_parts<'a>(content: &'a UserContent, parts: &mut MessageParts<'a>) {
    match content {
        UserContent::Text(text) => parts.fragments.push(Cow::Borrowed(text)),
        // Empty block text is omitted by OMP, while empty string content is a
        // fragment. Original user pictures are not charged by this estimator.
        UserContent::Blocks(blocks) => {
            for block in blocks {
                match block {
                    UserBlock::Text(text) if !text.text.is_empty() => parts.fragments.push(Cow::Borrowed(&text.text)),
                    UserBlock::Image(image) if image.compaction_frame => {
                        parts.add_extra(SNAPCOMPACT_FRAME_TOKEN_ESTIMATE)
                    }
                    _ => {}
                }
            }
        }
    }
}

fn message_parts(message: &Message, options: MessageCountOptions) -> MessageParts<'_> {
    let mut parts = MessageParts { fragments: Vec::new(), extra: Ok(0) };
    match message {
        Message::User(user) => user_parts(&user.content, &mut parts),
        // Developer is an ARA wire role absent from OMP's AgentMessage union.
        Message::Developer(developer) => user_parts(&developer.content, &mut parts),
        Message::Assistant(assistant) => {
            for block in &assistant.content {
                match block {
                    AssistantBlock::Text(text) => parts.fragments.push(Cow::Borrowed(&text.text)),
                    AssistantBlock::Thinking(thinking) => {
                        parts.fragments.push(Cow::Borrowed(&thinking.thinking));
                        if !options.exclude_encrypted_reasoning
                            && let Some(signature) =
                                thinking.thinking_signature.as_deref().filter(|signature| !signature.is_empty())
                        {
                            parts.fragments.push(Cow::Borrowed(signature));
                        }
                    }
                    AssistantBlock::RedactedThinking { data } if !options.exclude_encrypted_reasoning => {
                        parts.fragments.push(Cow::Borrowed(data));
                    }
                    AssistantBlock::RedactedThinking { .. } => {}
                    AssistantBlock::ToolCall(call) => {
                        parts.fragments.push(Cow::Borrowed(&call.name));
                        parts.fragments.push(Cow::Owned(
                            serde_json::to_string(&call.arguments).unwrap_or_else(|_| "null".to_owned()),
                        ));
                    }
                    // ARA's assistant image extension uses OMP's result estimate.
                    AssistantBlock::Image(_) => parts.add_extra(IMAGE_TOKEN_ESTIMATE),
                }
            }
        }
        Message::ToolResult(result) => {
            for block in &result.content {
                match block {
                    UserBlock::Text(text) if !text.text.is_empty() => parts.fragments.push(Cow::Borrowed(&text.text)),
                    UserBlock::Image(_) => parts.add_extra(IMAGE_TOKEN_ESTIMATE),
                    _ => {}
                }
            }
        }
    }
    parts
}

/// Compatibility heuristic for callers without selected-model metadata.
/// Actual Hosts use `Tokenizer`; this helper has no environment-dependent policy.
pub fn count_message(message: &Message, options: MessageCountOptions) -> usize {
    let parts = message_parts(message, options);
    count_fragments(parts.fragments.iter().map(|fragment| fragment.as_ref()), EstimateMode::Approximate)
        .saturating_add(parts.extra.unwrap_or(usize::MAX))
}

pub fn count_messages(messages: &[Message], options: MessageCountOptions) -> usize {
    messages.iter().fold(0usize, |sum, message| sum.saturating_add(count_message(message, options)))
}

#[cfg(test)]
mod tests {
    use super::{TokenCountError, checked_total};

    #[test]
    fn checked_addition_reports_overflow_instead_of_a_small_estimate() {
        assert_eq!(checked_total([usize::MAX, 1]), Err(TokenCountError::CountOverflow));
        assert_eq!(checked_total([usize::MAX]), Ok(usize::MAX));
    }
}
