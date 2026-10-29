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

#[path = "support/freeform_protocol.rs"]
mod freeform_protocol;

#[path = "support/computer_protocol.rs"]
mod computer_protocol;

#[tokio::test]
async fn computer_responses_wire_contract_group() {
    computer_protocol::verify(
        &|base, context, request, cancel| {
            let mut opts = options();
            opts.request = request;
            opts.cancel = cancel;
            opts.retry.max_attempts = 2;
            openai_responses::stream(reqwest::Client::new(), model(&base), context, opts)
        },
        &model,
        false,
    )
    .await;
}

#[tokio::test]
async fn conditional_freeform_responses_protocol_group() {
    freeform_protocol::verify(&|base, context, request, cancel| {
        let mut opts = options();
        opts.request = request;
        opts.cancel = cancel;
        opts.retry.max_attempts = 2;
        openai_responses::stream(reqwest::Client::new(), model(&base), context, opts)
    })
    .await;
}

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
        context_window: None,
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

fn completed_text(id: &str, text: &str) -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,
            "item":{"type":"message","id":format!("msg_{id}"),"role":"assistant",
                "content":[{"type":"output_text","text":text}]}}},
        {"data":{"type":"response.completed","response":{"id":id,"status":"completed",
            "usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}}}}
    ]})
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
async fn reasoning_responses_requests_preserve_native_summary_off_mapping_and_mandatory_routes() {
    use ara_ai::{
        CallOptions, ModelProvider, OpenAIReasoningPolicy, OpenAIResponsesProvider, ReasoningEffort as Effort,
        ReasoningRequest, ReasoningSummary as Summary,
    };
    let effort = ReasoningRequest { effort: Some(Effort::High), ..Default::default() };
    let off = ReasoningRequest { disable_reasoning: true, ..Default::default() };
    let force_off = ReasoningRequest { force_reasoning_off: true, ..Default::default() };
    let cases = [
        (json!({}), effort.clone(), json!({"effort":"baked-high","summary":"auto"})),
        (
            json!({"reasoningEffortMap":{"high":"wire-high"}}),
            ReasoningRequest { summary: Summary::Hidden, ..effort.clone() },
            json!({"effort":"wire-high"}),
        ),
        (
            json!({}),
            ReasoningRequest { summary: Summary::Concise, ..effort.clone() },
            json!({"effort":"baked-high","summary":"concise"}),
        ),
        (
            json!({}),
            ReasoningRequest { summary: Summary::Detailed, ..effort.clone() },
            json!({"effort":"baked-high","summary":"detailed"}),
        ),
        (json!({"supportsReasoningSummary":false}), effort.clone(), json!({"effort":"baked-high"})),
        (
            json!({"omitReasoningEffort":true}),
            ReasoningRequest { summary: Summary::Auto, ..effort.clone() },
            json!({"summary":"auto"}),
        ),
        (
            json!({"omitReasoningEffort":true}),
            ReasoningRequest { summary: Summary::Hidden, ..effort.clone() },
            Value::Null,
        ),
        (json!({"reasoningDisableMode":"none-effort"}), off.clone(), json!({"effort":"none"})),
        (json!({"reasoningDisableMode":"lowest-effort"}), off.clone(), json!({"effort":"low"})),
        (json!({"reasoningDisableMode":"openrouter-enabled-false"}), off.clone(), json!({"enabled":false})),
        (json!({"reasoningDisableMode":"omit"}), off.clone(), Value::Null),
        (json!({"reasoningDisableMode":"omit"}), force_off.clone(), json!({"effort":"none"})),
    ];
    for (compat, request, expected) in cases {
        let server =
            FakeUpstream::start(script(json!({"responses":[completed_text("reasoning","ok")]})), None).await.unwrap();
        let mut endpoint = model(&server.base_url());
        endpoint.reasoning = true;
        let original = endpoint.clone();
        let mut base = options();
        base.request.reasoning_policy = Some(
            OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,
                "thinking":{"mode":"effort","efforts":["low","high"],"effortMap":{"high":"baked-high"}},"compat":compat
            }))
            .unwrap(),
        );
        base.request.reasoning =
            Some(ReasoningRequest { effort: Some(Effort::Low), summary: Summary::Detailed, ..Default::default() });
        let provider = OpenAIResponsesProvider { client: reqwest::Client::new(), base };
        let (_, output) = collect(provider.stream(
            &endpoint,
            &Context::default(),
            CallOptions { reasoning: Some(request), ..Default::default() },
        ))
        .await;
        assert_eq!(output.text(), "ok");
        assert_eq!(endpoint, original);
        let recorded = server.requests.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["body"]["reasoning"], expected, "compat={compat}");
    }
    for (mandatory, suppress_off) in [(false, false), (true, false), (true, true)] {
        let server =
            FakeUpstream::start(script(json!({"responses":[completed_text("mandatory","ok")]})), None).await.unwrap();
        let mut endpoint = model(&server.base_url());
        endpoint.reasoning = true;
        let mut base = options();
        base.request.reasoning = Some(off.clone());
        base.request.reasoning_policy = Some(OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,"requestModelId":"high-default",
            "thinking":{"mode":"effort","efforts":["low","high"],"requiresEffort":mandatory,"suppressWhenOff":suppress_off,"effortRouting":{"low":"low-route","high":"high-default","off":"off-route"}},
            "compat":{"reasoningDisableMode":"none-effort"}
        })).unwrap());
        assert_eq!(
            collect(openai_responses::stream(reqwest::Client::new(), endpoint, Context::default(), base))
                .await
                .1
                .text(),
            "ok"
        );
        let recorded = server.requests.lock().await;
        let body = &recorded[0]["body"];
        assert_eq!(body["model"], if mandatory && !suppress_off { "high-default" } else { "off-route" });
        assert_eq!(
            body["reasoning"],
            if mandatory && !suppress_off {
                json!({"effort":"high","summary":"auto"})
            } else {
                json!({"effort":"none"})
            }
        );
    }
    let server = FakeUpstream::start(script(json!({"responses":[completed_text("juice","ok")]})), None).await.unwrap();
    let mut endpoint = model(&server.base_url());
    endpoint.reasoning = true;
    let mut base = options();
    base.request.reasoning =
        Some(ReasoningRequest { force_reasoning_off: true, effort: Some(Effort::High), ..Default::default() });
    base.request.reasoning_policy = Some(
        OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,"reasoningMode":"pro",
            "thinking":{"mode":"effort","efforts":["high"]},"compat":{"requiresReasoningOffJuiceInstruction":true}
        }))
        .unwrap(),
    );
    assert_eq!(
        collect(openai_responses::stream(reqwest::Client::new(), endpoint, Context::default(), base)).await.1.text(),
        "ok"
    );
    let recorded = server.requests.lock().await;
    assert_eq!(recorded[0]["body"]["reasoning"], json!({"effort":"none"}));
    assert_eq!(
        recorded[0]["body"]["input"].as_array().unwrap().last().unwrap()["content"][0]["text"],
        "# Juice: 48 !important"
    );
    drop(recorded);
    for request in [effort.clone(), off.clone()] {
        let server =
            FakeUpstream::start(script(json!({"responses":[completed_text("pro","ok")]})), None).await.unwrap();
        let mut endpoint = model(&server.base_url());
        endpoint.reasoning = true;
        let mut base = options();
        base.request.reasoning = Some(request.clone());
        base.request.reasoning_policy = Some(
            OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,"reasoningMode":"pro",
                "thinking":{"mode":"effort","efforts":["high"]},"compat":{"reasoningDisableMode":"none-effort"}
            }))
            .unwrap(),
        );
        assert_eq!(
            collect(openai_responses::stream(reqwest::Client::new(), endpoint, Context::default(), base))
                .await
                .1
                .text(),
            "ok"
        );
        let recorded = server.requests.lock().await;
        assert_eq!(
            recorded[0]["body"]["reasoning"],
            if request.disable_reasoning {
                json!({"effort":"none","mode":"pro"})
            } else {
                json!({"effort":"high","summary":"auto","mode":"pro"})
            }
        );
    }
    for summary in [Summary::ProviderDefault, Summary::Detailed] {
        let server =
            FakeUpstream::start(script(json!({"responses":[completed_text("no-dial","ok")]})), None).await.unwrap();
        let mut endpoint = model(&server.base_url());
        endpoint.reasoning = true;
        let mut base = options();
        base.request.reasoning = Some(ReasoningRequest { effort: Some(Effort::High), summary, ..Default::default() });
        base.request.reasoning_policy = Some(
            OpenAIReasoningPolicy::from_model_spec(
                &json!({"reasoning":true,"compat":{"supportsReasoningEffort":false}}),
            )
            .unwrap(),
        );
        assert_eq!(
            collect(openai_responses::stream(reqwest::Client::new(), endpoint, Context::default(), base))
                .await
                .1
                .text(),
            "ok"
        );
        let recorded = server.requests.lock().await;
        assert_eq!(
            recorded[0]["body"]["reasoning"],
            if summary == Summary::Detailed { json!({"summary":"detailed"}) } else { Value::Null }
        );
    }
    let policy = OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,"requestModelId":"default-high",
        "thinking":{"mode":"effort","efforts":["low","high","max"],"defaultLevel":"high","requiresEffort":true,"prefixBinding":true,"supportsDisplay":false,"effortRouting":{"high":"default-high"}},"compat":{}
    })).unwrap();
    assert_eq!(policy.supported_efforts(), &[Effort::Low, Effort::High, Effort::Max]);
    assert_eq!(policy.clamp_effort(Effort::Medium), Some(Effort::Low));
    assert_eq!(policy.clamp_effort(Effort::Minimal), Some(Effort::Low));
    assert_eq!(policy.default_effort(), Some(Effort::High));
    assert_eq!(policy.default_supported_effort(), Some(Effort::High));
    assert!(!policy.model_supports_effort_ceiling(Effort::Minimal));
    assert!(policy.model_supports_effort_ceiling(Effort::Low));
    // Native ladder eligibility is distinct from the effective mandatory
    // default route, which the Host must check for this particular request.
    let normalized = policy.normalize_request(Some(&off)).unwrap();
    assert_eq!(normalized.effort, Some(Effort::High));
    assert!(!normalized.disable_reasoning && !normalized.force_reasoning_off);
    assert!(normalized.effort.unwrap() > Effort::Low);
    let no_dial = OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true})).unwrap();
    assert!(no_dial.model_supports_effort_ceiling(Effort::Minimal));
    assert_eq!(no_dial.normalize_request(Some(&effort)).unwrap().effort, None);
    let sparse = OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,
        "thinking":{"mode":"effort","efforts":["max"]}
    }))
    .unwrap();
    assert!(!sparse.model_supports_effort_ceiling(Effort::XHigh));
    assert!(sparse.model_supports_effort_ceiling(Effort::Max));
}

