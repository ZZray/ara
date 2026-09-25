//! Agent loop (OMP `packages/agent/src/agent-loop.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d).
//!
//! Ported: `agentLoop` / `agentLoopContinue` event sequence, streamed
//! assistant turns with `message_start`/`message_update`/`message_end`,
//! abort races that keep only tool calls that reached `toolcall_end`
//! (`retainCompletedToolCalls`), synthetic tool results for
//! error/aborted/length/deadline stops, argument validation and the
//! `beforeToolCall` block decision, shared/exclusive tool scheduling with
//! completion-order result emission, result coercion, deadline handling,
//! steering and follow-up queues, and resuming an unpaired tool-call tail.
//!
//! Not ported (open ledger items): telemetry spans, Harmony leak recovery,
//! soft tool requirements, pause gate, asides, owned in-band dialects, intent
//! tracing, `transformContext`/`beforeModelCall`/`afterToolCall` hooks,
//! mid-batch steering interrupts of interruptible tools, argument streams,
//! transient error tool-turn recovery, pause-turn continuations.

use crate::event::{AgentEvent, AgentEventSink};
use crate::tool::{AgentTool, Concurrency, ToolDecision, ToolOutput};
use ara_ai::validation::validate_tool_arguments;
use ara_ai::{
    AssistantMessage, AssistantMessageEvent, CallOptions, Context, JsonObject, Message, Model, ModelProvider,
    StopReason, ToolCall, ToolChoice, ToolResultMessage, UserBlock, now_ms,
};
use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

pub const STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL: &str = "stream_interrupted_after_content";
pub const ABORTED_TEXT: &str = "Request was aborted";
pub const DEADLINE_TEXT: &str = "Deadline exceeded";
pub const UNPAIRED_TAIL_REFUSED: &str =
    "Cannot continue: the last assistant message has tool calls without recorded results; their effects are unknown";
const EMPTY_ERROR_TOOL_RESULT_TEXT: &str = "Tool failed with no output.";
const MISSING_TERMINAL_EVENT: &str = "Provider stream ended without a terminal event";

/// Host callbacks consulted by the loop. All methods default to no-ops.
#[async_trait]
pub trait LoopHooks: Send + Sync {
    /// Permission gate for a validated call (OMP `beforeToolCall` block).
    /// `cancel` fires when the run is aborted; a pending approval must give up.
    async fn before_tool_call(
        &self,
        _call: &ToolCall,
        _args: &JsonObject,
        _cancel: &CancellationToken,
    ) -> ToolDecision {
        ToolDecision::Allow
    }
    /// Consuming dequeue of steering messages (injected at turn boundaries).
    async fn steering_messages(&self) -> Vec<Message> {
        Vec::new()
    }
    /// Consuming dequeue of follow-up messages (drained when the agent would stop).
    async fn follow_up_messages(&self) -> Vec<Message> {
        Vec::new()
    }
}

pub struct NoHooks;
impl LoopHooks for NoHooks {}

#[derive(Clone)]
pub struct AgentConfig {
    pub model: Model,
    pub provider: Arc<dyn ModelProvider>,
    pub system_prompt: Vec<String>,
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    /// Wall-clock deadline (OMP `config.deadline`).
    pub deadline: Option<Instant>,
    /// Upper bound on model calls in one run (host budget gate, like OMP's
    /// `beforeModelCall` stop). The run ends before the next call once reached.
    pub max_model_calls: Option<usize>,
    pub hooks: Arc<dyn LoopHooks>,
}

/// What `agent_loop_continue` does with a trailing assistant message whose
/// tool calls have no recorded results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnpairedTail {
    /// Refuse: the calls may already have run, so their effects are unknown
    /// (ARA default; AGENTS.md forbids replaying unknown effects).
    Refuse,
    /// Execute them (OMP behavior). Only for a host that knows the calls never ran.
    Execute,
}

/// Run-scoped cancellation: the caller's token, the deadline timer, and the
/// reason text used for aborted messages (OMP `abortReasonText`). Dropping
/// the run cancels everything it started.
struct RunControl {
    token: CancellationToken,
    reason: Arc<std::sync::Mutex<Option<String>>>,
    _guard: tokio_util::sync::DropGuard,
    timer: Option<tokio::task::JoinHandle<()>>,
}

