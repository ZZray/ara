//! Owner loss in the middle of a real Core tool call, then Host-settled resume.
//! The owner is lost by dropping its whole Tokio runtime: no outcome is
//! published and the advisory lock is released, as after a process kill.
mod common;
use anyhow::Result;
use ara_agent_lite_harness::{
    core::{
        agent,
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunRequest,
    },
    *,
};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

/// Starts an external effect and never returns while its owner lives.
struct Hang {
    schema: Tool,
    executions: AtomicUsize,
    started: AtomicBool,
}
#[async_trait]
impl AgentTool for Hang {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "deploy_step"
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        self.started.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Ok(AgentToolResult::text("unreachable"))
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Unknown,
    Uncommitted,
    WrongCalls,
    Settled,
}

struct Resumer {
    inner: Arc<Host>,
    mode: Mutex<Mode>,
    reviews: Mutex<Vec<ResumeReview>>,
}
#[async_trait]
impl HarnessHost for Resumer {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.inner.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.inner.prepare_continuation(continuation, cancel).await
    }
    async fn reconcile_resume(&self, review: ResumeReview, _: CancellationToken) -> Result<ResumeReceipt> {
        self.reviews.lock().unwrap().push(review.clone());
        let mode = *self.mode.lock().unwrap();
        let calls = review.interrupted_run.as_ref().map(|run| run.open_tool_calls.clone()).unwrap_or_default();
        let tool_calls = calls
            .iter()
            .map(|call| ToolCallReconciliation {
                call_id: if mode == Mode::WrongCalls { "invented".into() } else { call.id.clone() },
                settlement: ToolCallSettlement::Completed {
                    output: "Host deploy log: step finished; artifact checksum verified.".into(),
                },
            })
            .collect();
        Ok(ResumeReceipt {
            committed: mode != Mode::Uncommitted,
            settlement: if mode == Mode::Unknown { Settlement::Unknown } else { Settlement::Settled },
            tool_calls,
            evidence: vec!["host-deploy-log:sha256:0f00".into()],
        })
    }
}

fn hang() -> Arc<Hang> {
    Arc::new(Hang {
        schema: Tool {
            name: "deploy_step".into(),
            description: "Run one deployment step.".into(),
            parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
            constrained_sampling: None,
        },
        executions: AtomicUsize::new(0),
        started: AtomicBool::new(false),
    })
}

/// Run the owner on its own runtime and drop that runtime mid-tool.
fn crash_owner(host: Arc<Host>, spec: TaskSpec, tool: Arc<Hang>) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let mut request = host.request("original");
            request.objective = "Call deploy_step once, then submit the answer.".into();
            let _handle = start(spec, request, host.clone()).unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
                while !tool.started.load(Ordering::SeqCst) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap();
        });
        runtime.shutdown_timeout(Duration::from_millis(0));
    })
    .join()
    .unwrap();
}

fn resumed(host: &Host, spec: &TaskSpec, name: &str) -> (TaskSpec, RunRequest) {
    let mut spec = spec.clone();
    spec.evidence_dir = host.root.join(name);
    let mut request = host.request(&format!("resume-{name}"));
    request.objective = "Resume: the Host settled the interrupted deploy_step. Submit the answer `correct`.".into();
    (spec, request)
}

fn fence_of(root: &Path) -> std::path::PathBuf {
    std::fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(".fence.json"))
        .expect("fence")
}

