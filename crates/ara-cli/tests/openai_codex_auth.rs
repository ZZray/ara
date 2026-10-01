//! Complete host auth flows against a controlled local HTTP upstream and real
//! temporary SQLite. These do not claim real OpenAI authorization acceptance.
#![cfg(feature = "test-fixture")]

use ara_ai::{CallOptions, Context, Message, Model, StopReason, UserMessage};
use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
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
    jwt(json!({"https://api.openai.com/auth": {
        "chatgpt_account_id": account, "chatgpt_data_residency": residency,
        "chatgpt_plan_type": "new-plan",
    }, "https://api.openai.com/profile": {"email": " NEW@Example.COM "}}))
}

fn model(base_url: String) -> Model {
    Model {
        id: "gpt-5.3-codex".into(),
        api: "openai-codex-responses".into(),
        provider: "openai-codex".into(),
        base_url,
        reasoning: false,
        max_tokens: None,
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
    let mut fields = json!({
        "access": access(account, "old-region"), "refresh": "fixture-refresh-original", "expires": expires,
        "accountId": account, "email": "old@example.com", "orgId": "original-org", "orgName": "original-plan",
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
