//! Real ara RPC children + controlled HTTP provider. Fixed OMP 596f2da:
//! host-tools.ts, host-uris.ts and rpc-mode.ts side-channel dispatch.
//! A shared case deadline bounds all waits, including shutdown and reopen.

use ara_testkit::chunks::{done, finish, text, tool_call};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ara");
const CASE_BOUND: Duration = Duration::from_secs(8);

struct Env {
    home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
    deadline: Instant,
}

impl Env {
    fn new() -> Self {
        let home = tempfile::Builder::new().prefix("ara-host-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-host-work-").tempdir().unwrap();
        std::fs::create_dir(work.path().join(".git")).unwrap();
        let sessions = home.path().join("sessions");
        Self { home, work, sessions, deadline: Instant::now() + CASE_BOUND }
    }

    fn command(&self, upstream: &FakeUpstream, args: &[&str]) -> Command {
        let mut command = Command::new(BIN);
        for key in [
            "OPENROUTER_API_KEY",
            "ANTHROPIC_API_KEY",
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ARA_MODEL",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "OPENROUTER_BASE_URL",
            "ARA_TEST_MODEL_ID",
            "CLAUDE_CONFIG_DIR",
            "COPILOT_HOME",
            "COPILOT_CUSTOM_INSTRUCTIONS_DIRS",
            "WSL_DISTRO_NAME",
            "WSL_INTEROP",
        ] {
            command.env_remove(key);
        }
        command
            .env("HOME", self.home.path())
            .env("ARA_HOME", self.home.path())
            .env("ARA_API_KEY", "sk-host-bridge-fixture-only")
            .args(["--mode", "rpc", "--model", "fake-model", "--base-url", &upstream.base_url(), "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(["--compact-threshold", "0"])
            .args(args);
        command
    }
}

struct RpcChild {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Result<Value, String>>,
    seen: Vec<Value>,
    stderr: Arc<Mutex<String>>,
    readers: Vec<JoinHandle<()>>,
    deadline: Instant,
}

impl RpcChild {
    fn spawn(env: &Env, upstream: &FakeUpstream, args: &[&str]) -> Self {
        let mut child = env
            .command(upstream, args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let mut stderr_pipe = child.stderr.take().unwrap();
        let (tx, frames) = mpsc::channel();
        let output_reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let frame = line
                    .map_err(|error| error.to_string())
                    .and_then(|line| serde_json::from_str(&line).map_err(|error| format!("{error}: {line}")));
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let captured = stderr.clone();
        let error_reader = std::thread::spawn(move || {
            let mut bytes = [0; 4096];
            while let Ok(length) = stderr_pipe.read(&mut bytes) {
                if length == 0 {
                    break;
                }
                captured.lock().unwrap().push_str(&String::from_utf8_lossy(&bytes[..length]));
            }
        });
        Self {
            child,
            stdin,
            frames,
            seen: Vec::new(),
            stderr,
            readers: vec![output_reader, error_reader],
            deadline: env.deadline,
        }
    }

    fn send(&mut self, frame: Value) {
        self.send_many(&[frame]);
    }

    fn send_many(&mut self, frames: &[Value]) {
        let stdin = self.stdin.as_mut().unwrap();
        for frame in frames {
            writeln!(stdin, "{frame}").unwrap();
        }
        stdin.flush().unwrap();
    }

    fn next(&mut self) -> Value {
        match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(frame)) => {
                self.seen.push(frame.clone());
                frame
            }
            result => {
                panic!("RPC wait failed: {result:?}; stderr: {}; frames: {:?}", self.stderr.lock().unwrap(), self.seen)
            }
        }
    }

    fn until(&mut self, matches: impl Fn(&Value) -> bool) -> Value {
        loop {
            let frame = self.next();
            if matches(&frame) {
                return frame;
            }
        }
    }

    fn response(&mut self, id: &str) -> Value {
        self.until(|frame| frame["type"] == "response" && frame["id"] == id)
    }

    fn success(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], true, "{response}");
        response["data"].clone()
    }

    fn error(&mut self, id: &str) -> Value {
        let response = self.response(id);
        assert_eq!(response["success"], false, "{response}");
        response
    }

    fn ready(&mut self) {
        assert_eq!(self.next()["type"], "ready");
        let metadata = self.next();
        assert_eq!(metadata["type"], "available_commands_update");
        assert_eq!(metadata["commands"], json!([]));
    }

    fn state(&mut self, id: &str) -> Value {
        self.send(json!({"id":id,"type":"get_state"}));
        self.success(id)
    }

    fn prompt(&mut self, id: &str) {
        self.send(json!({"id":id,"type":"prompt","message":format!("bridge task {id}")}));
        self.success(id);
    }

    fn finish(&mut self) {
        self.stdin.take();
        loop {
            match self.frames.recv_timeout(self.deadline.saturating_duration_since(Instant::now())) {
                Ok(Ok(frame)) => self.seen.push(frame),
                Ok(Err(error)) => panic!("{error}"),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => panic!("RPC EOF did not settle: {error}; {}", self.stderr.lock().unwrap()),
            }
        }
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < self.deadline, "RPC child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        };
        for reader in self.readers.drain(..) {
            reader.join().unwrap();
        }
        assert_eq!(status.code(), Some(0), "{}", self.stderr.lock().unwrap());
        assert_eq!(self.seen.iter().filter(|frame| frame["type"] == "session_shutdown").count(), 1);
    }
}

