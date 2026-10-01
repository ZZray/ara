//! Message model shared by providers, the agent loop and the session journal.
//!
//! Ported from OMP `packages/ai/src/types.ts` and `packages/catalog/src/types.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d. Field names and tags follow the
//! upstream JSON shape so persisted sessions stay comparable.
//!
//! Intentional difference: every [`Usage`] bucket is optional. Upstream fills
//! zeros when a provider reports nothing; ARA keeps `None` so unknown usage is
//! never reported as zero.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type JsonObject = Map<String, Value>;

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
    #[serde(rename = "textSignature", default, skip_serializing_if = "Option::is_none")]
    pub text_signature: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ThinkingContent {
    pub thinking: String,
    #[serde(rename = "thinkingSignature", default, skip_serializing_if = "Option::is_none")]
    pub thinking_signature: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageContent {
    /// Base64 encoded bytes.
    pub data: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: JsonObject,
    #[serde(rename = "thoughtSignature", default, skip_serializing_if = "Option::is_none")]
    pub thought_signature: Option<String>,
}

/// Content allowed in user, developer and tool-result messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum UserBlock {
    Text(TextContent),
    Image(ImageContent),
}

impl UserBlock {
    pub fn text(text: impl Into<String>) -> Self {
        UserBlock::Text(TextContent { text: text.into(), text_signature: None })
    }
}

/// Content allowed in assistant messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AssistantBlock {
    Text(TextContent),
    Thinking(ThinkingContent),
    RedactedThinking { data: String },
    Image(ImageContent),
    ToolCall(ToolCall),
}

impl AssistantBlock {
    pub fn text(text: impl Into<String>) -> Self {
        AssistantBlock::Text(TextContent { text: text.into(), text_signature: None })
    }
    pub fn as_tool_call(&self) -> Option<&ToolCall> {
        match self {
            AssistantBlock::ToolCall(call) => Some(call),
            _ => None,
        }
    }
}

/// `string | (TextContent | ImageContent)[]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Blocks(Vec<UserBlock>),
}

