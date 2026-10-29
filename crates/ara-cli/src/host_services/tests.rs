//! One concentrated unit bank. Actual broker/process/Host receipts live in
//! tests/service_control.rs; these assertions do not stand for those effects.
use super::*;
use super::{process::QueryResponder,protocol::*,store::*};
use ara_session::SessionJournal;

fn snapshot(owner:&str)->ServiceSnapshot {ServiceSnapshot{name:"one<&>".into(),id:"generation-a".into(),state:ServiceState::Exited,pid:None,created_at:10,started_at:11,exited_at:Some(12),exit_code:Some(0),exit_reason:None,owner:Some(owner.into()),persist:false,detached:false,generation:1}}
fn queued(lease:&SessionServiceLease,id:&str)->oneshot::Receiver<Result<CompletionAdoption,String>>{
    let(tx,rx)=oneshot::channel();let mut state=lease.state.lock().unwrap();
    let owner=state.binding.as_ref().map(|binding|binding.id.clone()).unwrap_or_else(||"logical-a".into());
    let mut daemon=snapshot(&owner);daemon.name="one".into();
    state.queue.push_back(Pending{notification:Completion{completion_id:id.into(),owner,daemon},settler:tx});rx
}
fn receipt(persistent:bool)->CompletionAdoption{CompletionAdoption{session_id:"logical-a".into(),entry_id:"real-append-receipt".into(),persistent}}

