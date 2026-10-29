use std::{
    collections::{BTreeMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{ensure, Result};
use ara_agent_lite_harness::{
    self as harness,
    core::{
        model::{CatalogModelHost, GenerationPolicy, LengthRecoveryPolicy, ModelHost, ModelSelection, Route},
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        steering::SteeringBuffer,
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    BudgetStopContinuation, BudgetStopReviewPolicy, BudgetStopSnapshot, Candidate, Continuation, HarnessHost,
    ReviewReceipt, Settlement,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

use crate::{
    evidence,
    protocol::{Resolution, RunConfig, Start},
    rpc::{Bridge, Reply, ToolKey},
};

pub fn validate(start: &Start) -> Result<()> {
    ensure!(start.kind == "start_task" && start.version == 1, "start_kind_or_version");
    ensure!((1000..=15000).contains(&start.cancel_grace_ms), "cancel_grace_range");
    let run = &start.run;
    ensure!(
        start.budget_stop_review == BudgetStopReviewPolicy::Reject || run.mode == "execute",
        "budget_stop_review_execute_only"
    );
    ensure!(matches!(run.mode.as_str(), "execute" | "plan"), "mode_not_supported");
    ensure!(run.length_recovery == LengthRecoveryPolicy::Stop || run.mode == "execute", "length_recovery_execute_only");
    ensure!(
        run.cwd.is_absolute()
            && run.cwd.is_dir()
            && run.session_path.is_absolute()
            && run.environment_state.is_absolute()
            && start.task.evidence_dir.is_absolute(),
        "absolute_paths_required"
    );
    ensure!(!run.session_path.is_dir(), "session_file_required");
    ensure!(
        !run.model.trim().is_empty()
            && !run.run_id.trim().is_empty()
            && !run.objective.trim().is_empty()
            && !run.device_fp.trim().is_empty()
            && !run.session_key.trim().is_empty(),
        "run_identity_required"
    );
    ensure!(run.host_output_ceiling != Some(0), "start_config_rejected");
    let bound = ara_agent_lite_harness::core::output_limit::admission_bound(run.host_output_ceiling);
    ensure!(
        run.max_calls > 0 && run.max_seconds > 0 && (1..=bound).contains(&u64::from(run.max_output)),
        "run_budget_invalid"
    );
    ensure!(run.tools.len() <= 32, "too_many_tools");
    let mut names = HashSet::new();
    for tool in &run.tools {
        ensure!(
            !tool.name.is_empty()
                && tool.name.len() <= 64
                && tool.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                && names.insert(&tool.name)
                && !harness::core::agent::BUILTIN_TOOL_NAMES.contains(&tool.name.as_str()),
            "invalid_or_reserved_tool"
        );
        ensure!(tool.parameters.is_object(), "tool_schema_object_required");
    }
    let endpoint = Url::parse(&run.endpoint)?;
    harness::core::model::validate_endpoint(&endpoint)?;
    if let Resolution::HostDeclaredLoopback { capabilities } = &run.model_resolution {
        validate_loopback(&run.endpoint)?;
        ensure!(
            capabilities.max_tokens > 0
                && capabilities.input.as_ref().is_none_or(|items| items.iter().all(|item| item == "text")),
            "host_caps_text_only"
        );
    }
    Ok(())
}

fn validate_loopback(endpoint: &str) -> Result<()> {
    let url = Url::parse(endpoint)?;
    ensure!(
        url.scheme() == "http"
            && url.port().is_some()
            && matches!(url.host(), Some(Host::Ipv4(ip)) if ip.is_loopback())
            || url.scheme() == "http"
                && url.port().is_some()
                && matches!(url.host(), Some(Host::Ipv6(ip)) if ip.is_loopback()),
        "literal_loopback_required"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path().trim_end_matches('/') == "/v1",
        "loopback_components_invalid"
    );
    Ok(())
}

struct DeclaredModelHost(harness::core::model_caps::ModelCaps);

#[async_trait]
impl ModelHost for DeclaredModelHost {
    async fn resolve(&self, route: Route, endpoint: &str, _: &str, _: &str) -> Result<ModelSelection> {
        validate_loopback(endpoint)?;
        ensure!(route.protocol == "openai-completions", "chat_route_required");
        Ok(ModelSelection {
            route,
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .build()?,
            capabilities: self.0.clone(),
            headers: BTreeMap::new(),
        })
    }
}

/// One immutable Host policy for the whole Task. All model resolution and
/// provenance still come from the selected base Host; prepare cannot replace it.
struct TaskModelHost {
    inner: Arc<dyn ModelHost>,
    generation: GenerationPolicy,
    length_recovery: LengthRecoveryPolicy,
    output_ceiling: Option<u32>,
}

#[async_trait]
impl ModelHost for TaskModelHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        self.inner.resolve(route, endpoint, token, run).await
    }

    fn capabilities_source(&self) -> &'static str {
        self.inner.capabilities_source()
    }

    fn generation_policy(&self, _: &Route) -> Result<GenerationPolicy> {
        Ok(self.generation)
    }

    fn length_recovery_policy(&self, _: &Route) -> Result<LengthRecoveryPolicy> {
        Ok(self.length_recovery)
    }

    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(self.output_ceiling)
    }
}

