//! Grouped models.yml command-value acceptance through the actual Rust CLI.
//! Source contract: OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/coding-agent/test/model-registry-command-values.test.ts (MIT).
//! All credentials and helper outputs are synthetic and isolated per process.

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

struct Host {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
    launch: tempfile::TempDir,
    models: PathBuf,
    sessions: PathBuf,
}

impl Host {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-command-home-").tempdir().unwrap();
        let project = tempfile::Builder::new().prefix("ara-command-project-").tempdir().unwrap();
        let launch = tempfile::Builder::new().prefix("ara-command-launch-").tempdir().unwrap();
        let models = home.path().join("models.json");
        let sessions = home.path().join("sessions");
        Self { home, project, launch, models, sessions }
    }

    fn config(&self, provider: &str, value: Value) {
        std::fs::write(&self.models, serde_json::to_vec(&json!({"providers":{provider:value}})).unwrap()).unwrap();
    }

    fn command(&self, provider: &str, args: &[&str]) -> Command {
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
            "CMD_CLI_FIXTURE_KEY",
        ] {
            command.env_remove(name);
        }
        command
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .current_dir(self.launch.path())
            .args(["--models-config"])
            .arg(&self.models)
            .args([
                "--provider",
                provider,
                "--model",
                "command-model",
                "--no-skills",
                "--max-model-calls",
                "4",
                "--compact-threshold",
                "0",
                "--session-dir",
            ])
            .arg(&self.sessions)
            .args(args)
            .stdin(Stdio::null());
        command
    }

    fn session(&self) -> PathBuf {
        std::fs::read_dir(&self.sessions)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .unwrap()
    }

    fn assert_private(&self, output: &Output, values: &[&str]) {
        let mut public = String::from_utf8_lossy(&output.stdout).into_owned();
        public.push_str(&String::from_utf8_lossy(&output.stderr));
        if let Ok(entries) = std::fs::read_dir(&self.sessions) {
            for entry in entries.flatten() {
                public.push_str(&std::fs::read_to_string(entry.path()).unwrap());
            }
        }
        for value in values {
            assert!(!public.contains(value), "private configuration appeared in public output or journal");
        }
    }
}

/// Native shell helpers need no Node/Python installation. A shared command's
/// counter and cwd receipts distinguish eager/cache behavior from wire retries.
struct Helper {
    command: String,
    token: PathBuf,
    count: PathBuf,
    cwd: PathBuf,
}

impl Helper {
    fn new(host: &Host, name: &str, value: &str) -> Self {
        let token = host.home.path().join(format!("{name}.token"));
        let count = host.home.path().join(format!("{name}.count"));
        let cwd = host.home.path().join(format!("{name}.cwd"));
        std::fs::write(&token, format!("{value}\n")).unwrap();
        #[cfg(windows)]
        let body = format!(
            "@echo off\r\n>>\"{}\" echo run\r\ncd >>\"{}\"\r\nset /p CMD_VALUE=<\"{}\"\r\nif \"%CMD_VALUE%\"==\"FAIL\" (\r\n echo config-command-private-stdout\r\n echo config-command-private-stderr 1>&2\r\n exit /b 1\r\n)\r\nif \"%CMD_VALUE%\"==\"EMPTY\" exit /b 0\r\necho %CMD_VALUE%\r\n",
            count.display(),
            cwd.display(),
            token.display(),
        );
        #[cfg(not(windows))]
        let body = format!(
            "printf 'run\\n' >> {}; pwd >> {}; IFS= read -r value < {}; case \"$value\" in FAIL) printf %s config-command-private-stdout; printf %s config-command-private-stderr >&2; exit 1;; EMPTY) exit 0;; *) printf %s \"$value\";; esac\n",
            quote(&count),
            quote(&cwd),
            quote(&token),
        );
        Self { command: script(host, name, &body), token, count, cwd }
    }

    fn rotate(&self, value: &str) {
        std::fs::write(&self.token, format!("{value}\n")).unwrap();
    }

    fn executions(&self) -> usize {
        std::fs::read_to_string(&self.count).unwrap_or_default().lines().count()
    }

    fn assert_cwd(&self, expected: &Path) {
        let expected = std::fs::canonicalize(expected).unwrap();
        for actual in std::fs::read_to_string(&self.cwd).unwrap().lines() {
            assert_eq!(std::fs::canonicalize(actual.trim()).unwrap(), expected);
        }
    }
}

#[cfg(not(windows))]
fn quote(path: &Path) -> String {
    format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"))
}

