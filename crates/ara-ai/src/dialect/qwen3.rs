//! Fixed OMP 596f2da, packages/ai/src/dialect/qwen3.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::coercion::{mint_tool_call_id, parse_named_call};
use super::types::{ActualInbandScanEvent, InbandNameValue, InbandScanEvent};
use super::units::{find_ascii, is_space, partial_suffix_overlap_any, starts_ascii, wire};
use crate::relaxed_json::FinalValueFactory;
use ara_rpc::{WireString, WireValue};
use std::sync::Arc;

const TOOL_OPEN: &str = "<tool_call>";
const TOOL_CLOSE: &str = "</tool_call>";
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
pub const QWEN3_PROMPT: &str = include_str!("qwen3.md");
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Outside,
    Thinking,
    Tool,
}

pub struct Qwen3InbandUnitsScanner<F: FinalValueFactory> {
    factory: Arc<F>,
    buffer: Vec<u16>,
    state: State,
    id: WireString,
    name: WireString,
    started: bool,
    thinking: Vec<u16>,
    parse_thinking: bool,
}
impl<F: FinalValueFactory> Qwen3InbandUnitsScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self::with_parse_thinking(factory, true)
    }
    pub fn with_parse_thinking(factory: Arc<F>, parse_thinking: bool) -> Self {
        Self {
            factory,
            buffer: Vec::new(),
            state: State::Outside,
            id: wire(&[]),
            name: wire(&[]),
            started: false,
            thinking: Vec::new(),
            parse_thinking,
        }
    }
    pub fn try_feed_units(&mut self, text: &WireString) -> Result<Vec<ActualInbandScanEvent<F::Value>>, F::Error> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        self.buffer.extend_from_slice(text.units());
        self.consume(false)
    }
    pub fn try_flush_units(&mut self) -> Result<Vec<ActualInbandScanEvent<F::Value>>, F::Error> {
        self.consume(true)
    }
    fn consume(&mut self, final_input: bool) -> Result<Vec<ActualInbandScanEvent<F::Value>>, F::Error> {
        let mut events = Vec::new();
        while !self.buffer.is_empty() {
            match self.state {
                State::Outside => {
                    self.consume_outside(final_input, &mut events);
                    if self.state == State::Outside {
                        break;
                    }
                }
                State::Thinking => {
                    self.consume_thinking(final_input, &mut events);
                    if self.state == State::Thinking {
                        break;
                    }
                }
                State::Tool => {
                    self.consume_tool(final_input, &mut events)?;
                    if self.state == State::Tool {
                        break;
                    }
                }
            }
        }
        if final_input && self.state == State::Thinking {
            self.end_thinking(&mut events);
        }
        Ok(events)
    }
    fn consume_outside(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        let tool = find_ascii(&self.buffer, TOOL_OPEN);
        let think = if self.parse_thinking { find_ascii(&self.buffer, THINK_OPEN) } else { None };
        let is_think = think.is_some_and(|think| tool.is_none_or(|tool| think < tool));
        let start = if is_think { think } else { tool };
        let Some(start) = start else {
            let tags: &[&str] = if self.parse_thinking { &[TOOL_OPEN, THINK_OPEN] } else { &[TOOL_OPEN] };
            let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, tags) };
            let end = self.buffer.len() - hold;
            if end > 0 {
                events.push(InbandScanEvent::Text { text: wire(&self.buffer[..end]) });
            }
            self.buffer.drain(..end);
            return;
        };
        if start > 0 {
            events.push(InbandScanEvent::Text { text: wire(&self.buffer[..start]) });
        }
        if is_think {
            self.buffer.drain(..start + THINK_OPEN.len());
            self.state = State::Thinking;
            self.thinking.clear();
            events.push(InbandScanEvent::ThinkingStart);
            return;
        }
        self.buffer.drain(..start + TOOL_OPEN.len());
        self.state = State::Tool;
        self.id = mint_tool_call_id().into();
        self.name = wire(&[]);
        self.started = false;
    }
    fn consume_thinking(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if let Some(close) = find_ascii(&self.buffer, THINK_CLOSE) {
            self.emit_thinking(close, events);
            self.buffer.drain(..close + THINK_CLOSE.len());
            self.end_thinking(events);
        } else {
            let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, &[THINK_CLOSE]) };
            let end = self.buffer.len() - hold;
            self.emit_thinking(end, events);
            self.buffer.drain(..end);
            if final_input {
                self.end_thinking(events);
            }
        }
    }
    fn consume_tool(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<(), F::Error> {
        let close = find_ascii(&self.buffer, TOOL_CLOSE);
        let body = wire(&self.buffer[..close.unwrap_or(self.buffer.len())]);
        if !self.started {
            self.try_start(body.units(), events);
        }
        let Some(close) = close else {
            if final_input {
                self.reset();
            }
            return Ok(());
        };
        if let Some((name, arguments)) = parse_named_call(&body, self.factory.as_ref())? {
            if !self.started {
                events.push(InbandScanEvent::ToolStart {
                    id: self.id.clone(),
                    name: InbandNameValue::Actual(name.clone()),
                });
                self.started = true;
            }
            let mut raw = WireString::from(TOOL_OPEN).units().to_vec();
            raw.extend_from_slice(body.units());
            raw.extend(TOOL_CLOSE.encode_utf16());
            events.push(InbandScanEvent::ToolEnd {
                id: self.id.clone(),
                name: InbandNameValue::Actual(name),
                arguments,
                raw_block: Some(wire(&raw)),
            });
        }
        self.buffer.drain(..close + TOOL_CLOSE.len());
        self.reset();
        Ok(())
    }
    fn emit_thinking(&mut self, end: usize, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if end > 0 {
            self.thinking.extend_from_slice(&self.buffer[..end]);
            events.push(InbandScanEvent::ThinkingDelta { delta: wire(&self.buffer[..end]) });
        }
    }
    fn end_thinking(&mut self, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        events.push(InbandScanEvent::ThinkingEnd { thinking: wire(&self.thinking) });
        self.thinking.clear();
        self.state = State::Outside;
    }
    fn try_start(&mut self, body: &[u16], events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        let Some(quoted) = complete_name_prefix(body) else {
            return;
        };
        // Native local try includes strict JSON.parse only; scalar extraction
        // follows it. A quoted prefix has no graph import/repair fallback.
        let Ok(name) = WireValue::parse_units(quoted) else {
            return;
        };
        let WireValue::String(name) = &name else {
            return;
        };
        if name.is_empty() {
            return;
        }
        self.name = name.clone();
        self.started = true;
        events
            .push(InbandScanEvent::ToolStart { id: self.id.clone(), name: InbandNameValue::Source(self.name.clone()) });
    }
    fn reset(&mut self) {
        self.state = State::Outside;
        self.id = wire(&[]);
        self.name = wire(&[]);
        self.started = false;
    }
}

