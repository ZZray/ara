//! Anthropic Messages API-key wire tests against a controlled HTTP upstream.
//! Frames follow fixed OMP packages/ai/src/providers/anthropic.ts at
//! 596f2da7101178214aa27a753529d15e6b7ad91d.

use std::time::Duration;

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::anthropic::{self, StreamOptions};
use ara_ai::providers::openai_completions::RetryPolicy;
use ara_ai::{
    AssistantBlock, AssistantMessage, Context, ImageContent, Message, Model, StopReason, Tool, ToolResultMessage,
    UserBlock, UserContent, UserMessage,
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

fn raw(event: &str, data: &str) -> Value {
    json!({"raw":format!("event: {event}\ndata: {data}\n\n")})
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
async fn tool_stream_snapshots_keep_the_call_at_one_content_index_until_final_validation() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_tool_stream"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Preparing."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_stream","name":"write","input":{"legacy":"value"}}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"strea"}})),
            frame(json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":"While streaming."}})),
            frame(json!({"type":"content_block_stop","index":2})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"m.txt\",\"content\":\"ok\"}"}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("write"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    let tool_events: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AssistantMessageEvent::ToolcallStart { .. }
                    | AssistantMessageEvent::ToolcallDelta { .. }
                    | AssistantMessageEvent::ToolcallEnd { .. }
            )
        })
        .collect();
    assert_eq!(tool_events.len(), 4);
    let (start_index, start) = match tool_events[0] {
        AssistantMessageEvent::ToolcallStart { content_index, partial } => (*content_index, partial),
        other => panic!("expected tool start, got {other:?}"),
    };
    assert_eq!(start_index, 1);
    assert!(matches!(&start.content[0], AssistantBlock::Text(text) if text.text == "Preparing."));
    let call = start.content[1].as_tool_call().unwrap();
    assert_eq!((&call.id[..], &call.name[..]), ("toolu_stream", "write"));
    assert_eq!(call.arguments["legacy"], "value");
    let (delta_index, first_delta) = match tool_events[1] {
        AssistantMessageEvent::ToolcallDelta { content_index, partial, .. } => (*content_index, partial),
        other => panic!("expected tool delta, got {other:?}"),
    };
    assert_eq!(delta_index, start_index);
    assert_eq!(first_delta.content[1].as_tool_call().unwrap().arguments["path"], "strea");
    assert_eq!(
        tool_events[2].partial().content[1].as_tool_call().unwrap().arguments["path"],
        "strea",
        "small follow-up deltas reuse the latest display parse until final validation"
    );
    let (end_index, final_call, final_partial) = match tool_events[3] {
        AssistantMessageEvent::ToolcallEnd { content_index, tool_call, partial } => {
            (*content_index, tool_call, partial)
        }
        other => panic!("expected tool end, got {other:?}"),
    };
    assert_eq!(end_index, start_index);
    assert_eq!(final_call.arguments, final_partial.content[1].as_tool_call().unwrap().arguments);
    assert_eq!(final_call.arguments["path"], "stream.txt");
    assert_eq!(final_call.arguments["content"], "ok");
    assert!(final_call.arguments.get("legacy").is_none());
    assert!(matches!(&final_partial.content[2], AssistantBlock::Text(text) if text.text == "While streaming."));
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::ToolUse, .. })));
}

