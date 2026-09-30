//! `bash` tool (subset of OMP `tools/bash.ts`, `exec/bash-executor.ts`,
//! `exec/non-interactive-env.ts`, `session/streaming-output.ts`).
//!
//! Ported: `command`/`env`/`timeout`/`cwd` parameters, timeout default 300 s
//! clamped to 1..3600 with `0` disabling it, the non-interactive environment,
//! merged stdout/stderr, tail truncation at 50 KB with `[Showing lines X-Y of
//! Z]`, `(no output)`, `Command exited with code N` as an error result,
//! `[Command timed out after N seconds]` as an error result, cancellation as
//! an aborted error, live partial output updates, shared concurrency.
//!
//! Intentional differences: each call runs a fresh `bash -c` instead of OMP's
//! persistent embedded brush shell, so shell state such as `cd` or variables
//! does not carry across calls. The call's process tree is contained — a Unix
//! process group, or on Windows a `KILL_ON_JOB_CLOSE` Job Object that Bash is
//! spawned suspended into and resumed only after assignment succeeds (a
//! failed assignment kills it unrun) — and the whole tree is killed when the
//! call ends (exit, timeout, cancel, or the call being dropped), so no
//! background process outlives the call with unknown effects (long-running
//! services need a host job facility, not ported).
//! stdout and stderr share one pipe so their order is preserved, and only a
//! bounded tail of the output is kept in memory.
//!
//! Not ported (open): PTY mode, async/background jobs and auto-backgrounding,
//! artifact spill of full output, interceptors/rewrites, ACP terminals,
//! direnv, Windows shells, per-line column caps, middle elision.

use crate::{DEFAULT_MAX_BYTES, ToolContext};
use ara_agent::{AgentTool, ToolError, ToolOutput, UpdateFn};
use ara_ai::{JsonObject, Tool};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::io::Read;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
#[cfg(windows)]
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
pub const MIN_TIMEOUT_SECS: u64 = 1;
pub const MAX_TIMEOUT_SECS: u64 = 3600;
const UPDATE_INTERVAL: Duration = Duration::from_millis(250);

