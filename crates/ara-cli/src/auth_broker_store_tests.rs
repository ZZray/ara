//! Grouped behavioral cases from fixed OMP remote-store.ts; actual loopback
//! HTTP/SSE exercises transport and owner lifetimes without a real account.
use super::*;
use crate::auth_broker_client::AuthBrokerClientOptions;
use crate::auth_broker_wire::{parse_snapshot, parse_stream_event};
use crate::auth_storage_policy::UsageLimit;
use serde_json::json;
use std::sync::atomic::AtomicI64;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;

const NOW: f64 = 1_000_000.0;

#[derive(Clone)]
struct Request {
    method: String,
    path: String,
    body: Value,
}
enum Reply {
    Json { status: u16, body: Value, gate: Option<Arc<Semaphore>> },
    Stream { first: String, later: String, gate: Arc<Semaphore> },
    Disconnect,
}
impl Reply {
    fn json(status: u16, body: Value) -> Self {
        Self::Json { status, body, gate: None }
    }
    fn gated(status: u16, body: Value, gate: Arc<Semaphore>) -> Self {
        Self::Json { status, body, gate: Some(gate) }
    }
}

struct LoopbackBroker {
    url: String,
    requests: Arc<Mutex<Vec<Request>>>,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}
impl LoopbackBroker {
    async fn start(handler: impl Fn(&Request) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let (seen, stop) = (requests.clone(), cancel.clone());
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let socket =
                    tokio::select! { biased; _ = stop.cancelled() => break, socket = listener.accept() => socket };
                let Ok((mut socket, _)) = socket else {
                    break;
                };
                let (seen, stop, handler) = (seen.clone(), stop.clone(), handler.clone());
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let mut buffer = [0u8; 2_048];
                    let (header_end, length) = loop {
                        let read = tokio::select! { biased; _ = stop.cancelled() => return, read = socket.read(&mut buffer) => read };
                        let Ok(count) = read else {
                            return;
                        };
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]);
                            let length = headers
                                .lines()
                                .filter_map(|line| line.split_once(':'))
                                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .map(|(_, value)| value.trim().parse::<usize>().unwrap())
                                .unwrap_or(0);
                            break (end + 4, length);
                        }
                        assert!(bytes.len() < 65_536);
                    };
                    while bytes.len() < header_end + length {
                        let read = tokio::select! { biased; _ = stop.cancelled() => return, read = socket.read(&mut buffer) => read };
                        let Ok(count) = read else {
                            return;
                        };
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    let first = String::from_utf8_lossy(&bytes[..header_end]);
                    let first = first.lines().next().unwrap();
                    let mut words = first.split_whitespace();
                    let request = Request {
                        method: words.next().unwrap().into(),
                        path: words.next().unwrap().into(),
                        body: if length == 0 {
                            Value::Null
                        } else {
                            serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap()
                        },
                    };
                    seen.lock().unwrap().push(request.clone());
                    match handler(&request) {
                        Reply::Disconnect => {}
                        Reply::Json { status, body, gate } => {
                            if let Some(gate) = gate {
                                tokio::select! { biased; _ = stop.cancelled() => return, permit = gate.acquire() => { permit.unwrap().forget(); } }
                            }
                            let body = body.to_string();
                            let wire = format!(
                                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = socket.write_all(wire.as_bytes()).await;
                        }
                        Reply::Stream { first, later, gate } => {
                            let head =
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                            if socket.write_all(head.as_bytes()).await.is_err()
                                || socket.write_all(first.as_bytes()).await.is_err()
                            {
                                return;
                            }
                            tokio::select! { biased; _ = stop.cancelled() => return, permit = gate.acquire() => { permit.unwrap().forget(); } }
                            if socket.write_all(later.as_bytes()).await.is_err() {
                                return;
                            }
                            stop.cancelled().await;
                        }
                    }
                });
            }
        });
        Self { url, requests, cancel, task }
    }
    fn client(&self) -> AuthBrokerClient {
        AuthBrokerClient::new(
            &self.url,
            "fixture-broker-bearer",
            AuthBrokerClientOptions {
                request_timeout: Duration::from_millis(500),
                max_retries: 0,
                ..AuthBrokerClientOptions::default()
            },
        )
        .unwrap()
    }
    fn count(&self, path: &str) -> usize {
        self.requests.lock().unwrap().iter().filter(|request| request.path.starts_with(path)).count()
    }
}
impl Drop for LoopbackBroker {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.task.abort();
    }
}

