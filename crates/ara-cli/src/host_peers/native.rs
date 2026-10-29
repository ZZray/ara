//! Fixed session/irc-bridge.ts queues and trusted projection, no provider owner here.
use super::{PeerMessage,RuntimeKey};
use serde::{Serialize,Deserialize};
use std::{collections::HashMap,sync::{Mutex,OnceLock}};
#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum DeliveryRoute {ParentSteer,SiblingInterrupt,PlanAppend}
#[derive(Clone,Debug,Serialize,Deserialize)]
pub struct NativeDelivery {pub route:DeliveryRoute,pub message:PeerMessage,pub rendered:String}
#[derive(Clone,Debug,Default)]
pub struct QueueSnapshot {pub interrupts:Vec<NativeDelivery>,pub asides:Vec<NativeDelivery>,pub deferred_wakes:Vec<PeerMessage>}
/// Runtime-keyed switch snapshots cannot leak A's queues into B or a revived A.
#[derive(Default)]pub struct PeerQueues {state:Mutex<HashMap<RuntimeKey,QueueSnapshot>>}
impl PeerQueues {
 pub fn interrupt(&self,runtime:&RuntimeKey,message:NativeDelivery){self.state.lock().unwrap_or_else(|e|e.into_inner()).entry(runtime.clone()).or_default().interrupts.push(message);}
 pub fn aside(&self,runtime:&RuntimeKey,message:NativeDelivery){self.state.lock().unwrap_or_else(|e|e.into_inner()).entry(runtime.clone()).or_default().asides.push(message);}
 pub fn defer_wake(&self,runtime:&RuntimeKey,messages:Vec<PeerMessage>){self.state.lock().unwrap_or_else(|e|e.into_inner()).entry(runtime.clone()).or_default().deferred_wakes.extend(messages);}
 pub fn has_pending(&self,runtime:&RuntimeKey)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).get(runtime).is_some_and(|q|!q.interrupts.is_empty()||!q.asides.is_empty()||!q.deferred_wakes.is_empty())}
 pub fn has_interrupts(&self,runtime:&RuntimeKey)->bool{self.state.lock().unwrap_or_else(|e|e.into_inner()).get(runtime).is_some_and(|q|!q.interrupts.is_empty())}
 pub fn clear(&self,runtime:&RuntimeKey)->QueueSnapshot{self.state.lock().unwrap_or_else(|e|e.into_inner()).remove(runtime).unwrap_or_default()}
 pub fn restore(&self,runtime:&RuntimeKey,mut snapshot:QueueSnapshot){let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let current=state.entry(runtime.clone()).or_default();snapshot.interrupts.append(&mut current.interrupts);snapshot.asides.append(&mut current.asides);snapshot.deferred_wakes.append(&mut current.deferred_wakes);*current=snapshot;}
 pub fn drain_deferred(&self,runtime:&RuntimeKey)->Vec<PeerMessage>{self.state.lock().unwrap_or_else(|e|e.into_inner()).get_mut(runtime).map(|q|std::mem::take(&mut q.deferred_wakes)).unwrap_or_default()}
 pub fn drain_injection(&self,runtime:&RuntimeKey)->Vec<NativeDelivery>{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let Some(q)=state.get_mut(runtime)else{return Vec::new()};let mut records=std::mem::take(&mut q.interrupts);records.append(&mut q.asides);records}
 pub fn take_inbox(&self,runtime:&RuntimeKey,from:Option<&str>,limit:usize)->Vec<PeerMessage>{let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());let Some(q)=state.get_mut(runtime)else{return Vec::new()};let mut messages=Vec::new();for queue in [&mut q.interrupts,&mut q.asides]{let mut remaining=Vec::new();for record in std::mem::take(queue){if messages.len()<limit&&from.is_none_or(|f|record.message.from==f){messages.push(record.message)}else{remaining.push(record)}}*queue=remaining;}messages}
}
/// Exact harness-tags.ts grammar, including name-boundary checks and case folding.
/// Only '<' for trusted harness names changes; raw message and task-result stay intact.
pub fn escape_harness_tags(text:&str)->String {
 static TAG:OnceLock<regex::Regex>=OnceLock::new();let re=TAG.get_or_init(||regex::Regex::new(r"(?i)<\s*/?\s*(?:irc|system-[a-z][a-z-]*)").unwrap());
 let mut out=String::new();let mut end=0;for m in re.find_iter(text){let next=text[m.end()..].chars().next();if next.is_some_and(|c|c.is_ascii_alphanumeric()||c=='_'||c=='-'){continue;}out.push_str(&text[end..m.start()]);out.push_str("&lt;");out.push_str(&text[m.start()+1..m.end()]);end=m.end();}out.push_str(&text[end..]);out
}
pub fn render_incoming(message:&PeerMessage,streaming:bool,relay_on_stop:bool,from_parent:bool)->String {
 let body=escape_harness_tags(&message.body);
 let template=if streaming&&from_parent{include_str!("prompts/parent-irc.md")}else{include_str!("prompts/irc-incoming.md")};
 ara_prompt::render(template,&serde_json::json!({"from":message.from,"message":body,"replyTo":message.reply_to.as_deref().unwrap_or(""),"interrupting":streaming,"relayOnStop":relay_on_stop})).unwrap_or_else(|_|format!("<irc from={:?}>\n{body}\n</irc>",message.from))
}

/// Untrusted original evidence is kept verbatim, independently of the trusted rendered envelope.
pub fn incoming_details(message:&PeerMessage,from_parent:bool,interrupting:bool,relay_on_stop:bool)->serde_json::Value{serde_json::json!({"id":message.id,"from":message.from,"to":message.to,"message":message.body,"ts":message.ts,"replyTo":message.reply_to,"wakeRelay":message.wake_relay,"fromParent":from_parent,"interrupting":interrupting,"relayOnStop":relay_on_stop})}
pub fn incoming_batch_details(messages:&[PeerMessage],parent_id:Option<&str>,relay_on_stop:bool)->serde_json::Value{serde_json::json!({"messages":messages.iter().map(|m|incoming_details(m,parent_id==Some(m.from.as_str()),false,relay_on_stop&&!m.wake_relay)).collect::<Vec<_>>()})}
