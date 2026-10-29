// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in types.rs.
// This LSP-scoped owner replaces the unavailable generic launch broker. Each
// concurrent link gets its own child; only an idle identity may be reused.

use super::{
    config::{home_dir, resolve_command},
    custom,
    types::*,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        Arc, Mutex as StdMutex, OnceLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    sync::{Mutex, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

const CONNECT: &str = "ara/muxConnect";
const PING: &str = "ara/muxPing";
const RESTART: &str = "ara/muxRestartServer";
const LEASE: &str = "ara/muxLease";
const SERVER_LINGER: Duration = Duration::from_secs(300);
const MUX_IDLE: Duration = Duration::from_secs(900);
const LAST_LEASE_GRACE: Duration = Duration::from_secs(3);
pub const WORKER_ARG: &str = "__ara_worker_lsp_mux";

pub(super) struct WrappedCommand {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub wrapped: bool,
}
type WrapperState = Option<(Instant, Option<PathBuf>)>;
static WRAPPER_STATE: OnceLock<Mutex<WrapperState>> = OnceLock::new();
pub(super) async fn wrapped_command(
    command: &Path,
    args: &[String],
    cwd: &Path,
    cancel: Option<&CancellationToken>,
) -> io::Result<WrappedCommand> {
    let original =
        || WrappedCommand { command: command.to_owned(), args: args.to_owned(), env: BTreeMap::new(), wrapped: false };
    if command.file_name().and_then(|v| v.to_str()) != Some("rust-analyzer") {
        return Ok(original());
    }
    if std::env::var("ARA_DISABLE_LSPMUX").is_ok_and(|v| matches!(v.as_str(), "1" | "true" | "yes")) {
        return Ok(original());
    }
    let mut state = WRAPPER_STATE.get_or_init(|| Mutex::new(None)).lock().await;
    if state.as_ref().is_none_or(|(at, _)| at.elapsed() >= Duration::from_secs(300)) {
        let mut available = None;
        if let Some(binary) = resolve_command("lspmux", cwd) {
            let bound = cancel.map(CancellationToken::child_token).unwrap_or_default();
            match tokio::time::timeout(
                Duration::from_secs(1),
                custom::run(&binary, &["status".into()], cwd, Some(&bound)),
            )
            .await
            {
                Ok(Ok((_, _, true))) => available = Some(binary),
                Err(_) => bound.cancel(),
                _ => {}
            }
            check_cancel(cancel)?;
        }
        *state = Some((Instant::now(), available));
    }
    let Some(binary) = state.as_ref().and_then(|(_, binary)| binary.clone()) else {
        return Ok(original());
    };
    let mut env = BTreeMap::new();
    let wrapped_args = if command == Path::new("rust-analyzer") && args.is_empty() {
        Vec::new()
    } else {
        env.insert("LSPMUX_SERVER".into(), command.to_string_lossy().into());
        let mut result = vec!["client".into()];
        if !args.is_empty() {
            result.push("--".into());
            result.extend(args.iter().cloned());
        }
        result
    };
    Ok(WrappedCommand { command: binary, args: wrapped_args, env, wrapped: true })
}

fn digest(value: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, value).as_ref().iter().map(|byte| format!("{byte:02x}")).collect()
}
fn shared_state_root() -> PathBuf {
    // The host agent/config directory may vary by ARA_HOME or profile. Mux
    // ownership is machine state, so every such host uses the same base root.
    // Fixed dirs.ts rootSubdir("run/daemons", "state") supplies the layout;
    // its existing XDG app-root rule is retained without a profile suffix.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if let Some(root) = std::env::var_os("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(|path| path.join("ara"))
        .filter(|path| path.exists())
    {
        return absolute(&root);
    }
    absolute(&home_dir().join(".ara"))
}
fn runtime_dir(cwd: &Path, _agent_dir: &Path) -> PathBuf {
    shared_state_root().join("run/daemons").join(&digest(absolute(cwd).to_string_lossy().as_bytes())[..24])
}
fn endpoint(cwd: &Path, agent_dir: &Path) -> String {
    let runtime = absolute(&runtime_dir(cwd, agent_dir));
    if cfg!(windows) {
        format!("\\\\.\\pipe\\ara-lsp-mux-{}", &digest(runtime.to_string_lossy().as_bytes())[..24])
    } else {
        runtime.join("lsp-mux.sock").to_string_lossy().into()
    }
}
/// Independent process presence, registered at Host factory construction even
/// when LSP is disabled or remains lazy. Creating it starts no worker or server.
pub(super) struct ProjectPresence {
    file: StdMutex<Option<fs::File>>,
    path: PathBuf,
}
impl Drop for ProjectPresence {
    fn drop(&mut self) {
        let file = self.file.lock().unwrap().take();
        drop(file);
        let _ = fs::remove_file(&self.path);
    }
}
pub(super) fn register_project_presence(cwd: &Path, agent_dir: &Path) -> io::Result<Arc<ProjectPresence>> {
    let root = runtime_dir(cwd, agent_dir).join("presence");
    fs::create_dir_all(&root)?;
    let path = root.join(format!("{}-{}.lease", std::process::id(), uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = io::Error::last_os_error();
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(error);
        }
    }
    Ok(Arc::new(ProjectPresence { file: StdMutex::new(Some(file)), path }))
}
fn has_project_presence(runtime: &Path) -> bool {
    let entries = match fs::read_dir(runtime.join("presence")) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
        Err(error) => {
            eprintln!("ara: LSP project presence enumeration remains unknown: {error}");
            return true;
        }
    };
    let mut active = false;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!("ara: LSP project presence entry remains unknown: {error}");
                return true;
            }
        };
        let path = entry.path();
        if path.extension().and_then(|v| v.to_str()) != Some("lease") {
            continue;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(0);
        }
        match options.open(&path) {
            #[cfg(windows)]
            Err(error) if error.raw_os_error() == Some(32) => active = true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => active = true, // Unknown ownership cannot authorize worker teardown.
            Ok(file) => {
                #[cfg(unix)]
                {
                    use std::os::fd::AsRawFd;
                    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                        active = true;
                        continue;
                    }
                }
                drop(file);
                let _ = fs::remove_file(&path); // Names are unique and never reused.
            }
        }
    }
    active
}
fn describe_worker(runtime: &Path) -> Option<Value> {
    serde_json::from_slice(&fs::read(runtime.join("worker.json")).ok()?).ok()
}
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return io::Error::last_os_error().raw_os_error() == Some(5);
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut code = 0;
        (unsafe { GetExitCodeProcess(handle.as_raw_handle(), &mut code) }) != 0 && code == 259
    }
    #[cfg(unix)]
    {
        (unsafe { libc::kill(pid as i32, 0) }) == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}
