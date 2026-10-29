//! Controlled tests of Length stop-snapshot review (requirement 16) over the
//! real Core `execute` and a loopback provider.
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::Result;
use ara_agent_lite_harness::{core::RunRequest, *};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

fn length(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"length"})
}

/// Wraps the common Host and adds stop-snapshot review over the same file.
struct StopHost {
    base: Host,
    supports: bool,
    stop_reviews: AtomicUsize,
    snapshots: std::sync::Mutex<Vec<StopSnapshot>>,
}

#[async_trait]
impl HarnessHost for StopHost {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.base.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.base.prepare_continuation(continuation, cancel).await
    }
    async fn verify_stop_snapshot(&self, snapshot: StopSnapshot, _: CancellationToken) -> Result<ReviewReceipt> {
        anyhow::ensure!(self.supports, "fixture Host does not review stop snapshots");
        self.stop_reviews.fetch_add(1, Ordering::SeqCst);
        self.snapshots.lock().unwrap().push(snapshot.clone());
        let passed = std::fs::read(self.base.root.join("answer.txt")).unwrap_or_default() == b"correct";
        Ok(ReviewReceipt {
            basis: snapshot.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict: if passed {
                Verdict::Verified
            } else {
                Verdict::NeedsChanges { feedback: "answer.txt is wrong; write the correct value".into() }
            },
            evidence: vec!["fixture-stop-review".into()],
        })
    }
    async fn prepare_stop_continuation(
        &self,
        continuation: StopContinuation,
        _: CancellationToken,
    ) -> Result<RunRequest> {
        let mut request = self.base.request(&format!("stop-repair-{}", continuation.next_run_number));
        request.objective = "Repair the answer after the truncated response.".into();
        Ok(request)
    }
}

fn host(root: &std::path::Path, provider: &Provider, supports: bool) -> Arc<StopHost> {
    Arc::new(StopHost {
        base: Host::new(root, provider, Fault::None),
        supports,
        stop_reviews: AtomicUsize::new(0),
        snapshots: Default::default(),
    })
}

fn review_length() -> TaskOptions {
    TaskOptions { length_stop_review: LengthStopReviewPolicy::ReviewClosedLength, ..TaskOptions::default() }
}

async fn wait(handle: TaskHandle) -> Arc<TaskOutcome> {
    tokio::time::timeout(std::time::Duration::from_secs(30), handle.wait()).await.unwrap()
}

#[tokio::test]
async fn closed_length_stop_is_reviewed_and_verified_without_a_new_run() {
    let root = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), length("partial report")]);
    let host = host(root.path(), &provider, true);
    let outcome = wait(
        start_with_options(host.base.spec("length"), host.base.request("original"), host.clone(), review_length())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, host.stop_reviews.load(Ordering::SeqCst)), (1, 1));
    assert!(!outcome.task_accepted);
    let snapshot = host.snapshots.lock().unwrap()[0].clone();
    assert!(matches!(snapshot.evidence, core::termination::StopEvidence::Length { .. }));
    let truncated = snapshot.truncated.expect("truncated message reference");
    assert!(truncated.raw_index >= snapshot.basis.raw_start && truncated.raw_index < snapshot.basis.raw_end);
    assert_eq!(snapshot.diagnostic_output, "partial report");
    let evidence = root.path().join("length");
    assert!(stop_snapshot_path(&evidence, 1).is_file());
    assert!(stop_review_index_path(&evidence, 1).is_file());
}

#[tokio::test]
async fn length_review_needs_changes_continues_in_the_same_session() {
    let root = tempfile::tempdir().unwrap();
    let provider =
        Provider::new(vec![write_answer("wrong"), length("cut"), write_answer("correct"), text("submitted")]);
    let host = host(root.path(), &provider, true);
    let outcome = wait(
        start_with_options(host.base.spec("repair"), host.base.request("original"), host.clone(), review_length())
            .unwrap(),
    )
    .await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!(outcome.runs, 2);
    assert_eq!(host.stop_reviews.load(Ordering::SeqCst), 1);
    assert_eq!(host.base.reviews.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read(root.path().join("answer.txt")).unwrap(), b"correct");
}

