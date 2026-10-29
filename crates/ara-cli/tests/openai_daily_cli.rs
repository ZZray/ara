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
            "ARA_NO_STRICT",
            "ARA_TOKENIZER",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_API_KEY",
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            "ARA_TEST_CODEX_CALLBACK_PORT",
            "ARA_TEST_CODEX_CALLBACK_TIMEOUT_MS",
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

impl Host {
    fn thinking_config(&self, endpoint: &str, model_default: bool) {
        let agent = self.home.path().join("agent");
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("config.yml"),
            serde_json::to_vec(&json!({
                "defaultThinkingLevel":"auto", "modelRoles":{"tiny":"custom/classifier"},
                "providers":{"autoThinkingModel":"online","autoThinkingMaxEffort":"xhigh"}
            }))
            .unwrap(),
        )
        .unwrap();
        let mut model = json!({"id":"daily-model","reasoning":true,"contextWindow":131072,"maxTokens":8192,
            "thinking":{"mode":"effort","efforts":["low","medium","high","xhigh","max"]}});
        if model_default {
            model["thinking"]["defaultLevel"] = json!("high");
        }
        std::fs::write(
            agent.join("models.yml"),
            serde_json::to_vec(&json!({"providers":{"custom":{
                "api":"openai-completions","baseUrl":endpoint,"auth":"none","compat":{"maxTokensField":"max_tokens"},
                "models":[model,{"id":"classifier","maxTokens":4096,"reasoning":false}]
            }}}))
            .unwrap(),
        )
        .unwrap();
    }
}

fn chat_answer(answer: &str) -> Value {
    json!({"events":[text(answer),finish("stop"),
        {"data":{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1,"total_tokens":4}}},done()]})
}

