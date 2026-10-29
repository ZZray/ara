//! Controlled `execute` scenarios that produce every event `type` (DR-P4-1).
//! `event_samples` captured `tests/fixtures/events-v1/` from them once,
//! before `LiteEvent` existed; `event_compat` reruns them and compares the
//! event count, order, `type` values and field-name sets with the golden.
//! Loopback provider only; no real model or OS effect.
#![allow(dead_code)]

use std::{
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use ara_agent_lite_core::{
    compaction_refresh::{FileToolKind, RefreshPolicy},
    completion_gate::{CompletionGate, GateRequest, GateVerdict, Registration},
    model::{CatalogModelHost, GenerationPolicy, LengthRecoveryPolicy, ModelHost, ModelSelection, Route},
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    stuck_detector::StuckPolicy,
    RunMode, RunRequest,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::support::*;

/// Scenario names; each is one sample file `<name>.jsonl`.
pub const SCENARIOS: &[&str] = &[
    "completion",
    "tools",
    "generation_policy",
    "compaction",
    "compaction_refresh",
    "length_replan",
    "length_stop_evidence",
    "closed_budget",
    "goal",
    "completion_gate",
    "stuck_detector",
    "output_limit",
    "summary_budget_stop",
];

/// Every event `type` the Core emitted before `LiteEvent` (DR-P4-1).
pub const EVENT_TYPES: &[&str] = &[
    "agent_start",
    "agent_end",
    "text_delta",
    "text",
    "tool_start",
    "tool_end",
    "model_capabilities",
    "generation_policy",
    "model_call",
    "model_result",
    "context_status",
    "compact",
    "length_recovery",
    "goal_status",
    "goal_update",
    "task_update",
    "completion_gate",
    "stuck_detector",
    "compaction_refresh",
];

/// Run 1 leaves ~16 KiB; a ~16 KiB Run 2 objective then needs a summary.
const CONTEXT_WINDOW: u64 = 60_000 + 512 + 4096;

/// Catalog-resolving Host with fixed pure policies.
struct PolicyHost {
    generation: GenerationPolicy,
    length: LengthRecoveryPolicy,
    ceiling: Option<u32>,
}

impl PolicyHost {
    fn new() -> Self {
        Self { generation: GenerationPolicy::ProviderDefault, length: LengthRecoveryPolicy::Stop, ceiling: None }
    }
}

#[async_trait]
impl ModelHost for PolicyHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        CatalogModelHost.resolve(route, endpoint, token, run).await
    }
    fn generation_policy(&self, _: &Route) -> Result<GenerationPolicy> {
        Ok(self.generation)
    }
    fn length_recovery_policy(&self, _: &Route) -> Result<LengthRecoveryPolicy> {
        Ok(self.length)
    }
    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(self.ceiling)
    }
}

/// Business tool returning fixed structured details.
struct DetailTool {
    schema: Tool,
    details: Value,
}

impl DetailTool {
    fn new(name: &str, details: Value) -> Arc<Self> {
        Arc::new(Self {
            schema: Tool {
                name: name.into(),
                description: format!("Controlled fixture tool {name}."),
                parameters: Schema(json!({"type":"object","additionalProperties":true})),
                constrained_sampling: None,
            },
            details,
        })
    }
}

#[async_trait]
impl AgentTool for DetailTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        _: &str,
        _: Value,
        _: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let mut result = AgentToolResult::text("fixture effect state unknown");
        result.details = self.details.clone();
        Ok(result)
    }
}

/// Fails the first `pass_after` checks, passes later ones.
struct FlakyGate {
    checks: AtomicUsize,
    pass_after: usize,
}

#[async_trait]
impl CompletionGate for FlakyGate {
    async fn check(&self, _: GateRequest, _: CancellationToken) -> Result<GateVerdict> {
        let number = self.checks.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(if number > self.pass_after {
            GateVerdict::Passed
        } else {
            GateVerdict::Failed { feedback: format!("public check {number} failed: 2 of 7 cases") }
        })
    }
}

fn gate(pass_after: usize) -> Registration {
    let gate = Arc::new(FlakyGate { checks: AtomicUsize::new(0), pass_after });
    Registration { check_id: "public-checks".into(), gate, timeout: Some(Duration::from_secs(5)) }
}

fn probe() -> Arc<ScriptedTool> {
    ScriptedTool::new("probe", |_| Ok("probe state".into()))
}

