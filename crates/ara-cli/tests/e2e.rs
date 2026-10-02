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

fn invocation_skill(env: &Env, hidden: bool) -> PathBuf {
    let path = env.work.path().join(".ara").join("skills").join("proof").join("SKILL.md");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("---\nname: proof\ndescription: Invocation proof\nhide: {hidden}\n---\nFollow the user's task and preserve exact output.\n")).unwrap();
    path
}

fn skill_entries(entries: &[Value]) -> Vec<&Value> {
    entries.iter().filter(|entry| entry["type"] == "custom_message" && entry["customType"] == "skill-prompt").collect()
}

#[tokio::test]
async fn repl_skill_dispatch_parses_js_whitespace_before_display_sanitization() {
    let env = Env::new();
    invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"events":[text("Embedded."),finish("stop"),done()]},
        {"events":[text("Ordinary."),finish("stop"),done()]},
        {"events":[text("BOM."),finish("stop"),done()]},
        {"events":[text("Internal tab name."),finish("stop"),done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0"]),
        "before\u{b}/skill:proof\n/skill:proof\t\n\u{feff}/skill:proof arg\n/skill:proof\tother\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let entries = journal(&env.session_files()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 3);
    assert_eq!(skills[0]["details"]["args"], "before");
    assert_eq!(skills[0]["details"]["originalText"], "before\u{b}/skill:proof\n");
    assert!(skills[1]["details"].get("args").is_none());
    assert_eq!(skills[2]["details"]["args"], "arg");
    assert_eq!(
        journal_user_texts(&entries),
        vec!["/skill:proof\tother"],
        "leading name retains its internal tab for lookup; ordinary input keeps its existing normalization"
    );
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn repl_skill_file_replaced_with_fifo_fails_without_blocking_exit() {
    use std::os::unix::ffi::OsStrExt;
    let env = Env::new();
    let path = invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"events":[text("Before FIFO."),finish("stop"),done()]},
        {"events":[text("must not run"),finish("stop"),done()]}
    ]}))
    .await;
    let mut command =
        env.cmd(&up.base_url(), &["--repl", "--mode", "json", "--compact-threshold", "0", "--max-time", "1"]);
    let mut child = spawn_repl(&mut command);
    let mut stdin = child.stdin.take().unwrap();
    let stderr = StderrLog::start(&mut child);
    let stdout = StderrLog::reading(child.stdout.take().unwrap());
    tokio::task::block_in_place(|| {
        writeln!(stdin, "before").unwrap();
        assert!(stdout.wait_for("agent_end", Duration::from_secs(10)));
        std::fs::remove_file(&path).unwrap();
        let fifo = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // The named pipe has no writer; an ordinary read would block forever.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        writeln!(stdin, "/skill:proof fifo-args").unwrap();
        writeln!(stdin, "/exit").unwrap();
        drop(stdin);
        assert_eq!(wait_exit(&mut child, Duration::from_secs(5)), Some(0));
    });
    let error = stderr.finish();
    let output = stdout.finish();
    assert!(error.contains("not a regular file"), "{error}");
    assert_eq!(up.served(), 1);
    let entries = journal(&env.session_files()[0]);
    assert!(skill_entries(&entries).is_empty());
    assert_eq!(journal_user_texts(&entries), vec!["before"]);
    if let Some(directory) = std::env::var_os("ARA_CTX_INVOCATION_RECEIPTS") {
        let directory = PathBuf::from(directory).join("fifo-replacement");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("stdout.jsonl"), output).unwrap();
        std::fs::write(directory.join("stderr.txt"), error).unwrap();
        std::fs::copy(&env.session_files()[0], directory.join("session.jsonl")).unwrap();
        std::fs::write(directory.join("requests.json"), serde_json::to_vec_pretty(&*up.requests.lock().await).unwrap())
            .unwrap();
        std::fs::write(
            directory.join("summary.json"),
            serde_json::to_vec_pretty(&json!({
                "status":"PASS","case":"regular Skill replaced by unopened FIFO after discovery", "exitCode":0,
                "modelRequests":up.served(),"customEntries":skill_entries(&entries).len(), "exitBoundSeconds":5,
                "invocation":"/skill:proof fifo-args","initialPrompt":"before"
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_skill_cancelled_tool_keeps_one_custom_and_returns_to_prompt() {
    let env = Env::new();
    invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"skill-slow","bash","{\"command\":\"echo started; sleep 30\"}"),finish("tool_calls"),done()]},
        {"events":[text("After skill abort."),finish("stop"),done()]}
    ]})).await;
    let mut command = env.cmd(&up.base_url(), &["--repl", "--mode", "json", "--compact-threshold", "0"]);
    let mut child = spawn_repl(&mut command);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let started = Instant::now();
    let (events, code) = tokio::task::block_in_place(|| {
        writeln!(stdin, "/skill:proof slow-task").unwrap();
        let mut events =
            read_until(&mut reader, |event| event["type"] == "tool_execution_update", Duration::from_secs(15));
        interrupt(&child);
        events.extend(read_until(&mut reader, |event| event["type"] == "agent_end", Duration::from_secs(10)));
        assert!(log.wait_for("ara: turn 1 cancelled; session kept", Duration::from_secs(5)));
        assert!(child.try_wait().unwrap().is_none());
        writeln!(stdin, "next").unwrap();
        drop(stdin);
        events.extend(reader.lines().map(|line| serde_json::from_str::<Value>(&line.unwrap()).unwrap()));
        (events, wait_exit(&mut child, Duration::from_secs(15)))
    });
    let error = log.finish();
    assert_eq!(code, Some(0), "{error}");
    assert!(started.elapsed() < Duration::from_secs(20));
    let entries = journal(&env.session_files()[0]);
    assert_eq!(skill_entries(&entries).len(), 1);
    assert_eq!(journal_user_texts(&entries), vec!["next"]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["toolCallId"] == "skill-slow").count(), 1);
    assert_eq!(events.iter().filter(|event| event["type"] == "tool_execution_start").count(), 1, "no replay");
    assert_eq!(up.served(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_skill_is_custom_runs_tools_and_replays_after_source_removal() {
    let env = Env::new();
    let path = invocation_skill(&env, true);
    let up = upstream(json!({"responses":[
        {"events":[tool_call(0,"proof-write","write","{\"path\":\"PROOF.txt\",\"content\":\"artifact\"}"),finish("tool_calls"),done()]},
        {"events":[text("Artifact written."),finish("stop"),done()]},
        {"events":[text("Next turn."),finish("stop"),done()]}
    ]})).await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--mode", "json", "--compact-threshold", "0"]),
        "left /skill:proof focus\nnext\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(std::fs::read_to_string(env.work.path().join("PROOF.txt")).unwrap(), "artifact");
    let entries = journal(&env.session_files()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 1);
    let skill = skills[0];
    assert_eq!(skill["display"], true);
    assert_eq!(skill["attribution"], "user");
    assert_eq!(skill["details"]["name"], "proof");
    assert_eq!(skill["details"]["path"], path.to_string_lossy().as_ref());
    assert_eq!(skill["details"]["args"], "left focus");
    assert_eq!(skill["details"]["lineCount"], 1);
    assert_eq!(skill["details"]["originalText"], "left /skill:proof focus\n");
    assert_eq!(journal_user_texts(&entries), vec!["next"], "the Skill has no ordinary duplicate");
    let events: Vec<Value> = stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    for event_type in ["message_start", "message_end"] {
        let custom: Vec<_> =
            events.iter().filter(|event| event["type"] == event_type && event["message"]["role"] == "custom").collect();
        assert_eq!(custom.len(), 1, "{events:?}");
        assert_eq!(custom[0]["message"]["content"], skill["content"]);
    }
    let first_end = events.iter().find(|event| event["type"] == "agent_end").unwrap();
    assert_eq!(first_end["messages"][0]["role"], "custom");
    let reqs = up.requests.lock().await;
    for request in reqs.iter() {
        let body = &request["body"];
        let text = request_text(body["messages"].as_array().unwrap());
        assert_eq!(text.matches("[IMPORTANT: User invoked").count(), 1, "{body}");
        assert!(text.contains("User: left focus"));
        assert!(!body.to_string().contains("originalText"));
        assert!(body["messages"].as_array().unwrap().iter().all(|message| message["role"] != "custom"));
    }
    drop(reqs);
    std::fs::remove_file(&path).unwrap();
    let resumed = upstream(json!({"responses":[{"events":[text("Recalled."),finish("stop"),done()]}]})).await;
    let out = repl_output(
        env.cmd(&resumed.base_url(), &["--repl", "--continue", "--compact-threshold", "0"]),
        "after restart\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let reqs = resumed.requests.lock().await;
    let body = &reqs[0]["body"];
    let text = request_text(body["messages"].as_array().unwrap());
    assert_eq!(text.matches("[IMPORTANT: User invoked").count(), 1);
    assert!(text.contains("User: left focus"));
    assert_eq!(skill_entries(&journal(&env.session_files()[0])).len(), 1);
}

#[tokio::test]
async fn repl_unknown_disabled_and_print_skill_inputs_take_the_ordinary_path() {
    for (args, input) in [
        (vec!["--repl"], "/skill:unknown\n"),
        (vec!["--repl", "--no-skills"], "/skill:proof\n"),
        (vec!["-p", "/skill:proof"], ""),
    ] {
        let env = Env::new();
        invocation_skill(&env, false);
        let up = upstream(json!({"responses":[{"events":[text("Ordinary."),finish("stop"),done()]}]})).await;
        let out = if args.contains(&"--repl") {
            repl_output(env.cmd(&up.base_url(), &args), input).await
        } else {
            output(env.cmd(&up.base_url(), &args)).await
        };
        assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
        let entries = journal(&env.session_files()[0]);
        assert!(skill_entries(&entries).is_empty());
        assert_eq!(journal_user_texts(&entries), vec![if input.is_empty() { "/skill:proof" } else { input.trim() }]);
        let reqs = up.requests.lock().await;
        let text = request_text(reqs[0]["body"]["messages"].as_array().unwrap());
        assert!(!text.contains("[IMPORTANT: User invoked"));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_known_skill_reloads_then_consumes_a_deleted_file_error() {
    let env = Env::new();
    let path = invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"events":[text("Before."),finish("stop"),done()]},
        {"events":[text("Fresh."),finish("stop"),done()]},
        {"events":[text("After failure."),finish("stop"),done()]}
    ]}))
    .await;
    let mut c = env.cmd(&up.base_url(), &["--repl", "--mode", "json", "--compact-threshold", "0"]);
    let mut child = spawn_repl(&mut c);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let code = tokio::task::block_in_place(|| {
        writeln!(stdin, "before").unwrap();
        read_until(&mut reader, |event| event["type"] == "agent_end", Duration::from_secs(15));
        std::fs::write(&path, "Fresh instructions unique marker.\n").unwrap();
        writeln!(stdin, "/skill:proof fresh-args").unwrap();
        read_until(&mut reader, |event| event["type"] == "agent_end", Duration::from_secs(15));
        std::fs::remove_file(&path).unwrap();
        writeln!(stdin, "/skill:proof deleted-args").unwrap();
        writeln!(stdin, "ordinary-after-failure").unwrap();
        drop(stdin);
        let _rest: Vec<Value> = reader.lines().map(|line| serde_json::from_str(&line.unwrap()).unwrap()).collect();
        wait_exit(&mut child, Duration::from_secs(15))
    });
    let stderr = log.finish();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("failed to load skill"), "{stderr}");
    assert_eq!(up.served(), 3, "failed load never becomes a provider prompt");
    let entries = journal(&env.session_files()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 1);
    assert!(skills[0]["content"].as_str().unwrap().contains("Fresh instructions unique marker."));
    assert_eq!(journal_user_texts(&entries), vec!["before", "ordinary-after-failure"]);
    let reqs = up.requests.lock().await;
    assert!(
        request_text(reqs[1]["body"]["messages"].as_array().unwrap()).contains("Fresh instructions unique marker.")
    );
    assert!(!reqs[2]["body"].to_string().contains("deleted-args"));
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_skill_compaction_keeps_real_custom_ids_and_restarts() {
    let env = Env::new();
    let path = invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"events":[text("First."),finish("stop"),done()]},
        {"events":[text("Second."),finish("stop"),done()]},
        {"events":[text("First Skill task finished."),finish("stop"),done()]},
        {"events":[text("First Skill task finished."),finish("stop"),done()]},
        {"events":[text("Third."),finish("stop"),done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "/skill:proof first-args\n/skill:proof kept-args\n/compact\nthird\n",
    )
    .await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    let entries = journal(&env.session_files()[0]);
    let skills = skill_entries(&entries);
    assert_eq!(skills.len(), 2);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1, "{entries:?}");
    let compaction = compactions[0];
    let before_kept = entries.iter().position(|entry| entry["id"] == compaction["firstKeptEntryId"]).unwrap();
    assert_eq!(entries[before_kept]["message"]["role"], "assistant");
    assert_eq!(entries[before_kept]["message"]["content"][0]["text"], "Second.");
    let expected: Vec<_> = entries[..before_kept]
        .iter()
        .filter(|entry| entry["type"] == "message" || entry["type"] == "custom_message")
        .map(|entry| entry["id"].clone())
        .collect();
    assert_eq!(compaction["sourceEntryIds"], json!(expected));
    assert!(expected.contains(&skills[0]["id"]));
    assert!(expected.contains(&skills[1]["id"]));
    assert_eq!(skills[0]["details"]["originalText"], "/skill:proof first-args\n");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 5);
    let summaries: Vec<_> = reqs[2..4]
        .iter()
        .map(|request| {
            assert!(tools_absent_or_empty(&request["body"]));
            request_text(request["body"]["messages"].as_array().unwrap())
        })
        .collect();
    assert!(summaries.iter().any(|text| text.contains("User: first-args")));
    assert!(summaries.iter().any(|text| text.contains("User: kept-args")));
    let last = request_text(reqs[4]["body"]["messages"].as_array().unwrap());
    assert!(last.contains("First Skill task finished.") && last.contains("Second."));
    assert!(!last.contains("User: first-args"));
    assert!(!last.contains("User: kept-args"));
    drop(reqs);
    std::fs::remove_file(path).unwrap();
    let resumed = upstream(json!({"responses":[{"events":[text("Restored."),finish("stop"),done()]}]})).await;
    let out = repl_output(
        env.cmd(&resumed.base_url(), &["--repl", "--continue", "--compact-threshold", "0"]),
        "after-compacted-restart\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let reqs = resumed.requests.lock().await;
    let last = request_text(reqs[0]["body"]["messages"].as_array().unwrap());
    assert!(last.contains("First Skill task finished.") && last.contains("Second."));
    assert!(!last.contains("User: first-args"));
    assert!(!last.contains("User: kept-args"));
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_skill_provider_error_retains_custom_and_next_turn() {
    let env = Env::new();
    invocation_skill(&env, false);
    let up = upstream(json!({"responses":[
        {"status":400,"body":"{\"error\":{\"message\":\"skill backend down\"}}"},
        {"events":[text("Recovered."),finish("stop"),done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0"]),
        "/skill:proof error-turn\nnext\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Recovered.\n");
    assert!(stderr.contains("skill backend down"));
    let entries = journal(&env.session_files()[0]);
    assert_eq!(skill_entries(&entries).len(), 1);
    assert_eq!(journal_user_texts(&entries), vec!["next"]);
    assert_eq!(up.served(), 2);
}

struct Env {
    _home: tempfile::TempDir,
    work: tempfile::TempDir,
    sessions: PathBuf,
}

impl Env {
    fn new() -> Env {
        // Non-hidden names: discovery skips AGENTS.md in hidden directories
        // (tempfile's default `.tmpXXXX`).
        let home = tempfile::Builder::new().prefix("ara-e2e-home-").tempdir().unwrap();
        let work = tempfile::Builder::new().prefix("ara-e2e-work-").tempdir().unwrap();
        let sessions = home.path().join("sessions");
        Env { _home: home, work, sessions }
    }

    fn cmd(&self, base_url: &str, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        for k in [
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
            c.env_remove(k);
        }
        // Discovery reads the user's home: isolate it.
        c.env("ARA_API_KEY", "sk-e2e-secret-value")
            .env("HOME", self._home.path())
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
    // System blocks (main prompt + project footer), then the prompt with the
    // date/cwd reminder the provider hook prepends at request time.
    let messages = reqs[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(messages.iter().filter(|m| m["role"] == "system").count(), 2);
    assert!(messages[0]["content"].as_str().unwrap().contains("in ARA coding harness"));
    assert!(messages[1]["content"].as_str().unwrap().contains("<workstation>"));
    let first_user = messages[2]["content"].as_str().unwrap();
    assert!(first_user.starts_with("<system-reminder>\nToday: "), "{first_user}");
    assert!(first_user.ends_with("</system-reminder>\n\nSay hello"), "{first_user}");
    // The stored transcript keeps the prompt without the reminder.
    assert_eq!(entries[2]["message"]["content"], json!("Say hello"));
}

#[tokio::test]
async fn anthropic_messages_runs_tool_and_replays_after_restart() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_tool","usage":{"input_tokens":10}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_write","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"anthropic.txt\",\"content\":\"from anthropic\\n\"}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":5}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_final"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Wrote anthropic.txt."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_resume"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"The file remains."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]})).await;
    let first =
        output(env.cmd(&up.base_url(), &["--api", "anthropic-messages", "--tools", "write", "Write anthropic.txt"]))
            .await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote anthropic.txt.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("anthropic.txt")).unwrap(), "from anthropic\n");
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    assert!(entries.iter().any(|entry| entry["message"]["role"] == "toolResult"));
    assert!(entries.iter().any(|entry| {
        entry["message"]["role"] == "assistant"
            && entry["message"]["content"]
                .as_array()
                .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "toolCall"))
    }));
    let resumed = output(env.cmd(
        &up.base_url(),
        &["--api", "anthropic-messages", "--resume", files[0].to_str().unwrap(), "Check the file"],
    ))
    .await;
    let (stdout, stderr) = text_of(&resumed);
    assert_eq!(resumed.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "The file remains.\n");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request["request"] == "POST /v1/messages HTTP/1.1"));
    assert!(
        requests.iter().all(|request| request["headers"]["authorization"].as_str().unwrap().starts_with("<redacted"))
    );
    assert!(requests.iter().all(|request| request["headers"].get("x-api-key").is_none()));
    assert_eq!(requests[1]["body"]["messages"][2]["content"][0]["tool_use_id"], "toolu_write");
    assert_eq!(requests[2]["body"]["messages"][2]["content"][0]["tool_use_id"], "toolu_write");
}