async fn wait_ready(endpoint: &str, cancel: Option<&CancellationToken>) -> io::Result<bool> {
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        if probe(endpoint, cancel).await? {
            return Ok(true);
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(100))=>{},_=cancellation(cancel)=>return Err(cancelled())}
    }
    Ok(false)
}
async fn stop_instance(runtime: &Path, instance: &str, cancel: Option<&CancellationToken>) -> io::Result<()> {
    let instance = uuid::Uuid::parse_str(instance).map_err(|_| error("invalid LSP mux owner instance"))?.to_string();
    // Ask the observed instance to stop itself. Never terminate a PID merely
    // because this caller spawned it: another process may already own a lease.
    if describe_worker(runtime).and_then(|value| value["instance"].as_str().map(str::to_string)).as_deref()
        != Some(&instance)
    {
        return Ok(());
    }
    fs::write(runtime.join(format!("{instance}.stop")), b"stop")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        check_cancel(cancel)?;
        if describe_worker(runtime).and_then(|value| value["instance"].as_str().map(str::to_string)).as_deref()
            != Some(&instance)
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "LSP mux instance did not stop"));
        }
        tokio::select! {_=tokio::time::sleep(Duration::from_millis(100))=>{},_=cancellation(cancel)=>return Err(cancelled())}
    }
}
struct WorkerRecord {
    runtime: PathBuf,
    instance: String,
}
impl WorkerRecord {
    fn new(runtime: &Path, endpoint: &str, project: &str) -> io::Result<Self> {
        fs::create_dir_all(runtime)?;
        let instance = uuid::Uuid::new_v4().to_string();
        let path = runtime.join("worker.json");
        let temp = runtime.join(format!("worker-{instance}.json"));
        fs::write(&temp,serde_json::to_vec(&json!({"instance":instance,"pid":std::process::id(),"endpoint":endpoint,"project":absolute(Path::new(project)),"ready":true})).map_err(io::Error::other)?)?;
        // Listener ownership is already exclusive. A prior record can only be
        // stale; clean it while still holding that same instance's endpoint.
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::rename(temp, path)?;
        Ok(Self { runtime: runtime.to_owned(), instance })
    }
    fn stop_requested(&self) -> bool {
        self.runtime.join(format!("{}.stop", self.instance)).exists()
    }
}
impl Drop for WorkerRecord {
    fn drop(&mut self) {
        if describe_worker(&self.runtime).and_then(|value| value["instance"].as_str().map(str::to_string)).as_deref()
            == Some(&self.instance)
        {
            let _ = fs::remove_file(self.runtime.join("worker.json"));
        }
        let _ = fs::remove_file(self.runtime.join(format!("{}.stop", self.instance)));
    }
}
async fn dial(endpoint: &str, cancel: Option<&CancellationToken>) -> io::Result<(FramedReader, WriteStream)> {
    let operation = async {
        #[cfg(windows)]
        {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                match tokio::net::windows::named_pipe::ClientOptions::new().open(endpoint) {
                    Ok(pipe) => {
                        let (reader, writer) = tokio::io::split(pipe);
                        return Ok((FramedReader::new(Box::new(reader)), Box::new(writer) as WriteStream));
                    }
                    Err(error) if error.raw_os_error() == Some(231) && Instant::now() < deadline => {
                        tokio::time::sleep(Duration::from_millis(30)).await
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        #[cfg(unix)]
        {
            let socket = tokio::net::UnixStream::connect(endpoint).await?;
            let (reader, writer) = socket.into_split();
            Ok((FramedReader::new(Box::new(reader)), Box::new(writer) as WriteStream))
        }
    };
    tokio::select! { result = tokio::time::timeout(Duration::from_secs(3), operation) => result.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"LSP mux connect timed out"))?, _ = cancellation(cancel) => Err(cancelled()) }
}
async fn handshake(
    reader: &mut FramedReader,
    writer: &mut WriteStream,
    method: &str,
    params: Value,
    timeout_ms: u64,
    cancel: Option<&CancellationToken>,
) -> io::Result<Value> {
    let operation = async {
        write_frame(writer, &json!({"jsonrpc":"2.0","id":"handshake","method":method,"params":params})).await?;
        loop {
            let message = reader.next().await?.ok_or_else(|| error("LSP mux closed during handshake"))?;
            if message["id"] != "handshake" {
                continue;
            }
            if let Some(problem) = message.get("error") {
                return Err(error(format!(
                    "LSP mux handshake failed: {}",
                    problem["message"].as_str().unwrap_or("unknown")
                )));
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    };
    tokio::select! { result = tokio::time::timeout(Duration::from_millis(timeout_ms), operation) => result.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"LSP mux handshake timed out"))?, _ = cancellation(cancel) => Err(cancelled()) }
}
async fn probe(endpoint: &str, cancel: Option<&CancellationToken>) -> io::Result<bool> {
    let operation = async {
        let (mut reader, mut writer) = dial(endpoint, cancel).await?;
        let response = handshake(&mut reader, &mut writer, PING, Value::Null, 1500, cancel).await?;
        Ok::<_, io::Error>(response == "pong")
    };
    let result = tokio::time::timeout(Duration::from_millis(1500), operation).await;
    check_cancel(cancel)?;
    Ok(matches!(result, Ok(Ok(true))))
}
async fn ensure(cwd: &Path, agent_dir: &Path, cancel: Option<&CancellationToken>) -> io::Result<String> {
    let endpoint = endpoint(cwd, agent_dir);
    let runtime = runtime_dir(cwd, agent_dir);
    fs::create_dir_all(&runtime)?;
    for _ in 0..3 {
        check_cancel(cancel)?;
        if probe(&endpoint, cancel).await? {
            return Ok(endpoint);
        }
        if let Some(existing) =
            describe_worker(&runtime).filter(|value| value["pid"].as_u64().is_some_and(|pid| process_alive(pid as u32)))
        {
            if existing["ready"] != true && wait_ready(&endpoint, cancel).await? {
                return Ok(endpoint);
            }
            if probe(&endpoint, cancel).await? {
                return Ok(endpoint);
            }
            if let Some(instance) = existing["instance"].as_str() {
                stop_instance(&runtime, instance, cancel).await?;
                continue;
            }
        }
        // Each worker uses exclusive pipe instance / lifetime flock. Contending
        // starts exit without touching the winner or its listening endpoint.
        let mut worker = custom::command(&std::env::current_exe()?)
            .arg(WORKER_ARG)
            .env("ARA_LSP_MUX_SOCKET", &endpoint)
            .env("ARA_LSP_MUX_PROJECT_DIR", absolute(cwd))
            .env("ARA_LSP_MUX_RUNTIME_DIR", runtime_dir(cwd, agent_dir))
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(false)
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Err(error) = check_cancel(cancel) {
                tokio::spawn(async move {
                    let _ = worker.wait().await;
                });
                return Err(error);
            }
            let alive = match probe(&endpoint, cancel).await {
                Ok(alive) => alive,
                Err(error) => {
                    tokio::spawn(async move {
                        let _ = worker.wait().await;
                    });
                    return Err(error);
                }
            };
            if alive {
                tokio::spawn(async move {
                    let _ = worker.wait().await;
                });
                return Ok(endpoint);
            }
            if worker.try_wait()?.is_some() || Instant::now() >= deadline {
                break;
            }
            tokio::select! { _ = tokio::time::sleep(Duration::from_millis(100)) => {}, _ = cancellation(cancel) => {tokio::spawn(async move{let _=worker.wait().await;});return Err(cancelled());} }
        }
        if worker.try_wait()?.is_none() {
            tokio::spawn(async move {
                let _ = worker.wait().await;
            });
        }
        if probe(&endpoint, cancel).await? {
            return Ok(endpoint);
        }
    }
    Err(error("LSP mux could not become ready after three attempts"))
}

