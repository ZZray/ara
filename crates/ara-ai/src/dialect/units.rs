//! Fixed OMP 596f2da, packages/ai/src/dialect/{coercion,thinking,harmony}.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! UTF-16 helpers for the fixed Native scanners; no lossy string conversion.

use super::types::InbandScanEvent;
use ara_rpc::WireString;

pub(super) fn wire(units: &[u16]) -> WireString {
    WireString::from_units(units.to_vec())
}
/// Generic token helpers coexist with the unchanged ASCII helpers below.
pub(super) fn find_units(units: &[u16], token: &[u16]) -> Option<usize> {
    if token.is_empty() {
        return Some(0);
    }
    units.windows(token.len()).position(|window| window == token)
}
pub(super) fn suffix_units_overlap(units: &[u16], token: &[u16]) -> usize {
    (1..=units.len().min(token.len().saturating_sub(1)))
        .rev()
        .find(|&len| units[units.len() - len..] == token[..len])
        .unwrap_or(0)
}
pub(super) fn ascii(text: &str) -> Vec<u16> {
    assert!(text.is_ascii(), "fixed control token is ASCII");
    text.bytes().map(u16::from).collect()
}
pub(super) fn starts_ascii(units: &[u16], token: &str) -> bool {
    token.is_ascii()
        && units.len() >= token.len()
        && units.iter().zip(token.bytes()).all(|(&unit, byte)| unit == u16::from(byte))
}
pub(super) fn equals_ascii(units: &[u16], token: &str) -> bool {
    units.len() == token.len() && starts_ascii(units, token)
}
pub(super) fn is_ascii_prefix(units: &[u16], token: &str) -> bool {
    token.is_ascii()
        && units.len() <= token.len()
        && units.iter().zip(token.bytes()).all(|(&unit, byte)| unit == u16::from(byte))
}
pub(super) fn find_ascii(units: &[u16], token: &str) -> Option<usize> {
    if token.is_empty() {
        return Some(0);
    }
    (0..=units.len().saturating_sub(token.len())).find(|&index| starts_ascii(&units[index..], token))
}
pub(super) fn partial_suffix_overlap(units: &[u16], token: &str) -> usize {
    (1..=units.len().min(token.len().saturating_sub(1)))
        .rev()
        .find(|&length| is_ascii_prefix(&units[units.len() - length..], token))
        .unwrap_or(0)
}
pub(super) fn partial_suffix_overlap_any(units: &[u16], tokens: &[&str]) -> usize {
    tokens.iter().map(|token| partial_suffix_overlap(units, token)).max().unwrap_or(0)
}
pub(super) fn is_space(unit: u16) -> bool {
    // All Native whitespace codepoints are BMP. A surrogate has no scalar
    // predicate and is never replaced or classified as whitespace.
    char::from_u32(u32::from(unit)).is_some_and(ara_prompt::js::is_space)
}
pub(super) fn trim_start(units: &[u16]) -> &[u16] {
    let first = units.iter().position(|&unit| !is_space(unit)).unwrap_or(units.len());
    &units[first..]
}
pub(super) fn trim(units: &[u16]) -> &[u16] {
    let units = trim_start(units);
    let length = units.iter().rposition(|&unit| !is_space(unit)).map_or(0, |index| index + 1);
    &units[..length]
}
pub(super) fn trim_space_tab_end(units: &[u16]) -> &[u16] {
    let length = units.iter().rposition(|&unit| !matches!(unit, 0x20 | 0x09)).map_or(0, |index| index + 1);
    &units[..length]
}
pub(super) fn checked_string_events<V>(events: Vec<InbandScanEvent<V, WireString>>) -> Vec<InbandScanEvent<V>> {
    // Only restricted String facades call this. Their private kernels have
    // received Unicode scalar strings, and cuts are at ASCII delimiters.
    events.into_iter().map(|event| event.map_strings(checked_string)).collect()
}
pub(super) fn checked_string(text: WireString) -> String {
    text.to_utf8().expect("String facade admits only scalar source and ASCII cuts")
}

/// Native stripper's for-of step within one input chunk. Pairs are one scalar;
/// a lone unit is one step. The caller retains the original unit slice.
pub(super) fn scalar_step(units: &[u16], index: usize) -> usize {
    if (0xd800..=0xdbff).contains(&units[index])
        && units.get(index + 1).is_some_and(|next| (0xdc00..=0xdfff).contains(next))
    {
        2
    } else {
        1
    }
}

/// Prefix predicate after Native toLowerCase. Its targets are ASCII, so any
/// non-ASCII lowered unit is enough to reject. Lone units remain non-ASCII.
/// Contextual Greek sigma variants cannot affect an ASCII prefix match.
pub(super) fn lowercase_ascii_prefix(units: &[u16], target: &str) -> bool {
    let mut expected = target.bytes();
    for decoded in char::decode_utf16(units.iter().copied()) {
        let Ok(character) = decoded else {
            return false;
        };
        for lowered in character.to_lowercase() {
            if !lowered.is_ascii() || expected.next() != Some(lowered as u8) {
                return false;
            }
        }
    }
    true
}
