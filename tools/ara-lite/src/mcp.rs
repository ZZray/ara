//! MCP server over stdio (Go: `mcp.go`, built there on the official Go SDK).
//!
//! This is a small hand-written JSON-RPC 2.0 server (newline-delimited messages) instead
//! of a Rust MCP SDK. The tool surface is a fixed list of request/response calls, so the
//! protocol needs only `initialize`, `ping`, `tools/list`, `tools/call` and the
//! `notifications/cancelled` notification. Owning the loop keeps three things exact:
//! integers above 2^53 pass through untouched (results are forwarded as the service's own
//! JSON text), a cancelled long poll closes its HTTP connection at once, and the
//! dependency tree stays small.
//!
//! Every tool talks to the service through the profile's loopback HTTP API. The
//! credential is never part of any tool input or output.

use crate::client::{CancelHandle, call_wait, fetch_task, new_request_id, task_snapshot};
use crate::error::{Error, Result};
use crate::profile::read_connection;
use crate::snapshot::git_snapshot;
use crate::types::{Connection, Request};
use clap::Parser;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, ErrorKind, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Protocol revisions this server can speak, newest first. The tool surface is the same in all of them.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_IN_FLIGHT: usize = 32;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const SERVER_BUSY: i64 = -32000;

#[derive(Parser, Debug)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
struct McpArgs {
    #[arg(long, allow_hyphen_values = true)]
    profile: Option<String>,
}

/// `ara-lite mcp --profile FILE` (Go: `serveMCP`).
pub fn serve_mcp(args: &[String]) -> Result<()> {
    let usage = || Error::new("usage: ara-lite mcp --profile FILE");
    let parsed = McpArgs::try_parse_from(args).map_err(|_| usage())?;
    let Some(profile) = parsed.profile.filter(|p| !p.is_empty()) else { return Err(usage()) };
    let connection = read_connection(Path::new(&profile)).map_err(|e| e.context("read MCP profile"))?;
    run(connection, std::io::stdin().lock(), std::io::stdout())
}

// ---------------------------------------------------------------------------------------------
// Tool catalog
// ---------------------------------------------------------------------------------------------

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    /// Read-only tools never change service state.
    read_only: bool,
}

fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn int(description: &str) -> Value {
    json!({ "type": "integer", "description": description })
}

fn int_range(description: &str, min: i64, max: i64) -> Value {
    json!({ "type": "integer", "minimum": min, "maximum": max, "description": description })
}

