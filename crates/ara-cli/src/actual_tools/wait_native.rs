//! Final Native Wait consumer, fixed OMP b91db66e tools/wait.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Producers retain their own truth. This owner only watches and adopts results.
use super::*;
use crate::host_peers::{ForegroundReservation,ParentAdoption,ParentConsumption,PeerCoordinator,PeerMessage,PeerQueues,RuntimeKey};
use crate::host_services::SessionServiceLease;
use std::time::Duration;
use futures::FutureExt;

pub struct WaitTool;
#[async_trait]
impl AgentTool for WaitTool {
    fn definition(&self)->Tool {Tool{name:"wait".into(),description:include_str!("prompts/wait.md").trim_end().into(),parameters:serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),strict:Some(true),..Default::default()}}
    fn intent_mode(&self)->ara_agent::ToolIntentMode{ara_agent::ToolIntentMode::Optional}
    fn interruptible(&self,_:&JsonObject)->bool{true}
    async fn execute(&self,_:&str,_:JsonObject,_:CancellationToken,_:UpdateFn)->Result<ToolOutput,ToolError>{Err(ToolError("Wait requires its actual Native Session ownership and steering capability".into()))}
}
#[derive(Clone)]
struct PeerBinding {coordinator:Arc<PeerCoordinator>,queues:Arc<PeerQueues>,runtime:RuntimeKey}
struct Claim {
    call_id:ara_rpc::WireString,
    peer:Option<(Arc<crate::host_peers::PeerWakeJobs>,ForegroundReservation)>,
    bash:Vec<String>,
    details:Option<ValueHandle>,
    unknown:Option<String>,
}
#[derive(Default)]
struct WaitState {session:Option<(String,bool)>,binding:Option<PeerBinding>,service:Option<Arc<SessionServiceLease>>,claims:HashMap<u64,Claim>,closed:bool}
/// Short, synchronous publication ticket. The actual Host performs every
/// await before acquiring it; dropping preparation leaves its old binding intact.
pub struct PreparedWaitBinding<'a> {
    state:std::sync::MutexGuard<'a,WaitState>,
    session:(String,bool),
    binding:PeerBinding,
}
impl PreparedWaitBinding<'_> {
    pub fn commit(mut self){self.state.session=Some(self.session);self.state.binding=Some(self.binding);}
}
pub struct SessionWaitOwner {
    bash:Weak<bash_native::SessionBashOwner>,
    values:Mutex<Option<Weak<SessionValues>>>,
    state:Mutex<WaitState>,
    closed:CancellationToken,
}
impl SessionWaitOwner {
    pub(super) fn new(bash:Weak<bash_native::SessionBashOwner>)->Arc<Self>{Arc::new(Self{bash,values:Mutex::new(None),state:Mutex::new(WaitState::default()),closed:CancellationToken::new()})}
    pub fn bind_values(&self,values:&Arc<SessionValues>)->Result<(),ActualProviderError>{
        let mut old=self.values.lock().unwrap_or_else(|e|e.into_inner());
        if old.as_ref().is_some_and(|old|old.upgrade().is_none_or(|old|!Arc::ptr_eq(&old,values))){return Err(ActualProviderError::Protocol("Wait belongs to another logical Session"));}
        *old=Some(Arc::downgrade(values));Ok(())
    }
    pub fn bind_session(&self,id:&str,persistent:bool)->anyhow::Result<()>{
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!state.closed&&!id.is_empty(),"Wait Session is closed or lacks identity");
        anyhow::ensure!(state.session.as_ref().is_none_or(|old|old==&(id.to_owned(),persistent)),"Wait cannot replace its original logical Session");
        state.session=Some((id.into(),persistent));Ok(())
    }
    pub fn bind_peer(&self,coordinator:Arc<PeerCoordinator>,queues:Arc<PeerQueues>,runtime:RuntimeKey)->anyhow::Result<()>{
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!state.closed,"Wait Session closed");
        anyhow::ensure!(state.binding.as_ref().is_none_or(|old|old.runtime==runtime),"Wait binding epoch changed; new logical Session owner required");
        state.binding=Some(PeerBinding{coordinator,queues,runtime});Ok(())
    }
    pub fn is_closed(&self)->bool{self.closed.is_cancelled()||self.state.lock().unwrap_or_else(|e|e.into_inner()).closed}
    pub fn prepare_binding<'a>(&'a self,id:&str,persistent:bool,coordinator:Arc<PeerCoordinator>,queues:Arc<PeerQueues>,runtime:RuntimeKey,retired:&[(Arc<PeerCoordinator>,crate::host_peers::RootCloseReceipt)])->anyhow::Result<PreparedWaitBinding<'a>>{
        let state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        anyhow::ensure!(!state.closed&&!self.closed.is_cancelled()&&!id.is_empty()&&runtime.peer.id==id,"Wait binding lacks its live logical Session");
        anyhow::ensure!(state.session.as_ref().is_none_or(|old|old==&(id.to_owned(),persistent)),"Wait cannot replace its original logical Session");
        if let Some(old)=state.binding.as_ref().filter(|old|old.runtime!=runtime){
            anyhow::ensure!(old.runtime.peer.id==id&&old.runtime.peer.root==runtime.peer.root&&state.claims.is_empty(),"Wait Peer reactivation differs from logical Session or retains unresolved private claims");
            let proof=retired.iter().find(|(owner,receipt)|Arc::ptr_eq(owner,&old.coordinator)&&receipt.expected==old.runtime&&receipt.root==old.runtime.peer.root&&receipt.settled).map(|(_,receipt)|receipt);
            anyhow::ensure!(proof.is_some()&&old.coordinator.registry.root_is_sealed(&old.runtime.peer.root)&&old.coordinator.lifecycle.root_close_snapshot(&old.runtime.peer.root).is_some_and(|receipt|receipt.expected==old.runtime&&receipt.settled),"Wait Peer reactivation lacks the actual archived exact-runtime settlement receipt");
        }
        Ok(PreparedWaitBinding{state,session:(id.to_owned(),persistent),binding:PeerBinding{coordinator,queues,runtime}})
    }
    fn adoption_notice()->ToolOutput{let mut output=ToolOutput::text("A background result is being delivered to this Session.").with_details(serde_json::json!({"op":"wait","jobs":[],"delivering":true}));output.useless=true;output}
    pub fn bind_service(&self,service:Option<Arc<SessionServiceLease>>){self.state.lock().unwrap_or_else(|e|e.into_inner()).service=service;}
    fn queued(binding:Option<&PeerBinding>)->Option<PeerMessage>{
        let binding=binding?;
        binding.queues.take_inbox(&binding.runtime,None,1).into_iter().next().or_else(||binding.coordinator.inbox.take(&binding.runtime.peer.root,&binding.runtime.peer.id,None))
    }
    fn message_output(binding:&PeerBinding,message:PeerMessage)->ToolOutput {
        let parent=binding.coordinator.registry.get_exact(&binding.runtime.peer).and_then(|record|record.parent_id);
        ToolOutput::text(crate::host_peers::native::render_incoming(&message,false,false,parent.as_deref()==Some(message.from.as_str())))
            .with_details(serde_json::json!({"op":"wait","jobs":[],"message":crate::host_peers::native::incoming_details(&message,parent.as_deref()==Some(message.from.as_str()),false,false)}))
    }
    fn snapshot_jobs(peer:Option<&crate::host_peers::PeerJobWatch>,bash:&bash_native::BashWaitWatch)->Vec<serde_json::Value>{
        let mut rows=Vec::new();
        if let Some(peer)=peer {for row in peer.snapshots(){rows.push(serde_json::json!({"kind":"task","id":row.id,"runtime":row.runtime,"status":row.completion.as_ref().map_or("running",|completion|if completion.outcome.cancelled{"cancelled"}else if completion.outcome.error.is_some(){"failed"}else{"completed"}),"consumption":row.consumption,"result":row.completion}));}}
        for row in bash.snapshots(){rows.push(serde_json::json!({"kind":"bash","id":row.id,"label":row.label,"status":row.status,"output":row.latest_text}));}
        rows
    }
    fn reserve_output(&self,invocation:u64,call_id:&ara_rpc::WireString,binding:Option<&PeerBinding>,peer:Option<&crate::host_peers::PeerJobWatch>,bash:&bash_native::BashWaitWatch)->Option<ToolOutput>{
        let peer_reservation=peer.and_then(|watch|watch.reserve_ready());
        let bash_ready=bash.reserve_ready();
        if peer_reservation.is_none()&&bash_ready.is_empty(){return None;}
        let mut text=Vec::new();
        if let Some(reservation)=&peer_reservation{for completion in reservation.completions(){text.push(completion.outcome.relay_body(&completion.job.child.id,false));}}
        for row in &bash_ready{text.push(format!("{} ({})\n{}",row.label,row.status,row.latest_text));}
        let details=serde_json::json!({"op":"wait","jobs":Self::snapshot_jobs(peer,bash)});
        let peer=peer_reservation.map(|reservation|(binding.expect("Peer watch has binding").coordinator.jobs.clone(),reservation));
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        // Only an authentic Native invocation can create this private claim.
        let output=ToolOutput::text(text.join("\n\n")).with_details(details);
        state.claims.insert(invocation,Claim{call_id:call_id.clone(),peer,bash:bash_ready.into_iter().map(|row|row.id).collect(),details:None,unknown:None});
        Some(output)
    }
    pub(super) async fn execute(self:Arc<Self>,invocation:u64,call_id:ara_rpc::WireString,cancel:CancellationToken,soft:Option<CancellationToken>,update:UpdateFn)->Result<ToolOutput,ToolError>{
        let (binding,service)={let state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.closed{return Err(ToolError("Wait Session closed".into()));}(state.binding.clone(),state.service.clone())};
        if let Some(message)=Self::queued(binding.as_ref()){return Ok(Self::message_output(binding.as_ref().unwrap(),message));}
        if let Some(service)=service.as_ref().filter(|service|service.launch_enabled()){
            service.refresh_owned_tolerant(&cancel).await.map_err(|error|ToolError(error.to_string()))?;
            if let Some(message)=Self::queued(binding.as_ref()){return Ok(Self::message_output(binding.as_ref().unwrap(),message));}
        }
        let bash=self.bash.upgrade().ok_or_else(||ToolError("Wait Bash Session owner disposed".into()))?;
        let bash_watch=bash.watch_foreground(invocation);
        let peer_watch=binding.as_ref().map(|binding|binding.coordinator.jobs.watch_parent(&binding.runtime.peer.root,&binding.runtime.peer.id));
        if let Some(output)=self.reserve_output(invocation,&call_id,binding.as_ref(),peer_watch.as_ref(),&bash_watch){return Ok(output);}
        let auto_adopting=||peer_watch.as_ref().is_some_and(|watch|watch.snapshots().iter().any(|row|row.completion.is_some()&&matches!(row.consumption,ParentConsumption::AutoInFlight)));
        // Automatic adoption keeps its original single consumer. In interrupt
        // mode Wait, returning this notice opens the actual aside boundary;
        // no Immediate-only soft signal and no second result are required.
        if auto_adopting(){return Ok(Self::adoption_notice());}
        let running=||bash_watch.snapshots().iter().any(|row|row.result.is_none())||peer_watch.as_ref().is_some_and(|watch|watch.snapshots().iter().any(|row|matches!(row.consumption,ParentConsumption::AutoInFlight)||(row.completion.is_none()&&matches!(row.consumption,ParentConsumption::Ready))));
        let service_running=service.as_ref().is_some_and(|service|service.has_live_owned_service());
        if !running()&&!service_running{return Err(ToolError("Nothing to wait for: no background job or service you started is running. Other agents' results and messages arrive on their own.".into()));}
        let bus_cancel=CancellationToken::new();
        let mut bus=if let Some(binding)=binding.clone(){let stop=bus_cancel.clone();async move{binding.coordinator.inbox.wait(&binding.runtime.peer.root,&binding.runtime.peer.id,None,0,true,&stop).await}.boxed()}else{let stop=bus_cancel.clone();async move{stop.cancelled().await;Ok(None)}.boxed()};
        let bash_wake=bash.wake();
        let peer_wake=binding.as_ref().map(|binding|binding.coordinator.jobs.wake()).unwrap_or_else(||Arc::new(tokio::sync::Notify::new()));
        let service_wake=service.as_ref().map(|service|service.wake()).unwrap_or_else(||Arc::new(tokio::sync::Notify::new()));
        let deadline=tokio::time::sleep(Duration::from_secs(30*60));tokio::pin!(deadline);
        let mut progress=tokio::time::interval(Duration::from_millis(500));
        let soft=soft.unwrap_or_else(CancellationToken::new);
        let mut adoption_pending=false;
        let wake=loop {
            let changed_bash=bash_wake.notified();let changed_peer=peer_wake.notified();let changed_service=service_wake.notified();
            tokio::pin!(changed_bash,changed_peer,changed_service);changed_bash.as_mut().enable();changed_peer.as_mut().enable();changed_service.as_mut().enable();
            if let Some(message)=Self::queued(binding.as_ref()){break (Some(Ok(Some(message))),false);}
            if cancel.is_cancelled()||self.closed.is_cancelled(){break (None,false);}
            if soft.is_cancelled(){break (None,false);}
            if auto_adopting(){adoption_pending=true;break (None,false);}
            if service_running&&service.as_ref().is_some_and(|service|service.has_pending()||!service.has_live_owned_service()){break (None,false);}
            if peer_watch.as_ref().is_some_and(|watch|watch.snapshots().iter().any(|row|row.completion.is_some()&&matches!(row.consumption,ParentConsumption::Ready)))||bash_watch.snapshots().iter().any(|row|row.result.is_some()) {break (None,false);}
            tokio::select!{biased;
                message=&mut bus=>break (Some(message),true),
                _=cancel.cancelled()=>break (None,false),
                _=self.closed.cancelled()=>break (None,false),
                _=soft.cancelled()=>break (None,false),
                _=&mut changed_bash=>{},_=&mut changed_peer=>{},
                _=&mut changed_service=>break (None,false),
                _=&mut deadline=>break (None,false),
                _=progress.tick()=>{let mut partial=ToolOutput::text("");partial.details=Some(serde_json::json!({"op":"wait","jobs":Self::snapshot_jobs(peer_watch.as_ref(),&bash_watch)}));update(partial);},
            }
        };
        // Cancel the message leg, then await it once: a dequeued message wins
        // a photo finish, and no detached waiter can steal future inbox data.
        let (wake,bus_taken)=wake;
        bus_cancel.cancel();
        let dequeued=if bus_taken{None}else{bus.await.ok().flatten()};
        let direct=wake.and_then(Result::ok).flatten();
        // Two simultaneous messages remain two messages; returning a queued
        // injection must not drop a message already dequeued by the bus leg.
        let message=if let Some(direct)=direct{if let(Some(binding),Some(other))=(binding.as_ref(),dequeued){binding.coordinator.inbox.enqueue(&binding.runtime.peer.root,other);}Some(direct)}else{dequeued};
        if let Some(message)=message{return Ok(Self::message_output(binding.as_ref().expect("message requires binding"),message));}
        if self.closed.is_cancelled(){return Err(ToolError("Wait Session closed".into()));}
        // BatchInterrupt sets this source-owned signal only when steering won
        // before external abort. A genuine abort without that source throws;
        // the invocation separately rechecks actual grant revocation.
        if soft.is_cancelled(){let mut output=ToolOutput::text("Wait interrupted by message.").with_details(serde_json::json!({"op":"wait","jobs":[],"interrupted":true}));output.useless=true;return Ok(output);}
        if cancel.is_cancelled(){return Err(ToolError("Aborted: Cancelled".into()));}
        if let Some(output)=self.reserve_output(invocation,&call_id,binding.as_ref(),peer_watch.as_ref(),&bash_watch){return Ok(output);}
        if adoption_pending||auto_adopting(){return Ok(Self::adoption_notice());}
        let text=if service_running&&service.as_ref().is_some_and(|service|service.has_pending()||!service.has_live_owned_service()){"A service finished. Read proc:// for its status and output."}else{"Wait limit reached; background work may still be running. Read proc:// for status."};
        Ok(ToolOutput::text(text).with_details(serde_json::json!({"op":"wait","jobs":Self::snapshot_jobs(peer_watch.as_ref(),&bash_watch)})))
    }
    pub(super) fn bind_result(&self,invocation:u64,call_id:&ara_rpc::WireString,output:&ValueHandle,values:&SessionValues)->Result<(),ActualProviderError>{
        let details=values.heap().get(output,&"details".into(),values.runtime())?;
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(claim)=state.claims.get_mut(&invocation){if &claim.call_id!=call_id{return Err(ActualProviderError::Protocol("Wait original call identity changed"));}claim.details=Some(details);}
        Ok(())
    }
    fn claim_for_message(&self,message:&ara_agent::AgentMessage)->Option<u64>{
        let ara_agent::AgentMessageBody::Graph(root)=message.body_snapshot()else{return None};
        let values=self.values.lock().unwrap_or_else(|e|e.into_inner()).as_ref()?.upgrade()?;
        values.check_handle(&root).ok()?;let heap=values.heap();
        let role=heap.get(&root,&"role".into(),values.runtime()).ok()?;
        if !matches!(heap.snapshot(&role).ok()?,ValueSnapshot::String(role) if role.equals_ascii("toolResult")){return None;}
        let is_error=heap.get(&root,&"isError".into(),values.runtime()).ok()?;
        if heap.truthy(&is_error).ok()?{return None;}
        let call=heap.get(&root,&"toolCallId".into(),values.runtime()).ok()?;
        let ValueSnapshot::String(call)=heap.snapshot(&call).ok()? else{return None};
        let details=heap.get(&root,&"details".into(),values.runtime()).ok()?;
        self.state.lock().unwrap_or_else(|e|e.into_inner()).claims.iter().find(|(_,claim)|claim.call_id==call&&claim.details.as_ref().is_some_and(|original|original.same_identity(&details))).map(|(token,_)|*token)
    }
    pub fn requires_durable_append(&self,message:&ara_agent::AgentMessage)->bool{self.claim_for_message(message).is_some()}
    pub fn append_receipt(&self,message:&ara_agent::AgentMessage,receipt:Result<ParentAdoption,String>)->Result<(),ToolError>{
        let Some(token)=self.claim_for_message(message)else{return Ok(())};
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        let valid=receipt.and_then(|receipt|if receipt.persistent&&!receipt.entry_id.is_empty()&&state.session.as_ref().is_some_and(|(id,persistent)|*persistent&&*id==receipt.session_id){Ok(receipt)}else{Err("Wait ToolResult receipt is not durable or belongs to another Session".into())});
        let claim=state.claims.get_mut(&token).expect("private wait claim retained");
        match valid {
            Ok(receipt)=>{if let Some((jobs,reservation))=&claim.peer{jobs.commit_foreground(reservation,receipt).map_err(|error|ToolError(error.to_string()))?;}if let Some(bash)=self.bash.upgrade(){bash.commit_foreground(token,&claim.bash)?;}state.claims.remove(&token);Ok(())},
            Err(error)=>{claim.unknown=Some(error.clone());if let Some((jobs,reservation))=&claim.peer{let _=jobs.mark_foreground_unknown(reservation,error.clone());}if let Some(bash)=self.bash.upgrade(){bash.mark_foreground_unknown(token,&claim.bash,&error);}Err(ToolError(error))},
        }
    }
    pub fn release_unappended(&self){let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());for(token,claim)in &mut state.claims {let error="Run ended before private Native Wait ToolResult durable materialization";claim.unknown=Some(error.into());if let Some((jobs,reservation))=&claim.peer{let _=jobs.mark_foreground_unknown(reservation,error);}if let Some(bash)=self.bash.upgrade(){bash.mark_foreground_unknown(*token,&claim.bash,error);}}}
    pub fn close(&self){self.closed.cancel();self.state.lock().unwrap_or_else(|e|e.into_inner()).closed=true;self.release_unappended();}
}