#[tokio::test]
async fn successful_terminal_keeps_context_recovery_wire_proof() {
    for length in [false, true] {
        for native in [false, true] {
            let terminal = if length {
                json!({"type":"response.incomplete","response":{"status":"incomplete", "incomplete_details":{"reason":"max_output_tokens"}}})
            } else {
                json!({"type":"response.completed","response":{"status":"completed"}})
            };
            let mut events = vec![json!({"data":{"type":"response.created","response":{"id":"terminal-proof"}}})];
            if native {
                events.push(json!({"data":{"type":"response.future_native_completed","item":{"type":"future_native","id":"unknown-effect"}}}));
            }
            events.push(json!({"data":terminal}));
            let server = FakeUpstream::start(script(json!({"responses":[{"events":events}]})), None).await.unwrap();
            let (_, output) = collect(openai_responses::stream(
                reqwest::Client::new(),
                model(&server.base_url()),
                Context::default(),
                options(),
            ))
            .await;
            assert_eq!(
                output.stop_reason,
                if length { StopReason::Length } else { StopReason::Stop },
                "length={length} native={native}: {:?}",
                output.error_message
            );
            assert_eq!(
                output.terminal_context_recovery,
                Some(if native {
                    ara_ai::ContextRecoveryEvidence::NativeOutput
                } else {
                    ara_ai::ContextRecoveryEvidence::ContentOnly
                })
            );
            assert!(output.failure_evidence.is_none(), "successful terminal must not fabricate a failure");
            let mut old = serde_json::to_value(&output).unwrap();
            old.as_object_mut().unwrap().remove("terminalContextRecovery");
            assert_eq!(serde_json::from_value::<AssistantMessage>(old).unwrap().terminal_context_recovery, None);
            assert_eq!(server.served(), 1);
        }
    }
}

