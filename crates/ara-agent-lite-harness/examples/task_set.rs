//! A responsive Host over two independent Lite Tasks, using the existing
//! capability Host and real artifact verification. No product installation.
//!
//! Usage: task_set CONFIG.json [--run-live]
//! CONFIG is an array of two capability_host configurations with distinct fresh
//! absolute roots. `kind: reasoning` needs no compiler. Default: controlled HTTP
//! provider, not a model capability test. --run-live uses the same environment
//! variables as capability_host and each config's explicit call/time limits.

#[path = "capability/host.rs"]
mod host;
#[path = "capability/provider.rs"]
mod provider;

use std::{path::PathBuf, sync::Arc};

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{task_set::TaskSet, TaskOptions, TaskStop};
use serde_json::json;

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(args.next().context("usage: task_set CONFIG.json [--run-live]")?);
    let live = match args.next() {
        None => false,
        Some(value) if value == "--run-live" => true,
        Some(_) => anyhow::bail!("only --run-live is accepted after CONFIG.json"),
    };
    ensure!(args.next().is_none(), "unexpected arguments");
    ensure!(std::fs::metadata(&path)?.len() <= 64 * 1024, "config exceeds 64 KiB");
    let configs: Vec<host::Config> = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(configs.len() == 2, "this example requires exactly two independent Tasks");
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        let mut hosts = Vec::new();
        let mut providers = Vec::new();
        for config in configs {
            let host = Arc::new(host::Host::prepare(config, live).await?);
            let controlled = if live { None } else { Some(provider::Provider::new(host.controlled_responses())?) };
            host.set_route(if let Some(provider) = &controlled {
                host::RouteConfig {
                    endpoint: provider.endpoint.clone(),
                    model: "capability-controlled".into(),
                    token: "controlled-only".into(),
                }
            } else {
                host::RouteConfig {
                    endpoint: std::env::var("ARA_CAPABILITY_MODEL_ENDPOINT").context("live endpoint env missing")?,
                    model: std::env::var("ARA_CAPABILITY_MODEL_ID").context("live model env missing")?,
                    token: std::env::var("ARA_CAPABILITY_MODEL_KEY").context("live credential env missing")?,
                }
            })?;
            hosts.push(host);
            providers.push(controlled);
        }
        let mut tasks = TaskSet::new(2, 8)?;
        let admission = (|| -> Result<()> {
            for (index, host) in hosts.iter().enumerate() {
                let mut spec = host.spec()?;
                spec.task_id = format!("background-{index}");
                tasks.start(spec, host.request(1, host.objective())?, host.clone(), TaskOptions::default())?;
                tasks.guide(
                    &format!("background-{index}"),
                    "host-followup-1",
                    "Keep the original task scope and verify the actual artifact before submission.",
                )?;
            }
            Ok(())
        })();
        if let Err(error) = admission {
            tasks.cancel_all();
            while tasks.next_completed().await.is_some() {}
            return Err(error);
        }
        // The Host can now return to its message loop. This line is emitted
        // before either Task is awaited, even on this single-threaded runtime.
        println!("{}", json!({"event":"host_available","dispatched":2}));
        let mut passed = true;
        let mut save_error = None;
        while let Some(completion) = tasks.next_completed().await {
            let outcome = &completion.outcome;
            let index = if outcome.task_id == "background-0" { 0 } else { 1 };
            let saved =
                host::save_new(&hosts[index].private.join("task-set-outcome.json"), &**outcome).and_then(|()| {
                    host::save_new(&hosts[index].private.join("task-set-guidance.json"), &completion.guidance)
                });
            if let Err(error) = saved {
                tasks.cancel_all();
                save_error.get_or_insert(error);
            }
            println!(
                "{}",
                json!({"event":"task_completed","task_id":outcome.task_id,
                "stop":outcome.stop,"calls":outcome.observed_calls,"task_accepted":outcome.task_accepted,
                "guidance":completion.guidance})
            );
            passed &= outcome.stop == TaskStop::AwaitingHostAcceptance;
        }
        if let Some(error) = save_error {
            return Err(error);
        }
        for (host, provider) in hosts.iter().zip(&providers) {
            if let Some(provider) = provider {
                host::save_new(&host.private.join("controlled-requests.json"), &*provider.requests.lock().unwrap())?;
            }
        }
        ensure!(passed, "a Task did not pass Host verification; inspect original receipts, do not replay");
        Ok(())
    })
}
