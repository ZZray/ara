//! Fixed OMP 596f2da `prompts/system/{thinking-loop-redirect,
//! gemini-tool-call-reminder}.md` and Session custom-message projection.
//! Copyright (c) 2025-2026 Can Bölük; (c) 2026 Stencil Labs, Inc.
//! MIT license and permission notice: repository THIRD_PARTY_NOTICES.md.

use ara_ai::{DeveloperMessage, Message, UserContent, now_ms};
use serde_json::{Value, json};

pub const THINKING_LOOP_REDIRECT_TYPE: &str = "thinking-loop-redirect";
pub const GEMINI_TOOL_CALL_REMINDER_TYPE: &str = "gemini-tool-call-reminder";

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

#[derive(Clone, Debug, PartialEq, Eq)]
enum NoticeKind {
    ThinkingLoop,
    GeminiHeaders(usize),
}

/// Only fixed trusted templates can be constructed or restored. Error text,
/// streamed model content and detector diagnostics cannot acquire this role.
#[derive(Clone, Debug, PartialEq, Eq)]
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

    pub fn timestamp(&self) -> i64 {
        self.timestamp
    }

    pub fn custom_type(&self) -> &'static str {
        match self.kind {
            NoticeKind::ThinkingLoop => THINKING_LOOP_REDIRECT_TYPE,
            NoticeKind::GeminiHeaders(_) => GEMINI_TOOL_CALL_REMINDER_TYPE,
        }
    }

    fn content(&self) -> String {
        match self.kind {
            NoticeKind::ThinkingLoop => THINKING_LOOP_REDIRECT.into(),
            NoticeKind::GeminiHeaders(headers) => GEMINI_TOOL_CALL_REMINDER.replace("{{count}}", &headers.to_string()),
        }
    }

    pub fn model_message(&self) -> Message {
        Message::Developer(DeveloperMessage { content: UserContent::Text(self.content()), timestamp: self.timestamp })
    }

    pub fn event_message(&self) -> Value {
        let mut message = json!({
            "role":"custom", "customType":self.custom_type(), "content":self.content(),
            "display":false, "attribution":"agent", "timestamp":self.timestamp,
        });
        if let NoticeKind::GeminiHeaders(headers) = self.kind {
            message["details"] = json!({"headers":headers});
        }
        message
    }

    pub(crate) fn is_candidate(raw: &Value) -> bool {
        raw["type"] == "custom_message"
            && matches!(raw["customType"].as_str(), Some(THINKING_LOOP_REDIRECT_TYPE | GEMINI_TOOL_CALL_REMINDER_TYPE))
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
            _ => return None,
        };
        let timestamp = chrono::DateTime::parse_from_rfc3339(raw["timestamp"].as_str()?).ok()?.timestamp_millis();
        let notice = Self { kind, timestamp };
        (raw["content"].as_str()? == notice.content()).then_some(notice)
    }
}