async fn line_output(command: Command, input: &str) -> Output {
    use tokio::io::AsyncWriteExt;
    let mut command = tokio::process::Command::from(command);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input.as_bytes()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await.expect("bounded line host").unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_cli_fixed_initial_resume_and_native_flag_precedence() {
    for repl in [false, true] {
        let host = Host::new();
        let up = upstream(json!([
            chat_answer("first"),
            chat_answer("saved"),
            chat_answer("override"),
            chat_answer("still saved")
        ]))
        .await;
        host.thinking_config(&up.base_url(), true);
        let settings = std::fs::read(host.home.path().join("agent/config.yml")).unwrap();
        let initial = if repl {
            line_output(
                host.command("custom", &["--repl", "--thinking", "me", "--thinking", "inherit"]),
                "first User\n/exit\n",
            )
            .await
        } else {
            output(host.command("custom", &["--thinking", "me", "--thinking", "inherit", "first User"])).await
        };
        success(&initial);
        assert!(String::from_utf8_lossy(&initial.stderr).contains("invalid --thinking"));
        let path = host.session();
        let thinking_entries = |path: &std::path::Path| {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter(|entry| entry["type"] == "thinking_level_change")
                .collect::<Vec<_>>()
        };
        let saved = thinking_entries(&path);
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0]["thinkingLevel"], "medium");
        for (value, expected) in [(None, "medium"), (Some("hi"), "high"), (None, "medium")] {
            let mut command = host.command("custom", &["--resume", path.to_str().unwrap()]);
            if let Some(value) = value {
                command.args(["--thinking", value]);
            }
            let answer = if repl {
                command.arg("--repl");
                line_output(command, "resumed User\n/exit\n").await
            } else {
                command.arg("resumed User");
                output(command).await
            };
            success(&answer);
            assert_eq!(up.requests.lock().await.last().unwrap()["body"]["reasoning_effort"], expected);
            assert_eq!(thinking_entries(&path), saved, "resume keeps original selection receipts");
        }
        assert_eq!(std::fs::read(host.home.path().join("agent/config.yml")).unwrap(), settings);
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 4);
        assert!(requests.iter().all(|request| request["body"]["model"] == "daily-model"));
        assert_eq!(requests[0]["body"]["reasoning_effort"], "medium");
    }
    // The selected model's default precedes a global Auto setting.
    let host = Host::new();
    let up = upstream(json!([chat_answer("model default")])).await;
    host.thinking_config(&up.base_url(), true);
    success(&output(host.command("custom", &["model default User"])).await);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["body"]["reasoning_effort"], "high", "model default precedes global Auto");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_cli_auto_actual_tool_usage_and_last_resolution() {
    use ara_testkit::chunks::tool_call;
    for repl in [false, true] {
        let host = Host::new();
        let up = upstream(json!([
            chat_answer("low"),
            {"events":[tool_call(0,"thinking-write","write",r#"{"path":"thinking.txt","content":"owned"}"#),finish("tool_calls"),done()]},
            chat_answer("wrote owned"), chat_answer("unparseable classification"),chat_answer("kept low"),chat_answer("maximum requested")
        ])).await;
        host.thinking_config(&up.base_url(), !repl);
        let answer = if repl {
            // Global Auto startup (no model default); context/provider reset
            // and /new retain the resolved Auto effort and its fallback.
            line_output(
                host.command("custom", &["--repl"]),
                "first User\n/clear\n/new\nsecond User\nultrathink third User\n/exit\n",
            )
            .await
        } else {
            output(
                host.command("custom", &["--thinking", "auto", "first User", "second User", "ultrathink third User"]),
            )
            .await
        };
        success(&answer);
        assert_eq!(std::fs::read_to_string(host.work.path().join("thinking.txt")).unwrap(), "owned");
        let requests = up.requests.lock().await;
        assert_eq!(
            requests.len(),
            6,
            "repl={repl}; model/effort={:?}; stderr={}",
            requests
                .iter()
                .map(|request| (&request["body"]["model"], &request["body"]["reasoning_effort"]))
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&answer.stderr)
        );
        assert_eq!(requests[0]["body"]["model"], "classifier");
        assert_eq!(requests[3]["body"]["model"], "classifier");
        for index in [1, 2, 4] {
            assert_eq!(requests[index]["body"]["reasoning_effort"], "low");
        }
        assert_eq!(requests[5]["body"]["reasoning_effort"], "max", "ultrathink bypasses classifier ceiling");
        let files = std::fs::read_dir(&host.sessions)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
            .collect::<Vec<_>>();
        let entries = files
            .iter()
            .flat_map(|path| {
                std::fs::read_to_string(path)
                    .unwrap()
                    .lines()
                    .map(|line| serde_json::from_str::<Value>(line).unwrap())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let usage = entries.iter().filter(|entry| entry["type"] == "model_usage").collect::<Vec<_>>();
        assert_eq!(usage.len(), 2);
        assert!(usage.iter().all(|entry| entry["purpose"] == "auto-thinking"
            && entry["model"] == "classifier"
            && entry["usage"]["input"] == 3));
        assert_eq!(
            entries.iter().filter(|entry| entry["type"] == "message" && entry["message"]["role"] == "user").count(),
            3
        );
        assert_eq!(files.len(), if repl { 2 } else { 1 });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thinking_cli_auto_timeout_keeps_provisional_and_primary_task() {
    let host = Host::new();
    let up = upstream(json!([
        {"events":[{"sleep_ms":4500},text("low"),finish("stop"),done()]},chat_answer("primary fallback")
    ]))
    .await;
    host.thinking_config(&up.base_url(), true);
    success(&output(host.command("custom", &["--thinking", "auto", "timeout fallback User"])).await);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["body"]["model"], "classifier");
    assert_eq!(requests[1]["body"]["model"], "daily-model");
    assert_eq!(requests[1]["body"]["reasoning_effort"], "high");
    let journal = std::fs::read_to_string(host.session()).unwrap();
    assert!(journal.contains("timeout fallback User"));
    assert!(
        !journal
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .any(|entry| entry["type"] == "thinking_level_change" && entry["configured"] == "auto"),
        "unchanged provisional classification does not persist an Auto selector"
    );
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
    for (raw, expected) in [(None, false), (Some("true"), true), (Some("True"), false)] {
        let host = Host::new();
        let up = upstream(json!([chat_answer("schema startup")])).await;
        host.config("openai-completions", &up.base_url(), "none");
        // Native loopback Host compat defaults to supportsStrictMode:false.
        // Author support in this fixture; NO_STRICT then controls its wire use.
        let path = host.home.path().join("agent/models.yml");
        let mut config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        config["providers"]["custom"]["compat"] = json!({"supportsStrictMode":true});
        std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut command = host.command("custom", &["--tools", "read", "schema startup User"]);
        if let Some(raw) = raw {
            command.env("ARA_NO_STRICT", raw);
        }
        assert!(success(&output(command).await).contains("schema startup"));
        let requests = up.requests.lock().await;
        assert_eq!(requests.len(), 1, "startup policy never triggers a retry");
        let tool = &requests[0]["body"]["tools"][0]["function"];
        eprintln!("ARA-NO-STRICT CLI_WIRE raw={raw:?} fixture_supports_strict=true tool={tool}");
        assert_eq!(tool["name"], "read");
        assert_eq!(tool.get("strict").and_then(Value::as_bool), if expected { None } else { Some(true) });
        let required = tool["parameters"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("path")));
        assert!(required.contains(&json!("i")), "native Agent injects required concise intent");
        assert_eq!(
            required,
            &if expected { vec![json!("path"), json!("i")] } else { vec![json!("i"), json!("path")] },
            "strict adaptation enumerates intent-first properties; NO_STRICT keeps original required order"
        );
    }
    eprintln!("ARA-NO-STRICT CLI_STARTUP PASS 3/3");
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

/// Fixed bundled capability -> actual CLI/socket -> alias -> EditSession ->
/// native journal -> same-Session restart without a second file effect.
#[tokio::test]
async fn bundled_freeform_custom_edit_socket_and_restart_preserve_original_receipts() {
    let host = Host::new();
    let raw = "*** Begin Patch\n*** Update File: custom.txt\n@@\n-alpha\n+beta 中文\n*** End Patch";
    std::fs::write(host.work.path().join("custom.txt"), "alpha\n").unwrap();
    let up = upstream(json!([
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,
                "item":{"type":"custom_tool_call","id":"ctc_host","call_id":"call_host",
                    "name":"apply_patch","input":""}}},
            {"data":{"type":"response.custom_tool_call_input.delta","output_index":0,"delta":raw}},
            {"data":{"type":"response.custom_tool_call_input.done","output_index":0,"input":raw}},
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"custom_tool_call","id":"ctc_host","call_id":"call_host",
                    "name":"apply_patch","input":raw}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        response_text("Custom edit complete."),
        response_text("Original custom receipt retained.")
    ]))
    .await;
    let config = host.home.path().join("agent/models.yml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    // Override only the route of a fixed bundled donor. Auth is synthetic/keyless;
    // freeform metadata is not invented inside an authored custom definition.
    std::fs::write(
        &config,
        serde_json::to_vec(&json!({"providers":{"openai":{
            "api":"openai-responses", "baseUrl":up.base_url(), "auth":"none", "models":[]
        }}}))
        .unwrap(),
    )
    .unwrap();
    let arguments = [
        "--mode",
        "json",
        "--thinking",
        "off",
        "--tools",
        "edit",
        "--edit-mode",
        "apply_patch",
        "apply the custom edit",
    ];
    let mut command = host.command_for_model("openai", "gpt-5.4", &arguments);
    command.env_remove("OPENAI_API_KEY");
    let first = success(&output(command).await);
    let frames: Vec<Value> = first.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let original = frames.iter().find(|frame| frame["type"] == "session").unwrap()["id"].clone();
    assert!(frames.iter().any(|frame| frame["type"] == "tool_execution_end"
        && frame["toolName"] == "apply_patch"
        && frame["toolCallId"] == "call_host|ctc_host"
        && frame["isError"] == false));
    assert_eq!(std::fs::read_to_string(host.work.path().join("custom.txt")).unwrap(), "beta 中文\n");
    let session = host.session();
    let before = std::fs::read(&session).unwrap();
    let mut command = host.command_for_model(
        "openai",
        "gpt-5.4",
        &[
            "--mode",
            "json",
            "--thinking",
            "off",
            "--tools",
            "edit",
            "--edit-mode",
            "apply_patch",
            "--resume",
            session.to_str().unwrap(),
            "continue from our original custom receipt",
        ],
    );
    command.env_remove("OPENAI_API_KEY");
    let resumed = success(&output(command).await);
    let frames: Vec<Value> = resumed.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(frames.iter().find(|frame| frame["type"] == "session").unwrap()["id"], original);
    assert!(!frames.iter().any(|frame| frame["type"] == "tool_execution_start"));
    assert_eq!(std::fs::read_to_string(host.work.path().join("custom.txt")).unwrap(), "beta 中文\n");
    let after = std::fs::read(&session).unwrap();
    assert!(after.starts_with(&before), "restart only appends to the original journal");
    let entries: Vec<Value> =
        String::from_utf8(after).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let calls: Vec<&Value> = entries
        .iter()
        .filter_map(|entry| entry["message"]["content"].as_array())
        .flatten()
        .filter(|part| part["type"] == "toolCall")
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["name"], "apply_patch");
    assert_eq!(calls[0]["customWireName"], "apply_patch");
    assert_eq!(calls[0]["arguments"]["input"], raw);
    let results: Vec<&Value> = entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["message"]["toolCallId"], "call_host|ctc_host");
    assert_eq!(results[0]["message"]["isError"], false);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["request"], "POST /v1/responses HTTP/1.1");
        assert_eq!(request["body"]["model"], "gpt-5.4");
        assert_eq!(request["body"]["tools"][0]["type"], "custom");
        assert_eq!(request["body"]["tools"][0]["name"], "apply_patch");
    }
    for index in [1, 2] {
        let input = requests[index]["body"]["input"].as_array().unwrap();
        let calls: Vec<&Value> = input.iter().filter(|item| item["type"] == "custom_tool_call").collect();
        let outputs: Vec<&Value> = input.iter().filter(|item| item["type"] == "custom_tool_call_output").collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(outputs.len(), 1);
        assert_eq!(calls[0]["call_id"], "call_host");
        assert_eq!(calls[0]["input"], raw);
        assert_eq!(outputs[0]["call_id"], "call_host");
        assert!(!input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_host"));
    }
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
async fn unsupported_configuration_and_supported_codex_rpc_startup_keep_their_actual_contracts() {
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
    let frames = success(&rpc).lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
    assert!(frames.iter().any(|frame| frame["type"] == "ready"));
    assert_eq!(frames.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    let store = ara_cli::credential_store::SqliteCredentialStore::open(host.home.path().join("agent/auth.db")).unwrap();
    assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
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
    native_codex_compaction_observation_regression().await;
    use base64::Engine as _;
    for browser in [false, true] {
        let host = Host::new();
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "https://api.openai.com/auth":{"chatgpt_account_id":"synthetic-account","chatgpt_data_residency":"eu"},
                "https://api.openai.com/profile":{"email":"fixture@example.invalid"}
            }))
            .unwrap(),
        );
        let token = format!("e30.{payload}.synthetic");
        let mut oversized_summary = codex_account_text_response("Oversized hidden reasoning summary.");
        oversized_summary["events"][2]["data"]["response"]["usage"] = json!({"output_tokens":14_000});
        // Both parallel requests must reach the fixture before either branch fails.
        oversized_summary["delay_ms"] = json!(200);
        // Fixed Codex treats output_item.done arguments as authoritative.
        // Keep the Compatible fixture's conflicting final frame in its own cases.
        let mut codex_write = response_write();
        codex_write["events"][2]["data"]["item"]["arguments"] = codex_write["events"][1]["data"]["arguments"].clone();
        let grant = json!({"body":json!({"access_token":token,"refresh_token":"synthetic-refresh","expires_in":3600}).to_string()});
        let mut replies = if browser {
            vec![grant]
        } else {
            vec![
                json!({"body":json!({"device_auth_id":"device-fixture","user_code":"CLI-FIXTURE","interval":1}).to_string()}),
                json!({"status":404,"body":"{}"}),
                json!({"body":json!({"authorization_code":"synthetic-code","code_verifier":"synthetic-verifier"}).to_string()}),
                grant,
            ]
        };
        let auth_requests = replies.len();
        replies.extend([
            codex_write,
            codex_account_text_response("Written."),
            codex_account_text_response("Continued."),
            // Native split cuts issue history/prefix summaries concurrently. Equal
            // branch responses keep the script independent of request arrival order.
            oversized_summary.clone(),
            oversized_summary,
            codex_account_text_response("Summary preserves daily.txt."),
            codex_account_text_response("Summary preserves daily.txt."),
            codex_account_text_response("Cleared context."),
            codex_account_text_response("Fresh Session."),
            json!({"events":[],"end":"hang"}),
        ]);
        let up = upstream(Value::Array(replies)).await;
        // A valid stored grant lets usage and Responses use independent fixture
        // routes. Background usage may interleave with model/summary calls without
        // consuming an ordered model response. Login/expired refresh still use up.
        let usage = upstream(json!([
        {"body":json!({"plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":11,"limit_window_seconds":18000,"reset_after_seconds":600}}}).to_string()},
        {"body":json!({"plan_type":"pro","rate_limit":{"allowed":true,"limit_reached":false,
            "primary_window":{"used_percent":12,"limit_window_seconds":18000,"reset_after_seconds":600}}}).to_string()},
    ]))
    .await;
        // Login/logout must succeed independently of broken model configuration.
        let broken = host.home.path().join("agent/models.yml");
        std::fs::create_dir_all(broken.parent().unwrap()).unwrap();
        std::fs::write(&broken, "providers: [").unwrap();
        let mut login =
            host.command("openai-codex", if browser { &["login"] } else { &["login", "--method", "device"] });
        login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
        let login = if browser { browser_login_output(login).await } else { output(login).await };
        success(&login);
        if browser {
            assert!(String::from_utf8_lossy(&login.stderr).contains("Waiting for browser authentication"));
        } else {
            assert!(String::from_utf8_lossy(&login.stderr).contains("CLI-FIXTURE"));
        }
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
        first.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", usage.addr));
        let first = success(&output(first).await);
        let original: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
        let session = host.session();
        let mut resumed = host.command(
            "openai-codex",
            &["--mode", "json", "--resume", session.to_str().unwrap(), "--repl", "--compact-keep-tokens", "0"],
        );
        resumed.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", usage.addr));
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
        assert_eq!(
            requests.len(),
            auth_requests + 9,
            "background usage never consumes an auth/model/summary fixture response"
        );
        let usage_requests = usage.requests.lock().await;
        assert_eq!(
            usage_requests.len(),
            2,
            "warm /usage reuses the background full report; explicit refresh starts one new GET"
        );
        for request in usage_requests.iter() {
            assert_eq!(request["request"], "GET /backend-api/wham/usage HTTP/1.1");
            assert_eq!(request["headers"]["chatgpt-account-id"], "synthetic-account");
        }
        drop(usage_requests);
        for request in &requests[auth_requests..auth_requests + 7] {
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
        let runtime = requests[auth_requests + 7]["headers"]["session_id"].as_str().unwrap();
        assert_ne!(runtime, original["id"].as_str().unwrap());
        assert_ne!(runtime, fresh["id"].as_str().unwrap());
        for name in ["session_id", "conversation_id", "x-client-request-id"] {
            assert_eq!(requests[auth_requests + 7]["headers"][name], runtime);
            assert_eq!(requests[auth_requests + 8]["headers"][name], fresh["id"]);
        }
        assert_eq!(requests[auth_requests + 7]["body"]["prompt_cache_key"], runtime);
        assert!(!requests[auth_requests + 7]["body"].to_string().contains("daily.txt"));
        assert_eq!(requests[auth_requests + 8]["body"]["prompt_cache_key"], fresh["id"]);
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
        assert_eq!(up.served(), auth_requests + 9, "logout and rejected post-logout turn add no auth/model request");
        assert_eq!(usage.served(), 2, "logout and rejection add no usage GET");

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
        assert_eq!(up.served(), auth_requests + 10);
        let store = ara_cli::credential_store::SqliteCredentialStore::open(&database).unwrap();
        assert!(store.list_auth_credentials(Some("openai-codex")).unwrap().is_empty());
        let mut restarted = host.command("openai-codex", &["after unknown refresh"]);
        restarted.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
        assert_eq!(output(restarted).await.status.code(), Some(1));
        assert_eq!(up.served(), auth_requests + 10, "restart must not replay the unknown refresh");
        assert_eq!(usage.served(), 2);
        if browser {
            browser_cli_explicit_method_and_failure_owners(&token).await;
        }
    }
    email_only_cli_login_tool_readback_refresh_restart_and_logout().await;
}

