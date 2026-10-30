//! `lite-eval long-trial` (requirement 35; tasks 60.1, 60.2).
//!
//! The command checks the policy, task and environment, then runs the driver
//! [`DRIVER_PATH`] under `budgets.LONG-TRIAL`. The driver acts as the Host
//! of two JSONL_Sidecar processes over one Task and one Session (see its
//! module docstring). The driver writes this layout; nothing else is assumed:
//!
//! * `trial.json`: paths, kill and resume summary;
//! * `trial-log.jsonl`: one line per Host callback, best-effort `event`
//!   frame, Host receipt, marker, kill and reconcile reply, numbered `seq`
//!   in driver order;
//! * `evidence-1/`, `evidence-2/`: the sidecars' Task evidence (Harness
//!   `run-N-*` files, `jsonl-<rpc>-request|reply.json`, `resume.json`);
//! * `host-receipts/`, `ops-N/`, `sidecar-N.log`, `fence-*.json`,
//!   `journal-*.json` snapshots, and the Session under `state/`.
//!
//! The checks then read only those files and record expected and actual
//! values in `long-trial.json`:
//!
//! * coverage (35.1): completed `compact` events, Host review continuations
//!   (a committed NeedsChanges `prepare_request` answered with a
//!   `prepare_reply` whose new Run was admitted), length-truncation
//!   continuations (the same over `prepare_stop_request` of a `length` stop),
//!   and resumes (`resume.json`), each with its Run ID and event sequence;
//! * the Session path, the journal records captured during the trial
//!   (compacted prefixes, truncated messages and their references), the Task
//!   Logical_Call count, the authority hash, and every effects-unknown call's
//!   committed Host receipt and its order (35.3, 35.4).
//!
//! A failed check or a refused resume writes a Failure_Record (35.5); too few
//! required events without a failed check is `覆盖不足` (35.6). Nothing is
//! deleted: the record lists every preserved artifact.
//!
//! When the driver times out or exits non-zero, every sidecar the trial log
//! shows as spawned but not ended is killed before the checks run, and each
//! kill is listed in `orphan_sidecars`.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{core::session_journal::Journal, TaskStop};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    common::{
        new_run_dir, run_bounded, sha256_file, sha256_hex, timestamp_now, write_json_new, write_output_new, Bounded,
        Credentials,
    },
    digest::tree_digest,
    failure::{validate_refs, Classification, EvidenceRef, FailureClass, FailureContext, FailureRecord, OracleRecord},
    metrics::{load_events, EventsFile, PathRecord},
    policy::load_fixed,
    probe::probe,
    registry::{load, TASK_SET_PATH},
};

/// Run_Policy budget key of this trial.
pub const LONG_TRIAL_BUDGET: &str = "LONG-TRIAL";
/// Repository-relative path of the Python driver.
pub const DRIVER_PATH: &str = "tools/lite-eval/long_trial/long_trial.py";
/// Task whose fixture the trial copies when `--task` is not given.
pub const DEFAULT_TASK: &str = "S3";
pub const RECORD_VERSION: u32 = 1;
/// Label of a trial whose required events fell short (35.6).
pub const COVERAGE_INSUFFICIENT: &str = "覆盖不足";
/// Extra time the driver gets beyond the Task deadline (resume and cleanup).
const DRIVER_MARGIN: Duration = Duration::from_secs(900);
/// Largest number of problems listed per check (the count is always given).
const PROBLEM_LIMIT: usize = 20;
/// Time one `taskkill` / `kill -9` of an orphaned sidecar may take.
const KILL_TIMEOUT: Duration = Duration::from_secs(30);

/// Required events (35.1): type, count, whether the count is exact.
pub const REQUIRED_EVENTS: [(&str, usize, bool); 4] = [
    ("compaction", 1, false),
    ("host_review_continuation", 2, false),
    ("length_continuation", 1, false),
    ("resume", 1, true),
];

// ----- what the driver wrote ------------------------------------------------

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Fences {
    #[serde(default)]
    pub start: Option<PathBuf>,
    #[serde(default)]
    pub before_resume: Option<PathBuf>,
    #[serde(default)]
    pub after_resume: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ResumeAttempt {
    pub attempted: bool,
    pub refused: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub rejected: Option<Value>,
}

/// `trial.json` (fields the checks use; others are kept on disk).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct TrialRecord {
    #[serde(default)]
    pub task_id: String,
    /// The Session path given at Task start.
    #[serde(default)]
    pub session_path: PathBuf,
    #[serde(default)]
    pub workspace: PathBuf,
    #[serde(default)]
    pub trial_log: PathBuf,
    #[serde(default)]
    pub host_receipts: PathBuf,
    #[serde(default)]
    pub evidence_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub operation_dirs: Vec<PathBuf>,
    #[serde(default)]
    pub sidecar_logs: Vec<PathBuf>,
    #[serde(default)]
    pub fences: Fences,
    #[serde(default)]
    pub journal_snapshots: Vec<PathBuf>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub kill: Option<Value>,
    #[serde(default)]
    pub resume: Option<ResumeAttempt>,
    #[serde(default)]
    pub stop: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// One admitted Run of a sidecar evidence directory.
#[derive(Clone, Debug)]
pub struct RunEvidence {
    pub number: usize,
    pub admission: PathBuf,
    pub run_id: String,
    pub session_path: String,
    /// The Run's admitted call cap (`remaining_calls` of its admission).
    pub remaining_calls: u64,
    /// `run-N-observation.json`; absent for a Run whose process was killed.
    pub observation: Option<Value>,
    pub events: EventsFile,
}

/// One `jsonl-<rpc>-request.json` and its reply, if any.
#[derive(Clone, Debug)]
pub struct RpcCall {
    pub rpc: String,
    pub request_path: PathBuf,
    pub request: Value,
    pub reply: Option<Value>,
}

/// One sidecar's Task evidence directory.
#[derive(Clone, Debug)]
pub struct EvidenceDir {
    pub path: PathBuf,
    pub resume: Option<Value>,
    pub runs: Vec<RunEvidence>,
    pub rpc: Vec<RpcCall>,
    /// `released.json` or `outcome.json`.
    pub outcome: Option<(PathBuf, Value)>,
    pub stop_snapshots: Vec<(PathBuf, Value)>,
    pub rejected: Option<Value>,
}

/// Everything the checks read, loaded once.
#[derive(Clone, Debug)]
pub struct Trial {
    pub dir: PathBuf,
    pub record: TrialRecord,
    pub log: Vec<Value>,
    pub evidence: Vec<EvidenceDir>,
    /// `snapshot_ref` of every Python_Client `*-stop-snapshot-checked.json`.
    pub snapshot_refs: Vec<Value>,
    /// Files that could not be read or parsed.
    pub errors: Vec<String>,
}

fn read_json(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("{} is not JSON", path.display()))
}

