//! Fixed OMP 596f2da, packages/ai/src/dialect/thinking.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::fenced_thinking_units::FencedThinkingUnitsScanner;
use super::thinking::ThinkingInbandScannerOptions;
use super::types::{InbandScanEvent, InbandScannerUnits};
use super::units::{find_ascii, is_ascii_prefix, partial_suffix_overlap, starts_ascii, trim, wire};
use ara_rpc::WireString;
use std::marker::PhantomData;

#[derive(Clone, Copy)]
struct Tag {
    open: &'static str,
    close: &'static str,
    fenced: bool,
    implied_open: bool,
}
const TAGS: &[Tag] = &[
    Tag { open: "<think>", close: "</think>", fenced: false, implied_open: true },
    Tag { open: "<thinking>", close: "</thinking>", fenced: false, implied_open: false },
    Tag { open: "<scratchpad>", close: "</scratchpad>", fenced: false, implied_open: false },
    Tag { open: "```thinking\n", close: "```", fenced: true, implied_open: false },
    Tag { open: "<|channel>thought\n", close: "<channel|>", fenced: false, implied_open: false },
    Tag {
        open: "<|start|>assistant<|channel|>analysis<|message|>",
        close: "<|end|>",
        fenced: false,
        implied_open: false,
    },
    Tag { open: "<|channel|>analysis<|message|>", close: "<|end|>", fenced: false, implied_open: false },
];
pub struct ThinkingInbandUnitsScanner<V> {
    implied_open: bool,
    buffer: Vec<u16>,
    close_tag: &'static str,
    thinking: Vec<u16>,
    fenced: Option<FencedThinkingUnitsScanner>,
    code_ticks: usize,
    code_fenced: bool,
    line_indent: isize,
    arguments: PhantomData<fn() -> V>,
}
impl<V> ThinkingInbandUnitsScanner<V> {
    pub fn new(options: ThinkingInbandScannerOptions) -> Self {
        Self {
            implied_open: options.implied_open,
            buffer: Vec::new(),
            close_tag: "",
            thinking: Vec::new(),
            fenced: None,
            code_ticks: 0,
            code_fenced: false,
            line_indent: 0,
            arguments: PhantomData,
        }
    }
    fn consume(&mut self, final_input: bool) -> Vec<InbandScanEvent<V, WireString>> {
        let mut events = Vec::new();
        loop {
            if let Some(fenced) = self.fenced.as_mut() {
                // Native final checks the held fence before the empty guard.
                let result = fenced.feed_units(&wire(&self.buffer), final_input);
                self.buffer = if result.closed { result.rest.units().to_vec() } else { Vec::new() };
                self.emit_thinking(result.thinking.units(), &mut events);
                if result.closed || final_input {
                    events.push(InbandScanEvent::ThinkingEnd {
                        thinking: WireString::from_units(std::mem::take(&mut self.thinking)),
                    });
                    self.close_tag = "";
                    self.fenced = None;
                }
                if self.fenced.is_some() {
                    break;
                }
                continue;
            }
            if self.buffer.is_empty() {
                break;
            }
            if !self.close_tag.is_empty() {
                let Some(close) = find_ascii(&self.buffer, self.close_tag) else {
                    let hold = if final_input { 0 } else { partial_suffix_overlap(&self.buffer, self.close_tag) };
                    let emit_end = self.buffer.len() - hold;
                    let chunk = self.buffer[..emit_end].to_vec();
                    self.emit_thinking(&chunk, &mut events);
                    self.buffer.drain(..emit_end);
                    break;
                };
                let chunk = self.buffer[..close].to_vec();
                self.emit_thinking(&chunk, &mut events);
                self.buffer.drain(..close + self.close_tag.len());
                events.push(InbandScanEvent::ThinkingEnd {
                    thinking: WireString::from_units(std::mem::take(&mut self.thinking)),
                });
                self.close_tag = "";
                continue;
            }
            if self.code_ticks > 0 {
                if self.emit_code(final_input, &mut events) {
                    continue;
                }
                break;
            }
            let hit = scan_visible(&self.buffer, final_input, self.implied_open);
            let Some(index) = hit.index() else {
                let chunk = std::mem::take(&mut self.buffer);
                self.emit_text(&chunk, &mut events);
                break;
            };
            if index > 0 {
                let chunk = self.buffer[..index].to_vec();
                self.emit_text(&chunk, &mut events);
            }
            match hit {
                VisibleHit::Hold { .. } => {
                    self.buffer.drain(..index);
                    break;
                }
                VisibleHit::ImpliedClose { tag, .. } => {
                    self.buffer.drain(..index + tag.close.len());
                    events.push(InbandScanEvent::ImpliedThinkingEnd);
                }
                VisibleHit::Code { ticks, .. } => {
                    let fenced = ticks >= 3 && (0..=3).contains(&self.line_indent);
                    let chunk = self.buffer[index..index + ticks].to_vec();
                    self.emit_text(&chunk, &mut events);
                    self.buffer.drain(..index + ticks);
                    self.code_ticks = ticks;
                    self.code_fenced = fenced;
                }
                VisibleHit::Tag { tag, .. } => {
                    self.buffer.drain(..index + tag.open.len());
                    self.close_tag = tag.close;
                    self.thinking.clear();
                    if tag.fenced {
                        self.fenced = Some(FencedThinkingUnitsScanner::new());
                    }
                    events.push(InbandScanEvent::ThinkingStart);
                }
                VisibleHit::None => unreachable!("none returned no index"),
            }
        }
        events
    }
    fn emit_code(&mut self, final_input: bool, events: &mut Vec<InbandScanEvent<V, WireString>>) -> bool {
        if self.code_fenced {
            if let Some(end) = find_fence_close_end(&self.buffer, self.code_ticks, final_input) {
                let chunk = self.buffer[..end].to_vec();
                self.emit_text(&chunk, events);
                self.buffer.drain(..end);
                self.code_ticks = 0;
                self.code_fenced = false;
                return true;
            }
            if final_input {
                let chunk = std::mem::take(&mut self.buffer);
                self.emit_text(&chunk, events);
                self.code_ticks = 0;
                self.code_fenced = false;
                return false;
            }
            if let Some(newline) = self.buffer.iter().rposition(|&unit| unit == 0x0a) {
                let chunk = self.buffer[..newline + 1].to_vec();
                self.emit_text(&chunk, events);
                self.buffer.drain(..newline + 1);
            }
            return false;
        }
        if let Some(close) = find_backtick_run(&self.buffer, 0, self.code_ticks)
            && (final_input || close + self.code_ticks < self.buffer.len())
        {
            let chunk = self.buffer[..close + self.code_ticks].to_vec();
            self.emit_text(&chunk, events);
            self.buffer.drain(..close + self.code_ticks);
            self.code_ticks = 0;
            return true;
        }
        let hold = if final_input { 0 } else { trailing_backtick_run(&self.buffer) };
        let emit_end = self.buffer.len() - hold;
        let chunk = self.buffer[..emit_end].to_vec();
        self.emit_text(&chunk, events);
        self.buffer.drain(..emit_end);
        if final_input {
            self.code_ticks = 0;
        }
        false
    }
    fn emit_text(&mut self, text: &[u16], events: &mut Vec<InbandScanEvent<V, WireString>>) {
        if text.is_empty() {
            return;
        }
        events.push(InbandScanEvent::Text { text: wire(text) });
        self.line_indent = trailing_line_indent(text, self.line_indent);
    }
    fn emit_thinking(&mut self, delta: &[u16], events: &mut Vec<InbandScanEvent<V, WireString>>) {
        if delta.is_empty() {
            return;
        }
        self.thinking.extend_from_slice(delta);
        events.push(InbandScanEvent::ThinkingDelta { delta: wire(delta) });
    }
}
impl<V> InbandScannerUnits for ThinkingInbandUnitsScanner<V> {
    type Arguments = V;
    fn feed_units(&mut self, text: &WireString) -> Vec<InbandScanEvent<V, WireString>> {
        if text.is_empty() {
            return Vec::new();
        }
        self.buffer.extend_from_slice(text.units());
        self.consume(false)
    }
    fn flush_units(&mut self) -> Vec<InbandScanEvent<V, WireString>> {
        let mut events = self.consume(true);
        if self.buffer.is_empty() {
            return events;
        }
        let tail = std::mem::take(&mut self.buffer);
        if !self.close_tag.is_empty() {
            self.emit_thinking(&tail, &mut events);
            events.push(InbandScanEvent::ThinkingEnd { thinking: wire(&self.thinking) });
        } else {
            events.push(InbandScanEvent::Text { text: WireString::from_units(tail) });
        }
        self.close_tag = "";
        events
    }
}

