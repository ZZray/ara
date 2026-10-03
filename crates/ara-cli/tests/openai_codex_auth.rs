//! Complete host auth flows against a controlled local HTTP upstream and real
//! temporary SQLite. These do not claim real OpenAI authorization acceptance.
#![cfg(feature = "test-fixture")]

use ara_ai::{CallOptions, Context, Message, Model, StopReason, UserMessage};
use ara_cli::auth_storage::{AuthRequestContext, AuthStorage, AuthStorageError, AuthStorageOptions};
use ara_cli::auth_storage_policy::UsageReport;
use ara_cli::codex_usage::CodexUsageProvider;
use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore, StoredAuthCredential};
use ara_cli::model_route::{AuthResolveError, CredentialIdentity, PreparedRoute, ProtocolOptions, RequestAuthResolver};
use ara_cli::openai_codex_auth::{CodexAuthError, OpenAiCodexAuth};
use ara_testkit::{FakeUpstream, Script};
use base64::Engine as _;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

fn jwt(payload: Value) -> String {
    format!("fixture.{}.signature", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string()))
}

fn access(account: &str, residency: &str) -> String {
    access_with_email(account, residency, " NEW@Example.COM ")
}

fn access_with_email(account: &str, residency: &str, email: &str) -> String {
    jwt(json!({"https://api.openai.com/auth": {
        "chatgpt_account_id": account, "chatgpt_data_residency": residency,
        "chatgpt_plan_type": "new-plan",
    }, "https://api.openai.com/profile": {"email": email}}))
}

fn model(base_url: String) -> Model {
    Model {
        id: "gpt-5.3-codex".into(),
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        base_url,
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    }
}

fn response(body: Value) -> Value {
    json!({"body": body.to_string()})
}

fn completed() -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{
            "type":"message","id":"msg_fixture","content":[{"type":"output_text","text":"hello"}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_fixture","status":"completed"}}}
    ]})
}

async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses": responses})).unwrap(), None).await.unwrap()
}

async fn auth(path: &Path, fake: &FakeUpstream) -> OpenAiCodexAuth {
    OpenAiCodexAuth::open_with_endpoints(path.to_owned(), client(), &format!("http://{}", fake.addr)).await.unwrap()
}

fn seed(path: &Path, account: &str, authorized_at: Option<i64>, expires: i64) -> i64 {
    let store = SqliteCredentialStore::open(path).unwrap();
    let email = format!("{account}@example.invalid");
    let mut fields = json!({
        "access": access_with_email(account, "old-region", &email), "refresh": "fixture-refresh-original", "expires": expires,
        // Native credential replacement identifies Codex by email + org.
        // Distinct accounts must have distinct identities before row tests run.
        "accountId": account, "email": email, "orgId": "original-org", "orgName": "original-plan",
    })
    .as_object()
    .unwrap()
    .clone();
    if let Some(at) = authorized_at {
        fields.insert("authorizedAt".into(), at.into());
    }
    let rows = store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(fields)).unwrap();
    rows.iter()
        .find(|row| match &row.credential {
            AuthCredential::OAuth { fields } => fields.get("accountId").and_then(Value::as_str) == Some(account),
            _ => false,
        })
        .unwrap()
        .id
}

fn fields(path: &Path, id: i64) -> Value {
    let store = SqliteCredentialStore::open(path).unwrap();
    let rows = store.list_auth_credentials(Some("openai-codex")).unwrap();
    let row = rows.into_iter().find(|row| row.id == id).unwrap();
    match row.credential {
        AuthCredential::OAuth { fields } => Value::Object(fields),
        _ => panic!("expected oauth"),
    }
}

async fn wait_requests(fake: &FakeUpstream, count: usize) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if fake.requests.lock().await.len() >= count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("controlled request reached upstream");
}

fn usage_payload(tag: &str) -> Value {
    json!({"fixtureTag":tag,"plan_type":"pro","rate_limit":{
        "allowed":true,"limit_reached":false,
        "primary_window":{"used_percent":11,"limit_window_seconds":18000,"reset_after_seconds":600}
    }})
}

fn usage_storage(service: Arc<OpenAiCodexAuth>, fake: &FakeUpstream, timeout: Duration) -> AuthStorage {
    let storage = AuthStorage::for_codex(
        service,
        AuthStorageOptions { usage_request_timeout: timeout, jitter: Arc::new(|| 0.5), ..Default::default() },
    )
    .unwrap();
    let origin = format!("http://{}", fake.addr);
    let builtin = CodexUsageProvider::with_fixture_endpoint_resolver(Arc::new(move |canonical| {
        let path = reqwest::Url::parse(canonical).unwrap().path().to_owned();
        format!("{origin}{path}")
    }))
    .unwrap();
    storage.register_usage_provider("openai-codex", Arc::new(builtin)).unwrap();
    storage
}

fn replace_expiry(path: &Path, id: i64, expires: i64) {
    let store = SqliteCredentialStore::open(path).unwrap();
    let row = store.list_auth_credentials(Some("openai-codex")).unwrap().into_iter().find(|row| row.id == id).unwrap();
    let mut replacement = fields(path, id).as_object().unwrap().clone();
    replacement.insert("expires".into(), expires.into());
    assert!(
        store
            .try_update_auth_credential_if_matches(id, &row.serialized_data, &AuthCredential::oauth(replacement), None)
            .unwrap()
    );
}

