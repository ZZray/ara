//! Host filesystem owner for fixed OMP b91db66e stream.ts provider in-flight leases.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. MIT; see THIRD_PARTY_NOTICES.md.
//! The shared Core sees only the opaque permit. Paths, process identity and
//! filesystem coordination remain properties of this reference Host.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::{FutureExt, future::{BoxFuture, Shared}};
use serde::{Deserialize, Serialize};
use std::{io, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, SystemTime, UNIX_EPOCH}};
use tokio::fs;
use tokio_util::sync::CancellationToken;
use ara_ai::provider_dispatch::{ProviderInFlightPort, ProviderPermit, ProviderCoordinationError, ProviderLimits, effective_provider_limit};
use ara_rpc::WireString;

const LOCK_STALE: u64 = 10_000;
const LEASE_STALE: u64 = 30_000;
const HEARTBEAT: u64 = 5_000;
const SIGNAL_FALLBACK: u64 = 250;
const FLUSH_TIMEOUT: u64 = 1_000;
const RELEASE_TIMEOUT: u64 = 5_000;

type SharedRelease = Shared<BoxFuture<'static, Result<(), Arc<io::Error>>>>;

#[derive(Clone)]
pub struct ProviderInFlightOwner {
    root: PathBuf,
    hooks: Arc<ProviderInFlightTestHooks>,
}
type PathOperation = Arc<dyn Fn(PathBuf) -> BoxFuture<'static, io::Result<()>> + Send + Sync>;
type HeartbeatOperation = Arc<dyn Fn(BoxFuture<'static, io::Result<()>>) -> BoxFuture<'static, io::Result<()>> + Send + Sync>;
/// Exact seams used by the fixed upstream provider-inflight test bank. They
/// do not replace admission, lock identity, real files or process capacity.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct ProviderInFlightTestHooks {
    pub heartbeat_ms: Option<u64>,
    pub heartbeat_flush_timeout_ms: Option<u64>,
    pub heartbeat_writer: Option<HeartbeatOperation>,
    pub lease_remover: Option<PathOperation>,
    pub lock_created: Option<PathOperation>,
    pub lock_identified: Option<PathOperation>,
    pub lock_mkdir: Option<PathOperation>,
    pub wait_observer: Option<Arc<dyn Fn(PathBuf) + Send + Sync>>,
    pub after_acquire: Option<Arc<dyn Fn() + Send + Sync>>,
    pub lock_platform_win32: Option<bool>,
    pub lock_retry_budget_ms: Option<u64>,
    pub lock_retry_initial_ms: Option<u64>,
    pub lock_retry_max_ms: Option<u64>,
}
#[derive(Serialize, Deserialize)]
struct Info { pid: u32, timestamp: f64, token: String }
#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity { device: u64, file: u64, birth: u64 }
enum StaleLock { Token(String), Modified(SystemTime) }

