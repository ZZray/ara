//! `lite-eval long-compact` (requirements 25.5, 25.6; task 31.5).
//!
//! Runs one registered task through Harness_Lite under the Run_Policy
//! budget `budgets.LONG-COMPACT` and writes a record of:
//!
//! * the `model_call` reservations with phase `summary` and with phase
//!   `agent` (status `reserved`), counted from the Harness events files;
//! * the Task cumulative Logical_Call count the Harness observed, which must
//!   equal their sum and stay within the budget's `max_calls`;
//! * the Session references before and after every completed compaction:
//!   the raw journal range the summary replaced (with its journal proof) and
//!   the summary installed in the context sidecar at the end of the task.
//!
//! A task that ends without a completed compaction is recorded as
//! `uncovered` with the reason, never as passed (25.6).

use std::path::{Path, PathBuf};

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::core::{context_memory, session_journal::Journal};
use serde::Serialize;
use serde_json::Value;

use crate::{
    common::{new_run_dir, sha256_file, timestamp_now, write_json_new, Credentials},
    digest::tree_digest,
    host::{session_paths, Features},
    metrics::{load_events, EventsFile},
    policy::load_fixed,
    probe::probe,
    registry::{load, TASK_SET_PATH},
    runner::{TaskContext, TaskExecutor},
};

/// Run_Policy budget key of this trial.
pub const LONG_COMPACT_BUDGET: &str = "LONG-COMPACT";
/// Task run when `--task` is not given: the largest registered coding task.
pub const DEFAULT_TASK: &str = "S3";

/// One completed `compact` event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CompactionEvent {
    /// Run number of the events file.
    pub run: usize,
    pub seq: u64,
    /// Global raw message index the summary covers up to (exclusive).
    pub covered_prefix: u64,
    /// Raw message count of the Session when the summary was installed.
    pub raw_messages: u64,
}

/// What the events files show.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct EventCounts {
    pub summary_reservations: usize,
    pub agent_reservations: usize,
    /// Reservations with any other phase (none are expected).
    pub other_reservations: usize,
    pub compactions: Vec<CompactionEvent>,
    /// Unreadable files and lines that are not event envelopes.
    pub event_errors: Vec<String>,
}

/// Count the reservations and compactions of a task's events files, in Run
/// order. Pure: no file is read.
pub fn count_events(files: &[EventsFile]) -> EventCounts {
    let mut counts = EventCounts::default();
    for file in files {
        counts.event_errors.extend(file.errors.iter().cloned());
        for line in &file.lines {
            let event = &line.event;
            match event["type"].as_str() {
                Some("model_call") if event["status"] == "reserved" => match event["phase"].as_str() {
                    Some("summary") => counts.summary_reservations += 1,
                    Some("agent") => counts.agent_reservations += 1,
                    _ => counts.other_reservations += 1,
                },
                Some("compact") if event["status"] == "completed" => counts.compactions.push(CompactionEvent {
                    run: file.run,
                    seq: line.seq,
                    covered_prefix: event["covered_prefix"].as_u64().unwrap_or(0),
                    raw_messages: event["raw_messages"].as_u64().unwrap_or(0),
                }),
                _ => {}
            }
        }
    }
    counts
}

/// Session reference of one compaction: the raw journal range it replaced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BeforeReference {
    pub run: usize,
    pub seq: u64,
    /// `[from, to)` in global raw message indexes.
    pub from: u64,
    pub to: u64,
    /// `Journal::proof(from, to)`, or why it could not be computed.
    pub journal_proof: Result<String, String>,
}

/// The summary in the context sidecar at the end of the task.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AfterReference {
    pub context_path: PathBuf,
    pub summary_hash: String,
    pub prefix_hash: String,
    pub prefix_end: usize,
    pub previous_summary_hash: Option<String>,
    pub refresh: Vec<String>,
}