/// OMP `NON_INTERACTIVE_ENV` (SSH_ASKPASS points at `false`).
pub const NON_INTERACTIVE_ENV: &[(&str, &str)] = &[
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("MANPAGER", "cat"),
    ("SYSTEMD_PAGER", "cat"),
    ("BAT_PAGER", "cat"),
    ("DELTA_PAGER", "cat"),
    ("GH_PAGER", "cat"),
    ("GLAB_PAGER", "cat"),
    ("PSQL_PAGER", "cat"),
    ("MYSQL_PAGER", "cat"),
    ("AWS_PAGER", ""),
    ("HOMEBREW_PAGER", "cat"),
    ("LESS", "FRX"),
    ("TERM", "dumb"),
    ("NO_COLOR", "1"),
    ("PYTHONUNBUFFERED", "1"),
    ("GIT_EDITOR", "true"),
    ("VISUAL", "true"),
    ("EDITOR", "true"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("SSH_ASKPASS", "false"),
    ("CI", "true"),
    ("AGENT", "1"),
    ("npm_config_yes", "true"),
    ("npm_config_update_notifier", "false"),
    ("npm_config_fund", "false"),
    ("npm_config_audit", "false"),
    ("npm_config_progress", "false"),
    ("PNPM_DISABLE_SELF_UPDATE_CHECK", "true"),
    ("PNPM_UPDATE_NOTIFIER", "false"),
    ("YARN_ENABLE_TELEMETRY", "0"),
    ("YARN_ENABLE_PROGRESS_BARS", "0"),
    ("CARGO_TERM_PROGRESS_WHEN", "never"),
    ("DEBIAN_FRONTEND", "noninteractive"),
    ("PIP_NO_INPUT", "1"),
    ("PIP_DISABLE_PIP_VERSION_CHECK", "1"),
    ("TF_INPUT", "0"),
    ("TF_IN_AUTOMATION", "1"),
    ("GH_PROMPT_DISABLED", "1"),
    ("COMPOSER_NO_INTERACTION", "1"),
    ("CLOUDSDK_CORE_DISABLE_PROMPTS", "1"),
];

/// Resolve the effective timeout: `None` disables it.
pub fn resolve_timeout(requested: Option<f64>) -> Option<u64> {
    match requested {
        Some(0.0) => None,
        Some(t) if t.is_finite() => {
            Some((t.round() as i64).clamp(MIN_TIMEOUT_SECS as i64, MAX_TIMEOUT_SECS as i64) as u64)
        }
        _ => Some(DEFAULT_TIMEOUT_SECS),
    }
}

/// Bounded output accumulator: keeps a rolling tail plus exact totals.
#[derive(Default)]
pub struct OutputSink {
    tail: String,
    total_bytes: u64,
    total_newlines: u64,
    ends_with_newline: bool,
    decoder: Utf8Decoder,
}

impl OutputSink {
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let text = self.decoder.decode(bytes);
        self.push_str(&text);
    }

    fn push_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.total_bytes += text.len() as u64;
        self.total_newlines += text.bytes().filter(|b| *b == b'\n').count() as u64;
        self.ends_with_newline = text.ends_with('\n');
        self.tail.push_str(text);
        let keep = 2 * DEFAULT_MAX_BYTES;
        if self.tail.len() > 2 * keep {
            let mut cut = self.tail.len() - keep;
            while !self.tail.is_char_boundary(cut) {
                cut += 1;
            }
            self.tail.drain(..cut);
        }
    }

    pub fn finish(&mut self) {
        let rest = self.decoder.flush();
        self.push_str(&rest);
    }

    fn total_lines(&self) -> u64 {
        if self.total_bytes == 0 { 0 } else { self.total_newlines + u64::from(!self.ends_with_newline) }
    }

    /// Last `max_bytes` on a line boundary plus the `[Showing lines X-Y of Z]`
    /// notice when truncated.
    pub fn render(&self, max_bytes: usize) -> (String, Option<String>) {
        if self.total_bytes as usize <= max_bytes {
            return (self.tail.clone(), None);
        }
        let s = &self.tail;
        let mut start = s.len().saturating_sub(max_bytes);
        while !s.is_char_boundary(start) {
            start += 1;
        }
        if let Some(nl) = s[start..].find('\n')
            && start + nl + 1 < s.len()
        {
            start += nl + 1;
        }
        let kept = &s[start..];
        let total = self.total_lines();
        let kept_lines = kept.trim_end_matches('\n').split('\n').count() as u64;
        let first = total.saturating_sub(kept_lines) + 1;
        (kept.to_string(), Some(format!("[Showing lines {first}-{total} of {total}]")))
    }
}

/// Incremental UTF-8 decoder: invalid bytes become U+FFFD, a sequence split
/// across reads is held until complete.
#[derive(Default)]
pub struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut out = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(s) => {
                    out.push_str(s);
                    self.pending.clear();
                    return out;
                }
                Err(e) => {
                    let valid = e.valid_up_to();
                    out.push_str(std::str::from_utf8(&self.pending[..valid]).unwrap());
                    match e.error_len() {
                        Some(bad) => {
                            out.push('\u{FFFD}');
                            self.pending.drain(..valid + bad);
                        }
                        None => {
                            self.pending.drain(..valid);
                            return out;
                        }
                    }
                }
            }
        }
    }

    pub fn flush(&mut self) -> String {
        let rest = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        rest
    }
}

