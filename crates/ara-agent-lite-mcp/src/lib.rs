//! MCP tool source for ARA Agent Lite hosts (requirement 42).
//!
//! Reads Claude Code `.mcp.json` server configuration, connects stdio and
//! Streamable HTTP servers concurrently, and hands their tools to the Host as
//! ordinary `AgentTool`s named `mcp__<server>__<tool>`. Core does not depend
//! on this crate; the agent loop never sees MCP.
pub mod config;
pub mod names;
pub mod text;
pub mod tool;

mod client;
mod resources;
mod transport;

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use ara_agent_lite_core::rpi_agent::AgentTool;
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

pub use client::{MAX_PAGES, MAX_TOOLS, SUPPORTED_PROTOCOL_VERSIONS};
pub use config::{ConfigError, McpConfig, ServerConfig};
pub use resources::{LIST_TOOL as LIST_RESOURCES_TOOL, READ_TOOL as READ_RESOURCE_TOOL};

use client::Client;

/// Connection options.
#[derive(Debug, Clone)]
pub struct McpOptions {
    /// Per-server bound for start, `initialize` and `tools/list` (default 30 s).
    pub startup_timeout: Duration,
    /// Per-call bound for `tools/call` and resource requests (default 600 s).
    pub call_timeout: Duration,
    /// Servers whose failure fails the whole connect.
    pub required: BTreeSet<String>,
    /// Where success results over 50000 characters are written in full.
    pub results_dir: Option<PathBuf>,
}

impl Default for McpOptions {
    fn default() -> Self {
        Self {
            startup_timeout: Duration::from_secs(30),
            call_timeout: Duration::from_secs(600),
            required: BTreeSet::new(),
            results_dir: None,
        }
    }
}

/// Per-server connection result. Reasons never contain configured values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ServerStatus {
    Connected { tools: usize, instructions: Option<String> },
    Failed { reason: String },
}

/// A server tool that is not offered to the agent, with the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExcludedTool {
    pub server: String,
    pub tool: String,
    pub reason: String,
}

/// Connect failure as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpError {
    /// `required` names a server that is not configured.
    UnknownRequired {
        server: String,
    },
    /// A `required` server failed; other servers were shut down.
    RequiredFailed {
        server: String,
        reason: String,
    },
    Cancelled,
}

impl McpError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownRequired { .. } => "unknown_required_server",
            Self::RequiredFailed { .. } => "required_server_failed",
            Self::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRequired { server } => write!(f, "required MCP server \"{server}\" is not configured"),
            Self::RequiredFailed { server, reason } => {
                write!(f, "required MCP server \"{server}\" failed: {reason}")
            }
            Self::Cancelled => write!(f, "MCP connect cancelled"),
        }
    }
}

impl std::error::Error for McpError {}

/// Connected MCP servers and the tools they contribute.
pub struct McpSession {
    clients: BTreeMap<String, Arc<Client>>,
    status: BTreeMap<String, ServerStatus>,
    tools: Vec<Arc<dyn AgentTool>>,
    excluded: Vec<ExcludedTool>,
}

type Connected = (Client, client::Listing);

async fn connect_one(
    server: &str,
    config: &ServerConfig,
    options: &McpOptions,
    cancel: &CancellationToken,
) -> Result<Connected, String> {
    let (expanded, redactor) =
        config::expand_with_secrets(server, config, &config::process_env).map_err(|error| error.to_string())?;
    let mut client = Client::open(server, &expanded, redactor)?;
    let started = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("connect cancelled".to_string()),
        // The outer bound owns the startup deadline; per-request bounds only backstop it.
        outcome = tokio::time::timeout(
            options.startup_timeout,
            client.handshake(options.startup_timeout + Duration::from_secs(1), cancel),
        ) => {
            outcome.unwrap_or_else(|_| Err(format!("startup timed out after {} s", options.startup_timeout.as_secs_f64())))
        }
    };
    match started {
        Ok(listing) => Ok((client, listing)),
        Err(reason) => {
            let reason = format!("{reason}{}", client.diagnostics().await);
            client.shutdown().await;
            Err(reason)
        }
    }
}