fn oauth(id: i64, account: &str, access: &str) -> Value {
    json!({"id":id,"provider":"openai-codex","credential":{"type":"oauth","access":access,"refresh":"__remote__","expires":9_000_000.0,"accountId":account,"email":format!("{account}@fixture.invalid")},"identityKey":format!("account:{account}"),"rotatesInMs":20_000.0,"blocks":[]})
}
fn api_key(id: i64, provider: &str) -> Value {
    json!({"id":id,"provider":provider,"credential":{"type":"api_key","key":format!("fixture-key-{id}")},"identityKey":null,"rotatesInMs":null,"blocks":[]})
}
fn snapshot(generation: i64, entries: Vec<Value>) -> Value {
    json!({"generation":generation,"generatedAt":NOW,"serverNowMs":NOW,"refresher":{"enabled":true,"intervalMs":60_000.0,"skewMs":300_000.0,"nextSweepInMs":10_000.0},"credentials":entries})
}
fn upload_entry(mut entry: Value) -> Value {
    let entry = entry.as_object_mut().unwrap();
    entry.remove("rotatesInMs");
    entry.remove("blocks");
    Value::Object(entry.clone())
}
fn event_frame(event: Value) -> String {
    format!("data: {event}\n\n")
}
fn stream_snapshot(mut snapshot: Value) -> Value {
    snapshot.as_object_mut().unwrap().insert("kind".into(), json!("snapshot"));
    snapshot
}
fn options(initial: Value) -> RemoteAuthCredentialStoreOptions {
    RemoteAuthCredentialStoreOptions {
        initial_snapshot: Some(parse_snapshot(&initial).unwrap()),
        background_idle: Duration::ZERO,
        clock: Arc::new(|| NOW),
        ..RemoteAuthCredentialStoreOptions::default()
    }
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(4), future).await.expect("bounded broker owner settlement")
}
async fn eventually(mut condition: impl FnMut() -> bool) {
    bounded(async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}
fn observed(at: i64, model: &str, requests: i64) -> ClientUsageEntry {
    ClientUsageEntry {
        at,
        provider: "openai-codex".into(),
        model: model.into(),
        requests,
        input_tokens: requests * 10,
        output_tokens: requests * 2,
        cache_read_tokens: requests * 3,
        cache_write_tokens: requests * 4,
        cost_usd: requests as f64 * 0.1,
    }
}
fn identity(app: &str) -> ClientUsageIdentity {
    ClientUsageIdentity {
        install_id: "fixture-install".into(),
        hostname: Some("fixture-host".into()),
        app: Some(app.into()),
    }
}

#[tokio::test]
async fn snapshot_pool_blocks_cache_and_projection_native_cases() {
    let broker = LoopbackBroker::start(|_| Reply::json(404, json!({}))).await;
    let mut visible = oauth(1, "visible", "first");
    visible["blocks"] = json!([
        {"providerKey":"openai-codex","blockScope":"spark","blockedUntilMs":NOW+600_000.0,"updatedAtMs":NOW-60_000.0},
        {"providerKey":"openai-codex","blockScope":"chat","blockedUntilMs":NOW-1.0}
    ]);
    let raw = snapshot(7, vec![visible.clone(), oauth(2, "hidden", "second"), api_key(3, "openai-codex")]);
    let calls = Arc::new(AtomicUsize::new(0));
    let callback_owner: Arc<Mutex<Weak<RemoteInner>>> = Arc::new(Mutex::new(Weak::new()));
    let (called, callback_weak) = (calls.clone(), callback_owner.clone());
    let mut opts = options(raw.clone());
    opts.account_pool = Some(BTreeMap::from([("openai-codex".into(), BTreeSet::from(["account:visible".into()]))]));
    opts.on_snapshot = Some(Arc::new(move |raw, _| {
        assert_eq!(raw.credentials.len(), 3);
        if let Some(inner) = callback_weak.lock().unwrap().upgrade() {
            assert!(inner.state.try_lock().is_ok(), "snapshot callback must run outside store lock");
        }
        called.fetch_add(1, Ordering::SeqCst);
    }));
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    *callback_owner.lock().unwrap() = Arc::downgrade(&store.inner);
    assert_eq!(calls.load(Ordering::SeqCst), 0, "initial snapshot is not callback-sourced");
    assert_eq!(store.rows(None).unwrap().iter().map(|row| row.id).collect::<Vec<_>>(), vec![1, 3]);
    assert_eq!(store.inner.state.lock().unwrap().raw_usage.len(), 3, "hidden IDs still size broker usage requests");
    assert_eq!(AuthCredentialStore::get_credential_block(&store, 1, "openai-codex", "chat").unwrap(), None);
    assert_eq!(
        AuthCredentialStore::get_credential_block_reconcile_after(&store, 1, "openai-codex", "spark").unwrap(),
        Some((NOW + 240_000.0) as i64)
    );
    AuthCredentialStore::delete_credential_block(&store, 1, "openai-codex", "spark").unwrap();
    assert!(
        AuthCredentialStore::get_credential_block(&store, 1, "openai-codex", "spark").unwrap().is_some(),
        "native scoped deletion is no-op"
    );
    AuthCredentialStore::set_cache(&store, "expired", "value", (NOW / 1_000.0) as i64).unwrap();
    assert!(AuthCredentialStore::get_cache(&store, "expired", true).unwrap().is_none());
    AuthCredentialStore::set_cache(&store, "界-prefix", "value", 2_000).unwrap();
    AuthCredentialStore::delete_cache_prefix(&store, "界-").unwrap();
    assert!(AuthCredentialStore::get_cache(&store, "界-prefix", false).unwrap().is_none());
    assert!(
        AuthCredentialStore::upsert_auth_credential_for_provider(&store, "p", &AuthCredential::api_key("key")).is_err()
    );
    assert!(AuthCredentialStore::replace_auth_credentials_for_provider(&store, "p", &[]).is_err());
    assert!(AuthCredentialStore::delete_auth_credentials_for_provider(&store, "p", "logout").is_err());
    // Native indexes previous block rows by ID, retaining the final duplicate.
    // Clearing that final blocked row must invalidate even if an earlier row
    // with the same ID was unblocked.
    let before = parse_snapshot(&snapshot(0, vec![oauth(1, "visible", "first"), visible.clone()])).unwrap();
    let after = parse_snapshot(&snapshot(0, vec![oauth(1, "visible", "first")])).unwrap();
    assert!(snapshot_blocks_changed(&before.credentials, &after.credentials));
    assert!(!snapshot_blocks_changed(&[], &after.credentials));
    let revision = store.projection_revision();
    let mut subscription = store.subscribe_projection_changes();
    let mut refreshed =
        crate::auth_broker_wire::parse_credential_entry(&upload_entry(oauth(1, "visible", "updated"))).unwrap();
    refreshed.rotates_in_ms = None;
    assert!(store.apply_credential_entry(refreshed));
    bounded(subscription.changed()).await.unwrap();
    assert!(store.projection_revision() > revision);
    assert_eq!(store.generation(), 7, "write acknowledgments do not invent a wire generation");
    store.apply_stream_event(parse_stream_event(&stream_snapshot(snapshot(6, vec![]))).unwrap());
    assert_eq!(store.generation(), 7, "old stream full snapshot ignored");
    store.apply_stream_event(parse_stream_event(&json!({"kind":"entry","generation":8,"serverNowMs":NOW+1.0,"refresher":raw["refresher"],"entry":oauth(2,"hidden","changed")})).unwrap());
    assert_eq!(store.rows(None).unwrap().len(), 2);
    assert_eq!(store.inner.state.lock().unwrap().raw_usage.len(), 3);
    store.apply_snapshot(parse_snapshot(&raw).unwrap(), 9, true);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.requests.lock().unwrap().len(), 0, "parked constructor and scoped delete issue no HTTP writes");
}

