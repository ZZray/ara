//! A directly invoked Skill prompt stored as one Session custom message.

use ara_ai::{Message, UserBlock, UserContent, UserMessage, now_ms};
use serde_json::{Value, json};

pub const SKILL_PROMPT_CUSTOM_TYPE: &str = "skill-prompt";

/// The user-authored Skill turn. `details` is host metadata, not model content.
#[derive(Clone, Debug, PartialEq)]
pub struct UserSkillPrompt {
    pub content: UserContent,
    pub details: Option<Value>,
    pub timestamp: i64,
}

impl UserSkillPrompt {
    pub fn new(content: UserContent, details: Option<Value>) -> Self {
        Self { content, details, timestamp: now_ms() }
    }

    /// Fixed OMP projects a user-invoked Skill into one user message. A string
    /// custom payload becomes one text block; details never reach the model.
    pub fn model_message(&self) -> Message {
        let content = match &self.content {
            UserContent::Text(text) => UserContent::Blocks(vec![UserBlock::text(text.clone())]),
            UserContent::Blocks(blocks) => UserContent::Blocks(blocks.clone()),
        };
        Message::User(UserMessage { content, synthetic: None, timestamp: self.timestamp })
    }

    /// Event payload for a host that displays the original custom message.
    pub fn event_message(&self) -> Value {
        let mut event = json!({
            "role": "custom",
            "customType": SKILL_PROMPT_CUSTOM_TYPE,
            "content": self.content,
            "display": true,
            "attribution": "user",
            "timestamp": self.timestamp,
        });
        if let Some(details) = &self.details {
            event["details"] = details.clone();
        }
        event
    }

    pub(crate) fn from_entry(raw: &Value) -> Option<Self> {
        if raw.get("type")?.as_str()? != "custom_message"
            || raw.get("customType")?.as_str()? != SKILL_PROMPT_CUSTOM_TYPE
            || raw.get("attribution")?.as_str()? != "user"
        {
            return None;
        }
        let content = serde_json::from_value(raw.get("content")?.clone()).ok()?;
        let timestamp = chrono::DateTime::parse_from_rfc3339(raw.get("timestamp")?.as_str()?).ok()?.timestamp_millis();
        Some(Self { content, details: raw.get("details").cloned(), timestamp })
    }
}
