#![allow(dead_code)]
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use ara_agent_lite_core::rpi_agent::{AgentError, AgentTool, AgentToolResult, TextContentOrImage};
use ara_agent_lite_mcp::{McpConfig, McpOptions, McpSession, ServerConfig};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("fake_mcp_server.py")
}

/// The fake server with `args`; `log` receives one line per received message.
pub fn fake(args: &[&str], log: Option<&Path>, env: &[(&str, &str)]) -> ServerConfig {
    let mut env: BTreeMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    if let Some(log) = log {
        env.insert("FAKE_MCP_LOG".into(), log.display().to_string());
    }
    let mut all = vec![fixture().display().to_string()];
    all.extend(args.iter().map(|arg| arg.to_string()));
    ServerConfig::Stdio { command: "python".into(), args: all, env }
}

pub fn config(servers: Vec<(&str, ServerConfig)>) -> McpConfig {
    McpConfig { mcp_servers: servers.into_iter().map(|(name, server)| (name.to_string(), server)).collect() }
}

pub fn options() -> McpOptions {
    McpOptions { startup_timeout: Duration::from_secs(20), ..Default::default() }
}

pub async fn connect(config: McpConfig, options: McpOptions) -> McpSession {
    McpSession::connect(config, options, CancellationToken::new()).await.expect("connect")
}

pub fn tool(session: &McpSession, name: &str) -> Arc<dyn AgentTool> {
    session.tools().into_iter().find(|tool| tool.schema().name == name).unwrap_or_else(|| panic!("tool {name} missing"))
}

pub fn names(session: &McpSession) -> Vec<String> {
    session.tools().iter().map(|tool| tool.schema().name.clone()).collect()
}

pub async fn run(
    tool: &Arc<dyn AgentTool>,
    args: Value,
    cancel: CancellationToken,
) -> Result<AgentToolResult, AgentError> {
    tool.execute("call-1", args, cancel, Arc::new(|_| {})).await
}

pub fn text_of(result: &AgentToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            TextContentOrImage::Text(text) => Some(text.text.as_str()),
            TextContentOrImage::Image(_) => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn error_text(result: Result<AgentToolResult, AgentError>) -> String {
    match result {
        Err(AgentError::Tool(text)) => text,
        other => panic!("expected a tool error, got {other:?}"),
    }
}

pub fn log_lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().map(str::to_string).collect()
}

pub fn count(lines: &[String], prefix: &str) -> usize {
    lines.iter().filter(|line| line.starts_with(prefix)).count()
}

/// Poll the log until `prefix` appears (bounded).
pub async fn wait_for(path: &Path, prefix: &str) -> Vec<String> {
    for _ in 0..100 {
        let lines = log_lines(path);
        if count(&lines, prefix) > 0 {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("log never contained {prefix}: {:?}", log_lines(path));
}
