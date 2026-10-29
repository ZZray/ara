//! Capability_Test_Host (design “Capability_Test_Host”): the Host used by
//! the Evaluation_Runner. Its tools only reach the task workspace.
//!
//! * `verify` runs the task's registered public check on a fresh copy of the
//!   workspace, never the hidden or holdout Oracle, whose results never reach
//!   the model. Without a registered public check the Host has nothing to
//!   check and declares the candidate verified; only the Oracle decides.
//! * `prepare_continuation` continues the same Session with the check
//!   feedback, reusing the same ModelHost, steering and tool `Arc`s.
//! * The stop-snapshot callbacks keep the Harness default (refuse) until a
//!   P2 switch in [`features`] turns them on.

pub mod features;
pub mod tools;

use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{
    core::{
        analysis_tools::{self, AnalysisConfig},
        model::{CatalogModelHost, GenerationPolicy, LengthRecoveryPolicy, ModelHost, ModelSelection, Route},
        rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
        rpi_ai::{Schema, Tool},
        steering::SteeringBuffer,
        RunMode, RunRequest, RuntimePaths, ToolRegistration,
    },
    Candidate, Continuation, HarnessHost, ReviewReceipt, Settlement, Verdict,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    common::{run_bounded, write_json_new, Bounded, Credentials},
    registry::{ProfileMode, TaskEntry},
    sealed::copy_workspace,
    workspace::Workspace,
};

pub use features::Features;
pub use tools::{WriteCounters, WriteCounts};

/// Largest feedback returned to the model (design: 8 KiB).
pub const FEEDBACK_LIMIT: usize = 8 * 1024;
/// Bound of one public-check process.
pub const PUBLIC_CHECK_TIMEOUT: Duration = Duration::from_secs(600);
/// Tool that runs the Host's fixed public-check command.
pub const PUBLIC_CHECK_TOOL: &str = "run_public_checks";

/// ModelHost of the Run_Policy: catalog resolution plus the policy's
/// generation and length-recovery settings. P2-a sets the output ceiling.
pub struct PolicyModelHost {
    pub generation: GenerationPolicy,
    pub length_recovery: LengthRecoveryPolicy,
    pub output_ceiling: Option<u32>,
}

#[async_trait]
impl ModelHost for PolicyModelHost {
    async fn resolve(&self, route: Route, endpoint: &str, token: &str, run: &str) -> Result<ModelSelection> {
        CatalogModelHost.resolve(route, endpoint, token, run).await
    }

    fn capabilities_source(&self) -> &'static str {
        CatalogModelHost.capabilities_source()
    }

    fn generation_policy(&self, _route: &Route) -> Result<GenerationPolicy> {
        Ok(self.generation)
    }

    fn length_recovery_policy(&self, _route: &Route) -> Result<LengthRecoveryPolicy> {
        Ok(self.length_recovery)
    }

    fn output_ceiling(&self) -> Result<Option<u32>> {
        Ok(self.output_ceiling)
    }
}

/// Everything a Run request of this task needs besides its ID, objective
/// and caps. `token` is the credential, kept in memory only.
pub struct RunTemplate {
    pub endpoint: String,
    pub token: String,
    pub route: Route,
    pub max_output: u32,
    pub session_path: PathBuf,
    pub environment_state: PathBuf,
    pub session_key: String,
    pub run_max_calls: usize,
    pub run_max_seconds: u64,
}

/// Inputs of [`CapabilityHost::build`].
pub struct HostConfig {
    pub task: TaskEntry,
    pub workspace: Workspace,
    pub features: Features,
    pub template: RunTemplate,
    pub model_host: Arc<dyn ModelHost>,
    /// Removed from every Host-started process (public checks).
    pub credential_env: String,
    /// Credential values checked before any receipt reaches disk.
    pub credentials: Credentials,
}

