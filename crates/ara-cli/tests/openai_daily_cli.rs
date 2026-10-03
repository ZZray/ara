//! Module acceptance through the real CLI, sockets, tools and Session journal.
//! Shared protocol fixtures are reused; OAuth values below are synthetic.

use ara_testkit::chunks::{done, finish, text};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
#[cfg(feature = "test-fixture")]
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, BufReader};

struct Host {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Host {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-daily-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-daily-work-").tempdir().unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions }
    }

    fn config(&self, api: &str, endpoint: &str, auth: &str) {
        let path = self.home.path().join("agent/models.yml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let provider = if api == "openai-codex-responses" { "openai-codex" } else { "custom" };
        let mut model = json!({"id":"daily-model","input":["text"]});
        if api != "openai-codex-responses" {
            model["maxTokens"] = json!(128);
        }
        let config = json!({"providers":{provider:{"api":api,"baseUrl":endpoint,
            "auth":auth,"headers":{"X-Daily-Route":"configured"},"models":[model]}}});
        std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }

    fn command(&self, provider: &str, arguments: &[&str]) -> Command {
        self.command_for_model(provider, "daily-model", arguments)
    }

    fn command_for_model(&self, provider: &str, model: &str, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ara"));
        for name in [
            "ARA_MODEL",
            "ARA_TEST_MODEL_ID",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "ARA_PROVIDER",
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ARA_API",
            "ARA_TOKENIZER",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_API_KEY",
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            "CLAUDE_CONFIG_DIR",
            "WSL_DISTRO_NAME",
            "WSL_INTEROP",
        ] {
            command.env_remove(name);
        }
        command
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .current_dir(self.work.path())
            .args([
                "--model",
                model,
                "--provider",
                provider,
                "--no-skills",
                "--max-model-calls",
                "4",
                "--compact-threshold",
                "0",
                "--session-dir",
            ])
            .arg(&self.sessions)
            .args(arguments)
            .stdin(Stdio::null());
        command
    }

    fn session(&self) -> PathBuf {
        std::fs::read_dir(&self.sessions)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
            .unwrap()
    }
}

async fn upstream(responses: Value) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

async fn output(command: Command) -> Output {
    tokio::time::timeout(Duration::from_secs(20), tokio::process::Command::from(command).kill_on_drop(true).output())
        .await
        .expect("CLI exceeded its bounded test deadline")
        .unwrap()
}

fn success(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn response_text(message: &str) -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message",
            "id":"msg_daily","role":"assistant","content":[{"type":"output_text","text":message}]}}},
        {"data":{"type":"response.completed","response":{"status":"completed"}}}
    ]})
}

fn response_write() -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call",
            "id":"fc_daily","call_id":"call_daily","name":"write"}}},
        {"data":{"type":"response.function_call_arguments.done","output_index":0,
            "arguments":"{\"path\":\"daily.txt\",\"content\":\"daily proof\\n\"}"}},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call",
            "id":"fc_daily","call_id":"call_daily","name":"write","arguments":"{}"}}},
        {"data":{"type":"response.completed","response":{"status":"completed"}}}
    ]})
}

#[tokio::test]
async fn custom_chat_configuration_and_explicit_overrides_reach_the_wire() {
    // Reuse the named SSE wire framing from e2e::anthropic_messages_runs_tool_and_replays_after_restart.
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let host = Host::new();
    let up = upstream(json!([
        {"events":[text("Configured."),finish("stop"),done()]},
        {"events":[text("Overridden."),finish("stop"),done()]},
        {"events":[text("Windows home."),finish("stop"),done()]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_legacy"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Legacy route."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]))
    .await;
    host.config("openai-completions", &up.base_url(), "none");
    assert!(success(&output(host.command("custom", &["--tools", "", "first"])).await).contains("Configured."));
    let mut command = host.command(
        "custom",
        &[
            "--max-tokens",
            "64",
            "--api-key-env",
            "DAILY_FIXTURE_KEY",
            "--header",
            "x-daily-route: cli",
            "--tools",
            "",
            "second",
        ],
    );
    command.env("DAILY_FIXTURE_KEY", "synthetic-private-key");
    assert!(success(&output(command).await).contains("Overridden."));
    let default = host.home.path().join(".ara/agent/models.yml");
    std::fs::create_dir_all(default.parent().unwrap()).unwrap();
    std::fs::copy(host.home.path().join("agent/models.yml"), default).unwrap();
    let mut windows_home = host.command("custom", &["--tools", "", "third"]);
    windows_home.env_remove("HOME").env_remove("ARA_HOME").env("USERPROFILE", host.home.path());
    assert!(success(&output(windows_home).await).contains("Windows home."));
    let mut legacy = host
        .command("anthropic", &["--api", "anthropic-messages", "--base-url", &up.base_url(), "--tools", "", "fourth"]);
    legacy.env("ARA_API_KEY", "synthetic-legacy-key");
    assert!(success(&output(legacy).await).contains("Legacy route."));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[3]["request"], "POST /v1/messages HTTP/1.1");
    assert_eq!(requests[0]["request"], "POST /v1/chat/completions HTTP/1.1");
    // The fixed native builder now resolves compat for the custom route.
    assert_eq!(requests[0]["body"]["max_completion_tokens"], 128);
    assert_eq!(requests[1]["body"]["max_completion_tokens"], 64);
    assert!(requests[0]["body"].get("max_tokens").is_none());
    assert!(requests[1]["body"].get("max_tokens").is_none());
    assert_eq!(requests[0]["headers"]["x-daily-route"], "configured");
    assert_eq!(requests[1]["headers"]["x-daily-route"], "cli");
}

