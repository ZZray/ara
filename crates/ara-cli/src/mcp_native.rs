//! Executable CLI adapter for source graph MCP methods and the real stdio owner.
//! No Graph-to-serde arguments or typed.execute fallback. Complete prepared
//! imports and this Run/driver must be retained together by the calling Host.

pub mod local;
use crate::{
    actual_tools::async_effect::{HostEffectId, HostEffectSettlementOwner, NativeToolFault},
    native_realm::{
        NativeErrorClasses,
        context::NativeScope,
        parent::{NativeParentId, NativeParentLease},
        promise::{NativeDrain, NativePendingPromise, NativePromiseAdmission, NativePromiseRun, NativePromiseState},
    },
};
use ara_agent::{
    ActualExecuteRequest, ActualProviderError, ActualToolBinding, ActualToolDecision, ActualToolInvoker,
    ActualToolSource, AgentTool, Concurrency, SessionValues, ToolError, ToolOutput, UpdateFn,
};
use ara_ai::{JsonObject, Tool};
use ara_mcp::{
    McpTool,
    native_dispatch::NativeMcpOutcome,
    native_graph::{self as graph, NativeMcpImports},
};
use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, PropertyKey, RuntimeReply,
        RuntimeRequest, ValueError, ValueHandle, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use async_trait::async_trait;
use futures::FutureExt;
use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ActualProviderError>;
type GraphResult<T> = std::result::Result<T, ValueError>;
static NEXT: AtomicU64 = AtomicU64::new(0x6000_0000_0000_0000);
fn capability() -> GraphResult<CapabilityId> {
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .map(CapabilityId)
        .map_err(|_| ValueError::HeapTokensExhausted)
}

struct Member {
    tool: Arc<McpTool>,
    source: ValueHandle,
    execute: ValueHandle,
    start: CapabilityId,
    start_port: ValueHandle,
    enabled: AtomicBool,
    run: Mutex<Option<Weak<NativeMcpRun>>>,
}
pub struct NativeMcpSource {
    values: Weak<SessionValues>,
    collection: ValueHandle,
    members: Vec<Arc<Member>>,
    capabilities: Vec<Arc<dyn AgentTool>>,
    imports: NativeMcpImports,
    routes: local::LocalRoutes,
    run_binding: Mutex<Option<Weak<NativeMcpRun>>>,
    device_snapshot: Mutex<Option<Weak<crate::native_factory::catalog::NativeCatalogSnapshot>>>,
    errors: NativeErrorClasses,
}
impl NativeMcpSource {
    pub(crate) fn bind_device_snapshot(&self,snapshot:&Arc<crate::native_factory::catalog::NativeCatalogSnapshot>,values:&SessionValues)->Result<()> {
        self.check(values)?;
        if !snapshot.devices_enabled(){return Ok(())}
        let mut binding=self.device_snapshot.lock().unwrap_or_else(|e|e.into_inner());
        if binding.as_ref().and_then(Weak::upgrade).is_some_and(|old|!Arc::ptr_eq(&old,snapshot)){
            return Err(ActualProviderError::Protocol("Original source is already bound to another retained catalog"))
        }
        *binding=Some(Arc::downgrade(snapshot));Ok(())
    }
    fn device_snapshot(&self,values:&SessionValues)->Result<Option<Arc<crate::native_factory::catalog::NativeCatalogSnapshot>>> {
        self.check(values)?;
        let binding=self.device_snapshot.lock().unwrap_or_else(|e|e.into_inner());
        match binding.as_ref(){Some(binding)=>binding.upgrade().map(Some).ok_or(ActualProviderError::Protocol("Bound original catalog expired")),None=>Ok(None)}
    }