#[tokio::test]
async fn default_policy_and_unsupported_hosts_keep_reconciliation_required() {
    for (options, supports, snapshot) in [(TaskOptions::default(), true, false), (review_length(), false, true)] {
        let root = tempfile::tempdir().unwrap();
        let provider = Provider::new(vec![write_answer("correct"), length("cut")]);
        let host = host(root.path(), &provider, supports);
        let outcome = wait(
            start_with_options(host.base.spec("length"), host.base.request("original"), host.clone(), options).unwrap(),
        )
        .await;
        assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
        assert_eq!(outcome.runs, 1);
        assert_eq!(stop_snapshot_path(&root.path().join("length"), 1).is_file(), snapshot);
        let rows = events(&root.path().join("length/run-1-events.jsonl"));
        let end = rows.iter().find(|row| row["event"]["type"] == "agent_end").unwrap();
        assert_eq!(end["event"].get("stop_evidence").is_some(), snapshot, "evidence only when opted in");
    }
}

/// Property 22: stop-snapshot review decisions (requirements 16.2, 16.4-16.9,
/// 18.6, 18.10) for Length and NoProgress evidence, the length policy,
/// structured unknown effects, every Host callback result and the Run budget.
mod property_22 {
    use super::*;
    use ara_agent_lite_harness::core::{
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        stuck_detector::{StuckPolicy, StuckRule},
        termination::StopEvidence,
    };
    use proptest::prelude::*;
    use std::sync::Mutex;

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum Review {
        Fails,
        Verified,
        NeedsChanges,
        EmptyFeedback,
        Uncommitted,
        SettlementUnknown,
        VerdictUnknown,
        BadBasis,
    }

    const REVIEWS: [Review; 8] = [
        Review::Fails,
        Review::Verified,
        Review::NeedsChanges,
        Review::EmptyFeedback,
        Review::Uncommitted,
        Review::SettlementUnknown,
        Review::VerdictUnknown,
        Review::BadBasis,
    ];

    /// A repeatable observation; `unknown` adds only an `unknown_effect_record`
    /// key, so the Run still reaches its stop and the journal scan decides.
    struct Observe {
        schema: Tool,
        unknown: bool,
    }

    #[async_trait]
    impl AgentTool for Observe {
        fn schema(&self) -> &Tool {
            &self.schema
        }
        fn label(&self) -> &str {
            "fixture_observe"
        }
        async fn execute(
            &self,
            _: &str,
            _: Value,
            _: CancellationToken,
            _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
        ) -> std::result::Result<AgentToolResult, AgentError> {
            let mut result = AgentToolResult::text("same observation");
            if self.unknown {
                result.details = json!({"unknown_effect_record":{"state":"unconfirmed"}});
            }
            Ok(result)
        }
    }

    struct DecisionHost {
        base: Host,
        review: Review,
        no_progress: bool,
        stop_reviews: AtomicUsize,
        snapshots: Mutex<Vec<StopSnapshot>>,
        continuations: Mutex<Vec<(String, usize)>>,
    }

    impl DecisionHost {
        fn request(&self, id: &str) -> RunRequest {
            let mut request = self.base.request(id);
            request.max_calls = 6;
            if self.no_progress {
                request.policies.stuck = Some(StuckPolicy { repeated_call: Some(2), ..StuckPolicy::default() });
            }
            request
        }
    }