fn object(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({ "type": "object", "properties": properties, "additionalProperties": false });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn request_id_property() -> Value {
    text(
        "Optional retry key (1..128 chars). Reuse it with identical arguments to retry safely after an unclear result.",
    )
}

fn catalog() -> Vec<Tool> {
    let task_ref = |extra: Value, required: &[&str]| {
        let mut properties = json!({
            "task_id": text("Task ID."),
            "version": int("Current task version from list_tasks or the last response."),
            "generation": int("Current task generation from list_tasks or the last response."),
            "request_id": request_id_property(),
        });
        if let (Some(base), Some(more)) = (properties.as_object_mut(), extra.as_object()) {
            base.extend(more.clone());
        }
        let mut all = vec!["task_id", "version", "generation"];
        all.extend_from_slice(required);
        object(properties, &all)
    };
    vec![
        Tool {
            name: "poll_messages",
            description: "Wait up to timeout_seconds for messages sent to this profile and return one page. Does not acknowledge: \
                          call ack_messages after handling. An empty page means nothing new.",
            schema: object(
                json!({
                    "after": int("Last handled seq; -1 (default) resumes from the server-side acknowledgement."),
                    "limit": int_range("Page size, 1..100 (default 100).", 1, 100),
                    "timeout_seconds": int_range("Longest wait in this call, 1..30 (default 5).", 1, 30),
                }),
                &[],
            ),
            read_only: true,
        },
        Tool {
            name: "ack_messages",
            description: "Acknowledge the message prefix you have really handled. expected_cursor is the ack_cursor of the last poll; \
                          ack_seq is the last handled seq.",
            schema: object(
                json!({
                    "expected_cursor": int("ack_cursor returned by poll_messages."),
                    "ack_seq": int("Seq of the last message handled."),
                }),
                &["expected_cursor", "ack_seq"],
            ),
            read_only: false,
        },
        Tool {
            name: "send_message",
            description: "Send a chat message. A worker writes to the master. The master sets task_id (goes to that task's worker) \
                          or worker_id, or neither to broadcast.",
            schema: object(
                json!({
                    "body": text("Message text."),
                    "task_id": text("Related task ID."),
                    "worker_id": text("Target worker ID (master only)."),
                    "request_id": request_id_property(),
                }),
                &["body"],
            ),
            read_only: false,
        },
        Tool {
            name: "list_tasks",
            description: "List tasks visible to this profile with state, version and generation. Task state is authoritative even \
                          after chat messages expire.",
            schema: object(json!({ "task_id": text("Only this task.") }), &[]),
            read_only: true,
        },
        Tool {
            name: "list_clients",
            description: "List the master and workers with their IDs and last-seen times.",
            schema: object(json!({}), &[]),
            read_only: true,
        },
        Tool {
            name: "create_task",
            description: "Master only. Create a queued task. A write task owns its workspace (and overlapping ones) until it passes.",
            schema: object(
                json!({
                    "title": text("Short title."),
                    "workspace": text("Existing directory the task works in."),
                    "mode": json!({ "type": "string", "enum": ["write", "read"], "description": "write (default) or read." }),
                    "body": text("Instructions and acceptance requirements."),
                    "worker_id": text("Assign to this worker; omit to let any worker claim."),
                    "request_id": request_id_property(),
                }),
                &["title", "workspace", "body"],
            ),
            read_only: false,
        },
        Tool {
            name: "claim_task",
            description: "Worker only. Claim a queued task and start its 2-minute lease. Progress starts at 0%. version and                           generation may be omitted; when given they must be current.",
            schema: object(
                json!({
                    "task_id": text("Task ID."),
                    "version": int("Optional. Current task version from list_tasks."),
                    "generation": int("Optional. Current task generation from list_tasks."),
                    "request_id": request_id_property(),
                }),
                &["task_id"],
            ),
            read_only: false,
        },
        Tool {
            name: "update_progress",
            description: "Worker only. Report real progress of your running task. Without subtask_id it sets the task's overall \
                          progress; with one it sets only that subtask. Returns the task with its new version.",
            schema: task_ref(
                json!({
                    "percent": int_range("Progress, 0..100.", 0, 100),
                    "description": text("What is done (up to 200 chars)."),
                    "subtask_id": text("Optional stable subtask ID."),
                    "subtask_title": text("Title, required when creating a subtask."),
                }),
                &["percent", "description"],
            ),
            read_only: false,
        },
        Tool {
            name: "heartbeat",
            description: "Worker only. Renew the 2-minute lease of your task, or restore an unconfirmed task you still own. \
                          Without task_id it only marks the client as seen. Progress renews the lease too; polling does not.",
            schema: object(
                json!({
                    "task_id": text("Task to renew."),
                    "version": int("Current task version."),
                    "generation": int("Current task generation."),
                    "request_id": request_id_property(),
                }),
                &[],
            ),
            read_only: false,
        },
        Tool {
            name: "submit_task",
            description: "Worker only. Hand the task in for review; the tool records the workspace's Git snapshot itself, so stop \
                          writing first. Then leave the workspace unchanged until the master reviews.",
            schema: task_ref(json!({ "body": text("What changed and how it was verified.") }), &["body"]),
            read_only: false,
        },
        Tool {
            name: "snapshot_task",
            description: "Capture the current Git snapshot of a task's workspace. Master: call before inspecting submitted work and \
                          pass the result to review_task.",
            schema: object(json!({ "task_id": text("Task ID.") }), &["task_id"]),
            read_only: true,
        },
        Tool {
            name: "review_task",
            description: "Master only. Review a submitted task: pass, changes_requested (worker continues) or blocked (frozen). \
                          Fails if the workspace no longer matches the submitted snapshot.",
            schema: task_ref(
                json!({
                    "decision": json!({ "type": "string", "enum": ["pass", "changes_requested", "blocked"], "description": "Review decision." }),
                    "body": text("Concrete reasons for the decision."),
                    "snapshot": text("Snapshot from snapshot_task taken before inspecting; omit to use the live one."),
                }),
                &["decision", "body"],
            ),
            read_only: false,
        },
        Tool {
            name: "confirm_stop",
            description: "Master only. Release a task's worker and requeue the task, but only with real evidence that the old \
                          execution stopped writing.",
            schema: task_ref(json!({ "evidence": text("Proof the old execution stopped.") }), &["evidence"]),
            read_only: false,
        },
        Tool {
            name: "clear_tasks",
            description: "Master only. Retire all open tasks of one exact workspace. Refused while any is running or unconfirmed. \
                          expected_count must equal the current number of uncleared tasks there.",
            schema: object(
                json!({
                    "workspace": text("Exact workspace directory."),
                    "expected_count": int("Current number of uncleared tasks in that workspace, from list_tasks."),
                    "reason": text("Why the queue is reset."),
                    "request_id": request_id_property(),
                }),
                &["workspace", "expected_count", "reason"],
            ),
            read_only: false,
        },
    ]
}

fn tools_list_json() -> Value {
    let tools: Vec<Value> = catalog()
        .into_iter()
        .map(|t| {
            let annotations =
                if t.read_only { json!({ "readOnlyHint": true }) } else { json!({ "idempotentHint": true }) };
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.schema,
                "annotations": annotations,
            })
        })
        .collect();
    json!({ "tools": tools })
}

