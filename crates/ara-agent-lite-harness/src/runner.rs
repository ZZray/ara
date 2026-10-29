use std::{
    collections::{HashMap, HashSet},
    fs::{File, OpenOptions},
    future::Future,
    io::Write,
    panic::AssertUnwindSafe,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{ensure, Context, Result};
use futures::FutureExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{broadcast, mpsc, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use ara_agent_lite_core::{
    agent, model::ModelHost, rpi_agent::AgentTool, termination::StopEvidence, RunMode, RunRequest,
};

use crate::{
    storage::{canonical, write_new, Admission},
    BudgetStopContinuation, BudgetStopDetail, BudgetStopReviewPolicy, BudgetStopReviewRecord, BudgetStopSnapshot,
    BudgetStopSnapshotReference, BudgetStopTrigger, Candidate, Continuation, EffectEvidence, EffectReceiptRef,
    EventEnvelope, FencePresence, HarnessHost, HostReceiptRef, LengthStopReviewPolicy, ResumeChecks, ResumeRefusal,
    ResumeReview, ReviewBasis, ReviewReceipt, Settlement, StopContinuation, StopSnapshot, StopSnapshotReference,
    TaskOptions, TaskOutcome, TaskSpec, TaskStop, TruncatedMessageRef, UnknownEffectRecord, Verdict,
};

/// Cloneable observation/cancellation handle. No handle drop aborts the worker.
#[derive(Clone)]
pub struct TaskHandle {
    cancellation: CancellationToken,
    outcome: watch::Receiver<Option<Arc<TaskOutcome>>>,
    events: broadcast::Sender<EventEnvelope>,
}

impl TaskHandle {
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    pub async fn wait(&self) -> Arc<TaskOutcome> {
        let mut receiver = self.outcome.clone();
        loop {
            if let Some(outcome) = receiver.borrow().clone() {
                return outcome;
            }
            // The supervisor retains its sender until it publishes an outcome.
            // A process/runtime termination leaves the durable Session fence.
            receiver.changed().await.expect("harness supervisor ended without outcome");
        }
    }
}

/// Admit one task using the actual Lite runtime. Hosts must keep control state,
/// fences and evidence outside the authority of untrusted tools. These files and
/// advisory locks cannot resist arbitrary mutation by the same OS account.
///
/// Continuations must reuse the same ModelHost and tool registry Arcs. Matching
/// identity is a continuity check, not proof of behavior inside those objects.
pub fn start(spec: TaskSpec, initial: RunRequest, host: Arc<dyn HarnessHost>) -> Result<TaskHandle> {
    start_with_options(spec, initial, host, TaskOptions::default())
}

pub fn start_with_options(
    spec: TaskSpec,
    mut initial: RunRequest,
    host: Arc<dyn HarnessHost>,
    options: TaskOptions,
) -> Result<TaskHandle> {
    tokio::runtime::Handle::try_current().context("start requires a running Tokio runtime")?;
    ensure!(!spec.task_id.trim().is_empty(), "task ID required");
    ensure!(!spec.criteria_revision.trim().is_empty(), "criteria revision required");
    ensure!(!spec.authorization_revision.trim().is_empty(), "authorization revision required");
    ensure!(spec.max_runs > 0 && spec.max_calls > 0, "positive task budgets required");
    ensure!(spec.evidence_dir.is_absolute(), "absolute evidence directory required");
    ensure!(spec.deadline_unix_ms > now_ms()?, "task deadline already elapsed");
    ensure!(!initial.id.trim().is_empty(), "initial Run ID required");
    ensure!(initial.max_calls > 0 && initial.max_seconds > 0, "positive initial Run budgets required");
    initial.cwd = canonical(&initial.cwd)?;
    initial.state_path = canonical(&initial.state_path)?;
    initial.environment_state = canonical(&initial.environment_state)?;
    let authority = Authority::capture(&initial)?;
    let admission = Admission::acquire(&spec, &initial.state_path, &authority.record()?)?;
    write_new(&admission.evidence.join("task-options.json"), &options)?;
    let progress = Progress::new(&spec);
    spawn(spec, authority, admission, progress, initial, host, options)
}

/// Continue a fenced Task (crash, kill, cancellation or reconciliation stop)
/// in its ORIGINAL Session, criteria, authority, deadline and Task budget.
///
/// The Host first settles what the fenced owner left behind through
/// `HarnessHost::reconcile_resume`. Open native tool calls are closed with the
/// Host's findings; none is replayed. Consumption is never refunded: a Run
/// with no observation is charged its whole admitted call limit. `spec` must
/// equal the original except for a new `evidence_dir`.
pub async fn resume(
    spec: TaskSpec,
    next: RunRequest,
    host: Arc<dyn HarnessHost>,
    options: TaskOptions,
) -> Result<TaskHandle> {
    resume_checked(spec, next, host, options, ResumeChecks::default()).await
}

/// `resume` with the extra pre-callback checks a JSONL Host needs (13.6, 14.4).
/// Every refusal can be inspected with `error.downcast_ref::<ResumeRefusal>()`.
pub async fn resume_checked(
    spec: TaskSpec,
    mut next: RunRequest,
    host: Arc<dyn HarnessHost>,
    options: TaskOptions,
    checks: ResumeChecks,
) -> Result<TaskHandle> {
    tokio::runtime::Handle::try_current().context("resume requires a running Tokio runtime")?;
    ensure!(spec.evidence_dir.is_absolute(), "absolute evidence directory required");
    if spec.deadline_unix_ms <= now_ms()? {
        return Err(ResumeRefusal::DeadlineElapsed.into());
    }
    ensure!(!next.id.trim().is_empty(), "resumed Run ID required");
    ensure!(!next.objective.trim().is_empty(), "resumed Run requires an objective");
    ensure!(next.max_calls > 0 && next.max_seconds > 0, "positive Run budgets required");
    next.cwd = canonical(&next.cwd)?;
    next.state_path = canonical(&next.state_path)?;
    next.environment_state = canonical(&next.environment_state)?;
    let mut authority = Authority::capture(&next)?;
    let record = authority.record()?;
    let fenced = Admission::fenced(&next.state_path).map_err(|error| {
        let kind = if error.downcast_ref::<serde_json::Error>().is_some() {
            ResumeRefusal::FenceUnreadable
        } else {
            ResumeRefusal::NoFence
        };
        error.context(kind)
    })?;
    if checks.expected_fence_sha256.as_ref().is_some_and(|expected| *expected != fenced.sha256()) {
        return Err(ResumeRefusal::FenceMismatch.into());
    }
    let fence = fenced.record.clone();
    let original = fence.get("authority").context(ResumeRefusal::FenceUnreadable)?;
    let same = |key: &str, value: Value| fence.get(key) == Some(&value);
    if !(same("task_id", json!(spec.task_id))
        && same("criteria_revision", json!(spec.criteria_revision))
        && same("authorization_revision", json!(spec.authorization_revision))
        && same("deadline_unix_ms", json!(spec.deadline_unix_ms))
        && same("session_path", json!(next.state_path)))
    {
        return Err(ResumeRefusal::TaskTermsMismatch.into());
    }
    if !(same("max_runs", json!(spec.max_runs)) && same("max_calls", json!(spec.max_calls))) {
        return Err(ResumeRefusal::BudgetLarger.into());
    }
    if original["declaration_sha256"] != record["declaration_sha256"] {
        return Err(ResumeRefusal::AuthorityChanged.into());
    }
    let run_max_calls = original["run_max_calls"].as_u64().context(ResumeRefusal::FenceUnreadable)?;
    let run_max_seconds = original["run_max_seconds"].as_u64().context(ResumeRefusal::FenceUnreadable)?;
    if checks.refuse_larger_run_caps && (next.max_calls as u64 > run_max_calls || next.max_seconds > run_max_seconds) {
        return Err(ResumeRefusal::RunCapsLarger.into());
    }
    // Per-Run caps never grow past the original Task's first admission.
    authority.max_calls = authority.max_calls.min(run_max_calls as usize);
    authority.max_seconds = authority.max_seconds.min(run_max_seconds);
    let fenced_evidence = std::path::PathBuf::from(fence["evidence_dir"].as_str().context("fence evidence missing")?);
    let outcome_path = fenced_evidence.join("outcome.json");
    let fenced_outcome: Option<TaskOutcome> = if outcome_path.is_file() {
        Some(serde_json::from_slice(&std::fs::read(&outcome_path)?).context("fenced outcome unreadable")?)
    } else {
        None
    };
    let (consumed_runs, consumed_calls) = consumed(&fenced_evidence, fenced_outcome.as_ref())?;
    if !(consumed_runs < spec.max_runs && consumed_calls < spec.max_calls) {
        return Err(ResumeRefusal::BudgetExhausted.into());
    }
    let interrupted = agent::interrupted_run(&next.state_path, &next.cwd)?;
    // Created before the callback so Host-side RPC evidence has a home. A
    // refused attempt leaves it, and the next attempt needs another new one.
    let evidence = crate::storage::new_evidence(&spec)?;
    let review = ResumeReview {
        task_id: spec.task_id.clone(),
        session_path: next.state_path.clone(),
        resume_evidence_dir: evidence.clone(),
        fenced_evidence_dir: fenced_evidence.clone(),
        fence: fence.clone(),
        fenced_outcome,
        interrupted_run: interrupted.clone(),
        consumed_runs,
        consumed_calls,
    };
    let deadline = Instant::now() + Duration::from_millis(spec.deadline_unix_ms.saturating_sub(now_ms()?));
    let cancellation = CancellationToken::new();
    let (receipt, stopped) =
        joined(host.reconcile_resume(review.clone(), cancellation.clone()), &cancellation, deadline).await;
    let receipt = receipt.context(ResumeRefusal::HostCallbackFailed)?;
    if stopped {
        return Err(ResumeRefusal::DeadlineElapsed.into());
    }
    if !receipt.committed {
        return Err(ResumeRefusal::ReceiptUncommitted.into());
    }
    if receipt.settlement != Settlement::Settled {
        return Err(ResumeRefusal::SettlementUnknown.into());
    }
    if receipt.evidence.is_empty() {
        return Err(ResumeRefusal::EvidenceEmpty.into());
    }
    // Check pairing before take-over, so a bad receipt leaves the old fence intact.
    let open: Vec<_> =
        interrupted.iter().flat_map(|run| run.open_tool_calls.iter().map(|call| call.id.as_str())).collect();
    if !receipt.tool_calls.iter().map(|call| call.call_id.as_str()).eq(open.iter().copied()) {
        return Err(ResumeRefusal::CallMismatch.into());
    }
    let resume_record = json!({
        "version":1,"kind":"fenced_task_resume","fenced_evidence_dir":fenced_evidence,
        "fence":fence,"consumed_runs":consumed_runs,"consumed_calls":consumed_calls,
        "interrupted_run":interrupted,"receipt":receipt,
    });
    let admission = fenced.take_over(&spec, &next.state_path, evidence, &record, &resume_record)?;
    if let Some(run) = &interrupted {
        // After take-over a crash leaves the new fence and resume record, so a
        // later resume sees the same pending Run and the same consumption.
        agent::reconcile_interrupted_run(&next.state_path, &next.cwd, run, &receipt.tool_calls, &receipt.evidence)?;
    }
    write_new(&admission.evidence.join("task-options.json"), &options)?;
    let mut progress = Progress::new(&spec);
    progress.outcome.runs = consumed_runs;
    progress.outcome.observed_calls = consumed_calls;
    progress.outcome.detail = "task resumed after Host reconciliation".into();
    spawn(spec, authority, admission, progress, next, host, options)
}

/// Non-refundable consumption recorded in one evidence chain.
fn consumed(evidence: &std::path::Path, outcome: Option<&TaskOutcome>) -> Result<(usize, usize)> {
    let read = |path: std::path::PathBuf| -> Result<Value> {
        serde_json::from_slice(&std::fs::read(&path)?).with_context(|| format!("unreadable {}", path.display()))
    };
    let count = |value: &Value, key: &str| -> Result<usize> {
        Ok(usize::try_from(value[key].as_u64().with_context(|| format!("{key} missing"))?)?)
    };
    let prior = evidence.join("resume.json");
    let (mut runs, mut calls) = if prior.is_file() {
        let prior = read(prior)?;
        (count(&prior, "consumed_runs")?, count(&prior, "consumed_calls")?)
    } else {
        (0, 0)
    };
    loop {
        let number = runs + 1;
        let admitted = evidence.join(format!("run-{number}-admission.json"));
        if !admitted.is_file() {
            break;
        }
        let observation = evidence.join(format!("run-{number}-observation.json"));
        let charged = if observation.is_file() {
            count(&read(observation)?, "observed_calls")?
        } else {
            count(&read(admitted)?, "remaining_calls")?
        };
        calls = calls.checked_add(charged).context("call counter overflow")?;
        runs = number;
    }
    if let Some(outcome) = outcome {
        runs = runs.max(outcome.runs);
        calls = calls.max(outcome.observed_calls);
    }
    Ok((runs, calls))
}

fn spawn(
    spec: TaskSpec,
    authority: Authority,
    admission: Admission,
    progress: Progress,
    initial: RunRequest,
    host: Arc<dyn HarnessHost>,
    options: TaskOptions,
) -> Result<TaskHandle> {
    let deadline = Instant::now() + Duration::from_millis(spec.deadline_unix_ms.saturating_sub(now_ms()?));
    let cancellation = CancellationToken::new();
    let (events, _) = broadcast::channel(128);
    let (send, outcome) = watch::channel(None);
    let handle = TaskHandle { cancellation: cancellation.clone(), outcome, events: events.clone() };
    tokio::spawn(async move {
        let mut admission = admission;
        let mut progress = progress;
        let result = AssertUnwindSafe(run_task(
            TaskContext {
                spec: &spec,
                authority: &authority,
                host: &host,
                cancellation: &cancellation,
                deadline,
                ui: &events,
                options,
            },
            initial,
            &mut admission,
            &mut progress,
        ))
        .catch_unwind()
        .await;
        let settled = match result {
            Ok(Ok(settled)) => settled,
            Ok(Err(_)) => {
                // Callback/provider errors can contain secrets. Preserve original
                // Core events; expose a content-free supervisor failure here.
                progress.stop(TaskStop::ReconciliationRequired, "harness stage failed; inspect original evidence");
                false
            }
            Err(_) => {
                progress
                    .stop(TaskStop::ReconciliationRequired, "harness or Host panicked; effects require reconciliation");
                false
            }
        };
        if admission.finish(&mut progress.outcome, settled).is_err() {
            progress.stop(
                TaskStop::ReconciliationRequired,
                "outcome/fence publication failed; inspect Session and evidence",
            );
            progress.outcome.reconciliation_fence = Some(admission.fence.clone());
            progress.outcome.fence_presence =
                if admission.fence.is_file() { FencePresence::Present } else { FencePresence::Unknown };
        }
        // Completion observers may immediately admit another settled task.
        // Release the OS lock before publishing that observable completion.
        drop(admission);
        send.send_replace(Some(Arc::new(progress.outcome)));
    });
    Ok(handle)
}

fn now_ms() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis().try_into()?)
}