fn stale_warmed_usage(path: &Path, account: &str, report: &UsageReport) -> String {
    // Source: auth_storage::{USAGE_CACHE_PREFIX, usage_identity, usage_report_key}.
    // Warm through the builtin GET before changing only payload expiresAt.
    let key = format!(
        "usage_cache:report:openai-codex:default:oauth|account:{account}|email:{account}@example.invalid|org:original-org"
    );
    let db = rusqlite::Connection::open(path).unwrap();
    let (raw, durable_expiry): (String, i64) = db
        .query_row("SELECT value,expires_at FROM cache WHERE key=?1", [&key], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap();
    let mut cached: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(cached["value"], serde_json::to_value(report).unwrap());
    assert!(durable_expiry > now_ms() / 1000);
    cached["expiresAt"] = (now_ms() - 1).into();
    assert_eq!(
        db.execute("UPDATE cache SET value=?1 WHERE key=?2", rusqlite::params![cached.to_string(), key]).unwrap(),
        1
    );
    let after: i64 = db.query_row("SELECT expires_at FROM cache WHERE key=?1", [&key], |row| row.get(0)).unwrap();
    assert_eq!(after, durable_expiry, "last-good retention and epoch were not invalidated");
    key
}

async fn usage_reports(storage: &AuthStorage) -> Vec<UsageReport> {
    storage
        .fetch_usage_reports(Some("openai-codex"), &AuthRequestContext::default(), &CancellationToken::new())
        .await
        .unwrap()
}

fn cached_usage(path: &Path, key: &str) -> Value {
    let store = SqliteCredentialStore::open(path).unwrap();
    serde_json::from_str(&store.get_cache(key, true).unwrap().unwrap()).unwrap()
}

#[tokio::test]
async fn device_profile_persistence_private_wire_identity_and_logout() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("private/auth.db");
    // Account/email/plan can be supplied by id_token while residency belongs
    // exclusively to the access token being sent on this request.
    let token = jwt(json!({"https://api.openai.com/auth": {
        "chatgpt_data_residency": "  ", "chatgpt_compute_residency": " eu "
    }}));
    let id_token = jwt(json!({
        "https://api.openai.com/auth": {"chatgpt_account_id": "workspace-a", "chatgpt_plan_type": " TEAM "},
        "https://api.openai.com/profile": {"email": " User@EXAMPLE.com "},
    }));
    let fake = upstream(vec![
        response(json!({"device_auth_id":"device-fixture", "user_code":"ABCD-EFGH", "interval":"1second"})),
        json!({"status":403}),
        json!({"status":404}),
        response(json!({"authorization_code":"fixture-code", "code_verifier":"fixture-verifier"})),
        response(
            json!({"access_token":token, "refresh_token":"fixture-refresh", "id_token":id_token, "expires_in":3600}),
        ),
        completed(),
    ])
    .await;
    let service = auth(&path, &fake).await;
    let shown = Arc::new(Mutex::new(Vec::new()));
    let output = shown.clone();
    let identity = service
        .login_device(&CancellationToken::new(), move |info| {
            output.lock().unwrap().push((info.verification_url, info.user_code));
        })
        .await
        .unwrap();
    assert_eq!(identity.account_id, "workspace-a");
    assert_eq!(identity.email.as_deref(), Some("user@example.com"));
    assert_eq!(&*shown.lock().unwrap(), &[("https://auth.openai.com/codex/device", "ABCD-EFGH".into())]);
    let stored = fields(&path, identity.credential_id);
    assert_eq!(stored["orgId"], "workspace-a");
    assert_eq!(stored["orgName"], "team");
    assert!(stored["authorizedAt"].as_i64().unwrap() > 0);
    drop(service);
    let reopened = Arc::new(auth(&path, &fake).await);
    let target = model(format!("http://{}/backend-api", fake.addr));
    let lease = reopened.resolve(&target, &CancellationToken::new()).await.unwrap();
    assert!(matches!(lease.identity(), CredentialIdentity::Stored { id, .. } if *id == identity.credential_id));
    let route =
        PreparedRoute::new(target.clone(), ProtocolOptions::CodexResponses(Default::default()), reopened.clone(), 1)
            .unwrap();
    let provider = route.bind(client(), None);
    let context = Context {
        system_prompt: vec!["fixture".into()],
        messages: vec![Message::User(UserMessage::text("hello"))],
        tools: None,
    };
    let mut stream = provider.stream(&target, &context, CallOptions::default());
    let mut terminal = None;
    while let Some(event) = stream.recv().await {
        if event.is_terminal() {
            terminal = Some(event.partial().clone());
        }
    }
    assert_eq!(terminal.unwrap().stop_reason, StopReason::Stop);
    let requests = fake.requests.lock().await;
    assert_eq!(requests[0]["body"]["client_id"], "app_EMoamEEZ73f0CkXaXp7hrann");
    assert!(requests[4]["body"].as_str().unwrap().contains("grant_type=authorization_code"));
    assert!(requests[4]["body"].as_str().unwrap().contains("code_verifier=fixture-verifier"));
    assert_eq!(requests[5]["headers"]["chatgpt-account-id"], "workspace-a");
    assert_eq!(requests[5]["headers"]["x-openai-internal-codex-residency"], "eu");
    assert_eq!(requests[5]["headers"]["authorization"], format!("<redacted {} chars>", token.len() + 7));
    drop(requests);
    reopened.logout().await.unwrap();
    assert!(matches!(reopened.resolve(&target, &CancellationToken::new()).await, Err(AuthResolveError::Unavailable)));
    let store = SqliteCredentialStore::open(&path).unwrap();
    assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
    assert_eq!(store.list_disabled_credentials(Some("openai-codex")).unwrap().len(), 1);
    assert_eq!(fake.served(), 6);
}

#[tokio::test]
async fn latest_interactive_identity_and_cross_connection_refresh_have_one_winner() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.db");
    seed(&path, "legacy", None, now_ms() + 3_600_000);
    seed(&path, "older", Some(41), now_ms() + 3_600_000);
    seed(&path, "tie-earlier", Some(42), now_ms() + 3_600_000);
    let id = seed(&path, "selected", Some(42), 0);
    let new_access = access("selected", "us");
    let mut refreshed = response(json!({"access_token":new_access, "expires_in":3600}));
    refreshed["delay_ms"] = 100.into();
    let fake = upstream(vec![refreshed]).await;
    let first = auth(&path, &fake).await;
    let second = auth(&path, &fake).await;
    let target = model(format!("http://{}/backend-api", fake.addr));
    let cancel = CancellationToken::new();
    let (a, b) = tokio::join!(first.resolve(&target, &cancel), second.resolve(&target, &cancel));
    for lease in [a.unwrap(), b.unwrap()] {
        assert!(matches!(lease.identity(), CredentialIdentity::Stored { id: actual, .. } if *actual == id));
    }
    assert_eq!(fake.served(), 1);
    let stored = fields(&path, id);
    assert_eq!(stored["access"], new_access);
    assert_eq!(stored["refresh"], "fixture-refresh-original");
    assert_eq!(stored["authorizedAt"], 42);
    assert_eq!(stored["orgId"], "original-org");
    assert_eq!(stored["orgName"], "original-plan");
    assert_eq!(stored["email"], "new@example.com");
}