impl ProviderInFlightOwner {
    /// The application chooses its state root. All bindings must share it.
    pub fn new(root: PathBuf) -> Arc<Self> { Arc::new(Self { root, hooks: Arc::new(ProviderInFlightTestHooks::default()) }) }
    #[doc(hidden)]
    pub fn with_test_hooks(self: &Arc<Self>, hooks: ProviderInFlightTestHooks) -> Arc<Self> { Arc::new(Self { root: self.root.clone(), hooks: Arc::new(hooks) }) }
    #[doc(hidden)]
    pub async fn capture_stale_lock_release(&self, provider: &str) -> io::Result<Option<BoxFuture<'static, ()>>> {
        let path = self.provider_dir(provider).with_extension("lock");
        Ok(stale_lock(&path).await?.map(|stale| async move { release_stale_lock(&path, stale).await; }.boxed()))
    }
    #[doc(hidden)]
    pub async fn capture_lock_identity_release(&self, provider: &str) -> Option<BoxFuture<'static, ()>> {
        let path = self.provider_dir(provider).with_extension("lock");
        let identity = file_identity(&path).await.ok()?;
        Some(async move {
            if read_info(&path).await.is_none() && file_identity(&path).await.is_ok_and(|now| now == identity) { let _ = remove_dir_if_present(&path).await; }
        }.boxed())
    }
    pub fn provider_dir(&self, provider: &str) -> PathBuf {
        let digest = ring::digest::digest(&ring::digest::SHA256, provider.as_bytes());
        self.root.join(URL_SAFE_NO_PAD.encode(digest.as_ref()))
    }
    pub async fn acquire(self: &Arc<Self>, provider: &str, limit: usize, cancel: CancellationToken) -> io::Result<Arc<HostProviderPermit>> {
        let dir = self.provider_dir(provider);
        loop {
            check_cancel(&cancel)?;
            if let Some(permit) = self.try_acquire(&dir, limit.max(1), &cancel).await? {
                if let Some(observer) = &self.hooks.after_acquire { observer(); }
                if let Err(error) = check_cancel(&cancel) {
                    let _ = permit.release_once().await;
                    return Err(error);
                }
                return Ok(permit);
            }
            wait_signal(dir.clone(), &cancel, &self.hooks).await?;
        }
    }
    async fn try_acquire(&self, dir: &Path, limit: usize, cancel: &CancellationToken) -> io::Result<Option<Arc<HostProviderPermit>>> {
        let lock_dir = dir.with_extension("lock");
        let lock = acquire_lock(lock_dir, dir, cancel, &self.hooks).await?;
        let result = async {
            fs::create_dir_all(dir).await?;
            let mut entries = fs::read_dir(dir).await?;
            let mut active = 0;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                let metadata = match fs::metadata(&path).await {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                if !metadata.is_dir() { continue; }
                if stale_lease(&path).await? { remove_dir_if_present(&path).await?; }
                else { active += 1; }
            }
            if active >= limit { return Ok(None); }
            let path = dir.join(format!("{}-{}-{}", std::process::id(), epoch_ms() as u64, uuid::Uuid::new_v4()));
            let token = uuid::Uuid::new_v4().to_string();
            let setup = async { fs::create_dir(&path).await?; write_info(&path, &token).await }.await;
            if let Err(error) = setup { let _ = remove_lease_dir(&path).await; return Err(error); }
            Ok(Some(HostProviderPermit::start(path, token, self.hooks.clone())))
        }.await;
        lock.release().await;
        result
    }
}

impl ProviderInFlightPort for ProviderInFlightOwner {
    fn acquire(&self, provider: WireString, limit: usize, cancel: CancellationToken) -> BoxFuture<'static, Result<Arc<dyn ProviderPermit>, ProviderCoordinationError>> {
        let owner = Arc::new(self.clone());
        // Bun's SHA256 string boundary encodes lone UTF-16 surrogates as U+FFFD.
        let provider = String::from_utf16_lossy(provider.units());
        async move {
            owner.acquire(&provider, limit, cancel.clone()).await
                .map(|permit| permit as Arc<dyn ProviderPermit>)
                .map_err(|error| if cancel.is_cancelled() { ProviderCoordinationError::Cancelled } else { ProviderCoordinationError::Failed(error.to_string()) })
        }.boxed()
    }
}
impl ProviderPermit for HostProviderPermit {
    fn release_once(&self) -> BoxFuture<'static, Result<(), ProviderCoordinationError>> {
        let cleanup = HostProviderPermit::release_once(self);
        async move { cleanup.await.map_err(|error| ProviderCoordinationError::Failed(error.to_string())) }.boxed()
    }
}

