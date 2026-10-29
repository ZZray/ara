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
//! owned in-band dialects,
//! Harmony reentry of the final-provider gate,
//! transient error tool-turn recovery. Soft directives, process pause, asides,
//! argument streams and bounded pause-turn continuations are wired through this same loop.

use crate::agent::AgentInput;
use crate::agent_message::{
    AgentMessage, AgentMessageBody, RawContextError, checked_model_messages,
    default_agent_message_conversion, typed_admission_context,
};
use crate::append_only_context::SharedAppendOnlyContextManager;
use crate::arg_stream::{ArgStreams, resolve_tool_for_call};
use crate::aside::{AsideError, AsideMessage, resolve_asides};
use crate::event::{AgentEvent, AgentEventSink, emit_raw_input};
use crate::final_provider_gate::{
    FinalProviderGateBinding, FinalProviderGateError, FinalProviderGateStop,
};
use crate::pause::AGENT_PAUSE_GATE;
use crate::provider_context::{
    ActualProviderError, PreparedAssistantStream, PreparedProviderCall, PreparedRequestContext,
    PreparedStreamEvent, ProviderContextBinding, ProviderContextError, SessionValues,
    annotate_call_intent, annotate_intents, main_actual_request_context, main_request_context,
    stripped_arguments, tool_owns_intent,
};
use crate::soft_tool_requirement::{
    SoftToolRequirementRunGuard, SoftToolRequirementState, ToolChoiceDirective,
    ToolChoiceDirectiveSelection,
};
use crate::tool::{
    ActualToolSource, AgentTool, Concurrency, ToolCallInfo, ToolDecision, ToolExecutionContext,
    ToolOutput,
};
use crate::tool_directives::{ToolChoiceSelection, ToolDirectiveError};
use ara_ai::validation::{
    ToolArgumentsOrigin, validate_tool_arguments, validate_tool_arguments_with_origin,
};
use ara_ai::{
    AssistantMessage, AssistantMessageEvent, CallOptions, Context, JsonObject, Message, Model,
    ModelProvider, StopReason, ToolCall, ToolChoice, ToolResultMessage, UserBlock, now_ms,
    snapshot_tool_call_provider_metadata, snapshot_tool_result_provider_metadata,
};
use async_trait::async_trait;
use futures::FutureExt;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

mod actual;
mod actual_execute;
mod actual_prepare;
use actual::{ActualFinalAssistant, ActualStreamState, process_actual_turn};

pub const STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL: &str = "stream_interrupted_after_content";
pub const ABORTED_TEXT: &str = "Request was aborted";
pub const DEADLINE_TEXT: &str = "Deadline exceeded";
pub const UNPAIRED_TAIL_REFUSED: &str = "Cannot continue: the last assistant message has tool calls without recorded results; their effects are unknown";
const EMPTY_ERROR_TOOL_RESULT_TEXT: &str = "Tool failed with no output.";
const MISSING_TERMINAL_EVENT: &str = "Provider stream ended without a terminal event";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueInputKind {
    Steering,
    FollowUp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolHookError(pub String);

impl std::fmt::Display for ToolHookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ToolHookError {}

pub struct AgentContextView<'a> {
    pub system_prompt: &'a [String],
    pub messages: &'a [AgentMessage],
    pub tools: &'a [Arc<dyn AgentTool>],
}

pub struct BeforeToolCallContext<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_call: &'a ToolCall,
    pub tool: &'a Arc<dyn AgentTool>,
    pub args: JsonObject,
    pub context: AgentContextView<'a>,
}

pub struct ActualAgentContextView<'a> {
    pub system_prompt: &'a [String],
    pub messages: &'a [AgentMessage],
    /// The original Host source capability, never the advertised typed tools.
    /// Host binds its original context from this source without a new registry.
    pub tool_source: Option<&'a dyn ActualToolSource>,
}

pub struct ActualBeforeToolCallContext<'a> {
    pub assistant_message: &'a AgentMessage,
    pub tool_call: &'a ara_rpc::value_heap::ValueHandle,
    pub tool: &'a crate::tool::ActualToolBinding,
    pub args: &'a ara_rpc::value_heap::ValueHandle,
    pub context: ActualAgentContextView<'a>,
    pub values: &'a SessionValues,
}

pub struct ActualAfterToolCallContext<'a> {
    pub assistant_message: &'a AgentMessage,
    pub tool_call: &'a ara_rpc::value_heap::ValueHandle,
    pub args: &'a ara_rpc::value_heap::ValueHandle,
    pub result: &'a ara_rpc::value_heap::ValueHandle,
    pub is_error: bool,
    pub context: ActualAgentContextView<'a>,
    pub terminal: TerminalToolResultHandle,
    pub values: &'a SessionValues,
}

pub struct ActualTurnEndContext<'a> {
    pub assistant_message: &'a AgentMessage,
    pub tool_results: &'a [AgentMessage],
    pub context: ActualAgentContextView<'a>,
    pub will_continue: bool,
    pub stop_reason: Option<StopReason>,
    pub stop_cause: Option<&'a RunStopCause>,
    pub values: &'a SessionValues,
}

