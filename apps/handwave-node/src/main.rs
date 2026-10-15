mod agent;
mod config;
mod context_memory;
mod execution;
mod experience;
mod file_discovery;
mod gateway;
mod goal_runtime;
mod local_tools;
mod managed_jobs;
mod model_caps;
mod model_telemetry;
mod operations;
mod project_rules;
mod proxy;
mod script_tools;
mod session_journal;
mod system_capabilities;
mod transport;
mod wire;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    if let Some(result) = managed_jobs::worker_entry_from_args() {
        return result;
    }
    let settings = config::Resolved::load(config::Config::parse())?;
    println!("HandWave Rust node {} | device {}", env!("CARGO_PKG_VERSION"), settings.fp);
    if settings.config.identity {
        return Ok(());
    }
    println!("Waiting for approval in the HandWave cloud workbench. Press Ctrl+C to stop.");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(transport::run(settings))
}
