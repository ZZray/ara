//! Console Task/Peer Host, fixed OMP b91 task/index.ts and irc-bridge.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! This actor never runs an Agent. It prepares existing ChildHosts and queues
//! Main wake input for main.rs's existing Native Agent loop.
use super::*;
use ara_cli::host_peers::{AttachmentExpectation,NativePeerState,ParentAdoption,PeerStatus,RuntimeKey,WakeOutcome};
use std::{collections::VecDeque,sync::{Mutex,Weak,OnceLock}};
use tokio::sync::{mpsc,oneshot,Notify,watch};

struct PendingWake {request:ara_cli::host_peers::WakeRequest,cancel:CancellationToken,reply:oneshot::Sender<Result<peer_host::PeerActorReply>>}
type CloseReceipt=Option<std::result::Result<(),String>>;
static ROOTS:OnceLock<Mutex<Vec<Arc<ConsoleTaskHost>>>>=OnceLock::new();
pub(super) struct ConsoleTaskHost {
    pub peers:Arc<peer_host::BinaryPeerHost>,
    runtime:RuntimeKey,
    dispatch:task_host::TaskDispatch,
    template:tokio::sync::Mutex<rpc_host::SessionFactory>,
    native:Mutex<Arc<native_setup::NativeCliOwner>>,
    plan:Mutex<Option<Arc<ara_cli::host_edit_session::HostEditSession>>>,
    sink:Weak<HostSink>,
    admission:tokio::sync::Mutex<()>,
    running:AtomicBool,closed:AtomicBool,unknown:AtomicBool,
    idle_messages:Mutex<Vec<AgentMessage>>,
    wakes:Mutex<VecDeque<PendingWake>>,active_wake:Mutex<Option<PendingWake>>,
    pub wake:Arc<Notify>,
    close:Mutex<Option<watch::Receiver<CloseReceipt>>>,
    actor_stop:CancellationToken,
}
fn factory(args:&Args,cwd:&Path,dir:Option<PathBuf>,provider:&ProviderFactory,owner:&CliToolOwner,jobs:native_setup::NativeSessionJobs,mcp:&Option<McpServerConfig>,peers:Arc<peer_host::BinaryPeerHost>,root:String)->rpc_host::SessionFactory{
    rpc_host::SessionFactory{peer_messaging_enabled:true,agent_context:None,peers:Some(peers),peer_root:Some(root),peer_attachment:None,dir,cwd:cwd.into(),provider:provider.borrowed_side_factory(),args:args.clone(),explicit_generation:ara_cli::daily_model_config::DailyGeneration{max_tokens:args.max_tokens,temperature:args.temperature},mcp_config:mcp.clone(),tool_owner:owner.clone(),uri_port:owner.uri_base.clone(),startup:None,context_controller:None,actual_values_factory:None,initial_native_session:owner.native.as_ref().map(|native|native.prepared.clone()),native_jobs:jobs,acp_plan:None}
}
#[allow(clippy::too_many_arguments)]
pub(super) async fn install(args:&Args,cwd:&Path,dir:Option<PathBuf>,provider:&ProviderFactory,owner:&CliToolOwner,jobs:native_setup::NativeSessionJobs,mcp:&Option<McpServerConfig>,sink:&Arc<HostSink>)->Result<Option<Arc<ConsoleTaskHost>>>{
    if !owner.owned_builtins.iter().any(|tool|tool.definition().name=="task"){return Ok(None);}
    let native=owner.native.clone().context("Console Task requires actual Native ownership")?;
    let (id,file,persistent)={let guard=sink.journal.lock().await;let journal=guard.as_ref().context("Console Task requires its actual journal")?;(journal.session_id().to_owned(),journal.is_persistent().then(||journal.path().to_owned()),journal.is_persistent())};
    if let Some(existing)=sink.task_host(){
        anyhow::ensure!(existing.runtime.peer.id==id,"Console Task cannot retarget its original Session");
        existing.refresh(args,cwd,dir,provider,owner,jobs,mcp).await?;return Ok(Some(existing));
    }
    let peers=peer_host::BinaryPeerHost::new();let(peer_tx,peer_rx)=mpsc::unbounded_channel();let(task_tx,task_rx)=mpsc::unbounded_channel();
    let attachment=peers.prepare_attachment(&id,&id,None,file.clone(),peer_tx,AttachmentExpectation::Absent,None)?;
    peers.bind_native(&native,&attachment.runtime,persistent)?;
    let dispatch=task_host::TaskDispatch{sender:task_tx,scheduler:Arc::new(task_host::TaskScheduler::default())};
    task_host::bind(&dispatch,&native,&id,cwd,owner.edit_session.clone())?;
    let host=Arc::new(ConsoleTaskHost{runtime:attachment.runtime.clone(),dispatch,template:tokio::sync::Mutex::new(factory(args,cwd,dir,provider,owner,jobs,mcp,peers.clone(),id.clone())),native:Mutex::new(native),plan:Mutex::new(owner.edit_session.clone()),sink:Arc::downgrade(sink),admission:tokio::sync::Mutex::new(()),running:AtomicBool::new(false),closed:AtomicBool::new(false),unknown:AtomicBool::new(false),idle_messages:Mutex::new(Vec::new()),wakes:Mutex::new(VecDeque::new()),active_wake:Mutex::new(None),wake:Arc::new(Notify::new()),close:Mutex::new(None),actor_stop:CancellationToken::new(),peers});
    attachment.commit();
    ROOTS.get_or_init(Default::default).lock().unwrap_or_else(|e|e.into_inner()).push(host.clone());
    *sink.console_task.lock().unwrap_or_else(|e|e.into_inner())=Some(host.clone());
    host.bind_uri(cwd,owner,file);
    let actor=host.clone();tokio::spawn(async move{actor.serve(task_rx,peer_rx).await;});Ok(Some(host))
}
impl ConsoleTaskHost {
    fn native(&self)->Arc<native_setup::NativeCliOwner>{self.native.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    pub fn ingress(&self)->Arc<peer_host::PeerIngress>{self.native().peer_ingress.clone()}
    fn bind_uri(&self,cwd:&Path,owner:&CliToolOwner,file:Option<PathBuf>){
        if let(Some(context),Some(sink))=(&owner.context,self.sink.upgrade()){
            let cwd=cwd.to_owned();let eligibility=Arc::new(move||rpc_host_settings::peer_messaging_enabled(&cwd,&ara_home().join("agent"),0,true).map_err(|error|error.to_string()));
            let peer=self.peers.uri(&self.runtime.peer.root,&self.runtime.peer.id,file,eligibility);
            context.set_uri_port(ara_cli::host_vault::compose_ports(sink.artifacts.clone(),Some(peer)));
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn refresh(&self,args:&Args,cwd:&Path,dir:Option<PathBuf>,provider:&ProviderFactory,owner:&CliToolOwner,jobs:native_setup::NativeSessionJobs,mcp:&Option<McpServerConfig>)->Result<()>{
        anyhow::ensure!(!self.closed.load(Ordering::Acquire),"Console Task root is closed");
        let next=owner.native.clone().context("Console Task refresh lacks Native owner")?;let previous=self.native();
        anyhow::ensure!(Arc::ptr_eq(previous.prepared.values(),next.prepared.values()),"Console Task refresh changed original logical Session values");
        let sink=self.sink.upgrade().context("Console Task sink disposed")?;let guard=sink.journal.lock().await;let journal=guard.as_ref().context("Console Task journal missing")?;
        anyhow::ensure!(journal.session_id()==self.runtime.peer.id,"Console Task refresh journal identity changed");
        self.peers.bind_native(&next,&self.runtime,journal.is_persistent())?;task_host::bind(&self.dispatch,&next,journal.session_id(),cwd,owner.edit_session.clone())?;
        let file=journal.is_persistent().then(||journal.path().to_owned());drop(guard);
        *self.template.lock().await=factory(args,cwd,dir,provider,owner,jobs,mcp,self.peers.clone(),self.runtime.peer.root.clone());
        *self.native.lock().unwrap_or_else(|e|e.into_inner())=next;*self.plan.lock().unwrap_or_else(|e|e.into_inner())=owner.edit_session.clone();self.bind_uri(cwd,owner,file);Ok(())
    }
    async fn serve(self:Arc<Self>,mut tasks:mpsc::UnboundedReceiver<task_host::TaskEnvelope>,mut peers:mpsc::UnboundedReceiver<peer_host::PeerActorRequest>){
        loop {tokio::select!{
            Some(envelope)=tasks.recv()=>{
                let prepared:Result<_>=async{
                    anyhow::ensure!(!self.closed.load(Ordering::Acquire)&&envelope.session_id==self.runtime.peer.id,"Console Task exact Session admission closed or replaced");
                    let sink=self.sink.upgrade().context("Console Task sink disposed before admission")?;
                    let template=self.template.lock().await;let journal=sink.journal.lock().await;let journal=journal.as_ref().context("Console Task journal missing")?;
                    anyhow::ensure!(journal.session_id()==envelope.session_id,"Console Task original journal changed");
                    let mut policy=task_host::host_policy(rpc_host_settings::TaskSettings::load(&template.cwd,&ara_home().join("agent"))?)?;
                    use ara_tools::edit_host::EditHostPort;
                    policy.plan_mode=template.tool_owner.edit_session.as_ref().is_some_and(|plan|plan.path_policy().plan_active);
                    task_host::prepare(&template,journal,envelope.request,policy,&envelope.cancel).await
                }.await;
                match prepared{Ok(prepared)=>task_host::launch(prepared,self.peers.clone(),self.dispatch.clone(),envelope.cancel,envelope.reply),Err(error)=>{let _=envelope.reply.send(Err(error));}}
            },
            Some(request)=peers.recv()=>self.peer_request(request).await,
            _=self.actor_stop.cancelled()=>break,
            else=>break,
        }}
    }
    fn state(&self)->NativePeerState{
        use ara_tools::edit_host::EditHostPort;
        let native=self.native();let child=native.child_contract.as_ref();let wake=self.active_wake.lock().unwrap_or_else(|e|e.into_inner()).is_some();
        NativePeerState{running:self.running.load(Ordering::Acquire),disposed:self.closed.load(Ordering::Acquire),plan_mode:self.plan.lock().unwrap_or_else(|e|e.into_inner()).as_ref().is_some_and(|plan|plan.path_policy().plan_active),dispatch_active:wake||child.is_some_and(|child|child.dispatch_active()),pending_publication:child.is_some_and(|child|child.desired_snapshot().generation!=child.published_snapshot().generation||child.publication_error().is_some()),reply_obligation:wake,owned_jobs:native.bash_session.effects().pending_count()!=0||native.bash_session.snapshots().iter().any(|job|job.status=="running"||job.result.is_none())}
    }
    async fn existing_receipt(&self,completion:&ara_cli::host_peers::WakeCompletion)->Result<Option<ParentAdoption>>{
        let sink=self.sink.upgrade().context("Console parent sink disposed; adoption unknown")?;let mut guard=sink.journal.lock().await;let journal=guard.as_mut().context("Console parent journal missing")?;
        anyhow::ensure!(completion.job.parent_id==journal.session_id()&&completion.job.child.root==self.runtime.peer.root,"Console parent result belongs to another exact Session/root");
        let key=serde_json::to_value(&completion.job)?;let runtime=serde_json::to_value(&completion.runtime)?;
        let entry=journal.entries().iter().find(|entry|entry.observed_message_field(&["details","peerWakeJob"]).ok().flatten().and_then(|body|ara_session::native_checked_serde(&body).ok()).as_ref()==Some(&key)&&entry.observed_message_field(&["details","peerWakeRuntime"]).ok().flatten().and_then(|body|ara_session::native_checked_serde(&body).ok()).as_ref()==Some(&runtime)).map(|entry|entry.id.clone());
        if let Some(entry_id)=entry{journal.materialize()?;return Ok(Some(ParentAdoption{session_id:journal.session_id().into(),entry_id,persistent:journal.is_persistent()&&journal.is_on_disk()}));}Ok(None)
    }
    async fn append_idle(&self,body:serde_json::Value)->Result<ParentAdoption>{
        let sink=self.sink.upgrade().context("Console sink disposed before idle append; effect unknown")?;
        let message=sink.projector.new_custom_input(body)?;
        let receipt={let mut guard=sink.journal.lock().await;let journal=guard.as_mut().context("Console journal missing")?;anyhow::ensure!(journal.session_id()==self.runtime.peer.id,"Console journal changed before idle append");let entry_id=sink.projector.append_current_body(journal,&message)?;journal.materialize()?;ParentAdoption{session_id:journal.session_id().into(),entry_id,persistent:journal.is_persistent()&&journal.is_on_disk()}};
        self.idle_messages.lock().unwrap_or_else(|e|e.into_inner()).push(message);self.wake.notify_waiters();Ok(receipt)
    }
    async fn parent_result(&self,pending:peer_host::PendingParentResult){
        let result=self.existing_receipt(&pending.completion).await;
        match result{Ok(Some(receipt))=>{let _=pending.reply.send(Ok(peer_host::PeerActorReply::Adoption(receipt)));return;},Err(error)=>{let _=pending.reply.send(Err(error));return;},_=>{}}
        let _gate=self.admission.lock().await;
        if self.running.load(Ordering::Acquire){self.native().peer_ingress.enqueue_parent(pending);return;}
        let result=self.append_idle(peer_host::parent_result_body(&pending.completion)).await.map(peer_host::PeerActorReply::Adoption);let _=pending.reply.send(result);
    }
    async fn peer_request(&self,request:peer_host::PeerActorRequest){
        use peer_host::{PeerActorOp as Op,PeerActorReply as Reply};
        if request.runtime!=self.runtime{let _=request.reply.send(Err(anyhow::anyhow!("Console Peer exact runtime was replaced")));return;}
        let operation=match request.operation{Op::ParentResult(completion)=>{self.parent_result(peer_host::PendingParentResult{completion,reply:request.reply}).await;return;},Op::Wake(wake,cancel)=>{
            if self.closed.load(Ordering::Acquire)||cancel.is_cancelled(){let _=request.reply.send(Err(anyhow::anyhow!("Console wake cancelled before admission")));return;}
            self.wakes.lock().unwrap_or_else(|e|e.into_inner()).push_back(PendingWake{request:wake,cancel,reply:request.reply});self.wake.notify_waiters();return;
        },operation=>operation};
        let result:Result<Reply>=async{match operation{
            Op::Inspect=>Ok(Reply::State(self.state())),
            Op::RelayObservation(message)=>{anyhow::ensure!(self.peers.coordinator.relay_main_is_current(&self.runtime)&&message.from!=self.runtime.peer.id&&message.to!=self.runtime.peer.id,"Console relay Main/root generation changed");if let Some(sink)=self.sink.upgrade(){if sink.mode==Mode::Json{sink.write_line(&serde_json::json!({"type":"irc_message","message":ara_cli::host_peers::main_relay_record(&message)}).to_string());}else{eprintln!("{}",sanitize_text(&format!("[IRC {} → {}] {}",message.from,message.to,message.body)));}}Ok(Reply::Delivered)},
            Op::Deliver(delivery)=>{
                anyhow::ensure!(delivery.message.to==self.runtime.peer.id,"Console delivery recipient changed");let _gate=self.admission.lock().await;let state=self.state();
                match delivery.route{
                    ara_cli::host_peers::DeliveryRoute::ParentSteer=>bail!("Console Main has no parent; ParentSteer is not an admitted route"),
                    ara_cli::host_peers::DeliveryRoute::SiblingInterrupt=>{anyhow::ensure!(state.running||state.dispatch_active,"Console interrupt recipient became idle");self.peers.queues.interrupt(&self.runtime,delivery);},
                    ara_cli::host_peers::DeliveryRoute::PlanAppend=>{anyhow::ensure!(!state.running&&state.plan_mode,"Console plan append recipient changed mode");self.append_idle(serde_json::json!({"role":"custom","customType":"irc","content":delivery.rendered,"display":true,"attribution":"agent","timestamp":delivery.message.ts,"details":ara_cli::host_peers::native::incoming_details(&delivery.message,false,false,false)})).await?;},
                }Ok(Reply::Delivered)
            },
            Op::Revive(record)=>{let sink=self.sink.upgrade().context("Console revive parent disposed")?;let template=self.template.lock().await;let journal=sink.journal.lock().await;let factory=child_host::ChildHostFactory::new(&template,journal.as_ref().context("Console revive journal missing")?)?;let child=match self.peers.task_resource(&record.key){Some(lease)=>factory.open_warm_isolated(record.clone(),lease,&sink.cancel).await?,None=>factory.open_peer(record.clone(),&sink.cancel).await?};Ok(Reply::Runtime(self.peers.attach_child_in_root(&record.key.root,child)?))},
            Op::Cancel(turn)=>{if let Some(wake)=self.active_wake.lock().unwrap_or_else(|e|e.into_inner()).as_ref().filter(|wake|wake.request.job.turn==turn){wake.cancel.cancel();}for wake in self.wakes.lock().unwrap_or_else(|e|e.into_inner()).iter().filter(|wake|wake.request.job.turn==turn){wake.cancel.cancel();}Ok(Reply::Delivered)},
            Op::Close=>{if let Some(sink)=self.sink.upgrade(){sink.cancel.cancel();}self.unknown.store(true,Ordering::Release);bail!("Console Main close requested without the existing Agent loop settlement receipt")},
            #[cfg(feature="test-fixture")]Op::FixturePlan(_)|Op::FixtureUriSchemes(_)|Op::FixtureRefreshHostTools|Op::FixtureNewSession(_)=>bail!("Console Host does not expose RPC fixture operations"),
            Op::Wake(..)|Op::ParentResult(..)=>unreachable!(),
        }}.await;let _=request.reply.send(result);
    }
    pub fn has_idle_input(&self)->bool{!self.wakes.lock().unwrap_or_else(|e|e.into_inner()).is_empty()}
    pub fn has_context_updates(&self)->bool{!self.idle_messages.lock().unwrap_or_else(|e|e.into_inner()).is_empty()}
    pub fn apply_idle_context(&self,context:&mut Vec<AgentMessage>){context.extend(std::mem::take(&mut *self.idle_messages.lock().unwrap_or_else(|e|e.into_inner())));}
    pub fn idle_input(&self)->Result<Option<AgentMessage>>{
        if self.active_wake.lock().unwrap_or_else(|e|e.into_inner()).is_some(){return Ok(None);}
        loop{let Some(pending)=self.wakes.lock().unwrap_or_else(|e|e.into_inner()).pop_front()else{return Ok(None)};
            if pending.cancel.is_cancelled(){let _=pending.reply.send(Err(anyhow::anyhow!("Console wake cancelled before original Native turn")));continue;}
            let sink=self.sink.upgrade().context("Console wake sink disposed")?;let body=serde_json::json!({"role":"custom","customType":"peer-wake","content":pending.request.rendered.join("\n\n"),"display":true,"attribution":"agent","timestamp":ara_cli::host_peers::now_ms(),"details":ara_cli::host_peers::native::incoming_batch_details(&pending.request.messages,None,false)});
            let message=sink.projector.new_custom_input(body)?;*self.active_wake.lock().unwrap_or_else(|e|e.into_inner())=Some(pending);return Ok(Some(message));}
    }
    pub async fn begin_run(self:&Arc<Self>,context:&mut Vec<AgentMessage>,cancel:&CancellationToken)->Result<ConsoleRun>{
        let _gate=self.admission.lock().await;anyhow::ensure!(!self.closed.load(Ordering::Acquire)&&!self.unknown.load(Ordering::Acquire),"Console Task root is closed or prior Main settlement is unknown");
        self.apply_idle_context(context);anyhow::ensure!(!self.running.swap(true,Ordering::AcqRel),"Console Main already has a writer");
        self.peers.coordinator.registry.set_status(&self.runtime.peer,PeerStatus::Running);
        let relay=CancellationToken::new();
        if let Some(wake)=self.active_wake.lock().unwrap_or_else(|e|e.into_inner()).as_ref(){let source=wake.cancel.clone();let target=cancel.clone();let done=relay.clone();tokio::spawn(async move{tokio::select!{_=source.cancelled()=>target.cancel(),_=done.cancelled()=>{}}});}
        Ok(ConsoleRun{owner:self.clone(),finished:false,relay})
    }
    async fn end_run(&self,report:&Result<ara_agent::RunReport>)->Result<()>{
        let _gate=self.admission.lock().await;self.running.store(false,Ordering::Release);self.peers.coordinator.registry.set_status(&self.runtime.peer,PeerStatus::Idle);
        if let Some(pending)=self.active_wake.lock().unwrap_or_else(|e|e.into_inner()).take(){
            let outcome=match report{Ok(report)=>{let answer=last_assistant(&report.messages,&self.sink.upgrade().context("Console wake sink disposed")?.projector)?.map(|message|message.content.iter().filter_map(|block|match block{ara_cli::actual_host::AssistantDisplayBlock::Text(text)=>Some(text.as_str()),_=>None}).collect::<Vec<_>>().join("\n"));WakeOutcome{accepted:true,response_at:Some(ara_cli::host_peers::now_ms()),answer,cancelled:pending.cancel.is_cancelled()||report.end==RunEnd::Aborted,error:(!matches!(report.end,RunEnd::Completed|RunEnd::TerminalToolResult)).then(||format!("Console Main ended {:?}",report.end)),..Default::default()}},Err(error)=>WakeOutcome::failed(format!("Console Main failed: {error:#}"))};let _=pending.reply.send(Ok(peer_host::PeerActorReply::Wake(outcome)));
        }
        let ingress=self.native().peer_ingress.clone();ingress.release_unappended();
        while let Some(pending)=ingress.take_parent(){let result=match self.existing_receipt(&pending.completion).await?{Some(receipt)=>Ok(receipt),None=>self.append_idle(peer_host::parent_result_body(&pending.completion)).await};let _=pending.reply.send(result.map(peer_host::PeerActorReply::Adoption));}
        self.wake.notify_waiters();Ok(())
    }
    pub async fn close(self:&Arc<Self>)->Result<()>{
        let(mut done,launch)={let mut state=self.close.lock().unwrap_or_else(|e|e.into_inner());match &*state{Some(done)=>(done.clone(),None),None=>{let(tx,rx)=watch::channel(None);*state=Some(rx.clone());(rx,Some(tx))}}};
        if let Some(finished)=launch{let owner=self.clone();tokio::spawn(async move{
            owner.closed.store(true,Ordering::Release);owner.native().task_session.close();
            for pending in owner.wakes.lock().unwrap_or_else(|e|e.into_inner()).drain(..){pending.cancel.cancel();let _=pending.reply.send(Err(anyhow::anyhow!("Console Session closed before wake admission")));}
            let result:Result<()>=async{
                // Join an already-admitted preparation before sealing the root
                // inventory. Drop this gate before external cleanup begins.
                let prepared=owner.template.lock().await;drop(prepared);
                anyhow::ensure!(!owner.running.load(Ordering::Acquire)&&!owner.unknown.load(Ordering::Acquire),"Console Main writer has not supplied a known stopped receipt; Task root retained");
                if let Some(pending)=owner.active_wake.lock().unwrap_or_else(|e|e.into_inner()).take(){pending.cancel.cancel();let _=pending.reply.send(Err(anyhow::anyhow!("Console wake input was prepared but no Native turn was admitted before Session close")));}
                let receipt=owner.peers.release_root(&owner.runtime).await?;
                anyhow::ensure!(receipt.settled,"Console Task root release incomplete: {}",serde_json::to_string(&receipt)?);owner.peers.unbind(&owner.runtime);owner.actor_stop.cancel();Ok(())}.await;
            let _=finished.send(Some(result.map_err(|error|format!("{error:#}"))));
        });}
        loop{if let Some(result)=done.borrow().clone(){return result.map_err(anyhow::Error::msg);}done.changed().await.context("Console close observer lost retained settlement receipt")?;}
    }
}
pub(super) struct ConsoleRun{owner:Arc<ConsoleTaskHost>,finished:bool,relay:CancellationToken}
impl ConsoleRun{pub async fn finish(mut self,report:&Result<ara_agent::RunReport>)->Result<()>{self.relay.cancel();self.owner.end_run(report).await?;self.finished=true;Ok(())}}
impl Drop for ConsoleRun{fn drop(&mut self){self.relay.cancel();if !self.finished{self.owner.unknown.store(true,Ordering::Release);self.owner.running.store(false,Ordering::Release);self.owner.native().peer_ingress.release_unappended();}}}
pub(super) async fn close_all()->Result<()>{let owners=ROOTS.get_or_init(Default::default).lock().unwrap_or_else(|e|e.into_inner()).clone();let mut errors=Vec::new();for owner in owners{if let Err(error)=owner.close().await{errors.push(format!("{}: {error:#}",owner.runtime.peer.id));}}if !errors.is_empty(){bail!("Console Task settlement: {}",errors.join("; "));}Ok(())}
