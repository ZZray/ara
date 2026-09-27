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
    let tag = pi_edit::store::file_hash("hi from ara\n");
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
    let displayed_path = work.canonicalize().unwrap().join("AGENTS.md").display().to_string().replace('\\', "/");
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
    std::fs::write(user_dir.join("SYSTEM.md"), "USER-SYSTEM-MARKER").unwrap();
    std::fs::write(work.join(".ara/SYSTEM.md"), "PROJECT-SYSTEM-MARKER").unwrap();
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

    let (system, _) = run(vec![]).await;
    assert_eq!(system.matches("PROJECT-SYSTEM-MARKER").count(), 1, "{system}");
    assert!(!system.contains("USER-SYSTEM-MARKER") && !system.contains("coding harness"), "{system}");
    assert_eq!(system.matches("APPEND-MARKER").count(), 1, "{system}");

    std::fs::remove_file(work.join(".ara/SYSTEM.md")).unwrap();
    let (system, _) = run(vec![]).await;
    assert!(system.contains("USER-SYSTEM-MARKER") && !system.contains("coding harness"), "{system}");

    let (system, _) = run(vec!["--system-prompt", "FIRST-FLAG", "--system-prompt", "FLAG-SYSTEM-MARKER"]).await;
    assert!(system.contains("FLAG-SYSTEM-MARKER"), "{system}");
    assert!(!system.contains("FIRST-FLAG") && !system.contains("USER-SYSTEM-MARKER"), "{system}");
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
