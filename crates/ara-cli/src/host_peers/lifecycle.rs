//! Exact-ref coalesced park/revive flights; fixed registry/agent-lifecycle.ts.
use super::{PeerKey,RuntimeKey,PeerRegistry,PeerNativePort,PeerWakeJobs,PeerStatus};
use std::{collections::HashMap,sync::{Arc,Mutex,atomic::{AtomicBool,AtomicU64,Ordering}},time::Duration};
use tokio::sync::watch;
use serde::{Serialize,Deserialize};
use futures::{StreamExt,FutureExt};
struct Adoption {ttl_ms:i64,timer_epoch:u64}
struct Park {cancelled:bool,detached:bool,done:watch::Receiver<bool>}
struct Revival {done:watch::Receiver<Option<Result<RuntimeKey,String>>>}
#[derive(Clone,Debug,Serialize,Deserialize)]pub struct PeerCloseReceipt {pub key:PeerKey,pub runtime:Option<RuntimeKey>,pub settled:bool,pub error:Option<String>}
#[derive(Clone,Debug,Serialize,Deserialize)]pub struct RootCloseReceipt {pub root:String,pub expected:RuntimeKey,pub peers:Vec<PeerCloseReceipt>,pub settled:bool,pub unresolved_jobs:Vec<super::wake::PeerJobSnapshot>,pub unresolved_turns:Vec<super::WakeJobId>}
struct RootFlight {expected:RuntimeKey,current:RootCloseReceipt,done:watch::Receiver<Option<RootCloseReceipt>>}
#[derive(Default)]struct State {park_receipts:HashMap<PeerKey,Result<(),String>>,roots:HashMap<String,RootFlight>,adopted:HashMap<PeerKey,Adoption>,parks:HashMap<PeerKey,Park>,revivals:HashMap<PeerKey,Revival>}
pub struct PeerLifecycle {registry:Arc<PeerRegistry>,native:Arc<dyn PeerNativePort>,jobs:Arc<PeerWakeJobs>,state:Mutex<State>,disposed:AtomicBool,epoch:AtomicU64}
impl PeerLifecycle {
 pub fn new(registry:Arc<PeerRegistry>,native:Arc<dyn PeerNativePort>,jobs:Arc<PeerWakeJobs>)->Arc<Self>{
  let this=Arc::new(Self{registry:registry.clone(),native,jobs,state:Mutex::new(State::default()),disposed:AtomicBool::new(false),epoch:AtomicU64::new(1)});
  let weak=Arc::downgrade(&this);registry.on_change(Arc::new(move|event|{let Some(this)=weak.upgrade()else{return};match event {
   super::registry::RegistryEvent::Status(key,PeerStatus::Idle)=>this.arm_idle(&key),
   super::registry::RegistryEvent::Status(key,_)=>{if let Some(adopted)=this.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.get_mut(&key){adopted.timer_epoch=this.epoch.fetch_add(1,Ordering::AcqRel);}},
   super::registry::RegistryEvent::Removed(key)=>{this.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.remove(&key);},
  }}));this
 }
 pub fn adopt(self:&Arc<Self>,key:PeerKey,ttl_ms:i64)->anyhow::Result<()>{anyhow::ensure!(!self.disposed.load(Ordering::Acquire)&&!self.registry.root_is_sealed(&key.root),"Peer lifecycle disposed or root closed");let record=self.registry.get_exact(&key).ok_or_else(||anyhow::anyhow!("Cannot adopt missing peer"))?;if record.kind==super::PeerKind::Main{return Ok(());}self.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.insert(key.clone(),Adoption{ttl_ms,timer_epoch:0});self.arm_idle(&key);Ok(())}
 pub fn has(&self,key:&PeerKey)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.contains_key(key)}
 pub fn is_parking(&self,key:&PeerKey)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).parks.get(key).is_some_and(|p|!p.cancelled)}
 pub fn arm_idle(self:&Arc<Self>,key:&PeerKey){
  let ttl={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.parks.contains_key(key){return;}let Some(adopted)=state.adopted.get_mut(key)else{return};let epoch=self.epoch.fetch_add(1,Ordering::AcqRel);adopted.timer_epoch=epoch;if adopted.ttl_ms<=0||self.registry.get_exact(key).is_none_or(|r|r.status!=PeerStatus::Idle){return;}(adopted.ttl_ms,epoch)};
  let deadline=tokio::time::Instant::now()+Duration::from_millis(ttl.0 as u64);let weak=Arc::downgrade(self);let key=key.clone();tokio::spawn(async move{tokio::time::sleep_until(deadline).await;let Some(this)=weak.upgrade()else{return};let valid=this.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.get(&key).is_some_and(|a|a.timer_epoch==ttl.1);if valid{let _=this.park(&key).await;}});
 }
 fn cancel_park(&self,key:&PeerKey)->Option<watch::Receiver<bool>>{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let park=state.parks.get_mut(key)?;if !park.detached{park.cancelled=true;}Some(park.done.clone())}
 async fn join_park(mut done:watch::Receiver<bool>){loop{if *done.borrow(){return;}if done.changed().await.is_err(){return;}}}
 pub fn park<'a>(self:&'a Arc<Self>,key:&'a PeerKey)->std::pin::Pin<Box<dyn std::future::Future<Output=anyhow::Result<()>>+Send+'a>> {Box::pin(async move {
  let (done,launch)={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(park)=state.parks.get(key){(park.done.clone(),None)}else{
   if !state.adopted.contains_key(key){return Ok(());}let Some(record)=self.registry.get_exact(key)else{return Ok(())};let Some(runtime)=record.runtime else{return Ok(())};
   if let Some(adopted)=state.adopted.get_mut(key){adopted.timer_epoch=self.epoch.fetch_add(1,Ordering::AcqRel);}
   let(tx,rx)=watch::channel(false);state.parks.insert(key.clone(),Park{cancelled:false,detached:false,done:rx.clone()});(rx,Some((tx,runtime)))
  }};
  if let Some((finished,runtime))=launch{let this=self.clone();let key=key.clone();tokio::spawn(async move{
   tokio::task::yield_now().await;
   let state=this.native.inspect(&runtime).await;
   let may_detach=state.is_ok_and(|s|!s.disposed&&!s.busy())&&!this.jobs.has_owned(&runtime);
   let commit={let mut state=this.state.lock().unwrap_or_else(|e|e.into_inner());let valid=state.adopted.contains_key(&key)&&this.registry.owns_runtime(&runtime);if let Some(park)=state.parks.get_mut(&key){if !park.cancelled&&valid&&may_detach{park.detached=true;true}else{false}}else{false}};
   if commit&&this.registry.detach(&runtime){this.registry.set_status(&key,PeerStatus::Parked);let receipt=this.native.close(&runtime).await.map_err(|e|e.to_string());this.state.lock().unwrap_or_else(|e|e.into_inner()).park_receipts.insert(key.clone(),receipt);}
   {this.state.lock().unwrap_or_else(|e|e.into_inner()).parks.remove(&key);}let _=finished.send(true);
   if !commit{this.arm_idle(&key);}
  });}
  Self::join_park(done).await;Ok(())
 })}
 pub async fn ensure_live(self:&Arc<Self>,key:&PeerKey)->anyhow::Result<RuntimeKey>{
  anyhow::ensure!(!self.disposed.load(Ordering::Acquire)&&!self.registry.root_is_sealed(&key.root),"Peer lifecycle disposed or root closed");
  if let Some(park)=self.cancel_park(key){Self::join_park(park).await;}
  let record=self.registry.get_exact(key).ok_or_else(||anyhow::anyhow!("Peer changed or was released"))?;
  anyhow::ensure!(record.status!=PeerStatus::Aborted,"Hard-aborted peer cannot revive");
  if let Some(runtime)=record.runtime{anyhow::ensure!(self.registry.owns_runtime(&runtime),"Peer native runtime changed");self.arm_idle(key);return Ok(runtime);}
  anyhow::ensure!(record.status==PeerStatus::Parked&&record.session_file.is_some(),"Peer has no retained revivable transcript");
  let (mut done,launch)={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(revival)=state.revivals.get(key){(revival.done.clone(),None)}else{let(tx,rx)=watch::channel(None);state.revivals.insert(key.clone(),Revival{done:rx.clone()});(rx,Some(tx))}};
  if let Some(finished)=launch{let this=self.clone();let key=key.clone();tokio::spawn(async move{
   // Native reviver obtains current owning parent/factory/grants now, never a saved credential.
   let candidate=this.native.revive(&record).await;
   let result=match candidate {Err(e)=>Err(e.to_string()),Ok(runtime)=>{
    let current=this.registry.get_exact(&key);
    let already=!this.disposed.load(Ordering::Acquire)&&!this.registry.root_is_sealed(&key.root)&&runtime.peer==key&&current.as_ref().is_some_and(|r|r.runtime.as_ref()==Some(&runtime)&&matches!(r.status,PeerStatus::Idle|PeerStatus::Running)&&r.session_file==record.session_file&&r.parent_id==record.parent_id&&r.kind==record.kind);
    // Actual Native bind must already have performed the complete expected-attachment CAS.
    // A library postcheck never publishes an actor or overwrites an admitted successor turn.
    if already{Ok(runtime)}else{let _=this.native.close(&runtime).await;Err("Revival was not atomically published or owner closed".into())}
   }};
   // Failed cold revival retains file/ref and removes only this flight, so fresh context can retry.
   {let mut state=this.state.lock().unwrap_or_else(|e|e.into_inner());state.revivals.remove(&key);if result.is_ok(){state.park_receipts.remove(&key);}if result.is_ok()&&!state.adopted.contains_key(&key){state.adopted.insert(key.clone(),Adoption{ttl_ms:super::DEFAULT_IDLE_TTL_MS,timer_epoch:0});}}
   let _=finished.send(Some(result));this.arm_idle(&key);
  });}
  loop{if let Some(result)=done.borrow().clone(){return result.map_err(anyhow::Error::msg);}done.changed().await.map_err(|_|anyhow::anyhow!("Revival owner ended without a receipt"))?;}
 }
 pub async fn release(self:&Arc<Self>,key:&PeerKey,tombstone:bool)->anyhow::Result<bool>{
  let Some(record)=self.registry.get_exact(key)else{return Ok(false)};
  let captured=if tombstone{self.registry.tombstone(key).flatten()}else{record.runtime.clone()};
  self.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.remove(key);
  if let Some(park)=self.cancel_park(key){Self::join_park(park).await;}
  let cancel_result=self.jobs.cancel_peer(key).await;
  let mut marker_result:anyhow::Result<()>=Ok(());
  if tombstone{if let Some(file)=&record.session_file{let mut options=tokio::fs::OpenOptions::new();options.write(true).create_new(true);#[cfg(unix)]options.mode(0o600);marker_result=match options.open(format!("{}.tombstone",file.display())).await{Ok(_)=>Ok(()),Err(e)if e.kind()==std::io::ErrorKind::AlreadyExists=>Ok(()),Err(e)=>Err(e.into())};}}
  if let Some(runtime)=captured{let _=self.native.close(&runtime).await;}
  let owned_result=self.native.release_owned(&record).await;
  if !tombstone{self.registry.unregister(key);}
  cancel_result?;marker_result?;owned_result?;Ok(true)
 }
 pub async fn reclaim_dead(&self,key:&PeerKey)->anyhow::Result<bool>{
  let Some(record)=self.registry.get_exact(key)else{return Ok(false)};
  if record.status!=PeerStatus::Parked||record.runtime.is_some(){return Ok(false);}
  {let state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.adopted.contains_key(key)||state.parks.contains_key(key)||state.revivals.contains_key(key){return Ok(false);}}
  if self.native.can_revive(&record).await?{return Ok(false);}
  let state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.adopted.contains_key(key)||state.parks.contains_key(key)||state.revivals.contains_key(key){return Ok(false);}
  let valid=self.registry.get_exact(key).is_some_and(|r|r.status==PeerStatus::Parked&&r.runtime.is_none());drop(state);Ok(valid&&self.registry.unregister_dead(key))
 }
 pub fn root_close_snapshot(&self,root:&str)->Option<RootCloseReceipt>{let mut receipt=self.state.lock().unwrap_or_else(|e|e.into_inner()).roots.get(root)?.current.clone();receipt.unresolved_jobs=self.jobs.snapshot_root(root).into_iter().filter(|j|j.completion.is_none()||matches!(j.consumption,super::wake::ParentConsumption::AutoInFlight|super::wake::ParentConsumption::ForegroundReserved{..}|super::wake::ParentConsumption::Unknown(_))).collect();receipt.unresolved_turns=self.jobs.active_root(root);receipt.settled=receipt.settled&&receipt.unresolved_jobs.is_empty()&&receipt.unresolved_turns.is_empty();Some(receipt)}
 /// A retained root flight owns closes; dropped/timeout observers never detach the operation.
 pub async fn release_root(self:&Arc<Self>,root:&str,expected:&RuntimeKey)->anyhow::Result<RootCloseReceipt>{
  let (mut done,launch)={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(flight)=state.roots.get(root){anyhow::ensure!(flight.expected==*expected,"Root close owner changed");(flight.done.clone(),None)}else{
   // Never hold this lifecycle lock while registry emits callbacks.
   drop(state);anyhow::ensure!(self.registry.seal_root(root,expected),"Root close lost exact Main runtime");self.jobs.seal_root(root);
   let records=self.registry.list(root).into_iter().filter(|r|r.key!=expected.peer).collect::<Vec<_>>();
   let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(flight)=state.roots.get(root){anyhow::ensure!(flight.expected==*expected,"Root close owner changed");(flight.done.clone(),None)}else{
    let receipt=RootCloseReceipt{root:root.into(),expected:expected.clone(),peers:records.iter().map(|r|PeerCloseReceipt{key:r.key.clone(),runtime:r.runtime.clone(),settled:false,error:Some("Close in flight; effects not yet settled".into())}).collect(),settled:false,unresolved_jobs:Vec::new(),unresolved_turns:Vec::new()};let(tx,rx)=watch::channel(None);state.roots.insert(root.into(),RootFlight{expected:expected.clone(),current:receipt,done:rx.clone()});(rx,Some((tx,records)))
   }
  }};
  if let Some((finished,records))=launch{let this=self.clone();let root=root.to_owned();tokio::spawn(async move{
   let mut pending=futures::stream::FuturesUnordered::new();
   for record in records{let this=this.clone();pending.push(async move{
    this.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.remove(&record.key);
    let park=this.cancel_park(&record.key);
    let key=record.key.clone();let runtime=record.runtime.clone();
    let operation=async{
     // Siblings run independently; each child joins cancellation ACK before close can end its actor.
     let a=this.jobs.cancel_peer(&key).await; // Retain errors; cleanup still runs, without a lock across await.
     let close=async{if let Some(done)=park{Self::join_park(done).await;}let captured=this.registry.tombstone(&key).flatten();if let Some(runtime)=&captured{this.native.close(runtime).await}else{this.state.lock().unwrap_or_else(|e|e.into_inner()).park_receipts.get(&key).cloned().unwrap_or(Ok(())).map_err(anyhow::Error::msg)}};
     let release=this.native.release_owned(&record);let(b,c)=tokio::join!(close,release);let mut errors=Vec::new();for result in [a,b,c]{if let Err(e)=result{errors.push(e.to_string());}}
     if errors.is_empty(){this.registry.unregister(&key);None}else{Some(errors.join("; "))}
    };
    let error=match std::panic::AssertUnwindSafe(operation).catch_unwind().await{Ok(error)=>error,Err(_)=>Some("Close panicked; effect settlement unknown".into())};PeerCloseReceipt{key,runtime,settled:error.is_none(),error}
   });}
   while let Some(receipt)=pending.next().await{if let Some(flight)=this.state.lock().unwrap_or_else(|e|e.into_inner()).roots.get_mut(&root){if let Some(slot)=flight.current.peers.iter_mut().find(|p|p.key==receipt.key){*slot=receipt;}}}
   let receipt={let mut state=this.state.lock().unwrap_or_else(|e|e.into_inner());let flight=state.roots.get_mut(&root).unwrap();flight.current.settled=flight.current.peers.iter().all(|p|p.settled);flight.current.clone()};let _=finished.send(Some(receipt));
  });}
  loop{if let Some(mut receipt)=done.borrow().clone(){receipt.unresolved_jobs=self.jobs.snapshot_root(root).into_iter().filter(|j|j.completion.is_none()||matches!(j.consumption,super::wake::ParentConsumption::AutoInFlight|super::wake::ParentConsumption::ForegroundReserved{..}|super::wake::ParentConsumption::Unknown(_))).collect();receipt.unresolved_turns=self.jobs.active_root(root);receipt.settled=receipt.settled&&receipt.unresolved_jobs.is_empty()&&receipt.unresolved_turns.is_empty();return Ok(receipt);}done.changed().await.map_err(|_|anyhow::anyhow!("Root close flight ended with Unknown settlement"))?;}
 }
 pub async fn dispose(self:&Arc<Self>){self.disposed.store(true,Ordering::Release);let keys=self.state.lock().unwrap_or_else(|e|e.into_inner()).adopted.keys().cloned().collect::<Vec<_>>();let mut pending=futures::stream::FuturesUnordered::new();for key in keys{let this=self.clone();let task=tokio::spawn(async move{let _=this.release(&key,false).await;});pending.push(async move{let _=tokio::time::timeout(Duration::from_secs(5),task).await;});}use futures::StreamExt;while pending.next().await.is_some(){}}

}
