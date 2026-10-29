//! Read-only Session probe for the process-interruption test (requirement 14.1).
//!
//! `session_probe <session_path> <cwd>` prints one JSON object:
//! `pending_run` (the journal's durable pending Run ID or null),
//! `interrupted_run` (Core `agent::interrupted_run`: Run ID, open tool calls,
//! raw_count, or null), `raw_count` (journal message count) and
//! `tool_results` (`toolCallId`, `toolName`, `isError`, `details.source` of
//! every ToolResult in the active journal range). It never writes the Session.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ara_agent_lite_harness::core::{agent, session_journal};
use serde_json::{json, Value};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let usage = "usage: session_probe <session_path> <cwd>";
    let session = PathBuf::from(args.next().context(usage)?);
    let cwd = PathBuf::from(args.next().context(usage)?);
    // Harness stores canonical paths; Core compares the Session cwd exactly.
    let session = session.canonicalize().context("session path unavailable")?;
    let cwd = cwd.canonicalize().context("cwd unavailable")?;
    let interrupted = agent::interrupted_run(&session, &cwd)?;
    let journal = session_journal::Journal::for_session(&session);
    let head = journal.open()?;
    let messages =
        journal.read(head.active_base, head.raw_count, session_journal::CHECKPOINT_BYTES).context("journal read")?;
    let tool_results: Vec<Value> = messages
        .iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|message| message.get("toolCallId").is_some())
        .map(|message| {
            json!({"toolCallId":message["toolCallId"],"toolName":message["toolName"],
                "isError":message["isError"],"source":message["details"]["source"]})
        })
        .collect();
    let report = json!({
        "pending_run": head.metadata["pending_run"],
        "interrupted_run": interrupted,
        "raw_count": head.raw_count,
        "tool_results": tool_results,
    });
    println!("{}", serde_json::to_string(&report)?);
    Ok(())
}
