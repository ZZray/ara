//! Run_Policy (requirement 3.2, 3.7; design “Run_Policy 与探测”).
//!
//! The policy is read only from the fixed repository path [`RUN_POLICY_PATH`].
//! There is deliberately no command-line or environment override for any
//! item. A missing, unparsable or incomplete file is reported with every
//! missing item before anything else happens. `credential_env` is only the
//! name of the environment variable that holds the model key; the key itself
//! never appears in the policy.

use std::{collections::BTreeMap, fmt, fs, path::Path, process::Command};

use ara_agent_lite_harness::core::model::{GenerationPolicy, LengthRecoveryPolicy};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::common::sha256_hex;

/// Repository-relative path of the Run_Policy.
pub const RUN_POLICY_PATH: &str = "tools/lite-eval/run-policy.json";

/// Items requirement 3.2 needs (JSON pointer form). `budgets.<task>` is
/// checked per selected task by [`RunPolicy::budget`].
pub const REQUIRED_ITEMS: [&str; 17] = [
    "/version",
    "/policy_id",
    "/model/id",
    "/model/protocol",
    "/model/route/endpoint",
    "/model/route/binding",
    "/model/route/decision",
    "/model/resolution",
    "/model/credential_env",
    "/probe/timeout_ms",
    "/budgets",
    "/generation/max_output",
    "/generation/generation_policy",
    "/generation/length_recovery",
    "/price",
    "/oracle_timeout_seconds",
    "/consumer_task",
];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPolicy {
    pub version: u32,
    pub policy_id: String,
    pub model: ModelPolicy,
    pub probe: ProbePolicy,
    /// Task ID (or `LONG-COMPACT`, `LONG-TRIAL`) to its budget.
    pub budgets: BTreeMap<String, TaskBudget>,
    pub generation: GenerationSettings,
    pub price: Price,
    pub oracle_timeout_seconds: u64,
    /// Task the out-of-tree consumer runs (requirement 31).
    pub consumer_task: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPolicy {
    pub id: String,
    pub protocol: String,
    pub route: RoutePolicy,
    /// How the model is resolved; only `catalog` (`GET /models`) is supported.
    pub resolution: String,
    /// Name of the environment variable holding the credential, never its value.
    pub credential_env: String,
    /// Other `model` values the route is known to echo for `id` (DR-P0-3:
    /// CAS answers `deepseek-flash`). An echo listed here passes the probe's
    /// model check but is recorded as an unverified alias, not as identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accepted_echo: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutePolicy {
    /// OpenAI-compatible base URL, for example `https://cas.ciqtek.com/v1`.
    pub endpoint: String,
    pub binding: String,
    pub decision: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbePolicy {
    pub timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskBudget {
    pub max_runs: usize,
    pub max_calls: usize,
    /// `deadline_unix_ms = now + deadline_seconds × 1000` at task start.
    pub deadline_seconds: u64,
    pub run_max_calls: usize,
    pub run_max_seconds: u64,
    /// Per-task output cap below `generation.max_output` (LONG-TRIAL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_output: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerationSettings {
    pub max_output: u32,
    /// Also the reasoning setting: `thinking_type_disabled` turns reasoning off.
    pub generation_policy: GenerationPolicy,
    pub length_recovery: LengthRecoveryPolicy,
}

/// Unit prices; `null` means unknown and makes every cost unknown (4.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub currency: Option<String>,
    pub input_per_million: Option<f64>,
    pub output_per_million: Option<f64>,
}

impl RunPolicy {
    /// Budget of `task_id`; a selected task without one is a missing item (3.7).
    pub fn budget(&self, task_id: &str) -> Result<&TaskBudget, PolicyError> {
        self.budgets.get(task_id).ok_or_else(|| PolicyError::Missing { items: vec![format!("/budgets/{task_id}")] })
    }
}

/// Git state of the policy file (DD-19): recorded, not enforced.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GitState {
    TrackedClean,
    TrackedModified,
    Untracked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolicyIdentity {
    pub path: &'static str,
    /// SHA-256 of the file bytes as read.
    pub sha256: String,
    pub git_state: GitState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    Unreadable {
        reason: String,
    },
    Unparsable {
        reason: String,
    },
    /// Every missing item, as JSON pointers.
    Missing {
        items: Vec<String>,
    },
    /// Present but unusable (wrong type, unsupported value, zero budget).
    Invalid {
        reason: String,
    },
    GitState {
        reason: String,
    },
}

impl fmt::Display for PolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { reason } => {
                write!(formatter, "Run_Policy {RUN_POLICY_PATH} is missing or unreadable: {reason}")
            }
            Self::Unparsable { reason } => {
                write!(formatter, "Run_Policy {RUN_POLICY_PATH} is not valid JSON: {reason}")
            }
            Self::Missing { items } => {
                write!(formatter, "Run_Policy {RUN_POLICY_PATH} lacks required items: {}", items.join(", "))
            }
            Self::Invalid { reason } => write!(formatter, "Run_Policy {RUN_POLICY_PATH} is invalid: {reason}"),
            Self::GitState { reason } => {
                write!(formatter, "cannot determine the git state of Run_Policy {RUN_POLICY_PATH}: {reason}")
            }
        }
    }
}

