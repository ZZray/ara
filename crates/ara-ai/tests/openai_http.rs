//! Real HTTP tests of the Chat Completions adapter against `ara-testkit`'s
//! controlled upstream (deterministic faults; no real model).

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_completions::{self, RetryPolicy, StreamOptions};
use ara_ai::{AssistantMessage, Context, Message, Model, StopReason, Tool, UserMessage};
use ara_testkit::chunks::*;
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn script(v: Value) -> Script {
    serde_json::from_value(v).unwrap()
}

fn model(base: &str) -> Model {
    Model {
        id: "fake-model".into(),
        api: "openai-completions".into(),
        provider: "fake".into(),
        base_url: base.into(),
        reasoning: false,
        max_tokens: None,
    }
}

fn ctx() -> Context {
    Context {
        system_prompt: vec!["be brief".into()],
        messages: vec![Message::User(UserMessage::text("hello"))],
        tools: Some(vec![Tool {
            name: "read".into(),
            description: "Read a file".into(),
            parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }]),
    }
}

fn opts() -> StreamOptions {
    StreamOptions {
        api_key: Some("sk-test-secret".into()),
        retry: RetryPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
        },
        ..Default::default()
    }
}

async fn collect(mut rx: ara_ai::AssistantStream) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    let last = events.last().expect("terminal event").clone();
    let msg = match last {
        AssistantMessageEvent::Done { message, .. } => message,
        AssistantMessageEvent::Error { error, .. } => error,
        other => panic!("stream ended without terminal event: {other:?}"),
    };
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1, "exactly one terminal event");
    (events, msg)
}

#[tokio::test]
async fn streams_text_usage_and_records_request() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [text("Hi"), text(" there"), finish("stop"), usage(12, 3), done()]}]})),
        None,
    )
    .await
    .unwrap();
    let rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts());
    let (events, msg) = collect(rx).await;
    assert!(matches!(events[0], AssistantMessageEvent::Start { .. }));
    assert_eq!(msg.text(), "Hi there");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.usage.input, Some(12));
    assert_eq!(msg.usage.output, Some(3));
    assert_eq!(msg.response_id.as_deref(), Some("fake-1"));
    let reqs = server.requests.lock().await;
    assert_eq!(reqs.len(), 1);
    let body = &reqs[0]["body"];
    assert_eq!(reqs[0]["request"], json!("POST /v1/chat/completions HTTP/1.1"));
    assert_eq!(body["model"], json!("fake-model"));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["messages"][0], json!({"role": "system", "content": "be brief"}));
    assert_eq!(body["messages"][1], json!({"role": "user", "content": "hello"}));
    assert_eq!(body["tools"][0]["function"]["name"], json!("read"));
    let auth = reqs[0]["headers"]["authorization"].as_str().unwrap();
    assert!(auth.starts_with("<redacted"), "recorded auth must be redacted: {auth}");
}

#[tokio::test]
async fn retries_429_then_succeeds() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"status": 429, "headers": {"retry-after": "0"}, "body": "{\"error\":{\"message\":\"slow down\"}}"},
            {"status": 503, "body": "upstream down"},
            {"events": [text("ok"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.text(), "ok");
    assert_eq!(server.served(), 3);
    assert!(msg.usage.is_unknown(), "no usage chunk means unknown usage, not zero");
}

#[tokio::test]
async fn auth_failure_is_not_retried() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"status": 401, "body": "{\"error\":{\"message\":\"bad key\"}}"}]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(events.len(), 1, "no start event before a failed request");
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_status, Some(401));
    assert_eq!(msg.error_message.as_deref(), Some("401 bad key"));
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn retries_exhausted_reports_last_status() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"status": 500, "body": "a"}, {"status": 500, "body": "b"}, {"status": 502, "body": "{\"message\":\"gateway\"}"}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.error_status, Some(502));
    assert_eq!(msg.error_message.as_deref(), Some("502 gateway"));
    assert_eq!(server.served(), 3);
}

#[tokio::test]
async fn idle_stall_times_out_and_keepalive_does_not_reset() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [
            text("partial"),
            {"sleep_ms": 150}, {"raw": ": keep-alive\n\n"},
            {"sleep_ms": 150}, {"data": {"choices": [{"delta": {"content": ""}}]}},
            {"sleep_ms": 150}, {"raw": ": keep-alive\n\n"}
        ], "end": "hang"}]})),
        None,
    )
    .await
    .unwrap();
    let mut o = opts();
    o.idle_timeout = Some(Duration::from_millis(300));
    let started = Instant::now();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), o)).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::IDLE_TIMEOUT_MESSAGE));
    assert_eq!(msg.text(), "partial", "partial output is kept on the error message");
    assert!(
        events.iter().any(|e| matches!(e, AssistantMessageEvent::TextEnd { .. })),
        "open text block closed before error"
    );
}

