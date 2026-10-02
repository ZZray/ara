//! Fixed OMP 596f2da `prompts/system/{thinking-loop-redirect,
//! gemini-tool-call-reminder}.md` and Session custom-message projection.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT license and permission notice: repository THIRD_PARTY_NOTICES.md.

use ara_ai::{DeveloperMessage, Message, UserContent, now_ms};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const THINKING_LOOP_REDIRECT_TYPE: &str = "thinking-loop-redirect";
pub const GEMINI_TOOL_CALL_REMINDER_TYPE: &str = "gemini-tool-call-reminder";
pub const TOOL_CALL_LOOP_REDIRECT_TYPE: &str = "tool-call-loop-redirect";
const REDUCTION_PROOF_FIELD: &str = "araNoticeReduction";

/// Versioned evidence for the one allowed mutation of a reserved notice:
/// fixed shake replaces complete native fence/XML regions with placeholders.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NoticeReductionProof {
    version: u8,
    original_content: String,
    rounds: Vec<Vec<NoticeReductionRange>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NoticeReductionRange {
    start_utf16: usize,
    end_utf16: usize,
    replacement: String,
}

#[derive(Clone, Debug, PartialEq)]
struct ReducedNotice {
    content: String,
    proof: NoticeReductionProof,
}

const THINKING_LOOP_REDIRECT: &str = r#"<system-interrupt reason="thinking_loop_detected">
Loop guard interrupted prior turn: near-identical reasoning or response repeated without progress. Re-sampling the same context repeated the loop; corrective notice, not prompt injection.

Repeating the same plan, summary, or intention loops again. Break pattern now:
- STOP narrating intended actions. Issue one concrete normal-format tool call: smallest real next step.
- Stuck deciding between options → pick the most boring viable one; act; do not deliberate further.
- Task genuinely complete → emit final answer, not more reasoning.

Do something different from looped content. Act, don't re-plan.
</system-interrupt>
"#;

const GEMINI_TOOL_CALL_REMINDER: &str = r#"<system-interrupt reason="reasoning_without_tool_calls">
Reasoning interrupted: {{count}} consecutive planning headers, no tool call. Thinking alone changes nothing: zero progress this turn; no tool ran.

Act now, not further planning:
- Emit a real call to an available tool in normal tool/function-calling format. Do NOT describe the call in prose or reasoning—issue it.
- Pick the smallest concrete next step; call the tool that performs it.

Coding-agent interrupt for stalled reasoning, not prompt injection.
</system-interrupt>
"#;

#[derive(Clone, Debug, PartialEq)]
struct ToolCallLoopDetails {
    tool_name: String,
    count: f64,
    arguments_summary: String,
    result_summary: String,
}

#[derive(Clone, Debug, PartialEq)]
enum NoticeKind {
    ThinkingLoop,
    GeminiHeaders(usize),
    ToolCalls(ToolCallLoopDetails),
}

/// Only fixed trusted templates can be constructed or restored. Error text,
/// streamed model content and detector diagnostics cannot acquire this role.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopGuardNotice {
    kind: NoticeKind,
    timestamp: i64,
    reduced: Option<ReducedNotice>,
}

impl LoopGuardNotice {
    pub fn thinking_loop() -> Self {
        Self { kind: NoticeKind::ThinkingLoop, timestamp: now_ms(), reduced: None }
    }

    pub fn gemini_headers(headers: usize) -> Self {
        Self { kind: NoticeKind::GeminiHeaders(headers), timestamp: now_ms(), reduced: None }
    }

