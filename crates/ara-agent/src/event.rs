//! Agent events (OMP `AgentEvent` in `packages/agent/src/types.ts`).

use ara_ai::{AssistantMessageEvent, JsonObject, Message, ToolResultMessage};
use async_trait::async_trait;
use serde_json::{Value, json};

use crate::agent::AgentInput;
use crate::agent_message::{AgentMessage, RawContextError};
use crate::tool::ToolOutput;

// Events are short-lived values handed to the sink; boxing large variants
// would only add indirection.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum AgentEvent {
    AgentStart,
    AgentEnd {
        messages: Vec<AgentMessage>,
    },
    /// A fallible actual turn-end callback failed after TurnEnd was emitted.
    ActualAgentEnd {
        messages: Vec<AgentMessage>,
        failure: std::sync::Arc<crate::provider_context::ActualProviderError>,
    },
    TurnStart,
    TurnEnd {
        message: Message,
        tool_results: Vec<ToolResultMessage>,
    },
    MessageStart {
        message: AgentMessage,
    },
    MessageUpdate {
        message: AgentMessage,
        event: AssistantMessageEvent,
    },
    ActualMessageUpdate {
        message: AgentMessage,
        event: ara_ai::ActualAssistantEvent,
    },
    ActualTurnEnd {
        message: AgentMessage,
        tool_results: Vec<AgentMessage>,
        preparation_error: Option<std::sync::Arc<crate::provider_context::ActualProviderError>>,
    },
    MessageEnd {
        message: AgentMessage,
    },
    /// A final Graph control/adoption failure retains the same unowned record.
    ActualMessageEnd {
        message: AgentMessage,
        finalization_error: std::sync::Arc<crate::provider_context::ActualProviderError>,
    },
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: JsonObject,
        intent: Option<String>,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        partial: ToolOutput,
    },
    ToolStreamUpdate {
        tool_call_id: String,
        tool_name: String,
        update: Value,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: ToolOutput,
        is_error: bool,
    },
    ActualToolExecutionStart {
        tool_call_id: ara_rpc::WireString,
        tool_name: ara_rpc::WireString,
        args: ara_rpc::value_heap::ValueHandle,
        intent: ara_rpc::value_heap::ValueHandle,
    },
    ActualToolExecutionUpdate {
        tool_call_id: ara_rpc::WireString,
        tool_name: ara_rpc::WireString,
        args: ara_rpc::value_heap::ValueHandle,
        partial: ara_rpc::value_heap::ValueHandle,
    },
    ActualToolExecutionEnd {
        tool_call_id: ara_rpc::WireString,
        tool_name: ara_rpc::WireString,
        result: ara_rpc::value_heap::ValueHandle,
        is_error: bool,
    },
}

