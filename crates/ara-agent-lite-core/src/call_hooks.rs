//! Experimental per-tool-call callbacks for embedding Hosts.
//!
//! The DTOs deliberately own their data. Hosts do not receive `rpi_agent`
//! callback lifetimes or mutable loop internals, and the default `None` path
//! keeps the existing Run behavior unchanged.

use anyhow::Result;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct BeforeToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BeforeToolCallDecision {
    pub allow: bool,
    pub reason: Option<String>,
    pub terminate: bool,
}

impl BeforeToolCallDecision {
    pub fn allow() -> Self {
        Self { allow: true, ..Self::default() }
    }

    pub fn deny(reason: impl Into<String>) -> Self {
        Self { allow: false, reason: Some(reason.into()), ..Self::default() }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AfterToolCall {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub output: String,
    pub details: Option<Value>,
    pub is_error: bool,
}

/// Host-owned synchronous hook contract. Implementations may record or
/// authorize calls, but they do not receive a capability to execute tools.
pub trait HostCallHooks: Send + Sync {
    fn before_tool_call(&self, call: BeforeToolCall) -> Result<BeforeToolCallDecision>;
    fn after_tool_call(&self, call: AfterToolCall) -> Result<()>;
}
