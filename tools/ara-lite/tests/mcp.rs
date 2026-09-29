//! MCP stdio server tests (port of `mcp_test.go`). The server runs as the real `ara-lite mcp`
//! process; its upstream is a mock HTTP service built from the crate's own request parser,
//! so the exact requests it sends can be asserted.

mod common;

use ara_lite::profile::save_private;
use ara_lite::server::{HttpRequest, read_request, write_response};
use ara_lite::types::{Connection, Request};
use common::McpProcess;
use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SECRET: &str = "fixture-private-token";

/// A loopback HTTP listener that hands every parsed request to `handler`.
struct Upstream {
    url: String,
    stop: Arc<AtomicBool>,
}

impl Upstream {
    fn start(handler: impl Fn(&mut TcpStream, HttpRequest) + Send + Sync + 'static) -> Upstream {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(handler);
        let flag = stop.clone();
        std::thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let handler = handler.clone();
                        std::thread::spawn(move || {
                            let now = Instant::now();
                            if let Ok(request) =
                                read_request(&mut stream, now + Duration::from_secs(5), now + Duration::from_secs(5))
                            {
                                handler(&mut stream, request);
                            }
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Upstream { url, stop }
    }
}

impl Drop for Upstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn profile(dir: &Path, url: &str) -> PathBuf {
    let path = dir.join("profile.json");
    save_private(
        &path,
        &Connection { url: url.into(), token: SECRET.into(), id: "client-1".into(), role: "worker".into() },
        false,
    )
    .unwrap();
    path
}

/// What the mock saw for one call.
struct Seen {
    request: Request,
    authorization: String,
    target: String,
}

/// An upstream that records every request and answers with `answer`.
fn recording(answer: &'static str) -> (Upstream, Receiver<Seen>) {
    let (tx, rx) = channel();
    let tx: Mutex<Sender<Seen>> = Mutex::new(tx);
    let upstream = Upstream::start(move |stream, http| {
        let request: Request = serde_json::from_slice(&http.body).expect("the MCP server sent a malformed request");
        let seen = Seen {
            request,
            authorization: http.header("authorization").unwrap_or_default().to_string(),
            target: if http.query.is_empty() { http.path.clone() } else { format!("{}?{}", http.path, http.query) },
        };
        let _ = tx.lock().unwrap().send(seen);
        let _ = write_response(stream, 200, true, answer.as_bytes());
    });
    (upstream, rx)
}

fn next(rx: &Receiver<Seen>) -> Seen {
    rx.recv_timeout(Duration::from_secs(5)).expect("the MCP server did not reach the service")
}

#[test]
fn tool_catalog_covers_the_lifecycle_and_never_contains_the_credential() {
    let (upstream, _rx) = recording("{\"result\":{}}");
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let reply = mcp.request("tools/list", json!({}));
    let text = reply.to_string();
    assert!(!text.contains(SECRET) && !text.contains("client-1"), "credential or identity leaked in the catalog");
    let tools = reply["result"]["tools"].as_array().unwrap();
    let mut names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    names.sort_unstable();
    let mut want = vec![
        "ack_messages",
        "claim_task",
        "clear_tasks",
        "confirm_stop",
        "create_task",
        "heartbeat",
        "list_clients",
        "list_tasks",
        "poll_messages",
        "review_task",
        "send_message",
        "snapshot_task",
        "submit_task",
        "update_progress",
    ];
    want.sort_unstable();
    assert_eq!(names, want);
    for tool in tools {
        let name = tool["name"].as_str().unwrap();
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        assert!(tool["description"].as_str().unwrap().len() < 300, "{name}: description too long");
        // Read-only tools say so; mutating tools are declared retry-safe.
        let read_only = ["poll_messages", "list_tasks", "list_clients", "snapshot_task"].contains(&name);
        assert_eq!(tool["annotations"]["readOnlyHint"] == true, read_only, "{name}");
    }
    let ping = mcp.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));
}

