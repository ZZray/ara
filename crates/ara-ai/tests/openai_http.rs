//! Real HTTP tests of the Chat Completions adapter against `ara-testkit`'s
//! controlled upstream (deterministic faults; no real model).

use ara_ai::event::AssistantMessageEvent;
use ara_ai::model_tokenizer::{ModelContentCount, count_model_fragments};
use ara_ai::providers::openai_completions::{self, PreparedTextCount, RequestTextObserver, RetryPolicy, StreamOptions};
use ara_ai::{
    AssistantBlock, AssistantMessage, Context, ImageContent, Message, Model, ModelTokenizer, StopReason, TextContent,
    ThinkingContent, Tool, ToolCall, ToolResultMessage, UserBlock, UserContent, UserMessage,
};
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
        context_window: None,
        tokenizer: None,
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
async fn delayed_real_tool_result_reaches_the_wire_before_guidance() {
    let server =
        FakeUpstream::start(script(json!({"responses": [{"events": [text("ack"), finish("stop"), done()]}]})), None)
            .await
            .unwrap();
    let mut assistant = AssistantMessage::empty("openai-completions", "fake", "fake-model");
    assistant.stop_reason = StopReason::ToolUse;
    assistant.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "call_late".into(),
        name: "read".into(),
        arguments: serde_json::from_value(json!({"path": "a.txt"})).unwrap(),
        thought_signature: None,
    }));
    let mut context = ctx();
    context.messages = vec![
        Message::Assistant(assistant),
        Message::User(UserMessage::text("continue after the read")),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "call_late".into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text("actual file body")],
            details: None,
            is_error: false,
            timestamp: 3,
        }),
    ];
    let (_, answer) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), context, opts())).await;
    assert_eq!(answer.text(), "ack");
    let reqs = server.requests.lock().await;
    assert_eq!(reqs.len(), 1);
    let wire = reqs[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(wire[1]["role"], "assistant");
    assert_eq!(wire[2], json!({"role": "tool", "content": "actual file body", "tool_call_id": "call_late"}));
    assert_eq!(wire[3], json!({"role": "user", "content": "continue after the read"}));
    assert_eq!(wire.len(), 4);
}

#[tokio::test]
async fn malformed_tool_call_and_result_do_not_reach_openai_wire() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [text("continued"), finish("stop"), done()]}]})),
        None,
    )
    .await
    .unwrap();
    let mut assistant = AssistantMessage::empty("openai-completions", "fake", "fake-model");
    assistant.stop_reason = StopReason::ToolUse;
    assistant.content = vec![
        AssistantBlock::text("Reading files."),
        AssistantBlock::ToolCall(ToolCall {
            id: "bad".into(),
            name: "".into(),
            arguments: Default::default(),
            thought_signature: None,
        }),
        AssistantBlock::ToolCall(ToolCall {
            id: "good".into(),
            name: "read".into(),
            arguments: serde_json::from_value(json!({"path": "good.txt"})).unwrap(),
            thought_signature: None,
        }),
    ];
    let mut context = ctx();
    context.messages = vec![
        Message::User(UserMessage::text("read two files")),
        Message::Assistant(assistant),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "bad".into(),
            tool_name: "".into(),
            content: vec![UserBlock::text("Tool not found")],
            details: None,
            is_error: true,
            timestamp: 2,
        }),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: "good".into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text("real file contents")],
            details: None,
            is_error: false,
            timestamp: 3,
        }),
        Message::User(UserMessage::text("continue")),
    ];
    let (_, answer) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), context, opts())).await;
    assert_eq!(answer.text(), "continued");
    let reqs = server.requests.lock().await;
    assert_eq!(reqs.len(), 1);
    let wire = reqs[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(wire.len(), 5);
    assert_eq!(wire[2]["content"], "Reading files.");
    assert_eq!(wire[2]["tool_calls"].as_array().unwrap().len(), 1);
    assert_eq!(wire[2]["tool_calls"][0]["id"], "good");
    assert_eq!(wire[2]["tool_calls"][0]["function"]["name"], "read");
    assert_eq!(wire[3], json!({"role": "tool", "content": "real file contents", "tool_call_id": "good"}));
    assert_eq!(wire[4], json!({"role": "user", "content": "continue"}));
}