fn numbered(dir: &Path, prefix: &str, suffix: &str) -> Vec<(usize, PathBuf)> {
    let mut files: Vec<(usize, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let path = item.ok()?.path();
            let number = path.file_name()?.to_str()?.strip_prefix(prefix)?.strip_suffix(suffix)?.parse().ok()?;
            Some((number, path))
        })
        .collect();
    files.sort();
    files
}

/// An optional file: `None` when absent; unreadable is listed in `errors`.
fn optional(file: &Path, errors: &mut Vec<String>) -> Option<Value> {
    if !file.exists() {
        return None;
    }
    read_json(file).map_err(|error| errors.push(format!("{error:#}"))).ok()
}

impl EvidenceDir {
    fn load(path: &Path, errors: &mut Vec<String>) -> Self {
        let resume = optional(&path.join("resume.json"), errors);
        let rejected = optional(&path.join("jsonl-resume-rejected.json"), errors);
        let outcome = ["released.json", "outcome.json"]
            .iter()
            .map(|name| path.join(name))
            .find(|file| file.exists())
            .and_then(|file| optional(&file, errors).map(|value| (file, value)));
        let mut runs = Vec::new();
        for (number, admission) in numbered(path, "run-", "-admission.json") {
            let value = match read_json(&admission) {
                Ok(value) => value,
                Err(error) => {
                    errors.push(format!("{error:#}"));
                    continue;
                }
            };
            let observation = optional(&path.join(format!("run-{number}-observation.json")), errors);
            let events = load_events(&path.join(format!("run-{number}-events.jsonl")));
            errors.extend(events.errors.iter().cloned());
            runs.push(RunEvidence {
                number,
                admission,
                run_id: value["run_id"].as_str().unwrap_or_default().to_owned(),
                session_path: value["session_path"].as_str().unwrap_or_default().to_owned(),
                remaining_calls: value["remaining_calls"].as_u64().unwrap_or(0),
                observation,
                events,
            });
        }
        let stop_snapshots = numbered(path, "run-", "-stop-snapshot.json")
            .into_iter()
            .filter_map(|(_, file)| optional(&file, errors).map(|value| (file, value)))
            .collect();
        let mut rpc: Vec<(u64, RpcCall)> = fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                let request_path = item.ok()?.path();
                let rpc = request_path.file_name()?.to_str()?.strip_prefix("jsonl-")?.strip_suffix("-request.json")?;
                Some((rpc.to_owned(), request_path))
            })
            .filter_map(|(rpc, request_path)| {
                let request = optional(&request_path, errors)?;
                let reply = optional(&path.join(format!("jsonl-{rpc}-reply.json")), errors);
                let order = rpc.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(u64::MAX);
                Some((order, RpcCall { rpc, request_path, request, reply }))
            })
            .collect();
        rpc.sort_by(|a, b| (a.0, &a.1.rpc).cmp(&(b.0, &b.1.rpc)));
        Self {
            path: path.to_owned(),
            resume,
            runs,
            rpc: rpc.into_iter().map(|(_, call)| call).collect(),
            outcome,
            stop_snapshots,
            rejected,
        }
    }
}

impl Trial {
    /// Load `<dir>/trial.json` and the files it names. Unreadable files are
    /// listed in `errors`; the checks then fail on what is missing.
    pub fn load(dir: &Path) -> Self {
        let mut errors = Vec::new();
        let record: TrialRecord = match read_json(&dir.join("trial.json"))
            .and_then(|value| serde_json::from_value(value).context("trial.json has unexpected field types"))
        {
            Ok(record) => record,
            Err(error) => {
                errors.push(format!("{error:#}"));
                TrialRecord::default()
            }
        };
        let log = match fs::read(&record.trial_log) {
            Ok(bytes) => String::from_utf8_lossy(&bytes)
                .lines()
                .enumerate()
                .filter(|(_, line)| !line.trim().is_empty())
                .filter_map(|(index, line)| {
                    serde_json::from_str::<Value>(line)
                        .map_err(|_| errors.push(format!("trial log line {} is not JSON", index + 1)))
                        .ok()
                })
                .collect(),
            Err(error) => {
                errors.push(format!("cannot read trial log {}: {error}", record.trial_log.display()));
                Vec::new()
            }
        };
        let evidence = record.evidence_dirs.iter().map(|path| EvidenceDir::load(path, &mut errors)).collect();
        let snapshot_refs = record
            .operation_dirs
            .iter()
            .flat_map(|dir| fs::read_dir(dir).into_iter().flatten())
            .filter_map(|item| item.ok().map(|item| item.path()))
            .filter(|path| path.to_string_lossy().ends_with("-stop-snapshot-checked.json"))
            .filter_map(|path| read_json(&path).map_err(|error| errors.push(format!("{error:#}"))).ok())
            .map(|value| value["snapshot_ref"].clone())
            .collect();
        Self { dir: dir.to_owned(), record, log, evidence, snapshot_refs, errors }
    }

    fn runs(&self) -> impl Iterator<Item = &RunEvidence> {
        self.evidence.iter().flat_map(|dir| dir.runs.iter())
    }

    fn run(&self, run_id: &str) -> Option<&RunEvidence> {
        self.runs().find(|run| run.run_id == run_id)
    }

    fn log_lines<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
        self.log.iter().filter(move |line| line["kind"] == kind)
    }
}

// ----- coverage (35.1, 35.6) -------------------------------------------------

/// One counted event: type, Run ID and event sequence number.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TrialEvent {
    pub event_type: &'static str,
    pub run_id: String,
    /// Sequence number in the Run's events file (for continuations and the
    /// resume: the new Run's first event); `None` when it has no events.
    pub seq: Option<u64>,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shortfall {
    pub event_type: &'static str,
    /// `>= n` or `== n`.
    pub required: String,
    pub actual: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub events: Vec<TrialEvent>,
    pub counts: BTreeMap<&'static str, usize>,
    /// Required events below their count (35.6).
    pub missing: Vec<Shortfall>,
    /// Exact counts exceeded (more than one resume): a failure, not coverage.
    pub excess: Vec<Shortfall>,
}

/// Inline body or the `<name>_ref` file of a request.
fn body(request: &Value, name: &str) -> Option<Value> {
    if let Some(value) = request.get(name) {
        return Some(value.clone());
    }
    read_json(Path::new(request.get(format!("{name}_ref"))?["path"].as_str()?)).ok()
}

fn committed_needs_changes(receipt: &Value) -> bool {
    receipt["committed"] == true
        && receipt["settlement"] == "Settled"
        && receipt["verdict"].get("NeedsChanges").is_some()
}

