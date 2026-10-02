//! Fixed OMP remote-compaction input families, grouped at the wire module.
//! Source: packages/agent/test/remote-compaction.test.ts at 596f2da7101178214aa27a753529d15e6b7ad91d.
//! Real local sockets and controlled responses; no live credentials.

use ara_ai::providers::{openai_codex_responses as codex, openai_responses as responses};
use ara_ai::remote_compaction::{
    self as remote, RemoteConfig, RemoteGenericRequest, RemoteNativeRequest, RemoteRequestOptions, RemoteVersion,
};
use ara_ai::remote_compaction_v2 as v2;
use ara_ai::{
    AssistantBlock, AssistantMessage, Context, Message, Model, ProviderError, StopReason, ThinkingContent, ToolCall,
    ToolResultMessage, UserBlock, UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn model(base: &str) -> Model {
    Model {
        id: "gpt-5".into(),
        api: responses::API.into(),
        provider: "custom-openai".into(),
        base_url: base.into(),
        reasoning: true,
        max_tokens: Some(128_000),
        context_window: None,
        tokenizer: None,
    }
}

fn options() -> RemoteRequestOptions {
    RemoteRequestOptions {
        api_key: Some("fake-remote-secret".into()),
        timeout: Some(Duration::from_secs(2)),
        retry_delay: Duration::from_millis(1),
        session_id: Some("transport-remote".into()),
        prompt_cache_key: Some("stable-cache".into()),
        ..Default::default()
    }
}

fn request(version: RemoteVersion) -> RemoteNativeRequest {
    RemoteNativeRequest {
        config: RemoteConfig {
            enabled: Some(true),
            v2_streaming_enabled: Some(true),
            model: Some("gpt-5-compact".into()),
            ..Default::default()
        },
        version,
        instructions: "preserve the plan".into(),
        previous_replacement_history: None,
    }
}

fn context() -> Context {
    Context {
        system_prompt: vec!["original instructions".into()],
        messages: vec![Message::User(UserMessage::text("real user"))],
        tools: None,
    }
}

async fn server(responses: Vec<Value>) -> FakeUpstream {
    FakeUpstream::start(serde_json::from_value::<Script>(json!({"responses":responses})).unwrap(), None).await.unwrap()
}

fn compaction_stream(items: Vec<Value>, terminal: Option<Value>) -> Value {
    let mut events: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| json!({"data":{"type":"response.output_item.done","output_index":index,"item":item}}))
        .collect();
    if let Some(terminal) = terminal {
        events.push(json!({"data":terminal}));
    }
    json!({"events":events})
}

fn completion() -> Value {
    json!({"type":"response.completed","response":{"usage":{"input_tokens":123,"output_tokens":4,"total_tokens":127,"input_tokens_details":{"cached_tokens":7},"output_tokens_details":{"reasoning_tokens":1}}}})
}

