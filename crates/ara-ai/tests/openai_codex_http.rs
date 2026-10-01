//! Codex daily SSE module: real local sockets, fake upstream transcripts.
//! Fixed OMP source scenarios: openai-codex-stream.test.ts,
//! openai-codex-include.test.ts and providers/openai-codex-error.test.ts.
//! This is transport evidence, not live OAuth or subscription acceptance.

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_codex_responses::{self as codex, OpenAICodexResponsesProvider, StreamOptions};
use ara_ai::providers::openai_completions::RetryPolicy;
use ara_ai::{
    AssistantBlock, AssistantMessage, CallOptions, Context, Message, Model, ModelProvider, StopReason, Tool,
    ToolChoice, ToolResultMessage, UserBlock, UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::time::Duration;

fn script(value: Value) -> Script {
    serde_json::from_value(value).unwrap()
}

fn model(base: &str) -> Model {
    Model {
        id: "gpt-5.3-codex-spark".into(),
        api: codex::API.into(),
        provider: "openai-codex".into(),
        base_url: base.into(),
        reasoning: true,
        // Capacity metadata must never become a Codex output cap.
        max_tokens: Some(128000),
        tokenizer: None,
    }
}

fn options() -> StreamOptions {
    StreamOptions {
        api_key: Some("fake-oauth-access".into()),
        extra_headers: vec![
            (codex::ACCOUNT_HEADER.into(), "fixture-account".into()),
            (codex::RESIDENCY_HEADER.into(), "eu".into()),
        ],
        session_id: Some("fixture-host-session".into()),
        retry: RetryPolicy {
            max_attempts: 1,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        },
        first_event_timeout: Some(Duration::from_secs(2)),
        idle_timeout: Some(Duration::from_secs(2)),
        ..Default::default()
    }
}

fn text_response(text: &str) -> Value {
    json!({"events": [
        {"data": {"type": "response.output_item.done", "output_index": 0, "item": {
            "type": "message", "id": "msg_final", "role": "assistant", "content": [{"type": "output_text", "text": text}]
        }}},
        {"data": {"type": "response.completed", "response": {"id": "resp_final", "status": "completed"}}}
    ]})
}

async fn collect(mut stream: ara_ai::AssistantStream) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
    })
    .await
    .expect("Codex stream must end");
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    let output = events.last().expect("terminal").partial().clone();
    assert_eq!(output.api, codex::API);
    assert_eq!(output.provider, "openai-codex");
    (events, output)
}

