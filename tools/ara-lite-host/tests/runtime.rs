#[path = "../../../crates/ara-agent-lite-harness/tests/common/mod.rs"]
mod common;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_harness::{
    core::{rpi_agent::AgentTool, RunRequest},
    Candidate, Continuation, HarnessHost, ReviewReceipt, TaskKind, TaskOptions, TaskSpec,
};
use ara_lite::types::Task;
use ara_lite_host::{
    evidence,
    journal::Journal,
    runtime::{AcceptMode, Adapter, Config, Prepared, ReviewMode, Runtime},
    types::*,
};
use async_trait::async_trait;
use common::{call, text, write_answer, DelayedCancelTool, Fault, Host, Provider};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(15);

#[tokio::test]
async fn released_result_can_be_collected_after_execution_deadline_without_new_calls() {
    let mut fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    fixture.config.max_seconds = 2;
    let provider = Arc::new(Provider::new(vec![write_answer("correct"), text("Done.")]));
    let adapter = Arc::new(FixtureAdapter::new(provider.clone(), Fault::None));
    let mut runtime = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
    let event = runtime.ingest(fixture.work("released-cut")).unwrap();
    runtime.tick().await.unwrap();
    let id = event_task(&runtime, &event);
    let binding = runtime.journal.binding(&id).unwrap().unwrap();
    tokio::time::timeout(WAIT, async {
        while !binding.spec.evidence_dir.join("released.json").is_file() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // No Host tick: the actual Harness is released but Host completion is not
    // recorded. Reopening the durable Host reproduces that finalization cut.
    drop(runtime);
    while now_ms() <= binding.spec.deadline_unix_ms {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut reopened = Runtime::open(fixture.config.clone(), adapter).unwrap();
    assert_eq!(reopened.journal.binding(&id).unwrap().unwrap().phase, ExecutionPhase::NeedsReconciliation);
    reopened.recover(&id, false).await.unwrap();
    assert_eq!(reopened.journal.binding(&id).unwrap().unwrap().phase, ExecutionPhase::Finished);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(reopened.status().unwrap()["tasks"][&id]["state"], "passed");
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn claimed_before_starting_cut_recovers_original_queued_binding() {
    let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    let second_workspace = fixture.config.workspace_root.join("second");
    repository(&second_workspace);
    let provider = Arc::new(Provider::new(vec![
        call("hold", "delayed_cancel", json!({})),
        write_answer("correct"),
        text("Done."),
    ]));
    let tool = Arc::new(DelayedCancelTool::new(&fixture.workspace));
    let mut factory = FixtureAdapter::new(provider.clone(), Fault::None);
    factory.tool = Some(tool.clone());
    let factory = Arc::new(factory);
    let mut runtime = Runtime::open(fixture.config.clone(), factory.clone()).unwrap();
    runtime.ingest(fixture.work("occupy")).unwrap();
    runtime.tick().await.unwrap();
    tokio::time::timeout(WAIT, async {
        while !tool.started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut second = fixture.work("queued-cut");
    if let EventBody::Work { workspace, .. } = &mut second.body {
        *workspace = second_workspace;
    }
    let event = runtime.ingest(second).unwrap();
    runtime.tick().await.unwrap();
    let id = event_task(&runtime, &event);
    let before = runtime.journal.binding(&id).unwrap().unwrap();
    assert_eq!(before.phase, ExecutionPhase::Queued);
    tool.release_settlement();
    runtime.shutdown().await.unwrap();
    drop(runtime);
    // Use the real coordinator API to materialize exactly the durable frontier
    // after claim commit and before Host Starting intent. No model has run for it.
    let store = ara_lite::store::Store::open(&fixture.config.state_dir.join("coordinator")).unwrap();
    let worker = ara_lite::profile::read_connection(&fixture.config.state_dir.join("worker.json")).unwrap();
    store
        .execute(
            &worker.token,
            &ara_lite::types::Request {
                op: "claim".into(),
                task_id: id.clone(),
                request_id: format!("claim:{id}"),
                ..Default::default()
            },
        )
        .unwrap();
    drop(store);
    let mut reopened = Runtime::open(fixture.config.clone(), factory).unwrap();
    assert_eq!(reopened.status().unwrap()["tasks"][&id]["state"], "unconfirmed");
    reopened.tick().await.unwrap();
    assert_eq!(reopened.journal.binding(&id).unwrap().unwrap().phase, ExecutionPhase::NeedsReconciliation);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    reopened.recover(&id, true).await.unwrap();
    tick_until(&mut reopened, |state| {
        state["tasks"][&id]["state"] == "passed"
            && state["execution"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["spec"]["task_id"] == id && entry["phase"] == "finished")
    })
    .await;
    let after = reopened.journal.binding(&id).unwrap().unwrap();
    assert_eq!(after.session_path, before.session_path);
    assert_eq!(after.spec.deadline_unix_ms, before.spec.deadline_unix_ms);
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
    reopened.shutdown().await.unwrap();
}

fn repository(path: &Path) {
    std::fs::create_dir_all(path).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "-c",
            "user.name=Host Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "--no-gpg-sign",
            "-qm",
            "fixture baseline",
        ],
    ] {
        let result = Command::new("git").arg("-C").arg(path).args(args).output().unwrap();
        assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    config: Config,
    workspace: PathBuf,
}

impl Fixture {
    fn new(mode: ReviewMode) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let workspace_root = directory.path().join("work");
        let workspace = workspace_root.join("project");
        repository(&workspace);
        let config = Config {
            state_dir: directory.path().join("state"),
            workspace_root,
            authorization_revision: "test-grant-v1".into(),
            allowed_actors: vec!["operator".into()],
            max_parallel: 1,
            max_calls: 8,
            max_runs: 2,
            max_seconds: 30,
            accept_mode: AcceptMode::Observe,
            review_mode: mode,
        };
        Self { _directory: directory, config, workspace }
    }

    fn work(&self, event_id: &str) -> InputEvent {
        InputEvent {
            source: "local-operator".into(),
            event_id: event_id.into(),
            actor: "operator".into(),
            authorization_revision: self.config.authorization_revision.clone(),
            explicit: true,
            body: EventBody::Work {
                title: "Inspect actual artifact".into(),
                workspace: self.workspace.clone(),
                objective: "Write the checked answer into the fixture artifact.".into(),
            },
        }
    }
}

struct BoundHost {
    inner: Arc<Host>,
    session: PathBuf,
    environment: PathBuf,
}

#[async_trait]
impl HarnessHost for BoundHost {
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        self.inner.verify(candidate, cancellation).await
    }

    async fn prepare_continuation(&self, next: Continuation, cancellation: CancellationToken) -> Result<RunRequest> {
        let mut request = self.inner.prepare_continuation(next, cancellation).await?;
        request.state_path = self.session.clone();
        request.environment_state = self.environment.clone();
        Ok(request)
    }
}

struct FixtureAdapter {
    provider: Arc<Provider>,
    fault: Fault,
    tool: Option<Arc<dyn AgentTool>>,
    hosts: Mutex<BTreeMap<String, Arc<Host>>>,
    prepares: AtomicUsize,
}

impl FixtureAdapter {
    fn new(provider: Arc<Provider>, fault: Fault) -> Self {
        Self { provider, fault, tool: None, hosts: Mutex::new(BTreeMap::new()), prepares: AtomicUsize::new(0) }
    }
}

impl Adapter for FixtureAdapter {
    fn revision(&self) -> String {
        "controlled-adapter-v1".into()
    }

    fn prepare(&self, binding: &ExecutionBinding, _: &Task, _: Arc<Journal>) -> Result<Prepared> {
        self.prepares.fetch_add(1, Ordering::SeqCst);
        let mut hosts = self.hosts.lock().unwrap();
        let host = hosts
            .entry(binding.spec.task_id.clone())
            .or_insert_with(|| {
                let mut host = Host::new(&binding.workspace, &self.provider, self.fault);
                host.read_tool = self.tool.clone();
                Arc::new(host)
            })
            .clone();
        let mut request = host.request(&binding.initial_run_id);
        request.state_path = binding.session_path.clone();
        request.environment_state = binding.environment_path.clone();
        Ok(Prepared {
            request,
            host: Arc::new(BoundHost {
                inner: host,
                session: binding.session_path.clone(),
                environment: binding.environment_path.clone(),
            }),
        })
    }
}

async fn tick_until(runtime: &mut Runtime, mut done: impl FnMut(&Value) -> bool) {
    tokio::time::timeout(WAIT, async {
        loop {
            runtime.tick().await.unwrap();
            if done(&runtime.status().unwrap()) {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

fn terminal(status: &Value) -> bool {
    status["execution"]
        .as_array()
        .unwrap()
        .iter()
        .all(|binding| matches!(binding["phase"].as_str(), Some("finished" | "needs_reconciliation")))
}

fn event_task(runtime: &Runtime, event: &EventRecord) -> String {
    runtime.journal.event(&event.key).unwrap().unwrap().task_id.unwrap()
}

fn all_notices(runtime: &Runtime) -> Vec<Notice> {
    [NoticeState::Pending, NoticeState::Dispatching, NoticeState::Sent, NoticeState::Unknown]
        .into_iter()
        .flat_map(|state| runtime.journal.notices(state).unwrap())
        .collect()
}

async fn offer(runtime: &mut Runtime, task_id: &str, run: usize) -> Notice {
    let id = format!("review:{task_id}:{run}");
    tokio::time::timeout(WAIT, async {
        loop {
            runtime.tick().await.unwrap();
            if let Some(notice) = all_notices(runtime).into_iter().find(|notice| notice.id == id) {
                return notice;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn work_identity_is_durable_and_observation_does_not_execute() {
    let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    let provider = Arc::new(Provider::new(vec![write_answer("correct"), text("Submitted.")]));
    let adapter = Arc::new(FixtureAdapter::new(provider.clone(), Fault::None));
    let input = fixture.work("work-1");
    let key;
    {
        let mut runtime = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
        let record = runtime.ingest(input.clone()).unwrap();
        key = record.key.clone();
        assert_eq!(runtime.ingest(input.clone()).unwrap().key, key);
        let mut changed = input.clone();
        changed.body = EventBody::Work {
            title: "Changed under same identity".into(),
            workspace: fixture.workspace.clone(),
            objective: "Different request.".into(),
        };
        assert!(runtime.ingest(changed).is_err());
        tick_until(&mut runtime, terminal).await;
        assert_eq!(runtime.status().unwrap()["tasks"].as_object().unwrap().len(), 1);
        assert_eq!(runtime.journal.event(&key).unwrap().unwrap().state, DeliveryState::Applied);
        assert_eq!(std::fs::read(fixture.workspace.join("answer.txt")).unwrap(), b"correct");
        let mut inferred = fixture.work("inferred-1");
        inferred.explicit = false;
        let observed = runtime.ingest(inferred).unwrap();
        runtime.tick().await.unwrap();
        assert_eq!(runtime.journal.event(&observed.key).unwrap().unwrap().state, DeliveryState::Observed);
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
    }
    let mut reopened = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
    assert_eq!(reopened.ingest(input).unwrap().state, DeliveryState::Applied);
    reopened.tick().await.unwrap();
    assert_eq!(reopened.status().unwrap()["tasks"].as_object().unwrap().len(), 1);
    assert_eq!(adapter.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_harness_receipt_cannot_pass_the_business_task() {
    for fault in [Fault::BadBasis, Fault::Unknown, Fault::Uncommitted] {
        let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
        let provider = Arc::new(Provider::new(vec![write_answer("correct"), text("Submitted.")]));
        let adapter = Arc::new(FixtureAdapter::new(provider.clone(), fault));
        let mut runtime = Runtime::open(fixture.config.clone(), adapter).unwrap();
        let event = runtime.ingest(fixture.work("invalid-receipt")).unwrap();
        tick_until(&mut runtime, terminal).await;
        let id = event_task(&runtime, &event);
        assert_ne!(runtime.status().unwrap()["tasks"][&id]["state"], "passed");
        let binding = runtime.journal.binding(&id).unwrap().unwrap();
        assert_eq!(binding.phase, ExecutionPhase::NeedsReconciliation);
        assert_eq!(provider.requests.lock().unwrap().len(), 2);
        let outcome: Value =
            serde_json::from_slice(&std::fs::read(binding.spec.evidence_dir.join("host-outcome.json")).unwrap())
                .unwrap();
        assert_eq!(outcome["task_accepted"], false);
        assert_eq!(outcome["stop"], "ReconciliationRequired");
    }
}

#[tokio::test]
async fn uncertain_followup_remains_unknown_after_runtime_reopen_without_replay() {
    let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    let provider = Arc::new(Provider::new(vec![write_answer("correct"), text("Submitted.")]));
    let adapter = Arc::new(FixtureAdapter::new(provider.clone(), Fault::None));
    let key;
    {
        let mut runtime = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
        let work = runtime.ingest(fixture.work("work-before-uncertain")).unwrap();
        tick_until(&mut runtime, terminal).await;
        let id = event_task(&runtime, &work);
        let mut followup = fixture.work("uncertain-followup");
        followup.body = EventBody::Followup {
            task_id: id.clone(),
            generation: 1,
            text: "Inspect the original context, never replay on uncertainty.".into(),
        };
        let record = runtime.ingest(followup).unwrap();
        key = record.key;
        runtime.journal.bind_event(&key, &id, 1).unwrap();
        // Emulate the durable cut immediately before a steering dispatch; the
        // separate process test owns actual kill/restart coverage.
        runtime.journal.mark_event(&key, DeliveryState::Dispatching, "original dispatch outcome unavailable").unwrap();
    }
    let mut reopened = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
    assert_eq!(reopened.journal.event(&key).unwrap().unwrap().state, DeliveryState::Unknown);
    reopened.tick().await.unwrap();
    assert_eq!(reopened.journal.event(&key).unwrap().unwrap().state, DeliveryState::Unknown);
    assert!(reopened.journal.events(DeliveryState::Pending).unwrap().is_empty());
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert_eq!(adapter.prepares.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn shutdown_neither_dispatches_pending_work_nor_launches_queued_tasks() {
    let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    let provider = Arc::new(Provider::new(vec![call("inflight", "delayed_cancel", json!({}))]));
    let tool = Arc::new(DelayedCancelTool::new(&fixture.workspace));
    let mut factory = FixtureAdapter::new(provider.clone(), Fault::None);
    factory.tool = Some(tool.clone());
    let adapter = Arc::new(factory);
    let mut runtime = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
    let first = runtime.ingest(fixture.work("first-active")).unwrap();
    runtime.tick().await.unwrap();
    tokio::time::timeout(WAIT, async {
        while !tool.started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let second_workspace = fixture.config.workspace_root.join("second");
    repository(&second_workspace);
    let mut second = fixture.work("second-queued");
    second.body = EventBody::Work {
        title: "Queued only".into(),
        workspace: second_workspace,
        objective: "Do not launch during shutdown.".into(),
    };
    let queued = runtime.ingest(second).unwrap();
    runtime.tick().await.unwrap();
    let queued_id = event_task(&runtime, &queued);
    assert_eq!(runtime.journal.binding(&queued_id).unwrap().unwrap().phase, ExecutionPhase::Queued);
    let pending = runtime.ingest(fixture.work("third-still-pending")).unwrap();
    let release = tool.clone();
    let releaser = tokio::spawn(async move {
        tokio::time::timeout(WAIT, async {
            while !release.cancellation_seen.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        release.release_settlement();
    });
    tokio::time::timeout(WAIT, runtime.shutdown()).await.unwrap().unwrap();
    releaser.await.unwrap();
    assert!(tool.joined.load(Ordering::SeqCst));
    assert_eq!(adapter.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert_eq!(runtime.journal.event(&pending.key).unwrap().unwrap().state, DeliveryState::Pending);
    assert_eq!(runtime.journal.binding(&queued_id).unwrap().unwrap().phase, ExecutionPhase::Queued);
    let original = runtime.journal.binding(&event_task(&runtime, &first)).unwrap().unwrap();
    assert_eq!(original.phase, ExecutionPhase::NeedsReconciliation);
}

#[tokio::test]
async fn shutdown_before_first_tick_does_not_create_a_task() {
    let fixture = Fixture::new(ReviewMode::VerifiedArtifact);
    let provider = Arc::new(Provider::new(vec![]));
    let adapter = Arc::new(FixtureAdapter::new(provider.clone(), Fault::None));
    let mut runtime = Runtime::open(fixture.config.clone(), adapter.clone()).unwrap();
    let event = runtime.ingest(fixture.work("pending-at-shutdown")).unwrap();
    runtime.shutdown().await.unwrap();
    assert_eq!(runtime.journal.event(&event.key).unwrap().unwrap().state, DeliveryState::Pending);
    assert!(runtime.status().unwrap()["tasks"].as_object().unwrap().is_empty());
    assert_eq!(adapter.prepares.load(Ordering::SeqCst), 0);
    assert!(provider.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn human_review_is_snapshot_bound_and_changes_keep_the_original_budget() {
    let fixture = Fixture::new(ReviewMode::Human);
    let provider = Arc::new(Provider::new(vec![
        write_answer("correct"),
        text("First submission."),
        call("second-write", "write_answer", json!({"content":"correct"})),
        text("Second submission."),
    ]));
    let adapter = Arc::new(FixtureAdapter::new(provider.clone(), Fault::None));
    let mut runtime = Runtime::open(fixture.config.clone(), adapter).unwrap();
    let event = runtime.ingest(fixture.work("human-review")).unwrap();
    runtime.tick().await.unwrap();
    let id = event_task(&runtime, &event);
    let original = runtime.journal.binding(&id).unwrap().unwrap();
    let first = offer(&mut runtime, &id, 1).await;
    let first_body: Value = serde_json::from_str(&first.body).unwrap();
    let snapshot = first_body["snapshot"].as_str().unwrap();
    assert_eq!(runtime.status().unwrap()["tasks"][&id]["state"], "awaiting_review");
    assert!(runtime.review(&id, "wrong-review-id", snapshot, "pass", "Invalid identity").is_err());
    std::fs::write(fixture.workspace.join("answer.txt"), "changed after submission").unwrap();
    assert!(runtime.review(&id, &first.id, snapshot, "pass", "Stale snapshot").is_err());
    std::fs::write(fixture.workspace.join("answer.txt"), "correct").unwrap();
    runtime
        .review(
            &id,
            &first.id,
            snapshot,
            "changes_requested",
            "Keep the artifact and demonstrate a second bounded Run.",
        )
        .unwrap();
    let second = offer(&mut runtime, &id, 2).await;
    let second_body: Value = serde_json::from_str(&second.body).unwrap();
    assert_eq!(second_body["basis"]["total_calls"], 4);
    runtime
        .review(
            &id,
            &second.id,
            second_body["snapshot"].as_str().unwrap(),
            "pass",
            "Actual artifact independently checked.",
        )
        .unwrap();
    tick_until(&mut runtime, terminal).await;
    assert_eq!(runtime.status().unwrap()["tasks"][&id]["state"], "passed");
    let final_binding = runtime.journal.binding(&id).unwrap().unwrap();
    assert_eq!(final_binding.session_path, original.session_path);
    assert_eq!(final_binding.spec.max_calls, original.spec.max_calls);
    assert_eq!(final_binding.spec.deadline_unix_ms, original.spec.deadline_unix_ms);
    let outcome: Value =
        serde_json::from_slice(&std::fs::read(final_binding.spec.evidence_dir.join("host-outcome.json")).unwrap())
            .unwrap();
    assert_eq!(outcome["observed_calls"], 4);
    assert_eq!(outcome["runs"], 2);
    assert_eq!(outcome["task_accepted"], false);
    assert_eq!(provider.requests.lock().unwrap().len(), 4);
}

fn source_binding(root: &Path, id: &str) -> ExecutionBinding {
    ExecutionBinding {
        spec: TaskSpec {
            task_id: id.into(),
            criteria_revision: "source-v1".into(),
            authorization_revision: "source-grant-v1".into(),
            kind: TaskKind::Reasoning,
            evidence_dir: root.join(format!("{id}-evidence")),
            max_calls: 4,
            max_runs: 1,
            deadline_unix_ms: now_ms() + 30_000,
        },
        options: TaskOptions::default(),
        session_path: root.join(format!("{id}-session.json")),
        environment_path: root.join(format!("{id}-environment")),
        workspace: root.into(),
        generation: 1,
        phase: ExecutionPhase::Finished,
        attempt: 0,
        adapter_revision: "source-adapter-v1".into(),
        initial_run_id: format!("{id}-initial"),
    }
}

#[test]
fn versioned_source_windows_enforce_owner_access_revocation_hash_and_utf8() {
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(&directory.path().join("sources.sqlite")).unwrap();
    for id in ["owner", "reader", "denied"] {
        journal.put_binding(&source_binding(directory.path(), id)).unwrap();
    }
    let source_path = directory.path().join("source.txt");
    let text = "甲乙\nsecret 42\n";
    std::fs::write(&source_path, text).unwrap();
    let mut source = EvidenceSource {
        id: "source-1".into(),
        task_id: "owner".into(),
        generation: 1,
        version: 1,
        path: source_path.clone(),
        sha256: format!("{:x}", Sha256::digest(text.as_bytes())),
        allowed_tasks: vec!["reader".into()],
        active: true,
    };
    journal.put_source(&source).unwrap();
    let window = evidence::read(&journal, "reader", 1, "source-1", 1, 0, 3).unwrap();
    assert_eq!(window.text, "甲");
    assert_eq!(window.next_offset, 3);
    assert!(window.truncated);
    assert_eq!(evidence::search(&journal, "reader", 1, "source-1", 1, "secret").unwrap(), Some(7));
    assert!(evidence::read(&journal, "denied", 1, "source-1", 1, 0, 3).is_err());
    assert!(evidence::read(&journal, "reader", 2, "source-1", 1, 0, 3).is_err());
    assert!(evidence::read(&journal, "reader", 1, "source-1", 1, 1, 3).is_err());
    assert!(evidence::read(&journal, "reader", 1, "source-1", 1, 0, 2).is_err());
    std::fs::write(&source_path, "changed bytes").unwrap();
    assert!(evidence::read(&journal, "reader", 1, "source-1", 1, 0, 3).is_err());
    std::fs::write(&source_path, text).unwrap();
    source.version = 2;
    source.allowed_tasks.clear();
    journal.put_source(&source).unwrap();
    assert!(evidence::read(&journal, "reader", 1, "source-1", 2, 0, 3).is_err());
    assert!(evidence::read(&journal, "owner", 1, "source-1", 1, 0, 3).is_err());
    assert_eq!(evidence::read(&journal, "owner", 1, "source-1", 2, 0, 3).unwrap().text, "甲");
    source.version = 3;
    source.active = false;
    journal.put_source(&source).unwrap();
    assert!(evidence::read(&journal, "owner", 1, "source-1", 3, 0, 3).is_err());
}