impl std::error::Error for PolicyError {}

/// Read the Run_Policy of the repository at `repo_root`. The only input is
/// the fixed path; nothing else can change a policy item.
pub fn load_fixed(repo_root: &Path) -> Result<(RunPolicy, PolicyIdentity), PolicyError> {
    let path = repo_root.join(RUN_POLICY_PATH);
    let bytes = fs::read(&path).map_err(|error| PolicyError::Unreadable { reason: error.to_string() })?;
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|error| PolicyError::Unparsable { reason: error.to_string() })?;
    let missing: Vec<String> = REQUIRED_ITEMS
        .iter()
        .filter(|pointer| value.pointer(pointer).is_none_or(Value::is_null))
        .map(|pointer| (*pointer).to_owned())
        .collect();
    if !missing.is_empty() {
        return Err(PolicyError::Missing { items: missing });
    }
    let policy: RunPolicy =
        serde_json::from_value(value).map_err(|error| PolicyError::Invalid { reason: error.to_string() })?;
    let invalid = |reason: String| Err(PolicyError::Invalid { reason });
    if policy.version != 1 {
        return invalid(format!("unsupported version {}", policy.version));
    }
    if !matches!(policy.model.protocol.as_str(), "openai-completions" | "openrouter-chat") {
        return invalid(format!("protocol `{}` is not an OpenAI-compatible chat protocol", policy.model.protocol));
    }
    if policy.model.resolution != "catalog" {
        return invalid(format!("model resolution `{}` is not supported; use `catalog`", policy.model.resolution));
    }
    let name = &policy.model.credential_env;
    if name.is_empty() || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
        return invalid("credential_env must be an environment variable name".into());
    }
    if policy.probe.timeout_ms == 0 || policy.generation.max_output == 0 || policy.oracle_timeout_seconds == 0 {
        return invalid("probe.timeout_ms, generation.max_output and oracle_timeout_seconds must be positive".into());
    }
    for (task, budget) in &policy.budgets {
        let positive = budget.max_runs > 0
            && budget.max_calls > 0
            && budget.deadline_seconds > 0
            && budget.run_max_calls > 0
            && budget.run_max_seconds > 0
            && budget.max_output != Some(0);
        if !positive {
            return invalid(format!("budgets.{task} must have positive values"));
        }
    }
    let identity =
        PolicyIdentity { path: RUN_POLICY_PATH, sha256: sha256_hex(&bytes), git_state: git_state(repo_root)? };
    Ok((policy, identity))
}