pub struct Template {
    config: RunConfig,
    token: String,
    model_host: Arc<dyn ModelHost>,
    tools: Vec<Arc<dyn AgentTool>>,
    steering: Arc<Mutex<SteeringBuffer>>,
}

impl Template {
    pub fn new(config: RunConfig, token: String, bridge: Arc<Bridge>) -> Self {
        let base: Arc<dyn ModelHost> = match &config.model_resolution {
            Resolution::Catalog => Arc::new(CatalogModelHost),
            Resolution::HostDeclaredLoopback { capabilities } => Arc::new(DeclaredModelHost(capabilities.model_caps())),
        };
        let model_host: Arc<dyn ModelHost> = Arc::new(TaskModelHost {
            inner: base,
            generation: config.generation_policy,
            length_recovery: config.length_recovery,
            output_ceiling: config.host_output_ceiling,
        });
        let tools = config
            .tools
            .iter()
            .map(|tool| {
                Arc::new(BusinessTool {
                    bridge: bridge.clone(),
                    schema: Tool {
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        parameters: Schema(tool.parameters.clone()),
                        constrained_sampling: None,
                    },
                }) as Arc<dyn AgentTool>
            })
            .collect();
        Self { config, token, model_host, tools, steering: Arc::new(Mutex::new(SteeringBuffer::default())) }
    }

    pub fn initial(&self) -> RunRequest {
        self.request(self.config.run_id.clone(), self.config.objective.clone())
    }

    fn request(&self, run_id: String, objective: String) -> RunRequest {
        let config = &self.config;
        let mut request = RunRequest::new(
            run_id,
            objective,
            RuntimePaths::new(&config.cwd, &config.session_path, &config.environment_state),
        );
        request.model_host = self.model_host.clone();
        request.mode = if config.mode == "execute" { RunMode::Execute } else { RunMode::Plan };
        request.endpoint = config.endpoint.clone();
        request.model = config.model.clone();
        request.route = Route {
            model: config.model.clone(),
            protocol: "openai-completions".into(),
            binding: "ara-shared-jsonl-v1".into(),
            decision: "fixed-host-route".into(),
        };
        request.token = self.token.clone();
        request.device_fp = config.device_fp.clone();
        request.session_key = config.session_key.clone();
        request.max_calls = config.max_calls;
        request.max_seconds = config.max_seconds;
        request.max_output = config.max_output;
        request.steering = self.steering.clone();
        request.extra_tools = self
            .tools
            .iter()
            .zip(&config.tools)
            .map(|(tool, declaration)| ToolRegistration {
                tool: tool.clone(),
                allow_in_plan: declaration.allow_in_plan,
            })
            .collect();
        // These are deliberately never configurable through this protocol.
        request.enable_local_tools = false;
        request.worker_executable = None;
        request
    }
}