#[derive(Clone, Debug)]
pub enum ActualToolDecision {
    Allow,
    Block(Option<ara_rpc::WireString>),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeforeToolCallResult {
    pub block: bool,
    pub reason: Option<String>,
    pub args: Option<JsonObject>,
}

pub struct AfterToolCallContext<'a> {
    pub assistant_message: &'a AssistantMessage,
    pub tool_call: &'a ToolCall,
    pub args: &'a JsonObject,
    pub result: &'a mut ToolOutput,
    /// Native local error flag, separate from result.is_error.
    pub is_error: bool,
    pub context: AgentContextView<'a>,
    pub terminal: TerminalToolResultHandle,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AfterToolCallResult {
    pub content: Option<Vec<UserBlock>>,
    pub details: Option<Value>,
    /// Raw extension override; None and JSON null keep the mutated old value.
    pub provider_metadata: Option<Value>,
    pub is_error: Option<bool>,
    pub useless: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InterruptMode {
    #[default]
    Immediate,
    Wait,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteeringSource {
    User,
    Agent,
    System,
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SteeringQueueState {
    pub queued: bool,
    pub source: Option<SteeringSource>,
}

#[async_trait]
pub trait SteeringQueuePeek: Send + Sync {
    async fn peek(&self) -> Result<SteeringQueueState, ToolHookError>;
}

type SteeringWaitFuture = Pin<Box<dyn Future<Output = Result<(), ToolHookError>> + Send>>;

/// Owns a listener already installed by SteeringWake::subscribe. Dropping an
/// unpolled or pending wait always releases its future and optional listener.
pub struct SteeringSubscription {
    future: Option<SteeringWaitFuture>,
    cleanup: Option<Box<dyn FnOnce() + Send>>,
}
impl SteeringSubscription {
    pub fn new(
        future: impl Future<Output = Result<(), ToolHookError>> + Send + 'static,
        cleanup: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self {
            future: Some(Box::pin(future)),
            cleanup: Some(Box::new(cleanup)),
        }
    }
    pub fn from_future(
        future: impl Future<Output = Result<(), ToolHookError>> + Send + 'static,
    ) -> Self {
        Self {
            future: Some(Box::pin(future)),
            cleanup: None,
        }
    }
    pub async fn wait(mut self) -> Result<(), ToolHookError> {
        self.future.take().expect("subscription waited once").await
    }
}
impl Drop for SteeringSubscription {
    fn drop(&mut self) {
        if let Some(cleanup) = self.cleanup.take() {
            cleanup();
        }
    }
}
pub trait SteeringWake: Send + Sync {
    /// Register synchronously BEFORE returning; never only when wait is polled.
    fn subscribe(&self, local_cancel: CancellationToken) -> SteeringSubscription;
}

#[derive(Clone)]
pub struct SteeringInterruptBinding {
    peek: Arc<dyn SteeringQueuePeek>,
    wake: Option<Arc<dyn SteeringWake>>,
    requires_polling: bool,
}
impl SteeringInterruptBinding {
    pub fn new(peek: Arc<dyn SteeringQueuePeek>, wake: Option<Arc<dyn SteeringWake>>) -> Self {
        let requires_polling = wake.is_none();
        Self {
            peek,
            wake,
            requires_polling,
        }
    }
    pub(crate) fn composite(
        peek: Arc<dyn SteeringQueuePeek>,
        wake: Arc<dyn SteeringWake>,
        requires_polling: bool,
    ) -> Self {
        Self {
            peek,
            wake: Some(wake),
            requires_polling,
        }
    }
    pub fn peek_port(&self) -> &Arc<dyn SteeringQueuePeek> {
        &self.peek
    }
    pub fn wake_port(&self) -> Option<&Arc<dyn SteeringWake>> {
        self.wake.as_ref()
    }
    pub fn requires_polling(&self) -> bool {
        self.requires_polling
    }
}

#[async_trait]
pub trait IrcInterruptPeek: Send + Sync {
    async fn queued(&self) -> Result<bool, ToolHookError>;
}

/// Pinned OMP b91 tool-scoped abort label. IDs retain exact UTF-16 identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolScopedAbortReason {
    pub message: String,
    pub tool_call_messages: std::collections::HashMap<ara_rpc::WireString, String>,
    pub default_tool_call_message: String,
}
impl ToolScopedAbortReason {
    pub fn for_tool(&self, id: &ara_rpc::WireString) -> &str {
        self.tool_call_messages
            .get(id)
            .map(String::as_str)
            .unwrap_or(&self.default_tool_call_message)
    }
}
/// One Host-owned reason slot per Run. Publish before cancelling that Run's
/// child token; this does not cancel a parent Session or create a new Run.
#[derive(Clone, Default)]
pub struct RunAbortReason(Arc<Mutex<Option<ToolScopedAbortReason>>>);
impl RunAbortReason {
    pub fn publish(&self, reason: ToolScopedAbortReason) -> bool {
        let mut current = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if current.is_some() {
            return false;
        }
        *current = Some(reason);
        true
    }
    pub fn get(&self) -> Option<ToolScopedAbortReason> {
        self.0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStopCause {
    ExternalAbort,
    ToolScopedAbort { reason: ToolScopedAbortReason },
    Deadline,
    TerminalToolResult { tool_call_id: String },
    PreModelStop { reason: Option<String> },
}

#[derive(Clone)]
pub struct TerminalToolResultHandle {
    state: Weak<Mutex<RunStopState>>,
    tool_call_id: String,
}
impl TerminalToolResultHandle {
    pub fn request_terminal_result(&self) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let mut state = state.lock().unwrap();
        if !state.active {
            return false;
        }
        state.observe_external();
        if state.cause.is_some() {
            return false;
        }
        state.cause = Some(RunStopCause::TerminalToolResult {
            tool_call_id: self.tool_call_id.clone(),
        });
        state.token.cancel();
        true
    }
}

pub struct TurnEndContext<'a> {
    pub message: &'a AssistantMessage,
    pub tool_results: &'a [ToolResultMessage],
    /// Current raw Run view, independent from the finalized typed receipts.
    pub context: AgentContextView<'a>,
    pub will_continue: bool,
    pub stop_cause: Option<&'a RunStopCause>,
}

/// Host callbacks consulted by the loop. All methods default to no-ops.
#[async_trait]
pub trait LoopHooks: Send + Sync {
    /// One final-request gate owner; empty bindings are valid Host owners.
    fn final_provider_gate_binding(&self) -> Option<FinalProviderGateBinding> {
        None
    }

    /// An unserved final request was refused while no turn/external abort is
    /// open. Dynamic directive claim/defer state remains a separate contract.
    fn on_final_provider_gate_rejected(&self) -> Result<(), FinalProviderGateError> {
        Ok(())
    }

    /// Resolve one logical-turn hard choice before provider preparation. This
    /// foundation accepts only the four canonical ara_ai ToolChoice variants.
    /// Direct loops inherit static fallback; Agent applies retained/active policy.
    fn select_tool_choice_directive(
        &self,
        _active_tools: &[Arc<dyn AgentTool>],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<ToolChoiceSelection, ToolDirectiveError> {
        Ok(ToolChoiceSelection::inherit(admitted_static_choice))
    }

    /// Full Native directive. Existing hard-only hosts use this adapter.
    fn select_tool_choice_directive_full(
        &self,
        active_tools: &[Arc<dyn AgentTool>],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<ToolChoiceDirectiveSelection, ToolDirectiveError> {
        self.select_tool_choice_directive(active_tools, admitted_static_choice)
            .map(Into::into)
    }

    /// Agent Runs bind one owner before execution. Direct loops use a local
    /// owner unless the host explicitly supplies a shared lifecycle.
    fn soft_tool_requirement_state(&self) -> Option<SoftToolRequirementState> {
        None
    }

    /// Called only when a retained Agent hard choice is no longer active.
    fn on_deferred_tool_choice_unavailable(&self) -> Result<(), ToolDirectiveError> {
        Ok(())
    }

    /// Opt in to owned transcript views. Default hosts keep the original
    /// snapshot-read and allocation behavior.
    fn context_admission_enabled(&self) -> bool {
        false
    }
    /// Current model route, tool adapters and matching prompt, read once before each model
    /// call and retained through that response's whole tool batch. Standalone
    /// tool execution reads once before preparation. `None` retains the Run's
    /// original configuration.
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        None
    }
    /// Fixed initial provider-context options. Dynamic Hosts carry this in the execution snapshot.
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        None
    }
    fn set_append_only_context(
        &self,
        _manager: Option<SharedAppendOnlyContextManager>,
    ) -> Result<(), ProviderContextError> {
        Err(ProviderContextError::HostOwnedMutation)
    }
    /// Admission before a consuming steering or follow-up dequeue. A failure
    /// leaves both the Host and Agent queues untouched.
    async fn before_queued_input_dequeue(
        &self,
        _cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        Ok(())
    }
    /// Admit this owned candidate before every model call, including a tool
    /// continuation. The returned snapshot is used for that response and its
    /// whole tool batch; the loop does not reread a live snapshot afterward.
    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        _cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        Ok(snapshot)
    }
    /// Typed compatibility transcript for context-sensitive legacy callbacks.
    /// The canonical raw callback owns its shallow view; no Agent transcript
    /// lock is held while a callback runs.
    /// Defaults preserve existing hook implementations.
    async fn before_queued_input_dequeue_with_context(
        &self,
        _snapshot: ExecutionSnapshot,
        _messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        self.before_queued_input_dequeue(cancel).await
    }
    async fn before_model_call_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        _messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        self.before_model_call(snapshot, cancel).await
    }
    /// Canonical raw admission. A no-context callback remains a no-op for
    /// application records; only an opted-in typed context consumer is adapted.
    async fn before_queued_input_dequeue_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        if self.context_admission_enabled() {
            let messages = typed_admission_context(&messages)
                .map_err(|error| AdmissionError::Blocked(error.0))?;
            self.before_queued_input_dequeue_with_context(snapshot, messages, cancel)
                .await
        } else {
            self.before_queued_input_dequeue(cancel).await
        }
    }
    async fn before_model_call_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        if self.context_admission_enabled() {
            let messages = typed_admission_context(&messages)
                .map_err(|error| AdmissionError::Blocked(error.0))?;
            self.before_model_call_with_context(snapshot, messages, cancel)
                .await
        } else {
            self.before_model_call(snapshot, cancel).await
        }
    }
    /// Native transformContext precedes conversion. None uses the mutated Run
    /// array; Some is request-only and must never replace Agent state.
    async fn transform_agent_context(
        &self,
        _live: &mut Vec<AgentMessage>,
        _cancel: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, RawContextError> {
        Ok(None)
    }
    async fn convert_agent_messages_to_llm(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Vec<Message>, RawContextError> {
        default_agent_message_conversion(messages)
    }

    /// Obtain the admitted Host's original actual tool-source capability once.
    /// Acquisition alone reads no tool metadata and invokes no tool execution.
    /// None preserves explicit absence; it is independent of wire advertisement.
    fn actual_tool_source(
        &self,
        _values: &SessionValues,
    ) -> Result<Option<Arc<dyn ActualToolSource>>, ActualProviderError> {
        Ok(None)
    }

    /// Graph counterparts are explicit opt-in. Typed custom hooks are never
    /// silently skipped or represented by a lossy argument/history bridge.
    async fn convert_agent_messages_to_llm_actual(
        &self,
        _messages: &[AgentMessage],
        _values: &crate::provider_context::SessionValues,
    ) -> Result<Vec<ara_rpc::value_heap::ValueHandle>, crate::provider_context::ActualProviderError>
    {
        Err(crate::provider_context::ActualProviderError::Unsupported(
            "actual Agent message conversion",
        ))
    }
    async fn transform_provider_context_actual(
        &self,
        _context: crate::provider_context::ActualProviderContext,
        _model: &Model,
        _values: &crate::provider_context::SessionValues,
    ) -> Result<
        crate::provider_context::ActualProviderContext,
        crate::provider_context::ActualProviderError,
    > {
        Err(crate::provider_context::ActualProviderError::Unsupported(
            "actual provider transform",
        ))
    }
    async fn transform_assistant_graph(
        &self,
        _owner: &AgentMessage,
        _cancel: &CancellationToken,
        _values: &crate::provider_context::SessionValues,
    ) -> Result<(), crate::provider_context::ActualProviderError> {
        Err(crate::provider_context::ActualProviderError::Unsupported(
            "actual assistant transform",
        ))
    }
    fn supports_actual_turn_end(&self) -> bool {
        false
    }

    /// Full actual prepare hooks are separately opted in; absence is not a
    /// serde adapter or permission. An opted-in Host represents genuine raw
    /// hook absence with None and explicitly implements the permission gate.
    fn supports_actual_tool_hooks(&self) -> bool {
        false
    }
    async fn before_tool_dispatch_actual(
        &self,
        _context: ActualBeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ActualProviderError> {
        Err(ActualProviderError::Unsupported("actual before-tool hook"))
    }
    async fn before_tool_call_actual(
        &self,
        _context: ActualBeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<ActualToolDecision, ActualProviderError> {
        Err(ActualProviderError::Unsupported("actual tool permission"))
    }
    async fn after_tool_call_actual(
        &self,
        _context: ActualAfterToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<ara_rpc::value_heap::ValueHandle>, ActualProviderError> {
        Err(ActualProviderError::Unsupported("actual after-tool hook"))
    }
    fn transform_tool_call_arguments_actual(
        &self,
        _args: ara_rpc::value_heap::ValueHandle,
        _name: &ara_rpc::WireString,
        _values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ActualProviderError> {
        Err(ActualProviderError::Unsupported(
            "actual tool argument transform",
        ))
    }
    fn get_tool_context_actual(
        &self,
        _context: &crate::tool::ActualToolExecutionContext,
        _values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ActualProviderError> {
        Err(ActualProviderError::Unsupported("actual Host tool context"))
    }
    /// Host formats an original thrown value at the native instanceof Error /
    /// .message / String boundary using its imported Error constructor. Core
    /// never substitutes a debug description of a thrown Graph handle.
    fn tool_error_message_actual(
        &self,
        _error: &ara_rpc::value_heap::ValueError,
        _values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ActualProviderError> {
        Err(ActualProviderError::Unsupported(
            "actual thrown tool-error formatter",
        ))
    }
    /// Fixed pi-utils sanitizeText callable and Bun.stripANSI capability in
    /// the original Host runtime. No lossy Rust String/serde fallback.
    fn sanitize_tool_result_text_actual(
        &self,
        _text: &ara_rpc::value_heap::ValueHandle,
        _values: &SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ActualProviderError> {
        Err(ActualProviderError::Unsupported(
            "fixed actual sanitizeText Host callable",
        ))
    }
    /// Original soft requirement predicate receives the actual call handle.
    /// None means the Host does not bind a typed predicate into this domain.
    fn soft_tool_satisfies_actual(
        &self,
        _requirement: &crate::soft_tool_requirement::SoftToolRequirement,
        _call: &ara_rpc::value_heap::ValueHandle,
        _values: &SessionValues,
    ) -> Result<bool, ActualProviderError> {
        Err(ActualProviderError::Unsupported(
            "actual soft-tool requirement predicate",
        ))
    }
    async fn turn_end_actual(
        &self,
        _owner: &AgentMessage,
        _context: &[AgentMessage],
        _cancel: &CancellationToken,
        _values: &crate::provider_context::SessionValues,
    ) -> Result<Vec<AgentMessage>, crate::provider_context::ActualProviderError> {
        Err(crate::provider_context::ActualProviderError::Unsupported(
            "actual turn-end consumer",
        ))
    }
    async fn turn_end_actual_context(
        &self,
        context: ActualTurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<AgentMessage>, ActualProviderError> {
        self.turn_end_actual(
            context.assistant_message,
            context.context.messages,
            cancel,
            context.values,
        )
        .await
    }

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
    async fn before_tool_dispatch(
        &self,
        _context: &mut BeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        Ok(BeforeToolCallResult::default())
    }
    async fn after_tool_call(
        &self,
        _context: AfterToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<AfterToolCallResult>, ToolHookError> {
        Ok(None)
    }
    async fn transform_assistant_message(
        &self,
        _message: &mut AssistantMessage,
        _cancel: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        Ok(())
    }
    fn resolve_fallback_tool(&self, _name: &str) -> Option<Arc<dyn AgentTool>> {
        None
    }
    fn transform_tool_call_arguments(
        &self,
        args: JsonObject,
        _tool_name: &str,
    ) -> Result<JsonObject, ToolHookError> {
        Ok(args)
    }
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        None
    }
    fn irc_interrupt_port(&self) -> Option<Arc<dyn IrcInterruptPeek>> {
        None
    }
    /// Passive entries do not participate in steering/IRC peeks or tool interrupts.
    async fn aside_messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
        Ok(Vec::new())
    }
    async fn before_yield(&self) -> Result<(), AsideError> {
        Ok(())
    }
    /// Consuming dequeue of steering messages (injected at turn boundaries).
    async fn steering_messages(&self) -> Vec<Message> {
        Vec::new()
    }
    /// Consuming dequeue of follow-up messages (drained when the agent would stop).
    async fn follow_up_messages(&self) -> Vec<Message> {
        Vec::new()
    }
    /// Owned steering dequeue. Existing Message hooks retain their behavior;
    /// hosts with provenance override this method instead.
    async fn steering_inputs(&self) -> Vec<AgentInput> {
        self.steering_messages()
            .await
            .into_iter()
            .map(AgentInput::from)
            .collect()
    }
    /// Owned follow-up dequeue, adapting existing Message hooks by default.
    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        self.follow_up_messages()
            .await
            .into_iter()
            .map(AgentInput::from)
            .collect()
    }
    async fn steering_raw_messages(&self) -> Vec<AgentMessage> {
        self.steering_inputs()
            .await
            .into_iter()
            .map(AgentMessage::from)
            .collect()
    }
    async fn follow_up_raw_messages(&self) -> Vec<AgentMessage> {
        self.follow_up_inputs()
            .await
            .into_iter()
            .map(AgentMessage::from)
            .collect()
    }
    async fn poll_queued_raw_messages_with_context(
        &self,
        kind: QueueInputKind,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Vec<AgentMessage>, AdmissionError> {
        if self.queued_input_pending(kind) == Some(false) {
            await_admission(self.before_queued_input_dequeue(cancel), cancel, deadline).await?;
            return Ok(Vec::new());
        }
        await_admission(
            self.before_queued_input_dequeue_with_raw_context(snapshot, messages, cancel),
            cancel,
            deadline,
        )
        .await?;
        Ok(match kind {
            QueueInputKind::Steering => self.steering_raw_messages().await,
            QueueInputKind::FollowUp => self.follow_up_raw_messages().await,
        })
    }
    /// Non-consuming availability of an external queue. `None` retains the
    /// conservative admission used by older queue owners without a peek API.
    fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
        None
    }
    /// Queue owners can couple their non-consuming peek to this poll. A known
    /// empty poll does not consume inputs that arrive while the legacy hook waits.
    async fn poll_queued_inputs_with_context(
        &self,
        kind: QueueInputKind,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
        deadline: Option<Instant>,
    ) -> Result<Vec<AgentInput>, AdmissionError> {
        let pending = self.queued_input_pending(kind) != Some(false);
        if pending {
            await_admission(
                self.before_queued_input_dequeue_with_context(snapshot, messages, cancel),
                cancel,
                deadline,
            )
            .await?;
        } else {
            await_admission(self.before_queued_input_dequeue(cancel), cancel, deadline).await?;
            return Ok(Vec::new());
        }
        Ok(match kind {
            QueueInputKind::Steering => self.steering_inputs().await,
            QueueInputKind::FollowUp => self.follow_up_inputs().await,
        })
    }
    /// Rewrite the provider request context just before each model call (OMP
    /// `transformProviderContext`). The run's own transcript is unchanged.
    async fn transform_provider_context(&self, context: Context, _model: &Model) -> Context {
        context
    }
    /// Host inputs produced by a healthy completed turn (fixed OMP
    /// `onTurnEnd`). They enter this transcript and the awaited input sink
    /// before terminal checks or user steering; provenance stays Host-owned.
    async fn turn_end_inputs(
        &self,
        _message: &AssistantMessage,
        _tool_results: &[ToolResultMessage],
        _cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        Vec::new()
    }
    async fn turn_end_raw_messages_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentMessage> {
        self.turn_end_inputs_with_context(context, cancel)
            .await
            .into_iter()
            .map(AgentMessage::from)
            .collect()
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.turn_end_inputs(context.message, context.tool_results, cancel)
            .await
    }
}

pub struct NoHooks;
impl LoopHooks for NoHooks {
    fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
        Some(false)
    }
}

/// Host admission failure, distinct from a failed provider request. A blocked
/// admission must not be retried as if the model had been called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionError {
    Blocked(String),
    Cancelled,
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Blocked(reason) => f.write_str(reason),
            Self::Cancelled => f.write_str(ABORTED_TEXT),
        }
    }
}

impl std::error::Error for AdmissionError {}

/// Host-owned live execution context. A prepared tool keeps its adapter `Arc`
/// even if the host replaces the catalogue while that call is pending.
#[derive(Clone)]
pub struct ExecutionSnapshot {
    pub tools: Vec<Arc<dyn AgentTool>>,
    pub system_prompt: Vec<String>,
    /// Model, transport and per-call options must be adopted together. `None`
    /// preserves the Run's original model binding for tools-only hosts.
    pub model: Option<ModelExecutionSnapshot>,
    pub provider_context: Option<ProviderContextBinding>,
}

/// Portable execution fields only. Registry, account identity and authentication
/// remain behind the host-owned provider port.
#[derive(Clone)]
pub struct ModelExecutionSnapshot {
    pub model: Model,
    pub provider: Arc<dyn ModelProvider>,
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
}

impl ExecutionSnapshot {
    pub fn from_config(config: &AgentConfig) -> Self {
        Self {
            tools: config.tools.clone(),
            system_prompt: config.system_prompt.clone(),
            model: Some(ModelExecutionSnapshot {
                model: config.model.clone(),
                provider: config.provider.clone(),
                tool_choice: config.tool_choice.clone(),
                max_tokens: config.max_tokens,
                temperature: config.temperature,
            }),
            provider_context: config.hooks.provider_context_binding(),
        }
    }
}

struct AdmittedExecution {
    config: AgentConfig,
    provider_context: Option<ProviderContextBinding>,
}

impl AdmittedExecution {
    fn tracing(&self) -> bool {
        self.provider_context
            .as_ref()
            .is_some_and(|binding| binding.build_options.intent_tracing)
    }
}

impl std::ops::Deref for AdmittedExecution {
    type Target = AgentConfig;
    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

fn execution_from_snapshot(
    config: &AgentConfig,
    mut snapshot: ExecutionSnapshot,
) -> AdmittedExecution {
    let provider_context = snapshot.provider_context.take();
    AdmittedExecution {
        config: config_from_snapshot(config, snapshot),
        provider_context,
    }
}

fn execution_config(config: &AgentConfig) -> AdmittedExecution {
    let snapshot = config
        .hooks
        .execution_snapshot()
        .unwrap_or_else(|| ExecutionSnapshot::from_config(config));
    execution_from_snapshot(config, snapshot)
}

fn config_from_snapshot(config: &AgentConfig, snapshot: ExecutionSnapshot) -> AgentConfig {
    let mut live = config.clone();
    live.tools = snapshot.tools;
    live.system_prompt = snapshot.system_prompt;
    if let Some(model) = snapshot.model {
        live.model = model.model;
        live.provider = model.provider;
        live.tool_choice = model.tool_choice;
        live.max_tokens = model.max_tokens;
        live.temperature = model.temperature;
    }
    live
}

pub(crate) async fn await_admission<T>(
    future: impl std::future::Future<Output = Result<T, AdmissionError>>,
    cancel: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<T, AdmissionError> {
    let cancelled = || cancel.is_cancelled() || deadline.is_some_and(|d| Instant::now() >= d);
    if cancelled() {
        return Err(AdmissionError::Cancelled);
    }
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(AdmissionError::Cancelled),
        _ = async {
            if let Some(deadline) = deadline {
                tokio::time::sleep_until(deadline.into()).await;
            } else {
                std::future::pending::<()>().await;
            }
        } => Err(AdmissionError::Cancelled),
        result = future => result,
    };
    // Cancellation may race the hook's successful return. Never consume an
    // input or dispatch the admitted model after that race.
    if cancelled() {
        Err(AdmissionError::Cancelled)
    } else {
        result
    }
}

pub(crate) async fn poll_queued_input(
    config: &AgentConfig,
    messages: &[AgentMessage],
    cancel: &CancellationToken,
    kind: QueueInputKind,
) -> Result<Vec<AgentMessage>, AdmissionError> {
    let inputs = if !config.hooks.context_admission_enabled() {
        await_admission(
            config.hooks.before_queued_input_dequeue(cancel),
            cancel,
            config.deadline,
        )
        .await?;
        match kind {
            QueueInputKind::Steering => config.hooks.steering_raw_messages().await,
            QueueInputKind::FollowUp => config.hooks.follow_up_raw_messages().await,
        }
    } else {
        let snapshot = config
            .hooks
            .execution_snapshot()
            .unwrap_or_else(|| ExecutionSnapshot::from_config(config));
        config
            .hooks
            .poll_queued_raw_messages_with_context(
                kind,
                snapshot,
                messages.to_vec(),
                cancel,
                config.deadline,
            )
            .await?
    };
    for input in &inputs {
        input
            .adopt()
            .map_err(|error| AdmissionError::Blocked(error.to_string()))?;
    }
    Ok(inputs)
}

async fn admit_model(
    config: &AgentConfig,
    messages: &[AgentMessage],
    cancel: &CancellationToken,
) -> Result<AdmittedExecution, AdmissionError> {
    let snapshot = config
        .hooks
        .execution_snapshot()
        .unwrap_or_else(|| ExecutionSnapshot::from_config(config));
    let snapshot = if config.hooks.context_admission_enabled() {
        await_admission(
            config
                .hooks
                .before_model_call_with_raw_context(snapshot, messages.to_vec(), cancel),
            cancel,
            config.deadline,
        )
        .await?
    } else {
        await_admission(
            config.hooks.before_model_call(snapshot, cancel),
            cancel,
            config.deadline,
        )
        .await?
    };
    Ok(execution_from_snapshot(config, snapshot))
}

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
    /// Captured for the Run, even if the Agent's default mode changes later.
    pub interrupt_mode: InterruptMode,
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
    state: Arc<Mutex<RunStopState>>,
    _guard: tokio_util::sync::DropGuard,
    timer: Option<tokio::task::JoinHandle<()>>,
}

struct RunStopState {
    active: bool,
    cause: Option<RunStopCause>,
    parent: CancellationToken,
    abort_reason: Option<RunAbortReason>,
    token: CancellationToken,
    deadline: Option<Instant>,
}
impl RunStopState {
    fn observe_external(&mut self) {
        if self.cause.is_some() && !matches!(self.cause, Some(RunStopCause::PreModelStop { .. })) {
            return;
        }
        let external = if self.parent.is_cancelled() {
            Some(
                match self.abort_reason.as_ref().and_then(RunAbortReason::get) {
                    Some(reason) => RunStopCause::ToolScopedAbort { reason },
                    None => RunStopCause::ExternalAbort,
                },
            )
        } else if self.deadline.is_some_and(|d| Instant::now() >= d) {
            Some(RunStopCause::Deadline)
        } else {
            None
        };
        if external.is_some() {
            self.cause = external;
        }
    }
}

impl RunControl {
    fn pre_model_stop(&self, reason: Option<String>) -> RunEnd {
        let mut state = self.state.lock().unwrap();
        state.observe_external();
        match &state.cause {
            Some(RunStopCause::ExternalAbort | RunStopCause::ToolScopedAbort { .. }) => {
                RunEnd::Aborted
            }
            Some(RunStopCause::Deadline) => RunEnd::Deadline,
            Some(RunStopCause::TerminalToolResult { .. }) => RunEnd::TerminalToolResult,
            _ => {
                state.cause = Some(RunStopCause::PreModelStop { reason });
                RunEnd::PreModelStop
            }
        }
    }

    fn gate_external_end(&self) -> Option<RunEnd> {
        match self.stop_cause() {
            Some(RunStopCause::ExternalAbort | RunStopCause::ToolScopedAbort { .. }) => {
                Some(RunEnd::Aborted)
            }
            Some(RunStopCause::Deadline) => Some(RunEnd::Deadline),
            Some(RunStopCause::TerminalToolResult { .. }) => Some(RunEnd::TerminalToolResult),
            _ => None,
        }
    }

    fn new(parent: &CancellationToken, deadline: Option<Instant>) -> RunControl {
        Self::new_with_reason(parent, deadline, None)
    }
    fn new_with_reason(
        parent: &CancellationToken,
        deadline: Option<Instant>,
        abort_reason: Option<RunAbortReason>,
    ) -> RunControl {
        let token = parent.child_token();
        let state = Arc::new(Mutex::new(RunStopState {
            active: true,
            cause: None,
            parent: parent.clone(),
            abort_reason,
            token: token.clone(),
            deadline,
        }));
        let timer = deadline.map(|d| {
            let (token, state) = (token.clone(), state.clone());
            tokio::spawn(async move {
                tokio::select! {
                    _ = tokio::time::sleep_until(d.into()) => {
                        state.lock().unwrap().observe_external();
                        token.cancel();
                    }
                    _ = token.cancelled() => {}
                }
            })
        });
        RunControl {
            _guard: token.clone().drop_guard(),
            token,
            state,
            timer,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    fn deadline_fired(&self) -> bool {
        self.stop_cause() == Some(RunStopCause::Deadline)
    }

    fn abort_text(&self) -> String {
        match self.stop_cause() {
            Some(RunStopCause::Deadline) => DEADLINE_TEXT.to_string(),
            Some(RunStopCause::ToolScopedAbort { reason }) => reason.message,
            _ => ABORTED_TEXT.to_string(),
        }
    }

    fn tool_abort_text(&self, id: &ara_rpc::WireString) -> String {
        match self.stop_cause() {
            Some(RunStopCause::ToolScopedAbort { reason }) => reason.for_tool(id).to_owned(),
            _ => self.abort_text(),
        }
    }
    fn scoped_abort(&self) -> Option<ToolScopedAbortReason> {
        match self.stop_cause() {
            Some(RunStopCause::ToolScopedAbort { reason }) => Some(reason),
            _ => None,
        }
    }
    fn stop_cause(&self) -> Option<RunStopCause> {
        let mut state = self.state.lock().unwrap();
        state.observe_external();
        state.cause.clone()
    }

    fn terminal_result(&self) -> bool {
        matches!(
            self.stop_cause(),
            Some(RunStopCause::TerminalToolResult { .. })
        )
    }

    fn terminal_handle(&self, tool_call_id: &str) -> TerminalToolResultHandle {
        TerminalToolResultHandle {
            state: Arc::downgrade(&self.state),
            tool_call_id: tool_call_id.to_string(),
        }
    }

    fn cancelled_end(&self) -> RunEnd {
        match self.stop_cause() {
            Some(RunStopCause::TerminalToolResult { .. }) => RunEnd::TerminalToolResult,
            Some(RunStopCause::Deadline) => RunEnd::Deadline,
            _ => RunEnd::Aborted,
        }
    }

    fn turn_end_token(&self) -> CancellationToken {
        if self.terminal_result() {
            self.state.lock().unwrap().parent.child_token()
        } else {
            self.token.clone()
        }
    }
}

impl Drop for RunControl {
    fn drop(&mut self) {
        self.state.lock().unwrap().active = false;
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
    /// A completed result explicitly stopped the Run through its scoped handle.
    TerminalToolResult,
    /// The owner refused a prepared provider request without sending it.
    PreModelStop,
}

/// Messages added by a run and why it ended.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub messages: Vec<AgentMessage>,
    pub end: RunEnd,
    pub stop_cause: Option<RunStopCause>,
}
impl RunReport {
    pub fn model_messages(&self) -> Result<Vec<Message>, RawContextError> {
        checked_model_messages(&self.messages)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoopError {
    CannotContinue(String),
    Aside(AsideError),
    RawContext(RawContextError),
}

impl std::fmt::Display for LoopError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoopError::CannotContinue(m) => f.write_str(m),
            LoopError::Aside(error) => error.fmt(f),
            LoopError::RawContext(error) => error.fmt(f),
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
            if matches!(a.stop_reason, StopReason::ToolUse | StopReason::Stop)
                && a.tool_calls().next().is_some() =>
        {
            Some(a)
        }
        _ => None,
    }
}

/// Inspect only the actual raw tail, never the converter or a filtered view.
pub fn unpaired_raw_tool_call_tail(
    messages: &[AgentMessage],
) -> Result<Option<AssistantMessage>, RawContextError> {
    let Some(last) = messages.last() else {
        return Ok(None);
    };
    if matches!(last.body_snapshot(), AgentMessageBody::Graph(_)) {
        let role = last
            .graph_stream_tail_role()
            .map_err(|error| RawContextError(error.to_string()))?;
        if role != ara_rpc::WireString::from("assistant") {
            return Ok(None);
        }
        return Err(RawContextError(
            "Graph tail requires an actual Graph preparation consumer".into(),
        ));
    }
    Ok(match last.checked_model_snapshot()? {
        Some(Message::Assistant(message))
            if matches!(message.stop_reason, StopReason::ToolUse | StopReason::Stop)
                && message.tool_calls().next().is_some() =>
        {
            Some(message)
        }
        _ => None,
    })
}

/// Same canonical tail observation for callers that need only the recovery
/// guard. The typed public convenience retains its explicit Graph boundary.
pub(crate) fn raw_has_unpaired_tool_calls(
    messages: &[AgentMessage],
) -> Result<bool, RawContextError> {
    if let Some(tail) = messages.last()
        && matches!(tail.body_snapshot(), AgentMessageBody::Graph(_))
    {
        let (role, runnable, calls, _) = tail
            .graph_identity_controls()
            .map_err(|error| RawContextError(error.to_string()))?;
        return Ok(role == ara_rpc::WireString::from("assistant") && runnable && !calls.is_empty());
    }
    Ok(unpaired_raw_tool_call_tail(messages)?.is_some())
}

/// Start a run with new prompt messages (OMP `agentLoop`). `context` is the
/// full transcript and is extended in place; the new messages are returned.
pub async fn agent_loop(
    prompts: Vec<Message>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> RunReport {
    agent_loop_raw(
        prompts.into_iter().map(AgentMessage::from).collect(),
        context,
        config,
        cancel,
        sink,
    )
    .await
}

/// Canonical raw input/context/output. Provider fragments are derived only
/// during preparation; opaque source stays on the original raw record.
pub async fn agent_loop_raw(
    prompts: Vec<AgentMessage>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> RunReport {
    // Preserve this original direct API's RunReport signature. Hosts requiring
    // typed callback errors use try_agent_loop_raw or the stateful Agent API.
    try_agent_loop_raw(prompts, context, config, cancel, sink)
        .await
        .unwrap_or_else(|error| std::panic::panic_any(error))
}

pub async fn try_agent_loop_raw(
    prompts: Vec<AgentMessage>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> Result<RunReport, AsideError> {
    let gate = config
        .hooks
        .final_provider_gate_binding()
        .and_then(|binding| binding.sample_for_run());
    agent_loop_raw_with_gate(prompts, context, config, cancel, sink, gate).await
}

/// The stateful Agent already sampled before its owned Run's first await.
pub(crate) async fn agent_loop_raw_with_gate(
    prompts: Vec<AgentMessage>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
    gate: Option<FinalProviderGateBinding>,
) -> Result<RunReport, AsideError> {
    for prompt in &prompts {
        prompt
            .adopt()
            .map_err(|error| AsideError(error.to_string()))?;
    }
    let ctl = RunControl::new_with_reason(cancel, config.deadline, sink.abort_reason());
    context.extend(prompts.iter().cloned());
    let mut new_messages = prompts.to_vec();
    // Native initial commits precede AgentStart and the pending finally scope.
    for prompt in &prompts {
        prompt.commit_aside()?;
    }
    sink.emit(AgentEvent::AgentStart).await;
    let end = run_loop(
        context,
        &mut new_messages,
        config,
        &ctl,
        sink,
        prompts,
        gate,
    )
    .await?;
    Ok(RunReport {
        messages: new_messages,
        end,
        stop_cause: ctl.stop_cause(),
    })
}

/// Typed input compatibility; the borrowed transcript is still canonical raw.
pub async fn agent_loop_inputs(
    prompts: Vec<AgentInput>,
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> RunReport {
    agent_loop_raw(
        prompts.into_iter().map(AgentMessage::from).collect(),
        context,
        config,
        cancel,
        sink,
    )
    .await
}

/// Owned typed convenience returns the raw transcript without lossy write-back.
pub async fn agent_loop_model(
    prompts: Vec<Message>,
    context: Vec<Message>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> (Vec<AgentMessage>, RunReport) {
    let mut context = context.into_iter().map(AgentMessage::from).collect();
    let report = agent_loop(prompts, &mut context, config, cancel, sink).await;
    (context, report)
}

/// Continue from the current transcript without a new message
/// (OMP `agentLoopContinue`).
pub async fn agent_loop_continue(
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
    tail: UnpairedTail,
) -> Result<RunReport, LoopError> {
    let gate = config
        .hooks
        .final_provider_gate_binding()
        .and_then(|binding| binding.sample_for_run());
    agent_loop_continue_with_gate(context, config, cancel, sink, tail, gate).await
}

/// Explicit raw continuation entry; inspects the actual tail without conversion.
pub async fn agent_loop_continue_raw(
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
    tail: UnpairedTail,
) -> Result<RunReport, LoopError> {
    agent_loop_continue(context, config, cancel, sink, tail).await
}

pub(crate) async fn agent_loop_continue_with_gate(
    context: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
    tail: UnpairedTail,
    gate: Option<FinalProviderGateBinding>,
) -> Result<RunReport, LoopError> {
    let Some(last) = context.last() else {
        return Err(LoopError::CannotContinue(
            "Cannot continue: no messages in context".into(),
        ));
    };
    let unpaired = raw_has_unpaired_tool_calls(context).map_err(LoopError::RawContext)?;
    let graph_assistant = if matches!(last.body_snapshot(), AgentMessageBody::Graph(_)) {
        last.graph_stream_tail_role()
            .map_err(|error| LoopError::RawContext(RawContextError(error.to_string())))?
            == ara_rpc::WireString::from("assistant")
    } else {
        false
    };
    let tail_model = if matches!(last.body_snapshot(), AgentMessageBody::Graph(_)) {
        None
    } else {
        last.checked_model_snapshot()
            .map_err(LoopError::RawContext)?
    };
    if (graph_assistant || matches!(tail_model, Some(Message::Assistant(_))))
        && !unpaired
        && !tail_model.as_ref().is_some_and(|message| {
            ara_ai::remote_compaction::is_native_compaction_carrier(message, &config.model)
        })
    {
        return Err(LoopError::CannotContinue(
            "Cannot continue from message role: assistant".into(),
        ));
    }
    if unpaired && tail == UnpairedTail::Refuse {
        return Err(LoopError::CannotContinue(UNPAIRED_TAIL_REFUSED.into()));
    }
    let ctl = RunControl::new_with_reason(cancel, config.deadline, sink.abort_reason());
    let mut new_messages = Vec::new();
    sink.emit(AgentEvent::AgentStart).await;
    let end = run_loop(
        context,
        &mut new_messages,
        config,
        &ctl,
        sink,
        Vec::new(),
        gate,
    )
    .await
    .map_err(LoopError::Aside)?;
    Ok(RunReport {
        messages: new_messages,
        end,
        stop_cause: ctl.stop_cause(),
    })
}

async fn emit_inputs(sink: &dyn AgentEventSink, inputs: &[AgentMessage]) {
    for input in inputs {
        emit_raw_input(sink, input.clone()).await;
    }
}

// Keep the existing turn boundary inputs explicit without reshaping canonical message ownership.
#[allow(clippy::too_many_arguments)]
async fn emit_turn_end(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    message: AssistantMessage,
    tool_results: Vec<ToolReceipt>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    will_continue: bool,
) {
    let tool_results: Vec<ToolResultMessage> = tool_results
        .into_iter()
        .map(|receipt| receipt.model)
        .collect();
    sink.emit(AgentEvent::TurnEnd {
        message: Message::Assistant(message.clone()),
        tool_results: tool_results.clone(),
    })
    .await;
    // Fixed emitTurnEnd skips a user abort, deadline or failed assistant. Once
    // the hook returns inputs, commit them even if cancellation raced the hook.
    if (ctl.is_cancelled() && !ctl.terminal_result())
        || matches!(message.stop_reason, StopReason::Error | StopReason::Aborted)
    {
        return;
    }
    let stop_cause = ctl.stop_cause();
    let cancel = ctl.turn_end_token();
    for input in config
        .hooks
        .turn_end_raw_messages_with_context(
            TurnEndContext {
                message: &message,
                tool_results: &tool_results,
                context: AgentContextView {
                    system_prompt: &config.system_prompt,
                    messages: context,
                    tools: &config.tools,
                },
                will_continue,
                stop_cause: stop_cause.as_ref(),
            },
            &cancel,
        )
        .await
    {
        context.push(input.clone());
        new_messages.push(input.clone());
        emit_raw_input(sink, input).await;
    }
}

async fn end(sink: &dyn AgentEventSink, new_messages: &[AgentMessage], reason: RunEnd) -> RunEnd {
    sink.emit(AgentEvent::AgentEnd {
        messages: new_messages.to_vec(),
    })
    .await;
    reason
}

async fn finish_admission_failure(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    error: AdmissionError,
    inputs: Option<Vec<AgentMessage>>,
) -> RunEnd {
    if let Some(inputs) = inputs {
        sink.emit(AgentEvent::TurnStart).await;
        emit_inputs(sink, &inputs).await;
    }
    // Fixed emitAbortedAssistantMessage reads the live selected model, including
    // a Host model switch while the just-settled tool was still running.
    // Capture model and its current Session value binding together. Dynamic
    // Hosts publish both in one snapshot; static Hosts expose the binding once.
    let snapshot = config.hooks.execution_snapshot();
    let current_model = if matches!(&error, AdmissionError::Cancelled) {
        snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.model.as_ref())
            .map(|selection| &selection.model)
    } else {
        None
    };
    let actual_values = match &snapshot {
        Some(snapshot) => snapshot
            .provider_context
            .as_ref()
            .and_then(|binding| binding.actual_values.clone()),
        None => config
            .hooks
            .provider_context_binding()
            .and_then(|binding| binding.actual_values),
    };
    let model = current_model.unwrap_or(&config.model);
    let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    let reason = match error {
        AdmissionError::Blocked(reason) => {
            message.stop_reason = StopReason::Error;
            message.failure_evidence =
                Some(ara_ai::ProviderError::Config(reason.clone()).failure_evidence(true));
            message.error_message = Some(reason);
            RunEnd::Error
        }
        AdmissionError::Cancelled => {
            message.stop_reason = StopReason::Aborted;
            let deadline = deadline_passed(config) || ctl.deadline_fired();
            message.error_message = Some(if deadline {
                DEADLINE_TEXT.into()
            } else {
                ctl.abort_text()
            });
            if deadline {
                RunEnd::Deadline
            } else {
                RunEnd::Aborted
            }
        }
    };
    let message = if let Some(values) = actual_values {
        // Only this newly authored empty failure metadata is imported. No
        // received/historical Graph root, argument or tool effect is converted.
        let source = serde_json::to_value(Message::Assistant(message.clone()))
            .expect("synthetic admission metadata serializes");
        let staged = values
            .heap()
            .import_serde(&source)
            .map_err(ActualProviderError::from)
            .and_then(|root| {
                let owner = AgentMessage::from_stream_graph(root.clone(), &values)
                    .map_err(ActualProviderError::from)?;
                Ok((owner, root))
            });
        let (owner, failure) = match staged {
            Ok((owner, root)) => {
                let failure = owner
                    .install_final_graph(root, &values)
                    .err()
                    .map(ActualProviderError::from);
                // Failed final pin retains this same newly created Graph owner.
                (owner, failure)
            }
            Err(error) => {
                // No Graph owner was created. Preserve the original metadata
                // failure output and expose the creation failure explicitly.
                (
                    AgentMessage::from_model(Message::Assistant(message.clone())),
                    Some(error),
                )
            }
        };
        context.push(owner.clone());
        sink.emit(AgentEvent::MessageStart {
            message: owner.clone(),
        })
        .await;
        if let Some(error) = failure {
            sink.emit(AgentEvent::ActualMessageEnd {
                message: owner,
                finalization_error: Arc::new(error),
            })
            .await;
        } else {
            sink.emit(AgentEvent::MessageEnd { message: owner }).await;
        }
        message
    } else {
        commit_final(context, sink, message, false, None).await
    };
    new_messages.push(context.last().expect("final Assistant retained").clone());
    emit_turn_end(
        context,
        new_messages,
        message,
        Vec::new(),
        config,
        ctl,
        sink,
        false,
    )
    .await;
    end(sink, new_messages, reason).await
}

async fn main_raw_request_context(
    config: &AgentConfig,
    live: &mut Vec<AgentMessage>,
    binding: Option<&ProviderContextBinding>,
    ctl: &RunControl,
) -> Result<PreparedProviderCall, ProviderContextError> {
    enum Converted {
        Typed(Vec<Message>),
        Actual(Vec<ara_rpc::value_heap::ValueHandle>, Arc<SessionValues>),
    }
    let actual_values = binding.and_then(|binding| binding.actual_values.clone());
    let preparation_failure = |message: String| {
        if actual_values.is_some() {
            ActualProviderError::Callback(message).into()
        } else {
            ProviderContextError::Schema(message)
        }
    };
    let preparation = std::panic::AssertUnwindSafe(async {
        let transformed = config
            .hooks
            .transform_agent_context(live, &ctl.token)
            .await
            .map_err(|error| preparation_failure(error.0))?;
        let view = transformed.as_deref().unwrap_or(live.as_slice());
        if let Some(values) = &actual_values {
            let messages = config
                .hooks
                .convert_agent_messages_to_llm_actual(view, values)
                .await?;
            Ok::<_, ProviderContextError>(Converted::Actual(messages, values.clone()))
        } else {
            config
                .hooks
                .convert_agent_messages_to_llm(view)
                .await
                .map(Converted::Typed)
                .map_err(|error| ProviderContextError::Schema(error.0))
        }
    })
    .catch_unwind();
    let messages = tokio::select! {
        biased;
        _ = ctl.token.cancelled() => return Err(preparation_failure(ctl.abort_text())),
        result = preparation => match result {
            Ok(Ok(messages)) => messages,
            Ok(Err(error)) => return Err(error),
            Err(panic) => return Err(preparation_failure(format!("Raw Agent context preparation panicked: {}", panic_text(panic)))),
        },
    };
    if ctl.gate_external_end().is_some() {
        return Err(preparation_failure(ctl.abort_text()));
    }
    match messages {
        Converted::Typed(messages) => main_request_context(config, &messages, binding).await,
        Converted::Actual(messages, values) => {
            let binding = binding.expect("actual selection came from this captured binding");
            let preparation = std::panic::AssertUnwindSafe(main_actual_request_context(
                config, messages, binding, values,
            ))
            .catch_unwind();
            tokio::select! {
                biased;
                _ = ctl.token.cancelled() => Err(preparation_failure(ctl.abort_text())),
                result = preparation => match result {
                    Ok(result) => result,
                    Err(panic) => Err(preparation_failure(format!("Actual Provider preparation panicked: {}", panic_text(panic)))),
                },
            }
        }
    }
}

// Keep the closed-turn failure inputs explicit without changing failure settlement ordering.
#[allow(clippy::too_many_arguments)]
async fn finish_provider_preparation_failure(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    execution: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    error: ProviderContextError,
    inputs: Vec<AgentMessage>,
) -> RunEnd {
    // The no-gate provider failure keeps its existing visible result. Only its
    // preparation now precedes TurnStart, as Native prepareProviderCall does.
    sink.emit(AgentEvent::TurnStart).await;
    emit_inputs(sink, &inputs).await;
    let mut message = AssistantMessage::empty(
        &execution.model.api,
        &execution.model.provider,
        &execution.model.id,
    );
    let reason = ctl.gate_external_end().unwrap_or(RunEnd::Error);
    message.stop_reason = if matches!(reason, RunEnd::Aborted | RunEnd::Deadline) {
        StopReason::Aborted
    } else {
        StopReason::Error
    };
    message.error_message = Some(if message.stop_reason == StopReason::Aborted {
        ctl.abort_text()
    } else {
        error.to_string()
    });
    let message = commit_final(context, sink, message, false, Some(execution)).await;
    new_messages.push(context.last().expect("final Assistant retained").clone());
    if let ProviderContextError::Actual(error) = error {
        sink.emit(AgentEvent::ActualTurnEnd {
            message: context
                .last()
                .expect("preparation failure retained")
                .clone(),
            tool_results: Vec::new(),
            preparation_error: Some(error),
        })
        .await;
    } else {
        emit_turn_end(
            context,
            new_messages,
            message,
            Vec::new(),
            config,
            ctl,
            sink,
            false,
        )
        .await;
    }
    end(sink, new_messages, reason).await
}

// Keep the gate failure inputs explicit without changing cancellation and event ordering.
#[allow(clippy::too_many_arguments)]
async fn finish_final_provider_failure(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    execution: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    error: FinalProviderGateError,
    inputs: Vec<AgentMessage>,
) -> RunEnd {
    sink.emit(AgentEvent::TurnStart).await;
    emit_inputs(sink, &inputs).await;
    let mut message = AssistantMessage::empty(
        &execution.model.api,
        &execution.model.provider,
        &execution.model.id,
    );
    let reason = if let Some(reason) = ctl.gate_external_end() {
        message.stop_reason = StopReason::Aborted;
        message.error_message = Some(ctl.abort_text());
        // Native Agent catch appends an abort but emits neither its message
        // events nor TurnEnd when a thrown gate races the external Run signal.
        let raw = AgentMessage::from_owned_model(Message::Assistant(message), None);
        context.push(raw.clone());
        new_messages.push(raw.clone());
        sink.commit_unemitted_message(raw).await;
        reason
    } else {
        message.stop_reason = StopReason::Error;
        message.error_message = Some(error.0.clone());
        message.failure_evidence =
            Some(ara_ai::ProviderError::Config(error.0).failure_evidence(true));
        let message = commit_final(context, sink, message, false, None).await;
        new_messages.push(context.last().expect("final Assistant retained").clone());
        emit_turn_end(
            context,
            new_messages,
            message,
            Vec::new(),
            config,
            ctl,
            sink,
            false,
        )
        .await;
        RunEnd::Error
    };
    end(sink, new_messages, reason).await;
    ctl.gate_external_end().unwrap_or(reason)
}

/// A post-Assistant directive failure occurs inside the already open turn.
/// The Native Agent catch emits a fresh error message without another start.
async fn finish_open_directive_failure(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    execution: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    error: FinalProviderGateError,
) -> RunEnd {
    let mut message = AssistantMessage::empty(
        &execution.model.api,
        &execution.model.provider,
        &execution.model.id,
    );
    let reason = if let Some(reason) = ctl.gate_external_end() {
        message.stop_reason = StopReason::Aborted;
        message.error_message = Some(ctl.abort_text());
        let raw = AgentMessage::from_owned_model(Message::Assistant(message), None);
        context.push(raw.clone());
        new_messages.push(raw.clone());
        sink.commit_unemitted_message(raw).await;
        reason
    } else {
        message.stop_reason = StopReason::Error;
        message.error_message = Some(error.0.clone());
        message.failure_evidence =
            Some(ara_ai::ProviderError::Config(error.0).failure_evidence(true));
        let message = commit_final(context, sink, message, false, None).await;
        new_messages.push(
            context
                .last()
                .expect("directive error Assistant retained")
                .clone(),
        );
        emit_turn_end(
            context,
            new_messages,
            message,
            Vec::new(),
            config,
            ctl,
            sink,
            false,
        )
        .await;
        RunEnd::Error
    };
    end(sink, new_messages, reason).await;
    ctl.gate_external_end().unwrap_or(reason)
}

// Keep the gate stop inputs explicit without changing closed-turn settlement ordering.
#[allow(clippy::too_many_arguments)]
async fn finish_final_provider_stop(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    execution: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    stop: Option<FinalProviderGateStop>,
    inputs: Vec<AgentMessage>,
) -> RunEnd {
    // This is a closed-turn boundary. Harmony's already-open retry boundary
    // remains a required integration and must never reject an already served
    // directive or reuse this closed-turn finalizer.
    if ctl.gate_external_end().is_none() {
        let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            config.hooks.on_final_provider_gate_rejected()
        }))
        .unwrap_or_else(|payload| {
            Err(FinalProviderGateError::panicked(
                "Final provider gate rejection",
                payload,
            ))
        });
        if let Err(error) = rejected {
            return finish_final_provider_failure(
                context,
                new_messages,
                config,
                execution,
                ctl,
                sink,
                error,
                inputs,
            )
            .await;
        }
    }
    emit_inputs(sink, &inputs).await;
    let reason = if let Some(reason) = ctl.gate_external_end() {
        reason
    } else {
        ctl.pre_model_stop(stop.and_then(|stop| stop.reason))
    };
    end(sink, new_messages, reason).await;
    ctl.gate_external_end().unwrap_or(reason)
}

// Keep dispatcher state behind a heap boundary for direct CLI/REPL callers.
// This changes allocation only: ownership, cancellation and polling stay local.
// Pending origin controls only the existing ARA ordinary-queue preservation
// paths. Callback finalization still retains the whole batch until every commit
// succeeds, matching Native's non-linear exception behavior.
#[derive(Default)]
struct PendingInputs(Vec<(AgentMessage, bool)>);

impl PendingInputs {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn set_normal(&mut self, messages: Vec<AgentMessage>) {
        self.0 = messages
            .into_iter()
            .map(|message| (message, false))
            .collect();
    }

    fn extend_asides(&mut self, messages: Vec<AgentMessage>) {
        self.0
            .extend(messages.into_iter().map(|message| (message, true)));
    }

    fn insert(
        &mut self,
        context: &mut Vec<AgentMessage>,
        added: &mut Vec<AgentMessage>,
        emit: &mut Vec<AgentMessage>,
    ) -> Result<(), AsideError> {
        for (message, _) in &self.0 {
            context.push(message.clone());
            added.push(message.clone());
            emit.push(message.clone());
            message.commit_aside()?;
        }
        self.0.clear();
        Ok(())
    }

    fn preserve_normal(
        &mut self,
        context: &mut Vec<AgentMessage>,
        added: &mut Vec<AgentMessage>,
        emit: &mut Vec<AgentMessage>,
    ) -> Result<(), AsideError> {
        for (message, aside) in &self.0 {
            if !aside {
                context.push(message.clone());
                added.push(message.clone());
                emit.push(message.clone());
                message.commit_aside()?;
            }
        }
        self.0.retain(|(_, aside)| *aside);
        Ok(())
    }

    fn discard(&self) -> Result<(), AsideError> {
        let reason =
            AsideError("Aside message was not committed before the agent loop ended".into());
        for (message, _) in &self.0 {
            message.discard_aside(&reason)?;
        }
        Ok(())
    }
}

async fn poll_aside_inputs(config: &AgentConfig) -> Result<Vec<AgentMessage>, AsideError> {
    let entries = std::panic::AssertUnwindSafe(config.hooks.aside_messages())
        .catch_unwind()
        .await
        .unwrap_or_else(|payload| {
            Err(AsideError(format!(
                "Aside provider panicked: {}",
                panic_text(payload)
            )))
        })?;
    resolve_asides(entries)
}

async fn before_yield(config: &AgentConfig) -> Result<(), AsideError> {
    std::panic::AssertUnwindSafe(config.hooks.before_yield())
        .catch_unwind()
        .await
        .unwrap_or_else(|payload| {
            Err(AsideError(format!(
                "Before yield panicked: {}",
                panic_text(payload)
            )))
        })
}

fn run_loop<'a>(
    context: &'a mut Vec<AgentMessage>,
    new_messages: &'a mut Vec<AgentMessage>,
    config: &'a AgentConfig,
    ctl: &'a RunControl,
    sink: &'a dyn AgentEventSink,
    initial: Vec<AgentMessage>,
    gate: Option<FinalProviderGateBinding>,
) -> Pin<Box<impl Future<Output = Result<RunEnd, AsideError>> + 'a>> {
    Box::pin(async move {
        let soft_state = config
            .hooks
            .soft_tool_requirement_state()
            .unwrap_or_default();
        let guard = SoftToolRequirementRunGuard::new(soft_state.clone());
        let mut pending = PendingInputs::default();
        let result = std::panic::AssertUnwindSafe(run_loop_body(
            context,
            new_messages,
            config,
            ctl,
            sink,
            initial,
            gate,
            &soft_state,
            &mut pending,
        ))
        .catch_unwind()
        .await;
        // Native emits end inside the body, then finally discards. A discard
        // failure replaces the prior error and skips its following Soft reset.
        if let Err(error) = pending.discard() {
            guard.retain_after_aside_discard_failure();
            return Err(error);
        }
        match result {
            Ok(Ok(end)) => {
                guard.finish(end, &ctl.stop_cause());
                Ok(end)
            }
            Ok(Err(error)) => Err(error),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

// Keep the dispatcher inputs explicit without introducing a new shared lifecycle state.
#[allow(clippy::too_many_arguments)]
fn run_loop_body<'a>(
    context: &'a mut Vec<AgentMessage>,
    new_messages: &'a mut Vec<AgentMessage>,
    config: &'a AgentConfig,
    ctl: &'a RunControl,
    sink: &'a dyn AgentEventSink,
    initial: Vec<AgentMessage>,
    gate: Option<FinalProviderGateBinding>,
    soft_state: &'a SoftToolRequirementState,
    pending: &'a mut PendingInputs,
) -> Pin<Box<impl Future<Output = Result<RunEnd, AsideError>> + 'a>> {
    Box::pin(run_loop_body_owned(
        context,
        new_messages,
        config,
        ctl,
        sink,
        initial,
        gate,
        soft_state,
        pending,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn run_loop_body_owned(
    context: &mut Vec<AgentMessage>,
    new_messages: &mut Vec<AgentMessage>,
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    initial: Vec<AgentMessage>,
    gate: Option<FinalProviderGateBinding>,
    soft_state: &SoftToolRequirementState,
    pending: &mut PendingInputs,
) -> Result<RunEnd, AsideError> {
    let mut to_emit = initial;
    if deadline_passed(config) {
        emit_inputs(sink, &to_emit).await;
        return Ok(end(sink, new_messages, RunEnd::Deadline).await);
    }
    pending.set_normal(if ctl.is_cancelled() {
        Vec::new()
    } else {
        match poll_queued_input(config, context, &ctl.token, QueueInputKind::Steering).await {
            Ok(inputs) => inputs,
            Err(error) => {
                return Ok(finish_admission_failure(
                    context,
                    new_messages,
                    config,
                    ctl,
                    sink,
                    error,
                    Some(to_emit),
                )
                .await);
            }
        }
    });

    let graph_tail = if let Some(tail) = context.last()
        && matches!(tail.body_snapshot(), AgentMessageBody::Graph(_))
        && raw_has_unpaired_tool_calls(context).map_err(|error| AsideError(error.to_string()))?
    {
        Some(tail.clone())
    } else {
        None
    };
    if let Some(tail) = graph_tail {
        sink.emit(AgentEvent::TurnStart).await;
        emit_inputs(sink, &std::mem::take(&mut to_emit)).await;
        let execution = execution_config(config);
        let final_assistant =
            match actual::prepare_owned_actual_tail(tail.clone(), context, &execution, ctl).await {
                Ok(final_assistant) => final_assistant,
                Err(error) => {
                    let failure = Arc::new(error);
                    sink.emit(AgentEvent::ActualTurnEnd {
                        message: tail,
                        tool_results: Vec::new(),
                        preparation_error: Some(failure.clone()),
                    })
                    .await;
                    sink.emit(AgentEvent::ActualAgentEnd {
                        messages: new_messages.clone(),
                        failure,
                    })
                    .await;
                    return Ok(ctl.gate_external_end().unwrap_or(RunEnd::Error));
                }
            };
        let mut paused = 0;
        match process_actual_turn(
            context,
            new_messages,
            final_assistant,
            &execution,
            ctl,
            sink,
            soft_state,
            None,
            None,
            &mut paused,
        )
        .await
        {
            // Fixed OMP leaves a normal tool continuation open under user
            // abort when no optional final gate was installed. The next loop
            // emits the aborted assistant; it must not dispatch another call.
            Ok((Some(RunEnd::Aborted), true))
                if gate.is_none()
                    && matches!(
                        ctl.stop_cause(),
                        Some(RunStopCause::ExternalAbort | RunStopCause::ToolScopedAbort { .. })
                    ) => {}
            Ok((Some(reason), _)) => return Ok(end(sink, new_messages, reason).await),
            Ok((None, _)) => {}
            Err(failure) => {
                sink.emit(AgentEvent::ActualAgentEnd {
                    messages: new_messages.clone(),
                    failure,
                })
                .await;
                return Ok(ctl.gate_external_end().unwrap_or(RunEnd::Error));
            }
        }
    } else if let Some(tail) =
        unpaired_raw_tool_call_tail(context).map_err(|error| AsideError(error.to_string()))?
    {
        sink.emit(AgentEvent::TurnStart).await;
        emit_inputs(sink, &std::mem::take(&mut to_emit)).await;
        let execution = execution_config(config);
        let mut tail = tail;
        let before = serde_json::to_value(&tail).expect("Assistant serializes to JSON");
        let prepared =
            transform_and_prepare(&mut tail, context, &execution, execution.tracing(), ctl).await;
        let record = context.last().unwrap();
        let body = match record.body_snapshot() {
            AgentMessageBody::Model(current) => {
                let mut current =
                    serde_json::to_value(current).expect("Message serializes to JSON");
                let after = serde_json::to_value(&tail).expect("Assistant serializes to JSON");
                apply_assistant_snapshot_edits(&mut current, &before, &after);
                AgentMessageBody::Model(
                    serde_json::from_value(current)
                        .expect("checked typed Assistant edits keep a valid current body"),
                )
            }
            AgentMessageBody::RawJson(mut raw) => {
                let after = serde_json::to_value(&tail).expect("Assistant serializes to JSON");
                apply_assistant_snapshot_edits(&mut raw, &before, &after);
                AgentMessageBody::RawJson(raw)
            }
            AgentMessageBody::Graph(_) => {
                return Err(AsideError(
                    "Graph tail requires an actual Graph preparation consumer".into(),
                ));
            }
        };
        record
            .replace_body(body)
            .expect("never-executed tail keeps its owned boundary");
        let results = match prepared {
            Ok(prepared) => execute_calls(&tail, context, &execution, ctl, sink, prepared).await,
            Err(error) => {
                let mut results = Vec::new();
                for call in tail.tool_calls() {
                    results.push(emit_synthetic_result(sink, call, "error", Some(&error.0)).await);
                }
                results
            }
        };
        for r in &results {
            context.push(r.record.clone());
            new_messages.push(r.record.clone());
        }
        emit_turn_end(
            context,
            new_messages,
            tail,
            results,
            config,
            ctl,
            sink,
            !ctl.is_cancelled(),
        )
        .await;
        if ctl.terminal_result() {
            return Ok(end(sink, new_messages, RunEnd::TerminalToolResult).await);
        }
    }

    let mut model_calls = 0usize;
    let mut paused_turn_continuations = 0usize;
    loop {
        let mut has_more_tool_calls = true;
        while has_more_tool_calls || !pending.is_empty() {
            if deadline_passed(config)
                || config.max_model_calls.is_some_and(|max| model_calls >= max)
            {
                // Commit already-dequeued messages so queued user input is never lost.
                // Preserve ordinary dequeue ownership; passive claims remain
                // pending for the Native finally discard instead of early commit.
                pending.preserve_normal(context, new_messages, &mut to_emit)?;
                emit_inputs(sink, &to_emit).await;
                let reason = if deadline_passed(config) {
                    RunEnd::Deadline
                } else {
                    RunEnd::ModelCallBudget
                };
                return Ok(end(sink, new_messages, reason).await);
            }
            AGENT_PAUSE_GATE.wait_until_resumed(Some(&ctl.token)).await;
            // Abort releases this wait only. Native still inserts already claimed
            // inputs; existing admission then refuses every external dispatch.
            let mut turn_messages = std::mem::take(&mut to_emit);
            pending.insert(context, new_messages, &mut turn_messages)?;
            // Fixed OMP syncs context before the model call, then keeps that
            // call's adapters through preparation and the whole tool batch.
            let execution = match admit_model(config, context, &ctl.token).await {
                Ok(execution) => execution,
                Err(error) => {
                    return Ok(finish_admission_failure(
                        context,
                        new_messages,
                        config,
                        ctl,
                        sink,
                        error,
                        Some(turn_messages),
                    )
                    .await);
                }
            };
            // Native skips the source after external cancellation. Keep ordinary
            // preparation behavior unchanged, including the no-gate panic boundary.
            let selection = if ctl.gate_external_end().is_some() {
                Ok(ToolChoiceDirectiveSelection::default())
            } else {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execution.hooks.select_tool_choice_directive_full(
                        &execution.tools,
                        execution.tool_choice.as_ref(),
                    )
                }))
                .unwrap_or_else(|payload| {
                    Err(ToolDirectiveError(
                        FinalProviderGateError::panicked(
                            "Tool choice directive selection",
                            payload,
                        )
                        .0,
                    ))
                })
            };
            let selection = match selection {
                Ok(selection) => selection,
                Err(error) => {
                    return Ok(finish_final_provider_failure(
                        context,
                        new_messages,
                        config,
                        &execution,
                        ctl,
                        sink,
                        FinalProviderGateError(error.0),
                        turn_messages,
                    )
                    .await);
                }
            };
            let (hard_directive, soft_requirement) = match selection.directive {
                Some(ToolChoiceDirective::Hard(choice)) => (Some(choice), None),
                Some(ToolChoiceDirective::Soft(requirement)) => (None, Some(requirement)),
                None => (None, None),
            };
            for reminder in soft_state.activate(soft_requirement.as_ref()) {
                reminder
                    .adopt()
                    .map_err(|error| AsideError(error.to_string()))?;
                context.push(reminder.clone());
                new_messages.push(reminder.clone());
                turn_messages.push(reminder);
            }
            let selected_tool_choice = soft_state.effective_choice(&ToolChoiceSelection {
                directive: hard_directive,
                static_fallback: selection.static_fallback,
            });
            let provider_context = if let Some(gate) = &gate {
                let prepared = std::panic::AssertUnwindSafe(async {
                    let mut prepared = main_raw_request_context(
                        &execution.config,
                        context,
                        execution.provider_context.as_ref(),
                        ctl,
                    )
                    .await
                    .map_err(|error| FinalProviderGateError(error.to_string()))?;
                    let stop = match &mut prepared.context {
                        PreparedRequestContext::Typed(context) => {
                            Ok(gate.invoke(context, &ctl.token).await?)
                        }
                        PreparedRequestContext::Actual { .. } => Err(Arc::new(
                            ActualProviderError::Unsupported("actual final prepared-request gate"),
                        )),
                    };
                    Ok::<_, FinalProviderGateError>((prepared, stop))
                })
                .catch_unwind()
                .await
                .unwrap_or_else(|payload| {
                    Err(FinalProviderGateError::panicked(
                        "Final provider request preparation",
                        payload,
                    ))
                });
                let (prepared, stop) = match prepared {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        return Ok(finish_final_provider_failure(
                            context,
                            new_messages,
                            config,
                            &execution,
                            ctl,
                            sink,
                            error,
                            turn_messages,
                        )
                        .await);
                    }
                };
                let stop = match stop {
                    Ok(stop) => stop,
                    Err(error) => {
                        return Ok(finish_provider_preparation_failure(
                            context,
                            new_messages,
                            config,
                            &execution,
                            ctl,
                            sink,
                            ProviderContextError::Actual(error),
                            turn_messages,
                        )
                        .await);
                    }
                };
                if stop.is_some() || ctl.gate_external_end().is_some() {
                    return Ok(finish_final_provider_stop(
                        context,
                        new_messages,
                        config,
                        &execution,
                        ctl,
                        sink,
                        stop,
                        turn_messages,
                    )
                    .await);
                }
                prepared
            } else {
                match main_raw_request_context(
                    &execution.config,
                    context,
                    execution.provider_context.as_ref(),
                    ctl,
                )
                .await
                {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        return Ok(finish_provider_preparation_failure(
                            context,
                            new_messages,
                            config,
                            &execution,
                            ctl,
                            sink,
                            error,
                            turn_messages,
                        )
                        .await);
                    }
                }
            };
            sink.emit(AgentEvent::TurnStart).await;
            emit_inputs(sink, &turn_messages).await;
            let final_assistant = stream_assistant_response(
                context,
                &execution,
                ctl,
                sink,
                provider_context,
                selected_tool_choice,
                &mut model_calls,
            )
            .await;
            new_messages.push(context.last().expect("final Assistant retained").clone());

            soft_state.assistant_finalized();
            match final_assistant {
                FinalAssistant::Actual(final_assistant) => {
                    match process_actual_turn(
                        context,
                        new_messages,
                        final_assistant,
                        &execution,
                        ctl,
                        sink,
                        soft_state,
                        soft_requirement.as_ref(),
                        if selection.suppress_static_soft_block {
                            None
                        } else {
                            execution.tool_choice.as_ref()
                        },
                        &mut paused_turn_continuations,
                    )
                    .await
                    {
                        // As on the resumed-tool path, ordinary external
                        // abort must retain the no-gate assistant receipt.
                        Ok((Some(RunEnd::Aborted), true))
                            if gate.is_none()
                                && matches!(
                                    ctl.stop_cause(),
                                    Some(
                                        RunStopCause::ExternalAbort
                                            | RunStopCause::ToolScopedAbort { .. }
                                    )
                                ) =>
                        {
                            has_more_tool_calls = true;
                        }
                        Ok((Some(reason), _)) => return Ok(end(sink, new_messages, reason).await),
                        Ok((None, has_more)) => {
                            has_more_tool_calls = has_more;
                        }
                        Err(failure) => {
                            sink.emit(AgentEvent::ActualAgentEnd {
                                messages: new_messages.to_vec(),
                                failure,
                            })
                            .await;
                            return Ok(ctl.gate_external_end().unwrap_or(RunEnd::Error));
                        }
                    }
                }
                FinalAssistant::Typed { message, prepared } => {
                    let tool_calls: Vec<ToolCall> = message.tool_calls().cloned().collect();
                    if matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
                        let reason = if message.stop_reason == StopReason::Aborted {
                            "aborted"
                        } else {
                            "error"
                        };
                        let mut results = Vec::new();
                        for call in &tool_calls {
                            let r = emit_synthetic_result(
                                sink,
                                call,
                                reason,
                                message.error_message.as_deref(),
                            )
                            .await;
                            context.push(r.record.clone());
                            new_messages.push(r.record.clone());
                            results.push(r);
                        }
                        let reason = match message.stop_reason {
                            StopReason::Aborted if ctl.deadline_fired() => RunEnd::Deadline,
                            StopReason::Aborted => RunEnd::Aborted,
                            _ => RunEnd::Error,
                        };
                        emit_turn_end(
                            context,
                            new_messages,
                            message,
                            results,
                            config,
                            ctl,
                            sink,
                            false,
                        )
                        .await;
                        return Ok(end(sink, new_messages, reason).await);
                    }

                    let runnable =
                        matches!(message.stop_reason, StopReason::ToolUse | StopReason::Stop);
                    has_more_tool_calls = runnable && !tool_calls.is_empty();
                    let deadline_hit = deadline_passed(config);
                    if has_more_tool_calls && deadline_hit {
                        has_more_tool_calls = false;
                    }
                    let soft_rejection =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            soft_state.noncompliant(
                                soft_requirement.as_ref(),
                                &tool_calls,
                                if selection.suppress_static_soft_block {
                                    None
                                } else {
                                    execution.tool_choice.as_ref()
                                },
                            )
                        }))
                        .unwrap_or_else(|payload| {
                            Err(ToolDirectiveError(
                                FinalProviderGateError::panicked(
                                    "Soft tool requirement compliance",
                                    payload,
                                )
                                .0,
                            ))
                        });
                    let soft_rejection = match soft_rejection {
                        Ok(reason) => reason,
                        Err(error) => {
                            return Ok(finish_open_directive_failure(
                                context,
                                new_messages,
                                config,
                                &execution,
                                ctl,
                                sink,
                                FinalProviderGateError(error.0),
                            )
                            .await);
                        }
                    };
                    let mut results = Vec::new();
                    if let Some(reason) = soft_rejection {
                        for call in &tool_calls {
                            let result =
                                emit_synthetic_result(sink, call, "skipped", Some(&reason)).await;
                            context.push(result.record.clone());
                            new_messages.push(result.record.clone());
                            results.push(result);
                        }
                        soft_state.record_escalation(
                            &soft_requirement
                                .as_ref()
                                .expect("active soft rejection")
                                .tool_name,
                        );
                        has_more_tool_calls = true;
                    } else if has_more_tool_calls {
                        results = execute_calls(
                            &message,
                            context,
                            &execution,
                            ctl,
                            sink,
                            prepared.unwrap_or_default(),
                        )
                        .await;
                        for r in &results {
                            context.push(r.record.clone());
                            new_messages.push(r.record.clone());
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
                            context.push(r.record.clone());
                            new_messages.push(r.record.clone());
                            results.push(r);
                        }
                        if message.stop_reason == StopReason::Length
                            && !results.is_empty()
                            && !deadline_hit
                        {
                            has_more_tool_calls = true;
                        }
                    }
                    // Fixed Native cap: nonterminal commentary resumes up to eight
                    // times, and any tool-call turn resets the continuation count.
                    const MAX_PAUSED_TURN_CONTINUATIONS: usize = 8;
                    if !tool_calls.is_empty() {
                        paused_turn_continuations = 0;
                    } else if !has_more_tool_calls
                        && message.stop_reason == StopReason::Stop
                        && message
                            .stop_details
                            .as_ref()
                            .and_then(|details| details.get("type"))
                            .and_then(Value::as_str)
                            == Some("pause_turn")
                        && paused_turn_continuations < MAX_PAUSED_TURN_CONTINUATIONS
                    {
                        paused_turn_continuations += 1;
                        has_more_tool_calls = true;
                    }
                    emit_turn_end(
                        context,
                        new_messages,
                        message,
                        results,
                        config,
                        ctl,
                        sink,
                        has_more_tool_calls && !ctl.is_cancelled(),
                    )
                    .await;
                }
            }
            if ctl.terminal_result() {
                return Ok(end(sink, new_messages, RunEnd::TerminalToolResult).await);
            }
            if deadline_passed(config) {
                return Ok(end(sink, new_messages, RunEnd::Deadline).await);
            }
            pending.set_normal(if ctl.is_cancelled() {
                Vec::new()
            } else {
                match poll_queued_input(config, context, &ctl.token, QueueInputKind::Steering).await
                {
                    Ok(inputs) => inputs,
                    Err(error) => {
                        return Ok(finish_admission_failure(
                            context,
                            new_messages,
                            config,
                            ctl,
                            sink,
                            error,
                            Some(to_emit),
                        )
                        .await);
                    }
                }
            });
            if has_more_tool_calls && !ctl.is_cancelled() {
                pending.extend_asides(poll_aside_inputs(config).await?);
            }
        }
        if deadline_passed(config) || ctl.deadline_fired() {
            return Ok(end(sink, new_messages, RunEnd::Deadline).await);
        }
        before_yield(config).await?;
        if deadline_passed(config) || ctl.deadline_fired() {
            return Ok(end(sink, new_messages, RunEnd::Deadline).await);
        }
        if ctl.is_cancelled() {
            return Ok(end(sink, new_messages, ctl.cancelled_end()).await);
        }
        let late =
            match poll_queued_input(config, context, &ctl.token, QueueInputKind::Steering).await {
                Ok(inputs) => inputs,
                Err(error) => {
                    return Ok(finish_admission_failure(
                        context,
                        new_messages,
                        config,
                        ctl,
                        sink,
                        error,
                        Some(to_emit),
                    )
                    .await);
                }
            };
        pending.set_normal(late);
        if !ctl.is_cancelled() {
            pending.extend_asides(poll_aside_inputs(config).await?);
        }
        let follow_up = if ctl.is_cancelled() {
            Vec::new()
        } else {
            match poll_queued_input(config, context, &ctl.token, QueueInputKind::FollowUp).await {
                Ok(inputs) => inputs,
                Err(error) => {
                    // A successful steering dequeue already transferred ownership.
                    // Preserve those inputs even if follow-up admission is refused.
                    pending.preserve_normal(context, new_messages, &mut to_emit)?;
                    return Ok(finish_admission_failure(
                        context,
                        new_messages,
                        config,
                        ctl,
                        sink,
                        error,
                        Some(to_emit),
                    )
                    .await);
                }
            }
        };
        pending
            .0
            .extend(follow_up.into_iter().map(|message| (message, false)));
        if pending.is_empty() {
            break;
        }
    }
    Ok(end(sink, new_messages, RunEnd::Completed).await)
}