fn output_json(o: &ToolOutput) -> Value {
    o.json()
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
            AgentEvent::AgentEnd { .. } | AgentEvent::ActualAgentEnd { .. } => "agent_end",
            AgentEvent::TurnStart => "turn_start",
            AgentEvent::TurnEnd { .. } | AgentEvent::ActualTurnEnd { .. } => "turn_end",
            AgentEvent::MessageStart { .. } => "message_start",
            AgentEvent::MessageUpdate { .. } | AgentEvent::ActualMessageUpdate { .. } => {
                "message_update"
            }
            AgentEvent::MessageEnd { .. } | AgentEvent::ActualMessageEnd { .. } => "message_end",
            AgentEvent::ToolExecutionStart { .. } | AgentEvent::ActualToolExecutionStart { .. } => {
                "tool_execution_start"
            }
            AgentEvent::ToolExecutionUpdate { .. }
            | AgentEvent::ActualToolExecutionUpdate { .. } => "tool_execution_update",
            AgentEvent::ToolStreamUpdate { .. } => "tool_stream_update",
            AgentEvent::ToolExecutionEnd { .. } | AgentEvent::ActualToolExecutionEnd { .. } => {
                "tool_execution_end"
            }
        }
    }

    /// JSON line for `--mode json` (OMP `printableEvent`: `message_update`
    /// carries only the incremental delta, never the partial snapshot).
    pub fn printable(&self) -> Result<Value, RawContextError> {
        self.project(false)
    }

    /// Complete public event for an observing host. Unlike print mode, live
    /// observers need both the message and the provider's current snapshot.
    /// Provider replay payloads/signatures retain the public redaction boundary.
    pub fn full(&self) -> Result<Value, RawContextError> {
        self.project(true)
    }

    fn project(&self, include_snapshot: bool) -> Result<Value, RawContextError> {
        fn scalar(value: &ara_rpc::WireString) -> Result<String, RawContextError> {
            value.to_utf8().map_err(|_| {
                RawContextError("Actual event Text is not a Unicode scalar string".into())
            })
        }
        fn graph(value: &ara_rpc::value_heap::ValueHandle) -> Result<Value, RawContextError> {
            value.heap().export_serde(value).map_err(|error| {
                RawContextError(format!(
                    "Actual event explicit JSON projection failed: {error}"
                ))
            })
        }
        let t = self.type_name();
        Ok(match self {
            AgentEvent::AgentStart | AgentEvent::TurnStart => json!({"type": t}),
            AgentEvent::AgentEnd { messages } => {
                let messages = messages
                    .iter()
                    .map(AgentMessage::json)
                    .collect::<Result<Vec<_>, _>>()?;
                public_event(json!({"type": t, "messages": messages}))
            }
            AgentEvent::ActualAgentEnd { messages, failure } => {
                let messages = messages
                    .iter()
                    .map(AgentMessage::json)
                    .collect::<Result<Vec<_>, _>>()?;
                public_event(json!({"type":t, "messages":messages, "failure":failure.to_string()}))
            }
            AgentEvent::ActualMessageEnd {
                message,
                finalization_error,
            } => public_event(
                json!({"type":t, "message":message.json()?, "finalizationError":finalization_error.to_string()}),
            ),
            AgentEvent::TurnEnd {
                message,
                tool_results,
            } => public_event(json!({"type": t, "message": message, "toolResults": tool_results})),
            AgentEvent::ActualTurnEnd {
                message,
                tool_results,
                preparation_error,
            } => {
                let message = message.json()?;
                let results = tool_results
                    .iter()
                    .map(AgentMessage::json)
                    .collect::<Result<Vec<_>, _>>()?;
                let mut event =
                    public_event(json!({"type":t,"message":message,"toolResults":results}));
                if let Some(error) = preparation_error {
                    event["preparationError"] = Value::String(error.to_string());
                }
                event
            }
            AgentEvent::ActualMessageUpdate { message, event } => {
                let mut assistant_event = event.try_printable().map_err(|error| {
                    RawContextError(format!(
                        "Actual stream printable projection failed: {error}"
                    ))
                })?;
                if include_snapshot {
                    let root = event.partial();
                    let snapshot = root.heap().export_serde(root).map_err(|error| {
                        RawContextError(format!(
                            "Actual stream snapshot projection failed: {error}"
                        ))
                    })?;
                    let snapshot = public_event(json!({"message":snapshot}));
                    let field = match event {
                        ara_ai::AssistantMessageEvent::Done { .. } => "message",
                        ara_ai::AssistantMessageEvent::Error { .. } => "error",
                        _ => "partial",
                    };
                    assistant_event[field] = snapshot["message"].clone();
                    public_event(
                        json!({"type":t,"message":message.json()?,"assistantMessageEvent":assistant_event}),
                    )
                } else {
                    json!({"type":t,"assistantMessageEvent":assistant_event})
                }
            }
            AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } => {
                let message = message.json()?;
                public_event(json!({"type": t, "message": message}))
            }
            AgentEvent::MessageUpdate { message, event } => {
                let mut assistant_event = event.printable();
                if include_snapshot {
                    let snapshot = public_event(
                        json!({"message": Message::Assistant(event.partial().clone())}),
                    );
                    let field = match event {
                        AssistantMessageEvent::Done { .. } => "message",
                        AssistantMessageEvent::Error { .. } => "error",
                        _ => "partial",
                    };
                    assistant_event[field] = snapshot["message"].clone();
                    let message = message.json()?;
                    public_event(
                        json!({"type": t, "message": message, "assistantMessageEvent": assistant_event}),
                    )
                } else {
                    json!({"type": t, "assistantMessageEvent": assistant_event})
                }
            }
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                intent,
            } => {
                let mut value = json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "args": args});
                if let Some(intent) = intent {
                    value["intent"] = Value::String(intent.clone());
                }
                value
            }
            AgentEvent::ToolStreamUpdate {
                tool_call_id,
                tool_name,
                update,
            } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "update": update})
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                tool_name,
                partial,
            } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "partialResult": output_json(partial)})
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                json!({"type": t, "toolCallId": tool_call_id, "toolName": tool_name, "result": output_json(result), "isError": is_error})
            }
            AgentEvent::ActualToolExecutionStart {
                tool_call_id,
                tool_name,
                args,
                intent,
            } => {
                let mut event = json!({"type":t,"toolCallId":scalar(tool_call_id)?,"toolName":scalar(tool_name)?,"args":graph(args)?});
                if intent
                    .heap()
                    .kind(intent)
                    .map_err(|error| RawContextError(error.to_string()))?
                    != ara_rpc::value_heap::ValueKind::Undefined
                {
                    event["intent"] = graph(intent)?;
                }
                event
            }
            AgentEvent::ActualToolExecutionUpdate {
                tool_call_id,
                tool_name,
                args,
                partial,
            } => {
                json!({"type":t,"toolCallId":scalar(tool_call_id)?,"toolName":scalar(tool_name)?,"args":graph(args)?,"partialResult":graph(partial)?})
            }
            AgentEvent::ActualToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => {
                json!({"type":t,"toolCallId":scalar(tool_call_id)?,"toolName":scalar(tool_name)?,"result":graph(result)?,"isError":is_error})
            }
        })
    }
}