    pub(crate) fn invoke_device(
        self: Arc<Self>,
        binding: ActualToolBinding,
        request: crate::actual_tools::xdev_native::NativeMethodRequest,
    ) -> futures::future::BoxFuture<'static, Result<ValueHandle>> {
        Box::pin(async move {
            let index = self
                .members
                .iter()
                .position(|member| member.source.same_identity(&binding.source))
                .ok_or(ActualProviderError::Protocol(
                    "Device MCP has no exact canonical member",
                ))?;
            if !Arc::ptr_eq(&self.capabilities[index], &binding.capability) {
                return Err(ActualProviderError::Protocol(
                    "Device MCP canonical capability differs",
                ));
            }
            McpInvoker {
                member: self.members[index].clone(),
            }
            .invoke_method(request)
            .await
        })
    }
    pub fn new(
        values: &Arc<SessionValues>,
        tools: Vec<Arc<McpTool>>,
        imports: NativeMcpImports,
        routes: local::LocalRoutes,
        errors: NativeErrorClasses,
    ) -> Result<Arc<Self>> {
        values.check_handle(&imports.environment)?;
        values.check_handle(&imports.format_failure)?;
        if !imports.format_failure.same_identity(&imports.errors.format_failure) {
            return Err(ActualProviderError::Protocol("MCP formatter differs from its evaluated fixed-source binding"));
        }
        for handle in [
            &imports.errors.is_record,
            &imports.errors.tool_abort_error,
            &imports.errors.throw_if_aborted,
            &imports.errors.transport_error,
            &imports.errors.normalize_transport_error,
            &imports.errors.create_json_rpc_error,
        ] {
            values.check_handle(handle)?;
        }
        for handle in [&routes.override_options, &routes.main_session_manager].into_iter().flatten() {
            values.check_handle(handle)?;
        }
        for handle in [&errors.error, &errors.type_error, &errors.syntax_error, &errors.range_error] {
            values.check_handle(handle)?;
        }
        let heap = values.heap();
        let mut members = Vec::new();
        for tool in tools {
            let source = heap.object()?;
            let start = capability()?;
            let start_port = heap.function(start)?;
            let execute = graph::execute_function(heap, &start_port)?;
            let definition = tool.protocol_definition();
            for (key, value) in [
                ("name", heap.string(definition.name.into())?),
                ("description", heap.string(definition.description.into())?),
                ("parameters", heap.import_serde(&definition.parameters)?),
                ("execute", execute.clone()),
                ("serverName", heap.string(tool.server_name().into())?),
                ("mcpToolName", heap.string(tool.original_tool_name().into())?),
            ] {
                heap.define_own_data(&source, key.into(), value, values.runtime())?;
            }
            members.push(Arc::new(Member {
                tool,
                source,
                execute,
                start,
                start_port,
                enabled: AtomicBool::new(true),
                run: Mutex::new(None),
            }));
        }
        let collection = heap.array(members.iter().map(|member| member.source.clone()).collect())?;
        let capabilities = members
            .iter()
            .map(|member| { Arc::new(NativeMcpTool { member: member.clone(), }) as Arc<dyn AgentTool>
            })
            .collect();
        Ok(Arc::new(Self {
            values: Arc::downgrade(values),
            collection,
            members,
            capabilities,
            imports,
            routes,
            device_snapshot: Mutex::new(None),
            run_binding: Mutex::new(None),
            errors,
        }))
    }
    fn check(&self, values: &SessionValues) -> Result<()> {
        let original = self.values.upgrade().ok_or(ActualProviderError::Protocol("MCP original Session expired"))?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ForeignHeap.into());
        }
        Ok(())
    }
    pub fn permission(&self, binding: &ActualToolBinding, values: &SessionValues) -> Result<ActualToolDecision> {
        self.check(values)?;
        values.check_handle(&binding.source)?;
        let index = self
            .members
            .iter()
            .zip(&self.capabilities)
            .position(|(member, capability)| {
                member.source.same_identity(&binding.source) && Arc::ptr_eq(capability, &binding.capability)
            })
            .ok_or(ActualProviderError::Protocol("MCP permission source/capability owner differs"))?;
        Ok(if self.members[index].enabled.load(Ordering::Acquire) {
            ActualToolDecision::Allow
        } else {
            ActualToolDecision::Block(Some("Tool execution was blocked by the current Host grant".into()))
        })
    }
    /// Revocation is checked again immediately before each native effect entry.
    pub fn set_enabled(&self, index: usize, enabled: bool) -> Result<()> {
        self.members
            .get(index)
            .ok_or(ActualProviderError::Protocol("MCP grant index"))?
            .enabled
            .store(enabled, Ordering::Release);
        Ok(())
    }
    pub fn bind_run(&self, run: &Arc<NativeMcpRun>) -> Result<()> {
        let values = self.values.upgrade().ok_or(ActualProviderError::Protocol("MCP original Session expired"))?;
        run.admission.check_values(&values)?;
        let mut binding = self.run_binding.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = binding.as_ref().and_then(Weak::upgrade) {
            if Arc::ptr_eq(&old, run) {
                return Ok(());
            }
            if old.admission.check_live().is_ok() {
                return Err(ActualProviderError::Protocol("MCP source already has a live Run"));
            }
        }
        for member in &self.members {
            let mut current = member.run.lock().unwrap_or_else(|e| e.into_inner());
            *current = Some(Arc::downgrade(run));
        }
        *binding = Some(Arc::downgrade(run));
        Ok(())
    }
}
impl ActualToolSource for NativeMcpSource {
    fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>> {
        self.check(values)?;
        Ok(Some(self.collection.clone()))
    }
    fn bind_selected(&self, selected: &ValueHandle, values: &SessionValues) -> Result<ActualToolBinding> {
        self.check(values)?;
        values.check_handle(selected)?;
        let index = self
            .members
            .iter()
            .position(|member| member.source.same_identity(selected))
            .ok_or(ActualProviderError::Protocol("MCP selected source has no exact owner"))?;
        Ok(ActualToolBinding {
            source: self.members[index].source.clone(),
            capability: self.capabilities[index].clone(),
        })
    }
    fn has_fallback(&self) -> bool {
        false
    }
}
struct NativeMcpTool {
    member: Arc<Member>,
}
#[async_trait]
impl AgentTool for NativeMcpTool {
    fn definition(&self) -> Tool {
        self.member.tool.definition()
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }
    fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
        Some(Arc::new(McpInvoker { member: self.member.clone() }))
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> std::result::Result<ToolOutput, ToolError> {
        self.member.tool.execute(id, args, cancel, update).await
    }
}