#[cfg(feature = "test-fixture")]
async fn browser_login_output(mut command: Command) -> Output {
    use tokio::io::AsyncBufReadExt;
    command.env("ARA_TEST_CODEX_CALLBACK_PORT", "0").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut child = tokio::process::Command::from(command).kill_on_drop(true).spawn().unwrap();
        let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
        let mut captured = String::new();
        let url = loop {
            let line = lines.next_line().await.unwrap().expect("browser URL before exit");
            captured.push_str(&line);
            captured.push('\n');
            if let Some(url) = line.strip_prefix("ara: http://").filter(|value| value.contains("/oauth/authorize?")) {
                break reqwest::Url::parse(&format!("http://{url}")).unwrap();
            }
        };
        let parameters: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(parameters["originator"], "omp");
        assert_eq!(parameters["code_challenge_method"], "S256");
        let redirect = reqwest::Url::parse(&parameters["redirect_uri"]).unwrap();
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let launch = http.get(format!("http://localhost:{}/launch", redirect.port().unwrap())).send().await.unwrap();
        assert_eq!(launch.status(), 302, "active launch URL published before Host notification");
        assert_eq!(launch.headers()["location"], url.as_str());
        let invalid =
            http.get(redirect.clone()).query(&[("code", "bad"), ("state", "wrong-state")]).send().await.unwrap();
        assert_eq!(invalid.status(), 500);
        let _ = invalid.text().await.unwrap();
        let accepted = http
            .get(redirect.clone())
            .query(&[("code", "synthetic-code"), ("state", parameters["state"].as_str())])
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), 200);
        let html = accepted.text().await.unwrap();
        assert!(html.trim_end().ends_with("</html>"), "complete success feedback before process exits");
        while let Some(line) = lines.next_line().await.unwrap() {
            captured.push_str(&line);
            captured.push('\n');
        }
        let mut result = child.wait_with_output().await.unwrap();
        result.stderr = captured.into_bytes();
        assert!(http.get(redirect).send().await.is_err(), "process releases its callback listeners");
        result
    })
    .await
    .expect("bounded browser login Host process")
}