/// Convenience for one-shot strings (tests).
pub fn tail_truncate(output: &str, max_bytes: usize) -> (String, Option<String>) {
    let mut sink = OutputSink::default();
    sink.push_str(output);
    sink.render(max_bytes)
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // Negative pid targets the whole process group created with process_group(0).
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

/// Kills the call's process tree when dropped (normal end, error, or the
/// execute future being dropped by the host).
struct GroupGuard {
    #[cfg(unix)]
    pid: Option<u32>,
    #[cfg(windows)]
    job: Option<WindowsJob>,
}

#[cfg(windows)]
impl GroupGuard {
    fn kill_windows(&mut self) {
        // KILL_ON_JOB_CLOSE terminates Bash and every descendant in the Job.
        self.job.take();
    }
}

#[cfg(windows)]
struct WindowsJob(OwnedHandle);

#[cfg(windows)]
impl WindowsJob {
    fn new() -> io::Result<Self> {
        // This handle is not inherited. Closing it ends every process in the
        // call's Job, including when the execute future is dropped.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    fn assign_and_resume(&self, child: &tokio::process::Child) -> io::Result<()> {
        let process = child.raw_handle().ok_or_else(|| io::Error::other("suspended Bash has no process handle"))?;
        let pid = child.id().ok_or_else(|| io::Error::other("suspended Bash has no process ID"))?;
        if unsafe { AssignProcessToJobObject(self.0.as_raw_handle(), process) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // std/Tokio closes CreateProcess's primary-thread handle. A suspended
        // process has only that thread; identify it before it can run.
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        if unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut thread_id = None;
        loop {
            if entry.th32OwnerProcessID == pid && thread_id.replace(entry.th32ThreadID).is_some() {
                return Err(io::Error::other("suspended Bash has multiple initial threads"));
            }
            if unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) } == 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
                    return Err(error);
                }
                break;
            }
        }
        let thread_id = thread_id.ok_or_else(|| io::Error::other("suspended Bash has no initial thread"))?;
        let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
        let prior = unsafe { ResumeThread(thread.as_raw_handle()) };
        if prior != 1 {
            return Err(if prior == u32::MAX {
                io::Error::last_os_error()
            } else {
                io::Error::other(format!("unexpected Bash initial-thread suspend count: {prior}"))
            });
        }
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod windows_job_tests {
    use super::*;
    use windows_sys::Win32::System::JobObjects::JOB_OBJECT_LIMIT_ACTIVE_PROCESS;

    #[tokio::test]
    async fn failed_job_assignment_never_runs_suspended_bash() {
        let dir = tempfile::tempdir().unwrap();
        let job = WindowsJob::new().unwrap();
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    job.0.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            },
            0
        );
        let system_root = std::env::var_os("SystemRoot").expect("Windows SystemRoot");
        let mut first = tokio::process::Command::new(std::path::PathBuf::from(system_root).join("System32/ping.exe"));
        first.args(["-n", "10", "127.0.0.1"]).stdout(Stdio::null()).creation_flags(CREATE_SUSPENDED).kill_on_drop(true);
        let mut first = first.spawn().unwrap();
        job.assign_and_resume(&first).unwrap();
        assert!(first.try_wait().unwrap().is_none(), "first Job process must still occupy the slot");

        let mut command = tokio::process::Command::new("bash");
        command.arg("-c").arg("touch uncontained").current_dir(dir.path()).creation_flags(CREATE_SUSPENDED);
        command.kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        assert!(job.assign_and_resume(&child).is_err());
        // A wrongly resumed `touch` exits and creates the marker well within
        // this window; a contained failure leaves Bash suspended.
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(child.try_wait().unwrap().is_none(), "Bash must stay suspended after failed assignment");
        assert!(!dir.path().join("uncontained").exists(), "Bash ran without its Job");
        let _ = child.start_kill();
        tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("suspended child reaped before timeout")
            .unwrap();
        drop(job);
        tokio::time::timeout(Duration::from_secs(2), first.wait())
            .await
            .expect("Job child reaped before timeout")
            .unwrap();
        assert!(!dir.path().join("uncontained").exists());
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        kill_group(self.pid);
        #[cfg(windows)]
        self.kill_windows();
    }
}

/// Typed process facts (OMP `exec/bash-executor.ts` `BashResult`). Counts
/// describe decoded UTF-8 output, including a final empty newline segment;
/// display notices are not included. This fresh process runner does not report
/// a final shell directory because it does not track persistent shell state.
#[derive(Clone, Debug)]
pub struct BashResult {
    pub output: String,
    pub exit_code: Option<i32>,
    pub cancelled: bool,
    pub timed_out: bool,
    pub truncated: bool,
    pub total_lines: u64,
    pub total_bytes: u64,
    pub output_lines: u64,
    pub output_bytes: u64,
    pub working_dir: Option<String>,
    wall_time_ms: u64,
    timeout_seconds: Option<u64>,
    truncation_notice: Option<String>,
}

/// A live typed snapshot. Until settlement its exit code is unknown; callers
/// may display output without parsing model-facing `ToolOutput` strings.
pub type BashUpdateFn = Arc<dyn Fn(BashResult) + Send + Sync>;