impl UserContent {
    /// Concatenated text parts (images omitted).
    pub fn plain_text(&self) -> String {
        match self {
            UserContent::Text(text) => text.clone(),
            UserContent::Blocks(blocks) => blocks
                .iter()
                .filter_map(|b| match b {
                    UserBlock::Text(t) => Some(t.text.as_str()),
                    UserBlock::Image(_) => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

impl StopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            StopReason::Stop => "stop",
            StopReason::Length => "length",
            StopReason::ToolUse => "toolUse",
            StopReason::Error => "error",
            StopReason::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    #[serde(rename = "cacheRead")]
    pub cache_read: f64,
    #[serde(rename = "cacheWrite")]
    pub cache_write: f64,
    pub total: f64,
}

/// Token usage. `None` means the provider did not report the bucket (unknown),
/// never zero.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    #[serde(rename = "cacheRead", default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<u64>,
    #[serde(rename = "cacheWrite", default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<u64>,
    #[serde(rename = "totalTokens", default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(rename = "reasoningTokens", default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    /// Provider-reported cost when present; ARA does not price usage locally yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
}

impl Usage {
    /// Usage for a call whose provider reported nothing.
    pub fn unknown() -> Self {
        Usage::default()
    }
    pub fn is_unknown(&self) -> bool {
        self.input.is_none() && self.output.is_none() && self.total_tokens.is_none()
    }
}

/// Outer replay attempts for one provider call. `usage` on the delivered
/// assistant message still describes only the delivered attempt. A missing
/// bucket in any attempt remains unknown when these records are added up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetryAccounting {
    pub attempts: Vec<RetryAttempt>,
    #[serde(rename = "elapsedMs")]
    pub elapsed_ms: u64,
}

/// One attempt observed by ARA's replay-safe wrapper. Inner HTTP retries that
/// never produced an assistant message cannot report token usage here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RetryAttempt {
    pub usage: Usage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    #[serde(rename = "elapsedMs")]
    pub elapsed_ms: u64,
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub content: UserContent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synthetic: Option<bool>,
    pub timestamp: i64,
}

impl UserMessage {
    pub fn text(text: impl Into<String>) -> Self {
        UserMessage { content: UserContent::Text(text.into()), synthetic: None, timestamp: now_ms() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeveloperMessage {
    pub content: UserContent,
    pub timestamp: i64,
}

/// Fixed OMP `AssistantRetryRecovery`: raw failed receipts remain sourced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantRetryRecovery {
    pub kind: String,
    pub status: String,
    pub attempt: usize,
    pub recovery: String,
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovered_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<Value>,
}

/// Responses wire evidence for replacing a failed turn's context. This does
/// not authorize transparent provider replay or ordinary same-route retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextRecoveryEvidence {
    ContentOnly,
    NativeOutput,
    UsageAdmission,
    NativeValidation,
}

impl ContextRecoveryEvidence {
    pub(crate) fn merge(self, other: Self) -> Self {
        if self == Self::ContentOnly { other } else { self }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Vec<AssistantBlock>,
    pub api: String,
    pub provider: String,
    pub model: String,
    #[serde(rename = "responseId", default, skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    #[serde(rename = "upstreamProvider", default, skip_serializing_if = "Option::is_none")]
    pub upstream_provider: Option<String>,
    /// Opaque provider-native history for same-provider continuation. Hosts
    /// persist this with the message, but must not print it in client events.
    #[serde(rename = "providerPayload", default, skip_serializing_if = "Option::is_none")]
    pub provider_payload: Option<Value>,
    pub usage: Usage,
    /// Present only after a replay was planned or started. These are outer
    /// attempts, including discarded ones; they are not a billing verdict.
    #[serde(rename = "retryAccounting", default, skip_serializing_if = "Option::is_none")]
    pub retry_accounting: Option<RetryAccounting>,
    #[serde(rename = "retryRecovery", default, skip_serializing_if = "Option::is_none")]
    pub retry_recovery: Option<AssistantRetryRecovery>,
    #[serde(rename = "stopReason")]
    pub stop_reason: StopReason,
    #[serde(rename = "stopDetails", default, skip_serializing_if = "Option::is_none")]
    pub stop_details: Option<Value>,
    #[serde(rename = "errorMessage", default, skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    #[serde(rename = "errorStatus", default, skip_serializing_if = "Option::is_none")]
    pub error_status: Option<u16>,
    /// Actual typed provider failure, with bounded classification and wait facts.
    #[serde(rename = "failureEvidence", default, skip_serializing_if = "Option::is_none")]
    pub failure_evidence: Option<crate::retry_classification::ProviderFailureEvidence>,
    /// Wire safety observed for a terminal, including successful Length/Stop.
    /// Absent historical metadata is unassessed, never positive replay proof.
    #[serde(rename = "terminalContextRecovery", default, skip_serializing_if = "Option::is_none")]
    pub terminal_context_recovery: Option<ContextRecoveryEvidence>,
    pub timestamp: i64,
    /// Request duration in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration: Option<u64>,
    /// Time to first token in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft: Option<u64>,
}

impl AssistantMessage {
    pub fn empty(api: &str, provider: &str, model: &str) -> Self {
        AssistantMessage {
            content: Vec::new(),
            api: api.to_string(),
            provider: provider.to_string(),
            model: model.to_string(),
            response_id: None,
            upstream_provider: None,
            provider_payload: None,
            usage: Usage::unknown(),
            retry_accounting: None,
            retry_recovery: None,
            stop_reason: StopReason::Stop,
            stop_details: None,
            error_message: None,
            error_status: None,
            failure_evidence: None,
            terminal_context_recovery: None,
            timestamp: now_ms(),
            duration: None,
            ttft: None,
        }
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(AssistantBlock::as_tool_call)
    }