fn git_state(repo_root: &Path) -> Result<GitState, PolicyError> {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(repo_root)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(args)
            .output()
            .map_err(|error| PolicyError::GitState { reason: error.to_string() })?;
        if !output.status.success() {
            return Err(PolicyError::GitState { reason: String::from_utf8_lossy(&output.stderr).trim().to_owned() });
        }
        Ok(output.stdout)
    };
    let tracked = !git(&["ls-files", "--", RUN_POLICY_PATH])?.is_empty();
    if !tracked {
        return Ok(GitState::Untracked);
    }
    let changed = !git(&["status", "--porcelain", "--", RUN_POLICY_PATH])?.is_empty();
    Ok(if changed { GitState::TrackedModified } else { GitState::TrackedClean })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        serde_json::json!({
            "version": 1, "policy_id": "p",
            "model": {"id": "m", "protocol": "openai-completions",
                      "route": {"endpoint": "http://127.0.0.1:9/v1", "binding": "b", "decision": "d"},
                      "resolution": "catalog", "credential_env": "LITE_EVAL_TEST_KEY"},
            "probe": {"timeout_ms": 1000},
            "budgets": {"S1": {"max_runs": 1, "max_calls": 2, "deadline_seconds": 60, "run_max_calls": 2, "run_max_seconds": 30}},
            "generation": {"max_output": 4096, "generation_policy": "thinking_type_disabled", "length_recovery": "stop"},
            "price": {"currency": null, "input_per_million": null, "output_per_million": null},
            "oracle_timeout_seconds": 600, "consumer_task": "S1"
        })
    }

    fn write(root: &Path, value: &Value) {
        let path = root.join(RUN_POLICY_PATH);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
    }

    #[test]
    fn missing_items_are_all_named_and_unknown_fields_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load_fixed(dir.path()), Err(PolicyError::Unreadable { .. })));
        fs::create_dir_all(dir.path().join("tools/lite-eval")).unwrap();
        fs::write(dir.path().join(RUN_POLICY_PATH), b"{").unwrap();
        assert!(matches!(load_fixed(dir.path()), Err(PolicyError::Unparsable { .. })));

        let mut value = sample();
        value["model"].as_object_mut().unwrap().remove("credential_env");
        value["probe"] = Value::Null;
        value.as_object_mut().unwrap().remove("price");
        write(dir.path(), &value);
        match load_fixed(dir.path()) {
            Err(PolicyError::Missing { items }) => {
                assert_eq!(items, ["/model/credential_env", "/probe/timeout_ms", "/price"]);
            }
            other => panic!("{other:?}"),
        }
        let mut value = sample();
        value["override"] = serde_json::json!(true);
        write(dir.path(), &value);
        assert!(matches!(load_fixed(dir.path()), Err(PolicyError::Invalid { .. })));
        let mut value = sample();
        value["budgets"]["S1"]["max_calls"] = serde_json::json!(0);
        write(dir.path(), &value);
        assert!(matches!(load_fixed(dir.path()), Err(PolicyError::Invalid { .. })));
    }

    #[test]
    fn a_complete_policy_parses_and_names_missing_budgets() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), &sample());
        // Not a git repository: the git state cannot be determined.
        assert!(matches!(load_fixed(dir.path()), Err(PolicyError::GitState { .. })));
        let git =
            |args: &[&str]| assert!(Command::new("git").current_dir(dir.path()).args(args).status().unwrap().success());
        git(&["init", "-q"]);
        let (policy, identity) = load_fixed(dir.path()).unwrap();
        assert_eq!(identity.git_state, GitState::Untracked);
        assert_eq!(identity.sha256, sha256_hex(&fs::read(dir.path().join(RUN_POLICY_PATH)).unwrap()));
        assert_eq!(policy.generation.generation_policy, GenerationPolicy::ThinkingTypeDisabled);
        assert_eq!(policy.price.input_per_million, None);
        assert_eq!(policy.budget("S1").unwrap().max_calls, 2);
        assert_eq!(policy.budget("S2"), Err(PolicyError::Missing { items: vec!["/budgets/S2".into()] }));
        git(&["add", RUN_POLICY_PATH]);
        assert_eq!(load_fixed(dir.path()).unwrap().1.git_state, GitState::TrackedModified);
    }
}