impl OutputSink {
    fn snapshot(&self) -> BashResult {
        let (output, truncation_notice) = self.render(DEFAULT_MAX_BYTES);
        let output_bytes = output.len() as u64;
        let output_lines =
            if output.is_empty() { 0 } else { output.bytes().filter(|&b| b == b'\n').count() as u64 + 1 };
        BashResult {
            output,
            exit_code: None,
            cancelled: false,
            timed_out: false,
            truncated: truncation_notice.is_some(),
            total_lines: if self.total_bytes == 0 { 0 } else { self.total_newlines + 1 },
            total_bytes: self.total_bytes,
            output_lines,
            output_bytes,
            working_dir: None,
            wall_time_ms: 0,
            timeout_seconds: None,
            truncation_notice,
        }
    }
}

impl BashResult {
    /// Fixed `OutputSink.dump(notice)` prefixes terminal status for native
    /// user-shell receipts. Output counters continue to describe captured
    /// process bytes/lines, excluding this status line.
    pub fn output_with_status_notice(&self) -> String {
        if self.timed_out {
            let notice = self.timeout_seconds.map_or_else(
                || "Command timed out".to_owned(),
                |seconds| format!("Command timed out after {seconds} seconds"),
            );
            format!("[{notice}]\n{}", self.output)
        } else if self.cancelled {
            format!("[Command cancelled]\n{}", self.output)
        } else {
            self.output.clone()
        }
    }

    fn tool_text(&self) -> String {
        let body = self.output.trim_end_matches('\n');
        let mut text = if body.is_empty() { "(no output)".to_owned() } else { body.to_owned() };
        if let Some(notice) = &self.truncation_notice {
            text.push_str("\n\n");
            text.push_str(notice);
        }
        text
    }

    fn into_tool_output(self) -> Result<ToolOutput, ToolError> {
        let mut text = self.tool_text();
        let mut details = json!({"wallTimeMs": self.wall_time_ms});
        match self.timeout_seconds {
            Some(seconds) => details["timeoutSeconds"] = json!(seconds),
            None => details["timeoutDisabled"] = json!(true),
        }
        if self.truncated {
            details["truncation"] = json!({"truncated": true, "direction": "tail", "totalBytes": self.total_bytes});
        }
        // Timeout is also a cancellation in the typed protocol; retain the
        // Bash tool's established timeout error before handling user aborts.
        if self.timed_out {
            details["timedOut"] = json!(true);
            text.push_str(&format!(
                "\n\n[Command timed out after {} seconds]",
                self.timeout_seconds.unwrap_or_default()
            ));
            return Ok(ToolOutput::error(text).with_details(details));
        }
        if self.cancelled {
            return Err(ToolError(if self.total_bytes == 0 {
                "Command aborted".into()
            } else {
                format!("{text}\n\n[Command aborted]")
            }));
        }
        match self.exit_code {
            Some(0) => Ok(ToolOutput::text(text).with_details(details)),
            Some(code) => {
                details["exitCode"] = json!(code);
                text.push_str(&format!("\n\nCommand exited with code {code}"));
                Ok(ToolOutput::error(text).with_details(details))
            }
            None => Err(ToolError(format!("{text}\n\nCommand failed: missing exit status"))),
        }
    }
}

pub struct BashTool {
    pub ctx: ToolContext,
}

enum Ending {
    Exited(Option<std::process::ExitStatus>),
    TimedOut,
    Cancelled,
}

#[async_trait]
impl AgentTool for BashTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "bash".into(),
            description: format!(
                "Runs a shell command with `bash -c` in a fresh non-interactive process. Use it for one binary or a short pipeline that computes a fact. Set `cwd` instead of `cd` (shell state does not persist between calls); pass `env` for extra variables. `timeout` is in seconds (default {DEFAULT_TIMEOUT_SECS}; 0 disables the deadline; other values are clamped to {MIN_TIMEOUT_SECS}-{MAX_TIMEOUT_SECS}). Output is stdout and stderr merged, truncated to the last 50KB. A non-zero exit is reported as an error with its code."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "command to execute"},
                    "env": {"type": "object", "additionalProperties": {"type": "string"}, "description": "extra env vars"},
                    "timeout": {"type": "number", "description": "timeout in seconds; 0 disables the command deadline"},
                    "cwd": {"type": "string", "description": "working directory"}
                },
                "required": ["command"],
                "additionalProperties": false
            }),
        }
    }

    async fn execute(
        &self,
        _id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        let typed_update: BashUpdateFn = Arc::new(move |partial| update(ToolOutput::text(partial.output)));
        execute_bash(&self.ctx, &args, cancel, typed_update).await?.into_tool_output()
    }
}

