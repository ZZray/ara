// Ported from OMP host-tools.ts / rpc-frame.ts at
// 596f2da7101178214aa27a753529d15e6b7ad91d. MIT: THIRD_PARTY_NOTICES.md.
//! Actual graph RPC Host tools over the binary's original connection owner.
//! This module owns no connection, pending registry or Promise driver. Raw
//! completions are consumed only by the same Session's synchronous graph pump.
//! Normal whole-frame stringify is implemented; original oversized graph
//! compaction/shrink/overflow re-observation remains an explicit delivery gap.

use crate::{
    actual_tools::async_effect::{HostEffectId, HostEffectSettlementOwner, NativeToolFault},
    native_realm::{
        NativeErrorClasses,
        context::NativeScope,
        parent::{NativeParentId, NativeParentLease},
        promise::{NativePendingPromise, NativePromiseAdmission, NativePromiseState},
    },
};
use ara_agent::{
    ActualExecuteRequest, ActualProviderError, ActualToolBinding, ActualToolDecision, ActualToolInvoker,
    ActualToolSource, AgentTool, Concurrency, SessionValues, ToolError, ToolOutput, UpdateFn,
};
use ara_ai::{JsonObject, Tool};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, RuntimeReply, RuntimeRequest,
        ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use async_trait::async_trait;
use futures::{FutureExt, future::BoxFuture};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ActualProviderError>;
type GraphResult<T> = std::result::Result<T, ValueError>;
pub type NativeRpcHostRawUpdate = Arc<dyn Fn(WireValue) + Send + Sync>;
/// Native-only final grant, called under the original pending publication lock.
pub type NativeRpcHostPublicationGrant = Arc<dyn Fn() -> bool + Send + Sync>;

/// Complete preserves the entire result and the separate frame.isError flag.
/// Only the original pending owner may classify authoritative publication.
pub enum NativeRpcHostOutcome {
    Complete { result: WireValue, is_error: bool },
    NotDispatched(String),
    Unknown(String),
}
pub enum NativeRpcHostPreparation {
    Ready(Box<dyn NativeRpcHostRequest>),
    NotDispatched(String),
}
pub trait NativeRpcHostRequest: Send {
    fn dispatch(
        self: Box<Self>,
        cancel: CancellationToken,
        grant: NativeRpcHostPublicationGrant,
    ) -> BoxFuture<'static, NativeRpcHostOutcome>;
}
/// Binary adapter implements this narrow port on the existing ToolBridge.
/// It registers the original pending ID BEFORE calling encode, outside locks.
/// encode is synchronous, must not escape, and receives the actual request ID.
pub trait NativeRpcHostOwner: Send + Sync {
    fn definition(&self) -> Tool;
    fn graph_definition(&self) -> WireValue;
    fn prepare_native(
        &self,
        call_id: &WireString,
        cancel: CancellationToken,
        update: NativeRpcHostRawUpdate,
        encode: &mut dyn FnMut(&WireString) -> GraphResult<WireValue>,
    ) -> GraphResult<NativeRpcHostPreparation>;
}

