//! `ara-fake-upstream --script <file> [--record <file>] [--port-file <file>]`
//!
//! Serves the script on 127.0.0.1 (random port), prints `http://127.0.0.1:<port>/v1`
//! on stdout, and runs until killed.

use ara_testkit::{FakeUpstream, Script};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (mut script, mut record, mut port_file) = (None, None, None);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--script" => script = args.next().map(PathBuf::from),
            "--record" => record = args.next().map(PathBuf::from),
            "--port-file" => port_file = args.next().map(PathBuf::from),
            other => anyhow::bail!("unknown argument {other}"),
        }
    }
    let script_path = script.ok_or_else(|| anyhow::anyhow!("--script is required"))?;
    let script: Script = serde_json::from_str(&std::fs::read_to_string(&script_path)?)?;
    let server = FakeUpstream::start(script, record).await?;
    let url = server.base_url();
    if let Some(path) = port_file {
        std::fs::write(path, &url)?;
    }
    println!("{url}");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
