//! Process ownership for the project service broker, never the Agent's Bash jobs.
//!
//! Source: OMP b91db66e launch/broker.ts, pi-natives/src/pty.rs,
//! pi-shell/src/process.rs and utils/src/vterm/query-responder.ts.
//! The query responder below is a Rust translation of the latter (MIT):
//! Copyright (c) 2025 Mario Zechner
//! Copyright (c) 2025-2026 Can Bölük
//! Copyright (c) 2026 Stencil Labs, Inc.
//! See THIRD_PARTY_NOTICES.md for the complete license notice.
//!
//! All blocking IO, child waits and tree sweeps live on owned threads or the
//! blocking pool. No service-registry lock is taken here. Dropping a handle is
//! deliberately NOT a termination operation: a detached child must survive its
//! broker. The broker explicitly stops session-owned children before shutdown.

use super::protocol::{ProcessExit, ProcessIdentity, ServiceSpec};
use anyhow::{Context, Result, anyhow, bail, ensure};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::watch;

type Input = Arc<Mutex<Option<Box<dyn Write + Send>>>>;
type Master = Arc<Mutex<Option<Box<dyn MasterPty + Send>>>>;

struct OutputReader {
    io: Box<dyn Read + Send>,
    #[cfg(unix)]
    fd: std::os::fd::RawFd,
}

