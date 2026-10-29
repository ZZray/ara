//! Grouped module/transport acceptance against real SQLite and controlled HTTP.
//! Fixtures do not establish real OpenAI or MCP authorization acceptance.
#![cfg(feature = "test-fixture")]
use ara_cli::{
    auth_broker_client::{AuthBrokerClient, AuthBrokerClientOptions},
    auth_broker_server::{AuthBrokerServerHandle, AuthBrokerServerOptions, parse_broker_bind, start_auth_broker},
    auth_broker_wire::{BrokerSnapshotResult, BrokerStreamEvent},
    auth_storage::{
        AggregateUsageSource, AuthRequestContext, AuthStorage, AuthStorageError, AuthStorageOptions,
        BrokerStorageFacade, OAuthProvider, ResolvedOAuth,
    },
    auth_storage_policy::UsageReport,
    credential_store::{
        AuthCredential, SqliteCredentialStore, StoredAuthCredential, StoredCredentialBlock, standard_oauth_projection,
    },
    model_route::{CredentialIdentity, RequestAuthLease},
    openai_codex_auth::OpenAiCodexAuth,
};
use ara_testkit::{FakeUpstream, Script};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    future::Future,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("bounded broker family")
}
fn make_storage(path: &Path, options: AuthStorageOptions) -> (Arc<Mutex<SqliteCredentialStore>>, Arc<AuthStorage>) {
    let store = Arc::new(Mutex::new(SqliteCredentialStore::open(path).unwrap()));
    let storage = Arc::new(AuthStorage::new(store.clone(), None, options).unwrap());
    (store, storage)
}
async fn server(storage: Arc<AuthStorage>) -> AuthBrokerServerHandle {
    let mut options = AuthBrokerServerOptions::new(storage, vec!["fixture-token".into()]);
    options.bind = "127.0.0.1:0".into();
    options.version = Some("fixed-fixture".into());
    options.disable_refresher = true;
    options.stream_keepalive = Duration::from_millis(25);
    options.external_change_poll = Duration::from_millis(10);
    options.managed_mcp_client = Some(client());
    start_auth_broker(options).await.unwrap()
}
fn broker(handle: &AuthBrokerServerHandle) -> AuthBrokerClient {
    AuthBrokerClient::new(
        handle.url(),
        "fixture-token",
        AuthBrokerClientOptions { client: client(), max_retries: 0, ..Default::default() },
    )
    .unwrap()
}
fn oauth(extra: Value) -> AuthCredential {
    let mut fields=json!({"access":"fixture-access","refresh":"fixture-refresh","expires":4102444800000_f64,"email":"fixture@example.invalid"}).as_object().unwrap().clone();
    fields.extend(extra.as_object().unwrap().clone());
    AuthCredential::oauth(fields)
}
async fn request(
    handle: &AuthBrokerServerHandle,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = client().request(method, format!("{}{path}", handle.url())).bearer_auth("fixture-token");
    if let Some(body) = body {
        request = request.json(&body);
    }
    let result = bounded(request.send()).await.unwrap();
    let status = result.status().as_u16();
    let bytes = result.bytes().await.unwrap();
    (status, if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() })
}
async fn fake_upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}
async fn wait_requests(fake: &FakeUpstream, count: usize) {
    bounded(async {
        while fake.requests.lock().await.len() < count {
            tokio::task::yield_now().await;
        }
    })
    .await
}
#[derive(Default)]
struct UsageSource {
    fail: AtomicBool,
}
#[async_trait]
impl AggregateUsageSource for UsageSource {
    async fn fetch_usage_reports(&self) -> Result<Option<Vec<UsageReport>>, AuthStorageError> {
        if self.fail.load(Ordering::Acquire) {
            return Err(AuthStorageError::Transient);
        }
        Ok(Some(vec![UsageReport {
            provider: "fixture".into(),
            fetched_at: 1000.0,
            raw: Some(json!({"privateUpstream":"fixture"})),
            ..Default::default()
        }]))
    }
}
#[tokio::test]
async fn broker_storage_wire_routes_and_usage_family() {
    for (raw, host, port) in
        [("4000", "127.0.0.1", 4000), (" [::1]:0 ", "[::1]", 0), ("0.0.0.0:65535", "0.0.0.0", 65535)]
    {
        assert_eq!(parse_broker_bind(raw).unwrap(), (host.into(), port));
    }
    for raw in ["", ":4000", "127.0.0.1:-1", "127.0.0.1:1.5", "65536", "127.0.0.1:0x20"] {
        assert!(parse_broker_bind(raw).is_err());
    }
    let temp = tempfile::tempdir().unwrap();
    let source = Arc::new(UsageSource::default());
    let (store, storage) = make_storage(
        &temp.path().join("auth.db"),
        AuthStorageOptions { aggregate_usage_override: Some(source.clone()), ..Default::default() },
    );
    let handle = server(storage.clone()).await;
    assert_eq!(bounded(client().get(format!("{}/v1/healthz", handle.url())).send()).await.unwrap().status(), 200);
    assert_eq!(bounded(client().get(format!("{}/v1/snapshot", handle.url())).send()).await.unwrap().status(), 401);
    let (status,upload)=request(&handle,reqwest::Method::POST,"/v1/credential",Some(json!({"provider":"openai-codex","credential":oauth(json!({"accountId":"fixture-account","orgId":"fixture-org","custom":"extension"}))}))).await;
    assert_eq!(status, 200);
    let id = upload["entries"][0]["id"].as_i64().unwrap();
    assert_eq!(upload["entries"][0]["credential"]["refresh"], "__remote__");
    assert_eq!(upload["entries"][0]["credential"]["custom"], "extension");
    let b = broker(&handle);
    let snapshot = b.fetch_snapshot(None, None, &CancellationToken::new()).await.unwrap();
    let generation = match snapshot {
        BrokerSnapshotResult::Snapshot { snapshot, .. } => {
            assert_eq!(snapshot.credentials.len(), 1);
            snapshot.generation
        }
        _ => panic!("snapshot"),
    };
    let same = b.fetch_snapshot(Some(generation), Some(0), &CancellationToken::new()).await.unwrap();
    assert!(matches!(same, BrokerSnapshotResult::Snapshot { .. }));
    let same = b.fetch_snapshot(Some(generation), Some(15), &CancellationToken::new()).await.unwrap();
    assert!(matches!(same, BrokerSnapshotResult::NotModified { .. }));
    for (scope, delta) in [("chat", 20_000), ("spark", 30_000)] {
        let until = chrono::Utc::now().timestamp_millis() + delta;
        assert_eq!(
            request(
                &handle,
                reqwest::Method::POST,
                &format!("/v1/credential/{id}/block"),
                Some(json!({"providerKey":"openai-codex:oauth","blockScope":scope,"blockedUntilMs":until}))
            )
            .await
            .0,
            200
        );
    }
    let legacy = client()
        .get(format!("{}/v1/snapshot", handle.url()))
        .bearer_auth("fixture-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let blocks = legacy["credentials"][0]["blocks"].as_array().unwrap();
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0]["blockScope"], "shared");
    let capable = client()
        .get(format!("{}/v1/snapshot", handle.url()))
        .bearer_auth("fixture-token")
        .header("OMP-Auth-Broker-Capabilities", "unknown, codex-meter-block-scopes")
        .send()
        .await
        .unwrap();
    assert_eq!(capable.headers()["Vary"], "OMP-Auth-Broker-Capabilities");
    assert_eq!(capable.headers()["Cache-Control"], "no-store");
    let capable = capable.json::<Value>().await.unwrap();
    assert_eq!(capable["credentials"][0]["blocks"].as_array().unwrap().len(), 2);
    let report = request(&handle, reqwest::Method::GET, "/v1/usage", None).await;
    assert_eq!(report.0, 200);
    assert!(report.1["reports"][0].get("raw").is_none());
    source.fail.store(true, Ordering::Release);
    assert_eq!(request(&handle, reqwest::Method::GET, "/v1/usage", None).await.0, 502);
    source.fail.store(false, Ordering::Release);
    let now = chrono::Utc::now().timestamp_millis();
    let observed = json!({"installId":"fixture-install","hostname":"fixture-host","app":"ara","entries":[{"at":now,"provider":"fixture","model":"fixture-model","requests":2,"inputTokens":7,"outputTokens":3,"cacheReadTokens":1,"cacheWriteTokens":0,"costUsd":0.01}]});
    assert_eq!(request(&handle, reqwest::Method::POST, "/v1/usage/observed", Some(observed)).await.0, 200);
    let usage = request(&handle, reqwest::Method::GET, "/v1/usage/clients?sinceMs=0suffix", None).await.1;
    assert_eq!(usage["clients"][0]["providers"][0]["requests"], 2);
    assert_eq!(store.lock().unwrap().get_client_usage_summary(0).unwrap().clients.len(), 1);
    assert_eq!(request(&handle, reqwest::Method::GET, "/v1/usage/history?sinceMs=0suffix", None).await.0, 200);
    assert_eq!(request(&handle, reqwest::Method::POST, "/v1/usage/stale", None).await.0, 200);
    assert_eq!(
        request(&handle, reqwest::Method::POST, "/v1/credential", Some(json!({"provider":"","credential":{}}))).await.0,
        400
    );
    assert_eq!(
        request(
            &handle,
            reqwest::Method::POST,
            &format!("/v1/credential/{id}/block"),
            Some(json!({"providerKey":"fixture","blockScope":"chat","blockedUntilMs":"bad"}))
        )
        .await
        .0,
        400
    );
    assert_eq!(request(&handle, reqwest::Method::DELETE, &format!("/v1/credential/{id}/blocks"), None).await.0, 200);
    assert_eq!(request(&handle, reqwest::Method::POST, &format!("/v1/credential/{id}/disable"), None).await.0, 200);
    let disabled =
        request(&handle, reqwest::Method::GET, "/v1/credentials/disabled?provider=openai-codex", None).await.1;
    assert_eq!(disabled["disabled"][0]["cause"], "disabled via auth-broker");
    assert_eq!(request(&handle, reqwest::Method::POST, &format!("/v1/credential/{id}/disable"), None).await.0, 404);
    assert_eq!(request(&handle, reqwest::Method::POST, "/v1/credential/999999/refresh", None).await.0, 404);
    assert_eq!(request(&handle, reqwest::Method::GET, "/missing", None).await.0, 404);
    handle.close().await.unwrap();
    storage.close_and_wait().await;
    let mut empty_options = AuthBrokerServerOptions::new(storage.clone(), Vec::new());
    empty_options.bind = "127.0.0.1:0".into();
    empty_options.disable_refresher = true;
    let empty = start_auth_broker(empty_options).await.unwrap();
    assert_eq!(client().get(format!("{}/v1/snapshot", empty.url())).send().await.unwrap().status(), 200);
    empty.close().await.unwrap();
}
#[tokio::test]
#[allow(
    clippy::await_holding_lock,
    reason = "The documented race fixture intentionally blocks the database while poll and the local generation notification run."
)]
async fn broker_stream_external_commit_longpoll_and_close_family() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.db");
    let race_hook = Arc::new(OverrideRefresh { calls: AtomicUsize::new(0), gate: Semaphore::new(0) });
    let (store, storage) = make_storage(
        &path,
        AuthStorageOptions { oauth_refresh_override: Some(race_hook.clone()), ..Default::default() },
    );
    let handle = server(storage.clone()).await;
    let b = broker(&handle);
    let cancel = CancellationToken::new();
    let mut stream = b.open_snapshot_stream(&cancel).await.unwrap();
    assert!(matches!(bounded(stream.next_event()).await.unwrap(), Some(BrokerStreamEvent::Snapshot(_))));
    let mut heartbeat =
        client().get(format!("{}/v1/snapshot/stream", handle.url())).bearer_auth("fixture-token").send().await.unwrap();
    let mut buffer = String::new();
    bounded(async {
        while !buffer.contains(": keepalive\n\n") {
            buffer.push_str(&String::from_utf8_lossy(&heartbeat.chunk().await.unwrap().unwrap()));
        }
    })
    .await;
    let peer = SqliteCredentialStore::open(&path).unwrap();
    let id =
        peer.upsert_auth_credential_for_provider("fixture-api", &AuthCredential::api_key("fixture-key")).unwrap()[0].id;
    let entry = bounded(stream.next_event()).await.unwrap();
    assert!(matches!(entry,Some(BrokerStreamEvent::Entry {entry,..}) if entry.id==id));
    let block_until = chrono::Utc::now().timestamp_millis() + 300_000;
    peer.upsert_credential_block(&StoredCredentialBlock {
        credential_id: id,
        provider_key: "fixture-api:oauth".into(),
        block_scope: "shared".into(),
        blocked_until_ms: block_until,
        updated_at_ms: block_until - 300_000,
    })
    .unwrap();
    bounded(async {
        loop {
            if let Some(BrokerStreamEvent::Entry { entry, .. }) = stream.next_event().await.unwrap()
                && entry.id == id
                && entry.blocks.iter().any(|block| block.blocked_until_ms == block_until as f64)
            {
                break;
            }
        }
    })
    .await;
    let snapshot = b.fetch_snapshot(None, None, &CancellationToken::new()).await.unwrap();
    let generation = match snapshot {
        BrokerSnapshotResult::Snapshot { generation, .. } => generation,
        _ => unreachable!(),
    };
    let waiter = CancellationToken::new();
    let bb = b.clone();
    let ww = waiter.clone();
    let wait = tokio::spawn(async move { bb.fetch_snapshot(Some(generation), Some(30_000), &ww).await });
    peer.update_auth_credential(id, &AuthCredential::api_key("fixture-key-new")).unwrap();
    assert!(
        matches!(bounded(wait).await.unwrap().unwrap(),BrokerSnapshotResult::Snapshot {generation:g,..} if g>generation)
    );
    peer.delete_auth_credential(id, "external fixture disable").unwrap();
    let removed = bounded(async {
        loop {
            if let Some(BrokerStreamEvent::Removed { id: removed, .. }) = stream.next_event().await.unwrap() {
                break removed;
            }
        }
    })
    .await;
    assert_eq!(removed, id);
    let snap = b.fetch_snapshot(None, None, &CancellationToken::new()).await.unwrap();
    let generation = match snap {
        BrokerSnapshotResult::Snapshot { generation, .. } => generation,
        _ => unreachable!(),
    };
    let bb = b.clone();
    let ww = waiter.clone();
    let wait = tokio::spawn(async move { bb.fetch_snapshot(Some(generation), Some(30_000), &ww).await });
    waiter.cancel();
    assert!(bounded(wait).await.unwrap().err().unwrap().is_cancelled());
    bounded(handle.close()).await.unwrap();
    drop(heartbeat);
    cancel.cancel();
    bounded(stream.next_event()).await.ok();
    assert!(client().get(format!("{}/v1/healthz", handle.url())).send().await.is_err());
    assert!(storage.has_auth("fixture-api").is_ok());
    // Force the old check -> database await window without a production hook.
    // The local refresh-start bump is consumed before the peer block commits;
    // it cannot stand in for that later external revision. The stopped transport
    // leaves one poll owner, so another poll cannot acknowledge the fixture first.
    let race_id =
        store.lock().unwrap().upsert_auth_credential_for_provider("race-oauth", &oauth(json!({}))).unwrap()[0].id;
    storage.reload().unwrap();
    let facade = BrokerStorageFacade::new(storage.clone());
    let mut notices = facade.subscribe_generation();
    let (poll, refresh, local_generation) = {
        let guard = store.lock().unwrap();
        let mut poll = Box::pin(facade.poll_external_changes());
        assert!(futures::poll!(&mut poll).is_pending());
        let refresh_facade = facade.clone();
        let refresh = tokio::spawn(async move { refresh_facade.force_refresh(race_id, &CancellationToken::new()).await });
        let local_generation = bounded(notices.changed(&CancellationToken::new())).await.unwrap();
        peer.upsert_credential_block(&StoredCredentialBlock {
            credential_id: race_id,
            provider_key: "race-oauth:oauth".into(),
            block_scope: "shared".into(),
            blocked_until_ms: block_until,
            updated_at_ms: block_until - 300_000,
        })
        .unwrap();
        drop(guard);
        (poll, refresh, local_generation)
    };
    assert!(bounded(poll).await.unwrap());
    let external_generation = bounded(notices.changed(&CancellationToken::new())).await.unwrap();
    assert!(external_generation > local_generation);
    assert!(
        facade
            .export_snapshot()
            .unwrap()
            .blocks
            .iter()
            .any(|block| block.credential_id == race_id && block.blocked_until_ms == block_until)
    );
    race_hook.gate.add_permits(1);
    assert!(bounded(refresh).await.unwrap().is_ok());
    storage.close_and_wait().await;
}
struct OverrideRefresh {
    calls: AtomicUsize,
    gate: Semaphore,
}
// Reference Host owner for the installed-provider contract. Core supplies the
// per-ID shared flight; this fixture owns HTTP and the original exact-row CAS.
struct PersistedRefresh {
    store: Arc<Mutex<SqliteCredentialStore>>,
    endpoint: String,
    calls: AtomicUsize,
    explicit: AtomicUsize,
    entered: Semaphore,
    gate: Semaphore,
}
impl PersistedRefresh {
    async fn owned(
        &self,
        observed: StoredAuthCredential,
        explicit: bool,
        cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        if explicit {
            self.explicit.fetch_add(1, Ordering::AcqRel);
        }
        self.entered.add_permits(1);
        tokio::select! {
            _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
            permit = self.gate.acquire() => permit.unwrap().forget(),
        }
        let AuthCredential::OAuth { fields: old } = &observed.credential else {
            return Err(AuthStorageError::Configuration);
        };
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AuthStorageError::Cancelled),
            response = client().post(&self.endpoint).json(&json!({"credentialId":observed.id,"refresh":old.get("refresh")})).send() =>
                response.map_err(|_| AuthStorageError::OutcomeUnknown)?,
        };
        if !response.status().is_success() {
            return Err(if response.status().as_u16() == 400 {
                AuthStorageError::Definitive
            } else {
                AuthStorageError::Transient
            });
        }
        let fresh: serde_json::Map<String, Value> =
            response.json().await.map_err(|_| AuthStorageError::OutcomeUnknown)?;
        let mut fields = old.clone();
        fields.extend(fresh);
        let usable = fields
            .get("expires")
            .and_then(Value::as_f64)
            .is_some_and(|at| at > chrono::Utc::now().timestamp_millis() as f64);
        let mut credential = AuthCredential::oauth(fields);
        if explicit && usable {
            credential =
                standard_oauth_projection(&credential, &observed.credential).ok_or(AuthStorageError::Configuration)?;
        }
        let store = self.store.lock().unwrap();
        let won = store
            .try_update_auth_credential_if_matches(observed.id, &observed.serialized_data, &credential, None)
            .map_err(|_| AuthStorageError::Storage)?;
        let row = store
            .list_auth_credentials(Some(&observed.provider))
            .map_err(|_| AuthStorageError::Storage)?
            .into_iter()
            .find(|row| row.id == observed.id)
            .ok_or(AuthStorageError::Unavailable)?;
        let own = won && row.credential == credential;
        let AuthCredential::OAuth { fields } = &row.credential else { return Err(AuthStorageError::Configuration) };
        let cutoff = chrono::Utc::now().timestamp_millis() as f64 + if own { 0.0 } else { 60_000.0 };
        if !fields.get("expires").and_then(Value::as_f64).is_some_and(|at| at > cutoff) {
            return Err(AuthStorageError::Unavailable);
        }
        Ok(ResolvedOAuth { lease: self.lease(&row)?, row })
    }
}
#[async_trait]
impl OAuthProvider for PersistedRefresh {
    fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
        let AuthCredential::OAuth { fields } = &row.credential else { return Err(AuthStorageError::Configuration) };
        Ok(RequestAuthLease::new(
            CredentialIdentity::Stored { id: row.id, revision: row.revision },
            fields.get("access").and_then(Value::as_str).map(str::to_owned),
        ))
    }
    async fn resolve(
        &self,
        row: StoredAuthCredential,
        _force: bool,
        cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        self.owned(row, false, cancel).await
    }
    async fn resolve_broker_explicit(
        &self,
        row: StoredAuthCredential,
        cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        self.owned(row, true, cancel).await
    }
}
struct NoExplicitRefresh {
    calls: AtomicUsize,
}
#[async_trait]
impl OAuthProvider for NoExplicitRefresh {
    fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
        Ok(RequestAuthLease::new(
            CredentialIdentity::Stored { id: row.id, revision: row.revision },
            Some("fresh-without-refresh".into()),
        ))
    }
    async fn resolve(
        &self,
        _row: StoredAuthCredential,
        _force: bool,
        _cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Err(AuthStorageError::Configuration)
    }
}
#[async_trait]
impl OAuthProvider for OverrideRefresh {
    fn lease(&self, row: &StoredAuthCredential) -> Result<RequestAuthLease, AuthStorageError> {
        Ok(RequestAuthLease::new(
            CredentialIdentity::Stored { id: row.id, revision: row.revision },
            Some("fixture-new".into()),
        ))
    }
    async fn resolve(
        &self,
        mut row: StoredAuthCredential,
        _force: bool,
        _cancel: &CancellationToken,
    ) -> Result<ResolvedOAuth, AuthStorageError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        self.gate.acquire().await.unwrap().forget();
        let AuthCredential::OAuth { fields } = &mut row.credential else { unreachable!() };
        fields.insert("access".into(), json!("fixture-new"));
        fields.insert("expires".into(), json!(4102444800000_f64));
        Ok(ResolvedOAuth { lease: self.lease(&row)?, row })
    }
}
#[tokio::test]
async fn broker_refresh_shared_owner_mcp_projection_and_fault_family() {
    let temp = tempfile::tempdir().unwrap();
    // Installed-provider owner contract: actual HTTP + SQLite, shared first
    // owner, detached settlement, original CAS and no facade post-projection.
    for case in [
        "explicit",
        "ordinary-force",
        "ordinary-first",
        "explicit-first",
        "peer",
        "peer-short",
        "own-short",
        "expired",
        "cancel",
        "transient",
        "definitive",
    ] {
        let now = chrono::Utc::now().timestamp_millis();
        let first = match case {
            "transient" => json!({"status":503,"body":"{}"}),
            "definitive" => json!({"status":400,"body":"{}"}),
            _ => {
                json!({"delay_ms":120,"body":json!({"access":"installed-new-access","refresh":"installed-new-refresh","expires":if case=="expired" {now-1} else if case=="own-short" {now+30_000} else {now+3_600_000},"projectId":if matches!(case, "ordinary-force" | "ordinary-first" | "expired") {json!("fallback-project")} else {Value::Null}}).to_string()})
            }
        };
        let mut script = vec![first];
        if case == "expired" {
            script.push(json!({"body":json!({"access":"installed-recovered-access","refresh":"installed-recovered-refresh","expires":now+3_600_000}).to_string()}));
        }
        let fake = fake_upstream(script).await;
        let (store, storage) = make_storage(&temp.path().join(format!("installed-{case}.db")), Default::default());
        let id=store.lock().unwrap().upsert_auth_credential_for_provider("fixture-custom",&oauth(json!({"accountId":"installed-account","projectId":"fallback-project","custom":"retained-extension"}))).unwrap()[0].id;
        let hook = Arc::new(PersistedRefresh {
            store: store.clone(),
            endpoint: format!("http://{}/custom-refresh", fake.addr),
            calls: AtomicUsize::new(0),
            explicit: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            gate: Semaphore::new(if matches!(case, "ordinary-first" | "explicit-first") { 0 } else { 1 }),
        });
        storage.register_oauth_provider("fixture-custom", hook.clone()).unwrap();
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let cancel = CancellationToken::new();
        let pending = if matches!(case, "ordinary-force" | "ordinary-first") {
            let s = storage.clone();
            tokio::spawn(async move {
                s.resolve(
                    "fixture-custom",
                    &AuthRequestContext { force_refresh: true, ..Default::default() },
                    &CancellationToken::new(),
                )
                .await
                .is_ok()
            })
        } else {
            let b = broker(&handle);
            let c = cancel.clone();
            tokio::spawn(async move { b.refresh_credential(id, &c).await.is_ok() })
        };
        bounded(hook.entered.acquire()).await.unwrap().forget();
        let joined = if case == "ordinary-first" {
            let b = broker(&handle);
            Some(tokio::spawn(async move { b.refresh_credential(id, &CancellationToken::new()).await.map(|_| ()).map_err(|e| format!("{e:?}")) }))
        } else if case == "explicit-first" {
            let s = storage.clone();
            Some(tokio::spawn(async move {
                s.resolve(
                    "fixture-custom",
                    &AuthRequestContext { force_refresh: true, ..Default::default() },
                    &CancellationToken::new(),
                )
                .await
                .map(|_| ()).map_err(|e| format!("{e:?}"))
            }))
        } else {
            None
        };
        if let Some(joined) = joined {
            assert!(
                tokio::time::timeout(Duration::from_millis(65), async {
                    while hook.calls.load(Ordering::Acquire) <= 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .is_err()
            );
            hook.gate.add_permits(1);
            bounded(joined).await.unwrap().unwrap_or_else(|error| panic!("{case} joined owner failed: {error}"));
        }
        if matches!(case, "peer" | "peer-short" | "cancel") {
            wait_requests(&fake, 1).await;
            if case == "cancel" {
                cancel.cancel();
            } else {
                store.lock().unwrap().update_auth_credential(id,&oauth(json!({"access":"installed-peer-access","refresh":"installed-peer-refresh","custom":"peer-extension","expires":if case=="peer-short" {now+30_000} else {now+3_600_000}}))).unwrap();
            }
        }
        assert_eq!(
            bounded(pending).await.unwrap(),
            !matches!(case, "peer-short" | "expired" | "cancel" | "transient" | "definitive"),
            "installed case {case}"
        );
        bounded(storage.wait_for_settlement()).await;
        let rows = store.lock().unwrap().list_auth_credentials(Some("fixture-custom")).unwrap();
        if case == "definitive" {
            assert!(rows.is_empty());
            assert_eq!(store.lock().unwrap().list_disabled_credentials(Some("fixture-custom")).unwrap().len(), 1);
        } else {
            let AuthCredential::OAuth { fields } = &rows[0].credential else { panic!("installed OAuth") };
            if matches!(case, "peer" | "peer-short") {
                assert_eq!(fields["access"], "installed-peer-access");
                assert_eq!(fields["refresh"], "installed-peer-refresh");
                assert_eq!(fields["custom"], "peer-extension");
            } else if matches!(case, "ordinary-force" | "ordinary-first" | "expired" | "transient") {
                assert_eq!(fields["custom"], "retained-extension");
                if case == "expired" {
                    assert_eq!(fields["refresh"], "installed-new-refresh");
                    hook.gate.add_permits(1);
                    bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.unwrap();
                    let row = store.lock().unwrap().list_auth_credentials(Some("fixture-custom")).unwrap().remove(0);
                    let AuthCredential::OAuth { fields } = row.credential else { panic!("recovered installed OAuth") };
                    assert!(!fields.contains_key("custom"));
                    assert_eq!(
                        fake.requests.lock().await[1]["body"]["refresh"],
                        "installed-new-refresh"
                    );
                }
            } else {
                assert!(!fields.contains_key("custom"));
                assert_eq!(fields["projectId"], "fallback-project");
                assert_eq!(fields["access"], "installed-new-access");
            }
        }
        assert_eq!(hook.calls.load(Ordering::Acquire), if case == "expired" { 2 } else { 1 });
        assert_eq!(
            hook.explicit.load(Ordering::Acquire),
            if matches!(case, "ordinary-force" | "ordinary-first") {
                0
            } else if case == "expired" {
                2
            } else {
                1
            }
        );
        assert_eq!(fake.requests.lock().await.len(), if case == "expired" { 2 } else { 1 });
        bounded(handle.close()).await.unwrap();
        storage.close_and_wait().await;
    }
    // The ordinary first owner must durably settle rejection even after its
    // waiter leaves and only a Broker follower remains. A peer CAS loss stays
    // untouched, with the full skew required before that follower may adopt it.
    for case in ["rejected", "peer", "peer-short"] {
        let now = chrono::Utc::now().timestamp_millis();
        let fake = fake_upstream(vec![json!({"delay_ms":120,"status":400,"body":"{}"})]).await;
        let (store, storage) =
            make_storage(&temp.path().join(format!("installed-cancel-reject-{case}.db")), Default::default());
        let id = store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider(
                "fixture-cancel-reject",
                &oauth(json!({"custom":"original-extension"})),
            )
            .unwrap()[0]
            .id;
        let hook = Arc::new(PersistedRefresh {
            store: store.clone(),
            endpoint: format!("http://{}/cancelled-owner-refresh", fake.addr),
            calls: AtomicUsize::new(0),
            explicit: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            gate: Semaphore::new(0),
        });
        storage.register_oauth_provider("fixture-cancel-reject", hook.clone()).unwrap();
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let cancelled_waiter = CancellationToken::new();
        let ordinary = {
            let s = storage.clone();
            let cancel = cancelled_waiter.clone();
            tokio::spawn(async move {
                s.resolve(
                    "fixture-cancel-reject",
                    &AuthRequestContext { force_refresh: true, ..Default::default() },
                    &cancel,
                )
                .await
            })
        };
        bounded(hook.entered.acquire()).await.unwrap().forget();
        let follower = {
            let b = broker(&handle);
            tokio::spawn(async move { b.refresh_credential(id, &CancellationToken::new()).await.is_ok() })
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(65), async {
                while hook.calls.load(Ordering::Acquire) <= 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .is_err()
        );
        hook.gate.add_permits(1);
        wait_requests(&fake, 1).await;
        let peer = if case != "rejected" {
            let peer = oauth(
                json!({"access":"surviving-peer-access","refresh":"surviving-peer-refresh","custom":"surviving-peer-extension","expires":if case=="peer-short" {now+30_000} else {now+3_600_000}}),
            );
            store.lock().unwrap().update_auth_credential(id, &peer).unwrap();
            Some(peer)
        } else {
            None
        };
        cancelled_waiter.cancel();
        assert!(matches!(bounded(ordinary).await.unwrap(), Err(AuthStorageError::Cancelled)));
        assert_eq!(bounded(follower).await.unwrap(), case == "peer");
        bounded(storage.wait_for_settlement()).await;
        let rows = store.lock().unwrap().list_auth_credentials(Some("fixture-cancel-reject")).unwrap();
        let disabled = store.lock().unwrap().list_disabled_credentials(Some("fixture-cancel-reject")).unwrap();
        if let Some(peer) = peer {
            assert_eq!(rows.len(), 1);
            assert!(rows[0].credential == peer);
            assert!(disabled.is_empty());
        } else {
            assert!(rows.is_empty());
            assert_eq!(disabled.len(), 1);
            assert!(bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_err());
            bounded(storage.wait_for_settlement()).await;
        }
        assert_eq!(hook.calls.load(Ordering::Acquire), 1);
        assert_eq!(hook.explicit.load(Ordering::Acquire), 0);
        assert_eq!(fake.requests.lock().await.len(), 1, "rejected original grant is never dispatched again");
        bounded(handle.close()).await.unwrap();
        storage.close_and_wait().await;
    }
    // Native custom provider without refreshToken supports the fresh pure lease,
    // while explicit refresh rejects before ordinary force or issuer dispatch.
    let (store, storage) = make_storage(&temp.path().join("installed-no-refresh.db"), Default::default());
    let id =
        store.lock().unwrap().upsert_auth_credential_for_provider("fixture-no-refresh", &oauth(json!({}))).unwrap()[0]
            .id;
    let hook = Arc::new(NoExplicitRefresh { calls: AtomicUsize::new(0) });
    storage.register_oauth_provider("fixture-no-refresh", hook.clone()).unwrap();
    storage.reload().unwrap();
    let resolved = bounded(storage.resolve("fixture-no-refresh", &Default::default(), &CancellationToken::new()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(resolved.lease.identity(), CredentialIdentity::Stored { id: lease_id, .. } if *lease_id == id));
    assert_eq!(resolved.oauth.as_ref().unwrap().credential_id, id);
    let handle = server(storage.clone()).await;
    assert!(bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_err());
    assert_eq!(hook.calls.load(Ordering::Acquire), 0);
    bounded(handle.close()).await.unwrap();
    storage.close_and_wait().await;
    // Local Codex projects only its own usable Broker-explicit grant, in the
    // existing token/lease CAS. Ordinary force (401 contract), peer CAS loss,
    // known-expired ACK and confirmed transient rejection retain extensions.
    for case in ["broker-explicit", "ordinary-force", "peer-cas-loss", "expired-ack", "transient"] {
        let mut script = if case == "transient" {
            vec![json!({"status":503,"body":json!({"error":"temporarily_unavailable"}).to_string()})]
        } else {
            vec![
                json!({"delay_ms":120,"body":json!({"access_token":"codex-owner-access","refresh_token":"codex-owner-refresh","expires_in":if case=="expired-ack" {0} else {3600}}).to_string()}),
            ]
        };
        if case == "expired-ack" {
            script.push(json!({"body":json!({"access_token":"codex-recovered-access","refresh_token":"codex-recovered-refresh","expires_in":3600}).to_string()}));
        }
        let fake = fake_upstream(script).await;
        let path = temp.path().join(format!("explicit-codex-{case}.db"));
        let store = SqliteCredentialStore::open(&path).unwrap();
        let id = store.upsert_auth_credential_for_provider(
            "openai-codex", &oauth(json!({"accountId":"codex-account","orgId":"codex-org","projectId":"codex-project","authorizedAt":17,"custom":"retained-extension","expires":4102444800000_i64})),
        ).unwrap()[0].id;
        let codex = Arc::new(
            OpenAiCodexAuth::open_with_endpoints(path, client(), &format!("http://{}", fake.addr)).await.unwrap(),
        );
        let storage = Arc::new(AuthStorage::for_codex(codex.clone(), Default::default()).unwrap());
        let handle = server(storage.clone()).await;
        let result = if case == "ordinary-force" {
            codex.resolve_account_credential(id, true, &CancellationToken::new()).await.is_ok()
        } else if case == "peer-cas-loss" {
            let b = broker(&handle);
            let pending = tokio::spawn(async move { b.refresh_credential(id, &CancellationToken::new()).await });
            wait_requests(&fake, 1).await;
            store.update_auth_credential(id, &oauth(json!({"accountId":"codex-account","orgId":"codex-org","access":"codex-peer-access","refresh":"codex-peer-refresh","custom":"peer-extension","expires":4102444800000_i64}))).unwrap();
            bounded(pending).await.unwrap().is_ok()
        } else {
            bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_ok()
        };
        assert_eq!(result, !matches!(case, "expired-ack" | "transient"), "Codex case {case}");
        let row = store.list_auth_credentials(Some("openai-codex")).unwrap().remove(0);
        let AuthCredential::OAuth { fields } = row.credential else { panic!("Codex OAuth") };
        if case == "broker-explicit" {
            assert!(!fields.contains_key("custom"));
            assert_eq!(fields["projectId"], "codex-project");
            assert_eq!(fields["authorizedAt"], 17);
            assert_eq!(fields["access"], "codex-owner-access");
        } else if case == "peer-cas-loss" {
            assert_eq!(fields["custom"], "peer-extension");
            assert_eq!(fields["access"], "codex-peer-access");
            assert_eq!(fields["refresh"], "codex-peer-refresh");
        } else {
            assert_eq!(fields["custom"], "retained-extension");
            if case == "expired-ack" {
                assert_eq!(fields["refresh"], "codex-owner-refresh", "unusable ACK is durable before rejection");
                bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.unwrap();
                let row = store.list_auth_credentials(Some("openai-codex")).unwrap().remove(0);
                let AuthCredential::OAuth { fields } = row.credential else { panic!("recovered OAuth") };
                assert!(!fields.contains_key("custom"));
                let requests = fake.requests.lock().await;
                assert!(requests[1]["body"].as_str().unwrap().contains("refresh_token=codex-owner-refresh"));
            }
        }
        assert_eq!(store.list_disabled_credentials(Some("openai-codex")).unwrap().len(), 0);
        assert_eq!(fake.requests.lock().await.len(), if case == "expired-ack" { 2 } else { 1 });
        bounded(handle.close()).await.unwrap();
        storage.close_and_wait().await;
    }
    // Native override is unpersisted grant material. AuthStorage publishes once
    // against its attempted row; ordinary force and peer rows stay untouched.
    for case in ["broker-explicit", "ordinary-force", "peer-cas-loss", "peer-short-lived", "request-owner-adopted"] {
        let hook = Arc::new(OverrideRefresh { calls: AtomicUsize::new(0), gate: Semaphore::new(0) });
        let (store, storage) = make_storage(
            &temp.path().join(format!("explicit-override-{case}.db")),
            AuthStorageOptions { oauth_refresh_override: Some(hook.clone()), ..Default::default() },
        );
        let id = store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider(
                "fixture",
                &oauth(json!({"custom":"retained-extension","projectId":"fallback-project","orgId":"fallback-org"})),
            )
            .unwrap()[0]
            .id;
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let pending = if matches!(case, "ordinary-force" | "request-owner-adopted") {
            let s = storage.clone();
            tokio::spawn(async move {
                s.resolve(
                    "fixture",
                    &AuthRequestContext { force_refresh: true, ..Default::default() },
                    &CancellationToken::new(),
                )
                .await
                .map(|_| ())
            })
        } else {
            let b = broker(&handle);
            tokio::spawn(async move {
                b.refresh_credential(id, &CancellationToken::new())
                    .await
                    .map(|_| ())
                    .map_err(|_| AuthStorageError::Transient)
            })
        };
        bounded(async {
            while hook.calls.load(Ordering::Acquire) < 1 {
                tokio::task::yield_now().await;
            }
        })
        .await;
        let joined = if case == "request-owner-adopted" {
            let b = broker(&handle);
            Some(tokio::spawn(async move { b.refresh_credential(id, &CancellationToken::new()).await }))
        } else {
            None
        };
        if matches!(case, "peer-cas-loss" | "peer-short-lived") {
            let expires = if case == "peer-short-lived" {
                chrono::Utc::now().timestamp_millis() + 30_000
            } else {
                4102444800000_i64
            };
            store.lock().unwrap().update_auth_credential(id, &oauth(json!({"access":"override-peer-access","refresh":"override-peer-refresh","custom":"peer-extension","expires":expires}))).unwrap();
        }
        if let Some(joined) = joined {
            // Leave the request owner pending while the explicit waiter joins;
            // joining it must not create a second grant or change its mode.
            assert!(
                tokio::time::timeout(Duration::from_millis(40), async {
                    while hook.calls.load(Ordering::Acquire) <= 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .is_err()
            );
            hook.gate.add_permits(1);
            assert!(bounded(joined).await.unwrap().is_ok());
        } else {
            hook.gate.add_permits(1);
        }
        assert_eq!(bounded(pending).await.unwrap().is_ok(), case != "peer-short-lived", "override case {case}");
        let row = store.lock().unwrap().list_auth_credentials(Some("fixture")).unwrap().remove(0);
        let AuthCredential::OAuth { fields } = row.credential else { panic!("override OAuth") };
        if case == "broker-explicit" {
            assert!(!fields.contains_key("custom"));
            assert_eq!(fields["projectId"], "fallback-project");
            assert_eq!(fields["orgId"], "fallback-org");
        } else if matches!(case, "peer-cas-loss" | "peer-short-lived") {
            assert_eq!(fields["custom"], "peer-extension");
            assert_eq!(fields["access"], "override-peer-access");
            assert_eq!(fields["refresh"], "override-peer-refresh");
        } else {
            assert_eq!(fields["custom"], "retained-extension");
        }
        assert_eq!(hook.calls.load(Ordering::Acquire), 1);
        bounded(handle.close()).await.unwrap();
        storage.close_and_wait().await;
    }
    // Enabled timer starts immediately, runs eligible IDs concurrently, ignores
    // a future credential, and gates repeated sweeps while an owner is pending.
    let timer_hook = Arc::new(OverrideRefresh { calls: AtomicUsize::new(0), gate: Semaphore::new(0) });
    let (timer_store, timer_storage) = make_storage(
        &temp.path().join("timer.db"),
        AuthStorageOptions { oauth_refresh_override: Some(timer_hook.clone()), ..Default::default() },
    );
    let mut timer_ids = Vec::new();
    for provider in ["near-a", "near-b"] {
        timer_ids.push(
            timer_store
                .lock()
                .unwrap()
                .upsert_auth_credential_for_provider(provider, &oauth(json!({"expires":0})))
                .unwrap()[0]
                .id,
        );
    }
    timer_store.lock().unwrap().upsert_auth_credential_for_provider("future", &oauth(json!({}))).unwrap();
    timer_storage.reload().unwrap();
    let mut options = AuthBrokerServerOptions::new(timer_storage.clone(), vec!["fixture-token".into()]);
    options.bind = "127.0.0.1:0".into();
    options.refresh_interval = Duration::from_millis(20);
    let timer_server = start_auth_broker(options).await.unwrap();
    bounded(async {
        while timer_hook.calls.load(Ordering::Acquire) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let (_, projection) = request(&timer_server, reqwest::Method::GET, "/v1/snapshot", None).await;
    assert_eq!(projection["refresher"]["enabled"], true);
    assert_eq!(projection["refresher"]["intervalMs"], 20.0);
    assert!(projection["credentials"].as_array().unwrap().iter().all(|entry| entry["rotatesInMs"].is_number()));
    let waiter = broker(&timer_server);
    let manual = tokio::spawn(async move { waiter.refresh_credential(timer_ids[0], &CancellationToken::new()).await });
    assert!(
        tokio::time::timeout(Duration::from_millis(65), async {
            while timer_hook.calls.load(Ordering::Acquire) <= 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
    );
    bounded(timer_server.close()).await.unwrap();
    timer_hook.gate.add_permits(2);
    bounded(timer_storage.wait_for_settlement()).await;
    let _ = bounded(manual).await;
    assert_eq!(timer_hook.calls.load(Ordering::Acquire), 2);
    timer_store
        .lock()
        .unwrap()
        .upsert_auth_credential_for_provider("after-close", &oauth(json!({"expires":0})))
        .unwrap();
    timer_storage.reload().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(65), async {
            while timer_hook.calls.load(Ordering::Acquire) <= 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .is_err()
    );
    timer_storage.close_and_wait().await;
    // The explicit override and Broker route share one flight, including after
    // a request and the transport abandon their waits. Normal Host exit drains it.
    let hook = Arc::new(OverrideRefresh { calls: AtomicUsize::new(0), gate: Semaphore::new(0) });
    let (store, storage) = make_storage(
        &temp.path().join("override.db"),
        AuthStorageOptions { oauth_refresh_override: Some(hook.clone()), ..Default::default() },
    );
    let id = store.lock().unwrap().upsert_auth_credential_for_provider("fixture", &oauth(json!({}))).unwrap()[0].id;
    storage.reload().unwrap();
    let handle = server(storage.clone()).await;
    let b = broker(&handle);
    let cancel = CancellationToken::new();
    let bb = b.clone();
    let cc = cancel.clone();
    let first = tokio::spawn(async move { bb.refresh_credential(id, &cc).await });
    bounded(async {
        while hook.calls.load(Ordering::Acquire) < 1 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let bb = b.clone();
    let second = tokio::spawn(async move { bb.refresh_credential(id, &CancellationToken::new()).await });
    tokio::task::yield_now().await;
    cancel.cancel();
    assert!(bounded(first).await.unwrap().err().unwrap().is_cancelled());
    bounded(handle.close()).await.unwrap();
    hook.gate.add_permits(1);
    bounded(storage.wait_for_settlement()).await;
    let _ = bounded(second).await;
    assert_eq!(hook.calls.load(Ordering::Acquire), 1);
    let row = store.lock().unwrap().list_auth_credentials(Some("fixture")).unwrap().remove(0);
    assert!(matches!(&row.credential,AuthCredential::OAuth {fields} if fields["access"]=="fixture-new"));
    // Caller override wins even when the managed MCP issuer is available.
    let override_issuer = fake_upstream(vec![json!({"body":"unexpected issuer dispatch"})]).await;
    let override_hook = Arc::new(OverrideRefresh { calls: AtomicUsize::new(0), gate: Semaphore::new(1) });
    let (override_store, override_storage) = make_storage(
        &temp.path().join("managed-override.db"),
        AuthStorageOptions { oauth_refresh_override: Some(override_hook.clone()), ..Default::default() },
    );
    let override_id = override_store
        .lock()
        .unwrap()
        .upsert_auth_credential_for_provider(
            "mcp_oauth_legacy_override",
            &oauth(json!({"tokenUrl":format!("http://{}/token", override_issuer.addr),"clientId":"fixture-client"})),
        )
        .unwrap()[0]
        .id;
    override_storage.reload().unwrap();
    let override_server = server(override_storage.clone()).await;
    let entry =
        bounded(broker(&override_server).refresh_credential(override_id, &CancellationToken::new())).await.unwrap();
    assert!(matches!(&entry.credential, AuthCredential::OAuth {fields} if fields["access"]=="fixture-new"));
    assert_eq!(override_hook.calls.load(Ordering::Acquire), 1);
    assert_eq!(override_issuer.requests.lock().await.len(), 0, "override must bypass managed issuer dispatch");
    bounded(override_server.close()).await.unwrap();
    override_storage.close_and_wait().await;
    // Both managed eras, resource origin policy, response fallback, and exact
    // Native descriptor loss are exercised together through the real HTTP server.
    for case in 0..6 {
        let fake =
            fake_upstream(vec![json!({"body":json!({"access_token":"mcp-new","expires_in":3600}).to_string()})]).await;
        let origin = format!("http://{}", fake.addr);
        let provider = match case {
            0 | 1 | 4 | 5 => format!("mcp_oauth:{origin}/mcp?q=1"),
            2 => "mcp_oauth:profile:fixture:https://resource.invalid/mcp?q=1".into(),
            _ => "mcp_oauth_legacy_fixture".into(),
        };
        let (store, storage) = make_storage(&temp.path().join(format!("resource-{case}.db")), Default::default());
        let mut extra = json!({"tokenUrl":format!("{origin}/token"),"clientId":" client-id ","clientSecret":"fixture-client-secret","accountId":"mcp-account","authorizedAt":42});
        if case == 1 {
            extra["resource"] = json!(format!("{origin}/advertised"));
        }
        if case == 4 {
            extra["authorizationUrl"] = json!("https://other-issuer.invalid/auth");
        }
        if case == 5 {
            extra["resource"] = json!("");
        }
        let id = store.lock().unwrap().upsert_auth_credential_for_provider(&provider, &oauth(extra)).unwrap()[0].id;
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let b = broker(&handle);
        let refreshed = bounded(b.refresh_credential(id, &CancellationToken::new())).await.unwrap();
        assert!(
            matches!(&refreshed.credential,AuthCredential::OAuth {fields} if fields["refresh"]=="__remote__" && fields["accountId"]=="mcp-account")
        );
        let calls = fake.requests.lock().await;
        let raw = calls[0]["body"].as_str().unwrap();
        let form = reqwest::Url::parse(&format!("http://fixture/?{raw}"))
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect::<std::collections::BTreeMap<_, _>>();
        assert_eq!(form["client_id"], "client-id");
        assert_eq!(form["client_secret"], "fixture-client-secret");
        assert_eq!(form["refresh_token"], "fixture-refresh");
        match case {
            0 | 3 | 5 => assert!(!form.contains_key("resource")),
            1 => assert_eq!(form["resource"], format!("{origin}/advertised")),
            2 => assert_eq!(form["resource"], "https://resource.invalid/mcp?q=1"),
            4 => assert_eq!(form["resource"], format!("{origin}/mcp?q=1")),
            _ => unreachable!(),
        };
        drop(calls);
        assert!(bounded(b.refresh_credential(id, &CancellationToken::new())).await.is_err());
        assert_eq!(fake.requests.lock().await.len(), 1, "second refresh has no descriptor and zero dispatch");
        let rows = store.lock().unwrap().list_auth_credentials(Some(&provider)).unwrap();
        let AuthCredential::OAuth { fields } = &rows[0].credential else { unreachable!() };
        assert_eq!(fields["refresh"], "fixture-refresh");
        for field in ["tokenUrl", "clientId", "clientSecret", "resource", "authorizationUrl"] {
            assert!(!fields.contains_key(field));
        }
        handle.close().await.unwrap();
        storage.close_and_wait().await;
    }
    for (status, detail, disabled) in
        [(400, "invalid_grant", true), (401, "cloudflare captcha", false), (429, "invalid_grant", true)]
    {
        let fake = fake_upstream(vec![json!({"status":status,"body":detail})]).await;
        let (store, storage) = make_storage(&temp.path().join(format!("failure-{status}.db")), Default::default());
        let provider = "mcp_oauth_legacy_fixture";
        let id = store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider(
                provider,
                &oauth(json!({"tokenUrl":format!("http://{}/token",fake.addr)})),
            )
            .unwrap()[0]
            .id;
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        assert!(bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_err());
        assert_eq!(
            store.lock().unwrap().list_disabled_credentials(Some(provider)).unwrap().len(),
            usize::from(disabled)
        );
        assert_eq!(fake.requests.lock().await.len(), 1);
        handle.close().await.unwrap();
        storage.close_and_wait().await;
    }
    // Build and execute-time builder errors are proven zero dispatch. They may
    // be retried after repairing the URL with the same rotating refresh token.
    for (case, bad_url) in [("malformed", "://bad-token-url"), ("unsupported-scheme", "file:///broker-token")] {
        let fake = fake_upstream(vec![json!({"body":json!({"access_token":"repaired-access"}).to_string()})]).await;
        let (store, storage) = make_storage(&temp.path().join(format!("zero-dispatch-{case}.db")), Default::default());
        let provider = "mcp_oauth_legacy_zero_dispatch";
        let id = store
            .lock()
            .unwrap()
            .upsert_auth_credential_for_provider(provider, &oauth(json!({"tokenUrl":bad_url})))
            .unwrap()[0]
            .id;
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let b = broker(&handle);
        for _ in 0..2 {
            assert!(bounded(b.refresh_credential(id, &CancellationToken::new())).await.is_err());
        }
        assert_eq!(fake.requests.lock().await.len(), 0);
        store
            .lock()
            .unwrap()
            .update_auth_credential(id, &oauth(json!({"tokenUrl":format!("http://{}/token",fake.addr)})))
            .unwrap();
        assert!(
            bounded(b.refresh_credential(id, &CancellationToken::new())).await.is_ok(),
            "zero-dispatch failure must not poison the unchanged refresh token"
        );
        assert_eq!(fake.requests.lock().await.len(), 1);
        handle.close().await.unwrap();
        storage.close_and_wait().await;
    }
    // Native known-expired success first persists the merged new grant, then
    // fails usability before standard outer projection. The next explicit call
    // may use that new refresh token, but must not repeat the old rotating grant.
    for (case, expires_in) in [("zero", 0), ("negative", -1)] {
        let fake = fake_upstream(vec![
            json!({"body":json!({"access_token":"unusable-access","refresh_token":"unusable-refresh","expires_in":expires_in}).to_string()}),
            json!({"body":json!({"access_token":"usable-access","refresh_token":"usable-refresh","expires_in":3600}).to_string()}),
        ]).await;
        let (store, storage) = make_storage(&temp.path().join(format!("expiry-{case}.db")), Default::default());
        let provider = "mcp_oauth_legacy_invalid_expiry";
        let extra = json!({"tokenUrl":format!("http://{}/token",fake.addr),"clientId":"fixture-client","clientSecret":"fixture-client-secret","resource":"https://resource.invalid/mcp","authorizationUrl":"https://issuer.invalid/auth","custom":"preserved-on-unusable"});
        let id =
            store.lock().unwrap().upsert_auth_credential_for_provider(provider, &oauth(extra.clone())).unwrap()[0].id;
        storage.reload().unwrap();
        let handle = server(storage.clone()).await;
        let b = broker(&handle);
        let (status, _) = request(&handle, reqwest::Method::POST, &format!("/v1/credential/{id}/refresh"), None).await;
        assert_eq!(status, 500, "known unusable grant cannot be a successful force response");
        assert_eq!(fake.requests.lock().await.len(), 1);
        let rows = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        let AuthCredential::OAuth { fields } = &rows[0].credential else { unreachable!() };
        assert_eq!(fields["access"], "unusable-access");
        assert_eq!(fields["refresh"], "unusable-refresh");
        assert!(fields["expires"].as_f64().unwrap() <= chrono::Utc::now().timestamp_millis() as f64);
        for field in ["tokenUrl", "clientId", "clientSecret", "resource", "authorizationUrl", "custom"] {
            assert_eq!(fields[field], extra[field]);
        }
        assert!(store.lock().unwrap().list_disabled_credentials(Some(provider)).unwrap().is_empty());
        let snapshot = ara_cli::auth_storage::BrokerStorageFacade::new(storage.clone()).export_snapshot().unwrap();
        assert!(
            matches!(&snapshot.credentials[0].credential,AuthCredential::OAuth {fields} if fields["access"]=="unusable-access")
        );
        let recovered = bounded(b.refresh_credential(id, &CancellationToken::new())).await.unwrap();
        assert!(matches!(&recovered.credential,AuthCredential::OAuth {fields} if fields["access"]=="usable-access"));
        let calls = fake.requests.lock().await;
        assert_eq!(calls.len(), 2);
        let grants = calls
            .iter()
            .map(|call| {
                reqwest::Url::parse(&format!("http://fixture/?{}", call["body"].as_str().unwrap()))
                    .unwrap()
                    .query_pairs()
                    .find(|(key, _)| key == "refresh_token")
                    .unwrap()
                    .1
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(grants, ["fixture-refresh", "unusable-refresh"]);
        drop(calls);
        let rows = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
        let AuthCredential::OAuth { fields } = &rows[0].credential else { unreachable!() };
        assert_eq!(fields["refresh"], "usable-refresh");
        for field in ["tokenUrl", "clientId", "clientSecret", "resource", "authorizationUrl", "custom"] {
            assert!(!fields.contains_key(field), "fresh force uses standard projection");
        }
        handle.close().await.unwrap();
        storage.close_and_wait().await;
    }
    // Lost/invalid successful ACK retains its durable fingerprint. A fresh
    // AuthStorage using the same SQLite cannot dispatch the same grant again.
    let fake = fake_upstream(vec![json!({"body":"invalid-success-json"})]).await;
    let path = temp.path().join("unknown.db");
    let (store, storage) = make_storage(&path, Default::default());
    let provider = "mcp_oauth_legacy_unknown";
    let id = store
        .lock()
        .unwrap()
        .upsert_auth_credential_for_provider(provider, &oauth(json!({"tokenUrl":format!("http://{}/token",fake.addr)})))
        .unwrap()[0]
        .id;
    storage.reload().unwrap();
    let handle = server(storage.clone()).await;
    assert!(bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_err());
    handle.close().await.unwrap();
    bounded(storage.wait_for_settlement()).await;
    let restarted = Arc::new(AuthStorage::new(store.clone(), None, Default::default()).unwrap());
    let handle = server(restarted.clone()).await;
    assert!(bounded(broker(&handle).refresh_credential(id, &CancellationToken::new())).await.is_err());
    assert_eq!(fake.requests.lock().await.len(), 1);
    assert_eq!(store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap().len(), 1);
    handle.close().await.unwrap();
    restarted.close_and_wait().await;
    // A peer/login swap during grant preserves that row when the old CAS loses.
    let fake=fake_upstream(vec![json!({"delay_ms":120,"body":json!({"access_token":"obsolete-result","refresh_token":"obsolete-refresh"}).to_string()})]).await;
    let (store, storage) = make_storage(&temp.path().join("peer.db"), Default::default());
    let provider = "mcp_oauth_legacy_peer";
    let extra = json!({"tokenUrl":format!("http://{}/token",fake.addr)});
    let id = store.lock().unwrap().upsert_auth_credential_for_provider(provider, &oauth(extra.clone())).unwrap()[0].id;
    storage.reload().unwrap();
    let handle = server(storage.clone()).await;
    let b = broker(&handle);
    let flight = tokio::spawn(async move { b.refresh_credential(id, &CancellationToken::new()).await });
    wait_requests(&fake, 1).await;
    let mut peer = extra;
    peer["access"] = json!("peer-access");
    peer["refresh"] = json!("peer-refresh");
    store.lock().unwrap().update_auth_credential(id, &oauth(peer)).unwrap();
    assert!(bounded(flight).await.unwrap().is_err());
    let rows = store.lock().unwrap().list_auth_credentials(Some(provider)).unwrap();
    assert!(
        matches!(&rows[0].credential,AuthCredential::OAuth {fields} if fields["access"]=="peer-access" && fields["refresh"]=="peer-refresh")
    );
    handle.close().await.unwrap();
    storage.close_and_wait().await;
}
