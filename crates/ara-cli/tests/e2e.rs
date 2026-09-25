//! Real-process tests: the `ara` binary → OpenAI-compatible HTTP → controlled
//! fake upstream, with real tool effects and a real session journal.

use ara_testkit::chunks::*;
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_ara");

struct Env {
    _home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Env {
    fn new() -> Env {
        let home = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let sessions = home.path().join("sessions");
        Env { _home: home, work, sessions }
    }

    fn cmd(&self, base_url: &str, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        for k in [
            "OPENROUTER_API_KEY",
            "ARA_API_KEY",
            "ARA_TEST_API_KEY",
            "ARA_MODEL",
            "ARA_BASE_URL",
            "ARA_TEST_BASE_URL",
            "OPENROUTER_BASE_URL",
            "ARA_TEST_MODEL_ID",
        ] {
            c.env_remove(k);
        }
        c.env("ARA_API_KEY", "sk-e2e-secret-value")
            .env("ARA_HOME", self._home.path())
            .args(["--model", "fake-model", "--base-url", base_url, "--cwd"])
            .arg(self.work.path())
            .arg("--session-dir")
            .arg(&self.sessions)
            .args(args)
            .stdin(Stdio::null());
        c
    }

    /// Like `cmd` but without `--cwd` (runs from `dir`).
    fn cmd_in(&self, dir: &Path, base_url: &str, args: &[&str]) -> Command {
        let mut c = self.cmd(base_url, &[]);
        let mut rebuilt = Command::new(BIN);
        for (k, v) in c.get_envs() {
            match v {
                Some(v) => rebuilt.env(k, v),
                None => rebuilt.env_remove(k),
            };
        }
        let _ = &mut c;
        rebuilt
            .current_dir(dir)
            .args(["--model", "fake-model", "--base-url", base_url, "--session-dir"])
            .arg(&self.sessions)
            .args(args)
            .stdin(Stdio::null());
        rebuilt
    }

    fn session_files(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = std::fs::read_dir(&self.sessions)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")).collect())
            .unwrap_or_default();
        v.sort();
        v
    }
}

fn journal(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path).unwrap().lines().skip(1).map(|l| serde_json::from_str(l).unwrap()).collect()
}

fn roles(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .skip(1)
        .map(|e| match e["type"].as_str().unwrap() {
            "message" => e["message"]["role"].as_str().unwrap().to_string(),
            other => other.to_string(),
        })
        .collect()
}

async fn upstream(v: Value) -> FakeUpstream {
    let script: Script = serde_json::from_value(v).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}

async fn output(mut c: Command) -> Output {
    tokio::task::spawn_blocking(move || c.output().unwrap()).await.unwrap()
}

fn text_of(o: &Output) -> (String, String) {
    (String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

#[tokio::test]
async fn text_answer_is_printed_and_journaled() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("Hello "), text("from the fake model."), finish("stop"), usage(40, 6), done()]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["-p", "Say hello"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(stdout, "Hello from the fake model.\n");
    assert!(stderr.starts_with("Working...\n"));
    assert!(!stdout.contains("sk-e2e") && !stderr.contains("sk-e2e"), "key never printed");
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    assert_eq!(entries[0]["type"], json!("session"));
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant"]);
    assert_eq!(entries[1]["model"], json!("openai-compatible/fake-model"));
    assert_eq!(entries[3]["message"]["usage"]["input"], json!(40));
    let reqs = up.requests.lock().await;
    assert!(reqs[0]["headers"]["authorization"].as_str().unwrap().starts_with("<redacted"));
    assert_eq!(reqs[0]["body"]["messages"][1], json!({"role": "user", "content": "Say hello"}));
}

#[tokio::test]
async fn tool_task_produces_file_and_receipts() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_w", "write", "{\"path\":\"hello.txt\",\"content\":\"hi from ara\\n\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_b", "bash", "{\"command\":\"cat hello.txt && echo done\"}"), finish("tool_calls"), done()]},
        {"events": [text("Created hello.txt and verified it."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Create hello.txt"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Created hello.txt and verified it.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("hello.txt")).unwrap(), "hi from ara\n");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(
        roles(&entries),
        vec!["model_change", "user", "assistant", "toolResult", "assistant", "toolResult", "assistant"]
    );
    assert_eq!(entries[4]["message"]["content"][0]["text"], json!("Successfully wrote 12 bytes to hello.txt"));
    assert_eq!(entries[6]["message"]["content"][0]["text"], json!("hi from ara\ndone"));
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 3);
    let last = reqs[2]["body"]["messages"].as_array().unwrap();
    assert!(
        last.iter().any(|m| m == &json!({"role": "tool", "content": "hi from ara\ndone", "tool_call_id": "call_b"}))
    );
}