enum Payload {
    Remote(NativeMcpOutcome),
    Local(std::result::Result<Option<(String, u64)>, String>),
}
struct EffectJob {
    id: HostEffectId,
    payload: Arc<Mutex<Option<Payload>>>,
    resolve: ValueHandle,
    reject: ValueHandle,
    scope: NativeScope,
    parent: NativeParentId,
    _lease: NativeParentLease,
}
pub struct NativeMcpRun {
    source: Arc<NativeMcpSource>,
    admission: NativePromiseAdmission,
    effects: Arc<HostEffectSettlementOwner>,
    jobs: Mutex<Vec<EffectJob>>,
    revoked: AtomicBool,
    sealed: AtomicBool,
}
impl NativeMcpRun {
    pub fn new(source: Arc<NativeMcpSource>, admission: NativePromiseAdmission) -> Result<Arc<Self>> {
        let values = source.values.upgrade().ok_or(ActualProviderError::Protocol("MCP original Session expired"))?;
        admission.check_values(&values)?;
        let run = Arc::new(Self {
            source,
            admission,
            effects: Arc::new(HostEffectSettlementOwner::new()),
            jobs: Mutex::new(Vec::new()),
            revoked: AtomicBool::new(false),
            sealed: AtomicBool::new(false),
        });
        run.source.bind_run(&run)?;
        Ok(run)
    }
    pub fn effects(&self) -> Arc<HostEffectSettlementOwner> {
        self.effects.clone()
    }
    pub fn wake(&self) -> Arc<tokio::sync::Notify> {
        self.effects.wake()
    }
    pub fn seal(&self) -> Result<()> {
        self.sealed.store(true, Ordering::Release);
        self.effects.seal();
        self.admission.wake().notify_waiters();
        Ok(())
    }
    pub fn revoke_graph(&self) {
        self.revoked.store(true, Ordering::Release);
        self.sealed.store(true, Ordering::Release);
        self.effects.seal();
        self.admission.wake().notify_waiters();
    }
    pub fn revoke(&self) {
        self.revoke_graph();
        self.effects.cancel();
    }
    fn check_entry(&self) -> GraphResult<()> {
        self.check_live()?;
        if self.sealed.load(Ordering::Acquire) {
            return Err(ValueError::UnsupportedOperation("MCP native Run admission is sealed"));
        }
        Ok(())
    }
    fn check_live(&self) -> GraphResult<()> {
        self.admission.check_live()?;
        if self.revoked.load(Ordering::Acquire) {
            return Err(ValueError::UnsupportedOperation("MCP native Run is revoked"));
        }
        Ok(())
    }
    /// Only the original graph driver calls this method. Foreign completions
    /// remain in their native owner; imported JSON is fresh remote data only.
    pub fn drain_one(&self) -> Result<bool> {
        let ready = {
            let jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            jobs.iter().find_map(|job| { self.effects.take_completion_for(job.id).map(|notice| (job.id, notice))
            })
        };
        let Some((id, notice)) = ready else {
            return Ok(false);
        };
        let job = {
            let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner());
            let index = jobs
                .iter()
                .position(|job| job.id == id)
                .ok_or(ActualProviderError::Protocol("MCP completion has no original graph job"))?;
            jobs.remove(index)
        };
        self.check_live()?;
        let values =
            self.source.values.upgrade().ok_or(ActualProviderError::Protocol("MCP original Session expired"))?;
        let heap = values.heap();
        let rt = self.admission.scoped(job.scope, job.parent)?;
        let payload = job.payload.lock().unwrap_or_else(|e| e.into_inner()).take();
        let (resolver, value) = match payload {
            Some(Payload::Remote(NativeMcpOutcome::Complete(value))) => { (&job.resolve, heap.import_serde(&value)?)
            }
            Some(Payload::Remote(NativeMcpOutcome::Aborted { .. })) =>
                (&job.reject, error(&values, &self.source.imports, "Aborted", true, &rt)?,),
            Some(Payload::Remote(
                NativeMcpOutcome::UnknownEffect(message) | NativeMcpOutcome::NotDispatched(message),
            )) => (&job.reject, error(&values, &self.source.imports, &message, false, &rt)?),
            Some(Payload::Local(Ok(Some((path, size))))) => {
                let file = heap.object()?;
                heap.define_own_data(&file, "path".into(), heap.string(path.into())?, &rt)?;
                heap.define_own_data(&file, "size".into(), heap.number(size as f64)?, &rt)?;
                (&job.resolve, file)
            }
            Some(Payload::Local(Ok(None))) => (&job.resolve, heap.null()?),
            Some(Payload::Local(Err(message))) =>
                (&job.reject, error(&values, &self.source.imports, &message, false, &rt)?,),
            None => {
                let fault = notice
                    .err()
                    .ok_or(ActualProviderError::Protocol("MCP native worker completed without its payload"))?;
                return Err(ActualProviderError::Callback(format!(
                    "MCP native effect failed before a payload receipt: {fault:?}"
                )));
            }
        };
        heap.call(resolver, &heap.undefined()?, &[value], &rt)?;
        Ok(true)
    }
    fn remote(
        &self,
        invocation: &Invocation,
        args: &ValueHandle,
        rt: &dyn ValueRuntime,
    ) -> GraphResult<NativePendingPromise> {
        self.check_entry()?;
        if !invocation.member.enabled.load(Ordering::Acquire) {
            return Err(ValueError::TypeError("MCP original effect grant was revoked".into()));
        }
        let request = invocation.member.tool.prepare_native_request(|id, name| {
            graph::rpc_frame(invocation.request.values.heap(), &self.source.imports, id, name, args, rt)
        })?;
        self.check_entry()?;
        if !invocation.member.enabled.load(Ordering::Acquire) {
            return Err(ValueError::TypeError("MCP original effect grant was revoked during stringify".into()));
        }
        let pending = self.admission.prepare_pending(invocation.scope, invocation.parent()?)?;
        let reservation = self.effects.reserve().map_err(fault)?;
        let id = reservation.id();
        let payload = Arc::new(Mutex::new(None));
        let worker_payload = payload.clone();
        // Private native transport knowledge, tied to this exact entered id.
        // No caller Graph detail or rendered error text grants Unknown status.
        let outcome_unknown = Arc::new(AtomicBool::new(false));
        let worker_unknown = outcome_unknown.clone();
        let cancel = invocation.request.cancel.child_token();
        let worker_cancel = cancel.clone();
        let parent = invocation.parent()?;
        let lease = self.admission.retain_parent(parent)?;
        reservation
            .enter_authoritative(
                cancel,
                async move {
                    let outcome = request.dispatch(worker_cancel).await;
                    worker_unknown.store(outcome.is_unknown(), Ordering::Release);
                    let status = match &outcome {
                        NativeMcpOutcome::Complete(value) => {
                            let mut status = ToolOutput::text("");
                            status.is_error = value.get("isError").is_some_and(json_truthy);
                            Ok(status)
                        }
                        NativeMcpOutcome::NotDispatched(message) => {
                            Err(NativeToolFault::NotDispatched(message.clone()))
                        }
                        NativeMcpOutcome::UnknownEffect(message) => {
                            Err(NativeToolFault::OutcomeUnknown(message.clone()))
                        }
                        NativeMcpOutcome::Aborted { published: true } => {
                            Err(NativeToolFault::OutcomeUnknown("MCP call aborted after publication".into()))
                        }
                        NativeMcpOutcome::Aborted { published: false } => {
                            Err(NativeToolFault::NotDispatched("MCP cancellation before publication".into()))
                        }
                    };
                    *worker_payload.lock().unwrap_or_else(|e| e.into_inner()) = Some(Payload::Remote(outcome));
                    status
                }
                .boxed(),
            )
            .map_err(fault)?;
        *invocation.remote_effect.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(RemoteEffect { id, outcome_unknown });
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).push(EffectJob {
            id,
            payload,
            resolve: pending.resolve.clone(),
            reject: pending.reject.clone(),
            scope: invocation.scope,
            parent,
            _lease: lease,
        });
        Ok(pending)
    }
    fn local(
        &self,
        invocation: &Invocation,
        input: String,
        context: &ValueHandle,
        rt: &dyn ValueRuntime,
    ) -> GraphResult<NativePendingPromise> {
        self.check_entry()?;
        let pending = self.admission.prepare_pending(invocation.scope, invocation.parent()?)?;
        let plan = local::prepare(invocation.request.values.heap(), context, &self.source.routes, input, rt)?;
        self.check_entry()?;
        let Some(plan) = plan else {
            invocation.resolve(&pending, invocation.request.values.heap().null()?, rt)?;
            return Ok(pending);
        };
        if !invocation.member.enabled.load(Ordering::Acquire) {
            return Err(ValueError::TypeError("MCP original effect grant was revoked".into()));
        }
        let reservation = self.effects.reserve().map_err(fault)?;
        let id = reservation.id();
        let payload = Arc::new(Mutex::new(None));
        let worker_payload = payload.clone();
        let parent = invocation.parent()?;
        let lease = self.admission.retain_parent(parent)?;
        reservation
            .enter(
                invocation.request.cancel.child_token(),
                async move {
                    let outcome = local::resolve(plan).await;
                    let status = outcome
                        .as_ref()
                        .map(|_| ToolOutput::text(""))
                        .map_err(|message| NativeToolFault::Kernel(message.clone()));
                    *worker_payload.lock().unwrap_or_else(|e| e.into_inner()) = Some(Payload::Local(outcome));
                    status
                }
                .boxed(),
            )
            .map_err(fault)?;
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).push(EffectJob {
            id,
            payload,
            resolve: pending.resolve.clone(),
            reject: pending.reject.clone(),
            scope: invocation.scope,
            parent,
            _lease: lease,
        });
        Ok(pending)
    }
}
fn fault(fault: NativeToolFault) -> ValueError {
    ValueError::TypeError(format!("MCP native effect admission: {fault:?}"))
}
fn json_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|number| number != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        _ => true,
    }
}
fn error(
    values: &SessionValues,
    imports: &NativeMcpImports,
    message: &str,
    abort: bool,
    rt: &dyn ValueRuntime,
) -> GraphResult<ValueHandle> {
    let heap = values.heap();
    let ctor = if abort {
        imports.errors.tool_abort_error.clone()
    } else {
        graph::get(heap, &imports.environment, "Error", rt)?
    };
    if abort { heap.construct(&ctor, &[], rt) } else { heap.construct(&ctor, &[heap.string(message.into())?], rt) }
}