static NEXT: AtomicU64 = AtomicU64::new(1 << 61);
fn capability() -> GraphResult<CapabilityId> {
    NEXT.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| { id.checked_add(1).filter(|next| *next < (1 << 62))
    })
        .map(CapabilityId)
        .map_err(|_| ValueError::HeapTokensExhausted)
}
struct Member {
    owner: Arc<dyn NativeRpcHostOwner>,
    source: ValueHandle,
    execute: ValueHandle,
    start: CapabilityId,
    start_port: ValueHandle,
    name: WireString,
    enabled: Arc<AtomicBool>,
    capability: OnceLock<Arc<dyn AgentTool>>,
    run: Mutex<Option<Weak<NativeRpcHostRun>>>,
}
pub struct NativeRpcHostSource {
    values: Weak<SessionValues>,
    collection: ValueHandle,
    members: Vec<Arc<Member>>,
    environment: ValueHandle,
    errors: NativeErrorClasses,
    run_binding: Mutex<Option<Weak<NativeRpcHostRun>>>,
    device_snapshot: Mutex<Option<Weak<crate::native_factory::catalog::NativeCatalogSnapshot>>>,
}
impl NativeRpcHostSource {
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
            let member = self
                .members
                .iter()
                .find(|member| member.source.same_identity(&binding.source))
                .ok_or(ActualProviderError::Protocol(
                    "Device RPC has no exact canonical member",
                ))?;
            if !member
                .capability
                .get()
                .is_some_and(|original| Arc::ptr_eq(original, &binding.capability))
            {
                return Err(ActualProviderError::Protocol(
                    "Device RPC canonical capability differs",
                ));
            }
            RpcInvoker {
                member: member.clone(),
            }
            .invoke_method(request)
            .await
        })
    }
    pub fn new(
        values: &Arc<SessionValues>,
        owners: Vec<Arc<dyn NativeRpcHostOwner>>,
        environment: ValueHandle,
        errors: NativeErrorClasses,
    ) -> Result<Arc<Self>> {
        values.check_handle(&environment)?;
        for handle in [&errors.error, &errors.type_error, &errors.syntax_error, &errors.range_error] {
            values.check_handle(handle)?;
        }
        let heap = values.heap();
        let mut members = Vec::new();
        for owner in owners {
            let definition_graph = heap.import_strict(owner.graph_definition())?;
            let source = heap.object()?;
            let name = heap.get(&definition_graph, &"name".into(), values.runtime())?;
            let ValueSnapshot::String(name) = heap.snapshot(&name)? else {
                return Err(ActualProviderError::Protocol("RPC owner definition name is not a string"));
            };
            let start = capability()?;
            let start_port = heap.function(start)?;
            let execute = heap.fresh_compiled_function(&EXECUTE, &[start_port.clone()])?;
            // Original class owns strict/concurrency/loadMode; execute is a
            // non-enumerable class method. JSON normalizer emits six fields,
            // so genuine Function/Ark schema binding is outside this ingress.
            heap.define_own(
                &source,
                "execute".into(),
                PropertyDescriptor::Data {
                    value: execute.clone(),
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
                values.runtime(),
            )?;
            for (key, value) in [
                ("strict", heap.boolean(true)?),
                ("concurrency", heap.string("shared".into())?),
                ("loadMode", heap.get(&definition_graph, &"loadMode".into(), values.runtime())?),
            ] {
                heap.define_own_data(&source, key.into(), value, values.runtime())?;
            }
            // Fixed applyToolProxy's normal JSON domain: every access observes
            // the captured definition again, with a getter-only descriptor.
            for key in ["name", "label", "description", "parameters", "hidden"] {
                if heap.has(&source, &key.into(), values.runtime())? {
                    continue;
                }
                let getter =
                    heap.fresh_compiled_function(&PROXY_GET, &[definition_graph.clone(), heap.string(key.into())?])?;
                let ValueSnapshot::Function(getter) = heap.snapshot(&getter)? else {
                    return Err(ActualProviderError::Protocol("RPC proxy getter is not callable"));
                };
                heap.define_own(
                    &source,
                    key.into(),
                    PropertyDescriptor::Accessor { get: Some(getter), set: None, enumerable: true, configurable: true },
                    values.runtime(),
                )?;
            }
            let definition = owner.definition();
            let member = Arc::new(Member {
                owner,
                source,
                execute,
                start,
                start_port,
                name,
                enabled: Arc::new(AtomicBool::new(true)),
                capability: OnceLock::new(),
                run: Mutex::new(None),
            });
            member
                .capability
                .set(Arc::new(NativeRpcTool { member: Arc::downgrade(&member), definition }))
                .map_err(|_| ActualProviderError::Protocol("RPC capability already initialized"))?;
            members.push(member);
        }
        let collection = heap.array(members.iter().map(|member| member.source.clone()).collect())?;
        Ok(Arc::new(Self {
            values: Arc::downgrade(values),
            collection,
            members,
            environment,
            errors,
            device_snapshot: Mutex::new(None),
            run_binding: Mutex::new(None),
        }))
    }
    fn check(&self, values: &SessionValues) -> Result<()> {
        let original = self.values.upgrade().ok_or(ActualProviderError::Protocol("RPC original Session expired"))?;
        if !std::ptr::eq(original.as_ref(), values) {
            return Err(ValueError::ForeignHeap.into());
        }
        Ok(())
    }
    pub fn set_enabled(&self, index: usize, enabled: bool) -> Result<()> {
        self.members
            .get(index)
            .ok_or(ActualProviderError::Protocol("RPC grant index"))?
            .enabled
            .store(enabled, Ordering::Release);
        Ok(())
    }
    pub fn permission(&self, binding: &ActualToolBinding, values: &SessionValues) -> Result<ActualToolDecision> {
        self.check(values)?;
        let member = self
            .members
            .iter()
            .find(|m| m.source.same_identity(&binding.source))
            .ok_or(ActualProviderError::Protocol("RPC permission has no exact source owner"))?;
        let original = member.capability.get().ok_or(ActualProviderError::Protocol("RPC capability absent"))?;
        if !Arc::ptr_eq(original, &binding.capability) {
            return Err(ActualProviderError::Protocol("RPC permission capability differs from selected source"));
        }
        Ok(if member.enabled.load(Ordering::Acquire) {
            ActualToolDecision::Allow
        } else {
            ActualToolDecision::Block(Some("RPC Host tool grant revoked".into()))
        })
    }
    fn bind_run(&self, run: &Arc<NativeRpcHostRun>) -> Result<()> {
        let values = self.values.upgrade().ok_or(ActualProviderError::Protocol("RPC original Session expired"))?;
        run.admission.check_values(&values)?;
        let mut binding = self.run_binding.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = binding.as_ref().and_then(Weak::upgrade) {
            if Arc::ptr_eq(&old, run) {
                return Ok(());
            }
            if old.check_live().is_ok() {
                return Err(ActualProviderError::Protocol("RPC source already has a live Run"));
            }
        }
        for member in &self.members {
            *member.run.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::downgrade(run));
        }
        *binding = Some(Arc::downgrade(run));
        Ok(())
    }
}
impl ActualToolSource for NativeRpcHostSource {
    fn current_tools(&self, values: &SessionValues) -> Result<Option<ValueHandle>> {
        self.check(values)?;
        Ok(Some(self.collection.clone()))
    }
    fn bind_selected(&self, selected: &ValueHandle, values: &SessionValues) -> Result<ActualToolBinding> {
        self.check(values)?;
        let member = self
            .members
            .iter()
            .find(|m| m.source.same_identity(selected))
            .ok_or(ActualProviderError::Protocol("RPC selected source has no exact owner"))?;
        Ok(ActualToolBinding {
            source: selected.clone(),
            capability: member.capability.get().ok_or(ActualProviderError::Protocol("RPC capability absent"))?.clone(),
        })
    }
    fn has_fallback(&self) -> bool {
        false
    }
}
struct NativeRpcTool {
    member: Weak<Member>,
    definition: Tool,
}
#[async_trait]
impl AgentTool for NativeRpcTool {
    fn definition(&self) -> Tool {
        self.definition.clone()
    }
    fn concurrency(&self, _: &JsonObject) -> Concurrency {
        Concurrency::Shared
    }
    fn actual_invoker(&self) -> Option<Arc<dyn ActualToolInvoker>> {
        self.member.upgrade().map(|member| Arc::new(RpcInvoker { member }) as Arc<dyn ActualToolInvoker>)
    }
    async fn execute(
        &self,
        _: &str,
        _: JsonObject,
        _: CancellationToken,
        _: UpdateFn,
    ) -> std::result::Result<ToolOutput, ToolError> {
        Err(ToolError("Actual RPC Host capability requires its original graph invoker".into()))
    }
}