// Fixed AuthStorage force-refresh/rotation and OAuth refresh-race families,
// grouped at the actual HTTP + durable-store seam. Real OpenAI login is separate.
#[tokio::test]
async fn native_exact_force_refresh_rejections_and_future_grant_fences() {
    let cancel = CancellationToken::new();
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("force.db");
    let selected_id = seed(&path, "force-target", None, now_ms() + 3_600_000);
    let sibling_id = seed(&path, "latest-sibling", Some(99), now_ms() + 3_600_000);
    let sibling_before = fields(&path, sibling_id);
    let new_access = access("force-target", "us");
    let mut refresh = response(json!({"access_token":new_access,"expires_in":3600}));
    refresh["delay_ms"] = 100.into();
    let fake = upstream(vec![refresh]).await;
    let first = auth(&path, &fake).await;
    let second = auth(&path, &fake).await;
    first.resolve_account_credential(selected_id, false, &cancel).await.unwrap();
    assert_eq!(fake.served(), 0, "normal native resolution keeps a fresh bearer");
    let (left, right) = tokio::join!(
        first.resolve_account_credential(selected_id, true, &cancel),
        second.resolve_account_credential(selected_id, true, &cancel),
    );
    for row in [left.unwrap(), right.unwrap()] {
        assert_eq!(row.id, selected_id);
        let AuthCredential::OAuth { fields } = row.credential else { panic!("OAuth row") };
        assert_eq!(fields["access"], new_access);
    }
    assert_eq!(fake.served(), 1, "simultaneous forced refreshes coalesce through the exact row fence");
    assert_eq!(fields(&path, sibling_id), sibling_before);
    drop(fake);

    for (case, (status, body, definitive)) in [
        (400, json!({"error":"invalid_grant"}).to_string(), true),
        (400, json!({"error":"invalid_request"}).to_string(), false),
        (401, json!({"error":"invalid_token"}).to_string(), true),
        (403, json!({"error":"forbidden"}).to_string(), false),
        (429, json!({"error":"rate_limit","message":"too many requests"}).to_string(), false),
        (503, json!({"error":"temporarily_unavailable"}).to_string(), false),
        (401, "temporarily unavailable".to_owned(), false),
        (401, "<html>Cloudflare forbidden</html>".to_owned(), false),
        (400, "invalid_grant".to_owned(), true),
        (400, format!("{}invalid_grant", "x".repeat(500)), false),
    ]
    .into_iter()
    .enumerate()
    {
        let path = temp.path().join(format!("reject-{case}.db"));
        let id = seed(&path, "reject-target", None, now_ms() + 3_600_000);
        let before = fields(&path, id);
        let fake = upstream(vec![json!({"status":status,"body":body})]).await;
        let service = auth(&path, &fake).await;
        assert!(matches!(
            service.resolve_account_credential(id, true, &cancel).await,
            Err(CodexAuthError::RefreshRejected { status:actual, definitive:category }) if actual == status && category == definitive
        ));
        let store = SqliteCredentialStore::open(&path).unwrap();
        assert_eq!(store.list_disabled_credentials(Some("openai-codex")).unwrap().len(), usize::from(definitive));
        if !definitive {
            assert_eq!(fields(&path, id), before, "a confirmed transient rejection preserves the grant");
        }
        assert_eq!(fake.served(), 1);
        drop(fake);

        // The classifier must survive the actual shared Host layer as well.
        // Otherwise a retained transient row can be disabled again upstream.
        let host_path = temp.path().join(format!("host-reject-{case}.db"));
        let host_id = seed(&host_path, "host-reject", None, now_ms() + 3_600_000);
        let before = fields(&host_path, host_id);
        let fake = upstream(vec![json!({"status":status,"body":body})]).await;
        let service = Arc::new(auth(&host_path, &fake).await);
        let storage = ara_cli::auth_storage::AuthStorage::for_codex(service, Default::default()).unwrap();
        let failed = ara_cli::model_route::RequestAuthLease::new(
            CredentialIdentity::Stored { id: host_id, revision: 0 },
            Some(before["access"].as_str().unwrap().to_owned()),
        );
        assert!(
            storage
                .refresh_failed_lease(
                    "openai-codex",
                    &ara_cli::auth_storage::AuthRequestContext::default(),
                    &failed,
                    &cancel
                )
                .await
                .unwrap()
                .is_none()
        );
        storage.wait_for_settlement().await;
        let store = SqliteCredentialStore::open(&host_path).unwrap();
        assert_eq!(
            store.list_disabled_credentials(Some("openai-codex")).unwrap().len(),
            usize::from(definitive),
            "Host classification for HTTP {status}"
        );
        if !definitive {
            assert_eq!(fields(&host_path, host_id), before);
            let blocks = storage.list_credential_blocks(&[host_id]).unwrap();
            assert!(
                blocks.iter().any(|block| block.blocked_until_ms > now_ms() + 290_000),
                "transient rejection applies native five-minute block"
            );
        }
        assert_eq!(fake.served(), 1);
        drop(fake);
    }

    // A missing required refresh field is a native configuration failure,
    // without any rejected grant or request. The shared coordinator blocks
    // it temporarily rather than disabling the durable account.
    let path = temp.path().join("host-missing-refresh.db");
    let id = seed(&path, "missing-refresh", None, now_ms() - 1);
    let store = SqliteCredentialStore::open(&path).unwrap();
    let row = store.list_auth_credentials(Some("openai-codex")).unwrap().remove(0);
    let AuthCredential::OAuth { fields: mut payload } = row.credential else { panic!("OAuth row") };
    payload.remove("refresh");
    assert!(
        store
            .try_update_auth_credential_if_matches(id, &row.serialized_data, &AuthCredential::oauth(payload), None)
            .unwrap()
    );
    let before = fields(&path, id);
    let fake = upstream(vec![]).await;
    let service = Arc::new(auth(&path, &fake).await);
    let storage = ara_cli::auth_storage::AuthStorage::for_codex(service, Default::default()).unwrap();
    let failed = ara_cli::model_route::RequestAuthLease::new(
        CredentialIdentity::Stored { id, revision: 0 },
        Some(before["access"].as_str().unwrap().to_owned()),
    );
    assert!(
        storage.refresh_failed_lease("openai-codex", &Default::default(), &failed, &cancel).await.unwrap().is_none()
    );
    storage.wait_for_settlement().await;
    assert_eq!(fields(&path, id), before);
    assert!(store.list_disabled_credentials(Some("openai-codex")).unwrap().is_empty());
    assert!(
        storage.list_credential_blocks(&[id]).unwrap().iter().any(|block| block.blocked_until_ms > now_ms() + 290_000)
    );
    assert_eq!(fake.served(), 0);
    drop(fake);

    let path = temp.path().join("forced-future-fence.db");
    let id = seed(&path, "fence-target", None, now_ms() + 3_600_000);
    let sibling_id = seed(&path, "fence-sibling", Some(100), now_ms() + 3_600_000);
    let sibling_before = fields(&path, sibling_id);
    let mut refresh = response(json!({"access_token":access("fence-target","us"),"expires_in":3600}));
    refresh["delay_ms"] = 120.into();
    let fake = upstream(vec![refresh]).await;
    let service = auth(&path, &fake).await;
    let pending = {
        let service = service.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { service.resolve_account_credential(id, true, &cancel).await })
    };
    wait_requests(&fake, 1).await;
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute("UPDATE auth_credential_refresh_leases SET owner='fixture-peer' WHERE credential_id=?1", [id])
        .unwrap();
    assert!(matches!(pending.await.unwrap(), Err(CodexAuthError::LoginRequired)));
    service.wait_for_settlement().await;
    let store = SqliteCredentialStore::open(&path).unwrap();
    assert_eq!(store.list_disabled_credentials(Some("openai-codex")).unwrap()[0].id, id);
    assert_eq!(fields(&path, sibling_id), sibling_before);
    assert_eq!(fake.served(), 1, "a lost future-dated force grant cannot be replayed or returned");
    drop(fake);
}

#[tokio::test]
async fn logout_during_refresh_cannot_resurrect_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.db");
    seed(&path, "selected", Some(42), 0);
    let mut refreshed =
        response(json!({"access_token":access("selected", "us"), "refresh_token":"rotated", "expires_in":3600}));
    refreshed["delay_ms"] = 150.into();
    let fake = upstream(vec![refreshed]).await;
    let service = Arc::new(auth(&path, &fake).await);
    let target = model(format!("http://{}/backend-api", fake.addr));
    let worker = service.clone();
    let target2 = target.clone();
    let resolving = tokio::spawn(async move { worker.resolve(&target2, &CancellationToken::new()).await });
    wait_requests(&fake, 1).await;
    service.logout().await.unwrap();
    assert!(matches!(resolving.await.unwrap(), Err(AuthResolveError::Unavailable)));
    let reopened = auth(&path, &fake).await;
    assert!(matches!(reopened.resolve(&target, &CancellationToken::new()).await, Err(AuthResolveError::Unavailable)));
    assert_eq!(fake.served(), 1);
}

