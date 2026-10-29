//! G09 source bank: actual HTTP/zstd/cache/OS lock/Wasmtime boundaries.
//! Candidate source only; no test execution or network was performed by its author.
use super::*;
use std::sync::atomic::{AtomicUsize,Ordering};
use tokio::io::{AsyncReadExt,AsyncWriteExt};

const INI:&[u8]=include_bytes!("../../vendor/ara-ast/src/language/testdata/ini.wasm");
fn fixture(file:&'static str)->&'static WasmGrammar {
    let digest=ring::digest::digest(&ring::digest::SHA256,INI).as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>();
    Box::leak(Box::new(WasmGrammar::new("ini","ini",file,Box::leak(digest.into_boxed_str()),INI.len() as u64)))
}
async fn server(body:Vec<u8>,status:u16,delay:Duration)->(String,Arc<AtomicUsize>,tokio::task::JoinHandle<()>) {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address=listener.local_addr().unwrap();let count=Arc::new(AtomicUsize::new(0));let observed=count.clone();
    let task=tokio::spawn(async move {
        loop {
            let (mut stream,_)=listener.accept().await.unwrap();let bytes=body.clone();let observed=observed.clone();
            tokio::spawn(async move {
                let mut request=vec![0u8;4096];let read=stream.read(&mut request).await.unwrap();
                assert!(String::from_utf8_lossy(&request[..read]).contains(wasm_grammars::RELEASE));
                observed.fetch_add(1,Ordering::SeqCst);tokio::time::sleep(delay).await;
                let head=format!("HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len());
                if stream.write_all(head.as_bytes()).await.is_ok() {let _=stream.write_all(&bytes).await;}
            });
        }
    });
    (format!("http://{address}"),count,task)
}
fn partials_absent(dir:&Path,file:&str) {
    assert!(!dir.join(format!("{file}.part")).exists());assert!(!dir.join(format!("{file}.zst.part")).exists());
}

#[tokio::test]
async fn grammar_http_singleflight_integrity_atomic_wasm_and_cache_bank() {
    let _serial=GRAMMAR_TEST_LOCK.lock().await;
    let activities=crate::download_activity::subscribe();
    let dir=tempfile::tempdir().unwrap();let info=fixture("g09-http-ini.wasm");
    let body=zstd::stream::encode_all(INI,1).unwrap();
    let (base,count,server)=server(body,200,Duration::from_millis(30)).await;
    let progress=Arc::new(AtomicUsize::new(0));let noted=progress.clone();
    let owner=HostGrammars::new(dir.path().to_owned(),base,Some(Arc::new(move |event|{
        assert!(event.loaded>0);assert_eq!(event.language,"ini");noted.fetch_add(1,Ordering::SeqCst);
    })));
    let (a,b,c)=tokio::join!(owner.ensure_one(info),owner.ensure_one(info),owner.ensure_one(info));
    assert!(a&&b&&c);assert_eq!(count.load(Ordering::SeqCst),1);assert!(progress.load(Ordering::SeqCst)>0);
    assert_eq!(fs::read(dir.path().join(info.file)).unwrap(),INI);partials_absent(dir.path(),info.file);
    assert!(owner.ensure_one(info).await);assert_eq!(count.load(Ordering::SeqCst),1);
    let activity=activities.drain().map(|notice|notice.activity).filter(|event|event.detail.as_deref()==Some(info.file)).collect::<Vec<_>>();
    assert_eq!(activity.first().unwrap().state,crate::download_activity::DownloadState::Running);
    assert_eq!(activity.last().unwrap().state,crate::download_activity::DownloadState::Done);
    assert!(activity.last().unwrap().loaded.unwrap()>0);
    assert_eq!(activity.iter().map(|event|event.id).collect::<std::collections::HashSet<_>>().len(),1,"singleflight and warm cache must not create another activity");
    // The installed bytes must load and parse through actual Wasmtime.
    grammar::set_grammar_dir(dir.path().to_owned());
    let language=info.load().expect("real downloaded wasm loads");assert!(language.is_wasm());
    let tree=grammar::parse(&language,"[core]\nname = ara\n").unwrap().unwrap();assert!(!tree.root_node().has_error());
    server.abort();
}

#[tokio::test]
async fn grammar_bad_size_hash_http_cleanup_and_failure_cooldown_bank() {
    let _serial=GRAMMAR_TEST_LOCK.lock().await;
    for (file,body,status,wrong_size,wrong_hash) in [
        ("g09-bad-size.wasm",zstd::stream::encode_all(INI,1).unwrap(),200,true,false),
        ("g09-bad-hash.wasm",zstd::stream::encode_all(INI,1).unwrap(),200,false,true),
        ("g09-bad-http.wasm",Vec::new(),503,false,false),
        ("g09-bad-zstd.wasm",b"invalid zstd".to_vec(),200,false,false),
    ] {
        let dir=tempfile::tempdir().unwrap();let valid=fixture(file);
        let info=Box::leak(Box::new(WasmGrammar::new("ini","ini",file,if wrong_hash {"00"} else {valid.sha256},valid.size+u64::from(wrong_size))));
        let (base,count,server)=server(body,status,Duration::ZERO).await;
        let owner=HostGrammars::new(dir.path().to_owned(),base,None);
        let activities=crate::download_activity::subscribe();
        assert!(!owner.ensure_one(info).await);assert!(!dir.path().join(file).exists());partials_absent(dir.path(),file);
        assert!(!owner.ensure_one(info).await,"failure cooldown prevents immediate refetch");assert_eq!(count.load(Ordering::SeqCst),1);
        let activity=activities.drain().map(|notice|notice.activity).filter(|event|event.detail.as_deref()==Some(info.file)).collect::<Vec<_>>();
        assert_eq!(activity.first().unwrap().state,crate::download_activity::DownloadState::Running);
        assert_eq!(activity.last().unwrap().state,crate::download_activity::DownloadState::Failed);
        assert!(activity.last().unwrap().error.as_ref().is_some_and(|error|!error.is_empty()));
        assert_eq!(activity.iter().map(|event|event.id).collect::<std::collections::HashSet<_>>().len(),1,"cooldown must not create a second activity");
        assert!(!activity.iter().any(|event|event.state==crate::download_activity::DownloadState::Done));
        let destination=dir.path().join(file);
        FLIGHTS.lock().unwrap().failed_at.insert(destination,Instant::now()-FAILURE_COOLDOWN-Duration::from_secs(1));
        assert!(!owner.ensure_one(info).await);assert_eq!(count.load(Ordering::SeqCst),2);
        server.abort();
    }
}

#[tokio::test]
async fn grammar_whole_transfer_timeout_and_detached_waiter_bank() {
    let _serial=GRAMMAR_TEST_LOCK.lock().await;
    let dir=tempfile::tempdir().unwrap();let info=fixture("g09-timeout.wasm");
    let (base,count,server)=server(zstd::stream::encode_all(INI,1).unwrap(),200,Duration::from_millis(250)).await;
    let owner=HostGrammars::new(dir.path().to_owned(),base,None);
    let error=owner.fetch_verify_with_timeout(info,&dir.path().join(info.file),&dir.path().join("x.zst.part"),&dir.path().join("x.part"),Duration::from_millis(25)).await.unwrap_err();
    assert_eq!(error.kind(),io::ErrorKind::TimedOut);assert!(!dir.path().join(info.file).exists());
    // Dropping a waiter must not cancel shared acquisition or cause a replay.
    {
        let future=owner.ensure_one(info);tokio::pin!(future);
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(50))=>{},_= &mut future=>panic!("fixture should still be transferring")};
    }
    assert!(owner.ensure_one(info).await);assert_eq!(count.load(Ordering::SeqCst),2);
    partials_absent(dir.path(),info.file);server.abort();
}

