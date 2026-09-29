//! Command line front end (Go: `run` and `clientCommand` in `main.go`).
//!
//! Flag names match the Go tool. Flags are `--name value` style (clap); Go's
//! single-dash spelling and `--flag=false` booleans are not supported.

use crate::client::{call, call_wait, new_request_id, task_snapshot};
use crate::error::{Error, Result};
use crate::help::{HELP_TEXT, prompt_help};
use crate::profile::{read_connection, save_private};
use crate::snapshot::git_snapshot;
use crate::types::{Client, Connection, Request};
use crate::util::{duration_from_nanos, indent_json, parse_go_duration};
use crate::{err, serve};
use clap::Parser;
use serde_json::value::RawValue;
use std::path::Path;
use std::time::Duration;

/// Longest server-side wait for one poll.
const MAX_POLL_WAIT: Duration = Duration::from_secs(30);
/// Body files larger than this are refused (matches the server body limit).
const MAX_BODY_FILE: usize = 512 << 10;

const CLIENT_OPS: [&str; 16] = [
    "join",
    "status",
    "clients",
    "tasks",
    "poll",
    "ack",
    "history",
    "send",
    "create",
    "claim",
    "heartbeat",
    "progress",
    "submit",
    "review",
    "confirm-stop",
    "clear-tasks",
];

/// Operations that change server state; the request ID is echoed on stderr for these.
const MUTATING_OPS: [&str; 10] =
    ["join", "create", "claim", "heartbeat", "progress", "submit", "review", "confirm-stop", "clear-tasks", "send"];

/// One flag set shared by every client command, like the single Go `FlagSet`.
#[derive(Parser, Debug)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
struct ClientArgs {
    /// Client profile path.
    #[arg(long, allow_hyphen_values = true)]
    profile: Option<String>,
    /// Role join file (join only).
    #[arg(long = "join-file", allow_hyphen_values = true)]
    join_file: Option<String>,
    /// Machine JSON output.
    #[arg(long)]
    json: bool,
    /// UTF-8 result or instruction file; replaces --body.
    #[arg(long = "body-file", allow_hyphen_values = true)]
    body_file: Option<String>,
    /// Poll timeout, 0..30s.
    #[arg(long, default_value = "25s", allow_hyphen_values = true)]
    timeout: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    role: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    name: String,
    #[arg(long = "task-id", default_value = "", allow_hyphen_values = true)]
    task_id: String,
    #[arg(long = "worker-id", default_value = "", allow_hyphen_values = true)]
    worker_id: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    title: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    workspace: String,
    #[arg(long, default_value = "write", allow_hyphen_values = true)]
    mode: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    body: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    description: String,
    #[arg(long = "subtask-id", default_value = "", allow_hyphen_values = true)]
    subtask_id: String,
    #[arg(long = "subtask-title", default_value = "", allow_hyphen_values = true)]
    subtask_title: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    decision: String,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    evidence: String,
    /// Reason for clearing tasks; sent in the request body field. Wins over --body when both are given.
    #[arg(long, allow_hyphen_values = true)]
    reason: Option<String>,
    #[arg(long, default_value = "", allow_hyphen_values = true)]
    snapshot: String,
    #[arg(long = "request-id", default_value = "", allow_hyphen_values = true)]
    request_id: String,
    /// Omitted means absent: claim then skips fencing, every other task operation fails.
    #[arg(long, allow_hyphen_values = true)]
    version: Option<i64>,
    #[arg(long, allow_hyphen_values = true)]
    generation: Option<i64>,
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    cursor: i64,
    #[arg(long, default_value_t = -1, allow_hyphen_values = true)]
    after: i64,
    #[arg(long, default_value_t = 100, allow_hyphen_values = true)]
    limit: i64,
    #[arg(long, default_value_t = 0, allow_hyphen_values = true)]
    percent: i64,
    #[arg(long = "expected-count", default_value_t = 0, allow_hyphen_values = true)]
    expected_count: i64,
    #[arg(long = "expected-cursor", default_value_t = 0, allow_hyphen_values = true)]
    expected_cursor: i64,
    #[arg(long = "ack-seq", default_value_t = 0, allow_hyphen_values = true)]
    ack_seq: i64,
}

