//! Post-run processing of the Sealed_Holdout (requirements 2.4, 2.6–2.8):
//! leak scan of tool results, Oracle SHA check, judging a copy of the
//! workspace in a new Oracle directory, and the Run_Manifest section.

use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::core::{rpi_agent::AgentMessage, session_journal::Journal};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::{
    common::{run_bounded, sha256_file, Bounded},
    registry::{mark_not_new, HoldoutLog, HoldoutStatus, SealedTask, TaskSet},
    workspace::{snapshot, Workspace},
};

/// Isolation statement of requirement 2.8.
pub const ISOLATION: &str = "oracle_outside_workspace_with_host_path_checks_no_os_sandbox";
/// Requirement 2.6.
pub const ORACLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Bound of one Journal record read.
const RECORD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum LeakLocation {
    Journal { session: String, raw_index: usize },
    Event { run: String, seq: u64 },
}

/// One feature string found in a tool result. `feature` is its 1-based
/// position in the sealed registration; the string itself is never copied.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LeakHit {
    pub feature: usize,
    pub location: LeakLocation,
}

/// Bytes of one tool result to scan.
#[derive(Clone, Debug)]
pub struct ScanRecord {
    pub location: LeakLocation,
    pub bytes: Vec<u8>,
}

/// Byte-substring match of every feature against every record (2.7).
pub fn scan(features: &[String], records: &[ScanRecord]) -> Vec<LeakHit> {
    let mut hits = Vec::new();
    for record in records {
        for (index, feature) in features.iter().enumerate() {
            let needle = feature.as_bytes();
            if !needle.is_empty() && record.bytes.windows(needle.len()).any(|window| window == needle) {
                hits.push(LeakHit { feature: index + 1, location: record.location.clone() });
            }
        }
    }
    hits
}

/// Every `ToolResult` of a Session journal, serialized as stored.
pub fn journal_records(session: &Path, label: &str) -> Result<Vec<ScanRecord>> {
    let journal = Journal::for_session(session);
    ensure!(journal.exists(), "Session {} has no journal to scan", session.display());
    let head = journal.open()?;
    let mut records = Vec::new();
    for raw_index in 0..head.raw_count {
        for message in journal.read(raw_index, raw_index + 1, RECORD_BYTES)? {
            if let AgentMessage::ToolResult(result) = message {
                records.push(ScanRecord {
                    location: LeakLocation::Journal { session: label.to_owned(), raw_index },
                    bytes: serde_json::to_vec(&result)?,
                });
            }
        }
    }
    Ok(records)
}

#[derive(Deserialize)]
struct Envelope {
    run_id: String,
    seq: u64,
    event: serde_json::Value,
}

/// `output` of every `tool_end` event in a Harness `run-N-events.jsonl`.
pub fn event_records(events: &Path) -> Result<Vec<ScanRecord>> {
    let file = fs::File::open(events).with_context(|| format!("cannot open events {}", events.display()))?;
    let mut records = Vec::new();
    for (index, line) in BufReader::new(file).lines().enumerate() {
        let line = line?;
        let envelope: Envelope = serde_json::from_str(&line)
            .with_context(|| format!("events {} line {} is not an event envelope", events.display(), index + 1))?;
        if envelope.event["type"] == "tool_end" {
            if let Some(output) = envelope.event["output"].as_str() {
                records.push(ScanRecord {
                    location: LeakLocation::Event { run: envelope.run_id, seq: envelope.seq },
                    bytes: output.as_bytes().to_vec(),
                });
            }
        }
    }
    Ok(records)
}

