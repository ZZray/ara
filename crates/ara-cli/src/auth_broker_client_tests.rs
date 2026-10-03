//! Grouped transport traces from fixed OMP auth-broker-wire.test.ts and
//! pi-utils stream.test.ts, plus the recorded unknown-mutation adaptation.
use super::*;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};

struct Reply {
    wire: Option<Vec<u8>>,
    delay: Duration,
}
impl Reply {
    fn bytes(status: u16, content_type: &str, body: &[u8]) -> Self {
        let mut wire = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
        wire.extend_from_slice(body);
        Self { wire: Some(wire), delay: Duration::ZERO }
    }
    fn json(status: u16, body: Value) -> Self {
        Self::bytes(status, "application/json", body.to_string().as_bytes())
    }
    fn header(status: u16, key: &str, value: &str, body: Value) -> Self {
        let body = body.to_string();
        Self { wire: Some(format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\n{key}: {value}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()), delay: Duration::ZERO }
    }
    fn disconnected() -> Self {
        Self { wire: None, delay: Duration::ZERO }
    }
}
#[derive(Clone)]
struct RequestReceipt {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl RequestReceipt {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.as_str())
    }
}
struct Fixture {
    url: String,
    receipts: Arc<Mutex<Vec<RequestReceipt>>>,
    stop: CancellationToken,
    task: JoinHandle<()>,
}
impl Fixture {
    async fn new(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("fixture listener");
        let url = format!("http://{}", listener.local_addr().expect("fixture address"));
        let receipts = Arc::new(Mutex::new(Vec::new()));
        let stop = CancellationToken::new();
        let queue = Arc::new(Mutex::new(VecDeque::from(replies)));
        let task_receipts = receipts.clone();
        let task_stop = stop.clone();
        let task = tokio::spawn(async move {
            let mut requests = JoinSet::new();
            loop {
                tokio::select! { biased;
                    _ = task_stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((mut socket, _)) = accepted else { break; };
                        let receipts = task_receipts.clone();
                        let queue = queue.clone();
                        requests.spawn(async move {
                            let Some(receipt) = read_request(&mut socket).await else { return; };
                            receipts.lock().expect("receipt lock").push(receipt);
                            let reply = queue.lock().expect("reply lock").pop_front().unwrap_or_else(|| Reply::json(500, json!({"error":"unexpected request"})));
                            if !reply.delay.is_zero() { tokio::time::sleep(reply.delay).await; }
                            if let Some(wire) = reply.wire { let _ = socket.write_all(&wire).await; }
                            let _ = socket.shutdown().await;
                        });
                    },
                    Some(_) = requests.join_next(), if !requests.is_empty() => {},
                }
            }
            requests.abort_all();
            while requests.join_next().await.is_some() {}
        });
        Self { url, receipts, stop, task }
    }
    fn client(&self, max_retries: usize, request_timeout: Duration) -> AuthBrokerClient {
        AuthBrokerClient::new(
            &self.url,
            "fixture-bearer",
            AuthBrokerClientOptions {
                client: Client::builder().no_proxy().build().expect("fixture client"),
                max_retries,
                request_timeout,
            },
        )
        .expect("broker client")
    }
    fn requests(&self) -> Vec<RequestReceipt> {
        self.receipts.lock().expect("receipt lock").clone()
    }
    async fn wait_for_requests(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.requests().len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request dispatched");
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop.cancel();
        self.task.abort();
    }
}
async fn read_request(socket: &mut TcpStream) -> Option<RequestReceipt> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let end = loop {
        let count = socket.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            break end + 4;
        }
        if bytes.len() > 128 * 1024 {
            return None;
        }
    };
    let header = String::from_utf8_lossy(&bytes[..end]);
    let mut lines = header.lines();
    let mut start = lines.next()?.split_whitespace();
    let method = start.next()?.to_owned();
    let path = start.next()?.to_owned();
    let headers: Vec<_> = lines
        .filter_map(|line| line.split_once(':').map(|(name, value)| (name.to_owned(), value.trim().to_owned())))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    if length > 128 * 1024 {
        return None;
    }
    while bytes.len() < end + length {
        let count = socket.read(&mut buffer).await.ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    Some(RequestReceipt { method, path, headers, body: bytes[end..end + length].to_vec() })
}

