//! Runs one Task through the Harness `start` entry and prints the outcome as JSON.
//!
//! The model credential is read from the environment variable named by
//! `--credential-env` and is never printed. The exit code says only whether
//! the Host's public check accepted the last Run; the task Oracle decides
//! pass or fail outside this process (requirement 31.3).
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration, time::Instant};

use anyhow::{bail, Context, Result};
use ara_agent_lite_harness::{start, TaskKind, TaskSpec, TaskStop};
use lite_consumer_check::{Config, ConsumerHost};

const USAGE: &str = "usage: lite-consumer-check --task-id ID --workspace DIR --state-dir DIR --evidence-dir NEW_DIR \
--endpoint URL --model ID --protocol NAME --credential-env VAR --objective TEXT \
--max-runs N --max-calls N --deadline-seconds N --run-max-calls N --run-max-seconds N --max-output N \
--check-timeout-seconds N -- CHECK_PROGRAM [CHECK_ARGS...]";

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let mut check = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--" {
            check.extend(args.by_ref());
            break;
        }
        let Some(key) = arg.strip_prefix("--") else { bail!("unexpected argument {arg}\n{USAGE}") };
        let value = args.next().with_context(|| format!("--{key} needs a value\n{USAGE}"))?;
        if options.insert(key.to_string(), value).is_some() {
            bail!("--{key} given twice");
        }
    }
    let mut take = |key: &str| options.remove(key).with_context(|| format!("missing --{key}\n{USAGE}"));
    let number = |text: String, key: &str| text.parse::<u64>().with_context(|| format!("--{key} must be a number"));

    let task_id = take("task-id")?;
    let workspace = PathBuf::from(take("workspace")?);
    let state_dir = PathBuf::from(take("state-dir")?);
    let evidence_dir = PathBuf::from(take("evidence-dir")?);
    let endpoint = take("endpoint")?;
    let model = take("model")?;
    let protocol = take("protocol")?;
    let credential_env = take("credential-env")?;
    let objective = take("objective")?;
    let max_runs = number(take("max-runs")?, "max-runs")? as usize;
    let max_calls = number(take("max-calls")?, "max-calls")? as usize;
    let deadline_seconds = number(take("deadline-seconds")?, "deadline-seconds")?;
    let run_max_calls = number(take("run-max-calls")?, "run-max-calls")? as usize;
    let run_max_seconds = number(take("run-max-seconds")?, "run-max-seconds")?;
    let max_output = u32::try_from(number(take("max-output")?, "max-output")?).context("--max-output too large")?;
    let check_timeout = Duration::from_secs(number(take("check-timeout-seconds")?, "check-timeout-seconds")?);
    if let Some(key) = options.keys().next() {
        bail!("unknown option --{key}\n{USAGE}");
    }
    let token = std::env::var(&credential_env)
        .with_context(|| format!("credential environment variable {credential_env} is not set"))?;

    let host = Arc::new(ConsumerHost::new(Config {
        workspace,
        state_dir,
        endpoint,
        model,
        protocol,
        token,
        run_max_calls,
        run_max_seconds,
        max_output,
        check,
        check_timeout,
    })?);
    let now_ms = u64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_millis())?;
    let spec = TaskSpec {
        task_id: task_id.clone(),
        criteria_revision: String::from("consumer-public-check-v1"),
        authorization_revision: String::from("consumer-workspace-tools-v1"),
        kind: TaskKind::Coding,
        evidence_dir,
        max_runs,
        max_calls,
        deadline_unix_ms: now_ms + deadline_seconds * 1000,
    };
    let started = Instant::now();
    let request = host.request(&format!("{task_id}-run-1"), &objective);
    let handle = start(spec, request, host)?;
    let outcome = handle.wait().await;
    let summary = serde_json::json!({
        "wall_ms": u64::try_from(started.elapsed().as_millis())?,
        "outcome": serde_json::to_value(&*outcome)?,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);
    std::process::exit(if outcome.stop == TaskStop::AwaitingHostAcceptance { 0 } else { 1 });
}
