//! Requirement 19.4–19.7 controlled test: a compaction summary with
//! Compaction_Refresh re-reads the current disk state of the files the old
//! history touched and the plan state from the Session; the request shows
//! the labelled summary first, then the refresh messages; the summarized
//! raw journal is unchanged. Without the policy no refresh appears and the
//! Run matches `tests/golden/p2d-no-refresh/`.
mod golden_support;
mod support;

use std::path::Path;

use ara_agent_lite_core::{
    compaction_refresh::{FileToolKind, RefreshPolicy},
    context_memory,
    session_journal::Journal,
};
use serde_json::{json, Value};
use support::*;

/// Context window whose 30 000-byte projection cannot hold Run 1's ~16 KiB
/// answer next to Run 2's ~16 KiB objective, so Run 2 must summarize Run 1.
const CONTEXT_WINDOW: u64 = 60_000 + 512 + 4096;

fn file_tools(root: &Path) -> Vec<std::sync::Arc<ScriptedTool>> {
    let write_root = root.to_path_buf();
    let read_root = root.to_path_buf();
    vec![
        ScriptedTool::new("ws_write_file", move |args| {
            let path = write_root.join(args["path"].as_str().unwrap());
            std::fs::write(&path, args["content"].as_str().unwrap()).map_err(|error| error.to_string())?;
            Ok(format!("wrote {}", path.display()))
        }),
        ScriptedTool::new("ws_read_file", move |args| {
            std::fs::read_to_string(read_root.join(args["path"].as_str().unwrap())).map_err(|error| error.to_string())
        }),
        // Succeeds with a `path` argument but is not a mapped file tool.
        ScriptedTool::new("exec_fixture", |_| Ok("ran".into())),
    ]
}

fn policy() -> RefreshPolicy {
    RefreshPolicy {
        max_files: 4,
        max_bytes: 16 * 1024,
        file_tools: [
            ("ws_write_file".to_owned(), FileToolKind::Write),
            ("ws_read_file".to_owned(), FileToolKind::Read),
        ]
        .into(),
    }
}

struct Case {
    root: TempDir,
    events: Vec<Value>,
    bodies: Vec<Value>,
    journal_before: (usize, String, Value),
    journal_after: (String, Value),
    endpoint: String,
}

fn journal_state(session: &Path, count: usize) -> (String, Value) {
    let journal = Journal::for_session(session);
    (journal.proof(0, count).unwrap(), serde_json::to_value(journal.read(0, count, 16 * 1024 * 1024).unwrap()).unwrap())
}

async fn compact(refresh: Option<RefreshPolicy>) -> Case {
    let root = TempDir::new();
    let tools = file_tools(root.path());
    let provider = Provider::with(ProviderConfig {
        context_window: CONTEXT_WINDOW,
        ..ProviderConfig::new(vec![
            calls(&[
                ("w1", "ws_write_file", json!({"path":"a.txt","content":"a-v1 stale"})),
                ("w2", "ws_write_file", json!({"path":"b.txt","content":"b-v1 OLD-B-CONTENT"})),
                ("w3", "exec_fixture", json!({"path":"c.txt"})),
            ]),
            call(
                "t1",
                "task_update",
                json!({"id":"s1","title":"write a and b","status":"done","evidence":"wrote both files"}),
            ),
            text(&"r".repeat(16 * 1024)),
            text("continued from disk"),
        ])
    });
    let mut first = request(root.path(), &provider, &tools, 4, "Write a.txt and b.txt, then report.");
    first.max_output = 512;
    let (result, _) = run(first).await;
    assert_eq!(result.code, 0, "{}", result.error);
    // Disk changes after Run 1 and before the compaction in Run 2.
    std::fs::write(root.path().join("a.txt"), "a-v2 CURRENT-A").unwrap();
    std::fs::remove_file(root.path().join("b.txt")).unwrap();
    let session = root.path().join("session.json");
    let count = Journal::for_session(&session).open().unwrap().raw_count;
    let (proof, messages) = journal_state(&session, count);
    let mut second = request(root.path(), &provider, &tools, 3, &format!("Continue. {}", "q".repeat(16 * 1024)));
    second.policies.compaction_refresh = refresh;
    let (result, events) = run(second).await;
    assert_eq!(result.code, 0, "{}", result.error);
    let journal_after = journal_state(&session, count);
    let bodies = provider.bodies();
    let endpoint = provider.endpoint.clone();
    Case { root, events, bodies, journal_before: (count, proof, messages), journal_after, endpoint }
}

