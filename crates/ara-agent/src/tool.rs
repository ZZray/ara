//! Tool port executed by the agent loop (OMP `AgentTool`, `AgentToolResult`).

use ara_ai::{JsonObject, Tool, UserBlock};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// How a call may overlap with its siblings in one assistant turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Concurrency {
    /// May run alongside other shared calls.
    Shared,
    /// Waits for everything before it; later calls wait for it.
    Exclusive,
}

/// Result produced by a tool (`AgentToolResult`).
#[derive(Clone, Debug, PartialEq)]
pub struct ToolOutput {
    pub content: Vec<UserBlock>,
    pub details: Option<Value>,
    /// Non-throwing failure flagged by the tool itself.
    pub is_error: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        ToolOutput { content: vec![UserBlock::text(text)], details: None, is_error: false }
    }
    pub fn error(text: impl Into<String>) -> Self {
        ToolOutput { content: vec![UserBlock::text(text)], details: None, is_error: true }
    }
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

/// A thrown tool failure; becomes an error result carrying the message.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolError(pub String);

impl<T: std::fmt::Display> From<T> for ToolError {
    fn from(value: T) -> Self {
        ToolError(value.to_string())
    }
}

/// Streams partial results (`tool_execution_update`).
pub type UpdateFn = std::sync::Arc<dyn Fn(ToolOutput) + Send + Sync>;

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> Tool;

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }

    /// Run the call. `cancel` fires on run abort; tools must stop promptly and
    /// report what they already did.
    async fn execute(
        &self,
        call_id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError>;
}

/// Host decision before a validated call executes (OMP `beforeToolCall`).
#[derive(Clone, Debug, PartialEq)]
pub enum ToolDecision {
    Allow,
    /// Block with an optional reason (default "Tool execution was blocked").
    Block(Option<String>),
}
