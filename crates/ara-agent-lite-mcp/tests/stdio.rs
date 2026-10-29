//! Controlled stdio server: handshake, pagination, naming, version check,
//! startup timeout, `required`, cancellation, disconnect, `-32601`,
//! resources, stderr bounds and process-tree shutdown (42.3, 42.6, 42.7, 42.8).
mod common;

use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

use ara_agent_lite_core::{effect_status::reports_unknown, rpi_agent::TextContentOrImage};
use ara_agent_lite_mcp::{McpError, McpOptions, McpSession, ServerStatus, LIST_RESOURCES_TOOL, READ_RESOURCE_TOOL};
use common::*;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn handshake_paginates_and_names_tools() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.txt");
    let server = fake(&["--naming", "--page-size", "3", "--instructions", "Prefer echo."], Some(&log), &[]);
    let session = connect(config(vec![("my.server", server)]), options()).await;
    let names = names(&session);
    for expected in
        ["mcp__my_server__echo", "mcp__my_server__weird_name_", "mcp__my_server__dup_x", "mcp__my_server__choice"]
    {
        assert!(names.contains(&expected.to_string()), "{expected} missing from {names:?}");
    }
    assert!(!names.iter().any(|name| name.contains("xxxxxxxx")), "over-long name must be excluded");
    assert!(names.iter().all(|name| name.len() <= 128));
    let reasons: Vec<_> = session.excluded().iter().map(|e| (e.tool.as_str(), e.reason.as_str())).collect();
    assert!(reasons.iter().any(|(tool, reason)| *tool == "dup_x" && reason.contains("duplicate")), "{reasons:?}");
    assert!(reasons.iter().any(|(tool, reason)| tool.starts_with("xxx") && reason.contains("128")), "{reasons:?}");
    assert!(reasons.iter().any(|(tool, reason)| *tool == "not_object" && reason.contains("object")), "{reasons:?}");
    let ServerStatus::Connected { tools, instructions } = &session.status()["my.server"] else { panic!() };
    assert_eq!(*tools, names.len());
    assert_eq!(instructions.as_deref(), Some("Prefer echo."));
    let choice = tool(&session, "mcp__my_server__choice");
    assert!(choice
        .schema()
        .description
        .starts_with("Provide exactly one of these parameter groups together: [a] or [b, c]."));
    let parameters = &choice.schema().parameters.0;
    assert_eq!(parameters["type"], "object");
    assert!(parameters.get("oneOf").is_none());
    assert!(parameters["properties"].get("c").is_some());
    let lines = log_lines(&log);
    assert_eq!(lines[0], "initialize 2025-11-25");
    assert!(lines.iter().any(|line| line.starts_with("notification notifications/initialized")));
    assert!(count(&lines, "tools/list") >= 6, "{lines:?}");
    session.shutdown().await;
    assert!(count(&wait_for(&log, "stdin closed").await, "stdin closed") == 1);
}

#[tokio::test]
async fn calls_convert_content_and_inherit_environment() {
    let dir = tempfile::tempdir().unwrap();
    let server = fake(&[], None, &[("FROM_CONFIG", "${ARA_MCP_UNSET_FOR_TEST:-fallback-value}")]);
    let mut opts = options();
    opts.results_dir = Some(dir.path().join("results"));
    let session = connect(config(vec![("fake", server)]), opts).await;
    let no_cancel = CancellationToken::new;
    let echo = run(&tool(&session, "mcp__fake__echo"), json!({"text":"hello"}), no_cancel()).await.unwrap();
    assert_eq!(text_of(&echo), "hello");
    assert!(!reports_unknown(&echo.details));
    let env = tool(&session, "mcp__fake__env");
    assert_eq!(text_of(&run(&env, json!({"name":"FROM_CONFIG"}), no_cancel()).await.unwrap()), "fallback-value");
    assert_ne!(text_of(&run(&env, json!({"name":"PATH"}), no_cancel()).await.unwrap()), "<unset>");
    let image = run(&tool(&session, "mcp__fake__image"), json!({}), no_cancel()).await.unwrap();
    assert!(matches!(&image.content[1], TextContentOrImage::Image(i) if i.mime_type == "image/png"));
    let structured = run(&tool(&session, "mcp__fake__structured"), json!({}), no_cancel()).await.unwrap();
    assert_eq!(structured.details["structuredContent"], json!({"answer":42}));
    let resource = run(&tool(&session, "mcp__fake__resource"), json!({}), no_cancel()).await.unwrap();
    assert!(text_of(&resource).contains("\"resource_link\""));
    let failed = error_text(run(&tool(&session, "mcp__fake__fail"), json!({"n":12000}), no_cancel()).await);
    assert!(failed.contains("[... 2000 characters omitted ...]"), "{}", failed.len());
    let big = run(&tool(&session, "mcp__fake__big"), json!({"n":60000}), no_cancel()).await.unwrap();
    let path = big.details["full_result_path"].as_str().expect("saved path");
    assert_eq!(std::fs::read_to_string(path).unwrap().len(), 60000);
    assert!(text_of(&big).len() < 3000);
    session.shutdown().await;
}