pub(super) struct Presence {
    stop: CancellationToken,
    task: StdMutex<Option<JoinHandle<()>>>,
}
impl Drop for Presence {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl Presence {
    pub async fn shutdown(&self) {
        self.stop.cancel();
        let task = self.task.lock().unwrap().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}
pub(super) struct SharedTransport {
    pub reader: FramedReader,
    pub writer: WriteStream,
    pub presence: Arc<Presence>,
}
pub(super) async fn connect_shared(
    command: &Path,
    args: &[String],
    cwd: &Path,
    agent_dir: &Path,
    env: &BTreeMap<String, String>,
    lifetime: &CancellationToken,
    cancel: Option<&CancellationToken>,
) -> io::Result<Option<SharedTransport>> {
    let operation = async {
        let endpoint = ensure(cwd, agent_dir, cancel).await?;
        let (mut lease_reader, mut lease_writer) = dial(&endpoint, cancel).await?;
        handshake(&mut lease_reader, &mut lease_writer, LEASE, json!({"pid":std::process::id()}), 10_000, cancel)
            .await?;
        let stop = lifetime.child_token();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            // The open pipe is the process presence lease. EOF or explicit
            // pool shutdown closes it, including when the parent process dies.
            tokio::select! { _ = task_stop.cancelled() => {}, _ = lease_reader.next() => {} }
            drop(lease_writer);
        });
        let presence = Arc::new(Presence { stop, task: StdMutex::new(Some(task)) });
        let (mut reader, mut writer) = dial(&endpoint, cancel).await?;
        handshake(
            &mut reader,
            &mut writer,
            CONNECT,
            json!({"command":command,"args":args,"cwd":absolute(cwd),"env":env}),
            10_000,
            cancel,
        )
        .await?;
        check_cancel(cancel)?;
        Ok::<_, io::Error>(SharedTransport { reader, writer, presence })
    };
    match operation.await {
        Ok(transport) => Ok(Some(transport)),
        Err(error) => {
            check_cancel(cancel)?;
            eprintln!("Shared LSP unavailable; using private server: {error}");
            Ok(None)
        }
    }
}

