//! Property 34 (requirements 25.1, 25.2), Core side: agent and compaction
//! summary calls share one Run call counter. Every reservation emits a
//! `model_call reserved` event, the counter never decreases and does not
//! depend on whether a `model_result` arrived (upstream errors refund
//! nothing), and a summary the budget cannot pay for stops the Run with
//! `SummaryBudgetExhausted` at the exact reserved count. The Harness-side
//! Task accumulation of the same events lives in the Harness crate.
mod support;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use ara_agent_lite_core::steering::SteeringBuffer;
use proptest::prelude::*;
use serde_json::{json, Value};
use support::*;

const SUMMARY_BUDGET_REASON: &str = "model call budget exhausted before compaction summary";

/// Run 1 leaves ~16 KiB; a ~16 KiB Run 2 answer then needs a summary of Run 1
/// before the next turn (see the Harness `summary_budget` test).
const CONTEXT_WINDOW: u64 = 60_000 + 512 + 4096;

#[derive(Clone, Copy, Debug)]
enum Agent {
    Big,
    Small,
    Tool,
    Upstream,
}

fn agent() -> impl Strategy<Value = Agent> {
    prop_oneof![3 => Just(Agent::Big), 2 => Just(Agent::Small), 2 => Just(Agent::Tool), 1 => Just(Agent::Upstream)]
}

struct Observed {
    result: ara_agent_lite_core::RunResult,
    events: Vec<Value>,
    /// Run 2 chat requests in order.
    bodies: Vec<Value>,
}

/// Run 1 writes a 16 KiB answer; Run 2 follows `replies` and, when
/// `steer[n]`, a Host note queued during its n-th agent request continues
/// the Run after a Stop.
fn scenario(replies: &[Agent], summaries: &[bool], steer: &[bool], max_calls: usize) -> Observed {
    let root = TempDir::new();
    let steering = Arc::new(Mutex::new(SteeringBuffer::default()));
    let agent_requests = Arc::new(AtomicUsize::new(0));
    let (hook_steering, hook_count, flags) = (steering.clone(), agent_requests.clone(), steer.to_vec());
    let mut responses = vec![text(&"a".repeat(16 * 1024))];
    responses.extend(replies.iter().enumerate().map(|(index, reply)| match reply {
        Agent::Big => text(&"b".repeat(16 * 1024)),
        Agent::Small => text("small answer"),
        Agent::Tool => call(&format!("t{index}"), "probe", json!({"n": index})),
        Agent::Upstream => http_error(503),
    }));
    let provider = Provider::with(ProviderConfig {
        context_window: CONTEXT_WINDOW,
        summary_responses: summaries.iter().map(|ok| if *ok { summary() } else { http_error(500) }).collect(),
        on_request: Box::new(move |body| {
            if is_summary(body) {
                return;
            }
            // Request 0 is Run 1; Run 2 agent requests are numbered from 1.
            let index = hook_count.fetch_add(1, Ordering::SeqCst);
            if index >= 1 && flags.get(index - 1).copied().unwrap_or(false) {
                let _ = hook_steering.lock().unwrap().submit_legacy("Host note: also report the size.".into());
            }
        }),
        ..ProviderConfig::new(responses)
    });
    let probe = ScriptedTool::new("probe", |_| Ok("probe state".into()));
    let rt = runtime();
    let (first, _) =
        rt.block_on(run(request(root.path(), &provider, std::slice::from_ref(&probe), 1, "Write the report.")));
    assert_eq!(first.code, 0, "{}", first.error);
    let before = provider.request_count();
    let mut second = request(root.path(), &provider, &[probe], max_calls, "Answer the follow-up.");
    second.steering = steering;
    let (result, events) = rt.block_on(run(second));
    let bodies = provider.bodies()[before..].to_vec();
    Observed { result, events, bodies }
}

fn check(observed: &Observed, max_calls: usize) -> Result<(), TestCaseError> {
    let reserved: Vec<&Value> =
        observed.events.iter().filter(|event| event["type"] == "model_call" && event["status"] == "reserved").collect();
    // One reservation event per upstream request, agent or summary.
    prop_assert_eq!(reserved.len(), observed.bodies.len());
    for (event, body) in reserved.iter().zip(&observed.bodies) {
        let phase = if is_summary(body) { "summary" } else { "agent" };
        prop_assert_eq!(event["phase"].as_str(), Some(phase));
    }
    prop_assert!(reserved.len() <= max_calls);
    // The shared counter only grows: reservation n carries call n.
    let numbers: Vec<u64> = reserved.iter().map(|event| event["call"].as_u64().unwrap()).collect();
    prop_assert_eq!(numbers, (1..=reserved.len() as u64).collect::<Vec<_>>());
    // Each agent request reports every earlier reservation, summaries included.
    // (Core omits the advisory carrier when it would not fit the request.)
    for (index, body) in observed.bodies.iter().enumerate().filter(|(_, body)| !is_summary(body)) {
        if let Some(budget) = run_budget(body) {
            prop_assert_eq!(budget["reserved_calls"].as_u64(), Some(index as u64));
            prop_assert_eq!(budget["call_limit"].as_u64(), Some(max_calls as u64));
        }
    }
    let end = of_type(&observed.events, "agent_end");
    prop_assert_eq!(end.len(), 1);
    if observed.result.error == SUMMARY_BUDGET_REASON {
        prop_assert_eq!(reserved.len(), max_calls);
        prop_assert_eq!(
            &end[0]["stop_evidence"],
            &json!({"cause":"summary_budget_exhausted","version":1,"run_calls":max_calls,"run_max_calls":max_calls})
        );
    } else if let Some(evidence) = end[0].get("stop_evidence") {
        prop_assert_ne!(&evidence["cause"], "summary_budget_exhausted");
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn reservations_count_agent_and_summary_calls_regardless_of_results(
        replies in prop::collection::vec(agent(), 1..5),
        summaries in prop::collection::vec(any::<bool>(), 0..2),
        steer in prop::collection::vec(any::<bool>(), 4),
        max_calls in 1usize..5,
    ) {
        let observed = scenario(&replies, &summaries, &steer, max_calls);
        check(&observed, max_calls)?;
    }
}

#[test]
fn summary_needed_with_no_call_left_is_a_summary_budget_stop() {
    let observed = scenario(&[Agent::Big], &[], &[true], 1);
    assert_eq!(observed.result.error, SUMMARY_BUDGET_REASON);
    assert_eq!(observed.bodies.len(), 1, "no summary request reached upstream");
    check(&observed, 1).unwrap();
}

#[test]
fn a_failed_summary_still_consumes_its_reservation() {
    let observed = scenario(&[Agent::Big, Agent::Small], &[false], &[true], 3);
    assert_ne!(observed.result.code, 0);
    let phases: Vec<_> = of_type(&observed.events, "model_call").iter().map(|e| e["phase"].clone()).collect();
    assert_eq!(phases, [json!("agent"), json!("summary")]);
    assert_eq!(observed.bodies.len(), 2);
    check(&observed, 3).unwrap();
}
