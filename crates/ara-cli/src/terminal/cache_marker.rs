//! OMP596f2da cache-invalidation-marker.ts; MIT, see THIRD_PARTY_NOTICES.md.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
use super::{
    render_utils::{TerminalPresentationSettings, width},
    theme::Theme,
};
use ara_ai::Usage;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheInvalidation {
    pub reprocessed_tokens: u64,
}
pub fn detect(prev: Option<&Usage>, current: &Usage) -> Option<CacheInvalidation> {
    let prev = prev?;
    if prev.cache_read? < 2048 || current.cache_read? > 0 || current.cache_write? == 0 {
        return None;
    }
    let tokens = current.cache_write?.checked_add(current.input?)?;
    (tokens >= 2048).then_some(CacheInvalidation { reprocessed_tokens: tokens })
}
pub fn render(info: &CacheInvalidation, columns: usize, theme: &Theme) -> Vec<String> {
    let tokens = info.reprocessed_tokens;
    let trim = positive_fixed_one;
    let count = if tokens < 1000 {
        tokens.to_string()
    } else if tokens < 10_000 {
        format!("{}K", trim(tokens as f64 / 1000.0))
    } else if tokens < 1_000_000 {
        format!("{}K", (tokens as f64 / 1000.0).round())
    } else if tokens < 10_000_000 {
        format!("{}M", trim(tokens as f64 / 1_000_000.0))
    } else if tokens < 1_000_000_000 {
        format!("{}M", (tokens as f64 / 1_000_000.0).round())
    } else if tokens < 10_000_000_000 {
        format!("{}B", trim(tokens as f64 / 1_000_000_000.0))
    } else {
        format!("{}B", (tokens as f64 / 1_000_000_000.0).round())
    };
    let icon = theme.symbol("icon.cacheMiss");
    let head = if icon.is_empty() { "cache miss".to_owned() } else { format!("{icon} cache miss") };
    let label = if tokens > 0 { format!("{head} {} {count} tokens", theme.symbol("sep.dot").trim()) } else { head };
    let rule = 10usize.min(columns.max(1).saturating_sub(width(&label) + 1));
    let row = if rule == 0 {
        theme.fg("muted", &label)
    } else {
        format!("{} {}", theme.fg("dim", &theme.symbol("tree.horizontal").repeat(rule)), theme.fg("muted", &label))
    };
    vec![String::new(), row, String::new()]
}
/// Fixed trim1: JS toFixed(1) rounds the exact binary64 value, with ties up.
/// All callers here supply a positive quotient in [1, 10). Integer arithmetic
/// avoids introducing another binary64 rounding step via `n * 10`.
fn positive_fixed_one(n: f64) -> String {
    debug_assert!((1.0..10.0).contains(&n));
    let bits = n.to_bits();
    let significand = u128::from((bits & ((1u64 << 52) - 1)) | (1u64 << 52));
    let shift = (1075 - ((bits >> 52) & 0x7ff)) as u32;
    let numerator = significand * 10;
    let integral = (numerator >> shift) + u128::from((numerator & ((1u128 << shift) - 1)) >= (1u128 << (shift - 1)));
    if integral % 10 == 0 { (integral / 10).to_string() } else { format!("{}.{}", integral / 10, integral % 10) }
}
pub(crate) fn observe(
    prev: &mut Option<Usage>,
    current: Usage,
    settings: &TerminalPresentationSettings,
) -> Option<CacheInvalidation> {
    // Unknown prompt fields cannot establish positive footprint or reset it.
    if !current.cache_read.is_some_and(|n| n > 0)
        && !current.cache_write.is_some_and(|n| n > 0)
        && !current.input.is_some_and(|n| n > 0)
    {
        return None;
    }
    let marker = if settings.cache_miss_marker { detect(prev.as_ref(), &current) } else { None };
    *prev = Some(current);
    marker
}