/// Keep only tool calls that reached `toolcall_end` on an error/aborted turn
/// (OMP `retainCompletedToolCalls`).
pub fn retain_completed_tool_calls(
    mut message: AssistantMessage,
    completed: &HashSet<String>,
) -> AssistantMessage {
    if !matches!(message.stop_reason, StopReason::Error | StopReason::Aborted) {
        return message;
    }
    let before = message.content.len();
    message
        .content
        .retain(|b| b.as_tool_call().is_none_or(|c| completed.contains(&c.id)));
    if message.content.len() != before {
        let already = message
            .stop_details
            .as_ref()
            .and_then(|d| d.get("type"))
            .and_then(Value::as_str)
            == Some(STREAM_INTERRUPTED_AFTER_CONTENT_STOP_DETAIL);
        if !already {
            let category = message
                .stop_details
                .as_ref()
                .and_then(|d| d.get("type"))
                .cloned()
                .unwrap_or(Value::Null);
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

// Short-lived final payloads move through the existing loop without a second
// canonical owner or boxing of the original typed Assistant.
#[allow(clippy::large_enum_variant)]
enum FinalAssistant {
    Typed {
        message: AssistantMessage,
        prepared: Option<Vec<Prepared>>,
    },
    Actual(ActualFinalAssistant),
}

async fn settle_actual_stream_receipt(
    receipt: Option<&ara_ai::ActualStreamFailureReceipt>,
    mut failure: Option<ActualProviderError>,
) -> Option<ActualProviderError> {
    if let Some(receipt) = receipt {
        // Caller already consumed terminal/EOF or dropped the receiver. Never
        // await a producer that is blocked on the channel we still hold open.
        for secondary in receipt.wait_settled().await {
            let secondary = ActualProviderError::Value(secondary.as_ref().clone());
            failure = Some(match failure {
                Some(primary) => ActualProviderError::Compound {
                    primary: Arc::new(primary),
                    secondary: Arc::new(secondary),
                },
                None => secondary,
            });
        }
    }
    failure
}

// Isolate future construction from the caller's poll frame on bounded stacks.
fn stream_assistant_response<'a>(
    context: &'a mut Vec<AgentMessage>,
    config: &'a AdmittedExecution,
    ctl: &'a RunControl,
    sink: &'a dyn AgentEventSink,
    prepared_call: PreparedProviderCall,
    selected_tool_choice: Option<ToolChoice>,
    model_calls: &'a mut usize,
) -> Pin<Box<impl Future<Output = FinalAssistant> + 'a>> {
    Box::pin(stream_assistant_response_owned(
        context,
        config,
        ctl,
        sink,
        prepared_call,
        selected_tool_choice,
        model_calls,
    ))
}

async fn stream_assistant_response_owned(
    context: &mut Vec<AgentMessage>,
    config: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    prepared_call: PreparedProviderCall,
    selected_tool_choice: Option<ToolChoice>,
    model_calls: &mut usize,
) -> FinalAssistant {
    let cancel = &ctl.token;
    let mut partial: Option<AssistantMessage> = None;
    let mut added_partial = false;
    let mut completed: HashSet<String> = HashSet::new();
    // ARA captured-call retention contract: keep the exact post-hook wire Arc
    // before producer consumption. Host execution authority remains separate.
    let mut actual_state = match &prepared_call.context {
        PreparedRequestContext::Actual { values, .. } => Some(ActualStreamState::new(
            values.clone(),
            prepared_call
                .owned
                .as_ref()
                .and_then(|owned| owned.actual_tools.clone()),
            prepared_call
                .owned
                .as_ref()
                .and_then(|owned| owned.actual_tool_source.clone()),
            prepared_call
                .owned
                .as_ref()
                .and_then(|owned| owned.original_post_hook_tools.clone()),
        )),
        PreparedRequestContext::Typed(_) => None,
    };
    if cancel.is_cancelled() {
        if let Some(actual) = actual_state.take() {
            return FinalAssistant::Actual(
                actual
                    .finish(context, config, ctl, sink, None, None, false, false)
                    .await,
            );
        }
        return finish_aborted(
            context,
            config,
            ctl,
            sink,
            partial,
            added_partial,
            &completed,
        )
        .await;
    }
    let mut arg_streams = ArgStreams::new();
    let provider_cancel = cancel.child_token();
    let PreparedProviderCall {
        context: llm_context,
        suppress_native_tool_choice,
        owned,
    } = prepared_call;
    let options = CallOptions {
        cancel: provider_cancel.clone(),
        max_in_flight_requests: None,
        wait_for_terminal_drain: false,
        tool_choice: if suppress_native_tool_choice {
            None
        } else {
            selected_tool_choice
        },
        suppress_native_tool_choice,
        max_tokens: config.max_tokens,
        temperature: config.temperature,
        loop_guard: None,
        on_response: None,
        reasoning: None,
        include: None,
        responses_lite: None,
    };
    *model_calls += 1;
    let mut rx = match llm_context {
        PreparedRequestContext::Typed(llm_context) => {
            PreparedAssistantStream::Typed(if let Some(owned) = owned {
                owned.port.stream_prepared(
                    config.provider.as_ref(),
                    &config.model,
                    &llm_context,
                    owned.tools,
                    options,
                )
            } else {
                config.provider.stream(&config.model, &llm_context, options)
            })
        }
        PreparedRequestContext::Actual {
            context: actual_context,
            values,
        } => {
            let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                actual_context.check_lineage(&values)?;
                let owned = owned.ok_or(ActualProviderError::Unsupported(
                    "actual call lost captured owned port",
                ))?;
                let selected_values = owned.values.ok_or(ActualProviderError::Unsupported(
                    "actual call lost captured Session values",
                ))?;
                if !Arc::ptr_eq(&selected_values, &values) {
                    return Err(ActualProviderError::Protocol(
                        "actual call and owned port have different Session bindings",
                    ));
                }
                let tools = owned.original_post_hook_tools;
                if let Some(tools) = &tools {
                    values.check_handle(tools)?;
                }
                owned.port.stream_prepared_original_actual(
                    config.provider.as_ref(),
                    &config.model,
                    &actual_context,
                    tools,
                    values.clone(),
                    options,
                )
            }));
            let prepared = match prepared {
                Ok(result) => result,
                Err(panic) => Err(ActualProviderError::Callback(format!(
                    "Actual stream producer panicked: {}",
                    panic_text(panic)
                ))),
            };
            match prepared {
                Ok(stream) => PreparedAssistantStream::Actual { stream },
                Err(error) => {
                    return FinalAssistant::Actual(
                        actual_state
                            .take()
                            .expect("actual prepared call retained its original state")
                            .finish(context, config, ctl, sink, None, Some(error), false, false)
                            .await,
                    );
                }
            }
        }
    };
    let actual_failure_receipt = match &rx {
        PreparedAssistantStream::Actual { stream } => Some(stream.failure_receipt()),
        PreparedAssistantStream::Typed(_) => None,
    };
    let preview_wake = actual_state
        .as_ref()
        .map(ActualStreamState::preview_wake)
        .unwrap_or_else(|| arg_streams.preview_wake());
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                provider_cancel.cancel();
                drop(rx);
                if let Some(actual) = actual_state.take() {
                    let failure = settle_actual_stream_receipt(actual_failure_receipt.as_ref(), None).await;
                    return FinalAssistant::Actual(actual.finish(context, config, ctl, sink, None, failure, false, false).await);
                }
                let final_assistant = finish_aborted(context, config, ctl, sink, partial, added_partial, &completed).await;
                arg_streams.finish(sink).await;
                return final_assistant;
            }
            // Keep cancellation first, with fair provider/preview selection.
            // An asynchronous native preview must wake this same attempt even
            // when the provider is paused waiting for the Host abort signal.
            ev = async {
                tokio::select! {
                    _ = preview_wake.notified() => Err(()),
                    ev = rx.recv() => Ok(ev),
                }
            } => match ev {
                Ok(event) => event,
                Err(()) => {
                    if let Some(actual) = actual_state.as_mut() {
                        actual.flush_previews(sink).await;
                    } else {
                        arg_streams.flush(sink).await;
                    }
                    continue;
                }
            },
        };
        let Some(event) = next else {
            if let Some(actual) = actual_state.take() {
                provider_cancel.cancel();
                let failure = settle_actual_stream_receipt(actual_failure_receipt.as_ref(), None)
                    .await
                    .unwrap_or(ActualProviderError::Protocol(MISSING_TERMINAL_EVENT));
                return FinalAssistant::Actual(
                    actual
                        .finish(
                            context,
                            config,
                            ctl,
                            sink,
                            None,
                            Some(failure),
                            false,
                            false,
                        )
                        .await,
                );
            }
            arg_streams.finish(sink).await;
            // Provider contract violation: synthesize an error turn instead of hanging.
            let mut msg = partial.clone().unwrap_or_else(|| {
                AssistantMessage::empty(&config.model.api, &config.model.provider, &config.model.id)
            });
            msg.stop_reason = StopReason::Error;
            msg.error_message = Some(MISSING_TERMINAL_EVENT.into());
            let message = commit_final(
                context,
                sink,
                retain_completed_tool_calls(msg, &completed),
                added_partial,
                Some(config),
            )
            .await;
            return FinalAssistant::Typed {
                message,
                prepared: None,
            };
        };
        let event = match event {
            PreparedStreamEvent::Typed(event) => event,
            PreparedStreamEvent::Actual(event) => {
                match event {
                    AssistantMessageEvent::Done { message, .. } => {
                        let failure =
                            settle_actual_stream_receipt(actual_failure_receipt.as_ref(), None)
                                .await;
                        return FinalAssistant::Actual(
                            actual_state
                                .take()
                                .expect("actual receiver has one actual state")
                                .finish(
                                    context,
                                    config,
                                    ctl,
                                    sink,
                                    Some(message),
                                    failure,
                                    true,
                                    false,
                                )
                                .await,
                        );
                    }
                    AssistantMessageEvent::Error { error, .. } => {
                        let failure =
                            settle_actual_stream_receipt(actual_failure_receipt.as_ref(), None)
                                .await;
                        return FinalAssistant::Actual(
                            actual_state
                                .take()
                                .expect("actual receiver has one actual state")
                                .finish(
                                    context,
                                    config,
                                    ctl,
                                    sink,
                                    Some(error),
                                    failure,
                                    true,
                                    true,
                                )
                                .await,
                        );
                    }
                    event => {
                        if let Err(error) = actual_state
                            .as_mut()
                            .expect("actual receiver has one actual state")
                            .update(context, sink, event)
                            .await
                        {
                            provider_cancel.cancel();
                            drop(rx);
                            let failure = settle_actual_stream_receipt(
                                actual_failure_receipt.as_ref(),
                                Some(error),
                            )
                            .await;
                            return FinalAssistant::Actual(
                                actual_state
                                    .take()
                                    .expect("actual receiver has one actual state")
                                    .finish(context, config, ctl, sink, None, failure, false, false)
                                    .await,
                            );
                        }
                    }
                }
                continue;
            }
        };
        if let Err(error) = arg_streams.on_event(&event, &config.tools, config.hooks.as_ref()) {
            provider_cancel.cancel();
            drop(rx);
            arg_streams.finish(sink).await;
            let mut message = event.partial().clone();
            message.stop_reason = StopReason::Error;
            message.error_message = Some(error.0);
            let message = commit_final(
                context,
                sink,
                retain_completed_tool_calls(message, &completed),
                added_partial,
                Some(config),
            )
            .await;
            return FinalAssistant::Typed {
                message,
                prepared: None,
            };
        }
        arg_streams.flush(sink).await;
        match event {
            AssistantMessageEvent::Done { message, .. }
            | AssistantMessageEvent::Error { error: message, .. } => {
                let mut final_message = retain_completed_tool_calls(message, &completed);
                let prepared = match transform_and_prepare(
                    &mut final_message,
                    context,
                    config,
                    config.tracing(),
                    ctl,
                )
                .await
                {
                    Ok(prepared) => Some(prepared),
                    Err(error) => {
                        final_message.stop_reason = StopReason::Error;
                        final_message.error_message = Some(error.0);
                        None
                    }
                };
                let message = commit_final(context, sink, final_message, added_partial, None).await;
                // Native finally cancels unfinished previews after MessageEnd,
                // before the outer loop dispatches any prepared tool slots.
                arg_streams.finish(sink).await;
                return FinalAssistant::Typed { message, prepared };
            }
            AssistantMessageEvent::Start { partial: p } => {
                if added_partial {
                    context
                        .last()
                        .unwrap()
                        .replace_body(AgentMessageBody::Model(Message::Assistant(p.clone())))
                        .expect("stream record has not been adopted");
                    completed.clear();
                    sink.emit(AgentEvent::MessageUpdate {
                        message: context.last().unwrap().clone(),
                        event: AssistantMessageEvent::Start { partial: p.clone() },
                    })
                    .await;
                } else {
                    context.push(AgentMessage::from_model(Message::Assistant(p.clone())));
                    added_partial = true;
                    sink.emit(AgentEvent::MessageStart {
                        message: context.last().unwrap().clone(),
                    })
                    .await;
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
                context
                    .last()
                    .unwrap()
                    .replace_body(AgentMessageBody::Model(Message::Assistant(p.clone())))
                    .expect("stream record has not been adopted");
                partial = Some(p.clone());
                sink.emit(AgentEvent::MessageUpdate {
                    message: context.last().unwrap().clone(),
                    event: other,
                })
                .await;
            }
        }
    }
}