#[tokio::test]
async fn responses_composite_results_and_opaque_chat_ids_match_calls_on_the_wire() {
    let server =
        FakeUpstream::start(script(json!({"responses": [{"events": [text("ack"), finish("stop"), done()]}]})), None)
            .await
            .unwrap();
    let mut responses = AssistantMessage::empty("openai-responses", "openai", "source-model");
    responses.stop_reason = StopReason::ToolUse;
    responses.content.push(AssistantBlock::ToolCall(ToolCall {
        id: "call_A|fc_assistant".into(),
        name: "read".into(),
        arguments: Default::default(),
        thought_signature: None,
    }));
    let mut chat = AssistantMessage::empty("openai-completions", "fake", "fake-model");
    chat.stop_reason = StopReason::ToolUse;
    for id in ["call_A|first", "call_A|second"] {
        chat.content.push(AssistantBlock::ToolCall(ToolCall {
            id: id.into(),
            name: "read".into(),
            arguments: Default::default(),
            thought_signature: None,
        }));
    }
    let result = |id: &str, body: &str| {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content: vec![UserBlock::text(body)],
            details: None,
            is_error: false,
            timestamp: 3,
        })
    };
    let mut context = ctx();
    context.messages = vec![
        Message::Assistant(responses),
        result("call_A|fc_result", "responses output"),
        Message::Assistant(chat),
        result("call_A|second", "chat second output"),
        Message::User(UserMessage::text("continue")),
    ];
    let (_, answer) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), context, opts())).await;
    assert_eq!(answer.text(), "ack");
    let reqs = server.requests.lock().await;
    assert_eq!(reqs.len(), 1);
    let wire = reqs[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(wire.len(), 7);
    assert_eq!(wire[1]["tool_calls"][0]["id"], "call_A");
    assert_eq!(wire[2], json!({"role": "tool", "content": "responses output", "tool_call_id": "call_A"}));
    assert_eq!(wire[3]["tool_calls"][0]["id"], "call_A|first");
    assert_eq!(wire[3]["tool_calls"][1]["id"], "call_A|second");
    assert_eq!(wire[4], json!({"role": "tool", "content": "chat second output", "tool_call_id": "call_A|second"}));
    assert_eq!(wire[5]["tool_call_id"], "call_A|first");
    assert_eq!(wire[5]["content"], "No result provided");
    assert_eq!(wire[6], json!({"role": "user", "content": "continue"}));
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
async fn replay_safe_retry_reissues_pre_start_transient_http_once() {
    for status in [408, 429, 503] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"status": status, "body": "{\"error\":{\"message\":\"temporary failure\"}}"},
                {"events": [text("recovered"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let mut options = opts();
        options.retry.max_attempts = 1;
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options))
                .await;
        assert_eq!(server.served(), 2, "status {status}: {msg:?}");
        assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
        assert_eq!(msg.text(), "recovered");
        let accounting = msg.retry_accounting.as_ref().expect("outer replay must retain both attempts");
        assert_eq!(accounting.attempts.len(), 2);
        assert_eq!(accounting.attempts[0].usage.input, None, "failed HTTP attempt has unknown usage");
        assert_eq!(accounting.attempts[0].stop_reason, StopReason::Error);
        assert_eq!(accounting.attempts[1].stop_reason, StopReason::Stop);
    }
}

#[tokio::test]
async fn replay_safe_retry_preserves_pre_start_account_cap_and_admission_rejection() {
    for (body, expected_detail) in [
        (r#"{"error":{"code":"insufficient_quota","message":"Request declined"}}"#, "Request declined"),
        (r#"{"error":{"message":"Account monthly quota reached"}}"#, "Account monthly quota reached"),
        (
            r#"{"error":{"rate_limit_type":"max_parallel_requests","message":"Concurrent request limit"}}"#,
            "Concurrent request limit",
        ),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"status": 429, "body": body},
                {"events": [text("must not run"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let mut options = opts();
        options.retry.max_attempts = 1;
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options))
                .await;
        assert_eq!(server.served(), 1, "{msg:?}");
        assert_eq!(events.len(), 1, "no Start before a rejected request");
        assert_eq!(msg.error_status, Some(429));
        assert!(msg.error_message.as_deref().is_some_and(|message| message.contains(expected_detail)));
    }
}

#[tokio::test]
async fn replay_safe_retry_preserves_header_only_admission_rejection() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {
                "status": 429,
                "headers": {"rate_limit_type": "max_parallel_requests"},
                "body": "{\"error\":{\"message\":\"Busy\"}}"
            },
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 1, "header-only admission rejection bypasses both retry layers");
    assert_eq!(events.len(), 1, "no Start before a rejected request");
    assert_eq!(msg.error_status, Some(429));
    assert_eq!(msg.error_message.as_deref(), Some("429 Busy"));
}

