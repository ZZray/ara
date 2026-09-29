//! Tests that run the real `ara-lite serve` process: startup output, join files, the
//! single-instance lock (port of `TestCoreCrashReleasesOSLock`) and abrupt-kill durability.

mod common;

use ara_lite::profile::read_connection;
use ara_lite::store::Store;
use common::{BIN, ServeProcess, cli, cli_ok, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn plain_status_prints_join_commands_and_never_a_credential() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let mut child = KillOnDrop(
        Command::new(BIN)
            .args(["serve", "--addr", "127.0.0.1:0", "--plain", "--data-dir"])
            .arg(&data)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut text = String::new();
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            text.push_str(&line);
            text.push('\n');
            if line.contains("Ctrl+C stops the service") {
                break;
            }
        }
        let _ = tx.send(text);
    });
    let text = rx.recv_timeout(Duration::from_secs(20)).expect("serve did not print its status block");
    assert!(text.starts_with("ara-lite READY\nhttp://127.0.0.1:"), "{text}");
    for needle in ["master join:", "worker join:", "--role master", "--role worker", "--join-file", "--profile"] {
        assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
    }
    // The startup text is safe to paste into a chat: it names files, not the secret inside them.
    let master = read_connection(&data.join("master-join.json")).unwrap();
    let worker = read_connection(&data.join("worker-join.json")).unwrap();
    assert!(!text.contains(&master.token) && !text.contains(&worker.token), "credential printed:\n{text}");
    assert_ne!(master.token, worker.token);
}

#[test]
fn json_readiness_line_matches_the_join_files() {
    let mut p = ServeProcess::start();
    assert!(p.is_running());
    for (path, role) in [(&p.master_join, "master"), (&p.worker_join, "worker")] {
        let c = read_connection(path).unwrap();
        assert_eq!((c.url.as_str(), c.role.as_str(), c.id.as_str()), (p.url.as_str(), role, ""));
    }
    let addr = p.url.strip_prefix("http://").unwrap();
    let mut stream = TcpStream::connect(addr).unwrap();
    stream.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    assert!(reply.starts_with("HTTP/1.1 200") && reply.ends_with("{\"status\":\"ready\"}"), "{reply}");
}

#[test]
fn listen_address_and_flag_errors_are_reported() {
    for (args, needle) in [
        (vec!["serve", "--addr", "0.0.0.0:0"], "loopback"),
        (vec!["serve", "--addr", "localhost:7342"], "loopback"),
        (vec!["serve", "--plain", "--json"], "choose --plain or --json"),
        (vec!["serve", "--bogus"], "--bogus"),
    ] {
        let out = cli(&args);
        assert!(!out.status.success(), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
    }
}

#[test]
fn crash_releases_the_data_directory_lock() {
    let mut p = ServeProcess::start();
    let first = Store::open(&p.data_dir);
    assert!(first.is_err(), "a second process obtained the data directory lock");
    p.kill();
    let store = Store::open(&p.data_dir).expect("a crashed process left a stale lock");
    store.close();
}

#[test]
fn state_survives_an_abrupt_kill() {
    let mut p = ServeProcess::start();
    let master = p.join("master", "durable-master");
    let _worker = p.join("worker", "durable-worker");
    let workspace = tempfile::tempdir().unwrap();
    let created = json(&cli_ok(&[
        "create",
        "--profile",
        master.to_str().unwrap(),
        "--title",
        "durable",
        "--workspace",
        workspace.path().to_str().unwrap(),
        "--mode",
        "read",
        "--json",
    ]));
    assert_eq!(created["state"], "queued");
    p.kill();
    let store = Store::open(&p.data_dir).unwrap();
    let state = store.snapshot().unwrap();
    assert_eq!(state.clients.len(), 2);
    assert_eq!(state.tasks.len(), 1);
    assert!(state.tasks.values().any(|t| t.title == "durable" && t.state == "queued"));
    store.close();
}