#[tokio::test]
async fn function_stream_and_serialized_resume_replay_full_encrypted_history() {
    let server = FakeUpstream::start(script(json!({"responses": [
        {"events": [
            {"data": {"type": "response.created", "response": {"id": "resp_tool"}}},
            {"data": {"type": "response.output_item.done", "output_index": 0, "item": {
                "type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "inspect"}], "encrypted_content": "opaque_fixture"
            }}},
            {"data": {"type": "response.output_item.added", "output_index": 1, "item": {
                "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read"
            }}},
            {"data": {"type": "response.function_call_arguments.delta", "output_index": 1, "item_id": "fc_1", "delta": "{\"path\":"}},
            {"data": {"type": "response.function_call_arguments.delta", "output_index": 1, "item_id": "fc_1", "delta": "\"file.txt\"}"}},
            {"data": {"type": "response.output_item.done", "output_index": 1, "item": {
                "type": "function_call", "id": "fc_1", "call_id": "call_1", "name": "read", "arguments": "{\"path\":\"file.txt\"}"
            }}},
            {"data": {"type": "response.completed", "response": {"id": "resp_tool", "status": "completed", "usage": {"input_tokens": 7, "output_tokens": 3}}}}
        ]},
        text_response("finished")
    ]})), None).await.unwrap();
    let endpoint = model(&format!("{}/codex/responses", server.base_url()));
    let user = Message::User(UserMessage::text("read file.txt"));
    let tool = Tool {
        name: "read".into(),
        description: "read a file".into(),
        parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}),
    };
    let context = Context {
        system_prompt: vec!["base instructions".into(), "extra constraints".into()],
        messages: vec![user.clone()],
        tools: Some(vec![tool]),
    };
    let provider = OpenAICodexResponsesProvider { client: reqwest::Client::new(), base: options() };
    let (events, output) = collect(provider.stream(
        &endpoint,
        &context,
        CallOptions { tool_choice: Some(ToolChoice::Auto), ..Default::default() },
    ))
    .await;
    assert_eq!(output.stop_reason, StopReason::ToolUse);
    assert!(events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    let call = output.tool_calls().next().unwrap();
    assert_eq!(call.arguments["path"], "file.txt");
    let call_id = call.id.clone();
    let hidden_item = json!({"type": "reasoning", "id": "rs_hidden", "summary": [{"type": "summary_text", "text": "inspect"}], "encrypted_content": "opaque_hidden_fixture"});
    let mut hidden = output.clone();
    hidden.stop_reason = StopReason::Stop;
    hidden.content.retain(|block| matches!(block, AssistantBlock::Thinking(_)));
    if let AssistantBlock::Thinking(thinking) = &mut hidden.content[0] {
        thinking.thinking_signature = Some(hidden_item.to_string());
    }
    hidden.provider_payload.as_mut().unwrap()["items"] = json!([hidden_item]);
    let hidden: AssistantMessage = serde_json::from_str(&serde_json::to_string(&hidden).unwrap()).unwrap();
    // A new provider has no warmed in-memory state, as after process restart.
    let restored: AssistantMessage = serde_json::from_str(&serde_json::to_string(&output).unwrap()).unwrap();
    let mut resumed = context;
    resumed.messages = vec![
        user,
        Message::Assistant(hidden),
        Message::Assistant(restored),
        Message::ToolResult(ToolResultMessage {
            tool_call_id: call_id,
            tool_name: "read".into(),
            content: vec![UserBlock::text("contents")],
            details: None,
            is_error: false,
            timestamp: 1,
        }),
    ];
    let (_, done) = collect(codex::stream(reqwest::Client::new(), endpoint.clone(), resumed, options())).await;
    assert_eq!(done.text(), "finished");
    assert_eq!(done.stop_reason, StopReason::Stop);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_eq!(request["request"], "POST /v1/codex/responses HTTP/1.1");
        let headers = &request["headers"];
        assert_eq!(headers["chatgpt-account-id"], "fixture-account");
        assert_eq!(headers["x-openai-internal-codex-residency"], "eu");
        assert_eq!(headers["openai-beta"], "responses=experimental");
        assert_eq!(headers["originator"], "omp");
        assert_eq!(headers["version"], "0.153.0");
        assert_eq!(headers["x-codex-routing-hint"], "model=gpt-5.3-codex-spark");
        for name in ["session_id", "conversation_id", "x-client-request-id"] {
            assert_eq!(headers[name], "fixture-host-session");
        }
        assert_eq!(headers["accept"], "text/event-stream");
        assert_eq!(headers["content-type"], "application/json");
        let body = &request["body"];
        assert_eq!(body["model"], endpoint.id);
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(body["instructions"], "base instructions");
        assert_eq!(body["prompt_cache_key"], "fixture-host-session");
        assert_eq!(body["include"], json!(["reasoning.encrypted_content"]));
        assert!(body.get("max_output_tokens").is_none());
        assert!(body.get("previous_response_id").is_none());
        assert_eq!(body["input"][0]["role"], "developer");
        assert_eq!(body["tools"][0]["type"], "function");
    }
    let input = requests[1]["body"]["input"].as_array().unwrap();
    assert!(input.iter().any(|item| item["encrypted_content"] == "opaque_fixture"));
    assert!(input.iter().any(|item| item["encrypted_content"] == "opaque_hidden_fixture"));
    let call = input.iter().find(|item| item["type"] == "function_call").unwrap();
    let result = input.iter().find(|item| item["type"] == "function_call_output").unwrap();
    assert_eq!(call["call_id"], result["call_id"]);
    assert_eq!(result["output"], "contents");
    assert_eq!(codex::resolve_responses_url("https://example.test/codex/"), "https://example.test/codex/responses");
    assert_eq!(codex::resolve_responses_url("https://example.test/"), "https://example.test/codex/responses");
}

#[tokio::test]
async fn unsupported_options_identity_and_history_fail_before_network() {
    let server = FakeUpstream::start(script(json!({"responses": []})), None).await.unwrap();
    let endpoint = model(&server.base_url());
    let mut cap = options();
    cap.request.max_tokens = Some(10);
    let mut sampling = options();
    sampling.request.temperature = Some(0.1);
    let mut images = options();
    images.request.supports_images = true;
    let mut cold = options();
    cold.request.native_history_replay = Some(false);
    let mut header = options();
    header.extra_headers.push(("OpenAI-Beta".into(), "wrong".into()));
    let mut account = options();
    account.extra_headers.clear();
    for opts in [cap, sampling, images, cold, header, account] {
        let (events, output) =
            collect(codex::stream(reqwest::Client::new(), endpoint.clone(), Context::default(), opts)).await;
        assert_eq!(output.stop_reason, StopReason::Error);
        assert_eq!(events.len(), 1);
        assert!(output.error_message.is_some());
    }
    let mut native = AssistantMessage::empty(codex::API, "openai-codex", &endpoint.id);
    native.provider_payload = Some(json!({"items": [{"type": "custom_tool_call", "call_id": "call_custom"}]}));
    let (_, output) = collect(codex::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::Assistant(native)], ..Default::default() },
        options(),
    ))
    .await;
    assert_eq!(output.stop_reason, StopReason::Error);
    assert_eq!(server.served(), 0);
    // The generic public entry remains protocol-specific.
    assert!(
        ara_ai::providers::openai_responses::build_request(&endpoint, &Context::default(), &Default::default())
            .is_err()
    );
}

