//! End-to-end flows against a real `ara-lite serve` process, driven through the real CLI
//! binary and the real MCP stdio server. Failure paths: stale versions, lease expiry,
//! restart recovery and an empty poll timeout.

mod common;

use ara_lite::profile::save_private;
use ara_lite::types::{Connection, Time};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use common::{McpProcess, ServeProcess, Service, cli_err, cli_ok, free_loopback_addr, git_fixture, json};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Run a CLI command as `profile` and parse its `--json` output.
fn call(profile: &Path, args: &[&str]) -> Value {
    let mut all = vec![args[0], "--profile", profile.to_str().unwrap()];
    all.extend_from_slice(&args[1..]);
    all.push("--json");
    json(&cli_ok(&all))
}

/// Run a CLI command that must fail and return its stderr.
fn fails(profile: &Path, args: &[&str]) -> String {
    let mut all = vec![args[0], "--profile", profile.to_str().unwrap()];
    all.extend_from_slice(&args[1..]);
    cli_err(&all)
}

fn num(task: &Value, key: &str) -> String {
    task[key].as_i64().unwrap_or_else(|| panic!("{key} missing in {task}")).to_string()
}

fn instant(task: &Value, key: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(task[key].as_str().unwrap()).unwrap().with_timezone(&Utc)
}

fn event_kinds(poll: &Value) -> Vec<String> {
    poll["messages"].as_array().unwrap().iter().filter_map(|m| m["event_kind"].as_str()).map(str::to_string).collect()
}

fn worker_id(clients: &Value) -> String {
    clients.as_array().unwrap().iter().find(|c| c["role"] == "worker").unwrap()["id"].as_str().unwrap().to_string()
}

