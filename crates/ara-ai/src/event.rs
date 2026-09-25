//! Assistant message stream protocol (OMP `AssistantMessageEvent`).
//!
//! Every provider stream emits `start`, then block lifecycle events, then
//! exactly one terminal `done` or `error` event carrying the final message.
//! `partial` is the provider's current snapshot of the assistant message.

use crate::types::{AssistantMessage, StopReason, ToolCall};
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq)]
pub enum AssistantMessageEvent {
    Start {
        partial: AssistantMessage,
    },
    TextStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    TextDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    TextEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    ThinkingStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ThinkingDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ThinkingEnd {
        content_index: usize,
        content: String,
        partial: AssistantMessage,
    },
    ToolcallStart {
        content_index: usize,
        partial: AssistantMessage,
    },
    ToolcallDelta {
        content_index: usize,
        delta: String,
        partial: AssistantMessage,
    },
    ToolcallEnd {
        content_index: usize,
        tool_call: ToolCall,
        partial: AssistantMessage,
    },
    /// `reason` is `stop`, `length` or `toolUse`.
    Done {
        reason: StopReason,
        message: AssistantMessage,
    },
    /// `reason` is `error` or `aborted`.
    Error {
        reason: StopReason,
        error: AssistantMessage,
    },
}

impl AssistantMessageEvent {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::TextStart { .. } => "text_start",
            Self::TextDelta { .. } => "text_delta",
            Self::TextEnd { .. } => "text_end",
            Self::ThinkingStart { .. } => "thinking_start",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ThinkingEnd { .. } => "thinking_end",
            Self::ToolcallStart { .. } => "toolcall_start",
            Self::ToolcallDelta { .. } => "toolcall_delta",
            Self::ToolcallEnd { .. } => "toolcall_end",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Error { .. })
    }

    /// Current snapshot carried by the event.
    pub fn partial(&self) -> &AssistantMessage {
        match self {
            Self::Start { partial }
            | Self::TextStart { partial, .. }
            | Self::TextDelta { partial, .. }
            | Self::TextEnd { partial, .. }
            | Self::ThinkingStart { partial, .. }
            | Self::ThinkingDelta { partial, .. }
            | Self::ThinkingEnd { partial, .. }
            | Self::ToolcallStart { partial, .. }
            | Self::ToolcallDelta { partial, .. }
            | Self::ToolcallEnd { partial, .. } => partial,
            Self::Done { message, .. } => message,
            Self::Error { error, .. } => error,
        }
    }

    /// Printable form without the partial snapshot (OMP `printableEvent`).
    pub fn printable(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Start { .. } => json!({"type": "start"}),
            Self::TextStart { content_index, .. }
            | Self::ThinkingStart { content_index, .. }
            | Self::ToolcallStart { content_index, .. } => {
                json!({"type": self.type_name(), "contentIndex": content_index})
            }
            Self::TextDelta { content_index, delta, .. }
            | Self::ThinkingDelta { content_index, delta, .. }
            | Self::ToolcallDelta { content_index, delta, .. } => {
                json!({"type": self.type_name(), "contentIndex": content_index, "delta": delta})
            }
            Self::TextEnd { content_index, content, .. } | Self::ThinkingEnd { content_index, content, .. } => {
                json!({"type": self.type_name(), "contentIndex": content_index, "content": content})
            }
            Self::ToolcallEnd { content_index, tool_call, .. } => {
                json!({"type": "toolcall_end", "contentIndex": content_index, "toolCall": tool_call})
            }
            Self::Done { reason, .. } | Self::Error { reason, .. } => {
                json!({"type": self.type_name(), "reason": reason})
            }
        }
    }
}

/// Receiving side of a provider stream.
pub type AssistantStream = mpsc::Receiver<AssistantMessageEvent>;

/// Sending side used by providers. Send failures mean the consumer went away;
/// providers keep going only as far as needed to release resources.
#[derive(Clone)]
pub struct EventSink {
    tx: mpsc::Sender<AssistantMessageEvent>,
}

impl EventSink {
    pub fn channel() -> (EventSink, AssistantStream) {
        let (tx, rx) = mpsc::channel(256);
        (EventSink { tx }, rx)
    }

    pub async fn push(&self, event: AssistantMessageEvent) -> bool {
        self.tx.send(event).await.is_ok()
    }

    /// Like [`push`](Self::push) but gives up when `cancel` fires, so a
    /// consumer that stopped reading cannot park a cancelled provider.
    pub async fn push_or_cancel(
        &self,
        event: AssistantMessageEvent,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> bool {
        tokio::select! {
            r = self.tx.send(event) => r.is_ok(),
            _ = cancel.cancelled() => false,
        }
    }
}