#[tokio::test]
async fn refresh_follows_the_summary_with_current_disk_and_session_plan_state() {
    let case = compact(Some(policy())).await;
    let summary_requests = case.bodies.iter().filter(|body| is_summary(body)).count();
    assert_eq!(summary_requests, 1, "Run 2 compacted Run 1 once");

    let refresh = of_type(&case.events, "compaction_refresh");
    assert_eq!(refresh.len(), 1);
    let refresh = refresh[0];
    let path = |name: &str| case.root.path().join(name);
    let a_sha = format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(b"a-v2 CURRENT-A"));
    // b.txt was written after a.txt, so it is the most recent candidate.
    assert_eq!(
        refresh["files"],
        json!([
            {"path": path("b.txt"), "result": {"result":"read_error","kind":"not_found"}, "sha256": null},
            {"path": path("a.txt"), "result": {"result":"full"}, "sha256": a_sha},
        ])
    );
    assert_eq!(refresh["plan_state"], true);
    let compacted = case.events.iter().position(|event| event["type"] == "compaction_refresh").unwrap();
    let next_call = case.events.iter().rposition(|event| event["type"] == "model_call").unwrap();
    assert!(compacted < next_call, "refresh is recorded before the next agent call");

    let agent = case.bodies.last().unwrap().to_string();
    assert!(!is_summary(case.bodies.last().unwrap()));
    // The working-context view (source directory excerpts, then the labelled
    // summary) comes first; the refresh messages follow it.
    let summary_at = agent.find("compaction_summary (historical, may be stale)").expect("labelled summary");
    let after = |needle: &str| agent[summary_at..].find(needle).map(|offset| summary_at + offset);
    let plan_at = after("source=compaction_refresh; current plan state").expect("plan refresh");
    let b_at = after(&format!("path={}", path("b.txt").display()).replace('\\', "\\\\")).expect("b refresh");
    let a_at = after(&format!("path={}", path("a.txt").display()).replace('\\', "\\\\")).expect("a refresh");
    assert!(plan_at < b_at && b_at < a_at, "plan, then files newest first");
    assert!(agent.contains("a-v2 CURRENT-A") && !agent.contains("a-v1 stale"), "current disk bytes only");
    assert!(agent.contains(r#"error=\"not_found\""#));
    assert!(!agent.contains("OLD-B-CONTENT"), "no stale content for an unreadable file");
    // The plan state comes from the Session's last successful task_update result.
    assert!(agent[plan_at..b_at].contains("wrote both files"));
    assert!(!agent.contains("c.txt"), "only mapped file tools are candidates");

    let (_, proof, messages) = &case.journal_before;
    assert_eq!(&case.journal_after.0, proof, "summarized raw records are unchanged");
    assert_eq!(&case.journal_after.1, messages);
}

#[tokio::test]
async fn without_the_policy_no_refresh_message_or_event_appears() {
    let case = compact(None).await;
    assert_eq!(case.bodies.iter().filter(|body| is_summary(body)).count(), 1);
    assert!(of_type(&case.events, "compaction_refresh").is_empty());
    for body in &case.bodies {
        let body = body.to_string();
        assert!(!body.contains("source=compaction_refresh"));
        assert!(!body.contains("(historical, may be stale)"));
    }
    let (_, proof, messages) = &case.journal_before;
    assert_eq!(&case.journal_after.0, proof);
    assert_eq!(&case.journal_after.1, messages);

    // Requirement 19.7: summary text, projection and Session as tests/golden/p2d-no-refresh/.
    let session = case.root.path().join("session.json");
    let store: Value = serde_json::from_slice(&std::fs::read(context_memory::path(&session)).unwrap()).unwrap();
    let agent = case.bodies.last().unwrap();
    assert!(!is_summary(agent));
    let mut normalizer = golden_support::Normalizer::new(case.root.path(), &case.endpoint);
    golden_support::check("p2d-no-refresh", "summary.json", &normalizer.value(&store["summary"]["text"]));
    golden_support::check("p2d-no-refresh", "projection.json", &normalizer.value(&agent["messages"]));
    let journal = golden_support::journal_entries(&session);
    golden_support::check("p2d-no-refresh", "session.json", &normalizer.value(&journal));
    golden_support::check("p2d-no-refresh", "events.json", &golden_support::event_types(&case.events));
}
