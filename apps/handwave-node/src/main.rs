mod agent;
mod config;
mod context_memory;
mod gateway;
mod local_tools;
mod operations;
mod project_rules;
mod proxy;
mod transport;
mod wire;

use clap::Parser;

fn main() -> anyhow::Result<()> {
    let settings = config::Resolved::load(config::Config::parse())?;
    println!("HandWave Rust node {} | device {}", env!("CARGO_PKG_VERSION"), settings.fp);
    if settings.config.identity {
        return Ok(());
    }
    println!("Waiting for approval in the HandWave cloud workbench. Press Ctrl+C to stop.");
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(transport::run(settings))
}