/// Turn a clap error into a one-line message (the full usage text is noise for scripts and AIs).
fn parse_error(e: clap::Error) -> Error {
    let rendered = e.render().to_string();
    let first = rendered.lines().next().unwrap_or_default();
    Error::new(format!("{}; use --help", first.trim_start_matches("error: ")))
}

/// Entry point for `argv[1..]` (Go: `run`).
pub fn run(args: &[String]) -> Result<()> {
    let Some(first) = args.first() else { return serve::serve(&[]) };
    let rest = &args[1..];
    match first.as_str() {
        "serve" => serve::serve(rest),
        "mcp" => crate::mcp::serve_mcp(rest),
        "help" | "--help" | "-h" => {
            print!("{HELP_TEXT}");
            print!("\n{}", prompt_help());
            Ok(())
        }
        "snapshot" => {
            if rest.len() != 1 {
                return Err(Error::new("usage: ara-lite snapshot WORKSPACE"));
            }
            println!("{}", git_snapshot(Path::new(&rest[0]))?);
            Ok(())
        }
        op => client_command(op, rest),
    }
}

/// Build the request for `op` from parsed flags (no I/O; the body file is applied by the caller).
fn build_request(op: &str, a: &ClientArgs) -> Request {
    Request {
        op: op.to_string(),
        request_id: a.request_id.clone(),
        client_id: String::new(),
        role: a.role.clone(),
        name: a.name.clone(),
        task_id: a.task_id.clone(),
        worker_id: a.worker_id.clone(),
        title: a.title.clone(),
        workspace: a.workspace.clone(),
        mode: a.mode.clone(),
        body: a.reason.clone().unwrap_or_else(|| a.body.clone()),
        snapshot: a.snapshot.clone(),
        decision: a.decision.clone(),
        evidence: a.evidence.clone(),
        version: a.version,
        generation: a.generation,
        cursor: a.cursor,
        after: a.after,
        limit: a.limit,
        ack_seq: a.ack_seq,
        expected_cursor: a.expected_cursor,
        expected_count: a.expected_count,
        percent: a.percent,
        description: a.description.clone(),
        subtask_id: a.subtask_id.clone(),
        subtask_title: a.subtask_title.clone(),
    }
}

fn poll_timeout(text: &str) -> Result<Duration> {
    let nanos = parse_go_duration(text).map_err(|e| err!("invalid value {text:?} for --timeout: {e}"))?;
    match duration_from_nanos(nanos) {
        Some(wait) if wait <= MAX_POLL_WAIT => Ok(wait),
        _ => Err(Error::new("--timeout must be between 0 and 30s")),
    }
}

/// Print a raw `result`. `--json` prints it exactly as received, otherwise it is indented.
fn print_result(raw: &RawValue, machine: bool) {
    if machine {
        println!("{}", raw.get());
    } else {
        println!("{}", indent_json(raw.get()));
    }
}

