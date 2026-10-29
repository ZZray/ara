//! Fixed OMP 596f2da, packages/ai/src/dialect/rendering.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See LICENSE.
//!
//! Message/text helpers and actual graph-render seams. Python value rendering remains
//! gated on Root's actual V2 value inspection; no serde value approximation.

use super::types::{DialectRenderOptions, DialectRenderer, DialectToolResult};
use super::units::{find_ascii, starts_ascii, wire};
use crate::relaxed_json::FinalValueFactory;
use crate::{AssistantBlock, AssistantMessage, Message, ToolCall, UserBlock, UserContent};
use ara_rpc::{WireString, value_heap::StringifyMode};

/// Same-lineage literal wrapper, then ONE stringify. This is the identical
/// Hermes/Qwen3 source call body; do not pre-read fields or stringify args alone.
pub(super) fn render_wrapped_call_units<F: FinalValueFactory>(
    call: &F::Value,
    factory: &F,
) -> Result<WireString, F::Error> {
    let mut wrapper = factory.object()?;
    let name = factory.get(call, &"name".into())?;
    factory.define_own_data(&mut wrapper, "name".into(), name)?;
    let arguments = factory.get(call, &"arguments".into())?;
    factory.define_own_data(&mut wrapper, "arguments".into(), arguments)?;
    let json = stringify_value_units(&wrapper, factory)?;
    let mut out = WireString::from("<tool_call>\n").units().to_vec();
    out.extend_from_slice(json.units());
    out.extend("\n</tool_call>".encode_utf16());
    Ok(wire(&out))
}
pub(super) fn stringify_value_units<F: FinalValueFactory>(
    value: &F::Value,
    factory: &F,
) -> Result<WireString, F::Error> {
    Ok(factory.stringify_json(value, StringifyMode::BigIntReplacer)?.unwrap_or_else(|| "null".into()))
}
pub fn render_tool_response_results_units(results: &[WireString]) -> WireString {
    let mut out = Vec::new();
    for (index, text) in results.iter().enumerate() {
        if index > 0 {
            out.push(0x0a);
        }
        out.extend("<tool_response>\n".encode_utf16());
        out.extend_from_slice(text.units());
        out.extend("\n</tool_response>".encode_utf16());
    }
    wire(&out)
}
pub fn render_simple_thinking_units(text: &WireString) -> WireString {
    if text.is_empty() {
        return wire(&[]);
    }
    let mut out = WireString::from("<think>\n").units().to_vec();
    out.extend_from_slice(text.units());
    out.extend("\n</think>".encode_utf16());
    wire(&out)
}
pub fn render_delimited_thinking_units(text: &WireString) -> WireString {
    if text.is_empty() {
        return wire(&[]);
    }
    let mut out = WireString::from("<think>\n").units().to_vec();
    out.extend(unwrap_thinking_units(text.units()));
    out.extend("\n</think>".encode_utf16());
    wire(&out)
}
fn ascii_space_unit(unit: u16) -> bool {
    matches!(unit, 9 | 10 | 11 | 12 | 13 | 32)
}
fn unwrap_thinking_units(text: &[u16]) -> Vec<u16> {
    let mut end = text.len();
    while end > 0 && ascii_space_unit(text[end - 1]) {
        end -= 1;
    }
    let mut cursor = 0;
    while cursor < end && ascii_space_unit(text[cursor]) {
        cursor += 1;
    }
    if cursor >= end || !starts_ascii(&text[cursor..], "<think>") {
        return text.to_vec();
    }
    let mut out = Vec::new();
    let mut segments = 0;
    while cursor < end {
        if !starts_ascii(&text[cursor..], "<think>") {
            return text.to_vec();
        }
        let inner_start = cursor + 7;
        let mut scan = inner_start;
        let mut depth = 1;
        let inner_end = loop {
            if scan >= end {
                return text.to_vec();
            }
            let Some(close) = find_ascii(&text[scan..], "</think>").map(|close| scan + close) else {
                return text.to_vec();
            };
            if close >= end {
                return text.to_vec();
            }
            if let Some(open) = find_ascii(&text[scan..], "<think>").map(|open| scan + open)
                && open < close
            {
                depth += 1;
                scan = open + 7;
                continue;
            }
            depth -= 1;
            if depth == 0 {
                break close;
            }
            scan = close + 8;
        };
        let mut trim_end = inner_end;
        while trim_end > inner_start && ascii_space_unit(text[trim_end - 1]) {
            trim_end -= 1;
        }
        let mut trim_start = inner_start;
        while trim_start < trim_end && ascii_space_unit(text[trim_start]) {
            trim_start += 1;
        }
        if segments > 0 {
            out.push(0x0a);
        }
        segments += 1;
        out.extend(unwrap_thinking_units(&text[trim_start..trim_end]));
        cursor = inner_end + 8;
        while cursor < end && ascii_space_unit(text[cursor]) {
            cursor += 1;
        }
    }
    out
}