/// Ordinary OpenAI requests use the Host's shared SQLite AuthStorage. These
/// synthetic credentials exercise real CLI processes and sockets, not login
/// command support or a real OpenAI account/model trial.
#[tokio::test]
async fn ordinary_openai_shared_storage_precedence_overrides_and_session_restart_reach_the_wire() {
    use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore, StoredAuthCredential};

    const LOGIN: &str = "synthetic-login-owner";
    const ENVIRONMENT: &str = "synthetic-environment-default-key";
    const STATIC: &str = "synthetic-static-store-fixture-key-material";
    const UPDATED: &str = "synthetic-updated-login-row-for-original-session";
    const EXPLICIT: &str = "synthetic-explicit-cli-environment-key-owner";
    const CONFIGURED: &str = "synthetic-config-api-key-owner-fixture";
    const CLI_HEADER: &str = "Bearer synthetic-private-cli-header-with-longer-material";
    const CONFIG_HEADER: &str = "synthetic-configured-header-key-with-even-longer-material";

    fn credential(key: &str, login: bool) -> AuthCredential {
        AuthCredential::ApiKey { key: key.into(), source: login.then(|| "login".into()) }
    }

    fn store(host: &Host, credentials: &[AuthCredential]) -> Vec<StoredAuthCredential> {
        SqliteCredentialStore::open(host.home.path().join("agent/auth.db"))
            .unwrap()
            .replace_auth_credentials_for_provider("openai", credentials)
            .unwrap()
    }

    fn config(host: &Host, endpoint: &str, options: Value) {
        let path = host.home.path().join("agent/models.yml");
        // New custom models require an authored key or auth none/oauth.
        // This is a provider override; the explicit CLI model/API/base URL own
        // the synthetic route while its default auth remains available to store.
        let mut provider = json!({"api":"openai-completions","baseUrl":endpoint,"models":[]});
        provider.as_object_mut().unwrap().extend(options.as_object().unwrap().clone());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&json!({"providers":{"openai":provider}})).unwrap()).unwrap();
    }

    fn command(host: &Host, endpoint: &str, arguments: &[&str]) -> Command {
        let mut command = host.command(
            "openai",
            &[
                "--mode",
                "json",
                "--tools",
                "",
                "--api",
                "openai-completions",
                "--base-url",
                endpoint,
                "--max-tokens",
                "128",
                "--tokenizer",
                "none",
            ],
        );
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", host.home.path())
            .env("ARA_HOME", host.home.path())
            .env("USERPROFILE", host.home.path())
            .env("TMP", host.home.path())
            .env("TEMP", host.home.path())
            .args(arguments);
        command
    }

    fn redacted(value: &[u8]) -> String {
        let mut value = String::from_utf8_lossy(value).into_owned();
        for private in [LOGIN, ENVIRONMENT, STATIC, UPDATED, EXPLICIT, CONFIGURED, CLI_HEADER, CONFIG_HEADER] {
            value = value.replace(private.strip_prefix("Bearer ").unwrap_or(private), "<synthetic-credential>");
        }
        value
    }

    async fn case_success(label: &str, output: &Output, up: &FakeUpstream, expected: &str) -> String {
        let stdout = redacted(&output.stdout);
        let stderr = redacted(&output.stderr);
        let requests = up.requests.lock().await;
        let routes = requests
            .iter()
            .map(|request| {
                let route = request["request"]
                    .as_str()
                    .unwrap_or("missing request line")
                    .split_whitespace()
                    .take(2)
                    .collect::<Vec<_>>()
                    .join(" ");
                redacted(route.as_bytes())
            })
            .collect::<Vec<_>>();
        let diagnostic = format!(
            "shared-storage CLI scenario {label}; request count {}; method/path {routes:?}; stdout {stdout}; stderr {stderr}",
            requests.len()
        );
        assert_eq!(output.status.code(), Some(0), "{diagnostic}");
        assert!(stdout.contains(expected), "expected scenario reply missing; {diagnostic}");
        stdout
    }

    fn session_header(stdout: &str) -> Value {
        serde_json::from_str(stdout.lines().next().expect("JSON mode Session header")).unwrap()
    }

    fn text_content(content: &Value) -> Option<String> {
        match content {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => parts
                .iter()
                .map(|part| (part["type"] == "text").then(|| part["text"].as_str()).flatten())
                .collect::<Option<Vec<_>>>()
                .map(|parts| parts.concat()),
            _ => None,
        }
    }

    fn history_shapes(messages: &[Value]) -> String {
        let shapes = messages.iter().filter(|message| matches!(message["role"].as_str(), Some("user" | "assistant")))
            .map(|message| {
                let shape = match &message["content"] {
                    Value::String(text) => json!({"kind":"string","textBytes":text.len()}),
                    Value::Array(parts) => json!({"kind":"array","parts":parts.iter().map(|part|
                        json!({"type":part["type"],"textBytes":part["text"].as_str().map(str::len)})).collect::<Vec<_>>()}),
                    Value::Null => json!({"kind":"null"}),
                    _ => json!({"kind":"other"}),
                };
                json!({"role":message["role"],"contentShape":shape})
            }).collect::<Vec<_>>();
        redacted(json!(shapes).to_string().as_bytes())
    }

    enum ExpectedAuth {
        Bearer(&'static str),
        Header(&'static str, &'static str),
        None,
    }
    fn assert_auth(request: &Value, expected: &ExpectedAuth) {
        let headers = request["headers"].as_object().unwrap();
        match expected {
            ExpectedAuth::Bearer(key) => {
                assert_eq!(headers["authorization"], format!("<redacted {} chars>", "Bearer ".len() + key.len()));
                assert!(!headers.contains_key("x-api-key"));
            }
            ExpectedAuth::Header(name, value) => {
                assert_eq!(headers[*name], format!("<redacted {} chars>", value.len()));
                if *name != "authorization" {
                    assert!(!headers.contains_key("authorization"));
                }
            }
            ExpectedAuth::None => {
                assert!(!headers.contains_key("authorization"));
                assert!(!headers.contains_key("x-api-key"));
            }
        }
    }

    struct Case {
        label: &'static str,
        stored_key: &'static str,
        login: bool,
        environment: Option<&'static str>,
        config: Option<Value>,
        arguments: &'static [&'static str],
        expected: ExpectedAuth,
    }
    let cases = [
        Case {
            label: "environment precedes static store",
            stored_key: STATIC,
            login: false,
            environment: Some(ENVIRONMENT),
            config: Some(json!({})),
            arguments: &[],
            expected: ExpectedAuth::Bearer(ENVIRONMENT),
        },
        Case {
            label: "static store without environment",
            stored_key: STATIC,
            login: false,
            environment: None,
            config: Some(json!({"headers":{"X-Daily-Route":"stored-static"}})),
            arguments: &[],
            expected: ExpectedAuth::Bearer(STATIC),
        },
        Case {
            label: "shared store without models config",
            stored_key: LOGIN,
            login: true,
            environment: Some(ENVIRONMENT),
            config: None,
            arguments: &[],
            expected: ExpectedAuth::Bearer(LOGIN),
        },
        Case {
            label: "explicit CLI environment owns auth",
            stored_key: LOGIN,
            login: true,
            environment: Some(ENVIRONMENT),
            config: Some(json!({})),
            arguments: &["--api-key-env", "DAILY_EXPLICIT_SHARED_KEY"],
            expected: ExpectedAuth::Bearer(EXPLICIT),
        },
        Case {
            label: "configured API key owns auth",
            stored_key: LOGIN,
            login: true,
            environment: None,
            config: Some(json!({"apiKey":CONFIGURED})),
            arguments: &[],
            expected: ExpectedAuth::Bearer(CONFIGURED),
        },
        Case {
            label: "auth none remains keyless",
            stored_key: LOGIN,
            login: true,
            environment: Some(ENVIRONMENT),
            config: Some(json!({"auth":"none"})),
            arguments: &[],
            expected: ExpectedAuth::None,
        },
        Case {
            label: "CLI credential header owns auth",
            stored_key: LOGIN,
            login: true,
            environment: None,
            config: Some(json!({})),
            arguments: &["--header", "Authorization: Bearer synthetic-private-cli-header-with-longer-material"],
            expected: ExpectedAuth::Header("authorization", CLI_HEADER),
        },
        Case {
            label: "configured credential header owns auth",
            stored_key: LOGIN,
            login: true,
            environment: None,
            config: Some(json!({"headers":{"X-Api-Key":CONFIG_HEADER}})),
            arguments: &[],
            expected: ExpectedAuth::Header("x-api-key", CONFIG_HEADER),
        },
    ];
    fn catalog_response() -> Value {
        // Native OpenAI discovery filters to known Responses-capable IDs.
        // The explicit CLI model remains the synthetic daily-model route.
        json!({"body":json!({"data":[{"id":"gpt-5.4"}]}).to_string()})
    }
    fn native_cache_entry(host: &Host) -> ara_cli::model_cache::WireCacheEntry {
        ara_cli::model_cache::SqliteModelCache::for_path(host.home.path().join("agent/model-cache.db"))
            .read_model_cache_wire(&"openai".into(), ara_cli::model_manager::DEFAULT_CACHE_TTL_MS, || {
                chrono::Utc::now().timestamp_millis() as f64
            })
            .unwrap()
            .expect("native OpenAI cache uses its provider-only key")
    }
    let mut responses = vec![
        catalog_response(),
        json!({"events":[text("Login store accepted."),finish("stop"),done()]}),
        json!({"events":[text("Updated row accepted."),finish("stop"),done()]}),
    ];
    for case in &cases {
        if case.config.is_some() {
            responses.push(catalog_response());
        }
        responses.push(json!({"events":[text(&format!("Accepted scenario: {}.", case.label)),finish("stop"),done()]}));
    }
    let up = upstream(json!(responses)).await;
    let endpoint = up.base_url();

    let host = Host::new();
    let rows = store(&host, &[credential(STATIC, false), credential(LOGIN, true)]);
    config(&host, &endpoint, json!({"headers":{"X-Daily-Route":"shared-login"}}));
    let mut first = command(&host, &endpoint, &["first shared auth turn"]);
    first.env("ARA_API_KEY", ENVIRONMENT);
    let first = output(first).await;
    let first_stdout = case_success("stored login precedes environment", &first, &up, "Login store accepted.").await;
    let original = session_header(&first_stdout);
    let session = host.session();
    {
        let database = SqliteCredentialStore::open(host.home.path().join("agent/auth.db")).unwrap();
        assert!(
            database
                .try_update_auth_credential_if_matches(
                    rows[1].id,
                    &rows[1].serialized_data,
                    &credential(UPDATED, true),
                    None,
                )
                .unwrap()
        );
        let updated = database.list_auth_credentials(Some("openai")).unwrap();
        assert_eq!(
            updated.iter().map(|row| row.id).collect::<Vec<_>>(),
            rows.iter().map(|row| row.id).collect::<Vec<_>>()
        );
        assert!(updated[0].credential == credential(STATIC, false));
    }
    let mut resumed =
        command(&host, &endpoint, &["--resume", session.to_str().unwrap(), "continue after exact stored row update"]);
    resumed.env("ARA_API_KEY", ENVIRONMENT);
    let resumed = output(resumed).await;
    let resumed_stdout =
        case_success("same Session after exact stored row update", &resumed, &up, "Updated row accepted.").await;
    assert_eq!(session_header(&resumed_stdout)["id"], original["id"]);
    {
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests.iter().filter(|request| request["request"] == "GET /v1/models HTTP/1.1").count(),
            1,
            "native cache must keep the original Session restart from fetching another catalog"
        );
        let models = requests
            .iter()
            .filter(|request| request["request"] == "POST /v1/chat/completions HTTP/1.1")
            .collect::<Vec<_>>();
        assert_eq!(models.len(), 2);
        assert_auth(models[0], &ExpectedAuth::Bearer(LOGIN));
        assert_auth(models[1], &ExpectedAuth::Bearer(UPDATED));
        assert_eq!(models[0]["headers"]["x-daily-route"], "shared-login");
        let messages = models[1]["body"]["messages"].as_array().unwrap();
        // CliHooks injects the date/cwd reminder into provider context only.
        // Native Completions preserves Text as a string and Blocks as arrays;
        // check the original bytes in both source-supported representations.
        let reminder = ara_context::render_date_cwd_reminder(
            &chrono::Local::now().format("%Y-%m-%d").to_string(),
            &host.work.path().to_string_lossy().replace('\\', "/"),
        );
        assert!(
            messages.iter().any(|message| message["role"] == "user"
                && text_content(&message["content"]).is_some_and(|text| text == "first shared auth turn"
                    || text == format!("{reminder}\n\nfirst shared auth turn")
                    || text == format!("{reminder}first shared auth turn"))),
            "resumed original user text missing; safe content shapes {}",
            history_shapes(messages)
        );
        assert!(
            messages.iter().any(|message| message["role"] == "assistant"
                && text_content(&message["content"]).as_deref() == Some("Login store accepted.")),
            "resumed original assistant text missing; safe content shapes {}",
            history_shapes(messages)
        );
    }
    let journal = std::fs::read_to_string(&session).unwrap();
    for private in [LOGIN, ENVIRONMENT, STATIC, UPDATED] {
        assert!(!journal.contains(private), "credential material must not enter the Session journal");
    }
    assert_eq!(
        std::fs::read_dir(&host.sessions)
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|extension| extension == "jsonl"))
            .count(),
        1
    );

    let native_cache = host.home.path().join("agent/model-cache.db");
    assert!(native_cache.is_file(), "cold native JSON discovery must write its real SQLite cache");
    let cached = native_cache_entry(&host);
    let native_fingerprint = ara_cli::model_manager::fingerprint_static_models(
        &ara_cli::model_manager::ModelArray::new(ara_cli::model_identity_wire::bundled_provider_models(
            &"openai".into(),
        )),
        false,
    );
    assert_eq!(cached.static_fingerprint, native_fingerprint);
    let mut catalog_count = 1;
    for (index, case) in cases.into_iter().enumerate() {
        let host = Host::new();
        let rows = store(&host, &[credential(case.stored_key, case.login)]);
        if let Some(options) = case.config {
            config(&host, &endpoint, options);
            catalog_count += 1;
        } else {
            assert!(!host.home.path().join("agent/models.yml").exists());
            // Without provider configuration, native catalog discovery owns
            // its official endpoint independently of --base-url. Reuse the
            // actual first process's warm native cache to keep synthetic keys
            // local while still exercising the no-models.yml request path.
            std::fs::copy(&native_cache, host.home.path().join("agent/model-cache.db")).unwrap();
            let copied = native_cache_entry(&host);
            assert!(
                copied.fresh
                    && copied.header_omitted_model_ids.is_empty()
                    && copied.unrestorable_header_model_ids.is_empty()
            );
            assert_eq!(copied.static_fingerprint, native_fingerprint);
            assert_eq!(copied.updated_at, cached.updated_at);
            assert!(
                chrono::Utc::now().timestamp_millis() as f64 - copied.updated_at
                    < ara_cli::model_manager::NON_AUTHORITATIVE_RETRY_MS,
                "abort before CLI if the native non-authoritative cache could trigger a remote discovery"
            );
        }
        let mut arguments = case.arguments.to_vec();
        arguments.push(case.label);
        let mut invocation = command(&host, &endpoint, &arguments);
        if let Some(key) = case.environment {
            invocation.env("ARA_API_KEY", key);
        }
        invocation.env("DAILY_EXPLICIT_SHARED_KEY", EXPLICIT);
        let result = output(invocation).await;
        case_success(case.label, &result, &up, &format!("Accepted scenario: {}.", case.label)).await;
        let requests = up.requests.lock().await;
        assert_eq!(
            requests.iter().filter(|request| request["request"] == "GET /v1/models HTTP/1.1").count(),
            catalog_count,
            "catalog count for scenario {}",
            case.label
        );
        let models = requests
            .iter()
            .filter(|request| request["request"] == "POST /v1/chat/completions HTTP/1.1")
            .collect::<Vec<_>>();
        assert_eq!(models.len(), index + 3, "each scenario must dispatch exactly one model request");
        assert_auth(models[index + 2], &case.expected);
        drop(requests);
        let stored = SqliteCredentialStore::open(host.home.path().join("agent/auth.db"))
            .unwrap()
            .list_auth_credentials(Some("openai"))
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].id, rows[0].id, "explicit auth must preserve the stored credential row");
        assert!(stored[0].credential == credential(case.stored_key, case.login));
        let journal = std::fs::read_to_string(host.session()).unwrap();
        for private in [LOGIN, ENVIRONMENT, STATIC, UPDATED, EXPLICIT, CONFIGURED, CLI_HEADER, CONFIG_HEADER] {
            assert!(!journal.contains(private), "credential material must not enter the Session journal");
        }
    }
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 18);
    assert_eq!(requests.iter().filter(|request| request["request"] == "GET /v1/models HTTP/1.1").count(), 8);
    let models = requests
        .iter()
        .filter(|request| request["request"] == "POST /v1/chat/completions HTTP/1.1")
        .collect::<Vec<_>>();
    assert_eq!(models.len(), 10);
    assert!(models.iter().all(|request| request["request"] == "POST /v1/chat/completions HTTP/1.1"
        && request["body"]["model"] == "daily-model"));
}