    /// Concatenated visible text blocks.
    pub fn text(&self) -> String {
        self.content
            .iter()
            .filter_map(|b| match b {
                AssistantBlock::Text(t) => Some(t.text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultMessage {
    #[serde(rename = "toolCallId")]
    pub tool_call_id: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    pub content: Vec<UserBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub timestamp: i64,
}

// Messages are plain transcript values moved between the journal, the loop and
// providers; boxing the assistant variant would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role")]
pub enum Message {
    #[serde(rename = "user")]
    User(UserMessage),
    #[serde(rename = "developer")]
    Developer(DeveloperMessage),
    #[serde(rename = "assistant")]
    Assistant(AssistantMessage),
    #[serde(rename = "toolResult")]
    ToolResult(ToolResultMessage),
}

impl Message {
    pub fn role(&self) -> &'static str {
        match self {
            Message::User(_) => "user",
            Message::Developer(_) => "developer",
            Message::Assistant(_) => "assistant",
            Message::ToolResult(_) => "toolResult",
        }
    }
    pub fn as_assistant(&self) -> Option<&AssistantMessage> {
        match self {
            Message::Assistant(m) => Some(m),
            _ => None,
        }
    }
}

/// Tool definition sent to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema object for the arguments.
    pub parameters: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Context {
    pub system_prompt: Vec<String>,
    pub messages: Vec<Message>,
    /// `None` means tools were not specified; `Some(vec![])` means explicitly none.
    pub tools: Option<Vec<Tool>>,
}

/// Wire tool-choice directive.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolChoice {
    Auto,
    None,
    Required,
    Tool(String),
}

/// A model endpoint selected by the host.
#[derive(Clone, Debug, PartialEq)]
pub struct Model {
    /// Local model id; also sent on the wire.
    pub id: String,
    /// Wire API, e.g. `openai-completions`.
    pub api: String,
    /// Provider label recorded on messages, e.g. `openrouter`.
    pub provider: String,
    pub base_url: String,
    pub reasoning: bool,
    pub max_tokens: Option<u64>,
    /// Host-resolved prompt capacity in tokens, as OMP's numeric contextWindow.
    /// None means unknown; this metadata is never sent on the wire.
    pub context_window: Option<f64>,
    /// Local content tokenizer selected by the host; never sent on the wire.
    pub tokenizer: Option<crate::model_tokenizer::ModelTokenizer>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn message_json_shape_matches_omp() {
        // Shape taken from OMP session fixture packages/coding-agent/test/fixtures/before-compaction.jsonl.
        let raw = json!({
            "role": "assistant",
            "content": [
                {"type": "text", "text": "hi"},
                {"type": "toolCall", "id": "call_1", "name": "read", "arguments": {"path": "a.txt"}}
            ],
            "api": "openai-completions",
            "provider": "openrouter",
            "model": "m",
            "usage": {"input": 3, "output": 4, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 7},
            "stopReason": "toolUse",
            "timestamp": 1
        });
        let msg: Message = serde_json::from_value(raw.clone()).unwrap();
        let assistant = msg.as_assistant().unwrap();
        assert_eq!(assistant.stop_reason, StopReason::ToolUse);
        assert_eq!(assistant.tool_calls().count(), 1);
        assert_eq!(serde_json::to_value(&msg).unwrap(), raw);
    }

    #[test]
    fn unknown_usage_is_not_zero() {
        let usage = Usage::unknown();
        assert!(usage.is_unknown());
        assert_eq!(serde_json::to_value(&usage).unwrap(), json!({}));
    }

    #[test]
    fn user_content_accepts_string_and_blocks() {
        let a: UserMessage = serde_json::from_value(json!({"content": "x", "timestamp": 1})).unwrap();
        assert_eq!(a.content, UserContent::Text("x".into()));
        let b: UserMessage =
            serde_json::from_value(json!({"content": [{"type": "text", "text": "y"}], "timestamp": 1})).unwrap();
        assert_eq!(b.content.plain_text(), "y");
    }
}