#[tokio::test]
async fn large_tool_input_keeps_stream_snapshots_small_but_validates_the_full_final_json() {
    let content = "x".repeat(16 * 1024);
    let raw = json!({"path":"large.txt","content":content}).to_string();
    let mut frames = vec![
        frame(json!({"type":"message_start","message":{"id":"msg_large_tool"}})),
        frame(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_large","name":"write","input":{"seed":"y".repeat(8 * 1024)}}}),
        ),
    ];
    for chunk in raw.as_bytes().chunks(128) {
        let delta = std::str::from_utf8(chunk).unwrap();
        frames.push(frame(
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":delta}}),
        ));
    }
    frames.extend([
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
        frame(json!({"type":"message_stop"})),
    ]);
    let server = FakeUpstream::start(upstream(frames), None).await.unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("write"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    let snapshots: Vec<_> = events
        .iter()
        .filter(|event| {
            matches!(event, AssistantMessageEvent::ToolcallStart { .. } | AssistantMessageEvent::ToolcallDelta { .. })
        })
        .collect();
    assert_eq!(snapshots.len(), 1 + raw.len().div_ceil(128));
    assert!(snapshots[0].partial().content[0].as_tool_call().unwrap().arguments.is_empty());
    assert!(snapshots[1].partial().content[0].as_tool_call().unwrap().arguments["content"].as_str().is_some());
    assert!(
        snapshots.iter().skip(34).all(|event| event.partial().content[0].as_tool_call().unwrap().arguments.is_empty())
    );
    let call = message(&events).tool_calls().next().unwrap();
    assert_eq!(call.arguments["content"].as_str().unwrap().len(), 16 * 1024);
    assert_eq!(call.arguments["path"], "large.txt");
}

#[tokio::test]
async fn spliced_envelope_keeps_first_message_and_does_not_replay_closed_blocks() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"first","usage":{"input_tokens":3}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"once"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_once","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"once.txt\"}"}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"redacted"}})),
            frame(json!({"type":"content_block_stop","index":2})),
            frame(json!({"type":"message_start","message":{"id":"second","usage":{"input_tokens":999}}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"duplicate"}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"duplicate"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"tool_twice","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"twice.txt\"}"}})),
            frame(json!({"type":"content_block_stop","index":1})),
            frame(json!({"type":"content_block_start","index":2,"content_block":{"type":"redacted_thinking","data":"replayed"}})),
            frame(json!({"type":"content_block_stop","index":2})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":999}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("write"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::ToolUse, .. })));
    assert_eq!(message(&events).response_id.as_deref(), Some("first"));
    assert_eq!(message(&events).usage.input, Some(3));
    assert_eq!(message(&events).usage.output, Some(7));
    assert_eq!(message(&events).text(), "once");
    let calls: Vec<_> = message(&events).tool_calls().collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "tool_once");
    assert_eq!(calls[0].arguments["path"], "once.txt");
    assert_eq!(
        message(&events)
            .content
            .iter()
            .filter(|block| matches!(block, AssistantBlock::RedactedThinking { .. }))
            .count(),
        1
    );
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })).count(), 1);
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::TextEnd { .. })).count(), 1);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn malformed_and_unknown_frames_do_not_discard_a_completed_message() {
    let server = FakeUpstream::start(
        upstream(vec![
            raw("telemetry", "not json"),
            raw("ping", "not json"),
            raw("message_start", "not json"),
            raw("message_start", r#"{"type":"telemetry"}"#),
            frame(json!({"type":"message_start","message":{"id":"good"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"future_block"}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"future_delta"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"content_block_start","index":1})),
            frame(json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":""}})),
            frame(json!({"type":"message_start","message":{"id":"spliced"}})),
            frame(json!({"type":"content_block_start","index":2,"content_block":{"type":"text","text":"duplicate"}})),
            frame(json!({"type":"content_block_delta","index":2})),
            frame(json!({"type":"content_block_delta","index":2,"delta":{"type":"thinking_delta","thinking":"wrong"}})),
            raw("content_block_delta", "not json"),
            frame(json!({"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"ok"}})),
            frame(json!({"type":"content_block_stop","index":2})),
            frame(json!({"type":"content_block_delta","index":9,"delta":{"type":"text_delta","text":"ignored"}})),
            frame(json!({"type":"content_block_stop","index":9})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
    assert_eq!(message(&events).response_id.as_deref(), Some("good"));
    assert_eq!(message(&events).text(), "ok");
    assert_eq!(message(&events).content.len(), 1);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn malformed_only_start_and_structured_stream_error_are_not_success() {
    let incomplete = FakeUpstream::start(upstream(vec![raw("message_start", "not json")]), None).await.unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&incomplete.base_url()), context.clone(), options()))
            .await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { .. })));
    assert_eq!(message(&events).tool_calls().count(), 0);
    let failed = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[
            {"events":[raw("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)]},
            {"events":[raw("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)]}
        ]}))
        .unwrap(),
        None,
    )
    .await
    .unwrap();
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&failed.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { .. })));
    assert!(
        message(&events).error_message.as_deref().unwrap().contains("overloaded_error"),
        "{:?}",
        message(&events).error_message
    );
    assert!(message(&events).error_message.as_deref().unwrap().contains("Overloaded"));
    assert_eq!(failed.served(), 2);
}

#[tokio::test]
async fn refusal_details_and_sensitive_stops_keep_their_reason_in_terminal_errors() {
    for (raw, details, expected_details, expected_message) in [
        ("refusal", Some(Value::Null), json!({"type":"refusal"}), "Refusal (no details provided)"),
        ("refusal", None, json!({"type":"refusal"}), "Refusal (no details provided)"),
        (
            "refusal",
            Some(json!({"type":"refusal","category":"policy","explanation":"  Request blocked.  "})),
            json!({"type":"refusal","category":"policy","explanation":"  Request blocked.  "}),
            "Refusal (policy): Request blocked.",
        ),
        (
            "refusal",
            Some(json!({"type":"refusal","explanation":"   "})),
            json!({"type":"refusal","explanation":"   "}),
            "Refusal",
        ),
        ("sensitive", Some(Value::Null), json!({"type":"sensitive"}), "Content flagged by safety filters"),
    ] {
        let mut delta = json!({"stop_reason":raw});
        if let Some(details) = details {
            delta["stop_details"] = details;
        }
        let server = FakeUpstream::start(
            upstream(vec![
                frame(json!({"type":"message_start","message":{"id":"msg_rejected","usage":{"input_tokens":5}}})),
                frame(json!({"type":"message_delta","delta":delta,"usage":{"output_tokens":0}})),
                frame(json!({"type":"message_stop"})),
            ]),
            None,
        )
        .await
        .unwrap();
        let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
        let events =
            collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
        assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
        assert_eq!(message(&events).stop_details.as_ref(), Some(&expected_details), "{raw}");
        assert_eq!(message(&events).error_message.as_deref(), Some(expected_message), "{raw}");
        assert_eq!(message(&events).usage.input, Some(5));
        assert_eq!(message(&events).usage.output, Some(0));
        assert_eq!(server.served(), 1);
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
    assert!(requests[0]["headers"]["authorization"].as_str().unwrap().starts_with("<redacted"));
    assert!(requests[0]["headers"].get("x-api-key").is_none());
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
async fn actual_messages_request_normalizes_legacy_and_nested_tool_schemas() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_schema"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ok"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let original = json!({
        "$schema":"http://json-schema.org/draft-07/schema#",
        "type":"array",
        "definitions":{"Code":{"type":"integer","minimum":1}},
        "properties":{
            "code":{"$ref":"#/definitions/Code"},
            "email":{"type":"string","format":"email","default":{"type":"object","pattern":"literal"}},
            "color":{"type":"string","description":"Color hint","pattern":"^#[0-9a-f]+$","format":"color-hex"},
            "count":{"type":["integer","null"],"minimum":0},
            "tags":{"type":"array","items":{"type":"string","minLength":1},"minItems":2,"uniqueItems":true},
            "choice":{"anyOf":[{"type":"string"},{"type":"number","minimum":1}]},
            "metadata":{"type":"object","additionalProperties":{}},
            "open":{"type":"object","additionalProperties":true},
            "typed":{"type":"object","additionalProperties":{"type":"integer","minimum":0}},
            "closed":{"type":"object","properties":{"name":{"type":"string"}}},
            "any":true
        },
        "required":["email",42,false],
        "allOf":[{"required":["email"]}],
        "oneOf":[{"required":["code"]}],
        "unsupportedRoot":"hint"
    });
    let context = Context {
        messages: vec![Message::User(UserMessage::text("validate schema"))],
        tools: Some(vec![Tool {
            name: "inspect".into(),
            description: "Inspect input".into(),
            parameters: original.clone(),
        }]),
        ..Default::default()
    };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
    assert_eq!(context.tools.as_ref().unwrap()[0].parameters, original);
    let requests = server.requests.lock().await;
    let schema = &requests[0]["body"]["tools"][0]["input_schema"];
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["required"], json!(["email"]));
    assert_eq!(schema["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(schema["$defs"]["Code"]["description"], "{minimum: 1}");
    assert_eq!(schema["properties"]["code"]["$ref"], "#/$defs/Code");
    assert_eq!(schema["properties"]["email"]["format"], "email");
    assert_eq!(schema["properties"]["email"]["default"], original["properties"]["email"]["default"]);
    assert_eq!(
        schema["properties"]["color"]["description"],
        "Color hint\n\n{pattern: \"^#[0-9a-f]+$\", format: \"color-hex\"}"
    );
    assert!(schema["properties"]["color"].get("format").is_none());
    assert_eq!(schema["properties"]["count"]["description"], "{minimum: 0}");
    assert_eq!(schema["properties"]["choice"]["anyOf"][1]["description"], "{minimum: 1}");
    assert_eq!(schema["properties"]["tags"]["items"]["description"], "{minLength: 1}");
    assert!(schema["properties"]["tags"].get("minItems").is_none());
    assert!(schema["properties"]["tags"]["description"].as_str().unwrap().contains("minItems: 2"));
    assert_eq!(schema["properties"]["metadata"]["additionalProperties"], true);
    assert_eq!(schema["properties"]["open"]["additionalProperties"], true);
    assert_eq!(schema["properties"]["typed"]["additionalProperties"]["description"], "{minimum: 0}");
    assert_eq!(schema["properties"]["closed"]["additionalProperties"], false);
    assert_eq!(schema["properties"]["any"], true);
    assert!(schema.get("allOf").is_none() && schema.get("oneOf").is_none());
    assert!(schema["description"].as_str().unwrap().contains("unsupportedRoot: \"hint\""));
    assert!(schema["description"].as_str().unwrap().contains("allOf:"));
    assert!(schema["description"].as_str().unwrap().contains("oneOf:"));
}

#[tokio::test]
async fn actual_messages_request_applies_shared_tool_wire_postprocessing() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_wire"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Schema received."}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let original = json!({"type":"object","properties":{
        "skip":{"anyOf":[{"type":"integer","minimum":0},{"type":"null"}],"description":"optional skip"},
        "bare":{"enum":[true,false]},
        "mode":{"anyOf":[{"const":"a","description":"mode"},{"const":"b","description":"mode"}]},
        "literal":{"default":{"anyOf":[{"const":"x"},{"const":"y"}]}},
        "unconstrained":{"not":{}},
        "typed":{"type":"object","anyOf":[{"type":"string"},{"type":"null"}]}
    },"required":["skip","mode","bare"]});
    let context = Context {
        messages: vec![Message::User(UserMessage::text("check schema"))],
        tools: Some(vec![Tool {
            name: "schema_wire".into(),
            description: "check".into(),
            parameters: original.clone(),
        }]),
        ..Default::default()
    };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), options())).await;
    assert_eq!(message(&events).text(), "Schema received.");
    assert_eq!(context.tools.as_ref().unwrap()[0].parameters, original);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let schema = &requests[0]["body"]["tools"][0]["input_schema"];
    assert_eq!(schema["required"], original["required"]);
    assert_eq!(schema["properties"]["skip"]["type"], json!(["integer", "null"]));
    assert_eq!(schema["properties"]["skip"]["description"], "optional skip\n\n{minimum: 0}");
    assert_eq!(schema["properties"]["bare"], json!({"type":"boolean","enum":[true,false]}));
    assert_eq!(schema["properties"]["mode"], json!({"type":"string","enum":["a","b"],"description":"mode"}));
    assert_eq!(schema["properties"]["literal"]["default"], original["properties"]["literal"]["default"]);
    assert_eq!(schema["properties"]["unconstrained"]["description"], "{not: true}");
    assert_eq!(schema["properties"]["typed"]["type"], "object");
    assert_eq!(schema["properties"]["typed"]["anyOf"].as_array().unwrap().len(), 2);
}

