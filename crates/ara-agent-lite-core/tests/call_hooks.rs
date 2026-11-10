mod support;

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use ara_agent_lite_core::{
    call_hooks::{AfterToolCall, BeforeToolCall, BeforeToolCallDecision, HostCallHooks},
    execute, ToolRegistration,
};
use serde_json::json;
use support::{call, run, text, Provider, ScriptedTool, TempDir};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

struct RecordingHooks {
    deny: bool,
    fail_before: bool,
    fail_after: bool,
    before: Mutex<Vec<BeforeToolCall>>,
    after: Mutex<Vec<AfterToolCall>>,
}

impl HostCallHooks for RecordingHooks {
    fn before_tool_call(&self, call: BeforeToolCall) -> Result<BeforeToolCallDecision> {
        self.before.lock().unwrap().push(call);
        if self.fail_before {
            return Err(anyhow!("controlled before hook failure"));
        }
        Ok(if self.deny { BeforeToolCallDecision::deny("controlled denial") } else { BeforeToolCallDecision::allow() })
    }

    fn after_tool_call(&self, call: AfterToolCall) -> Result<()> {
        self.after.lock().unwrap().push(call);
        if self.fail_after {
            return Err(anyhow!("controlled after hook failure"));
        }
        Ok(())
    }
}

fn request(root: &TempDir, provider: &Provider, tool: Arc<ScriptedTool>) -> ara_agent_lite_core::RunRequest {
    let mut request = support::request(root.path(), provider, &[], 4, "Use probe once.");
    request.extra_tools.push(ToolRegistration { tool, allow_in_plan: true });
    request
}

#[tokio::test]
async fn before_hook_can_deny_without_dispatching_the_tool() {
    let root = TempDir::new();
    let provider = Provider::new(vec![call("call-1", "probe", json!({"value": 1})), text("done")]);
    let tool = ScriptedTool::new("probe", |_| Ok("executed".into()));
    let hooks = Arc::new(RecordingHooks {
        deny: true,
        fail_before: false,
        fail_after: false,
        before: Mutex::new(Vec::new()),
        after: Mutex::new(Vec::new()),
    });
    let mut request = request(&root, &provider, tool.clone());
    request.call_hooks = Some(hooks.clone());

    let (result, _) = run(request).await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert_eq!(tool.count(), 0);
    assert_eq!(hooks.before.lock().unwrap().len(), 1);
    assert!(hooks.after.lock().unwrap().is_empty());
}

#[tokio::test]
async fn after_hook_observes_success_and_tool_error() {
    for (tool_error, expected_error) in [(false, false), (true, true)] {
        let root = TempDir::new();
        let provider = Provider::new(vec![call("call-1", "probe", json!({"value": 1})), text("done")]);
        let tool = if tool_error {
            ScriptedTool::new("probe", |_| Err("controlled tool failure".into()))
        } else {
            ScriptedTool::new("probe", |_| Ok("executed".into()))
        };
        let hooks = Arc::new(RecordingHooks {
            deny: false,
            fail_before: false,
            fail_after: false,
            before: Mutex::new(Vec::new()),
            after: Mutex::new(Vec::new()),
        });
        let mut request = request(&root, &provider, tool.clone());
        request.call_hooks = Some(hooks.clone());

        let (result, _) = run(request).await;
        assert_eq!(result.code, 0, "{}", result.error);
        let after = hooks.after.lock().unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].call_id, "call-1");
        assert_eq!(after[0].is_error, expected_error);
        assert_eq!(after[0].tool_name, "probe");
    }
}

#[tokio::test]
async fn after_hook_failure_stops_before_a_second_model_request() {
    let root = TempDir::new();
    let provider = Provider::new(vec![call("call-1", "probe", json!({})), text("must not be requested")]);
    let tool = ScriptedTool::new("probe", |_| Ok("executed".into()));
    let hooks = Arc::new(RecordingHooks {
        deny: false,
        fail_before: false,
        fail_after: true,
        before: Mutex::new(Vec::new()),
        after: Mutex::new(Vec::new()),
    });
    let mut request = request(&root, &provider, tool);
    request.call_hooks = Some(hooks.clone());

    let (result, _) = run(request).await;
    assert_ne!(result.code, 0);
    assert_eq!(provider.request_count(), 1, "after hook failure must stop the loop");
    assert_eq!(hooks.after.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn no_hook_preserves_the_existing_tool_flow() {
    let root = TempDir::new();
    let provider = Provider::new(vec![call("call-1", "probe", json!({})), text("done")]);
    let tool = ScriptedTool::new("probe", |_| Ok("executed".into()));
    let request = request(&root, &provider, tool.clone());

    let (result, _) = run(request).await;
    assert_eq!(result.code, 0, "{}", result.error);
    assert_eq!(tool.count(), 1);
    assert_eq!(provider.request_count(), 2);
}

#[tokio::test]
async fn hook_failure_propagates_and_does_not_dispatch() {
    let root = TempDir::new();
    let provider = Provider::new(vec![call("call-1", "probe", json!({}))]);
    let tool = ScriptedTool::new("probe", |_| Ok("must not run".into()));
    let hooks = Arc::new(RecordingHooks {
        deny: false,
        fail_before: true,
        fail_after: false,
        before: Mutex::new(Vec::new()),
        after: Mutex::new(Vec::new()),
    });
    let mut request = request(&root, &provider, tool.clone());
    request.call_hooks = Some(hooks.clone());
    let (sender, mut receiver) = mpsc::channel(64);
    tokio::spawn(async move { while receiver.recv().await.is_some() {} });

    let outcome = execute(request, CancellationToken::new(), sender).await;
    assert!(outcome.is_err() || outcome.as_ref().is_ok_and(|result| result.code != 0));
    assert_eq!(tool.count(), 0);
    assert_eq!(hooks.before.lock().unwrap().len(), 1);
}