#[tokio::test]
async fn configured_responses_stream_tool_artifact_and_restart_use_the_original_session() {
    let host = Host::new();
    let up = upstream(json!([
        response_write(),
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_live"}}},
            {"data":{"type":"response.output_text.delta","output_index":0,"item_id":"msg_live","delta":"live proof"}},
            {"sleep_ms":700},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_live",
                "content":[{"type":"output_text","text":"live proof"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        response_text("Resumed.")
    ]))
    .await;
    host.config("openai-responses", &up.base_url(), "none");
    let command = host.command("custom", &["--mode", "json", "--tools", "write", "write daily.txt"]);
    let mut child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut events = Vec::new();
    loop {
        let line = tokio::time::timeout(Duration::from_secs(10), reader.next_line()).await.unwrap().unwrap().unwrap();
        let event: Value = serde_json::from_str(&line).unwrap();
        let live = event["assistantMessageEvent"]["delta"] == "live proof";
        events.push(event);
        if live {
            break;
        }
    }
    assert!(child.try_wait().unwrap().is_none(), "stream event must arrive while the Run is active");
    while let Some(line) = tokio::time::timeout(Duration::from_secs(10), reader.next_line()).await.unwrap().unwrap() {
        events.push(serde_json::from_str(&line).unwrap());
    }
    assert_eq!(child.wait().await.unwrap().code(), Some(0));
    assert_eq!(std::fs::read_to_string(host.work.path().join("daily.txt")).unwrap(), "daily proof\n");
    let original = events[0]["id"].as_str().unwrap();
    let session = host.session();
    let resumed =
        output(host.command("custom", &["--mode", "json", "--resume", session.to_str().unwrap(), "continue"])).await;
    let resumed = success(&resumed);
    let header: Value = serde_json::from_str(resumed.lines().next().unwrap()).unwrap();
    assert_eq!(header["id"], original);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["request"], "POST /v1/responses HTTP/1.1");
    assert_eq!(requests[0]["body"]["max_output_tokens"], 128);
    let input = requests[2]["body"]["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_daily"));
    assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_daily"));
    let journal = std::fs::read_to_string(session).unwrap();
    assert!(journal.contains("openai-responses"));
}

