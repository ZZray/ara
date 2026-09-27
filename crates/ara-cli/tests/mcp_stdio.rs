//! Actual CLI → MCP child → Agent tool → Session process evidence.

use ara_agent::ToolOutput;
use ara_mcp::{ServerConfig, connect};
use ara_testkit::chunks::*;
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

const CLI: &str = env!("CARGO_BIN_EXE_ara");
const MCP: &str = env!("CARGO_BIN_EXE_ara-mcp-fixture");

fn config(mode: &str, record: &Path) -> ServerConfig {
    ServerConfig {
        name: "fixture".into(),
        command: PathBuf::from(MCP),
        args: vec!["--mode".into(), mode.into(), "--record".into(), record.to_string_lossy().into_owned()],
        env_from_host: Default::default(),
    }
}

fn update() -> Arc<dyn Fn(ToolOutput) + Send + Sync> {
    Arc::new(|_| {})
}

#[tokio::test]
async fn grants_protocol_and_client_capabilities_are_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools = connect(config("ask-roots", &record), dir.path(), &["fixture:echo".into()], &["read"]).await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].definition().name, "mcp__fixture__echo");
    let mut args = ara_ai::JsonObject::new();
    args.insert("text".into(), json!("hello"));
    let result = tools[0].execute("call-1", args, CancellationToken::new(), update()).await.unwrap();
    assert!(!result.is_error);
    assert_eq!(result.content, vec![ara_ai::UserBlock::text("echo: hello")]);
    let receipt: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(receipt["initialize"]["capabilities"], json!({}));
    assert_eq!(receipt["ambient_key_present"], false);
    assert_eq!(receipt["call"]["name"], "echo");
    assert_eq!(receipt["root_denied"], true);
}

#[tokio::test]
async fn collision_and_missing_grant_fail_before_tool_exposure() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let err =
        connect(config("collision", &record), dir.path(), &["fixture:foo-bar".into(), "fixture:foo_bar".into()], &[])
            .await
            .err()
            .unwrap();
    assert!(err.contains("collision"), "{err}");
    let err = connect(config("normal", &record), dir.path(), &["fixture:other".into()], &[]).await.err().unwrap();
    assert!(err.contains("missing"), "{err}");
}

#[tokio::test]
async fn child_exit_after_effect_is_unknown_and_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let marker = dir.path().join("effect.txt");
    let tools = connect(config("exit", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let mut args = ara_ai::JsonObject::new();
    args.insert("marker".into(), json!(marker));
    let first = tools[0].execute("call-1", args.clone(), CancellationToken::new(), update()).await.unwrap();
    assert!(first.is_error);
    assert_eq!(first.details.unwrap()["executed"], "unknown");
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
    let second = tools[0].execute("call-2", args, CancellationToken::new(), update()).await.unwrap();
    assert!(second.is_error);
    assert!(
        second.content.iter().any(|b| matches!(b, ara_ai::UserBlock::Text(t) if t.text.contains("not dispatched")))
    );
}

#[tokio::test]
async fn cancellation_after_dispatch_marks_unknown_effect() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let marker = dir.path().join("effect.txt");
    let tools = connect(config("hang", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let mut args = ara_ai::JsonObject::new();
    args.insert("marker".into(), json!(marker));
    let cancel = CancellationToken::new();
    let calling = {
        let tool = tools[0].clone();
        let cancel = cancel.clone();
        tokio::spawn(async move { tool.execute("call-hang", args, cancel, update()).await.unwrap() })
    };
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(marker.exists(), "fixture did not dispatch the call");
    cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), calling).await.unwrap().unwrap();
    assert!(result.is_error);
    assert_eq!(result.details.unwrap()["executed"], "unknown");
}