struct Progress {
    outcome: TaskOutcome,
}

impl Progress {
    fn new(spec: &TaskSpec) -> Self {
        Self {
            outcome: TaskOutcome {
                task_id: spec.task_id.clone(),
                stop: TaskStop::ReconciliationRequired,
                runs: 0,
                observed_calls: 0,
                last_candidate: None,
                last_receipt: None,
                detail: "task admitted".into(),
                task_accepted: false,
                reconciliation_fence: None,
                fence_presence: FencePresence::Unknown,
                unknown_effects: Vec::new(),
                budget_stop: None,
            },
        }
    }

    fn stop(&mut self, stop: TaskStop, detail: &str) {
        self.outcome.stop = stop;
        self.outcome.detail = detail.into();
    }
}

struct Authority {
    declaration: Value,
    model_host: Arc<dyn ModelHost>,
    tools: Vec<Arc<dyn AgentTool>>,
    steering: Arc<std::sync::Mutex<ara_agent_lite_core::steering::SteeringBuffer>>,
    max_calls: usize,
    max_seconds: u64,
    completion_gate: Option<ara_agent_lite_core::completion_gate::Registration>,
}

impl Authority {
    fn capture(request: &RunRequest) -> Result<Self> {
        Ok(Self {
            declaration: authority_declaration(request)?,
            model_host: request.model_host.clone(),
            tools: request.extra_tools.iter().map(|item| item.tool.clone()).collect(),
            steering: request.steering.clone(),
            max_calls: request.max_calls,
            max_seconds: request.max_seconds,
            completion_gate: request.policies.completion_gate.clone(),
        })
    }

