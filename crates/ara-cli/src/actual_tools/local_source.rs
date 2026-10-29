//! Owned same-realm local callback supplier for fixed OMP AgentTool declarations.
//! This supplies authentic source/capability and NativeRun admission, not an
//! Extension loader, typed execute fallback or new effect worker.
use super::{async_effect::{NativeBuiltinRun, ParentRegistration}, xdev_native::{NativeMethodRequest, approve_root_method}};
use crate::{native_factory::catalog::NativeToolCatalog, native_realm::{context::NativeScope, parent::NativeParentId, promise::NativePromiseState}};
use ara_agent::{ActualExecuteRequest,ActualProviderError,ActualToolBinding,ActualToolDecision,ActualToolInvoker,ActualToolSource,AgentTool,SessionValues,ToolError,ToolOutput,UpdateFn};
use ara_ai::{JsonObject,Tool};
use ara_rpc::{WireString,value_heap::{CapabilityId,CallableTarget,PropertyDescriptor,RuntimeReply,RuntimeRequest,ValueError,ValueHandle,ValueKind,ValueRuntime}};
use async_trait::async_trait;
use std::sync::{Arc,Mutex,OnceLock,Weak,atomic::{AtomicBool,AtomicU64,Ordering}};
use tokio_util::sync::CancellationToken;
type Result<T> = std::result::Result<T,ActualProviderError>;
static NEXT_REGISTRATION:AtomicU64=AtomicU64::new(1);