impl Drop for RpcChild {
    fn drop(&mut self) {
        self.stdin.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

async fn upstream(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn calls(calls: &[(&str, &str, Value)]) -> Value {
    let mut events = calls
        .iter()
        .enumerate()
        .map(|(index, (id, name, args))| tool_call(index as u64, id, name, &args.to_string()))
        .collect::<Vec<_>>();
    events.extend([finish("tool_calls"), done()]);
    json!({"events":events})
}

fn answer(message: &str) -> Value {
    json!({"events":[text(message),finish("stop"),done()]})
}

fn definition(name: &str, version: &str, hidden: bool) -> Value {
    json!({"name":name,"label":format!("Host {version}"),"description":format!("Host version {version}"),
        "hidden":hidden,"parameters":{"type":"object","properties":{"version":{"const":version}},
            "required":["version"],"additionalProperties":false}})
}

fn result(id: &Value, text: &str, version: &str) -> Value {
    json!({"type":"host_tool_result","id":id,"result":{"content":[{"type":"text","text":text}],"details":{"adapter":version}}})
}

fn text_content(value: &Value) -> String {
    value["content"].as_str().map(str::to_owned).unwrap_or_else(|| {
        value["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect()
}

fn receipt<'a>(entries: &'a [Value], id: &str) -> &'a Value {
    let matches = entries
        .iter()
        .filter(|entry| entry["message"]["role"] == "toolResult" && entry["message"]["toolCallId"] == id)
        .collect::<Vec<_>>();
    assert_eq!(matches.len(), 1, "exactly one durable receipt for {id}: {entries:?}");
    &matches[0]["message"]
}

fn schema<'a>(request: &'a Value, name: &str) -> &'a Value {
    &request["body"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["function"]["name"] == name)
        .expect("advertised host schema")["function"]
}

fn assert_no_side_channel_ack(child: &RpcChild, ids: &[Value]) {
    assert!(
        !child.seen.iter().any(|frame| frame["type"] == "response" && ids.contains(&frame["id"])),
        "valid bridge replies/updates are consumed without command ACK: {:?}",
        child.seen
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn host_tools_replace_inside_run_keep_pending_adapter_hidden_survivor_and_native_receipts() {
    let up = upstream(vec![
        calls(&[("model-A", "custom", json!({"version":"A"}))]),
        calls(&[("model-B", "custom", json!({"version":"B"}))]),
        answer("Both host receipts accepted."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"register-A","type":"set_host_tools","tools":[definition(" custom ","A",false)]}));
    assert_eq!(child.success("register-A"), json!({"toolNames":["custom"]}));
    child.prompt("same-run");
    let first = child.until(|frame| frame["type"] == "host_tool_call");
    assert_eq!(first["toolCallId"], "model-A");
    assert_ne!(first["id"], first["toolCallId"]);
    assert_eq!(first["arguments"], json!({"version":"A"}));
    let file = PathBuf::from(child.state("pending-file")["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"register-B","type":"set_host_tools","tools":[
        definition("custom","B",true),definition("new_hidden","H",true),definition("fresh","F",false)]}));
    assert_eq!(child.success("register-B")["toolNames"], json!(["custom", "new_hidden", "fresh"]));
    child.send(json!({"id":"duplicate","type":"set_host_tools","tools":[definition("custom","C",false),definition("custom","C",false)]}));
    assert!(child.error("duplicate")["error"].as_str().unwrap().contains("unique"));
    child.send(json!({"id":"collision","type":"set_host_tools","tools":[definition("read","C",false)]}));
    assert!(child.error("collision")["error"].as_str().unwrap().contains("conflicts"));
    let state = child.state("retained-B");
    let active = state["dumpTools"].as_array().unwrap();
    assert!(active.iter().any(|tool| tool["name"] == "read"));
    assert!(active.iter().any(|tool| tool["name"] == "fresh"));
    assert!(!active.iter().any(|tool| tool["name"] == "new_hidden"));
    assert_eq!(
        active.iter().find(|tool| tool["name"] == "custom").unwrap()["parameters"]["properties"]["version"]["const"],
        "B"
    );
    child.send(json!({"type":"host_tool_update","id":first["id"],"partialResult":{"content":[{"type":"text","text":"host progress A"}],"details":{"step":1}}}));
    let update = child.until(|frame| frame["type"] == "tool_execution_update" && frame["toolCallId"] == "model-A");
    assert_eq!(text_content(&update["partialResult"]), "host progress A");
    assert_eq!(update["partialResult"]["details"]["step"], 1);
    child.send(result(&first["id"], "receipt A", "A"));
    let second = child.until(|frame| frame["type"] == "host_tool_call" && frame["toolCallId"] == "model-B");
    assert_ne!(second["id"], first["id"]);
    assert_eq!(second["arguments"], json!({"version":"B"}));
    child.send(result(&first["id"], "stale A must be ignored", "stale"));
    assert_eq!(child.state("stale-barrier")["isStreaming"], true);
    assert!(!child.seen.iter().any(|frame| frame["type"] == "tool_execution_end" && frame["toolCallId"] == "model-B"));
    child.send(result(&second["id"], "receipt B", "B"));
    child.until(|frame| frame["type"] == "agent_end");
    child.finish();
    assert_no_side_channel_ack(&child, &[first["id"].clone(), second["id"].clone()]);
    let entries = journal(&file);
    assert_eq!(receipt(&entries, "model-A")["details"]["adapter"], "A");
    assert_eq!(receipt(&entries, "model-B")["details"]["adapter"], "B");
    assert!(entries.iter().any(|entry| text_content(&entry["message"]).contains("Both host receipts accepted.")));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(schema(&requests[0], "custom")["parameters"]["properties"]["version"]["const"], "A");
    assert_eq!(schema(&requests[1], "custom")["parameters"]["properties"]["version"]["const"], "B");
    assert!(
        !requests[1]["body"]["tools"].as_array().unwrap().iter().any(|tool| tool["function"]["name"] == "new_hidden")
    );
    assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "model-A"
        && text_content(message) == "receipt A"));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_uri_builtin_read_write_preserve_content_type_strip_display_and_enforce_readonly() {
    let up = upstream(vec![
        calls(&[("db-read", "read", json!({"path":"db://table"}))]),
        calls(&[(
            "db-write",
            "write",
            json!({"path":"[db://table#ABCD]","content":"[db://table#ABCD]\n1:标题😀\n2:end"}),
        )]),
        calls(&[("readonly-write", "write", json!({"path":"ro://table","content":"denied"}))]),
        calls(&[("readonly-read", "read", json!({"path":"ro://table"}))]),
        answer("URI work finished."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read,write,edit"]);
    child.ready();
    child.send(json!({"id":"schemes","type":"set_host_uri_schemes","schemes":[
        {"scheme":" DB ","writable":false},{"scheme":"ro","immutable":true},{"scheme":"db","writable":true,"immutable":true}]}));
    assert_eq!(child.success("schemes")["schemes"], json!(["db", "ro"]));
    child.send(json!({"id":"reserved-scheme","type":"set_host_uri_schemes","schemes":[{"scheme":"security"}]}));
    assert!(child.error("reserved-scheme")["error"].as_str().unwrap().contains("reserved"));
    child.prompt("uri-run");
    let read = child.until(|frame| frame["type"] == "host_uri_request");
    assert_eq!(read["operation"], "read");
    assert_eq!(read["url"], "db://table");
    assert!(read.get("content").is_none());
    child.send(json!({"type":"host_uri_result","id":read["id"],"content":"标题😀\nnext","contentType":"text/markdown","immutable":false,"notes":["host note"]}));
    let write = child.until(|frame| frame["type"] == "host_uri_request" && frame["operation"] == "write");
    assert_eq!(write["url"], "db://table");
    assert_eq!(write["content"], "标题😀\nend");
    child.send(json!({"type":"host_uri_result","id":write["id"],"content":"successful write payload is ignored"}));
    let readonly = child.until(|frame| frame["type"] == "host_uri_request" && frame["url"] == "ro://table");
    assert_eq!(readonly["operation"], "read");
    child.send(json!({"type":"host_uri_result","id":readonly["id"],"content":"readonly text"}));
    child.until(|frame| frame["type"] == "agent_end");
    let file = PathBuf::from(child.state("uri-file")["sessionFile"].as_str().unwrap());
    child.finish();
    assert_eq!(child.seen.iter().filter(|frame| frame["type"] == "host_uri_request").count(), 3);
    assert_no_side_channel_ack(&child, &[read["id"].clone(), write["id"].clone(), readonly["id"].clone()]);
    let entries = journal(&file);
    let read_receipt = receipt(&entries, "db-read");
    assert_eq!(read_receipt["isError"], false);
    assert_eq!(read_receipt["details"]["contentType"], "text/markdown");
    assert_eq!(read_receipt["details"]["meta"]["source"], json!({"type":"internal","value":"db://table"}));
    assert!(read_receipt["details"].get("resolvedPath").is_none());
    assert_eq!(text_content(read_receipt), "1:标题😀\n2:next");
    assert_eq!(
        text_content(receipt(&entries, "db-write")),
        "Successfully wrote 8 bytes to db://table\nNote: auto-stripped hashline display prefixes from content before writing."
    );
    assert_eq!(receipt(&entries, "readonly-write")["isError"], true);
    assert!(text_content(receipt(&entries, "readonly-write")).contains("read-only"));
    assert_eq!(receipt(&entries, "readonly-read")["details"]["contentType"], "text/plain");
    assert_eq!(text_content(receipt(&entries, "readonly-read")), "readonly text");
    assert_eq!(
        std::fs::read_dir(env.work.path()).unwrap().count(),
        1,
        "only fixture .git; URIs never materialize files"
    );
    assert_eq!(up.served(), 5);
}

#[tokio::test(flavor = "multi_thread")]
async fn side_channels_after_session_join_and_abort_are_consumed_without_cross_session_completion() {
    let up = upstream(vec![
        calls(&[("same-model-id", "custom", json!({"version":"A"}))]),
        calls(&[("same-model-id", "custom", json!({"version":"A"}))]),
        answer("New session receipt."),
        calls(&[("abort-uri", "read", json!({"path":"db://waiting"}))]),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"register","type":"set_host_tools","tools":[definition("custom","A",false)]}));
    child.success("register");
    child.send(json!({"id":"register-db","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}));
    child.success("register-db");
    child.prompt("origin-run");
    let old = child.until(|frame| frame["type"] == "host_tool_call");
    let old_file = PathBuf::from(child.state("old-file")["sessionFile"].as_str().unwrap());
    child.send(json!({"id":"join-new","type":"new_session"}));
    child.until(|frame| frame["type"] == "host_tool_cancel" && frame["targetId"] == old["id"]);
    child.send_many(&[
        json!({"type":"host_tool_update","id":old["id"],"partialResult":{"content":[{"type":"text","text":"after queued join"}]}}),
        result(&old["id"], "old session completion", "A"),
    ]);
    assert_eq!(child.success("join-new")["cancelled"], false);
    let state = child.state("new-state");
    let new_file = PathBuf::from(state["sessionFile"].as_str().unwrap());
    assert_ne!(new_file, old_file);
    assert!(state["dumpTools"].as_array().unwrap().iter().any(|tool| tool["name"] == "custom"));
    child.prompt("new-run");
    let fresh = child.until(|frame| frame["type"] == "host_tool_call");
    assert_eq!(fresh["toolCallId"], old["toolCallId"]);
    assert_ne!(fresh["id"], old["id"]);
    child.send(result(&old["id"], "stale origin completion", "stale"));
    assert_eq!(child.state("new-waiting")["isStreaming"], true);
    let begin = child.seen.len();
    child.send(result(&fresh["id"], "new receipt only", "A"));
    child.until(|frame| frame["type"] == "agent_end");
    assert!(
        child.seen[begin..]
            .iter()
            .any(|frame| frame["type"] == "tool_execution_end" && text_content(&frame["result"]) == "new receipt only")
    );
    child.prompt("abort-uri-run");
    let uri = child.until(|frame| frame["type"] == "host_uri_request");
    child.send(json!({"id":"join-abort","type":"abort"}));
    child.until(|frame| frame["type"] == "host_uri_cancel" && frame["targetId"] == uri["id"]);
    child.send(json!({"type":"host_uri_result","id":uri["id"],"content":"after queued abort"}));
    child.success("join-abort");
    assert_eq!(child.state("after-abort")["isStreaming"], false);
    child.finish();
    assert_no_side_channel_ack(&child, &[old["id"].clone(), fresh["id"].clone(), uri["id"].clone()]);
    let old_entries = journal(&old_file);
    let old_receipt = receipt(&old_entries, "same-model-id");
    assert_eq!(old_receipt["details"]["executed"], "unknown");
    let new_entries = journal(&new_file);
    assert_eq!(text_content(receipt(&new_entries, "same-model-id")), "new receipt only");
    assert!(!new_entries.iter().any(|entry| text_content(&entry["message"]).contains("stale origin")));
    receipt(&new_entries, "abort-uri");
    let terminal = child.seen.iter().position(|frame| frame["type"] == "agent_end").unwrap();
    let joined = child.seen.iter().position(|frame| frame["type"] == "response" && frame["id"] == "join-new").unwrap();
    assert!(terminal < joined, "old Run settles before new Session adoption ACK");
    assert_eq!(up.served(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn eof_releases_pending_host_and_uri_waits_journals_unknown_and_resume_never_replays() {
    let up = upstream(vec![
        calls(&[("eof-host", "custom", json!({"version":"A"})), ("eof-uri", "read", json!({"path":"db://waiting"}))]),
        answer("EOF drain inspected unknown host receipts."),
        answer("Resume inspected receipts without replay."),
    ])
    .await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send(json!({"id":"eof-tools","type":"set_host_tools","tools":[definition("custom","A",false)]}));
    child.success("eof-tools");
    child.send(json!({"id":"eof-schemes","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}));
    child.success("eof-schemes");
    child.prompt("eof-task");
    let mut pending = Vec::new();
    while pending.len() < 2 {
        let frame = child.next();
        if frame["type"] == "host_tool_call" || frame["type"] == "host_uri_request" {
            pending.push(frame);
        }
    }
    let file = PathBuf::from(child.state("eof-file")["sessionFile"].as_str().unwrap());
    child.finish();
    let entries = journal(&file);
    let host = receipt(&entries, "eof-host");
    assert_eq!(host["isError"], true);
    assert_eq!(host["details"]["executed"], "unknown");
    assert_eq!(host["details"]["source"], "interrupted_unknown_effect");
    let uri = receipt(&entries, "eof-uri");
    assert_eq!(uri["isError"], true);
    assert!(text_content(uri).contains("effect unknown"));
    assert_eq!(up.served(), 2);
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap(), "--tools", "read"]);
    reopened.ready();
    reopened.send(json!({"id":"restored","type":"get_messages"}));
    let restored = reopened.success("restored");
    assert_eq!(
        restored["messages"].as_array().unwrap().iter().filter(|message| message["role"] == "toolResult").count(),
        2
    );
    for frame in &pending {
        if frame["type"] == "host_tool_call" {
            reopened.send(result(&frame["id"], "late previous connection", "stale"));
        } else {
            reopened.send(json!({"type":"host_uri_result","id":frame["id"],"content":"late previous connection"}));
        }
    }
    reopened.prompt("resume-task");
    reopened.until(|frame| frame["type"] == "agent_end");
    reopened.finish();
    assert!(
        !reopened.seen.iter().any(|frame| frame["type"] == "host_tool_call" || frame["type"] == "host_uri_request")
    );
    assert_no_side_channel_ack(&reopened, &pending.iter().map(|frame| frame["id"].clone()).collect::<Vec<_>>());
    let final_entries = journal(&file);
    receipt(&final_entries, "eof-host");
    receipt(&final_entries, "eof-uri");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests[2]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "eof-host"
        && text_content(message).contains("effect unknown")));
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_side_channel_guards_return_ordinary_errors_but_valid_unknown_replies_have_no_ack() {
    let up = upstream(Vec::new()).await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send_many(&[
        json!({"type":"host_tool_result","id":"bad-result","result":{"content":{}}}),
        json!({"type":"host_tool_update","id":"bad-update","partialResult":{"content":"not an array"}}),
        json!({"type":"host_uri_result","id":42,"content":"numeric id"}),
    ]);
    child.error("bad-result");
    child.error("bad-update");
    let numeric = child.until(|frame| frame["type"] == "response" && frame["id"] == 42);
    assert_eq!(numeric["success"], false);
    child.send_many(&[
        json!({"type":"host_tool_result","id":"unknown-tool","result":{"content":[]}}),
        json!({"type":"host_tool_update","id":"unknown-update","partialResult":{"content":[]}}),
        json!({"type":"host_uri_result","id":"unknown-uri"}),
    ]);
    assert_eq!(child.state("guard-barrier")["isStreaming"], false);
    child.finish();
    assert_no_side_channel_ack(&child, &[json!("unknown-tool"), json!("unknown-update"), json!("unknown-uri")]);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn eof_before_accepted_registration_and_prompt_rejects_future_uri_without_dispatch() {
    let mut delayed = calls(&[("after-eof-uri", "read", json!({"path":"db://waiting"}))]);
    // Hold the model response while the stdin reader observes EOF. Scheme
    // registration may be processed either before or after that observation;
    // both paths must reject the later read without publishing a request.
    delayed["delay_ms"] = json!(100);
    let up = upstream(vec![delayed, answer("Queued task inspected the predispatch URI rejection.")]).await;
    let env = Env::new();
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "read"]);
    child.ready();
    child.send_many(&[
        json!({"id":"queued-db","type":"set_host_uri_schemes","schemes":[{"scheme":"db"}]}),
        json!({"id":"queued-run","type":"prompt","message":"accepted work after input EOF"}),
    ]);
    child.finish();
    assert!(
        child
            .seen
            .iter()
            .any(|frame| frame["type"] == "response" && frame["id"] == "queued-db" && frame["success"] == true)
    );
    assert!(
        child
            .seen
            .iter()
            .any(|frame| frame["type"] == "response" && frame["id"] == "queued-run" && frame["success"] == true)
    );
    assert!(
        !child.seen.iter().any(|frame| frame["type"] == "host_uri_request" || frame["type"] == "host_uri_cancel"),
        "an EOF connection cannot publish a new request or create an orphan waiter"
    );
    let files = std::fs::read_dir(&env.sessions)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "jsonl"))
        .collect::<Vec<_>>();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    let rejected = receipt(&entries, "after-eof-uri");
    assert_eq!(rejected["isError"], true);
    let error = text_content(rejected);
    // EOF removes registered routes; re-registration after EOF can restore the
    // route, but dispatch still rejects the permanently closed connection.
    assert!(
        error == "Unknown protocol: db://" || (error.contains("before dispatch") && error.contains("not executed")),
        "{error}"
    );
    assert!(!error.contains("effect unknown"), "absence of publication is proven");
    assert!(entries.iter().any(|entry| text_content(&entry["message"]).contains("Queued task inspected")));
    assert_eq!(up.served(), 2);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["body"]["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool"
        && message["tool_call_id"] == "after-eof-uri"
        && text_content(message) == error));
}