struct EffectJob {
    id: HostEffectId,
    invocation: Arc<Invocation>,
    pending: Arc<NativePendingPromise>,
    payload: Arc<Mutex<Option<NativeRpcHostOutcome>>>,
    updates: Arc<Mutex<VecDeque<WireValue>>>,
    _lease: NativeParentLease,
}
pub struct NativeRpcHostRun {
    source: Arc<NativeRpcHostSource>,
    admission: NativePromiseAdmission,
    effects: Arc<HostEffectSettlementOwner>,
    jobs: Mutex<Vec<Arc<EffectJob>>>,
    cursor: AtomicUsize,
    sealed: Arc<AtomicBool>,
    revoked: Arc<AtomicBool>,
}
impl NativeRpcHostRun {
    pub fn new(source: Arc<NativeRpcHostSource>, admission: NativePromiseAdmission) -> Result<Arc<Self>> {
        let values = source.values.upgrade().ok_or(ActualProviderError::Protocol("RPC original Session expired"))?;
        admission.check_values(&values)?;
        let run = Arc::new(Self {
            source,
            admission,
            effects: Arc::new(HostEffectSettlementOwner::new()),
            jobs: Mutex::new(Vec::new()),
            cursor: AtomicUsize::new(0),
            sealed: Arc::new(AtomicBool::new(false)),
            revoked: Arc::new(AtomicBool::new(false)),
        });
        run.source.bind_run(&run)?;
        Ok(run)
    }
    pub fn effects(&self) -> Arc<HostEffectSettlementOwner> {
        self.effects.clone()
    }
    pub fn wake(&self) -> Arc<Notify> {
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
        let _ = self.seal();
    }
    pub fn revoke(&self) {
        self.revoke_graph();
        self.effects.cancel();
    }
    fn check_live(&self) -> GraphResult<()> {
        self.admission.check_live()?;
        if self.revoked.load(Ordering::Acquire) {
            return Err(ValueError::UnsupportedOperation("RPC native graph Run is revoked"));
        }
        Ok(())
    }
    fn check_effect(&self, invocation: &Invocation) -> GraphResult<()> {
        self.check_live()?;
        if self.sealed.load(Ordering::Acquire) {
            return Err(ValueError::UnsupportedOperation("RPC native effect admission is sealed"));
        }
        invocation.request.check_admission().map_err(provider_value)?;
        if !invocation.member.enabled.load(Ordering::Acquire) {
            return Err(ValueError::TypeError("RPC original effect grant was revoked".into()));
        }
        Ok(())
    }
    /// The sole NativeRun graph pump consumes updates and completions. No raw
    /// input worker calls a graph callback or stores a heap/runtime owner.
    pub fn drain_one(&self) -> Result<bool> {
        enum Ready {
            Update(Arc<EffectJob>, WireValue),
            Complete(Arc<EffectJob>, std::result::Result<ToolOutput, NativeToolFault>),
        }
        let mut jobs = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if jobs.is_empty() {
            return Ok(false);
        }
        let cursor = self.cursor.fetch_add(1, Ordering::Relaxed) % jobs.len();
        jobs.rotate_left(cursor);
        let ready = jobs.iter().find_map(|job| {
            let raw = job.updates.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            if let Some(raw) = raw {
                Some(Ready::Update(job.clone(), raw))
            } else {
                self.effects.take_completion_for(job.id).map(|notice| Ready::Complete(job.clone(), notice))
            }
        });
        let (job, notice) = match ready {
            None => return Ok(false),
            Some(Ready::Complete(job, notice)) => (job, notice),
            Some(Ready::Update(job, raw)) => {
                self.check_live()?;
                let request = &job.invocation.request;
                let rt = self.admission.scoped(job.invocation.scope, job.invocation.parent()?)?;
                let heap = request.values.heap();
                let value = heap.import_strict(raw)?;
                let update =
                    job.invocation.update.get().ok_or(ActualProviderError::Protocol("RPC original onUpdate absent"))?;
                let result = heap.call(update, &heap.undefined()?, &[value], &rt);
                if let Err(error) = result {
                    // The original pending request and effect job remain live.
                    if let Some(original) =
                        job.invocation.update_failure.lock().unwrap_or_else(|e| e.into_inner()).take()
                    {
                        return Err(original);
                    }
                    return Err(self.source.errors.create_at_native_origin(error, &rt).into());
                }
                return Ok(true);
            }
        };
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).retain(|other| !Arc::ptr_eq(other, &job));
        self.check_live()?;
        let request = &job.invocation.request;
        let heap = request.values.heap();
        let rt = self.admission.scoped(job.invocation.scope, job.invocation.parent()?)?;
        let payload = job.payload.lock().unwrap_or_else(|e| e.into_inner()).take();
        let operation: GraphResult<(bool, ValueHandle)> = match payload {
            Some(NativeRpcHostOutcome::Complete { result, is_error }) => {
                let raw = heap.import_strict(result)?;
                if is_error {
                    host_error(heap, &self.source.environment, &raw, &rt).map(|e| (true, e))
                } else {
                    Ok((false, raw))
                }
            }
            Some(NativeRpcHostOutcome::NotDispatched(message) | NativeRpcHostOutcome::Unknown(message),) =>
                error(heap, &self.source.environment, message.into(), &rt).map(|e| (true, e)),
            None => {
                let fault =
                    notice.err().ok_or(ActualProviderError::Protocol("RPC worker completed without payload"))?;
                // This entered worker panic/join loss remains independently Unknown.
                error(
                    heap,
                    &self.source.environment,
                    format!("RPC native effect ended without result: {fault:?}").into(),
                    &rt,
                )
                .map(|e| (true, e))
            }
        };
        let (failed, value) = match operation {
            Ok(result) => result,
            Err(failure) => (true, reason(&self.source.errors, failure, &rt)?),
        };
        let resolver = if failed { &job.pending.reject } else { &job.pending.resolve };
        heap.call(resolver, &heap.undefined()?, &[value], &rt)?;
        Ok(true)
    }
    fn prepare(&self, invocation: Arc<Invocation>, rt: &dyn ValueRuntime) -> GraphResult<ValueHandle> {
        self.check_effect(&invocation)?;
        let heap = invocation.request.values.heap();
        let pending = Arc::new(self.admission.prepare_pending(invocation.scope, invocation.parent()?)?);
        *invocation.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(pending.clone());
        let updates = Arc::new(Mutex::new(VecDeque::new()));
        let update_queue = updates.clone();
        let wake = self.effects.wake();
        let update: NativeRpcHostRawUpdate = Arc::new(move |raw| {
            update_queue.lock().unwrap_or_else(|e| e.into_inner()).push_back(raw);
            wake.notify_waiters();
        });
        // Retain the actual settlement owner before original getters/toJSON can
        // seal/revoke the graph after the binary has already received Complete.
        // This native channel carries only the existing prepared request; no
        // graph handle, parent runtime or admission owner enters its worker.
        let lease = self.admission.retain_parent(invocation.parent()?)?;
        let reservation = self.effects.reserve().map_err(effect_fault)?;
        let id = reservation.id();
        let payload = Arc::new(Mutex::new(None));
        let worker_payload = payload.clone();
        let cancel = invocation.request.cancel.child_token();
        let worker_cancel = cancel.clone();
        let (prepared_send, prepared_receive) = oneshot::channel::<NativeRpcHostPreparation>();
        let publication_checked = Arc::new(AtomicBool::new(false));
        let checked = publication_checked.clone();
        let enabled = invocation.member.enabled.clone();
        let sealed = self.sealed.clone();
        let revoked = self.revoked.clone();
        let grant: NativeRpcHostPublicationGrant = Arc::new(move || {
            checked.load(Ordering::Acquire)
                && enabled.load(Ordering::Acquire)
                && !sealed.load(Ordering::Acquire)
                && !revoked.load(Ordering::Acquire)
        });
        reservation
            .enter_authoritative(
                cancel,
                async move {
                    let preparation = prepared_receive.await.unwrap_or_else(|_| {
                        NativeRpcHostPreparation::NotDispatched(
                            "RPC original frame preparation ended before publication".into(),
                        )
                    });
                    let outcome = match preparation {
                        NativeRpcHostPreparation::Ready(prepared) => { prepared.dispatch(worker_cancel, grant).await
                        }
                        NativeRpcHostPreparation::NotDispatched(message) => {
                            NativeRpcHostOutcome::NotDispatched(message)
                        }
                    };
                    let status = match &outcome {
                        NativeRpcHostOutcome::Complete { result, is_error } => {
                            let mut status = ToolOutput::text("");
                            status.is_error = *is_error || result.get("isError").is_some_and(wire_truthy);
                            Ok(status)
                        }
                        NativeRpcHostOutcome::NotDispatched(message) => {
                            Err(NativeToolFault::NotDispatched(message.clone()))
                        }
                        NativeRpcHostOutcome::Unknown(message) => { Err(NativeToolFault::OutcomeUnknown(message.clone()))
                        }
                    };
                    *worker_payload.lock().unwrap_or_else(|e| e.into_inner()) = Some(outcome);
                    status
                }
                .boxed(),
            )
            .map_err(effect_fault)?;
        self.jobs.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::new(EffectJob {
            id,
            invocation: invocation.clone(),
            pending: pending.clone(),
            payload,
            updates,
            _lease: lease,
        }));
        let prepared = match invocation.member.owner.prepare_native(
            &invocation.request.call_id,
            invocation.request.cancel.clone(),
            update,
            &mut |id| {
                encode_frame(
                    heap,
                    &self.source.environment,
                    id,
                    &invocation.member.name,
                    &invocation.request.call_id,
                    invocation.request.execution_args(),
                    rt,
                )
            },
        ) {
            Ok(prepared) => prepared,
            Err(original) => {
                // Binary's unpublished guard cleans its own pending slot. The
                // graph retains the exact encoder error; the entered native
                // owner gets a definite completion instead of a hung channel.
                let _ = prepared_send.send(NativeRpcHostPreparation::NotDispatched(
                    "RPC original frame encoding failed before publication".into(),
                ));
                return Err(original);
            }
        };
        // Re-check original graph/argument/grant admission outside binary locks.
        // A failed check forbids publication, but must not drop a received
        // Complete: the SAME binary receiver settles under authoritative receipt.
        let final_check = self.check_effect(&invocation);
        publication_checked.store(final_check.is_ok(), Ordering::Release);
        let _ = prepared_send.send(prepared);
        final_check?;
        Ok(pending.promise.clone())
    }
}

