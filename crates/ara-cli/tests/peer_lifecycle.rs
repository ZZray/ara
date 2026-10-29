//! Five coarse finite-Peer banks. Ordinary binary, Native owners/journals, controlled loopback upstream.
#![cfg(feature="test-fixture")]
use ara_testkit::{FakeUpstream,Script,chunks::{text,tool_call,finish,done}};
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}},time::Duration};
use tokio_util::sync::CancellationToken;
#[path="support/broker_consumers.rs"] mod cold_broker;
#[path="support/peer_main_relay.rs"] mod relay_cases;

struct Bank{root:PathBuf,receipts:Vec<Value>,requests:Vec<Value>,teardown:Value}
fn init(tools:&[&str])->Value{json!({"systemPrompt":["Peer host controlled fixture"],"task":"actual peer turn","tools":tools,"agent":"task","restrictToolNames":true,"readSummarize":false,"spawns":"*","outputSchemaMode":"permissive","retryFallback":{"primary":"default","chain":[]}})}
fn answer(body:&str)->Value{json!({"events":[text(body),finish("stop"),done()]})}
fn yielded(id:&str,data:Value)->Value{json!({"events":[tool_call(0,id,"yield",&json!({"data":data}).to_string()),finish("tool_calls"),done()]})}
/// The proxy holds original upstream bytes at a source-defined frontier, never synthesizes a Native event.
struct PeerHttpGate {url:String,task:tokio::task::JoinHandle<()>}
impl Drop for PeerHttpGate{fn drop(&mut self){self.task.abort();}}
fn actual_events(path:&Path)->Vec<Value>{std::fs::read_to_string(path).unwrap_or_default().split_inclusive('\n').filter(|s|s.ends_with('\n')).map(|s|serde_json::from_str(s).expect("actual event JSON framing")).collect()}
impl PeerHttpGate {
 async fn start(target:std::net::SocketAddr,root:PathBuf,mode:&str)->Self{
  use tokio::io::{AsyncReadExt,AsyncWriteExt};
  let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let mode=mode.to_owned();
  let task=tokio::spawn(async move{let mut ordinal=0usize;let mut connections=tokio::task::JoinSet::new();loop{tokio::select!{
   incoming=listener.accept()=>{let(client,_)=incoming.unwrap();ordinal+=1;let index=ordinal;let root=root.clone();let mode=mode.clone();connections.spawn(async move{
    let upstream=tokio::net::TcpStream::connect(target).await.unwrap();let(cr,mut cw)=client.into_split();let(mut ur,uw)=upstream.into_split();let is_cold=mode=="cold-auth";let cold_root=root.clone();
    let forward=tokio::spawn(async move{let mut cr=cr;let mut uw=uw;
     if is_cold{
      let mut bytes=Vec::new();let mut buffer=[0u8;4096];
      loop{let n=cr.read(&mut buffer).await.unwrap();assert!(n>0,"Controlled model request ended before headers");bytes.extend_from_slice(&buffer[..n]);if bytes.windows(4).any(|b|b==b"\r\n\r\n"){break;}assert!(bytes.len()<131072,"Controlled model headers exceeded bound");}
      let headers=String::from_utf8_lossy(&bytes);let bearer=headers.lines().find_map(|line|line.split_once(':').filter(|(name,_)|name.eq_ignore_ascii_case("authorization")).map(|(_,v)|v.trim()));
      let label=match bearer{Some("Bearer cold-synthetic-A")=>"A",Some("Bearer cold-synthetic-B")=>"B",Some("Bearer sk-peer-loopback-controlled-fixture")|Some("Bearer cold-key-K")=>"K",_=>"UNKNOWN"};
      use std::io::Write;let mut file=std::fs::OpenOptions::new().create(true).append(true).open(cold_root.join("cold-model-identities.jsonl")).unwrap();writeln!(file,"{}",json!({"class":"model","ordinal":index,"identity":label})).unwrap();
      uw.write_all(&bytes).await.unwrap();
     }
     let _=tokio::io::copy(&mut cr,&mut uw).await;
    });
    if mode=="relay-target"{
     // Only scripted model response bytes use the dynamically created target.
     // This controlled SSE response has connection-close framing, no length.
     let mut response=Vec::new();ur.read_to_end(&mut response).await.unwrap();let response=String::from_utf8(response).unwrap();
     assert!(!response.to_ascii_lowercase().contains("content-length:"));let target=std::fs::read_to_string(root.join("relay-target-id.txt")).unwrap();let response=response.replace("__RELAY_TO__",&target);cw.write_all(response.as_bytes()).await.unwrap();cw.flush().await.unwrap();
    }else if mode=="cold-auth"{let _=tokio::io::copy(&mut ur,&mut cw).await;}else if index==1||(mode=="relay-race"&&index==3) {
     let mut response=Vec::new();ur.read_to_end(&mut response).await.unwrap();
     let header=response.windows(4).position(|b|b==b"\r\n\r\n").unwrap()+4;
     if mode=="streaming107"||mode=="raw-routes"||mode=="auto-before-wait"{
      let split=header+response[header..].windows(2).position(|b|b==b"\n\n").unwrap()+2;std::fs::write(root.join("stream-prefix-held"),&response[..split]).unwrap();cw.write_all(&response[..split]).await.unwrap();cw.flush().await.unwrap();
      tokio::time::timeout(Duration::from_secs(15),async{while !root.join(if mode=="streaming107"{"stream-parent-steer-ack.json"}else if mode=="auto-before-wait"{"auto-before-wait-ready.json"}else{"raw-routes-ack.json"}).is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("complete tool-call stream never received real ParentSteer ACK");
      if mode=="streaming107"{let ack:Value=serde_json::from_slice(&std::fs::read(root.join("stream-parent-steer-ack.json")).unwrap()).unwrap();assert_eq!(ack["actualParentSteer"]["is_error"],false);assert_eq!(ack["actualParentSteer"]["receipts"][0]["outcome"],"injected");}std::fs::write(root.join("stream-finish-released"),b"after actual ParentSteer ACK").unwrap();cw.write_all(&response[split..]).await.unwrap();
     }else if mode=="relay-race"{
      cw.write_all(&response[..header]).await.unwrap();cw.flush().await.unwrap();std::fs::write(root.join(format!("relay-http-held-{index}")),b"original response body held").unwrap();tokio::time::timeout(Duration::from_secs(15),async{while !root.join("relay-release").is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("actual old relay/new B observation never settled");cw.write_all(&response[header..]).await.unwrap();
     }else if mode=="child-wait"{
      cw.write_all(&response[..header]).await.unwrap();cw.flush().await.unwrap();tokio::time::timeout(Duration::from_secs(15),async{while !root.join("child-wait-terminal.json").is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("actual entered Native Wait interruption never settled");cw.write_all(&response[header..]).await.unwrap();
     }else if mode=="hub-pending-wait"{
      cw.write_all(&response[..header]).await.unwrap();cw.flush().await.unwrap();std::fs::write(root.join("hub-wait-stream-held"),b"original Wait call response held before body").unwrap();
      tokio::time::timeout(Duration::from_secs(15),async{while !root.join("hub-wait-queued-ack.json").is_file(){tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("actual pending message never reached original Native queue");cw.write_all(&response[header..]).await.unwrap();
     }else if mode=="native-wait"{
      cw.write_all(&response[..header]).await.unwrap();cw.flush().await.unwrap();
      tokio::time::timeout(Duration::from_secs(15),async{loop{let events=actual_events(&root.join("root.events.jsonl"));if events.iter().any(|e|e["type"]=="tool_execution_update"&&e["toolCallId"]=="native-wait"&&e["partialResult"]["details"]["op"]=="wait"&&e["partialResult"]["details"]["jobs"].as_array().is_some_and(|jobs|jobs.iter().any(|j|j["kind"]=="task"&&j["status"]=="running"))){assert!(!events.iter().any(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="native-wait"));std::fs::write(root.join("native-wait-watched.json"),serde_json::to_vec_pretty(&events).unwrap()).unwrap();break;}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.expect("actual Native Wait never watched running child");cw.write_all(&response[header..]).await.unwrap();
     }else{panic!("unknown finite gate mode");}let _=cw.flush().await;
    }else{let _=tokio::io::copy(&mut ur,&mut cw).await;}
    forward.abort();
   });},_ = connections.join_next(),if !connections.is_empty()=>{}
  }}});Self{url:format!("http://{address}/v1"),task}
 }
}
async fn run(label:&str,ops:Vec<Value>,responses:Vec<Value>,read_only:bool)->Bank{run_at_project(label,ops,responses,read_only,None).await}
async fn run_at_project(label:&str,ops:Vec<Value>,responses:Vec<Value>,read_only:bool,project_override:Option<PathBuf>)->Bank{
 let base=PathBuf::from(std::env::var_os("ARA_TEST_PEER_ARTIFACT_DIR").expect("Root must bind Peer evidence directory"));let root=base.join(format!("{label}-{}",uuid::Uuid::now_v7()));let home=root.join("home");let project=project_override.unwrap_or_else(||root.join("project"));let agent=home.join("agent");std::fs::create_dir_all(&agent).unwrap();std::fs::create_dir_all(&project).unwrap();std::fs::write(project.join("raw-fixture.txt"),"controlled source body").unwrap();
 std::fs::write(agent.join("config.yml"),"features:\n  unexpectedStopDetection: none\nretry:\n  enabled: false\n  modelFallback: false\nxdev:\n  enabled: false\nedit:\n  mode: hashline\n").unwrap();let scenario=root.join("scenario.json");std::fs::write(&scenario,serde_json::to_vec_pretty(&ops).unwrap()).unwrap();
 let up=FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(),Some(root.join("requests.jsonl"))).await.unwrap();let gate=if label.starts_with("relay-main-"){Some(PeerHttpGate::start(up.addr,root.clone(),"relay-target").await)}else if label.starts_with("hub-pending-wait"){Some(PeerHttpGate::start(up.addr,root.clone(),"hub-pending-wait").await)}else if label.starts_with("cold9-"){Some(PeerHttpGate::start(up.addr,root.clone(),"cold-auth").await)}else if label.starts_with("p4-auto-before-wait-"){Some(PeerHttpGate::start(up.addr,root.clone(),"auto-before-wait").await)}else if label=="p4-relay-race"{Some(PeerHttpGate::start(up.addr,root.clone(),"relay-race").await)}else if label=="p4-streaming107"{Some(PeerHttpGate::start(up.addr,root.clone(),"streaming107").await)}else if label=="p2-running-raw"{Some(PeerHttpGate::start(up.addr,root.clone(),"raw-routes").await)}else if label.starts_with("p4-child-wait-"){Some(PeerHttpGate::start(up.addr,root.clone(),"child-wait").await)}else if label=="p4-native-wait"||label=="p4-wait-ack-unknown"{Some(PeerHttpGate::start(up.addr,root.clone(),"native-wait").await)}else{None};let url=gate.as_ref().map(|g|g.url.clone()).unwrap_or_else(||up.base_url());let cold=label.starts_with("cold9-");
 let broker=if cold{
  use ara_cli::auth_broker_wire::{validate_wire, BrokerWireSchema as Wire, REMOTE_REFRESH_SENTINEL};
  let initial=cold_broker::snapshot(1,vec![
   json!({"id":41,"provider":"cold-fixture","credential":{"type":"oauth","access":"cold-synthetic-A","refresh":REMOTE_REFRESH_SENTINEL,"expires":ara_ai::now_ms()+3_600_000,"accountId":"A","email":"a@cold"},"identityKey":"account:A","rotatesInMs":3600000,"blocks":[]}),
   json!({"id":42,"provider":"cold-fixture","credential":{"type":"oauth","access":"cold-synthetic-B","refresh":REMOTE_REFRESH_SENTINEL,"expires":ara_ai::now_ms()+3_600_000,"accountId":"B","email":"b@cold"},"identityKey":"account:B","rotatesInMs":3600000,"blocks":[]}),
   json!({"id":43,"provider":"cold-fixture","credential":{"type":"api_key","key":"cold-key-K"},"identityKey":null,"rotatesInMs":null,"blocks":[]})]);
  validate_wire(Wire::SnapshotResponse,&initial).expect("Cold broker snapshot must obey the production remote schema");
  // A refresh response uses CredentialSnapshotEntry, without snapshot schedule fields.
  let refreshed=initial["credentials"].as_array().unwrap().iter().filter(|row|row["credential"]["type"]=="oauth").map(|row|{
   let mut entry=row.clone();entry.as_object_mut().unwrap().remove("rotatesInMs");entry.as_object_mut().unwrap().remove("blocks");
   let body=json!({"entry":entry});validate_wire(Wire::CredentialRefreshResponse,&body).unwrap();(row["id"].as_i64().unwrap(),body)
  }).collect::<std::collections::BTreeMap<_,_>>();
  Some(cold_broker::BrokerFixture::start_with_snapshot_stream(move|request|{
   let path=request.path.split('?').next().unwrap();
   match(request.method.as_str(),path){
    ("GET","/v1/snapshot"|"/v1/snapshot/stream")=>(200,initial.clone()),
    ("GET","/v1/usage")=>{
     let body=json!({"generatedAt":ara_ai::now_ms(),"reports":(["A","B"].into_iter().map(|id|json!({"provider":"cold-fixture","fetchedAt":ara_ai::now_ms(),"limits":[{"id":"meter","label":"wire","scope":{"provider":"cold-fixture","accountId":id},"amount":{"unit":"requests"},"window":{"id":"w","label":"W"}}],"metadata":{"accountId":id},"raw":null})).collect::<Vec<_>>())});
     validate_wire(Wire::UsageResponse,&body).unwrap();(200,body)
    },
    ("POST","/v1/usage/observed"|"/v1/usage/stale")=>{
     if path=="/v1/usage/observed"{validate_wire(Wire::ClientUsageReportRequest,&request.body).unwrap();}
     let body=json!({"ok":true});validate_wire(if path=="/v1/usage/observed"{Wire::ClientUsageReportResponse}else{Wire::UsageStaleResponse},&body).unwrap();(200,body)
    },
    ("POST",path) if path.starts_with("/v1/credential/")&&path.ends_with("/refresh")=>{
     let id=path.trim_start_matches("/v1/credential/").trim_end_matches("/refresh").parse::<i64>().ok();
     id.and_then(|id|refreshed.get(&id).cloned()).map(|body|(200,body)).unwrap_or_else(||(404,json!({"error":"unknown synthetic refresh row"})))
    },
    _=>(404,json!({"error":"unexpected Cold broker fixture endpoint"}))
   }
  }).await)
 }else{None};
 if cold{
  let setup=ops.iter().find(|operation|operation["op"]=="cold_setup").expect("Cold9 setup missing");
  let mut settings=json!({"features":{"unexpectedStopDetection":"none"},"retry":{"enabled":false,"modelFallback":false},"xdev":{"enabled":false},"edit":{"mode":"hashline"}});
  if let Some(task)=setup.get("task"){settings["task"]=task.clone();}
  if let Some(roles)=setup.get("modelRoles"){settings["modelRoles"]=roles.clone();}
  if let Some(retry)=setup.get("retry"){settings["retry"]=retry.clone();}
  if let Some(compaction)=setup.get("compaction"){settings["compaction"]=compaction.clone();}
  if let Some(label)=setup.get("displayName"){std::fs::write(root.join("frontiers.json"),json!({"coldDisplayName":label,"gates":[],"probeFaults":[]}).to_string()).unwrap();}
  std::fs::write(agent.join("config.yml"),serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
  // Fixed custom-definition builders omit requestModelId. Supply the actual
  // materialized catalog donor to the ordinary startup Registry instead.
  if setup["materializedCatalog"]==true{
   std::fs::write(root.join("cold-materialized-model.json"),json!({"id":"cold-fixture/catalog-prefixed","name":"Cold catalog donor","provider":"cold-fixture","api":"openai-completions","baseUrl":url,"requestModelId":"wire-cold-override","reasoning":false,"contextWindow":200000,"maxTokens":512,"input":["text"],"cost":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0}}).to_string()).unwrap();
  }
  std::fs::write(if setup["deferModels"]==true{root.join("cold-models.pending.json")}else{agent.join("models.yml")},json!({"providers":{"cold-fixture":{"api":"openai-completions","baseUrl":url,"apiKey":"cold-key-K","models":[{"id":"fake-model","reasoning":true,"contextWindow":200000,"maxTokens":512,"input":["text"]},{"id":"role-model","contextWindow":200000,"maxTokens":512,"input":["text"]},{"id":"fallback-model","contextWindow":200000,"maxTokens":512,"input":["text"]}]},"cold-key-only":{"api":"openai-completions","baseUrl":url,"apiKey":"cold-key-K","models":[{"id":"role-model","contextWindow":200000,"maxTokens":512,"input":["text"]}]}}}).to_string()).unwrap();
 }
 if let Some(setup)=ops.iter().find(|op|op["op"]=="hub_setup"){
  let mut settings:Value=serde_json::from_slice(&std::fs::read(agent.join("config.yml")).unwrap()).unwrap_or_else(|_|json!({"features":{"unexpectedStopDetection":"none"},"retry":{"enabled":false,"modelFallback":false},"edit":{"mode":"hashline"}}));
  if let Some(maximum)=setup.get("maximum"){settings["task"]=json!({"maxRecursionDepth":maximum});}
  settings["tools"]=json!({"xdev":setup["device"]==true});
  std::fs::write(agent.join("config.yml"),serde_json::to_vec_pretty(&settings).unwrap()).unwrap();
 }
 let mut command=tokio::process::Command::new(env!("CARGO_BIN_EXE_ara"));
 command.env_clear();for key in ["PATH","PATHEXT","SYSTEMROOT","WINDIR","SYSTEMDRIVE","COMSPEC","ARA_TEST_BUN","ARA_TEST_IMAGE_HOST_ENTRY","ARA_TEST_IMAGE_PROJECT_DIR","ARA_TEST_TLAPLUS_GRAMMAR"]{if let Some(value)=std::env::var_os(key){command.env(key,value);}}
 command.env("HOME",&home).env("USERPROFILE",&home).env("APPDATA",home.join("AppData/Roaming")).env("LOCALAPPDATA",home.join("AppData/Local")).env("TEMP",root.join("temp")).env("TMP",root.join("temp")).env("TMPDIR",root.join("temp")).env("ARA_TEST_PEER_FRONTIERS",root.join("frontiers.json"));std::fs::create_dir_all(root.join("temp")).unwrap();
 if cold&&ops.iter().any(|op|op["materializedCatalog"]==true){command.env("ARA_TEST_PEER_MATERIALIZED_CATALOG",root.join("cold-materialized-model.json"));}
 if let Some(broker)=&broker{
  command.env("ARA_AUTH_BROKER_URL",&broker.url).env("ARA_AUTH_BROKER_TOKEN","synthetic-cold-broker-token");
  // Fixed OMP expands role aliases only without an explicit CLI provider.
  if !ops.iter().any(|op|op["rootAlias"]==true){command.args(["--provider","cold-fixture"]);}
 }
 command.env_remove("ARA_MODEL").env_remove("ARA_BASE_URL").env_remove("ARA_API").env_remove("ARA_PROVIDER").env("ARA_HOME",&home).env("ARA_API_KEY","sk-peer-loopback-controlled-fixture").env("ARA_TEST_PEER_SCENARIO",&scenario).env("ARA_TEST_CHILD_ARTIFACT_DIR",&root).args(["--mode","rpc","--api","openai-completions","--model",if cold&&ops.iter().any(|op|op["rootAlias"]==true){"@default"}else{"fake-model"},"--base-url",&url,"--no-skills","--tools",if read_only{"read"}else{"read,write,bash,wait"},"--compact-threshold",if cold&&ops.iter().any(|op|op["parentThreshold"]==120000){"120000"}else{"0"},"--cwd"]).arg(&project).arg("--session-dir").arg(home.join("sessions")).stdin(std::process::Stdio::null()).kill_on_drop(false);
 let stdout_file=std::fs::File::create(root.join("stdout.txt")).unwrap();let stderr_file=std::fs::File::create(root.join("stderr.txt")).unwrap();command.stdout(std::process::Stdio::from(stdout_file)).stderr(std::process::Stdio::from(stderr_file));let mut process=command.spawn().unwrap();let pid=process.id();std::fs::write(root.join("process.json"),json!({"state":"running","pid":pid}).to_string()).unwrap();
 let status=match tokio::time::timeout(Duration::from_secs(90),process.wait()).await{Ok(status)=>status.unwrap(),Err(_)=>{std::fs::write(root.join("process.json"),json!({"state":"unknown","pid":pid,"diagnostic":"Peer bank timeout; no automatic kill/replay. Reconcile original process and receipts."}).to_string()).unwrap();panic!("{label}: owned process timeout, retained PID {pid:?}");}};
 let stdout=std::fs::read(root.join("stdout.txt")).unwrap();let stderr=std::fs::read(root.join("stderr.txt")).unwrap();std::fs::write(root.join("process.json"),json!({"success":status.success(),"code":status.code(),"served":up.served(),"pid":pid}).to_string()).unwrap();if let Some(broker)=&broker{let safe=broker.requests.lock().unwrap().iter().map(|request|json!({"method":request.method,"path":request.path,"class":"global-broker"})).collect::<Vec<_>>();std::fs::write(root.join("cold-global-broker-requests.json"),serde_json::to_vec_pretty(&safe).unwrap()).unwrap();}if label=="p1-close-unknown"||label=="p4-wait-ack-unknown"||label.starts_with("p5-transition-unknown-"){assert!(!status.success(),"actual Unknown teardown must not exit success");}else{assert!(status.success(),"{label}: {}",String::from_utf8_lossy(&stderr));}drop(gate);
 let stdout=String::from_utf8(stdout).unwrap();let receipts=serde_json::from_str(stdout.lines().find_map(|line|line.strip_prefix("PEER_FIXTURE_RECEIPTS=")).expect("ordinary binary Peer receipts")).unwrap();let teardown=serde_json::from_str(stdout.lines().find_map(|line|line.strip_prefix("PEER_FIXTURE_TEARDOWN=")).expect("actual EOF/root teardown observation")).unwrap();
 let requests=up.requests.lock().await.clone();std::fs::write(root.join("all-requests.json"),serde_json::to_vec_pretty(&requests).unwrap()).unwrap();Bank{root,receipts,requests,teardown}
}
fn result(b:&Bank,i:usize)->&Value{&b.receipts[i]["data"]["result"]}
fn hub_call(id:&str,name:&str,args:Value)->Value{json!({"events":[tool_call(0,id,name,&args.to_string()),finish("tool_calls"),done()]})}
fn hub_end(b:&Bank,id:&str)->Value{let rows=actual_events(&b.root.join("root.events.jsonl"));let matching=rows.iter().filter(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]==id).collect::<Vec<_>>();assert_eq!(matching.len(),1,"one actual native result {id}");matching[0].clone()}
fn hub_text(event:&Value)->String{event["result"]["content"].as_array().into_iter().flatten().filter_map(|b|b["text"].as_str()).collect::<Vec<_>>().join("\n")}

fn file(r:&Value)->&Path{Path::new(r["session_file"].as_str().unwrap())}
fn contents(path:&Path,kind:&str)->Vec<String>{let journal=ara_session::SessionJournal::open(path).unwrap();journal.entries().iter().filter_map(|e|{if !e.observed_message_field(&["customType"]).ok().flatten()?.as_string().is_some_and(|s|s.equals_ascii(kind)){return None;}e.observed_message_field(&["content"]).ok().flatten()?.as_string()?.to_utf8().ok()}).collect()}
fn journal_bodies(path:&Path)->Vec<Value>{let journal=ara_session::SessionJournal::open(path).unwrap();journal.entries().iter().filter_map(|e|e.observed_message_body().unwrap()).map(|body|ara_session::native_checked_serde(&body).unwrap()).collect()}
fn parent_count(path:&Path)->usize{let journal=ara_session::SessionJournal::open(path).unwrap();let ids=journal.entries().iter().filter_map(|e|e.observed_message_field(&["details","peerWakeJob"]).ok().flatten()).map(|v|v.stringify()).collect::<Vec<_>>();let unique=ids.iter().collect::<std::collections::HashSet<_>>();assert_eq!(unique.len(),ids.len(),"wake result appended twice");ids.len()}

fn assert_journal_history_prefix(before:&str,after:&str){assert!(before.ends_with('\n')&&after.ends_with('\n'));let old=before.split_inclusive('\n').collect::<Vec<_>>();let new=after.split_inclusive('\n').collect::<Vec<_>>();assert!(new.len()>=old.len());for(index,(a,b))in old.iter().zip(&new).enumerate(){if a==b{continue;}assert_eq!(a.ends_with("\r\n"),b.ends_with("\r\n"));let mut a:Value=serde_json::from_str(a).unwrap();let mut b:Value=serde_json::from_str(b).unwrap();assert_eq!(a["type"],"session_init","only same full session_init may reformat row {index}");assert_eq!(b["type"],"session_init");for init in [&mut a,&mut b]{for field in ["thresholdPercent","thresholdTokens"]{if let Some(number)=init.get_mut("compactionThreshold").and_then(|pair|pair.get_mut(field)){assert!(number.is_number());let native=ara_session::native_wire_from_serde(number).expect("declared f64 must remain exactly representable");*number=ara_session::native_checked_serde(&native).expect("Native Number bits must survive JSON");}}}assert_eq!(a,b,"original init metadata must remain exact");}}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_p1_identity_owner_cancel_bank(){
 let b=run("p1",vec![json!({"op":"create","alias":"a","init":init(&["read"])}),json!({"op":"create","alias":"b","init":init(&["read"])}),json!({"op":"list"}),json!({"op":"cancel","alias":"b","from":"a","expectedError":true}),json!({"op":"cancel","alias":"b"}),json!({"op":"list"}),json!({"op":"create_nested","alias":"nested","from":"a","init":init(&["read"])}),json!({"op":"roster_snapshot"}),json!({"op":"root_close_probe","alias":"a","sibling":"nested"})],vec![],false).await;
 let a=result(&b,0);let other=result(&b,1);assert_ne!(a["key"]["id"],other["key"]["id"]);assert_eq!(a["key"]["root"],other["key"]["root"]);assert_eq!(a["parent_id"],a["key"]["root"]);assert!(file(a).is_file()&&file(other).is_file());assert_eq!(b.receipts[3]["success"],false);assert_eq!(result(&b,4)["cancelled"],true);assert_eq!(b.requests.len(),0);assert!(result(&b,5).as_array().unwrap().iter().any(|r|r["key"]["id"]==other["key"]["id"]&&r["status"]=="aborted"));assert!(PathBuf::from(format!("{}.tombstone",file(other).display())).is_file());
 let nested=result(&b,6);assert_eq!(nested["parent_id"],a["key"]["id"]);let disk=result(&b,7).as_array().unwrap();for live in [a,nested]{let restored=disk.iter().find(|r|r["key"]["id"]==live["key"]["id"]).expect("actual journal UUID rediscovered");assert_eq!(restored["parent_id"],live["parent_id"]);assert_eq!(std::fs::canonicalize(file(restored)).unwrap(),std::fs::canonicalize(file(live)).unwrap());let stem=file(live).file_stem().unwrap().to_string_lossy();assert_ne!(stem,live["key"]["id"].as_str().unwrap());assert!(!disk.iter().any(|r|r["key"]["id"]==stem.as_ref()));}assert_eq!(result(&b,8)["siblingJoinedBeforeRelease"],true);assert_eq!(result(&b,8)["inFlight"]["settled"],false);assert_eq!(result(&b,8)["receipt"]["settled"],true);

 let unknown=run("p1-close-unknown",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"close_unknown_probe"})],vec![],false).await;assert_eq!(unknown.requests.len(),0);assert_eq!(result(&unknown,1)["observerDropped"],true);assert_eq!(result(&unknown,1)["rootReceipt"]["settled"],false);assert_eq!(result(&unknown,1)["fault"]["sameHeldChannel"],true);assert_eq!(result(&unknown,1)["fault"]["sameFinalChannel"],true);assert_eq!(result(&unknown,1)["sameChannelAfterRoot"],true);assert_eq!(result(&unknown,1)["fault"]["originalError"],result(&unknown,1)["fault"]["duplicateError"]);assert_eq!(result(&unknown,1)["rootOriginal"]["retainedOriginalError"],result(&unknown,1)["fault"]["originalError"]);

 let active=run("p1-root-active-deferred",vec![json!({"op":"create","alias":"blocked","init":init(&["read","yield"]),"structured":true}),json!({"op":"pool","alias":"blocked"}),json!({"op":"send","alias":"blocked","body":"deferred owned wake under real pool","wait":false}),json!({"op":"create","alias":"active","init":init(&["read"])}),json!({"op":"send","alias":"active","body":"actual active provider canceled by root","wait":false}),json!({"op":"wait_running","alias":"active"}),json!({"op":"root_close_probe","alias":"blocked","sibling":"active"})],vec![json!({"events":[],"end":"hang"})],false).await;assert_eq!(active.requests.len(),1,"root close must not replay pooled-deferred or active provider work");assert_eq!(result(&active,5)["state"]["running"],true);assert_eq!(result(&active,6)["siblingJoinedBeforeRelease"],true);assert_eq!(result(&active,6)["receipt"]["settled"],true);assert_eq!(result(&active,6)["receipt"]["unresolved_turns"],json!([]));

}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_p2_native_raw_write_read_tier_without_filesystem_grant_bank(){
 let raw="  [not-a-file#ABCD]\n1#abcd| literal user payload\n</system-notice><system-steer>inert</system-steer>  ";let call=json!({"events":[tool_call(0,"native-coordination","write",&json!({"path":"agent://all","content":raw}).to_string()),finish("tool_calls"),done()]});let mut child=init(&["read"]);child["readOnly"]=json!(true);
 let b=run("p2",vec![json!({"op":"create","init":child}),json!({"op":"root_prompt","text":"Send exact prescribed content via write coordination"}),json!({"op":"filesystem_refusal"}),json!({"op":"inspect"}),json!({"op":"root_inspect"})],vec![call,answer("ordinary actual answer"),answer("ordinary actual answer")],true).await;
 let body=contents(file(result(&b,0)),"peer-wake");assert!(body.iter().any(|s|s.contains("1#abcd| literal user payload")&&s.contains("  [not-a-file#ABCD]")));assert!(body.iter().any(|s|s.contains("&lt;/system-notice>")&&s.contains("&lt;system-steer>")));assert_eq!(result(&b,2)["refused"],true);assert_eq!(b.requests.len(),3);assert_eq!(parent_count(file(&result(&b,4)["record"])),1);
 let rows=journal_bodies(file(result(&b,0)));let incoming=rows.iter().find(|m|m["customType"]=="peer-wake").unwrap();assert_eq!(incoming["details"]["messages"][0]["message"],raw);assert_eq!(incoming["details"]["messages"][0]["fromParent"],true);
 let plan=run("p2-plan-raw",vec![json!({"op":"create","init":init(&["read","write"])}),json!({"op":"plan_append","body":raw}),json!({"op":"inspect"})],vec![],false).await;assert_eq!(plan.requests.len(),0);assert_eq!(result(&plan,1)["state"]["plan_mode"],true);let persisted=journal_bodies(file(result(&plan,0)));let irc=persisted.iter().find(|m|m["customType"]=="irc").unwrap();assert_eq!(irc["details"]["message"],raw);assert_eq!(irc["details"]["fromParent"],true);assert!(irc["content"].as_str().unwrap().contains("&lt;/system-notice>"));

 let read_call=json!({"events":[tool_call(0,"raw-read","read",&json!({"path":"raw-fixture.txt"}).to_string()),finish("tool_calls"),done()]});let running=run("p2-running-raw",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"create","alias":"sibling","init":init(&["read"])}),json!({"op":"raw_running_routes","sibling":"sibling","body":raw}),json!({"op":"inspect"})],vec![read_call,answer("raw incoming parent and sibling handled")],false).await;assert_eq!(running.requests.len(),2);let persisted=journal_bodies(file(result(&running,0)));for parent in [true,false]{let actual=persisted.iter().find(|m|m["customType"]=="irc"&&m["details"]["message"]==raw&&m["details"]["fromParent"]==parent).expect("real four-route raw message evidence");assert!(actual["content"].as_str().unwrap().contains("&lt;/system-notice>"));assert_eq!(actual["details"]["to"],result(&running,0)["key"]["id"]);assert!(actual["details"]["id"].is_string());}

 // Hub27 migrated public surfaces: actual Native WRITE, never a new Hub API.
 for (label,maximum,enabled,allowed) in [("default",Value::Null,true,true),("zero",json!(0),true,false),("unlimited",json!(-1),true,true),("fraction",json!(0.5),true,true),("host-disabled",json!(-1),false,false)]{
  let mut setup=json!({"op":"hub_setup","enableIrc":enabled});if !maximum.is_null(){setup["maximum"]=maximum;}
  let write=hub_call("hub-policy","write",json!({"path":"agent://all","content":"  raw migrated send body  "}));
  let responses=if allowed{vec![write,answer("ordinary owner reply"),answer("ordinary owner reply")]}else{vec![write,answer("refused at actual dispatch")]};
  let b=run(&format!("hub-policy-{label}"),vec![setup,json!({"op":"create","init":init(&["read"])}),json!({"op":"root_prompt"}),json!({"op":"inspect"}),json!({"op":"cold_root_state"})],responses,false).await;
  let event=hub_end(&b,"hub-policy");assert_eq!(event["isError"],!allowed);assert_eq!(b.requests.len(),if allowed{3}else{2});
  let incoming=contents(file(result(&b,1)),"peer-wake");assert_eq!(!incoming.is_empty(),allowed);
  if allowed{
   assert!(hub_text(&event).contains("Broadcast delivered"));
   let rows=journal_bodies(file(result(&b,1)));
   let wake=rows.iter().find(|row|row["customType"]=="peer-wake").expect("actual migrated WRITE wake journal");
   let message=&wake["details"]["messages"][0];
   assert_eq!(message["message"],"  raw migrated send body  ","raw coordination payload retains both trailing spaces");
   assert_eq!(message["from"],result(&b,1)["parent_id"]);
   assert_eq!(message["to"],result(&b,1)["key"]["id"]);
   assert!(incoming.iter().any(|body|body.contains("\n  raw migrated send body\n")),"fixed prompt.render trims rendered line endings only");
  }else{assert!(hub_text(&event).contains("Peer messaging is unavailable"));}
  let system=result(&b,4)["systemPrompt"].to_string();assert_eq!(system.contains("Peer messaging is disabled for this Session."),!allowed,"advertised messaging follows actual policy");assert!(system.contains("Only the bare index is supported"));
 }
 // A leaf child still has a parent at the cap; root cannot spawn capacity is
 // not permission to cut off the child's existing communication authority.
 let b=run("hub-child-at-cap",vec![json!({"op":"hub_setup","maximum":0}),json!({"op":"create","init":init(&["read"])}),json!({"op":"send"}),json!({"op":"inspect"}),json!({"op":"root_inspect"})],vec![hub_call("hub-child","write",json!({"path":"agent://all","content":"child may still reach parent"})),answer("ordinary owner reply"),answer("ordinary owner reply")],false).await;
 let child_events=actual_events(&b.root.join(format!("{}.events.jsonl",result(&b,1)["key"]["id"].as_str().unwrap())));assert!(child_events.iter().any(|e|e["type"]=="tool_execution_end"&&e["toolCallId"]=="hub-child"&&e["isError"]==false));assert_eq!(b.requests.len(),3);
 assert!(journal_bodies(file(&result(&b,4)["record"])).iter().any(|m|m["customType"]=="peer-wake"&&m["details"]["messages"][0]["message"]=="child may still reach parent"));
 // Current settings are read again at the irreversible URI entry, even while
 // the already selected ordinary filesystem WRITE capability remains active.
 let b=run("hub-live-denial",vec![json!({"op":"hub_setup"}),json!({"op":"create","init":init(&["read"])}),json!({"op":"hub_settings","maximum":0}),json!({"op":"root_prompt"}),json!({"op":"inspect"})],vec![hub_call("hub-live","write",json!({"path":"agent://all","content":"must not be sent"})),answer("stopped without sending")],false).await;
 assert_eq!(hub_end(&b,"hub-live")["isError"],true);assert!(contents(file(result(&b,1)),"peer-wake").is_empty());assert_eq!(b.requests.len(),2);
 // Disable only agent coordination. A different READ-tier URI device still
 // dispatches through the actual selected Native WRITE and cannot write files.
 let b=run("hub-xdev-preserved",vec![json!({"op":"hub_setup","maximum":0,"device":true}),json!({"op":"create","init":init(&["read"])}),json!({"op":"root_prompt"}),json!({"op":"filesystem_refusal"}),json!({"op":"inspect"})],vec![hub_call("hub-denied-agent","write",json!({"path":"agent://all","content":"no coordination"})),hub_call("hub-other-device","write",json!({"path":"hub-device://echo","content":"other device remains writable"})),answer("both actual device outcomes handled")],true).await;
 assert_eq!(hub_end(&b,"hub-denied-agent")["isError"],true);assert_eq!(hub_end(&b,"hub-other-device")["isError"],false);assert_eq!(std::fs::read_to_string(b.root.join("hub-device-written.txt")).unwrap(),"other device remains writable");assert_eq!(result(&b,3)["refused"],true);assert!(contents(file(result(&b,1)),"peer-wake").is_empty());assert_eq!(b.requests.len(),3);
 // A missing issuing identity and an unknown/self target are real tool
 // failures with no recipient effects. The invalid sender fixture changes only
 // its selected URI owner; the actual actor/registry keep teardown ownership.
 for (label,path,missing) in [("unknown","agent://no-such-agent",false),("missing-sender","agent://all",true)]{
  let mut ops=vec![json!({"op":"hub_setup"}),json!({"op":"create","init":init(&["read"])})];if missing{ops.push(json!({"op":"hub_missing_sender"}));}ops.push(json!({"op":"root_prompt"}));
  let b=run(&format!("hub-{label}"),ops,vec![hub_call("hub-failed-send","write",json!({"path":path,"content":"no authorized recipient effect"})),answer("failed receipt retained")],false).await;
  assert_eq!(hub_end(&b,"hub-failed-send")["isError"],true);assert_eq!(b.requests.len(),2);assert!(contents(file(result(&b,1)),"peer-wake").is_empty());
 }
 let b=run("hub-self-denial",vec![json!({"op":"hub_setup"}),json!({"op":"hub_self_probe"})],vec![],false).await;assert_eq!(result(&b,1)["refused"],true);assert!(b.requests.is_empty());
 assert_eq!(result(&b,1)["nativeGraphCall"],true);
 let b=run("hub-immediate-direct",vec![json!({"op":"hub_setup"}),json!({"op":"create","init":init(&["read"])}),json!({"op":"hub_direct_probe"})],vec![json!({"events":[],"end":"hang"})],false).await;
 assert_eq!(b.requests.len(),1);assert_eq!(result(&b,2)["sent"]["nativeGraphCall"],true);assert_eq!(result(&b,2)["recipientBeforeCancel"]["running"],true);assert_eq!(result(&b,2)["cancelledAfterReceipt"],true);

 let b=run("hub-partial-broadcast",vec![json!({"op":"hub_setup"}),json!({"op":"create","init":init(&["read"])}),json!({"op":"create","alias":"parked","init":init(&["read"])}),json!({"op":"send","alias":"parked"}),json!({"op":"park","alias":"parked"}),json!({"op":"hub_failed_recipient"}),json!({"op":"root_prompt"}),json!({"op":"inspect"}),json!({"op":"inspect","alias":"parked"})],vec![answer("prime parked transcript"),hub_call("hub-partial","write",json!({"path":"agent://all","content":"one authoritative partial broadcast"})),answer("ordinary owner reply"),answer("ordinary owner reply")],false).await;
 let event=hub_end(&b,"hub-partial");assert_eq!(event["isError"],false);let receipts=&event["result"]["details"]["message"]["receipts"];assert_eq!(receipts.as_array().unwrap().len(),2);assert!(receipts.as_array().unwrap().iter().any(|r|r["to"]=="controlled-unavailable-peer"&&r["outcome"]=="failed"));assert!(receipts.as_array().unwrap().iter().any(|r|r["to"]==result(&b,1)["key"]["id"]&&r["outcome"]!="failed"));assert_eq!(b.requests.len(),4);assert_eq!(contents(file(result(&b,1)),"peer-wake").len(),1);assert_eq!(contents(file(result(&b,2)),"peer-wake").len(),1,"broadcast did not revive parked recipient");assert_eq!(result(&b,8)["record"]["status"],"parked");
 // Public READ history:// index: actual registered Main/self, idle, parked,
 // aborted, disk-only ID and advisor exclusion use the five-column schema.
 let b=run("hub-history-index",vec![json!({"op":"hub_setup","maximum":0}),json!({"op":"create","alias":"a","init":init(&["read"])}),json!({"op":"create","alias":"b","init":init(&["read"])}),json!({"op":"create","alias":"c","init":init(&["read"])}),json!({"op":"send","alias":"a"}),json!({"op":"park","alias":"a"}),json!({"op":"cancel","alias":"c"}),json!({"op":"hub_index_seed"}),json!({"op":"root_prompt"}),json!({"op":"root_prompt"}),json!({"op":"root_prompt"})],vec![answer("parked history"),hub_call("hub-index","read",json!({"path":"history://"})),answer("index read"),hub_call("hub-transcript-unavailable","read",json!({"path":"history://current/full"})),answer("unavailable transcript handled"),hub_call("hub-readonly-history","write",json!({"path":"history://","content":"must never write index"})),answer("readonly index handled")],false).await;
 let index=hub_end(&b,"hub-index");assert_eq!(index["isError"],false);let text=hub_text(&index);assert!(text.contains("# Agents")&&text.contains("| id | status | kind | parent | last activity |"));
 let root=result(&b,7)["rootId"].as_str().unwrap();assert!(text.contains(&format!("| {root} | running | main | — |")));
 for (op,status) in [(1,"parked"),(2,"idle"),(3,"aborted")]{let id=result(&b,op)["key"]["id"].as_str().unwrap();assert!(text.contains(&format!("| {id} | {status} | sub | {root} |")));assert!(!text.contains(file(result(&b,op)).file_stem().unwrap().to_str().unwrap()));}
 let disk=result(&b,7)["diskId"].as_str().unwrap();assert!(text.contains(&format!("| {disk} | on disk | — | — | — |")));assert!(!text.contains("__advisor-hidden"));assert!(!text.contains(result(&b,7)["foreignId"].as_str().unwrap()));assert_eq!(text.matches(&format!("| {disk} |")).count(),1);
 assert_eq!(hub_end(&b,"hub-transcript-unavailable")["isError"],true);assert_eq!(hub_end(&b,"hub-readonly-history")["isError"],true);assert_eq!(b.requests.len(),7);

 relay_cases::complete_family().await;
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_p3_park_cold_revive_latest_contract_and_writer_bank(){
 let b=run("p3",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"send","body":"first peer message"}),json!({"op":"inspect"}),json!({"op":"adopt_ttl","ttl":0}),json!({"op":"park"}),json!({"op":"revive"}),json!({"op":"send","body":"second peer message"}),json!({"op":"inspect"}),json!({"op":"root_inspect"}),json!({"op":"park"}),json!({"op":"probe_recovery"}),json!({"op":"park"}),json!({"op":"cold_bind_race"})],vec![answer("first peer answer"),answer("second peer answer")],false).await;
 let a=&result(&b,2)["record"];let c=&result(&b,7)["record"];assert_eq!(a["key"],c["key"]);assert_eq!(a["session_file"],c["session_file"]);assert_ne!(a["runtime"]["runtime_generation"],c["runtime"]["runtime_generation"]);assert_eq!(result(&b,4)["parked"],true);assert_journal_history_prefix(result(&b,2)["rawJournal"].as_str().unwrap(),result(&b,7)["rawJournal"].as_str().unwrap());assert_eq!(b.requests.len(),2);assert_eq!(parent_count(file(&result(&b,8)["record"])),2);
 assert_eq!(result(&b,4)["joined"],true);assert_eq!(result(&b,4)["after"]["status"],"parked");assert!(result(&b,4)["after"]["runtime"].is_null());assert_eq!(result(&b,10)["before"]["key"],result(&b,10)["retained"]["key"]);assert_eq!(result(&b,10)["before"]["session_file"],result(&b,10)["after"]["session_file"]);assert_eq!(result(&b,12)["before"]["key"],result(&b,12)["after"]["key"]);assert_eq!(result(&b,12)["after"]["status"],"aborted");assert!(result(&b,12)["published"].is_null());

}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_p4_concurrent_pool_parent_wait_and_failed_outcome_bank(){
 let b=run("p4",vec![json!({"op":"create","init":init(&["read","yield"]),"structured":true}),json!({"op":"concurrent","bodies":["first wake","second wake"]}),json!({"op":"pool"}),json!({"op":"send","body":"defer under actual pooled owner","wait":false}),json!({"op":"clear"}),json!({"op":"parent_wait"}),json!({"op":"settle"}),json!({"op":"inspect"}),json!({"op":"create","alias":"failed","init":init(&["read"])}),json!({"op":"send","alias":"failed","body":"controlled provider failure"}),json!({"op":"root_inspect"})],vec![yielded("yield-1",json!({"round":1})),yielded("yield-2",json!({"round":2})),yielded("yield-3",json!({"round":3})),json!({"status":400,"body":"{\"error\":{\"message\":\"controlled permanent peer failure\"}}"})],false).await;
 assert_eq!(b.requests.len(),4);assert_eq!(result(&b,2)["pooled"],true);assert_eq!(result(&b,4)["pooled"],false);assert!(result(&b,7)["artifact"].as_str().unwrap().contains("3"));assert_eq!(parent_count(file(&result(&b,10)["record"])),4);assert!(result(&b,10)["rawJournal"].as_str().unwrap().contains("controlled permanent peer failure"));
 // Final production Wait: child HTTP is held until the real Wait progress proves its owned watch exists.
 let wait_call=json!({"events":[tool_call(0,"native-wait","wait","{}"),finish("tool_calls"),done()]});
 let waiting=run("p4-native-wait",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"send","body":"child answer must wait for actual Native wait","wait":false,"waitForRequest":true}),json!({"op":"root_prompt","text":"Call wait for the actual running child"}),json!({"op":"inspect"}),json!({"op":"root_inspect"})],vec![answer("child native wait result"),wait_call,answer("Root consumed exact foreground result")],false).await;
 assert_eq!(waiting.requests.len(),3);assert!(waiting.root.join("native-wait-watched.json").is_file());let root_file=file(&result(&waiting,4)["record"]);let root_rows=journal_bodies(root_file);let tool_results=root_rows.iter().filter(|m|m["role"]=="toolResult"&&m["toolCallId"]=="native-wait").collect::<Vec<_>>();assert_eq!(tool_results.len(),1);assert_eq!(tool_results[0]["isError"],false);assert_eq!(tool_results[0]["details"]["op"],"wait");assert!(tool_results[0]["content"].to_string().contains("child native wait result"));assert_eq!(parent_count(root_file),0,"foreground ToolResult and custom parent completion must not duplicate consumption");
 // Fixed task/executor-irc-wake-yield-reset29: real reused Native tool and autonomous IRC observer.
 let reset_init=init(&["yield"]); // Fixed source has no output schema; a structured-schema guard must not replace no-text.
 let prior_section=json!({"events":[tool_call(0,"prior-section","yield",&json!({"type":["findings"],"data":"one finding"}).to_string()),finish("tool_calls"),done()]});let thinking=json!({"data":{"id":"fake-1","choices":[{"index":0,"delta":{"reasoning_content":"Thinking without a result text"}}]}});let empty_result=json!({"events":[thinking,tool_call(0,"wake-empty","yield",&json!({"type":"result"}).to_string()),finish("tool_calls"),done()]});
 let reset=run("p4-autonomous-reset",vec![json!({"op":"create","init":reset_init,"structured":true}),json!({"op":"create","alias":"sibling","init":init(&["read"])}),json!({"op":"autonomous_wake_reset","sibling":"sibling"})],vec![prior_section,answer("old incremental run finished conversationally"),empty_result,yielded("new-explicit-result",json!({"answer":"new actual result"})),answer("sibling received actual automatic stop relay")],false).await;assert_eq!(reset.requests.len(),5);assert_eq!(result(&reset,2)["originalRejectedResult"]["isError"],true);assert!(result(&reset,2)["originalRejectedResult"]["result"].to_string().contains("no text (thinking only)"));assert_eq!(result(&reset,2)["runtime"],result(&reset,2)["after"]["runtime"]);
 // A separate original private foreground ToolResult reaches durable disk, then its receipt is lost.
 let lost_wait=json!({"events":[tool_call(0,"native-wait","wait","{}"),finish("tool_calls"),done()]});let unknown_wait=run("p4-wait-ack-unknown",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"wait_ack_unknown"})],vec![answer("private foreground original result"),lost_wait],false).await;assert_eq!(unknown_wait.requests.len(),2);let lost=result(&unknown_wait,1);assert_eq!(lost["observerDropped"],true);assert_eq!(lost["unknown"]["consumption"]["state"],"unknown");assert_eq!(lost["originalEntryIds"],lost["afterEntryIds"]);assert_eq!(lost["originalEntryIds"].as_array().unwrap().len(),1);assert_eq!(lost["requestsBeforeUnknownEOF"],2);assert!(unknown_wait.root.join("native-wait-watched.json").is_file());assert_eq!(unknown_wait.teardown["lifecycleRootSnapshot"]["settled"],false);let unresolved=unknown_wait.teardown["lifecycleRootSnapshot"]["unresolved_jobs"].as_array().unwrap();assert!(unresolved.iter().any(|job|job["id"]==lost["unknown"]["id"]&&job["consumption"]["state"]=="unknown"));assert!(unknown_wait.teardown["actualEOF"]["error"].is_string()||unknown_wait.teardown["actualEOF"]["code"].as_i64().is_some_and(|code|code!=0));
 // Both legal interrupt modes: auto delivery already owns the completion before the actual Wait enters.
 for mode in ["immediate","wait"]{let call=json!({"events":[tool_call(0,"auto-before-native-wait","wait","{}"),finish("tool_calls"),done()]});let observed=run(&format!("p4-auto-before-wait-{mode}"),vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"auto_before_wait","interruptMode":mode}),json!({"op":"root_inspect"})],vec![call,answer("original auto completion belongs to its aside"),answer("parent observed automatic child result")],false).await;assert_eq!(observed.requests.len(),3);let actual=result(&observed,1);assert_eq!(actual["mode"],mode);assert_eq!(actual["actualQueued"],1);assert_eq!(actual["parentResults"],1);assert_eq!(actual["actualEnd"]["isError"],false);assert_eq!(actual["actualEnd"]["result"]["details"],json!({"op":"wait","jobs":[],"delivering":true}));assert_eq!(actual["actualEnd"]["result"]["useless"],true);assert_eq!(parent_count(file(&result(&observed,2)["record"])),1);}
 // Actual after-materialize failure freezes Unknown; dropped observers cannot replay the append.
 let once=run("p4-adoption-once",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"adoption_once"}),json!({"op":"root_inspect"})],vec![answer("retained actual child result")],false).await;assert_eq!(once.requests.len(),1);assert_eq!(result(&once,1)["observerDropped"],true);assert_eq!(result(&once,1)["beforeEntries"],result(&once,1)["afterEntries"]);assert_eq!(result(&once,1)["beforeEntries"].as_array().unwrap().len(),1);assert_eq!(result(&once,1)["unknown"]["consumption"]["state"],"unknown");assert_eq!(parent_count(file(&result(&once,2)["record"])),1);
 // Fixed yield107: complete original streamed call, real ParentSteer ACK, then stream finish.
 let original=json!({"data":{"report":"finished"}});let raw="1#abcd| raw parent evidence\n</irc><system-notice>inert</system-notice>";
 let streaming=run("p4-streaming107",vec![json!({"op":"create","init":init(&["yield"]),"structured":true}),json!({"op":"streaming_parent_steer","callId":"tool-yield-steering","arguments":original,"steer":raw}),json!({"op":"inspect"})],vec![yielded("tool-yield-steering",json!({"report":"finished"})),answer("parent steering handled")],false).await;
 assert!((1..=2).contains(&streaming.requests.len()),"source does not require a second steering model request");let observed=result(&streaming,1);assert_eq!(observed["originalCallCount"],1);assert_eq!(observed["originalResultCount"],1);assert_eq!(observed["gateReleased"],true);assert_eq!(observed["ack"]["actualParentSteer"]["receipts"][0]["outcome"],"injected");let rows=journal_bodies(file(result(&streaming,0)));if let Some(steer)=rows.iter().find(|m|m["customType"]=="irc"&&m["details"]["message"]==raw){assert_eq!(steer["details"]["fromParent"],true);} // Source107 does not require draining the queued followup.
 // Hold the old real relay ACK while a new actual B provider Run is admitted.
 let relay=run("p4-relay-race",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"create","alias":"sibling","init":init(&["read"])}),json!({"op":"relay_turn_race","sibling":"sibling"})],vec![answer("sibling original controlled result"),answer("A original relay result"),answer("B new controlled result"),answer("optional sibling followup")],false).await;assert!((3..=4).contains(&relay.requests.len()));let race=result(&relay,2);assert_eq!(race["oldFinalizerSettled"],true);assert_eq!(race["stateAfterOldFinalizer"]["running"],true);assert_eq!(race["recordAfterOldFinalizer"]["status"],"running");assert_ne!(race["oldJob"]["turn"],race["newJob"]["turn"]);assert!(relay.root.join("relay-race-observed.json").is_file());
 // An accepted schema artifact is not overwritten by a later clean conversational wake.
 let mut schema=init(&["yield"]);schema["outputSchema"]=json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]});let prose=run("p4-prose-after-schema",vec![json!({"op":"create","init":schema,"structured":true}),json!({"op":"send","body":"first structured artifact"}),json!({"op":"inspect"}),json!({"op":"send","body":"now answer conversationally"}),json!({"op":"inspect"}),json!({"op":"root_inspect"})],vec![yielded("schema-first",json!({"answer":"recorded"})),answer("clean conversational wake answer")],false).await;assert_eq!(prose.requests.len(),2);assert_eq!(result(&prose,2)["artifact"],result(&prose,4)["artifact"]);assert!(result(&prose,4)["artifact"].as_str().unwrap().contains("recorded"));let parent=contents(file(&result(&prose,5)["record"]),"peer-task-result");assert_eq!(parent.len(),2);assert!(parent.last().unwrap().contains("clean conversational wake answer"));assert!(!parent.last().unwrap().contains("does not match schema"));

 // Entered final Native Wait: authenticated parent steering returns normal; genuine abort remains error.
 for hard in [false,true]{let call=json!({"events":[tool_call(0,"native-child-wait","wait","{}"),finish("tool_calls"),done()]});let mut responses=vec![answer("owned grandchild terminal after Wait interruption"),call];if !hard{responses.push(answer("actual child steering handled"));}let label=if hard{"p4-child-wait-abort"}else{"p4-child-wait-soft"};let observed=run(label,vec![json!({"op":"create","init":init(&["read","wait"])}),json!({"op":"create_nested","alias":"grand","from":"child","init":init(&["read"])}),json!({"op":"child_wait_interrupt","grandchild":"grand","hard":hard}),json!({"op":"inspect"})],responses,false).await;assert_eq!(observed.requests.len(),if hard{2}else{3});assert_eq!(result(&observed,2)["actualEnd"]["isError"],hard);if !hard{assert_eq!(result(&observed,2)["actualEnd"]["result"]["details"]["interrupted"],true);}else{assert_ne!(result(&observed,2)["actualEnd"]["result"]["details"]["interrupted"],true);}}

 let no_work=run("hub-wait-no-work",vec![json!({"op":"hub_setup"}),json!({"op":"root_prompt"})],vec![hub_call("hub-no-work","wait",json!({})),answer("no owned work error retained")],false).await;
 assert_eq!(hub_end(&no_work,"hub-no-work")["isError"],true);assert!(hub_text(&hub_end(&no_work,"hub-no-work")).contains("Nothing to wait for"));assert_eq!(no_work.requests.len(),2);
 // Migrated pending inbox consumption uses the actual no-argument Native Wait.
 // The existing HTTP gate releases its original ToolCall only after a real queue ACK.
 // Real Wait mode reaches the kernel; Immediate intentionally skips at the earlier
 // general interrupt boundary. Kernel pre-cancelled precedence is a distinct control.
 let b=run("hub-pending-wait",vec![json!({"op":"hub_setup","interruptMode":"wait"}),json!({"op":"create","alias":"sender","init":init(&["read"])}),json!({"op":"hub_pending_wait","sibling":"sender"})],vec![hub_call("hub-pending-wait","wait",json!({})),answer("pending Wait body consumed")],false).await;
 assert_eq!(result(&b,2)["waitResult"]["isError"],false);assert_eq!(result(&b,2)["waitResult"]["result"]["details"]["message"]["message"],"pending aside wins the Native Wait interrupt boundary");assert_eq!(b.requests.len(),2);
 let immediate=run("hub-pending-wait-immediate",vec![json!({"op":"hub_setup","interruptMode":"immediate"}),json!({"op":"create","alias":"sender","init":init(&["read"])}),json!({"op":"hub_pending_wait","sibling":"sender","expectSkipped":true})],vec![hub_call("hub-pending-wait","wait",json!({})),answer("original early IRC admitted in the same Native Session")],false).await;
 assert_eq!(result(&immediate,2)["skippedBeforeAdmission"],true);
 assert_eq!(result(&immediate,2)["waitResult"]["result"]["details"]["executed"],false);
 assert_eq!(immediate.requests.len(),2,"automatic IRC must reach the next actual Graph request without replay");
 let actual=&immediate.requests[1];assert!(actual.to_string().contains("pending aside wins the Native Wait interrupt boundary"));

}
struct PolicyPort{coordination:bool,writes:AtomicUsize,body:Mutex<Option<String>>}
#[async_trait::async_trait]
impl ara_tools::ContentUriPort for PolicyPort{
 fn route(&self,url:&str)->ara_tools::ContentUriRoute{if url.starts_with("agent://"){ara_tools::ContentUriRoute::Registered{writable:true}}else{ara_tools::ContentUriRoute::Unregistered}}
 fn write_policy(&self,_:&str)->ara_tools::ContentUriWritePolicy{if self.coordination{ara_tools::ContentUriWritePolicy{payload:ara_tools::ContentUriWritePayload::Verbatim,scope:ara_tools::ContentUriWriteScope::Coordination,tier:ara_tools::ContentUriWriteTier::Read}}else{Default::default()}}
 async fn read(&self,_:&str,_:CancellationToken)->Result<ara_tools::UriResource,ara_agent::ToolError>{Err(ara_agent::ToolError("controlled policy fault, no read".into()))}
 async fn write(&self,_:&str,body:&str,_:CancellationToken)->Result<(),ara_agent::ToolError>{self.writes.fetch_add(1,Ordering::AcqRel);*self.body.lock().unwrap()=Some(body.into());Ok(())}
}
/// Actual original Native WRITE, held approval and mutable same-Arc selected leaf/epoch.
async fn held_native_route_approval(root:&Path,journal:&ara_session::SessionJournal){
 use ara_agent::{ActualToolSource,ActualExecuteRequest,AgentMessage,ActualProviderError,SessionValues};
 use ara_cli::actual_tools::{BuiltinKind,NativeDeviceApprovalHost,NativeDeviceApprovalRequest,NativeDeviceApprovalConfiguration};
 use ara_cli::native_factory::{PreparedNativeSession,NativeCatalogFactoryImports,catalog::{NativeToolCatalog,NativeDeviceOptions},host::{NativeHostCatalogInputs,NativeHostToolSelection,prepare_host_catalog_with_devices}};
 use ara_rpc::value_heap::{ValueHandle,ValueRuntime};
 use ara_tools::ContentUriPort;
 struct Selected {epoch:u64,route:ara_tools::ContentUriRoute,leaf:Arc<PolicyPort>}
 struct MutablePort {state:Mutex<Selected>}
 #[async_trait::async_trait]impl ContentUriPort for MutablePort{
  fn route(&self,_:&str)->ara_tools::ContentUriRoute{self.state.lock().unwrap().route.clone()}
  fn write_policy(&self,url:&str)->ara_tools::ContentUriWritePolicy{self.state.lock().unwrap().leaf.write_policy(url)}
  fn write_selection(&self,url:&str)->ara_tools::ContentUriWriteSelection{let state=self.state.lock().unwrap();ara_tools::ContentUriWriteSelection{owner:Arc::as_ptr(&state.leaf)as usize,epoch:state.epoch,route:state.route.clone(),policy:state.leaf.write_policy(url)}}
  async fn read(&self,_:&str,_:CancellationToken)->Result<ara_tools::UriResource,ara_agent::ToolError>{Err(ara_agent::ToolError("controlled leaf has no read".into()))}
  async fn write(&self,url:&str,content:&str,cancel:CancellationToken)->Result<(),ara_agent::ToolError>{let selected=self.write_selection(url);self.write_result_selected(url,content,&selected,cancel).await.map(|_|()).map_err(|e|ara_agent::ToolError(e.to_string()))}
  async fn write_result_selected(&self,url:&str,content:&str,selection:&ara_tools::ContentUriWriteSelection,_:CancellationToken)->Result<Option<ara_agent::ToolOutput>,ara_tools::ContentUriWriteDispatchError>{let state=self.state.lock().unwrap();let current=ara_tools::ContentUriWriteSelection{owner:Arc::as_ptr(&state.leaf)as usize,epoch:state.epoch,route:state.route.clone(),policy:state.leaf.write_policy(url)};if &current!=selection||!matches!(state.route,ara_tools::ContentUriRoute::Registered{writable:true}){return Err(ara_tools::ContentUriWriteDispatchError::NotDispatched(ara_agent::ToolError("controlled exact leaf changed before dispatch".into())));}state.leaf.writes.fetch_add(1,Ordering::SeqCst);*state.leaf.body.lock().unwrap()=Some(content.into());Ok(Some(ara_agent::ToolOutput::text("Actual selected URI transport accepted original content")))}
 }
 struct HeldHost {values:Arc<SessionValues>,entered:Arc<tokio::sync::Notify>,gate:Arc<tokio::sync::Semaphore>,requests:Arc<AtomicUsize>}
 impl NativeDeviceApprovalHost for HeldHost{
  fn configuration(&self,_:&ValueHandle,values:&SessionValues,_:&dyn ValueRuntime)->Result<NativeDeviceApprovalConfiguration,ActualProviderError>{assert!(std::ptr::eq(values,self.values.as_ref()));Ok(NativeDeviceApprovalConfiguration{mode:values.heap().string("always-ask".into())?,user_policies:values.heap().import_serde(&json!({"write":"prompt"}))?,has_ui:true,pending_provider_safety:values.heap().array(Vec::new())?})}
  fn approve(&self,request:Arc<NativeDeviceApprovalRequest>)->futures::future::BoxFuture<'static,Result<bool,ActualProviderError>>{assert!(Arc::ptr_eq(&request.values,&self.values));self.requests.fetch_add(1,Ordering::SeqCst);self.entered.notify_waiters();let gate=self.gate.clone();Box::pin(async move{gate.acquire().await.map_err(|_|ActualProviderError::Protocol("held actual approval gate closed"))?.forget();Ok(true)})}
  fn provider_safety_approved(&self,_:&ValueHandle,_:&SessionValues,_:&dyn ValueRuntime)->Result<(),ActualProviderError>{Err(ActualProviderError::Unsupported("No provider safety approval is part of this URI test"))}
 }
 for case in ["stable","replace-leaf","removed","aba"]{
  let first=Arc::new(PolicyPort{coordination:true,writes:AtomicUsize::new(0),body:Mutex::new(None)});let fallback=Arc::new(PolicyPort{coordination:false,writes:AtomicUsize::new(0),body:Mutex::new(None)});let leaf=Arc::new(MutablePort{state:Mutex::new(Selected{epoch:1,route:ara_tools::ContentUriRoute::Registered{writable:true},leaf:first.clone()})});
  let chain=ara_cli::host_vault::compose_ports(leaf.clone(),Some(fallback.clone()));let router=ara_cli::session_artifacts::ArtifactUriRouter::new(ara_cli::session_artifacts::SessionArtifacts::for_journal(journal),Some(chain));let context=ara_tools::ToolContext::new(root).with_edit(ara_edit::EditMode::Hashline,true).with_uri_port(router);context.set_device_only_write(true);
  let realm=ara_cli::native_realm::NativeRealm::prepare().unwrap();let values=realm.values.clone();let heap=values.heap();let entered=Arc::new(tokio::sync::Notify::new());let gate=Arc::new(tokio::sync::Semaphore::new(0));let requests=Arc::new(AtomicUsize::new(0));let host=Arc::new(HeldHost{values:values.clone(),entered:entered.clone(),gate:gate.clone(),requests:requests.clone()});
  let prepared_catalog=prepare_host_catalog_with_devices(&realm,NativeHostCatalogInputs{context:context.clone(),builtin_kinds:vec![BuiltinKind::Write],builtin_grants:vec![true],mcp_tools:Vec::new(),mcp_grants:Vec::new(),mcp_routes:Default::default(),rpc_owners:Vec::new(),rpc_grants:Vec::new(),advertised:vec![NativeHostToolSelection::Builtin(BuiltinKind::Write)],bash_session:ara_cli::actual_tools::bash_native::SessionBashOwner::new(Default::default())},NativeDeviceOptions{enabled:true,explicit_builtins:true,approval_host:Some(host),..Default::default()}).unwrap();
  let source=prepared_catalog.snapshot.builtin().clone();let originals=source.current_tools(&values).unwrap().unwrap();let receiver=heap.get(&originals,&"0".into(),values.runtime()).unwrap();let binding=source.bind_selected(&receiver,&values).unwrap();let catalog=NativeToolCatalog::new(&values,prepared_catalog.snapshot).unwrap();let draft=PreparedNativeSession::from_realm(realm.clone(),None).unwrap();let prepared=draft.finish_catalog_bindings(NativeCatalogFactoryImports{catalog,string_function:realm.globals.string.clone(),bun:realm.globals.bun.clone(),original_logger:None}).unwrap();
  let raw="  [raw#ABCD]\n1#abcd| literal URI body\n  ";let args=heap.import_serde(&json!({"path":"agent://peer","content":raw})).unwrap();let call=heap.import_serde(&json!({"type":"toolCall","id":"held-native-uri-write","name":"write"})).unwrap();heap.define_own_data(&call,"arguments".into(),args.clone(),values.runtime()).unwrap();let mut assistant=serde_json::to_value(ara_ai::AssistantMessage::empty("fixture","fixture","fixture")).unwrap();assistant["role"]="assistant".into();assistant["stopReason"]="toolUse".into();let message=heap.import_serde(&assistant).unwrap();heap.define_own_data(&message,"content".into(),heap.array(vec![call.clone()]).unwrap(),values.runtime()).unwrap();let owner=AgentMessage::from_graph(message,None).unwrap();let tool_context=heap.object().unwrap();let cancel=CancellationToken::new();let driver=ara_cli::native_run::NativeRun::begin(prepared.clone(),cancel.clone()).unwrap();prepared.require_tool_hooks().unwrap().bind_run_context(&values,&tool_context).unwrap();
  let request=ActualExecuteRequest{callable:heap.get(&receiver,&"execute".into(),values.runtime()).unwrap(),receiver,arguments_slot:owner.current_arguments_slot(&call).unwrap(),owner,tool_call:call.clone(),call_id:"held-native-uri-write".into(),args:args.clone(),values:values.clone(),cancel,on_update:Arc::new(|_|Ok(())),tool_context};
  let notified=entered.notified();tokio::pin!(notified);notified.as_mut().enable();let invoker=binding.capability.actual_invoker().unwrap();let invoke=driver.drive(invoker.invoke_settled(request));tokio::pin!(invoke);
  tokio::time::timeout(Duration::from_secs(5),async{tokio::select!{_=&mut notified=>{},early=&mut invoke=>panic!("{case} did not enter actual held Native approval: {early:?}")}}).await.unwrap();
  assert_eq!(requests.load(Ordering::SeqCst),1);assert_eq!(first.writes.load(Ordering::SeqCst),0);assert_eq!(fallback.writes.load(Ordering::SeqCst),0);
  {let mut current=leaf.state.lock().unwrap();match case{"stable"=>{},"replace-leaf"=>{current.leaf=fallback.clone();current.epoch+=1;},"removed"=>{current.route=ara_tools::ContentUriRoute::Removed;current.epoch+=1;},"aba"=>{current.leaf=fallback.clone();current.epoch+=1;current.leaf=first.clone();current.epoch+=1;},_=>unreachable!()}}
  gate.add_permits(1);let outcome=tokio::time::timeout(Duration::from_secs(5),&mut invoke).await.unwrap();
  if case=="stable"{assert!(outcome.is_ok()&&outcome.as_ref().unwrap().is_ok());assert_eq!(first.writes.load(Ordering::SeqCst),1);assert_eq!(first.body.lock().unwrap().as_deref(),Some(raw));}else{assert!(outcome.is_err()||outcome.as_ref().unwrap().is_err(),"{case} dispatched with stale approval");assert_eq!(first.writes.load(Ordering::SeqCst),0);}
  assert_eq!(fallback.writes.load(Ordering::SeqCst),0);assert!(heap.get(&call,&"arguments".into(),values.runtime()).unwrap().same_identity(&args),"original Native call arguments stay exact");
 }

}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_p5_route_forwarding_replacement_and_switch_regression_bank(){
 use ara_tools::ContentUriPort;
 let b=run("p5",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"switch_into_child","expectedError":true}),json!({"op":"root_inspect"}),json!({"op":"list"})],vec![],false).await;assert_eq!(b.requests.len(),0);assert_eq!(b.receipts[1]["success"],false);assert_ne!(result(&b,0)["key"]["id"],result(&b,2)["record"]["key"]["id"]);
 let first=Arc::new(PolicyPort{coordination:true,writes:AtomicUsize::new(0),body:Mutex::new(None)});let fallback=Arc::new(PolicyPort{coordination:false,writes:AtomicUsize::new(0),body:Mutex::new(None)});let chain=ara_cli::host_vault::compose_ports(first.clone(),Some(fallback.clone()));assert_eq!(chain.write_policy("agent://peer").tier,ara_tools::ContentUriWriteTier::Read);
 let journal=ara_session::SessionJournal::open(file(&result(&b,2)["record"])).unwrap();let router=ara_cli::session_artifacts::ArtifactUriRouter::new(ara_cli::session_artifacts::SessionArtifacts::for_journal(&journal),Some(chain));assert_eq!(router.write_policy("agent://peer").payload,ara_tools::ContentUriWritePayload::Verbatim);assert_ne!(router.write_policy("artifact://0").tier,ara_tools::ContentUriWriteTier::Read);
 let ctx=ara_tools::ToolContext::new(b.root.clone()).with_edit(ara_edit::EditMode::Hashline,true).with_uri_port(router);ctx.set_device_only_write(true);let write=ara_tools::write::WriteTool::new(ctx.clone());let raw="  [raw#ABCD]\n1#abcd| literal\n  ";write.execute_os_values("agent://peer",raw,CancellationToken::new()).await.unwrap();assert_eq!(first.body.lock().unwrap().as_deref(),Some(raw));assert_eq!(fallback.writes.load(Ordering::Acquire),0);
 let replacement=fallback.clone();let changed=ctx.clone();let outcome=write.execute_os_values_with_update("agent://peer","never dispatched",CancellationToken::new(),Arc::new(move|_|changed.set_uri_port(replacement.clone()))).await;assert!(outcome.is_err());assert_eq!(first.writes.load(Ordering::Acquire),1);assert_eq!(fallback.writes.load(Ordering::Acquire),0);
 held_native_route_approval(&b.root,&journal).await;

 let mut responses=Vec::new();for phase in ["registered","removed","refresh","new","switch"]{responses.push(json!({"events":[tool_call(0,&format!("uri-{phase}"),"write",&json!({"path":"agent://missing-peer","content":"must remain explicit URI route"}).to_string()),finish("tool_calls"),done()]}));responses.push(answer("Observed actual URI route refusal"));}let lifecycle=run("p5-uri-lifecycle",vec![json!({"op":"uri_lifecycle"})],responses,false).await;assert_eq!(lifecycle.requests.len(),10);let phases=result(&lifecycle,0)["observations"].as_array().unwrap();assert_eq!(phases.len(),5);assert!(phases[0]["diagnostic"].as_str().unwrap().contains("read-only"));for row in &phases[1..]{assert!(row["diagnostic"].as_str().unwrap().contains("Unknown protocol: agent://"));}assert_ne!(phases[3]["state"]["sessionId"],phases[4]["state"]["sessionId"]);

 // Historical actual close Unknown must reject actual new/switch before replacing the live activation.
 let rejected_new=run("p5-transition-unknown-new",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"transition_close_unknown","transition":"new"})],vec![],false).await;
 assert_eq!(rejected_new.requests.len(),0);let rejected=result(&rejected_new,1);assert_eq!(rejected["before"]["fixturePeerActivation"],rejected["after"]["fixturePeerActivation"]);assert_eq!(rejected["before"]["sessionId"],rejected["after"]["sessionId"]);assert_eq!(rejected["before"]["sessionFile"],rejected["after"]["sessionFile"]);assert_eq!(rejected["rootOriginal"]["retainedOriginalError"],rejected["fault"]["originalError"]);
 // The switch target is another fully created and closed ordinary Native CLI root journal, in the same real cwd.
 let target=run("p5-switch-target",vec![json!({"op":"root_inspect"})],vec![],false).await;assert_eq!(target.requests.len(),0);let target_file=file(&result(&target,0)["record"]).to_owned();
 let rejected_switch=run_at_project("p5-transition-unknown-switch",vec![json!({"op":"create","init":init(&["read"])}),json!({"op":"transition_close_unknown","transition":"switch","target":target_file})],vec![],false,Some(target.root.join("project"))).await;assert_eq!(rejected_switch.requests.len(),0);let rejected=result(&rejected_switch,1);assert_eq!(rejected["before"]["fixturePeerActivation"],rejected["after"]["fixturePeerActivation"]);assert_eq!(rejected["before"]["sessionId"],rejected["after"]["sessionId"]);assert_eq!(rejected["before"]["sessionFile"],rejected["after"]["sessionFile"]);assert_eq!(rejected["rootOriginal"]["retainedOriginalError"],rejected["fault"]["originalError"]);

}