fn spawn_json(c: &mut Command) -> Child {
    c.args(["--mode", "json"]).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap()
}

/// Read JSON lines until `pred` matches; returns the lines seen.
fn read_until(reader: &mut impl BufRead, pred: impl Fn(&Value) -> bool, limit: Duration) -> Vec<Value> {
    let started = Instant::now();
    let mut seen = Vec::new();
    let mut line = String::new();
    while started.elapsed() < limit {
        line.clear();
        if reader.read_line(&mut line).unwrap() == 0 {
            break;
        }
        let v: Value = serde_json::from_str(line.trim()).unwrap();
        let hit = pred(&v);
        seen.push(v);
        if hit {
            break;
        }
    }
    seen
}

#[tokio::test(flavor = "multi_thread")]
async fn json_mode_streams_events_before_the_run_ends() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("first "), {"sleep_ms": 1500}, text("second"), finish("stop"), done()]}]})).await;
    let mut c = env.cmd(&up.base_url(), &["stream please"]);
    let mut child = spawn_json(&mut c);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let seen = tokio::task::block_in_place(|| {
        read_until(
            &mut reader,
            |v| v["type"] == "message_update" && v["assistantMessageEvent"]["delta"] == "first ",
            Duration::from_secs(10),
        )
    });
    assert!(child.try_wait().unwrap().is_none(), "delta observed while ara is still running");
    assert_eq!(seen[0]["type"], json!("session"));
    assert_eq!(seen[1]["type"], json!("agent_start"));
    assert!(
        seen.last().unwrap()["assistantMessageEvent"].get("partial").is_none(),
        "no partial snapshots in JSON output"
    );
    let rest: Vec<Value> = reader.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(rest.last().unwrap()["type"], json!("agent_end"));
    let end = rest.iter().find(|v| v["type"] == "message_end" && v["message"]["role"] == "assistant").unwrap();
    assert_eq!(end["message"]["content"][0]["text"], json!("first second"));
}