#[tokio::test]
async fn first_event_timeout_covers_slow_headers() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"delay_ms": 2000, "events": [text("late"), finish("stop"), done()]}]})),
        None,
    )
    .await
    .unwrap();
    let mut o = opts();
    o.first_event_timeout = Some(Duration::from_millis(200));
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), o)).await;
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::FIRST_EVENT_TIMEOUT_MESSAGE));
}

#[tokio::test]
async fn cancellation_mid_stream_aborts() {
    let server =
        FakeUpstream::start(script(json!({"responses": [{"events": [text("working")], "end": "hang"}]})), None)
            .await
            .unwrap();
    let mut o = opts();
    let cancel = CancellationToken::new();
    o.cancel = cancel.clone();
    let mut rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), o);
    // Observe live streaming before cancelling.
    loop {
        match rx.recv().await.unwrap() {
            AssistantMessageEvent::TextDelta { delta, .. } => {
                assert_eq!(delta, "working");
                break;
            }
            _ => continue,
        }
    }
    cancel.cancel();
    let (_, msg) = collect(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(msg.error_message.as_deref(), Some("Request was aborted"));
    assert_eq!(msg.text(), "working");
}

#[tokio::test]
async fn dropped_stream_is_incomplete() {
    let server = FakeUpstream::start(script(json!({"responses": [{"events": [text("half")], "end": "drop"}]})), None)
        .await
        .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    let err = msg.error_message.unwrap();
    assert!(err == openai_completions::INCOMPLETE_STREAM_MESSAGE || err.starts_with("stream read failed"), "{err}");
}

#[tokio::test]
async fn finish_with_usage_completes_without_done_sentinel() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [tool_call(0, "c1", "read", "{\"path\":\"a.txt\"}"), finish("tool_calls"), {"data": {"choices": [], "usage": {"prompt_tokens": 5, "completion_tokens": 2, "prompt_tokens_details": {"cached_tokens": 1}}}}], "end": "hang"}]})),
        None,
    )
    .await
    .unwrap();
    let started = Instant::now();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert!(started.elapsed() < Duration::from_secs(2), "usage-only trailing chunk ends the response");
    assert_eq!(msg.stop_reason, StopReason::ToolUse);
    assert_eq!(msg.tool_calls().next().unwrap().arguments["path"], json!("a.txt"));
}

#[tokio::test]
async fn finish_without_usage_ends_after_grace() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [text("done"), finish("stop")], "end": "hang"}]})),
        None,
    )
    .await
    .unwrap();
    let started = Instant::now();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(2400) && elapsed < Duration::from_secs(6), "{elapsed:?}");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "done");
}

#[tokio::test]
async fn connection_refused_is_transport_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let mut o = opts();
    o.retry.max_attempts = 2;
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&format!("http://{addr}/v1")), ctx(), o))
            .await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(msg.error_message.unwrap().starts_with("request failed"));
}

#[tokio::test]
async fn empty_and_non_sse_bodies_are_errors() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": []},
            {"events": [{"raw": "{\"error\":{\"message\":\"quota\"}}\n"}]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::EMPTY_STREAM_MESSAGE));
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::EMPTY_STREAM_MESSAGE));
}

#[tokio::test]
async fn malformed_frame_is_an_error() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [text("a"), {"raw": "data: {\"choices\": [\n\n"}, finish("stop"), done()]}]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(msg.error_message.unwrap().starts_with("Malformed OpenAI completions stream frame"));
}

#[tokio::test]
async fn builder_error_fails_fast_without_retry() {
    let started = Instant::now();
    let (_, msg) = collect(openai_completions::stream(
        reqwest::Client::new(),
        model("api.example.invalid/v1"),
        ctx(),
        StreamOptions::default(),
    ))
    .await;
    assert!(started.elapsed() < Duration::from_secs(1), "{:?}", started.elapsed());
    assert!(msg.error_message.unwrap().starts_with("invalid request"));
}

#[tokio::test]
async fn read_error_after_finish_keeps_completed_response() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [text("answer"), finish("stop"), usage(4, 2)], "end": "drop"}]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "answer");
    assert_eq!(msg.usage.output, Some(2));
}

#[tokio::test]
async fn retry_after_http_date_beyond_cap_is_not_retried() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"status": 429, "headers": {"retry-after": "Wed, 21 Oct 2099 07:28:00 GMT"}, "body": "{\"error\":{\"message\":\"later\"}}"},
            {"events": [text("should not be reached"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.error_status, Some(429));
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn cancel_is_honoured_while_consumer_is_not_reading() {
    let events: Vec<Value> = (0..600).map(|i| text(&format!("t{i} "))).collect();
    let server =
        FakeUpstream::start(script(json!({"responses": [{"events": events, "end": "hang"}]})), None).await.unwrap();
    let mut o = opts();
    let cancel = CancellationToken::new();
    o.cancel = cancel.clone();
    let rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), o);
    tokio::time::sleep(Duration::from_millis(300)).await; // channel fills; provider parks in push
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    let (events, msg) = collect(rx).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert!(events.len() < 600, "provider stopped producing after cancel: {}", events.len());
}