impl RunControl {
    fn new(parent: &CancellationToken, deadline: Option<Instant>) -> RunControl {
        let token = parent.child_token();
        let reason = Arc::new(std::sync::Mutex::new(None));
        let timer = deadline.map(|d| {
            let (token, reason) = (token.clone(), reason.clone());
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep_until(d.into()) => {
                        *reason.lock().unwrap() = Some(DEADLINE_TEXT.to_string());
                        token.cancel();
                    }
                    _ = token.cancelled() => {}
                }
            })
        });
        RunControl { _guard: token.clone().drop_guard(), token, reason, timer }
    }

    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    fn deadline_fired(&self) -> bool {
        self.reason.lock().unwrap().as_deref() == Some(DEADLINE_TEXT)
    }

    fn abort_text(&self) -> String {
        self.reason.lock().unwrap().clone().unwrap_or_else(|| ABORTED_TEXT.to_string())
    }
}

impl Drop for RunControl {
    fn drop(&mut self) {
        if let Some(t) = self.timer.take() {
            t.abort();
        }
    }
}

/// Why a run ended. Hosts use it instead of inferring from the transcript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunEnd {
    /// The assistant stopped with nothing left to do.
    Completed,
    /// `AgentConfig::deadline` passed.
    Deadline,
    /// `AgentConfig::max_model_calls` was reached before another model call.
    ModelCallBudget,
    /// The caller cancelled the run.
    Aborted,
    /// The last assistant turn failed (`stopReason: error`).
    Error,
}

