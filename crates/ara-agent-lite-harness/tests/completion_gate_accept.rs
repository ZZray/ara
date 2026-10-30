//! Requirement 17.8 (task 44.5, Harness half): a Task whose Completion_Gate
//! fails once and then passes ends with `task_accepted` false. Kept in the
//! Harness crate because Core does not depend on the Harness.
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use anyhow::Result;
use ara_agent_lite_harness::{
    core::completion_gate::{CompletionGate, GateRequest, GateVerdict, Registration},
    *,
};
use async_trait::async_trait;
use common::*;
use tokio_util::sync::CancellationToken;

struct FailOnceGate {
    checks: AtomicUsize,
}

#[async_trait]
impl CompletionGate for FailOnceGate {
    async fn check(&self, _: GateRequest, _: CancellationToken) -> Result<GateVerdict> {
        Ok(match self.checks.fetch_add(1, Ordering::SeqCst) {
            0 => GateVerdict::Failed { feedback: "public check failed: answer.txt is not final".into() },
            _ => GateVerdict::Passed,
        })
    }
}

#[tokio::test]
async fn a_passed_gate_never_accepts_the_task() {
    let directory = tempfile::tempdir().unwrap();
    let provider = Provider::new(vec![write_answer("correct"), text("Submitted."), text("Checked again; submitted.")]);
    let host = Arc::new(Host::new(directory.path(), &provider, Fault::None));
    let gate = Arc::new(FailOnceGate { checks: AtomicUsize::new(0) });
    let mut request = host.request("initial");
    request.policies.completion_gate =
        Some(Registration { check_id: "public-checks".into(), gate: gate.clone(), timeout: None });
    let outcome = start(host.spec("gate"), request, host.clone()).unwrap().wait().await;
    assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{outcome:?}");
    assert!(!outcome.task_accepted);
    assert_eq!(gate.checks.load(Ordering::SeqCst), 2, "failed once, then passed");
    assert_eq!((outcome.runs, outcome.observed_calls), (1, 3));
    let rows = events(&outcome.last_candidate.as_ref().unwrap().basis.events_path);
    let results: Vec<_> = rows
        .iter()
        .filter(|row| row["event"]["type"] == "completion_gate")
        .map(|row| row["event"]["result"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(results, ["failed", "passed"]);
}