/// Real Rust CLI -> configured broker -> shared Registry/AuthStorage ->
/// request lease -> tool artifact/journal/resume, including cold broker failure.
#[tokio::test]
async fn broker_discovery_cache_shared_request_owner_and_restart_family() {
    let host = Host::new();
    let model = upstream(json!([
        response_write(),
        response_text("Broker task complete."),
        response_text("Broker resume complete.")
    ]))
    .await;
    host.config("openai-responses", &model.base_url(), "none");
    let path = host.home.path().join("agent/models.yml");
    let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["providers"]["custom"].as_object_mut().unwrap().remove("auth");
    // Native custom-model declarations require authored credentials. This
    // route uses an explicit CLI model with provider transport overrides and
    // lets the shared Broker owner supply authentication instead.
    config["providers"]["custom"]["models"] = json!([]);
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    const KEY: &str = "synthetic-remote-broker-api-key-only";
    let now = chrono::Utc::now().timestamp_millis();
    let snapshot = json!({"generation":3,"generatedAt":now,"serverNowMs":now,
        "refresher":{"enabled":false,"intervalMs":0,"skewMs":0,"nextSweepInMs":9007199254740991_u64},
        "credentials":[{"id":41,"provider":"custom","credential":{"type":"api_key","key":KEY},
            "identityKey":null,"rotatesInMs":null}]});
    let mut event = snapshot.clone();
    event["type"] = json!("snapshot");
    let broker = upstream(json!([
        {"body":snapshot.to_string()},
        {"events":[{"raw":format!("data: {event}\n\n")}],"end":"hang"},
        {"events":[{"raw":format!("data: {event}\n\n")}],"end":"hang"}
    ]))
    .await;
    let url = format!("http://{}", broker.addr);
    let command = |arguments: &[&str]| {
        let mut command = host.command("custom", arguments);
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", host.home.path())
            .env("USERPROFILE", host.home.path())
            .env("ARA_HOME", host.home.path())
            .env("TEMP", host.home.path())
            .env("TMP", host.home.path())
            .env("ARA_AUTH_BROKER_URL", &url)
            .env("ARA_AUTH_BROKER_TOKEN", "synthetic-broker-token");
        command
    };
    let first = output(command(&["--mode", "json", "--tools", "write", "write daily.txt"])).await;
    let stdout = success(&first);
    assert!(stdout.contains("Broker task complete."));
    assert_eq!(std::fs::read_to_string(host.work.path().join("daily.txt")).unwrap(), "daily proof\n");
    let original: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    let cache = host.home.path().join("cache/auth-broker-snapshot.enc");
    assert!(cache.exists());
    assert!(!host.home.path().join("agent/auth.db").exists());
    assert!(!stdout.contains(KEY));
    assert!(!String::from_utf8_lossy(&first.stderr).contains(KEY));
    let requests = broker.requests.lock().await;
    assert_eq!(requests.iter().filter(|request| request["request"] == "GET /v1/snapshot HTTP/1.1").count(), 1);
    // Missing cost/buckets remain unknown in the Session, so no fabricated
    // numeric observed usage report is sent for this controlled model.
    assert!(!requests.iter().any(|request| request["request"] == "POST /v1/usage/observed HTTP/1.1"));
    drop(requests);
    drop(broker);
    let journal = host.session();
    let resumed = success(
        &output(command(&["--mode", "json", "--tools", "", "--resume", journal.to_str().unwrap(), "continue"])).await,
    );
    assert!(resumed.contains("Broker resume complete."));
    let resumed_header: Value = serde_json::from_str(resumed.lines().next().unwrap()).unwrap();
    assert_eq!(resumed_header["id"], original["id"]);
    let requests = model.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["request"], "POST /v1/responses HTTP/1.1");
        assert_eq!(request["headers"]["authorization"], format!("<redacted {} chars>", KEY.len() + 7));
    }
    assert!(requests[2]["body"]["input"].to_string().contains("daily.txt"));
    drop(requests);
    // A reachable local key must not replace an explicitly configured cold,
    // unreachable broker when the encrypted cache has been disabled.
    let local = ara_cli::credential_store::SqliteCredentialStore::open(host.home.path().join("agent/auth.db")).unwrap();
    local
        .replace_auth_credentials_for_provider(
            "custom",
            &[ara_cli::credential_store::AuthCredential::api_key("synthetic-local-fallback")],
        )
        .unwrap();
    let mut cold = command(&["--mode", "json", "--tools", "", "cold broker"]);
    cold.env("ARA_AUTH_BROKER_SNAPSHOT_TTL_MS", "0");
    let failed = output(cold).await;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("Auth broker startup failed"));
    let mut missing = command(&["--mode", "json", "--tools", "", "missing bearer"]);
    missing.env_remove("ARA_AUTH_BROKER_TOKEN");
    let failed = output(missing).await;
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("no bearer token"));
    assert_eq!(model.requests.lock().await.len(), 3);
}

