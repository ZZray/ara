//! Real HTTP Responses adapter tests against a controlled fake upstream.

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_completions::RetryPolicy;
use ara_ai::providers::openai_responses::{self, ProviderSessionState, StreamOptions};
use ara_ai::{
    AssistantMessage, Context, Message, Model, StopReason, Tool, ToolChoice, ToolResultMessage, UserBlock, UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn script(value: Value) -> Script {
    serde_json::from_value(value).unwrap()
}

fn model(base: &str) -> Model {
    Model {
        id: "fake-responses-model".into(),
        api: "openai-responses".into(),
        provider: "fake".into(),
        base_url: base.into(),
        reasoning: false,
        max_tokens: None,
        tokenizer: None,
    }
}

fn options() -> StreamOptions {
    StreamOptions {
        api_key: Some("test-secret".into()),
        retry: RetryPolicy {
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(10),
        },
        first_event_timeout: Some(Duration::from_secs(2)),
        idle_timeout: Some(Duration::from_secs(2)),
        ..StreamOptions::default()
    }
}

async fn collect(stream: ara_ai::AssistantStream) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut stream = stream;
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
    })
    .await
    .expect("Responses stream should finish");
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    let final_message = match events.last().unwrap() {
        AssistantMessageEvent::Done { message, .. } => message.clone(),
        AssistantMessageEvent::Error { error, .. } => error.clone(),
        _ => panic!("missing terminal event"),
    };
    (events, final_message)
}

#[tokio::test]
async fn response_terminal_ends_a_hanging_socket_and_records_the_native_request() {
    let server = FakeUpstream::start(script(json!({"responses":[{"events":[
        {"data":{"type":"response.created","response":{"id":"resp_1"}}},
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1"}}},
        {"data":{"type":"response.output_text.delta","output_index":0,"item_id":"msg_1","delta":"draft"}},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_1","content":[{"type":"output_text","text":"final answer"}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_1","status":"completed","usage":{"input_tokens":7,"output_tokens":2,"total_tokens":9,"input_tokens_details":{"cached_tokens":1}}}}}
    ],"end":"hang"}]})), None).await.unwrap();
    let context = Context {
        system_prompt: vec!["be concise".into()],
        messages: vec![Message::User(UserMessage::text("hi"))],
        tools: None,
    };
    let (events, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), context, options())).await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    assert_eq!(output.text(), "final answer");
    assert_eq!(output.response_id.as_deref(), Some("resp_1"));
    assert_eq!(output.usage.input, Some(6));
    assert_eq!(output.usage.cache_read, Some(1));
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["request"], "POST /v1/responses HTTP/1.1");
    assert_eq!(requests[0]["body"]["model"], "fake-responses-model");
    assert_eq!(requests[0]["body"]["instructions"], "be concise");
    assert_eq!(requests[0]["body"]["store"], false);
    assert_eq!(requests[0]["body"]["input"][0]["content"][0]["text"], "hi");
    assert!(requests[0]["headers"]["authorization"].as_str().unwrap().starts_with("<redacted"));
}

#[tokio::test]
async fn warmed_full_snapshot_replaces_prior_request_history_on_the_wire() {
    let server = FakeUpstream::start(
        script(json!({"responses":[
            {"events":[
                {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"seed"}]}}},
                {"data":{"type":"response.completed","response":{"status":"completed"}}}
            ]},
            {"events":[
                {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}},
                {"data":{"type":"response.completed","response":{"status":"completed"}}}
            ]}
        ]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, mut seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Default::default() },
        opts.clone(),
    ))
    .await;
    assert_eq!(seed.text(), "seed");
    let payload = seed.provider_payload.as_mut().expect("first response saved native history");
    payload["dt"] = json!(false);
    payload["items"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"canonical user"}]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"canonical answer"}]},
        {"type":"function_call","call_id":"call_branch","name":"read","arguments":"{}"}
    ]);
    let (_, output) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context {
            messages: vec![
                Message::User(UserMessage::text("old user")),
                Message::Assistant(seed),
                Message::User(UserMessage::text("follow up")),
            ],
            ..Default::default()
        },
        opts,
    ))
    .await;
    assert_eq!(output.text(), "done");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let input = requests[1]["body"]["input"].as_array().unwrap();
    assert_eq!(input.len(), 5);
    assert_eq!(input[0]["content"][0]["text"], "canonical user");
    assert_eq!(input[1]["content"][0]["text"], "canonical answer");
    assert_eq!(input[2]["type"], "function_call");
    assert_eq!(input[3]["type"], "function_call_output");
    assert_eq!(input[3]["call_id"], "call_branch");
    assert!(input[3]["output"].as_str().unwrap().contains("interrupted"));
    assert_eq!(input[4]["content"][0]["text"], "follow up");
    assert!(!requests[1]["body"].to_string().contains("old user"));
}