#[tokio::test]
async fn crashed_owner_resumes_only_after_host_settlement_without_replay_or_budget_refund() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let provider = Provider::new(vec![
        call("deploy-1", "deploy_step", json!({})),
        write_answer("correct"),
        text("Submitted after resume."),
    ]);
    let tool = hang();
    let mut host = Host::new(&root, &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let spec = host.spec("original");
    crash_owner(host.clone(), spec.clone(), tool.clone());

    // The owner is gone: fence remains, no outcome, the Session is fenced in Core too.
    let fence = fence_of(&root);
    assert!(!root.join("original/outcome.json").exists());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    let pending = agent::interrupted_run(&root.join("session.json"), &root).unwrap().unwrap();
    assert_eq!(pending.run_id, "original");
    assert_eq!(pending.open_tool_calls.len(), 1);
    assert_eq!(pending.open_tool_calls[0].id, "deploy-1");
    assert!(start(host.spec("fresh"), host.request("fresh-run"), host.clone()).is_err());
    let original_fence = std::fs::read(&fence).unwrap();

    let resumer = Arc::new(Resumer { inner: host.clone(), mode: Mutex::new(Mode::Unknown), reviews: Mutex::default() });
    // Hosts without reconciliation support, unknown/uncommitted/mismatched
    // settlements and changed Task terms all keep the fence and send nothing.
    let (s, r) = resumed(&host, &spec, "plain");
    assert!(resume(s, r, host.clone(), TaskOptions::default()).await.is_err());
    for (mode, name) in [(Mode::Unknown, "unknown"), (Mode::Uncommitted, "uncommitted"), (Mode::WrongCalls, "wrong")] {
        *resumer.mode.lock().unwrap() = mode;
        let (s, r) = resumed(&host, &spec, name);
        assert!(resume(s, r, resumer.clone(), TaskOptions::default()).await.is_err(), "{name}");
    }
    *resumer.mode.lock().unwrap() = Mode::Settled;
    let (mut s, r) = resumed(&host, &spec, "bigger");
    s.max_calls += 10;
    assert!(resume(s, r, resumer.clone(), TaskOptions::default()).await.is_err());
    let (mut s, r) = resumed(&host, &spec, "later");
    s.deadline_unix_ms += 60_000;
    assert!(resume(s, r, resumer.clone(), TaskOptions::default()).await.is_err());
    let (s, mut r) = resumed(&host, &spec, "other-model");
    r.endpoint = "http://127.0.0.1:9/v1".into();
    assert!(resume(s, r, resumer.clone(), TaskOptions::default()).await.is_err());
    assert_eq!(std::fs::read(&fence).unwrap(), original_fence);
    assert!(agent::interrupted_run(&root.join("session.json"), &root).unwrap().is_some());
    assert_eq!(provider.requests.lock().unwrap().len(), 1);

    // Settled: the open call is closed with the Host's finding, then continue.
    let (s, r) = resumed(&host, &spec, "settled");
    let outcome = resume(s, r, resumer.clone(), TaskOptions::default()).await.unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.runs, 2);
    // Run 1 had no observation: charged its whole admitted limit (4), never refunded.
    assert_eq!(outcome.observed_calls, 4 + 2);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1, "interrupted effect must not be replayed");
    assert_eq!(std::fs::read(root.join("answer.txt")).unwrap(), b"correct");
    assert_eq!(outcome.fence_presence, FencePresence::Released);
    assert!(!fence.exists());
    let review = resumer.reviews.lock().unwrap().last().cloned().unwrap();
    assert!(review.fenced_outcome.is_none());
    assert_eq!((review.consumed_runs, review.consumed_calls), (1, 4));
    let record: Value = serde_json::from_slice(&std::fs::read(root.join("settled/resume.json")).unwrap()).unwrap();
    assert_eq!(record["consumed_calls"], 4);
    assert_eq!(record["interrupted_run"]["run_id"], "original");
    // The model saw the Host finding in the same Session, as a sourced tool result.
    let requests = provider.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 3);
    let resumed_body = requests[1].to_string();
    assert!(resumed_body.contains("artifact checksum verified"));
    assert!(resumed_body.contains("Call deploy_step once"), "original history kept");
    assert!(agent::interrupted_run(&root.join("session.json"), &root).unwrap().is_none());
    // The interrupted Run ID can never run again.
    let (s, r) = resumed(&host, &spec, "after");
    assert!(resume(s, r, resumer, TaskOptions::default()).await.is_err(), "no fence remains");
    let mut again = host.request("original");
    again.objective = "replay".into();
    assert!(
        start(host.spec("replay"), again, host.clone()).unwrap().wait().await.stop != TaskStop::AwaitingHostAcceptance
    );
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resume_of_a_reconciliation_stop_keeps_cumulative_consumption() {
    // An Unknown review leaves a published outcome and fence, with no native pending Run.
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let provider = Provider::new(vec![
        write_answer("partial"),
        text("Submitted."),
        write_answer("correct"),
        text("Submitted again."),
    ]);
    let host = Arc::new(Host::new(&root, &provider, Fault::Unknown));
    let spec = host.spec("first");
    let outcome = start(spec.clone(), host.request("first-run"), host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::ReconciliationRequired);
    assert_eq!(outcome.observed_calls, 2);
    // The settling Host reviews with the same tool, model and steering owners.
    let mut settled_host = Host::new(&root, &provider, Fault::None);
    settled_host.tool = host.tool.clone();
    settled_host.model_host = host.model_host.clone();
    settled_host.steering = host.steering.clone();
    let resumer =
        Arc::new(Resumer { inner: Arc::new(settled_host), mode: Mutex::new(Mode::Settled), reviews: Mutex::default() });
    let (s, r) = resumed(&host, &spec, "second");
    let outcome = resume(s, r, resumer.clone(), TaskOptions::default()).await.unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, outcome.observed_calls), (2, 4));
    let review = resumer.reviews.lock().unwrap()[0].clone();
    assert!(review.interrupted_run.is_none());
    assert_eq!(review.fenced_outcome.as_ref().unwrap().stop, TaskStop::ReconciliationRequired);
}