#[tokio::test]
async fn context_recovery_wire_facts_preserve_replay_vetoes_and_unknown_journals() {
    use ara_ai::ContextRecoveryEvidence::{ContentOnly, NativeOutput, NativeValidation, UsageAdmission};
    use ara_ai::providers::openai_codex_responses as codex;
    use ara_ai::retry_classification::classify_retry;
    let message = json!({"type":"response.output_item.done","output_index":0,
        "item":{"type":"message","id":"msg","content":[
            {"type":"output_text","text":"draft"},{"type":"refusal","refusal":"refusal"}]}});
    let overflow = json!({"type":"response.failed","response":{"status":"failed",
        "error":{"code":"context_length_exceeded","message":"maximum context length is 4000 tokens"}}});
    let cases = [
        ("message", vec![message.clone(), overflow.clone()], ContentOnly, false),
        (
            "reasoning",
            vec![
                json!({"type":"response.output_item.done","output_index":0,
            "item":{"type":"reasoning","id":"rs","summary":[{"type":"summary_text","text":"thinking"}]}}),
                overflow.clone(),
            ],
            ContentOnly,
            false,
        ),
        (
            "function-before-parse",
            vec![json!({"type":"response.output_item.added","output_index":0,
            "item":{"type":"function_call","id":"fc","name":"read"}})],
            NativeOutput,
            false,
        ),
        (
            "function-before-lookup",
            vec![
                json!({"type":"response.function_call_arguments.done",
            "item_id":"missing","arguments":"{}"}),
                overflow.clone(),
            ],
            NativeOutput,
            false,
        ),
        (
            "unknown-item",
            vec![json!({"type":"response.output_item.done","item":{"type":"future_native"}})],
            NativeOutput,
            false,
        ),
        (
            "unknown-terminal-output",
            vec![json!({"type":"response.failed","response":{"status":"failed",
            "output":[{"type":"future_native"}],"error":{"message":"maximum context length is 4000 tokens"}}})],
            NativeOutput,
            false,
        ),
        (
            "function-terminal-output",
            vec![json!({"type":"response.failed","response":{"status":"failed",
            "output":[{"type":"function_call","call_id":"hidden","name":"write","arguments":"{}"}],
            "error":{"message":"maximum context length is 4000 tokens"}}})],
            NativeOutput,
            false,
        ),
        (
            "unknown-message-part",
            vec![
                json!({"type":"response.output_item.done","output_index":0,
            "item":{"type":"message","content":[{"type":"output_image"}]}}),
                overflow.clone(),
            ],
            NativeOutput,
            false,
        ),
        (
            "malformed-known-part",
            vec![
                json!({"type":"response.content_part.added","part":{"type":"output_text","text":7}}),
                message.clone(),
                overflow.clone(),
            ],
            NativeOutput,
            false,
        ),
        (
            "unknown-event-then-message",
            vec![json!({"type":"future.native_tool"}), message.clone(), overflow.clone()],
            NativeOutput,
            false,
        ),
        (
            "usage-admission",
            vec![
                message.clone(),
                json!({"type":"response.failed","response":{"status":"failed",
            "error":{"code":"usage_limit_reached","message":"maximum context length is 4000 tokens"}}}),
            ],
            UsageAdmission,
            false,
        ),
        (
            "codex-usage-admission",
            vec![
                message.clone(),
                json!({"type":"response.failed","response":{"status":"failed",
            "error":{"code":"usage_limit_reached","message":"maximum context length is 4000 tokens"}}}),
            ],
            UsageAdmission,
            true,
        ),
        (
            "codex-validator",
            vec![json!({"type":"response.content_part.added","part":{"type":"output_image"}})],
            NativeValidation,
            true,
        ),
    ];
    for (name, frames, expected, is_codex) in cases {
        let response = json!({"events":frames.into_iter().map(|data| json!({"data":data})).collect::<Vec<_>>()});
        let server =
            FakeUpstream::start(script(json!({"responses":[response, completed_text("wrong", "wrong retry")]})), None)
                .await
                .unwrap();
        let mut selected = model(&server.base_url());
        let mut opts = options();
        opts.retry.max_attempts = 2;
        let stream = if is_codex {
            selected.api = codex::API.into();
            selected.provider = "openai-codex".into();
            codex::stream(
                reqwest::Client::new(),
                selected,
                Context::default(),
                codex::StreamOptions {
                    api_key: opts.api_key,
                    extra_headers: vec![(codex::ACCOUNT_HEADER.into(), "fixture-account".into())],
                    retry: opts.retry,
                    first_event_timeout: opts.first_event_timeout,
                    idle_timeout: opts.idle_timeout,
                    ..Default::default()
                },
            )
        } else {
            openai_responses::stream(reqwest::Client::new(), selected, Context::default(), opts)
        };
        let (_, output) = collect(stream).await;
        assert_eq!(output.stop_reason, StopReason::Error, "{name}");
        assert_eq!(server.served(), 1, "{name}: no transparent replay");
        let decoded: AssistantMessage = serde_json::from_value(serde_json::to_value(&output).unwrap()).unwrap();
        assert_eq!(decoded.failure_evidence.as_ref().unwrap().context_recovery, Some(expected), "{name}");
        let api = if is_codex { codex::API } else { openai_responses::API };
        let class = classify_retry(&decoded, api);
        assert_eq!(class.context_recovery_blocked, expected != ContentOnly, "{name}");
        if name.ends_with("usage-admission") {
            let evidence = decoded.failure_evidence.as_ref().unwrap();
            assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Stream, "{name}");
            assert_eq!(evidence.code.as_deref(), Some("usage_limit_reached"), "{name}");
            assert!(class.usage_limit && class.replay_blocked && evidence.replay_blocked, "{name}");
        }
        if name == "message" {
            assert!(class.overflow && class.replay_blocked);
            assert!(decoded.failure_evidence.as_ref().unwrap().replay_blocked);
            let mut legacy = serde_json::to_value(&decoded).unwrap();
            legacy["failureEvidence"].as_object_mut().unwrap().remove("contextRecovery");
            let legacy: AssistantMessage = serde_json::from_value(legacy).unwrap();
            assert!(classify_retry(&legacy, api).context_recovery_blocked, "absent proof stays unknown");
        }
        if name == "function-before-parse" || name == "function-terminal-output" || name == "unknown-terminal-output" {
            assert_eq!(output.tool_calls().count(), 0, "{name}: unsafe facts do not require projected tools");
        }
    }
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
async fn failed_response_frames_preserve_nested_codes_and_reasons() {
    let terminal_frames = [
        json!({"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "backend exploded"}}}),
        json!({"type": "response.failed", "response": {"status": "failed", "incomplete_details": {"reason": "max_output_tokens"}}}),
        json!({"type": "response.completed", "response": {"status": "failed", "status_details": {"error": {"code": "server_error", "message": "backend exploded late"}}}}),
        json!({"type": "response.completed", "response": {"status": "failed", "error": null, "status_details": {"error": {"code": "server_error", "message": "nested survives null"}}}}),
        json!({"type": "response.completed", "response": {"status": "cancelled", "status_details": {"reason": "capacity_exceeded"}}}),
        json!({"type": "error", "error": {"code": "upstream_error", "message": "stream broke"}}),
        json!({"type": "error", "error": null, "code": "upstream_error", "message": "top-level survives null"}),
    ];
    let responses = terminal_frames.into_iter().map(|terminal| json!({"events": [
        {"data": {"type": "response.output_item.added", "output_index": 0, "item": {"type": "message", "id": "partial"}}},
        {"data": {"type": "response.output_text.delta", "output_index": 0, "item_id": "partial", "delta": "draft"}},
        {"data": terminal}
    ]})).collect::<Vec<_>>();
    let server = FakeUpstream::start(script(json!({"responses": responses})), None).await.unwrap();
    for (index, expected) in [
        "server_error: backend exploded",
        "incomplete: max_output_tokens",
        "server_error: backend exploded late",
        "server_error: nested survives null",
        "status_details: capacity_exceeded",
        "Error Code upstream_error: stream broke",
        "Error Code upstream_error: top-level survives null",
    ]
    .iter()
    .enumerate()
    {
        let (events, output) = collect(openai_responses::stream(
            reqwest::Client::new(),
            model(&server.base_url()),
            Context::default(),
            options(),
        ))
        .await;
        assert!(matches!(events.last(), Some(AssistantMessageEvent::Error { reason: StopReason::Error, .. })));
        assert_eq!(output.error_message.as_deref(), Some(*expected));
        assert_eq!(output.text(), "draft", "case {index} preserves emitted output");
        assert_eq!(output.tool_calls().count(), 0);
    }
    assert_eq!(server.served(), 7, "emitted output must not be replayed");
}

#[tokio::test]
async fn pre_output_server_error_retries_once_without_duplicate_client_events() {
    let server = FakeUpstream::start(script(json!({"responses": [
        {"events": [
            {"data": {"type": "response.failed", "response": {"status": "failed", "error": {"code": "server_error", "message": "backend unavailable"}}}}
        ]},
        completed_text("recovered", "recovered answer")
    ]})), None).await.unwrap();
    let (events, output) = collect(openai_responses::stream(
        reqwest::Client::new(),
        model(&server.base_url()),
        Context::default(),
        options(),
    ))
    .await;
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::Start { .. })).count(), 1);
    assert_eq!(output.text(), "recovered answer");
    assert_eq!(output.tool_calls().count(), 0);
    assert_eq!(server.served(), 2, "only the uncommitted failed attempt is retried");
}

