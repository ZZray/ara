//! ARA agent loop. Ported from OMP `packages/agent` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, see THIRD_PARTY_NOTICES.md).

pub mod agent;
pub mod agent_loop;
pub mod agent_message;
pub mod aside;
pub mod pause;
pub use aside::{
    AsideCallbacks, AsideCommit, AsideDiscard, AsideError, AsideFactory, AsideMessage,
    AsideMessageProvider, BeforeYieldHook,
};
pub use pause::{AGENT_PAUSE_GATE, AgentPauseGate, AgentPauseListener, PauseSubscription};
pub mod append_only_context;
mod arg_stream;
pub mod event;
pub mod final_provider_gate;
pub mod provider_context;
pub mod soft_tool_requirement;
pub use soft_tool_requirement::{
    SoftToolPredicate, SoftToolRequirement, SoftToolRequirementSnapshot, SoftToolRequirementState,
    ToolChoiceDirective, ToolChoiceDirectiveSelection,
};
pub mod tokenizer;
pub mod tool;
pub mod tool_directives;
pub mod tool_inventory;

pub use agent::{Agent, AgentAdmissionHook, AgentError, AgentInput, HookRegistration, QueueMode};
pub use agent_loop::{
    ActualAfterToolCallContext, ActualAgentContextView, ActualBeforeToolCallContext,
    ActualToolDecision, ActualTurnEndContext, AdmissionError, AfterToolCallContext,
    AfterToolCallResult, AgentConfig, AgentContextView, BeforeToolCallContext,
    BeforeToolCallResult, ExecutionSnapshot, InterruptMode, IrcInterruptPeek, LoopError, LoopHooks,
    ModelExecutionSnapshot, NoHooks, QueueInputKind, RunAbortReason, RunEnd, RunReport,
    RunStopCause, SteeringInterruptBinding, SteeringQueuePeek, SteeringQueueState, SteeringSource,
    SteeringSubscription, SteeringWake, TerminalToolResultHandle, ToolHookError,
    ToolScopedAbortReason, TurnEndContext, UnpairedTail, agent_loop, agent_loop_continue,
    agent_loop_continue_raw, agent_loop_inputs, agent_loop_model, agent_loop_raw,
    coerce_raw_tool_result, execute_tool_calls, try_agent_loop_raw, unpaired_raw_tool_call_tail,
    unpaired_tool_call_tail,
};
pub use agent_message::{
    AgentMessage, AgentMessageBody, AgentMessageIdentity, CurrentArgumentsSlot,
    GraphAdmissionError, GraphOwnerError, RawContextError, checked_model_equal,
    checked_model_messages, checked_record_equal, default_agent_message_conversion,
};
pub use append_only_context::{
    AppendOnlyContextManager, AppendOnlyLog, BuildOptions, SharedAppendOnlyContextManager,
    StablePrefix, StablePrefixSnapshot,
};
pub use event::{AgentEvent, AgentEventSink, NullSink, RecordingSink, emit_raw_input};
pub use final_provider_gate::{
    FinalProviderGate, FinalProviderGateBinding, FinalProviderGateError, FinalProviderGateStop,
    GateRegistration,
};
pub use provider_context::{
    ActualProviderContext, ActualProviderError, OwnedProviderContextPort, ProviderContextBinding,
    ProviderContextError, SessionValues, build_side_request_context,
    build_side_request_context_from_snapshot, normalize_messages_for_provider, normalize_tools,
    render_tool_examples, render_tool_examples_jsdoc,
};
pub use tool::{
    ActualExecuteRequest, ActualToolBinding, ActualToolExecutionContext, ActualToolInvoker,
    ActualToolResultSanitizer, ActualToolSource, ActualUpdateFn, AgentTool, Concurrency,
    ToolArgStream, ToolArgStreamEmit, ToolArgStreamInit, ToolCallInfo, ToolDecision, ToolError,
    ToolExecutionContext, ToolIntentMode, ToolOutput, UpdateFn, cursor_exec_resolved_key,
};
pub use tool_directives::{ToolChoiceSelection, ToolDirectiveError};
pub use tool_inventory::{JsonSchemaToTsOptions, json_schema_to_typescript, render_tool_inventory};
pub mod compaction;
pub mod handoff;
pub mod remote;
