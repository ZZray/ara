//! Length stop review and continuation flow (requirements 16.1, 16.3, 16.5)
//! over the real Core `execute`, the real Session journal and a loopback
//! provider that answers `finish_reason=length`.
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use anyhow::Result;
use ara_agent_lite_harness::{
    core::{
        model::{CatalogModelHost, GenerationPolicy, LengthRecoveryPolicy, ModelHost, ModelSelection, Route},
        session_journal::Journal,
        termination::StopEvidence,
        RunRequest,
    },
    *,
};
use async_trait::async_trait;
use common::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

const CUT: &str = "partial report cut at the output limit";
/// Below the request (512) and the catalog `max_tokens` (4096).
const HOST_CEILING: u32 = 300;

fn length(value: &str) -> Value {
    json!({"delta":{"content":value},"finish":"length"})
}

/// Catalog resolution plus a Host output ceiling and a length-recovery policy.
struct PolicyHost {
    ceiling: Option<u32>,
    recovery: LengthRecoveryPolicy,
}

#[async_trait]
impl ModelHost for PolicyHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        CatalogModelHost.resolve(route, endpoint, token, run).await
    }
    fn capabilities_source(&self) -> &'static str {
        CatalogModelHost.capabilities_source()
    }
    fn generation_policy(&self, route: &Route) -> Result<GenerationPolicy> {
        CatalogModelHost.generation_policy(route)
    }
    fn length_recovery_policy(&self, _: &Route) -> Result<LengthRecoveryPolicy> {
        Ok(self.recovery)
    }
    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(self.ceiling)
    }
}

/// What the continuation callback saw of the truncated message.
#[derive(Clone, Debug)]
struct SeenAtPrepare {
    bytes: Vec<u8>,
    proof: String,
}

struct FlowHost {
    base: Host,
    stop_reviews: AtomicUsize,
    snapshots: Mutex<Vec<StopSnapshot>>,
    at_prepare: Mutex<Option<SeenAtPrepare>>,
}

#[async_trait]
impl HarnessHost for FlowHost {
    async fn verify(&self, candidate: Candidate, cancel: CancellationToken) -> Result<ReviewReceipt> {
        self.base.verify(candidate, cancel).await
    }
    async fn prepare_continuation(&self, continuation: Continuation, cancel: CancellationToken) -> Result<RunRequest> {
        self.base.prepare_continuation(continuation, cancel).await
    }
    async fn verify_stop_snapshot(&self, snapshot: StopSnapshot, _: CancellationToken) -> Result<ReviewReceipt> {
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
        let truncated = continuation.previous.truncated.as_ref().expect("Length snapshot names the truncated message");
        let journal = Journal::for_session(&continuation.previous.basis.session_path);
        let stored = journal.read(truncated.raw_index, truncated.raw_index + 1, 1 << 20)?;
        *self.at_prepare.lock().unwrap() = Some(SeenAtPrepare {
            bytes: serde_json::to_vec(&stored[0])?,
            proof: journal.proof(truncated.raw_index, truncated.raw_index + 1)?,
        });
        let mut request = self.base.request(&format!("stop-repair-{}", continuation.next_run_number));
        request.objective = "Repair the answer after the truncated response.".into();
        Ok(request)
    }
}

fn flow_host(root: &std::path::Path, provider: &Provider, model_host: PolicyHost) -> Arc<FlowHost> {
    let mut base = Host::new(root, provider, Fault::None);
    base.model_host = Arc::new(model_host);
    Arc::new(FlowHost {
        base,
        stop_reviews: AtomicUsize::new(0),
        snapshots: Mutex::default(),
        at_prepare: Mutex::default(),
    })
}

fn review_length() -> TaskOptions {
    TaskOptions { length_stop_review: LengthStopReviewPolicy::ReviewClosedLength, ..TaskOptions::default() }
}

async fn run(host: &Arc<FlowHost>, name: &str, options: TaskOptions) -> Arc<TaskOutcome> {
    let handle =
        start_with_options(host.base.spec(name), host.base.request("original"), host.clone(), options).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(30), handle.wait()).await.unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[tokio::test]
