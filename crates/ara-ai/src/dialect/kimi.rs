//! Fixed OMP 596f2da, packages/ai/src/dialect/kimi.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! One UTF-16 scanner kernel. The scalar facade only delegates to that kernel;
//! arguments remain the actual factory value, never a projected serde tree.
//! Rendering and the actual wrapper/Host binding are separate mandatory ports.

use super::types::{InbandScanEvent, InbandScanner, InbandScannerUnits};
use super::units::{ascii, checked_string_events, find_ascii, partial_suffix_overlap_any, starts_ascii, trim, wire};
use crate::relaxed_json::{FinalParseError, FinalValueFactory, parse_json_with_repair_units};
use ara_rpc::WireString;
use std::convert::Infallible;
use std::sync::Arc;

pub const KIMI_SECTION_BEGIN: &str = "<|tool_calls_section_begin|>";
pub const KIMI_SECTION_END: &str = "<|tool_calls_section_end|>";
pub const KIMI_CALL_BEGIN: &str = "<|tool_call_begin|>";
pub const KIMI_CALL_END: &str = "<|tool_call_end|>";
pub const KIMI_ARG_BEGIN: &str = "<|tool_call_argument_begin|>";

const TOKENS: &[&str] = &[KIMI_SECTION_BEGIN, KIMI_SECTION_END, KIMI_CALL_BEGIN, KIMI_CALL_END, KIMI_ARG_BEGIN];
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
const TOKENS_THINK: &[&str] =
    &[KIMI_SECTION_BEGIN, KIMI_SECTION_END, KIMI_CALL_BEGIN, KIMI_CALL_END, KIMI_ARG_BEGIN, THINK_OPEN];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Outside,
    Section,
    Header,
    Args,
    Thinking,
}

pub struct KimiInbandUnitsScanner<F: FinalValueFactory> {
    factory: Arc<F>,
    buffer: Vec<u16>,
    state: State,
    id: WireString,
    name: WireString,
    raw_block: Vec<u16>,
    thinking: Vec<u16>,
    parse_thinking: bool,
}