/// Direct readonly projection borrowed from the one actual message owner.
/// It is not another canonical message store or graph-validation engine.
pub enum TranscriptUserBlockUnits<'a> {
    Text(Option<&'a WireString>),
    Image(Option<&'a WireString>),
    Other,
}
pub enum TranscriptContentUnits<'a> {
    Text(&'a WireString),
    Blocks(&'a [TranscriptUserBlockUnits<'a>]),
}
pub enum TranscriptAssistantBlockUnits<'a, V> {
    Text(&'a WireString),
    Thinking(&'a WireString),
    ToolCall(&'a V),
    Other,
}
pub enum TranscriptMessageUnits<'a, V> {
    Assistant(&'a [TranscriptAssistantBlockUnits<'a, V>]),
    ToolResult { id: &'a WireString, name: &'a WireString, content: TranscriptContentUnits<'a>, is_error: bool },
    Content { role: &'a WireString, content: TranscriptContentUnits<'a> },
}
pub struct AssistantTranscriptPartsUnits<'a, V> {
    pub text: WireString,
    pub thinking: WireString,
    pub calls: Vec<&'a V>,
}
pub fn assistant_transcript_parts_units<'a, V>(
    blocks: &'a [TranscriptAssistantBlockUnits<'a, V>],
) -> AssistantTranscriptPartsUnits<'a, V> {
    let mut text = Vec::new();
    let mut thinking = Vec::new();
    let mut thinking_count = 0;
    let mut calls = Vec::new();
    for block in blocks {
        match block {
            TranscriptAssistantBlockUnits::Text(value) => text.extend_from_slice(value.units()),
            TranscriptAssistantBlockUnits::Thinking(value) => {
                if thinking_count > 0 {
                    thinking.push(0x0a);
                }
                thinking_count += 1;
                thinking.extend_from_slice(value.units());
            }
            TranscriptAssistantBlockUnits::ToolCall(value) => calls.push(*value),
            TranscriptAssistantBlockUnits::Other => {}
        }
    }
    AssistantTranscriptPartsUnits { text: wire(&text), thinking: wire(&thinking), calls }
}
pub fn message_content_text_units(content: &TranscriptContentUnits<'_>) -> WireString {
    match content {
        TranscriptContentUnits::Text(text) => (*text).clone(),
        TranscriptContentUnits::Blocks(blocks) => {
            let mut text = Vec::new();
            for block in *blocks {
                match block {
                    TranscriptUserBlockUnits::Text(Some(value)) => text.extend_from_slice(value.units()),
                    TranscriptUserBlockUnits::Image(mime) => {
                        if let Some(mime) = mime.filter(|mime| !mime.is_empty()) {
                            text.extend("[Image: ".encode_utf16());
                            text.extend_from_slice(mime.units());
                            text.push(0x5d);
                        } else {
                            text.extend("[Image]".encode_utf16());
                        }
                    }
                    _ => {}
                }
            }
            wire(&text)
        }
    }
}
pub fn collect_tool_result_run_units<V>(
    messages: &[TranscriptMessageUnits<'_, V>],
    start: usize,
) -> (Vec<WireString>, usize) {
    let mut texts = Vec::new();
    let mut next = start;
    while let Some(TranscriptMessageUnits::ToolResult { content, .. }) = messages.get(next) {
        texts.push(message_content_text_units(content));
        next += 1;
    }
    (texts, next)
}
pub fn chat_ml_turn_units(role: &WireString, body: &WireString) -> WireString {
    let mut out = WireString::from("<|im_start|>").units().to_vec();
    out.extend_from_slice(role.units());
    out.push(0x0a);
    out.extend_from_slice(body.units());
    out.extend("<|im_end|>\n".encode_utf16());
    wire(&out)
}
pub(super) fn render_chat_ml_transcript_units<V, E>(
    messages: &[TranscriptMessageUnits<'_, V>],
    result_role: &str,
    thinking: impl Fn(&WireString) -> WireString,
    calls: impl Fn(&[&V]) -> Result<WireString, E>,
    results: impl Fn(&[WireString]) -> WireString,
) -> Result<WireString, E> {
    let mut out = Vec::new();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        match message {
            TranscriptMessageUnits::Assistant(blocks) => {
                let parts = assistant_transcript_parts_units(blocks);
                let mut body = thinking(&parts.thinking).units().to_vec();
                body.extend_from_slice(parts.text.units());
                body.extend_from_slice(calls(&parts.calls)?.units());
                out.extend_from_slice(chat_ml_turn_units(&"assistant".into(), &wire(&body)).units());
                index += 1;
            }
            TranscriptMessageUnits::ToolResult { .. } => {
                let (texts, next) = collect_tool_result_run_units(messages, index);
                out.extend_from_slice(chat_ml_turn_units(&result_role.into(), &results(&texts)).units());
                index = next;
            }
            TranscriptMessageUnits::Content { role, content } => {
                let system = WireString::from("system");
                let role = if role.equals_ascii("developer") { &system } else { *role };
                out.extend_from_slice(chat_ml_turn_units(role, &message_content_text_units(content)).units());
                index += 1;
            }
        }
    }
    Ok(wire(&out))
}

pub fn render_tool_response_results(results: &[DialectToolResult]) -> String {
    results
        .iter()
        .map(|result| format!("<tool_response>\n{}\n</tool_response>", result.text))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn kimi_call_id(name: &str, id: &str, index: usize) -> String {
    let trimmed = ara_prompt::js::trim(id);
    if trimmed.starts_with("functions.") { trimmed.into() } else { format!("functions.{name}:{index}") }
}

pub fn harmony_recipient(name: &str) -> String {
    if name.starts_with("functions.") { name.into() } else { format!("functions.{name}") }
}

pub fn escape_xml_attr(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn escape_xml_text(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub struct AssistantTranscriptParts<'a> {
    pub text: String,
    pub thinking: String,
    pub tool_calls: Vec<&'a ToolCall>,
}

pub fn assistant_transcript_parts(message: &AssistantMessage) -> AssistantTranscriptParts<'_> {
    let mut text = String::new();
    let mut thinking = Vec::new();
    let mut tool_calls = Vec::new();
    for block in &message.content {
        match block {
            AssistantBlock::Text(block) => text.push_str(&block.text),
            AssistantBlock::Thinking(block) => thinking.push(block.thinking.as_str()),
            AssistantBlock::ToolCall(call) => tool_calls.push(call),
            _ => {}
        }
    }
    AssistantTranscriptParts { text, thinking: thinking.join("\n"), tool_calls }
}

pub fn collect_tool_result_run(messages: &[Message], start: usize) -> (Vec<DialectToolResult>, usize) {
    let mut results = Vec::new();
    let mut next = start;
    while let Some(Message::ToolResult(message)) = messages.get(next) {
        results.push(DialectToolResult {
            id: message.tool_call_id.clone(),
            name: message.tool_name.clone(),
            index: results.len(),
            text: user_blocks_text(&message.content),
            is_error: message.is_error,
        });
        next += 1;
    }
    (results, next)
}

pub fn message_content_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => user_blocks_text(blocks),
    }
}