fn entry() -> Value {
    json!({"id":7,"provider":"anthropic","credential":{"type":"oauth","access":"fixture-access","refresh":"__remote__","expires":5000,"tokenUrl":"https://provider.invalid/token"},"identityKey":"account:test"})
}
fn snapshot() -> Value {
    let mut entry = entry();
    entry["rotatesInMs"] = Value::Null;
    json!({"generation":2,"generatedAt":1000,"serverNowMs":2000,"refresher":{"enabled":false,"intervalMs":60000,"skewMs":300000,"nextSweepInMs":9007199254740991_u64},"credentials":[entry]})
}
fn observed() -> Value {
    json!({"installId":"fixture-install","app":"ara","entries":[{"at":1000,"provider":"anthropic","model":"fixture","requests":1,"inputTokens":2,"outputTokens":3,"cacheReadTokens":4,"cacheWriteTokens":5,"costUsd":0.01}]})
}

#[tokio::test]
async fn native_endpoint_headers_queries_and_payload_trace() {
    let fixture = Fixture::new(vec![
        Reply::json(200, json!({"ok":true,"version":"fixture"})),
        Reply::header(200, "ETag", "W/\"4\"", snapshot()),
        Reply::header(304, "ETag", "\"5\"", Value::Null),
        Reply::json(200, json!({"generatedAt":2000,"reports":[]})),
        Reply::json(200, json!({"generatedAt":2000,"entries":[]})),
        Reply::json(200, json!({"entry":entry()})),
        Reply::json(200, json!({"entries":[entry()]})),
        Reply::json(200, json!({"ok":true})), Reply::json(200, json!({"ok":true})),
        Reply::json(200, json!({"ok":true})), Reply::json(200, json!({"ok":false})),
        Reply::json(200, json!({"ok":true})),
        Reply::json(200, json!({"generatedAt":2000,"clients":[]})),
        Reply::json(404, json!({"error":"unsupported"})),
        Reply::json(200, json!({"generatedAt":2000,"disabled":[{"id":7,"provider":"anthropic","type":"oauth","cause":"revoked","disabledAtMs":2000.5}]})),
    ]).await;
    let client = fixture.client(1, Duration::from_secs(2));
    let cancel = CancellationToken::new();
    assert_eq!(client.healthz(&cancel).await.expect("health")["ok"], json!(true));
    match client.fetch_snapshot(Some(2), Some(10), &cancel).await.expect("snapshot") {
        BrokerSnapshotResult::Snapshot { snapshot, generation } => {
            assert_eq!(snapshot.generation, 2);
            assert_eq!(generation, 4);
        }
        _ => panic!("expected snapshot"),
    }
    assert!(matches!(
        client.fetch_snapshot(Some(4), Some(10), &cancel).await.expect("304"),
        BrokerSnapshotResult::NotModified { generation: 5 }
    ));
    client.fetch_usage(Some(3.5), &cancel).await.expect("usage");
    client.fetch_usage_history(Some(123.5), Some("a/b c"), &cancel).await.expect("history");
    assert_eq!(client.refresh_credential(7, &cancel).await.expect("refresh").id, 7);
    let mut real = entry()["credential"].clone();
    real["refresh"] = json!("fixture-real-refresh");
    let credential: AuthCredential = serde_json::from_value(real).expect("credential");
    assert_eq!(client.upload_credential("anthropic", &credential, &cancel).await.expect("upload").len(), 1);
    let block = BrokerBlock {
        provider_key: "anthropic:oauth".to_owned(),
        block_scope: "".to_owned(),
        blocked_until_ms: 4000.5,
        updated_at_ms: Some(3000.5),
    };
    assert!(client.upsert_credential_block(7, &block, &cancel).await.expect("block"));
    assert!(client.delete_credential_blocks(7, &cancel).await.expect("delete blocks"));
    assert!(client.disable_credential(7, "revoked", &cancel).await.expect("disable"));
    assert!(!client.notify_usage_stale(&cancel).await.expect("false receipt"));
    assert!(client.report_client_usage(&observed(), &cancel).await.expect("observed"));
    client.fetch_client_usage_summary(Some(123.5), &cancel).await.expect("summary");
    assert!(client.list_disabled_credentials(None, &cancel).await.expect("old broker").is_empty());
    let disabled = client.list_disabled_credentials(Some("a/b c"), &cancel).await.expect("disabled");
    assert_eq!(disabled[0].disabled_at_ms, Some(2000.5));
    let requests = fixture.requests();
    let actual: Vec<_> = requests.iter().map(|request| (request.method.as_str(), request.path.as_str())).collect();
    assert_eq!(
        actual,
        vec![
            ("GET", "/v1/healthz"),
            ("GET", "/v1/snapshot?wait=10"),
            ("GET", "/v1/snapshot?wait=10"),
            ("GET", "/v1/usage"),
            ("GET", "/v1/usage/history?sinceMs=123.5&provider=a%2Fb+c"),
            ("POST", "/v1/credential/7/refresh"),
            ("POST", "/v1/credential"),
            ("POST", "/v1/credential/7/block"),
            ("DELETE", "/v1/credential/7/blocks"),
            ("POST", "/v1/credential/7/disable"),
            ("POST", "/v1/usage/stale"),
            ("POST", "/v1/usage/observed"),
            ("GET", "/v1/usage/clients?sinceMs=123.5"),
            ("GET", "/v1/credentials/disabled"),
            ("GET", "/v1/credentials/disabled?provider=a%2Fb+c"),
        ]
    );
    assert!(requests[0].header("authorization").is_none());
    for request in &requests[1..] {
        assert_eq!(request.header("authorization"), Some("Bearer fixture-bearer"));
    }
    assert_eq!(
        requests[1].header(wire::AUTH_BROKER_CAPABILITIES_HEADER),
        Some(wire::AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES)
    );
    assert_eq!(requests[1].header("if-none-match"), Some("\"2\""));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[6].body).expect("upload body")["credential"]["refresh"],
        json!("fixture-real-refresh")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[7].body).expect("block body")["blockedUntilMs"],
        json!(4000.5)
    );
    assert_eq!(serde_json::from_slice::<Value>(&requests[11].body).expect("observed body"), observed());
}