struct McpInvoker {
    member: Arc<Member>,
}
#[async_trait]
impl ActualToolInvoker for McpInvoker {
    async fn invoke_settled(&self, request: ActualExecuteRequest) -> Result<ValueHandle> {
        self.invoke_method(crate::actual_tools::xdev_native::NativeMethodRequest::root(
            request,
        ))
        .await
    }
}
impl McpInvoker {
    async fn invoke_method(
        &self,
        request: crate::actual_tools::xdev_native::NativeMethodRequest,
    ) -> Result<ValueHandle> {
        request.check_admission()?;
        if !request.receiver.same_identity(&self.member.source) || !request.callable.same_identity(&self.member.execute)
        {
            return Err(ActualProviderError::Protocol("MCP callable/receiver owner differs"));
        }
        let run = self
            .member
            .run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol("MCP original Run is absent"))?;
        run.admission.check_values(&request.values)?;
        run.check_entry()?;
        let scope = request.scope(& run.admission)?;
        let port_id = capability()?;
        let heap = request.values.heap().clone();
        let invocation = Arc::new(Invocation {
            request: Arc::new(request),
            member: self.member.clone(),
            run: Arc::downgrade(&run),
            scope,
            parent: OnceLock::new(),
            port_id,
            port: heap.function(port_id)?,
            signal_cap: OnceLock::new(),
            result: Mutex::new(None),
            remote_effect: Mutex::new(None),
            continuations: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(1),
            started: AtomicBool::new(false),
        });
        let original: Arc<dyn ValueRuntime> = invocation.clone();
        let parent = run.admission.register_parent(original)?;
        invocation.parent.set(parent).map_err(|_| ActualProviderError::Protocol("MCP parent already set"))?;
        struct Registration {
            admission: NativePromiseAdmission,
            parent: NativeParentId,
        }
        impl Drop for Registration {
            fn drop(&mut self) {
                let _ = self.admission.release_parent_registration(self.parent);
            }
        }
        let _registration = Registration { admission: run.admission.clone(), parent };
        let rt = run.admission.scoped(scope, parent)?;
        crate::actual_tools::xdev_native::approve_root_method(
            run.source.device_snapshot(&invocation.request.values)?,&invocation.request,
            Arc::new(run.admission.scoped(invocation.scope,parent)?),invocation.scope,parent,
        ).await?;
        invocation.request.check_admission()?;
        run.admission.scoped(invocation.scope,parent)?;
        run.check_entry()?;