struct Member {
    source:ValueHandle,
    execute:ValueHandle,
    capability:Arc<dyn AgentTool>,
    enabled:AtomicBool,
}
/// A Host-authored original collection. Re-registering a removed source creates
/// a new owner/generation/capability; a revoked old binding never revives.
pub struct NativeLocalSource {
    values:Weak<SessionValues>,
    originals:ValueHandle,
    members:Vec<Member>,
    builtin_names:Vec<WireString>,
    generation:u64,
    run:Mutex<Option<Weak<NativeBuiltinRun>>>,
    catalog:Mutex<Option<Weak<NativeToolCatalog>>>,
}
impl NativeLocalSource {
    pub(crate) fn new(values:&Arc<SessionValues>,originals:ValueHandle,definitions:Vec<Tool>,builtin_names:Vec<WireString>)->Result<Arc<Self>> {
        values.check_handle(&originals)?;
        let heap=values.heap();
        let length=heap.array_length(&originals,values.runtime())?;
        if length as usize!=definitions.len(){return Err(ActualProviderError::Protocol("Local original declarations/capabilities differ"))}
        let generation=NEXT_REGISTRATION.fetch_update(Ordering::Relaxed,Ordering::Relaxed,|n|n.checked_add(1))
            .map_err(|_|ActualProviderError::Protocol("Local registration generation exhausted"))?;
        let mut sources=Vec::new();
        for index in 0..length {
            let selected=heap.get(&originals,&index.to_string().as_str().into(),values.runtime())?;
            values.check_handle(&selected)?;
            if sources.iter().any(|(old,_):&(ValueHandle,ValueHandle)|old.same_identity(&selected)) {
                return Err(ActualProviderError::Protocol("Local original source registered twice"))
            }
            let execute=heap.get(&selected,&"execute".into(),values.runtime())?;
            if heap.kind(&execute)?!=ValueKind::Function{return Err(ValueError::TypeError("Local original execute is not callable".into()).into())}
            sources.push((selected,execute));
        }
        Ok(Arc::new_cyclic(|owner| Self {
            values:Arc::downgrade(values),originals,members:sources.into_iter().zip(definitions).enumerate().map(|(index,((source,execute),definition))|Member {
                source,execute,enabled:AtomicBool::new(true),capability:Arc::new(LocalCapability {definition,invoker:Arc::new(LocalInvoker {owner:owner.clone(),index,generation})}),
            }).collect(),builtin_names,generation,run:Mutex::new(None),catalog:Mutex::new(None),
        }))
    }
    fn check(&self,values:&SessionValues)->Result<()> {
        let original=self.values.upgrade().ok_or(ActualProviderError::Protocol("Local source Session expired"))?;
        if !std::ptr::eq(original.as_ref(),values){return Err(ValueError::ForeignHeap.into())}
        Ok(())
    }
    pub(crate) fn is_builtin(&self,name:&WireString)->bool {self.builtin_names.contains(name)}
    pub(crate) fn bind_catalog(&self,catalog:&Weak<NativeToolCatalog>,values:&SessionValues)->Result<()> {
        self.check(values)?;
        let mut binding=self.catalog.lock().unwrap_or_else(|e|e.into_inner());
        if binding.as_ref().is_some_and(|old|!Weak::ptr_eq(old,catalog)) {return Err(ActualProviderError::Protocol("Local source belongs to another original catalog"))}
        *binding=Some(catalog.clone());Ok(())
    }
    pub(crate) fn bind_run(&self,run:&Arc<NativeBuiltinRun>,values:&SessionValues)->Result<()> {
        self.check(values)?;run.check_values(values)?;run.check_live()?;
        let mut binding=self.run.lock().unwrap_or_else(|e|e.into_inner());
        if binding.as_ref().and_then(Weak::upgrade).is_some_and(|old|old.check_live().is_ok()&&!Arc::ptr_eq(&old,run)) {
            return Err(ActualProviderError::Protocol("Local source already belongs to another live Run"))
        }
        *binding=Some(Arc::downgrade(run));Ok(())
    }
    pub(crate) fn permission(&self,binding:&ActualToolBinding,values:&SessionValues)->Result<ActualToolDecision> {
        self.check(values)?;values.check_handle(&binding.source)?;
        let member=self.members.iter().find(|m|m.source.same_identity(&binding.source)&&Arc::ptr_eq(&m.capability,&binding.capability))
            .ok_or(ActualProviderError::Protocol("Local source/capability owner differs"))?;
        Ok(if member.enabled.load(Ordering::Acquire){ActualToolDecision::Allow}else{ActualToolDecision::Block(Some("Local source registration was removed".into()))})
    }
    /// Publication revokes only an exact absent source/capability association.
    pub(crate) fn revoke_binding(&self,binding:&ActualToolBinding) {
        if let Some(member)=self.members.iter().find(|m|m.source.same_identity(&binding.source)&&Arc::ptr_eq(&m.capability,&binding.capability)) {
            member.enabled.store(false,Ordering::Release);
            let run=self.run.lock().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(Weak::upgrade);
            if let Some(run)=run{run.admission.wake().notify_waiters();}
        }
    }
    pub(crate) fn invoke_device(self:Arc<Self>,binding:ActualToolBinding,request:NativeMethodRequest)->futures::future::BoxFuture<'static,Result<ValueHandle>> {
        Box::pin(async move {
            let index=self.members.iter().position(|m|m.source.same_identity(&binding.source)&&Arc::ptr_eq(&m.capability,&binding.capability))
                .ok_or(ActualProviderError::Protocol("Local device has no exact owned member"))?;
            LocalInvoker{owner:Arc::downgrade(&self),index,generation:self.generation}.invoke_method(request).await
        })
    }
}
impl ActualToolSource for NativeLocalSource {
    fn current_tools(&self,values:&SessionValues)->Result<Option<ValueHandle>> {self.check(values)?;Ok(Some(self.originals.clone()))}
    fn bind_selected(&self,selected:&ValueHandle,values:&SessionValues)->Result<ActualToolBinding> {
        self.check(values)?;values.check_handle(selected)?;
        let member=self.members.iter().find(|m|m.source.same_identity(selected)).ok_or(ActualProviderError::Protocol("Local selected source has no owned capability"))?;
        Ok(ActualToolBinding{source:member.source.clone(),capability:member.capability.clone()})
    }
    fn has_fallback(&self)->bool {false}
}
struct LocalCapability {definition:Tool,invoker:Arc<LocalInvoker>}
#[async_trait]
impl AgentTool for LocalCapability {
    fn definition(&self)->Tool {self.definition.clone()}
    fn actual_invoker(&self)->Option<Arc<dyn ActualToolInvoker>> {Some(self.invoker.clone())}
    async fn execute(&self,_:&str,_:JsonObject,_:CancellationToken,_:UpdateFn)->std::result::Result<ToolOutput,ToolError> {
        Err(ToolError("Local original callback requires its NativeRun admission".into()))
    }
}
struct LocalInvoker {owner:Weak<NativeLocalSource>,index:usize,generation:u64}
#[async_trait]
impl ActualToolInvoker for LocalInvoker {
    async fn invoke_settled(&self,request:ActualExecuteRequest)->Result<ValueHandle> {self.invoke_method(NativeMethodRequest::root(request)).await}
}
impl LocalInvoker {
    async fn invoke_method(&self,request:NativeMethodRequest)->Result<ValueHandle> {
        request.check_admission()?;
        let owner=self.owner.upgrade().ok_or(ActualProviderError::Protocol("Local invocation owner expired"))?;
        owner.check(&request.values)?;
        let member=owner.members.get(self.index).ok_or(ActualProviderError::Protocol("Local member absent"))?;
        if owner.generation!=self.generation||!member.source.same_identity(&request.receiver)||!member.execute.same_identity(&request.callable) {
            return Err(ActualProviderError::Protocol("Local callable/receiver registration differs"))
        }
        if !member.enabled.load(Ordering::Acquire){return Err(ActualProviderError::Protocol("Local registration was removed"))}
        let run=owner.run.lock().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol("Local original Run is absent"))?;
        run.check_values(&request.values)?;run.check_live()?;
        let claimed=request.claim_context(&run)?;
        let catalog=owner.catalog.lock().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(Weak::upgrade)
            .ok_or(ActualProviderError::Protocol("Local original catalog expired"))?;
        let invocation=Arc::new(LocalInvocation {
            owner,index:self.index,generation:self.generation,request:Arc::new(request),run:Arc::downgrade(&run),scope:claimed.scope,parent:OnceLock::new(),
            update:super::capability()?,signal:super::capability()?,entered:AtomicBool::new(false),closed:AtomicBool::new(false),update_failure:Mutex::new(None),
        });
        let parent=run.admission.register_parent(invocation.clone())?;
        invocation.parent.set(parent).map_err(|_|ActualProviderError::Protocol("Local original parent already set"))?;
        let _registration=ParentRegistration{admission:run.admission.clone(),id:parent};
        struct Exit(Weak<LocalInvocation>);
        impl Drop for Exit {fn drop(&mut self){if let Some(original)=self.0.upgrade(){original.closed.store(true,Ordering::Release);}}}
        let _exit=Exit(Arc::downgrade(&invocation));
        approve_root_method(Some(catalog.execution_snapshot(&invocation.request.values)?),&invocation.request,
            Arc::new(run.admission.scoped(invocation.scope,parent)?),invocation.scope,parent).await?;
        invocation.check_entry()?;invocation.request.check_execution_ready()?;
        let heap=invocation.request.values.heap();
        let runtime=run.admission.scoped(invocation.scope,parent)?;
        // Explicit Get(execute) after all guards catches changed current method identity.
        let current=heap.get(&invocation.request.receiver,&"execute".into(),&runtime)?;
        if !current.same_identity(&invocation.request.callable){return Err(ActualProviderError::Protocol("Local original execute changed before entry"))}
        let signal=if let Some(signal)=invocation.request.signal(){signal.clone()}else{
            let signal=heap.object()?;
            heap.define_own(&signal,"aborted".into(),PropertyDescriptor::Accessor{get:Some(CallableTarget::Host(invocation.signal)),set:None,enumerable:true,configurable:true},&runtime)?;signal
        };
        let update=heap.function(invocation.update)?;
        let args=[heap.string(invocation.request.call_id.clone())?,invocation.request.execution_args().clone(),signal,update,invocation.request.tool_context.clone()];
        let pending=run.admission.prepare_pending(invocation.scope,parent)?;
        // Repeat after all programmable Gets and Promise preparation, before
        // entering the original callback; removal never authorizes a replay.
        invocation.check_entry()?;invocation.request.check_execution_ready()?;
        invocation.entered.store(true,Ordering::Release);
        // Exactly one source callback Call. The real Promise resolver adopts
        // its original return/thenable; no Graph-to-serde or typed execution.
        let called=heap.call(&invocation.request.callable,&invocation.request.receiver,&args,&runtime)?;
        heap.call(&pending.resolve,&heap.undefined()?,&[called],&runtime)?;
        drop(runtime);
        loop {
            let wake=run.admission.wake();let notified=wake.notified();tokio::pin!(notified);notified.as_mut().enable();
            invocation.check_entry()?;
            if let Some(error)=invocation.update_failure.lock().unwrap_or_else(|e|e.into_inner()).clone(){return Err(error)}
            match run.admission.ticket_state(&pending.ticket())? {
                NativePromiseState::Fulfilled(value)=>return Ok(heap.restore_deferred(&value)?),
                NativePromiseState::Rejected(value)=>return Err(ValueError::Thrown(heap.restore_deferred(&value)?).into()),
                NativePromiseState::Pending=>tokio::select!{_=notified=>{},_=invocation.request.cancel.cancelled()=>return Err(ActualProviderError::Callback("Local callback cancelled; entered effects are not replayable".into()))},
            }
        }
    }
}
struct LocalInvocation {
    owner:Arc<NativeLocalSource>,index:usize,generation:u64,request:Arc<NativeMethodRequest>,run:Weak<NativeBuiltinRun>,
    scope:NativeScope,parent:OnceLock<NativeParentId>,update:CapabilityId,signal:CapabilityId,entered:AtomicBool,closed:AtomicBool,update_failure:Mutex<Option<ActualProviderError>>,
}
impl LocalInvocation {
    fn check_entry(&self)->Result<()> {
        if self.closed.load(Ordering::Acquire){return Err(ActualProviderError::Protocol("Local original invocation is closed"))}
        self.request.check_admission()?;self.owner.check(&self.request.values)?;
        let run=self.run.upgrade().ok_or(ActualProviderError::Protocol("Local original Run expired"))?;run.check_live()?;
        run.admission.scoped(self.scope,*self.parent.get().ok_or(ActualProviderError::Protocol("Local original parent is not registered"))?)?;
        let member=&self.owner.members[self.index];
        if self.owner.generation!=self.generation||!member.enabled.load(Ordering::Acquire)||!member.source.same_identity(&self.request.receiver)||!member.execute.same_identity(&self.request.callable) {
            return Err(ActualProviderError::Protocol("Local invocation registration/grant changed"))
        }
        if self.request.cancel.is_cancelled(){return Err(ActualProviderError::Callback("Local callback cancelled; effects may be unknown".into()))}
        Ok(())
    }
    fn graph_check(&self)->std::result::Result<(),ValueError> {
        self.check_entry().map_err(|e|match e{ActualProviderError::Value(e)=>e,_=>ValueError::UnsupportedOperation("Local original invocation admission failed")})
    }
}
impl ValueRuntime for LocalInvocation {
    fn resolve(&self,request:RuntimeRequest<'_>)->std::result::Result<RuntimeReply,ValueError> {self.resolve_effective(request,self)}
    fn resolve_effective(&self,request:RuntimeRequest<'_>,effective:&dyn ValueRuntime)->std::result::Result<RuntimeReply,ValueError> {
        match request {RuntimeRequest::Call{capability,receiver,arguments}=>self.resolve_call(capability,receiver,arguments,effective),other=>{self.graph_check()?;self.request.runtime().resolve_effective(other,effective)}}
    }
    fn resolve_call(&self,capability:CapabilityId,receiver:&ValueHandle,arguments:&[ValueHandle],effective:&dyn ValueRuntime)->std::result::Result<RuntimeReply,ValueError> {
        self.graph_check()?;
        if capability==self.signal {return self.request.values.heap().boolean(self.request.cancel.is_cancelled()).map(RuntimeReply::Value)}
        if capability==self.update {
            self.request.check_execution_ready()?;
            if !self.entered.load(Ordering::Acquire){return Err(ValueError::TypeError("Local callback has not entered".into()))}
            if self.request.values.heap().kind(receiver)?!=ValueKind::Undefined||arguments.len()!=1{return Err(ValueError::TypeError("Wrong Local update receiver/arguments".into()))}
            let value=arguments[0].clone();self.request.values.check_handle(&value).map_err(|_|ValueError::ForeignHeap)?;
            let updated=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||(self.request.on_update)(value))).unwrap_or_else(|_|Err(ActualProviderError::Callback("Local update callback panicked".into())));
            if let Err(error)=updated {
                let reply=match &error{ActualProviderError::Value(error)=>error.clone(),_=>ValueError::UnsupportedOperation("Local original update failed")};
                *self.update_failure.lock().unwrap_or_else(|e|e.into_inner())=Some(error);return Err(reply)
            }
            return self.request.values.heap().undefined().map(RuntimeReply::Value)
        }
        self.request.runtime().resolve_call(capability,receiver,arguments,effective)
    }
    fn resolve_is_constructor(&self,capability:CapabilityId,constructor:&ValueHandle,effective:&dyn ValueRuntime)->std::result::Result<bool,ValueError>{
        self.graph_check()?;self.request.runtime().resolve_is_constructor(capability,constructor,effective)
    }
    fn resolve_construct(&self,capability:CapabilityId,constructor:&ValueHandle,arguments:&[ValueHandle],effective:&dyn ValueRuntime)->std::result::Result<ValueHandle,ValueError>{
        self.graph_check()?;self.request.runtime().resolve_construct(capability,constructor,arguments,effective)
    }
    fn resolve_construct_with_new_target(&self,capability:CapabilityId,constructor:&ValueHandle,new_target:&ValueHandle,arguments:&[ValueHandle],effective:&dyn ValueRuntime)->std::result::Result<ValueHandle,ValueError>{
        self.graph_check()?;self.request.runtime().resolve_construct_with_new_target(capability,constructor,new_target,arguments,effective)
    }
    fn resolve_local_call(&self,token:u64,receiver:&ValueHandle,arguments:&[ValueHandle],effective:&dyn ValueRuntime)->std::result::Result<RuntimeReply,ValueError>{
        self.graph_check()?;self.request.runtime().resolve_local_call(token,receiver,arguments,effective)
    }
}