#[test]
fn empty_and_open_tool_objects_keep_their_non_strict_wire_meaning() {
    let endpoint = model("http://fixture/v1");
    let context = Context {
        messages: vec![Message::User(UserMessage::text("hi"))],
        tools: Some(vec![
            Tool { name: "empty".into(), description: String::new(), parameters: json!({}) },
            Tool { name: "open".into(), description: String::new(), parameters: json!({"additionalProperties":{}}) },
            Tool {
                name: "union".into(),
                description: String::new(),
                parameters: json!({
                    "properties":{"anything":{},"value":{"oneOf":[{"type":"string"},{"type":"number"}]}},
                    "anyOf":[{"required":["anything"]}],
                    "required":["anything"]
                }),
            },
        ]),
        ..Default::default()
    };
    let params = anthropic::build_params(&endpoint, &context, &options()).unwrap();
    assert_eq!(
        params["tools"][0]["input_schema"],
        json!({"type":"object","properties":{},"required":[],"additionalProperties":false})
    );
    assert_eq!(params["tools"][1]["input_schema"]["additionalProperties"], true);
    assert_eq!(params["tools"][2]["input_schema"]["properties"]["anything"], true);
    assert!(params["tools"][2]["input_schema"].get("anyOf").is_none());
    assert!(params["tools"][2]["input_schema"]["description"].as_str().unwrap().contains("anyOf:"));
    assert!(
        params["tools"][2]["input_schema"]["properties"]["value"]["description"].as_str().unwrap().contains("oneOf:")
    );
    assert!(params["tools"][0].get("strict").is_none());
    assert!(params["tools"][1].get("strict").is_none());
    assert!(params["tools"][2].get("strict").is_none());
}