#[tokio::test]
async fn get_retry_http_body_and_cancel_boundaries() {
    let fixture = Fixture::new(vec![
        Reply::disconnected(),
        Reply::json(200, json!({"ok":true})),
        Reply::bytes(401, "text/plain", b"forbidden"),
        Reply::bytes(200, "application/json", b"broken-json"),
        Reply::json(200, json!({"ok":true,"secret":"must-not-leak"})),
        Reply::bytes(200, "application/json", b"\xef\xbb\xbf{\"ok\":true,\"version\":\"\xff\"}"),
    ])
    .await;
    let client = fixture.client(3, Duration::from_secs(2));
    let cancel = CancellationToken::new();
    assert_eq!(client.healthz(&cancel).await.expect("GET retry")["ok"], json!(true));
    assert_eq!(fixture.requests().len(), 2);
    let http = client.healthz(&cancel).await.expect_err("401");
    assert_eq!(http.status, Some(401));
    assert!(!http.is_outcome_unknown());
    assert_eq!(client.healthz(&cancel).await.expect_err("JSON").kind, BrokerErrorKind::InvalidJson);
    let schema = client.healthz(&cancel).await.expect_err("schema");
    assert_eq!(schema.kind, BrokerErrorKind::InvalidSchema);
    assert!(!format!("{schema:?} {schema}").contains("must-not-leak"));
    assert_eq!(client.healthz(&cancel).await.expect("native text decoding")["version"], json!("\u{fffd}"));
    assert_eq!(fixture.requests().len(), 6);
    cancel.cancel();
    assert!(client.healthz(&cancel).await.expect_err("pre-cancel").is_cancelled());
    assert_eq!(fixture.requests().len(), 6);
}

#[tokio::test]
async fn mutation_unknown_receipts_are_never_automatically_replayed() {
    let fixture = Fixture::new(vec![
        Reply::disconnected(),
        Reply::bytes(200, "application/json", b"broken-json"),
        Reply::json(200, json!({"ok":"not boolean"})),
        Reply::json(500, json!({"error":"prefix already persisted"})),
        Reply::json(429, json!({"error":"rate limit"})),
        Reply::json(501, json!({"error":"broker store does not persist client usage"})),
    ])
    .await;
    let client = fixture.client(3, Duration::from_secs(2));
    let cancel = CancellationToken::new();
    let lost = client.disable_credential(7, "fixture", &cancel).await.expect_err("lost ACK");
    assert!(lost.is_outcome_unknown());
    let json = client.delete_credential_blocks(7, &cancel).await.expect_err("invalid JSON");
    assert_eq!(json.kind, BrokerErrorKind::InvalidJson);
    assert!(json.is_outcome_unknown());
    let schema = client.disable_credential(7, "fixture", &cancel).await.expect_err("invalid schema");
    assert_eq!(schema.kind, BrokerErrorKind::InvalidSchema);
    assert!(schema.is_outcome_unknown());
    let partial = client.report_client_usage(&observed(), &cancel).await.expect_err("partial observed");
    assert_eq!(partial.status, Some(500));
    assert!(partial.is_outcome_unknown());
    let rejected = client.report_client_usage(&observed(), &cancel).await.expect_err("known rejection");
    assert_eq!(rejected.status, Some(429));
    assert!(!rejected.is_outcome_unknown());
    assert_eq!(fixture.requests().len(), 5);
    let unsupported = client.report_client_usage(&observed(), &cancel).await.expect_err("observed unsupported");
    assert_eq!(unsupported.status, Some(501));
    assert!(!unsupported.is_outcome_unknown());
    assert_eq!(fixture.requests().len(), 6);
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let error = client.report_client_usage(&observed(), &cancelled).await.expect_err("pre-cancelled");
    assert!(error.is_cancelled());
    assert!(!error.is_outcome_unknown());
    assert_eq!(fixture.requests().len(), 6);
}