#[test]
fn poll_forwards_defaults_and_passes_large_integers_through() {
    let (upstream, rx) = recording(
        r#"{"result":{"messages":[{"seq":9007199254740993,"kind":"chat","body":"hello"}],"next_cursor":9007199254740993,"has_more":false,"retention_gap":false}}"#,
    );
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let id = mcp.send_request("tools/call", json!({ "name": "poll_messages", "arguments": { "timeout_seconds": 1 } }));
    let seen = next(&rx);
    assert_eq!(seen.authorization, format!("Bearer {SECRET}"));
    assert_eq!(seen.target, "/api?timeout=1000ms");
    assert_eq!((seen.request.op.as_str(), seen.request.after, seen.request.limit), ("poll", -1, 100));
    assert_eq!(seen.request.client_id, "client-1");
    let line = {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let line = mcp.next_line(deadline.saturating_duration_since(Instant::now())).expect("no poll response");
            if line.contains(&format!("\"id\":{id}")) {
                break line;
            }
        }
    };
    // The service's numbers survive untouched in both the text and the structured content.
    assert_eq!(line.matches("9007199254740993").count(), 4, "{line}");
    assert!(!line.contains(SECRET));
    let reply: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(reply["result"]["structuredContent"]["messages"][0]["seq"].as_u64(), Some(9_007_199_254_740_993));
}

