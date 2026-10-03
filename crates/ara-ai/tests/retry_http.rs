//! Fixed fetch-retry.ts hints through real HTTP sockets and all consumers of
//! post_with_retry_policy. Controlled faults only; no real provider calls.
use ara_ai::providers::{openai_codex_responses as codex, openai_completions as chat, openai_responses as responses};
use ara_ai::{AssistantMessage, AssistantStream, Context, Model, StopReason};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug)]
enum Route {
    Chat,
    Responses,
    Codex,
}

fn policy() -> chat::RetryPolicy {
    chat::RetryPolicy { max_attempts: 3, base_delay: Duration::from_millis(1), max_delay: Duration::from_secs(2) }
}

fn successful(route: Route) -> Value {
    match route {
        Route::Chat => {
            json!({"events":[ara_testkit::chunks::text("recovered"), ara_testkit::chunks::finish("stop"), ara_testkit::chunks::done()]})
        }
        _ => json!({"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg","role":"assistant","content":[{"type":"output_text","text":"recovered"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}),
    }
}

async fn fixture(replies: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":replies})).unwrap(), None).await.unwrap()
}

fn stream(
    route: Route,
    server: &FakeUpstream,
    retry: chat::RetryPolicy,
    cancel: CancellationToken,
    first: Option<Duration>,
) -> AssistantStream {
    let api = match route {
        Route::Chat => "openai-completions",
        Route::Responses => "openai-responses",
        Route::Codex => codex::API,
    };
    let model = Model {
        id: "retry-fixture".into(),
        api: api.into(),
        provider: "fixture".into(),
        base_url: server.base_url(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let client = reqwest::Client::new();
    match route {
        Route::Chat => chat::stream(
            client,
            model,
            Context::default(),
            chat::StreamOptions {
                api_key: Some("synthetic".into()),
                retry,
                cancel,
                first_event_timeout: first,
                ..Default::default()
            },
        ),
        Route::Responses => responses::stream(
            client,
            model,
            Context::default(),
            responses::StreamOptions {
                api_key: Some("synthetic".into()),
                retry,
                cancel,
                first_event_timeout: first,
                ..Default::default()
            },
        ),
        Route::Codex => codex::stream(
            client,
            model,
            Context::default(),
            codex::StreamOptions {
                api_key: Some("synthetic".into()),
                extra_headers: vec![(codex::ACCOUNT_HEADER.into(), "fixture-account".into())],
                session_id: Some("fixture-session".into()),
                retry,
                cancel,
                first_event_timeout: first,
                ..Default::default()
            },
        ),
    }
}

async fn collect(mut stream: AssistantStream) -> AssistantMessage {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut terminal = None;
        while let Some(event) = stream.recv().await {
            if event.is_terminal() {
                assert!(terminal.is_none(), "one terminal per stream");
                terminal = Some(event.partial().clone());
            }
        }
        terminal.expect("terminal message")
    })
    .await
    .expect("bounded retry fixture")
}

async fn request_started(server: &FakeUpstream) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while server.served() == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("request reaches controlled upstream");
}

#[tokio::test]
async fn body_only_long_account_and_generic_hints_bypass_http_retries() {
    for route in [Route::Chat, Route::Responses, Route::Codex] {
        for body in [
            json!({"error":{"message":"Your limit will reset in 2 hours. Please retry in 5s"}}).to_string(),
            json!({"error":{"message":"busy","retryDelay":"7200000ms"}}).to_string(),
            json!({"error":{"message":"try again in ~120 min"}}).to_string(),
        ] {
            let server = fixture(vec![json!({"status":429,"body":body}), successful(route)]).await;
            let retry = if matches!(route, Route::Codex) { chat::RetryPolicy::default() } else { policy() };
            let output =
                collect(stream(route, &server, retry, CancellationToken::new(), Some(Duration::from_secs(2)))).await;
            assert_eq!(output.stop_reason, StopReason::Error, "{route:?}");
            assert_eq!(output.error_status, Some(429));
            let evidence = output.failure_evidence.as_ref().unwrap();
            assert!(evidence.replay_blocked);
            assert_eq!(evidence.wait_ms, Some(7_200_000.0));
            assert_eq!(ara_ai::retry_classification::classify_retry(&output, &output.api).wait_ms, Some(7_200_000.0));
            assert_eq!(server.served(), 1, "{route:?}: body window is not consumed by ordinary HTTP retry");
        }
    }
}

#[tokio::test]
async fn first_valid_header_wins_and_short_body_hints_wait_or_cancel() {
    for route in [Route::Chat, Route::Responses, Route::Codex] {
        let server = fixture(vec![json!({"status":429,"headers":{"retry-after-ms":"0","retry-after":"7200"},"body":"Your limit will reset in 2 hours"}), successful(route)]).await;
        let output =
            collect(stream(route, &server, policy(), CancellationToken::new(), Some(Duration::from_secs(2)))).await;
        assert_eq!(output.text(), "recovered", "{route:?}: header zero precedes later headers and body");
        assert_eq!(server.served(), 2);

        let server = fixture(vec![json!({"status":503,"body":"Please retry in 120ms"}), successful(route)]).await;
        let start = Instant::now();
        let output =
            collect(stream(route, &server, policy(), CancellationToken::new(), Some(Duration::from_secs(2)))).await;
        assert_eq!(output.text(), "recovered");
        assert!(start.elapsed() >= Duration::from_millis(110), "{route:?}: body delay replaces the 1ms fallback");
        assert_eq!(server.served(), 2);

        let server = fixture(vec![json!({"status":503,"body":"try again in 1 sec"}), successful(route)]).await;
        let cancel = CancellationToken::new();
        let mut pending = stream(route, &server, policy(), cancel.clone(), Some(Duration::from_secs(2)));
        request_started(&server).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(30), pending.recv()).await.is_err(),
            "{route:?}: the body hint keeps the stream waiting before caller cancellation"
        );
        cancel.cancel();
        let output = collect(pending).await;
        assert_eq!(output.stop_reason, StopReason::Aborted);
        // Fixed fetch-retry.ts::waitForRetry throws a plain abort error; its
        // terminal does not promise the previous response's retry hint.
        assert_eq!(output.error_message.as_deref(), Some("Request was aborted"));
        assert_eq!(server.served(), 1);
    }
}