#[tokio::test]
async fn one_failed_server_leaves_others_and_required_fails_the_connect() {
    let started = Instant::now();
    let servers = || {
        config(vec![
            ("good", fake(&[], None, &[])),
            ("oldversion", fake(&["--protocol", "2024-11-05"], None, &[])),
            ("slowpoke", fake(&["--init-delay", "10"], None, &[])),
            ("missing", fake(&[], None, &[("KEY", "${ARA_MCP_MISSING_FOR_TEST}")])),
        ])
    };
    let mut opts = options();
    opts.startup_timeout = Duration::from_millis(2500);
    let session = connect(servers(), opts.clone()).await;
    assert!(started.elapsed() < Duration::from_secs(8), "servers connect concurrently");
    assert!(matches!(session.status()["good"], ServerStatus::Connected { .. }));
    let reason = |name: &str| match &session.status()[name] {
        ServerStatus::Failed { reason } => reason.clone(),
        other => panic!("{name}: {other:?}"),
    };
    assert!(reason("oldversion").contains("unsupported protocol version \"2024-11-05\""));
    assert!(reason("slowpoke").contains("startup timed out"));
    assert!(reason("missing").contains("missing_variable") && reason("missing").contains("ARA_MCP_MISSING_FOR_TEST"));
    assert!(names(&session).iter().all(|name| name.starts_with("mcp__good__")));
    session.shutdown().await;

    opts.required = BTreeSet::from(["slowpoke".to_string()]);
    let error = McpSession::connect(servers(), opts.clone(), CancellationToken::new()).await.err().unwrap();
    assert!(
        matches!(&error, McpError::RequiredFailed { server, reason } if server == "slowpoke" && reason.contains("timed out"))
    );
    opts.required = BTreeSet::from(["absent".to_string()]);
    let error = McpSession::connect(servers(), opts, CancellationToken::new()).await.err().unwrap();
    assert_eq!(error, McpError::UnknownRequired { server: "absent".into() });
}

#[tokio::test]
async fn startup_failure_reports_a_bounded_redacted_stderr_tail() {
    let secret = "fixture-secret-7Qx9";
    let server = fake(&["--stderr-flood", "100000", "--exit-at-start"], None, &[("FAKE_SECRET", secret)]);
    let session = connect(config(vec![("broken", server)]), options()).await;
    let ServerStatus::Failed { reason } = &session.status()["broken"] else { panic!() };
    assert!(reason.contains("stderr tail"), "{reason}");
    assert!(reason.contains("secret=[redacted]"), "{reason}");
    assert!(!reason.contains(secret));
    assert!(reason.len() < 2000, "{}", reason.len());
    assert!(format!("{:?}", session.status()).find(secret).is_none());
}

