//! `lite-eval run` (requirements 1.3–1.5, 3.1, 3.3–3.8, 4.1, 4.2).
//!
//! Check order, each before anything later can happen:
//!
//! 1. Read and parse the Run_Policy (3.7).
//! 2. Every selected task ID is registered and has a budget (3.7).
//! 3. Code version and each selected fixture SHA-256 can be computed (3.8).
//!    Failures 1–3 exit 2 and write no Run_Manifest.
//! 4. Task_Set structure and holdout log (1.3): violations are written to a
//!    Run_Manifest and the run exits 4 without any request.
//! 5. One probe (3.3); failure writes an “environment unavailable” manifest
//!    without task records and exits 3 (3.4).
//! 6. Tasks one after another in the selected order. Each fixture SHA-256 is
//!    recomputed right before the task's first upstream request; a mismatch
//!    or unreadable fixture refuses only that task (1.4).
//!
//! A task passes only when its Oracle judged it passed (4.2). Exit 0 means
//! every selected task passed; otherwise 1.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_harness::{
    core::{model::Route, session_journal::Journal},
    start_with_options, TaskKind, TaskOptions, TaskSpec, TaskStop,
};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};

pub use crate::metrics::{Counts, PathRecord};
use crate::{
    baseline,
    common::{
        new_run_dir, run_bounded, timestamp_now, write_json_new, write_new, write_output_new, Bounded, Credentials,
    },
    digest::{tree_digest, worktree_diff},
    failure::{self, FailureContext, OracleRecord},
    host::{run_id, session_paths, CapabilityHost, Features, HostConfig, PolicyModelHost, RunTemplate, WriteCounts},
    manifest::{
        CodeVersionRecord, FixtureRecord, ManifestStatus, ProbeRecord, Refusal, RunManifest, SelectedRecord, Summary,
        TaskSetRecord, TaskState, MANIFEST_VERSION,
    },
    metrics::{self, load_events, CorrectnessMetrics, TaskMetrics},
    policy::{load_fixed, PolicyIdentity, RunPolicy, TaskBudget},
    probe::probe,
    registry::{
        check_holdout_log, load, load_sealed, save, validate, HoldoutLog, SealedTask, TaskEntry, HOLDOUT_LOG_PATH,
        TASK_SET_PATH,
    },
    sealed::{self, event_records, journal_records, scan, SealedOutcome, ISOLATION},
    workspace::{assert_isolated, prepare, snapshot, Exposure, Workspace},
};

/// Which registered tasks to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    /// All registered tasks in ascending task ID order.
    All,
    /// These tasks, in this order.
    Ids(Vec<String>),
}

