//! Tool port executed by the agent loop (OMP `AgentTool`, `AgentToolResult`).

use crate::agent_message::{AgentMessage, CurrentArgumentsSlot};
use crate::provider_context::{ActualProviderError, SessionValues};
pub use ara_ai::graph_values::cursor_exec_resolved_key;
use ara_rpc::{
    WireString,
    value_heap::{ValueError, ValueHandle},
};
use std::sync::Arc;

use ara_ai::{CustomToolFormat, JsonObject, Tool, ToolCallProviderMetadata, ToolResultProviderMetadata, UserBlock};
use async_trait::async_trait;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

#[path = "tool_sanitize.rs"]
mod sanitize;
pub use sanitize::ActualToolResultSanitizer;

/// Host-owned original AgentContext.tools source, separate from provider wire
/// advertisement. Acquisition retains a capability; metadata is read only at
/// the Native resolver/validation positions. Host owns member-to-capability
/// association and grants; Core does not rebuild a name/index registry.
pub trait ActualToolSource: Send + Sync {
    /// Current original collection. None means it is absent, not an empty Graph.
    /// Membership and Graph metadata stay live; do not normalize/import here.
    fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>, ActualProviderError>;

    /// Associate exactly this selected original member with its Host capability.
    /// This identity-only admission must not read metadata or execute a tool.
    fn bind_selected(
        &self,
        selected: &ValueHandle,
        values: &SessionValues,
    ) -> Result<ActualToolBinding, ActualProviderError>;

    /// Host callback presence, stable for this resolution's fallback binding.
    /// This is not permission. True preserves the explicit Unsupported default;
    /// false skips fallback argument evaluation, matching Native optional call.
    fn has_fallback(&self) -> bool {
        true
    }

    /// Called after both source searches miss with the original name Get value.
    /// No string coercion; Host determines its accepted domain. Unadvertised tools are valid;
    /// a typed fallback/serde import cannot manufacture their actual source.
    fn resolve_fallback(
        &self,
        _name: &ValueHandle,
        _values: &SessionValues,
    ) -> Result<Option<ActualToolBinding>, ActualProviderError> {
        Err(ActualProviderError::Unsupported("actual Host tool fallback"))
    }
}

/// Transient association of the selected original Graph tool and original Host
/// capability. This receipt does not grant permission or enable actual execute.
#[derive(Clone)]
pub struct ActualToolBinding {
    pub source: ValueHandle,
    pub capability: Arc<dyn AgentTool>,
}

pub type ActualUpdateFn = Arc<dyn Fn(ValueHandle) -> Result<(), ActualProviderError> + Send + Sync>;

/// Native getToolContext input; Host identity/grants are separate from this
/// portable batch attribution. All metadata remains in the original heap.
pub struct ActualToolExecutionContext {
    pub batch_id: WireString,
    pub index: usize,
    pub total: usize,
    pub tool_calls: ValueHandle,
    pub provider_metadata: ValueHandle,
    pub steering_signal: CancellationToken,
}

/// Exact source-method invocation. The Host materializes the actual signal and
/// fresh update Function and invokes these five native arguments once. It must
/// admit the exact current callable/receiver and final effect permission; a
/// stale Arc/name/source selection is not an authorization receipt.
pub struct ActualExecuteRequest {
    pub callable: ValueHandle,
    pub receiver: ValueHandle,
    pub owner: AgentMessage,
    pub tool_call: ValueHandle,
    pub arguments_slot: CurrentArgumentsSlot,
    pub call_id: WireString,
    pub args: ValueHandle,
    pub values: Arc<SessionValues>,
    pub cancel: CancellationToken,
    pub on_update: ActualUpdateFn,
    pub tool_context: ValueHandle,
}

impl ActualExecuteRequest {
    /// Current raw owner/lineage admission only. The original Host must still
    /// check exact callable/receiver association and the final effect grant.
    /// This performs no Get(execute), arguments read, or invocation itself.
    pub fn check_admission(&self) -> Result<(), ActualProviderError> {
        self.arguments_slot.require_owned_for(&self.owner, &self.tool_call)?;
        for handle in [&self.callable, &self.receiver, &self.tool_call, &self.args, &self.tool_context] {
            self.values.check_handle(handle)?;
        }
        if self.values.heap().kind(&self.callable)? != ara_rpc::value_heap::ValueKind::Function {
            return Err(ValueError::TypeError("Value is not callable".into()).into());
        }
        Ok(())
    }
}

#[async_trait]
pub trait ActualToolInvoker: Send + Sync {
    /// Settles the original method's Host-owned Future to its actual result or
    /// thrown receipt. Cancellation never proves an entered effect did not run.
    /// No synchronous block_on, typed/serde fallback, or duplicate method Call.
    async fn invoke_settled(&self, request: ActualExecuteRequest) -> Result<ValueHandle, ActualProviderError>;
}

impl std::fmt::Debug for ActualToolBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("ActualToolBinding").field("source", &self.source).finish_non_exhaustive()
    }
}

impl ActualToolBinding {
    pub fn check_lineage(&self, values: &SessionValues) -> Result<(), ActualProviderError> {
        values.check_handle(&self.source)?;
        Ok(())
    }

