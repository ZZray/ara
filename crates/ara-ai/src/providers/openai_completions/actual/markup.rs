//! Fixed OMP 596f2da, utils/stream-markup-healing.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.

use super::qwen_xml::QwenXmlScanner;
use crate::actual_provider::GraphBuilder;
use crate::dialect::{
    ActualInbandScanEvent, DeepSeekInbandUnitsScanner, InbandNameValue, InbandScanEvent, InbandScannerUnits,
    KimiInbandUnitsScanner, ThinkingInbandScannerOptions, ThinkingInbandUnitsScanner,
};
use crate::graph_values::GraphValueFactory;
use ara_rpc::{
    WireString,
    value_heap::{StringifyMode, ValueError, ValueHandle},
};
use ring::rand::{SecureRandom, SystemRandom};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealingPattern {
    Kimi,
    Dsml,
    Qwen,
    Thinking,
}

pub struct HealedToolCall {
    pub id: WireString,
    pub name: ValueHandle,
    /// Actual result of JSON.stringify; undefined stays undefined.
    pub arguments: ValueHandle,
}
pub enum HealingEvent {
    Text(WireString),
    Thinking(WireString),
    ToolCall(HealedToolCall),
}
enum ToolScanner {
    Kimi(KimiInbandUnitsScanner<GraphValueFactory>),
    // Fixed XML(xmlTagset=dsml) delegates directly to DeepSeek's scanner.
    Dsml(DeepSeekInbandUnitsScanner<GraphValueFactory>),
    Qwen(QwenXmlScanner),
}
impl ToolScanner {
    fn feed(&mut self, text: &WireString) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>, ValueError> {
        match self {
            Self::Kimi(scanner) => scanner.try_feed_units(text).map(source_events),
            Self::Dsml(scanner) => scanner.try_feed_units(text),
            Self::Qwen(scanner) => scanner.feed(text),
        }
    }
    fn flush(&mut self) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>, ValueError> {
        match self {
            Self::Kimi(scanner) => scanner.try_flush_units().map(source_events),
            Self::Dsml(scanner) => scanner.try_flush_units(),
            Self::Qwen(scanner) => scanner.flush(),
        }
    }
}

