use std::path::PathBuf;

use ara_agent_lite_harness::{
    core::{
        model::{GenerationPolicy, LengthRecoveryPolicy},
        model_caps::ModelCaps,
    },
    BudgetStopReviewPolicy, ReviewReceipt, Settlement, TaskKind, TaskSpec,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FRAME_BYTES: usize = 256 * 1024;
pub const TOOL_RESULT_BYTES: usize = 128 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Start {
    pub kind: String,
    pub version: u32,
    pub task: TaskConfig,
    pub run: RunConfig,
    #[serde(default)]
    pub budget_stop_review: BudgetStopReviewPolicy,
    #[serde(default = "grace")]
    pub cancel_grace_ms: u64,
    #[serde(default)]
    pub emit_events: bool,
}

fn grace() -> u64 {
    5000
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskConfig {
    pub task_id: String,
    pub criteria_revision: String,
    pub authorization_revision: String,
    pub kind: TaskKind,
    pub evidence_dir: PathBuf,
    pub max_runs: usize,
    pub max_calls: usize,
    pub deadline_unix_ms: u64,
}

impl From<TaskConfig> for TaskSpec {
    fn from(value: TaskConfig) -> Self {
        Self {
            task_id: value.task_id,
            criteria_revision: value.criteria_revision,
            authorization_revision: value.authorization_revision,
            kind: value.kind,
            evidence_dir: value.evidence_dir,
            max_runs: value.max_runs,
            max_calls: value.max_calls,
            deadline_unix_ms: value.deadline_unix_ms,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunConfig {
    pub run_id: String,
    pub objective: String,
    pub cwd: PathBuf,
    pub session_path: PathBuf,
    pub environment_state: PathBuf,
    pub device_fp: String,
    pub session_key: String,
    pub mode: String,
    pub endpoint: String,
    pub model: String,
    pub max_calls: usize,
    pub max_seconds: u64,
    pub max_output: u32,
    /// Optional Host output ceiling (15.5); absent keeps the builtin 8192 rule.
    #[serde(default)]
    pub host_output_ceiling: Option<u32>,
    pub model_resolution: Resolution,
    #[serde(default)]
    pub generation_policy: GenerationPolicy,
    #[serde(default)]
    pub length_recovery: LengthRecoveryPolicy,
    pub tools: Vec<ToolConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Resolution {
    Catalog,
    HostDeclaredLoopback { capabilities: Caps },
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Caps {
    pub context_window: Option<u64>,
    pub max_tokens: u64,
    pub input: Option<Vec<String>>,
}

impl Caps {
    pub fn model_caps(&self) -> ModelCaps {
        ModelCaps { context_window: self.context_window, max_tokens: self.max_tokens, input: self.input.clone() }
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolConfig {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default)]
    pub allow_in_plan: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolReply {
    pub value: Value,
    pub is_error: bool,
    pub settlement: Settlement,
    pub operation_receipt: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Incoming {
    Cancel {
        version: u32,
        task_id: String,
    },
    ToolReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        native_call_id: String,
        tool_name: String,
        args_sha256: String,
        result: ToolReply,
    },
    VerifyReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        receipt: ReviewReceipt,
    },
    PrepareReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        run_id_next: String,
        objective: String,
    },
    VerifyBudgetReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        receipt: ReviewReceipt,
    },
    PrepareBudgetReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        run_id_next: String,
        objective: String,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub path: PathBuf,
    pub sha256: String,
}