#[derive(Clone)]
pub struct ProviderInFlightConfiguration {
    pub owner: Arc<dyn ProviderInFlightPort>,
    pub limits: ProviderLimits,
}
fn configuration_slot() -> &'static std::sync::RwLock<ProviderInFlightConfiguration> {
    static CONFIGURATION: std::sync::OnceLock<std::sync::RwLock<ProviderInFlightConfiguration>> = std::sync::OnceLock::new();
    CONFIGURATION.get_or_init(|| {
        let home = std::env::var_os("ARA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(|home| PathBuf::from(home).join(".ara"))).unwrap_or_else(|| PathBuf::from(".ara"));
        let home = std::path::absolute(&home).unwrap_or(home);
        std::sync::RwLock::new(ProviderInFlightConfiguration { owner: ProviderInFlightOwner::new(home.join("run").join("provider-inflight")), limits: ProviderLimits::new() })
    })
}
/// Reference Host equivalent of configureProviderMaxInFlightRequests. This
/// binds the same real OS owner for daily routes and registered builtin/custom
/// dispatches. An empty map preserves the source unlimited default.
pub fn configure_provider_inflight(configuration: ProviderInFlightConfiguration) {
    *configuration_slot().write().unwrap_or_else(|e| e.into_inner()) = configuration;
}
pub fn configuration() -> ProviderInFlightConfiguration { configuration_slot().read().unwrap_or_else(|e| e.into_inner()).clone() }

