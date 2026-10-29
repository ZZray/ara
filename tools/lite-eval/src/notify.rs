//! DingTalk progress notices (requirement 38, module M-X-NOTIFY) through the
//! local script `Send-QSpinV3Notice.ps1`. The script owns the webhook (DPAPI
//! file) and computes the elapsed time; this module decides whether to send,
//! builds the argument array, classifies the outcome and appends one record
//! per decision to `notifications.jsonl`. Nothing is ever resent automatically.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::{
    common::{sha256_hex, timestamp_now, Credentials},
    ledger::{current, module, Entry, Phase, State, CHECKPOINTS},
};

pub const SCRIPT: &str = "D:/workspace/env/tools/codex-notify/Send-QSpinV3Notice.ps1";
pub const SENDER: &str = "Kiro / ARA Lite Core 能力提升 / edfef3ce";
/// Actual task start; the script derives the elapsed time from it.
pub const STARTED_AT: &str = "2026-10-26T16:01:31+08:00";
pub const NOTIFICATIONS_PATH: &str = "crates/ara-agent-lite-core/docs/evidence/lite-capability/notifications.jsonl";
/// Outer bound around the script's own 20-second network timeout.
pub const PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Milestone,
    Decision,
    Blocker,
}

impl Severity {
    fn as_arg(self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Milestone => "Milestone",
            Self::Decision => "Decision",
            Self::Blocker => "Blocker",
        }
    }
}

/// Content of one notice; sender and start time are fixed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Notice {
    pub severity: Severity,
    pub title: String,
    pub message: String,
    pub progress: String,
    pub overall_progress: String,
}

/// Script argument array (no command-line string is ever assembled).
pub fn arguments(script: &Path, notice: &Notice) -> Vec<OsString> {
    let pairs = [
        ("-Title", notice.title.as_str()),
        ("-Message", notice.message.as_str()),
        ("-Progress", notice.progress.as_str()),
        ("-OverallProgress", notice.overall_progress.as_str()),
        ("-StartedAt", STARTED_AT),
        ("-Severity", notice.severity.as_arg()),
        ("-Sender", SENDER),
    ];
    let mut args: Vec<OsString> = vec!["-NoProfile".into(), "-NonInteractive".into(), "-File".into(), script.into()];
    for (name, value) in pairs {
        args.push(name.into());
        args.push(value.into());
    }
    args
}

/// What running the script produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunOutcome {
    Exited {
        code: Option<i32>,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    TimedOut,
    /// No PowerShell could be started.
    SpawnFailed(String),
}

/// Runs PowerShell with an argument array; replaceable in tests.
pub trait Runner {
    fn run(&self, args: &[OsString], timeout: Duration) -> RunOutcome;
}

/// `pwsh`, falling back to `powershell.exe` when pwsh is not installed.
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, args: &[OsString], timeout: Duration) -> RunOutcome {
        let mut last = String::new();
        for program in ["pwsh", "powershell.exe"] {
            let spawned = Command::new(program)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            let mut child = match spawned {
                Ok(child) => child,
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    last = format!("{program} not found");
                    continue;
                }
                Err(error) => return RunOutcome::SpawnFailed(format!("{program}: {}", error.kind())),
            };
            let drain = |pipe: Option<Box<dyn Read + Send>>| {
                thread::spawn(move || {
                    let mut bytes = Vec::new();
                    if let Some(mut pipe) = pipe {
                        let _ = pipe.read_to_end(&mut bytes);
                    }
                    bytes
                })
            };
            let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
            let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as Box<dyn Read + Send>));
            let deadline = Instant::now() + timeout;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                    _ => break None,
                }
            };
            let Some(status) = status else {
                let _ = child.kill();
                let _ = child.wait();
                return RunOutcome::TimedOut;
            };
            return RunOutcome::Exited {
                code: status.code(),
                stdout: stdout.join().unwrap_or_default(),
                stderr: stderr.join().unwrap_or_default(),
            };
        }
        RunOutcome::SpawnFailed(last)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Delivered,
    Undelivered,
    Unconfirmed,
    NotSent,
}

/// One line of `notifications.jsonl`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationRecord {
    pub seq: u64,
    pub ledger_seq: Option<u64>,
    pub severity: Severity,
    pub state_digest: String,
    pub status: Status,
    pub errcode: Option<String>,
    pub sent_at: String,
    pub reason_category: Option<String>,
    pub content_sha256: String,
}