pub fn coverage(trial: &Trial) -> Coverage {
    let first_seq = |run: &RunEvidence| run.events.lines.first().map(|line| line.seq);
    let mut events = Vec::new();
    for run in trial.runs() {
        for line in &run.events.lines {
            if line.event["type"] == "compact" && line.event["status"] == "completed" {
                events.push(TrialEvent {
                    event_type: "compaction",
                    run_id: run.run_id.clone(),
                    seq: Some(line.seq),
                    source: format!("{}#{}", run.events.path.display(), line.seq),
                });
            }
        }
    }
    for dir in &trial.evidence {
        for call in &dir.rpc {
            let (event_type, body_name, reply_kind) = match call.request["kind"].as_str() {
                Some("prepare_request") => ("host_review_continuation", "continuation", "prepare_reply"),
                Some("prepare_stop_request") => ("length_continuation", "stop_continuation", "prepare_stop_reply"),
                _ => continue,
            };
            let Some(continuation) = body(&call.request, body_name) else { continue };
            if !committed_needs_changes(&continuation["receipt"]) {
                continue;
            }
            if event_type == "length_continuation" && continuation["previous"]["evidence"]["cause"] != "length" {
                continue;
            }
            let Some(reply) = call.reply.as_ref().filter(|reply| reply["kind"] == reply_kind) else { continue };
            let Some(next) = reply["run_id_next"].as_str().and_then(|id| trial.run(id)) else { continue };
            events.push(TrialEvent {
                event_type,
                run_id: next.run_id.clone(),
                seq: first_seq(next),
                source: call.request_path.display().to_string(),
            });
        }
        if let (Some(resume), Some(run)) = (&dir.resume, dir.runs.first()) {
            if resume["kind"] == "fenced_task_resume" {
                events.push(TrialEvent {
                    event_type: "resume",
                    run_id: run.run_id.clone(),
                    seq: first_seq(run),
                    source: dir.path.join("resume.json").display().to_string(),
                });
            }
        }
    }
    let counts: BTreeMap<&'static str, usize> = REQUIRED_EVENTS
        .iter()
        .map(|(kind, _, _)| (*kind, events.iter().filter(|event| event.event_type == *kind).count()))
        .collect();
    let shortfall = |kind, required: usize, exact: bool| Shortfall {
        event_type: kind,
        required: format!("{} {required}", if exact { "==" } else { ">=" }),
        actual: counts[kind],
    };
    let missing =
        REQUIRED_EVENTS.iter().filter(|(kind, n, _)| counts[kind] < *n).map(|(k, n, e)| shortfall(k, *n, *e)).collect();
    let excess = REQUIRED_EVENTS
        .iter()
        .filter(|(kind, n, exact)| *exact && counts[kind] > *n)
        .map(|(k, n, e)| shortfall(k, *n, *e))
        .collect();
    Coverage { events, counts, missing, excess }
}

// ----- checks (35.3, 35.4) --------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Passed,
    Failed,
    /// The trial never reached the state the check needs (no resume).
    NotApplicable,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Check {
    pub name: &'static str,
    pub status: CheckStatus,
    pub expected: Value,
    pub actual: Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub problems: Vec<String>,
}

fn check(name: &'static str, expected: Value, actual: Value, mut problems: Vec<String>) -> Check {
    let status = if problems.is_empty() { CheckStatus::Passed } else { CheckStatus::Failed };
    if problems.len() > PROBLEM_LIMIT {
        let more = problems.len() - PROBLEM_LIMIT;
        problems.truncate(PROBLEM_LIMIT);
        problems.push(format!("… and {more} more"));
    }
    Check { name, status, expected, actual, problems }
}

/// Comparable spelling of a path: canonical when it exists; on Windows
/// without the verbatim prefix, with backslashes and in lowercase.
fn path_key(path: &Path) -> String {
    let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let text = resolved.to_string_lossy().into_owned();
    if cfg!(windows) {
        let text = text.replace('/', "\\");
        text.strip_prefix(r"\\?\").unwrap_or(&text).trim_end_matches('\\').to_lowercase()
    } else {
        text
    }
}

fn fence_value(path: &Option<PathBuf>) -> Option<(PathBuf, Result<Value>)> {
    path.as_ref().map(|path| (path.clone(), read_json(path)))
}

fn check_session_path(trial: &Trial) -> Check {
    let expected = &trial.record.session_path;
    let key = path_key(expected);
    let mut seen: Vec<(String, String)> =
        trial.runs().map(|run| (run.admission.display().to_string(), run.session_path.clone())).collect();
    let fences = &trial.record.fences;
    for (path, value) in
        [&fences.start, &fences.before_resume, &fences.after_resume].into_iter().filter_map(fence_value)
    {
        if let Some(session) = value.ok().and_then(|value| value["session_path"].as_str().map(str::to_owned)) {
            seen.push((path.display().to_string(), session));
        }
    }
    for dir in &trial.evidence {
        if let Some(session) = dir.resume.as_ref().and_then(|resume| resume["fence"]["session_path"].as_str()) {
            seen.push((dir.path.join("resume.json").display().to_string(), session.to_owned()));
        }
    }
    let mut problems: Vec<String> = seen
        .iter()
        .filter(|(_, session)| path_key(Path::new(session)) != key)
        .map(|(source, session)| format!("{source} names Session {session}"))
        .collect();
    if expected.as_os_str().is_empty() {
        problems.push("trial.json names no Session path".into());
    }
    if trial.runs().next().is_none() {
        problems.push("no Run admission was recorded".into());
    }
    let actual: Vec<Value> =
        seen.iter().map(|(source, session)| json!({"source": source, "session_path": session})).collect();
    check("session_path", json!({"session_path": expected}), json!(actual), problems)
}

/// `(index → (hash, chain))` of the committed records in a journal's segments.
fn journal_rows(directory: &Path, raw_count: u64) -> Result<BTreeMap<u64, (String, String)>> {
    let mut segments: Vec<PathBuf> = fs::read_dir(directory)?
        .filter_map(|item| item.ok().map(|item| item.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("segment-") && name.ends_with(".jsonl"))
        })
        .collect();
    segments.sort();
    let mut rows = BTreeMap::new();
    for segment in segments {
        for line in fs::read(&segment)?.split(|byte| *byte == b'\n').filter(|line| !line.is_empty()) {
            let row: Value = serde_json::from_slice(line)
                .with_context(|| format!("{} has a line that is not JSON", segment.display()))?;
            let index = row["index"].as_u64().context("journal row without index")?;
            if index < raw_count {
                let text = |key: &str| row[key].as_str().unwrap_or_default().to_owned();
                rows.insert(index, (text("hash"), text("chain")));
            }
        }
    }
    Ok(rows)
}

