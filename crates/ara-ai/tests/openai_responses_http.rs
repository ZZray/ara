//! Real HTTP Responses adapter tests against a controlled fake upstream.

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_completions::RetryPolicy;
use ara_ai::providers::openai_responses::{self, StreamOptions};
use ara_ai::{AssistantMessage, Context, Message, Model, StopReason, ToolResultMessage, UserBlock, UserMessage};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
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