async fn commit_final(
    context: &mut Vec<AgentMessage>,
    sink: &dyn AgentEventSink,
    mut message: AssistantMessage,
    added_partial: bool,
    execution: Option<&AdmittedExecution>,
) -> AssistantMessage {
    if let Some(execution) = execution {
        annotate_intents(&mut message, &execution.tools, execution.tracing());
    }
    let raw = if added_partial {
        context.last().expect("streamed Assistant retained").clone()
    } else {
        let raw = AgentMessage::from_model(Message::Assistant(message.clone()));
        context.push(raw.clone());
        sink.emit(AgentEvent::MessageStart {
            message: raw.clone(),
        })
        .await;
        raw
    };
    raw.install_final_model(Message::Assistant(message.clone()))
        .expect("finalized Assistant boundary cannot be erased");
    sink.emit(AgentEvent::MessageEnd { message: raw }).await;
    message
}

/// OMP `emitAbortedAssistantMessage`.
async fn finish_aborted(
    context: &mut Vec<AgentMessage>,
    config: &AdmittedExecution,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    partial: Option<AssistantMessage>,
    added_partial: bool,
    completed: &HashSet<String>,
) -> FinalAssistant {
    let mut base = partial.unwrap_or_else(|| {
        AssistantMessage::empty(&config.model.api, &config.model.provider, &config.model.id)
    });
    base.stop_reason = StopReason::Aborted;
    base.error_message = Some(ctl.abort_text());
    let retained = retain_completed_tool_calls(base, completed);
    let message = commit_final(context, sink, retained, added_partial, Some(config)).await;
    FinalAssistant::Typed {
        message,
        prepared: None,
    }
}

