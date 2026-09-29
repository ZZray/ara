//! HTTP layer tests against a real listener and store (ports of `main_test.go`
//! `TestHTTPPoll*` and `TestHTTPRejectsBrowserAndExtraJSON`, plus the edge cases the
//! Go `net/http` server handled implicitly).

mod common;

use ara_lite::client::{call, call_wait};
use ara_lite::types::{Connection, PollResult, Request};
use common::{Service, join_with};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

/// Send raw bytes and return `(status, body)` of the single response.
fn raw_exchange(url: &str, request: &str) -> (u16, String) {
    let addr = url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut out = Vec::new();
    // A reset after the response was delivered is fine; whatever arrived is what counts.
    let _ = stream.read_to_end(&mut out);
    let text = String::from_utf8_lossy(&out).into_owned();
    let status = text.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("no status in {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn post(url: &str, extra_headers: &str, body: &str) -> (u16, String) {
    let request = format!(
        "POST /api HTTP/1.1\r\nHost: x\r\nContent-Length: {}\r\nConnection: close\r\n{extra_headers}\r\n{body}",
        body.len()
    );
    raw_exchange(url, &request)
}

fn poll_request(after: i64) -> Request {
    Request { op: "poll".into(), after, limit: 100, ..Request::default() }
}

#[test]
fn poll_wakes_only_for_a_directed_durable_message() {
    let svc = Service::start();
    let master = svc.join("master", "master");
    let w1 = svc.join("worker", "one");
    let w2 = svc.join("worker", "two");
    let (tx, rx) = std::sync::mpsc::channel();
    let waiter: Connection = w1.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let raw = call_wait(&waiter, &poll_request(-1), Duration::from_secs(2), None);
        let _ = tx.send((raw.map(|r| r.get().to_string()), started.elapsed()));
    });
    // Give the long poll time to register before sending anything.
    std::thread::sleep(Duration::from_millis(200));
    let send = |to: &Connection, body: &str, id: &str| {
        let r = Request {
            op: "send".into(),
            worker_id: to.id.clone(),
            body: body.into(),
            request_id: id.into(),
            ..Request::default()
        };
        call(&master, &r).unwrap();
    };
    send(&w2, "only two", "msg2");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "another worker's message woke the poll");
    send(&w1, "only one", "msg1");
    let (raw, elapsed) = rx.recv_timeout(Duration::from_secs(2)).expect("message did not wake the poll promptly");
    let poll: PollResult = serde_json::from_str(&raw.unwrap()).unwrap();
    assert_eq!(poll.messages.len(), 1, "wrong routing: {poll:?}");
    assert_eq!(poll.messages[0].body, "only one");
    assert!(elapsed < Duration::from_millis(1900), "poll ran to its timeout: {elapsed:?}");
}

#[test]
fn poll_includes_task_ids() {
    let svc = Service::start();
    let master = svc.join("master", "master");
    let worker = svc.join("worker", "worker");
    for id in ["task-a", "task-b"] {
        let workspace = svc.dir.path().join(id);
        std::fs::create_dir(&workspace).unwrap();
        let r = Request {
            op: "create".into(),
            task_id: id.into(),
            title: id.into(),
            workspace: workspace.to_string_lossy().into_owned(),
            mode: "read".into(),
            worker_id: worker.id.clone(),
            request_id: id.into(),
            ..Request::default()
        };
        call(&master, &r).unwrap();
    }
    let send = Request {
        op: "send".into(),
        worker_id: worker.id.clone(),
        task_id: "task-a".into(),
        body: "selected task".into(),
        request_id: "selected".into(),
        ..Request::default()
    };
    call(&master, &send).unwrap();
    let raw = call(&worker, &poll_request(-1)).unwrap();
    let poll: PollResult = serde_json::from_str(raw.get()).unwrap();
    let seen: std::collections::BTreeSet<&str> = poll.messages.iter().map(|m| m.task_id.as_str()).collect();
    assert!(seen.contains("task-a") && seen.contains("task-b"), "poll lost task routing: {}", raw.get());
}

