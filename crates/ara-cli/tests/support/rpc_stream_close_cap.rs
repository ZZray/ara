//! Fixed native retry-cap inputs through the existing real child/socket suite.
use super::*;

fn responses_answer() -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{
            "type":"message","id":"answer","content":[{"type":"output_text","text":"Original Session reopened"}]}}},
        {"data":{"type":"response.completed","response":{"id":"response_answer","status":"completed"}}}
    ]})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn known_thinking_closes_stop_after_one_session_retry_and_original_session_reopens() {
    // Native OpenRouter/Copilot cases plus one actual route near-miss. Responses
    // EOF/DONE must reach the cap through the real parser, not an injected error.
    for mode in [
        "openrouter",
        "copilot-eof",
        "copilot-done",
        "copilot-reasoning-done",
        "copilot-error",
        "other-responses",
        "unknown-item",
    ] {
        let mut env = Env::new();
        let vetoed = mode == "unknown-item";
        let (provider, api, bounded) = if mode == "openrouter" {
            ("openrouter", "openai-completions", true)
        } else {
            env.model = "grok-4.6";
            (
                if mode == "other-responses" { "fixture" } else { "github-copilot" },
                "openai-responses",
                mode != "other-responses",
            )
        };
        config(&env, true, 3.0, 0.0, 300_000.0);
        let failure = if mode == "openrouter" {
            json!({"events":[
                {"data":{"choices":[{"index":0,"delta":{"reasoning_content":"Checking the original source before continuing."},"finish_reason":null}]}},
                {"data":{"error":{"code":503,"message":"server_error: stream closed with reason: error"}}}
            ]})
        } else {
            let mut events = vec![
                json!({"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"reasoning"}}}),
                json!({"data":{"type":"response.reasoning_summary_text.delta","output_index":0,"item_id":"reasoning","summary_index":0,"delta":"Checking the original source before continuing."}}),
            ];
            if mode == "copilot-done" {
                events.push(done());
            }
            if mode == "copilot-reasoning-done" {
                events.push(json!({"data":{"type":"response.output_item.done","output_index":0,"item":{
                    "type":"reasoning","id":"reasoning","summary":[{"type":"summary_text","text":"Completed plan"}]}}}));
            }
            if mode == "copilot-error" {
                events.push(json!({"data":{"type":"error","error":{"code":"server_error",
                    "message":"OpenAI responses stream closed before a terminal response event was received"}}}));
            }
            if vetoed {
                events.push(json!({"data":{"type":"response.output_item.done","output_index":1,
                    "item":{"type":"computer_call","id":"unknown-effect"}}}));
            }
            json!({"events":events})
        };
        // Meaningful thinking commits the Provider stream, so each Host
        // attempt has one request and no transparent Provider replay.
        let answer = if mode == "openrouter" { super::answer("Original Session reopened") } else { responses_answer() };
        let failures =
            if vetoed { vec![failure, answer] } else { vec![failure.clone(), failure, answer.clone(), answer] };
        let up = upstream(failures).await;
        let mut child = RpcChild::spawn(&env, &up, &["--provider", provider, "--api", api, "--tools", ""]);
        child.ready();
        let original = child.state("original")["sessionId"].clone();
        child.prompt("bounded-close", "Continue the bounded original task");
        let end = child.until(|frame| frame["type"] == if vetoed { "agent_end" } else { "auto_retry_end" });
        if !vetoed {
            assert_eq!(end["success"], !bounded, "{mode}: {end}");
            assert_eq!(end["attempt"], if bounded { 1 } else { 2 });
        }
        let starts = child.seen.iter().filter(|frame| frame["type"] == "auto_retry_start").collect::<Vec<_>>();
        assert_eq!(
            starts.len(),
            if vetoed {
                0
            } else if bounded {
                1
            } else {
                2
            }
        );
        assert!(starts.iter().all(|frame| frame["maxAttempts"] == if bounded { 1 } else { 3 }));
        assert_eq!(
            up.served(),
            if vetoed {
                1
            } else if bounded {
                2
            } else {
                3
            },
            "Provider and Session budgets are distinct"
        );
        let path = session_file(&mut child, "file");
        let rows = journal(&path);
        let failed = failed_entries(&rows);
        assert_eq!(failed.len(), if vetoed { 1 } else { 2 });
        if api == "openai-responses" {
            assert!(failed.iter().all(|row| row["message"]["failureEvidence"]["replayBlocked"] == true), "{mode}");
        }
        assert!(failed.iter().all(|row| row["message"]["failureEvidence"]["sameRouteBlocked"] == vetoed), "{mode}");
        if vetoed {
            assert!(failed[0]["message"].get("retryRecovery").is_none());
        } else {
            assert_eq!(
                failed[0]["message"]["retryRecovery"]["status"],
                if bounded { "superseded" } else { "recovered" }
            );
        }
        if bounded && !vetoed {
            assert!(failed[1]["message"].get("retryRecovery").is_none());
            assert!(failed[1]["message"]["errorMessage"].as_str().unwrap().contains("stream closed"));
        }
        assert_eq!(child.state("idle")["isRetrying"], false);
        child.finish();
        let before = up.served();
        let mut reopened = RpcChild::spawn(
            &env,
            &up,
            &["--provider", provider, "--api", api, "--resume", path.to_str().unwrap(), "--tools", ""],
        );
        reopened.ready();
        assert_eq!(reopened.state("same")["sessionId"], original);
        reopened.run("resume", "Resume the same Session without tools");
        reopened.finish();
        assert_eq!(up.served(), before + 1);
    }
}
