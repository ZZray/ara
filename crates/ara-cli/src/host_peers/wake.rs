//! Fixed executor.ts3010-3385: one autonomous turn, exact observation and reply/job ownership.
use super::{PeerCoordinator,PeerMessage,PeerRecord,PeerStatus,PeerKey,PeerNativePort,RuntimeKey,now_ms};
use serde::{Serialize,Deserialize};
use std::{collections::{HashMap,HashSet},path::PathBuf,sync::{Arc,Mutex,atomic::{AtomicU64,Ordering}}};
use tokio::sync::{Notify,watch};
use futures::FutureExt;
use tokio_util::sync::CancellationToken;
#[derive(Clone,Debug,PartialEq,Eq,Hash,Serialize,Deserialize)]
pub struct WakeJobId {pub child:PeerKey,pub parent_id:String,pub turn:u64}
#[derive(Clone,Debug)]pub struct WakeRequest {pub runtime:RuntimeKey,pub job:WakeJobId,pub messages:Vec<PeerMessage>,pub rendered:Vec<String>}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct WakeOutcome {pub accepted:bool,pub response_at:Option<u64>,pub answer:Option<String>,pub output_path:Option<PathBuf>,pub error:Option<String>,pub cancelled:bool,pub accepted_yield:bool}
impl WakeOutcome {
 pub fn failed(error:impl Into<String>)->Self{Self{error:Some(error.into()),..Self::default()}}
 pub fn successful(&self)->bool{!self.cancelled&&self.error.is_none()&&(self.accepted_yield||self.answer.as_deref().is_some_and(|s|!s.trim().is_empty()))}
 pub fn relay_body(&self,id:&str,already_messaged:bool)->String{
  let transcript=format!("See history://{id} for details.");
  let summary=self.output_path.as_ref().filter(|_|self.accepted_yield).map(|_|format!("<task-result id=\"{id}\">\n{}\nRead agent://{id} for the recorded result.\n</task-result>",self.answer.as_deref().unwrap_or("")));
  let headline=if let Some(error)=&self.error{Some(format!("Wake turn failed: {error}."))}else if self.cancelled{Some("Wake turn was cancelled.".into())}else{None};
  if let Some(headline)=headline{if let Some(summary)=summary{return format!("{headline} A result was recorded before the turn ended:\n\n{summary}\n\n{transcript}");}let context=if already_messaged{"You received a message earlier in this turn, but that was not the answer and nothing further was sent."}else{"No answer was produced."};return format!("{headline} {context} {transcript}");}
  if let Some(summary)=summary{return summary;}if let Some(answer)=self.answer.as_deref().filter(|s|!s.trim().is_empty()){return answer.trim().into();}
  format!("Wake turn produced no output. {transcript}")
 }
}
#[derive(Clone,Debug,Serialize,Deserialize)]pub struct WakeCompletion {pub job:WakeJobId,pub runtime:RuntimeKey,pub outcome:WakeOutcome}
#[derive(Clone,Debug,Serialize,Deserialize)]pub struct ParentAdoption {pub session_id:String,pub entry_id:String,pub persistent:bool}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(tag="state",content="receipt",rename_all="snake_case")]
pub enum ParentConsumption {Ready,AutoInFlight,ForegroundReserved{token:u64},Durable(ParentAdoption),Unknown(String)}
#[derive(Clone,Debug,Serialize,Deserialize)]pub struct PeerJobSnapshot {pub id:WakeJobId,pub runtime:RuntimeKey,pub completion:Option<WakeCompletion>,pub consumption:ParentConsumption}
type AdoptionResult=Option<Result<ParentAdoption,String>>;
struct Job {runtime:RuntimeKey,completion:Option<WakeCompletion>,consumption:ParentConsumption,adoption:Option<ParentAdoption>,operation:Option<watch::Receiver<AdoptionResult>>,foreground:Option<u64>,cancel:CancellationToken}
struct Active {runtime:RuntimeKey,cancel:CancellationToken}
#[derive(Default)]struct State {jobs:HashMap<WakeJobId,Job>,order:Vec<WakeJobId>,active:HashMap<WakeJobId,Active>,watches:HashMap<u64,HashSet<WakeJobId>>,sealed_roots:HashSet<String>}
/// Private fields prevent callers from forging a reservation from model-visible details.
#[derive(Clone,Debug)]pub struct ForegroundReservation {token:u64,root:String,parent:String,completions:Vec<WakeCompletion>}
impl ForegroundReservation {pub fn token(&self)->u64{self.token}pub fn completions(&self)->&[WakeCompletion]{&self.completions}}
pub struct PeerJobWatch {owner:Arc<PeerWakeJobs>,token:u64,root:String,parent:String}
impl PeerJobWatch {
 pub fn snapshots(&self)->Vec<PeerJobSnapshot>{let state=self.owner.state.lock().unwrap_or_else(|e|e.into_inner());let Some(ids)=state.watches.get(&self.token)else{return Vec::new()};state.order.iter().filter(|id|ids.contains(*id)).filter_map(|id|state.jobs.get(id).map(|j|PeerJobSnapshot{id:id.clone(),runtime:j.runtime.clone(),completion:j.completion.clone(),consumption:j.consumption.clone()})).collect()}
 pub fn reserve_ready(&self)->Option<ForegroundReservation>{
  let mut state=self.owner.state.lock().unwrap_or_else(|e|e.into_inner());let ids=state.watches.get(&self.token)?.clone();let mut completions=Vec::new();
  for id in state.order.clone(){if !ids.contains(&id){continue;}if let Some(job)=state.jobs.get_mut(&id){if matches!(job.consumption,ParentConsumption::Ready){if let Some(completion)=job.completion.clone(){job.consumption=ParentConsumption::ForegroundReserved{token:self.token};job.foreground=Some(self.token);completions.push(completion);}}}}
  if completions.is_empty(){None}else{Some(ForegroundReservation{token:self.token,root:self.root.clone(),parent:self.parent.clone(),completions})}
 }
}
impl Drop for PeerJobWatch {fn drop(&mut self){self.owner.state.lock().unwrap_or_else(|e|e.into_inner()).watches.remove(&self.token);self.owner.notify.notify_waiters();}}
pub struct PeerWakeJobs {native:Arc<dyn PeerNativePort>,state:Arc<Mutex<State>>,notify:Arc<Notify>,sequence:AtomicU64}
impl PeerWakeJobs {
 pub fn new(native:Arc<dyn PeerNativePort>)->Self{Self{native,state:Arc::new(Mutex::new(State::default())),notify:Arc::new(Notify::new()),sequence:AtomicU64::new(1)}}
 pub fn wake(&self)->Arc<Notify>{self.notify.clone()}
 pub fn seal_root(&self,root:&str){let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.sealed_roots.insert(root.into());for(id,a)in &state.active{if id.child.root==root{a.cancel.cancel();}}for(id,j)in &state.jobs{if id.child.root==root{j.cancel.cancel();}}drop(state);self.notify.notify_waiters();}
 pub fn active_root(&self,root:&str)->Vec<WakeJobId>{self.state.lock().unwrap_or_else(|e|e.into_inner()).active.keys().filter(|id|id.child.root==root).cloned().collect()}
 pub fn snapshot_root(&self,root:&str)->Vec<PeerJobSnapshot>{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.order.iter().filter(|id|id.child.root==root).filter_map(|id|state.jobs.get(id).map(|j|PeerJobSnapshot{id:id.clone(),runtime:j.runtime.clone(),completion:j.completion.clone(),consumption:j.consumption.clone()})).collect()}
 pub fn next_id(&self,child:PeerKey,parent_id:String)->WakeJobId{WakeJobId{child,parent_id,turn:self.sequence.fetch_add(1,Ordering::AcqRel)}}
 /// Called synchronously before an accepted autonomous yield becomes idle.
 /// Registration during a parent-triggered wake is idempotent for the same turn.
 pub fn register(&self,id:WakeJobId,runtime:RuntimeKey)->anyhow::Result<()> {
  anyhow::ensure!(id.child==runtime.peer,"Wake task/ref mismatch");
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
  anyhow::ensure!(!state.sealed_roots.contains(&id.child.root),"Wake root is closed");
  if let Some(existing)=state.jobs.get(&id){anyhow::ensure!(existing.runtime==runtime,"Wake task was replaced");return Ok(());}
  let cancel=if let Some(active)=state.active.get(&id){anyhow::ensure!(active.runtime==runtime,"Wake native generation changed");active.cancel.clone()}else{CancellationToken::new()};
  state.order.push(id.clone());state.jobs.insert(id,Job{runtime,completion:None,consumption:ParentConsumption::Ready,adoption:None,operation:None,foreground:None,cancel});drop(state);self.notify.notify_waiters();Ok(())
 }
 pub fn begin_turn(&self,id:WakeJobId,runtime:RuntimeKey,parent_triggered:bool)->anyhow::Result<CancellationToken>{
  let cancel=CancellationToken::new();{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());anyhow::ensure!(!state.sealed_roots.contains(&id.child.root),"Wake root is closed");anyhow::ensure!(!state.active.contains_key(&id),"Wake turn already exists");state.active.insert(id.clone(),Active{runtime:runtime.clone(),cancel:cancel.clone()});}
  if parent_triggered{if let Err(error)=self.register(id.clone(),runtime){self.state.lock().unwrap_or_else(|e|e.into_inner()).active.remove(&id);return Err(error);}}Ok(cancel)
 }
 pub fn contains(&self,id:&WakeJobId)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).jobs.contains_key(id)}
 pub fn settle(&self,completion:WakeCompletion)->anyhow::Result<()> {
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let Some(job)=state.jobs.get_mut(&completion.job)else{return Ok(())};anyhow::ensure!(job.runtime==completion.runtime,"Stale wake result");
  if job.completion.is_none(){job.completion=Some(completion);}drop(state);self.notify.notify_waiters();Ok(())
 }
 pub fn finish_replies(&self,id:&WakeJobId){self.state.lock().unwrap_or_else(|e|e.into_inner()).active.remove(id);self.notify.notify_waiters();}
 pub fn has_owned(&self,runtime:&RuntimeKey)->bool{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.active.values().any(|a|a.runtime==*runtime)||state.jobs.values().any(|j|j.runtime==*runtime&&j.adoption.is_none())}
 pub fn pending_parent(&self,root:&str,parent:&str)->Vec<WakeCompletion>{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.order.iter().filter(|id|id.child.root==root&&id.parent_id==parent&&!state.watches.values().any(|ids|ids.contains(*id))).filter_map(|id|state.jobs.get(id)).filter(|job|matches!(job.consumption,ParentConsumption::Ready)).filter_map(|job|job.completion.clone()).collect()}
 pub fn snapshot_parent(&self,root:&str,parent:&str)->Vec<PeerJobSnapshot>{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.order.iter().filter(|id|id.child.root==root&&id.parent_id==parent).filter_map(|id|state.jobs.get(id).map(|j|PeerJobSnapshot{id:id.clone(),runtime:j.runtime.clone(),completion:j.completion.clone(),consumption:j.consumption.clone()})).collect()}
 pub fn watch_parent(self:&Arc<Self>,root:&str,parent:&str)->PeerJobWatch{let token=self.sequence.fetch_add(1,Ordering::AcqRel);let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let ids=state.jobs.iter().filter(|(id,j)|id.child.root==root&&id.parent_id==parent&&j.adoption.is_none()).map(|(id,_)|id.clone()).collect();state.watches.insert(token,ids);PeerJobWatch{owner:self.clone(),token,root:root.into(),parent:parent.into()}}
 fn validate_reservation(state:&State,reservation:&ForegroundReservation)->anyhow::Result<()>{anyhow::ensure!(!reservation.completions.is_empty(),"Empty foreground reservation");for c in &reservation.completions{anyhow::ensure!(c.job.child.root==reservation.root&&c.job.parent_id==reservation.parent,"Foreground parent owner mismatch");let j=state.jobs.get(&c.job).ok_or_else(||anyhow::anyhow!("Foreground job disappeared"))?;anyhow::ensure!(j.runtime==c.runtime&&j.foreground==Some(reservation.token)&&matches!(j.consumption,ParentConsumption::ForegroundReserved{..}|ParentConsumption::Unknown(_)),"Foreground job reservation changed");}Ok(())}
 pub fn commit_foreground(&self,reservation:&ForegroundReservation,receipt:ParentAdoption)->anyhow::Result<()>{
  anyhow::ensure!(receipt.persistent&&receipt.session_id==reservation.parent&&!receipt.entry_id.is_empty(),"Foreground result lacks exact durable ToolResult receipt");let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
  // A repeated reconciliation is idempotent only for the original durable entry.
  if reservation.completions.iter().all(|c|state.jobs.get(&c.job).is_some_and(|j|j.runtime==c.runtime&&j.adoption.as_ref().is_some_and(|r|r.session_id==receipt.session_id&&r.entry_id==receipt.entry_id&&r.persistent))){return Ok(());}
  Self::validate_reservation(&state,reservation)?;for c in &reservation.completions{let j=state.jobs.get_mut(&c.job).unwrap();j.adoption=Some(receipt.clone());j.consumption=ParentConsumption::Durable(receipt.clone());}drop(state);self.notify.notify_waiters();Ok(())
 }
 pub fn release_foreground_not_dispatched(&self,reservation:&ForegroundReservation)->anyhow::Result<()>{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());Self::validate_reservation(&state,reservation)?;for c in &reservation.completions{anyhow::ensure!(matches!(state.jobs.get(&c.job).unwrap().consumption,ParentConsumption::ForegroundReserved{..}),"Unknown foreground effects require reconciliation, not release");}for c in &reservation.completions{let j=state.jobs.get_mut(&c.job).unwrap();j.consumption=ParentConsumption::Ready;j.foreground=None;}drop(state);self.notify.notify_waiters();Ok(())}
 pub fn mark_foreground_unknown(&self,reservation:&ForegroundReservation,error:impl Into<String>)->anyhow::Result<()>{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());Self::validate_reservation(&state,reservation)?;let error=error.into();for c in &reservation.completions{state.jobs.get_mut(&c.job).unwrap().consumption=ParentConsumption::Unknown(format!("foreground {}: {error}",reservation.token));}drop(state);self.notify.notify_waiters();Ok(())}
 /// Only a reconciled original journal receipt may advance an Unknown operation.
 pub fn reconcile_adoption(&self,id:&WakeJobId,runtime:&RuntimeKey,receipt:ParentAdoption)->anyhow::Result<()>{anyhow::ensure!(receipt.persistent&&receipt.session_id==id.parent_id&&!receipt.entry_id.is_empty(),"Parent receipt is not durable or has another owner");let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let j=state.jobs.get_mut(id).filter(|j|j.runtime==*runtime).ok_or_else(||anyhow::anyhow!("Parent reconciliation runtime changed"))?;if let Some(old)=&j.adoption{anyhow::ensure!(old.entry_id==receipt.entry_id&&old.session_id==receipt.session_id,"Parent result already has another durable entry");}j.adoption=Some(receipt.clone());j.consumption=ParentConsumption::Durable(receipt);drop(state);self.notify.notify_waiters();Ok(())}
 pub fn has_parent_work(&self,root:&str,parent:&str)->bool{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.jobs.iter().any(|(id,j)|id.child.root==root&&id.parent_id==parent&&j.adoption.is_none())}
 pub async fn claim_and_adopt(&self,id:&WakeJobId)->anyhow::Result<Option<ParentAdoption>>{
  let (mut done,launch)={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.watches.values().any(|ids|ids.contains(id)){return Ok(None);}let sealed=state.sealed_roots.contains(&id.child.root);let Some(job)=state.jobs.get_mut(id)else{return Ok(None)};
   if sealed&&matches!(job.consumption,ParentConsumption::Ready){anyhow::bail!("Parent result root closed before dispatch");}
   match &job.consumption {ParentConsumption::Durable(_)=>return Ok(None),ParentConsumption::Unknown(error)=>anyhow::bail!("Parent result effect is unresolved: {error}"),ParentConsumption::ForegroundReserved{..}=>return Ok(None),ParentConsumption::AutoInFlight=>(job.operation.clone().ok_or_else(||anyhow::anyhow!("Adoption operation missing"))?,None),ParentConsumption::Ready=>{
    let Some(completion)=job.completion.clone()else{return Ok(None)};let(tx,rx)=watch::channel(None);job.consumption=ParentConsumption::AutoInFlight;job.operation=Some(rx.clone());(rx,Some((tx,completion)))
   }}
  };
  if let Some((finished,completion))=launch{let native=self.native.clone();let state=self.state.clone();let notify=self.notify.clone();tokio::spawn(async move{
   // This task owns dispatch and survives observer cancellation. No Drop path grants replay.
   let result=std::panic::AssertUnwindSafe(native.persist_parent_result(&completion)).catch_unwind().await;
   let mut result=match result{Ok(Ok(receipt))if receipt.persistent&&receipt.session_id==completion.job.parent_id&&!receipt.entry_id.is_empty()=>Ok(receipt),Ok(Ok(_))=>Err("Parent result returned a non-durable or wrong-owner receipt".into()),Ok(Err(error))=>Err(error.to_string()),Err(_)=>Err("Parent result operation panicked after possible dispatch".into())};
   {let mut state=state.lock().unwrap_or_else(|e|e.into_inner());if let Some(job)=state.jobs.get_mut(&completion.job).filter(|j|j.runtime==completion.runtime){if let Some(receipt)=&job.adoption{result=Ok(receipt.clone());}match &result{Ok(receipt)=>{job.adoption=Some(receipt.clone());job.consumption=ParentConsumption::Durable(receipt.clone());},Err(error)=>job.consumption=ParentConsumption::Unknown(error.clone())}}else{result=Err("Wake owner changed during durable adoption".into());}}
   let _=finished.send(Some(result));notify.notify_waiters();
  });}
  loop{if let Some(result)=done.borrow().clone(){return result.map(Some).map_err(anyhow::Error::msg);}done.changed().await.map_err(|_|anyhow::anyhow!("Parent adoption operation ended without a durable receipt"))?;}
 }
 /// Wait and ordinary completion delivery share this exact reservation and append receipt.
 pub async fn wait_parent(&self,root:&str,parent:&str,cancel:&CancellationToken)->anyhow::Result<Option<WakeCompletion>>{
  loop{anyhow::ensure!(!self.state.lock().unwrap_or_else(|e|e.into_inner()).sealed_roots.contains(root),"Parent task root closed");let notified=self.notify.notified();tokio::pin!(notified);notified.as_mut().enable();
   let ready=self.pending_parent(root,parent).into_iter().next();
   if let Some(completion)=ready{if self.claim_and_adopt(&completion.job).await?.is_some(){return Ok(Some(completion));}continue;}
   if !self.has_parent_work(root,parent){return Ok(None);}
   tokio::select!{_=cancel.cancelled()=>anyhow::bail!("Parent task wait cancelled"),_=notified=>{}}
  }
 }
 pub async fn cancel(&self,id:&WakeJobId)->anyhow::Result<bool>{
  let runtime={let state=self.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(job)=state.jobs.get(id){if job.completion.is_some(){return Ok(false);}job.cancel.cancel();job.runtime.clone()}else if let Some(active)=state.active.get(id){active.cancel.cancel();active.runtime.clone()}else{return Ok(false)}};
  self.native.cancel_turn(&runtime,id.turn).await?;Ok(true)
 }
 pub async fn cancel_peer(&self,key:&PeerKey)->anyhow::Result<()>{let ids={let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.active.keys().chain(state.jobs.keys()).filter(|id|id.child==*key).cloned().collect::<HashSet<_>>()};for id in ids{self.cancel(&id).await?;}Ok(())}
}
impl PeerCoordinator {
 pub fn start_wake(self:&Arc<Self>,runtime:RuntimeKey,messages:Vec<PeerMessage>)->anyhow::Result<WakeJobId>{
  anyhow::ensure!(self.registry.owns_runtime(&runtime)&&!self.registry.root_is_sealed(&runtime.peer.root),"Wake lost exact native ref or root closed");let record=self.registry.get_exact(&runtime.peer).ok_or_else(||anyhow::anyhow!("Wake ref gone"))?;
  let parent=record.parent_id.clone().unwrap_or_default();let job=self.jobs.next_id(record.key.clone(),parent.clone());
  let parent_triggered=!parent.is_empty()&&messages.iter().any(|m|m.from==parent&&!m.wake_relay);
  let cancel=self.jobs.begin_turn(job.clone(),runtime.clone(),parent_triggered)?;
  let this=self.clone();let returned=job.clone();
  tokio::spawn(async move {
   let start=now_ms();let rendered=messages.iter().map(|m|super::native::render_incoming(m,false,record.kind!=super::PeerKind::Main&&!m.wake_relay,record.parent_id.as_deref()==Some(m.from.as_str()))).collect();
   let request=WakeRequest{runtime:runtime.clone(),job:job.clone(),messages:messages.clone(),rendered};
   let mut outcome=match this.native.drive_wake(request,cancel).await{Ok(outcome)=>outcome,Err(error)=>WakeOutcome::failed(format!("finalization: {error}"))};
   if outcome.accepted_yield&&!parent.is_empty()&&!this.jobs.contains(&job){outcome.error=Some("Accepted yield did not register its parent task owner before idle".into());outcome.accepted=false;}
   if outcome.error.is_none()&&!outcome.cancelled&&!outcome.accepted_yield&&outcome.answer.as_deref().is_none_or(|s|s.trim().is_empty()){outcome.error=Some("Wake turn produced no output".into());}
   if outcome.accepted_yield&&!outcome.accepted&&outcome.error.is_none(){outcome.error=Some("Wake finalization did not accept the observed yield".into());}
   this.registry.finish_wake(&runtime,job.turn,outcome.accepted_yield&&outcome.accepted,outcome.response_at);
   let _=this.jobs.settle(WakeCompletion{job:job.clone(),runtime:runtime.clone(),outcome:outcome.clone()});
   // Every source gets one obligation; job owner uses task completion, never an extra IRC relay.
   let mut sources=HashSet::new();for source in &messages {
    if record.kind==super::PeerKind::Main||source.wake_relay||source.from==record.key.id||!sources.insert(source.from.clone()){continue;}
    if this.jobs.contains(&job)&&source.from==parent{continue;}
    let already=this.inbox.sent_since(&record.key.root,&record.key.id,&source.from,start);
    if already&&outcome.successful(){continue;}
    let mut relay=super::PeerInbox::fresh(&record.key.id,&source.from,&outcome.relay_body(&record.key.id,already));relay.reply_to=Some(source.id.clone());relay.wake_relay=true;
    let _=this.deliver_message(&record.key.root,relay).await;
   }
   this.jobs.finish_replies(&job);
   if this.registry.owns_runtime(&runtime){this.lifecycle.arm_idle(&record.key);}
  });Ok(returned)
 }
}
