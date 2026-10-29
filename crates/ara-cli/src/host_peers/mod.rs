//! Finite reference-Host peer coordination, OMP b91db66e (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Full Peer, advisor/extension runtimes and Task scheduling remain OPEN.
pub mod agent_uri;
pub mod history_uri;
pub mod inbox;
pub mod lifecycle;
pub mod native;
pub mod registry;
pub mod roster;
pub mod wake;
#[cfg(any(test,feature="test-fixture"))] pub mod tests;

use async_trait::async_trait;
use serde::{Deserialize,Serialize};
use std::{path::PathBuf,sync::Arc,time::{SystemTime,UNIX_EPOCH}};
use tokio_util::sync::CancellationToken;
pub use inbox::{PeerInbox,PeerMessage};
pub use lifecycle::{PeerLifecycle,RootCloseReceipt,PeerCloseReceipt};
pub use native::{DeliveryRoute,NativeDelivery,PeerQueues,QueueSnapshot};
pub use registry::{AttachmentExpectation,PeerKey,PeerKind,PeerRecord,PeerRegistration,PeerRegistry,PeerStatus,RunMilestones,RuntimeKey};
pub use roster::PeerRoster;
pub use wake::{ParentAdoption,PeerWakeJobs,WakeCompletion,WakeJobId,WakeOutcome,WakeRequest,ParentConsumption,PeerJobSnapshot,PeerJobWatch,ForegroundReservation};
pub const DEFAULT_IDLE_TTL_MS:i64=420_000;
pub fn now_ms()->u64 {SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis().min(u64::MAX as u128) as u64}

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct NativePeerState {
 pub running:bool,pub plan_mode:bool,pub disposed:bool,pub dispatch_active:bool,
 pub pending_publication:bool,pub reply_obligation:bool,pub owned_jobs:bool,
}
impl NativePeerState {pub fn busy(&self)->bool {self.running||self.dispatch_active||self.pending_publication||self.reply_obligation||self.owned_jobs}}
/// The binary owns current factories/grants and all actual Agent/Journal effects.
/// Every method checks the exact native owner; a stable id alone is not authority.
#[async_trait]
pub trait PeerNativePort:Send+Sync {
 async fn inspect(&self,runtime:&RuntimeKey)->anyhow::Result<NativePeerState>;
 async fn deliver(&self,runtime:&RuntimeKey,delivery:NativeDelivery)->anyhow::Result<()>;
 /// Display only: synchronously enqueue once, never await the Main actor or writer.
 /// An absent observer is valid. Errors/panics cannot change the delivered effect.
 fn observe_main_relay(&self,_main:&RuntimeKey,_message:&PeerMessage)->anyhow::Result<()>{Ok(())}
 async fn revive(&self,record:&PeerRecord)->anyhow::Result<RuntimeKey>;
 async fn close(&self,runtime:&RuntimeKey)->anyhow::Result<()>;
 async fn can_revive(&self,record:&PeerRecord)->anyhow::Result<bool>{Ok(record.session_file.is_some())}
 /// Optional owned resource seam; default is no owned resource, not parity evidence.
 async fn release_owned(&self,_record:&PeerRecord)->anyhow::Result<()>{Ok(())}
 async fn cancel_turn(&self,runtime:&RuntimeKey,turn:u64)->anyhow::Result<()>;
 async fn drive_wake(&self,request:WakeRequest,cancel:CancellationToken)->anyhow::Result<WakeOutcome>;
 async fn transcript(&self,record:&PeerRecord)->anyhow::Result<Vec<serde_json::Value>>;
 /// Reconcile this exact job's existing durable receipt before any append retry.
 async fn persist_parent_result(&self,completion:&WakeCompletion)->anyhow::Result<ParentAdoption>;
}

#[derive(Clone,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum DeliveryOutcome {Injected,Woken,Revived,Failed}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct DeliveryReceipt {pub to:String,pub outcome:DeliveryOutcome,pub error:Option<String>}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct SendSummary {pub from:String,pub to:String,pub receipts:Vec<DeliveryReceipt>,pub is_error:bool,pub text:String}