fn check_journal(trial: &Trial, coverage: &Coverage) -> Check {
    let session = &trial.record.session_path;
    let directory = session.with_extension("journal");
    let journal = Journal::for_session(session);
    let mut problems = Vec::new();
    let raw_count = read_json(&directory.join("head.json"))
        .ok()
        .and_then(|head| head["head"]["raw_count"].as_u64())
        .unwrap_or_else(|| {
            problems.push(format!("journal head {} is unreadable", directory.join("head.json").display()));
            0
        });
    let integrity = match usize::try_from(raw_count)
        .map_err(anyhow::Error::from)
        .and_then(|count| journal.read(0, count, usize::MAX))
    {
        Ok(messages) => format!("verified {} records", messages.len()),
        Err(error) => {
            problems.push(format!("journal integrity: {error:#}"));
            format!("failed: {error:#}")
        }
    };
    let rows = journal_rows(&directory, raw_count).unwrap_or_else(|error| {
        problems.push(format!("journal segments: {error:#}"));
        BTreeMap::new()
    });
    let mut snapshots = Vec::new();
    let mut captured = 0usize;
    for path in &trial.record.journal_snapshots {
        let value = match read_json(path) {
            Ok(value) => value,
            Err(error) => {
                problems.push(format!("{error:#}"));
                continue;
            }
        };
        let records = value["records"].as_array().cloned().unwrap_or_default();
        captured += records.len();
        let mismatched: Vec<u64> = records
            .iter()
            .filter_map(|record| {
                let index = record["index"].as_u64()?;
                let expected = (record["hash"].as_str()?.to_owned(), record["chain"].as_str()?.to_owned());
                (rows.get(&index) != Some(&expected)).then_some(index)
            })
            .collect();
        problems.extend(
            mismatched.iter().map(|index| format!("record {index} of {} is missing or rewritten", path.display())),
        );
        snapshots.push(json!({"path": path, "records": records.len(), "mismatched": mismatched}));
    }
    let compactions: Vec<Value> = trial
        .runs()
        .flat_map(|run| run.events.lines.iter().map(move |line| (run, line)))
        .filter(|(_, line)| line.event["type"] == "compact" && line.event["status"] == "completed")
        .map(|(run, line)| {
            let covered = line.event["covered_prefix"].as_u64().unwrap_or(u64::MAX);
            let present = covered <= raw_count;
            if !present {
                problems.push(format!(
                    "compaction at {}#{} covered raw messages [0, {covered}) but the journal holds {raw_count}",
                    run.events.path.display(),
                    line.seq
                ));
            }
            json!({"run_id": run.run_id, "seq": line.seq, "covered_prefix": covered, "present": present})
        })
        .collect();
    let mut truncated = Vec::new();
    for (path, snapshot) in trial.evidence.iter().flat_map(|dir| dir.stop_snapshots.iter()) {
        let reference = &snapshot["truncated"];
        let Some(index) = reference["raw_index"].as_u64().and_then(|index| usize::try_from(index).ok()) else {
            continue;
        };
        let proof = journal.proof(index, index + 1).map_err(|error| format!("{error:#}"));
        let typed =
            journal.read(index, index + 1, usize::MAX).map_err(|error| format!("{error:#}")).and_then(|messages| {
                let message = messages.first().ok_or_else(|| "message missing".to_owned())?;
                serde_json::to_vec(message).map(|bytes| sha256_hex(&bytes)).map_err(|error| error.to_string())
            });
        let proof_ok = proof.as_deref().ok() == reference["journal_proof"].as_str();
        let typed_ok = typed.as_deref().ok() == reference["typed_sha256"].as_str();
        if !proof_ok || !typed_ok {
            problems.push(format!(
                "truncated message {index} of {} changed (proof {proof:?}, typed sha256 {typed:?})",
                path.display()
            ));
        }
        truncated.push(json!({"snapshot": path, "raw_index": index, "proof_matches": proof_ok,
            "typed_sha256_matches": typed_ok}));
    }
    let references: Vec<Value> = trial
        .snapshot_refs
        .iter()
        .filter_map(|reference| {
            let path = PathBuf::from(reference["path"].as_str()?);
            let actual = sha256_file(&path).map_err(|error| format!("{error:#}"));
            let matches = actual.as_deref().ok() == reference["sha256"].as_str();
            if !matches {
                problems.push(format!("reviewed stop snapshot {} changed: {actual:?}", path.display()));
            }
            Some(json!({"path": path, "sha256": reference["sha256"], "matches": matches}))
        })
        .collect();
    let expected = json!({
        "rule": "every journal record captured during the trial exists with the same hash and chain; the chain verifies",
        "captured_records": captured,
        "compactions": coverage.counts.get("compaction").copied().unwrap_or(0),
        "truncated_messages": truncated.len(),
        "stop_snapshot_references": references.len(),
    });
    let actual = json!({"raw_count": raw_count, "integrity": integrity, "snapshots": snapshots,
        "compactions": compactions, "truncated": truncated, "references": references});
    check("journal_unchanged", expected, actual, problems)
}

fn check_logical_calls(trial: &Trial, max_calls: usize) -> Check {
    let max = max_calls as u64;
    let mut records: Vec<(String, u64)> = Vec::new();
    let mut interrupted = Vec::new();
    let mut problems = Vec::new();
    let mut carried: Option<u64> = None;
    for dir in &trial.evidence {
        let mut cumulative = 0u64;
        if let Some(resume) = &dir.resume {
            let consumed = resume["consumed_calls"].as_u64().unwrap_or(0);
            if let Some(expected) = carried {
                if consumed != expected {
                    problems.push(format!(
                        "resume.json of {} charges {consumed} consumed calls, the fenced evidence gives {expected} \
                         (interrupted Runs at their admitted cap)",
                        dir.path.display()
                    ));
                }
            }
            records.push((dir.path.join("resume.json").display().to_string(), consumed));
            cumulative = consumed;
        }
        for run in &dir.runs {
            match &run.observation {
                Some(observation) => {
                    cumulative = observation["total_calls"].as_u64().unwrap_or(0);
                    records.push((format!("{} total_calls", run.admission.display()), cumulative));
                }
                None => {
                    // 35.3: a Run without observation is charged its admitted cap.
                    cumulative += run.remaining_calls;
                    interrupted.push(json!({"run_id": run.run_id, "admitted_calls": run.remaining_calls,
                        "cumulative_after": cumulative}));
                    records.push((format!("{} interrupted, charged its cap", run.admission.display()), cumulative));
                }
            }
        }
        if let Some((path, outcome)) = &dir.outcome {
            let observed = outcome["observed_calls"].as_u64().unwrap_or(0);
            records.push((path.display().to_string(), observed));
        }
        carried = Some(cumulative);
    }
    for window in records.windows(2) {
        if window[1].1 < window[0].1 {
            problems.push(format!("{} ({}) is below {} ({})", window[1].0, window[1].1, window[0].0, window[0].1));
        }
    }
    problems.extend(
        records.iter().filter(|(_, value)| *value > max).map(|(source, value)| format!("{source}: {value} > {max}")),
    );
    if records.is_empty() {
        problems.push("no Logical_Call record exists".into());
    }
    let actual = json!({
        "records": records.iter().map(|(source, value)| json!({"source": source, "total_calls": value})).collect::<Vec<_>>(),
        "interrupted_runs": interrupted,
        "final": records.last().map(|(_, value)| *value),
    });
    let expected = json!({"max_calls": max, "never_decreases": true, "interrupted_run_charged_at_admission_cap": true});
    check("logical_calls", expected, actual, problems)
}

