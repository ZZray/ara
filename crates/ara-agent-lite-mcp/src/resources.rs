//! `list_mcp_resources` and `read_mcp_resource` (requirement 42.7).
//! Resource reads are treated as read-only: interruptions are plain errors.
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

use ara_agent_lite_core::{
    rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial},
    rpi_ai::Tool,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::{
    client::{Client, MAX_PAGES},
    tool::{convert_result, schema},
};

/// Resources listed per call.
pub const MAX_RESOURCES: usize = 1000;
pub const LIST_TOOL: &str = "list_mcp_resources";
pub const READ_TOOL: &str = "read_mcp_resource";

pub(crate) struct Resources {
    servers: BTreeMap<String, Arc<Client>>,
    call_timeout: Duration,
    results_dir: Option<PathBuf>,
}

pub(crate) struct ListResources {
    schema: Tool,
    shared: Arc<Resources>,
}

pub(crate) struct ReadResource {
    schema: Tool,
    shared: Arc<Resources>,
}

/// The two tools, or none when no connected server declares `resources`.
pub(crate) fn tools(
    servers: BTreeMap<String, Arc<Client>>,
    call_timeout: Duration,
    results_dir: Option<PathBuf>,
) -> Vec<Arc<dyn AgentTool>> {
    if servers.is_empty() {
        return Vec::new();
    }
    let names = servers.keys().cloned().collect::<Vec<_>>().join(", ");
    let shared = Arc::new(Resources { servers, call_timeout, results_dir });
    let list = ListResources {
        schema: schema(
            LIST_TOOL.into(),
            format!(
                "List resources offered by connected MCP servers ({names}). Returns uri, name, mimeType and server."
            ),
            json!({"type":"object","properties":{"server":{"type":"string","description":"Only list this server's resources."}},"additionalProperties":false}),
        ),
        shared: shared.clone(),
    };
    let read = ReadResource {
        schema: schema(
            READ_TOOL.into(),
            format!("Read one resource from a connected MCP server ({names}) by server name and uri."),
            json!({"type":"object","properties":{"server":{"type":"string"},"uri":{"type":"string"}},"required":["server","uri"],"additionalProperties":false}),
        ),
        shared,
    };
    vec![Arc::new(list), Arc::new(read)]
}

impl Resources {
    fn client(&self, server: &str) -> Result<&Arc<Client>, AgentError> {
        self.servers
            .get(server)
            .ok_or_else(|| AgentError::Tool(format!("no connected MCP server named {server} offers resources")))
    }

    async fn list(&self, client: &Client, cancel: &CancellationToken, out: &mut Vec<Value>) -> Result<(), AgentError> {
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let params = cursor.take().map_or(json!({}), |cursor| json!({"cursor":cursor}));
            let page = client.request("resources/list", params, self.call_timeout, cancel).await.map_err(|error| {
                AgentError::Tool(format!("resources/list on server {} failed: {error}", client.server))
            })?;
            for item in page.get("resources").and_then(Value::as_array).into_iter().flatten() {
                if out.len() == MAX_RESOURCES {
                    out.push(json!({"server": client.server, "note": format!("listing stopped at {MAX_RESOURCES} resources")}));
                    return Ok(());
                }
                out.push(json!({
                    "server": client.server,
                    "uri": item.get("uri"),
                    "name": item.get("name"),
                    "mimeType": item.get("mimeType"),
                }));
            }
            match page.get("nextCursor").and_then(Value::as_str) {
                Some(next) => cursor = Some(next.to_string()),
                None => return Ok(()),
            }
        }
        out.push(json!({"server": client.server, "note": format!("listing stopped at {MAX_PAGES} pages")}));
        Ok(())
    }
}

#[async_trait]
impl AgentTool for ListResources {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        LIST_TOOL
    }

    async fn execute(
        &self,
        call_id: &str,
        params: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let mut out = Vec::new();
        match params.get("server").and_then(Value::as_str) {
            Some(server) => self.shared.list(self.shared.client(server)?, &signal, &mut out).await?,
            None => {
                for client in self.shared.servers.values() {
                    if let Err(AgentError::Tool(error)) = self.shared.list(client, &signal, &mut out).await {
                        out.push(json!({"server": client.server, "error": error}));
                    }
                    if signal.is_cancelled() {
                        return Err(AgentError::Abort);
                    }
                }
            }
        }
        let text = json!({"content":[{"type":"text","text": Value::Array(out).to_string()}]});
        convert_result("", LIST_TOOL, call_id, &text, self.shared.results_dir.as_deref()).map_err(AgentError::Tool)
    }
}

#[async_trait]
impl AgentTool for ReadResource {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        READ_TOOL
    }

    async fn execute(
        &self,
        call_id: &str,
        params: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let (Some(server), Some(uri)) =
            (params.get("server").and_then(Value::as_str), params.get("uri").and_then(Value::as_str))
        else {
            return Err(AgentError::Tool("read_mcp_resource requires server and uri".into()));
        };
        let client = self.shared.client(server)?;
        let result = match client
            .request("resources/read", json!({"uri": uri}), self.shared.call_timeout, &signal)
            .await
        {
            Ok(result) => result,
            Err(error) => return Err(AgentError::Tool(format!("resources/read on server {server} failed: {error}"))),
        };
        // Resource contents use the tools/call conversion: text stays text,
        // image blobs become images, everything else becomes JSON text.
        let content: Vec<Value> = result
            .get("contents")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or_default();
                match (item.get("text").and_then(Value::as_str), item.get("blob").and_then(Value::as_str)) {
                    (Some(text), _) => json!({"type":"text","text":text}),
                    (None, Some(blob)) if mime.starts_with("image/") => {
                        json!({"type":"image","data":blob,"mimeType":mime})
                    }
                    _ => item.clone(),
                }
            })
            .collect();
        let mut converted = convert_result(
            server,
            READ_TOOL,
            call_id,
            &json!({"content": content}),
            self.shared.results_dir.as_deref(),
        )
        .map_err(AgentError::Tool)?;
        converted.details["uri"] = json!(uri);
        Ok(converted)
    }
}