#[tokio::test]
async fn replay_safe_retry_classifies_broad_terminal_account_caps() {
    let google_quota = json!({"error": {
        "code": 429,
        "status": "RESOURCE_EXHAUSTED",
        "message": "Too many requests",
        "details": [{
            "@type": "type.googleapis.com/google.rpc.ErrorInfo",
            "reason": "QUOTA_EXHAUSTED"
        }]
    }})
    .to_string();
    let google_long_ms = json!({"error": {
        "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Too many requests",
        "details": [
            {"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"},
            {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "300000ms"}
        ]
    }})
    .to_string();
    let google_reset_text = json!({"error": {
        "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Your limit will reset in 10 minutes",
        "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"}]
    }})
    .to_string();
    let google_absolute = json!({"error": {
        "code": 429, "status": "RESOURCE_EXHAUSTED",
        "message": "Your limit will reset at 2099-01-01 00:00:00Z. Please retry in 5s",
        "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"}]
    }})
    .to_string();
    let google_short = json!({"error": {
        "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Your limit will reset in 30s",
        "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"}]
    }})
    .to_string();
    let dashscope_throttle = json!({"error": {
        "code": "insufficient_quota",
        "message": "You exceeded your current quota, please check your plan and billing details. https://help.aliyun.com/zh/model-studio/error-code#token-limit"
    }})
    .to_string();
    for (name, body, blocked) in [
        ("opaque", "".to_string(), true),
        (
            "subscription",
            json!({"error": {"message": "You've exceeded your subscription rate limits"}}).to_string(),
            true,
        ),
        ("Chinese account", json!({"error": {"message": "已达到 5 小时的使用上限"}}).to_string(), true),
        ("Google structured", google_quota, true),
        ("Google long milliseconds", google_long_ms, true),
        ("Google reset text", google_reset_text, true),
        ("Google absolute reset", google_absolute, true),
        ("Google short reset", google_short, false),
        ("DashScope token throttle", dashscope_throttle, false),
        ("Chinese throttle", json!({"error": {"message": "每分钟使用次数已达上限"}}).to_string(), false),
        ("generic throttle", json!({"error": {"message": "Too many requests"}}).to_string(), false),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"status": 429, "body": body},
                {"events": [text("recovered"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let mut options = opts();
        options.retry.max_attempts = 1;
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options))
                .await;
        if blocked {
            assert_eq!(server.served(), 1, "{name}: {msg:?}");
            assert_eq!(events.len(), 1, "{name}: rejected before Start");
            assert_eq!(msg.error_status, Some(429), "{name}");
        } else {
            assert_eq!(server.served(), 2, "{name}: {msg:?}");
            assert_eq!(msg.text(), "recovered", "{name}");
        }
    }
}

#[tokio::test]
async fn replay_safe_retry_classifies_broad_in_band_account_caps() {
    let google = |retry_delay: &str| {
        json!({
            "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Too many requests",
            "details": [
                {"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"},
                {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": retry_delay}
            ]
        })
    };
    for (name, error, blocked) in [
        ("subscription", json!({"code": 429, "message": "You've exceeded your subscription rate limits"}), true),
        ("Chinese account", json!({"code": 429, "message": "额度已用完，请充值"}), true),
        ("Chinese throttle", json!({"code": 429, "message": "每分钟使用次数已达上限"}), false),
        ("Google long milliseconds", google("300000ms"), true),
        (
            "Google absolute reset",
            json!({
                "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "Your limit will reset at 2099-01-01 00:00:00Z",
                "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "RATE_LIMIT_EXCEEDED"}]
            }),
            true,
        ),
        ("Google short seconds", google("30s"), false),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"events": [{"data": {"error": error}}]},
                {"events": [text("recovered"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
        if blocked {
            assert_eq!(server.served(), 1, "{name}: {msg:?}");
            assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
            assert_eq!(msg.error_status, Some(429), "{name}");
        } else {
            assert_eq!(server.served(), 2, "{name}: {msg:?}");
            assert_eq!(msg.text(), "recovered", "{name}");
        }
    }
}

#[tokio::test]
async fn replay_safe_retry_does_not_replay_stalled_error_body_with_no_retry_header() {
    for headers in [json!({"rate_limit_type": "max_parallel_requests"}), json!({"retry-after": "120"})] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"status": 429, "headers": headers, "events": [], "end": "hang"},
                {"events": [text("must not run"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let mut options = opts();
        options.first_event_timeout = Some(Duration::from_millis(100));
        let (_, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options))
                .await;
        assert_eq!(msg.error_message.as_deref(), Some(openai_completions::FIRST_EVENT_TIMEOUT_MESSAGE));
        assert_eq!(server.served(), 1, "explicit no-retry header survives a stalled body");
    }
}

#[tokio::test]
async fn replay_safe_retry_cancellation_during_pre_start_backoff_stops_requests() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"status": 503, "body": "temporary failure"},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut options = opts();
    options.retry.max_attempts = 1;
    options.cancel = cancel.clone();
    let rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("first request should reach the upstream");
    cancel.cancel();
    let (_, msg) = collect(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn replay_safe_retry_discards_known_empty_stop_before_text() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), usage(3, 0), done()]},
            {"events": [text("answer"), finish("stop"), usage(5, 2), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 2);
    assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
    assert_eq!(msg.text(), "answer");
    assert_eq!(msg.usage.input, Some(5), "delivered usage is unchanged");
    assert_eq!(msg.usage.output, Some(2));
    let accounting = msg.retry_accounting.as_ref().unwrap();
    assert_eq!(accounting.attempts.len(), 2);
    assert_eq!(accounting.attempts[0].usage.input, Some(3));
    assert_eq!(accounting.attempts[0].usage.output, Some(0));
    assert_eq!(accounting.attempts[1].usage.input, Some(5));
    assert_eq!(accounting.attempts[1].usage.output, Some(2));
    assert!(accounting.elapsed_ms >= accounting.attempts[0].elapsed_ms);
}

