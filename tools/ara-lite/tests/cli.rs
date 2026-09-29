//! CLI process tests against an in-process service (ports of `main_test.go`
//! `TestCLIJoinSubmitReviewAndSnapshotGate`, the join-help/profile tests,
//! `TestGitSnapshotIncludesUntrackedIndexAndDeletion` and `TestClientClearTasksCommand`).

mod common;

use ara_lite::profile::{read_connection, save_private};
use ara_lite::serve::join_commands;
use ara_lite::snapshot::git_snapshot;
use ara_lite::store::Store;
use ara_lite::types::{Connection, Request};
use common::{Service, cli, cli_err, cli_ok, git_fixture, json};
use std::path::Path;
use std::process::Command;

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn cli_join_submit_review_and_snapshot_gate() {
    let svc = Service::start();
    let dir = tempfile::tempdir().unwrap();
    let mut profiles = std::collections::BTreeMap::new();
    for role in ["master", "worker"] {
        let join_file = svc.join_file(role, dir.path());
        let profile = dir.path().join(format!("{role}.json"));
        let out = json(&cli_ok(&[
            "join",
            "--join-file",
            s(&join_file),
            "--profile",
            s(&profile),
            "--role",
            role,
            "--name",
            role,
            "--json",
        ]));
        assert_eq!(out["role"], role);
        // The saved profile carries the issued identity, not the shared join key.
        let saved = read_connection(&profile).unwrap();
        assert_eq!(saved.id, out["id"].as_str().unwrap());
        profiles.insert(role, profile);
    }
    // Joining again into an existing profile is refused without touching it.
    let before = std::fs::read(&profiles["worker"]).unwrap();
    let stderr = cli_err(&[
        "join",
        "--join-file",
        s(&dir.path().join("worker-join.json")),
        "--profile",
        s(&profiles["worker"]),
        "--role",
        "worker",
        "--name",
        "again",
    ]);
    assert!(stderr.contains("profile already exists"), "{stderr}");
    assert_eq!(std::fs::read(&profiles["worker"]).unwrap(), before);

    // Identity issuance succeeds but an unusable parent blocks the profile; the same request
    // ID recovers that identity once the disk problem is fixed.
    let blocked = dir.path().join("blocked-parent");
    std::fs::write(&blocked, "fixture blocker").unwrap();
    let retry_profile = blocked.join("worker.json");
    let args: Vec<String> = [
        "join",
        "--join-file",
        s(&dir.path().join("worker-join.json")),
        "--profile",
        s(&retry_profile),
        "--role",
        "worker",
        "--name",
        "retry-worker",
        "--request-id",
        "join-retry",
        "--json",
    ]
    .iter()
    .map(|a| a.to_string())
    .collect();
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let stderr = cli_err(&refs);
    assert!(stderr.contains("identity issued but profile save failed"), "{stderr}");
    assert!(stderr.contains("join-retry"), "the retry hint must name the request ID: {stderr}");
    assert_eq!(svc.store.snapshot().unwrap().clients.len(), 3, "identity was not issued before the profile failure");
    std::fs::remove_file(&blocked).unwrap();
    cli_ok(&refs);
    assert_eq!(svc.store.snapshot().unwrap().clients.len(), 3, "retry issued a duplicate identity");
    assert!(read_connection(&retry_profile).is_ok());

    let workspace = git_fixture();
    let (master, worker) = (s(&profiles["master"]).to_string(), s(&profiles["worker"]).to_string());
    let task = json(&cli_ok(&[
        "create",
        "--profile",
        &master,
        "--title",
        "CLI fixture",
        "--workspace",
        s(workspace.path()),
        "--json",
    ]));
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["state"], "queued");
    let claimed = json(&cli_ok(&["claim", "--profile", &worker, "--task-id", &id, "--version", "1", "--json"]));
    assert_eq!(claimed["state"], "running");
    let submitted = json(&cli_ok(&[
        "submit",
        "--profile",
        &worker,
        "--task-id",
        &id,
        "--version",
        "2",
        "--generation",
        "1",
        "--body",
        "ready for review",
        "--json",
    ]));
    assert_eq!(submitted["state"], "awaiting_review");
    let stale = submitted["snapshot"].as_str().unwrap().to_string();
    assert!(stale.contains(":dev/working:"), "{stale}");

    // A change after the snapshot blocks the review.
    let extra = workspace.path().join("unexpected.txt");
    std::fs::write(&extra, "changed").unwrap();
    let review = |snapshot: &str| {
        cli(&[
            "review",
            "--profile",
            &master,
            "--task-id",
            &id,
            "--version",
            "3",
            "--generation",
            "1",
            "--snapshot",
            snapshot,
            "--decision",
            "pass",
            "--body",
            "reviewed",
            "--json",
        ])
    };
    let refused = review(&stale);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("workspace changed"), "{refused:?}");
    // A missing --snapshot is refused as well.
    let refused = review("");
    assert!(!refused.status.success());
    std::fs::remove_file(&extra).unwrap();
    let passed = review(&stale);
    assert!(passed.status.success(), "{}", String::from_utf8_lossy(&passed.stderr));
    let passed = json(String::from_utf8_lossy(&passed.stdout).trim());
    assert_eq!(passed["state"], "passed", "{passed}");

    let polled = cli_ok(&["poll", "--profile", &worker, "--after", "-1", "--timeout", "0s", "--json"]);
    assert!(polled.contains("\"event_kind\":\"pass\""), "missing review notification: {polled}");
    // Without --json the same result is indented for humans.
    let human = cli_ok(&["tasks", "--profile", &master]);
    assert!(human.contains("\n  "), "{human}");
    assert!(json(&human).is_array());
}