#[test]
fn cli_lifecycle_against_a_real_service() {
    let p = ServeProcess::start();
    let master = p.join("master", "e2e-master");
    let worker = p.join("worker", "e2e-worker");
    let workspace = git_fixture();
    let ws = workspace.path().to_str().unwrap();

    let clients = call(&master, &["clients"]);
    assert_eq!(clients.as_array().unwrap().len(), 2, "{clients}");
    let wid = worker_id(&clients);

    // create
    let task = call(
        &master,
        &["create", "--title", "E2E task", "--workspace", ws, "--body", "Do the thing", "--worker-id", &wid],
    );
    assert_eq!(
        (task["state"].as_str(), task["version"].as_i64(), task["generation"].as_i64()),
        (Some("queued"), Some(1), Some(0))
    );
    let id = task["id"].as_str().unwrap().to_string();

    // claim
    let claimed = call(&worker, &["claim", "--task-id", &id, "--version", &num(&task, "version"), "--generation", "0"]);
    assert_eq!((claimed["state"].as_str(), claimed["generation"].as_i64()), (Some("running"), Some(1)));
    let lease = instant(&claimed, "lease_until");
    assert!(lease > Utc::now() + ChronoDuration::seconds(60), "lease must run about two minutes: {lease}");

    // progress: overall, then one subtask
    let overall = call(
        &worker,
        &[
            "progress",
            "--task-id",
            &id,
            "--version",
            &num(&claimed, "version"),
            "--generation",
            "1",
            "--percent",
            "40",
            "--description",
            "half done",
        ],
    );
    assert_eq!(overall["progress_percent"], 40);
    // A progress report by the owner renews the lease (round 2, simplification proposal 1).
    assert!(instant(&overall, "lease_until") > lease, "progress must move the lease forward: {overall}");
    let sub = call(
        &worker,
        &[
            "progress",
            "--task-id",
            &id,
            "--version",
            &num(&overall, "version"),
            "--generation",
            "1",
            "--percent",
            "100",
            "--description",
            "tests green",
            "--subtask-id",
            "tests",
            "--subtask-title",
            "Run tests",
        ],
    );
    assert_eq!(sub["progress_percent"], 40, "a subtask report must not move the overall percent");
    assert_eq!(sub["subtasks"][0]["percent"], 100);

    // A stale version is refused and changes nothing.
    let stale = fails(
        &worker,
        &[
            "progress",
            "--task-id",
            &id,
            "--version",
            &num(&overall, "version"),
            "--generation",
            "1",
            "--percent",
            "50",
            "--description",
            "stale write",
        ],
    );
    assert!(stale.contains("request rejected (409)") && stale.contains("version conflict"), "{stale}");
    // A stale generation is fenced too.
    let fenced =
        fails(&worker, &["heartbeat", "--task-id", &id, "--version", &num(&sub, "version"), "--generation", "0"]);
    assert!(fenced.contains("stale claim generation"), "{fenced}");

    // heartbeat renews the lease
    let beat = call(&worker, &["heartbeat", "--task-id", &id, "--version", &num(&sub, "version"), "--generation", "1"]);
    assert_eq!(beat["state"], "running");
    assert!(instant(&beat, "lease_until") >= lease);

    // chat + master's view of the events, then a cursor-safe ack
    call(&worker, &["send", "--body", "question for the master"]);
    let poll = call(&master, &["poll", "--after", "-1", "--timeout", "0s"]);
    assert_eq!(event_kinds(&poll), ["claimed", "progress", "progress"], "{poll}");
    let messages = poll["messages"].as_array().unwrap();
    assert!(messages.iter().any(|m| m["kind"] == "chat" && m["body"] == "question for the master"), "{poll}");
    let last = messages.last().unwrap()["seq"].as_i64().unwrap().to_string();
    let ack = call(
        &master,
        &["ack", "--expected-cursor", &poll["ack_cursor"].as_i64().unwrap().to_string(), "--ack-seq", &last],
    );
    assert_eq!(ack["ack_cursor"].as_i64().unwrap().to_string(), last);
    let after_ack = call(&master, &["poll", "--after", "-1", "--timeout", "0s"]);
    assert!(after_ack["messages"].as_array().unwrap().is_empty(), "acknowledged messages came back: {after_ack}");

    // submit records the workspace snapshot; the master reviews exactly that state
    let submitted = call(
        &worker,
        &[
            "submit",
            "--task-id",
            &id,
            "--version",
            &num(&beat, "version"),
            "--generation",
            "1",
            "--body",
            "implemented and tested",
        ],
    );
    assert_eq!(submitted["state"], "awaiting_review");
    let snapshot = submitted["snapshot"].as_str().unwrap().to_string();
    assert_eq!(cli_ok(&["snapshot", ws]), snapshot);
    let passed = call(
        &master,
        &[
            "review",
            "--task-id",
            &id,
            "--version",
            &num(&submitted, "version"),
            "--generation",
            "1",
            "--snapshot",
            &snapshot,
            "--decision",
            "pass",
            "--body",
            "verified",
        ],
    );
    assert_eq!(passed["state"], "passed");

    // The worker learns the verdict; the master sees the whole task history.
    let worker_poll = call(&worker, &["poll", "--after", "-1", "--timeout", "0s"]);
    assert_eq!(event_kinds(&worker_poll), ["queued", "pass"], "{worker_poll}");
    let master_poll = call(&master, &["poll", "--after", "-1", "--timeout", "0s"]);
    assert_eq!(event_kinds(&master_poll), ["submitted"], "{master_poll}");
    let status = call(&master, &["status"]);
    assert_eq!(status["tasks"][&id]["state"], "passed");
    // History is per recipient and outlives acknowledgement: each side sees the events sent to it.
    let master_history = call(&master, &["history", "--task-id", &id, "--limit", "50"]);
    let kinds = |v: &Value| event_kinds(&json!({ "messages": v }));
    assert_eq!(kinds(&master_history), ["claimed", "progress", "progress", "submitted"], "{master_history}");
    let worker_history = call(&worker, &["history", "--task-id", &id, "--limit", "50"]);
    assert_eq!(kinds(&worker_history), ["queued", "pass"], "{worker_history}");

    // A long poll returns as soon as a message for this worker is sent, not at its timeout.
    let cursor = num(&worker_poll, "next_cursor");
    let waiting = {
        let (profile, cursor) = (worker.to_str().unwrap().to_string(), cursor.clone());
        std::thread::spawn(move || {
            let started = Instant::now();
            let out = cli_ok(&["poll", "--profile", &profile, "--after", &cursor, "--timeout", "20s", "--json"]);
            (out, started.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(700));
    call(&master, &["send", "--worker-id", &wid, "--body", "wake up"]);
    let (out, elapsed) = waiting.join().unwrap();
    assert!(out.contains("wake up"), "{out}");
    assert!(elapsed < Duration::from_secs(10), "poll ran to its timeout instead of waking: {elapsed:?}");

    // With nothing new, a poll waits for its timeout and returns an empty page.
    let cursor = num(&json(&out), "next_cursor");
    let started = Instant::now();
    let empty = call(&worker, &["poll", "--after", &cursor, "--timeout", "1s"]);
    assert!(started.elapsed() >= Duration::from_millis(900), "returned early: {:?}", started.elapsed());
    assert!(empty["messages"].as_array().unwrap().is_empty(), "{empty}");
}

#[test]
fn mcp_lifecycle_against_a_real_service() {
    let p = ServeProcess::start();
    let master_profile = p.join("master", "mcp-master");
    let worker_profile = p.join("worker", "mcp-worker");
    let mut master = McpProcess::start(&master_profile);
    let mut worker = McpProcess::start(&worker_profile);
    let workspace = git_fixture();
    let ws = workspace.path().to_str().unwrap();

    let (err, clients) = master.tool("list_clients", json!({}));
    assert!(!err, "{clients}");
    assert_eq!(clients["clients"].as_array().unwrap().len(), 2);
    let wid = worker_id(&clients["clients"]);

    let (err, task) = master.tool(
        "create_task",
        json!({ "title": "MCP task", "workspace": ws, "body": "Do the thing", "worker_id": wid, "request_id": "create-mcp" }),
    );
    assert!(!err, "{task}");
    let id = task["id"].as_str().unwrap().to_string();
    // Retrying with the same request_id and arguments returns the recorded result.
    let (_, again) = master.tool(
        "create_task",
        json!({ "title": "MCP task", "workspace": ws, "body": "Do the thing", "worker_id": wid, "request_id": "create-mcp" }),
    );
    assert_eq!(again["id"], task["id"]);
    let (_, listed) = master.tool("list_tasks", json!({}));
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 1, "the retry created a duplicate: {listed}");

    let (err, claimed) = worker.tool("claim_task", json!({ "task_id": id, "version": 1, "generation": 0 }));
    assert!(!err, "{claimed}");
    assert_eq!(claimed["state"], "running");
    let (err, progress) = worker.tool(
        "update_progress",
        json!({
            "task_id": id, "version": claimed["version"], "generation": 1, "percent": 60, "description": "most of it done",
            "subtask_id": "impl", "subtask_title": "Implementation"
        }),
    );
    assert!(!err, "{progress}");
    assert_eq!(progress["subtasks"][0]["id"], "impl");

    // A stale version is a tool error that names the rejection and changes nothing.
    let (err, message) = worker.tool(
        "update_progress",
        json!({ "task_id": id, "version": claimed["version"], "generation": 1, "percent": 70, "description": "stale" }),
    );
    assert!(err);
    let text = message.as_str().unwrap();
    assert!(text.contains("request rejected (409)") && text.contains("version conflict"), "{text}");

    // heartbeat is idempotent under one request_id
    let beat_args = json!({ "task_id": id, "version": progress["version"], "generation": 1, "request_id": "beat-1" });
    let (err, beat) = worker.tool("heartbeat", beat_args.clone());
    assert!(!err, "{beat}");
    let (_, beat_again) = worker.tool("heartbeat", beat_args);
    assert_eq!(beat["version"], beat_again["version"], "a retried heartbeat must not advance the task");

    let (err, submitted) = worker.tool(
        "submit_task",
        json!({ "task_id": id, "version": beat["version"], "generation": 1, "body": "implemented and tested" }),
    );
    assert!(!err, "{submitted}");
    assert_eq!(submitted["state"], "awaiting_review");

    // The master pins the reviewed state with snapshot_task, then reviews exactly that.
    let (err, snap) = master.tool("snapshot_task", json!({ "task_id": id }));
    assert!(!err, "{snap}");
    assert_eq!(snap["snapshot"], submitted["snapshot"]);
    std::fs::write(workspace.path().join("late.txt"), "changed after submit").unwrap();
    let (err, refused) = master.tool(
        "review_task",
        json!({
            "task_id": id, "version": submitted["version"], "generation": 1, "decision": "pass", "body": "looks fine",
            "snapshot": snap["snapshot"]
        }),
    );
    assert!(err, "a review of a changed workspace must be refused");
    assert!(refused.as_str().unwrap().contains("workspace changed"), "{refused}");
    std::fs::remove_file(workspace.path().join("late.txt")).unwrap();
    let (err, passed) = master.tool(
        "review_task",
        json!({
            "task_id": id, "version": submitted["version"], "generation": 1, "decision": "pass", "body": "verified",
            "snapshot": snap["snapshot"]
        }),
    );
    assert!(!err, "{passed}");
    assert_eq!(passed["state"], "passed");

    // The worker polls the verdict and acknowledges exactly what it handled.
    let (err, poll) = worker.tool("poll_messages", json!({ "timeout_seconds": 1 }));
    assert!(!err, "{poll}");
    assert_eq!(event_kinds(&poll), ["queued", "pass"], "{poll}");
    let last = poll["messages"].as_array().unwrap().last().unwrap()["seq"].clone();
    let (err, ack) = worker.tool("ack_messages", json!({ "expected_cursor": poll["ack_cursor"], "ack_seq": last }));
    assert!(!err, "{ack}");
    assert_eq!(ack["ack_cursor"], last);

    // Nothing new: the poll waits out its timeout and returns an empty page.
    let started = Instant::now();
    let (err, empty) = worker.tool("poll_messages", json!({ "timeout_seconds": 1 }));
    assert!(!err, "{empty}");
    assert!(empty["messages"].as_array().unwrap().is_empty(), "{empty}");
    assert!(started.elapsed() >= Duration::from_millis(900), "returned early: {:?}", started.elapsed());

    // Without timeout_seconds the MCP poll waits the 5 s default (round 2, simplification proposal 10).
    let started = Instant::now();
    let (err, empty) = worker.tool("poll_messages", json!({}));
    assert!(!err, "{empty}");
    assert!(empty["messages"].as_array().unwrap().is_empty(), "{empty}");
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(4500) && waited < Duration::from_secs(10), "default poll wait: {waited:?}");

    // The finished workspace queue can be retired by the master.
    let (err, cleared) = master.tool(
        "clear_tasks",
        json!({ "workspace": ws, "expected_count": 1, "reason": "e2e cleanup", "request_id": "clear-mcp" }),
    );
    assert!(!err, "{cleared}");
    assert_eq!(cleared["count"], 1);

    // Both stdio servers stop cleanly when their client hangs up.
    assert!(master.close().is_some_and(|s| s.success()));
    assert!(worker.close().is_some_and(|s| s.success()));
}

/// Profiles for an in-process service, written like `ara-lite join` would.
fn profile_for(svc: &Service, dir: &Path, role: &str, name: &str) -> std::path::PathBuf {
    let c: Connection = svc.join(role, name);
    let path = dir.join(format!("{name}.json"));
    save_private(&path, &c, true).unwrap();
    path
}

/// Round 2 (simplification proposal 3): claim may omit version and generation over the CLI
/// and over MCP; progress still needs both.
#[test]
fn claim_without_version_or_generation_over_cli_and_mcp() {
    let svc = Service::start();
    let dir = tempfile::tempdir().unwrap();
    let master = profile_for(&svc, dir.path(), "master", "claim-master");
    let worker = profile_for(&svc, dir.path(), "worker", "claim-worker");
    let cli_ws = git_fixture();
    let mcp_ws = git_fixture();

    let task =
        call(&master, &["create", "--title", "cli", "--workspace", cli_ws.path().to_str().unwrap(), "--body", "w"]);
    let id = task["id"].as_str().unwrap().to_string();
    let running = call(&worker, &["claim", "--task-id", &id]);
    assert_eq!((running["state"].as_str(), running["generation"].as_i64()), (Some("running"), Some(1)), "{running}");
    let refused = fails(&worker, &["progress", "--task-id", &id, "--percent", "10", "--description", "no numbers"]);
    assert!(refused.contains("version conflict"), "{refused}");

    let task =
        call(&master, &["create", "--title", "mcp", "--workspace", mcp_ws.path().to_str().unwrap(), "--body", "w"]);
    let id = task["id"].as_str().unwrap().to_string();
    let mut mcp = McpProcess::start(&worker);
    let (err, claimed) = mcp.tool("claim_task", json!({ "task_id": id }));
    assert!(!err, "{claimed}");
    assert_eq!((claimed["state"].as_str(), claimed["generation"].as_i64()), (Some("running"), Some(1)), "{claimed}");
    let (err, again) = mcp.tool("claim_task", json!({ "task_id": id }));
    assert!(err && again.to_string().contains("task is not queued"), "{again}");
    assert!(mcp.close().is_some_and(|s| s.success()));
}

#[test]
fn lease_expiry_makes_a_running_task_unconfirmed_and_only_confirm_stop_requeues_it() {
    let svc = Service::start();
    let dir = tempfile::tempdir().unwrap();
    let clock: Arc<Mutex<Time>> = Arc::new(Mutex::new(Utc::now()));
    let shared = clock.clone();
    svc.store.set_clock(move || *shared.lock().unwrap());
    let master = profile_for(&svc, dir.path(), "master", "lease-master");
    let worker = profile_for(&svc, dir.path(), "worker", "lease-worker");
    let other = profile_for(&svc, dir.path(), "worker", "lease-other");
    let workspace = git_fixture();
    let ws = workspace.path().to_str().unwrap();

    let task = call(&master, &["create", "--title", "lease", "--workspace", ws, "--body", "work"]);
    let id = task["id"].as_str().unwrap().to_string();
    let running = call(&worker, &["claim", "--task-id", &id, "--version", &num(&task, "version"), "--generation", "0"]);
    assert_eq!(running["state"], "running");

    // The lease runs out: the task becomes unconfirmed, the worker keeps ownership.
    *clock.lock().unwrap() += ChronoDuration::seconds(121);
    svc.store.tick().unwrap();
    let tasks = call(&master, &["tasks"]);
    let unconfirmed = &tasks[0];
    assert_eq!(unconfirmed["state"], "unconfirmed", "{tasks}");
    assert_eq!(unconfirmed["worker_id"], running["worker_id"]);

    // Nobody else can take it over, and the owner cannot submit until it renews the claim.
    let taken = fails(
        &other,
        &[
            "claim",
            "--task-id",
            &id,
            "--version",
            &num(unconfirmed, "version"),
            "--generation",
            &num(unconfirmed, "generation"),
        ],
    );
    assert!(taken.contains("task is not queued"), "{taken}");
    let early = fails(
        &worker,
        &[
            "submit",
            "--task-id",
            &id,
            "--version",
            &num(unconfirmed, "version"),
            "--generation",
            &num(unconfirmed, "generation"),
            "--body",
            "too early",
        ],
    );
    assert!(early.contains("renew an unconfirmed claim"), "{early}");

    // The owner's heartbeat restores it.
    let restored = call(
        &worker,
        &[
            "heartbeat",
            "--task-id",
            &id,
            "--version",
            &num(unconfirmed, "version"),
            "--generation",
            &num(unconfirmed, "generation"),
        ],
    );
    assert_eq!(restored["state"], "running");

    // Expire again. Only the master's confirm-stop with evidence requeues, and it fences the old owner.
    *clock.lock().unwrap() += ChronoDuration::seconds(121);
    svc.store.tick().unwrap();
    let lost = call(&master, &["tasks"]);
    let lost = &lost[0];
    assert_eq!(lost["state"], "unconfirmed");
    let no_evidence = fails(
        &master,
        &[
            "confirm-stop",
            "--task-id",
            &id,
            "--version",
            &num(lost, "version"),
            "--generation",
            &num(lost, "generation"),
        ],
    );
    assert!(no_evidence.contains("evidence"), "{no_evidence}");
    let requeued = call(
        &master,
        &[
            "confirm-stop",
            "--task-id",
            &id,
            "--version",
            &num(lost, "version"),
            "--generation",
            &num(lost, "generation"),
            "--evidence",
            "worker process was killed",
        ],
    );
    assert_eq!((requeued["state"].as_str(), requeued["worker_id"].as_str()), (Some("queued"), Some("")));
    assert_eq!(requeued["generation"].as_i64().unwrap(), lost["generation"].as_i64().unwrap() + 1);
    let fenced = fails(
        &worker,
        &["heartbeat", "--task-id", &id, "--version", &num(lost, "version"), "--generation", &num(lost, "generation")],
    );
    assert!(fenced.contains("request rejected (409)"), "the old execution kept write access: {fenced}");
    let again = call(
        &other,
        &[
            "claim",
            "--task-id",
            &id,
            "--version",
            &num(&requeued, "version"),
            "--generation",
            &num(&requeued, "generation"),
        ],
    );
    assert_eq!(again["state"], "running");
    assert_ne!(again["worker_id"], running["worker_id"]);
}

#[test]
fn a_service_restart_turns_running_work_into_unconfirmed_and_keeps_it_recoverable() {
    let mut p = ServeProcess::start_in(tempfile::tempdir().unwrap(), &free_loopback_addr());
    let master = p.join("master", "restart-master");
    let worker = p.join("worker", "restart-worker");
    let workspace = git_fixture();
    let ws = workspace.path().to_str().unwrap();
    let task = call(&master, &["create", "--title", "restart", "--workspace", ws, "--body", "work"]);
    let id = task["id"].as_str().unwrap().to_string();
    let running = call(&worker, &["claim", "--task-id", &id, "--version", &num(&task, "version"), "--generation", "0"]);
    assert_eq!(running["state"], "running");

    // The service dies without any shutdown code and comes back on the same address.
    p.restart();
    let tasks = call(&master, &["tasks"]);
    assert_eq!(tasks[0]["state"], "unconfirmed", "a running claim must not survive a restart as running: {tasks}");
    assert_eq!(tasks[0]["worker_id"], running["worker_id"]);
    // Existing profiles still work: identities and credentials are durable.
    let restored = call(
        &worker,
        &[
            "heartbeat",
            "--task-id",
            &id,
            "--version",
            &num(&tasks[0], "version"),
            "--generation",
            &num(&tasks[0], "generation"),
        ],
    );
    assert_eq!(restored["state"], "running");
}
