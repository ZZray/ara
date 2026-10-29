//! Stability_Test_Crate for requirement 29 (tasks 35.2, 57.1): every Core item that
//! `docs/stability.md` marks stable is used once through its public path, so
//! deleting, renaming or retyping one of them fails to compile (29.5). The
//! test also parses the document and checks that its stable set is exactly
//! the set used here (29.3) and that every `RunRequest` field is classified
//! once (29.1).
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ara_agent_lite_core::{
    model::{CatalogModelHost, GenerationPolicy, LengthRecoveryPolicy, ModelHost, ModelSelection, Route},
    steering::SteeringBuffer,
    RunMode, RunRequest, RunResult, RuntimePaths, ToolRegistration,
};

const CORE: &str = "ara_agent_lite_core::";

/// Stable Core items exercised by `stable_items_are_usable_through_public_paths`.
const USED_STABLE: &[&str] = &[
    "RunRequest.model_host",
    "RunRequest.extra_tools",
    "RunRequest.enable_local_tools",
    "RunRequest.mode",
    "RunRequest.goal_criteria",
    "RunRequest.goal_id",
    "RunRequest.session_key",
    "RunRequest.expected_revision",
    "RunRequest.reviewed_run_id",
    "RunRequest.id",
    "RunRequest.device_fp",
    "RunRequest.objective",
    "RunRequest.model",
    "RunRequest.endpoint",
    "RunRequest.token",
    "RunRequest.route",
    "RunRequest.max_output",
    "RunRequest.cwd",
    "RunRequest.state_path",
    "RunRequest.environment_state",
    "RunRequest.global_knowledge",
    "RunRequest.brief",
    "RunRequest.max_calls",
    "RunRequest.max_seconds",
    "RunRequest.steering",
    "rpi_agent",
    "rpi_ai",
    "RunRequest::new",
    "RuntimePaths",
    "RuntimePaths::new",
    "ToolRegistration",
    "RunMode",
    "RunResult",
    "execute",
    "model::Route",
    "model::ModelHost",
    "model::ModelHost::generation_policy",
    "model::ModelHost::length_recovery_policy",
    "model::GenerationPolicy",
    "model::LengthRecoveryPolicy",
    "model::CatalogModelHost",
    "model::ModelSelection",
    "steering::SteeringBuffer",
];

fn rows(crate_prefix: &str) -> Vec<(String, String)> {
    let doc = include_str!("../docs/stability.md");
    doc.lines()
        .filter_map(|line| {
            let cells: Vec<_> = line.trim().strip_prefix('|')?.split('|').map(str::trim).collect();
            let path = cells.first()?.strip_prefix('`')?.strip_suffix('`')?;
            let item = path.strip_prefix(crate_prefix)?;
            Some((item.to_string(), cells.get(1)?.to_string()))
        })
        .collect()
}

#[test]
fn stability_doc_classifies_each_item_once_for_this_version() {
    let doc = include_str!("../docs/stability.md");
    let version = format!("`ara-agent-lite-core {}`", env!("CARGO_PKG_VERSION"));
    assert!(doc.contains(&version), "stability.md must name {version}");

    let rows = rows(CORE);
    let mut levels = BTreeMap::new();
    for (item, level) in &rows {
        assert!(matches!(level.as_str(), "stable" | "experimental" | "internal"), "{item}: level {level}");
        assert!(levels.insert(item.clone(), level.clone()).is_none(), "{item} is listed twice");
    }

    // Exhaustive destructuring: adding or removing a `RunRequest` field breaks
    // this pattern, and the document must list exactly these 31 names (29.1).
    let request = RunRequest::new("id", "objective", RuntimePaths::new(".", "session.json", "environment"));
    let RunRequest {
        model_host: _,
        worker_executable: _,
        extra_tools: _,
        context_snapshot: _,
        knowledge_roots: _,
        enable_local_tools: _,
        mode: _,
        goal_criteria: _,
        goal_id: _,
        goal_lease: _,
        resume_job_lease: _,
        session_key: _,
        expected_revision: _,
        reviewed_run_id: _,
        id: _,
        device_fp: _,
        objective: _,
        model: _,
        endpoint: _,
        token: _,
        route: _,
        max_output: _,
        cwd: _,
        state_path: _,
        environment_state: _,
        global_knowledge: _,
        brief: _,
        max_calls: _,
        max_seconds: _,
        steering: _,
        policies: _,
    } = request;
    let fields: BTreeSet<&str> = [
        "model_host",
        "worker_executable",
        "extra_tools",
        "context_snapshot",
        "knowledge_roots",
        "enable_local_tools",
        "mode",
        "goal_criteria",
        "goal_id",
        "goal_lease",
        "resume_job_lease",
        "session_key",
        "expected_revision",
        "reviewed_run_id",
        "id",
        "device_fp",
        "objective",
        "model",
        "endpoint",
        "token",
        "route",
        "max_output",
        "cwd",
        "state_path",
        "environment_state",
        "global_knowledge",
        "brief",
        "max_calls",
        "max_seconds",
        "steering",
        "policies",
    ]
    .into_iter()
    .collect();
    let listed_fields: BTreeSet<&str> = levels.keys().filter_map(|item| item.strip_prefix("RunRequest.")).collect();
    assert_eq!(listed_fields, fields, "RunRequest fields in stability.md");
    let reexports: BTreeSet<&str> = levels.keys().map(String::as_str).filter(|item| item.starts_with("rpi_")).collect();
    assert_eq!(reexports, BTreeSet::from(["rpi_agent", "rpi_ai"]), "crate-root rpi_* re-exports");
    assert_eq!(listed_fields.len() + reexports.len(), 33);
}