#[derive(Clone)]
struct Link {
    id: u64,
    tx: mpsc::UnboundedSender<Value>,
    initialized: bool,
}
struct Pending {
    link: u64,
    original: Value,
    dropped: bool,
    initialize: bool,
    internal: Option<oneshot::Sender<()>>,
}
struct ClientRequest {
    link: u64,
    original: Value,
    relay: bool,
}
struct MuxState {
    link: Option<Link>,
    open_uris: HashSet<String>,
    diagnostics: HashMap<String, Value>,
    registrations: Vec<Value>,
    progress: HashMap<String, Value>,
    pending: HashMap<String, Pending>,
    forwarded: HashMap<String, u64>,
    client_requests: HashMap<String, ClientRequest>,
    initialize: Option<Value>,
    initializing: bool,
    initialize_waiters: HashMap<u64, Value>,
    initialized_sent: bool,
    idle_since: Instant,
}
struct MuxChild {
    key: String,
    pid: u32,
    busy: AtomicBool,
    dead: AtomicBool,
    stopping: AtomicBool,
    next_id: AtomicU64,
    next_client_id: AtomicU64,
    state: StdMutex<MuxState>,
    output: Mutex<WriteStream>,
    stop: CancellationToken,
    tasks: StdMutex<Vec<JoinHandle<()>>>,
}
impl MuxChild {
    fn spawn(key: String, params: &Value, lifetime: &CancellationToken) -> io::Result<Arc<Self>> {
        let command = params["command"].as_str().ok_or_else(|| error("invalid mux command"))?;
        let cwd = params["cwd"].as_str().ok_or_else(|| error("invalid mux cwd"))?;
        let args = params["args"]
            .as_array()
            .ok_or_else(|| error("invalid mux args"))?
            .iter()
            .map(|arg| arg.as_str().map(str::to_string).ok_or_else(|| error("invalid mux argument")))
            .collect::<io::Result<Vec<_>>>()?;
        let env = params
            .get("env")
            .and_then(Value::as_object)
            .map(|v| {
                v.iter()
                    .map(|(key, value)| {
                        value
                            .as_str()
                            .map(|value| (key.clone(), value.to_string()))
                            .ok_or_else(|| error("invalid mux environment"))
                    })
                    .collect::<io::Result<BTreeMap<_, _>>>()
            })
            .transpose()?
            .unwrap_or_default();
        let mut child_command = custom::command(Path::new(command));
        child_command
            .args(args)
            .envs(env)
            .current_dir(absolute(Path::new(cwd)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = custom::spawn(child_command)?;
        let pid = child.id().ok_or_else(|| error("LSP child has no PID"))?;
        let stdout = child.stdout.take().ok_or_else(|| error("missing mux server stdout"))?;
        let stdin = child.stdin.take().ok_or_else(|| error("missing mux server stdin"))?;
        let stderr = child.stderr.take();
        let server = Arc::new(Self {
            key,
            pid,
            busy: AtomicBool::new(true),
            dead: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            next_client_id: AtomicU64::new(1),
            state: StdMutex::new(MuxState {
                link: None,
                open_uris: HashSet::new(),
                diagnostics: HashMap::new(),
                registrations: Vec::new(),
                progress: HashMap::new(),
                pending: HashMap::new(),
                forwarded: HashMap::new(),
                client_requests: HashMap::new(),
                initialize: None,
                initializing: false,
                initialize_waiters: HashMap::new(),
                initialized_sent: false,
                idle_since: Instant::now(),
            }),
            output: Mutex::new(Box::new(stdin)),
            stop: lifetime.child_token(),
            tasks: StdMutex::new(Vec::new()),
        });
        let owned = server.clone();
        server.tasks.lock().unwrap().push(tokio::spawn(async move {
            owned.read_server(FramedReader::new(Box::new(stdout))).await;
        }));
        if let Some(mut stderr) = stderr {
            let stop = server.stop.clone();
            server.tasks.lock().unwrap().push(tokio::spawn(async move { let mut buffer=[0;4096]; loop { tokio::select! { read=stderr.read(&mut buffer) => if !matches!(read,Ok(size) if size>0) {break;}, _=stop.cancelled()=>break } } }));
        }
        let owned = server.clone();
        server.tasks.lock().unwrap().push(tokio::spawn(async move {
            let status = tokio::select! {status=child.wait()=>status.map(|_|()),_=owned.stop.cancelled()=>Ok(())};
            let tree_confirmed = custom::terminate(&mut child).await.is_ok();
            owned.exited(status.is_ok() && tree_confirmed);
        }));
        Ok(server)
    }
    fn exited(&self, confirmed: bool) {
        self.dead.store(confirmed, Ordering::Release);
        self.stop.cancel();
        let mut state = self.state.lock().unwrap();
        state.pending.clear();
        state.client_requests.clear();
        state.initialize_waiters.clear();
        state.link = None;
    }
    async fn write(&self, message: &Value) -> io::Result<()> {
        let operation = async {
            let mut output = self.output.lock().await;
            write_frame(&mut output, message).await
        };
        let result = tokio::select! { result=tokio::time::timeout(Duration::from_secs(2),operation)=>result.map_err(|_|io::Error::new(io::ErrorKind::TimedOut,"LSP mux server write timed out"))?, _=self.stop.cancelled()=>Err(error("LSP mux server stopped")) };
        if result.is_err() {
            self.stop.cancel();
        }
        result
    }
    fn send(&self, message: Value) {
        if let Some(link) = &self.state.lock().unwrap().link {
            let _ = link.tx.send(message);
        }
    }
    async fn from_link(&self, link_id: u64, mut message: Value) -> io::Result<bool> {
        let method = message.get("method").and_then(Value::as_str).map(str::to_string);
        let request = message.get("id").is_some();
        let Some(method) = method else {
            let pending = self.state.lock().unwrap().client_requests.remove(&message["id"].to_string());
            if let Some(pending) = pending {
                if pending.relay && pending.link == link_id {
                    message["id"] = pending.original;
                    self.write(&message).await?;
                }
            }
            return Ok(true);
        };
        if method == RESTART && !request {
            self.stop.cancel();
            return Ok(false);
        }
        if method == "initialize" && request {
            let (cached, send) = {
                let mut state = self.state.lock().unwrap();
                if let Some(result) = &state.initialize {
                    (Some(result.clone()), false)
                } else {
                    state.initialize_waiters.insert(link_id, message["id"].clone());
                    let send = !state.initializing;
                    state.initializing = true;
                    (None, send)
                }
            };
            if let Some(result) = cached {
                self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":result}));
            } else if send {
                let id = self.next_id.fetch_add(1, Ordering::SeqCst);
                self.state.lock().unwrap().pending.insert(
                    id.to_string(),
                    Pending {
                        link: link_id,
                        original: message["id"].clone(),
                        dropped: false,
                        initialize: true,
                        internal: None,
                    },
                );
                message["id"] = Value::from(id);
                if !message["params"].is_object() {
                    message["params"] = json!({});
                }
                message["params"]["processId"] = Value::from(std::process::id());
                self.write(&message).await?;
            }
            return Ok(true);
        }
        if method == "initialized" && !request {
            let first = {
                let mut state = self.state.lock().unwrap();
                let first = !state.initialized_sent;
                state.initialized_sent = true;
                if let Some(link) = state.link.as_mut() {
                    link.initialized = true;
                }
                first
            };
            if first {
                self.write(&message).await?;
            } else {
                self.replay();
            }
            return Ok(true);
        }
        if method == "shutdown" && request {
            self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":null}));
            return Ok(true);
        }
        if method == "exit" && !request {
            return Ok(false);
        }
        if method == "textDocument/didOpen" && !request {
            if let Some(uri) = message.pointer("/params/textDocument/uri").and_then(Value::as_str) {
                self.state.lock().unwrap().open_uris.insert(uri.into());
            }
        }
        if method == "textDocument/didClose" && !request {
            if let Some(uri) = message.pointer("/params/textDocument/uri").and_then(Value::as_str) {
                self.state.lock().unwrap().open_uris.remove(uri);
            }
        }
        if method == "$/cancelRequest" && !request {
            let forwarded = self.state.lock().unwrap().forwarded.get(&message["params"]["id"].to_string()).copied();
            if let Some(id) = forwarded {
                message["params"]["id"] = Value::from(id);
                self.write(&message).await?;
            }
            return Ok(true);
        }
        if request {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let original = message["id"].clone();
            {
                let mut state = self.state.lock().unwrap();
                state.forwarded.insert(original.to_string(), id);
                state.pending.insert(
                    id.to_string(),
                    Pending { link: link_id, original, dropped: false, initialize: false, internal: None },
                );
            }
            message["id"] = Value::from(id);
        }
        self.write(&message).await?;
        Ok(true)
    }
    async fn read_server(self: Arc<Self>, mut reader: FramedReader) {
        loop {
            let message = tokio::select! { result=reader.next()=>result, _=self.stop.cancelled()=>return };
            let message = match message {
                Ok(Some(message)) => message,
                _ => break,
            };
            if let Err(error) = self.from_server(message).await {
                eprintln!("LSP mux server handler failed: {error}");
            }
        }
        self.stop.cancel();
    }
    async fn from_server(&self, mut message: Value) -> io::Result<()> {
        let method = message.get("method").and_then(Value::as_str).map(str::to_string);
        let Some(method) = method else {
            let pending = self.state.lock().unwrap().pending.remove(&message["id"].to_string());
            if let Some(mut pending) = pending {
                if let Some(done) = pending.internal.take() {
                    let _ = done.send(());
                    return Ok(());
                }
                if pending.initialize {
                    let waiters = {
                        let mut state = self.state.lock().unwrap();
                        state.initializing = false;
                        if message.get("error").is_none() {
                            state.initialize = Some(message["result"].clone());
                        }
                        std::mem::take(&mut state.initialize_waiters)
                    };
                    for (link, id) in waiters {
                        if self.state.lock().unwrap().link.as_ref().is_some_and(|current| current.id == link) {
                            let mut response = message.clone();
                            response["id"] = id;
                            self.send(response);
                        }
                    }
                    if message.get("error").is_some() {
                        self.stop.cancel();
                    }
                } else if !pending.dropped
                    && self.state.lock().unwrap().link.as_ref().is_some_and(|link| link.id == pending.link)
                {
                    self.state.lock().unwrap().forwarded.remove(&pending.original.to_string());
                    message["id"] = pending.original;
                    self.send(message);
                }
            }
            return Ok(());
        };
        if message.get("id").is_some() {
            let no_relay = matches!(
                method.as_str(),
                "client/registerCapability" | "client/unregisterCapability" | "window/workDoneProgress/create"
            );
            if no_relay {
                {
                    let mut state = self.state.lock().unwrap();
                    if method == "client/registerCapability" {
                        if let Some(items) = message["params"]["registrations"].as_array() {
                            state.registrations.push(json!({"registrations":items.iter().filter(|item|item["id"].is_string()).cloned().collect::<Vec<_>>()}));
                        }
                    }
                    if method == "client/unregisterCapability" {
                        if let Some(items) = message["params"]
                            .get("unregisterations")
                            .or_else(|| message["params"].get("unregistrations"))
                            .and_then(Value::as_array)
                        {
                            let ids = items.iter().filter_map(|item| item["id"].as_str()).collect::<HashSet<_>>();
                            for batch in &mut state.registrations {
                                if let Some(items) = batch["registrations"].as_array_mut() {
                                    items.retain(|item| !item["id"].as_str().is_some_and(|id| ids.contains(id)));
                                }
                            }
                        }
                    }
                }
                self.write(&json!({"jsonrpc":"2.0","id":message["id"],"result":null})).await?;
            }
            let link = self.state.lock().unwrap().link.clone();
            if let Some(link) = link.filter(|link| !no_relay || link.initialized) {
                let id = format!("mux:{}", self.next_client_id.fetch_add(1, Ordering::SeqCst));
                self.state.lock().unwrap().client_requests.insert(
                    Value::String(id.clone()).to_string(),
                    ClientRequest { link: link.id, original: message["id"].clone(), relay: !no_relay },
                );
                message["id"] = Value::String(id);
                let _ = link.tx.send(message);
            } else if !no_relay {
                self.write(
                    &json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"no client attached"}}),
                )
                .await?;
            }
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if method == "textDocument/publishDiagnostics" {
            if let Some(uri) = message["params"]["uri"].as_str() {
                state.diagnostics.insert(uri.into(), message["params"].clone());
            }
        }
        if method == "$/progress" {
            let token = message["params"]["token"].to_string();
            match message.pointer("/params/value/kind").and_then(Value::as_str) {
                Some("begin") => {
                    state.progress.insert(token, message["params"].clone());
                }
                Some("end") => {
                    state.progress.remove(&token);
                }
                _ => {}
            }
        }
        if let Some(link) = state.link.as_ref().filter(|link| link.initialized) {
            let _ = link.tx.send(message);
        }
        Ok(())
    }
    fn replay(&self) {
        let mut state = self.state.lock().unwrap();
        let Some(link) = state.link.clone() else {
            return;
        };
        for batch in state.registrations.clone() {
            if batch["registrations"].as_array().is_none_or(Vec::is_empty) {
                continue;
            }
            let id = format!("mux:{}", self.next_client_id.fetch_add(1, Ordering::SeqCst));
            state.client_requests.insert(
                Value::String(id.clone()).to_string(),
                ClientRequest { link: link.id, original: Value::Null, relay: false },
            );
            let _ = link.tx.send(json!({"jsonrpc":"2.0","id":id,"method":"client/registerCapability","params":batch}));
        }
        for params in state.diagnostics.values() {
            let _ = link.tx.send(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":params}));
        }
        for params in state.progress.values() {
            let _ = link.tx.send(json!({"jsonrpc":"2.0","method":"$/progress","params":params}));
        }
    }
    async fn close_link(&self, id: u64) {
        let (uris, pending, client_requests) = {
            let mut state = self.state.lock().unwrap();
            if state.link.as_ref().is_none_or(|link| link.id != id) {
                return;
            }
            let uris = state.open_uris.drain().collect::<Vec<_>>();
            let pending = state
                .pending
                .iter_mut()
                .filter_map(|(request, pending)| {
                    if pending.link == id && !pending.initialize {
                        pending.dropped = true;
                        Some(request.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            let client_requests = state
                .client_requests
                .drain()
                .filter_map(|(_, pending)| pending.relay.then_some(pending.original))
                .collect::<Vec<_>>();
            (uris, pending, client_requests)
        };
        for uri in uris {
            let _ = self
                .write(&json!({"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":uri}}}))
                .await;
        }
        for request in pending {
            let id = serde_json::from_str::<Value>(&request).unwrap_or(Value::String(request));
            let _ = self.write(&json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}})).await;
        }
        for request in client_requests {
            let _ = self
                .write(&json!({"jsonrpc":"2.0","id":request,"error":{"code":-32601,"message":"client disconnected"}}))
                .await;
        }
        {
            let mut state = self.state.lock().unwrap();
            state.initialize_waiters.remove(&id);
            state.forwarded.clear();
            state.link = None;
            state.idle_since = Instant::now();
        }
        self.busy.store(false, Ordering::Release);
    }
    async fn shutdown(&self) -> io::Result<()> {
        if self.stopping.swap(true, Ordering::AcqRel) {
            return if self.dead.load(Ordering::Acquire) {
                Ok(())
            } else {
                Err(error("LSP mux child termination remains unknown"))
            };
        }
        if !self.dead.load(Ordering::Acquire) {
            let id = self.next_id.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = oneshot::channel();
            self.state.lock().unwrap().pending.insert(
                id.to_string(),
                Pending { link: 0, original: Value::Null, dropped: false, initialize: false, internal: Some(tx) },
            );
            if self.write(&json!({"jsonrpc":"2.0","id":id,"method":"shutdown","params":null})).await.is_ok() {
                let _ = tokio::time::timeout(Duration::from_secs(2), rx).await;
                let _ = self.write(&json!({"jsonrpc":"2.0","method":"exit"})).await;
            }
        }
        self.stop.cancel();
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(7), &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
        if !self.dead.load(Ordering::Acquire) {
            return Err(error("LSP mux child tree exit was not confirmed"));
        }
        Ok(())
    }
}
struct Registry {
    children: StdMutex<Vec<Arc<MuxChild>>>,
    stop: CancellationToken,
    next_link: AtomicU64,
    sessions: AtomicUsize,
    leases: AtomicUsize,
    had_lease: AtomicBool,
    empty_since: StdMutex<Instant>,
    last_lease_since: StdMutex<Option<Instant>>,
}
impl Registry {
    fn child(&self, key: String, params: &Value) -> io::Result<(Arc<MuxChild>, bool)> {
        let mut children = self.children.lock().unwrap();
        if let Some(server) = children.iter().find(|server| {
            server.key == key
                && !server.dead.load(Ordering::Acquire)
                && !server.stopping.load(Ordering::Acquire)
                && !server.stop.is_cancelled()
                && server.busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed).is_ok()
        }) {
            return Ok((server.clone(), false));
        }
        let server = MuxChild::spawn(key, params, &self.stop)?;
        children.push(server.clone());
        Ok((server, true))
    }
    async fn connection(self: Arc<Self>, reader: ReadStream, writer: WriteStream) {
        self.sessions.fetch_add(1, Ordering::AcqRel);
        let link = self.next_link.fetch_add(1, Ordering::SeqCst);
        let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
        let writer_stop = CancellationToken::new();
        let stop = writer_stop.clone();
        let mut writer = writer;
        let mut writer_task = tokio::spawn(async move {
            loop {
                let message = tokio::select! {message=rx.recv()=>match message{Some(message)=>message,None=>break},_=stop.cancelled()=>break};
                if !matches!(
                    tokio::time::timeout(Duration::from_secs(2), write_frame(&mut writer, &message)).await,
                    Ok(Ok(()))
                ) {
                    break;
                }
            }
        });
        let mut reader = FramedReader::new(reader);
        let mut bound: Option<Arc<MuxChild>> = None;
        let mut leased = false;
        loop {
            let message = tokio::select! {message=reader.next()=>message,_=self.stop.cancelled()=>break,_=&mut writer_task=>break,_=async {match &bound{Some(server)=>server.stop.cancelled().await,None=>std::future::pending().await}}=>break};
            let message = match message {
                Ok(Some(message)) => message,
                _ => break,
            };
            let method = message["method"].as_str().unwrap_or_default();
            let id = message.get("id").cloned();
            if let Some(server) = &bound {
                if !server.from_link(link, message).await.unwrap_or(false) {
                    break;
                }
                continue;
            }
            if method == PING {
                if let Some(id) = id {
                    let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"result":"pong"}));
                }
                continue;
            }
            if method == LEASE && id.is_some() && !leased {
                leased = true;
                self.leases.fetch_add(1, Ordering::AcqRel);
                self.had_lease.store(true, Ordering::Release);
                *self.last_lease_since.lock().unwrap() = None;
                let _ = tx.send(json!({"jsonrpc":"2.0","id":id,"result":"leased"}));
                continue;
            }
            if method == CONNECT && id.is_some() && !leased {
                let result = (|| {
                    let params = &message["params"];
                    let command = params["command"].as_str().ok_or_else(|| error("invalid mux command"))?;
                    let args = params["args"].as_array().ok_or_else(|| error("invalid mux args"))?;
                    if !args.iter().all(Value::is_string) {
                        return Err(error("invalid mux argument"));
                    }
                    let cwd = params["cwd"].as_str().ok_or_else(|| error("invalid mux cwd"))?;
                    let env = params
                        .get("env")
                        .and_then(Value::as_object)
                        .map(|v| {
                            v.iter()
                                .map(|(key, value)| {
                                    value
                                        .as_str()
                                        .map(|value| (key.clone(), value.to_string()))
                                        .ok_or_else(|| error("invalid mux environment"))
                                })
                                .collect::<io::Result<BTreeMap<_, _>>>()
                        })
                        .transpose()?
                        .unwrap_or_default();
                    let identity = json!([command, args, absolute(Path::new(cwd)), env.iter().collect::<Vec<_>>()]);
                    let key = format!("sha256:{}", digest(identity.to_string().as_bytes()));
                    self.child(key, params)
                })();
                match result {
                    Ok((server, spawned)) => {
                        let _=tx.send(json!({"jsonrpc":"2.0","id":id,"result":{"key":server.key,"spawned":spawned,"pid":server.pid}}));
                        server.state.lock().unwrap().link = Some(Link { id: link, tx: tx.clone(), initialized: false });
                        bound = Some(server);
                    }
                    Err(error) => {
                        let _ = tx
                            .send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":error.to_string()}}));
                    }
                }
            } else if let Some(id) = id {
                let _ = tx.send(
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32002,"message":"muxConnect must be first"}}),
                );
            }
        }
        if let Some(server) = bound {
            server.close_link(link).await;
        }
        if leased && self.leases.fetch_sub(1, Ordering::AcqRel) == 1 {
            *self.last_lease_since.lock().unwrap() = Some(Instant::now());
        }
        if self.sessions.fetch_sub(1, Ordering::AcqRel) == 1 {
            *self.empty_since.lock().unwrap() = Instant::now();
        }
        writer_stop.cancel();
        if !writer_task.is_finished() {
            let _ = writer_task.await;
        }
    }
    async fn shutdown(&self) -> io::Result<()> {
        let children = self.children.lock().unwrap().clone();
        let results = futures::future::join_all(children.iter().map(|server| server.shutdown())).await;
        self.stop.cancel();
        results.into_iter().collect::<io::Result<Vec<_>>>().map(|_| ())
    }
}