/// OMP `createSyntheticToolResultMessage`: a call that never executed locally.
pub fn synthetic_tool_result(
    call: &ToolCall,
    reason: &str,
    error_message: Option<&str>,
) -> ToolResultMessage {
    let message = match reason {
        "aborted" => "Tool execution was aborted",
        "length" => {
            "Tool call was not executed because the assistant hit its output token limit (stop_reason: length) before the arguments could complete; the recorded arguments are truncated and unsafe to run. Do NOT retry by re-emitting the same large payload — split the work into several smaller tool calls (e.g. for `write`/`edit`, write the first chunk then append the rest with subsequent `edit` insert ops, or break the file into multiple `write` targets)"
        }
        "skipped" => "Tool call was not executed because the assistant ended its turn",
        _ => {
            "Tool call was not executed because the provider stream ended with an error before the tool could run"
        }
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
        provider_metadata: None,
        useless: false,
        timestamp: now_ms(),
    }
}

#[derive(Clone)]
struct ToolReceipt {
    model: ToolResultMessage,
    record: AgentMessage,
}

async fn emit_synthetic_result(
    sink: &dyn AgentEventSink,
    call: &ToolCall,
    reason: &str,
    error_message: Option<&str>,
) -> ToolReceipt {
    let scoped = (reason == "aborted")
        .then(|| sink.abort_reason().and_then(|owner| owner.get()))
        .flatten();
    let error_message = scoped
        .as_ref()
        .map(|reason| reason.for_tool(&call.id.as_str().into()))
        .or(error_message);
    let msg = synthetic_tool_result(call, reason, error_message);
    let output = ToolOutput {
        content: msg.content.clone(),
        details: msg.details.clone(),
        is_error: true,
        provider_metadata: None,
        useless: false,
    };
    sink.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.arguments.clone(),
        intent: call.intent.clone(),
    })
    .await;
    sink.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        result: output,
        is_error: true,
    })
    .await;
    let record = AgentMessage::from_owned_model(Message::ToolResult(msg.clone()), None);
    sink.emit(AgentEvent::MessageStart {
        message: record.clone(),
    })
    .await;
    sink.emit(AgentEvent::MessageEnd {
        message: record.clone(),
    })
    .await;
    ToolReceipt { model: msg, record }
}