#[tokio::test]
async fn anthropic_strict_rejection_falls_back_and_stays_disabled_in_one_cli_session() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    std::fs::write(env.work.path().join("math.txt"), "one minus one\n").unwrap();
    let edit_args = json!({"path":"math.txt","old_string":"minus","new_string":"plus"}).to_string();
    let up = upstream(json!({"responses":[
        {"status":400,"body":json!({"error":{"type":"invalid_request_error","message":"The compiled grammar is too large"}}).to_string()},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_edit"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_edit","name":"edit","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":edit_args}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_edited"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Edited once."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_checked"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Confirmed once."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]})).await;
    let out = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "anthropic-messages",
            "--anthropic-strict-tools",
            "--tools",
            "edit",
            "--edit-mode",
            "replace",
            "Change minus to plus",
            "Confirm the edit",
        ],
    ))
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Confirmed once.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("math.txt")).unwrap(), "one plus one\n");
    assert_eq!(up.served(), 4);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0]["body"]["tools"][0]["strict"], true);
    for request in requests.iter().skip(1) {
        assert!(request["body"]["tools"][0].get("strict").is_none());
    }
    assert_eq!(requests[0]["body"]["messages"], requests[1]["body"]["messages"]);
    assert_eq!(requests[2]["body"]["messages"][2]["content"][0]["tool_use_id"], "toolu_edit");
    assert_eq!(requests[3]["body"]["messages"][2]["content"][0]["tool_use_id"], "toolu_edit");
    assert!(requests[3]["body"]["messages"].to_string().contains("Confirm the edit"));
    drop(requests);
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "user").count(), 2);
    let results: Vec<_> = entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["message"]["toolCallId"], "toolu_edit");
    assert_eq!(results[0]["message"]["isError"], false);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| {
                entry["message"]["role"] == "assistant"
                    && entry["message"]["content"].as_array().is_some_and(|blocks| {
                        blocks.iter().any(|block| block["type"] == "toolCall" && block["id"] == "toolu_edit")
                    })
            })
            .count(),
        1
    );
}

#[tokio::test]
async fn anthropic_strict_option_rejects_other_api_and_unrelated_bad_request() {
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"status":400,"body":json!({"error":{"type":"invalid_request_error","message":"Unrelated request error"}}).to_string()}
    ]})).await;
    let invalid = output(env.cmd(&up.base_url(), &["--anthropic-strict-tools", "hello"])).await;
    assert_eq!(invalid.status.code(), Some(2));
    assert!(text_of(&invalid).1.contains("--anthropic-strict-tools requires --api anthropic-messages"));
    assert!(env.session_files().is_empty());
    assert_eq!(up.served(), 0);

    std::fs::write(env.work.path().join("math.txt"), "one minus one\n").unwrap();
    let rejected = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "anthropic-messages",
            "--anthropic-strict-tools",
            "--tools",
            "edit",
            "--edit-mode",
            "replace",
            "Edit math.txt",
        ],
    ))
    .await;
    assert_eq!(rejected.status.code(), Some(1), "{}", text_of(&rejected).1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("math.txt")).unwrap(), "one minus one\n");
    assert_eq!(up.served(), 1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["body"]["tools"][0]["strict"], true);
    let entries = journal(&env.session_files()[0]);
    assert!(entries.iter().all(|entry| entry["message"]["role"] != "toolResult"));
    assert_eq!(entries.last().unwrap()["message"]["stopReason"], "error");
}

#[tokio::test]
async fn anthropic_explicit_max_tokens_reaches_the_real_host_request() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_limit"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Limit received."}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        frame(json!({"type":"message_stop"}))
    ]}]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--api", "anthropic-messages", "--max-tokens", "8192", "Reply"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Limit received.\n");
    assert_eq!(up.served(), 1);
    let requests = up.requests.lock().await;
    assert_eq!(requests[0]["body"]["max_tokens"], 8192);
    assert_eq!(journal(&env.session_files()[0]).last().unwrap()["message"]["stopReason"], "stop");
}

#[tokio::test]
async fn anthropic_ping_bridged_tool_call_runs_once_in_the_real_host() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_ping_tool"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_ping_write","name":"write","input":{}}})),
            {"sleep_ms":220}, frame(json!({"type":"ping"})),
            {"sleep_ms":220}, frame(json!({"type":"ping"})),
            {"sleep_ms":220},
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"ping.txt\",\"content\":\"one write\\n\"}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_ping_final"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Wrote ping.txt."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]})).await;
    let out = output(env.cmd(
        &up.base_url(),
        &["--api", "anthropic-messages", "--tools", "write", "--stream-idle-timeout", "0.35", "Write ping.txt"],
    ))
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote ping.txt.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("ping.txt")).unwrap(), "one write\n");
    assert_eq!(up.served(), 2);
    let entries = journal(&env.session_files()[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
}

#[tokio::test]
async fn anthropic_refusal_exits_with_reason_and_preserves_details_without_tool_effects() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let details = json!({"type":"refusal","category":"policy","explanation":"  Request blocked.  "});
    let up = upstream(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_refused"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_refused","name":"write","input":{}}})),
        frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"refused.txt\",\"content\":\"unsafe\"}"}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"refusal","stop_details":details}})),
        frame(json!({"type":"message_stop"}))
    ]}]}))
    .await;
    let out =
        output(env.cmd(&up.base_url(), &["--api", "anthropic-messages", "--tools", "write", "Write refused.txt"]))
            .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("Refusal (policy): Request blocked."), "{stderr}");
    assert!(!env.work.path().join("refused.txt").exists());
    assert_eq!(up.served(), 1);
    let entries = journal(&env.session_files()[0]);
    let results: Vec<_> = entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").collect();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["message"]["toolCallId"], json!("toolu_refused"));
    assert_eq!(results[0]["message"]["isError"], json!(true));
    assert_eq!(results[0]["message"]["details"]["__synthetic"], json!(true));
    assert_eq!(results[0]["message"]["details"]["executed"], json!(false));
    let assistant = entries.iter().find(|entry| entry["message"]["role"] == "assistant").unwrap();
    assert_eq!(assistant["message"]["stopReason"], json!("error"));
    assert_eq!(assistant["message"]["stopDetails"], details);
}

#[tokio::test]
async fn anthropic_ping_stall_does_not_execute_an_unfinished_write() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let mut frames = vec![
        frame(json!({"type":"message_start","message":{"id":"msg_ping_stall"}})),
        frame(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_unfinished","name":"write","input":{}}}),
        ),
        frame(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"never-written.txt\",\"content\":\"unsafe\"}"}}),
        ),
    ];
    for _ in 0..30 {
        frames.push(json!({"sleep_ms":100}));
        frames.push(frame(json!({"type":"ping"})));
    }
    let up = upstream(json!({"responses":[{"events":frames,"end":"hang"}]})).await;
    let out = output(env.cmd(
        &up.base_url(),
        &["--api", "anthropic-messages", "--tools", "write", "--stream-idle-timeout", "0.25", "Write now"],
    ))
    .await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Anthropic stream stalled"), "{stderr}");
    assert!(!env.work.path().join("never-written.txt").exists());
    assert_eq!(up.served(), 1);
    let entries = journal(&env.session_files()[0]);
    assert!(entries.iter().all(|entry| entry["message"]["role"] != "toolResult"));
}

#[tokio::test]
async fn anthropic_official_key_is_not_sent_to_a_custom_endpoint() {
    let env = Env::new();
    let up = upstream(json!({"responses":[{"status":401,"body":"{\"error\":{\"message\":\"stop\"}}"}]})).await;
    let mut command = env.cmd(&up.base_url(), &["--api", "anthropic-messages", "hi"]);
    command.env("ANTHROPIC_API_KEY", "official-secret-dont-send").env("ARA_API_KEY", "local-key");
    let out = output(command).await;
    assert_eq!(out.status.code(), Some(1));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["headers"]["authorization"], "<redacted 16 chars>");
    assert!(requests[0]["headers"].get("x-api-key").is_none());
    assert!(!requests[0].to_string().contains("official-secret-dont-send"));
}

#[tokio::test]
async fn anthropic_incomplete_tool_does_not_write_a_file() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_partial"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_partial","name":"write","input":{}}})),
        frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"should-not-exist.txt\",\"content\":\"unsafe\"}"}}))
    ]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["--api", "anthropic-messages", "--tools", "write", "write now"])).await;
    assert_eq!(out.status.code(), Some(1), "{}", text_of(&out).1);
    assert!(!env.work.path().join("should-not-exist.txt").exists());
    assert_eq!(up.served(), 1);
    let entries = journal(&env.session_files()[0]);
    assert!(entries.iter().all(|entry| entry["message"]["role"] != "toolResult"));
}

#[tokio::test]
async fn responses_route_runs_a_real_tool_and_replays_it_after_host_restart() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_write", "call_id": "call_write", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{\"path\":\"response.txt\",\"content\":\"from responses\\n\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_write", "call_id": "call_write", "name": "write", "arguments": "{}"}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_done", "content": [{"type": "output_text", "text": "Wrote response.txt."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_resume", "content": [{"type": "output_text", "text": "Session resumed."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let first =
        output(env.cmd(&up.base_url(), &["--api", "openai-responses", "--tools", "write", "Write response.txt"])).await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote response.txt.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("response.txt")).unwrap(), "from responses\n");
    let session = env.session_files();
    assert_eq!(session.len(), 1);
    let entries = journal(&session[0]);
    assert!(entries.iter().any(|entry| {
        entry["message"]["role"] == "assistant"
            && entry["message"]["content"]
                .as_array()
                .is_some_and(|content| content.iter().any(|block| block["type"] == "toolCall"))
    }));
    assert!(entries.iter().any(|entry| entry["message"]["role"] == "toolResult"));
    let resumed = output(
        env.cmd(&up.base_url(), &["--api", "openai-responses", "--resume", session[0].to_str().unwrap(), "Continue"]),
    )
    .await;
    let (stdout, stderr) = text_of(&resumed);
    assert_eq!(resumed.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Session resumed.\n");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["request"], "POST /v1/responses HTTP/1.1");
        assert_eq!(request["body"]["store"], false);
        assert!(request["body"].get("previous_response_id").is_none());
    }
    let input = requests[2]["body"]["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_write"));
    assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_write"));
}

#[tokio::test]
async fn responses_tool_history_replays_to_anthropic_after_cli_restart() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }

    let env = Env::new();
    let responses = upstream(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_write","call_id":"call_write","name":"write"}}},
            {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"cross-provider.txt\",\"content\":\"written once\\n\"}"}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_write","call_id":"call_write","name":"write","arguments":"{}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_written","content":[{"type":"output_text","text":"Written once."}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})).await;
    let first = output(env.cmd(
        &responses.base_url(),
        &["--api", "openai-responses", "--tools", "write", "Write cross-provider.txt once"],
    ))
    .await;
    assert_eq!(first.status.code(), Some(0), "{}", text_of(&first).1);
    assert_eq!(text_of(&first).0, "Written once.\n");
    let path = env.work.path().join("cross-provider.txt");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "written once\n");
    let sessions = env.session_files();
    assert_eq!(sessions.len(), 1);
    let before = journal(&sessions[0]);
    let raw_call = before
        .iter()
        .find_map(|entry| {
            entry["message"]["content"]
                .as_array()?
                .iter()
                .find(|block| block["type"] == "toolCall")?
                .get("id")?
                .as_str()
        })
        .unwrap()
        .to_owned();
    let raw_result =
        before
            .iter()
            .find_map(|entry| {
                if entry["message"]["role"] == "toolResult" { entry["message"]["toolCallId"].as_str() } else { None }
            })
            .unwrap()
            .to_owned();
    assert_eq!(raw_call, "call_write|fc_write");
    assert_eq!(raw_result, "call_write|fc_write");
    std::fs::write(&path, "sentinel after first process\n").unwrap();

    let anthropic = upstream(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_continue"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"The file was written once."}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        frame(json!({"type":"message_stop"}))
    ]}]})).await;
    let resumed = output(env.cmd(
        &anthropic.base_url(),
        &["--api", "anthropic-messages", "--resume", sessions[0].to_str().unwrap(), "Confirm the prior write"],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    assert_eq!(text_of(&resumed).0, "The file was written once.\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "sentinel after first process\n");
    assert_eq!(anthropic.served(), 1);
    let requests = anthropic.requests.lock().await;
    let messages = requests[0]["body"]["messages"].as_array().unwrap();
    let call = messages
        .iter()
        .find_map(|message| message["content"].as_array()?.iter().find(|block| block["type"] == "tool_use"))
        .unwrap();
    let result = messages
        .iter()
        .find_map(|message| message["content"].as_array()?.iter().find(|block| block["type"] == "tool_result"))
        .unwrap();
    let wire_id = call["id"].as_str().unwrap();
    assert!(!wire_id.contains('|') && !wire_id.is_empty() && wire_id.len() <= 64);
    assert_eq!(result["tool_use_id"], wire_id);
    assert!(result["content"].to_string().contains("cross-provider.txt"));
    drop(requests);
    let after = journal(&sessions[0]);
    assert_eq!(&after[..before.len()], before.as_slice(), "resume must append without rewriting raw history");
    assert_eq!(after.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    assert!(after.iter().any(|entry| {
        entry["message"]["content"]
            .as_array()
            .is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "toolCall" && block["id"] == raw_call))
    }));
    assert!(after.iter().any(|entry| entry["message"]["toolCallId"] == raw_result));
}

#[tokio::test]
async fn responses_stateful_chains_tool_result_and_resumes_with_full_history() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_stateful", "call_id": "call_stateful", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{\"path\":\"stateful.txt\",\"content\":\"once\\n\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_stateful", "call_id": "call_stateful", "name": "write", "arguments": "{}"}}},
            {"data": {"type": "response.completed", "response": {"id": "resp_tool", "status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_done", "content": [{"type": "output_text", "text": "Wrote stateful.txt."}]}}},
            {"data": {"type": "response.completed", "response": {"id": "resp_done", "status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_resume", "content": [{"type": "output_text", "text": "Session resumed."}]}}},
            {"data": {"type": "response.completed", "response": {"id": "resp_resume", "status": "completed"}}}
        ]}
    ]})).await;
    let first = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--responses-stateful", "--tools", "write", "Write stateful.txt"],
    ))
    .await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote stateful.txt.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("stateful.txt")).unwrap(), "once\n");
    let session = env.session_files();
    assert_eq!(session.len(), 1);
    let resumed = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--responses-stateful", "--resume", session[0].to_str().unwrap(), "Continue"],
    ))
    .await;
    let (stdout, stderr) = text_of(&resumed);
    assert_eq!(resumed.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Session resumed.\n");

    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request["request"] == "POST /v1/responses HTTP/1.1"));
    assert!(requests.iter().all(|request| request["body"]["store"] == true));
    assert!(requests[0]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_tool");
    let delta = requests[1]["body"]["input"].as_array().unwrap();
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0]["type"], "function_call_output");
    assert_eq!(delta[0]["call_id"], "call_stateful");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    let full = requests[2]["body"]["input"].as_array().unwrap();
    assert!(full.iter().any(|item| item["role"] == "user"
        && item["content"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| part["text"].as_str().is_some_and(|text| text.ends_with("Write stateful.txt")))
        })));
    assert!(full.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_stateful"));
    assert!(full.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_stateful"));
    assert!(full.iter().any(|item| {
        item["type"] == "message"
            && item["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["text"] == "Wrote stateful.txt."))
    }));
    assert!(full.iter().any(|item| item["role"] == "user"
        && item["content"].as_array().is_some_and(|parts| parts.iter().any(|part| part["text"] == "Continue"))));
    drop(requests);
    let entries = journal(&session[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("stateful.txt")).unwrap(), "once\n");
}

#[tokio::test]
async fn responses_stateful_code_only_rejection_replays_full_history_without_repeating_a_tool() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_once", "call_id": "call_once", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{\"path\":\"coded-chain.txt\",\"content\":\"once\\n\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_once", "call_id": "call_once", "name": "write", "arguments": "{}"}}},
            {"data": {"type": "response.completed", "response": {"id": "resp_once", "status": "completed"}}}
        ]},
        {"status": 400, "body": "{\"error\":{\"code\":\"previous_response_not_found\",\"message\":\"lookup failed\"}}"},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_done", "content": [{"type": "output_text", "text": "Recovered."}]}}},
            {"data": {"type": "response.completed", "response": {"id": "resp_recovered", "status": "completed"}}}
        ]}
    ]})).await;
    let out = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--responses-stateful", "--tools", "write", "Write coded-chain.txt"],
    ))
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Recovered.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("coded-chain.txt")).unwrap(), "once\n");
    let session = env.session_files();
    assert_eq!(session.len(), 1);
    assert_eq!(journal(&session[0]).iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests[0]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_once");
    let delta = requests[1]["body"]["input"].as_array().unwrap();
    assert_eq!(delta.len(), 1);
    assert_eq!(delta[0]["type"], "function_call_output");
    assert_eq!(delta[0]["call_id"], "call_once");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["store"], true);
    let full = requests[2]["body"]["input"].as_array().unwrap();
    assert!(full.iter().any(|item| item["role"] == "user"));
    assert!(full.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_once"));
    assert!(full.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_once"));
}

#[tokio::test]
async fn responses_reasoning_history_survives_tool_turn_and_restart_without_json_disclosure() {
    let env = Env::new();
    let secret = "opaque-reasoning-e2e-marker";
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "reasoning", "id": "rs_1"}}},
            {"data": {"type": "response.reasoning_summary_text.delta", "output_index": 0, "summary_index": 0, "delta": "Need a file"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "Need a file"}], "encrypted_content": secret}}},
            {"data": {"type": "response.output_item.done", "output_index": 1, "item": {"type": "message", "id": "msg_phase", "phase": "commentary", "content": [{"type": "output_text", "text": "Writing it."}]}}},
            {"data": {"type": "response.output_item.added", "output_index": 2, "item": {"type": "function_call", "id": "fc_reason", "call_id": "call_reason", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.done", "output_index": 2, "arguments": "{\"path\":\"reason.txt\",\"content\":\"ok\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 2, "item": {"type": "function_call", "id": "fc_reason", "call_id": "call_reason", "name": "write", "arguments": "{}"}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_final", "content": [{"type": "output_text", "text": "File ready."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_resume", "content": [{"type": "output_text", "text": "Continued."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let first = output(env.cmd(
        &up.base_url(),
        &["--mode", "json", "--api", "openai-responses", "--reasoning", "--tools", "write", "Create reason.txt"],
    ))
    .await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert!(!stdout.contains(secret) && !stdout.contains("providerPayload") && !stdout.contains("thinkingSignature"));
    assert_eq!(std::fs::read_to_string(env.work.path().join("reason.txt")).unwrap(), "ok");
    let session = env.session_files();
    let entries = journal(&session[0]);
    let reasoning_turn = entries
        .iter()
        .find(|entry| entry["message"]["providerPayload"]["items"].as_array().is_some_and(|items| items.len() == 3))
        .unwrap();
    assert_eq!(reasoning_turn["message"]["content"][0]["thinking"], "Need a file");
    assert_eq!(reasoning_turn["message"]["providerPayload"]["items"][0]["encrypted_content"], secret);
    assert_eq!(reasoning_turn["message"]["providerPayload"]["dt"], true);
    let resumed = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--reasoning", "--resume", session[0].to_str().unwrap(), "Continue"],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        assert_eq!(request["body"]["include"], json!(["reasoning.encrypted_content"]));
    }
    let warm_input = requests[1]["body"]["input"].as_array().unwrap();
    let pos =
        warm_input.iter().position(|item| item["type"] == "reasoning" && item["encrypted_content"] == secret).unwrap();
    assert_eq!(warm_input[pos + 1]["phase"], "commentary");
    assert_eq!(warm_input[pos + 2]["call_id"], "call_reason");
    assert_eq!(warm_input[pos + 3]["type"], "function_call_output");
    assert_eq!(warm_input[pos + 3]["call_id"], "call_reason");
    let cold_input = requests[2]["body"]["input"].as_array().unwrap();
    assert!(cold_input.iter().all(|item| item["type"] != "reasoning"));
    assert!(cold_input.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "Writing it."));
    assert!(cold_input.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_reason"));
    assert!(cold_input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == "call_reason"));
}

#[tokio::test]
async fn responses_incomplete_reasoning_history_warms_in_process_and_resumes_cold() {
    let env = Env::new();
    let secret = "opaque-incomplete-cli-marker";
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "reasoning", "id": "rs_partial", "encrypted_content": secret}}},
            {"data": {"type": "response.output_item.done", "output_index": 1, "item": {"type": "message", "id": "msg_partial", "content": [{"type": "output_text", "text": "Partial answer."}]}}},
            {"data": {"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_continue", "content": [{"type": "output_text", "text": "Continued."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_resume", "content": [{"type": "output_text", "text": "Resumed."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let first =
        output(env.cmd(&up.base_url(), &["--api", "openai-responses", "--reasoning", "First", "Continue"])).await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Continued.\n");
    assert!(!stdout.contains(secret));
    let session = env.session_files();
    assert_eq!(session.len(), 1);
    let entries = journal(&session[0]);
    assert!(entries.iter().any(|entry| entry["message"]["stopReason"] == "length"
        && entry["message"]["providerPayload"]["items"][0]["encrypted_content"] == secret));
    let resumed = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--reasoning", "--resume", session[0].to_str().unwrap(), "Again"],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]["body"]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "reasoning" && item["encrypted_content"] == secret)
    );
    let cold_input = requests[2]["body"]["input"].as_array().unwrap();
    assert!(cold_input.iter().all(|item| item["type"] != "reasoning"));
    assert!(cold_input.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "Partial answer."));
}

#[tokio::test]
async fn responses_identifierless_parallel_calls_write_distinct_files() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write"}}},
            {"data": {"type": "response.output_item.added", "output_index": 1, "item": {"type": "function_call", "id": "fc_b", "call_id": "call_b", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.delta", "delta": "{\"path\":\"a.txt\",\"content\":\"alpha\"}"}},
            {"data": {"type": "response.function_call_arguments.delta", "delta": "{\"path\":\"b.txt\","}},
            {"data": {"type": "response.function_call_arguments.delta", "delta": "\"content\":\"beta\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write", "arguments": ""}}},
            {"data": {"type": "response.output_item.done", "output_index": 1, "item": {"type": "function_call", "id": "fc_b", "call_id": "call_b", "name": "write", "arguments": ""}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_done", "content": [{"type": "output_text", "text": "Both files written."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let out =
        output(env.cmd(&up.base_url(), &["--api", "openai-responses", "--tools", "write", "Write a.txt and b.txt"]))
            .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Both files written.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("a.txt")).unwrap(), "alpha");
    assert_eq!(std::fs::read_to_string(env.work.path().join("b.txt")).unwrap(), "beta");
    let entries = journal(&env.session_files()[0]);
    let assistant = entries.iter().find(|entry| entry["message"]["role"] == "assistant").unwrap();
    let calls = assistant["message"]["content"].as_array().unwrap();
    assert_eq!(calls[0]["id"], "call_a|fc_a");
    assert_eq!(calls[0]["arguments"]["path"], "a.txt");
    assert_eq!(calls[1]["id"], "call_b|fc_b");
    assert_eq!(calls[1]["arguments"]["path"], "b.txt");
    let requests = up.requests.lock().await;
    let input = requests[1]["body"]["input"].as_array().unwrap();
    for call_id in ["call_a", "call_b"] {
        assert!(input.iter().any(|item| item["type"] == "function_call_output" && item["call_id"] == call_id));
    }
}

#[tokio::test]
async fn responses_identifierless_conflict_does_not_execute_either_write() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write"}}},
        {"data": {"type": "response.output_item.added", "output_index": 1, "item": {"type": "function_call", "id": "fc_b", "call_id": "call_b", "name": "write"}}},
        {"data": {"type": "response.function_call_arguments.done", "arguments": "{\"path\":\"b.txt\",\"content\":\"wrong\"}"}},
        {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write", "arguments": "{\"path\":\"a.txt\",\"content\":\"right\"}"}}},
        {"data": {"type": "response.completed", "response": {"status": "completed"}}}
    ]}]})).await;
    let out =
        output(env.cmd(&up.base_url(), &["--api", "openai-responses", "--tools", "write", "Write two files"])).await;
    assert_ne!(out.status.code(), Some(0));
    assert!(!env.work.path().join("a.txt").exists());
    assert!(!env.work.path().join("b.txt").exists());
    assert_eq!(up.served(), 1);
}