#[tokio::test]
async fn replay_safe_retry_exhausts_two_known_empty_stops() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), usage(3, 1), done()]},
            {"events": [finish("stop"), usage(3, 1), done()]},
            {"events": [finish("stop"), usage(3, 1), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 3);
    assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "");
    assert_eq!(msg.usage.output, Some(1));
    let accounting = msg.retry_accounting.as_ref().unwrap();
    assert_eq!(accounting.attempts.len(), 3);
    assert!(accounting.attempts.iter().all(|attempt| attempt.usage.input == Some(3)));
}

#[tokio::test]
async fn replay_safe_retry_records_partial_usage_without_filling_unknown_buckets() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), {"data": {"choices": [], "usage": {"completion_tokens": 0}}}, done()]},
            {"events": [text("answer"), finish("stop"), {"data": {"choices": [], "usage": {"prompt_tokens": 5}}}, done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 2);
    assert_eq!(msg.text(), "answer");
    let attempts = &msg.retry_accounting.as_ref().unwrap().attempts;
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].usage.input, None);
    assert_eq!(attempts[0].usage.output, Some(0));
    assert_eq!(attempts[1].usage.input, Some(5));
    assert_eq!(attempts[1].usage.output, None);
}

#[tokio::test]
async fn replay_safe_retry_does_not_treat_unknown_usage_as_zero() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), done()]},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 1);
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "");
    assert_eq!(msg.usage.output, None);
}