#[tokio::test]
async fn relogin_and_refresh_fence_loss_only_adopt_persisted_access() {
    // Data, owner, and expiry independently fence the refresh commit.
    for race in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), 0);
        let uncommitted = access("selected", "uncommitted");
        let mut refreshed =
            response(json!({"access_token":uncommitted, "refresh_token":"uncommitted-refresh", "expires_in":3600}));
        refreshed["delay_ms"] = 150.into();
        let fake = upstream(vec![refreshed]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let target = model(format!("http://{}/backend-api", fake.addr));
        let target2 = target.clone();
        let worker = service.clone();
        let resolving = tokio::spawn(async move { worker.resolve(&target2, &CancellationToken::new()).await });
        wait_requests(&fake, 1).await;
        let store = SqliteCredentialStore::open(&path).unwrap();
        let rows = store.list_auth_credentials(Some("openai-codex")).unwrap();
        let row = rows.iter().find(|row| row.id == id).unwrap();
        if race == 2 {
            let original = fields(&path, id);
            let db = rusqlite::Connection::open(&path).unwrap();
            // Acquisition-time now is before expiry; commit-time now is after.
            db.execute(
                "UPDATE auth_credential_refresh_leases SET expires_at_ms=?1 WHERE credential_id=?2",
                rusqlite::params![now_ms() + 25, id],
            )
            .unwrap();
            assert!(matches!(resolving.await.unwrap(), Err(AuthResolveError::Unavailable)));
            service.wait_for_settlement().await;
            assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
            let raw: String =
                db.query_row("SELECT data FROM auth_credentials WHERE id=?1", [id], |row| row.get(0)).unwrap();
            let disabled: Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(disabled["access"], original["access"]);
            assert_eq!(disabled["refresh"], "fixture-refresh-original");
            assert_eq!(
                store.list_disabled_credentials(Some("openai-codex")).unwrap()[0].cause,
                "refresh outcome unknown; login required"
            );
            assert!(matches!(
                service.resolve(&target, &CancellationToken::new()).await,
                Err(AuthResolveError::Unavailable)
            ));
            assert_eq!(fake.served(), 1);
            continue;
        }
        let mut replacement = match &row.credential {
            AuthCredential::OAuth { fields } => fields.clone(),
            _ => unreachable!(),
        };
        let persisted = access("selected", "persisted");
        replacement.insert("access".into(), persisted.clone().into());
        replacement.insert("refresh".into(), "interactive-or-peer-refresh".into());
        replacement.insert("expires".into(), (now_ms() + 3_600_000).into());
        if race == 1 {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "UPDATE auth_credential_refresh_leases SET owner='controlled-peer' WHERE credential_id=?1",
                [id],
            )
            .unwrap();
            assert!(
                store
                    .try_update_auth_credential_if_matches(
                        id,
                        &row.serialized_data,
                        &AuthCredential::oauth(replacement),
                        None
                    )
                    .unwrap()
            );
        } else {
            replacement.insert("authorizedAt".into(), 43.into());
            store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(replacement)).unwrap();
        }
        assert!(resolving.await.unwrap().is_ok());
        let stored = fields(&path, id);
        assert_eq!(stored["access"], persisted);
        assert_eq!(stored["refresh"], "interactive-or-peer-refresh");
        assert_eq!(fake.served(), 1);
    }
}

#[tokio::test]
async fn cancellation_before_dispatch_preserves_credentials_after_dispatch_disables_across_restart() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.db");
    let id = seed(&path, "selected", Some(42), 0);
    let mut refreshed =
        response(json!({"access_token":access("selected", "us"), "refresh_token":"rotated", "expires_in":3600}));
    refreshed["delay_ms"] = 1000.into();
    let fake = upstream(vec![refreshed]).await;
    let service = Arc::new(auth(&path, &fake).await);
    let target = model(format!("http://{}/backend-api", fake.addr));
    let before = CancellationToken::new();
    before.cancel();
    assert!(matches!(service.resolve(&target, &before).await, Err(AuthResolveError::Cancelled)));
    assert_eq!(fields(&path, id)["refresh"], "fixture-refresh-original");
    assert_eq!(fake.served(), 0);
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    let worker = service.clone();
    let target2 = target.clone();
    let resolving = tokio::spawn(async move { worker.resolve(&target2, &cancel2).await });
    wait_requests(&fake, 1).await;
    cancel.cancel();
    assert!(matches!(resolving.await.unwrap(), Err(AuthResolveError::Cancelled)));
    let store = SqliteCredentialStore::open(&path).unwrap();
    assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
    assert_eq!(
        store.list_disabled_credentials(Some("openai-codex")).unwrap()[0].cause,
        "refresh outcome unknown; login required"
    );
    drop(service);
    let reopened = auth(&path, &fake).await;
    assert!(matches!(reopened.resolve(&target, &CancellationToken::new()).await, Err(AuthResolveError::Unavailable)));
    assert_eq!(fake.served(), 1);
}

#[tokio::test]
async fn dropped_resolver_and_truncated_success_do_not_replay_unknown_refresh() {
    for drop_caller in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        seed(&path, "selected", Some(42), 0);
        let script = if drop_caller {
            let mut delayed = response(json!({"access_token":access("selected", "us"), "expires_in":3600}));
            delayed["delay_ms"] = 1000.into();
            delayed
        } else {
            json!({"events":[], "end":"drop"})
        };
        let fake = upstream(vec![script]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let target = model(format!("http://{}/backend-api", fake.addr));
        let worker = service.clone();
        let target2 = target.clone();
        let resolving = tokio::spawn(async move { worker.resolve(&target2, &CancellationToken::new()).await });
        wait_requests(&fake, 1).await;
        if drop_caller {
            resolving.abort();
            let _ = resolving.await;
        } else {
            assert!(matches!(resolving.await.unwrap(), Err(AuthResolveError::Refresh)));
        }
        tokio::time::timeout(Duration::from_secs(3), service.wait_for_settlement())
            .await
            .expect("owned worker settles dropped refresh");
        let store = SqliteCredentialStore::open(&path).unwrap();
        assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
        let reopened = auth(&path, &fake).await;
        assert!(matches!(
            reopened.resolve(&target, &CancellationToken::new()).await,
            Err(AuthResolveError::Unavailable)
        ));
        assert_eq!(fake.served(), 1);
    }
}