#[tokio::test]
async fn opted_in_chain_sends_only_the_new_user_turn_on_real_http() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_first", "first answer"), completed_text("resp_second", "second answer")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let first_context =
        Context { messages: vec![Message::User(UserMessage::text("first question"))], ..Context::default() };
    let (_, first) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), first_context, opts.clone())).await;
    assert_eq!(first.text(), "first answer");
    let second_context = Context {
        messages: vec![
            Message::User(UserMessage::text("first question")),
            Message::Assistant(first),
            Message::User(UserMessage::text("second question")),
        ],
        ..Context::default()
    };
    let (_, second) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, second_context, opts)).await;
    assert_eq!(second.text(), "second answer");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["body"]["store"], true);
    assert!(requests[0]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_first");
    assert_eq!(requests[1]["body"]["store"], true);
    assert_eq!(requests[1]["body"]["input"].as_array().unwrap().len(), 1);
    assert_eq!(requests[1]["body"]["input"][0]["content"][0]["text"], "second question");
}

#[tokio::test]
async fn stale_chain_retries_once_with_complete_history_before_start() {
    let server = FakeUpstream::start(
        script(json!({"responses":[
            completed_text("resp_first", "first answer"),
            {"status":400,"body":"{\"error\":{\"message\":\"previous_response_id not found\"}}"},
            completed_text("resp_fallback", "recovered")
        ]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("first"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("first")),
            Message::Assistant(first),
            Message::User(UserMessage::text("next")),
        ],
        ..Context::default()
    };
    let (events, output) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(output.text(), "recovered");
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::Start { .. })).count(), 1);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_first");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["input"].as_array().unwrap().len(), 3);
    assert_eq!(requests[2]["body"]["store"], true);
}

#[tokio::test]
async fn coded_stale_chain_retries_even_when_the_message_does_not_name_the_previous_response() {
    for field in ["code", "type"] {
        let mut rejection = json!({"error":{"message":"lookup failed"}});
        rejection["error"][field] = json!("previous_response_not_found");
        let server = FakeUpstream::start(
            script(json!({"responses":[
                completed_text("resp_first", "first answer"),
                {"status":400,"body":rejection.to_string()},
                completed_text("resp_fallback", "recovered")
            ]})),
            None,
        )
        .await
        .unwrap();
        let endpoint = model(&server.base_url());
        let mut opts = options();
        opts.stateful_responses = true;
        opts.session_state = Some(Arc::new(ProviderSessionState::default()));
        let (_, first) = collect(openai_responses::stream(
            reqwest::Client::new(),
            endpoint.clone(),
            Context { messages: vec![Message::User(UserMessage::text("first"))], ..Context::default() },
            opts.clone(),
        ))
        .await;
        let context = Context {
            messages: vec![
                Message::User(UserMessage::text("first")),
                Message::Assistant(first),
                Message::User(UserMessage::text("next")),
            ],
            ..Context::default()
        };
        let (events, output) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
        assert_eq!(output.text(), "recovered");
        assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::Start { .. })).count(), 1);
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1]["body"]["previous_response_id"], "resp_first");
        assert!(requests[2]["body"].get("previous_response_id").is_none());
        assert_eq!(requests[2]["body"]["input"].as_array().unwrap().len(), 3);
        assert_eq!(requests[2]["body"]["store"], true);
    }
}

#[tokio::test]
async fn blocked_chained_prompt_without_previous_id_text_retries_full_history() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"),
            {"status":400,"body":"{\"error\":{\"code\":\"invalid_prompt\",\"message\":\"policy notice\"}}"},
            completed_text("resp_retry", "full retry")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("seed")),
            Message::Assistant(seed),
            Message::User(UserMessage::text("next")),
        ],
        ..Context::default()
    };
    let (_, output) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(output.text(), "full retry");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_seed");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["input"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn chained_tool_result_is_the_only_delta_and_changed_history_breaks_the_chain() {
    let tool_turn = json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{
            "type":"function_call","id":"fc_read","call_id":"call_read","name":"read","arguments":"{}"}}},
        {"data":{"type":"response.completed","response":{"id":"resp_tool","status":"completed"}}}
    ]});
    let server = FakeUpstream::start(
        script(json!({"responses":[tool_turn, completed_text("resp_result", "read complete"),
            completed_text("resp_rewrite", "rewritten")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("read file"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    assert_eq!(first.stop_reason, StopReason::ToolUse);
    let call_id = first.tool_calls().next().unwrap().id.clone();
    let result = Message::ToolResult(ToolResultMessage {
        tool_call_id: call_id,
        tool_name: "read".into(),
        content: vec![UserBlock::text("file body")],
        details: None,
        is_error: false,
        timestamp: 1,
        provider_metadata: None,
        useless: false,
    });
    let (_, second) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context {
            messages: vec![
                Message::User(UserMessage::text("read file")),
                Message::Assistant(first.clone()),
                result.clone(),
            ],
            ..Context::default()
        },
        opts.clone(),
    ))
    .await;
    assert_eq!(second.text(), "read complete");
    let (_, third) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context {
            messages: vec![
                Message::User(UserMessage::text("edited first request")),
                Message::Assistant(first),
                result,
                Message::Assistant(second),
                Message::User(UserMessage::text("continue")),
            ],
            ..Context::default()
        },
        opts,
    ))
    .await;
    assert_eq!(third.text(), "rewritten");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_tool");
    assert_eq!(
        requests[1]["body"]["input"],
        json!([{
            "type":"function_call_output","call_id":"call_read","output":"file body"
        }])
    );
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["input"][0]["content"][0]["text"], "edited first request");
}

