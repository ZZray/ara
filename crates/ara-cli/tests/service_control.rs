//! Four coarse actual-process banks for HOST-SERVICE-CONTROL.
//! Controlled provider/ACK faults are not real-model acceptance. Run delivered
//! code with --features test-fixture on Windows and Linux after the lock gate.
#![cfg(all(feature="test-fixture",any(windows,target_os="linux")))]
use ara_cli::host_services::{ProjectServiceClient,ServiceError,ServiceMode,ServicePolicy,ServiceSpec,ServiceState};
use ara_cli::host_services::protocol::{Operation,Reply,Request,Response,VERSION};
use ara_testkit::{FakeUpstream,Script,chunks::*};
use serde_json::{Value,json};
use std::{collections::BTreeMap,future::Future,io::{Read,Write},path::{Path,PathBuf},process::Stdio,sync::{Arc,Mutex},time::{Duration,Instant}};
use tokio::{io::{AsyncBufReadExt,AsyncReadExt,AsyncWriteExt,BufReader},process::{Child,ChildStdin,Command}};
use tokio_util::sync::CancellationToken;
const BIN:&str=env!("CARGO_BIN_EXE_ara");
const WAIT:Duration=Duration::from_secs(25); // source default stop: 5+6+6 seconds plus IO

async fn eventually<F,Fut>(label:&str,mut check:F) where F:FnMut()->Fut,Fut:Future<Output=bool>{let start=Instant::now();while !check().await{assert!(start.elapsed()<WAIT,"timed out: {label}");tokio::time::sleep(Duration::from_millis(25)).await;}}
fn exists(path:&Path)->bool{path.is_file()}
fn bytes(path:&Path)->Vec<u8>{std::fs::read(path).unwrap_or_default()}
fn write_file(path:&Path,data:&[u8]){std::fs::write(path,data).unwrap();}
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}

struct Env{home:tempfile::TempDir,work:tempfile::TempDir,clients:Mutex<Vec<Arc<ProjectServiceClient>>>}
impl Env{
    fn new()->Self{Self{home:tempfile::Builder::new().prefix("ara-service-home-").tempdir().unwrap(),work:tempfile::Builder::new().prefix("ara-service-project-").tempdir().unwrap(),clients:Mutex::new(vec![])}}
    fn runtime(&self)->PathBuf{self.home.path().join("run/services")}
    fn client(&self)->Arc<ProjectServiceClient>{let client=ProjectServiceClient::new(self.work.path(),&self.runtime(),PathBuf::from(BIN),ServicePolicy{launch_enabled:true}).unwrap();self.clients.lock().unwrap().push(client.clone());client}
    fn spec(&self,bank:&str,name:&str,role:&str,pty:bool)->ServiceSpec{let mut env=BTreeMap::new();env.insert("ARA_SERVICE_FIXTURE_ROLE".into(),role.into());env.insert("ARA_SERVICE_FIXTURE_BANK".into(),bank.into());for (key,suffix) in [("READY","ready"),("GATE","gate"),("RECEIVED","received"),("LEAF","leaf")]{env.insert(format!("ARA_SERVICE_FIXTURE_{key}"),self.work.path().join(format!("{name}.{suffix}")).to_string_lossy().into_owned());}ServiceSpec{name:name.into(),application:std::env::current_exe().unwrap().to_string_lossy().into_owned(),args:vec!["--exact".into(),bank.into(),"--nocapture".into()],env,cwd:self.work.path().into(),pty,persist:false,detached:false}}
    fn path(&self,name:&str,suffix:&str)->PathBuf{self.work.path().join(format!("{name}.{suffix}"))}
    fn state(&self,client:&ProjectServiceClient,name:&str)->Value{serde_json::from_slice(&bytes(&client.scope.runtime.join("services").join(name).join("state.json"))).unwrap_or(Value::Null)}
    fn host(&self,url:&str,extra:&[&str])->Host{let mut cmd=Command::new(BIN);for key in ["OPENROUTER_API_KEY","ANTHROPIC_API_KEY","ARA_API_KEY","ARA_TEST_API_KEY","ARA_MODEL","ARA_BASE_URL","ARA_TEST_BASE_URL","OPENROUTER_BASE_URL","ARA_TEST_MODEL_ID","CLAUDE_CONFIG_DIR","COPILOT_HOME","COPILOT_CUSTOM_INSTRUCTIONS_DIRS","WSL_DISTRO_NAME","WSL_INTEROP","ARA_SERVICE_TEST_DROP_SEND_REPLY_ONCE"]{cmd.env_remove(key);}cmd.env("ARA_API_KEY","fixture-local-only").env("HOME",self.home.path()).env("ARA_HOME",self.home.path()).args(["--model","fake-model","--base-url",url,"--cwd"]).arg(self.work.path()).arg("--session-dir").arg(self.home.path().join("sessions")).args(["--api","openai-completions","--repl","--mode","json","--compact-threshold","0"]).args(extra).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);Host::spawn(cmd)}
    fn journal(&self,id:&str,host:&Host)->PathBuf {
        // The actual CLI header precedes the lazy journal's first assistant
        // entry. Derive its planned file from that observed header using the
        // fixed SessionJournal::create_with_parent naming rule, without an
        // eager-file assumption or a timing delay. Subsequent assertions still
        // read real custom-message entries from this exact journal.
        let events=host.events.lock().unwrap();
        let header=events.iter().find(|event|event["type"]=="session"&&event["id"]==id)
            .expect("observed actual CLI Session header");
        let stamp=header["timestamp"].as_str().expect("actual Session timestamp");
        self.home.path().join("sessions").join(format!("{}_{}.jsonl",stamp.replace([':', '.'],"-"),id))
    }
}
impl Env {
    fn retain_artifacts(&self, phase: &str) -> std::io::Result<()> {
        let Some(root) = std::env::var_os("ARA_TEST_SERVICE_ARTIFACT_DIR") else { return Ok(()); };
        let destination=PathBuf::from(root).join(self.home.path().file_name().unwrap()).join(phase);
        std::fs::create_dir_all(&destination)?;
        let(mut files,mut skipped,mut errors)=(Vec::new(),Vec::new(),Vec::new());
        fn record_error(errors:&mut Vec<Value>,path:&str,operation:&str,error:std::io::Error) {
            errors.push(json!({"path":path,"operation":operation,"error":error.to_string(),"os_error":error.raw_os_error()}));
        }
        fn copy_bank(source:&Path,target:&Path,prefix:&str,files:&mut Vec<Value>,skipped:&mut Vec<Value>,errors:&mut Vec<Value>) {
            let entries=match std::fs::read_dir(source) {
                Ok(entries)=>entries,
                Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{skipped.push(json!({"path":prefix,"reason":"source-not-found"}));return;},
                Err(error)=>{record_error(errors,prefix,"read_dir",error);return;}
            };
            for entry in entries {
                let entry=match entry {
                    Ok(entry)=>entry,
                    Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{skipped.push(json!({"path":prefix,"reason":"entry-not-found"}));continue;},
                    Err(error)=>{record_error(errors,prefix,"directory_entry",error);continue;}
                };
                let name=entry.file_name();let text=name.to_string_lossy();let relative=format!("{prefix}/{text}");
                // broker.lock is intentionally share_mode(0) on Windows. It is
                // an operational lease, not process output. No blanket ignore
                // of sharing errors on actual evidence files is permitted.
                let lease=prefix.starts_with("services/")&&text=="broker.lock";
                if name=="token"||text.starts_with(".token-")||text.ends_with(".tmp")||text.contains(".tmp-")||lease {
                    skipped.push(json!({"path":relative,"reason":"operational-token-temp-or-lease"}));continue;
                }
                let metadata=match std::fs::symlink_metadata(entry.path()) {
                    Ok(metadata)=>metadata,
                    Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{skipped.push(json!({"path":relative,"reason":"entry-not-found"}));continue;},
                    Err(error)=>{record_error(errors,&relative,"symlink_metadata",error);continue;}
                };
                let link=metadata.file_type().is_symlink();
                #[cfg(windows)]let link={use std::os::windows::fs::MetadataExt;link||metadata.file_attributes()&0x400!=0};
                if link {skipped.push(json!({"path":relative,"reason":"symlink-or-reparse"}));continue;}
                if metadata.is_dir() {copy_bank(&entry.path(),&target.join(&name),&relative,files,skipped,errors);}
                else if metadata.is_file() {
                    let bytes=match std::fs::read(entry.path()) {
                        Ok(bytes)=>bytes,
                        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{skipped.push(json!({"path":relative,"reason":"file-not-found"}));continue;},
                        Err(error)=>{record_error(errors,&relative,"read",error);continue;}
                    };
                    if let Err(error)=std::fs::create_dir_all(target) {record_error(errors,&relative,"create-artifact-directory",error);continue;}
                    if let Err(error)=std::fs::write(target.join(&name),&bytes) {record_error(errors,&relative,"write-artifact",error);continue;}
                    files.push(json!({"path":relative,"bytes":bytes.len()}));
                }
            }
        }
        // One receipt exit retains all independent evidence even when one file
        // failed. Only NotFound races are tolerated; other errors still fail
        // the bank after publication, with details retained for review.
        for(source,name)in[(self.work.path().to_path_buf(),"project"),(self.home.path().join("sessions"),"sessions"),(self.home.path().join("run/services"),"services")] {
            copy_bank(&source,&destination.join(name),name,&mut files,&mut skipped,&mut errors);
        }
        std::fs::write(destination.join("ARTIFACTS.json"),serde_json::to_vec_pretty(&json!({
            "phase":phase,"files":files,"skipped":skipped,"errors":errors,
            "token_files_excluded":true,"lease_files_excluded":true,"symlinks_excluded":true,
            "source":"actual controlled service process bank; not real model evidence"
        }))?)?;
        eprintln!("SERVICE_ARTIFACTS:{}",destination.display());
        if errors.is_empty(){Ok(())}else{Err(std::io::Error::other(format!("{} service artifact error(s); details retained in {}",errors.len(),destination.join("ARTIFACTS.json").display())))}
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let before=self.retain_artifacts("before-cleanup");
        let clients=std::mem::take(self.clients.get_mut().unwrap());
        if !clients.is_empty() {
            let cleanup=std::thread::spawn(move||{
                let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                runtime.block_on(async move{for client in clients{let cancel=CancellationToken::new();if let Ok(services)=client.list(&cancel).await{for service in services{if !service.state.terminal(){let _=client.stop(&service.name,&cancel).await;}}}client.close_presence();}});
            });
            let _=cleanup.join();
        }
        let after=self.retain_artifacts("after-cleanup");
        let mut failures=Vec::new();
        for (phase,result) in [("before-cleanup",before),("after-cleanup",after)] {
            if let Err(error)=result {failures.push(format!("{phase}: {error}"));}
        }
        // Evidence failure must not skip owned process cleanup or the second
        // evidence phase, nor cause a double panic during an original failure.
        if !failures.is_empty() {
            let error=failures.join("; ");
            if std::thread::panicking(){eprintln!("service artifact retention failed during unwind: {error}");}
            else{panic!("service artifact retention failed: {error}");}
        }
    }
}

