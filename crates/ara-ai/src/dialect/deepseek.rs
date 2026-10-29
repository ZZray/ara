//! Fixed OMP 596f2da, packages/ai/src/dialect/deepseek.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! All ten original states, fullwidth/ASCII DSML and actual ordinary assignment.

use super::coercion::{as_record_value, mint_tool_call_id};
use super::types::{ActualInbandScanEvent, InbandNameValue, InbandScanEvent};
use super::units::{find_ascii, find_units, is_space, starts_ascii, suffix_units_overlap, trim, trim_start, wire};
use crate::relaxed_json::{FinalParseError, FinalValueFactory, parse_json_with_repair_units};
use ara_rpc::WireString;
use std::sync::{Arc, LazyLock};

pub const DEEPSEEK_TOOL_CALLS_BEGIN: &str = "<｜tool▁calls▁begin｜>";
pub const DEEPSEEK_TOOL_CALLS_END: &str = "<｜tool▁calls▁end｜>";
pub const DEEPSEEK_TOOL_CALL_BEGIN: &str = "<｜tool▁call▁begin｜>";
pub const DEEPSEEK_TOOL_CALL_END: &str = "<｜tool▁call▁end｜>";
pub const DEEPSEEK_TOOL_SEPARATOR: &str = "<｜tool▁sep｜>";
pub const DEEPSEEK_TOOL_OUTPUT_BEGIN: &str = "<｜tool▁output▁begin｜>";
pub const DEEPSEEK_TOOL_OUTPUT_END: &str = "<｜tool▁output▁end｜>";
pub const DEEPSEEK_BOS: &str = "<｜begin▁of▁sentence｜>";
pub const DEEPSEEK_USER: &str = "<｜User｜>";
pub const DEEPSEEK_ASSISTANT: &str = "<｜Assistant｜>";
pub const DEEPSEEK_EOS: &str = "<｜end▁of▁sentence｜>";
const THINK_OPEN: &str = "<think>";
const THINK_CLOSE: &str = "</think>";
const DSML_CALLS_OPEN: &[&str] = &["<｜DSML｜tool_calls>", "<|DSML|tool_calls>"];
const DSML_CALLS_CLOSE: &[&str] = &["</｜DSML｜tool_calls>", "</|DSML|tool_calls>"];
const DSML_INVOKE_OPEN: &[&str] = &["<｜DSML｜invoke", "<|DSML|invoke"];
const DSML_INVOKE_CLOSE: &[&str] = &["</｜DSML｜invoke>", "</|DSML|invoke>"];
const DSML_PARAM_OPEN: &[&str] = &["<｜DSML｜parameter", "<|DSML|parameter"];
const DSML_PARAM_CLOSE: &[&str] = &["</｜DSML｜parameter>", "</|DSML|parameter>"];
const CONTROL_TOKENS: &[&str] = &[
    DEEPSEEK_BOS,
    DEEPSEEK_EOS,
    "<｜▁pad▁｜>",
    DEEPSEEK_USER,
    DEEPSEEK_ASSISTANT,
    "<|EOT|>",
    "<｜search▁begin｜>",
    "<｜search▁end｜>",
    "<｜fim▁hole｜>",
    "<｜fim▁begin｜>",
    "<｜fim▁end｜>",
    "<｜tool▁outputs▁begin｜>",
    "<｜tool▁outputs▁end｜>",
    DEEPSEEK_TOOL_OUTPUT_BEGIN,
    DEEPSEEK_TOOL_OUTPUT_END,
];
pub const DEEPSEEK_PROMPT: &str = include_str!("deepseek.md");

