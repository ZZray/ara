//! Optional tool argument previews. Fixed OMP `agent-loop.ts` 1779-1792,
//! 1906-1951 and 2006-2009 at 596f2da7101178214aa27a753529d15e6b7ad91d
//! (MIT; see THIRD_PARTY_NOTICES.md). This owns no tool execution or journal.

#[path = "arg_stream_actual.rs"]
mod actual;

use crate::agent_loop::{LoopHooks, ToolHookError};
use crate::event::{AgentEvent, AgentEventSink};
use crate::tool::{AgentTool, ToolArgStream, ToolArgStreamInit, ToolError};
use ara_ai::{AssistantMessageEvent, Tool};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

pub(crate) type ResolvedTool = (Arc<dyn AgentTool>, Tool);

/// The same logical-name, wire-alias, Host-fallback lookup serves preparation
/// and preview start. Resolver failure is distinct from a preview consumer.
pub(crate) fn resolve_tool_for_call(
    tools: &[ResolvedTool],
    name: &str,
    hooks: &dyn LoopHooks,
) -> Result<Option<ResolvedTool>, ToolHookError> {
    if let Some(found) = tools
        .iter()
        .find(|(_, definition)| definition.name == name)
        .or_else(|| tools.iter().find(|(_, definition)| definition.custom_wire_name.as_deref() == Some(name)))
    {
        return Ok(Some(found.clone()));
    }
    let fallback = catch_unwind(AssertUnwindSafe(|| hooks.resolve_fallback_tool(name)))
        .map_err(|panic| ToolHookError(format!("Fallback tool resolver panicked: {}", panic_text(panic))))?;
    Ok(fallback.map(|tool| {
        let definition = tool.protocol_definition();
        (tool, definition)
    }))
}

fn panic_text(panic: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).into()
    } else {
        "non-string panic".into()
    }
}

/// No logger is owned by Core. Debug builds report the callback boundary only,
/// with neither raw argument fragments nor previews in diagnostics.
fn consumer<T>(id: &str, phase: &str, callback: impl FnOnce() -> Result<T, ToolError>) -> Option<T> {
    match catch_unwind(AssertUnwindSafe(callback)) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(_)) | Err(_) => {
            #[cfg(debug_assertions)]
            eprintln!("ara: tool argument stream {phase} failed ({id})");
            #[cfg(not(debug_assertions))]
            let _ = (id, phase);
            None
        }
    }
}

struct Entry {
    id: String,
    stream: Box<dyn ToolArgStream>,
}

struct PreviewQueue {
    open: bool,
    pending: VecDeque<AgentEvent>,
    wake: Arc<tokio::sync::Notify>,
}

/// One provider attempt. The start order mirrors Native Map insertion order;
/// content indices identify entries independently from the call's wire name.
pub(crate) struct ArgStreams {
    entries: HashMap<usize, Entry>,
    order: Vec<usize>,
    actual_entries: Vec<(ara_rpc::value_heap::ValueHandle, Entry)>,
    updates: Arc<Mutex<PreviewQueue>>,
}

impl ArgStreams {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            actual_entries: Vec::new(),
            updates: Arc::new(Mutex::new(PreviewQueue {
                open: true,
                pending: VecDeque::new(),
                wake: Arc::new(tokio::sync::Notify::new()),
            })),
        }
    }

    pub(crate) fn on_event(
        &mut self,
        event: &AssistantMessageEvent,
        tools: &[Arc<dyn AgentTool>],
        hooks: &dyn LoopHooks,
    ) -> Result<(), ToolHookError> {
        match event {
            AssistantMessageEvent::ToolcallStart { content_index, partial } => {
                let Some(call) = partial.content.get(*content_index).and_then(|block| block.as_tool_call()) else {
                    return Ok(());
                };
                let definitions: Vec<_> = tools.iter().map(|tool| (tool.clone(), tool.protocol_definition())).collect();
                let Some((tool, _)) = resolve_tool_for_call(&definitions, &call.name, hooks)? else {
                    return Ok(());
                };
                let updates = Arc::downgrade(&self.updates);
                let tool_call_id = call.id.clone();
                let tool_name = call.name.clone();
                let emit = Arc::new(move |update: Value| {
                    let Some(updates) = updates.upgrade() else { return };
                    let mut updates = updates.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if updates.open {
                        updates.pending.push_back(AgentEvent::ToolStreamUpdate {
                            tool_call_id: tool_call_id.clone(),
                            tool_name: tool_name.clone(),
                            update,
                        });
                        updates.wake.notify_one();
                    }
                });
                let init = ToolArgStreamInit {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    custom_wire_name: call.custom_wire_name.clone(),
                    emit,
                };
                if let Some(Some(stream)) = consumer(&call.id, "open", || tool.open_arg_stream(init)) {
                    let entry = Entry { id: call.id.clone(), stream };
                    if self.entries.insert(*content_index, entry).is_none() {
                        self.order.push(*content_index);
                    }
                }
            }
            AssistantMessageEvent::ToolcallDelta { content_index, delta, .. } => {
                if let Some(entry) = self.entries.get_mut(content_index) {
                    let _ = consumer(&entry.id, "push", || entry.stream.push(delta));
                }
            }
            AssistantMessageEvent::ToolcallEnd { content_index, tool_call, .. } => {
                if let Some(mut entry) = self.entries.remove(content_index) {
                    self.order.retain(|index| index != content_index);
                    // Provider-final arguments precede transform, validation,
                    // intent stripping and beforeToolCall revisions.
                    let _ = consumer(&entry.id, "end", || entry.stream.end(&tool_call.arguments));
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn preview_wake(&self) -> Arc<tokio::sync::Notify> {
        self.updates.lock().unwrap_or_else(|error| error.into_inner()).wake.clone()
    }

    /// The loop calls this before publishing the ordinary provider frame.
    /// Synchronous callback updates are already enqueued in invocation order.
    pub(crate) async fn flush(&mut self, sink: &dyn AgentEventSink) {
        let pending = {
            let mut updates = self.updates.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            std::mem::take(&mut updates.pending)
        };
        for event in pending {
            sink.emit(event).await;
        }
    }

    fn cancel(&mut self) {
        for (_, mut entry) in self.actual_entries.drain(..) {
            let _ = consumer(&entry.id, "cancel", || entry.stream.cancel());
        }
        for index in self.order.drain(..) {
            if let Some(mut entry) = self.entries.remove(&index) {
                let _ = consumer(&entry.id, "cancel", || entry.stream.cancel());
            }
        }
    }

    /// Call on every ordinary attempt exit, matching Native finally ordering.
    /// Cancel-produced previews are flushed once. Retained emit callbacks are
    /// then inert, including callbacks retained from an already-ended receiver.
    pub(crate) async fn finish(&mut self, sink: &dyn AgentEventSink) {
        self.cancel();
        self.updates.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).open = false;
        self.flush(sink).await;
    }
}

impl Drop for ArgStreams {
    fn drop(&mut self) {
        self.cancel();
        let mut updates = self.updates.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        updates.open = false;
        // Dropping the provider-attempt future cannot await its Host sink.
        // Cleanup callbacks still run; unpublished previews cannot escape into
        // a new attempt or fabricate a final message/tool receipt.
        updates.pending.clear();
    }
}