#[cfg(feature = "test-fixture")]
async fn browser_cli_explicit_method_and_failure_owners(token: &str) {
    let host = Host::new();
    let up = upstream(json!([{ "body":json!({"access_token":token,"refresh_token":"explicit-browser-refresh","expires_in":3600}).to_string() }])).await;
    let mut login = host.command("openai-codex", &["login", "--method", "browser"]);
    login.env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", up.addr));
    success(&browser_login_output(login).await);
    assert_eq!(up.served(), 1);
    let database = host.home.path().join("agent/auth.db");
    let before = ara_cli::credential_store::SqliteCredentialStore::open(&database)
        .unwrap()
        .list_auth_credentials(Some("openai-codex"))
        .unwrap();
    assert_eq!(before.len(), 1);
    let empty = upstream(json!([])).await;
    let blocker = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let mut blocked = host.command("openai-codex", &["login", "--method", "browser"]);
    blocked
        .env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", empty.addr))
        .env("ARA_TEST_CODEX_CALLBACK_PORT", blocker.local_addr().unwrap().port().to_string());
    let blocked = output(blocked).await;
    assert_eq!(blocked.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&blocked.stderr).contains("callback port"));
    assert!(!String::from_utf8_lossy(&blocked.stderr).contains("/oauth/authorize?"));
    drop(blocker);
    let mut timed = host.command("openai-codex", &["login"]);
    timed
        .env("ARA_TEST_CODEX_AUTH_BASE_URL", format!("http://{}", empty.addr))
        .env("ARA_TEST_CODEX_CALLBACK_PORT", "0")
        .env("ARA_TEST_CODEX_CALLBACK_TIMEOUT_MS", "25");
    let timed = output(timed).await;
    assert_eq!(timed.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&timed.stderr);
    assert!(stderr.contains("authentication timed out"));
    let url = stderr
        .lines()
        .find_map(|line| line.strip_prefix("ara: http://").filter(|value| value.contains("/oauth/authorize?")))
        .map(|value| reqwest::Url::parse(&format!("http://{value}")).unwrap())
        .unwrap();
    let redirect = url.query_pairs().find(|(key, _)| key == "redirect_uri").unwrap().1.into_owned();
    assert!(reqwest::Client::new().get(redirect).send().await.is_err());
    let after = ara_cli::credential_store::SqliteCredentialStore::open(&database)
        .unwrap()
        .list_auth_credentials(Some("openai-codex"))
        .unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, before[0].id);
    assert!(after[0].credential == before[0].credential, "failed issuance preserves the saved grant");
    assert_eq!(empty.served(), 0, "no token exchange, retry or automatic device fallback");
    assert!(!host.sessions.exists());
}


