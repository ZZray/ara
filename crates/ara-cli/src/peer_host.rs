//! Concrete reference Host adapter for the finite b91 Peer module.
//! MIT upstream behaviors: irc-bridge.ts, executor.ts wake monitor and persisted-revive.ts.
use super::*;
use ara_cli::host_peers::*;
use std::{collections::HashMap,sync::{Mutex,Weak,atomic::AtomicU64}};
use tokio::sync::{mpsc,oneshot};
use futures::FutureExt;
use serde_json::Value;
#[cfg(feature="test-fixture")]
use serde_json::json;
type CloseResult=Option<Result<(),String>>;
#[cfg(feature="test-fixture")]
struct HubDeviceFixture {file:PathBuf}
#[cfg(feature="test-fixture")]
#[async_trait::async_trait]
impl ara_tools::ContentUriPort for HubDeviceFixture {
    fn route(&self,url:&str)->ara_tools::ContentUriRoute{if url.starts_with("hub-device://"){ara_tools::ContentUriRoute::Registered{writable:true}}else{ara_tools::ContentUriRoute::Unregistered}}
    fn write_policy(&self,_:&str)->ara_tools::ContentUriWritePolicy{ara_tools::ContentUriWritePolicy{payload:ara_tools::ContentUriWritePayload::Verbatim,scope:ara_tools::ContentUriWriteScope::Coordination,tier:ara_tools::ContentUriWriteTier::Read}}
    async fn read(&self,_:&str,_:CancellationToken)->Result<ara_tools::UriResource,ara_agent::ToolError>{Err(ara_agent::ToolError("controlled device is write-only".into()))}
    async fn write(&self,_:&str,content:&str,cancel:CancellationToken)->Result<(),ara_agent::ToolError>{if cancel.is_cancelled(){return Err(ara_agent::ToolError("controlled device cancelled".into()));}std::fs::write(&self.file,content).map_err(|error|ara_agent::ToolError(error.to_string()))}
}

/// The actual predecessor and combined close receipt never come from model data.
struct RetiredActivation {owner:Arc<BinaryPeerHost>,receipt:RootCloseReceipt}
pub(super) struct PreparedNativePeer<'a>{
    native:&'a super::native_setup::NativeCliOwner,coordinator:Arc<PeerCoordinator>,queues:Arc<PeerQueues>,runtime:RuntimeKey,parent:Option<String>,kind:PeerKind,
    wait:ara_cli::actual_tools::wait_native::PreparedWaitBinding<'a>,
    service:Option<ara_cli::host_services::ServiceBindingPreparation<'a>>,
}
impl PreparedNativePeer<'_>{pub fn commit(self){
    self.wait.commit();if let Some(service)=self.service{service.commit();}
    self.native.peer_ingress.bind(&self.coordinator,self.queues,self.runtime,self.parent,self.kind);
}}
/// An unpublished exact endpoint is removed on failed preparation. Its actor
/// cannot process queued requests until the serial adoption method returns.
pub(super) struct PreparedPeerAttachment {pub owner:Arc<BinaryPeerHost>,pub runtime:RuntimeKey,created:bool,committed:bool}
impl PreparedPeerAttachment {pub fn commit(mut self){self.committed=true;}}
impl Drop for PreparedPeerAttachment{fn drop(&mut self){if self.created&&!self.committed{self.owner.unbind(&self.runtime);self.owner.coordinator.registry.unregister(&self.runtime.peer);}}}