#[tokio::test]
async fn upstream_auth_failure_exits_nonzero_with_the_error() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"status": 401, "body": "{\"error\":{\"message\":\"bad key\"}}"}]})).await;
    let out = output(env.cmd(&up.base_url(), &["hi"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout, "");
    assert!(stderr.lines().any(|l| l == "401 bad key"), "{stderr}");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(entries.last().unwrap()["message"]["stopReason"], json!("error"));
    assert_eq!(entries.last().unwrap()["message"]["errorStatus"], json!(401));
}

fn signal(child: &Child, sig: i32) {
    assert_eq!(unsafe { libc::kill(child.id() as i32, sig) }, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn sigint_during_a_tool_aborts_and_journals() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_s", "bash", "{\"command\":\"echo started; sleep 30\"}"), finish("tool_calls"), done()]},
        {"events": [text("should not be requested"), finish("stop"), done()]}
    ]}))
    .await;
    let mut c = env.cmd(&up.base_url(), &["run something slow"]);
    let mut child = spawn_json(&mut c);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    tokio::task::block_in_place(|| {
        read_until(&mut reader, |v| v["type"] == "tool_execution_update", Duration::from_secs(10));
    });
    let started = Instant::now();
    signal(&child, libc::SIGINT);
    let _rest: Vec<String> = reader.lines().map(Result::unwrap).collect();
    let status = child.wait().unwrap();
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    assert_eq!(status.code(), Some(1));
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(stderr.contains("interrupt received") && stderr.lines().any(|l| l == "Request was aborted"), "{stderr}");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert!(entries[4]["message"]["content"][0]["text"].as_str().unwrap().ends_with("[Command aborted]"));
    assert_eq!(entries[5]["message"]["stopReason"], json!("aborted"));
    assert_eq!(up.served(), 1, "no model call after the abort");
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_mid_tool_resume_reports_unknown_effect_without_replay() {
    let env = Env::new();
    let token = format!("ara-e2e-{}", std::process::id());
    let cmd = format!("echo run >> runs.log; sleep 30 # {token}");
    let args = json!({"command": cmd}).to_string();
    let up = upstream(
        json!({"responses": [{"events": [tool_call(0, "call_crash", "bash", &args), finish("tool_calls"), done()]}]}),
    )
    .await;
    let mut c = env.cmd(&up.base_url(), &["do the long thing"]);
    let mut child = spawn_json(&mut c);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    tokio::task::block_in_place(|| {
        read_until(&mut reader, |v| v["type"] == "tool_execution_start", Duration::from_secs(10));
    });
    let runs = env.work.path().join("runs.log");
    let t0 = Instant::now();
    while !runs.exists() && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(20));
    }
    signal(&child, libc::SIGKILL);
    child.wait().unwrap();
    // The orphaned process group keeps running: its effect really is unknown to ara.
    let _ = Command::new("pkill").args(["-f", &token]).status();
    let session = env.session_files().pop().expect("journal materialized before the tool started");

    let up2 =
        upstream(json!({"responses": [{"events": [text("Resumed after checking."), finish("stop"), done()]}]})).await;
    let out = output(env.cmd(&up2.base_url(), &["--resume", session.to_str().unwrap(), "continue carefully"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Resumed after checking.\n");
    assert!(stderr.contains("call_crash were interrupted before a result was recorded; their effects are unknown and they were not re-run"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&runs).unwrap(), "run\n", "the command was not replayed");
    let entries = journal(&session);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "user", "assistant"]);
    assert_eq!(entries[4]["message"]["details"]["source"], json!("interrupted_unknown_effect"));
    let reqs = up2.requests.lock().await;
    let msgs = reqs[0]["body"]["messages"].as_array().unwrap();
    let tool_msg = msgs.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool_msg["tool_call_id"], json!("call_crash"));
    assert!(tool_msg["content"].as_str().unwrap().contains("effects are unknown"));
}

#[tokio::test]
async fn deadline_and_model_call_budget() {
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"delay_ms": 5000, "events": [text("late"), finish("stop"), done()]}]})).await;
    let started = Instant::now();
    let out = output(env.cmd(&up.base_url(), &["--max-time", "1", "hi"])).await;
    let (_, stderr) = text_of(&out);
    assert!(started.elapsed() < Duration::from_secs(4), "{:?}", started.elapsed());
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr.lines().any(|l| l == "Deadline exceeded"), "{stderr}");

    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "c1", "bash", "{\"command\":\"echo one\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "c2", "bash", "{\"command\":\"echo two\"}"), finish("tool_calls"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--max-model-calls", "1", "loop"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stdout, "");
    assert!(stderr.contains("model call limit reached (1)"), "{stderr}");
    assert_eq!(up.served(), 1);
}

#[tokio::test]
async fn continue_and_stdin_prompt() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("one"), finish("stop"), done()]},
        {"events": [text("two"), finish("stop"), done()]}
    ]}))
    .await;
    assert_eq!(output(env.cmd(&up.base_url(), &["first"])).await.status.code(), Some(0));
    let mut c = env.cmd(&up.base_url(), &["-c"]);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"second from stdin\n").unwrap();
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().unwrap()).await.unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "two\n");
    let files = env.session_files();
    assert_eq!(files.len(), 1, "continued the same session");
    assert_eq!(roles(&journal(&files[0])), vec!["model_change", "user", "assistant", "user", "assistant"]);
    let reqs = up.requests.lock().await;
    assert_eq!(reqs[1]["body"]["messages"].as_array().unwrap().len(), 4, "system + prior turn + new prompt");
    assert_eq!(reqs[1]["body"]["messages"][3], json!({"role": "user", "content": "second from stdin"}));
}

