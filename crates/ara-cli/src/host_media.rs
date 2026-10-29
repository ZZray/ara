//! Reference Host media primitives for fixed OMP 596f2da utils/video.ts.
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
//! MIT; see THIRD_PARTY_NOTICES.md for the full upstream notice.
//! Core owns argv/policy; Host owns granted READ publication, paths and effects.
//! Port construction is not a grant: the actual Host must bind it only behind
//! its existing admitted READ capability. No test fixture creates that grant.
use ara_tools::read_video_port::{
    MediaBinary, MediaCommand, MediaProcessOutput, ReadVideoHostPort, ReadVideoWorkspace,
    VideoError,
};
use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, process::Command, sync::watch};
use tokio_util::sync::CancellationToken;

const WORKSPACE_SETTLEMENT_WINDOW: Duration = Duration::from_secs(5);
const HOST_SETTLEMENT_WINDOW: Duration = Duration::from_secs(10);

// Unknown ownership outcomes are retryable observations, not terminal cached
// cleanup failures. Keep this distinction typed rather than parsing messages.
enum CleanupFailure {
    Known(VideoError),
    Unknown(VideoError),
}
impl CleanupFailure {
    fn error(self) -> VideoError {
        match self {
            Self::Known(error) | Self::Unknown(error) => error,
        }
    }
}