pub(super) enum PeerActorOp {
    Inspect,
    #[cfg(feature="test-fixture")]
    FixturePlan(Arc<dyn ara_cli::host_edit_session::EditPlanPolicy>),
    #[cfg(feature="test-fixture")]
    FixtureUriSchemes(ara_rpc::WireValue),
    #[cfg(feature="test-fixture")]
    FixtureRefreshHostTools,
    #[cfg(feature="test-fixture")]
    FixtureNewSession(Option<String>),
    Deliver(NativeDelivery),
    RelayObservation(PeerMessage),
    Revive(PeerRecord),
    Wake(WakeRequest,CancellationToken),
    ParentResult(WakeCompletion),
    Cancel(u64),
    Close,
}
pub(super) enum PeerActorReply { State(NativePeerState), Delivered, Runtime(RuntimeKey), Wake(WakeOutcome), Adoption(ParentAdoption) }
pub(super) struct PeerActorRequest { pub runtime:RuntimeKey,pub operation:PeerActorOp,pub reply:oneshot::Sender<Result<PeerActorReply>> }
pub(super) struct PendingWake {pub request:WakeRequest,pub cancel:CancellationToken,pub reply:oneshot::Sender<Result<PeerActorReply>>,pub outcome:Option<WakeOutcome>,pub turn:Option<Arc<super::child_host::ChildTurn>>}
#[derive(Clone)]
struct OwnedTaskResource {lease:Arc<ara_cli::task_isolation::IsolationLease>,runtime:RuntimeKey,file:Option<PathBuf>}
struct Attached {
    runtime:RuntimeKey,sender:mpsc::UnboundedSender<PeerActorRequest>,
    child:Arc<tokio::sync::Mutex<Option<super::child_host::ChildHost>>>,
    file:Option<PathBuf>,
}
/// One Host ownership table. It is shared by children, never a global OS lease.
pub(super) struct BinaryPeerHost {
    pub coordinator:Arc<PeerCoordinator>,pub queues:Arc<PeerQueues>,
    attached:Mutex<HashMap<(String,String),Attached>>,sequence:Arc<AtomicU64>,
    retired:Option<RetiredActivation>,
    #[cfg(feature="test-fixture")] relay_fixture:Mutex<RelayFixtureState>,
    closes:Mutex<HashMap<RuntimeKey,tokio::sync::watch::Receiver<CloseResult>>>,
    pending_resources:Mutex<HashMap<(String,String),Arc<ara_cli::task_isolation::IsolationLease>>>,
    owned_resources:Mutex<HashMap<PeerKey,OwnedTaskResource>>,
    failed_resources:Mutex<HashMap<(String,String),Arc<ara_cli::task_isolation::IsolationLease>>>,
    resource_closes:Mutex<HashMap<PeerKey,tokio::sync::watch::Receiver<CloseResult>>>,
}
#[cfg(feature="test-fixture")]
#[derive(Default)]
struct RelayFixtureState {mode:String,held:Vec<(RuntimeKey,PeerMessage)>,attempts:Vec<(RuntimeKey,PeerMessage)>}
struct NativePort(Weak<BinaryPeerHost>);
impl BinaryPeerHost {
    pub fn new()->Arc<Self>{Self::assemble(Arc::new(AtomicU64::new(1)),None)}
    fn assemble(sequence:Arc<AtomicU64>,retired:Option<RetiredActivation>)->Arc<Self>{Arc::new_cyclic(|weak|Self{coordinator:PeerCoordinator::new(Arc::new(NativePort(weak.clone()))),queues:Arc::new(PeerQueues::default()),attached:Mutex::new(HashMap::new()),sequence,retired,#[cfg(feature="test-fixture")]relay_fixture:Mutex::new(RelayFixtureState::default()),closes:Mutex::new(HashMap::new()),pending_resources:Mutex::new(HashMap::new()),owned_resources:Mutex::new(HashMap::new()),failed_resources:Mutex::new(HashMap::new()),resource_closes:Mutex::new(HashMap::new())})}
    pub fn successor(self:&Arc<Self>,receipt:RootCloseReceipt)->Result<Arc<Self>>{
        anyhow::ensure!(receipt.settled&&self.coordinator.registry.root_is_sealed(&receipt.root)&&self.coordinator.lifecycle.root_close_snapshot(&receipt.root).is_some_and(|actual|actual.expected==receipt.expected&&actual.settled),"Peer root successor lacks exact known settlement");
        Ok(Self::assemble(self.sequence.clone(),Some(RetiredActivation{owner:self.clone(),receipt})))
    }
    fn retired_proofs(&self)->Vec<(Arc<PeerCoordinator>,RootCloseReceipt)>{
        let mut proofs=Vec::new();let mut previous=self.retired.as_ref();
        while let Some(retired)=previous{proofs.push((retired.owner.coordinator.clone(),retired.receipt.clone()));previous=retired.owner.retired.as_ref();}proofs
    }
    pub fn prepare_attachment(self:&Arc<Self>,root:&str,id:&str,parent:Option<String>,file:Option<PathBuf>,sender:mpsc::UnboundedSender<PeerActorRequest>,expected:AttachmentExpectation,reuse:Option<RuntimeKey>)->Result<PreparedPeerAttachment>{
        let created=reuse.is_none();let runtime=match reuse{Some(runtime)=>{anyhow::ensure!(!self.coordinator.registry.root_is_sealed(&runtime.peer.root)&&self.coordinator.registry.owns_runtime(&runtime),"Current Peer runtime is closed or no longer adopted");runtime},None=>self.bind_actor(root,id,parent,file,sender,expected)?};
        Ok(PreparedPeerAttachment{owner:self.clone(),runtime,created,committed:false})
    }
    pub fn reserve_task_resource(&self,root:&str,id:&str,lease:Arc<ara_cli::task_isolation::IsolationLease>)->Result<()> {
        anyhow::ensure!(!self.coordinator.registry.root_is_sealed(root),"Task root is sealed");
        let mut pending=self.pending_resources.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!pending.contains_key(&(root.into(),id.into())),"Task resource already reserved");
        pending.insert((root.into(),id.into()),lease);Ok(())
    }
    pub fn cancel_task_reservation(&self,root:&str,id:&str,lease:&Arc<ara_cli::task_isolation::IsolationLease>){
        let mut pending=self.pending_resources.lock().unwrap_or_else(|e|e.into_inner());
        let key=(root.to_owned(),id.to_owned());
        if pending.get(&key).is_some_and(|actual|Arc::ptr_eq(actual,lease)){pending.remove(&key);self.failed_resources.lock().unwrap_or_else(|e|e.into_inner()).insert(key,lease.clone());}
    }
    pub fn task_resource(&self,key:&PeerKey)->Option<Arc<ara_cli::task_isolation::IsolationLease>>{self.owned_resources.lock().unwrap_or_else(|e|e.into_inner()).get(key).map(|r|r.lease.clone())}
    pub async fn task_admission(&self,key:&PeerKey)->Result<Option<ara_cli::task_isolation::LeaseAdmission>>{match self.task_resource(key){Some(lease)=>Ok(Some(lease.admit().await?)),None=>Ok(None)}}
    pub fn child_slot(&self,runtime:&RuntimeKey)->Result<Arc<tokio::sync::Mutex<Option<super::child_host::ChildHost>>>>{self.attached.lock().unwrap_or_else(|e|e.into_inner()).get(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|r|r.runtime==*runtime).map(|r|r.child.clone()).context("Exact Task child runtime is unavailable")}
    pub async fn release_task_resource(self:&Arc<Self>,key:&PeerKey)->Result<()> {
        let(mut done,launch)={let mut flights=self.resource_closes.lock().unwrap_or_else(|e|e.into_inner());
            if let Some(done)=flights.get(key){(done.clone(),None)}else{
                let resource=self.owned_resources.lock().unwrap_or_else(|e|e.into_inner()).get(key).cloned();
                let Some(resource)=resource else{return Ok(())};
                let(tx,rx)=tokio::sync::watch::channel(None);flights.insert(key.clone(),rx.clone());(rx,Some((tx,resource)))
            }
        };
        if let Some((finished,initial))=launch{let owner=self.clone();let key=key.clone();tokio::spawn(async move{
            let operation=async{
                initial.lease.seal_admission()?;
                // Same publication mutex covers the endpoint/map update before
                // commit releases it. Re-read AFTER seal for a preceding warm bind.
                let resource=owner.owned_resources.lock().unwrap_or_else(|e|e.into_inner()).get(&key).filter(|current|Arc::ptr_eq(&current.lease,&initial.lease)).cloned().context("Sealed Task resource owner changed")?;
                let stopped=owner.close_runtime(&resource.runtime).await.map_err(|e|e.to_string());
                let receipt=resource.lease.finalize(stopped).await?;
                if let (Some(capture),Some(file))=(&receipt.capture,&resource.file){super::task_host::publish_capture(&owner,&key,file,capture)?;}
                anyhow::ensure!(receipt.status=="released","Task resource remains {}: {}",receipt.status,receipt.error.as_deref().unwrap_or("unsettled"));Ok::<(),anyhow::Error>(())
            };
            let result=match std::panic::AssertUnwindSafe(operation).catch_unwind().await{Ok(result)=>result.map_err(|e|format!("{e:#}")),Err(_)=>Err("Task release panicked; original workspace retained, effects unknown".into())};
            let _=finished.send(Some(result));
        });}
        loop{if let Some(result)=done.borrow().clone(){return result.map_err(anyhow::Error::msg);}done.changed().await.context("Task release flight lost retained receipt; outcome unknown")?;}
    }
    pub fn bind_actor(&self,root:&str,id:&str,parent:Option<String>,file:Option<PathBuf>,sender:mpsc::UnboundedSender<PeerActorRequest>,expected:AttachmentExpectation)->Result<RuntimeKey>{
        let generation=match &expected{AttachmentExpectation::Rebind(runtime)=>runtime.runtime_generation,_=>self.sequence.fetch_add(1,Ordering::AcqRel)};
        let kind=match &expected{AttachmentExpectation::Parked(old)=>old.kind,_=>if parent.is_none(){PeerKind::Main}else{PeerKind::Sub}};
        let pending_lease=self.pending_resources.lock().unwrap_or_else(|e|e.into_inner()).get(&(root.into(),id.into())).cloned();
        let lease=pending_lease.or_else(||match &expected{AttachmentExpectation::Parked(old)=>self.task_resource(&old.key),AttachmentExpectation::Rebind(runtime)=>self.task_resource(&runtime.peer),_=>None});
        let publish=lease.as_ref().map(|lease|match &expected {
            AttachmentExpectation::Parked(old)=>lease.prepare_republish(&old.key),
            AttachmentExpectation::Rebind(runtime)=>lease.prepare_republish(&runtime.peer),
            _=>lease.prepare_publish(),
        }).transpose()?;
        let mut attached=self.attached.lock().unwrap_or_else(|e|e.into_inner());
        let slot=(root.to_owned(),id.to_owned());
        if attached.get(&slot).is_some_and(|entry|!entry.sender.is_closed()&&!matches!(&expected,AttachmentExpectation::Rebind(runtime)if entry.runtime==*runtime)){bail!("Peer Session already has an adopted runtime writer");}
        let display_name=if kind==PeerKind::Main{"Main".into()}else{id.into()};
        #[cfg(feature="test-fixture")]
        let display_name=if kind==PeerKind::Sub{fixture_frontier_config()?.and_then(|(_,config)|config["coldDisplayName"].as_str().map(str::to_owned)).unwrap_or(display_name)}else{display_name};
        let input=PeerRegistration{root:root.into(),id:id.into(),parent_id:parent,display_name,kind,status:PeerStatus::Idle,runtime_generation:Some(generation),session_file:file.clone(),created_at:None,last_activity:None,history:serde_json::Value::Null};
        // Endpoint and exact ref become observable under one admission. The
        // closure is infallible, performs no callback, and never reenters registry.
        let record=self.coordinator.registry.publish_attachment(input,&expected,|record|{
            let runtime=record.runtime.clone().expect("published attachment has runtime");
            if let Some(lease)=lease.as_ref(){self.owned_resources.lock().unwrap_or_else(|e|e.into_inner()).insert(record.key.clone(),OwnedTaskResource{lease:lease.clone(),runtime:runtime.clone(),file:file.clone()});}
            attached.insert(slot,Attached{runtime,sender,child:Arc::new(tokio::sync::Mutex::new(None)),file});
            if let Some(publish)=publish{publish.commit(&record.key);}
        })?;
        drop(attached);
        self.pending_resources.lock().unwrap_or_else(|e|e.into_inner()).remove(&(root.into(),id.into()));
        self.coordinator.lifecycle.adopt(record.key.clone(),DEFAULT_IDLE_TTL_MS)?;
        record.runtime.context("Published Peer runtime is missing")
    }
    pub fn bind_native(&self,native:&super::native_setup::NativeCliOwner,runtime:&RuntimeKey,persistent:bool)->Result<()> {
        self.prepare_native(native,runtime,persistent)?.commit();Ok(())
    }
    pub fn prepare_native<'a>(&self,native:&'a super::native_setup::NativeCliOwner,runtime:&RuntimeKey,persistent:bool)->Result<PreparedNativePeer<'a>> {
        let record=self.coordinator.registry.get_exact(&runtime.peer).context("Native Peer ref missing")?;
        let wait=native.wait_session.prepare_binding(&runtime.peer.id,persistent,self.coordinator.clone(),self.queues.clone(),runtime.clone(),&self.retired_proofs())?;
        let service=native.service_session.as_ref().map(|service|service.prepare_binding(&runtime.peer.id,persistent)).transpose()?;
        Ok(PreparedNativePeer{native,coordinator:self.coordinator.clone(),queues:self.queues.clone(),runtime:runtime.clone(),parent:record.parent_id,kind:record.kind,wait,service})
    }
    pub fn unbind(&self,runtime:&RuntimeKey){let mut entries=self.attached.lock().unwrap_or_else(|e|e.into_inner());let slot=(runtime.peer.root.clone(),runtime.peer.id.clone());if entries.get(&slot).is_some_and(|entry|entry.runtime==*runtime){entries.remove(&slot);self.coordinator.registry.detach(runtime);}}
    pub fn owns_file(&self,path:&Path,current:Option<&RuntimeKey>)->bool{
        let wanted=std::fs::canonicalize(path).ok();let entries=self.attached.lock().unwrap_or_else(|e|e.into_inner());entries.values().any(|entry|Some(&entry.runtime)!=current&&!entry.sender.is_closed()&&wanted.as_ref().zip(entry.file.as_ref().and_then(|f|std::fs::canonicalize(f).ok()).as_ref()).is_some_and(|(a,b)|a==b))
    }
    pub fn attach_child_in_root(&self,root:&str,child:super::child_host::ChildHost)->Result<RuntimeKey>{
        let (runtime,slot)={let entries=self.attached.lock().unwrap_or_else(|e|e.into_inner());let entry=entries.get(&(root.into(),child.id.clone())).context("Child Native actor has no current ownership binding")?;(entry.runtime.clone(),entry.child.clone())};
        let mut held=slot.try_lock().map_err(|_|anyhow::anyhow!("Child publication is busy"))?;
        if held.is_some(){bail!("Child already attached; original runtime preserved");}*held=Some(child);Ok(runtime)
    }
    fn endpoint(&self,runtime:&RuntimeKey)->Result<mpsc::UnboundedSender<PeerActorRequest>>{let entries=self.attached.lock().unwrap_or_else(|e|e.into_inner());let entry=entries.get(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|e|e.runtime==*runtime&&!e.sender.is_closed()).context("Peer runtime generation is no longer adopted")?;Ok(entry.sender.clone())}
    pub async fn request(&self,runtime:&RuntimeKey,operation:PeerActorOp)->Result<PeerActorReply>{if self.coordinator.registry.root_is_sealed(&runtime.peer.root)&&!matches!(&operation,PeerActorOp::Inspect|PeerActorOp::Close|PeerActorOp::Cancel(_)){bail!("Peer root closed before request admission");}let endpoint=self.endpoint(runtime)?;let(reply,wait)=oneshot::channel();endpoint.send(PeerActorRequest{runtime:runtime.clone(),operation,reply}).map_err(|_|anyhow::anyhow!("Peer actor closed before admission"))?;wait.await.context("Peer actor response was lost; reconcile its journal/retained turn before retry")?}
    fn enqueue_relay_now(&self,main:&RuntimeKey,message:&PeerMessage)->Result<()>{
        anyhow::ensure!(self.coordinator.relay_main_is_current(main)&&message.from!=main.peer.id&&message.to!=main.peer.id,"Relay Main/root generation changed");
        let endpoint=self.endpoint(main)?;let(reply,_discarded_observer)=oneshot::channel();
        endpoint.send(PeerActorRequest{runtime:main.clone(),operation:PeerActorOp::RelayObservation(message.clone()),reply}).map_err(|_|anyhow::anyhow!("Relay observer closed"))
    }
    fn enqueue_relay(&self,main:&RuntimeKey,message:&PeerMessage)->Result<()>{
        anyhow::ensure!(self.coordinator.relay_main_is_current(main),"Relay Main generation changed before enqueue");
        #[cfg(feature="test-fixture")]{
            let mut state=self.relay_fixture.lock().unwrap();state.attempts.push((main.clone(),message.clone()));
            match state.mode.as_str(){"error"=>bail!("controlled relay observer error"),"drop"=>return Ok(()),"hold"=>{state.held.push((main.clone(),message.clone()));return Ok(());},_=>{}}
        }
        self.enqueue_relay_now(main,message)
    }
    fn parent_runtime(&self,root:&str,parent:&str)->Result<RuntimeKey>{self.attached.lock().unwrap_or_else(|e|e.into_inner()).get(&(root.into(),parent.into())).filter(|e|!e.sender.is_closed()).map(|e|e.runtime.clone()).context("Actual parent Host is not currently bound; revival does not borrow another root's grants")}
    pub fn uri(self:&Arc<Self>,root:&str,id:&str,file:Option<PathBuf>,eligibility:ara_cli::host_peers::agent_uri::MessagingEligibility)->Arc<dyn ara_tools::ContentUriPort>{ara_cli::host_peers::agent_uri::AgentUriPort::with_eligibility(self.coordinator.clone(),root.to_owned(),id.to_owned(),file,eligibility)}
    pub async fn wait_parent(&self,root:&str,parent:&str,cancel:&CancellationToken)->Result<Option<WakeCompletion>>{self.coordinator.jobs.wait_parent(root,parent,cancel).await}
    pub fn dispatch_parent_completions(self:&Arc<Self>,runtime:&RuntimeKey){
        for completion in self.coordinator.jobs.pending_parent(&runtime.peer.root,&runtime.peer.id){let owner=self.clone();tokio::spawn(async move{if let Err(error)=owner.coordinator.jobs.claim_and_adopt(&completion.job).await{eprintln!("ara: parent peer result remains pending: {error}");}});}
    }
    /// Exactly one retained close operation owns the ChildHost take/join.
    /// Observers may disappear, but repeated closes never replay that operation.
    pub async fn close_runtime(self:&Arc<Self>,runtime:&RuntimeKey)->Result<()> {
        let(mut done,launch)={let mut closes=self.closes.lock().unwrap_or_else(|e|e.into_inner());
            if let Some(done)=closes.get(runtime){(done.clone(),None)}else{let(tx,rx)=tokio::sync::watch::channel(None);closes.insert(runtime.clone(),rx.clone());(rx,Some(tx))}
        };
        if let Some(finished)=launch{let owner=self.clone();let runtime=runtime.clone();tokio::spawn(async move {
            let operation=async {
                let slot={let entries=owner.attached.lock().unwrap_or_else(|e|e.into_inner());entries.get(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|entry|entry.runtime==runtime).map(|entry|entry.child.clone())}.context("Peer close has no exact adopted actor or retained join receipt; outcome unknown")?;
                let child=slot.lock().await.take();
                if let Some(child)=child{let code=child.close().await?;anyhow::ensure!(code==0,"Child Native close returned failure exit {code}; persistence/effect settlement requires reconciliation");}
                else {
                    // A positive actor ACK closes admission, but without its
                    // ChildHost join owner this is not a stopped/effect receipt.
                    let _=owner.request(&runtime,PeerActorOp::Close).await?;
                    bail!("Peer actor acknowledged close without a published ChildHost join owner; outcome unknown");
                }
                owner.unbind(&runtime);Ok::<(),anyhow::Error>(())
            };
            let result=match std::panic::AssertUnwindSafe(operation).catch_unwind().await{Ok(result)=>result.map_err(|error|error.to_string()),Err(_)=>Err("Peer close owner panicked; joined/effect outcome unknown".into())};
            let _=finished.send(Some(result));
        });}
        loop {if let Some(result)=done.borrow().clone(){return result.map_err(anyhow::Error::msg);}done.changed().await.context("Peer close flight lost its retained result; outcome unknown")?;}
    }
    pub async fn release_root(self:&Arc<Self>,expected:&RuntimeKey)->Result<RootCloseReceipt>{
        let mut receipt=self.coordinator.lifecycle.release_root(&expected.peer.root,expected).await?;
        // release(key) intentionally preserves upstream warn-and-continue. Its
        // earlier close errors must still survive a later root shutdown, even
        // after a tombstone/unregister removed the library's live runtime row.
        let history={self.closes.lock().unwrap_or_else(|e|e.into_inner()).iter().filter(|(runtime,_)|runtime.peer.root==expected.peer.root).map(|(runtime,done)|(runtime.clone(),done.clone())).collect::<Vec<_>>()};
        for(runtime,mut done)in history {
            let result=loop {if let Some(result)=done.borrow().clone(){break result;}if done.changed().await.is_err(){break Err("Historical close flight ended without a join/effect receipt; outcome unknown".into());}};
            let settled=result.is_ok();let error=result.err();
            if let Some(row)=receipt.peers.iter_mut().find(|row|row.key==runtime.peer&&row.runtime.as_ref().is_none_or(|current|current==&runtime)) {
                row.runtime=Some(runtime.clone());row.settled&=settled;
                if let Some(error)=error{row.error=Some(match row.error.take(){Some(previous)if previous!=error=>format!("{previous}; retained close: {error}"),_=>error});}
            }else{receipt.peers.push(PeerCloseReceipt{key:runtime.peer.clone(),runtime:Some(runtime),settled,error});}
            receipt.settled&=settled;
        }
        let failed=self.failed_resources.lock().unwrap_or_else(|e|e.into_inner()).iter().filter(|((root,_),_)|root==&expected.peer.root).map(|((_,id),lease)|(id.clone(),lease.clone())).collect::<Vec<_>>();
        for (id,lease) in failed {
            let result=lease.finalize(Err("Task startup has no known stopped/runtime receipt".into())).await;
            let error=match result{Ok(receipt)if receipt.status=="released"=>None,Ok(receipt)=>Some(format!("Task startup retained resource {}: {:?}",lease.id(),receipt.error)),Err(error)=>Some(format!("Task startup resource {}: {error}",lease.id()))};
            if let Some(error)=error{receipt.settled=false;receipt.peers.push(PeerCloseReceipt{key:expected.peer.clone(),runtime:Some(expected.clone()),settled:false,error:Some(format!("Root retains unpublished Task Agent {id}: {error}"))});}
        }
        let resources=self.owned_resources.lock().unwrap_or_else(|e|e.into_inner()).keys().filter(|key|key.root==expected.peer.root).cloned().collect::<Vec<_>>();
        for key in resources{if let Err(error)=self.release_task_resource(&key).await{receipt.settled=false;if let Some(row)=receipt.peers.iter_mut().find(|row|row.key==key){row.settled=false;row.error=Some(format!("{}; resource: {error}",row.error.as_deref().unwrap_or("")));}else{receipt.peers.push(PeerCloseReceipt{key,runtime:None,settled:false,error:Some(format!("retained resource: {error}"))});}}}
        Ok(receipt)
    }
    pub async fn close_all(&self)->Result<()>{let slots={self.attached.lock().unwrap_or_else(|e|e.into_inner()).values().map(|entry|(entry.runtime.clone(),entry.child.clone())).collect::<Vec<_>>()};for(runtime,slot)in slots{let child=slot.lock().await.take();if let Some(child)=child{child.close().await?;}self.unbind(&runtime);}Ok(())}
}
#[async_trait]
impl PeerNativePort for NativePort {
    async fn release_owned(&self,record:&PeerRecord)->Result<()>{self.0.upgrade().context("Task resource Host closed")?.release_task_resource(&record.key).await}
    fn observe_main_relay(&self,main:&RuntimeKey,message:&PeerMessage)->Result<()>{self.0.upgrade().context("Relay Host closed")?.enqueue_relay(main,message)}
    async fn inspect(&self,runtime:&RuntimeKey)->Result<NativePeerState>{let host=self.0.upgrade().context("Peer Host closed")?;match host.request(runtime,PeerActorOp::Inspect).await?{PeerActorReply::State(state)=>Ok(state),_=>bail!("Wrong peer state receipt")}}
    async fn deliver(&self,runtime:&RuntimeKey,delivery:NativeDelivery)->Result<()>{let host=self.0.upgrade().context("Peer Host closed")?;let _lease_admission=if delivery.route==DeliveryRoute::PlanAppend{host.task_admission(&runtime.peer).await?}else{None};match host.request(runtime,PeerActorOp::Deliver(delivery)).await?{PeerActorReply::Delivered=>Ok(()),_=>bail!("Wrong peer delivery receipt")}}
    async fn revive(&self,record:&PeerRecord)->Result<RuntimeKey>{let host=self.0.upgrade().context("Peer Host closed")?;let _lease_admission=host.task_admission(&record.key).await?;let parent=host.parent_runtime(&record.key.root,record.parent_id.as_deref().context("Main has no cold child factory")?)?;match host.request(&parent,PeerActorOp::Revive(record.clone())).await?{PeerActorReply::Runtime(runtime)=>Ok(runtime),_=>bail!("Wrong peer revival receipt")}}
    async fn close(&self,runtime:&RuntimeKey)->Result<()>{let host=self.0.upgrade().context("Peer Host closed")?;host.close_runtime(runtime).await}
    async fn cancel_turn(&self,runtime:&RuntimeKey,turn:u64)->Result<()>{let host=self.0.upgrade().context("Peer Host closed")?;match host.request(runtime,PeerActorOp::Cancel(turn)).await?{PeerActorReply::Delivered=>Ok(()),_=>bail!("Wrong peer cancellation receipt")}}
    async fn drive_wake(&self,request:WakeRequest,cancel:CancellationToken)->Result<WakeOutcome>{let host=self.0.upgrade().context("Peer Host closed")?;let runtime=request.runtime.clone();let _lease_admission=host.task_admission(&runtime.peer).await?;match host.request(&runtime,PeerActorOp::Wake(request,cancel)).await?{PeerActorReply::Wake(outcome)=>Ok(outcome),_=>bail!("Wrong exact peer wake receipt")}}
    async fn transcript(&self,record:&PeerRecord)->Result<Vec<serde_json::Value>>{let file=record.session_file.as_ref().context("Peer has no transcript")?;let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;anyhow::ensure!(journal.session_id()==record.key.id,"Transcript id differs from peer ref");journal.entries().iter().filter_map(|entry|match entry.observed_message_body(){Ok(Some(body))=>Some(ara_session::native_checked_serde(&body).map_err(Into::into)),Ok(None)=>None,Err(error)=>Some(Err(error.into()))}).collect()}
    async fn can_revive(&self,record:&PeerRecord)->Result<bool>{
        let Some(file)=&record.session_file else{return Ok(false)};
        #[cfg(feature="test-fixture")]
        if let Some(error)=fixture_probe_error(file){return Err(error.into());}
        let metadata=match std::fs::metadata(file){Ok(metadata)=>metadata,Err(error)if matches!(error.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::NotADirectory)=>return Ok(false),Err(error)=>return Err(error.into())};
        if !metadata.is_file(){return Ok(false);}
        let journal=match SessionJournal::open_with_blob_directory(file,&ara_blobs_directory()){
            Ok(journal)=>journal,Err(ara_session::SessionError::Corrupt{..})=>return Ok(false),Err(error)=>return Err(error.into()),
        };
        if journal.session_id()!=record.key.id{return Ok(false);}
        let Some(init)=journal.latest_child_init()? else{return Ok(false)};
        if init.isolated==Some(true){let host=self.0.upgrade().context("Task resource Host closed")?;if host.task_resource(&record.key).is_none(){return Ok(false);}}
        let Some(cwd)=journal.header()["cwd"].as_str().map(PathBuf::from)else{return Ok(false)};
        let cwd=match std::fs::metadata(cwd){Ok(metadata)=>metadata,Err(error)if matches!(error.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::NotADirectory)=>return Ok(false),Err(error)=>return Err(error.into())};
        if !cwd.is_dir(){return Ok(false);}
        if !journal.entries().iter().any(|entry|entry.kind=="message"||entry.kind=="custom_message"){return Ok(false);}
        let host=self.0.upgrade().context("Peer Host closed")?;
        let parent_id=record.parent_id.as_ref().context("Non-child cannot revive")?;
        let parent=host.coordinator.registry.get(&record.key.root,parent_id).and_then(|parent|parent.session_file).context("Actual parent identity temporarily unavailable; revival remains unknown")?;
        if init.extra.get("araChildParentId").and_then(Value::as_str)!=Some(parent_id.as_str()){return Ok(false);}
        let Some(expected)=journal.header()["parentSession"].as_str().map(PathBuf::from)else{return Ok(false)};
        if std::fs::canonicalize(expected)?!=std::fs::canonicalize(parent)?{return Ok(false);}
        Ok(true)
    }
    async fn persist_parent_result(&self,completion:&WakeCompletion)->Result<ParentAdoption>{let host=self.0.upgrade().context("Peer Host closed")?;let parent=host.parent_runtime(&completion.job.child.root,&completion.job.parent_id)?;match host.request(&parent,PeerActorOp::ParentResult(completion.clone())).await?{PeerActorReply::Adoption(adoption)=>Ok(adoption),_=>bail!("Wrong parent adoption receipt")}}
}