// Same ordinary executable and shared SQLite owner as the identified workflow.
// These loopback OAuth/SSE receipts cannot establish live OpenAI authorization.
#[cfg(feature = "test-fixture")]
async fn email_only_cli_login_tool_readback_refresh_restart_and_logout() {
    use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
    use base64::Engine as _;
    let jwt = |payload: Value| {
        format!(
            "fixture.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.to_string())
        )
    };
    let usage_reply = json!({"body":json!({"plan_type":"team","rate_limit":{
        "allowed":true,"limit_reached":false,"primary_window":{
            "used_percent":1,"limit_window_seconds":18000,"reset_after_seconds":600
        }}}).to_string()});
    for browser in [false, true] {
        let host = Host::new();
        let token = jwt(json!({"sub":"email-user","https://api.openai.com/auth":{
            "user_id":"email-user","poid":"not-a-workspace","chatgpt_plan_type":" TEAM ","chatgpt_data_residency":"eu"
        },"https://api.openai.com/profile":{"email":" CLI.Email@EXAMPLE.com "}}));
        let filled = jwt(json!({"https://api.openai.com/auth":{
            "chatgpt_account_id":"cli-filled-workspace","chatgpt_data_residency":"us"
        }}));
        let grant = json!({"body":json!({"access_token":token,"refresh_token":"email-cli-refresh","expires_in":3600}).to_string()});
        let login_replies = if browser {
            vec![grant]
        } else {
            vec![
                json!({"body":json!({"device_auth_id":"email-device","user_code":"EMAIL-CLI","interval":-3}).to_string()}),
                json!({"body":json!({"authorization_code":"email-code","code_verifier":"email-verifier"}).to_string()}),
                grant,
            ]
        };
        let issuer = upstream(Value::Array(login_replies)).await;
        let mut login = host.command(
            "openai-codex",
            if browser {
                &["login"]
            } else {
                &["login", "--method", "device"]
            },
        );
        login.env(
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            format!("http://{}", issuer.addr),
        );
        let login = if browser {
            browser_login_output(login).await
        } else {
            output(login).await
        };
        success(&login);
        assert_eq!(issuer.served(), if browser { 1 } else { 3 });
        assert!(!host.sessions.exists());
        let database = host.home.path().join("agent/auth.db");
        let store = SqliteCredentialStore::open(&database).unwrap();
        let rows = store.list_auth_credentials(Some("openai-codex")).unwrap();
        assert_eq!(rows.len(), 1);
        let id = rows[0].id;
        let AuthCredential::OAuth { fields } = &rows[0].credential else {
            panic!("OAuth")
        };
        assert_eq!(fields["email"], "cli.email@example.com");
        assert_eq!(fields["orgName"], "team");
        assert!(!fields.contains_key("accountId"));
        assert!(!fields.contains_key("orgId"));
        let mut write = response_write();
        write["events"][2]["data"]["item"]["arguments"] =
            write["events"][1]["data"]["arguments"].clone();
        let read = json!({"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{
                "type":"function_call","id":"fc_read","call_id":"call_read","name":"read"}}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{
                "type":"function_call","id":"fc_read","call_id":"call_read","name":"read","arguments":"{\"path\":\"daily.txt\"}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]});
        let models = upstream(json!([
            write,
            codex_account_text_response("Email wrote."),
            read,
            codex_account_text_response("Email read."),
            codex_account_text_response("Refreshed original Session.")
        ]))
        .await;
        let usage = upstream(Value::Array(vec![usage_reply.clone(); 8])).await;
        host.config(
            "openai-codex-responses",
            &format!("http://{}", models.addr),
            "oauth",
        );
        // Account auth is real SQLite; only model metadata is warmed so a
        // synthetic access token cannot escape to the public catalog.
        let cache = ara_cli::model_cache::SqliteModelCache::for_path(
            host.home.path().join("agent/model-cache.db"),
        );
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
        let mut first = host.command(
            "openai-codex",
            &["--mode", "json", "--tools", "write", "write daily.txt"],
        );
        first.env(
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            format!("http://{}", usage.addr),
        );
        let first = success(&output(first).await);
        let original: Value = serde_json::from_str(first.lines().next().unwrap()).unwrap();
        let session = host.session();
        assert_eq!(
            std::fs::read_to_string(host.work.path().join("daily.txt")).unwrap(),
            "daily proof\n"
        );
        let mut reopened = host.command(
            "openai-codex",
            &[
                "--mode",
                "json",
                "--resume",
                session.to_str().unwrap(),
                "--tools",
                "read",
                "read daily.txt",
            ],
        );
        reopened.env(
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            format!("http://{}", usage.addr),
        );
        let reopened = success(&output(reopened).await);
        let resumed: Value = serde_json::from_str(reopened.lines().next().unwrap()).unwrap();
        assert_eq!(resumed["id"], original["id"]);
        let requests = models.requests.lock().await;
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert!(request["headers"].get("chatgpt-account-id").is_none());
            assert_eq!(
                request["headers"]["authorization"],
                format!("<redacted {} chars>", token.len() + 7)
            );
            assert_eq!(
                request["headers"]["x-openai-internal-codex-residency"],
                "eu"
            );
            assert_eq!(request["headers"]["version"], "0.159.0");
            assert_eq!(request["headers"]["session_id"], original["id"]);
        }
        assert!(
            requests[1]["body"].to_string().contains("call_daily"),
            "actual WRITE tool receipt returned to model"
        );
        assert!(
            requests[3]["body"].to_string().contains("daily proof"),
            "actual READ readback returned to model"
        );
        drop(requests);
        for request in usage.requests.lock().await.iter() {
            assert_eq!(request["request"], "GET /backend-api/wham/usage HTTP/1.1");
            assert!(request["headers"].get("chatgpt-account-id").is_none());
        }
        // Only expire this row. Next ordinary process must refresh that grant,
        // fill its real workspace, preserve its email/plan and original Session.
        let row = store
            .list_auth_credentials(Some("openai-codex"))
            .unwrap()
            .remove(0);
        let AuthCredential::OAuth { mut fields } = row.credential else {
            panic!("OAuth")
        };
        fields.insert("expires".into(), json!(1));
        assert!(
            store
                .try_update_auth_credential_if_matches(
                    id,
                    &row.serialized_data,
                    &AuthCredential::oauth(fields),
                    None
                )
                .unwrap()
        );
        let refresh = upstream(Value::Array(
            std::iter::once(json!({"body":json!({
            "access_token":filled,"refresh_token":"email-cli-rotated","expires_in":3600
        }).to_string()}))
            .chain(std::iter::repeat_n(usage_reply.clone(), 8))
            .collect(),
        ))
        .await;
        let mut refreshed = host.command(
            "openai-codex",
            &[
                "--mode",
                "json",
                "--resume",
                session.to_str().unwrap(),
                "after refresh",
            ],
        );
        refreshed.env(
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            format!("http://{}", refresh.addr),
        );
        let refreshed = success(&output(refreshed).await);
        let resumed: Value = serde_json::from_str(refreshed.lines().next().unwrap()).unwrap();
        assert_eq!(resumed["id"], original["id"]);
        let rows = store.list_auth_credentials(Some("openai-codex")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, id);
        let AuthCredential::OAuth { fields } = &rows[0].credential else {
            panic!("OAuth")
        };
        assert_eq!(fields["email"], "cli.email@example.com");
        assert_eq!(fields["orgName"], "team");
        assert_eq!(fields["accountId"], "cli-filled-workspace");
        assert!(!fields.contains_key("orgId"));
        let requests = refresh.requests.lock().await;
        assert!(
            requests[0]["request"]
                .as_str()
                .unwrap()
                .starts_with("POST /oauth/token ")
        );
        assert!(
            requests[0]["body"]
                .as_str()
                .unwrap()
                .contains("refresh_token=email-cli-refresh")
        );
        for request in &requests[1..] {
            assert_eq!(request["request"], "GET /backend-api/wham/usage HTTP/1.1");
            assert_eq!(
                request["headers"]["chatgpt-account-id"],
                "cli-filled-workspace"
            );
        }
        drop(requests);
        let requests = models.requests.lock().await;
        assert_eq!(requests.len(), 5);
        assert_eq!(
            requests[4]["headers"]["chatgpt-account-id"],
            "cli-filled-workspace"
        );
        assert_eq!(
            requests[4]["headers"]["authorization"],
            format!("<redacted {} chars>", filled.len() + 7)
        );
        assert_eq!(requests[4]["headers"]["session_id"], original["id"]);
        drop(requests);
        let journal = std::fs::read_to_string(&session).unwrap();
        assert!(journal.contains("call_read"));
        assert!(journal.contains("call_daily"));
        for secret in [
            &token,
            &filled,
            &"email-cli-refresh".to_owned(),
            &"email-cli-rotated".to_owned(),
        ] {
            assert!(!journal.contains(secret));
        }
        success(&output(host.command("openai-codex", &["logout"])).await);
        let before = models.served();
        let auth_before = refresh.served();
        let mut denied = host.command("openai-codex", &["after logout"]);
        denied.env(
            "ARA_TEST_CODEX_AUTH_BASE_URL",
            format!("http://{}", refresh.addr),
        );
        assert_eq!(output(denied).await.status.code(), Some(1));
        assert_eq!(models.served(), before);
        assert_eq!(refresh.served(), auth_before);
        assert!(
            store
                .list_auth_credentials(Some("openai-codex"))
                .unwrap()
                .is_empty()
        );
    }
}