fn check_authority(trial: &Trial) -> Check {
    let declaration = |value: &Value, key: &str| value[key]["declaration_sha256"].as_str().map(str::to_owned);
    let fences = &trial.record.fences;
    let mut problems = Vec::new();
    let mut values: Vec<(String, Option<String>)> = Vec::new();
    for (label, path) in
        [("start", &fences.start), ("before_resume", &fences.before_resume), ("after_resume", &fences.after_resume)]
    {
        if let Some((path, value)) = fence_value(path) {
            match value {
                Ok(value) => values.push((format!("{label}: {}", path.display()), declaration(&value, "authority"))),
                Err(error) => problems.push(format!("{error:#}")),
            }
        }
    }
    for dir in &trial.evidence {
        if let Some(resume) = &dir.resume {
            values.push((
                format!("{} fence", dir.path.join("resume.json").display()),
                declaration(&resume["fence"], "authority"),
            ));
        }
    }
    let expected = values.first().filter(|(label, _)| label.starts_with("start:")).and_then(|(_, value)| value.clone());
    match &expected {
        None => problems.push("no authority declaration hash was captured at Task start".into()),
        Some(expected) => problems.extend(
            values
                .iter()
                .filter(|(_, value)| value.as_ref() != Some(expected))
                .map(|(source, value)| format!("{source} has authority hash {value:?}")),
        ),
    }
    let actual: Vec<Value> =
        values.iter().map(|(source, value)| json!({"source": source, "declaration_sha256": value})).collect();
    check("authority_hash", json!({"declaration_sha256": expected}), json!(actual), problems)
}

fn check_effects_unknown(trial: &Trial, coverage: &Coverage) -> Check {
    let resumed = coverage.counts.get("resume").copied().unwrap_or(0) > 0;
    // (call id, tool, where it was found)
    let mut unknown: Vec<(String, String, String)> = Vec::new();
    let mut add = |id: Option<&str>, tool: Option<&str>, source: String| {
        if let Some(id) = id.filter(|id| !id.is_empty()) {
            if !unknown.iter().any(|(known, _, _)| known == id) {
                unknown.push((id.to_owned(), tool.unwrap_or_default().to_owned(), source));
            }
        }
    };
    for dir in &trial.evidence {
        if let Some(resume) = &dir.resume {
            for call in resume["interrupted_run"]["open_tool_calls"].as_array().into_iter().flatten() {
                add(call["id"].as_str(), call["name"].as_str(), "open call at process termination".into());
            }
        }
        if let Some((path, outcome)) = &dir.outcome {
            for record in outcome["unknown_effects"].as_array().into_iter().flatten() {
                add(record["native_call_id"].as_str(), record["tool"].as_str(), path.display().to_string());
            }
        }
    }
    for run in trial.runs() {
        for line in &run.events.lines {
            if line.event["type"] == "tool_end" && line.event["effects_unknown"] == true {
                add(
                    line.event["step_id"].as_str(),
                    line.event["title"].as_str(),
                    format!("{}#{}", run.events.path.display(), line.seq),
                );
            }
        }
    }
    let expected = json!({
        "effects_unknown_calls": ">= 1 (including the call open at process termination)",
        "each_has_committed_host_receipt": true,
        "receipt_before": ["the first upstream model request after resume", "every re-issue of the same operation"],
    });
    if unknown.is_empty() && !resumed {
        return Check {
            name: "effects_unknown_receipts",
            status: CheckStatus::NotApplicable,
            expected,
            actual: json!({"effects_unknown_calls": 0, "reason": "the sidecar was never terminated and resumed"}),
            problems: Vec::new(),
        };
    }
    let seq = |line: &Value| line["seq"].as_u64().unwrap_or(u64::MAX);
    // The first upstream request after resume: the first `model_call` event
    // of the resumed sidecar. The reconcile reply is a lower bound when that
    // best-effort frame was not observed (the Harness sends nothing upstream
    // before it accepted the reply).
    let anchor = trial
        .log_lines("event")
        .find(|line| line["process"] == 2 && line["event_type"] == "model_call")
        .map(|line| ("first model_call event of the resumed sidecar", seq(line)))
        .or_else(|| {
            trial
                .log_lines("reconcile_reply")
                .next()
                .map(|line| ("reconcile reply (precedes every upstream request after resume)", seq(line)))
        });
    let mut problems = Vec::new();
    if unknown.is_empty() {
        problems.push("no effects-unknown call although the sidecar was terminated and resumed".into());
    }
    if resumed && anchor.is_none() {
        problems.push("no upstream-request anchor after resume in the trial log".into());
    }
    let calls: Vec<Value> = unknown
        .iter()
        .map(|(id, tool, source)| {
            let receipt = trial.log_lines("host_receipt").find(|line| {
                line["committed"] == true
                    && line["native_call_id"] == id.as_str()
                    && line["path"]
                        .as_str()
                        .and_then(|path| read_json(Path::new(path)).ok())
                        .is_some_and(|file| file["committed"] == true && file["native_call_id"] == id.as_str())
            });
            let original = trial
                .log_lines("callback")
                .find(|line| line["frame_kind"] == "tool_call" && line["native_call_id"] == id.as_str());
            let reissues: Vec<u64> = original
                .map(|original| {
                    trial
                        .log_lines("callback")
                        .filter(|line| {
                            line["frame_kind"] == "tool_call"
                                && seq(line) > seq(original)
                                && (line["native_call_id"] == original["native_call_id"]
                                    || (line["tool_name"] == original["tool_name"]
                                        && line["args_sha256"] == original["args_sha256"]))
                        })
                        .map(seq)
                        .collect()
                })
                .unwrap_or_default();
            match receipt {
                None => problems.push(format!("call {id} ({tool}) has no committed Host receipt")),
                Some(receipt) => {
                    let at = seq(receipt);
                    if let Some((kind, anchor)) = anchor.filter(|_| resumed) {
                        if at >= anchor {
                            problems
                                .push(format!("receipt of {id} (log {at}) is not before the {kind} (log {anchor})"));
                        }
                    }
                    problems.extend(
                        reissues.iter().filter(|again| **again <= at).map(|again| {
                            format!("call {id} was re-issued (log {again}) before its receipt (log {at})")
                        }),
                    );
                }
            }
            json!({
                "native_call_id": id, "tool": tool, "found_in": source,
                "receipt": receipt.map(|line| json!({"path": line["path"], "log_seq": line["seq"]})),
                "anchor": anchor.filter(|_| resumed).map(|(kind, seq)| json!({"kind": kind, "log_seq": seq})),
                "reissues_log_seq": reissues,
            })
        })
        .collect();
    check("effects_unknown_receipts", expected, json!({"effects_unknown_calls": calls.len(), "calls": calls}), problems)
}

