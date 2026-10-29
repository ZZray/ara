//! Exact registry/reference generation ownership; fixed registry/agent-registry.ts.
use super::now_ms;
use serde::{Serialize,Deserialize};
use std::{collections::{HashMap,HashSet},path::PathBuf,sync::Mutex};
#[derive(Clone,Debug,PartialEq,Eq,Hash,Serialize,Deserialize)]
pub struct PeerKey {pub root:String,pub id:String,pub generation:u64}
#[derive(Clone,Debug,PartialEq,Eq,Hash,Serialize,Deserialize)]
pub struct RuntimeKey {pub peer:PeerKey,pub runtime_generation:u64}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum PeerKind {Main,Sub,Advisor}
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
#[serde(rename_all="snake_case")]
pub enum PeerStatus {Running,Idle,Parked,Aborted}
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct RunMilestones {pub response_at:Option<u64>,pub accepted_at:Option<u64>,pub terminal_at:Option<u64>}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct PeerRecord {
 pub key:PeerKey,pub parent_id:Option<String>,pub display_name:String,pub kind:PeerKind,pub status:PeerStatus,
 pub runtime:Option<RuntimeKey>,pub session_file:Option<PathBuf>,pub created_at:u64,pub last_activity:u64,
 pub activity:Option<String>,pub history:serde_json::Value,pub lifecycle:RunMilestones,
}
#[derive(Clone,Debug)]
pub struct PeerRegistration {
 pub root:String,pub id:String,pub parent_id:Option<String>,pub display_name:String,pub kind:PeerKind,pub status:PeerStatus,
 pub runtime_generation:Option<u64>,pub session_file:Option<PathBuf>,pub created_at:Option<u64>,pub last_activity:Option<u64>,pub history:serde_json::Value,
}
impl PeerRegistration {
 pub fn child(root:impl Into<String>,id:impl Into<String>,parent:impl Into<String>,file:PathBuf)->Self{let id=id.into();Self{root:root.into(),display_name:id.clone(),id,parent_id:Some(parent.into()),kind:PeerKind::Sub,status:PeerStatus::Parked,runtime_generation:None,session_file:Some(file),created_at:None,last_activity:None,history:serde_json::Value::Null}}
}
/// Publication is checked at the actual endpoint bind, never after a cold actor is exposed.
#[derive(Clone,Debug)]pub enum AttachmentExpectation {Absent,Parked(PeerRecord),Rebind(RuntimeKey)}
#[derive(Default)]struct State {next:u64,refs:HashMap<(String,String),PeerRecord>,order:Vec<(String,String)>,turns:HashMap<RuntimeKey,u64>,sealed:HashSet<String>}
#[derive(Clone,Debug)]pub enum RegistryEvent {Status(PeerKey,PeerStatus),Removed(PeerKey)}
type Listener=std::sync::Arc<dyn Fn(RegistryEvent)+Send+Sync>;
#[derive(Default)]pub struct PeerRegistry {state:Mutex<State>,listeners:Mutex<Vec<Listener>>}
impl PeerRegistry {
 pub fn on_change(&self,listener:Listener){self.listeners.lock().unwrap_or_else(|e|e.into_inner()).push(listener);}
 fn emit(&self,event:RegistryEvent){let listeners=self.listeners.lock().unwrap_or_else(|e|e.into_inner()).clone();for listener in listeners{let _=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||listener(event.clone())));}}
 pub fn register(&self,input:PeerRegistration)->PeerRecord {let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());Self::insert(&mut state,input)}
 fn insert(state:&mut State,input:PeerRegistration)->PeerRecord {
  state.next=state.next.checked_add(1).expect("Peer generation exhausted");let key=PeerKey{root:input.root,id:input.id,generation:state.next};
  let record=PeerRecord{runtime:input.runtime_generation.map(|runtime_generation|RuntimeKey{peer:key.clone(),runtime_generation}),key:key.clone(),parent_id:input.parent_id,display_name:input.display_name,kind:input.kind,status:input.status,session_file:input.session_file,created_at:input.created_at.unwrap_or_else(now_ms),last_activity:input.last_activity.unwrap_or_else(now_ms),activity:None,history:input.history,lifecycle:RunMilestones::default()};
  let slot=(key.root,key.id);if !state.refs.contains_key(&slot){state.order.push(slot.clone());}state.refs.insert(slot,record.clone());record
 }
 /// None expects absence. Some expects that exact parked, detached ref and reuses its generation.
 pub fn register_if_available(&self,input:PeerRegistration,expected:Option<&PeerKey>)->Option<PeerRecord>{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let slot=(input.root.clone(),input.id.clone());
  if state.sealed.contains(&input.root){return None;}
  match expected {None=>if state.refs.contains_key(&slot){None}else{Some(Self::insert(&mut state,input))},Some(key)=>state.refs.get(&slot).filter(|r|r.key==*key&&r.status==PeerStatus::Parked&&r.runtime.is_none()).cloned()}
 }
 pub fn get(&self,root:&str,id:&str)->Option<PeerRecord>{self.state.lock().unwrap_or_else(|e|e.into_inner()).refs.get(&(root.into(),id.into())).cloned()}
 pub fn get_exact(&self,key:&PeerKey)->Option<PeerRecord>{self.get(&key.root,&key.id).filter(|r|r.key==*key)}
 pub fn list(&self,root:&str)->Vec<PeerRecord>{let state=self.state.lock().unwrap_or_else(|e|e.into_inner());state.order.iter().filter(|(namespace,_)|namespace==root).filter_map(|slot|state.refs.get(slot).cloned()).collect()}
 pub fn visible(&self,root:&str,caller:&str)->Vec<PeerRecord>{self.list(root).into_iter().filter(|r|r.key.id!=caller&&r.kind!=PeerKind::Advisor&&matches!(r.status,PeerStatus::Running|PeerStatus::Idle)).collect()}
 fn mutate(&self,key:&PeerKey,f:impl FnOnce(&mut PeerRecord)->bool)->bool{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let Some(r)=state.refs.get_mut(&(key.root.clone(),key.id.clone())).filter(|r|r.key==*key)else{return false};f(r)}
 pub fn owns_runtime(&self,runtime:&RuntimeKey)->bool{self.get_exact(&runtime.peer).is_some_and(|r|r.runtime.as_ref()==Some(runtime)&&r.status!=PeerStatus::Aborted)}
 pub fn set_status(&self,key:&PeerKey,status:PeerStatus)->bool{let mut changed=false;let success=self.mutate(key,|r|{
  if r.status==PeerStatus::Aborted{return status==PeerStatus::Aborted;}
  if r.status==status{return true;}let old=r.status;r.status=status;r.last_activity=now_ms();changed=true;
  if status==PeerStatus::Running{r.lifecycle=RunMilestones::default();}else{r.activity=None;if old==PeerStatus::Running{r.lifecycle.terminal_at=Some(r.last_activity);}}true
 });if changed{self.emit(RegistryEvent::Status(key.clone(),status));}success}
 pub fn set_activity(&self,key:&PeerKey,text:&str)->bool{self.mutate(key,|r|{if r.status!=PeerStatus::Running{return false;}r.last_activity=now_ms();r.activity=Some(text.split_whitespace().collect::<Vec<_>>().join(" ").chars().filter(|c|!c.is_control()).take(160).collect());true})}
 pub fn set_history(&self,key:&PeerKey,history:serde_json::Value,expected_file:Option<&std::path::Path>)->bool{self.mutate(key,|r|{
  if expected_file.is_some_and(|file|r.session_file.as_deref()!=Some(file)){return false;}
  let Some(incoming)=history.as_object()else{return false;};if !r.history.is_object(){r.history=serde_json::json!({});}
  let target=r.history.as_object_mut().unwrap();for(k,v)in incoming{if v.is_null(){continue;}let value=if k=="directCost"&&expected_file.is_some(){match(target.get(k).and_then(|v|v.as_f64()),v.as_f64()){(Some(a),Some(b))if a.is_finite()&&a>=0.0&&b.is_finite()&&b>=0.0=>serde_json::json!(a.max(b)),_=>v.clone()}}else{v.clone()};target.insert(k.clone(),value);}true
 })}
 /// Caller holds its endpoint-table lock. `publish` is infallible and must not reenter registry.
 pub fn publish_attachment(&self,input:PeerRegistration,expected:&AttachmentExpectation,publish:impl FnOnce(&PeerRecord))->anyhow::Result<PeerRecord>{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
  anyhow::ensure!(!state.sealed.contains(&input.root),"Peer root admission is closed");
  anyhow::ensure!(input.runtime_generation.is_some()&&input.status!=PeerStatus::Aborted,"Attachment requires a live runtime");
  let slot=(input.root.clone(),input.id.clone());
  let record=match expected {
   AttachmentExpectation::Absent=>{anyhow::ensure!(!state.refs.contains_key(&slot),"Peer attachment expected absence");Self::insert(&mut state,input)},
   AttachmentExpectation::Parked(old)=>{
    let row=state.refs.get_mut(&slot).ok_or_else(||anyhow::anyhow!("Expected parked peer is absent"))?;
    anyhow::ensure!(row.key==old.key&&row.status==PeerStatus::Parked&&row.runtime.is_none()&&old.status==PeerStatus::Parked&&old.runtime.is_none()&&row.session_file==old.session_file&&row.parent_id==old.parent_id&&row.kind==old.kind&&input.session_file==old.session_file&&input.parent_id==old.parent_id&&input.kind==old.kind,"Cold revival attachment expectation changed");
    row.runtime=Some(RuntimeKey{peer:row.key.clone(),runtime_generation:input.runtime_generation.unwrap()});row.status=input.status;row.last_activity=now_ms();row.clone()
   },
   AttachmentExpectation::Rebind(runtime)=>{
    let row=state.refs.get_mut(&slot).ok_or_else(||anyhow::anyhow!("Rebind peer is absent"))?;
    anyhow::ensure!(row.key==runtime.peer&&row.runtime.as_ref()==Some(runtime)&&row.status!=PeerStatus::Aborted&&input.runtime_generation==Some(runtime.runtime_generation)&&row.session_file==input.session_file&&row.parent_id==input.parent_id&&row.kind==input.kind,"Peer rebind no longer owns runtime");row.clone()
   }
  };
  publish(&record);drop(state);self.emit(RegistryEvent::Status(record.key.clone(),record.status));Ok(record)
 }
 pub fn root_is_sealed(&self,root:&str)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).sealed.contains(root)}
 pub fn seal_root(&self,root:&str,expected:&RuntimeKey)->bool{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
  if expected.peer.root!=root{return false;}
  let valid=state.refs.get(&(root.into(),expected.peer.id.clone())).is_some_and(|r|r.key==expected.peer&&r.runtime.as_ref()==Some(expected)&&r.kind==PeerKind::Main&&r.parent_id.is_none()&&r.status!=PeerStatus::Aborted);
  if !valid{return false;}state.sealed.insert(root.into());true
 }
 pub fn begin_wake(&self,runtime:&RuntimeKey,turn:u64)->bool{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.sealed.contains(&runtime.peer.root){return false;}
  let Some(r)=state.refs.get_mut(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|r|r.runtime.as_ref()==Some(runtime)&&r.key==runtime.peer&&r.status!=PeerStatus::Aborted)else{return false;};
  r.status=PeerStatus::Running;r.last_activity=now_ms();r.lifecycle=RunMilestones::default();state.turns.insert(runtime.clone(),turn);drop(state);self.emit(RegistryEvent::Status(runtime.peer.clone(),PeerStatus::Running));true
 }
 pub fn begin_ordinary_turn(&self,runtime:&RuntimeKey)->bool{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.sealed.contains(&runtime.peer.root){return false;}
  let Some(r)=state.refs.get_mut(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|r|r.runtime.as_ref()==Some(runtime)&&r.key==runtime.peer&&r.status!=PeerStatus::Aborted)else{return false;};
  r.status=PeerStatus::Running;r.last_activity=now_ms();r.lifecycle=RunMilestones::default();state.turns.remove(runtime);drop(state);self.emit(RegistryEvent::Status(runtime.peer.clone(),PeerStatus::Running));true
 }
 pub fn finish_wake(&self,runtime:&RuntimeKey,turn:u64,accepted:bool,response_at:Option<u64>)->bool{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.turns.get(runtime)!=Some(&turn){return false;}
  let Some(r)=state.refs.get_mut(&(runtime.peer.root.clone(),runtime.peer.id.clone())).filter(|r|r.runtime.as_ref()==Some(runtime)&&r.key==runtime.peer&&r.status!=PeerStatus::Aborted)else{return false;};
  let now=now_ms();r.status=PeerStatus::Idle;r.activity=None;r.last_activity=now;r.lifecycle.terminal_at=Some(now);r.lifecycle.response_at=response_at;if accepted{r.lifecycle.accepted_at=Some(now);}state.turns.remove(runtime);drop(state);self.emit(RegistryEvent::Status(runtime.peer.clone(),PeerStatus::Idle));true
 }
 pub fn attach(&self,key:&PeerKey,runtime:RuntimeKey)->bool{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if state.sealed.contains(&key.root){return false;}let Some(r)=state.refs.get_mut(&(key.root.clone(),key.id.clone())).filter(|r|r.key==*key&&r.status!=PeerStatus::Aborted&&runtime.peer==r.key)else{return false};r.runtime=Some(runtime);r.last_activity=now_ms();true}
 pub fn detach(&self,runtime:&RuntimeKey)->bool{self.mutate(&runtime.peer,|r|{if r.runtime.as_ref()!=Some(runtime){return false;}r.runtime=None;true})}
 pub fn mark_accepted(&self,runtime:&RuntimeKey,response_at:Option<u64>,actually_running:bool)->bool{let mut terminal=false;let success=self.mutate(&runtime.peer,|r|{
  if r.runtime.as_ref()!=Some(runtime)||r.status==PeerStatus::Aborted{return false;}let now=now_ms();r.lifecycle.response_at=Some(response_at.unwrap_or(now));r.lifecycle.accepted_at=Some(now);
  if r.status==PeerStatus::Running&&!actually_running{r.status=PeerStatus::Idle;r.activity=None;r.last_activity=now;r.lifecycle.terminal_at=Some(now);terminal=true;}true
 });if terminal{self.emit(RegistryEvent::Status(runtime.peer.clone(),PeerStatus::Idle));}success}
 /// Terminal publication detaches before aborted becomes observable.
 pub fn tombstone(&self,key:&PeerKey)->Option<Option<RuntimeKey>>{let mut old=None;if self.mutate(key,|r|{old=Some(r.runtime.take());r.status=PeerStatus::Aborted;r.activity=None;r.last_activity=now_ms();r.lifecycle.terminal_at=Some(r.last_activity);true}){self.emit(RegistryEvent::Status(key.clone(),PeerStatus::Aborted));old}else{None}}
 pub fn unregister_dead(&self,key:&PeerKey)->bool{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let slot=(key.root.clone(),key.id.clone());if state.refs.get(&slot).is_some_and(|r|r.key==*key&&r.status==PeerStatus::Parked&&r.runtime.is_none()){state.refs.remove(&slot);state.order.retain(|key|key!=&slot);drop(state);self.emit(RegistryEvent::Removed(key.clone()));true}else{false}}
 pub fn unregister(&self,key:&PeerKey)->bool{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let slot=(key.root.clone(),key.id.clone());if state.refs.get(&slot).is_some_and(|r|r.key==*key){state.refs.remove(&slot);state.order.retain(|key|key!=&slot);drop(state);self.emit(RegistryEvent::Removed(key.clone()));true}else{false}}
}