#[tokio::test]
async fn background_live_sse_deltas_and_open_settlement() {
    let gate = Arc::new(Semaphore::new(0));
    let initial = snapshot(1, vec![oauth(1, "account", "first")]);
    let next = json!({"kind":"entry","generation":2,"serverNowMs":NOW+10.0,"refresher":initial["refresher"],"entry":oauth(1,"account","second")});
    let stale = json!({"kind":"removed","generation":1,"serverNowMs":NOW,"refresher":initial["refresher"],"id":1});
    let (stream_gate, first, later) = (
        gate.clone(),
        event_frame(stream_snapshot(initial.clone())),
        format!("{}{}", event_frame(next), event_frame(stale)),
    );
    let broker = LoopbackBroker::start(move |request| {
        assert_eq!(request.path, "/v1/snapshot/stream");
        Reply::Stream { first: first.clone(), later: later.clone(), gate: stream_gate.clone() }
    })
    .await;
    let store = RemoteAuthCredentialStore::new(
        broker.client(),
        RemoteAuthCredentialStoreOptions {
            background_idle: Duration::from_secs(3),
            ..RemoteAuthCredentialStoreOptions::default()
        },
    )
    .unwrap();
    eventually(|| store.background_status().streaming_active && store.generation() == 1).await;
    bounded(store.wait_for_settlement()).await; // an open stream is not a finite owner
    gate.add_permits(1);
    eventually(|| store.generation() == 2).await;
    let rows = store.rows(None).unwrap();
    assert_eq!(rows.len(), 1, "stale removal cannot erase newer row");
    assert!(matches!(&rows[0].credential, AuthCredential::OAuth { fields } if fields["access"] == "second"));
    bounded(store.close_and_wait()).await;
    assert!(store.background_status().background_done);
    assert!(!store.background_status().streaming_active);
}

#[tokio::test]
async fn background_404_idle_resume_backoff_and_drop_cases() {
    let gate = Arc::new(Semaphore::new(0));
    let wait = gate.clone();
    let broker = LoopbackBroker::start(move |request| {
        if request.path == "/v1/snapshot/stream" {
            Reply::json(404, json!({}))
        } else {
            Reply::gated(304, Value::Null, wait.clone())
        }
    })
    .await;
    let store = RemoteAuthCredentialStore::new(
        broker.client(),
        RemoteAuthCredentialStoreOptions {
            background_idle: Duration::from_millis(150),
            ..RemoteAuthCredentialStoreOptions::default()
        },
    )
    .unwrap();
    eventually(|| store.background_status().parked && broker.count("/v1/snapshot?wait=") == 1).await;
    assert_eq!(broker.count("/v1/snapshot/stream"), 1);
    assert!(store.background_status().streaming_unsupported);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(broker.count("/v1/snapshot?wait="), 1, "idle owner holds no polling timer");
    let _ = store.snapshot();
    eventually(|| broker.count("/v1/snapshot?wait=") == 2).await;
    assert_eq!(broker.count("/v1/snapshot/stream"), 1, "404 latch survives resume");
    bounded(store.close_and_wait()).await;

    let failed = LoopbackBroker::start(|_| Reply::json(503, json!({}))).await;
    let store = RemoteAuthCredentialStore::new(failed.client(), RemoteAuthCredentialStoreOptions::default()).unwrap();
    eventually(|| failed.count("/v1/snapshot/stream") == 1).await;
    bounded(store.close_and_wait()).await; // closes before 500ms reconnect backoff
    let store = RemoteAuthCredentialStore::new(failed.client(), options(snapshot(0, vec![]))).unwrap();
    let weak = Arc::downgrade(&store.inner);
    let lifecycle = store.inner.lifecycle.clone();
    drop(store);
    eventually(|| lifecycle.background_done.load(Ordering::Acquire)).await;
    assert!(weak.upgrade().is_none(), "background future must not retain its owner");
}