const GIT_REPO_LOCATION_ENV: [&str; 6] = [
    "GIT_DIR", "GIT_COMMON_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// Source: utils/src/env.ts stripGitRepoLocationEnv at the pinned OMP target.
/// POSIX keys are exact; Windows lookups ignore ASCII case for these six names.
pub(super) fn is_git_repo_location_key(key: &OsStr, windows: bool) -> bool {
    GIT_REPO_LOCATION_ENV.iter().any(|name| if windows {
        key.to_str().is_some_and(|key| key.eq_ignore_ascii_case(name))
    } else { key == OsStr::new(name) })
}

/// Source: worker-client.ts:185-197. Copy the inherited environment, remove
/// repository-location overrides, THEN apply explicit caller choices. OsString
/// preserves opaque POSIX values instead of failing the entire environment.
/// Builders consume the ordered entries, applying their native key comparison.
pub(crate) fn worker_environment(overlay: &BTreeMap<String, String>) -> Vec<(OsString, OsString)> {
    std::env::vars_os()
        .filter(|(key, _)| !is_git_repo_location_key(key, cfg!(windows)))
        .chain(overlay.iter().map(|(key, value)| (OsString::from(key.as_str()), OsString::from(value.as_str()))))
        .collect()
}

/// Parse only the ASCII fields AFTER comm. Linux truncates comm to 15 BYTES,
/// potentially inside a UTF-8 codepoint; spaces and ')' are also legal there.
/// This cross-platform pure parser is exposed to the centralized unit bank.
pub(super) fn parse_linux_stat_bytes(raw: &[u8]) -> Result<(u32, u32, u32, u64)> {
    let end = raw.iter().rposition(|byte| *byte == b')')
        .ok_or_else(|| anyhow!("malformed /proc process stat"))?;
    let fields = &raw[end + 1..];
    ensure!(fields.is_ascii(), "non-ASCII /proc process stat fields");
    let fields: Vec<_> = std::str::from_utf8(fields)?.split_ascii_whitespace().collect();
    ensure!(fields.len() > 19, "short /proc process stat");
    Ok((fields[1].parse()?, fields[2].parse()?, fields[3].parse()?, fields[19].parse()?))
}

pub(crate) struct ProcessHandle {
    root: Arc<platform::Target>,
    input: Input,
    pty: bool,
    terminal: watch::Receiver<Option<ProcessExit>>,
    stop_gate: Arc<tokio::sync::Mutex<()>>,
    // Keeping this owner alive preserves the PTY while the service is running.
    _master: Master,
}

impl ProcessHandle {
    pub(crate) fn pid(&self) -> u32 { self.root.identity.pid }

    pub(crate) fn identity(&self) -> ProcessIdentity { self.root.identity.clone() }

    pub(crate) fn terminal(&self) -> watch::Receiver<Option<ProcessExit>> {
        self.terminal.clone()
    }

    pub(crate) fn is_alive(&self) -> Result<bool> { self.root.alive() }

    /// Caller runs this on the blocking pool. It is one write operation, never
    /// replayed after an IO error (some bytes may already have reached the child).
    /// Pipe mode changes only a FINAL CR into LF, matching launch/broker.ts.
    pub(crate) fn write(&self, data: &[u8]) -> Result<()> {
        ensure!(self.is_alive()?, "service process has exited");
        let mut input = self.input.lock().map_err(|_| anyhow!("service stdin lock poisoned"))?;
        let input = input.as_mut().ok_or_else(|| anyhow!("service stdin is unavailable"))?;
        if !self.pty && data.last() == Some(&b'\r') {
            input.write_all(&data[..data.len() - 1]).context("service stdin may be partially written")?;
            input.write_all(b"\n").context("service stdin may be partially written")?;
        } else {
            input.write_all(data).context("service stdin may be partially written")?;
        }
        input.flush().context("service stdin flush failed; delivery is unknown")
    }

    /// The owned task continues settling a dispatched stop even if its caller
    /// disconnects. Repeated callers serialize on the same process owner.
    pub(crate) async fn stop(&self, grace_ms: u64) -> Result<ProcessExit> {
        let root = self.root.clone();
        let gate = self.stop_gate.clone();
        let mut terminal = self.terminal.clone();
        tokio::spawn(async move {
            let _guard = gate.lock().await;
            let observed = tokio::task::spawn_blocking(move || terminate_tree(root, grace_ms))
                .await.context("service termination worker failed")??;
            if let Some(exit) = terminal.borrow().clone() { return Ok(exit); }
            // The process wait receipt is a distinct frontier from a successful
            // signal syscall. This is the broker's source second wait window.
            let settle = async {
                loop {
                    if let Some(exit) = terminal.borrow().clone() { return Ok(exit); }
                    terminal.changed().await.context("service waiter disappeared before terminal receipt")?;
                }
            };
            match tokio::time::timeout(Duration::from_millis(grace_ms.saturating_add(1_000)), settle).await {
                Ok(result) => result,
                Err(_) => bail!("service tree stopped but child wait receipt did not settle: {:?}", observed.reason),
            }
        }).await.context("service stop settlement task failed")?
    }
}

enum Child {
    Pipe(std::process::Child),
    Pty(Box<dyn portable_pty::Child + Send + Sync>),
}

impl Child {
    fn pid(&self) -> Result<u32> {
        match self {
            Self::Pipe(child) => Ok(child.id()),
            Self::Pty(child) => child.process_id().ok_or_else(|| anyhow!("PTY child has no OS pid")),
        }
    }

    fn wait(&mut self) -> Result<ProcessExit> {
        match self {
            Self::Pipe(child) => {
                let status = child.wait().context("wait service process")?;
                #[cfg(unix)]
                let reason = { use std::os::unix::process::ExitStatusExt; status.signal().map(|s| format!("signal {s}")) };
                #[cfg(not(unix))]
                let reason = None;
                Ok(ProcessExit { code: status.code(), reason })
            }
            Self::Pty(child) => {
                let status = child.wait().context("wait PTY service process")?;
                Ok(ProcessExit {
                    code: if status.signal().is_some() { None } else { i32::try_from(status.exit_code()).ok() },
                    reason: status.signal().map(|signal| format!("signal {signal}")),
                })
            }
        }
    }

    fn cleanup_failed_start(&mut self) {
        match self {
            Self::Pipe(child) => { let _ = child.kill(); let _ = child.wait(); }
            Self::Pty(child) => { let _ = child.kill(); let _ = child.wait(); }
        }
    }

    fn kill_root(&mut self) -> Result<()> {
        match self {
            Self::Pipe(child) => child.kill().context("kill failed-launch root"),
            Self::Pty(child) => child.kill().context("kill failed-launch PTY root"),
        }
    }

    fn try_wait(&mut self) -> Result<Option<ProcessExit>> {
        match self {
            Self::Pipe(child) => child.try_wait().context("reap failed-launch root").map(|status| status.map(|status| {
                #[cfg(unix)]
                let reason = { use std::os::unix::process::ExitStatusExt; status.signal().map(|signal| format!("signal {signal}")) };
                #[cfg(not(unix))]
                let reason = None;
                ProcessExit { code: status.code(), reason }
            })),
            Self::Pty(child) => child.try_wait().context("reap failed-launch PTY root").map(|status| status.map(|status| ProcessExit {
                code: if status.signal().is_some() { None } else { i32::try_from(status.exit_code()).ok() },
                reason: status.signal().map(|signal| format!("signal {signal}")),
            })),
        }
    }
}

/// Called by the broker on spawn_blocking; it publishes no registry state.
pub(crate) fn spawn(spec: &ServiceSpec, log_path: &Path) -> Result<Arc<ProcessHandle>> {
    #[cfg(not(any(windows, target_os = "linux")))]
    bail!("service process identity unsupported on this platform");
    ensure!(!spec.application.is_empty(), "empty service executable");
    ensure!(!(spec.detached && spec.pty), "detached service must use pipe mode");
    let log = Arc::new(Mutex::new(OpenOptions::new().create(true).append(true).open(log_path)
        .with_context(|| format!("open service log {}", log_path.display()))?));
    let input: Input = Arc::new(Mutex::new(None));
    let master: Master = Arc::new(Mutex::new(None));
    let environment = worker_environment(&spec.env);
    let mut readers = Vec::new();
    let mut child = if spec.pty {
        let pair = native_pty_system().openpty(PtySize {
            rows: 40, cols: 120, pixel_width: 0, pixel_height: 0,
        }).context("open service PTY")?;
        let reader = pair.master.try_clone_reader().context("clone service PTY reader")?;
        #[cfg(unix)]
        let reader_fd = pair.master.as_raw_fd().ok_or_else(|| anyhow!("PTY has no pollable master fd"))?;
        let writer = pair.master.take_writer().context("take service PTY writer")?;
        *input.lock().expect("new input lock") = Some(writer);
        let mut command = CommandBuilder::new(&spec.application);
        command.args(&spec.args);
        command.cwd(&spec.cwd);
        command.env_clear();
        for (key, value) in &environment { command.env(key, value); }
        // The default precedes the explicit overlay, as in launch/broker.ts.
        command.env("TERM", "xterm-256color");
        for (key, value) in &spec.env { command.env(key, value); }
        let spawned = pair.slave.spawn_command(command).context("spawn service PTY executable")?;
        drop(pair.slave);
        *master.lock().expect("new PTY lock") = Some(pair.master);
        readers.push(OutputReader { io: reader, #[cfg(unix)] fd: reader_fd });
        Child::Pty(spawned)
    } else {
        let mut command = Command::new(&spec.application);
        command.args(&spec.args).current_dir(&spec.cwd).env_clear().envs(environment.iter().map(|(key, value)| (key, value)));
        platform::configure_command(&mut command, spec.detached);
        if spec.detached {
            // No broker-owned pipe, PTY, kill-on-close Job or stdin can keep a
            // detached child tied to broker lifetime. The child owns log handles.
            command.stdin(Stdio::null());
            let file = log.lock().expect("new log lock").try_clone()?;
            command.stdout(Stdio::from(file.try_clone()?)).stderr(Stdio::from(file));
        } else {
            command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        let mut spawned = command.spawn().context("spawn service pipe executable")?;
        if !spec.detached {
            *input.lock().expect("new input lock") = spawned.stdin.take().map(|s| Box::new(s) as Box<dyn Write + Send>);
            if let Some(stdout) = spawned.stdout.take() {
                #[cfg(unix)]
                let fd = { use std::os::fd::AsRawFd; stdout.as_raw_fd() };
                readers.push(OutputReader { io: Box::new(stdout), #[cfg(unix)] fd });
            }
            if let Some(stderr) = spawned.stderr.take() {
                #[cfg(unix)]
                let fd = { use std::os::fd::AsRawFd; stderr.as_raw_fd() };
                readers.push(OutputReader { io: Box::new(stderr), #[cfg(unix)] fd });
            }
        }
        Child::Pipe(spawned)
    };
    let root = match child.pid().and_then(platform::Target::capture) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            // Before a stable identity is available, only the owned Child can
            // be targeted. Do not claim a verified descendant-tree cleanup.
            child.cleanup_failed_start();
            return Err(error.context("service identity capture failed; root-only cleanup attempted, descendant cleanup unconfirmed"));
        }
    };
    let mut drains = Vec::new();
    let stop_draining = Arc::new(AtomicBool::new(false));
    #[cfg(windows)]
    if spec.pty {
        // portable-pty 0.9.0 exposes raw ConPTY. Like pi-natives/src/pty.rs,
        // this wrapper must answer its initial INHERIT_CURSOR handshake itself.
        let handshake = (|| -> Result<()> {
            let mut guard = input.lock().map_err(|_| anyhow!("service stdin lock poisoned"))?;
            let writer = guard.as_mut().ok_or_else(|| anyhow!("missing PTY startup writer"))?;
            writer.write_all(b"\x1b[1;1R")?;
            writer.flush()?;
            Ok(())
        })();
        if let Err(error) = handshake {
            return Err(cleanup_thread_start_failure(error, &root, &mut child, &stop_draining, &mut drains, &input, &master));
        }
    }
    let reader_count = readers.len();
    for (index, reader) in readers.into_iter().enumerate() {
        if index + 1 == reader_count {
            if let Some(error) = inject_start_failure(spec, "output") {
                return Err(cleanup_thread_start_failure(error, &root, &mut child, &stop_draining, &mut drains, &input, &master));
            }
        }
        let reader_input = if spec.pty { Some(input.clone()) } else { None };
        let log = log.clone();
        let stop = stop_draining.clone();
        match thread::Builder::new().name(format!("service-output-{}-{index}", root.identity.pid))
            .spawn(move || drain(reader, log, reader_input, stop)) {
            Ok(drain) => drains.push(drain),
            Err(error) => {
                return Err(cleanup_thread_start_failure(error.into(), &root, &mut child, &stop_draining, &mut drains, &input, &master));
            }
        }
    }
    if let Some(error) = inject_start_failure(spec, "waiter") {
        return Err(cleanup_thread_start_failure(error, &root, &mut child, &stop_draining, &mut drains, &input, &master));
    }
    let (terminal_tx, terminal) = watch::channel(None);
    let wait_master = master.clone();
    // A failed OS spawn drops its closure. Keep both Child and reader handles in
    // a recoverable slot, rather than losing the only way to join those threads.
    let child_slot = Arc::new(Mutex::new(Some((child, drains))));
    let wait_child = child_slot.clone();
    let wait_log = log.clone();
    let wait_root = root.clone();
    let wait_stop = stop_draining.clone();
    let waiter = thread::Builder::new().name(format!("service-wait-{}", root.identity.pid)).spawn(move || {
        let (mut child, mut drains) = wait_child.lock().expect("private child slot").take().expect("single child waiter");
        let mut exit = match child.wait() {
            Ok(exit) => exit,
            Err(error) => {
                // A wait failure is NOT proof of process exit. Poll the pinned
                // OS object; do not publish a terminal receipt while it is live.
                let reason = format!("wait failed: {error:#}");
                if let Ok(mut log) = wait_log.lock() { let _ = writeln!(log, "[service] {reason}"); }
                loop {
                    match wait_root.alive() {
                        Ok(false) => break,
                        Ok(true) | Err(_) => thread::sleep(Duration::from_millis(100)),
                    }
                }
                ProcessExit { code: None, reason: Some(reason) }
            }
        };
        // Normal EOF drains all output before publishing terminal. A descendant
        // may retain the slave/pipe forever; source uses a two-second stale-slave
        // window rather than claiming that child.wait implies output EOF.
        let deadline = Instant::now() + Duration::from_secs(2);
        while drains.iter().any(|drain| !drain.is_finished()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        if let Err(error) = shutdown_drains(&mut drains, &wait_stop) {
            exit.reason = Some(format!("{:?}; output cleanup failed: {error:#}", exit.reason));
        }
        if let Ok(mut log) = wait_log.lock() { let _ = log.flush(); }
        if let Err(error) = close_master_bounded(&wait_master) {
            exit.reason = Some(format!("{:?}; PTY cleanup failed: {error:#}", exit.reason));
        }
        terminal_tx.send_replace(Some(exit));
    });
    if let Err(error) = waiter {
        let (mut child, mut drains) = child_slot.lock().expect("private child slot").take()
            .expect("failed waiter spawn leaves launch resources in slot");
        return Err(cleanup_thread_start_failure(error.into(), &root, &mut child, &stop_draining, &mut drains, &input, &master));
    }
    Ok(Arc::new(ProcessHandle {
        root, input, pty: spec.pty, terminal,
        stop_gate: Arc::new(tokio::sync::Mutex::new(())), _master: master,
    }))
}

fn shutdown_drains(drains: &mut Vec<thread::JoinHandle<()>>, stop: &AtomicBool) -> Result<()> {
    stop.store(true, Ordering::Release);
    // Cancel only our own blocking IO threads. Repeating closes the race
    // between their stop check and the next synchronous ReadFile/WriteFile.
    #[cfg(windows)]
    let budget = Duration::from_secs(1);
    #[cfg(not(windows))]
    let budget = Duration::from_millis(100);
    let deadline = Instant::now() + budget;
    while drains.iter().any(|drain| !drain.is_finished()) && Instant::now() < deadline {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            for drain in drains.iter().filter(|drain| !drain.is_finished()) {
                unsafe { windows_sys::Win32::System::IO::CancelSynchronousIo(drain.as_raw_handle()); }
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    let mut failures = Vec::new();
    for drain in drains.drain(..) {
        if drain.is_finished() {
            if drain.join().is_err() { failures.push("output thread panicked"); }
        } else {
            // Do not block forever joining IO the OS could not interrupt.
            // Its stop flag remains set and its failure is part of the receipt.
            failures.push("output thread did not stop within IO cancellation budget");
        }
    }
    ensure!(failures.is_empty(), "{}", failures.join("; "));
    Ok(())
}

fn close_master_bounded(master: &Master) -> Result<()> {
    let Some(pty) = master.try_lock().map_err(|_| anyhow!("PTY owner unavailable during cleanup"))?.take() else { return Ok(()); };
    // ClosePseudoConsole can block in the OS. A recoverable slot prevents an
    // OS thread-creation failure from synchronously dropping it on this caller.
    let slot = Arc::new(Mutex::new(Some(pty)));
    let close_slot = slot.clone();
    let closer = thread::Builder::new().name("service-pty-close".into()).spawn(move || {
        let pty = close_slot.lock().expect("private PTY disposal slot").take();
        drop(pty);
    });
    let closer = match closer {
        Ok(closer) => closer,
        Err(error) => {
            // Actual OS thread exhaustion leaves no safe place for an
            // uninterruptible destructor. Explicitly retain this OS resource,
            // report incomplete cleanup, and never report a successful launch.
            if let Some(pty) = slot.lock().expect("private PTY disposal slot").take() { std::mem::forget(pty); }
            return Err(error).context("PTY disposal could not start; OS resource retained to keep failure cleanup bounded");
        }
    };
    let deadline = Instant::now() + Duration::from_secs(1);
    while !closer.is_finished() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); }
    ensure!(closer.is_finished(), "PTY close remains in progress beyond IO cleanup budget");
    closer.join().map_err(|_| anyhow!("PTY closer panicked"))
}

fn cleanup_thread_start_failure(
    cause: anyhow::Error, root: &Arc<platform::Target>, child: &mut Child,
    stop: &AtomicBool, drains: &mut Vec<thread::JoinHandle<()>>, input: &Input, master: &Master,
) -> anyhow::Error {
    stop.store(true, Ordering::Release);
    let mut failures = Vec::new();
    // A failed launch owns its entire new process family, including a detached
    // family. This explicit failure path is distinct from normal handle Drop.
    if let Err(error) = terminate_tree(root.clone(), 200) { failures.push(format!("tree termination: {error:#}")); }
    match root.alive() {
        Ok(false) => {},
        Ok(true) | Err(_) => if let Err(error) = child.kill_root() { failures.push(format!("root fallback: {error:#}")); },
    }
    let reap_deadline = Instant::now() + Duration::from_secs(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < reap_deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => { failures.push("root child wait did not settle within cleanup budget".into()); break; },
            Err(error) => { failures.push(format!("root wait: {error:#}")); break; },
        }
    }
    if let Err(error) = shutdown_drains(drains, stop) { failures.push(format!("reader cleanup: {error:#}")); }
    match input.try_lock() {
        Ok(mut input) => { input.take(); },
        Err(_) => failures.push("stdin owner unavailable during failed-launch cleanup".into()),
    }
    if let Err(error) = close_master_bounded(master) { failures.push(format!("PTY cleanup: {error:#}")); }
    if failures.is_empty() {
        cause.context("service launch thread failed; cleanup observed tree terminal, child reaped, and readers joined")
    } else {
        cause.context(format!("service launch thread failed; cleanup incomplete: {}", failures.join("; ")))
    }
}

fn inject_start_failure(spec: &ServiceSpec, stage: &str) -> Option<anyhow::Error> {
    #[cfg(feature = "test-fixture")]
    if spec.env.get("ARA_SERVICE_TEST_FAIL_START_STAGE").is_some_and(|value| value == stage) {
        let Some(ready) = spec.env.get("ARA_SERVICE_TEST_START_READY_FILE") else {
            return Some(anyhow!("fixture launch-failure injection requires owned leaf.ready path"));
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        while !Path::new(ready).is_file() && Instant::now() < deadline { thread::sleep(Duration::from_millis(10)); }
        if !Path::new(ready).is_file() { return Some(anyhow!("fixture leaf.ready was not observed within 3s")); }
        return Some(std::io::Error::new(std::io::ErrorKind::WouldBlock, format!("injected EAGAIN creating service {stage} thread after owned leaf.ready")).into());
    }
    let _ = (spec, stage);
    None
}

fn drain(mut reader: OutputReader, log: Arc<Mutex<File>>, input: Option<Input>, stop: Arc<AtomicBool>) {
    let mut bytes = [0u8; 65_536];
    let mut responder = QueryResponder::new(cfg!(windows));
    let mut log_failed = false;
    loop {
        if stop.load(Ordering::Acquire) { break; }
        #[cfg(unix)]
        {
            // Poll the sole reader before blocking read, allowing a stale slave
            // to be abandoned. Do not set O_NONBLOCK on the PTY: its duplicated
            // writer shares file-status flags and must keep blocking semantics.
            let mut fd = libc::pollfd { fd: reader.fd, events: libc::POLLIN, revents: 0 };
            let ready = unsafe { libc::poll(&mut fd, 1, 50) };
            if ready == 0 { continue; }
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted { continue; }
                if let Ok(mut log) = log.lock() { let _ = writeln!(log, "[service] output poll failed: {error}"); }
                break;
            }
        }
        let count = match reader.io.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => { thread::sleep(Duration::from_millis(10)); continue; }
            Err(error) => {
                if stop.load(Ordering::Acquire) { break; }
                // Linux PTYs conventionally signal EOF as EIO.
                #[cfg(unix)]
                if error.raw_os_error() == Some(libc::EIO) { break; }
                if let Ok(mut log) = log.lock() { let _ = writeln!(log, "[service] output read failed: {error}"); }
                break;
            }
        };
        if let Some(input) = &input {
            let reply = responder.feed(&bytes[..count]);
            if !reply.is_empty() {
                if let Ok(mut guard) = input.lock() {
                    if let Some(writer) = guard.as_mut() {
                        // A final probe can race exit; never replay a partial reply.
                        let _ = writer.write_all(&reply).and_then(|_| writer.flush());
                    }
                }
            }
        }
        // A full/broken log must not stop draining the child's output pipe.
        if !log_failed {
            log_failed = match log.lock() {
                Ok(mut log) => log.write_all(&bytes[..count]).and_then(|_| log.flush()).is_err(),
                Err(_) => true,
            };
        }
    }
}

pub(crate) fn inspect_identity(pid: u32) -> Result<Option<ProcessIdentity>> {
    match platform::Target::capture(pid) {
        Ok(target) => if target.alive()? { Ok(Some(target.identity)) } else { Ok(None) },
        Err(error) if platform::is_gone(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Recovered detached children have no inherited std::process::Child waiter.
/// Pin their OS identity before any signal, and return only observed termination.
pub(crate) async fn stop_recovered(identity: &ProcessIdentity, grace_ms: u64) -> Result<ProcessExit> {
    let identity = identity.clone();
    tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let target = match platform::Target::capture(identity.pid) {
                Ok(target) => target,
                Err(error) if platform::is_gone(&error) => return Ok(ProcessExit { code: None, reason: Some("recovered process no longer exists".into()) }),
                Err(error) => return Err(error),
            };
            ensure!(target.identity == identity, "service PID creation identity changed; refusing to stop another process");
            terminate_tree(Arc::new(target), grace_ms)
        }).await.context("recovered service termination worker failed")?
    }).await.context("recovered service stop settlement failed")?
}

fn terminate_tree(root: Arc<platform::Target>, grace_ms: u64) -> Result<ProcessExit> {
    ensure!(root.identity.pid != std::process::id(), "refusing to stop the service broker itself");
    let mut targets = root.descendants()?;
    targets.push(root);
    // Two real, independent waits: polite grace then hard timeout. In OMP
    // terminate_tree_impl, timeout_ms is a HARD-wave wait, not an overall cap.
    for target in &targets { target.signal(false)?; }
    if wait_all(&targets, Duration::from_millis(grace_ms))? {
        return observed_exit(&targets);
    }
    let mut additional = Vec::new();
    for target in &targets {
        for child in target.descendants()? {
            if !targets.iter().any(|known| known.identity == child.identity)
                && !additional.iter().any(|known: &Arc<platform::Target>| known.identity == child.identity) {
                additional.push(child);
            }
        }
    }
    targets.extend(additional);
    // Descendants first. Captured targets survive parent exit/reparenting.
    targets.sort_by_key(|target| std::cmp::Reverse(target.creation_order()));
    for target in &targets { target.signal(true)?; }
    ensure!(wait_all(&targets, Duration::from_millis(grace_ms.saturating_add(1_000)))?,
        "service process tree still alive after hard termination; outcome unresolved");
    observed_exit(&targets)
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{collections::{HashMap, HashSet}, mem, os::windows::process::CommandExt};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, GetLastError, FILETIME, HANDLE, INVALID_HANDLE_VALUE, ERROR_INVALID_PARAMETER,
            ERROR_NO_MORE_FILES, WAIT_OBJECT_0, WAIT_TIMEOUT},
        System::{
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS},
            Threading::{OpenProcess, GetProcessTimes, TerminateProcess, WaitForSingleObject,
                CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_SYNCHRONIZE},
        },
    };

    struct Handle(HANDLE);
    // These are owned kernel handles, not GUI/thread-affine resources.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}
    impl Drop for Handle { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }

    pub(super) struct Target {
        pub(super) identity: ProcessIdentity,
        handle: Handle,
        created: u64,
    }

    impl Target {
        pub(super) fn capture(pid: u32) -> Result<Self> {
            ensure!(pid != 0, "invalid service pid");
            // An owned handle remains bound to this process object after PID recycling.
            let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
            if handle.is_null() { return Err(std::io::Error::last_os_error().into()); }
            let handle = Handle(handle);
            let mut creation: FILETIME = unsafe { mem::zeroed() };
            let mut exit = creation;
            let mut kernel = creation;
            let mut user = creation;
            ensure!(unsafe { GetProcessTimes(handle.0, &mut creation, &mut exit, &mut kernel, &mut user) } != 0,
                "read service process creation time: {}", std::io::Error::last_os_error());
            let created = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            Ok(Self { identity: ProcessIdentity { pid, created: format!("windows-filetime:{created}") }, handle, created })
        }

        pub(super) fn alive(&self) -> Result<bool> {
            match unsafe { WaitForSingleObject(self.handle.0, 0) } {
                WAIT_TIMEOUT => Ok(true), WAIT_OBJECT_0 => Ok(false),
                _ => Err(std::io::Error::last_os_error().into()),
            }
        }

        pub(super) fn creation_order(&self) -> u64 { self.created }

        /// Query through the original owned HANDLE, including after its process
        /// exits. A numeric Toolhelp PPID is not itself a creation identity.
        fn exit_time(&self) -> Result<Option<u64>> {
            // GetProcessTimes' exit FILETIME is only meaningful after exit.
            // If this process exits just after the probe, retaining its HANDLE
            // still prevents adopting a later process through the same PID.
            if self.alive()? { return Ok(None); }
            let mut creation: FILETIME = unsafe { mem::zeroed() };
            let mut exit = creation;
            let mut kernel = creation;
            let mut user = creation;
            ensure!(unsafe { GetProcessTimes(self.handle.0, &mut creation, &mut exit, &mut kernel, &mut user) } != 0,
                "read service lineage times: {}", std::io::Error::last_os_error());
            let created = (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
            ensure!(created == self.created, "pinned service process creation identity changed");
            let exited = (u64::from(exit.dwHighDateTime) << 32) | u64::from(exit.dwLowDateTime);
            ensure!(exited != 0, "exited service process has no observable exit time");
            Ok(Some(exited))
        }

        pub(super) fn signal(&self, _hard: bool) -> Result<()> {
            ensure!(self.identity.pid != std::process::id(), "refusing to terminate broker process");
            if !self.alive()? { return Ok(()); }
            // This matches fixed pi-shell's Windows behavior: both POSIX signal
            // numbers become TerminateProcess. Attaching/freeing the broker's
            // process-global console to imitate SIGTERM would be unsafe.
            if unsafe { TerminateProcess(self.handle.0, 1) } == 0 {
                let error = std::io::Error::last_os_error();
                if self.alive()? { return Err(error.into()); }
            }
            Ok(())
        }

        pub(super) fn descendants(&self) -> Result<Vec<Arc<Self>>> {
            // An exited parent can still have live children holding its PTY or
            // stdout. Its retained HANDLE continues to anchor the ORIGINAL
            // creation identity; do not replace it by reopening the numeric PID
            // or infer an empty tree merely from its signalled process object.
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
            if snapshot == INVALID_HANDLE_VALUE { return Err(std::io::Error::last_os_error().into()); }
            let snapshot = Handle(snapshot);
            let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
            entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
            let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) } != 0;
            while more {
                children.entry(entry.th32ParentProcessID).or_default().push(entry.th32ProcessID);
                more = unsafe { Process32NextW(snapshot.0, &mut entry) } != 0;
            }
            let snapshot_error = unsafe { GetLastError() };
            if snapshot_error != ERROR_NO_MORE_FILES {
                return Err(std::io::Error::from_raw_os_error(snapshot_error as i32).into());
            }
            let mut seen = HashSet::from([self.identity.pid, std::process::id()]);
            let mut pending = vec![(self.identity.pid, self.created, self.exit_time()?)];
            let mut out = Vec::new();
            while let Some((parent, parent_created, parent_exited)) = pending.pop() {
                for &pid in children.get(&parent).into_iter().flatten() {
                    // Protect the whole broker subtree: never recurse through a
                    // protected node, even if Toolhelp's stale PPID matches.
                    if !seen.insert(pid) { continue; }
                    let child = match Self::capture(pid) {
                        Ok(child) => child,
                        Err(error) if is_gone(&error) => continue,
                        Err(error) => return Err(error.context("pin service descendant")),
                    };
                    // PPIDs survive parent death on Windows. The relationship
                    // must fit the ORIGINAL parent's lifetime. An older orphan
                    // or a child born after its claimed parent's exit must not
                    // become a signal target through numeric PID reuse.
                    if child.created < parent_created
                        || parent_exited.is_some_and(|exited| child.created > exited) { continue; }
                    let child_exited = child.exit_time()?;
                    pending.push((pid, child.created, child_exited));
                    // Retain even an exited intermediate parent's HANDLE while
                    // walking and across the hard wave. Its own signal is a
                    // no-op; its still-live descendants remain discoverable.
                    out.push(Arc::new(child));
                }
            }
            out.sort_by_key(|child| std::cmp::Reverse(child.created));
            Ok(out)
        }
    }

    pub(super) fn is_gone(error: &anyhow::Error) -> bool {
        error.downcast_ref::<std::io::Error>().is_some_and(|error|
            error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32)
                || error.kind() == std::io::ErrorKind::NotFound)
    }

    pub(super) fn configure_command(command: &mut Command, detached: bool) {
        command.creation_flags(if detached {
            // Never silently retry without BREAKAWAY: succeeding inside an
            // inherited kill-on-close Job would falsely promise persistence.
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB
        } else {
            CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
        });
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{collections::{HashMap, HashSet}, fs, os::{fd::{AsRawFd, FromRawFd, OwnedFd}, unix::process::CommandExt}};

    pub(super) struct Target {
        pub(super) identity: ProcessIdentity,
        pidfd: OwnedFd,
        start: u64,
        session_leader: bool,
    }

    struct Stat { pid: u32, parent: u32, group: u32, session: u32, start: u64 }

    fn stat(pid: u32) -> Result<Stat> {
        let raw = fs::read(format!("/proc/{pid}/stat"))?;
        let (parent, group, session, start) = parse_linux_stat_bytes(&raw)?;
        Ok(Stat { pid, parent, group, session, start })
    }

    impl Target {
        pub(super) fn capture(pid: u32) -> Result<Self> {
            ensure!(pid > 0 && pid <= i32::MAX as u32, "invalid service pid");
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0) };
            if fd < 0 { return Err(std::io::Error::last_os_error().into()); }
            let pidfd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
            let info = stat(pid)?;
            let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
            Ok(Self {
                identity: ProcessIdentity { pid, created: format!("linux-start:{}:{}", boot.trim(), info.start) },
                pidfd, start: info.start,
                session_leader: info.group == pid && info.session == pid,
            })
        }

        pub(super) fn alive(&self) -> Result<bool> {
            let mut descriptor = libc::pollfd { fd: self.pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            loop {
                let result = unsafe { libc::poll(&mut descriptor, 1, 0) };
                if result >= 0 { return Ok(result == 0); }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted { return Err(error.into()); }
            }
        }

        pub(super) fn creation_order(&self) -> u64 { self.start }

        pub(super) fn signal(&self, hard: bool) -> Result<()> {
            ensure!(self.identity.pid != std::process::id(), "refusing to signal broker process");
            let signal = if hard { libc::SIGKILL } else { libc::SIGTERM };
            // pidfd_send_signal is atomic with respect to PID reuse. In
            // particular, recovered detached state never authorizes kill(pid)
            // against a process opened after the creation-identity check.
            let result = unsafe { libc::syscall(libc::SYS_pidfd_send_signal, self.pidfd.as_raw_fd(), signal, std::ptr::null::<libc::siginfo_t>(), 0) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) { return Err(error.into()); }
            }
            Ok(())
        }

        pub(super) fn descendants(&self) -> Result<Vec<Arc<Self>>> {
            // Rewalk the OWN session group as well as PPID descendants, so
            // grandchildren reparented during the polite wait are not missed.
            // Every member is pinned individually; do not signal a numeric
            // negative PGID after the original leader's PID might be recycled.
            if let Ok(current) = stat(self.identity.pid) {
                if current.start != self.start { return Ok(Vec::new()); }
            }
            let mut all = Vec::new();
            for entry in fs::read_dir("/proc")? {
                let entry = entry?;
                let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue; };
                match stat(pid) {
                    Ok(info) => all.push(info),
                    Err(error) if is_gone(&error) || error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied) => continue,
                    Err(error) => return Err(error),
                }
            }
            let by_pid: HashMap<_, _> = all.iter().map(|info| (info.pid, info)).collect();
            let mut seen = HashSet::from([self.identity.pid, std::process::id()]);
            let mut pending = vec![(self.identity.pid, self.start)];
            if self.session_leader {
                for info in &all {
                    if info.pid != std::process::id() && info.pid != self.identity.pid
                        && info.group == self.identity.pid && info.session == self.identity.pid && info.start >= self.start {
                        pending.push((info.pid, info.start));
                    }
                }
            }
            let mut selected = HashSet::new();
            while let Some((parent, parent_start)) = pending.pop() {
                if parent != self.identity.pid { selected.insert(parent); }
                for info in &all {
                    if info.parent == parent && info.start >= parent_start && seen.insert(info.pid) {
                        pending.push((info.pid, info.start));
                    }
                }
            }
            let mut out = Vec::new();
            for pid in selected {
                if pid == std::process::id() { continue; }
                let target = match Self::capture(pid) {
                    Ok(target) => target,
                    Err(error) if is_gone(&error) => continue,
                    Err(error) => return Err(error.context("pin service descendant")),
                };
                if by_pid.get(&pid).is_some_and(|info| info.start == target.start) { out.push(Arc::new(target)); }
            }
            out.sort_by_key(|child| std::cmp::Reverse(child.start));
            Ok(out)
        }
    }

    pub(super) fn is_gone(error: &anyhow::Error) -> bool {
        error.downcast_ref::<std::io::Error>().is_some_and(|error|
            error.raw_os_error() == Some(libc::ESRCH) || error.kind() == std::io::ErrorKind::NotFound)
    }

    pub(super) fn configure_command(command: &mut Command, _detached: bool) {
        // A private session/process group is required for both supervised and
        // detached pipe children, and detached children must not inherit the
        // broker's controlling terminal. This callback is async-signal-safe.
        unsafe { command.pre_exec(|| if libc::setsid() == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }); }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    use super::*;
    // No unverified `ps` parsing or weak PID-only fallback. This delivered Host
    // currently supports Windows and Linux identity-pinned service control.
    pub(super) struct Target { pub(super) identity: ProcessIdentity }
    impl Target {
        pub(super) fn capture(_pid: u32) -> Result<Self> { bail!("service process identity unsupported on this platform") }
        pub(super) fn alive(&self) -> Result<bool> { bail!("service process identity unsupported on this platform") }
        pub(super) fn creation_order(&self) -> u64 { 0 }
        pub(super) fn signal(&self, _hard: bool) -> Result<()> { bail!("service process control unsupported on this platform") }
        pub(super) fn descendants(&self) -> Result<Vec<Arc<Self>>> { bail!("service process control unsupported on this platform") }
    }
    pub(super) fn is_gone(_error: &anyhow::Error) -> bool { false }
    pub(super) fn configure_command(_command: &mut Command, _detached: bool) {}
}