/// Receives events in order. The loop awaits each call, so a host that
/// persists `message_end` here has journaled an assistant tool-call message
/// before any of its tools start.
#[async_trait]
pub trait AgentEventSink: Send + Sync {
    /// Optional reason owner for this exact Run. Adapters forward the same
    /// slot; the default preserves existing plain cancellation semantics.
    fn abort_reason(&self) -> Option<crate::agent_loop::RunAbortReason> {
        None
    }

    async fn emit(&self, event: AgentEvent);

    /// Receives one consumed input with its opaque host provenance. The
    /// default preserves the original public MessageStart/MessageEnd pair.
    /// Both calls are awaited before model/tool execution continues.
    async fn emit_input(&self, input: AgentInput) {
        emit_raw_input(self, input.into()).await;
    }

    /// Native Agent catch may retain an externally aborted Assistant without
    /// publishing a message event. Only the stateful Agent adapter commits it;
    /// ordinary observing sinks leave it unjournaled and unforwarded.
    async fn commit_unemitted_message(&self, _message: AgentMessage) {}
}

/// One canonical input pair. Both events traverse the actual Run dispatcher,
/// including its state commit and Host persistence. Source stays on the record.
pub async fn emit_raw_input<S: AgentEventSink + ?Sized>(sink: &S, input: AgentMessage) {
    sink.emit(AgentEvent::MessageStart {
        message: input.clone(),
    })
    .await;
    sink.emit(AgentEvent::MessageEnd { message: input }).await;
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

    #[tokio::test]
    async fn default_owned_input_callback_awaits_original_message_events() {
        struct GateSink {
            events: tokio::sync::Mutex<Vec<AgentEvent>>,
            started: tokio::sync::Notify,
            release: tokio::sync::Notify,
        }
        #[async_trait]
        impl AgentEventSink for GateSink {
            async fn emit(&self, event: AgentEvent) {
                let start = matches!(event, AgentEvent::MessageStart { .. });
                self.events.lock().await.push(event);
                if start {
                    self.started.notify_one();
                    self.release.notified().await;
                }
            }
        }
        let model = Message::User(ara_ai::UserMessage {
            content: ara_ai::UserContent::Text("same model projection".into()),
            synthetic: None,
            timestamp: 42,
            provider_payload: None,
            provider_payload_origin: None,
        });
        let input = AgentInput {
            model: model.clone(),
            provenance: Some(std::sync::Arc::new(json!({"host":"only"}))),
        };
        let raw: AgentMessage = input.clone().into();
        let sink = std::sync::Arc::new(GateSink {
            events: tokio::sync::Mutex::new(Vec::new()),
            started: tokio::sync::Notify::new(),
            release: tokio::sync::Notify::new(),
        });
        let waiting = {
            let sink = sink.clone();
            tokio::spawn(async move { sink.emit_input(input).await })
        };
        tokio::time::timeout(std::time::Duration::from_secs(3), sink.started.notified())
            .await
            .unwrap();
        assert!(matches!(sink.events.lock().await.as_slice(),
            [AgentEvent::MessageStart { message }] if crate::checked_record_equal(message, &raw).unwrap()));
        assert!(!waiting.is_finished());
        sink.release.notify_one();
        waiting.await.unwrap();
        assert!(matches!(sink.events.lock().await.as_slice(),
            [AgentEvent::MessageStart { message: start }, AgentEvent::MessageEnd { message: end }]
                if crate::checked_record_equal(start, &raw).unwrap()
                    && crate::checked_record_equal(end, &raw).unwrap()));
    }

    #[test]
    fn event_projection_hides_only_assistant_replay_data() {
        let event = public_event(json!({
            "message": {"role":"assistant", "providerPayload":{"items":["opaque"]},
                "content":[{"type":"thinking","thinking":"summary","thinkingSignature":"opaque"},
                    {"type":"toolCall","arguments":{"providerPayload":"business value"}}]},
            "toolResults":[{"details":{"providerPayload":"tool detail"}}]
        }));
        assert!(event["message"].get("providerPayload").is_none());
        assert!(
            event["message"]["content"][0]
                .get("thinkingSignature")
                .is_none()
        );
        assert_eq!(
            event["message"]["content"][1]["arguments"]["providerPayload"],
            "business value"
        );
        assert_eq!(
            event["toolResults"][0]["details"]["providerPayload"],
            "tool detail"
        );
    }

    #[test]
    fn full_updates_keep_snapshots_without_changing_print_or_exposing_replay_data() {
        use ara_ai::{AssistantMessage, ThinkingContent};
        let mut assistant = AssistantMessage::empty("openai-responses", "fixture", "model");
        assistant.provider_payload = Some(json!({"private":"native continuation"}));
        assistant
            .content
            .push(ara_ai::AssistantBlock::Thinking(ThinkingContent {
                thinking: "visible summary".into(),
                thinking_signature: Some("private signature".into()),
            }));
        let events = [
            AssistantMessageEvent::ThinkingDelta {
                content_index: 0,
                delta: "summary".into(),
                partial: assistant.clone(),
            },
            AssistantMessageEvent::Done {
                reason: ara_ai::StopReason::Stop,
                message: assistant.clone(),
            },
            AssistantMessageEvent::Error {
                reason: ara_ai::StopReason::Error,
                error: assistant.clone(),
            },
        ];
        for (event, field) in events.into_iter().zip(["partial", "message", "error"]) {
            let update = AgentEvent::MessageUpdate {
                message: Message::Assistant(assistant.clone()).into(),
                event,
            };
            let full = update.full().unwrap();
            assert_eq!(full["message"]["content"][0]["thinking"], "visible summary");
            assert!(full["message"].get("providerPayload").is_none());
            assert!(
                full["message"]["content"][0]
                    .get("thinkingSignature")
                    .is_none()
            );
            let snapshot = &full["assistantMessageEvent"][field];
            assert_eq!(snapshot["content"][0]["thinking"], "visible summary");
            assert!(snapshot.get("providerPayload").is_none());
            assert!(snapshot["content"][0].get("thinkingSignature").is_none());
            let print = update.printable().unwrap();
            assert!(print.get("message").is_none());
            assert!(print["assistantMessageEvent"].get(field).is_none());
        }
    }
}