impl Selection {
    /// `all` or a comma-separated list of task IDs.
    pub fn parse(text: &str) -> Result<Self> {
        if text.trim() == "all" {
            return Ok(Self::All);
        }
        let ids: Vec<String> = text.split(',').map(|id| id.trim().to_owned()).collect();
        ensure!(ids.iter().all(|id| !id.is_empty()), "--tasks must be `all` or a comma-separated list of task IDs");
        Ok(Self::Ids(ids))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Correctness {
    /// The Oracle completed a judgement.
    Judged {
        passed: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        public: Option<Counts>,
        #[serde(skip_serializing_if = "Option::is_none")]
        hidden: Option<Counts>,
        #[serde(skip_serializing_if = "Option::is_none")]
        holdout: Option<Counts>,
        raw_output: Option<PathBuf>,
    },
    /// The Oracle did not run (for example the Harness never started).
    Unknown { reason: String },
    /// The Oracle ran but gave no judgement: copy failure, timeout, error.
    OracleError { reason: String },
}

impl Correctness {
    /// Requirement 4.2: success means the Oracle judged the task passed.
    pub fn passed(&self) -> bool {
        matches!(self, Correctness::Judged { passed: true, .. })
    }

    /// Requirement 4.7: without a judgement no count is written, not even 0.
    pub fn metrics(&self) -> CorrectnessMetrics {
        match self {
            Correctness::Judged { passed, public, hidden, holdout, .. } => CorrectnessMetrics {
                known: true,
                passed: Some(*passed),
                public: *public,
                hidden: *hidden,
                holdout: *holdout,
                reason: None,
            },
            Correctness::Unknown { reason } | Correctness::OracleError { reason } => CorrectnessMetrics {
                known: false,
                passed: None,
                public: None,
                hidden: None,
                holdout: None,
                reason: Some(reason.clone()),
            },
        }
    }

    /// Oracle output of a Failure_Record (5.5, 5.6).
    pub fn oracle_record(&self, stop: Option<TaskStop>) -> OracleRecord {
        match self {
            Correctness::Judged { passed, public, hidden, holdout, raw_output } => OracleRecord {
                executed: true,
                passed: Some(*passed),
                public: *public,
                hidden: *hidden,
                holdout: *holdout,
                raw_output: raw_output.clone(),
                reason: None,
            },
            Correctness::OracleError { reason } => OracleRecord {
                executed: true,
                passed: None,
                public: None,
                hidden: None,
                holdout: None,
                raw_output: None,
                reason: Some(format!("the Oracle gave no judgement: {reason}")),
            },
            Correctness::Unknown { reason } => OracleRecord {
                executed: false,
                passed: None,
                public: None,
                hidden: None,
                holdout: None,
                raw_output: None,
                reason: Some(match stop {
                    Some(stop) => format!("{reason}; Harness stop {stop:?}"),
                    None => format!("{reason}; the Harness reported no stop"),
                }),
            },
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ToolRecord {
    pub registered: Vec<String>,
    pub skipped: Vec<String>,
    /// Authorized tools that bypass the Host path checks (2.8).
    pub unchecked: Vec<String>,
}

/// What one task execution produced. The runner adds the task ID, the
/// success decision and writes it as `tasks/<id>/result.json`.
#[derive(Clone, Debug, Serialize)]
pub struct TaskRecord {
    pub stop: Option<TaskStop>,
    pub detail: String,
    pub runs: usize,
    pub run_ids: Vec<String>,
    /// Logical_Calls observed by the Harness; `None` when it never ran.
    pub logical_calls: Option<usize>,
    /// From before `start_with_options` until `TaskHandle::wait` returned.
    pub wall_ms: Option<u64>,
    pub correctness: Correctness,
    pub write_counts: WriteCounts,
    pub paths: Value,
    pub tools: ToolRecord,
    /// Host-side error that ended the task early, credential-redacted.
    pub error: Option<String>,
    /// Sealed_Holdout processing, when this task is the Sealed_Holdout.
    #[serde(skip)]
    pub sealed: Option<SealedOutcome>,
    /// The Harness `run-N-events.jsonl` files in Run order (usage and
    /// Failure_Record evidence).
    #[serde(skip)]
    pub events: Vec<PathBuf>,
    /// The Harness evidence directory, when it exists.
    #[serde(skip)]
    pub harness_evidence: Option<PathBuf>,
}

impl TaskRecord {
    /// A task that ended before the Harness ran.
    pub fn not_run(reason: String) -> Self {
        Self {
            stop: None,
            detail: String::new(),
            runs: 0,
            run_ids: Vec::new(),
            logical_calls: None,
            wall_ms: None,
            correctness: Correctness::Unknown { reason: reason.clone() },
            write_counts: WriteCounts::default(),
            paths: Value::Null,
            tools: ToolRecord::default(),
            error: Some(reason),
            sealed: None,
            events: Vec::new(),
            harness_evidence: None,
        }
    }
}

/// Everything one task execution needs. `token` is the credential read
/// once from the environment by [`run`].
pub struct TaskContext {
    pub entry: TaskEntry,
    pub budget: TaskBudget,
    /// `<run>/tasks/<id>/`, already created and empty.
    pub task_dir: PathBuf,
    pub policy: RunPolicy,
    pub policy_identity: PolicyIdentity,
    pub token: String,
    pub credentials: Credentials,
    pub repo_root: PathBuf,
    pub manifest_id: String,
    /// The sealed registration when this task is the Sealed_Holdout.
    pub sealed: Option<SealedTask>,
    /// Path of `task-set.sealed.json` (it holds the feature strings).
    pub sealed_registration: Option<PathBuf>,
}

/// Runs one task to a terminal state. The production implementation is
/// [`HarnessExecutor`]; tests substitute controlled ones.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, context: &TaskContext) -> TaskRecord;
}

pub struct RunInputs<'a> {
    pub repo_root: &'a Path,
    /// Run_Manifests go to `<evidence_root>/runs/<manifest_id>/`.
    pub evidence_root: &'a Path,
    pub selection: &'a Selection,
    pub executor: &'a dyn TaskExecutor,
    /// Environment lookup; the credential variable is read exactly once.
    pub env: &'a (dyn Fn(&str) -> Option<String> + Sync),
    /// Stop after a successful probe without starting any task.
    pub probe_only: bool,
    /// Baseline mode (7.2): the run goes to `<evidence_root>/baseline/` and
    /// an in-repo index with every file's SHA-256 is written after the manifest.
    pub baseline: bool,
}

#[derive(Clone, Debug)]
pub struct RunReport {
    /// 0, 1, 3 or 4 (2 is returned as an error instead).
    pub exit: u8,
    pub manifest_path: PathBuf,
    pub manifest: RunManifest,
}

/// Fixed objective of every task kind; the task statement is the fixture's
/// README.md. No Oracle path or case appears here.
pub fn objective(kind: TaskKind) -> &'static str {
    match kind {
        TaskKind::Coding => {
            "Complete the task described in README.md in your working directory. Inspect files with \
             ws_read_file, ws_list_dir and ws_search_files and change them only with the ws_* write tools; \
             paths are relative to the working directory. If run_public_checks is available, run it before \
             you finish and fix any failure it reports. Finish with a short summary of what you changed."
        }
        TaskKind::ReverseAnalysis => {
            "Analyze the files in your working directory as README.md describes, using only the read-only \
             tools you have. Give your final answer in exactly the format README.md requires."
        }
        TaskKind::Reasoning => {
            "Solve the task described in README.md in your working directory. Give your final answer in \
             exactly the format README.md requires."
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Whether the route is a literal loopback endpoint, i.e. a controlled fake
/// upstream. Such runs never change a Holdout_Status (2.5).
pub fn is_controlled_endpoint(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint).is_ok_and(|url| ara_agent_lite_harness::core::model::plaintext_loopback(&url))
}

/// Execute `lite-eval run`. An `Err` is a start failure (exit 2): nothing
/// was sent and no Run_Manifest exists.
pub async fn run(inputs: RunInputs<'_>) -> Result<RunReport> {
    let started_at = timestamp_now();
    let root = inputs.repo_root;
    let (policy, policy_identity) = load_fixed(root)?;
    let task_set_path = root.join(TASK_SET_PATH);
    let set_bytes = fs::read(&task_set_path).with_context(|| format!("cannot read Task_Set {TASK_SET_PATH}"))?;
    let mut set = load(&task_set_path)?;
    let mut log = HoldoutLog::load(&root.join(HOLDOUT_LOG_PATH))?;

    let registered: BTreeSet<&str> =
        set.tasks.iter().map(|entry| entry.task_id.as_str()).filter(|id| !id.is_empty()).collect();
    let selected: Vec<String> = match inputs.selection {
        Selection::All => registered.iter().map(|id| (*id).to_owned()).collect(),
        Selection::Ids(ids) => {
            let unknown: Vec<&str> = ids.iter().map(String::as_str).filter(|id| !registered.contains(id)).collect();
            ensure!(unknown.is_empty(), "task IDs not registered in {TASK_SET_PATH}: {}", unknown.join(", "));
            let distinct: BTreeSet<&String> = ids.iter().collect();
            ensure!(distinct.len() == ids.len(), "--tasks lists a task ID more than once");
            ids.clone()
        }
    };
    ensure!(!selected.is_empty(), "no task selected");
    let missing_budgets: Vec<String> =
        selected.iter().filter(|id| !policy.budgets.contains_key(*id)).map(|id| format!("/budgets/{id}")).collect();
    if !missing_budgets.is_empty() {
        return Err(crate::policy::PolicyError::Missing { items: missing_budgets }.into());
    }

    let diff = worktree_diff(root).context("cannot determine the code version (HEAD commit and worktree diff)")?;
    let mut all_tasks = Vec::new();
    for entry in &set.tasks {
        let digest = tree_digest(&entry.fixture_path);
        if selected.contains(&entry.task_id) {
            if let Err(error) = &digest {
                bail!("cannot determine the fixture SHA-256 of selected task {}: {error:#}", entry.task_id);
            }
        }
        let (sha, status) = match digest {
            Ok(digest) => (Some(digest.sha256), "ok".to_owned()),
            Err(error) => (None, format!("unavailable:{error:#}")),
        };
        all_tasks.push(FixtureRecord {
            task_id: entry.task_id.clone(),
            fixture_sha256: sha,
            fixture_status: status,
            holdout_status_at_start: serde_json::to_value(&entry.holdout)?,
        });
    }

    let violations: Vec<_> = validate(&set).into_iter().chain(check_holdout_log(&set, log.events())).collect();
    let token = (inputs.env)(&policy.model.credential_env).unwrap_or_default();
    let credentials = Credentials::new([token.clone()]);
    let run_dir = new_run_dir(&inputs.evidence_root.join(if inputs.baseline { "baseline" } else { "runs" }))?;
    let manifest_id = run_dir.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_owned();
    let diff_path = if diff.dirty {
        let path = run_dir.join("code-version").join("worktree.diff");
        fs::create_dir(path.parent().expect("has parent"))?;
        diff.save(&path, &credentials)?;
        Some(path)
    } else {
        None
    };
    let features = Features::V1;
    let mut manifest = RunManifest {
        version: MANIFEST_VERSION,
        manifest_id: manifest_id.clone(),
        status: ManifestStatus::Completed,
        started_at,
        finished_at: String::new(),
        run_policy: policy_identity.clone(),
        policy: policy.clone(),
        code_version: CodeVersionRecord {
            head: diff.head.clone(),
            dirty: diff.dirty,
            worktree_diff_sha256: diff.sha256.clone(),
            diff_rule: diff.rule.clone(),
            diff_path,
            capability_host_features: features.enabled(),
        },
        probe: ProbeRecord::NotSent,
        task_set: TaskSetRecord {
            path: TASK_SET_PATH,
            sha256: crate::common::sha256_hex(&set_bytes),
            violations: violations.clone(),
        },
        all_tasks,
        requested: selected.clone(),
        selected: Vec::new(),
        sealed_holdout: json!({"isolation": ISOLATION, "selected": false}),
        summary: Summary::new(metrics::summarize(&[])),
        baseline: None,
        baseline_checks: Vec::new(),
        limits: vec![
            "usage is what the provider adapter reported in model_result.usage_metrics; a call without \
             positive counts is unknown, never 0"
                .into(),
            "S1, S2, S3, R1 and H2 are not new holdouts; their Oracle files are readable by absolute path \
             through process tools outside the Host path checks (DR-P0-1)"
                .into(),
        ],
    };
    let finish = |manifest: &mut RunManifest, status, exit: u8| -> Result<RunReport> {
        manifest.status = status;
        // 7.4: every registered baseline is re-verified at the end of every run.
        manifest.baseline_checks = baseline::verify_all(root);
        if inputs.baseline {
            let missing = baseline::missing_judgement(manifest);
            if status == ManifestStatus::Completed && !missing.is_empty() {
                manifest.status = ManifestStatus::IncompleteBaseline;
            }
            manifest.baseline = Some(baseline::BaselineMark {
                index: baseline::index_path(&manifest.manifest_id),
                usable: manifest.status == ManifestStatus::Completed,
                missing_judgement: missing,
            });
        }
        manifest.finished_at = timestamp_now();
        let path = run_dir.join("manifest.json");
        write_json_new(&path, &manifest, &credentials)?;
        if inputs.baseline {
            baseline::write_index(root, &run_dir, manifest, &path, &credentials)?;
        }
        Ok(RunReport { exit, manifest_path: path, manifest: manifest.clone() })
    };
    if !violations.is_empty() {
        return finish(&mut manifest, ManifestStatus::TaskSetInvalid, 4);
    }

    match probe(&policy, &token).await {
        Ok(receipt) if inputs.probe_only => {
            manifest.probe = ProbeRecord::Ok { receipt };
            return finish(&mut manifest, ManifestStatus::ProbeOnly, 0);
        }
        Ok(receipt) => manifest.probe = ProbeRecord::Ok { receipt },
        Err(failure) => {
            let value = serde_json::to_value(&failure)?;
            let value: Value = serde_json::from_str(&credentials.redact(&value.to_string()))?;
            manifest.probe = ProbeRecord::Failed { category: failure.category(), failure: value };
            return finish(&mut manifest, ManifestStatus::EnvironmentUnavailable, 3);
        }
    }

    let sealed_registration = set.sealed.as_ref().map(load_sealed);
    let tasks_dir = run_dir.join("tasks");
    fs::create_dir(&tasks_dir)?;
    for task_id in &selected {
        let entry = set.tasks.iter().find(|entry| entry.task_id == *task_id).cloned();
        let entry = entry.context("selected task disappeared from the Task_Set")?;
        let task_dir = tasks_dir.join(task_id);
        fs::create_dir(&task_dir)?;
        // 1.4: the last check before this task's first upstream request.
        let actual = tree_digest(&entry.fixture_path);
        let refusal = match &actual {
            Ok(digest) if digest.sha256 == entry.fixture_sha256 => None,
            Ok(digest) => Some(Refusal {
                expected_sha256: entry.fixture_sha256.clone(),
                actual_sha256: Some(digest.sha256.clone()),
                reason: None,
            }),
            Err(error) => Some(Refusal {
                expected_sha256: entry.fixture_sha256.clone(),
                actual_sha256: None,
                reason: Some(credentials.redact(&format!("{error:#}"))),
            }),
        };
        if let Some(refusal) = refusal {
            manifest.summary.refused += 1;
            manifest.selected.push(SelectedRecord {
                task_id: task_id.clone(),
                state: TaskState::Refused,
                refusal: Some(refusal),
                result: None,
                failure_record: None,
                failure_record_error: None,
                metrics: None,
            });
            continue;
        }
        // The task judged by the sealed Oracle is the one the sealed
        // registration names; when that file is unusable, the task that has
        // no registered Oracle command of its own cannot be judged.
        let (sealed, unusable) = match &sealed_registration {
            Some(Ok(sealed)) if sealed.task_id == *task_id => (Some(sealed.clone()), None),
            Some(Err(error)) if entry.oracle.command.is_empty() => {
                (None, Some(format!("sealed registration unusable: {error:#}")))
            }
            _ => (None, None),
        };
        let record = match unusable {
            Some(reason) => TaskRecord::not_run(reason),
            None => {
                let context = TaskContext {
                    entry: entry.clone(),
                    budget: *policy.budget(task_id)?,
                    task_dir: task_dir.clone(),
                    policy: policy.clone(),
                    policy_identity: policy_identity.clone(),
                    token: token.clone(),
                    credentials: credentials.clone(),
                    repo_root: root.to_owned(),
                    manifest_id: manifest_id.clone(),
                    sealed_registration: sealed
                        .is_some()
                        .then(|| set.sealed.as_ref().map(|r| r.path.clone()))
                        .flatten(),
                    sealed,
                };
                inputs.executor.execute(&context).await
            }
        };
        if let (Some(outcome), Some(Ok(sealed))) = (&record.sealed, &sealed_registration) {
            let real_model = !is_controlled_endpoint(&policy.model.route.endpoint);
            let date = chrono::Local::now().date_naive();
            let changed = sealed::record_compromise(&mut set, &mut log, sealed, outcome, real_model, date)?;
            if changed.is_some() {
                save(&set, &task_set_path)?;
            }
            manifest.sealed_holdout = serde_json::to_value(outcome)?;
            manifest.sealed_holdout["selected"] = json!(true);
            manifest.sealed_holdout["holdout_status_changed"] = json!(changed);
        }
        let passed = record.correctness.passed();
        let events: Vec<_> = record.events.iter().map(|path| load_events(path)).collect();
        let usage = metrics::task_usage(&events, record.logical_calls);
        let cost = metrics::cost(&usage, Some(&policy.price));
        let not_produced = record.error.clone().unwrap_or_else(|| "not produced by the Harness".into());
        let paths = metrics::checked_paths(record.paths.clone(), &not_produced);
        // Requirement 5.1: every task that did not pass gets a Failure_Record.
        let failure_path = task_dir.join("failure.json");
        let failure = (!passed).then(|| {
            let artifacts = serde_json::from_value::<PathRecord>(paths["artifacts"].clone())
                .unwrap_or(PathRecord::Missing { reason: not_produced.clone() });
            let context = FailureContext {
                task_id: task_id.clone(),
                run_ids: record.run_ids.clone(),
                stop: record.stop,
                detail: record.detail.clone(),
                error: record.error.clone(),
                events: events.clone(),
                harness_evidence: record.harness_evidence.clone(),
                artifacts,
                oracle: record.correctness.oracle_record(record.stop),
                registered_tools: record.tools.registered.clone(),
            };
            failure::write(&context, &failure_path, &credentials)
                .map_err(|error| credentials.redact(&format!("cannot write the Failure_Record: {error:#}")))
        });
        let (failure_class, failure_error) = match &failure {
            None => (None, None),
            Some(Ok(written)) => (Some(written.classification.label().to_owned()), None),
            Some(Err(error)) => (None, Some(error.clone())),
        };
        let state = match (passed, &failure_error) {
            (true, _) => TaskState::Passed,
            (false, None) => TaskState::Failed,
            (false, Some(_)) => TaskState::FailureRecordMissing,
        };
        let task_metrics = TaskMetrics {
            success: passed,
            stop: record.stop.map(|stop| format!("{stop:?}")),
            correctness: record.correctness.metrics(),
            logical_calls: record.logical_calls,
            wall_ms: record.wall_ms,
            usage: metrics::UsageTotals { calls: Vec::new(), ..usage.clone() },
            cost: cost.clone(),
            write_counts: record.write_counts,
            failure_class,
        };
        let result_path = task_dir.join("result.json");
        let mut result = serde_json::to_value(&record)?;
        result["task_id"] = json!(task_id);
        result["success"] = json!(passed);
        result["usage"] = serde_json::to_value(&usage)?;
        result["cost"] = serde_json::to_value(&cost)?;
        result["paths"] = paths;
        result["failure_record"] = match &failure {
            None => Value::Null,
            Some(Ok(_)) => json!(failure_path),
            Some(Err(error)) => json!({"status": "missing", "reason": error}),
        };
        if let Some(outcome) = &record.sealed {
            result["holdout"] = json!({"compromised": outcome.compromised, "hits": outcome.leaks});
        }
        let result = serde_json::from_str::<Value>(&credentials.redact(&result.to_string()))?;
        let written = write_json_new(&result_path, &result, &credentials);
        match state {
            TaskState::Passed => manifest.summary.passed += 1,
            TaskState::FailureRecordMissing => {
                manifest.summary.failed += 1;
                manifest.summary.failure_record_missing += 1;
            }
            _ => manifest.summary.failed += 1,
        }
        manifest.selected.push(SelectedRecord {
            task_id: task_id.clone(),
            state,
            refusal: None,
            result: written.is_ok().then_some(result_path),
            failure_record: matches!(failure, Some(Ok(_))).then_some(failure_path),
            failure_record_error: failure_error,
            metrics: Some(task_metrics),
        });
    }
    let task_metrics: Vec<TaskMetrics> = manifest.selected.iter().filter_map(|record| record.metrics.clone()).collect();
    let summary = metrics::summarize(&task_metrics);
    (manifest.summary.usage, manifest.summary.cost, manifest.summary.correctness) =
        (summary.usage, summary.cost, summary.correctness);
    let all_passed = manifest.selected.iter().all(|record| record.state == TaskState::Passed);
    finish(&mut manifest, ManifestStatus::Completed, if all_passed { 0 } else { 1 })
}

/// Production executor: Capability_Test_Host over the real Harness and Core.
pub struct HarnessExecutor;

/// Every `run-N-events.jsonl` of a Harness evidence directory, by Run number.
fn events_files(evidence: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(usize, PathBuf)> = fs::read_dir(evidence)
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let path = item.ok()?.path();
            let name = path.file_name()?.to_str()?;
            let number = name.strip_prefix("run-")?.strip_suffix("-events.jsonl")?.parse().ok()?;
            Some((number, path))
        })
        .collect();
    files.sort();
    files.into_iter().map(|(_, path)| path).collect()
}

#[derive(serde::Deserialize)]
struct OracleOutput {
    status: String,
    public: Option<CountsInput>,
    hidden: Option<CountsInput>,
    reason: Option<String>,
}

#[derive(serde::Deserialize)]
struct CountsInput {
    passed: u64,
    total: u64,
}

impl From<CountsInput> for Counts {
    fn from(input: CountsInput) -> Self {
        Counts { passed: input.passed, total: input.total }
    }
}

/// Judge a registered (non-sealed) task: copy the workspace into a new
/// Oracle directory and run `<command> --artifacts <dir> --final-text <file>
/// --out <json>` from the repository root (`tools/lite-eval/oracles`).
fn judge_registered(context: &TaskContext, workspace: &Workspace, final_text: &str) -> Result<(Correctness, Value)> {
    let command = &context.entry.oracle.command;
    if command.is_empty() {
        return Ok((Correctness::Unknown { reason: "no Oracle command registered".into() }, Value::Null));
    }
    let oracle_dir = context.task_dir.join("oracle-dir");
    let before = snapshot(workspace)?;
    let copied = match sealed::copy_workspace(workspace, &oracle_dir) {
        Ok(copied) => copied,
        Err(error) => {
            let reason = format!("copy failed: {error:#}");
            let dir = PathRecord::of(&oracle_dir, "the Oracle directory was not created");
            return Ok((Correctness::OracleError { reason }, json!({"oracle_dir": dir})));
        }
    };
    let text_path = context.task_dir.join("final-text.txt");
    write_new(&text_path, context.credentials.redact(final_text).as_bytes(), &context.credentials)?;
    let out = context.task_dir.join("oracle-result.json");
    let mut process = Command::new(&command[0]);
    process
        .args(&command[1..])
        .arg("--artifacts")
        .arg(&oracle_dir)
        .arg("--final-text")
        .arg(&text_path)
        .arg("--out")
        .arg(&out)
        .current_dir(&context.repo_root)
        .env_remove(&context.policy.model.credential_env);
    let timeout = std::time::Duration::from_secs(context.policy.oracle_timeout_seconds);
    let correctness = match run_bounded(process, timeout)? {
        Bounded::TimedOut => {
            Correctness::OracleError { reason: format!("Oracle exceeded {} seconds", timeout.as_secs()) }
        }
        Bounded::Exited { code, stdout, stderr } => {
            write_output_new(&context.task_dir.join("oracle.stdout.bin"), &stdout, &context.credentials)?;
            write_output_new(&context.task_dir.join("oracle.stderr.bin"), &stderr, &context.credentials)?;
            let parsed = fs::read(&out).ok().and_then(|bytes| serde_json::from_slice::<OracleOutput>(&bytes).ok());
            match parsed {
                Some(output) if code == Some(0) && matches!(output.status.as_str(), "PASS" | "FAIL") => {
                    Correctness::Judged {
                        passed: output.status == "PASS",
                        public: output.public.map(Counts::from),
                        hidden: output.hidden.map(Counts::from),
                        holdout: None,
                        raw_output: Some(out.clone()),
                    }
                }
                Some(output) => Correctness::OracleError {
                    reason: format!(
                        "Oracle exited {code:?} with status {}: {}",
                        output.status,
                        output.reason.unwrap_or_default()
                    ),
                },
                None => Correctness::OracleError {
                    reason: format!("Oracle exited {code:?} without a parsable lite-eval-oracle-v1 result"),
                },
            }
        }
    };
    let unchanged = snapshot(workspace)? == before;
    Ok((
        correctness,
        json!({
            "oracle_dir": PathRecord::of(&oracle_dir, "the Oracle directory was not created"),
            "output": PathRecord::of(&out, "the Oracle wrote no result file"),
            "copied": copied,
            "workspace_unchanged": unchanged,
        }),
    ))
}

impl HarnessExecutor {
    async fn execute_inner(&self, context: &TaskContext) -> Result<TaskRecord> {
        let entry = &context.entry;
        let kind = entry.kind.context("task has no kind")?;
        let workspace = prepare(&entry.fixture_path, &context.task_dir)?;
        let (session_path, environment_state) = session_paths(&workspace.private);
        let policy = &context.policy;
        let features = Features::V1;
        let route = Route {
            model: policy.model.id.clone(),
            protocol: policy.model.protocol.clone(),
            binding: policy.model.route.binding.clone(),
            decision: policy.model.route.decision.clone(),
        };
        let budget = context.budget;
        let template = RunTemplate {
            endpoint: policy.model.route.endpoint.clone(),
            token: context.token.clone(),
            route,
            max_output: budget
                .max_output
                .map_or(policy.generation.max_output, |cap| cap.min(policy.generation.max_output)),
            session_path: session_path.clone(),
            environment_state,
            session_key: format!("lite-eval/{}/{}", context.manifest_id, entry.task_id),
            run_max_calls: budget.run_max_calls,
            run_max_seconds: budget.run_max_seconds,
        };
        let model_host = Arc::new(PolicyModelHost {
            generation: policy.generation.generation_policy,
            length_recovery: policy.generation.length_recovery,
            output_ceiling: features.output_ceiling,
        });
        let host = Arc::new(
            CapabilityHost::build(HostConfig {
                task: entry.clone(),
                workspace: workspace.clone(),
                features,
                template,
                model_host,
                credential_env: policy.model.credential_env.clone(),
                credentials: context.credentials.clone(),
            })
            .await?,
        );
        let tools = ToolRecord {
            registered: host.tools.iter().map(|(tool, _)| tool.schema().name.clone()).collect(),
            skipped: host.skipped_tools.clone(),
            unchecked: host.unchecked_tools.clone(),
        };
        let objective = objective(kind);
        if let Some(sealed) = &context.sealed {
            let mut texts = vec![("objective".to_owned(), objective.to_owned())];
            texts.extend(host.tool_descriptions());
            let environment = std::env::vars().filter(|(name, _)| *name != policy.model.credential_env).collect();
            let mut oracle_paths = vec![sealed.oracle_path.clone()];
            oracle_paths.extend(context.sealed_registration.clone());
            let problems =
                assert_isolated(&workspace, &oracle_paths, &sealed.features, &Exposure { texts, environment })?;
            if !problems.is_empty() {
                let mut record =
                    TaskRecord::not_run(format!("isolation check failed: {}", serde_json::to_string(&problems)?));
                record.tools = tools;
                return Ok(record);
            }
        }
        let evidence = context.task_dir.join("harness");
        let spec = TaskSpec {
            task_id: entry.task_id.clone(),
            criteria_revision: format!("lite-eval-oracle:{}", entry.fixture_sha256),
            authorization_revision: format!("{}:{}", policy.policy_id, context.policy_identity.sha256),
            kind,
            evidence_dir: evidence.clone(),
            max_runs: budget.max_runs,
            max_calls: budget.max_calls,
            deadline_unix_ms: now_ms() + budget.deadline_seconds * 1000,
        };
        let first = host.request(&run_id(&entry.task_id, 1), objective);
        let started = Instant::now();
        let handle = match start_with_options(spec, first, host.clone(), TaskOptions::default()) {
            Ok(handle) => handle,
            Err(error) => {
                let mut record =
                    TaskRecord::not_run(context.credentials.redact(&format!("Harness did not start: {error:#}")));
                record.tools = tools;
                return Ok(record);
            }
        };
        let outcome = handle.wait().await;
        let wall_ms = started.elapsed().as_millis() as u64;
        let events = events_files(&evidence);
        let final_text = outcome.last_candidate.as_ref().map(|candidate| candidate.output.clone()).unwrap_or_default();
        let (correctness, oracle, sealed_outcome) = match &context.sealed {
            Some(sealed) => {
                let mut records = if Journal::for_session(&session_path).exists() {
                    journal_records(&session_path, "session")?
                } else {
                    Vec::new()
                };
                for path in &events {
                    records.extend(event_records(path)?);
                }
                let leaks = scan(&sealed.features, &records);
                let outcome = sealed::judge(
                    &workspace,
                    sealed,
                    leaks,
                    host.unchecked_tools.clone(),
                    &context.task_dir,
                    &[policy.model.credential_env.as_str()],
                    std::time::Duration::from_secs(policy.oracle_timeout_seconds),
                )?;
                let correctness = match &outcome.correctness {
                    sealed::Correctness::Judged { passed, total } => Correctness::Judged {
                        passed: passed == total,
                        public: None,
                        hidden: None,
                        holdout: Some(Counts { passed: *passed, total: *total }),
                        raw_output: outcome.oracle_output.clone(),
                    },
                    sealed::Correctness::Unknown { reason } => Correctness::Unknown { reason: reason.clone() },
                    sealed::Correctness::OracleError { reason } => Correctness::OracleError { reason: reason.clone() },
                };
                let dir = PathRecord::of(&outcome.oracle_dir, "the Oracle directory was not created");
                (correctness, json!({"oracle_dir": dir}), Some(outcome))
            }
            None => {
                let (correctness, oracle) = judge_registered(context, &workspace, &final_text)?;
                (correctness, oracle, None)
            }
        };
        let run_ids = (1..=outcome.runs).map(|number| run_id(&entry.task_id, number)).collect();
        Ok(TaskRecord {
            stop: Some(outcome.stop),
            detail: context.credentials.redact(&outcome.detail),
            runs: outcome.runs,
            run_ids,
            logical_calls: Some(outcome.observed_calls),
            wall_ms: Some(wall_ms),
            correctness,
            write_counts: host.write_counts(),
            paths: json!({
                "artifacts": PathRecord::of(&workspace.root, "workspace was not created"),
                "session": PathRecord::of(&session_path, "no Session file was written"),
                "events": events.iter().map(|path| PathRecord::of(path, "missing")).collect::<Vec<_>>(),
                "harness_evidence": PathRecord::of(&evidence, "Harness wrote no evidence"),
                "oracle": oracle,
            }),
            tools,
            error: None,
            sealed: sealed_outcome,
            events,
            harness_evidence: evidence.exists().then_some(evidence),
        })
    }
}

#[async_trait]
impl TaskExecutor for HarnessExecutor {
    async fn execute(&self, context: &TaskContext) -> TaskRecord {
        match self.execute_inner(context).await {
            Ok(record) => record,
            Err(error) => {
                // Whatever the Harness already wrote stays referenced (5.1).
                let mut record = TaskRecord::not_run(context.credentials.redact(&format!("{error:#}")));
                let evidence = context.task_dir.join("harness");
                record.events = events_files(&evidence);
                record.harness_evidence = evidence.exists().then_some(evidence);
                record
            }
        }
    }
}
