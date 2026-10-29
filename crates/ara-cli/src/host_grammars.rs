//! OMP b91db66e utils/grammars.ts Host port (MIT; THIRD_PARTY_NOTICES.md).
//! Copyright (c) 2025-2026 Can B枚l眉k; (c) 2026 Stencil Labs, Inc.
//! Exact catalog pins and actual wasm parsing live in ara-ast. This owner
//! supplies bounded acquisition, process/OS locking and verified atomic files.
use ara_ast::language::{SupportLang, grammar::{self,WasmGrammar}, wasm_grammars};
use ara_agent::ToolError;
use ara_tools::{ToolContext, grammars::GrammarHostPort};
use async_trait::async_trait;
use futures::future::{BoxFuture,FutureExt,Shared,join_all};
use std::{collections::HashMap,fs,io::{self,Read},path::{Path,PathBuf},sync::{Arc,LazyLock,Mutex},time::{Duration,Instant}};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
pub(crate) static GRAMMAR_TEST_LOCK:tokio::sync::Mutex<()>=tokio::sync::Mutex::const_new(());

const DEFAULT_BASE: &str="https://github.com/stencil-hq/wasm-grammars/releases/download";
const DOWNLOAD_TIMEOUT: Duration=Duration::from_secs(60);
const LOCK_WAIT: Duration=Duration::from_secs(65);
const FAILURE_COOLDOWN: Duration=Duration::from_secs(300);
type Install=Shared<BoxFuture<'static,bool>>;
#[derive(Default)]
struct Flights {pending:HashMap<PathBuf,Install>,failed_at:HashMap<PathBuf,Instant>}
static FLIGHTS: LazyLock<Mutex<Flights>>=LazyLock::new(||Mutex::new(Flights::default()));

