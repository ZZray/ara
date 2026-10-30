//! Property 29: unknown effects are judged only from structured status
//! (requirements 22.1, 22.2, 22.3, 22.8), over the real Core `execute` and
//! a loopback provider. A custom tool's result text never decides the
//! verdict; only `details` markers or an `unknown_effect_record` key do.
mod common;

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use ara_agent_lite_harness::{
    core::{
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
    },
    *,
};
use async_trait::async_trait;
use common::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// Result texts: plain, each of the four builtin phrases, and JSON text that
/// carries structured-marker spelling. None of them is structured status.
const TEXTS: [&str; 7] = [
    "probe finished",
    "effects_unknown: printed by a custom tool",
    "partial effects may remain (custom wording)",
    "effects may be unknown (custom wording)",
    "do not replay the command without reconciling its effects",
    r#"{"executed":"unknown","effects_unknown":true}"#,
    r#"{"status":"effects_unknown","unknown_effect_record":{"x":1}}"#,
];

/// Details variants. The marker keys mirror the structured contract of
/// requirement 22.1; `unknown_effect_record` may be unparsable (22.8).
fn details() -> Vec<Value> {
    vec![
        Value::Null,
        json!({"executed":"yes"}),
        json!({"note":"effects_unknown: text inside a value is not a marker"}),
        json!({"effects_unknown":false}),
        json!({"executed":"unknown"}),
        json!({"effects_unknown":true}),
        json!({"panicked":true}),
        json!({"nested":[{"status":"effects_unknown"}]}),
        json!({"source":"interrupted_unknown_effect"}),
        json!({"unknown_effect_record":"not a record"}),
        json!({"outer":{"unknown_effect_record":{"bad":1}}}),
    ]
}

/// Independent reference of the structured judgment.
fn reference_unknown(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(reference_unknown),
        Value::Object(object) => {
            object.contains_key("unknown_effect_record")
                || object.get("executed") == Some(&json!("unknown"))
                || object.get("panicked") == Some(&json!(true))
                || object.get("effects_unknown") == Some(&json!(true))
                || object.get("source") == Some(&json!("interrupted_unknown_effect"))
                || object.get("status") == Some(&json!("effects_unknown"))
                || object.values().any(reference_unknown)
        }
        _ => false,
    }
}

struct Probe {
    schema: Tool,
    failed: bool,
    text: String,
    details: Value,
    executions: AtomicUsize,
}

impl Probe {
    fn new(failed: bool, text: &str, details: Value) -> Self {
        Self {
            schema: Tool {
                name: "fixture_probe".into(),
                description: "Run one fixture probe.".into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
            failed,
            text: text.into(),
            details,
            executions: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl AgentTool for Probe {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "fixture_probe"
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        self.executions.fetch_add(1, Ordering::SeqCst);
        if self.failed {
            return Err(AgentError::Tool(self.text.clone()));
        }
        let mut result = AgentToolResult::text(&self.text);
        result.details = self.details.clone();
        Ok(result)
    }
}

fn run_case(failed: bool, result_text: &str, details: Value) -> (Arc<TaskOutcome>, usize) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    runtime.block_on(async {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let provider = Provider::new(vec![
            call("probe-1", "fixture_probe", json!({})),
            write_answer("correct"),
            text("Submitted."),
        ]);
        let probe = Arc::new(Probe::new(failed, result_text, details));
        let mut host = Host::new(&root, &provider, Fault::None);
        host.read_tool = Some(probe.clone());
        let host = Arc::new(host);
        let handle = start(host.spec("probe"), host.request("probe-run"), host.clone()).unwrap();
        let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), handle.wait()).await.unwrap();
        assert_eq!(probe.executions.load(Ordering::SeqCst), 1, "probe never replayed");
        (outcome, host.reviews.load(Ordering::SeqCst))
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn unknown_iff_structured_details(failed in any::<bool>(), text in 0..TEXTS.len(), detail in 0..11usize) {
        let details = details()[detail].clone();
        // A failed custom tool has no details; its text is never classified (22.3).
        let expected = !failed && reference_unknown(&details);
        let (outcome, reviews) = run_case(failed, TEXTS[text], details.clone());
        prop_assert!(!outcome.task_accepted);
        if expected {
            prop_assert_eq!(outcome.stop, TaskStop::ReconciliationRequired, "{:?} {:?}", details, outcome);
            prop_assert_eq!(reviews, 0);
            prop_assert_eq!(outcome.unknown_effects.len(), 1, "{:?}", outcome);
            let record = &outcome.unknown_effects[0];
            prop_assert_eq!(record.run_id.as_str(), "probe-run");
            prop_assert_eq!(record.native_call_id.as_str(), "probe-1");
            prop_assert_eq!(record.tool.as_str(), "fixture_probe");
        } else {
            prop_assert_eq!(outcome.stop, TaskStop::AwaitingHostAcceptance, "{:?} {:?}", details, outcome);
            prop_assert_eq!(reviews, 1);
            prop_assert!(outcome.unknown_effects.is_empty());
        }
    }
}
