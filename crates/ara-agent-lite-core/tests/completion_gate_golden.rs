//! Requirement 17.7: without a registered Completion_Gate, a controlled Run
//! produces the event type sequence, stop reason and Session journal entries
//! of `tests/golden/p2b-no-gate/` and no `completion_gate` event. The
//! Harness half (17.8, `task_accepted` after a passed gate) is
//! `crates/ara-agent-lite-harness/tests/completion_gate_accept.rs`: Core does
//! not depend on the Harness.
mod golden_support;
mod support;

use golden_support::{check, event_types, journal_entries, Normalizer};
use serde_json::json;
use support::*;

const GOLDEN: &str = "p2b-no-gate";

#[tokio::test]
async fn without_a_gate_the_run_matches_the_golden() {
    let root = TempDir::new();
    let provider = Provider::new(vec![call("p1", "probe", json!({"n": 1})), text("answer-2")]);
    let probe = ScriptedTool::new("probe", |_| Ok("fixture state".into()));
    let request = request(root.path(), &provider, std::slice::from_ref(&probe), 4, "Use probe and answer.");
    assert!(request.policies.completion_gate.is_none());
    let (result, events) = run(request).await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert!(of_type(&events, "completion_gate").is_empty());
    assert_eq!(provider.request_count(), 2);

    let mut normalizer = Normalizer::new(root.path(), &provider.endpoint);
    check(GOLDEN, "events.json", &event_types(&events));
    let stop = json!({"code": result.code, "error": result.error, "text": result.text});
    check(GOLDEN, "stop.json", &normalizer.value(&stop));
    check(GOLDEN, "journal.json", &normalizer.value(&journal_entries(&root.path().join("session.json"))));
}