#[tokio::test]
async fn stateful_baseline_is_scoped_to_credentials_and_session_state() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_first", "first answer"),
            completed_text("resp_other_key", "key changed"), completed_text("resp_new_state", "new state")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("first"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("first")),
            Message::Assistant(first),
            Message::User(UserMessage::text("next")),
        ],
        ..Context::default()
    };
    opts.api_key = Some("another-test-secret".into());
    let (_, second) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(second.text(), "key changed");
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let mut third_messages = context.messages;
    third_messages.push(Message::Assistant(second));
    third_messages.push(Message::User(UserMessage::text("third")));
    let (_, third) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context { messages: third_messages, ..Context::default() },
        opts,
    ))
    .await;
    assert_eq!(third.text(), "new state");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests.iter().all(|request| request["body"].get("previous_response_id").is_none()));
    assert!(requests.iter().all(|request| request["body"]["store"] == true));
    assert_eq!(requests[2]["body"]["input"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn changed_request_controls_force_full_replay_then_establish_a_new_baseline() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"),
            completed_text("resp_changed", "changed"), completed_text("resp_follow", "follow")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let mut messages = vec![Message::User(UserMessage::text("seed"))];
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: messages.clone(), ..Context::default() },
        opts.clone(),
    ))
    .await;
    messages.push(Message::Assistant(seed));
    messages.push(Message::User(UserMessage::text("next")));
    opts.request.max_tokens = Some(123);
    let (_, changed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: messages.clone(), ..Context::default() },
        opts.clone(),
    ))
    .await;
    assert_eq!(changed.text(), "changed");
    messages.push(Message::Assistant(changed));
    messages.push(Message::User(UserMessage::text("after change")));
    let (_, follow) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context { messages, ..Context::default() },
        opts,
    ))
    .await;
    assert_eq!(follow.text(), "follow");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert!(requests[1]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[1]["body"]["max_output_tokens"], 123);
    assert_eq!(requests[1]["body"]["input"].as_array().unwrap().len(), 3);
    assert_eq!(requests[2]["body"]["previous_response_id"], "resp_changed");
    assert_eq!(requests[2]["body"]["input"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn zero_data_retention_rejection_disables_storage_and_future_chaining() {
    let server = FakeUpstream::start(
        script(json!({"responses":[
            completed_text("resp_first", "first answer"),
            {"status":400,"body":"{\"error\":{\"code\":\"previous_response_not_found\",\"message\":\"Request blocked: previous_response_id unsupported under Zero_Data_Retention\"}}"},
            completed_text("resp_fallback", "fallback answer"),
            completed_text("resp_next", "next answer")
        ]})), None,
    ).await.unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("first"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let messages = vec![
        Message::User(UserMessage::text("first")),
        Message::Assistant(first),
        Message::User(UserMessage::text("second")),
    ];
    let (_, second) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: messages.clone(), ..Context::default() },
        opts.clone(),
    ))
    .await;
    assert_eq!(second.text(), "fallback answer");
    let mut third_messages = messages;
    third_messages.push(Message::Assistant(second));
    third_messages.push(Message::User(UserMessage::text("third")));
    let (_, third) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context { messages: third_messages, ..Context::default() },
        opts,
    ))
    .await;
    assert_eq!(third.text(), "next answer");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_first");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["store"], false);
    assert!(requests[3]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[3]["body"]["store"], false);
}

#[tokio::test]
async fn three_consecutive_stale_ids_disable_chaining_for_the_session() {
    let stale = json!({"status":400,
        "body":"{\"error\":{\"message\":\"previous_response_id expired\"}}"});
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"),
            stale.clone(), completed_text("resp_one", "one"),
            stale.clone(), completed_text("resp_two", "two"),
            stale, completed_text("resp_three", "three"),
            completed_text("resp_after", "after")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let mut messages = vec![Message::User(UserMessage::text("seed"))];
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: messages.clone(), ..Context::default() },
        opts.clone(),
    ))
    .await;
    messages.push(Message::Assistant(seed));
    for (n, expected) in [(1, "one"), (2, "two"), (3, "three")] {
        messages.push(Message::User(UserMessage::text(format!("turn {n}"))));
        let (_, output) = collect(openai_responses::stream(
            reqwest::Client::new(),
            endpoint.clone(),
            Context { messages: messages.clone(), ..Context::default() },
            opts.clone(),
        ))
        .await;
        assert_eq!(output.text(), expected);
        messages.push(Message::Assistant(output));
    }
    messages.push(Message::User(UserMessage::text("after")));
    let (_, after) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context { messages, ..Context::default() },
        opts,
    ))
    .await;
    assert_eq!(after.text(), "after");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 8);
    for index in [1, 3, 5] {
        assert!(requests[index]["body"].get("previous_response_id").is_some());
        assert_eq!(requests[index]["body"]["store"], true);
    }
    for index in [2, 4, 6] {
        assert!(requests[index]["body"].get("previous_response_id").is_none());
    }
    assert!(requests[7]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[7]["body"]["store"], false);
}

#[tokio::test]
async fn in_band_chain_failure_is_not_replayed_and_clears_its_baseline() {
    let server = FakeUpstream::start(
        script(json!({"responses":[
            completed_text("resp_first", "first answer"),
            {"events":[
                {"data":{"type":"response.output_text.delta","output_index":0,"delta":"partial"}},
                {"data":{"type":"response.failed","response":{"status":"failed",
                    "error":{"message":"previous_response_id invalid"}}}}
            ]},
            completed_text("resp_after_error", "after error")
        ]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, first) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("first"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("first")),
            Message::Assistant(first),
            Message::User(UserMessage::text("second")),
        ],
        ..Context::default()
    };
    let (_, failed) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(failed.stop_reason, StopReason::Error);
    let (_, recovered) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(recovered.text(), "after error");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_first");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert_eq!(requests[2]["body"]["store"], true);
}

#[tokio::test]
async fn overlapping_calls_cannot_publish_a_forked_chain_baseline() {
    let slow = json!({"events":[
        {"sleep_ms":300},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message",
            "role":"assistant","content":[{"type":"output_text","text":"slow branch"}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_slow","status":"completed"}}}
    ]});
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"), slow,
            completed_text("resp_fast", "fast branch"), completed_text("resp_follow", "follow up")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let branch_context = Context {
        messages: vec![
            Message::User(UserMessage::text("seed")),
            Message::Assistant(seed),
            Message::User(UserMessage::text("branch")),
        ],
        ..Context::default()
    };
    let slow_task = tokio::spawn({
        let endpoint = endpoint.clone();
        let context = branch_context.clone();
        let opts = opts.clone();
        async move { collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await.1 }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let (_, fast) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        branch_context.clone(),
        opts.clone(),
    ))
    .await;
    assert_eq!(fast.text(), "fast branch");
    assert_eq!(slow_task.await.unwrap().text(), "slow branch");
    let mut follow_messages = branch_context.messages;
    follow_messages.push(Message::Assistant(fast));
    follow_messages.push(Message::User(UserMessage::text("after branches")));
    let (_, follow) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint,
        Context { messages: follow_messages, ..Context::default() },
        opts,
    ))
    .await;
    assert_eq!(follow.text(), "follow up");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_seed");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
    assert!(requests[3]["body"].get("previous_response_id").is_none());
}