#[tokio::test]
async fn readiness_refresh_sentinel_suspect_and_pool_cases() {
    let mut original = oauth(1, "account", "old");
    original["rotatesInMs"] = json!(500.0);
    original["blocks"] = json!([{"providerKey":"openai-codex","blockScope":"spark","blockedUntilMs":NOW+50_000.0}]);
    let updated = oauth(1, "account", "fresh");
    let fresh = snapshot(2, vec![updated.clone()]);
    let refresh_count = Arc::new(AtomicUsize::new(0));
    let count = refresh_count.clone();
    let broker = LoopbackBroker::start(move |request| {
        if request.path.starts_with("/v1/snapshot") {
            Reply::json(200, fresh.clone())
        } else if request.path == "/v1/credential/1/refresh" {
            let index = count.fetch_add(1, Ordering::SeqCst);
            Reply::json(
                200,
                json!({"entry":upload_entry(if index < 2 { updated.clone() } else { oauth(1, "outside", "other") })}),
            )
        } else {
            Reply::json(404, json!({}))
        }
    })
    .await;
    let mut opts = options(snapshot(1, vec![original]));
    opts.account_pool = Some(BTreeMap::from([("openai-codex".into(), BTreeSet::from(["account:account".into()]))]));
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    assert!(bounded(store.prepare_for_request(1, &CancellationToken::new())).await.unwrap());
    assert!(!bounded(store.prepare_for_request(1, &CancellationToken::new())).await.unwrap());
    let row = bounded(store.refresh_oauth_credential(1, &CancellationToken::new())).await.unwrap();
    assert!(
        matches!(row.credential, AuthCredential::OAuth { fields } if fields["refresh"] == "__remote__" && fields["access"] == "fresh")
    );
    bounded(store.mark_credential_suspect(1, &CancellationToken::new())).await.unwrap();
    bounded(store.wait_for_settlement()).await;
    assert_eq!(
        bounded(store.mark_credential_suspect(1, &CancellationToken::new())).await,
        Err(AuthStorageError::Unavailable)
    );
    assert!(store.rows(None).unwrap().is_empty(), "out-of-pool refresh removes visible row");
    assert_eq!(
        store.inner.state.lock().unwrap().raw_usage.len(),
        1,
        "hidden refreshed account still contributes usage count"
    );
    assert_eq!(refresh_count.load(Ordering::SeqCst), 3, "store never falls back to a local sentinel refresh");
    assert!(bounded(store.list_disabled_credentials(None, &CancellationToken::new())).await.unwrap().is_empty());
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn partial_replace_logout_and_owned_caller_drop_cases() {
    let release = Arc::new(Semaphore::new(0));
    let gate = release.clone();
    let uploads = Arc::new(AtomicUsize::new(0));
    let up = uploads.clone();
    let broker = LoopbackBroker::start(move |request| match request.path.as_str() {
        "/v1/credential/11/disable" => Reply::json(403, json!({})),
        "/v1/credential/12/disable" => Reply::json(200, json!({"ok":true})),
        "/v1/credential/13/disable" => Reply::json(403, json!({})),
        "/v1/credential" => {
            let index = up.fetch_add(1, Ordering::SeqCst);
            let body = json!({"entries":[upload_entry(api_key(if index == 0 {13} else {14},"p"))]});
            if index == 0 { Reply::json(200, body) } else { Reply::gated(200, body, gate.clone()) }
        }
        _ => Reply::json(200, snapshot(1, vec![api_key(14, "p")])),
    })
    .await;
    let store =
        RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![api_key(11, "p"), api_key(12, "p")])))
            .unwrap();
    store.inner.state.lock().unwrap().streaming_active = true; // suppress only the native write follow-up
    let rows = bounded(store.replace_auth_credentials_remote(
        "p",
        &[AuthCredential::api_key("new")],
        &CancellationToken::new(),
    ))
    .await
    .unwrap();
    assert_eq!(rows.iter().map(|row| row.id).collect::<Vec<_>>(), vec![13]);
    assert_eq!(broker.count("/v1/credential/11/disable"), 1);
    assert_eq!(broker.count("/v1/credential/12/disable"), 1);
    assert!(
        store
            .operation_receipts()
            .iter()
            .any(|receipt| receipt.operation == (RemoteOperation::Disable { credential_id: 11 })
                && receipt.outcome == RemoteOperationOutcome::Failed)
    );
    bounded(store.delete_auth_credentials_remote("p", "logout", &CancellationToken::new())).await.unwrap();
    assert!(store.rows(Some("p")).unwrap().is_empty(), "native logout removes rows despite known individual failure");
    let owner = store.clone();
    let waiting = tokio::spawn(async move {
        owner
            .upsert_auth_credential_remote("p", &AuthCredential::api_key("after-drop"), &CancellationToken::new())
            .await
    });
    eventually(|| uploads.load(Ordering::SeqCst) == 2).await;
    waiting.abort();
    let _ = waiting.await;
    release.add_permits(1);
    bounded(store.wait_for_settlement()).await;
    assert_eq!(store.rows(Some("p")).unwrap()[0].id, 14, "started write applies after its caller is dropped");
    assert_eq!(store.operation_receipts().last().unwrap().outcome, RemoteOperationOutcome::Succeeded);
    bounded(store.close_and_wait()).await;

    let uncertain = LoopbackBroker::start(|_| Reply::Disconnect).await;
    let store =
        RemoteAuthCredentialStore::new(uncertain.client(), options(snapshot(1, vec![api_key(11, "p")]))).unwrap();
    assert!(matches!(
        bounded(store.replace_auth_credentials_remote(
            "p",
            &[AuthCredential::api_key("new")],
            &CancellationToken::new()
        ))
        .await,
        Err(AuthStorageError::OutcomeUnknown)
    ));
    assert_eq!(store.rows(Some("p")).unwrap().len(), 1, "unknown disable retains the current projection");
    assert_eq!(uncertain.count("/v1/credential"), 1, "unknown mutation is not replayed and no upload begins");
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::OutcomeUnknown);
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn blocks_detached_writes_expiry_and_sync_disable_cases() {
    let posts = Arc::new(AtomicUsize::new(0));
    let posted = posts.clone();
    let broker = LoopbackBroker::start(move |request| {
        if request.path.ends_with("/block") || request.path.ends_with("/blocks") || request.path.ends_with("/disable") {
            posted.fetch_add(1, Ordering::SeqCst);
            Reply::json(200, json!({"ok":true}))
        } else {
            Reply::json(200, snapshot(1, vec![oauth(1, "account", "first")]))
        }
    })
    .await;
    let store =
        RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![oauth(1, "account", "first")])))
            .unwrap();
    store.inner.state.lock().unwrap().streaming_active = true;
    let block = StoredCredentialBlock {
        credential_id: 1,
        provider_key: "openai-codex".into(),
        block_scope: "spark".into(),
        blocked_until_ms: (NOW + 30_000.0) as i64,
        updated_at_ms: NOW as i64,
    };
    AuthCredentialStore::upsert_credential_block(&store, &block).unwrap();
    let mut earlier = block.clone();
    earlier.blocked_until_ms -= 10_000;
    AuthCredentialStore::upsert_credential_block(&store, &earlier).unwrap();
    assert_eq!(
        AuthCredentialStore::get_credential_block(&store, 1, "openai-codex", "spark").unwrap(),
        Some(block.blocked_until_ms)
    );
    AuthCredentialStore::delete_credential_block(&store, 1, "openai-codex", "spark").unwrap();
    AuthCredentialStore::delete_credential_blocks(&store, 1).unwrap();
    bounded(store.wait_for_settlement()).await;
    assert_eq!(posts.load(Ordering::SeqCst), 3, "scoped delete has no broader remote write");
    assert!(AuthCredentialStore::list_credential_blocks(&store, &[1]).unwrap().is_empty());
    assert!(
        AuthCredentialStore::try_disable_auth_credential_if_matches(&store, 1, "different bytes", "failure", None)
            .unwrap(),
        "native remote does not have local CAS semantics"
    );
    bounded(store.wait_for_settlement()).await;
    assert_eq!(posts.load(Ordering::SeqCst), 4);
    assert!(store.rows(None).unwrap().is_empty());
    assert!(store.operation_receipts().iter().all(|receipt| receipt.outcome == RemoteOperationOutcome::Succeeded));
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn observed_block_ack_scope_false_unknown_and_owned_settlement_family() {
    for (name, status, body, disconnect, expected) in [
        ("applied", 200, json!({"ok":true}), false, true),
        ("not applied", 200, json!({"ok":false}), false, false),
        ("malformed acknowledgement", 200, json!({"ok":"true"}), false, false),
        ("denied", 403, json!({}), false, false),
        ("unknown POST", 200, Value::Null, true, false),
    ] {
        let broker = LoopbackBroker::start(move |request| {
            assert_eq!(request.method, "POST");
            assert_eq!(request.path, "/v1/credential/17/block");
            if disconnect { Reply::Disconnect } else { Reply::json(status, body.clone()) }
        })
        .await;
        let store =
            RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![oauth(17, "account", "first")])))
                .unwrap();
        store.inner.state.lock().unwrap().streaming_active = true;
        let block = StoredCredentialBlock {
            credential_id: 17,
            provider_key: "openai-codex:oauth".into(),
            block_scope: "spark".into(),
            blocked_until_ms: (NOW + 1_800_000.0) as i64,
            updated_at_ms: NOW as i64,
        };
        let receipt = AuthCredentialStore::upsert_credential_block_observed(&store, &block).unwrap();
        assert_eq!(bounded(receipt.confirmed()).await, expected, "{name}");
        bounded(store.wait_for_settlement()).await;
        assert_eq!(broker.count("/v1/credential/17/block"), 1, "{name}: no duplicate or unknown replay");
        let sent = broker.requests.lock().unwrap()[0].body.clone();
        assert_eq!(sent["providerKey"], "openai-codex:oauth", "{name}");
        assert_eq!(sent["blockScope"], "spark", "{name}");
        assert_eq!(sent["blockedUntilMs"].as_f64(), Some(NOW + 1_800_000.0), "{name}");
        if name == "not applied" {
            // Generic legacy diagnostics call Ok(false) Succeeded. That fact
            // must not substitute for this operation's actual applied ack.
            assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Succeeded);
        }
        bounded(store.close_and_wait()).await;
    }

    // Two writes to the same row must not borrow the other scope's result.
    let release = Arc::new(Semaphore::new(0));
    let gate = release.clone();
    let broker = LoopbackBroker::start(move |request| {
        if request.body["blockScope"] == "spark" {
            Reply::gated(200, json!({"ok":true}), gate.clone())
        } else {
            Reply::json(200, json!({"ok":false}))
        }
    })
    .await;
    let store =
        RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![oauth(17, "account", "first")])))
            .unwrap();
    store.inner.state.lock().unwrap().streaming_active = true;
    let mut block = StoredCredentialBlock {
        credential_id: 17,
        provider_key: "openai-codex:oauth".into(),
        block_scope: "spark".into(),
        blocked_until_ms: (NOW + 50_000.0) as i64,
        updated_at_ms: NOW as i64,
    };
    let first = AuthCredentialStore::upsert_credential_block_observed(&store, &block).unwrap();
    eventually(|| broker.count("/v1/credential/17/block") == 1).await;
    block.block_scope = "other".into();
    let second = AuthCredentialStore::upsert_credential_block_observed(&store, &block).unwrap();
    assert!(!bounded(second.confirmed()).await);
    let first = tokio::spawn(first.confirmed());
    assert!(!first.is_finished());
    release.add_permits(1);
    assert!(bounded(first).await.unwrap());
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.count("/v1/credential/17/block"), 2);

    // Dropping a caller's private completion never cancels an admitted POST.
    let release = Arc::new(Semaphore::new(0));
    let gate = release.clone();
    let broker = LoopbackBroker::start(move |_| Reply::gated(200, json!({"ok":true}), gate.clone())).await;
    let store =
        RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![oauth(17, "account", "first")])))
            .unwrap();
    store.inner.state.lock().unwrap().streaming_active = true;
    let receipt = AuthCredentialStore::upsert_credential_block_observed(&store, &block).unwrap();
    eventually(|| broker.count("/v1/credential/17/block") == 1).await;
    drop(receipt);
    let waiting = store.clone();
    let close = tokio::spawn(async move { waiting.close_and_wait().await });
    assert!(!close.is_finished());
    assert!(store.background_status().pending_operations > 0);
    release.add_permits(1);
    bounded(close).await.unwrap();
    assert_eq!(broker.count("/v1/credential/17/block"), 1);
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Succeeded);
}

