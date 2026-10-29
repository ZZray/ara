//! `lite-eval` command line. Every subcommand from the design CLI is present;
//! the ones whose spec task has not landed fail with an explicit error.
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
    Run(Pending),
    /// Validate the Task_Set and Run_Policy without sending any request.
    Validate,
    /// Record adaptive use of a task's Oracle (holdout status changes).
    #[command(subcommand)]
    Holdout(HoldoutCommand),
    /// Fork a Session at a journal prefix into two new Sessions.
    Fork(ForkArgs),
    /// Compare a run against the baseline Run_Manifest.
    Compare(Pending),
    /// Append a manual Failure_Class to a Failure_Record.
    Classify(Pending),
    /// Judge a task's artifacts with its Oracle outside the workspace.
    Oracle(Pending),
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
    /// Run the combined long-task reliability trial.
    LongTrial(Pending),
    /// Write the Stage_Acceptance_Record for a phase.
    Stage(StageArgs),
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
/// Arguments of a subcommand whose implementation has not landed yet.
#[derive(Args)]
struct Pending {
    /// Accepted and ignored until the subcommand is implemented.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "ARGS")]
    _args: Vec<String>,
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

#[derive(Args)]
struct GateArgs {
    /// Module the gate record belongs to.
    #[arg(long)]
    module: String,
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

fn gate(args: GateArgs) -> Result<ExitCode> {
    let root = repo_root()?;
    ensure!(lite_eval::ledger::module(&args.module).is_some(), "unknown module `{}`", args.module);
    let credentials = credentials();
    let start = CodeVersion::current(&root)?;
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let output_dir = Path::new(EVIDENCE_ROOT).join("gates").join(format!("{}-{stamp}", args.module));
    let python_evidence = output_dir.join("python-evidence");
    std::fs::create_dir_all(&python_evidence)?;
    let plan = GatePlan {
        repo_root: &root,
        module: &args.module,
        code_version: start.clone(),
        output_dir,
        jsonl_binary: build_jsonl(&root),
        python_evidence,
    };
    let record = run_gate(&plan, &SystemRunner, &credentials)?;
    let path = save_record(&root, &record, &stamp, &credentials)?;
    println!("{path}");
    ensure!(CodeVersion::current(&root)? == start, "code changed while the gate ran; record {path} is stale");
    match record.check_passes() {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("lite-eval: {error:#}");
            Ok(ExitCode::from(1))
        }
    }
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

fn not_implemented(name: &str) -> Result<ExitCode> {
    bail!("subcommand `{name}` is not implemented yet")
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
        Command::Run(_) => not_implemented("run"),
        Command::Validate => validate_task_set(),
        Command::Holdout(command) => holdout(command),
        Command::Fork(args) => fork(args),
        Command::Compare(_) => not_implemented("compare"),
        Command::Classify(_) => not_implemented("classify"),
        Command::Oracle(_) => not_implemented("oracle"),
        Command::Gate(args) => gate(args),
        Command::Deny => deny(),
        Command::ReviewRecord(args) => review_record(args),
        Command::Ledger(command) => ledger(command),
        Command::Notify(args) => notify(args),
        Command::LongTrial(_) => not_implemented("long-trial"),
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