#[tokio::test]
async fn cancelling_a_chained_request_clears_the_baseline_without_a_fallback_post() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"),
            {"delay_ms":5000,"events":[{"data":{"type":"response.completed",
                "response":{"id":"resp_late","status":"completed"}}}]},
            completed_text("resp_after", "after cancel")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("seed")),
            Message::Assistant(seed),
            Message::User(UserMessage::text("next")),
        ],
        ..Context::default()
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut cancelled_opts = opts.clone();
    cancelled_opts.cancel = cancel.clone();
    let pending = tokio::spawn({
        let endpoint = endpoint.clone();
        let context = context.clone();
        async move { collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, cancelled_opts)).await.1 }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while server.served() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert_eq!(pending.await.unwrap().stop_reason, StopReason::Aborted);
    let (_, after) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(after.text(), "after cancel");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_seed");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
}

#[tokio::test]
async fn unrelated_http_rejection_does_not_retry_a_chained_request() {
    let server = FakeUpstream::start(
        script(json!({"responses":[completed_text("resp_seed", "seed"),
            {"status":400,"body":"{\"error\":{\"code\":\"schema_invalid\",\"message\":\"invalid tool schema\"}}"},
            completed_text("resp_after", "after error")]})),
        None,
    )
    .await
    .unwrap();
    let endpoint = model(&server.base_url());
    let mut opts = options();
    opts.stateful_responses = true;
    opts.session_state = Some(Arc::new(ProviderSessionState::default()));
    let (_, seed) = collect(openai_responses::stream(
        reqwest::Client::new(),
        endpoint.clone(),
        Context { messages: vec![Message::User(UserMessage::text("seed"))], ..Context::default() },
        opts.clone(),
    ))
    .await;
    let context = Context {
        messages: vec![
            Message::User(UserMessage::text("seed")),
            Message::Assistant(seed),
            Message::User(UserMessage::text("next")),
        ],
        ..Context::default()
    };
    let (_, rejected) =
        collect(openai_responses::stream(reqwest::Client::new(), endpoint.clone(), context.clone(), opts.clone()))
            .await;
    assert_eq!(rejected.stop_reason, StopReason::Error);
    assert_eq!(rejected.error_status, Some(400));
    let (_, after) = collect(openai_responses::stream(reqwest::Client::new(), endpoint, context, opts)).await;
    assert_eq!(after.text(), "after error");
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[1]["body"]["previous_response_id"], "resp_seed");
    assert!(requests[2]["body"].get("previous_response_id").is_none());
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
                ..Default::default()
            },
            Tool {
                name: "safe".into(),
                description: "safe schema".into(),
                parameters: json!({"type":"object","properties":{
                    "value":{"oneOf":[{"type":"string"},{"type":"null"}]},
                    "patternOnly":{"pattern":"^(?!bad$)"}
                }}),
                ..Default::default()
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
async fn wire_postprocess_is_visible_on_responses_http_without_mutating_tool_schema() {
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
    let parameters = json!({"type":"object","properties":{
        "skip":{"anyOf":[{"type":"number","minimum":0},{"type":"null"}],"description":"optional skip"},
        "mode":{"anyOf":[{"const":"a","description":"mode"},{"const":"b","description":"mode"}]},
        "bare":{"enum":[true,false]},
        "literal":{"default":{"anyOf":[{"const":"x"},{"const":"y"}]}}
    },"required":["skip","mode"]});
    let context = Context {
        messages: vec![Message::User(UserMessage::text("inspect tool schema"))],
        tools: Some(vec![
            Tool {
                name: "broken".into(),
                description: "incompatible enum".into(),
                parameters: json!({"type":"object","properties":{"x":{"type":"integer","enum":["bad"]}}}),
                ..Default::default()
            },
            Tool {
                name: "safe".into(),
                description: "safe wire".into(),
                parameters: parameters.clone(),
                ..Default::default()
            },
        ]),
        ..Context::default()
    };
    let mut opts = options();
    opts.request.tool_choice = Some(ToolChoice::Tool("safe".into()));
    let (_, output) =
        collect(openai_responses::stream(reqwest::Client::new(), model(&server.base_url()), context.clone(), opts))
            .await;
    assert_eq!(output.text(), "Schema received.");
    assert_eq!(context.tools.as_ref().unwrap()[1].parameters, parameters);
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 1);
    let body = &requests[0]["body"];
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["name"], "safe");
    assert_eq!(body["tool_choice"], json!({"type":"function","name":"safe"}));
    let wire = &body["tools"][0]["parameters"];
    assert_eq!(
        wire["properties"]["skip"],
        json!({
            "type":["number","null"],"minimum":0,"description":"optional skip"
        })
    );
    assert_eq!(wire["properties"]["mode"], json!({"type":"string","enum":["a","b"],"description":"mode"}));
    assert_eq!(wire["properties"]["bare"], json!({"type":"boolean","enum":[true,false]}));
    assert_eq!(wire["properties"]["literal"], parameters["properties"]["literal"]);
    assert_eq!(wire["required"], parameters["required"]);
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
            ..Default::default()
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
                provider_metadata: None,
                useless: false,
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
async fn eof_after_unfilled_tool_start_retries_without_exposing_the_failed_call() {
    // OMP b91db66e: the unfilled start is still buffered; argument content,
    // including the literal "{}", is what makes the attempt irrevocable.
    let server = FakeUpstream::start(script(json!({"responses":[
        {"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read"}}}
        ]},
        completed_text("recovered", "recovered answer")
    ]})), None).await.unwrap();
    let (events, output) = collect(openai_responses::stream(
        reqwest::Client::new(),
        model(&server.base_url()),
        Context::default(),
        options(),
    ))
    .await;
    assert_eq!(output.stop_reason, StopReason::Stop);
    assert_eq!(output.text(), "recovered answer");
    assert_eq!(output.tool_calls().count(), 0);
    assert_eq!(server.served(), 2);
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::Start { .. })).count(), 1);
    assert!(!events.iter().any(|event| matches!(event,
        AssistantMessageEvent::ToolcallStart { .. }
        | AssistantMessageEvent::ToolcallDelta { .. }
        | AssistantMessageEvent::ToolcallEnd { .. }
    )));
    assert!(matches!(events.last(), Some(AssistantMessageEvent::Done { reason: StopReason::Stop, .. })));
    let attempts = &output.retry_accounting.as_ref().expect("both native attempts are recorded").attempts;
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].usage.input, None);
    assert_eq!(attempts[0].stop_reason, StopReason::Error);
    assert_eq!(attempts[1].stop_reason, StopReason::Stop);
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
    let attempts = &output.retry_accounting.as_ref().unwrap().attempts;
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].usage.input, None);
    assert_eq!(attempts[0].stop_reason, StopReason::Error);
    assert_eq!(attempts[1].stop_reason, StopReason::Stop);
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