#[tokio::test]
async fn observed_merge_known_retry_unknown_isolation_and_final_close_cases() {
    let response_index = Arc::new(AtomicUsize::new(0));
    let index = response_index.clone();
    let broker = LoopbackBroker::start(move |request| {
        assert_eq!(request.path, "/v1/usage/observed");
        match index.fetch_add(1, Ordering::SeqCst) {
            0 => Reply::json(429, json!({})),
            1 => Reply::json(200, json!({"ok":true})),
            2 => Reply::json(500, json!({})),
            _ => Reply::json(200, json!({"ok":true})),
        }
    })
    .await;
    let mut opts = options(snapshot(0, vec![]));
    opts.observed_usage_flush = Duration::from_secs(3_600);
    opts.default_client_identity = identity("default");
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    store.record_observed_usage(&[observed(20, "m", 1), observed(10, "m", 2)], None);
    assert!(bounded(store.flush_observed_usage(&CancellationToken::new())).await.is_err());
    assert_eq!(store.inner.state.lock().unwrap().observed_usage[0].entry.requests, 3, "known failure is rebuffered");
    bounded(store.flush_observed_usage(&CancellationToken::new())).await.unwrap();
    {
        let requests = broker.requests.lock().unwrap();
        assert_eq!(requests[1].body["entries"][0]["at"], 20);
        assert_eq!(requests[1].body["entries"][0]["requests"], 3);
        assert_eq!(requests[1].body["app"], "default");
    }
    store.record_observed_usage(&[observed(30, "unknown", 4)], Some(identity("gateway")));
    assert_eq!(
        bounded(store.flush_observed_usage(&CancellationToken::new())).await,
        Err(AuthStorageError::OutcomeUnknown)
    );
    assert!(store.inner.state.lock().unwrap().observed_usage.is_empty());
    assert_eq!(store.unknown_observed_batches().len(), 1);
    assert_eq!(store.unknown_observed_batches()[0].report.entries[0].requests, 4);
    assert_eq!(store.unknown_observed_batches()[0].status, Some(500));
    store.record_observed_usage(&[observed(40, "final", 1)], Some(identity("other-app")));
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.count("/v1/usage/observed"), 4, "final flush runs once and unknown batch is never replayed");
    assert_eq!(store.unknown_observed_batches().len(), 1, "close retains unknown receipts and payload");
    store.close();
    store.record_observed_usage(&[observed(50, "after-close", 1)], None);
    assert_eq!(broker.count("/v1/usage/observed"), 4);
    assert!(store.rows(None).is_err());
}