fn refusal(error: anyhow::Error) -> ResumeRefusal {
    *error.downcast_ref::<ResumeRefusal>().unwrap_or_else(|| panic!("untyped refusal: {error:#}"))
}

#[tokio::test]
async fn resume_checked_refuses_with_typed_reasons_before_any_callback() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let provider = Provider::new(vec![call("deploy-1", "deploy_step", json!({}))]);
    let tool = hang();
    let mut host = Host::new(&root, &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let spec = host.spec("original");
    crash_owner(host.clone(), spec.clone(), tool.clone());
    let fence = fence_of(&root);
    let original_fence = std::fs::read(&fence).unwrap();
    let resumer = Arc::new(Resumer { inner: host.clone(), mode: Mutex::new(Mode::Settled), reviews: Mutex::default() });
    let checked = |spec: TaskSpec, request: RunRequest, checks: ResumeChecks| {
        resume_checked(spec, request, resumer.clone(), TaskOptions::default(), checks)
    };

    let (s, r) = resumed(&host, &spec, "fence-sha");
    let checks = ResumeChecks { expected_fence_sha256: Some("0".repeat(64)), ..ResumeChecks::default() };
    assert_eq!(refusal(checked(s, r, checks).await.err().unwrap()), ResumeRefusal::FenceMismatch);
    let (s, mut r) = resumed(&host, &spec, "caps");
    r.max_calls += 1;
    let checks = ResumeChecks { refuse_larger_run_caps: true, ..ResumeChecks::default() };
    assert_eq!(refusal(checked(s, r, checks).await.err().unwrap()), ResumeRefusal::RunCapsLarger);
    let (mut s, r) = resumed(&host, &spec, "budget");
    s.max_calls += 1;
    assert_eq!(refusal(checked(s, r, ResumeChecks::default()).await.err().unwrap()), ResumeRefusal::BudgetLarger);
    let (mut s, r) = resumed(&host, &spec, "terms");
    s.criteria_revision = "other".into();
    assert_eq!(refusal(checked(s, r, ResumeChecks::default()).await.err().unwrap()), ResumeRefusal::TaskTermsMismatch);
    let (s, mut r) = resumed(&host, &spec, "authority");
    r.endpoint = "http://127.0.0.1:9/v1".into();
    assert_eq!(refusal(checked(s, r, ResumeChecks::default()).await.err().unwrap()), ResumeRefusal::AuthorityChanged);
    // None of these reached the Host callback or changed the fence.
    assert!(resumer.reviews.lock().unwrap().is_empty());
    for (mode, expected) in [
        (Mode::Unknown, ResumeRefusal::SettlementUnknown),
        (Mode::Uncommitted, ResumeRefusal::ReceiptUncommitted),
        (Mode::WrongCalls, ResumeRefusal::CallMismatch),
    ] {
        *resumer.mode.lock().unwrap() = mode;
        let (s, r) = resumed(&host, &spec, &format!("{expected:?}"));
        assert_eq!(refusal(checked(s, r, ResumeChecks::default()).await.err().unwrap()), expected);
    }
    assert_eq!(std::fs::read(&fence).unwrap(), original_fence);
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert!(ResumeRefusal::CallMismatch.to_string().ends_with("fence kept"));
}