#[test]
fn history_preserve_budget_and_retained_user_families() {
    let route = model("https://example.test/v1");
    let reasoning = json!({"type":"reasoning","encrypted_content":"encrypted-history","summary":[]});
    let mut assistant = AssistantMessage::empty(responses::API, &route.provider, &route.id);
    assistant.stop_reason = StopReason::ToolUse;
    assistant.content = vec![
        AssistantBlock::Thinking(ThinkingContent {
            thinking: "plan".into(),
            thinking_signature: Some(reasoning.to_string()),
        }),
        AssistantBlock::ToolCall(ToolCall {
            id: "call_1|fc_1".into(),
            name: "read".into(),
            arguments: json!({"path":"a"}).as_object().unwrap().clone(),
            thought_signature: None,
        }),
        AssistantBlock::text("trailing text"),
    ];
    let raw = Context {
        messages: vec![
            Message::Assistant(assistant),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_1|fc_1".into(),
                tool_name: "read".into(),
                content: vec![UserBlock::text("output")],
                details: None,
                is_error: false,
                timestamp: 1,
            }),
        ],
        ..Default::default()
    };
    let saved = raw.clone();
    let items = remote::build_native_history(&route, &raw, None).unwrap();
    assert_eq!(
        items.iter().map(|item| item["type"].as_str().unwrap()).collect::<Vec<_>>(),
        ["reasoning", "message", "function_call", "function_call_output"]
    );
    assert_eq!(items[0], reasoning);
    assert_eq!(items[2]["call_id"], "call_1");
    assert_eq!(items[3]["call_id"], "call_1");
    assert_eq!(raw, saved);
    let summary = json!({"type":"compaction_summary"});
    let preserve = json!({remote::PRESERVE_KEY:{"provider":route.provider,"replacementHistory":[summary],"compactionItem":summary}});
    let parsed = remote::parse_native_preserve_data(&preserve).unwrap();
    assert!(parsed.used_tokens.is_none());
    let payload = remote::native_replacement_payload(&route, &parsed.replacement_history).unwrap();
    let mut snapshot = AssistantMessage::empty(responses::API, &route.provider, &route.id);
    snapshot.provider_payload = Some(payload);
    let input = remote::build_native_history(
        &route,
        &Context {
            messages: vec![
                Message::User(UserMessage::text("old")),
                Message::Assistant(snapshot),
                Message::User(UserMessage::text("new")),
            ],
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(input[0], summary);
    assert_eq!(input.len(), 2);
    let mut invalid = preserve.clone();
    invalid[remote::PRESERVE_KEY]["compactionItem"] = json!({"type":"compaction"});
    assert!(remote::parse_native_preserve_data(&invalid).is_none());

    let mut limited = route.clone();
    limited.context_window = Some(1000.0);
    let overflow = vec![
        json!({"type":"function_call","call_id":"a","name":"read","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"a","output":"x".repeat(5000)}),
    ];
    let trim = remote::trim_native_input_to_context_window(&limited, &overflow, "compact", None);
    assert_eq!(trim.rewritten_outputs, 1);
    assert_eq!(trim.input[0], overflow[0]);
    assert_eq!(overflow[1]["output"].as_str().unwrap().len(), 5000);
    assert!(!trim.exact_tokenizer);
    let stranded = vec![
        json!({"type":"function_call_output","call_id":"a","output":"x".repeat(5000)}),
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"y".repeat(3000)}]}),
    ];
    assert_eq!(remote::trim_native_input_to_context_window(&limited, &stranded, "compact", None).input, stranded);

    let compact = json!({"type":"compaction","encrypted_content":"enc"});
    let retained = vec![
        json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"dev"}]}),
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":" <environment_context> repo"}]}),
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"old"}]}),
        json!({"type":"message","role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AA"},{"type":"input_text","text":"abcdefghijkl"}]}),
    ];
    let (small, images) = v2::build_replacement_history(&retained, &compact, 2);
    assert_eq!(images, 0);
    assert_eq!(small.len(), 2);
    assert_eq!(small[0]["content"][0]["text"], "abcdefgh");
    assert_eq!(small[1], compact);
    let (large, images) = v2::build_replacement_history(&retained, &compact, 64_000);
    assert_eq!(images, 1);
    assert_eq!(large.len(), 3);
    assert_eq!(v2::resolve_retained_budget(u64::MAX), 64_000);
    assert!(remote::should_use_v1(&route, &request(RemoteVersion::V1).config));
    assert!(remote::should_use_v2(&route, &request(RemoteVersion::V2).config));
}

