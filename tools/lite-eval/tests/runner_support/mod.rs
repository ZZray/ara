//! Controlled upstream and throw-away repositories for the runner tests.
//!
//! [`Provider`] is a local HTTP server: `GET /v1/models` answers the Core
//! catalog lookup, a non-streaming `chat/completions` is the probe, and a
//! streaming one is a model call answered by [`model_reply`]. [`TestRepo`]
//! is a git repository with its own `tools/lite-eval/run-policy.json`,
//! `task-set.json` and holdout log, so `runner::run` reads the same fixed
//! paths as in production.
#![allow(dead_code)]

use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::Command,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

use serde_json::{json, Value};

pub const MODEL: &str = "fixture-model";
pub const IDS: [&str; 6] = ["S1", "S2", "S3", "R1", "H2", "SH1"];

/// How the probe request is answered.
#[derive(Clone, Debug, PartialEq)]
pub enum ProbeMode {
    /// HTTP status and raw body.
    Reply { status: u16, body: String, delay_ms: u64 },
    /// Read the request, then close without answering.
    Drop,
}

impl ProbeMode {
    pub fn ok(model: &str) -> Self {
        let body = json!({"id": "probe", "object": "chat.completion", "model": model,
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "OK"}, "finish_reason": "length"}]});
        ProbeMode::Reply { status: 200, body: body.to_string(), delay_ms: 0 }
    }
}

/// One model call: tool calls from `script` in order (one per turn), then
/// a final text. The turn is the number of `tool` messages in the request.
pub type Script = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

pub fn text(value: &str) -> Value {
    json!({"delta": {"content": value}, "finish": "stop"})
}

pub fn call(id: &str, name: &str, args: Value) -> Value {
    json!({"delta": {"tool_calls": [{"index": 0, "id": id, "type": "function",
        "function": {"name": name, "arguments": args.to_string()}}]}, "finish": "tool_calls"})
}

/// Turn index of a model request: how many tool results it carries.
pub fn turn(body: &Value) -> usize {
    body["messages"].as_array().map_or(0, |messages| messages.iter().filter(|m| m["role"] == "tool").count())
}

/// Default script: write `answer.txt` = `correct`, then finish.
pub fn model_reply(body: &Value) -> Value {
    match turn(body) {
        0 => call("w1", "ws_write_file", json!({"path": "answer.txt", "content": "correct"})),
        _ => text("Done: answer.txt written."),
    }
}

pub struct Provider {
    pub endpoint: String,
    pub probe_mode: Arc<Mutex<ProbeMode>>,
    pub probe_requests: Arc<AtomicUsize>,
    pub model_requests: Arc<AtomicUsize>,
    /// Every `Authorization` header value seen.
    pub authorizations: Arc<Mutex<Vec<String>>>,
    pub probe_bodies: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let headers = String::from_utf8_lossy(&bytes[..header_end]).into_owned();
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
        })
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
    }
    Some((headers, bytes[header_end..].to_vec()))
}