fn check_resume(trial: &Trial) -> Check {
    let attempt = trial.record.resume.clone().unwrap_or_default();
    let rejected: Vec<Value> = trial.evidence.iter().filter_map(|dir| dir.rejected.clone()).collect();
    let expected = json!({"resume": "accepted after the Host settlement; no jsonl-resume-rejected.json"});
    let actual = json!({"attempted": attempt.attempted, "refused": attempt.refused, "error": attempt.error,
        "rejected": rejected, "killed": trial.record.kill.as_ref().is_some_and(|kill| kill.get("pid").is_some())});
    let killed = trial.record.kill.as_ref().is_some_and(|kill| kill.get("pid").is_some());
    if !attempt.attempted && !killed && rejected.is_empty() {
        return Check {
            name: "resume_accepted",
            status: CheckStatus::NotApplicable,
            expected,
            actual,
            problems: Vec::new(),
        };
    }
    let mut problems = Vec::new();
    if killed && !attempt.attempted {
        problems.push(format!(
            "the sidecar was terminated but no resume was attempted: {}",
            attempt.error.as_deref().or(trial.record.error.as_deref()).unwrap_or("no reason recorded")
        ));
    }
    if attempt.refused || !rejected.is_empty() {
        problems.push(format!(
            "resume was refused: {}",
            attempt.error.as_deref().unwrap_or("see jsonl-resume-rejected.json")
        ));
    }
    check("resume_accepted", expected, actual, problems)
}

fn check_readable(trial: &Trial) -> Check {
    check(
        "evidence_readable",
        json!("trial.json, the trial log and every file it names are readable"),
        json!({"unreadable": trial.errors.len()}),
        trial.errors.clone(),
    )
}

// ----- verdict and record ---------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum TrialVerdict {
    Passed,
    /// A check failed, a resume was refused, or an exact count was exceeded.
    Failed {
        failed: Vec<String>,
        resume_refused: bool,
    },
    /// No check failed, but required events fell short (35.6): not a pass.
    CoverageInsufficient {
        label: &'static str,
        missing: Vec<Shortfall>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Evaluation {
    pub coverage: Coverage,
    pub checks: Vec<Check>,
    pub verdict: TrialVerdict,
}

/// Coverage, every check and the verdict; reads the trial's files only.
pub fn evaluate(trial: &Trial, max_calls: usize) -> Evaluation {
    let coverage = coverage(trial);
    let checks = vec![
        check_readable(trial),
        check_session_path(trial),
        check_journal(trial, &coverage),
        check_logical_calls(trial, max_calls),
        check_authority(trial),
        check_effects_unknown(trial, &coverage),
        check_resume(trial),
    ];
    let mut failed: Vec<String> =
        checks.iter().filter(|check| check.status == CheckStatus::Failed).map(|check| check.name.to_owned()).collect();
    failed.extend(
        coverage
            .excess
            .iter()
            .map(|excess| format!("{} {} (actual {})", excess.event_type, excess.required, excess.actual)),
    );
    let resume_refused =
        checks.iter().any(|check| check.name == "resume_accepted" && check.status == CheckStatus::Failed);
    let verdict = if !failed.is_empty() {
        TrialVerdict::Failed { failed, resume_refused }
    } else if !coverage.missing.is_empty() {
        TrialVerdict::CoverageInsufficient { label: COVERAGE_INSUFFICIENT, missing: coverage.missing.clone() }
    } else {
        TrialVerdict::Passed
    };
    Evaluation { coverage, checks, verdict }
}

/// Failure_Class of a failed trial by its first failed check, with
/// evidence; `None` leaves the automatic (or pending) classification.
fn derived_class(trial: &Trial, evaluation: &Evaluation) -> Option<(FailureClass, Vec<EvidenceRef>)> {
    let failed =
        |name: &str| evaluation.checks.iter().any(|check| check.name == name && check.status == CheckStatus::Failed);
    let file = |path: PathBuf| EvidenceRef::File { path };
    let mut evidence = vec![file(trial.dir.join("trial.json"))];
    // Persisted Core/Harness state changed: Session, journal, call counter, authority.
    if ["session_path", "journal_unchanged", "logical_calls", "authority_hash"].iter().any(|name| failed(name)) {
        return Some((FailureClass::CoreDefect, evidence));
    }
    // The Host's receipt chain is this driver's wiring.
    if failed("effects_unknown_receipts") {
        evidence.push(file(trial.record.trial_log.clone()));
        return Some((FailureClass::WiringGap, evidence));
    }
    // A Host reply the Harness refused is a Host (wiring) defect; other refusals need a judgement.
    const HOST_REPLY: [&str; 6] = [
        "rpc_mismatch",
        "invalid_reply_fields",
        "receipt_uncommitted",
        "settlement_unknown",
        "evidence_empty",
        "call_mismatch",
    ];
    let host_refusal = trial.evidence.iter().find(|dir| {
        dir.rejected
            .as_ref()
            .and_then(|value| value["category"].as_str())
            .is_some_and(|category| HOST_REPLY.contains(&category))
    });
    host_refusal.map(|dir| {
        evidence.push(file(dir.path.join("jsonl-resume-rejected.json")));
        (FailureClass::WiringGap, evidence)
    })
}

/// `long-trial.json`.
#[derive(Clone, Debug, Serialize)]
pub struct LongTrialRecord {
    pub version: u32,
    pub checked_at: String,
    pub trial_dir: PathBuf,
    pub task_id: String,
    pub budget_key: &'static str,
    pub max_calls: usize,
    pub verdict: TrialVerdict,
    /// `passed`, `failed` or `覆盖不足`.
    pub label: &'static str,
    pub coverage: Coverage,
    pub checks: Vec<Check>,
    /// Every artifact 35.5 keeps; nothing is deleted.
    pub preserved: Vec<(String, PathRecord)>,
    pub failure_record: Option<PathBuf>,
    pub failure_record_error: Option<String>,
    pub driver_error: Option<String>,
    /// Sidecars the driver left running when it timed out or failed, and the
    /// kill that ended each one (empty when there were none).
    pub orphan_sidecars: Vec<OrphanKill>,
}

/// One sidecar left running by a failed driver, and its kill.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OrphanKill {
    pub pid: u32,
    pub command: String,
    /// Exit code of the kill command; `None` when it did not run or finish.
    pub exit_code: Option<i32>,
    pub output: String,
}