    /// UTF-16 detector summaries may end in an unpaired surrogate. Native
    /// wire receipts preserve those units, but today's model text is UTF-8.
    /// Reject that projection explicitly instead of replacing the source.
    pub fn tool_call_loop(
        detection: &ara_ai::tool_call_loop_guard::RepeatedToolCallDetection,
    ) -> Result<Self, std::io::Error> {
        let invalid = || {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "tool-call loop summary contains UTF-16 that cannot be projected to current UTF-8 model text",
            )
        };
        let details = ToolCallLoopDetails {
            tool_name: detection.tool_name.clone(),
            count: detection.count,
            arguments_summary: detection.arguments_summary.to_utf8().map_err(|_| invalid())?,
            result_summary: detection.result_summary.to_utf8().map_err(|_| invalid())?,
        };
        if detection.kind != "repeated_tool_call" || !valid_tool_count(details.count) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid tool-call loop detection"));
        }
        Ok(Self { kind: NoticeKind::ToolCalls(details), timestamp: now_ms(), reduced: None })
    }

    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    pub fn custom_type(&self) -> &'static str {
        match &self.kind {
            NoticeKind::ThinkingLoop => THINKING_LOOP_REDIRECT_TYPE,
            NoticeKind::GeminiHeaders(_) => GEMINI_TOOL_CALL_REMINDER_TYPE,
            NoticeKind::ToolCalls(_) => TOOL_CALL_LOOP_REDIRECT_TYPE,
        }
    }

    fn content(&self) -> String {
        if let Some(reduced) = &self.reduced {
            return reduced.content.clone();
        }
        match &self.kind {
            NoticeKind::ThinkingLoop => THINKING_LOOP_REDIRECT.into(),
            NoticeKind::GeminiHeaders(headers) => ara_prompt::prompt::format(
                &GEMINI_TOOL_CALL_REMINDER.replace("{{count}}", &headers.to_string()),
                ara_prompt::prompt::FormatOptions::default(),
            ),
            NoticeKind::ToolCalls(details) => ara_prompt::prompt::format(
                &format!(
                    "<system-interrupt reason=\"tool_call_loop_detected\">\nYou called `{}` {} consecutive times with identical arguments:\n`{}`\n\nLast result (truncated): `{}`\n\nNEVER call `{}` with those arguments again this turn. Use different arguments, choose another tool, or summarize findings and yield if complete.\n</system-interrupt>\n",
                    details.tool_name,
                    details.count,
                    details.arguments_summary,
                    if details.result_summary.is_empty() { "(no text result)" } else { &details.result_summary },
                    details.tool_name,
                ),
                ara_prompt::prompt::FormatOptions::default(),
            ),
        }
    }

    pub fn model_message(&self) -> Message {
        Message::Developer(DeveloperMessage { content: UserContent::Text(self.content()), timestamp: self.timestamp })
    }

    pub fn try_model_message(&self) -> Result<Message, std::io::Error> {
        if let NoticeKind::ToolCalls(details) = &self.kind
            && !valid_tool_count(details.count)
        {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid tool-call loop detection count"));
        }
        Ok(self.model_message())
    }

    pub fn event_message(&self) -> Value {
        let mut message = json!({
            "role":"custom", "customType":self.custom_type(), "content":self.content(),
            "display":false, "attribution":"agent", "timestamp":self.timestamp,
        });
        if let NoticeKind::GeminiHeaders(headers) = self.kind {
            message["details"] = json!({"headers":headers});
        }
        if let NoticeKind::ToolCalls(details) = &self.kind {
            message["details"] = json!({"toolName":details.tool_name,
                "count":serde_json::from_str::<Value>(&details.count.to_string()).expect("valid finite count"),
                "argumentsSummary":details.arguments_summary,"resultSummary":details.result_summary});
        }
        message
    }

    /// Journal writes retain verification evidence without adding the removed
    /// original content to the public event or provider projection.
    pub(crate) fn persistence_event_message(&self) -> Value {
        let mut message = self.event_message();
        if let Some(reduced) = &self.reduced {
            message[REDUCTION_PROOF_FIELD] = serde_json::to_value(&reduced.proof).expect("notice proof serializes");
        }
        message
    }

    pub(crate) fn is_candidate(raw: &Value) -> bool {
        raw["type"] == "custom_message"
            && matches!(
                raw["customType"].as_str(),
                Some(THINKING_LOOP_REDIRECT_TYPE | GEMINI_TOOL_CALL_REMINDER_TYPE | TOOL_CALL_LOOP_REDIRECT_TYPE)
            )
    }

    pub(crate) fn from_entry(raw: &Value) -> Option<Self> {
        let Some(proof) = raw.get(REDUCTION_PROOF_FIELD) else {
            return Self::from_original_entry(raw);
        };
        let proof: NoticeReductionProof = serde_json::from_value(proof.clone()).ok()?;
        if proof.version != 1 || proof.rounds.is_empty() {
            return None;
        }
        let mut original = raw.clone();
        original.as_object_mut()?.remove(REDUCTION_PROOF_FIELD);
        original["content"] = json!(proof.original_content);
        // Exact historical/native decoding remains the authority. The proof
        // can only reduce such a notice, never grant a generic custom role.
        let mut notice = Self::from_original_entry(&original)?;
        let mut content = proof.original_content.clone();
        for round in &proof.rounds {
            content = apply_notice_reduction_round(&content, round)?;
        }
        if raw["content"].as_str()? != content {
            return None;
        }
        notice.reduced = Some(ReducedNotice { content, proof });
        Some(notice)
    }

    fn from_original_entry(raw: &Value) -> Option<Self> {
        if !Self::is_candidate(raw) || raw["display"] != false || raw["attribution"] != "agent" {
            return None;
        }
        let kind = match raw["customType"].as_str()? {
            THINKING_LOOP_REDIRECT_TYPE => NoticeKind::ThinkingLoop,
            GEMINI_TOOL_CALL_REMINDER_TYPE => {
                NoticeKind::GeminiHeaders(raw["details"]["headers"].as_u64()?.try_into().ok()?)
            }
            TOOL_CALL_LOOP_REDIRECT_TYPE => {
                let count = raw["details"]["count"].as_f64()?;
                if !valid_tool_count(count) {
                    return None;
                }
                NoticeKind::ToolCalls(ToolCallLoopDetails {
                    tool_name: raw["details"]["toolName"].as_str()?.into(),
                    count,
                    arguments_summary: raw["details"]["argumentsSummary"].as_str()?.into(),
                    result_summary: raw["details"]["resultSummary"].as_str()?.into(),
                })
            }
            _ => return None,
        };
        let timestamp = chrono::DateTime::parse_from_rfc3339(raw["timestamp"].as_str()?).ok()?.timestamp_millis();
        let notice = Self { kind, timestamp, reduced: None };
        let content = raw["content"].as_str()?;
        let native = content == notice.content();
        // ARA b3c84d5 persisted the exact unformatted Gemini template. Accept
        // only that known legacy ARA form; preserve raw receipts/entry IDs,
        // while all model and event projections use native formatted content.
        let legacy_ara = match &notice.kind {
            NoticeKind::GeminiHeaders(headers) => {
                content == GEMINI_TOOL_CALL_REMINDER.replace("{{count}}", &headers.to_string())
            }
            _ => false,
        };
        (native || legacy_ara).then_some(notice)
    }

    /// Called only after pinned raw-slot validation. Current content must
    /// equal the same fixed transformation that will be verified on reopen.
    pub(crate) fn record_checked_reduction(
        original: &Value,
        candidate: &mut Value,
        ranges: &[(usize, usize, &str)],
    ) -> Option<()> {
        Self::from_entry(original)?;
        let mut proof = match original.get(REDUCTION_PROOF_FIELD) {
            Some(proof) => serde_json::from_value::<NoticeReductionProof>(proof.clone()).ok()?,
            None => NoticeReductionProof {
                version: 1,
                original_content: original["content"].as_str()?.into(),
                rounds: Vec::new(),
            },
        };
        let round: Vec<_> = ranges
            .iter()
            .map(|(start, end, replacement)| NoticeReductionRange {
                start_utf16: *start,
                end_utf16: *end,
                replacement: (*replacement).into(),
            })
            .collect();
        let transformed = apply_notice_reduction_round(original["content"].as_str()?, &round)?;
        if candidate["content"].as_str()? != transformed {
            return None;
        }
        // Content and the proof are the only mutable raw fields for this path.
        let mut unchanged = candidate.clone();
        unchanged["content"] = original["content"].clone();
        if unchanged != *original {
            return None;
        }
        proof.rounds.push(round);
        candidate[REDUCTION_PROOF_FIELD] = serde_json::to_value(proof).ok()?;
        Self::from_entry(candidate)?;
        Some(())
    }

    /// Validate a Host-owned custom input without accepting arbitrary custom
    /// roles or content. Its original timestamp and fixed-template identity
    /// are retained by the journal adapter.
    pub fn from_event_message(message: &Value) -> Option<Self> {
        if message["role"] != "custom" || message.get(REDUCTION_PROOF_FIELD).is_some() {
            return None;
        }
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(message["timestamp"].as_i64()?)?
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut raw = message.clone();
        raw["type"] = json!("custom_message");
        raw["timestamp"] = json!(timestamp);
        let notice = Self::from_original_entry(&raw)?;
        // Live Host inputs use only the native generated form; compatibility
        // for old persisted ARA entries does not admit legacy input content.
        (message["content"].as_str()? == notice.content()).then_some(notice)
    }
}