        let signal = heap.object()?;
        let aborted = capability()?;
        invocation.signal_cap.set(aborted).map_err(|_| ActualProviderError::Protocol("MCP signal already set"))?;
        heap.define_own(
            &signal,
            "aborted".into(),
            PropertyDescriptor::Accessor {
                get: Some(ara_rpc::value_heap::CallableTarget::Host(aborted)),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &rt,
        )?;
        let signal = invocation.request.signal().cloned().unwrap_or(signal);
        let args = [
            heap.string(invocation.request.call_id.clone())?,
            invocation.request.execution_args().clone(),
            heap.undefined()?,
            invocation.request.tool_context.clone(),
            signal,
        ];
        // Exactly one Call of the selected original method, with MCP's own
        // argument order, using its registered original InvocationRuntime.
        let called = heap.call(&invocation.request.callable, &invocation.request.receiver, &args, &rt)?;
        drop(rt);
        let result = invocation
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(ActualProviderError::Protocol("MCP execute returned no original Promise"))?;
        if !called.same_identity(&result.promise) {
            return Err(ActualProviderError::Protocol("MCP original execute returned a different Promise"));
        }
        loop {
            let wake = run.admission.wake();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            run.check_live()?;
            match run.admission.ticket_state(&result.ticket())? {
                NativePromiseState::Fulfilled(value) => {
                    let output = heap.restore_deferred(&value)?;
                    let remote = invocation.remote_effect.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    if let Some(remote) = remote {
                        // complete_with_receipt publishes the native receipt
                        // before drain_one resolves/rejects the Graph Promise.
                        // Join only this id's retained monitor, never the driver
                        // or unrelated effects, before exposing Host knowledge.
                        let receipt = run.effects.settle_one(remote.id).await.ok_or(ActualProviderError::Protocol(
                            "MCP original result has no exact native effect receipt",
                        ))?;
                        run.check_live()?;
                        // Complete (including isError) is authoritative remote
                        // knowledge. Cancellation-derived uncertainty alone
                        // cannot relabel a received Complete as Unknown.
                        if remote.outcome_unknown.load(Ordering::Acquire) {
                            if !receipt.uncertain() {
                                return Err(ActualProviderError::Protocol(
                                    "MCP Unknown transport outcome lost its native receipt",
                                ));
                            }
                            let rt = run.admission.scoped(scope, parent)?;
                            let details = graph::get(&heap, &output, "details", &rt)?;
                            for (key, value) in [
                                ("__synthetic", heap.boolean(true)?),
                                ("source", heap.string("interrupted_unknown_effect".into())?),
                                ("executed", heap.string("unknown".into())?),
                                ("transport", heap.string("mcp_stdio".into())?),
                            ] {
                                heap.define_own_data(&details, key.into(), value, &rt)?;
                            }
                            // Retain fixed formatMCPToolFailure and its source
                            // fields; append explicit Host recovery knowledge.
                            let block = heap.object()?;
                            heap.define_own_data(&block, "type".into(), heap.string("text".into())?, &rt)?;
                            heap.define_own_data(
                                &block, "text".into(),
                                heap.string("MCP call effect unknown; do not replay automatically. Inspect affected state before retrying.".into())?,
                                &rt,
                            )?;
                            let content = graph::get(&heap, &output, "content", &rt)?;
                            graph::method(&heap, &content, "push", &[block], &rt)?;
                        }
                    }
                    return Ok(output);
                }
                NativePromiseState::Rejected(value) => {
                    return Err(ValueError::Thrown(heap.restore_deferred(&value)?).into());
                }
                NativePromiseState::Pending => notified.await,
            }
        }
    }
}

