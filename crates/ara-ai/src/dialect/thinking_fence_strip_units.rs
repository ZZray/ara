//! Fixed OMP 596f2da, packages/ai/src/dialect/thinking-fence-strip.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Structured block kernel only; actual Google consumer remains a Root gate.

use super::fenced_thinking_units::backtick_lead;
use super::units::{lowercase_ascii_prefix, scalar_step, trim_space_tab_end};
use ara_rpc::WireString;

fn without_trailing_cr(units: &[u16]) -> &[u16] {
    if units.last() == Some(&0x0d) { &units[..units.len() - 1] } else { units }
}
fn equals_ascii_ignore_case(units: &[u16], word: &str) -> bool {
    units.len() == word.len()
        && units.iter().zip(word.bytes()).all(|(&unit, byte)| {
            let lower = if (0x41..=0x5a).contains(&unit) { unit | 0x20 } else { unit };
            lower == u16::from(byte)
        })
}
fn opener_line(line: &[u16]) -> bool {
    // Native non-unicode /i with one optional CR and explicit space/tab tail.
    let (ticks, rest) = backtick_lead(without_trailing_cr(line));
    if ticks < 3 {
        return false;
    }
    let word = trim_space_tab_end(rest);
    equals_ascii_ignore_case(word, "thinking") || equals_ascii_ignore_case(word, "reasoning")
}
fn could_be_opener_prefix(line: &[u16]) -> bool {
    let (ticks, rest) = backtick_lead(without_trailing_cr(line));
    if rest.is_empty() {
        return true;
    }
    if ticks < 3 {
        return false;
    }
    let word = trim_space_tab_end(rest);
    lowercase_ascii_prefix(word, "thinking") || lowercase_ascii_prefix(word, "reasoning")
}
#[derive(Default)]
pub struct ThinkingFenceUnitsStripper {
    carry: Vec<u16>,
    passthrough: bool,
}
impl ThinkingFenceUnitsStripper {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push_units(&mut self, chunk: &WireString) -> WireString {
        let mut out = Vec::new();
        let units = chunk.units();
        let mut index = 0;
        // Native for-of works per input chunk: a pair is one step, a lone unit
        // is one step. Delimiters are ASCII and original units are retained.
        while index < units.len() {
            let step = scalar_step(units, index);
            let character = &units[index..index + step];
            let newline = character == [0x0a];
            index += step;
            if self.passthrough {
                out.extend_from_slice(character);
                if newline {
                    self.passthrough = false;
                }
                continue;
            }
            if newline {
                if !opener_line(&self.carry) {
                    out.extend_from_slice(&self.carry);
                    out.push(0x0a);
                }
                self.carry.clear();
                continue;
            }
            self.carry.extend_from_slice(character);
            if !could_be_opener_prefix(&self.carry) {
                out.extend_from_slice(&self.carry);
                self.carry.clear();
                self.passthrough = true;
            }
        }
        WireString::from_units(out)
    }
    pub fn flush_units(&mut self) -> WireString {
        let carry = std::mem::take(&mut self.carry);
        self.passthrough = false;
        WireString::from_units(if opener_line(&carry) { Vec::new() } else { carry })
    }
}
