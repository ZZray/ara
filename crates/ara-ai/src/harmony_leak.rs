//! Fixed OMP `packages/ai/src/utils/harmony-leak.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d. Pure helpers only: callers own
//! model gates, transport-copy escaping, tool parse boundaries and recovery.
//! Offsets/lengths are JavaScript UTF-16 code units, not UTF-8 bytes.
//!
//! MIT License
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//!
//! Permission is hereby granted, free of charge, to any person obtaining a copy
//! of this software and associated documentation files (the "Software"), to deal
//! in the Software without restriction, including without limitation the rights
//! to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
//! copies of the Software, and to permit persons to whom the Software is
//! furnished to do so, subject to the following conditions:
//!
//! The above copyright notice and this permission notice shall be included in
//! all copies or substantial portions of the Software.
//!
//! THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
//! IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
//! FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
//! AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
//! LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
//! FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS
//! IN THE SOFTWARE.

use crate::{AssistantBlock, AssistantMessage, Model, StopReason, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarmonySignalClass {
    H,
    M,
    C,
    G,
    S,
    B,
    R,
    T,
}

impl HarmonySignalClass {
    fn label(self) -> &'static str {
        match self {
            Self::H => "H",
            Self::M => "M",
            Self::C => "C",
            Self::G => "G",
            Self::S => "S",
            Self::B => "B",
            Self::R => "R",
            Self::T => "T",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmonySurface {
    AssistantText,
    AssistantThinking,
    ToolArg,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarmonySignal {
    pub classes: Vec<HarmonySignalClass>,
    /// UTF-16 offset, as in the fixed Native regex match index.
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyDetection {
    pub surface: HarmonySurface,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub signals: Vec<HarmonySignal>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HarmonyDetectionOptions<'a> {
    /// Structurally valid tool-input parse end, in UTF-16 code units.
    pub parsed_end: Option<usize>,
    pub content_index: Option<usize>,
    pub tool_name: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarmonyAuditAction {
    TruncateResume,
    AbortRetry,
    Escalated,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarmonyAuditEvent {
    pub action: HarmonyAuditAction,
    pub surface: HarmonySurface,
    pub signal: String,
    pub retry_n: usize,
    pub model: String,
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    pub removed_len: usize,
    pub removed_sha8: String,
    pub removed_preview: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_blob: Option<String>,
}

pub struct HarmonyAuditParams<'a> {
    pub action: HarmonyAuditAction,
    pub detection: &'a HarmonyDetection,
    pub model: &'a Model,
    pub retry_n: usize,
    pub removed: &'a str,
    /// The Host supplies its strict ARA_HARMONY_DEBUG=1 snapshot. The helper
    /// reads no environment and preserves the Native opt-in blob branch.
    pub debug_removed_blob: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HarmonyRecoveredToolCall {
    pub message: AssistantMessage,
    pub removed: String,
}

const LEAK_SPELLINGS: [&str; 6] = ["<|start|>", "<|end|>", "<|channel|>", "<|message|>", "<|call|>", "<|return|>"];
const CONTROL_TOKENS: [&str; 7] = ["start", "end", "message", "channel", "constrain", "return", "call"];
const CHANNELS: [&str; 7] = ["analysis", "commentary", "assistant", "user", "system", "developer", "tool"];
const GLITCHES: [&str; 5] = ["changedFiles", "RTLU", "Jsii_commentary", "Jsii", "Japgolly"];
const MARKER: &str = "to=functions.";
const RECOVERY_SENTINEL: &str = "\n*** Abort\n";

/// Callers gate the target and escape only a transport copy, never history.
pub fn escape_harmony_control_tokens(text: &str) -> String {
    escape_control_tokens(text, false)
}

/// Double backslashes keep JSON documents parseable after decoding the inert
/// spelling. Malformed documents receive the same best-effort substitution.
pub fn escape_harmony_control_tokens_in_json(text: &str) -> String {
    escape_control_tokens(text, true)
}

fn escape_control_tokens(text: &str, json_document: bool) -> String {
    let mut result = text.to_owned();
    for token in CONTROL_TOKENS {
        let from = format!("<|{token}|>");
        let slash = if json_document { "\\\\" } else { "\\" };
        result = result.replace(&from, &format!("<{slash}|{token}{slash}|>"));
    }
    result
}

/// Pass the final materialized Native model spec. Fixed source checks only
/// identity.class == gpt-oss; it does not infer from IDs or endpoints.
pub fn is_harmony_dialect_model(model_spec: &Value) -> bool {
    model_spec.pointer("/identity/class").and_then(Value::as_str) == Some("gpt-oss")
}

pub fn is_harmony_leak_mitigation_target(model_spec: &Value) -> bool {
    model_spec.pointer("/compat/harmonyLeakMitigation").and_then(Value::as_bool) == Some(true)
}

pub fn signal_list_label(signals: &[HarmonySignal]) -> String {
    let mut labels = Vec::new();
    for signal in signals {
        let label = signal.classes.iter().map(|class| class.label()).collect::<Vec<_>>().join("+");
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    if labels.is_empty() { "none".into() } else { labels.join(",") }
}

/// H alone or M plus a co-signal trips. Tool arguments additionally require
/// T; without a parse-end boundary that data surface remains inert.
pub fn detect_harmony_leak(
    text: &str,
    surface: HarmonySurface,
    options: HarmonyDetectionOptions<'_>,
) -> Option<HarmonyDetection> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let fences = fence_ranges(&units);
    let mut signals = Vec::new();
    for start in 0..units.len() {
        if units[start] != u16::from(b'<') || inside_fence(&fences, start) {
            continue;
        }
        for spelling in LEAK_SPELLINGS {
            if starts_ascii(&units, start, spelling) {
                signals.push(HarmonySignal {
                    classes: vec![HarmonySignalClass::H],
                    start,
                    end: start + spelling.len(),
                    text: spelling.to_owned(),
                });
            }
        }
    }
    for (start, end) in marker_matches(&units, true, true) {
        if inside_fence(&fences, start) {
            continue;
        }
        let mut classes = vec![HarmonySignalClass::M];
        let adjacent = window(&units, start.saturating_sub(64), end.saturating_add(16));
        let near = window(&units, start.saturating_sub(16), end.saturating_add(16));
        let forward = window(&units, start, start.saturating_add(240));
        if channel_adjacency(adjacent) {
            classes.push(HarmonySignalClass::C);
        }
        if glitch_adjacency(near) {
            classes.push(HarmonySignalClass::G);
        }
        if script_mismatch(&units, start, end) {
            classes.push(HarmonySignalClass::S);
        }
        if body_cascade(forward) {
            classes.push(HarmonySignalClass::B);
        }
        if fake_result(forward) {
            classes.push(HarmonySignalClass::R);
        }
        if options.parsed_end.is_some_and(|parsed_end| start >= parsed_end) {
            classes.push(HarmonySignalClass::T);
        }
        if classes.len() > 1 {
            signals.push(HarmonySignal { classes, start, end, text: utf16_text(&units[start..end]) });
        }
    }
    if signals.is_empty()
        || surface == HarmonySurface::ToolArg && !signals.iter().any(|s| s.classes.contains(&HarmonySignalClass::T))
    {
        return None;
    }
    signals.sort_by_key(|s| (s.start, s.end));
    Some(HarmonyDetection {
        surface,
        content_index: options.content_index,
        tool_name: options.tool_name.map(str::to_owned),
        tool_call_id: options.tool_call_id.map(str::to_owned),
        signals,
    })
}

/// The callback must return a UTF-16 boundary in get_tool_argument_text's
/// selected input. Omitting it matches the safe Native Core consumer.
pub type HarmonyToolArgParseEnd<'a> = dyn Fn(&ToolCall) -> Option<usize> + 'a;

pub fn detect_harmony_leak_in_assistant_message(
    message: &AssistantMessage,
    tool_arg_parse_end: Option<&HarmonyToolArgParseEnd<'_>>,
) -> Option<HarmonyDetection> {
    for (index, block) in message.content.iter().enumerate() {
        let (text, surface, tool_name, tool_call_id, parsed_end) = match block {
            AssistantBlock::Text(block) => (block.text.clone(), HarmonySurface::AssistantText, None, None, None),
            AssistantBlock::Thinking(block) => {
                (block.thinking.clone(), HarmonySurface::AssistantThinking, None, None, None)
            }
            AssistantBlock::ToolCall(call) => {
                let Some(text) = get_tool_argument_text(call) else {
                    continue;
                };
                (
                    text,
                    HarmonySurface::ToolArg,
                    Some(call.name.as_str()),
                    Some(call.id.as_str()),
                    tool_arg_parse_end.and_then(|parse| parse(call)),
                )
            }
            AssistantBlock::RedactedThinking { .. } | AssistantBlock::Image(_) => continue,
        };
        if let Some(detection) = detect_harmony_leak(
            &text,
            surface,
            HarmonyDetectionOptions { parsed_end, content_index: Some(index), tool_name, tool_call_id },
        ) {
            return Some(detection);
        }
    }
    None
}

pub fn recover_harmony_tool_call(
    message: &AssistantMessage,
    detection: &HarmonyDetection,
) -> Option<HarmonyRecoveredToolCall> {
    if detection.surface != HarmonySurface::ToolArg {
        return None;
    }
    let AssistantBlock::ToolCall(call) = message.content.get(detection.content_index?)? else {
        return None;
    };
    let input = call.arguments.get("input")?.as_str()?;
    match call.name.as_str() {
        "edit" if input.trim_start_matches(js_space_char).starts_with('@') => {}
        "eval" => {}
        _ => return None,
    }
    let offset = detection.signals.first()?.start;
    let (clean, removed) = truncate_at_line(input, offset)?;
    let mut call = call.clone();
    call.arguments.insert("input".into(), Value::String(clean));
    let mut clean_message = message.clone();
    clean_message.content = vec![AssistantBlock::ToolCall(call)];
    clean_message.provider_payload = None;
    clean_message.stop_reason = StopReason::ToolUse;
    clean_message.error_message = None;
    Some(HarmonyRecoveredToolCall { message: clean_message, removed })
}

pub fn extract_harmony_removed(message: &AssistantMessage, detection: &HarmonyDetection) -> String {
    let Some(block) = detection.content_index.and_then(|index| message.content.get(index)) else {
        return String::new();
    };
    let text = match block {
        AssistantBlock::Text(block) => block.text.clone(),
        AssistantBlock::Thinking(block) => block.thinking.clone(),
        AssistantBlock::ToolCall(call) => get_tool_argument_text(call).unwrap_or_default(),
        AssistantBlock::RedactedThinking { .. } | AssistantBlock::Image(_) => return String::new(),
    };
    let units: Vec<u16> = text.encode_utf16().collect();
    let start = detection.signals.first().map_or(0, |s| s.start).min(units.len());
    utf16_text(&units[start..])
}

pub fn create_harmony_audit_event(params: HarmonyAuditParams<'_>) -> HarmonyAuditEvent {
    let digest = ring::digest::digest(&ring::digest::SHA512_256, params.removed.as_bytes());
    let removed_sha8 = digest.as_ref()[..4].iter().map(|byte| format!("{byte:02x}")).collect();
    HarmonyAuditEvent {
        action: params.action,
        surface: params.detection.surface,
        signal: signal_list_label(&params.detection.signals),
        retry_n: params.retry_n,
        model: params.model.id.clone(),
        provider: params.model.provider.clone(),
        tool_name: params.detection.tool_name.clone(),
        removed_len: params.removed.encode_utf16().count(),
        removed_sha8,
        removed_preview: redacted_junk_preview(params.removed),
        removed_blob: params.debug_removed_blob.then(|| params.removed.to_owned()),
    }
}

fn utf16_text(units: &[u16]) -> String {
    String::from_utf16_lossy(units)
}
fn starts_ascii(units: &[u16], start: usize, pattern: &str) -> bool {
    units
        .get(start..start.saturating_add(pattern.len()))
        .is_some_and(|part| part.iter().copied().eq(pattern.bytes().map(u16::from)))
}
fn window(units: &[u16], start: usize, end: usize) -> &[u16] {
    &units[start.min(units.len())..end.min(units.len())]
}
fn ascii_word(unit: u16) -> bool {
    matches!(unit, 0x30..=0x39 | 0x41..=0x5a | 0x5f | 0x61..=0x7a)
}
fn word_start(unit: u16) -> bool {
    matches!(unit, 0x41..=0x5a | 0x5f | 0x61..=0x7a)
}
fn js_space(unit: u16) -> bool {
    matches!(unit, 0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028..=0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff)
}
fn js_space_char(ch: char) -> bool {
    u16::try_from(u32::from(ch)).is_ok_and(js_space)
}

/// MARKER_RE uses an ASCII initial identifier and boundary; B/R deliberately
/// use \w+ without that initial/boundary rule, exactly as Native.
fn marker_end(units: &[u16], start: usize, initial_letter: bool, boundary: bool) -> Option<usize> {
    let word = start + MARKER.len();
    if !starts_ascii(units, start, MARKER)
        || boundary && start > 0 && ascii_word(units[start - 1])
        || !units.get(word).is_some_and(|&u| if initial_letter { word_start(u) } else { ascii_word(u) })
    {
        return None;
    }
    let mut end = word + 1;
    while end < units.len() && ascii_word(units[end]) {
        end += 1;
    }
    Some(end)
}

fn marker_matches(units: &[u16], initial_letter: bool, boundary: bool) -> Vec<(usize, usize)> {
    let mut result = Vec::new();
    let mut start = 0;
    while start < units.len() {
        if let Some(end) = marker_end(units, start, initial_letter, boundary) {
            result.push((start, end));
            start = end;
        } else {
            start += 1;
        }
    }
    result
}

fn fence_ranges(units: &[u16]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence_start = None;
    let mut line_start = 0;
    loop {
        let newline = units[line_start..].iter().position(|&u| u == 10).map(|n| line_start + n);
        let line_end = newline.unwrap_or(units.len());
        let mut leading = line_start;
        while leading < line_end && js_space(units[leading]) {
            leading += 1;
        }
        if starts_ascii(&units[..line_end], leading, "```") || starts_ascii(&units[..line_end], leading, "~~~") {
            if let Some(start) = fence_start.take() {
                ranges.push((start, line_end));
            } else {
                fence_start = Some(line_start);
            }
        }
        let Some(newline) = newline else {
            break;
        };
        line_start = newline + 1;
    }
    if let Some(start) = fence_start {
        ranges.push((start, units.len()));
    }
    ranges
}
fn inside_fence(ranges: &[(usize, usize)], position: usize) -> bool {
    for &(start, end) in ranges {
        if position >= start && position < end {
            return true;
        }
        if start > position {
            break;
        }
    }
    false
}

fn channel_adjacency(units: &[u16]) -> bool {
    for start in 0..units.len() {
        if start > 0 && ascii_word(units[start - 1]) {
            continue;
        }
        for channel in CHANNELS {
            if !starts_ascii(units, start, channel) {
                continue;
            }
            let mut next = start + channel.len();
            let whitespace = next;
            while next < units.len() && js_space(units[next]) {
                next += 1;
            }
            if next > whitespace && starts_ascii(units, next, MARKER) {
                return true;
            }
        }
    }
    false
}
fn glitch_adjacency(units: &[u16]) -> bool {
    for start in 0..units.len() {
        if start > 0 && ascii_word(units[start - 1]) {
            continue;
        }
        for glitch in GLITCHES {
            let end = start + glitch.len();
            if starts_ascii(units, start, glitch) && (end == units.len() || !ascii_word(units[end])) {
                return true;
            }
        }
    }
    false
}
fn script_unit(unit: u16) -> bool {
    matches!(unit, 0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff | 0x0400..=0x04ff
        | 0x0e00..=0x0e7f | 0x10a0..=0x10ff | 0x0530..=0x058f | 0x0c80..=0x0cff
        | 0x0c00..=0x0c7f | 0x0900..=0x097f | 0x0600..=0x06ff | 0x0d00..=0x0d7f)
}
fn script_mismatch(units: &[u16], start: usize, end: usize) -> bool {
    let near = window(units, start.saturating_sub(32), end.saturating_add(32));
    if !near.windows(2).any(|pair| pair.iter().all(|&u| script_unit(u))) {
        return false;
    }
    let surrounding = window(units, start.saturating_sub(200), end.saturating_add(200));
    !surrounding.is_empty()
        && surrounding.iter().filter(|&&u| u < 128).count() as f64 / surrounding.len() as f64 >= 0.85
}
fn body_cascade(units: &[u16]) -> bool {
    // The complete regex can retry at a prefix nested inside a failed outer
    // match. Unlike MARKER_RE.matchAll, these starts must permit overlap.
    for start in 0..units.len() {
        let Some(end) = marker_end(units, start, false, false) else {
            continue;
        };
        let mut next = end;
        while next < units.len() && js_space(units[next]) {
            next += 1;
        }
        if next == end || !starts_ascii(units, next, "code") {
            continue;
        }
        let after = next + 4;
        if after < units.len() && ascii_word(units[after]) {
            continue;
        }
        for position in after..=after.saturating_add(200).min(units.len()) {
            if starts_ascii(units, position, MARKER) {
                return true;
            }
        }
    }
    false
}
fn fake_result(units: &[u16]) -> bool {
    for start in 0..units.len() {
        let Some(end) = marker_end(units, start, false, false) else {
            continue;
        };
        // \w+ may backtrack: code_output can start within the same identifier.
        for position in start + MARKER.len() + 1..=end.saturating_add(80).min(units.len()) {
            if !starts_ascii(units, position, "code_output") {
                continue;
            }
            let mut next = position + "code_output".len();
            while next < units.len() && js_space(units[next]) {
                if units[next] == 10 && starts_ascii(units, next + 1, "Cell") {
                    let mut digits = next + 5;
                    let before_space = digits;
                    while digits < units.len() && js_space(units[digits]) {
                        digits += 1;
                    }
                    if digits > before_space {
                        let begin = digits;
                        while digits < units.len() && matches!(units[digits], 0x30..=0x39) {
                            digits += 1;
                        }
                        if digits > begin && units.get(digits) == Some(&58) {
                            return true;
                        }
                    }
                }
                next += 1;
            }
        }
    }
    false
}

fn get_tool_argument_text(call: &ToolCall) -> Option<String> {
    if let Some(input) = call.arguments.get("input").and_then(Value::as_str) {
        return Some(input.to_owned());
    }
    serde_json::to_string(&call.arguments).ok()
}
fn truncate_at_line(input: &str, offset: usize) -> Option<(String, String)> {
    if offset == 0 {
        return None;
    }
    let units: Vec<u16> = input.encode_utf16().collect();
    let line_start = units[..offset.min(units.len())].iter().rposition(|&u| u == 10)? + 1;
    let head = utf16_text(&units[..line_start]);
    let head = head.trim_end_matches(js_space_char);
    if head.is_empty() {
        return None;
    }
    Some((format!("{head}{RECOVERY_SENTINEL}"), utf16_text(&units[line_start..])))
}
fn redacted_junk_preview(text: &str) -> String {
    let units: Vec<u16> = text.encode_utf16().take(64).collect();
    let mut result = String::new();
    let mut offset = 0;
    while offset < units.len() {
        if let Some(end) = marker_end(&units, offset, true, false) {
            result.push_str(&utf16_text(&units[offset..end]));
            offset = end;
            continue;
        }
        let token = CHANNELS.into_iter().chain(GLITCHES).find(|token| starts_ascii(&units, offset, token));
        if let Some(token) = token {
            result.push_str(token);
            offset += token.len();
            continue;
        }
        let unit = units[offset];
        if script_unit(unit)
            || js_space(unit)
            || matches!(unit, 0x3011 | 0x3010 | 0x201d | 0x201c | 0x2026 | 0x300d | 0x300c | 0x3001 | 0x3002)
        {
            result.push(char::from_u32(u32::from(unit)).expect("kept preview unit is a BMP scalar"));
        } else {
            result.push('·');
        }
        offset += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TextContent, ThinkingContent};
    use serde_json::json;

    #[test]
    fn harmony_helper_contract_group() {
        let defaults = HarmonyDetectionOptions::default();
        let detect = |text: &str| detect_harmony_leak(text, HarmonySurface::AssistantText, defaults);
        for (text, label) in [
            ("<|channel|>", "H"),
            ("analysis to=functions.x", "M+C"),
            ("RTLU to=functions.x", "M+G"),
            ("to=functions.x code to=functions.y", "M+B"),
            ("to=functions.x code_output\nCell 2: output", "M+R"),
        ] {
            assert_eq!(signal_list_label(&detect(text).unwrap().signals), label, "{text}");
        }
        for spelling in LEAK_SPELLINGS {
            assert_eq!(signal_list_label(&detect(spelling).unwrap().signals), "H");
        }
        let all_classes = detect_harmony_leak(
            "RTLU中文 analysis to=functions.x code to=functions.y code_output\nCell 1:",
            HarmonySurface::AssistantText,
            HarmonyDetectionOptions { parsed_end: Some(0), ..defaults },
        )
        .unwrap();
        assert_eq!(signal_list_label(&all_classes.signals[..1]), "M+C+G+S+B+R+T");
        for text in [
            "to=functions.x",
            "to=functions.9bad RTLU",
            "ato=functions.x RTLU",
            "<|constrain|>",
            "```\nanalysis to=functions.x <|start|>\n```",
            "  ~~~\n<|message|>",
            "``` <|end|>\n~~~",
        ] {
            assert!(detect(text).is_none(), "clean: {text}");
        }
        assert_eq!(signal_list_label(&detect("éto=functions.x RTLU").unwrap().signals), "M+G");
        assert_eq!(signal_list_label(&detect("éRTLU to=functions.x").unwrap().signals), "M+G");
        for text in
            ["xRTLU to=functions.x", "RTLU_ to=functions.x", "xanalysis to=functions.x", "analysis_to=functions.x"]
        {
            assert!(detect(text).is_none(), "ASCII boundary: {text}");
        }
        assert!(detect("analysis\u{0085}to=functions.x").is_none());
        assert_eq!(signal_list_label(&detect("analysis\u{feff}to=functions.x").unwrap().signals), "M+C");
        assert!(detect("say ``` <|start|>").is_some());
        let after_fence = detect("```\n<|start|>\n~~~\n<|end|>").unwrap();
        assert_eq!(after_fence.signals.len(), 1);
        assert_eq!(after_fence.signals[0].text, "<|end|>");
        for scripts in
            ["中日", "\u{3400}\u{3401}", "\u{f900}\u{f901}", "Аб", "กข", "აბ", "ԱԲ", "ಅಆ", "అఆ", "अआ", "اب", "അആ"]
        {
            let text = format!("{} {scripts} to=functions.x {}", "a".repeat(200), "a".repeat(200));
            assert_eq!(signal_list_label(&detect(&text).unwrap().signals), "M+S");
        }
        assert!(detect(&format!("{} to=functions.x", "中文".repeat(100))).is_none());
        assert_eq!(signal_list_label(&detect("中文日 to=functions.x  ").unwrap().signals), "M+S"); // 17/20 = .85
        assert!(detect("中文日中 to=functions.x  ").is_none());
        assert!(detect("中 to=functions.x").is_none());
        for (gap, leaks) in [(200, true), (201, false)] {
            assert_eq!(detect(&format!("to=functions.x code{}to=functions.", ".".repeat(gap))).is_some(), leaks);
        }
        for (gap, leaks) in [(80, true), (81, false)] {
            assert_eq!(detect(&format!("to=functions.x{}code_output\nCell 1:", ".".repeat(gap))).is_some(), leaks);
        }
        for (gap, leaks) in [(100, true), (101, false)] {
            assert_eq!(detect(&format!("to=functions.x code{}to=functions.", "😀".repeat(gap))).is_some(), leaks);
        }
        for (gap, leaks) in [(40, true), (41, false)] {
            assert_eq!(detect(&format!("to=functions.x{}code_output\nCell 1:", "😀".repeat(gap))).is_some(), leaks);
        }
        assert_eq!(
            signal_list_label(&detect("to=functions.to=functions.x code to=functions.").unwrap().signals),
            "M+B"
        );
        assert_eq!(
            signal_list_label(
                &detect(&format!("to=functions.to=functions.x{}code_output\nCell 1:", ".".repeat(80))).unwrap().signals
            ),
            "M+R"
        );
        assert!(detect(&format!("to=functions.to=functions.x{}code_output\nCell 1:", ".".repeat(81))).is_none());
        assert_eq!(signal_list_label(&detect("to=functions.xcode_output\nCell 1:").unwrap().signals), "M+R");
        let unicode = "😀<|start|> analysis to=functions.x";
        let d = detect(unicode).unwrap();
        assert_eq!((d.signals[0].start, d.signals[0].end), (2, 11));
        assert_eq!(signal_list_label(&d.signals), "H,M+C");
        assert_eq!(signal_list_label(&[d.signals[0].clone(), d.signals[0].clone()]), "H");
        assert_eq!(signal_list_label(&[]), "none");
        for text in ["<|start|>", "analysis to=functions.x", "RTLU to=functions.x", "中日 to=functions.x"] {
            assert!(detect_harmony_leak(text, HarmonySurface::ToolArg, defaults).is_none());
        }
        let t = detect_harmony_leak(
            "<|start|> to=functions.x",
            HarmonySurface::ToolArg,
            HarmonyDetectionOptions { parsed_end: Some(10), ..defaults },
        )
        .unwrap();
        assert_eq!(signal_list_label(&t.signals), "H,M+T");
        assert!(
            detect_harmony_leak(
                "to=functions.x",
                HarmonySurface::ToolArg,
                HarmonyDetectionOptions { parsed_end: Some(1), ..defaults }
            )
            .is_none()
        );
        assert_eq!(
            signal_list_label(
                &detect_harmony_leak(
                    "to=functions.x",
                    HarmonySurface::ToolArg,
                    HarmonyDetectionOptions { parsed_end: Some(0), ..defaults }
                )
                .unwrap()
                .signals
            ),
            "M+T"
        );
        let original = "😀<|start|> <|end|> <|message|> <|channel|> <|constrain|> <|return|> <|call|>".to_owned();
        let expected = r"😀<\|start\|> <\|end\|> <\|message\|> <\|channel\|> <\|constrain\|> <\|return\|> <\|call\|>";
        assert_eq!(escape_harmony_control_tokens(&original), expected);
        assert_eq!(escape_harmony_control_tokens("ordinary data"), "ordinary data");
        let document = json!({"data": original}).to_string();
        let escaped: Value = serde_json::from_str(&escape_harmony_control_tokens_in_json(&document)).unwrap();
        assert_eq!(escaped["data"], expected);
        assert_eq!(original, "😀<|start|> <|end|> <|message|> <|channel|> <|constrain|> <|return|> <|call|>");
        assert_eq!(escape_harmony_control_tokens_in_json("{bad <|call|>"), r"{bad <\\|call\\|>");
        assert!(is_harmony_dialect_model(&json!({"identity":{"class":"gpt-oss"}})));
        assert!(!is_harmony_dialect_model(
            &json!({"id":"gpt-5.4","requestModelId":"gpt-oss","identity":{"class":"openai"}})
        ));
        for value in [
            json!({}),
            json!({"compat":{"harmonyLeakMitigation":"true"}}),
            json!({"compat":{"harmonyLeakMitigation":false}}),
        ] {
            assert!(!is_harmony_leak_mitigation_target(&value));
        }
        assert!(is_harmony_leak_mitigation_target(&json!({"compat":{"harmonyLeakMitigation":true}})));

        let input = " \u{feff}@f\n😀line\nanalysis to=functions.edit";
        let mut message = AssistantMessage::empty("openai-responses", "fixture", "fixture");
        message.provider_payload = Some(json!({"opaque":"original"}));
        message.error_message = Some("original error".into());
        message.response_id = Some("retained-response".into());
        message.timestamp = 123;
        message.duration = Some(45);
        message.error_status = Some(429);
        message.content = vec![AssistantBlock::ToolCall(ToolCall {
            id: "c".into(),
            name: "edit".into(),
            arguments: json!({"input":input,"retained":1}).as_object().unwrap().clone(),
            intent: Some("intent".into()),
            ..Default::default()
        })];
        let original_message = message.clone();
        assert!(detect_harmony_leak_in_assistant_message(&message, None).is_none());
        let boundary = input.find("analysis").map(|byte| input[..byte].encode_utf16().count()).unwrap();
        let parse = |_: &ToolCall| Some(boundary);
        let detection = detect_harmony_leak_in_assistant_message(&message, Some(&parse)).unwrap();
        assert_eq!(detection.tool_name.as_deref(), Some("edit"));
        assert_eq!(detection.tool_call_id.as_deref(), Some("c"));
        let recovered = recover_harmony_tool_call(&message, &detection).unwrap();
        assert_eq!(recovered.message.content.len(), 1);
        assert_eq!(recovered.message.stop_reason, StopReason::ToolUse);
        assert!(recovered.message.provider_payload.is_none() && recovered.message.error_message.is_none());
        assert_eq!(recovered.message.response_id, message.response_id);
        assert_eq!(recovered.message.timestamp, 123);
        assert_eq!(recovered.message.duration, Some(45));
        assert_eq!(recovered.message.error_status, Some(429));
        assert_eq!(recovered.message.usage, message.usage);
        let call = recovered.message.tool_calls().next().unwrap();
        assert_eq!(call.arguments["input"], " \u{feff}@f\n😀line\n*** Abort\n");
        assert_eq!(call.arguments["retained"], 1);
        assert_eq!(call.intent.as_deref(), Some("intent"));
        assert_eq!(recovered.removed, "analysis to=functions.edit");
        assert_eq!(extract_harmony_removed(&message, &detection), "to=functions.edit");
        assert_eq!(message, original_message);
        for (name, input, recovers) in [
            ("eval", "print(1)\nanalysis to=functions.eval", true),
            ("edit", "*** Begin Patch\nanalysis to=functions.edit", false),
            ("write", "@f\nanalysis to=functions.write", false),
            ("edit", "@analysis to=functions.edit", false),
            ("eval", "\nanalysis to=functions.eval", false),
        ] {
            let mut variant = message.clone();
            let AssistantBlock::ToolCall(call) = &mut variant.content[0] else { unreachable!() };
            call.name = name.into();
            call.arguments["input"] = json!(input);
            let detect = detect_harmony_leak_in_assistant_message(&variant, Some(&|_| Some(0))).unwrap();
            assert_eq!(recover_harmony_tool_call(&variant, &detect).is_some(), recovers, "{name}/{input}");
        }
        let mut nonstring = message.clone();
        let AssistantBlock::ToolCall(call) = &mut nonstring.content[0] else { unreachable!() };
        call.arguments["input"] = json!(true);
        call.arguments.insert("data".into(), json!("analysis to=functions.x"));
        let serialized_detection = detect_harmony_leak_in_assistant_message(&nonstring, Some(&|_| Some(0))).unwrap();
        assert!(recover_harmony_tool_call(&nonstring, &serialized_detection).is_none());
        let mut scan = message.clone();
        scan.content.insert(
            0,
            AssistantBlock::Thinking(ThinkingContent { thinking: "<|channel|>".into(), thinking_signature: None }),
        );
        scan.content.insert(0, AssistantBlock::Text(TextContent { text: "ordinary".into(), text_signature: None }));
        let first = detect_harmony_leak_in_assistant_message(&scan, Some(&parse)).unwrap();
        assert_eq!(first.surface, HarmonySurface::AssistantThinking);
        assert_eq!(first.content_index, Some(1));
        assert!(recover_harmony_tool_call(&scan, &first).is_none());
        assert_eq!(extract_harmony_removed(&scan, &first), "<|channel|>");
        let mut absent = detection.clone();
        absent.content_index = Some(999);
        assert_eq!(extract_harmony_removed(&message, &absent), "");
        assert!(recover_harmony_tool_call(&message, &absent).is_none());
        absent.content_index = None;
        assert_eq!(extract_harmony_removed(&message, &absent), "");
        assert!(recover_harmony_tool_call(&message, &absent).is_none());
        absent.content_index = Some(0);
        absent.signals.clear();
        assert_eq!(extract_harmony_removed(&message, &absent), input);
        assert!(recover_harmony_tool_call(&message, &absent).is_none());
        let model = Model {
            id: "m".into(),
            api: "openai-responses".into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        };
        for debug in [false, true] {
            let audit = create_harmony_audit_event(HarmonyAuditParams {
                action: HarmonyAuditAction::AbortRetry,
                detection: &detection,
                model: &model,
                retry_n: 2,
                removed: "abc",
                debug_removed_blob: debug,
            });
            assert_eq!(audit.removed_sha8, "53048e26");
            assert_eq!(audit.removed_len, 3);
            assert_eq!(audit.removed_preview, "···");
            assert_eq!(audit.removed_blob.as_deref(), debug.then_some("abc"));
            assert_eq!(audit.retry_n, 2);
            assert_eq!((audit.model.as_str(), audit.provider.as_str()), ("m", "p"));
        }
        assert_eq!(redacted_junk_preview(&"😀".repeat(40)), "·".repeat(64));
        let unicode_audit = create_harmony_audit_event(HarmonyAuditParams {
            action: HarmonyAuditAction::Escalated,
            detection: &detection,
            model: &model,
            retry_n: 3,
            removed: "😀",
            debug_removed_blob: false,
        });
        assert_eq!(unicode_audit.removed_len, 2);
        assert_eq!(unicode_audit.removed_preview, "··");
        assert_eq!(
            redacted_junk_preview("SECRET analysis to=functions.eval RTLU 中文。"),
            "······ analysis to=functions.eval RTLU 中文。"
        );
    }
}
