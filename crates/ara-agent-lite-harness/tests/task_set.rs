mod common;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_harness::{
    core::{
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        RunRequest,
    },
    task_set::{Completion, GuidanceStatus, Submission, TaskSet, MAX_GUIDANCE_BYTES, MAX_MESSAGE_ID_BYTES},
    Candidate, Continuation, FencePresence, HarnessHost, ReviewReceipt, TaskOptions, TaskStop,
};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const WAIT: Duration = Duration::from_secs(8);

/// The provider has already issued a real tool call; only this test decides when
/// its callback settles. Both start and settlement leave actual file artifacts.
struct BarrierTool {
    schema: Tool,
    root: PathBuf,
    entered: Notify,
    release: Notify,
    executions: AtomicUsize,
}

impl BarrierTool {
    fn new(root: PathBuf) -> Self {
        Self {
            schema: Tool {
                name: "task_barrier".into(),
                description: "Wait for the test's explicit settlement signal.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            root,
            entered: Notify::new(),
            release: Notify::new(),
            executions: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AgentTool for BarrierTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "task_barrier"
    }

    async fn execute(
        &self,
        _: &str,
        _: Value,
        cancellation: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        std::fs::write(self.root.join("barrier-started.txt"), b"started")
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        self.entered.notify_one();
        tokio::select! {
            _ = cancellation.cancelled() => return Err(AgentError::Abort),
            result = tokio::time::timeout(WAIT, self.release.notified()) => {
                result.map_err(|_| AgentError::Tool("test did not release the barrier".into()))?;
            }
        }
        std::fs::write(self.root.join("barrier-settled.txt"), b"settled")
            .map_err(|error| AgentError::Tool(error.to_string()))?;
        Ok(AgentToolResult::text("Original barrier callback settled."))
    }
}

struct ReviewBarrier {
    inner: Host,
    entered: Notify,
    release: Notify,
}

#[async_trait]
impl HarnessHost for ReviewBarrier {
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        self.entered.notify_one();
        tokio::time::timeout(WAIT, self.release.notified()).await?;
        self.inner.verify(candidate, cancellation).await
    }

    async fn prepare_continuation(&self, next: Continuation, cancellation: CancellationToken) -> Result<RunRequest> {
        self.inner.prepare_continuation(next, cancellation).await
    }
}

async fn completion(tasks: &mut TaskSet) -> Completion {
    tokio::time::timeout(WAIT, tasks.next_completed()).await.unwrap().expect("missing Task completion")
}

async fn flag_is_set(flag: &AtomicBool) {
    tokio::time::timeout(WAIT, async {
        while !flag.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn blocked_task_leaves_other_tasks_and_live_observation_responsive() {
    let slow_dir = tempfile::tempdir().unwrap();
    let fast_dir = tempfile::tempdir().unwrap();
    let slow_provider = Provider::new(vec![
        call("slow-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("Slow complete."),
    ]);
    let fast_provider = Provider::new(vec![write_answer("correct"), text("Fast complete.")]);
    let barrier = Arc::new(BarrierTool::new(slow_dir.path().into()));
    let mut slow_host = Host::new(slow_dir.path(), &slow_provider, Fault::None);
    slow_host.read_tool = Some(barrier.clone());
    let slow_host = Arc::new(slow_host);
    let fast_host = Arc::new(Host::new(fast_dir.path(), &fast_provider, Fault::None));
    let mut tasks = TaskSet::new(2, 2).unwrap();
    let slow = tasks
        .start(slow_host.spec("slow"), slow_host.request("slow-run"), slow_host.clone(), TaskOptions::default())
        .unwrap();
    let mut live = slow.subscribe();
    tokio::time::timeout(WAIT, async {
        loop {
            let event = live.recv().await.unwrap();
            assert_eq!(event.task_id, "slow");
            if event.event["type"] == "tool_start" {
                break;
            }
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(WAIT, barrier.entered.notified()).await.unwrap();
    assert!(slow.try_outcome().is_none());
    assert!(slow_dir.path().join("barrier-started.txt").is_file());
    tasks
        .start(fast_host.spec("fast"), fast_host.request("fast-run"), fast_host.clone(), TaskOptions::default())
        .unwrap();
    let fast = completion(&mut tasks).await;
    assert_eq!(fast.outcome.task_id, "fast");
    assert_eq!(fast.outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert!(!fast.outcome.task_accepted);
    assert!(slow.try_outcome().is_none(), "fast Task waited for the unrelated blocked callback");
    assert_eq!(std::fs::read(fast_dir.path().join("answer.txt")).unwrap(), b"correct");
    assert!(!slow_dir.path().join("answer.txt").exists());
    barrier.release.notify_one();
    let slow = completion(&mut tasks).await;
    assert_eq!(slow.outcome.task_id, "slow");
    assert_eq!(slow.outcome.observed_calls, 3);
    assert_eq!(slow.outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert_eq!(std::fs::read(slow_dir.path().join("answer.txt")).unwrap(), b"correct");
    assert!(tasks.next_completed().await.is_none());
}

#[tokio::test]
async fn directed_guidance_is_idempotent_and_reaches_only_the_original_task() {
    let left_dir = tempfile::tempdir().unwrap();
    let right_dir = tempfile::tempdir().unwrap();
    let left_provider = Provider::new(vec![
        call("left-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("Left complete."),
    ]);
    let right_provider = Provider::new(vec![
        call("right-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("Right complete."),
    ]);
    let left_barrier = Arc::new(BarrierTool::new(left_dir.path().into()));
    let right_barrier = Arc::new(BarrierTool::new(right_dir.path().into()));
    let mut left_host = Host::new(left_dir.path(), &left_provider, Fault::None);
    left_host.read_tool = Some(left_barrier.clone());
    let left_host = Arc::new(left_host);
    let mut right_host = Host::new(right_dir.path(), &right_provider, Fault::None);
    right_host.read_tool = Some(right_barrier.clone());
    let right_host = Arc::new(right_host);
    let mut tasks = TaskSet::new(2, 2).unwrap();
    tasks
        .start(left_host.spec("left"), left_host.request("left-run"), left_host.clone(), TaskOptions::default())
        .unwrap();
    tasks
        .start(right_host.spec("right"), right_host.request("right-run"), right_host.clone(), TaskOptions::default())
        .unwrap();
    tokio::time::timeout(WAIT, left_barrier.entered.notified()).await.unwrap();
    tokio::time::timeout(WAIT, right_barrier.entered.notified()).await.unwrap();
    let marker = "followup-left-only-7f6026";
    assert_eq!(tasks.guide("left", "message-1", marker).unwrap(), Submission::Queued);
    assert_eq!(tasks.guide("left", "message-1", marker).unwrap(), Submission::Duplicate);
    assert!(tasks.guide("left", "message-1", "changed content").is_err());
    assert_eq!(left_host.steering.lock().unwrap().len(), 1);
    assert!(right_host.steering.lock().unwrap().is_empty());
    left_barrier.release.notify_one();
    right_barrier.release.notify_one();
    let first = completion(&mut tasks).await;
    let second = completion(&mut tasks).await;
    let left = if first.outcome.task_id == "left" { &first } else { &second };
    let right = if first.outcome.task_id == "right" { &first } else { &second };
    assert_eq!(left.guidance.len(), 1);
    assert_eq!(left.guidance[0].message_id, "message-1");
    assert_eq!(left.guidance[0].status, GuidanceStatus::AppliedToContext);
    assert!(right.guidance.is_empty());
    for result in [&first, &second] {
        assert_eq!(result.outcome.runs, 1);
        assert_eq!(result.outcome.observed_calls, 3);
        assert_eq!(result.outcome.stop, TaskStop::AwaitingHostAcceptance);
        assert!(!result.outcome.task_accepted);
    }
    let left_requests = left_provider.requests.lock().unwrap();
    assert_eq!(left_requests.len(), 3);
    // The working source directory also carries an excerpt of this reference.
    // Count native user inputs, not textual occurrences across both projections.
    assert_eq!(
        left_requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| { message["role"] == "user" && message["content"].as_str() == Some(marker) })
            .count(),
        1
    );
    assert!(!left_requests[0].to_string().contains(marker));
    assert!(!right_provider.requests.lock().unwrap().iter().any(|request| request.to_string().contains(marker)));
    let left_context: ara_agent_lite_harness::core::context_memory::Store =
        serde_json::from_slice(&std::fs::read(left_dir.path().join("session.context.json")).unwrap()).unwrap();
    let sources: Vec<_> = left_context
        .references
        .iter()
        .filter(|reference| reference.kind == "steering" && reference.text == marker)
        .collect();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].run, "left-run");
    assert_eq!(sources[0].status, "active");
    let right_context: ara_agent_lite_harness::core::context_memory::Store =
        serde_json::from_slice(&std::fs::read(right_dir.path().join("session.context.json")).unwrap()).unwrap();
    assert!(!right_context.references.iter().any(|reference| reference.text.contains(marker)));
    let outcomes = left_host.steering.lock().unwrap().outcomes_snapshot();
    let receipts: Vec<_> = outcomes
        .iter()
        .filter(|receipt| receipt["task_id"] == "left" && receipt["message_id"] == "message-1")
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["status"], "applied");
    let candidate = left.outcome.last_candidate.as_ref().unwrap();
    assert_eq!(candidate.basis.session_path, left_dir.path().join("session.json").canonicalize().unwrap());
    assert_eq!(left_barrier.executions.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn review_time_guidance_is_unapplied_and_cancelled_wait_loses_no_completion() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let host = Arc::new(ReviewBarrier {
        inner: Host::new(directory.path(), &provider, Fault::None),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let mut tasks = TaskSet::new(1, 2).unwrap();
    let handle = tasks
        .start(host.inner.spec("review"), host.inner.request("review-run"), host.clone(), TaskOptions::default())
        .unwrap();
    tokio::time::timeout(WAIT, host.entered.notified()).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(25), tasks.next_completed()).await.is_err());
    assert_eq!(
        tasks.guide("review", "late-message", "A message after the Core Run ended.").unwrap(),
        Submission::Queued
    );
    assert_eq!(host.inner.steering.lock().unwrap().len(), 1);
    assert!(handle.try_outcome().is_none());
    host.release.notify_one();
    let result = completion(&mut tasks).await;
    assert_eq!(result.outcome.task_id, "review");
    assert_eq!(result.outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert_eq!(result.outcome.observed_calls, 2);
    assert_eq!(result.guidance.len(), 1);
    assert_eq!(result.guidance[0].status, GuidanceStatus::Unapplied);
    assert!(host.inner.steering.lock().unwrap().is_empty());
    assert!(Arc::ptr_eq(&result.outcome, &handle.try_outcome().unwrap()));
    assert_eq!(
        tasks.guide("review", "late-message", "A message after the Core Run ended.").unwrap(),
        Submission::Duplicate
    );
    assert!(tasks.guide("review", "new-message", "Must not wake an ended Task.").is_err());
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
    assert!(tasks.next_completed().await.is_none());
}

#[tokio::test]
async fn cancellation_waits_for_original_tool_settlement_and_preserves_unknown_effects() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![call("inflight", "delayed_cancel", json!({})), write_answer("must-not-run")]);
    let tool = Arc::new(DelayedCancelTool::new(directory.path()));
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(tool.clone());
    let host = Arc::new(host);
    let mut tasks = TaskSet::new(1, 2).unwrap();
    let handle =
        tasks.start(host.spec("cancel"), host.request("cancel-run"), host.clone(), TaskOptions::default()).unwrap();
    flag_is_set(&tool.started).await;
    tasks.cancel("cancel").unwrap();
    tasks.cancel_all(); // Repeated cancellation is idempotent; never a second execution.
    assert!(tasks.guide("cancel", "after-cancel", "Do not admit.").is_err());
    flag_is_set(&tool.cancellation_seen).await;
    assert!(tokio::time::timeout(Duration::from_millis(25), tasks.next_completed()).await.is_err());
    assert!(handle.try_outcome().is_none());
    assert!(!tool.joined.load(Ordering::SeqCst));
    assert!(!directory.path().join("inflight-joined.json").exists());
    tool.release_settlement();
    let result = completion(&mut tasks).await;
    assert_eq!(result.outcome.stop, TaskStop::Cancelled);
    assert_eq!(result.outcome.fence_presence, FencePresence::Present);
    assert!(result.outcome.reconciliation_fence.as_ref().unwrap().is_file());
    assert!(!result.outcome.task_accepted);
    assert!(result.outcome.last_candidate.is_none());
    assert!(tool.joined.load(Ordering::SeqCst));
    assert!(directory.path().join("inflight-joined.json").is_file());
    let settled: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("inflight-joined.json")).unwrap()).unwrap();
    assert_eq!(settled["external_effects"], "unknown");
    assert_eq!(tool.executions.load(Ordering::SeqCst), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    assert!(!directory.path().join("answer.txt").exists());
    assert!(tasks.next_completed().await.is_none());
}

#[tokio::test]
async fn task_identity_and_capacity_refusals_do_not_start_hidden_work() {
    let first_dir = tempfile::tempdir().unwrap();
    let second_dir = tempfile::tempdir().unwrap();
    let third_dir = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("first-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("First complete."),
    ]);
    let second_provider = Provider::new(vec![write_answer("correct"), text("Second complete.")]);
    let third_provider = Provider::new(vec![]);
    let barrier = Arc::new(BarrierTool::new(first_dir.path().into()));
    let mut first_host = Host::new(first_dir.path(), &provider, Fault::None);
    first_host.read_tool = Some(barrier.clone());
    let first_host = Arc::new(first_host);
    let second_host = Arc::new(Host::new(second_dir.path(), &second_provider, Fault::None));
    let third_host = Arc::new(Host::new(third_dir.path(), &third_provider, Fault::None));
    let mut tasks = TaskSet::new(2, 2).unwrap();
    tasks
        .start(first_host.spec("first"), first_host.request("first-run"), first_host.clone(), TaskOptions::default())
        .unwrap();
    tokio::time::timeout(WAIT, barrier.entered.notified()).await.unwrap();
    assert!(tasks.guide("missing", "message", "No target.").is_err());
    assert!(tasks.cancel("missing").is_err());
    assert!(tasks
        .start(
            second_host.spec("first"),
            second_host.request("duplicate-run"),
            second_host.clone(),
            TaskOptions::default()
        )
        .is_err());
    let mut shared = second_host.request("shared-buffer-run");
    shared.steering = first_host.steering.clone();
    assert!(tasks.start(second_host.spec("shared"), shared, second_host.clone(), TaskOptions::default()).is_err());
    assert!(second_provider.requests.lock().unwrap().is_empty());
    assert!(!second_dir.path().join("first").exists());
    assert!(!second_dir.path().join("shared").exists());
    tasks
        .start(
            second_host.spec("second"),
            second_host.request("second-run"),
            second_host.clone(),
            TaskOptions::default(),
        )
        .unwrap();
    let second = completion(&mut tasks).await;
    assert_eq!(second.outcome.task_id, "second");
    // Completed identities retain their slot: capacity bounds the whole batch.
    assert!(tasks
        .start(third_host.spec("third"), third_host.request("third-run"), third_host.clone(), TaskOptions::default())
        .is_err());
    assert!(!third_dir.path().join("third").exists());
    assert!(third_provider.requests.lock().unwrap().is_empty());
    barrier.release.notify_one();
    assert_eq!(completion(&mut tasks).await.outcome.task_id, "first");
}

#[tokio::test]
async fn guidance_limits_are_checked_before_queueing_and_do_not_spend_calls() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("limits-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("Complete."),
    ]);
    let barrier = Arc::new(BarrierTool::new(directory.path().into()));
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(barrier.clone());
    let host = Arc::new(host);
    let mut tasks = TaskSet::new(1, 1).unwrap();
    tasks.start(host.spec("limits"), host.request("limits-run"), host.clone(), TaskOptions::default()).unwrap();
    tokio::time::timeout(WAIT, barrier.entered.notified()).await.unwrap();
    assert!(tasks.guide("limits", "", "input").is_err());
    assert!(tasks.guide("limits", &"i".repeat(MAX_MESSAGE_ID_BYTES + 1), "input").is_err());
    assert!(tasks.guide("limits", "empty", " \t").is_err());
    assert!(tasks.guide("limits", "oversize", &"x".repeat(MAX_GUIDANCE_BYTES + 1)).is_err());
    assert!(tasks.guide("limits", "unicode", &"界".repeat(MAX_GUIDANCE_BYTES / 3 + 1)).is_err());
    assert!(host.steering.lock().unwrap().is_empty());
    let id = "i".repeat(MAX_MESSAGE_ID_BYTES);
    let input = "x".repeat(MAX_GUIDANCE_BYTES);
    assert_eq!(tasks.guide("limits", &id, &input).unwrap(), Submission::Queued);
    assert_eq!(tasks.guide("limits", &id, &input).unwrap(), Submission::Duplicate);
    assert!(tasks.guide("limits", "second", "No capacity.").is_err());
    assert_eq!(host.steering.lock().unwrap().len(), 1);
    assert_eq!(provider.requests.lock().unwrap().len(), 1);
    barrier.release.notify_one();
    let result = completion(&mut tasks).await;
    assert_eq!(result.guidance.len(), 1);
    assert_eq!(result.guidance[0].status, GuidanceStatus::AppliedToContext);
    assert_eq!(result.outcome.observed_calls, 3);
    assert_eq!(provider.requests.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn failed_harness_admission_does_not_consume_task_identity_or_capacity() {
    assert!(TaskSet::new(0, 1).is_err());
    assert!(TaskSet::new(1, 0).is_err());
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let mut tasks = TaskSet::new(1, 1).unwrap();
    let mut invalid = host.spec("retry-admission");
    invalid.max_calls = 0;
    assert!(tasks.start(invalid, host.request("invalid-run"), host.clone(), TaskOptions::default()).is_err());
    assert!(!directory.path().join("retry-admission").exists());
    assert!(provider.requests.lock().unwrap().is_empty());
    assert!(tasks.next_completed().await.is_none());
    tasks.start(host.spec("retry-admission"), host.request("valid-run"), host.clone(), TaskOptions::default()).unwrap();
    let result = completion(&mut tasks).await;
    assert_eq!(result.outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert_eq!(provider.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn dropping_task_set_does_not_cancel_the_original_worker() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![
        call("drop-barrier", "task_barrier", json!({})),
        write_answer("correct"),
        text("Complete."),
    ]);
    let barrier = Arc::new(BarrierTool::new(directory.path().into()));
    let mut host = Host::new(directory.path(), &provider, Fault::None);
    host.read_tool = Some(barrier.clone());
    let host = Arc::new(host);
    let mut tasks = TaskSet::new(1, 1).unwrap();
    let handle =
        tasks.start(host.spec("drop"), host.request("drop-run"), host.clone(), TaskOptions::default()).unwrap();
    tokio::time::timeout(WAIT, barrier.entered.notified()).await.unwrap();
    drop(tasks);
    barrier.release.notify_one();
    let outcome = tokio::time::timeout(WAIT, handle.wait()).await.unwrap();
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance);
    assert_eq!(std::fs::read(directory.path().join("answer.txt")).unwrap(), b"correct");
    assert_eq!(barrier.executions.load(Ordering::SeqCst), 1);
}
