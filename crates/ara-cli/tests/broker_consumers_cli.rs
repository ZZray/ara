//! Actual auth commands against synthetic device/Broker endpoints, with the
//! Broker's temporary SQLite as authority. No real account credentials are used.
#![cfg(feature = "test-fixture")]

#[path = "support/broker_consumers.rs"]
mod broker_consumers;

use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
use ara_testkit::{FakeUpstream, Script};
use base64::Engine as _;
use broker_consumers::{BrokerFixture, snapshot};
use serde_json::{Value, json};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PROVIDER: &str = "openai-codex";

fn synthetic_token(suffix: &str) -> String {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        json!({"https://api.openai.com/auth":{"chatgpt_account_id":"consumer-account"},
            "https://api.openai.com/profile":{"email":"consumer@example.invalid"}})
        .to_string(),
    );
    format!("e30.{payload}.{suffix}")
}

async fn device(token: &str) -> FakeUpstream {
    let responses = json!([
        {"body":json!({"device_auth_id":"consumer-device","user_code":"CONSUMER-CODE","interval":1}).to_string()},
        {"body":json!({"authorization_code":"consumer-code","code_verifier":"consumer-verifier"}).to_string()},
        {"body":json!({"access_token":token,"refresh_token":"consumer-refresh","expires_in":3600}).to_string()}
    ]);
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn entries(store: &SqliteCredentialStore, provider: Option<&str>) -> Vec<Value> {
    store
        .list_auth_credentials(provider)
        .unwrap()
        .into_iter()
        .map(|row| {
            let mut credential = serde_json::to_value(row.credential).unwrap();
            // Broker responses redact the server-owned refresh grant; uploads
            // still carry the issued credential to the authority above.
            if credential["type"] == "oauth" {
                credential["refresh"] = json!(ara_cli::auth_broker_wire::REMOTE_REFRESH_SENTINEL);
            }
            json!({"id":row.id,"provider":row.provider,"credential":credential,
            "identityKey":null})
        })
        .collect()
}

async fn broker(store: Arc<Mutex<SqliteCredentialStore>>, failure: Arc<AtomicU8>) -> BrokerFixture {
    BrokerFixture::start(move |request| {
        let store = store.lock().unwrap();
        match (request.method.as_str(), request.path.split('?').next().unwrap()) {
            ("GET", "/v1/snapshot") => {
                let rows = entries(&store, None)
                    .into_iter()
                    .map(|mut row| {
                        row["rotatesInMs"] = Value::Null;
                        row
                    })
                    .collect();
                (200, snapshot(3, rows))
            }
            ("POST", "/v1/credential") => {
                assert_eq!(request.body["provider"], PROVIDER);
                let credential: AuthCredential = serde_json::from_value(request.body["credential"].clone()).unwrap();
                store.upsert_auth_credential_for_provider(PROVIDER, &credential).unwrap();
                if failure.load(Ordering::Acquire) == 1 {
                    // Mutation committed, but the client's result is uncertain.
                    (500, json!({"error":"synthetic post-commit failure"}))
                } else {
                    (200, json!({"entries":entries(&store, Some(PROVIDER))}))
                }
            }
            ("POST", path) if path.starts_with("/v1/credential/") && path.ends_with("/disable") => {
                assert_eq!(request.body["cause"], "deleted by user");
                let id: i64 = path.trim_start_matches("/v1/credential/").trim_end_matches("/disable").parse().unwrap();
                store.delete_auth_credential(id, "deleted by user").unwrap();
                if failure.load(Ordering::Acquire) == 2 {
                    (500, json!({"error":"synthetic post-disable failure"}))
                } else {
                    (200, json!({"ok":true}))
                }
            }
            _ => (404, json!({})),
        }
    })
    .await
}

fn command(home: &std::path::Path, work: &std::path::Path, broker: &BrokerFixture, verb: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
    command.env_clear();
    for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .current_dir(work)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("ARA_HOME", home)
        .env("TEMP", home)
        .env("TMP", home)
        .env("ARA_AUTH_BROKER_URL", &broker.url)
        .env("ARA_AUTH_BROKER_TOKEN", "synthetic-consumer-bearer")
        .env("ARA_AUTH_BROKER_SNAPSHOT_TTL_MS", "0")
        .args([verb, PROVIDER])
        .stdin(Stdio::null());
    command
}

async fn output(command: Command) -> Output {
    tokio::time::timeout(Duration::from_secs(20), tokio::process::Command::from(command).kill_on_drop(true).output())
        .await
        .expect("auth command exceeded its deadline")
        .unwrap()
}

fn seeded_store(path: &std::path::Path) -> Arc<Mutex<SqliteCredentialStore>> {
    let store = SqliteCredentialStore::open(path).unwrap();
    let other: AuthCredential = serde_json::from_value(json!({"type":"oauth","access":"other-account-access",
        "refresh":"other-account-refresh","expires":4_000_000_000_000_i64,"accountId":"other-account",
        "email":"other@example.invalid"}))
    .unwrap();
    store.upsert_auth_credential_for_provider(PROVIDER, &other).unwrap();
    store.upsert_auth_credential_for_provider(PROVIDER, &AuthCredential::api_key("legacy-consumer-key")).unwrap();
    store.upsert_auth_credential_for_provider("unrelated", &AuthCredential::api_key("unrelated-consumer-key")).unwrap();
    Arc::new(Mutex::new(store))
}

fn assert_private(output: &Output, token: &str, home: &std::path::Path) {
    for bytes in [&output.stdout, &output.stderr] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains(token));
        assert!(!text.contains("consumer-refresh"));
    }
    assert!(!home.join("agent/auth.db").exists(), "Remote commands must not mirror grants into Local SQLite");
}

