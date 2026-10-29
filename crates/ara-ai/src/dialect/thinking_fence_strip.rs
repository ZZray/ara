//! Fixed OMP 596f2da, packages/ai/src/dialect/thinking-fence-strip.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Restricted scalar-string facade; actual Google consumer is a separate gate.

use super::thinking_fence_strip_units::ThinkingFenceUnitsStripper;
use super::units::checked_string;
use ara_rpc::WireString;

#[derive(Default)]
pub struct ThinkingFenceStripper {
    inner: ThinkingFenceUnitsStripper,
}
impl ThinkingFenceStripper {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn push(&mut self, chunk: &str) -> String {
        checked_string(self.inner.push_units(&WireString::from(chunk)))
    }
    pub fn flush(&mut self) -> String {
        checked_string(self.inner.flush_units())
    }
}