impl<F: FinalValueFactory> KimiInbandUnitsScanner<F> {
    /// Native options.parseThinking !== false: omitted defaults to enabled.
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
            raw_block: Vec::new(),
            thinking: Vec::new(),
            parse_thinking,
        }
    }

    pub fn try_feed_units(
        &mut self,
        text: &WireString,
    ) -> Result<Vec<InbandScanEvent<F::Value, WireString>>, F::Error> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        self.buffer.extend_from_slice(text.units());
        self.consume(false)
    }

    pub fn try_flush_units(&mut self) -> Result<Vec<InbandScanEvent<F::Value, WireString>>, F::Error> {
        self.consume(true)
    }

    fn consume(&mut self, final_input: bool) -> Result<Vec<InbandScanEvent<F::Value, WireString>>, F::Error> {
        let mut events = Vec::new();
        while !self.buffer.is_empty() {
            let consumed = match self.state {
                State::Outside => self.consume_outside(final_input, &mut events),
                State::Thinking => self.consume_thinking(final_input, &mut events),
                State::Section => self.consume_section(final_input),
                State::Header => self.consume_header(final_input, &mut events),
                State::Args => self.consume_args(final_input, &mut events)?,
            };
            if !consumed {
                break;
            }
        }
        // Native ends thinking on flush even after a previous feed emptied the
        // buffer. This final-state check is distinct from Harmony's behavior.
        if final_input && self.state == State::Thinking {
            self.end_thinking(&mut events);
        }
        Ok(events)
    }

    fn consume_outside(&mut self, final_input: bool, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) -> bool {
        let token_start = self.next_token_index();
        let think_start = if self.parse_thinking { find_ascii(&self.buffer, THINK_OPEN) } else { None };
        let start = match (token_start, think_start) {
            (Some(token), Some(thinking)) if thinking < token => Some(thinking),
            (Some(token), _) => Some(token),
            (None, thinking) => thinking,
        };
        let Some(start) = start else {
            let tags = if self.parse_thinking { TOKENS_THINK } else { TOKENS };
            let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, tags) };
            let emit_end = self.buffer.len() - hold;
            if emit_end > 0 {
                events.push(InbandScanEvent::Text { text: wire(&self.buffer[..emit_end]) });
            }
            self.buffer.drain(..emit_end);
            return false;
        };
        if start > 0 {
            events.push(InbandScanEvent::Text { text: wire(&self.buffer[..start]) });
        }
        self.buffer.drain(..start);
        if self.parse_thinking && starts_ascii(&self.buffer, THINK_OPEN) {
            self.buffer.drain(..THINK_OPEN.len());
            self.thinking.clear();
            events.push(InbandScanEvent::ThinkingStart);
            self.state = State::Thinking;
            return true;
        }
        let Some(token) = self.token_at_start() else {
            return false;
        };
        self.buffer.drain(..token.len());
        if token == KIMI_SECTION_BEGIN {
            self.state = State::Section;
        } else {
            events.push(InbandScanEvent::Text { text: WireString::from(token) });
        }
        true
    }

    fn consume_thinking(&mut self, final_input: bool, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) -> bool {
        let Some(close) = find_ascii(&self.buffer, THINK_CLOSE) else {
            let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, &[THINK_CLOSE]) };
            let emit_end = self.buffer.len() - hold;
            let delta = self.buffer[..emit_end].to_vec();
            self.emit_thinking(&delta, events);
            self.buffer.drain(..emit_end);
            if final_input {
                self.end_thinking(events);
                self.state = State::Outside;
            }
            return false;
        };
        let delta = self.buffer[..close].to_vec();
        self.emit_thinking(&delta, events);
        self.buffer.drain(..close + THINK_CLOSE.len());
        self.end_thinking(events);
        self.state = State::Outside;
        true
    }

    fn emit_thinking(&mut self, delta: &[u16], events: &mut Vec<InbandScanEvent<F::Value, WireString>>) {
        if delta.is_empty() {
            return;
        }
        self.thinking.extend_from_slice(delta);
        events.push(InbandScanEvent::ThinkingDelta { delta: wire(delta) });
    }

    fn end_thinking(&mut self, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) {
        events.push(InbandScanEvent::ThinkingEnd { thinking: wire(&self.thinking) });
        self.thinking.clear();
        self.state = State::Outside;
    }

    fn consume_section(&mut self, final_input: bool) -> bool {
        self.skip_whitespace();
        if self.buffer.is_empty() {
            return false;
        }
        match self.token_at_start() {
            Some(KIMI_SECTION_END) => {
                self.buffer.drain(..KIMI_SECTION_END.len());
                self.state = State::Outside;
                return true;
            }
            Some(KIMI_CALL_BEGIN) => {
                self.buffer.drain(..KIMI_CALL_BEGIN.len());
                self.state = State::Header;
                return true;
            }
            Some(token) => {
                self.buffer.drain(..token.len());
                return true;
            }
            None => {}
        }
        if !final_input && partial_suffix_overlap_any(&self.buffer, TOKENS) == self.buffer.len() {
            return false;
        }
        // Native slice(1) discards one UTF-16 unit, including a lone surrogate.
        self.buffer.drain(..1);
        true
    }

    fn consume_header(&mut self, final_input: bool, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) -> bool {
        let Some(separator) = find_ascii(&self.buffer, KIMI_ARG_BEGIN) else {
            if final_input {
                self.drop_buffered_call();
            }
            return false;
        };
        let raw_header = &self.buffer[..separator];
        self.id = wire(trim(raw_header));
        self.name = normalize_kimi_name_units(self.id.units());
        self.raw_block = ascii(KIMI_CALL_BEGIN);
        self.raw_block.extend_from_slice(raw_header);
        self.raw_block.extend(ascii(KIMI_ARG_BEGIN));
        events.push(InbandScanEvent::ToolStart { id: self.id.clone(), name: self.name.clone() });
        self.buffer.drain(..separator + KIMI_ARG_BEGIN.len());
        self.state = State::Args;
        true
    }

    fn consume_args(
        &mut self,
        final_input: bool,
        events: &mut Vec<InbandScanEvent<F::Value, WireString>>,
    ) -> Result<bool, F::Error> {
        let Some(end) = find_ascii(&self.buffer, KIMI_CALL_END) else {
            if final_input {
                self.drop_buffered_call();
            }
            return Ok(false);
        };
        let raw_args_block = &self.buffer[..end];
        let arguments = self.parse_args(trim(raw_args_block))?;
        let mut raw_block = self.raw_block.clone();
        raw_block.extend_from_slice(raw_args_block);
        raw_block.extend(ascii(KIMI_CALL_END));
        events.push(InbandScanEvent::ToolEnd {
            id: self.id.clone(),
            name: self.name.clone(),
            arguments,
            raw_block: Some(WireString::from_units(raw_block)),
        });
        // A failed fallback leaves this buffer/current call intact, just as
        // Native throws before its post-event slice/reset/state transitions.
        self.buffer.drain(..end + KIMI_CALL_END.len());
        self.reset_call();
        self.state = State::Section;
        Ok(true)
    }

    fn parse_args(&self, raw_args: &[u16]) -> Result<F::Value, F::Error> {
        // Fixed kimi.ts:213-220 has the same local catch position as Harmony.
        // Empty raw allocation is outside it; parse AND asRecord are inside.
        if raw_args.is_empty() {
            return self.factory.object();
        }
        let parsed: Result<F::Value, FinalParseError<F::Error>> = (|| {
            let value = parse_json_with_repair_units(&wire(raw_args), self.factory.as_ref())?;
            if self.factory.is_record(&value).map_err(FinalParseError::Runtime)? {
                Ok(value)
            } else {
                self.factory.object().map_err(FinalParseError::Runtime)
            }
        })();
        match parsed {
            Ok(value) => Ok(value),
            // Unsupported Host/carrier boundaries are not Native exceptions.
            Err(FinalParseError::Runtime(error)) if !self.factory.should_catch_runtime_error(&error) => Err(error),
            // The fresh fallback object remains outside the caught operation.
            Err(FinalParseError::Syntax(_) | FinalParseError::Runtime(_)) => self.factory.object(),
        }
    }

    fn next_token_index(&self) -> Option<usize> {
        let mut best = None;
        for token in TOKENS {
            if let Some(index) = find_ascii(&self.buffer, token)
                && best.is_none_or(|previous| index < previous)
            {
                best = Some(index);
            }
        }
        best
    }

    fn token_at_start(&self) -> Option<&'static str> {
        TOKENS.iter().copied().find(|token| starts_ascii(&self.buffer, token))
    }

    fn skip_whitespace(&mut self) {
        // Native Kimi section whitespace is exactly these six units; id/args
        // trim is the separate full ECMAScript trim implemented by units::trim.
        let length =
            self.buffer.iter().take_while(|&&unit| matches!(unit, 0x20 | 0x09 | 0x0a | 0x0d | 0x0b | 0x0c)).count();
        self.buffer.drain(..length);
    }

    fn drop_buffered_call(&mut self) {
        self.buffer.clear();
        self.reset_call();
        self.state = State::Outside;
    }

    fn reset_call(&mut self) {
        self.id = wire(&[]);
        self.name = wire(&[]);
        self.raw_block.clear();
    }
}