#[tokio::test]
async fn full_snapshot_tool_images_follow_the_host_vision_capability() {
    let response = json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}},
        {"data":{"type":"response.completed","response":{"status":"completed"}}}
    ]});
    let server = FakeUpstream::start(script(json!({"responses":[response.clone(), response.clone(), response]})), None)
        .await
        .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, mut seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Default::default() },
        opts.clone(),
    ))
    .await;
    let payload = seed.provider_payload.as_mut().expect("first response saved native history");
    payload["dt"] = json!(false);
    payload["items"] = json!([
        {"type":"message","role":"user","content":[{"type":"input_text","text":"canonical"}]},
        {"type":"function_call","call_id":"call_image","name":"read","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_image","output":[
            {"type":"input_image","image_url":"data:image/png;base64,AAEC"}
        ]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"canonical answer"}]}
    ]);
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("old user")),
            Message::Assistant(seed),
            Message::User(UserMessage::text("follow up")),
        ],
        ..Default::default()
    };
    let (_, no_vision) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(no_vision.text(), "ok");
    opts.request.supports_images = true;
    let (_, vision) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(vision.text(), "ok");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["input"][0]["content"][0]["text"], "old user");
    assert!(!requests[1]["body"].to_string().contains("data:image/png"));
    assert_eq!(requests[2]["body"]["input"][0]["content"][0]["text"], "canonical");
    assert_eq!(requests[2]["body"]["input"][2]["output"][0]["image_url"], "data:image/png;base64,AAEC");
    assert!(!requests[2]["body"].to_string().contains("old user"));
}

#[tokio::test]
async fn incompatible_tool_schema_is_quarantined_on_the_actual_responses_request() {
    let server = FakeUpstream::start(script(json!({"responses":[{"events":[
        {"data":{"type":"response.output_item.done","output_index":0,
            "item":{"type":"message","content":[{"type":"output_text","text":"Only the safe tool is available."}]}}},
        {"data":{"type":"response.completed","response":{"status":"completed"}}}
    ]}]})), None).await.unwrap();
    let context = Context {
        messages: vec![Message::User(UserMessage::text("check tools"))],
        tools: Some(vec![
            Tool {
                name: "broken".into(),
                description: "bad schema".into(),
                parameters: json!({"type":"object","properties":{"value":{"type":"null","const":"not null"}}}),
            },
            Tool {
                name: "safe".into(),
                description: "safe schema".into(),
                parameters: json!({"type":"object","properties":{
                    "value":{"oneOf":[{"type":"string"},{"type":"null"}]},
                    "patternOnly":{"pattern":"^(?!bad$)"}
                }}),
            },
        ]),
        ..Context::default()
    };
    let mut opts = options();
    opts.request.tool_choice = Some(ToolChoice::Tool("broken".into()));
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert_eq!(output.text(), "Only the safe tool is available.");
    let requests = server.requests.lock().await;
    let body = &requests[0]["body"];
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["name"], "safe");
    assert_eq!(body["tools"][0]["parameters"]["properties"]["value"]["anyOf"].as_array().unwrap().len(), 2);
    assert!(body["tools"][0]["parameters"]["properties"]["value"].get("oneOf").is_none());
    assert_eq!(body["tools"][0]["parameters"]["properties"]["patternOnly"], true);
    assert!(body.get("tool_choice").is_none());
}

