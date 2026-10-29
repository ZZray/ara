//! Fixed OMP 596f2da, packages/ai/src/dialect/hermes.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! Actual graph names/arguments; exact UTF16 scanner and Native catch positions.

use super::coercion::{mint_tool_call_id, parse_named_call};
use super::types::{ActualInbandScanEvent, InbandNameValue, InbandScanEvent};
use super::units::{find_ascii, partial_suffix_overlap_any, wire};
use crate::relaxed_json::{FinalParseError, FinalValueFactory, parse_streaming_json_units};
use ara_rpc::WireString;
use std::sync::Arc;

const TOOL_OPEN: &str = "<tool_call>";
const TOOL_CLOSE: &str = "</tool_call>";
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
const HOLD_TAGS: &[&str] = &[TOOL_OPEN, TOOL_CLOSE, THINK_OPEN, THINK_CLOSE];
pub const HERMES_PROMPT: &str = include_str!("hermes.md");

pub struct HermesInbandUnitsScanner<F: FinalValueFactory> {
    factory: Arc<F>,
    buffer: Vec<u16>,
    inside: bool,
    id: WireString,
    name: InbandNameValue<F::Value>,
    started: bool,
    parse_thinking: bool,
    in_thinking: bool,
    thinking: Vec<u16>,
}
impl<F: FinalValueFactory> HermesInbandUnitsScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self::with_parse_thinking(factory, false)
    }
    pub fn with_parse_thinking(factory: Arc<F>, parse_thinking: bool) -> Self {
        Self {
            factory,
            buffer: Vec::new(),
            inside: false,
            id: wire(&[]),
            name: InbandNameValue::Source(wire(&[])),
            started: false,
            parse_thinking,
            in_thinking: false,
            thinking: Vec::new(),
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
            if self.in_thinking {
                let close = find_ascii(&self.buffer, THINK_CLOSE);
                if let Some(close) = close {
                    self.emit_thinking(close, &mut events);
                    self.buffer.drain(..close + THINK_CLOSE.len());
                    self.end_thinking(&mut events);
                    continue;
                }
                let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, &[THINK_CLOSE]) };
                let end = self.buffer.len() - hold;
                self.emit_thinking(end, &mut events);
                self.buffer.drain(..end);
                if final_input {
                    self.end_thinking(&mut events);
                }
                break;
            }
            if !self.inside {
                let open = find_ascii(&self.buffer, TOOL_OPEN);
                let think = if self.parse_thinking { find_ascii(&self.buffer, THINK_OPEN) } else { None };
                let start = match (open, think) {
                    (Some(open), Some(think)) => Some(open.min(think)),
                    (Some(open), None) => Some(open),
                    (None, think) => think,
                };
                let Some(start) = start else {
                    // Even when thinking is disabled, all four suffix tags hold.
                    let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, HOLD_TAGS) };
                    let end = self.buffer.len() - hold;
                    if end > 0 {
                        events.push(InbandScanEvent::Text { text: wire(&self.buffer[..end]) });
                    }
                    self.buffer.drain(..end);
                    break;
                };
                if start > 0 {
                    events.push(InbandScanEvent::Text { text: wire(&self.buffer[..start]) });
                }
                if think == Some(start) {
                    self.buffer.drain(..start + THINK_OPEN.len());
                    self.in_thinking = true;
                    self.thinking.clear();
                    events.push(InbandScanEvent::ThinkingStart);
                    continue;
                }
                self.buffer.drain(..start + TOOL_OPEN.len());
                self.inside = true;
                self.id = mint_tool_call_id().into();
                self.name = InbandNameValue::Source(wire(&[]));
                self.started = false;
                continue;
            }
            let close = find_ascii(&self.buffer, TOOL_CLOSE);
            let body = wire(&self.buffer[..close.unwrap_or(self.buffer.len())]);
            if !self.started {
                self.try_start(&body, &mut events)?;
            }
            let Some(close) = close else {
                if final_input {
                    self.reset();
                }
                break;
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
        }
        // Native Hermes has no extra final-state check outside this loop.
        Ok(events)
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
        self.in_thinking = false;
    }
    fn try_start(
        &mut self,
        body: &WireString,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<(), F::Error> {
        let attempt = (|| {
            let partial = parse_streaming_json_units(body, self.factory.as_ref())?;
            let first = self.factory.get(&partial, &"name".into()).map_err(FinalParseError::Runtime)?;
            if self.factory.string_value(&first).map_err(FinalParseError::Runtime)?.is_none() {
                return Ok(None);
            }
            let second = self.factory.get(&partial, &"name".into()).map_err(FinalParseError::Runtime)?;
            let length = self.factory.get(&second, &"length".into()).map_err(FinalParseError::Runtime)?;
            if self.factory.is_number_zero(&length).map_err(FinalParseError::Runtime)? {
                return Ok(None);
            }
            self.factory.get(&partial, &"name".into()).map(Some).map_err(FinalParseError::Runtime)
        })();
        let name = match attempt {
            Ok(name) => name,
            Err(FinalParseError::Syntax(_)) => None,
            Err(FinalParseError::Runtime(error)) if self.factory.should_catch_runtime_error(&error) => None,
            Err(FinalParseError::Runtime(error)) => return Err(error),
        };
        if let Some(name) = name {
            self.name = InbandNameValue::Actual(name);
            self.started = true;
            events.push(InbandScanEvent::ToolStart { id: self.id.clone(), name: self.name.clone() });
        }
        Ok(())
    }
    fn reset(&mut self) {
        self.inside = false;
        self.id = wire(&[]);
        self.name = InbandNameValue::Source(wire(&[]));
        self.started = false;
    }
}

/// Actual call-handle renderer. Root owns the same-lineage Host/message view.
pub struct HermesRenderer<F: FinalValueFactory> {
    factory: Arc<F>,
}
pub struct HermesInbandScanner<F: FinalValueFactory> {
    inner: HermesInbandUnitsScanner<F>,
}
impl<F: FinalValueFactory> HermesInbandScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { inner: HermesInbandUnitsScanner::new(factory) }
    }
    pub fn with_parse_thinking(factory: Arc<F>, enabled: bool) -> Self {
        Self { inner: HermesInbandUnitsScanner::with_parse_thinking(factory, enabled) }
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
impl<F: FinalValueFactory> HermesRenderer<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { factory }
    }
    pub fn prompt(&self) -> &'static str {
        HERMES_PROMPT
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
        super::rendering::render_delimited_thinking_units(text)
    }
    pub fn render_transcript_units(
        &self,
        messages: &[super::rendering::TranscriptMessageUnits<'_, F::Value>],
    ) -> Result<WireString, F::Error> {
        super::rendering::render_chat_ml_transcript_units(
            messages,
            "tool",
            |text| self.render_thinking_units(text),
            |calls| self.render_assistant_tool_calls_units(calls),
            |results| self.render_tool_results_units(results),
        )
    }
}
