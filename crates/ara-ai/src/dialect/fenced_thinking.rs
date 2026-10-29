//! Fixed OMP 596f2da, packages/ai/src/dialect/fenced-thinking.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Restricted scalar-string facade over the one UTF-16 kernel.

use super::fenced_thinking_units::FencedThinkingUnitsScanner;
use super::units::checked_string;
use ara_rpc::WireString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedThinkingResult {
    pub thinking: String,
    pub closed: bool,
    pub rest: String,
}

#[derive(Default)]
pub struct FencedThinkingScanner {
    inner: FencedThinkingUnitsScanner,
}

impl FencedThinkingScanner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn feed(&mut self, text: &str, final_input: bool) -> FencedThinkingResult {
        let result = self.inner.feed_units(&WireString::from(text), final_input);
        FencedThinkingResult {
            thinking: checked_string(result.thinking),
            closed: result.closed,
            rest: checked_string(result.rest),
        }
    }
}