#[tokio::test]
async fn responses_truncated_parallel_call_does_not_execute_either_write() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.done", "output_index": 0, "arguments": "{\"path\":\"first.txt\",\"content\":\"unsafe\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "function_call", "id": "fc_a", "call_id": "call_a", "name": "write", "arguments": ""}}},
            {"data": {"type": "response.output_item.added", "output_index": 1, "item": {"type": "function_call", "id": "fc_b", "call_id": "call_b", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.delta", "output_index": 1, "delta": "{\"path\":\"second.txt\","}},
            {"data": {"type": "response.incomplete", "response": {"incomplete_details": {"reason": "max_output_tokens"}}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_recovered", "content": [{"type": "output_text", "text": "No files were written."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let first = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--tools", "write", "--max-model-calls", "1", "Write two files"],
    ))
    .await;
    assert_eq!(first.status.code(), Some(1));
    assert!(!env.work.path().join("first.txt").exists());
    assert!(!env.work.path().join("second.txt").exists());
    assert_eq!(up.served(), 1);
    let session = env.session_files();
    assert_eq!(session.len(), 1);
    let entries = journal(&session[0]);
    let assistant = entries.iter().find(|entry| entry["message"]["role"] == "assistant").unwrap();
    assert_eq!(assistant["message"]["stopReason"], json!("length"));
    let calls = assistant["message"]["content"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["id"], json!("call_a|fc_a"));
    assert_eq!(calls[1]["id"], json!("call_b|fc_b"));
    assert_eq!(calls[1]["arguments"]["__rawJson"], json!("{\"path\":\"second.txt\","));
    assert!(calls[1]["arguments"].get("__parseError").is_some());
    let results = entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    for (result, id) in results.iter().zip(["call_a|fc_a", "call_b|fc_b"]) {
        assert_eq!(result["message"]["toolCallId"], json!(id));
        assert_eq!(result["message"]["details"]["source"], json!("assistant_stop_length"));
        assert_eq!(result["message"]["details"]["executed"], json!(false));
    }
    let resumed = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--tools", "write", "--resume", session[0].to_str().unwrap(), "Continue safely"],
    ))
    .await;
    let (stdout, stderr) = text_of(&resumed);
    assert_eq!(resumed.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "No files were written.\n");
    assert!(!env.work.path().join("first.txt").exists());
    assert!(!env.work.path().join("second.txt").exists());
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let input = requests[1]["body"]["input"].as_array().unwrap();
    for (call_id, path) in [("call_a", "first.txt"), ("call_b", "second.txt")] {
        let call = input.iter().find(|item| item["type"] == "function_call" && item["call_id"] == call_id).unwrap();
        assert!(call["arguments"].as_str().unwrap().contains(path));
        let result =
            input.iter().find(|item| item["type"] == "function_call_output" && item["call_id"] == call_id).unwrap();
        assert!(result["output"].as_str().unwrap().contains("not executed"));
    }
}

#[tokio::test]
async fn responses_truncated_call_resamples_without_executing_it() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_partial", "call_id": "call_partial", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.delta", "output_index": 0, "delta": "{\"path\":\"never.txt\","}},
            {"data": {"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_safe", "content": [{"type": "output_text", "text": "The partial write was skipped."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let out =
        output(env.cmd(&up.base_url(), &["--api", "openai-responses", "--tools", "write", "Write never.txt"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "The partial write was skipped.\n");
    assert!(!env.work.path().join("never.txt").exists());
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let input = requests[1]["body"]["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["type"] == "function_call" && item["call_id"] == "call_partial"));
    assert!(input.iter().any(|item| item["type"] == "function_call_output"
        && item["call_id"] == "call_partial"
        && item["output"].as_str().unwrap().contains("not executed")));
}

#[tokio::test]
async fn responses_mixed_partial_call_replays_native_text_without_replaying_the_call() {
    let env = Env::new();
    let secret = "mixed-partial-opaque-marker";
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "reasoning", "id": "rs_mixed", "encrypted_content": secret}}},
            {"data": {"type": "response.output_item.done", "output_index": 1,
                "item": {"type": "message", "id": "msg_mixed",
                    "content": [{"type": "output_text", "text": "Planning the write."}]}}},
            {"data": {"type": "response.output_item.added", "output_index": 2,
                "item": {"type": "function_call", "id": "fc_partial", "call_id": "call_partial", "name": "write"}}},
            {"data": {"type": "response.function_call_arguments.delta", "output_index": 2,
                "delta": "{\"path\":\"never.txt\","}},
            {"data": {"type": "response.incomplete", "response": {"status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "message", "content": [{"type": "output_text", "text": "The partial write was skipped."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]},
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0,
                "item": {"type": "message", "content": [{"type": "output_text", "text": "Resumed."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]})).await;
    let first = output(
        env.cmd(&up.base_url(), &["--api", "openai-responses", "--reasoning", "--tools", "write", "Write never.txt"]),
    )
    .await;
    let (stdout, stderr) = text_of(&first);
    assert_eq!(first.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "The partial write was skipped.\n");
    assert!(!env.work.path().join("never.txt").exists());
    assert!(!stdout.contains(secret));
    let sessions = env.session_files();
    assert_eq!(sessions.len(), 1);
    let entries = journal(&sessions[0]);
    assert!(entries.iter().any(|entry| entry["message"]["stopReason"] == "length"
        && entry["message"]["providerPayload"]["items"][0]["encrypted_content"] == secret));
    assert!(entries.iter().any(|entry| entry["message"]["role"] == "toolResult"
        && entry["message"]["content"][0]["text"].as_str().unwrap_or("").contains("not executed")));
    let resumed = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "openai-responses",
            "--reasoning",
            "--tools",
            "write",
            "--resume",
            sessions[0].to_str().unwrap(),
            "Continue",
        ],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    assert!(!env.work.path().join("never.txt").exists());
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let warm = requests[1]["body"]["input"].as_array().unwrap();
    assert!(
        warm.iter().any(|item| item["type"] == "reasoning" && item["encrypted_content"] == secret),
        "warm={warm:#?} entries={entries:#?}"
    );
    assert!(warm.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "Planning the write."));
    assert!(warm.iter().any(|item| item["type"] == "message"
        && item["role"] == "assistant"
        && item["content"].as_str().unwrap_or("").contains("[Orphan tool result; call_id=call_partial]")));
    assert!(warm.iter().all(|item| item["type"] != "function_call" && item["type"] != "function_call_output"));
    assert!(requests[2]["body"]["input"].as_array().unwrap().iter().all(|item| item["type"] != "reasoning"));
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
    // Hashline mode (the default edit mode) returns a fresh snapshot header.
    let tag = ara_edit::store::file_hash("hi from ara\n");
    assert_eq!(
        entries[4]["message"]["content"][0]["text"],
        json!(format!("[hello.txt#{tag}]\nSuccessfully wrote 12 bytes to hello.txt"))
    );
    assert_eq!(entries[6]["message"]["content"][0]["text"], json!("hi from ara\ndone"));
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 3);
    let last = reqs[2]["body"]["messages"].as_array().unwrap();
    assert!(
        last.iter().any(|m| m == &json!({"role": "tool", "content": "hi from ara\ndone", "tool_call_id": "call_b"}))
    );
}

#[tokio::test]
async fn object_streamed_write_arguments_reach_tool_and_journal_once() {
    let env = Env::new();
    let tool_frame = |arguments: Value| {
        json!({"data": {"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "call_object_write", "function": {"name": "write", "arguments": arguments}}
        ]}}]}})
    };
    let up = upstream(json!({"responses": [
        {"events": [
            tool_frame(json!({"path": "fragment.txt", "content": "hello "})),
            tool_frame(json!({"content": "world\n"})),
            finish("tool_calls"), done()
        ]},
        {"events": [text("Wrote fragment.txt."), finish("stop"), done()]}
    ]}))
    .await;
    let out =
        output(env.cmd(&up.base_url(), &["--api", "openai-completions", "--tools", "write", "Write fragment.txt"]))
            .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote fragment.txt.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("fragment.txt")).unwrap(), "hello world\n");
    assert_eq!(up.served(), 2);
    let entries = journal(&env.session_files()[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert_eq!(entries[3]["message"]["content"][0]["arguments"]["content"], json!("hello world\n"));
    assert_eq!(entries[4]["message"]["toolCallId"], json!("call_object_write"));
}

#[tokio::test]
async fn required_chat_reasoning_replays_tool_and_text_turns_after_restart() {
    let env = Env::new();
    let reasoning = json!({"data": {"choices": [{"delta": {"reasoning_content": "write the file once"}}]}});
    let up = upstream(json!({"responses": [
        {"events": [
            reasoning,
            tool_call(0, "call_reason_write", "write", "{\"path\":\"reason.txt\",\"content\":\"once\\n\"}"),
            finish("tool_calls"), done()
        ]},
        {"events": [text("Written."), finish("stop"), done()]},
        {"events": [text("Still there."), finish("stop"), done()]},
        {"events": [text("Done."), finish("stop"), done()]}
    ]}))
    .await;
    let first = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "openai-completions",
            "--chat-replay-reasoning-content",
            "--tools",
            "write",
            "Write reason.txt once",
        ],
    ))
    .await;
    assert_eq!(first.status.code(), Some(0), "{}", text_of(&first).1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("reason.txt")).unwrap(), "once\n");
    let session = env.session_files().pop().unwrap();
    let before = std::fs::read(&session).unwrap();
    let entries = journal(&session);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert_eq!(entries[3]["message"]["content"][0]["type"], "thinking");
    assert_eq!(entries[3]["message"]["content"][0]["thinking"], "write the file once");
    assert_eq!(entries[3]["message"]["content"][0]["thinkingSignature"], "reasoning_content");
    std::fs::write(env.work.path().join("reason.txt"), "sentinel\n").unwrap();

    let resumed = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "openai-completions",
            "--chat-replay-reasoning-content",
            "--resume",
            session.to_str().unwrap(),
            "Check",
        ],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    assert!(std::fs::read(&session).unwrap().starts_with(&before), "resume appended to the same journal");
    assert_eq!(std::fs::read_to_string(env.work.path().join("reason.txt")).unwrap(), "sentinel\n");
    let plain = output(
        env.cmd(&up.base_url(), &["--api", "openai-completions", "--resume", session.to_str().unwrap(), "Finish"]),
    )
    .await;
    assert_eq!(plain.status.code(), Some(0), "{}", text_of(&plain).1);

    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 4);
    let second = reqs[1]["body"]["messages"].as_array().unwrap();
    let tool_turn = second.iter().find(|m| m["role"] == "assistant").unwrap();
    assert_eq!(tool_turn["reasoning_content"], "write the file once");
    assert_eq!(tool_turn["content"], "");
    assert_eq!(tool_turn["tool_calls"][0]["id"], "call_reason_write");
    assert!(second.iter().any(|m| m["role"] == "tool" && m["tool_call_id"] == "call_reason_write"));
    let third: Vec<_> =
        reqs[2]["body"]["messages"].as_array().unwrap().iter().filter(|m| m["role"] == "assistant").collect();
    assert_eq!(third.len(), 2);
    assert_eq!(third[0]["reasoning_content"], "write the file once");
    assert_eq!(third[1]["content"], "Written.");
    assert_eq!(third[1]["reasoning_content"], "");
    assert!(reqs[3]["body"]["messages"].as_array().unwrap().iter().all(|m| m.get("reasoning_content").is_none()));
    assert_eq!(up.served(), 4, "replay did not repeat the write tool");
}

#[tokio::test]
async fn explicit_mistral_chat_profile_replays_one_tool_effect_after_restart() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            tool_call(0, "call-write-long", "write", "{\"path\":\"mistral.txt\",\"content\":\"once\\n\"}"),
            finish("tool_calls"), done()
        ]},
        {"events": [text("The file is present."), finish("stop"), done()]}
    ]}))
    .await;
    let first = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "openai-completions",
            "--chat-mistral-compat",
            "--tools",
            "write",
            "--max-model-calls",
            "1",
            "Write once",
        ],
    ))
    .await;
    assert_eq!(first.status.code(), Some(1), "{}", text_of(&first).1);
    assert_eq!(std::fs::read_to_string(env.work.path().join("mistral.txt")).unwrap(), "once\n");
    let session = env.session_files().pop().unwrap();
    let before = std::fs::read(&session).unwrap();
    let entries = journal(&session);
    let assistant = entries.iter().find(|entry| entry["message"]["role"] == "assistant").unwrap();
    assert_eq!(assistant["message"]["content"][0]["id"], "call-write-long");
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
    std::fs::write(env.work.path().join("mistral.txt"), "sentinel\n").unwrap();

    let resumed = output(env.cmd(
        &up.base_url(),
        &[
            "--api",
            "openai-completions",
            "--chat-mistral-compat",
            "--resume",
            session.to_str().unwrap(),
            "Check the result",
        ],
    ))
    .await;
    assert_eq!(resumed.status.code(), Some(0), "{}", text_of(&resumed).1);
    assert!(std::fs::read(&session).unwrap().starts_with(&before));
    assert_eq!(std::fs::read_to_string(env.work.path().join("mistral.txt")).unwrap(), "sentinel\n");
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["body"]["messages"].as_array().unwrap();
    let tool_assistant = messages.iter().find(|message| message["tool_calls"].is_array()).unwrap();
    let wire_id = tool_assistant["tool_calls"][0]["id"].as_str().unwrap();
    assert_eq!(wire_id.len(), 9);
    assert!(wire_id.chars().all(|character| character.is_ascii_alphanumeric()));
    let result_index = messages.iter().position(|message| message["role"] == "tool").unwrap();
    assert_eq!(messages[result_index]["tool_call_id"], wire_id);
    assert_eq!(messages[result_index]["name"], "write");
    assert_eq!(messages[result_index + 1]["role"], "assistant");
    assert_eq!(messages[result_index + 1]["content"], "I have processed the tool results.");
    assert_eq!(messages[result_index + 2]["role"], "user");
    assert_eq!(up.served(), 2);
}