fn script(host: &Host, name: &str, body: &str) -> String {
    #[cfg(windows)]
    {
        let path = host.home.path().join(format!("{name}.cmd"));
        std::fs::write(&path, body).unwrap();
        format!("!call \"{}\"", path.display())
    }
    #[cfg(not(windows))]
    {
        let path = host.home.path().join(format!("{name}.sh"));
        std::fs::write(&path, body).unwrap();
        format!("!/bin/sh {}", quote(&path))
    }
}

struct ReleaseOnDrop(PathBuf);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.0, "release");
    }
}

/// FakeUpstream intentionally redacts bearer values. This private loopback
/// capture forwards bytes unchanged and checks exact synthetic auth values,
/// while reusing its established SSE fixtures and request/body receipts.
struct Wire {
    up: FakeUpstream,
    addr: std::net::SocketAddr,
    auth: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Wire {
    async fn new(responses: Value) -> Self {
        let up = FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None)
            .await
            .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let target = up.addr;
        let auth = Arc::new(Mutex::new(Vec::new()));
        let records = auth.clone();
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let records = records.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    // These CLI requests have ordinary JSON Content-Length.
                    while !bytes.ends_with(b"\r\n\r\n") {
                        let mut byte = [0];
                        if socket.read_exact(&mut byte).await.is_err() {
                            return;
                        }
                        bytes.push(byte[0]);
                        assert!(bytes.len() <= 64 * 1024, "fixture header bound");
                    }
                    let head = String::from_utf8(bytes.clone()).unwrap();
                    let authorization = head
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
                        .map(|(_, value)| value.trim().to_owned())
                        .unwrap_or_default();
                    records.lock().await.push(authorization);
                    let mut upstream = TcpStream::connect(target).await.unwrap();
                    upstream.write_all(&bytes).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut socket, &mut upstream).await;
                });
            }
        });
        Self { up, addr, auth, task }
    }

    fn base_url(&self) -> String {
        format!("http://{}/v1", self.addr)
    }
}