/// Gate one logical dispatch, including its same-route credential retries.
/// Concrete provider HTTP retries do not acquire a second slot.
pub fn wrap_model_provider(inner: Arc<dyn ara_ai::ModelProvider>) -> Arc<dyn ara_ai::ModelProvider> { Arc::new(LimitedProvider { inner }) }
struct LimitedProvider { inner: Arc<dyn ara_ai::ModelProvider> }
async fn release_best_effort(permit: &Arc<dyn ProviderPermit>) {
    if let Err(error) = permit.release_once().await { eprintln!("Provider in-flight permit release failed: {error}"); }
}
impl ara_ai::ModelProvider for LimitedProvider {
    fn stream(&self, model: &ara_ai::Model, context: &ara_ai::Context, mut options: ara_ai::CallOptions) -> ara_ai::AssistantStream {
        let policy = configuration();
        let key: WireString = model.provider.as_str().into();
        let Some(limit) = effective_provider_limit(&key, options.max_in_flight_requests.as_ref(), &policy.limits) else { return self.inner.stream(model, context, options); };
        options.wait_for_terminal_drain = true;
        let model = model.clone(); let context = context.clone(); let inner_provider = self.inner.clone();
        let cancel = options.cancel.child_token(); options.cancel = cancel.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(256);
        tokio::spawn(async move {
            let _cancel_on_drop = cancel.clone().drop_guard();
            let acquiring = policy.owner.acquire(key, limit, cancel.clone());
            tokio::pin!(acquiring);
            let acquired = tokio::select! { biased; _ = tx.closed() => { cancel.cancel(); acquiring.await }, result = &mut acquiring => result };
            let permit = match acquired { Ok(permit) => permit, Err(error) => { let _ = tx.send(coordination_event(&model, &error)).await; return; } };
            if cancel.is_cancelled() { release_best_effort(&permit).await; let _ = tx.send(coordination_event(&model, &ProviderCoordinationError::Cancelled)).await; return; }
            let mut inner = inner_provider.stream(&model, &context, options);
            let mut terminal = None;
            loop {
                let next = tokio::select! { biased; _ = tx.closed(), if !cancel.is_cancelled() => { cancel.cancel(); inner.recv().await }, event = inner.recv() => event };
                let Some(event) = next else { break; };
                if event.is_terminal() { terminal = Some(event); continue; }
                if terminal.is_none() && tx.send(event).await.is_err() { cancel.cancel(); }
            }
            // Chat's wait flag holds its producer through actual bounded body
            // drain. Receiver closure also joins the route's auth retry owner.
            release_best_effort(&permit).await;
            if let Some(terminal) = terminal { let _ = tx.send(terminal).await; }
        });
        rx
    }
    fn stream_actual(&self, model: &ara_ai::Model, context: &ara_ai::GraphProviderContext, factory: Arc<ara_ai::graph_values::GraphValueFactory>, mut options: ara_ai::CallOptions) -> Result<ara_ai::ActualAssistantStream, ara_rpc::value_heap::ValueError> {
        context.check_lineage(&factory)?;
        let policy = configuration(); let key: WireString = model.provider.as_str().into();
        let Some(limit) = effective_provider_limit(&key, options.max_in_flight_requests.as_ref(), &policy.limits) else { return self.inner.stream_actual(model, context, factory, options); };
        options.wait_for_terminal_drain = true;
        let model = model.clone(); let context = context.clone(); let inner_provider = self.inner.clone();
        let cancel = options.cancel.child_token(); options.cancel = cancel.clone();
        let (sink, outer, receipt) = ara_ai::ActualAssistantStream::channel();
        let settled = receipt.owner_settlement_guard();
        tokio::spawn(async move {
            let _settled = settled; let _cancel_on_drop = cancel.clone().drop_guard();
            let acquiring = policy.owner.acquire(key, limit, cancel.clone());
            tokio::pin!(acquiring);
            let acquired = tokio::select! { biased; _ = sink.closed() => { cancel.cancel(); acquiring.await }, result = &mut acquiring => result };
            let permit = match acquired {
                Ok(permit) => permit,
                Err(error) => { publish_coordination_actual(&sink, &factory, &model, error, &receipt).await; return; }
            };
            if cancel.is_cancelled() { release_best_effort(&permit).await; publish_coordination_actual(&sink, &factory, &model, ProviderCoordinationError::Cancelled, &receipt).await; return; }
            let mut inner = match inner_provider.stream_actual(&model, &context, factory.clone(), options) {
                Ok(inner) => inner,
                Err(error) => { release_best_effort(&permit).await; receipt.record(error); return; }
            };
            let original_receipt = inner.failure_receipt(); receipt.inherit_stream(&inner);
            let mut terminal = None;
            loop {
                let next = tokio::select! { biased; _ = sink.closed(), if !cancel.is_cancelled() => { cancel.cancel(); inner.recv().await }, event = inner.recv() => event };
                match next {
                    Some(event) if event.is_terminal() => { terminal = Some(event); },
                    Some(event) => { if terminal.is_none() && !sink.push(event).await { cancel.cancel(); } },
                    None => break,
                }
            }
            // This is an actual producer receipt, not a terminal Boolean.
            let _ = original_receipt.wait_settled().await;
            release_best_effort(&permit).await;
            if let Some(terminal) = terminal { receipt.retain_terminal(&terminal); let _ = sink.push(terminal).await; }
        });
        Ok(outer)
    }
}
fn coordination_event(model: &ara_ai::Model, error: &ProviderCoordinationError) -> ara_ai::AssistantMessageEvent {
    let mut message = ara_ai::AssistantMessage::empty(&model.api, &model.provider, &model.id);
    message.stop_reason = if matches!(error, ProviderCoordinationError::Cancelled) { ara_ai::StopReason::Aborted } else { ara_ai::StopReason::Error };
    message.error_message = Some(error.to_string());
    ara_ai::AssistantMessageEvent::Error { reason: message.stop_reason, error: message }
}
async fn publish_coordination_actual(sink: &ara_ai::event::EventSink<ara_ai::ActualAssistantEvent>, factory: &Arc<ara_ai::graph_values::GraphValueFactory>, model: &ara_ai::Model, error: ProviderCoordinationError, receipt: &ara_ai::ActualStreamFailureReceipt) {
    let reason = if matches!(error, ProviderCoordinationError::Cancelled) { ara_ai::StopReason::Aborted } else { ara_ai::StopReason::Error };
    match crate::model_route::actual_error_event(model, factory, reason, &error.to_string()) {
        Ok(event) => { receipt.retain_terminal(&event); let _ = sink.push(event).await; },
        Err(error) => receipt.record(error),
    }
}