#[test]
fn poll_without_a_timeout_waits_five_seconds() {
    let (upstream, rx) =
        recording(r#"{"result":{"messages":[],"next_cursor":0,"has_more":false,"retention_gap":false}}"#);
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    mcp.send_request("tools/call", json!({ "name": "poll_messages", "arguments": {} }));
    assert_eq!(next(&rx).target, "/api?timeout=5000ms");
}

#[test]
fn mutating_tools_forward_exact_arguments() {
    let (upstream, rx) = recording("{\"result\":{\"ok\":true}}");
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let calls = [
        ("ack_messages", json!({ "expected_cursor": 3, "ack_seq": 5 })),
        ("send_message", json!({ "task_id": "task-1", "body": "update", "request_id": "send-1" })),
        (
            "update_progress",
            json!({
                "task_id": "task-1", "version": 4, "generation": 2, "percent": 35, "description": "tests running",
                "subtask_id": "tests", "subtask_title": "Run tests", "request_id": "progress-1"
            }),
        ),
        (
            "create_task",
            json!({ "title": "T", "workspace": "/w", "body": "do it", "worker_id": "w9", "request_id": "create-1" }),
        ),
        ("claim_task", json!({ "task_id": "task-1", "version": 1, "generation": 0, "request_id": "claim-1" })),
        ("heartbeat", json!({ "task_id": "task-1", "version": 5, "generation": 1, "request_id": "hb-1" })),
        (
            "confirm_stop",
            json!({ "task_id": "task-1", "version": 6, "generation": 1, "evidence": "process gone", "request_id": "stop-1" }),
        ),
        (
            "clear_tasks",
            json!({ "workspace": "/w", "expected_count": 2, "reason": "reset queue", "request_id": "clear-1" }),
        ),
        ("list_tasks", json!({ "task_id": "task-1" })),
        ("list_clients", json!({})),
    ];
    let mut sent = Vec::new();
    for (name, arguments) in calls {
        let (is_error, payload) = mcp.tool(name, arguments);
        assert!(!is_error, "{name}: {payload}");
        sent.push((name, next(&rx)));
    }
    let by_name = |name: &str| &sent.iter().find(|(n, _)| *n == name).unwrap().1.request;
    let ack = by_name("ack_messages");
    assert_eq!((ack.op.as_str(), ack.expected_cursor, ack.ack_seq), ("ack", 3, 5));
    // Without a request_id the server invents one, so a lost reply can still be retried by the model.
    assert_eq!(ack.request_id, "");
    let send = by_name("send_message");
    assert_eq!((send.op.as_str(), send.task_id.as_str(), send.body.as_str()), ("send", "task-1", "update"));
    assert_eq!(send.request_id, "send-1");
    let p = by_name("update_progress");
    assert_eq!(
        (p.op.as_str(), p.task_id.as_str(), p.version, p.generation, p.percent),
        ("progress", "task-1", Some(4), Some(2), 35)
    );
    assert_eq!(
        (p.description.as_str(), p.subtask_id.as_str(), p.subtask_title.as_str()),
        ("tests running", "tests", "Run tests")
    );
    assert_eq!(p.request_id, "progress-1");
    let c = by_name("create_task");
    assert_eq!(
        (c.op.as_str(), c.mode.as_str(), c.title.as_str(), c.workspace.as_str()),
        ("create", "write", "T", "/w")
    );
    assert_eq!((c.body.as_str(), c.worker_id.as_str(), c.request_id.as_str()), ("do it", "w9", "create-1"));
    let claim = by_name("claim_task");
    assert_eq!(
        (claim.op.as_str(), claim.task_id.as_str(), claim.version, claim.generation),
        ("claim", "task-1", Some(1), Some(0))
    );
    let hb = by_name("heartbeat");
    assert_eq!(
        (hb.op.as_str(), hb.task_id.as_str(), hb.version, hb.generation),
        ("heartbeat", "task-1", Some(5), Some(1))
    );
    let stop = by_name("confirm_stop");
    assert_eq!((stop.op.as_str(), stop.evidence.as_str(), stop.version), ("confirm-stop", "process gone", Some(6)));
    let clear = by_name("clear_tasks");
    assert_eq!((clear.op.as_str(), clear.workspace.as_str(), clear.expected_count), ("clear-tasks", "/w", 2));
    assert_eq!(clear.body, "reset queue");
    assert_eq!(by_name("list_tasks").op, "tasks");
    assert_eq!(by_name("list_tasks").task_id, "task-1");
    assert_eq!(by_name("list_clients").op, "clients");
}

#[test]
fn mutation_without_a_request_id_gets_a_generated_one() {
    let (upstream, rx) = recording("{\"result\":{}}");
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let (is_error, _) = mcp.tool("send_message", json!({ "body": "hi" }));
    assert!(!is_error);
    let id = next(&rx).request.request_id;
    assert_eq!(id.len(), 32, "{id}");
    assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn invalid_input_is_a_tool_error_and_protocol_mistakes_are_rpc_errors() {
    let (upstream, rx) = recording("{\"result\":{}}");
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    for (name, arguments, needle) in [
        ("poll_messages", json!({ "timeout_seconds": 99 }), "timeout_seconds"),
        ("poll_messages", json!({ "limit": 0 }), "limit"),
        ("poll_messages", json!({ "after": -2 }), "after"),
        ("ack_messages", json!({ "expected_cursor": 5, "ack_seq": 2 }), "ack_seq"),
        ("ack_messages", json!({ "expected_cursor": 1 }), "ack_seq"),
        ("send_message", json!({ "body": "" }), "body is required"),
        ("send_message", json!({ "body": "x", "surprise": 1 }), "surprise"),
        (
            "update_progress",
            json!({ "task_id": "t", "version": 1, "generation": 1, "percent": 101, "description": "d" }),
            "percent",
        ),
    ] {
        let (is_error, payload) = mcp.tool(name, arguments);
        assert!(is_error, "{name} accepted bad input");
        assert!(payload.as_str().unwrap().contains(needle), "{name}: {payload}");
    }
    assert!(rx.recv_timeout(Duration::from_millis(200)).is_err(), "invalid input still reached the service");
    let unknown = mcp.request("tools/call", json!({ "name": "delete_everything", "arguments": {} }));
    assert_eq!(unknown["error"]["code"], -32602);
    let missing = mcp.request("resources/list", json!({}));
    assert_eq!(missing["error"]["code"], -32601);
    // Garbage on the wire is answered, not fatal.
    mcp.send_raw("this is not json");
    let line = mcp.next_line(Duration::from_secs(5)).expect("no parse error reply");
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["error"]["code"], -32700);
    let ping = mcp.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));
}