/// Value of `"key":"…"` or `"key":bare` in the script's receipt line. The
/// receipt may be in the console code page, so only ASCII bytes are relied on.
fn receipt_field(stdout: &[u8], key: &str) -> Option<String> {
    let needle = format!("\"{key}\":");
    let start = stdout.windows(needle.len()).rposition(|window| window == needle.as_bytes())? + needle.len();
    let rest = &stdout[start..];
    let value: Vec<u8> = if rest.first() == Some(&b'"') {
        rest[1..].iter().take_while(|byte| **byte != b'"').copied().collect()
    } else {
        rest.iter().take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'-').copied().collect()
    };
    String::from_utf8(value).ok().filter(|value| !value.is_empty())
}

/// Requirement 38.4–38.8: map the run to a status, errcode and reason category.
pub fn classify(outcome: &RunOutcome) -> (Status, Option<String>, Option<String>) {
    let reason = |text: &str| Some(text.to_owned());
    match outcome {
        RunOutcome::SpawnFailed(_) => (Status::NotSent, None, reason("launcher_unavailable")),
        RunOutcome::TimedOut => (Status::Unconfirmed, None, reason("timeout")),
        RunOutcome::Exited { code, stdout, stderr } => {
            let errcode = receipt_field(stdout, "errcode").filter(|code| code != "missing");
            let error = String::from_utf8_lossy(stderr);
            let contains = |needle: &str| error.contains(needle);
            if *code == Some(0) && errcode.as_deref() == Some("0") {
                (Status::Delivered, errcode, None)
            } else if let Some(errcode) = errcode.filter(|code| code != "0") {
                (Status::Undelivered, Some(errcode), reason("rejected"))
            } else if contains("Supply current Progress") {
                (Status::NotSent, None, reason("missing_metadata"))
            } else if contains("exceeds the local 16 KiB") {
                (Status::NotSent, None, reason("too_large"))
            } else if contains("Unexpected notification endpoint") {
                (Status::NotSent, None, reason("endpoint_rejected"))
            } else if contains("Import-Clixml") || contains("clixml") {
                (Status::NotSent, None, reason("credential_unreadable"))
            } else if contains("ParameterBinding") || contains("Cannot process argument") || contains("Cannot convert")
            {
                (Status::NotSent, None, reason("parameter_rejected"))
            } else if contains("could not be confirmed") {
                (Status::Unconfirmed, None, reason("network"))
            } else {
                (Status::Unconfirmed, None, reason("no_errcode"))
            }
        }
    }
}

/// Requirement 38.6: digest of every module's current state plus the set of
/// unresolved blockers. `topic` distinguishes an explicit Decision notice.
pub fn state_digest(entries: &[Entry], topic: Option<&str>) -> String {
    let mut text = String::new();
    for (module, entry) in current(entries) {
        text.push_str(&format!("{module}\t{:?}\t{}\n", entry.state, entry.criteria.done));
        if let Some(blocker) = &entry.blocker {
            text.push_str(&format!("blocker\t{module}\t{}\t{}\n", blocker.reason, blocker.decision_needed));
        }
    }
    if let Some(topic) = topic {
        text.push_str(&format!("topic\t{topic}\n"));
    }
    sha256_hex(text.as_bytes())
}

/// Requirement 38.1: a new blocker → Blocker; module completion → Milestone;
/// everything else → Info.
pub fn severity_for(entry: &Entry, previous: Option<&Entry>) -> Severity {
    let new_blocker = entry.blocker.is_some() && previous.is_none_or(|previous| previous.blocker != entry.blocker);
    if new_blocker {
        Severity::Blocker
    } else if matches!(entry.state, State::Done | State::Verified)
        && previous.is_none_or(|previous| previous.state != entry.state)
    {
        Severity::Milestone
    } else {
        Severity::Info
    }
}

/// Overall progress over the design checkpoints (requirement 38.2).
pub fn overall_progress(entries: &[Entry]) -> String {
    let states = current(entries);
    let checkpoint =
        |state: State| states.values().filter(|entry| entry.phase != Phase::Cross && entry.state == state).count();
    format!(
        "verified 检查点 {}/{CHECKPOINTS}；done {}；open {}；未开始 {}",
        checkpoint(State::Verified),
        checkpoint(State::Done),
        checkpoint(State::Open),
        CHECKPOINTS - states.values().filter(|entry| entry.phase != Phase::Cross).count()
    )
}