#[tokio::test]
async fn mistral_chat_profile_rejects_other_protocol_and_conflicting_reasoning_mode() {
    let env = Env::new();
    let wrong_api =
        output(env.cmd("http://127.0.0.1:1/v1", &["--api", "openai-responses", "--chat-mistral-compat", "hello"]))
            .await;
    assert_eq!(wrong_api.status.code(), Some(2));
    assert!(text_of(&wrong_api).1.contains("--chat-mistral-compat requires --api openai-completions"));
    let conflicting = output(
        env.cmd("http://127.0.0.1:1/v1", &["--chat-mistral-compat", "--chat-replay-reasoning-content", "hello"]),
    )
    .await;
    assert_eq!(conflicting.status.code(), Some(2));
    assert!(text_of(&conflicting).1.contains("cannot be used with"));
    assert!(env.session_files().is_empty());
}

#[tokio::test]
async fn empty_stream_retry_reaches_one_tool_task_without_duplicate_effects() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [finish("stop"), usage(4, 0), done()]},
        {"events": [tool_call(0, "call_w", "write", "{\"path\":\"retry.txt\",\"content\":\"once\\n\"}"), finish("tool_calls"), done()]},
        {"events": [text("Wrote retry.txt once."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Write retry.txt with once"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote retry.txt once.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("retry.txt")).unwrap(), "once\n");
    assert_eq!(up.served(), 3, "one empty retry, one tool turn, one final turn");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert_eq!(entries[3]["message"]["content"][0]["id"], json!("call_w"));
    assert_eq!(entries[4]["message"]["toolCallId"], json!("call_w"));
}

#[tokio::test]
async fn retried_usage_reaches_json_and_journal_with_one_tool_effect() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [finish("stop"), usage(4, 0), done()]},
        {"events": [tool_call(0, "call_w", "write", "{\"path\":\"retry-accounting.txt\",\"content\":\"once\\n\"}"), finish("tool_calls"), usage(5, 2), done()]},
        {"events": [text("Written once."), finish("stop"), usage(6, 3), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--mode", "json", "Write retry-accounting.txt with once"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(std::fs::read_to_string(env.work.path().join("retry-accounting.txt")).unwrap(), "once\n");
    assert_eq!(up.served(), 3);
    let events: Vec<Value> = stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let assistant_ends: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "message_end" && event["message"]["role"] == "assistant")
        .collect();
    assert_eq!(assistant_ends.len(), 2);
    let receipt = &assistant_ends[0]["message"]["retryAccounting"];
    assert_eq!(assistant_ends[0]["message"]["usage"]["input"], 5);
    assert_eq!(receipt["attempts"].as_array().unwrap().len(), 2);
    assert_eq!(receipt["attempts"][0]["usage"]["input"], 4);
    assert_eq!(receipt["attempts"][1]["usage"]["input"], 5);
    assert_eq!(assistant_ends[1]["message"]["retryAccounting"], Value::Null);
    let entries = journal(&env.session_files()[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert_eq!(entries[3]["message"]["retryAccounting"], *receipt);
    assert_eq!(entries[3]["message"]["content"][0]["id"], "call_w");
    assert_eq!(entries[4]["message"]["toolCallId"], "call_w");
}

#[tokio::test]
async fn pre_start_http_retry_reaches_one_tool_task_without_duplicate_effects() {
    let env = Env::new();
    let mut responses = vec![
        json!({
            "status": 503,
            "headers": {"retry-after": "0"},
            "body": "{\"error\":{\"message\":\"temporary failure\"}}"
        });
        6
    ];
    responses.push(json!({"events": [
        tool_call(0, "call_w", "write", "{\"path\":\"retry.txt\",\"content\":\"once\\n\"}"),
        finish("tool_calls"), done()
    ]}));
    responses.push(json!({"events": [text("Wrote retry.txt once."), finish("stop"), done()]}));
    let up = upstream(json!({"responses": responses})).await;

    let out = output(env.cmd(&up.base_url(), &["Write retry.txt with once"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Wrote retry.txt once.\n");
    assert_eq!(std::fs::read_to_string(env.work.path().join("retry.txt")).unwrap(), "once\n");
    assert_eq!(up.served(), 8, "six inner attempts, one replay with a tool turn, one final turn");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "assistant"]);
    assert_eq!(entries[3]["message"]["content"][0]["id"], json!("call_w"));
    assert_eq!(entries[4]["message"]["toolCallId"], json!("call_w"));
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

#[tokio::test(flavor = "multi_thread")]
async fn responses_json_mode_emits_text_before_terminal() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg_live"}}},
        {"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "msg_live", "delta": "first "}},
        {"sleep_ms": 1500},
        {"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "msg_live", "delta": "second"}},
        {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_live", "content": [{"type": "output_text", "text": "first second"}]}}},
        {"data": {"type": "response.completed", "response": {"status": "completed"}}}
    ]}]})).await;
    let mut command = env.cmd(&up.base_url(), &["--api", "openai-responses", "stream please"]);
    let mut child = spawn_json(&mut command);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let seen = tokio::task::block_in_place(|| {
        read_until(
            &mut reader,
            |event| event["type"] == "message_update" && event["assistantMessageEvent"]["delta"] == "first ",
            Duration::from_secs(10),
        )
    });
    assert!(seen.iter().any(|event| event["assistantMessageEvent"]["delta"] == "first "));
    let start_index = seen.iter().position(|event| event["type"] == "message_start").unwrap();
    let delta_index = seen.iter().position(|event| event["assistantMessageEvent"]["delta"] == "first ").unwrap();
    assert!(start_index < delta_index);
    assert!(child.try_wait().unwrap().is_none(), "Responses delta reached the client before the terminal frame");
    let rest: Vec<Value> = reader.lines().map(|line| serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    assert_eq!(rest.iter().filter(|event| event["type"] == "agent_end").count(), 1);
    assert_eq!(
        rest.iter().filter(|event| event["type"] == "message_end" && event["message"]["role"] == "assistant").count(),
        1
    );
    let end =
        rest.iter().find(|event| event["type"] == "message_end" && event["message"]["role"] == "assistant").unwrap();
    assert_eq!(end["message"]["content"][0]["text"], json!("first second"));
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_json_mode_emits_text_before_terminal() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[{"events":[
        frame(json!({"type":"message_start","message":{"id":"msg_live"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"first "}})),
        {"sleep_ms":1500},
        frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"second"}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        frame(json!({"type":"message_stop"}))
    ]}]}))
    .await;
    let mut command = env.cmd(&up.base_url(), &["--api", "anthropic-messages", "stream please"]);
    let mut child = spawn_json(&mut command);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let seen = tokio::task::block_in_place(|| {
        read_until(
            &mut reader,
            |event| event["type"] == "message_update" && event["assistantMessageEvent"]["delta"] == "first ",
            Duration::from_secs(10),
        )
    });
    assert!(seen.iter().any(|event| event["assistantMessageEvent"]["delta"] == "first "));
    assert!(child.try_wait().unwrap().is_none(), "Anthropic delta reached the client before the terminal frame");
    let rest: Vec<Value> = reader.lines().map(|line| serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let end =
        rest.iter().find(|event| event["type"] == "message_end" && event["message"]["role"] == "assistant").unwrap();
    assert_eq!(end["message"]["content"][0]["text"], json!("first second"));
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_tool_delta_reaches_json_client_before_execution() {
    fn frame(value: Value) -> Value {
        let name = value["type"].as_str().unwrap();
        json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
    }
    let env = Env::new();
    let up = upstream(json!({"responses":[
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_live_tool"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_live","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"live.txt\",\"content\":\"one\"}"}})),
            {"sleep_ms":800},
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"}))
        ]},
        {"events":[
            frame(json!({"type":"message_start","message":{"id":"msg_live_done"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Done."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"}))
        ]}
    ]})).await;
    let mut command = env.cmd(&up.base_url(), &["--api", "anthropic-messages", "--tools", "write", "Write live.txt"]);
    let mut child = spawn_json(&mut command);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let seen = tokio::task::block_in_place(|| {
        read_until(
            &mut reader,
            |event| event["type"] == "message_update" && event["assistantMessageEvent"]["type"] == "toolcall_delta",
            Duration::from_secs(10),
        )
    });
    assert!(seen.iter().any(|event| event["assistantMessageEvent"]["type"] == "toolcall_start"));
    assert_eq!(seen.last().unwrap()["assistantMessageEvent"]["contentIndex"], json!(0));
    assert!(child.try_wait().unwrap().is_none(), "tool delta reached the client before the response ended");
    assert!(!env.work.path().join("live.txt").exists(), "a partial tool call cannot run");
    let rest: Vec<Value> = reader.lines().map(|line| serde_json::from_str(&line.unwrap()).unwrap()).collect();
    assert_eq!(child.wait().unwrap().code(), Some(0));
    let end_index = rest.iter().position(|event| event["assistantMessageEvent"]["type"] == "toolcall_end").unwrap();
    let execute_index = rest.iter().position(|event| event["type"] == "tool_execution_start").unwrap();
    assert!(end_index < execute_index);
    assert_eq!(std::fs::read_to_string(env.work.path().join("live.txt")).unwrap(), "one");
    assert_eq!(up.served(), 2);
    let entries = journal(&env.session_files()[0]);
    assert_eq!(entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").count(), 1);
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

#[tokio::test]
async fn responses_failed_detail_reaches_cli_and_session_journal() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "partial"}}},
        {"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "partial", "delta": "draft"}},
        {"data": {"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "backend exploded"}}}}
    ]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["--api", "openai-responses", "Explain the failure"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.lines().any(|line| line == "server_error: backend exploded"), "{stderr}");
    assert_eq!(up.served(), 1, "visible draft prevents request replay");
    let entries = journal(&env.session_files()[0]);
    let assistant = &entries.last().unwrap()["message"];
    assert_eq!(assistant["stopReason"], "error");
    assert_eq!(assistant["errorMessage"], "server_error: backend exploded");
    assert_eq!(assistant["content"][0]["text"], "draft");
}

#[cfg(unix)]
fn signal(child: &Child, sig: i32) {
    assert_eq!(unsafe { libc::kill(child.id() as i32, sig) }, 0);
}

#[cfg(unix)]
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

#[cfg(unix)]
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

/// Spawn the REPL with piped stdio. On Windows it gets its own process group
/// so the test can deliver Ctrl+Break to it (Ctrl+C cannot target a group).
fn spawn_repl(c: &mut Command) -> Child {
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP);
    }
    c.spawn().unwrap()
}

/// The REPL's interrupt: SIGINT on Unix, Ctrl+Break on Windows.
fn interrupt(child: &Child) {
    #[cfg(unix)]
    signal(child, libc::SIGINT);
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, GenerateConsoleCtrlEvent};
        let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.id()) };
        assert_ne!(sent, 0, "GenerateConsoleCtrlEvent: {}", std::io::Error::last_os_error());
    }
}

/// Run the REPL to completion on `input` (stdin then closes: EOF).
async fn repl_output(mut c: Command, input: &'static str) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut child = spawn_repl(&mut c);
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap()
}

/// A child's stderr collected on a thread (the `> ` prompt has no newline).
struct StderrLog(std::sync::Arc<std::sync::Mutex<String>>, std::thread::JoinHandle<()>);

impl StderrLog {
    fn start(child: &mut Child) -> StderrLog {
        StderrLog::reading(child.stderr.take().unwrap())
    }

    /// The same collector for any child pipe (stdout without newlines too).
    fn reading(mut err: impl std::io::Read + Send + 'static) -> StderrLog {
        let buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let sink = buf.clone();
        let reader = std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = std::io::Read::read(&mut err, &mut chunk) {
                if n == 0 {
                    break;
                }
                sink.lock().unwrap().push_str(&String::from_utf8_lossy(&chunk[..n]));
            }
        });
        StderrLog(buf, reader)
    }

    fn wait_for(&self, needle: &str, limit: Duration) -> bool {
        let started = Instant::now();
        while started.elapsed() < limit {
            if self.0.lock().unwrap().contains(needle) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn finish(self) -> String {
        self.1.join().unwrap();
        self.0.lock().unwrap().clone()
    }
}

fn wait_exit(child: &mut Child, limit: Duration) -> Option<i32> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status.code();
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("ara did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Text of every user message, from a journal or a Chat Completions request.
fn user_texts<'a>(messages: impl IntoIterator<Item = &'a Value>) -> Vec<String> {
    messages
        .into_iter()
        .filter(|m| m["role"] == "user")
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks.iter().filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join(""),
            other => other.to_string(),
        })
        .collect()
}

