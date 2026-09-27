//! Agent events (OMP `AgentEvent` in `packages/agent/src/types.ts`).

use ara_ai::{AssistantMessageEvent, JsonObject, Message, ToolResultMessage};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::tool::ToolOutput;

// Events are short-lived values handed to the sink; boxing large variants
// would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd { messages: Vec<Message> },
    TurnStart,
    TurnEnd { message: Message, tool_results: Vec<ToolResultMessage> },
    MessageStart { message: Message },
    MessageUpdate { message: Message, event: AssistantMessageEvent },
    MessageEnd { message: Message },
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: JsonObject },
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, partial: ToolOutput },
    ToolExecutionEnd { tool_call_id: String, tool_name: String, result: ToolOutput, is_error: bool },
}

fn output_json(o: &ToolOutput) -> Value {
    let mut v = json!({"content": o.content});
    if let Some(d) = &o.details {
        v["details"] = d.clone();
    }
    if o.is_error {
        v["isError"] = json!(true);
    }
    v
}

fn public_event(mut value: Value) -> Value {
    fn strip(message: &mut Value) {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        if let Some(fields) = message.as_object_mut() {
            fields.remove("providerPayload");
        }
        if let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) {
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("thinking")
                    && let Some(fields) = block.as_object_mut()
                {
                    fields.remove("thinkingSignature");
                }
            }
        }
    }
    if let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) {
        messages.iter_mut().for_each(strip);
    }
    if let Some(message) = value.get_mut("message") {
        strip(message);
    }
    value
}

impl AgentEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            AgentEvent::AgentStart => "agent_start",
            AgentEvent::AgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart => "turn_start",
            AgentEvent::TurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } => "message_update",
            AgentEvent::MessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } => "tool_execution_start",
            AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end",
        }
    }

    /// JSON line for `--mode json` (OMP `printableEvent`: `message_update`
    /// carries only the incremental delta, never the partial snapshot).
    pub fn printable(&self) -> Value {
        let t = self.type_name();
        match self {
            AgentEvent::AgentStart | AgentEvent::TurnStart => json!({"type": t}),
            AgentEvent::AgentEnd { messages } => public_event(json!({"type": t, "messages": messages})),
            AgentEvent::TurnEnd { message, tool_results } => {
                public_event(json!({"type": t, "message": message, "toolResults": tool_results}))
            }
            AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } => {
                public_event(json!({"type": t, "message": message}))
            }
            AgentEvent::MessageUpdate { event, .. } => json!({"type": t, "assistantMessageEvent": event.printable()}),
            AgentEvent::ToolExecutionStart { tool_call_id, tool_name, args } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "args": args})
            }
            AgentEvent::ToolExecutionUpdate { tool_call_id, tool_name, partial } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "partialResult": output_json(partial)})
            }
            AgentEvent::ToolExecutionEnd { tool_call_id, tool_name, result, is_error } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "result": output_json(result), "isError": is_error})
            }
        }
    }
}

/// Receives events in order. The loop awaits each call, so a host that
/// persists `message_end` here has journaled an assistant tool-call message
/// before any of its tools start.
#[async_trait]
pub trait AgentEventSink: Send + Sync {
    async fn emit(&self, event: AgentEvent);
}

/// Sink that discards events.
pub struct NullSink;

#[async_trait]
impl AgentEventSink for NullSink {
    async fn emit(&self, _event: AgentEvent) {}
}

/// Sink that records events (tests).
#[derive(Default)]
pub struct RecordingSink {
    pub events: tokio::sync::Mutex<Vec<AgentEvent>>,
}

#[async_trait]
impl AgentEventSink for RecordingSink {
    async fn emit(&self, event: AgentEvent) {
        self.events.lock().await.push(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_projection_hides_only_assistant_replay_data() {
        let event = public_event(json!({
            "message": {"role":"assistant", "providerPayload":{"items":["opaque"]},
                "content":[{"type":"thinking","thinking":"summary","thinkingSignature":"opaque"},
                    {"type":"toolCall","arguments":{"providerPayload":"business value"}}]},
            "toolResults":[{"details":{"providerPayload":"tool detail"}}]
        }));
        assert!(event["message"].get("providerPayload").is_none());
        assert!(event["message"]["content"][0].get("thinkingSignature").is_none());
        assert_eq!(event["message"]["content"][1]["arguments"]["providerPayload"], "business value");
        assert_eq!(event["toolResults"][0]["details"]["providerPayload"], "tool detail");
    }
}
