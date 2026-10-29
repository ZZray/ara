//! Concrete Host binding for the existing seven builtin action owners.
//! Fixed OMP 596f2da (MIT; THIRD_PARTY.md). Original call/args remain in their
//! Session realm; only selected OS scalars and fresh effect output cross ports.
//! These kernels remain the recorded Rust subsets, not complete tool parity.

pub(crate) mod approval;

mod ast_edit_input;
mod ast_edit_native;
mod ast_grep_native;
mod bun_glob;
#[cfg(test)]
mod local_original4_tests;
pub(crate) mod local_source;
pub(crate) mod omp_string_object;
pub mod xdev_docs;
pub(crate) mod xdev_native;
pub use approval::{ApprovalPolicy, ResolvedApproval, ToolTier};
pub use bun_glob::install as install_bun_glob;
pub use xdev_native::{
    NativeDeviceApprovalConfiguration, NativeDeviceApprovalHost, NativeDeviceApprovalRequest,
    NativeDeviceDecorator, NoUiDeviceApproval,
};
pub mod async_effect;
pub mod bash_native;
pub mod wait_native;
pub mod task_native;
mod edit_batch;
mod edit_inspection;
pub(crate) mod read_attachments;
pub mod read_continuation;
mod read_delimited;

use crate::native_factory::catalog::{NativeCatalogSnapshot, NativeToolCatalog};

use xdev_native::NativeMethodRequest;

use crate::native_realm::{
    parent::NativeParentId,
    promise::{NativePendingPromise, NativePromiseState},
};
use async_effect::{
    LegacyEffect, NativeBuiltinRun, NativeEffectFuture, NativeToolFault, ParentRegistration,
};

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicU64, Ordering},
    },
};

use ara_agent::{
    ActualExecuteRequest, ActualProviderError, ActualToolBinding, ActualToolDecision,
    ActualToolExecutionContext, ActualToolInvoker, ActualToolResultSanitizer, ActualToolSource,
    AgentTool, Concurrency, SessionValues, ToolError, ToolOutput, UpdateFn,
};
use ara_ai::{JsonObject, Tool};
use ara_rpc::value_heap::{
    CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, OpaqueKind,
    OpaqueOperation, PropertyDescriptor, PropertyKey, RuntimeReply, RuntimeRequest, StringifyMode,
    ValueError, ValueHandle, ValueRuntime, ValueSnapshot,
};
use ara_tools::{
    ToolContext,
    ast_edit::AstEditTool,
    ast_grep::{AstGrepOsValues, AstGrepTool},
    bash::{BashTool, BashUpdateFn},
    default_declarations::{DebugTool, WebSearchTool},
    edit::{EditCall, EditTool},
    glob::{GlobOsValues, GlobTool},
    grep::{GrepOsValues, GrepTool},
    read::ReadTool,
    write::WriteTool,
};
use async_trait::async_trait;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

struct NativeEditBlackboxSerializer(async_effect::NativeEffectPublisher);
struct NativeEditRepairDispatcher(async_effect::NativeEffectPublisher);
#[async_trait]
impl ara_tools::edit_post_apply::EditRepairCompletionDispatcher for NativeEditRepairDispatcher {
    async fn complete(
        &self,
        completion: Arc<dyn ara_tools::edit_post_apply::EditRepairCompletion>,
        prompt: String,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        self.0
            .complete_edit_repair(completion, prompt, cancel)
            .await
    }
}
#[async_trait]
impl ara_tools::edit_post_apply::EditBlackboxSerializer for NativeEditBlackboxSerializer {
    async fn stringify(
        &self,
        record: ara_tools::edit_post_apply::EditBlackboxRecord,
    ) -> Result<String, String> {
        self.0.stringify_edit_blackbox(record).await
    }
}

struct NativeWriteProgress(async_effect::NativeEffectPublisher);
#[async_trait]
impl ara_tools::write::WriteProgress for NativeWriteProgress {
    async fn acknowledge(&self, output: ToolOutput) -> Result<(), ToolError> {
        self.0.acknowledge_write_progress(output).await
    }
    async fn prepare_batch(&self) -> Result<Option<ara_tools::edit_host::EditLspBatch>, ToolError> {
        self.0.prepare_write_batch().await
    }
}

