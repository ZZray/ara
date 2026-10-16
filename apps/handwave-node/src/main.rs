mod agent;
mod config;
mod connection;
mod context_memory;
mod execution;
mod experience;
mod file_discovery;
mod gateway;
mod goal_runtime;
mod job_control;
mod local_tools;
mod managed_jobs;
mod model_caps;
mod model_telemetry;
mod node_identity;
mod operations;
mod project_rules;
mod proxy;
mod script_tools;
mod session_journal;
mod system_capabilities;
mod terminal_emulator;
mod terminal_process;
mod transport;
mod transport_security;
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
