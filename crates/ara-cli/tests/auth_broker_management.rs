//! Grouped management acceptance, including actual child lifecycle and durable
//! unknown-grant fencing. All account/token endpoints are synthetic loopback.
#![cfg(feature = "test-fixture")]
use ara_cli::{
    auth_broker_discover::{BrokerConfigResolver, BrokerDiscoveryHost},
    auth_broker_management::{
        BrokerArgs, BrokerCommand, BrokerConsole, BrokerDeviceLoginPort, BrokerLocalAuthFactory, BrokerManagementError,
        BrokerManagementHost, run_auth_broker_command,
    },
    auth_broker_server::{AuthBrokerServerOptions, start_auth_broker},
    auth_storage::{AuthStorage, Environment, EnvironmentKey},
    credential_store::{AuthCredential, SqliteCredentialStore},
    model_config_values::ConfigValueEnvironment,
};
use ara_testkit::{FakeUpstream, Script};
use async_trait::async_trait;
use base64::Engine;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    future::Future,
    path::Path,
    process::{Command, Output, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(15), future).await.expect("bounded management family")
}
fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}
fn oauth(extra: Value) -> AuthCredential {
    let mut fields=json!({"access":"fixture-local-access","refresh":"fixture-local-refresh","expires":4102444800000_f64,"email":"fixture@example.invalid"}).as_object().unwrap().clone();
    fields.extend(extra.as_object().unwrap().clone());
    AuthCredential::oauth(fields)
}
#[derive(Default)]
struct Env(Mutex<BTreeMap<String, String>>);
impl Env {
    fn set(&self, name: &str, value: &str) {
        self.0.lock().unwrap().insert(name.into(), value.into());
    }
}
impl ConfigValueEnvironment for Env {
    fn get(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().get(name).cloned()
    }
}
impl Environment for Env {
    fn api_key(&self, provider: &str) -> Option<EnvironmentKey> {
        let name = match provider {
            "deepseek" => "DEEPSEEK_API_KEY",
            "openai" => "OPENAI_API_KEY",
            _ => return None,
        };
        ConfigValueEnvironment::get(self, name).map(|value| EnvironmentKey { variable: name.into(), value })
    }
    fn variable(&self, name: &str) -> Option<String> {
        ConfigValueEnvironment::get(self, name)
    }
}
#[derive(Default)]
struct Console(Mutex<String>);
#[async_trait]
impl BrokerConsole for Console {
    fn write(&self, text: &str) -> Result<(), BrokerManagementError> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
    async fn prompt(&self, _text: &str, _cancel: &CancellationToken) -> Result<String, BrokerManagementError> {
        Ok("1 trailing-native-parseInt".into())
    }
}
struct Factory {
    storage: Arc<AuthStorage>,
    calls: AtomicUsize,
}
#[async_trait]
impl BrokerLocalAuthFactory for Factory {
    async fn open(&self) -> Result<Arc<AuthStorage>, BrokerManagementError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(self.storage.clone())
    }
}
struct Device {
    calls: AtomicUsize,
}
#[async_trait]
impl BrokerDeviceLoginPort for Device {
    async fn issue(&self, _cancel: &CancellationToken) -> Result<AuthCredential, BrokerManagementError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        Ok(oauth(json!({"accountId":"fixture-local-account"})))
    }
}
fn make_host(
    root: &Path,
    env: Arc<Env>,
    storage: Arc<AuthStorage>,
) -> (BrokerManagementHost, Arc<Factory>, Arc<Console>, Arc<Device>) {
    let factory = Arc::new(Factory { storage, calls: AtomicUsize::new(0) });
    let console = Arc::new(Console::default());
    let device = Arc::new(Device { calls: AtomicUsize::new(0) });
    (
        BrokerManagementHost {
            config_root: root.into(),
            version: "fixture-version".into(),
            client: client(),
            clock: Arc::new(|| chrono::Utc::now().timestamp_millis() as f64),
            config: BrokerConfigResolver::new(BrokerDiscoveryHost {
                config_root: root.into(),
                agent_dir: root.join("agent"),
                environment: env.clone(),
                config_value_resolver: None,
            }),
            local_auth: factory.clone(),
            device_login: device.clone(),
            environment: env,
            console: console.clone(),
            callback_ports: BTreeMap::new(),
        },
        factory,
        console,
        device,
    )
}
async fn run(command: BrokerCommand, host: &BrokerManagementHost, retained: &mut Option<Arc<AuthStorage>>) -> i32 {
    bounded(run_auth_broker_command(BrokerArgs { command: Some(command) }, host, retained, &CancellationToken::new()))
        .await
        .unwrap()
}
fn make_storage(path: &Path) -> (Arc<Mutex<SqliteCredentialStore>>, Arc<AuthStorage>) {
    let store = Arc::new(Mutex::new(SqliteCredentialStore::open(path).unwrap()));
    let auth = Arc::new(AuthStorage::new(store.clone(), None, Default::default()).unwrap());
    (store, auth)
}
#[tokio::test]
async fn management_lazy_import_local_login_and_org_migration_family() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("lazy-home");
    let (_store, auth) = make_storage(&temp.path().join("fixture.db"));
    let env = Arc::new(Env::default());
    let (host, factory, console, device) = make_host(&root, env.clone(), auth.clone());
    let mut retained = None;
    assert_eq!(run(BrokerCommand::List { json: true }, &host, &mut retained).await, 0);
    assert_eq!(run(BrokerCommand::Status { json: true }, &host, &mut retained).await, 0);
    assert_eq!(factory.calls.load(Ordering::Acquire), 0);
    assert!(!root.join("agent/auth.db").exists());
    run(BrokerCommand::Token { regenerate: false, json: true }, &host, &mut retained).await;
    let first = tokio::fs::read_to_string(root.join("auth-broker.token")).await.unwrap();
    assert_eq!(first.len(), 43);
    assert_eq!(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(&first).unwrap().len(), 32);
    run(BrokerCommand::Token { regenerate: false, json: true }, &host, &mut retained).await;
    assert_eq!(first, tokio::fs::read_to_string(root.join("auth-broker.token")).await.unwrap());
    run(BrokerCommand::Token { regenerate: true, json: true }, &host, &mut retained).await;
    assert_ne!(first, tokio::fs::read_to_string(root.join("auth-broker.token")).await.unwrap());
    assert_eq!(factory.calls.load(Ordering::Acquire), 0);
    let imports = temp.path().join("imports");
    tokio::fs::create_dir(&imports).await.unwrap();
    tokio::fs::create_dir(imports.join("nested")).await.unwrap();
    for (name, body) in [
        (
            "a-codex.json",
            json!({"type":"codex","access_token":"import-access","refresh_token":"import-refresh","expired":"2099-12-31T23:59:59+08:00","email":"import@example.invalid","account_id":"import-account"}),
        ),
        (
            "b-disabled.json",
            json!({"type":"claude","disabled":true,"access_token":"disabled-access","refresh_token":"disabled-refresh","expired":"2099-12-31T23:59:59Z"}),
        ),
        (
            "gemini-fallback.json",
            json!({"type":"unrecognized","access_token":"gemini-access","refresh_token":"gemini-refresh","expired":"2099-12-31T23:59:59Z"}),
        ),
        ("bad-expiry.json", json!({"type":"codex","access_token":"bad","refresh_token":"bad","expired":"invalid"})),
    ] {
        tokio::fs::write(imports.join(name), body.to_string()).await.unwrap();
    }
    tokio::fs::write(imports.join("nested/ignored.json"), "{}").await.unwrap();
    env.set("ARA_AUTH_BROKER_URL", "http://127.0.0.1:1");
    env.set("ARA_AUTH_BROKER_TOKEN", "fixture-unused");
    console.0.lock().unwrap().clear();
    run(
        BrokerCommand::Import {
            source: imports.clone(),
            provider: None,
            include_disabled: false,
            dry_run: true,
            json: true,
        },
        &host,
        &mut retained,
    )
    .await;
    let output: Value = serde_json::from_str(console.0.lock().unwrap().trim()).unwrap();
    assert_eq!(output["plan"].as_array().unwrap().len(), 2);
    assert_eq!(output["skipped"].as_array().unwrap().len(), 2);
    assert_eq!(factory.calls.load(Ordering::Acquire), 0);
    assert!(!output.to_string().contains("import-refresh"));
    assert!(!root.join("agent/auth.db").exists());
    run(
        BrokerCommand::Login {
            provider: None,
            provider_flag: Some("openai-codex".into()),
            via: None,
            dry_run: false,
            json: false,
        },
        &host,
        &mut retained,
    )
    .await;
    assert_eq!(factory.calls.load(Ordering::Acquire), 1);
    assert_eq!(device.calls.load(Ordering::Acquire), 1);
    assert!(Arc::ptr_eq(retained.as_ref().unwrap(), &auth));
    run(
        BrokerCommand::Logout {
            provider: Some("openai-codex".into()),
            provider_flag: Some("ignored-positional-wins".into()),
            json: false,
        },
        &host,
        &mut retained,
    )
    .await;
    assert!(!auth.has_auth("openai-codex").unwrap());
    assert!(
        run_auth_broker_command(
            BrokerArgs {
                command: Some(BrokerCommand::Login {
                    provider: Some("openai-codex".into()),
                    provider_flag: None,
                    via: Some("fixture@host".into()),
                    dry_run: true,
                    json: false
                })
            },
            &host,
            &mut retained,
            &CancellationToken::new()
        )
        .await
        .err()
        .unwrap()
        .to_string()
        .contains("Use device-code flow")
    );
    // Real remote Broker + actual local store: org-qualified OAuth duplicates,
    // one API key per provider, local before env, sentinels, dry-run and rerun.
    let (remote_store, remote_auth) = make_storage(&temp.path().join("remote.db"));
    remote_store
        .lock()
        .unwrap()
        .upsert_auth_credential_for_provider("anthropic", &oauth(json!({"orgId":"org-a","accountId":"org-account"})))
        .unwrap();
    remote_auth.reload().unwrap();
    let mut options = AuthBrokerServerOptions::new(remote_auth.clone(), vec!["remote-token".into()]);
    options.bind = "127.0.0.1:0".into();
    options.disable_refresher = true;
    let broker = start_auth_broker(options).await.unwrap();
    let (local_store, local_auth) = make_storage(&temp.path().join("migration-local.db"));
    {
        let store = local_store.lock().unwrap();
        for org in ["org-a", "org-b"] {
            store
                .upsert_auth_credential_for_provider(
                    "anthropic",
                    &oauth(json!({"orgId":org,"accountId":"org-account"})),
                )
                .unwrap();
        }
        store
            .replace_auth_credentials_for_provider(
                "deepseek",
                &[AuthCredential::api_key("local-first"), AuthCredential::api_key("local-second")],
            )
            .unwrap();
        store
            .upsert_auth_credential_for_provider("amazon-bedrock", &AuthCredential::api_key("<authenticated>"))
            .unwrap();
    }
    local_auth.reload().unwrap();
    let env = Arc::new(Env::default());
    env.set("ARA_AUTH_BROKER_URL", broker.url());
    env.set("ARA_AUTH_BROKER_TOKEN", "remote-token");
    env.set("DEEPSEEK_API_KEY", "env-should-lose");
    env.set("OPENAI_API_KEY", "env-openai-key");
    let (host, _factory, console, _device) =
        make_host(&temp.path().join("migration-home"), env.clone(), local_auth.clone());
    let mut retained = None;
    assert!(
        run_auth_broker_command(
            BrokerArgs {
                command: Some(BrokerCommand::Migrate {
                    from_local: false,
                    include_env: true,
                    include_oauth: true,
                    dry_run: true,
                    json: true
                })
            },
            &host,
            &mut retained,
            &CancellationToken::new()
        )
        .await
        .is_err()
    );
    console.0.lock().unwrap().clear();
    run(
        BrokerCommand::Migrate { from_local: true, include_env: true, include_oauth: true, dry_run: true, json: true },
        &host,
        &mut retained,
    )
    .await;
    let output: Value = serde_json::from_str(console.0.lock().unwrap().trim()).unwrap();
    assert_eq!(output["plan"].as_array().unwrap().len(), 3);
    assert!(output["skipped"].as_array().unwrap().iter().any(|skip| skip["reason"] == "already on broker"));
    assert_eq!(remote_store.lock().unwrap().list_auth_credentials(None).unwrap().len(), 1);
    assert!(!output.to_string().contains("local-first"));
    assert!(!output.to_string().contains("env-openai-key"));
    run(
        BrokerCommand::Migrate { from_local: true, include_env: true, include_oauth: true, dry_run: false, json: true },
        &host,
        &mut retained,
    )
    .await;
    assert_eq!(remote_store.lock().unwrap().list_auth_credentials(None).unwrap().len(), 4);
    let keys = remote_store.lock().unwrap().list_auth_credentials(Some("deepseek")).unwrap();
    assert!(matches!(&keys[0].credential,AuthCredential::ApiKey {key,..} if key=="local-first"));
    console.0.lock().unwrap().clear();
    run(
        BrokerCommand::Migrate { from_local: true, include_env: true, include_oauth: true, dry_run: true, json: true },
        &host,
        &mut retained,
    )
    .await;
    let output: Value = serde_json::from_str(console.0.lock().unwrap().trim()).unwrap();
    assert!(output["plan"].as_array().unwrap().is_empty());
    broker.close().await.unwrap();
    local_auth.close_and_wait().await;
    remote_auth.close_and_wait().await;
}
fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
    command.env_clear();
    for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
        .current_dir(root)
        .env("HOME", root)
        .env("USERPROFILE", root)
        .env("ARA_HOME", root)
        .env("TEMP", root)
        .env("TMP", root)
        .stdin(Stdio::null());
    command
}
async fn output(command: Command) -> Output {
    bounded(tokio::process::Command::from(command).kill_on_drop(true).output()).await.unwrap()
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
struct RunningBroker {
    child: tokio::process::Child,
    url: String,
    token: String,
    _stdout: BufReader<tokio::process::ChildStdout>,
}
async fn serve(root: &Path) -> RunningBroker {
    let mut command = command(root);
    command
        .args(["auth-broker", "serve", "--bind", "127.0.0.1:0"])
        .env("ARA_AUTH_BROKER_URL", "http://127.0.0.1:1")
        .env("ARA_AUTH_BROKER_TOKEN", "must-not-control-local-serve")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    bounded(stdout.read_line(&mut line)).await.unwrap();
    let first: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(first["event"], "auth-broker listening");
    let url = first["url"].as_str().unwrap().to_owned();
    line.clear();
    bounded(stdout.read_line(&mut line)).await.unwrap();
    let token = tokio::fs::read_to_string(root.join("auth-broker.token")).await.unwrap();
    RunningBroker { child, url, token, _stdout: stdout }
}
fn interrupt(child: &tokio::process::Child) {
    #[cfg(unix)]
    {
        assert_eq!(unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGTERM) }, 0);
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
        assert_ne!(
            unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id().unwrap()) },
            0,
            "broker Ctrl+Break failed"
        );
    }
}
#[tokio::test]
async fn management_actual_process_login_serve_normal_exit_and_hardkill_family() {
    let root = tempfile::tempdir().unwrap();
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "status", "--json"]);
    let result = output(cmd).await;
    assert!(result.status.success());
    assert_eq!(serde_json::from_slice::<Value>(&result.stdout).unwrap()["reason"], "not_configured");
    assert!(!root.path().join("agent/auth.db").exists());
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "list", "--json"]);
    assert!(output(cmd).await.status.success());
    assert!(!root.path().join("agent/auth.db").exists());
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "token", "--json"]);
    let token_output = output(cmd).await;
    assert!(token_output.status.success());
    let token_receipt: Value = serde_json::from_slice(&token_output.stdout).unwrap();
    assert_eq!(token_receipt["token"].as_str().unwrap().len(), 43);
    assert!(root.path().join("auth-broker.token").exists());
    assert!(!root.path().join("agent/auth.db").exists());
    let import_file = root.path().join("codex-process-import.json");
    tokio::fs::write(&import_file, json!({"type":"codex","access_token":"fixture-import-access","refresh_token":"fixture-import-refresh","expired":"2099-01-01T00:00:00Z"}).to_string()).await.unwrap();
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "import"])
        .arg(&import_file)
        .args(["--dry-run", "--json"])
        .env("ARA_AUTH_BROKER_URL", "http://127.0.0.1:1")
        .env("ARA_AUTH_BROKER_TOKEN", "unused");
    let import_output = output(cmd).await;
    assert!(import_output.status.success());
    let import_plan: Value = serde_json::from_slice(&import_output.stdout).unwrap();
    assert_eq!(import_plan["plan"].as_array().unwrap().len(), 1);
    assert!(!String::from_utf8_lossy(&import_output.stdout).contains("fixture-import-refresh"));
    assert!(!root.path().join("agent/auth.db").exists());
    let payload=base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json!({"https://api.openai.com/auth":{"chatgpt_account_id":"process-account"},"https://api.openai.com/profile":{"email":"process@example.invalid"}}).to_string());
    let access = format!("e30.{payload}.fixture");
    let fake = fake_upstream(vec![
        json!({"body":json!({"device_auth_id":"process-device","user_code":"PROCESS-CODE","interval":1}).to_string()}),
        json!({"body":json!({"authorization_code":"process-code","code_verifier":"process-verifier"}).to_string()}),
        json!({"body":json!({"access_token":access,"refresh_token":"process-refresh","expires_in":3600}).to_string()}),
    ])
    .await;
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "login", "--provider", "openai-codex"])
        .env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", fake.addr))
        .env("ARA_AUTH_BROKER_URL", "http://127.0.0.1:1")
        .env("ARA_AUTH_BROKER_TOKEN", "unused");
    let result = output(cmd).await;
    assert!(result.status.success(), "device login failed: {}", String::from_utf8_lossy(&result.stderr));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("process-refresh"));
    let store = SqliteCredentialStore::open(root.path().join("agent/auth.db")).unwrap();
    assert_eq!(store.list_auth_credentials(Some("openai-codex")).unwrap().len(), 1);
    drop(store);
    let (remote_store, remote_auth) = make_storage(&root.path().join("process-migrate-target.db"));
    let mut target_options = AuthBrokerServerOptions::new(remote_auth.clone(), vec!["fixture-migrate-token".into()]);
    target_options.bind = "127.0.0.1:0".into();
    target_options.disable_refresher = true;
    let target = start_auth_broker(target_options).await.unwrap();
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "migrate", "--from-local", "--include-oauth", "--dry-run", "--json"])
        .env("ARA_AUTH_BROKER_URL", target.url())
        .env("ARA_AUTH_BROKER_TOKEN", "fixture-migrate-token");
    let migrate_output = output(cmd).await;
    assert!(
        migrate_output.status.success(),
        "process migrate failed: {}",
        String::from_utf8_lossy(&migrate_output.stderr)
    );
    let migrate_plan: Value = serde_json::from_slice(&migrate_output.stdout).unwrap();
    assert_eq!(migrate_plan["plan"].as_array().unwrap().len(), 1);
    assert!(!String::from_utf8_lossy(&migrate_output.stdout).contains("process-refresh"));
    assert!(remote_store.lock().unwrap().list_auth_credentials(None).unwrap().is_empty());
    target.close().await.unwrap();
    remote_auth.close_and_wait().await;
    let mut cmd = command(root.path());
    cmd.args(["auth-broker", "logout", "openai-codex", "--provider", "ignored-positional-wins"])
        .env("ARA_AUTH_BROKER_URL", "http://127.0.0.1:1")
        .env("ARA_AUTH_BROKER_TOKEN", "unused");
    assert!(output(cmd).await.status.success());
    // Normal SIGTERM on Unix / Ctrl+Break on Windows stops transport and drains the original persisted owner.
    let fake=fake_upstream(vec![json!({"delay_ms":180,"body":json!({"access_token":"normal-new","refresh_token":"normal-refresh"}).to_string()})]).await;
    let provider = "mcp_oauth_legacy_process_normal";
    let store = SqliteCredentialStore::open(root.path().join("agent/auth.db")).unwrap();
    let id = store
        .upsert_auth_credential_for_provider(provider, &oauth(json!({"tokenUrl":format!("http://{}/token",fake.addr)})))
        .unwrap()[0]
        .id;
    drop(store);
    let mut broker = serve(root.path()).await;
    let url = format!("{}/v1/credential/{id}/refresh", broker.url);
    let token = broker.token.clone();
    let call = tokio::spawn(async move { client().post(url).bearer_auth(token).send().await });
    wait_requests(&fake, 1).await;
    interrupt(&broker.child);
    let status = bounded(broker.child.wait()).await.unwrap();
    assert!(status.success());
    let _ = bounded(call).await;
    let store = SqliteCredentialStore::open(root.path().join("agent/auth.db")).unwrap();
    let row = store.list_auth_credentials(Some(provider)).unwrap().remove(0);
    assert!(
        matches!(&row.credential,AuthCredential::OAuth {fields} if fields["access"]=="normal-new" && fields["refresh"]=="normal-refresh" && !fields.contains_key("tokenUrl"))
    );
    drop(store);
    // A hard kill after dispatch leaves the durable claim. Restart does not
    // replay it even though its in-process flight owner has disappeared.
    let fake =
        fake_upstream(vec![json!({"delay_ms":1500,"body":json!({"access_token":"lost-process-result"}).to_string()})])
            .await;
    let provider = "mcp_oauth_legacy_process_unknown";
    let store = SqliteCredentialStore::open(root.path().join("agent/auth.db")).unwrap();
    let id = store
        .upsert_auth_credential_for_provider(provider, &oauth(json!({"tokenUrl":format!("http://{}/token",fake.addr)})))
        .unwrap()[0]
        .id;
    drop(store);
    let mut broker = serve(root.path()).await;
    let url = format!("{}/v1/credential/{id}/refresh", broker.url);
    let token = broker.token.clone();
    let call = tokio::spawn(async move { client().post(url).bearer_auth(token).send().await });
    wait_requests(&fake, 1).await;
    broker.child.kill().await.unwrap();
    bounded(broker.child.wait()).await.unwrap();
    let _ = bounded(call).await;
    drop(broker);
    let mut restarted = serve(root.path()).await;
    let result = bounded(
        client().post(format!("{}/v1/credential/{id}/refresh", restarted.url)).bearer_auth(&restarted.token).send(),
    )
    .await
    .unwrap();
    assert_eq!(result.status(), 500);
    assert_eq!(fake.requests.lock().await.len(), 1, "hard-killed grant must not be replayed");
    interrupt(&restarted.child);
    assert!(bounded(restarted.child.wait()).await.unwrap().success());
}