/// Native transcript image placeholders are distinct from encoded history,
/// whose tool-result text omits images and preserves the image blocks.
pub fn user_blocks_text(blocks: &[UserBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        match block {
            UserBlock::Text(block) => text.push_str(&block.text),
            UserBlock::Image(block) => {
                if block.mime_type.is_empty() {
                    text.push_str("[Image]");
                } else {
                    text.push_str(&format!("[Image: {}]", block.mime_type));
                }
            }
        }
    }
    text
}

pub struct ChatMlTranscriptConfig<'a, D: DialectRenderer> {
    pub bos: Option<&'a str>,
    pub tool_result_role: &'a str,
    pub dialect: &'a D,
}

pub fn render_chat_ml_transcript<D: DialectRenderer>(
    messages: &[Message],
    options: DialectRenderOptions<'_>,
    config: ChatMlTranscriptConfig<'_, D>,
) -> Result<String, D::Error> {
    if messages.is_empty() {
        return Ok(String::new());
    }
    let mut out = config.bos.unwrap_or("").to_owned();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        match message {
            Message::Assistant(message) => {
                let parts = assistant_transcript_parts(message);
                let body = format!(
                    "{}{}{}",
                    config.dialect.render_thinking(&parts.thinking),
                    parts.text,
                    config.dialect.render_assistant_tool_calls(&parts.tool_calls, options)?
                );
                out.push_str(&chat_ml_turn("assistant", &body));
                index += 1;
            }
            Message::ToolResult(_) => {
                let (results, next) = collect_tool_result_run(messages, index);
                out.push_str(&chat_ml_turn(
                    config.tool_result_role,
                    &config.dialect.render_tool_results(&results, options),
                ));
                index = next;
            }
            Message::User(message) => {
                out.push_str(&chat_ml_turn("user", &message_content_text(&message.content)));
                index += 1;
            }
            Message::Developer(message) => {
                out.push_str(&chat_ml_turn("system", &message_content_text(&message.content)));
                index += 1;
            }
        }
    }
    Ok(out)
}