fn journal_user_texts(entries: &[Value]) -> Vec<String> {
    user_texts(entries.iter().filter(|e| e["type"] == "message").map(|e| &e["message"]))
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_runs_turns_in_one_journal_and_exits_on_eof() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl"]), "alpha-turn\nbeta-turn\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "First answer.\nSecond answer.\n");
    let files = env.session_files();
    assert_eq!(files.len(), 1, "both turns share one Session");
    let entries = journal(&files[0]);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "user", "assistant"]);
    assert_eq!(journal_user_texts(&entries), vec!["alpha-turn", "beta-turn"]);
    assert!(stderr.contains(&format!("ara: session {}", files[0].display())), "{stderr}");
    let reqs = up.requests.lock().await;
    let second = reqs[1]["body"]["messages"].as_array().unwrap();
    assert!(user_texts(second).iter().any(|t| t.contains("alpha-turn")), "{second:?}");
    assert!(
        second.iter().any(|m| m["role"] == "assistant" && m["content"].to_string().contains("First answer.")),
        "{second:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_help_and_exit_run_no_turn() {
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl"]), "/help\n/exit\nnot sent\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("commands: /help, /new, /compact, /exit"), "{stderr}");
    assert_eq!(up.served(), 0);
    assert!(env.session_files().is_empty(), "no turn, so the lazy Session was never written");
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_new_starts_a_fresh_session_file() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("Answer alpha."), finish("stop"), done()]},
        {"events": [text("Answer beta."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl"]), "alpha-turn\n/new\nbeta-turn\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Answer alpha.\nAnswer beta.\n");
    let files = env.session_files();
    assert_eq!(files.len(), 2, "{files:?}");
    let first = journal(&files[0]);
    let second = journal(&files[1]);
    assert_eq!(journal_user_texts(&first), vec!["alpha-turn"]);
    assert_eq!(roles(&first), vec!["model_change", "user", "assistant"]);
    assert_eq!(journal_user_texts(&second), vec!["beta-turn"]);
    assert_eq!(roles(&second), vec!["model_change", "user", "assistant"]);
    assert!(
        stderr.contains(&format!("ara: new session; the previous one stays at {}", files[0].display())),
        "{stderr}"
    );
    let reqs = up.requests.lock().await;
    let texts = user_texts(reqs[1]["body"]["messages"].as_array().unwrap());
    assert!(texts.iter().all(|t| !t.contains("alpha-turn")), "{texts:?}");
}

#[tokio::test]
async fn repl_rejects_prompt_arguments() {
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["--repl", "some prompt"])).await;
    assert_eq!(out.status.code(), Some(2), "{}", text_of(&out).1);
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_interrupt_during_bash_returns_to_prompt() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_s", "bash", "{\"command\":\"echo started; sleep 30\"}"), finish("tool_calls"), done()]},
        {"events": [text("After the abort."), finish("stop"), done()]},
        {"events": [text("must not be requested"), finish("stop"), done()]}
    ]}))
    .await;
    let mut c = env.cmd(&up.base_url(), &["--repl", "--mode", "json"]);
    let mut child = spawn_repl(&mut c);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let started = Instant::now();
    let (turn1, rest, code) = tokio::task::block_in_place(|| {
        writeln!(stdin, "run something slow").unwrap();
        let mut turn1 = read_until(&mut reader, |v| v["type"] == "tool_execution_update", Duration::from_secs(15));
        assert_eq!(turn1.last().unwrap()["type"], json!("tool_execution_update"), "{turn1:?}");
        interrupt(&child);
        turn1.extend(read_until(&mut reader, |v| v["type"] == "agent_end", Duration::from_secs(10)));
        assert!(log.wait_for("ara: turn 1 cancelled; session kept", Duration::from_secs(5)));
        assert!(child.try_wait().unwrap().is_none(), "the REPL survives a turn interrupt");
        writeln!(stdin, "after").unwrap();
        drop(stdin);
        let rest: Vec<Value> = reader.lines().map(|l| serde_json::from_str(&l.unwrap()).unwrap()).collect();
        (turn1, rest, wait_exit(&mut child, Duration::from_secs(15)))
    });
    let stderr = log.finish();
    assert!(started.elapsed() < Duration::from_secs(20), "sleep 30 was not waited out: {:?}", started.elapsed());
    assert_eq!(code, Some(0), "EOF after the second turn: {stderr}");
    assert!(stderr.contains("ara: interrupt received, aborting"), "{stderr}");
    assert_eq!(turn1.last().unwrap()["type"], json!("agent_end"), "{turn1:?}");
    let answer = rest.iter().find(|v| v["type"] == "message_end" && v["message"]["role"] == "assistant").unwrap();
    assert_eq!(answer["message"]["content"][0]["text"], json!("After the abort."));
    assert_eq!(up.served(), 2, "no model call after the abort; one for the next turn");
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    assert_eq!(
        roles(&entries),
        vec!["model_change", "user", "assistant", "toolResult", "assistant", "user", "assistant"]
    );
    // Windows: Ctrl+Break reaches the whole process group, Bash included, so
    // the tool may report its own exit instead of `[Command aborted]`; the
    // turn-level abort below is deterministic on both platforms.
    #[cfg(unix)]
    assert!(entries[4]["message"]["content"][0]["text"].as_str().unwrap().ends_with("[Command aborted]"));
    assert_eq!(entries[5]["message"]["stopReason"], json!("aborted"));
    assert_eq!(journal_user_texts(&entries), vec!["run something slow", "after"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_interrupt_at_idle_prompt_exits_130() {
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let mut c = env.cmd(&up.base_url(), &["--repl"]);
    let mut child = spawn_repl(&mut c);
    let _stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let code = tokio::task::block_in_place(|| {
        assert!(log.wait_for("> ", Duration::from_secs(15)), "no prompt");
        std::thread::sleep(Duration::from_millis(300));
        interrupt(&child);
        wait_exit(&mut child, Duration::from_secs(5))
    });
    let stderr = log.finish();
    assert_eq!(code, Some(130), "{stderr}");
    assert_eq!(up.served(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_streams_text_and_reports_tool_progress() {
    let env = Env::new();
    // The tool call arrives 4 s after the text, so text shown earlier was streamed.
    let up = upstream(json!({"responses": [
        {"events": [text("Checking first."), {"sleep_ms": 4000}, tool_call(0, "call_p", "bash", "{\"command\":\"echo progress-ran; sleep 3\"}"), finish("tool_calls"), done()]},
        {"events": [text("All "), text("done."), finish("stop"), done()]}
    ]}))
    .await;
    let mut c = env.cmd(&up.base_url(), &["--repl"]);
    let mut child = spawn_repl(&mut c);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let out = StderrLog::reading(child.stdout.take().unwrap());
    let (shown_after, done_early, code) = tokio::task::block_in_place(|| {
        assert!(log.wait_for("> ", Duration::from_secs(15)), "no prompt");
        let sent = Instant::now();
        writeln!(stdin, "check it").unwrap();
        assert!(out.wait_for("Checking first.", Duration::from_secs(15)), "no streamed text");
        let shown_after = sent.elapsed();
        let start = "ara: tool bash: echo progress-ran; sleep 3\n";
        assert!(log.wait_for(start, Duration::from_secs(15)), "no tool start line");
        let done_early = log.0.lock().unwrap().contains("ara: tool bash done");
        // EOF is read only after the turn ends.
        drop(stdin);
        (shown_after, done_early, wait_exit(&mut child, Duration::from_secs(30)))
    });
    let stderr = log.finish();
    let stdout = out.finish();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(shown_after < Duration::from_secs(3), "text waited for the message end: {shown_after:?}");
    assert!(!done_early, "the start line appeared only after the tool ended: {stderr}");
    assert_eq!(stdout, "Checking first.\nAll done.\n", "two deltas form one line; nothing is printed twice");
    let working = stderr.find("Working... (turn 1)").unwrap();
    let start = stderr.find("ara: tool bash: echo progress-ran; sleep 3").unwrap();
    let end = stderr.find("ara: tool bash done").unwrap_or_else(|| panic!("{stderr}"));
    assert!(working < start && start < end, "{stderr}");
    assert_eq!(up.served(), 2);
}

/// OMP `editToolRenderer.activitySummary`: an `edit` whose payload is one
/// `input` string names its target file, parsed in the configured edit mode.
#[tokio::test(flavor = "multi_thread")]
async fn repl_edit_progress_names_the_target_file() {
    let env = Env::new();
    let patch = "*** Begin Patch
*** Add File: notes.txt
+hello
*** End Patch";
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_e", "edit", &json!({"input": patch}).to_string()), finish("tool_calls"), done()]},
        {"events": [text("Added."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--edit-mode", "apply_patch"]),
        "add notes
",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains(
            "ara: tool edit: notes.txt
"
        ),
        "{stderr}"
    );
    assert!(stderr.contains("ara: tool edit done"), "{stderr}");
    assert_eq!(
        std::fs::read_to_string(env.work.path().join("notes.txt")).unwrap(),
        "hello
"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_shows_text_a_provider_sent_without_deltas() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {"type": "message", "id": "msg_whole", "content": [{"type": "output_text", "text": "Whole answer."}]}}},
            {"data": {"type": "response.completed", "response": {"status": "completed"}}}
        ]}
    ]}))
    .await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl", "--api", "openai-responses"]), "hello\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Whole answer.\n", "shown once, although no text delta was streamed");
    assert_eq!(up.served(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn repl_failed_turn_shows_the_provider_cause_and_keeps_the_session() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"status": 400, "body": "{\"error\":{\"message\":\"turn backend down\"}}"},
        {"events": [text("Recovered."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl"]), "first prompt\nsecond prompt\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Recovered.\n");
    assert!(stderr.contains("ara: turn 1 ended in error (400 turn backend down); session kept"), "{stderr}");
    assert!(!stderr.contains("turn 2 ended in error"), "{stderr}");
    let entries = journal(&env.session_files()[0]);
    assert_eq!(journal_user_texts(&entries), vec!["first prompt", "second prompt"]);
    assert_eq!(up.served(), 2);
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
    assert_eq!(reqs[1]["body"]["messages"].as_array().unwrap().len(), 5, "2 system + prior turn + new prompt");
    // The reminder stays on the first user turn; later turns are unchanged.
    assert!(reqs[1]["body"]["messages"][2]["content"].as_str().unwrap().starts_with("<system-reminder>"));
    assert_eq!(reqs[1]["body"]["messages"][4], json!({"role": "user", "content": "second from stdin"}));
}

#[tokio::test]
async fn usage_errors_exit_2() {
    let env = Env::new();
    let out = output(env.cmd("http://127.0.0.1:9/v1", &["--tools", "read,teleport", "x"])).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown tool \"teleport\""));
}

/// `--max-tokens 0` must be a clap usage error that names the flag, before
/// any session or model work: the run used to send `max_tokens: 0` and only
/// `/compact` failed, as `InvalidMaxTokens` with no hint at the flag.
#[tokio::test]
async fn max_tokens_zero_is_rejected_by_the_flag_parser() {
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["--max-tokens", "0", "hi"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "0 is not a valid budget: {stderr}");
    assert!(stderr.contains("--max-tokens"), "the error names the flag: {stderr}");
    assert_eq!(stdout, "");
    assert_eq!(up.served(), 0, "no request is sent for a usage error");
    assert!(env.session_files().is_empty(), "no session is created for a usage error");
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
    let first_user = reqs[0]["body"]["messages"][2]["content"].as_str().unwrap();
    assert!(first_user.ends_with("</system-reminder>\n\n--- a\n+++ b\nreview this diff"), "{first_user}");
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
    let out =
        output(env.cmd(&up.base_url(), &["--resume", session.to_str().unwrap(), "--responses-stateful", "y"])).await;
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--responses-stateful requires --api openai-responses"));
    assert_eq!(std::fs::read(&session).unwrap(), before, "invalid Responses route left the journal untouched");
    let out = output(env.cmd(
        &up.base_url(),
        &["--api", "openai-responses", "--chat-replay-reasoning-content", "--resume", session.to_str().unwrap(), "y"],
    ))
    .await;
    assert_eq!(out.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("--chat-replay-reasoning-content requires --api openai-completions")
    );
    assert_eq!(std::fs::read(&session).unwrap(), before, "invalid Chat route left the journal untouched");
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
    let expected = Command::new("bash").arg("-c").arg("pwd").current_dir(project.path()).output().unwrap();
    assert!(expected.status.success());
    assert_eq!(recorded.trim(), String::from_utf8_lossy(&expected.stdout).trim());
    assert!(!elsewhere.path().join("where.txt").exists());
}

#[tokio::test]
async fn search_and_hashline_edit_fix_a_seeded_bug() {
    let env = Env::new();
    let w = env.work.path();
    std::fs::create_dir_all(w.join("src")).unwrap();
    std::fs::write(w.join("src/math.py"), "def add(a, b):\n    return a - b  # BUG\n").unwrap();
    std::fs::write(w.join("src/util.py"), "def ident(x):\n    return x\n").unwrap();
    // The tag is the content hash upstream also computes for this text.
    let edit = json!({"input": "[src/math.py#450E]\nPUT 2.=2:\n+    return a + b\n"}).to_string();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_g", "glob", "{\"path\":\"src/*.py\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_s", "grep", "{\"pattern\":\"BUG\",\"path\":\"src\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_e", "edit", &edit), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_r", "read", "{\"path\":\"src/math.py\"}"), finish("tool_calls"), done()]},
        {"events": [text("Fixed add() in src/math.py."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Find and fix the bug marked BUG"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Fixed add() in src/math.py.\n");
    assert_eq!(std::fs::read_to_string(w.join("src/math.py")).unwrap(), "def add(a, b):\n    return a + b\n");
    let entries = journal(&env.session_files()[0]);
    let result = |i: usize| entries[i]["message"]["content"][0]["text"].as_str().unwrap().to_string();
    let mut listed: Vec<String> = result(4).lines().map(str::to_string).collect();
    listed.sort();
    assert_eq!(listed, ["# src/", "math.py", "util.py"]);
    assert_eq!(result(6), "# src/\n## math.py#450E\n 1:def add(a, b):\n*2:    return a - b  # BUG");
    let edited = result(8);
    assert!(edited.starts_with("[src/math.py#") && edited.contains("return a + b"), "{edited}");
    assert!(!edited.contains("#450E"), "the tag changes after an edit: {edited}");
    let new_tag = &edited[13..17];
    assert_eq!(result(10), format!("[src/math.py#{new_tag}]\n1:def add(a, b):\n2:    return a + b"));
    let reqs = up.requests.lock().await;
    let tool_names: Vec<&str> =
        reqs[0]["body"]["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
    assert_eq!(tool_names, ["read", "write", "edit", "bash", "grep", "glob"]);
}

#[tokio::test]
async fn bare_tagged_hashline_header_recovers_the_nested_file() {
    // The CLI canonicalizes `--cwd`; on Windows that adds the `\\?\` prefix the
    // snapshot keys do not carry, which used to reject every nested recovery.
    let env = Env::new();
    let w = env.work.path();
    std::fs::create_dir_all(w.join("src")).unwrap();
    std::fs::write(w.join("src/math.py"), "def add(a, b):\n    return a - b  # BUG\n").unwrap();
    let edit = json!({"input": "[math.py#450E]\nPUT 2.=2:\n+    return a + b\n"}).to_string();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_r", "read", "{\"path\":\"src/math.py\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_e", "edit", &edit), finish("tool_calls"), done()]},
        {"events": [text("Fixed add()."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Fix the bug marked BUG"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Fixed add().\n");
    assert_eq!(std::fs::read_to_string(w.join("src/math.py")).unwrap(), "def add(a, b):\n    return a + b\n");
    assert!(!w.join("math.py").exists());
    let entries = journal(&env.session_files()[0]);
    let receipt = entries.iter().find(|entry| entry["message"]["toolCallId"] == json!("call_e")).unwrap();
    assert_eq!(receipt["message"]["isError"], json!(false), "{receipt}");
    let body = receipt["message"]["content"][0]["text"].as_str().unwrap();
    assert!(body.contains("math.py#") && body.contains("return a + b"), "{body}");
}

/// Windows `canonicalize` adds a verbatim `\\?\` prefix; OMP's `setProjectDir`
/// keeps a plain absolute path. Neither the session header nor the model
/// request may carry the prefix from `--cwd`.
#[tokio::test]
async fn absolute_cwd_flag_never_reaches_the_header_or_the_model_request_verbatim() {
    let env = Env::new();
    let dir = tempfile::Builder::new().prefix("ara-e2e-plain-cwd-").tempdir().unwrap();
    let up = upstream(json!({"responses": [{"events": [text("ok"), finish("stop"), done()]}]})).await;
    let out = output(env.cmd_in(dir.path(), &up.base_url(), &["--cwd", dir.path().to_str().unwrap(), "hi"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "ok\n");
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    // The journal starts with a title entry; the Session header follows it.
    let entries = journal(&files[0]);
    let header = entries.iter().find(|e| e["type"] == "session").expect("session header");
    let canonical = dir.path().canonicalize().unwrap().display().to_string();
    let plain = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical);
    assert_eq!(header["cwd"], json!(plain), "{header}");
    let requests = up.requests.lock().await;
    let body = requests[0]["body"].to_string();
    // In the JSON body every backslash is escaped, so `\\?\` also appears as
    // `\\\\?\\`.
    for needle in [r"\\?\", "//?/", r"\\\\?\\"] {
        assert!(!body.contains(needle), "the request body carries {needle:?}: {body}");
    }
}

#[tokio::test]
async fn deleted_tag_target_drops_its_own_snapshot_from_recovery() {
    // `math.py` is deleted outside the edit tool. Its own stale snapshot must not
    // count as a candidate: alone it reports the authored path, and next to a
    // same-content nested copy it leaves exactly one recovery target.
    let edit = json!({"input": "[math.py#450E]\nPUT 2.=2:\n+    return a + b\n"}).to_string();
    for nested in [false, true] {
        let env = Env::new();
        let w = env.work.path();
        std::fs::write(w.join("math.py"), "def add(a, b):\n    return a - b  # BUG\n").unwrap();
        let mut responses = vec![json!({"events": [
            tool_call(0, "call_r", "read", "{\"path\":\"math.py\"}"), finish("tool_calls"), done()]})];
        if nested {
            std::fs::create_dir_all(w.join("src")).unwrap();
            std::fs::write(w.join("src/math.py"), "def add(a, b):\n    return a - b  # BUG\n").unwrap();
            responses.push(json!({"events": [
                tool_call(0, "call_n", "read", "{\"path\":\"src/math.py\"}"), finish("tool_calls"), done()]}));
        }
        responses.push(json!({"events": [
            tool_call(0, "call_rm", "bash", "{\"command\":\"rm math.py\"}"), finish("tool_calls"), done()]}));
        responses.push(json!({"events": [tool_call(0, "call_e", "edit", &edit), finish("tool_calls"), done()]}));
        responses.push(json!({"events": [text("done"), finish("stop"), done()]}));
        let up = upstream(json!({ "responses": responses })).await;
        let out = output(env.cmd(&up.base_url(), &["Fix the bug marked BUG"])).await;
        assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
        assert!(!w.join("math.py").exists());
        let entries = journal(&env.session_files()[0]);
        let receipt = entries.iter().find(|entry| entry["message"]["toolCallId"] == json!("call_e")).unwrap();
        let body = receipt["message"]["content"][0]["text"].as_str().unwrap();
        if nested {
            assert_eq!(receipt["message"]["isError"], json!(false), "{receipt}");
            let fixed = std::fs::read_to_string(w.join("src/math.py")).unwrap();
            assert_eq!(fixed, "def add(a, b):\n    return a + b\n");
        } else {
            assert_eq!(receipt["message"]["isError"], json!(true), "{receipt}");
            assert!(body.contains("File not found: math.py."), "{body}");
        }
    }
}

#[tokio::test]
async fn read_multiple_ranges_reaches_model_and_session_journal() {
    let env = Env::new();
    std::fs::write(env.work.path().join("lines.txt"), (1..=12).map(|n| format!("line{n}\n")).collect::<String>())
        .unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_read_ranges", "read", "{\"path\":\"lines.txt:3-3,9-9\"}"),
            finish("tool_calls"), done()]},
        {"events": [text("I found line3 and line9."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Read lines 3 and 9"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "I found line3 and line9.\n");
    let entries = journal(&env.session_files()[0]);
    let receipt = entries.iter().find(|entry| entry["message"]["role"] == "toolResult").unwrap();
    assert_eq!(receipt["message"]["toolCallId"], json!("call_read_ranges"));
    let body = receipt["message"]["content"][0]["text"].as_str().unwrap();
    assert!(body.contains("\n3:line3\n…\n9:line9") && !body.contains("4:line4"), "{body}");
    let reqs = up.requests.lock().await;
    let forwarded = reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap();
    assert_eq!(forwarded["role"], json!("tool"));
    assert_eq!(forwarded["content"], json!(body));
}

#[tokio::test]
async fn read_disjoint_ranges_send_block_boundaries_to_model_and_journal() {
    let env = Env::new();
    std::fs::write(
        env.work.path().join("blocks.ts"),
        "function one() {\n  const x = 1;\n  return x;\n}\nfunction two() {\n  const y = 2;\n  return y;\n}\n",
    )
    .unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_context", "read", "{\"path\":\"blocks.ts:1-1,5-5\"}"),
            finish("tool_calls"), done()]},
        {"events": [text("Both function boundaries are visible."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Inspect both functions"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Both function boundaries are visible.\n");
    let entries = journal(&env.session_files()[0]);
    let receipt = entries.iter().find(|entry| entry["message"]["role"] == "toolResult").unwrap();
    let body = receipt["message"]["content"][0]["text"].as_str().unwrap();
    assert!(body.contains("\n1:function one() {\n…\n4:}\n5:function two() {\n…\n8:}"), "{body}");
    assert!(!body.contains("2:  const x") && !body.contains("7:  return y"), "{body}");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"], json!(body));
}

#[tokio::test]
async fn read_skill_disjoint_ranges_send_block_boundaries_to_model_and_journal() {
    let env = Env::new();
    let skill_dir = env.work.path().join(".ara/skills/greeting");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\ndescription: Greeting\n---\nRead this skill.\n").unwrap();
    std::fs::write(skill_dir.join("blocks.ts"), "function one() {\n  return 1;\n}\nfunction two() {\n  return 2;\n}\n")
        .unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_skill_context", "read", "{\"path\":\"skill://greeting/blocks.ts:1-1,4-4\"}"),
            finish("tool_calls"), done()]},
        {"events": [text("Both skill function boundaries are visible."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["--line-numbers", "Inspect the skill functions"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Both skill function boundaries are visible.\n");
    let entries = journal(&env.session_files()[0]);
    let receipt = entries.iter().find(|entry| entry["message"]["role"] == "toolResult").unwrap();
    let body = receipt["message"]["content"][0]["text"].as_str().unwrap();
    assert_eq!(body, "1|function one() {\n…\n3|}\n4|function two() {\n…\n6|}");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"], json!(body));
}

#[tokio::test]
async fn context_files_and_skills_reach_the_model_and_skill_urls_resolve() {
    let env = Env::new();
    let work = env.work.path();
    std::fs::write(work.join("AGENTS.md"), "Always answer in French.").unwrap();
    let skill_dir = work.join(".ara/skills/greeting");
    std::fs::create_dir_all(skill_dir.join("assets")).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\ndescription: How to greet people\n---\nSay bonjour twice.")
        .unwrap();
    std::fs::write(skill_dir.join("assets/extra.txt"), "extra asset").unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_s", "read", "{\"path\":\"skill://greeting\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_a", "read", "{\"path\":\"skill://greeting/assets/extra.txt\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_x", "read", "{\"path\":\"skill://greeting/../../AGENTS.md\"}"), finish("tool_calls"), done()]},
        {"events": [text("Bonjour, bonjour."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Greet me"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Bonjour, bonjour.\n");

    let reqs = up.requests.lock().await;
    let first = reqs[0]["body"]["messages"].as_array().unwrap();
    let system: String =
        first.iter().filter(|m| m["role"] == "system").map(|m| m["content"].as_str().unwrap()).collect();
    assert!(system.contains("- greeting: How to greet people"), "skill listed");
    assert!(system.contains("`skill://<name>`"), "skill:// advertised");
    assert!(!system.contains("history://") && !system.contains("omp://"), "unported URLs not advertised");
    assert!(system.contains("<repo-rules>") && system.contains("Always answer in French."), "AGENTS.md included");
    // The CLI strips the verbatim drive prefix `canonicalize` adds on Windows.
    let canonical = work.join("AGENTS.md").canonicalize().unwrap().display().to_string();
    let displayed_path = canonical.strip_prefix(r"\\?\").unwrap_or(&canonical).replace('\\', "/");
    assert!(system.contains(&format!("<file path=\"{displayed_path}\">")));

    let entries = journal(&env.session_files()[0]);
    let results: Vec<&Value> = entries.iter().filter(|e| e["message"]["role"] == "toolResult").collect();
    assert!(results[0]["message"]["content"][0]["text"].as_str().unwrap().contains("Say bonjour twice."));
    assert!(results[1]["message"]["content"][0]["text"].as_str().unwrap().contains("extra asset"));
    assert_eq!(results[2]["message"]["isError"], json!(true));
    assert!(
        results[2]["message"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Path traversal (..) is not allowed in skill:// URLs")
    );
}

#[tokio::test]
async fn skill_url_search_and_read_only_write_reach_cli() {
    let env = Env::new();
    let work = env.work.path();
    let skill_dir = work.join(".ara/skills/greeting");
    std::fs::create_dir_all(skill_dir.join("assets")).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\ndescription: Greeting\n---\nmarker-in-skill\n").unwrap();
    let asset = skill_dir.join("assets/extra.txt");
    std::fs::write(&asset, "original asset\n").unwrap();
    std::fs::write(work.join("ordinary.txt"), "marker-in-skill\n").unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_glob", "glob", "{\"path\":\"skill://greeting\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_grep", "grep", "{\"pattern\":\"marker-in-skill\",\"path\":\"skill://greeting\"}"), finish("tool_calls"), done()]},
        {"events": [tool_call(0, "call_write", "write", "{\"path\":\"skill://greeting/assets/extra.txt\",\"content\":\"changed\"}"), finish("tool_calls"), done()]},
        {"events": [text("Done."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Inspect the skill"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Done.\n");
    let entries = journal(&env.session_files()[0]);
    let results: Vec<&Value> = entries.iter().filter(|e| e["message"]["role"] == "toolResult").collect();
    assert_eq!(results.len(), 3);
    assert!(results[0]["message"]["content"][0]["text"].as_str().unwrap().contains("SKILL.md"));
    assert!(results[1]["message"]["content"][0]["text"].as_str().unwrap().contains("marker-in-skill"));
    assert!(!results[0]["message"]["content"][0]["text"].as_str().unwrap().contains("ordinary.txt"));
    assert!(!results[1]["message"]["content"][0]["text"].as_str().unwrap().contains("ordinary.txt"));
    assert_eq!(results[2]["message"]["isError"], json!(true));
    assert!(results[2]["message"]["content"][0]["text"].as_str().unwrap().contains("read-only for write"));
    assert_eq!(std::fs::read_to_string(&asset).unwrap(), "original asset\n");
    assert!(!work.join("skill:").exists());
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 4);
    let system: String = reqs[0]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .filter_map(|m| m["content"].as_str())
        .collect();
    assert!(system.contains("`grep` can search a `skill://` file or directory"));
    assert!(system.contains("`glob` can list a `skill://` file or directory"));
    assert!(system.contains("`write` rejects `skill://` URLs as read-only"));
    for (request, id, fragment) in
        [(1, "call_glob", "SKILL.md"), (2, "call_grep", "marker-in-skill"), (3, "call_write", "read-only for write")]
    {
        let messages = reqs[request]["body"]["messages"].as_array().unwrap();
        let receipt = messages.iter().find(|m| m["role"] == "tool" && m["tool_call_id"] == id).unwrap();
        assert!(receipt["content"].as_str().unwrap().contains(fragment), "{id}: {receipt}");
    }
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
#[tokio::test]
async fn skill_directory_locale_order_reaches_cli_and_journal() {
    let env = Env::new();
    let skill_dir = env.work.path().join(".ara/skills/greeting");
    let assets = skill_dir.join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\ndescription: Greeting\n---\nbody\n").unwrap();
    // This punctuation pair has the same order in the two recorded Bun locales.
    for name in ["a_1.txt", "a-1.txt"] {
        std::fs::write(assets.join(name), b"").unwrap();
    }
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_read", "read", "{\"path\":\"skill://greeting/assets:raw:1-1\"}"), finish("tool_calls"), done()]},
        {"events": [text("Done."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&up.base_url(), &["Read the first asset"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Done.\n");
    // Linux's recorded Bun locale is en-US. Other Windows locales have no
    // fixed-source oracle, so only assert the host-to-journal-to-model path.
    #[cfg(target_os = "linux")]
    let expected = Some("a_1.txt");
    #[cfg(windows)]
    let expected: Option<&str> = None;
    let entries = journal(&env.session_files()[0]);
    let result = entries.iter().find(|entry| entry["message"]["role"] == "toolResult").unwrap();
    let content = result["message"]["content"][0]["text"].as_str().unwrap();
    let selected = content.lines().next().unwrap();
    assert!(["a_1.txt", "a-1.txt"].contains(&selected), "{content:?}");
    if let Some(expected) = expected {
        assert_eq!(selected, expected);
    }
    assert!(content.contains("[1 more lines in resource. Use :2 to continue]"));
    let requests = up.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["body"]["messages"].as_array().unwrap();
    let carried = messages.iter().find(|m| m["role"] == "tool" && m["tool_call_id"] == "call_read").unwrap();
    assert!(carried["content"].as_str().unwrap().starts_with(selected));
}

#[tokio::test]
async fn skill_flags_filter_listing_and_resolution() {
    let env = Env::new();
    let work = env.work.path();
    for (name, description) in [("greeting", "How to greet people"), ("farewell", "How to say goodbye")] {
        let dir = work.join(".ara/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\ndescription: {description}\n---\nBody of {name}.")).unwrap();
    }
    // A malformed SKILL.md is reported on stderr, not dropped silently.
    let broken = work.join(".ara/skills/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("SKILL.md"), "---\ndescription: ok\nbad: [unclosed\n---\nbody").unwrap();
    let read_greeting = || tool_call(0, "call_g", "read", "{\"path\":\"skill://greeting\"}");
    let script = || {
        json!({"responses": [
            {"events": [read_greeting(), finish("tool_calls"), done()]},
            {"events": [text("done"), finish("stop"), done()]}
        ]})
    };

    // `--skills` filters by name glob: only `farewell` is listed and resolvable.
    let up = upstream(script()).await;
    let out = output(env.cmd(&up.base_url(), &["--skills", "fare*", "hi"])).await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: skill warning") && stderr.contains("Failed to parse YAML frontmatter"), "{stderr}");
    let reqs = up.requests.lock().await;
    let system: String = reqs[0]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert!(system.contains("- farewell: How to say goodbye") && !system.contains("- greeting:"), "{system}");
    let tool_result = &reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"];
    assert!(tool_result.as_str().unwrap().contains("Unknown skill: greeting\nAvailable: farewell"), "{tool_result}");
    drop(reqs);

    // `--no-skills`: no listing and nothing resolves.
    let up = upstream(script()).await;
    let out = output(env.cmd(&up.base_url(), &["--no-skills", "hi"])).await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("skill warning"), "{stderr}");
    let reqs = up.requests.lock().await;
    let system: String = reqs[0]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert!(!system.contains("- farewell:") && !system.contains("- greeting:"), "{system}");
    let tool_result = &reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"];
    assert!(tool_result.as_str().unwrap().contains("Unknown skill: greeting\nAvailable: none"), "{tool_result}");
}

/// Joined system messages of the first request.
async fn first_system(up: &ara_testkit::FakeUpstream) -> String {
    let reqs = up.requests.lock().await;
    reqs[0]["body"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "system")
        .map(|m| m["content"].as_str().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Review F1: without flags the CLI uses the discovered `SYSTEM.md` (project
/// before user) and `APPEND_SYSTEM.md`, as upstream `main.ts` does; flags win
/// and the last occurrence of a flag counts.
#[tokio::test]
async fn discovered_system_md_and_prompt_flags() {
    let env = Env::new();
    let work = env.work.path();
    let user_dir = env._home.path().join("agent");
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::create_dir_all(work.join(".ara")).unwrap();
    std::fs::write(work.join(".ara/APPEND_SYSTEM.md"), "APPEND-MARKER").unwrap();
    let run = |args: Vec<&'static str>| {
        let env = &env;
        async move {
            let up = upstream(json!({"responses": [{"events": [text("ok"), finish("stop"), done()]}]})).await;
            let mut all = args;
            all.push("hi");
            let out = output(env.cmd(&up.base_url(), &all)).await;
            let (_, stderr) = text_of(&out);
            assert_eq!(out.status.code(), Some(0), "{stderr}");
            (first_system(&up).await, stderr)
        }
    };

    // The actual CLI/provider request carries ARA's default workflow before
    // a host-owned SYSTEM.md or explicit custom prompt replaces it.
    let (system, _) = run(vec![]).await;
    assert!(
        system.contains("evidence-guided problem solving") && system.contains("falsifiable hypotheses"),
        "{system}"
    );
    assert!(system.contains("host-granted budgets and deadlines"), "{system}");
    assert!(system.contains("underlying problem and desired outcome"), "{system}");
    assert!(
        system.contains("hard constraints (physical limits, protocol requirements, published contracts)"),
        "{system}"
    );
    assert!(
        system.contains("given facts F and constraints C, choose solution S and validate by observation V"),
        "{system}"
    );
    assert!(system.contains("a clear explanation alone does not establish correctness"), "{system}");
    assert_eq!(system.matches("APPEND-MARKER").count(), 1, "{system}");

    std::fs::write(user_dir.join("SYSTEM.md"), "USER-SYSTEM-MARKER").unwrap();
    std::fs::write(work.join(".ara/SYSTEM.md"), "PROJECT-SYSTEM-MARKER").unwrap();
    let (system, _) = run(vec![]).await;
    assert_eq!(system.matches("PROJECT-SYSTEM-MARKER").count(), 1, "{system}");
    assert!(!system.contains("USER-SYSTEM-MARKER") && !system.contains("coding harness"), "{system}");
    assert!(!system.contains("evidence-guided problem solving"), "{system}");
    assert_eq!(system.matches("APPEND-MARKER").count(), 1, "{system}");

    std::fs::remove_file(work.join(".ara/SYSTEM.md")).unwrap();
    let (system, _) = run(vec![]).await;
    assert!(system.contains("USER-SYSTEM-MARKER") && !system.contains("coding harness"), "{system}");

    let (system, _) = run(vec!["--system-prompt", "FIRST-FLAG", "--system-prompt", "FLAG-SYSTEM-MARKER"]).await;
    assert!(system.contains("FLAG-SYSTEM-MARKER"), "{system}");
    assert!(!system.contains("FIRST-FLAG") && !system.contains("USER-SYSTEM-MARKER"), "{system}");
    assert!(!system.contains("evidence-guided problem solving"), "{system}");
    let (system, _) = run(vec!["--append-system-prompt", "A1", "--append-system-prompt", "FLAG-APPEND"]).await;
    assert!(system.contains("FLAG-APPEND") && !system.contains("APPEND-MARKER"), "{system}");
    assert!(!system.contains("\nA1"), "{system}");

    // Review F8: an unreadable prompt file warns and falls back to the text.
    let dir = work.join("prompt-dir");
    std::fs::create_dir_all(&dir).unwrap();
    let dir_arg: &'static str = Box::leak(dir.display().to_string().into_boxed_str());
    let (system, stderr) = run(vec!["--system-prompt", dir_arg]).await;
    assert!(stderr.contains(&format!("ara: warning: Could not read system prompt file {dir_arg}")), "{stderr}");
    assert!(system.contains(dir_arg), "{system}");
}

/// Review F7: a relative `ARA_HOME` is taken against the process cwd, so a
/// user skill listed in the prompt also resolves through `skill://`.
#[tokio::test]
async fn relative_ara_home_user_skills_resolve() {
    let env = Env::new();
    let skill = env._home.path().join("rel-home/agent/skills/userskill");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "---\ndescription: A user skill\n---\nUser skill body.").unwrap();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_u", "read", "{\"path\":\"skill://userskill\"}"), finish("tool_calls"), done()]},
        {"events": [text("done"), finish("stop"), done()]}
    ]}))
    .await;
    let mut cmd = env.cmd(&up.base_url(), &["hi"]);
    cmd.env("ARA_HOME", "rel-home").current_dir(env._home.path());
    let out = output(cmd).await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(first_system(&up).await.contains("- userskill: A user skill"));
    let reqs = up.requests.lock().await;
    let result = &reqs[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"];
    assert!(result.as_str().unwrap().contains("User skill body."), "{result}");
}

#[tokio::test]
async fn ast_grep_is_opt_in_and_reaches_model_and_session_journal() {
    let env = Env::new();
    std::fs::write(env.work.path().join("search.ts"), "const sharedSymbol = 1;\n").unwrap();

    let plain = upstream(json!({"responses": [
        {"events": [text("No search requested."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&plain.base_url(), &["-p", "Say hello"])).await;
    let (_, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let requests = plain.requests.lock().await;
    let default_tools = requests[0]["body"]["tools"].as_array().unwrap();
    assert!(!default_tools.iter().any(|tool| tool["function"]["name"] == "ast_grep"));
    drop(requests);

    let searching = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_ast", "ast_grep", "{\"pat\":\"sharedSymbol\",\"path\":\"search.ts\"}"), finish("tool_calls"), done()]},
        {"events": [text("Found sharedSymbol."), finish("stop"), done()]}
    ]}))
    .await;
    let out = output(env.cmd(&searching.base_url(), &["--tools", "edit,ast_grep", "Find sharedSymbol"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Found sharedSymbol.\n");
    let requests = searching.requests.lock().await;
    let enabled_tools = requests[0]["body"]["tools"].as_array().unwrap();
    assert_eq!(
        enabled_tools.iter().map(|tool| tool["function"]["name"].as_str().unwrap()).collect::<Vec<_>>(),
        ["edit", "ast_grep"]
    );
    let forwarded = requests[1]["body"]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap();
    assert!(forwarded.contains("sharedSymbol") && forwarded.contains("search.ts#"), "{forwarded}");
    drop(requests);
    let sessions = env.session_files();
    assert_eq!(sessions.len(), 2);
    let entries = sessions
        .iter()
        .map(|path| journal(path))
        .find(|entries| entries.iter().any(|entry| entry["message"]["toolCallId"] == "call_ast"))
        .unwrap();
    let result = entries.iter().find(|entry| entry["message"]["role"] == "toolResult").unwrap();
    assert_eq!(result["message"]["toolCallId"], json!("call_ast"));
    assert_eq!(result["message"]["details"]["matchCount"], json!(1));
}

/// Terminate a run mid-tool (and its process tree) so the Session ends with a
/// tool call that has no recorded result — the same end state as SIGKILL in
/// `crash_mid_tool_resume_reports_unknown_effect_without_replay`.
fn kill_mid_tool(child: &mut Child, tag: &str) {
    // `tag` is on the tool command line so an orphaned process can be found.
    assert!(!tag.is_empty(), "the tool command must carry a unique tag");
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/F", "/T", "/PID", &child.id().to_string()]).status();
    }
    #[cfg(unix)]
    {
        let _ = child.kill();
        let _ = Command::new("pkill").args(["-f", tag]).status();
    }
    let _ = child.wait();
}

/// V1-RESUME: exit, restart with `--repl --continue`, and the second process
/// still sees the first turn's user text and assistant answer.
#[tokio::test(flavor = "multi_thread")]
async fn repl_continue_replays_the_prior_turn_in_the_model_request() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(env.cmd(&up.base_url(), &["--repl"]), "alpha-turn\n").await;
    let (first_stdout, first_stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{first_stderr}");
    assert_eq!(first_stdout, "First answer.\n");
    let files = env.session_files();
    assert_eq!(files.len(), 1, "the first process wrote one Session");
    let session = files[0].clone();

    let out = repl_output(env.cmd(&up.base_url(), &["--repl", "--continue"]), "beta-turn\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Second answer.\n");
    assert!(stderr.contains(&format!("ara: session {}", session.display())), "{stderr}");
    assert_eq!(env.session_files(), vec![session.clone()], "still exactly one Session file");
    let entries = journal(&session);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "user", "assistant"]);
    assert_eq!(journal_user_texts(&entries), vec!["alpha-turn", "beta-turn"]);
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 2);
    let second = reqs[1]["body"]["messages"].as_array().unwrap();
    assert!(user_texts(second).iter().any(|t| t.contains("alpha-turn")), "{second:?}");
    assert!(
        second.iter().any(|m| m["role"] == "assistant" && m["content"].to_string().contains("First answer.")),
        "{second:?}"
    );
}

/// V1-RESUME: `--repl --resume <older file>` appends the turn to that file,
/// not to the newest Session in the directory.
#[tokio::test(flavor = "multi_thread")]
async fn repl_resume_named_file_appends_to_that_session() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("Answer one."), finish("stop"), done()]},
        {"events": [text("Answer two."), finish("stop"), done()]},
        {"events": [text("Answer three."), finish("stop"), done()]}
    ]}))
    .await;
    assert_eq!(output(env.cmd(&up.base_url(), &["first turn"])).await.status.code(), Some(0));
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(output(env.cmd(&up.base_url(), &["second turn"])).await.status.code(), Some(0));
    let files = env.session_files();
    assert_eq!(files.len(), 2, "{files:?}");
    let older = files[0].clone();
    let newer = files[1].clone();

    let out =
        repl_output(env.cmd(&up.base_url(), &["--repl", "--resume", older.to_str().unwrap()]), "third turn\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Answer three.\n");
    assert_eq!(env.session_files(), vec![older.clone(), newer.clone()], "no extra Session file");
    assert_eq!(journal_user_texts(&journal(&older)), vec!["first turn", "third turn"]);
    assert_eq!(journal_user_texts(&journal(&newer)), vec!["second turn"], "the newest file is untouched");
    let reqs = up.requests.lock().await;
    let third = reqs[2]["body"]["messages"].as_array().unwrap();
    assert!(user_texts(third).iter().any(|t| t.contains("first turn")), "{third:?}");
    assert!(user_texts(third).iter().all(|t| !t.contains("second turn")), "{third:?}");
}

