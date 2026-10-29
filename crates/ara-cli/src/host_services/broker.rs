//! Actual project broker. Adapted OMP launch/broker.ts at b91db66e (MIT).
//! Map locks select a record only; blocking OS/file work never holds that lock.
use super::{process::{self,ProcessHandle},protocol::*,store::{self,BrokerLease,ProjectScope,Store,StoredRecord}};
use anyhow::{Context,Result,bail};
use std::{collections::HashMap,path::PathBuf,sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio::io::{AsyncBufReadExt,AsyncRead,AsyncReadExt,AsyncWrite,AsyncWriteExt,BufReader};

type Reader=Box<dyn AsyncRead+Unpin+Send>;
type Writer=Box<dyn AsyncWrite+Unpin+Send>;
struct Managed {record:StoredRecord,process:Option<Arc<ProcessHandle>>,stop_requested:bool}
type Record=Arc<tokio::sync::Mutex<Managed>>;
struct Broker {
    store:Store, records:Mutex<HashMap<String,Record>>, subscriptions:Mutex<HashMap<String,String>>,
    name_gates:Mutex<HashMap<String,Arc<tokio::sync::Mutex<()>>>>,
    #[cfg(feature="test-fixture")] drop_send:std::sync::atomic::AtomicBool,
    #[cfg(feature="test-fixture")] drop_ack:std::sync::atomic::AtomicBool,
    #[cfg(feature="test-fixture")] hold_send:std::sync::atomic::AtomicBool,
}
async fn persist(store:&Store,record:&StoredRecord)->Result<()> {
    let store=store.clone();let record=record.clone();tokio::task::spawn_blocking(move||store.save(&record)).await??;Ok(())
}
impl Broker {
    async fn open(scope:ProjectScope)->Result<Arc<Self>>{
        let store=Store::new(scope)?;let copy=store.clone();let loaded=tokio::task::spawn_blocking(move||copy.load()).await??;
        let broker=Arc::new(Self{store,records:Mutex::new(HashMap::new()),subscriptions:Mutex::new(HashMap::new()),name_gates:Mutex::new(HashMap::new()),
            #[cfg(feature="test-fixture")] drop_send:std::sync::atomic::AtomicBool::new(std::env::var_os("ARA_SERVICE_TEST_DROP_SEND_REPLY_ONCE").is_some()),
            #[cfg(feature="test-fixture")] drop_ack:std::sync::atomic::AtomicBool::new(std::env::var_os("ARA_SERVICE_TEST_DROP_ACK_ONCE").is_some()),
            #[cfg(feature="test-fixture")] hold_send:std::sync::atomic::AtomicBool::new(std::env::var_os("ARA_SERVICE_TEST_HOLD_SEND_REPLY_ONCE").is_some()),
        });
        for mut record in loaded {
            let mut recovered_live=false;
            if !record.snapshot.state.terminal(){
                if let Some(identity)=&record.process {
                    let current=process::inspect_identity(identity.pid)?;
                    if current.as_ref()==Some(identity){
                        if record.spec.detached {recovered_live=true;}
                        else {process::stop_recovered(identity,500).await?;}
                    }
                }
                if !recovered_live {
                    let completion=record.snapshot.state!=ServiceState::Stopping;
                    record.snapshot.state=ServiceState::Exited;record.snapshot.pid=None;record.snapshot.exited_at=Some(now());
                    record.snapshot.exit_reason=Some("broker recovered an untracked supervised generation".into());
                    if completion&&record.completion_capable&&record.pending.is_empty(){
                        if let Some(owner)=&record.snapshot.owner {record.pending.push(Completion{completion_id:uuid::Uuid::new_v4().to_string(),owner:owner.clone(),daemon:record.snapshot.clone()});}
                    }
                }
            }
            if record.completion_capable {
                if let (Some(owner),Some(subscription))=(&record.snapshot.owner,&record.subscription){broker.subscriptions.lock().unwrap().insert(owner.clone(),subscription.clone());}
            }
            persist(&broker.store,&record).await?;
            let name=record.spec.name.clone();let managed=Arc::new(tokio::sync::Mutex::new(Managed{record,process:None,stop_requested:false}));
            broker.records.lock().unwrap().insert(name,managed.clone());
            if recovered_live {broker.monitor_recovered(managed);}
        }
        Ok(broker)
    }
    fn select(&self,name:&str)->Result<Record>{
        validate_name(name).map_err(anyhow::Error::msg)?;
        self.records.lock().unwrap().get(name).cloned().with_context(||format!("Service {name} was not found"))
    }
    fn all(&self)->Vec<Record>{self.records.lock().unwrap().values().cloned().collect()}
    async fn finish_locked(&self,managed:&mut Managed,exit:ProcessExit)->Result<()> {
        if managed.record.snapshot.state.terminal(){return Ok(());}
        let snapshot=&mut managed.record.snapshot;
        snapshot.state=if !managed.stop_requested&&(exit.reason.is_some()||exit.code.is_some_and(|c|c!=0)){ServiceState::Failed}else{ServiceState::Exited};
        snapshot.pid=None;snapshot.exited_at=Some(now());snapshot.exit_code=exit.code;snapshot.exit_reason=exit.reason;
        managed.process=None;
        if !managed.stop_requested&&managed.record.completion_capable {
            if let Some(owner)=&snapshot.owner {
                managed.record.pending.push(Completion{completion_id:uuid::Uuid::new_v4().to_string(),owner:owner.clone(),daemon:snapshot.clone()});
            }
        }
        persist(&self.store,&managed.record).await
    }
    fn monitor(self:&Arc<Self>,record:Record,process:Arc<ProcessHandle>,generation:u64){
        let broker=self.clone();tokio::spawn(async move{
            let mut terminal=process.terminal();
            loop {
                let exit=terminal.borrow().clone();
                if let Some(exit)=exit {
                    let mut managed=record.lock().await;
                    if managed.record.snapshot.generation==generation {
                        if let Err(error)=broker.finish_locked(&mut managed,exit).await{eprintln!("ara: service terminal persistence failed: {error:#}");}
                    }break;
                }
                if terminal.changed().await.is_err(){break;}
            }
        });
    }
    fn monitor_recovered(self:&Arc<Self>,record:Record){
        let broker=self.clone();tokio::spawn(async move{
            loop {
                tokio::time::sleep(Duration::from_millis(100)).await;
                let (identity,generation,state)={let m=record.lock().await;(m.record.process.clone(),m.record.snapshot.generation,m.record.snapshot.state)};
                if state.terminal(){break;}
                let Some(identity)=identity else{break;};
                let observed=tokio::task::spawn_blocking(move||process::inspect_identity(identity.pid).map(|found|found.as_ref()==Some(&identity))).await;
                match observed {
                    Ok(Ok(true))=>{},
                    Ok(Ok(false))=>{let mut m=record.lock().await;if m.record.snapshot.generation==generation {if let Err(error)=broker.finish_locked(&mut m,ProcessExit{code:None,reason:Some("recovered detached process exited; exit code unavailable".into())}).await{eprintln!("ara: service recovery persistence failed: {error:#}");}}break;},
                    Ok(Err(error))=>{eprintln!("ara: cannot observe detached service: {error:#}");},
                    Err(error)=>{eprintln!("ara: detached observation worker: {error}");break;},
                }
            }
        });
    }
    async fn launch(self:&Arc<Self>,record:&Record)->Result<()> {
        let mut managed=record.lock().await;
        let generation=managed.record.snapshot.generation+1;
        managed.record.snapshot.generation=generation;
        managed.record.snapshot.started_at=now().max(managed.record.snapshot.started_at+1);
        managed.record.snapshot.state=ServiceState::Starting;
        managed.record.snapshot.exited_at=None;managed.record.snapshot.exit_code=None;managed.record.snapshot.exit_reason=None;
        managed.stop_requested=false;
        persist(&self.store,&managed.record).await?;
        let spec=managed.record.spec.clone();let log=self.store.dir(&spec.name)?.join("output.log");
        let created=tokio::task::spawn_blocking(move||process::spawn(&spec,&log)).await?;
        match created {
            Ok(process)=>{
                managed.record.snapshot.pid=Some(process.pid());managed.record.process=Some(process.identity());
                managed.record.snapshot.state=ServiceState::Running;managed.process=Some(process.clone());
                let result=persist(&self.store,&managed.record).await;
                drop(managed);self.monitor(record.clone(),process,generation);result
            },
            Err(error)=>{
                self.finish_locked(&mut managed,ProcessExit{code:None,reason:Some(format!("start failed: {error:#}"))}).await?;
                Err(error)
            },
        }
    }
    async fn stop_locked(&self,managed:&mut Managed,timeout_ms:u64)->Result<()> {
        if managed.record.snapshot.state.terminal(){return Ok(());}
        managed.stop_requested=true;managed.record.snapshot.state=ServiceState::Stopping;
        persist(&self.store,&managed.record).await?;
        let exit=if let Some(process)=&managed.process {process.stop(timeout_ms).await?}
            else if let Some(identity)=&managed.record.process {process::stop_recovered(identity,timeout_ms).await?}
            else {bail!("Service has no observable process identity");};
        self.finish_locked(managed,exit).await
    }
    async fn apply(self:&Arc<Self>,operation:Operation,effected:&std::sync::atomic::AtomicBool)->Result<Reply>{
        let name=match &operation{Operation::Start{spec,..}=>Some(&spec.name),Operation::Send{name,..}|Operation::Mode{name,..}|Operation::Stop{name,..}=>Some(name),_=>None};
        let gate=if let Some(name)=name {validate_name(name).map_err(anyhow::Error::msg)?;Some(self.name_gates.lock().unwrap().entry(name.clone()).or_default().clone())}else{None};
        let _name_guard=if let Some(gate)=gate{Some(gate.lock_owned().await)}else{None};
        match operation {
            Operation::Ping=>Ok(Reply::Pong),
            Operation::List=>{
                let mut services=Vec::new();for r in self.all(){services.push(r.lock().await.record.snapshot.clone());}
                services.sort_by(|a,b|match(a.state.terminal(),b.state.terminal()){
                    (false,false)=>a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)),
                    (true,true)=>b.exited_at.unwrap_or(b.created_at).cmp(&a.exited_at.unwrap_or(a.created_at)).then(a.name.cmp(&b.name)),
                    (false,true)=>std::cmp::Ordering::Less,(true,false)=>std::cmp::Ordering::Greater,
                });let active=services.iter().take_while(|entry|!entry.state.terminal()).count();services.truncate(active+10);Ok(Reply::List{services})
            },
            Operation::Find{name}=>{let record=self.select(&name)?;let daemon=record.lock().await.record.snapshot.clone();Ok(Reply::Service{daemon})},
            Operation::Start{spec,owner,replace}=>{
                spec.validate().map_err(anyhow::Error::msg)?;
                let prior=self.records.lock().unwrap().get(&spec.name).cloned();
                let mut pending=Vec::new();
                if let Some(prior)=prior {
                    let mut old=prior.lock().await;
                    if !old.record.snapshot.state.terminal(){
                        if !replace {bail!("Service {} already exists",spec.name);}
                        effected.store(true,std::sync::atomic::Ordering::Release);self.stop_locked(&mut old,2000).await?;
                    }
                    if !old.record.pending.is_empty()&&!replace{bail!("Service {} has unacknowledged completion notifications",spec.name);}
                    // Source keeps old completions in its owner-level pending
                    // map even when replace selects a new service record. Our
                    // disk record carries those owner/generation tagged entries.
                    pending=old.record.pending.clone();
                }
                let time=now();let subscription=owner.as_ref().and_then(|o|self.subscriptions.lock().unwrap().get(o).cloned());
                let snapshot=ServiceSnapshot{name:spec.name.clone(),id:uuid::Uuid::new_v4().to_string(),state:ServiceState::Starting,pid:None,created_at:time,started_at:time,generation:0,exited_at:None,exit_code:None,exit_reason:None,owner,persist:spec.persist,detached:spec.detached};
                let record=Arc::new(tokio::sync::Mutex::new(Managed{record:StoredRecord{spec:spec.clone(),snapshot,process:None,completion_capable:subscription.is_some(),subscription,pending},process:None,stop_requested:false}));
                // The request admission reservation below is held only while
                // selecting the exact record; per-name start serializes separately.
                effected.store(true,std::sync::atomic::Ordering::Release);self.records.lock().unwrap().insert(spec.name,record.clone());
                self.launch(&record).await?;let snapshot=record.lock().await.record.snapshot.clone();Ok(Reply::Service{daemon:snapshot})
            },
            Operation::Send{name,data}=>{
                if data.len()>MAX_INPUT {bail!("Service stdin exceeds limit");}
                let record=self.select(&name)?;let managed=record.lock().await;
                if managed.record.snapshot.state.terminal()||managed.record.snapshot.state==ServiceState::Stopping{bail!("Service {name} is {:?}",managed.record.snapshot.state);}
                let process=managed.process.clone().context("Service stdin is unavailable for recovered detached process")?;
                let bytes=data.into_bytes();effected.store(true,std::sync::atomic::Ordering::Release);tokio::task::spawn_blocking(move||process.write(&bytes)).await??;
                Ok(Reply::Service{daemon:managed.record.snapshot.clone()})
            },
            Operation::Stop{name,timeout_ms}=>{
                if timeout_ms>60_000{bail!("Service stop timeout exceeds limit");}
                let record=self.select(&name)?;let mut managed=record.lock().await;effected.store(true,std::sync::atomic::Ordering::Release);self.stop_locked(&mut managed,timeout_ms).await?;
                Ok(Reply::Service{daemon:managed.record.snapshot.clone()})
            },
            Operation::Mode{name,mode}=>{
                let record=self.select(&name)?;
                let relaunch={let mut managed=record.lock().await;
                    if managed.record.snapshot.state.terminal()||managed.record.snapshot.state==ServiceState::Stopping{bail!("Service {name} is {:?}",managed.record.snapshot.state);}
                    if mode==ServiceMode::Detached&&super::shared_browser_name(&name){bail!("Project-shared browser service cannot be detached");}
                    if managed.record.spec.detached&&mode==ServiceMode::Session{bail!("Detached daemon {name} must remain persistent");}
                    effected.store(true,std::sync::atomic::Ordering::Release);
                    if mode==ServiceMode::Detached&&!managed.record.spec.detached {
                        self.stop_locked(&mut managed,2000).await?;
                        managed.record.spec.detached=true;managed.record.spec.pty=false;managed.record.spec.persist=true;
                        managed.record.snapshot.detached=true;managed.record.snapshot.persist=true;true
                    }else{
                        managed.record.spec.persist=mode!=ServiceMode::Session;managed.record.snapshot.persist=managed.record.spec.persist;
                        persist(&self.store,&managed.record).await?;false
                    }
                };
                if relaunch{self.launch(&record).await?;}
                let daemon=record.lock().await.record.snapshot.clone();Ok(Reply::Service{daemon})
            },
            Operation::Subscribe{owner,subscription}=>{
                if owner.is_empty()||subscription.is_empty(){bail!("Completion owner and subscription are required");}
                self.subscriptions.lock().unwrap().insert(owner.clone(),subscription.clone());
                for record in self.all(){let mut managed=record.lock().await;if managed.record.snapshot.owner.as_ref()==Some(&owner){
                    managed.record.completion_capable=true;managed.record.subscription=Some(subscription.clone());persist(&self.store,&managed.record).await?;
                }}Ok(Reply::Pong)
            },
            Operation::Poll{owner,subscription}=>{
                if self.subscriptions.lock().unwrap().get(&owner)!=Some(&subscription){bail!("Completion subscription was superseded");}
                let mut entries=Vec::new();let mut encoded=1024usize;
                for record in self.all(){let managed=record.lock().await;for pending in managed.record.pending.iter().filter(|p|p.owner==owner){
                    let size=serde_json::to_vec(pending)?.len()+1;
                    if entries.len()>=256||encoded.saturating_add(size)>=MAX_FRAME{break;}
                    encoded+=size;entries.push(pending.clone());
                }if entries.len()>=256||encoded>=MAX_FRAME-1024{break;}}
                Ok(Reply::Completions{entries})
            },
            Operation::Ack{owner,subscription,completion_ids}=>{
                if self.subscriptions.lock().unwrap().get(&owner)!=Some(&subscription){bail!("Completion subscription was superseded");}
                let mut removed=0;for record in self.all(){let mut managed=record.lock().await;let original=managed.record.pending.clone();let before=original.len();
                    managed.record.pending.retain(|p|p.owner!=owner||!completion_ids.contains(&p.completion_id));
                    if before!=managed.record.pending.len(){let count=before-managed.record.pending.len();if let Err(error)=persist(&self.store,&managed.record).await{managed.record.pending=original;return Err(error);}removed+=count;}
                }Ok(Reply::Acked{removed})
            },
            Operation::Unsubscribe{owner,subscription,preserve_pending}=>{
                if self.subscriptions.lock().unwrap().get(&owner)!=Some(&subscription){return Ok(Reply::Pong);}
                if !preserve_pending{
                    self.subscriptions.lock().unwrap().remove(&owner);
                    for record in self.all(){let mut managed=record.lock().await;let mut changed=false;
                        if managed.record.snapshot.owner.as_ref()==Some(&owner){managed.record.completion_capable=false;managed.record.subscription=None;changed=true;}
                        let before=managed.record.pending.len();managed.record.pending.retain(|entry|entry.owner!=owner);
                        if changed||before!=managed.record.pending.len(){persist(&self.store,&managed.record).await?;}
                    }
                }Ok(Reply::Pong)
            },
        }
    }
    async fn connection(self:Arc<Self>,reader:Reader,mut writer:Writer,token:Arc<String>){
        let mut reader=BufReader::new(reader.take((MAX_FRAME+1)as u64));let mut bytes=Vec::new();
        let read=tokio::time::timeout(Duration::from_secs(10),reader.read_until(b'\n',&mut bytes)).await;
        if !matches!(read,Ok(Ok(n)) if n>0)||bytes.len()>MAX_FRAME{return;}
        let request:Request=match serde_json::from_slice(&bytes){Ok(v)=>v,Err(_)=>return};
        let equal=request.token.len()==token.len()&&request.token.bytes().zip(token.bytes()).fold(0u8,|n,(a,b)|n|(a^b))==0;
        // Controlled ACK loss happens BEFORE broker adoption; the Session's
        // bounded seen window must reACK without appending another custom body.
        #[cfg(feature="test-fixture")]
        if equal&&request.version==VERSION&&matches!(&request.operation,Operation::Ack{..})&&self.drop_ack.swap(false,std::sync::atomic::Ordering::AcqRel){return;}
        let effected=std::sync::atomic::AtomicBool::new(false);
        #[cfg(feature="test-fixture")] let send=matches!(&request.operation,Operation::Send{..});
        let reply=if request.version!=VERSION||!equal {
            Err(WireError{code:"unauthorized".into(),message:"Invalid service protocol version or authentication".into(),outcome_unknown:false})
        }else{
            self.apply(request.operation,&effected).await.map_err(|error|WireError{code:if error.to_string().contains("was not found"){"not_found".into()}else{"operation_failed".into()},message:format!("{error:#}"),outcome_unknown:effected.load(std::sync::atomic::Ordering::Acquire)})
        };
        #[cfg(feature="test-fixture")] if send&&reply.is_ok()&&self.drop_send.swap(false,std::sync::atomic::Ordering::AcqRel){return;}
        #[cfg(feature="test-fixture")] if send&&reply.is_ok()&&self.hold_send.swap(false,std::sync::atomic::Ordering::AcqRel){tokio::time::sleep(Duration::from_secs(1)).await;}
        let response=Response::new(request.id,reply);
        if let Ok(mut bytes)=serde_json::to_vec(&response){bytes.push(b'\n');let _=writer.write_all(&bytes).await;let _=writer.flush().await;}
    }
    async fn can_idle_shutdown(&self)->Result<bool>{
        for record in self.all(){let m=record.lock().await;if m.record.spec.persist&&!m.record.snapshot.state.terminal(){return Ok(false);}}
        let scope=self.store.scope.clone();Ok(!tokio::task::spawn_blocking(move||store::live_presence(&scope)).await??)
    }
    async fn shutdown(&self)->Result<()>{
        for record in self.all(){let mut m=record.lock().await;if !m.record.spec.detached{self.stop_locked(&mut m,5000).await?;}}
        Ok(())
    }
}
struct Listener {
    #[cfg(windows)] pipe:tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(unix)] socket:tokio::net::UnixListener,
    endpoint:String,
}
impl Listener {
    fn bind(scope:&ProjectScope)->Result<Self>{
        #[cfg(windows)]{
            let pipe=tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).reject_remote_clients(true).create(&scope.endpoint)?;
            Ok(Self{pipe,endpoint:scope.endpoint.clone()})
        }
        #[cfg(unix)]{
            use std::os::unix::fs::PermissionsExt;
            let _=std::fs::remove_file(&scope.endpoint);
            let socket=tokio::net::UnixListener::bind(&scope.endpoint)?;
            std::fs::set_permissions(&scope.endpoint,std::fs::Permissions::from_mode(0o600))?;
            Ok(Self{socket,endpoint:scope.endpoint.clone()})
        }
    }
    async fn accept(&mut self)->Result<(Reader,Writer)>{
        #[cfg(windows)]{
            self.pipe.connect().await?;
            let next=tokio::net::windows::named_pipe::ServerOptions::new().reject_remote_clients(true).create(&self.endpoint)?;
            let pipe=std::mem::replace(&mut self.pipe,next);let(reader,writer)=tokio::io::split(pipe);Ok((Box::new(reader),Box::new(writer)))
        }
        #[cfg(unix)]{let(socket,_)=self.socket.accept().await?;let(reader,writer)=socket.into_split();Ok((Box::new(reader),Box::new(writer)))}
    }
}
impl Drop for Listener{fn drop(&mut self){#[cfg(unix)]let _=std::fs::remove_file(&self.endpoint);}}

/// Only the explicit internal worker entrance invokes this function.
pub async fn run_worker_from_environment()->Result<()>{
    let project=PathBuf::from(std::env::var_os("ARA_SERVICE_PROJECT").context("Service worker project missing")?);
    let runtime=PathBuf::from(std::env::var_os("ARA_SERVICE_RUNTIME_ROOT").context("Service worker runtime root missing")?);
    let scope=ProjectScope::new(&project,&runtime)?;
    let _lease=BrokerLease::acquire(&scope)?;
    let token=Arc::new(scope.token()?);let broker=Broker::open(scope.clone()).await?;
    let identity=process::inspect_identity(std::process::id())?.context("Service broker process identity missing")?;
    // Publish readiness only after the actual endpoint exists. Clients still
    // retry pre-dispatch busy opens; this metadata is never a request receipt.
    let mut listener=Listener::bind(&scope)?;
    store::atomic_json(&scope.runtime.join("broker.json"),&identity)?;
    let mut links=tokio::task::JoinSet::new();let mut timer=tokio::time::interval(Duration::from_millis(100));let mut idle=None::<Instant>;
    loop {tokio::select!{
        accepted=listener.accept()=>{let(reader,writer)=accepted?;let owner=broker.clone();let token=token.clone();links.spawn(async move{owner.connection(reader,writer,token).await;});idle=None;},
        _=timer.tick()=>{while links.try_join_next().is_some(){} if links.is_empty()&&broker.can_idle_shutdown().await? {let since=idle.get_or_insert_with(Instant::now);if since.elapsed()>=Duration::from_secs(3){broker.shutdown().await?;break;}}else{idle=None;}},
    }}
    links.abort_all();while links.join_next().await.is_some(){}Ok(())
}