#[tokio::test]
async fn computer_include_and_lite_per_call_override_family() {
    use ara_ai::{CallOptions, ModelProvider};
    for (include, lite) in [
        (None, None),
        (Some(Vec::new()), Some(false)),
        (Some(vec!["computer_call_output.output.image_url".into()]), Some(true)),
    ] {
        let endpoint=FakeUpstream::start(script(json!({"responses":[
            {"events":[{"data":{"type":"response.output_item.done","output_index":0,"item":{
                "type":"message","id":"msg_override","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}},
                {"data":{"type":"response.completed","response":{"status":"completed"}}}]}
        ]})),None).await.unwrap();
        let mut base = options();
        base.request.include = vec!["inherited-only".into()];
        base.request.responses_lite = true;
        let provider = ara_ai::OpenAIResponsesProvider { client: reqwest::Client::new(), base };
        let (_, out) = collect(provider.stream(
            &model(&endpoint.base_url()),
            &Context { messages: vec![Message::User(UserMessage::text("call override"))], ..Default::default() },
            CallOptions { include: include.clone(), responses_lite: lite, ..Default::default() },
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let body = &recorded[0]["body"];
        let includes = body.get("include").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(includes.contains(&json!("inherited-only")), include.is_none());
        assert_eq!(
            includes.contains(&json!("computer_call_output.output.image_url")),
            include.as_ref().is_some_and(|items| !items.is_empty())
        );
        assert!(body.get("tools").is_none());
        assert!(recorded[0]["headers"].get("x-openai-internal-codex-responses-lite").is_none());
        assert_ne!(body["input"][0]["type"], "additional_tools");
    }
}


#[tokio::test]
async fn responses_retry_unfilled_tool_wire_boundary_group() {
    use ara_ai::ActualAssistantEvent;
    use ara_ai::graph_values::GraphValueFactory;
    use ara_ai::providers::openai_responses::actual::{GraphResponsesOptions, stream_prepared};
    use ara_rpc::value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, NoRuntime, PropertyDescriptor, PropertyKey, ValueHeap, ValueSnapshot,
    };

    // The reference NativeRealm installs this Host primitive on Array.prototype.
    // A bare ValueHeap includes push/iteration but intentionally omits indexOf.
    // Reuse its public operation owner, preserving the provider's original Get,
    // receiver and argument identities; no Provider-local index approximation.
    static ARRAY_INDEX_OF: CompiledFunctionSpec = CompiledFunctionSpec {
        source: "Responses fixture binding to existing Native Array.prototype.indexOf owner",
        name: "indexOf",
        length: 1,
        form: CompiledFunctionForm::Builtin,
        capture_count: 0,
        body: |heap, _, receiver, arguments, runtime| heap.array_index_of_native(receiver, arguments, runtime),
    };

    // Keep the migrated EOF fixture and all original response banks unchanged.
    // This one coarse wire group exercises the changed typed parser plus the
    // existing public actual transport binding of the same native retry owner.
    // It does not stand for full native request preparation/Provider parity.
    for actual in [false, true] {
        for (label, item_kind, delta, should_retry) in [
            ("function-start", "function_call", None, true),
            ("custom-start", "custom_tool_call", None, true),
            ("computer-start", "computer_call", None, true),
            ("function-empty-delta", "function_call", Some(""), true),
            ("custom-empty-delta", "custom_tool_call", Some(""), true),
            (
                "function-nonempty-delta",
                "function_call",
                Some("{}"),
                false,
            ),
            (
                "custom-nonempty-delta",
                "custom_tool_call",
                Some("echo accepted"),
                false,
            ),
        ] {
            let mut item =
                json!({"type":item_kind,"id":"item_failed","call_id":"call_failed","name":"read"});
            if item_kind == "computer_call" {
                item = json!({"type":"computer_call","id":"item_failed","call_id":"call_failed",
                    "actions":[{"type":"screenshot"}],"pending_safety_checks":[],"status":"in_progress"});
            }
            let mut failed = vec![json!({"data":{
                "type":"response.output_item.added","output_index":0,"item":item
            }})];
            if let Some(delta) = delta {
                failed.push(json!({"data":{
                    "type":if item_kind == "custom_tool_call" {
                        "response.custom_tool_call_input.delta"
                    } else {
                        "response.function_call_arguments.delta"
                    },"output_index":0,"item_id":"item_failed","delta":delta
                }}));
            }
            let server = FakeUpstream::start(
                script(json!({"responses":[
                    {"events":failed}, completed_text("recovered_boundary", "recovered boundary")
                ]})),
                None,
            )
            .await
            .unwrap();
            let mut opts = options();
            opts.retry.max_attempts = 1;
            if !actual {
                let (events, output) = collect(openai_responses::stream(
                    reqwest::Client::new(),
                    model(&server.base_url()),
                    Context::default(),
                    opts,
                ))
                .await;
                assert_eq!(
                    output.stop_reason,
                    if should_retry {
                        StopReason::Stop
                    } else {
                        StopReason::Error
                    },
                    "{label}"
                );
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, AssistantMessageEvent::Start { .. }))
                        .count(),
                    1
                );
                if should_retry {
                    assert_eq!(output.text(), "recovered boundary", "{label}");
                    assert_eq!(output.tool_calls().count(), 0);
                    assert!(
                        !events.iter().any(|event| matches!(
                            event,
                            AssistantMessageEvent::ToolcallStart { .. }
                                | AssistantMessageEvent::ToolcallDelta { .. }
                                | AssistantMessageEvent::ToolcallEnd { .. }
                        )),
                        "discard failed lifecycle: {label}"
                    );
                    let attempts = &output
                        .retry_accounting
                        .as_ref()
                        .expect("both attempts are recorded")
                        .attempts;
                    assert_eq!(attempts.len(), 2);
                    assert_eq!(attempts[0].usage.input, None);
                    assert_eq!(attempts[0].stop_reason, StopReason::Error);
                    assert_eq!(attempts[1].stop_reason, StopReason::Stop);
                } else {
                    assert_eq!(output.tool_calls().count(), 1);
                    assert_eq!(
                        events
                            .iter()
                            .filter(|event| matches!(
                                event,
                                AssistantMessageEvent::ToolcallStart { .. }
                            ))
                            .count(),
                        1
                    );
                    assert!(events.iter().any(|event| matches!(event,
                        AssistantMessageEvent::ToolcallDelta { delta: observed, .. } if Some(observed.as_str()) == delta
                    )));
                    assert!(matches!(
                        events.last(),
                        Some(AssistantMessageEvent::Error {
                            reason: StopReason::Error,
                            ..
                        })
                    ));
                }
            } else {
                let heap = ValueHeap::new().unwrap();
                let factory =
                    Arc::new(GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
                let index_of = heap.fresh_compiled_function(&ARRAY_INDEX_OF, &[]).unwrap();
                heap.define_own(
                    &heap.array_prototype(),
                    "indexOf".into(),
                    PropertyDescriptor::Data {
                        value: index_of,
                        writable: true,
                        enumerable: false,
                        configurable: true,
                    },
                    factory.runtime(),
                )
                .unwrap();
                let request_body = heap
                    .import_strict(
                        ara_rpc::WireValue::parse(
                            r#"{"model":"fake-responses-model","input":[],"stream":true}"#,
                        )
                        .unwrap(),
                    )
                    .unwrap();
                let mut stream = stream_prepared(
                    reqwest::Client::new(),
                    model(&server.base_url()),
                    request_body,
                    factory.clone(),
                    opts,
                    GraphResponsesOptions::default(),
                )
                .unwrap();
                let receipt = stream.failure_receipt();
                let events = tokio::time::timeout(Duration::from_secs(5), async {
                    let mut events = Vec::new();
                    while let Some(event) = stream.recv().await {
                        events.push(event);
                    }
                    events
                })
                .await
                .expect("actual Responses boundary drain");
                drop(stream);
                let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled())
                    .await
                    .expect("actual Responses boundary settlement");
                assert!(failures.is_empty(), "{label}: {failures:?}");
                assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
                assert_eq!(
                    events
                        .iter()
                        .filter(|event| matches!(event, ActualAssistantEvent::Start { .. }))
                        .count(),
                    1
                );
                let terminal = events.last().expect("actual terminal");
                assert!(
                    if should_retry {
                        matches!(
                            terminal,
                            ActualAssistantEvent::Done {
                                reason: StopReason::Stop,
                                ..
                            }
                        )
                    } else {
                        matches!(
                            terminal,
                            ActualAssistantEvent::Error {
                                reason: StopReason::Error,
                                ..
                            }
                        )
                    },
                    "{label}"
                );
                let root = terminal.partial();
                let get = |root: &ara_rpc::value_heap::ValueHandle, name: &str| {
                    heap.get(root, &PropertyKey::from(name), factory.runtime())
                        .unwrap()
                };
                let content = get(root, "content");
                assert!(matches!(
                    heap.snapshot(&get(&content, "length")).unwrap(),
                    ValueSnapshot::Number(1.0)
                ));
                let block = get(&content, "0");
                assert!(
                    matches!(heap.snapshot(&get(&block, "type")).unwrap(), ValueSnapshot::String(kind)
                    if kind.equals_ascii(if should_retry { "text" } else { "toolCall" })),
                    "{label}"
                );
                if should_retry {
                    assert!(
                        matches!(heap.snapshot(&get(&block, "text")).unwrap(), ValueSnapshot::String(text)
                        if text.equals_ascii("recovered boundary"))
                    );
                    assert!(!events.iter().any(|event| matches!(
                        event,
                        ActualAssistantEvent::ToolcallStart { .. }
                            | ActualAssistantEvent::ToolcallDelta { .. }
                            | ActualAssistantEvent::ToolcallEnd { .. }
                    )));
                    let attempts = get(&get(root, "retryAccounting"), "attempts");
                    assert!(matches!(
                        heap.snapshot(&get(&attempts, "length")).unwrap(),
                        ValueSnapshot::Number(2.0)
                    ));
                } else {
                    assert_eq!(
                        events
                            .iter()
                            .filter(|event| matches!(
                                event,
                                ActualAssistantEvent::ToolcallStart { .. }
                            ))
                            .count(),
                        1
                    );
                    assert!(events.iter().any(|event| match event {
                        ActualAssistantEvent::ToolcallDelta { delta: observed, .. } => {
                            matches!(heap.snapshot(&observed).unwrap(), ValueSnapshot::String(observed)
                                if delta.is_some_and(|expected| observed.equals_ascii(expected)))
                        }
                        _ => false,
                    }));
                }
            }
            assert_eq!(
                server.served(),
                if should_retry { 2 } else { 1 },
                "actual={actual}: {label}"
            );
            let requests = server.requests.lock().await;
            assert_eq!(requests.len(), if should_retry { 2 } else { 1 });
            if should_retry {
                assert_eq!(
                    requests[0]["body"], requests[1]["body"],
                    "same complete body: {label}"
                );
                assert_eq!(
                    requests[0]["headers"], requests[1]["headers"],
                    "same complete headers: {label}"
                );
            }
            eprintln!("ARA-RETRY RESPONSES CLOSURE PASS actual={actual} {label}");
        }
    }

    // Codex is a distinct caller of the shared typed parser. Its fixed retry
    // owner vetoes any visible tool block, including an unfilled start.
    // Exercise that ResponsesProtocol policy with a valid Codex model fixture.
    for item_kind in ["function_call", "custom_tool_call", "computer_call"] {
        let item = if item_kind == "computer_call" {
            json!({"type":"computer_call","id":"codex_item","call_id":"codex_call",
                "actions":[{"type":"screenshot"}],"pending_safety_checks":[],"status":"in_progress"})
        } else {
            json!({"type":item_kind,"id":"codex_item","call_id":"codex_call","name":"read"})
        };
        let server = FakeUpstream::start(script(json!({"responses":[
            {"events":[{"data":{"type":"response.output_item.added","output_index":0,"item":item}}]},
            completed_text("must_not_retry_codex", "wrong Codex replay")
        ]})), None).await.unwrap();
        let mut control_model = model(&server.base_url());
        control_model.api = ara_ai::providers::openai_codex_responses::API.into();
        let opts = ara_ai::providers::openai_codex_responses::StreamOptions {
            api_key: Some("fixture-codex-token".into()),
            extra_headers: vec![(
                ara_ai::providers::openai_codex_responses::ACCOUNT_HEADER.into(),
                "fixture-account".into(),
            )],
            retry: options().retry,
            first_event_timeout: Some(Duration::from_secs(2)),
            idle_timeout: Some(Duration::from_secs(2)),
            ..Default::default()
        };
        ara_ai::providers::openai_codex_responses::build_request(
            &control_model,
            &Context::default(),
            &opts.request,
        )
        .expect("valid Codex fixture request");
        let (events, output) = collect(ara_ai::providers::openai_codex_responses::stream(
            reqwest::Client::new(),
            control_model,
            Context::default(),
            opts,
        ))
        .await;
        assert_eq!(output.stop_reason, StopReason::Error, "Codex {item_kind}");
        assert_eq!(
            server.served(),
            1,
            "Codex must preserve its tool-block veto: {item_kind}"
        );
        assert_eq!(output.tool_calls().count(), 1);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AssistantMessageEvent::ToolcallStart { .. }))
                .count(),
            1
        );
        assert!(matches!(
            events.last(),
            Some(AssistantMessageEvent::Error {
                reason: StopReason::Error,
                ..
            })
        ));
        assert!(
            output
                .failure_evidence
                .as_ref()
                .expect("failure facts")
                .replay_blocked
        );
        eprintln!("ARA-RETRY RESPONSES CLOSURE PASS codex-protocol {item_kind}");
    }
}