#[tokio::test]
async fn observed_unsupported_statuses_latch_and_close_does_not_requeue_known_failure() {
    for status in [400, 404, 501] {
        let broker = LoopbackBroker::start(move |_| Reply::json(status, json!({}))).await;
        let mut opts = options(snapshot(0, vec![]));
        opts.observed_usage_flush = Duration::from_secs(3_600);
        let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
        store.record_observed_usage(&[observed(10, "first", 1)], Some(identity("app")));
        let _ = bounded(store.flush_observed_usage(&CancellationToken::new())).await;
        store.record_observed_usage(&[observed(20, "second", 1)], Some(identity("app")));
        bounded(store.close_and_wait()).await;
        assert!(store.background_status().observed_usage_unsupported);
        assert_eq!(broker.count("/v1/usage/observed"), 1);
    }
    let broker = LoopbackBroker::start(|_| Reply::json(429, json!({}))).await;
    let mut opts = options(snapshot(0, vec![]));
    opts.observed_usage_flush = Duration::from_secs(3_600);
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    store.record_observed_usage(&[observed(10, "final-failure", 1)], Some(identity("app")));
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.count("/v1/usage/observed"), 1);
    assert!(store.inner.state.lock().unwrap().observed_usage.is_empty());
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Failed);
}

#[tokio::test]
async fn authoritative_usage_hooks_share_one_adapter_and_raw_identity_projection() {
    let report = UsageReport {
        provider: "openai-codex".into(),
        fetched_at: NOW,
        metadata: Some(serde_json::from_value(json!({"accountId":"visible"})).unwrap()),
        limits: vec![UsageLimit { id: "meter".into(), label: "Fixture".into(), ..UsageLimit::default() }],
        ..UsageReport::default()
    };
    let wire = json!({"generatedAt":NOW,"reports":[report]});
    let broker = LoopbackBroker::start(move |request| {
        assert_eq!(request.path, "/v1/usage");
        Reply::json(200, wire.clone())
    })
    .await;
    let mut opts = options(snapshot(1, vec![oauth(1, "visible", "one"), oauth(2, "hidden", "two")]));
    opts.account_pool = Some(BTreeMap::from([("openai-codex".into(), BTreeSet::from(["account:visible".into()]))]));
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    let request = UsageRequest {
        provider: "openai-codex".into(),
        credential: serde_json::from_value(
            json!({"type":"oauth","accessToken":"fixture-access","accountId":"visible"}),
        )
        .unwrap(),
        credential_id: Some(1),
        base_url: None,
        account_key: "visible".into(),
    };
    let cancel = CancellationToken::new();
    let (aggregate, per_account) = tokio::join!(store.fetch_usage_reports(), store.get_usage_report(request, &cancel));
    assert_eq!(aggregate.unwrap().unwrap().len(), 1);
    assert!(per_account.unwrap().is_some());
    assert_eq!(broker.count("/v1/usage"), 1);
    assert_eq!(store.inner.state.lock().unwrap().raw_usage.len(), 2);
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn cache_and_block_clock_expiry_revisions_are_observable() {
    let broker = LoopbackBroker::start(|_| Reply::json(404, json!({}))).await;
    let clock = Arc::new(AtomicI64::new(NOW as i64));
    let now = clock.clone();
    let mut entry = oauth(1, "a", "access");
    entry["blocks"] = json!([{"providerKey":"p","blockScope":"scope","blockedUntilMs":NOW+100.0}]);
    let mut opts = options(snapshot(1, vec![entry]));
    opts.clock = Arc::new(move || now.load(Ordering::SeqCst) as f64);
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    AuthCredentialStore::set_cache(&store, "live", "value", 1_001).unwrap();
    let revision = store.projection_revision();
    clock.store(NOW as i64 + 1_000, Ordering::SeqCst);
    assert!(AuthCredentialStore::get_cache(&store, "live", false).unwrap().is_none());
    assert!(AuthCredentialStore::list_credential_blocks(&store, &[1]).unwrap().is_empty());
    assert!(store.projection_revision() > revision);
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn observed_timer_and_cancelled_owned_wait_keep_started_effects_owned() {
    let observed_release = Arc::new(Semaphore::new(0));
    let write_release = Arc::new(Semaphore::new(0));
    let (observed_gate, write_gate) = (observed_release.clone(), write_release.clone());
    let broker = LoopbackBroker::start(move |request| match request.path.as_str() {
        "/v1/usage/observed" => Reply::gated(200, json!({"ok":true}), observed_gate.clone()),
        "/v1/credential" => Reply::gated(200, json!({"entries":[upload_entry(api_key(2,"p"))]}), write_gate.clone()),
        _ => Reply::json(404, json!({})),
    })
    .await;
    let mut opts = options(snapshot(0, vec![]));
    opts.observed_usage_flush = Duration::from_millis(25);
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    store.record_observed_usage(&[observed(10, "m", 1)], Some(identity("app")));
    store.record_observed_usage(&[observed(20, "m", 2)], Some(identity("app")));
    eventually(|| broker.count("/v1/usage/observed") == 1).await;
    store.close();
    observed_release.add_permits(1);
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.requests.lock().unwrap()[0].body["entries"][0]["requests"], 3);
    assert_eq!(
        broker.count("/v1/usage/observed"),
        1,
        "close does not abort or duplicate an already-started timer flush"
    );
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Succeeded);

    let store = RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![api_key(1, "p")]))).unwrap();
    store.inner.state.lock().unwrap().streaming_active = true;
    let cancel = CancellationToken::new();
    let (owner, wait_cancel) = (store.clone(), cancel.clone());
    let waiter = tokio::spawn(async move {
        owner.upsert_auth_credential_remote("p", &AuthCredential::api_key("new"), &wait_cancel).await
    });
    eventually(|| broker.count("/v1/credential") == 1).await;
    cancel.cancel();
    assert!(matches!(bounded(waiter).await.unwrap(), Err(AuthStorageError::Cancelled)));
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Pending);
    write_release.add_permits(1);
    bounded(store.wait_for_settlement()).await;
    assert_eq!(store.rows(Some("p")).unwrap()[0].id, 2);
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Succeeded);
    assert_eq!(broker.count("/v1/credential"), 1);
    bounded(store.close_and_wait()).await;
}

