use std::path::PathBuf;

use ara_agent_lite_harness::{
    core::{
        model::{GenerationPolicy, LengthRecoveryPolicy},
        model_caps::ModelCaps,
    },
    BudgetStopReviewPolicy, LengthStopReviewPolicy, ResumeReceipt, ReviewReceipt, Settlement, TaskKind, TaskSpec,
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
    /// Opt-in Host review of a closed Length stop (16.1, 16.2); Execute only.
    #[serde(default)]
    pub length_stop_review: LengthStopReviewPolicy,
    #[serde(default = "grace")]
    pub cancel_grace_ms: u64,
    #[serde(default)]
    pub emit_events: bool,
    /// Claude Code `mcpServers` map (42.9); parsed by `ara-agent-lite-mcp`.
    #[serde(default)]
    pub mcp_servers: Option<Value>,
    #[serde(default)]
    pub mcp_options: Option<McpStartOptions>,
    /// Resume the fenced Task of this Session (9.1). Absent keeps the
    /// original start path; `null` is a type error, not an absent field.
    #[serde(default, deserialize_with = "present")]
    pub resume: Option<ResumeStart>,
}

fn grace() -> u64 {
    5000
}

fn present<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<ResumeStart>, D::Error> {
    ResumeStart::deserialize(deserializer).map(Some)
}

/// The fence the Host inspected; `validate` requires version 1 and 64
/// lowercase hex digits (9.3).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeStart {
    pub version: u32,
    pub fence_sha256: String,
}

/// Host connection policy for `mcp_servers`; it grants no other authority.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpStartOptions {
    /// Servers whose failure rejects the start frame.
    #[serde(default)]
    pub required: Vec<String>,
    #[serde(default)]
    pub startup_timeout_ms: Option<u64>,
    #[serde(default)]
    pub call_timeout_ms: Option<u64>,
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
    HostDeclaredLoopback {
        capabilities: Caps,
    },
    /// Experimental: catalog capabilities with the context window lowered to
    /// at most `context_window_cap` tokens, so a long Task can reach compaction
    /// on a model that declares a very large window. The range is checked at
    /// start validation.
    CatalogCapped {
        context_window_cap: u64,
    },
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
    VerifyStopReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        receipt: ReviewReceipt,
    },
    PrepareStopReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        run_id_next: String,
        objective: String,
    },
    /// Host settlement of a fenced Task (10.3); only sent after a `reconcile_request`.
    ReconcileReply {
        version: u32,
        rpc_id: String,
        task_id: String,
        run_id: String,
        receipt: ResumeReceipt,
    },
}

/// One stdin frame as the typed message and its raw value. Deserializing the
/// typed frame directly as well: deriving from Value alone would erase
/// duplicate JSON object keys.
pub fn decode(bytes: &[u8]) -> Option<(Incoming, Value)> {
    let message: Incoming = serde_json::from_slice(bytes).ok()?;
    let value: Value = serde_json::from_slice(bytes).ok()?;
    Some((message, value))
}

#[derive(Clone, Debug, Serialize)]
pub struct Reference {
    pub path: PathBuf,
    pub sha256: String,
}

#[cfg(test)]
mod tests {
    use super::Start;
    use ara_agent_lite_harness::core::output_limit::admission_bound;
    use proptest::prelude::*;
    use serde_json::{json, Value};

    /// Field mutations of an otherwise valid frame, and whether each keeps it admissible.
    const MUTATIONS: [(&str, bool); 16] = [
        ("none", true),
        ("unknown_top_field", false),
        ("unknown_run_field", false),
        ("ceiling_wrong_type", false),
        ("length_review_in_plan", false),
        ("length_review_in_execute", true),
        ("mcp_options_without_servers", false),
        ("cancel_grace_out_of_range", false),
        ("resume_valid", true),
        ("resume_wrong_type", false),
        ("resume_null", false),
        ("resume_missing_field", false),
        ("resume_unknown_field", false),
        ("resume_version_2", false),
        ("resume_uppercase_hex", false),
        ("resume_short_hex", false),
    ];
    const FENCE: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn frame(ceiling: Option<u32>, max_output: u64, mutation: &str) -> Value {
        let root = std::env::temp_dir();
        let mut frame = json!({"kind":"start_task","version":1,
            "task":{"task_id":"prop-task","criteria_revision":"c1","authorization_revision":"a1","kind":"Coding",
                "evidence_dir":root.join("ara-jsonl-prop-evidence"),"max_runs":1,"max_calls":1,
                "deadline_unix_ms":4_102_444_800_000u64},
            "run":{"run_id":"run-1","objective":"Admission only.","cwd":root,
                "session_path":root.join("ara-jsonl-prop-session.json"),"environment_state":root.join("ara-jsonl-prop-env"),
                "device_fp":"prop","session_key":"prop/session","mode":"execute","endpoint":"http://127.0.0.1:9/v1",
                "model":"prop-model","max_calls":1,"max_seconds":1,"max_output":max_output,
                "model_resolution":{"kind":"catalog"},"tools":[]}});
        if let Some(ceiling) = ceiling {
            frame["run"]["host_output_ceiling"] = json!(ceiling);
        }
        match mutation {
            "unknown_top_field" => frame["unexpected"] = json!(true),
            "unknown_run_field" => frame["run"]["unexpected"] = json!(1),
            "ceiling_wrong_type" => frame["run"]["host_output_ceiling"] = json!("8192"),
            "length_review_in_plan" => {
                frame["length_stop_review"] = json!("review_closed_length");
                frame["run"]["mode"] = json!("plan");
            }
            "length_review_in_execute" => frame["length_stop_review"] = json!("review_closed_length"),
            "mcp_options_without_servers" => frame["mcp_options"] = json!({"required":[]}),
            "cancel_grace_out_of_range" => frame["cancel_grace_ms"] = json!(999),
            "resume_valid" => frame["resume"] = json!({"version":1,"fence_sha256":FENCE}),
            "resume_wrong_type" => frame["resume"] = json!(FENCE),
            "resume_null" => frame["resume"] = Value::Null,
            "resume_missing_field" => frame["resume"] = json!({"version":1}),
            "resume_unknown_field" => frame["resume"] = json!({"version":1,"fence_sha256":FENCE,"task_id":"x"}),
            "resume_version_2" => frame["resume"] = json!({"version":2,"fence_sha256":FENCE}),
            "resume_uppercase_hex" => frame["resume"] = json!({"version":1,"fence_sha256":FENCE.to_uppercase()}),
            "resume_short_hex" => frame["resume"] = json!({"version":1,"fence_sha256":&FENCE[1..]}),
            _ => {}
        }
        frame
    }