struct Outbound {
    input: ValueHandle,
    context: ValueHandle,
    seen: ValueHandle,
    pending: Arc<NativePendingPromise>,
    resolved: Option<ValueHandle>,
    array: bool,
    index: u32,
    keys: Vec<PropertyKey>,
    key_index: usize,
}
enum Continuation {
    Forward(Arc<NativePendingPromise>),
    OutboundChild { frame: Arc<Mutex<Outbound>>, key: PropertyKey, original: ValueHandle },
    LocalPath { original: ValueHandle, pending: Arc<NativePendingPromise> },
    ExecutePrepared,
    ExecuteRemote { details_source: ValueHandle },
}
struct Invocation {
    request: Arc<crate::actual_tools::xdev_native::NativeMethodRequest>,
    member: Arc<Member>,
    run: Weak<NativeMcpRun>,
    scope: NativeScope,
    parent: OnceLock<NativeParentId>,
    port_id: CapabilityId,
    port: ValueHandle,
    signal_cap: OnceLock<CapabilityId>,
    result: Mutex<Option<Arc<NativePendingPromise>>>,
    remote_effect: Mutex<Option<RemoteEffect>>,
    continuations: Mutex<HashMap<u64, Continuation>>,
    sequence: AtomicU64,
    started: AtomicBool,
}
#[derive(Clone)]
struct RemoteEffect {
    id: HostEffectId,
    outcome_unknown: Arc<AtomicBool>,
}
impl Invocation {
    fn run(&self) -> GraphResult<Arc<NativeMcpRun>> {
        self.run.upgrade().ok_or(ValueError::ExpiredCapability(self.port_id))
    }
    fn parent(&self) -> GraphResult<NativeParentId> {
        self.parent.get().copied().ok_or(ValueError::ExpiredCapability(self.port_id))
    }
    fn pending(&self) -> GraphResult<Arc<NativePendingPromise>> {
        Ok(Arc::new(self.run()?.admission.prepare_pending(self.scope, self.parent()?)?))
    }
    fn resolve(&self, pending: &NativePendingPromise, value: ValueHandle, rt: &dyn ValueRuntime) -> GraphResult<()> {
        self.request.values.heap().call(&pending.resolve, &self.request.values.heap().undefined()?, &[value], rt)?;
        Ok(())
    }
    fn reject(&self, pending: &NativePendingPromise, reason: ValueHandle, rt: &dyn ValueRuntime) -> GraphResult<()> {
        self.request.values.heap().call(&pending.reject, &self.request.values.heap().undefined()?, &[reason], rt)?;
        Ok(())
    }
    fn reason(&self, failure: ValueError, rt: &dyn ValueRuntime) -> GraphResult<ValueHandle> {
        match self.run()?.source.errors.create_at_native_origin(failure, rt) {
            ValueError::Thrown(reason) => Ok(reason),
            other => Err(other),
        }
    }
    fn attach(&self, pending: &NativePendingPromise, continuation: Continuation) -> GraphResult<()> {
        let id = self.sequence.fetch_add(1, Ordering::Relaxed);
        if id > 9_007_199_254_740_991 {
            return Err(ValueError::HeapTokensExhausted);
        }
        let heap = self.request.values.heap();
        let captures = [self.port.clone(), heap.number(id as f64)?];
        let ok = heap.fresh_compiled_function(&AWAIT_OK, &captures)?;
        let reject = heap.fresh_compiled_function(&AWAIT_ERR, &captures)?;
        self.continuations.lock().unwrap_or_else(|e| e.into_inner()).insert(id, continuation);
        self.run()?.admission.attach_native_await(&pending.ticket(), &ok, &reject, self.scope, self.parent()?)
    }
    fn start(&self, arguments: &[ValueHandle], rt: &dyn ValueRuntime) -> GraphResult<ValueHandle> {
        self.request.check_execution_ready()?;
        if arguments.len() != 4
            || !arguments[0].same_identity(&self.member.source)
            || !arguments[1].same_identity(self.request.execution_args())
            || !arguments[2].same_identity(&self.request.tool_context)
            || self.started.swap(true, Ordering::AcqRel)
        {
            return Err(ValueError::ExpiredCapability(self.member.start));
        }
        let result = self.pending()?;
        *self.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(result.clone());
        let operation = (|| {
            let heap = self.request.values.heap();
            let run = self.run()?;
            heap.call(&run.source.imports.errors.throw_if_aborted, &heap.undefined()?, &[arguments[3].clone()], rt)?;
            let schema = graph::get(heap, &self.member.source, "parameters", rt)?;
            let params = graph::normalize_prepare(heap, &run.source.imports, self.request.execution_args(), &schema, rt)?;
            let seen_ctor = graph::get(heap, &run.source.imports.environment, "WeakSet", rt)?;
            let seen = heap.construct(&seen_ctor, &[], rt)?;
            let outbound = self.outbound(params, self.request.tool_context.clone(), seen, rt)?;
            let prepared = self.pending()?;
            self.attach(&outbound, Continuation::Forward(prepared.clone()))?;
            self.attach(&prepared, Continuation::ExecutePrepared)
        })();
        if let Err(error) = operation {
            self.reject(&result, self.reason(error, rt)?, rt)?;
        }
        Ok(result.promise.clone())
    }
    fn outbound(
        &self,
        input: ValueHandle,
        context: ValueHandle,
        seen: ValueHandle,
        rt: &dyn ValueRuntime,
    ) -> GraphResult<Arc<NativePendingPromise>> {
        let heap = self.request.values.heap();
        let run = self.run()?;
        let pending = self.pending()?;
        let operation = (|| {
            if let ValueSnapshot::String(value) = heap.snapshot(&input)? {
                let mut normalized = value.clone();
                let prefix: Vec<u16> = "local:/".encode_utf16().collect();
                if value.units().starts_with(&prefix) && value.units().get(prefix.len()) != Some(&u16::from(b'/')) {
                    let mut units = prefix;
                    units.push(u16::from(b'/'));
                    units.extend_from_slice(&value.units()[7..]);
                    normalized = WireString::from_units(units);
                }
                if normalized.units().starts_with(&"local://".encode_utf16().collect::<Vec<_>>()) {
                    let input_string = normalized
                        .to_utf8()
                        .map_err(|_| { ValueError::TypeError("local:// path has unpaired UTF16 units".into())
                    })?;
                    let local = run.local(self, input_string, &context, rt)?;
                    self.attach(&local, Continuation::LocalPath { original: input.clone(), pending: pending.clone() })?;
                } else {
                    self.resolve(&pending, input.clone(), rt)?;
                }
                return Ok(());
            }
            if !matches!(heap.kind(&input)?, ValueKind::Object | ValueKind::Array) {
                self.resolve(&pending, input.clone(), rt)?;
                return Ok(());
            }
            if heap.truthy(&graph::method(heap, &seen, "has", &[input.clone()], rt)?)? {
                self.resolve(&pending, input.clone(), rt)?;
                return Ok(());
            }
            graph::method(heap, &seen, "add", &[input.clone()], rt)?;
            let array = heap.truthy(&graph::global_method(
                heap,
                &run.source.imports,
                "Array",
                "isArray",
                &[input.clone()],
                rt,
            )?)?;
            let keys = if array {
                Vec::new()
            } else {
                heap.for_in_string_keys(&input, rt)?.into_iter().map(PropertyKey::String).collect()
            };
            let frame = Arc::new(Mutex::new(Outbound {
                input: input.clone(),
                context,
                seen,
                pending: pending.clone(),
                resolved: None,
                array,
                index: 0,
                keys,
                key_index: 0,
            }));
            self.advance(frame, rt)
        })();
        if let Err(error) = operation {
            self.reject(&pending, self.reason(error, rt)?, rt)?;
        }
        Ok(pending)
    }
    fn advance(&self, frame: Arc<Mutex<Outbound>>, rt: &dyn ValueRuntime) -> GraphResult<()> {
        // Locks never span a graph Get/Call. Accessors may re-enter this owner.
        let (input, context, seen, pending, resolved, array, index, keys, key_index) = {
            let frame = frame.lock().unwrap_or_else(|e| e.into_inner());
            (
                frame.input.clone(),
                frame.context.clone(),
                frame.seen.clone(),
                frame.pending.clone(),
                frame.resolved.clone(),
                frame.array,
                frame.index,
                frame.keys.clone(),
                frame.key_index,
            )
        };
        let heap = self.request.values.heap();
        let key = if array {
            let length = graph::get(heap, &input, "length", rt)?;
            if f64::from(index) < heap.to_number(&length, rt)? {
                Some(PropertyKey::String(index.to_string().into()))
            } else {
                None
            }
        } else {
            keys.get(key_index).cloned()
        };
        let Some(key) = key else {
            return self.resolve(&pending, resolved.unwrap_or(input), rt);
        };
        let original = heap.get(&input, &key, rt)?;
        let child = self.outbound(original.clone(), context, seen, rt)?;
        self.attach(&child, Continuation::OutboundChild { frame, key, original })
    }
    fn resume(&self, args: &[ValueHandle], rt: &dyn ValueRuntime) -> GraphResult<ValueHandle> {
        let heap = self.request.values.heap();
        if args.len() != 3 {
            return Err(ValueError::ExpiredCapability(self.port_id));
        }
        let ValueSnapshot::Number(id) = heap.snapshot(&args[0])? else {
            return Err(ValueError::ExpiredCapability(self.port_id));
        };
        let failed = heap.truthy(&args[1])?;
        let value = &args[2];
        let continuation = self
            .continuations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(id as u64))
            .ok_or(ValueError::ExpiredCapability(self.port_id))?;
        let target = match &continuation {
            Continuation::Forward(pending) | Continuation::LocalPath { pending, .. } => { pending.clone()
            }
            Continuation::OutboundChild { frame, .. } =>
                frame.lock().unwrap_or_else(|e| e.into_inner()).pending.clone(),
            Continuation::ExecutePrepared | Continuation::ExecuteRemote { .. } => self
                .result
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .ok_or(ValueError::ExpiredCapability(self.port_id))?,
        };
        let operation: GraphResult<()> = (|| {
            match continuation {
                Continuation::Forward(pending) => {
                    if failed {
                        self.reject(&pending, value.clone(), rt)?;
                    } else {
                        self.resolve(&pending, value.clone(), rt)?;
                    }
                }
                Continuation::LocalPath { original, pending } => {
                    if failed {
                        self.reject(&pending, value.clone(), rt)?;
                    } else {
                        let path = if matches!(heap.kind(value)?, ValueKind::Null | ValueKind::Undefined) {
                            original.clone()
                        } else {
                            graph::get(heap, value, "path", rt)?
                        };
                        let path = if matches!(heap.kind(&path)?, ValueKind::Null | ValueKind::Undefined) {
                            original
                        } else {
                            path
                        };
                        self.resolve(&pending, path, rt)?;
                    }
                }
                Continuation::OutboundChild { frame, key, original } => {
                    let (input, existing, pending, array) = {
                        let state = frame.lock().unwrap_or_else(|e| e.into_inner());
                        (state.input.clone(), state.resolved.clone(), state.pending.clone(), state.array)
                    };
                    if failed {
                        self.reject(&pending, value.clone(), rt)?;
                    } else {
                        let operation = (|| {
                            let resolved = if graph::strict_equal(heap, value, &original)? && existing.is_none() {
                                None
                            } else {
                                let resolved = if let Some(existing) = existing {
                                    existing
                                } else if array {
                                    graph::method(heap, &input, "slice", &[], rt)?
                                } else {
                                    heap.spread(&input, rt)?
                                };
                                heap.assign(&resolved, key, value.clone(), rt)?;
                                Some(resolved)
                            };
                            {
                                let mut state = frame.lock().unwrap_or_else(|e| e.into_inner());
                                state.resolved = resolved;
                                if array {
                                    state.index = state.index.checked_add(1).ok_or(ValueError::NodeIdsExhausted)?;
                                } else {
                                    state.key_index += 1;
                                }
                            }
                            self.advance(frame, rt)
                        })();
                        if let Err(error) = operation {
                            self.reject(&pending, self.reason(error, rt)?, rt)?;
                        }
                    }
                }
                Continuation::ExecutePrepared => {
                    let result = self
                        .result
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                        .ok_or(ValueError::ExpiredCapability(self.port_id))?;
                    if failed {
                        self.reject(&result, value.clone(), rt)?;
                    } else {
                        let run = self.run()?;
                        // provider/providerName are original locals read only after
                        // preparation, before the tools/call network attempt.
                        let details_source = heap.object()?;
                        for key in ["serverName", "mcpToolName", "provider", "providerName"] {
                            heap.define_own_data(
                                &details_source,
                                key.into(),
                                graph::get(heap, &self.member.source, key, rt)?,
                                rt,
                            )?;
                        }
                        match run.remote(self, value, rt) {
                            Ok(remote) => self.attach(&remote, Continuation::ExecuteRemote { details_source })?,
                            Err(failure) => {
                                let reason = self.reason(failure, rt)?;
                                if let Some(abort) = self.abort_reason(&reason, rt)? {
                                    self.reject(&result, abort, rt)?;
                                } else {
                                    self.resolve(
                                        &result,
                                        graph::build_error(heap, &run.source.imports, &reason, &details_source, rt)?,
                                        rt,
                                    )?;
                                }
                            }
                        }
                    }
                }
                Continuation::ExecuteRemote { details_source } => {
                    let run = self.run()?;
                    let result = self
                        .result
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .clone()
                        .ok_or(ValueError::ExpiredCapability(self.port_id))?;
                    if failed && let Some(abort) = self.abort_reason(value, rt)? {
                        self.reject(&result, abort, rt)?;
                    } else {
                        let output = if failed {
                            graph::build_error(heap, &run.source.imports, value, &details_source, rt)
                        } else if !heap.truthy(value)? {
                            let reason = error(
                                &self.request.values,
                                &run.source.imports,
                                "MCP tool call returned no result",
                                false,
                                rt,
                            )?;
                            graph::build_error(heap, &run.source.imports, &reason, &details_source, rt)
                        } else {
                            graph::build_result(heap, &run.source.imports, value, &details_source, rt)
                        };
                        match output {
                            Ok(output) => self.resolve(&result, output, rt)?,
                            Err(failure) => {
                                let reason = self.reason(failure, rt)?;
                                if let Some(abort) = self.abort_reason(&reason, rt)? {
                                    self.reject(&result, abort, rt)?;
                                } else {
                                    let error_result =
                                        graph::build_error(heap, &run.source.imports, &reason, &details_source, rt)?;
                                    self.resolve(&result, error_result, rt)?;
                                }
                            }
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(failure) = operation {
            self.reject(&target, self.reason(failure, rt)?, rt)?;
        }
        heap.undefined()
    }
    fn abort_reason(&self, reason: &ValueHandle, rt: &dyn ValueRuntime) -> GraphResult<Option<ValueHandle>> {
        let run = self.run()?;
        let heap = self.request.values.heap();
        let tool_abort = run.source.imports.errors.tool_abort_error.clone();
        if heap.instance_of(reason, &tool_abort, rt)? {
            return Ok(Some(reason.clone()));
        }
        let error_ctor = graph::get(heap, &run.source.imports.environment, "Error", rt)?;
        if heap.instance_of(reason, &error_ctor, rt)? {
            let name = graph::get(heap, reason, "name", rt)?;
            if matches!(heap.snapshot(&name)?, ValueSnapshot::String(name) if name.equals_ascii("AbortError")) {
                return heap.construct(&tool_abort, &[], rt).map(Some);
            }
        }
        if self.request.cancel.is_cancelled() {
            return heap.construct(&tool_abort, &[], rt).map(Some);
        }
        Ok(None)
    }
}
impl ValueRuntime for Invocation {
    fn resolve(&self, request: RuntimeRequest<'_>) -> GraphResult<RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> GraphResult<RuntimeReply> {
        match request {
            RuntimeRequest::Call { capability, receiver, arguments, } =>
                self.resolve_call(capability, receiver, arguments, effective),
            request => self.request.runtime().resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> GraphResult<RuntimeReply> {
        if capability == self.member.start {
            if !receiver.same_identity(&self.member.start_port) {
                return Err(ValueError::ExpiredCapability(capability));
            }
            return self.start(arguments, effective).map(RuntimeReply::Value);
        }
        if capability == self.port_id {
            if !receiver.same_identity(&self.port) {
                return Err(ValueError::ExpiredCapability(capability));
            }
            return self.resume(arguments, effective).map(RuntimeReply::Value);
        }
        if self.signal_cap.get() == Some(&capability) {
            return self.request.values.heap().boolean(self.request.cancel.is_cancelled()).map(RuntimeReply::Value);
        }
        self.request.runtime().resolve_call(capability, receiver, arguments, effective)
    }
}
static AWAIT_OK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da MCP original async await fulfillment continuation",
    name: "mcpAfterAwait",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, arguments, rt| {
        let value = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(&captures[0], &captures[0], &[captures[1].clone(), heap.boolean(false)?, value], rt)
    },
};
static AWAIT_ERR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da MCP original async await rejection continuation",
    name: "mcpRejectedAwait",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, arguments, rt| {
        let value = arguments.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(&captures[0], &captures[0], &[captures[1].clone(), heap.boolean(true)?, value], rt)
    },
};

/// Concrete executable adapter, separate from the production mixed catalog
/// factory. It owns the one mutable Promise pump and retains native joins.
pub struct NativeMcpDriver {
    promise: NativePromiseRun,
    run: Arc<NativeMcpRun>,
}
impl NativeMcpDriver {
    pub fn new(promise: NativePromiseRun, source: Arc<NativeMcpSource>) -> Result<Self> {
        let run = NativeMcpRun::new(source, promise.admission())?;
        Ok(Self { promise, run })
    }
    pub fn run(&self) -> Arc<NativeMcpRun> {
        self.run.clone()
    }
    pub async fn drive<F: Future>(mut self, future: F) -> Result<F::Output> {
        tokio::pin!(future);
        let result = loop {
            let promise_wake = self.promise.wake();
            let host_wake = self.run.effects.wake();
            let promise_notice = promise_wake.notified();
            let host_notice = host_wake.notified();
            tokio::pin!(promise_notice, host_notice);
            promise_notice.as_mut().enable();
            host_notice.as_mut().enable();
            let mut ready = false;
            for _ in 0..64 {
                let host = match self.run.drain_one() {
                    Ok(host) => host,
                    Err(error) => {
                        self.run.revoke();
                        self.promise.revoker().revoke();
                        self.run.effects.settle().await;
                        return Err(error);
                    }
                };
                let promise = match self.promise.drain_one() {
                    Ok(promise) => !matches!(promise, NativeDrain::Idle),
                    Err(error) => {
                        self.run.revoke();
                        self.promise.revoker().revoke();
                        self.run.effects.settle().await;
                        return Err(error.into());
                    }
                };
                ready = host || promise;
                if !ready {
                    break;
                }
            }
            tokio::select! { result = &mut future => break result, _ = promise_notice => {}, _ = host_notice => {}, _ = tokio::task::yield_now(), if ready => {} }
        };
        self.run.effects.seal();
        let receipts = self.run.effects.settle().await;
        self.run.revoke();
        self.promise.revoker().revoke();
        let unknown: Vec<_> = receipts.iter().filter(|r| r.uncertain()).map(|r| r.summary()).collect();
        if !unknown.is_empty() {
            return Err(ActualProviderError::Callback(format!(
                "MCP native effects have unknown outcomes: {}",
                unknown.join("; ")
            )));
        }
        Ok(result)
    }
}
impl Drop for NativeMcpDriver {
    fn drop(&mut self) {
        self.run.revoke();
        self.promise.revoker().revoke();
    }
}