// This concrete Host owns this reserved capability range. A composed Host must
// reserve it in its resolver too; IDs are never reused across invocations/realms.
static NEXT_CAPABILITY: AtomicU64 = AtomicU64::new(1 << 63);
fn capability() -> Result<CapabilityId, ValueError> {
    NEXT_CAPABILITY
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(CapabilityId)
        .map_err(|_| ValueError::HeapTokensExhausted)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinKind {
    Read,
    Write,
    Bash,
    Edit,
    Grep,
    Glob,
    AstGrep,
    AstEdit,
    Debug,
    WebSearch,
    Wait,
    Task,
}
#[derive(Clone)]
enum Kernel {
    Read(Arc<ReadTool>),
    Write(Arc<WriteTool>),
    Bash(Arc<BashTool>),
    Edit(Arc<EditTool>),
    Grep(Arc<GrepTool>),
    Glob(Arc<GlobTool>),
    AstGrep(Arc<AstGrepTool>),
    AstEdit(Arc<AstEditTool>),
    Debug(Arc<DebugTool>),
    WebSearch(Arc<WebSearchTool>),
    Wait(Arc<wait_native::WaitTool>),
    Task(Arc<task_native::TaskTool>),
}
impl Kernel {
    fn tool(&self) -> &dyn AgentTool {
        match self {
            Self::Read(tool) => tool.as_ref(),
            Self::Write(tool) => tool.as_ref(),
            Self::Bash(tool) => tool.as_ref(),
            Self::Edit(tool) => tool.as_ref(),
            Self::Grep(tool) => tool.as_ref(),
            Self::Glob(tool) => tool.as_ref(),
            Self::AstGrep(tool) => tool.as_ref(),
            Self::AstEdit(tool) => tool.as_ref(),
            Self::Debug(tool) => tool.as_ref(),
            Self::WebSearch(tool) => tool.as_ref(),
            Self::Wait(tool)=>tool.as_ref(),
            Self::Task(tool)=>tool.as_ref(),
        }
    }
    fn kind(&self) -> BuiltinKind {
        match self {
            Self::Read(_) => BuiltinKind::Read,
            Self::Write(_) => BuiltinKind::Write,
            Self::Bash(_) => BuiltinKind::Bash,
            Self::Edit(_) => BuiltinKind::Edit,
            Self::Grep(_) => BuiltinKind::Grep,
            Self::Glob(_) => BuiltinKind::Glob,
            Self::AstGrep(_) => BuiltinKind::AstGrep,
            Self::AstEdit(_) => BuiltinKind::AstEdit,
            Self::Debug(_) => BuiltinKind::Debug,
            Self::WebSearch(_) => BuiltinKind::WebSearch,
            Self::Wait(_)=>BuiltinKind::Wait,
            Self::Task(_)=>BuiltinKind::Task,
        }
    }
}
struct Member {
    kernel: Kernel,
    source: ValueHandle,
    execute: ValueHandle,
    method: CapabilityId,
    capability: Arc<BoundTool>,
}
struct Grant {
    enabled: bool,
    entered: HashMap<CapabilityId, CancellationToken>,
}

struct BatchContextSignal {
    tool_calls: ValueHandle,
    signal: ValueHandle,
    token: CancellationToken,
}
impl BatchContextSignal {
    fn for_context(&self, context: &ActualToolExecutionContext) -> Option<ValueHandle> {
        (self.tool_calls.same_identity(&context.tool_calls)
            && self.token == context.steering_signal)
            .then(|| self.signal.clone())
    }
}
#[derive(Default)]
struct ContextSignals {
    batches: HashMap<(ara_rpc::WireString, ara_rpc::value_heap::NodeId), Vec<BatchContextSignal>>,
    capabilities: HashMap<CapabilityId, (ValueHandle, CancellationToken)>,
}

/// Original tools and current permission owner for exactly one logical Session.
/// The Weak values link prevents a runtime/values/source ownership cycle.
pub struct BuiltinToolSource {
    values: Weak<SessionValues>,
    collection: ValueHandle,
    members: Vec<Member>,
    grants: Arc<Mutex<Vec<Grant>>>,
    context_signals: Mutex<ContextSignals>,
    native_run: Mutex<Option<Weak<NativeBuiltinRun>>>,
    read_session: Mutex<Option<DeferredValue>>,
    bash_session: Arc<bash_native::SessionBashOwner>,
    wait_session:Arc<wait_native::SessionWaitOwner>,
    task_session:Mutex<Arc<task_native::SessionTaskOwner>>,
    task_metadata:HashMap<CapabilityId,(ValueHandle,&'static str)>,
    ast_module: OnceLock<Arc<crate::native_realm::ast::NativeAstModule>>,
    device_snapshot: Mutex<Option<Weak<NativeCatalogSnapshot>>>,
    approval_paths: Mutex<HashMap<CapabilityId, (ValueHandle, ToolContext)>>,
    write_descriptions: HashMap<CapabilityId, (ValueHandle, Arc<WriteTool>)>,
    edit_descriptions: HashMap<CapabilityId, (ValueHandle, Arc<EditTool>, &'static str)>,
}
impl BuiltinToolSource {
    pub fn new(
        values: &Arc<SessionValues>,
        context: ToolContext,
        kinds: &[BuiltinKind],
    ) -> Result<Arc<Self>, ActualProviderError> {
        Self::new_with_bash_session(
            values,
            context,
            kinds,
            bash_native::SessionBashOwner::new(bash_native::BashSessionSettings::default()),
        )
    }

    pub(crate) fn bind_ast_module(
        &self,
        module: Arc<crate::native_realm::ast::NativeAstModule>,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        module.check_heap(values.heap())?;
        if let Some(current) = self.ast_module.get() {
            return if Arc::ptr_eq(current, &module) {
                Ok(())
            } else {
                Err(ActualProviderError::Protocol(
                    "Original AST module binding cannot be replaced",
                ))
            };
        }
        self.ast_module
            .set(module.clone())
            .map_err(|_| ActualProviderError::Protocol("Concurrent original AST module binding"))?;
        self.install_builtin_approval(values, &module, &values.heap().array(Vec::new())?)
    }
    fn install_builtin_approval(
        &self,
        values: &SessionValues,
        module: &crate::native_realm::ast::NativeAstModule,
        bash_patterns: &ValueHandle,
    ) -> Result<(), ActualProviderError> {
        let heap = values.heap();
        let runtime = values.runtime();
        for member in &self.members {
            let kind = member.kernel.kind();
            let (label, mode, summary) = match kind {
                BuiltinKind::Task => ("Task", "essential", Some("Spawn subagents to complete delegated tasks")),
                BuiltinKind::Read => ("Read", "essential", None),
                BuiltinKind::Write => ("Write", "essential", None),
                BuiltinKind::Bash => ("Bash", "essential", None),
                BuiltinKind::Wait=>("Wait","essential",Some("Wait for the next result of a background job or service you started")),
                BuiltinKind::Glob => ("Glob", "essential", None),
                BuiltinKind::Grep => (
                    "Grep",
                    "discoverable",
                    Some("Grep file contents using ripgrep (fast regex search)"),
                ),
                BuiltinKind::AstGrep => (
                    "AST Grep",
                    "discoverable",
                    Some("Search code with AST patterns (structural grep)"),
                ),
                BuiltinKind::AstEdit => (
                    "AST Edit",
                    "discoverable",
                    Some("Perform AST-aware code edits (structural refactoring)"),
                ),
                BuiltinKind::Debug => (
                    "Debug",
                    "discoverable",
                    Some("Debug a running process with DAP (debugger adapter protocol)"),
                ),
                BuiltinKind::WebSearch => (
                    "Web Search",
                    "discoverable",
                    Some("Search the web for up-to-date information"),
                ),
                BuiltinKind::Edit => continue,
            };
            for (key, value) in [("label", label), ("loadMode", mode)] {
                heap.define_own_data(
                    &member.source,
                    key.into(),
                    heap.string(value.into())?,
                    runtime,
                )?
            }
            if let Some(summary) = summary {
                heap.define_own_data(
                    &member.source,
                    "summary".into(),
                    heap.string(summary.into())?,
                    runtime,
                )?
            }
            let query = if let Kernel::Write(tool) = &member.kernel {
                let capability = capability()?;
                let function = heap.function(capability)?;
                self.approval_paths
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(capability, (member.source.clone(), tool.ctx.clone()));
                function
            } else {
                heap.undefined()?
            };
            let approval = xdev_native::builtin_approval_function(
                heap,
                kind,
                &module.environment,
                &query,
                &member.source,
                bash_patterns,
            )?;
            heap.define_own_data(&member.source, "approval".into(), approval, runtime)?;
        }
        Ok(())
    }
    pub(crate) fn configure_bash_approval(
        &self,
        values: &SessionValues,
        patterns: &ValueHandle,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        let Some(module) = self.ast_module.get() else {
            return Ok(());
        };
        for member in &self.members {
            if member.kernel.kind() == BuiltinKind::Bash {
                let approval = xdev_native::builtin_approval_function(
                    values.heap(),
                    BuiltinKind::Bash,
                    &module.environment,
                    &values.heap().undefined()?,
                    &member.source,
                    patterns,
                )?;
                values.heap().define_own_data(
                    &member.source,
                    "approval".into(),
                    approval,
                    values.runtime(),
                )?;
            }
        }
        Ok(())
    }
    fn ast_module(
        &self,
    ) -> Result<&Arc<crate::native_realm::ast::NativeAstModule>, ActualProviderError> {
        self.ast_module.get().ok_or(ActualProviderError::Protocol(
            "Original AST module imports absent",
        ))
    }
    /// Same logical Session rebuilds its catalog with the same managed jobs.
    /// A new/reset Session supplies a new owner after settling the old one.
    pub fn new_with_bash_session(
        values: &Arc<SessionValues>,
        context: ToolContext,
        kinds: &[BuiltinKind],
        bash_session: Arc<bash_native::SessionBashOwner>,
    ) -> Result<Arc<Self>, ActualProviderError> {
        bash_session.bind_values(values)?;
        let wait_session=bash_session.wait_session();
        wait_session.bind_values(values)?;
        let heap = values.heap();
        let mut prepared = Vec::new();
        let mut write_descriptions = HashMap::new();
        let mut edit_descriptions = HashMap::new();
        let mut task_metadata = HashMap::new();
        let task_session=task_native::SessionTaskOwner::new(values);
        for kind in kinds {
            if prepared.iter().any(
                |(kernel, _, _, _): &(Kernel, ValueHandle, ValueHandle, CapabilityId)| {
                    kernel.kind() == *kind
                },
            ) {
                return Err(ActualProviderError::Protocol(
                    "duplicate concrete builtin capability",
                ));
            }
            let kernel = match kind {
                BuiltinKind::Read => Kernel::Read(Arc::new(ReadTool {
                    ctx: context.clone(),
                })),
                BuiltinKind::Write => Kernel::Write(Arc::new(WriteTool::new(context.clone()))),
                BuiltinKind::Bash => Kernel::Bash(Arc::new(BashTool {
                    ctx: context.clone(),
                })),
                BuiltinKind::Edit => Kernel::Edit(Arc::new(EditTool::new(context.clone()))),
                BuiltinKind::Grep => Kernel::Grep(Arc::new(GrepTool::new(context.clone()))),
                BuiltinKind::Glob => Kernel::Glob(Arc::new(GlobTool::new(context.clone()))),
                BuiltinKind::AstGrep => Kernel::AstGrep(Arc::new(AstGrepTool {
                    ctx: context.clone(),
                })),
                BuiltinKind::AstEdit => Kernel::AstEdit(Arc::new(AstEditTool {
                    ctx: context.clone(),
                })),
                BuiltinKind::Debug => Kernel::Debug(Arc::new(DebugTool)),
                BuiltinKind::WebSearch => Kernel::WebSearch(Arc::new(WebSearchTool)),
                BuiltinKind::Wait=>Kernel::Wait(Arc::new(wait_native::WaitTool)),
                BuiltinKind::Task=>Kernel::Task(Arc::new(task_native::TaskTool)),
            };
            let mut definition = kernel.tool().protocol_definition();
            if matches!(kind, BuiltinKind::Bash) {
                let properties = definition.parameters["properties"].as_object_mut().ok_or(
                    ActualProviderError::Protocol("Bash native schema properties"),
                )?;
                properties.insert(
                    "pty".into(),
                    serde_json::json!({"type":"boolean","description":"run in pty mode"}),
                );
                if bash_session.settings().async_enabled {
                    properties.insert(
                        "async".into(),
                        serde_json::json!({"type":"boolean","description":"run in background"}),
                    );
                }
            }
            let source = heap.object()?;
            let method = capability()?;
            let execute = heap.function(method)?;
            // Host-authored definition data, not original argument/history export.
            for (key, value) in [
                ("name", heap.string(definition.name.into())?),
                ("description", heap.string(definition.description.into())?),
                ("parameters", heap.import_serde(&definition.parameters)?),
                ("execute", execute.clone()),
                (
                    "concurrency",
                    if matches!(kind, BuiltinKind::Bash) {
                        bash_native::concurrency(heap)?
                    } else {
                        heap.string(
                            if matches!(
                                kind,
                                BuiltinKind::Write | BuiltinKind::Edit | BuiltinKind::Debug
                            ) {
                                "exclusive"
                            } else {
                                "shared"
                            }
                            .into(),
                        )?
                    },
                ),
            ] {
                heap.define_own_data(&source, key.into(), value, values.runtime())?;
            }
            if matches!(kind,BuiltinKind::Wait){
                heap.define_own_data(&source,"interruptible".into(),heap.boolean(true)?,values.runtime())?;
                heap.define_own_data(&source,"intent".into(),heap.string("optional".into())?,values.runtime())?;
                heap.define_own_data(&source,"loadMode".into(),heap.string("essential".into())?,values.runtime())?;
                heap.define_own_data(&source,"approval".into(),heap.string("read".into())?,values.runtime())?;
            }
            if matches!(kind,BuiltinKind::Task){
                heap.define_own_data(&source,"lenientArgValidation".into(),heap.boolean(true)?,values.runtime())?;
                heap.define_own_data(&source,"approval".into(),heap.string("exec".into())?,values.runtime())?;
                heap.define_own_data(&source,"loadMode".into(),heap.string("essential".into())?,values.runtime())?;
                for property in ["parameters","description"] {
                    let getter=capability()?;
                    let ValueSnapshot::Function(target)=heap.snapshot(&heap.function(getter)?)?else{return Err(ActualProviderError::Protocol("Task metadata getter must be callable"));};
                    heap.define_own(&source,property.into(),PropertyDescriptor::Accessor{get:Some(target),set:None,enumerable:true,configurable:true},values.runtime())?;
                    task_metadata.insert(getter,(source.clone(),property));
                }
            }
            if let Kernel::Write(tool) = &kernel {
                let getter = capability()?;
                let ValueSnapshot::Function(target) = heap.snapshot(&heap.function(getter)?)?
                else {
                    return Err(ActualProviderError::Protocol(
                        "Write description getter must be callable",
                    ));
                };
                heap.define_own(
                    &source,
                    "description".into(),
                    PropertyDescriptor::Accessor {
                        get: Some(target),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    values.runtime(),
                )?;
                write_descriptions.insert(getter, (source.clone(), tool.clone()));
            }
            if let Kernel::Edit(tool) = &kernel {
                let getter = capability()?;
                let ValueSnapshot::Function(target) = heap.snapshot(&heap.function(getter)?)?
                else {
                    return Err(ActualProviderError::Protocol(
                        "Edit description getter must be callable",
                    ));
                };
                heap.define_own(
                    &source,
                    "description".into(),
                    PropertyDescriptor::Accessor {
                        get: Some(target),
                        set: None,
                        enumerable: true,
                        configurable: true,
                    },
                    values.runtime(),
                )?;
                edit_descriptions.insert(getter, (source.clone(), tool.clone(), "description"));
            }
            // Native provider declarations and alias lookup read the canonical
            // Graph source, independently of BoundTool's Rust metadata methods.
            if let Some(format) = definition.custom_format {
                let format = serde_json::to_value(format)
                    .map_err(|_| ActualProviderError::Protocol("derived builtin custom format"))?;
                heap.define_own_data(
                    &source,
                    "customFormat".into(),
                    heap.import_serde(&format)?,
                    values.runtime(),
                )?;
            }
            if let Some(name) = definition.custom_wire_name {
                heap.define_own_data(
                    &source,
                    "customWireName".into(),
                    heap.string(name.into())?,
                    values.runtime(),
                )?;
            }
            if let Some(strict) = definition.strict {
                heap.define_own_data(
                    &source,
                    "strict".into(),
                    heap.boolean(strict)?,
                    values.runtime(),
                )?;
            }
            if let Kernel::AstEdit(_) = &kernel {
                ast_edit_input::install(heap, &source, values.runtime())?;
            }
            {
                if let Some(examples) = &definition.examples {
                    let examples = serde_json::to_value(examples)
                        .map_err(|_| ActualProviderError::Protocol("AST authored examples"))?;
                    heap.define_own_data(
                        &source,
                        "examples".into(),
                        heap.import_serde(&examples)?,
                        values.runtime(),
                    )?;
                }
            }
            if let Kernel::Edit(tool) = &kernel {
                for property in ["mode","parameters","examples","customFormat","customWireName"] {
                    let getter=capability()?;
                    let ValueSnapshot::Function(target)=heap.snapshot(&heap.function(getter)?)?else{return Err(ActualProviderError::Protocol("Edit metadata getter must be callable"));};
                    heap.define_own(&source,property.into(),PropertyDescriptor::Accessor{get:Some(target),set:None,enumerable:true,configurable:true},values.runtime())?;
                    edit_descriptions.insert(getter,(source.clone(),tool.clone(),property));
                }
                edit_inspection::install(heap, &source, tool.ctx.edit_mode(), values.runtime())?;
            }
            prepared.push((kernel, source, execute, method));
        }
        let collection = heap.array(
            prepared
                .iter()
                .map(|(_, source, _, _)| source.clone())
                .collect(),
        )?;
        let grants = Arc::new(Mutex::new(
            kinds
                .iter()
                .map(|_| Grant {
                    enabled: true,
                    entered: HashMap::new(),
                })
                .collect(),
        ));
        Ok(Arc::new_cyclic(|owner| Self {
            values: Arc::downgrade(values),
            ast_module: OnceLock::new(),
            device_snapshot: Mutex::new(None),
            approval_paths: Mutex::new(HashMap::new()),
            write_descriptions,
            edit_descriptions,
            task_session:Mutex::new(task_session),
            task_metadata,
            collection,
            grants,
            context_signals: Mutex::new(ContextSignals::default()),
            native_run: Mutex::new(None),
            read_session: Mutex::new(None),
            bash_session,
            wait_session,
            members: prepared
                .into_iter()
                .enumerate()
                .map(|(index, (kernel, source, execute, method))| {
                    let invoker = Arc::new(BuiltinInvoker {
                        owner: owner.clone(),
                        index,
                    });
                    let capability = Arc::new(BoundTool {
                        kernel: kernel.clone(),
                        invoker,
                    });
                    Member {
                        kernel,
                        source,
                        execute,
                        method,
                        capability,
                    }
                })
                .collect(),
        }))
    }

    /// Stable Session job owner shared by the complete catalog and host lifecycle.
    // Observes exactly the original route-approval callback operand, rather
    // than introducing another Get(path) before or after a held approval.
    pub(super) fn capture_write_approval_query(&self,request:&RuntimeRequest<'_>)->Result<Option<(ara_tools::ContentUriWriteReceipt,RuntimeReply)>,ValueError> {
        let RuntimeRequest::Call{capability,receiver,arguments}=request else{return Ok(None)};
        let selected=self.approval_paths.lock().unwrap_or_else(|e|e.into_inner()).get(capability).cloned();
        let Some((source,context))=selected else{return Ok(None)};
        let values=self.values.upgrade().ok_or(ValueError::ExpiredCapability(*capability))?;
        values.check_handle(receiver)?;
        if !source.same_identity(receiver)||arguments.len()!=1{return Err(ValueError::TypeError("Wrong original Write route approval receiver/arguments".into()));}
        let ValueSnapshot::String(path)=values.heap().snapshot(&arguments[0])? else{return Err(ValueError::TypeError("Write route approval path must be a String".into()));};
        let path=path.to_utf8().map_err(|_|ValueError::UnsupportedOperation("Current Host URI route requires valid UTF-8"))?;
        let receipt=ara_tools::ContentUriWriteReceipt::capture(&context,&path);
        let write_tier=receipt.selection.as_ref().is_some_and(|selection|selection.route==ara_tools::ContentUriRoute::Registered{writable:true}&&selection.policy.tier!=ara_tools::ContentUriWriteTier::Read);
        Ok(Some((receipt,RuntimeReply::Value(values.heap().string(if write_tier{"write"}else{"read"}.into())?))))
    }
    pub fn task_session(&self)->Arc<task_native::SessionTaskOwner>{self.task_session.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    /// Catalog refresh retains the exact logical Session execution owner.
    pub fn retain_task_session(&self,owner:Arc<task_native::SessionTaskOwner>)->Result<(),ActualProviderError>{
        let values=self.values.upgrade().ok_or(ActualProviderError::Protocol("Task Session values expired"))?;
        owner.check_values(&values).map_err(|_|ActualProviderError::Protocol("Task refresh belongs to another logical Session"))?;
        *self.task_session.lock().unwrap_or_else(|e|e.into_inner())=owner;Ok(())
    }
    pub fn wait_session(&self)->Arc<wait_native::SessionWaitOwner>{self.wait_session.clone()}
    pub fn bash_session(&self) -> Arc<bash_native::SessionBashOwner> {
        self.bash_session.clone()
    }
    pub(crate) fn bind_device_snapshot(
        &self,
        snapshot: &Arc<NativeCatalogSnapshot>,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        if !snapshot.devices_enabled() {
            return Ok(());
        }
        let mut current = self
            .device_snapshot
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|old| !Arc::ptr_eq(&old, snapshot))
        {
            return Err(ActualProviderError::Protocol(
                "Device source is already bound to another retained catalog",
            ));
        }
        *current = Some(Arc::downgrade(snapshot));
        Ok(())
    }
    fn device_snapshot(
        &self,
        values: &SessionValues,
    ) -> Result<Option<Arc<NativeCatalogSnapshot>>, ActualProviderError> {
        self.check_values(values)?;
        let binding = self
            .device_snapshot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        match binding.as_ref() {
            Some(binding) => binding
                .upgrade()
                .map(Some)
                .ok_or(ActualProviderError::Protocol(
                    "Bound original catalog expired",
                )),
            None => Ok(None),
        }
    }
    pub(crate) fn invoke_device(
        self: Arc<Self>,
        binding: ActualToolBinding,
        request: NativeMethodRequest,
    ) -> BoxFuture<'static, Result<ValueHandle, ActualProviderError>> {
        Box::pin(async move {
            let index = self
                .members
                .iter()
                .position(|member| member.source.same_identity(&binding.source))
                .ok_or(ActualProviderError::Protocol(
                    "Device builtin has no exact canonical member",
                ))?;
            let original: Arc<dyn AgentTool> = self.members[index].capability.clone();
            if !Arc::ptr_eq(&original, &binding.capability) {
                return Err(ActualProviderError::Protocol(
                    "Device builtin canonical capability differs",
                ));
            }
            BuiltinInvoker {
                owner: Arc::downgrade(&self),
                index,
            }
            .invoke_method(request)
            .await
        })
    }

    fn bound_run(&self) -> Result<Option<Arc<NativeBuiltinRun>>, ActualProviderError> {
        let locator = self
            .native_run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match locator {
            None => Ok(None),
            Some(locator) => locator
                .upgrade()
                .map(Some)
                .ok_or(ActualProviderError::Protocol(
                    "Native builtin Run locator expired",
                )),
        }
    }

    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.members
            .iter()
            .map(|member| member.capability.clone() as Arc<dyn AgentTool>)
            .collect()
    }

    fn check_values(&self, values: &SessionValues) -> Result<(), ActualProviderError> {
        let retained = self.values.upgrade().ok_or(ActualProviderError::Protocol(
            "builtin Session values expired",
        ))?;
        if !std::ptr::eq(retained.as_ref(), values) {
            return Err(ActualProviderError::Protocol(
                "builtin capability requires the same logical Session values",
            ));
        }
        Ok(())
    }

    /// Permission for an admitted original source/capability pair. Name equality
    /// is never a grant; the invoker repeats its final gate after all Graph Gets.
    pub fn permission(
        &self,
        binding: &ActualToolBinding,
        values: &SessionValues,
    ) -> Result<ActualToolDecision, ActualProviderError> {
        self.check_values(values)?;
        values.check_handle(&binding.source)?;
        let index = self
            .members
            .iter()
            .position(|member| {
                let capability: Arc<dyn AgentTool> = member.capability.clone();
                member.source.same_identity(&binding.source)
                    && Arc::ptr_eq(&capability, &binding.capability)
            })
            .ok_or(ActualProviderError::Protocol(
                "actual permission source/capability owner differs",
            ))?;
        let enabled = self
            .grants
            .lock()
            .unwrap_or_else(|error| error.into_inner())[index]
            .enabled;
        Ok(if enabled {
            ActualToolDecision::Allow
        } else {
            ActualToolDecision::Block(Some(
                "Tool execution was blocked by the current Host grant".into(),
            ))
        })
    }

    fn context_signal(
        &self,
        context: &ActualToolExecutionContext,
        values: &SessionValues,
    ) -> Result<ValueHandle, ActualProviderError> {
        self.check_values(values)?;
        let key = (context.batch_id.clone(), context.tool_calls.node_id());
        {
            let signals = self
                .context_signals
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(signal) = signals
                .batches
                .get(&key)
                .and_then(|batches| batches.iter().find_map(|batch| batch.for_context(context)))
            {
                return Ok(signal);
            }
        }
        // Allocate outside the owner mutex. Concurrent contexts may prepare a
        // candidate, but only one same-batch signal is published below.
        let id = capability()?;
        let signal = values.heap().opaque(OpaqueKind::Object, id, id.0)?;
        let mut signals = self
            .context_signals
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(signal) = signals
            .batches
            .get(&key)
            .and_then(|batches| batches.iter().find_map(|batch| batch.for_context(context)))
        {
            return Ok(signal);
        }
        // batchId is display attribution and may repeat. Original toolCalls and
        // the fresh controller token distinguish separate execute batches.
        signals
            .batches
            .entry(key)
            .or_default()
            .push(BatchContextSignal {
                tool_calls: context.tool_calls.clone(),
                signal: signal.clone(),
                token: context.steering_signal.clone(),
            });
        signals
            .capabilities
            .insert(id, (signal.clone(), context.steering_signal.clone()));
        Ok(signal)
    }

    /// Compose this same source owner into the Host's current realm resolver.
    /// The invoker also delegates here while an original tool method is active.
    /// Unknown capabilities belong to the Host; unsupported AbortSignal members
    /// are explicit failures, never invented event/listener behavior.
    pub fn resolve_context_signal(
        &self,
        request: &RuntimeRequest<'_>,
    ) -> Option<Result<RuntimeReply, ValueError>> {
        if let RuntimeRequest::Call {
            capability,
            receiver,
            arguments,
        } = request
        {
            if let Some((source,property))=self.task_metadata.get(capability){
                return Some((||{
                    let values=self.values.upgrade().ok_or(ValueError::ExpiredCapability(*capability))?;
                    values.check_handle(receiver)?;
                    if !source.same_identity(receiver)||!arguments.is_empty(){return Err(ValueError::TypeError("Wrong original Task metadata receiver/arguments".into()));}
                    let definition=self.task_session().definition();
                    let value=if *property=="parameters"{values.heap().import_serde(&definition.parameters)?}else{values.heap().string(definition.description.into())?};
                    Ok(RuntimeReply::Value(value))
                })());
            }
            if let Some((source, tool)) = self.write_descriptions.get(capability) {
                return Some((|| {
                    let values = self
                        .values
                        .upgrade()
                        .ok_or(ValueError::ExpiredCapability(*capability))?;
                    values.check_handle(receiver)?;
                    if !source.same_identity(receiver) || !arguments.is_empty() {
                        return Err(ValueError::TypeError(
                            "Wrong original Write description receiver/arguments".into(),
                        ));
                    }
                    Ok(RuntimeReply::Value(
                        values.heap().string(tool.definition().description.into())?,
                    ))
                })());
            }
            if let Some((source, tool, property)) = self.edit_descriptions.get(capability) {
                return Some((|| {
                    let values = self
                        .values
                        .upgrade()
                        .ok_or(ValueError::ExpiredCapability(*capability))?;
                    values.check_handle(receiver)?;
                    if !source.same_identity(receiver) || !arguments.is_empty() {
                        return Err(ValueError::TypeError(
                            "Wrong original Edit description receiver/arguments".into(),
                        ));
                    }
                    let heap=values.heap();
                    let value=if *property=="description" {heap.string(tool.description().map_err(|error|ValueError::TypeError(error.0))?.into())?}
                    else {let mode=tool.live_mode().map_err(|error|ValueError::TypeError(error.0))?;match *property {
                        "mode"=>heap.string(mode.as_str().into())?,
                        "parameters"=>heap.import_serde(&ara_tools::edit::parameters(mode))?,
                        "examples"=>match ara_tools::edit::examples_for_mode(mode){Some(examples)=>heap.import_serde(&serde_json::to_value(examples).map_err(|_|ValueError::WrongRuntimeReply("Edit examples serialization"))?)?,None=>heap.undefined()?},
                        "customFormat"=>match ara_edit::grammar(mode){Some(definition)=>heap.import_serde(&serde_json::json!({"syntax":"lark","definition":definition}))?,None=>heap.undefined()?},
                        "customWireName"=>if mode==ara_edit::EditMode::ApplyPatch{heap.string("apply_patch".into())?}else{heap.undefined()?},
                        _=>return Err(ValueError::WrongRuntimeReply("Edit metadata property"))
                    }};
                    Ok(RuntimeReply::Value(value))
                })());
            }
            let selected = self
                .approval_paths
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(capability)
                .cloned();
            if let Some((source, context)) = selected {
                return Some((|| {
                    let values = self
                        .values
                        .upgrade()
                        .ok_or(ValueError::ExpiredCapability(*capability))?;
                    values.check_handle(receiver)?;
                    if !source.same_identity(receiver) || arguments.len() != 1 {
                        return Err(ValueError::TypeError(
                            "Wrong original Write route approval receiver/arguments".into(),
                        ));
                    }
                    let ValueSnapshot::String(path) = values.heap().snapshot(&arguments[0])? else {
                        return Err(ValueError::TypeError(
                            "Write route approval path must be a String".into(),
                        ));
                    };
                    let path = path.to_utf8().map_err(|_| {
                        ValueError::UnsupportedOperation(
                            "Current Host URI route requires valid UTF-8",
                        )
                    })?;
                    let write_tier = context.content_uri_port().is_some_and(|port| {
                        matches!(port.route(&path), ara_tools::ContentUriRoute::Registered { writable: true })
                            && port.write_policy(&path).tier != ara_tools::ContentUriWriteTier::Read
                    });
                    Ok(RuntimeReply::Value(
                        values
                            .heap()
                            .string(if write_tier { "write" } else { "read" }.into())?,
                    ))
                })());
            }
        }
        let RuntimeRequest::Opaque {
            capability,
            target,
            receiver,
            operation,
        } = request
        else {
            return None;
        };
        let (signal, token) = self
            .context_signals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .capabilities
            .get(capability)
            .cloned()?;
        if self.values.upgrade().is_none() {
            return Some(Err(ValueError::ExpiredCapability(*capability)));
        }
        if !target.same_identity(&signal) || !receiver.same_identity(&signal) {
            return Some(Err(ValueError::TypeError(
                "Wrong original tool-context AbortSignal receiver".into(),
            )));
        }
        Some(match operation {
            OpaqueOperation::Get(PropertyKey::String(name)) if name.equals_ascii("aborted") => {
                signal
                    .heap()
                    .boolean(token.is_cancelled())
                    .map(RuntimeReply::Value)
            }
            _ => Err(ValueError::UnsupportedOperation(
                "actual tool-context AbortSignal member is not connected",
            )),
        })
    }

    /// Before admission, revocation denies entry. After admission it cancels the
    /// already entered operation; its original owner still settles that effect.
    pub fn set_granted(&self, kind: BuiltinKind, enabled: bool) -> Result<(), ActualProviderError> {
        let index = self
            .members
            .iter()
            .position(|member| member.kernel.kind() == kind)
            .ok_or(ActualProviderError::Protocol(
                "builtin is absent from this Session",
            ))?;
        let tokens = {
            let mut grants = self
                .grants
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            grants[index].enabled = enabled;
            if enabled {
                Vec::new()
            } else {
                grants[index].entered.values().cloned().collect()
            }
        };
        for token in tokens {
            token.cancel();
        }
        Ok(())
    }

    fn check_read_part_source(
        &self,
        index: usize,
        request: &NativeMethodRequest,
    ) -> Result<(), ActualProviderError> {
        self.check_values(&request.values)?;
        request.check_admission()?;
        request.check_read_parent_grant(self, index)?;
        let member = &self.members[index];
        if !request.is_read_part()
            || !matches!(&member.kernel, Kernel::Read(_))
            || !member.source.same_identity(&request.receiver)
        {
            return Err(ActualProviderError::Protocol(
                "Private READ supplier lost canonical source association",
            ));
        }
        Ok(())
    }
    fn check_exact(
        &self,
        index: usize,
        request: &NativeMethodRequest,
    ) -> Result<(), ActualProviderError> {
        self.check_values(&request.values)?;
        request.check_admission()?;
        let member = &self.members[index];
        if !member.source.same_identity(&request.receiver)
            || !member.execute.same_identity(&request.callable)
        {
            return Err(ActualProviderError::Protocol(
                "builtin execute callable/receiver capability association changed",
            ));
        }
        Ok(())
    }

    fn enter(
        self: &Arc<Self>,
        index: usize,
        id: CapabilityId,
        request: &NativeMethodRequest,
        cancel: CancellationToken,
    ) -> Result<EffectPermit, ActualProviderError> {
        self.check_exact(index, request)?;
        self.enter_granted(index, id, cancel)
    }

    /// Request admission must be checked before taking the Native Run state
    /// lock. This private helper takes only native grant operands, allowing the
    /// Run to retain its final state lock through the atomic source grant.
    fn enter_granted(
        self: &Arc<Self>,
        index: usize,
        id: CapabilityId,
        cancel: CancellationToken,
    ) -> Result<EffectPermit, ActualProviderError> {
        // No Get, coercion, Host callback or await under the grant mutex.
        let mut grants = self
            .grants
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !grants[index].enabled {
            return Err(ActualProviderError::Callback(
                "Tool execution was blocked by the current Host grant".into(),
            ));
        }
        if grants[index].entered.contains_key(&id) {
            return Err(ActualProviderError::Protocol(
                "duplicate builtin effect admission",
            ));
        }
        grants[index].entered.insert(id, cancel);
        Ok(EffectPermit {
            grants: self.grants.clone(),
            index,
            id,
        })
    }
}

/// CLI's genuinely absent extension callbacks are represented by None/identity
/// in CliHooks. This owner supplies the native contracts that are not optional:
/// exact permission, fresh context attribution, sanitizer and thrown formatting.
/// It does not enable a factory or manufacture bindings for other advertised tools.
pub struct NativeBuiltinToolHooks {
    source: Arc<BuiltinToolSource>,
    catalog: OnceLock<Arc<NativeToolCatalog>>,
    error_constructor: Option<ValueHandle>,
    string_function: Option<ValueHandle>,
    sanitizer: Option<ActualToolResultSanitizer>,
}
impl NativeBuiltinToolHooks {
    pub fn new(
        source: Arc<BuiltinToolSource>,
        error_constructor: Option<ValueHandle>,
        string_function: Option<ValueHandle>,
        sanitizer: Option<ActualToolResultSanitizer>,
        values: &SessionValues,
    ) -> Result<Arc<Self>, ActualProviderError> {
        source.check_values(values)?;
        for function in error_constructor.iter().chain(string_function.iter()) {
            values.check_handle(function)?;
            if values.heap().kind(function)? != ara_rpc::value_heap::ValueKind::Function {
                return Err(ValueError::TypeError(
                    "Native Error/String binding must be callable".into(),
                )
                .into());
            }
        }
        if let Some(sanitizer) = &sanitizer {
            values.check_handle(sanitizer.function())?;
        }
        Ok(Arc::new(Self {
            source,
            catalog: OnceLock::new(),
            error_constructor,
            string_function,
            sanitizer,
        }))
    }
    pub fn check_values(&self, values: &SessionValues) -> Result<(), ActualProviderError> {
        self.source.check_values(values)?;
        if let Some(catalog) = self.catalog.get() {
            catalog.check_values(values)?;
        }
        Ok(())
    }
    pub(crate) fn install_catalog(
        &self,
        catalog: Arc<NativeToolCatalog>,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        let snapshot = catalog.snapshot(values)?;
        if !Arc::ptr_eq(snapshot.builtin(), &self.source) {
            return Err(ActualProviderError::Protocol(
                "Native common hooks have another initial builtin owner",
            ));
        }
        if let Some(original) = self.catalog.get() {
            return if Arc::ptr_eq(original, &catalog) {
                Ok(())
            } else {
                Err(ActualProviderError::Protocol(
                    "Native hooks catalog cannot be replaced",
                ))
            };
        }
        self.catalog.set(catalog).map_err(|_| {
            ActualProviderError::Protocol("Concurrent Native hooks catalog installation")
        })
    }
    fn execution_source(
        &self,
        values: &SessionValues,
    ) -> Result<Arc<BuiltinToolSource>, ActualProviderError> {
        self.check_values(values)?;
        match self.catalog.get() {
            Some(catalog) => Ok(catalog.execution_snapshot(values)?.builtin().clone()),
            None => Ok(self.source.clone()),
        }
    }
    pub fn source(
        &self,
        values: &SessionValues,
    ) -> Result<Arc<dyn ActualToolSource>, ActualProviderError> {
        self.check_values(values)?;
        let source: Arc<dyn ActualToolSource> = match self.catalog.get() {
            Some(catalog) => catalog.clone(),
            None => self.source.clone(),
        };
        Ok(source)
    }
    pub fn permission(
        &self,
        binding: &ActualToolBinding,
        values: &SessionValues,
    ) -> Result<ActualToolDecision, ActualProviderError> {
        self.check_values(values)?;
        match self.catalog.get() {
            Some(catalog) => catalog.permission(binding, values),
            None => self.source.permission(binding, values),
        }
    }
    /// Only Host preparation binds this original Read field. No per-Run Session
    /// graph is manufactured, and an existing binding cannot be replaced.
    /// Capture original Agent membership without inspecting roles/content or IO.
    pub fn capture_read_attachments(
        &self,
        messages: &[ara_agent::AgentMessage],
        blobs: std::path::PathBuf,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        if let Some(run) = self.execution_source(values)?.bound_run()? {
            run.capture_read_history(messages, blobs, values)?;
        }
        Ok(())
    }
    pub fn bind_read_session(
        &self,
        session: &ValueHandle,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        Self::bind_source_read_session(&self.source, session, values)
    }
    pub(crate) fn prepare_catalog_snapshot(
        &self,
        snapshot: &NativeCatalogSnapshot,
        session: &ValueHandle,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        snapshot.check_values(values)?;
        if let Some(module) = self.source.ast_module.get() {
            snapshot.builtin().bind_ast_module(module.clone(), values)?;
        }
        Self::bind_source_read_session(snapshot.builtin(), session, values)
    }
    fn bind_source_read_session(
        source: &BuiltinToolSource,
        session: &ValueHandle,
        values: &SessionValues,
    ) -> Result<(), ActualProviderError> {
        source.check_values(values)?;
        values.check_handle(session)?;
        let Some(member) = source
            .members
            .iter()
            .find(|member| member.kernel.kind() == BuiltinKind::Read)
        else {
            return Ok(());
        };
        let heap = values.heap();
        let edge = heap.capture_deferred(session)?;
        let mut bound = source
            .read_session
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = *bound {
            return if existing == edge {
                Ok(())
            } else {
                Err(ActualProviderError::Protocol(
                    "Original Read Session binding cannot be replaced",
                ))
            };
        }
        if heap
            .own_descriptor(&member.source, &"session".into(), values.runtime())?
            .is_some()
        {
            return Err(ActualProviderError::Protocol(
                "Original Read source session already exists before Host binding",
            ));
        }
        // This source is an original ordinary Node::Object, so defining a data
        // field cannot invoke an opaque Host or accessor callback under this lock.
        heap.define_own_data(
            &member.source,
            "session".into(),
            session.clone(),
            values.runtime(),
        )?;
        *bound = Some(edge);
        Ok(())
    }
    pub fn bind_run(&self, run: &Arc<NativeBuiltinRun>) -> Result<(), ActualProviderError> {
        Self::bind_source_run(&self.source, run)
    }
    pub(crate) fn bind_source_run(
        source: &BuiltinToolSource,
        run: &Arc<NativeBuiltinRun>,
    ) -> Result<(), ActualProviderError> {
        let values = source
            .values
            .upgrade()
            .ok_or(ActualProviderError::Protocol(
                "Native builtin Session expired",
            ))?;
        run.check_values(&values)?;
        run.check_live()?;
        let mut locator = source.native_run.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = locator.as_ref().and_then(Weak::upgrade) {
            if Arc::ptr_eq(&old, run) {
                return Ok(());
            }
            if old.check_live().is_ok() {
                return Err(ActualProviderError::Protocol(
                    "Native builtin already has a live Run",
                ));
            }
        }
        *locator = Some(Arc::downgrade(run));
        Ok(())
    }
    /// RPC calls this only after delegating original fresh context creation.
    pub fn bind_run_context(
        &self,
        values: &SessionValues,
        context: &ValueHandle,
    ) -> Result<(), ActualProviderError> {
        self.check_values(values)?;
        self.execution_source(values)?
            .bound_run()?
            .ok_or(ActualProviderError::Protocol(
                "Native builtin Run is not installed",
            ))?
            .bind_context(values, context)
    }
    pub fn tool_context(
        &self,
        context: &ActualToolExecutionContext,
        values: &SessionValues,
    ) -> Result<ValueHandle, ActualProviderError> {
        self.check_values(values)?;
        values.check_handle(&context.tool_calls)?;
        values.check_handle(&context.provider_metadata)?;
        let index = u32::try_from(context.index)
            .map_err(|_| ActualProviderError::Protocol("Native tool index"))?;
        let total = u32::try_from(context.total)
            .map_err(|_| ActualProviderError::Protocol("Native tool count"))?;
        let heap = values.heap();
        let call = heap.object()?;
        for (key, value) in [
            ("batchId", heap.string(context.batch_id.clone())?),
            ("index", heap.number(f64::from(index))?),
            ("total", heap.number(f64::from(total))?),
            ("toolCalls", context.tool_calls.clone()),
            (
                "steeringSignal",
                self.execution_source(values)?
                    .context_signal(context, values)?,
            ),
            ("providerMetadata", context.provider_metadata.clone()),
        ] {
            heap.define_own_data(&call, key.into(), value, values.runtime())?;
        }
        let result = heap.object()?;
        heap.define_own_data(&result, "toolCall".into(), call, values.runtime())?;
        Ok(result)
    }
    pub fn error_message(
        &self,
        error: &ValueError,
        values: &SessionValues,
    ) -> Result<ValueHandle, ActualProviderError> {
        self.check_values(values)?;
        // Native engine errors currently have a Rust diagnostic, not their
        // original realm Error object. An Error/String binding cannot restore
        // that lost constructor identity; refuse this domain rather than debug it.
        let ValueError::Thrown(thrown) = error else {
            return Err(ActualProviderError::Unsupported(
                "original Native engine Error constructor binding",
            ));
        };
        values.check_handle(thrown)?;
        let constructor =
            self.error_constructor
                .as_ref()
                .ok_or(ActualProviderError::Unsupported(
                    "original Native Error constructor is not connected",
                ))?;
        let heap = values.heap();
        if let Some(module) = self.source.ast_module.get() {
            if heap.instance_of(thrown, &module.tool_error, values.runtime())? {
                let render = heap.get(thrown, &"render".into(), values.runtime())?;
                return Ok(heap.call(&render, thrown, &[], values.runtime())?);
            }
        }
        if heap.instance_of(thrown, constructor, values.runtime())? {
            Ok(heap.get(thrown, &"message".into(), values.runtime())?)
        } else {
            let string = self
                .string_function
                .as_ref()
                .ok_or(ActualProviderError::Unsupported(
                    "original Native String Function is not connected",
                ))?;
            Ok(heap.call(
                string,
                &heap.undefined()?,
                std::slice::from_ref(thrown),
                values.runtime(),
            )?)
        }
    }
    pub fn sanitize(
        &self,
        text: &ValueHandle,
        values: &SessionValues,
    ) -> Result<ValueHandle, ActualProviderError> {
        self.check_values(values)?;
        self.sanitizer
            .as_ref()
            .ok_or(ActualProviderError::Unsupported(
                "fixed Native sanitizeText is not connected",
            ))?
            .sanitize(text, values)
    }
    pub fn soft_satisfies(
        &self,
        requirement: &ara_agent::soft_tool_requirement::SoftToolRequirement,
        call: &ValueHandle,
        values: &SessionValues,
    ) -> Result<bool, ActualProviderError> {
        self.check_values(values)?;
        values.check_handle(call)?;
        if requirement.id.starts_with("pending-action:") {
            return ast_edit_input::satisfies(values, call);
        }
        if requirement.satisfies.is_some() {
            return Err(ActualProviderError::Unsupported(
                "typed custom soft predicate has no Native binding",
            ));
        }
        let name = values.heap().get(call, &"name".into(), values.runtime())?;
        Ok(values.heap().object_is(
            &name,
            &values
                .heap()
                .string(requirement.tool_name.as_str().into())?,
        )?)
    }
}
impl ActualToolSource for BuiltinToolSource {
    fn current_tools(
        &self,
        values: &SessionValues,
    ) -> Result<Option<ValueHandle>, ActualProviderError> {
        self.check_values(values)?;
        Ok(Some(self.collection.clone()))
    }
    fn bind_selected(
        &self,
        selected: &ValueHandle,
        values: &SessionValues,
    ) -> Result<ActualToolBinding, ActualProviderError> {
        self.check_values(values)?;
        values.check_handle(selected)?;
        let member = self
            .members
            .iter()
            .find(|member| member.source.same_identity(selected))
            .ok_or(ActualProviderError::Protocol(
                "selected original tool has no concrete builtin capability",
            ))?;
        Ok(ActualToolBinding {
            source: member.source.clone(),
            capability: member.capability.clone(),
        })
    }
    fn has_fallback(&self) -> bool {
        false
    }
}
struct EffectPermit {
    grants: Arc<Mutex<Vec<Grant>>>,
    index: usize,
    id: CapabilityId,
}
impl Drop for EffectPermit {
    fn drop(&mut self) {
        self.grants
            .lock()
            .unwrap_or_else(|error| error.into_inner())[self.index]
            .entered
            .remove(&self.id);
    }
}

struct BoundTool {
    kernel: Kernel,
    invoker: Arc<BuiltinInvoker>,
}
#[async_trait]
impl AgentTool for BoundTool {
    fn lenient_argument_validation(&self)->bool{self.kernel.tool().lenient_argument_validation()}
    fn interruptible(&self,args:&JsonObject)->bool{matches!(&self.kernel,Kernel::Wait(_))&&self.kernel.tool().interruptible(args)}
    fn intent_mode(&self)->ara_agent::ToolIntentMode{if matches!(&self.kernel,Kernel::Wait(_)){ara_agent::ToolIntentMode::Optional}else{ara_agent::ToolIntentMode::Require}}
    fn definition(&self) -> Tool {
        self.kernel.tool().definition()
    }
    fn custom_format(&self) -> Option<ara_ai::CustomToolFormat> {
        self.kernel.tool().custom_format()
    }
    fn custom_wire_name(&self) -> Option<String> {
        self.kernel.tool().custom_wire_name()
    }
    fn open_arg_stream(
        &self,
        init: ara_agent::ToolArgStreamInit,
    ) -> Result<Option<Box<dyn ara_agent::ToolArgStream>>, ToolError> {
        self.kernel.tool().open_arg_stream(init)
    }
    fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
        Some(self.invoker.clone())
    }
    fn concurrency(&self, args: &JsonObject) -> Concurrency {
        self.kernel.tool().concurrency(args)
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.kernel.tool().execute(id, args, cancel, update).await
    }
}
struct BuiltinInvoker {
    owner: Weak<BuiltinToolSource>,
    index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    AwaitingCall,
    Preparing,
    Running,
    Settled,
}
struct InvocationState {
    phase: Phase,
    future: Option<LegacyEffect>,
    native: Option<NativeInvocationState>,
    device: Option<xdev_native::DevicePrepared>,
    call_failure: Option<ActualProviderError>,
    update_failure: Option<ActualProviderError>,
    bash: Option<bash_native::BashPreparation>,
    ast: Option<ast_edit_native::AstPreparation>,
    ast_publication_open: bool,
}
struct InvocationRuntime {
    owner: Arc<BuiltinToolSource>,
    index: usize,
    request: Arc<NativeMethodRequest>,
    arguments: Vec<ValueHandle>,
    update: CapabilityId,
    signal: CapabilityId,
    cancel: CancellationToken,
    marker: ValueHandle,
    state: Mutex<InvocationState>,
    run: Option<Weak<NativeBuiltinRun>>,
    parent: OnceLock<NativeParentId>,
    completion: CapabilityId,
    completion_port: ValueHandle,
    read_history: Option<read_attachments::CapturedHistory>,
}

/// The invocation future owns this weak, scalar-only exit path. Parent and
/// pending Reaction leases can retain the runtime after the waiter is dropped.
struct AstPublicationWaiterExit {
    runtime: Weak<InvocationRuntime>,
    armed: bool,
}
impl Drop for AstPublicationWaiterExit {
    fn drop(&mut self) {
        if self.armed {
            if let Some(runtime) = self.runtime.upgrade() {
                runtime.close_ast_publication_admission();
            }
        }
    }
}

#[async_trait]
impl ActualToolInvoker for BuiltinInvoker {
    async fn invoke_settled(
        &self,
        request: ActualExecuteRequest,
    ) -> Result<ValueHandle, ActualProviderError> {
        self.invoke_method(NativeMethodRequest::root(request)).await
    }
}
impl BuiltinInvoker {
    /// Only the sealed READ child supplier can enter a freshly evaluated
    /// this.execute method. Root/device callers keep check_exact unchanged.
    async fn invoke_read_part(
        &self,
        request: NativeMethodRequest,
    ) -> Result<ValueHandle, ActualProviderError> {
        let owner = self.owner.upgrade().ok_or(ActualProviderError::Protocol(
            "Original READ supplier owner expired",
        ))?;
        owner.check_read_part_source(self.index, &request)?;
        if owner.members[self.index]
            .execute
            .same_identity(&request.callable)
        {
            return self.invoke_method(request).await;
        }
        let run = owner.bound_run()?.ok_or(ActualProviderError::Protocol(
            "Original READ supplier requires Native Run",
        ))?;
        self.invoke_native(owner, run, request).await
    }
    async fn invoke_method(
        &self,
        request: NativeMethodRequest,
    ) -> Result<ValueHandle, ActualProviderError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or(ActualProviderError::Protocol("builtin source expired"))?;
        owner.check_exact(self.index, &request)?;
        match owner.bound_run()? {
            Some(run) => self.invoke_native(owner, run, request).await,
            None => self.invoke_legacy(request).await,
        }
    }
}
impl BuiltinInvoker {
    /// Existing BoundaryRuntime fixtures have no Native Run. This explicit
    /// legacy entrance uses no graph-owning worker and is not Native acceptance.
    async fn invoke_legacy(
        &self,
        request: NativeMethodRequest,
    ) -> Result<ValueHandle, ActualProviderError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or(ActualProviderError::Protocol("builtin source expired"))?;
        owner.check_exact(self.index, &request)?;
        let request = Arc::new(request);
        let heap = request.values.heap();
        let update = capability()?;
        let signal = capability()?;
        let cancel = request.cancel.child_token();
        let arguments = vec![
            heap.string(request.call_id.clone())?,
            request.args.clone(),
            request.signal().cloned().unwrap_or(heap.opaque(
                OpaqueKind::Object,
                signal,
                signal.0,
            )?),
            heap.function(update)?,
            request.tool_context.clone(),
        ];
        let marker = heap.object()?;
        let completion = capability()?;
        let completion_port = heap.function(completion)?;
        let runtime = Arc::new(InvocationRuntime {
            owner,
            index: self.index,
            request: request.clone(),
            arguments,
            update,
            signal,
            cancel,
            marker,
            run: None,
            parent: OnceLock::new(),
            completion,
            completion_port,
            read_history: None,
            state: Mutex::new(InvocationState {
                phase: Phase::AwaitingCall,
                future: None,
                native: None,
                device: None,
                call_failure: None,
                update_failure: None,
                bash: None,
                ast: None,
                ast_publication_open: true,
            }),
        });
        let mut publication_waiter = AstPublicationWaiterExit {
            runtime: Arc::downgrade(&runtime),
            armed: true,
        };
        // One original method Call; no repeated execute/arguments Get.
        let called = heap.call(
            &request.callable,
            &request.receiver,
            &runtime.arguments,
            runtime.as_ref(),
        );
        let (future, call_failure) = {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (state.future.take(), state.call_failure.take())
        };
        let call_result = match called {
            Ok(value) if value.same_identity(&runtime.marker) => Ok(()),
            Ok(_) => Err(ActualProviderError::Protocol(
                "builtin method returned a foreign Future lease",
            )),
            Err(error) => Err(call_failure.unwrap_or_else(|| error.into())),
        };
        // If a method entered, even a reply failure cannot discard its Future.
        let settled = match future {
            Some(future) => future.settle(runtime.as_ref()).await,
            None => Err(ActualProviderError::Protocol(
                "builtin method did not enter its one Future",
            )),
        };
        let update_failure = {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.phase = Phase::Settled;
            state.update_failure.take()
        };
        let result = match call_result {
            Ok(()) => settled,
            Err(primary) => match settled {
                Err(secondary)
                    if !matches!(
                        secondary,
                        ActualProviderError::Protocol(
                            "builtin method did not enter its one Future"
                        )
                    ) =>
                {
                    Err(compound(primary, secondary))
                }
                _ => Err(primary),
            },
        };
        let result = match (result, update_failure) {
            (Ok(_), Some(error)) => Err(error),
            (Err(primary), Some(secondary)) => Err(compound(primary, secondary)),
            (result, None) => result,
        };
        runtime.accept_ast_publication(result.is_ok())?;
        if result.is_ok() {
            publication_waiter.armed = false;
        }
        result
    }
}
struct NativeInvocationState {
    job: async_effect::HostEffectId,
    kernel: NativePendingPromise,
    result: NativePendingPromise,
    after_inner: Option<ValueHandle>,
    kernel_completed: bool,
    result_completed: bool,
}