/// Session references before and after compaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SessionReferences {
    pub session_path: PathBuf,
    /// SHA-256 of the Session file at the end of the task.
    pub session_sha256: Result<String, String>,
    pub before: Vec<BeforeReference>,
    pub after: Result<AfterReference, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Verdict {
    /// Compaction happened and every count check holds.
    Passed,
    /// No compaction within the budget (25.6); not a pass.
    Uncovered {
        reason: String,
    },
    Failed {
        reasons: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CountCheck {
    pub summary_reservations: usize,
    pub agent_reservations: usize,
    /// `summary_reservations + agent_reservations`.
    pub reservations_total: usize,
    /// Task cumulative Logical_Call count from the Harness; `None` when it
    /// did not report one.
    pub task_logical_calls: Option<usize>,
    pub max_calls: usize,
    pub compactions: usize,
    pub verdict: Verdict,
}

/// Requirement 25.5/25.6 decision over the counted events. A count mismatch
/// fails even without compaction; without a completed compaction and with
/// consistent counts the requirement is uncovered.
pub fn check_counts(counts: &EventCounts, task_logical_calls: Option<usize>, max_calls: usize) -> CountCheck {
    let total = counts.summary_reservations + counts.agent_reservations;
    let mut reasons = Vec::new();
    match task_logical_calls {
        None => reasons.push("the Harness reported no Task Logical_Call count".to_owned()),
        Some(task) if task != total => reasons.push(format!(
            "Task Logical_Call count {task} differs from summary {} + agent {} reservations",
            counts.summary_reservations, counts.agent_reservations
        )),
        Some(_) => {}
    }
    if total > max_calls {
        reasons.push(format!("{total} reservations exceed max_calls {max_calls}"));
    }
    if counts.other_reservations > 0 {
        reasons.push(format!("{} reservations have a phase other than summary or agent", counts.other_reservations));
    }
    reasons.extend(counts.event_errors.iter().map(|error| format!("events unreadable: {error}")));
    let verdict = if !reasons.is_empty() {
        Verdict::Failed { reasons }
    } else if counts.compactions.is_empty() {
        Verdict::Uncovered {
            reason: format!(
                "no compaction completed within the {LONG_COMPACT_BUDGET} budget ({total} of {max_calls} calls used, \
                 {} summary reservations)",
                counts.summary_reservations
            ),
        }
    } else {
        Verdict::Passed
    };
    CountCheck {
        summary_reservations: counts.summary_reservations,
        agent_reservations: counts.agent_reservations,
        reservations_total: total,
        task_logical_calls,
        max_calls,
        compactions: counts.compactions.len(),
        verdict,
    }
}

/// Session references of the counted compactions. Each compaction covers
/// the raw range from the previous one's end (0 for the first).
pub fn session_references(session_path: &Path, compactions: &[CompactionEvent]) -> SessionReferences {
    let journal = Journal::for_session(session_path);
    let mut from = 0;
    let before = compactions
        .iter()
        .map(|event| {
            let to = event.covered_prefix;
            let journal_proof = usize::try_from(from)
                .and_then(|start| usize::try_from(to).map(|end| (start, end)))
                .map_err(|error| error.to_string())
                .and_then(|(start, end)| journal.proof(start, end).map_err(|error| format!("{error:#}")));
            let reference = BeforeReference { run: event.run, seq: event.seq, from, to, journal_proof };
            from = to;
            reference
        })
        .collect();
    let context_path = context_memory::path(session_path);
    let after = std::fs::read(&context_path)
        .map_err(|error| format!("cannot read {}: {error}", context_path.display()))
        .and_then(|bytes| {
            serde_json::from_slice::<context_memory::Store>(&bytes)
                .map_err(|error| format!("{} is not a context store: {error}", context_path.display()))
        })
        .and_then(|store| store.summary.ok_or_else(|| "the context store holds no summary".to_owned()))
        .map(|summary| AfterReference {
            context_path: context_path.clone(),
            summary_hash: summary.hash,
            prefix_hash: summary.prefix_hash,
            prefix_end: summary.prefix_end,
            previous_summary_hash: summary.previous_summary_hash,
            refresh: summary.refresh,
        });
    SessionReferences {
        session_path: session_path.to_owned(),
        session_sha256: sha256_file(session_path).map_err(|error| format!("{error:#}")),
        before,
        after,
    }
}

/// `long-compact.json`.
#[derive(Clone, Debug, Serialize)]
pub struct LongCompactRecord {
    pub started_at: String,
    pub finished_at: String,
    pub task_id: String,
    pub budget_key: &'static str,
    pub budget: crate::policy::TaskBudget,
    pub run_policy: crate::policy::PolicyIdentity,
    pub capability_host_features: Vec<&'static str>,
    pub stop: Option<String>,
    pub runs: usize,
    pub run_ids: Vec<String>,
    pub events: Vec<PathBuf>,
    pub check: CountCheck,
    pub compactions: Vec<CompactionEvent>,
    pub session: SessionReferences,
    pub task_error: Option<String>,
    /// `<run>/tasks/<id>/` with the Harness evidence and Oracle outputs.
    pub task_dir: PathBuf,
}

pub struct LongCompactInputs<'a> {
    pub repo_root: &'a Path,
    /// The record goes to `<evidence_root>/long-compact/<stamp>/`.
    pub evidence_root: &'a Path,
    pub task_id: &'a str,
    pub executor: &'a dyn TaskExecutor,
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
    pub features: Features,
}

pub struct LongCompactReport {
    /// 0 passed, 1 failed or uncovered.
    pub exit: u8,
    pub record_path: PathBuf,
    pub record: LongCompactRecord,
}

/// Execute `lite-eval long-compact`. An `Err` means nothing was started
/// (missing budget, unknown task, fixture mismatch, probe failure).
pub async fn run(inputs: LongCompactInputs<'_>) -> Result<LongCompactReport> {
    let started_at = timestamp_now();
    let root = inputs.repo_root;
    let (policy, policy_identity) = load_fixed(root)?;
    let budget = *policy.budget(LONG_COMPACT_BUDGET)?;
    let set = load(&root.join(TASK_SET_PATH))?;
    let entry = set
        .tasks
        .iter()
        .find(|entry| entry.task_id == inputs.task_id)
        .cloned()
        .with_context(|| format!("task {} is not registered in {TASK_SET_PATH}", inputs.task_id))?;
    ensure!(
        !entry.oracle.command.is_empty(),
        "task {} has no registered Oracle command; the Sealed_Holdout cannot be the long-compaction task",
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
    let token = (inputs.env)(&policy.model.credential_env).unwrap_or_default();
    let credentials = Credentials::new([token.clone()]);
    probe(&policy, &token).await.map_err(|failure| {
        anyhow::anyhow!(credentials.redact(&format!("environment unavailable: probe failed ({:?})", failure.category())))
    })?;
    let run_dir = new_run_dir(&inputs.evidence_root.join("long-compact"))?;
    let manifest_id = run_dir.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned();
    let task_dir = run_dir.join("tasks").join(&entry.task_id);
    std::fs::create_dir_all(&task_dir)?;
    let context = TaskContext {
        entry: entry.clone(),
        budget,
        task_dir: task_dir.clone(),
        policy: policy.clone(),
        policy_identity: policy_identity.clone(),
        token,
        credentials: credentials.clone(),
        repo_root: root.to_owned(),
        manifest_id,
        sealed: None,
        sealed_registration: None,
        features: inputs.features,
    };
    let task = inputs.executor.execute(&context).await;
    let files: Vec<EventsFile> = task.events.iter().map(|path| load_events(path)).collect();
    let counts = count_events(&files);
    let check = check_counts(&counts, task.logical_calls, budget.max_calls);
    let (session_path, _) = session_paths(&task_dir.join("host-private"));
    let session = session_references(&session_path, &counts.compactions);
    let exit = if check.verdict == Verdict::Passed { 0 } else { 1 };
    let record = LongCompactRecord {
        started_at,
        finished_at: timestamp_now(),
        task_id: entry.task_id,
        budget_key: LONG_COMPACT_BUDGET,
        budget,
        run_policy: policy_identity,
        capability_host_features: inputs.features.enabled(),
        stop: task.stop.map(|stop| format!("{stop:?}")),
        runs: task.runs,
        run_ids: task.run_ids,
        events: task.events,
        check,
        compactions: counts.compactions,
        session,
        task_error: task.error,
        task_dir,
    };
    let record_path = run_dir.join("long-compact.json");
    let value: Value = serde_json::from_str(&credentials.redact(&serde_json::to_string(&record)?))?;
    write_json_new(&record_path, &value, &credentials)?;
    Ok(LongCompactReport { exit, record_path, record })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::metrics::EventLine;

    fn file(run: usize, events: &[Value]) -> EventsFile {
        EventsFile {
            path: PathBuf::from(format!("run-{run}-events.jsonl")),
            run,
            lines: events
                .iter()
                .enumerate()
                .map(|(index, event)| EventLine { seq: index as u64 + 1, event: event.clone() })
                .collect(),
            errors: Vec::new(),
        }
    }

    fn reserve(phase: &str) -> Value {
        json!({"type": "model_call", "phase": phase, "status": "reserved"})
    }

    #[test]
    fn counts_reservations_by_phase_and_completed_compactions() {
        let compact = json!({"type": "compact", "status": "completed", "covered_prefix": 7, "raw_messages": 12});
        let result = json!({"type": "model_result", "phase": "summary", "status": "received"});
        let files = [
            file(1, &[reserve("agent"), reserve("agent"), reserve("summary"), result, compact]),
            file(2, &[reserve("agent")]),
        ];
        let counts = count_events(&files);
        assert_eq!((counts.summary_reservations, counts.agent_reservations, counts.other_reservations), (1, 3, 0));
        assert_eq!(counts.compactions, vec![CompactionEvent { run: 1, seq: 5, covered_prefix: 7, raw_messages: 12 }]);
        assert_eq!(check_counts(&counts, Some(4), 80).verdict, Verdict::Passed);
    }

    #[test]
    fn no_compaction_is_uncovered_and_mismatches_fail() {
        let counts = count_events(&[file(1, &[reserve("agent"), reserve("agent")])]);
        let check = check_counts(&counts, Some(2), 80);
        assert!(matches!(check.verdict, Verdict::Uncovered { ref reason } if reason.contains("no compaction")));
        assert!(matches!(check_counts(&counts, Some(3), 80).verdict, Verdict::Failed { .. }));
        assert!(matches!(check_counts(&counts, None, 80).verdict, Verdict::Failed { .. }));
        assert!(matches!(check_counts(&counts, Some(2), 1).verdict, Verdict::Failed { .. }));
    }
}