/// Messages added by a run and why it ended.
#[derive(Clone, Debug, PartialEq)]
pub struct RunReport {
    pub messages: Vec<Message>,
    pub end: RunEnd,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopError {
    CannotContinue(String),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopError::CannotContinue(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for LoopError {}

fn deadline_passed(config: &AgentConfig) -> bool {
    config.deadline.is_some_and(|d| Instant::now() >= d)
}

/// Trailing assistant message whose runnable tool calls have no results yet
/// (OMP `unpairedToolCallTail`). `length` turns never qualify.
pub fn unpaired_tool_call_tail(messages: &[Message]) -> Option<&AssistantMessage> {
    match messages.last() {
        Some(Message::Assistant(a))
            if matches!(a.stop_reason, StopReason::ToolUse | StopReason::Stop) && a.tool_calls().next().is_some() =>
        {
            Some(a)
        }
        _ => None,
    }
}

/// Start a run with new prompt messages (OMP `agentLoop`). `context` is the
/// full transcript and is extended in place; the new messages are returned.
pub async fn agent_loop(
    prompts: Vec<Message>,
    context: &mut Vec<Message>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> RunReport {
    let ctl = RunControl::new(cancel, config.deadline);
    context.extend(prompts.iter().cloned());
    let mut new_messages = prompts.clone();
    sink.emit(AgentEvent::AgentStart).await;
    let end = run_loop(context, &mut new_messages, config, &ctl, sink, prompts).await;
    RunReport { messages: new_messages, end }
}

/// Continue from the current transcript without a new message
/// (OMP `agentLoopContinue`).
pub async fn agent_loop_continue(
    context: &mut Vec<Message>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
    tail: UnpairedTail,
) -> Result<RunReport, LoopError> {
    let Some(last) = context.last() else {
        return Err(LoopError::CannotContinue("Cannot continue: no messages in context".into()));
    };
    let unpaired = unpaired_tool_call_tail(context).is_some();
    if matches!(last, Message::Assistant(_)) && !unpaired {
        return Err(LoopError::CannotContinue("Cannot continue from message role: assistant".into()));
    }
    if unpaired && tail == UnpairedTail::Refuse {
        return Err(LoopError::CannotContinue(UNPAIRED_TAIL_REFUSED.into()));
    }
    let ctl = RunControl::new(cancel, config.deadline);
    let mut new_messages = Vec::new();
    sink.emit(AgentEvent::AgentStart).await;
    let end = run_loop(context, &mut new_messages, config, &ctl, sink, Vec::new()).await;
    Ok(RunReport { messages: new_messages, end })
}

async fn emit_inputs(sink: &dyn AgentEventSink, messages: &[Message]) {
    for m in messages {
        sink.emit(AgentEvent::MessageStart { message: m.clone() }).await;
        sink.emit(AgentEvent::MessageEnd { message: m.clone() }).await;
    }
}

async fn end(sink: &dyn AgentEventSink, new_messages: &[Message], reason: RunEnd) -> RunEnd {
    sink.emit(AgentEvent::AgentEnd { messages: new_messages.to_vec() }).await;
    reason
}

async fn run_loop(
    context: &mut Vec<Message>,
    new_messages: &mut Vec<Message>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    initial: Vec<Message>,
) -> RunEnd {
    let mut to_emit = initial;
    if deadline_passed(config) {
        emit_inputs(sink, &to_emit).await;
        return end(sink, new_messages, RunEnd::Deadline).await;
    }
    let mut pending: Vec<Message> =
        if ctl.is_cancelled() { Vec::new() } else { config.hooks.steering_messages().await };

    if let Some(tail) = unpaired_tool_call_tail(context).cloned() {
        sink.emit(AgentEvent::TurnStart).await;
        emit_inputs(sink, &std::mem::take(&mut to_emit)).await;
        let results = execute_calls(&tail, config, ctl, sink).await;
        for r in &results {
            context.push(Message::ToolResult(r.clone()));
            new_messages.push(Message::ToolResult(r.clone()));
        }
        sink.emit(AgentEvent::TurnEnd { message: Message::Assistant(tail), tool_results: results }).await;
    }

    let mut model_calls = 0usize;
    loop {
        let mut has_more_tool_calls = true;
        while has_more_tool_calls || !pending.is_empty() {
            if deadline_passed(config) || config.max_model_calls.is_some_and(|max| model_calls >= max) {
                // Commit already-dequeued messages so queued user input is never lost.
                for m in pending.drain(..) {
                    context.push(m.clone());
                    new_messages.push(m.clone());
                    to_emit.push(m);
                }
                emit_inputs(sink, &to_emit).await;
                let reason = if deadline_passed(config) { RunEnd::Deadline } else { RunEnd::ModelCallBudget };
                return end(sink, new_messages, reason).await;
            }
            let mut turn_messages = std::mem::take(&mut to_emit);
            for m in pending.drain(..) {
                context.push(m.clone());
                new_messages.push(m.clone());
                turn_messages.push(m);
            }
            sink.emit(AgentEvent::TurnStart).await;
            emit_inputs(sink, &turn_messages).await;

            model_calls += 1;
            let message = stream_assistant_response(context, config, ctl, sink).await;
            new_messages.push(Message::Assistant(message.clone()));

            let tool_calls: Vec<ToolCall> = message.tool_calls().cloned().collect();
            if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                let reason = if message.stop_reason == StopReason::Aborted { "aborted" } else { "error" };
                let mut results = Vec::new();
                for call in &tool_calls {
                    let r = emit_synthetic_result(sink, call, reason, message.error_message.as_deref()).await;
                    context.push(Message::ToolResult(r.clone()));
                    new_messages.push(Message::ToolResult(r.clone()));
                    results.push(r);
                }
                let reason = match message.stop_reason {
                    StopReason::Aborted if ctl.deadline_fired() => RunEnd::Deadline,
                    StopReason::Aborted => RunEnd::Aborted,
                    _ => RunEnd::Error,
                };
                sink.emit(AgentEvent::TurnEnd { message: Message::Assistant(message), tool_results: results }).await;
                return end(sink, new_messages, reason).await;
            }

            let runnable = matches!(message.stop_reason, StopReason::ToolUse | StopReason::Stop);
            has_more_tool_calls = runnable && !tool_calls.is_empty();
            let deadline_hit = deadline_passed(config);
            if has_more_tool_calls && deadline_hit {
                has_more_tool_calls = false;
            }
            let mut results = Vec::new();
            if has_more_tool_calls {
                results = execute_calls(&message, config, ctl, sink).await;
                for r in &results {
                    context.push(Message::ToolResult(r.clone()));
                    new_messages.push(Message::ToolResult(r.clone()));
                }
            } else if !tool_calls.is_empty() {
                let (reason, msg) = if deadline_hit {
                    ("aborted", Some(DEADLINE_TEXT))
                } else if message.stop_reason == StopReason::Length {
                    ("length", None)
                } else {
                    ("skipped", None)
                };
                for call in &tool_calls {
                    let r = emit_synthetic_result(sink, call, reason, msg).await;
                    context.push(Message::ToolResult(r.clone()));
                    new_messages.push(Message::ToolResult(r.clone()));
                    results.push(r);
                }
                if message.stop_reason == StopReason::Length && !results.is_empty() && !deadline_hit {
                    has_more_tool_calls = true;
                }
            }
            sink.emit(AgentEvent::TurnEnd { message: Message::Assistant(message), tool_results: results }).await;
            if deadline_passed(config) {
                return end(sink, new_messages, RunEnd::Deadline).await;
            }
            pending = if ctl.is_cancelled() { Vec::new() } else { config.hooks.steering_messages().await };
        }
        if deadline_passed(config) || ctl.deadline_fired() {
            return end(sink, new_messages, RunEnd::Deadline).await;
        }
        if ctl.is_cancelled() {
            return end(sink, new_messages, RunEnd::Aborted).await;
        }
        let mut late = config.hooks.steering_messages().await;
        late.extend(config.hooks.follow_up_messages().await);
        if late.is_empty() {
            break;
        }
        pending = late;
    }
    end(sink, new_messages, RunEnd::Completed).await
}

/// Keep only tool calls that reached `toolcall_end` on an error/aborted turn
/// (OMP `retainCompletedToolCalls`).
pub fn retain_completed_tool_calls(mut message: AssistantMessage, completed: &HashSet<String>) -> AssistantMessage {
    if !matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
        return message;
    }
    let before = message.content.len();
    message.content.retain(|b| b.as_tool_call().is_none_or(|c| completed.contains(&c.id)));
    if message.content.len() != before {
        let already = message.stop_details.as_ref().and_then(|d| d.get("type")).and_then(Value::as_str)
            == Some(STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL);
        if !already {
            let category = message.stop_details.as_ref().and_then(|d| d.get("type")).cloned().unwrap_or(Value::Null);
            let explanation = message
                .stop_details
                .as_ref()
                .and_then(|d| d.get("explanation"))
                .filter(|v| !v.is_null())
                .cloned()
                .or_else(|| message.error_message.clone().map(Value::String))
                .unwrap_or(Value::Null);
            message.stop_details = Some(json!({
                "type": STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL,
                "category": category,
                "explanation": explanation,
            }));
        }
    }
    message
}

fn tool_definitions(config: &AgentConfig) -> Vec<ara_ai::Tool> {
    config.tools.iter().map(|t| t.definition()).collect()
}

async fn stream_assistant_response(
    context: &mut Vec<Message>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
) -> AssistantMessage {
    let cancel = &ctl.token;
    let mut partial: Option<AssistantMessage> = None;
    let mut added_partial = false;
    let mut completed: HashSet<String> = HashSet::new();
    if cancel.is_cancelled() {
        return finish_aborted(context, config, ctl, sink, partial, added_partial, &completed).await;
    }
    let llm_context = Context {
        system_prompt: config.system_prompt.clone(),
        messages: context.clone(),
        tools: Some(tool_definitions(config)),
    };
    let provider_cancel = cancel.child_token();
    let mut rx = config.provider.stream(
        &config.model,
        &llm_context,
        CallOptions {
            cancel: provider_cancel.clone(),
            tool_choice: config.tool_choice.clone(),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        },
    );
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                provider_cancel.cancel();
                drop(rx);
                return finish_aborted(context, config, ctl, sink, partial, added_partial, &completed).await;
            }
            ev = rx.recv() => ev,
        };
        let Some(event) = next else {
            // Provider contract violation: synthesize an error turn instead of hanging.
            let mut msg = partial.clone().unwrap_or_else(|| {
                AssistantMessage::empty(&config.model.api, &config.model.provider, &config.model.id)
            });
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(MISSING_TERMINAL_EVENT.into());
            return commit_final(context, sink, retain_completed_tool_calls(msg, &completed), added_partial).await;
        };
        match event {
            AssistantMessageEvent::Done { message, .. } | AssistantMessageEvent::Error { error: message, .. } => {
                let final_message = retain_completed_tool_calls(message, &completed);
                return commit_final(context, sink, final_message, added_partial).await;
            }
            AssistantMessageEvent::Start { partial: p } => {
                if added_partial {
                    *context.last_mut().unwrap() = Message::Assistant(p.clone());
                    completed.clear();
                    sink.emit(AgentEvent::MessageUpdate {
                        message: Message::Assistant(p.clone()),
                        event: AssistantMessageEvent::Start { partial: p.clone() },
                    })
                    .await;
                } else {
                    context.push(Message::Assistant(p.clone()));
                    added_partial = true;
                    sink.emit(AgentEvent::MessageStart { message: Message::Assistant(p.clone()) }).await;
                }
                partial = Some(p);
            }
            other => {
                if !added_partial {
                    continue;
                }
                if let AssistantMessageEvent::ToolcallEnd { tool_call, .. } = &other {
                    completed.insert(tool_call.id.clone());
                }
                let p = other.partial().clone();
                *context.last_mut().unwrap() = Message::Assistant(p.clone());
                partial = Some(p.clone());
                sink.emit(AgentEvent::MessageUpdate { message: Message::Assistant(p), event: other }).await;
            }
        }
    }
}

