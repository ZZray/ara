//! Streamable HTTP against an in-test server: JSON and SSE replies,
//! `Mcp-Session-Id`, `MCP-Protocol-Version`, header expansion, server
//! requests answered with `-32601`, timeout with `notifications/cancelled`,
//! a stream that ends before the reply, and session DELETE (42.3, 42.6, 42.7).
mod common;

use std::{
    collections::{BTreeMap, HashMap},
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use ara_agent_lite_core::effect_status::reports_unknown;
use ara_agent_lite_mcp::{McpConfig, ServerConfig, ServerStatus};
use common::*;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const SESSION: &str = "sess-fixture-1";
const TOKEN: &str = "tok-default-123";

#[derive(Default)]
struct State {
    version: String,
    seen: Vec<(String, HashMap<String, String>, Value)>,
    client_replies: HashMap<String, Value>,
}

struct Server {
    url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.url.trim_start_matches("http://").trim_end_matches("/mcp"));
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, extra: &str, body: &str) {
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}",
        body.len()
    );
}

fn sse_start(stream: &mut TcpStream) {
    let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
}

fn sse_event(stream: &mut TcpStream, message: &Value) {
    let _ = write!(stream, "event: message\ndata: {message}\n\n");
    let _ = stream.flush();
}

fn handle(mut stream: TcpStream, state: Arc<Mutex<State>>) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let Ok(n) = stream.read(&mut chunk) else { return };
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]).to_string();
    let verb = head.split(' ').next().unwrap_or_default().to_string();
    let headers: HashMap<String, String> = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let length = headers.get("content-length").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    while bytes.len() < header_end + length {
        let Ok(n) = stream.read(&mut chunk) else { return };
        if n == 0 {
            return;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    let body: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap_or(Value::Null);
    state.lock().unwrap().seen.push((verb.clone(), headers.clone(), body.clone()));
    if verb == "DELETE" {
        return respond(&mut stream, "200 OK", "text/plain", "", "");
    }
    let method = body["method"].as_str().unwrap_or_default().to_string();
    if method.is_empty() {
        let id = body["id"].as_str().unwrap_or_default().to_string();
        state.lock().unwrap().client_replies.insert(id, body);
        return respond(&mut stream, "202 Accepted", "text/plain", "", "");
    }
    if method.starts_with("notifications/") {
        return respond(&mut stream, "202 Accepted", "text/plain", "", "");
    }
    let id = body["id"].clone();
    let version = state.lock().unwrap().version.clone();
    let reply = |result: Value| json!({"jsonrpc":"2.0","id":id,"result":result}).to_string();
    if headers.get("authorization").map(String::as_str) != Some(&format!("Bearer {TOKEN}")) {
        return respond(&mut stream, "401 Unauthorized", "text/plain", "", "");
    }
    if method == "initialize" {
        let result = json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"http-fixture","version":"1"}});
        return respond(
            &mut stream,
            "200 OK",
            "application/json",
            &format!("Mcp-Session-Id: {SESSION}\r\n"),
            &reply(result),
        );
    }
    if headers.get("mcp-session-id").map(String::as_str) != Some(SESSION)
        || headers.get("mcp-protocol-version") != Some(&version)
    {
        return respond(&mut stream, "400 Bad Request", "text/plain", "", "missing session or version header");
    }
    match (method.as_str(), body["params"]["name"].as_str()) {
        ("tools/list", _) => {
            sse_start(&mut stream);
            sse_event(
                &mut stream,
                &json!({"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info"}}),
            );
            let tool = |name: &str| json!({"name":name,"inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}});
            let tools = json!({"tools":[tool("echo"),tool("ask"),tool("slow"),tool("drop")]});
            sse_event(&mut stream, &serde_json::from_str::<Value>(&reply(tools)).unwrap());
        }
        ("tools/call", Some("echo")) => {
            let text = body["params"]["arguments"]["text"].clone();
            respond(
                &mut stream,
                "200 OK",
                "application/json",
                "",
                &reply(json!({"content":[{"type":"text","text":text}]})),
            );
        }
        ("tools/call", Some("ask")) => {
            sse_start(&mut stream);
            sse_event(&mut stream, &json!({"jsonrpc":"2.0","id":"s1","method":"sampling/createMessage","params":{}}));
            let mut code = Value::Null;
            for _ in 0..100 {
                if let Some(answer) = state.lock().unwrap().client_replies.get("s1") {
                    code = answer["error"]["code"].clone();
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            let result = json!({"content":[{"type":"text","text":format!("code:{code}")}]});
            sse_event(&mut stream, &serde_json::from_str::<Value>(&reply(result)).unwrap());
        }
        ("tools/call", Some("slow")) => {
            thread::sleep(Duration::from_secs(3));
            respond(&mut stream, "200 OK", "application/json", "", &reply(json!({"content":[]})));
        }
        ("tools/call", Some("drop")) => sse_start(&mut stream),
        _ => respond(&mut stream, "404 Not Found", "text/plain", "", ""),
    }
}

fn server(version: &str) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let state = Arc::new(Mutex::new(State { version: version.into(), ..Default::default() }));
    let stop = Arc::new(AtomicBool::new(false));
    let (shared, done) = (state.clone(), stop.clone());
    thread::spawn(move || {
        for stream in listener.incoming() {
            if done.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let state = shared.clone();
            thread::spawn(move || handle(stream, state));
        }
    });
    Server { url, state, stop }
}

fn http_config(url: &str) -> McpConfig {
    let headers = BTreeMap::from([(
        "Authorization".to_string(),
        "Bearer ${ARA_MCP_HTTP_UNSET_FOR_TEST:-tok-default-123}".to_string(),
    )]);
    config(vec![("remote", ServerConfig::Http { url: url.into(), headers })])
}

#[tokio::test]
async fn session_headers_json_and_sse_replies() {
    let fixture = server("2025-03-26");
    let session = connect(http_config(&fixture.url), options()).await;
    assert!(matches!(session.status()["remote"], ServerStatus::Connected { tools: 4, .. }), "{:?}", session.status());
    let echo =
        run(&tool(&session, "mcp__remote__echo"), json!({"text":"over http"}), CancellationToken::new()).await.unwrap();
    assert_eq!(text_of(&echo), "over http");
    let ask = run(&tool(&session, "mcp__remote__ask"), json!({}), CancellationToken::new()).await.unwrap();
    assert_eq!(text_of(&ask), "code:-32601");
    session.shutdown().await;
    let state = fixture.state.lock().unwrap();
    let delete = state.seen.iter().find(|(verb, _, _)| verb == "DELETE").expect("session DELETE");
    assert_eq!(delete.1.get("mcp-session-id").map(String::as_str), Some(SESSION));
    assert!(state.seen.iter().any(|(_, _, body)| body["method"] == "notifications/initialized"));
    assert_eq!(state.seen[0].2["params"]["protocolVersion"], "2025-11-25");
}

#[tokio::test]
async fn timeout_and_early_stream_end_are_unknown_effects() {
    let fixture = server("2025-06-18");
    let mut opts = options();
    opts.call_timeout = Duration::from_millis(500);
    let session = connect(http_config(&fixture.url), opts).await;
    let slow = run(&tool(&session, "mcp__remote__slow"), json!({}), CancellationToken::new()).await.unwrap();
    assert_eq!(slow.details["source"], "mcp_call_timeout");
    assert!(reports_unknown(&slow.details));
    let mut cancelled = false;
    for _ in 0..40 {
        cancelled =
            fixture.state.lock().unwrap().seen.iter().any(|(_, _, body)| body["method"] == "notifications/cancelled");
        if cancelled {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(cancelled, "notifications/cancelled was not posted");
    let dropped = run(&tool(&session, "mcp__remote__drop"), json!({}), CancellationToken::new()).await.unwrap();
    assert_eq!(dropped.details["source"], "mcp_connection_lost");
    let calls = fixture.state.lock().unwrap().seen.iter().filter(|(_, _, body)| body["method"] == "tools/call").count();
    assert_eq!(calls, 2, "no retries");
    session.shutdown().await;
}

#[tokio::test]
async fn unsupported_version_and_rejected_auth_fail_the_server() {
    let old = server("2024-11-05");
    let session = connect(http_config(&old.url), options()).await;
    assert!(
        matches!(&session.status()["remote"], ServerStatus::Failed { reason } if reason.contains("unsupported protocol version"))
    );
    let fixture = server("2025-06-18");
    let headers = BTreeMap::from([("Authorization".to_string(), "Bearer wrong-token-value".to_string())]);
    let config = config(vec![("remote", ServerConfig::Http { url: fixture.url.clone(), headers })]);
    let session = connect(config, options()).await;
    let ServerStatus::Failed { reason } = &session.status()["remote"] else { panic!() };
    assert!(reason.contains("HTTP 401") && !reason.contains("wrong-token-value"), "{reason}");
}