struct RpcInvoker {
    member: Arc<Member>,
}
#[async_trait]
impl ActualToolInvoker for RpcInvoker {
    async fn invoke_settled(&self, request: ActualExecuteRequest) -> Result<ValueHandle> {
        self.invoke_method(crate::actual_tools::xdev_native::NativeMethodRequest::root(
            request,
        ))
        .await
    }
}
impl RpcInvoker {
    async fn invoke_method(
        &self,
        request: crate::actual_tools::xdev_native::NativeMethodRequest,
    ) -> Result<ValueHandle> {
        request.check_admission()?;
        if !request.receiver.same_identity(&self.member.source) || !request.callable.same_identity(&self.member.execute)
        {
            return Err(ActualProviderError::Protocol("RPC callable/receiver differs from original owner"));
        }
        let run = self
            .member
            .run
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol("RPC original Run absent"))?;
        run.admission.check_values(&request.values)?;
        run.check_live()?;
        let heap = request.values.heap().clone();
        let scope = request.scope(&run.admission)?;
        let invocation = Arc::new(Invocation {
            request: Arc::new(request),
            member: self.member.clone(),
            run: Arc::downgrade(&run),
            scope,
            parent: OnceLock::new(),
            update_cap: capability()?,
            update_port: OnceLock::new(),
            update: OnceLock::new(),
            signal_cap: capability()?,
            signal: OnceLock::new(),
            result: Mutex::new(None),
            started: AtomicBool::new(false),
            update_failure: Mutex::new(None),
            original: OnceLock::new(),
        });
        invocation
            .original
            .set(Arc::downgrade(&invocation))
            .map_err(|_| ActualProviderError::Protocol("RPC invocation already initialized"))?;
        let parent = run.admission.register_parent(invocation.clone())?;
        invocation.parent.set(parent).map_err(|_| ActualProviderError::Protocol("RPC parent already set"))?;
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
        let rt = run.admission.scoped(invocation.scope, parent)?;
        crate::actual_tools::xdev_native::approve_root_method(
            run.source.device_snapshot(&invocation.request.values)?,&invocation.request,
            Arc::new(run.admission.scoped(invocation.scope,parent)?),invocation.scope,parent,
        ).await?;
        invocation.request.check_admission()?;
        run.admission.scoped(invocation.scope,parent)?;
        run.check_live()?;