#[tokio::test]
async fn service_control_unit_bank(){
    // Pinned responder cases, plus every chunk boundary (including OSC ST).
    let probes:&[(&[u8],&[u8])]=&[
        (b"\x1b[6n",b"\x1b[1;1R"),(b"\x1b[5n",b"\x1b[0n"),
        (b"\x1b[c",b"\x1b[?1;2c"),(b"\x1b[0c",b"\x1b[?1;2c"),(b"\x1b[>c",b"\x1b[>0;10;1c"),
        (b"\x1b]11;?\x07",b"\x1b]11;rgb:0000/0000/0000\x07"),
        (b"\x1b]10;?\x1b\\",b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\"),
        (b"\x1b]11;?\x1b\\",b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
        (b"\x1b]10;?\x07",b"\x1b]10;rgb:ffff/ffff/ffff\x07"),
    ];
    for &(query,expected) in probes{for split in 0..=query.len(){
        let mut parser=QueryResponder::new(false);let mut actual=parser.feed(&query[..split]);actual.extend(parser.feed(&query[split..]));assert_eq!(actual,expected,"query={query:?}, split={split}");assert!(parser.feed(b"ordinary UTF8 \xe4\xb8\xad\n").is_empty());
    }}
    let mut parser=QueryResponder::new(true);assert_eq!(parser.feed(b"\x1b[6n\x1b[c"),b"\x1b[?1;2c");assert_eq!(parser.feed(b"READY\x1b[6n"),b"\x1b[1;1R");assert_eq!(parser.feed(b"\x1b[6n"),b"\x1b[1;1R");
    assert!(parser.feed(b"\x1b[31mred\x1b[0m\x1b[2A\x1b[?6n\x1b[=cplain\n").is_empty());
    assert_eq!(QueryResponder::new(false).feed(b"\x1b]11;?\x07\x1b[6n"),b"\x1b]11;rgb:0000/0000/0000\x07\x1b[1;1R");
    let mut bounded=QueryResponder::new(false);bounded.feed(&[b'\x1b';100_000]);assert_eq!(bounded.feed(b"[5n"),b"\x1b[0n");

    use std::ffi::OsStr;
    assert!(process::is_git_repo_location_key(OsStr::new("GIT_DIR"),false));assert!(!process::is_git_repo_location_key(OsStr::new("git_dir"),false));assert!(process::is_git_repo_location_key(OsStr::new("git_dir"),true));assert!(!process::is_git_repo_location_key(OsStr::new("GIT_CONFIG_GLOBAL"),true));
    let environment=process::worker_environment(&[("GIT_DIR".into(),"explicit".into())].into_iter().collect());assert_eq!(environment.iter().filter(|(key,_)|process::is_git_repo_location_key(key,cfg!(windows))).map(|(_,value)|value.to_string_lossy().into_owned()).collect::<Vec<_>>(),vec!["explicit"]);
    let mut opaque=b"123 (opaque) ".to_vec();opaque.splice(5..5,[0xff,0xfe]);let mut fields=vec!["S","1","2","3"];fields.extend(std::iter::repeat_n("0",15));fields.push("99");opaque.extend_from_slice(fields.join(" ").as_bytes());assert_eq!(process::parse_linux_stat_bytes(&opaque).unwrap(),(1,2,3,99));assert!(process::parse_linux_stat_bytes(b"broken proc record").is_err());
    let temp=tempfile::tempdir().unwrap();let project=temp.path().join("project");std::fs::create_dir(&project).unwrap();let runtime=temp.path().join("runtime");
    let scope=ProjectScope::new(&project,&runtime).unwrap();let scope2=ProjectScope::new(&project.join("."),&runtime).unwrap();assert_eq!(scope.endpoint,scope2.endpoint);
    let threads=(0..8).map(|_|{let scope=scope.clone();std::thread::spawn(move||scope.token().unwrap())}).collect::<Vec<_>>();let tokens=threads.into_iter().map(|t|t.join().unwrap()).collect::<Vec<_>>();assert!(tokens.iter().all(|token|token==&tokens[0]));
    let lease1=BrokerLease::acquire(&scope).unwrap();assert!(BrokerLease::acquire(&scope).is_err());drop(lease1);drop(BrokerLease::acquire(&scope).unwrap());
    for name in ["", ".dot", "../escape", "a/b", "中文", &"a".repeat(49)]{assert!(validate_name(name).is_err(),"{name}");}for name in ["a","A0.good-name_1"]{validate_name(name).unwrap();}
    assert!(serde_json::from_value::<Operation>(json!({"op":"send","name":"a","data":"x","extra":true})).is_err());
    assert!(serde_json::from_value::<Operation>(json!({"op":"stop","name":"a","timeout_ms":"5"})).is_err());
    let mut spec=ServiceSpec{name:"one".into(),application:"executable".into(),args:vec![],env:Default::default(),cwd:project.clone(),pty:false,persist:false,detached:false};spec.validate().unwrap();spec.detached=true;assert!(spec.validate().is_err());spec.persist=true;spec.validate().unwrap();spec.pty=true;assert!(spec.validate().is_err());
    let current=process::inspect_identity(std::process::id()).unwrap().unwrap();let mut stale=current.clone();stale.created.push_str("-stale");assert!(process::stop_recovered(&stale,1).await.is_err());assert_eq!(process::inspect_identity(current.pid).unwrap(),Some(current));
    let store=Store::new(scope.clone()).unwrap();let mut daemon=snapshot("logical-a");daemon.name="one".into();spec.pty=false;spec.detached=false;let record=StoredRecord{spec,snapshot:daemon,process:None,completion_capable:true,subscription:Some("sub".into()),pending:vec![]};store.save(&record).unwrap();assert_eq!(store.load().unwrap().len(),1);
    let mut historical=record.clone();historical.spec.cwd=temp.path().join("deleted-historical-cwd");assert!(historical.spec.validate_structure().is_ok());assert!(historical.spec.validate().is_err());store.save(&historical).unwrap();assert_eq!(store.load().unwrap()[0].spec.cwd,historical.spec.cwd,"recovery validates structure without re-statting historical cwd");
    std::fs::write(store.dir("one").unwrap().join("state.json"),b"{bad").unwrap();assert!(store.load().is_err(),"corrupt recovery metadata cannot be reported as an empty successful registry");
    #[cfg(unix)]{use std::os::unix::fs::{PermissionsExt,symlink};assert_eq!(std::fs::metadata(&scope.runtime).unwrap().permissions().mode()&0o777,0o700);let target=temp.path().join("target");std::fs::create_dir(&target).unwrap();let link=temp.path().join("link");symlink(&target,&link).unwrap();assert!(private_dir(&link).is_err());}

    // Local adoption unit: no broker is started. The actual Core callback and
    // HostSink append are exercised by the process completion bank below.
    let lease=SessionServiceLease::new(&project,&runtime,std::env::current_exe().unwrap(),ServicePolicy{launch_enabled:false}).unwrap();
    assert!(lease.bind("ephemeral",false).is_err());lease.state.lock().unwrap().binding=Some(Binding{id:"logical-a".into(),persistent:true});
    assert!(lease.bind("other",true).is_err());let projector=Arc::new(AgentMessageProjector::default());
    let rx=queued(&lease,"unclaimed");let message=lease.idle_message(projector.clone(),None).unwrap().unwrap();let body=message.json().unwrap();assert_eq!(body["customType"],"launch-completion");assert_eq!(body["content"],"Supervised process one exited with exit code 0.");assert_eq!(body["details"]["daemons"][0]["id"],"generation-a");
    lease.append_receipt(&message,Ok(receipt(true)));assert!(rx.await.unwrap().is_err(),"append without ephemeral context claim must not ACK");
    let rx=queued(&lease,"wrong-durability");let message=lease.idle_message(projector.clone(),None).unwrap().unwrap();lease.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;lease.append_receipt(&message,Ok(receipt(false)));assert!(rx.await.unwrap().is_err());
    let rx=queued(&lease,"append-failed");let message=lease.idle_message(projector.clone(),None).unwrap().unwrap();lease.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;lease.append_receipt(&message,Err("real-journal-write-failed".into()));assert_eq!(rx.await.unwrap().unwrap_err(),"real-journal-write-failed");
    let rx=queued(&lease,"run-ended");let _message=lease.idle_message(projector.clone(),None).unwrap();lease.release_unappended();assert!(rx.await.unwrap().is_err());
    let mut newer=snapshot("logical-a");newer.name="one".into();newer.id="generation-b".into();newer.started_at=13;newer.state=ServiceState::Running;lease.track(&newer);
    let rx=queued(&lease,"claimed-success");let message=lease.idle_message(projector.clone(),None).unwrap().unwrap();lease.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;lease.append_receipt(&message,Ok(receipt(true)));assert_eq!(rx.await.unwrap().unwrap().entry_id,"real-append-receipt");assert!(lease.has_live_owned_service(),"old completion cannot retire replacement generation tracker");let mut stale=snapshot("logical-a");stale.name="one".into();lease.track(&stale);assert!(lease.has_live_owned_service());newer.state=ServiceState::Exited;lease.track(&newer);assert!(!lease.has_live_owned_service());

    // Original native prompt/render + array.join + Date.now graph lane.
    let prepared=PreparedNativeSession::prepare().unwrap();let mut journal=SessionJournal::create(temp.path(),&project).unwrap();let native_projector=Arc::new(AgentMessageProjector::default());native_projector.bind_native_session(&mut journal,Arc::new(crate::actual_session::GraphJournalEncoder::new(prepared.values().clone(),None).unwrap())).unwrap();
    let notifications=vec![Completion{completion_id:"zero".into(),owner:"logical-a".into(),daemon:snapshot("logical-a")},Completion{completion_id:"unknown".into(),owner:"logical-a".into(),daemon:ServiceSnapshot{exit_code:None,..snapshot("logical-a")}}];
    let graph=build_completion(&notifications,&native_projector,Some(&prepared)).unwrap();assert!(matches!(graph.body_snapshot(),ara_agent::AgentMessageBody::Graph(_)));let body=graph.json().unwrap();assert_eq!(body["content"],"Supervised process one<&> exited with exit code 0.\nSupervised process one<&> exited without an exit code.");assert!(body["timestamp"].is_number());let id=native_projector.append_current_body(&mut journal,&graph).unwrap();assert!(journal.entries().iter().find(|entry|entry.id==id).unwrap().native_message.is_some());
    // The fixed service template enters the same cached Native AST/realm.
    // Its context is read on each invocation, including exit code zero and
    // a later false branch; unknown template nodes remain explicitly rejected.
    let values=prepared.values();let heap=values.heap();let graph_runtime=values.runtime();
    let prompt=prepared.prompt().prompt();let render=heap.get(prompt,&"render".into(),graph_runtime).unwrap();
    let template=heap.string(TEMPLATE.into()).unwrap();
    let context=heap.import_serde(&json!({"name":"before<&>","state":"exited","exitCode":0,"hasExitCode":true})).unwrap();
    let first=heap.call(&render,prompt,&[template.clone(),context.clone()],graph_runtime).unwrap();
    assert_eq!(heap.to_string(&first,graph_runtime).unwrap().to_utf8().unwrap(),"Supervised process before<&> exited with exit code 0.");
    heap.define_own_data(&context,"name".into(),heap.string("late<&>".into()).unwrap(),graph_runtime).unwrap();
    heap.define_own_data(&context,"hasExitCode".into(),heap.boolean(false).unwrap(),graph_runtime).unwrap();
    let later=heap.call(&render,prompt,&[template,context.clone()],graph_runtime).unwrap();
    assert_eq!(heap.to_string(&later,graph_runtime).unwrap().to_utf8().unwrap(),"Supervised process late<&> exited without an exit code.");
    assert!(matches!(heap.call(&render,prompt,&[heap.string("{{unported_service_field}}".into()).unwrap(),context],graph_runtime),Err(ara_rpc::value_heap::ValueError::UnsupportedOperation(_))));

    // A successful custom append alone is not a durable receipt on a new
    // journal. Exercise the existing materialize barrier with physical files.
    let mut durable_journal=SessionJournal::create(&temp.path().join("durable-journal"),&project).unwrap();
    let durable=SessionServiceLease::new(&project,&runtime,std::env::current_exe().unwrap(),ServicePolicy{launch_enabled:false}).unwrap();
    durable.state.lock().unwrap().binding=Some(Binding{id:durable_journal.session_id().into(),persistent:true});
    let rx=queued(&durable,"durable-first");let message=durable.idle_message(projector.clone(),None).unwrap().unwrap();
    assert!(!durable.requires_durable_append(&message));
    durable.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;
    assert!(durable.requires_durable_append(&message));
    let lookalike=projector.new_custom_input(message.json().unwrap()).unwrap();
    assert!(!durable.requires_durable_append(&lookalike),"customType/content cannot forge a weak pending claim");
    let id=projector.append_current_body(&mut durable_journal,&message).unwrap();
    assert!(!durable_journal.is_on_disk()&&!durable_journal.path().exists(),"fresh custom append retains ordinary lazy behavior until service barrier");
    durable_journal.materialize().unwrap();assert!(durable_journal.is_on_disk()&&durable_journal.path().is_file());
    let disk=std::fs::read_to_string(durable_journal.path()).unwrap();assert!(disk.contains(&id)&&disk.contains("launch-completion"));
    durable.append_receipt(&message,Ok(CompletionAdoption{session_id:durable_journal.session_id().into(),entry_id:id,persistent:durable_journal.is_persistent()&&durable_journal.is_on_disk()}));
    assert!(rx.await.unwrap().unwrap().persistent);assert!(!durable.requires_durable_append(&message));durable.close();
    let mut failed_journal=SessionJournal::create(&temp.path().join("failed-materialize"),&project).unwrap();
    let failed=SessionServiceLease::new(&project,&runtime,std::env::current_exe().unwrap(),ServicePolicy{launch_enabled:false}).unwrap();
    failed.state.lock().unwrap().binding=Some(Binding{id:failed_journal.session_id().into(),persistent:true});
    let rx=queued(&failed,"materialize-fault");let message=failed.idle_message(projector.clone(),None).unwrap().unwrap();
    failed.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;
    assert!(failed.requires_durable_append(&message));projector.append_current_body(&mut failed_journal,&message).unwrap();
    std::fs::create_dir(failed_journal.path()).unwrap();let error=failed_journal.materialize().unwrap_err().to_string();
    failed.append_receipt(&message,Err(error.clone()));assert_eq!(rx.await.unwrap().unwrap_err(),error,"actual materialize failure rejects adoption/ACK");
    assert!(!failed_journal.is_on_disk());failed.close();

    let rx=queued(&lease,"disposed");lease.close();assert!(rx.await.unwrap().is_err());assert!(lease.bind("logical-a",true).is_err());
    let memory=SessionServiceLease::new(&project,&runtime,std::env::current_exe().unwrap(),ServicePolicy{launch_enabled:false}).unwrap();memory.state.lock().unwrap().binding=Some(Binding{id:"logical-a".into(),persistent:false});let rx=queued(&memory,"memory");let message=memory.idle_message(projector,None).unwrap().unwrap();memory.state.lock().unwrap().claims.get_mut(&message.weak_identity()).unwrap().context_claimed=true;assert!(!memory.requires_durable_append(&message),"MemoryAdopted never forces a file");memory.append_receipt(&message,Ok(receipt(false)));assert!(!rx.await.unwrap().unwrap().persistent,"MemoryAdopted must not claim durable journaling");memory.close();
}