/// Existing module owners with a thin controlled Native port; never substitutes for the cold actual Host.
mod coverage_cases {
 use super::*;
 use ara_cli::host_peers::*;
 #[derive(Default)] struct NativeProbe{states:Mutex<std::collections::HashMap<RuntimeKey,NativePeerState>>,closes:AtomicUsize,deliveries:AtomicUsize}
 #[async_trait::async_trait] impl PeerNativePort for NativeProbe{
  async fn inspect(&self,runtime:&RuntimeKey)->anyhow::Result<NativePeerState>{Ok(self.states.lock().unwrap().get(runtime).cloned().unwrap_or_default())}
  async fn deliver(&self,_:&RuntimeKey,_:NativeDelivery)->anyhow::Result<()>{self.deliveries.fetch_add(1,Ordering::SeqCst);Ok(())}
  async fn revive(&self,_:&PeerRecord)->anyhow::Result<RuntimeKey>{anyhow::bail!("Clock/FIFO coverage must not manufacture a cold Host")}
  async fn close(&self,_:&RuntimeKey)->anyhow::Result<()>{self.closes.fetch_add(1,Ordering::SeqCst);Ok(())}
  async fn cancel_turn(&self,_:&RuntimeKey,_:u64)->anyhow::Result<()>{Ok(())}
  async fn drive_wake(&self,_:WakeRequest,_:CancellationToken)->anyhow::Result<WakeOutcome>{anyhow::bail!("Clock/FIFO coverage must not drive a model")}
  async fn transcript(&self,_:&PeerRecord)->anyhow::Result<Vec<Value>>{anyhow::bail!("No synthetic transcript acceptance")}
  async fn persist_parent_result(&self,_:&WakeCompletion)->anyhow::Result<ParentAdoption>{anyhow::bail!("No synthetic parent adoption")}
 }
 fn fixture()->(Arc<PeerCoordinator>,Arc<NativeProbe>){let native=Arc::new(NativeProbe::default());(PeerCoordinator::new(native.clone()),native)}
 fn add(peers:&Arc<PeerCoordinator>,id:&str,main:bool)->PeerRecord{let mut input=PeerRegistration::child("coverage-root",id,"parent",PathBuf::from(format!("{id}.jsonl")));input.runtime_generation=Some(1);input.status=PeerStatus::Idle;if main{input.kind=PeerKind::Main;input.parent_id=None;}peers.registry.register(input)}
 async fn flush(){for _ in 0..32{tokio::task::yield_now().await;}}
 pub async fn waiter_fifo(){
  let(peers,native)=fixture();let receiver=add(&peers,"Main",true);let cancel=CancellationToken::new();
  let first=peers.inbox.wait("coverage-root","Main",None,1000,true,&cancel);tokio::pin!(first);assert!(futures::poll!(first.as_mut()).is_pending(),"First original waiter must actually be registered");
  let second=peers.inbox.wait("coverage-root","Main",None,1000,true,&cancel);tokio::pin!(second);assert!(futures::poll!(second.as_mut()).is_pending(),"Second matching original waiter must actually be registered");
  let one=peers.send("coverage-root","Sub","Main","one",None,false).await.unwrap();let two=peers.send("coverage-root","Sub","Main","two",None,false).await.unwrap();assert_eq!(one.receipts[0].outcome,DeliveryOutcome::Injected);assert_eq!(two.receipts[0].outcome,DeliveryOutcome::Injected);
  let a=first.await.unwrap().unwrap();let b=second.await.unwrap().unwrap();assert_eq!(a.body,"one");assert_eq!(b.body,"two");assert_ne!(a.id,b.id);assert_eq!(native.deliveries.load(Ordering::SeqCst),0);assert_eq!(peers.inbox.unread("coverage-root","Main"),0);assert!(peers.inbox.take("coverage-root","Main",None).is_none());assert_eq!(peers.registry.get_exact(&receiver.key).unwrap().runtime,receiver.runtime);
 }
 pub async fn ttl_deadlines(){
  // Keep the original20ms arm deadline; sleep_until drives Tokio's rounded wheel without extending TTL.
  tokio::time::pause();let ttl=Duration::from_millis(20);
  {let(peers,native)=fixture();let peer=add(&peers,"ttl-expiry",false);let deadline=tokio::time::Instant::now()+ttl;peers.lifecycle.adopt(peer.key.clone(),20).unwrap();flush().await;tokio::time::advance(Duration::from_millis(19)).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Idle);assert_eq!(native.closes.load(Ordering::SeqCst),0);tokio::time::sleep_until(deadline).await;flush().await;let after=peers.registry.get_exact(&peer.key).unwrap();assert_eq!(after.status,PeerStatus::Parked);assert!(after.runtime.is_none());assert_eq!(after.session_file,peer.session_file);assert_eq!(native.closes.load(Ordering::SeqCst),1);assert!(peers.lifecycle.has(&peer.key));}
  {let(peers,native)=fixture();let peer=add(&peers,"ttl-running",false);let runtime=peer.runtime.clone().unwrap();peers.lifecycle.adopt(peer.key.clone(),20).unwrap();flush().await;native.states.lock().unwrap().insert(runtime.clone(),NativePeerState{running:true,..Default::default()});peers.registry.set_status(&peer.key,PeerStatus::Running);tokio::time::advance(ttl*10).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Running);assert_eq!(peers.registry.get_exact(&peer.key).unwrap().runtime,Some(runtime.clone()));assert_eq!(native.closes.load(Ordering::SeqCst),0);native.states.lock().unwrap().insert(runtime,NativePeerState::default());let deadline=tokio::time::Instant::now()+ttl;peers.registry.set_status(&peer.key,PeerStatus::Idle);flush().await;tokio::time::advance(Duration::from_millis(19)).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Idle);tokio::time::sleep_until(deadline).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Parked);assert_eq!(native.closes.load(Ordering::SeqCst),1);}
  {let(peers,native)=fixture();let peer=add(&peers,"ttl-cancel-before-detach",false);peers.lifecycle.adopt(peer.key.clone(),20).unwrap();flush().await;let parking=peers.lifecycle.park(&peer.key);tokio::pin!(parking);assert!(futures::poll!(parking.as_mut()).is_pending());let(kept,parked)=tokio::join!(peers.lifecycle.ensure_live(&peer.key),parking);assert_eq!(kept.unwrap(),peer.runtime.clone().unwrap());parked.unwrap();assert_eq!(native.closes.load(Ordering::SeqCst),0);assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Idle);let deadline=tokio::time::Instant::now()+ttl;flush().await;tokio::time::advance(Duration::from_millis(19)).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Idle);tokio::time::sleep_until(deadline).await;flush().await;assert_eq!(peers.registry.get_exact(&peer.key).unwrap().status,PeerStatus::Parked);assert_eq!(native.closes.load(Ordering::SeqCst),1);}
  for ttl_ms in [0,-1]{let(peers,native)=fixture();let peer=add(&peers,&format!("ttl-disabled-{ttl_ms}"),false);peers.lifecycle.adopt(peer.key.clone(),ttl_ms).unwrap();flush().await;tokio::time::advance(Duration::from_secs(60)).await;flush().await;let after=peers.registry.get_exact(&peer.key).unwrap();assert_eq!(after.status,PeerStatus::Idle);assert_eq!(after.runtime,peer.runtime);assert_eq!(native.closes.load(Ordering::SeqCst),0);assert!(peers.lifecycle.has(&peer.key));}
  tokio::time::resume();
 }
 pub async fn active_leaf_metrics(){
  for leaf in [Some("active-leaf"),None,Some("rolled-back")]{
   let registry=Arc::new(PeerRegistry::default());let roster=PeerRoster::new(registry.clone());let dir=tempfile::tempdir().unwrap();let file=dir.path().join("Branching.jsonl");
   let rows=vec![json!({"type":"session","id":"Branching"}),json!({"type":"session_init","id":"si","parentId":null}),json!({"id":"old-owner","parentId":"si","type":"message","message":{"role":"assistant","provider":"anthropic","model":"sonnet","stopReason":"stop","content":[{"type":"text","text":"older branch"}],"usage":{"input":1,"output":0,"cost":{"total":0.2}}}}),json!({"id":"m2","parentId":"si","type":"model_change","model":"openai-codex/gpt-5.6-sol","role":"task","resolvedModelIsFallback":false}),json!({"id":"active-leaf","parentId":"m2","type":"message","message":{"role":"assistant","provider":"openai-codex","model":"gpt-5.6-sol","stopReason":"stop","content":[{"type":"text","text":"active branch"}],"usage":{"input":1,"output":0,"cost":{"total":0.3}}}}),json!({"id":"late-usage","parentId":"old-owner","type":"model_usage","timestamp":"2026-08-07T11:30:00.000Z","activeLeafId":leaf,"usage":{"cost":{"total":0.1}}})];std::fs::write(&file,rows.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")+"\n").unwrap();let peer=registry.register(PeerRegistration::child("coverage-root","Branching","Main",file));roster.hydrate_history(&peer.key).await.unwrap();let history=registry.get_exact(&peer.key).unwrap().history;assert!((history["directCost"].as_f64().unwrap()-0.6).abs()<1e-12);if leaf.is_none(){assert!(history.get("metrics").is_none(),"Null activeLeaf must clear cold leaf metrics");}else{assert_eq!(history["metrics"]["requests"],1);assert!((history["metrics"]["cost"].as_f64().unwrap()-0.3).abs()<1e-12);}
  }
 }
}