struct Listener {
    #[cfg(windows)]
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    #[cfg(unix)]
    socket: tokio::net::UnixListener,
    #[cfg(unix)]
    _lock: fs::File,
    endpoint: String,
}
impl Listener {
    fn bind(endpoint: String, _runtime: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        {
            let pipe =
                tokio::net::windows::named_pipe::ServerOptions::new().first_pipe_instance(true).create(&endpoint)?;
            Ok(Self { pipe, endpoint })
        }
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            fs::create_dir_all(_runtime)?;
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(_runtime.join("owner.lock"))?;
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(io::Error::last_os_error());
            }
            // Holding the lifetime lock proves no other adapter instance owns
            // this endpoint; a losing contender cannot unlink a winner.
            match fs::remove_file(&endpoint) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let socket = tokio::net::UnixListener::bind(&endpoint)?;
            Ok(Self { socket, _lock: lock, endpoint })
        }
    }
    async fn accept(&mut self) -> io::Result<(ReadStream, WriteStream)> {
        #[cfg(windows)]
        {
            self.pipe.connect().await?;
            let next = tokio::net::windows::named_pipe::ServerOptions::new().create(&self.endpoint)?;
            let pipe = std::mem::replace(&mut self.pipe, next);
            let (reader, writer) = tokio::io::split(pipe);
            Ok((Box::new(reader), Box::new(writer)))
        }
        #[cfg(unix)]
        {
            let (socket, _) = self.socket.accept().await?;
            let (reader, writer) = socket.into_split();
            Ok((Box::new(reader), Box::new(writer)))
        }
    }
}
#[cfg(unix)]
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.endpoint);
    }
}

