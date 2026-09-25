//! ARA agent loop. Ported from OMP `packages/agent` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d (MIT, see THIRD_PARTY_NOTICES.md).

pub mod agent_loop;
pub mod event;
pub mod tool;

pub use agent_loop::{
    AgentConfig, LoopError, LoopHooks, NoHooks, UnpairedTail, agent_loop, agent_loop_continue, execute_tool_calls,
    unpaired_tool_call_tail,
};
pub use event::{AgentEvent, AgentEventSink, NullSink, RecordingSink};
pub use tool::{AgentTool, Concurrency, ToolDecision, ToolError, ToolOutput, UpdateFn};