#[tokio::test]
async fn p1_identity_generation_ownership_and_acceptance_bank(){ara_cli::host_peers::tests::p1_identity_generation_ownership_and_acceptance_bank().await;}

#[tokio::test]
async fn p2_fifo_four_routes_raw_uri_and_progress_bank(){ara_cli::host_peers::tests::p2_fifo_four_routes_raw_uri_and_progress_bank().await;coverage_cases::waiter_fifo().await;}

#[tokio::test]
async fn p3_park_revive_root_latches_artifacts_and_history_bank(){ara_cli::host_peers::tests::p3_park_revive_root_latches_artifacts_and_history_bank().await;coverage_cases::ttl_deadlines().await;coverage_cases::active_leaf_metrics().await;}

#[tokio::test]
async fn p4_wake_parent_wait_once_reply_failure_and_relay_bank(){ara_cli::host_peers::tests::p4_wake_parent_wait_once_reply_failure_and_relay_bank().await;}

#[tokio::test]
async fn p5_queue_switch_revoked_uri_and_shared_negative_bank(){ara_cli::host_peers::tests::p5_queue_switch_revoked_uri_and_shared_negative_bank().await;}


fn cold_init(agent:Option<&str>)->Value{
 let mut value=init(&["read"]);value["agent"]=agent.map(Value::from).unwrap_or(Value::Null);
 value["resolvedModel"]=json!("cold-fixture/fake-model");value
}
fn cold_setup(pools:Value)->Value{json!({"op":"cold_setup","task":{"agentAccountPools":pools}})}
fn cold_scope(b:&Bank,index:usize)->&Value{&result(b,index)["fixtureAgentScope"]}
fn cold_labels(b:&Bank)->Vec<String>{std::fs::read_to_string(b.root.join("cold-model-identities.jsonl")).unwrap().lines().map(|line|serde_json::from_str::<Value>(line).unwrap()["identity"].as_str().unwrap().to_owned()).collect()}
fn assert_cold_owner(scope:&Value,actual_id:&Value,effective:&str){
 for owner in ["session","factory","provider"]{assert_eq!(scope[owner]["effectiveName"],effective);assert_eq!(&scope[owner]["primarySessionId"],actual_id);}
 assert_eq!(&scope["registry"]["primarySessionId"],actual_id);
 if scope["session"]["pools"].get("cold-fixture").is_some(){
  assert_eq!(&scope["resolver"]["primarySessionId"],actual_id);
  assert_eq!(&scope["resolver"]["assignmentSessionId"],actual_id);
 }
}