#[test]
fn poll_without_messages_waits_then_returns_an_empty_page() {
    let svc = Service::start();
    let worker = svc.join("worker", "worker");
    let started = Instant::now();
    let raw = call_wait(&worker, &poll_request(-1), Duration::from_millis(1200), None).unwrap();
    let elapsed = started.elapsed();
    let poll: PollResult = serde_json::from_str(raw.get()).unwrap();
    assert!(poll.messages.is_empty() && !poll.has_new && !poll.retention_gap, "{}", raw.get());
    assert!(elapsed >= Duration::from_millis(1000), "returned before the timeout: {elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "took far longer than the timeout: {elapsed:?}");
    // wait == 0 answers immediately.
    let started = Instant::now();
    call(&worker, &poll_request(-1)).unwrap();
    assert!(started.elapsed() < Duration::from_millis(800));
}

#[test]
fn rejects_browser_origin_and_extra_json() {
    let svc = Service::start();
    let (status, body) = post(&svc.url, "", r#"{"op":"status"} {}"#);
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(&svc.url, "Origin: http://example.test\r\n", r#"{"op":"status"} {}"#);
    assert_eq!(status, 403, "{body}");
    // Origin is checked before the body is even parsed.
    let (status, _) = post(&svc.url, "Origin: http://example.test\r\n", "not json");
    assert_eq!(status, 403);
}

#[test]
fn request_shape_and_credential_checks() {
    let svc = Service::start();
    let bearer = "Authorization: Bearer nope\r\n";
    // Unknown fields are rejected, like Go's DisallowUnknownFields.
    let (status, body) = post(&svc.url, bearer, r#"{"op":"status","surprise":1}"#);
    assert_eq!(status, 400, "{body}");
    // No credential.
    let (status, body) = post(&svc.url, "", r#"{"op":"status"}"#);
    assert_eq!(status, 401, "{body}");
    // A wrong credential is an operation error.
    let (status, body) = post(&svc.url, bearer, r#"{"op":"status"}"#);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("\"error\""), "{body}");
    // Poll timeout bounds are checked at the HTTP layer.
    let request = |query: &str| {
        let body = r#"{"op":"poll","after":-1,"limit":100}"#;
        raw_exchange(
            &svc.url,
            &format!(
                "POST /api{query} HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer nope\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    };
    assert_eq!(request("?timeout=31s").0, 400);
    assert_eq!(request("?timeout=-1s").0, 400);
    assert_eq!(request("?timeout=soon").0, 400);
    // Within bounds the request reaches the store (which rejects the credential).
    assert_eq!(request("?timeout=0s").0, 409);
}

#[test]
fn health_routes_methods_and_body_limits() {
    let svc = Service::start();
    let (status, body) = raw_exchange(&svc.url, "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!((status, body.as_str()), (200, "{\"status\":\"ready\"}"));
    let (status, _) = raw_exchange(&svc.url, "GET /api HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status, 405);
    let (status, _) = raw_exchange(&svc.url, "GET /elsewhere HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status, 404);
    // Chunked request bodies are refused with a clear status instead of being misread.
    let (status, body) = raw_exchange(
        &svc.url,
        "POST /api HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n0\r\n\r\n",
    );
    assert_eq!(status, 411, "{body}");
    // Bodies over 1 MiB are refused before being read.
    let (status, body) =
        raw_exchange(&svc.url, "POST /api HTTP/1.1\r\nHost: x\r\nContent-Length: 1048577\r\nConnection: close\r\n\r\n");
    assert_eq!(status, 400, "{body}");
    // Garbage does not crash the listener.
    let (status, _) = raw_exchange(&svc.url, "NOT-HTTP\r\n\r\n");
    assert_eq!(status, 400);
    let (status, _) = raw_exchange(&svc.url, "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    assert_eq!(status, 200);
}

#[test]
fn a_join_with_the_wrong_role_key_is_refused() {
    let svc = Service::start();
    let (_, worker_key) = svc.store.join_keys();
    let bootstrap =
        Connection { url: svc.url.clone(), token: worker_key.clone(), id: String::new(), role: "master".into() };
    let request = Request {
        op: "join".into(),
        role: "master".into(),
        name: "m".into(),
        request_id: "m".into(),
        ..Request::default()
    };
    let e = call(&bootstrap, &request).unwrap_err();
    assert!(e.message().starts_with("request rejected (409)"), "{e}");
    // The right key works, and the resulting credential differs from the join key.
    let master = join_with(&svc.url, &svc.store.join_keys().0, "master", "m");
    assert_ne!(master.token, worker_key);
    assert!(!master.id.is_empty());
}
