//! Independent embedding example: no HandWave transport, identity or configuration.
//! Usage: business_host request.json. Credentials come from the named environment variable.
use anyhow::{ensure, Context, Result};
use ara_agent_lite_core::{
    execute, goal_runtime, managed_jobs,
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::{Schema, Tool},
    RunMode, RunRequest, RuntimePaths, ToolRegistration,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
struct Request {
    id: String,
    objective: String,
    cwd: PathBuf,
    session_path: PathBuf,
    environment_state: PathBuf,
    #[serde(default)]
    endpoint: String,
    #[serde(default)]
    model: String,
    #[serde(default)]
    token_env: String,
    #[serde(default)]
    mode: String,
    #[serde(default)]
    enable_local_tools: bool,
    #[serde(default)]
    enable_worker: bool,
    #[serde(default)]
    action: String,
    #[serde(default)]
    extension_name: String,
    #[serde(default)]
    extension_denied_in_plan: bool,
    #[serde(default)]
    cancel_after_ms: Option<u64>,
    #[serde(default)]
    close_events: bool,
}

struct BusinessLookup {
    schema: Tool,
}
#[async_trait]
impl AgentTool for BusinessLookup {
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
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if args.as_object().is_none_or(|o| !o.is_empty()) {
            return Err(AgentError::Tool("business_lookup accepts an empty object".into()));
        }
        // Replace with a business-owned, separately authorized service call.
        Ok(AgentToolResult::text("BUSINESS_LOOKUP_OK"))
    }
}

fn main() -> Result<()> {
    // This must precede ordinary CLI/configuration parsing and business startup.
    if let Some(result) = managed_jobs::worker_entry_from_args() {
        return result;
    }
    let args: Vec<_> = std::env::args_os().collect();
    ensure!(args.len() == 2, "usage: business_host request.json");
    let bytes = std::fs::read(&args[1])?;
    ensure!(bytes.len() <= 64 * 1024, "example request exceeds 64 KiB");
    let config: Request = serde_json::from_slice(&bytes)?;
    tokio::runtime::Builder::new_multi_thread().enable_all().build()?.block_on(run(config))
}

async fn run(config: Request) -> Result<()> {
    if matches!(config.action.as_str(), "job_smoke" | "pty_smoke") {
        return job_smoke(&config).await;
    }
    let mut request = RunRequest::new(
        &config.id,
        &config.objective,
        RuntimePaths::new(&config.cwd, &config.session_path, &config.environment_state),
    );
    request.mode = if config.mode.is_empty() { RunMode::Plan } else { RunMode::parse(&config.mode)? };
    request.enable_local_tools = config.enable_local_tools;
    request.worker_executable = config.enable_worker.then(std::env::current_exe).transpose()?;
    request.endpoint = config.endpoint;
    request.model = config.model.clone();
    request.route.model = config.model;
    request.route.protocol = "openai-completions".into();
    request.token = std::env::var(&config.token_env).context("set the configured model-token environment variable")?;
    request.max_calls = 8;
    request.max_seconds = 45;
    request.max_output = 2048;
    request.device_fp = "business-example".into();
    request.session_key = "business/session".into();
    if !config.extension_name.is_empty() {
        request.extra_tools.push(ToolRegistration {
            tool: Arc::new(BusinessLookup {
                schema: Tool {
                    name: config.extension_name,
                    description: "Read this example's business-owned status.".into(),
                    parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                    constrained_sampling: None,
                },
            }),
            allow_in_plan: !config.extension_denied_in_plan,
        });
    }
    let cancellation = CancellationToken::new();
    let timer = config.cancel_after_ms.map(|millis| {
        let signal = cancellation.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(millis)).await;
            signal.cancel();
        })
    });
    let (events, mut receiver) = mpsc::channel(64);
    let reader = if config.close_events {
        drop(receiver);
        None
    } else {
        // Drain while execute is running. Awaiting execute before reading can
        // stall the bounded channel and makes tool effects uncertain.
        Some(tokio::spawn(async move {
            while let Some(event) = receiver.recv().await {
                println!("{}", json!({"kind":"event","event":event}));
                let _ = std::io::stdout().flush();
            }
        }))
    };
    let result = execute(request, cancellation, events).await;
    if let Some(timer) = timer {
        timer.abort();
    }
    if let Some(reader) = reader {
        reader.await?;
    }
    let result = result?;
    println!("{}", json!({"kind":"result","code":result.code,"text":result.text,"error":result.error}));
    ensure!(result.code == 0, "Run did not complete successfully");
    Ok(())
}

async fn job_smoke(config: &Request) -> Result<()> {
    std::fs::create_dir_all(&config.environment_state)?;
    let marker = config.cwd.join("business-startup.txt");
    std::fs::OpenOptions::new().create(true).append(true).open(marker)?.write_all(b"startup\n")?;
    let manager = managed_jobs::Manager::new(
        &config.environment_state,
        &config.cwd,
        managed_jobs::Policy {
            owner: managed_jobs::Owner::new(&config.environment_state, "business-example", "business/session")?,
            goal_id: "example-goal".into(),
            lease_expires_at: goal_runtime::now() + 60,
            max_jobs: 4,
            max_log_bytes: 64 * 1024,
            max_seconds: 20,
            worker_executable: config.enable_worker.then(std::env::current_exe).transpose()?,
        },
    )?;
    let terminal = config.action == "pty_smoke";
    let script = if terminal {
        "print('READY',flush=True); value=input(); print('READ:'+value,flush=True)"
    } else {
        "from pathlib import Path; p=Path('job-effect.txt'); p.open('a').write('once\\n'); print('JOB_OK',flush=True)"
    };
    let mut args = json!({"op_id":"example-operation","language":"python","script":script,
        "cwd":config.cwd,"timeout":15,"interactive":terminal,"terminal":terminal});
    if terminal {
        args["rows"] = json!(24);
        args["cols"] = json!(80);
    }
    let first = manager.start(&args).await?;
    let duplicate = manager.start(&args).await?;
    ensure!(duplicate.job_id == first.job_id, "duplicate operation changed identity");
    if terminal {
        manager.write_input(&first.job_id, "input-one", "hello\r", false).await?;
        manager.write_input(&first.job_id, "input-one", "hello\r", false).await?;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let receipt = manager.status(&first.job_id)?;
        if receipt.finished_at.is_some() {
            let log = manager.read_log(&first.job_id, "stdout", 0, 8192)?;
            println!("{}", json!({"kind":"job_result","receipt":receipt,"log":log}));
            ensure!(receipt.state == "succeeded", "worker did not succeed: {}", receipt.reason);
            return Ok(());
        }
        ensure!(tokio::time::Instant::now() < deadline, "example worker observation deadline");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