#[tokio::test]
async fn draft_07_tool_schema_is_upgraded_on_the_actual_responses_request() {
    let server = FakeUpstream::start(
        script(json!({"responses":[{"events":[
            {"data":{"type":"response.output_item.done","output_index":0,
                "item":{"type":"message","content":[{"type":"output_text","text":"Schema received."}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}]})),
        None,
    )
    .await
    .unwrap();
    let context = Context {
        messages: vec![Message::User(UserMessage::text("check schema"))],
        tools: Some(vec![Tool {
            name: "legacy".into(),
            description: "legacy schema".into(),
            parameters: json!({
                "$schema":"http://json-schema.org/draft-07/schema#",
                "type":"object",
                "properties":{
                    "pair":{"type":"array","items":[{"type":"string"},{"type":"integer"}],"additionalItems":false},
                    "gate":{"type":"object","dependencies":{"a":["b"]}},
                    "name":{"type":"string","nullable":true},
                    "item":{"$ref":"#/definitions/Item"},
                    "literal":{"default":{"definitions":{"Example":{}}}}
                },
                "definitions":{"Item":{"type":"string"}}
            }),
        }]),
        ..Context::default()
    };
    let mut opts = options();
    opts.request.tool_choice = Some(ToolChoice::Tool("legacy".into()));
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), context, opts)).await;
    assert_eq!(output.text(), "Schema received.");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let body = &requests[0]["body"];
    assert_eq!(body["tool_choice"], json!({"type":"function","name":"legacy"}));
    let parameters = &body["tools"][0]["parameters"];
    assert_eq!(parameters["$schema"], "https://json-schema.org/draft/2020-12/schema");
    assert_eq!(parameters["$defs"]["Item"]["type"], "string");
    assert_eq!(parameters["properties"]["item"]["$ref"], "#/$defs/Item");
    assert_eq!(parameters["properties"]["pair"]["prefixItems"], json!([{"type":"string"},{"type":"integer"}]));
    assert_eq!(parameters["properties"]["pair"]["items"], false);
    assert_eq!(parameters["properties"]["gate"]["dependentRequired"]["a"], json!(["b"]));
    assert_eq!(parameters["properties"]["name"]["type"], json!(["string", "null"]));
    assert_eq!(parameters["properties"]["literal"]["default"], json!({"definitions":{"Example":{}}}));
    assert!(parameters.get("definitions").is_none());
    assert!(parameters["properties"]["pair"].get("additionalItems").is_none());
    assert!(parameters["properties"]["gate"].get("dependencies").is_none());
    assert!(parameters["properties"]["name"].get("nullable").is_none());
}

#[tokio::test]
async fn tool_result_is_replayed_on_the_next_real_http_request() {
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read"}}},
            {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"a.txt\"}"}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":"{}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_2","content":[{"type":"output_text","text":"read complete"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let endpoint = model(&server.base_url());
    let context = Context { messages: vec![Message::User(UserMessage::text("read a.txt"))], ..Context::default() };
    let (_, first) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context, options())).await;
    assert_eq!(first.stop_reason, StopReason::ToolUse);
    let call = first.tool_calls().next().unwrap();
    assert_eq!(call.id, "call_1|fc_1");
    assert_eq!(call.arguments["path"], "a.txt");
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("read a.txt")),
            Message::Assistant(first),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1|fc_1".into(),
                tool_name: "read".into(),
                content: vec![UserBlock::text("file body")],
                details: None,
                is_error: false,
                timestamp: 1,
            }),
        ],
        ..Context::default()
    };
    let (_, second) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, options())).await;
    assert_eq!(second.text(), "read complete");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    let input = requests[1]["body"]["input"].as_array().unwrap();
    assert_eq!(input[1]["call_id"], "call_1");
    assert_eq!(input[2], json!({"type":"function_call_output","call_id":"call_1","output":"file body"}));
}

#[tokio::test]
async fn failed_and_retried_cold_requests_warm_only_after_replayable_success() {
    let secret = "opaque-cold-warm-marker";
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_seed","summary":[{"type":"summary_text","text":"plan"}],"encrypted_content":secret}}},
            {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"message","id":"msg_seed","content":[{"type":"output_text","text":"seed answer"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[{"data":{"type":"response.failed","response":{"status":"failed","error":{"message":"temporary failure"}}}}]},
        {"events":[{"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}}]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_recovered","content":[{"type":"output_text","text":"recovered"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_warm","content":[{"type":"output_text","text":"warm"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let endpoint = model(&server.base_url());
    let (_, seed) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), Context::default(), options()))
            .await;
    assert_eq!(seed.provider_payload.as_ref().unwrap()["dt"], true);
    let context = Context {
        messages: vec![Message::Assistant(seed), Message::User(UserMessage::text("continue"))],
        ..Context::default()
    };
    let session_state = Arc::new(ProviderSessionState::default());
    let mut opts = options();
    opts.session_state = Some(session_state.clone());
    let (_, failed) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(failed.stop_reason, StopReason::Error);
    opts.retry.max_attempts = 2;
    let (_, recovered) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(recovered.text(), "recovered");
    let (_, warmed) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(warmed.text(), "warm");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 5);
    for index in [1, 2, 3] {
        let input = requests[index]["body"]["input"].as_array().unwrap();
        assert!(input.iter().all(|item| item["type"] != "reasoning"));
        assert!(input.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "seed answer"));
    }
    let warm_input = requests[4]["body"]["input"].as_array().unwrap();
    assert!(warm_input.iter().any(|item| item["type"] == "reasoning" && item["encrypted_content"] == secret));
}

#[tokio::test]
async fn incomplete_visible_response_warms_native_history_but_new_session_starts_cold() {
    let secret = "opaque-incomplete-marker";
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_partial","encrypted_content":secret}}},
            {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"message","id":"msg_partial","content":[{"type":"output_text","text":"partial answer"}]}}},
            {"data":{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"continued"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"resumed"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), Context::default(), opts.clone()))
            .await;
    assert_eq!(first.stop_reason, StopReason::Length);
    assert_eq!(first.provider_payload.as_ref().unwrap()["items"][0]["encrypted_content"], secret);
    let context = Context {
        messages: vec![Message::Assistant(first), Message::User(UserMessage::text("continue"))],
        ..Context::default()
    };
    let (_, second) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(second.text(), "continued");
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, resumed) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(resumed.text(), "resumed");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(
        requests[1]["body"]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "reasoning" && item["encrypted_content"] == secret)
    );
    assert!(requests[2]["body"]["input"].as_array().unwrap().iter().all(|item| item["type"] != "reasoning"));
}

