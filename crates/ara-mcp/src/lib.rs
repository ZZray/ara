//! A bounded, host-owned MCP stdio adapter for the fixed OMP MCP surface.
//! Upstream: `packages/coding-agent/src/mcp/{client,tool-bridge,transports/stdio}.ts`
//! at 596f2da7101178214aa27a753529d15e6b7ad91d.
//!
//! Intentional first-slice differences: explicit per-tool grants, no ambient
//! credentials, no reconnect/replay, and direct executable launch only.

use ara_agent::{AgentTool, Concurrency, ToolError, ToolOutput, UpdateFn};
use ara_ai::{ImageContent, JsonObject, Tool, UserBlock};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[cfg(windows)]
mod windows_child;

const FRAME_MAX: usize = 1024 * 1024;
const RESULT_MAX: usize = 256 * 1024;
const SCHEMA_MAX: usize = 64 * 1024;
const MAX_PAGES: usize = 16;
const MAX_TOOLS: usize = 64;
const MAX_INTERLEAVED: usize = 64;
const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The CLI host approves launching one direct executable and explicitly maps
/// child environment names to parent environment variable names. Values never
/// appear in this file or in diagnostics.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub name: String,
    pub command: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env_from_host: BTreeMap<String, String>,
}

impl ServerConfig {
    pub fn from_file(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|e| format!("reading MCP config {}: {e}", path.display()))?;
        let mut bytes = Vec::new();
        file.take(64 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("reading MCP config {}: {e}", path.display()))?;
        if bytes.len() > 64 * 1024 {
            return Err("MCP config exceeds 64 KiB".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid MCP config {}: {e}", path.display()))
    }

    fn validate(&self) -> Result<(), String> {
        if !valid_part(&self.name) {
            return Err("MCP server name must contain only ASCII letters, digits, `_` or `-`".into());
        }
        if !self.command.is_absolute() || !self.command.is_file() {
            return Err("MCP command must be an existing absolute executable path".into());
        }
        if cfg!(windows)
            && self.command.extension().and_then(|s| s.to_str()).is_none_or(|s| !s.eq_ignore_ascii_case("exe"))
        {
            return Err("MCP stdio currently requires a direct .exe; .cmd/.bat launch is deferred".into());
        }
        if self.args.len() > 64 || self.args.iter().any(|a| a.len() > 8192) {
            return Err("MCP command arguments exceed the configured limit".into());
        }
        for (child, parent) in &self.env_from_host {
            if !valid_env_name(child) || !valid_env_name(parent) {
                return Err("MCP environment mapping has an invalid variable name".into());
            }
        }
        Ok(())
    }
}

fn valid_part(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_env_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

struct Connection {
    child: Child,
    #[cfg(windows)]
    job: Option<windows_child::Job>,
    stdin: ChildStdin,
    stdout: ChildStdout,
    pending: Vec<u8>,
    next_id: u64,
    healthy: bool,
}

impl Connection {
    async fn write(&mut self, message: &Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(message).map_err(|e| e.to_string())?;
        if bytes.len() >= FRAME_MAX {
            return Err("MCP outbound frame exceeds 1 MiB".into());
        }
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await.map_err(|e| format!("MCP stdin write failed: {e}"))?;
        self.stdin.flush().await.map_err(|e| format!("MCP stdin flush failed: {e}"))
    }

    async fn read(&mut self) -> Result<Value, String> {
        loop {
            if let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
                let frame: Vec<u8> = self.pending.drain(..=end).collect();
                if frame.len() > FRAME_MAX {
                    return Err("MCP inbound frame exceeds 1 MiB".into());
                }
                return serde_json::from_slice(&frame).map_err(|e| format!("invalid MCP JSON-RPC frame: {e}"));
            }
            if self.pending.len() >= FRAME_MAX {
                return Err("MCP inbound frame exceeds 1 MiB".into());
            }
            let mut buf = [0u8; 4096];
            let n = self.stdout.read(&mut buf).await.map_err(|e| format!("MCP stdout read failed: {e}"))?;
            if n == 0 {
                return Err("MCP server closed stdout before replying".into());
            }
            self.pending.extend_from_slice(&buf[..n]);
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await?;
        for _ in 0..MAX_INTERLEAVED {
            let reply = self.read().await?;
            if reply.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                return Err("MCP server sent an invalid JSON-RPC version".into());
            }
            if let Some(server_method) = reply.get("method").and_then(Value::as_str) {
                if let Some(request_id) = reply.get("id") {
                    // No roots, sampling or elicitation capability is advertised.
                    self.write(&json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32601,"message":format!("Unsupported client method: {server_method}")}})).await?;
                }
                continue;
            }
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                return Err("MCP server replied with an unexpected request ID".into());
            }
            if let Some(error) = reply.get("error") {
                let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
                let message = error.get("message").and_then(Value::as_str).unwrap_or("unknown error");
                return Err(format!("MCP JSON-RPC error {code}: {message}"));
            }
            return reply.get("result").cloned().ok_or_else(|| "MCP response has no result".into());
        }
        Err("MCP server sent too many messages before the response".into())
    }

    fn stop(&mut self) {
        self.healthy = false;
        #[cfg(windows)]
        drop(self.job.take());
        let _ = self.child.start_kill();
    }
}

