//! `lite-eval` command line for evaluation, independent judging and delivery gates.
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Parser, Subcommand};
use lite_eval::{
    common::{Credentials, EVIDENCE_ROOT},
    gate::{run_gate, save_record, CommandRunner, GatePlan, SystemRunner},
    ledger::{Append, Attempt, Blocker, CodeVersion, Ledger, State, LEDGER_SUMMARY_PATH},
    notify::{overall_progress, state_digest, Notice, NotificationRecord, Notifier, Severity, Status},
    records::{Finding, ReviewGap, ReviewRecord, ReviewScope, REVIEWS_DIR},
};

#[derive(Parser)]
#[command(name = "lite-eval", version, about = "ARA Agent Lite evaluation runner and delivery gates")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run Task_Set tasks under the fixed Run_Policy and write a Run_Manifest.
    Run(RunArgs),
    /// Validate the Task_Set and Run_Policy without sending any request.
    Validate,
    /// Record adaptive use of a task's Oracle (holdout status changes).
    #[command(subcommand)]
    Holdout(HoldoutCommand),
    /// Fork a Session at a journal prefix into two new Sessions.
    Fork(ForkArgs),
    /// Compare a run against the baseline Run_Manifest.
    Compare(CompareArgs),
    /// Append a manual Failure_Class to a Failure_Record.
    Classify(ClassifyArgs),
    /// Judge a task's artifacts with its Oracle outside the workspace.
    Oracle(OracleArgs),
    /// Run the delivery quality gate for a module.
    Gate(GateArgs),
    /// Run cargo-deny and compare against the recorded baseline.
    Deny,
    /// Record an independent review result.
    ReviewRecord(ReviewArgs),
    /// Append to, revalidate or render the Progress_Ledger.
    #[command(subcommand)]
    Ledger(LedgerCommand),
    /// Send a DingTalk progress notification for the current ledger state.
    Notify(NotifyArgs),
    /// Run the combined long-task reliability trial under `budgets.LONG-TRIAL`.
    LongTrial(LongTrialArgs),
    /// Run one compaction-heavy task through the Harness under `budgets.LONG-COMPACT`.
    LongCompact(LongCompactArgs),
    /// Write the Stage_Acceptance_Record for a phase.
    Stage(StageArgs),
}

/// Only task selection and the output location; no Run_Policy item can be
/// overridden here or through the environment (requirement 3.2).
#[derive(Args)]
struct RunArgs {
    /// `all` (ascending task ID) or a comma-separated list run in that order.
    #[arg(long, default_value = "all")]
    tasks: String,
    /// Raw outputs go to `<dir>/runs/<manifest_id>/`.
    #[arg(long, default_value = EVIDENCE_ROOT)]
    evidence_root: PathBuf,
    /// Run every pre-task check and the probe, then stop without starting a
    /// task (manifest status `probe_only`; re-check before real trials).
    #[arg(long)]
    probe_only: bool,
    /// Baseline mode: write to `<dir>/baseline/<manifest_id>/` and register
    /// the run's file SHA-256 list in the repository (requirement 7).
    #[arg(long, conflicts_with = "probe_only")]
    baseline: bool,
    /// Capability_Test_Host P2 configuration: every sub-item up to and
    /// including this one on (`P2-a` ... `P2-e`); omitted = v1, all off.
    #[arg(long, conflicts_with = "baseline")]
    p2_through: Option<String>,
}

#[derive(Args)]
struct CompareArgs {
    /// Baseline Run_Manifest; repeat for more samples.
    #[arg(long, required = true)]
    baseline: Vec<PathBuf>,
    /// Current Run_Manifest; repeat for more samples.
    #[arg(long, required = true)]
    current: Vec<PathBuf>,
    /// P2 sub-item this comparison is for (P2-a … P2-e).
    #[arg(long)]
    item: String,
    /// Output file; defaults to the in-repo comparisons directory.
    #[arg(long)]
    out: Option<PathBuf>,
}

#[derive(Args)]
struct ClassifyArgs {
    /// Failure_Record (`tasks/<id>/failure.json`); it is never modified.
    record: PathBuf,
    /// model_gap, core_defect, wiring_gap or tool_gap.
    #[arg(long)]
    class: String,
    /// `event:<events-file>#<seq>`, `file:<path>` or `log:<path>#<line>`; repeatable.
    #[arg(long = "evidence", required = true)]
    evidence: Vec<String>,
}