struct CoercedToolOutput {
    output: ToolOutput,
    malformed: bool,
}

fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

/// Native coerceToolResult at a real untyped Host/extension boundary. The same
/// implementation is used for typed execute output and returned After overrides.
/// A None After return intentionally does not call it a second time.
pub fn coerce_raw_tool_result(raw: &Value) -> ToolOutput {
    coerce_result(raw).output
}

fn coerce_result(raw: &Value) -> CoercedToolOutput {
    let details = Some(raw.get("details").cloned().unwrap_or_else(|| json!({})));
    let metadata = snapshot_tool_result_provider_metadata(raw.get("providerMetadata"));
    let malformed_metadata = metadata.is_err();
    let Some(raw_content) = raw.get("content").and_then(Value::as_array) else {
        return CoercedToolOutput {
            output: ToolOutput {
                content: vec![UserBlock::text(
                    "Tool returned an invalid result: missing content array.",
                )],
                details,
                is_error: true,
                provider_metadata: None,
                useless: false,
            },
            malformed: true,
        };
    };
    let mut content = Vec::new();
    let mut invalid_blocks = 0;
    for block in raw_content {
        match block.get("type").and_then(Value::as_str) {
            Some("text") if block.get("text").and_then(Value::as_str).is_some() => {
                content.push(UserBlock::text(block["text"].as_str().unwrap()));
            }
            Some("image")
                if block.get("data").and_then(Value::as_str).is_some()
                    && block.get("mimeType").and_then(Value::as_str).is_some() =>
            {
                match serde_json::from_value::<UserBlock>(block.clone()) {
                    Ok(block) => content.push(block),
                    Err(_) => invalid_blocks += 1,
                }
            }
            _ => invalid_blocks += 1,
        }
    }
    if invalid_blocks > 0 {
        content.push(UserBlock::text(format!(
            "Tool returned an invalid result: {invalid_blocks} content block{} had an unsupported shape.",
            if invalid_blocks == 1 { "" } else { "s" }
        )));
    }
    if malformed_metadata {
        content.push(UserBlock::text(
            "Tool returned an invalid result: computer providerMetadata had an unsupported shape.",
        ));
    }
    let malformed = invalid_blocks > 0 || malformed_metadata;
    let mut output = ToolOutput {
        content,
        details,
        is_error: js_truthy(raw.get("isError")) || malformed,
        provider_metadata: metadata.ok().flatten(),
        useless: false,
    };
    output.useless = js_truthy(raw.get("useless")) && !output.is_error;
    let substantive = output.content.iter().any(|b| match b {
        UserBlock::Image(_) => true,
        UserBlock::Text(t) => !t.text.trim().is_empty(),
    });
    if output.is_error && !substantive {
        output.content = vec![UserBlock::text(EMPTY_ERROR_TOOL_RESULT_TEXT)];
    }
    CoercedToolOutput { output, malformed }
}

