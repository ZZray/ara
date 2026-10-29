//! Fixed OMP 596f2da, packages/ai/src/dialect/thinking.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Restricted scalar-string facade over the one UTF-16 kernel.

use super::thinking_units::ThinkingInbandUnitsScanner;
use super::types::{InbandScanEvent, InbandScanner, InbandScannerUnits};
use super::units::checked_string_events;
use ara_rpc::WireString;

#[derive(Clone, Copy, Default)]
pub struct ThinkingInbandScannerOptions {
    pub implied_open: bool,
}

pub struct ThinkingInbandScanner<V> {
    inner: ThinkingInbandUnitsScanner<V>,
}
impl<V> ThinkingInbandScanner<V> {
    pub fn new(options: ThinkingInbandScannerOptions) -> Self {
        Self { inner: ThinkingInbandUnitsScanner::new(options) }
    }
}
impl<V> InbandScanner for ThinkingInbandScanner<V> {
    type Arguments = V;
    fn feed(&mut self, text: &str) -> Vec<InbandScanEvent<V>> {
        checked_string_events(self.inner.feed_units(&WireString::from(text)))
    }
    fn flush(&mut self) -> Vec<InbandScanEvent<V>> {
        checked_string_events(self.inner.flush_units())
    }
}