#[tokio::test]
async fn usage_errors_exit_2() {
    let env = Env::new();
    let out = output(env.cmd("http://127.0.0.1:9/v1", &["--tools", "read,teleport", "x"])).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown tool \"teleport\""));
}

#[tokio::test]
async fn deadline_during_a_tool_and_zero_budget_exit_nonzero() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "c1", "bash", "{\"command\":\"sleep 5\"}"), finish("tool_calls"), done()]},
        {"events": [text("never"), finish("stop"), done()]}
    ]}))
    .await;
    let started = Instant::now();
    let out = output(env.cmd(&up.base_url(), &["--max-time", "1", "slow tool"])).await;
    let (stdout, stderr) = text_of(&out);
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.lines().any(|l| l == "Deadline exceeded"), "{stderr}");
    assert_eq!(up.served(), 1);

    let up = upstream(json!({"responses": []})).await;
    let out = output(env.cmd(&up.base_url(), &["--max-model-calls", "0", "hi"])).await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text_of(&out).1.contains("model call limit reached (0)"));
    assert_eq!(up.served(), 0);
}

#[tokio::test]
async fn stdin_is_prepended_and_keys_stay_on_their_route() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("reviewed"), finish("stop"), done()]}]})).await;
    let mut c = env.cmd(&up.base_url(), &["review this diff"]);
    c.env_remove("ARA_API_KEY").env("OPENROUTER_API_KEY", "sk-or-must-not-leak");
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"--- a\n+++ b\n").unwrap();
    let out = tokio::task::spawn_blocking(move || child.wait_with_output().unwrap()).await.unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let reqs = up.requests.lock().await;
    assert_eq!(reqs[0]["body"]["messages"][1]["content"], json!("--- a\n+++ b\nreview this diff"));
    assert!(reqs[0]["headers"].get("authorization").is_none(), "OpenRouter key not sent to another host");
}

#[tokio::test]
async fn argument_errors_do_not_touch_a_resumed_session() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "c1", "bash", "{\"command\":\"true\"}"), finish("tool_calls"), done()]},
        {"status": 400, "body": "{\"error\":{\"message\":\"stop here\"}}"}
    ]}))
    .await;
    // The second model call fails, so the run errors after the tool.
    let _ = output(env.cmd(&up.base_url(), &["x"])).await;
    let session = env.session_files().pop().unwrap();
    let before = std::fs::read(&session).unwrap();
    let out =
        output(env.cmd(&up.base_url(), &["--resume", session.to_str().unwrap(), "--tools", "read,teleport", "y"]))
            .await;
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(std::fs::read(&session).unwrap(), before, "usage error left the journal untouched");
    let out = output(env.cmd(&up.base_url(), &["--no-session", "--resume", session.to_str().unwrap(), "y"])).await;
    assert_eq!(out.status.code(), Some(2), "--no-session cannot silently drop --resume");
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot be used with"));
}

#[tokio::test]
async fn resume_runs_tools_in_the_session_cwd() {
    let env = Env::new();
    let project = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let up = upstream(json!({"responses": [
        {"events": [text("started"), finish("stop"), done()]},
        {"events": [tool_call(0, "c1", "bash", "{\"command\":\"pwd > where.txt\"}"), finish("tool_calls"), done()]},
        {"events": [text("done"), finish("stop"), done()]}
    ]}))
    .await;
    assert_eq!(output(env.cmd_in(project.path(), &up.base_url(), &["start"])).await.status.code(), Some(0));
    let session = env.session_files().pop().unwrap();
    let out =
        output(env.cmd_in(elsewhere.path(), &up.base_url(), &["--resume", session.to_str().unwrap(), "where am i"]))
            .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let recorded = std::fs::read_to_string(project.path().join("where.txt")).unwrap();
    assert_eq!(recorded.trim(), std::fs::canonicalize(project.path()).unwrap().to_string_lossy());
    assert!(!elsewhere.path().join("where.txt").exists());
}