/// V1-RESUME: a failed resume is reported and never replaced by a new
/// conversation — nonzero exit, no model call, no new Session file.
#[tokio::test]
async fn repl_failed_resume_exits_without_a_new_session() {
    // --resume of a missing path
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let missing = env.sessions.join("no-such-session.jsonl");
    let out = output(env.cmd(&up.base_url(), &["--repl", "--resume", missing.to_str().unwrap()])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert_eq!(stdout, "");
    assert_eq!(up.served(), 0, "a failed resume never calls the model");
    assert!(env.session_files().is_empty(), "a failed resume never creates a Session file");
    assert!(stderr.contains("opening session") && stderr.contains(&missing.display().to_string()), "{stderr}");

    // --continue with an empty session directory
    let env = Env::new();
    let up =
        upstream(json!({"responses": [{"events": [text("must not be requested"), finish("stop"), done()]}]})).await;
    let out = output(env.cmd(&up.base_url(), &["--repl", "--continue"])).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert_eq!(stdout, "");
    assert_eq!(up.served(), 0);
    assert!(env.session_files().is_empty(), "an empty --continue never creates a Session file");
    assert!(
        stderr.contains("no session to continue") && stderr.contains(&env.sessions.display().to_string()),
        "{stderr}"
    );
}

/// V1-RESUME: a Session that ends with a tool call and no result is resumed in
/// the REPL with the unknown-effect warning; the tool is not re-run and the
/// synthesized result is journaled.
#[tokio::test(flavor = "multi_thread")]
async fn repl_resume_after_interrupted_tool_reports_unknown_effect_without_replay() {
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
    kill_mid_tool(&mut child, &token);
    let session = env.session_files().pop().expect("journal materialized before the tool started");

    let up2 =
        upstream(json!({"responses": [{"events": [text("Resumed after checking."), finish("stop"), done()]}]})).await;
    let out = repl_output(
        env.cmd(&up2.base_url(), &["--repl", "--resume", session.to_str().unwrap()]),
        "continue carefully\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Resumed after checking.\n");
    assert!(
        stderr.contains("call_crash were interrupted before a result was recorded; their effects are unknown and they were not re-run"),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&runs).unwrap(), "run\n", "the command was not replayed");
    let entries = journal(&session);
    assert_eq!(roles(&entries), vec!["model_change", "user", "assistant", "toolResult", "user", "assistant"]);
    assert_eq!(entries[4]["message"]["toolCallId"], json!("call_crash"));
    assert_eq!(entries[4]["message"]["details"]["source"], json!("interrupted_unknown_effect"));
    assert!(entries[4]["message"]["isError"].as_bool().unwrap_or(false), "{entries:?}");
    assert_eq!(journal_user_texts(&entries), vec!["do the long thing", "continue carefully"]);
    let reqs = up2.requests.lock().await;
    let msgs = reqs[0]["body"]["messages"].as_array().unwrap();
    let tool_msg = msgs.iter().find(|m| m["role"] == "tool").unwrap();
    assert_eq!(tool_msg["tool_call_id"], json!("call_crash"));
    assert!(tool_msg["content"].as_str().unwrap().contains("effects are unknown"));
}

