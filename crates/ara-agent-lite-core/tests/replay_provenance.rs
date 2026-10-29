//! Property 33 (requirements 24.6, 24.7): after the Host settles interrupted
//! calls as NotExecuted, a new call with the same tool and equal arguments
//! records `replay_of` pointing at the most recent such call, other calls do
//! not; a new call reusing a native call ID of the Session is never executed
//! and ends the Run with `duplicate_native_call_id`, leaving prior results.
mod support;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use ara_agent_lite_core::agent::{
    interrupted_run, reconcile_interrupted_run, ToolCallReconciliation, ToolCallSettlement,
};
use proptest::prelude::*;
use serde_json::{json, Value};
use support::*;

const TOOLS: [&str; 2] = ["alpha", "beta"];

fn args(value: usize) -> Value {
    json!({"value": value, "label": "fixture"})
}

/// Reorders keys: equal JSON values regardless of the model's key order.
fn args_reordered(value: usize) -> Value {
    serde_json::from_str(&format!(r#"{{"label":"fixture","value":{value}}}"#)).unwrap()
}

#[derive(Clone, Debug)]
struct NewCall {
    tool: usize,
    value: usize,
    reordered: bool,
    /// Reuse the native ID of the interrupted call at this index.
    reuse: Option<usize>,
}

fn new_call() -> impl Strategy<Value = NewCall> {
    (0..2usize, 0..3usize, any::<bool>(), prop::option::weighted(0.15, 0..4usize))
        .prop_map(|(tool, value, reordered, reuse)| NewCall { tool, value, reordered, reuse })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn replay_of_names_the_latest_matching_not_executed_call(
        interrupted in prop::collection::vec((0..2usize, 0..3usize, any::<bool>()), 1..5),
        fresh in prop::collection::vec(new_call(), 1..5),
    ) {
        let root = TempDir::new();
        let hold = Arc::new(AtomicBool::new(true));
        let started = Arc::new(AtomicBool::new(false));
        let tools: Vec<_> = TOOLS
            .iter()
            .map(|name| ScriptedTool::holding(name, hold.clone(), started.clone(), |args| Ok(format!("ran {args}"))))
            .collect();

        // Run 1 crashes inside its first call; every call of that round stays open.
        let old_ids: Vec<String> = (0..interrupted.len()).map(|index| format!("old-{index}")).collect();
        let round: Vec<(&str, &str, Value)> = interrupted
            .iter()
            .zip(&old_ids)
            .map(|((tool, value, _), id)| (id.as_str(), TOOLS[*tool], args(*value)))
            .collect();
        let crashed = Provider::new(vec![calls(&round)]);
        crash_mid_tool(request(root.path(), &crashed, &tools, 4, "Run the fixture tools."), started.clone());
        hold.store(false, Ordering::SeqCst);
        let session = root.path().join("session.json");
        let inspected = interrupted_run(&session, root.path()).unwrap().expect("pending Run");
        prop_assert_eq!(inspected.open_tool_calls.len(), interrupted.len());
        let settlements: Vec<_> = inspected
            .open_tool_calls
            .iter()
            .zip(&interrupted)
            .map(|(call, (_, _, not_executed))| ToolCallReconciliation {
                call_id: call.id.clone(),
                settlement: if *not_executed {
                    ToolCallSettlement::NotExecuted
                } else {
                    ToolCallSettlement::Completed { output: "Host receipt: finished".into() }
                },
            })
            .collect();
        reconcile_interrupted_run(&session, root.path(), &inspected, &settlements, &["host-receipt:1".into()]).unwrap();
        let settled = session_tool_results(&session);
        let executions_before: usize = tools.iter().map(|tool| tool.count()).sum();

        // Run 2 replays new calls one per model turn.
        let mut script = Vec::new();
        let mut expected = Vec::new();
        let mut duplicate = None;
        for (index, call) in fresh.iter().enumerate() {
            let id = call.reuse.filter(|old| *old < old_ids.len()).map_or(format!("new-{index}"), |old| old_ids[old].clone());
            let arguments = if call.reordered { args_reordered(call.value) } else { args(call.value) };
            script.push(support::call(&id, TOOLS[call.tool], arguments));
            if id.starts_with("old-") {
                duplicate = Some(id);
                break;
            }
            let replay = interrupted
                .iter()
                .enumerate()
                .rev()
                .find(|(_, (tool, value, not_executed))| *not_executed && *tool == call.tool && *value == call.value)
                .map(|(old, _)| json!({"run_id": inspected.run_id, "native_call_id": old_ids[old]}));
            expected.push((id, replay));
        }
        script.push(text("done"));
        let provider = Provider::new(script);
        let (result, _) = runtime().block_on(run(request(root.path(), &provider, &tools, 8, "Retry what is needed.")));

        let executions: usize = tools.iter().map(|tool| tool.count()).sum();
        prop_assert_eq!(executions - executions_before, expected.len(), "duplicates are never dispatched");
        let results = session_tool_results(&session);
        prop_assert_eq!(&results[..settled.len()], &settled[..], "prior results unchanged");
        let added = &results[settled.len()..];
        match &duplicate {
            Some(id) => prop_assert_eq!(result.error.clone(), format!("duplicate_native_call_id: {id}")),
            None => prop_assert_eq!(result.code, 0, "{}", result.error),
        }
        // The blocked duplicate may leave one non-executed result for its own call.
        let executed: Vec<&Value> = added.iter().filter(|result| result["toolCallId"].as_str().is_some_and(|id| id.starts_with("new-"))).collect();
        prop_assert_eq!(executed.len(), expected.len());
        for (result, (id, replay)) in executed.iter().zip(&expected) {
            prop_assert_eq!(result["toolCallId"].as_str(), Some(id.as_str()));
            prop_assert_eq!(result["isError"].as_bool(), Some(false));
            let recorded = result["details"].get("replay_of").cloned();
            prop_assert_eq!(recorded, replay.clone(), "call {}", id);
        }
    }
}