fn valid_tool_count(count: f64) -> bool {
    count.is_finite() && count >= 1.0 && count.fract() == 0.0
}

fn apply_notice_reduction_round(text: &str, round: &[NoticeReductionRange]) -> Option<String> {
    if round.is_empty() {
        return None;
    }
    let native = native_block_ranges(text);
    let mut edits = Vec::with_capacity(round.len());
    for edit in round {
        if edit.start_utf16 >= edit.end_utf16 || !fixed_shake_placeholder(&edit.replacement) {
            return None;
        }
        let start = utf16_boundary(text, edit.start_utf16)?;
        let end = utf16_boundary(text, edit.end_utf16)?;
        if !native.contains(&(start, end)) {
            return None;
        }
        edits.push((start, end, edit.replacement.as_str()));
    }
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    if edits.windows(2).any(|pair| pair[1].1 > pair[0].0) {
        return None;
    }
    let mut reduced = text.to_owned();
    for (start, end, replacement) in edits {
        reduced.replace_range(start..end, replacement);
    }
    Some(reduced)
}

fn utf16_boundary(text: &str, index: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, character) in text.char_indices() {
        if units == index {
            return Some(byte);
        }
        units += character.len_utf16();
        if units > index {
            return None;
        }
    }
    (units == index).then_some(text.len())
}