    /// Durable, secret-free identity of this authority for a later resume.
    fn record(&self) -> Result<Value> {
        Ok(json!({
            "declaration_sha256":crate::storage::hash(&serde_json::to_vec(&self.declaration)?),
            "run_max_calls":self.max_calls,"run_max_seconds":self.max_seconds,
        }))
    }

    fn check(&self, request: &RunRequest) -> Result<()> {
        ensure!(self.declaration == authority_declaration(request)?, "continuation changed Host authority");
        ensure!(Arc::ptr_eq(&self.model_host, &request.model_host), "continuation changed ModelHost registry");
        ensure!(Arc::ptr_eq(&self.steering, &request.steering), "continuation changed steering owner");
        ensure!(self.tools.len() == request.extra_tools.len(), "continuation changed tools");
        for (original, current) in self.tools.iter().zip(&request.extra_tools) {
            ensure!(Arc::ptr_eq(original, &current.tool), "continuation changed tool registry");
        }
        let gate = |registration: Option<&ara_agent_lite_core::completion_gate::Registration>| {
            registration.map(|registration| registration.gate.clone())
        };
        match (gate(self.completion_gate.as_ref()), gate(request.policies.completion_gate.as_ref())) {
            (None, None) => {}
            (Some(original), Some(current)) => {
                ensure!(Arc::ptr_eq(&original, &current), "continuation changed the completion gate")
            }
            _ => anyhow::bail!("continuation changed the completion gate"),
        }
        Ok(())
    }
}

fn authority_declaration(request: &RunRequest) -> Result<Value> {
    let roots = request.knowledge_roots.as_ref().map(|roots| {
        json!({
            "environment":roots.environment_dir,"global_knowledge":roots.global_knowledge,
            "project_knowledge":roots.project_knowledge,"global_skills":roots.global_skills,
            "project_skills":roots.project_skills,
        })
    });
    let tools = request
        .extra_tools
        .iter()
        .map(|item| Ok(json!({"schema":serde_json::to_value(item.tool.schema())?,"allow_in_plan":item.allow_in_plan})))
        .collect::<Result<Vec<_>>>()?;
    let mut declaration = json!({
        "cwd":canonical(&request.cwd)?,"state":canonical(&request.state_path)?,
        "environment":canonical(&request.environment_state)?,"device":request.device_fp,
        "session":request.session_key,"route":request.route,"model":request.model,"endpoint":request.endpoint,
        "token_hash":crate::storage::hash(request.token.as_bytes()),"mode":format!("{:?}",request.mode),
        "local_tools":request.enable_local_tools,"tools":tools,"worker":request.worker_executable,
        "global_knowledge":request.global_knowledge,"roots":roots,"snapshot":request.context_snapshot,
        "goal_criteria":request.goal_criteria,"goal_id":request.goal_id,"goal_lease":request.goal_lease,
        "resume_job_lease":request.resume_job_lease,"expected_revision":request.expected_revision,
        "reviewed_run_id":request.reviewed_run_id,"max_output":request.max_output,
    });
    // Default policies add no key, so existing fence declaration hashes are unchanged.
    if let Some(policies) = request.policies.declaration() {
        declaration["policies"] = policies;
    }
    Ok(declaration)
}

/// On cancellation or deadline, signal and JOIN the same future. Cooperative
/// Host callbacks may take time to settle; we never detach their effects.
async fn joined<F: Future>(future: F, cancellation: &CancellationToken, deadline: Instant) -> (F::Output, bool) {
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => (future.await, true),
        _ = tokio::time::sleep_until(deadline) => {
            cancellation.cancel();
            (future.await, true)
        },
        result = &mut future => (result, cancellation.is_cancelled() || Instant::now() >= deadline),
    }
}

/// Read-only task dependencies. Mutable Session ownership and progress remain
/// separate borrows at the task-loop boundary.
struct TaskContext<'a> {
    spec: &'a TaskSpec,
    authority: &'a Authority,
    host: &'a Arc<dyn HarnessHost>,
    cancellation: &'a CancellationToken,
    deadline: Instant,
    ui: &'a broadcast::Sender<EventEnvelope>,
    options: TaskOptions,
}

enum ReviewSubject {
    Ordinary(Candidate),
    Budget {
        snapshot: BudgetStopSnapshot,
        reference: BudgetStopSnapshotReference,
    },
    /// Closed Length or NoProgress stop (16.2, 18.6).
    Stop {
        snapshot: StopSnapshot,
        reference: StopSnapshotReference,
    },
}

impl ReviewSubject {
    fn basis(&self) -> &ReviewBasis {
        match self {
            Self::Ordinary(candidate) => &candidate.basis,
            Self::Budget { snapshot, .. } => &snapshot.basis,
            Self::Stop { snapshot, .. } => &snapshot.basis,
        }
    }

    fn is_budget(&self) -> bool {
        matches!(self, Self::Budget { .. })
    }

    fn is_stop(&self) -> bool {
        matches!(self, Self::Stop { .. })
    }