pub struct McpTool {
    definition: Tool,
    server: String,
    original_name: String,
    connection: Arc<Mutex<Connection>>,
}

#[async_trait]
impl AgentTool for McpTool {
    fn definition(&self) -> Tool {
        self.definition.clone()
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn execute(
        &self,
        _call_id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        if cancel.is_cancelled() {
            return Ok(ToolOutput::error("MCP tool cancelled before dispatch"));
        }
        let mut connection = tokio::select! {
            _ = cancel.cancelled() => return Ok(ToolOutput::error("MCP tool cancelled before dispatch")),
            guard = self.connection.lock() => guard,
        };
        if !connection.healthy {
            return Ok(ToolOutput::error("MCP server unavailable; the call was not dispatched"));
        }
        let request = connection.request("tools/call", json!({"name":self.original_name,"arguments":args}));
        let result = tokio::select! {
            _ = cancel.cancelled() => Err("MCP tool cancelled after dispatch; effect unknown".to_owned()),
            result = tokio::time::timeout(CALL_TIMEOUT, request) => match result {
                Ok(result) => result,
                Err(_) => Err("MCP tool timed out after dispatch; effect unknown".to_owned()),
            },
        };
        match result {
            Ok(value) => match parse_tool_result(&value, &self.server, &self.original_name) {
                Ok(output) => Ok(output),
                Err(error) => {
                    connection.stop();
                    Ok(unknown_effect(error))
                }
            },
            Err(error) => {
                connection.stop();
                Ok(unknown_effect(error))
            }
        }
    }
}

fn unknown_effect(message: String) -> ToolOutput {
    ToolOutput::error(format!("MCP call effect unknown; do not replay automatically: {message}")).with_details(
        json!({"__synthetic":true,"source":"interrupted_unknown_effect","executed":"unknown","transport":"mcp_stdio"}),
    )
}

fn parse_tool_result(value: &Value, server: &str, tool: &str) -> Result<ToolOutput, String> {
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > RESULT_MAX {
        return Err("MCP tool result exceeds 256 KiB; effect unknown".into());
    }
    let content =
        value.get("content").and_then(Value::as_array).ok_or("MCP tool result has no content array; effect unknown")?;
    let mut blocks = Vec::new();
    for part in content {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => blocks
                .push(UserBlock::text(part.get("text").and_then(Value::as_str).ok_or("MCP text content is invalid")?)),
            Some("image") => blocks.push(UserBlock::Image(ImageContent {
                data: part.get("data").and_then(Value::as_str).ok_or("MCP image data is invalid")?.to_owned(),
                mime_type: part
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .ok_or("MCP image MIME type is invalid")?
                    .to_owned(),
            })),
            Some("resource") => {
                let resource = part.get("resource").ok_or("MCP resource content is invalid")?;
                let uri = resource.get("uri").and_then(Value::as_str).ok_or("MCP resource URI is invalid")?;
                let text = resource.get("text").and_then(Value::as_str).unwrap_or("");
                let rendered =
                    if text.is_empty() { format!("[Resource: {uri}]") } else { format!("[Resource: {uri}]\n{text}") };
                blocks.push(UserBlock::text(rendered));
            }
            Some(other) => blocks.push(UserBlock::text(format!("[Unsupported MCP content type: {other}]"))),
            None => return Err("MCP content type is missing; effect unknown".into()),
        }
    }
    if let Some(structured) = value.get("structuredContent") {
        if !structured.is_object() {
            return Err("MCP structuredContent is invalid; effect unknown".into());
        }
        let duplicated = content.iter().any(|part| {
            part.get("type").and_then(Value::as_str) == Some("text")
                && part
                    .get("text")
                    .and_then(Value::as_str)
                    .and_then(|s| serde_json::from_str::<Value>(s.trim()).ok())
                    .as_ref()
                    == Some(structured)
        });
        if !duplicated {
            let formatted = serde_json::to_string_pretty(structured).map_err(|e| e.to_string())?;
            blocks.push(UserBlock::text(format!("```json\n{formatted}\n```")));
        }
    }
    if blocks.is_empty() {
        blocks.push(UserBlock::text(""));
    }
    let is_error = value.get("isError").and_then(Value::as_bool).unwrap_or(false);
    if is_error {
        match blocks.first_mut() {
            Some(UserBlock::Text(text)) => text.text.insert_str(0, "Error: "),
            _ => blocks.insert(0, UserBlock::text("Error:")),
        }
    }
    Ok(ToolOutput {
        content: blocks,
        details: Some(json!({"source":"mcp_stdio","server":server,"tool":tool})),
        is_error,
    })
}

