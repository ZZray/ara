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
//! Intentional differences: each call runs a fresh `bash -c` in its own
//! process group instead of OMP's persistent embedded brush shell, so shell
//! state such as `cd` or variables does not carry across calls; the whole
//! group is killed when the call ends (exit, timeout, cancel, or the call
//! being dropped), so no background process outlives the call with unknown
//! effects (long-running services need a host job facility, not ported).
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

/// Kills the call's process group when dropped (normal end, error, or the
/// execute future being dropped by the host).
struct GroupGuard(Option<u32>);

impl Drop for GroupGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        kill_group(self.0);
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
        // `skill://` URLs in the command, env values and cwd resolve to paths
        // (OMP `expandInternalUrls`).
        let skills = self.ctx.skills.read().unwrap_or_else(|e| e.into_inner()).clone();
        let expand = |text: &str, no_escape: bool| crate::internal_urls::expand_skill_urls(text, &skills, no_escape);
        let command = expand(args.get("command").and_then(Value::as_str).unwrap_or_default(), false);
        let timeout = resolve_timeout(args.get("timeout").and_then(Value::as_f64));
        let cwd = args
            .get("cwd")
            .and_then(Value::as_str)
            .map(|c| self.ctx.resolve(&expand(c, true)))
            .unwrap_or_else(|| self.ctx.cwd.clone());
        if !cwd.is_dir() {
            return Err(ToolError(format!("Working directory does not exist: {}", self.ctx.display(&cwd))));
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
        cmd.kill_on_drop(true);
        let started = Instant::now();
        let mut child = cmd.spawn().map_err(|e| ToolError(format!("Failed to start bash: {e}")))?;
        drop(cmd); // release the parent's copies of the pipe's write end
        let _group = GroupGuard(child.id());
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
                    let (bytes, rendered) = { let s = sink.lock().unwrap(); (s.total_bytes, s.render(DEFAULT_MAX_BYTES).0) };
                    if bytes != last_sent {
                        last_sent = bytes;
                        update(ToolOutput::text(rendered));
                    }
                }
            }
        };
        // End of call: the whole group goes (including background children
        // still holding the pipe), then the reader sees EOF.
        #[cfg(unix)]
        kill_group(pid);
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), done_rx).await;
        let (body, notice, total_bytes) = {
            let mut s = sink.lock().unwrap();
            s.finish();
            let (b, n) = s.render(DEFAULT_MAX_BYTES);
            (b, n, s.total_bytes)
        };
        let body = body.trim_end_matches('\n').to_string();
        let mut details = json!({"wallTimeMs": started.elapsed().as_millis() as u64});
        match timeout {
            Some(t) => details["timeoutSeconds"] = json!(t),
            None => details["timeoutDisabled"] = json!(true),
        }
        if notice.is_some() {
            details["truncation"] = json!({"truncated": true, "direction": "tail", "totalBytes": total_bytes});
        }
        let mut text = if body.is_empty() { "(no output)".to_string() } else { body };
        if let Some(n) = &notice {
            text.push_str("\n\n");
            text.push_str(n);
        }
        let exit_code = match &ending {
            Ending::Exited(Some(status)) => status.code().or_else(|| {
                // OMP maps a signal kill without an exit code to 128 + signal (137 for SIGKILL).
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal().map(|s| 128 + s)
                }
                #[cfg(not(unix))]
                None
            }),
            _ => None,
        };
        match ending {
            Ending::Exited(_) if exit_code == Some(0) => Ok(ToolOutput::text(text).with_details(details)),
            Ending::Exited(_) if exit_code.is_some() => {
                let code = exit_code.unwrap();
                details["exitCode"] = json!(code);
                text.push_str(&format!("\n\nCommand exited with code {code}"));
                Ok(ToolOutput::error(text).with_details(details))
            }
            Ending::Exited(_) => Err(ToolError(format!("{text}\n\nCommand failed: missing exit status"))),
            Ending::TimedOut => {
                details["timedOut"] = json!(true);
                let secs = timeout.unwrap_or_default();
                text.push_str(&format!("\n\n[Command timed out after {secs} seconds]"));
                Ok(ToolOutput::error(text).with_details(details))
            }
            Ending::Cancelled => Err(ToolError(if total_bytes == 0 {
                "Command aborted".into()
            } else {
                format!("{text}\n\n[Command aborted]")
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