/// This exercises the production startup/Registry/cache/Session path using
/// controlled JSON and SSE upstreams. It is not a real-model acceptance trial.
#[tokio::test]
async fn discovered_registry_model_cold_tools_hot_cache_and_config_replacement_keep_original_session() {
    use ara_cli::model_cache::SqliteModelCache;
    use ara_cli::model_registry_loader::STARTUP_CACHE_TTL_MS;
    use ara_testkit::chunks::tool_call;

    fn discovery_config(host: &Host, endpoint: &str) {
        let path = host.home.path().join("agent/models.yml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // No static models: production selection must obtain catalog metadata
        // from discovery/cache. Header-free keyless transport has no omitted
        // header marker requiring an authenticated restoration fallback.
        let config = json!({"providers":{"custom":{"api":"openai-completions","baseUrl":endpoint,
            "auth":"none","discovery":{"type":"openai-models-list","timeoutMs":2000}}}});
        std::fs::write(path, serde_json::to_vec_pretty(&config).unwrap()).unwrap();
    }

    fn registry_command(host: &Host, arguments: &[&str]) -> Command {
        let mut command = host.command_for_model("custom", "registry-fixture-model", arguments);
        // Global background discovery is part of the production entrypoint.
        // Isolate credentials and routing from the developer's environment.
        command.env_clear();
        for name in ["PATH", "SystemRoot", "WINDIR", "SystemDrive", "ComSpec", "PATHEXT"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", host.home.path())
            .env("ARA_HOME", host.home.path())
            .env("USERPROFILE", host.home.path())
            .env("TMP", host.home.path())
            .env("TEMP", host.home.path())
            .args(["--max-tokens", "128"]);
        command
    }

    async fn registry_output(command: Command) -> Output {
        tokio::time::timeout(
            Duration::from_secs(25),
            tokio::process::Command::from(command).kill_on_drop(true).output(),
        )
        .await
        .expect("production Registry CLI exceeded the 25-second process deadline")
        .unwrap()
    }

    fn header(output: &Output) -> Value {
        let stdout = success(output);
        serde_json::from_str(stdout.lines().next().expect("JSON mode Session header")).unwrap()
    }

    fn cache_entry(host: &Host) -> ara_cli::model_cache::WireCacheEntry {
        let path = host.home.path().join("agent/model-cache.db");
        assert!(path.is_file(), "real production CLI must write its SQLite cache");
        let cache = SqliteModelCache::open(path).unwrap();
        cache
            .read_model_cache_wire(&"custom:openai-models-list-context-v3".into(), STARTUP_CACHE_TTL_MS, || {
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
            })
            .unwrap()
            .expect("configured discovery namespace must be persisted")
    }

    tokio::time::timeout(Duration::from_secs(120), async {
        let host = Host::new();
        let first = upstream(json!([
            {"body":json!({"data":[{"id":"registry-fixture-model","context_length":8192}]}).to_string()},
            {"events":[tool_call(0,"registry-write","write",&json!({"path":"registry-proof.txt","content":"registry proof\n"}).to_string()),finish("tool_calls"),done()]},
            {"events":[tool_call(0,"registry-read","read",&json!({"path":"registry-proof.txt"}).to_string()),finish("tool_calls"),done()]},
            {"events":[text("Cold discovery and tools accepted."),finish("stop"),done()]},
            {"events":[text("Cached resume accepted."),finish("stop"),done()]}
        ])).await;
        discovery_config(&host, &first.base_url());
        assert!(!host.home.path().join("agent/model-cache.db").exists());
        let cold = registry_output(registry_command(&host,
            &["--mode","json","--tools","write,read","write registry-proof.txt then read it"])).await;
        let original = header(&cold);
        assert!(success(&cold).contains("Cold discovery and tools accepted."));
        let session = host.session();
        let artifact = host.work.path().join("registry-proof.txt");
        assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "registry proof\n");
        let cached = cache_entry(&host);
        assert!(cached.fresh && cached.authoritative);
        assert!(cached.header_omitted_model_ids.is_empty() && cached.unrestorable_header_model_ids.is_empty());
        assert_eq!(cached.models.value.as_array().unwrap().len(), 1);
        let cached_model = &cached.models.value.as_array().unwrap()[0];
        assert_eq!(cached_model.get("id"), Some(&ara_rpc::WireValue::String("registry-fixture-model".into())));
        assert_eq!(cached_model.get("contextWindow"), Some(&ara_rpc::WireValue::Number(8192.0)));
        {
            let requests = first.requests.lock().await;
            assert_eq!(requests.len(), 4, "cold discovery adds one GET to the three actual model calls");
            assert_eq!(requests[0]["request"], "GET /v1/models HTTP/1.1");
            assert!(requests[1..].iter().all(|request| request["request"] == "POST /v1/chat/completions HTTP/1.1"
                && request["body"]["model"] == "registry-fixture-model"));
            let messages = requests[3]["body"]["messages"].as_array().unwrap();
            assert!(messages.iter().any(|message| message["role"] == "tool" && message["tool_call_id"] == "registry-read"
                && message["content"].as_str().is_some_and(|content| content.contains("registry proof"))),
                "the follow-up model call must receive the real read tool result");
        }

        // There is no CLI offline flag. A same-directory warm restart with an
        // unchanged config proves OnlineIfUncached takes the actual SQLite row:
        // its script has only SSE left, and a second GET fails the count gate.
        let warm = registry_output(registry_command(&host,
            &["--mode","json","--tools","read","--resume",session.to_str().unwrap(),"continue from the artifact"])).await;
        assert_eq!(header(&warm)["id"], original["id"]);
        assert!(success(&warm).contains("Cached resume accepted."));
        assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "registry proof\n");
        let after_warm = cache_entry(&host);
        assert_eq!(after_warm.updated_at, cached.updated_at, "a warm process must not refresh the selected configured cache");
        {
            let requests = first.requests.lock().await;
            assert_eq!(requests.len(), 5);
            assert_eq!(requests.iter().filter(|request| request["request"] == "GET /v1/models HTTP/1.1").count(), 1);
            let messages = requests[4]["body"]["messages"].as_array().unwrap();
            assert!(messages.iter().any(|message| message["role"] == "tool" && message["tool_call_id"] == "registry-write"));
            assert!(messages.iter().any(|message| message["role"] == "tool" && message["tool_call_id"] == "registry-read"));
        }

        let replacement = upstream(json!([
            {"body":json!({"data":[{"id":"registry-fixture-model","context_length":6144}]}).to_string()},
            {"events":[text("Replacement route accepted."),finish("stop"),done()]}
        ])).await;
        // Ensure the replaced source's mtime crosses the fixed loader's integer
        // millisecond comparison even on a fast local filesystem.
        tokio::time::sleep(Duration::from_millis(20)).await;
        discovery_config(&host, &replacement.base_url());
        let replaced = registry_output(registry_command(&host,
            &["--mode","json","--tools","read","--resume",session.to_str().unwrap(),"continue after replacing the catalog source"])).await;
        assert_eq!(header(&replaced)["id"], original["id"]);
        assert!(success(&replaced).contains("Replacement route accepted."));
        assert_eq!(std::fs::read_to_string(&artifact).unwrap(), "registry proof\n");
        assert_eq!(first.served(), 5, "configuration replacement must stop routing to the original source");
        {
            let requests = replacement.requests.lock().await;
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0]["request"], "GET /v1/models HTTP/1.1");
            assert_eq!(requests[1]["request"], "POST /v1/chat/completions HTTP/1.1");
            let messages = requests[1]["body"]["messages"].as_array().unwrap();
            assert!(messages.iter().any(|message| message["role"] == "tool" && message["tool_call_id"] == "registry-read"));
        }
        let refreshed = cache_entry(&host);
        assert!(refreshed.fresh && refreshed.authoritative && refreshed.updated_at > cached.updated_at);
        let model = &refreshed.models.value.as_array().unwrap()[0];
        assert_eq!(model.get("contextWindow"), Some(&ara_rpc::WireValue::Number(6144.0)));
        assert_eq!(model.get("baseUrl"), Some(&ara_rpc::WireValue::String(replacement.base_url().into())));
        let journal: Vec<Value> = std::fs::read_to_string(&session).unwrap().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect();
        // Native title-slot metadata may precede the actual Session header.
        let session_headers = journal.iter().filter(|entry| entry["type"] == "session").collect::<Vec<_>>();
        assert_eq!(session_headers.len(), 1);
        assert_eq!(session_headers[0]["id"], original["id"]);
        for call_id in ["registry-write","registry-read"] {
            assert_eq!(journal.iter().filter(|entry| entry["message"]["role"] == "toolResult"
                && entry["message"]["toolCallId"] == call_id).count(), 1,
                "resuming an accepted tool receipt must not replay the tool");
        }
        assert_eq!(std::fs::read_dir(&host.sessions).unwrap().flatten().filter(|entry|
            entry.path().extension().is_some_and(|extension| extension == "jsonl")).count(), 1,
            "all three real processes must keep the original Session file");
    }).await.expect("grouped Registry process scenario exceeded the 120-second test deadline");
}