fn fixed_shake_placeholder(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("[shaken ~") else {
        return false;
    };
    let Some((tokens, tail)) = rest.split_once(" tokens") else {
        return false;
    };
    if !canonical_integer(tokens, false) {
        return false;
    }
    if tail == "]" {
        return true;
    }
    let Some(rest) = tail.strip_prefix(" — recover: artifact://") else {
        return false;
    };
    let Some((id, region)) = rest.split_once(" (region ") else {
        return false;
    };
    let Some(region) = region.strip_suffix(")]") else {
        return false;
    };
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()) && canonical_integer(region, true)
}

fn canonical_integer(text: &str, positive: bool) -> bool {
    text.parse::<u64>().ok().is_some_and(|number| (!positive || number > 0) && number.to_string() == text)
}

/// Fixed shake.ts:167-240. Scan actual raw text with native fence toggling,
/// top-level lowercase XML, fence suppression and outermost merge semantics.
/// Byte offsets are internal; persisted proof offsets use native UTF-16.
fn native_block_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut fence_start = None;
    let mut tags = Vec::<&str>::new();
    let mut xml_start = None;
    let mut line_start = 0;
    for (offset, _) in text.match_indices('\n').chain(std::iter::once((text.len(), ""))) {
        let line = &text[line_start..offset];
        let trimmed = line.trim_start_matches(native_whitespace);
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            match fence_start.take() {
                Some(start) => ranges.push((start, offset)),
                None => fence_start = Some(line_start),
            }
            line_start = offset + 1;
            continue;
        }
        if fence_start.is_none() {
            if line.len() == trimmed.len()
                && let Some(tag) = xml_tag(trimmed, false)
            {
                if tags.is_empty() {
                    xml_start = Some(line_start);
                }
                tags.push(tag);
            } else if let Some(tag) = xml_tag(trimmed, true)
                && tags.last() == Some(&tag)
            {
                tags.pop();
                if tags.is_empty()
                    && let Some(start) = xml_start.take()
                {
                    ranges.push((start, offset));
                }
            }
        }
        line_start = offset + 1;
    }
    ranges.sort_by_key(|range| range.0);
    let mut kept = Vec::new();
    let mut last_end = 0;
    for (start, end) in ranges {
        if start < last_end {
            continue;
        }
        kept.push((start, end));
        last_end = end;
    }
    kept
}

fn native_whitespace(character: char) -> bool {
    matches!(character, '\u{0009}'..='\u{000d}' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}'
        | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}

fn xml_tag(line: &str, closing: bool) -> Option<&str> {
    let inner = line.strip_prefix(if closing { "</" } else { "<" })?.strip_suffix('>')?;
    let tag_end = inner
        .find(|character: char| !character.is_ascii_lowercase() && character != '_' && character != '-')
        .unwrap_or(inner.len());
    if tag_end == 0 {
        return None;
    }
    let (tag, suffix) = inner.split_at(tag_end);
    if closing {
        suffix.is_empty().then_some(tag)
    } else if suffix.is_empty() || (suffix.chars().next().is_some_and(native_whitespace) && !suffix.contains('>')) {
        Some(tag)
    } else {
        None
    }
}