/// Outcome of one public-check run, as recorded in its receipt.
#[derive(Clone, Debug)]
pub struct CheckOutcome {
    pub passed: bool,
    pub receipt: PathBuf,
    /// Exit status and output tail, at most [`FEEDBACK_LIMIT`] bytes.
    pub feedback: String,
}

/// The fixed public check of a task, run on a copy of the workspace.
pub struct PublicCheck {
    command: Vec<String>,
    workspace: Workspace,
    credential_env: String,
    credentials: Credentials,
    sequence: AtomicUsize,
}

impl PublicCheck {
    /// Copy the workspace into a new `<private>/<label>-<n>/`, run the
    /// command there and write `<private>/<label>-<n>.json`.
    pub fn run(&self, label: &str) -> Result<CheckOutcome> {
        let n = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let copy = self.workspace.private.join(format!("{label}-{n}"));
        copy_workspace(&self.workspace, &copy)?;
        let mut command = Command::new(&self.command[0]);
        command.args(&self.command[1..]).current_dir(&copy).env_remove(&self.credential_env);
        let (passed, exit, stdout, stderr) = match run_bounded(command, PUBLIC_CHECK_TIMEOUT)? {
            Bounded::TimedOut => (false, "timed out".to_owned(), Vec::new(), Vec::new()),
            Bounded::Exited { code, stdout, stderr } => (code == Some(0), format!("exit {code:?}"), stdout, stderr),
        };
        let output = self.credentials.redact(&format!(
            "{}{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&stderr)
        ));
        let verdict = if passed { "passed" } else { "failed" };
        let feedback = tail(&format!("public checks {verdict} ({exit}):\n{output}"), FEEDBACK_LIMIT);
        let receipt = self.workspace.private.join(format!("{label}-{n}.json"));
        let record = json!({
            "command": self.command, "copy": copy, "passed": passed, "exit": exit,
            "stdout": self.credentials.redact(&String::from_utf8_lossy(&stdout)),
            "stderr": self.credentials.redact(&String::from_utf8_lossy(&stderr)),
        });
        write_json_new(&receipt, &record, &self.credentials)?;
        Ok(CheckOutcome { passed, receipt, feedback })
    }
}

/// The last `limit` bytes of `text`, starting on a character boundary.
fn tail(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut start = text.len() - limit;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_owned()
}

struct PublicCheckTool {
    schema: Tool,
    check: Arc<PublicCheck>,
}

#[async_trait]
impl AgentTool for PublicCheckTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        PUBLIC_CHECK_TOOL
    }

    async fn execute(
        &self,
        _: &str,
        _: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let check = self.check.clone();
        let outcome = tokio::task::spawn_blocking(move || check.run("public-check"))
            .await
            .map_err(|error| AgentError::Tool(format!("public check did not finish: {error}")))?
            .map_err(|error| AgentError::Tool(format!("public check could not run: {error:#}")))?;
        Ok(AgentToolResult::text(outcome.feedback))
    }
}

pub struct CapabilityHost {
    pub task: TaskEntry,
    pub workspace: Workspace,
    pub features: Features,
    pub counters: Arc<WriteCounters>,
    pub model_host: Arc<dyn ModelHost>,
    /// Registered tools with `allow_in_plan`, in registration order.
    pub tools: Vec<(Arc<dyn AgentTool>, bool)>,
    pub steering: Arc<Mutex<SteeringBuffer>>,
    /// Profile tools not registered, each with the reason.
    pub skipped_tools: Vec<String>,
    /// Registered tools that bypass the Host path checks (2.8).
    pub unchecked_tools: Vec<String>,
    template: RunTemplate,
    public_check: Option<Arc<PublicCheck>>,
    credentials: Credentials,
}

/// Executable named `name` on PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(&std::env::var_os("PATH")?).map(|dir| dir.join(&file)).find(|path| path.is_file())
}