fn coerce(output: ToolOutput) -> CoercedToolOutput {
    coerce_result(&output.json())
}

#[derive(Clone)]
struct Prepared {
    call: ToolCall,
    tool: Option<Arc<dyn AgentTool>>,
    args: JsonObject,
    concurrency: Concurrency,
    interruptible: bool,
    validation_error: Option<String>,
    prepare_error: Option<String>,
    blocked: Option<String>,
}

async fn prepare(
    mut call: ToolCall,
    assistant: &AssistantMessage,
    messages: &[AgentMessage],
    tools: &[(Arc<dyn AgentTool>, ara_ai::Tool)],
    config: &AgentConfig,
    tracing: bool,
    ctl: &RunControl,
) -> Prepared {
    // Preview start and execution preparation share the exact Native lookup.
    let resolved = resolve_tool_for_call(tools, &call.name, config.hooks.as_ref());
    let tool = resolved
        .as_ref()
        .ok()
        .and_then(|found| found.as_ref().map(|(tool, _)| tool.clone()));
    let def = resolved
        .as_ref()
        .ok()
        .and_then(|found| found.as_ref().map(|(_, definition)| definition.clone()));
    let ownership = if tracing {
        def.as_ref().map(tool_owns_intent).transpose()
    } else {
        Ok(None)
    };
    let owns_intent = ownership.as_ref().ok().copied().flatten().unwrap_or(false);
    if ownership.is_ok() {
        annotate_call_intent(&mut call, tool.as_deref(), tracing, owns_intent);
    }
    call.provider_metadata = call
        .provider_metadata
        .as_ref()
        .and_then(|metadata| snapshot_tool_call_provider_metadata(Some(&json!(metadata))));
    let execution_arguments = stripped_arguments(&call, tracing && !owns_intent);
    let mut p = Prepared {
        call: call.clone(),
        tool,
        args: execution_arguments.clone(),
        concurrency: Concurrency::Shared,
        interruptible: false,
        validation_error: None,
        prepare_error: resolved
            .err()
            .map(|error| error.0)
            .or_else(|| ownership.err().map(|error| error.to_string())),
        blocked: None,
    };
    if p.prepare_error.is_some() {
        return p;
    }
    let Some(def) = def else {
        p.validation_error = Some(format!("Tool {} not found", call.name));
        return p;
    };
    let tool = p.tool.as_ref().unwrap().clone();
    let mut canonical_args_alias = false;
    match validate_tool_arguments_with_origin(&def, &call.name, &execution_arguments) {
        Ok(validated) => {
            // Native extractIntent always makes a rest-copy, even without i.
            canonical_args_alias = (!tracing || owns_intent)
                && validated.origin == ToolArgumentsOrigin::InputReference;
            p.args = validated.args;
        }
        Err(msg) => {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tool.lenient_argument_validation()
            }))
            .unwrap_or(false)
            {
                p.args.shift_remove("__parseError");
                p.args.shift_remove("__rawJson");
            } else {
                // Keep only the parse error, never the (possibly huge) raw payload (OMP parity).
                if let Some(parse_error) = execution_arguments.get("__parseError") {
                    let mut trimmed = JsonObject::new();
                    trimmed.insert("__parseError".into(), parse_error.clone());
                    p.args = trimmed;
                }
                p.validation_error = Some(msg);
                return p;
            }
        }
    }
    if ctl.is_cancelled() {
        return p; // run_tool reports the abort; do not prompt for approval.
    }
    let mut context = BeforeToolCallContext {
        assistant_message: assistant,
        tool_call: &call,
        tool: &tool,
        args: p.args,
        context: AgentContextView {
            system_prompt: &config.system_prompt,
            messages,
            tools: &config.tools,
        },
    };
    let before =
        std::panic::AssertUnwindSafe(config.hooks.before_tool_dispatch(&mut context, &ctl.token))
            .catch_unwind()
            .await;
    p.args = context.args;
    if canonical_args_alias {
        // Native mutates the original referenced object before the hook result
        // is inspected, including block/throw/cancel. Copy-mode never writes
        // through; explicit returned args still revalidate and take priority.
        p.call.arguments = p.args.clone();
    }
    match before {
        Ok(Ok(before)) => {
            if before.block {
                p.blocked = Some(
                    before
                        .reason
                        .unwrap_or_else(|| "Tool execution was blocked".into()),
                );
                return p;
            }
            if let Some(replacement) = before.args {
                match validate_tool_arguments(&def, &call.name, &replacement) {
                    Ok(args) => {
                        p.args = args;
                        p.call.arguments = replacement;
                    }
                    Err(error) => {
                        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            tool.lenient_argument_validation()
                        }))
                        .unwrap_or(false)
                        {
                            p.args = replacement.clone();
                            p.args.shift_remove("__parseError");
                            p.args.shift_remove("__rawJson");
                            p.call.arguments = replacement;
                        } else {
                            p.args = replacement;
                            if let Some(parse_error) = p.args.get("__parseError").cloned() {
                                p.args = JsonObject::new();
                                p.args.insert("__parseError".into(), parse_error);
                            }
                            p.validation_error = Some(error);
                            return p;
                        }
                    }
                }
            }
        }
        Ok(Err(error)) => {
            p.prepare_error = Some(error.0);
            return p;
        }
        Err(panic) => {
            p.prepare_error = Some(format!("Before tool hook panicked: {}", panic_text(panic)));
            return p;
        }
    }
    if ctl.is_cancelled() {
        return p;
    }
    match std::panic::AssertUnwindSafe(config.hooks.before_tool_call(&p.call, &p.args, &ctl.token))
        .catch_unwind()
        .await
    {
        Ok(ToolDecision::Block(reason)) => {
            p.blocked = Some(reason.unwrap_or_else(|| "Tool execution was blocked".into()))
        }
        Ok(ToolDecision::Allow) => {}
        Err(panic) => {
            p.prepare_error = Some(format!(
                "Tool permission hook panicked: {}",
                panic_text(panic)
            ))
        }
    }
    p
}

async fn prepare_calls(
    message: &mut AssistantMessage,
    messages: &[AgentMessage],
    config: &AgentConfig,
    tracing: bool,
    ctl: &RunControl,
) -> Vec<Prepared> {
    let tools: Vec<_> = config
        .tools
        .iter()
        .map(|tool| (tool.clone(), tool.protocol_definition()))
        .collect();
    let mut prepared = Vec::new();
    for index in 0..message.content.len() {
        let Some(call) = message.content[index].as_tool_call().cloned() else {
            continue;
        };
        let p = prepare(call, message, messages, &tools, config, tracing, ctl).await;
        message.content[index] = ara_ai::AssistantBlock::ToolCall(p.call.clone());
        prepared.push(p);
    }
    for p in &mut prepared {
        if let Some(tool) = &p.tool {
            p.concurrency = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tool.concurrency(&p.args)
            }))
            .unwrap_or(Concurrency::Exclusive);
            p.interruptible = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tool.interruptible(&p.args)
            }))
            .unwrap_or(false);
        }
    }
    prepared
}

/// Apply only edits made by the typed Assistant hooks to the current raw body.
/// Unknown record/block fields stay lossless; stable tool-call IDs retain their
/// raw block when hooks insert/remove/reorder neighboring content. This is not
/// a transcript replacement or a source-based reconstruction.
fn apply_assistant_snapshot_edits(raw: &mut Value, before: &Value, after: &Value) {
    if before == after {
        return;
    }
    match (raw, before, after) {
        (Value::Object(raw), Value::Object(before), Value::Object(after)) => {
            for key in before.keys().filter(|key| !after.contains_key(*key)) {
                raw.remove(key);
            }
            for (key, value) in after {
                match (raw.get_mut(key), before.get(key)) {
                    (Some(current), Some(previous)) => {
                        apply_assistant_snapshot_edits(current, previous, value)
                    }
                    _ => {
                        raw.insert(key.clone(), value.clone());
                    }
                }
            }
        }
        (Value::Array(raw), Value::Array(before), Value::Array(after)) => {
            if before
                .iter()
                .chain(after)
                .all(|block| block.get("type").and_then(Value::as_str).is_some())
            {
                let current_for_before = align_assistant_blocks(raw, before);
                let before_for_after = align_assistant_blocks(before, after);
                let mut foreign_before: Vec<Vec<Value>> = vec![Vec::new(); before.len()];
                let mut trailing = Vec::new();
                for (current, block) in raw.iter().enumerate() {
                    if current_for_before.contains(&Some(current)) {
                        continue;
                    }
                    if let Some(next) = current_for_before
                        .iter()
                        .enumerate()
                        .filter_map(|(old, position)| {
                            position
                                .filter(|position| *position > current)
                                .map(|position| (old, position))
                        })
                        .min_by_key(|(_, position)| *position)
                        .map(|(old, _)| old)
                    {
                        foreign_before[next].push(block.clone());
                    } else {
                        trailing.push(block.clone());
                    }
                }
                let mut revised = Vec::with_capacity(raw.len() + after.len());
                for (index, value) in after.iter().enumerate() {
                    if let Some(original) = before_for_after[index] {
                        revised.append(&mut foreign_before[original]);
                        let mut current = current_for_before[original]
                            .and_then(|position| raw.get(position))
                            .cloned()
                            .unwrap_or_else(|| before[original].clone());
                        apply_assistant_snapshot_edits(&mut current, &before[original], value);
                        revised.push(current);
                    } else {
                        revised.push(value.clone());
                    }
                }
                for mut foreign in foreign_before {
                    revised.append(&mut foreign);
                }
                revised.append(&mut trailing);
                *raw = revised;
            } else {
                // Free-form argument arrays are owned typed argument values.
                *raw = after.clone();
            }
        }
        (raw, _, after) => *raw = after.clone(),
    }
}

fn assistant_known_fields_match(raw: &Value, typed: &Value) -> bool {
    match (raw, typed) {
        (Value::Object(raw), Value::Object(typed)) => typed.iter().all(|(key, value)| {
            raw.get(key)
                .is_some_and(|raw| assistant_known_fields_match(raw, value))
        }),
        (Value::Array(raw), Value::Array(typed)) => {
            raw.len() == typed.len()
                && raw
                    .iter()
                    .zip(typed)
                    .all(|(raw, typed)| assistant_known_fields_match(raw, typed))
        }
        _ => raw == typed,
    }
}

/// First reserve exact retained blocks and stable call identities for the whole
/// array; only then pair edited ordinary blocks by their type/order. Current
/// Host insertions are never mistaken for the old call's positional index.
fn align_assistant_blocks(source: &[Value], target: &[Value]) -> Vec<Option<usize>> {
    let mut mapping = vec![None; target.len()];
    let mut used = HashSet::new();
    for (index, block) in target.iter().enumerate() {
        let found = source.iter().enumerate().find(|(i, raw)| {
            !used.contains(i)
                && if block["type"].as_str() == Some("toolCall") {
                    raw["type"].as_str() == Some("toolCall") && raw.get("id") == block.get("id")
                } else {
                    assistant_known_fields_match(raw, block)
                }
        });
        if let Some((position, _)) = found {
            mapping[index] = Some(position);
            used.insert(position);
        }
    }
    for (index, block) in target.iter().enumerate() {
        if mapping[index].is_some() || block["type"].as_str() == Some("toolCall") {
            continue;
        }
        if let Some((position, _)) = source.iter().enumerate().find(|(i, raw)| {
            !used.contains(i) && block.get("type").is_some() && raw.get("type") == block.get("type")
        }) {
            mapping[index] = Some(position);
            used.insert(position);
        }
    }
    mapping
}

async fn transform_and_prepare(
    message: &mut AssistantMessage,
    messages: &[AgentMessage],
    config: &AgentConfig,
    tracing: bool,
    ctl: &RunControl,
) -> Result<Vec<Prepared>, ToolHookError> {
    match std::panic::AssertUnwindSafe(
        config
            .hooks
            .transform_assistant_message(message, &ctl.token),
    )
    .catch_unwind()
    .await
    {
        Ok(result) => result?,
        Err(panic) => {
            return Err(ToolHookError(format!(
                "Assistant transform panicked: {}",
                panic_text(panic)
            )));
        }
    }
    Ok(prepare_calls(message, messages, config, tracing, ctl).await)
}

