use crate::{journal::Journal, types::*};
use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_harness::{
    self as harness,
    core::{self, steering::SteeringBuffer, RunRequest},
    *,
};
use ara_lite::{
    store::Store,
    types::{Connection, Request, Task},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub state_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub authorization_revision: String,
    pub allowed_actors: Vec<String>,
    pub max_parallel: usize,
    pub max_calls: usize,
    pub max_runs: usize,
    pub max_seconds: u64,
    pub accept_mode: AcceptMode,
    pub review_mode: ReviewMode,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptMode {
    Off,
    Observe,
    Auto,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMode {
    Human,
    VerifiedArtifact,
}

pub struct Prepared {
    pub request: RunRequest,
    pub host: Arc<dyn HarnessHost>,
}

/// Product-specific model routing, tools and artifact checks remain in this port.
pub trait Adapter: Send + Sync + 'static {
    fn revision(&self) -> String;
    fn prepare(&self, binding: &ExecutionBinding, task: &Task, journal: Arc<Journal>) -> Result<Prepared>;
}

struct Coordination {
    store: Arc<Store>,
    master: Connection,
    worker: Connection,
    gate: Mutex<()>,
}
impl Coordination {
    fn task(&self, id: &str) -> Result<Task> {
        self.store.raw_task(id).context("unknown coordination Task")
    }
    fn change(&self, id: &str, master: bool, mut request: Request) -> Result<Task> {
        let _guard = self.gate.lock().unwrap();
        let task = self.task(id)?;
        // A review can freeze the Task after the scheduler's earlier read.
        // No heartbeat is needed once its execution lease has been cleared.
        if request.op == "heartbeat" && !matches!(task.state.as_str(), "running" | "unconfirmed") {
            return Ok(task);
        }
        request.task_id = id.into();
        request.version = Some(task.version);
        request.generation = Some(task.generation);
        self.store
            .execute(if master { &self.master.token } else { &self.worker.token }, &request)?
            .into_task()
            .context("coordinator did not return Task")
    }
}

struct Active {
    handle: TaskHandle,
    steering: Arc<Mutex<SteeringBuffer>>,
}

pub struct Runtime {
    pub config: Config,
    pub journal: Arc<Journal>,
    coordination: Arc<Coordination>,
    adapter: Arc<dyn Adapter>,
    active: BTreeMap<String, Active>,
}

fn connection(store: &Store, dir: &Path, role: &str) -> Result<Connection> {
    let path = dir.join(format!("{role}.json"));
    if path.exists() {
        return Ok(ara_lite::profile::read_connection(&path)?);
    }
    let keys = store.join_keys();
    let token = if role == "master" { keys.0 } else { keys.1 };
    let client = store
        .execute(
            &token,
            &Request {
                op: "join".into(),
                request_id: format!("reference-host-{role}"),
                role: role.into(),
                name: format!("reference-host-{role}"),
                ..Request::default()
            },
        )?
        .into_client()
        .context("join returned no client")?;
    let connection =
        Connection { url: "http://127.0.0.1:1".into(), token: client.token, id: client.id, role: role.into() };
    ara_lite::profile::save_private(&path, &connection, true)?;
    Ok(connection)
}

impl Runtime {
    pub fn open(mut config: Config, adapter: Arc<dyn Adapter>) -> Result<Self> {
        ensure!(config.state_dir.is_absolute() && config.workspace_root.is_absolute(), "absolute Host paths required");
        ensure!(
            (1..=16).contains(&config.max_parallel)
                && (1..=128).contains(&config.max_calls)
                && (1..=16).contains(&config.max_runs)
                && (1..=3600).contains(&config.max_seconds),
            "invalid Host bounds"
        );
        ensure!(
            !config.authorization_revision.is_empty() && !config.allowed_actors.is_empty(),
            "explicit Host policy required"
        );
        ara_lite::profile::create_private_dir(&config.state_dir)?;
        config.state_dir = config.state_dir.canonicalize()?;
        config.workspace_root = config.workspace_root.canonicalize()?;
        let journal = Arc::new(Journal::open(&config.state_dir.join("execution.sqlite"))?);
        let store = Arc::new(Store::open(&config.state_dir.join("coordinator"))?);
        let master = connection(&store, &config.state_dir, "master")?;
        let worker = connection(&store, &config.state_dir, "worker")?;
        std::fs::create_dir_all(config.state_dir.join("tasks"))?;
        Ok(Self {
            config,
            journal,
            coordination: Arc::new(Coordination { store, master, worker, gate: Mutex::new(()) }),
            adapter,
            active: BTreeMap::new(),
        })
    }

    fn policy(&self, event: &InputEvent) -> Result<()> {
        ensure!(event.authorization_revision == self.config.authorization_revision, "authorization revision changed");
        ensure!(self.config.allowed_actors.contains(&event.actor), "actor is not authorized");
        Ok(())
    }

    fn adapter_fingerprint(&self) -> String {
        let policy = json!({"actors":self.config.allowed_actors,"workspace_root":self.config.workspace_root,
            "authorization_revision":self.config.authorization_revision,"review_mode":self.config.review_mode});
        format!(
            "{}:{:x}",
            self.adapter.revision(),
            Sha256::digest(serde_json::to_vec(&policy).expect("Host policy serializes"))
        )
    }

    pub fn ingest(&mut self, input: InputEvent) -> Result<EventRecord> {
        self.policy(&input)?;
        if let EventBody::Work { workspace, objective, title } = &input.body {
            let workspace = workspace.canonicalize()?;
            ensure!(
                workspace.starts_with(&self.config.workspace_root) && !self.config.state_dir.starts_with(&workspace),
                "workspace outside Host grant or contains control state"
            );
            ensure!(!title.trim().is_empty() && !objective.trim().is_empty(), "work title and objective required");
        }
        let (record, _) = self.journal.ingest(&input)?;
        Ok(record)
    }

    pub fn status(&self) -> Result<serde_json::Value> {
        Ok(json!({"tasks":self.coordination.store.snapshot()?.tasks,"execution":self.journal.bindings()?,
            "unknown_events":self.journal.events(DeliveryState::Unknown)?,
            "pending_events":self.journal.events(DeliveryState::Pending)?,"rejected_events":self.journal.events(DeliveryState::Rejected)?,
            "pending_notices":self.journal.notices(NoticeState::Pending)?,"unknown_notices":self.journal.notices(NoticeState::Unknown)?}))
    }

    pub async fn tick(&mut self) -> Result<()> {
        self.coordination.store.tick()?;
        self.collect_completed()?;
        self.dispatch().await
    }

    fn collect_completed(&mut self) -> Result<()> {
        let completed: Vec<_> = self
            .active
            .iter()
            .filter_map(|(id, active)| active.handle.try_outcome().map(|outcome| (id.clone(), outcome)))
            .collect();
        for (id, outcome) in completed {
            let active = &self.active[&id];
            let mut steering = active.steering.lock().unwrap();
            steering.finish();
            let receipts = steering.outcomes_snapshot();
            for record in self.journal.events(DeliveryState::Dispatching)? {
                if record.task_id.as_deref() != Some(&id) {
                    continue;
                }
                let status =
                    receipts.iter().find(|r| r["host_event_key"] == record.key).and_then(|r| r["status"].as_str());
                self.journal.mark_event(
                    &record.key,
                    match status {
                        Some("applied") => DeliveryState::Applied,
                        Some("unapplied") => DeliveryState::Unapplied,
                        _ => DeliveryState::Unknown,
                    },
                    "original steering receipt at Task settlement",
                )?;
            }
            let mut binding = self.journal.binding(&id)?.context("binding missing")?;
            binding.phase = if outcome.fence_presence == FencePresence::Released {
                ExecutionPhase::Finished
            } else {
                ExecutionPhase::NeedsReconciliation
            };
            let path = binding.spec.evidence_dir.join("host-outcome.json");
            let mut file = std::fs::File::create(path)?;
            std::io::Write::write_all(&mut file, &serde_json::to_vec_pretty(&*outcome)?)?;
            file.sync_all()?;
            self.journal.complete(
                &binding,
                &Notice {
                    id: format!("result:{id}:{}", binding.attempt),
                    task_id: id.clone(),
                    generation: binding.generation,
                    kind: "result".into(),
                    body: serde_json::to_string(&*outcome)?,
                    state: NoticeState::Pending,
                },
            )?;
            drop(steering);
            self.active.remove(&id);
        }
        Ok(())
    }

    async fn dispatch(&mut self) -> Result<()> {
        for record in self.journal.events(DeliveryState::Pending)? {
            if let Err(error) = self.process_event(&record) {
                let state = self.journal.event(&record.key)?.context("event disappeared")?.state;
                if state == DeliveryState::Pending && !matches!(record.input.body, EventBody::Work { .. }) {
                    self.journal.mark_event(&record.key, DeliveryState::Rejected, &format!("{error:#}"))?;
                } else if state == DeliveryState::Dispatching {
                    self.journal.mark_event(
                        &record.key,
                        DeliveryState::Unknown,
                        &format!("delivery did not settle: {error:#}"),
                    )?;
                }
                let latest = self.journal.event(&record.key)?.context("event missing")?;
                if latest.state == DeliveryState::Pending {
                    self.journal.mark_event(
                        &record.key,
                        DeliveryState::Pending,
                        &format!("idempotent admission needs completion: {error:#}"),
                    )?;
                }
                let target = latest.task_id.as_deref().or(match &record.input.body {
                    EventBody::Followup { task_id, .. }
                    | EventBody::Answer { task_id, .. }
                    | EventBody::Cancel { task_id, .. } => Some(task_id.as_str()),
                    _ => None,
                });
                if let Some(binding) = target.map(|id| self.journal.binding(id)).transpose()?.flatten() {
                    let notice_id = format!("event-error:{}", record.key);
                    if self.journal.notice(&notice_id)?.is_none() {
                        self.journal.put_notice(&Notice {
                            id: notice_id,
                            task_id: binding.spec.task_id,
                            generation: binding.generation,
                            kind: "blocked".into(),
                            body: format!("Event {} requires inspection: {error:#}", record.key),
                            state: NoticeState::Pending,
                        })?;
                    }
                }
            }
        }
        for mut binding in self.journal.bindings()? {
            if self.active.contains_key(&binding.spec.task_id) {
                let task = self.coordination.task(&binding.spec.task_id)?;
                if task.state == "running" {
                    self.coordination.change(
                        &task.id,
                        false,
                        Request { op: "heartbeat".into(), request_id: core::fresh_id(), ..Request::default() },
                    )?;
                }
                continue;
            }
            if binding.phase != ExecutionPhase::Queued || self.active.len() >= self.config.max_parallel {
                continue;
            }
            if self.coordination.task(&binding.spec.task_id)?.state == "unconfirmed" {
                binding.phase = ExecutionPhase::NeedsReconciliation;
                self.journal.put_binding(&binding)?;
                self.journal.put_notice(&Notice{id:format!("claim-recovery:{}",binding.spec.task_id),task_id:binding.spec.task_id.clone(),generation:binding.generation,
                    kind:"blocked".into(),body:"Coordinator claim survived restart before Starting; explicitly prove no original admission before recovering".into(),state:NoticeState::Pending})?;
                continue;
            }
            if binding.spec.authorization_revision != self.config.authorization_revision
                || binding.adapter_revision != self.adapter_fingerprint()
            {
                binding.phase = ExecutionPhase::NeedsReconciliation;
                self.journal.put_binding(&binding)?;
                self.journal.put_notice(&Notice {
                    id: format!("policy-changed:{}", binding.spec.task_id),
                    task_id: binding.spec.task_id.clone(),
                    generation: binding.generation,
                    kind: "blocked".into(),
                    body: "Queued Host policy or adapter configuration changed; no execution admitted".into(),
                    state: NoticeState::Pending,
                })?;
                continue;
            }
            let claimed = match self.coordination.store.execute(
                &self.coordination.worker.token,
                &Request {
                    op: "claim".into(),
                    task_id: binding.spec.task_id.clone(),
                    request_id: format!("claim:{}", binding.spec.task_id),
                    ..Request::default()
                },
            ) {
                Ok(outcome) => outcome.into_task().context("claim returned no Task")?,
                Err(error) if error.to_string().contains("workspace is still owned by task") => continue,
                Err(error) => return Err(error.into()),
            };
            ensure!(claimed.generation == binding.generation, "claim generation mismatch");
            binding.phase = ExecutionPhase::Starting;
            self.journal.put_binding(&binding)?;
            match self.launch(&binding, false).await {
                Ok(()) => {}
                Err(error) => {
                    binding.phase = ExecutionPhase::NeedsReconciliation;
                    self.journal.put_binding(&binding)?;
                    self.journal.put_notice(&Notice {
                        id: format!("blocked:{}:{}", claimed.id, binding.attempt),
                        task_id: claimed.id,
                        generation: binding.generation,
                        kind: "blocked".into(),
                        body: format!("Task admission failed; inspect original evidence: {error:#}"),
                        state: NoticeState::Pending,
                    })?;
                }
            }
        }
        Ok(())
    }

    fn process_event(&mut self, record: &EventRecord) -> Result<()> {
        self.policy(&record.input)?;
        if !record.input.explicit
            && matches!(record.input.body, EventBody::Work { .. })
            && self.config.accept_mode != AcceptMode::Auto
        {
            return self.journal.mark_event(
                &record.key,
                DeliveryState::Observed,
                "inferred event recorded without execution",
            );
        }
        match &record.input.body {
            EventBody::Work { title, workspace, objective } => {
                let workspace = workspace.canonicalize()?;
                ensure!(
                    workspace.starts_with(&self.config.workspace_root)
                        && !self.config.state_dir.starts_with(&workspace),
                    "workspace outside Host grant or contains control state"
                );
                let id = format!("host-{}", &record.key[..24.min(record.key.len())]);
                let task = self
                    .coordination
                    .store
                    .execute(
                        &self.coordination.master.token,
                        &Request {
                            op: "create".into(),
                            request_id: record.key.clone(),
                            task_id: id.clone(),
                            title: title.clone(),
                            workspace: workspace.to_string_lossy().into_owned(),
                            mode: "write".into(),
                            body: objective.clone(),
                            worker_id: self.coordination.worker.id.clone(),
                            ..Request::default()
                        },
                    )?
                    .into_task()
                    .context("create returned no Task")?;
                if self.journal.binding(&task.id)?.is_none() {
                    let root = self.config.state_dir.join("tasks").join(&task.id);
                    std::fs::create_dir_all(&root)?;
                    self.journal.put_binding(&ExecutionBinding {
                        spec: TaskSpec {
                            task_id: task.id.clone(),
                            criteria_revision: "host-artifact-v1".into(),
                            authorization_revision: self.config.authorization_revision.clone(),
                            kind: TaskKind::Reasoning,
                            evidence_dir: root.join("attempt-0"),
                            max_calls: self.config.max_calls,
                            max_runs: self.config.max_runs,
                            deadline_unix_ms: now_ms() + self.config.max_seconds * 1000,
                        },
                        options: TaskOptions::default(),
                        session_path: root.join("session.json"),
                        environment_path: root.join("environment"),
                        workspace,
                        generation: 1,
                        phase: ExecutionPhase::Queued,
                        attempt: 0,
                        adapter_revision: self.adapter_fingerprint(),
                        initial_run_id: format!("initial-{}", task.id),
                    })?;
                }
                self.journal.bind_event(&record.key, &task.id, 1)?;
                self.journal.mark_event(
                    &record.key,
                    DeliveryState::Applied,
                    "original coordinator Task created; not executed or accepted",
                )?;
            }
            EventBody::Followup { task_id, generation, text } => {
                self.check_target(task_id, *generation)?;
                let active =
                    self.active.get(task_id).context("Task is not active; new work needs explicit admission")?;
                self.journal.bind_event(&record.key, task_id, *generation)?;
                self.journal.mark_event(&record.key, DeliveryState::Dispatching, "durable intent before steering")?;
                if let Err(error) =
                    active.steering.lock().unwrap().submit(text.clone(), json!({"host_event_key":record.key}))
                {
                    self.journal.mark_event(
                        &record.key,
                        DeliveryState::Unapplied,
                        "steering refused before admission",
                    )?;
                    return Err(error);
                }
            }
            EventBody::Answer { task_id, generation, question_id, text } => {
                self.check_target(task_id, *generation)?;
                let binding = self.journal.binding(task_id)?.context("answer Task missing")?;
                ensure!(
                    self.active.contains_key(task_id) || binding.phase == ExecutionPhase::NeedsReconciliation,
                    "answer needs original active or reconciling Task"
                );
                self.journal.answer(question_id, task_id, *generation, text)?;
                self.journal.bind_event(&record.key, task_id, *generation)?;
                self.journal.mark_event(
                    &record.key,
                    DeliveryState::Applied,
                    "answer saved for original tool callback",
                )?;
            }
            EventBody::Cancel { task_id, generation } => {
                self.check_target(task_id, *generation)?;
                self.active.get(task_id).context("Task is not active")?.handle.cancel();
                self.journal.bind_event(&record.key, task_id, *generation)?;
                self.journal.mark_event(
                    &record.key,
                    DeliveryState::Applied,
                    "cancellation requested, not effect settlement",
                )?;
            }
        }
        Ok(())
    }

    fn check_target(&self, id: &str, generation: i64) -> Result<ExecutionBinding> {
        let binding = self.journal.binding(id)?.context("unknown Task binding")?;
        let task = self.coordination.task(id)?;
        ensure!(task.generation == generation && binding.generation == generation, "Task generation changed");
        ensure!(
            binding.spec.authorization_revision == self.config.authorization_revision,
            "Task authorization changed"
        );
        ensure!(binding.spec.deadline_unix_ms > now_ms(), "original deadline elapsed");
        Ok(binding)
    }

    async fn launch(&mut self, binding: &ExecutionBinding, resuming: bool) -> Result<()> {
        ensure!(binding.adapter_revision == self.adapter_fingerprint(), "Host adapter configuration changed");
        let task = self.coordination.task(&binding.spec.task_id)?;
        let prepared = self.adapter.prepare(binding, &task, self.journal.clone())?;
        ensure!(
            prepared.request.state_path == binding.session_path && prepared.request.cwd == binding.workspace,
            "adapter changed Task paths"
        );
        let steering = prepared.request.steering.clone();
        // Commit execution intent before spawning. A background callback may
        // immediately advance to WaitingHuman/ReadyForReview; never overwrite it.
        let mut running = binding.clone();
        running.phase = ExecutionPhase::Running;
        self.journal.put_binding(&running)?;
        let host = Arc::new(ReviewHost {
            inner: prepared.host,
            journal: self.journal.clone(),
            coordination: self.coordination.clone(),
            binding: binding.clone(),
            mode: self.config.review_mode,
        });
        let started = if resuming {
            harness::resume(binding.spec.clone(), prepared.request, host, binding.options).await
        } else {
            harness::start_with_options(binding.spec.clone(), prepared.request, host, binding.options)
        };
        let handle = match started {
            Ok(handle) => handle,
            Err(error) => {
                running.phase = ExecutionPhase::NeedsReconciliation;
                self.journal.put_binding(&running)?;
                return Err(error);
            }
        };
        self.active.insert(task.id.clone(), Active { handle: handle.clone(), steering });
        if self.coordination.task(&task.id)?.state == "unconfirmed" {
            if let Err(error) = self.coordination.change(
                &task.id,
                false,
                Request { op: "heartbeat".into(), request_id: core::fresh_id(), ..Request::default() },
            ) {
                handle.cancel();
                handle.wait().await;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Explicit operator recovery, after reviewing original effects. Adapter's
    /// reconcile_resume must provide committed, settled native receipts.
    pub async fn recover(&mut self, id: &str, not_started: bool) -> Result<()> {
        ensure!(!self.active.contains_key(id), "Task already active");
        ensure!(self.active.len() < self.config.max_parallel, "Host concurrency full");
        let mut binding = self.journal.binding(id)?.context("unknown Task")?;
        ensure!(binding.phase == ExecutionPhase::NeedsReconciliation, "Task does not need recovery");
        ensure!(self.coordination.task(id)?.generation == binding.generation, "recovery generation changed");
        let released = binding.spec.evidence_dir.join("released.json");
        if released.is_file() {
            let outcome: TaskOutcome = serde_json::from_slice(&std::fs::read(released)?)?;
            ensure!(
                outcome.task_id == id && outcome.fence_presence == FencePresence::Released,
                "original release receipt mismatch"
            );
            binding.phase = ExecutionPhase::Finished;
            self.journal.complete(
                &binding,
                &Notice {
                    id: format!("result:{id}:{}", binding.attempt),
                    task_id: id.into(),
                    generation: binding.generation,
                    kind: "result".into(),
                    body: serde_json::to_string(&outcome)?,
                    state: NoticeState::Pending,
                },
            )?;
            return Ok(());
        }
        let task = self.coordination.task(id)?;
        if matches!(task.state.as_str(), "awaiting_review" | "blocked" | "passed") {
            let (candidate, _receipt) = checked_review(&binding)?;
            if task.state == "passed" {
                return self.finish_recovered_review(&binding, &task, &candidate);
            }
            ensure!(
                task.snapshot == ara_lite::snapshot::git_snapshot(&binding.workspace)?,
                "original review workspace changed"
            );
            binding.phase = ExecutionPhase::ReadyForReview;
            self.journal.put_binding(&binding)?;
            self.journal.put_notice(&Notice {
                id: format!("review:{id}:{}", candidate.basis.run_number),
                task_id: id.into(),
                generation: binding.generation,
                kind: "review".into(),
                body: serde_json::to_string(&ReviewOffer {
                    task_id: id.into(),
                    generation: binding.generation,
                    snapshot: task.snapshot,
                    basis: candidate.basis,
                })?,
                state: NoticeState::Pending,
            })?;
            return Ok(());
        }
        self.check_target(id, binding.generation)?;
        if not_started {
            ensure!(
                binding.attempt == 0
                    && !binding.spec.evidence_dir.exists()
                    && !binding.session_path.exists()
                    && !core::session_journal::Journal::for_session(&binding.session_path).exists(),
                "cannot prove no original admission"
            );
        } else {
            binding.attempt += 1;
            binding.spec.evidence_dir =
                self.config.state_dir.join("tasks").join(id).join(format!("attempt-{}", binding.attempt));
            self.journal.put_binding(&binding)?;
        }
        self.launch(&binding, !not_started).await
    }

    pub fn review(&self, id: &str, review_id: &str, snapshot: &str, decision: &str, reason: &str) -> Result<()> {
        let binding = self.journal.binding(id)?.context("unknown review Task")?;
        ensure!(binding.phase == ExecutionPhase::ReadyForReview, "original Task is not awaiting review");
        let task = self.coordination.task(id)?;
        ensure!(
            task.generation == binding.generation
                && binding.spec.authorization_revision == self.config.authorization_revision,
            "review authority changed"
        );
        if self.active.contains_key(id) {
            self.check_target(id, task.generation)?;
        } else {
            checked_review(&binding)?;
        }
        let notice = self.journal.notice(review_id)?.context("unknown review offer")?;
        let offer: ReviewOffer = serde_json::from_str(&notice.body)?;
        ensure!(
            notice.kind == "review"
                && offer.task_id == id
                && offer.generation == binding.generation
                && offer.snapshot == snapshot,
            "review identity mismatch"
        );
        ensure!(
            ara_lite::snapshot::git_snapshot(&binding.workspace)? == snapshot && task.snapshot == snapshot,
            "review workspace changed"
        );
        let decided = self.coordination.change(
            id,
            true,
            Request {
                op: "review".into(),
                request_id: format!(
                    "decision-{:x}",
                    Sha256::digest(serde_json::to_vec(&(review_id, snapshot, decision, reason))?)
                ),
                snapshot: snapshot.into(),
                decision: decision.into(),
                body: reason.into(),
                ..Request::default()
            },
        )?;
        if !self.active.contains_key(id) {
            let (candidate, _) = checked_review(&binding)?;
            if decided.state == "passed" {
                self.finish_recovered_review(&binding, &decided, &candidate)?;
            } else {
                let mut pending = binding;
                pending.phase = ExecutionPhase::NeedsReconciliation;
                self.journal.put_binding(&pending)?;
            }
        }
        Ok(())
    }

    fn finish_recovered_review(&self, binding: &ExecutionBinding, task: &Task, candidate: &Candidate) -> Result<()> {
        let mut finished = binding.clone();
        finished.phase = ExecutionPhase::Finished;
        let body = json!({"kind":"recovered_human_review","task_id":task.id,"accepted_snapshot":task.snapshot,
            "candidate_basis":candidate.basis,"business_state":task.state,"native_fence_retained":true,
            "native_fence_directory":binding.session_path.parent(),"not_a_harness_task_outcome":true});
        let path = binding.spec.evidence_dir.join("recovered-review.json");
        let mut file = std::fs::File::create(&path)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&body)?)?;
        file.sync_all()?;
        self.journal.complete(
            &finished,
            &Notice {
                id: format!("recovered-review:{}:{}", task.id, candidate.basis.run_number),
                task_id: task.id.clone(),
                generation: task.generation,
                kind: "result".into(),
                body: serde_json::to_string(&body)?,
                state: NoticeState::Pending,
            },
        )
    }

    /// reserve-before-output; the transport ACK is separate from business review.
    pub fn take_notices(&self) -> Result<Vec<Notice>> {
        let notices = self.journal.notices(NoticeState::Pending)?;
        for notice in &notices {
            self.journal.mark_notice(&notice.id, NoticeState::Dispatching)?;
        }
        Ok(notices)
    }
    pub fn ack_notice(&self, id: &str) -> Result<()> {
        self.journal.mark_notice(id, NoticeState::Sent)
    }
    pub fn settle_event(&self, key: &str, state: DeliveryState, evidence: &str) -> Result<()> {
        ensure!(!evidence.trim().is_empty(), "explicit reconciliation evidence required");
        ensure!(
            self.journal.event(key)?.is_some_and(|event| event.state == DeliveryState::Unknown),
            "event is not unknown"
        );
        ensure!(
            matches!(state, DeliveryState::Applied | DeliveryState::Unapplied | DeliveryState::Rejected),
            "unsafe reconciliation target"
        );
        self.journal.mark_event(key, state, evidence)
    }
    pub async fn shutdown(&mut self) -> Result<()> {
        for active in self.active.values() {
            active.handle.cancel();
        }
        for active in self.active.values() {
            active.handle.wait().await;
        }
        self.collect_completed()
    }
}

#[derive(Serialize, Deserialize)]
struct ReviewOffer {
    task_id: String,
    generation: i64,
    snapshot: String,
    basis: ReviewBasis,
}

fn checked_review(binding: &ExecutionBinding) -> Result<(Candidate, ReviewReceipt)> {
    ensure!(
        core::agent::interrupted_run(&binding.session_path, &binding.workspace)?.is_none(),
        "review recovery requires a closed original Run"
    );
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&binding.spec.evidence_dir)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().ends_with("-candidate.json") {
            let candidate: Candidate = serde_json::from_slice(&std::fs::read(entry.path())?)?;
            candidates.push(candidate);
        }
    }
    let candidate = candidates
        .into_iter()
        .max_by_key(|candidate| candidate.basis.run_number)
        .context("original candidate unavailable")?;
    ensure!(
        candidate.basis.task_id == binding.spec.task_id
            && candidate.basis.authorization_revision == binding.spec.authorization_revision
            && candidate.basis.session_path == binding.session_path.canonicalize()?,
        "review candidate identity changed"
    );
    let receipt: ReviewReceipt = serde_json::from_slice(&std::fs::read(
        binding.spec.evidence_dir.join(format!("artifact-receipt-{}.json", candidate.basis.run_number)),
    )?)?;
    ensure!(
        receipt.basis == candidate.basis
            && receipt.committed
            && receipt.settlement == Settlement::Settled
            && matches!(receipt.verdict, Verdict::Verified),
        "original artifact review is invalid"
    );
    let events = std::fs::read(&candidate.basis.events_path)?;
    ensure!(format!("{:x}", Sha256::digest(&events)) == candidate.basis.events_sha256, "review events changed");
    let events: Vec<EventEnvelope> =
        std::str::from_utf8(&events)?.lines().map(serde_json::from_str).collect::<std::result::Result<_, _>>()?;
    ensure!(
        events.iter().any(|event| event.event["type"] == "agent_end"
            && event.event["status"] == "completed"
            && event.event["code"] == 0)
            && !events.iter().any(|event| event.event["effects_unknown"] == true),
        "review does not prove a complete ordinary Run"
    );
    Ok((candidate, receipt))
}