/// Receipt faults of `PropResumer`; `None` is a valid settled receipt.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ReceiptFault {
    None,
    CallbackError,
    Uncommitted,
    Unknown,
    EmptyEvidence,
    WrongCall,
    ExtraCall,
    MissingCall,
}

struct PropResumer {
    inner: Arc<Host>,
    fault: Mutex<ReceiptFault>,
    callbacks: AtomicUsize,
}
#[async_trait]
impl HarnessHost for PropResumer {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.inner.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.inner.prepare_continuation(continuation, cancel).await
    }
    async fn reconcile_resume(&self, review: ResumeReview, _: CancellationToken) -> Result<ResumeReceipt> {
        self.callbacks.fetch_add(1, Ordering::SeqCst);
        let fault = *self.fault.lock().unwrap();
        anyhow::ensure!(fault != ReceiptFault::CallbackError, "fixture reconciliation callback failed");
        let calls = review.interrupted_run.as_ref().map(|run| run.open_tool_calls.clone()).unwrap_or_default();
        let mut tool_calls: Vec<_> = calls
            .iter()
            .map(|call| ToolCallReconciliation {
                call_id: if fault == ReceiptFault::WrongCall { "invented".into() } else { call.id.clone() },
                settlement: ToolCallSettlement::Completed { output: "Host deploy log: step finished.".into() },
            })
            .collect();
        match fault {
            ReceiptFault::ExtraCall => tool_calls.push(tool_calls[0].clone()),
            ReceiptFault::MissingCall => tool_calls.clear(),
            _ => {}
        }
        Ok(ResumeReceipt {
            committed: fault != ReceiptFault::Uncommitted,
            settlement: if fault == ReceiptFault::Unknown { Settlement::Unknown } else { Settlement::Settled },
            tool_calls,
            evidence: if fault == ReceiptFault::EmptyEvidence {
                vec![]
            } else {
                vec!["host-deploy-log:sha256:0f00".into()]
            },
        })
    }
}