#[tokio::test]
async fn native_v1_v2_http_preserve_and_cold_resume_family() {
    let compact = json!({"type":"compaction_summary"});
    let fake = server(vec![json!({"body":json!({"output":[{"type":"reasoning","encrypted_content":"discard"},compact,{"type":"message","role":"assistant","content":[{"type":"output_text","text":"retained"}]}]}).to_string()})]).await;
    let result = remote::request_native(
        reqwest::Client::new(),
        model(&fake.base_url()),
        context(),
        request(RemoteVersion::V1),
        options(),
    )
    .await
    .unwrap();
    let parsed = remote::parse_native_preserve_data(result.preserve_data.as_ref().unwrap()).unwrap();
    assert_eq!(parsed.replacement_history.len(), 2);
    assert_eq!(parsed.compaction_item, compact);
    assert!(result.usage.is_unknown());
    let records = fake.requests.lock().await;
    assert!(records[0]["request"].as_str().unwrap().contains("/v1/responses/compact"));
    assert_eq!(records[0]["body"]["model"], "gpt-5-compact");
    assert!(records[0]["body"].get("stream").is_none());
    drop(records);

    let compaction = json!({"type":"compaction","encrypted_content":"enc_123"});
    let fake = server(vec![
        compaction_stream(
            vec![
                json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"ignored"}]}),
                compaction.clone(),
            ],
            Some(completion()),
        ),
        compaction_stream(
            vec![json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"resumed"}]} )],
            Some(json!({"type":"response.completed","response":{"id":"next","status":"completed"}})),
        ),
    ])
    .await;
    let route = model(&fake.base_url());
    let mut ctx = context();
    ctx.messages.insert(0, Message::User(UserMessage::text("<skills> context")));
    let result =
        remote::request_native(reqwest::Client::new(), route.clone(), ctx, request(RemoteVersion::V2), options())
            .await
            .unwrap();
    assert_eq!(result.usage.total_tokens, Some(127));
    assert_eq!(result.usage.cache_read, Some(7));
    assert_eq!(result.attempts.len(), 1);
    let parsed = remote::parse_native_preserve_data(result.preserve_data.as_ref().unwrap()).unwrap();
    assert_eq!(parsed.used_tokens, Some(123));
    assert_eq!(parsed.replacement_history.len(), 2);
    assert_eq!(parsed.replacement_history.last(), Some(&compaction));
    let mut snapshot = AssistantMessage::empty(responses::API, &route.provider, &route.id);
    snapshot.provider_payload = remote::native_replacement_payload(&route, &parsed.replacement_history);
    let resumed = Context {
        messages: vec![Message::Assistant(snapshot), Message::User(UserMessage::text("continue"))],
        ..Default::default()
    };
    let mut stream = responses::stream(
        reqwest::Client::new(),
        route,
        resumed,
        responses::StreamOptions {
            session_state: Some(Arc::new(responses::ProviderSessionState::default())),
            api_key: Some("fake-remote-secret".into()),
            ..Default::default()
        },
    );
    let mut terminal = None;
    while let Some(event) = stream.recv().await {
        if event.is_terminal() {
            terminal = Some(event.partial().clone());
        }
    }
    assert_eq!(terminal.unwrap().text(), "resumed");
    let records = fake.requests.lock().await;
    assert_eq!(records[0]["body"]["input"].as_array().unwrap().last().unwrap(), &json!({"type":"compaction_trigger"}));
    assert_eq!(records[0]["body"]["store"], false);
    assert_eq!(records[0]["body"]["prompt_cache_key"], "stable-cache");
    assert!(records[0]["body"].get("max_output_tokens").is_none());
    assert!(records[1]["body"]["input"].as_array().unwrap().iter().any(|item| item == &compaction));
}

