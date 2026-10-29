//! Requirement 24 examples: inspecting and settling an interrupted Run
//! executes no tool (24.1); a continuation whose response has no tool call
//! executes nothing (24.3, 24.4); after a NotExecuted settlement a call with
//! the same tool and arguments is an ordinary call under the model's own
//! native call ID (24.5); a reused native call ID is refused (24.7).
mod support;

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use ara_agent_lite_core::agent::{
    interrupted_run, reconcile_interrupted_run, InterruptedRun, ToolCallReconciliation, ToolCallSettlement,
};
use serde_json::json;
use support::*;

struct Crashed {
    root: TempDir,
    tool: Arc<ScriptedTool>,
    inspected: InterruptedRun,
}

/// One `deploy` call that was interrupted, then settled NotExecuted by the Host.
fn crashed_and_settled() -> Crashed {
    let root = TempDir::new();
    let hold = Arc::new(AtomicBool::new(true));
    let started = Arc::new(AtomicBool::new(false));
    let tool = ScriptedTool::holding("deploy", hold.clone(), started.clone(), |args| Ok(format!("deployed {args}")));
    let provider = Provider::new(vec![call("deploy-1", "deploy", json!({"step": 1}))]);
    crash_mid_tool(request(root.path(), &provider, std::slice::from_ref(&tool), 4, "Deploy step 1."), started);
    hold.store(false, Ordering::SeqCst);
    assert_eq!(tool.count(), 1, "the interrupted execution");
    let session = root.path().join("session.json");
    let before = std::fs::read(&session).unwrap();
    let inspected = interrupted_run(&session, root.path()).unwrap().expect("pending Run");
    assert_eq!(inspected.open_tool_calls.len(), 1);
    assert_eq!(std::fs::read(&session).unwrap(), before, "inspection writes nothing");
    let settlement = ToolCallReconciliation { call_id: "deploy-1".into(), settlement: ToolCallSettlement::NotExecuted };
    reconcile_interrupted_run(&session, root.path(), &inspected, &[settlement], &["host-log:1".into()]).unwrap();
    assert_eq!(tool.count(), 1, "inspection and settlement execute no tool");
    assert_eq!(provider.request_count(), 1);
    Crashed { root, tool, inspected }
}

#[tokio::test(flavor = "multi_thread")]
async fn same_call_after_not_executed_runs_once_under_the_new_native_id() {
    let crashed = crashed_and_settled();
    let provider = Provider::new(vec![call("deploy-2", "deploy", json!({"step": 1})), text("deployed")]);
    let (result, _) =
        run(request(crashed.root.path(), &provider, std::slice::from_ref(&crashed.tool), 4, "Retry step 1 if needed."))
            .await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert_eq!(crashed.tool.count(), 2, "exactly one new execution");
    let results = session_tool_results(&crashed.root.path().join("session.json"));
    let replayed = results.iter().find(|result| result["toolCallId"] == "deploy-2").expect("new result");
    assert_eq!(replayed["isError"], false);
    assert_eq!(
        replayed["details"]["replay_of"],
        json!({"run_id": crashed.inspected.run_id, "native_call_id": "deploy-1"})
    );
    let settled = results.iter().find(|result| result["toolCallId"] == "deploy-1").unwrap();
    assert_eq!(settled["details"]["source"], "host_reconciled_interrupted_run");
}

#[tokio::test(flavor = "multi_thread")]
async fn continuation_without_tool_calls_executes_nothing() {
    let crashed = crashed_and_settled();
    let provider = Provider::new(vec![text("The Host reported step 1 did not run; nothing to retry now.")]);
    let (result, _) =
        run(request(crashed.root.path(), &provider, std::slice::from_ref(&crashed.tool), 2, "Continue.")).await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert_eq!(crashed.tool.count(), 1, "zero executions in the continuation");
}

#[tokio::test(flavor = "multi_thread")]
async fn reused_interrupted_call_id_is_refused_without_execution() {
    let crashed = crashed_and_settled();
    let provider = Provider::new(vec![call("deploy-1", "deploy", json!({"step": 1})), text("x")]);
    let session = crashed.root.path().join("session.json");
    let before = session_tool_results(&session);
    let (result, _) =
        run(request(crashed.root.path(), &provider, std::slice::from_ref(&crashed.tool), 4, "Retry.")).await;
    assert_eq!(result.error, "duplicate_native_call_id: deploy-1");
    assert_eq!(crashed.tool.count(), 1);
    assert_eq!(&session_tool_results(&session)[..before.len()], &before[..]);
}
