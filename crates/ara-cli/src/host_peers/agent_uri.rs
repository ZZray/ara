//! Actual agent:// read/progress/artifact/JSON and verbatim coordination WRITE.
//! The same selected route's policy is rechecked by the native tool admission owner.
use super::{PeerCoordinator,PeerKind,PeerRecord,PeerStatus};
use ara_agent::{ToolError,ToolOutput};
use ara_tools::{ContentUriPort,ContentUriRoute,ContentUriWritePolicy,ContentUriWritePayload,ContentUriWriteScope,ContentUriWriteTier,UriResource,UriFileResource};
use async_trait::async_trait;
use std::{collections::{HashMap,HashSet},path::PathBuf,sync::Arc};
use tokio_util::sync::CancellationToken;
/// Host-owned live settings/depth/explicit-enable gate. No credentials or grants.
pub type MessagingEligibility=Arc<dyn Fn()->Result<bool,String>+Send+Sync>;
pub struct AgentUriPort {eligibility:MessagingEligibility,history:Arc<super::history_uri::HistoryIndexPort>,pub coordinator:Arc<PeerCoordinator>,pub root:String,pub sender_id:String,pub session_file:Option<PathBuf>,sender_key:Option<super::PeerKey>}
impl AgentUriPort {
 pub fn new(coordinator:Arc<PeerCoordinator>,root:impl Into<String>,sender_id:impl Into<String>,session_file:Option<PathBuf>)->Arc<Self>{Self::with_eligibility(coordinator,root,sender_id,session_file,Arc::new(||Ok(true)))}
 pub fn with_eligibility(coordinator:Arc<PeerCoordinator>,root:impl Into<String>,sender_id:impl Into<String>,session_file:Option<PathBuf>,eligibility:MessagingEligibility)->Arc<Self>{let root=root.into();let sender_id=sender_id.into();let sender_key=coordinator.registry.get(&root,&sender_id).map(|r|r.key);let history=super::history_uri::HistoryIndexPort::new(coordinator.clone(),root.clone(),session_file.clone());Arc::new(Self{coordinator,root,sender_id,session_file,sender_key,eligibility,history})}
 pub fn prompt_doc()->&'static str{include_str!("prompts/agent.md")}
 async fn dirs(&self)->Result<Vec<PathBuf>,ToolError>{let mut dirs=Vec::new();if let Some(file)=&self.session_file{let root=self.coordinator.roster.ensure(&self.root,file).await.map_err(error)?;dirs.push(root.with_extension(""));}for record in self.coordinator.registry.list(&self.root){if let Some(file)=record.session_file{let canonical=super::roster::artifact_paths(&file,&record.key.id);if let Some(parent)=canonical.markdown.parent(){let dir=parent.to_path_buf();if !dirs.contains(&dir){dirs.push(dir);}}let dir=file.with_extension("");if !dirs.contains(&dir){dirs.push(dir);}}}Ok(dirs)}
 async fn scan(&self,id:&str)->Result<OutputScan,ToolError>{let mut paths=HashMap::new();let mut sidecars=HashMap::new();let mut available=Vec::new();let mut any=false;for dir in self.dirs().await?{let mut entries=match tokio::fs::read_dir(dir).await{Ok(entries)=>entries,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>continue,Err(e)=>return Err(error(e))};any=true;while let Some(entry)=entries.next_entry().await.map_err(error)?{let name=entry.file_name().to_string_lossy().into_owned();if let Some(stem)=name.strip_suffix(".md"){if !paths.contains_key(stem){available.push(stem.to_owned());paths.insert(stem.to_owned(),entry.path());}}else if let Some(stem)=name.strip_suffix(".json"){sidecars.entry(stem.to_owned()).or_insert_with(||entry.path());}}}
  let found=paths.get(id).cloned();let sidecar=sidecars.get(id).filter(|p|p.parent()==found.as_ref().and_then(|f|f.parent())).cloned();Ok(OutputScan{found,sidecar,any,available})
 }
 async fn superseded(&self,id:&str)->bool{let Some(record)=self.coordinator.registry.get(&self.root,id).filter(|r|r.kind!=PeerKind::Advisor&&r.status==PeerStatus::Running)else{return false};let Some(runtime)=record.runtime else{return false};self.coordinator.native.inspect(&runtime).await.is_ok_and(|s|s.running&&!s.disposed)}
 async fn progress(&self,url:&str,record:PeerRecord,extraction:bool)->Result<UriResource,ToolError>{
  if extraction{return Err(error(format!("Output {} is not published yet (status: {:?}); read agent://{} for its progress.",record.key.id,record.status,record.key.id)));}
  let messages=self.coordinator.native.transcript(&record).await.map_err(error)?;let mut sections=Vec::new();let mut last_text=None;
  for row in &messages {let message=row.get("message").filter(|v|v.is_object()).unwrap_or(row);if message["role"]=="toolResult"&&message["toolName"]=="yield"&&message["isError"]!=true&&message["details"]["status"]=="success" {
   let details=&message["details"];if let Some(data)=details.get("data"){let labels=if let Some(labels)=details["type"].as_array(){labels.iter().filter_map(|s|s.as_str()).collect::<Vec<_>>().join(", ")}else{details["type"].as_str().unwrap_or("").into()};sections.push(serde_json::json!({"labels":labels,"data":serde_json::to_string_pretty(data).map_err(error)?}));}
  }
  if message["role"]=="assistant"{let text=message["content"].as_array().map(|blocks|blocks.iter().filter(|b|b["type"]=="text").filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n")).or_else(||message["content"].as_str().map(str::to_owned)).unwrap_or_default();if !text.trim().is_empty(){last_text=Some(text.trim().to_owned());}}}
  let empty=sections.is_empty()&&last_text.is_none();let status=serde_json::to_value(record.status).map_err(error)?.as_str().unwrap_or("unknown").to_owned();
  let content=ara_prompt::render(include_str!("prompts/agent-url-progress.md"),&serde_json::json!({"id":record.key.id,"status":status,"sections":sections,"lastText":last_text,"empty":empty})).map_err(error)?+"\n";
  Ok(UriResource{content,content_type:"text/markdown".into(),notes:vec![format!("No published output; progress from {} ({status}); {url}",if record.runtime.is_some(){"live session"}else if record.session_file.is_some(){"session file (read-only)"}else{"no transcript"})],immutable:true})
 }
 pub async fn complete(&self)->Result<Vec<String>,ToolError>{let mut ids=HashSet::new();for dir in self.dirs().await?{let mut entries=match tokio::fs::read_dir(dir).await{Ok(entries)=>entries,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>continue,Err(e)=>return Err(error(e))};while let Some(entry)=entries.next_entry().await.map_err(error)?{if let Some(id)=entry.file_name().to_string_lossy().strip_suffix(".md"){ids.insert(id.to_owned());}}}let mut ids=ids.into_iter().collect::<Vec<_>>();ids.sort();Ok(ids)}
}
struct OutputScan {found:Option<PathBuf>,sidecar:Option<PathBuf>,any:bool,available:Vec<String>}
fn error(value:impl ToString)->ToolError{ToolError(value.to_string())}
fn parse(url:&str)->Result<(String,Vec<String>),ToolError>{let Some((scheme,rest))=url.split_once("://")else{return Err(error("Expected agent://<id>"))};if !scheme.eq_ignore_ascii_case("agent"){return Err(error("Expected agent:// URI"));}let(id,path)=rest.split_once('/').unwrap_or((rest,""));if id.is_empty(){return Err(error("agent:// URL requires an output ID: agent://<id>"));}let segments=path.split('/').filter(|s|!s.is_empty()).map(decode).collect();Ok((id.into(),segments))}
fn decode(segment:&str)->String{let bytes=segment.as_bytes();let mut out=Vec::new();let mut index=0;while index<bytes.len(){if bytes[index]==b'%'&&index+2<bytes.len(){if let(Ok(a),Ok(b))=(char::from(bytes[index+1]).to_digit(16).ok_or(()),char::from(bytes[index+2]).to_digit(16).ok_or(())){out.push((a*16+b)as u8);index+=3;continue;}}out.push(bytes[index]);index+=1;}String::from_utf8(out).unwrap_or_else(|_|segment.into())}
fn extract(mut value:&serde_json::Value,path:&[String])->serde_json::Value{for segment in path{value=if let Some(array)=value.as_array(){if !segment.is_empty()&&segment.bytes().all(|c|c.is_ascii_digit()){segment.parse::<usize>().ok().and_then(|i|array.get(i)).unwrap_or(&serde_json::Value::Null)}else{&serde_json::Value::Null}}else if value.is_object(){value.get(segment).unwrap_or(&serde_json::Value::Null)}else{&serde_json::Value::Null};}value.clone()}
#[async_trait]
impl ContentUriPort for AgentUriPort {
 fn route(&self,url:&str)->ContentUriRoute{if !matches!(self.history.route(url),ContentUriRoute::Unregistered){return self.history.route(url);}if url.split_once("://").is_some_and(|(s,_)|s.eq_ignore_ascii_case("agent")){ContentUriRoute::Registered{writable:true}}else{ContentUriRoute::Unregistered}}
 fn write_policy(&self,url:&str)->ContentUriWritePolicy{if matches!(self.route(url),ContentUriRoute::Registered{writable:true}){ContentUriWritePolicy{payload:ContentUriWritePayload::Verbatim,scope:ContentUriWriteScope::Coordination,tier:ContentUriWriteTier::Read}}else{ContentUriWritePolicy::default()}}
 async fn read_file(&self,url:&str,cancel:CancellationToken)->Result<Option<UriFileResource>,ToolError>{if !matches!(self.history.route(url),ContentUriRoute::Unregistered){return self.history.read_file(url,cancel).await;}if cancel.is_cancelled(){return Err(error("agent read cancelled"));}let(id,path)=parse(url)?;if id=="all"||!path.is_empty()||self.superseded(&id).await{return Ok(None);}Ok(self.scan(&id).await?.found.map(|path|UriFileResource{path,content_type:"text/markdown".into(),max_inline_bytes:None,immutable:true}))}
 async fn read(&self,url:&str,cancel:CancellationToken)->Result<UriResource,ToolError>{
  if !matches!(self.history.route(url),ContentUriRoute::Unregistered){return self.history.read(url,cancel).await;}
  if cancel.is_cancelled(){return Err(error("agent read cancelled"));}let(id,path)=parse(url)?;if id=="all"{return Err(error("agent://all is write-only; use it to broadcast a message."));}
  let scan=self.scan(&id).await?;let Some(file)=scan.found else{
   if let Some(record)=self.coordinator.registry.get(&self.root,&id).filter(|r|r.kind!=PeerKind::Advisor){return self.progress(url,record,!path.is_empty()).await;}
   if !scan.any{return Err(error("No artifacts directory found"));}let mut candidates=scan.available;for r in self.coordinator.registry.list(&self.root){if r.kind!=PeerKind::Advisor{candidates.push(r.key.id);}}
   return Err(not_found_error(&id,candidates));
  };
  let raw=tokio::fs::read_to_string(&file).await.map_err(error)?;let mut notes=Vec::new();let(content,content_type)=if path.is_empty(){if self.superseded(&id).await{let age=tokio::fs::metadata(&file).await.ok().and_then(|m|m.modified().ok()).and_then(|t|t.elapsed().ok()).map(|d|format!("{}s",d.as_secs())).unwrap_or_else(||"unknown".into());notes.push(format!("Superseded: {id} is running a newer turn"));let banner=ara_prompt::render(include_str!("prompts/agent-url-superseded.md"),&serde_json::json!({"id":id,"age":age})).map_err(error)?;(format!("{banner}{raw}"),"text/markdown")}else{(raw,"text/markdown")}}else{
   let sidecar=if let Some(path)=scan.sidecar{tokio::fs::read_to_string(path).await.ok().and_then(|s|serde_json::from_str::<serde_json::Value>(&s).ok())}else{None};let value=match sidecar{Some(value)=>value,None=>serde_json::from_str(&raw).map_err(|e|error(format!("Output {id} is not valid JSON: {e}")))?};let value=extract(&value,&path);notes.push(format!("Extracted: /{}",path.join("/")));if let Some(text)=value.as_str(){(text.into(),"text/markdown")}else{(serde_json::to_string_pretty(&value).map_err(error)?,"application/json")}
  };Ok(UriResource{content,content_type:content_type.into(),notes,immutable:true})
 }
 async fn write(&self,url:&str,content:&str,cancel:CancellationToken)->Result<(),ToolError>{self.write_result(url,content,cancel).await.map(|_|())}
 async fn write_result(&self,url:&str,content:&str,cancel:CancellationToken)->Result<Option<ToolOutput>,ToolError>{
  if !matches!(self.history.route(url),ContentUriRoute::Unregistered){return self.history.write_result(url,content,cancel).await;}
  if cancel.is_cancelled(){return Err(error("agent message not dispatched: cancelled"));}
  if !(self.eligibility)().map_err(error)?{return Err(error("Peer messaging is unavailable in this session."));}
  let(to,path)=parse(url)?;if !path.is_empty(){return Err(error("agent:// message target cannot have a JSON-path suffix."));}
  // Host exposure/current capability is checked at real admission+dispatch; this verifies sender ref too.
  let sender=self.coordinator.registry.get(&self.root,&self.sender_id).ok_or_else(||error("Peer messaging is unavailable in this session."))?;if self.sender_key.as_ref()!=Some(&sender.key)||sender.status==PeerStatus::Aborted||sender.kind==PeerKind::Advisor{return Err(error("Peer messaging is unavailable in this session."));}
  if let Some(file)=&self.session_file{if to!="all"&&self.coordinator.registry.get(&self.root,&to).is_none_or(|r|r.status==PeerStatus::Parked){self.coordinator.roster.ensure(&self.root,file).await.map_err(error)?;}}
  let result=self.coordinator.send(&self.root,&self.sender_id,&to,content,None,false).await.map_err(error)?;let mut output=if result.is_error{ToolOutput::error(result.text.clone())}else{ToolOutput::text(result.text.clone())};output.details=Some(serde_json::json!({"message":result}));Ok(Some(output))
 }
}
// Fixed agent-protocol.ts notFoundError: first-occurrence exact dedup,
// raw exact-ID exclusion, score-only stable ranking, then the five-item cap.
pub(super) fn not_found_error(id:&str,candidates:impl IntoIterator<Item=String>)->ToolError {
 let mut seen=HashSet::new();let unique=candidates.into_iter().filter(|candidate|seen.insert(candidate.clone())&&candidate.as_str()!=id).collect::<Vec<_>>();
 let suggestions=crate::model_roles::fuzzy_filter_texts(&unique,id).into_iter().take(5).collect::<Vec<_>>();
 error(format!("Not found: {id}\n{}",if suggestions.is_empty(){"List agents with history://".into()}else{format!("Did you mean: {}",suggestions.join(", "))}))
}
