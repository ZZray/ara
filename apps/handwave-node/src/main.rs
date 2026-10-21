pub use ara_agent_lite_core::agent;
mod cloud_context;
mod config;
mod connection;
pub use ara_agent_lite_core::context_memory;
pub use ara_agent_lite_core::environment_library;
pub use ara_agent_lite_core::environment_tools;
pub use ara_agent_lite_core::execution;
pub use ara_agent_lite_core::experience;
pub use ara_agent_lite_core::file_discovery;
mod gateway;
mod goal_runtime;
mod job_control;
pub use ara_agent_lite_core::local_tools;
pub use ara_agent_lite_core::managed_jobs;
pub use ara_agent_lite_core::model_caps;
pub use ara_agent_lite_core::model_telemetry;
mod node_identity;
mod operations;
pub use ara_agent_lite_core::project_rules;
mod proxy;
pub use ara_agent_lite_core::script_tools;
pub use ara_agent_lite_core::session_journal;
pub use ara_agent_lite_core::skill_library;
pub use ara_agent_lite_core::system_capabilities;
pub use ara_agent_lite_core::terminal_emulator;
pub use ara_agent_lite_core::terminal_process;
mod transport;
mod transport_security;
pub use ara_agent_lite_core::web_tools;
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