type MediaResult = Result<MediaProcessOutput, VideoError>;
type Reply = watch::Receiver<Option<MediaResult>>;
#[derive(Clone)]
struct Operation {
    cancel: CancellationToken,
    done: watch::Receiver<bool>,
    workspaces: Vec<u64>,
}
#[derive(Default)]
struct Admission {
    closed: bool,
    serial: u64,
    operations: HashMap<u64, Operation>,
    workspaces: HashMap<u64, Arc<WorkspaceState>>,
    workspace_closed: HashSet<u64>,
}
struct State {
    cwd: PathBuf,
    temp: PathBuf,
    lifetime: CancellationToken,
    admission: Mutex<Admission>,
    discoveries: Mutex<HashMap<(&'static str, Option<OsString>), Option<PathBuf>>>,
    issued: Mutex<HashSet<PathBuf>>,
    cleanup_errors: Mutex<Vec<VideoError>>,
    unsafe_workspaces: Mutex<HashSet<u64>>,
}
/// One owner per actual reference Host process, retained across tool snapshots.
/// The companion Core ReadVideoRuntime must also be retained by the Host so
/// fixed version-probe cache (including concurrent cold probes) is not reset.
pub struct HostMediaPort {
    state: Arc<State>,
    close: tokio::sync::Mutex<Option<Result<(), VideoError>>>,
}
impl HostMediaPort {
    pub fn new(cwd: &Path, temp: &Path) -> Result<Arc<Self>, VideoError> {
        if !temp.is_absolute() {
            return Err(VideoError::text(
                "READ media workspace root must be an explicit absolute path",
            ));
        }
        #[cfg(windows)]
        if !temp.to_string_lossy().to_ascii_lowercase().starts_with("d:") {
            return Err(VideoError::text(
                "READ media workspace root must be an explicit absolute D path",
            ));
        }
        std::fs::create_dir_all(temp).map_err(os_error)?;
        let temp = std::fs::canonicalize(temp).map_err(os_error)?;
        #[cfg(windows)]
        if !matches!(temp.components().next(), Some(Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(b'D' | b'd') | std::path::Prefix::VerbatimDisk(b'D' | b'd')))
        {
            return Err(VideoError::text(
                "READ media resolved workspace root must remain on D",
            ));
        }
        let cwd = std::fs::canonicalize(cwd).map_err(os_error)?;
        Ok(Arc::new(Self {
            state: Arc::new(State {
                cwd,
                temp,
                lifetime: CancellationToken::new(),
                admission: Default::default(),
                discoveries: Default::default(),
                issued: Default::default(),
                cleanup_errors: Default::default(),
                unsafe_workspaces: Default::default(),
            }),
            close: Default::default(),
        }))
    }
    /// Close admission before cancelling and joining every registered process,
    /// including operations whose caller future disappeared. Then delete owned
    /// workspaces; a failed process settlement never permits their deletion.
    /// An unknown settlement returns within the shared wait window while the
    /// registered owners remain alive. A later close may observe settlement.
    pub async fn close(&self) -> Result<(), VideoError> {
        let deadline = tokio::time::Instant::now() + HOST_SETTLEMENT_WINDOW;
        let mut close = tokio::time::timeout_at(deadline, self.close.lock())
            .await
            .map_err(|_| {
                VideoError::unknown("READ media Host close wait expired; settlement unknown")
            })?;
        if let Some(result) = &*close {
            return result.clone();
        }
        let operations = {
            let mut admission = self.state.admission.lock().unwrap();
            admission.closed = true;
            admission.operations.values().cloned().collect::<Vec<_>>()
        };
        self.state.lifetime.cancel();
        for operation in &operations {
            operation.cancel.cancel();
        }
        let mut errors = Vec::new();
        for operation in operations {
            // Do not cache UNKNOWN or visit workspace deletion while an owner
            // is unsettled. Neither the actor nor its child is aborted here.
            settled(operation.done, deadline).await?;
        }
        let workspaces = self
            .state
            .admission
            .lock()
            .unwrap()
            .workspaces
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for workspace in workspaces {
            match cleanup_workspace_until(self.state.clone(), workspace, deadline).await {
                Ok(()) => {}
                Err(CleanupFailure::Unknown(error)) => return Err(error),
                Err(CleanupFailure::Known(error)) => errors.push(
                    error
                        .message
                        .to_utf8()
                        .unwrap_or_else(|_| "native UTF16 cleanup error".into()),
                ),
            }
        }
        // A final owner-disappearance error is still UNKNOWN even when the
        // command had no workspace (for example a version probe). Preserve its
        // class; do not reduce it to an ordinary text aggregate or cache it.
        let deferred_errors = self.state.cleanup_errors.lock().unwrap().clone();
        if let Some(error) = deferred_errors.iter().find(|error| error.is_unknown()) {
            return Err(error.clone());
        }
        errors.extend(deferred_errors.into_iter().map(|error| {
            error
                .message
                .to_utf8()
                .unwrap_or_else(|_| "native UTF16 cleanup error".into())
        }));
        let result = if errors.is_empty() {
            Ok(())
        } else {
            Err(VideoError::text(format!(
                "READ media settlement failed: {}",
                errors.join("; ")
            )))
        };
        *close = Some(result.clone());
        result
    }
    pub fn cleanup_errors(&self) -> Vec<String> {
        self.state
            .cleanup_errors
            .lock()
            .unwrap()
            .iter()
            .map(|error| {
                error
                    .message
                    .to_utf8()
                    .unwrap_or_else(|_| "native UTF16 cleanup error".into())
            })
            .collect()
    }
}
impl Drop for HostMediaPort {
    fn drop(&mut self) {
        self.state.lifetime.cancel();
    }
}

struct CallerExit(CancellationToken);
impl Drop for CallerExit {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[async_trait]
impl ReadVideoHostPort for HostMediaPort {
    async fn find_binary(&self, binary: MediaBinary) -> Result<Option<PathBuf>, VideoError> {
        // Fixed $which defaults Cached and includes the live PATH snapshot.
        // It executes before the Core probe's pre-aborted check.
        let path = std::env::var_os("PATH");
        let key = (binary.name(), path.clone());
        if let Some(found) = self.state.discoveries.lock().unwrap().get(&key).cloned() {
            return Ok(found);
        }
        let found = find_on_path(binary.name(), path.as_deref(), &self.state.cwd)?;
        if let Some(found) = &found {
            self.state.issued.lock().unwrap().insert(found.clone());
        }
        self.state
            .discoveries
            .lock()
            .unwrap()
            .insert(key, found.clone());
        Ok(found)
    }
    async fn execute(
        &self,
        command: MediaCommand,
        cancel: Option<CancellationToken>,
    ) -> MediaResult {
        if !self
            .state
            .issued
            .lock()
            .unwrap()
            .contains(&command.executable)
        {
            return Err(VideoError::text(
                "READ media executable was not issued by this Host discovery owner",
            ));
        }
        let operation_cancel = CancellationToken::new();
        let caller_exit = CallerExit(operation_cancel.clone());
        let (sender, reply) = watch::channel(None);
        let (done_sender, done) = watch::channel(false);
        let serial = {
            let mut admission = self.state.admission.lock().unwrap();
            if admission.closed {
                return Err(VideoError::aborted("READ media Host admission is closed"));
            }
            let workspaces = admission
                .workspaces
                .iter()
                .filter(|(_, workspace)| {
                    command.args.iter().any(|arg| {
                        Path::new(arg).is_absolute() && Path::new(arg).starts_with(&workspace.path)
                    })
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            if workspaces
                .iter()
                .any(|id| admission.workspace_closed.contains(id))
            {
                return Err(VideoError::aborted(
                    "READ media workspace admission is closed",
                ));
            }
            admission.serial += 1;
            let serial = admission.serial;
            admission.operations.insert(
                serial,
                Operation {
                    cancel: operation_cancel.clone(),
                    done,
                    workspaces,
                },
            );
            let owned_workspaces = admission.operations[&serial].workspaces.clone();
            let state = self.state.clone();
            // Registered ownership precedes spawn under the same close gate.
            // The task retains process/pipe owners independently of the caller.
            tokio::spawn(async move {
                let result = run_process(&state, command, cancel, operation_cancel, &sender).await;
                if let Err(error) = &result {
                    if error.is_unknown() {
                        state.cleanup_errors.lock().unwrap().push(error.clone());
                        state
                            .unsafe_workspaces
                            .lock()
                            .unwrap()
                            .extend(owned_workspaces);
                    }
                }
                let _ = sender.send(Some(result));
                let _ = done_sender.send(true);
                state.admission.lock().unwrap().operations.remove(&serial);
            });
            serial
        };
        let _ = serial;
        let result = observe(reply).await;
        drop(caller_exit);
        result
    }
    async fn create_workspace(
        &self,
        prefix: &str,
    ) -> Result<Arc<dyn ReadVideoWorkspace>, VideoError> {
        if prefix.is_empty()
            || Path::new(prefix).components().count() != 1
            || !matches!(
                Path::new(prefix).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(VideoError::text(
                "READ media workspace prefix must be a single relative name",
            ));
        }
        let mut admission = self.state.admission.lock().unwrap();
        if admission.closed {
            return Err(VideoError::aborted("READ media Host admission is closed"));
        }
        let path = self
            .state
            .temp
            .join(format!("{prefix}{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&path).map_err(os_error)?;
        let path = std::fs::canonicalize(path).map_err(os_error)?;
        admission.serial += 1;
        let serial = admission.serial;
        let workspace = Arc::new(WorkspaceState {
            serial,
            path,
            cleanup: Default::default(),
        });
        admission.workspaces.insert(serial, workspace.clone());
        Ok(Arc::new(NativeWorkspace {
            state: self.state.clone(),
            workspace,
        }))
    }
}

async fn observe(mut reply: Reply) -> MediaResult {
    loop {
        if let Some(result) = reply.borrow().clone() {
            return result;
        }
        reply.changed().await.map_err(|_| {
            VideoError::unknown("READ media process receipt missing; settlement unknown")
        })?;
    }
}
async fn settled(
    mut done: watch::Receiver<bool>,
    deadline: tokio::time::Instant,
) -> Result<(), VideoError> {
    tokio::time::timeout_at(deadline, async {
        loop {
            if *done.borrow() {
                return Ok(());
            }
            done.changed().await.map_err(|_| {
                VideoError::unknown(
                    "READ media retained process owner disappeared; settlement unknown",
                )
            })?;
        }
    })
    .await
    .map_err(|_| {
        VideoError::unknown("READ media retained process wait expired; settlement unknown")
    })?
}
async fn run_process(
    state: &State,
    command: MediaCommand,
    run_cancel: Option<CancellationToken>,
    caller_exit: CancellationToken,
    receipt: &watch::Sender<Option<MediaResult>>,
) -> MediaResult {
    let cancelled = || {
        state.lifetime.is_cancelled()
            || caller_exit.is_cancelled()
            || run_cancel
                .as_ref()
                .is_some_and(CancellationToken::is_cancelled)
    };
    if cancelled() {
        return Err(VideoError::aborted("Aborted"));
    }
    let mut builder = Command::new(command.executable);
    builder
        .args(command.args)
        .current_dir(&state.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    // Build a direct argv command ourselves: the LSP command() shell facade
    // is deliberately not exported or called by media.
    let mut child: crate::host_lsp::OwnedProcess =
        crate::host_lsp::spawn_owned_process(builder).map_err(os_error)?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| VideoError::unknown("READ media stdout missing; settlement unknown"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| VideoError::unknown("READ media stderr missing; settlement unknown"))?;
    let stdout_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut was_cancelled = false;
    let exit = tokio::select! {
        biased;
        _ = state.lifetime.cancelled() => { was_cancelled=true; None },
        _ = caller_exit.cancelled() => { was_cancelled=true; None },
        _ = async { if let Some(cancel)=&run_cancel { cancel.cancelled().await; } else { std::future::pending::<()>().await; } } => { was_cancelled=true; None },
        exit = child.wait() => Some(exit),
    };
    // Even a normal exit keeps its retained Job/group until all descendants
    // and both pipes settle. This uses the existing contained-process owner.
    while let Err(error) = crate::host_lsp::terminate_owned_process(&mut child).await {
        // A bounded helper timeout is an UNKNOWN caller receipt, not permission
        // to detach the process or delete outputs. The registered task keeps
        // this child/Job/group and retries only idempotent termination, never
        // command execution, until actual settlement is observed.
        let _ = receipt.send(Some(Err(VideoError::unknown(format!(
            "READ media process settlement unknown: {error}"
        )))));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut stdout_task = stdout_task;
    let mut stderr_task = stderr_task;
    let reads = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(&mut stdout_task, &mut stderr_task)
    })
    .await;
    let (stdout, stderr) = match reads {
        Ok(reads) => reads,
        Err(_) => {
            let _ = receipt.send(Some(Err(VideoError::unknown(
                "READ media pipe settlement unknown",
            ))));
            tokio::join!(stdout_task, stderr_task)
        }
    };
    let stdout = stdout
        .map_err(|error| VideoError::text(format!("READ media stdout task: {error}")))?
        .map_err(os_error)?;
    let stderr = stderr
        .map_err(|error| VideoError::text(format!("READ media stderr task: {error}")))?
        .map_err(os_error)?;
    if was_cancelled || cancelled() {
        return Err(VideoError::aborted("Aborted"));
    }
    let exit = exit
        .ok_or_else(|| VideoError::unknown("READ media child exit missing; settlement unknown"))?
        .map_err(os_error)?;
    #[cfg(unix)]
    let exit_code = {
        use std::os::unix::process::ExitStatusExt;
        exit.code()
            .unwrap_or_else(|| 128 + exit.signal().unwrap_or(0))
    };
    #[cfg(not(unix))]
    let exit_code = exit.code().unwrap_or(-1);
    Ok(MediaProcessOutput {
        stdout,
        stderr,
        exit_code,
    })
}

struct WorkspaceState {
    serial: u64,
    path: PathBuf,
    cleanup: tokio::sync::Mutex<Option<Result<(), VideoError>>>,
}
struct NativeWorkspace {
    state: Arc<State>,
    workspace: Arc<WorkspaceState>,
}
impl NativeWorkspace {
    fn named(&self, name: &str) -> Result<PathBuf, VideoError> {
        if Path::new(name).components().count() != 1
            || !matches!(
                Path::new(name).components().next(),
                Some(Component::Normal(_))
            )
        {
            return Err(VideoError::text(
                "READ media workspace name must be a single relative file",
            ));
        }
        Ok(self.workspace.path.join(name))
    }
}
#[async_trait]
impl ReadVideoWorkspace for NativeWorkspace {
    fn path(&self) -> &Path {
        &self.workspace.path
    }
    async fn read_named(&self, name: &str) -> Result<Option<Vec<u8>>, VideoError> {
        match tokio::fs::read(self.named(name)?).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(os_error(error)),
        }
    }
    async fn nonempty_named(&self, name: &str) -> Result<bool, VideoError> {
        match tokio::fs::metadata(self.named(name)?).await {
            Ok(metadata) => Ok(metadata.len() > 0),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(os_error(error)),
        }
    }
    async fn cleanup(&self) -> Result<(), VideoError> {
        cleanup_workspace(self.state.clone(), self.workspace.clone()).await
    }
}
impl Drop for NativeWorkspace {
    fn drop(&mut self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let state = self.state.clone();
            let workspace = self.workspace.clone();
            runtime.spawn(async move {
                let deadline = tokio::time::Instant::now() + WORKSPACE_SETTLEMENT_WINDOW;
                if let Err(failure) =
                    cleanup_workspace_until(state.clone(), workspace, deadline).await
                {
                    let known = matches!(&failure, CleanupFailure::Known(_));
                    let error = failure.error();
                    let message = error
                        .message
                        .to_utf8()
                        .unwrap_or_else(|_| "native UTF16 cleanup error".into());
                    eprintln!("ara: READ media deferred cleanup {message}");
                    // A retryable UNKNOWN remains owned and visible, but must
                    // not poison a later successful cleanup/Host close.
                    if known {
                        state.cleanup_errors.lock().unwrap().push(error);
                    }
                }
            });
        } else {
            // The workspace remains in admission for a later runtime/close.
            // This transient observation must not become a cached failure.
            eprintln!(
                "READ media workspace cleanup deferred without runtime: {}",
                self.workspace.path.display()
            );
        }
    }
}
async fn cleanup_workspace(
    state: Arc<State>,
    workspace: Arc<WorkspaceState>,
) -> Result<(), VideoError> {
    let deadline = tokio::time::Instant::now() + WORKSPACE_SETTLEMENT_WINDOW;
    cleanup_workspace_until(state, workspace, deadline)
        .await
        .map_err(CleanupFailure::error)
}
async fn cleanup_workspace_until(
    state: Arc<State>,
    workspace: Arc<WorkspaceState>,
    deadline: tokio::time::Instant,
) -> Result<(), CleanupFailure> {
    let mut cleanup = tokio::time::timeout_at(deadline, workspace.cleanup.lock())
        .await
        .map_err(|_| {
            CleanupFailure::Unknown(VideoError::unknown(
                "READ media workspace cleanup wait expired; settlement unknown",
            ))
        })?;
    if let Some(result) = &*cleanup {
        return result.clone().map_err(CleanupFailure::Known);
    }
    let operations = {
        let mut admission = state.admission.lock().unwrap();
        admission.workspace_closed.insert(workspace.serial);
        admission
            .operations
            .values()
            .filter(|operation| operation.workspaces.contains(&workspace.serial))
            .cloned()
            .collect::<Vec<_>>()
    };
    // Workspace disappearance/caller drop owns cancellation before deletion.
    for operation in &operations {
        operation.cancel.cancel();
    }
    for operation in operations {
        settled(operation.done, deadline)
            .await
            .map_err(CleanupFailure::Unknown)?;
    }
    if state
        .unsafe_workspaces
        .lock()
        .unwrap()
        .contains(&workspace.serial)
    {
        return Err(CleanupFailure::Unknown(VideoError::unknown(
            "READ media cleanup retained workspace after unknown process settlement",
        )));
    }
    let result = async {
        let parent = std::fs::canonicalize(
            workspace
                .path
                .parent()
                .ok_or_else(|| VideoError::text("READ media workspace parent missing"))?,
        )
        .map_err(os_error)?;
        if parent != state.temp || !workspace.path.starts_with(&state.temp) {
            return Err(VideoError::text(
                "READ media workspace deletion escaped its owned root",
            ));
        }
        for attempt in 0..=40 {
            match tokio::fs::remove_dir_all(&workspace.path).await {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error)
                    if cfg!(windows)
                        && attempt < 40
                        && matches!(error.raw_os_error(), Some(5 | 32 | 33 | 145)) =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                Err(error) => return Err(os_error(error)),
            }
        }
        unreachable!()
    }
    .await;
    if result.is_ok() {
        state
            .admission
            .lock()
            .unwrap()
            .workspaces
            .remove(&workspace.serial);
    }
    *cleanup = Some(result.clone());
    result.map_err(CleanupFailure::Known)
}
fn os_error(error: std::io::Error) -> VideoError {
    VideoError::text(error.to_string())
}
fn find_on_path(
    name: &str,
    path: Option<&std::ffi::OsStr>,
    cwd: &Path,
) -> Result<Option<PathBuf>, VideoError> {
    let Some(path) = path else {
        return Ok(None);
    };
    for directory in std::env::split_paths(path) {
        let directory = if directory.is_absolute() {
            directory
        } else {
            cwd.join(directory)
        };
        #[cfg(windows)]
        let candidates = [
            directory.join(format!("{name}.exe")),
            directory.join(format!("{name}.com")),
        ];
        #[cfg(not(windows))]
        let candidates = [directory.join(name)];
        for candidate in candidates {
            match std::fs::metadata(&candidate) {
                Ok(metadata) if metadata.is_file() => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if metadata.permissions().mode() & 0o111 == 0 {
                            continue;
                        }
                    }
                    return std::fs::canonicalize(candidate).map(Some).map_err(os_error);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(os_error(error)),
            }
        }
    }
    Ok(None)
}