impl Drop for Wire {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn output(command: Command) -> Output {
    tokio::time::timeout(Duration::from_secs(20), tokio::process::Command::from(command).kill_on_drop(true).output())
        .await
        .expect("CLI exceeded bounded module test deadline")
        .unwrap()
}

fn spawn(command: Command) -> tokio::process::Child {
    tokio::process::Command::from(command)
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

async fn wait_for(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("module fixture did not reach its observed checkpoint");
}

fn success(output: &Output) -> String {
    assert_eq!(output.status.code(), Some(0), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn reply(api: &str, message: &str) -> Value {
    if api == "openai-completions" {
        json!({"events":[text(message),finish("stop"),done()]})
    } else {
        json!({"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message",
                "id":"msg_command","role":"assistant","content":[{"type":"output_text","text":message}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]})
    }
}

fn write_reply(api: &str) -> Value {
    let arguments = json!({"path":"command-proof.txt","content":"command workflow proof\n"}).to_string();
    if api == "openai-completions" {
        json!({"events":[tool_call(0,"call_command","write",&arguments),finish("tool_calls"),done()]})
    } else {
        json!({"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call",
                "id":"fc_command","call_id":"call_command","name":"write"}}},
            {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":arguments}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call",
                "id":"fc_command","call_id":"call_command","name":"write","arguments":"{}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]})
    }
}

fn configured(api: &str, endpoint: &str, key: &Helper, provider: &Helper, model: &Helper, overrides: &Helper) -> Value {
    json!({"api":api,"baseUrl":endpoint,"apiKey":key.command,"authHeader":true,
        "headers":{"X-Shared-Key":format!("!  {}  ",&key.command[1..]),"X-Layer":provider.command,"X-Provider":provider.command},
        "models":[{"id":"command-model","input":["text"],"maxTokens":128,
            "headers":{"X-Layer":model.command,"X-Model":model.command}}],
        "modelOverrides":{"command-model":{"headers":{"X-Layer":overrides.command,"X-Override":overrides.command}}}})
}

#[tokio::test]
async fn chat_and_responses_commands_preserve_overrides_tools_cwd_and_session_reopen() {
    for api in ["openai-completions", "openai-responses"] {
        let host = Host::new();
        let key = Helper::new(&host, "key", "synthetic-config-bearer");
        let provider = Helper::new(&host, "provider", "synthetic-provider-header");
        let model = Helper::new(&host, "model", "synthetic-model-header");
        let overrides = Helper::new(&host, "override", "synthetic-override-header");
        let wire = Wire::new(json!([
            write_reply(api),
            reply(api, "Written."),
            reply(api, "Resumed."),
            reply(api, "CLI key.")
        ]))
        .await;
        host.config("custom", configured(api, &wire.base_url(), &key, &provider, &model, &overrides));
        let first = output(host.command(
            "custom",
            &[
                "--cwd",
                host.project.path().to_str().unwrap(),
                "--mode",
                "json",
                "--header",
                "x-layer: cli-header",
                "--tools",
                "write",
                "write command-proof.txt",
            ],
        ))
        .await;
        let first_stdout = success(&first);
        let first_header: Value = serde_json::from_str(first_stdout.lines().next().unwrap()).unwrap();
        assert_eq!(
            std::fs::read_to_string(host.project.path().join("command-proof.txt")).unwrap(),
            "command workflow proof\n"
        );
        assert!(!host.launch.path().join("command-proof.txt").exists());
        for helper in [&key, &provider, &model, &overrides] {
            assert_eq!(helper.executions(), 1, "both logical model calls must reuse the command cache");
            helper.assert_cwd(host.project.path());
        }
        let session = host.session();
        let resumed =
            output(host.command("custom", &["--mode", "json", "--resume", session.to_str().unwrap(), "continue"]))
                .await;
        let resumed_stdout = success(&resumed);
        let resumed_header: Value = serde_json::from_str(resumed_stdout.lines().next().unwrap()).unwrap();
        assert_eq!(resumed_header["id"], first_header["id"]);
        let mut cli = host.command(
            "custom",
            &[
                "--cwd",
                host.project.path().to_str().unwrap(),
                "--api-key-env",
                "CMD_CLI_FIXTURE_KEY",
                "--tools",
                "",
                "explicit key",
            ],
        );
        cli.env("CMD_CLI_FIXTURE_KEY", "synthetic-explicit-bearer");
        let cli = output(cli).await;
        success(&cli);
        for helper in [&key, &provider, &model, &overrides] {
            assert_eq!(
                helper.executions(),
                3,
                "eager provider values and overwritten sources still execute once per process"
            );
            helper.assert_cwd(host.project.path());
        }
        let requests = wire.up.requests.lock().await;
        assert_eq!(requests.len(), 4);
        for request in requests.iter() {
            assert_eq!(
                request["request"],
                if api == "openai-completions" {
                    "POST /v1/chat/completions HTTP/1.1"
                } else {
                    "POST /v1/responses HTTP/1.1"
                }
            );
            assert_eq!(request["body"]["model"], "command-model");
            assert_eq!(request["headers"]["x-shared-key"], "synthetic-config-bearer");
            assert_eq!(request["headers"]["x-provider"], "synthetic-provider-header");
            assert_eq!(request["headers"]["x-model"], "synthetic-model-header");
            assert_eq!(request["headers"]["x-override"], "synthetic-override-header");
        }
        assert_eq!(requests[0]["headers"]["x-layer"], "cli-header");
        assert_eq!(requests[1]["headers"]["x-layer"], "cli-header");
        assert_eq!(requests[2]["headers"]["x-layer"], "synthetic-override-header");
        let replay = requests[2]["body"].to_string();
        assert!(replay.contains("call_command") && replay.contains("command workflow proof"));
        assert_eq!(
            *wire.auth.lock().await,
            vec![
                "Bearer synthetic-config-bearer",
                "Bearer synthetic-config-bearer",
                "Bearer synthetic-config-bearer",
                "Bearer synthetic-explicit-bearer"
            ]
        );
        for receipt in [&first, &resumed, &cli] {
            host.assert_private(
                receipt,
                &[
                    "synthetic-config-bearer",
                    "synthetic-explicit-bearer",
                    "synthetic-provider-header",
                    "synthetic-model-header",
                    "synthetic-override-header",
                    &key.command,
                ],
            );
        }

        // Native composition retains the configured authHeader source, while
        // the daily Host's ambient credential must own the final wire bearer.
        let env_host = Host::new();
        let env_wire = Wire::new(json!([reply(api, "Ambient key.")])).await;
        env_host.config(
            "custom",
            json!({"api":api,"baseUrl":env_wire.base_url(),
            "apiKey":"synthetic-config-plain","authHeader":true,
            "models":[{"id":"command-model","input":["text"]}],
            "modelOverrides":{"command-model":{"contextWindow":24000,"maxTokens":96,
                "name":"Override metadata","cost":{"input":0.2},"headers":{"X-Override":"literal-proof"}}}}),
        );
        let mut command = env_host.command("custom", &["--mode", "json", "--tools", "", "ambient key"]);
        command.env("ARA_API_KEY", "synthetic-ambient-bearer");
        let ambient = output(command).await;
        assert!(success(&ambient).contains("Ambient key."));
        assert_eq!(*env_wire.auth.lock().await, vec!["Bearer synthetic-ambient-bearer"]);
        let requests = env_wire.up.requests.lock().await;
        assert_eq!(requests[0]["headers"]["x-override"], "literal-proof");
        assert_eq!(
            requests[0]["body"]
                [if api == "openai-completions" { "max_completion_tokens" } else { "max_output_tokens" }],
            96
        );
        env_host.assert_private(&ambient, &["synthetic-config-plain", "synthetic-ambient-bearer"]);

        // Native loadCustomModels resolves provider headers before installing
        // apiKey (fixed model-registry.ts:1340,1407). The header helper may
        // generate the file the different key helper needs on its first run.
        let dependent_host = Host::new();
        let key = Helper::new(&dependent_host, "dependent-key", "FAIL");
        let provider = Helper::new(&dependent_host, "header-generator", "unused");
        let order = dependent_host.home.path().join("header-key-order.txt");
        #[cfg(windows)]
        let provider_body = format!(
            "@echo off\r\n>>\"{}\" echo run\r\ncd >>\"{}\"\r\n>>\"{}\" echo provider-header\r\n>\"{}\" echo synthetic-generated-private-key\r\necho synthetic-generated-provider-header\r\n",
            provider.count.display(),
            provider.cwd.display(),
            order.display(),
            key.token.display(),
        );
        #[cfg(not(windows))]
        let provider_body = format!(
            "printf 'run\\n' >> {}; pwd >> {}; printf 'provider-header\\n' >> {}; printf 'synthetic-generated-private-key\\n' > {}; printf %s synthetic-generated-provider-header\n",
            quote(&provider.count),
            quote(&provider.cwd),
            quote(&order),
            quote(&key.token),
        );
        assert_eq!(script(&dependent_host, "header-generator", &provider_body), provider.command);
        #[cfg(windows)]
        let key_body = format!(
            "@echo off\r\n>>\"{}\" echo run\r\ncd >>\"{}\"\r\n>>\"{}\" echo api-key\r\nset /p CMD_VALUE=<\"{}\"\r\nif \"%CMD_VALUE%\"==\"FAIL\" exit /b 1\r\necho %CMD_VALUE%\r\n",
            key.count.display(),
            key.cwd.display(),
            order.display(),
            key.token.display(),
        );
        #[cfg(not(windows))]
        let key_body = format!(
            "printf 'run\\n' >> {}; pwd >> {}; printf 'api-key\\n' >> {}; IFS= read -r value < {}; [ \"$value\" = FAIL ] && exit 1; printf %s \"$value\"\n",
            quote(&key.count),
            quote(&key.cwd),
            quote(&order),
            quote(&key.token),
        );
        assert_eq!(script(&dependent_host, "dependent-key", &key_body), key.command);
        assert_ne!(provider.command, key.command);
        let dependent_wire = Wire::new(json!([reply(api, "Dependent credential ready.")])).await;
        dependent_host.config(
            "custom",
            json!({
                "api":api,"baseUrl":dependent_wire.base_url(),"apiKey":key.command,
                "headers":{"X-Generated":provider.command},"models":[{"id":"command-model","input":["text"]}]
            }),
        );
        let dependent = output(dependent_host.command(
            "custom",
            &[
                "--cwd",
                dependent_host.project.path().to_str().unwrap(),
                "--mode",
                "json",
                "--tools",
                "",
                "provider header generates key file",
            ],
        ))
        .await;
        assert!(success(&dependent).contains("Dependent credential ready."));
        assert_eq!(
            std::fs::read_to_string(&order).unwrap().lines().collect::<Vec<_>>(),
            vec!["provider-header", "api-key"]
        );
        for helper in [&provider, &key] {
            assert_eq!(helper.executions(), 1, "load and request share each distinct helper's cached value");
            helper.assert_cwd(dependent_host.project.path());
        }
        assert_eq!(std::fs::read_to_string(&key.token).unwrap().trim(), "synthetic-generated-private-key");
        let requests = dependent_wire.up.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["headers"]["x-generated"], "synthetic-generated-provider-header");
        assert_eq!(*dependent_wire.auth.lock().await, vec!["Bearer synthetic-generated-private-key"]);
        dependent_host.assert_private(
            &dependent,
            &[
                "synthetic-generated-private-key",
                "synthetic-generated-provider-header",
                &key.command,
                &provider.command,
            ],
        );
    }
}

#[tokio::test]
async fn ordinary_retry_reuses_cache_and_one_401_refresh_rotates_all_sources_or_stops() {
    for refresh_fails in [false, true] {
        let host = Host::new();
        let key = Helper::new(&host, "key", "stale-command-bearer");
        let provider = Helper::new(&host, "provider", "stale-provider");
        let model = Helper::new(&host, "model", "stale-model");
        let overrides = Helper::new(&host, "override", "stale-override");
        let wire = Wire::new(json!([
            {"status":500,"body":"{\"error\":{\"message\":\"transient fixture failure\"}}"},
            {"status":401,"delay_ms":800,"body":"{\"error\":{\"message\":\"invalid api key\",\"type\":\"authentication_error\"}}"},
            reply("openai-completions","Refreshed.")
        ])).await;
        host.config("custom", configured("openai-completions", &wire.base_url(), &key, &provider, &model, &overrides));
        let child = spawn(host.command(
            "custom",
            &["--cwd", host.project.path().to_str().unwrap(), "--mode", "json", "--tools", "", "refresh"],
        ));
        wait_for(|| wire.up.served() == 2).await;
        for helper in [&key, &provider, &model, &overrides] {
            assert_eq!(helper.executions(), 1);
        }
        key.rotate(if refresh_fails { "FAIL" } else { "fresh-command-bearer" });
        provider.rotate("fresh-provider");
        model.rotate("fresh-model");
        overrides.rotate("fresh-override");
        let result = tokio::time::timeout(Duration::from_secs(20), child.wait_with_output()).await.unwrap().unwrap();
        assert_eq!(key.executions(), 2);
        let requests = wire.up.requests.lock().await;
        if refresh_fails {
            assert_eq!(result.status.code(), Some(1));
            assert_eq!(requests.len(), 2, "failed refresh must not redispatch the rejected key");
            assert_eq!(*wire.auth.lock().await, vec!["Bearer stale-command-bearer", "Bearer stale-command-bearer"]);
        } else {
            assert!(success(&result).contains("Refreshed."));
            assert_eq!(requests.len(), 3);
            for helper in [&provider, &model, &overrides] {
                assert_eq!(helper.executions(), 2);
            }
            for (name, old, new) in [
                ("x-provider", "stale-provider", "fresh-provider"),
                ("x-model", "stale-model", "fresh-model"),
                ("x-override", "stale-override", "fresh-override"),
                ("x-shared-key", "stale-command-bearer", "fresh-command-bearer"),
            ] {
                assert_eq!(requests[0]["headers"][name], old);
                assert_eq!(requests[1]["headers"][name], old);
                assert_eq!(requests[2]["headers"][name], new);
            }
            assert_eq!(
                *wire.auth.lock().await,
                vec!["Bearer stale-command-bearer", "Bearer stale-command-bearer", "Bearer fresh-command-bearer"]
            );
        }
        host.assert_private(
            &result,
            &[
                "stale-command-bearer",
                "fresh-command-bearer",
                "config-command-private-stdout",
                "config-command-private-stderr",
                &key.command,
            ],
        );
    }
}

#[tokio::test]
async fn failed_and_empty_commands_are_negative_cached_without_dispatch_or_public_secrets() {
    for value in ["FAIL", "EMPTY"] {
        let host = Host::new();
        let key = Helper::new(&host, "unavailable", value);
        let wire = Wire::new(json!([])).await;
        host.config(
            "custom",
            json!({"api":"openai-completions","baseUrl":wire.base_url(),"apiKey":key.command,
            "headers":{"X-Shared":format!("!  {}  ",&key.command[1..])},"models":[{"id":"command-model"}]}),
        );
        let result = output(host.command(
            "custom",
            &["--cwd", host.project.path().to_str().unwrap(), "--mode", "json", "--tools", "", "failure"],
        ))
        .await;
        assert_eq!(result.status.code(), Some(1));
        assert_eq!(key.executions(), 1, "eager key/header and request resolution must share the negative cache");
        assert_eq!(wire.up.served(), 0);
        assert!(wire.auth.lock().await.is_empty());
        host.assert_private(&result, &["config-command-private-stdout", "config-command-private-stderr", &key.command]);
    }
}

#[tokio::test]
async fn deadline_during_active_refresh_waits_for_owned_settlement_without_late_http() {
    let host = Host::new();
    let count = host.home.path().join("gated.count");
    let active = host.home.path().join("gated.active");
    let release = host.home.path().join("gated.release");
    let settled = host.home.path().join("gated.settled");
    let _release_on_failure = ReleaseOnDrop(release.clone());
    #[cfg(windows)]
    let body = format!(
        "@echo off\r\nif exist \"{}\" goto refresh\r\n>>\"{}\" echo run\r\necho gated-stale-bearer\r\nexit /b 0\r\n:refresh\r\n>>\"{}\" echo run\r\n>\"{}\" echo active\r\n:wait\r\nif not exist \"{}\" goto wait\r\n>\"{}\" echo settled\r\necho gated-fresh-bearer\r\n",
        count.display(),
        count.display(),
        count.display(),
        active.display(),
        release.display(),
        settled.display()
    );
    #[cfg(not(windows))]
    let body = format!(
        "if [ ! -e {} ]; then printf 'run\\n' >> {}; printf %s gated-stale-bearer; exit 0; fi; printf 'run\\n' >> {}; printf active > {}; while [ ! -e {} ]; do sleep 0.02; done; printf settled > {}; printf %s gated-fresh-bearer\n",
        quote(&count),
        quote(&count),
        quote(&count),
        quote(&active),
        quote(&release),
        quote(&settled)
    );
    let command = script(&host, "gated", &body);
    let wire = Wire::new(json!([
        {"status":401,"body":"{\"error\":{\"message\":\"invalid api key\"}}"},
        reply("openai-completions","Must not dispatch.")
    ]))
    .await;
    host.config(
        "custom",
        json!({"api":"openai-completions","baseUrl":wire.base_url(),"apiKey":command,
        "models":[{"id":"command-model"}]}),
    );
    let mut child = spawn(host.command(
        "custom",
        &[
            "--cwd",
            host.project.path().to_str().unwrap(),
            "--max-time",
            "0.3",
            "--mode",
            "json",
            "--tools",
            "",
            "cancel refresh",
        ],
    ));
    wait_for(|| active.exists()).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let still_owned = child.try_wait().unwrap().is_none();
    // Release even if ownership was broken, so this test never leaves its
    // synthetic helper waiting. Its side effects are observed, not rolled back.
    std::fs::write(&release, "release").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await.unwrap().unwrap();
    assert!(still_owned, "CLI exited before its active helper settled");
    assert_eq!(result.status.code(), Some(1));
    assert!(settled.exists());
    assert_eq!(std::fs::read_to_string(&count).unwrap().lines().count(), 2);
    assert_eq!(wire.up.served(), 1, "cancelled refresh must not send a late HTTP request");
    assert_eq!(*wire.auth.lock().await, vec!["Bearer gated-stale-bearer"]);
    let journal = std::fs::read_to_string(host.session()).unwrap();
    assert!(journal.contains("aborted") || journal.contains("Deadline exceeded"));
    host.assert_private(&result, &["gated-stale-bearer", "gated-fresh-bearer", &command]);

    // Separate logical calls in the same Host process contend for its owned
    // operation gate. Cancelling B while A is active must prevent B's helper
    // from starting when the gate becomes available.
    use ara_cli::config_request_auth::{ConfigRequestAuth, ConfigRequestAuthSpec};
    use ara_cli::model_route::{AuthResolveError, CredentialIdentity, RequestAuthLease, RequestAuthResolver};
    use tokio_util::sync::CancellationToken;
    let queued_host = Host::new();
    let active = queued_host.home.path().join("owner.active");
    let release = queued_host.home.path().join("owner.release");
    let settled = queued_host.home.path().join("owner.settled");
    let _release_on_failure = ReleaseOnDrop(release.clone());
    #[cfg(windows)]
    let body = format!(
        "@echo off\r\n>\"{}\" echo active\r\n:wait\r\nif not exist \"{}\" goto wait\r\n>\"{}\" echo settled\r\necho synthetic-owner-bearer\r\n",
        active.display(),
        release.display(),
        settled.display()
    );
    #[cfg(not(windows))]
    let body = format!(
        "printf active > {}; while [ ! -e {} ]; do sleep 0.02; done; printf settled > {}; printf %s synthetic-owner-bearer\n",
        quote(&active),
        quote(&release),
        quote(&settled)
    );
    let owner_command = script(&queued_host, "owner", &body);
    let queued = Helper::new(&queued_host, "queued-must-not-run", "synthetic-queued-bearer");
    let auth = |command: String| {
        ConfigRequestAuth::new(
            ConfigRequestAuthSpec {
                base: RequestAuthLease::new(CredentialIdentity::Runtime, None),
                key_config: Some(command.clone()),
                startup_key: Some(command.clone()),
                header_sources: Vec::new(),
                composed_headers: None,
                invalidation_values: vec![command],
                cli_headers: Vec::new(),
                auth_header: false,
                codex_account: false,
            },
            queued_host.project.path().to_path_buf(),
            None,
        )
    };
    let owner_auth = auth(owner_command);
    let queued_auth = auth(queued.command.clone());
    let model = ara_ai::Model {
        id: "command-model".into(),
        api: "openai-completions".into(),
        provider: "custom".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let owner_model = model.clone();
    let owner = tokio::spawn(async move { owner_auth.resolve(&owner_model, &CancellationToken::new()).await });
    wait_for(|| active.exists()).await;
    let cancelled = CancellationToken::new();
    let worker_cancel = cancelled.clone();
    let (polled_tx, polled_rx) = tokio::sync::oneshot::channel();
    let pending = tokio::spawn(async move {
        polled_tx.send(()).unwrap();
        // This test uses Tokio's current-thread runtime: the task continues
        // into resolve's blocking-worker await before the receiver can resume.
        queued_auth.resolve(&model, &worker_cancel).await
    });
    polled_rx.await.unwrap();
    cancelled.cancel();
    assert!(!queued.count.exists());
    std::fs::write(&release, "release").unwrap();
    let owner = tokio::time::timeout(Duration::from_secs(5), owner).await.unwrap().unwrap();
    assert!(owner.is_ok());
    let pending = tokio::time::timeout(Duration::from_secs(5), pending).await.unwrap().unwrap();
    assert!(matches!(pending, Err(AuthResolveError::Cancelled)));
    assert!(settled.exists());
    assert!(!queued.count.exists(), "cancelled queued call executed a helper after acquiring the gate");

    // The static catalog nests model and override Live sources. A deadline
    // during the first source must settle that helper and stop the next one.
    let host = Host::new();
    let active = host.home.path().join("nested.active");
    let release = host.home.path().join("nested.release");
    let settled = host.home.path().join("nested.settled");
    let _release_on_failure = ReleaseOnDrop(release.clone());
    #[cfg(windows)]
    let body = format!(
        "@echo off\r\n>\"{}\" echo active\r\n:wait\r\nif not exist \"{}\" goto wait\r\n>\"{}\" echo settled\r\necho synthetic-nested-header\r\n",
        active.display(),
        release.display(),
        settled.display()
    );
    #[cfg(not(windows))]
    let body = format!(
        "printf active > {}; while [ ! -e {} ]; do sleep 0.02; done; printf settled > {}; printf %s synthetic-nested-header\n",
        quote(&active),
        quote(&release),
        quote(&settled)
    );
    let slow = script(&host, "nested-slow", &body);
    let next = Helper::new(&host, "nested-must-not-run", "synthetic-next-header");
    let wire = Wire::new(json!([])).await;
    host.config(
        "custom",
        json!({"api":"openai-completions","baseUrl":wire.base_url(),
        "auth":"none","models":[{"id":"command-model","headers":{"X-First":slow}}],
        "modelOverrides":{"command-model":{"headers":{"X-Next":next.command}}}}),
    );
    let mut child = spawn(host.command(
        "custom",
        &[
            "--cwd",
            host.project.path().to_str().unwrap(),
            "--max-time",
            "0.3",
            "--mode",
            "json",
            "--tools",
            "",
            "cancel nested headers",
        ],
    ));
    wait_for(|| active.exists()).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    let still_owned = child.try_wait().unwrap().is_none();
    std::fs::write(&release, "release").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await.unwrap().unwrap();
    assert!(still_owned, "CLI exited before its nested helper settled");
    assert_eq!(result.status.code(), Some(1));
    assert!(settled.exists());
    assert_eq!(next.executions(), 0, "cancelled nested source started the next helper");
    assert_eq!(wire.up.served(), 0);
    host.assert_private(&result, &["synthetic-nested-header", "synthetic-next-header", &slow, &next.command]);
}

#[tokio::test]
async fn codex_rejects_owned_config_before_helpers_and_preserves_account_with_allowed_headers() {
    let host = Host::new();
    let helper = Helper::new(&host, "keyless-must-not-execute", "synthetic-override-credential");
    let wire = Wire::new(json!([])).await;
    host.config(
        "custom",
        json!({"api":"openai-completions","baseUrl":wire.base_url(),"auth":"none",
        "models":[{"id":"command-model"}],
        "modelOverrides":{"command-model":{"headers":{"Authorization":helper.command}}}}),
    );
    let result = output(host.command("custom", &["--tools", "", "reject keyless override"])).await;
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(helper.executions(), 0);
    assert_eq!(wire.up.served(), 0);
    assert!(!host.sessions.exists());
    host.assert_private(&result, &["synthetic-override-credential", &helper.command]);

    // Cover both credential and protocol/session ownership at all config levels.
    for (field, level) in
        [("apiKey", 0), ("Authorization", 0), ("chatgpt-account-id", 1), ("session_id", 2), ("x-client-request-id", 0)]
    {
        let host = Host::new();
        let helper = Helper::new(&host, "must-not-execute", "synthetic-owned-value");
        let wire = Wire::new(json!([])).await;
        let mut provider = json!({"api":"openai-codex-responses","auth":"oauth","baseUrl":format!("http://{}",wire.addr),
            "headers":{"X-Permitted":helper.command},"models":[{"id":"command-model"}]});
        if field == "apiKey" {
            provider[field] = json!(helper.command);
        } else {
            let headers = json!({field:helper.command});
            match level {
                0 => provider["headers"][field] = json!(helper.command),
                1 => provider["models"][0]["headers"] = headers,
                _ => provider["modelOverrides"] = json!({"command-model":{"headers":headers}}),
            }
        }
        host.config("openai-codex", provider);
        let result = output(host.command("openai-codex", &["--tools", "", "reject ownership"])).await;
        assert_eq!(result.status.code(), Some(2));
        assert_eq!(helper.executions(), 0, "ownership rejection must precede all helper execution");
        assert_eq!(wire.up.served(), 0);
        assert!(!host.sessions.exists());
        assert!(!host.home.path().join("agent/auth.db").exists());
        host.assert_private(&result, &["synthetic-owned-value", &helper.command]);
    }

    #[cfg(feature = "test-fixture")]
    {
        use ara_cli::credential_store::{AuthCredential, SqliteCredentialStore};
        let host = Host::new();
        let helper = Helper::new(&host, "allowed", "synthetic-account-custom-header");
        let wire = Wire::new(json!([reply("openai-responses", "Account command header.")])).await;
        let base = format!("http://{}", wire.addr);
        host.config(
            "openai-codex",
            json!({"api":"openai-codex-responses","auth":"oauth","baseUrl":base,
            "headers":{"X-Allowed":helper.command},"models":[{"id":"command-model","input":["text"]}]}),
        );
        let mut fields = serde_json::Map::new();
        fields.insert("access".into(), json!("synthetic-account-access"));
        fields.insert("refresh".into(), json!("synthetic-account-refresh"));
        fields.insert("expires".into(), json!(chrono::Utc::now().timestamp_millis() + 3_600_000));
        fields.insert("authorizedAt".into(), json!(chrono::Utc::now().timestamp_millis()));
        fields.insert("accountId".into(), json!("synthetic-account"));
        let store = SqliteCredentialStore::open(host.home.path().join("agent/auth.db")).unwrap();
        store.upsert_auth_credential_for_provider("openai-codex", &AuthCredential::oauth(fields)).unwrap();
        drop(store);
        let mut command = host.command(
            "openai-codex",
            &["--cwd", host.project.path().to_str().unwrap(), "--mode", "json", "--tools", "", "account header"],
        );
        command.env("ARA_TEST_CODEX_AUTH_BASE_URL", &base);
        let result = output(command).await;
        assert!(success(&result).contains("Account command header."));
        assert_eq!(helper.executions(), 1);
        helper.assert_cwd(host.project.path());
        let requests = wire.up.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["request"], "POST /codex/responses HTTP/1.1");
        assert_eq!(requests[0]["headers"]["chatgpt-account-id"], "synthetic-account");
        assert_eq!(requests[0]["headers"]["x-allowed"], "synthetic-account-custom-header");
        let session: Value = serde_json::from_str(success(&result).lines().next().unwrap()).unwrap();
        assert_eq!(requests[0]["headers"]["session_id"], session["id"]);
        assert_eq!(*wire.auth.lock().await, vec!["Bearer synthetic-account-access"]);
        host.assert_private(
            &result,
            &[
                "synthetic-account-access",
                "synthetic-account-refresh",
                "synthetic-account-custom-header",
                &helper.command,
            ],
        );
    }
}
