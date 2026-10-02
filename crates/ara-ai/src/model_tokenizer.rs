//! Native content encodings selected by fixed OMP catalog metadata.
//! Catalog identity resolution is Host-owned; the Core counts the family
//! materialized on the model without guessing from a provider or API.

use crate::Model;
use ara_ctok::Encoding;

/// The eight catalog families, distinct from native OpenAI fallback encodings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelTokenizer {
    ClaudeV3,
    ClaudeV47,
    ClaudeV5,
    ClaudeV5Sonnet,
    Qwen3,
    DeepSeekV3,
    KimiK2,
    Glm5,
}

impl ModelTokenizer {
    pub fn from_name(value: &str) -> Option<Self> {
        match value {
            "claude-v3" => Some(Self::ClaudeV3),
            "claude-v47" => Some(Self::ClaudeV47),
            "claude-v5" => Some(Self::ClaudeV5),
            "claude-v5-sonnet" => Some(Self::ClaudeV5Sonnet),
            "qwen3" => Some(Self::Qwen3),
            "deepseek-v3" => Some(Self::DeepSeekV3),
            "kimi-k2" => Some(Self::KimiK2),
            "glm5" => Some(Self::Glm5),
            _ => None,
        }
    }

    pub fn encoding(self) -> Encoding {
        match self {
            Self::ClaudeV3 => Encoding::ClaudeV3,
            Self::ClaudeV47 => Encoding::ClaudeV47,
            Self::ClaudeV5 => Encoding::ClaudeV5,
            Self::ClaudeV5Sonnet => Encoding::ClaudeV5Sonnet,
            Self::Qwen3 => Encoding::Qwen3,
            Self::DeepSeekV3 => Encoding::DeepSeekV3,
            Self::KimiK2 => Encoding::KimiK2,
            Self::Glm5 => Encoding::Glm5,
        }
    }
}

/// Exact counts of selected-family content fragments only. Provider framing,
/// tools, images and routing transformations are outside this result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelContentCount {
    Exact(u64),
    UnknownTokenizer,
    CountOverflow,
}

pub fn count_model_fragments<'a>(model: &Model, fragments: impl IntoIterator<Item = &'a str>) -> ModelContentCount {
    match model.tokenizer {
        Some(family) => count_family_fragments(family, fragments),
        None => ModelContentCount::UnknownTokenizer,
    }
}

pub fn count_family_fragments<'a>(
    family: ModelTokenizer,
    fragments: impl IntoIterator<Item = &'a str>,
) -> ModelContentCount {
    count_encoding_fragments(family.encoding(), fragments)
}

/// OMP's native default for strict or explicitly accurate unknown-model counts.
/// A missing catalog family remains unknown in `count_model_fragments`.
pub fn count_default_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>) -> ModelContentCount {
    count_encoding_fragments(Encoding::O200kBase, fragments)
}

fn count_encoding_fragments<'a>(encoding: Encoding, fragments: impl IntoIterator<Item = &'a str>) -> ModelContentCount {
    match ara_ctok::count_encoding_fragments(fragments, encoding) {
        Some(count) => ModelContentCount::Exact(count),
        None => ModelContentCount::CountOverflow,
    }
}

/// Resolve the canonical Claude ids covered by the pinned reconstruction.
/// Unrecognized names and future generations require an explicit host choice.
/// This is a deliberately narrow subset of OMP's catalog taxonomy, not a
/// provider-name or API-protocol heuristic.
pub fn resolve_known_claude_tokenizer(model_id: &str) -> Option<ModelTokenizer> {
    let bare = model_id.rsplit('/').next()?.to_ascii_lowercase();
    let name = bare.strip_prefix("claude-")?;
    let mut parts = name.split(['-', '.']);
    let first = parts.next()?;
    if first == "3" {
        // Older names put the generation before the family, for example
        // claude-3-7-sonnet. Only generation 3 is unambiguous here.
        return parts.any(|part| matches!(part, "opus" | "sonnet" | "haiku")).then_some(ModelTokenizer::ClaudeV3);
    }
    let family = first;
    if !matches!(family, "opus" | "sonnet" | "fable" | "mythos" | "haiku") {
        return None;
    }
    let major: u8 = parts.next()?.parse().ok()?;
    let minor = match parts.next() {
        None => 0,
        // A whole-number major may be followed by a date, not a minor.
        Some(part) if part.len() >= 6 && part.bytes().all(|byte| byte.is_ascii_digit()) => 0,
        Some(part) => part.parse::<u8>().ok()?,
    };
    match (family, major, minor) {
        ("opus", 3, _) | ("opus", 4, 0..=6) => Some(ModelTokenizer::ClaudeV3),
        ("opus", 4, 7..=9) => Some(ModelTokenizer::ClaudeV47),
        ("opus", 5, _) => Some(ModelTokenizer::ClaudeV5),
        ("sonnet" | "fable" | "mythos", 3..=4, _) => Some(ModelTokenizer::ClaudeV3),
        ("sonnet" | "fable" | "mythos", 5, _) => Some(ModelTokenizer::ClaudeV5Sonnet),
        ("haiku", 3..=4, _) => Some(ModelTokenizer::ClaudeV3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ModelTokenizer as T, resolve_known_claude_tokenizer as resolve};

    #[test]
    fn pinned_claude_family_boundaries() {
        assert_eq!(resolve("anthropic/claude-opus-4-6"), Some(T::ClaudeV3));
        assert_eq!(resolve("anthropic/claude-opus-4-7"), Some(T::ClaudeV47));
        assert_eq!(resolve("claude-opus-4.8"), Some(T::ClaudeV47));
        assert_eq!(resolve("claude-opus-4-20250514"), Some(T::ClaudeV3));
        assert_eq!(resolve("claude-opus-5"), Some(T::ClaudeV5));
        assert_eq!(resolve("claude-sonnet-5"), Some(T::ClaudeV5Sonnet));
        assert_eq!(resolve("claude-fable-5.1"), Some(T::ClaudeV5Sonnet));
        assert_eq!(resolve("claude-3-7-sonnet-20250219"), Some(T::ClaudeV3));
    }

    #[test]
    fn uncertain_names_are_not_silently_assigned() {
        for name in [
            "alias",
            "claude-opus",
            "claude-opus-4-10",
            "claude-opus-6",
            "claude-haiku-5",
            "claude-opus-4-preview",
            "deepseek-r1-distill-claude-5",
            "claude-other-5",
        ] {
            assert_eq!(resolve(name), None, "{name}");
        }
    }
}