    /// Checks Graph lineage and exact selection without a metadata/name read.
    /// The Host remains responsible for associating its capability with source.
    pub fn check_selected(&self, selected: &ValueHandle, values: &SessionValues) -> Result<(), ActualProviderError> {
        values.check_handle(selected)?;
        self.check_lineage(values)?;
        if !self.source.same_identity(selected) {
            return Err(ActualProviderError::Protocol("actual Host tool binding changed selected source identity"));
        }
        Ok(())
    }
}

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
    pub provider_metadata: Option<ToolResultProviderMetadata>,
    pub useless: bool,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![UserBlock::text(text)],
            details: None,
            is_error: false,
            provider_metadata: None,
            useless: false,
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        ToolOutput {
            content: vec![UserBlock::text(text)],
            details: None,
            is_error: true,
            provider_metadata: None,
            useless: false,
        }
    }
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    pub(crate) fn json(&self) -> Value {
        let mut raw = serde_json::json!({"content": self.content});
        if let Some(details) = &self.details {
            raw["details"] = details.clone();
        }
        if self.is_error {
            raw["isError"] = Value::Bool(true);
        }
        if let Some(metadata) = &self.provider_metadata {
            raw["providerMetadata"] = serde_json::json!(metadata);
        }
        if self.useless {
            raw["useless"] = Value::Bool(true);
        }
        raw
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallInfo {
    pub id: String,
    pub name: String,
}

/// Portable batch attribution plus cooperative steering. This is not a Host
/// identity/permission context and does not itself authorize the operation.
#[derive(Clone)]
pub struct ToolExecutionContext {
    pub batch_id: String,
    pub index: usize,
    pub total: usize,
    pub tool_calls: std::sync::Arc<[ToolCallInfo]>,
    pub provider_metadata: Option<ToolCallProviderMetadata>,
    pub steering_signal: CancellationToken,
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

/// Emits a serializable argument preview, never a tool result or receipt.
/// The callback may be retained during its provider attempt; after attempt
/// finalization/drop it is inert and does not keep the attempt alive.
pub type ActualToolArgStreamArguments = ValueHandle;

pub type ToolArgStreamEmit = std::sync::Arc<dyn Fn(Value) + Send + Sync>;

pub struct ToolArgStreamInit {
    pub tool_call_id: String,
    pub tool_name: String,
    pub custom_wire_name: Option<String>,
    pub emit: ToolArgStreamEmit,
}

/// Optional synchronous receiver of raw wire argument fragments. A callback
/// error/panic is nonfatal; push failure still permits end or cancel.
pub trait ToolArgStream: Send {
    fn push(&mut self, delta: &str) -> Result<(), ToolError>;
    /// Provider-final parsed arguments before execution preparation/revisions.
    /// A completed receiver is removed even when this callback fails.
    fn end(&mut self, args: &JsonObject) -> Result<(), ToolError>;
    /// Original Graph arguments at the same provider-final point. An actual
    /// consumer opts in explicitly; Core never exports these arguments to serde.
    fn end_actual(&mut self, _args: &ValueHandle, _values: &SessionValues) -> Result<(), ToolError> {
        Err(ToolError("argument preview has no actual end binding".into()))
    }
    /// Release an unfinished receiver on an attempt exit or dropped future.
    fn cancel(&mut self) -> Result<(), ToolError>;
}

/// Fixed Native AgentTool.intent policy. Callback-valued intent uses Derived.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolIntentMode {
    #[default]
    Require,
    Optional,
    Omit,
    Derived,
}

#[async_trait]
pub trait AgentTool: Send + Sync {
    fn definition(&self) -> Tool;

    /// Explicit actual opt-in on this same Host capability. Typed methods keep
    /// their original contracts; absence never serializes Graph into execute.
    fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
        None
    }

    fn intent_mode(&self) -> ToolIntentMode {
        ToolIntentMode::Require
    }

    /// Derive an intent from execution arguments without the injected i field.
    /// Called only for Derived mode; a callback panic cannot break execution.
    fn derive_intent(&self, _args: &JsonObject) -> Option<String> {
        None
    }

    /// Optional Native custom-format metadata, independent of logical grants.
    fn custom_format(&self) -> Option<CustomToolFormat> {
        None
    }

    fn custom_wire_name(&self) -> Option<String> {
        None
    }

    /// Preserve definitions supplied by hosts while adding tool-owned metadata.
    /// Ordinary implementations need not implement the optional methods.
    fn protocol_definition(&self) -> Tool {
        let mut definition = self.definition();
        definition.custom_format = self.custom_format().or(definition.custom_format);
        definition.custom_wire_name = self.custom_wire_name().or(definition.custom_wire_name);
        definition
    }

    /// Open only at the provider's toolcall_start, before any argument delta.
    /// None opts out; finalized/done-only calls never open a receiver.
    fn open_arg_stream(&self, _init: ToolArgStreamInit) -> Result<Option<Box<dyn ToolArgStream>>, ToolError> {
        Ok(None)
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }

    fn lenient_argument_validation(&self) -> bool {
        false
    }

    /// Argument-dependent policy, resolved once on prepared args. A panic
    /// defaults to retaining the tool outcome, never to interrupting it.
    fn interruptible(&self, _args: &JsonObject) -> bool {
        false
    }

    async fn execute_with_context(
        &self,
        call_id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
        _context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.execute(call_id, args, cancel, update).await
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