#[tokio::test]
async fn replay_safe_retry_reissues_pre_output_stream_reset_once() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [{"raw": ": keep-alive\n\n"}, {"sleep_ms": 50}], "end": "drop"},
            {"events": [text("recovered"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut options = opts();
    options.retry.max_attempts = 1;
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert_eq!(server.served(), 2, "{msg:?}");
    assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
    assert_eq!(msg.text(), "recovered");
}

#[tokio::test]
async fn replay_safe_retry_only_reissues_transient_in_band_statuses() {
    for (status, detail, should_retry) in [
        (409, "stream rejected", false),
        (425, "stream rejected", false),
        (408, "stream rejected", true),
        (429, "Rate limit exceeded, retry after one second", true),
        (429, "Concurrent requests quota exceeded", true),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"events": [{"data": {"error": {"code": status, "message": detail}}}]},
                {"events": [text("recovered"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (_, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
        if should_retry {
            assert_eq!(server.served(), 2, "status {status}");
            assert_eq!(msg.text(), "recovered");
        } else {
            assert_eq!(server.served(), 1, "status {status}");
            assert_eq!(msg.error_status, Some(status));
        }
    }
}

#[tokio::test]
async fn replay_safe_retry_reissues_statusless_transient_in_band_errors() {
    for error_frame in [
        json!({"error": {"message": "Service unavailable"}}),
        json!({"error": {"message": "HTTP 408"}}),
        json!({"error": {"message": "HTTP 501"}}),
        json!({"message": "Upstream connect error"}),
        json!({"error": "The socket connection was closed unexpectedly"}),
        json!({"error": {"message": "stream_read_error"}}),
        json!({"error": {"message": "JSON Parse error: Unterminated string"}}),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"events": [{"data": error_frame}]},
                {"events": [text("recovered"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
        assert_eq!(server.served(), 2, "{msg:?}");
        assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
        assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
        assert_eq!(msg.text(), "recovered");
    }
}

#[tokio::test]
async fn replay_safe_retry_preserves_statusless_account_and_permanent_in_band_errors() {
    for (error_frame, detail) in [
        (json!({"error": {"message": "Your account rate limit was reached"}}), "Your account rate limit"),
        (json!({"error": {"message": "HTTP 429"}}), "HTTP 429"),
        (json!({"error": {"message": "HTTP 401 Service unavailable"}}), "HTTP 401 Service unavailable"),
        (json!({"error": {"message": "Invalid tool schema"}}), "Invalid tool schema"),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"events": [{"data": error_frame}]},
                {"events": [text("must not run"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
        assert_eq!(server.served(), 1, "{msg:?}");
        assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_status, None);
        assert!(msg.error_message.as_deref().is_some_and(|message| message.contains(detail)));
    }
}

#[tokio::test]
async fn replay_safe_retry_does_not_replay_statusless_error_after_text() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [text("partial"), {"data": {"error": {"message": "Service unavailable"}}}]},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 1, "committed text must prevent replay");
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
    assert_eq!(msg.text(), "partial");
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert!(msg.error_message.as_deref().is_some_and(|message| message.contains("Service unavailable")));
}

#[tokio::test]
async fn replay_safe_retry_preserves_in_band_account_limits_without_replay() {
    for (error, expected_detail) in [
        (
            json!({"type": "TOO_MANY_REQUESTS", "code": "insufficient_quota", "message": "Request declined"}),
            "Request declined",
        ),
        (json!({"code": 429, "message": "Account monthly quota reached"}), "Account monthly quota reached"),
        (json!({"code": 429, "message": "Your account rate limit was reached"}), "Your account rate limit"),
        (
            json!({"type": "TOO_MANY_REQUESTS", "code": "insufficient_quota", "message":
                "You exceeded your current quota, please check your plan and billing details."}),
            "You exceeded your current quota",
        ),
    ] {
        let server = FakeUpstream::start(
            script(json!({"responses": [
                {"events": [{"data": {"error": error}}]},
                {"events": [text("must not run"), finish("stop"), done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (events, msg) =
            collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
        assert_eq!(server.served(), 1, "{msg:?}");
        assert_eq!(events.iter().filter(|e| matches!(e, AssistantMessageEvent::Start { .. })).count(), 1);
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert_eq!(msg.error_status, Some(429));
        assert!(msg.error_message.as_deref().is_some_and(|message| message.contains(expected_detail)));
        assert_eq!(msg.text(), "");
    }
}

#[tokio::test]
async fn replay_safe_retry_reissues_a_concurrent_quota_throttle() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [{"data": {"error": {
                "type": "TOO_MANY_REQUESTS", "code": "quota_exceeded", "message": "Concurrent requests quota exceeded"
            }}}]},
            {"events": [text("recovered"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 2);
    assert_eq!(msg.text(), "recovered");
}

#[tokio::test]
async fn replay_safe_retry_reissues_generic_quota_code_with_rate_limit_message() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [{"data": {"error": {
                "type": "TOO_MANY_REQUESTS", "code": "quota_exceeded", "message": "Rate limit exceeded"
            }}}]},
            {"events": [text("recovered"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 2);
    assert_eq!(msg.text(), "recovered");
}

#[tokio::test]
async fn replay_safe_retry_reissues_documented_dashscope_token_throttle() {
    let detail = "You exceeded your current quota, please check your plan and billing details. See https://help.aliyun.com/zh/model-studio/error-code#token-limit";
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [{"data": {"error": {
                "type": "TOO_MANY_REQUESTS", "code": "insufficient_quota", "message": detail
            }}}]},
            {"events": [text("recovered"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(server.served(), 2);
    assert_eq!(msg.text(), "recovered");
}

#[tokio::test]
async fn replay_safe_retry_cancellation_during_backoff_stops_requests() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), usage(3, 0), done()]},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut options = opts();
    options.cancel = cancel.clone();
    let rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    cancel.cancel();
    let (_, msg) = collect(rx).await;
    assert_eq!(server.served(), 1);
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    let accounting = msg.retry_accounting.as_ref().expect("cancelled backoff retains discarded usage");
    assert_eq!(accounting.attempts.len(), 1);
    assert_eq!(accounting.attempts[0].usage.input, Some(3));
    assert_eq!(accounting.attempts[0].usage.output, Some(0));
}

#[tokio::test]
async fn replay_safe_retry_never_reissues_after_tool_event() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [tool_call(0, "call-1", "read", "{\"path\":\"a.txt\"}"), {"sleep_ms": 50}], "end": "drop"},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut options = opts();
    options.retry.max_attempts = 1;
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert!(events.iter().any(|e| matches!(e, AssistantMessageEvent::ToolcallStart { .. })));
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(server.served(), 1, "a tool call must commit its provider attempt");
    assert!(msg.retry_accounting.is_none(), "one committed attempt needs no replay receipt");
}

#[tokio::test]
async fn replay_safe_retry_streams_recovered_text_before_terminal() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), usage(3, 0), done()]},
            {"events": [text("live"), {"sleep_ms": 1000}, finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let mut options = opts();
    options.cancel = cancel.clone();
    let mut rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options);
    let delta = tokio::time::timeout(Duration::from_millis(900), async {
        loop {
            if let AssistantMessageEvent::TextDelta { delta, .. } = rx.recv().await.expect("live event") {
                break delta;
            }
        }
    })
    .await
    .expect("text should arrive before the delayed terminal frame");
    assert_eq!(delta, "live");
    assert_eq!(server.served(), 2);
    cancel.cancel();
    let (_, msg) = collect(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(msg.text(), "live");
    let accounting = msg.retry_accounting.as_ref().expect("cancelled second attempt retains both records");
    assert_eq!(accounting.attempts.len(), 2);
    assert_eq!(accounting.attempts[0].usage.input, Some(3));
    assert_eq!(accounting.attempts[1].stop_reason, StopReason::Aborted);
}

#[tokio::test]
async fn replay_safe_retry_accept_empty_response_disables_reissue() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"events": [finish("stop"), usage(3, 0), done()]},
            {"events": [text("must not run"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut options = opts();
    options.accept_empty_response = true;
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert_eq!(server.served(), 1);
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "");
}

#[tokio::test]
async fn prepared_text_observation_matches_retried_wire_body() {
    let server = FakeUpstream::start(
        script(json!({"responses": [
            {"status": 429, "headers": {"retry-after": "0"}, "body": "retry"},
            {"events": [text("ok"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut m = model(&server.base_url());
    m.tokenizer = Some(ModelTokenizer::ClaudeV3);
    let mut assistant = AssistantMessage::empty("openai-completions", "fake", "fake-model");
    assistant.content = vec![
        AssistantBlock::Text(TextContent { text: "A".into(), text_signature: None }),
        AssistantBlock::Thinking(ThinkingContent { thinking: "hidden reasoning".into(), thinking_signature: None }),
        AssistantBlock::Text(TextContent { text: "B".into(), text_signature: None }),
        AssistantBlock::ToolCall(ToolCall {
            id: "call-1".into(),
            name: "read".into(),
            arguments: json!({"path":"secret"}).as_object().unwrap().clone(),
            thought_signature: None,
        }),
    ];
    let context = Context {
        system_prompt: vec!["be brief".into()],
        messages: vec![
            Message::User(UserMessage {
                content: UserContent::Blocks(vec![
                    UserBlock::text("ask"),
                    UserBlock::Image(ImageContent { data: "YWJj".into(), mime_type: "image/png".into() }),
                ]),
                synthetic: None,
                timestamp: 0,
            }),
            Message::Assistant(assistant),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call-1".into(),
                tool_name: "read".into(),
                content: vec![
                    UserBlock::text("one"),
                    UserBlock::text("two"),
                    UserBlock::Image(ImageContent { data: "YWJj".into(), mime_type: "image/png".into() }),
                ],
                details: None,
                is_error: false,
                timestamp: 0,
            }),
        ],
        tools: ctx().tools,
    };
    let (tx, mut observations) = tokio::sync::mpsc::channel(2);
    let mut options = opts();
    options.request_text_observer = Some(RequestTextObserver::new(tx));
    let (_, response) = collect(openai_completions::stream(reqwest::Client::new(), m.clone(), context, options)).await;
    assert_eq!(response.text(), "ok");
    let observed = observations.try_recv().expect("one prepared observation");
    assert!(observations.try_recv().is_err(), "transport retry must not duplicate the observation");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["body"], requests[1]["body"]);
    let body = &requests[0]["body"];
    assert_eq!(body["messages"][2]["content"], "AB");
    assert_eq!(body["messages"][3]["content"], "one\ntwo");
    assert_eq!(body["messages"][4]["content"][0]["text"], "Attached image(s) from tool result:");
    let expected =
        count_model_fragments(&m, ["be brief", "ask", "AB", "one\ntwo", "Attached image(s) from tool result:"]);
    let ModelContentCount::Exact(expected) = expected else { panic!("selected family") };
    assert_eq!(observed.measurement.count, PreparedTextCount::Exact(expected));
    assert_eq!(observed.measurement.text_fields, 5);
    assert!(observed.measurement.has_tool_definitions);
    assert!(observed.measurement.has_tool_calls);
    assert!(observed.measurement.has_images);
    assert!(observed.measurement.coverage_complete);
    assert!(!format!("{observed:?}").contains("secret"), "diagnostic must not contain tool payloads");
}

#[test]
fn prepared_text_observation_keeps_uncertainty_explicit() {
    let m = model("http://unused/v1");
    let unknown = openai_completions::measure_prepared_message_text(&m, &json!({"messages":[{"content":"ξ"}]}));
    assert_eq!(unknown.count, PreparedTextCount::UnknownTokenizer);
    let unsupported = openai_completions::measure_prepared_message_text(
        &m,
        &json!({"messages":[{"content":[{"type":"audio","data":"opaque"}]}]}),
    );
    assert_eq!(unsupported.count, PreparedTextCount::UnsupportedShape);
    assert!(!unsupported.coverage_complete);
    let mut selected = m;
    selected.tokenizer = Some(ModelTokenizer::ClaudeV3);
    let large = openai_completions::measure_prepared_message_text(
        &selected,
        &json!({"messages":[{"content":"a".repeat(1024 * 1024 + 1)}]}),
    );
    assert_eq!(large.count, PreparedTextCount::TooLarge);
    assert!(!large.coverage_complete);
    let many = openai_completions::measure_prepared_message_text(
        &selected,
        &json!({"messages": vec![json!({"content": ""}); 16_385]}),
    );
    assert_eq!(many.count, PreparedTextCount::TooLarge);
    assert!(!many.coverage_complete);
}

#[tokio::test]
async fn unavailable_observer_does_not_change_delivery_or_cancellation() {
    let server =
        FakeUpstream::start(script(json!({"responses": [{"events": [text("ok"), finish("stop"), done()]}]})), None)
            .await
            .unwrap();
    let (tx, rx) = tokio::sync::mpsc::channel(1);
    drop(rx);
    let mut options = opts();
    let observer = RequestTextObserver::new(tx);
    options.request_text_observer = Some(observer.clone());
    let (_, response) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert_eq!(response.text(), "ok");
    assert_eq!(server.served(), 1);
    assert_eq!(observer.stats().dropped(), 1);

    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let mut options = opts();
    options.cancel.cancel();
    options.request_text_observer = Some(RequestTextObserver::new(tx));
    let (_, response) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert_eq!(response.stop_reason, StopReason::Aborted);
    assert!(rx.try_recv().is_err());
    assert_eq!(server.served(), 1);
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
            {"status": 500, "body": "a"}, {"status": 500, "body": "b"}, {"status": 502, "body": "c"},
            {"status": 500, "body": "d"}, {"status": 500, "body": "e"}, {"status": 502, "body": "{\"message\":\"gateway\"}"}
        ]})),
        None,
    )
    .await
    .unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.error_status, Some(502));
    assert_eq!(msg.error_message.as_deref(), Some("502 gateway"));
    assert_eq!(server.served(), 6, "three inner attempts in each of two outer attempts");
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
        script(json!({"responses": [
            {"delay_ms": 2000, "events": [text("late"), finish("stop"), done()]},
            {"delay_ms": 2000, "events": [text("late again"), finish("stop"), done()]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut o = opts();
    o.first_event_timeout = Some(Duration::from_millis(200));
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), o)).await;
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::FIRST_EVENT_TIMEOUT_MESSAGE));
    assert_eq!(server.served(), 2, "one bounded replay after the pre-Start timeout");
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
    let server = FakeUpstream::start(script(json!({"responses": [{"events": [text("half")]}]})), None).await.unwrap();
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.text(), "half");
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::INCOMPLETE_STREAM_MESSAGE));
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn reset_without_confirmed_frames_never_completes_or_dispatches_tools() {
    let server = FakeUpstream::start(
        // No frame before the reset: a frame sent just before an RST is dropped
        // on Windows but delivered on Linux, where it would count as content.
        script(json!({"responses": [
            {"events": [], "end": "drop"},
            {"events": [], "end": "drop"}
        ]})),
        None,
    )
    .await
    .unwrap();
    let mut options = opts();
    options.retry.max_attempts = 1;
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options)).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.tool_calls().count(), 0);
    assert_eq!(server.served(), 2, "one bounded replay before any confirmed content");
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
async fn object_tool_arguments_merge_fragments_and_emit_concat_safe_delta() {
    let frame = |arguments: Value| {
        json!({"data": {"choices": [{"delta": {"tool_calls": [
            {"index": 0, "id": "call_edit", "function": {"name": "edit", "arguments": arguments}}
        ]}}]}})
    };
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [
            frame(json!({"path": "a.txt", "oldText": "one ", "nested": {"text": "hel", "items": ["a"]}})),
            frame(json!({"oldText": "two", "nested": {"text": "lo", "items": ["b"]}})),
            frame(json!({"oldText": "one two", "nested": {"items": ["a", "b"], "extra": true}, "newText": "three"})),
            finish("tool_calls"), done()
        ]}]})),
        None,
    )
    .await
    .unwrap();
    let (events, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), opts())).await;
    let call = msg.tool_calls().next().unwrap();
    let expected = json!({
        "path": "a.txt", "oldText": "one two", "newText": "three",
        "nested": {"text": "hello", "items": ["a", "b"], "extra": true}
    });
    assert_eq!(Value::Object(call.arguments.clone()), expected);
    let deltas = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolcallDelta { delta, .. } => Some(delta.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(serde_json::from_str::<Value>(&deltas).unwrap(), expected);
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })).count(), 1);
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
    let mut once = opts();
    once.accept_empty_response = true; // inspect each malformed response without outer stream retries
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), once.clone()))
            .await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some(openai_completions::EMPTY_STREAM_MESSAGE));
    let (_, msg) =
        collect(openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), once)).await;
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            assert!(n > 0, "request closed before its body was read");
            request.extend_from_slice(&buf[..n]);
            let Some(header_end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") else { continue };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|n| n.trim().parse::<usize>().ok())
                })
                .unwrap();
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
        socket
            .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: 100000\r\nconnection: close\r\n\r\n")
            .await
            .unwrap();
        let frame = json!({
            "id": "fake-1",
            "choices": [{"index": 0, "delta": {"content": "answer"}, "finish_reason": "stop"}]
        });
        socket.write_all(format!("data: {frame}\n\n").as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        released.await.unwrap();
        #[allow(deprecated)]
        socket.set_linger(Some(Duration::from_secs(0))).unwrap();
        drop(socket);
    });
    let mut stream =
        openai_completions::stream(reqwest::Client::new(), model(&format!("http://{addr}/v1")), ctx(), opts());
    loop {
        let event = tokio::time::timeout(Duration::from_secs(3), stream.recv()).await.unwrap().unwrap();
        if matches!(event, AssistantMessageEvent::TextDelta { .. }) {
            break;
        }
    }
    assert!(tokio::time::timeout(Duration::from_millis(100), stream.recv()).await.is_err());
    release.send(()).unwrap();
    let (_, msg) = tokio::time::timeout(Duration::from_secs(2), collect(stream)).await.unwrap();
    server.await.unwrap();
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.text(), "answer");
    assert!(msg.usage.is_unknown());
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
    tokio::time::sleep(Duration::from_millis(2100)).await; // terminal must survive a delayed consumer
    let started = Instant::now();
    let (events, msg) = collect(rx).await;
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert!(events.len() < 600, "provider stopped producing after cancel: {}", events.len());
}

#[tokio::test]
async fn cancel_while_terminal_waits_for_full_channel_keeps_one_terminal() {
    // Start + text_start + 253 deltas + text_end fill the 256-slot outer
    // channel; the final Done waits for a consumer that has not started.
    let mut frames: Vec<Value> = (0..253).map(|i| text(&format!("t{i} "))).collect();
    frames.push(finish("stop"));
    frames.push(done());
    let server = FakeUpstream::start(script(json!({"responses": [{"events": frames}]})), None).await.unwrap();
    let cancel = CancellationToken::new();
    let mut options = opts();
    options.cancel = cancel.clone();
    let rx = openai_completions::stream(reqwest::Client::new(), model(&server.base_url()), ctx(), options);
    tokio::time::timeout(Duration::from_secs(2), async {
        while rx.len() < 256 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("outer event channel should fill before cancellation");
    tokio::time::sleep(Duration::from_millis(50)).await; // let Done reach the blocked send
    assert_eq!(server.served(), 1);
    cancel.cancel();
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let (events, msg) = collect(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Aborted);
    assert_eq!(events.iter().filter(|e| e.is_terminal()).count(), 1);
}