pub struct HostProviderPermit {
    path: PathBuf,
    active: Arc<AtomicBool>,
    heartbeat_stop: CancellationToken,
    heartbeat: Mutex<Option<tokio::task::JoinHandle<()>>>,
    release: Mutex<Option<SharedRelease>>,
    hooks: Arc<ProviderInFlightTestHooks>,
}
impl HostProviderPermit {
    fn start(path: PathBuf, token: String, hooks: Arc<ProviderInFlightTestHooks>) -> Arc<Self> {
        let active = Arc::new(AtomicBool::new(true));
        let heartbeat_stop = CancellationToken::new();
        let worker_active = active.clone();
        let worker_stop = heartbeat_stop.clone();
        let worker_path = path.clone();
        let worker_hooks = hooks.clone();
        let heartbeat = tokio::spawn(async move {
            // Sequential writes provide the source heartbeat flush chain.
            loop {
                tokio::select! { biased; _ = worker_stop.cancelled() => break, _ = tokio::time::sleep(Duration::from_millis(worker_hooks.heartbeat_ms.unwrap_or(HEARTBEAT))) => {} }
                if !worker_active.load(Ordering::Acquire) { break; }
                let active = worker_active.clone(); let path = worker_path.clone(); let token = token.clone();
                let write = async move { if active.load(Ordering::Acquire) { write_info(&path, &token).await } else { Ok(()) } }.boxed();
                if let Some(writer) = &worker_hooks.heartbeat_writer { let _ = writer(write).await; }
                else { let _ = write.await; }
            }
        });
        Arc::new(Self { path, active, heartbeat_stop, heartbeat: Mutex::new(Some(heartbeat)), release: Mutex::new(None), hooks })
    }
    /// Memoize cleanup, including failure. Repeated waiters never remove twice.
    pub fn release_once(&self) -> SharedRelease {
        let mut shared = self.release.lock().unwrap_or_else(|e| e.into_inner());
        shared.get_or_insert_with(|| {
            self.active.store(false, Ordering::Release);
            self.heartbeat_stop.cancel();
            let heartbeat = self.heartbeat.lock().unwrap_or_else(|e| e.into_inner()).take();
            let path = self.path.clone();
            let hooks = self.hooks.clone();
            async move {
                if let Some(heartbeat) = heartbeat {
                    // Dropping the JoinHandle on timeout does not abort an
                    // already-entered filesystem write. write_info never mkdirs.
                    let _ = tokio::time::timeout(Duration::from_millis(hooks.heartbeat_flush_timeout_ms.unwrap_or(FLUSH_TIMEOUT)), heartbeat).await;
                }
                let removal = async { match &hooks.lease_remover { Some(remover) => remover(path.clone()).await, None => remove_lease_dir(&path).await } };
                let remove = tokio::time::timeout(Duration::from_millis(RELEASE_TIMEOUT), removal).await;
                match remove {
                    Ok(Ok(())) => {},
                    Ok(Err(error)) => return Err(Arc::new(error)),
                    Err(_) => return Err(Arc::new(io::Error::new(io::ErrorKind::TimedOut, "Provider in-flight lease cleanup timed out"))),
                }
                // Use the acquired lease's directory, not a mutable root.
                if let Some(dir) = path.parent() {
                    let dir = dir.to_owned();
                    tokio::spawn(async move { signal_waiters(&dir).await; });
                }
                Ok(())
            }.boxed().shared()
        }).clone()
    }
}
impl Drop for HostProviderPermit {
    fn drop(&mut self) {
        // Graph code never runs on this cleanup worker. An explicit dispatch
        // completion awaits the same shared future before publishing success.
        let cleanup = self.release_once();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move { let _ = cleanup.await; });
        }
    }
}

