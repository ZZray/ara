//! Helpers shared by the integration tests. Each test file is its own crate, so anything
//! one file does not use would warn without the `allow` below.
#![allow(dead_code)]

use ara_lite::client::call;
use ara_lite::server::Server;
use ara_lite::store::Store;
use ara_lite::types::{Client, Connection, Request};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The compiled `ara-lite` binary under test.
pub const BIN: &str = env!("CARGO_BIN_EXE_ara-lite");

/// An in-process service on an ephemeral loopback port (real store, real HTTP server).
pub struct Service {
    // Field order is drop order: stop the listener first, then release the store, then delete the directory.
    pub server: Option<Server>,
    pub store: Arc<Store>,
    pub url: String,
    pub dir: TempDir,
}

impl Service {
    pub fn start() -> Service {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = Server::start(listener, store.clone()).unwrap();
        Service { server: Some(server), store, url, dir }
    }

    /// Join as `role` with the service's own join key; the request ID is the name.
    pub fn join(&self, role: &str, name: &str) -> Connection {
        let (master_key, worker_key) = self.store.join_keys();
        let key = if role == "master" { master_key } else { worker_key };
        join_with(&self.url, &key, role, name)
    }

    /// A join file for `role`, as `ara-lite serve` writes it.
    pub fn join_file(&self, role: &str, into: &Path) -> PathBuf {
        let (master_key, worker_key) = self.store.join_keys();
        let key = if role == "master" { master_key } else { worker_key };
        let path = into.join(format!("{role}-join.json"));
        let connection = Connection { url: self.url.clone(), token: key, id: String::new(), role: role.to_string() };
        ara_lite::profile::save_private(&path, &connection, true).unwrap();
        path
    }
}

pub fn join_with(url: &str, key: &str, role: &str, name: &str) -> Connection {
    let bootstrap = Connection { url: url.to_string(), token: key.to_string(), id: String::new(), role: role.into() };
    let request = Request {
        op: "join".into(),
        role: role.into(),
        name: name.into(),
        request_id: name.into(),
        ..Request::default()
    };
    let raw = call(&bootstrap, &request).unwrap();
    let client: Client = serde_json::from_str(raw.get()).unwrap();
    Connection { url: url.to_string(), token: client.token, id: client.id, role: client.role }
}

/// Run the CLI binary and return its full output.
pub fn cli(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().unwrap()
}

/// Run the CLI expecting success; returns trimmed stdout (which must be valid JSON when `--json` is used).
pub fn cli_ok(args: &[&str]) -> String {
    let out = cli(args);
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "ara-lite {args:?} failed: stdout={stdout} stderr={stderr}");
    stdout
}

/// Run the CLI expecting failure; returns stderr.
pub fn cli_err(args: &[&str]) -> String {
    let out = cli(args);
    assert!(
        !out.status.success(),
        "ara-lite {args:?} unexpectedly succeeded: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// Parse CLI JSON output into a value.
pub fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("invalid JSON ({e}): {text}"))
}

/// A Git repository with one empty commit on `dev/working`.
pub fn git_fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "--initial-branch=dev/working"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let out = Command::new("git").arg("-C").arg(dir.path()).args(&args).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    dir
}

/// A real `ara-lite serve` process on an ephemeral loopback port with a private data directory.
/// The child is killed on drop.
pub struct ServeProcess {
    child: Child,
    pub url: String,
    pub data_dir: PathBuf,
    pub master_join: PathBuf,
    pub worker_join: PathBuf,
    // Kept last so the directory outlives the killed child.
    pub root: TempDir,
}

/// Start `ara-lite serve --json` and wait for its readiness line.
fn spawn_serve(addr: &str, data_dir: &Path) -> (Child, serde_json::Value) {
    let mut child = Command::new(BIN)
        .args(["serve", "--addr", addr, "--json", "--data-dir"])
        .arg(data_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    // The readiness line arrives on a helper thread so a wedged child cannot hang the test.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(line) if !line.trim().is_empty() => line,
        other => {
            let _ = child.kill();
            let _ = child.wait();
            let mut stderr = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = std::io::Read::read_to_string(&mut e, &mut stderr);
            }
            panic!("serve did not report readiness: {other:?} stderr={stderr}");
        }
    };
    let ready = json(line.trim());
    assert_eq!(ready["status"], "ready");
    (child, ready)
}

/// A loopback port that is free right now (another process could take it; fine for a local test).
pub fn free_loopback_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().to_string()
}

