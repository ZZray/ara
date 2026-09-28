//! Real CLI process → proxy model list → selected wire provider → Session.

use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const KEY: &str = "sk-proxy-discovery-test-secret";

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            home: tempfile::Builder::new().prefix("ara-proxy-home-").tempdir().unwrap(),
            work: tempfile::Builder::new().prefix("ara-proxy-work-").tempdir().unwrap(),
        }
    }

    fn cmd(&self, base_url: &str, args: &[&str]) -> Command {
        let mut cmd = Command::new(BIN);
        for name in [
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENROUTER_API_KEY",
            "ARA_MODEL",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "OPENROUTER_BASE_URL",
            "CLAUDE_CONFIG_DIR",
            "COPILOT_HOME",
            "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
        ] {
            cmd.env_remove(name);
        }
        cmd.env("ARA_API_KEY", KEY)
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .args(["--model", "selected-model", "--base-url", base_url, "--cwd"])
            .arg(self.work.path())
            .args(["--session-dir"])
            .arg(self.home.path().join("sessions"))
            .args(args)
            .stdin(Stdio::null());
        cmd
    }

    fn sessions(&self) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(self.home.path().join("sessions"))
            .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default()
    }
}

async fn upstream(responses: Value) -> FakeUpstream {
    let script: Script = serde_json::from_value(json!({"responses":responses})).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}

async fn output(mut cmd: Command) -> Output {
    tokio::task::spawn_blocking(move || cmd.output().unwrap()).await.unwrap()
}

fn anthropic_frame(value: Value) -> Value {
    let kind = value["type"].as_str().unwrap();
    json!({"raw":format!("event: {kind}\ndata: {value}\n\n")})
}

fn assert_key_absent(out: &Output, sessions: &[std::path::PathBuf]) {
    assert!(!String::from_utf8_lossy(&out.stdout).contains(KEY));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(KEY));
    for path in sessions {
        assert!(!std::fs::read_to_string(path).unwrap().contains(KEY));
    }
}

#[tokio::test]
async fn dual_protocol_proxy_prefers_anthropic_and_records_the_proxy_identity() {
    let env = Env::new();
    let up = upstream(json!([
        {"body":json!({"data":[{"id":"other","supported_endpoint_types":["openai"]},{"id":"selected-model","supported_endpoint_types":["openai","anthropic"]}]}).to_string()},
        {"events":[
            anthropic_frame(json!({"type":"message_start","message":{"id":"msg_proxy_tool"}})),
            anthropic_frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_proxy","name":"write","input":{}}})),
            anthropic_frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"proxy.txt\",\"content\":\"routed once\\n\"}"}})),
            anthropic_frame(json!({"type":"content_block_stop","index":0})),
            anthropic_frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            anthropic_frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            anthropic_frame(json!({"type":"message_start","message":{"id":"msg_proxy_final"}})),
            anthropic_frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Proxy answer."}})),
            anthropic_frame(json!({"type":"content_block_stop","index":0})),
            anthropic_frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            anthropic_frame(json!({"type":"message_stop"}))
        ]}
    ]))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "--tools", "write", "Write proxy.txt"])).await;
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Proxy answer.\n");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 3);
    assert_eq!(reqs[0]["request"], "GET /v1/models HTTP/1.1");
    assert_eq!(reqs[1]["request"], "POST /v1/messages HTTP/1.1");
    assert_eq!(reqs[2]["request"], "POST /v1/messages HTTP/1.1");
    assert_eq!(reqs[2]["body"]["messages"][2]["content"][0]["tool_use_id"], "toolu_proxy");
    assert!(reqs.iter().all(|request| request["headers"]["authorization"].as_str().unwrap().starts_with("<redacted")));
    drop(reqs);
    let sessions = env.sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("proxy.txt")).unwrap(), "routed once\n");
    let journal = std::fs::read_to_string(&sessions[0]).unwrap();
    assert!(journal.contains("\"model\":\"proxy/selected-model\""));
    assert_eq!(journal.matches("\"role\":\"toolResult\"").count(), 1);
    assert!(journal.contains("Proxy answer."));
    assert_key_absent(&out, &sessions);
}

