//! Independent consumer of the public Lite Harness. No product or Full Core dependency.
//!
//! Usage: capability_host CONFIG.json [--run-live]
//! Config: {"kind":"coding|reasoning|reverse","root":"ABSENT_ABSOLUTE_DIRECTORY",
//! "compiler":"ABSOLUTE_GNU_CXX","objdump":"ABSOLUTE_GNU_OBJDUMP",
//! "strip":"ABSOLUTE_GNU_STRIP","max_calls":24,"max_runs":2,"max_seconds":240}
//! compiler is needed for coding/reverse; objdump/strip only for reverse.
//! Live credentials: ARA_CAPABILITY_MODEL_ENDPOINT / ARA_CAPABILITY_MODEL_ID /
//! ARA_CAPABILITY_MODEL_KEY. Without --run-live only the controlled provider runs.
//! Controlled replies exercise real tools and verification, not model capability.
//! Tool grants and the private Host directory are not an OS sandbox.

#[path = "capability/host.rs"]
mod host;
#[path = "capability/provider.rs"]
mod provider;

use anyhow::{ensure, Context, Result};
use ara_agent_lite_harness::{start, TaskStop};
use serde_json::json;
use std::{path::PathBuf, sync::Arc};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let config_path = PathBuf::from(args.next().context("usage: capability_host CONFIG.json [--run-live]")?);
    let live = match args.next() {
        None => false,
        Some(value) if value == "--run-live" => true,
        Some(_) => anyhow::bail!("only --run-live is accepted after the config path"),
    };
    ensure!(args.next().is_none(), "unexpected arguments");
    ensure!(std::fs::metadata(&config_path)?.len() <= 64 * 1024, "config exceeds 64 KiB");
    let config: host::Config = serde_json::from_slice(&std::fs::read(config_path)?)?;
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
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
        let spec = host.spec()?;
        let initial = host.request(1, host.objective())?;
        let task = start(spec, initial, host.clone())?;
        let outcome = task.wait().await;
        if let Some(provider) = &controlled {
            host::save_new(&host.private.join("controlled-requests.json"), &*provider.requests.lock().unwrap())?;
        }
        host::save_new(&host.private.join("consumer-outcome.json"), &*outcome)?;
        println!(
            "{}",
            json!({"mode":if live {"live"} else {"controlled"},"kind":host.config.kind,
            "root":host.config.root,"stop":outcome.stop,"runs":outcome.runs,
            "calls":outcome.observed_calls,"task_accepted":outcome.task_accepted,
            "process_effects_unknown":host.processes.unknown(),"evidence":host.config.root.join("harness")})
        );
        ensure!(
            outcome.stop == TaskStop::AwaitingHostAcceptance,
            "task did not pass Host verification; inspect original receipts (no automatic replay)"
        );
        Ok(())
    })
}