/// Copy every workspace file into the new directory `oracle_dir` and return
/// relative path to SHA-256 of each copy, checked against the source.
pub fn copy_workspace(workspace: &Workspace, oracle_dir: &Path) -> Result<BTreeMap<String, String>> {
    let files = snapshot(workspace)?;
    fs::create_dir(oracle_dir)
        .with_context(|| format!("Oracle directory {} must not exist yet", oracle_dir.display()))?;
    for (relative, sha256) in &files {
        let target = oracle_dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(workspace.root.join(relative), &target).with_context(|| format!("cannot copy {relative}"))?;
        ensure!(&sha256_file(&target)? == sha256, "copy of {relative} differs from the workspace file");
    }
    Ok(files)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Correctness {
    Judged {
        passed: u64,
        total: u64,
    },
    /// The Oracle was not run (its SHA-256 differs from the registration).
    Unknown {
        reason: String,
    },
    /// Copy failure, timeout or unparsable Oracle output (2.6).
    OracleError {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct OracleSha {
    pub expected: String,
    pub actual: Option<String>,
}

/// `sealed_holdout` section of the Run_Manifest.
#[derive(Clone, Debug, Serialize)]
pub struct SealedOutcome {
    pub isolation: &'static str,
    /// Authorized tools that bypass the Host path checks (2.8).
    pub unchecked_tools: Vec<String>,
    pub compromised: bool,
    pub leaks: Vec<LeakHit>,
    pub oracle_sha256: OracleSha,
    pub oracle_dir: PathBuf,
    /// Relative path to SHA-256 of each copied file, when the copy finished.
    pub copied: Option<BTreeMap<String, String>>,
    pub oracle_output: Option<PathBuf>,
    pub workspace_unchanged: Option<bool>,
    pub correctness: Correctness,
}

#[derive(Deserialize)]
struct Score {
    passed: u64,
    total: u64,
}

/// Judge the finished workspace (2.4, 2.6, 2.7). `task_dir` must exist; the
/// Oracle directory and outputs are created inside it and kept. `scrub_env`
/// names credential variables removed from the Oracle process.
pub fn judge(
    workspace: &Workspace,
    sealed: &SealedTask,
    leaks: Vec<LeakHit>,
    unchecked_tools: Vec<String>,
    task_dir: &Path,
    scrub_env: &[&str],
    timeout: Duration,
) -> Result<SealedOutcome> {
    let actual = sha256_file(&sealed.oracle_path).ok();
    let sha_matches = actual.as_deref() == Some(sealed.oracle_sha256.as_str());
    let mut outcome = SealedOutcome {
        isolation: ISOLATION,
        unchecked_tools,
        compromised: !leaks.is_empty() || !sha_matches,
        leaks,
        oracle_sha256: OracleSha { expected: sealed.oracle_sha256.clone(), actual },
        oracle_dir: task_dir.join("oracle-dir"),
        copied: None,
        oracle_output: None,
        workspace_unchanged: None,
        correctness: Correctness::Unknown { reason: "Oracle SHA-256 differs from the sealed registration".into() },
    };
    if !sha_matches {
        return Ok(outcome);
    }
    let before = snapshot(workspace)?;
    outcome.correctness = match copy_workspace(workspace, &outcome.oracle_dir) {
        Err(error) => Correctness::OracleError { reason: format!("copy failed: {error:#}") },
        Ok(copied) => {
            outcome.copied = Some(copied);
            let output = task_dir.join("oracle-result.json");
            let mut command = Command::new("python");
            command.arg(&sealed.oracle_path).arg("--artifacts").arg(&outcome.oracle_dir).arg("--out").arg(&output);
            command.current_dir(task_dir);
            for name in scrub_env {
                command.env_remove(name);
            }
            let correctness = match run_bounded(command, timeout)? {
                Bounded::TimedOut => {
                    Correctness::OracleError { reason: format!("Oracle exceeded {} seconds", timeout.as_secs()) }
                }
                Bounded::Exited { code, stdout, stderr } => {
                    fs::write(task_dir.join("oracle.stdout.bin"), stdout)?;
                    fs::write(task_dir.join("oracle.stderr.bin"), stderr)?;
                    let score = fs::read(&output).ok().and_then(|bytes| serde_json::from_slice::<Score>(&bytes).ok());
                    match score {
                        Some(Score { passed, total }) if code == Some(0) && total > 0 && passed <= total => {
                            Correctness::Judged { passed, total }
                        }
                        _ => Correctness::OracleError {
                            reason: format!("Oracle exited {code:?} without a parsable passed/total result"),
                        },
                    }
                }
            };
            outcome.oracle_output = output.is_file().then_some(output);
            correctness
        }
    };
    outcome.workspace_unchanged = Some(snapshot(workspace)? == before);
    Ok(outcome)
}

/// Requirement 2.7: a compromised real-model result turns the Sealed_Holdout
/// into `not new holdout`. Controlled runs with a fake upstream never change
/// the status (2.5). Returns the reason when the status changed.
pub fn record_compromise(
    set: &mut TaskSet,
    log: &mut HoldoutLog,
    sealed: &SealedTask,
    outcome: &SealedOutcome,
    real_model: bool,
    date: NaiveDate,
) -> Result<Option<String>> {
    let entry = set.tasks.iter().find(|entry| entry.task_id == sealed.task_id);
    let is_new = matches!(entry.and_then(|entry| entry.holdout.as_ref()), Some(HoldoutStatus::New { .. }));
    if !outcome.compromised || !real_model || !is_new {
        return Ok(None);
    }
    let reason = if outcome.leaks.is_empty() {
        format!(
            "holdout_compromised: Oracle SHA-256 expected {} actual {}",
            outcome.oracle_sha256.expected,
            outcome.oracle_sha256.actual.as_deref().unwrap_or("unreadable")
        )
    } else {
        let locations: Vec<String> =
            outcome.leaks.iter().map(|hit| format!("#{} {:?}", hit.feature, hit.location)).collect();
        format!("holdout_compromised: feature strings in tool results at {}", locations.join(", "))
    };
    mark_not_new(set, log, &sealed.task_id, date, vec![reason.clone()], Some("requirement 2.7".into()))?;
    Ok(Some(reason))
}
