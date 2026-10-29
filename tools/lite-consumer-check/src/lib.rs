//! Minimal out-of-tree Host over the ARA Lite crates (requirement 31).
//!
//! Only items that `crates/ara-agent-lite-core/docs/stability.md` marks stable
//! are referenced. The agent loop runs inside Lite Core through the Harness
//! `start` entry; this crate never calls a model provider itself and has no
//! model-call / tool-execution / result loop of its own.
use std::{
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Result};
use ara_agent_lite_core::{
    model::ModelHost,
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    steering::SteeringBuffer,
    RunMode, RunRequest, RuntimePaths, ToolRegistration,
};
use ara_agent_lite_harness::{Candidate, Continuation, HarnessHost, ReviewReceipt, Settlement, Verdict};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

/// Largest file the read tool returns and the write tool accepts.
const MAX_FILE_BYTES: usize = 256 * 1024;
/// Bytes of each check output stream kept in the Host record and feedback.
const CHECK_TAIL_BYTES: usize = 4096;

/// Host configuration; every value is chosen by the operator, never by the model.
#[derive(Clone, Debug)]
pub struct Config {
    /// Directory the restricted tools may read and write.
    pub workspace: PathBuf,
    /// Host-owned directory for the native Session, environment state and check records.
    pub state_dir: PathBuf,
    pub endpoint: String,
    pub model: String,
    pub protocol: String,
    /// In-memory credential; never written to records or output.
    pub token: String,
    pub run_max_calls: usize,
    pub run_max_seconds: u64,
    pub max_output: u32,
    /// Public check: program and arguments, run in `workspace`.
    pub check: Vec<String>,
    pub check_timeout: Duration,
}

pub struct ConsumerHost {
    config: Config,
    // The Harness requires the same ModelHost, steering owner and tool Arcs
    // in every Run of a Task, so they are created once here.
    model_host: Arc<dyn ModelHost>,
    steering: Arc<Mutex<SteeringBuffer>>,
    tools: Vec<(Arc<dyn AgentTool>, bool)>,
}

impl ConsumerHost {
    pub fn new(config: Config) -> Result<Self> {
        ensure!(!config.check.is_empty(), "public check command is required");
        std::fs::create_dir_all(config.state_dir.join("host")).context("create Host record directory")?;
        let workspace = config.workspace.canonicalize().context("workspace must exist")?;
        let defaults = RunRequest::new("defaults", "defaults", paths(&config));
        let tools: Vec<(Arc<dyn AgentTool>, bool)> = vec![
            (Arc::new(WorkspaceTool::read(workspace.clone())), true),
            (Arc::new(WorkspaceTool::list(workspace.clone())), true),
            (Arc::new(WorkspaceTool::write(workspace)), false),
        ];
        Ok(Self { model_host: defaults.model_host, steering: defaults.steering, tools, config })
    }

    /// A Run request built only with `RunRequest::new` and stable fields.
    pub fn request(&self, id: &str, objective: &str) -> RunRequest {
        let config = &self.config;
        let mut request = RunRequest::new(id, objective, paths(config));
        request.model_host = self.model_host.clone();
        request.steering = self.steering.clone();
        request.mode = RunMode::Execute;
        request.endpoint = config.endpoint.clone();
        request.token = config.token.clone();
        request.model = config.model.clone();
        request.route.model = config.model.clone();
        request.route.protocol = config.protocol.clone();
        request.device_fp = String::from("lite-consumer-check");
        request.session_key = String::from("lite-consumer-check/session");
        request.max_calls = config.run_max_calls;
        request.max_seconds = config.run_max_seconds;
        request.max_output = config.max_output;
        request.extra_tools = self
            .tools
            .iter()
            .map(|(tool, allow_in_plan)| ToolRegistration { tool: tool.clone(), allow_in_plan: *allow_in_plan })
            .collect();
        request
    }
}

fn paths(config: &Config) -> RuntimePaths {
    RuntimePaths::new(&config.workspace, config.state_dir.join("session.json"), config.state_dir.join("environment"))
}

fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(CHECK_TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[async_trait::async_trait]
impl HarnessHost for ConsumerHost {
    /// Runs the public check on the actual workspace; the model's text is not evidence.
    async fn verify(&self, candidate: Candidate, cancellation: CancellationToken) -> Result<ReviewReceipt> {
        let config = &self.config;
        let mut command = tokio::process::Command::new(&config.check[0]);
        command.args(&config.check[1..]).current_dir(&config.workspace).stdin(Stdio::null()).kill_on_drop(true);
        let output = tokio::select! {
            output = tokio::time::timeout(config.check_timeout, command.output()) => {
                output.context("public check timed out")?.context("public check could not run")?
            }
            _ = cancellation.cancelled() => bail!("public check cancelled"),
        };
        let passed = output.status.success();
        let record = config.state_dir.join("host").join(format!("check-run-{}.json", candidate.basis.run_number));
        let body = json!({
            "run_id": candidate.basis.run_id,
            "run_number": candidate.basis.run_number,
            "check": config.check,
            "exit_code": output.status.code(),
            "passed": passed,
            "stdout_tail": tail(&output.stdout),
            "stderr_tail": tail(&output.stderr),
        });
        std::fs::write(&record, serde_json::to_vec_pretty(&body)?).context("write check record")?;
        // Windows flushes only through a handle opened for writing.
        std::fs::OpenOptions::new().write(true).open(&record)?.sync_all().context("sync check record")?;
        let verdict = if passed {
            Verdict::Verified
        } else {
            Verdict::NeedsChanges {
                feedback: format!(
                    "The public check failed (exit code {:?}). Output tail:\n{}\n{}",
                    output.status.code(),
                    tail(&output.stdout),
                    tail(&output.stderr)
                ),
            }
        };
        Ok(ReviewReceipt {
            basis: candidate.basis,
            committed: true,
            settlement: Settlement::Settled,
            verdict,
            evidence: vec![record.display().to_string()],
        })
    }

    /// Same Session, same tools and authority; only a new Run ID and the feedback objective.
    async fn prepare_continuation(&self, continuation: Continuation, _: CancellationToken) -> Result<RunRequest> {
        let Verdict::NeedsChanges { feedback } = &continuation.receipt.verdict else {
            bail!("only NeedsChanges continues");
        };
        let id = format!("{}-run-{}", continuation.receipt.basis.task_id, continuation.next_run_number);
        let objective = format!(
            "Host review of the previous Run: {feedback}\nInspect the workspace files, repair them with the \
             workspace tools, then stop."
        );
        Ok(self.request(&id, &objective))
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Read,
    List,
    Write,
}

/// File tools confined to one workspace directory; this is a path check, not an OS sandbox.
pub struct WorkspaceTool {
    root: PathBuf,
    kind: Kind,
    schema: Tool,
}

impl WorkspaceTool {
    fn new(root: PathBuf, kind: Kind, name: &str, description: &str, parameters: Value) -> Self {
        let schema = Tool {
            name: name.into(),
            description: description.into(),
            parameters: Schema(parameters),
            constrained_sampling: None,
        };
        Self { root, kind, schema }
    }
    pub fn read(root: PathBuf) -> Self {
        Self::new(
            root,
            Kind::Read,
            "ws_read_file",
            "Read a UTF-8 text file in the workspace. `path` is relative to the workspace root.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        )
    }
    pub fn list(root: PathBuf) -> Self {
        Self::new(
            root,
            Kind::List,
            "ws_list_dir",
            "List a workspace directory. `path` is relative to the workspace root; use \".\" for the root.",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
        )
    }
    pub fn write(root: PathBuf) -> Self {
        Self::new(
            root,
            Kind::Write,
            "ws_write_file",
            "Create or replace a UTF-8 text file in the workspace with the full new content.",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}),
        )
    }

    /// Relative path made of normal components only, whose existing ancestor stays inside the root.
    fn resolve(&self, relative: &str) -> std::result::Result<PathBuf, String> {
        let path = Path::new(relative);
        if relative.is_empty()
            || !path.components().all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!("path must be relative to the workspace without '..': {relative}"));
        }
        // The canonical root may be a Windows verbatim path, which does not
        // normalize "." components, so only normal components are joined.
        let mut joined = self.root.clone();
        joined.extend(path.components().filter(|component| matches!(component, Component::Normal(_))));
        let mut existing = joined.as_path();
        while !existing.exists() {
            existing = existing.parent().ok_or("path has no existing ancestor")?;
        }
        let canonical = existing.canonicalize().map_err(|error| error.to_string())?;
        if !canonical.starts_with(&self.root) {
            return Err(format!("path leaves the workspace: {relative}"));
        }
        Ok(joined)
    }

    fn run(&self, args: &Value) -> std::result::Result<String, String> {
        let relative = args["path"].as_str().ok_or("path must be a string")?;
        let path = self.resolve(relative)?;
        match self.kind {
            Kind::Read => {
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                if bytes.len() > MAX_FILE_BYTES {
                    return Err(format!("file exceeds {MAX_FILE_BYTES} bytes"));
                }
                String::from_utf8(bytes).map_err(|_| "file is not UTF-8".to_string())
            }
            Kind::List => {
                let mut names = std::fs::read_dir(&path)
                    .map_err(|error| error.to_string())?
                    .map(|entry| {
                        let entry = entry.map_err(|error| error.to_string())?;
                        let suffix = if entry.path().is_dir() { "/" } else { "" };
                        Ok(format!("{}{suffix}", entry.file_name().to_string_lossy()))
                    })
                    .collect::<std::result::Result<Vec<_>, String>>()?;
                names.sort();
                Ok(names.join("\n"))
            }
            Kind::Write => {
                let content = args["content"].as_str().ok_or("content must be a string")?;
                if content.len() > MAX_FILE_BYTES {
                    return Err(format!("content exceeds {MAX_FILE_BYTES} bytes"));
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                std::fs::write(&path, content).map_err(|error| error.to_string())?;
                Ok(format!("wrote {} bytes to {relative}", content.len()))
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for WorkspaceTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> std::result::Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        self.run(&args).map(AgentToolResult::text).map_err(AgentError::Tool)
    }
}