/// Run one client command against the service named in the profile (Go: `clientCommand`).
pub fn client_command(op: &str, args: &[String]) -> Result<()> {
    if !CLIENT_OPS.contains(&op) {
        return Err(err!("unknown command {op:?}; use --help"));
    }
    let a = ClientArgs::try_parse_from(args).map_err(parse_error)?;
    let mut r = build_request(op, &a);
    let Some(profile) = a.profile.as_deref().filter(|p| !p.is_empty()) else {
        return Err(Error::new("--profile FILE is required; see the server's join instructions"));
    };
    if let Some(path) = a.body_file.as_deref().filter(|p| !p.is_empty()) {
        let bytes = std::fs::read(path).map_err(|e| err!("read {path}: {e}"))?;
        if bytes.len() > MAX_BODY_FILE {
            return Err(Error::new("body file exceeds 512 KiB"));
        }
        r.body = String::from_utf8(bytes).map_err(|_| Error::new("body file is not valid UTF-8"))?;
    }
    if r.request_id.is_empty() {
        r.request_id = new_request_id()?;
    }
    if MUTATING_OPS.contains(&op) {
        eprintln!("request-id: {} (retry with identical arguments)", r.request_id);
    }
    let mut source = profile;
    if op == "join" {
        let Some(join_file) = a.join_file.as_deref().filter(|p| !p.is_empty()) else {
            return Err(Error::new("join needs --join-file"));
        };
        source = join_file;
        if Path::new(profile).exists() {
            return Err(Error::new("profile already exists; reuse it or choose a new path"));
        }
    }
    let c = read_connection(Path::new(source))?;
    r.client_id = c.id.clone();
    if op == "poll" {
        let raw = call_wait(&c, &r, poll_timeout(&a.timeout)?, None)?;
        print_result(&raw, a.json);
        return Ok(());
    }
    if op == "submit" {
        r.snapshot = task_snapshot(&c, &r.task_id, None)?;
    } else if op == "review" {
        r.snapshot = task_snapshot(&c, &r.task_id, Some(&r.snapshot))?;
    }
    let mut raw = call(&c, &r)?;
    if op == "join" {
        let issued: Client = serde_json::from_str(raw.get())?;
        let saved = Connection {
            url: c.url.clone(),
            token: issued.token.clone(),
            id: issued.id.clone(),
            role: issued.role.clone(),
        };
        save_private(Path::new(profile), &saved, true).map_err(|e| {
            err!("identity issued but profile save failed: {e}; retry identical --request-id {}", r.request_id)
        })?;
        let summary = serde_json::json!({
            "id": issued.id,
            "role": issued.role,
            "profile": profile,
            "next": "use status/tasks/poll with --profile",
        });
        raw = RawValue::from_string(serde_json::to_string(&summary)?)?;
    }
    print_result(&raw, a.json);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<ClientArgs> {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        ClientArgs::try_parse_from(owned).map_err(parse_error)
    }

    #[test]
    fn defaults_match_the_go_flag_set() {
        let a = parse(&["--profile", "p.json"]).unwrap();
        let r = build_request("poll", &a);
        assert_eq!((r.after, r.limit, r.mode.as_str()), (-1, 100, "write"));
        assert_eq!(a.timeout, "25s");
    }

    #[test]
    fn negative_and_dash_values_are_values_not_flags() {
        let a = parse(&["--after", "-1", "--body", "--not-a-flag", "--version", "7"]).unwrap();
        assert_eq!(a.after, -1);
        assert_eq!(a.body, "--not-a-flag");
        assert_eq!(a.version, Some(7));
    }

    #[test]
    fn reason_is_sent_as_the_body() {
        let a = parse(&["--reason", "reset queue"]).unwrap();
        assert_eq!(build_request("clear-tasks", &a).body, "reset queue");
    }

    #[test]
    fn unknown_flags_and_positionals_are_rejected() {
        assert!(parse(&["--bogus", "1"]).is_err());
        assert!(parse(&["stray"]).is_err());
        assert!(parse(&["--version", "abc"]).is_err());
    }

    #[test]
    fn poll_timeout_is_bounded() {
        assert_eq!(poll_timeout("25s").unwrap(), Duration::from_secs(25));
        assert_eq!(poll_timeout("0").unwrap(), Duration::ZERO);
        assert!(poll_timeout("31s").is_err());
        assert!(poll_timeout("-1s").is_err());
        assert!(poll_timeout("soon").is_err());
    }

    #[test]
    fn unknown_command_is_refused_before_parsing() {
        let e = client_command("frobnicate", &[]).unwrap_err();
        assert!(e.message().contains("unknown command"));
    }
}
