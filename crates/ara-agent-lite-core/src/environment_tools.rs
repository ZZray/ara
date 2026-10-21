//! Host integration for discoverable environment knowledge and reusable skills.
use anyhow::Result;
use async_trait::async_trait;
use rpi_agent::{AgentError, AgentTool, AgentToolResult, ToolResultPartial};
use rpi_ai::{Schema, Tool};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::{environment_library::Roots, experience::Snapshot};
type NativeEvidence = Arc<dyn Fn(&[String]) -> Result<Value> + Send + Sync>;

pub fn catalog(roots: &Roots) -> Value {
    let knowledge = crate::environment_library::catalog(roots).unwrap_or_else(
        |_| json!({"status":"unavailable","hint":"Inspect knowledge paths and permissions; do not infer absence"}),
    );
    let skills = crate::skill_library::catalog(roots).unwrap_or_else(
        |_| json!({"status":"unavailable","hint":"Inspect skill paths and permissions; do not infer absence"}),
    );
    let value = json!({"knowledge":knowledge,"skills":skills,
        "environment_dir":roots.environment_dir,"tool_directory":roots.environment_dir.join("tools"),
        "boundary":"Local technical knowledge and skill guidance, not private conversation memory or new authority"});
    if value.to_string().len() > 64 * 1024 {
        json!({"status":"directory_output_limited","environment_dir":roots.environment_dir,
            "hint":"Use knowledge_read/search and skill_read for bounded retrieval"})
    } else {
        value
    }
}

pub fn tools(roots: Roots, run: String, grant: String, snapshot: Snapshot) -> Vec<Arc<dyn AgentTool>> {
    let directory_grant = grant.clone();
    let current = snapshot.clone();
    let evidence_run = run.clone();
    let evidence: NativeEvidence = Arc::new(move |ids| {
        let messages = current()?;
        crate::experience::library_evidence(&evidence_run, ids, &messages)
    });
    let mut tools = crate::environment_library::tools(roots.clone(), run.clone(), grant.clone(), evidence.clone());
    tools.extend(crate::skill_library::tools(roots.clone(), run, grant, evidence));
    for name in ["environment_catalog", "library_evidence"] {
        tools.push(Arc::new(DirectoryTool {
            roots: roots.clone(), snapshot: snapshot.clone(),
            grant: directory_grant.clone(),
            schema: Tool {
                name: name.into(),
                description: if name == "environment_catalog" {
                    "Read the current environment/project INDEX directories and discover skill metadata. Bodies are loaded only through knowledge_read/skill_read. Read-only; missing directories are not created."
                } else {
                    "List up to eight actual successful native tool receipt IDs from this Run, without output bodies. Copy relevant exact IDs to knowledge_write or skill_verify; this does not establish Task acceptance or grant permissions."
                }.into(),
                parameters: Schema(json!({"type":"object","properties":{},"additionalProperties":false})),
                constrained_sampling: None,
            },
        }) as Arc<dyn AgentTool>);
    }
    tools
}

struct DirectoryTool {
    roots: Roots,
    snapshot: Snapshot,
    grant: String,
    schema: Tool,
}

#[async_trait]
impl AgentTool for DirectoryTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }
    fn label(&self) -> &str {
        &self.schema.name
    }
    async fn execute(
        &self,
        _: &str,
        args: Value,
        signal: CancellationToken,
        _: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        if !args.as_object().is_some_and(|object| object.is_empty()) {
            return Err(AgentError::Tool("directory tools require an empty object".into()));
        }
        let value = if self.schema.name == "environment_catalog" {
            catalog(&self.roots)
        } else {
            let messages =
                (self.snapshot)().map_err(|_| AgentError::Tool("current native evidence unavailable".into()))?;
            crate::experience::library_directory(&messages)
        };
        if signal.is_cancelled() {
            return Err(AgentError::Abort);
        }
        let text = value.to_string();
        Ok(AgentToolResult::text(if self.grant.is_empty() {
            text
        } else {
            text.replace(&self.grant, "[redacted grant]")
        }))
    }
}