#[tokio::test]
async fn oversized_result_is_not_exposed_as_success() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools = connect(config("large-result", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let result = tools[0].execute("call-large", Default::default(), CancellationToken::new(), update()).await.unwrap();
    assert!(result.is_error);
    assert_eq!(result.details.unwrap()["executed"], "unknown");
}

#[tokio::test]
async fn resource_and_structured_result_reach_model_content() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools = connect(config("structured", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let result =
        tools[0].execute("call-structured", Default::default(), CancellationToken::new(), update()).await.unwrap();
    assert!(!result.is_error);
    let text = result
        .content
        .iter()
        .filter_map(|b| match b {
            ara_ai::UserBlock::Text(t) => Some(t.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("[Resource: file:///report.txt]\nreport body"), "{text}");
    assert!(text.contains("\"status\": \"ok\""), "{text}");
    let only = connect(config("structured-only", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let result = only[0].execute("call-only", Default::default(), CancellationToken::new(), update()).await.unwrap();
    assert!(
        result.content.iter().any(|b| matches!(b, ara_ai::UserBlock::Text(t) if t.text.contains("\"status\": \"ok\"")))
    );
}

#[tokio::test]
async fn cli_journals_mcp_call_and_keeps_provider_key_out_of_child() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let config_path = dir.path().join("mcp.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&json!({
            "name":"fixture", "command":MCP, "args":["--record",record]
        }))
        .unwrap(),
    )
    .unwrap();
    let session_dir = dir.path().join("sessions");
    let script: Script = serde_json::from_value(json!({"responses":[
        {"events":[tool_call(0,"call_mcp","mcp__fixture__echo","{\"text\":\"from model\"}"),finish("tool_calls"),done()]},
        {"events":[text("MCP complete"),finish("stop"),done()]}
    ]})).unwrap();
    let upstream = FakeUpstream::start(script, None).await.unwrap();
    let mut command = Command::new(CLI);
    command
        .env("ARA_API_KEY", "test-provider-only-secret")
        .env("HOME", dir.path())
        .env("ARA_HOME", dir.path())
        .args(["--model", "fake-model", "--base-url", &upstream.base_url(), "--cwd"])
        .arg(dir.path())
        .arg("--session-dir")
        .arg(&session_dir)
        .args(["--tools", "", "--mcp-config"])
        .arg(&config_path)
        .args(["--mcp-allow", "fixture:echo", "Run the MCP tool"])
        .stdin(Stdio::null());
    let output = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "MCP complete\n");
    let receipt: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(receipt["ambient_key_present"], false);
    assert_eq!(receipt["call"]["name"], "echo");
    let files: Vec<_> = std::fs::read_dir(&session_dir).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    let entries: Vec<Value> =
        std::fs::read_to_string(&files[0]).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(
        entries
            .iter()
            .any(|e| e["message"]["role"] == "toolResult" && e["message"]["toolName"] == "mcp__fixture__echo")
    );
    let requests = upstream.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|s| s.contains("echo: from model")))
    );
}

#[tokio::test]
async fn cli_resume_keeps_unknown_mcp_effect_without_replaying_it() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let marker = dir.path().join("effect.txt");
    let config_path = dir.path().join("mcp.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec(&json!({
            "name":"fixture", "command":MCP, "args":["--mode","exit","--record",record]
        }))
        .unwrap(),
    )
    .unwrap();
    let session_dir = dir.path().join("sessions");
    let tool_args = json!({"marker":marker}).to_string();
    let script: Script = serde_json::from_value(json!({"responses":[
        {"events":[tool_call(0,"call_uncertain","mcp__fixture__echo",&tool_args),finish("tool_calls"),done()]},
        {"events":[text("Effect needs review"),finish("stop"),done()]},
        {"events":[text("Continued same session"),finish("stop"),done()]}
    ]}))
    .unwrap();
    let upstream = FakeUpstream::start(script, None).await.unwrap();
    let run = |resume: Option<PathBuf>,
               prompt: &'static str,
               base_url: String,
               root: PathBuf,
               config_path: PathBuf,
               session_dir: PathBuf| {
        let mut command = Command::new(CLI);
        command
            .env("ARA_API_KEY", "test-provider-only-secret")
            .env("HOME", &root)
            .env("ARA_HOME", &root)
            .args(["--model", "fake-model", "--base-url", &base_url, "--cwd"])
            .arg(&root)
            .arg("--session-dir")
            .arg(&session_dir)
            .args(["--tools", "", "--mcp-config"])
            .arg(&config_path)
            .args(["--mcp-allow", "fixture:echo"]);
        if let Some(resume) = resume {
            command.arg("--resume").arg(resume);
        }
        command.arg(prompt).stdin(Stdio::null());
        command
    };
    let mut first = run(
        None,
        "Make an MCP call",
        upstream.base_url(),
        dir.path().to_path_buf(),
        config_path.clone(),
        session_dir.clone(),
    );
    let first = tokio::task::spawn_blocking(move || first.output().unwrap()).await.unwrap();
    assert!(first.status.success(), "{}", String::from_utf8_lossy(&first.stderr));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
    let files: Vec<_> = std::fs::read_dir(&session_dir).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    let before = std::fs::read_to_string(&files[0]).unwrap();
    let entries: Vec<Value> = before.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert!(entries.iter().any(|e| e["message"]["toolCallId"] == "call_uncertain" && e["message"]["details"]["executed"] == "unknown"));
    let mut second = run(
        Some(files[0].clone()),
        "Continue",
        upstream.base_url(),
        dir.path().to_path_buf(),
        config_path,
        session_dir,
    );
    let second = tokio::task::spawn_blocking(move || second.output().unwrap()).await.unwrap();
    assert!(second.status.success(), "{}", String::from_utf8_lossy(&second.stderr));
    assert_eq!(String::from_utf8_lossy(&second.stdout), "Continued same session\n");
    let after = std::fs::read_to_string(&files[0]).unwrap();
    assert!(after.starts_with(&before));
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "dispatched");
    let requests = upstream.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests[2]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["role"] == "tool" && m["content"].as_str().is_some_and(|s| s.contains("effect unknown")))
    );
}