#[test]
fn official_anthropic_route_selects_bounded_strict_tools_without_changing_custom_routes() {
    let mut endpoint = model("https://api.anthropic.com/v1");
    let edit = Tool {
        name: "edit".into(),
        description: "Edit a file".into(),
        parameters: json!({"type":"object","properties":{
            "path":{"type":"string"},
            "content":{"type":"string"}
        },"required":["path"]}),
    };
    let bash_open = Tool {
        name: "bash".into(),
        description: "Run a command".into(),
        parameters: json!({"type":"object","properties":{
            "command":{"type":"string"},
            "env":{"type":"object","additionalProperties":{"type":"string"}}
        },"required":["command"]}),
    };
    let incompatible = Tool {
        name: "find".into(),
        description: "Find a file".into(),
        parameters: json!({"type":"object","properties":{
            "query":{"oneOf":[{"type":"string"},{"type":"number"}]}
        }}),
    };
    let context = Context {
        messages: vec![Message::User(UserMessage::text("edit"))],
        tools: Some(vec![edit.clone(), bash_open.clone(), incompatible.clone()]),
        ..Default::default()
    };
    let mut opts = options();
    opts.tool_choice = Some(ara_ai::ToolChoice::Tool("edit".into()));
    let params = anthropic::build_params(&endpoint, &context, &opts).unwrap();
    assert_eq!(params["tools"][0]["strict"], true);
    assert_eq!(params["tools"][0]["input_schema"]["required"], json!(["path"]));
    assert_eq!(params["tools"][0]["input_schema"]["properties"]["content"]["type"], "string");
    assert!(params["tools"][1].get("strict").is_none());
    assert_eq!(
        params["tools"][1]["input_schema"]["properties"]["env"]["additionalProperties"],
        json!({"type":"string"})
    );
    assert!(params["tools"][2].get("strict").is_none());
    assert_eq!(params["tool_choice"], json!({"type":"tool","name":"edit"}));
    assert_eq!(context.tools.as_ref().unwrap()[0], edit);
    assert_eq!(context.tools.as_ref().unwrap()[1], bash_open);
    assert_eq!(context.tools.as_ref().unwrap()[2], incompatible);

    endpoint.base_url = "http://fixture/v1".into();
    let custom = anthropic::build_params(&endpoint, &context, &opts).unwrap();
    assert!(custom["tools"].as_array().unwrap().iter().all(|tool| tool.get("strict").is_none()));
    endpoint.base_url = "https://api.anthropic.com/custom".into();
    let custom_path = anthropic::build_params(&endpoint, &context, &opts).unwrap();
    assert!(custom_path["tools"].as_array().unwrap().iter().all(|tool| tool.get("strict").is_none()));
}