async fn commit_final(
    context: &mut Vec<Message>,
    sink: &dyn AgentEventSink,
    message: AssistantMessage,
    added_partial: bool,
) -> AssistantMessage {
    if added_partial {
        *context.last_mut().unwrap() = Message::Assistant(message.clone());
    } else {
        context.push(Message::Assistant(message.clone()));
        sink.emit(AgentEvent::MessageStart { message: Message::Assistant(message.clone()) }).await;
    }
    sink.emit(AgentEvent::MessageEnd { message: Message::Assistant(message.clone()) }).await;
    message
}

/// OMP `emitAbortedAssistantMessage`.
async fn finish_aborted(
    context: &mut Vec<Message>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    partial: Option<AssistantMessage>,
    added_partial: bool,
    completed: &HashSet<String>,
) -> AssistantMessage {
    let mut base =
        partial.unwrap_or_else(|| AssistantMessage::empty(&config.model.api, &config.model.provider, &config.model.id));
    base.stop_reason = StopReason::Aborted;
    base.error_message = Some(ctl.abort_text());
    let retained = retain_completed_tool_calls(base, completed);
    commit_final(context, sink, retained, added_partial).await
}

/// OMP `createSyntheticToolResultMessage`: a call that never executed locally.
pub fn synthetic_tool_result(call: &ToolCall, reason: &str, error_message: Option<&str>) -> ToolResultMessage {
    let message = match reason {
        "aborted" => "Tool execution was aborted",
        "length" => {
            "Tool call was not executed because the assistant hit its output token limit (stop_reason: length) before the arguments could complete; the recorded arguments are truncated and unsafe to run. Do NOT retry by re-emitting the same large payload — split the work into several smaller tool calls (e.g. for `write`/`edit`, write the first chunk then append the rest with subsequent `edit` insert ops, or break the file into multiple `write` targets)"
        }
        "skipped" => "Tool call was not executed because the assistant ended its turn",
        _ => "Tool call was not executed because the provider stream ended with an error before the tool could run",
    };
    let source = match reason {
        "aborted" => "assistant_stop_aborted",
        "error" => "assistant_stop_error",
        "length" => "assistant_stop_length",
        _ => "assistant_stop_skipped",
    };
    let mut details = json!({"__synthetic": true, "source": source, "executed": false});
    if reason == "error"
        && let Some(e) = error_message
    {
        details["upstreamError"] = json!(e);
    }
    let text = match error_message {
        Some(e) => format!("{message}: {e}"),
        None => format!("{message}."),
    };
    ToolResultMessage {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: vec![UserBlock::text(text)],
        details: Some(details),
        is_error: true,
        timestamp: now_ms(),
    }
}