fn fixture()->bool{
    let Ok(role)=std::env::var("ARA_SERVICE_FIXTURE_ROLE")else{return false;};
    let env_path=|key:&str|PathBuf::from(std::env::var_os(format!("ARA_SERVICE_FIXTURE_{key}")).unwrap());
    let ready=env_path("READY");let gate=env_path("GATE");let received=env_path("RECEIVED");
    if role=="envreport"{let values=["GIT_DIR","GIT_COMMON_DIR","GIT_WORK_TREE","GIT_INDEX_FILE","GIT_OBJECT_DIRECTORY","GIT_ALTERNATE_OBJECT_DIRECTORIES"].into_iter().map(|key|(key,std::env::var(key).ok())).collect::<BTreeMap<_,_>>();write_file(&received,&serde_json::to_vec(&values).unwrap());return true;}
    if role=="leaf"{write_file(&env_path("LEAF"),std::process::id().to_string().as_bytes());while !exists(&gate){std::thread::sleep(Duration::from_millis(20));}return true;}
    if role=="tree"||role=="tree-exit"{let mut command=std::process::Command::new(std::env::current_exe().unwrap());command.args(["--exact",&std::env::var("ARA_SERVICE_FIXTURE_BANK").unwrap(),"--nocapture"]).env("ARA_SERVICE_FIXTURE_ROLE","leaf").stdin(Stdio::null());if role=="tree"{command.stdout(Stdio::null()).stderr(Stdio::null());}let _owned_child=command.spawn().unwrap();}
    if role=="tree-exit"{let start=Instant::now();while !exists(&env_path("LEAF")){assert!(start.elapsed()<Duration::from_secs(5));std::thread::sleep(Duration::from_millis(10));}write_file(&ready,std::process::id().to_string().as_bytes());std::thread::sleep(Duration::from_millis(100));return true;}
    if role=="query"||role=="raw"{
        #[cfg(unix)]unsafe{let mut mode:libc::termios=std::mem::zeroed();assert_eq!(libc::tcgetattr(0,&mut mode),0);libc::cfmakeraw(&mut mode);assert_eq!(libc::tcsetattr(0,libc::TCSANOW,&mode),0);}
        #[cfg(windows)]unsafe{use windows_sys::Win32::System::Console::{GetStdHandle,SetConsoleMode,STD_INPUT_HANDLE,ENABLE_VIRTUAL_TERMINAL_INPUT};assert_ne!(SetConsoleMode(GetStdHandle(STD_INPUT_HANDLE),ENABLE_VIRTUAL_TERMINAL_INPUT),0);}
    }
    write_file(&ready,std::process::id().to_string().as_bytes());
    if role=="query"{let record=|stage:&str,query:&[u8],reply:&[u8]|{let mut file=std::fs::OpenOptions::new().create(true).append(true).open(received.with_extension("progress")).unwrap();writeln!(file,"{}",json!({"stage":stage,"query":hex(query),"reply":hex(reply)})).unwrap();file.flush().unwrap();};let mut out=std::io::stdout();let mut replies=vec![];for (query,expected) in [(b"\x1b[5n".as_slice(),b"\x1b[0n".as_slice()),(b"\x1b[6n".as_slice(),b"\x1b[1;1R".as_slice()),(b"\x1b]11;?\x07".as_slice(),b"\x1b]11;rgb:0000/0000/0000\x07".as_slice())]{#[cfg(windows)] if query==b"\x1b[6n"{out.write_all(b"\x1b[1;1H").unwrap();out.flush().unwrap();}out.write_all(&query[..2]).unwrap();out.flush().unwrap();std::thread::sleep(Duration::from_millis(75));out.write_all(&query[2..]).unwrap();out.flush().unwrap();record("sent",query,&[]);let mut reply=vec![0;expected.len()];std::io::stdin().read_exact(&mut reply).unwrap();record("received",query,&reply);assert_eq!(reply,expected);record("accepted",query,&reply);replies.extend(reply);}write_file(&received,&replies);return true;}
    if role=="echo"||role=="raw"{let mut input=std::io::stdin();let mut output=std::io::stdout();loop{let mut buffer=[0u8;256];let n=input.read(&mut buffer).unwrap();if n==0{break;}let mut file=std::fs::OpenOptions::new().create(true).append(true).open(&received).unwrap();file.write_all(&buffer[..n]).unwrap();file.flush().unwrap();output.write_all(format!("RECEIPT:{}\n",hex(&buffer[..n])).as_bytes()).unwrap();output.flush().unwrap();}}
    else{while !exists(&gate){std::thread::sleep(Duration::from_millis(20));}}
    true
}
async fn ready(env:&Env,name:&str){eventually("fixture OS process ready",||async{exists(&env.path(name,"ready"))}).await;}
async fn terminal(client:&ProjectServiceClient,name:&str){eventually("real terminal snapshot",||async{client.find(name,&CancellationToken::new()).await.unwrap().is_some_and(|s|s.state.terminal()&&s.pid.is_none())}).await;}
fn journal(path:&Path)->Vec<Value>{String::from_utf8_lossy(&bytes(path)).lines().filter_map(|line|serde_json::from_str(line).ok()).collect()}
fn completions(path:&Path)->Vec<Value>{journal(path).into_iter().filter(|e|e["type"]=="custom_message"&&e["customType"]=="launch-completion").collect()}

struct Host{child:Child,input:ChildStdin,events:Arc<Mutex<Vec<Value>>>,stderr:Arc<Mutex<String>>}
impl Host{
    fn spawn(mut command:Command)->Self{let mut child=command.spawn().unwrap();let input=child.stdin.take().unwrap();let stdout=child.stdout.take().unwrap();let stderr_pipe=child.stderr.take().unwrap();let events=Arc::new(Mutex::new(vec![]));let out=events.clone();tokio::spawn(async move{let mut lines=BufReader::new(stdout).lines();while let Ok(Some(line))=lines.next_line().await{if let Ok(event)=serde_json::from_str::<Value>(&line){out.lock().unwrap().push(event);}}});let stderr=Arc::new(Mutex::new(String::new()));let err=stderr.clone();tokio::spawn(async move{let mut lines=BufReader::new(stderr_pipe).lines();while let Ok(Some(line))=lines.next_line().await{err.lock().unwrap().push_str(&format!("{line}\n"));}});Self{child,input,events,stderr}}
    async fn session(&self,index:usize)->String{eventually("actual CLI Session header",||async{self.events.lock().unwrap().iter().filter(|e|e["type"]=="session").count()>index}).await;self.events.lock().unwrap().iter().filter(|e|e["type"]=="session").nth(index).unwrap()["id"].as_str().unwrap().into()}
    async fn send(&mut self,text:&str){self.input.write_all(text.as_bytes()).await.unwrap();self.input.flush().await.unwrap();}
    async fn exit(mut self)->std::process::ExitStatus{let _=self.input.write_all(b"/exit\n").await;drop(self.input);tokio::time::timeout(WAIT,self.child.wait()).await.expect("CLI exit bounded").unwrap()}
}
async fn upstream(responses:Vec<Value>)->FakeUpstream{FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(),None).await.unwrap()}
fn answer(text_value:&str)->Value{json!({"events":[text(text_value),finish("stop"),done()]})}
async fn pending(env:&Env,client:&ProjectServiceClient,name:&str,n:usize){eventually("persisted broker pending receipt",||async{env.state(client,name)["pending"].as_array().is_some_and(|p|p.len()==n)}).await;}
async fn capable(env:&Env,client:&ProjectServiceClient,name:&str){eventually("actual Host subscription adopted",||async{env.state(client,name)["completion_capable"]==true}).await;}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn service_process_io(){if fixture(){return;} // SERVICE-PROCESS-IO
    let env=Env::new();let client=env.client();let second=env.client();let cancel=CancellationToken::new();
    assert!(client.list(&cancel).await.unwrap().is_empty());
    for pty in [false,true]{let name=if pty{"pty"}else{"pipe"};client.start(env.spec("service_process_io",name,if pty{"raw"}else{"echo"},pty),Some("io-owner".into()),false,&cancel).await.unwrap();ready(&env,name).await;
        let inputs=["中文","LF\n","CR\r","BOTH\r\n"];let mut expected=vec![];for input in inputs{second.send(name,input,&cancel).await.unwrap();let mut data=input.as_bytes().to_vec();if !data.ends_with(b"\r")&&!data.ends_with(b"\n"){data.push(b'\r');}if !pty&&data.last()==Some(&b'\r'){*data.last_mut().unwrap()=b'\n';}expected.extend(data);}
        eventually("actual child stdin bytes",||async{bytes(&env.path(name,"received"))==expected}).await;
        let log=client.scope.runtime.join("services").join(name).join("output.log");eventually("real output log receipts",||async{String::from_utf8_lossy(&bytes(&log)).contains("RECEIPT:")}).await;
        let stopped=client.stop(name,&cancel).await.unwrap();assert!(stopped.state.terminal());assert!(stopped.pid.is_none());assert!(client.send(name,"after-stop",&cancel).await.is_err());assert!(client.mode(name,ServiceMode::Persist,&cancel).await.is_err());
    }
    client.start(env.spec("service_process_io","query","query",true),None,false,&cancel).await.unwrap();eventually("real PTY query reply reaches child",||async{bytes(&env.path("query","received"))==b"\x1b[0n\x1b[1;1R\x1b]11;rgb:0000/0000/0000\x07"}).await;terminal(&client,"query").await;
    let canonical=ProjectServiceClient::new(&env.work.path().join("."),&env.runtime(),PathBuf::from(BIN),ServicePolicy{launch_enabled:false}).unwrap();assert_eq!(canonical.scope.endpoint,client.scope.endpoint);assert_eq!(canonical.list(&cancel).await.unwrap().len(),3);
    let other=Env::new();assert!(other.client().list(&cancel).await.unwrap().is_empty());
    // Parent git-hook environment must not redirect child repository discovery.
    // A worker-local polluted environment avoids mutating test process globals.
    let polluted=Env::new();let polluted_client=polluted.client();let mut command=Command::new(BIN);command.arg("__ara_worker_services").env("ARA_SERVICE_PROJECT",polluted.work.path()).env("ARA_SERVICE_RUNTIME_ROOT",polluted.runtime());for key in ["GIT_DIR","GIT_COMMON_DIR","GIT_WORK_TREE","GIT_INDEX_FILE","GIT_OBJECT_DIRECTORY","GIT_ALTERNATE_OBJECT_DIRECTORIES"]{command.env(key,"wrong-parent-repository");}#[cfg(windows)]command.env("git_dir","wrong-parent-lowercase");let mut worker=command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn().unwrap();eventually("polluted fixture broker published",||async{exists(&polluted_client.scope.runtime.join("broker.json"))}).await;
    for pty in [false,true]{let name=if pty{"env-pty"}else{"env-pipe"};polluted_client.start(polluted.spec("service_process_io",name,"envreport",pty),None,false,&cancel).await.unwrap();eventually("sanitized real child environment",||async{exists(&polluted.path(name,"received"))}).await;let values:Value=serde_json::from_slice(&bytes(&polluted.path(name,"received"))).unwrap();assert!(values.as_object().unwrap().values().all(Value::is_null));terminal(&polluted_client,name).await;}
    let mut explicit=polluted.spec("service_process_io","env-overlay","envreport",false);explicit.env.insert("GIT_DIR".into(),"explicit-spec-override".into());polluted_client.start(explicit,None,false,&cancel).await.unwrap();eventually("explicit service overlay wins",||async{exists(&polluted.path("env-overlay","received"))}).await;let values:Value=serde_json::from_slice(&bytes(&polluted.path("env-overlay","received"))).unwrap();assert_eq!(values["GIT_DIR"],"explicit-spec-override");terminal(&polluted_client,"env-overlay").await;worker.kill().await.unwrap();worker.wait().await.unwrap();
    let mut invalid=env.spec("service_process_io","invalid","wait",false);invalid.application="ara-service-no-such-executable-7e7c".into();assert!(matches!(client.start(invalid,None,false,&cancel).await,Err(ServiceError::OutcomeUnknown{..})));terminal(&client,"invalid").await;assert_eq!(client.find("invalid",&cancel).await.unwrap().unwrap().state,ServiceState::Failed);
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn service_lifecycle_modes(){if fixture(){return;} // SERVICE-LIFECYCLE-MODES
    let env=Env::new();let client=env.client();let cancel=CancellationToken::new();
    let first=client.start(env.spec("service_lifecycle_modes","session","wait",true),Some("owner".into()),false,&cancel).await.unwrap();ready(&env,"session").await;
    let persisted=client.mode("session",ServiceMode::Persist,&cancel).await.unwrap();assert!(persisted.persist&&!persisted.detached);assert_eq!(first.pid,persisted.pid);
    client.close_presence();let fresh=env.client();assert_eq!(fresh.find("session",&cancel).await.unwrap().unwrap().pid,first.pid);
    let detached=fresh.mode("session",ServiceMode::Detached,&cancel).await.unwrap();assert!(detached.persist&&detached.detached);assert_ne!(detached.pid,first.pid);assert!(detached.generation>first.generation);assert!(!env.state(&fresh,"session")["spec"]["pty"].as_bool().unwrap());
    assert!(matches!(fresh.mode("session",ServiceMode::Session,&cancel).await,Err(ServiceError::Rejected{..})));
    assert!(fresh.send("session","unavailable",&cancel).await.is_err());
    let shared=fresh.start(env.spec("service_lifecycle_modes","omp.browser.headless","wait",false),None,false,&cancel).await.unwrap();assert!(fresh.mode(&shared.name,ServiceMode::Detached,&cancel).await.is_err());fresh.stop(&shared.name,&cancel).await.unwrap();
    // Kill ONLY this fixture's exact broker PID with an owned OS handle. Its
    // detached generation must survive and be recovered with the same identity.
    let broker:Value=serde_json::from_slice(&bytes(&fresh.scope.runtime.join("broker.json"))).unwrap();kill_owned(broker["pid"].as_u64().unwrap()as u32,&broker["created"].as_str().unwrap());
    eventually("recovered detached record",||async{fresh.list(&cancel).await.is_ok()}).await;let recovered=fresh.find("session",&cancel).await.unwrap().unwrap();assert_eq!(recovered.pid,detached.pid);assert_eq!(recovered.id,detached.id);
    fresh.stop("session",&cancel).await.unwrap();terminal(&fresh,"session").await;
    let tree=fresh.start(env.spec("service_lifecycle_modes","tree","tree",false),None,false,&cancel).await.unwrap();eventually("actual descendant PID",||async{exists(&env.path("tree","leaf"))}).await;let leaf=String::from_utf8(bytes(&env.path("tree","leaf"))).unwrap().parse::<u32>().unwrap();let leaf_identity=identity(leaf).unwrap();fresh.stop("tree",&cancel).await.unwrap();assert!(identity(tree.pid.unwrap()).is_none());assert!(identity(leaf).as_ref()!=Some(&leaf_identity),"stop must terminate real descendants, not only root");
    #[cfg(target_os="linux")]{use std::os::unix::fs::symlink;let alias=env.work.path().join("abcdefghijklmn😀");symlink(std::env::current_exe().unwrap(),&alias).unwrap();let mut opaque=env.spec("service_lifecycle_modes","opaque-comm","wait",false);opaque.application=alias.to_string_lossy().into_owned();let started=fresh.start(opaque,None,false,&cancel).await.unwrap();ready(&env,"opaque-comm").await;assert!(started.pid.is_some());fresh.stop("opaque-comm",&cancel).await.unwrap();terminal(&fresh,"opaque-comm").await;}
    // Historical cwd is structural metadata on recovery, not a fresh start
    // prerequisite. Deleting it cannot make every project service unavailable.
    let deleted=Env::new();let deleted_client=deleted.client();let historical=deleted.work.path().join("historical-cwd");std::fs::create_dir(&historical).unwrap();let mut spec=deleted.spec("service_lifecycle_modes","deleted-cwd","wait",false);spec.cwd=historical.clone();deleted_client.start(spec,None,false,&cancel).await.unwrap();ready(&deleted,"deleted-cwd").await;write_file(&deleted.path("deleted-cwd","gate"),b"exit");terminal(&deleted_client,"deleted-cwd").await;std::fs::remove_dir(&historical).unwrap();let broker:Value=serde_json::from_slice(&bytes(&deleted_client.scope.runtime.join("broker.json"))).unwrap();kill_owned(broker["pid"].as_u64().unwrap()as u32,broker["created"].as_str().unwrap());eventually("recovery retains terminal record with deleted cwd",||async{deleted_client.list(&cancel).await.is_ok()}).await;assert!(deleted_client.find("deleted-cwd",&cancel).await.unwrap().unwrap().state.terminal());assert!(deleted_client.list(&cancel).await.unwrap().iter().any(|entry|entry.name=="deleted-cwd"));
    let mut invalid=deleted.spec("service_lifecycle_modes","new-deleted-cwd","wait",false);invalid.cwd=historical;assert!(matches!(deleted_client.start(invalid.clone(),None,false,&cancel).await,Err(ServiceError::NotDispatched{..})));assert!(matches!(deleted_client.request(Operation::Start{spec:invalid,owner:None,replace:false},&cancel).await,Err(ServiceError::Rejected{..})));assert!(deleted_client.find("new-deleted-cwd",&cancel).await.unwrap().is_none());deleted_client.start(deleted.spec("service_lifecycle_modes","other-after-recovery","wait",false),None,false,&cancel).await.unwrap();ready(&deleted,"other-after-recovery").await;assert!(deleted_client.stop("other-after-recovery",&cancel).await.unwrap().state.terminal());
    #[cfg(target_os="linux")]{let active_cwd=deleted.work.path().join("active-deleted-cwd");std::fs::create_dir(&active_cwd).unwrap();let mut spec=deleted.spec("service_lifecycle_modes","active-deleted-cwd","wait",false);spec.cwd=active_cwd.clone();spec.persist=true;spec.detached=true;let active=deleted_client.start(spec,None,false,&cancel).await.unwrap();ready(&deleted,"active-deleted-cwd").await;std::fs::remove_dir(&active_cwd).unwrap();let broker:Value=serde_json::from_slice(&bytes(&deleted_client.scope.runtime.join("broker.json"))).unwrap();kill_owned(broker["pid"].as_u64().unwrap()as u32,broker["created"].as_str().unwrap());eventually("deleted cwd live detached recovery",||async{deleted_client.list(&cancel).await.is_ok()}).await;let recovered=deleted_client.find("active-deleted-cwd",&cancel).await.unwrap().unwrap();assert_eq!(recovered.pid,active.pid);assert_eq!(recovered.id,active.id);assert!(!recovered.state.terminal());assert!(deleted_client.stop("active-deleted-cwd",&cancel).await.unwrap().state.terminal());}
    // Root exited while its still-live child holds inherited stdout. Stop is
    // issued within the production stale-drain window, before root terminal
    // receipt publication, and must not equate dead root with an empty tree.
    let orphan=fresh.start(env.spec("service_lifecycle_modes","dead-parent","tree-exit",false),None,false,&cancel).await.unwrap();ready(&env,"dead-parent").await;let leaf=String::from_utf8(bytes(&env.path("dead-parent","leaf"))).unwrap().parse::<u32>().unwrap();let lineage=identity(leaf).unwrap();eventually("owned root exited before inherited output holder",||async{identity(orphan.pid.unwrap()).is_none()}).await;assert!(identity(leaf).is_some());assert!(!fresh.find("dead-parent",&cancel).await.unwrap().unwrap().state.terminal(),"fixture must exercise Stop during stale-output drain, not terminal NoOp");fresh.stop("dead-parent",&cancel).await.unwrap();assert_ne!(identity(leaf).as_ref(),Some(&lineage));
    // Deterministic thread-creation fault, actual already-created process tree
    // and output readers: no resource-exhaustion stunt or fake cleanup receipt.
    for stage in ["output","waiter"]{let name=format!("failed-{stage}");let mut spec=env.spec("service_lifecycle_modes",&name,"tree",false);spec.env.insert("ARA_SERVICE_TEST_FAIL_START_STAGE".into(),stage.into());spec.env.insert("ARA_SERVICE_TEST_START_READY_FILE".into(),env.path(&name,"leaf").to_string_lossy().into_owned());let error=fresh.start(spec,None,false,&cancel).await.unwrap_err();assert!(matches!(error,ServiceError::OutcomeUnknown{..}));let root_pid=String::from_utf8(bytes(&env.path(&name,"ready"))).unwrap().parse::<u32>().unwrap();let leaf_pid=String::from_utf8(bytes(&env.path(&name,"leaf"))).unwrap().parse::<u32>().unwrap();assert!(identity(root_pid).is_none()&&identity(leaf_pid).is_none(),"failed {stage} start must settle all actual owned children");assert_eq!(fresh.find(&name,&cancel).await.unwrap().unwrap().state,ServiceState::Failed);}
    // Per-name admission permits exactly one of two racing producers.
    let spec=env.spec("service_lifecycle_modes","race","wait",false);let(a,b)=tokio::join!(client.start(spec.clone(),None,false,&cancel),fresh.start(spec,None,false,&cancel));assert_ne!(a.is_ok(),b.is_ok());fresh.stop("race",&cancel).await.unwrap();
    fresh.start(env.spec("service_lifecycle_modes","replace","wait",false),None,false,&cancel).await.unwrap();let next=fresh.start(env.spec("service_lifecycle_modes","replace","wait",false),None,true,&cancel).await.unwrap();tokio::time::sleep(Duration::from_millis(200)).await;assert_eq!(fresh.find("replace",&cancel).await.unwrap().unwrap().id,next.id,"old watcher cannot finish replacement generation");fresh.stop("replace",&cancel).await.unwrap();
    // Replacing a terminal record must preserve old owner's unACKed receipt,
    // which source stores independently of its current name -> record map.
    fresh.request(Operation::Subscribe{owner:"replace-owner".into(),subscription:"replace-sub".into()},&cancel).await.unwrap();let old=fresh.start(env.spec("service_lifecycle_modes","pending-replace","wait",false),Some("replace-owner".into()),false,&cancel).await.unwrap();write_file(&env.path("pending-replace","gate"),b"exit");terminal(&fresh,"pending-replace").await;pending(&env,&fresh,"pending-replace",1).await;assert!(fresh.start(env.spec("service_lifecycle_modes","pending-replace","wait",false),Some("new-owner".into()),false,&cancel).await.is_err());std::fs::remove_file(env.path("pending-replace","gate")).unwrap();let new=fresh.start(env.spec("service_lifecycle_modes","pending-replace","wait",false),Some("new-owner".into()),true,&cancel).await.unwrap();assert_ne!(old.id,new.id);let Reply::Completions{entries}=fresh.request(Operation::Poll{owner:"replace-owner".into(),subscription:"replace-sub".into()},&cancel).await.unwrap()else{panic!("typed completion page")};assert_eq!(entries.len(),1);assert_eq!(entries[0].daemon.id,old.id);fresh.request(Operation::Ack{owner:"replace-owner".into(),subscription:"replace-sub".into(),completion_ids:vec![entries[0].completion_id.clone()]},&cancel).await.unwrap();pending(&env,&fresh,"pending-replace",0).await;fresh.stop("pending-replace",&cancel).await.unwrap();
    let first=fresh.start(env.spec("service_lifecycle_modes","order-a","wait",false),None,false,&cancel).await.unwrap();tokio::time::sleep(Duration::from_millis(2)).await;let second=fresh.start(env.spec("service_lifecycle_modes","order-b","wait",false),None,false,&cancel).await.unwrap();let list=fresh.list(&cancel).await.unwrap();assert_eq!((&list[0].name,&list[1].name),(&first.name,&second.name));assert!(list.iter().skip(2).all(|entry|entry.state.terminal()));fresh.stop("order-a",&cancel).await.unwrap();tokio::time::sleep(Duration::from_millis(2)).await;fresh.stop("order-b",&cancel).await.unwrap();let list=fresh.list(&cancel).await.unwrap();assert_eq!(list[0].name,"order-b");assert_eq!(list.len(),10,"fixed list contract caps only displayed terminal history");assert!(fresh.find("session",&cancel).await.unwrap().is_some(),"direct find must retain older records outside list cap");
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn service_completion_ownership(){if fixture(){return;} // SERVICE-COMPLETION-OWNERSHIP
    let env=Env::new();let client=env.client();let cancel=CancellationToken::new();
    // Busy yield uses the actual bash owner and actual native agent pipeline.
    let sleep_command=if cfg!(windows){"ping -n 3 127.0.0.1 > /dev/null"}else{"sleep 2"};
    let up=upstream(vec![json!({"events":[tool_call(0,"busy-1","bash",&json!({"command":sleep_command}).to_string()),finish("tool_calls"),done()]}),answer("busy completed"),answer("idle completed"),answer("resume completed"),answer("new session"),answer("memory completed")]).await;
    let mut host=env.host(&up.base_url(),&[]);let owner=host.session(0).await;let path=env.journal(&owner,&host);
    client.start(env.spec("service_completion_ownership","busy","wait",false),Some(owner.clone()),false,&cancel).await.unwrap();ready(&env,"busy").await;capable(&env,&client,"busy").await;
    host.send("run bounded busy tool\n").await;eventually("actual provider first request",||async{up.served()>=1}).await;write_file(&env.path("busy","gate"),b"exit");
    eventually("busy completion appended by actual HostSink",||async{completions(&path).len()==1}).await;pending(&env,&client,"busy",0).await;
    eventually("busy continuation reaches provider",||async{up.served()>=2}).await;assert!(up.requests.lock().await.iter().skip(1).any(|request|request["body"].to_string().contains("Supervised process busy exited with exit code 0.")),"completion body must enter actual next model context");
    client.start(env.spec("service_completion_ownership","idle","wait",false),Some(owner.clone()),false,&cancel).await.unwrap();capable(&env,&client,"idle").await;write_file(&env.path("idle","gate"),b"exit");eventually("idle continuation uses retained input reader",||async{completions(&path).len()==2&&up.served()>=3}).await;pending(&env,&client,"idle",0).await;
    assert!(host.exit().await.success());
    // Real journal write failure: replace the exact owned journal file with a
    // directory (works even under a Unix root test account). No synthetic sink.
    let mut host=env.host(&up.base_url(),&["--resume",path.to_str().unwrap()]);assert_eq!(host.session(0).await,owner);
    client.start(env.spec("service_completion_ownership","append-fault","wait",false),Some(owner.clone()),false,&cancel).await.unwrap();capable(&env,&client,"append-fault").await;
    let backup=path.with_extension("owned-backup");std::fs::rename(&path,&backup).unwrap();std::fs::create_dir(&path).unwrap();write_file(&env.path("append-fault","gate"),b"exit");pending(&env,&client,"append-fault",1).await;eventually("actual Host reports journal failure",||async{!host.stderr.lock().unwrap().is_empty()&&host.child.id().is_some()}).await;
    let status=tokio::time::timeout(WAIT,host.child.wait()).await.unwrap().unwrap();assert!(!status.success());assert_eq!(env.state(&client,"append-fault")["pending"].as_array().unwrap().len(),1);std::fs::remove_dir(&path).unwrap();std::fs::rename(&backup,&path).unwrap();
    let mut resumed=env.host(&up.base_url(),&["--resume",path.to_str().unwrap()]);assert_eq!(resumed.session(0).await,owner);eventually("original Session re-adopts unACKed completion",||async{completions(&path).len()==3}).await;pending(&env,&client,"append-fault",0).await;
    // A successful /new changes the actual owner. Old completion is preserved
    // for old resume and cannot appear in the fresh Session's journal/context.
    client.start(env.spec("service_completion_ownership","old-owner","wait",false),Some(owner.clone()),false,&cancel).await.unwrap();capable(&env,&client,"old-owner").await;resumed.send("/new\n").await;let new_owner=resumed.session(1).await;assert_ne!(new_owner,owner);write_file(&env.path("old-owner","gate"),b"exit");pending(&env,&client,"old-owner",1).await;let new_path=env.journal(&new_owner,&resumed);assert!(completions(&new_path).is_empty());assert!(resumed.exit().await.success());
    let old=env.host(&up.base_url(),&["--resume",path.to_str().unwrap()]);assert_eq!(old.session(0).await,owner);eventually("old owner resume adopts old completion",||async{completions(&path).len()==4}).await;pending(&env,&client,"old-owner",0).await;assert!(old.exit().await.success());
    // Same-project --no-session owners are unique; adoption is in-memory and
    // does not create a durable Session journal or leak into the other owner.
    let before=std::fs::read_dir(env.home.path().join("sessions")).unwrap().count();let memory_a=env.host(&up.base_url(),&["--no-session"]);let memory_b=env.host(&up.base_url(),&["--no-session"]);let a=memory_a.session(0).await;let b=memory_b.session(0).await;assert_ne!(a,b);assert_ne!(a,"ephemeral");
    client.start(env.spec("service_completion_ownership","memory","wait",false),Some(a.clone()),false,&cancel).await.unwrap();capable(&env,&client,"memory").await;write_file(&env.path("memory","gate"),b"exit");eventually("memory Host emitted own completion",||async{memory_a.events.lock().unwrap().iter().any(|e|e.to_string().contains("launch-completion"))}).await;pending(&env,&client,"memory",0).await;assert!(!memory_b.events.lock().unwrap().iter().any(|e|e.to_string().contains("launch-completion")));assert_eq!(std::fs::read_dir(env.home.path().join("sessions")).unwrap().count(),before);assert!(memory_a.exit().await.success());assert!(memory_b.exit().await.success());
    // Enter and completion wake share the retained real stdin reader. Whichever
    // is admitted first, the user draft appears once and no parallel Run starts.
    let race=Env::new();let race_client=race.client();let race_up=upstream((0..8).map(|_|answer("serial race settled")).collect()).await;let mut race_host=race.host(&race_up.base_url(),&[]);let race_owner=race_host.session(0).await;let race_path=race.journal(&race_owner,&race_host);race_client.start(race.spec("service_completion_ownership","enter-race","wait",false),Some(race_owner.clone()),false,&cancel).await.unwrap();capable(&race,&race_client,"enter-race").await;
    race_host.send("single-draft-enter-race\n").await;write_file(&race.path("enter-race","gate"),b"exit");eventually("Enter/wake completion adopted once",||async{completions(&race_path).len()==1}).await;pending(&race,&race_client,"enter-race",0).await;assert!(race_host.exit().await.success());assert_eq!(journal(&race_path).iter().filter(|entry|entry["type"]=="message"&&entry["message"]["role"]=="user"&&entry["message"].to_string().contains("single-draft-enter-race")).count(),1);
    // EOF may win admission. In that case preservePending retains exactly the
    // one receipt; if wake wins, one journal adoption removes it. Both are valid.
    let mut eof_host=race.host(&race_up.base_url(),&["--resume",race_path.to_str().unwrap()]);assert_eq!(eof_host.session(0).await,race_owner);race_client.start(race.spec("service_completion_ownership","eof-race","wait",false),Some(race_owner),false,&cancel).await.unwrap();capable(&race,&race_client,"eof-race").await;write_file(&race.path("eof-race","gate"),b"exit");drop(eof_host.input);assert!(tokio::time::timeout(WAIT,eof_host.child.wait()).await.unwrap().unwrap().success());terminal(&race_client,"eof-race").await;let adopted=completions(&race_path).iter().filter(|entry|entry.to_string().contains("eof-race")).count();assert!(adopted<=1);pending(&race,&race_client,"eof-race",usize::from(adopted==0)).await;
    // Provider continuation failure cannot revoke an already real append/ACK.
    let failed=Env::new();let failed_client=failed.client();let failed_up=upstream(vec![json!({"status":400,"body":"{\"error\":{\"message\":\"controlled-post-adoption-error\"}}"})]).await;let failed_host=failed.host(&failed_up.base_url(),&[]);let failed_owner=failed_host.session(0).await;let failed_path=failed.journal(&failed_owner,&failed_host);failed_client.start(failed.spec("service_completion_ownership","continuation-failure","wait",false),Some(failed_owner),false,&cancel).await.unwrap();capable(&failed,&failed_client,"continuation-failure").await;write_file(&failed.path("continuation-failure","gate"),b"exit");eventually("append before provider failure",||async{completions(&failed_path).len()==1}).await;pending(&failed,&failed_client,"continuation-failure",0).await;eventually("actual provider failure reported separately",||async{failed_host.events.lock().unwrap().iter().any(|event|event["type"]=="message_end"&&event["message"]["role"]=="assistant"&&event["message"]["stopReason"]=="error"&&event["message"]["errorMessage"].as_str().is_some_and(|message|message.contains("controlled-post-adoption-error")))}).await;assert_eq!(failed_up.served(),1,"one real provider continuation request");assert!(failed_up.requests.lock().await.iter().any(|request|request.to_string().contains("continuation-failure")),"actual failed request includes the adopted completion");assert_eq!(completions(&failed_path).len(),1);let _=failed_host.exit().await;
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn service_fault_unknown(){if fixture(){return;} // SERVICE-FAULT-UNKNOWN
    let env=Env::new();let client=env.client();let cancel=CancellationToken::new();
    // Start the actual broker with a per-worker deterministic send-response
    // loss, without mutating this test process's global environment.
    let mut worker=Command::new(BIN).arg("__ara_worker_services").env("ARA_SERVICE_PROJECT",env.work.path()).env("ARA_SERVICE_RUNTIME_ROOT",env.runtime()).env("ARA_SERVICE_TEST_DROP_SEND_REPLY_ONCE","1").env("ARA_SERVICE_TEST_DROP_ACK_ONCE","1").env("ARA_SERVICE_TEST_HOLD_SEND_REPLY_ONCE","1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true).spawn().unwrap();
    eventually("explicit broker metadata published",||async{exists(&client.scope.runtime.join("broker.json"))}).await;eventually("explicit broker ready",||async{client.list(&cancel).await.is_ok()}).await;
    client.start(env.spec("service_fault_unknown","effect","echo",false),Some("fault-owner".into()),false,&cancel).await.unwrap();ready(&env,"effect").await;
    assert!(matches!(client.send("effect","one-effect",&cancel).await,Err(ServiceError::OutcomeUnknown{..})));eventually("unknown request actually wrote once",||async{bytes(&env.path("effect","received"))==b"one-effect\n"}).await;tokio::time::sleep(Duration::from_millis(250)).await;assert_eq!(bytes(&env.path("effect","received")),b"one-effect\n","unknown request must not auto-replay");
    let stopped=CancellationToken::new();stopped.cancel();assert!(matches!(client.send("effect","never",&stopped).await,Err(ServiceError::NotDispatched{..})));assert_eq!(bytes(&env.path("effect","received")),b"one-effect\n");
    let after=CancellationToken::new();let request=client.send("effect","cancel-after-effect",&after);tokio::pin!(request);
    loop{tokio::select!{result=&mut request=>panic!("held send settled before cancellation: {result:?}"),_=tokio::time::sleep(Duration::from_millis(20))=>{if bytes(&env.path("effect","received"))==b"one-effect\ncancel-after-effect\n"{break;}}}}
    after.cancel();assert!(matches!(request.await,Err(ServiceError::OutcomeUnknown{..})));tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(client.find("missing",&cancel).await.unwrap().is_none());assert!(matches!(client.send("missing","x",&cancel).await,Err(ServiceError::Rejected{code,..})if code=="not_found"));
    let state_file=client.scope.runtime.join("services/effect/state.json");let state_backup=state_file.with_extension("owned-backup");std::fs::rename(&state_file,&state_backup).unwrap();std::fs::create_dir(&state_file).unwrap();assert!(matches!(client.mode("effect",ServiceMode::Persist,&cancel).await,Err(ServiceError::OutcomeUnknown{..})));std::fs::remove_dir(&state_file).unwrap();std::fs::rename(&state_backup,&state_file).unwrap();client.mode("effect",ServiceMode::Session,&cancel).await.unwrap();
    let token=String::from_utf8(bytes(&client.scope.runtime.join("token"))).unwrap();let unauthorized=Request{version:VERSION,id:"wrong-token".into(),token:"not-the-token".into(),operation:Operation::Send{name:"effect".into(),data:"forbidden\n".into()}};
    let response=raw(&client,serde_json::to_vec(&unauthorized).unwrap()).await;let response:Response=serde_json::from_slice(&response).unwrap();assert_eq!(response.error.unwrap().code,"unauthorized");assert!(raw(&client,br#"{"version":1,"id":"bad","token":"x","operation":{"op":"unknown"}}"#.to_vec()).await.is_empty());assert!(raw(&client,b"{malformed".to_vec()).await.is_empty());assert_eq!(bytes(&env.path("effect","received")),b"one-effect\ncancel-after-effect\n");
    // Competing worker cannot acquire the OS-held lease or replace endpoint.
    let mut contender=Command::new(BIN).arg("__ara_worker_services").env("ARA_SERVICE_PROJECT",env.work.path()).env("ARA_SERVICE_RUNTIME_ROOT",env.runtime()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();assert!(!tokio::time::timeout(Duration::from_secs(5),contender.wait()).await.unwrap().unwrap().success());assert!(client.find("effect",&cancel).await.unwrap().unwrap().pid.is_some());
    let unavailable=Env::new();let isolated=ProjectServiceClient::new(unavailable.work.path(),&unavailable.runtime(),PathBuf::from("no-such-ara-broker-fixture-executable"),ServicePolicy{launch_enabled:false}).unwrap();assert!(matches!(isolated.list(&cancel).await,Err(ServiceError::NotDispatched{..})));let(values,error)=isolated.list_tolerant(&cancel).await.unwrap();assert!(values.is_empty()&&error.is_some());
    // ACK loss before adoption leaves broker pending. A real Host retries ACK
    // from its bounded seen window while appending only one completion record.
    let up=upstream(vec![answer("ack loss adopted")]).await;let host=env.host(&up.base_url(),&[]);let owner=host.session(0).await;let path=env.journal(&owner,&host);client.start(env.spec("service_fault_unknown","ack-loss","wait",false),Some(owner),false,&cancel).await.unwrap();capable(&env,&client,"ack-loss").await;write_file(&env.path("ack-loss","gate"),b"exit");eventually("ACK-loss journal append",||async{completions(&path).len()==1}).await;pending(&env,&client,"ack-loss",0).await;tokio::time::sleep(Duration::from_millis(350)).await;assert_eq!(completions(&path).len(),1);assert!(host.exit().await.success());
    client.stop("effect",&cancel).await.unwrap();let broker:Value=serde_json::from_slice(&bytes(&client.scope.runtime.join("broker.json"))).unwrap();worker.kill().await.unwrap();worker.wait().await.unwrap();
    // Stale creation identity is never authority to kill a reused live PID.
    let state=client.scope.runtime.join("services/effect/state.json");let mut metadata:Value=serde_json::from_slice(&bytes(&state)).unwrap();metadata["snapshot"]["state"]=json!("running");metadata["snapshot"]["pid"]=json!(std::process::id());metadata["process"]=json!({"pid":std::process::id(),"created":"not-this-process-generation"});write_file(&state,&serde_json::to_vec(&metadata).unwrap());let before=identity(std::process::id()).unwrap();client.list(&cancel).await.unwrap();assert_eq!(identity(std::process::id()).unwrap(),before);assert!(client.find("effect",&cancel).await.unwrap().unwrap().state.terminal());let _=token;let _=broker;
}

async fn raw(client:&ProjectServiceClient,mut bytes:Vec<u8>)->Vec<u8>{bytes.push(b'\n');
    #[cfg(windows)]let mut stream=tokio::net::windows::named_pipe::ClientOptions::new().open(&client.scope.endpoint).unwrap();
    #[cfg(unix)]let mut stream=tokio::net::UnixStream::connect(&client.scope.endpoint).await.unwrap();
    stream.write_all(&bytes).await.unwrap();stream.flush().await.unwrap();let mut response=vec![];let mut reader=BufReader::new(stream.take((ara_cli::host_services::protocol::MAX_FRAME+1)as u64));match tokio::time::timeout(Duration::from_secs(5),reader.read_until(b'\n',&mut response)).await.unwrap(){Ok(_)=>{},Err(error)if matches!(error.kind(),std::io::ErrorKind::BrokenPipe|std::io::ErrorKind::UnexpectedEof)=>{},Err(error)=>panic!("private fixture IPC read: {error}")};response
}

#[cfg(target_os="linux")]
fn identity(pid:u32)->Option<String>{let stat=std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;let end=stat.rfind(')')?;let fields=stat[end+1..].split_whitespace().collect::<Vec<_>>();if matches!(*fields.first()?,"Z"|"X"){return None;}Some(format!("linux-start:{}:{}",std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?.trim(),fields.get(19)?))}
#[cfg(target_os="linux")]
fn kill_owned(pid:u32,expected:&str){use std::os::fd::{FromRawFd,OwnedFd,AsRawFd};assert_ne!(pid,std::process::id());let fd=unsafe{libc::syscall(libc::SYS_pidfd_open,pid,0)};assert!(fd>=0);let fd=unsafe{OwnedFd::from_raw_fd(fd as i32)};assert_eq!(identity(pid).as_deref(),Some(expected));assert_eq!(unsafe{libc::syscall(libc::SYS_pidfd_send_signal,fd.as_raw_fd(),libc::SIGKILL,std::ptr::null::<libc::siginfo_t>(),0)},0);}
#[cfg(windows)]
fn identity(pid:u32)->Option<String>{use windows_sys::Win32::{Foundation::CloseHandle,System::Threading::{OpenProcess,GetProcessTimes,GetExitCodeProcess,PROCESS_QUERY_LIMITED_INFORMATION}};unsafe{let handle=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,0,pid);if handle.is_null(){return None;}let mut creation=std::mem::zeroed();let mut exit=std::mem::zeroed();let mut kernel=std::mem::zeroed();let mut user=std::mem::zeroed();let mut code=0;let ok=GetProcessTimes(handle,&mut creation,&mut exit,&mut kernel,&mut user)!=0&&GetExitCodeProcess(handle,&mut code)!=0;CloseHandle(handle);if !ok||code!=259{return None;}Some(format!("windows-filetime:{}",((creation.dwHighDateTime as u64)<<32)|creation.dwLowDateTime as u64))}}
#[cfg(windows)]
fn kill_owned(pid:u32,expected:&str){use windows_sys::Win32::{Foundation::CloseHandle,System::Threading::{OpenProcess,GetProcessTimes,TerminateProcess,WaitForSingleObject,PROCESS_QUERY_LIMITED_INFORMATION,PROCESS_TERMINATE,PROCESS_SYNCHRONIZE}};assert_ne!(pid,std::process::id());unsafe{let handle=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION|PROCESS_TERMINATE|PROCESS_SYNCHRONIZE,0,pid);assert!(!handle.is_null());let mut creation=std::mem::zeroed();let mut exit=std::mem::zeroed();let mut kernel=std::mem::zeroed();let mut user=std::mem::zeroed();assert_ne!(GetProcessTimes(handle,&mut creation,&mut exit,&mut kernel,&mut user),0);let actual=format!("windows-filetime:{}",((creation.dwHighDateTime as u64)<<32)|creation.dwLowDateTime as u64);assert_eq!(actual,expected);assert_ne!(TerminateProcess(handle,1),0);assert_eq!(WaitForSingleObject(handle,5000),0);CloseHandle(handle);}}