#[test]
fn official_strict_tool_budget_demotes_unrepresentable_schemas() {
    let mut properties = serde_json::Map::new();
    for index in 0..25 {
        properties.insert(format!("field_{index:02}"), json!({"type":"string"}));
    }
    let eligible = Tool {
        name: "edit".into(),
        description: "Edit".into(),
        parameters: json!({"type":"object","properties":properties}),
    };
    let mut unions = serde_json::Map::new();
    for index in 0..17 {
        unions.insert(format!("union_{index:02}"), json!({"type":["string","null"]}));
    }
    let over_union_budget = Tool {
        name: "find".into(),
        description: "Find".into(),
        parameters: json!({"type":"object","properties":unions}),
    };
    let context = Context {
        messages: vec![Message::User(UserMessage::text("use a tool"))],
        tools: Some(vec![eligible, over_union_budget]),
        ..Default::default()
    };
    let params = anthropic::build_params(&model("https://api.anthropic.com/v1"), &context, &options()).unwrap();
    let first = &params["tools"][0];
    assert_eq!(first["strict"], true);
    assert_eq!(first["input_schema"]["required"], json!(["field_24"]));
    assert_eq!(first["input_schema"]["properties"]["field_24"]["anyOf"][1]["type"], "null");
    assert!(params["tools"][1].get("strict").is_none());
    assert_eq!(params["tools"][1]["input_schema"]["properties"]["union_16"]["type"], json!(["string", "null"]));
}