#[tokio::test]
async fn remote_login_replace_logout_preserves_other_accounts_and_has_no_local_mirror() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let authority = tempfile::tempdir().unwrap();
    let store = seeded_store(&authority.path().join("broker.db"));
    let failure = Arc::new(AtomicU8::new(0));
    let broker = broker(store.clone(), failure).await;
    std::fs::create_dir_all(home.path().join("agent")).unwrap();
    // Auth commands are independent of unrelated model configuration failures.
    std::fs::write(home.path().join("agent/models.yml"), "providers: [").unwrap();
    let mut registered_id = None;
    for suffix in ["first", "replacement"] {
        let token = synthetic_token(suffix);
        let device = device(&token).await;
        let mut login = command(home.path(), work.path(), &broker, "login");
        login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", device.addr));
        let result = output(login).await;
        assert_eq!(result.status.code(), Some(0), "{}", String::from_utf8_lossy(&result.stderr));
        assert!(String::from_utf8_lossy(&result.stderr).contains("CONSUMER-CODE"));
        assert_private(&result, &token, home.path());
        assert_eq!(device.served(), 3);
        let rows = entries(&store.lock().unwrap(), Some(PROVIDER));
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row["credential"]["type"] == "oauth"));
        assert!(rows.iter().any(|row| row["credential"]["accountId"] == "other-account"));
        let registered = rows.iter().find(|row| row["credential"]["accountId"] == "consumer-account").unwrap();
        assert_eq!(registered["credential"]["access"], token);
        assert!(registered["credential"]["authorizedAt"].as_i64().unwrap() > 0);
        if let Some(id) = registered_id {
            assert_eq!(registered["id"], id);
        }
        registered_id = Some(registered["id"].clone());
    }
    let result = output(command(home.path(), work.path(), &broker, "logout")).await;
    assert_eq!(result.status.code(), Some(0), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(entries(&store.lock().unwrap(), Some(PROVIDER)).is_empty());
    assert_eq!(entries(&store.lock().unwrap(), Some("unrelated")).len(), 1);
    assert_private(&result, &synthetic_token("replacement"), home.path());
    let requests = broker.requests.lock().unwrap();
    assert_eq!(
        requests.iter().filter(|request| request.method == "POST" && request.path == "/v1/credential").count(),
        2
    );
    assert_eq!(requests.iter().filter(|request| request.path.ends_with("/disable")).count(), 2);
}

#[tokio::test]
async fn uncertain_remote_login_and_logout_do_not_report_success_or_replay_mutations() {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let authority = tempfile::tempdir().unwrap();
    let store = seeded_store(&authority.path().join("broker.db"));
    let failure = Arc::new(AtomicU8::new(1));
    let broker = broker(store.clone(), failure.clone()).await;
    let token = synthetic_token("uncertain");
    let authorization = device(&token).await;
    let mut login = command(home.path(), work.path(), &broker, "login");
    login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", authorization.addr));
    let result = output(login).await;
    assert_eq!(result.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("signed in"));
    assert_private(&result, &token, home.path());
    assert_eq!(authorization.served(), 3);
    assert_eq!(
        entries(&store.lock().unwrap(), Some(PROVIDER)).len(),
        2,
        "inspect committed state after uncertain response"
    );
    failure.store(2, Ordering::Release);
    let result = output(command(home.path(), work.path(), &broker, "logout")).await;
    assert_eq!(result.status.code(), Some(2));
    assert!(!String::from_utf8_lossy(&result.stderr).contains("signed out"));
    assert_private(&result, &token, home.path());
    assert_eq!(
        entries(&store.lock().unwrap(), Some(PROVIDER)).len(),
        1,
        "unknown first disable stops the provider batch"
    );
    {
        let requests = broker.requests.lock().unwrap();
        assert_eq!(
            requests.iter().filter(|request| request.method == "POST" && request.path == "/v1/credential").count(),
            1
        );
        assert_eq!(requests.iter().filter(|request| request.path.ends_with("/disable")).count(), 1);
    }
    let refused = BrokerFixture::start(|_| (401, json!({"error":"synthetic bearer rejected"}))).await;
    let uninvoked = device(&token).await;
    let mut login = command(home.path(), work.path(), &refused, "login");
    login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", uninvoked.addr));
    let result = output(login).await;
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(uninvoked.served(), 0, "configured Broker failure must stop before issuing another grant");
    assert_private(&result, &token, home.path());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("signed in"));
}