#[tokio::test]
async fn oauth_route_and_fixture_endpoints_cannot_be_redirected_to_arbitrary_hosts() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("auth.db");
    seed(&path, "selected", Some(42), now_ms() + 3_600_000);
    let production = OpenAiCodexAuth::open(path.clone(), client()).await.unwrap();
    for endpoint in [
        "https://example.com/backend-api",
        "https://chatgpt.com.evil.test/backend-api",
        "http://chatgpt.com/backend-api",
        "https://chatgpt.com/other",
        "https://user@chatgpt.com/backend-api",
        "https://chatgpt.com/backend-api?destination=evil",
        "https://chatgpt.com/backend-api#fragment",
    ] {
        assert!(matches!(
            production.resolve(&model(endpoint.into()), &CancellationToken::new()).await,
            Err(AuthResolveError::Unavailable)
        ));
    }
    let official = model("https://chatgpt.com/backend-api".into());
    assert!(production.resolve(&official, &CancellationToken::new()).await.is_ok());
    for endpoint in ["https://example.com", "http://example.com", "http://user@127.0.0.1", "http://127.0.0.1/path"] {
        assert!(matches!(
            OpenAiCodexAuth::open_with_endpoints(path.clone(), client(), endpoint).await,
            Err(CodexAuthError::InvalidEndpoint)
        ));
    }
    let mut mismatch = official;
    mismatch.provider = "custom".into();
    assert!(matches!(
        production.resolve(&mismatch, &CancellationToken::new()).await,
        Err(AuthResolveError::Unavailable)
    ));
}

#[tokio::test]
async fn exact_discovery_accounts_keep_row_affinity_refresh_fences_and_unknown_outcome_settlement() {
    let row_fields = |row: &StoredAuthCredential| match &row.credential {
        AuthCredential::OAuth { fields } => Value::Object(fields.clone()),
        _ => panic!("expected an OAuth discovery row"),
    };

    // The older row has no interactive authorizedAt marker. Discovery includes
    // it, whereas normal request auth still selects the latest interactive row.
    // Two service connections resolving that exact id share one refresh grant.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let legacy = seed(&path, "legacy", None, 0);
        let selected = seed(&path, "selected", Some(99), now_ms() + 3_600_000);
        let token = access("legacy", "refreshed-legacy");
        let mut refreshed = response(json!({"access_token":token,"refresh_token":"legacy-rotated","expires_in":3600}));
        refreshed["delay_ms"] = 150.into();
        let fake = upstream(vec![refreshed]).await;
        let first = auth(&path, &fake).await;
        let second = auth(&path, &fake).await;
        assert_eq!(
            first.stored_oauth_snapshot().unwrap().iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![legacy, selected]
        );
        assert_eq!(fake.served(), 0, "snapshot does not refresh credentials");
        let cancel = CancellationToken::new();
        let (a, b) = tokio::join!(
            first.resolve_discovery_account(legacy, &cancel),
            second.resolve_discovery_account(legacy, &cancel)
        );
        for row in [a.unwrap(), b.unwrap()] {
            assert_eq!(row.id, legacy);
            assert_eq!(row_fields(&row)["access"], token);
            assert_eq!(row_fields(&row)["accountId"], "legacy");
            assert!(row_fields(&row).get("authorizedAt").is_none());
        }
        assert_eq!(fake.served(), 1);
        assert_eq!(fields(&path, legacy)["refresh"], "legacy-rotated");
        let target = model(format!("http://{}/backend-api", fake.addr));
        let request = first.resolve(&target, &cancel).await.unwrap();
        assert!(matches!(request.identity(), CredentialIdentity::Stored { id, .. } if *id == selected));
        assert_eq!(fake.served(), 1, "exact discovery did not change latest interactive request selection");
    }

    // Different row ids independently own their grants. Request admission is
    // ordered only to associate the two scripted responses deterministically;
    // both refresh workers remain active while the responses are delayed.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let first_id = seed(&path, "first-account", None, 0);
        let second_id = seed(&path, "second-account", Some(42), 0);
        let first_token = access("first-account", "first-refreshed");
        let second_token = access("second-account", "second-refreshed");
        let mut first_reply = response(json!({"access_token":first_token,"expires_in":3600}));
        let mut second_reply = response(json!({"access_token":second_token,"expires_in":3600}));
        first_reply["delay_ms"] = 150.into();
        second_reply["delay_ms"] = 150.into();
        let fake = upstream(vec![first_reply, second_reply]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let first_worker = service.clone();
        let first =
            tokio::spawn(
                async move { first_worker.resolve_discovery_account(first_id, &CancellationToken::new()).await },
            );
        wait_requests(&fake, 1).await;
        let second_worker = service.clone();
        let second =
            tokio::spawn(
                async move { second_worker.resolve_discovery_account(second_id, &CancellationToken::new()).await },
            );
        wait_requests(&fake, 2).await;
        for (row, id, token) in [
            (first.await.unwrap().unwrap(), first_id, first_token),
            (second.await.unwrap().unwrap(), second_id, second_token),
        ] {
            assert_eq!(row.id, id);
            assert_eq!(row_fields(&row)["access"], token);
            assert_eq!(fields(&path, id)["access"], token);
        }
        assert_eq!(fake.served(), 2);
        service.resolve_discovery_account(first_id, &CancellationToken::new()).await.unwrap();
        service.resolve_discovery_account(second_id, &CancellationToken::new()).await.unwrap();
        assert_eq!(fake.served(), 2, "fresh exact rows do not replay grants");
    }

    // A dropped consumer leaves an owned settlement worker. Only its unchanged
    // expired row is disabled; a fresh latest sibling remains usable for normal
    // requests but must never satisfy the failed exact-id lookup after restart.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let target_id = seed(&path, "non-selected", Some(1), 0);
        let sibling_id = seed(&path, "fresh-sibling", Some(99), now_ms() + 3_600_000);
        let sibling_before = fields(&path, sibling_id);
        let mut reply = response(json!({"access_token":access("non-selected", "uncommitted"),"expires_in":3600}));
        reply["delay_ms"] = 1000.into();
        let fake = upstream(vec![reply]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let worker = service.clone();
        let resolving =
            tokio::spawn(async move { worker.resolve_discovery_account(target_id, &CancellationToken::new()).await });
        wait_requests(&fake, 1).await;
        resolving.abort();
        let _ = resolving.await;
        tokio::time::timeout(Duration::from_secs(3), service.wait_for_settlement())
            .await
            .expect("exact-row worker settles after caller drop");
        let store = SqliteCredentialStore::open(&path).unwrap();
        assert_eq!(
            store.list_auth_credentials(Some("openai-codex")).unwrap().iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![sibling_id]
        );
        let disabled = store.list_disabled_credentials(Some("openai-codex")).unwrap();
        assert_eq!(disabled.len(), 1);
        assert_eq!(disabled[0].id, target_id);
        assert_eq!(disabled[0].cause, "refresh outcome unknown; login required");
        assert_eq!(fields(&path, sibling_id), sibling_before);
        drop(service);
        let reopened = auth(&path, &fake).await;
        assert!(matches!(
            reopened.resolve_discovery_account(target_id, &CancellationToken::new()).await,
            Err(CodexAuthError::LoginRequired)
        ));
        let target = model(format!("http://{}/backend-api", fake.addr));
        let request = reopened.resolve(&target, &CancellationToken::new()).await.unwrap();
        assert!(matches!(request.identity(), CredentialIdentity::Stored { id, .. } if *id == sibling_id));
        assert_eq!(fake.served(), 1, "restart must not replay the unknown exact-row refresh");
    }

    // Reuse the existing raw-data/lease fixture interventions for an exact
    // non-selected row: lease loss cannot adopt a sibling; a peer-written fresh
    // target may be adopted; logging in a different account cannot redirect the
    // target's successful refresh/readback.
    for race in 0..3 {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let target_id = seed(&path, "target-account", None, 0);
        let sibling_id = seed(&path, "fresh-sibling", Some(99), now_ms() + 3_600_000);
        let sibling_before = fields(&path, sibling_id);
        let uncommitted = access("target-account", "network-response");
        let mut reply =
            response(json!({"access_token":uncommitted,"refresh_token":"network-rotated","expires_in":3600}));
        reply["delay_ms"] = 150.into();
        let fake = upstream(vec![reply]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let worker = service.clone();
        let resolving =
            tokio::spawn(async move { worker.resolve_discovery_account(target_id, &CancellationToken::new()).await });
        wait_requests(&fake, 1).await;
        let store = SqliteCredentialStore::open(&path).unwrap();
        let row = store
            .list_auth_credentials(Some("openai-codex"))
            .unwrap()
            .into_iter()
            .find(|row| row.id == target_id)
            .unwrap();
        let original_data = row.serialized_data.clone();
        let peer_token = access("target-account", "persisted-peer");
        if race == 0 {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "UPDATE auth_credential_refresh_leases SET owner='controlled-peer' WHERE credential_id=?1",
                [target_id],
            )
            .unwrap();
        } else if race == 1 {
            let mut replacement = row_fields(&row).as_object().unwrap().clone();
            replacement.insert("access".into(), peer_token.clone().into());
            replacement.insert("refresh".into(), "persisted-peer-refresh".into());
            replacement.insert("expires".into(), (now_ms() + 3_600_000).into());
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute(
                "UPDATE auth_credential_refresh_leases SET owner='controlled-peer' WHERE credential_id=?1",
                [target_id],
            )
            .unwrap();
            assert!(
                store
                    .try_update_auth_credential_if_matches(
                        target_id,
                        &row.serialized_data,
                        &AuthCredential::oauth(replacement),
                        None
                    )
                    .unwrap()
            );
        } else {
            seed(&path, "new-interactive-login", Some(100), now_ms() + 3_600_000);
        }
        let result = resolving.await.unwrap();
        service.wait_for_settlement().await;
        if race == 0 {
            assert!(
                matches!(result, Err(CodexAuthError::LoginRequired)),
                "fresh sibling cannot satisfy an exact-row fence loss"
            );
            let disabled = store.list_disabled_credentials(Some("openai-codex")).unwrap();
            assert_eq!(disabled.len(), 1);
            assert_eq!(disabled[0].id, target_id);
            assert_eq!(disabled[0].cause, "refresh outcome unknown; login required");
            let db = rusqlite::Connection::open(&path).unwrap();
            let raw: String =
                db.query_row("SELECT data FROM auth_credentials WHERE id=?1", [target_id], |row| row.get(0)).unwrap();
            assert_eq!(raw, original_data, "uncommitted network token was not persisted");
            let reopened = auth(&path, &fake).await;
            assert!(matches!(
                reopened.resolve_discovery_account(target_id, &CancellationToken::new()).await,
                Err(CodexAuthError::LoginRequired)
            ));
        } else {
            let resolved = result.unwrap();
            assert_eq!(resolved.id, target_id);
            let expected = if race == 1 { peer_token } else { uncommitted };
            assert_eq!(row_fields(&resolved)["access"], expected);
            assert_eq!(fields(&path, target_id)["access"], expected);
        }
        assert_eq!(fields(&path, sibling_id), sibling_before);
        assert_eq!(fake.served(), 1);
    }
}