pub struct PeerCoordinator {
 pub registry:Arc<PeerRegistry>,pub inbox:Arc<PeerInbox>,pub lifecycle:Arc<PeerLifecycle>,
 pub jobs:Arc<PeerWakeJobs>,pub roster:Arc<PeerRoster>,pub native:Arc<dyn PeerNativePort>,
}
impl PeerCoordinator {
 pub fn new(native:Arc<dyn PeerNativePort>)->Arc<Self>{
  let registry=Arc::new(PeerRegistry::default());let inbox=Arc::new(PeerInbox::default());
  let jobs=Arc::new(PeerWakeJobs::new(native.clone()));
  let lifecycle=PeerLifecycle::new(registry.clone(),native.clone(),jobs.clone());
  Arc::new(Self{roster:Arc::new(PeerRoster::new(registry.clone())),registry,inbox,lifecycle,jobs,native})
 }
 pub fn adopt(self:&Arc<Self>,input:PeerRegistration,ttl_ms:i64)->anyhow::Result<PeerRecord>{
  anyhow::ensure!(!self.registry.root_is_sealed(&input.root),"Peer root admission is closed");let record=self.registry.register(input);self.lifecycle.adopt(record.key.clone(),ttl_ms)?;Ok(record)
 }
 pub async fn send(self:&Arc<Self>,root:&str,from:&str,to:&str,body:&str,reply_to:Option<String>,wake_relay:bool)->anyhow::Result<SendSummary>{
  anyhow::ensure!(!to.trim().is_empty(),"A recipient is required.");
  anyhow::ensure!(!body.trim().is_empty(),"A non-empty message is required.");
  anyhow::ensure!(from!=to,"Cannot send a message to yourself.");
  let to=to.trim();let selected=if to=="all"{self.registry.visible(root,from)}else{Vec::new()};
  // Fixed messaging.ts: decide once from the same initial record snapshot,
  // even when the Main leg subsequently fails or its registration changes.
  let suppress_relay=to=="all"&&selected.iter().any(|r|r.kind==PeerKind::Main);
  let targets=if to=="all"{selected.into_iter().map(|r|r.key.id).collect::<Vec<_>>()}else{vec![to.to_owned()]};
  let mut receipts=Vec::new();
  // A per-recipient receipt survives partial broadcast failure; generation is re-read for every leg.
  for target in &targets {
   let message=PeerMessage{id:uuid::Uuid::new_v4().to_string(),from:from.into(),to:target.clone(),body:body.into(),ts:now_ms(),reply_to:reply_to.clone(),wake_relay};
   receipts.push(self.deliver_message_with_relay_policy(root,message,suppress_relay).await);
  }
  let delivered=receipts.iter().filter(|r|r.outcome!=DeliveryOutcome::Failed).count();
  let text=if to=="all" {if targets.is_empty(){"No live peers to broadcast to.".into()}else{format!("Broadcast delivered to {delivered} of {} peer(s).",targets.len())}}else if delivered>0{format!("Delivered to {to}.")}else{format!("Failed: {to}. {}",receipts.first().and_then(|r|r.error.as_deref()).unwrap_or("not running"))};
  Ok(SendSummary{from:from.into(),to:to.into(),is_error:!targets.is_empty()&&delivered==0,receipts,text})
 }
 /// Recheck the exact captured root-owned Main at both enqueue and consumption.
 pub fn relay_main_is_current(&self,main:&RuntimeKey)->bool{
  !self.registry.root_is_sealed(&main.peer.root)&&self.registry.get_exact(&main.peer).is_some_and(|r|r.kind==PeerKind::Main&&r.parent_id.is_none()&&r.status!=PeerStatus::Aborted&&r.runtime.as_ref()==Some(main))
 }
 fn relay_main_at_admission(&self,root:&str,message:&PeerMessage,suppress:bool)->Option<RuntimeKey>{
  if suppress||self.registry.root_is_sealed(root){return None;}
  self.registry.list(root).into_iter().find(|r|r.kind==PeerKind::Main&&r.parent_id.is_none())
   .and_then(|r|r.runtime).filter(|main|main.peer.id!=message.from&&main.peer.id!=message.to&&self.relay_main_is_current(main))
 }
 fn observe_delivered_relay(&self,main:Option<&RuntimeKey>,message:&PeerMessage){
  if let Some(main)=main.filter(|main|self.relay_main_is_current(main)){
   let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.native.observe_main_relay(main,message)));
  }
 }
 pub fn deliver_message<'a>(self:&'a Arc<Self>,root:&'a str,message:PeerMessage)->std::pin::Pin<Box<dyn std::future::Future<Output=DeliveryReceipt>+Send+'a>> {self.deliver_message_with_relay_policy(root,message,false)}
 fn deliver_message_with_relay_policy<'a>(self:&'a Arc<Self>,root:&'a str,message:PeerMessage,suppress_relay:bool)->std::pin::Pin<Box<dyn std::future::Future<Output=DeliveryReceipt>+Send+'a>> {Box::pin(async move {
  let failed=|error:String|DeliveryReceipt{to:message.to.clone(),outcome:DeliveryOutcome::Failed,error:Some(error)};
  if self.registry.root_is_sealed(root){return failed("Peer root admission is closed".into());}
  let Some(record)=self.registry.get(root,&message.to) else{return failed(format!("Unknown agent {}",message.to));};
  if record.status==PeerStatus::Aborted||record.kind==PeerKind::Advisor{return failed("Agent is terminal or an advisor transcript".into());}
  // Capture before the first awaited revival/inspection/handoff; never retarget to a successor.
  let relay_main=self.relay_main_at_admission(root,&message,suppress_relay);
  let old=record.runtime.clone();
  let runtime=match self.lifecycle.ensure_live(&record.key).await {Ok(r)=>r,Err(e)=>return failed(e.to_string())};
  let revived=old.as_ref()!=Some(&runtime);
  if self.inbox.deliver_waiter(root,&message){self.inbox.note_sent(root,&message);self.observe_delivered_relay(relay_main.as_ref(),&message);return DeliveryReceipt{to:message.to.clone(),outcome:if revived{DeliveryOutcome::Revived}else{DeliveryOutcome::Injected},error:None};}
  if !self.registry.owns_runtime(&runtime){return failed("Recipient generation changed before delivery".into());}
  let state=match self.native.inspect(&runtime).await {Ok(s)if !s.disposed=>s,Ok(_)=>return failed("Recipient disposed".into()),Err(e)=>return failed(e.to_string())};
  let actual=self.registry.get_exact(&record.key);
  let from_parent=actual.as_ref().and_then(|r|r.parent_id.as_deref())==Some(message.from.as_str());
  let result=if state.running||state.dispatch_active {
   self.native.deliver(&runtime,NativeDelivery{route:if from_parent{DeliveryRoute::ParentSteer}else{DeliveryRoute::SiblingInterrupt},rendered:native::render_incoming(&message,true,false,from_parent),message:message.clone()}).await.map(|_|DeliveryOutcome::Injected)
  }else if state.plan_mode {
   self.native.deliver(&runtime,NativeDelivery{route:DeliveryRoute::PlanAppend,rendered:native::render_incoming(&message,false,false,from_parent),message:message.clone()}).await.map(|_|DeliveryOutcome::Injected)
  }else{self.start_wake(runtime.clone(),vec![message.clone()]).map(|_|DeliveryOutcome::Woken)};
  match result {Ok(outcome)=>{self.inbox.note_sent(root,&message);self.observe_delivered_relay(relay_main.as_ref(),&message);DeliveryReceipt{to:message.to.clone(),outcome:if revived{DeliveryOutcome::Revived}else{outcome},error:None}},Err(error)=>{self.inbox.enqueue(root,message.clone());failed(error.to_string())}}
 })}
 pub async fn cancel(&self,root:&str,caller:&str,target:&str)->anyhow::Result<bool>{
  let Some(record)=self.registry.get(root,target)else{return Ok(false);};
  anyhow::ensure!(caller==target||record.parent_id.as_deref()==Some(caller),"Only the agent itself or its direct parent can cancel it");
  self.lifecycle.release(&record.key,true).await
 }
 /// Reserved finite host hook for an already-created child; native owner remains authoritative.
 pub fn attached(&self,key:&PeerKey,runtime:RuntimeKey)->anyhow::Result<()> {anyhow::ensure!(runtime.peer==*key&&self.registry.owns_runtime(&runtime)&&!self.registry.root_is_sealed(&key.root),"Peer attachment was not atomically published");Ok(())}
}

/// Fixed bus.ts/irc-bridge.ts observation shape. Never a journal/projector input.
pub fn main_relay_record(message:&PeerMessage)->serde_json::Value{
 serde_json::json!({"role":"custom","customType":"irc:relay","content":format!("[IRC `{}` → `{}`]\n\n{}",message.from,message.to,message.body),"display":true,"details":{"from":message.from,"to":message.to,"body":message.body},"attribution":"agent","timestamp":message.ts})
}
