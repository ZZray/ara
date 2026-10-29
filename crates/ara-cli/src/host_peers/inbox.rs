//! Fixed irc/bus.ts mailbox semantics. Only failed live handoffs enter this FIFO.
use super::now_ms;
use serde::{Serialize,Deserialize};
use std::{collections::{HashMap,VecDeque},sync::{Arc,Mutex},time::Duration};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
pub struct PeerMessage {pub id:String,pub from:String,pub to:String,pub body:String,pub ts:u64,pub reply_to:Option<String>,pub wake_relay:bool}
struct Waiter {id:u64,from:Option<String>,sender:oneshot::Sender<PeerMessage>}
#[derive(Default)]struct State {boxes:HashMap<(String,String),VecDeque<PeerMessage>>,waiters:HashMap<(String,String),VecDeque<Waiter>>,sent:HashMap<(String,String,String),u64>,next:u64}
#[derive(Default)]pub struct PeerInbox {state:Mutex<State>}
impl PeerInbox {
 pub fn enqueue(&self,root:&str,message:PeerMessage){let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let queue=state.boxes.entry((root.into(),message.to.clone())).or_default();queue.push_back(message);if queue.len()>100{queue.pop_front();}}
 pub fn unread(&self,root:&str,id:&str)->usize{self.state.lock().unwrap_or_else(|e|e.into_inner()).boxes.get(&(root.into(),id.into())).map_or(0,VecDeque::len)}
 fn take_locked(state:&mut State,slot:&(String,String),from:Option<&str>)->Option<PeerMessage>{let queue=state.boxes.get_mut(slot)?;let index=queue.iter().position(|m|from.is_none_or(|f|m.from==f))?;let message=queue.remove(index);if queue.is_empty(){state.boxes.remove(slot);}message}
 pub fn take(&self,root:&str,id:&str,from:Option<&str>)->Option<PeerMessage>{Self::take_locked(&mut self.state.lock().unwrap_or_else(|e|e.into_inner()),&(root.into(),id.into()),from)}
 pub fn deliver_waiter(&self,root:&str,message:&PeerMessage)->bool{
  let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let slot=(root.into(),message.to.clone());let Some(queue)=state.waiters.get_mut(&slot)else{return false};
  queue.retain(|w|!w.sender.is_closed());let mut delivered=false;
  while let Some(index)=queue.iter().position(|w|w.from.as_deref().is_none_or(|f|f==message.from)) {let waiter=queue.remove(index).unwrap();if waiter.sender.send(message.clone()).is_ok(){delivered=true;break;}}
  if queue.is_empty(){state.waiters.remove(&slot);}delivered
 }
 pub async fn wait(self:&Arc<Self>,root:&str,id:&str,from:Option<&str>,timeout_ms:i64,drain_pending:bool,cancel:&CancellationToken)->anyhow::Result<Option<PeerMessage>>{
  anyhow::ensure!(!cancel.is_cancelled(),"IRC wait aborted");
  let slot=(root.to_owned(),id.to_owned());let(waiter_id,receiver)={
   let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());if drain_pending{if let Some(m)=Self::take_locked(&mut state,&slot,from){return Ok(Some(m));}}
   state.next=state.next.checked_add(1).expect("IRC waiter identity exhausted");let waiter_id=state.next;let(tx,rx)=oneshot::channel();state.waiters.entry(slot.clone()).or_default().push_back(Waiter{id:waiter_id,from:from.map(str::to_owned),sender:tx});(waiter_id,rx)
  };
  struct Cleanup {inbox:Arc<PeerInbox>,slot:(String,String),id:u64}
  impl Drop for Cleanup {fn drop(&mut self){let mut state=self.inbox.state.lock().unwrap_or_else(|e|e.into_inner());if let Some(queue)=state.waiters.get_mut(&self.slot){queue.retain(|w|w.id!=self.id);if queue.is_empty(){state.waiters.remove(&self.slot);}}}}
  let _cleanup=Cleanup{inbox:self.clone(),slot,id:waiter_id};
  if timeout_ms>0{tokio::select!{biased;message=receiver=>Ok(Some(message.map_err(|_|anyhow::anyhow!("IRC waiter closed"))?)),_=cancel.cancelled()=>anyhow::bail!("IRC wait aborted"),_=tokio::time::sleep(Duration::from_millis(timeout_ms as u64))=>Ok(None)}}else{tokio::select!{biased;message=receiver=>Ok(Some(message.map_err(|_|anyhow::anyhow!("IRC waiter closed"))?)),_=cancel.cancelled()=>anyhow::bail!("IRC wait aborted")}}
 }
 pub fn note_sent(&self,root:&str,message:&PeerMessage){self.state.lock().unwrap_or_else(|e|e.into_inner()).sent.insert((root.into(),message.from.clone(),message.to.clone()),message.ts);}
 pub fn sent_since(&self,root:&str,from:&str,to:&str,since:u64)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).sent.get(&(root.into(),from.into(),to.into())).is_some_and(|time|*time>=since)}
 pub fn fresh(from:&str,to:&str,body:&str)->PeerMessage{PeerMessage{id:uuid::Uuid::new_v4().to_string(),from:from.into(),to:to.into(),body:body.into(),ts:now_ms(),reply_to:None,wake_relay:false}}
}