// Fixed OMP auth-storage.ts usage refresh/cache families and the actual builtin
// openai-codex.ts GET/null/detail behavior, at HTTP + temporary SQLite seams.
#[tokio::test]
async fn builtin_usage_refresh_cache_and_durable_row_outcomes() {
    // Fresh credentials and a fresh report do not spend a refresh grant.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let before = fields(&path, id);
        let fake = upstream(vec![response(usage_payload("fresh"))]).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let reports = usage_reports(&storage).await;
        assert_eq!(reports.len(), 1);
        assert_eq!(usage_reports(&storage).await, reports);
        assert_eq!(fields(&path, id), before);
        let requests = fake.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["request"], "GET /backend-api/wham/usage HTTP/1.1");
        assert_eq!(requests[0]["headers"]["chatgpt-account-id"], "selected");
    }

    // Owned commits use now < expires, including a minted 30-second token.
    // A zero-lifetime token is committed but cannot authorize a usage GET.
    for lifetime in [3600, 30, 0] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let token = access("minted-account-with-longer-name", "minted-region");
        let mut script =
            vec![response(usage_payload("last-good")), response(json!({"access_token":token,"expires_in":lifetime}))];
        if lifetime > 0 {
            script.push(response(usage_payload("minted")));
        }
        let fake = upstream(script).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        let key = stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() + if lifetime == 0 { -1 } else { 30_000 });
        let reports = usage_reports(&storage).await;
        let stored = fields(&path, id);
        assert_eq!(stored["access"], token);
        assert_eq!(stored["accountId"], "minted-account-with-longer-name");
        assert_eq!(stored["email"], "new@example.com");
        for (field, expected) in [
            ("refresh", json!("fixture-refresh-original")),
            ("authorizedAt", json!(42)),
            ("orgId", json!("original-org")),
            ("orgName", json!("original-plan")),
        ] {
            assert_eq!(stored[field], expected);
        }
        let store = SqliteCredentialStore::open(&path).unwrap();
        assert_eq!(
            store.list_auth_credentials(Some("openai-codex")).unwrap().iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![id]
        );
        assert!(store.list_disabled_credentials(Some("openai-codex")).unwrap().is_empty());
        let requests = fake.requests.lock().await;
        assert_eq!(requests[1]["request"], "POST /oauth/token HTTP/1.1");
        assert!(requests[1]["body"].as_str().unwrap().contains("refresh_token=fixture-refresh-original"));
        if lifetime == 0 {
            assert_eq!(reports, vec![old]);
            assert_eq!(requests.len(), 2, "zero lifetime cannot send a GET or repeat refresh");
            assert!(stored["expires"].as_i64().unwrap() <= now_ms());
        } else {
            assert_eq!(reports.len(), 1);
            assert_eq!(reports[0].raw.as_ref().unwrap()["fixtureTag"], "minted");
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[2]["headers"]["authorization"], format!("<redacted {} chars>", token.len() + 7));
            assert_ne!(requests[2]["headers"]["authorization"], requests[0]["headers"]["authorization"]);
            assert_eq!(requests[2]["headers"]["chatgpt-account-id"], "minted-account-with-longer-name");
        }
        assert_eq!(cached_usage(&path, &key)["value"], serde_json::to_value(&reports[0]).unwrap());
    }

    // Durable definitive removal wraps the original error: this flight may
    // retain SG. The next public poll excludes the disabled account entirely.
    // Transient refresh failures retain the active row and its original grant.
    for (offset, status, message, definitive) in [
        (-1, 400, "invalid_grant", true),
        (30_000, 400, "invalid_grant", true),
        (-1, 503, "temporarily_unavailable", false),
        (30_000, 429, "rate_limit", false),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let mut script = vec![
            response(usage_payload("last-good")),
            json!({"status":status,"body":json!({"error":message}).to_string()}),
        ];
        if offset > 0 {
            script.push(json!({"status":503}));
        }
        let fake = upstream(script).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        let key = stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() + offset);
        let original = fields(&path, id);
        assert_eq!(usage_reports(&storage).await, vec![old.clone()]);
        assert_eq!(cached_usage(&path, &key)["value"], serde_json::to_value(&old).unwrap());
        let store = SqliteCredentialStore::open(&path).unwrap();
        if definitive {
            assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
            assert_eq!(
                store
                    .list_disabled_credentials(Some("openai-codex"))
                    .unwrap()
                    .iter()
                    .map(|row| row.id)
                    .collect::<Vec<_>>(),
                vec![id]
            );
            assert!(usage_reports(&storage).await.is_empty());
        } else {
            assert_eq!(fields(&path, id), original);
            assert!(store.list_disabled_credentials(Some("openai-codex")).unwrap().is_empty());
            assert_eq!(usage_reports(&storage).await, vec![old]);
        }
        assert_eq!(fake.served(), if offset > 0 { 3 } else { 2 });
    }

    // The native builtin returns null for a failed main GET, including 401 and
    // 403. This is distinct from a custom UsageProvider's typed auth error.
    for status in [401, 403, 503] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let original = fields(&path, id);
        let fake = upstream(vec![response(usage_payload("last-good")), json!({"status":status})]).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        let key = stale_warmed_usage(&path, "selected", &old);
        assert_eq!(usage_reports(&storage).await, vec![old.clone()]);
        assert_eq!(cached_usage(&path, &key)["value"], serde_json::to_value(old).unwrap());
        assert_eq!(fields(&path, id), original);
        let requests = fake.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert!(requests.iter().all(|request| request["request"].as_str().unwrap().starts_with("GET ")));
    }

    // Ancillary detail failure preserves the newly fetched main report/count.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let mut payload = usage_payload("main-with-credits");
        payload["rate_limit_reset_credits"] = json!({"available_count":3});
        let fake = upstream(vec![response(payload), json!({"status":403})]).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let reports = usage_reports(&storage).await;
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].raw.as_ref().unwrap()["fixtureTag"], "main-with-credits");
        assert_eq!(reports[0].reset_credits.as_ref().unwrap().available_count, 3.0);
        assert!(reports[0].reset_credits.as_ref().unwrap().credits.is_none());
        let requests = fake.requests.lock().await;
        assert_eq!(requests[1]["request"], "GET /backend-api/wham/rate-limit-reset-credits HTTP/1.1");
        assert_eq!(requests[1]["headers"]["authorization"], requests[0]["headers"]["authorization"]);
    }
}

