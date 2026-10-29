// Ported from Oh My Pi 596f2da7101178214aa27a753529d15e6b7ad91d, MIT.
// Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük; 2026 Stencil Labs, Inc.
// The upstream MIT permission and disclaimer are reproduced in types.rs.

use super::{
    config::ServerConfig,
    types::{Diagnostic, FormatResult, absolute, cancellation, cancelled, check_cancel, error},
};
use serde_json::{Value, json};
use std::{
    io,
    ops::{Deref, DerefMut},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
};
use tokio_util::sync::CancellationToken;

pub(super) fn command(program: &Path) -> Command {
    #[cfg(windows)]
    let mut command = {
        let ext = program.extension().and_then(|v| v.to_str()).unwrap_or_default();
        if ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat") {
            let mut cmd = Command::new(std::env::var_os("COMSPEC").unwrap_or_else(|| "cmd.exe".into()));
            cmd.args(["/d", "/s", "/c"]);
            cmd.arg(program);
            cmd
        } else {
            Command::new(program)
        }
    };
    #[cfg(not(windows))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    command.kill_on_drop(true);
    command
}

pub(crate) struct OwnedProcess {
    child: Child,
    #[cfg(windows)]
    job: WindowsJob,
    #[cfg(unix)]
    group: u32,
}
impl Deref for OwnedProcess {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.child
    }
}
impl DerefMut for OwnedProcess {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.child
    }
}
#[cfg(unix)]
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if self.group > 0 {
            unsafe {
                libc::kill(-(self.group as i32), libc::SIGKILL);
            }
        }
    }
}
pub(crate) fn spawn(mut command: Command) -> io::Result<OwnedProcess> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        command.creation_flags(0x08000000 | CREATE_SUSPENDED);
        let job = WindowsJob::new()?;
        let mut child = command.spawn()?;
        if let Err(error) = job.assign_and_resume(&child) {
            let _ = child.start_kill();
            return Err(error);
        }
        Ok(OwnedProcess { child, job })
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
        let child = command.spawn()?;
        let group = child.id().ok_or_else(|| error("child has no PID"))?;
        Ok(OwnedProcess { child, group })
    }
}
pub(crate) async fn terminate(child: &mut OwnedProcess) -> io::Result<()> {
    #[cfg(windows)]
    child.job.terminate()?;
    #[cfg(unix)]
    {
        if (unsafe { libc::kill(-(child.group as i32), libc::SIGKILL) }) != 0 {
            let observed = io::Error::last_os_error();
            if observed.raw_os_error() != Some(libc::ESRCH) {
                return Err(observed);
            }
        }
    }
    let _ = child.start_kill();
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LSP child termination was not confirmed"))??;
    #[cfg(windows)]
    tokio::time::timeout(Duration::from_secs(3), child.job.wait_empty())
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LSP child tree termination was not confirmed"))??;
    #[cfg(unix)]
    tokio::time::timeout(Duration::from_secs(3), wait_group_empty(child.group))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "LSP process group termination remains unknown"))??;
    Ok(())
}

