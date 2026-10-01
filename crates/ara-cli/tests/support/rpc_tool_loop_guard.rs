//! Fixed OMP primary tool-loop flows through the existing real RPC harness.
use super::*;

const KIND: &str = "tool-call-loop-redirect";
const REPEATED: &str = "printf 'x' >> loop-effects.txt";

fn tool_policy(env: &Env, enabled: bool, threshold: f64, exempt: &[&str]) {
    config(env, true, 0.0, 0.0, 300_000.0);
    let path = env.home.path().join("agent/config.yml");
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str(&format!(
        "model:\n  toolCallLoopGuard:\n    enabled: {enabled}\n    threshold: {threshold}\n    exemptTools: {}\n",
        json!(exempt)
    ));
    std::fs::write(path, content).unwrap();
}

fn notices(rows: &[Value]) -> Vec<&Value> {
    rows.iter().filter(|row| row["type"] == "custom_message" && row["customType"] == KIND).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_tools_commit_notice_before_steering_and_keep_original_restart() {
    let env = Env::new();
    tool_policy(&env, true, 2.0, &["hub"]);
    let up = upstream(vec![
        tool(REPEATED, "repeat-1"),
        tool(REPEATED, "repeat-2"),
        tool("printf 'guard-accepted' > outcome.txt", "different-call"),
        answer("Finished"),
        answer("Original Session reopened"),
    ])
    .await;
    let mut gate = HttpGate::start(&up, 1).await;
    let mut child = RpcChild::spawn_url(&env, &gate.url, &["--tools", "bash"]);
    child.ready();
    let original = child.state("original")["sessionId"].clone();
    child.prompt("loop-task", "Finish the bounded task and write outcome.txt");
    gate.reached(env.deadline).await;
    child.send(
        json!({"id":"queued-steer","type":"prompt","message":"queued steering marker", "streamingBehavior":"steer"}),
    );
    child.success("queued-steer");
    gate.release();
    child.until(|frame| frame["type"] == "agent_end");
    let path = session_file(&mut child, "path");
    let rows = journal(&path);
    let native = notices(&rows);
    assert_eq!(native.len(), 1);
    assert_eq!(native[0]["display"], false);
    assert_eq!(native[0]["attribution"], "agent");
    assert_eq!(native[0]["details"]["toolName"], "bash");
    assert_eq!(native[0]["details"]["count"], 2);
    let notice_index = rows.iter().position(|row| row["customType"] == KIND).unwrap();
    let steer_index = rows.iter().position(|row| row["message"]["content"] == "queued steering marker").unwrap();
    assert!(notice_index < steer_index, "Durable order agrees with the running Agent");
    assert_eq!(std::fs::read_to_string(env.work.path().join("loop-effects.txt")).unwrap(), "xx");
    assert_eq!(std::fs::read_to_string(env.work.path().join("outcome.txt")).unwrap(), "guard-accepted");
    for id in ["repeat-1", "repeat-2", "different-call"] {
        assert_eq!(rows.iter().filter(|row| row["message"]["toolCallId"] == id).count(), 1);
    }
    child.finish();
    let mut reopened = RpcChild::spawn(&env, &up, &["--resume", path.to_str().unwrap(), "--tools", ""]);
    reopened.ready();
    assert_eq!(reopened.state("reopened")["sessionId"], original);
    reopened.run("recall", "Recall the preceding result using the conversation only");
    reopened.finish();
    assert_eq!(up.served(), 5);
    let requests = up.requests.lock().await;
    let next = requests[2]["body"]["messages"].as_array().unwrap();
    let redirect =
        next.iter().position(|message| message["content"].to_string().contains("tool_call_loop_detected")).unwrap();
    let steer = next.iter().position(|message| message["content"] == "queued steering marker").unwrap();
    assert!(redirect < steer, "Notice is committed before user steering in the request too");
    assert!(next[redirect]["content"].as_str().unwrap().contains("identical arguments"));
    assert_eq!(requests[4]["body"]["tools"].as_array().map_or(0, Vec::len), 0);
    assert!(
        requests[4]["body"]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| { message["content"].to_string().contains("tool_call_loop_detected") })
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn policy_and_terminal_boundaries_keep_completed_effects_without_extra_calls() {
    for mode in ["disabled", "exempt", "budget", "utf16"] {
        let env = Env::new();
        tool_policy(&env, mode != "disabled", 2.0, if mode == "exempt" { &["bash"] } else { &["hub"] });
        let repeated = if mode == "utf16" { format!("printf '{}🌊'", "a".repeat(199)) } else { REPEATED.into() };
        if mode == "utf16" {
            let path = env.home.path().join("agent/config.yml");
            let content = std::fs::read_to_string(&path).unwrap().replace("maxRetries: 0", "maxRetries: 1");
            std::fs::write(path, content).unwrap();
        }
        let up = upstream(vec![tool(&repeated, "one"), tool(&repeated, "two"), answer("Finished")]).await;
        let args =
            if mode == "budget" { vec!["--tools", "bash", "--max-model-calls", "2"] } else { vec!["--tools", "bash"] };
        let mut child = RpcChild::spawn(&env, &up, &args);
        child.ready();
        child.run(mode, "Finish the bounded repetition task");
        let path = session_file(&mut child, "file");
        let rows = journal(&path);
        assert_eq!(notices(&rows).len(), usize::from(mode == "budget"));
        if mode == "utf16" {
            let refusal =
                child.seen.iter().find(|frame| frame["type"] == "notice" && frame["level"] == "error").unwrap();
            assert!(refusal["message"].as_str().unwrap().contains("UTF-16"));
            let native = ara_rpc::WireValue::parse(refusal["losslessDetailsJson"].as_str().unwrap()).unwrap();
            let units = native.get("resultSummary").unwrap().as_string().unwrap().units();
            assert_eq!(&units[199..], &[0xd83c, 0x2026]);
        } else {
            assert_eq!(std::fs::read_to_string(env.work.path().join("loop-effects.txt")).unwrap(), "xx");
        }
        assert_no_retry(&child);
        child.finish();
        assert_eq!(up.served(), if matches!(mode, "budget" | "utf16") { 2 } else { 3 });
        let restored = ara_session::SessionJournal::open(&path).unwrap();
        assert_eq!(restored.model_context().iter().filter(|message| {
            matches!(message, ara_ai::Message::Developer(message) if message.content.plain_text().contains("tool_call_loop_detected"))
        }).count(), usize::from(mode == "budget"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_host_guard_lifetime_is_retained_across_new_session() {
    let env = Env::new();
    tool_policy(&env, true, 2.0, &["hub"]);
    let up = upstream(vec![tool(REPEATED, "old"), tool(REPEATED, "new"), answer("Finished")]).await;
    let mut child = RpcChild::spawn(&env, &up, &["--tools", "bash", "--max-model-calls", "1"]);
    child.ready();
    let first = child.state("first")["sessionId"].clone();
    child.run("first-run", "One bounded tool turn");
    child.send(json!({"id":"new-session","type":"new_session"}));
    child.success("new-session");
    assert_ne!(child.state("second")["sessionId"], first);
    child.run("second-run", "One identical tool turn in a new Session");
    let path = session_file(&mut child, "path");
    let rows = journal(&path);
    assert_eq!(notices(&rows).len(), 1, "Fixed AgentSession retains its LoopGuards across new_session");
    assert!(!rows.iter().any(|row| row["message"]["toolCallId"] == "old"));
    assert!(rows.iter().any(|row| row["message"]["toolCallId"] == "new"));
    child.finish();
    assert_eq!(up.served(), 2);
}