#[tokio::test]
async fn unsupported_selected_configuration_fails_before_journal_or_model_request() {
    let host = Host::new();
    let up = upstream(json!([])).await;
    host.config("openai-completions", &up.base_url(), "none");
    let path = host.home.path().join("agent/models.yml");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["providers"]["custom"]["compat"] = json!({"extraBody":{"secret":"must-not-be-printed"}});
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    let result = output(host.command("custom", &["hello"])).await;
    assert_eq!(result.status.code(), Some(2));
    let error = String::from_utf8_lossy(&result.stderr);
    assert!(error.contains("extraBody"), "{error}");
    assert!(!error.contains("must-not-be-printed"));
    assert!(!host.sessions.exists());
    assert_eq!(up.served(), 0);
    host.config("openai-codex-responses", &format!("http://{}", up.addr), "oauth");
    let rpc = output(host.command("openai-codex", &["--mode", "rpc"])).await;
    assert_eq!(rpc.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&rpc.stderr).contains("supports print and REPL"),
        "{}",
        String::from_utf8_lossy(&rpc.stderr)
    );
    assert!(!host.sessions.exists());
    assert!(!host.home.path().join("agent/auth.db").exists());
    assert_eq!(up.served(), 0);
}

#[tokio::test]
async fn native_catalog_tokenizer_executes_without_an_override_and_rejects_unknown_names_before_effects() {
    let host = Host::new();
    let up = upstream(json!([{"events":[text("Native count."),finish("stop"),done()]}])).await;
    host.config("openai-completions", &up.base_url(), "none");
    let path = host.home.path().join("agent/models.yml");
    let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let model_id = "deepseek-v4.1-flash";
    value["providers"]["custom"]["models"][0]["id"] = json!(model_id);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let result = output(host.command_for_model(
        "custom",
        model_id,
        &["--report-request-text-tokens", "--tools", "", "native count"],
    ))
    .await;
    assert!(success(&result).contains("Native count."));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let body = &requests[0]["body"];
    assert_eq!(body["model"], model_id);
    assert!(body.get("tokenizer").is_none());
    let fragments: Vec<_> = body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["content"].as_str().expect("text-only fixture"))
        .collect();
    let expected = ara_ai::model_tokenizer::count_family_fragments(ara_ai::ModelTokenizer::DeepSeekV3, fragments);
    let ara_ai::model_tokenizer::ModelContentCount::Exact(expected) = expected else { panic!("exact native count") };
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(diagnostics.contains(&format!("Exact({expected})")), "{diagnostics}");
    assert!(diagnostics.contains("complete=true"));
    drop(requests);
    let before = std::fs::read(host.session()).unwrap();
    let failure = output(host.command_for_model("custom", model_id, &["--tokenizer", "unrecognized", "fail"])).await;
    assert_eq!(failure.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&failure.stderr).contains("tokenizer"));
    assert_eq!(std::fs::read(host.session()).unwrap(), before);
    assert_eq!(up.served(), 1);
}