#[tokio::test]
async fn openai_only_proxy_selects_chat_and_explicit_api_skips_discovery() {
    let env = Env::new();
    let up = upstream(json!([
        {"body":json!({"data":[{"id":"selected-model","supported_endpoint_types":["openai"]}]}).to_string()},
        {"events":[
            {"data":{"id":"chat-1","choices":[{"index":0,"delta":{"content":"Chat answer."}}]}},
            {"data":{"id":"chat-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}},
            {"done":true}
        ]}
    ]))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "Answer"])).await;
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "Chat answer.\n");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs[0]["request"], "GET /v1/models HTTP/1.1");
    assert_eq!(reqs[1]["request"], "POST /v1/chat/completions HTTP/1.1");
    drop(reqs);
    assert_key_absent(&out, &env.sessions());

    let explicit = upstream(json!([{"events":[
        {"data":{"id":"chat-2","choices":[{"index":0,"delta":{"content":"Explicit."}}]}},
        {"data":{"id":"chat-2","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}},
        {"done":true}
    ]}]))
    .await;
    let explicit_out = output(env.cmd(&explicit.base_url(), &["--api", "openai-completions", "Answer"])).await;
    assert_eq!(explicit_out.status.code(), Some(0), "{}", String::from_utf8_lossy(&explicit_out.stderr));
    assert_eq!(explicit.served(), 1);
    assert_eq!(explicit.requests.lock().await[0]["request"], "POST /v1/chat/completions HTTP/1.1");
}

#[tokio::test]
async fn discovery_failures_do_not_start_a_session_or_inference() {
    let cases = [
        (json!({"data":[]}), "not found"),
        (json!({"data":[{"id":"selected-model"}]}), "supported_endpoint_types"),
        (json!({"data":[{"id":"selected-model","supported_endpoint_types":["responses"]}]}), "no supported"),
        (
            json!({"data":[{"id":"selected-model","supported_endpoint_types":["openai"]},{"id":"selected-model","supported_endpoint_types":["anthropic"]}]}),
            "duplicate",
        ),
    ];
    for (payload, expected) in cases {
        let env = Env::new();
        let up = upstream(json!([{"body":payload.to_string()}])).await;
        let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "Answer"])).await;
        assert_eq!(out.status.code(), Some(2), "{}", String::from_utf8_lossy(&out.stderr));
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
        assert_eq!(up.served(), 1);
        assert!(env.sessions().is_empty());
        assert_key_absent(&out, &[]);
    }
    for (response, expected) in [
        (json!({"status":401,"body":"secret upstream error"}), "HTTP 401"),
        (json!({"body":"not json"}), "invalid JSON"),
        (json!({"body":"x".repeat(1024 * 1024 + 1)}), "exceeds"),
    ] {
        let env = Env::new();
        let up = upstream(json!([response])).await;
        let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "Answer"])).await;
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains(expected));
        assert!(!String::from_utf8_lossy(&out.stderr).contains("secret upstream error"));
        assert_eq!(up.served(), 1);
        assert!(env.sessions().is_empty());
        assert_key_absent(&out, &[]);
    }
}

#[tokio::test]
async fn discovery_does_not_forward_credentials_on_redirect_and_rejects_resume() {
    let env = Env::new();
    let other = upstream(json!([])).await;
    let up = upstream(json!([{"status":302,"headers":{"location":format!("{}/models", other.base_url())}}])).await;
    let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "Answer"])).await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(up.served(), 1);
    assert_eq!(other.served(), 0);
    assert!(env.sessions().is_empty());
    assert_key_absent(&out, &[]);

    let resume = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "--continue", "Answer"])).await;
    assert_eq!(resume.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&resume.stderr).contains("cannot resume"));
    assert_eq!(up.served(), 1);
}

#[tokio::test]
async fn invalid_local_input_never_probes_and_slow_discovery_fails_closed() {
    let env = Env::new();
    let up = upstream(json!([])).await;
    let no_prompt = output(env.cmd(&up.base_url(), &["--api", "proxy-auto"])).await;
    assert_eq!(no_prompt.status.code(), Some(2));
    assert_eq!(up.served(), 0);

    let special_provider =
        output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "--provider", "opencode-go", "Answer"])).await;
    assert_eq!(special_provider.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&special_provider.stderr).contains("different authentication scheme"));
    assert_eq!(up.served(), 0);

    let official = output(env.cmd("https://api.anthropic.com/v1", &["--api", "proxy-auto", "Answer"])).await;
    assert_eq!(official.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&official.stderr).contains("requires a dual-protocol proxy"));
    assert_eq!(up.served(), 0);

    let bad_url = format!("{}?token=private-url-secret", up.base_url());
    let invalid = output(env.cmd(&bad_url, &["--api", "proxy-auto", "Answer"])).await;
    assert_eq!(invalid.status.code(), Some(2));
    assert_eq!(up.served(), 0);
    assert!(!String::from_utf8_lossy(&invalid.stderr).contains("private-url-secret"));
    assert!(env.sessions().is_empty());

    let delayed = upstream(json!([{"delay_ms":6000,"body":json!({"data":[]}).to_string()}])).await;
    let timed_out = output(env.cmd(&delayed.base_url(), &["--api", "proxy-auto", "Answer"])).await;
    assert_eq!(timed_out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&timed_out.stderr).contains("discovery request failed"));
    assert_eq!(delayed.served(), 1);
    assert!(env.sessions().is_empty());
    assert_key_absent(&timed_out, &[]);
}

#[tokio::test]
async fn selected_proxy_cannot_redirect_inference_to_another_origin() {
    let env = Env::new();
    let other = upstream(json!([])).await;
    let up = upstream(json!([
        {"body":json!({"data":[{"id":"selected-model","supported_endpoint_types":["openai"]}]}).to_string()},
        {"status":302,"headers":{"location":format!("{}/chat/completions", other.base_url())}}
    ]))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--api", "proxy-auto", "Answer"])).await;
    assert_ne!(out.status.code(), Some(0));
    assert_eq!(up.served(), 2);
    assert_eq!(other.served(), 0);
    assert_key_absent(&out, &env.sessions());
}
