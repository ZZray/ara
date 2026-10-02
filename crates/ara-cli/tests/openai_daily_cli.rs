//! Module acceptance through the real CLI, sockets, tools and Session journal.
//! Shared protocol fixtures are reused; OAuth values below are synthetic.

use ara_testkit::chunks::{done, finish, text};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

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
                "daily-model",
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
    assert_eq!(requests[0]["body"]["max_tokens"], 128);
    assert_eq!(requests[1]["body"]["max_tokens"], 64);
    assert_eq!(requests[0]["headers"]["x-daily-route"], "configured");
    assert_eq!(requests[1]["headers"]["x-daily-route"], "cli");
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
    assert!(String::from_utf8_lossy(&rpc.stderr).contains("supports print and REPL"));
    assert!(!host.sessions.exists());
    assert!(!host.home.path().join("agent/auth.db").exists());
    assert_eq!(up.served(), 0);
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
        .write_all(b"continue\n/compact\n/compact\n/clear\ncleared turn\n/new\nfresh turn\n/exit\n")
        .await
        .unwrap();
    let resumed = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await.unwrap().unwrap();
    let stdout = success(&resumed);
    let continued: Value = serde_json::from_str(stdout.lines().next().unwrap()).unwrap();
    assert_eq!(continued["id"], original["id"]);
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("summary persisted"));
    assert!(String::from_utf8_lossy(&resumed.stderr).contains("local adoption budget"));
    assert_eq!(std::fs::read_to_string(host.work.path().join("daily.txt")).unwrap(), "daily proof\n");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 13);
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
    assert_eq!(up.served(), 13);

    // A deadline during refresh must settle before normal process exit. A new
    // process must not automatically replay that unknown refresh grant.
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
    assert_eq!(up.served(), 14);
    let store = ara_cli::credential_store::SqliteCredentialStore::open(&database).unwrap();
    assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
    let mut restarted = host.command("openai-codex", &["after unknown refresh"]);
    restarted.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    assert_eq!(output(restarted).await.status.code(), Some(1));
    assert_eq!(up.served(), 14);
}