    #[async_trait]
    impl HarnessHost for DecisionHost {
        async fn verify(&self, candidate: Candidate, _: CancellationToken) -> Result<ReviewReceipt> {
            Ok(ReviewReceipt {
                basis: candidate.basis,
                committed: true,
                settlement: Settlement::Settled,
                verdict: Verdict::Verified,
                evidence: vec!["fixture-ordinary-review".into()],
            })
        }
        async fn prepare_continuation(&self, _: Continuation, _: CancellationToken) -> Result<RunRequest> {
            anyhow::bail!("ordinary continuation is not part of this property")
        }
        async fn verify_stop_snapshot(&self, snapshot: StopSnapshot, _: CancellationToken) -> Result<ReviewReceipt> {
            self.stop_reviews.fetch_add(1, Ordering::SeqCst);
            self.snapshots.lock().unwrap().push(snapshot.clone());
            anyhow::ensure!(self.review != Review::Fails, "fixture stop review failed");
            let mut basis = snapshot.basis;
            if self.review == Review::BadBasis {
                basis.criteria_revision = "other".into();
            }
            Ok(ReviewReceipt {
                basis,
                committed: self.review != Review::Uncommitted,
                settlement: if self.review == Review::SettlementUnknown {
                    Settlement::Unknown
                } else {
                    Settlement::Settled
                },
                verdict: match self.review {
                    Review::Verified | Review::Uncommitted | Review::SettlementUnknown | Review::BadBasis => {
                        Verdict::Verified
                    }
                    Review::VerdictUnknown => Verdict::Unknown,
                    Review::EmptyFeedback => Verdict::NeedsChanges { feedback: " ".into() },
                    _ => Verdict::NeedsChanges { feedback: "continue from the reviewed stop".into() },
                },
                evidence: vec!["fixture-stop-review".into()],
            })
        }
        async fn prepare_stop_continuation(
            &self,
            continuation: StopContinuation,
            _: CancellationToken,
        ) -> Result<RunRequest> {
            let id = format!("stop-repair-{}", continuation.next_run_number);
            self.continuations
                .lock()
                .unwrap()
                .push((continuation.previous.basis.run_id.clone(), continuation.remaining_calls));
            let mut request = self.request(&id);
            request.objective = "Continue after the reviewed stop.".into();
            Ok(request)
        }
    }

    #[derive(Debug)]
    struct Case {
        no_progress: bool,
        policy_on: bool,
        unknown: bool,
        review: Review,
        max_runs: usize,
    }