/// Original Cold9 identities plus stronger actual Host/wire scope controls.
/// Global broker usage/discovery receipts are kept separate from model identity.
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_cold9_persisted_identity_live_pool_and_model_bank(){
 // 551/594/610/631: meaningful actual PeerRecord label and exact raw pools.
 for (case,agent,expected) in [("551",Some("scout"),"A"),("594",None,"A"),("610",Some("main"),"A"),("631",Some("sub"),"A"),("610-js-space",Some(" \u{feff}MaIn\t"),"B"),("631-js-space",Some(" SuB "),"B"),("551-raw-space",Some("  scout  "),"B"),("551-only-space",Some("  "),"B")]{
  let raw=agent.map(Value::from).unwrap_or(Value::Null);let label="Deliberate cold research partner";
  let setup=json!({"op":"cold_setup","displayName":label,"task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]},"main":{"cold-fixture":["account:A"]},"sub":{"cold-fixture":["account:A"]}," \u{feff}MaIn\t":{"cold-fixture":["account:B"]}," SuB ":{"cold-fixture":["account:B"]},"  scout  ":{"cold-fixture":["account:B"]},"  ":{"cold-fixture":["account:B"]}}}});
  let b=run(&format!("cold9-{case}"),vec![setup,
   json!({"op":"create","init":cold_init(agent)}),json!({"op":"send","body":"prime exact original history"}),json!({"op":"park"}),
   json!({"op":"cold_init","agent":raw}),json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send","body":"revived actual identity"}),json!({"op":"cold_root_state"}),json!({"op":"inspect"})],
   vec![answer("prime actual history"),answer("restored actual history")],false).await;
  let id=&result(&b,1)["key"]["id"];let normalized=agent.map(ara_prompt::js::trim).unwrap_or_default();
  let effective=agent.filter(|raw|!raw.is_empty()&&!normalized.eq_ignore_ascii_case("main")&&!normalized.eq_ignore_ascii_case("sub")).unwrap_or(label);
  assert_ne!(label,id.as_str().unwrap());assert_eq!(result(&b,1)["display_name"],label);assert_eq!(result(&b,9)["record"]["display_name"],label);
  assert_cold_owner(cold_scope(&b,6),id,effective);assert_eq!(cold_scope(&b,6)["session"]["definitionName"],raw);
  assert_eq!(cold_scope(&b,6)["accountOwner"],result(&b,8)["fixtureAgentScope"]["accountOwner"]);
  assert_eq!(cold_scope(&b,6)["registry"]["backend"],result(&b,8)["fixtureAgentScope"]["registry"]["backend"]);
  assert_eq!(b.requests.len(),2);if agent.is_some(){assert_eq!(cold_labels(&b),vec![expected,expected]);}else{assert!(cold_labels(&b).iter().all(|label|["A","B","K"].contains(&label.as_str())));}
 }
 // 571: reload current exact-name pool A->B on the same transcript; parent B
 // still uses its original unscoped owner, then [] and malformed policies deny.
 let b=run("cold9-571-live",vec![json!({"op":"cold_setup","deferModels":true,"task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]}}}}),json!({"op":"create","init":cold_init(Some("scout"))}),
  json!({"op":"send","body":"A model"}),json!({"op":"park"}),json!({"op":"cold_settings","task":{"agentAccountPools":{"scout":{"cold-fixture":["account:B"]}}}}),
  json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send","body":"B model"}),json!({"op":"cold_pin_root","credentialId":42}),json!({"op":"root_prompt","text":"parent still uses B"}),
  json!({"op":"park"}),json!({"op":"cold_settings","task":{"agentAccountPools":{"scout":{"cold-fixture":[]}}}}),json!({"op":"revive","expectedError":true}),
  json!({"op":"cold_settings","task":{"agentAccountPools":{"scout":{"cold-fixture":null}}}}),json!({"op":"revive","expectedError":true})],
  vec![answer("A model"),answer("B model"),answer("parent B")],false).await;
 assert_eq!(cold_scope(&b,6)["session"]["pools"]["cold-fixture"],json!(["account:B"]));assert_eq!(cold_labels(&b),vec!["A","B","B"]);assert_eq!(b.requests.len(),3);
 assert_eq!(result(&b,0)["deferredCatalog"]["startupConfigAbsent"],true);assert_eq!(result(&b,0)["deferredCatalog"]["beforeRegistry"],result(&b,0)["deferredCatalog"]["afterRegistry"]);
 assert_eq!(b.receipts[12]["success"],false);assert_eq!(b.receipts[14]["success"],false);

 // 700 +718: changed custom role is selected before the saved concrete model;
 // saved child threshold is read by the real production Host decision owner.
 let mut saved=cold_init(Some("scout"));saved["compactionThreshold"]=json!({"thresholdPercent":72.0,"thresholdTokens":-1.0});
 let setup=json!({"op":"cold_setup","parentThreshold":120000,"compaction":{"thresholdPercent":45.0,"thresholdTokens":120000},"task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]}}}});
 let b=run("cold9-700-718",vec![setup,json!({"op":"create","init":saved}),json!({"op":"send"}),json!({"op":"park"}),
  json!({"op":"cold_init","modelRole":"worker"}),json!({"op":"cold_settings","modelRoles":{"worker":"cold-fixture/role-model"}}),json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send"}),json!({"op":"cold_root_state"})],
  vec![answer("history"),answer("role actual")],false).await;
 assert_eq!(cold_scope(&b,7)["model"],"role-model");assert_eq!(cold_scope(&b,7)["childThreshold"],144000);
 assert_eq!(cold_scope(&b,7)["compactionDecisions"]["130000"],false);assert_eq!(cold_scope(&b,7)["compactionDecisions"]["144001"],true);
 assert_eq!(cold_scope(&b,9)["childThreshold"],120000);assert_eq!(cold_scope(&b,9)["compactionDecisions"]["130000"],true);assert_eq!(b.requests[1]["body"]["model"],"role-model");

 // 700 negative controls: absent role and configured-but-OAuth-ineligible role
 // both fall back to the persisted concrete, through actual ChildHost.open.
 for role in ["missing-provider/not-present","cold-key-only/role-model"]{
  let setup=cold_setup(json!({"scout":{"cold-fixture":["account:A"],"cold-key-only":[]}}));
  let b=run("cold9-700-role-denied",vec![setup,json!({"op":"create","init":cold_init(Some("scout"))}),json!({"op":"send"}),json!({"op":"park"}),json!({"op":"cold_init","modelRole":"worker"}),json!({"op":"cold_settings","modelRoles":{"worker":role}}),json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send"}),json!({"op":"cold_side_usage"})],vec![answer("history"),answer("persisted concrete"),answer("scoped side A")],false).await;
  assert_eq!(cold_scope(&b,7)["model"],"fake-model");assert_eq!(b.requests[1]["body"]["model"],"fake-model");assert_eq!(cold_labels(&b),vec!["A","A","A"]);
  let side=result(&b,9);assert_eq!(side["sideText"],"scoped side A");assert_eq!(side["scope"]["primarySessionId"],cold_scope(&b,7)["session"]["primarySessionId"]);
  let accounts=side["reports"].as_array().unwrap().iter().map(|report|report["metadata"]["accountId"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();assert_eq!(accounts,std::collections::BTreeSet::from(["A","B"]));
  let inventory=side["globalAccounts"].as_array().unwrap().iter().map(|row|row["accountId"].as_str().unwrap()).collect::<std::collections::BTreeSet<_>>();assert_eq!(inventory,std::collections::BTreeSet::from(["A","B"]));
 }

 // 748 and same-current alias regression: live @default changes after parent
 // launch, while the persisted concrete remains the current model A.
 let setup=json!({"op":"cold_setup","rootAlias":true,"task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]}}},"modelRoles":{"default":"cold-fixture/fake-model"}});
 let b=run("cold9-748-alias",vec![setup,json!({"op":"create","init":cold_init(Some("scout"))}),json!({"op":"send"}),json!({"op":"park"}),json!({"op":"cold_init","modelRole":"default"}),
  json!({"op":"cold_settings","modelRoles":{"default":"cold-fixture/role-model"}}),json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send"})],vec![answer("history"),answer("saved A")],false).await;
 assert_eq!(cold_scope(&b,7)["model"],"fake-model");assert_eq!(b.requests[1]["body"]["model"],"fake-model");assert_eq!(cold_labels(&b),vec!["A","A"]);
 // Same saved-concrete contract with provider-prefixed catalog ID and a distinct wire ID.
 let setup=json!({"op":"cold_setup","rootAlias":true,"materializedCatalog":true,"task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]}}},"modelRoles":{"default":"cold-fixture/cold-fixture/catalog-prefixed"}});
 let mut saved=cold_init(Some("scout"));saved.as_object_mut().unwrap().remove("resolvedModel");
 let b=run("cold9-748-catalog-wire",vec![setup,json!({"op":"create","init":saved}),json!({"op":"send"}),json!({"op":"park"}),json!({"op":"cold_settings","modelRoles":{"default":"cold-fixture/role-model"}}),json!({"op":"revive"}),json!({"op":"cold_state"}),json!({"op":"send"})],vec![answer("catalog history"),answer("saved catalog")],false).await;
 assert_eq!(cold_scope(&b,6)["model"],"wire-cold-override");assert_eq!(b.requests.len(),2);assert!(b.requests.iter().all(|request|request["body"]["model"]=="wire-cold-override"));assert_eq!(cold_labels(&b),vec!["A","A"]);
 let saved=ara_session::SessionJournal::open(file(result(&b,1))).unwrap();assert_eq!(saved.latest_child_init().unwrap().unwrap().resolved_model.as_deref(),Some("cold-fixture/cold-fixture/catalog-prefixed"));
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_cold9_actual_primary_new_switch_commit_and_failed_switch_bank(){
 let b=run("cold9-adoption",vec![json!({"op":"cold_setup","rootScope":true,"rootPools":{"cold-fixture":["account:A"]}}),json!({"op":"root_prompt","text":"materialize actual scoped original"}),
  json!({"op":"cold_adopt_probe"}),json!({"op":"root_prompt","text":"actual scoped switched original"})],vec![answer("original"),answer("fresh actual primary"),answer("same owner after native publication failure"),answer("switched")],false).await;
 let observed=result(&b,2);let before=&observed["before"];let fresh=&observed["fresh"];let failed=&observed["afterFailed"];let switched=&observed["switched"];
 assert_ne!(before["sessionId"],fresh["sessionId"]);assert_eq!(before["sessionId"],switched["sessionId"]);
 for state in [before,fresh,failed,switched]{assert_cold_owner(&state["fixtureAgentScope"],&state["sessionId"],"scoped-fixture-root");}
 assert_eq!(fresh["sessionId"],failed["sessionId"]);assert_eq!(fresh["fixtureAgentScope"],failed["fixtureAgentScope"]);
 for field in ["accountOwner","registry"]{if field=="registry"{assert_eq!(before["fixtureAgentScope"][field]["backend"],fresh["fixtureAgentScope"][field]["backend"]);}else{assert_eq!(before["fixtureAgentScope"][field],fresh["fixtureAgentScope"][field]);}}
 for key in ["oldKey","newKey","switchKey"]{assert_eq!(observed[key]["identity"],"A");}
 assert_eq!(observed["newKey"]["actualSessionId"],fresh["sessionId"]);assert_eq!(observed["newKey"]["registry"]["primarySessionId"],fresh["sessionId"]);
 assert_eq!(cold_labels(&b),vec!["A","A","A","A"]);assert_eq!(b.requests.len(),4);
 assert!(!observed["publicationFailure"].as_str().unwrap().is_empty());
 let publication=&observed["publicationPrepared"];assert_eq!(publication["publicationAttempted"],true);assert!(!publication["nativeError"].as_str().unwrap().is_empty());
 assert_eq!(publication["preparedPrimary"]["actualId"],fresh["sessionId"]);assert_eq!(publication["preparedPrimary"]["primaryId"],fresh["sessionId"]);
 assert_eq!(publication["ownerBefore"],publication["ownerAfter"]);assert_eq!(publication["preparedPrimary"]["oldBinding"],publication["ownerBefore"]);
 assert_eq!(publication["ownerBefore"]["configProvider"],publication["ownerBefore"]["snapshotProvider"]);
 assert_eq!(observed["publicationBefore"]["fixtureAgentScope"],observed["publicationAfter"]["fixtureAgentScope"]);
 assert_eq!(observed["publicationBefore"]["sessionId"],fresh["sessionId"]);assert_eq!(observed["publicationAfter"]["sessionId"],fresh["sessionId"]);
}

// Original unchanged regression files share the one Peer image. Their native
// test names gain only this module prefix; original assertions/limits remain.
#[path="model_route.rs"] mod cold_route_regression;
#[path="model_registry.rs"] mod cold_registry_regression;
#[path="codex_auto_reset.rs"] mod cold_auto_reset_regression;
#[path="codex_reset_controller.rs"] mod cold_reset_controller_regression;


/// 766: the saved subagent role owns its fallback after an effort change.
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_cold9_persisted_fallback_survives_changed_effort_bank(){
 let mut saved=cold_init(Some("scout"));saved["retryFallback"]=json!({"primary":"cold-fixture/fake-model:high","chain":["cold-fixture/fallback-model"]});
 let setup=json!({"op":"cold_setup","task":{"agentAccountPools":{"scout":{"cold-fixture":["account:A"]}}},"retry":{"enabled":true,"maxRetries":0,"modelFallback":true,"usageAwareFallback":false,"baseDelayMs":0,"maxDelayMs":0}});
 let failure=json!({"events":[{"data":{"error":{"message":"controlled cold fallback capacity failure","type":"server_error","code":"overloaded"}}}],"end":"close"});
 let b=run("cold9-766",vec![setup,json!({"op":"create","init":saved}),json!({"op":"send","body":"prime saved fallback history"}),json!({"op":"park"}),json!({"op":"revive"}),json!({"op":"cold_thinking","level":"xhigh"}),json!({"op":"cold_state"}),json!({"op":"send","body":"execute original persisted fallback"})],vec![answer("history"),failure.clone(),failure,answer("saved fallback answer")],false).await;
 assert_eq!(result(&b,6)["configuredThinkingLevel"],"xhigh");
 // The provider may replay one uncommitted transient failure before the Host
 // sees an error. Both identical failures must precede the real Host fallback.
 assert_eq!(b.requests.iter().map(|request|request["body"]["model"].as_str().unwrap()).collect::<Vec<_>>(),vec!["fake-model","fake-model","fake-model","fallback-model"]);
 assert_eq!(b.requests[1]["body"]["reasoning_effort"],"xhigh");
 assert_eq!(b.requests[2]["body"]["reasoning_effort"],"xhigh");
 assert_eq!(b.requests.last().unwrap()["body"]["model"],"fallback-model");
 assert!(b.requests.iter().any(|request|request["body"]["model"]=="fake-model"));
 assert!(cold_labels(&b).iter().all(|identity|identity=="A"));
}


#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn peer_cold9_legal_non_peer_legacy_open_and_nested_own_pool_bank(){
 for agent in [Value::Null,json!("main"),json!("sub")]{
  let b=run("cold9-non-peer",vec![cold_setup(json!({})),json!({"op":"create","init":cold_init(Some("task"))}),json!({"op":"send"}),json!({"op":"park"}),json!({"op":"cold_init","agent":agent}),json!({"op":"cold_non_peer"}),json!({"op":"revive"}),json!({"op":"cold_state"})],vec![answer("history")],false).await;
  let id=&result(&b,1)["key"]["id"];
  assert_eq!(&result(&b,5)["sessionId"],id);assert_eq!(&result(&b,5)["fixtureAgentScope"]["session"]["effectiveName"],id);
  assert_cold_owner(cold_scope(&b,7),id,id.as_str().unwrap());assert_eq!(b.requests.len(),1);
 }
 let b=run("cold9-nested",vec![cold_setup(json!({"scout":{"cold-fixture":["account:A"]},"worker":{"cold-fixture":["account:B"]}})),
  json!({"op":"create","alias":"parent","init":cold_init(Some("scout"))}),json!({"op":"send","alias":"parent"}),
  json!({"op":"create_nested","from":"parent","alias":"nested","init":cold_init(Some("worker"))}),json!({"op":"cold_state","alias":"parent"}),json!({"op":"cold_state","alias":"nested"}),json!({"op":"send","alias":"nested"}),json!({"op":"cold_root_state"})],vec![answer("parent A"),answer("nested B"),answer("root observed nested completion")],false).await;
 assert_eq!(cold_scope(&b,4)["session"]["pools"]["cold-fixture"],json!(["account:A"]));assert_eq!(cold_scope(&b,5)["session"]["pools"]["cold-fixture"],json!(["account:B"]));
 assert_eq!(cold_scope(&b,4)["accountOwner"],cold_scope(&b,5)["accountOwner"]);assert_eq!(cold_scope(&b,4)["registry"]["backend"],cold_scope(&b,5)["registry"]["backend"]);
 // The root sent the nested message, so the automatic completion relay wakes
 // its ordinary unscoped K route. Keep both child identities exact separately.
 let identities=cold_labels(&b);assert_eq!(&identities[..2],&["A".to_owned(),"B".to_owned()]);
 assert_eq!(identities,vec!["A","B","K"]);assert_eq!(b.requests.len(),3);
 assert_eq!(result(&b,3)["parent_id"],result(&b,1)["key"]["id"]);
 assert_eq!(result(&b,7)["sessionId"],result(&b,1)["key"]["root"]);
 let nested_id=result(&b,3)["key"]["id"].as_str().unwrap();let root_id=result(&b,7)["sessionId"].as_str().unwrap();
 let root_events=actual_events(&b.root.join("root.events.jsonl"));
 assert!(root_events.iter().any(|event|event["type"]=="message_end"&&event["message"]["customType"]=="peer-wake"&&event["message"]["details"]["messages"].as_array().is_some_and(|messages|messages.iter().any(|message|message["from"]==nested_id&&message["to"]==root_id&&message["message"]=="nested B"&&message["wakeRelay"]==true))));
 let root_journal=journal_bodies(Path::new(result(&b,7)["sessionFile"].as_str().unwrap()));
 assert!(root_journal.iter().any(|message|message["role"]=="assistant"&&message["content"][0]["text"]=="root observed nested completion"&&message["provider"]=="cold-fixture"&&message["model"]==b.requests[2]["body"]["model"]));
 assert!(b.requests[2]["body"]["messages"].to_string().contains(nested_id));
 assert!(b.requests[2]["body"]["messages"].to_string().contains("nested B"));
}
