//! Host Task contract, fixed OMP b91db66e task/{index,types,settings}.ts (MIT).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//! Original graph operands stay in their Native Session. These selected values
//! are an execution request, never a replacement for the original arguments.
use ara_agent::ToolOutput;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskPolicy {
    pub isolation_enabled: bool,
    pub batch_enabled: bool,
    pub effort_enabled: bool,
    pub structured: bool,
    pub eval_tools_enabled: bool,
    pub async_enabled: bool,
    pub keep_alive: bool,
    pub merge_mode: String,
    pub apply_changes: bool,
    pub backend: String,
    pub maximum_depth: f64,
    pub max_concurrency: f64,
    pub max_runtime_ms: f64,
    pub default_agent: String,
    pub plan_mode: bool,
}
impl Default for TaskPolicy {
    fn default() -> Self {
        Self {
            isolation_enabled: false, batch_enabled: true, effort_enabled: false,
            structured: false, eval_tools_enabled: true, async_enabled: true,
            keep_alive: true, merge_mode: "patch".into(), apply_changes: true,
            backend: "auto".into(), maximum_depth: 2.0, max_concurrency: 32.0,
            max_runtime_ms: 0.0, default_agent: "task".into(), plan_mode: false,
        }
    }
}
impl TaskPolicy {
    pub fn can_spawn_at_depth(&self, depth: usize) -> bool {
        self.maximum_depth < 0.0 || (depth as f64) < self.maximum_depth
    }
    pub fn parameters(&self) -> Value {
        let mut properties = json!({
            "name":{"type":"string"},
            "agent":{"type":"string"},
            "task":{"type":"string"},"solutionSpace":{"type":"string"}
        });
        let fields = properties.as_object_mut().expect("authored object");
        let agent=self.default_agent.trim();
        if !agent.is_empty()&&agent.bytes().all(|value|value.is_ascii_alphanumeric()||matches!(value,b'_'|b'-')){
            fields.insert("agent".into(),json!({"type":"string","default":agent}));
        }
        if self.isolation_enabled && !self.plan_mode {
            fields.insert("isolated".into(), json!({"type":"boolean"}));
        }
        if self.effort_enabled {
            fields.insert("effort".into(), json!({"type":"string","enum":["lo","med","hi"]}));
        }
        if self.structured {
            fields.insert("outputSchema".into(), json!({"type":["object","boolean","string","null"]}));
            fields.insert("schemaMode".into(), json!({"type":"string","enum":["permissive","strict"]}));
        }
        if self.eval_tools_enabled {
            fields.insert("tools".into(), json!({"type":"array","items":{"type":"string"}}));
        }
        // OMP agent has a default; only task and solutionSpace are required.
        let item = json!({"type":"object","properties":properties,
            "required":["task","solutionSpace"],"additionalProperties":false});
        if self.batch_enabled {
            json!({"type":"object","properties":{"context":{"type":"string"},
                "tasks":{"type":"array","items":item}},
                "required":["context","tasks"],"additionalProperties":false})
        } else { item }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskItem {
    pub name: Option<String>,
    pub agent: String,
    pub task: String,
    pub solution_space: Option<String>,
    pub output_schema: Option<Value>,
    pub schema_mode: Option<String>,
    pub tools: Option<Vec<String>>,
    pub isolated: Option<bool>,
    pub effort: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskRequest {
    pub items: Vec<TaskItem>,
    pub context: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub content: String,
    pub details: Value,
    pub is_error: bool,
}
impl TaskResult {
    pub fn into_tool_output(self) -> ToolOutput {
        let mut output = ToolOutput::text(self.content).with_details(self.details);
        output.is_error = self.is_error;
        output
    }
}
#[async_trait]
pub trait TaskExecutionPort: Send + Sync {
    /// Current Host settings, read again for schema inspection and admission.
    fn policy(&self) -> TaskPolicy;
    /// Host owns child identity, scheduler, isolation, capture and adoption.
    /// A successful Run remains evidence awaiting Task review.
    async fn execute(&self, request: TaskRequest, cancel: CancellationToken) -> anyhow::Result<TaskResult>;
}