/// Exact anchored /^\s*\{\s*"name"\s*:\s*("(?:\\.|[^"\\])*")/.
/// A backslash-dot excludes the four JS line terminators; the negated class
/// itself allows them. Strict parsing later rejects unescaped controls.
fn complete_name_prefix(body: &[u16]) -> Option<&[u16]> {
    let mut at = 0;
    while body.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    if body.get(at) != Some(&0x7b) {
        return None;
    }
    at += 1;
    while body.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    if !starts_ascii(&body[at..], "\"name\"") {
        return None;
    }
    at += 6;
    while body.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    if body.get(at) != Some(&0x3a) {
        return None;
    }
    at += 1;
    while body.get(at).copied().is_some_and(is_space) {
        at += 1;
    }
    if body.get(at) != Some(&0x22) {
        return None;
    }
    let start = at;
    at += 1;
    while let Some(&unit) = body.get(at) {
        if unit == 0x22 {
            return Some(&body[start..=at]);
        }
        if unit == 0x5c {
            at += 1;
            if body.get(at).is_none_or(|unit| matches!(unit, 0x0a | 0x0d | 0x2028 | 0x2029)) {
                return None;
            }
        }
        at += 1;
    }
    None
}

pub struct Qwen3Renderer<F: FinalValueFactory> {
    factory: Arc<F>,
}
pub struct Qwen3InbandScanner<F: FinalValueFactory> {
    inner: Qwen3InbandUnitsScanner<F>,
}
impl<F: FinalValueFactory> Qwen3InbandScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { inner: Qwen3InbandUnitsScanner::new(factory) }
    }
    pub fn with_parse_thinking(factory: Arc<F>, enabled: bool) -> Self {
        Self { inner: Qwen3InbandUnitsScanner::with_parse_thinking(factory, enabled) }
    }
    pub fn try_feed(
        &mut self,
        text: &str,
    ) -> Result<Vec<InbandScanEvent<F::Value>>, super::types::InbandEventProjectionError<F::Error>> {
        let events =
            self.inner.try_feed_units(&text.into()).map_err(super::types::InbandEventProjectionError::Runtime)?;
        super::types::try_project_actual_events_to_strings(events, self.inner.factory.as_ref())
    }
    pub fn try_flush(
        &mut self,
    ) -> Result<Vec<InbandScanEvent<F::Value>>, super::types::InbandEventProjectionError<F::Error>> {
        let events = self.inner.try_flush_units().map_err(super::types::InbandEventProjectionError::Runtime)?;
        super::types::try_project_actual_events_to_strings(events, self.inner.factory.as_ref())
    }
}
impl<F: FinalValueFactory> Qwen3Renderer<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { factory }
    }
    pub fn prompt(&self) -> &'static str {
        QWEN3_PROMPT
    }
    pub fn render_tool_call_units(&self, call: &F::Value) -> Result<WireString, F::Error> {
        super::rendering::render_wrapped_call_units(call, self.factory.as_ref())
    }
    pub fn render_assistant_tool_calls_units(&self, calls: &[&F::Value]) -> Result<WireString, F::Error> {
        let mut out = Vec::new();
        for (index, call) in calls.iter().enumerate() {
            if index > 0 {
                out.push(0x0a);
            }
            out.extend_from_slice(self.render_tool_call_units(call)?.units());
        }
        Ok(wire(&out))
    }
    pub fn render_tool_results_units(&self, results: &[WireString]) -> WireString {
        super::rendering::render_tool_response_results_units(results)
    }
    pub fn render_thinking_units(&self, text: &WireString) -> WireString {
        super::rendering::render_simple_thinking_units(text)
    }
    pub fn render_transcript_units(
        &self,
        messages: &[super::rendering::TranscriptMessageUnits<'_, F::Value>],
    ) -> Result<WireString, F::Error> {
        super::rendering::render_chat_ml_transcript_units(
            messages,
            "user",
            |text| self.render_thinking_units(text),
            |calls| self.render_assistant_tool_calls_units(calls),
            |results| self.render_tool_results_units(results),
        )
    }
}