async fn emit_result(
    sink: &dyn AgentEventSink,
    p: &Prepared,
    output: ToolOutput,
    is_error: bool,
    started: bool,
) -> ToolReceipt {
    if !started {
        sink.emit(AgentEvent::ToolExecutionStart {
            tool_call_id: p.call.id.clone(),
            tool_name: p.call.name.clone(),
            args: p.args.clone(),
            intent: p.call.intent.clone(),
        })
        .await;
    }
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
        provider_metadata: output.provider_metadata,
        useless: output.useless && !is_error,
        is_error,
        timestamp: now_ms(),
    };
    let record = AgentMessage::from_owned_model(Message::ToolResult(msg.clone()), None);
    sink.emit(AgentEvent::MessageStart {
        message: record.clone(),
    })
    .await;
    sink.emit(AgentEvent::MessageEnd {
        message: record.clone(),
    })
    .await;
    ToolReceipt { model: msg, record }
}

fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}

#[derive(Clone, Copy)]
enum InterruptSource {
    Steering(SteeringSource),
    Irc,
}

struct BatchInterrupt {
    mode: InterruptMode,
    external: CancellationToken,
    hard: CancellationToken,
    soft: CancellationToken,
    source: Mutex<Option<InterruptSource>>,
    steering: Option<SteeringInterruptBinding>,
    irc: Option<Arc<dyn IrcInterruptPeek>>,
    watcher_cancel: CancellationToken,
}

impl BatchInterrupt {
    fn new(config: &AgentConfig, ctl: &RunControl) -> Self {
        Self {
            mode: config.interrupt_mode,
            external: ctl.token.clone(),
            hard: ctl.token.child_token(),
            soft: CancellationToken::new(),
            source: Mutex::new(None),
            steering: config.hooks.steering_interrupt_binding(),
            irc: config.hooks.irc_interrupt_port(),
            watcher_cancel: ctl.token.child_token(),
        }
    }
    fn source(&self) -> Option<InterruptSource> {
        *self.source.lock().unwrap()
    }
    fn trigger(&self, source: InterruptSource) {
        if self.mode != InterruptMode::Immediate || self.external.is_cancelled() {
            return;
        }
        let mut current = self.source.lock().unwrap();
        // Native has separate IRC/steering abort latches. The first steering
        // detection can replace an earlier IRC attribution, but later steering
        // detections never replace the first steering source.
        let set_source = match source {
            InterruptSource::Irc => current.is_none(),
            InterruptSource::Steering(_) => !matches!(*current, Some(InterruptSource::Steering(_))),
        };
        if set_source {
            *current = Some(source);
            self.hard.cancel();
            self.soft.cancel();
        }
    }
    async fn check_irc(&self) -> Result<(), ToolHookError> {
        if self.mode != InterruptMode::Immediate
            || self.external.is_cancelled()
            || self.source().is_some()
        {
            return Ok(());
        }
        if let Some(irc) = &self.irc {
            let queued = tokio::select! {
                biased;
                _ = self.watcher_cancel.cancelled() => return Ok(()),
                result = std::panic::AssertUnwindSafe(irc.queued()).catch_unwind() => match result {
                    Ok(result) => result?, Err(panic) => return Err(ToolHookError(panic_text(panic))),
                },
            };
            if queued {
                self.trigger(InterruptSource::Irc);
            }
        }
        Ok(())
    }
    async fn check(&self) -> Result<(), ToolHookError> {
        if self.mode != InterruptMode::Immediate || self.external.is_cancelled() {
            return Ok(());
        }
        if let Some(binding) = &self.steering {
            let state = tokio::select! {
                biased;
                _ = self.watcher_cancel.cancelled() => return Ok(()),
                result = std::panic::AssertUnwindSafe(binding.peek_port().peek()).catch_unwind() => match result {
                    Ok(result) => result?, Err(panic) => return Err(ToolHookError(panic_text(panic))),
                },
            };
            if state.queued {
                self.trigger(InterruptSource::Steering(
                    state.source.unwrap_or(SteeringSource::Unknown),
                ));
                return Ok(());
            }
        }
        self.check_irc().await
    }
    async fn watch_events(&self) {
        if self.mode != InterruptMode::Immediate {
            return;
        }
        let Some(wake) = self
            .steering
            .as_ref()
            .and_then(SteeringInterruptBinding::wake_port)
        else {
            return;
        };
        while !self.watcher_cancel.is_cancelled() {
            // Synchronous registration before peek is part of the wake contract.
            let subscription = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                wake.subscribe(self.watcher_cancel.clone())
            })) {
                Ok(subscription) => subscription,
                Err(_) => return,
            };
            if self.check().await.is_err()
                || self.watcher_cancel.is_cancelled()
                || self.source().is_some()
            {
                return;
            }
            let result = tokio::select! {
                biased;
                _ = self.watcher_cancel.cancelled() => return,
                result = std::panic::AssertUnwindSafe(subscription.wait()).catch_unwind() => result,
            };
            if !matches!(result, Ok(Ok(()))) {
                return;
            }
        }
    }
    async fn watch_timer(&self) {
        let poll_steering = self
            .steering
            .as_ref()
            .is_some_and(SteeringInterruptBinding::requires_polling);
        if self.mode != InterruptMode::Immediate || (!poll_steering && self.irc.is_none()) {
            return;
        }
        let mut timer = tokio::time::interval(std::time::Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! { biased; _ = self.watcher_cancel.cancelled() => return, _ = timer.tick() => {} }
            let result = if poll_steering {
                self.check().await
            } else {
                self.check_irc().await
            };
            if result.is_err() {
                return;
            }
        }
    }
}

fn skipped_tool_output(source: Option<InterruptSource>, execute_entered: bool) -> ToolOutput {
    let (reason, blocker) = match source {
        Some(InterruptSource::Steering(SteeringSource::User)) => {
            ("queued user message", "queued message")
        }
        Some(InterruptSource::Steering(SteeringSource::Agent)) => {
            ("pending parent steering message", "steering message")
        }
        Some(InterruptSource::Steering(SteeringSource::System)) => {
            ("pending system advisory", "advisory")
        }
        Some(InterruptSource::Irc) => ("pending peer interrupt", "interrupt"),
        _ => ("pending steering message", "queued message"),
    };
    ToolOutput::text(format!("Skipped due to {reason}. Do not count this skipped result as completed work or verification. After the {blocker} is handled on the next step, retry the skipped tool if it is still needed."))
        .with_details(if execute_entered { json!({"__interrupted": true, "source": "interrupt_skipped", "execution": "started"}) }
            else { json!({"__synthetic": true, "source": "interrupt_skipped", "executed": false}) })
}

fn native_caught_result(text: String) -> ToolOutput {
    ToolOutput::text(text).with_details(json!({}))
}

// Keep the tool execution inputs explicit without changing batch and cancellation ownership.
#[allow(clippy::too_many_arguments)]
async fn run_tool(
    p: &Prepared,
    index: usize,
    batch_id: &str,
    tool_calls: &Arc<[ToolCallInfo]>,
    message: &AssistantMessage,
    messages: &[AgentMessage],
    config: &AgentConfig,
    batch: &BatchInterrupt,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
) -> Option<ToolReceipt> {
    // Pending waits produce no result here. One tail sweep owns their receipts.
    if p.interruptible && batch.source().is_some() {
        return None;
    }
    let signal = if p.interruptible {
        batch.hard.child_token()
    } else {
        ctl.token.child_token()
    };
    AGENT_PAUSE_GATE.wait_until_resumed(Some(&signal)).await;
    let mut p = p.clone();
    if let Some(err) = &p.validation_error {
        let out =
            ToolOutput::text(err.clone()).with_details(json!({"isError": true, "error": err}));
        return Some(emit_result(sink, &p, out, true, false).await);
    }
    if signal.is_cancelled() {
        let out = native_caught_result(format!(
            "Tool was not executed because the run was aborted: {}.",
            ctl.tool_abort_text(&p.call.id.as_str().into())
        ));
        return Some(emit_result(sink, &p, out, true, false).await);
    }
    sink.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: p.call.id.clone(),
        tool_name: p.call.name.clone(),
        args: p.args.clone(),
        intent: p.call.intent.clone(),
    })
    .await;
    let mut execute_entered = false;
    let mut execute_returned = false;
    let mut effective_is_error;
    let mut output = if signal.is_cancelled() {
        effective_is_error = true;
        native_caught_result(format!(
            "Tool execution was aborted: {}",
            ctl.tool_abort_text(&p.call.id.as_str().into())
        ))
    } else if let Some(error) = p.prepare_error.clone().or_else(|| p.blocked.clone()) {
        effective_is_error = true;
        native_caught_result(error)
    } else {
        let transform = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            config
                .hooks
                .transform_tool_call_arguments(p.args.clone(), &p.call.name)
        }));
        match transform {
            Ok(Ok(args)) => {
                p.args = args;
                let tool = p.tool.clone().expect("validated tool exists");
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ToolOutput>();
                let update: crate::tool::UpdateFn = Arc::new(move |partial| {
                    let _ = tx.send(partial);
                });
                let tool_context = ToolExecutionContext {
                    batch_id: batch_id.to_string(),
                    index,
                    total: tool_calls.len(),
                    tool_calls: tool_calls.clone(),
                    provider_metadata: p.call.provider_metadata.clone(),
                    steering_signal: batch.soft.clone(),
                };
                execute_entered = true;
                let fut = std::panic::AssertUnwindSafe(async {
                    tool.execute_with_context(
                        &p.call.id,
                        p.args.clone(),
                        signal.clone(),
                        update,
                        tool_context,
                    )
                    .await
                })
                .catch_unwind();
                tokio::pin!(fut);
                // Cooperative cancellation never drops the real execute future.
                let result = loop {
                    tokio::select! {
                        result = &mut fut => break result,
                        Some(partial) = rx.recv() => sink.emit(AgentEvent::ToolExecutionUpdate { tool_call_id: p.call.id.clone(),
                            tool_name: p.call.name.clone(), partial: coerce(partial).output }).await,
                    }
                };
                while let Ok(partial) = rx.try_recv() {
                    sink.emit(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: p.call.id.clone(),
                        tool_name: p.call.name.clone(),
                        partial: coerce(partial).output,
                    })
                    .await;
                }
                match result {
                    Ok(Ok(raw)) => {
                        execute_returned = true; // Set BEFORE coercion, even for malformed/error output.
                        let coerced = coerce(raw);
                        effective_is_error = coerced.malformed || coerced.output.is_error;
                        coerced.output
                    }
                    Ok(Err(error)) => {
                        effective_is_error = true;
                        native_caught_result(error.0)
                    }
                    Err(panic) => {
                        effective_is_error = true;
                        native_caught_result(format!(
                            "Tool {} panicked: {}",
                            p.call.name,
                            panic_text(panic)
                        ))
                    }
                }
            }
            Ok(Err(error)) => {
                effective_is_error = true;
                native_caught_result(error.0)
            }
            Err(panic) => {
                effective_is_error = true;
                native_caught_result(format!(
                    "Tool argument transform panicked: {}",
                    panic_text(panic)
                ))
            }
        }
    };
    if !signal.is_cancelled() || execute_returned {
        let after = std::panic::AssertUnwindSafe(config.hooks.after_tool_call(
            AfterToolCallContext {
                assistant_message: message,
                tool_call: &p.call,
                args: &p.args,
                result: &mut output,
                is_error: effective_is_error,
                context: AgentContextView {
                    system_prompt: &config.system_prompt,
                    messages,
                    tools: &config.tools,
                },
                terminal: ctl.terminal_handle(&p.call.id),
            },
            &signal,
        ))
        .catch_unwind()
        .await;
        match after {
            Ok(Ok(None)) => {} // Preserve in-place mutations; no extra coerce or flag recomputation.
            Ok(Ok(Some(after))) => {
                let mut raw = output.json();
                if let Some(content) = after.content {
                    raw["content"] = json!(content);
                }
                if let Some(details) = after.details.filter(|value| !value.is_null()) {
                    raw["details"] = details;
                }
                if let Some(metadata) = after.provider_metadata.filter(|value| !value.is_null()) {
                    raw["providerMetadata"] = metadata;
                }
                if let Some(is_error) = after.is_error {
                    raw["isError"] = json!(is_error);
                }
                if let Some(useless) = after.useless {
                    raw["useless"] = json!(useless);
                }
                let coerced = coerce_result(&raw);
                effective_is_error =
                    coerced.malformed || after.is_error.unwrap_or(effective_is_error);
                output = coerced.output;
            }
            Ok(Err(error)) => {
                output = native_caught_result(error.0);
                effective_is_error = true;
            }
            Err(panic) => {
                output = native_caught_result(format!(
                    "After tool hook panicked: {}",
                    panic_text(panic)
                ));
                effective_is_error = true;
            }
        }
    }
    if batch.source().is_some() && signal.is_cancelled() && effective_is_error && !execute_returned
    {
        output = skipped_tool_output(batch.source(), execute_entered);
    }
    let result = emit_result(sink, &p, output, effective_is_error, true).await;
    Some(result)
}

async fn execute_calls(
    message: &AssistantMessage,
    messages: &[AgentMessage],
    config: &AgentConfig,
    ctl: &RunControl,
    sink: &dyn AgentEventSink,
    prepared: Vec<Prepared>,
) -> Vec<ToolReceipt> {
    static NEXT_BATCH_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let batch_id = format!(
        "tool-batch-{}-{}",
        now_ms(),
        NEXT_BATCH_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let tool_calls: Arc<[ToolCallInfo]> = prepared
        .iter()
        .map(|p| ToolCallInfo {
            id: p.call.id.clone(),
            name: p.call.name.clone(),
        })
        .collect::<Vec<_>>()
        .into();
    let batch = BatchInterrupt::new(config, ctl);
    let _watch_guard = batch.watcher_cancel.clone().drop_guard();
    let results = tokio::sync::Mutex::new(Vec::new());
    let skipped = tokio::sync::Mutex::new(Vec::new());
    let execution = async {
        let mut i = 0;
        while i < prepared.len() {
            let batch_end = if prepared[i].concurrency == Concurrency::Exclusive {
                i + 1
            } else {
                let mut end = i;
                while end < prepared.len() && prepared[end].concurrency != Concurrency::Exclusive {
                    end += 1;
                }
                end
            };
            let futs = prepared[i..batch_end]
                .iter()
                .enumerate()
                .map(|(offset, p)| {
                    let index = i + offset;
                    let (batch, batch_id, tool_calls, results, skipped) =
                        (&batch, &batch_id, &tool_calls, &results, &skipped);
                    async move {
                        match run_tool(
                            p, index, batch_id, tool_calls, message, messages, config, batch, ctl,
                            sink,
                        )
                        .await
                        {
                            Some(result) => {
                                // Native emits and appends the result before awaiting
                                // the post-tool peek. Receipt order must stay transcript
                                // order even when a later callback delays this task.
                                results.lock().await.push(result);
                                // A hostile peek can hold this dispatch boundary;
                                // watcher teardown does not bound arbitrary callbacks.
                                let _ = batch.check().await;
                            }
                            None => skipped.lock().await.push(index),
                        }
                    }
                });
            futures::future::join_all(futs).await;
            i = batch_end;
        }
        batch.watcher_cancel.cancel();
    };
    // Event watch subscribes before its positive peek. Each watcher races both
    // peek and wait against the local batch token and is joined during teardown.
    tokio::join!(batch.watch_events(), batch.watch_timer(), execution);
    let mut results = results.into_inner();
    let mut skipped = skipped.into_inner();
    skipped.sort_unstable();
    for index in skipped {
        results.push(
            emit_result(
                sink,
                &prepared[index],
                skipped_tool_output(batch.source(), false),
                true,
                false,
            )
            .await,
        );
    }
    results
}

/// Execute a known-never-executed assistant tail outside a model Run. The
/// detached message is transformed/prepared once and retained through dispatch.
pub async fn execute_tool_calls(
    message: &AssistantMessage,
    config: &AgentConfig,
    cancel: &CancellationToken,
    sink: &dyn AgentEventSink,
) -> Vec<ToolResultMessage> {
    let ctl = RunControl::new_with_reason(cancel, config.deadline, sink.abort_reason());
    let execution = execution_config(config);
    let mut message = message.clone();
    match transform_and_prepare(&mut message, &[], &execution, execution.tracing(), &ctl).await {
        Ok(prepared) => execute_calls(&message, &[], &execution, &ctl, sink, prepared)
            .await
            .into_iter()
            .map(|receipt| receipt.model)
            .collect(),
        Err(error) => {
            let mut results = Vec::new();
            for call in message.tool_calls() {
                results.push(
                    emit_synthetic_result(sink, call, "error", Some(&error.0))
                        .await
                        .model,
                );
            }
            results
        }
    }
}