    fn admitted(frame: &Value) -> bool {
        serde_json::from_value::<Start>(frame.clone()).is_ok_and(|start| crate::runtime::validate(&start).is_ok())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// Property 18 (9.3, 15.5, 15.6): a frame is admitted iff it matches the
        /// schema and `max_output ∈ 1..=admission_bound(host_output_ceiling)`.
        #[test]
        fn start_frame_admission(
            ceiling in prop_oneof![Just(None), Just(Some(0u32)), (1u32..=200_000).prop_map(Some)],
            selector in 0u8..3,
            raw in any::<u32>(),
            mutation in 0usize..MUTATIONS.len(),
        ) {
            let bound = admission_bound(ceiling);
            let max_output = match selector {
                0 => 0,
                1 if bound > 0 => 1 + u64::from(raw) % bound,
                1 => 1,
                _ => bound + 1 + u64::from(raw % 1000),
            };
            let (name, schema_ok) = MUTATIONS[mutation];
            let expected = schema_ok
                && ceiling != Some(0)
                && max_output <= u64::from(u32::MAX)
                && (1..=bound).contains(&max_output);
            prop_assert_eq!(admitted(&frame(ceiling, max_output, name)), expected, "{} {:?} {}", name, ceiling, max_output);
        }

        /// Property 18 (9.3): a `resume` object is admitted iff its version is 1
        /// and `fence_sha256` is exactly 64 lowercase hex digits.
        #[test]
        fn resume_start_admission(version in prop_oneof![Just(1u32), any::<u32>()], fence in prop_oneof!["[0-9a-f]{64}", "[0-9a-fA-Fg]{0,70}"]) {
            let mut start = frame(None, 512, "none");
            start["resume"] = json!({"version":version,"fence_sha256":fence});
            let expected = version == 1
                && fence.len() == 64
                && fence.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            prop_assert_eq!(admitted(&start), expected, "{} {}", version, fence);
        }
    }

    #[test]
    fn host_output_ceiling_zero_is_rejected_even_with_a_small_max_output() {
        assert!(admitted(&frame(None, 1, "none")));
        assert!(!admitted(&frame(Some(0), 1, "none")));
        assert!(admitted(&frame(Some(16), 16, "none")));
        assert!(!admitted(&frame(Some(16), 17, "none")));
    }

    fn capped(resolution: Value) -> Value {
        let mut start = frame(None, 512, "none");
        start["run"]["model_resolution"] = resolution;
        start
    }

    #[test]
    fn catalog_capped_is_admitted_only_within_the_cap_range() {
        for cap in [1024u64, 98_304, 131_072, 10_000_000] {
            let start = capped(json!({"kind":"catalog_capped","context_window_cap":cap}));
            assert!(admitted(&start), "{cap}");
        }
        for cap in [0u64, 1, 1023, 10_000_001, u64::MAX] {
            let start: Start =
                serde_json::from_value(capped(json!({"kind":"catalog_capped","context_window_cap":cap}))).unwrap();
            let error = crate::runtime::validate(&start).expect_err("out of range");
            assert_eq!(error.to_string(), "context_window_cap_out_of_range", "{cap}");
        }
        // Not a schema match at all: wrong type, negative, missing, unknown fields.
        for resolution in [
            json!({"kind":"catalog_capped","context_window_cap":"131072"}),
            json!({"kind":"catalog_capped","context_window_cap":-1}),
            json!({"kind":"catalog_capped","context_window_cap":131072.5}),
            json!({"kind":"catalog_capped"}),
            json!({"kind":"catalog_capped","context_window_cap":131072,"extra":1}),
        ] {
            assert!(serde_json::from_value::<Start>(capped(resolution.clone())).is_err(), "{resolution}");
        }
    }
}