async fn emit_synthetic_result(
    sink: &dyn AgentEventSink,
    call: &ToolCall,
    reason: &str,
    error_message: Option<&str>,
) -> ToolResultMessage {
    let msg = synthetic_tool_result(call, reason, error_message);
    let output = ToolOutput { content: msg.content.clone(), details: msg.details.clone(), is_error: true };
    sink.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.arguments.clone(),
    })
    .await;
    sink.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        result: output,
        is_error: true,
    })
    .await;
    sink.emit(AgentEvent::MessageStart { message: Message::ToolResult(msg.clone()) }).await;
    sink.emit(AgentEvent::MessageEnd { message: Message::ToolResult(msg.clone()) }).await;
    msg
}

/// OMP `coerceToolResult` for typed results: error results never have empty content.
fn coerce(mut output: ToolOutput) -> ToolOutput {
    let substantive = output.content.iter().any(|b| match b {
        UserBlock::Image(_) => true,
        UserBlock::Text(t) => !t.text.trim().is_empty(),
    });
    if output.is_error && !substantive {
        output.content = vec![UserBlock::text(EMPTY_ERROR_TOOL_RESULT_TEXT)];
    }
    output
}

struct Prepared {
    call: ToolCall,
    tool: Option<Arc<dyn AgentTool>>,
    args: JsonObject,
    concurrency: Concurrency,
    validation_error: Option<String>,
    blocked: Option<String>,
}