impl CapabilityHost {
    /// Register the profile's tools for this workspace. Plan-mode tasks get
    /// only read-only tools.
    pub async fn build(config: HostConfig) -> Result<Self> {
        let HostConfig { task, workspace, features, template, model_host, credential_env, credentials } = config;
        let counters = Arc::new(WriteCounters::default());
        let plan = task.profile.mode == ProfileMode::Plan;
        let public_check = (!task.profile.public_check.is_empty()).then(|| {
            Arc::new(PublicCheck {
                command: task.profile.public_check.clone(),
                workspace: workspace.clone(),
                credential_env: credential_env.clone(),
                credentials: credentials.clone(),
                sequence: AtomicUsize::new(0),
            })
        });
        let mut tools: Vec<(Arc<dyn AgentTool>, bool)> = Vec::new();
        let mut skipped = Vec::new();
        let mut unchecked = Vec::new();
        let mut analysis: Option<Vec<ToolRegistration>> = None;
        for name in &task.profile.tools {
            if tools.iter().any(|(tool, _)| &tool.schema().name == name) {
                continue;
            }
            if tools::WS_TOOLS.contains(&name.as_str()) {
                let read_only = tools::is_read_only(name);
                if plan && !read_only {
                    skipped.push(format!("{name}: not read-only, Plan mode"));
                    continue;
                }
                match tools::ws_tool(name, &workspace.root, counters.clone(), features.edit_file) {
                    Some(tool) => tools.push((tool, read_only)),
                    None => skipped.push(format!("{name}: P2-e switch is off")),
                }
            } else if name == PUBLIC_CHECK_TOOL {
                match (&public_check, plan) {
                    (_, true) => skipped.push(format!("{name}: executes a process, Plan mode")),
                    (None, false) => skipped.push(format!("{name}: no public_check command registered")),
                    (Some(check), false) => {
                        let schema = Tool {
                            name: PUBLIC_CHECK_TOOL.into(),
                            description: "Run this task's fixed public checks on a fresh copy of the workspace and \
                                          return the exit status and output tail. Takes no arguments."
                                .into(),
                            parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                            constrained_sampling: None,
                        };
                        tools.push((Arc::new(PublicCheckTool { schema, check: check.clone() }), false));
                        unchecked.push(name.clone());
                    }
                }
            } else if matches!(name.as_str(), "binary_inspect" | "source_symbols") {
                if analysis.is_none() {
                    let artifact_root = workspace.private.join("analysis");
                    std::fs::create_dir_all(&artifact_root)?;
                    let config = AnalysisConfig {
                        cwd: workspace.root.clone(),
                        artifact_root,
                        ctags: None,
                        objdump: on_path("objdump"),
                        nm: on_path("nm"),
                    };
                    analysis = Some(match analysis_tools::register(config).await {
                        Ok(registrations) => registrations,
                        Err(error) => {
                            skipped.push(format!("analysis tools: {error:#}"));
                            Vec::new()
                        }
                    });
                }
                match analysis.as_ref().and_then(|list| list.iter().find(|item| &item.tool.schema().name == name)) {
                    Some(item) => {
                        tools.push((item.tool.clone(), item.allow_in_plan));
                        unchecked.push(name.clone());
                    }
                    None => skipped.push(format!("{name}: no backend available")),
                }
            } else {
                skipped.push(format!("{name}: not a Capability_Test_Host tool"));
            }
        }
        Ok(Self {
            task,
            workspace,
            features,
            counters,
            model_host,
            tools,
            steering: Arc::new(Mutex::new(SteeringBuffer::default())),
            skipped_tools: skipped,
            unchecked_tools: unchecked,
            template,
            public_check,
            credentials,
        })
    }