/// Replace the temp root in every string so samples hold no local path.
fn scrub(value: Value, roots: &[String]) -> Value {
    match value {
        Value::String(text) => {
            Value::String(roots.iter().fold(text, |text, root| text.replace(root.as_str(), "%ROOT%")))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(|item| scrub(item, roots)).collect()),
        Value::Object(map) => Value::Object(map.into_iter().map(|(key, item)| (key, scrub(item, roots))).collect()),
        other => other,
    }
}

fn root_forms(root: &Path) -> Vec<String> {
    let display = root.display().to_string();
    let plain = display.trim_start_matches(r"\\?\").to_owned();
    let mut forms = vec![display, plain];
    forms.dedup();
    forms
}

/// Run every request of a scenario in order; returns the events of each Run.
async fn runs(root: &TempDir, requests: Vec<RunRequest>) -> Vec<Vec<Value>> {
    let roots = root_forms(root.path());
    let mut all = Vec::new();
    for request in requests {
        let (_, events) = run(request).await;
        all.push(events.into_iter().map(|event| scrub(event, &roots)).collect());
    }
    all
}

/// Events of each Run of `name`, in emission order.
pub async fn scenario(name: &str) -> Vec<Vec<Value>> {
    let root = TempDir::new();
    let dir = root.path();
    match name {
        "completion" => {
            let provider = Provider::new(vec![text("done")]);
            runs(&root, vec![request(dir, &provider, &[], 2, "Report done.")]).await
        }
        "tools" => {
            let provider = Provider::new(vec![
                calls(&[("p1", "probe", json!({"n":1})), ("f1", "broken", json!({})), ("u1", "effect", json!({}))]),
                call("t1", "task_update", json!({"id":"s1","title":"probe state","status":"done","evidence":"p1 ok"})),
                call("t2", "task_update", json!({"id":"s2","title":"report","status":"running"})),
                text("tools done"),
            ]);
            let broken = ScriptedTool::new("broken", |_| Err("fixture failure".into()));
            let mut request = request(dir, &provider, &[probe(), broken], 6, "Use the tools and report.");
            let effect = DetailTool::new("effect", json!({"executed":"unknown","source":"fixture"}));
            request.extra_tools.push(ara_agent_lite_core::ToolRegistration { tool: effect, allow_in_plan: true });
            runs(&root, vec![request]).await
        }
        "generation_policy" => {
            let provider = Provider::new(vec![text("done")]);
            let mut request = request(dir, &provider, &[], 2, "Report done.");
            request.model_host =
                Arc::new(PolicyHost { generation: GenerationPolicy::ThinkingTypeDisabled, ..PolicyHost::new() });
            runs(&root, vec![request]).await
        }
        "compaction" => {
            let provider = Provider::with(ProviderConfig {
                context_window: CONTEXT_WINDOW,
                ..ProviderConfig::new(vec![text(&"a".repeat(16 * 1024)), text("continued")])
            });
            let first = request(dir, &provider, &[], 1, "Write the long report.");
            let second = request(dir, &provider, &[], 3, &format!("Continue. {}", "q".repeat(16 * 1024)));
            runs(&root, vec![first, second]).await
        }
        "compaction_refresh" => {
            let write_root = dir.to_path_buf();
            let write = ScriptedTool::new("ws_write_file", move |args| {
                std::fs::write(write_root.join(args["path"].as_str().unwrap()), args["content"].as_str().unwrap())
                    .map_err(|error| error.to_string())?;
                Ok("written".into())
            });
            let provider = Provider::with(ProviderConfig {
                context_window: CONTEXT_WINDOW,
                ..ProviderConfig::new(vec![
                    call("w1", "ws_write_file", json!({"path":"a.txt","content":"a-v1"})),
                    call("t1", "task_update", json!({"id":"s1","title":"write a","status":"done","evidence":"w1"})),
                    text(&"r".repeat(16 * 1024)),
                    text("continued from disk"),
                ])
            });
            let tools = [write];
            let first = request(dir, &provider, &tools, 4, "Write a.txt, then report.");
            let mut second = request(dir, &provider, &tools, 3, &format!("Continue. {}", "q".repeat(16 * 1024)));
            second.policies.compaction_refresh = Some(RefreshPolicy {
                max_files: 4,
                max_bytes: 16 * 1024,
                file_tools: [("ws_write_file".to_owned(), FileToolKind::Write)].into(),
            });
            runs(&root, vec![first, second]).await
        }
        "length_replan" => {
            let provider = Provider::new(vec![length("partial answer"), text("replanned answer")]);
            let mut request = request(dir, &provider, &[], 3, "Answer briefly.");
            request.model_host = Arc::new(PolicyHost { length: LengthRecoveryPolicy::ReplanOnce, ..PolicyHost::new() });
            runs(&root, vec![request]).await
        }
        "length_stop_evidence" => {
            let provider = Provider::new(vec![length("partial answer")]);
            let mut request = request(dir, &provider, &[], 2, "Answer briefly.");
            request.policies.length_stop_evidence = true;
            runs(&root, vec![request]).await
        }
        "closed_budget" => {
            let provider = Provider::new(vec![call("c1", "probe", json!({}))]);
            runs(&root, vec![request(dir, &provider, &[probe()], 1, "Use probe and report.")]).await
        }
        "goal" => {
            let provider = Provider::new(vec![
                call(
                    "g1",
                    "goal_update",
                    json!({"steps":[{"name":"inspect","status":"done","evidence":"probe state observed"}]}),
                ),
                text("goal slice done"),
            ]);
            let mut first = request(dir, &provider, &[], 3, "Reach the fixture goal.");
            first.mode = RunMode::Goal;
            first.goal_criteria = vec!["probe state reported".into()];
            let mut status = request(dir, &provider, &[], 1, "/goal status");
            status.mode = RunMode::Goal;
            runs(&root, vec![first, status]).await
        }
        "completion_gate" => {
            let provider = Provider::new(vec![text("done"), text("fixed"), text("done again")]);
            let mut continued = request(dir, &provider, &[], 4, "Report done.");
            continued.policies.completion_gate = Some(gate(1));
            let mut exhausted = request(dir, &provider, &[], 1, "Report done again.");
            exhausted.policies.completion_gate = Some(gate(usize::MAX));
            runs(&root, vec![continued, exhausted]).await
        }
        "stuck_detector" => {
            let provider = Provider::new((1..=4).map(|n| call(&format!("c{n}"), "probe", json!({}))).collect());
            let mut request = request(dir, &provider, &[probe()], 8, "Use probe and report.");
            request.policies.stuck = Some(StuckPolicy { repeated_call: Some(2), ..StuckPolicy::default() });
            runs(&root, vec![request]).await
        }
        "output_limit" => {
            let provider = Provider::new(vec![text("done")]);
            let mut request = request(dir, &provider, &[], 2, "Report done.");
            request.max_output = 2048;
            request.model_host = Arc::new(PolicyHost { ceiling: Some(1024), ..PolicyHost::new() });
            runs(&root, vec![request]).await
        }
        "summary_budget_stop" => {
            // A Host note queued during Run 2's only agent request continues
            // the Run, whose next turn needs a summary no call can pay for.
            let steering = Arc::new(std::sync::Mutex::new(ara_agent_lite_core::steering::SteeringBuffer::default()));
            let (hook, agent_requests) = (steering.clone(), Arc::new(AtomicUsize::new(0)));
            let provider = Provider::with(ProviderConfig {
                context_window: CONTEXT_WINDOW,
                on_request: Box::new(move |body| {
                    if !is_summary(body) && agent_requests.fetch_add(1, Ordering::SeqCst) == 1 {
                        let _ = hook.lock().unwrap().submit_legacy("Host note: also report the size.".into());
                    }
                }),
                ..ProviderConfig::new(vec![text(&"a".repeat(16 * 1024)), text(&"b".repeat(16 * 1024))])
            });
            let first = request(dir, &provider, &[], 1, "Write the report.");
            let mut second = request(dir, &provider, &[], 1, "Answer the follow-up.");
            second.steering = steering;
            runs(&root, vec![first, second]).await
        }
        other => panic!("unknown scenario {other}"),
    }
}

/// Golden shape of one Run: `type` and sorted top-level field names per event.
pub fn shape(events: &[Value]) -> Value {
    Value::Array(
        events
            .iter()
            .map(|event| {
                let object = event.as_object().expect("event is a JSON object");
                let mut fields: Vec<&String> = object.keys().collect();
                fields.sort();
                json!({"type": event["type"], "fields": fields})
            })
            .collect(),
    )
}

pub fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/events-v1")
}