fn epoch_ms() -> f64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64() * 1000.0 }
fn old(timestamp: f64, timeout: u64) -> bool { epoch_ms() - timestamp > timeout as f64 }
fn old_mtime(timestamp: SystemTime, timeout: u64) -> bool { SystemTime::now().duration_since(timestamp).is_ok_and(|age| age > Duration::from_millis(timeout)) }
fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() { Err(io::Error::new(io::ErrorKind::Interrupted, "Provider request aborted before dispatch")) } else { Ok(()) }
}
async fn read_info(dir: &Path) -> Option<Info> { serde_json::from_slice(&fs::read(dir.join("info.json")).await.ok()?).ok() }
async fn write_info(dir: &Path, token: &str) -> io::Result<()> {
    let info = Info { pid: std::process::id(), timestamp: epoch_ms(), token: token.to_owned() };
    let temp = dir.join(format!(".info-{}-{}.tmp", info.pid, uuid::Uuid::new_v4()));
    let result = async {
        fs::write(&temp, serde_json::to_vec(&info).map_err(io::Error::other)?).await?;
        fs::rename(&temp, dir.join("info.json")).await
    }.await;
    if result.is_err() { let _ = fs::remove_file(&temp).await; }
    result
}
async fn stale_lease(dir: &Path) -> io::Result<bool> {
    if let Some(info) = read_info(dir).await { return Ok(!process_alive(info.pid) || old(info.timestamp, LEASE_STALE)); }
    match fs::metadata(dir.join("info.json")).await {
        Ok(info) => return Ok(old_mtime(info.modified()?, LEASE_STALE)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }
    match fs::metadata(dir).await {
        Ok(info) => Ok(old_mtime(info.modified()?, LEASE_STALE)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}
async fn stale_lock(dir: &Path) -> io::Result<Option<StaleLock>> {
    if let Some(info) = read_info(dir).await { return Ok((!process_alive(info.pid)).then_some(StaleLock::Token(info.token))); }
    match fs::metadata(dir).await {
        Ok(info) => { let modified = info.modified()?; Ok(old_mtime(modified, LOCK_STALE).then_some(StaleLock::Modified(modified))) },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}
async fn remove_dir_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path).await { Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()), result => result }
}
async fn remove_lease_dir(path: &Path) -> io::Result<()> {
    for attempt in 0..3 {
        match remove_dir_if_present(path).await {
            Ok(()) => return Ok(()),
            Err(error) if attempt < 2 && transient_remove(&error) => tokio::time::sleep(Duration::from_millis(25)).await,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}
fn transient_remove(error: &io::Error) -> bool {
    #[cfg(windows)] { matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145)) }
    #[cfg(unix)] { matches!(error.raw_os_error(), Some(libc::EBUSY | libc::ENOTEMPTY | libc::EPERM)) }
    #[cfg(not(any(windows, unix)))] { let _ = error; false }
}
async fn release_token_lock(dir: &Path, token: &str) {
    if read_info(dir).await.is_some_and(|info| info.token == token) { let _ = remove_dir_if_present(dir).await; }
}
async fn release_stale_lock(dir: &Path, stale: StaleLock) {
    match stale {
        StaleLock::Token(token) => release_token_lock(dir, &token).await,
        StaleLock::Modified(expected) => {
            if read_info(dir).await.is_some() { return; }
            if let Ok(info) = fs::metadata(dir).await {
                if info.modified().is_ok_and(|mtime| mtime == expected && old_mtime(mtime, LOCK_STALE)) { let _ = remove_dir_if_present(dir).await; }
            }
        }
    }
}
struct ProviderLock { path: PathBuf, token: String, released: bool }
impl ProviderLock { async fn release(mut self) { release_token_lock(&self.path, &self.token).await; self.released = true; } }
impl Drop for ProviderLock {
    fn drop(&mut self) {
        if !self.released {
            let path = self.path.clone(); let token = self.token.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() { runtime.spawn(async move { release_token_lock(&path, &token).await; }); }
        }
    }
}
async fn acquire_lock(path: PathBuf, provider_dir: &Path, cancel: &CancellationToken, hooks: &Arc<ProviderInFlightTestHooks>) -> io::Result<ProviderLock> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).await?; }
    loop {
        check_cancel(cancel)?;
        match mkdir_lock(&path, cancel, hooks).await {
            Ok(()) => {
                if let Some(observer) = &hooks.lock_created { observer(path.clone()).await?; }
                let identity = match file_identity(&path).await {
                    Ok(identity) => identity,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                };
                let token = uuid::Uuid::new_v4().to_string();
                let publish = async { if let Some(observer) = &hooks.lock_identified { observer(path.clone()).await?; } write_info(&path, &token).await }.await;
                if let Err(error) = publish {
                    if read_info(&path).await.is_none() && file_identity(&path).await.is_ok_and(|now| now == identity) { let _ = remove_dir_if_present(&path).await; }
                    if error.kind() == io::ErrorKind::NotFound { continue; }
                    return Err(error);
                }
                return Ok(ProviderLock { path, token, released: false });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
        if let Some(stale) = stale_lock(&path).await? { release_stale_lock(&path, stale).await; signal_waiters(provider_dir).await; continue; }
        wait_signal(provider_dir.to_owned(), cancel, hooks).await?;
    }
}
async fn mkdir_lock(path: &Path, cancel: &CancellationToken, hooks: &Arc<ProviderInFlightTestHooks>) -> io::Result<()> {
    let mut failure_count = 0u32;
    let mut started = std::time::Instant::now();
    loop {
        let result = match &hooks.lock_mkdir { Some(mkdir) => mkdir(path.to_owned()).await, None => fs::create_dir(path).await };
        match result {
            Ok(()) => return Ok(()),
            Err(error) => {
                let transient = hooks.lock_platform_win32.unwrap_or(cfg!(windows)) && error.kind() == io::ErrorKind::PermissionDenied;
                if !transient { return Err(error); }
                if failure_count == 0 { started = std::time::Instant::now(); }
                if started.elapsed() >= Duration::from_millis(hooks.lock_retry_budget_ms.unwrap_or(3_000)) { return Err(error); }
                let delay = hooks.lock_retry_initial_ms.unwrap_or(25).saturating_mul(1u64.checked_shl(failure_count).unwrap_or(u64::MAX)).min(hooks.lock_retry_max_ms.unwrap_or(250));
                failure_count = failure_count.saturating_add(1);
                tokio::select! { biased; _ = cancel.cancelled() => return check_cancel(cancel), _ = tokio::time::sleep(Duration::from_millis(delay)) => {} }
            }
        }
    }
}
async fn signal_waiters(dir: &Path) { if fs::create_dir_all(dir).await.is_ok() { let _ = fs::write(dir.join(".wakeup"), epoch_ms().to_string()).await; } }
async fn wait_signal(dir: PathBuf, cancel: &CancellationToken, hooks: &Arc<ProviderInFlightTestHooks>) -> io::Result<()> {
    check_cancel(cancel)?;
    if let Some(observer) = &hooks.wait_observer { observer(dir.clone()); }
    let since = SystemTime::now();
    let watch_dir = dir.clone();
    let stopped = Arc::new(AtomicBool::new(false)); let worker_stop = stopped.clone();
    let mut watch = tokio::task::spawn_blocking(move || watch_signal(&watch_dir, &worker_stop));
    let already_signalled = fs::metadata(dir.join(".wakeup")).await.ok().and_then(|info| info.modified().ok()).is_some_and(|mtime| mtime >= since);
    let result = if already_signalled { Ok(()) } else {
        tokio::select! { biased; _ = cancel.cancelled() => check_cancel(cancel), _ = &mut watch => Ok(()), _ = tokio::time::sleep(Duration::from_millis(SIGNAL_FALLBACK)) => Ok(()) }
    };
    stopped.store(true, Ordering::Release);
    // Join watcher teardown before returning; an abandoned Windows directory
    // watcher must not make the next lock mkdir delete-pending unexpectedly.
    if !watch.is_finished() { let _ = watch.await; }
    result
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool { unsafe { libc::kill(pid as libc::pid_t, 0) == 0 || io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) } }
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER}, System::Threading::{OpenProcess, GetExitCodeProcess, PROCESS_QUERY_LIMITED_INFORMATION}};
    unsafe { let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid); if handle.is_null() { return GetLastError() != ERROR_INVALID_PARAMETER; } let mut code = 0; let ok = GetExitCodeProcess(handle, &mut code); CloseHandle(handle); ok == 0 || code == 259 }
}
#[cfg(not(any(windows, unix)))] fn process_alive(_: u32) -> bool { true }