#[tokio::test]
async fn v2_fault_retry_auth_timeout_and_cancellation_families() {
    let compact = json!({"type":"compaction","encrypted_content":"enc"});
    let okay = compaction_stream(vec![compact.clone()], Some(completion()));
    for first in [
        json!({"status":500,"body":"try again"}),
        json!({"events":[{"raw":"data: {broken}\n\n"}]}),
        compaction_stream(vec![compact.clone()], None),
    ] {
        let fake = server(vec![first, okay.clone()]).await;
        let result = remote::request_native(
            reqwest::Client::new(),
            model(&fake.base_url()),
            context(),
            request(RemoteVersion::V2),
            options(),
        )
        .await
        .unwrap();
        assert_eq!(result.attempts.len(), 2);
        assert_eq!(fake.served(), 2);
        assert!(result.attempts[0].error.is_some());
    }
    let fake = server(vec![compaction_stream(vec![compact.clone(), compact.clone()], Some(completion()))]).await;
    let error = remote::request_native(
        reqwest::Client::new(),
        model(&fake.base_url()),
        context(),
        request(RemoteVersion::V2),
        options(),
    )
    .await
    .unwrap_err();
    assert_eq!(fake.served(), 1);
    assert!(error.to_string().contains("exactly one"));
    assert_eq!(error.attempts[0].usage.total_tokens, Some(127));
    let fake = server(vec![json!({"status":503,"body":json!({"error":{"type":"auth_unavailable","message":"no lease; fake-remote-secret"}}).to_string()})]).await;
    let error = remote::request_native(
        reqwest::Client::new(),
        model(&fake.base_url()),
        context(),
        request(RemoteVersion::V2),
        options(),
    )
    .await
    .unwrap_err();
    assert!(error.auth_failed);
    assert_eq!(fake.served(), 1);
    assert!(!format!("{error:?}").contains("fake-remote-secret"));
    let fake = server(vec![json!({"delay_ms":1000}); 3]).await;
    let mut timed = options();
    timed.timeout = Some(Duration::from_millis(20));
    let error = remote::request_native(
        reqwest::Client::new(),
        model(&fake.base_url()),
        context(),
        request(RemoteVersion::V2),
        timed,
    )
    .await
    .unwrap_err();
    assert!(matches!(error.cause, ProviderError::Timeout(_)));
    assert_eq!(error.attempts.len(), 3);
    assert_eq!(fake.served(), 3);
    for version in [RemoteVersion::V1, RemoteVersion::V2] {
        let fake = server(vec![json!({"events":[],"end":"hang"})]).await;
        let opts = options();
        let cancel = opts.cancel.clone();
        let route = model(&fake.base_url());
        let work = tokio::spawn(async move {
            remote::request_native(reqwest::Client::new(), route, context(), request(version), opts).await
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while fake.served() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let error = work.await.unwrap().unwrap_err();
        assert_eq!(error.cause, ProviderError::Aborted);
        assert_eq!(fake.served(), 1);
    }
}

#[tokio::test]
async fn generic_json_chat_formats_and_codex_negotiation_family() {
    for (suffix, reply, expected, auth) in [
        (
            "/chat/completions?query=x",
            json!({"choices":[{"message":{"content":[{"type":"text","text":"remote "},{"text":"summary"}]}}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
            "remote summary",
            true,
        ),
        ("/summarize", json!({"summary":"generic summary","shortSummary":"generic"}), "generic summary", false),
    ] {
        let fake = server(vec![json!({"body":reply.to_string()})]).await;
        let result = remote::request_generic(
            reqwest::Client::new(),
            model(&fake.base_url()),
            format!("{}{suffix}", fake.base_url()),
            RemoteGenericRequest {
                system_prompt: "summarize".into(),
                prompt: "conversation".into(),
                max_tokens: Some(16_384),
            },
            options(),
        )
        .await
        .unwrap();
        assert_eq!(result.summary, expected);
        assert!(result.preserve_data.is_none());
        let records = fake.requests.lock().await;
        assert_eq!(records[0]["headers"].get("authorization").is_some(), auth);
        if auth {
            assert_eq!(records[0]["body"]["messages"].as_array().unwrap().len(), 2);
            assert_eq!(records[0]["body"]["max_tokens"], 16_384);
            assert_eq!(result.usage.total_tokens, Some(5));
        } else {
            assert_eq!(records[0]["body"]["maxTokens"], 16_384);
            assert!(records[0]["body"].get("messages").is_none());
            assert_eq!(result.short_summary.as_deref(), Some("generic"));
        }
    }
    let compact = json!({"type":"compaction","encrypted_content":"enc"});
    let fake = server(vec![compaction_stream(vec![compact.clone()], Some(completion()))]).await;
    let mut route = model(&fake.base_url());
    route.api = codex::API.into();
    route.provider = "openai-codex".into();
    let mut req = request(RemoteVersion::V2);
    req.config.v2_endpoint = Some(format!("{}/responses", fake.base_url()));
    let mut opts = options();
    opts.extra_headers =
        vec![(codex::ACCOUNT_HEADER.into(), "fixture-account".into()), (codex::RESIDENCY_HEADER.into(), "us".into())];
    remote::request_native(reqwest::Client::new(), route.clone(), context(), req, opts).await.unwrap();
    let records = fake.requests.lock().await;
    assert_eq!(records[0]["headers"]["x-codex-beta-features"], "remote_compaction_v2");
    assert_eq!(records[0]["headers"][codex::ACCOUNT_HEADER], "fixture-account");
    assert_eq!(records[0]["headers"][codex::RESIDENCY_HEADER], "us");
    drop(records);
    let mut native = AssistantMessage::empty(codex::API, &route.provider, &route.id);
    native.provider_payload = remote::native_replacement_payload(&route, &[compact]);
    let encoded = codex::build_request(
        &route,
        &Context { messages: vec![Message::Assistant(native)], ..Default::default() },
        &responses::RequestOptions::default(),
    )
    .unwrap();
    assert_eq!(encoded["input"][0]["type"], "compaction");
}