/// Notice for a just-appended ledger entry; `entries` includes it.
pub fn notice_for(entry: &Entry, previous: Option<&Entry>, entries: &[Entry]) -> Notice {
    let severity = severity_for(entry, previous);
    let (title, state) = match module(&entry.module) {
        Some(module) => (module.title, entry.state),
        None => ("未知模块", entry.state),
    };
    let state_text = match state {
        State::Open => "进行中",
        State::Done => "完成",
        State::Verified => "已验证",
    };
    let headline = match (&entry.blocker, severity) {
        (Some(blocker), Severity::Blocker) => format!("{} 阻断：{}", entry.module, first_line(&blocker.reason, 60)),
        _ => format!("{} {title}：{state_text}", entry.module),
    };
    let mut message = format!(
        "- **模块**：{}（{}）\n- **状态**：{state_text}，完成标准 {}/{}\n- **证据**：`{}`\n- **未覆盖**：{}\n",
        entry.module, title, entry.criteria.done, entry.criteria.total, entry.evidence, entry.uncovered
    );
    if let Some(blocker) = &entry.blocker {
        message.push_str(&format!("\n**阻断原因**：{}\n\n**已尝试**：\n", blocker.reason));
        for attempt in &blocker.attempts {
            message.push_str(&format!("- {} → {}\n", attempt.action, attempt.result));
        }
        message.push_str(&format!("\n**需要你决定**：{}（请回到 Kiro 会话答复）\n", blocker.decision_needed));
    }
    Notice {
        severity,
        title: headline,
        message,
        progress: format!("{title} {}/{}", entry.criteria.done, entry.criteria.total),
        overall_progress: overall_progress(entries),
    }
}

fn first_line(text: &str, limit: usize) -> String {
    text.lines().next().unwrap_or_default().chars().take(limit).collect()
}

/// Sends notices and keeps `notifications.jsonl`.
pub struct Notifier<R> {
    log: PathBuf,
    script: PathBuf,
    runner: R,
    credentials: Credentials,
}

impl Notifier<ProcessRunner> {
    /// The real script and log in the repository at `repo_root`.
    pub fn in_repo(repo_root: &Path, credentials: Credentials) -> Self {
        Self::new(repo_root.join(NOTIFICATIONS_PATH), PathBuf::from(SCRIPT), ProcessRunner, credentials)
    }
}

impl<R: Runner> Notifier<R> {
    pub fn new(log: PathBuf, script: PathBuf, runner: R, credentials: Credentials) -> Self {
        Self { log, script, runner, credentials }
    }