impl BuiltinInvoker {
    async fn invoke_native(
        &self,
        owner: Arc<BuiltinToolSource>,
        run: Arc<NativeBuiltinRun>,
        request: NativeMethodRequest,
    ) -> Result<ValueHandle, ActualProviderError> {
        // The original permission/source check precedes one exact fresh claim.
        // A present Native locator never falls back for unbound/old contexts.
        run.check_values(&request.values)?;
        let claimed = request.claim_context(&run)?;
        let request = Arc::new(request);
        let heap = request.values.heap();
        let update = capability()?;
        let signal = capability()?;
        let completion = capability()?;
        let arguments = vec![
            heap.string(request.call_id.clone())?,
            request.args.clone(),
            request.signal().cloned().unwrap_or(heap.opaque(
                OpaqueKind::Object,
                signal,
                signal.0,
            )?),
            heap.function(update)?,
            request.tool_context.clone(),
        ];
        let runtime = Arc::new(InvocationRuntime {
            owner,
            index: self.index,
            request: request.clone(),
            arguments,
            update,
            signal,
            cancel: request.cancel.child_token(),
            marker: heap.object()?,
            state: Mutex::new(InvocationState {
                phase: Phase::AwaitingCall,
                future: None,
                native: None,
                device: None,
                call_failure: None,
                update_failure: None,
                bash: None,
                ast: None,
                ast_publication_open: true,
            }),
            run: Some(Arc::downgrade(&run)),
            parent: OnceLock::new(),
            completion,
            completion_port: heap.function(completion)?,
            read_history: run.read_history(),
        });
        let mut publication_waiter = AstPublicationWaiterExit {
            runtime: Arc::downgrade(&runtime),
            armed: true,
        };
        let original: Arc<dyn ValueRuntime> = runtime.clone();
        let parent = run.admission.register_parent(original)?;
        let _registration = ParentRegistration {
            admission: run.admission.clone(),
            id: parent,
        };
        runtime.parent.set(parent).map_err(|_| {
            ActualProviderError::Protocol("Native Invocation parent already installed")
        })?;
        let effective = run.admission.scoped(claimed.scope, parent)?;
        if run.admission.capture_parent(&effective)? != parent {
            return Err(ActualProviderError::Protocol(
                "Native original Invocation parent capture differs",
            ));
        }
        // Claimed root ancestry is separate from the InvocationRuntime parent.
        let _claimed_root_parent = claimed.parent;
        xdev_native::approve_root_method(
            runtime.owner.device_snapshot(&request.values)?,
            &request,
            Arc::new(run.admission.scoped(claimed.scope, parent)?),
            claimed.scope,
            parent,
        )
        .await?;
        request.check_admission()?;
        run.check_live()?;
        run.admission.scoped(claimed.scope, parent)?;
        // Build the execution operands after sealing, preserving all other
        // original operands and callbacks owned by this InvocationRuntime.
        let arguments = runtime.execution_arguments();

        let dynamic_read = request.is_read_part()
            && !runtime.owner.members[self.index]
                .execute
                .same_identity(&request.callable);
        let dynamic_pending = if dynamic_read {
            let adopted = run.admission.prepare_pending(claimed.scope, parent)?;
            let awaited = run.admission.prepare_pending(claimed.scope, parent)?;
            let fulfilled =
                heap.fresh_compiled_function(&READ_PART_AWAIT, &[awaited.resolve.clone()])?;
            let rejected =
                heap.fresh_compiled_function(&READ_PART_AWAIT, &[awaited.reject.clone()])?;
            run.admission.attach_native_await(
                &adopted.ticket(),
                &fulfilled,
                &rejected,
                claimed.scope,
                parent,
            )?;
            Some((adopted, awaited))
        } else {
            None
        };
        let called = heap.call(&request.callable, &request.receiver, &arguments, &effective);
        if let Some((adopted, _)) = &dynamic_pending {
            match &called {
                Ok(result) => {
                    heap.call(
                        &adopted.resolve,
                        &heap.undefined()?,
                        &[result.clone()],
                        &effective,
                    )?;
                }
                Err(ValueError::Thrown(reason)) => {
                    heap.call(
                        &adopted.reject,
                        &heap.undefined()?,
                        &[reason.clone()],
                        &effective,
                    )?;
                }
                Err(error) => {
                    return Err(runtime
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .call_failure
                        .take()
                        .unwrap_or_else(|| error.clone().into()));
                }
            }
        }
        // Register the actual admitted ancestor before the driver can consume a
        // delimiter notice. Weak metadata never retains this InvocationRuntime.
        let read_job = if matches!(&runtime.owner.members[self.index].kernel, Kernel::Read(_)) {
            runtime
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .native
                .as_ref()
                .map(|native| native.job)
        } else {
            None
        };
        if let Some(job) = read_job {
            run.bind_read_parent(job, &runtime)?;
        }
        drop(effective);
        let device = {
            let mut state = runtime
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.device.as_mut().and_then(|device| {
                device.operation.take().map(|operation| {
                    (
                        operation,
                        device.scope,
                        device.parent,
                        device.pending.clone(),
                    )
                })
            })
        };
        if let Some((operation, scope, parent, pending)) = device {
            // No fake OS-effect reservation: the selected existing supplier
            // owns any entered effects and their independent settlement.
            let settled =
                xdev_native::execute_device(runtime.clone(), operation, scope, parent).await;
            let effective = run.admission.scoped(scope, parent)?;
            let (resolver, value) = match settled {
                Ok(result) => (&pending.resolve, result),
                Err(ActualProviderError::Value(ValueError::Thrown(reason))) => {
                    (&pending.reject, reason)
                }
                Err(ActualProviderError::Value(ValueError::TypeError(message))) => (
                    &pending.reject,
                    run.native_type_error(&request.values, &message, &effective)?,
                ),
                Err(error) => return Err(error),
            };
            heap.call(resolver, &heap.undefined()?, &[value], &effective)?;
        }
        let (ticket, expected, call_failure) = {
            let mut state = runtime.state.lock().unwrap_or_else(|e| e.into_inner());
            let pending = dynamic_pending
                .as_ref()
                .map(|(_, awaited)| (awaited.ticket(), awaited.promise.clone()))
                .or_else(|| {
                    state
                        .native
                        .as_ref()
                        .map(|native| (native.result.ticket(), native.result.promise.clone()))
                })
                .or_else(|| {
                    state
                        .device
                        .as_ref()
                        .map(|device| (device.pending.ticket(), device.pending.promise.clone()))
                });
            let failure = state.call_failure.take();
            match pending {
                Some((ticket, expected)) => (ticket, expected, failure),
                None => {
                    return Err(failure.unwrap_or_else(|| match called {
                        Err(error) => error.into(),
                        _ => ActualProviderError::Protocol(
                            "Native execute did not prepare its authoritative Promise",
                        ),
                    }));
                }
            }
        };
        let call_result = if dynamic_read {
            Ok(())
        } else {
            match called {
                Ok(value) if value.same_identity(&expected) => Ok(()),
                Ok(_) => Err(ActualProviderError::Protocol(
                    "Original Native execute returned a different async result",
                )),
                Err(error) => Err(call_failure.unwrap_or_else(|| error.into())),
            }
        };
        // Only the single production graph driver pumps. This waiter observes
        // the private authoritative ticket and revocation/fatal/cancel wakes.
        let settled = loop {
            let promise_wake = run.admission.wake();
            let promise_notice = promise_wake.notified();
            tokio::pin!(promise_notice);
            promise_notice.as_mut().enable();
            let tool_wake = run.wake();
            let tool_notice = tool_wake.notified();
            tokio::pin!(tool_notice);
            tool_notice.as_mut().enable();
            if let Err(error) = run.check_live() {
                break Err(error);
            }
            match run.admission.ticket_state(&ticket)? {
                NativePromiseState::Fulfilled(edge) => break Ok(heap.restore_deferred(&edge)?),
                NativePromiseState::Rejected(edge) => {
                    break Err(ValueError::Thrown(heap.restore_deferred(&edge)?).into());
                }
                NativePromiseState::Pending => {}
            }
            tokio::select! {
                _ = promise_notice => {}, _ = tool_notice => {},
            }
        };
        let retained_failure = {
            let mut state = runtime.state.lock().unwrap_or_else(|e| e.into_inner());
            state.phase = Phase::Settled;
            state.update_failure.take()
        };
        let settled = match (settled, retained_failure) {
            (Ok(_), Some(error)) => Err(error),
            (Err(error), Some(secondary)) => {
                // Native continuation already arranged the primary reason;
                // retain its complete update failure carrier without formatting.
                if native_reason(&secondary).is_some() {
                    Err(secondary)
                } else {
                    Err(compound(error, secondary))
                }
            }
            (result, None) => result,
        };
        let result = match call_result {
            Ok(()) => settled,
            Err(primary) => match settled {
                Err(secondary) => Err(compound(primary, secondary)),
                Ok(_) => Err(primary),
            },
        };
        // The final original result ticket, Call and retained failures have
        // all settled. Earlier resolve/send_result success cannot disarm this.
        runtime.accept_ast_publication(result.is_ok())?;
        if result.is_ok() {
            publication_waiter.armed = false;
        }
        result
    }
}