    fn check(&self) -> Result<()> {
        check_basis(self.basis())?;
        match self {
            Self::Budget { snapshot, reference } => crate::snapshot::check(snapshot, reference)?,
            Self::Stop { snapshot, reference } => crate::stop_review::check(snapshot, reference)?,
            Self::Ordinary(_) => {}
        }
        Ok(())
    }

    async fn verify(&self, host: &Arc<dyn HarnessHost>, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        match self {
            Self::Ordinary(candidate) => host.verify(candidate.clone(), cancellation).await,
            Self::Budget { snapshot, .. } => host.verify_budget_snapshot(snapshot.clone(), cancellation).await,
            Self::Stop { snapshot, .. } => host.verify_stop_snapshot(snapshot.clone(), cancellation).await,
        }
    }
}

async fn run_task(
    context: TaskContext<'_>,
    mut request: RunRequest,
    admission: &mut Admission,
    progress: &mut Progress,
) -> Result<bool> {
    let TaskContext { spec, authority, host, cancellation, deadline, ui, options } = context;
    let mut ids = HashSet::new();
    loop {
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            progress.stop(TaskStop::Cancelled, "task canceled or original deadline elapsed");
            return Ok(false);
        }
        ensure!(ids.insert(request.id.clone()) && !request.id.trim().is_empty(), "fresh Run ID required");
        // Core also derives context scope from the actual path string. Compare
        // AND execute the canonical representation on every Run, including a
        // factory request using ordinary Windows paths after a verbatim first Run.
        request.cwd = canonical(&request.cwd)?;
        request.state_path = canonical(&request.state_path)?;
        request.environment_state = canonical(&request.environment_state)?;
        authority.check(&request)?;
        // Harness-owned reporting switch; not part of the authority declaration.
        request.policies.length_stop_evidence = options.length_stop_review
            == LengthStopReviewPolicy::ReviewClosedLength
            && request.mode == RunMode::Execute;
        let remaining = spec.max_calls.saturating_sub(progress.outcome.observed_calls);
        request.max_calls = request.max_calls.min(authority.max_calls).min(remaining);
        request.max_seconds = request
            .max_seconds
            .min(authority.max_seconds)
            .min(deadline.saturating_duration_since(Instant::now()).as_secs());
        if request.max_calls == 0 || request.max_seconds == 0 || progress.outcome.runs >= spec.max_runs {
            progress.stop(TaskStop::BudgetExhausted, "original task or Run budget exhausted");
            // Only a previously committed and settled Host review permits release.
            return Ok(progress.outcome.last_receipt.as_ref().is_some_and(valid_settlement));
        }
        let run_id = request.id.clone();
        let mode = request.mode;
        let state_path = request.state_path.clone();
        let cwd = request.cwd.clone();
        let run_calls_limit = request.max_calls;
        let run_token = request.token.clone();
        let journal = ara_agent_lite_core::session_journal::Journal::for_session(&state_path);
        let raw_start = if journal.exists() { journal.open()?.raw_count } else { 0 };
        progress.outcome.runs += 1;
        let run_number = progress.outcome.runs;
        write_new(
            &admission.evidence.join(format!("run-{run_number}-admission.json")),
            &json!({
                "task_id":spec.task_id,"run_id":run_id,"run_number":run_number,
                "criteria_revision":spec.criteria_revision,"authorization_revision":spec.authorization_revision,
                "session_path":state_path,"remaining_calls":run_calls_limit,
                "deadline_unix_ms":spec.deadline_unix_ms,"max_seconds":request.max_seconds,
            }),
        )?;
        let events_path = admission.evidence.join(format!("run-{run_number}-events.jsonl"));
        let file = OpenOptions::new().write(true).create_new(true).open(&events_path)?;
        crate::storage::sync_parent(&events_path)?;
        let (sender, receiver) = mpsc::channel(64);
        // `join!` drains independently while execute and cancellation settle.
        // Neither the UI nor a dropped TaskHandle owns this pump.
        let (executed, observed) = tokio::join!(
            joined(ara_agent_lite_core::execute(request, cancellation.clone(), sender), cancellation, deadline),
            drain(receiver, file, spec.task_id.clone(), run_id.clone(), cancellation, ui),
        );
        progress.outcome.observed_calls =
            progress.outcome.observed_calls.checked_add(observed.calls).context("logical call counter overflow")?;
        write_new(
            &admission.evidence.join(format!("run-{run_number}-observation.json")),
            &json!({
                "task_id":spec.task_id,"run_id":run_id,"observed_calls":observed.calls,
                "total_calls":progress.outcome.observed_calls,"event_count":observed.seq,
                "events_sha256":observed.digest,"event_failure":observed.failed,
                "core_joined":true,"stopped":executed.1,
            }),
        )?;
        if observed.failed || observed.calls > run_calls_limit || progress.outcome.observed_calls > spec.max_calls {
            // 22.6: the Run is already joined; record what has unknown effects.
            progress.outcome.unknown_effects.extend(unknown_effect_records(
                &journal,
                &state_path,
                raw_start,
                &run_id,
                &events_path,
            ));
            progress.stop(
                TaskStop::ReconciliationRequired,
                "event/receipt protocol or persistence failed; original Core joined",
            );
            return Ok(false);
        }
        if executed.1 || cancellation.is_cancelled() {
            progress.outcome.unknown_effects.extend(unknown_effect_records(
                &journal,
                &state_path,
                raw_start,
                &run_id,
                &events_path,
            ));
            progress.stop(TaskStop::Cancelled, "canceled/deadline; original Core future joined; reconcile effects");
            return Ok(false);
        }
        let result = match executed.0 {
            Ok(result) => result,
            Err(error) => {
                // Core's early admission errors bypass its final-result redactor.
                // Keep the original diagnostic with this known grant removed;
                // arbitrary Host/provider error text is not generally secret-free.
                let error = format!("{error:#}");
                let redacted =
                    if run_token.is_empty() { error } else { error.replace(&run_token, "[REDACTED_MODEL_GRANT]") };
                write_new(
                    &admission.evidence.join(format!("run-{run_number}-core-error.json")),
                    &json!({
                        "task_id":spec.task_id,"run_id":run_id,"category":"core_returned_error",
                        "known_model_grant_redacted":true,"error":redacted,
                        "settlement":"unknown_unless_independently_reconciled",
                    }),
                )?;
                progress.stop(TaskStop::ReconciliationRequired, "Core returned an error; no automatic replay");
                return Ok(false);
            }
        };
        let budget_termination = if options.budget_stop_review == BudgetStopReviewPolicy::ReviewClosedToolRound
            && mode == RunMode::Execute
        {
            observed.reviewable_budget_termination(&result, run_calls_limit)
        } else {
            None
        };
        let ordinary = result.code == 0 && result.error.is_empty() && observed.normal_terminal();
        // 16.2/18.6: a closed Length (opted in, Execute only) or NoProgress stop
        // may be reviewed; everything else keeps the existing conclusions.
        let stop_review = observed.stop_evidence.clone().filter(|evidence| {
            let enabled = match evidence {
                StopEvidence::Length { .. } => {
                    options.length_stop_review == LengthStopReviewPolicy::ReviewClosedLength && mode == RunMode::Execute
                }
                StopEvidence::NoProgress { .. } => true,
                StopEvidence::SummaryBudgetExhausted { .. } => false,
            };
            enabled && evidence.is_reviewable() && observed.closed_failed_terminal(&result)
        });
        if let Some(ara_agent_lite_core::termination::StopEvidence::SummaryBudgetExhausted { .. }) =
            &observed.stop_evidence
        {
            // 25.4: a summary the Task budget cannot pay for ends the Task;
            // when only this Run's cap was reached the Task stays open (DD-18).
            let unknown = unknown_effect_records(&journal, &state_path, raw_start, &run_id, &events_path);
            let settled = journal.open()?.metadata.get("pending_run").is_some_and(Value::is_null);
            if unknown.is_empty() && settled && progress.outcome.observed_calls >= spec.max_calls {
                progress.outcome.budget_stop = Some(BudgetStopDetail {
                    trigger: BudgetStopTrigger::CompactionSummary,
                    task_max_calls: spec.max_calls,
                    total_calls: progress.outcome.observed_calls,
                });
                progress.stop(TaskStop::BudgetExhausted, "task call budget exhausted before a compaction summary");
                return Ok(progress.outcome.last_receipt.as_ref().is_some_and(valid_settlement));
            }
            progress.outcome.unknown_effects.extend(unknown);
            progress.stop(
                TaskStop::ReconciliationRequired,
                "Run call cap reached before a compaction summary; Task budget or effects need reconciliation",
            );
            return Ok(false);
        }
        if !ordinary && budget_termination.is_none() && stop_review.is_none() {
            progress.stop(
                TaskStop::ReconciliationRequired,
                "Core/event completion cannot establish a settled ordinary candidate",
            );
            return Ok(false);
        }
        if budget_termination.is_some() {
            // A previous ordinary candidate is not the object reviewed now.
            // Keep its per-Run files, but never pair it with a new receipt.
            progress.outcome.last_candidate = None;
            progress.outcome.last_receipt = None;
        }
        // The UI event reducer exposes bounded text rather than every native
        // result field. Check the real durable journal as well, without treating
        // a convenient display projection as proof of complete settlement.
        let head = journal.open()?;
        ensure!(head.metadata.get("pending_run").is_some_and(Value::is_null), "native Run remains pending");
        ensure!(journal.contains_run(&run_id)?, "native Run has no durable completed receipt");
        let raw_end = head.raw_count;
        let mut native_unknown = false;
        let mut cursor = raw_start;
        while cursor < raw_end {
            let end = cursor.saturating_add(16).min(raw_end);
            let messages = journal.read(cursor, end, 64 * 1024 * 1024)?;
            for message in messages {
                if let ara_agent_lite_core::rpi_agent::AgentMessage::ToolResult(result) = message {
                    // Structured status only; Core types its builtin error contracts (22.2).
                    native_unknown |= unknown_effect(&serde_json::to_value(&result)?);
                }
            }
            cursor = end;
        }
        if native_unknown {
            progress.outcome.unknown_effects.extend(unknown_effect_records(
                &journal,
                &state_path,
                raw_start,
                &run_id,
                &events_path,
            ));
            progress.stop(
                TaskStop::ReconciliationRequired,
                "native tool receipt reports unknown/partial effects; no retry",
            );
            return Ok(false);
        }
        let goal = agent::session_goal(&state_path, &cwd)?;
        let goal = goal.as_ref().map(serde_json::to_value).transpose()?;
        if mode == RunMode::Goal && goal.as_ref().and_then(|goal| goal["status"].as_str()) != Some("awaiting_review") {
            progress.stop(
                TaskStop::GoalWaiting,
                "Goal awaits Host scheduling/settlement; no ordinary review or automatic slice",
            );
            return Ok(false);
        }
        if goal.is_some() && observed.last_goal != goal {
            progress.stop(TaskStop::ReconciliationRequired, "final Goal event differs from durable Goal state");
            return Ok(false);
        }
        if observed.calls == 0 {
            progress.stop(
                TaskStop::Informational,
                "management/command Run has no model artifact candidate; inspect its native receipts",
            );
            return Ok(false);
        }
        if budget_termination.is_none()
            && stop_review.is_none()
            && (observed.pending.is_some() || observed.last_result.as_deref() != Some("agent:stop"))
        {
            progress.stop(TaskStop::ReconciliationRequired, "last model result is not an ordinary complete agent Stop");
            return Ok(false);
        }
        let basis = ReviewBasis {
            task_id: spec.task_id.clone(),
            criteria_revision: spec.criteria_revision.clone(),
            authorization_revision: spec.authorization_revision.clone(),
            run_id,
            run_number,
            session_path: state_path,
            events_path,
            events_sha256: observed.digest,
            event_count: observed.seq,
            run_calls: observed.calls,
            total_calls: progress.outcome.observed_calls,
            journal_head_sha256: crate::storage::hash(&serde_json::to_vec(&head)?),
            raw_start,
            raw_end,
            raw_proof: journal.proof(raw_start, raw_end)?,
        };
        check_basis(&basis)?;
        let subject = if let Some(termination) = budget_termination {
            let snapshot = BudgetStopSnapshot { basis, diagnostic_output: result.text, goal, termination };
            let reference = crate::snapshot::persist(&snapshot, &admission.evidence)?;
            ReviewSubject::Budget { snapshot, reference }
        } else if let Some(evidence) = stop_review {
            let truncated = match &evidence {
                StopEvidence::Length { source_raw_index, source_sha256, effective_output_tokens, .. } => {
                    let index = *source_raw_index;
                    ensure!(index >= basis.raw_start && index < basis.raw_end, "truncated message outside this Run");
                    let stored = journal.read(index, index + 1, 8 * 1024 * 1024)?;
                    let stored = stored.first().context("truncated message missing from journal")?;
                    ensure!(
                        crate::storage::hash(&serde_json::to_vec(stored)?) == *source_sha256,
                        "truncated message differs from Core stop evidence"
                    );
                    Some(TruncatedMessageRef {
                        raw_index: index,
                        typed_sha256: source_sha256.clone(),
                        journal_proof: journal.proof(index, index + 1)?,
                        effective_output_tokens: *effective_output_tokens,
                    })
                }
                _ => None,
            };
            let snapshot = StopSnapshot { basis, diagnostic_output: result.text, goal, evidence, truncated };
            let reference = crate::stop_review::persist(&snapshot, &admission.evidence)?;
            ReviewSubject::Stop { snapshot, reference }
        } else {
            let candidate = Candidate { basis, output: result.text, goal };
            write_new(&admission.evidence.join(format!("run-{run_number}-candidate.json")), &candidate)?;
            progress.outcome.last_candidate = Some(candidate.clone());
            ReviewSubject::Ordinary(candidate)
        };
        subject.check()?;
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            progress.stop(TaskStop::Cancelled, "original deadline/cancellation reached before review admission");
            return Ok(false);
        }
        let (review, stopped) = joined(subject.verify(host, cancellation.clone()), cancellation, deadline).await;
        let receipt = match review {
            Ok(receipt) => receipt,
            Err(_) if subject.is_stop() => {
                // 16.8: unsupported or failed stop review keeps the snapshot.
                progress.stop(
                    TaskStop::ReconciliationRequired,
                    "Host stop-snapshot review failed or is unsupported; snapshot retained",
                );
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let review_name = if subject.is_budget() {
            format!("run-{run_number}-budget-review.json")
        } else if subject.is_stop() {
            format!("run-{run_number}-stop-review.json")
        } else {
            format!("run-{run_number}-review.json")
        };
        write_new(&admission.evidence.join(review_name), &receipt)?;
        progress.outcome.last_receipt = Some(receipt.clone());
        if stopped || cancellation.is_cancelled() {
            progress.stop(
                TaskStop::Cancelled,
                "review callback joined after cancellation/deadline; reconcile callback effects",
            );
            return Ok(false);
        }
        if receipt.basis != *subject.basis() || !valid_settlement(&receipt) {
            progress.stop(
                TaskStop::ReconciliationRequired,
                "Host review is uncommitted, unknown, or bound to another candidate",
            );
            return Ok(false);
        }
        subject.check()?;
        if let ReviewSubject::Budget { reference, .. } = &subject {
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                progress
                    .stop(TaskStop::Cancelled, "original deadline/cancellation reached during budget review checks");
                return Ok(false);
            }
            write_new(
                &crate::budget_review_index_path(&admission.evidence, run_number),
                &BudgetStopReviewRecord {
                    version: 1,
                    kind: "closed_budget_snapshot_review".into(),
                    snapshot: reference.clone(),
                    receipt: receipt.clone(),
                },
            )?;
            if cancellation.is_cancelled() || Instant::now() >= deadline {
                progress.stop(
                    TaskStop::Cancelled,
                    "original deadline/cancellation reached during budget review publication",
                );
                return Ok(false);
            }
        }
        if let ReviewSubject::Stop { reference, .. } = &subject {
            write_new(
                &crate::stop_review_index_path(&admission.evidence, run_number),
                &json!({"version":1,"kind":"stop_snapshot_review","snapshot":reference,"receipt":receipt}),
            )?;
        }
        match &receipt.verdict {
            Verdict::Verified => {
                progress.stop(
                    TaskStop::AwaitingHostAcceptance,
                    if subject.is_budget() {
                        "Host verified the closed-budget snapshot; business Task acceptance remains external"
                    } else if subject.is_stop() {
                        "Host verified the stop snapshot; business Task acceptance remains external"
                    } else {
                        "Host verification passed; business Task acceptance remains external"
                    },
                );
                return Ok(true);
            }
            Verdict::Unknown => {
                progress.stop(TaskStop::ReconciliationRequired, "Host review reports unknown outcome");
                return Ok(false);
            }
            Verdict::NeedsChanges { feedback } if feedback.trim().is_empty() && !subject.is_budget() => {
                progress.stop(
                    TaskStop::NeedsChanges,
                    "Host requested changes without correction feedback; no continuation",
                );
                return Ok(true);
            }
            Verdict::NeedsChanges { .. } => {}
        }
        if run_number >= spec.max_runs || progress.outcome.observed_calls >= spec.max_calls {
            progress.stop(TaskStop::BudgetExhausted, "Host requested changes but original task budget is exhausted");
            return Ok(true);
        }
        if matches!(&receipt.verdict, Verdict::NeedsChanges { feedback } if feedback.trim().is_empty()) {
            progress
                .stop(TaskStop::NeedsChanges, "Host requested changes without correction feedback; no continuation");
            return Ok(true);
        }
        if cancellation.is_cancelled() || Instant::now() >= deadline {
            progress.stop(TaskStop::Cancelled, "original deadline/cancellation reached before continuation callback");
            return Ok(false);
        }
        let (prepared, stopped) = match &subject {
            ReviewSubject::Ordinary(candidate) => {
                let continuation = Continuation {
                    previous: candidate.clone(),
                    receipt,
                    next_run_number: run_number + 1,
                    remaining_calls: spec.max_calls - progress.outcome.observed_calls,
                    deadline_unix_ms: spec.deadline_unix_ms,
                };
                joined(host.prepare_continuation(continuation, cancellation.clone()), cancellation, deadline).await
            }
            ReviewSubject::Budget { snapshot, .. } => {
                let continuation = BudgetStopContinuation {
                    previous: snapshot.clone(),
                    receipt,
                    next_run_number: run_number + 1,
                    remaining_calls: spec.max_calls - progress.outcome.observed_calls,
                    deadline_unix_ms: spec.deadline_unix_ms,
                };
                joined(
                    host.prepare_budget_stop_continuation(continuation, cancellation.clone()),
                    cancellation,
                    deadline,
                )
                .await
            }
            ReviewSubject::Stop { snapshot, .. } => {
                let continuation = StopContinuation {
                    previous: snapshot.clone(),
                    receipt,
                    next_run_number: run_number + 1,
                    remaining_calls: spec.max_calls - progress.outcome.observed_calls,
                    deadline_unix_ms: spec.deadline_unix_ms,
                };
                joined(host.prepare_stop_continuation(continuation, cancellation.clone()), cancellation, deadline).await
            }
        };
        request = prepared?;
        if stopped || cancellation.is_cancelled() {
            progress.stop(
                TaskStop::Cancelled,
                "continuation callback joined after cancellation/deadline; reconcile effects",
            );
            return Ok(false);
        }
        ensure!(!request.objective.trim().is_empty(), "continuation requires correction objective");
        subject.check()?;
    }
}

