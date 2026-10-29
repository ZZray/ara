//! Root-aware persisted discovery and bounded settled latches; fixed persisted-agents.ts.
use super::{PeerRegistry,PeerRegistration,PeerKind,PeerStatus,PeerKey};
use std::{collections::{HashMap,VecDeque},path::{Path,PathBuf},sync::{Arc,Mutex},time::UNIX_EPOCH};
use tokio::{io::{AsyncBufReadExt,BufReader},sync::watch};
struct Latch {generation:u64,settled:bool,owned:Vec<(PeerKey,PathBuf)>,done:watch::Receiver<Option<Result<(),String>>>}
#[derive(Default)]struct State {generation:u64,latches:HashMap<(String,PathBuf),Latch>,order:VecDeque<(String,PathBuf)>}
#[derive(Clone,Debug)]pub struct PeerArtifactPaths {pub markdown:PathBuf,pub json:PathBuf,pub patch:PathBuf}
/// The session stem remains the traversal key; outputs are indexed by actual journal ID.
pub fn artifact_paths(file:&Path,id:&str)->PeerArtifactPaths{let directory=file.parent().unwrap_or_else(||Path::new("."));PeerArtifactPaths{markdown:directory.join(format!("{id}.md")),json:directory.join(format!("{id}.json")),patch:directory.join(format!("{id}.patch"))}}
pub struct PeerRoster {registry:Arc<PeerRegistry>,state:Mutex<State>,serial:tokio::sync::Mutex<()>}
impl PeerRoster {
 pub fn new(registry:Arc<PeerRegistry>)->Self{Self{registry,state:Mutex::new(State::default()),serial:tokio::sync::Mutex::new(())}}
 pub fn latch_count(&self)->usize{self.state.lock().unwrap_or_else(|e|e.into_inner()).latches.len()}
 pub async fn resolve_root(hint:&Path)->anyhow::Result<PathBuf>{
  anyhow::ensure!(hint.extension().is_some_and(|x|x=="jsonl"),"Peer root hint is not a JSONL session");
  let mut current=if hint.is_absolute(){hint.to_path_buf()}else{std::env::current_dir()?.join(hint)};
  // Existing files canonicalize once; no fixed depth ceiling and no followed directory symlinks.
  if tokio::fs::try_exists(&current).await?{current=tokio::fs::canonicalize(&current).await?;}
  loop {let Some(parent)=current.parent()else{return Ok(current)};let candidate=PathBuf::from(format!("{}.jsonl",parent.display()));if candidate==current||!tokio::fs::try_exists(&candidate).await?{return Ok(current);}current=candidate;}
 }
 pub fn belongs_to_root(file:&Path,root:&Path)->bool{file==root||file.starts_with(root.with_extension(""))}
 pub async fn ensure(self:&Arc<Self>,namespace:&str,hint:&Path)->anyhow::Result<PathBuf>{
  let root=Self::resolve_root(hint).await?;let slot=(namespace.to_owned(),root.clone());
  let(mut done,launch)={let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
   let stale=state.latches.get(&slot).is_some_and(|l|l.settled&&l.owned.iter().any(|(key,file)|self.registry.get_exact(key).is_none_or(|r|r.session_file.as_ref()!=Some(file))));if stale{state.latches.remove(&slot);state.order.retain(|key|key!=&slot);}
   if let Some(latch)=state.latches.get(&slot){(latch.done.clone(),None)}else{
    while state.latches.len()>=32 {let evict=state.order.iter().find(|key|state.latches.get(*key).is_some_and(|l|l.settled)).cloned();let Some(evict)=evict else{break};state.order.retain(|key|key!=&evict);state.latches.remove(&evict);}
    state.generation=state.generation.checked_add(1).expect("Roster latch generation exhausted");let generation=state.generation;let(tx,rx)=watch::channel(None);state.order.push_back(slot.clone());state.latches.insert(slot.clone(),Latch{generation,settled:false,owned:Vec::new(),done:rx.clone()});(rx,Some((tx,generation)))
   }
  };
  if let Some((finished,generation))=launch{let this=self.clone();let slot=slot.clone();tokio::spawn(async move{
   let result={let _serial=this.serial.lock().await;this.scan(&slot.0,&slot.1).await};
   let receipt=result.as_ref().map(|_|()).map_err(ToString::to_string);
   {let mut state=this.state.lock().unwrap_or_else(|e|e.into_inner());if state.latches.get(&slot).is_some_and(|l|l.generation==generation){match result {Ok(owned)=>{let latch=state.latches.get_mut(&slot).unwrap();latch.settled=true;latch.owned=owned;},Err(_)=>{state.latches.remove(&slot);state.order.retain(|key|key!=&slot);}}}}
   let _=finished.send(Some(receipt));
  });}
  loop{if let Some(receipt)=done.borrow().clone(){let _scan_error=receipt.err();return Ok(root);}done.changed().await.map_err(|_|anyhow::anyhow!("Roster scan owner ended without settlement"))?;}
 }
 async fn scan(&self,namespace:&str,root:&Path)->anyhow::Result<Vec<(PeerKey,PathBuf)>>{
  let root_meta=read_metadata(root).await?;let owner=root_meta.id.ok_or_else(||anyhow::anyhow!("Root journal has no authoritative header id"))?;
  let artifact_root=root.with_extension("");let mut stack=vec![(artifact_root,owner,root.to_path_buf())];let mut owned=Vec::new();
  while let Some((dir,owner,owner_file))=stack.pop(){
   let mut entries=match tokio::fs::read_dir(&dir).await{Ok(entries)=>entries,Err(e)if matches!(e.kind(),std::io::ErrorKind::NotFound|std::io::ErrorKind::NotADirectory)=>continue,Err(e)=>return Err(e.into())};
   let mut paths=Vec::new();let mut patches=Vec::new();let mut directories=HashMap::new();while let Some(entry)=entries.next_entry().await?{let kind=entry.file_type().await?;let name=entry.file_name().to_string_lossy().into_owned();if kind.is_dir(){directories.insert(name,entry.path());}else if kind.is_file()&&name.ends_with(".jsonl")&&!name.contains(".bak"){paths.push((name,entry.path()));}else if kind.is_file()&&name.ends_with(".patch"){patches.push((name,entry.path()));}}
   paths.sort_by(|a,b|a.0.cmp(&b.0));
   for(name,file)in paths {
    let stem=name.trim_end_matches(".jsonl");if stem=="."||stem==".."{continue;}
    let advisor=name=="__advisor.jsonl"||name.starts_with("__advisor.");let mut metadata=read_metadata(&file).await?;
    let id=if advisor{let slug=stem.strip_prefix("__advisor.").unwrap_or("");if slug.is_empty(){format!("{owner}/advisor")}else{format!("{owner}/advisor:{slug}")}}else{
     let Some(id)=metadata.id.clone().filter(|id|!id.is_empty()&&id!="."&&id!=".."&&!id.contains(['/', '\\']))else{continue;};
     if metadata.parent_id.as_deref().is_some_and(|parent|parent!=owner){continue;}
     let Some(parent_file)=metadata.parent_session.as_ref()else{continue;};
     // Unknown I/O does not prove another parent. Propagate it; never compare two failed probes.
     if tokio::fs::canonicalize(parent_file).await?!=tokio::fs::canonicalize(&owner_file).await?{continue;}
     id
    };
    let expected=self.registry.get(namespace,&id);
    // Existing live, aborted, or currently owned generations are never replaced by a disk scan.
    if let Some(existing)=&expected {if existing.session_file.as_ref()==Some(&file){owned.push((existing.key.clone(),file.clone()));}else if existing.runtime.is_some()||existing.status==PeerStatus::Aborted||(!advisor&&existing.status!=PeerStatus::Parked){if let Some(child)=directories.remove(stem){stack.push((child,id.clone(),file.clone()));}continue;}if advisor&&existing.kind!=PeerKind::Advisor{continue;}}
    let tombstone=tokio::fs::try_exists(format!("{}.tombstone",file.display())).await?;
    let prefix=format!("{stem}.nested-");let mut nested=patches.iter().filter(|(name,_)|name.starts_with(&prefix)).collect::<Vec<_>>();nested.sort_by_key(|(name,_)|name[prefix.len()..].split('-').next().and_then(|n|n.parse::<u64>().ok()).unwrap_or(0));if !nested.is_empty(){metadata.history["nestedPatchPaths"]=serde_json::json!(nested.iter().map(|(_,path)|path.to_string_lossy().into_owned()).collect::<Vec<_>>());}
    if (!metadata.incomplete||tombstone)&&expected.as_ref().is_none_or(|r|r.session_file.as_ref()!=Some(&file)){
     let mut input=PeerRegistration::child(namespace,&id,owner.clone(),file.clone());input.kind=if advisor{PeerKind::Advisor}else{PeerKind::Sub};input.status=if tombstone{PeerStatus::Aborted}else{PeerStatus::Parked};input.created_at=metadata.created_at;input.last_activity=metadata.last_activity;input.history=metadata.history;
     input.history["transcriptStem"]=stem.into();
     let artifacts=artifact_paths(&file,&id);for(path,key)in [(artifacts.markdown,"outputPath"),(artifacts.patch,"patchPath")]{if tokio::fs::try_exists(&path).await?{input.history[key]=path.to_string_lossy().into_owned().into();}}
     if advisor{input.history["readOnly"]=true.into();}
     let still_valid=match &expected {None=>self.registry.get(namespace,&id).is_none(),Some(old)=>self.registry.get_exact(&old.key).is_some_and(|r|r.runtime.is_none()&&r.status==PeerStatus::Parked)};
     if still_valid {let vacated=expected.as_ref().is_none_or(|old|self.registry.unregister_dead(&old.key));if vacated{if let Some(record)=self.registry.register_if_available(input,None){owned.push((record.key,file.clone()));}}}
    }
    if let Some(child)=directories.remove(stem){stack.push((child,id,file.clone()));}
    tokio::task::yield_now().await;
   }
  }Ok(owned)
 }
}
struct Metadata {id:Option<String>,parent_session:Option<PathBuf>,parent_id:Option<String>,incomplete:bool,created_at:Option<u64>,last_activity:Option<u64>,history:serde_json::Value}
async fn read_metadata(file:&Path)->anyhow::Result<Metadata>{
 let stat=match tokio::fs::metadata(file).await{Ok(stat)=>Some(stat),Err(e)if e.kind()==std::io::ErrorKind::NotFound=>None,Err(e)=>return Err(e.into())};
 let mut meta=Metadata{id:None,parent_session:None,parent_id:None,incomplete:true,created_at:None,last_activity:stat.as_ref().and_then(|s|s.modified().ok()).and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_millis()as u64),history:serde_json::json!({})};
 let file_handle=match tokio::fs::File::open(file).await{Ok(file)=>file,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(meta),Err(e)=>return Err(e.into())};let mut lines=BufReader::new(file_handle).lines();
 for _ in 0..64{let Some(line)=lines.next_line().await?else{break};let Ok(row)=serde_json::from_str::<serde_json::Value>(&line)else{continue};
  if let Some(ts)=row.get("timestamp").and_then(|v|v.as_str()).and_then(|v|chrono::DateTime::parse_from_rfc3339(v).ok()){meta.created_at=Some(meta.created_at.unwrap_or(ts.timestamp_millis().max(0)as u64));}
  if row["type"]=="session"{meta.id=row["id"].as_str().map(str::to_owned);meta.parent_session=row["parentSession"].as_str().map(PathBuf::from);}
  match row.get("type").and_then(|v|v.as_str()){Some("message"|"custom_message")=>meta.incomplete=false,Some("model_change")=>{for(from,to)in [("model","resolvedModel"),("role","modelRole"),("resolvedModelIsFallback","resolvedModelIsFallback")]{if let Some(v)=row.get(from){if from!="role"||!v.as_str().is_some_and(role_ephemeral){meta.history[to]=v.clone();}}}},Some("session_init")=>{meta.parent_id=row["araChildParentId"].as_str().map(str::to_owned);meta.incomplete=false;for key in ["agent","modelRole","resolvedModel","readOnly"]{if let Some(v)=row.get(key){meta.history[key]=v.clone();}}},_=>{}}
 }
 if meta.created_at.is_none(){meta.created_at=stat.as_ref().and_then(|s|s.created().ok()).and_then(|t|t.duration_since(UNIX_EPOCH).ok()).map(|d|d.as_millis()as u64);}

 Ok(meta)
}