struct BusinessTool {
    schema: Tool,
    bridge: Arc<Bridge>,
}

#[async_trait]
impl AgentTool for BusinessTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }

    async fn execute(
        &self,
        native_call_id: &str,
        arguments: Value,
        cancellation: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        if native_call_id.is_empty() || native_call_id.len() > 256 || !arguments.is_object() {
            return Err(AgentError::Tool("invalid_external_tool_request".into()));
        }
        let run_id = self.bridge.active_run.lock().unwrap().clone();
        let key = ToolKey {
            native_call_id: native_call_id.to_owned(),
            tool_name: self.schema.name.clone(),
            args_sha256: evidence::hash(&serde_json::to_vec(&arguments).unwrap_or_default()),
        };
        let body = json!({"native_call_id":key.native_call_id,"tool_name":key.tool_name,
            "args_sha256":key.args_sha256,"arguments":arguments});
        let reply = self.bridge.call("tool_call", run_id, body, Some(key), cancellation.clone()).await;
        match reply {
            Ok(Reply::Tool(result)) if result.settlement == Settlement::Settled => {
                let text = serde_json::to_string(&result.value).unwrap_or_default();
                if result.is_error {
                    return Err(AgentError::Tool(text));
                }
                let mut value = AgentToolResult::text(text);
                value.details = json!({"host_operation_receipt":result.operation_receipt,
                    "settlement":"host_declared_settled"});
                Ok(value)
            }
            _ => {
                cancellation.cancel();
                self.bridge.fail();
                let mut value = AgentToolResult::text(
                    "External tool outcome unknown; inspect original Host operation receipt; do not replay.",
                );
                value.details = json!({"executed":"unknown","source":"jsonl_external_callback"});
                Ok(value)
            }
        }
    }
}

pub struct ExternalHost {
    pub template: Template,
    pub bridge: Arc<Bridge>,
}

#[async_trait]
impl HarnessHost for ExternalHost {
    async fn verify_budget_snapshot(
        &self,
        snapshot: BudgetStopSnapshot,
        cancellation: CancellationToken,
    ) -> Result<ReviewReceipt> {
        let reference = evidence::checked_budget_snapshot(&snapshot).inspect_err(|_| self.bridge.fail())?;
        let run_id = snapshot.basis.run_id.clone();
        let mut body = json!({"snapshot":snapshot});
        if self
            .bridge
            .output
            .needs_reference(&with_worst_budget_header(body.clone(), &self.bridge.task_id, &run_id))
            .inspect_err(|_| self.bridge.fail())?
        {
            body = json!({"snapshot_ref":reference});
        }
        let reply = self.bridge.call("verify_budget_request", run_id, body, None, cancellation).await;
        let after = evidence::checked_budget_snapshot(&snapshot).inspect_err(|_| self.bridge.fail())?;
        ensure!(
            after.sha256 == reference.sha256 && after.path == reference.path,
            "budget_snapshot_changed_during_verify"
        );
        match reply? {
            Reply::Verify(receipt) => Ok(*receipt),
            _ => anyhow::bail!("verify_budget_reply_kind"),
        }
    }