pub fn render_legacy_text_transcript<D: DialectRenderer>(
    messages: &[Message],
    options: DialectRenderOptions<'_>,
    dialect: &D,
) -> Result<String, D::Error> {
    let mut out = String::new();
    let mut index = 0;
    while let Some(message) = messages.get(index) {
        match message {
            Message::Assistant(message) => {
                let parts = assistant_transcript_parts(message);
                out.push_str(&format!(
                    "\n\nAssistant: {}{}{}",
                    dialect.render_thinking(&parts.thinking),
                    parts.text,
                    dialect.render_assistant_tool_calls(&parts.tool_calls, options)?
                ));
                index += 1;
            }
            Message::ToolResult(_) => {
                let (results, next) = collect_tool_result_run(messages, index);
                out.push_str(&format!("\n\nHuman: {}", dialect.render_tool_results(&results, options)));
                index = next;
            }
            Message::User(message) => {
                out.push_str(&format!("\n\nHuman: {}", message_content_text(&message.content)));
                index += 1;
            }
            Message::Developer(message) => {
                let text = message_content_text(&message.content);
                if !text.is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(&text);
                }
                index += 1;
            }
        }
    }
    Ok(out)
}

fn is_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, 9 | 10 | 11 | 12 | 13 | 32)
}
fn trim_ascii_start(text: &str, mut start: usize, end: usize) -> usize {
    while start < end && is_ascii_whitespace(text.as_bytes()[start]) {
        start += 1;
    }
    start
}
fn trim_ascii_end(text: &str, start: usize, mut end: usize) -> usize {
    while end > start && is_ascii_whitespace(text.as_bytes()[end - 1]) {
        end -= 1;
    }
    end
}
fn find_delimited_thinking_close(open: &str, close: &str, text: &str, start: usize, end: usize) -> Option<usize> {
    let mut depth = 1;
    let mut cursor = start;
    while cursor < end {
        let next_close = cursor + text[cursor..].find(close)?;
        if next_close >= end {
            return None;
        }
        if let Some(next_open) = text[cursor..].find(open).map(|index| cursor + index)
            && next_open < next_close
        {
            depth += 1;
            cursor = next_open + open.len();
            continue;
        }
        depth -= 1;
        if depth == 0 {
            return Some(next_close);
        }
        cursor = next_close + close.len();
    }
    None
}
fn unwrap_delimited_thinking(open: &str, close: &str, text: &str) -> String {
    let end = trim_ascii_end(text, 0, text.len());
    let mut cursor = trim_ascii_start(text, 0, end);
    if cursor >= end || !text[cursor..].starts_with(open) {
        return text.into();
    }
    let mut segments = Vec::new();
    while cursor < end {
        if !text[cursor..].starts_with(open) {
            return text.into();
        }
        let inner_start = cursor + open.len();
        let Some(inner_end) = find_delimited_thinking_close(open, close, text, inner_start, end) else {
            return text.into();
        };
        let trimmed_end = trim_ascii_end(text, inner_start, inner_end);
        let trimmed_start = trim_ascii_start(text, inner_start, trimmed_end);
        segments.push(unwrap_delimited_thinking(open, close, &text[trimmed_start..trimmed_end]));
        cursor = trim_ascii_start(text, inner_end + close.len(), end);
    }
    segments.join("\n")
}
pub fn render_delimited_thinking(open: &str, close: &str, text: &str) -> String {
    if text.is_empty() {
        String::new()
    } else {
        format!("{open}\n{}\n{close}", unwrap_delimited_thinking(open, close, text))
    }
}

pub fn chat_ml_turn(role: &str, body: &str) -> String {
    format!("<|im_start|>{role}\n{body}<|im_end|>\n")
}
pub fn kimi_turn(role: &str, name: &str, body: &str) -> String {
    format!("<|im_{role}|>{name}<|im_middle|>{body}<|im_end|>")
}
pub fn gemma_turn(role: &str, body: &str) -> String {
    format!("<|turn>{role}\n{body}<turn|>")
}
pub fn gemini_turn(role: &str, body: &str) -> String {
    format!("<start_of_turn>{role}\n{body}<end_of_turn>\n")
}
pub fn join_user_bodies(left: &str, right: &str) -> String {
    if left.is_empty() {
        right.into()
    } else if right.is_empty() {
        left.into()
    } else {
        format!("{left}\n{right}")
    }
}