impl<F: FinalValueFactory<Error = Infallible>> InbandScannerUnits for KimiInbandUnitsScanner<F> {
    type Arguments = F::Value;
    fn feed_units(&mut self, text: &WireString) -> Vec<InbandScanEvent<F::Value, WireString>> {
        match self.try_feed_units(text) {
            Ok(events) => events,
            Err(error) => match error {},
        }
    }
    fn flush_units(&mut self) -> Vec<InbandScanEvent<F::Value, WireString>> {
        match self.try_flush_units() {
            Ok(events) => events,
            Err(error) => match error {},
        }
    }
}

/// Restricted scalar facade over the one units kernel; never used to admit raw
/// UTF-16 inputs. Actual fallible integrations call the units try APIs directly.
pub struct KimiInbandScanner<F: FinalValueFactory> {
    inner: KimiInbandUnitsScanner<F>,
}
impl<F: FinalValueFactory> KimiInbandScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { inner: KimiInbandUnitsScanner::new(factory) }
    }
    pub fn with_parse_thinking(factory: Arc<F>, parse_thinking: bool) -> Self {
        Self { inner: KimiInbandUnitsScanner::with_parse_thinking(factory, parse_thinking) }
    }
}
impl<F: FinalValueFactory<Error = Infallible>> InbandScanner for KimiInbandScanner<F> {
    type Arguments = F::Value;
    fn feed(&mut self, text: &str) -> Vec<InbandScanEvent<F::Value>> {
        checked_string_events(self.inner.feed_units(&WireString::from(text)))
    }
    fn flush(&mut self) -> Vec<InbandScanEvent<F::Value>> {
        checked_string_events(self.inner.flush_units())
    }
}

fn normalize_kimi_name_units(raw_id: &[u16]) -> WireString {
    // Exact coercion.ts split(':',1), split('.'), last part, trim in unit space.
    // The existing scalar helper cannot admit unpaired UTF-16 source units.
    let end = raw_id.iter().position(|&unit| unit == 0x3a).unwrap_or(raw_id.len());
    let before_index = &raw_id[..end];
    let start = before_index.iter().rposition(|&unit| unit == 0x2e).map_or(0, |index| index + 1);
    wire(trim(&before_index[start..]))
}