/// Compaction entries in a journal (after the session header).
fn compaction_entries(entries: &[Value]) -> Vec<&Value> {
    entries.iter().filter(|e| e["type"] == "compaction").collect()
}

/// Concatenated text of every message in a Chat Completions request body.
fn request_text(messages: &[Value]) -> String {
    user_texts(messages)
        .into_iter()
        .chain(messages.iter().filter(|m| m["role"] == "assistant").map(|m| m["content"].to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether the request carries no tool definitions.
fn tools_absent_or_empty(body: &Value) -> bool {
    match body.get("tools") {
        None => true,
        Some(Value::Array(a)) => a.is_empty(),
        Some(_) => false,
    }
}

/// A compaction's `sourceEntryIds` are exactly the message entries before
/// `firstKeptEntryId`, which is a user or assistant message, and `tokensBefore` is a
/// positive estimate.
fn assert_compaction_provenance(entries: &[Value], compaction: &Value) {
    let first_kept = compaction["firstKeptEntryId"].as_str().unwrap();
    let kept_index = entries.iter().position(|e| e["id"] == first_kept).expect("firstKeptEntryId exists");
    assert!(
        matches!(entries[kept_index]["message"]["role"].as_str(), Some("user" | "assistant")),
        "native cut boundary"
    );
    let expected: Vec<&str> =
        entries[..kept_index].iter().filter(|e| e["type"] == "message").map(|e| e["id"].as_str().unwrap()).collect();
    let sources: Vec<&str> =
        compaction["sourceEntryIds"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(sources, expected, "sourceEntryIds are the summarized message entries");
    assert!(compaction["tokensBefore"].as_u64().is_some_and(|t| t > 0), "{compaction}");
}

/// Native manual `/compact` splits the last turn, then keeps working.
#[tokio::test(flavor = "multi_thread")]
async fn repl_manual_compact_then_keep_working() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Fake summary of earlier work."), finish("stop"), done()]},
        {"events": [text("Fake summary of earlier work."), finish("stop"), done()]},
        {"events": [text("Third answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n/compact\nthird prompt\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "First answer.\nSecond answer.\nThird answer.\n");
    assert!(stderr.contains("ara: compacted"), "{stderr}");

    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1, "{entries:?}");
    let compaction = compactions[0];
    assert_eq!(compaction["method"], json!("soft"));
    assert!(compaction["summary"].as_str().unwrap().contains("Fake summary of earlier work."), "{compaction}");
    let first_kept = compaction["firstKeptEntryId"].as_str().unwrap();
    let kept = entries.iter().find(|e| e["id"] == first_kept).expect("firstKeptEntryId exists");
    assert_eq!(kept["type"], json!("message"));
    assert_eq!(kept["message"]["role"], json!("assistant"), "native cut keeps the last assistant");
    let sources = compaction["sourceEntryIds"].as_array().unwrap();
    assert!(!sources.is_empty());
    let compaction_index = entries.iter().position(|e| e["id"] == compaction["id"]).unwrap();
    for source in sources {
        let id = source.as_str().unwrap();
        let index = entries.iter().position(|e| e["id"] == id).unwrap_or_else(|| panic!("source {id} exists"));
        assert!(index < compaction_index, "source {id} precedes the compaction entry");
    }
    assert_compaction_provenance(&entries, compaction);

    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 5, "two turns + history/prefix summaries + third turn");
    let summaries: Vec<_> = reqs[2..4]
        .iter()
        .map(|request| {
            assert!(tools_absent_or_empty(&request["body"]), "summary request has no tools: {request}");
            request_text(request["body"]["messages"].as_array().unwrap())
        })
        .collect();
    assert!(summaries.iter().any(|text| text.contains("first prompt") && text.contains("First answer.")));
    assert!(summaries.iter().any(|text| text.contains("second prompt") && !text.contains("First answer.")));
    let third = reqs[4]["body"]["messages"].as_array().unwrap();
    let third_text = request_text(third);
    assert!(third_text.contains("[Compacted summary of earlier turns"), "{third_text}");
    assert!(third_text.contains("Fake summary of earlier work."), "{third_text}");
    assert!(!third_text.contains("first prompt"), "summarized raw user text is gone: {third_text}");
    assert!(!third_text.contains("First answer."), "summarized raw answer is gone: {third_text}");
    assert!(third_text.contains("**Turn Context (split turn):**"), "{third_text}");
    assert!(!third_text.contains("second prompt"), "raw turn prefix is summarized: {third_text}");
    assert!(third_text.contains("Second answer."), "kept answer remains: {third_text}");
}

/// Native auto threshold fires after a completed turn; a disabled
/// threshold makes no summary call.
#[tokio::test(flavor = "multi_thread")]
async fn repl_auto_compact_threshold_and_disabled_companion() {
    // Auto: threshold exceeded after the second turn.
    let env = Env::new();
    let second_answer = (0..64)
        .map(|index| format!("Second answer item {index}: recorded value {}.\n", index * 17))
        .collect::<String>();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text(&second_answer), finish("stop"), done()]},
        {"events": [text("Auto summary."), finish("stop"), done()]},
        {"events": [text("Auto summary."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "128", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    assert_eq!(up.served(), 4, "two turns plus history/prefix summary calls");
    let entries = journal(&env.session_files()[0]);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1, "{entries:?}");
    assert_eq!(compactions[0]["method"], json!("soft"));
    assert!(compactions[0]["summary"].as_str().unwrap().contains("Auto summary."), "{entries:?}");

    // Companion: auto disabled, no summary call.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("ara: compacted"), "{stderr}");
    assert_eq!(up.served(), 2, "no summary call when auto is off");
    assert!(compaction_entries(&journal(&env.session_files()[0])).is_empty());
}

/// Native split-turn compaction restores its kept assistant on restart.
#[tokio::test(flavor = "multi_thread")]
async fn repl_continue_uses_the_persisted_summary() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Fake summary of earlier work."), finish("stop"), done()]},
        {"events": [text("Fake summary of earlier work."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n/compact\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let session = files[0].clone();
    assert_eq!(compaction_entries(&journal(&session)).len(), 1);

    let up2 = upstream(json!({"responses": [{"events": [text("After restart."), finish("stop"), done()]}]})).await;
    let out = repl_output(env.cmd(&up2.base_url(), &["--repl", "--continue"]), "after restart\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "After restart.\n");
    assert_eq!(env.session_files(), vec![session.clone()]);
    let entries = journal(&session);
    assert_eq!(compaction_entries(&entries).len(), 1, "no second compaction");
    assert_eq!(journal_user_texts(&entries), vec!["first prompt", "second prompt", "after restart"]);
    let reqs = up2.requests.lock().await;
    assert_eq!(reqs.len(), 1);
    let first = request_text(reqs[0]["body"]["messages"].as_array().unwrap());
    assert!(first.contains("[Compacted summary of earlier turns"), "{first}");
    assert!(first.contains("Fake summary of earlier work."), "{first}");
    assert!(!first.contains("first prompt"), "summarized raw text is not resent: {first}");
    assert!(!first.contains("second prompt"), "raw turn prefix is summarized: {first}");
    assert!(first.contains("Second answer."), "kept assistant remains: {first}");
}

/// Native compaction after cancellation: an interrupted turn does not block a later
/// `/compact` (fixed OMP summarizes aborted turns). The summary covers the
/// aborted turn, and a resumed process sends the same context the compacting
/// process built in memory.
#[tokio::test(flavor = "multi_thread")]
async fn repl_compact_summarizes_past_an_interrupted_turn() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_s", "bash", "{\"command\":\"echo started; sleep 30\"}"), finish("tool_calls"), done()]},
        {"events": [text("After the abort."), finish("stop"), done()]},
        {"events": [text("Third answer."), finish("stop"), done()]},
        {"events": [text("Summary past the abort."), finish("stop"), done()]},
        {"events": [text("Summary past the abort."), finish("stop"), done()]},
        {"events": [text("Fourth answer."), finish("stop"), done()]}
    ]}))
    .await;
    let mut c = env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]);
    let mut child = spawn_repl(&mut c);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let out = StderrLog::reading(child.stdout.take().unwrap());
    let code = tokio::task::block_in_place(|| {
        writeln!(stdin, "run something slow").unwrap();
        assert!(log.wait_for("ara: tool bash: echo started; sleep 30", Duration::from_secs(15)), "tool started");
        std::thread::sleep(Duration::from_millis(300));
        interrupt(&child);
        assert!(log.wait_for("ara: turn 1 cancelled; session kept", Duration::from_secs(10)));
        writeln!(stdin, "after").unwrap();
        writeln!(stdin, "third prompt").unwrap();
        writeln!(stdin, "/compact").unwrap();
        writeln!(stdin, "fourth prompt").unwrap();
        drop(stdin);
        wait_exit(&mut child, Duration::from_secs(20))
    });
    let stderr = log.finish();
    let stdout = out.finish();
    assert_eq!(code, Some(0), "{stderr}");
    assert!(!stderr.contains("nothing to compact"), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    assert!(stdout.ends_with("After the abort.\nThird answer.\nFourth answer.\n"), "{stdout:?}");

    let session = env.session_files()[0].clone();
    let entries = journal(&session);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1, "{entries:?}");
    assert_compaction_provenance(&entries, compactions[0]);
    let first_kept = compactions[0]["firstKeptEntryId"].as_str().unwrap();
    let kept = entries.iter().find(|e| e["id"] == first_kept).unwrap();
    assert_eq!(kept["message"]["role"], "assistant", "the native cut is past the aborted turn");
    assert_eq!(kept["message"]["content"][0]["text"], "Third answer.");
    let summarized: Vec<&Value> = compactions[0]["sourceEntryIds"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| entries.iter().find(|e| e["id"] == *id).unwrap())
        .collect();
    assert!(
        summarized.iter().any(|e| e["message"]["stopReason"] == json!("aborted")),
        "the aborted assistant is summarized: {summarized:?}"
    );

    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 6, "three turns, history/prefix summaries, one turn after them");
    let summary_text = reqs[3..5]
        .iter()
        .map(|request| request_text(request["body"]["messages"].as_array().unwrap()))
        .find(|text| text.contains("run something slow"))
        .expect("history summary");
    assert!(summary_text.contains("run something slow"), "{summary_text}");
    assert!(summary_text.contains("aborted"), "the summarizer sees the abort: {summary_text}");
    assert!(summary_text.contains("After the abort."), "{summary_text}");
    let fourth: Vec<Value> = reqs[5]["body"]["messages"].as_array().unwrap().clone();
    drop(reqs);
    let fourth_text = request_text(&fourth);
    assert!(fourth_text.contains("Summary past the abort."), "{fourth_text}");
    assert!(!fourth_text.contains("run something slow"), "{fourth_text}");
    assert!(!fourth_text.contains("third prompt") && fourth_text.contains("Third answer."), "{fourth_text}");

    // A resumed process rebuilds the same context from the journal.
    let up2 = upstream(json!({"responses": [{"events": [text("Fifth answer."), finish("stop"), done()]}]})).await;
    let out =
        repl_output(env.cmd(&up2.base_url(), &["--repl", "--continue", "--compact-threshold", "0"]), "fifth\n").await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let reqs2 = up2.requests.lock().await;
    let resumed = reqs2[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(&resumed[..fourth.len()], &fourth[..], "resume sends the in-memory compacted context");
    assert_eq!(user_texts(&resumed[fourth.len()..]), vec!["fifth"]);
    assert_eq!(resumed.len(), fourth.len() + 2, "plus the fourth answer and the new prompt");
}

