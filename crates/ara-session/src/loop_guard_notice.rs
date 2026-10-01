//! Fixed OMP 596f2da `prompts/system/{thinking-loop-redirect,
//! gemini-tool-call-reminder}.md` and Session custom-message projection.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT license and permission notice: repository THIRD_PARTY_NOTICES.md.

use ara_ai::{DeveloperMessage, Message, UserContent, now_ms};
use serde_json::{Value, json};

pub const THINKING_LOOP_REDIRECT_TYPE: &str = "thinking-loop-redirect";
pub const GEMINI_TOOL_CALL_REMINDER_TYPE: &str = "gemini-tool-call-reminder";
pub const TOOL_CALL_LOOP_REDIRECT_TYPE: &str = "tool-call-loop-redirect";

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
}

impl LoopGuardNotice {
    pub fn thinking_loop() -> Self {
        Self { kind: NoticeKind::ThinkingLoop, timestamp: now_ms() }
    }

    pub fn gemini_headers(headers: usize) -> Self {
        Self { kind: NoticeKind::GeminiHeaders(headers), timestamp: now_ms() }
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
        Ok(Self { kind: NoticeKind::ToolCalls(details), timestamp: now_ms() })
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

    pub(crate) fn is_candidate(raw: &Value) -> bool {
        raw["type"] == "custom_message"
            && matches!(
                raw["customType"].as_str(),
                Some(THINKING_LOOP_REDIRECT_TYPE | GEMINI_TOOL_CALL_REMINDER_TYPE | TOOL_CALL_LOOP_REDIRECT_TYPE)
            )
    }

    pub(crate) fn from_entry(raw: &Value) -> Option<Self> {
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
        let notice = Self { kind, timestamp };
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

    /// Validate a Host-owned custom input without accepting arbitrary custom
    /// roles or content. Its original timestamp and fixed-template identity
    /// are retained by the journal adapter.
    pub fn from_event_message(message: &Value) -> Option<Self> {
        if message["role"] != "custom" {
            return None;
        }
        let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(message["timestamp"].as_i64()?)?
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        let mut raw = message.clone();
        raw["type"] = json!("custom_message");
        raw["timestamp"] = json!(timestamp);
        let notice = Self::from_entry(&raw)?;
        // Live Host inputs use only the native generated form; compatibility
        // for old persisted ARA entries does not admit legacy input content.
        (message["content"].as_str()? == notice.content()).then_some(notice)
    }
}

fn valid_tool_count(count: f64) -> bool {
    count.is_finite() && count >= 1.0 && count.fract() == 0.0
}