fn native_reason(error: &ActualProviderError) -> Option<ValueHandle> {
    match error {
        ActualProviderError::Value(ValueError::Thrown(reason)) => Some(reason.clone()),
        ActualProviderError::Compound { primary, secondary } => {
            // A Compound containing infrastructure failure is not an ordinary
            // graph rejection, even when its first reason is a native throw.
            let reason = native_reason(primary)?;
            native_reason(secondary)?;
            Some(reason)
        }
        _ => None,
    }
}

static READ_PART_AWAIT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da read.ts:787 await this.execute original adoption continuation",
    name: "readDelimitedPartAfterAwait",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, arguments, runtime| {
        let value = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(&captures[0], &heap.undefined()?, &[value], runtime)
    },
};
static NATIVE_AWAIT_OK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Native execute post-kernel-await dispatch; Read calls fixed after_inner",
    name: "nativeAfterKernelAwait",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, arguments, effective| {
        let value = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(
            &captures[0],
            &captures[0],
            &[heap.number(3.0)?, value],
            effective,
        )
    },
};
static NATIVE_AWAIT_ERR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Original Native execute await rejection dispatch, retaining original reason",
    name: "nativeAfterKernelRejection",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: |heap, captures, _, arguments, effective| {
        let reason = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(
            &captures[0],
            &captures[0],
            &[heap.number(4.0)?, reason],
            effective,
        )
    },
};

