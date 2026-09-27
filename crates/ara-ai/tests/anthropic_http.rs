//! Anthropic Messages API-key wire tests against a controlled HTTP upstream.
//! Frames follow fixed OMP packages/ai/src/providers/anthropic.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.

use std::time::Duration;

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::anthropic::{self, StreamOptions};
use ara_ai::providers::openai_completions::RetryPolicy;
use ara_ai::{
    AssistantBlock, AssistantMessage, Context, Message, Model, StopReason, Tool, ToolResultMessage, UserBlock,
    UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn model(base_url: &str) -> Model {
    Model {
        id: "claude-fixture".into(),
        api: "anthropic-messages".into(),
        provider: "anthropic".into(),
        base_url: base_url.into(),
        reasoning: false,
        max_tokens: None,
        tokenizer: None,
    }
}

fn options() -> StreamOptions {
    StreamOptions {
        api_key: Some("anthropic-test-secret".into()),
        first_event_timeout: Some(Duration::from_secs(2)),
        idle_timeout: Some(Duration::from_secs(2)),
        retry: RetryPolicy {
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        },
        ..Default::default()
    }
}

fn frame(value: Value) -> Value {
    let name = value["type"].as_str().unwrap();
    json!({"raw":format!("event: {name}\ndata: {value}\n\n")})
}

fn upstream(events: Vec<Value>) -> Script {
    serde_json::from_value(json!({"responses":[{"events":events}]})).unwrap()
}

async fn collect(mut stream: ara_ai::AssistantStream) -> Vec<AssistantMessageEvent> {
    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("Anthropic stream should finish");
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    events
}

fn message(events: &[AssistantMessageEvent]) -> &AssistantMessage {
    match events.last().unwrap() {
        AssistantMessageEvent::Done { message, .. } => message,
        AssistantMessageEvent::Error { error, .. } => error,
        _ => panic!("missing terminal event"),
    }
}

#[tokio::test]
async fn text_tool_usage_and_followup_wire_are_preserved() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":11,"cache_read_input_tokens":2}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Writing."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a.txt\",\"content\":\"ok\"}"}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let mut context = Context {
        system_prompt: vec!["Be concise".into()],
        messages: vec![Message::User(UserMessage::text("write a file"))],
        tools: Some(vec![Tool {
            name: "write".into(),
            description: "Write a file".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}}}),
        }]),
    };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::ToolUse, .. })));
    assert_eq!(message(&events).text(), "Writing.");
    assert_eq!(message(&events).response_id.as_deref(), Some("msg_1"));
    assert_eq!(message(&events).usage.input, Some(11));
    assert_eq!(message(&events).usage.output, Some(7));
    assert_eq!(message(&events).usage.cache_read, Some(2));
    assert_eq!(message(&events).usage.total_tokens, Some(20));
    let call = message(&events).tool_calls().next().unwrap();
    assert_eq!(call.id, "toolu_1");
    assert_eq!(call.arguments["path"], "a.txt");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["request"], "POST /v1/messages HTTP/1.1");
    assert_eq!(requests[0]["headers"]["anthropic-version"], "2023-06-01");
    assert!(requests[0]["headers"]["x-api-key"].as_str().unwrap().starts_with("<redacted"));
    assert!(!requests[0].to_string().contains("anthropic-test-secret"));
    assert_eq!(requests[0]["body"]["max_tokens"], 4096);
    assert_eq!(requests[0]["body"]["system"][0]["text"], "Be concise");
    assert_eq!(requests[0]["body"]["tools"][0]["name"], "write");
    drop(requests);

    context.messages.push(Message::Assistant(message(&events).clone()));
    context.messages.push(Message::ToolResult(ToolResultMessage {
        tool_call_id: "toolu_1".into(),
        tool_name: "write".into(),
        content: vec![UserBlock::text("written")],
        details: None,
        is_error: false,
        timestamp: 1,
    }));
    let params = anthropic::build_params(&model(&server.base_url()), &context, &options()).unwrap();
    assert_eq!(params["messages"][1]["content"][1]["type"], "tool_use");
    assert_eq!(params["messages"][2]["content"][0]["type"], "tool_result");
    assert_eq!(params["messages"][2]["content"][0]["tool_use_id"], "toolu_1");
}