struct Tokens {
    calls_begin: Vec<u16>,
    calls_end: Vec<u16>,
    call_begin: Vec<u16>,
    call_end: Vec<u16>,
    separator: Vec<u16>,
    calls_open: Vec<Vec<u16>>,
    calls_close: Vec<Vec<u16>>,
    invoke_open: Vec<Vec<u16>>,
    invoke_close: Vec<Vec<u16>>,
    param_open: Vec<Vec<u16>>,
    param_close: Vec<Vec<u16>>,
    orphan: Vec<Vec<u16>>,
    control: Vec<Vec<u16>>,
    outside: Vec<Vec<u16>>,
    section: Vec<Vec<u16>>,
    dsml_section: Vec<Vec<u16>>,
    dsml_invoke: Vec<Vec<u16>>,
}
fn encoded(tokens: &[&str]) -> Vec<Vec<u16>> {
    tokens.iter().map(|text| text.encode_utf16().collect()).collect()
}
static TOKENS: LazyLock<Tokens> = LazyLock::new(|| {
    let calls_begin = DEEPSEEK_TOOL_CALLS_BEGIN.encode_utf16().collect::<Vec<_>>();
    let calls_end = DEEPSEEK_TOOL_CALLS_END.encode_utf16().collect::<Vec<_>>();
    let call_begin = DEEPSEEK_TOOL_CALL_BEGIN.encode_utf16().collect::<Vec<_>>();
    let call_end = DEEPSEEK_TOOL_CALL_END.encode_utf16().collect();
    let separator = DEEPSEEK_TOOL_SEPARATOR.encode_utf16().collect();
    let calls_open = encoded(DSML_CALLS_OPEN);
    let calls_close = encoded(DSML_CALLS_CLOSE);
    let invoke_open = encoded(DSML_INVOKE_OPEN);
    let invoke_close = encoded(DSML_INVOKE_CLOSE);
    let param_open = encoded(DSML_PARAM_OPEN);
    let param_close = encoded(DSML_PARAM_CLOSE);
    let orphan = invoke_close.iter().chain(&param_close).cloned().collect::<Vec<_>>();
    let control = encoded(CONTROL_TOKENS);
    let mut outside = vec![calls_begin.clone(), calls_end.clone(), call_begin.clone()];
    outside.extend(encoded(&[THINK_OPEN, THINK_CLOSE]));
    outside.extend(calls_open.iter().chain(&calls_close).chain(&orphan).chain(&control).cloned());
    let section = vec![call_begin.clone(), calls_end.clone()];
    let dsml_section = calls_close.iter().chain(&invoke_open).cloned().collect();
    let dsml_invoke = invoke_close.iter().chain(&param_open).cloned().collect();
    Tokens {
        calls_begin,
        calls_end,
        call_begin,
        call_end,
        separator,
        calls_open,
        calls_close,
        invoke_open,
        invoke_close,
        param_open,
        param_close,
        orphan,
        control,
        outside,
        section,
        dsml_section,
        dsml_invoke,
    }
});
fn matching<'a>(buffer: &[u16], tokens: &'a [Vec<u16>]) -> Option<&'a [u16]> {
    tokens.iter().find(|token| buffer.starts_with(token)).map(Vec::as_slice)
}
fn overlap(buffer: &[u16], tokens: &[Vec<u16>]) -> usize {
    tokens.iter().map(|token| suffix_units_overlap(buffer, token)).max().unwrap_or(0)
}
/// Equal offsets prefer the longer token, preserving fixed DeepSeek's policy.
fn earliest<'a>(buffer: &[u16], tokens: &'a [Vec<u16>]) -> Option<(usize, &'a [u16])> {
    let mut best: Option<(usize, &[u16])> = None;
    for token in tokens {
        if let Some(index) = find_units(buffer, token)
            && best.is_none_or(|(at, previous)| index < at || (index == at && token.len() > previous.len()))
        {
            best = Some((index, token));
        }
    }
    best
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Outside,
    Thinking,
    Section,
    Header,
    Args,
    LegacyName,
    LegacyArgs,
    DsmlSection,
    DsmlInvoke,
    DsmlParam,
}
struct DsmlOpen {
    name: WireString,
    string_attr: Option<bool>,
    raw: Vec<u16>,
}