#[test]
fn cli_clear_tasks_command() {
    let svc = Service::start();
    let master = svc.join("master", "master");
    let worker = svc.join("worker", "worker");
    let workspace = tempfile::tempdir().unwrap();
    let create = Request {
        op: "create".into(),
        title: "queued".into(),
        workspace: workspace.path().to_string_lossy().into_owned(),
        mode: "read".into(),
        worker_id: worker.id.clone(),
        request_id: "create-1".into(),
        ..Request::default()
    };
    ara_lite::client::call(&master, &create).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("master.json");
    save_private(&profile, &Connection { role: "master".into(), ..master.clone() }, true).unwrap();
    let out = json(&cli_ok(&[
        "clear-tasks",
        "--profile",
        s(&profile),
        "--workspace",
        s(workspace.path()),
        "--expected-count",
        "1",
        "--reason",
        "replace queue",
        "--request-id",
        "cli-clear",
        "--json",
    ]));
    assert_eq!(out["count"], 1, "{out}");
    let tasks = json(&cli_ok(&["tasks", "--profile", s(&profile), "--json"]));
    assert_eq!(tasks.as_array().unwrap().len(), 0, "CLI did not clear the task view: {tasks}");
}

#[test]
fn cli_usage_errors_are_a_single_actionable_line() {
    let svc = Service::start();
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("w.json");
    save_private(&profile, &svc.join("worker", "w"), true).unwrap();
    for (args, needle) in [
        (vec!["status"], "--profile FILE is required"),
        (vec!["frobnicate"], "unknown command"),
        (vec!["poll", "--profile", s(&profile), "--timeout", "31s"], "between 0 and 30s"),
        (vec!["poll", "--profile", s(&profile), "--timeout", "soon"], "--timeout"),
        (vec!["join", "--profile", s(&dir.path().join("new.json"))], "join needs --join-file"),
        (vec!["status", "--profile", s(&dir.path().join("missing.json"))], "open "),
        (vec!["status", "--profile", s(&profile), "--bogus", "1"], "--bogus"),
        (vec!["send", "--profile", s(&profile), "--body-file", s(&dir.path().join("nope.txt"))], "read "),
        (vec!["snapshot"], "usage: ara-lite snapshot WORKSPACE"),
    ] {
        let stderr = cli_err(&args);
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
        assert_eq!(
            stderr.lines().filter(|l| l.starts_with("ara-lite:")).count(),
            1,
            "{args:?} should report one error line: {stderr}"
        );
    }
}