    /// A Run request of this task's Session. The Harness overrides the caps
    /// of a continuation with the remaining Task budget.
    pub fn request(&self, run_id: &str, objective: &str) -> RunRequest {
        let template = &self.template;
        let paths = RuntimePaths::new(&self.workspace.root, &template.session_path, &template.environment_state);
        let mut request = RunRequest::new(run_id, objective, paths);
        request.model_host = self.model_host.clone();
        request.steering = self.steering.clone();
        request.endpoint = template.endpoint.clone();
        request.token = template.token.clone();
        request.model = template.route.model.clone();
        request.route = template.route.clone();
        request.max_output = template.max_output;
        request.device_fp = "lite-eval-capability-host".into();
        request.session_key = template.session_key.clone();
        request.mode = match self.task.profile.mode {
            ProfileMode::Execute => RunMode::Execute,
            ProfileMode::Plan => RunMode::Plan,
        };
        request.max_calls = template.run_max_calls;
        request.max_seconds = template.run_max_seconds;
        request.extra_tools = self
            .tools
            .iter()
            .map(|(tool, allow_in_plan)| ToolRegistration { tool: tool.clone(), allow_in_plan: *allow_in_plan })
            .collect();
        request
    }

    /// Texts the model sees besides the workspace: tool descriptions.
    pub fn tool_descriptions(&self) -> Vec<(String, String)> {
        self.tools
            .iter()
            .map(|(tool, _)| (format!("tool:{}", tool.schema().name), tool.schema().description.clone()))
            .collect()
    }

    pub fn write_counts(&self) -> WriteCounts {
        self.counters.snapshot()
    }

    fn receipt_path(&self, run_number: usize) -> PathBuf {
        self.workspace.private.join(format!("verify-run-{run_number}.json"))
    }
}

#[async_trait]
impl HarnessHost for CapabilityHost {
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        ensure!(!cancellation.is_cancelled(), "verification cancelled before it started");
        let run_number = candidate.basis.run_number;
        let (verdict, evidence) = match &self.public_check {
            None => {
                let receipt = self.receipt_path(run_number);
                let record = json!({"public_check": "none_registered", "basis": candidate.basis});
                write_json_new(&receipt, &record, &self.credentials)?;
                (Verdict::Verified, receipt)
            }
            Some(check) => {
                let check = check.clone();
                let label = format!("verify-run-{run_number}");
                let outcome = tokio::task::spawn_blocking(move || check.run(&label))
                    .await
                    .context("public check task did not finish")??;
                let verdict = if outcome.passed {
                    Verdict::Verified
                } else {
                    Verdict::NeedsChanges { feedback: outcome.feedback }
                };
                (verdict, outcome.receipt)
            }
        };
        Ok(ReviewReceipt {
            basis: candidate.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict,
            evidence: vec![evidence.display().to_string()],
        })
    }

    async fn prepare_continuation(&self, continuation: Continuation, _: CancellationToken) -> Result<RunRequest> {
        ensure!(continuation.receipt.committed, "continuation of an uncommitted review");
        let feedback = match &continuation.receipt.verdict {
            Verdict::NeedsChanges { feedback } => feedback.clone(),
            other => anyhow::bail!("continuation needs a NeedsChanges review, got {other:?}"),
        };
        let objective = format!(
            "The Host ran this task's public checks on your current workspace and they did not pass.\n\n{feedback}\n\n\
             Repair the workspace files, then finish with a short summary."
        );
        Ok(self.request(&run_id(&self.task.task_id, continuation.next_run_number), &objective))
    }
}

/// Run ID of Run `number` (1-based) of `task_id`.
pub fn run_id(task_id: &str, number: usize) -> String {
    format!("{}-run-{number}", task_id.to_lowercase())
}

/// Paths of a task's Session files under its private directory.
pub fn session_paths(private: &Path) -> (PathBuf, PathBuf) {
    (private.join("session.json"), private.join("environment"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_tail_keeps_the_end_on_a_char_boundary() {
        assert_eq!(tail("abc", 8), "abc");
        let text = format!("{}中文", "x".repeat(10));
        let kept = tail(&text, 5);
        assert!(kept.len() <= 5 && text.ends_with(&kept), "{kept}");
        assert_eq!(run_id("SH1", 2), "sh1-run-2");
    }
}