#[tokio::test]
async fn watchdog_and_cancellation_preserve_only_observed_header_and_body_facts() {
    for route in [Route::Chat, Route::Responses, Route::Codex] {
        let server = fixture(vec![json!({"status":429,"headers":{"retry-after-ms":"360000"},"events":[{"raw":"Please retry in 1ms"}],"end":"hang"}), successful(route)]).await;
        let output =
            collect(stream(route, &server, policy(), CancellationToken::new(), Some(Duration::from_millis(100)))).await;
        assert_eq!(output.stop_reason, StopReason::Error);
        let evidence = output.failure_evidence.as_ref().unwrap();
        assert_eq!(evidence.status, Some(429));
        assert_eq!(evidence.wait_ms, Some(360_000.0));
        assert!(evidence.replay_blocked, "{route:?}: long observed header survives the body watchdog");
        assert_eq!(server.served(), 1);

        let server = fixture(vec![json!({"status":429,"delay_ms":250,"headers":{"retry-after-ms":"360000"},"body":"Your limit will reset in 2 hours"}), successful(route)]).await;
        let cancel = CancellationToken::new();
        let pending = stream(route, &server, policy(), cancel.clone(), None);
        request_started(&server).await;
        cancel.cancel();
        let output = collect(pending).await;
        assert_eq!(output.stop_reason, StopReason::Aborted);
        assert_eq!(output.error_status, None, "{route:?}: headers not yet received");
        assert_eq!(output.failure_evidence.as_ref().and_then(|evidence| evidence.status), None);
        assert_eq!(
            output.failure_evidence.as_ref().and_then(|evidence| evidence.wait_ms),
            None,
            "{route:?}: absent evidence and unobserved hints remain unknown"
        );
        assert_eq!(server.served(), 1);
    }
}

#[tokio::test]
async fn codex_default_keeps_six_http_attempts_and_the_five_minute_cap() {
    let route = Route::Codex;
    let mut replies = vec![json!({"status":503,"headers":{"retry-after-ms":"0"},"body":"busy"}); 5];
    replies.push(successful(route));
    let server = fixture(replies).await;
    let output = collect(stream(
        route,
        &server,
        chat::RetryPolicy::default(),
        CancellationToken::new(),
        Some(Duration::from_secs(2)),
    ))
    .await;
    assert_eq!(output.text(), "recovered");
    assert_eq!(server.served(), 6);

    let server = fixture(vec![json!({"status":503,"body":"Please retry in 301s"}), successful(route)]).await;
    let output = collect(stream(
        route,
        &server,
        chat::RetryPolicy::default(),
        CancellationToken::new(),
        Some(Duration::from_secs(2)),
    ))
    .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert_eq!(output.failure_evidence.as_ref().unwrap().wait_ms, Some(301_000.0));
    assert_eq!(server.served(), 1);

    // Above the generic 60s cap but below Codex's 300s cap enters a cancellable
    // wait, rather than immediately returning an HTTP terminal.
    let server = fixture(vec![json!({"status":503,"body":"Please retry in 61s"}), successful(route)]).await;
    let cancel = CancellationToken::new();
    let mut pending =
        stream(route, &server, chat::RetryPolicy::default(), cancel.clone(), Some(Duration::from_secs(2)));
    request_started(&server).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), pending.recv()).await.is_err(),
        "the 61s hint enters a wait under Codex's 300s cap instead of an early HTTP terminal"
    );
    cancel.cancel();
    let output = collect(pending).await;
    assert_eq!(output.stop_reason, StopReason::Aborted);
    assert_eq!(output.error_message.as_deref(), Some("Request was aborted"));
    assert_eq!(server.served(), 1);
}