#[cfg(unix)]
async fn wait_group_empty(group: u32) -> io::Result<()> {
    loop {
        // Reaping the original child proves only that child's exit. ESRCH is
        // the observation that the owned process group no longer exists.
        if (unsafe { libc::kill(-(group as i32), 0) }) != 0 {
            let observed = io::Error::last_os_error();
            if observed.raw_os_error() == Some(libc::ESRCH) {
                return Ok(());
            }
            return Err(observed);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(windows)]
struct WindowsJob(std::os::windows::io::OwnedHandle);
#[cfg(windows)]
impl WindowsJob {
    fn new() -> io::Result<Self> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::System::JobObjects::*;
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
    fn assign_and_resume(&self, child: &Child) -> io::Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
            System::{
                Diagnostics::ToolHelp::*,
                JobObjects::AssignProcessToJobObject,
                Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
            },
        };
        let process = child.raw_handle().ok_or_else(|| error("suspended LSP has no process handle"))?;
        let pid = child.id().ok_or_else(|| error("suspended LSP has no PID"))?;
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut thread = None;
        loop {
            if entry.th32OwnerProcessID == pid && thread.replace(entry.th32ThreadID).is_some() {
                return Err(error("suspended LSP has multiple initial threads"));
            }
            if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                    return Err(error);
                }
                break;
            }
        }
        let raw = unsafe {
            OpenThread(THREAD_SUSPEND_RESUME, 0, thread.ok_or_else(|| error("suspended LSP has no initial thread"))?)
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
        if unsafe { ResumeThread(thread.as_raw_handle()) } == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    fn terminate(&self) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    async fn wait_empty(&self) -> io::Result<()> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::*;
        loop {
            let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            if unsafe {
                QueryInformationJobObject(
                    self.0.as_raw_handle(),
                    JobObjectBasicAccountingInformation,
                    (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&accounting) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            if accounting.ActiveProcesses == 0 {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

pub(super) async fn run(
    program: &Path,
    args: &[String],
    cwd: &Path,
    cancel: Option<&CancellationToken>,
) -> io::Result<(String, String, bool)> {
    check_cancel(cancel)?;
    let mut child_command = command(program);
    child_command.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = spawn(child_command)?;
    let mut stdout = child.stdout.take().ok_or_else(|| error("missing CLI stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| error("missing CLI stderr"))?;
    // Reads run alongside wait; a full stderr pipe cannot wedge child exit.
    struct ReaderOwners {
        out: tokio::task::JoinHandle<io::Result<Vec<u8>>>,
        err: tokio::task::JoinHandle<io::Result<Vec<u8>>>,
    }
    impl Drop for ReaderOwners {
        fn drop(&mut self) {
            self.out.abort();
            self.err.abort();
        }
    }
    let mut readers = ReaderOwners {
        out: tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        }),
        err: tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        }),
    };
    let status = tokio::select! {
        status = child.wait() => status,
        _ = cancellation(cancel) => {
            let _ = terminate(&mut child).await;
            readers.out.abort();readers.err.abort();let _=(&mut readers.out).await;let _=(&mut readers.err).await;
            return Err(cancelled());
        }
    };
    let status = match status {
        Ok(status) => status,
        Err(error) => {
            readers.out.abort();
            readers.err.abort();
            let _ = (&mut readers.out).await;
            let _ = (&mut readers.err).await;
            return Err(error);
        }
    };
    // A successful parent may leave descendants holding its output handles.
    // Settle the owned tree before awaiting EOF; collected output is retained.
    terminate(&mut child).await?;
    let outputs = tokio::select! {
        outputs = async { Ok::<_, io::Error>((&mut readers.out).await.map_err(io::Error::other)??) } => {
            let stdout = outputs?;
            let stderr = tokio::select! { output = &mut readers.err => output.map_err(io::Error::other)??, _ = cancellation(cancel) => {let _=terminate(&mut child).await;readers.err.abort();let _=(&mut readers.err).await;return Err(cancelled());} };
            (stdout, stderr)
        },
        _ = cancellation(cancel) => {let _=terminate(&mut child).await;readers.out.abort();readers.err.abort();let _=(&mut readers.out).await;let _=(&mut readers.err).await;return Err(cancelled());},
    };
    check_cancel(cancel)?;
    Ok((String::from_utf8_lossy(&outputs.0).into(), String::from_utf8_lossy(&outputs.1).into(), status.success()))
}

pub(super) async fn format(
    server: &ServerConfig,
    path: &Path,
    content: &str,
    cwd: &Path,
    cancel: Option<&CancellationToken>,
) -> FormatResult {
    let mut result =
        FormatResult { content: content.into(), failed: false, unsupported: false, external_write_unknown: false };
    if check_cancel(cancel).is_err() {
        result.failed = true;
        return result;
    }
    if server.name == "swiftlint" {
        return result;
    }
    // Writethrough has committed the caller's bytes through its native fallback.
    // Avoid the standalone Biome client's redundant unmediated initial write.
    let args = vec!["format".into(), "--write".into(), path.to_string_lossy().into()];
    let run_result = run(&server.resolved_command, &args, cwd, cancel).await;
    result.failed = !matches!(&run_result, Ok((_, _, true)));
    // The external formatter can mutate before failing. Observe rather than replay.
    match tokio::fs::read_to_string(path).await {
        Ok(observed) => result.content = observed,
        Err(_) => {
            result.failed = true;
            result.external_write_unknown = true;
        }
    }
    result
}

pub(super) async fn lint(
    server: &ServerConfig,
    path: &Path,
    cwd: &Path,
    cancel: Option<&CancellationToken>,
) -> io::Result<Vec<Diagnostic>> {
    let args = if server.name == "biome" {
        vec!["lint", "--reporter=json"]
    } else {
        vec!["lint", "--quiet", "--reporter", "json"]
    };
    let mut args = args.into_iter().map(str::to_string).collect::<Vec<_>>();
    args.push(path.to_string_lossy().into());
    let (stdout, stderr, success) = run(&server.resolved_command, &args, cwd, cancel).await?;
    // Nonzero is expected when lint issues exist. Empty/malformed output is unknown.
    if stdout.trim().is_empty() && (!success || server.name == "swiftlint") {
        return Err(error(format!("{} lint failed: {}", server.name, stderr.chars().take(500).collect::<String>())));
    }
    let value: Value = serde_json::from_str(&stdout).map_err(io::Error::other)?;
    let mut diagnostics = Vec::new();
    if server.name == "biome" {
        let items = value
            .get("diagnostics")
            .and_then(Value::as_array)
            .ok_or_else(|| error("Biome JSON reporter lacks diagnostics"))?;
        let target = absolute(path);
        let mut usable = false;
        for item in items {
            let Some(raw_path) = item.pointer("/location/path").and_then(Value::as_str) else {
                continue;
            };
            usable = true;
            if absolute(&cwd.join(raw_path)) != target {
                continue;
            }
            let start = item.pointer("/location/start");
            let end = item.pointer("/location/end").or(start);
            let position = |value: Option<&Value>, field: &str, fallback: u64| {
                value.and_then(|v| v.get(field)).and_then(Value::as_u64).unwrap_or(fallback).saturating_sub(1)
            };
            let line = start.and_then(|v| v.get("line")).and_then(Value::as_u64).unwrap_or(1);
            let col = start.and_then(|v| v.get("column")).and_then(Value::as_u64).unwrap_or(1);
            let severity = match item["severity"].as_str() {
                Some("error") => 1,
                Some("info") => 3,
                Some("hint") => 4,
                _ => 2,
            };
            let diagnostic = json!({"range":{"start":{"line":position(start,"line",1),"character":position(start,"column",1)},"end":{"line":position(end,"line",line),"character":position(end,"column",col)}},"severity":severity,"message":item["message"],"source":"biome","code":item["category"]});
            diagnostics.push(serde_json::from_value(diagnostic).map_err(io::Error::other)?);
        }
        if !items.is_empty() && !usable {
            return Err(error("Biome JSON reporter schema has no usable location"));
        }
    } else {
        for item in value.as_array().ok_or_else(|| error("SwiftLint JSON reporter is not an array"))? {
            let line = item["line"].as_u64().unwrap_or(1).saturating_sub(1);
            let character = item["character"].as_u64().unwrap_or(1).saturating_sub(1);
            diagnostics.push(serde_json::from_value(json!({"range":{"start":{"line":line,"character":character},"end":{"line":line,"character":character}},"severity":if item["severity"] == "Error" {1} else {2},"message":item["reason"],"source":"swiftlint","code":item["rule_id"]})).map_err(io::Error::other)?);
        }
    }
    Ok(diagnostics)
}