#[cfg(unix)]
async fn file_identity(path: &Path) -> io::Result<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let info = fs::metadata(path).await?;
    Ok(FileIdentity { device: info.dev(), file: info.ino(), birth: 0 })
}
#[cfg(windows)]
async fn file_identity(path: &Path) -> io::Result<FileIdentity> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::{Foundation::{CloseHandle, INVALID_HANDLE_VALUE}, Storage::FileSystem::{CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_SHARE_DELETE, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS}};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe { let handle = CreateFileW(wide.as_ptr(), FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, std::ptr::null(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut()); if handle == INVALID_HANDLE_VALUE { return Err(io::Error::last_os_error()); } let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed(); let ok = GetFileInformationByHandle(handle, &mut info); let error = io::Error::last_os_error(); CloseHandle(handle); if ok == 0 { return Err(error); } let file = ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64; Ok(FileIdentity { device: info.dwVolumeSerialNumber as u64, file, birth: if file == 0 { ((info.ftCreationTime.dwHighDateTime as u64) << 32) | info.ftCreationTime.dwLowDateTime as u64 } else { 0 } }) }
    }).await.map_err(io::Error::other)?
}
#[cfg(not(any(windows, unix)))] async fn file_identity(_: &Path) -> io::Result<FileIdentity> { Err(io::Error::new(io::ErrorKind::Unsupported, "Provider coordination filesystem identity is unavailable")) }

