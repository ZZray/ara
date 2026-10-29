//! Fixed OMP 596f2da, packages/ai/src/dialect/harmony.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::coercion::mint_tool_call_id;
use super::harmony::{CALL, CHANNEL, CONSTRAIN, END, MESSAGE, RETURN, START};
use super::types::{InbandScanEvent, InbandScannerUnits};
use super::units::{
    ascii, equals_ascii, find_ascii, is_space, partial_suffix_overlap_any, starts_ascii, trim, trim_start, wire,
};
use crate::relaxed_json::{FinalParseError, FinalValueFactory, parse_json_with_repair_units};
use ara_rpc::WireString;
use std::convert::Infallible;
use std::sync::Arc;

const ALL_TOKENS: &[&str] = &[START, END, MESSAGE, CHANNEL, CONSTRAIN, RETURN, CALL];
const BODY_TOKENS: &[&str] = &[END, CALL, RETURN, START, CHANNEL, MESSAGE, CONSTRAIN];
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Outside,
    Header,
    Body,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyMode {
    Text,
    Thinking,
    Tool,
    Skip,
}

pub struct HarmonyInbandUnitsScanner<F: FinalValueFactory> {
    factory: Arc<F>,
    id_source: Arc<dyn Fn() -> WireString + Send + Sync>,
    buffer: Vec<u16>,
    state: State,
    mode: BodyMode,
    id: WireString,
    name: WireString,
    tool_args: Vec<u16>,
    thinking: Vec<u16>,
    raw_block: Vec<u16>,
}
impl<F: FinalValueFactory> HarmonyInbandUnitsScanner<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self::with_id_source(factory, Arc::new(|| WireString::from(mint_tool_call_id())))
    }
    pub fn with_id_source(factory: Arc<F>, id_source: Arc<dyn Fn() -> WireString + Send + Sync>) -> Self {
        Self {
            factory,
            id_source,
            buffer: Vec::new(),
            state: State::Outside,
            mode: BodyMode::Skip,
            id: wire(&[]),
            name: wire(&[]),
            tool_args: Vec::new(),
            thinking: Vec::new(),
            raw_block: Vec::new(),
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
        // Native flush consumes only a nonempty pending buffer; do not invent
        // a tool/thinking end after feed already drained a plain body.
        while !self.buffer.is_empty() {
            if self.state == State::Outside {
                let Some((index, token)) = find_next_token(&self.buffer, ALL_TOKENS) else {
                    let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, ALL_TOKENS) };
                    let emit_end = self.buffer.len() - hold;
                    if emit_end > 0 {
                        events.push(InbandScanEvent::Text { text: wire(&self.buffer[..emit_end]) });
                    }
                    self.buffer.drain(..emit_end);
                    break;
                };
                if index > 0 {
                    events.push(InbandScanEvent::Text { text: wire(&self.buffer[..index]) });
                }
                if token == START {
                    self.raw_block = ascii(START);
                    self.buffer.drain(..index + START.len());
                    self.state = State::Header;
                    continue;
                }
                if token == CHANNEL {
                    self.raw_block.clear();
                    self.buffer.drain(..index);
                    self.state = State::Header;
                    continue;
                }
                self.buffer.drain(..index + token.len());
                continue;
            }
            if self.state == State::Header {
                let Some(message) = find_ascii(&self.buffer, MESSAGE) else {
                    if final_input {
                        self.reset_all();
                    }
                    break;
                };
                self.raw_block.extend_from_slice(&self.buffer[..message + MESSAGE.len()]);
                let header = parse_header(&self.buffer[..message]);
                self.buffer.drain(..message + MESSAGE.len());
                self.enter_body(header, &mut events);
                continue;
            }
            let Some((index, token)) = find_next_token(&self.buffer, BODY_TOKENS) else {
                let hold = if final_input { 0 } else { partial_suffix_overlap_any(&self.buffer, BODY_TOKENS) };
                let emit_end = self.buffer.len() - hold;
                let chunk = self.buffer[..emit_end].to_vec();
                self.emit_body(&chunk, &mut events);
                self.buffer.drain(..emit_end);
                if final_input && self.buffer.is_empty() {
                    self.finish_body(&mut events)?;
                    self.state = State::Outside;
                }
                break;
            };
            let chunk = self.buffer[..index].to_vec();
            self.emit_body(&chunk, &mut events);
            if matches!(token, END | CALL | RETURN) {
                if self.mode == BodyMode::Tool {
                    self.raw_block.extend(ascii(token));
                }
                self.buffer.drain(..index + token.len());
                self.finish_body(&mut events)?;
                self.state = State::Outside;
                continue;
            }
            if token == START {
                self.finish_body(&mut events)?;
                self.raw_block = ascii(START);
                self.buffer.drain(..index + START.len());
                self.state = State::Header;
                continue;
            }
            if token == CHANNEL {
                self.finish_body(&mut events)?;
                self.raw_block.clear();
                self.buffer.drain(..index);
                self.state = State::Header;
                continue;
            }
            if self.mode == BodyMode::Tool {
                self.raw_block.extend(ascii(token));
            }
            self.buffer.drain(..index + token.len());
        }
        Ok(events)
    }
    fn enter_body(&mut self, header: HeaderFields, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) {
        self.clear_body(false);
        self.state = State::Body;
        if !header.role.is_empty() && !equals_ascii(&header.role, "assistant") {
            self.mode = BodyMode::Skip;
        } else if !header.recipient.is_empty() && !equals_ascii(&header.recipient, "assistant") {
            self.mode = BodyMode::Tool;
            self.id = (self.id_source)();
            self.name = wire(if starts_ascii(&header.recipient, "functions.") {
                &header.recipient[10..]
            } else {
                &header.recipient
            });
            events.push(InbandScanEvent::ToolStart { id: self.id.clone(), name: self.name.clone() });
        } else if equals_ascii(&header.channel, "analysis") {
            self.mode = BodyMode::Thinking;
            events.push(InbandScanEvent::ThinkingStart);
        } else {
            self.mode = BodyMode::Text;
        }
    }
    fn emit_body(&mut self, chunk: &[u16], events: &mut Vec<InbandScanEvent<F::Value, WireString>>) {
        if chunk.is_empty() {
            return;
        }
        match self.mode {
            BodyMode::Text => events.push(InbandScanEvent::Text { text: wire(chunk) }),
            BodyMode::Thinking => {
                self.thinking.extend_from_slice(chunk);
                events.push(InbandScanEvent::ThinkingDelta { delta: wire(chunk) });
            }
            BodyMode::Tool => {
                self.raw_block.extend_from_slice(chunk);
                self.tool_args.extend_from_slice(chunk);
            }
            BodyMode::Skip => {}
        }
    }
    fn finish_body(&mut self, events: &mut Vec<InbandScanEvent<F::Value, WireString>>) -> Result<(), F::Error> {
        if self.mode == BodyMode::Thinking {
            events.push(InbandScanEvent::ThinkingEnd { thinking: wire(&self.thinking) });
        } else if self.mode == BodyMode::Tool && !self.name.is_empty() {
            let arguments = self.parse_args()?;
            events.push(InbandScanEvent::ToolEnd {
                id: self.id.clone(),
                name: self.name.clone(),
                arguments,
                raw_block: Some(wire(&self.raw_block)),
            });
        }
        self.clear_body(true);
        Ok(())
    }
    fn parse_args(&self) -> Result<F::Value, F::Error> {
        let raw = trim(&self.tool_args);
        // Fixed harmony.ts:225-233: empty allocation is outside the try.
        if raw.is_empty() {
            return self.factory.object();
        }
        // parse AND asRecord share the Native catch, including asRecord's
        // fresh-object allocation for a successfully parsed non-record.
        let parsed = (|| {
            let value = parse_json_with_repair_units(&wire(raw), self.factory.as_ref())?;
            if self.factory.is_record(&value).map_err(FinalParseError::Runtime)? {
                Ok(value)
            } else {
                self.factory.object().map_err(FinalParseError::Runtime)
            }
        })();
        match parsed {
            Ok(value) => Ok(value),
            // Native Syntax and exceptions share the fixed catch. A binding
            // boundary is not a Native exception and must remain an error.
            Err(FinalParseError::Runtime(error)) if !self.factory.should_catch_runtime_error(&error) => Err(error),
            // This fallback allocation is outside the caught operation. Its
            // failure propagates without clearing or rewinding scanner effects.
            Err(FinalParseError::Syntax(_) | FinalParseError::Runtime(_)) => self.factory.object(),
        }
    }
    fn clear_body(&mut self, reset_raw_block: bool) {
        self.mode = BodyMode::Skip;
        self.id = wire(&[]);
        self.name = wire(&[]);
        self.tool_args.clear();
        self.thinking.clear();
        if reset_raw_block {
            self.raw_block.clear();
        }
    }
    fn reset_all(&mut self) {
        self.buffer.clear();
        self.state = State::Outside;
        self.clear_body(true);
    }
}
impl<F: FinalValueFactory<Error = Infallible>> InbandScannerUnits for HarmonyInbandUnitsScanner<F> {
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
fn find_next_token(text: &[u16], tokens: &[&'static str]) -> Option<(usize, &'static str)> {
    let mut best = None;
    for &token in tokens {
        if let Some(index) = find_ascii(text, token)
            && best.is_none_or(|(previous, _)| index < previous)
        {
            best = Some((index, token));
        }
    }
    best
}
struct HeaderFields {
    role: Vec<u16>,
    channel: Vec<u16>,
    recipient: Vec<u16>,
}
fn parse_header(raw: &[u16]) -> HeaderFields {
    let (role, channel) =
        find_ascii(raw, CHANNEL).map_or((raw, &[][..]), |index| (&raw[..index], &raw[index + CHANNEL.len()..]));
    HeaderFields {
        role: first_word(role).to_vec(),
        channel: first_word(channel).to_vec(),
        recipient: parse_recipient(raw).to_vec(),
    }
}
fn first_word(text: &[u16]) -> &[u16] {
    let text = trim_start(text);
    let end = text.iter().position(|&unit| unit == 0x3c || is_space(unit)).unwrap_or(text.len());
    &text[..end]
}
fn parse_recipient(header: &[u16]) -> &[u16] {
    let mut from = 0;
    while let Some(relative) = find_ascii(&header[from..], "to=") {
        let index = from + relative;
        from = index + 3;
        if index > 0 && !is_space(header[index - 1]) {
            continue;
        }
        let tail = &header[from..];
        let end = tail.iter().position(|&unit| unit == 0x3c || is_space(unit)).unwrap_or(tail.len());
        if end > 0 {
            return &tail[..end];
        }
    }
    &[]
}