/// PIDs of the trial log's `sidecar_spawned` lines that no later
/// `sidecar_end` or `kill` line names, in spawn order. A `kill` line means
/// the driver already terminated that process; killing it again could hit a
/// reused PID. Lines that are not JSON (the torn last line of a killed
/// driver) are skipped.
pub fn orphaned_sidecars(log: &str) -> Vec<u32> {
    let mut open: Vec<u32> = Vec::new();
    for line in log.lines().filter_map(|line| serde_json::from_str::<Value>(line).ok()) {
        let Some(pid) = line["pid"].as_u64().and_then(|pid| u32::try_from(pid).ok()) else { continue };
        match line["kind"].as_str() {
            Some("sidecar_spawned") if !open.contains(&pid) => open.push(pid),
            Some("sidecar_end" | "kill") => open.retain(|known| *known != pid),
            _ => {}
        }
    }
    open
}

/// Kill every sidecar of `<trial_dir>/trial-log.jsonl` that [`orphaned_sidecars`]
/// names, with its process tree on Windows. A missing log means no sidecar
/// was spawned.
fn kill_orphans(trial_dir: &Path) -> Result<Vec<OrphanKill>> {
    let path = trial_dir.join("trial-log.jsonl");
    let log = match fs::read(&path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    Ok(orphaned_sidecars(&log)
        .into_iter()
        .map(|pid| {
            let (program, args) = if cfg!(windows) {
                ("taskkill", vec!["/PID".to_owned(), pid.to_string(), "/T".to_owned(), "/F".to_owned()])
            } else {
                ("kill", vec!["-9".to_owned(), pid.to_string()])
            };
            let mut command = Command::new(program);
            command.args(&args);
            let (exit_code, output) = match run_bounded(command, KILL_TIMEOUT) {
                Ok(Bounded::Exited { code, stdout, stderr }) => {
                    (code, String::from_utf8_lossy(&[stdout, stderr].concat()).trim().to_owned())
                }
                Ok(Bounded::TimedOut) => (None, format!("did not finish within {} s", KILL_TIMEOUT.as_secs())),
                Err(error) => (None, format!("{error:#}")),
            };
            OrphanKill { pid, command: format!("{program} {}", args.join(" ")), exit_code, output }
        })
        .collect())
}

fn first_unused(dir: &Path, stem: &str) -> PathBuf {
    (1..)
        .map(|n| if n == 1 { dir.join(format!("{stem}.json")) } else { dir.join(format!("{stem}-{n}.json")) })
        .find(|path| !path.exists())
        .expect("an unbounded range has an unused name")
}

fn preserved(trial: &Trial) -> Vec<(String, PathRecord)> {
    let record = &trial.record;
    let mut items = vec![
        ("session".to_owned(), PathRecord::of(&record.session_path, "no Session file")),
        ("journal".to_owned(), PathRecord::of(&record.session_path.with_extension("journal"), "no journal")),
        ("trial_log".to_owned(), PathRecord::of(&record.trial_log, "no trial log")),
        ("host_receipts".to_owned(), PathRecord::of(&record.host_receipts, "no Host receipts")),
        ("run_manifest".to_owned(), PathRecord::of(&trial.dir.join("run-manifest.json"), "no Run_Manifest")),
    ];
    items.extend(record.sidecar_logs.iter().map(|path| ("sidecar_log".to_owned(), PathRecord::of(path, "missing"))));
    items.extend(record.evidence_dirs.iter().map(|path| ("evidence_dir".to_owned(), PathRecord::of(path, "missing"))));
    items.extend(trial.runs().map(|run| ("events".to_owned(), PathRecord::of(&run.events.path, "missing"))));
    items
}

/// Run the checks on `trial_dir` and write `long-trial[-N].json`, plus a
/// Failure_Record `failure[-N].json` when the trial failed.
pub fn check_trial(
    trial_dir: &Path,
    max_calls: usize,
    credentials: &Credentials,
    driver_error: Option<String>,
    orphan_sidecars: Vec<OrphanKill>,
) -> Result<(LongTrialRecord, PathBuf)> {
    let trial = Trial::load(trial_dir);
    let evaluation = evaluate(&trial, max_calls);
    let (mut failure_record, mut failure_record_error) = (None, None);
    if let TrialVerdict::Failed { failed, .. } = &evaluation.verdict {
        let events: Vec<EventsFile> = trial.runs().map(|run| run.events.clone()).collect();
        let final_outcome = trial.evidence.iter().rev().find_map(|dir| dir.outcome.as_ref());
        let context = FailureContext {
            task_id: trial.record.task_id.clone(),
            run_ids: trial.runs().map(|run| run.run_id.clone()).collect(),
            stop: final_outcome
                .and_then(|(_, outcome)| serde_json::from_value::<TaskStop>(outcome["stop"].clone()).ok()),
            detail: format!("long trial failed: {}", failed.join(", ")),
            error: trial.record.error.clone().or_else(|| driver_error.clone()),
            events,
            harness_evidence: trial.record.evidence_dirs.last().cloned(),
            artifacts: PathRecord::of(&trial.record.workspace, "workspace was not created"),
            oracle: OracleRecord {
                executed: false,
                passed: None,
                public: None,
                hidden: None,
                holdout: None,
                raw_output: None,
                reason: Some("the long trial has no Oracle; its checks are in long-trial.json".into()),
            },
            registered_tools: trial.record.tools.clone(),
        };
        let mut record = FailureRecord::build(&context);
        if let Some((class, evidence)) = derived_class(&trial, &evaluation) {
            let classification = Classification::Classified { class, evidence };
            if validate_refs(&classification).is_ok() {
                record.classification = classification;
            }
        }
        let path = first_unused(trial_dir, "failure");
        let written = serde_json::to_string(&record)
            .map_err(anyhow::Error::from)
            .and_then(|text| Ok(serde_json::from_str::<Value>(&credentials.redact(&text))?))
            .and_then(|value| write_json_new(&path, &value, credentials));
        match written {
            Ok(()) => failure_record = Some(path),
            Err(error) => failure_record_error = Some(format!("{error:#}")),
        }
    }
    let label = match &evaluation.verdict {
        TrialVerdict::Passed => "passed",
        TrialVerdict::Failed { .. } => "failed",
        TrialVerdict::CoverageInsufficient { .. } => COVERAGE_INSUFFICIENT,
    };
    let record = LongTrialRecord {
        version: RECORD_VERSION,
        checked_at: timestamp_now(),
        trial_dir: trial_dir.to_owned(),
        task_id: trial.record.task_id.clone(),
        budget_key: LONG_TRIAL_BUDGET,
        max_calls,
        verdict: evaluation.verdict,
        label,
        coverage: evaluation.coverage,
        checks: evaluation.checks,
        preserved: preserved(&trial),
        failure_record,
        failure_record_error,
        driver_error,
        orphan_sidecars,
    };
    let path = first_unused(trial_dir, "long-trial");
    let value: Value = serde_json::from_str(&credentials.redact(&serde_json::to_string(&record)?))?;
    write_json_new(&path, &value, credentials)?;
    Ok((record, path))
}

// ----- the command ----------------------------------------------------------

pub struct LongTrialInputs<'a> {
    pub repo_root: &'a Path,
    /// A new trial goes to `<evidence_root>/long-trial/<stamp>/`.
    pub evidence_root: &'a Path,
    pub task_id: &'a str,
    /// JSONL_Sidecar binary (required unless `check_only`).
    pub sidecar: Option<&'a Path>,
    /// Overrides the task's registered `profile.public_check`.
    pub public_check: Option<Vec<String>>,
    pub python: &'a str,
    /// Only check an existing trial directory.
    pub check_only: Option<&'a Path>,
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
}