#[tokio::test]
async fn incomplete_function_arguments_are_not_promoted_to_an_executable_call() {
    let server = FakeUpstream::start(script(json!({"responses": [{"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "function_call", "id": "fc_partial", "call_id": "call_partial", "name": "write"}}},
        {"data": {"type": "response.function_call_arguments.delta", "output_index": 0, "item_id": "fc_partial", "delta": "{\"path\":"}},
        {"data": {"type": "response.incomplete", "response": {"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}}}}
    ]}]})), None).await.unwrap();
    let (events, output) =
        collect(codex::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), options())).await;
    assert_eq!(output.stop_reason, StopReason::Length);
    assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
    assert_ne!(output.stop_reason, StopReason::ToolUse);
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn output_errors_unsupported_native_items_and_eof_never_report_success_or_replay() {
    let after_output = vec![
        json!({"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg"}}}),
        json!({"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "msg", "delta": "draft"}}),
    ];
    let mut failed = after_output.clone();
    failed.push(json!({"data": {"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "backend unavailable"}}}}));
    let mut envelope = after_output.clone();
    envelope.push(json!({"data": {"type": "error", "code": "server_error", "message": "backend unavailable"}}));
    let responses = vec![
        json!({"events": failed}),
        json!({"events": envelope}),
        json!({"events": after_output}),
        json!({"events": [{"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "custom_tool_call", "call_id": "native_call"}}}]}),
        json!({"events": [{"data": {"type": "response.completed", "response": {"status": "completed", "output": [{"type": "message", "content": [{"type": "output_image", "image_url": "fixture"}]}]}}}]}),
        json!({"status": 401, "body": "{\"detail\":\"fixture unauthorized\"}"}),
    ];
    for response in responses {
        let server =
            FakeUpstream::start(script(json!({"responses": [response, text_response("must not replay")]})), None)
                .await
                .unwrap();
        let mut opts = options();
        opts.retry.max_attempts = 3;
        let (_, output) =
            collect(codex::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts)).await;
        assert_eq!(output.stop_reason, StopReason::Error);
        assert!(output.error_message.is_some());
        assert_eq!(server.served(), 1);
        assert!(output.usage.total_tokens.is_none());
    }
}

#[tokio::test]
async fn cancellation_after_visible_output_ends_once_without_replay() {
    let server = FakeUpstream::start(
        script(json!({"responses": [{"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "msg"}}},
        {"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "msg", "delta": "draft"}}
    ], "end": "hang"}]})),
        None,
    )
    .await
    .unwrap();
    let mut opts = options();
    opts.retry.max_attempts = 3;
    let cancel = opts.cancel.clone();
    let mut stream = codex::stream(reqwest::Client::new(), model(&server.base_url()), Context::default(), opts);
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.recv().await {
            if matches!(event, AssistantMessageEvent::TextDelta { .. }) {
                cancel.cancel();
                return;
            }
            assert!(!event.is_terminal());
        }
        panic!("no visible output");
    })
    .await
    .unwrap();
    let (_, output) = collect(stream).await;
    assert_eq!(output.stop_reason, StopReason::Aborted);
    assert_eq!(output.text(), "draft");
    assert_eq!(server.served(), 1);
}

#[tokio::test]
async fn safe_pre_output_retry_reuses_the_same_auth_and_full_request() {
    for failure in [
        json!({"status": 503, "body": "{\"error\":{\"code\":\"server_error\",\"message\":\"fixture unavailable\"}}"}),
        json!({"events": [{"data": {"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "backend unavailable"}}}}]}),
    ] {
        let server = FakeUpstream::start(script(json!({"responses": [failure, text_response("recovered")]})), None)
            .await
            .unwrap();
        let mut opts = options();
        opts.retry.max_attempts = 2;
        let (_, output) = collect(codex::stream(
            reqwest::Client::new(),
            model(&server.base_url()),
            Context { messages: vec![Message::User(UserMessage::text("hello"))], ..Default::default() },
            opts,
        ))
        .await;
        assert_eq!(output.stop_reason, StopReason::Stop);
        assert_eq!(output.text(), "recovered");
        assert_eq!(server.served(), 2);
        let requests = server.requests.lock().await;
        assert_eq!(requests[0]["body"], requests[1]["body"]);
        assert_eq!(requests[0]["headers"], requests[1]["headers"]);
    }
}