#[test]
fn excessively_deep_tool_schema_fails_before_sending_a_request() {
    let mut nested = json!({"type":"string"});
    for _ in 0..130 {
        nested = json!({"type":"array","items":nested});
    }
    let context = Context {
        messages: vec![Message::User(UserMessage::text("hi"))],
        tools: Some(vec![Tool {
            name: "deep".into(),
            description: String::new(),
            parameters: json!({"type":"object","properties":{"value":nested}}),
        }]),
        ..Default::default()
    };
    let error = anthropic::build_params(&model("http://fixture/v1"), &context, &options()).unwrap_err();
    assert!(error.to_string().contains("tool input schema is too deep"));
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

    endpoint.max_tokens = None;
    opts.max_tokens = None;
    assert_eq!(anthropic::build_params(&endpoint, &context, &opts).unwrap()["max_tokens"], 4096);
    opts.max_tokens = Some(8192);
    assert_eq!(anthropic::build_params(&endpoint, &context, &opts).unwrap()["max_tokens"], 8192);
    opts.max_tokens = Some(128_000);
    assert_eq!(anthropic::build_params(&endpoint, &context, &opts).unwrap()["max_tokens"], 64_000);
    opts.max_tokens = Some(0);
    assert!(anthropic::build_params(&endpoint, &context, &opts).is_err());

    endpoint.max_tokens = Some(128_000);
    opts.max_tokens = Some(100_000);
    assert_eq!(anthropic::build_params(&endpoint, &context, &opts).unwrap()["max_tokens"], 100_000);
}

#[tokio::test]
async fn actual_messages_request_uses_explicit_limit_without_an_unknown_model_4096_cap() {
    for (model_ceiling, requested, expected) in [
        (None, None, 4096),
        (None, Some(8192), 8192),
        (None, Some(128_000), 64_000),
        (Some(128_000), Some(100_000), 100_000),
    ] {
        let server = FakeUpstream::start(
            upstream(vec![
                frame(json!({"type":"message_start","message":{"id":"msg_limit"}})),
                frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"ok"}})),
                frame(json!({"type":"content_block_stop","index":0})),
                frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
                frame(json!({"type":"message_stop"})),
            ]),
            None,
        )
        .await
        .unwrap();
        let mut endpoint = model(&server.base_url());
        endpoint.max_tokens = model_ceiling;
        let mut opts = options();
        opts.max_tokens = requested;
        let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
        let events = collect(anthropic::stream(reqwest::Client::new(), endpoint, context, opts)).await;
        assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
        assert_eq!(message(&events).text(), "ok");
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["body"]["max_tokens"], expected);
        assert_eq!(server.served(), 1);
    }

    let server = FakeUpstream::start(upstream(vec![]), None).await.unwrap();
    let mut opts = options();
    opts.max_tokens = Some(0);
    let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
    let events = collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
    assert!(message(&events).error_message.as_deref().unwrap().contains("max_tokens must be positive"));
    assert_eq!(server.served(), 0);
}

#[tokio::test]
async fn custom_anthropic_endpoint_uses_bearer_without_an_automatic_api_key_header() {
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[{"status":401,"body":"unauthorized"}]})).unwrap(),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("hi"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { .. })));
    let requests = server.requests.lock().await;
    let headers = &requests[0]["headers"];
    assert_eq!(headers["authorization"], format!("<redacted {} chars>", "Bearer anthropic-test-secret".len()));
    assert!(headers.get("x-api-key").is_none(), "{headers}");
}

#[tokio::test]
async fn custom_anthropic_endpoint_honors_explicit_auth_headers_once() {
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[{"status":401,"body":"unauthorized"}]})).unwrap(),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("hi"))], ..Default::default() };
    let mut opts = options();
    opts.extra_headers = vec![
        ("AUTHORIZATION".into(), "Bearer first".into()),
        ("authorization".into(), "Custom last".into()),
        ("X-API-KEY".into(), "explicit-key".into()),
    ];
    let events = collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { .. })));
    let requests = server.requests.lock().await;
    let headers = &requests[0]["headers"];
    assert_eq!(headers["authorization"], "<redacted 11 chars>");
    assert_eq!(headers["x-api-key"], "<redacted 12 chars>");
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
    assert_eq!(message(&events).tool_calls().count(), 1, "error snapshot retains the in-flight call");
    assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    assert!(message(&events).error_message.as_deref().unwrap().contains("before message_stop"));
}