/// This per-Native-owner inbox is reused on same-realm catalog refresh. Its
/// binding is replaced only by actual actor adoption, never by a tool payload.
#[derive(Default)]
pub(super) struct PeerIngress {
    binding:Mutex<Option<(Arc<PeerQueues>,RuntimeKey,Option<String>,PeerKind)>>,
    coordinator:Mutex<Option<Weak<PeerCoordinator>>>,
    parent_queue:Mutex<std::collections::VecDeque<PendingParentResult>>,
    parent_claims:Mutex<HashMap<ara_agent::AgentMessageIdentity,ParentResultClaim>>,
}
pub(super) struct PendingParentResult {pub completion:WakeCompletion,pub reply:oneshot::Sender<Result<PeerActorReply>>}
struct ParentResultClaim {pending:PendingParentResult,committed:bool}
pub(super) fn parent_result_body(completion:&WakeCompletion)->serde_json::Value {
    serde_json::json!({"role":"custom","customType":"peer-task-result","content":format!("<system-notice>\n{}\n</system-notice>",ara_cli::host_peers::native::escape_harness_tags(&completion.outcome.relay_body(&completion.job.child.id,false))),"display":true,"attribution":"agent","timestamp":ara_cli::host_peers::now_ms(),"details":{"peerWakeJob":completion.job,"peerWakeRuntime":completion.runtime}})
}
impl PeerIngress {
    pub fn enqueue_parent(&self,pending:PendingParentResult){self.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).push_back(pending);}
    pub fn take_parent(&self)->Option<PendingParentResult>{self.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).pop_front()}
    pub fn is_parent_claim(&self,message:&ara_agent::AgentMessage)->bool {self.parent_claims.lock().unwrap_or_else(|e|e.into_inner()).get(&message.weak_identity()).is_some_and(|claim|claim.committed)}
    pub fn append_parent_receipt(&self,message:&ara_agent::AgentMessage,receipt:Result<ParentAdoption>){if let Some(claim)=self.parent_claims.lock().unwrap_or_else(|e|e.into_inner()).remove(&message.weak_identity()){let result=receipt.and_then(|receipt|{anyhow::ensure!(claim.committed&&receipt.session_id==claim.pending.completion.job.parent_id,"Parent claim owner differs from durable receipt");if let Some(coordinator)=self.coordinator.lock().unwrap_or_else(|e|e.into_inner()).as_ref().and_then(Weak::upgrade){coordinator.jobs.reconcile_adoption(&claim.pending.completion.job,&claim.pending.completion.runtime,receipt.clone())?;}
            Ok(PeerActorReply::Adoption(receipt))});let _=claim.pending.reply.send(result);}}
    pub fn release_unappended(&self){let claims=std::mem::take(&mut *self.parent_claims.lock().unwrap_or_else(|e|e.into_inner()));for(_,claim)in claims {let _=claim.pending.reply.send(Err(anyhow::anyhow!("Run ended before parent completion durable adoption; reconcile exact job before retry")));}}
    pub fn bind(&self,coordinator:&Arc<PeerCoordinator>,queues:Arc<PeerQueues>,runtime:RuntimeKey,parent:Option<String>,kind:PeerKind){
        *self.coordinator.lock().unwrap_or_else(|e|e.into_inner())=Some(Arc::downgrade(coordinator));
        *self.binding.lock().unwrap_or_else(|e|e.into_inner())=Some((queues,runtime,parent,kind));
    }
    pub fn asides(self:&Arc<Self>,projector:Arc<ara_cli::agent_message_projection::AgentMessageProjector>,prepared:Option<Arc<ara_cli::native_factory::PreparedNativeSession>>)->Result<Vec<ara_agent::AsideMessage>,ara_agent::AsideError>{
        // This producer can run after a tool refresh whose temporary projector
        // has no journal encoder. Admit new Host input in its current Session
        // values, like the existing edit/service producers; never at request time.
        let input=|body:serde_json::Value|->Result<ara_agent::AgentMessage,ara_agent::RawContextError>{
            if let Some(prepared)=prepared.as_ref(){
                ara_session::Entry::project_agent_message_body(&body).map_err(|error|ara_agent::RawContextError(error.to_string()))?;
                let source=Some(Arc::new(serde_json::json!({"origin":body.get("customType"),"attribution":body.get("attribution")})));
                let root=prepared.values().heap().import_serde(&body).map_err(|error|ara_agent::RawContextError(error.to_string()))?;
                ara_agent::AgentMessage::from_graph(root,source).map_err(|error|ara_agent::RawContextError(error.to_string()))
            }else{projector.new_custom_input(body)}
        };
        let Some((queues,runtime,parent,kind))=self.binding.lock().unwrap_or_else(|e|e.into_inner()).clone()else{return Ok(Vec::new())};
        let records=queues.drain_injection(&runtime);let mut projected=Vec::new();
        for record in &records {let body=serde_json::json!({"role":"custom","customType":"irc","content":record.rendered,"display":true,"attribution":"agent","timestamp":record.message.ts,"details":ara_cli::host_peers::native::incoming_details(&record.message,parent.as_deref()==Some(record.message.from.as_str()),record.route==DeliveryRoute::SiblingInterrupt,kind!=PeerKind::Main&&!record.message.wake_relay)});match input(body){Ok(message)=>projected.push(message),Err(error)=>{queues.restore(&runtime,QueueSnapshot{asides:records.clone(),..Default::default()});return Err(ara_agent::AsideError(error.to_string()));}}}
        for(message,record)in projected.iter().zip(records){let queues=queues.clone();let runtime=runtime.clone();message.set_aside_callbacks(ara_agent::AsideCallbacks{commit:None,discard:Some(Arc::new(move|_|{queues.restore(&runtime,QueueSnapshot{asides:vec![record.clone()],..Default::default()});Ok(())}))});}
        while let Some(pending)=self.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).pop_front() {
            let message=match input(parent_result_body(&pending.completion)) {Ok(message)=>message,Err(error)=>{let _=pending.reply.send(Err(anyhow::anyhow!("Parent completion projection failed: {error}")));continue;}};
            let identity=message.weak_identity();self.parent_claims.lock().unwrap_or_else(|e|e.into_inner()).insert(identity.clone(),ParentResultClaim{pending,committed:false});
            let commit=Arc::downgrade(self);let discard=commit.clone();let commit_id=identity.clone();
            message.set_aside_callbacks(ara_agent::AsideCallbacks{commit:Some(Arc::new(move||{if let Some(owner)=commit.upgrade(){if let Some(claim)=owner.parent_claims.lock().unwrap_or_else(|e|e.into_inner()).get_mut(&commit_id){claim.committed=true;}}Ok(())})),discard:Some(Arc::new(move|error|{if let Some(owner)=discard.upgrade(){if let Some(claim)=owner.parent_claims.lock().unwrap_or_else(|e|e.into_inner()).remove(&identity){let _=claim.pending.reply.send(Err(anyhow::anyhow!("Parent completion context discarded: {error}")));}}Ok(())}))});projected.push(message);
        }
        Ok(projected.into_iter().map(ara_agent::AsideMessage::Ready).collect())
    }
}
#[async_trait]
impl ara_agent::IrcInterruptPeek for PeerIngress {async fn queued(&self)->Result<bool,ara_agent::ToolHookError>{
    // Automatic parent adoption has its own jobs notification and Wait notice.
    // It is not a Peer inbox interrupt: treating it as one skips an unentered
    // Native Wait before that owner can return its nonconsuming delivery notice.
    Ok(self.binding.lock().unwrap_or_else(|e|e.into_inner()).as_ref().is_some_and(|(queues,runtime,_,_)|queues.has_interrupts(runtime)))
}}
/// Test-only observer at a real production frontier. It never creates a receipt.
#[cfg(feature="test-fixture")]
fn fixture_frontier_config()->Result<Option<(PathBuf,serde_json::Value)>>{
 let Some(path)=std::env::var_os("ARA_TEST_PEER_FRONTIERS").map(PathBuf::from)else{return Ok(None)};
 let bytes=match std::fs::read(&path){Ok(bytes)=>bytes,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(None),Err(error)=>return Err(error.into())};
 Ok(Some((path,serde_json::from_slice(&bytes)?)))
}
#[cfg(feature="test-fixture")]
fn fixture_occurrence(key:&str)->u64{
 static COUNTS:std::sync::OnceLock<Mutex<HashMap<String,u64>>>=std::sync::OnceLock::new();
 let mut counts=COUNTS.get_or_init(Default::default).lock().unwrap_or_else(|e|e.into_inner());let count=counts.entry(key.into()).or_default();*count+=1;*count
}
#[cfg(feature="test-fixture")]
pub(super) async fn fixture_frontier(name:&str,identity:&str)->Result<()>{
 let Some((config_path,config))=fixture_frontier_config()?else{return Ok(())};
 for gate in config["gates"].as_array().into_iter().flatten(){
  if gate["name"].as_str()!=Some(name)||!identity.contains(gate["identityContains"].as_str().unwrap_or("")){continue;}
  let key=gate["key"].as_str().context("Frontier gate key missing")?;anyhow::ensure!(key.chars().all(|c|c.is_ascii_alphanumeric()||c=='-'||c=='_'),"Invalid private frontier gate key");
  let occurrence=fixture_occurrence(key);if occurrence!=gate["occurrence"].as_u64().unwrap_or(1){continue;}
  let dir=config_path.parent().context("Private frontier config directory missing")?.join("frontiers");std::fs::create_dir_all(&dir)?;
  std::fs::write(dir.join(format!("{key}.reached.json")),serde_json::to_vec_pretty(&json!({"name":name,"identity":identity,"occurrence":occurrence,"at":ara_cli::host_peers::now_ms()}))?)?;
  tokio::time::timeout(Duration::from_secs(15),async{while !dir.join(format!("{key}.release")).is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}Ok::<(),anyhow::Error>(())}).await.context("Actual frontier remained held; effect settlement unknown, no replay")??;
  if gate["failAfterRelease"]==true{bail!("Controlled actual frontier failure after {name}; effect outcome requires reconciliation");}
 }
 Ok(())
}
#[cfg(feature="test-fixture")]
pub(super) fn fixture_probe_error(path:&Path)->Option<std::io::Error>{
 let (_,config)=match fixture_frontier_config(){Ok(Some(config))=>config,Ok(None)=>return None,Err(error)=>return Some(std::io::Error::other(error.to_string()))};
 for fault in config["probeFaults"].as_array().into_iter().flatten(){if !path.to_string_lossy().contains(fault["pathContains"].as_str().unwrap_or("__never__")){continue;}
  let key=fault["key"].as_str().unwrap_or("probe-invalid");if fixture_occurrence(key)!=fault["occurrence"].as_u64().unwrap_or(1){continue;}
  let kind=match fault["kind"].as_str(){Some("PermissionDenied")=>std::io::ErrorKind::PermissionDenied,Some("NotFound")=>std::io::ErrorKind::NotFound,_=>std::io::ErrorKind::Other};return Some(std::io::Error::new(kind,"controlled actual peer probe fault"));
 }None
}
#[cfg(feature="test-fixture")]
fn fixture_events(path:&Path)->Result<Vec<serde_json::Value>>{
 let raw=match std::fs::read_to_string(path){Ok(raw)=>raw,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),Err(error)=>return Err(error.into())};
 raw.split_inclusive('\n').filter(|line|line.ends_with('\n')).map(|line|serde_json::from_str(line).map_err(Into::into)).collect()
}
#[cfg(feature="test-fixture")]
async fn fixture_observe(mut check:impl FnMut()->Result<bool>)->Result<()>{tokio::time::timeout(Duration::from_secs(15),async{loop{if check()?{return Ok::<(),anyhow::Error>(());}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.context("Actual Peer observation timed out; inspect original receipts")?}
#[cfg(feature="test-fixture")]
fn fixture_configure(dir:&Path,gates:serde_json::Value,probes:serde_json::Value)->Result<()>{std::fs::write(dir.join("frontiers.json"),serde_json::to_vec_pretty(&json!({"gates":gates,"probeFaults":probes}))?)?;Ok(())}
#[cfg(feature="test-fixture")]
fn fixture_release(dir:&Path,key:&str)->Result<()>{std::fs::create_dir_all(dir.join("frontiers"))?;std::fs::write(dir.join("frontiers").join(format!("{key}.release")),b"release original operation after observation")?;Ok(())}
#[cfg(feature="test-fixture")]
fn fixture_message_call(message:&serde_json::Value,id:&str,args:&serde_json::Value)->bool{message.get("content").and_then(Value::as_array).is_some_and(|blocks|blocks.iter().any(|b|b["type"]=="toolCall"&&b["id"]==id&&b["name"]=="yield"&&b["arguments"]==*args))}

#[cfg(feature="test-fixture")]
async fn fixture_close_fault(owner:&Arc<BinaryPeerHost>,record:&PeerRecord,output_dir:&Path)->Result<serde_json::Value>{fixture_close_fault_owned(owner,record,output_dir,false).await}
#[cfg(feature="test-fixture")]
async fn fixture_close_fault_owned(owner:&Arc<BinaryPeerHost>,record:&PeerRecord,output_dir:&Path,resource_owned:bool)->Result<serde_json::Value>{
 let runtime=record.runtime.clone().context("Unknown-close Native runtime missing")?;fixture_configure(output_dir,json!([{"name":"child-close-before-join","identityContains":record.key.id,"key":"unknown-close","failAfterRelease":true}]),json!([]))?;
 let closing_owner=owner.clone();let first_runtime=runtime.clone();let observer=tokio::spawn(async move{closing_owner.close_runtime(&first_runtime).await});fixture_observe(||Ok(output_dir.join("frontiers/unknown-close.reached.json").is_file())).await?;
 let original={owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(&runtime).context("Actual original retained close flight missing")?.clone()};anyhow::ensure!(original.borrow().is_none(),"Original close was not actually held before join");observer.abort();let observer_dropped=observer.await.is_err();
 let joining_owner=owner.clone();let second_runtime=runtime.clone();let joined=tokio::spawn(async move{joining_owner.close_runtime(&second_runtime).await});tokio::task::yield_now().await;anyhow::ensure!(!joined.is_finished(),"Dropped observer released original held close");let same_held={owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(&runtime).is_some_and(|current|original.same_channel(current))};anyhow::ensure!(same_held,"Duplicate close replaced original retained flight");
 fixture_release(output_dir,"unknown-close")?;let duplicate_error=joined.await?.err().context("Injected close-after-take did not retain Unknown")?.to_string();let original_error=original.borrow().clone().context("Original retained close did not settle")?.err().context("Original after-effect failure became success")?;
 anyhow::ensure!(original_error=="Controlled actual frontier failure after child-close-before-join; effect outcome requires reconciliation"&&duplicate_error==original_error,"Duplicate close returned another missing-owner error instead of original after-effect diagnostic");
 let same_final={owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(&runtime).is_some_and(|current|original.same_channel(current)&&current.borrow().as_ref()==Some(&Err(original_error.clone())))};anyhow::ensure!(same_final,"Final retained close channel/result changed");
 let release=owner.coordinator.lifecycle.release(&record.key,false).await;
 let(released,release_error)=if resource_owned{let error=release.err().context("Unknown actual Task close incorrectly allowed owned-resource release")?;anyhow::ensure!(owner.task_resource(&record.key).is_some(),"Unknown Task release discarded its exact resource owner");(false,Some(format!("{error:#}")))}else{let released=release?;anyhow::ensure!(released&&owner.coordinator.registry.get_exact(&record.key).is_none(),"Library release did not unregister original child while retaining close ledger");(released,None)};
 fixture_configure(output_dir,json!([]),json!([]))?;
 Ok(json!({"runtime":runtime,"observerDropped":observer_dropped,"originalError":original_error,"duplicateError":duplicate_error,"sameHeldChannel":same_held,"sameFinalChannel":same_final,"released":released,"releaseError":release_error}))
}
#[cfg(feature="test-fixture")]
fn fixture_root_close_state(owner:&Arc<BinaryPeerHost>,root:&str)->serde_json::Value{
 let closes=owner.closes.lock().unwrap_or_else(|e|e.into_inner()).iter().filter(|(runtime,_)|runtime.peer.root==root).map(|(runtime,done)|json!({"runtime":runtime,"result":done.borrow().clone()})).collect::<Vec<_>>();
 let attached=owner.attached.lock().unwrap_or_else(|e|e.into_inner()).values().filter(|entry|entry.runtime.peer.root==root).map(|entry|json!({"runtime":entry.runtime,"senderClosed":entry.sender.is_closed()})).collect::<Vec<_>>();
 json!({"rootReceipt":owner.coordinator.lifecycle.root_close_snapshot(root),"jobs":owner.coordinator.jobs.snapshot_root(root),"active":owner.coordinator.jobs.active_root(root),"registry":owner.coordinator.registry.list(root),"closes":closes,"attached":attached})
}
#[cfg(feature="test-fixture")]
fn fixture_root_close_original(owner:&Arc<BinaryPeerHost>,runtime:&RuntimeKey,receipt:&RootCloseReceipt,error:&str)->Result<serde_json::Value>{
 let row=receipt.peers.iter().find(|row|row.runtime.as_ref()==Some(runtime)&&row.key==runtime.peer).context("Root receipt lost exact historical child runtime")?;anyhow::ensure!(!receipt.settled&&!row.settled&&row.error.as_deref().is_some_and(|actual|actual.contains(error)&&!actual.contains("missing")&&!actual.contains("without a published ChildHost")),"Root retained another error instead of original actual close diagnostic");let done=owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(runtime).cloned().context("Root discarded original close flight")?;anyhow::ensure!(done.borrow().as_ref()==Some(&Err(error.to_owned())),"Root changed retained original close result");Ok(json!({"runtime":runtime,"row":row,"retainedOriginalError":error}))
}

/// Five-bank entry exercises the same production RPC actor and Native factories.
/// Only selected upstream faults/response bodies are controlled by a loopback server.
#[cfg(feature="test-fixture")]
pub(super) fn run_fixture<'a>(config:AgentConfig,skills:Vec<ara_discovery::LoadedSkill>,messages:Vec<AgentMessage>,journal:Option<SessionJournal>,header:serde_json::Value,max_time:Option<f64>,sessions:super::rpc_host::SessionFactory,scenario:&'a Path)->std::pin::Pin<Box<impl std::future::Future<Output=Result<i32>>+'a>>{
    // Keep the large fixture future out of the ordinary startup poll frame.
    Box::pin(run_fixture_owned(config,skills,messages,journal,header,max_time,sessions,scenario))
}
#[cfg(feature="test-fixture")]
async fn run_fixture_owned(mut config:AgentConfig,skills:Vec<ara_discovery::LoadedSkill>,messages:Vec<AgentMessage>,mut journal:Option<SessionJournal>,header:serde_json::Value,max_time:Option<f64>,mut sessions:super::rpc_host::SessionFactory,scenario:&Path)->Result<i32>{
    use ara_cli::host_peers::now_ms;
    let root_id=header["id"].as_str().context("Peer fixture root id missing")?.to_owned();
    let root_file=journal.as_ref().map(|journal|journal.path().to_owned()).context("Peer fixture needs persistent root")?;
    let owner=BinaryPeerHost::new();sessions.peers=Some(owner.clone());sessions.peer_root=Some(root_id.clone());
    let context=sessions.tool_owner.context.clone().context("Peer fixture needs actual root ToolContext")?;
    let operations:Vec<serde_json::Value>=serde_json::from_slice(&std::fs::read(scenario)?)?;
    // One existing control needs a genuine ordinary unscoped root owner, then
    // a later configured child catalog. Keep the root's real startup route;
    // install the pending synthetic catalog through the existing same-backend
    // offline refresh before any child creation. No selection/auth flag is forged.
    let deferred_catalog=if operations.iter().any(|op|op["op"]=="cold_setup"&&op["deferModels"]==true){
        anyhow::ensure!(sessions.agent_context.is_none(),"Deferred catalog requires the actual unscoped root");
        let target=ara_home().join("agent/models.yml");
        anyhow::ensure!(!target.exists(),"Deferred catalog was present during ordinary root startup");
        let pending=scenario.parent().context("Cold fixture directory missing")?.join("cold-models.pending.json");
        let registry=sessions.provider.registry.as_ref().context("Actual broker-backed root Registry missing")?;
        anyhow::ensure!(registry.config().is_none(),"Deferred root did not start without models configuration");
        let before=registry.session_scope_receipt();
        std::fs::copy(&pending,&target)?;
        registry.runtime().refresh(ara_cli::model_manager::ModelRefreshStrategy::Offline).await?;
        anyhow::ensure!(registry.config_error().is_none()&&registry.find_reference(&"cold-fixture".into(),&"fake-model".into())?.is_some(),"Pending configured child catalog did not load through the actual Registry");
        json!({"startupConfigAbsent":true,"beforeRegistry":before,"afterRegistry":registry.session_scope_receipt(),"installed":"standard apiKey catalog through offline refresh"})
    }else{Value::Null};
    // Only the controlled fixture can give a root a scoped creation input;
    // production roots retain None. This reaches the normal route/Session owners.
    if let Some(setup)=operations.iter().find(|op|op["op"]=="cold_setup"&&op["rootScope"]==true){
        let pools:ara_cli::auth_storage::OAuthAccountPools=serde_json::from_value(setup["rootPools"].clone())?;
        let agent=Arc::new(super::rpc_host::HostAgentContext{identity:super::rpc_host::HostAgentIdentity{definition_name:Some("scoped-fixture-root".into()),effective_name:"scoped-fixture-root".into()},auth_scope:Arc::new(ara_cli::auth_storage::AuthSessionScope{primary_session_id:root_id.clone(),oauth_account_pools:pools})});
        let mut args=sessions.args.clone();let mut account=sessions.provider.shared_account.clone();
        let mut route=resolve_startup_route_scoped(&mut args,&sessions.cwd,&mut account,&CancellationToken::new(),journal.as_ref(),Some(agent.clone()),sessions.provider.registry.clone(),false).await?;
        let selection=route.daily.take().context("Scoped actual root fixture requires normal daily route")?;
        let provider=ProviderFactory::daily_scoped(sessions.provider.client.clone(),selection,Some(root_id.clone()),&sessions.cwd,&CancellationToken::new(),account,route.registry.clone(),Some(agent.clone())).await?;
        config.provider=provider.build();config.model=route.model;sessions.provider=provider;sessions.agent_context=Some(agent);
    }
    // A snapshot-only fixture needs a real on-disk switch target even though
    // ordinary Session journals intentionally remain lazy before model work.
    if operations.len()==1&&operations[0]["op"]=="root_inspect"{journal.as_mut().context("Snapshot-only fixture requires its actual persistent root journal")?.materialize()?;}
    let mut config=config;if let Some(mode)=operations.iter().find_map(|op|op["interruptMode"].as_str()){config.interrupt_mode=match mode{"immediate"=>ara_agent::InterruptMode::Immediate,"wait"=>ara_agent::InterruptMode::Wait,_=>bail!("Invalid actual fixture interrupt mode")};}
    if let Some(setup)=operations.iter().find(|op|op["op"]=="hub_setup"){
        sessions.peer_messaging_enabled=setup["enableIrc"].as_bool().unwrap_or(true);
        if setup["device"]==true{
            let device:Arc<dyn ara_tools::ContentUriPort>=Arc::new(HubDeviceFixture{file:scenario.parent().context("Hub fixture directory missing")?.join("hub-device-written.txt")});
            sessions.tool_owner.uri_base=Some(ara_cli::host_vault::compose_ports(device,sessions.tool_owner.uri_base.clone()));
        }
    }
    let root_ingress=sessions.tool_owner.native.as_ref().context("Peer fixture needs actual Native ingress owner")?.peer_ingress.clone();
    let output_dir=scenario.parent().context("Peer fixture directory missing")?.to_owned();let frames=output_dir.join("root.events.jsonl");
    let root=super::rpc_host::spawn_native_session(config,skills,messages,journal,header,max_time,sessions,Arc::new(move|frame|{use std::io::Write;if let Ok(mut file)=std::fs::OpenOptions::new().append(true).create(true).open(&frames){let _=writeln!(file,"{}",frame.stringify());}}));
    root.handle.call(super::rpc_host::NativeAction::State).await?;
    let mut aliases=HashMap::<String,String>::new();aliases.insert("Main".into(),root_id.clone());let mut receipts=Vec::new();let mut unresolved_terminal=false;
    let mut task_keys=HashMap::<String,PeerKey>::new();
    for(index,operation)in operations.into_iter().enumerate(){
        let action=operation["op"].as_str().context("Peer fixture op missing")?.to_owned();let alias=operation["alias"].as_str().unwrap_or("child").to_owned();
        let started=now_ms();
        let future=async{let outcome:Result<serde_json::Value>=async{match action.as_str(){
            "create"=>{let result=root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"create".into(),init:Some(operation["init"].clone()),target:None,context:operation["context"].as_str().map(str::to_owned),structured:operation["structured"].as_bool().unwrap_or(false)}).await?;let runtime:RuntimeKey=serde_json::from_value(result)?;aliases.insert(alias.clone(),runtime.peer.id.clone());Ok(serde_json::to_value(owner.coordinator.registry.get_exact(&runtime.peer))?)},
            "task_alias"|"task_snapshot"=>{
                let key=if action=="task_alias"{
                    let mut candidates=Vec::new();
                    for record in owner.coordinator.registry.list(&root_id){
                        if record.parent_id.as_deref()!=Some(root_id.as_str())||task_keys.values().any(|key|key==&record.key){continue;}
                        let Some(file)=&record.session_file else{continue};
                        let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;
                        if journal.latest_child_init()?.is_some_and(|init|init.extra.contains_key("araTaskExecutionId")){candidates.push(record.key);}
                    }
                    let key=candidates.into_iter().nth(operation["index"].as_u64().unwrap_or(0)as usize).context("Actual Native Task child was not created")?;
                    aliases.insert(alias.clone(),key.id.clone());task_keys.insert(alias.clone(),key.clone());key
                }else{task_keys.get(&alias).context("Actual Task alias missing")?.clone()};
                let record=owner.coordinator.registry.get_exact(&key).context("Exact Task reference changed")?;
                let file=record.session_file.as_ref().context("Actual Task journal missing")?;
                let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;
                let init=journal.latest_child_init()?.context("Actual Task session_init missing")?;
                let execution=init.extra["araTaskExecutionId"].as_str().context("Actual Task execution id missing")?;
                let receipt=root_file.with_extension("").join(format!("task-{execution}")).join("result.json");
                let result_receipt=match std::fs::read(&receipt){Ok(bytes)=>Some(serde_json::from_slice::<Value>(&bytes)?),Err(error)if error.kind()==std::io::ErrorKind::NotFound=>None,Err(error)=>return Err(error.into())};
                Ok(json!({"record":record,"init":init,"resource":owner.task_resource(&key).map(|lease|lease.snapshot()),
                    "rawJournal":std::fs::read_to_string(file)?,"resultReceipt":result_receipt,"resultReceiptPath":receipt}))
            },
            "task_cold_probe"=>{
                let key=task_keys.get(&alias).context("Actual Task exact alias missing")?;
                root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"task_cold_probe".into(),init:None,target:Some(key.id.clone()),context:None,structured:false}).await
            },
            "task_release"=>{
                let key=task_keys.get(&alias).context("Actual Task exact alias missing")?.clone();
                let before=owner.coordinator.registry.get_exact(&key);
                let lease=owner.task_resource(&key).context("Actual Task resource missing")?;
                let resource_before=lease.snapshot();let cwd=lease.cwd().to_owned();
                if operation["observerDrop"]==true{
                    let releasing=owner.clone();let expected=key.clone();
                    let observer=tokio::spawn(async move{releasing.release_task_resource(&expected).await});
                    fixture_observe(||Ok(owner.resource_closes.lock().unwrap_or_else(|e|e.into_inner()).contains_key(&key))).await?;observer.abort();
                }
                let released=owner.coordinator.lifecycle.release(&key,false).await;
                // Finalize joins the original retained release; no new parent apply.
                let release=lease.finalize(released.as_ref().map(|_|()).map_err(|error|format!("{error:#}"))).await?;
                Ok(json!({"before":before,"resourceBefore":resource_before,"resourceAfter":lease.snapshot(),
                    "releaseReceipt":release,"releaseError":released.err().map(|error|format!("{error:#}")),"cwdExists":cwd.exists()}))
            },
            "relay_prepare"|"relay_release"|"relay_snapshot"|"relay_concrete_guards"=>fixture_relay_operation(&action,&operation,&owner,&root,&root_id,&root_file,&output_dir,&aliases).await,
            "hub_setup"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"hub_refresh".into(),init:None,target:None,context:None,structured:false}).await,
            "hub_self_probe"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"hub_self_probe".into(),init:None,target:None,context:None,structured:false}).await,
            "hub_direct_probe"=>{
                let child=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Hub direct target missing")?).context("Hub direct record missing")?;
                let sent=root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"hub_direct_probe".into(),init:None,target:Some(child.key.id.clone()),context:None,structured:false}).await?;
                let receipts=sent["nativeResult"]["details"]["message"]["receipts"].as_array().context("Native direct send receipts missing")?;
                anyhow::ensure!(receipts.len()==1&&receipts[0]["to"]==child.key.id&&receipts[0]["outcome"]=="woken","Native direct send lost immediate wake receipt");
                fixture_observe(||Ok(std::fs::read_to_string(output_dir.join("requests.jsonl")).is_ok_and(|raw|raw.lines().filter(|line|!line.is_empty()).count()==1))).await?;
                let state=owner.coordinator.native.inspect(child.runtime.as_ref().context("Native direct target runtime missing")?).await?;
                anyhow::ensure!(state.running,"Direct send waited for the recipient terminal result");
                let cancelled=owner.coordinator.cancel(&root_id,&root_id,&child.key.id).await?;
                fixture_settle(&owner,&root_id).await?;
                Ok(json!({"sent":sent,"recipientBeforeCancel":state,"cancelledAfterReceipt":cancelled}))
            },
            "hub_failed_recipient"=>{
                owner.coordinator.registry.register(PeerRegistration{root:root_id.clone(),id:"controlled-unavailable-peer".into(),parent_id:Some(root_id.clone()),display_name:"controlled failure recipient".into(),kind:PeerKind::Sub,status:PeerStatus::Idle,runtime_generation:None,session_file:None,created_at:None,last_activity:None,history:Value::Null});Ok(json!({"fault":"visible recipient has no revivable runtime"}))
            },
            "hub_settings"=>{
                let path=ara_home().join("agent/config.yml");let mut value:Value=serde_json::from_slice(&std::fs::read(&path)?)?;value["task"]["maxRecursionDepth"]=operation["maximum"].clone();std::fs::write(&path,serde_json::to_vec_pretty(&value)?)?;Ok(json!({"settingsChanged":true}))
            },
            "hub_missing_sender"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"hub_missing_sender".into(),init:None,target:None,context:None,structured:false}).await,
            "hub_index_seed"=>{
                let parent=SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?;
                let cwd=PathBuf::from(parent.header()["cwd"].as_str().context("Hub actual cwd missing")?);
                let directory=root_file.with_extension("");std::fs::create_dir_all(&directory)?;
                let mut disk=SessionJournal::create_with_parent(&directory,&cwd,Some(&root_file.to_string_lossy()))?;
                // A valid-header incomplete transcript remains disk-only; it is
                // not a runnable parked child registered by the roster scan.
                disk.materialize()?;let id=disk.session_id().to_owned();let disk_file=disk.path().to_owned();
                anyhow::ensure!(disk_file.file_stem().and_then(|s|s.to_str())!=Some(id.as_str()),"Disk fixture must distinguish actual ID from filename");
                let advisor=PeerRegistration{root:root_id.clone(),id:"__advisor-hidden".into(),parent_id:Some(root_id.clone()),display_name:"hidden advisor".into(),kind:PeerKind::Advisor,status:PeerStatus::Parked,runtime_generation:None,session_file:None,created_at:None,last_activity:None,history:Value::Null};owner.coordinator.registry.register(advisor);
                let foreign_dir=output_dir.join("foreign-root");std::fs::create_dir_all(&foreign_dir)?;let mut foreign=SessionJournal::create_with_parent(&foreign_dir,&cwd,None)?;foreign.materialize()?;let foreign_id=foreign.session_id().to_owned();std::fs::copy(foreign.path(),directory.join("cross-root-shadow.jsonl"))?;
                std::fs::copy(&disk_file,directory.join("same-actual-id-copy.jsonl"))?;
                std::fs::copy(&disk_file,directory.join("__advisor-hidden.jsonl"))?;std::fs::copy(&disk_file,directory.join("ignored.bak.jsonl"))?;
                Ok(json!({"diskId":id,"diskFile":disk_file,"foreignId":foreign_id,"rootId":root_id}))
            },
            "cold_non_peer"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Cold alias missing")?).context("Cold Peer missing")?;
                anyhow::ensure!(record.runtime.is_none()&&record.status==PeerStatus::Parked,"Non-Peer probe requires joined original owner");
                root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"cold_non_peer".into(),init:None,target:Some(record.session_file.context("Cold original file missing")?.to_string_lossy().into_owned()),context:None,structured:false}).await
            },
            "cold_root_state"=>root.handle.call(super::rpc_host::NativeAction::State).await,
            "cold_root_key"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"cold_key".into(),init:None,target:None,context:None,structured:false}).await,
            "cold_thinking"=>{
                let id=aliases.get(&alias).context("Cold thinking alias missing")?;
                let slot={let entries=owner.attached.lock().unwrap();entries.get(&(root_id.clone(),id.clone())).context("Cold child missing")?.child.clone()};
                let handle={let held=slot.lock().await;held.as_ref().context("Cold ChildHost missing")?.handle().clone()};
                handle.call(super::rpc_host::NativeAction::PeerControl{operation:"cold_thinking".into(),init:None,target:None,context:operation["level"].as_str().map(str::to_owned),structured:false}).await
            },
            "cold_adopt_probe"=>{
                let before=root.handle.call(super::rpc_host::NativeAction::State).await?;
                let control=|operation:&str|super::rpc_host::NativeAction::PeerControl{operation:operation.into(),init:None,target:None,context:None,structured:false};
                let old_key=root.handle.call(control("cold_key")).await?;
                root.handle.call(control("cold_new")).await?;
                root.handle.call(super::rpc_host::NativeAction::Prompt{text:"actual scoped newly adopted UUID".into(),images:Vec::new()}).await?;
                root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;
                let fresh=root.handle.call(super::rpc_host::NativeAction::State).await?;
                let new_key=root.handle.call(control("cold_key")).await?;
                let missing=output_dir.join("never-created-switch-target.jsonl");
                anyhow::ensure!(!missing.exists(),"Cold precommit fault path was not absent");
                let failed=root.handle.call(super::rpc_host::NativeAction::Switch{path:missing}).await.err().context("Missing switch target unexpectedly adopted")?;
                let after_failed=root.handle.call(super::rpc_host::NativeAction::State).await?;
                // Same-ID publication failure does not seal a different Peer
                // namespace. Cross-ID sealing is a distinct lifecycle boundary.
                let publication_before=root.handle.call(super::rpc_host::NativeAction::State).await?;
                let fresh_file=PathBuf::from(publication_before["sessionFile"].as_str().context("Actual fresh Run did not materialize its journal")?);
                root.handle.call(control("cold_publication_fault")).await?;
                let publication_failure=root.handle.call(super::rpc_host::NativeAction::Switch{path:fresh_file}).await.err().context("Foreign-realm Native publication unexpectedly succeeded")?;
                let publication_observed=root.handle.call(control("cold_publication_observation")).await?;
                anyhow::ensure!(publication_observed["publicationAttempted"]==true&&publication_observed["preparedPrimary"].is_object(),"Publication fault did not cross actual primary preparation and Native publication");
                anyhow::ensure!(publication_observed["nativeError"].as_str().is_some_and(|error|!error.is_empty()),"Actual Native publication failure receipt missing");
                anyhow::ensure!(publication_observed["ownerBefore"]==publication_observed["ownerAfter"],"Failed publication changed actual Session/epoch/provider snapshot");
                let publication_after=root.handle.call(super::rpc_host::NativeAction::State).await?;
                root.handle.call(super::rpc_host::NativeAction::Prompt{text:"actual original owner after rejected Native publication".into(),images:Vec::new()}).await?;
                root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;
                root.handle.call(super::rpc_host::NativeAction::Switch{path:root_file.clone()}).await?;
                let switched=root.handle.call(super::rpc_host::NativeAction::State).await?;
                let switch_key=root.handle.call(control("cold_key")).await?;
                Ok(json!({"before":before,"oldKey":old_key,"fresh":fresh,"newKey":new_key,"failure":failed.to_string(),"afterFailed":after_failed,
                    "publicationFailure":publication_failure.to_string(),"publicationPrepared":publication_observed,
                    "publicationBefore":publication_before,"publicationAfter":publication_after,
                    "switched":switched,"switchKey":switch_key}))
            },
            "cold_pin_root"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"cold_pin".into(),init:None,target:Some(operation["credentialId"].as_i64().context("Cold controlled credential id missing")?.to_string()),context:None,structured:false}).await,
            "cold_setup"=>Ok(json!({"source":"controlled test-only settings installed before actual Host startup","deferredCatalog":deferred_catalog})),
            "cold_state"|"cold_side_usage"=>{
                let id=aliases.get(&alias).context("Cold Peer alias missing")?;
                let slot={let attached=owner.attached.lock().unwrap();attached.get(&(root_id.clone(),id.clone())).context("Cold actual runtime missing")?.child.clone()};
                let handle={let held=slot.lock().await;held.as_ref().context("Cold actual ChildHost missing")?.handle().clone()};
                if action=="cold_state"{handle.call(super::rpc_host::NativeAction::State).await}else{handle.call(super::rpc_host::NativeAction::PeerControl{operation:"cold_side_usage".into(),init:None,target:None,context:None,structured:false}).await}
            },
            "cold_settings"=>{
                let path=ara_home().join("agent/config.yml");
                let mut value:serde_json::Value=serde_json::from_slice(&std::fs::read(&path)?)?;
                if let Some(task)=operation.get("task"){value["task"]=task.clone();}
                if let Some(roles)=operation.get("modelRoles"){value["modelRoles"]=roles.clone();}
                std::fs::write(&path,serde_json::to_vec_pretty(&value)?)?;
                Ok(json!({"liveSettingsWritten":true}))
            },
            "cold_init"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Cold Peer alias missing")?).context("Cold Peer missing")?;
                anyhow::ensure!(record.runtime.is_none()&&record.status==PeerStatus::Parked,"Cold init mutation requires joined parked owner");
                let file=record.session_file.context("Cold original file missing")?;
                let mut journal=SessionJournal::open_with_blob_directory(&file,&ara_blobs_directory())?;
                let mut init=journal.latest_child_init()?.context("Cold original init missing")?;
                if operation.get("agent").is_some(){init.agent=operation["agent"].as_str().map(str::to_owned);}
                if operation.get("modelRole").is_some(){init.model_role=operation["modelRole"].as_str().map(str::to_owned);}
                if let Some(model)=operation["resolvedModel"].as_str(){init.resolved_model=Some(model.into());}
                if let Some(pair)=operation.get("compactionThreshold"){init.compaction_threshold=Some(serde_json::from_value(pair.clone())?);}
                if let Some(fallback)=operation.get("retryFallback"){init.retry_fallback=Some(serde_json::from_value(fallback.clone())?);}
                journal.append_child_init(&init)?;
                Ok(json!({"actualId":journal.session_id(),"file":file,"init":init}))
            },
            "cold_new"|"cold_switch"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Cold Peer alias missing")?).context("Cold Peer missing")?;
                let runtime=record.runtime.context("Cold actual runtime missing")?;
                if action=="cold_new"{owner.request(&runtime,PeerActorOp::FixtureNewSession(None)).await.map(|_|json!({"adopted":true}))}
                else{let slot={let attached=owner.attached.lock().unwrap();attached.get(&(root_id.clone(),record.key.id.clone())).context("Cold actual runtime missing")?.child.clone()};let handle={let held=slot.lock().await;held.as_ref().context("Cold actual ChildHost missing")?.handle().clone()};handle.call(super::rpc_host::NativeAction::Switch{path:PathBuf::from(operation["path"].as_str().context("Cold switch path missing")?)}).await}
            },
            "list"=>root.handle.call(super::rpc_host::NativeAction::PeerControl{operation:"list".into(),init:None,target:None,context:None,structured:false}).await,
            "send"|"write"=>{let target=operation["to"].as_str().unwrap_or(&alias);let target=aliases.get(target).cloned().unwrap_or_else(||target.to_owned());let body=operation["body"].as_str().unwrap_or("peer fixture message");let result=if action=="write"{let output=ara_tools::write::WriteTool::new(context.clone()).execute_os_values(&format!("agent://{target}"),body,CancellationToken::new()).await.map_err(|error|anyhow::Error::msg(error.0))?;serde_json::json!({"content":output.content,"details":output.details,"isError":output.is_error})}else{let from=operation["from"].as_str().unwrap_or("Main");let from=aliases.get(from).cloned().unwrap_or_else(||from.to_owned());serde_json::to_value(owner.coordinator.send(&root_id,&from,&target,body,None,false).await?)?};if operation["waitForRequest"]==true{fixture_observe(||Ok(std::fs::read_to_string(output_dir.join("requests.jsonl")).is_ok_and(|raw|raw.lines().any(|line|!line.is_empty())))).await?;}if operation["wait"].as_bool().unwrap_or(true){fixture_settle(&owner,&root_id).await?;}Ok(result)},
            "concurrent"=>{let id=aliases.get(&alias).context("Peer alias missing")?.clone();let mut tasks=tokio::task::JoinSet::new();for body in operation["bodies"].as_array().context("Concurrent bodies missing")?{let owner=owner.clone();let root=root_id.clone();let id=id.clone();let body=body.as_str().context("Concurrent body must be String")?.to_owned();tasks.spawn(async move{owner.coordinator.send(&root,&root,&id,&body,None,false).await});}let mut summaries=Vec::new();while let Some(result)=tasks.join_next().await{summaries.push(serde_json::to_value(result??)?);}fixture_settle(&owner,&root_id).await?;Ok(serde_json::json!(summaries))},
            "settle"=>{fixture_settle(&owner,&root_id).await?;Ok(serde_json::json!({"settled":true}))},
            "park"=>{let before=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Peer alias missing")?).context("Peer record missing")?;owner.coordinator.lifecycle.park(&before.key).await?;let after=owner.coordinator.registry.get_exact(&before.key).context("Park removed exact retained ref")?;let joined={let table=owner.attached.lock().unwrap();!table.values().any(|entry|Some(&entry.runtime)==before.runtime.as_ref())};let parked=after.status==PeerStatus::Parked&&after.runtime.is_none()&&after.session_file==before.session_file&&joined;Ok(json!({"parked":parked,"before":before,"after":after,"joined":joined}))},
            "revive"=>{let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Peer alias missing")?).context("Peer record missing")?;Ok(serde_json::to_value(owner.coordinator.lifecycle.ensure_live(&record.key).await?)?)},
            "adopt_ttl"=>{let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Peer alias missing")?).context("Peer record missing")?;owner.coordinator.lifecycle.adopt(record.key,operation["ttl"].as_i64().unwrap_or(-1))?;Ok(serde_json::json!({"ttl":operation["ttl"]}))},
            "pool"|"clear"=>{let id=aliases.get(&alias).context("Peer alias missing")?;let slot={let attached=owner.attached.lock().unwrap();attached.get(&(root_id.clone(),id.clone())).context("Peer runtime missing")?.child.clone()};let held=slot.lock().await;let child=held.as_ref().context("Actual ChildHost missing")?;let items=if action=="clear"{Vec::new()}else{vec![ara_cli::child_contract::WorkPoolYieldItem{id:"pooled-item".into(),index:1}]};let receipt=child.install_items(items,&CancellationToken::new()).await?;Ok(serde_json::json!({"generation":receipt.generation,"pooled":!receipt.items.is_empty()}))},
            "cancel"=>{let target=aliases.get(&alias).context("Peer alias missing")?;let caller=operation["from"].as_str().unwrap_or("Main");let caller=aliases.get(caller).context("Cancel caller missing")?;Ok(serde_json::json!({"cancelled":owner.coordinator.cancel(&root_id,caller,target).await?}))},
            "cancel_turn"=>{let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Peer alias missing")?).context("Peer record missing")?;owner.coordinator.jobs.cancel_peer(&record.key).await?;fixture_settle(&owner,&root_id).await?;Ok(serde_json::json!({"cancelled":true}))},
            "parent_wait"=>{std::fs::write(output_dir.join(format!("parent-wait-{index}-before.json")),serde_json::to_vec_pretty(&json!({"jobs":owner.coordinator.jobs.snapshot_root(&root_id),"active":owner.coordinator.jobs.active_root(&root_id),"registry":owner.coordinator.registry.list(&root_id)}))?)?;Ok(serde_json::to_value(owner.wait_parent(&root_id,&root_id,&CancellationToken::new()).await?)?)},
            "root_prompt"=>{root.handle.call(super::rpc_host::NativeAction::Prompt{text:operation["text"].as_str().unwrap_or("fixture root prompt").into(),images:Vec::new()}).await?;root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;fixture_settle(&owner,&root_id).await?;Ok(serde_json::json!({"settled":true}))},
            "filesystem_refusal"=>{let target=output_dir.join("forbidden-filesystem-write.txt");let result=ara_tools::write::WriteTool::new(context.clone()).execute_os_values(&target.to_string_lossy(),"forbidden",CancellationToken::new()).await;anyhow::ensure!(result.is_err()&&!target.exists(),"Coordination transport granted an ordinary filesystem write");Ok(serde_json::json!({"refused":true,"exists":target.exists()}))},
            "switch_into_child"=>{let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Peer alias missing")?).context("Peer record missing")?;root.handle.call(super::rpc_host::NativeAction::Switch{path:record.session_file.context("No child transcript")?}).await},
            "inspect"|"root_inspect"=>{let id=if action=="root_inspect"{&root_id}else{aliases.get(&alias).context("Peer alias missing")?};let record=owner.coordinator.registry.get(&root_id,id).context("Peer record missing")?;let file=record.session_file.as_ref().unwrap_or(&root_file);let raw=std::fs::read_to_string(file)?;let artifact=std::fs::read_to_string(ara_cli::host_peers::roster::artifact_paths(file,&record.key.id).markdown).ok();let state=if let Some(runtime)=&record.runtime{serde_json::to_value(owner.coordinator.native.inspect(runtime).await?)?}else{serde_json::Value::Null};Ok(serde_json::json!({"record":record,"rawJournal":raw,"artifact":artifact,"state":state}))},
            "create_nested"=>{
                let parent=aliases.get(operation["from"].as_str().context("Nested parent alias missing")?).context("Nested parent alias missing")?.clone();
                let slot={let table=owner.attached.lock().unwrap();table.get(&(root_id.clone(),parent.clone())).context("Actual parent runtime missing")?.child.clone()};
                let handle={let held=slot.lock().await;held.as_ref().context("Actual nested parent ChildHost missing")?.handle().clone()};
                let result=handle.call(super::rpc_host::NativeAction::PeerControl{operation:"create".into(),init:Some(operation["init"].clone()),target:None,context:None,structured:false}).await?;let runtime:RuntimeKey=serde_json::from_value(result)?;aliases.insert(alias.clone(),runtime.peer.id.clone());Ok(serde_json::to_value(owner.coordinator.registry.get_exact(&runtime.peer))?)
            },
            "roster_snapshot"=>{
                let fresh=Arc::new(PeerRegistry::default());let roster=PeerRoster::new(fresh.clone());roster.register_persisted(&root_id,&root_file,false).await?;Ok(serde_json::to_value(fresh.list(&root_id))?)
            },
            "probe_recovery"=>{
                let adopted=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Probe alias missing")?).context("Probe ref missing")?;
                anyhow::ensure!(adopted.status==PeerStatus::Parked&&adopted.runtime.is_none(),"Probe requires the actual joined parked child");
                // Dead-ref reclamation intentionally excludes adopted TTL owners. Retire
                // that ownership first, then discover the same real transcript afresh.
                anyhow::ensure!(owner.coordinator.lifecycle.release(&adopted.key,false).await?,"Probe parked ownership release failed");
                owner.coordinator.roster.register_persisted(&root_id,&root_file,false).await?;
                let before=owner.coordinator.registry.get(&root_id,&adopted.key.id).context("Probe transcript was not rediscovered")?;
                anyhow::ensure!(before.status==PeerStatus::Parked&&before.runtime.is_none()&&before.session_file==adopted.session_file&&before.key!=adopted.key&&!owner.coordinator.lifecycle.has(&before.key),"Probe did not reach a real unadopted parked ref for the same transcript");
                fixture_configure(&output_dir,json!([]),json!([{"pathContains":before.session_file.as_ref().context("Probe file missing")?.to_string_lossy(),"key":"probe-unknown","kind":"PermissionDenied"}]))?;
                let error=owner.coordinator.lifecycle.reclaim_dead(&before.key).await.err().context("Controlled I/O fault was treated as dead or suppressed")?;let retained=owner.coordinator.registry.get_exact(&before.key).context("Unknown probe reclaimed ref")?;anyhow::ensure!(retained.session_file==before.session_file&&retained.runtime==before.runtime,"Unknown probe changed exact ref");
                fixture_configure(&output_dir,json!([]),json!([]))?;let runtime=owner.coordinator.lifecycle.ensure_live(&before.key).await?;Ok(json!({"error":error.to_string(),"before":before,"retained":retained,"runtime":runtime,"after":owner.coordinator.registry.get_exact(&before.key)}))
            },
            "cold_bind_race"=>{
                let before=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Cold alias missing")?).context("Cold ref missing")?;anyhow::ensure!(before.status==PeerStatus::Parked&&before.runtime.is_none(),"Cold race requires actually parked ref");
                let gate_key="cold-bind";fixture_configure(&output_dir,json!([{"name":"peer-before-bind","identityContains":before.key.id,"key":gate_key}]),json!([]))?;
                let lifecycle=owner.coordinator.lifecycle.clone();let key=before.key.clone();let candidate=tokio::spawn(async move{lifecycle.ensure_live(&key).await});
                fixture_observe(||Ok(output_dir.join("frontiers/cold-bind.reached.json").is_file())).await?;let reached:Value=serde_json::from_slice(&std::fs::read(output_dir.join("frontiers/cold-bind.reached.json"))?)?;
                owner.coordinator.registry.tombstone(&before.key);fixture_release(&output_dir,gate_key)?;let error=candidate.await?.err().context("Cold CAS overwrote tombstone")?;fixture_configure(&output_dir,json!([]),json!([]))?;
                let after=owner.coordinator.registry.get_exact(&before.key).context("Cold race lost tombstone")?;let published=owner.attached.lock().unwrap().get(&(root_id.clone(),before.key.id.clone())).map(|entry|entry.runtime.clone());anyhow::ensure!(after.status==PeerStatus::Aborted&&after.runtime.is_none()&&published.is_none(),"Cold rejected candidate became published");
                let events=fixture_events(&output_dir.join(format!("{}.events.jsonl",before.key.id)))?;Ok(json!({"before":before,"reached":reached,"error":error.to_string(),"after":after,"published":published,"events":events}))
            },
            "wait_running"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Running alias missing")?).context("Running ref missing")?;let runtime=record.runtime.context("Running actual owner missing")?;
                fixture_observe(||Ok(fixture_events(&output_dir.join(format!("{}.events.jsonl",record.key.id)))?.iter().any(|e|e["type"]=="agent_start")&&std::fs::read_to_string(output_dir.join("requests.jsonl")).is_ok_and(|raw|raw.lines().any(|line|!line.is_empty())))).await?;let state=owner.coordinator.native.inspect(&runtime).await?;anyhow::ensure!(state.running,"Actual provider request did not belong to a still running child");Ok(json!({"runtime":runtime,"state":state,"requestBytes":std::fs::metadata(output_dir.join("requests.jsonl"))?.len()}))
            },
            "root_close_probe"=>{
                let blocked_id=aliases.get(&alias).context("Blocked root-close alias missing")?.clone();let sibling_id=aliases.get(operation["sibling"].as_str().context("Sibling close alias missing")?).context("Sibling close alias missing")?.clone();let expected=owner.coordinator.registry.get(&root_id,&root_id).and_then(|r|r.runtime).context("Root close actual runtime missing")?;
                let slot={let table=owner.attached.lock().unwrap();table.get(&(root_id.clone(),blocked_id.clone())).context("Actual blocked Child owner missing")?.child.clone()};let held=slot.lock().await;
                let closing_owner=owner.clone();let runtime=expected.clone();let closing=tokio::spawn(async move{closing_owner.release_root(&runtime).await});
                fixture_observe(||Ok(!owner.attached.lock().unwrap().contains_key(&(root_id.clone(),sibling_id.clone())))).await?;let sibling_joined_before_release=!closing.is_finished();anyhow::ensure!(sibling_joined_before_release&&owner.coordinator.registry.root_is_sealed(&root_id),"Root serialized sibling close behind blocked Child owner");
                let in_flight=owner.coordinator.lifecycle.root_close_snapshot(&root_id).context("Root retained close snapshot missing")?;std::fs::write(output_dir.join("root-sealed"),b"actual root admission sealed")?;drop(held);let first_result=closing.await?;
                std::fs::write(output_dir.join("root-close-first.json"),serde_json::to_vec_pretty(&json!({"first":first_result.as_ref().map_err(|error|error.to_string()),"actual":fixture_root_close_state(&owner,&root_id)}))?)?;let first=first_result?;
                let observed=fixture_observe(||Ok(owner.coordinator.lifecycle.root_close_snapshot(&root_id).is_some_and(|receipt|receipt.settled))).await;
                std::fs::write(output_dir.join("root-close-observed.json"),serde_json::to_vec_pretty(&json!({"observed":observed.as_ref().map_err(|error|error.to_string()),"actual":fixture_root_close_state(&owner,&root_id)}))?)?;observed?;let receipt=owner.coordinator.lifecycle.root_close_snapshot(&root_id).unwrap();Ok(json!({"blocked":blocked_id,"sibling":sibling_id,"siblingJoinedBeforeRelease":sibling_joined_before_release,"inFlight":in_flight,"first":first,"receipt":receipt}))
            },
            "plan_append"=>{
                struct Policy(String);impl ara_cli::host_edit_session::EditPlanPolicy for Policy{fn current_plan_path(&self)->Option<String>{Some(self.0.clone())}}
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Plan alias missing")?).context("Plan ref missing")?;let runtime=record.runtime.as_ref().context("Plan actual runtime missing")?;
                owner.request(runtime,PeerActorOp::FixturePlan(Arc::new(Policy(output_dir.join("controlled-plan.md").to_string_lossy().into_owned())))).await?;
                let state=owner.coordinator.native.inspect(runtime).await?;anyhow::ensure!(state.plan_mode&&!state.running,"Actual Host plan policy not active");let message=operation["body"].as_str().context("Plan original raw body missing")?;let receipt=owner.coordinator.send(&root_id,&root_id,&record.key.id,message,None,false).await?;
                let journal=SessionJournal::open_with_blob_directory(record.session_file.as_ref().context("Plan transcript missing")?,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;anyhow::ensure!(rows.iter().any(|row|row["customType"]=="irc"&&row["details"]["message"]==message),"PlanAppend did not durably retain raw original body");Ok(json!({"state":state,"receipt":receipt,"rows":rows}))
            },
            "adoption_once"=>{
                let child=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Adoption alias missing")?).context("Adoption child missing")?;
                fixture_configure(&output_dir,json!([{"name":"parent-after-materialize-before-ack","identityContains":child.key.id,"key":"adoption-ack","failAfterRelease":true}]),json!([]))?;
                owner.coordinator.send(&root_id,&root_id,&child.key.id,"Original one-result assignment",None,false).await?;
                fixture_observe(||Ok(output_dir.join("frontiers/adoption-ack.reached.json").is_file())).await?;
                let completion=owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).into_iter().find(|job|job.id.child==child.key).and_then(|job|job.completion).context("Actual completed task missing before ACK")?;
                let matches=|journal:&SessionJournal|->Vec<String>{journal.entries().iter().filter(|entry|entry.observed_message_field(&["details","peerWakeJob"]).ok().flatten().and_then(|value|ara_session::native_checked_serde(&value).ok())==serde_json::to_value(&completion.job).ok()).map(|entry|entry.id.clone()).collect()};
                let before=matches(&SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?);anyhow::ensure!(before.len()==1,"Actual materialize must exist exactly once before ACK fault");
                let jobs=owner.coordinator.jobs.clone();let job=completion.job.clone();let observer=tokio::spawn(async move{jobs.claim_and_adopt(&job).await});tokio::task::yield_now().await;anyhow::ensure!(!observer.is_finished(),"Observer did not join retained adoption operation");observer.abort();let observer_dropped=observer.await.is_err();
                fixture_release(&output_dir,"adoption-ack")?;
                fixture_observe(||Ok(owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).iter().any(|job|job.id==completion.job&&matches!(job.consumption,ParentConsumption::Unknown(_))))).await?;
                let unknown=owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).into_iter().find(|job|job.id==completion.job).unwrap();let retry=owner.coordinator.jobs.claim_and_adopt(&completion.job).await.err().context("Unknown operation dispatched again")?;
                fixture_configure(&output_dir,json!([]),json!([]))?;let parent=owner.coordinator.registry.get(&root_id,&root_id).and_then(|row|row.runtime).context("Actual parent runtime missing")?;
                let receipt=match owner.request(&parent,PeerActorOp::ParentResult(completion.clone())).await?{PeerActorReply::Adoption(receipt)=>receipt,_=>bail!("Wrong original parent receipt")};anyhow::ensure!(receipt.entry_id==before[0]&&receipt.persistent,"Reconciliation returned another journal effect");owner.coordinator.jobs.reconcile_adoption(&completion.job,&completion.runtime,receipt.clone())?;
                let after=matches(&SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?);anyhow::ensure!(before==after,"Unknown ACK replayed parent context/journal effect");fixture_settle(&owner,&root_id).await?;Ok(json!({"completion":completion,"observerDropped":observer_dropped,"beforeEntries":before,"unknown":unknown,"retryError":retry.to_string(),"receipt":receipt,"afterEntries":after,"after":owner.coordinator.jobs.snapshot_parent(&root_id,&root_id)}))
            },
            "task_unknown_close"=>{
                let key=task_keys.get(&alias).context("Actual Task exact alias missing")?.clone();
                let record=owner.coordinator.registry.get_exact(&key).context("Actual Task ref missing before close fault")?;
                let lease=owner.task_resource(&key).context("Actual Task owned resource missing before close fault")?;
                let resource_before=lease.snapshot();let cwd=lease.cwd().to_owned();
                let fault=fixture_close_fault_owned(&owner,&record,&output_dir,true).await?;
                let runtime=record.runtime.as_ref().context("Original Task runtime missing")?;
                let original=owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(runtime).cloned().context("Original Task close flight disappeared")?;
                let release=lease.finalize(Err(fault["originalError"].as_str().context("Actual original Task close error missing")?.into())).await?;
                anyhow::ensure!(release.status=="retained"&&cwd.is_dir(),"Unknown actual Task close did not preserve its actual workspace");
                let parent=owner.coordinator.registry.get(&root_id,&root_id).and_then(|row|row.runtime).context("Actual Main root missing")?;
                let receipt=owner.release_root(&parent).await?;
                let root_original=fixture_root_close_original(&owner,runtime,&receipt,fault["originalError"].as_str().unwrap())?;
                let same=owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(runtime).is_some_and(|current|original.same_channel(current));
                anyhow::ensure!(!receipt.settled&&same,"Unknown Task resource root replayed/lost original close outcome");
                unresolved_terminal=true;
                Ok(json!({"fault":fault,"rootReceipt":receipt,"rootOriginal":root_original,"resourceBefore":resource_before,"resourceAfter":lease.snapshot(),"releaseReceipt":release,"cwdExists":cwd.is_dir(),"sameChannelAfterRoot":same}))
            },
            "close_unknown_probe"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Unknown-close alias missing")?).context("Unknown-close ref missing")?;let fault=fixture_close_fault(&owner,&record,&output_dir).await?;let runtime=record.runtime.context("Original close runtime missing")?;let original={owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(&runtime).cloned().context("Retained flight missing before root")?};let parent=owner.coordinator.registry.get(&root_id,&root_id).and_then(|row|row.runtime).context("Root runtime missing")?;let root_receipt=owner.release_root(&parent).await?;let root_original=fixture_root_close_original(&owner,&runtime,&root_receipt,fault["originalError"].as_str().unwrap())?;unresolved_terminal=true;let same_after_root={owner.closes.lock().unwrap_or_else(|e|e.into_inner()).get(&runtime).is_some_and(|current|original.same_channel(current))};anyhow::ensure!(same_after_root,"Root shutdown replaced original close flight");Ok(json!({"runtime":runtime,"observerDropped":fault["observerDropped"],"released":fault["released"],"fault":fault,"rootReceipt":root_receipt,"rootOriginal":root_original,"sameChannelAfterRoot":same_after_root}))
            },
            "raw_running_routes"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Raw target missing")?).context("Raw target ref missing")?;let sibling=aliases.get(operation["sibling"].as_str().context("Raw sibling alias missing")?).context("Raw sibling missing")?.clone();let runtime=record.runtime.clone().context("Raw target Native runtime missing")?;let events_path=output_dir.join(format!("{}.events.jsonl",record.key.id));let raw=operation["body"].as_str().context("Raw original body missing")?;
                owner.coordinator.send(&root_id,&root_id,&record.key.id,"Read the controlled fixture file",None,false).await?;
                fixture_observe(||Ok(fixture_events(&events_path)?.iter().any(|event|event["type"]=="message_update"&&[&event["message"],&event["assistantMessageEvent"]["partial"]].iter().any(|m|m["content"].as_array().is_some_and(|blocks|blocks.iter().any(|b|b["type"]=="toolCall"&&b["id"]=="raw-read"&&b["name"]=="read"&&b["arguments"]["path"]=="raw-fixture.txt")))))).await?;
                anyhow::ensure!(owner.coordinator.native.inspect(&runtime).await?.running,"Raw routes need actual running receiver");let sibling_ack=owner.coordinator.send(&root_id,&sibling,&record.key.id,raw,None,false).await?;let parent_ack=owner.coordinator.send(&root_id,&root_id,&record.key.id,raw,None,false).await?;anyhow::ensure!([&sibling_ack,&parent_ack].iter().all(|r|!r.is_error&&r.receipts.len()==1&&r.receipts[0].outcome==DeliveryOutcome::Injected),"Actual raw ingress not acknowledged");std::fs::write(output_dir.join("raw-routes-ack.json"),serde_json::to_vec_pretty(&json!({"sibling":sibling_ack,"parent":parent_ack}))?)?;
                fixture_settle(&owner,&root_id).await?;let journal=SessionJournal::open_with_blob_directory(record.session_file.as_ref().context("Raw journal missing")?,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;
                for(from,parent)in [(&root_id,true),(&sibling,false)]{anyhow::ensure!(rows.iter().any(|r|r["customType"]=="irc"&&r["details"]["from"]==*from&&r["details"]["message"]==raw&&r["details"]["fromParent"]==parent),"Actual persisted raw ingress missing for parent={parent}");}Ok(json!({"rows":rows,"sibling":sibling,"runtime":runtime,"parentAck":parent_ack,"siblingAck":sibling_ack}))
            },
            "uri_lifecycle"=>{
                let runtime=owner.coordinator.registry.get(&root_id,&root_id).and_then(|r|r.runtime).context("Actual root URI owner missing")?;let schemes=ara_rpc::WireValue::parse("[{\"scheme\":\"agent\",\"writable\":false}]")?;owner.request(&runtime,PeerActorOp::FixtureUriSchemes(schemes)).await?;
                let mut observations=Vec::new();let mut new_file=None;
                for phase in ["registered","removed","refresh","new","switch"]{
                    match phase{
                        "removed"=>{owner.request(&runtime,PeerActorOp::FixtureUriSchemes(ara_rpc::WireValue::Array(Vec::new()))).await?;},
                        "refresh"=>{owner.request(&runtime,PeerActorOp::FixtureRefreshHostTools).await?;},
                        "new"=>{owner.request(&runtime,PeerActorOp::FixtureNewSession(None)).await?;},
                        "switch"=>{root.handle.call(super::rpc_host::NativeAction::Switch{path:root_file.clone()}).await?;},_=>{}
                    }
                    root.handle.call(super::rpc_host::NativeAction::Prompt{text:format!("Use prescribed WRITE URI route observation {phase}"),images:Vec::new()}).await?;root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;
                    let state=root.handle.call(super::rpc_host::NativeAction::State).await?;if phase=="new"{new_file=state["sessionFile"].as_str().map(str::to_owned);}
                    let events=fixture_events(&output_dir.join("root.events.jsonl"))?;let id=format!("uri-{phase}");let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]==id).collect::<Vec<_>>();anyhow::ensure!(ends.len()==1&&ends[0]["isError"]==true,"URI phase {phase} lacked exact actual Native refusal");
                    let result=&ends[0]["result"];let diagnostic=result["content"].as_array().into_iter().flatten().filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n");let expected=if phase=="registered"{"agent:// URLs are read-only for write"}else{"Unknown protocol: agent://"};anyhow::ensure!(diagnostic.contains(expected)&&!diagnostic.contains("Unknown agent"),"Explicit URI override/removal lost across {phase}: {diagnostic}");
                    observations.push(json!({"phase":phase,"state":state,"nativeResult":result,"diagnostic":diagnostic}));
                }
                anyhow::ensure!(observations[3]["state"]["sessionId"]!=root_id&&observations[4]["state"]["sessionId"]==root_id,"Actual new/switch Session identities did not change and restore");Ok(json!({"observations":observations,"originalRootFile":root_file,"newFile":new_file}))
            },
            "hub_pending_wait"=>{
                let sibling=aliases.get(operation["sibling"].as_str().context("Hub sender alias missing")?).context("Hub sender record missing")?.clone();
                root.handle.call(super::rpc_host::NativeAction::Prompt{text:"Use the real no-argument Wait for the pending message".into(),images:Vec::new()}).await?;
                fixture_observe(||Ok(output_dir.join("hub-wait-stream-held").is_file())).await?;
                let sent=owner.coordinator.send(&root_id,&sibling,&root_id,"pending aside wins the Native Wait interrupt boundary",None,false).await?;anyhow::ensure!(!sent.is_error&&sent.receipts.len()==1&&sent.receipts[0].outcome==DeliveryOutcome::Injected,"Actual pending aside dispatch failed");
                let runtime=owner.coordinator.registry.get(&root_id,&root_id).and_then(|record|record.runtime).context("Actual pending Wait root runtime missing")?;
                anyhow::ensure!(owner.queues.has_pending(&runtime),"Actual Native queue lost pending message before Wait entry");
                std::fs::write(output_dir.join("hub-wait-queued-ack.json"),serde_json::to_vec_pretty(&json!({"runtime":runtime,"senderAck":sent}))?)?;
                root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;
                let events=fixture_events(&output_dir.join("root.events.jsonl"))?;let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="hub-pending-wait").collect::<Vec<_>>();anyhow::ensure!(ends.len()==1,"Original pending Wait result was repeated or lost");
                let skipped=operation["expectSkipped"].as_bool().unwrap_or(false);
                if skipped{
                    anyhow::ensure!(ends[0]["isError"]==true&&ends[0]["result"]["details"]["__synthetic"]==true&&ends[0]["result"]["details"]["source"]=="interrupt_skipped"&&ends[0]["result"]["details"]["executed"]==false,"Immediate early IRC did not retain its real pre-admission skip");
                }else{
                    anyhow::ensure!(ends[0]["isError"]==false,"Pending actual Native Wait did not finish once");
                    anyhow::ensure!(ends[0]["result"]["details"]["message"]["message"]=="pending aside wins the Native Wait interrupt boundary","Native Wait did not consume the original pending message");
                }
                fixture_settle(&owner,&root_id).await?;
                let journal=SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;
                if skipped{
                    let original=rows.iter().filter(|m|m["customType"]=="irc"&&m["details"]["message"]=="pending aside wins the Native Wait interrupt boundary").collect::<Vec<_>>();
                    anyhow::ensure!(original.len()==1&&original[0]["details"]["from"]==sibling&&original[0]["details"]["to"]==root_id,"Immediate skipped Wait did not auto-inject its original IRC exactly once");
                }else{
                    anyhow::ensure!(!rows.iter().any(|m|m["customType"]=="irc"&&m["details"]["message"]=="pending aside wins the Native Wait interrupt boundary"),"Wait-consumed aside also auto-injected");
                }
                Ok(json!({"waitResult":ends[0],"rows":rows,"senderAck":sent,"skippedBeforeAdmission":skipped}))
            },
            "child_wait_interrupt"=>{
                let child=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Wait child alias missing")?).context("Actual Wait child missing")?;let grand_id=aliases.get(operation["grandchild"].as_str().context("Wait owned grandchild alias missing")?).context("Owned grandchild alias missing")?.clone();let events_path=output_dir.join(format!("{}.events.jsonl",child.key.id));
                owner.coordinator.send(&root_id,&child.key.id,&grand_id,"owned task remains running during actual child Wait",None,false).await?;fixture_observe(||Ok(std::fs::read_to_string(output_dir.join("requests.jsonl")).is_ok_and(|raw|raw.lines().any(|line|!line.is_empty())))).await?;owner.coordinator.send(&root_id,&root_id,&child.key.id,"Call wait for the actual grandchild task",None,false).await?;
                fixture_observe(||Ok(fixture_events(&events_path)?.iter().any(|e|e["type"]=="tool_execution_update"&&e["toolCallId"]=="native-child-wait"&&e["partialResult"]["details"]["op"]=="wait"))).await?;
                let hard=operation["hard"].as_bool().unwrap_or(false);let admitted=if hard{owner.coordinator.jobs.cancel_peer(&child.key).await?;json!({"source":"actual cancel exact child wake"})}else{
                    // Bus delivery intentionally gives an entered Wait its raw
                    // message first. Exercise the distinct actual ParentSteer
                    // callback here so this bank observes soft interruption.
                    let runtime=child.runtime.clone().context("Entered Wait actual child runtime missing")?;
                    anyhow::ensure!(child.parent_id.as_deref()==Some(root_id.as_str())&&owner.coordinator.registry.owns_runtime(&runtime)&&owner.coordinator.native.inspect(&runtime).await?.running,"Actual entered Wait ParentSteer owner changed");
                    let message=PeerInbox::fresh(&root_id,&child.key.id,"Wrap up this wait using actual parent steering");
                    let rendered=ara_cli::host_peers::native::render_incoming(&message,true,false,true);
                    owner.coordinator.native.deliver(&runtime,NativeDelivery{route:DeliveryRoute::ParentSteer,rendered,message:message.clone()}).await?;
                    json!({"source":"actual Native ParentSteer callback","runtime":runtime,"message":message,"ack":"delivered"})
                };
                fixture_observe(||Ok(fixture_events(&events_path)?.iter().any(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="native-child-wait"))).await?;let events=fixture_events(&events_path)?;let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="native-child-wait").collect::<Vec<_>>();anyhow::ensure!(ends.len()==1,"Entered Native Wait result was replayed");let ended=ends[0].clone();
                if hard{anyhow::ensure!(ended["isError"]==true&&ended["result"]["details"]["interrupted"]!=true,"Genuine abort became successful steering interruption");}else{anyhow::ensure!(ended["isError"]==false&&ended["result"]["details"]["op"]=="wait"&&ended["result"]["details"]["interrupted"]==true,"Entered soft Native Wait did not retain its normal original result");}
                // Hold the original grandchild response until this child Run has
                // actually ended, not merely until its interrupted Wait returned.
                // Otherwise a legal parent-result aside can add a fourth model
                // request before the scripted three-response scenario finishes.
                fixture_observe(||Ok(fixture_events(&events_path)?.iter().any(|event|event["type"]=="agent_end"))).await?;
                std::fs::write(output_dir.join("child-wait-terminal.json"),serde_json::to_vec_pretty(&json!({"hard":hard,"admitted":admitted,"end":ended,"childEndObserved":true}))?)?;fixture_settle(&owner,&root_id).await?;let journal=SessionJournal::open_with_blob_directory(child.session_file.as_ref().context("Wait child transcript missing")?,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;
                if !hard{anyhow::ensure!(rows.iter().filter(|row|row["role"]=="toolResult"&&row["toolCallId"]=="native-child-wait"&&row["isError"]==false&&row["details"]["interrupted"]==true).count()==1,"Successful entered Wait was replaced by skip or lost before durable journal");anyhow::ensure!(rows.iter().filter(|row|row["customType"]=="irc"&&row["details"]["from"]==root_id&&row["details"]["message"]==admitted["message"]["body"]&&row["details"]["fromParent"]==true).count()==1,"Actual parent steering raw message was not retained exactly once after entered Wait");}Ok(json!({"hard":hard,"admitted":admitted,"actualEnd":ended,"events":events,"rows":rows}))
            },
            "wait_ack_unknown"=>{
                let child=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Private Wait child alias missing")?).context("Private Wait child ref missing")?;fixture_configure(&output_dir,json!([{"name":"wait-after-materialize-before-ack","identityContains":root_id,"key":"wait-private-ack","failAfterRelease":true}]),json!([]))?;
                owner.coordinator.send(&root_id,&root_id,&child.key.id,"Original completion selected only by the private actual Native Wait",None,false).await?;fixture_observe(||Ok(std::fs::read_to_string(output_dir.join("requests.jsonl")).is_ok_and(|raw|raw.lines().any(|line|!line.is_empty())))).await?;root.handle.call(super::rpc_host::NativeAction::Prompt{text:"Call the actual wait for the running owned child".into(),images:Vec::new()}).await?;let observing=root.handle.clone();let observer=tokio::spawn(async move{observing.call(super::rpc_host::NativeAction::WaitIdle).await});
                fixture_observe(||Ok(output_dir.join("frontiers/wait-private-ack.reached.json").is_file())).await?;anyhow::ensure!(output_dir.join("native-wait-watched.json").is_file(),"Original private Wait did not install an actual owned watch before selection");let before=owner.coordinator.jobs.snapshot_parent(&root_id,&root_id);let job=before.iter().find(|job|job.id.child==child.key).context("Private Wait actual job missing")?;anyhow::ensure!(job.completion.is_some()&&matches!(job.consumption,ParentConsumption::ForegroundReserved{..}),"Private Wait was consumed successfully before its materialization ACK");let exact_job=job.id.clone();
                let journal=SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;let selected=rows.iter().filter(|row|row["role"]=="toolResult"&&row["toolCallId"]=="native-wait").collect::<Vec<_>>();anyhow::ensure!(selected.len()==1&&selected[0]["isError"]==false&&selected[0]["content"].to_string().contains("private foreground original result")&&!rows.iter().any(|row|row["customType"]=="peer-task-result"),"Original private ToolResult was not materialized exactly once before ACK loss");let original_entry_ids=journal.entries().iter().filter(|entry|entry.observed_message_field(&["toolCallId"]).ok().flatten().is_some_and(|value|value.as_string().is_some_and(|id|id.equals_ascii("native-wait")))).map(|entry|entry.id.clone()).collect::<Vec<_>>();anyhow::ensure!(original_entry_ids.len()==1,"Private ToolResult durable entry identity missing");
                anyhow::ensure!(!observer.is_finished(),"Native WaitIdle observer escaped held materialization frontier");observer.abort();let observer_dropped=observer.await.is_err();fixture_release(&output_dir,"wait-private-ack")?;fixture_observe(||Ok(owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).iter().any(|job|job.id==exact_job&&matches!(job.consumption,ParentConsumption::Unknown(_))))).await?;
                let unknown=owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).into_iter().find(|job|job.id==exact_job).context("Unknown private Wait owner disappeared")?;let retry_error=owner.coordinator.jobs.claim_and_adopt(&exact_job).await.err().context("Unknown private Wait released its selection for automatic replay")?.to_string();let after=SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?;let after_rows=after.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;let after_entry_ids=after.entries().iter().filter(|entry|entry.observed_message_field(&["toolCallId"]).ok().flatten().is_some_and(|value|value.as_string().is_some_and(|id|id.equals_ascii("native-wait")))).map(|entry|entry.id.clone()).collect::<Vec<_>>();anyhow::ensure!(after_entry_ids==original_entry_ids&&!after_rows.iter().any(|row|row["customType"]=="peer-task-result"),"Private ACK loss replayed or duplicated original durable result");let requests=std::fs::read_to_string(output_dir.join("requests.jsonl"))?.lines().filter(|line|!line.is_empty()).count();anyhow::ensure!(requests==2,"Unknown private Wait drove another model request");fixture_configure(&output_dir,json!([]),json!([]))?;unresolved_terminal=true;Ok(json!({"before":before,"unknown":unknown,"observerDropped":observer_dropped,"originalEntryIds":original_entry_ids,"afterEntryIds":after_entry_ids,"originalRows":rows,"afterRows":after_rows,"retryError":retry_error,"requestsBeforeUnknownEOF":requests}))
            },
            "transition_close_unknown"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Transition child alias missing")?).context("Transition child missing")?;let root_runtime=owner.coordinator.registry.get(&root_id,&root_id).and_then(|row|row.runtime).context("Actual old root runtime missing")?;let before=root.handle.call(super::rpc_host::NativeAction::State).await?;let before_bytes=std::fs::read(&root_file)?;let native_before=serde_json::to_value(owner.coordinator.native.inspect(&root_runtime).await?)?;anyhow::ensure!(before["fixturePeerActivation"]["runtime"]==serde_json::to_value(&root_runtime)?&&before["fixturePeerActivation"]["owner"]==json!(Arc::as_ptr(&owner)as usize),"Before snapshot is not the actual installed original Host/runtime");
                let fault=fixture_close_fault(&owner,&record,&output_dir).await?;anyhow::ensure!(!owner.coordinator.registry.root_is_sealed(&root_id),"Fixture sealed root before actual adoption boundary");let mode=operation["transition"].as_str().context("Actual transition kind missing")?;let error=match mode{"new"=>owner.request(&root_runtime,PeerActorOp::FixtureNewSession(None)).await.err().context("Actual new_session installed target after historical close Unknown")?,"switch"=>{let target=PathBuf::from(operation["target"].as_str().context("Real other Native root journal path missing")?);let target_journal=SessionJournal::open_with_blob_directory(&target,&ara_blobs_directory())?;anyhow::ensure!(target_journal.session_id()!=root_id&&target_journal.header()["cwd"]==SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?.header()["cwd"],"Switch target was not a distinct real matching-cwd Native journal");root.handle.call(super::rpc_host::NativeAction::Switch{path:target}).await.err().context("Actual switch_session installed target after historical close Unknown")?},_=>bail!("Unknown actual transition kind")};
                let after=root.handle.call(super::rpc_host::NativeAction::State).await?;let native_after=serde_json::to_value(owner.coordinator.native.inspect(&root_runtime).await?)?;for field in ["sessionId","sessionFile","fixturePeerActivation"]{anyhow::ensure!(before[field]==after[field],"Actual failed {mode} replaced original {field}");}anyhow::ensure!(after["sessionId"]==root_id&&std::fs::read(&root_file)?==before_bytes&&native_before==native_after&&owner.coordinator.registry.get_exact(&root_runtime.peer).and_then(|row|row.runtime)==Some(root_runtime.clone()),"Actual failed transition changed old Session/file/native runtime");let root_receipt=owner.release_root(&root_runtime).await?;let child_runtime=record.runtime.context("Historical child runtime missing")?;let original=fixture_root_close_original(&owner,&child_runtime,&root_receipt,fault["originalError"].as_str().unwrap())?;anyhow::ensure!(owner.coordinator.registry.root_is_sealed(&root_id)&&error.to_string().contains("unresolved descendant effects"),"Transition did not actually reach historical Unknown precommit rejection");let requests=std::fs::read_to_string(output_dir.join("requests.jsonl")).unwrap_or_default().lines().filter(|line|!line.is_empty()).count();anyhow::ensure!(requests==0,"Failed transition replayed model work");unresolved_terminal=true;Ok(json!({"mode":mode,"before":before,"after":after,"nativeBefore":native_before,"nativeAfter":native_after,"fault":fault,"transitionError":error.to_string(),"rootReceipt":root_receipt,"rootOriginal":original,"requestsBeforeUnknownEOF":requests}))
            },
            "autonomous_wake_reset"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Reset child alias missing")?).context("Actual reset child missing")?;let runtime=record.runtime.clone().context("Actual reset runtime missing")?;let sibling=aliases.get(operation["sibling"].as_str().context("Reset sibling alias missing")?).context("Actual reset sibling missing")?.clone();let events_path=output_dir.join(format!("{}.events.jsonl",record.key.id));
                owner.coordinator.send(&root_id,&root_id,&record.key.id,"Emit original incremental findings section, then finish this prior run conversationally",None,false).await?;fixture_settle(&owner,&root_id).await?;let prior=fixture_events(&events_path)?;let prior_ends=prior.iter().filter(|event|event["type"]=="tool_execution_end"&&event["toolCallId"]=="prior-section").collect::<Vec<_>>();anyhow::ensure!(prior_ends.len()==1&&prior_ends[0]["isError"]==false&&prior_ends[0]["result"]["details"]["status"]=="success"&&prior_ends[0]["result"]["details"]["data"]=="one finding","Original actual incremental section did not execute before autonomous reset");
                let wake=owner.coordinator.send(&root_id,&sibling,&record.key.id,"Actual autonomous IRC wake: use thinking-only result form",None,false).await?;anyhow::ensure!(!wake.is_error&&wake.receipts[0].outcome==DeliveryOutcome::Woken,"Reset fixture did not enter actual autonomous wake admission");fixture_settle(&owner,&root_id).await?;let after=owner.coordinator.registry.get_exact(&record.key).context("Reset ref disappeared")?;anyhow::ensure!(after.runtime.as_ref()==Some(&runtime),"Autonomous reset replaced the original kept-alive Native runtime");
                let events=fixture_events(&events_path)?;let ends=events.iter().filter(|event|event["type"]=="tool_execution_end"&&event["toolCallId"]=="wake-empty").collect::<Vec<_>>();anyhow::ensure!(ends.len()==1&&ends[0]["isError"]==true&&ends[0]["result"].to_string().contains("no text (thinking only)"),"Autonomous IRC observer retained prior incremental state and accepted thinking-only result");
                let journal=SessionJournal::open_with_blob_directory(record.session_file.as_ref().context("Reset actual journal missing")?,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;let assistant=rows.iter().find(|row|row["role"]=="assistant"&&row["content"].as_array().is_some_and(|content|content.iter().any(|block|block["type"]=="toolCall"&&block["id"]=="wake-empty"))).context("Original actual thinking-only assistant call absent")?;let content=assistant["content"].as_array().unwrap();anyhow::ensure!(content.iter().any(|block|block["type"]=="thinking")&&!content.iter().any(|block|block["type"]=="text"&&block["text"].as_str().is_some_and(|text|!text.trim().is_empty())),"Reset observation did not use original thinking-only/no-text assistant");anyhow::ensure!(rows.iter().any(|row|row["customType"]=="peer-wake"&&row["details"]["messages"].as_array().is_some_and(|messages|messages.iter().any(|message|message["from"]==sibling))),"No actual nonparent autonomous IRC wake receipt");anyhow::ensure!(rows.iter().filter(|row|row["role"]=="toolResult"&&row["toolCallId"]=="wake-empty"&&row["isError"]==true).count()==1,"Original rejected result was replaced or replayed");let prior_position=rows.iter().position(|row|row["role"]=="toolResult"&&row["toolCallId"]=="prior-section").context("Prior incremental durable receipt missing")?;let wake_position=rows.iter().position(|row|row["customType"]=="peer-wake"&&row["details"]["messages"].as_array().is_some_and(|messages|messages.iter().any(|message|message["from"]==sibling))).context("Autonomous wake durable receipt missing")?;let rejected_position=rows.iter().position(|row|row["role"]=="toolResult"&&row["toolCallId"]=="wake-empty").unwrap();anyhow::ensure!(prior_position<wake_position&&wake_position<rejected_position&&events.iter().filter(|event|event["type"]=="agent_start").count()==2,"Incremental/actual new wake/rejected result order or genuine Native run boundary differed");Ok(json!({"runtime":runtime,"priorEvents":prior,"events":events,"originalRejectedResult":ends[0],"rows":rows,"sibling":sibling,"wake":wake,"after":after}))
            },
            "auto_before_wait"=>{
                let child=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Auto-before-Wait child alias missing")?).context("Actual auto-before-Wait child missing")?;let mode=operation["interruptMode"].as_str().context("Actual interrupt mode missing")?;
                root.handle.call(super::rpc_host::NativeAction::Prompt{text:"Call wait while the original child completion is being automatically delivered".into(),images:Vec::new()}).await?;fixture_observe(||Ok(output_dir.join("stream-prefix-held").is_file())).await?;
                owner.coordinator.send(&root_id,&root_id,&child.key.id,"Complete before parent enters the actual streamed Wait",None,false).await?;
                fixture_observe(||Ok(owner.coordinator.jobs.snapshot_parent(&root_id,&root_id).iter().any(|job|job.id.child==child.key&&job.completion.is_some()&&matches!(job.consumption,ParentConsumption::AutoInFlight))&&root_ingress.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).iter().any(|pending|pending.completion.job.child==child.key))).await?;
                let before=owner.coordinator.jobs.snapshot_parent(&root_id,&root_id);let parent_state=root.handle.call(super::rpc_host::NativeAction::State).await?;anyhow::ensure!(parent_state["interruptMode"]==mode,"Actual parent interrupt mode drifted");let queue_count=root_ingress.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).len();anyhow::ensure!(queue_count==1,"Exact busy-parent completion queue was not singular");std::fs::write(output_dir.join("auto-before-wait-ready.json"),serde_json::to_vec_pretty(&json!({"jobs":before,"parentState":parent_state,"actualQueued":queue_count}))?)?;
                root.handle.call(super::rpc_host::NativeAction::WaitIdle).await?;fixture_settle(&owner,&root_id).await?;let events=fixture_events(&output_dir.join("root.events.jsonl"))?;let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="auto-before-native-wait").collect::<Vec<_>>();anyhow::ensure!(ends.len()==1&&ends[0]["isError"]==false,"Actual AutoInFlight-before-Wait did not finish normally exactly once");let end=ends[0].clone();anyhow::ensure!(end["result"]["details"]==json!({"op":"wait","jobs":[],"delivering":true})&&end["result"]["useless"]==true,"AutoInFlight result was copied into Wait or used soft interruption");let notice=end["result"]["content"].as_array().into_iter().flatten().filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n");anyhow::ensure!(notice=="A background result is being delivered to this Session.","Actual Wait did not return its original nonconsuming delivering notice");
                let journal=SessionJournal::open_with_blob_directory(&root_file,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;let parent_results=rows.iter().filter(|row|row["details"]["peerWakeJob"]["child"]["id"]==child.key.id).count();anyhow::ensure!(parent_results==1&&root_ingress.parent_queue.lock().unwrap_or_else(|e|e.into_inner()).is_empty(),"Auto completion did not materialize exactly once after real Wait returned");Ok(json!({"mode":mode,"before":before,"actualQueued":queue_count,"actualEnd":end,"parentResults":parent_results,"rows":rows,"after":owner.coordinator.jobs.snapshot_parent(&root_id,&root_id)}))
            },
            "relay_turn_race"=>{
                let worker=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Relay worker alias missing")?).context("Relay worker ref missing")?;let sibling_id=aliases.get(operation["sibling"].as_str().context("Relay sibling alias missing")?).context("Relay sibling alias missing")?.clone();let runtime=worker.runtime.clone().context("Relay worker Native missing")?;
                owner.coordinator.send(&root_id,&root_id,&sibling_id,"Remain running while old A replies",None,false).await?;fixture_observe(||Ok(output_dir.join("relay-http-held-1").is_file())).await?;
                fixture_configure(&output_dir,json!([{"name":"peer-after-delivery-before-ack","identityContains":sibling_id,"key":"relay-old-ack"}]),json!([]))?;
                owner.coordinator.send(&root_id,&sibling_id,&worker.key.id,"A old turn replies to real sibling",None,false).await?;fixture_observe(||Ok(output_dir.join("frontiers/relay-old-ack.reached.json").is_file())).await?;
                let reached:Value=serde_json::from_slice(&std::fs::read(output_dir.join("frontiers/relay-old-ack.reached.json"))?)?;let old_job=owner.coordinator.jobs.active_root(&root_id).into_iter().find(|id|id.child==worker.key).context("Old relay no longer owns actual finalizer")?;
                owner.coordinator.send(&root_id,&root_id,&worker.key.id,"B new turn must remain running after old relay settles",None,false).await?;fixture_observe(||Ok(output_dir.join("relay-http-held-3").is_file())).await?;
                let new_job=owner.coordinator.jobs.active_root(&root_id).into_iter().find(|id|id.child==worker.key&&id.turn!=old_job.turn).context("New B wake owner was not admitted")?;let before=owner.coordinator.native.inspect(&runtime).await?;anyhow::ensure!(before.running&&owner.coordinator.registry.get_exact(&worker.key).is_some_and(|r|r.status==PeerStatus::Running),"B did not reach actual running owner before old relay ACK");
                fixture_release(&output_dir,"relay-old-ack")?;fixture_observe(||Ok(!owner.coordinator.jobs.active_root(&root_id).contains(&old_job))).await?;
                let state=owner.coordinator.native.inspect(&runtime).await?;let after=owner.coordinator.registry.get_exact(&worker.key).context("Relay worker ref lost")?;anyhow::ensure!(state.running&&after.status==PeerStatus::Running&&owner.coordinator.jobs.active_root(&root_id).contains(&new_job)&&!output_dir.join("relay-release").is_file(),"Old relay finalization overwrote actual running B");
                let receipt=json!({"oldJob":old_job,"newJob":new_job,"reached":reached,"before":before,"stateAfterOldFinalizer":state,"recordAfterOldFinalizer":after,"oldFinalizerSettled":true});std::fs::write(output_dir.join("relay-race-observed.json"),serde_json::to_vec_pretty(&receipt)?)?;std::fs::write(output_dir.join("relay-release"),b"release original sibling and B bodies after actual finalizer observation")?;fixture_configure(&output_dir,json!([]),json!([]))?;fixture_settle(&owner,&root_id).await?;Ok(receipt)
            },
            "streaming_parent_steer"=>{
                let record=owner.coordinator.registry.get(&root_id,aliases.get(&alias).context("Streaming alias missing")?).context("Streaming ref missing")?;let runtime=record.runtime.clone().context("Streaming Native runtime missing")?;let events_path=output_dir.join(format!("{}.events.jsonl",record.key.id));
                let call_id=operation["callId"].as_str().context("Streaming original call id missing")?;let expected_args=operation["arguments"].clone();
                owner.coordinator.send(&root_id,&root_id,&record.key.id,operation["body"].as_str().unwrap_or("Start controlled streaming yield"),None,false).await?;
                fixture_observe(||Ok(fixture_events(&events_path)?.iter().any(|event|event["type"]=="message_update"&&(fixture_message_call(&event["message"],call_id,&expected_args)||fixture_message_call(&event["assistantMessageEvent"]["partial"],call_id,&expected_args))))).await?;
                let before=fixture_events(&events_path)?;anyhow::ensure!(!before.iter().any(|e|e["type"]=="message_end"&&fixture_message_call(&e["message"],call_id,&expected_args)),"Original stream already ended before steer");anyhow::ensure!(!before.iter().any(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]==call_id),"Original tool already executed before streaming steer");
                anyhow::ensure!(output_dir.join("stream-prefix-held").is_file()&&!output_dir.join("stream-finish-released").is_file(),"HTTP stream gate did not hold finish");let state=owner.coordinator.native.inspect(&runtime).await?;anyhow::ensure!(state.running,"Steer did not target an actual streaming Run");
                let steer=owner.coordinator.send(&root_id,&root_id,&record.key.id,operation["steer"].as_str().unwrap_or("Keep the original completed yield call"),None,false).await?;anyhow::ensure!(!steer.is_error&&steer.receipts.len()==1&&steer.receipts[0].outcome==DeliveryOutcome::Injected,"ParentSteer was not actually acknowledged as injected");
                let ack=json!({"runtime":runtime,"beforeEvents":before,"state":state,"actualParentSteer":steer,"at":now_ms()});std::fs::write(output_dir.join("stream-parent-steer-ack.json"),serde_json::to_vec_pretty(&ack)?)?;
                fixture_settle(&owner,&root_id).await?;let events=fixture_events(&events_path)?;let ends=events.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]==call_id).collect::<Vec<_>>();anyhow::ensure!(ends.len()==1&&ends[0]["isError"]==false&&ends[0]["result"]["details"]["status"]=="success"&&ends[0]["result"]["details"]["data"]==expected_args["data"],"Streaming steer replaced, failed or replayed the original yield");
                let file=record.session_file.as_ref().context("Streaming journal missing")?;let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;let rows=journal.entries().iter().filter_map(|entry|entry.observed_message_body().ok().flatten()).map(|body|ara_session::native_checked_serde(&body)).collect::<std::result::Result<Vec<_>,_>>()?;
                let calls=rows.iter().filter(|m|fixture_message_call(m,call_id,&expected_args)).count();let results=rows.iter().filter(|m|m["role"]=="toolResult"&&m["toolCallId"]==call_id&&m["isError"]!=true&&m["details"]["status"]=="success"&&m["details"]["data"]==expected_args["data"]).count();anyhow::ensure!(calls==1&&results==1,"Original streamed yield journal receipt not exactly once");
                Ok(json!({"ack":ack,"events":events,"originalCallCount":calls,"originalResultCount":results,"after":owner.coordinator.registry.get_exact(&record.key),"gateReleased":output_dir.join("stream-finish-released").is_file()}))
            },
            _=>bail!("Unknown Peer fixture op {action}"),
        }}.await;outcome};
        let timed=tokio::time::timeout(Duration::from_secs(25),future).await;
        if timed.is_err()&&action=="parent_wait"{std::fs::write(output_dir.join(format!("parent-wait-{index}-timeout.json")),serde_json::to_vec_pretty(&json!({"jobs":owner.coordinator.jobs.snapshot_root(&root_id),"active":owner.coordinator.jobs.active_root(&root_id),"registry":owner.coordinator.registry.list(&root_id)}))?)?;}
        let outcome=timed.unwrap_or_else(|_|Err(anyhow::anyhow!("Peer operation timeout; effects may be unknown, no replay")));
        let success=outcome.is_ok();let result=match outcome{Ok(result)=>serde_json::json!({"result":result}),Err(error)=>serde_json::json!({"error":error.to_string()})};receipts.push(serde_json::json!({"index":index,"op":action,"success":success,"data":result,"elapsedMs":now_ms()-started}));
        if success==operation["expectedError"].as_bool().unwrap_or(false){std::fs::write(output_dir.join("partial-peer-receipts.json"),serde_json::to_vec_pretty(&receipts)?)?;bail!("Peer fixture operation {index} unexpected success={success}: {}",receipts.last().unwrap());}
    }
    if !unresolved_terminal{fixture_settle(&owner,&root_id).await?;}
    let root_closed=root.close().await;
    // The Host receipt includes retained historical closes and Task resources
    // that the library-only lifecycle snapshot cannot observe after unregister.
    let root_shutdown=fixture_events(&output_dir.join("root.events.jsonl"))?.into_iter().rev()
        .find(|event|event["type"]=="peer_root_shutdown")
        .context("Actual Main root shutdown receipt missing")?;
    let actual_teardown=json!({"actualEOF":(match &root_closed{Ok(code)=>json!({"code":code}),Err(error)=>json!({"error":error.to_string()})}),"lifecycleRootSnapshot":root_shutdown["receipt"],"lifecycleRootSnapshotSource":"actual peer_root_shutdown receipt","libraryLifecycleRootSnapshot":owner.coordinator.lifecycle.root_close_snapshot(&root_id)});
    println!("PEER_FIXTURE_TEARDOWN={}",serde_json::to_string(&actual_teardown)?);
    println!("PEER_FIXTURE_RECEIPTS={}",serde_json::to_string(&receipts)?);Ok(root_closed?)
}
#[cfg(feature="test-fixture")]
async fn fixture_settle(owner:&Arc<BinaryPeerHost>,root:&str)->Result<()>{
    tokio::time::timeout(Duration::from_secs(20),async{loop{
        let mut busy=false;for record in owner.coordinator.registry.list(root){if let Some(runtime)=record.runtime{busy|=owner.coordinator.jobs.has_owned(&runtime)||owner.coordinator.native.inspect(&runtime).await.is_ok_and(|state|state.running||state.dispatch_active);}}
        if !busy{return Ok(());}tokio::time::sleep(Duration::from_millis(5)).await;
    }}).await.context("Peer settlement timed out; inspect retained Native/job receipts before retry")?
}