impl Provider {
    pub fn new(script: Script) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let probe_mode = Arc::new(Mutex::new(ProbeMode::ok(MODEL)));
        let probe_requests = Arc::new(AtomicUsize::new(0));
        let model_requests = Arc::new(AtomicUsize::new(0));
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let probe_bodies = Arc::new(Mutex::new(Vec::new()));
        let (done, mode, probes, models, auth, bodies) = (
            stop.clone(),
            probe_mode.clone(),
            probe_requests.clone(),
            model_requests.clone(),
            authorizations.clone(),
            probe_bodies.clone(),
        );
        let worker = thread::spawn(move || {
            for stream in listener.incoming() {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let (mode, probes, models, auth, bodies, script) =
                    (mode.clone(), probes.clone(), models.clone(), auth.clone(), bodies.clone(), script.clone());
                thread::spawn(move || {
                    let Some((headers, body)) = read_request(&mut stream) else { return };
                    if let Some(value) = headers.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("authorization").then(|| value.trim().to_owned())
                    }) {
                        auth.lock().unwrap().push(value);
                    }
                    if headers.starts_with("GET /v1/models ") {
                        let body = json!({"data": [{"id": MODEL, "protocols": ["openai-completions"],
                            "context_window": 1_000_000, "max_tokens": 4096,
                            "omp": {"supportsTools": true, "input": ["text"], "reasoning": false}}]})
                        .to_string();
                        let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        return;
                    }
                    let Ok(request) = serde_json::from_slice::<Value>(&body) else { return };
                    if request["stream"] != json!(true) {
                        probes.fetch_add(1, Ordering::SeqCst);
                        bodies.lock().unwrap().push(request);
                        let mode = mode.lock().unwrap().clone();
                        if let ProbeMode::Reply { status, body, delay_ms } = mode {
                            thread::sleep(Duration::from_millis(delay_ms));
                            let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        }
                        return;
                    }
                    models.fetch_add(1, Ordering::SeqCst);
                    let response = script(&request);
                    // Optional controls: `delay_ms` before answering, an
                    // `http_status` error reply, and `usage` on the last chunk.
                    if let Some(delay) = response["delay_ms"].as_u64() {
                        thread::sleep(Duration::from_millis(delay));
                    }
                    if let Some(status) = response["http_status"].as_u64() {
                        let body = json!({"error": {"message": "controlled provider error", "type": "server_error"}})
                            .to_string();
                        let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                        return;
                    }
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                    );
                    for (delta, finish, usage) in [
                        (json!({"role": "assistant"}), Value::Null, Value::Null),
                        (response["delta"].clone(), Value::Null, Value::Null),
                        (json!({}), response["finish"].clone(), response["usage"].clone()),
                    ] {
                        let mut packet = json!({"id": "controlled", "object": "chat.completion.chunk", "model": MODEL,
                            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
                        if !usage.is_null() {
                            packet["usage"] = usage;
                        }
                        let _ = write!(stream, "data: {packet}\n\n");
                    }
                    let _ = stream.write_all(b"data: [DONE]\n\n");
                });
            }
        });
        Self {
            endpoint: format!("http://{addr}/v1"),
            probe_mode,
            probe_requests,
            model_requests,
            authorizations,
            probe_bodies,
            stop,
            worker: Some(worker),
        }
    }

    pub fn standard() -> Self {
        Self::new(Arc::new(model_reply))
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://").trim_end_matches("/v1"));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Python Oracle of the test tasks: PASS when `answer.txt` equals
/// `expect.txt` in the copied workspace.
pub const ORACLE: &str = r#"
import argparse, json, pathlib
p = argparse.ArgumentParser()
p.add_argument('--artifacts', type=pathlib.Path, required=True)
p.add_argument('--final-text', type=pathlib.Path, required=True)
p.add_argument('--out', type=pathlib.Path, required=True)
a = p.parse_args()
answer = a.artifacts / 'answer.txt'
expect = (a.artifacts / 'expect.txt').read_text()
ok = answer.is_file() and answer.read_text() == expect
a.out.write_text(json.dumps({'schema': 'lite-eval-oracle-v1', 'status': 'PASS' if ok else 'FAIL',
                             'public': None, 'hidden': {'passed': int(ok), 'total': 1}, 'reason': None}))
"#;

pub fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=lite-eval-test", "-c", "user.email=test@example.invalid", "-c", "core.autocrlf=false"])
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

pub struct TestRepo {
    pub dir: tempfile::TempDir,
    pub root: PathBuf,
    pub fixtures: PathBuf,
    pub policy: Value,
    pub set: Value,
}

pub fn policy(endpoint: &str, credential_env: &str) -> Value {
    let budget =
        json!({"max_runs": 2, "max_calls": 8, "deadline_seconds": 120, "run_max_calls": 4, "run_max_seconds": 60});
    json!({
        "version": 1, "policy_id": "test-policy",
        "model": {"id": MODEL, "protocol": "openai-completions",
                  "route": {"endpoint": endpoint, "binding": "test", "decision": "test"},
                  "resolution": "catalog", "credential_env": credential_env},
        "probe": {"timeout_ms": 5000},
        "budgets": IDS.iter().map(|id| (id.to_string(), budget.clone())).collect::<serde_json::Map<_, _>>(),
        "generation": {"max_output": 1024, "generation_policy": "provider_default", "length_recovery": "stop"},
        "price": {"currency": null, "input_per_million": null, "output_per_million": null},
        "oracle_timeout_seconds": 60, "consumer_task": "S1"
    })
}

impl TestRepo {
    /// Six tasks; `expect` gives each task's expected answer (the default
    /// script writes `correct`, so `correct` passes and anything else fails).
    pub fn new(endpoint: &str, credential_env: &str, expect: &dyn Fn(&str) -> String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let base = lite_eval::workspace::canonical(dir.path()).unwrap();
        let root = base.join("repo");
        let fixtures = base.join("fixtures");
        std::fs::create_dir_all(root.join("tools/lite-eval")).unwrap();
        let oracle = base.join("oracle.py");
        std::fs::write(&oracle, ORACLE).unwrap();
        let mut tasks = Vec::new();
        let mut log = String::new();
        for (index, id) in IDS.iter().enumerate() {
            let fixture = fixtures.join(id);
            std::fs::create_dir_all(&fixture).unwrap();
            std::fs::write(fixture.join("README.md"), "Write the word correct into answer.txt.\n").unwrap();
            std::fs::write(fixture.join("expect.txt"), expect(id)).unwrap();
            let sha = lite_eval::digest::tree_digest(&fixture).unwrap().sha256;
            let (public, hidden) = match *id {
                "S3" => (Some(10), 48),
                "H2" => (None, 128),
                _ => (None, 1),
            };
            let mut oracle_spec = json!({"type": "test_python", "hidden_cases": hidden,
                "command": ["python", oracle.to_string_lossy()]});
            if let Some(public) = public {
                oracle_spec["public_cases"] = json!(public);
            }
            tasks.push(json!({
                "task_id": id, "kind": "Coding", "fixture_path": fixture, "fixture_sha256": sha,
                "oracle": oracle_spec,
                "holdout": {"status": "not new holdout", "first_adaptive_use": "2026-10-23", "changes": ["test"]},
                "profile": {"mode": "execute", "tools": ["ws_read_file", "ws_list_dir", "ws_write_file"]}
            }));
            log.push_str(&format!(
                "{{\"seq\":{},\"task_id\":\"{id}\",\"event\":\"marked_not_new\",\"date\":\"2026-10-23\",\"changes\":[\"test\"]}}\n",
                index + 1
            ));
        }
        let set = json!({"version": 1, "digest_rule": "lite-eval-tree-v1", "tasks": tasks});
        let policy = policy(endpoint, credential_env);
        let repo = Self { dir, root, fixtures, policy, set };
        repo.write_policy();
        repo.write_set();
        std::fs::write(repo.root.join("tools/lite-eval/holdout-log.jsonl"), log).unwrap();
        git(&repo.root, &["init", "-q"]);
        git(&repo.root, &["add", "."]);
        git(&repo.root, &["commit", "-q", "-m", "fixture"]);
        repo
    }

    pub fn write_policy(&self) {
        std::fs::write(
            self.root.join("tools/lite-eval/run-policy.json"),
            serde_json::to_vec_pretty(&self.policy).unwrap(),
        )
        .unwrap();
    }

    pub fn write_set(&self) {
        std::fs::write(self.root.join("tools/lite-eval/task-set.json"), serde_json::to_vec_pretty(&self.set).unwrap())
            .unwrap();
    }

    pub fn evidence(&self, name: &str) -> PathBuf {
        let path = self.root.parent().unwrap().join("evidence").join(name);
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}

/// Every file under `dir`, recursively.
pub fn files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(next) = pending.pop() {
        for item in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = item.path();
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => pending.push(path),
                Ok(meta) if meta.is_file() => out.push(path),
                _ => {}
            }
        }
    }
    out
}
