//! Fixed OMP 596f2da, dialect/qwen-xml.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//! UTF16 source scanning; ordinary attribute assignment uses the original heap.

use crate::actual_provider::GraphBuilder;
use crate::dialect::{ActualInbandScanEvent, InbandNameValue, InbandScanEvent};
use ara_rpc::{
    WireString,
    value_heap::{PropertyKey, ValueError, ValueHandle},
};

const OPEN: &str = "<tool_calls>";
const CLOSE: &str = "</tool_calls>";

pub(super) struct QwenXmlScanner {
    builder: GraphBuilder,
    buffer: Vec<u16>,
    inside: bool,
}

impl QwenXmlScanner {
    pub(super) fn new(builder: GraphBuilder) -> Self {
        Self { builder, buffer: Vec::new(), inside: false }
    }
    pub(super) fn feed(&mut self, chunk: &WireString) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>, ValueError> {
        self.buffer.extend_from_slice(chunk.units());
        self.drain(false)
    }
    pub(super) fn flush(&mut self) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>, ValueError> {
        self.drain(true)
    }
    fn drain(&mut self, final_input: bool) -> Result<Vec<ActualInbandScanEvent<ValueHandle>>, ValueError> {
        let mut events = Vec::new();
        while !self.buffer.is_empty() {
            if !self.inside {
                if let Some(open) = find(&self.buffer, OPEN.as_bytes()) {
                    if open > 0 {
                        events.push(InbandScanEvent::Text { text: wire(&self.buffer[..open]) });
                    }
                    self.buffer.drain(..open + OPEN.len());
                    self.inside = true;
                    continue;
                }
                if final_input {
                    events.push(InbandScanEvent::Text { text: wire(&self.buffer) });
                    self.buffer.clear();
                    break;
                }
                let held = (1..=self.buffer.len().min(OPEN.len() - 1))
                    .rev()
                    .find(|length| ascii_eq(&self.buffer[self.buffer.len() - length..], &OPEN.as_bytes()[..*length]))
                    .unwrap_or(0);
                let visible = self.buffer.len() - held;
                if visible > 0 {
                    events.push(InbandScanEvent::Text { text: wire(&self.buffer[..visible]) });
                    self.buffer.drain(..visible);
                }
                break;
            }
            let Some(close) = find(&self.buffer, CLOSE.as_bytes()) else {
                if final_input {
                    self.buffer.clear();
                }
                break;
            };
            let section = self.buffer[..close].to_vec();
            // /<[^<>]+\/>/gs: global nonoverlapping self-closing candidates.
            let mut index = 0;
            while index < section.len() {
                if section[index] != 0x3c {
                    index += 1;
                    continue;
                }
                let start = index;
                index += 1;
                while index < section.len() && section[index] != 0x3c && section[index] != 0x3e {
                    index += 1;
                }
                if index == section.len() {
                    break;
                }
                if section[index] == 0x3c {
                    continue;
                }
                let end = index + 1;
                if index > start + 2 && section[index - 1] == 0x2f {
                    let raw = &section[start..end];
                    if let Some((name, arguments)) = self.parse_element(raw)? {
                        let id = super::markup::new_call_id()?;
                        events.push(InbandScanEvent::ToolStart {
                            id: id.clone(),
                            name: InbandNameValue::Source(name.clone()),
                        });
                        events.push(InbandScanEvent::ToolEnd {
                            id,
                            name: InbandNameValue::Source(name),
                            arguments,
                            raw_block: Some(wire(raw)),
                        });
                    }
                    index = end;
                } else {
                    index += 1;
                }
            }
            self.buffer.drain(..close + CLOSE.len());
            self.inside = false;
        }
        Ok(events)
    }
    fn parse_element(&self, element: &[u16]) -> Result<Option<(WireString, ValueHandle)>, ValueError> {
        // /^<([A-Za-z_][\w.-]*)(\s[^<>]*?)?\s*\/>$/s
        if element.len() < 4 || element[0] != 0x3c || !name_start(element[1]) {
            return Ok(None);
        }
        let mut end_name = 2;
        while end_name < element.len() && name_rest(element[end_name]) {
            end_name += 1;
        }
        let attributes = &element[end_name..element.len() - 2];
        if !attributes.is_empty() && !space(attributes[0]) {
            return Ok(None);
        }
        let name = wire(&element[1..end_name]);
        let arguments = self.builder.0.heap().object()?;
        // /([A-Za-z_][\w.-]*)\s*=\s*("([^"]*)"|'([^']*)')/g
        let mut index = 0;
        while index < attributes.len() {
            if !name_start(attributes[index]) {
                index += 1;
                continue;
            }
            let start = index;
            let mut key_end = index + 1;
            while key_end < attributes.len() && name_rest(attributes[key_end]) {
                key_end += 1;
            }
            let mut cursor = key_end;
            while cursor < attributes.len() && space(attributes[cursor]) {
                cursor += 1;
            }
            if attributes.get(cursor) != Some(&0x3d) {
                index += 1;
                continue;
            }
            cursor += 1;
            while cursor < attributes.len() && space(attributes[cursor]) {
                cursor += 1;
            }
            let Some(&quote @ (0x22 | 0x27)) = attributes.get(cursor) else {
                index += 1;
                continue;
            };
            cursor += 1;
            let value_start = cursor;
            while cursor < attributes.len() && attributes[cursor] != quote {
                cursor += 1;
            }
            if cursor == attributes.len() {
                index += 1;
                continue;
            }
            let key = wire(&attributes[start..key_end]);
            let value = self.builder.text(wire(&attributes[value_start..cursor]))?;
            self.builder.0.heap().assign(&arguments, PropertyKey::String(key), value, self.builder.0.runtime())?;
            index = cursor + 1;
        }
        Ok(Some((name, arguments)))
    }
}

fn wire(units: &[u16]) -> WireString {
    WireString::from_units(units.to_vec())
}
fn name_start(unit: u16) -> bool {
    matches!(unit, 0x41..=0x5a | 0x61..=0x7a | 0x5f)
}
fn name_rest(unit: u16) -> bool {
    name_start(unit) || matches!(unit, 0x30..=0x39 | 0x2e | 0x2d)
}
fn space(unit: u16) -> bool {
    char::from_u32(u32::from(unit)).is_some_and(ara_prompt::js::is_space)
}
fn ascii_eq(units: &[u16], ascii: &[u8]) -> bool {
    units.len() == ascii.len() && units.iter().zip(ascii).all(|(left, right)| *left == u16::from(*right))
}
fn find(units: &[u16], ascii: &[u8]) -> Option<usize> {
    units.windows(ascii.len()).position(|window| ascii_eq(window, ascii))
}
