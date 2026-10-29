//! Reference Host service-control point, OMP b91db66e launch/services.ts,
//! launch/client.ts, session/{yield-queue,launch-completion}.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
pub mod broker;
pub mod client;
mod process;
pub mod protocol;
mod store;
#[cfg(test)] mod tests;
pub use client::{ProjectServiceClient,ServiceError};
pub use protocol::{Completion,ServiceMode,ServiceSnapshot,ServiceSpec,ServiceState};
pub use store::ProjectScope;

use crate::{agent_message_projection::AgentMessageProjector,native_factory::PreparedNativeSession};
use ara_agent::{AgentMessage,AgentMessageIdentity,AsideCallbacks,AsideError,AsideMessage};
use futures::{stream::FuturesUnordered,StreamExt};
use serde_json::json;
use std::{collections::{HashMap,HashSet,VecDeque},future::Future,path::Path,pin::Pin,sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},time::Duration};
use tokio::sync::{Notify,oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone,Copy,Debug)] pub struct ServicePolicy {pub launch_enabled:bool}
/// Reserved name syntax from the fixed shared browser producer, not an ACL.
pub fn shared_browser_name(name:&str)->bool {matches!(name,"ara.browser.headless"|"ara.browser.headed"|"omp.browser.headless"|"omp.browser.headed")}

