//! Run_Manifest (`<evidence_root>/runs/<manifest_id>/manifest.json`, design
//! “Run_Manifest”). Written once per run with `create_new`, never updated.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::Value;

use crate::{digest::DiffRule, policy::PolicyIdentity, probe::ProbeReceipt, registry::Violation};

pub const MANIFEST_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    /// Every selected task reached a terminal state (passed, failed or refused).
    Completed,
    /// The probe failed; no task was started and no task result exists (3.4).
    EnvironmentUnavailable,
    /// The Task_Set failed its structure checks; nothing was sent (1.3).
    TaskSetInvalid,
    /// `--probe-only`: every pre-task check and the probe passed; no task
    /// was started (route re-check before real-model trials, DR-P0-3).
    ProbeOnly,
}

#[derive(Clone, Debug, Serialize)]
pub struct CodeVersionRecord {
    pub head: String,
    pub dirty: bool,
    pub worktree_diff_sha256: Option<String>,
    pub diff_rule: DiffRule,
    /// The saved complete diff; `None` when the tree is clean.
    pub diff_path: Option<PathBuf>,
    /// P2 sub-items switched on in the Capability_Test_Host.
    pub capability_host_features: Vec<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProbeRecord {
    Ok {
        #[serde(flatten)]
        receipt: ProbeReceipt,
    },
    Failed {
        category: &'static str,
        /// The typed failure, credential-redacted.
        failure: Value,
    },
    /// The run stopped before the probe (Task_Set invalid).
    NotSent,
}

#[derive(Clone, Debug, Serialize)]
pub struct TaskSetRecord {
    pub path: &'static str,
    pub sha256: String,
    pub violations: Vec<Violation>,
}

/// One registered task at run start (1.5): every task, selected or not.
#[derive(Clone, Debug, Serialize)]
pub struct FixtureRecord {
    pub task_id: String,
    pub fixture_sha256: Option<String>,
    /// `ok`, or `unavailable:<reason>`.
    pub fixture_status: String,
    pub holdout_status_at_start: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// The Oracle judged the task passed.
    Passed,
    /// Anything else that ran: Oracle failure, Oracle error, unknown correctness.
    Failed,
    /// Not run: the fixture SHA-256 differed or was unreadable before the
    /// first upstream request (1.4); neither passed nor failed.
    Refused,
}

#[derive(Clone, Debug, Serialize)]
pub struct Refusal {
    pub expected_sha256: String,
    pub actual_sha256: Option<String>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SelectedRecord {
    pub task_id: String,
    pub state: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<Refusal>,
    /// `tasks/<id>/result.json`, when written.
    pub result: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Summary {
    pub passed: usize,
    pub failed: usize,
    pub refused: usize,
    /// Usage, cost and correctness aggregation belong to M-P0-METRICS
    /// (task 14); until it lands they are recorded as not computed, never 0.
    pub usage: Value,
    pub cost: Value,
}

#[derive(Clone, Debug, Serialize)]
pub struct RunManifest {
    pub version: u32,
    pub manifest_id: String,
    pub status: ManifestStatus,
    pub started_at: String,
    pub finished_at: String,
    pub run_policy: PolicyIdentity,
    pub code_version: CodeVersionRecord,
    pub probe: ProbeRecord,
    pub task_set: TaskSetRecord,
    pub all_tasks: Vec<FixtureRecord>,
    /// Task IDs in run order, as selected by the command.
    pub requested: Vec<String>,
    /// Terminal state of each started or refused task; empty when the
    /// environment was unavailable or the Task_Set invalid.
    pub selected: Vec<SelectedRecord>,
    pub sealed_holdout: Value,
    pub summary: Summary,
    /// Known limits of this run's evidence.
    pub limits: Vec<String>,
}
