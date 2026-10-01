//! Fixed OMP whole-module recovery flows through the actual RPC child.
use super::*;

const LOOP_UNIT: &str = "I am reviewing the source and verifying the same result repeatedly. The source and result are unchanged while this sentence keeps repeating and provides no new information. ";

fn loop_response() -> Value {
    json!({"events":[
        thinking(&LOOP_UNIT.repeat(12)),
        finish("stop"), done()
    ]})
}

fn thinking(delta: &str) -> Value {
    json!({"data":{"choices":[{"index":0,"delta":{"reasoning_content":delta},"finish_reason":null}]}})
}

fn header_response() -> Value {
    let headers = (0..36).map(|index| format!("**Stage {index}**\n")).collect::<String>();
    json!({"events":[thinking(&headers), {"sleep_ms":200}, finish("stop"), done()]})
}

fn native_notices(entries: &[Value], kind: &str) -> Vec<Value> {
    entries.iter().filter(|entry| entry["type"] == "custom_message" && entry["customType"] == kind).cloned().collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_loop_retries_keep_both_notices_actual_effect_receipts_and_restart_projection() {
    let env = Env::new();
    config(&env, true, 2.0, 0.0, 300_000.0);
    let up = upstream(vec![
        loop_response(),
        loop_response(),
        tool("printf 'recovered\\n' > loop-effect.txt", "one-effect"),
        answer("Recovered"),
        answer("Original Session reopened"),
    ])
    .await;
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "bash"]);
    child.ready();
    child.prompt("loop-task", "Create loop-effect.txt once after recovery");
    let start = child.until(|frame| frame["type"] == "auto_retry_start");
    assert_ne!(start["errorId"].as_u64().unwrap() & 0x10000, 0);
    let original_session = child.state("active-session")["sessionId"].clone();
    let end = child.until(|frame| frame["type"] == "auto_retry_end");
    assert_eq!(end["success"], true, "{end}");
    assert_eq!(end["attempt"], 2);
    assert_eq!(child.state("settled-session")["sessionId"], original_session);
    let file = session_file(&mut child, "loop-journal");
    let entries = journal(&file);
    let notices = native_notices(&entries, "thinking-loop-redirect");
    assert_eq!(notices.len(), 2, "Fixed OMP preserves one notice per retried loop");
    assert!(notices.iter().all(|notice| notice["display"] == false && notice["attribution"] == "agent"));
    assert_eq!(notices[0]["content"], notices[1]["content"]);
    assert_eq!(end["retryErrors"].as_array().unwrap().len(), 2);
    for receipt in end["retryErrors"].as_array().unwrap() {
        assert!(entries.iter().any(|entry| entry["id"] == receipt["entryId"]));
    }
    assert_eq!(std::fs::read_to_string(env.work.path().join("loop-effect.txt")).unwrap(), "recovered\n");
    assert_eq!(entries.iter().filter(|entry| entry["message"]["toolCallId"] == "one-effect").count(), 1);
    child.finish();
    assert_eq!(up.served(), 4, "Loop retries use the Session budget, without a hidden provider retry");
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", file.to_str().unwrap(), "--tools", "bash"]);
    reopened.ready();
    assert_eq!(reopened.state("reopened-id")["sessionId"], original_session);
    reopened.run("reopened-task", "Recall the preceding recovery without repeating the write");
    reopened.finish();
    assert_eq!(up.served(), 5);
    let requests = up.requests.lock().await;
    for (index, expected_notices) in [(0, 0), (1, 1), (2, 2), (4, 2)] {
        let messages = requests[index]["body"]["messages"].as_array().unwrap();
        let projected =
            messages.iter().filter(|message| message["content"].to_string().contains("thinking_loop_detected")).count();
        assert_eq!(projected, expected_notices, "request {index}: {messages:?}");
        assert!(!messages.iter().any(|message| message["content"].to_string().contains("Thinking loop detected")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_guard_abort_and_exhaustion_settle_without_an_extra_request() {
    for mode in ["disabled", "abort", "exhausted"] {
        let env = Env::new();
        config(&env, true, 1.0, if mode == "abort" { 5_000.0 } else { 0.0 }, 300_000.0);
        if mode == "disabled" {
            let path = env.home.path().join("agent/config.yml");
            let mut settings = std::fs::read_to_string(&path).unwrap();
            settings.push_str("model:\n  loopGuard:\n    enabled: false\n");
            std::fs::write(path, settings).unwrap();
        }
        let up = upstream(vec![loop_response(), loop_response(), answer("must not execute")]).await;
        let mut child = RpcChild::spawn(&env, &up, &[]);
        child.ready();
        child.prompt(mode, "Observe the bounded recovery lifecycle");
        if mode == "disabled" {
            child.until(|frame| frame["type"] == "agent_end");
            assert_no_retry(&child);
        } else {
            child.until(|frame| frame["type"] == "auto_retry_start");
            if mode == "abort" {
                child.send(json!({"id":"cancel-loop","type":"abort_retry"}));
                child.success("cancel-loop");
            }
            let end = child
                .seen
                .iter()
                .find(|frame| frame["type"] == "auto_retry_end")
                .cloned()
                .unwrap_or_else(|| child.until(|frame| frame["type"] == "auto_retry_end"));
            assert_eq!(end["success"], false, "{end}");
        }
        let entries = journal(&session_file(&mut child, "bounded-loop-file"));
        let notices = native_notices(&entries, "thinking-loop-redirect");
        // Fixed turn-recovery injects the redirect before awaiting backoff;
        // cancelling that wait retains its already-recorded notice.
        assert_eq!(notices.len(), usize::from(mode != "disabled"));
        assert_eq!(child.state("bounded-loop-state")["isRetrying"], false);
        child.finish();
        assert_eq!(up.served(), if mode == "exhausted" { 2 } else { 1 });
        if mode == "exhausted" {
            let failures = failed_entries(&entries);
            assert_eq!(failures.len(), 2);
            assert!(failures[1]["message"].get("retryRecovery").is_none());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gemini_header_reminder_continues_original_session_and_switch_disables_it() {
    for enabled in [true, false] {
        let mut env = Env::new();
        env.model = "google/gemini-3.5-flash";
        config(&env, true, 1.0, 0.0, 300_000.0);
        if !enabled {
            let path = env.home.path().join("agent/config.yml");
            let mut settings = std::fs::read_to_string(&path).unwrap();
            settings.push_str("model:\n  loopGuard:\n    toolCallReminder: false\n");
            std::fs::write(path, settings).unwrap();
        }
        let up = upstream(vec![
            header_response(),
            tool("printf 'header-recovered\\n' > header-effect.txt", "header-effect"),
            answer("Tool issued after header reminder"),
        ])
        .await;
        let mut child = RpcChild::spawn(&env, &up, &["--tools", "bash"]);
        child.ready();
        let session = child.state("header-original")["sessionId"].clone();
        child.prompt("header-task", "Issue the requested concrete tool action");
        if enabled {
            // Observe the warning while the original Run is still active.
            let warning = child.until(|frame| frame["type"] == "notice");
            assert!(warning.to_string().contains("36"), "{warning}");
            child.until(|frame| frame["type"] == "tool_execution_end");
            child.until(|frame| frame["type"] == "agent_end");
            assert_eq!(
                std::fs::read_to_string(env.work.path().join("header-effect.txt")).unwrap(),
                "header-recovered\n"
            );
        } else {
            child.until(|frame| frame["type"] == "agent_end");
            assert!(!env.work.path().join("header-effect.txt").exists());
        }
        assert_eq!(child.state("header-final")["sessionId"], session);
        let entries = journal(&session_file(&mut child, "header-file"));
        let notices = native_notices(&entries, "gemini-tool-call-reminder");
        assert_eq!(notices.len(), usize::from(enabled));
        if enabled {
            assert_eq!(notices[0]["details"]["headers"], 36);
            assert_eq!(notices[0]["attribution"], "agent");
            assert_eq!(notices[0]["display"], false);
        }
        child.finish();
        assert_eq!(up.served(), if enabled { 3 } else { 1 });
        assert_no_retry(&child);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aborting_gemini_continuation_before_response_prevents_tool_effect_and_new_session_stays_empty() {
    let mut env = Env::new();
    env.model = "google/gemini-3.5-flash";
    config(&env, true, 1.0, 0.0, 300_000.0);
    let up = upstream(vec![
        header_response(),
        tool("printf 'must-not-run' > stale-header-effect.txt", "stale-effect"),
        answer("must not run"),
    ])
    .await;
    let mut gate = HttpGate::start(&up, 1).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &["--tools", "bash"]);
    child.ready();
    let original = child.state("cancel-header-original")["sessionId"].clone();
    child.prompt("cancel-header-task", "Observe the cancellable post-header continuation");
    child.until(|frame| frame["type"] == "notice");
    gate.reached(env.deadline).await;
    child.send(json!({"id":"abort-header-run","type":"abort"}));
    child.success("abort-header-run");
    assert_eq!(child.state("header-aborted")["isStreaming"], false);
    child.send(json!({"id":"replace-header-session","type":"new_session"}));
    child.success("replace-header-session");
    assert_ne!(child.state("new-header-session")["sessionId"], original);
    assert!(child.messages("new-header-messages").is_empty());
    child.finish();
    assert_eq!(up.served(), 1, "Withheld continuation never reaches a model response or tool");
    assert!(!env.work.path().join("stale-header-effect.txt").exists());
    assert_no_retry(&child);
}