#[tokio::test]
async fn observed_host_identity_omits_absent_optional_wire_fields() {
    let broker = LoopbackBroker::start(|request| {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/v1/usage/observed");
        assert!(request.body.get("hostname").is_none());
        assert!(request.body.get("app").is_none());
        Reply::json(200, json!({"ok":true}))
    })
    .await;
    let mut opts = options(snapshot(0, vec![]));
    opts.default_client_identity =
        ClientUsageIdentity { install_id: "host-owned-install".into(), hostname: None, app: None };
    opts.observed_usage_flush = Duration::from_secs(3_600);
    let store = RemoteAuthCredentialStore::new(broker.client(), opts).unwrap();
    store.record_observed_usage(&[observed(10, "model", 1)], None);
    bounded(store.close_and_wait()).await;
    assert_eq!(broker.count("/v1/usage/observed"), 1);
    assert_eq!(store.operation_receipts()[0].outcome, RemoteOperationOutcome::Succeeded);
}

#[tokio::test]
async fn broker_refresh_errors_do_not_disable_authstorage_oauth_credentials() {
    use crate::auth_storage::{AuthRequestContext, AuthStorage, AuthStorageOptions};

    let mut wrong_provider = oauth(1, "account", "wrong-provider");
    wrong_provider["provider"] = json!("different-provider");
    let cases = [
        ("rate limited", 429, None),
        ("broker bearer unauthorized", 401, None),
        ("broker bearer forbidden", 403, None),
        ("schema-valid non-OAuth refresh", 200, Some(upload_entry(api_key(1, "openai-codex")))),
        ("refresh returned a different ID", 200, Some(upload_entry(oauth(2, "account", "wrong-id")))),
        ("refresh returned a different provider", 200, Some(upload_entry(wrong_provider))),
    ];
    for (label, status, entry) in cases {
        let initial = snapshot(1, vec![oauth(1, "account", "original-access")]);
        let snapshot_reply = initial.clone();
        let broker = LoopbackBroker::start(move |request| {
            if request.path == "/v1/credential/1/refresh" {
                Reply::json(status, entry.as_ref().map(|entry| json!({"entry":entry})).unwrap_or_else(|| json!({})))
            } else if request.path.starts_with("/v1/snapshot") {
                Reply::json(200, snapshot_reply.clone())
            } else {
                Reply::json(200, json!({"ok":true}))
            }
        })
        .await;
        let remote = Arc::new(RemoteAuthCredentialStore::new(broker.client(), options(initial)).unwrap());
        let storage = AuthStorage::for_remote(
            remote.clone(),
            AuthStorageOptions { clock: Arc::new(|| NOW), jitter: Arc::new(|| 0.0), ..AuthStorageOptions::default() },
        )
        .unwrap();
        let context = AuthRequestContext { force_refresh: true, ..AuthRequestContext::default() };
        let result = bounded(storage.resolve("openai-codex", &context, &CancellationToken::new())).await;
        if status == 429 {
            assert!(matches!(result, Ok(None)), "{label}: refresh remains a transient failed candidate");
        } else {
            assert!(
                matches!(result, Err(AuthStorageError::Configuration)),
                "{label}: error belongs to broker configuration or response identity"
            );
        }
        bounded(storage.wait_for_settlement()).await;
        assert_eq!(broker.count("/v1/credential/1/refresh"), 1, "{label}: no refresh replay");
        assert_eq!(broker.count("/v1/credential/1/disable"), 0, "{label}: no provider-grant rejection receipt exists");
        let rows = storage.stored_oauth_snapshot("openai-codex").unwrap();
        assert_eq!(rows.len(), 1, "{label}: the original credential remains enabled");
        assert_eq!(rows[0].id, 1);
        assert!(rows[0].disabled_cause.is_none());
        assert!(
            matches!(&rows[0].credential, AuthCredential::OAuth { fields } if fields["access"] == "original-access")
        );
        assert_eq!(
            remote.rows(None).unwrap().len(),
            1,
            "{label}: a response identity error does not inject another row"
        );
        bounded(storage.close_and_wait()).await;
    }
}

