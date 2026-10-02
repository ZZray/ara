//! Complete host auth flows against a controlled local HTTP upstream and real
//! temporary SQLite. These do not claim real OpenAI authorization acceptance.
#![cfg(feature = "test-fixture")]

use ara_ai::{CallOptions, Context, Message, Model, StopReason, UserMessage};
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