#[test]
fn initialize_negotiates_a_supported_protocol_version() {
    let (upstream, _rx) = recording("{\"result\":{}}");
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let old =
        mcp.request("initialize", json!({ "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {} }));
    assert_eq!(old["result"]["protocolVersion"], "2024-11-05");
    let future =
        mcp.request("initialize", json!({ "protocolVersion": "2099-01-01", "capabilities": {}, "clientInfo": {} }));
    assert_eq!(future["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(future["result"]["serverInfo"]["name"], "ara-lite");
    assert!(future["result"]["capabilities"]["tools"].is_object());
}

/// An upstream that accepts a request, reports it, then holds the connection until the peer hangs up.
fn holding() -> (Upstream, Receiver<()>, Receiver<()>) {
    let (entered_tx, entered_rx) = channel();
    let (closed_tx, closed_rx) = channel();
    let entered_tx = Mutex::new(entered_tx);
    let closed_tx = Mutex::new(closed_tx);
    let upstream = Upstream::start(move |stream, _request| {
        let _ = entered_tx.lock().unwrap().send(());
        let mut byte = [0u8; 1];
        let _ = stream.set_read_timeout(Some(Duration::from_secs(20)));
        // A read of 0 bytes (or a reset) means the client closed the connection.
        let _ = std::io::Read::read(stream, &mut byte);
        let _ = closed_tx.lock().unwrap().send(());
    });
    (upstream, entered_rx, closed_rx)
}

#[test]
fn poll_cancellation_closes_the_http_request_and_sends_no_response() {
    let (upstream, entered, closed) = holding();
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    let id = mcp.send_request("tools/call", json!({ "name": "poll_messages", "arguments": { "timeout_seconds": 30 } }));
    entered.recv_timeout(Duration::from_secs(3)).expect("poll did not reach the service");
    let started = Instant::now();
    mcp.notify("notifications/cancelled", json!({ "requestId": id, "reason": "user cancelled" }));
    closed.recv_timeout(Duration::from_secs(3)).expect("the HTTP poll stayed open after MCP cancellation");
    assert!(started.elapsed() < Duration::from_secs(3));
    // A cancelled request gets no response, and the server keeps working.
    assert!(mcp.next_line(Duration::from_millis(500)).is_none(), "a cancelled call was answered");
    let ping = mcp.request("ping", json!({}));
    assert_eq!(ping["result"], json!({}));
}

#[test]
fn ending_stdin_stops_the_server_and_abandons_polls() {
    let (upstream, entered, closed) = holding();
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &upstream.url));
    mcp.send_request("tools/call", json!({ "name": "poll_messages", "arguments": { "timeout_seconds": 30 } }));
    entered.recv_timeout(Duration::from_secs(3)).expect("poll did not reach the service");
    let status = mcp.close().expect("the MCP server did not exit after its input ended");
    assert!(status.success(), "{status:?}");
    closed.recv_timeout(Duration::from_secs(3)).expect("the HTTP poll stayed open after the server exited");
}

#[test]
fn an_unreachable_service_names_the_retry_key() {
    // Bind then drop a listener so the port is closed.
    let url = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let dir = tempfile::tempdir().unwrap();
    let mut mcp = McpProcess::start(&profile(dir.path(), &url));
    let (is_error, payload) = mcp.tool("send_message", json!({ "body": "hi", "request_id": "retry-me" }));
    assert!(is_error);
    let text = payload.as_str().unwrap();
    assert!(text.contains("service unavailable") && text.contains("request_id retry-me"), "{text}");
    assert!(!text.contains(SECRET));
    let (is_error, payload) = mcp.tool("list_tasks", json!({}));
    assert!(is_error && payload.as_str().unwrap().contains("service unavailable"), "{payload}");
}

#[test]
fn profile_problems_fail_before_serving() {
    let dir = tempfile::tempdir().unwrap();
    for args in [vec!["mcp"], vec!["mcp", "--profile", ""], vec!["mcp", "--profile", "missing.json"]] {
        let out = common::cli(&args);
        assert!(!out.status.success(), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("ara-lite:"), "{args:?}");
    }
    let bad = dir.path().join("bad.json");
    save_private(
        &bad,
        &Connection { url: "https://example.com".into(), token: "t".into(), id: String::new(), role: "worker".into() },
        false,
    )
    .unwrap();
    let out = common::cli(&["mcp", "--profile", bad.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("loopback"));
}