#[derive(Clone,Debug)]
pub struct CompletionAdoption {pub session_id:String,pub entry_id:String,pub persistent:bool}
type Settler=oneshot::Sender<Result<CompletionAdoption,String>>;
struct Pending {notification:Completion,settler:Settler}
struct Claim {entries:Vec<Pending>,context_claimed:bool}
#[derive(Clone)]struct Binding {id:String,persistent:bool}
#[derive(Default)]struct State {
    binding:Option<Binding>,queue:VecDeque<Pending>,claims:HashMap<AgentMessageIdentity,Claim>,
    tracked:HashMap<String,(String,u64)>,
}
/// One logical Session's pending receipts. No service/process truth lives here.
pub struct SessionServiceLease {
    client:Arc<ProjectServiceClient>,state:Mutex<State>,wake:Arc<Notify>,stop:CancellationToken,
    closed:AtomicBool,subscription:String,launch_enabled:bool,
}
pub struct ServiceBindingPreparation<'a>{owner:&'a Arc<SessionServiceLease>,state:std::sync::MutexGuard<'a,State>,id:String,persistent:bool}
impl ServiceBindingPreparation<'_>{pub fn commit(mut self){
    if self.state.binding.is_some(){return;}
    self.state.binding=Some(Binding{id:self.id.clone(),persistent:self.persistent});
    drop(self.state);self.owner.start_subscription(&self.id);
}}
impl SessionServiceLease {
    pub fn new(project:&Path,runtime_root:&Path,executable:std::path::PathBuf,policy:ServicePolicy)->anyhow::Result<Arc<Self>>{
        let client=ProjectServiceClient::new(project,runtime_root,executable,policy)?;
        Ok(Arc::new(Self{client,state:Mutex::new(State::default()),wake:Arc::new(Notify::new()),stop:CancellationToken::new(),closed:AtomicBool::new(false),subscription:uuid::Uuid::new_v4().to_string(),launch_enabled:policy.launch_enabled}))
    }
    pub fn launch_enabled(&self)->bool{self.launch_enabled}
    /// Broker failure is unknown, not an empty successful ownership refresh.
    pub async fn refresh_owned_tolerant(&self,cancel:&CancellationToken)->Result<Option<String>,ServiceError>{
        let (services,error)=self.client.list_tolerant(cancel).await?;
        if error.is_none(){self.state.lock().unwrap().tracked.clear();for snapshot in &services{self.track(snapshot);}}
        Ok(error)
    }
    pub fn client(&self)->&Arc<ProjectServiceClient>{&self.client}
    /// Called only after the Host has published its actual journal/native owner.
    /// The literal legacy 'ephemeral' ID is refused rather than shared silently.
    pub fn prepare_binding<'a>(self:&'a Arc<Self>,id:&str,persistent:bool)->anyhow::Result<ServiceBindingPreparation<'a>> {
        if id.is_empty()||(!persistent&&id=="ephemeral"){anyhow::bail!("Service completion requires a unique logical Session identity");}
        if self.closed.load(Ordering::Acquire){anyhow::bail!("Service Session is disposed");}
        let state=self.state.lock().unwrap();
        if let Some(binding)=&state.binding {anyhow::ensure!(binding.id==id&&binding.persistent==persistent,"Service Session identity cannot be rebound");}
        Ok(ServiceBindingPreparation{owner:self,state,id:id.to_owned(),persistent})
    }
    pub fn bind(self:&Arc<Self>,id:&str,persistent:bool)->anyhow::Result<()> {
        self.prepare_binding(id,persistent)?.commit();Ok(())
    }
    fn start_subscription(self:&Arc<Self>,id:&str) {
        let lease=Arc::downgrade(self);let client=self.client.clone();let owner=id.to_owned();let subscription=self.subscription.clone();let stop=self.stop.clone();
        tokio::spawn(async move{
            type PendingReceipt=Pin<Box<dyn Future<Output=(String,Result<Result<CompletionAdoption,String>,oneshot::error::RecvError>)>+Send>>;
            let mut pending=FuturesUnordered::<PendingReceipt>::new();let mut inflight=HashSet::new();let mut seen=VecDeque::<String>::new();let mut subscribed=false;
            let mut tick=tokio::time::interval(Duration::from_millis(100));
            loop {tokio::select!{
                _=stop.cancelled()=>break,
                receipt=pending.next(),if !pending.is_empty()=>{
                    if let Some((id,result))=receipt {
                        inflight.remove(&id);
                        if matches!(result,Ok(Ok(_))) {
                            if seen.len()>=512{seen.pop_front();}seen.push_back(id.clone());
                            // ACK is separate from the append receipt. If lost,
                            // replay in this client's bounded seen window reACKs.
                            let _=client.request(protocol::Operation::Ack{owner:owner.clone(),subscription:subscription.clone(),completion_ids:vec![id]},&stop).await;
                        }else{subscribed=false;}
                    }
                },
                _=tick.tick()=>{
                    if lease.upgrade().is_none(){break;}
                    if !subscribed {
                        match client.request(protocol::Operation::Subscribe{owner:owner.clone(),subscription:subscription.clone()},&stop).await{Ok(_)=>subscribed=true,Err(_)=>continue}
                    }
                    let result=client.request(protocol::Operation::Poll{owner:owner.clone(),subscription:subscription.clone()},&stop).await;
                    let Ok(protocol::Reply::Completions{entries})=result else{subscribed=false;continue;};
                    for notification in entries {
                        if seen.contains(&notification.completion_id){let _=client.request(protocol::Operation::Ack{owner:owner.clone(),subscription:subscription.clone(),completion_ids:vec![notification.completion_id]},&stop).await;continue;}
                        if inflight.contains(&notification.completion_id){continue;}
                        let Some(lease)=lease.upgrade() else{break;};
                        let id=notification.completion_id.clone();let(receiver,accepted)={
                            let(tx,rx)=oneshot::channel();let mut state=lease.state.lock().unwrap();
                            let accepted=!lease.closed.load(Ordering::Acquire)&&inflight.len()<1024&&state.binding.as_ref().is_some_and(|b|b.id==notification.owner);
                            if accepted{state.queue.push_back(Pending{notification,settler:tx});}else{let _=tx.send(Err("Service completion owner disposed or queue full".into()));}
                            (rx,accepted)
                        };
                        if accepted {inflight.insert(id.clone());pending.push(Box::pin(async move{(id,receiver.await)}));lease.wake.notify_one();}
                    }
                },
            }}
            let close=CancellationToken::new();let _=tokio::time::timeout(Duration::from_secs(2),client.request(protocol::Operation::Unsubscribe{owner,subscription,preserve_pending:true},&close)).await;
        });
    }
    pub fn wake(&self)->Arc<Notify>{self.wake.clone()}
    pub fn has_pending(&self)->bool{!self.state.lock().unwrap().queue.is_empty()}
    pub fn track(&self,snapshot:&ServiceSnapshot){
        let mut state=self.state.lock().unwrap();if !state.binding.as_ref().is_some_and(|b|snapshot.owner.as_ref()==Some(&b.id)){return;}
        if snapshot.state.terminal(){if state.tracked.get(&snapshot.name).is_some_and(|(id,started)|id==&snapshot.id&&*started==snapshot.started_at){state.tracked.remove(&snapshot.name);}}else{state.tracked.insert(snapshot.name.clone(),(snapshot.id.clone(),snapshot.started_at));}
    }
    pub fn has_live_owned_service(&self)->bool{!self.state.lock().unwrap().tracked.is_empty()}
    fn owner(&self)->Result<String,ServiceError>{if self.closed.load(Ordering::Acquire){return Err(ServiceError::NotDispatched{request_id:String::new(),message:"Service Session is disposed".into()});}self.state.lock().unwrap().binding.as_ref().map(|b|b.id.clone()).ok_or_else(||ServiceError::NotDispatched{request_id:String::new(),message:"Service producer must bind the actual logical Session first".into()})}
    pub async fn start(&self,spec:ServiceSpec,replace:bool,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{let snapshot=self.client.start(spec,Some(self.owner()?),replace,cancel).await?;self.track(&snapshot);Ok(snapshot)}
    pub async fn send(&self,name:&str,content:&str,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{let snapshot=self.client.send(name,content,cancel).await?;self.track(&snapshot);Ok(snapshot)}
    pub async fn mode(&self,name:&str,mode:ServiceMode,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{let snapshot=self.client.mode(name,mode,cancel).await?;self.track(&snapshot);Ok(snapshot)}
    pub async fn stop_service(&self,name:&str,cancel:&CancellationToken)->Result<ServiceSnapshot,ServiceError>{let snapshot=self.client.stop(name,cancel).await?;self.track(&snapshot);Ok(snapshot)}
    pub async fn refresh_owned(&self,cancel:&CancellationToken)->Result<Vec<ServiceSnapshot>,ServiceError>{let services=self.client.list(cancel).await?;self.state.lock().unwrap().tracked.clear();for snapshot in &services{self.track(snapshot);}Ok(services)}

    pub fn asides(self:&Arc<Self>,projector:Arc<AgentMessageProjector>,prepared:Option<Arc<PreparedNativeSession>>)->Vec<AsideMessage>{
        let entries={let mut state=self.state.lock().unwrap();if state.queue.is_empty(){return Vec::new();}state.queue.drain(..).collect::<Vec<_>>()};
        let held=Arc::new(Mutex::new(Some(entries)));let weak=Arc::downgrade(self);
        vec![AsideMessage::Lazy(Arc::new(move||{
            let Some(entries)=held.lock().unwrap().take() else{return Ok(None);};
            let Some(lease)=weak.upgrade() else {for e in entries{let _=e.settler.send(Err("Service Session disposed".into()));}return Ok(None);};
            let binding=lease.state.lock().unwrap().binding.clone();
            if lease.closed.load(Ordering::Acquire)||binding.as_ref().is_none_or(|b|entries.iter().any(|e|e.notification.owner!=b.id)){
                for e in entries{let _=e.settler.send(Err("Service completion became stale".into()));}return Ok(None);
            }
            let notifications=entries.iter().map(|e|e.notification.clone()).collect::<Vec<_>>();
            let message=match build_completion(&notifications,&projector,prepared.as_deref()){
                Ok(message)=>message,Err(error)=>{for e in entries{let _=e.settler.send(Err(error.to_string()));}return Err(error);}
            };
            let identity=message.weak_identity();let commit_id=identity.clone();let discard_id=identity.clone();
            lease.state.lock().unwrap().claims.insert(identity,Claim{entries,context_claimed:false});
            let commit=Arc::downgrade(&lease);let discard=commit.clone();
            message.set_aside_callbacks(AsideCallbacks{
                commit:Some(Arc::new(move||{if let Some(lease)=commit.upgrade(){if let Some(claim)=lease.state.lock().unwrap().claims.get_mut(&commit_id){claim.context_claimed=true;}}Ok(())})),
                discard:Some(Arc::new(move|error|{if let Some(lease)=discard.upgrade(){lease.reject_identity(&discard_id,&error.to_string());}Ok(())})),
            });Ok(Some(message))
        }))]
    }
    pub fn idle_message(self:&Arc<Self>,projector:Arc<AgentMessageProjector>,prepared:Option<Arc<PreparedNativeSession>>)->Result<Option<AgentMessage>,AsideError>{
        match self.asides(projector,prepared).into_iter().next(){Some(AsideMessage::Lazy(f))=>f(),Some(AsideMessage::Ready(m))=>Ok(Some(m)),None=>Ok(None)}
    }
    /// Only this lease's exact already-claimed pending weak message can ask
    /// the Host to cross the lazy journal's materialization boundary. A public
    /// customType or an unrelated equal-looking body is never authority.
    pub fn requires_durable_append(&self,message:&AgentMessage)->bool{
        let identity=message.weak_identity();let state=self.state.lock().unwrap();
        !self.closed.load(Ordering::Acquire)&&state.binding.as_ref().is_some_and(|binding|{
            binding.persistent&&state.claims.get(&identity).is_some_and(|claim|{
                claim.context_claimed&&!claim.entries.is_empty()
                    &&claim.entries.iter().all(|entry|entry.notification.owner==binding.id)
            })
        })
    }
    /// HostSink calls this after its existing append_current_body and, for an
    /// owned persistent completion, successful materialize, before display. This is an intentional persistence-first Host
    /// frontier; Core's earlier ephemeral claim callback never sends an ACK.
    pub fn append_receipt(&self,message:&AgentMessage,result:Result<CompletionAdoption,String>){
        let identity=message.weak_identity();let mut state=self.state.lock().unwrap();
        let Some(claim)=state.claims.remove(&identity) else{return;};
        let result=match result{
            Ok(receipt) if claim.context_claimed&&state.binding.as_ref().is_some_and(|b|b.id==receipt.session_id&&b.persistent==receipt.persistent)=>Ok(receipt),
            Ok(_)=>Err("Service append did not match its claimed logical Session".into()),Err(error)=>Err(error),
        };
        for entry in claim.entries {
            if result.is_ok(){
                if state.tracked.get(&entry.notification.daemon.name).is_some_and(|(id,started)|id==&entry.notification.daemon.id&&*started==entry.notification.daemon.started_at){state.tracked.remove(&entry.notification.daemon.name);}
            }
            let _=entry.settler.send(result.clone());
        }
    }
    fn reject_identity(&self,id:&AgentMessageIdentity,error:&str){if let Some(claim)=self.state.lock().unwrap().claims.remove(id){for entry in claim.entries{let _=entry.settler.send(Err(error.into()));}}}
    pub fn release_unappended(&self){
        let claims=std::mem::take(&mut self.state.lock().unwrap().claims);
        for(_,claim)in claims{for entry in claim.entries{let _=entry.settler.send(Err("Run ended before service completion journal adoption".into()));}}
        if self.has_pending(){self.wake.notify_one();}
    }
    pub fn close(&self){
        if self.closed.swap(true,Ordering::AcqRel){return;}
        self.stop.cancel();let(mut queue,claims)={let mut state=self.state.lock().unwrap();state.tracked.clear();(std::mem::take(&mut state.queue),std::mem::take(&mut state.claims))};
        for entry in queue.drain(..){let _=entry.settler.send(Err("Service Session disposed before adoption".into()));}
        for(_,claim)in claims{for entry in claim.entries{let _=entry.settler.send(Err("Service Session disposed before adoption".into()));}}
        self.client.close_presence();self.wake.notify_waiters();
    }
}
impl Drop for SessionServiceLease{fn drop(&mut self){self.close();}}
pub struct ServiceRunClaims(pub Option<Arc<SessionServiceLease>>);
impl Drop for ServiceRunClaims{fn drop(&mut self){if let Some(lease)=&self.0{lease.release_unappended();}}}

// Exact source template from prompts/session/launch-completion.md at b91db66e.
const TEMPLATE:&str="Supervised process {{name}} {{state}} {{#if hasExitCode}}with exit code {{exitCode}}{{else}}without an exit code{{/if}}.";
fn build_completion(entries:&[Completion],projector:&AgentMessageProjector,prepared:Option<&PreparedNativeSession>)->Result<AgentMessage,AsideError>{
    let fail=|e:&dyn std::fmt::Display|AsideError(format!("service completion: {e}"));
    let contexts=entries.iter().map(|entry|json!({"name":entry.daemon.name,"state":entry.daemon.state,"exitCode":entry.daemon.exit_code,"hasExitCode":entry.daemon.exit_code.is_some()})).collect::<Vec<_>>();
    let daemons=entries.iter().map(|e|e.daemon.clone()).collect::<Vec<_>>();
    if let Some(prepared)=prepared {
        let values=prepared.values();let heap=values.heap();let runtime=values.runtime();let prompt=prepared.prompt().prompt();
        let render=heap.get(prompt,&"render".into(),runtime).map_err(|e|fail(&e))?;
        let mut rows=Vec::new();for context in contexts{rows.push(heap.call(&render,prompt,&[heap.string(TEMPLATE.into()).map_err(|e|fail(&e))?,heap.import_serde(&context).map_err(|e|fail(&e))?],runtime).map_err(|e|fail(&e))?);}
        let array=heap.array(rows).map_err(|e|fail(&e))?;let join=heap.get(&array,&"join".into(),runtime).map_err(|e|fail(&e))?;
        let content=heap.call(&join,&array,&[heap.string("\n".into()).map_err(|e|fail(&e))?],runtime).map_err(|e|fail(&e))?;
        let body=heap.import_serde(&json!({"role":"custom","customType":"launch-completion","display":true,"attribution":"agent","details":{"daemons":daemons}})).map_err(|e|fail(&e))?;
        heap.define_own_data(&body,"content".into(),content,runtime).map_err(|e|fail(&e))?;
        let date=prepared.prompt().date();let now=heap.get(date,&"now".into(),runtime).map_err(|e|fail(&e))?;
        let timestamp=heap.call(&now,date,&[],runtime).map_err(|e|fail(&e))?;heap.define_own_data(&body,"timestamp".into(),timestamp,runtime).map_err(|e|fail(&e))?;
        AgentMessage::from_graph(body,Some(Arc::new(json!({"origin":"launch-completion","attribution":"agent"})))).map_err(|e|fail(&e))
    }else{
        let rows=contexts.iter().map(|context|ara_prompt::render(TEMPLATE,context).map_err(|e|fail(&e))).collect::<Result<Vec<_>,_>>()?;
        projector.new_custom_input(json!({"role":"custom","customType":"launch-completion","content":rows.join("\n"),"display":true,"attribution":"agent","details":{"daemons":daemons},"timestamp":chrono::Utc::now().timestamp_millis()})).map_err(|e|fail(&e))
    }
}