/// Dogfood follow-up: a Bash call that timed out (the tool killed its process
/// group, as on a cancel) is summarized like any failed call and does not
/// block later compaction.
#[tokio::test(flavor = "multi_thread")]
async fn repl_compact_summarizes_past_a_timed_out_tool() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [tool_call(0, "call_t", "bash", "{\"command\":\"echo started; sleep 30\",\"timeout\":1}"), finish("tool_calls"), done()]},
        {"events": [text("After the timeout."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Summary past the timeout."), finish("stop"), done()]},
        {"events": [text("Summary past the timeout."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "build it\nsecond prompt\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");

    let entries = journal(&env.session_files()[0]);
    let result = entries.iter().find(|e| e["message"]["toolCallId"] == "call_t").expect("tool result");
    assert_eq!(result["message"]["details"]["timedOut"], json!(true), "{result}");
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1, "{entries:?}");
    let first_kept = compactions[0]["firstKeptEntryId"].as_str().unwrap();
    let kept = entries.iter().find(|e| e["id"] == first_kept).unwrap();
    assert_eq!(kept["message"]["role"], "assistant", "the native cut is past the timed-out turn");
    assert_eq!(kept["message"]["content"][0]["text"], "Second answer.");

    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 5, "two turns (three calls) and history/prefix summaries");
    let summary_text = reqs[3..5]
        .iter()
        .map(|request| request_text(request["body"]["messages"].as_array().unwrap()))
        .find(|text| text.contains("build it"))
        .expect("history summary");
    assert!(summary_text.contains("[Command timed out after 1 seconds]"), "{summary_text}");
    assert!(summary_text.contains("\"unknown_effect\":false"), "{summary_text}");
}

/// Native compaction can repeat after new work; failed/cancelled summaries,
/// missing Sessions and histories too small to cut leave the Session usable.
#[tokio::test(flavor = "multi_thread")]
async fn repl_compact_failure_paths_leave_the_session_usable() {
    // A second /compact carries the prior summary and the assistant-leading
    // projected window once a new turn makes a later cut available.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]},
        {"events": [text("Third answer."), finish("stop"), done()]},
        {"events": [text("Updated summary."), finish("stop"), done()]},
        {"events": [text("Updated summary."), finish("stop"), done()]},
        {"events": [text("After repeated compact."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n/compact\nthird prompt\n/compact\nafter repeated compact\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "First answer.\nSecond answer.\nThird answer.\nAfter repeated compact.\n");
    assert_eq!(stderr.matches("ara: compacted").count(), 2, "{stderr}");
    assert_eq!(up.served(), 8, "four turns and two pairs of summary calls");
    let entries = journal(&env.session_files()[0]);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 2);
    for compaction in &compactions {
        assert_compaction_provenance(&entries, compaction);
    }
    let earlier = compactions[0]["sourceEntryIds"].as_array().unwrap();
    let later = compactions[1]["sourceEntryIds"].as_array().unwrap();
    assert!(later.starts_with(earlier), "cumulative raw provenance survives repeated compaction");
    let reqs = up.requests.lock().await;
    let summaries: Vec<_> =
        reqs[5..7].iter().map(|request| request_text(request["body"]["messages"].as_array().unwrap())).collect();
    assert!(
        summaries.iter().any(|text| text.contains("Fake summary.") && text.contains("Second answer.")),
        "{summaries:?}"
    );
    assert!(summaries.iter().any(|text| text.contains("third prompt")), "{summaries:?}");
    let follow_up = request_text(reqs[7]["body"]["messages"].as_array().unwrap());
    assert!(follow_up.contains("Updated summary.") && follow_up.contains("Third answer."), "{follow_up}");
    assert!(!follow_up.contains("third prompt") && !follow_up.contains("Second answer."), "{follow_up}");
    drop(reqs);

    // Failed summary call: session bytes unchanged by /compact; the next turn
    // is answered on a later resume.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let session = env.session_files()[0].clone();
    let before = std::fs::read(&session).unwrap();
    let up2 = upstream(json!({"responses": [
        {"status": 400, "delay_ms": 200, "body": "{\"error\":{\"message\":\"summary backend down\"}}"},
        {"events": [text("Unadopted sibling summary."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up2.base_url(), &["--repl", "--continue", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "/compact\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("session untouched"), "{stderr}");
    assert!(stderr.contains("HTTP 400") && stderr.contains("summary backend down"), "provider cause shown: {stderr}");
    assert_eq!(std::fs::read(&session).unwrap(), before, "failed summary left the journal untouched");
    assert!(compaction_entries(&journal(&session)).is_empty());

    let up3 = upstream(json!({"responses": [{"events": [text("Still works."), finish("stop"), done()]}]})).await;
    let out = repl_output(env.cmd(&up3.base_url(), &["--repl", "--continue"]), "still works\n").await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "Still works.\n");
    assert_eq!(journal_user_texts(&journal(&session)), vec!["first prompt", "second prompt", "still works"]);
    assert!(compaction_entries(&journal(&session)).is_empty());

    // Failed summary call, next turn in the same process: the in-memory
    // history survives and the journal only grows.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n",
    )
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", text_of(&out).1);
    let session = env.session_files()[0].clone();
    let before = std::fs::read(&session).unwrap();
    let up2 = upstream(json!({"responses": [
        {"status": 400, "delay_ms": 200, "body": "{\"error\":{\"message\":\"summary backend down\"}}"},
        {"events": [text("Unadopted sibling summary."), finish("stop"), done()]},
        {"events": [text("Next answer."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up2.base_url(), &["--repl", "--continue", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "/compact\nnext turn\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("session untouched"), "{stderr}");
    assert_eq!(stdout, "Next answer.\n");
    assert_eq!(up2.served(), 3, "failed/sibling summary calls and one follow-up turn");
    let after = std::fs::read(&session).unwrap();
    assert!(after.starts_with(&before), "failed summary rewrote earlier journal bytes");
    let entries = journal(&session);
    assert!(compaction_entries(&entries).is_empty(), "{entries:?}");
    assert_eq!(journal_user_texts(&entries), vec!["first prompt", "second prompt", "next turn"]);
    let reqs = up2.requests.lock().await;
    let follow_up = request_text(reqs[2]["body"]["messages"].as_array().unwrap());
    for kept in ["first prompt", "First answer.", "second prompt", "Second answer."] {
        assert!(follow_up.contains(kept), "{kept} survives the failed summary: {follow_up}");
    }
    assert!(!follow_up.contains("[Compacted summary of earlier turns"), "{follow_up}");
    drop(reqs);

    // /compact under --no-session is refused with no model call.
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("Hello."), finish("stop"), done()]}]})).await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--no-session", "--compact-threshold", "0"]),
        "hello\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("compaction skipped: it needs a session"), "{stderr}");
    assert_eq!(up.served(), 1, "no summary call under --no-session");

    // One turn can split before its assistant: only the turn-prefix branch
    // calls the model, and native's explicit no-history marker is persisted.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("Only answer."), finish("stop"), done()]},
        {"events": [text("Only prompt context."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "only prompt\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    assert_eq!(up.served(), 2, "one turn and one prefix-only summary call");
    let entries = journal(&env.session_files()[0]);
    let compactions = compaction_entries(&entries);
    assert_eq!(compactions.len(), 1);
    assert_compaction_provenance(&entries, compactions[0]);
    assert_eq!(
        compactions[0]["summary"],
        "No prior history.\n\n---\n\n**Turn Context (split turn):**\n\nOnly prompt context."
    );

    // History within the keep target: no earlier prefix, no model call.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("One."), finish("stop"), done()]},
        {"events": [text("Two."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "100000"]),
        "first\nsecond\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("no earlier message prefix can be summarized at this retention target"), "{stderr}");
    assert_eq!(up.served(), 2, "no summary call within the target");

    // Auto-compaction explains a refusal once per process; /compact always does.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("One."), finish("stop"), done()]},
        {"events": [text("Two."), finish("stop"), done()]},
        {"events": [text("Three."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--no-session", "--compact-threshold", "1"]),
        "first\nsecond\nthird\n/compact\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "One.\nTwo.\nThree.\n");
    assert_eq!(stderr.matches("ara: auto-compaction skipped: it needs a session").count(), 1, "{stderr}");
    assert_eq!(stderr.matches("ara: compaction skipped: it needs a session").count(), 1, "{stderr}");
    assert_eq!(up.served(), 3, "no summary call under --no-session");
}

/// History/prefix calls get OMP's distinct default output budgets,
/// floor(0.8 * 16384) and floor(0.5 * 16384), capped by `--max-tokens`.
/// Nonempty Length summaries are accepted; an unchanged projected window
/// with only the kept assistant has no later cut to summarize.
#[tokio::test(flavor = "multi_thread")]
async fn repl_compact_uses_the_omp_summary_budget_and_accepts_nonempty_length() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Cut off sum"), finish("length"), usage(900, 1000), done()]},
        {"events": [text("Cut off sum"), finish("length"), usage(900, 1000), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1"]),
        "first prompt\nsecond prompt\n/compact\n/compact\n",
    )
    .await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert_eq!(stdout, "First answer.\nSecond answer.\n");
    assert!(stderr.contains("ara: compacted"), "the first Length summary succeeds: {stderr}");
    assert!(stderr.contains("no earlier message prefix can be summarized at this retention target"), "{stderr}");
    assert_eq!(up.served(), 4);
    let reqs = up.requests.lock().await;
    let mut budgets: Vec<_> =
        reqs[2..4].iter().map(|request| request["body"]["max_tokens"].as_u64().unwrap()).collect();
    budgets.sort_unstable();
    assert_eq!(budgets, [8192, 13107]);
    drop(reqs);
    let rows = journal(&env.session_files()[0]);
    let summaries = compaction_entries(&rows);
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["summary"], "Cut off sum\n\n---\n\n**Turn Context (split turn):**\n\nCut off sum");

    // --max-tokens caps the summary budget.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(
            &up.base_url(),
            &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1", "--max-tokens", "700"],
        ),
        "first prompt\nsecond prompt\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 4);
    for request in &reqs[2..4] {
        assert_eq!(request["body"]["max_tokens"], 700, "{request}");
    }

    // A --max-tokens above the default does not raise the summary budget.
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]},
        {"events": [text("Fake summary."), finish("stop"), done()]}
    ]}))
    .await;
    let out = repl_output(
        env.cmd(
            &up.base_url(),
            &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1", "--max-tokens", "100000"],
        ),
        "first prompt\nsecond prompt\n/compact\n",
    )
    .await;
    let (_stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("ara: compacted"), "{stderr}");
    let reqs = up.requests.lock().await;
    assert_eq!(reqs.len(), 4);
    let mut budgets: Vec<_> =
        reqs[2..4].iter().map(|request| request["body"]["max_tokens"].as_u64().unwrap()).collect();
    budgets.sort_unstable();
    assert_eq!(budgets, [8192, 13107]);
}

/// A prompt line that is not UTF-8 ends the REPL with exit 1 and a message;
/// the Session keeps the turns before it.
#[tokio::test(flavor = "multi_thread")]
async fn repl_unreadable_prompt_exits_1_and_keeps_the_session() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("First answer."), finish("stop"), done()]}]})).await;
    let mut c = env.cmd(&up.base_url(), &["--repl"]);
    let out = tokio::task::spawn_blocking(move || {
        let mut child = spawn_repl(&mut c);
        child.stdin.take().unwrap().write_all(b"first prompt\n\xff\xfe not text\nnot sent\n").unwrap();
        child.wait_with_output().unwrap()
    })
    .await
    .unwrap();
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(stdout, "First answer.\n");
    assert!(stderr.contains("ara: cannot read the next prompt ("), "{stderr}");
    assert!(stderr.contains("session kept"), "{stderr}");
    assert_eq!(up.served(), 1);
    let entries = journal(&env.session_files()[0]);
    assert_eq!(journal_user_texts(&entries), vec!["first prompt"]);
}

/// Native compaction: interrupt during parallel summary calls returns to the prompt, leaves
/// no compaction entry, and the next line is answered.
#[tokio::test(flavor = "multi_thread")]
async fn repl_interrupt_during_compact_leaves_no_compaction_entry() {
    let env = Env::new();
    let up = upstream(json!({"responses": [
        {"events": [text("First answer."), finish("stop"), done()]},
        {"events": [text("Second answer."), finish("stop"), done()]},
        {"events": [{"data": {"choices": [{"delta": {"content": "partial"}}]}}, {"sleep_ms": 30000}], "end": "hang"},
        {"events": [{"data": {"choices": [{"delta": {"content": "partial"}}]}}, {"sleep_ms": 30000}], "end": "hang"},
        {"events": [text("After compact."), finish("stop"), done()]}
    ]}))
    .await;
    let mut c = env
        .cmd(&up.base_url(), &["--repl", "--compact-threshold", "0", "--compact-keep-tokens", "1", "--mode", "json"]);
    let mut child = spawn_repl(&mut c);
    let mut stdin = child.stdin.take().unwrap();
    let log = StderrLog::start(&mut child);
    let reader = BufReader::new(child.stdout.take().unwrap());
    let started = Instant::now();
    let code = tokio::task::block_in_place(|| {
        writeln!(stdin, "first prompt").unwrap();
        writeln!(stdin, "second prompt").unwrap();
        writeln!(stdin, "/compact").unwrap();
        // Wait until both turns finished and both summary requests are in flight.
        let t0 = Instant::now();
        while up.served() < 4 && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(up.served(), 4, "history and prefix summary requests started");
        interrupt(&child);
        assert!(log.wait_for("session untouched", Duration::from_secs(8)), "summary cancel reported");
        writeln!(stdin, "after compact").unwrap();
        drop(stdin);
        let _rest: Vec<String> = reader.lines().map(|l| l.unwrap()).collect();
        wait_exit(&mut child, Duration::from_secs(15))
    });
    let stderr = log.finish();
    assert!(started.elapsed() < Duration::from_secs(25), "{:?}", started.elapsed());
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stderr.contains("ara: interrupt received, aborting"), "{stderr}");
    assert!(stderr.contains("session untouched"), "{stderr}");
    assert_eq!(up.served(), 5, "the follow-up turn is answered; cancelled summaries are not replayed");
    let entries = journal(&env.session_files()[0]);
    assert!(compaction_entries(&entries).is_empty(), "{entries:?}");
    // The follow-up turn is in the journal.
    assert_eq!(journal_user_texts(&entries), vec!["first prompt", "second prompt", "after compact"]);
    // The follow-up request still carries the whole in-process history.
    let reqs = up.requests.lock().await;
    let follow_up = request_text(reqs[4]["body"]["messages"].as_array().unwrap());
    for kept in ["first prompt", "First answer.", "second prompt", "Second answer."] {
        assert!(follow_up.contains(kept), "{kept} survives the cancelled summary: {follow_up}");
    }
    assert!(!follow_up.contains("[Compacted summary of earlier turns"), "{follow_up}");
}

const SWITCH_SKILL_NAMES: [&str; 9] = [
    "agents-global",
    "agents-project",
    "claude-global",
    "claude-project",
    "codex-global",
    "codex-project",
    "opencode-global",
    "opencode-project",
    "native-proof",
];

fn compatibility_skill_fixtures(env: &Env) {
    // The isolated HOME and work directories are siblings. Without a repo
    // boundary, ancestor Skill discovery could reach the real user's profile.
    std::fs::create_dir_all(env.work.path().join(".git")).unwrap();
    for (root, directory, name) in [
        (env._home.path(), ".agents/skills", "agents-global"),
        (env.work.path(), ".agents/skills", "agents-project"),
        (env._home.path(), ".claude/skills", "claude-global"),
        (env.work.path(), ".claude/skills", "claude-project"),
        (env._home.path(), ".codex/skills", "codex-global"),
        (env.work.path(), ".codex/skills", "codex-project"),
        (env._home.path(), ".config/opencode/skills", "opencode-global"),
        (env.work.path(), ".opencode/skills", "opencode-project"),
        (env.work.path(), ".ara/skills", "native-proof"),
    ] {
        let path = root.join(directory).join(name).join("SKILL.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("---\ndescription: Switch fixture {name}\n---\nSOURCE-BODY {name}\n")).unwrap();
    }
    std::fs::write(env.work.path().join(".claude/CLAUDE.md"), "SKILL-SWITCH-INDEPENDENT-CONTEXT").unwrap();
}

async fn assert_cli_skill_source_case(case_name: &str, args: &[&str], enabled: &[&str], reads: &[(&str, bool)]) {
    let env = Env::new();
    compatibility_skill_fixtures(&env);
    let mut responses: Vec<Value> = reads
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            json!({"events": [
                tool_call(0, &format!("source-read-{index}"), "read", &json!({"path":format!("skill://{name}")}).to_string()),
                finish("tool_calls"), done()
            ]})
        })
        .collect();
    responses.push(json!({"events": [text("Sources verified."), finish("stop"), done()]}));
    let up = upstream(json!({"responses": responses})).await;
    let mut all_args = args.to_vec();
    all_args.extend(["-p", "Inspect the selected skills"]);
    let out = output(env.cmd(&up.base_url(), &all_args)).await;
    let (stdout, stderr) = text_of(&out);
    assert_eq!(out.status.code(), Some(0), "{args:?}: {stderr}");
    assert_eq!(stdout, "Sources verified.\n");
    assert_eq!(up.served(), reads.len() + 1);
    let system = first_system(&up).await;
    let mut actual_enabled = Vec::new();
    if let Some((_, tail)) = system.split_once("<skills>") {
        let (listing, _) = tail.split_once("</skills>").expect("closed Skills listing");
        for line in listing.lines().map(str::trim).filter(|line| !line.is_empty()) {
            let (name, description) = line.strip_prefix("- ").unwrap().split_once(": ").unwrap();
            assert!(SWITCH_SKILL_NAMES.contains(&name), "non-fixture Skill discovered: {line}");
            assert_eq!(description, format!("Switch fixture {name}"));
            actual_enabled.push(name);
        }
    }
    actual_enabled.sort_unstable();
    let mut expected_enabled = enabled.to_vec();
    expected_enabled.sort_unstable();
    assert_eq!(actual_enabled, expected_enabled, "{args:?}: exact Skills listing: {system}");
    assert!(system.contains("SKILL-SWITCH-INDEPENDENT-CONTEXT"), "context discovery remains independent");
    let files = env.session_files();
    assert_eq!(files.len(), 1);
    let entries = journal(&files[0]);
    let receipts: Vec<_> = entries.iter().filter(|entry| entry["message"]["role"] == "toolResult").collect();
    assert_eq!(receipts.len(), reads.len());
    let requests = up.requests.lock().await;
    for (index, (name, succeeds)) in reads.iter().enumerate() {
        let id = format!("source-read-{index}");
        let receipt = receipts.iter().find(|entry| entry["message"]["toolCallId"] == id).unwrap();
        assert_eq!(receipt["message"]["isError"], json!(!succeeds), "{args:?}: {name}: {receipt}");
        let content = receipt["message"]["content"][0]["text"].as_str().unwrap();
        let expected = if *succeeds { format!("SOURCE-BODY {name}") } else { format!("Unknown skill: {name}") };
        assert!(content.contains(&expected), "{args:?}: {name}: {content}");
        if !succeeds {
            let available = if actual_enabled.is_empty() { "none".to_string() } else { actual_enabled.join(", ") };
            let observed: Vec<_> = content.lines().filter_map(|line| line.strip_prefix("Available: ")).collect();
            assert_eq!(observed, [available.as_str()], "{args:?}: {name}: exact available Skills: {content}");
        }
        let messages = requests[index + 1]["body"]["messages"].as_array().unwrap();
        let carried =
            messages.iter().find(|message| message["role"] == "tool" && message["tool_call_id"] == id).unwrap();
        assert_eq!(carried["content"], json!(content), "the actual receipt reaches the next model request");
    }
    if let Some(directory) = std::env::var_os("ARA_SKILL_SOURCE_RECEIPTS") {
        let directory = PathBuf::from(directory).join(case_name);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("stdout.txt"), &stdout).unwrap();
        std::fs::write(directory.join("stderr.txt"), &stderr).unwrap();
        std::fs::copy(&files[0], directory.join("session.jsonl")).unwrap();
        std::fs::write(directory.join("requests.json"), serde_json::to_vec_pretty(&*requests).unwrap()).unwrap();
        let read_results: Vec<_> = reads
            .iter()
            .enumerate()
            .map(|(index, (name, expected))| {
                let id = format!("source-read-{index}");
                let receipt = receipts.iter().find(|entry| entry["message"]["toolCallId"] == id).unwrap();
                json!({
                    "skill": name, "toolCallId": id, "expectedSuccess": expected,
                    "actualSuccess": !receipt["message"]["isError"].as_bool().unwrap(),
                    "content": receipt["message"]["content"][0]["text"]
                })
            })
            .collect();
        std::fs::write(
            directory.join("summary.json"),
            serde_json::to_vec_pretty(&json!({
                "status": "PASS", "case": case_name, "arguments": args,
                "exitCode": out.status.code(), "expectedEnabled": expected_enabled, "actualEnabled": actual_enabled,
                "reads": read_results, "modelRequests": requests.len(), "servedRequests": up.served(),
                "toolCalls": receipts.len(), "sessions": files.len(), "contextPreserved": true
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

#[tokio::test]
async fn skill_sources_default_lists_three_compatibility_paths_and_resolves_their_user_skills() {
    assert_cli_skill_source_case(
        "default",
        &[],
        &SWITCH_SKILL_NAMES[..6].iter().copied().chain(["native-proof"]).collect::<Vec<_>>(),
        &[("codex-global", true), ("opencode-project", false)],
    )
    .await;
}

#[tokio::test]
async fn skill_sources_explicit_opencode_selection_enables_both_paths_and_removes_other_sources() {
    assert_cli_skill_source_case(
        "opencode",
        &["--skill-sources", "opencode"],
        &["opencode-global", "opencode-project", "native-proof"],
        &[("opencode-global", true), ("agents-project", false)],
    )
    .await;
}

#[tokio::test]
async fn skill_sources_empty_keeps_native_but_no_skills_disables_every_skill() {
    assert_cli_skill_source_case(
        "empty",
        &["--skill-sources", ""],
        &["native-proof"],
        &[("native-proof", true), ("claude-global", false)],
    )
    .await;
    assert_cli_skill_source_case(
        "no-skills",
        &["--skill-sources", "agents,claude,codex,opencode", "--no-skills"],
        &[],
        &[("native-proof", false), ("opencode-project", false)],
    )
    .await;
}

#[tokio::test]
async fn skill_sources_and_name_filter_both_control_listing_and_url_resolution() {
    assert_cli_skill_source_case(
        "name-filter",
        &["--skill-sources", "codex", "--skills", "*-project"],
        &["codex-project"],
        &[("codex-project", true), ("codex-global", false), ("agents-project", false)],
    )
    .await;
}

#[tokio::test]
async fn skill_sources_invalid_selection_exits_two_without_a_model_request_or_session() {
    let env = Env::new();
    let up = upstream(json!({"responses": [{"events": [text("must not run"), finish("stop"), done()]}]})).await;
    for (index, invalid) in ["unknown", "agents,unknown", "Agents"].iter().enumerate() {
        let out = output(env.cmd(&up.base_url(), &["--skill-sources", invalid, "-p", "must not run"])).await;
        assert_eq!(out.status.code(), Some(2), "{}", text_of(&out).1);
        let (stdout, stderr) = text_of(&out);
        assert!(stdout.is_empty());
        assert!(stderr.contains("--skill-sources") && stderr.contains("unknown"), "{stderr}");
        assert_eq!(up.served(), 0);
        assert!(up.requests.lock().await.is_empty());
        assert!(env.session_files().is_empty());
        if let Some(directory) = std::env::var_os("ARA_SKILL_SOURCE_RECEIPTS") {
            let directory = PathBuf::from(directory).join(format!("invalid-{index}"));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("stdout.txt"), &stdout).unwrap();
            std::fs::write(directory.join("stderr.txt"), &stderr).unwrap();
            std::fs::write(
                directory.join("summary.json"),
                serde_json::to_vec_pretty(&json!({
                    "status": "PASS", "case": format!("invalid-{index}"), "sourceArgument": invalid,
                    "exitCode": out.status.code(), "modelRequests": up.served(), "sessions": env.session_files().len()
                }))
                .unwrap(),
            )
            .unwrap();
        }
    }
}