pub struct StreamMarkupHealing {
    builder: GraphBuilder,
    pattern: HealingPattern,
    tool: Option<ToolScanner>,
    thinking: ThinkingInbandUnitsScanner<ValueHandle>,
    section_terminated: bool,
    completed: Vec<HealedToolCall>,
}
impl StreamMarkupHealing {
    pub fn new(factory: Arc<GraphValueFactory>, pattern: HealingPattern) -> Result<Self, ValueError> {
        let builder = GraphBuilder(factory.clone());
        let tool = match pattern {
            HealingPattern::Kimi => Some(ToolScanner::Kimi(KimiInbandUnitsScanner::new(factory))),
            HealingPattern::Dsml => Some(ToolScanner::Dsml(DeepSeekInbandUnitsScanner::new(factory)?)),
            HealingPattern::Qwen => Some(ToolScanner::Qwen(QwenXmlScanner::new(builder.clone()))),
            HealingPattern::Thinking => None,
        };
        Ok(Self {
            builder,
            pattern,
            tool,
            thinking: ThinkingInbandUnitsScanner::new(ThinkingInbandScannerOptions::default()),
            section_terminated: false,
            completed: Vec::new(),
        })
    }
    pub fn pattern(&self) -> HealingPattern {
        self.pattern
    }
    pub fn section_closed(&self) -> bool {
        self.section_terminated
    }
    pub fn feed_events(&mut self, text: &WireString) -> Result<Vec<HealingEvent>, ValueError> {
        if text.is_empty() {
            return Ok(Vec::new());
        }
        if !self.section_terminated && self.tool.is_some() {
            // Source tests only this chunk; do not join split close tokens here.
            self.section_terminated = match self.pattern {
                HealingPattern::Kimi => contains(text, "<|tool_calls_section_end|>"),
                HealingPattern::Qwen => contains(text, "</tool_calls>"),
                _ => contains(text, "</｜DSML｜tool_calls>") || contains(text, "</|DSML|tool_calls>"),
            };
        }
        let scanned = if let Some(tool) = &mut self.tool {
            let events = tool.feed(text)?;
            self.heal_thinking(events)
        } else {
            source_events(self.thinking.feed_units(text))
        };
        self.convert(scanned)
    }
    pub fn feed_events_without_calls(&mut self, text: &WireString) -> Result<Vec<HealingEvent>, ValueError> {
        Ok(self.feed_events(text)?.into_iter().filter(|event| !matches!(event, HealingEvent::ToolCall(_))).collect())
    }
    pub fn feed(&mut self, text: &WireString) -> Result<WireString, ValueError> {
        let events = self.feed_events(text)?;
        Ok(self.text_and_completed(events))
    }
    pub fn drain_completed(&mut self) -> Vec<HealedToolCall> {
        std::mem::take(&mut self.completed)
    }
    pub fn flush_events(&mut self) -> Result<Vec<HealingEvent>, ValueError> {
        let mut tail = if let Some(tool) = &mut self.tool {
            let events = tool.flush()?;
            self.heal_thinking(events)
        } else {
            Vec::new()
        };
        tail.extend(source_events(self.thinking.flush_units()));
        self.convert(tail)
    }
    pub fn flush_pending(&mut self) -> Result<WireString, ValueError> {
        let events = self.flush_events()?;
        Ok(self.text_and_completed(events))
    }
    fn text_and_completed(&mut self, events: Vec<HealingEvent>) -> WireString {
        let mut output = Vec::new();
        for event in events {
            match event {
                HealingEvent::Text(text) => output.extend_from_slice(text.units()),
                HealingEvent::ToolCall(call) => self.completed.push(call),
                HealingEvent::Thinking(_) => {}
            }
        }
        WireString::from_units(output)
    }
    fn heal_thinking(
        &mut self,
        events: Vec<ActualInbandScanEvent<ValueHandle>>,
    ) -> Vec<ActualInbandScanEvent<ValueHandle>> {
        let mut output = Vec::new();
        for event in events {
            match event {
                InbandScanEvent::Text { text } => output.extend(source_events(self.thinking.feed_units(&text))),
                event => output.push(event),
            }
        }
        output
    }
    fn convert(&self, events: Vec<ActualInbandScanEvent<ValueHandle>>) -> Result<Vec<HealingEvent>, ValueError> {
        let mut output = Vec::new();
        for event in events {
            match event {
                InbandScanEvent::Text { text } => output.push(HealingEvent::Text(text)),
                InbandScanEvent::ThinkingDelta { delta } if !delta.is_empty() => {
                    output.push(HealingEvent::Thinking(delta))
                }
                InbandScanEvent::ToolEnd { name, arguments, .. } => {
                    // Generate the wrapper id before JSON.stringify, as source.
                    let id = new_call_id()?;
                    let name = match name {
                        InbandNameValue::Source(name) => self.builder.text(name)?,
                        InbandNameValue::Actual(name) => name,
                    };
                    let arguments = match self.builder.0.heap().stringify(
                        &arguments,
                        StringifyMode::Plain,
                        None,
                        self.builder.0.runtime(),
                    )? {
                        Some(text) => self.builder.text(text)?,
                        None => self.builder.0.heap().undefined()?,
                    };
                    output.push(HealingEvent::ToolCall(HealedToolCall { id, name, arguments }));
                }
                _ => {}
            }
        }
        Ok(output)
    }
}