impl McpSession {
    /// Connect all configured servers concurrently.
    pub async fn connect(config: McpConfig, options: McpOptions, cancel: CancellationToken) -> Result<Self, McpError> {
        if let Some(server) = options.required.iter().find(|server| !config.mcp_servers.contains_key(*server)) {
            return Err(McpError::UnknownRequired { server: server.clone() });
        }
        let attempts = config.mcp_servers.iter().map(|(server, entry)| {
            let (options, cancel) = (&options, &cancel);
            async move { (server.clone(), connect_one(server, entry, options, cancel).await) }
        });
        let outcomes = futures::future::join_all(attempts).await;
        let mut status = BTreeMap::new();
        let mut connected = Vec::new();
        let mut failure = None;
        for (server, outcome) in outcomes {
            match outcome {
                Ok(pair) => connected.push((server, pair)),
                Err(reason) => {
                    if failure.is_none() && options.required.contains(&server) {
                        failure = Some(McpError::RequiredFailed { server: server.clone(), reason: reason.clone() });
                    }
                    status.insert(server, ServerStatus::Failed { reason });
                }
            }
        }
        let failure = if cancel.is_cancelled() { Some(McpError::Cancelled) } else { failure };
        if let Some(failure) = failure {
            futures::future::join_all(connected.iter().map(|(_, (client, _))| client.shutdown())).await;
            return Err(failure);
        }
        let mut names = names::NameRegistry::default();
        let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
        let mut excluded = Vec::new();
        let mut clients = BTreeMap::new();
        let mut resource_servers = BTreeMap::new();
        for (server, (client, listing)) in connected {
            let client = Arc::new(client);
            let mut count = 0;
            for definition in &listing.tools {
                let original = definition.get("name").and_then(Value::as_str).unwrap_or_default();
                let mut exclude = |reason: String| {
                    excluded.push(ExcludedTool {
                        server: server.clone(),
                        tool: text::bounded(&text::sanitize(original), 200),
                        reason,
                    })
                };
                let schema = match definition.get("inputSchema").map(names::flatten_schema) {
                    Some(Ok(schema)) => schema,
                    Some(Err(reason)) => {
                        exclude(reason);
                        continue;
                    }
                    None => {
                        exclude("missing inputSchema".into());
                        continue;
                    }
                };
                let full = match names.assign(&server, original) {
                    Ok(full) => full,
                    Err(reason) => {
                        exclude(reason);
                        continue;
                    }
                };
                let read_only = definition.pointer("/annotations/readOnlyHint").and_then(Value::as_bool) == Some(true);
                let description = names::description(
                    schema.1.as_deref(),
                    definition.get("description").and_then(Value::as_str).unwrap_or_default(),
                );
                tools.push(Arc::new(tool::McpTool {
                    schema: tool::schema(full, description, schema.0),
                    server: server.clone(),
                    tool: original.to_string(),
                    read_only,
                    client: client.clone(),
                    call_timeout: options.call_timeout,
                    results_dir: options.results_dir.clone(),
                }));
                count += 1;
            }
            if let Some(reason) = listing.truncated {
                excluded.push(ExcludedTool { server: server.clone(), tool: "*".into(), reason });
            }
            if client.has_capability("resources") {
                resource_servers.insert(server.clone(), client.clone());
            }
            status.insert(
                server.clone(),
                ServerStatus::Connected { tools: count, instructions: client.instructions.clone() },
            );
            clients.insert(server, client);
        }
        tools.extend(resources::tools(resource_servers, options.call_timeout, options.results_dir.clone()));
        Ok(Self { clients, status, tools, excluded })
    }

    /// MCP tools plus the resource tools when any server declares `resources`.
    pub fn tools(&self) -> Vec<Arc<dyn AgentTool>> {
        self.tools.clone()
    }

    pub fn status(&self) -> &BTreeMap<String, ServerStatus> {
        &self.status
    }

    pub fn excluded(&self) -> &[ExcludedTool] {
        &self.excluded
    }

    /// Close every connection: stdin is closed, the server gets a short
    /// grace period, then its process tree is ended. HTTP sessions get DELETE.
    pub async fn shutdown(self) {
        futures::future::join_all(self.clients.values().map(|client| client.shutdown())).await;
    }
}