#[test]
fn grammar_os_lock_child_fixture() {
    let Some(directory)=std::env::var_os("ARA_G09_LOCK_CHILD") else {return;};
    let directory=PathBuf::from(directory);let file=fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(directory.join("g09-lock.wasm.install")).unwrap();
    try_install_lock(&file).unwrap();fs::write(directory.join("child-locked"),b"locked").unwrap();
    let deadline=Instant::now()+Duration::from_secs(10);
    while !directory.join("parent-release").exists() {
        assert!(Instant::now()<deadline,"parent failed to release peer lock");
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(file);
}

#[tokio::test]
async fn grammar_real_peer_process_lock_serializes_install_bank() {
    let _serial=GRAMMAR_TEST_LOCK.lock().await;
    let dir=tempfile::tempdir().unwrap();let info=fixture("g09-lock.wasm");
    struct Peer(std::process::Child);
    impl Drop for Peer {fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();let _=self.0.wait();}}}
    let mut child=Peer(std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact","host_grammars::tests::grammar_os_lock_child_fixture","--nocapture"])
        .env("ARA_G09_LOCK_CHILD",dir.path()).stdout(std::process::Stdio::null()).spawn().unwrap());
    let deadline=Instant::now()+Duration::from_secs(10);
    while !dir.path().join("child-locked").exists() {
        assert!(Instant::now()<deadline,"peer failed to take OS lock");tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (base,count,server)=server(zstd::stream::encode_all(INI,1).unwrap(),200,Duration::ZERO).await;
    // A delayed parent must still exercise a contested lease, not wait for a
    // fixed child sleep to happen to overlap this process's scheduler.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(child.0.try_wait().unwrap().is_none());
    let probe=fs::OpenOptions::new().read(true).write(true).open(dir.path().join("g09-lock.wasm.install")).unwrap();
    assert_eq!(try_install_lock(&probe).unwrap_err().kind(),io::ErrorKind::WouldBlock);drop(probe);
    let owner=HostGrammars::new(dir.path().to_owned(),base,None);
    let install=owner.ensure_one(info);tokio::pin!(install);
    tokio::select! {result=&mut install=>panic!("install crossed held peer lock: {result}"),_=tokio::time::sleep(Duration::from_millis(50))=>{}}
    assert_eq!(count.load(Ordering::SeqCst),0);assert!(!dir.path().join(info.file).exists());
    fs::write(dir.path().join("parent-release"),b"release").unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(10),&mut install).await.unwrap());
    let deadline=Instant::now()+Duration::from_secs(10);
    while child.0.try_wait().unwrap().is_none(){assert!(Instant::now()<deadline,"peer did not settle");tokio::time::sleep(Duration::from_millis(5)).await;}
    assert!(child.0.wait().unwrap().success());assert_eq!(count.load(Ordering::SeqCst),1);
    assert!(dir.path().join("g09-lock.wasm.install").exists(),"retain lock inode for future peers");
    partials_absent(dir.path(),info.file);server.abort();
}