#[tokio::test]
async fn builtin_usage_peer_unknown_deadline_and_shared_refresh_flights() {
    // A request has already dispatched A's grant before a peer logs into B.
    // The shared worker is pinned to A; exact usage cannot inherit B's receipt.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let token = access_with_email("selected", "pinned-account-refresh", "selected@example.invalid");
        let mut refresh = response(json!({"access_token":token,"refresh_token":"pinned-rotated","expires_in":3600}));
        refresh["delay_ms"] = 300.into();
        let fake =
            upstream(vec![response(usage_payload("last-good")), refresh, response(usage_payload("pinned-A"))]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let storage = usage_storage(service.clone(), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() - 1);
        let row =
            SqliteCredentialStore::open(&path).unwrap().list_auth_credentials(Some("openai-codex")).unwrap().remove(0);
        let worker = service.clone();
        let target = model(format!("http://{}/backend-api", fake.addr));
        let target2 = target.clone();
        let request = tokio::spawn(async move { worker.resolve(&target2, &CancellationToken::new()).await });
        wait_requests(&fake, 2).await;
        let sibling = seed(&path, "new-latest-B", Some(99), now_ms() + 3_600_000);
        let sibling_before = fields(&path, sibling);
        let report = storage
            .usage_report("openai-codex", &row, &AuthRequestContext::default(), false, &CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        let lease = request.await.unwrap().unwrap();
        assert!(matches!(lease.identity(), CredentialIdentity::Stored { id: actual, .. } if *actual == id));
        assert_eq!(report.raw.as_ref().unwrap()["fixtureTag"], "pinned-A");
        assert_eq!(report.metadata.as_ref().unwrap()["accountId"], "selected");
        assert_eq!(fields(&path, id)["access"], token);
        assert_eq!(fields(&path, id)["refresh"], "pinned-rotated");
        assert_eq!(fields(&path, sibling), sibling_before);
        let next = service.resolve(&target, &CancellationToken::new()).await.unwrap();
        assert!(matches!(next.identity(), CredentialIdentity::Stored { id: actual, .. } if *actual == sibling));
        let requests = fake.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1]["request"], "POST /oauth/token HTTP/1.1");
        assert_eq!(requests[2]["headers"]["chatgpt-account-id"], "selected");
        assert_eq!(requests[2]["headers"]["authorization"], format!("<redacted {} chars>", token.len() + 7));
    }

    // After losing the data+lease CAS, only the persisted exact row is usable.
    // A peer's 30-second grant fails the skew test; its 120-second grant passes.
    for peer_lifetime in [30_000, 120_000] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let uncommitted = access("uncommitted-account", "network-result-must-not-be-sent");
        let peer = access_with_email("selected", "persisted-peer", "selected@example.invalid");
        let mut refresh =
            response(json!({"access_token":uncommitted,"refresh_token":"uncommitted-refresh","expires_in":3600}));
        refresh["delay_ms"] = 150.into();
        let mut script = vec![response(usage_payload("last-good")), refresh];
        if peer_lifetime > 60_000 {
            script.push(response(usage_payload("peer")));
        }
        let fake = upstream(script).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        let key = stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() - 1);
        let worker = storage.clone();
        let fetching = tokio::spawn(async move { usage_reports(&worker).await });
        wait_requests(&fake, 2).await;
        let store = SqliteCredentialStore::open(&path).unwrap();
        let row = store.list_auth_credentials(Some("openai-codex")).unwrap().remove(0);
        let mut replacement = fields(&path, id).as_object().unwrap().clone();
        replacement.insert("access".into(), peer.clone().into());
        replacement.insert("refresh".into(), "persisted-peer-refresh".into());
        replacement.insert("expires".into(), (now_ms() + peer_lifetime).into());
        let db = rusqlite::Connection::open(&path).unwrap();
        assert_eq!(
            db.execute(
                "UPDATE auth_credential_refresh_leases SET owner='usage-fixture-peer' WHERE credential_id=?1",
                [id]
            )
            .unwrap(),
            1
        );
        assert!(
            store
                .try_update_auth_credential_if_matches(
                    id,
                    &row.serialized_data,
                    &AuthCredential::oauth(replacement),
                    None
                )
                .unwrap()
        );
        let reports = fetching.await.unwrap();
        assert_eq!(fields(&path, id)["access"], peer);
        assert_eq!(fields(&path, id)["refresh"], "persisted-peer-refresh");
        assert!(store.list_disabled_credentials(Some("openai-codex")).unwrap().is_empty());
        let requests = fake.requests.lock().await;
        if peer_lifetime > 60_000 {
            assert_eq!(reports[0].raw.as_ref().unwrap()["fixtureTag"], "peer");
            assert_eq!(requests.len(), 3);
            assert_eq!(requests[2]["headers"]["authorization"], format!("<redacted {} chars>", peer.len() + 7));
            assert_ne!(requests[2]["headers"]["authorization"], format!("<redacted {} chars>", uncommitted.len() + 7));
            assert_eq!(requests[2]["headers"]["chatgpt-account-id"], "selected");
        } else {
            assert_eq!(reports, vec![old]);
            assert_eq!(requests.len(), 2, "short peer and uncommitted network token cannot authorize a GET");
        }
        assert_eq!(cached_usage(&path, &key)["value"], serde_json::to_value(&reports[0]).unwrap());
    }

    // Advisory unknown preserves the row but durably fences that exact grant.
    // Neither a usage epoch reset nor process-style reopening may replay it.
    {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let fake = upstream(vec![response(usage_payload("last-good")), json!({"events":[],"end":"drop"})]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let storage = usage_storage(service.clone(), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        let key = stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() - 1);
        let before = fields(&path, id);
        assert_eq!(usage_reports(&storage).await, vec![old.clone()]);
        tokio::time::timeout(Duration::from_secs(3), service.wait_for_settlement()).await.unwrap();
        assert_eq!(fields(&path, id), before);
        let marker = || {
            let db = rusqlite::Connection::open(&path).unwrap();
            let mut query = db.prepare("SELECT key,value,expires_at FROM cache WHERE key LIKE 'oauth-refresh-pending:openai-codex:%' ORDER BY key").unwrap();
            query
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let pending = marker();
        assert_eq!(pending.len(), 1);
        assert!(!pending[0].1.is_empty());
        assert!(pending[0].2 > now_ms() / 1000);
        assert_eq!(cached_usage(&path, &key)["value"], serde_json::to_value(old).unwrap());
        storage.invalidate_usage_cache(Some("openai-codex")).unwrap();
        assert!(usage_reports(&storage).await.is_empty());
        assert_eq!(fields(&path, id), before);
        assert_eq!(marker(), pending);
        drop(storage);
        drop(service);
        let reopened = Arc::new(auth(&path, &fake).await);
        let restarted = usage_storage(reopened, &fake, Duration::from_secs(2));
        assert!(usage_reports(&restarted).await.is_empty());
        assert_eq!(fields(&path, id), before);
        assert_eq!(marker(), pending);
        assert!(
            SqliteCredentialStore::open(&path)
                .unwrap()
                .list_disabled_credentials(Some("openai-codex"))
                .unwrap()
                .is_empty()
        );
        let requests = fake.requests.lock().await;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1]["request"], "POST /oauth/token HTTP/1.1");
    }

    // Each stage is shorter than the budget, but their sum exceeds it. The
    // public operation must retain SG within one refresh+GET/detail deadline.
    for detail_stage in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let token = access("selected", "deadline-refreshed");
        let mut refresh = response(json!({"access_token":token,"expires_in":3600}));
        refresh["delay_ms"] = 200.into();
        let mut payload = usage_payload("too-late");
        if detail_stage {
            payload["rate_limit_reset_credits"] = json!({"available_count":1});
        }
        let mut main = response(payload);
        if !detail_stage {
            main["delay_ms"] = 200.into();
        }
        let mut script = vec![response(usage_payload("last-good")), refresh, main];
        if detail_stage {
            script.push(json!({"delay_ms":200,"body":json!({"credits":[],"available_count":0}).to_string()}));
        }
        let fake = upstream(script).await;
        let storage = usage_storage(Arc::new(auth(&path, &fake).await), &fake, Duration::from_millis(300));
        let old = usage_reports(&storage).await.remove(0);
        stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() - 1);
        assert_eq!(tokio::time::timeout(Duration::from_secs(2), usage_reports(&storage)).await.unwrap(), vec![old]);
        assert_eq!(fields(&path, id)["access"], token, "completed refresh committed before the total deadline");
        assert_eq!(fake.served(), if detail_stage { 4 } else { 3 });
    }

    // One usage and one request consumer share the same OAuth worker in both
    // arrival orders. Cancelling either consumer cannot abort the other.
    for usage_first in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("auth.db");
        let id = seed(&path, "selected", Some(42), now_ms() + 3_600_000);
        let token = access_with_email("selected", "shared-refresh", "selected@example.invalid");
        let mut refresh = response(json!({"access_token":token,"refresh_token":"shared-rotated","expires_in":3600}));
        refresh["delay_ms"] = 250.into();
        let fake =
            upstream(vec![response(usage_payload("last-good")), refresh, response(usage_payload("shared"))]).await;
        let service = Arc::new(auth(&path, &fake).await);
        let storage = usage_storage(service.clone(), &fake, Duration::from_secs(2));
        let old = usage_reports(&storage).await.remove(0);
        stale_warmed_usage(&path, "selected", &old);
        replace_expiry(&path, id, now_ms() - 1);
        let usage_cancel = CancellationToken::new();
        let request_cancel = CancellationToken::new();
        let start_usage = || {
            let storage = storage.clone();
            let cancel = usage_cancel.clone();
            tokio::spawn(async move {
                storage.fetch_usage_reports(Some("openai-codex"), &AuthRequestContext::default(), &cancel).await
            })
        };
        let start_request = || {
            let service = service.clone();
            let cancel = request_cancel.clone();
            let target = model(format!("http://{}/backend-api", fake.addr));
            tokio::spawn(async move { service.resolve(&target, &cancel).await })
        };
        let (usage, request) = if usage_first {
            let usage = start_usage();
            wait_requests(&fake, 2).await;
            (usage, start_request())
        } else {
            let request = start_request();
            wait_requests(&fake, 2).await;
            (start_usage(), request)
        };
        // Both public consumers enter while the controlled POST stays pending.
        tokio::time::sleep(Duration::from_millis(25)).await;
        if usage_first {
            usage_cancel.cancel();
            assert!(matches!(usage.await.unwrap(), Err(AuthStorageError::Cancelled)));
            let lease = request.await.unwrap().unwrap();
            assert!(matches!(lease.identity(), CredentialIdentity::Stored { id: actual, .. } if *actual == id));
            tokio::time::timeout(Duration::from_secs(3), storage.wait_for_settlement()).await.unwrap();
            assert_eq!(usage_reports(&storage).await[0].raw.as_ref().unwrap()["fixtureTag"], "shared");
        } else {
            request_cancel.cancel();
            assert!(matches!(request.await.unwrap(), Err(AuthResolveError::Cancelled)));
            assert_eq!(usage.await.unwrap().unwrap()[0].raw.as_ref().unwrap()["fixtureTag"], "shared");
        }
        assert_eq!(fields(&path, id)["access"], token);
        assert_eq!(fields(&path, id)["refresh"], "shared-rotated");
        assert!(
            SqliteCredentialStore::open(&path)
                .unwrap()
                .list_disabled_credentials(Some("openai-codex"))
                .unwrap()
                .is_empty()
        );
        let requests = fake.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests.iter().filter(|request| request["request"].as_str().unwrap().starts_with("POST ")).count(),
            1
        );
        assert_eq!(requests[2]["headers"]["authorization"], format!("<redacted {} chars>", token.len() + 7));
    }
}