#[test]
fn cli_body_file_and_retry_ids() {
    let svc = Service::start();
    let dir = tempfile::tempdir().unwrap();
    let master = dir.path().join("m.json");
    save_private(&master, &svc.join("master", "m"), true).unwrap();
    let worker = svc.join("worker", "w");
    let body = dir.path().join("body.txt");
    std::fs::write(&body, "long instruction from a file\nwith two lines").unwrap();
    let out = cli(&[
        "send",
        "--profile",
        s(&master),
        "--worker-id",
        &worker.id,
        "--body-file",
        s(&body),
        "--request-id",
        "send-file",
        "--json",
    ]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Mutations echo the retry key on stderr; the same key with the same arguments is idempotent.
    assert!(String::from_utf8_lossy(&out.stderr).contains("request-id: send-file"));
    let first = json(String::from_utf8_lossy(&out.stdout).trim());
    assert_eq!(first["body"], "long instruction from a file\nwith two lines");
    let again = json(&cli_ok(&[
        "send",
        "--profile",
        s(&master),
        "--worker-id",
        &worker.id,
        "--body-file",
        s(&body),
        "--request-id",
        "send-file",
        "--json",
    ]));
    assert_eq!(first["seq"], again["seq"]);
    // The same key with different arguments is refused.
    let stderr = cli_err(&[
        "send",
        "--profile",
        s(&master),
        "--worker-id",
        &worker.id,
        "--body",
        "different",
        "--request-id",
        "send-file",
    ]);
    assert!(stderr.contains("request rejected (409)"), "{stderr}");
}

#[test]
fn help_lists_the_commands_and_both_prompts() {
    let out = cli_ok(&["help"]);
    for needle in ["Serve:", "mcp --profile FILE", "clear-tasks", "confirm-stop", "history"] {
        assert!(out.contains(needle), "help is missing {needle:?}");
    }
    // Both prompts are part of the help output, verbatim.
    assert!(out.contains(ara_lite::help::MASTER_PROMPT.trim_end()));
    assert!(out.contains(ara_lite::help::WORKER_PROMPT.trim_end()));
    assert_eq!(out, cli_ok(&["--help"]));
}

#[test]
fn git_snapshot_includes_untracked_index_and_deletion() {
    let dir = git_fixture();
    let first = git_snapshot(dir.path()).unwrap();
    let path = dir.path().join("中文.txt");
    std::fs::write(&path, "first").unwrap();
    let second = git_snapshot(dir.path()).unwrap();
    assert_ne!(first, second, "untracked file missing");
    let added = Command::new("git").arg("-C").arg(dir.path()).args(["add", "."]).output().unwrap();
    assert!(added.status.success());
    let third = git_snapshot(dir.path()).unwrap();
    assert_ne!(third, second, "index not represented");
    std::fs::remove_file(&path).unwrap();
    let fourth = git_snapshot(dir.path()).unwrap();
    assert_ne!(fourth, third, "deletion not represented");
    // Same state, same snapshot; and the CLI prints the same value.
    assert_eq!(fourth, git_snapshot(dir.path()).unwrap());
    assert_eq!(cli_ok(&["snapshot", s(dir.path())]), fourth);
    assert!(fourth.contains(":dev/working:"), "{fourth}");
}

#[test]
fn git_snapshot_rejects_non_repositories() {
    let dir = tempfile::tempdir().unwrap();
    assert!(git_snapshot(dir.path()).is_err());
}

#[test]
fn profile_and_join_help_do_not_expose_the_token() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.json");
    let good = Connection {
        url: "http://127.0.0.1:7342".into(),
        token: "secret".into(),
        id: String::new(),
        role: "worker".into(),
    };
    save_private(&path, &good, false).unwrap();
    assert_eq!(read_connection(&path).unwrap(), good);
    let help = join_commands(
        Path::new(r"C:\Program Files\ara-lite.exe"),
        Path::new(r"C:\Users\a'b\Data"),
        &Default::default(),
        true,
    );
    assert!(help.contains("--join-file") && !help.contains("secret"), "{help}");
    for address in ["https://example.com", "http://localhost:7342", "http://127.0.0.1:7342/steal"] {
        save_private(
            &path,
            &Connection { url: address.into(), token: "secret".into(), ..Connection::default() },
            false,
        )
        .unwrap();
        assert!(read_connection(&path).is_err(), "accepted {address}");
    }
    // A profile without a credential is refused too.
    save_private(&path, &Connection { url: good.url.clone(), token: String::new(), ..Connection::default() }, false)
        .unwrap();
    assert!(read_connection(&path).is_err());
}

#[test]
fn join_help_keeps_the_persisted_master_name_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let (master_key, _) = store.join_keys();
    let master_name = "Reviewer's master";
    let join = |request_id: &str| Request {
        op: "join".into(),
        role: "master".into(),
        name: master_name.into(),
        request_id: request_id.into(),
        ..Request::default()
    };
    store.execute(&master_key, &join("join-master")).unwrap();
    store.close();
    drop(store);
    std::fs::write(dir.path().join("master-1.json"), "existing profile").unwrap();
    let store = Store::open(dir.path()).unwrap();
    let help = join_commands(Path::new("ara-lite"), dir.path(), &store.snapshot().unwrap(), false);
    assert!(help.contains(r#"--name 'Reviewer'"'"'s master'"#), "join help lost the persisted master identity: {help}");
    assert!(help.contains("master-2.json") && !help.contains("--name 'master-2'"), "{help}");
    // The generated command really can rejoin.
    store.execute(&master_key, &join("rejoin-master")).expect("generated master name cannot rejoin");
    store.close();
}