impl InvocationRuntime {
    fn execution_arguments(&self) -> Vec<ValueHandle> {
        let mut arguments = self.arguments.clone();
        arguments[1] = self.request.execution_args().clone();
        if self.request.is_read_part() {
            arguments.truncate(3);
        }
        arguments
    }

    fn process_native(
        &self,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        let run = self
            .run
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(ValueError::ExpiredCapability(self.completion))?;
        run.check_live().map_err(|error| match error {
            ActualProviderError::Value(error) => error,
            _ => ValueError::ExpiredCapability(self.completion),
        })?;
        if !receiver.same_identity(&self.completion_port) || arguments.len() != 2 {
            return Err(ValueError::TypeError(
                "Wrong original Native completion port receiver/arguments".into(),
            ));
        }
        let heap = self.request.values.heap();
        let ValueSnapshot::Number(kind) = heap.snapshot(&arguments[0])? else {
            return Err(ValueError::WrongRuntimeReply("Native completion kind"));
        };
        heap.adopt(&arguments[1])?;
        if kind == 9.0 { return self.ast_grep_language_original(&arguments[1],effective); }
        if kind == 8.0 {
            {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let native = state
                    .native
                    .as_ref()
                    .ok_or(ValueError::ExpiredCapability(self.completion))?;
                if state.phase != Phase::Running || native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
            }
            let ValueSnapshot::String(path) = heap.snapshot(&arguments[1])? else {
                return Err(ValueError::WrongRuntimeReply("Attachment lookup URI"));
            };
            let path = path
                .to_utf8()
                .map_err(|_| ValueError::UnsupportedOperation("Attachment URI UTF16 boundary"))?;
            let guard = || {
                run.check_live().map_err(|e| match e {
                    ActualProviderError::Value(e) => e,
                    _ => ValueError::ExpiredCapability(self.completion),
                })?;
                let grants = self.owner.grants.lock().unwrap_or_else(|e| e.into_inner());
                let grant = &grants[self.index];
                if !grant.enabled
                    || self.cancel.is_cancelled()
                    || !grant.entered.contains_key(&self.update)
                {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
                Ok(())
            };
            let error = heap.restore_deferred(&run.error_constructor)?;
            let outcome = read_attachments::lookup(
                &self.request.values,
                &self.request.receiver,
                &path,
                self.read_history.as_ref(),
                &error,
                effective,
                &guard,
            );
            match outcome {
                Ok(result) => {
                    let reply = heap.object()?;
                    let (name, text) = match result {
                        Ok(path) => ("path", path),
                        Err(message) => ("error", message),
                    };
                    heap.define_own_data(
                        &reply,
                        name.into(),
                        heap.string(text.into())?,
                        effective,
                    )?;
                    return Ok(reply);
                }
                Err(error) => {
                    run.fail(error.clone().into());
                    return Err(error);
                }
            }
        }
        if kind == 7.0 {
            {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let native = state
                    .native
                    .as_ref()
                    .ok_or(ValueError::ExpiredCapability(self.completion))?;
                if state.phase != Phase::Running || native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
            }
            let batch = match edit_batch::prepare(heap, &self.request.tool_context, effective) {
                Ok(batch) => batch,
                Err(error) => {
                    run.fail(error.clone().into());
                    return Err(error);
                }
            };
            let Some(id) = batch.lsp_batch_id else {
                return heap.undefined();
            };
            let result = heap.object()?;
            heap.define_own_data(&result, "id".into(), heap.string(id.into())?, effective)?;
            heap.define_own_data(
                &result,
                "flush".into(),
                heap.boolean(batch.lsp_flush)?,
                effective,
            )?;
            return Ok(result);
        }
        if kind == 6.0 {
            // Original synchronous Writing callback, before the worker enters
            // mkdir/write. Ignore its returned value, including a Promise.
            {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let native = state
                    .native
                    .as_ref()
                    .ok_or(ValueError::ExpiredCapability(self.completion))?;
                if state.phase != Phase::Running || native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
            }
            if let Err(error) = heap.call(
                &self.arguments[3],
                &heap.undefined()?,
                &[arguments[1].clone()],
                effective,
            ) {
                let retained = self
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update_failure
                    .clone();
                // Preserve the full original thrown/Host failure carrier. The
                // worker receives only refusal, never Graph handles or text.
                run.fail(retained.unwrap_or_else(|| error.clone().into()));
                return Err(error);
            }
            return heap.undefined();
        }
        if kind == 5.0 {
            // This does not settle either promise or mutate completion flags.
            // Original params may have programmable toJSON/getters; consume
            // them now under key "arg", after the preceding repair settled.
            {
                let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let native = state
                    .native
                    .as_ref()
                    .ok_or(ValueError::ExpiredCapability(self.completion))?;
                if state.phase != Phase::Running || native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
            }
            heap.define_own_data(
                &arguments[1],
                PropertyKey::String("arg".into()),
                self.request.execution_args().clone(),
                effective,
            )?;
            let text = heap
                .stringify(&arguments[1], StringifyMode::Plain, None, effective)?
                .ok_or(ValueError::WrongRuntimeReply(
                    "Edit blackbox JSON.stringify undefined",
                ))?;
            return heap.string(text);
        }
        let (job, kernel_resolver, result_resolve, result_reject, after_inner) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.phase != Phase::Running {
                return Err(ValueError::ExpiredCapability(self.completion));
            }
            let native = state
                .native
                .as_mut()
                .ok_or(ValueError::ExpiredCapability(self.completion))?;
            if kind == 0.0 {
                if native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
            } else if kind == 1.0 || kind == 2.0 {
                if native.kernel_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
                native.kernel_completed = true;
            } else if kind == 3.0 || kind == 4.0 {
                if !native.kernel_completed || native.result_completed {
                    return Err(ValueError::ExpiredCapability(self.completion));
                }
                native.result_completed = true;
            } else {
                return Err(ValueError::WrongRuntimeReply(
                    "Unknown Native completion kind",
                ));
            }
            (
                native.job,
                if kind == 2.0 {
                    native.kernel.reject.clone()
                } else {
                    native.kernel.resolve.clone()
                },
                native.result.resolve.clone(),
                native.result.reject.clone(),
                native.after_inner.clone(),
            )
        };
        if kind == 0.0 {
            let updated = heap.call(
                &self.arguments[3],
                &heap.undefined()?,
                &[arguments[1].clone()],
                effective,
            );
            return match updated {
                Ok(_) => heap.undefined(),
                Err(ValueError::Thrown(_)) => heap.undefined(), // exact failure already retained by original update port
                Err(error) => {
                    let retained = self
                        .state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .update_failure
                        .clone();
                    if let Some(failure) = retained {
                        run.fail(failure);
                    }
                    Err(error)
                }
            };
        }
        if kind == 1.0 || kind == 2.0 {
            if kind==1.0&&matches!(&self.owner.members[self.index].kernel,Kernel::Wait(_)){
                // The actual Native effect owner has just materialized its
                // successful kernel result. Only this sealed completion edge
                // may associate the private reservation with a Graph result.
                self.owner.wait_session.bind_result(self.update.0,&self.request.call_id,&arguments[1],&self.request.values)
                    .map_err(|error|match error{ActualProviderError::Value(error)=>error,_=>ValueError::UnsupportedOperation("Wait private kernel-result binding failed")})?;
            }
            // Completing this slot may enqueue dynamic thenable adoption. The
            // persistent native awaiter runs only after that slot is terminal.
            heap.call(
                &kernel_resolver,
                &heap.undefined()?,
                &[arguments[1].clone()],
                effective,
            )?;
            return heap.undefined();
        }
        let result = if kind == 4.0 {
            let error = self.ast_apply_failure(&arguments[1], effective);
            self.close_ast_scope(effective);
            Err(error)
        } else if self.has_ast_scope() {
            self.resume_ast_native(effective)
                .map_err(|error| match error {
                    ActualProviderError::Value(error) => error,
                    other => {
                        run.fail(other);
                        ValueError::UnsupportedOperation(
                            "AST native scope continuation infrastructure failure retained",
                        )
                    }
                })
        } else if self.has_bash_preflight() {
            self.resume_bash_native(effective)
                .map_err(|error| match error {
                    ActualProviderError::Value(error) => error,
                    other => {
                        run.fail(other);
                        ValueError::UnsupportedOperation(
                            "Bash native continuation infrastructure failure retained",
                        )
                    }
                })
        } else if let Some(body) = after_inner {
            heap.call(
                &body,
                &heap.undefined()?,
                &[arguments[1].clone()],
                effective,
            )
        } else {
            self.finish_ast_output(&arguments[1], effective)
        };
        let updates = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .update_failure
            .take();
        let result = match (result, updates) {
            (Ok(_), Some(error)) => {
                let Some(reason) = native_reason(&error) else {
                    run.fail(error);
                    return Err(ValueError::UnsupportedOperation(
                        "Native update infrastructure failure retained",
                    ));
                };
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update_failure = Some(error);
                Err(ValueError::Thrown(reason))
            }
            (Err(primary), Some(secondary)) => {
                if native_reason(&secondary).is_none() {
                    run.fail(compound(primary.into(), secondary));
                    return Err(ValueError::UnsupportedOperation(
                        "Native combined update infrastructure failure retained",
                    ));
                }
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update_failure = Some(compound(primary.clone().into(), secondary));
                Err(primary)
            }
            (result, None) => result,
        };
        match result {
            Ok(value) => {
                heap.call(&result_resolve, &heap.undefined()?, &[value], effective)?;
            }
            Err(ValueError::Thrown(reason)) => {
                heap.call(&result_reject, &heap.undefined()?, &[reason], effective)?;
            }
            Err(error) => {
                run.fail(error.clone().into());
                return Err(error);
            }
        }
        run.finish_graph(job);
        // Phase stays Running through execute-result adoption. Invoker changes
        // it only when its authoritative ticket or terminal failure has settled.
        heap.undefined()
    }
}