/// Selected before normal CLI argument parsing by the same executable.
pub async fn run_worker_from_environment() -> io::Result<()> {
    let endpoint = std::env::var("ARA_LSP_MUX_SOCKET").map_err(|_| error("LSP mux environment is incomplete"))?;
    let project = std::env::var("ARA_LSP_MUX_PROJECT_DIR").map_err(|_| error("LSP mux environment is incomplete"))?;
    let runtime = absolute(
        &std::env::var_os("ARA_LSP_MUX_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_dir(Path::new(&project), Path::new(""))),
    );
    let mut listener = Listener::bind(endpoint.clone(), &runtime)?;
    let owner_record = WorkerRecord::new(&runtime, &endpoint, &project)?;
    let registry = Arc::new(Registry {
        children: StdMutex::new(Vec::new()),
        stop: CancellationToken::new(),
        next_link: AtomicU64::new(1),
        sessions: AtomicUsize::new(0),
        leases: AtomicUsize::new(0),
        had_lease: AtomicBool::new(false),
        empty_since: StdMutex::new(Instant::now()),
        last_lease_since: StdMutex::new(None),
    });
    println!("ara lsp mux listening on {endpoint}");
    let mut links = tokio::task::JoinSet::new();
    let mut timer = tokio::time::interval(Duration::from_millis(100));
    let mut listener_error = None;
    loop {
        tokio::select! {
            connection=listener.accept()=>match connection{Ok((reader,writer))=>{let registry=registry.clone();links.spawn(async move{registry.connection(reader,writer).await;});},Err(error)=>{listener_error=Some(error);break;}},
            _=timer.tick()=>{
                if owner_record.stop_requested(){break;}
                let presence=has_project_presence(&runtime);
                if presence||registry.leases.load(Ordering::Acquire)>0 {registry.had_lease.store(true,Ordering::Release);*registry.last_lease_since.lock().unwrap()=None;}
                else if registry.had_lease.load(Ordering::Acquire) {let mut empty=registry.last_lease_since.lock().unwrap();empty.get_or_insert_with(Instant::now);}
                let last_lease=registry.had_lease.load(Ordering::Acquire)&&!presence&&registry.leases.load(Ordering::Acquire)==0&&registry.last_lease_since.lock().unwrap().is_some_and(|at|at.elapsed()>=LAST_LEASE_GRACE);
                let idle=registry.sessions.load(Ordering::Acquire)==0&&registry.empty_since.lock().unwrap().elapsed()>=MUX_IDLE;
                if last_lease||idle{break;}
                let expired={let children=registry.children.lock().unwrap();children.iter().filter(|server|!server.busy.load(Ordering::Acquire)&&!server.stopping.load(Ordering::Acquire)&&server.state.lock().unwrap().idle_since.elapsed()>=SERVER_LINGER).cloned().collect::<Vec<_>>()};
                for server in expired{if let Err(error)=server.shutdown().await{eprintln!("LSP mux idle child shutdown failed: {error}");}}
                registry.children.lock().unwrap().retain(|server|!server.dead.load(Ordering::Acquire));
            },
            _=tokio::signal::ctrl_c()=>break,
            Some(_)=links.join_next(),if !links.is_empty()=>{},
        }
    }
    let shutdown = registry.shutdown().await;
    while links.join_next().await.is_some() {}
    shutdown?;
    match listener_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