fn valid_settlement(receipt: &ReviewReceipt) -> bool {
    receipt.committed && receipt.settlement == Settlement::Settled && receipt.verdict != Verdict::Unknown
}

fn check_basis(basis: &ReviewBasis) -> Result<()> {
    ensure!(crate::storage::file_hash(&basis.events_path)? == basis.events_sha256, "review event bytes changed");
    let journal = ara_agent_lite_core::session_journal::Journal::for_session(&basis.session_path);
    let head = journal.open()?;
    ensure!(
        crate::storage::hash(&serde_json::to_vec(&head)?) == basis.journal_head_sha256,
        "review journal head changed"
    );
    ensure!(journal.proof(basis.raw_start, basis.raw_end)? == basis.raw_proof, "review journal source changed");
    Ok(())
}

#[derive(Default)]
struct Observation {
    calls: usize,
    pending: Option<(usize, String)>,
    last_result: Option<String>,
    seq: u64,
    digest: String,
    failed: bool,
    starts: usize,
    terminals: usize,
    terminal_ok: bool,
    terminal_code: Option<i64>,
    terminal_detail: Option<String>,
    terminal_status: Option<String>,
    budget_termination: Option<ara_agent_lite_core::termination::ClosedBudgetTermination>,
    stop_evidence: Option<ara_agent_lite_core::termination::StopEvidence>,
    tools: HashMap<String, String>,
    completed_tools: HashSet<String>,
    last_goal: Option<Value>,
}

