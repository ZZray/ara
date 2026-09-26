//! Host-selected tokenizer metadata. The fixed OMP catalog resolves a family
//! before Agent counting; ARA recognizes only canonical Claude ids here until
//! its broader catalog identity policy is ported.

/// Embedded Claude content-token reconstruction selected for a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelTokenizer {
    ClaudeV3,
    ClaudeV47,
    ClaudeV5,
    ClaudeV5Sonnet,
}

impl ModelTokenizer {
    pub fn from_name(value: &str) -> Option<Self> {
        match value {
            "claude-v3" => Some(Self::ClaudeV3),
            "claude-v47" => Some(Self::ClaudeV47),
            "claude-v5" => Some(Self::ClaudeV5),
            "claude-v5-sonnet" => Some(Self::ClaudeV5Sonnet),
            _ => None,
        }
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