    pub fn records(&self) -> Result<Vec<NotificationRecord>> {
        let text = match fs::read_to_string(&self.log) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).with_context(|| format!("cannot read {}", self.log.display())),
        };
        text.lines()
            .map(|line| serde_json::from_str(line).with_context(|| format!("invalid record in {}", self.log.display())))
            .collect()
    }

    /// Whether a notice with `digest` would repeat an earlier one: the same
    /// digest as the latest record of any status, or as the latest delivered
    /// or unconfirmed record (requirements 38.6 and 38.9).
    fn repeats(records: &[NotificationRecord], digest: &str) -> bool {
        let last = records.last().map(|record| record.state_digest.as_str());
        let last_reached = records
            .iter()
            .rev()
            .find(|record| matches!(record.status, Status::Delivered | Status::Unconfirmed))
            .map(|record| record.state_digest.as_str());
        last == Some(digest) || last_reached == Some(digest)
    }

    /// Send `notice` unless it repeats; returns the new record, or `None`
    /// when nothing was sent. Failures are recorded, never retried.
    pub fn send(&self, notice: &Notice, digest: &str, ledger_seq: Option<u64>) -> Result<Option<NotificationRecord>> {
        let records = self.records()?;
        if Self::repeats(&records, digest) {
            return Ok(None);
        }
        let content = serde_json::to_vec(notice)?;
        ensure!(!self.credentials.occurs_in(&content), "notice contains a known credential value; not sent");
        ensure!(!content.windows(13).any(|window| window == b"access_token="), "notice contains an access token");
        let outcome = self.runner.run(&arguments(&self.script, notice), PROCESS_TIMEOUT);
        let (status, errcode, reason_category) = classify(&outcome);
        let record = NotificationRecord {
            seq: records.len() as u64 + 1,
            ledger_seq,
            severity: notice.severity,
            state_digest: digest.to_owned(),
            status,
            errcode,
            sent_at: timestamp_now(),
            reason_category,
            content_sha256: sha256_hex(&content),
        };
        let mut line = serde_json::to_vec(&record)?;
        line.push(b'\n');
        let mut file = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.log)
            .with_context(|| format!("cannot open {}", self.log.display()))?;
        file.write_all(&line)?;
        file.sync_all()?;
        Ok(Some(record))
    }

    /// Notice for the latest ledger entry, sent when the module states or
    /// blockers changed since the last notice.
    pub fn on_ledger_change(&self, entries: &[Entry]) -> Result<Option<NotificationRecord>> {
        let Some(entry) = entries.last() else { return Ok(None) };
        let previous = entries[..entries.len() - 1].iter().rev().find(|earlier| earlier.module == entry.module);
        let notice = notice_for(entry, previous, entries);
        self.send(&notice, &state_digest(entries, None), Some(entry.seq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(code: i32, stdout: &str, stderr: &str) -> RunOutcome {
        RunOutcome::Exited { code: Some(code), stdout: stdout.as_bytes().to_vec(), stderr: stderr.as_bytes().to_vec() }
    }

    #[test]
    fn arguments_are_an_array_with_fixed_sender_and_start() {
        let notice = Notice {
            severity: Severity::Blocker,
            title: "M-P0-RUNNER 阻断：路由不可用".into(),
            message: "a \"quoted\" `code`\nline".into(),
            progress: "一键评估运行器与固定运行策略 0/8".into(),
            overall_progress: "verified 检查点 0/36".into(),
        };
        let args: Vec<String> =
            arguments(Path::new(SCRIPT), &notice).into_iter().map(|arg| arg.into_string().unwrap()).collect();
        assert_eq!(&args[..4], ["-NoProfile", "-NonInteractive", "-File", SCRIPT]);
        let value = |name: &str| args[args.iter().position(|arg| arg == name).unwrap() + 1].clone();
        assert_eq!(value("-Sender"), SENDER);
        assert_eq!(value("-StartedAt"), STARTED_AT);
        assert_eq!(value("-Severity"), "Blocker");
        assert_eq!(value("-Message"), notice.message);
        assert!(!args.iter().any(|arg| arg == "-CredentialPath"));
    }

    #[test]
    fn outcomes_map_to_statuses() {
        let receipt =
            |code: &str| format!("{{\"timestamp\":\"x\",\"delivered\":{},\"errcode\":\"{code}\"}}\n", code == "0");
        assert_eq!(classify(&exited(0, &receipt("0"), "")), (Status::Delivered, Some("0".into()), None));
        let (status, errcode, _) = classify(&exited(1, &receipt("310000"), "DingTalk rejected delivery."));
        assert_eq!((status, errcode.as_deref()), (Status::Undelivered, Some("310000")));
        let cases = [
            (
                "Supply current Progress, OverallProgress and the actual task StartedAt",
                Status::NotSent,
                "missing_metadata",
            ),
            ("Notification exceeds the local 16 KiB limit", Status::NotSent, "too_large"),
            ("Unexpected notification endpoint; refusing", Status::NotSent, "endpoint_rejected"),
            ("Import-Clixml : 找不到路径", Status::NotSent, "credential_unreadable"),
            ("Cannot process argument transformation on parameter 'StartedAt'", Status::NotSent, "parameter_rejected"),
            ("DingTalk delivery could not be confirmed. Exception type: WebException", Status::Unconfirmed, "network"),
            ("something else", Status::Unconfirmed, "no_errcode"),
        ];
        for (stderr, status, reason) in cases {
            assert_eq!(classify(&exited(1, "", stderr)), (status, None, Some(reason.into())), "{stderr}");
        }
        assert_eq!(classify(&RunOutcome::TimedOut).0, Status::Unconfirmed);
        assert_eq!(classify(&RunOutcome::SpawnFailed("none".into())).0, Status::NotSent);
        // Code-page bytes around the ASCII receipt keys do not hide errcode.
        let mut gbk = b"{\"title\":\"\xd6\xd0\\\",\"errcode\":\"0\"}".to_vec();
        gbk.push(b'\n');
        let outcome = RunOutcome::Exited { code: Some(0), stdout: gbk, stderr: Vec::new() };
        assert_eq!(classify(&outcome).0, Status::Delivered);
    }
}