pub struct DeepSeekInbandUnitsScanner<F: FinalValueFactory> {
    factory: Arc<F>,
    buffer: Vec<u16>,
    state: State,
    parse_thinking: bool,
    in_tool_section: bool,
    id: WireString,
    name: WireString,
    thinking: Vec<u16>,
    dsml_args: F::Value,
    dsml_param_name: WireString,
    dsml_param_is_string: bool,
    dsml_param_raw: Vec<u16>,
    raw_block: Vec<u16>,
    strip_leading_whitespace: bool,
}
impl<F: FinalValueFactory> DeepSeekInbandUnitsScanner<F> {
    pub fn new(factory: Arc<F>) -> Result<Self, F::Error> {
        Self::with_parse_thinking(factory, true)
    }
    pub fn with_parse_thinking(factory: Arc<F>, parse_thinking: bool) -> Result<Self, F::Error> {
        // Native class field initialization happens even for a legacy-only stream.
        let dsml_args = factory.object()?;
        Ok(Self {
            factory,
            buffer: Vec::new(),
            state: State::Outside,
            parse_thinking,
            in_tool_section: false,
            id: wire(&[]),
            name: wire(&[]),
            thinking: Vec::new(),
            dsml_args,
            dsml_param_name: wire(&[]),
            dsml_param_is_string: true,
            dsml_param_raw: Vec::new(),
            raw_block: Vec::new(),
            strip_leading_whitespace: false,
        })
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
                    if self.state != State::Outside && !self.buffer.is_empty() {
                        continue;
                    }
                    break;
                }
                State::Thinking => {
                    self.consume_thinking(final_input, &mut events);
                    if !final_input && self.state == State::Thinking {
                        break;
                    }
                }
                State::Section => {
                    if !self.consume_section(final_input) {
                        break;
                    }
                }
                State::Header => {
                    if !self.consume_header(final_input, &mut events) {
                        break;
                    }
                }
                State::LegacyName => {
                    if !self.consume_legacy_name(final_input, &mut events) {
                        break;
                    }
                }
                State::Args | State::LegacyArgs => {
                    if !self.consume_args(final_input, &mut events)? {
                        break;
                    }
                }
                State::DsmlSection => {
                    if !self.consume_dsml_section(final_input, &mut events)? {
                        break;
                    }
                }
                State::DsmlInvoke => {
                    if !self.consume_dsml_invoke(final_input, &mut events)? {
                        break;
                    }
                }
                State::DsmlParam => {
                    if !self.consume_dsml_param(final_input, &mut events)? {
                        break;
                    }
                }
            }
        }
        if final_input && self.state == State::Thinking {
            self.end_thinking(&mut events);
        }
        if final_input && self.buffer.is_empty() && !self.raw_block.is_empty() {
            self.raw_block.clear();
        }
        Ok(events)
    }
    fn consume_outside(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        while !self.buffer.is_empty() {
            if self.strip_leading_whitespace {
                let first = self.buffer.len() - trim_start(&self.buffer).len();
                self.buffer.drain(..first);
                if self.buffer.is_empty() {
                    return;
                }
                self.strip_leading_whitespace = false;
            }
            let Some((index, token)) = earliest(&self.buffer, &TOKENS.outside) else {
                let hold = if final_input { 0 } else { overlap(&self.buffer, &TOKENS.outside) };
                let end = self.buffer.len() - hold;
                if end > 0 {
                    events.push(InbandScanEvent::Text { text: wire(&self.buffer[..end]) });
                }
                self.buffer.drain(..end);
                return;
            };
            if index > 0 {
                events.push(InbandScanEvent::Text { text: wire(&self.buffer[..index]) });
            }
            self.buffer.drain(..index);
            if self.buffer.starts_with(&TOKENS.calls_begin) {
                self.buffer.drain(..TOKENS.calls_begin.len());
                self.in_tool_section = true;
                self.state = State::Section;
                return;
            }
            if self.buffer.starts_with(&TOKENS.call_begin) {
                self.buffer.drain(..TOKENS.call_begin.len());
                self.raw_block = TOKENS.call_begin.clone();
                self.in_tool_section = false;
                self.state = State::Header;
                return;
            }
            if starts_ascii(&self.buffer, THINK_OPEN) {
                self.buffer.drain(..THINK_OPEN.len());
                self.state = State::Thinking;
                self.thinking.clear();
                if self.parse_thinking {
                    events.push(InbandScanEvent::ThinkingStart);
                }
                return;
            }
            if let Some(open) = matching(&self.buffer, &TOKENS.calls_open) {
                self.buffer.drain(..open.len());
                self.state = State::DsmlSection;
                return;
            }
            if let Some(close) = matching(&self.buffer, &TOKENS.orphan) {
                self.buffer.drain(..close.len());
                continue;
            }
            let control = if self.buffer.starts_with(&TOKENS.calls_end) {
                Some(TOKENS.calls_end.as_slice())
            } else if starts_ascii(&self.buffer, THINK_CLOSE) {
                Some(&[0x3c, 0x2f, 0x74, 0x68, 0x69, 0x6e, 0x6b, 0x3e][..])
            } else {
                matching(&self.buffer, &TOKENS.calls_close).or_else(|| matching(&self.buffer, &TOKENS.control))
            };
            if let Some(control) = control {
                self.buffer.drain(..control.len());
                self.strip_leading_whitespace = true;
                continue;
            }
            self.buffer.drain(..token.len());
        }
    }
    fn consume_thinking(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if let Some(close) = find_ascii(&self.buffer, THINK_CLOSE) {
            self.emit_thinking(wire(&self.buffer[..close]), events);
            self.buffer.drain(..close + THINK_CLOSE.len());
            self.end_thinking(events);
        } else {
            let close = [0x3c, 0x2f, 0x74, 0x68, 0x69, 0x6e, 0x6b, 0x3e];
            let hold = if final_input { 0 } else { suffix_units_overlap(&self.buffer, &close) };
            let end = self.buffer.len() - hold;
            self.emit_thinking(wire(&self.buffer[..end]), events);
            self.buffer.drain(..end);
            if final_input {
                self.end_thinking(events);
            }
        }
    }
    fn consume_section(&mut self, final_input: bool) -> bool {
        while !self.buffer.is_empty() {
            self.skip_whitespace();
            if self.buffer.starts_with(&TOKENS.calls_end) {
                self.buffer.drain(..TOKENS.calls_end.len());
                self.in_tool_section = false;
                self.state = State::Outside;
                return true;
            }
            if self.buffer.starts_with(&TOKENS.call_begin) {
                self.buffer.drain(..TOKENS.call_begin.len());
                self.raw_block = TOKENS.call_begin.clone();
                self.state = State::Header;
                return true;
            }
            if !final_input && overlap(&self.buffer, &TOKENS.section) == self.buffer.len() {
                return false;
            }
            if self.buffer.is_empty() {
                return false;
            }
            self.buffer.drain(..1);
        }
        final_input
    }
    fn consume_header(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) -> bool {
        let Some(sep) = find_units(&self.buffer, &TOKENS.separator) else {
            if final_input {
                self.reset_tool(State::Outside);
            }
            return false;
        };
        let end = sep + TOKENS.separator.len();
        let head = wire(trim(&self.buffer[..sep]));
        self.raw_block.extend_from_slice(&self.buffer[..end]);
        self.buffer.drain(..end);
        if head.equals_ascii("function") {
            self.state = State::LegacyName;
            return true;
        }
        self.start_tool(head, events);
        self.state = State::Args;
        true
    }
    fn consume_legacy_name(&mut self, final_input: bool, events: &mut Vec<ActualInbandScanEvent<F::Value>>) -> bool {
        let Some(fence) = find_ascii(&self.buffer, "```json") else {
            if final_input {
                self.reset_tool(State::Outside);
            }
            return false;
        };
        let end = fence + 7;
        let name = wire(trim(&self.buffer[..fence]));
        self.raw_block.extend_from_slice(&self.buffer[..end]);
        self.buffer.drain(..end);
        let line = if starts_ascii(&self.buffer, "\r\n") {
            2
        } else if starts_ascii(&self.buffer, "\n") {
            1
        } else {
            0
        };
        self.raw_block.extend_from_slice(&self.buffer[..line]);
        self.buffer.drain(..line);
        self.start_tool(name, events);
        self.state = State::LegacyArgs;
        true
    }
    fn consume_args(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<bool, F::Error> {
        let Some(end) = find_units(&self.buffer, &TOKENS.call_end) else {
            if final_input {
                self.reset_tool(State::Outside);
            }
            return Ok(false);
        };
        let mut arg_end = end;
        if self.state == State::LegacyArgs
            && let Some(fence) = self.buffer[..end].windows(3).rposition(|units| units == [0x60, 0x60, 0x60])
        {
            arg_end = fence;
        }
        let raw_args = wire(&self.buffer[..arg_end]);
        let tail_end = end + TOKENS.call_end.len();
        self.raw_block.extend_from_slice(&self.buffer[..tail_end]);
        let arguments = self.parse_args(&raw_args)?;
        events.push(InbandScanEvent::ToolEnd {
            id: self.id.clone(),
            name: InbandNameValue::Source(self.name.clone()),
            arguments,
            raw_block: Some(wire(&self.raw_block)),
        });
        self.buffer.drain(..tail_end);
        self.reset_tool(if self.in_tool_section { State::Section } else { State::Outside });
        Ok(true)
    }
    fn consume_dsml_section(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<bool, F::Error> {
        while !self.buffer.is_empty() {
            self.skip_whitespace();
            if let Some(close) = matching(&self.buffer, &TOKENS.calls_close) {
                self.buffer.drain(..close.len());
                self.state = State::Outside;
                return Ok(true);
            }
            if let Some(invoke) = self.match_dsml_open(false) {
                self.raw_block = invoke.raw;
                self.name = invoke.name;
                self.id = mint_tool_call_id().into();
                self.dsml_args = self.factory.object()?;
                events.push(InbandScanEvent::ToolStart {
                    id: self.id.clone(),
                    name: InbandNameValue::Source(self.name.clone()),
                });
                self.state = State::DsmlInvoke;
                return Ok(true);
            }
            if !final_input {
                if matching(&self.buffer, &TOKENS.invoke_open).is_some() && !self.buffer.contains(&0x3e) {
                    return Ok(false);
                }
                if overlap(&self.buffer, &TOKENS.dsml_section) == self.buffer.len() {
                    return Ok(false);
                }
            }
            if self.buffer.is_empty() {
                return Ok(false);
            }
            self.buffer.drain(..1);
        }
        Ok(final_input)
    }
    fn consume_dsml_invoke(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<bool, F::Error> {
        while !self.buffer.is_empty() {
            let skipped = self.skip_whitespace();
            self.raw_block.extend_from_slice(&skipped);
            if let Some(close) = matching(&self.buffer, &TOKENS.invoke_close) {
                self.raw_block.extend_from_slice(close);
                self.buffer.drain(..close.len());
                events.push(InbandScanEvent::ToolEnd {
                    id: self.id.clone(),
                    name: InbandNameValue::Source(self.name.clone()),
                    arguments: self.dsml_args.clone(),
                    raw_block: Some(wire(&self.raw_block)),
                });
                self.reset_dsml_tool()?;
                self.state = State::DsmlSection;
                return Ok(true);
            }
            if let Some(param) = self.match_dsml_open(true) {
                self.raw_block.extend(param.raw);
                self.dsml_param_name = param.name;
                self.dsml_param_is_string = param.string_attr != Some(false);
                self.state = State::DsmlParam;
                return Ok(true);
            }
            if !final_input {
                if matching(&self.buffer, &TOKENS.param_open).is_some() && !self.buffer.contains(&0x3e) {
                    return Ok(false);
                }
                if overlap(&self.buffer, &TOKENS.dsml_invoke) == self.buffer.len() {
                    return Ok(false);
                }
            }
            // Source accesses buffer[0] after the whitespace skip; empty
            // buffer yields undefined then string concatenation "undefined".
            if self.buffer.is_empty() {
                self.raw_block.extend("undefined".encode_utf16());
            } else {
                self.raw_block.push(self.buffer[0]);
                self.buffer.drain(..1);
            }
        }
        Ok(final_input)
    }
    fn consume_dsml_param(
        &mut self,
        final_input: bool,
        events: &mut Vec<ActualInbandScanEvent<F::Value>>,
    ) -> Result<bool, F::Error> {
        let Some((index, token)) = earliest(&self.buffer, &TOKENS.param_close) else {
            let hold = if final_input { 0 } else { overlap(&self.buffer, &TOKENS.param_close) };
            let end = self.buffer.len() - hold;
            self.stream_dsml_param(wire(&self.buffer[..end]), events);
            self.buffer.drain(..end);
            if final_input {
                self.reset_dsml_tool()?;
            }
            return Ok(false);
        };
        self.stream_dsml_param(wire(&self.buffer[..index]), events);
        let value = self.coerce_dsml_value()?;
        self.factory.assign(&mut self.dsml_args, self.dsml_param_name.clone(), value)?;
        // On assignment failure the closer and state are still untouched.
        self.raw_block.extend_from_slice(token);
        self.buffer.drain(..index + token.len());
        self.dsml_param_name = wire(&[]);
        self.dsml_param_is_string = true;
        self.dsml_param_raw.clear();
        self.state = State::DsmlInvoke;
        Ok(true)
    }
    fn start_tool(&mut self, name: WireString, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        self.name = name;
        self.id = mint_tool_call_id().into();
        events
            .push(InbandScanEvent::ToolStart { id: self.id.clone(), name: InbandNameValue::Source(self.name.clone()) });
    }
    fn stream_dsml_param(&mut self, chunk: WireString, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if chunk.is_empty() {
            return;
        }
        self.dsml_param_raw.extend_from_slice(chunk.units());
        self.raw_block.extend_from_slice(chunk.units());
        events.push(InbandScanEvent::ToolArgDelta {
            id: self.id.clone(),
            name: InbandNameValue::Source(self.name.clone()),
            key: self.dsml_param_name.clone(),
            delta: chunk,
        });
    }
    fn emit_thinking(&mut self, delta: WireString, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if delta.is_empty() {
            return;
        }
        if self.parse_thinking {
            self.thinking.extend_from_slice(delta.units());
            events.push(InbandScanEvent::ThinkingDelta { delta });
        } else {
            events.push(InbandScanEvent::Text { text: delta });
        }
    }
    fn end_thinking(&mut self, events: &mut Vec<ActualInbandScanEvent<F::Value>>) {
        if self.parse_thinking {
            events.push(InbandScanEvent::ThinkingEnd { thinking: wire(&self.thinking) });
        }
        self.thinking.clear();
        self.state = State::Outside;
    }
    fn parse_args(&self, raw: &WireString) -> Result<F::Value, F::Error> {
        let trimmed = wire(trim(raw.units()));
        if trimmed.is_empty() {
            return self.factory.object();
        }
        let attempt = parse_json_with_repair_units(&trimmed, self.factory.as_ref())
            .and_then(|value| as_record_value(self.factory.as_ref(), value).map_err(FinalParseError::Runtime));
        match attempt {
            Ok(value) => Ok(value),
            Err(FinalParseError::Syntax(_)) => self.factory.object(),
            Err(FinalParseError::Runtime(error)) if self.factory.should_catch_runtime_error(&error) => {
                self.factory.object()
            }
            Err(FinalParseError::Runtime(error)) => Err(error),
        }
    }
    fn coerce_dsml_value(&self) -> Result<F::Value, F::Error> {
        if self.dsml_param_is_string || trim(&self.dsml_param_raw).is_empty() {
            return self.factory.string(wire(&self.dsml_param_raw));
        }
        match parse_json_with_repair_units(&wire(trim(&self.dsml_param_raw)), self.factory.as_ref()) {
            Ok(value) => Ok(value),
            Err(FinalParseError::Syntax(_)) => self.factory.string(wire(&self.dsml_param_raw)),
            Err(FinalParseError::Runtime(error)) if self.factory.should_catch_runtime_error(&error) => {
                self.factory.string(wire(&self.dsml_param_raw))
            }
            Err(FinalParseError::Runtime(error)) => Err(error),
        }
    }
    fn skip_whitespace(&mut self) -> Vec<u16> {
        let end = self.buffer.iter().position(|&unit| !is_space(unit)).unwrap_or(self.buffer.len());
        self.buffer.drain(..end).collect()
    }
    fn match_dsml_open(&mut self, parameter: bool) -> Option<DsmlOpen> {
        let prefixes = if parameter { &TOKENS.param_open } else { &TOKENS.invoke_open };
        matching(&self.buffer, prefixes)?;
        let end = self.buffer.iter().position(|&unit| unit == 0x3e)?;
        let raw = self.buffer[..=end].to_vec();
        let name = dsml_attribute(&raw, "name", None)?;
        let string_attr =
            dsml_attribute(&raw, "string", Some(&["true", "false"])).map(|value| value.equals_ascii("true"));
        self.buffer.drain(..=end);
        Some(DsmlOpen { name, string_attr, raw })
    }
    fn reset_tool(&mut self, next: State) {
        self.state = next;
        self.id = wire(&[]);
        self.name = wire(&[]);
        self.raw_block.clear();
    }
    fn reset_dsml_tool(&mut self) -> Result<(), F::Error> {
        self.id = wire(&[]);
        self.name = wire(&[]);
        self.dsml_args = self.factory.object()?;
        self.dsml_param_name = wire(&[]);
        self.dsml_param_is_string = true;
        self.dsml_param_raw.clear();
        self.raw_block.clear();
        Ok(())
    }
}
/// Source regex /\sname="([^"]*)"/ or /\sstring="(true|false)"/.
/// Skip invalid string matches and keep looking; name permits the empty value.
fn dsml_attribute(tag: &[u16], name: &str, allowed: Option<&[&str]>) -> Option<WireString> {
    for at in 0..tag.len() {
        if !is_space(tag[at]) || !starts_ascii(&tag[at + 1..], name) {
            continue;
        }
        let opening = at + 1 + name.len();
        if tag.get(opening..opening + 2) != Some(&[0x3d, 0x22][..]) {
            continue;
        }
        let start = opening + 2;
        let end = start + tag[start..].iter().position(|&unit| unit == 0x22)?;
        let value = wire(&tag[start..end]);
        if allowed.is_none_or(|values| values.iter().any(|value_allowed| value.equals_ascii(value_allowed))) {
            return Some(value);
        }
    }
    None
}

pub struct DeepSeekRenderer<F: FinalValueFactory> {
    factory: Arc<F>,
}
pub struct DeepSeekInbandScanner<F: FinalValueFactory> {
    inner: DeepSeekInbandUnitsScanner<F>,
}
impl<F: FinalValueFactory> DeepSeekInbandScanner<F> {
    pub fn new(factory: Arc<F>) -> Result<Self, F::Error> {
        Ok(Self { inner: DeepSeekInbandUnitsScanner::new(factory)? })
    }
    pub fn with_parse_thinking(factory: Arc<F>, enabled: bool) -> Result<Self, F::Error> {
        Ok(Self { inner: DeepSeekInbandUnitsScanner::with_parse_thinking(factory, enabled)? })
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
impl<F: FinalValueFactory> DeepSeekRenderer<F> {
    pub fn new(factory: Arc<F>) -> Self {
        Self { factory }
    }
    pub fn prompt(&self) -> &'static str {
        DEEPSEEK_PROMPT
    }
    pub fn render_tool_call_units(&self, call: &F::Value) -> Result<WireString, F::Error> {
        let name = self.factory.get(call, &"name".into())?;
        let name = self.factory.implicit_string(&name)?;
        let arguments = self.factory.get(call, &"arguments".into())?;
        let json = super::rendering::stringify_value_units(&arguments, self.factory.as_ref())?;
        let mut out = TOKENS.call_begin.clone();
        out.extend_from_slice(name.units());
        out.extend(&TOKENS.separator);
        out.extend_from_slice(json.units());
        out.extend(&TOKENS.call_end);
        Ok(wire(&out))
    }
    pub fn render_assistant_tool_calls_units(&self, calls: &[&F::Value]) -> Result<WireString, F::Error> {
        if calls.is_empty() {
            return Ok(wire(&[]));
        }
        let mut out = TOKENS.calls_begin.clone();
        for call in calls {
            out.extend_from_slice(self.render_tool_call_units(call)?.units());
        }
        out.extend(&TOKENS.calls_end);
        Ok(wire(&out))
    }
    pub fn render_tool_results_units(&self, results: &[WireString]) -> WireString {
        let mut out = Vec::new();
        for (index, text) in results.iter().enumerate() {
            if index > 0 {
                out.push(0x0a);
            }
            out.extend(DEEPSEEK_TOOL_OUTPUT_BEGIN.encode_utf16());
            out.extend_from_slice(text.units());
            out.extend(DEEPSEEK_TOOL_OUTPUT_END.encode_utf16());
        }
        wire(&out)
    }
    pub fn render_thinking_units(&self, text: &WireString) -> WireString {
        super::rendering::render_simple_thinking_units(text)
    }
    pub fn render_transcript_units(
        &self,
        messages: &[super::rendering::TranscriptMessageUnits<'_, F::Value>],
    ) -> Result<WireString, F::Error> {
        use super::rendering::{
            TranscriptMessageUnits, assistant_transcript_parts_units, collect_tool_result_run_units,
            message_content_text_units,
        };
        if messages.is_empty() {
            return Ok(wire(&[]));
        }
        let mut out = WireString::from(DEEPSEEK_BOS).units().to_vec();
        let mut index = 0;
        while let Some(message) = messages.get(index) {
            match message {
                TranscriptMessageUnits::Assistant(blocks) => {
                    let parts = assistant_transcript_parts_units(blocks);
                    out.extend(DEEPSEEK_ASSISTANT.encode_utf16());
                    out.extend_from_slice(self.render_thinking_units(&parts.thinking).units());
                    out.extend_from_slice(parts.text.units());
                    out.extend_from_slice(self.render_assistant_tool_calls_units(&parts.calls)?.units());
                    out.extend(DEEPSEEK_EOS.encode_utf16());
                    index += 1;
                }
                TranscriptMessageUnits::ToolResult { .. } => {
                    let (texts, next) = collect_tool_result_run_units(messages, index);
                    out.extend_from_slice(self.render_tool_results_units(&texts).units());
                    index = next;
                }
                TranscriptMessageUnits::Content { role, content } => {
                    if !role.equals_ascii("developer") {
                        out.extend(DEEPSEEK_USER.encode_utf16());
                    }
                    out.extend_from_slice(message_content_text_units(content).units());
                    index += 1;
                }
            }
        }
        Ok(wire(&out))
    }
}