#[tokio::test]
async fn cancel_timeout_and_disconnect_mark_unknown_effects_once() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.txt");
    let mut opts = options();
    opts.call_timeout = Duration::from_millis(600);
    let session = connect(config(vec![("fake", fake(&[], Some(&log), &[]))]), opts).await;

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;
        trigger.cancel();
    });
    let slow = tool(&session, "mcp__fake__slow");
    let cancelled = run(&slow, json!({"ms":400}), cancel).await.unwrap();
    assert_eq!(cancelled.details["source"], "mcp_call_cancelled");
    assert!(reports_unknown(&cancelled.details));
    let lines = wait_for(&log, "notification notifications/cancelled").await;
    assert!(lines.iter().any(|line| line.contains("\"reason\": \"cancelled by host\"")), "{lines:?}");

    let timed_out = run(&slow, json!({"ms":2000}), CancellationToken::new()).await.unwrap();
    assert_eq!(
        timed_out.details,
        json!({"effects_unknown":true,"source":"mcp_call_timeout","server":"fake","tool":"slow"})
    );
    let read_only =
        error_text(run(&tool(&session, "mcp__fake__slow_ro"), json!({"ms":2000}), CancellationToken::new()).await);
    assert!(read_only.contains("read-only"));

    let lost = run(&tool(&session, "mcp__fake__die"), json!({}), CancellationToken::new()).await.unwrap();
    assert_eq!(lost.details["source"], "mcp_connection_lost");
    let after = run(&tool(&session, "mcp__fake__echo"), json!({"text":"x"}), CancellationToken::new()).await;
    assert!(error_text(after).contains("not sent"));
    let lines = log_lines(&log);
    assert_eq!(count(&lines, "call slow "), 2, "no retries: {lines:?}");
    assert_eq!(count(&lines, "call die "), 1);
    session.shutdown().await;
}

#[tokio::test]
async fn server_requests_get_method_not_found() {
    let session = connect(config(vec![("fake", fake(&[], None, &[]))]), options()).await;
    let reply = run(&tool(&session, "mcp__fake__ask_client"), json!({}), CancellationToken::new()).await.unwrap();
    assert_eq!(text_of(&reply), "codes:-32601,-32601,-32601,ok");
    session.shutdown().await;
}

#[tokio::test]
async fn resource_tools_follow_the_capability() {
    let session = connect(
        config(vec![("docs", fake(&["--resources", "5", "--no-tools"], None, &[])), ("plain", fake(&[], None, &[]))]),
        options(),
    )
    .await;
    assert!(matches!(session.status()["docs"], ServerStatus::Connected { tools: 0, .. }));
    let listed = run(&tool(&session, LIST_RESOURCES_TOOL), json!({}), CancellationToken::new()).await.unwrap();
    let entries: serde_json::Value = serde_json::from_str(&text_of(&listed)).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 5, "{entries}");
    assert_eq!(entries[4], json!({"server":"docs","uri":"fake://doc/4","name":"doc 4","mimeType":"text/plain"}));
    let read = tool(&session, READ_RESOURCE_TOOL);
    let content = run(&read, json!({"server":"docs","uri":"fake://doc/1"}), CancellationToken::new()).await.unwrap();
    assert_eq!(text_of(&content), "content of fake://doc/1");
    let unknown = run(&read, json!({"server":"plain","uri":"x"}), CancellationToken::new()).await;
    assert!(error_text(unknown).contains("offers resources"));
    session.shutdown().await;

    let session = connect(config(vec![("plain", fake(&[], None, &[]))]), options()).await;
    assert!(!names(&session).iter().any(|name| name == LIST_RESOURCES_TOOL || name == READ_RESOURCE_TOOL));
    session.shutdown().await;
}

#[tokio::test]
async fn page_bound_is_reported() {
    let session =
        connect(config(vec![("fake", fake(&["--page-size", "1", "--endless-pages"], None, &[]))]), options()).await;
    assert!(session.excluded().iter().any(|e| e.tool == "*" && e.reason.contains("page limit")));
    session.shutdown().await;
}

#[tokio::test]
async fn shutdown_ends_the_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("grandchild.pid");
    let server = fake(&["--grandchild", pid_file.to_str().unwrap()], None, &[]);
    let session = connect(config(vec![("tree", server)]), options()).await;
    let mut pid = String::new();
    for _ in 0..100 {
        pid = std::fs::read_to_string(&pid_file).unwrap_or_default();
        if !pid.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid: u32 = pid.trim().parse().expect("grandchild pid");
    assert!(alive(pid));
    let started = Instant::now();
    session.shutdown().await;
    assert!(started.elapsed() < Duration::from_secs(6));
    for _ in 0..40 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("grandchild {pid} survived session shutdown");
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    let output =
        std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().unwrap().success()
}

#[tokio::test]
async fn connect_cancellation_shuts_servers_down() {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let outcome = McpSession::connect(
        config(vec![("slowpoke", fake(&["--init-delay", "10"], None, &[])), ("good", fake(&[], None, &[]))]),
        McpOptions { startup_timeout: Duration::from_secs(20), ..Default::default() },
        cancel,
    )
    .await;
    assert_eq!(outcome.err(), Some(McpError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(8));
}