/// Execute one contained process and return typed settlement and output facts.
/// Host callers need not infer process status from model-facing tool text.
/// Output is the captured tail only; no display/error sentinel is appended.
pub async fn execute_bash(
    ctx: &ToolContext,
    args: &JsonObject,
    cancel: CancellationToken,
    update: BashUpdateFn,
) -> Result<BashResult, ToolError> {
    let timeout = resolve_timeout(args.get("timeout").and_then(Value::as_f64));
    if cancel.is_cancelled() {
        let mut result = OutputSink::default().snapshot();
        result.cancelled = true;
        result.timeout_seconds = timeout;
        return Ok(result);
    }
    // `skill://` URLs in the command, env values and cwd resolve to paths
    // (OMP `expandInternalUrls`).
    let skills = ctx.skills.read().unwrap_or_else(|e| e.into_inner()).clone();
    let expand = |text: &str, no_escape: bool| crate::internal_urls::expand_skill_urls(text, &skills, no_escape);
    let command = expand(args.get("command").and_then(Value::as_str).unwrap_or_default(), false);
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .map(|c| ctx.resolve(&expand(c, true)))
        .unwrap_or_else(|| ctx.cwd.clone());
    if !cwd.is_dir() {
        return Err(ToolError(format!("Working directory does not exist: {}", ctx.display(&cwd))));
    }
    let (reader, writer) = std::io::pipe().map_err(|e| ToolError(format!("Failed to create pipe: {e}")))?;
    let writer_err = writer.try_clone().map_err(|e| ToolError(format!("Failed to create pipe: {e}")))?;
    let mut cmd = tokio::process::Command::new("bash");
    cmd.arg("-c").arg(&command).current_dir(&cwd).stdin(Stdio::null()).stdout(writer).stderr(writer_err);
    for (k, v) in NON_INTERACTIVE_ENV {
        cmd.env(k, v);
    }
    if let Some(Value::Object(env)) = args.get("env") {
        for (k, v) in env {
            if let Some(v) = v.as_str() {
                cmd.env(k, expand(v, true));
            }
        }
    }
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_SUSPENDED);
    cmd.kill_on_drop(true);
    #[cfg(windows)]
    let job = WindowsJob::new().map_err(|e| ToolError(format!("Failed to create Bash process job: {e}")))?;
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| ToolError(format!("Failed to start bash: {e}")))?;
    drop(cmd); // release the parent's copies of the pipe's write end
    #[cfg(windows)]
    if let Err(e) = job.assign_and_resume(&child) {
        // The initial thread has not run on every ordinary setup failure.
        // Never execute this command without its Job as a fallback.
        let _ = child.start_kill();
        return Err(ToolError(format!("Failed to contain Bash process tree: {e}")));
    }
    // Unix only needs the guard's Drop; Windows also kills through it below.
    #[cfg_attr(not(windows), allow(unused_mut, unused_variables))]
    let mut group = GroupGuard {
        #[cfg(unix)]
        pid: child.id(),
        #[cfg(windows)]
        job: Some(job),
    };
    #[cfg(unix)]
    let pid = child.id();
    let sink = Arc::new(Mutex::new(OutputSink::default()));
    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    let reader_sink = sink.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 16 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => reader_sink.lock().unwrap().push_bytes(&buf[..n]),
            }
        }
        let _ = done_tx.send(());
    });

    let deadline = timeout.map(|t| started + Duration::from_secs(t));
    let mut ticker = tokio::time::interval(UPDATE_INTERVAL);
    ticker.tick().await;
    let mut last_sent = 0u64;
    let ending = loop {
        tokio::select! {
            status = child.wait() => break Ending::Exited(status.ok()),
            _ = cancel.cancelled() => break Ending::Cancelled,
            _ = async { match deadline { Some(d) => tokio::time::sleep_until(d.into()).await, None => std::future::pending().await } } => break Ending::TimedOut,
            _ = ticker.tick() => {
                let snapshot = sink.lock().unwrap().snapshot();
                if snapshot.total_bytes != last_sent {
                    last_sent = snapshot.total_bytes;
                    update(snapshot);
                }
            }
        }
    };
    // End of call: the whole group goes (including background children
    // still holding the pipe), then the reader sees EOF.
    #[cfg(unix)]
    kill_group(pid);
    #[cfg(windows)]
    group.kill_windows();
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(2), done_rx).await;
    let mut result = {
        let mut sink = sink.lock().unwrap();
        sink.finish();
        sink.snapshot()
    };
    result.wall_time_ms = started.elapsed().as_millis() as u64;
    result.timeout_seconds = timeout;
    let exit_code = match &ending {
        Ending::Exited(Some(status)) => {
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                // OMP maps a signal kill without an exit code to 128 + signal (137 for SIGKILL).
                status.code().or_else(|| status.signal().map(|s| 128 + s))
            }
            #[cfg(not(unix))]
            status.code()
        }
        _ => None,
    };
    result.exit_code = exit_code;
    result.timed_out = matches!(ending, Ending::TimedOut);
    // Fixed OMP represents deadline cancellation as cancelled + timedOut.
    result.cancelled = matches!(ending, Ending::Cancelled | Ending::TimedOut);
    if matches!(ending, Ending::Exited(_)) && exit_code.is_none() {
        return Err(ToolError(format!("{}\n\nCommand failed: missing exit status", result.tool_text())));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process_args(value: Value) -> JsonObject {
        value.as_object().unwrap().clone()
    }

    fn no_process_update() -> BashUpdateFn {
        Arc::new(|_| {})
    }

    #[tokio::test]
    async fn typed_process_result_preserves_output_and_distinguishes_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let output = execute_bash(
            &ctx,
            &process_args(json!({"command":"printf 'é😀\\nsecond\\n'"})),
            CancellationToken::new(),
            no_process_update(),
        )
        .await
        .unwrap();
        assert_eq!(output.output, "é😀\nsecond\n");
        assert_eq!(output.exit_code, Some(0));
        assert!(!output.cancelled && !output.timed_out && !output.truncated);
        assert_eq!((output.total_lines, output.output_lines), (3, 3));
        assert_eq!((output.total_bytes, output.output_bytes), (14, 14));
        assert!(output.working_dir.is_none());
        let failed = execute_bash(
            &ctx,
            &process_args(json!({"command":"printf 'nope\\n'; exit 3"})),
            CancellationToken::new(),
            no_process_update(),
        )
        .await
        .unwrap();
        assert_eq!(failed.output, "nope\n");
        assert_eq!(failed.exit_code, Some(3));
        assert!(!failed.cancelled && !failed.timed_out);
        assert!(!failed.output.contains("Command exited"));
    }

    #[tokio::test]
    async fn typed_process_result_reports_exact_totals_for_a_bounded_tail() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let output = execute_bash(
            &ctx,
            &process_args(json!({"command":"seq 1 20000"})),
            CancellationToken::new(),
            no_process_update(),
        )
        .await
        .unwrap();
        let expected_total: usize = (1..=20000).map(|line| format!("{line}\n").len()).sum();
        assert_eq!(output.total_bytes, expected_total as u64);
        assert_eq!(output.total_lines, 20001);
        assert!(output.truncated);
        assert_eq!(output.output_bytes, output.output.len() as u64);
        assert_eq!(output.output_lines, output.output.matches('\n').count() as u64 + 1);
        assert!(output.output.len() <= DEFAULT_MAX_BYTES);
        assert!(output.output.ends_with("20000\n"));
        assert!(!output.output.contains("Showing lines"));
    }

    #[tokio::test]
    async fn typed_process_cancel_before_dispatch_and_on_live_update() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let cancel = CancellationToken::new();
        cancel.cancel();
        let output =
            execute_bash(&ctx, &process_args(json!({"command":"touch pre-cancelled"})), cancel, no_process_update())
                .await
                .unwrap();
        assert!(output.cancelled && !output.timed_out);
        assert_eq!(output.exit_code, None);
        assert_eq!(output.output, "");
        assert_eq!(output.output_with_status_notice(), "[Command cancelled]\n");
        assert_eq!((output.total_lines, output.total_bytes, output.output_lines, output.output_bytes), (0, 0, 0, 0));
        assert!(!dir.path().join("pre-cancelled").exists());
        let cancel = CancellationToken::new();
        let cancel_on_output = cancel.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let updates = seen.clone();
        let update: BashUpdateFn = Arc::new(move |partial| {
            if partial.output.contains("started") {
                updates.lock().unwrap().push(partial);
                cancel_on_output.cancel();
            }
        });
        let result = tokio::time::timeout(
            Duration::from_secs(4),
            execute_bash(
                &ctx,
                &process_args(json!({"command":"echo started; sleep 3; touch after-cancel"})),
                cancel,
                update,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.output, "started\n");
        assert_eq!(result.output_with_status_notice(), "[Command cancelled]\nstarted\n");
        assert!(result.cancelled && !result.timed_out);
        assert!(result.exit_code.is_none());
        let partial = seen.lock().unwrap();
        assert!(!partial.is_empty());
        assert_eq!(partial[0].total_bytes, 8);
        assert!(!partial[0].cancelled);
        assert!(!dir.path().join("after-cancel").exists());
    }

    #[tokio::test]
    async fn typed_process_timeout_is_a_timed_cancellation_without_output_sentinel() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = ToolContext::new(dir.path());
        let result = tokio::time::timeout(
            Duration::from_secs(4),
            execute_bash(
                &ctx,
                &process_args(json!({"command":"(sleep 2; touch after-timeout) & echo ready; wait","timeout":1})),
                CancellationToken::new(),
                no_process_update(),
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.output, "ready\n");
        assert_eq!(result.output_with_status_notice(), "[Command timed out after 1 seconds]\nready\n");
        assert!(result.cancelled && result.timed_out);
        assert!(result.exit_code.is_none());
        assert_eq!((result.total_lines, result.output_lines), (2, 2));
        assert_eq!((result.total_bytes, result.output_bytes), (6, 6));
        tokio::time::sleep(Duration::from_millis(1400)).await;
        assert!(!dir.path().join("after-timeout").exists(), "contained child did not survive timeout");
    }

    #[test]
    fn timeout_resolution() {
        assert_eq!(resolve_timeout(None), Some(300));
        assert_eq!(resolve_timeout(Some(0.0)), None);
        assert_eq!(resolve_timeout(Some(0.2)), Some(1));
        assert_eq!(resolve_timeout(Some(99999.0)), Some(3600));
        assert_eq!(resolve_timeout(Some(12.0)), Some(12));
    }

    #[test]
    fn utf8_decoder_keeps_split_sequences_after_invalid_bytes() {
        let mut d = Utf8Decoder::default();
        let euro = "€".as_bytes();
        let mut first = vec![0xFF];
        first.extend_from_slice(&euro[..2]);
        assert_eq!(d.decode(&first), "\u{FFFD}");
        assert_eq!(d.decode(&euro[2..]), "€");
        assert_eq!(d.decode(&[0xE2]), "");
        assert_eq!(d.flush(), "\u{FFFD}");
    }

    #[test]
    fn sink_is_bounded_but_counts_everything() {
        let mut s = OutputSink::default();
        let line = "0123456789\n".repeat(1000);
        for _ in 0..200 {
            s.push_bytes(line.as_bytes());
        }
        assert!(s.tail.len() <= 4 * DEFAULT_MAX_BYTES + line.len());
        let (kept, notice) = s.render(DEFAULT_MAX_BYTES);
        assert!(kept.len() <= DEFAULT_MAX_BYTES);
        assert!(notice.unwrap().ends_with("-200000 of 200000]"));
    }

    #[test]
    fn tail_truncation_keeps_whole_lines() {
        let text: String = (1..=100).map(|i| format!("line {i}\n")).collect();
        let (kept, notice) = tail_truncate(&text, 40);
        assert!(kept.starts_with("line "));
        assert!(kept.ends_with("line 100\n"));
        let n = notice.unwrap();
        assert!(n.starts_with("[Showing lines ") && n.ends_with("-100 of 100]"), "{n}");
        assert_eq!(tail_truncate("small", 40), ("small".to_string(), None));
    }
}