#[cfg(windows)]
fn watch_signal(dir: &Path, stopped: &AtomicBool) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{Foundation::INVALID_HANDLE_VALUE, Storage::FileSystem::{FindFirstChangeNotificationW, FindCloseChangeNotification, FILE_NOTIFY_CHANGE_LAST_WRITE}, System::Threading::WaitForSingleObject};
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    unsafe { let watch = FindFirstChangeNotificationW(wide.as_ptr(), 0, FILE_NOTIFY_CHANGE_LAST_WRITE); if watch != INVALID_HANDLE_VALUE { let since = std::time::Instant::now(); while !stopped.load(Ordering::Acquire) && since.elapsed() < Duration::from_millis(SIGNAL_FALLBACK) { if WaitForSingleObject(watch,10) == 0 { break; } } FindCloseChangeNotification(watch); } else { wait_watch_fallback(stopped); } }
}
#[cfg(target_os = "linux")]
fn watch_signal(dir: &Path, stopped: &AtomicBool) {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let Ok(path) = CString::new(dir.as_os_str().as_bytes()) else { return; };
    unsafe { let fd = libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC); if fd < 0 { wait_watch_fallback(stopped); return; } if libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) >= 0 { let mut event = libc::pollfd { fd, events: libc::POLLIN, revents: 0 }; let since = std::time::Instant::now(); while !stopped.load(Ordering::Acquire) && since.elapsed() < Duration::from_millis(SIGNAL_FALLBACK) { if libc::poll(&mut event,1,10) != 0 { break; } } } else { wait_watch_fallback(stopped); } libc::close(fd); }
}
#[cfg(not(any(windows, target_os = "linux")))] fn watch_signal(_: &Path, stopped: &AtomicBool) { wait_watch_fallback(stopped); }

fn wait_watch_fallback(stopped:&AtomicBool) {let since=std::time::Instant::now();while !stopped.load(Ordering::Acquire)&&since.elapsed()<Duration::from_millis(SIGNAL_FALLBACK){std::thread::sleep(Duration::from_millis(10));}}