async fn length_snapshot_binds_the_cut_message_and_continuation_keeps_it_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let provider = Provider::new(vec![write_answer("wrong"), length(CUT), write_answer("correct"), text("submitted")]);
    let host =
        flow_host(&root, &provider, PolicyHost { ceiling: Some(HOST_CEILING), recovery: LengthRecoveryPolicy::Stop });
    let outcome = run(&host, "flow", review_length()).await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert_eq!((outcome.runs, host.stop_reviews.load(Ordering::SeqCst)), (2, 1));
    assert!(!outcome.task_accepted);

    // 16.1: ReviewBasis of Run 1, a journal-bound reference to the cut message
    // and the effective output limit of that call.
    let snapshot = host.snapshots.lock().unwrap()[0].clone();
    assert_eq!((snapshot.basis.run_id.as_str(), snapshot.basis.run_number), ("original", 1));
    let StopEvidence::Length { source_raw_index, source_sha256, effective_output_tokens, .. } = &snapshot.evidence
    else {
        panic!("expected Length evidence, got {:?}", snapshot.evidence)
    };
    let truncated = snapshot.truncated.clone().expect("truncated message reference");
    assert_eq!((truncated.raw_index, &truncated.typed_sha256), (*source_raw_index, source_sha256));
    assert!(truncated.raw_index >= snapshot.basis.raw_start && truncated.raw_index < snapshot.basis.raw_end);
    assert_eq!(truncated.effective_output_tokens, u64::from(HOST_CEILING));
    assert_eq!(*effective_output_tokens, u64::from(HOST_CEILING));
    let evidence = root.join("flow");
    let rows = events(&evidence.join("run-1-events.jsonl"));
    // The `reserved` receipt of each agent call carries its output limit.
    let agent_calls: Vec<&Value> = rows
        .iter()
        .filter(|row| {
            row["event"]["type"] == "model_call"
                && row["event"]["phase"] == "agent"
                && row["event"]["status"] == "reserved"
        })
        .collect();
    assert_eq!(agent_calls.len(), 2, "write call and the cut call");
    let cut_call = agent_calls[1];
    assert_eq!(cut_call["event"]["output_limit"]["effective"], u64::from(HOST_CEILING));
    assert_eq!(cut_call["event"]["output_limit"]["sources"], json!(["host_policy"]));
    let request_limit = |body: &Value| body.get("max_tokens").or_else(|| body.get("max_completion_tokens")).cloned();
    let bodies = provider.requests.lock().unwrap().clone();
    assert_eq!(request_limit(&bodies[1]), Some(json!(HOST_CEILING)), "the wire carries the recorded limit");

    // The cut text is diagnostic output, never a final answer or a Candidate.
    assert_eq!(snapshot.diagnostic_output, CUT);
    assert!(!evidence.join("run-1-candidate.json").exists());
    let last = outcome.last_candidate.as_ref().expect("the continued Run ends with an ordinary candidate");
    assert_eq!(last.basis.run_number, 2);
    assert_ne!(last.output, CUT);
    let journal = Journal::for_session(&snapshot.basis.session_path);
    let stored = journal.read(truncated.raw_index, truncated.raw_index + 1, 1 << 20).unwrap();
    let stored_json = serde_json::to_value(&stored[0]).unwrap();
    assert_eq!(stored_json["role"], "assistant");
    assert_eq!(stored_json["stopReason"], "length", "{stored_json}");

    // 16.3: the cut message bytes and its journal proof are the same before
    // the continuation (seen by the Host) and after the Task ended; the
    // continuation only appended after the snapshot's range.
    let seen = host.at_prepare.lock().unwrap().clone().expect("prepare_stop_continuation ran");
    let after = serde_json::to_vec(&stored[0]).unwrap();
    assert_eq!(after, seen.bytes);
    assert_eq!(sha256(&after), truncated.typed_sha256);
    let proof = journal.proof(truncated.raw_index, truncated.raw_index + 1).unwrap();
    assert_eq!(
        (proof.as_str(), seen.proof.as_str()),
        (truncated.journal_proof.as_str(), truncated.journal_proof.as_str())
    );
    assert_eq!(journal.proof(snapshot.basis.raw_start, snapshot.basis.raw_end).unwrap(), snapshot.basis.raw_proof);
    assert!(last.basis.raw_start >= snapshot.basis.raw_end, "{:?} vs {:?}", last.basis, snapshot.basis);
    let admission: Value =
        serde_json::from_slice(&std::fs::read(evidence.join("run-2-admission.json")).unwrap()).unwrap();
    assert_eq!(admission["run_id"], "stop-repair-2");
    assert_eq!(std::fs::read(root.join("answer.txt")).unwrap(), b"correct");
}

/// Event types of one Run's events file, in order.
fn event_types(path: &std::path::Path) -> Vec<String> {
    events(path).iter().map(|row| row["event"]["type"].as_str().unwrap_or_default().to_owned()).collect()
}

#[tokio::test]
async fn policy_off_keeps_pre_feature_stops_and_length_recovery_is_unchanged() {
    // Per-Run length recovery replans the first cut once; the second cut ends the Run.
    for recovery in [LengthRecoveryPolicy::Stop, LengthRecoveryPolicy::ReplanOnce] {
        let mut observed = Vec::new();
        for options in [TaskOptions::default(), review_length()] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path().canonicalize().unwrap();
            let provider = Provider::new(vec![write_answer("correct"), length("cut one"), length("cut two")]);
            let host = flow_host(&root, &provider, PolicyHost { ceiling: None, recovery });
            let outcome = run(&host, "compat", options).await;
            let evidence = root.join("compat");
            let types = event_types(&evidence.join("run-1-events.jsonl"));
            let recoveries = types.iter().filter(|kind| *kind == "length_recovery").count();
            let requests = provider.requests.lock().unwrap().len();
            if options == TaskOptions::default() {
                // 16.5: the same conclusion and Run count as before the policy existed,
                // and no stop callbacks.
                assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{outcome:?}");
                assert_eq!(outcome.runs, 1);
                assert_eq!(host.stop_reviews.load(Ordering::SeqCst), 0);
                assert!(host.at_prepare.lock().unwrap().is_none());
                assert!(!stop_snapshot_path(&evidence, 1).exists());
                let rows = events(&evidence.join("run-1-events.jsonl"));
                let end = rows.iter().find(|row| row["event"]["type"] == "agent_end").unwrap();
                assert!(end["event"].get("stop_evidence").is_none());
            } else {
                assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
                assert_eq!((outcome.runs, host.stop_reviews.load(Ordering::SeqCst)), (1, 1));
            }
            assert!(!outcome.task_accepted);
            observed.push((types, recoveries, requests, outcome.observed_calls));
        }
        // The Harness policy changes neither the per-Run recovery nor the Run's events.
        assert_eq!(observed[0], observed[1], "{recovery:?}");
        let expected = if recovery == LengthRecoveryPolicy::ReplanOnce { (1, 3) } else { (0, 2) };
        assert_eq!((observed[0].1, observed[0].2), expected, "{recovery:?}: {:?}", observed[0].0);
    }
}