struct ReviewHost {
    inner: Arc<dyn HarnessHost>,
    journal: Arc<Journal>,
    coordination: Arc<Coordination>,
    binding: ExecutionBinding,
    mode: ReviewMode,
}
#[async_trait]
impl HarnessHost for ReviewHost {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        let mut receipt = self.inner.verify(candidate.clone(), cancel.clone()).await?;
        if !matches!(receipt.verdict, Verdict::Verified) {
            return Ok(receipt);
        }
        ensure!(
            receipt.basis == candidate.basis
                && receipt.committed
                && receipt.settlement == Settlement::Settled
                && !receipt.evidence.is_empty(),
            "invalid delegated artifact receipt; no coordinator submission"
        );
        let proof =
            self.binding.spec.evidence_dir.join(format!("artifact-receipt-{}.json", candidate.basis.run_number));
        let mut file = std::fs::File::create(proof)?;
        std::io::Write::write_all(&mut file, &serde_json::to_vec(&receipt)?)?;
        file.sync_all()?;
        while self.coordination.task(&candidate.basis.task_id)?.state == "unconfirmed" {
            tokio::select! {_=cancel.cancelled()=>bail!("review canceled before recovered ownership publication"),_=tokio::time::sleep(std::time::Duration::from_millis(25))=>{}}
        }
        let workspace = self.binding.workspace.clone();
        let snapshot = tokio::task::spawn_blocking(move || ara_lite::snapshot::git_snapshot(&workspace)).await??;
        let submitted = self.coordination.change(
            &candidate.basis.task_id,
            false,
            Request {
                op: "submit".into(),
                request_id: format!("submit:{}:{}", candidate.basis.task_id, candidate.basis.run_number),
                snapshot: snapshot.clone(),
                body: format!("Artifact checked; original review {}", candidate.basis.events_sha256),
                ..Request::default()
            },
        )?;
        let offer = ReviewOffer {
            task_id: submitted.id.clone(),
            generation: submitted.generation,
            snapshot: snapshot.clone(),
            basis: candidate.basis.clone(),
        };
        let mut binding = self.journal.binding(&submitted.id)?.context("review binding missing")?;
        binding.phase = ExecutionPhase::ReadyForReview;
        self.journal.put_binding(&binding)?;
        let review_id = format!("review:{}:{}", submitted.id, candidate.basis.run_number);
        self.journal.put_notice(&Notice {
            id: review_id.clone(),
            task_id: submitted.id.clone(),
            generation: submitted.generation,
            kind: "review".into(),
            body: serde_json::to_string(&offer)?,
            state: NoticeState::Pending,
        })?;
        if self.mode == ReviewMode::VerifiedArtifact {
            self.coordination.change(
                &submitted.id,
                true,
                Request {
                    op: "review".into(),
                    request_id: format!("auto:{review_id}"),
                    snapshot,
                    decision: "pass".into(),
                    body: "Explicit Host verified-artifact policy; actual verifier passed".into(),
                    ..Request::default()
                },
            )?;
        }
        loop {
            if cancel.is_cancelled() {
                bail!("review canceled; original outcome and evidence retained");
            }
            let task = self.coordination.task(&submitted.id)?;
            if task.state == "passed" {
                let path =
                    self.binding.spec.evidence_dir.join(format!("host-review-{}.json", candidate.basis.run_number));
                let mut file = std::fs::File::create(&path)?;
                std::io::Write::write_all(
                    &mut file,
                    &serde_json::to_vec(&json!({"artifact_receipt":receipt,"decision":task}))?,
                )?;
                file.sync_all()?;
                receipt.evidence.push(path.display().to_string());
                return Ok(receipt);
            }
            if task.state == "running" {
                binding.phase = ExecutionPhase::Running;
                self.journal.put_binding(&binding)?;
                let path =
                    self.binding.spec.evidence_dir.join(format!("host-review-{}.json", candidate.basis.run_number));
                let original = receipt.clone();
                receipt.verdict = Verdict::NeedsChanges { feedback: task.body.clone() };
                let mut file = std::fs::File::create(&path)?;
                std::io::Write::write_all(
                    &mut file,
                    &serde_json::to_vec(
                        &json!({"artifact_receipt":original,"returned_receipt":receipt,"decision":task}),
                    )?,
                )?;
                file.sync_all()?;
                receipt.evidence.push(path.display().to_string());
                return Ok(receipt);
            }
            if task.state == "blocked" {
                bail!("human review blocked Task: {}", task.body);
            }
            tokio::select! {_=cancel.cancelled()=>bail!("review canceled"),_=tokio::time::sleep(std::time::Duration::from_millis(50))=>{}}
        }
    }
    async fn prepare_continuation(&self, next: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.inner.prepare_continuation(next, cancel).await
    }
    async fn reconcile_resume(&self, next: ResumeReview, cancel: CancellationToken) -> Result<ResumeReceipt> {
        self.inner.reconcile_resume(next, cancel).await
    }
}