#[tokio::test]
async fn malformed_final_tool_json_never_emits_a_completed_call() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_bad_tool"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_bad","name":"write","input":{}}})),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("write"))], ..Default::default() };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
    assert_eq!(message(&events).tool_calls().count(), 1, "partial call remains visible on error");
    assert!(message(&events).error_message.as_deref().unwrap().contains("invalid Anthropic tool input"));
    assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    assert_eq!(server.served(), 1);
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
async fn actual_messages_request_normalizes_images_and_keeps_unsupported_mime_visible() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_images"}})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let image = |mime_type: &str| UserBlock::Image(ImageContent { data: "YQ==".into(), mime_type: mime_type.into() });
    let context = Context {
        messages: vec![
            Message::User(UserMessage {
                content: UserContent::Blocks(vec![
                    UserBlock::text("   "),
                    image(" IMAGE/JPG "),
                    image("image/png"),
                    image("image/gif"),
                    image("image/webp"),
                    image(" image/svg+xml "),
                ]),
                synthetic: None,
                timestamp: 1,
            }),
            Message::User(UserMessage {
                content: UserContent::Blocks(vec![UserBlock::text(" \n ")]),
                synthetic: None,
                timestamp: 2,
            }),
        ],
        ..Default::default()
    };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
    let requests = server.requests.lock().await;
    let messages = requests[0]["body"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    let blocks = messages[0]["content"].as_array().unwrap();
    assert_eq!(blocks[0], json!({"type":"text","text":"(see attached image)"}));
    for (index, media_type) in ["image/jpeg", "image/png", "image/gif", "image/webp"].iter().enumerate() {
        assert_eq!(blocks[index + 1]["source"]["media_type"], *media_type);
    }
    assert_eq!(blocks[5], json!({"type":"text","text":"[unsupported image:  image/svg+xml ]"}));
}

