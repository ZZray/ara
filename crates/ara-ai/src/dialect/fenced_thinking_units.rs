//! Fixed OMP 596f2da, packages/ai/src/dialect/fenced-thinking.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::units::{trim, wire};
use ara_rpc::WireString;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FencedThinkingUnitsResult {
    pub thinking: WireString,
    pub closed: bool,
    pub rest: WireString,
}

#[derive(Default)]
pub struct FencedThinkingUnitsScanner {
    buffer: Vec<u16>,
    inner: Vec<u16>,
    emitted: usize,
}

impl FencedThinkingUnitsScanner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn feed_units(&mut self, text: &WireString, final_input: bool) -> FencedThinkingUnitsResult {
        self.buffer.extend_from_slice(text.units());
        let mut thinking = Vec::new();
        while let Some(newline) = self.buffer.iter().position(|&unit| unit == 0x0a) {
            let line = &self.buffer[..newline];
            if self.inner.is_empty()
                && let Some(close) = close_rest(line)
            {
                let mut rest = close.to_vec();
                rest.extend_from_slice(&self.buffer[newline..]);
                self.reset();
                return FencedThinkingUnitsResult {
                    thinking: WireString::from_units(thinking),
                    closed: true,
                    rest: WireString::from_units(rest),
                };
            }
            thinking.extend_from_slice(&self.buffer[self.emitted..newline + 1]);
            let line = line.to_vec();
            self.update_inner(&line);
            self.buffer.drain(..newline + 1);
            self.emitted = 0;
        }
        if !self.inner.is_empty() {
            thinking.extend_from_slice(&self.buffer[self.emitted..]);
            self.emitted = self.buffer.len();
            // Native retains nested state even on final; the owner discards it.
            return FencedThinkingUnitsResult {
                thinking: WireString::from_units(thinking),
                closed: false,
                rest: wire(&[]),
            };
        }
        if final_input {
            if let Some(close) = close_rest_final(&self.buffer) {
                let rest = wire(close);
                self.reset();
                return FencedThinkingUnitsResult { thinking: WireString::from_units(thinking), closed: true, rest };
            }
        } else {
            if let Some(close) = close_rest_streaming_tail(&self.buffer) {
                let rest = wire(close);
                self.reset();
                return FencedThinkingUnitsResult { thinking: WireString::from_units(thinking), closed: true, rest };
            }
            if must_hold(&self.buffer) {
                return FencedThinkingUnitsResult {
                    thinking: WireString::from_units(thinking),
                    closed: false,
                    rest: wire(&[]),
                };
            }
        }
        thinking.extend_from_slice(&self.buffer[self.emitted..]);
        if final_input {
            self.reset();
        } else {
            self.emitted = self.buffer.len();
        }
        FencedThinkingUnitsResult { thinking: WireString::from_units(thinking), closed: false, rest: wire(&[]) }
    }
    fn reset(&mut self) {
        self.buffer.clear();
        self.inner.clear();
        self.emitted = 0;
    }
    fn update_inner(&mut self, line: &[u16]) {
        let Some((run, info)) = fence_line(line) else {
            return;
        };
        if self.inner.is_empty() {
            self.inner = run.to_vec();
        } else if run[0] == self.inner[0] && run.len() >= self.inner.len() && trim(info).is_empty() {
            self.inner.clear();
        }
    }
}

/// Native /^ {0,3}(`*)([\s\S]*)$/ greedily consumes up to three spaces.
pub(super) fn backtick_lead(text: &[u16]) -> (usize, &[u16]) {
    let leading = text.iter().take(3).take_while(|&&unit| unit == 0x20).count();
    let ticks = text[leading..].iter().take_while(|&&unit| unit == 0x60).count();
    (ticks, &text[leading + ticks..])
}
fn language_token(text: &[u16]) -> bool {
    !text.is_empty()
        && text.iter().all(|&unit| {
            (0x41..=0x5a).contains(&unit)
                || (0x61..=0x7a).contains(&unit)
                || (0x30..=0x39).contains(&unit)
                || matches!(unit, 0x5f | 0x2b | 0x23 | 0x2d)
        })
}
fn close_rest(line: &[u16]) -> Option<&[u16]> {
    let (ticks, rest) = backtick_lead(line);
    if ticks < 3 {
        None
    } else if trim(rest).is_empty() {
        Some(&[])
    } else if language_token(rest) {
        None
    } else {
        Some(rest)
    }
}
fn close_rest_final(tail: &[u16]) -> Option<&[u16]> {
    let (ticks, rest) = backtick_lead(tail);
    (ticks >= 3).then(|| if trim(rest).is_empty() { &[][..] } else { rest })
}
fn close_rest_streaming_tail(tail: &[u16]) -> Option<&[u16]> {
    let (ticks, rest) = backtick_lead(tail);
    if ticks < 3 || trim(rest).is_empty() || language_token(rest) { None } else { Some(rest) }
}
fn must_hold(tail: &[u16]) -> bool {
    let (ticks, rest) = backtick_lead(tail);
    if trim(rest).is_empty() {
        return ticks >= 1 || (tail.len() <= 3 && tail.iter().all(|&unit| unit == 0x20));
    }
    ticks >= 3 && language_token(rest)
}
fn fence_line(line: &[u16]) -> Option<(&[u16], &[u16])> {
    let leading = line.iter().take(3).take_while(|&&unit| unit == 0x20).count();
    let marker = *line.get(leading)?;
    if !matches!(marker, 0x60 | 0x7e) {
        return None;
    }
    let length = line[leading..].iter().take_while(|&&unit| unit == marker).count();
    if length < 3 {
        return None;
    }
    let info = &line[leading + length..];
    // JS (.*) is non-dotall and non-multiline in the fixed regex.
    if info.iter().any(|&unit| matches!(unit, 0x0d | 0x0a | 0x2028 | 0x2029)) {
        return None;
    }
    Some((&line[leading..leading + length], info))
}