// Same-layer old IRC1122 / fixed b91 tools/wait.ts:74 contract. This helper is
// called only by the existing libtest Peer P4 bank, never by a product Host or
// public Native admission. It invokes the original kernel and real queues.
#[cfg(test)]
pub(crate) async fn check_pending_before_abort_contract(coordinator:Arc<PeerCoordinator>,runtime:RuntimeKey){
    for (hard,soft) in [(true,false),(false,true),(true,true)]{
        let queues=Arc::new(PeerQueues::default());
        let owner=SessionWaitOwner::new(Weak::new());
        owner.bind_session(&runtime.peer.id,false).unwrap();
        owner.bind_peer(coordinator.clone(),queues.clone(),runtime.clone()).unwrap();
        let message=PeerMessage{id:format!("pending-hard-{hard}-soft-{soft}"),from:"Main".into(),to:runtime.peer.id.clone(),body:"queued original message before interrupt".into(),ts:crate::host_peers::now_ms(),reply_to:None,wake_relay:false};
        queues.interrupt(&runtime,crate::host_peers::NativeDelivery{route:crate::host_peers::DeliveryRoute::SiblingInterrupt,rendered:"queued original message before interrupt".into(),message:message.clone()});
        let cancel=CancellationToken::new();let signal=CancellationToken::new();if hard{cancel.cancel();}if soft{signal.cancel();}
        let output=owner.clone().execute(1,"same-layer-pending-wait".into(),cancel,Some(signal),Arc::new(|_|{})).await.unwrap();
        assert!(!output.is_error);assert!(serde_json::to_value(&output.content).unwrap().to_string().contains(&message.body));let details=output.details.unwrap();
        assert_eq!(details["op"],"wait");assert_eq!(details["message"]["id"],message.id);assert_eq!(details["message"]["message"],message.body);assert_eq!(details["message"]["from"],message.from);assert_eq!(details["message"]["to"],message.to);
        assert!(!queues.has_pending(&runtime));assert!(queues.drain_injection(&runtime).is_empty());assert_eq!(coordinator.inbox.unread(&runtime.peer.root,&runtime.peer.id),0);
    }
}