enum VisibleHit {
    Tag { index: usize, tag: Tag },
    ImpliedClose { index: usize, tag: Tag },
    Code { index: usize, ticks: usize },
    Hold { index: usize },
    None,
}
impl VisibleHit {
    fn index(&self) -> Option<usize> {
        match self {
            Self::Tag { index, .. }
            | Self::ImpliedClose { index, .. }
            | Self::Code { index, .. }
            | Self::Hold { index } => Some(*index),
            Self::None => None,
        }
    }
}
fn scan_visible(buffer: &[u16], final_input: bool, implied_open: bool) -> VisibleHit {
    for (index, &unit) in buffer.iter().enumerate() {
        let rest = &buffer[index..];
        if let Some(&tag) = TAGS.iter().find(|tag| starts_ascii(rest, tag.open)) {
            return VisibleHit::Tag { index, tag };
        }
        if implied_open && let Some(&tag) = TAGS.iter().find(|tag| tag.implied_open && starts_ascii(rest, tag.close)) {
            return VisibleHit::ImpliedClose { index, tag };
        }
        if !final_input
            && TAGS.iter().any(|tag| {
                tag.open.len() > rest.len() && is_ascii_prefix(rest, tag.open)
                    || implied_open
                        && tag.implied_open
                        && tag.close.len() > rest.len()
                        && is_ascii_prefix(rest, tag.close)
            })
        {
            return VisibleHit::Hold { index };
        }
        if unit == 0x60 {
            let ticks = backtick_run(buffer, index);
            if !final_input && index + ticks == buffer.len() {
                return VisibleHit::Hold { index };
            }
            return VisibleHit::Code { index, ticks };
        }
    }
    VisibleHit::None
}
fn backtick_run(buffer: &[u16], from: usize) -> usize {
    buffer[from..].iter().take_while(|&&unit| unit == 0x60).count()
}
fn find_backtick_run(buffer: &[u16], mut from: usize, ticks: usize) -> Option<usize> {
    while let Some(index) = buffer[from..].iter().position(|&unit| unit == 0x60).map(|index| from + index) {
        let run = backtick_run(buffer, index);
        if run == ticks {
            return Some(index);
        }
        from = index + run;
    }
    None
}
fn trailing_backtick_run(buffer: &[u16]) -> usize {
    buffer.iter().rev().take_while(|&&unit| unit == 0x60).count()
}
fn trailing_line_indent(text: &[u16], prior: isize) -> isize {
    let (mut indent, tail) =
        text.iter().rposition(|&unit| unit == 0x0a).map_or((prior, text), |newline| (0, &text[newline + 1..]));
    for &unit in tail {
        if indent == -1 {
            break;
        }
        indent = if unit == 0x20 { indent + 1 } else { -1 };
    }
    indent
}
fn find_fence_close_end(buffer: &[u16], ticks: usize, final_input: bool) -> Option<usize> {
    let mut start = 0;
    while start <= buffer.len() {
        let newline = buffer[start..].iter().position(|&unit| unit == 0x0a).map(|index| start + index);
        let end = newline.unwrap_or(buffer.len());
        let line = trim(&buffer[start..end]);
        if line.len() >= ticks
            && !line.is_empty()
            && line.iter().all(|&unit| unit == 0x60)
            && (newline.is_some() || final_input)
        {
            return Some(newline.map_or(buffer.len(), |newline| newline + 1));
        }
        let Some(newline) = newline else {
            break;
        };
        start = newline + 1;
    }
    None
}