pub struct LongTrialReport {
    /// 0 passed, 1 failed or coverage insufficient.
    pub exit: u8,
    pub record_path: PathBuf,
    pub record: LongTrialRecord,
}

/// Execute `lite-eval long-trial`. An `Err` means no trial was started
/// (missing budget, task, public check, sidecar or credential; probe failed).
pub async fn run(inputs: LongTrialInputs<'_>) -> Result<LongTrialReport> {
    let root = inputs.repo_root;
    let (policy, identity) = load_fixed(root)?;
    let budget = *policy.budget(LONG_TRIAL_BUDGET)?;
    let token = (inputs.env)(&policy.model.credential_env).unwrap_or_default();
    let credentials = Credentials::new([token.clone()]);
    let finish = |(record, record_path): (LongTrialRecord, PathBuf)| {
        let exit = if record.verdict == TrialVerdict::Passed { 0 } else { 1 };
        LongTrialReport { exit, record_path, record }
    };
    if let Some(dir) = inputs.check_only {
        ensure!(dir.join("trial.json").is_file(), "{} has no trial.json", dir.display());
        return check_trial(dir, budget.max_calls, &credentials, None, Vec::new()).map(finish);
    }
    let set = load(&root.join(TASK_SET_PATH))?;
    let entry = set
        .tasks
        .iter()
        .find(|entry| entry.task_id == inputs.task_id)
        .cloned()
        .with_context(|| format!("task {} is not registered in {TASK_SET_PATH}", inputs.task_id))?;
    let public_check = inputs.public_check.clone().unwrap_or_else(|| entry.profile.public_check.clone());
    ensure!(
        !public_check.is_empty(),
        "task {} registers no public_check; pass --public-check so the Host can review (35.1)",
        entry.task_id
    );
    let digest = tree_digest(&entry.fixture_path)?;
    ensure!(
        digest.sha256 == entry.fixture_sha256,
        "fixture of {} has SHA-256 {}, registered {}",
        entry.task_id,
        digest.sha256,
        entry.fixture_sha256
    );
    let sidecar = inputs.sidecar.context("--sidecar <ara-agent-lite-jsonl binary> is required for a new trial")?;
    ensure!(sidecar.is_file(), "sidecar binary {} does not exist", sidecar.display());
    let driver = root.join(DRIVER_PATH);
    ensure!(driver.is_file(), "driver {} does not exist", driver.display());
    let receipt =
        probe(&policy, &token).await.map_err(|failure| {
            anyhow::anyhow!(
                credentials.redact(&format!("environment unavailable: probe failed ({:?})", failure.category()))
            )
        })?;
    let trial_dir = new_run_dir(&inputs.evidence_root.join("long-trial"))?;
    let max_output = budget.max_output.unwrap_or(policy.generation.max_output).min(8192);
    let manifest = json!({
        "version": RECORD_VERSION, "kind": "long_trial", "started_at": timestamp_now(),
        "run_policy": identity, "policy": policy, "budget_key": LONG_TRIAL_BUDGET, "budget": budget,
        "task_id": entry.task_id, "fixture_path": entry.fixture_path, "fixture_sha256": digest.sha256,
        "public_check": public_check, "max_output": max_output,
        "sidecar": {"path": sidecar, "sha256": sha256_file(sidecar)?},
        "driver": {"path": DRIVER_PATH, "sha256": sha256_file(&driver)?},
        "probe": receipt,
    });
    write_json_new(&trial_dir.join("run-manifest.json"), &manifest, &credentials)?;
    let input = trial_dir.join("trial-input.json");
    let spec = json!({
        "version": RECORD_VERSION, "task_id": entry.task_id,
        "kind": entry.kind.as_ref().map_or(json!("Coding"), |kind| json!(kind)),
        "fixture": entry.fixture_path, "public_check": public_check, "sidecar": sidecar,
        "credential_env": policy.model.credential_env,
        "model": {"endpoint": policy.model.route.endpoint, "id": policy.model.id,
                  "generation_policy": policy.generation.generation_policy,
                  "length_recovery": policy.generation.length_recovery},
        "budget": budget, "max_output": max_output,
    });
    write_json_new(&input, &spec, &credentials)?;
    let mut command = Command::new(inputs.python);
    command.arg(&driver).arg("--input").arg(&input).arg("--trial-dir").arg(&trial_dir).current_dir(root);
    let timeout = Duration::from_secs(budget.deadline_seconds) + DRIVER_MARGIN;
    let driver_error = match run_bounded(command, timeout)? {
        Bounded::TimedOut => Some(format!("driver did not finish within {} s and was killed", timeout.as_secs())),
        Bounded::Exited { code, stdout, stderr } => {
            write_output_new(&trial_dir.join("driver-stdout.log"), &stdout, &credentials)?;
            write_output_new(&trial_dir.join("driver-stderr.log"), &stderr, &credentials)?;
            (code != Some(0)).then(|| format!("driver exited with {code:?}"))
        }
    };
    // Only the driver is killed at its deadline; a sidecar it spawned can
    // outlive it. End those before the checks read the evidence.
    let (driver_error, orphans) = match driver_error {
        None => (None, Vec::new()),
        Some(error) => match kill_orphans(&trial_dir) {
            Ok(orphans) => (Some(error), orphans),
            Err(kill) => (Some(format!("{error}; orphaned sidecars not checked: {kill:#}")), Vec::new()),
        },
    };
    check_trial(&trial_dir, budget.max_calls, &credentials, driver_error, orphans).map(finish)
}