#[test]
fn stable_set_in_the_document_equals_the_items_used_here() {
    let documented: BTreeSet<String> =
        rows(CORE).into_iter().filter(|(_, level)| level == "stable").map(|(item, _)| item).collect();
    let used: BTreeSet<String> = USED_STABLE.iter().map(|item| item.to_string()).collect();
    assert_eq!(USED_STABLE.len(), used.len(), "USED_STABLE has duplicates");
    assert_eq!(documented, used, "stable items in stability.md versus this test");
}

#[test]
fn stable_items_are_usable_through_public_paths() {
    // Constructors and paths.
    let paths: RuntimePaths = RuntimePaths::new("work", "work/session.json", "work/environment");
    let RuntimePaths { cwd, session_path, environment_state } = paths;
    let mut request: RunRequest =
        RunRequest::new("run-1", "objective", RuntimePaths { cwd, session_path, environment_state });

    // Every stable RunRequest field: one typed write and one typed read.
    let host: Arc<dyn ModelHost> = request.model_host.clone();
    request.model_host = host;
    let _: &Arc<dyn ModelHost> = &request.model_host;
    let tool: Arc<dyn ara_agent_lite_core::rpi_agent::AgentTool> = Arc::new(NoopTool::new());
    request.extra_tools = vec![ToolRegistration { tool, allow_in_plan: true }];
    let registration: &ToolRegistration = &request.extra_tools[0];
    assert!(registration.allow_in_plan && registration.tool.schema().name == "noop");
    request.enable_local_tools = false;
    assert!(!request.enable_local_tools);
    request.mode = RunMode::Execute;
    assert!(matches!(request.mode, RunMode::Execute | RunMode::Plan | RunMode::Goal));
    request.goal_criteria = vec!["criterion".to_string()];
    let _: &Vec<String> = &request.goal_criteria;
    request.goal_id = String::from("goal");
    let _: &String = &request.goal_id;
    request.session_key = String::from("tenant/session");
    let _: &String = &request.session_key;
    request.expected_revision = Some(1u64);
    let _: Option<u64> = request.expected_revision;
    request.reviewed_run_id = String::from("run-0");
    let _: &String = &request.reviewed_run_id;
    request.id = String::from("run-2");
    let _: &String = &request.id;
    request.device_fp = String::from("device");
    let _: &String = &request.device_fp;
    request.objective = String::from("changed objective");
    let _: &String = &request.objective;
    request.model = String::from("model-id");
    let _: &String = &request.model;
    request.endpoint = String::from("http://127.0.0.1:9/v1");
    let _: &String = &request.endpoint;
    request.token = String::from("in-memory-credential");
    let _: &String = &request.token;
    request.route = Route {
        model: "model-id".into(),
        protocol: "openai-completions".into(),
        binding: String::new(),
        decision: String::new(),
    };
    let route: &Route = &request.route;
    assert_eq!((route.model.as_str(), route.binding.as_str(), route.decision.as_str()), ("model-id", "", ""));
    request.max_output = 1024u32;
    let _: u32 = request.max_output;
    request.cwd = PathBuf::from("work");
    let _: &PathBuf = &request.cwd;
    request.state_path = PathBuf::from("work/session.json");
    let _: &PathBuf = &request.state_path;
    request.environment_state = PathBuf::from("work/environment");
    let _: &PathBuf = &request.environment_state;
    request.global_knowledge = None::<PathBuf>;
    let _: &Option<PathBuf> = &request.global_knowledge;
    request.brief = String::from("brief");
    let _: &String = &request.brief;
    request.max_calls = 3usize;
    let _: usize = request.max_calls;
    request.max_seconds = 30u64;
    let _: u64 = request.max_seconds;
    request.steering = Arc::new(Mutex::new(SteeringBuffer::default()));
    let _: &Arc<Mutex<SteeringBuffer>> = &request.steering;

    // Model policy items: the default Host, both stable default methods with
    // their documented defaults, every variant, and the serde names a Host
    // reads from its own configuration (DR-P4-3).
    // `Default` is part of the documented contract; the fn pointer keeps it checked.
    let default_host: fn() -> CatalogModelHost = CatalogModelHost::default;
    let catalog = default_host();
    let route = request.route.clone();
    let generation: GenerationPolicy = catalog.generation_policy(&route).unwrap();
    assert_eq!(generation, GenerationPolicy::default());
    let length: LengthRecoveryPolicy = catalog.length_recovery_policy(&route).unwrap();
    assert_eq!(length, LengthRecoveryPolicy::default());
    for (policy, name) in [
        (GenerationPolicy::ProviderDefault, "provider_default"),
        (GenerationPolicy::ThinkingTypeDisabled, "thinking_type_disabled"),
    ] {
        match policy {
            GenerationPolicy::ProviderDefault | GenerationPolicy::ThinkingTypeDisabled => {}
        }
        assert_eq!(serde_json::to_value(policy).unwrap(), serde_json::json!(name));
        assert_eq!(serde_json::from_value::<GenerationPolicy>(serde_json::json!(name)).unwrap(), policy);
    }
    assert_eq!(generation, GenerationPolicy::ProviderDefault);
    for (policy, name) in [(LengthRecoveryPolicy::Stop, "stop"), (LengthRecoveryPolicy::ReplanOnce, "replan_once")] {
        match policy {
            LengthRecoveryPolicy::Stop | LengthRecoveryPolicy::ReplanOnce => {}
        }
        assert_eq!(serde_json::to_value(policy).unwrap(), serde_json::json!(name));
        assert_eq!(serde_json::from_value::<LengthRecoveryPolicy>(serde_json::json!(name)).unwrap(), policy);
    }
    assert_eq!(length, LengthRecoveryPolicy::Stop);
    let policy_host = PolicyHost {
        catalog: CatalogModelHost,
        generation: GenerationPolicy::ThinkingTypeDisabled,
        length: LengthRecoveryPolicy::ReplanOnce,
    };
    assert_eq!(policy_host.generation_policy(&route).unwrap(), GenerationPolicy::ThinkingTypeDisabled);
    assert_eq!(policy_host.length_recovery_policy(&route).unwrap(), LengthRecoveryPolicy::ReplanOnce);
    let policy_host: Arc<dyn ModelHost> = Arc::new(policy_host);
    request.model_host = policy_host;

    // The entry point with its full signature; the future is dropped unpolled,
    // so nothing runs and no request is sent.
    let (events, _receiver) = tokio::sync::mpsc::channel(1);
    drop(call_execute(request, tokio_util::sync::CancellationToken::new(), events));
    let result = RunResult { text: String::new(), code: 0i32, error: String::new() };
    let RunResult { text, code, error } = result;
    assert!(text.is_empty() && code == 0 && error.is_empty());
}