#[cfg(feature = "test-fixture")]
#[tokio::test]
async fn device_login_tool_resume_compaction_and_logout_close_the_cli_account_workflow() {
    use base64::Engine as _;
    let host = Host::new();
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({
            "https://api.openai.com/auth":{"chatgpt_account_id":"synthetic-account","chatgpt_data_residency":"eu"},
            "https://api.openai.com/profile":{"email":"fixture@example.invalid"}
        }))
        .unwrap(),
    );
    let token = format!("e30.{payload}.synthetic");
    let mut oversized_summary = response_text("Oversized hidden reasoning summary.");
    oversized_summary["events"][1]["data"]["response"]["usage"] = json!({"output_tokens":14_000});
    // Both parallel requests must reach the fixture before either branch fails.
    oversized_summary["delay_ms"] = json!(200);
    let up = upstream(json!([
        {"body":json!({"device_auth_id":"device-fixture","user_code":"CLI-FIXTURE","interval":1}).to_string()},
        {"status":404,"body":"{}"},
        {"body":json!({"authorization_code":"synthetic-code","code_verifier":"synthetic-verifier"}).to_string()},
        {"body":json!({"access_token":token,"refresh_token":"synthetic-refresh","expires_in":3600}).to_string()},
        response_write(), response_text("Written."), response_text("Continued."),
        // Native split cuts issue history/prefix summaries concurrently. Equal
        // branch responses keep the script independent of request arrival order.
        oversized_summary.clone(), oversized_summary,
        response_text("Summary preserves daily.txt."), response_text("Summary preserves daily.txt."),
        response_text("Cleared context."), response_text("Fresh Session."),
        {"body":json!({"plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":11,"limit_window_seconds":18000,"reset_after_seconds":600}}}).to_string()},
        {"body":json!({"plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":600}}}).to_string()},
        {"events":[],"end":"hang"}
    ]))
    .await;
    // Login/logout must succeed independently of broken model configuration.
    let broken = host.home.path().join("agent/models.yml");
    std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
    std::fs::write(&broken, "providers: [").unwrap();
    let mut login = host.command("openai-codex", &["login"]);
    login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    let login = output(login).await;
    success(&login);
    assert!(String::from_utf8_lossy(&login.stderr).contains("CLI-FIXTURE"));
    assert!(!host.sessions.exists());
    host.config("openai-codex-responses", &format!("http://{}", up.addr), "oauth");
    // This controlled account/Responses workflow starts with a fresh real
    // catalog cache. Cold per-account discovery is exercised by the Registry
    // module; synthetic account tokens must not reach the public catalog.
    let cache = ara_cli::model_cache::SqliteModelCache::for_path(host.home.path().join("agent/model-cache.db"));
    let bundled = ara_cli::model_identity_wire::bundled_provider_models(&"openai-codex".into());
    let fingerprint = ara_cli::model_manager::fingerprint_static_models(
        &ara_cli::model_manager::ModelArray::new(bundled.clone()),
        true,
    );
    cache
        .write_model_cache_wire(
            &"openai-codex".into(),
            chrono::Utc::now().timestamp_millis() as f64,
            &[],
            ara_cli::model_cache::WireModelCacheWriteOptions {
                authoritative: true,
                static_fingerprint: &fingerprint,
                static_header_sources: &bundled,
                restorable_header_fallback: None,
            },
        )
        .unwrap();
    // This account workflow serves ordinary Responses and soft summaries.
    // Native remote endpoints have their own grouped Host fixture.
    std::fs::write(host.home.path().join("agent/config.yml"), "compaction:\n  methodOrder: [soft]\n").unwrap();
    let mut first = host.command("openai-codex", &["--mode", "json", "--tools", "write", "write daily.txt"]);
    first.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    let first = success(&output(first).await);
    let original: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
    let session = host.session();
    let mut resumed = host.command(
        "openai-codex",
        &["--mode", "json", "--resume", session.to_str().unwrap(), "--repl", "--compact-keep-tokens", "0"],
    );
    resumed.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    let mut child = tokio::process::Command::from(resumed)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"continue\n/compact\n/compact\n/clear\ncleared turn\n/new\nfresh turn\n/usage\n/usage\n/usage refresh\n/usage\n/exit\n")
        .await
        .unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await.unwrap().unwrap();
    let stdout = success(&resumed);
    let continued: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(continued["id"], original["id"]);
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("summary persisted"));
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("local adoption budget"));
    assert_eq!(String::from_utf8_lossy(&resumed.stderr).matches("11.00% used (89.0% left)").count(), 2);
    assert_eq!(String::from_utf8_lossy(&resumed.stderr).matches("12.00% used (88.0% left)").count(), 2);
    assert_eq!(std::fs::read_to_string(host.work.path().join("daily.txt")).unwrap(), "daily proof\n");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 15, "warm /usage reuses the full report; explicit refresh starts one new GET");
    for request in &requests[13..15] {
        assert_eq!(request["request"], "GET /backend-api/wham/usage HTTP/1.1");
        assert_eq!(request["headers"]["chatgpt-account-id"], "synthetic-account");
    }
    for request in &requests[4..11] {
        assert_eq!(request["request"], "POST /codex/responses HTTP/1.1");
        assert_eq!(request["headers"]["chatgpt-account-id"], "synthetic-account");
        assert_eq!(request["headers"]["session_id"], original["id"]);
        assert_eq!(request["body"]["store"], false);
        assert!(request["body"].get("max_output_tokens").is_none());
        assert!(request["body"].get("temperature").is_none());
    }
    let fresh: Value = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|value| value["type"] == "session" && value["id"] != original["id"])
        .unwrap();
    let runtime = requests[11]["headers"]["session_id"].as_str().unwrap();
    assert_ne!(runtime, original["id"].as_str().unwrap());
    assert_ne!(runtime, fresh["id"].as_str().unwrap());
    for name in ["session_id", "conversation_id", "x-client-request-id"] {
        assert_eq!(requests[11]["headers"][name], runtime);
        assert_eq!(requests[12]["headers"][name], fresh["id"]);
    }
    assert_eq!(requests[11]["body"]["prompt_cache_key"], runtime);
    assert!(!requests[11]["body"].to_string().contains("daily.txt"));
    assert_eq!(requests[12]["body"]["prompt_cache_key"], fresh["id"]);
    drop(requests);
    let journal = std::fs::read_to_string(session).unwrap();
    assert!(journal.contains("openai-codex-responses"));
    assert!(journal.contains("\"type\":\"compaction\""));
    assert!(journal.contains("\"type\":\"reset_boundary\""));
    assert!(!journal.contains("Oversized hidden reasoning summary."));
    assert!(!journal.contains("synthetic-refresh"));
    assert!(!journal.contains(&token));
    std::fs::write(broken, "providers: [").unwrap();
    let mut logout = host.command("openai-codex", &["logout"]);
    logout.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    success(&output(logout).await);
    host.config("openai-codex-responses", &format!("http://{}", up.addr), "oauth");
    let mut denied = host.command("openai-codex", &["after logout"]);
    denied.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    let denied = output(denied).await;
    assert_eq!(denied.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&denied.stderr).contains("ara login"));
    assert_eq!(up.served(), 15, "logout and rejected post-logout turn add no request after the two usage GETs");

    // Expired account preflight runs before the turn's --max-time deadline.
    // Its own OAuth transport timeout must settle before normal process exit;
    // a new process must not automatically replay the unknown refresh grant.
    let database = host.home.path().join("agent/auth.db");
    {
        use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
        let store = SqliteCredentialStore::open(&database).unwrap();
        let mut fields = serde_json::Map::new();
        fields.insert("access".into(), json!(token));
        fields.insert("refresh".into(), json!("synthetic-cancelled-refresh"));
        fields.insert("expires".into(), json!(1));
        fields.insert("authorizedAt".into(), json!(chrono::Utc::now().timestamp_millis()));
        fields.insert("accountId".into(), json!("synthetic-account"));
        store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(fields)).unwrap();
    }
    let mut cancelled = host.command("openai-codex", &["--max-time", "0.5", "refresh then cancel"]);
    cancelled.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    assert_eq!(output(cancelled).await.status.code(), Some(1));
    assert_eq!(up.served(), 16);
    let store = ara_cli::credential_store::SqliteCredentialStore::open(&database).unwrap();
    assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
    let mut restarted = host.command("openai-codex", &["after unknown refresh"]);
    restarted.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    assert_eq!(output(restarted).await.status.code(), Some(1));
    assert_eq!(up.served(), 16, "restart must not replay the unknown refresh after the usage GETs");
}