impl PeerRoster {
 /// Explicit history hydration is separate from the cheap lazy roster scan, as upstream.
 pub async fn hydrate_history(&self,key:&PeerKey)->anyhow::Result<()> {
  let record=self.registry.get_exact(key).ok_or_else(||anyhow::anyhow!("History ref changed"))?;let Some(file)=record.session_file else{return Ok(())};
  let rows=read_rows(&file,if record.kind==PeerKind::Advisor{Some(200_000)}else{None}).await?;
  let history=history_summary(&rows,record.created_at,record.last_activity);
  self.registry.set_history(key,history,Some(&file));Ok(())
 }
 pub async fn register_persisted(&self,namespace:&str,root:&Path,hydrate:bool)->anyhow::Result<()> {
  let owned=self.scan(namespace,root).await?;if hydrate{for(key,_)in owned{self.hydrate_history(&key).await?;}}Ok(())
 }
}
async fn read_rows(file:&Path,limit:Option<usize>)->anyhow::Result<Vec<serde_json::Value>>{
 let input=match tokio::fs::File::open(file).await{Ok(input)=>input,Err(e)if e.kind()==std::io::ErrorKind::NotFound=>return Ok(Vec::new()),Err(e)=>return Err(e.into())};
 let mut lines=BufReader::new(input).lines();let mut rows=Vec::new();let mut count=0usize;while let Some(line)=lines.next_line().await?{if limit.is_some_and(|limit|count>=limit){break;}if let Ok(value)=serde_json::from_str(&line){rows.push(value);count+=1;}}Ok(rows)
}
fn number(value:&serde_json::Value)->f64{value.as_f64().filter(|n|n.is_finite()).unwrap_or(0.0)}
fn role_ephemeral(role:&str)->bool{role=="fallback"}
fn produced_output(message:&serde_json::Value)->bool {
 let stop=message["stopReason"].as_str().unwrap_or("");if matches!(stop,"error"|"aborted"){return false;}
 let Some(content)=message["content"].as_array()else{return false};let actionable=|block:&serde_json::Value|match block["type"].as_str(){Some("toolCall"|"image"|"redactedThinking"|"anthropicServerTool")=>true,Some("text")=>block["text"].as_str().is_some_and(|t|!t.trim().is_empty()),Some("thinking")=>block["thinkingSignature"].as_str().is_some_and(|t|!t.trim().is_empty()),_=>false};
 if stop=="toolUse"&&!content.iter().any(|b|b["type"]=="toolCall"||(b["type"]=="text"&&b["text"].as_str().is_some_and(|s|!s.trim().is_empty()))){return false;}content.iter().any(actionable)
}
/// Leaf selection and serving-model attribution based on fixed persisted-agents.ts147-298; candidate execution remains pending.
fn history_summary(rows:&[serde_json::Value],created:u64,last:u64)->serde_json::Value{
 use serde_json::{Value,json};let mut parents=HashMap::<String,Option<String>>::new();let mut assistants=HashMap::<String,Value>::new();let mut changes=HashMap::<String,Value>::new();let mut leaf=None::<String>;let mut selected=None::<String>;let mut leaf_time=None;let mut direct_cost=0.0;
 for row in rows{let Some(id)=row["id"].as_str()else{continue};parents.insert(id.into(),row["parentId"].as_str().map(str::to_owned));leaf=Some(id.into());if let Some(time)=row["timestamp"].as_str().and_then(|t|chrono::DateTime::parse_from_rfc3339(t).ok()){leaf_time=Some(time.timestamp_millis().max(0)as u64);}
  if row["type"]=="model_usage"{direct_cost+=number(&row["usage"]["cost"]["total"]);if let Some(active)=row.get("activeLeafId"){leaf=active.as_str().map(str::to_owned);}else{selected=Some(id.into());}continue;}
  selected=Some(id.into());if row["type"]=="model_change"&&row["model"].is_string(){changes.insert(id.into(),row.clone());}
  if row["type"]=="message"&&row["message"]["role"]=="assistant"{let message=&row["message"];direct_cost+=number(&message["usage"]["cost"]["total"]);assistants.insert(id.into(),message.clone());}
 }
 if leaf.as_ref().is_some_and(|id|!parents.contains_key(id)){leaf=selected;}
 let mut metrics=json!({"tokens":0.0,"requests":0,"tools":0,"cost":0.0,"durationMs":leaf_time.unwrap_or(last).saturating_sub(created),"durationKind":"span"});let mut served=None::<String>;let mut model=None::<String>;let mut fallback=None::<bool>;let mut role=None::<String>;let mut latest=None::<Value>;let mut context=None::<f64>;let mut visited=std::collections::HashSet::new();let mut cursor=leaf;
 while let Some(id)=cursor {if !visited.insert(id.clone()){break;}if let Some(change)=changes.get(&id){if latest.is_none(){latest=Some(change.clone());}if let Some(r)=change["role"].as_str().filter(|r|!role_ephemeral(r)){if role.is_none(){role=Some(r.into());}}
   if let Some(served)=&served{let candidate=change["model"].as_str().unwrap_or("");if model.is_none()&&(candidate==served||candidate.starts_with(&format!("{served}@"))){model=Some(candidate.into());fallback=Some(change["resolvedModelIsFallback"]==true);}}
  }
  if let Some(message)=assistants.get(&id){let usage=&message["usage"];let total=number(&usage["input"])+number(&usage["output"])+number(&usage["cacheWrite"]);let tokens=if total>0.0{total}else{number(&usage["totalTokens"])};
   metrics["requests"]=json!(metrics["requests"].as_u64().unwrap_or(0)+1);metrics["tokens"]=json!(number(&metrics["tokens"])+tokens);metrics["cost"]=json!(number(&metrics["cost"])+number(&usage["cost"]["total"]));metrics["tools"]=json!(metrics["tools"].as_u64().unwrap_or(0)+message["content"].as_array().map(|blocks|blocks.iter().filter(|b|b["type"]=="toolCall").count()as u64).unwrap_or(0));
   if context.is_none(){let amount=number(&usage["totalTokens"]);if amount!=0.0{context=Some(amount);}}
   if served.is_none()&&produced_output(message){if let(Some(provider),Some(model))=(message["provider"].as_str(),message["model"].as_str()){served=Some(format!("{provider}/{model}"));}}
  }
  cursor=parents.get(&id).cloned().flatten();
 }
 if model.is_none(){model=served.clone().or_else(||latest.as_ref().and_then(|c|c["model"].as_str().map(str::to_owned)));fallback=if served.is_some(){Some(false)}else{latest.as_ref().map(|c|c["resolvedModelIsFallback"]==true)};}
 if let Some(context)=context{metrics["contextTokens"]=json!(context);}let mut history=json!({"directCost":direct_cost});if metrics["requests"].as_u64().unwrap_or(0)>0{history["metrics"]=metrics;}if let Some(model)=model{history["resolvedModel"]=model.into();if let Some(fallback)=fallback{history["resolvedModelIsFallback"]=fallback.into();}}if let Some(role)=role{history["modelRole"]=role.into();}history
}