// ---------------------------------------------------------------------------------------------
// Tool inputs
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PollIn {
    after: Option<i64>,
    limit: Option<i64>,
    timeout_seconds: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AckIn {
    expected_cursor: i64,
    ack_seq: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SendIn {
    body: String,
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    worker_id: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListTasksIn {
    #[serde(default)]
    task_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoInput {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIn {
    title: String,
    workspace: String,
    #[serde(default)]
    mode: String,
    body: String,
    #[serde(default)]
    worker_id: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimIn {
    task_id: String,
    version: Option<i64>,
    generation: Option<i64>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgressIn {
    task_id: String,
    version: i64,
    generation: i64,
    percent: i64,
    description: String,
    #[serde(default)]
    subtask_id: String,
    #[serde(default)]
    subtask_title: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartbeatIn {
    #[serde(default)]
    task_id: String,
    #[serde(default)]
    version: i64,
    #[serde(default)]
    generation: i64,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SubmitIn {
    task_id: String,
    version: i64,
    generation: i64,
    body: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotIn {
    task_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewIn {
    task_id: String,
    version: i64,
    generation: i64,
    decision: String,
    body: String,
    snapshot: Option<String>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmIn {
    task_id: String,
    version: i64,
    generation: i64,
    evidence: String,
    request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearIn {
    workspace: String,
    expected_count: i64,
    reason: String,
    request_id: Option<String>,
}

fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T> {
    let arguments = if arguments.is_null() { json!({}) } else { arguments };
    serde_json::from_value(arguments).map_err(|e| Error::new(format!("invalid arguments: {e}")))
}

// ---------------------------------------------------------------------------------------------
// Tool execution
// ---------------------------------------------------------------------------------------------

/// The service's JSON text on a single line (stdio framing forbids raw newlines).
fn one_line(text: &str) -> String {
    if !text.contains(['\n', '\r']) {
        return text.to_string();
    }
    serde_json::from_str::<Value>(text).map(|v| v.to_string()).unwrap_or_else(|_| text.replace(['\n', '\r'], " "))
}

/// One tool call: the profile, and the handle that aborts its HTTP request on cancellation.
struct Ctx<'a> {
    c: &'a Connection,
    cancel: &'a CancelHandle,
}

impl Ctx<'_> {
    fn request(&self, op: &str) -> Request {
        Request { op: op.to_string(), client_id: self.c.id.clone(), ..Request::default() }
    }

    fn read(&self, r: Request, wait: Duration) -> Result<String> {
        Ok(one_line(call_wait(self.c, &r, wait, Some(self.cancel))?.get()))
    }

    /// A state-changing call with a retry key. When the connection fails mid-call the error
    /// names the key, because the change may or may not have been applied.
    fn mutate(&self, mut r: Request, request_id: Option<String>) -> Result<String> {
        let id = match request_id {
            Some(id) if !id.is_empty() => id,
            _ => new_request_id()?,
        };
        r.request_id = id.clone();
        self.read(r, Duration::ZERO).map_err(|e| {
            if e.message().starts_with("service unavailable") {
                Error::new(format!(
                    "{e} (request_id {id}: the change may have been applied; retry with this request_id and identical arguments)"
                ))
            } else {
                e
            }
        })
    }
}

fn call_tool(c: &Connection, cancel: &CancelHandle, name: &str, arguments: Value) -> Result<String> {
    let ctx = Ctx { c, cancel };
    match name {
        "poll_messages" => {
            let a: PollIn = decode(arguments)?;
            let after = a.after.unwrap_or(-1);
            if after < -1 {
                return Err(Error::new("after must be -1 or a nonnegative sequence"));
            }
            let limit = a.limit.unwrap_or(100);
            if !(1..=100).contains(&limit) {
                return Err(Error::new("limit must be between 1 and 100"));
            }
            let seconds = a.timeout_seconds.unwrap_or(5);
            if !(1..=30).contains(&seconds) {
                return Err(Error::new("timeout_seconds must be between 1 and 30"));
            }
            let r = Request { after, limit, ..ctx.request("poll") };
            ctx.read(r, Duration::from_secs(seconds as u64))
        }
        "ack_messages" => {
            let a: AckIn = decode(arguments)?;
            if a.expected_cursor < 0 || a.ack_seq < a.expected_cursor {
                return Err(Error::new("ack_seq must not be less than a nonnegative expected_cursor"));
            }
            let r = Request { expected_cursor: a.expected_cursor, ack_seq: a.ack_seq, ..ctx.request("ack") };
            ctx.read(r, Duration::ZERO)
        }
        "send_message" => {
            let a: SendIn = decode(arguments)?;
            if a.body.is_empty() {
                return Err(Error::new("body is required"));
            }
            let r = Request { task_id: a.task_id, worker_id: a.worker_id, body: a.body, ..ctx.request("send") };
            ctx.mutate(r, a.request_id)
        }
        "list_tasks" => {
            let a: ListTasksIn = decode(arguments)?;
            let tasks = ctx.read(Request { task_id: a.task_id, ..ctx.request("tasks") }, Duration::ZERO)?;
            Ok(format!("{{\"tasks\":{tasks}}}"))
        }
        "list_clients" => {
            let NoInput {} = decode(arguments)?;
            let clients = ctx.read(ctx.request("clients"), Duration::ZERO)?;
            Ok(format!("{{\"clients\":{clients}}}"))
        }
        "create_task" => {
            let a: CreateIn = decode(arguments)?;
            let mode = if a.mode.is_empty() { "write".to_string() } else { a.mode };
            let r = Request {
                title: a.title,
                workspace: a.workspace,
                mode,
                body: a.body,
                worker_id: a.worker_id,
                ..ctx.request("create")
            };
            ctx.mutate(r, a.request_id)
        }
        "claim_task" => {
            let a: ClaimIn = decode(arguments)?;
            let r =
                Request { task_id: a.task_id, version: a.version, generation: a.generation, ..ctx.request("claim") };
            ctx.mutate(r, a.request_id)
        }
        "update_progress" => {
            let a: ProgressIn = decode(arguments)?;
            if a.task_id.is_empty() || !(0..=100).contains(&a.percent) {
                return Err(Error::new("task_id is required and percent must be 0..100"));
            }
            let r = Request {
                task_id: a.task_id,
                version: Some(a.version),
                generation: Some(a.generation),
                percent: a.percent,
                description: a.description,
                subtask_id: a.subtask_id,
                subtask_title: a.subtask_title,
                ..ctx.request("progress")
            };
            ctx.mutate(r, a.request_id)
        }
        "heartbeat" => {
            let a: HeartbeatIn = decode(arguments)?;
            let r = Request {
                task_id: a.task_id,
                version: Some(a.version),
                generation: Some(a.generation),
                ..ctx.request("heartbeat")
            };
            ctx.mutate(r, a.request_id)
        }
        "submit_task" => {
            let a: SubmitIn = decode(arguments)?;
            let snapshot = task_snapshot(c, &a.task_id, None)?;
            let r = Request {
                task_id: a.task_id,
                version: Some(a.version),
                generation: Some(a.generation),
                body: a.body,
                snapshot,
                ..ctx.request("submit")
            };
            ctx.mutate(r, a.request_id)
        }
        "snapshot_task" => {
            let a: SnapshotIn = decode(arguments)?;
            let task = fetch_task(c, &a.task_id)?;
            let snapshot = git_snapshot(Path::new(&task.workspace))?;
            Ok(json!({ "task_id": task.id, "workspace": task.workspace, "snapshot": snapshot }).to_string())
        }
        "review_task" => {
            let a: ReviewIn = decode(arguments)?;
            let given = a.snapshot.as_deref().filter(|s| !s.is_empty());
            let snapshot = task_snapshot(c, &a.task_id, given)?;
            let r = Request {
                task_id: a.task_id,
                version: Some(a.version),
                generation: Some(a.generation),
                decision: a.decision,
                body: a.body,
                snapshot,
                ..ctx.request("review")
            };
            ctx.mutate(r, a.request_id)
        }
        "confirm_stop" => {
            let a: ConfirmIn = decode(arguments)?;
            let r = Request {
                task_id: a.task_id,
                version: Some(a.version),
                generation: Some(a.generation),
                evidence: a.evidence,
                ..ctx.request("confirm-stop")
            };
            ctx.mutate(r, a.request_id)
        }
        "clear_tasks" => {
            let a: ClearIn = decode(arguments)?;
            let r = Request {
                workspace: a.workspace,
                expected_count: a.expected_count,
                body: a.reason,
                ..ctx.request("clear-tasks")
            };
            ctx.mutate(r, a.request_id)
        }
        other => Err(Error::new(format!("unknown tool {other:?}"))),
    }
}

/// The `result` object of `tools/call` (`structuredContent` is the service's JSON as sent).
fn tool_result(outcome: Result<String>) -> String {
    match outcome {
        Ok(structured) => {
            let content = Value::String(structured.clone());
            format!(
                "{{\"content\":[{{\"type\":\"text\",\"text\":{content}}}],\"structuredContent\":{structured},\"isError\":false}}"
            )
        }
        Err(e) => {
            let content = Value::String(e.to_string());
            format!("{{\"content\":[{{\"type\":\"text\",\"text\":{content}}}],\"isError\":true}}")
        }
    }
}

// ---------------------------------------------------------------------------------------------
// JSON-RPC loop
// ---------------------------------------------------------------------------------------------

struct Mcp {
    c: Connection,
    out: Mutex<Box<dyn Write + Send>>,
    /// In-flight `tools/call` requests by their JSON-encoded request ID.
    in_flight: Mutex<HashMap<String, Arc<CancelHandle>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Mcp {
    fn write_line(&self, line: &str) {
        let mut out = lock(&self.out);
        // A closed stdout means the client is gone; there is nobody left to tell.
        let _ = out.write_all(line.as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush());
    }

    fn reply(&self, id: &Value, result: &str) {
        self.write_line(&format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"));
    }

    fn reply_error(&self, id: &Value, code: i64, message: &str) {
        let error = json!({ "code": code, "message": message });
        self.write_line(&format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"error\":{error}}}"));
    }

    fn handle_line(self: &Arc<Self>, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let message: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return self.reply_error(&Value::Null, PARSE_ERROR, &format!("parse error: {e}")),
        };
        let Value::Object(mut fields) = message else {
            return self.reply_error(&Value::Null, INVALID_REQUEST, "expected one JSON-RPC object per line");
        };
        let id = fields.remove("id");
        let method = fields.remove("method");
        let params = fields.remove("params").unwrap_or(Value::Null);
        match (method, id) {
            (Some(Value::String(method)), None) => self.notification(&method, &params),
            (Some(Value::String(method)), Some(id)) => {
                if id.is_string() || id.is_number() {
                    self.request(id, &method, params);
                } else {
                    self.reply_error(&Value::Null, INVALID_REQUEST, "request id must be a string or a number");
                }
            }
            // A response from the client; this server never sends requests.
            (None, Some(_)) => {}
            _ => self.reply_error(&Value::Null, INVALID_REQUEST, "invalid request"),
        }
    }

    fn notification(&self, method: &str, params: &Value) {
        if method == "notifications/cancelled"
            && let Some(request_id) = params.get("requestId")
        {
            let handle = lock(&self.in_flight).get(&request_id.to_string()).cloned();
            if let Some(handle) = handle {
                handle.cancel();
            }
        }
        // notifications/initialized and any other notification need no action.
    }

    fn request(self: &Arc<Self>, id: Value, method: &str, params: Value) {
        match method {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
                let version = PROTOCOL_VERSIONS.iter().find(|v| **v == asked).unwrap_or(&PROTOCOL_VERSIONS[0]);
                let result = json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "ara-lite", "version": env!("CARGO_PKG_VERSION") },
                });
                self.reply(&id, &result.to_string());
            }
            "ping" => self.reply(&id, "{}"),
            "tools/list" => self.reply(&id, &tools_list_json().to_string()),
            "tools/call" => self.start_call(id, params),
            other => self.reply_error(&id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
        }
    }

    /// Run a tool on its own thread so a long poll never blocks other requests or cancellation.
    fn start_call(self: &Arc<Self>, id: Value, params: Value) {
        let Some(name) = params.get("name").and_then(Value::as_str).map(str::to_string) else {
            return self.reply_error(&id, INVALID_PARAMS, "tools/call needs a tool name");
        };
        if !catalog().iter().any(|t| t.name == name) {
            return self.reply_error(&id, INVALID_PARAMS, &format!("unknown tool: {name}"));
        }
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
        let key = id.to_string();
        let cancel = CancelHandle::new();
        {
            let mut in_flight = lock(&self.in_flight);
            if in_flight.len() >= MAX_IN_FLIGHT {
                drop(in_flight);
                return self.reply_error(&id, SERVER_BUSY, "too many tool calls in flight");
            }
            in_flight.insert(key.clone(), cancel.clone());
        }
        let mcp = self.clone();
        let spawned = thread::Builder::new().name("ara-lite-mcp-call".into()).spawn(move || {
            let outcome = catch_unwind(AssertUnwindSafe(|| call_tool(&mcp.c, &cancel, &name, arguments)))
                .unwrap_or_else(|_| Err(Error::new("internal error while running the tool")));
            lock(&mcp.in_flight).remove(&key);
            // A cancelled request gets no response (MCP cancellation).
            if !cancel.is_cancelled() {
                mcp.reply(&id, &tool_result(outcome));
            }
        });
        if let Err(e) = spawned {
            lock(&self.in_flight).clear();
            self.reply_error(&Value::Null, SERVER_BUSY, &format!("cannot start tool thread: {e}"));
        }
    }

    fn cancel_all(&self) {
        let handles: Vec<Arc<CancelHandle>> = lock(&self.in_flight).values().cloned().collect();
        for handle in handles {
            handle.cancel();
        }
    }
}

/// Serve MCP over the given streams until the input ends. Ending the input cancels every
/// in-flight call, which closes its HTTP request.
pub fn run(c: Connection, input: impl BufRead, output: impl Write + Send + 'static) -> Result<()> {
    let mcp = Arc::new(Mcp { c, out: Mutex::new(Box::new(output)), in_flight: Mutex::new(HashMap::new()) });
    for line in input.lines() {
        match line {
            Ok(line) => mcp.handle_line(&line),
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                mcp.reply_error(&Value::Null, PARSE_ERROR, "request is not valid UTF-8");
            }
            Err(_) => break,
        }
    }
    mcp.cancel_all();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_catalog_tool_has_an_object_schema_and_a_short_description() {
        let tools = catalog();
        assert_eq!(tools.len(), 14);
        for t in &tools {
            assert_eq!(t.schema["type"], "object", "{}", t.name);
            assert_eq!(t.schema["additionalProperties"], false, "{}", t.name);
            assert!(t.description.len() < 300, "{} description is too long", t.name);
            assert!(t.description.ends_with('.'), "{}", t.name);
        }
        let mut names: Vec<_> = tools.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len());
    }

    #[test]
    fn required_properties_exist_in_every_schema() {
        for t in catalog() {
            let properties = t.schema["properties"].as_object().unwrap();
            for name in t.schema["required"].as_array().into_iter().flatten() {
                assert!(properties.contains_key(name.as_str().unwrap()), "{}: {name}", t.name);
            }
        }
    }

    #[test]
    fn tool_result_carries_structured_content_verbatim() {
        let ok = tool_result(Ok("{\"seq\":9007199254740993}".to_string()));
        assert!(ok.contains("\"structuredContent\":{\"seq\":9007199254740993}"), "{ok}");
        assert!(ok.contains("\"isError\":false"));
        let value: Value = serde_json::from_str(&ok).unwrap();
        assert_eq!(value["content"][0]["text"], "{\"seq\":9007199254740993}");
        let failed: Value = serde_json::from_str(&tool_result(Err(Error::new("boom \"quoted\"")))).unwrap();
        assert_eq!(failed["isError"], true);
        assert_eq!(failed["content"][0]["text"], "boom \"quoted\"");
    }

    #[test]
    fn one_line_removes_pretty_printing() {
        assert_eq!(one_line("{\n  \"a\": 1\n}"), "{\"a\":1}");
        assert_eq!(one_line("{\"a\":1}"), "{\"a\":1}");
    }

    #[test]
    fn argument_validation_reports_the_field() {
        let e = decode::<AckIn>(json!({ "expected_cursor": 1 })).err().unwrap();
        assert!(e.message().contains("ack_seq"), "{e}");
        let e = decode::<AckIn>(json!({ "expected_cursor": 1, "ack_seq": 2, "extra": true })).err().unwrap();
        assert!(e.message().contains("extra"), "{e}");
        assert!(decode::<NoInput>(Value::Null).is_ok());
    }
}