impl Observation {
    fn normal_terminal(&self) -> bool {
        self.starts == 1
            && self.terminals == 1
            && self.terminal_ok
            && self.pending.is_none()
            && self.tools.is_empty()
            && self.budget_termination.is_none()
    }

    /// One settled failed `agent_end` matching the Core result, with every
    /// model call and tool call closed.
    fn closed_failed_terminal(&self, result: &ara_agent_lite_core::RunResult) -> bool {
        !self.failed
            && self.starts == 1
            && self.terminals == 1
            && self.terminal_status.as_deref() == Some("failed")
            && self.terminal_code == Some(-1)
            && result.code == -1
            && !result.error.is_empty()
            && self.terminal_detail.as_deref() == Some(result.error.as_str())
            && self.pending.is_none()
            && self.tools.is_empty()
    }

    fn reviewable_budget_termination(
        &self,
        result: &ara_agent_lite_core::RunResult,
        limit: usize,
    ) -> Option<ara_agent_lite_core::termination::ClosedBudgetTermination> {
        let termination = self.budget_termination.as_ref()?;
        if !self.failed
            && self.starts == 1
            && self.terminals == 1
            && self.terminal_status.as_deref() == Some("failed")
            && self.terminal_code == Some(-1)
            && result.code == -1
            && !result.error.is_empty()
            && self.terminal_detail.as_deref() == Some(result.error.as_str())
            && limit > 0
            && self.calls == limit
            && self.pending.is_none()
            && self.tools.is_empty()
            && !self.completed_tools.is_empty()
            && self.last_result.as_deref() == Some("agent:toolUse")
            && termination.is_reviewable()
        {
            Some(termination.clone())
        } else {
            None
        }
    }