fn source_events(events: Vec<InbandScanEvent<ValueHandle, WireString>>) -> Vec<ActualInbandScanEvent<ValueHandle>> {
    events
        .into_iter()
        .map(|event| match event {
            InbandScanEvent::Text { text } => InbandScanEvent::Text { text },
            InbandScanEvent::ThinkingStart => InbandScanEvent::ThinkingStart,
            InbandScanEvent::ThinkingDelta { delta } => InbandScanEvent::ThinkingDelta { delta },
            InbandScanEvent::ThinkingEnd { thinking } => InbandScanEvent::ThinkingEnd { thinking },
            InbandScanEvent::ImpliedThinkingEnd => InbandScanEvent::ImpliedThinkingEnd,
            InbandScanEvent::ToolStart { id, name } => {
                InbandScanEvent::ToolStart { id, name: InbandNameValue::Source(name) }
            }
            InbandScanEvent::ToolArgDelta { id, name, key, delta } => {
                InbandScanEvent::ToolArgDelta { id, name: InbandNameValue::Source(name), key, delta }
            }
            InbandScanEvent::ToolEnd { id, name, arguments, raw_block } => {
                InbandScanEvent::ToolEnd { id, name: InbandNameValue::Source(name), arguments, raw_block }
            }
        })
        .collect()
}
pub(super) fn new_call_id() -> Result<WireString, ValueError> {
    let mut uuid = [0_u8; 16];
    SystemRandom::new()
        .fill(&mut uuid)
        .map_err(|_| ValueError::UnsupportedOperation("System crypto randomUUID is unavailable"))?;
    uuid[6] = (uuid[6] & 0x0f) | 0x40;
    uuid[8] = (uuid[8] & 0x3f) | 0x80;
    const HEX: &[u8] = b"0123456789abcdef";
    let mut text = b"call_".to_vec();
    for byte in &uuid[..12] {
        text.push(HEX[(byte >> 4) as usize]);
        text.push(HEX[(byte & 0x0f) as usize]);
    }
    Ok(String::from_utf8(text).expect("ASCII UUID").into())
}
fn contains(text: &WireString, needle: &str) -> bool {
    let needle = needle.encode_utf16().collect::<Vec<_>>();
    text.units().windows(needle.len()).any(|window| window == needle)
}

/// Source DeepSeek post-healing token stripping and its held partial suffix.
pub(super) fn trailing_deepseek_partial(text: &WireString) -> usize {
    let units = text.units();
    let start = units
        .windows(2)
        .enumerate()
        .filter(|(_, pair)| pair[0] == 0x3c && matches!(pair[1], 0x7c | 0xff5c))
        .map(|(index, _)| index)
        .last();
    let Some(start) = start else {
        return usize::from(units.last() == Some(&0x3c));
    };
    let tail = &units[start..];
    if tail.len() > 256 || tail.windows(2).any(|pair| matches!(pair[0], 0x7c | 0xff5c) && pair[1] == 0x3e) {
        return 0;
    }
    tail.len()
}
fn token_end(units: &[u16], start: usize) -> Option<usize> {
    if units.get(start) != Some(&0x3c) || !units.get(start + 1).is_some_and(|unit| matches!(unit, 0x7c | 0xff5c)) {
        return None;
    }
    let mut index = start + 2;
    while index < units.len() && index <= start + 66 {
        let unit = units[index];
        if index >= start + 3 && matches!(unit, 0x7c | 0xff5c) && units.get(index + 1) == Some(&0x3e) {
            return Some(index + 2);
        }
        if !matches!(unit, 0x41..=0x5a | 0x61..=0x7a | 0x30..=0x39 | 0x5f | 0x2e | 0x7c | 0xff5c | 0x2581) {
            return None;
        }
        index += 1;
    }
    None
}
fn space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(ara_prompt::js::is_space)
}
pub(super) fn strip_deepseek_tokens(text: &WireString) -> WireString {
    let units = text.units();
    let first = units.iter().position(|unit| !space(*unit)).unwrap_or(units.len());
    let trim_start = token_end(units, first).is_some();
    let mut trim_end = false;
    let mut output = Vec::new();
    let mut changed = false;
    let mut index = 0;
    while index < units.len() {
        if let Some(end) = token_end(units, index) {
            if units[end..].iter().all(|unit| space(*unit)) {
                trim_end = true;
            }
            changed = true;
            index = end;
        } else {
            output.push(units[index]);
            index += 1;
        }
    }
    if !changed {
        return text.clone();
    }
    let start = if trim_start { output.iter().position(|unit| !space(*unit)).unwrap_or(output.len()) } else { 0 };
    let end = if trim_end {
        output.iter().rposition(|unit| !space(*unit)).map_or(0, |index| index + 1)
    } else {
        output.len()
    };
    WireString::from_units(output[start.min(end)..end].to_vec())
}