fn compound(primary: ActualProviderError, secondary: ActualProviderError) -> ActualProviderError {
    ActualProviderError::Compound {
        primary: Arc::new(primary),
        secondary: Arc::new(secondary),
    }
}

impl InvocationRuntime {
    fn get(
        &self,
        key: &str,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ActualProviderError> {
        Ok(self
            .request
            .values
            .heap()
            .get(self.request.execution_args(), &key.into(), effective)?)
    }
    fn string(
        &self,
        value: &ValueHandle,
        _effective: &dyn ValueRuntime,
    ) -> Result<String, ActualProviderError> {
        match self.request.values.heap().snapshot(value)? {
            ValueSnapshot::String(value) => value.to_utf8().map_err(|_| {
                ActualProviderError::Unsupported("builtin OS UTF16 lone-surrogate boundary")
            }),
            _ => Err(
                ValueError::TypeError("Builtin OS string argument must be a string".into()).into(),
            ),
        }
    }
    fn method(
        &self,
        receiver: &ValueHandle,
        name: &str,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ActualProviderError> {
        let heap = self.request.values.heap();
        let callable = heap.get(receiver, &name.into(), effective)?;
        Ok(heap.call(&callable, receiver, &[], effective)?)
    }
    fn check_not_aborted(&self) -> Result<(), ActualProviderError> {
        if self.cancel.is_cancelled() {
            return Err(ActualProviderError::Callback("Aborted: Cancelled".into()));
        }
        Ok(())
    }
    fn nullish_boolean(&self, value: &ValueHandle) -> Result<bool, ActualProviderError> {
        match self.request.values.heap().snapshot(value)? {
            ValueSnapshot::Undefined | ValueSnapshot::Null => Ok(true),
            ValueSnapshot::Boolean(value) => Ok(value),
            _ => Err(ActualProviderError::Unsupported(
                "search native Boolean option binding",
            )),
        }
    }
    fn path_string(
        &self,
        value: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<Option<String>, ActualProviderError> {
        match self.request.values.heap().snapshot(value)? {
            ValueSnapshot::Undefined | ValueSnapshot::Null => Ok(None),
            ValueSnapshot::String(_) => Ok(Some(self.string(value, effective)?)),
            // A raw path iterable interleaves iterator observations with awaited
            // path expansion. Never flatten it to JSON or consume it eagerly.
            _ => Err(ActualProviderError::Unsupported(
                "search original path iterable action boundary",
            )),
        }
    }
    fn remember_update(&self, error: ActualProviderError) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if self.run.is_none() {
            // Preserve first-failure Boundary fixture behavior; emit's outer
            // frame must not append the same callback failure a second time.
            if state.update_failure.is_none() {
                state.update_failure = Some(error);
            }
            return;
        }
        state.update_failure = Some(match state.update_failure.take() {
            Some(primary) => compound(primary, error),
            None => error,
        });
    }
    fn output(&self, output: ToolOutput) -> Result<ValueHandle, ActualProviderError> {
        output_graph(&self.request.values, output)
    }
    fn emit(&self, output: ToolOutput, effective: &dyn ValueRuntime) {
        let result = self.output(output).and_then(|partial| {
            Ok(self.request.values.heap().call(
                &self.arguments[3],
                &self.request.values.heap().undefined()?,
                &[partial],
                effective,
            )?)
        });
        if let Err(error) = result {
            self.remember_update(error);
        }
    }
    fn environment(
        &self,
        env: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<Vec<(String, ValueHandle)>, ActualProviderError> {
        let heap = self.request.values.heap();
        // Object.keys and Object.entries are separate phases in fixed Bash.
        if !heap.truthy(env)? || heap.own_enumerable_string_keys(env, effective)?.is_empty() {
            return Ok(Vec::new());
        }
        let mut entries = Vec::new();
        for key in heap.own_keys(env, effective)? {
            let PropertyKey::String(name) = &key else {
                continue;
            };
            if heap
                .own_descriptor(env, &key, effective)?
                .is_some_and(|descriptor| descriptor.enumerable())
            {
                entries.push((name.clone(), heap.get(env, &key, effective)?));
            }
        }
        // Object.entries completes all value Gets before the source for-loop.
        let mut normalized = Vec::new();
        for (key, value) in entries {
            let key = key
                .to_utf8()
                .map_err(|_| ActualProviderError::Unsupported("Bash env key UTF16 OS boundary"))?;
            if !valid_env_name(&key) {
                return Err(ActualProviderError::Callback(format!(
                    "Invalid bash env name: {key}"
                )));
            }
            normalized.push((key, value));
        }
        Ok(normalized)
    }
    fn prepare_action(
        &self,
        effective: &dyn ValueRuntime,
    ) -> Result<PreparedAction, ActualProviderError> {
        let member = &self.owner.members[self.index];
        let action = match &member.kernel {
            Kernel::Read(tool) => {
                let path = self.get("path", effective)?;
                let path = self.string(&path, effective)?;
                let tool = tool.clone();
                if read_attachments::matches(&path) {
                    return Ok(PreparedAction::Native(Action::ReadAttachment(
                        tool, path, None,
                    )));
                }
                let local_available = tool
                    .ctx
                    .edit_host()
                    .is_some_and(|host| host.path_policy().local_sandbox_root.is_some());
                let normalized_read = ara_tools::paths::normalize_path_like_input(&path);
                let native_routed = |entry: &str| {
                    let (url, _) = ara_tools::internal_urls::split_content_url_selector(entry);
                    let route = tool
                        .ctx
                        .content_uri_port()
                        .map_or(ara_tools::ContentUriRoute::Unregistered, |port| {
                            port.route(&url)
                        });
                    let local = local_available
                        && ara_tools::internal_urls::hierarchical_scheme(entry)
                            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("local"))
                        && route == ara_tools::ContentUriRoute::Unregistered;
                    // A whole attachment list reaches only the shared routed
                    // semicolon parser. Each resulting attachment part must
                    // still satisfy the exact real lookup supplier's pattern.
                    let attachment = route != ara_tools::ContentUriRoute::Removed
                        && (read_attachments::matches(entry)
                            || entry == normalized_read
                                && entry.contains(';')
                                && ara_tools::internal_urls::hierarchical_scheme(entry)
                                    .is_some_and(|scheme| {
                                        scheme.eq_ignore_ascii_case("attachment")
                                    }));
                    local || attachment
                };
                if let Some(parts) = tool
                    .routed_delimited_parts(&path, &native_routed)
                    .map_err(|error| ActualProviderError::Callback(error.0))?
                {
                    return Ok(PreparedAction::Native(Action::ReadDelimited(parts, None)));
                }
                match self
                    .owner
                    .device_snapshot
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .as_ref()
                    .and_then(Weak::upgrade)
                    .and_then(|snapshot| xdev_native::operation(&snapshot, &path, None))
                {
                    Some(operation) => return Ok(PreparedAction::Device(operation)),
                    None => {
                        // Fixed read.ts:1304-1324 promotes local files into the
                        // existing FS reader, while retaining URL selectors.
                        // Registered/Removed Host routes keep precedence.
                        let local = ara_tools::internal_urls::hierarchical_scheme(&path)
                            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("local"));
                        let (url, selector) =
                            ara_tools::internal_urls::split_content_url_selector(&path);
                        let native_local = local
                            && tool.ctx.content_uri_port().is_none_or(|port| {
                                port.route(&url) == ara_tools::ContentUriRoute::Unregistered
                            });
                        if native_local {
                            let plan = tool
                                .ctx
                                .edit_host()
                                .and_then(|host| host.path_policy().local_sandbox_root)
                                .map(|root| {
                                    crate::mcp_native::local::LocalPlan::for_session_root(
                                        root,
                                        format!("local://{}", &url["local://".len()..]),
                                    )
                                });
                            Action::ReadLocal(tool, plan, path, selector, None)
                        } else {
                            Action::Read(tool, path, None)
                        }
                    }
                }
            }
            Kernel::Write(tool) => {
                // Fixed native parameter binding, before any routing/body work.
                let path = self.get("path", effective)?;
                let content = self.get("content", effective)?;
                {
                    let path = self.string(&path, effective)?;
                    let content = self.string(&content, effective)?;
                    tool.require_write_transport_target(&path)
                        .map_err(|error| match self.owner.ast_module() {
                            Ok(module) => {
                                ActualProviderError::Value(module.throw(error.0.into(), effective))
                            }
                            Err(boundary) => boundary,
                        })?;
                    match self.prepare_ast_resolution(tool, &path, &content, effective)? {
                        Some(action) => action,
                        None => match self
                            .owner
                            .device_snapshot
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .as_ref()
                            .and_then(Weak::upgrade)
                            .and_then(|snapshot| {
                                xdev_native::operation(&snapshot, &path, Some(&content))
                            }) {
                            Some(operation) => return Ok(PreparedAction::Device(operation)),
                            None => {
                                let receipt=self.request.write_receipt().unwrap_or_else(||ara_tools::ContentUriWriteReceipt::capture(&tool.ctx,&path));
                                receipt.check(&tool.ctx,&path).map_err(|error|ActualProviderError::Callback(error.0))?;
                                Action::Write(tool.clone(), path, content, None,receipt)
                            },
                        },
                    }
                }
            }
            Kernel::Task(_)=>{let owner=self.owner.task_session();let request=task_native::prepare(&owner,self.request.execution_args(),effective)?;Action::Task(owner,request)},
            Kernel::Wait(_)=>Action::Wait(self.owner.wait_session.clone(),self.update.0,self.request.call_id.clone(),self.bash_steering(effective)?,None),
            Kernel::Bash(tool) => self.prepare_bash(tool, effective)?,
            Kernel::Edit(tool) => {
                // b91 compares the effective hooked parameters on every execute,
                // including calls with a provider-final streamed preview.
                let call_id = self.request.call_id.to_utf8().map_err(|_| {
                    ActualProviderError::Unsupported("EditSession call-ID UTF16 boundary")
                })?;
                let json = self
                    .request
                    .values
                    .heap()
                    .stringify(
                        self.request.execution_args(),
                        StringifyMode::Plain,
                        None,
                        effective,
                    )?
                    .ok_or(ActualProviderError::Unsupported(
                        "EditSession.setArgsJson undefined binding",
                    ))?;
                let json = json.to_utf8().map_err(|_| {
                    ActualProviderError::Unsupported("EditSession JSON text UTF16 boundary")
                })?;
                let call = tool
                    .try_call_for_args_json(&call_id, &json)
                    .map_err(|error| ValueError::TypeError(error.0))?;
                let batch = edit_batch::prepare(
                    self.request.values.heap(),
                    &self.request.tool_context,
                    effective,
                )?;
                Action::Edit(tool.clone(), call, batch)
            }
            Kernel::Grep(tool) => {
                // Fixed grep.ts:960 destructures before untilAborted.
                let pattern = self.get("pattern", effective)?;
                let path = self.get("path", effective)?;
                let case_sensitive = self.get("case", effective)?;
                let gitignore = self.get("gitignore", effective)?;
                let skip = self.get("skip", effective)?;
                self.check_not_aborted()?;
                let trimmed = self.method(&pattern, "trim", effective)?;
                let heap = self.request.values.heap();
                if !heap.truthy(&trimmed)? {
                    return Err(ActualProviderError::Callback(
                        "Pattern must not be empty".into(),
                    ));
                }
                // Number.isFinite is strict: it never coerces an object/string.
                let skip = match heap.snapshot(&skip)? {
                    ValueSnapshot::Undefined | ValueSnapshot::Null => 0.0,
                    ValueSnapshot::Number(value) if value.is_finite() => value.floor(),
                    _ => f64::NAN,
                };
                if !skip.is_finite() || skip < 0.0 {
                    return Err(ActualProviderError::Callback(
                        "Skip must be a non-negative number".into(),
                    ));
                }
                let case_sensitive = match heap.snapshot(&case_sensitive)? {
                    ValueSnapshot::Undefined | ValueSnapshot::Null => true,
                    _ => heap.truthy(&case_sensitive)?,
                };
                Action::Grep(
                    tool.clone(),
                    GrepOsValues {
                        pattern: self.string(&pattern, effective)?,
                        path: self.path_string(&path, effective)?,
                        case_sensitive,
                        use_gitignore: self.nullish_boolean(&gitignore)?,
                        skip,
                    },
                )
            }
            Kernel::Glob(tool) => {
                // Fixed glob.ts:178 destructures all four before abort/path work.
                let path = self.get("path", effective)?;
                let limit = self.get("limit", effective)?;
                let hidden = self.get("hidden", effective)?;
                let gitignore = self.get("gitignore", effective)?;
                self.check_not_aborted()?;
                let limit = match self.request.values.heap().snapshot(&limit)? {
                    ValueSnapshot::Undefined | ValueSnapshot::Null => {
                        ara_tools::glob::DEFAULT_LIMIT as f64
                    }
                    ValueSnapshot::Number(value) => value,
                    _ => f64::NAN,
                };
                // Validate limit in the action kernel at the original position
                // after path preparation, so path errors keep their precedence.
                Action::Glob(
                    tool.clone(),
                    GlobOsValues {
                        path: self.path_string(&path, effective)?,
                        limit,
                        include_hidden: self.nullish_boolean(&hidden)?,
                        use_gitignore: self.nullish_boolean(&gitignore)?,
                    },
                )
            }
            Kernel::Debug(_) => {
                Action::Unavailable(ara_tools::default_declarations::DEBUG_UNAVAILABLE)
            }
            Kernel::WebSearch(_) => {
                Action::Unavailable(ara_tools::default_declarations::WEB_SEARCH_UNAVAILABLE)
            }
            Kernel::AstEdit(tool) => {
                self.check_not_aborted()?;
                self.prepare_ast_scope(tool, effective)?
            }
            Kernel::AstGrep(tool) => {
                // Fixed ast-grep.ts:199 puts all parameter Gets in untilAborted.
                self.check_not_aborted()?;
                let pattern = self.get("pat", effective)?;
                let pattern = self.method(&pattern, "trim", effective)?;
                let heap = self.request.values.heap();
                let length = heap.get(&pattern, &"length".into(), effective)?;
                if matches!(heap.snapshot(&length)?, ValueSnapshot::Number(value) if value == 0.0) {
                    return Err(ActualProviderError::Callback(
                        "`pat` must be a non-empty pattern".into(),
                    ));
                }
                // Defined skip is read twice, then Math.floor performs ToNumber.
                let first_skip = self.get("skip", effective)?;
                let skip = if matches!(heap.snapshot(&first_skip)?, ValueSnapshot::Undefined) {
                    0.0
                } else {
                    heap.to_number(&self.get("skip", effective)?, effective)?
                        .floor()
                };
                if !skip.is_finite() || skip < 0.0 {
                    return Err(ActualProviderError::Callback(
                        "skip must be a non-negative number".into(),
                    ));
                }
                let path = self.get("path", effective)?;
                Action::AstGrep(
                    tool.clone(),
                    AstGrepOsValues {
                        pattern: self.string(&pattern, effective)?,
                        language: None, // Read only after the actual scope await, through its dispatcher.
                        path: self.path_string(&path, effective)?,
                        skip,
                    },
                    None,
                )
            }
        };
        Ok(PreparedAction::Native(action))
    }
}

impl InvocationRuntime {
    fn start_native(
        &self,
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ActualProviderError> {
        let prepared = (|| {
            let run = self
                .run
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or(ActualProviderError::Protocol("Native tool Run expired"))?;
            let scope = run.admission.capture_scope(effective)?;
            let parent = run.admission.capture_parent(effective)?;
            if self.parent.get() != Some(&parent) {
                return Err(ActualProviderError::Protocol(
                    "Native execute lost its original InvocationRuntime parent",
                ));
            }
            let prepared_action = self.prepare_action(effective)?;
            let mut action = match prepared_action {
                PreparedAction::Native(action) => action,
                PreparedAction::Device(operation) => {
                    let pending = Arc::new(run.admission.prepare_pending(scope, parent)?);
                    let promise = pending.promise.clone();
                    self.state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .device = Some(xdev_native::DevicePrepared {
                        pending,
                        scope,
                        parent,
                        operation: Some(operation),
                    });
                    return Ok(promise);
                }
            };
            let kernel = run.admission.prepare_pending(scope, parent)?;
            let result = run.admission.prepare_pending(scope, parent)?;
            let heap = self.request.values.heap();
            let after_inner = if matches!(
                &action,
                Action::Read(..)
                    | Action::ReadLocal(..)
                    | Action::ReadAttachment(..)
                    | Action::ReadDelimited(..)
            ) {
                Some(run.after_inner(
                    &self.request.values,
                    &self.request.receiver,
                    self.request.execution_args(),
                )?)
            } else {
                None
            };
            let fulfilled =
                heap.fresh_compiled_function(&NATIVE_AWAIT_OK, &[self.completion_port.clone()])?;
            let rejected =
                heap.fresh_compiled_function(&NATIVE_AWAIT_ERR, &[self.completion_port.clone()])?;
            run.admission.attach_native_await(
                &kernel.ticket(),
                &fulfilled,
                &rejected,
                scope,
                parent,
            )?;
            let reservation =
                run.reserve(&self.request.values, scope, parent, &self.completion_port)?;
            let id = reservation.id();
            let permit = match run.final_enter(
                &reservation,
                &self.owner,
                self.index,
                self.update,
                &self.request,
                self.cancel.clone(),
            ) {
                Ok(permit) => permit,
                Err(error) => {
                    run.release(&reservation);
                    return Err(error);
                }
            };
            let publisher = reservation.publisher();
            if let Action::AstGrep(_,_,language)=&mut action {
                *language=Some(Arc::new(ast_grep_native::NativeAstGrepLanguage(publisher.clone())));
            }
            match &mut action {
                Action::Read(_, _, dispatcher)
                | Action::ReadLocal(_, _, _, _, dispatcher)
                | Action::ReadDelimited(_, dispatcher) => *dispatcher = Some(publisher.clone()),
                _ => {}
            }
            if let Action::ReadAttachment(_, _, lookup) = &mut action {
                *lookup = Some(publisher.clone());
            }
            if let Action::Edit(_, call, _) = &mut action {
                call.set_blackbox_serializer(Arc::new(NativeEditBlackboxSerializer(
                    publisher.clone(),
                )));
                call.set_repair_dispatcher(Arc::new(NativeEditRepairDispatcher(publisher.clone())));
            }
            if let Action::Wait(_,_,_,_,wait_publisher)=&mut action{*wait_publisher=Some(publisher.clone());}
            if let Action::Write(_, _, _, progress,_) = &mut action {
                *progress = Some(Arc::new(NativeWriteProgress(publisher.clone())));
            }
            let update: BashUpdateFn = Arc::new(move |partial| {
                let _ = publisher.partial(ToolOutput::text(partial.output));
            });
            let public_result = result.promise.clone();
            self.state.lock().unwrap_or_else(|e| e.into_inner()).native =
                Some(NativeInvocationState {
                    job: id,
                    kernel,
                    result,
                    after_inner,
                    kernel_completed: false,
                    result_completed: false,
                });
            // The entered future is retained in the independent owner before any
            // await or graph callback; all future captures are native data only.
            if let Err(fault) = reservation.enter(
                self.cancel.clone(),
                action_future(action, permit, self.cancel.clone(), update),
            ) {
                run.finish_graph(id);
                // Positive not-entered branch: owner kept its released receipt and
                // never polled the OS future. Do not wait an unresolvable ticket.
                self.state.lock().unwrap_or_else(|e| e.into_inner()).native = None;
                return Err(ActualProviderError::Callback(fault.message().to_owned()));
            }
            Ok(public_result)
        })();
        // An AST scope iterator may already be open even though the next
        // native effect was not admitted. There will be no kind4 receipt.
        if prepared.is_err() {
            self.close_ast_scope(effective);
        }
        prepared
    }
    fn start_legacy(
        &self,
        effective: &dyn ValueRuntime,
    ) -> Result<LegacyEffect, ActualProviderError> {
        let prepared = (|| {
            let action = match self.prepare_action(effective)? {
                PreparedAction::Native(action) => action,
                PreparedAction::Device(_) => {
                    return Err(ActualProviderError::Unsupported(
                        "Device dispatch requires Native Run ownership",
                    ));
                }
            };
            let permit =
                self.owner
                    .enter(self.index, self.update, &self.request, self.cancel.clone())?;
            let partials = Arc::new(Mutex::new(std::collections::VecDeque::new()));
            let wake = Arc::new(tokio::sync::Notify::new());
            let native_partials = partials.clone();
            let native_wake = wake.clone();
            let update: BashUpdateFn = Arc::new(move |partial| {
                native_partials
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push_back(ToolOutput::text(partial.output));
                native_wake.notify_waiters();
            });
            Ok(LegacyEffect {
                future: action_future(action, permit, self.cancel.clone(), update),
                partials,
                wake,
            })
        })();
        if prepared.is_err() {
            self.close_ast_scope(effective);
        }
        prepared
    }
}

/// Native captures only. Materialization and original update/continuation Calls
/// happen on the graph driver (or the explicit pre-Native Boundary fixture).
/// Effect-bound exact READ publication. The original delimiter dispatcher
/// still owns child invocation/aggregation; no alternate driver is introduced.
struct NativeReadDispatcher(async_effect::NativeEffectPublisher);
#[async_trait]
impl ara_tools::read::ReadDelimitedDispatcher for NativeReadDispatcher {
    async fn read_parts(
        &self,
        parts: Vec<String>,
        cancel: CancellationToken,
    ) -> Result<ToolOutput, ToolError> {
        ara_tools::read::ReadDelimitedDispatcher::read_parts(
            &read_delimited::Dispatcher(self.0.clone()),
            parts,
            cancel,
        )
        .await
    }
    async fn publish_in_memory_output(&self, output: ara_rpc::WireValue) -> Result<(), ToolError> {
        self.0
            .publish_read_output(output)
            .map_err(|error| ToolError(error.message().to_owned()))
    }
    async fn publish_profile_type_error(&self, message: String) -> Result<(), ToolError> {
        self.0
            .publish_read_type_error(message)
            .map_err(|error| ToolError(error.message().to_owned()))
    }
}

fn action_future(
    action: Action,
    permit: EffectPermit,
    cancel: CancellationToken,
    update: BashUpdateFn,
) -> NativeEffectFuture {
    let action = match action {
        Action::BashPreflight(tool, command, cwd, env, prepared) => {
            return bash_native::preflight_future(tool, command, cwd, env, prepared, permit);
        }
        other => other,
    };
    Box::pin(async move {
        let _permit = permit;
        if matches!(&action, Action::Write(..)) && cancel.is_cancelled() {
            return Err(NativeToolFault::NotDispatched("Aborted: Cancelled".into()));
        }
        if matches!(
            &action,
            Action::Grep(..)
                | Action::Glob(..)
                | Action::AstGrep(..)
                | Action::AstEdit(..)
                | Action::AstScope(..)
        ) && cancel.is_cancelled()
        {
            return Err(NativeToolFault::Cancelled);
        }
        match action {
            Action::Task(owner,request)=>{
                let result=owner.execute(request,cancel).await;
                if !_permit.grants.lock().unwrap_or_else(|e|e.into_inner())[_permit.index].enabled{return Err(NativeToolFault::Kernel("Task current Host grant revoked".into()));}
                return result.map_err(|error|NativeToolFault::Kernel(error.0));
            }
            Action::Wait(owner,invocation,call_id,soft,publisher)=>{
                let result=owner.execute(invocation,call_id,cancel,soft,Arc::new(move|partial|{if let Some(publisher)=&publisher{let _=publisher.partial(partial);}})).await;
                // Revoking the authentic grant cannot become soft-interrupt success.
                if !_permit.grants.lock().unwrap_or_else(|e|e.into_inner())[_permit.index].enabled{return Err(NativeToolFault::Kernel("Wait current Host grant revoked".into()));}
                return result.map_err(|error|NativeToolFault::Kernel(error.0));
            }
            Action::ReadAttachment(tool, path, lookup) => {
                let publisher = lookup.ok_or(NativeToolFault::NotDispatched(
                    "Attachment READ requires Native Run ownership".into(),
                ))?;
                let resolved = publisher.lookup_read_attachment(path).await?;
                if cancel.is_cancelled() {
                    return Err(NativeToolFault::Cancelled);
                }
                tool.execute_os_path_with_delimited_dispatcher(
                    &resolved,
                    cancel,
                    Some(Arc::new(NativeReadDispatcher(publisher))
                        as Arc<dyn ara_tools::read::ReadDelimitedDispatcher>),
                )
                .await
            }
            Action::Read(tool, path, publisher) => {
                tool.execute_os_path_with_delimited_dispatcher(
                    &path,
                    cancel,
                    publisher.map(|publisher| {
                        Arc::new(NativeReadDispatcher(publisher))
                            as Arc<dyn ara_tools::read::ReadDelimitedDispatcher>
                    }),
                )
                .await
            }
            Action::ReadDelimited(parts, publisher) => {
                let publisher = publisher.ok_or(NativeToolFault::NotDispatched(
                    "Delimited READ requires Native Run ownership".into(),
                ))?;
                publisher.read_delimited(parts).await?;
                return Ok(ToolOutput::text("")); // exact result remains driver-owned
            }
            Action::ReadLocal(tool, plan, path, selector, publisher) => {
                async {
                    if cancel.is_cancelled() {
                        return Err(ara_agent::ToolError(format!(
                            "Host URI read for {path} was aborted"
                        )));
                    }
                    let plan = plan.ok_or_else(|| {
                        ara_agent::ToolError("local:// is unavailable in this session".into())
                    })?;
                    let resolved = crate::mcp_native::local::resolve(plan)
                        .await
                        .map_err(ara_agent::ToolError)?;
                    if cancel.is_cancelled() {
                        return Err(ara_agent::ToolError(format!(
                            "Host URI read for {path} was aborted"
                        )));
                    }
                    match resolved {
                        Some((absolute, _)) => {
                            tool.execute_promoted_os_path_with_delimited_dispatcher(
                                &absolute,
                                selector.as_deref(),
                                cancel,
                                publisher.map(|publisher| {
                                    Arc::new(NativeReadDispatcher(publisher))
                                        as Arc<dyn ara_tools::read::ReadDelimitedDispatcher>
                                }),
                            )
                            .await
                        }
                        // Existing non-file URI routing remains authoritative.
                        None => {
                            tool.execute_os_path_with_delimited_dispatcher(
                                &path,
                                cancel,
                                publisher.map(|publisher| {
                                    Arc::new(NativeReadDispatcher(publisher))
                                        as Arc<dyn ara_tools::read::ReadDelimitedDispatcher>
                                }),
                            )
                            .await
                        }
                    }
                }
                .await
            }
            Action::Write(tool, path, content, progress,receipt) => {
                return match progress {
                    Some(progress) => tool
                        .execute_os_values_with_receipt(&path, &content, cancel, progress,receipt)
                        .await
                        .map_err(|error| match error {
                            ara_tools::write::WriteExecutionError::NotDispatched(error) => {
                                NativeToolFault::NotDispatched(error.0)
                            }
                            ara_tools::write::WriteExecutionError::Tool(error) => {
                                NativeToolFault::Kernel(error.0)
                            }
                        }),
                    None => tool
                        .execute_os_values_selected(&path, &content, cancel,receipt)
                        .await
                        .map_err(|error|match error{ara_tools::write::WriteExecutionError::NotDispatched(error)=>NativeToolFault::NotDispatched(error.0),ara_tools::write::WriteExecutionError::Tool(error)=>NativeToolFault::Kernel(error.0)}),
                };
            }
            Action::Edit(tool, call, batch) => {
                tool.execute_os_call_with_request(call, batch, cancel).await
            }
            Action::Grep(tool, input) => tool.execute_os_values(input, cancel).await,
            Action::Glob(tool, input) => tool.execute_os_values(input, cancel).await,
            Action::AstGrep(tool,input,language) => tool.execute_os_values_with_language(input,cancel,language).await,
            Action::Unavailable(message) => Err(ToolError(message.into())),
            Action::AstEdit(tool, step, receipt) => {
                ast_edit_native::execute_finish(tool, step, receipt).await
            }
            Action::AstScope(tool, step, receipt) => {
                ast_edit_native::scope_step(tool, step, receipt, cancel).await
            }
            Action::BashPreflight(..) => {
                unreachable!("Bash preflight uses its original native future")
            }
        }
        .map_err(|error| NativeToolFault::Kernel(error.0))
    })
}

enum PreparedAction {
    Native(Action),
    Device(xdev_native::DeviceOperation),
}

enum Action {
    Task(Arc<task_native::SessionTaskOwner>,crate::task_contract::TaskRequest),
    Wait(Arc<wait_native::SessionWaitOwner>,u64,ara_rpc::WireString,Option<CancellationToken>,Option<async_effect::NativeEffectPublisher>),
    Unavailable(&'static str),
    Read(
        Arc<ReadTool>,
        String,
        Option<async_effect::NativeEffectPublisher>,
    ),
    ReadAttachment(
        Arc<ReadTool>,
        String,
        Option<async_effect::NativeEffectPublisher>,
    ),
    ReadLocal(
        Arc<ReadTool>,
        Option<crate::mcp_native::local::LocalPlan>,
        String,
        Option<String>,
        Option<async_effect::NativeEffectPublisher>,
    ),
    ReadDelimited(Vec<String>, Option<async_effect::NativeEffectPublisher>),
    Write(
        Arc<WriteTool>,
        String,
        String,
        Option<Arc<dyn ara_tools::write::WriteProgress>>,
        ara_tools::ContentUriWriteReceipt,
    ),
    BashPreflight(
        Arc<BashTool>,
        String,
        Option<String>,
        Vec<(String, String)>,
        Arc<Mutex<Option<ara_tools::bash::BashPreparedOsValues>>>,
    ),
    Edit(Arc<EditTool>, EditCall, ara_edit::ApplyRequest),
    Grep(Arc<GrepTool>, GrepOsValues),
    Glob(Arc<GlobTool>, GlobOsValues),
    AstGrep(Arc<AstGrepTool>,AstGrepOsValues,Option<Arc<dyn ara_tools::ast_grep::AstGrepLanguagePort>>),
    AstEdit(
        Arc<AstEditTool>,
        ast_edit_native::AstFinishStep,
        ast_edit_native::ScopeReceiptSlot,
    ),
    AstScope(
        Arc<AstEditTool>,
        ast_edit_native::AstOsStep,
        ast_edit_native::ScopeReceiptSlot,
    ),
}
fn valid_env_name(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
fn output_graph(
    values: &SessionValues,
    output: ToolOutput,
) -> Result<ValueHandle, ActualProviderError> {
    // This is fresh OS-produced data. No original args/history is serialized.
    let mut raw = serde_json::json!({"content": output.content});
    if let Some(details) = output.details {
        raw["details"] = details;
    }
    if output.is_error {
        raw["isError"] = true.into();
    }
    if let Some(metadata) = output.provider_metadata {
        raw["providerMetadata"] = serde_json::to_value(metadata)
            .map_err(|_| ActualProviderError::Protocol("derived OS result metadata"))?;
    }
    if output.useless {
        raw["useless"] = true.into();
    }
    Ok(values.heap().import_serde(&raw)?)
}

impl ValueRuntime for InvocationRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        if let Some(reply) = self.owner.resolve_context_signal(&request) {
            return reply;
        }
        match request {
            RuntimeRequest::Call {
                capability,
                receiver,
                arguments,
            } => self.resolve_call(capability, receiver, arguments, effective),
            RuntimeRequest::Opaque {
                capability,
                target,
                operation: OpaqueOperation::Get(key),
                ..
            } if capability == self.signal => {
                if !target.same_identity(&self.arguments[2]) {
                    return Err(ValueError::ForeignHeap);
                }
                if self
                    .state
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .phase
                    == Phase::Settled
                {
                    return Err(ValueError::ExpiredCapability(capability));
                }
                if matches!(key, PropertyKey::String(name) if name.equals_ascii("aborted")) {
                    Ok(RuntimeReply::Value(
                        self.request
                            .values
                            .heap()
                            .boolean(self.cancel.is_cancelled())?,
                    ))
                } else {
                    Err(ValueError::UnsupportedOperation(
                        "actual AbortSignal member is not connected",
                    ))
                }
            }
            request => self.request.runtime().resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        if let (Some(locator), Some(parent)) = (&self.run, self.parent.get()) {
            let run = locator
                .upgrade()
                .ok_or(ValueError::ExpiredCapability(capability))?;
            if let Some(reply) = run
                .admission
                .resolve_parent_call(capability, receiver, arguments, *parent, effective)
            {
                return reply;
            }
        }
        if self.bash_completion_capability(capability) {
            return self
                .complete_bash_native(receiver, arguments, effective)
                .map(RuntimeReply::Value);
        }
        if self.ast_completion_capability(capability) {
            return self
                .complete_ast_native(receiver, arguments, effective)
                .map(RuntimeReply::Value);
        }
        if capability == self.completion {
            return self
                .process_native(receiver, arguments, effective)
                .map(RuntimeReply::Value);
        }
        let member = &self.owner.members[self.index];
        if capability == member.method {
            self.request.check_execution_ready()?;
            if !receiver.same_identity(&member.source)
                || arguments.len() != self.execution_arguments().len()
                || !arguments
                    .iter()
                    .zip(self.execution_arguments())
                    .all(|(a, b)| a.same_identity(&b))
            {
                return Err(ValueError::TypeError(
                    "Wrong original builtin receiver/arguments".into(),
                ));
            }
            {
                let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                if state.phase != Phase::AwaitingCall {
                    return Err(ValueError::ExpiredCapability(capability));
                }
                state.phase = Phase::Preparing;
            }
            if self.run.is_some() {
                return match self.start_native(effective) {
                    Ok(promise) => {
                        self.state.lock().unwrap_or_else(|e| e.into_inner()).phase = Phase::Running;
                        Ok(RuntimeReply::Value(promise))
                    }
                    Err(error) => {
                        let reply = match &error {
                            ActualProviderError::Value(error) => error.clone(),
                            _ => ValueError::UnsupportedOperation(
                                "original Native preparation failure retained",
                            ),
                        };
                        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                        state.call_failure = Some(error);
                        state.phase = Phase::Settled;
                        Err(reply)
                    }
                };
            }
            match self.start_legacy(effective) {
                Ok(future) => {
                    let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                    state.future = Some(future);
                    state.phase = Phase::Running;
                    Ok(RuntimeReply::Value(self.marker.clone()))
                }
                Err(error) => {
                    let reply = match &error {
                        ActualProviderError::Value(error) => error.clone(),
                        _ => ValueError::UnsupportedOperation(
                            "original builtin preparation error retained by invocation",
                        ),
                    };
                    let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
                    state.call_failure = Some(error);
                    state.phase = Phase::Settled;
                    Err(reply)
                }
            }
        } else if capability == self.update {
            if self
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .phase
                != Phase::Running
            {
                return Err(ValueError::ExpiredCapability(capability));
            }
            if !matches!(
                self.request.values.heap().snapshot(receiver)?,
                ValueSnapshot::Undefined
            ) || arguments.len() != 1
            {
                return Err(ValueError::TypeError(
                    "Wrong native builtin update receiver/arguments".into(),
                ));
            }
            self.request.values.check_handle(&arguments[0])?;
            let updated = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (self.request.on_update)(arguments[0].clone())
            }))
            .unwrap_or_else(|panic| {
                let message = panic
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("non-string panic payload");
                Err(ActualProviderError::Callback(format!(
                    "Builtin update callback panicked: {message}"
                )))
            });
            match updated {
                Ok(()) => Ok(RuntimeReply::Value(self.request.values.heap().undefined()?)),
                Err(error) => {
                    let reply = match &error {
                        ActualProviderError::Value(error) => error.clone(),
                        _ => ValueError::UnsupportedOperation(
                            "original builtin update error retained by invocation",
                        ),
                    };
                    self.remember_update(error);
                    Err(reply)
                }
            }
        } else {
            self.request
                .runtime()
                .resolve_call(capability, receiver, arguments, effective)
        }
    }
    fn resolve_is_constructor(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool, ValueError> {
        self.request
            .runtime()
            .resolve_is_constructor(capability, constructor, effective)
    }
    fn resolve_construct(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        self.request
            .runtime()
            .resolve_construct(capability, constructor, arguments, effective)
    }
    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        self.request.runtime().resolve_construct_with_new_target(
            capability,
            constructor,
            new_target,
            arguments,
            effective_runtime,
        )
    }

    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        self.request
            .runtime()
            .resolve_local_call(token, receiver, arguments, effective)
    }
}

#[cfg(test)]
mod edit_upgrade_tests;
