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

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};

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

#[cfg(windows)]
#[test]
fn windows_force_exit_helper() {
    let Some(root) = std::env::var_os("ARA_TEST_MCP_EXIT_DIR").map(PathBuf::from) else {
        return;
    };
    let record = root.join("record.json");
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let tools =
        runtime.block_on(connect(config("linger-after-list", &record), &root, &["fixture:echo".into()], &[])).unwrap();
    assert_eq!(tools.len(), 1);
    std::fs::write(root.join("ready"), "ready").unwrap();
    let start = Instant::now();
    while !root.join("go").exists() && start.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(root.join("go").exists(), "parent did not release helper");
    std::hint::black_box(&tools);
    std::process::exit(130);
}

#[cfg(windows)]
struct ReapHelper(std::process::Child);

#[cfg(windows)]
impl Drop for ReapHelper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(windows)]
#[test]
fn windows_force_exit_reaps_mcp_server() {
    let dir = tempfile::tempdir().unwrap();
    let mut parent = ReapHelper(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "windows_force_exit_helper", "--nocapture"])
            .env("ARA_TEST_MCP_EXIT_DIR", dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let ready = dir.path().join("ready");
    let start = Instant::now();
    while !ready.exists() && start.elapsed() < Duration::from_secs(10) {
        if let Some(status) = parent.0.try_wait().unwrap() {
            panic!("MCP helper exited before readiness: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    if !ready.exists() {
        panic!("MCP helper did not become ready");
    }
    let receipt: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("record.json")).unwrap()).unwrap();
    let pid = receipt["pid"].as_u64().unwrap() as u32;
    // SAFETY: OpenProcess returns a new handle that OwnedHandle closes.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    assert!(!raw.is_null(), "could not open running MCP fixture: {}", std::io::Error::last_os_error());
    let server = unsafe { OwnedHandle::from_raw_handle(raw) };
    // SAFETY: the process handle is valid for this synchronous wait.
    assert_eq!(unsafe { WaitForSingleObject(server.as_raw_handle(), 0) }, WAIT_TIMEOUT);
    std::fs::write(dir.path().join("go"), "go").unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(status) = parent.0.try_wait().unwrap() {
            break status;
        }
        if start.elapsed() >= Duration::from_secs(5) {
            // SAFETY: the process handle is valid; this is failure cleanup.
            unsafe { TerminateProcess(server.as_raw_handle(), 1) };
            panic!("MCP helper did not force-exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status.code(), Some(130));
    // SAFETY: the process handle remains valid after parent exit.
    let result = unsafe { WaitForSingleObject(server.as_raw_handle(), 3000) };
    if result != WAIT_OBJECT_0 {
        // SAFETY: clean up a server left running by a failed assertion.
        unsafe { TerminateProcess(server.as_raw_handle(), 1) };
    }
    assert_eq!(result, WAIT_OBJECT_0, "MCP server survived parent process::exit");
}

#[cfg(windows)]
#[tokio::test]
async fn dropping_last_tool_reaps_idle_mcp_server() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools = connect(config("linger-after-list", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let receipt: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    let pid = receipt["pid"].as_u64().unwrap() as u32;
    // SAFETY: OpenProcess returns a new handle that OwnedHandle closes.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    assert!(!raw.is_null(), "could not open running MCP fixture: {}", std::io::Error::last_os_error());
    let server = unsafe { OwnedHandle::from_raw_handle(raw) };
    assert_eq!(unsafe { WaitForSingleObject(server.as_raw_handle(), 0) }, WAIT_TIMEOUT);
    drop(tools);
    let start = Instant::now();
    let result = loop {
        let result = unsafe { WaitForSingleObject(server.as_raw_handle(), 0) };
        if result != WAIT_TIMEOUT || start.elapsed() >= Duration::from_secs(3) {
            break result;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    if result != WAIT_OBJECT_0 {
        // SAFETY: clean up a server left running by a failed assertion.
        unsafe { TerminateProcess(server.as_raw_handle(), 1) };
    }
    assert_eq!(result, WAIT_OBJECT_0, "MCP server survived its last tool");
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
async fn server_ping_is_answered_while_idle_and_during_a_tool_call() {
    let dir = tempfile::tempdir().unwrap();
    let idle_record = dir.path().join("idle.json");
    let idle =
        connect(config("ping-after-list", &idle_record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let idle_pong = idle_record.with_extension("pong");
    let start = Instant::now();
    while !idle_pong.exists() && start.elapsed() < Duration::from_secs(2) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(idle_pong.exists(), "MCP server did not receive idle ping response");
    let response: Value = serde_json::from_str(&std::fs::read_to_string(idle_pong).unwrap()).unwrap();
    assert_eq!(response["id"], "idle-ping");
    assert_eq!(response["result"], json!({}));
    let mut args = ara_ai::JsonObject::new();
    args.insert("text".into(), json!("after idle ping"));
    let result = idle[0].execute("call-idle", args, CancellationToken::new(), update()).await.unwrap();
    assert!(!result.is_error);

    let call_record = dir.path().join("call.json");
    let call =
        connect(config("ping-during-call", &call_record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let result = call[0].execute("call-ping", Default::default(), CancellationToken::new(), update()).await.unwrap();
    assert!(!result.is_error);
    let response: Value =
        serde_json::from_str(&std::fs::read_to_string(call_record.with_extension("pong")).unwrap()).unwrap();
    assert_eq!(response["id"], "call-ping");
    assert_eq!(response["result"], json!({}));
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
async fn malformed_frames_and_catalog_bounds_fail_before_tool_exposure() {
    let dir = tempfile::tempdir().unwrap();
    for (mode, expected) in [
        ("malformed-frame", "invalid MCP JSON-RPC frame"),
        ("truncated-frame", "closed stdout"),
        ("catalog-cycle", "cursor is invalid or repeated"),
        ("catalog-overflow", "exceeds 64 tools"),
    ] {
        let record = dir.path().join(format!("{mode}.json"));
        let error = connect(config(mode, &record), dir.path(), &["fixture:echo".into()], &[]).await.err().unwrap();
        assert!(error.contains(expected), "{mode}: {error}");
    }
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
async fn cancellation_while_waiting_for_server_does_not_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let first_marker = dir.path().join("first.txt");
    let second_marker = dir.path().join("second.txt");
    let tools = connect(config("hang", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let mut first_args = ara_ai::JsonObject::new();
    first_args.insert("marker".into(), json!(first_marker));
    let first_cancel = CancellationToken::new();
    let first = {
        let tool = tools[0].clone();
        let cancel = first_cancel.clone();
        tokio::spawn(async move { tool.execute("first", first_args, cancel, update()).await.unwrap() })
    };
    let start = Instant::now();
    while !first_marker.exists() && start.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(first_marker.exists(), "first call was not dispatched");
    let mut second_args = ara_ai::JsonObject::new();
    second_args.insert("marker".into(), json!(second_marker));
    let second_cancel = CancellationToken::new();
    let second = tools[0].execute("second", second_args, second_cancel.clone(), update());
    tokio::pin!(second);
    assert!(tokio::time::timeout(Duration::from_millis(50), &mut second).await.is_err(), "second call was not waiting");
    second_cancel.cancel();
    let result = tokio::time::timeout(Duration::from_secs(2), second).await.unwrap().unwrap();
    assert!(result.is_error);
    assert_eq!(result.details.unwrap()["executed"], false);
    assert!(!second_marker.exists(), "cancelled call reached the server");
    first_cancel.cancel();
    let first_result = tokio::time::timeout(Duration::from_secs(2), first).await.unwrap().unwrap();
    assert_eq!(first_result.details.unwrap()["executed"], "unknown");
}

#[cfg(windows)]
#[tokio::test]
async fn dropped_dispatched_call_closes_server_before_another_call() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let marker = dir.path().join("effect.txt");
    let tools = connect(config("hang", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let receipt: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    let pid = receipt["pid"].as_u64().unwrap() as u32;
    // SAFETY: OpenProcess returns a new handle that OwnedHandle closes.
    let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid) };
    assert!(!raw.is_null(), "could not open running MCP fixture: {}", std::io::Error::last_os_error());
    let server = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut args = ara_ai::JsonObject::new();
    args.insert("marker".into(), json!(marker));
    let first = {
        let tool = tools[0].clone();
        tokio::spawn(async move { tool.execute("first", args, CancellationToken::new(), update()).await.unwrap() })
    };
    let start = Instant::now();
    while !marker.exists() && start.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(marker.exists(), "first call was not dispatched");
    first.abort();
    assert!(first.await.is_err());
    let start = Instant::now();
    let exit = loop {
        let result = unsafe { WaitForSingleObject(server.as_raw_handle(), 0) };
        if result != WAIT_TIMEOUT || start.elapsed() >= Duration::from_secs(3) {
            break result;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    if exit != WAIT_OBJECT_0 {
        // SAFETY: clean up a server left running by a failed assertion.
        unsafe { TerminateProcess(server.as_raw_handle(), 1) };
    }
    assert_eq!(exit, WAIT_OBJECT_0, "abandoned call kept the MCP server alive");
    let second = tokio::time::timeout(
        Duration::from_secs(2),
        tools[0].execute("second", Default::default(), CancellationToken::new(), update()),
    )
    .await
    .expect("another call waited on the abandoned request")
    .unwrap();
    assert!(second.is_error);
    assert_eq!(second.details.unwrap()["executed"], false);
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
async fn large_server_error_does_not_fill_tool_result() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools =
        connect(config("large-jsonrpc-error", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let result = tools[0].execute("call-error", Default::default(), CancellationToken::new(), update()).await.unwrap();
    assert!(result.is_error);
    assert_eq!(result.details.unwrap()["executed"], "unknown");
    let text = result
        .content
        .iter()
        .filter_map(|block| match block {
            ara_ai::UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert!(text.contains("777"), "{text}");
    assert!(text.contains("truncated"), "{text}");
    assert!(text.len() < 5000, "error grew to {} bytes", text.len());
}

#[tokio::test]
async fn oversized_outbound_frame_is_not_dispatched_and_keeps_server_available() {
    let dir = tempfile::tempdir().unwrap();
    let record = dir.path().join("record.json");
    let tools = connect(config("normal", &record), dir.path(), &["fixture:echo".into()], &[]).await.unwrap();
    let mut large = ara_ai::JsonObject::new();
    large.insert("text".into(), json!("x".repeat(1024 * 1024)));
    let rejected = tools[0].execute("call-too-large", large, CancellationToken::new(), update()).await.unwrap();
    assert!(rejected.is_error);
    assert_eq!(rejected.details.unwrap()["executed"], false);
    let before: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert!(before.get("call").is_none(), "the large request reached the server");

    let mut small = ara_ai::JsonObject::new();
    small.insert("text".into(), json!("still alive"));
    let accepted = tools[0].execute("call-small", small, CancellationToken::new(), update()).await.unwrap();
    assert!(!accepted.is_error);
    assert_eq!(accepted.content, vec![ara_ai::UserBlock::text("echo: still alive")]);
    let after: Value = serde_json::from_str(&std::fs::read_to_string(&record).unwrap()).unwrap();
    assert_eq!(after["call"]["arguments"]["text"], "still alive");
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