#[tokio::test]
async fn redirected_connect_failure_still_has_an_unknown_mutation_owner() {
    let unreachable = TcpListener::bind("127.0.0.1:0").await.expect("unused listener");
    let destination = format!("http://{}/after-dispatch", unreachable.local_addr().expect("address"));
    drop(unreachable);
    let fixture = Fixture::new(vec![Reply::header(307, "Location", &destination, Value::Null)]).await;
    let client = fixture.client(3, Duration::from_secs(2));
    let error =
        client.disable_credential(7, "fixture", &CancellationToken::new()).await.expect_err("redirect connect failure");
    assert!(error.is_outcome_unknown());
    assert_eq!(error.kind, BrokerErrorKind::OutcomeUnknown);
    assert_eq!(fixture.requests().len(), 1);
}

#[tokio::test]
async fn caller_cancellation_after_dispatch_and_usage_timeout_scaling() {
    let mut delayed = Reply::json(200, json!({"ok":true}));
    delayed.delay = Duration::from_secs(10);
    let mut usage = Reply::json(200, json!({"generatedAt":2000,"reports":[]}));
    usage.delay = Duration::from_millis(300);
    let fixture = Fixture::new(vec![delayed, usage]).await;
    let client = fixture.client(1, Duration::from_millis(250));
    let cancel = CancellationToken::new();
    let pending_client = client.clone();
    let pending_cancel = cancel.clone();
    let pending = tokio::spawn(async move { pending_client.disable_credential(7, "fixture", &pending_cancel).await });
    fixture.wait_for_requests(1).await;
    cancel.cancel();
    let error = pending.await.expect("pending owner").expect_err("cancel");
    assert!(error.is_cancelled());
    assert!(error.is_outcome_unknown());
    client
        .fetch_usage(Some(3.0), &CancellationToken::new())
        .await
        .expect("native usage timeout exceeds base allowance");
    assert_eq!(fixture.requests().len(), 2);
}

fn sse_body() -> String {
    let mut full = snapshot();
    full["kind"] = json!("snapshot");
    let mut scheduled = entry();
    scheduled["rotatesInMs"] = Value::Null;
    let update =
        json!({"kind":"entry","generation":3,"serverNowMs":2500,"refresher":full["refresher"],"entry":scheduled});
    let removed = json!({"kind":"removed","generation":4,"serverNowMs":3000,"refresher":full["refresher"],"id":7});
    format!(
        ": keepalive\r\n\r\nid: cursor\nretry: 25\n\ndata: {full}\r\n\r\nevent: entry\ndata: {update}\n\ndata: {removed}"
    )
}

#[tokio::test]
async fn sse_initial_snapshot_deltas_eof_and_cancellation_trace() {
    let mut delayed_stream = Reply::bytes(200, "Text/Event-Stream; charset=utf-8", sse_body().as_bytes());
    delayed_stream.delay = Duration::from_millis(60);
    let fixture = Fixture::new(vec![
        delayed_stream,
        Reply::bytes(200, "text/event-stream", b"event: only\n\n"),
        Reply::bytes(200, "text/event-stream", b": keepalive\n\n"),
    ])
    .await;
    let client = fixture.client(3, Duration::from_millis(20));
    let cancel = CancellationToken::new();
    let mut stream = client.open_snapshot_stream(&cancel).await.expect("SSE");
    assert!(matches!(stream.next_event().await.expect("snapshot"), Some(BrokerStreamEvent::Snapshot(_))));
    assert!(matches!(stream.next_event().await.expect("entry"), Some(BrokerStreamEvent::Entry { generation: 3, .. })));
    assert!(matches!(
        stream.next_event().await.expect("tail removed"),
        Some(BrokerStreamEvent::Removed { id: 7, generation: 4, .. })
    ));
    assert_eq!(stream.next_event().await.err().expect("unexpected EOF").kind, BrokerErrorKind::StreamEnded);
    let mut event_only = client.open_snapshot_stream(&cancel).await.expect("event-only SSE");
    assert_eq!(event_only.next_event().await.err().expect("empty JSON").kind, BrokerErrorKind::InvalidJson);
    let mut empty = client.open_snapshot_stream(&cancel).await.expect("comment SSE");
    assert_eq!(empty.next_event().await.err().expect("no initial snapshot").kind, BrokerErrorKind::StreamEnded);
    cancel.cancel();
    assert!(stream.next_event().await.expect("cancelled stream").is_none());
    let receipts = fixture.requests();
    assert_eq!(receipts.len(), 3);
    assert_eq!(receipts[0].header("accept"), Some("text/event-stream"));
    assert_eq!(
        receipts[0].header(wire::AUTH_BROKER_CAPABILITIES_HEADER),
        Some(wire::AUTH_BROKER_CAPABILITY_CODEX_METER_BLOCK_SCOPES)
    );
}