/// Every file below `dir`, with its bytes.
fn tree(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in std::fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// One crashed owner with one open `deploy_step` call.
struct Crashed {
    _directory: tempfile::TempDir,
    root: std::path::PathBuf,
    provider: Provider,
    tool: Arc<Hang>,
    host: Arc<Host>,
    spec: TaskSpec,
}

fn crashed(responses: Vec<Value>, max_calls: usize) -> Crashed {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let provider = Provider::new(responses);
    let tool = hang();
    let mut host = Host::new(&root, &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let mut spec = host.spec("original");
    spec.max_calls = max_calls;
    crash_owner(host.clone(), spec.clone(), tool.clone());
    Crashed { _directory: directory, root, provider, tool, host, spec }
}

/// Property 16: a refused resume leaves no side effect (requirements 8.4,
/// 8.6, 14.4, 24.1). One crashed owner serves every case, because each
/// refusal must leave it exactly as it was.
#[test]
fn resume_refusal_leaves_no_side_effects() {
    use proptest::test_runner::{Config, TestRunner};
    let fixture = crashed(vec![call("deploy-1", "deploy_step", json!({}))], 8);
    let resumer = Arc::new(PropResumer {
        inner: fixture.host.clone(),
        fault: Mutex::new(ReceiptFault::None),
        callbacks: AtomicUsize::new(0),
    });
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let session = fixture.root.join("session.json");
    let fence = fence_of(&fixture.root);
    let fence_bytes = std::fs::read(&fence).unwrap();
    let fenced_files = tree(&fixture.root.join("original"));
    let pending = serde_json::to_value(agent::interrupted_run(&session, &fixture.root).unwrap()).unwrap();
    let attempts = AtomicUsize::new(0);
    // 0..10 change one fence term; 10..17 return an invalid receipt.
    const TERMS: usize = 10;
    let faults = [
        ReceiptFault::CallbackError,
        ReceiptFault::Uncommitted,
        ReceiptFault::Unknown,
        ReceiptFault::EmptyEvidence,
        ReceiptFault::WrongCall,
        ReceiptFault::ExtraCall,
        ReceiptFault::MissingCall,
    ];
    let mut runner = TestRunner::new(Config { cases: 32, ..Config::default() });
    runner
        .run(&(0..TERMS + faults.len()), |variant| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let (mut s, mut r) = resumed(&fixture.host, &fixture.spec, &format!("attempt-{attempt}"));
            let mut checks = ResumeChecks::default();
            match variant {
                0 => s.task_id = "other-task".into(),
                1 => s.criteria_revision = "other-criteria".into(),
                2 => s.authorization_revision = "other-authorization".into(),
                3 => s.deadline_unix_ms += 1_000,
                4 => s.max_runs += 1,
                5 => s.max_calls += 1,
                6 => r.state_path = fixture.root.join("other-session.json"),
                7 => r.endpoint = "http://127.0.0.1:9/v1".into(),
                8 => r.model = "other-model".into(),
                9 => {
                    r.max_calls += 1;
                    checks.refuse_larger_run_caps = true;
                }
                _ => {}
            }
            *resumer.fault.lock().unwrap() = if variant < TERMS { ReceiptFault::None } else { faults[variant - TERMS] };
            let before = resumer.callbacks.load(Ordering::SeqCst);
            let result = runtime.block_on(resume_checked(s, r, resumer.clone(), TaskOptions::default(), checks));
            let error = result.err().unwrap_or_else(|| panic!("variant {variant} was not refused"));
            proptest::prop_assert!(error.downcast_ref::<ResumeRefusal>().is_some(), "untyped: {error:#}");
            let called = resumer.callbacks.load(Ordering::SeqCst) - before;
            proptest::prop_assert_eq!(called, usize::from(variant >= TERMS), "variant {}", variant);
            proptest::prop_assert_eq!(&std::fs::read(&fence).unwrap(), &fence_bytes);
            proptest::prop_assert!(tree(&fixture.root.join("original")) == fenced_files, "fenced receipts changed");
            let now = serde_json::to_value(agent::interrupted_run(&session, &fixture.root).unwrap()).unwrap();
            proptest::prop_assert_eq!(&now, &pending);
            proptest::prop_assert_eq!(fixture.provider.requests.lock().unwrap().len(), 1);
            proptest::prop_assert_eq!(fixture.tool.executions.load(Ordering::SeqCst), 1);
            Ok(())
        })
        .unwrap();
}

/// `resume` clamps a larger per-Run cap to the original admission, where
/// `resume_checked` would refuse it (requirement 8.5).
#[tokio::test]
async fn resume_clamps_larger_run_caps_to_the_original_admission() {
    let fixture =
        crashed(vec![call("deploy-1", "deploy_step", json!({})), write_answer("correct"), text("Submitted.")], 8);
    let resumer =
        Arc::new(Resumer { inner: fixture.host.clone(), mode: Mutex::new(Mode::Settled), reviews: Mutex::default() });
    let (s, mut r) = resumed(&fixture.host, &fixture.spec, "clamped");
    r.max_calls = 7;
    let outcome = resume(s, r, resumer, TaskOptions::default()).await.unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    let admission: Value =
        serde_json::from_slice(&std::fs::read(fixture.root.join("clamped/run-2-admission.json")).unwrap()).unwrap();
    // The original first Run was admitted with 4 calls; the request asked for 7.
    assert_eq!(admission["remaining_calls"], 4, "{admission}");
    assert_eq!(fixture.tool.executions.load(Ordering::SeqCst), 1);
}

/// Every remaining refusal kind is typed, and pre-callback refusals never
/// reach the Host (requirement 8.7).
#[tokio::test]
async fn every_refusal_kind_is_typed() {
    // NoFence: a Session that has never been fenced.
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![]);
    let plain = Arc::new(Host::new(&directory.path().canonicalize().unwrap(), &provider, Fault::None));
    let error = resume(plain.spec("nothing"), plain.request("resume-nothing"), plain.clone(), TaskOptions::default())
        .await
        .err()
        .unwrap();
    assert_eq!(refusal(error), ResumeRefusal::NoFence);

    let fixture = crashed(vec![call("deploy-1", "deploy_step", json!({}))], 8);
    let resumer = Arc::new(PropResumer {
        inner: fixture.host.clone(),
        fault: Mutex::new(ReceiptFault::None),
        callbacks: AtomicUsize::new(0),
    });
    // DeadlineElapsed and FenceMismatch are decided before any callback.
    let (mut s, r) = resumed(&fixture.host, &fixture.spec, "late");
    s.deadline_unix_ms = now_ms() - 1;
    let error = resume(s, r, resumer.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::DeadlineElapsed);
    let (s, r) = resumed(&fixture.host, &fixture.spec, "sha");
    let checks = ResumeChecks { expected_fence_sha256: Some("f".repeat(64)), ..ResumeChecks::default() };
    let error = resume_checked(s, r, resumer.clone(), TaskOptions::default(), checks).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::FenceMismatch);
    assert_eq!(resumer.callbacks.load(Ordering::SeqCst), 0);
    // A Host without reconciliation support, and an empty evidence list.
    let (s, r) = resumed(&fixture.host, &fixture.spec, "unsupported");
    let error = resume(s, r, fixture.host.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::HostCallbackFailed);
    *resumer.fault.lock().unwrap() = ReceiptFault::EmptyEvidence;
    let (s, r) = resumed(&fixture.host, &fixture.spec, "empty");
    let error = resume(s, r, resumer.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::EvidenceEmpty);
    *resumer.fault.lock().unwrap() = ReceiptFault::CallbackError;
    let (s, r) = resumed(&fixture.host, &fixture.spec, "callback");
    let error = resume(s, r, resumer.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::HostCallbackFailed);

    // BudgetExhausted: the unobserved Run is charged its whole 4-call limit.
    let exhausted = crashed(vec![call("deploy-1", "deploy_step", json!({}))], 4);
    let (s, r) = resumed(&exhausted.host, &exhausted.spec, "spent");
    let error = resume(s, r, resumer.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::BudgetExhausted);
    // FenceUnreadable: the fence is no longer JSON.
    std::fs::write(fence_of(&exhausted.root), b"{not json").unwrap();
    let (s, r) = resumed(&exhausted.host, &exhausted.spec, "unreadable");
    let error = resume(s, r, resumer.clone(), TaskOptions::default()).await.err().unwrap();
    assert_eq!(refusal(error), ResumeRefusal::FenceUnreadable);
    assert_eq!(resumer.callbacks.load(Ordering::SeqCst), 2, "only the two receipt cases reached the Host");
}