/// Launch an explicitly configured server, initialize it, and expose only
/// exact `(server, original tool name)` grants. The returned tools own the
/// child; dropping them ends the direct child process.
pub async fn connect(
    config: ServerConfig,
    cwd: &Path,
    allow: &[String],
    reserved_names: &[&str],
) -> Result<Vec<Arc<dyn AgentTool>>, String> {
    config.validate()?;
    if allow.is_empty() {
        return Err("MCP config requires at least one --mcp-allow server:tool grant".into());
    }
    let mut grants = HashSet::new();
    for grant in allow {
        let Some((server, tool)) = grant.split_once(':') else {
            return Err("--mcp-allow must be server:tool".into());
        };
        if server != config.name || tool.is_empty() {
            return Err(format!("MCP grant {grant:?} does not name this server and a tool"));
        }
        grants.insert(tool.to_owned());
    }
    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args)
        .current_dir(cwd)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // The Windows loader and runtime need their OS installation path. No
    // provider credentials or general PATH are inherited implicitly.
    if cfg!(windows) {
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                cmd.env(name, value);
            }
        }
    }
    for (child, parent) in &config.env_from_host {
        let value = std::env::var_os(parent).ok_or_else(|| format!("MCP environment source {parent} is not set"))?;
        cmd.env(child, value);
    }
    #[cfg(windows)]
    let job = windows_child::Job::new().map_err(|e| format!("creating MCP process job: {e}"))?;
    let mut child = cmd.spawn().map_err(|e| format!("starting MCP server {}: {e}", config.name))?;
    #[cfg(windows)]
    if let Err(error) = job.assign(&child) {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(format!("assigning MCP server to Windows process job: {error}"));
    }
    let stdin = child.stdin.take().ok_or("MCP child has no stdin")?;
    let stdout = child.stdout.take().ok_or("MCP child has no stdout")?;
    let mut connection = Connection {
        child,
        #[cfg(windows)]
        job: Some(job),
        stdin,
        stdout,
        pending: Vec::new(),
        next_id: 1,
        healthy: true,
    };
    let catalog = tokio::time::timeout(START_TIMEOUT, async {
        let initialized = connection
            .request(
                "initialize",
                json!({
                    "protocolVersion":"2025-11-25",
                    "capabilities":{},
                    "clientInfo":{"name":"ara","version":env!("CARGO_PKG_VERSION")}
                }),
            )
            .await?;
        if initialized.get("protocolVersion").and_then(Value::as_str) != Some("2025-11-25") {
            return Err("MCP server did not negotiate pinned protocol 2025-11-25".into());
        }
        if initialized.get("capabilities").and_then(|c| c.get("tools")).is_none() {
            return Err("MCP server does not advertise tools".into());
        }
        connection.write(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await?;
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = HashSet::new();
        for _ in 0..MAX_PAGES {
            let params = cursor.as_ref().map_or(json!({}), |c| json!({"cursor":c}));
            let page = connection.request("tools/list", params).await?;
            let tools = page.get("tools").and_then(Value::as_array).ok_or("MCP tools/list has no tools array")?;
            if all.len() + tools.len() > MAX_TOOLS {
                return Err("MCP tool catalog exceeds 64 tools".into());
            }
            all.extend(tools.iter().cloned());
            cursor = page.get("nextCursor").and_then(Value::as_str).map(str::to_owned);
            if let Some(next) = &cursor {
                if next.len() > 1024 || !seen_cursors.insert(next.clone()) {
                    return Err("MCP tool catalog cursor is invalid or repeated".into());
                }
            } else {
                return Ok::<Vec<Value>, String>(all);
            }
        }
        Err::<Vec<Value>, String>("MCP tool catalog exceeds 16 pages".into())
    })
    .await
    .map_err(|_| "MCP initialization exceeded 15 seconds".to_owned())??;
    let connection = Arc::new(Mutex::new(connection));
    let mut names: HashSet<String> = reserved_names.iter().map(|s| (*s).to_owned()).collect();
    let mut found = HashSet::new();
    let mut tools: Vec<Arc<dyn AgentTool>> = Vec::new();
    for item in catalog {
        let original = item.get("name").and_then(Value::as_str).ok_or("MCP tool has no name")?;
        if !grants.contains(original) {
            continue;
        }
        if !found.insert(original.to_owned()) {
            return Err(format!("MCP server advertised duplicate tool {original:?}"));
        }
        let public = format!(
            "mcp__{}__{}",
            config.name,
            original.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>()
        );
        if public.len() > 64 || !names.insert(public.clone()) {
            return Err(format!("MCP public tool name collision or overlong name: {public}"));
        }
        let schema = item.get("inputSchema").cloned().ok_or("MCP tool is missing inputSchema")?;
        if !schema.is_object() || serde_json::to_vec(&schema).map_err(|e| e.to_string())?.len() > SCHEMA_MAX {
            return Err(format!("MCP tool {original:?} has invalid or oversized schema"));
        }
        let description = item.get("description").and_then(Value::as_str).unwrap_or("");
        if description.len() > 8192 {
            return Err(format!("MCP tool {original:?} description exceeds 8 KiB"));
        }
        tools.push(Arc::new(McpTool {
            definition: Tool { name: public, description: description.to_owned(), parameters: schema },
            server: config.name.clone(),
            original_name: original.to_owned(),
            connection: connection.clone(),
        }));
    }
    if found.len() != grants.len() {
        let missing = grants.difference(&found).cloned().collect::<Vec<_>>();
        return Err(format!("MCP granted tool(s) missing from server catalog: {}", missing.join(", ")));
    }
    Ok(tools)
}
