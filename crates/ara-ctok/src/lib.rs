//! Universal offline tokenization from OMP `crates/pi-natives/src/utok`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Vocabulary data is embedded; counting requires no network or tokenizer
//! runtime. [`Encoding`] counts ordinary content for every pinned family and
//! encodes token IDs for the BPE families. Claude provides reconstructed counts
//! only. Counts exclude special tokens, chat templates and provider wire frames.
//!
//! The port retains the pinned OMP MIT license (`LICENSE.omp`), the separate
//! `data/LICENSE.ctok` notice and the Unicode data notice (`data/LICENSE-UNICODE`).

mod utok;
mod xutf_compat;

pub use utok::claude::Family as ClaudeFamily;
pub use utok::{Cursor, Encoding, RankTable, Unit, Utf};

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

/// Sum ordinary content counts with each fragment tokenized separately using
/// an explicitly selected encoding. Returns `None` if the aggregate exceeds
/// `u64`; no approximate fallback is substituted for an exact count.
pub fn count_encoding_fragments<'a>(fragments: impl IntoIterator<Item = &'a str>, encoding: Encoding) -> Option<u64> {
    fragments.into_iter().try_fold(0u64, |sum, text| sum.checked_add(u64::from(encoding.count(text))))
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