// Private, controlled verification of the existing concrete relay/actor seam.
// Never inserts a display frame or a recipient success receipt directly.
#[cfg(feature="test-fixture")]
async fn fixture_relay_operation(action:&str,operation:&Value,owner:&Arc<BinaryPeerHost>,root:&super::rpc_host::NativeSessionRuntime,root_id:&str,root_file:&Path,dir:&Path,aliases:&HashMap<String,String>)->Result<Value>{
    let main=owner.coordinator.registry.get(root_id,root_id).and_then(|r|r.runtime).context("Actual relay Main missing")?;
    let target=aliases.get(operation["recipient"].as_str().unwrap_or("receiver")).context("Actual relay recipient alias missing")?;
    let recipient=owner.coordinator.registry.get(root_id,target).context("Actual relay recipient record missing")?;
    match action{
        "relay_prepare"=>{
            struct Policy(String);impl ara_cli::host_edit_session::EditPlanPolicy for Policy{fn current_plan_path(&self)->Option<String>{Some(self.0.clone())}}
            // Plan append makes the recipient and Main deterministic without
            // fabricating an IRC delivery or starting an extra model wake.
            for runtime in [main.clone(),recipient.runtime.clone().context("Recipient runtime missing")?]{owner.request(&runtime,PeerActorOp::FixturePlan(Arc::new(Policy(dir.join("relay-plan.md").to_string_lossy().into_owned())))).await?;}
            let wire_target=if operation["targetMain"]==true{root_id}else{target.as_str()};
            std::fs::write(dir.join("relay-target-id.txt"),wire_target)?;
            let mode=operation["observer"].as_str().unwrap_or("");let mut state=owner.relay_fixture.lock().unwrap();state.mode=mode.into();state.held.clear();state.attempts.clear();
            Ok(json!({"main":main,"target":recipient,"observer":mode}))
        },
        "relay_release"=>{
            let(sender_id,held)={let mut state=owner.relay_fixture.lock().unwrap();(aliases.get(operation["sender"].as_str().unwrap_or("sender")).context("Actual relay sender missing")?.clone(),std::mem::take(&mut state.held))};
            let ended=fixture_events(&dir.join(format!("{sender_id}.events.jsonl")))?.into_iter().find(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="relay-native-write").context("Held observer prevented original Native WRITE completion")?;
            anyhow::ensure!(ended["isError"]==false&&held.len()==1,"Held observer altered actual tool receipt");
            anyhow::ensure!(fixture_events(&dir.join("root.events.jsonl"))?.iter().all(|e|e["type"]!="irc_message"),"Held observation was already displayed");
            let original=held[0].clone();for(runtime,message)in held{owner.enqueue_relay_now(&runtime,&message)?;}
            fixture_observe(||Ok(fixture_events(&dir.join("root.events.jsonl"))?.iter().any(|e|e["type"]=="irc_message"&&e["message"]["timestamp"]==original.1.ts))).await?;
            Ok(json!({"toolEndBeforeObserverRelease":ended,"releasedOriginal":original.1,"deliveryNotReplayed":true}))
        },
        "relay_snapshot"=>{
            if operation["expectRelay"]==true{fixture_observe(||Ok(fixture_events(&dir.join("root.events.jsonl"))?.iter().any(|e|e["type"]=="irc_message"))).await?;}
            let live=root.handle.call(super::rpc_host::NativeAction::Messages).await?;
            let rows=|file:&Path|->Result<Vec<Value>>{let journal=SessionJournal::open_with_blob_directory(file,&ara_blobs_directory())?;journal.entries().iter().filter_map(|entry|match entry.observed_message_body(){Ok(Some(body))=>Some(ara_session::native_checked_serde(&body).map_err(Into::into)),Ok(None)=>None,Err(error)=>Some(Err(error.into()))}).collect()};
            let attempts=owner.relay_fixture.lock().unwrap().attempts.clone();
            Ok(json!({"main":main,"recipient":recipient,"rootRows":rows(root_file)?,"recipientRows":rows(recipient.session_file.as_deref().context("Recipient file missing")?)?,"liveMain":live,"attempts":attempts,"events":fixture_events(&dir.join("root.events.jsonl"))?}))
        },
        "relay_concrete_guards"=>{
            let message=PeerMessage{id:"isolated-observation-not-a-delivery".into(),from:aliases.get("sender").context("Sender missing")?.clone(),to:target.clone(),body:"guard-only display probe".into(),ts:123,reply_to:None,wake_relay:false};
            let before=fixture_events(&dir.join("root.events.jsonl"))?.iter().filter(|e|e["type"]=="irc_message").count();
            let endpoint=owner.endpoint(&main)?;
            for foreign in [false,true]{let mut stale=main.clone();if foreign{stale.peer.root="different-root".into();}else{stale.runtime_generation+=1;}let(reply,received)=oneshot::channel();endpoint.send(PeerActorRequest{runtime:stale,operation:PeerActorOp::RelayObservation(message.clone()),reply}).map_err(|_|anyhow::anyhow!("Actual observer endpoint closed"))?;anyhow::ensure!(received.await?.is_err(),"Actual actor admitted stale/cross-root observation");}
            // The real concrete enqueue path sees a genuinely closed endpoint.
            let isolated=BinaryPeerHost::new();let(tx,rx)=mpsc::unbounded_channel();let isolated_main=isolated.bind_actor("private-root","private-main-uuid",None,None,tx,AttachmentExpectation::Absent)?;drop(rx);
            anyhow::ensure!(isolated.enqueue_relay_now(&isolated_main,&message).is_err(),"Closed concrete observer accepted a frame");
            isolated.coordinator.registry.unregister(&isolated_main.peer);let(tx,_rx)=mpsc::unbounded_channel();let replacement=isolated.bind_actor("private-root","private-main-uuid",None,None,tx,AttachmentExpectation::Absent)?;
            anyhow::ensure!(isolated.enqueue_relay_now(&isolated_main,&message).is_err(),"Concrete enqueue retargeted old observation");
            anyhow::ensure!(isolated.coordinator.registry.seal_root("private-root",&replacement),"Isolated root did not seal");anyhow::ensure!(isolated.enqueue_relay_now(&replacement,&message).is_err(),"Sealed concrete root accepted observation");
            // Last operation only: seal the actual Main root, then consume an
            // exact already-queued actor request. Cleanup still owns the root.
            anyhow::ensure!(owner.coordinator.registry.seal_root(root_id,&main),"Actual root did not seal");let(reply,received)=oneshot::channel();endpoint.send(PeerActorRequest{runtime:main,operation:PeerActorOp::RelayObservation(message),reply}).map_err(|_|anyhow::anyhow!("Actual observer closed"))?;anyhow::ensure!(received.await?.is_err(),"Actual actor displayed after root sealing");
            let after=fixture_events(&dir.join("root.events.jsonl"))?.iter().filter(|e|e["type"]=="irc_message").count();anyhow::ensure!(before==after,"Rejected observer probes emitted a display frame");Ok(json!({"staleActualActorRejected":true,"crossRootActualActorRejected":true,"closedConcreteEndpointRejected":true,"replacementNotRetargeted":true,"sealedActualActorRejected":true,"beforeFrames":before,"afterFrames":after}))
        },
        _=>bail!("Unknown relay fixture action")
    }
}
