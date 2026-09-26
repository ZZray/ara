//! Exact Claude content-token counts from OMP `crates/pi-natives/src/utok/claude`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! The ported implementation and vocabulary data retain the pinned OMP MIT
//! license and the separate `data/LICENSE.ctok` notice. Counts are exact for
//! the selected reconstruction family, not a provider's entire wire request.

mod utok;
mod xutf_compat;

pub use utok::claude::Family as ClaudeFamily;

/// Count ordinary message content without the fixed Claude message frame.
pub fn count_content(text: &str, family: ClaudeFamily) -> u32 {
    utok::claude::content_token_count(text.as_bytes(), family)
}

/// Sum Claude content counts with each fragment tokenized separately.
/// Returns `None` if the aggregate exceeds `u64`; no approximate fallback is
/// substituted for a failed exact count.
pub fn count_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>, family: ClaudeFamily) -> Option<u64> {
    fragments.into_iter().try_fold(0u64, |sum, text| sum.checked_add(u64::from(count_content(text, family))))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentBudget {
    Fits { tokens: u64 },
    Exceeds { tokens: u64 },
    CountOverflow,
}

/// Compare only content tokens with a budget for an explicitly selected Claude
/// family. This does not include provider framing, tools or system prompts.
pub fn check_content_budget<'a>(
    fragments: impl IntoIterator<Item = &'a str>,
    family: ClaudeFamily,
    budget: u64,
) -> ContentBudget {
    let Some(tokens) = count_fragments(fragments, family) else {
        return ContentBudget::CountOverflow;
    };
    if tokens <= budget { ContentBudget::Fits { tokens } } else { ContentBudget::Exceeds { tokens } }
}