#[tokio::test]
async fn signed_thinking_replays_only_to_the_same_model() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_think"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"start-"}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step"}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig_123"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Done."}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let mut context = Context { messages: vec![Message::User(UserMessage::text("think"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    assert_eq!(message(&events).usage.input, None);
    assert_eq!(message(&events).usage.total_tokens, None);
    let AssistantBlock::Thinking(thinking) = &message(&events).content[0] else { panic!("missing thinking") };
    assert_eq!(thinking.thinking, "start-step");
    assert_eq!(thinking.thinking_signature.as_deref(), Some("sig_123"));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AssistantMessageEvent::ThinkingDelta { delta, .. } if delta == "start-"))
    );
    context.messages.push(Message::Assistant(message(&events).clone()));
    let params = anthropic::build_params(&model(&server.base_url()), &context, &options()).unwrap();
    assert_eq!(params["messages"][1]["content"][0]["type"], "thinking");
    assert_eq!(params["messages"][1]["content"][0]["signature"], "sig_123");
    let mut other = model(&server.base_url());
    other.id = "other-model".into();
    let params = anthropic::build_params(&other, &context, &options()).unwrap();
    assert_eq!(params["messages"][1]["content"][0]["type"], "text");
}

#[test]
fn call_token_limit_is_capped_and_unavailable_forced_tools_are_omitted() {
    let mut endpoint = model("http://fixture/v1");
    endpoint.max_tokens = Some(8192);
    let context = Context {
        messages: vec![Message::User(UserMessage::text("hi"))],
        tools: Some(Vec::new()),
        ..Default::default()
    };
    let mut opts = options();
    opts.max_tokens = Some(16_384);
    opts.tool_choice = Some(ara_ai::ToolChoice::Required);
    let params = anthropic::build_params(&endpoint, &context, &opts).unwrap();
    assert_eq!(params["max_tokens"], 8192);
    assert!(params.get("tool_choice").is_none());
    opts.tool_choice = Some(ara_ai::ToolChoice::Tool("missing".into()));
    let params = anthropic::build_params(&endpoint, &context, &opts).unwrap();
    assert!(params.get("tool_choice").is_none());
}

#[tokio::test]
async fn incomplete_tool_input_is_never_a_completed_call() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_partial"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_partial","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"x\""}})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("write"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
    assert_eq!(message(&events).tool_calls().count(), 0);
    assert!(message(&events).error_message.as_deref().unwrap().contains("before message_stop"));
}

#[test]
fn replay_repairs_tool_tail_adjacent_assistants_and_error_images() {
    let endpoint = model("http://fixture/v1");
    let mut first = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-fixture");
    first.content = vec![
        AssistantBlock::ToolCall(ara_ai::ToolCall {
            id: "toolu_1".into(),
            name: "read".into(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        }),
        AssistantBlock::text("After the call."),
    ];
    first.stop_reason = StopReason::ToolUse;
    let mut second = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-fixture");
    second.content = vec![AssistantBlock::text("Second turn.")];
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("read")),
            Message::Assistant(first),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "toolu_1".into(),
                tool_name: "read".into(),
                content: vec![
                    UserBlock::text("read failed"),
                    UserBlock::Image(ara_ai::ImageContent { data: "YQ==".into(), mime_type: "image/png".into() }),
                ],
                details: None,
                is_error: true,
                timestamp: 2,
            }),
            Message::Assistant(second.clone()),
            Message::User(UserMessage::text("")),
            Message::Assistant(second),
            Message::User(UserMessage::text("continue")),
        ],
        ..Default::default()
    };
    let messages = anthropic::convert_messages(&endpoint, &context);
    assert_eq!(messages[1]["content"][0]["type"], "text");
    assert_eq!(messages[1]["content"][1]["type"], "tool_use");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["content"][0]["type"], "text");
    assert!(messages[2]["content"][0]["content"].as_array().unwrap().iter().all(|block| block["type"] == "text"));
    assert_eq!(messages[2]["content"][2]["type"], "image");
    for pair in messages.windows(2) {
        assert!(!(pair[0]["role"] == "assistant" && pair[1]["role"] == "assistant"));
    }
}

#[tokio::test]
async fn missing_message_stop_after_completed_reason_is_best_effort_done() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_no_tail"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"complete"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    assert_eq!(message(&events).text(), "complete");
}

#[tokio::test]
async fn http_auth_failure_and_cancellation_are_not_success() {
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[
            {"status":401,"body":"{\"error\":{\"type\":\"authentication_error\",\"message\":\"invalid key\"}}"},
            {"events":[{"raw":"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_cancel\"}}\n\n"}],"end":"hang"}
        ]}))
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("hi"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { .. })));
    assert_eq!(message(&events).error_status, Some(401));
    assert!(message(&events).error_message.as_deref().unwrap().contains("invalid key"));

    let cancel = CancellationToken::new();
    let mut opts = options();
    opts.cancel = cancel.clone();
    let mut stream = anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts);
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let mut events = Vec::new();
    while let Some(event) = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap() {
        events.push(event);
    }
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Aborted, .. })));
}