#[tokio::test]
async fn actual_messages_request_keeps_error_image_hints_and_unsupported_text_in_result() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_results"}})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let image = |mime_type: &str| UserBlock::Image(ImageContent { data: "YQ==".into(), mime_type: mime_type.into() });
    let mut assistant = AssistantMessage::empty("anthropic-messages", "anthropic", "claude-fixture");
    assistant.content = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| {
            AssistantBlock::ToolCall(ara_ai::ToolCall {
                id: id.into(),
                name: "read".into(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            })
        })
        .collect();
    assistant.stop_reason = StopReason::ToolUse;
    let result = |id: &str, content: Vec<UserBlock>, is_error| {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "read".into(),
            content,
            details: None,
            is_error,
            timestamp: 2,
        })
    };
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("read")),
            Message::Assistant(assistant),
            result("a", vec![image("image/png")], true),
            result("b", vec![image("image/svg+xml")], true),
            result("c", Vec::new(), true),
            result("d", vec![image(" IMAGE/JPG ")], false),
            result("e", vec![UserBlock::text("  \n ")], false),
        ],
        ..Default::default()
    };
    let events =
        collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { .. })));
    let requests = server.requests.lock().await;
    let blocks = requests[0]["body"]["messages"][2]["content"].as_array().unwrap();
    assert_eq!(blocks[0]["content"], json!([{"type":"text","text":"(see attached image)"}]));
    assert_eq!(blocks[1]["content"], json!([{"type":"text","text":"[unsupported image: image/svg+xml]"}]));
    assert_eq!(blocks[2]["content"], "Tool failed with no output.");
    assert_eq!(blocks[3]["content"][0]["text"], "(see attached image)");
    assert_eq!(blocks[3]["content"][1]["source"]["media_type"], "image/jpeg");
    assert_eq!(blocks[4]["content"], "");
    assert_eq!(blocks[5]["text"], "Attached image(s) from the tool result(s) above:");
    assert_eq!(blocks[6]["source"]["media_type"], "image/png");
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
async fn pings_bridge_a_slow_tool_input_within_the_semantic_progress_cap() {
    let server = FakeUpstream::start(
        upstream(vec![
            frame(json!({"type":"message_start","message":{"id":"msg_slow_tool"}})),
            frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_slow","name":"read","input":{}}})),
            json!({"sleep_ms":220}),
            frame(json!({"type":"ping"})),
            json!({"sleep_ms":220}),
            frame(json!({"type":"ping"})),
            json!({"sleep_ms":220}),
            frame(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"slow.txt\"}"}})),
            frame(json!({"type":"content_block_stop","index":0})),
            frame(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}})),
            frame(json!({"type":"message_stop"})),
        ]),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("read"))], ..Default::default() };
    let mut opts = options();
    opts.idle_timeout = Some(Duration::from_millis(350));
    let events = collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::ToolUse, .. })));
    let calls: Vec<_> = message(&events).tool_calls().collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "toolu_slow");
    assert_eq!(calls[0].arguments["path"], "slow.txt");
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn pings_without_semantic_progress_end_a_partial_tool_call() {
    let mut frames = vec![
        frame(json!({"type":"message_start","message":{"id":"msg_stalled_tool"}})),
        frame(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_stalled","name":"read","input":{}}}),
        ),
    ];
    for _ in 0..30 {
        frames.push(json!({"sleep_ms":100}));
        frames.push(frame(json!({"type":"ping"})));
    }
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[{"events":frames,"end":"hang"}]})).unwrap(),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("read"))], ..Default::default() };
    let mut opts = options();
    opts.idle_timeout = Some(Duration::from_millis(250));
    let started = std::time::Instant::now();
    let events = collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    let elapsed = started.elapsed();
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
    assert_eq!(message(&events).tool_calls().count(), 1, "error snapshot retains the stalled call");
    assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    assert!(message(&events).error_message.as_deref().unwrap().contains("stalled"));
    assert!(elapsed >= Duration::from_millis(600), "pings were not honored: {elapsed:?}");
    assert!(elapsed < Duration::from_millis(1800), "ping cap did not end the stall: {elapsed:?}");
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn pings_before_message_start_do_not_satisfy_the_first_event_watchdog() {
    let mut preamble = Vec::new();
    for _ in 0..10 {
        preamble.push(json!({"sleep_ms":100}));
        preamble.push(frame(json!({"type":"ping"})));
    }
    let retry = vec![
        frame(json!({"type":"message_start","message":{"id":"msg_recovered"}})),
        frame(json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"recovered"}})),
        frame(json!({"type":"content_block_stop","index":0})),
        frame(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}})),
        frame(json!({"type":"message_stop"})),
    ];
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[{"events":preamble,"end":"hang"},{"events":retry}]})).unwrap(),
        None,
    )
    .await
    .unwrap();
    let context = Context { messages: vec![Message::User(UserMessage::text("reply"))], ..Default::default() };
    let mut opts = options();
    opts.first_event_timeout = Some(Duration::from_millis(350));
    let events = collect(anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    assert_eq!(message(&events).response_id.as_deref(), Some("msg_recovered"));
    assert_eq!(message(&events).text(), "recovered");
    assert_eq!(server.served(), 2);
}

#[tokio::test]
async fn cancellation_during_ping_keepalives_stops_the_partial_tool_call() {
    let mut frames = vec![
        frame(json!({"type":"message_start","message":{"id":"msg_cancel_pings"}})),
        frame(
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_cancel","name":"read","input":{}}}),
        ),
    ];
    for _ in 0..30 {
        frames.push(json!({"sleep_ms":100}));
        frames.push(frame(json!({"type":"ping"})));
    }
    let server = FakeUpstream::start(
        serde_json::from_value(json!({"responses":[{"events":frames,"end":"hang"}]})).unwrap(),
        None,
    )
    .await
    .unwrap();
    let cancel = CancellationToken::new();
    let context = Context { messages: vec![Message::User(UserMessage::text("read"))], ..Default::default() };
    let mut opts = options();
    opts.idle_timeout = Some(Duration::from_millis(250));
    opts.cancel = cancel.clone();
    let mut stream = anthropic::stream(reqwest::Client::new(), model(&server.base_url()), context, opts);
    let mut observed = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(2), stream.recv()).await.unwrap().unwrap();
        let tool_started = matches!(event, AssistantMessageEvent::ToolcallStart { .. });
        observed.push(event);
        if tool_started {
            break;
        }
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.cancel();
    observed.extend(collect(stream).await);
    assert!(matches!(observed.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Aborted, .. })));
    assert_eq!(message(&observed).tool_calls().count(), 1, "abort snapshot retains the cancelled call");
    assert!(!observed.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    assert_eq!(server.served(), 1);
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