    fn run(case: &Case) -> (Arc<TaskOutcome>, Arc<DecisionHost>, std::path::PathBuf, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let observe = |id: &str| call(id, "fixture_observe", json!({}));
        let script = if case.no_progress {
            // repeated_call=2: rounds 2 and 4 reach the threshold; the second is NoProgress.
            vec![observe("obs-1"), observe("obs-2"), observe("obs-3"), observe("obs-4"), text("Continued.")]
        } else {
            vec![observe("obs-1"), length("cut at the output limit"), text("Continued.")]
        };
        let provider = Provider::new(script);
        let mut base = Host::new(&root, &provider, Fault::None);
        base.read_tool = Some(Arc::new(Observe {
            schema: Tool {
                name: "fixture_observe".into(),
                description: "Return one fixture observation.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            unknown: case.unknown,
        }));
        let host = Arc::new(DecisionHost {
            base,
            review: case.review,
            no_progress: case.no_progress,
            stop_reviews: AtomicUsize::new(0),
            snapshots: Mutex::default(),
            continuations: Mutex::default(),
        });
        let mut spec = host.base.spec("decision");
        spec.max_runs = case.max_runs;
        spec.max_calls = 16;
        let options = if case.policy_on { review_length() } else { TaskOptions::default() };
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let outcome = runtime.block_on(async {
            wait(start_with_options(spec, host.request("original"), host.clone(), options).unwrap()).await
        });
        drop(provider);
        (outcome, host, root.join("decision"), directory)
    }

    fn case() -> impl Strategy<Value = Case> {
        (any::<bool>(), any::<bool>(), any::<bool>(), 0..REVIEWS.len(), prop_oneof![Just(1usize), Just(3usize)])
            .prop_map(|(no_progress, policy_on, unknown, review, max_runs)| Case {
                no_progress,
                policy_on,
                unknown,
                review: REVIEWS[review],
                max_runs,
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]

        #[test]
        fn stop_review_follows_the_decision_table(case in case()) {
            let (outcome, host, evidence, _directory) = run(&case);
            let reviews = host.stop_reviews.load(Ordering::SeqCst);
            let continuations = host.continuations.lock().unwrap().clone();
            prop_assert!(!outcome.task_accepted, "{:?}", outcome);
            let snapshot = stop_snapshot_path(&evidence, 1).is_file();
            let receipt = evidence.join("run-1-stop-review.json").is_file();
            let index = stop_review_index_path(&evidence, 1).is_file();
            let reviewed = case.no_progress || case.policy_on;
            if !reviewed || case.unknown {
                // Policy off keeps the pre-existing Length conclusion; unknown
                // effects are never sent to review.
                prop_assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{:?}", outcome);
                prop_assert_eq!((reviews, snapshot, outcome.runs), (0, false, 1));
                if reviewed {
                    prop_assert!(!outcome.unknown_effects.is_empty(), "{:?}", outcome);
                }
                return Ok(());
            }
            prop_assert_eq!(reviews, 1, "{:?}", outcome);
            prop_assert!(snapshot);
            let reviewed_snapshot = host.snapshots.lock().unwrap()[0].clone();
            match &reviewed_snapshot.evidence {
                StopEvidence::NoProgress { rule, native_call_ids, threshold, .. } => {
                    prop_assert!(case.no_progress);
                    prop_assert_eq!(*rule, StuckRule::RepeatedCall);
                    prop_assert_eq!(*threshold, 2);
                    prop_assert!(!native_call_ids.is_empty());
                    prop_assert!(native_call_ids.iter().all(|id| id.starts_with("obs-")), "{:?}", native_call_ids);
                    prop_assert!(reviewed_snapshot.truncated.is_none());
                }
                StopEvidence::Length { .. } => {
                    prop_assert!(!case.no_progress);
                    prop_assert!(reviewed_snapshot.truncated.is_some());
                }
                other => prop_assert!(false, "unexpected evidence {:?}", other),
            }
            match case.review {
                Review::Fails => {
                    prop_assert_eq!(outcome.stop, TaskStop::ReconciliationRequired);
                    prop_assert!(!receipt && !index);
                }
                Review::Uncommitted | Review::SettlementUnknown | Review::VerdictUnknown | Review::BadBasis => {
                    prop_assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{:?}", outcome);
                    prop_assert!(receipt && !index, "snapshot and receipt retained");
                }
                Review::Verified => {
                    prop_assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{:?}", outcome);
                    prop_assert!(index);
                }
                Review::EmptyFeedback => {
                    prop_assert_eq!(outcome.stop, TaskStop::NeedsChanges, "{:?}", outcome);
                }
                Review::NeedsChanges if case.max_runs == 1 => {
                    prop_assert_eq!(outcome.stop, TaskStop::BudgetExhausted, "{:?}", outcome);
                }
                Review::NeedsChanges => {
                    // A new Run in the same Task, capped by the remaining budget.
                    prop_assert_eq!(continuations.len(), 1);
                    prop_assert_eq!(continuations[0].0.as_str(), "original");
                    let admission: Value = serde_json::from_slice(
                        &std::fs::read(evidence.join("run-2-admission.json")).unwrap(),
                    ).unwrap();
                    prop_assert_eq!(admission["run_id"].as_str(), Some("stop-repair-2"));
                    prop_assert!(admission["remaining_calls"].as_u64().unwrap() as usize <= continuations[0].1);
                    prop_assert_eq!(outcome.runs, 2);
                    prop_assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{:?}", outcome);
                }
            }
            if case.review != Review::NeedsChanges || case.max_runs == 1 {
                prop_assert!(continuations.is_empty());
                prop_assert_eq!(outcome.runs, 1);
            }
        }
    }
}