#[tokio::test]
async fn sse_404_content_type_and_first_event_failures_do_not_retry() {
    let mut full = snapshot();
    full["kind"] = json!("snapshot");
    let removed = json!({"kind":"removed","generation":4,"serverNowMs":3000,"refresher":full["refresher"],"id":7});
    let fixture = Fixture::new(vec![
        Reply::json(404, json!({"error":"unsupported"})),
        Reply::json(200, json!({})),
        Reply::bytes(200, "text/event-stream", format!("data: {removed}\n\n").as_bytes()),
        Reply::bytes(200, "text/event-stream", b"data: {broken\n\n"),
    ])
    .await;
    let client = fixture.client(3, Duration::from_secs(2));
    let cancel = CancellationToken::new();
    let error = client.open_snapshot_stream(&cancel).await.err().expect("unsupported");
    assert!(error.is_stream_unsupported());
    assert_eq!(error.status, Some(404));
    assert_eq!(client.open_snapshot_stream(&cancel).await.err().expect("non-SSE").kind, BrokerErrorKind::InvalidSchema);
    let mut first_delta = client.open_snapshot_stream(&cancel).await.expect("delta stream");
    assert_eq!(first_delta.next_event().await.err().expect("first delta").kind, BrokerErrorKind::InvalidSchema);
    let mut malformed = client.open_snapshot_stream(&cancel).await.expect("malformed stream");
    assert_eq!(malformed.next_event().await.err().expect("malformed").kind, BrokerErrorKind::InvalidJson);
    assert_eq!(fixture.requests().len(), 4);
}

#[test]
fn native_lf_cr_bom_utf8_batch_and_generation_tag_cases() {
    let mut decoder = BrokerSseDecoder::default();
    assert!(decoder.feed(b"event: snapshot\r\nda").is_empty());
    assert!(decoder.feed(b"ta: \xe4\xbd").is_empty());
    let decoded = decoder.feed(b"\xa0\xe5\xa5\xbd\r\n\r\n");
    assert_eq!(decoded.len(), 1);
    assert_eq!(decoded[0].event.as_deref(), Some("snapshot"));
    assert_eq!(decoded[0].data, "你好");
    let bare_cr = decoder.feed(b"data: one\rdata: two\n\n");
    assert_eq!(bare_cr[0].data, "one\rdata: two");
    let bom = decoder.feed("\u{feff}data: first\n\n".as_bytes());
    assert_eq!(bom[0].data, "first");
    let second_bom = decoder.feed("\u{feff}data: second\n\n".as_bytes());
    assert_eq!(second_bom[0].data, "second");
    let control = decoder.feed(b"id: cursor\nretry: 25\n\n");
    assert_eq!(control.len(), 1);
    assert!(control[0].event.is_none());
    assert!(control[0].data.is_empty());
    assert!(decoder.feed(b"id: bad\0id\nretry: -2\n\n").is_empty());
    let multi = decoder.feed(b"data: first\ndata:  second\nevent:\n\n");
    assert_eq!(multi[0].data, "first\n second");
    assert_eq!(multi[0].event.as_deref(), Some(""));
    assert!(decoder.feed(b"data: tail").is_empty());
    assert_eq!(decoder.finish().expect("native trailing line").data, "tail");
    for (input, expected) in [
        ("", None),
        (" ", Some(0)),
        ("W/\"7\"", Some(7)),
        ("0x10", Some(16)),
        ("0b11", Some(3)),
        ("0o10", Some(8)),
        ("1e2", Some(100)),
        ("1.5", None),
        ("-1", None),
        ("NaN", None),
        ("\u{feff}3\u{feff}", Some(3)),
    ] {
        assert_eq!(parse_generation_tag(input), expected, "{input}");
    }
}