    fn observe(&mut self, event: &Value) {
        if self.terminals > 0 {
            self.failed = true;
        }
        // A tool_end carrying Core's structured `effects_unknown` is effect
        // state, not a protocol failure: the durable journal scan stops the
        // Task, and cancellation keeps its own outcome (22.6).
        if event["type"] != "tool_end" && unknown_effect(event) {
            self.failed = true;
        }
        match event["type"].as_str() {
            Some("agent_start") => self.starts += 1,
            Some("agent_end") => {
                self.terminals += 1;
                self.terminal_code = event["code"].as_i64();
                self.terminal_detail = event["detail"].as_str().map(str::to_owned);
                self.terminal_status = event["status"].as_str().map(str::to_owned);
                if let Some(value) = event.get("termination") {
                    match serde_json::from_value::<ara_agent_lite_core::termination::ClosedBudgetTermination>(
                        value.clone(),
                    ) {
                        Ok(termination) if termination.is_reviewable() => self.budget_termination = Some(termination),
                        _ => self.failed = true,
                    }
                }
                if let Some(value) = event.get("stop_evidence") {
                    match serde_json::from_value::<ara_agent_lite_core::termination::StopEvidence>(value.clone()) {
                        Ok(evidence) => self.stop_evidence = Some(evidence),
                        Err(_) => self.failed = true,
                    }
                }
                self.terminal_ok = event["status"] == "completed"
                    && event["code"] == 0
                    && event.get("detail").and_then(Value::as_str).is_some_and(str::is_empty);
            }
            Some("goal_status" | "goal_update") => {
                self.last_goal = event.get("goal").cloned();
            }
            Some("model_call") => {
                let call = event["call"].as_u64().and_then(|n| usize::try_from(n).ok());
                let phase = event["phase"].as_str().unwrap_or("");
                if self.pending.is_some()
                    || call != self.calls.checked_add(1)
                    || !matches!(phase, "agent" | "summary")
                    || event["status"] != "reserved"
                {
                    self.failed = true;
                }
                // Count every observed reservation, including malformed/duplicate
                // ones. A failure never refunds the logical budget.
                self.calls = self.calls.saturating_add(1);
                self.pending = call.map(|call| (call, phase.to_owned()));
            }
            Some("model_result") => {
                let call = event["call"].as_u64().and_then(|n| usize::try_from(n).ok());
                let phase = event["phase"].as_str().unwrap_or("");
                let expected = call.map(|call| (call, phase.to_owned()));
                if self.pending.is_none() || self.pending != expected || event["status"] != "received" {
                    self.failed = true;
                }
                self.pending = None;
                self.last_result = event["stop_reason"].as_str().map(|stop| format!("{phase}:{stop}"));
            }
            Some("tool_start") => {
                let id = event["step_id"].as_str().unwrap_or("");
                let name = event["title"].as_str().unwrap_or("");
                if id.is_empty()
                    || name.is_empty()
                    || self.completed_tools.contains(id)
                    || self.tools.insert(id.to_owned(), name.to_owned()).is_some()
                {
                    self.failed = true;
                }
            }
            Some("tool_end") => {
                let id = event["step_id"].as_str().unwrap_or("");
                let name = event["title"].as_str().unwrap_or("");
                if self.tools.remove(id).as_deref() != Some(name) || !self.completed_tools.insert(id.to_owned()) {
                    self.failed = true;
                }
            }
            _ => {}
        }
    }
}

async fn drain(
    mut receiver: mpsc::Receiver<Value>,
    mut file: File,
    task_id: String,
    run_id: String,
    cancellation: &CancellationToken,
    ui: &broadcast::Sender<EventEnvelope>,
) -> Observation {
    let mut observed = Observation::default();
    let mut digest = Sha256::new();
    while let Some(event) = receiver.recv().await {
        observed.seq = observed.seq.saturating_add(1);
        observed.observe(&event);
        // 22.6: a structured unknown effect observed while the Task is still
        // running stops the Run (no further dispatch). After an external
        // cancellation the Task keeps its Cancelled outcome.
        if event["type"] == "tool_end" && unknown_effect(&event) && !cancellation.is_cancelled() {
            observed.failed = true;
        }
        if observed.failed {
            // Ask the same original execution to stop after a protocol fault or
            // explicit unknown receipt; still drain and join its actual future.
            cancellation.cancel();
        }
        let envelope = EventEnvelope { task_id: task_id.clone(), run_id: run_id.clone(), seq: observed.seq, event };
        let written = (|| -> Result<()> {
            let mut bytes = serde_json::to_vec(&envelope)?;
            bytes.push(b'\n');
            file.write_all(&bytes)?;
            file.sync_all()?;
            digest.update(&bytes);
            Ok(())
        })();
        if written.is_err() {
            observed.failed = true;
            cancellation.cancel();
            // Keep draining and join Core after failure; never block it on UI.
        } else {
            let _ = ui.send(envelope);
        }
    }
    if file.sync_all().is_err() {
        observed.failed = true;
        cancellation.cancel();
    }
    observed.digest = format!("{:x}", digest.finalize());
    observed
}

/// Structured unknown-effect state only (22.1, 22.3). Core types its own
/// builtin error contracts; a present but unparsable `unknown_effect_record`
/// still counts as unknown (22.8).
fn unknown_effect(value: &Value) -> bool {
    fn has_record(value: &Value) -> bool {
        match value {
            Value::Array(items) => items.iter().any(has_record),
            Value::Object(object) => object.contains_key("unknown_effect_record") || object.values().any(has_record),
            _ => false,
        }
    }
    ara_agent_lite_core::effect_status::reports_unknown(value) || has_record(value)
}

