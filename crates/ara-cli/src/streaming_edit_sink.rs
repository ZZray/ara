//! Host event adapter for fixed OMP AgentSession StreamingEditGuard callbacks.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! Original Graph reads happen synchronously during the Native event callback.
//! Only owned filesystem-check inputs leave that callback; previews remain
//! observations and never acquire the Host journal/effect owner.

use crate::streaming_edit_guard::{StreamingEditAbort, StreamingEditGuard};
use ara_agent::{AgentEvent, AgentEventSink, AgentInput, AgentMessage, SessionValues};
use async_trait::async_trait;
use std::ops::Deref;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

/// `S` is the existing borrowed CLI sink or owned RPC sink. Both paths keep
/// their original persistence, projection, input and un-emitted commit owner.
pub struct StreamingEditSink<S> {
    next: S,
    guard: Option<Arc<StreamingEditGuard>>,
    values: Option<Arc<SessionValues>>,
    cancel: CancellationToken,
    live: Arc<Mutex<bool>>,
    reason: ara_agent::RunAbortReason,
}

impl<S> StreamingEditSink<S> {
    pub fn new(
        next: S,
        guard: Option<Arc<StreamingEditGuard>>,
        values: Option<Arc<SessionValues>>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            next,
            guard,
            values,
            cancel,
            live: Arc::new(Mutex::new(true)),
            reason: Default::default(),
        }
    }
}

fn report_abort(
    abort: &StreamingEditAbort,
    cancel: &CancellationToken,
    reason: &ara_agent::RunAbortReason,
) {
    crate::terminal::report_diagnostic(&format!(
        "ara: streaming edit aborted ({}: {})",
        abort.path, abort.error
    ));
    reason.publish(ara_agent::ToolScopedAbortReason {
        message: "Streaming edit preview failed".into(),
        tool_call_messages: std::collections::HashMap::from([(
            abort.tool_call_id.clone(),
            format!(
                "Streaming edit preview failed for {}: {}",
                abort.path, abort.error
            ),
        )]),
        default_tool_call_message: "Streaming edit preview failed".into(),
    });
    cancel.cancel();
}

impl<S> Drop for StreamingEditSink<S> {
    fn drop(&mut self) {
        // Serialise late-check publication with Run exit. Never cancel an
        // already completed Run, a shared REPL token, or an RPC connection.
        *self.live.lock().unwrap_or_else(|error| error.into_inner()) = false;
        if let Some(guard) = &self.guard {
            guard.reset();
        }
    }
}

#[async_trait]
impl<S> AgentEventSink for StreamingEditSink<S>
where
    S: Deref + Send + Sync,
    S::Target: AgentEventSink,
{
    fn abort_reason(&self) -> Option<ara_agent::RunAbortReason> {
        Some(self.reason.clone())
    }

    async fn emit(&self, event: AgentEvent) {
        if let Some(guard) = &self.guard {
            // Fixed AgentSession resets before any awaited event fan-out.
            if matches!(&event, AgentEvent::TurnStart) {
                guard.reset();
            }
            if let Some(abort) = guard.maybe_abort(&event) {
                let active = self.live.lock().unwrap_or_else(|error| error.into_inner());
                // First source owns the cancellation reason. A late preview
                // cannot relabel an externally cancelled or completed Run.
                if *active && !self.cancel.is_cancelled() {
                    report_abort(&abort, &self.cancel, &self.reason);
                }
            }
        }
        self.next.emit(event).await;
    }

    async fn emit_input(&self, input: AgentInput) {
        self.next.emit_input(input).await;
    }

    async fn commit_unemitted_message(&self, message: AgentMessage) {
        self.next.commit_unemitted_message(message).await;
    }
}