#[tokio::test]
async fn hidden_only_incomplete_response_is_saved_without_warming_the_session() {
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_seed","encrypted_content":"seed-secret"}}},
            {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"message","content":[{"type":"output_text","text":"seed answer"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"reasoning","id":"rs_hidden","encrypted_content":"hidden-secret"}}},
            {"data":{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"continued"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let endpoint = model(&server.base_url());
    let (_, seed) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), Context::default(), options()))
            .await;
    let context = Context {
        messages: vec![Message::Assistant(seed), Message::User(UserMessage::text("continue"))],
        ..Context::default()
    };
    let mut opts = options();
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, hidden) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(hidden.stop_reason, StopReason::Length);
    assert_eq!(hidden.provider_payload.as_ref().unwrap()["items"][0]["encrypted_content"], "hidden-secret");
    let (_, next) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(next.text(), "continued");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    let input = requests[2]["body"]["input"].as_array().unwrap();
    assert!(input.iter().all(|item| item["type"] != "reasoning"));
    assert!(input.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "seed answer"));
}

#[tokio::test]
async fn eof_after_tool_start_never_reissues_the_request_or_succeeds() {
    let server = FakeUpstream::start(script(json!({"responses":[{"events":[
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read"}}}
    ]}]})), None).await.unwrap();
    let (_, output) = collect(openai_responses::stream(
        reqwest::Client::new(),
        model(&server.base_url()),
        Context::default(),
        options(),
    ))
    .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn reasoning_boundary_before_disconnect_is_not_retried() {
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}},
            {"data":{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"plan"}},
            {"data":{"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"wrong retry"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let mut opts = options();
    opts.retry.max_attempts = 2;
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts))
            .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert_eq!(server.served(), 1);
    assert!(output.provider_payload.is_none());
}

#[tokio::test]
async fn empty_reasoning_start_can_retry_a_disconnected_stream() {
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[{"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}}]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"recovered"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let mut opts = options();
    opts.retry.max_attempts = 2;
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts))
            .await;
    assert_eq!(output.text(), "recovered");
    assert_eq!(server.served(), 2);
}

#[tokio::test]
async fn completed_reasoning_text_before_disconnect_is_not_retried() {
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning"}}},
            {"data":{"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":"plan"}}
        ]},
        {"events":[
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","content":[{"type":"output_text","text":"wrong retry"}]}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}
    ]})), None).await.unwrap();
    let mut opts = options();
    opts.retry.max_attempts = 2;
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts))
            .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn unknown_events_do_not_extend_the_idle_deadline() {
    let server = FakeUpstream::start(script(json!({"responses":[{"events":[
        {"data":{"type":"response.created","response":{"id":"resp_idle"}}},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_idle","content":[{"type":"output_text","text":"partial"}]}}},
        {"sleep_ms":60}, {"data":{"type":"response.heartbeat"}},
        {"sleep_ms":60}, {"data":{"type":"response.heartbeat"}},
        {"sleep_ms":60}, {"data":{"type":"response.heartbeat"}}
    ],"end":"hang"}]})), None).await.unwrap();
    let mut opts = options();
    opts.idle_timeout = Some(Duration::from_millis(100));
    let started = Instant::now();
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts))
            .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert!(output.error_message.as_deref().unwrap_or("").contains("stalled"), "{:?}", output.error_message);
    assert!(started.elapsed() < Duration::from_millis(240), "unknown events extended the idle deadline");
}

#[tokio::test]
async fn cancelling_after_a_partial_tool_start_remains_aborted() {
    let server = FakeUpstream::start(script(json!({"responses":[{"events":[
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_cancel","call_id":"call_cancel","name":"read"}}},
        {"data":{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"path\":"}}
    ],"end":"hang"}]})), None).await.unwrap();
    let opts = options();
    let cancel = opts.cancel.clone();
    let mut stream =
        openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts);
    let mut saw_partial_tool = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.recv().await {
            if matches!(event, AssistantMessageEvent::ToolcallDelta { .. }) {
                saw_partial_tool = true;
                cancel.cancel();
            }
            if let AssistantMessageEvent::Error { error, .. } = event {
                assert_eq!(error.stop_reason, StopReason::Aborted);
                assert!(error.provider_payload.is_none());
                return;
            }
        }
        panic!("cancelled Responses stream ended without Error");
    })
    .await
    .expect("cancellation should end the stream");
    assert!(saw_partial_tool);
    assert_eq!(server.served(), 1);
}