/// Unknown-effect records of one Run (22.4): durable ToolResults from the
/// journal range first, then `tool_end` events whose result never became
/// durable. Deduplicated by native call ID.
fn unknown_effect_records(
    journal: &ara_agent_lite_core::session_journal::Journal,
    session_path: &Path,
    raw_start: usize,
    run_id: &str,
    events_path: &Path,
) -> Vec<UnknownEffectRecord> {
    let mut records: Vec<UnknownEffectRecord> = Vec::new();
    if let Ok(head) = journal.open() {
        for raw_index in raw_start..head.raw_count {
            let Ok(messages) = journal.read(raw_index, raw_index + 1, 8 * 1024 * 1024) else { break };
            for message in messages {
                let ara_agent_lite_core::rpi_agent::AgentMessage::ToolResult(result) = &message else { continue };
                let Ok(value) = serde_json::to_value(result) else { continue };
                if !unknown_effect(&value) || records.iter().any(|record| record.native_call_id == result.tool_call_id)
                {
                    continue;
                }
                let host_receipt = result
                    .details
                    .as_ref()
                    .and_then(|details| details.get("receipt_ref"))
                    .and_then(|receipt| serde_json::from_value::<HostReceiptRef>(receipt.clone()).ok());
                records.push(UnknownEffectRecord {
                    run_id: run_id.to_owned(),
                    native_call_id: result.tool_call_id.clone(),
                    tool: result.tool_name.clone(),
                    receipt_ref: EffectReceiptRef {
                        evidence: EffectEvidence::Journal {
                            session_path: session_path.to_owned(),
                            raw_index,
                            message_sha256: crate::storage::hash(&serde_json::to_vec(&message).unwrap_or_default()),
                        },
                        host_receipt,
                    },
                });
            }
        }
    }
    if let Ok(text) = std::fs::read_to_string(events_path) {
        for line in text.lines() {
            let Ok(envelope) = serde_json::from_str::<EventEnvelope>(line) else { continue };
            let event = &envelope.event;
            let id = event["step_id"].as_str().unwrap_or_default();
            if event["type"] != "tool_end"
                || !unknown_effect(event)
                || id.is_empty()
                || records.iter().any(|record| record.native_call_id == id)
            {
                continue;
            }
            records.push(UnknownEffectRecord {
                run_id: run_id.to_owned(),
                native_call_id: id.to_owned(),
                tool: event["title"].as_str().unwrap_or_default().to_owned(),
                receipt_ref: EffectReceiptRef {
                    evidence: EffectEvidence::Event { events_path: events_path.to_owned(), seq: envelope.seq },
                    host_receipt: None,
                },
            });
        }
    }
    records
}

#[cfg(test)]
mod budget_observation_tests {
    use super::*;

    fn termination() -> Value {
        json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":true,"model_final_stop":false})
    }

    fn observed(termination: Value) -> Observation {
        let mut observed = Observation::default();
        for event in [
            json!({"type":"agent_start"}),
            json!({"type":"model_call","call":1,"phase":"agent","status":"reserved"}),
            json!({"type":"model_result","call":1,"phase":"agent","status":"received","stop_reason":"toolUse"}),
            json!({"type":"tool_start","step_id":"original-write","title":"write_answer"}),
            json!({"type":"tool_end","step_id":"original-write","title":"write_answer","status":"completed"}),
            json!({"type":"agent_end","code":-1,"status":"failed","detail":"arbitrary human diagnostic","termination":termination}),
        ] {
            observed.observe(&event);
        }
        observed
    }

    fn result() -> ara_agent_lite_core::RunResult {
        ara_agent_lite_core::RunResult { code: -1, text: String::new(), error: "arbitrary human diagnostic".into() }
    }

    #[test]
    fn typed_budget_gate_does_not_parse_the_human_error_or_wire_stop_spelling() {
        let mut observation = observed(termination());
        assert!(observation.reviewable_budget_termination(&result(), 1).is_some());
        assert!(!observation.normal_terminal());
        observation.last_result = Some("agent:tool_calls".into());
        assert!(observation.reviewable_budget_termination(&result(), 1).is_none());
    }

    #[test]
    fn malformed_or_unreviewable_termination_is_never_eligible() {
        for bad in [
            Value::Null,
            json!({"version":2,"cause":"call_budget_exhausted","native_round_closed":true,"model_final_stop":false}),
            json!({"version":1,"cause":"invented","native_round_closed":true,"model_final_stop":false}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":false,"model_final_stop":false}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":true,"model_final_stop":true}),
            json!({"version":1,"cause":"call_budget_exhausted","native_round_closed":true,"model_final_stop":false,"extra":true}),
        ] {
            let observation = observed(bad);
            assert!(observation.failed);
            assert!(observation.reviewable_budget_termination(&result(), 1).is_none());
        }
    }

    #[test]
    fn budget_gate_requires_exact_exhaustion_and_a_single_closed_terminal() {
        let mut observation = observed(termination());
        assert!(observation.reviewable_budget_termination(&result(), 2).is_none());
        observation.tools.insert("missing-result".into(), "write_answer".into());
        assert!(observation.reviewable_budget_termination(&result(), 1).is_none());
        let mut observation = observed(termination());
        observation.observe(&json!({"type":"agent_end","status":"completed","code":0,"detail":""}));
        assert!(observation.failed);
        assert!(observation.reviewable_budget_termination(&result(), 1).is_none());
    }
}

/// Property 17: consumption accumulates and is never refunded (requirement 8.8).
#[cfg(test)]
mod consumed_tests {
    use super::*;
    use proptest::prelude::*;

    fn write(path: std::path::PathBuf, value: Value) {
        std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn outcome(runs: usize, observed_calls: usize) -> TaskOutcome {
        TaskOutcome {
            task_id: "t".into(),
            stop: TaskStop::ReconciliationRequired,
            runs,
            observed_calls,
            last_candidate: None,
            last_receipt: None,
            detail: String::new(),
            task_accepted: false,
            reconciliation_fence: None,
            fence_presence: FencePresence::Present,
            unknown_effects: Vec::new(),
            budget_stop: None,
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn consumption_counts_every_admitted_run_without_refund(
            prior in proptest::option::of((0usize..4, 0usize..40)),
            runs in proptest::collection::vec((1usize..10, proptest::option::of(0usize..10)), 0..6),
            published in proptest::option::of((0usize..12, 0usize..80)),
        ) {
            let directory = tempfile::tempdir().unwrap();
            let evidence = directory.path();
            let (prior_runs, prior_calls) = prior.unwrap_or((0, 0));
            if let Some((consumed_runs, consumed_calls)) = prior {
                write(evidence.join("resume.json"), json!({"consumed_runs":consumed_runs,"consumed_calls":consumed_calls}));
            }
            let mut expected_calls = prior_calls;
            for (offset, (admitted, observed)) in runs.iter().enumerate() {
                let number = prior_runs + offset + 1;
                write(evidence.join(format!("run-{number}-admission.json")), json!({"remaining_calls":admitted}));
                if let Some(observed) = observed {
                    write(evidence.join(format!("run-{number}-observation.json")), json!({"observed_calls":observed}));
                }
                // An unobserved Run is charged its whole admitted limit.
                expected_calls += observed.unwrap_or(*admitted);
            }
            let mut expected_runs = prior_runs + runs.len();
            let published = published.map(|(runs, calls)| outcome(runs, calls));
            if let Some(outcome) = &published {
                expected_runs = expected_runs.max(outcome.runs);
                expected_calls = expected_calls.max(outcome.observed_calls);
            }
            let (consumed_runs, consumed_calls) = consumed(evidence, published.as_ref()).unwrap();
            prop_assert_eq!((consumed_runs, consumed_calls), (expected_runs, expected_calls));
            // Never below any earlier record of the chain.
            prop_assert!(consumed_runs >= prior_runs && consumed_calls >= prior_calls);
            if let Some(outcome) = &published {
                prop_assert!(consumed_runs >= outcome.runs && consumed_calls >= outcome.observed_calls);
            }
        }
    }
}
