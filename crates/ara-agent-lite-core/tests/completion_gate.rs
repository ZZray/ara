//! Property 23 (requirements 17.1–17.6, 17.9, 17.10): Completion_Gate loop.
//! Random provider responses (ordinary Stop, tool call, Length, upstream
//! error) and gate results (passed, failed, error, timeout, cancelled) with a
//! random call budget run through the real `execute`; a reference model
//! predicts every gate call, continuation and the final reason.
mod support;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, Result};
use ara_agent_lite_core::completion_gate::{CompletionGate, GateRequest, GateVerdict, Registration};
use async_trait::async_trait;
use proptest::prelude::*;
use serde_json::{json, Value};
use support::*;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Reply {
    Stop,
    Tool,
    Length,
    UpstreamError,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    Pass,
    Fail,
    Error,
    Timeout,
    Cancel,
}

impl Step {
    fn label(self) -> &'static str {
        match self {
            Step::Pass => "passed",
            Step::Fail => "failed",
            Step::Error => "error",
            Step::Timeout => "timeout",
            Step::Cancel => "cancelled",
        }
    }
}

struct ScriptedGate {
    steps: Vec<Step>,
    seen: Mutex<Vec<GateRequest>>,
    run_cancellation: CancellationToken,
}

#[async_trait]
impl CompletionGate for ScriptedGate {
    async fn check(&self, request: GateRequest, cancellation: CancellationToken) -> Result<GateVerdict> {
        let index = {
            let mut seen = self.seen.lock().unwrap();
            seen.push(request);
            seen.len() - 1
        };
        match self.steps.get(index).copied().unwrap_or(Step::Pass) {
            Step::Pass => Ok(GateVerdict::Passed),
            Step::Fail => {
                Ok(GateVerdict::Failed { feedback: format!("public check {} failed: 1 of 3 cases", index + 1) })
            }
            Step::Error => bail!("gate infrastructure failed"),
            Step::Timeout => {
                tokio::select! {
                    _ = cancellation.cancelled() => {},
                    _ = tokio::time::sleep(Duration::from_secs(20)) => {},
                }
                Ok(GateVerdict::Passed)
            }
            Step::Cancel => {
                // Core observes the Run cancellation; this check never answers.
                self.run_cancellation.cancel();
                std::future::pending::<()>().await;
                unreachable!()
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum End {
    Success(String),
    Budget,
    GateError(&'static str),
    Cancelled,
    Length,
    Upstream,
}

struct Expected {
    calls: usize,
    /// Gate step, the model calls used when it ran and the checked Stop text.
    gates: Vec<(Step, usize, String)>,
    end: End,
}

/// Reference model of the loop in the property text.
fn model(replies: &[Reply], steps: &[Step], max_calls: usize) -> Expected {
    let (mut calls, mut gates) = (0, Vec::new());
    loop {
        let reply = replies.get(calls).copied().unwrap_or(Reply::Stop);
        calls += 1;
        let end = match reply {
            Reply::Tool if calls >= max_calls => End::Budget,
            Reply::Tool => continue,
            Reply::Length => End::Length,
            Reply::UpstreamError => End::Upstream,
            Reply::Stop => {
                let step = steps.get(gates.len()).copied().unwrap_or(Step::Pass);
                let text = if calls <= replies.len() { answer(calls) } else { UNSCRIPTED.to_owned() };
                gates.push((step, calls, text.clone()));
                match step {
                    Step::Pass => End::Success(text),
                    Step::Fail if calls >= max_calls => End::Budget,
                    Step::Fail => continue,
                    Step::Error => End::GateError("error"),
                    Step::Timeout => End::GateError("timeout"),
                    Step::Cancel => End::Cancelled,
                }
            }
        };
        return Expected { calls, gates, end };
    }
}

/// Default reply of the loopback provider after the script ends.
const UNSCRIPTED: &str = "No further controlled response.";

fn answer(call: usize) -> String {
    format!("answer-{call}")
}

fn script(replies: &[Reply]) -> Vec<Value> {
    replies
        .iter()
        .enumerate()
        .map(|(index, reply)| match reply {
            Reply::Stop => text(&answer(index + 1)),
            Reply::Tool => call(&format!("c{}", index + 1), "probe", json!({"n": index + 1})),
            Reply::Length => length("truncated"),
            Reply::UpstreamError => http_error(500),
        })
        .collect()
}

fn reply() -> impl Strategy<Value = Reply> {
    prop_oneof![4 => Just(Reply::Stop), 2 => Just(Reply::Tool), 1 => Just(Reply::Length), 1 => Just(Reply::UpstreamError)]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        2 => Just(Step::Pass),
        4 => Just(Step::Fail),
        1 => Just(Step::Error),
        1 => Just(Step::Timeout),
        1 => Just(Step::Cancel),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn gate_runs_once_per_ordinary_stop_and_continues_only_within_budget(
        replies in prop::collection::vec(reply(), 1..7),
        steps in prop::collection::vec(step(), 0..5),
        max_calls in 1usize..7,
    ) {
        let expected = model(&replies, &steps, max_calls);
        let root = TempDir::new();
        let provider = Provider::new(script(&replies));
        let probe = ScriptedTool::new("probe", |_| Ok("fixture state".into()));
        let cancellation = CancellationToken::new();
        let gate = Arc::new(ScriptedGate {
            steps: steps.clone(),
            seen: Mutex::new(Vec::new()),
            run_cancellation: cancellation.clone(),
        });
        let mut request = request(root.path(), &provider, std::slice::from_ref(&probe), max_calls, "Use probe and answer.");
        let run_id = request.id.clone();
        request.policies.completion_gate = Some(Registration {
            check_id: "public-checks".into(),
            gate: gate.clone(),
            timeout: Some(Duration::from_millis(300)),
        });
        let (result, events) = runtime().block_on(run_with(request, cancellation));

        // Gate calls: one per ordinary Stop, same Run, numbered, never a model call.
        let seen = gate.seen.lock().unwrap().clone();
        prop_assert_eq!(seen.len(), expected.gates.len());
        for (index, (request, (_, calls, text))) in seen.iter().zip(&expected.gates).enumerate() {
            prop_assert_eq!(&request.run_id, &run_id);
            prop_assert_eq!(request.sequence as usize, index + 1);
            prop_assert_eq!(request.calls_used, *calls);
            prop_assert_eq!(&request.final_text, text);
        }
        prop_assert_eq!(provider.request_count(), expected.calls, "model requests");
        let model_calls = of_type(&events, "model_call");
        prop_assert_eq!(model_calls.len(), expected.calls, "Logical_Call count");
        prop_assert!(model_calls.iter().all(|call| call["phase"] == "agent"));

        // One event per check, before any later action.
        let positions: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event["type"] == "completion_gate")
            .map(|(index, _)| index)
            .collect();
        prop_assert_eq!(positions.len(), expected.gates.len());
        let bodies = provider.bodies();
        for (index, ((step, calls, text), position)) in expected.gates.iter().zip(&positions).enumerate() {
            let event = &events[*position];
            prop_assert_eq!(event["result"].as_str(), Some(step.label()));
            prop_assert_eq!(event["sequence"].as_u64(), Some(index as u64 + 1));
            let continued = *step == Step::Fail && *calls < max_calls;
            prop_assert_eq!(event["feedback_ref"].is_string(), continued);
            let later_calls = events[..*position].iter().filter(|event| event["type"] == "model_call").count();
            prop_assert_eq!(later_calls, *calls, "the check runs before any later model call");
            if continued {
                // The next request keeps the Stop and appends exactly one sourced feedback after it.
                let next = bodies[*calls].to_string();
                let header = format!("source=completion_gate; check_id=public-checks; sequence={}", index + 1);
                prop_assert_eq!(next.matches(&header).count(), 1);
                let stop_at = next.rfind(text.as_str()).unwrap();
                prop_assert!(stop_at < next.find(&header).unwrap());
            } else if let Some(next) = bodies.get(*calls) {
                let header = format!("source=completion_gate; check_id=public-checks; sequence={}", index + 1);
                prop_assert!(!next.to_string().contains(&header));
            }
        }
        let end = of_type(&events, "agent_end");
        prop_assert_eq!(end.len(), 1);
        prop_assert!(positions.iter().all(|position| *position < events.len() - 1));
        match &expected.end {
            End::Success(text) => {
                prop_assert_eq!(result.code, 0, "{}", result.error);
                prop_assert_eq!(&result.text, text);
            }
            End::Budget => prop_assert_eq!(result.error.as_str(), "model call budget exhausted before a final response"),
            End::GateError(kind) => prop_assert_eq!(result.error.clone(), format!("completion_gate_error: {kind}")),
            End::Cancelled => prop_assert_eq!(result.error.as_str(), "run canceled; partial effects may remain"),
            End::Length => prop_assert_eq!(result.error.as_str(), "assistant stopped with length"),
            End::Upstream => prop_assert_ne!(result.code, 0),
        }
        if !matches!(expected.end, End::Success(_)) {
            prop_assert_ne!(result.code, 0);
        }
    }
}