impl ServeProcess {
    /// Start on an ephemeral port with a fresh data directory.
    pub fn start() -> ServeProcess {
        ServeProcess::start_in(tempfile::tempdir().unwrap(), "127.0.0.1:0")
    }

    /// Start with `root/data` as the data directory, listening on `addr`.
    pub fn start_in(root: TempDir, addr: &str) -> ServeProcess {
        let data_dir = root.path().join("data");
        let (child, ready) = spawn_serve(addr, &data_dir);
        ServeProcess {
            child,
            url: ready["url"].as_str().unwrap().to_string(),
            data_dir,
            master_join: PathBuf::from(ready["master_join_file"].as_str().unwrap()),
            worker_join: PathBuf::from(ready["worker_join_file"].as_str().unwrap()),
            root,
        }
    }

    /// Kill the process abruptly, then start it again on the same data directory and address,
    /// so existing profiles stay valid.
    pub fn restart(&mut self) {
        self.kill();
        let addr = self.url.strip_prefix("http://").unwrap().to_string();
        let (child, ready) = spawn_serve(&addr, &self.data_dir);
        assert_eq!(ready["url"].as_str().unwrap(), self.url);
        self.child = child;
    }

    pub fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Kill the process abruptly (no shutdown code runs) and wait for it to be gone.
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Join through the CLI and return the profile path.
    pub fn join(&self, role: &str, name: &str) -> PathBuf {
        let join_file = if role == "master" { &self.master_join } else { &self.worker_join };
        let profile = self.root.path().join(format!("{name}.json"));
        cli_ok(&[
            "join",
            "--join-file",
            join_file.to_str().unwrap(),
            "--profile",
            profile.to_str().unwrap(),
            "--role",
            role,
            "--name",
            name,
            "--json",
        ]);
        profile
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A running `ara-lite mcp` process speaking newline-delimited JSON-RPC over stdio.
pub struct McpProcess {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: std::sync::mpsc::Receiver<String>,
    next_id: i64,
}

impl McpProcess {
    pub fn start(profile: &Path) -> McpProcess {
        let mut child = Command::new(BIN)
            .arg("mcp")
            .arg("--profile")
            .arg(profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout: ChildStdout = child.stdout.take().unwrap();
        let (tx, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let mut mcp = McpProcess { child, stdin, lines, next_id: 0 };
        let init = mcp.request(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "ara-lite-test", "version": "1" }
            }),
        );
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
        mcp.notify("notifications/initialized", serde_json::json!({}));
        mcp
    }

    fn send(&mut self, value: &serde_json::Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{value}").unwrap();
        stdin.flush().unwrap();
    }

    /// Write one raw line (for malformed-input tests).
    pub fn send_raw(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    pub fn notify(&mut self, method: &str, params: serde_json::Value) {
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }));
    }

    /// Send a request without waiting; returns its ID.
    pub fn send_request(&mut self, method: &str, params: serde_json::Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        id
    }

    /// Wait up to `timeout` for the next output line.
    pub fn next_line(&self, timeout: Duration) -> Option<String> {
        self.lines.recv_timeout(timeout).ok()
    }

    /// Wait for the response with `id`; other lines are not expected in these tests.
    pub fn wait_for(&self, id: i64, timeout: Duration) -> serde_json::Value {
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.next_line(left).unwrap_or_else(|| panic!("no MCP response for id {id}"));
            let value = json(&line);
            if value["id"] == id {
                return value;
            }
        }
    }

    pub fn request(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let id = self.send_request(method, params);
        self.wait_for(id, Duration::from_secs(20))
    }

    /// Call a tool and return `(is_error, structuredContent-or-text)`.
    pub fn tool(&mut self, name: &str, arguments: serde_json::Value) -> (bool, serde_json::Value) {
        let reply = self.request("tools/call", serde_json::json!({ "name": name, "arguments": arguments }));
        let result = &reply["result"];
        assert!(result.is_object(), "tools/call {name} returned no result: {reply}");
        let is_error = result["isError"].as_bool().unwrap_or(false);
        let payload = if is_error { result["content"][0]["text"].clone() } else { result["structuredContent"].clone() };
        (is_error, payload)
    }

    /// Close stdin (like a client hanging up) and wait for the process to exit.
    pub fn close(mut self) -> Option<std::process::ExitStatus> {
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
                _ => return None,
            }
        }
    }
}

impl Drop for McpProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