        let signal = heap.object()?;
        heap.define_own(
            &signal,
            "aborted".into(),
            PropertyDescriptor::Accessor {
                get: Some(ara_rpc::value_heap::CallableTarget::Host(invocation.signal_cap)),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &rt,
        )?;
        let signal = invocation.request.signal().cloned().unwrap_or(signal);
        invocation.signal.set(signal.clone()).map_err(|_| ActualProviderError::Protocol("RPC signal already set"))?;
        let update_port = heap.function(invocation.update_cap)?;
        invocation
            .update_port
            .set(update_port.clone())
            .map_err(|_| ActualProviderError::Protocol("RPC update port already set"))?;
        let update = heap.fresh_compiled_function(&UPDATE, &[update_port])?;
        invocation.update.set(update.clone()).map_err(|_| ActualProviderError::Protocol("RPC update already set"))?;
        // Fixed RPC method order differs from MCP: id, params, signal, onUpdate.
        let called = heap
            .call(
                &invocation.request.callable,
                &invocation.request.receiver,
                &[
                    heap.string(invocation.request.call_id.clone())?,
                    invocation.request.execution_args().clone(),
                    signal,
                    update,
                    invocation.request.tool_context.clone(),
                ],
                &rt,
            )
            .map_err(|e| run.source.errors.create_at_native_origin(e, &rt))?;
        drop(rt);
        let pending = invocation
            .result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or(ActualProviderError::Protocol("RPC execute returned no original Promise"))?;
        if !called.same_identity(&pending.promise) {
            return Err(ActualProviderError::Protocol("RPC execute returned a different Promise"));
        }
        loop {
            let wake = run.admission.wake();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            run.check_live()?;
            match run.admission.ticket_state(&pending.ticket())? {
                NativePromiseState::Fulfilled(value) => return Ok(heap.restore_deferred(&value)?),
                NativePromiseState::Rejected(value) => {
                    return Err(ValueError::Thrown(heap.restore_deferred(&value)?).into());
                }
                NativePromiseState::Pending => notified.await,
            }
        }
    }
}
struct Invocation {
    request: Arc<crate::actual_tools::xdev_native::NativeMethodRequest>,
    member: Arc<Member>,
    run: Weak<NativeRpcHostRun>,
    scope: NativeScope,
    parent: OnceLock<NativeParentId>,
    original: OnceLock<Weak<Invocation>>,
    update_cap: CapabilityId,
    update_port: OnceLock<ValueHandle>,
    update: OnceLock<ValueHandle>,
    signal_cap: CapabilityId,
    signal: OnceLock<ValueHandle>,
    result: Mutex<Option<Arc<NativePendingPromise>>>,
    started: AtomicBool,
    update_failure: Mutex<Option<ActualProviderError>>,
}
impl Invocation {
    fn parent(&self) -> GraphResult<NativeParentId> {
        self.parent.get().copied().ok_or(ValueError::UnsupportedOperation("RPC parent unregistered"))
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
        cap: CapabilityId,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> GraphResult<RuntimeReply> {
        if cap == self.signal_cap {
            if !receiver.same_identity(self.signal.get().ok_or(ValueError::ExpiredCapability(cap))?) || !args.is_empty()
            {
                return Err(ValueError::ExpiredCapability(cap));
            }
            return self.request.values.heap().boolean(self.request.cancel.is_cancelled()).map(RuntimeReply::Value);
        }
        if cap == self.update_cap {
            let expected = self.update_port.get().ok_or(ValueError::ExpiredCapability(cap))?;
            if !receiver.same_identity(expected) || args.len() != 1 {
                return Err(ValueError::ExpiredCapability(cap));
            }
            self.request.values.check_handle(&args[0])?;
            if let Err(failure) = (self.request.on_update)(args[0].clone()) {
                let returned = match &failure {
                    ActualProviderError::Value(ValueError::Thrown(value)) => { ValueError::Thrown(value.clone())
                    }
                    _ => ValueError::UnsupportedOperation("RPC original onUpdate failed"),
                };
                *self.update_failure.lock().unwrap_or_else(|e| e.into_inner()) = Some(failure);
                return Err(returned);
            }
            return self.request.values.heap().undefined().map(RuntimeReply::Value);
        }
        if cap == self.member.start {
            self.request.check_execution_ready()?;
            if !receiver.same_identity(&self.member.start_port)
                || args.len() != 5
                || !args[0].same_identity(&self.member.source)
                || !args[2].same_identity(self.request.execution_args())
                || !args[3].same_identity(self.signal.get().ok_or(ValueError::ExpiredCapability(cap))?)
                || !args[4].same_identity(self.update.get().ok_or(ValueError::ExpiredCapability(cap))?)
            {
                return Err(ValueError::ExpiredCapability(cap));
            }
            if !matches!(self.request.values.heap().snapshot(&args[1])?, ValueSnapshot::String(id) if id == self.request.call_id)
            {
                return Err(ValueError::ExpiredCapability(cap));
            }
            if self.started.swap(true, Ordering::AcqRel) {
                return Err(ValueError::ExpiredCapability(cap));
            }
            let original = self.original.get().and_then(Weak::upgrade).ok_or(ValueError::ExpiredCapability(cap))?;
            let run = self.run.upgrade().ok_or(ValueError::ExpiredCapability(cap))?;
            return run.prepare(original, effective).map(RuntimeReply::Value);
        }
        self.request.runtime().resolve_call(cap, receiver, args, effective)
    }
}
static PROXY_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da extensibility/tool-proxy.ts getter-only normal JSON definition access",
    name: "get",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: |heap, captures, _, _, rt| {
        let ValueSnapshot::String(key) = heap.snapshot(&captures[1])? else {
            return Err(ValueError::TypeError("RPC proxy property key is not a string".into()));
        };
        heap.get(&captures[0], &key.into(), rt)
    },
};
static EXECUTE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da host-tools.ts RpcHostToolAdapter.execute captured bridge requestExecution",
    name: "execute",
    length: 4,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, receiver, args, rt| {
        let mut forwarded = vec![receiver.clone()];
        for index in 0..4 {
            forwarded.push(args.get(index).cloned().unwrap_or(heap.undefined()?));
        }
        heap.call(&captures[0], &captures[0], &forwarded, rt)
    },
};
static UPDATE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da host-tools.ts pending.onUpdate original callback",
    name: "onUpdate",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, args, rt| {
        heap.call(&captures[0], &captures[0], &[args.first().cloned().unwrap_or(heap.undefined()?)], rt)
    },
};
fn provider_value(error: ActualProviderError) -> ValueError {
    match error {
        ActualProviderError::Value(error) => error,
        _ => ValueError::UnsupportedOperation("RPC actual admission failed"),
    }
}
fn effect_fault(fault: NativeToolFault) -> ValueError {
    ValueError::TypeError(format!("RPC native effect admission: {fault:?}"))
}
fn wire_truthy(value: &WireValue) -> bool {
    match value {
        WireValue::Null => false,
        WireValue::Bool(value) => *value,
        WireValue::Number(value) => *value != 0.0 && !value.is_nan(),
        WireValue::String(value) => !value.is_empty(),
        WireValue::Array(_) | WireValue::Object(_) => true,
    }
}
fn reason(errors: &NativeErrorClasses, error: ValueError, rt: &dyn ValueRuntime) -> GraphResult<ValueHandle> {
    match errors.create_at_native_origin(error, rt) {
        ValueError::Thrown(value) => Ok(value),
        other => Err(other),
    }
}
fn error(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    message: WireString,
    rt: &dyn ValueRuntime,
) -> GraphResult<ValueHandle> {
    let ctor = heap.get(environment, &"Error".into(), rt)?;
    heap.construct(&ctor, &[heap.string(message)?], rt)
}
fn method(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    name: &str,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> GraphResult<ValueHandle> {
    let callable = heap.get(receiver, &name.into(), rt)?;
    heap.call(&callable, receiver, args, rt)
}
fn host_error(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    raw: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> GraphResult<ValueHandle> {
    let content = heap.get(raw, &"content".into(), rt)?;
    let predicate = heap.fresh_compiled_function(&FILTER_TEXT, &[])?;
    let content = method(heap, &content, "filter", &[predicate], rt)?;
    let mapper = heap.fresh_compiled_function(&MAP_TEXT, &[])?;
    let content = method(heap, &content, "map", &[mapper], rt)?;
    let joined = method(heap, &content, "join", &[heap.string("\n".into())?], rt)?;
    let text = method(heap, &joined, "trim", &[], rt)?;
    let text = if heap.truthy(&text)? { text } else { heap.string("Host tool execution failed".into())? };
    let ctor = heap.get(environment, &"Error".into(), rt)?;
    heap.construct(&ctor, &[text], rt)
}
static FILTER_TEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da host-tools.ts handleResult original filter text predicate",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, args, rt| {
        let item = args.first().cloned().unwrap_or(heap.undefined()?);
        let kind = heap.get(&item, &"type".into(), rt)?;
        let text = matches!(heap.snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("text"));
        let valid = text && matches!(heap.snapshot(&heap.get(&item, &"text".into(), rt)?)?, ValueSnapshot::String(_));
        heap.boolean(valid)
    },
};
static MAP_TEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da host-tools.ts handleResult original text mapper",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, args, rt| { heap.get(&args.first().cloned().unwrap_or(heap.undefined()?), &"text".into(), rt,)
    },
};
fn encode_frame(
    heap: &Arc<ValueHeap>,
    environment: &ValueHandle,
    id: &WireString,
    name: &WireString,
    call_id: &WireString,
    args: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> GraphResult<WireValue> {
    let frame = heap.object()?;
    for (key, value) in [
        ("type", heap.string("host_tool_call".into())?),
        ("id", heap.string(id.clone())?),
        ("toolCallId", heap.string(call_id.clone())?),
        ("toolName", heap.string(name.clone())?),
        ("arguments", args.clone()),
    ] {
        heap.define_own_data(&frame, key.into(), value, rt)?;
    }
    let json = heap.get(environment, &"JSON".into(), rt)?;
    let serialized = method(heap, &json, "stringify", &[frame], rt)?;
    let ValueSnapshot::String(serialized) = heap.snapshot(&serialized)? else {
        return Err(ValueError::TypeError("RPC original JSON.stringify did not return a string".into()));
    };
    WireValue::parse_units(serialized.units())
        .map_err(|_| ValueError::SyntaxError("RPC JSON.stringify output is not JSON".into()))
}