// This local guard extends the existing real CLI account workflow. The actual
// Responses router constructs and settles its private tool-call Symbol; no
// typed substitute is used to manufacture the offending native message.
#[cfg(feature = "test-fixture")]
async fn native_codex_compaction_observation_regression() {
    use ara_ai::providers::openai_codex_responses::{self as codex, StreamOptions};
    use ara_ai::providers::openai_responses::actual::GraphResponsesOptions;
    use ara_cli::{actual_session::GraphJournalEncoder, native_realm::NativeRealm};
    use ara_rpc::value_heap::ValueSnapshot;
    use ara_session::{NativeJournalEncoder, NativeMessageSource, SessionJournal};
    use std::sync::Arc;

    let realm = NativeRealm::prepare().unwrap();
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    let model = ara_ai::Model {
        id: "native-compaction-guard".into(),
        provider: "openai-codex".into(),
        api: "openai-codex-responses".into(),
        base_url: "http://fixture.invalid/backend-api".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let mut response = response_write();
    response["events"][2]["data"]["item"]["arguments"] = response["events"][1]["data"]["arguments"].clone();
    let up = upstream(json!([response])).await;
    let mut routed = model.clone();
    routed.base_url = up.base_url();
    let body = heap.import_serde(&json!({"model":routed.id,"input":[],"stream":true,"store":false})).unwrap();
    let mut options = StreamOptions {
        api_key: Some("synthetic-native-guard".into()),
        first_event_timeout: Some(Duration::from_secs(2)),
        idle_timeout: Some(Duration::from_secs(2)),
        ..Default::default()
    };
    options.retry.max_attempts = 1;
    let mut stream = codex::stream_actual_prepared(
        reqwest::Client::new(), routed, body, realm.values.factory().clone(), options, GraphResponsesOptions::default(),
    )
    .unwrap();
    let receipt = stream.failure_receipt();
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await { events.push(event); }
        events
    }).await.expect("native compaction guard SSE drain");
    drop(stream);
    let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled()).await.expect("native guard settlement");
    assert!(failures.is_empty(), "{failures:?}");
    assert!(matches!(events.last(), Some(ara_ai::ActualAssistantEvent::Done { reason: ara_ai::StopReason::ToolUse, .. })));
    assert_eq!(up.served(), 1);
    let root = events.last().unwrap().partial().clone();
    let content = heap.get(&root, &"content".into(), runtime).unwrap();
    let block = heap.get(&content, &"0".into(), runtime).unwrap();
    let arguments = heap.get(&block, &"arguments".into(), runtime).unwrap();
    let partial = heap.module_symbol("ara-ai.block-symbols", "provider.block.partialJson").unwrap();
    assert!(heap.has_own(&block, &partial, runtime).unwrap());
    let settled_partial = heap.get(&block, &partial, runtime).unwrap();
    assert!(matches!(heap.snapshot(&settled_partial).unwrap(), ValueSnapshot::Undefined));

    let encoder = Arc::new(GraphJournalEncoder::new(realm.values.clone(), None).unwrap());
    let source = NativeMessageSource::live(root.clone(), encoder.clone());
    assert!(source.checked_body().is_err(), "the strict raw bridge still rejects private Symbols");
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let id = journal.append_native_message(source).unwrap();
    assert!(journal.entries().last().unwrap().checked_raw().is_err());
    let snapshot = journal.native_projected_compaction_snapshot_for_route(&model, true).unwrap();
    let group = snapshot.entries.iter().find(|entry| entry.entry_id == id).unwrap();
    let ara_ai::Message::Assistant(message) = &group.messages[0] else { panic!("assistant compaction fragment") };
    assert_eq!(message.stop_reason, ara_ai::StopReason::ToolUse);
    assert_eq!(message.tool_calls().next().unwrap().arguments["path"], "daily.txt");
    assert_eq!(message.tool_calls().next().unwrap().arguments["content"], "daily proof\n");
    assert_eq!(serde_json::to_value(&message.usage).unwrap(), json!({}), "unknown usage is retained");
    assert_eq!(message.provider_payload.as_ref().unwrap()["items"][0]["id"], "fc_daily");
    assert_eq!(group.raw_token_message.as_ref().unwrap(), &group.messages[0]);
    assert!(journal.entries().last().unwrap().checked_raw().is_err(), "snapshot does not relax raw observation");
    let current = journal.entries().last().unwrap().native_message.as_ref().unwrap().live_body().unwrap();
    assert!(root.same_identity(current));
    assert!(block.same_identity(&heap.get(&content, &"0".into(), runtime).unwrap()));
    assert!(arguments.same_identity(&heap.get(&block, &"arguments".into(), runtime).unwrap()));
    assert!(settled_partial.same_identity(&heap.get(&block, &partial, runtime).unwrap()));

    // A selected argument remains a strict domain boundary. Do not drop it,
    // stringify it, or replace it with zero/empty data to pass compaction.
    heap.define_own_data(&arguments, "nativeOnly".into(), heap.bigint_decimal("17").unwrap(), runtime).unwrap();
    assert!(journal.native_projected_compaction_snapshot_for_route(&model, true).is_err());
    assert!(heap.delete_own(&arguments, &"nativeOnly".into(), runtime).unwrap());
    assert!(journal.native_projected_compaction_snapshot_for_route(&model, true).is_ok());

    // Match the actual emit_result shape after a normal Write: its optional
    // providerMetadata is an own Undefined even though JSON persistence omits it.
    let tool_result = heap.import_serde(&json!({
        "role":"toolResult", "toolCallId":"call_daily|fc_daily", "toolName":"write",
        "content":[{"type":"text","text":"Successfully wrote 12 bytes to daily.txt"}],
        "details":{"resolvedPath":"daily.txt"}, "useless":true, "isError":false, "timestamp":2
    })).unwrap();
    heap.define_own_data(&tool_result, "providerMetadata".into(), heap.undefined().unwrap(), runtime).unwrap();
    let original_details = heap.get(&tool_result, &"details".into(), runtime).unwrap();
    let tool_source = NativeMessageSource::live(tool_result.clone(), encoder.clone());
    assert!(tool_source.checked_body().is_err(), "own Undefined remains outside the strict raw domain");
    let tool_entry_id = journal.append_native_message(tool_source).unwrap();
    assert!(journal.entries().last().unwrap().checked_raw().is_err());
    let snapshot = journal.native_projected_compaction_snapshot_for_route(&model, true).unwrap();
    let tool_group = snapshot.entries.iter().find(|entry| entry.entry_id == tool_entry_id).unwrap();
    let ara_ai::Message::ToolResult(result) = &tool_group.messages[0] else { panic!("tool-result compaction fragment") };
    assert_eq!(result.tool_call_id, "call_daily|fc_daily");
    assert_eq!(result.tool_name, "write");
    assert_eq!(result.details, Some(json!({"resolvedPath":"daily.txt"})));
    assert!(result.useless, "compaction DTO must retain this consumed field");
    assert!(result.provider_metadata.is_none());
    assert!(!result.is_error);
    assert_eq!(tool_group.raw_token_message.as_ref().unwrap(), &tool_group.messages[0]);
    assert!(journal.entries().last().unwrap().checked_raw().is_err());
    let retained = journal.entries().last().unwrap().native_message.as_ref().unwrap().live_body().unwrap();
    assert!(tool_result.same_identity(retained));
    assert!(original_details.same_identity(&heap.get(&tool_result, &"details".into(), runtime).unwrap()));
    assert!(heap.has_own(&tool_result, &"providerMetadata".into(), runtime).unwrap());
    assert!(matches!(heap.snapshot(&heap.get(&tool_result, &"providerMetadata".into(), runtime).unwrap()).unwrap(), ValueSnapshot::Undefined));
    heap.define_own_data(&original_details, "nativeOnly".into(), heap.bigint_decimal("19").unwrap(), runtime).unwrap();
    assert!(journal.native_projected_compaction_snapshot_for_route(&model, true).is_err(), "selected details cannot be dropped to accept unsupported data");
    assert!(heap.delete_own(&original_details, &"nativeOnly".into(), runtime).unwrap());
    assert!(journal.native_projected_compaction_snapshot_for_route(&model, true).is_ok());

    // Other roles and custom envelopes retain their original strict behavior.
    let custom = heap.import_serde(&json!({"role":"custom","customType":"guard","content":"kept", "timestamp":1})).unwrap();
    heap.define_own_data(&custom, partial.clone(), heap.undefined().unwrap(), runtime).unwrap();
    assert!(encoder.observe_compaction_message_body(&custom).is_err());
    let mut custom_journal = SessionJournal::in_memory(json!({"type":"session","version":3,"id":"custom-strict-guard","timestamp":"2026-10-14T00:00:00.000Z","cwd":"."})).unwrap();
    custom_journal.append_native_message(NativeMessageSource::live(custom, encoder)).unwrap();
    assert_eq!(custom_journal.entries()[0].kind, "custom_message");
    assert!(custom_journal.native_projected_compaction_snapshot().is_err());
}

// Only the account workflow uses this registered Graph message fixture.
#[cfg(feature = "test-fixture")]
fn codex_account_text_response(message: &str) -> Value {
    let mut response = response_text(message);
    response["events"].as_array_mut().unwrap().insert(0, json!({"data":{
        "type":"response.output_item.added","output_index":0,"item":{
            "type":"message","id":"msg_daily","role":"assistant","status":"in_progress","content":[]
        }
    }}));
    response
}