async fn prepare(
    call: &ToolCall,
    tools: &[(Arc<dyn AgentTool>, ara_ai::Tool)],
    config: &AgentConfig,
    ctl: &RunControl,
) -> Prepared {
    let found = tools.iter().find(|(_, def)| def.name == call.name);
    let mut p = Prepared {
        call: call.clone(),
        tool: found.map(|(t, _)| t.clone()),
        args: call.arguments.clone(),
        concurrency: Concurrency::Shared,
        validation_error: None,
        blocked: None,
    };
    let Some((tool, def)) = found else {
        p.validation_error = Some(format!("Tool {} not found", call.name));
        return p;
    };
    match validate_tool_arguments(def, &call.name, &call.arguments) {
        Ok(args) => p.args = args,
        Err(msg) => {
            // Keep only the parse error, never the (possibly huge) raw payload (OMP parity).
            if let Some(parse_error) = call.arguments.get("__parseError") {
                let mut trimmed = JsonObject::new();
                trimmed.insert("__parseError".into(), parse_error.clone());
                p.args = trimmed;
            }
            p.validation_error = Some(msg);
            return p;
        }
    }
    p.concurrency = tool.concurrency(&p.args);
    if ctl.is_cancelled() {
        return p; // run_tool reports the abort; do not prompt for approval.
    }
    if let ToolDecision::Block(reason) = config.hooks.before_tool_call(call, &p.args, &ctl.token).await {
        p.blocked = Some(reason.unwrap_or_else(|| "Tool execution was blocked".into()));
    }
    p
}

async fn emit_result(sink: &dyn AgentEventSink, p: &Prepared, output: ToolOutput, started: bool) -> ToolResultMessage {
    if !started {
        sink.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: p.call.id.clone(),
            tool_name: p.call.name.clone(),
            args: p.args.clone(),
        })
        .await;
    }
    let is_error = output.is_error;
    sink.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: p.call.id.clone(),
        tool_name: p.call.name.clone(),
        result: output.clone(),
        is_error,
    })
    .await;
    let msg = ToolResultMessage {
        tool_call_id: p.call.id.clone(),
        tool_name: p.call.name.clone(),
        content: output.content,
        details: output.details,
        is_error,
        timestamp: now_ms(),
    };
    sink.emit(AgentEvent::MessageStart { message: Message::ToolResult(msg.clone()) }).await;
    sink.emit(AgentEvent::MessageEnd { message: Message::ToolResult(msg.clone()) }).await;
    msg
}

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

