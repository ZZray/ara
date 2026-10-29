//! Pending preview resolution, ported from fixed OMP 596f2da tools/resolve.ts
//! and session/tool-choice-queue.ts (MIT; THIRD_PARTY_NOTICES.md).
//! A Session owns this registry. The existing loop owns SoftToolRequirement
//! escalation; the existing write action owns the actual apply effect.

use ara_agent::{AgentMessage, SoftToolRequirement, ToolError, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub const PREVIEW_PENDING_NOTICE: &str = "Staged as a proposal — files NOT modified yet. To apply: write a one-sentence reason to xd://resolve. To discard: write to xd://reject.";
static NEXT_PREVIEW: AtomicU64 = AtomicU64::new(0);

#[async_trait]
pub trait PendingResolution: Send + Sync {
    /// Native AST exposes only its original scalar effect continuation. The
    /// Session's CLI module owns the Graph binding; no worker can read it.
    fn native_ast_stage(&self) -> Option<crate::ast_edit::NativeAstStage> {
        None
    }
    async fn apply(&self, reason: &str) -> Result<ToolOutput, PendingApplyFault>;
    /// Current write owner supplies its context after the preview has ended.
    /// Existing resolution actions retain their original contract by default.
    async fn apply_in_context(&self, reason:&str, _ctx:&crate::ToolContext)->Result<ToolOutput,PendingApplyFault> {
        self.apply(reason).await
    }
    async fn reject(&self, _reason: &str) -> Result<Option<ToolOutput>, ToolError> {
        Ok(None)
    }
}

/// Fixed runResolveInvocation preserves ToolError and wraps ordinary native
/// Error. AST native computation reports the latter; other tools may report
/// an already authored ToolError without receiving a second prefix.
#[derive(Debug)]
pub enum PendingApplyFault {
    Tool(ToolError),
    Native(String),
}
impl From<ToolError> for PendingApplyFault {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}

/// A Host's existing in-flight directive owns its invocation and lifecycle.
/// This narrow port supplies only peekQueueInvoker precedence; it creates no
/// directive queue or new Agent state machine in the shared Tools crate.
#[async_trait]
pub trait QueueResolutionInvoker: Send + Sync {
    async fn invoke(&self, action: &str, reason: &str) -> Result<ToolOutput, ToolError>;
}

#[derive(Clone)]
struct Pending {
    id: String,
    source_tool: String,
    label: String,
    action: Arc<dyn PendingResolution>,
}

/// Same-ID registration replaces just that entry and makes it the newest
/// head. Unrelated concurrent previews retain their original owners.
#[derive(Clone, Default)]
pub struct PendingResolutionQueue(
    Arc<Mutex<Vec<Pending>>>,
    ara_agent::SoftToolRequirementState,
    Arc<Mutex<Option<Arc<dyn QueueResolutionInvoker>>>>,
);

#[derive(Clone)]
pub struct NativeAstSelection {
    pub id: String,
    pub stage: crate::ast_edit::NativeAstStage,
    pending: Pending,
    source_tool: String,
    label: String,
}
impl PendingResolutionQueue {
    pub fn native_ast_head(&self) -> Option<NativeAstSelection> {
        // Original peekQueueInvoker precedence always wins over previews.
        if self.2.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
            return None;
        }
        let pending = self.0.lock().unwrap_or_else(|e| e.into_inner()).last().cloned()?;
        Some(NativeAstSelection {
            id: pending.id.clone(),
            stage: pending.action.native_ast_stage()?,
            source_tool: pending.source_tool.clone(),
            label: pending.label.clone(),
            pending,
        })
    }
    pub fn restore_native_ast(&self, pending: &NativeAstSelection) {
        self.register_entry(pending.pending.clone());
    }
    pub fn discard_native_ast(&self, pending: &NativeAstSelection, reason: &str) -> ToolOutput {
        self.finish_native_ast(
            pending,
            "reject",
            reason,
            ToolOutput::text(format!("Discarded: {}. Reason: {reason}", pending.label)),
        )
    }
    pub fn finish_native_ast(
        &self,
        pending: &NativeAstSelection,
        device: &str,
        reason: &str,
        output: ToolOutput,
    ) -> ToolOutput {
        self.remove(&pending.id);
        let mut inner = json!({"action":if device == "resolve" { "apply" } else { "discard" }, "reason":reason, "sourceToolName":pending.source_tool, "label":pending.label});
        if let Some(source) = &output.details {
            inner["sourceResultDetails"] = source.clone();
        }
        output.with_details(json!({"xdev":{"tool":device,"mode":"execute","args":{"reason":reason},"inner":inner}}))
    }

    pub fn soft_state(&self) -> ara_agent::SoftToolRequirementState {
        self.1.clone()
    }
    pub fn set_queue_invoker(&self, invoker: Option<Arc<dyn QueueResolutionInvoker>>) {
        *self.2.lock().unwrap_or_else(|e| e.into_inner()) = invoker;
    }
    pub fn register(&self, source_tool: &str, label: String, action: Arc<dyn PendingResolution>) -> String {
        let id = format!("pending-action:{source_tool}:{}", NEXT_PREVIEW.fetch_add(1, Ordering::Relaxed));
        self.register_entry(Pending { id: id.clone(), source_tool: source_tool.into(), label, action });
        id
    }
    fn register_entry(&self, pending: Pending) {
        let mut entries = self.0.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|entry| entry.id != pending.id);
        entries.push(pending);
    }
    pub fn remove(&self, id: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|entry| entry.id != id);
    }
    pub fn clear(&self) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    pub fn pending_head(&self) -> Option<(String, String)> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).last().map(|p| (p.id.clone(), p.source_tool.clone()))
    }
    pub fn soft_requirement(&self) -> Option<SoftToolRequirement> {
        self.soft_requirement_with(|body| AgentMessage::from_raw(body, None)).expect("fixed custom reminder shape")
    }
    /// The Host admits this newly authored reminder at its creation boundary.
    /// Existing historical and received messages do not pass through this adapter.
    pub fn soft_requirement_with<E>(
        &self,
        create_reminder: impl FnOnce(serde_json::Value) -> Result<AgentMessage, E>,
    ) -> Result<Option<SoftToolRequirement>, E> {
        let Some((id, source_tool)) = self.pending_head() else {
            return Ok(None);
        };
        let reminder = create_reminder(json!({
            "role":"custom", "customType":"resolve-reminder", "display":false,
            "content":format!("<system-reminder>\n`{source_tool}` result above: PREVIEW — no files changed. Finalize now with `write`: write a one-sentence plain-text reason to `xd://resolve` to APPLY, or `xd://reject` to DISCARD.\n</system-reminder>"),
            "details":{"toolName":source_tool},"attribution":"agent",
            "timestamp":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64
        }))?;
        Ok(Some(SoftToolRequirement {
            id,
            tool_name: "write".into(),
            satisfies: Some(Arc::new(is_preview_resolution_call)),
            reminder: vec![reminder],
        }))
    }
    /// No token is forwarded into the staged apply: fixed OMP re-runs with no
    /// preview signal. Await settlement through the current write effect owner.
    pub async fn dispatch(&self, device: &str, body: &str) -> Result<ToolOutput, ToolError> {
        self.dispatch_inner(device,body,None).await
    }
    pub async fn dispatch_in_context(&self,device:&str,body:&str,ctx:&crate::ToolContext)->Result<ToolOutput,ToolError> {
        self.dispatch_inner(device,body,Some(ctx)).await
    }
    async fn dispatch_inner(&self,device:&str,body:&str,ctx:Option<&crate::ToolContext>)->Result<ToolOutput,ToolError> {
        let reason = body.trim();
        let apply = device == "resolve";
        let action = if apply { "apply" } else { "discard" };
        let queue_invoker = self.2.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(invoker) = queue_invoker {
            let output = invoker.invoke(action, reason).await?;
            let mut xdev = json!({"tool":device,"mode":"execute","args":{"reason":reason}});
            if let Some(details) = &output.details {
                xdev["inner"] = details.clone();
            }
            return Ok(output.with_details(json!({"xdev":xdev})));
        }
        let pending = self.0.lock().unwrap_or_else(|e| e.into_inner()).last().cloned();
        let Some(pending) = pending else {
            if apply {
                return Err(ToolError(
                    "No pending action to apply — xd://resolve is only valid while a staged preview is pending.".into(),
                ));
            }
            self.clear();
            let inner = json!({"action":action,"reason":reason});
            return Ok(ToolOutput::text("Nothing to reject; no pending action remains.").with_details(
                json!({"xdev":{"tool":device,"mode":"execute","args":{"reason":reason},"inner":inner}}),
            ));
        };
        let output = if apply {
            let applied=match ctx {
                Some(ctx)=>pending.action.apply_in_context(reason,ctx).await,
                None=>pending.action.apply(reason).await,
            };
            match applied {
                Ok(output) => output,
                Err(error) => {
                    self.register_entry(pending);
                    return Err(match error {
                        PendingApplyFault::Tool(error) => error,
                        PendingApplyFault::Native(message) => ToolError(format!("Apply failed: {message}")),
                    });
                }
            }
        } else {
            pending
                .action
                .reject(reason)
                .await?
                .unwrap_or_else(|| ToolOutput::text(format!("Discarded: {}. Reason: {reason}", pending.label)))
        };
        // A returned isError (including stale-after-write) is a settled result,
        // unlike a thrown failure: consume this exact ID in both return cases.
        self.remove(&pending.id);
        let mut inner =
            json!({"action":action,"reason":reason,"sourceToolName":pending.source_tool,"label":pending.label});
        if let Some(source) = &output.details {
            inner["sourceResultDetails"] = source.clone();
        }
        Ok(output.with_details(json!({"xdev":{"tool":device,"mode":"execute","args":{"reason":reason},"inner":inner}})))
    }
}

/// Prefix is case insensitive; device itself is case sensitive, with no
/// slash/query/fragment accepted (xd-protocol.ts parseXdUrl).
pub fn resolution_device(path: &str) -> Option<&str> {
    let path = path.trim();
    if !path.get(..5).is_some_and(|p| p.eq_ignore_ascii_case("xd://")) {
        return None;
    }
    match &path[5..] {
        "resolve" => Some("resolve"),
        "reject" => Some("reject"),
        _ => None,
    }
}

pub fn is_preview_resolution_call(call: &ara_ai::ToolCall) -> bool {
    if call.name != "write" {
        return false;
    }
    let path = call
        .arguments
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| call.arguments.get("file_path").and_then(|v| v.as_str()));
    path.is_some_and(|path| resolution_device(path).is_some())
}