    async fn prepare_budget_stop_continuation(
        &self,
        continuation: BudgetStopContinuation,
        cancellation: CancellationToken,
    ) -> Result<RunRequest> {
        let snapshot_reference =
            evidence::checked_budget_snapshot(&continuation.previous).inspect_err(|_| self.bridge.fail())?;
        let run_id = continuation.previous.basis.run_id.clone();
        let payload = json!({"previous":continuation.previous,"receipt":continuation.receipt,
            "next_run_number":continuation.next_run_number,"remaining_calls":continuation.remaining_calls,
            "deadline_unix_ms":continuation.deadline_unix_ms});
        let path =
            self.bridge.evidence_dir.join(format!("jsonl-budget-continuation-{}.json", continuation.next_run_number));
        let reference = evidence::write_new(&path, &payload)?;
        evidence::checked_value(&path, &payload)?;
        let mut body = json!({"budget_continuation":payload});
        if self
            .bridge
            .output
            .needs_reference(&with_worst_budget_header(body.clone(), &self.bridge.task_id, &run_id))
            .inspect_err(|_| self.bridge.fail())?
        {
            body = json!({"budget_continuation_ref":reference});
        }
        let reply = self.bridge.call("prepare_budget_request", run_id, body, None, cancellation).await;
        let after_snapshot =
            evidence::checked_budget_snapshot(&continuation.previous).inspect_err(|_| self.bridge.fail())?;
        let after = evidence::checked_value(&path, &payload).inspect_err(|_| self.bridge.fail())?;
        ensure!(
            after.sha256 == reference.sha256 && after_snapshot.sha256 == snapshot_reference.sha256,
            "budget_continuation_changed_during_prepare"
        );
        match reply? {
            Reply::Prepare { run_id, objective } => {
                *self.bridge.active_run.lock().unwrap() = run_id.clone();
                Ok(self.template.request(run_id, objective))
            }
            _ => anyhow::bail!("prepare_budget_reply_kind"),
        }
    }

    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        let mut body = json!({"candidate":candidate});
        if self
            .bridge
            .output
            .needs_reference(&with_worst_header(body.clone(), &self.bridge.task_id, &candidate.basis.run_id))
            .inspect_err(|_| self.bridge.fail())?
        {
            let reference = match evidence::checked_candidate(&candidate) {
                Ok(reference) => reference,
                Err(error) => {
                    self.bridge.fail();
                    return Err(error);
                }
            };
            body = json!({"candidate_ref":reference});
        }
        match self.bridge.call("verify_request", candidate.basis.run_id.clone(), body, None, cancellation).await? {
            Reply::Verify(receipt) => Ok(*receipt),
            _ => anyhow::bail!("verify_reply_kind"),
        }
    }

    async fn prepare_continuation(
        &self,
        continuation: Continuation,
        cancellation: CancellationToken,
    ) -> Result<RunRequest> {
        let run_id = continuation.previous.basis.run_id.clone();
        let payload = json!({"previous":continuation.previous,"receipt":continuation.receipt,
            "next_run_number":continuation.next_run_number,"remaining_calls":continuation.remaining_calls,
            "deadline_unix_ms":continuation.deadline_unix_ms});
        let mut body = json!({"continuation":payload});
        if self
            .bridge
            .output
            .needs_reference(&with_worst_header(body.clone(), &self.bridge.task_id, &run_id))
            .inspect_err(|_| self.bridge.fail())?
        {
            let path: PathBuf =
                self.bridge.evidence_dir.join(format!("jsonl-continuation-{}.json", continuation.next_run_number));
            body = json!({"continuation_ref":evidence::write_new(&path, &payload)?});
        }
        match self.bridge.call("prepare_request", run_id, body, None, cancellation).await? {
            Reply::Prepare { run_id, objective } => {
                *self.bridge.active_run.lock().unwrap() = run_id.clone();
                Ok(self.template.request(run_id, objective))
            }
            _ => anyhow::bail!("prepare_reply_kind"),
        }
    }
}

fn with_worst_header(mut value: Value, task: &str, run: &str) -> Value {
    let object = value.as_object_mut().unwrap();
    object.insert("kind".into(), json!("prepare_request"));
    object.insert("version".into(), json!(1));
    object.insert("rpc_id".into(), json!("rpc-18446744073709551615"));
    object.insert("task_id".into(), json!(task));
    object.insert("run_id".into(), json!(run));
    value
}

fn with_worst_budget_header(value: Value, task: &str, run: &str) -> Value {
    let mut value = with_worst_header(value, task, run);
    value["kind"] = json!("prepare_budget_request");
    value
}