async fn run_tool(p: &Prepared, ctl: &RunControl, sink: &dyn AgentEventSink) -> ToolResultMessage {
    if let Some(err) = &p.validation_error {
        let out = ToolOutput {
            content: vec![UserBlock::text(err.clone())],
            details: Some(json!({"isError": true, "error": err})),
            is_error: true,
        };
        return emit_result(sink, p, out, false).await;
    }
    if ctl.is_cancelled() {
        let out =
            ToolOutput::error(format!("Tool was not executed because the run was aborted: {}.", ctl.abort_text()))
                .with_details(json!({}));
        return emit_result(sink, p, out, false).await;
    }
    sink.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: p.call.id.clone(),
        tool_name: p.call.name.clone(),
        args: p.args.clone(),
    })
    .await;
    let output = if let Some(reason) = &p.blocked {
        ToolOutput::error(reason.clone()).with_details(json!({}))
    } else {
        let tool = p.tool.clone().expect("validated tool exists");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();
        let update: crate::tool::UpdateFn = Arc::new(move |partial| {
            let _ = tx.send(partial);
        });
        // A panicking tool becomes an error result for its own call instead of
        // unwinding through the batch (OMP catches thrown tools per call).
        let fut =
            std::panic::AssertUnwindSafe(tool.execute(&p.call.id, p.args.clone(), ctl.token.child_token(), update))
                .catch_unwind();
        tokio::pin!(fut);
        let result = loop {
            tokio::select! {
                r = &mut fut => break r,
                Some(partial) = rx.recv() => {
                    sink.emit(AgentEvent::ToolExecutionUpdate { tool_call_id: p.call.id.clone(), tool_name: p.call.name.clone(), partial: coerce(partial) }).await;
                }
            }
        };
        while let Ok(partial) = rx.try_recv() {
            sink.emit(AgentEvent::ToolExecutionUpdate {
                tool_call_id: p.call.id.clone(),
                tool_name: p.call.name.clone(),
                partial: coerce(partial),
            })
            .await;
        }
        match result {
            Ok(Ok(out)) => coerce(out),
            Ok(Err(e)) => coerce(ToolOutput::error(e.0).with_details(json!({}))),
            Err(panic) => ToolOutput::error(format!("Tool {} panicked: {}", p.call.name, panic_text(panic)))
                .with_details(json!({"panicked": true})),
        }
    };
    emit_result(sink, p, output, true).await
}

async fn execute_calls(
    message: &AssistantMessage,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
) -> Vec<ToolResultMessage> {
    let tools: Vec<(Arc<dyn AgentTool>, ara_ai::Tool)> =
        config.tools.iter().map(|t| (t.clone(), t.definition())).collect();
    let mut prepared = Vec::new();
    for call in message.tool_calls() {
        prepared.push(prepare(call, &tools, config, ctl).await);
    }
    let results = tokio::sync::Mutex::new(Vec::new());
    let mut i = 0;
    while i < prepared.len() {
        let batch_end = if prepared[i].concurrency == Concurrency::Exclusive {
            i + 1
        } else {
            let mut j = i;
            while j < prepared.len() && prepared[j].concurrency != Concurrency::Exclusive {
                j += 1;
            }
            j
        };
        let futs = prepared[i..batch_end].iter().map(|p| async {
            let r = run_tool(p, ctl, sink).await;
            results.lock().await.push(r);
        });
        futures::future::join_all(futs).await;
        i = batch_end;
    }
    results.into_inner()
}

/// Execute the assistant's tool calls outside a run (OMP `executeToolCalls`).
/// Consecutive `shared` calls run concurrently; an `exclusive` call waits for
/// everything before it and blocks everything after it. Results are emitted
/// and returned in completion order, as upstream does.
pub async fn execute_tool_calls(
    message: &AssistantMessage,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> Vec<ToolResultMessage> {
    let ctl = RunControl::new(cancel, config.deadline);
    execute_calls(message, config, &ctl, sink).await
}