fn wait_all(targets: &[impl AsRef<platform::Target>], timeout: Duration) -> Result<bool> {
    let start = Instant::now();
    loop {
        let mut alive = false;
        for target in targets { alive |= target.as_ref().alive()?; }
        if !alive { return Ok(true); }
        if start.elapsed() >= timeout { return Ok(false); }
        thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(start.elapsed())));
    }
}

fn observed_exit(targets: &[impl AsRef<platform::Target>]) -> Result<ProcessExit> {
    ensure!(targets.iter().all(|target| matches!(target.as_ref().alive(), Ok(false))), "terminal observation lost");
    Ok(ProcessExit { code: None, reason: Some("process tree termination observed; recovered exit code unavailable".into()) })
}

// Exact canned responses and bounded cross-chunk residual from the pinned
// TerminalQueryResponder. Raw bytes preserve UTF-8 log data without decoding it.
pub(super) struct QueryResponder { residual: Vec<u8>, skip_cursor_handshake: bool }

impl QueryResponder {
    pub(super) fn new(skip_cursor_handshake: bool) -> Self { Self { residual: Vec::new(), skip_cursor_handshake } }

    pub(super) fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let mut buffer = std::mem::take(&mut self.residual);
        buffer.extend_from_slice(chunk);
        let mut replies = Vec::new();
        let mut index = 0;
        let mut last_end = 0;
        while index < buffer.len() {
            if buffer[index] != 0x1b { index += 1; continue; }
            let start = index;
            if buffer.get(index + 1) == Some(&b'[') {
                index += 2;
                let intermediate = match buffer.get(index) {
                    Some(b'?' | b'>' | b'=') => { let c = buffer[index]; index += 1; Some(c) }
                    _ => None,
                };
                let params = index;
                while buffer.get(index).is_some_and(|c| c.is_ascii_digit() || *c == b';') { index += 1; }
                if let Some(final_byte @ (b'n' | b'c')) = buffer.get(index).copied() {
                    let selector = buffer[params..index].split(|c| *c == b';').next().unwrap_or_default();
                    let reply: &[u8] = match (final_byte, intermediate, selector) {
                        (b'c', Some(b'>'), _) => b"\x1b[>0;10;1c",
                        (b'c', None, _) => b"\x1b[?1;2c",
                        (b'n', None, b"6") => {
                            if self.skip_cursor_handshake { self.skip_cursor_handshake = false; b"" } else { b"\x1b[1;1R" }
                        }
                        (b'n', None, b"5") => b"\x1b[0n",
                        _ => b"",
                    };
                    replies.extend_from_slice(reply);
                    index += 1;
                    last_end = index;
                    continue;
                }
            } else if buffer.get(index + 1) == Some(&b']') {
                for (prefix, reply) in [
                    (&b"\x1b]10;?"[..], &b"\x1b]10;rgb:ffff/ffff/ffff"[..]),
                    (&b"\x1b]11;?"[..], &b"\x1b]11;rgb:0000/0000/0000"[..]),
                ] {
                    if buffer[start..].starts_with(prefix) {
                        let end = start + prefix.len();
                        let terminator = if buffer.get(end) == Some(&7) { Some(&b"\x07"[..]) }
                            else if buffer.get(end..end + 2) == Some(&b"\x1b\\"[..]) { Some(&b"\x1b\\"[..]) } else { None };
                        if let Some(terminator) = terminator {
                            replies.extend_from_slice(reply);
                            replies.extend_from_slice(terminator);
                            index = end + terminator.len();
                            last_end = index;
                            break;
                        }
                    }
                }
                if index > start { continue; }
            }
            index = start + 1;
        }
        if let Some(mut tail) = buffer.iter().rposition(|c| *c == 0x1b) {
            // Preserve an OSC whose ST terminator is split BETWEEN ESC and
            // '\\'. The fixed source's lastIndexOf(ESC) alone drops its prefix;
            // retaining the bounded complete query prefix avoids a lost probe.
            if tail + 1 == buffer.len() {
                if let Some(start) = tail.checked_sub(6) {
                    if start >= last_end && matches!(&buffer[start..tail], b"\x1b]10;?" | b"\x1b]11;?") { tail = start; }
                }
            }
            if tail >= last_end && buffer.len() - tail <= 32 { self.residual.extend_from_slice(&buffer[tail..]); }
        }
        replies
    }
}