/// Exit 0 comparable with a valid baseline, 1 the record has no valid
/// conclusions (not comparable, environment unavailable, invalid baseline).
fn compare(args: CompareArgs) -> Result<ExitCode> {
    use lite_eval::compare::{check_item, compare, load, Conclusions, COMPARISONS_DIR};
    check_item(&args.item)?;
    let bases = args.baseline.iter().map(|path| load(path)).collect::<Result<Vec<_>>>()?;
    let currents = args.current.iter().map(|path| load(path)).collect::<Result<Vec<_>>>()?;
    let record = compare(&bases, &currents, &args.item);
    let out = match args.out {
        Some(out) => out,
        None => {
            let current = &currents.last().expect("required").reference.manifest_id;
            let dir = repo_root()?.join(COMPARISONS_DIR);
            std::fs::create_dir_all(&dir)?;
            dir.join(format!("{}-{current}.json", args.item))
        }
    };
    lite_eval::common::write_json_new(&out, &record, &credentials())?;
    println!("{}", out.display());
    println!("comparability {:?}, conclusions {:?}", record.comparability, record.conclusions);
    Ok(if record.conclusions == Conclusions::Valid { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

fn classify(args: ClassifyArgs) -> Result<ExitCode> {
    use lite_eval::failure::{classify, EvidenceRef, FailureClass};
    let class = FailureClass::parse(&args.class)?;
    let evidence = args.evidence.iter().map(|text| EvidenceRef::parse(text)).collect::<Result<Vec<_>>>()?;
    let path = classify(&args.record, class, evidence, &credentials())?;
    println!("{}", path.display());
    Ok(ExitCode::SUCCESS)
}

/// Exit 0 all passed, 1 a task failed or was refused, 2 start failure (no
/// manifest), 3 environment unavailable, 4 Task_Set invalid.
fn run(args: RunArgs) -> Result<ExitCode> {
    use lite_eval::runner::{run, HarnessExecutor, RunInputs, Selection};
    let root = repo_root()?;
    let selection = Selection::parse(&args.tasks)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let env = |name: &str| std::env::var(name).ok();
    let report = runtime.block_on(run(RunInputs {
        repo_root: &root,
        evidence_root: &args.evidence_root,
        selection: &selection,
        executor: &HarnessExecutor,
        env: &env,
        probe_only: args.probe_only,
        baseline: args.baseline,
        features: match &args.p2_through {
            Some(item) => lite_eval::host::Features::through(item)?,
            None => lite_eval::host::Features::V1,
        },
    }))?;
    let summary = &report.manifest.summary;
    println!("{}", report.manifest_path.display());
    println!(
        "status {:?}, passed {}, failed {}, refused {}, exit {}",
        report.manifest.status, summary.passed, summary.failed, summary.refused, report.exit
    );
    Ok(ExitCode::from(report.exit))
}

/// Task selection and output location only; the budget is always the
/// Run_Policy `budgets.LONG-COMPACT` (requirement 25.5).
#[derive(Args)]
struct LongCompactArgs {
    /// Registered task to run as the long compaction task.
    #[arg(long, default_value = lite_eval::long_compact::DEFAULT_TASK)]
    task: String,
    /// The record goes to `<dir>/long-compact/<stamp>/long-compact.json`.
    #[arg(long, default_value = EVIDENCE_ROOT)]
    evidence_root: PathBuf,
    /// Capability_Test_Host P2 configuration, as for `run`; omitted = v1.
    #[arg(long)]
    p2_through: Option<String>,
    // Not a doc comment: the help text is computed from `ModelCaps::budget`.
    #[arg(long, value_name = "TOKENS", help = lite_eval::long_compact::context_window_help())]
    context_window: Option<u64>,
}

/// Exit 0 compaction happened and every count check holds, 1 failed or
/// uncovered (no compaction), 2 nothing was started.
fn long_compact(args: LongCompactArgs) -> Result<ExitCode> {
    use lite_eval::long_compact::{run, LongCompactInputs};
    let root = repo_root()?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let env = |name: &str| std::env::var(name).ok();
    let report = runtime.block_on(run(LongCompactInputs {
        repo_root: &root,
        evidence_root: &args.evidence_root,
        task_id: &args.task,
        executor: &lite_eval::runner::HarnessExecutor,
        env: &env,
        features: match &args.p2_through {
            Some(item) => lite_eval::host::Features::through(item)?,
            None => lite_eval::host::Features::V1,
        },
        context_window_cap: args.context_window,
    }))?;
    println!("{}", report.record_path.display());
    println!("{}", serde_json::to_string(&report.record.check)?);
    Ok(ExitCode::from(report.exit))
}

/// Task fixture, sidecar and output location only; the budget is always the
/// Run_Policy `budgets.LONG-TRIAL` (requirement 35).
#[derive(Args)]
struct LongTrialArgs {
    /// Registered task whose fixture the trial copies.
    #[arg(long, default_value = lite_eval::long_trial::DEFAULT_TASK)]
    task: String,
    /// The trial goes to `<dir>/long-trial/<stamp>/`.
    #[arg(long, default_value = EVIDENCE_ROOT)]
    evidence_root: PathBuf,
    /// `ara-agent-lite-jsonl` binary; required for a new trial.
    #[arg(long)]
    sidecar: Option<PathBuf>,
    /// Public check argv as a JSON array; default: the task's registered `public_check`.
    #[arg(long)]
    public_check: Option<String>,
    /// Python interpreter that runs the driver.
    #[arg(long, default_value = "python")]
    python: String,
    /// Only check an existing trial directory; nothing is started.
    #[arg(long, value_name = "DIR")]
    check_only: Option<PathBuf>,
    // Not a doc comment: the help text is computed from `ModelCaps::budget`.
    #[arg(long, value_name = "TOKENS", help = lite_eval::long_trial::context_window_help())]
    context_window: Option<u64>,
    /// Trial-only additional Host review rounds; zero keeps normal public-check acceptance.
    #[arg(long, default_value_t = 0)]
    host_review_rounds: usize,
}

/// Exit 0 passed, 1 failed or coverage insufficient, 2 nothing was started.
fn long_trial(args: LongTrialArgs) -> Result<ExitCode> {
    use lite_eval::long_trial::{run, LongTrialInputs};
    let root = repo_root()?;
    let public_check = match &args.public_check {
        Some(text) => {
            Some(serde_json::from_str::<Vec<String>>(text).context("--public-check must be a JSON array of strings")?)
        }
        None => None,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let env = |name: &str| std::env::var(name).ok();
    let report = runtime.block_on(run(LongTrialInputs {
        repo_root: &root,
        evidence_root: &args.evidence_root,
        task_id: &args.task,
        sidecar: args.sidecar.as_deref(),
        public_check,
        python: &args.python,
        check_only: args.check_only.as_deref(),
        env: &env,
        context_window_cap: args.context_window,
        host_review_rounds: args.host_review_rounds,
    }))?;
    println!("{}", report.record_path.display());
    println!("{}", serde_json::to_string(&report.record.verdict)?);
    Ok(ExitCode::from(report.exit))
}

#[derive(Args)]
struct ForkArgs {
    /// Source Session state path (its `.journal` directory is read only).
    #[arg(long)]
    source: PathBuf,
    /// Working directory recorded in the source Session.
    #[arg(long)]
    cwd: PathBuf,
    /// Number of leading journal records to copy (1..=source record count).
    #[arg(long)]
    k: usize,
    /// First new Session state path; must not exist.
    #[arg(long)]
    target_a: PathBuf,
    /// Second new Session state path; must not exist.
    #[arg(long)]
    target_b: PathBuf,
}
#[derive(Args)]
struct OracleArgs {
    task_id: String,
    /// Finished workspace; judging uses a separate copy and preserves this directory.
    #[arg(long)]
    artifacts: PathBuf,
    /// New result file outside the finished workspace. Its parent must exist.
    #[arg(long)]
    out: PathBuf,
    /// Final model answer; required for reverse-analysis and reasoning tasks.
    #[arg(long)]
    final_text: Option<PathBuf>,
}

fn oracle(args: OracleArgs) -> Result<ExitCode> {
    use lite_eval::{
        registry::{load, TASK_SET_PATH},
        runner::{judge_registered, Correctness, RegisteredOracle},
        workspace::{canonical, snapshot, Workspace},
    };
    let root = repo_root()?;
    let set = load(&root.join(TASK_SET_PATH))?;
    let entry = set
        .tasks
        .iter()
        .find(|entry| entry.task_id == args.task_id)
        .with_context(|| format!("unknown task `{}`", args.task_id))?;
    ensure!(
        !entry.oracle.command.is_empty(),
        "task has no registered Oracle command; use `run` for sealed-holdout judging and isolation tracking"
    );
    ensure!(
        args.final_text.is_some() || entry.kind == Some(ara_agent_lite_harness::TaskKind::Coding),
        "task {} requires --final-text",
        entry.task_id
    );
    let final_text = args.final_text.as_ref().map(std::fs::read_to_string).transpose()?.unwrap_or_default();
    let artifacts = canonical(&args.artifacts)?;
    ensure!(artifacts.is_dir(), "--artifacts must be a directory");
    let name = args.out.file_name().context("--out must name a new file")?;
    let parent = args.out.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let out = canonical(parent)?.join(name);
    ensure!(!out.exists(), "output {} already exists", out.display());
    ensure!(!out.starts_with(&artifacts), "--out must be outside --artifacts");
    let task_dir = out.with_file_name(format!("{}-evidence", name.to_string_lossy()));
    let (policy, identity) = lite_eval::policy::load_fixed(&root)?;
    let credentials = Credentials::new(
        ["ARA_AGENT_MODEL_TOKEN", "LITE_EVAL_MODEL_TOKEN", &policy.model.credential_env]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok()),
    );
    std::fs::create_dir(&task_dir)
        .with_context(|| format!("Oracle evidence {} must not exist yet", task_dir.display()))?;
    let workspace = Workspace { root: artifacts, private: task_dir.clone() };
    let before = snapshot(&workspace)?;
    let (correctness, evidence) = judge_registered(
        &RegisteredOracle { entry, task_dir: &task_dir, repo_root: &root, policy: &policy, credentials: &credentials },
        &workspace,
        &final_text,
    )?;
    let unchanged = snapshot(&workspace)? == before;
    let exit = match correctness {
        Correctness::Judged { passed: true, .. } if unchanged => 0,
        Correctness::Judged { .. } if unchanged => 1,
        _ => 2,
    };
    lite_eval::common::write_json_new(
        &out,
        &serde_json::json!({
            "schema": "lite-eval-independent-oracle-v1", "task_id": entry.task_id,
            "run_policy": identity, "correctness": correctness, "evidence": evidence,
            "artifacts": workspace.root, "artifact_sha256": before, "workspace_unchanged": unchanged,
        }),
        &credentials,
    )?;
    println!("{}", out.display());
    Ok(ExitCode::from(exit))
}

#[derive(Subcommand)]
enum HoldoutCommand {
    /// Mark a task `not new holdout` (only once; status and date never change).
    Mark(MarkArgs),
    /// Register the Sealed_Holdout as `new holdout` before its first real Run.
    Register(RegisterArgs),
}

#[derive(Args)]
struct RegisterArgs {
    task_id: String,
    /// Registration date, YYYY-MM-DD.
    #[arg(long)]
    date: chrono::NaiveDate,
}

#[derive(Args)]
struct MarkArgs {
    task_id: String,
    /// First adaptive use, YYYY-MM-DD.
    #[arg(long)]
    date: chrono::NaiveDate,
    /// Commit ID or change note; at least one, repeatable.
    #[arg(long = "change", required = true)]
    changes: Vec<String>,
    #[arg(long)]
    reason: Option<String>,
}

/// Task_Set structure and holdout-log consistency (1.3, 1.7). Exit 4 lists
/// every violation, one JSON object per line; unreadable files exit 2.
fn validate_task_set() -> Result<ExitCode> {
    use lite_eval::registry::{check_holdout_log, load, validate, HoldoutLog, HOLDOUT_LOG_PATH, TASK_SET_PATH};
    let root = repo_root()?;
    let set = load(&root.join(TASK_SET_PATH))?;
    let log = HoldoutLog::load(&root.join(HOLDOUT_LOG_PATH))?;
    let violations: Vec<_> = validate(&set).into_iter().chain(check_holdout_log(&set, log.events())).collect();
    for violation in &violations {
        println!("{}", serde_json::to_string(violation)?);
    }
    eprintln!("lite-eval: {} tasks, {} violations", set.tasks.len(), violations.len());
    let (_, identity) = lite_eval::policy::load_fixed(&root)?;
    eprintln!("lite-eval: Run_Policy {} sha256 {} ({:?})", identity.path, identity.sha256, identity.git_state);
    Ok(if violations.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(4) })
}

fn holdout(command: HoldoutCommand) -> Result<ExitCode> {
    use lite_eval::registry::{load, mark_not_new, register_new, save, HoldoutLog, HOLDOUT_LOG_PATH, TASK_SET_PATH};
    let root = repo_root()?;
    let path = root.join(TASK_SET_PATH);
    let mut set = load(&path)?;
    let mut log = HoldoutLog::load(&root.join(HOLDOUT_LOG_PATH))?;
    match command {
        HoldoutCommand::Mark(args) => {
            mark_not_new(&mut set, &mut log, &args.task_id, args.date, args.changes, args.reason)?
        }
        HoldoutCommand::Register(args) => register_new(&mut set, &mut log, &args.task_id, args.date)?,
    }
    // The log line is already durable; a failed save leaves a mismatch that
    // `validate` reports, never a silently reverted status.
    save(&set, &path)?;
    println!("{}", serde_json::to_string(log.events().last().context("log is empty after marking")?)?);
    Ok(ExitCode::SUCCESS)
}

#[derive(Subcommand)]
enum LedgerCommand {
    /// Append one entry at the current delivery code version.
    Append(AppendArgs),
    /// Demote verified modules whose delivery diff changed.
    Revalidate,
    /// Regenerate progress-ledger.md.
    Render,
}

#[derive(Args)]
struct AppendArgs {
    #[arg(long)]
    module: String,
    /// open, done or verified.
    #[arg(long, value_parser = parse_state)]
    state: State,
    /// Evidence file or directory, repository-relative or absolute.
    #[arg(long)]
    evidence: String,
    #[arg(long, default_value = "")]
    uncovered: String,
    #[arg(long, default_value_t = 0)]
    criteria_done: u32,
    #[arg(long)]
    gate_record: Option<String>,
    #[arg(long = "review-record")]
    review_records: Vec<String>,
    #[arg(long)]
    blocker_reason: Option<String>,
    /// `action => result`, repeatable.
    #[arg(long = "attempt")]
    attempts: Vec<String>,
    #[arg(long)]
    decision: Option<String>,
    /// Record the entry without the automatic DingTalk notice (backfills).
    #[arg(long)]
    no_notify: bool,
}

#[derive(Args)]
struct NotifyArgs {
    /// Info, Milestone, Decision or Blocker.
    #[arg(long, default_value = "Info", value_parser = parse_severity)]
    severity: Severity,
    #[arg(long)]
    title: String,
    /// Markdown body.
    #[arg(long)]
    message: String,
    /// Extra digest input so a Decision notice is not taken for a repeat.
    #[arg(long)]
    topic: Option<String>,
}

fn parse_severity(text: &str) -> Result<Severity, String> {
    match text {
        "Info" => Ok(Severity::Info),
        "Milestone" => Ok(Severity::Milestone),
        "Decision" => Ok(Severity::Decision),
        "Blocker" => Ok(Severity::Blocker),
        _ => Err(format!("severity must be Info, Milestone, Decision or Blocker, not `{text}`")),
    }
}

/// Report a notice outcome on the console; failures are never retried.
fn report(record: Option<NotificationRecord>) {
    match record {
        None => eprintln!("lite-eval: notice skipped: module states and blockers unchanged since the last notice"),
        Some(record) if record.status == Status::Delivered => {
            eprintln!("lite-eval: notice {} delivered (errcode 0)", record.seq)
        }
        Some(record) => eprintln!(
            "lite-eval: notice {} {:?} ({}); not retried",
            record.seq,
            record.status,
            record.reason_category.as_deref().or(record.errcode.as_deref()).unwrap_or("unknown")
        ),
    }
}

#[derive(Args)]
struct ReviewArgs {
    /// Label used in the record file name, e.g. a checkpoint name.
    #[arg(long)]
    scope: String,
    /// Modules the review covers; repeatable.
    #[arg(long = "module")]
    modules: Vec<String>,
    /// codex, ara-git-review, ara-rust-core-review or ara-provider-review.
    #[arg(long)]
    tool: String,
    /// Reviewed base commit (defaults to HEAD).
    #[arg(long)]
    base: Option<String>,
    /// Reviewed files, repository-relative; repeatable.
    #[arg(long = "file")]
    files: Vec<String>,
    /// Findings file: JSON array of {location, description, disposition, reason}.
    #[arg(long)]
    findings: Option<PathBuf>,
    /// Record a review gap instead of findings.
    #[arg(long)]
    gap: Option<String>,
    #[arg(long)]
    session_id: Option<String>,
    #[arg(long)]
    implementation_session_id: Option<String>,
    /// When the review started (ISO 8601 with offset); defaults to now.
    #[arg(long)]
    started_at: Option<String>,
}

fn review_record(args: ReviewArgs) -> Result<ExitCode> {
    let root = repo_root()?;
    ensure!(!args.modules.is_empty(), "name at least one --module");
    for module in &args.modules {
        ensure!(lite_eval::ledger::module(module).is_some(), "unknown module `{module}`");
    }
    let credentials = credentials();
    let version = CodeVersion::current(&root)?;
    let findings: Vec<Finding> = match &args.findings {
        Some(path) => serde_json::from_slice(&std::fs::read(path)?)
            .with_context(|| format!("findings file {} is not a JSON array of findings", path.display()))?,
        None => Vec::new(),
    };
    ensure!(args.gap.is_none() || findings.is_empty(), "a review gap carries no findings");
    let now = lite_eval::common::timestamp_now();
    let record = ReviewRecord {
        tool: args.tool,
        scope: ReviewScope {
            base: args.base.unwrap_or_else(|| version.head.clone()),
            target: "worktree".into(),
            files: args.files,
        },
        diff_sha256: version.diff_sha256.clone(),
        session_id: args.session_id,
        implementation_session_id: args.implementation_session_id,
        findings,
        gap: args.gap.map(|reason| ReviewGap { reason }),
        started_at: args.started_at.unwrap_or_else(|| now.clone()),
        finished_at: now,
    };
    lite_eval::common::parse_timestamp(&record.started_at)?;
    let complete = record.check_complete();
    let diff = version.diff_sha256.as_deref().and_then(|sha| sha.get(..8)).unwrap_or("clean");
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let relative = format!("{REVIEWS_DIR}/{}-{diff}-{}-{stamp}.json", args.scope, record.tool);
    lite_eval::common::write_json_new(&root.join(&relative), &record, &credentials)?;
    println!("{relative}");
    if let Err(error) = complete {
        // Requirement 32.3: record the gap on every covered module. The module
        // keeps its current open/done state; the gap only blocks `verified`.
        let ledger = Ledger::in_repo(&root);
        for module in args.modules {
            let entries = ledger.entries()?;
            let latest = entries.iter().rev().find(|entry| entry.module == module);
            let state = match latest.map(|entry| entry.state) {
                Some(State::Done | State::Verified) => State::Done,
                _ => State::Open,
            };
            ledger.append(
                Append {
                    module,
                    state,
                    evidence: relative.clone(),
                    uncovered: format!("审查缺口：{error:#}"),
                    code_version: version.clone(),
                    criteria_done: latest.map_or(0, |entry| entry.criteria.done),
                    gate_record: None,
                    review_records: vec![relative.clone()],
                    blocker: None,
                },
                &credentials,
            )?;
        }
        eprintln!("lite-eval: review gap recorded on the ledger: {error:#}");
    }
    Ok(ExitCode::SUCCESS)
}

#[derive(Args)]
struct StageArgs {
    /// P0 … P5.
    #[arg(long)]
    phase: String,
}

fn stage(args: StageArgs) -> Result<ExitCode> {
    use lite_eval::stage::{build_from_repo, render, STAGE_DIR};
    ensure!(
        matches!(args.phase.as_str(), "P0" | "P1" | "P2" | "P3" | "P4" | "P5"),
        "phase must be P0 … P5, not `{}`",
        args.phase
    );
    let root = repo_root()?;
    let record = build_from_repo(&root, &args.phase)?;
    let base = root.join(STAGE_DIR).join(format!("stage-acceptance-{}", args.phase));
    // A derived view regenerated at each phase end; run evidence is never overwritten.
    std::fs::write(base.with_extension("json"), serde_json::to_vec_pretty(&record)?)?;
    std::fs::write(base.with_extension("md"), render(&record))?;
    println!("{}", base.with_extension("md").display());
    println!("covered {}, uncovered {}", record.covered.len(), record.uncovered.len());
    Ok(ExitCode::SUCCESS)
}

fn deny() -> Result<ExitCode> {
    use lite_eval::deny::{
        baseline_failures, baseline_packages, compare, current_packages, run_checks, CheckStatus, DenyRecord,
        BASELINE_PATH, DENY_DIR,
    };
    let root = repo_root()?;
    let credentials = credentials();
    let code_version = CodeVersion::current(&root)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let out_dir = Path::new(EVIDENCE_ROOT).join("deny").join(&stamp);
    let checks = run_checks(&root, &out_dir, &SystemRunner, &credentials)?;
    let failures: Vec<_> = checks.iter().flat_map(|check| check.failures.iter().cloned()).collect();
    let (base_packages, package_baseline_missing) = baseline_packages(&root)?;
    let comparison =
        compare(&baseline_failures(&root.join(BASELINE_PATH))?, &failures, &base_packages, &current_packages(&root)?);
    let not_executed: Vec<String> = checks
        .iter()
        .filter(|check| check.status == CheckStatus::NotExecuted)
        .map(|check| format!("{} {}：{}", check.manifest, check.check, check.reason.as_deref().unwrap_or("未知")))
        .collect();
    let record = DenyRecord { code_version: code_version.clone(), checks, comparison, package_baseline_missing };
    let diff = code_version.diff_sha256.as_deref().map_or("clean", |sha| &sha[..8]);
    let relative = format!("{DENY_DIR}/{diff}-{stamp}.json");
    std::fs::create_dir_all(root.join(DENY_DIR))?;
    lite_eval::common::write_json_new(&root.join(&relative), &record, &credentials)?;
    println!("{relative}");
    println!(
        "known {}, new {}, new dependencies {}, not executed {}",
        record.comparison.known_failures.len(),
        record.comparison.new_failures.len(),
        record.comparison.new_dependencies.len(),
        not_executed.len()
    );
    for key in &record.comparison.new_failures {
        println!("new failure: {} {} {} {}", key.check, key.crate_name, key.version, key.rule_or_advisory_id);
    }
    if !not_executed.is_empty() {
        // Requirement 34.5: an incomplete audit is a ledger gap, never a pass.
        let entries = Ledger::in_repo(&root).entries()?;
        let done =
            entries.iter().rev().find(|entry| entry.module == "M-P5-DENY").map_or(0, |entry| entry.criteria.done);
        Ledger::in_repo(&root).append(
            Append {
                module: "M-P5-DENY".into(),
                state: State::Open,
                evidence: relative,
                uncovered: format!("依赖审计未执行：{}", not_executed.join("；")),
                code_version,
                criteria_done: done,
                gate_record: None,
                review_records: Vec::new(),
                blocker: None,
            },
            &credentials,
        )?;
    }
    let clean = record.comparison.new_failures.is_empty() && not_executed.is_empty();
    Ok(if clean { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// Without a part flag the gate runs `--quality` only. Each selected part
/// writes its own record; the exit code is 0 only when every part passed.
#[derive(Args)]
struct GateArgs {
    /// Module the gate record belongs to.
    #[arg(long)]
    module: String,
    /// The 11 fmt/clippy/test commands of requirement 33.1.
    #[arg(long)]
    quality: bool,
    /// Run every Lite test referenced by support-matrix.json (30.4, 30.5).
    #[arg(long)]
    matrix: bool,
    /// Remove one stable item from a temporary Core copy and require the
    /// stability test to stop compiling (29.5).
    #[arg(long)]
    stability_demo: bool,
    /// Build, lint and scan tools/lite-consumer-check (31.2, 31.7).
    #[arg(long)]
    consumer: bool,
}

/// Build the JSONL sidecar so the Python tests use this build's binary.
fn build_jsonl(root: &Path) -> Result<PathBuf, String> {
    let crate_dir = root.join("crates/ara-agent-lite-jsonl");
    let build = SystemRunner.run(&crate_dir, "cargo", &["build".to_owned()], &[]);
    if build.exit_code != Some(0) {
        return Err(format!("cargo build exited {:?}", build.exit_code));
    }
    let metadata = SystemRunner.run(
        &crate_dir,
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps"].map(str::to_owned),
        &[],
    );
    let target = serde_json::from_slice::<serde_json::Value>(&metadata.stdout)
        .ok()
        .and_then(|value| value["target_directory"].as_str().map(PathBuf::from))
        .ok_or("cargo metadata gave no target directory")?;
    let binary = target.join("debug").join(format!("ara-agent-lite-jsonl{}", std::env::consts::EXE_SUFFIX));
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(format!("built binary missing at {}", binary.display()))
    }
}

/// Target directory cargo uses for `crate_dir`, so the Core copy of the
/// stability demo reuses already built dependencies.
fn target_dir(crate_dir: &Path) -> Option<PathBuf> {
    let metadata = SystemRunner.run(
        crate_dir,
        "cargo",
        &["metadata", "--format-version", "1", "--no-deps", "--offline"].map(str::to_owned),
        &[],
    );
    serde_json::from_slice::<serde_json::Value>(&metadata.stdout)
        .ok()
        .and_then(|value| value["target_directory"].as_str().map(PathBuf::from))
}

fn gate(args: GateArgs) -> Result<ExitCode> {
    use lite_eval::gate::{
        consumer::{run_consumer, ConsumerPlan},
        matrix::{run_matrix, Matrix, MatrixPlan, MATRIX_PATH},
        save_part,
        stability::{run_demo, DemoPlan, CORE_DIR},
    };
    let root = repo_root()?;
    ensure!(lite_eval::ledger::module(&args.module).is_some(), "unknown module `{}`", args.module);
    let quality = args.quality || !(args.matrix || args.stability_demo || args.consumer);
    let credentials = credentials();
    let start = CodeVersion::current(&root)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let output_dir = Path::new(EVIDENCE_ROOT).join("gates").join(format!("{}-{stamp}", args.module));
    let python_evidence = output_dir.join("python-evidence");
    std::fs::create_dir_all(&python_evidence)?;
    let needs_jsonl = quality || args.matrix;
    let jsonl_binary = if needs_jsonl { build_jsonl(&root) } else { Err("not built".to_owned()) };
    let mut failures = Vec::new();
    let mut paths = Vec::new();

    if quality {
        let plan = GatePlan {
            repo_root: &root,
            module: &args.module,
            code_version: start.clone(),
            output_dir: output_dir.clone(),
            jsonl_binary: jsonl_binary.clone(),
            python_evidence: python_evidence.clone(),
        };
        let record = run_gate(&plan, &SystemRunner, &credentials)?;
        paths.push(save_record(&root, &record, &stamp, &credentials)?);
        failures.extend(record.check_passes().err());
    }
    if args.matrix {
        let bytes = std::fs::read(root.join(MATRIX_PATH)).with_context(|| format!("cannot read {MATRIX_PATH}"))?;
        let matrix: Matrix = serde_json::from_slice(&bytes).with_context(|| format!("invalid {MATRIX_PATH}"))?;
        let plan = MatrixPlan {
            repo_root: &root,
            module: &args.module,
            code_version: start.clone(),
            matrix: &matrix,
            matrix_sha256: lite_eval::common::sha256_hex(&bytes),
            output_dir: output_dir.join("matrix"),
            jsonl_binary: jsonl_binary.clone(),
            python_evidence: python_evidence.join("matrix"),
        };
        let record = run_matrix(&plan, &SystemRunner, &credentials)?;
        paths.push(save_part(&root, &args.module, "matrix", &start, &stamp, &record, &credentials)?);
        failures.extend(record.check_passes().err());
    }
    if args.stability_demo {
        let work_dir = std::env::temp_dir().join(format!("lite-eval-stability-{stamp}-{}", std::process::id()));
        let env = match std::env::var_os("CARGO_TARGET_DIR") {
            Some(_) => Vec::new(),
            None => target_dir(&root.join(CORE_DIR))
                .map(|dir| vec![("CARGO_TARGET_DIR".to_owned(), dir.into_os_string())])
                .unwrap_or_default(),
        };
        let plan = DemoPlan {
            repo_root: &root,
            module: &args.module,
            code_version: start.clone(),
            work_dir: work_dir.clone(),
            output_dir: output_dir.join("stability-demo"),
            env,
        };
        let record = run_demo(&plan, &SystemRunner, &credentials);
        // The copy is scratch space; its outputs are already saved.
        let _ = std::fs::remove_dir_all(&work_dir);
        let record = record?;
        paths.push(save_part(&root, &args.module, "stability-demo", &start, &stamp, &record, &credentials)?);
        if !record.passed {
            failures.push(anyhow::anyhow!(
                "stability demo did not pass: baseline exit {:?}, patched exit {:?}, failure names `{}`: {}",
                record.baseline.exit_code,
                record.patched.exit_code,
                record.patch.item,
                record.failure_names_item
            ));
        }
    }
    if args.consumer {
        let plan = ConsumerPlan {
            repo_root: &root,
            module: &args.module,
            code_version: start.clone(),
            output_dir: output_dir.join("consumer"),
        };
        let record = run_consumer(&plan, &SystemRunner, &credentials)?;
        paths.push(save_part(&root, &args.module, "consumer", &start, &stamp, &record, &credentials)?);
        failures.extend(record.check_passes().err());
    }

    for path in &paths {
        println!("{path}");
    }
    ensure!(CodeVersion::current(&root)? == start, "code changed while the gate ran; records {paths:?} are stale");
    if failures.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    for error in failures {
        eprintln!("lite-eval: {error:#}");
    }
    Ok(ExitCode::from(1))
}

fn notify(args: NotifyArgs) -> Result<ExitCode> {
    let root = repo_root()?;
    let entries = Ledger::in_repo(&root).entries()?;
    let latest = entries.last().context("the ledger is empty; append an entry first")?;
    let module = lite_eval::ledger::module(&latest.module).context("unknown module in ledger")?;
    let notice = Notice {
        severity: args.severity,
        title: args.title,
        message: args.message,
        progress: format!("{} {}/{}", module.title, latest.criteria.done, latest.criteria.total),
        overall_progress: overall_progress(&entries),
    };
    let digest = state_digest(&entries, args.topic.as_deref());
    report(Notifier::in_repo(&root, credentials()).send(&notice, &digest, Some(latest.seq))?);
    Ok(ExitCode::SUCCESS)
}

fn parse_state(text: &str) -> Result<State, String> {
    match text {
        "open" => Ok(State::Open),
        "done" => Ok(State::Done),
        "verified" => Ok(State::Verified),
        _ => Err(format!("state must be open, done or verified, not `{text}`")),
    }
}

/// Requirement 6: only creates the two Sessions; never starts a Run on them.
fn fork(args: ForkArgs) -> Result<ExitCode> {
    let receipt = ara_agent_lite_harness::core::session_fork::fork_session(
        &args.source,
        &args.cwd,
        args.k,
        [&args.target_a, &args.target_b],
    )?;
    println!("{}", serde_json::to_string(&receipt)?);
    Ok(ExitCode::SUCCESS)
}

/// Top of the git working tree containing the current directory.
fn repo_root() -> Result<PathBuf> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .context("cannot run git")?;
    ensure!(output.status.success(), "the current directory is not inside a git working tree");
    Ok(PathBuf::from(String::from_utf8(output.stdout).context("git root is not UTF-8")?.trim()))
}

/// Known credential values that must never reach evidence.
fn credentials() -> Credentials {
    Credentials::new(
        ["ARA_AGENT_MODEL_TOKEN", "LITE_EVAL_MODEL_TOKEN"].into_iter().filter_map(|name| std::env::var(name).ok()),
    )
}

fn blocker(args: &AppendArgs) -> Result<Option<Blocker>> {
    let attempts = args
        .attempts
        .iter()
        .map(|text| {
            let (action, result) = text.split_once("=>").context("--attempt must be `action => result`")?;
            Ok(Attempt { action: action.trim().to_owned(), result: result.trim().to_owned() })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(match (&args.blocker_reason, &args.decision) {
        (None, None) if attempts.is_empty() => None,
        (Some(reason), Some(decision)) => {
            Some(Blocker { reason: reason.clone(), attempts, decision_needed: decision.clone() })
        }
        _ => bail!("a blocker needs --blocker-reason, at least one --attempt and --decision"),
    })
}

fn ledger(command: LedgerCommand) -> Result<ExitCode> {
    let root = repo_root()?;
    let ledger = Ledger::in_repo(&root);
    let credentials = credentials();
    match command {
        LedgerCommand::Append(args) => {
            let blocker = blocker(&args)?;
            let entry = ledger.append(
                Append {
                    module: args.module,
                    state: args.state,
                    evidence: args.evidence,
                    uncovered: args.uncovered,
                    code_version: CodeVersion::current(&root)?,
                    criteria_done: args.criteria_done,
                    gate_record: args.gate_record,
                    review_records: args.review_records,
                    blocker,
                },
                &credentials,
            )?;
            println!("{}", serde_json::to_string(&entry)?);
            if !args.no_notify {
                report(Notifier::in_repo(&root, credentials.clone()).on_ledger_change(&ledger.entries()?)?);
            }
        }
        LedgerCommand::Revalidate => {
            for entry in ledger.revalidate(&credentials)? {
                println!("{}", serde_json::to_string(&entry)?);
            }
        }
        LedgerCommand::Render => {}
    }
    // Every ledger command leaves the summary in step with the ledger.
    ledger.render_to(&root.join(Path::new(LEDGER_SUMMARY_PATH)), &credentials)?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Run(args) => run(args),
        Command::Validate => validate_task_set(),
        Command::Holdout(command) => holdout(command),
        Command::Fork(args) => fork(args),
        Command::Compare(args) => compare(args),
        Command::Classify(args) => classify(args),
        Command::Oracle(args) => oracle(args),
        Command::Gate(args) => gate(args),
        Command::Deny => deny(),
        Command::ReviewRecord(args) => review_record(args),
        Command::Ledger(command) => ledger(command),
        Command::Notify(args) => notify(args),
        Command::LongTrial(args) => long_trial(args),
        Command::LongCompact(args) => long_compact(args),
        Command::Stage(args) => stage(args),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("lite-eval: {error:#}");
            ExitCode::from(2)
        }
    }
}