#[derive(Clone,Debug)]
pub struct GrammarDownloadProgress {pub language:String,pub file:String,pub loaded:u64,pub total:Option<u64>}
pub type GrammarProgress=Arc<dyn Fn(GrammarDownloadProgress)+Send+Sync>;
#[derive(Clone)]
pub struct HostGrammars {dir:PathBuf,base:String,client:reqwest::Client,progress:Option<GrammarProgress>}
impl HostGrammars {
    pub fn install(context:&ToolContext,dir:PathBuf) {
        let base=std::env::var("ARA_GRAMMARS_URL").ok().map(|value|value.trim().trim_end_matches('/').to_owned()).filter(|value|!value.is_empty()).unwrap_or_else(||DEFAULT_BASE.into());
        // Constructing the owner performs no download or directory creation.
        grammar::set_grammar_dir(dir.clone());
        context.set_grammar_host(Arc::new(Self::new(dir,base,None)));
    }
    pub fn new(dir:PathBuf,base:String,progress:Option<GrammarProgress>)->Self {
        Self {dir,base:base.trim_end_matches('/').to_owned(),client:reqwest::Client::new(),progress}
    }
    async fn ensure_one(&self,info:&'static WasmGrammar)->bool {
        if info.is_installed() {return true;}
        let dest=self.dir.join(info.file);
        let future={
            let mut flights=FLIGHTS.lock().unwrap_or_else(|error|error.into_inner());
            if let Some(pending)=flights.pending.get(&dest) {pending.clone()}
            else {
                if flights.failed_at.get(&dest).is_some_and(|failed|failed.elapsed()<FAILURE_COOLDOWN) {return false;}
                let owner=self.clone();let destination=dest.clone();
                // Shared acquisition starts once and keeps running to its own
                // bounded settlement even if every caller stops waiting.
                let task=tokio::spawn(async move {
                    let result=owner.install_locked(info,&destination).await;
                    let success=match result {Ok(())=>true,Err(error)=>{crate::terminal::driver::report_diagnostic(&format!("ara: grammar install failed; {} files are skipped: {error}",info.language));false}};
                    let mut flights=FLIGHTS.lock().unwrap_or_else(|error|error.into_inner());
                    flights.pending.remove(&destination);
                    if !success {flights.failed_at.insert(destination,Instant::now());}
                    success
                });
                let future=async move {task.await.unwrap_or(false)}.boxed().shared();
                flights.pending.insert(dest,future.clone());future
            }
        };
        future.await
    }
    async fn install_locked(&self,info:&'static WasmGrammar,dest:&Path)->io::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        let lock_path=dest.with_file_name(format!("{}.install",info.file));
        let lock=fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(lock_path)?;
        let deadline=tokio::time::Instant::now()+LOCK_WAIT;
        loop {
            match try_install_lock(&lock) {
                Ok(())=>break,
                Err(error) if error.kind()==io::ErrorKind::WouldBlock=>{
                    if tokio::time::Instant::now()>=deadline {return Err(io::Error::new(io::ErrorKind::TimedOut,"grammar install lock timed out after65000ms"));}
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error)=>return Err(error),
            }
        }
        // Never unlink the lock inode: independent processes must all acquire
        // the same OS lock. Closing its descriptor releases ownership on exit.
        let _lock=lock;
        if tokio::fs::try_exists(dest).await? {return Ok(());}
        let compressed=self.dir.join(format!("{}.zst.part",info.file));
        let part=self.dir.join(format!("{}.part",info.file));
        let tracker=crate::download_activity::track_download(format!("{} grammar",info.language),crate::download_activity::DownloadProgress{detail:Some(info.file.into()),..Default::default()});
        let _scope=crate::download_activity::DownloadScope(tracker.clone());
        let received=tracker.clone();let previous=self.progress.clone();let mut tracked=self.clone();
        tracked.progress=Some(Arc::new(move|event|{
            received.update(crate::download_activity::DownloadProgress{loaded:Some(event.loaded),total:event.total,..Default::default()});
            if let Some(observer)=&previous {observer(event);}
        }));
        let result=tracked.fetch_verify(info,dest,&compressed,&part).await;
        for temporary in [&compressed,&part] {
            match tokio::fs::remove_file(temporary).await {Ok(())=>{},Err(error) if error.kind()==io::ErrorKind::NotFound=>{},Err(error)=>crate::terminal::driver::report_diagnostic(&format!("ara: grammar partial cleanup failed: {error}"))}
        }
        match &result {Ok(())=>tracker.done(),Err(error)=>tracker.fail(error)}
        result
    }
    async fn fetch_verify(&self,info:&'static WasmGrammar,dest:&Path,compressed:&Path,part:&Path)->io::Result<()> {
        self.fetch_verify_with_timeout(info,dest,compressed,part,DOWNLOAD_TIMEOUT).await
    }
    async fn fetch_verify_with_timeout(&self,info:&'static WasmGrammar,dest:&Path,compressed:&Path,part:&Path,timeout:Duration)->io::Result<()> {
        let url=format!("{}/{}/{}.zst",self.base,wasm_grammars::RELEASE,info.file);
        let transfer=async {
            let mut response=self.client.get(url).send().await.map_err(io::Error::other)?.error_for_status().map_err(io::Error::other)?;
            let total=response.content_length();let mut loaded=0u64;
            let mut file=tokio::fs::File::create(compressed).await?;
            while let Some(chunk)=response.chunk().await.map_err(io::Error::other)? {
                file.write_all(&chunk).await?;loaded=loaded.saturating_add(chunk.len() as u64);
                if let Some(progress)=&self.progress {progress(GrammarDownloadProgress {language:info.language.into(),file:info.file.into(),loaded,total});}
            }
            file.flush().await?;Ok::<_,io::Error>(())
        };
        tokio::time::timeout(timeout,transfer).await.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"grammar download timed out after60000ms"))??;
        let compressed=compressed.to_owned();let part=part.to_owned();let destination=dest.to_owned();
        tokio::task::spawn_blocking(move || {
            let input=fs::File::open(&compressed)?;
            let mut decoder=zstd::stream::read::Decoder::new(input)?.take(info.size.saturating_add(1));
            let mut wasm=Vec::with_capacity(usize::try_from(info.size).map_err(io::Error::other)?);
            decoder.read_to_end(&mut wasm)?;
            if wasm.len() as u64!=info.size {return Err(io::Error::new(io::ErrorKind::InvalidData,format!("{}: expected {} bytes, received {}",info.file,info.size,wasm.len())));}
            let digest=ring::digest::digest(&ring::digest::SHA256,&wasm).as_ref().iter().map(|byte|format!("{byte:02x}")).collect::<String>();
            if !digest.eq_ignore_ascii_case(info.sha256) {return Err(io::Error::new(io::ErrorKind::InvalidData,format!("{}: SHA-256 mismatch ({digest})",info.file)));}
            fs::write(&part,wasm)?;
            fs::rename(part,destination)
        }).await.map_err(io::Error::other)?
    }
}
#[async_trait]
impl GrammarHostPort for HostGrammars {
    async fn ensure_grammars(&self,languages:&[String],cancel:CancellationToken)->Result<bool,ToolError> {
        let requests=languages.iter().filter_map(|name|SupportLang::from_alias(name).and_then(SupportLang::wasm_grammar)).map(|info|self.ensure_one(info));
        tokio::select! {biased;_=cancel.cancelled()=>Err(ToolError("Aborted: Cancelled".into())),results=join_all(requests)=>Ok(results.into_iter().any(|installed|installed))}
    }
}

// std::fs::File::try_lock is newer than this workspace's Rust1.88 MSRV.
// Use the Host's existing OS bindings. Both locks are released when File drops.
#[cfg(unix)]
fn try_install_lock(file:&fs::File)->io::Result<()> {
    use std::os::fd::AsRawFd;
    if unsafe {libc::flock(file.as_raw_fd(),libc::LOCK_EX|libc::LOCK_NB)}==0 {Ok(())}
    else {Err(io::Error::last_os_error())}
}
#[cfg(windows)]
fn try_install_lock(file:&fs::File)->io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{Foundation::{ERROR_LOCK_VIOLATION,ERROR_SHARING_VIOLATION},Storage::FileSystem::LockFile};
    if unsafe {LockFile(file.as_raw_handle() as _,0,0,u32::MAX,u32::MAX)}!=0 {return Ok(());}
    let error=io::Error::last_os_error();
    if matches!(error.raw_os_error().map(|n|n as u32),Some(ERROR_LOCK_VIOLATION)|Some(ERROR_SHARING_VIOLATION)) {
        Err(io::Error::new(io::ErrorKind::WouldBlock,error))
    } else {Err(error)}
}
#[cfg(not(any(unix,windows)))]
fn try_install_lock(_file:&fs::File)->io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported,"Host grammar OS lock is unavailable"))
}

#[cfg(test)]
#[path = "host_grammars_tests.rs"]
mod tests;