async fn call_execute(
    request: RunRequest,
    cancellation: tokio_util::sync::CancellationToken,
    events: tokio::sync::mpsc::Sender<serde_json::Value>,
) -> anyhow::Result<RunResult> {
    ara_agent_lite_core::execute(request, cancellation, events).await
}

/// A Host ModelHost written only against stable items: `resolve` passes the
/// default Host's `ModelSelection` through unchanged, and both stable default
/// methods are overridden with their exact signatures.
struct PolicyHost {
    catalog: CatalogModelHost,
    generation: GenerationPolicy,
    length: LengthRecoveryPolicy,
}
#[async_trait::async_trait]
impl ModelHost for PolicyHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> anyhow::Result<ModelSelection> {
        self.catalog.resolve(route, endpoint, token, run).await
    }
    fn generation_policy(&self, _: &Route) -> anyhow::Result<GenerationPolicy> {
        Ok(self.generation)
    }
    fn length_recovery_policy(&self, _: &Route) -> anyhow::Result<LengthRecoveryPolicy> {
        Ok(self.length)
    }
}

/// A Host tool written only against the stable `rpi_agent`/`rpi_ai` re-exports.
struct NoopTool {
    schema: ara_agent_lite_core::rpi_ai::Tool,
}
impl NoopTool {
    fn new() -> Self {
        Self {
            schema: ara_agent_lite_core::rpi_ai::Tool {
                name: "noop".into(),
                description: "Does nothing.".into(),
                parameters: ara_agent_lite_core::rpi_ai::Schema(serde_json::json!({"type":"object","properties":{}})),
                constrained_sampling: None,
            },
        }
    }
}
#[async_trait::async_trait]
impl ara_agent_lite_core::rpi_agent::AgentTool for NoopTool {
    fn schema(&self) -> &ara_agent_lite_core::rpi_ai::Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        "noop"
    }
    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
        _: tokio_util::sync::CancellationToken,
        _: Arc<dyn Fn(ara_agent_lite_core::rpi_agent::ToolResultPartial) + Send + Sync>,
    ) -> Result<ara_agent_lite_core::rpi_agent::AgentToolResult, ara_agent_lite_core::rpi_agent::AgentError> {
        Ok(ara_agent_lite_core::rpi_agent::AgentToolResult::text("noop"))
    }
}