/// Fixed OMP auth-storage.ts:5395-5414 chooses the explicit callback before
/// store/local refresh, and :5603-5620 publishes its result through AuthStorage.
/// The callback itself is not responsible for writing the credential store.
#[tokio::test]
async fn explicit_refresh_override_local_and_remote_consumers_publish_and_settle() {
    use crate::auth_storage::{AuthRequestContext, AuthStorage, AuthStorageOptions, OAuthProvider, ResolvedOAuth};
    use crate::credential_store::SqliteCredentialStore;
    use crate::model_route::{CredentialIdentity, RequestAuthLease};

    struct ReturningOverride {
        calls: AtomicUsize,
        entered: Arc<Semaphore>,
        release: Option<Arc<Semaphore>>,
    }
    #[async_trait::async_trait]
    impl OAuthProvider for ReturningOverride {
        async fn resolve(
            &self,
            mut row: StoredAuthCredential,
            force_refresh: bool,
            cancel: &CancellationToken,
        ) -> Result<ResolvedOAuth, AuthStorageError> {
            assert!(force_refresh, "expired-row consumer asks the explicit callback to refresh");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            if let Some(release) = &self.release {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
                    permit = release.acquire() => { permit.unwrap().forget(); },
                }
            }
            let AuthCredential::OAuth { fields } = &mut row.credential else {
                return Err(AuthStorageError::Configuration);
            };
            fields.insert("access".into(), json!("override-fresh"));
            fields.insert("expires".into(), json!(NOW + 9_000_000.0));
            row.serialized_data =
                serialize_credential(&row.provider, &row.credential).map_err(|_| AuthStorageError::Storage)?.data;
            let lease = self.lease(&row)?;
            // This fixture has no SQLite/Remote handle and writes no store.
            Ok(ResolvedOAuth { row, lease })
        }
        fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
            let AuthCredential::OAuth { fields } = &row.credential else {
                return Err(AuthStorageError::Configuration);
            };
            let access = fields.get("access").and_then(Value::as_str).ok_or(AuthStorageError::Configuration)?;
            Ok(RequestAuthLease::new(
                CredentialIdentity::Stored { id: row.id, revision: row.revision },
                Some(access.to_owned()),
            ))
        }
    }

    for remote_mode in [false, true] {
        for cancel_wait in [false, true] {
            let broker = LoopbackBroker::start(|_| Reply::json(503, json!({}))).await;
            let mut expired = oauth(1, "override-account", "override-expired");
            expired["credential"]["expires"] = json!(NOW - 1.0);
            let release = cancel_wait.then(|| Arc::new(Semaphore::new(0)));
            let callback = Arc::new(ReturningOverride {
                calls: AtomicUsize::new(0),
                entered: Arc::new(Semaphore::new(0)),
                release: release.clone(),
            });
            let storage_options = AuthStorageOptions {
                oauth_refresh_override: Some(callback.clone()),
                clock: Arc::new(|| NOW),
                jitter: Arc::new(|| 0.0),
                ..AuthStorageOptions::default()
            };
            let (storage, expected_id, local, remote) = if remote_mode {
                let remote = Arc::new(
                    RemoteAuthCredentialStore::new(broker.client(), options(snapshot(1, vec![expired]))).unwrap(),
                );
                (AuthStorage::for_remote(remote.clone(), storage_options).unwrap(), 1, None, Some(remote))
            } else {
                expired["credential"]["refresh"] = json!("local-fixture-refresh");
                let credential: AuthCredential = serde_json::from_value(expired["credential"].clone()).unwrap();
                let local = Arc::new(Mutex::new(
                    SqliteCredentialStore::from_connection(
                        rusqlite::Connection::open_in_memory().unwrap(),
                        Duration::from_millis(200),
                    )
                    .unwrap(),
                ));
                let id = {
                    let store = local.lock().unwrap();
                    store.upsert_auth_credential_for_provider("openai-codex", &credential).unwrap()[0].id
                };
                (AuthStorage::new(local.clone(), None, storage_options).unwrap(), id, Some(local), None)
            };
            let context = AuthRequestContext {
                session_id: Some("override-consumer-session".into()),
                ..AuthRequestContext::default()
            };
            if cancel_wait {
                let cancel = CancellationToken::new();
                let (owner, wait_context, wait_cancel) = (storage.clone(), context.clone(), cancel.clone());
                let waiter =
                    tokio::spawn(async move { owner.resolve("openai-codex", &wait_context, &wait_cancel).await });
                bounded(callback.entered.acquire()).await.unwrap().forget();
                cancel.cancel();
                assert!(matches!(bounded(waiter).await.unwrap(), Err(AuthStorageError::Cancelled)));
                assert_eq!(callback.calls.load(Ordering::SeqCst), 1);
                release.as_ref().unwrap().add_permits(1);
                bounded(storage.wait_for_settlement()).await;
            } else {
                let first = bounded(storage.resolve("openai-codex", &context, &CancellationToken::new()))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(first.lease.api_key(), Some("override-fresh"));
                assert!(matches!(first.lease.identity(), CredentialIdentity::Stored { id, .. } if *id == expected_id));
            }
            // Both a completed and an abandoned wait must have published to
            // the same owner before the next ordinary, non-forced resolve.
            let rows = storage.stored_oauth_snapshot("openai-codex").unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, expected_id);
            assert!(
                matches!(&rows[0].credential, AuthCredential::OAuth { fields } if fields["access"] == "override-fresh")
            );
            let next =
                bounded(storage.resolve("openai-codex", &context, &CancellationToken::new())).await.unwrap().unwrap();
            assert_eq!(next.lease.api_key(), Some("override-fresh"));
            assert!(matches!(next.lease.identity(), CredentialIdentity::Stored { id, .. } if *id == expected_id));
            assert_eq!(callback.calls.load(Ordering::SeqCst), 1, "fresh stored row must bypass a second callback");
            if let Some(local) = &local {
                let store = local.lock().unwrap();
                let durable = store.list_auth_credentials(Some("openai-codex")).unwrap();
                assert_eq!(durable[0].id, expected_id);
                assert!(
                    matches!(&durable[0].credential, AuthCredential::OAuth { fields } if fields["access"] == "override-fresh")
                );
            }
            if let Some(remote) = &remote {
                assert_eq!(remote.rows(Some("openai-codex")).unwrap()[0].id, expected_id);
                assert_eq!(
                    broker.count("/v1/credential/1/refresh"),
                    0,
                    "explicit override has precedence over the broker refresh hook"
                );
                assert_eq!(
                    broker.count("/v1/credential"),
                    0,
                    "callback publication only mirrors the remote view; it never uploads"
                );
            }
            assert!(broker.requests.lock().unwrap().is_empty(), "this family performs no broker HTTP operation");
            bounded(storage.close_and_wait()).await;
        }
    }
}
