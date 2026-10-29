//! Coarse request-history fixtures; these do not prove HTTP/Host acceptance.
use ara_ai::providers::openai_completions::{StreamOptions, actual::build_request};
use ara_ai::{GraphProviderContext, Model, dialect::Dialect, graph_values::GraphValueFactory};
use ara_rpc::{
    WireValue,
    value_heap::{NoRuntime, PropertyKey, ValueHandle, ValueHeap, ValueRuntime},
};
use serde_json::{Value, json};
use std::sync::Arc;

fn setup(messages: Vec<Value>) -> (Model, GraphProviderContext, Arc<GraphValueFactory>) {
    setup_with_runtime(messages, Arc::new(NoRuntime))
}
fn setup_with_runtime(
    messages: Vec<Value>,
    runtime: Arc<dyn ValueRuntime>,
) -> (Model, GraphProviderContext, Arc<GraphValueFactory>) {
    let model = Model {
        id: "m".into(),
        api: "openai-completions".into(),
        provider: "custom".into(),
        base_url: "http://127.0.0.1:1/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let heap = ValueHeap::new().unwrap();
    let factory = Arc::new(GraphValueFactory::new(heap, runtime).unwrap());
    let import = |value: Value| -> ValueHandle {
        factory.heap().import_strict(WireValue::parse(&value.to_string()).unwrap()).unwrap()
    };
    // These are the original Native Context containers in one heap. The
    // legacy observations stay empty so they cannot supply request history.
    let original_context = factory.heap().object().unwrap();
    for (key, value) in [
        ("systemPrompt", import(json!([]))),
        ("messages", import(Value::Array(messages))),
        ("tools", factory.heap().undefined().unwrap()),
    ] {
        factory.heap().define_own_data(&original_context, PropertyKey::from(key), value, factory.runtime()).unwrap();
    }
    let context = GraphProviderContext {
        original_context: Some(original_context),
        system_prompt: Vec::new(),
        messages: Vec::new(),
        tools: None,
        history_demotion_dialect: Some(Dialect::Deepseek),
        history_id_hash: None,
        history_id_hash_function: None,
    };
    (model, context, factory)
}
fn assistant(content: Value, stop: &str) -> Value {
    json!({"role":"assistant","content":content,"provider":"custom","api":"openai-completions","model":"m","stopReason":stop,"timestamp":1})
}
fn call(id: &str, name: &str) -> Value {
    json!({"type":"toolCall","id":id,"name":name,"arguments":{"x":1}})
}
fn result(id: &str, value: &str) -> Value {
    json!({"role":"toolResult","toolCallId":id,"toolName":"read","content":[{"type":"text","text":value}],"isError":false,"timestamp":2})
}
fn wire(
    model: &Model,
    context: &GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
) -> Value {
    serde_json::from_slice(&build_request(model, context, &options, factory).unwrap()).unwrap()
}

fn occurrence_repair_delayed_aborted_duplicates_and_orphans_share_one_history() {
    let (model, context, factory) = setup(vec![
        assistant(json!([call("same", ""), call("same", "read")]), "toolUse"),
        result("same", "malformed rejection"),
        result("same", "first"),
        assistant(json!([call("same", "read")]), "toolUse"),
        json!({"role":"user","content":"boundary","timestamp":3}),
        result("same", "delayed second"),
        assistant(json!([call("missing", "read")]), "aborted"),
        json!({"role":"user","content":"next","timestamp":4}),
        result("orphan", "stale output"),
        assistant(
            json!([{"type":"thinking","thinking":"unfinished","thinkingSignature":"reasoning_content"}]),
            "length",
        ),
    ]);
    let encoded = wire(&model, &context, factory, StreamOptions::default());
    let messages = encoded["messages"].as_array().unwrap();
    assert_eq!(messages[0]["tool_calls"][0]["id"], "same");
    assert_eq!(messages[1]["content"], "first");
    assert_eq!(messages[2]["tool_calls"][0]["id"], "same_dup1");
    assert_eq!(messages[3]["tool_call_id"], "same_dup1");
    assert_eq!(messages[3]["content"], "delayed second");
    assert_eq!(messages[4]["content"], "boundary");
    assert_eq!(messages[5]["tool_calls"][0]["id"], "missing");
    assert_eq!(messages[6]["content"], "aborted");
    assert_eq!(messages[7]["content"], "next");
    assert_eq!(messages[8]["role"], "user");
    assert!(messages[8]["content"].as_str().unwrap().contains("stale output"));
    assert_eq!(messages.len(), 9);
}

fn cross_provider_components_demote_reasoning_and_group_result_images() {
    let mut foreign = assistant(
        json!([
            {"type":"thinking","thinking":"prior","thinkingSignature":"foreign-signature"},
            {"type":"text","text":"answer"}, call("call_A|fc_origin", "read"),call("call_B|fc_other", "read")
        ]),
        "toolUse",
    );
    foreign["provider"] = json!("openai");
    foreign["api"] = json!("openai-responses");
    foreign["model"] = json!("other");
    let mut first = result("call_A|fc_result", "A");
    first["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"image","url":"https://example.invalid/a.png","detail":"original"}));
    let mut second = result("call_B|fc_result", "B");
    second["content"].as_array_mut().unwrap().push(json!({"type":"image","mimeType":"image/png","data":"AA=="}));
    let (model, context, factory) = setup(vec![foreign, first, second]);
    let encoded = wire(&model, &context, factory, StreamOptions::default());
    let messages = encoded["messages"].as_array().unwrap();
    assert_eq!(messages[0]["content"], "<think>\nprior\n</think>\nanswer");
    assert_eq!(messages[0]["tool_calls"][0]["id"], "call_A");
    assert_eq!(messages[1]["tool_call_id"], "call_A");
    assert_eq!(messages[2]["tool_call_id"], "call_B");
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(messages[3]["content"].as_array().unwrap().len(), 3);
    assert_eq!(messages[3]["content"][1]["image_url"]["url"], "https://example.invalid/a.png");
    assert_eq!(messages[3]["content"][2]["image_url"]["url"], "data:image/png;base64,AA==");
    assert_eq!(messages.len(), 4);
}

fn exact_reasoning_replay_keeps_empty_capture_and_opaque_pipe_ids() {
    let (mut model, context, factory) = setup(vec![
        assistant(
            json!([{"type":"thinking","thinking":"","thinkingSignature":"reasoning_text"},{"type":"text","text":"visible"}]),
            "stop",
        ),
        assistant(json!([call("call_A|first", "read"), call("call_A|second", "read")]), "toolUse"),
        result("call_A|first", "one"),
        result("call_A|second", "two"),
    ]);
    model.reasoning = true;
    let prompts = factory.heap().import_strict(WireValue::parse(r#"["first","second"]"#).unwrap()).unwrap();
    factory
        .heap()
        .define_own_data(
            context.original_context.as_ref().unwrap(),
            PropertyKey::from("systemPrompt"),
            prompts,
            factory.runtime(),
        )
        .unwrap();
    assert!(context.system_prompt.is_empty());
    let mut options = StreamOptions::default();
    options.compat.requires_reasoning_content_on_all_assistant_turns = true;
    options.compat.requires_reasoning_content_for_tool_calls = true;
    options.compat.allows_synthetic_reasoning_content_for_tool_calls = false;
    let encoded = wire(&model, &context, factory, options);
    assert_eq!(encoded["messages"][0]["content"], "first\n\nsecond");
    assert_eq!(encoded["messages"][1]["reasoning_content"], "");
    assert_eq!(encoded["messages"][2]["tool_calls"][0]["id"], "call_A|first");
    assert_eq!(encoded["messages"][2]["tool_calls"][1]["id"], "call_A|second");
    assert_eq!(encoded["messages"][3]["tool_call_id"], "call_A|first");
    assert_eq!(encoded["messages"][4]["tool_call_id"], "call_A|second");
}

fn fallback_id_uses_one_neutral_hash_owner_and_original_source_seed() {
    use ara_rpc::{
        WireString,
        value_heap::{CapabilityId, RuntimeReply, RuntimeRequest, ValueError, ValueSnapshot},
    };
    use std::sync::Mutex;
    #[derive(Debug, Default)]
    struct RecordingHash(Mutex<Vec<WireString>>);
    impl ValueRuntime for RecordingHash {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(7301), receiver, arguments } => {
                    assert_eq!(arguments.len(), 1);
                    let heap = arguments[0].heap();
                    assert!(matches!(heap.snapshot(receiver)?, ValueSnapshot::Undefined));
                    let ValueSnapshot::String(seed) = heap.snapshot(&arguments[0])? else {
                        panic!("Original hash Function must receive its exact string seed");
                    };
                    self.0.lock().unwrap().push(seed);
                    // Exact BigInt whose actual Native toString(36) is
                    // fixturehash; the hash Function never returns a string.
                    heap.bigint_decimal("56765891969059553").map(RuntimeReply::Value)
                }
                other => Err(ValueError::MissingCapability(other.capability())),
            }
        }
    }
    let mut foreign = assistant(json!([call("|fc_origin", "read")]), "toolUse");
    foreign["provider"] = json!("openai");
    foreign["api"] = json!("openai-responses");
    foreign["model"] = json!("other");
    let hasher = Arc::new(RecordingHash::default());
    let (model, mut context, factory) = setup_with_runtime(vec![foreign], hasher.clone());
    let mut unavailable = context.clone();
    unavailable.original_context = None;
    assert!(matches!(
        build_request(&model, &unavailable, &StreamOptions::default(), factory.clone()),
        Err(ValueError::UnsupportedOperation("Original Native provider Context is not connected"))
    ));
    assert!(build_request(&model, &context, &StreamOptions::default(), factory.clone()).is_err());
    assert!(hasher.0.lock().unwrap().is_empty());
    // The retained callback captures only this same-heap original Function
    // edge. Its invocation resolves through the consumer's current runtime.
    context.history_id_hash_function = Some(factory.heap().function(CapabilityId(7301)).unwrap());
    assert!(context.history_id_hash.is_none());
    let encoded = wire(&model, &context, factory, StreamOptions::default());
    assert_eq!(encoded["messages"][0]["tool_calls"][0]["id"], "call_fixturehash");
    assert_eq!(encoded["messages"][1]["tool_call_id"], "call_fixturehash");
    assert_eq!(&*hasher.0.lock().unwrap(), &[WireString::from("custom:m:0:0:read:1")]);
}

fn set_original(factory: &GraphValueFactory, context: &GraphProviderContext, key: &str, value: Value) {
    let imported = factory.heap().import_strict(WireValue::parse(&value.to_string()).unwrap()).unwrap();
    factory
        .heap()
        .define_own_data(context.original_context.as_ref().unwrap(), key.into(), imported, factory.runtime())
        .unwrap();
}

fn policy_request(
    choice: ara_ai::ToolChoice,
    compat_patch: Value,
) -> (Model, GraphProviderContext, Arc<GraphValueFactory>, StreamOptions) {
    use ara_ai::{OpenAIReasoningPolicy, ReasoningEffort, ReasoningRequest};
    let (mut model, mut context, factory) = setup(vec![
        assistant(json!([{"type":"thinking","thinking":"earlier"},{"type":"text","text":"answer"}]), "stop"),
        json!({"role":"user","content":"continue","timestamp":3}),
    ]);
    model.reasoning = true;
    context.history_demotion_dialect = Some(Dialect::Xml);
    set_original(&factory, &context, "systemPrompt", json!(["be brief"]));
    set_original(
        &factory,
        &context,
        "tools",
        json!([
            {"name":"read","description":"read","parameters":{"type":"object"}},
            {"name":"write","description":"write","parameters":{"type":"object"}}
        ]),
    );
    let mut compat = json!({"disableReasoningOnToolChoice":true,"reasoningDisableMode":"none-effort",
        "whenThinking":{"supportsDeveloperRole":true,"requiresThinkingAsText":true,
            "maxTokensField":"max_completion_tokens","supportsUsageInStreaming":false,
            "reasoningEffortMap":{"high":"enabled-high"}}});
    compat.as_object_mut().unwrap().extend(compat_patch.as_object().unwrap().clone());
    let options = StreamOptions {
        tool_choice: Some(choice),
        max_tokens: Some(64),
        reasoning: Some(ReasoningRequest { effort: Some(ReasoningEffort::High), ..Default::default() }),
        reasoning_policy: Some(
            OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,
            "thinking":{"mode":"effort","efforts":["high"]},"compat":compat}))
            .unwrap(),
        ),
        ..Default::default()
    };
    (model, context, factory, options)
}

fn assert_final_policy(body: &Value, choice: Option<&Value>, thinking: bool, tool_count: usize) {
    assert_eq!(body.get("tool_choice"), choice);
    assert_eq!(body["tools"].as_array().unwrap().len(), tool_count);
    assert_eq!(body["reasoning_effort"], if thinking { "enabled-high" } else { "none" });
    assert_eq!(body["messages"][0]["role"], if thinking { "developer" } else { "system" });
    assert_eq!(
        body["messages"][1]["content"],
        if thinking { "<thinking>\nearlier\n</thinking> answer" } else { "answer" }
    );
    assert_eq!(body[if thinking { "max_completion_tokens" } else { "max_tokens" }], 64);
    assert!(body.get(if thinking { "max_tokens" } else { "max_completion_tokens" }).is_none());
}

pub fn observe_all_native_choices() {
    use ara_ai::ToolChoice;
    for (choice, expected) in [
        (ToolChoice::Auto, Some(json!("auto"))),
        (ToolChoice::None, Some(json!("none"))),
        (ToolChoice::Any, Some(json!("required"))),
        (ToolChoice::Required, Some(json!("required"))),
        (ToolChoice::Function("read".into()), Some(json!({"type":"function","function":{"name":"read"}}))),
        (ToolChoice::FunctionNested("read".into()), Some(json!({"type":"function","function":{"name":"read"}}))),
        (ToolChoice::Tool("read".into()), Some(json!({"type":"function","function":{"name":"read"}}))),
        (ToolChoice::Computer, None),
    ] {
        let (model, context, factory) = setup(vec![]);
        set_original(
            &factory,
            &context,
            "tools",
            json!([
                {"name":"read","description":"read","parameters":{"type":"object"}}
            ]),
        );
        let body = wire(&model, &context, factory, StreamOptions { tool_choice: Some(choice), ..Default::default() });
        assert_eq!(body.get("tool_choice"), expected.as_ref());
        assert_eq!(body["tools"][0]["function"]["name"], "read");
    }
}

fn requested_choice_preparation_and_final_history_share_one_native_request() {
    use ara_ai::ToolChoice;
    for (choice, patch, expected, thinking, count) in [
        (ToolChoice::Auto, json!({}), None, true, 2),
        (ToolChoice::Required, json!({}), Some(json!("required")), false, 2),
        (ToolChoice::None, json!({}), Some(json!("none")), false, 2),
        (ToolChoice::Tool("read".into()), json!({"supportsNamedToolChoice":false}), Some(json!("required")), false, 1),
        (ToolChoice::Tool("missing".into()), json!({"supportsNamedToolChoice":false}), None, true, 2),
        (ToolChoice::Tool("read".into()), json!({"supportsForcedToolChoice":false}), None, true, 2),
        (ToolChoice::Tool("read".into()), json!({"supportsToolChoice":false}), None, true, 2),
    ] {
        let (model, context, factory, options) = policy_request(choice, patch);
        let body = wire(&model, &context, factory, options);
        assert_final_policy(&body, expected.as_ref(), thinking, count);
        // Requested choice selects base usage controls, even when final
        // surviving-choice policy selects whenThinking history/max tokens.
        assert_eq!(body["stream_options"]["include_usage"], true);
    }
    // Base named support differs from the initial requested-choice variant.
    // This catches preparing Native tools with typed/base policy flags.
    let (model, context, factory, options) = policy_request(
        ara_ai::ToolChoice::Tool("read".into()),
        json!({
            "disableReasoningOnToolChoice":false,
            "whenThinking":{"supportsNamedToolChoice":false,"supportsDeveloperRole":true,
                "requiresThinkingAsText":true,"maxTokensField":"max_completion_tokens",
                "supportsUsageInStreaming":false,"reasoningEffortMap":{"high":"enabled-high"}}
        }),
    );
    let body = wire(&model, &context, factory, options);
    assert_final_policy(&body, Some(&json!("required")), true, 1);
    assert!(body.get("stream_options").is_none());
    assert_eq!(body["tools"][0]["function"]["name"], "read");
}

fn empty_tools_and_history_keep_fixed_choice_guards() {
    use ara_ai::ToolChoice;
    for explicit in [false, true] {
        for choice in [ToolChoice::Auto, ToolChoice::None, ToolChoice::Required, ToolChoice::Tool("read".into())] {
            let (model, context, factory) =
                setup(vec![assistant(json!([call("call-1", "read")]), "toolUse"), result("call-1", "ok")]);
            if explicit {
                set_original(&factory, &context, "tools", json!([]));
            }
            let options = StreamOptions { tool_choice: Some(choice.clone()), ..Default::default() };
            let body = wire(&model, &context, factory, options);
            if explicit {
                assert!(body.get("tools").is_none());
            } else {
                assert_eq!(body["tools"], json!([]));
            }
            let expected = (choice == ToolChoice::Auto).then(|| json!("auto"));
            assert_eq!(body.get("tool_choice"), expected.as_ref());
            assert_eq!(body["messages"][1]["tool_call_id"], "call-1");
        }
    }
    let (model, context, factory) = setup(vec![]);
    let body = wire(
        &model,
        &context,
        factory,
        StreamOptions { tool_choice: Some(ToolChoice::Required), ..Default::default() },
    );
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

fn original_tools_getters_and_schema_keep_their_native_receiver() {
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
    };
    use std::sync::Mutex;
    #[derive(Debug, Default)]
    struct OriginalTools {
        arrays: Mutex<Vec<ValueHandle>>,
        context: Mutex<Option<ValueHandle>>,
        schema: Mutex<Option<ValueHandle>>,
        trace: Mutex<Vec<&'static str>>,
        throw_second: bool,
    }
    impl ValueRuntime for OriginalTools {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability: CapabilityId(7601), receiver, arguments } => {
                    assert!(arguments.is_empty());
                    let mut trace = self.trace.lock().unwrap();
                    let index = trace.iter().filter(|entry| **entry == "tools").count();
                    trace.push("tools");
                    if self.throw_second && index == 1 {
                        return Err(ValueError::TypeError("original tools getter failed".into()));
                    }
                    let arrays = self.arrays.lock().unwrap();
                    assert!(receiver.same_identity(self.context.lock().unwrap().as_ref().unwrap()));
                    Ok(RuntimeReply::Value(arrays[index].clone()))
                }
                RuntimeRequest::Call { capability: CapabilityId(7602), receiver, arguments } => {
                    assert!(arguments.is_empty());
                    assert!(receiver.same_identity(self.schema.lock().unwrap().as_ref().unwrap()));
                    self.trace.lock().unwrap().push("schema");
                    Ok(RuntimeReply::Value(receiver.heap().string("object".into())?))
                }
                other => Err(ValueError::MissingCapability(other.capability())),
            }
        }
    }
    for throw_second in [false, true] {
        let runtime = Arc::new(OriginalTools { throw_second, ..Default::default() });
        let (model, context, factory) = setup_with_runtime(vec![], runtime.clone());
        *runtime.context.lock().unwrap() = context.original_context.clone();
        let import =
            |value: Value| factory.heap().import_strict(WireValue::parse(&value.to_string()).unwrap()).unwrap();
        let tools = import(json!([{"name":"read","description":"read","parameters":{"type":"object"}}]));
        let tool = factory.heap().get(&tools, &"0".into(), factory.runtime()).unwrap();
        let schema = factory.heap().get(&tool, &"parameters".into(), factory.runtime()).unwrap();
        *runtime.schema.lock().unwrap() = Some(schema.clone());
        factory
            .heap()
            .define_own(
                &schema,
                "type".into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(7602))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                factory.runtime(),
            )
            .unwrap();
        *runtime.arrays.lock().unwrap() = vec![import(json!([{"name":"wrong-first-array"}])), tools];
        factory
            .heap()
            .define_own(
                context.original_context.as_ref().unwrap(),
                "tools".into(),
                PropertyDescriptor::Accessor {
                    get: Some(CallableTarget::Host(CapabilityId(7601))),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                factory.runtime(),
            )
            .unwrap();
        let options =
            StreamOptions { tool_choice: Some(ara_ai::ToolChoice::Tool("read".into())), ..Default::default() };
        let request = build_request(&model, &context, &options, factory);
        if throw_second {
            assert!(
                matches!(request, Err(ValueError::TypeError(message)) if message == "original tools getter failed")
            );
            assert_eq!(&*runtime.trace.lock().unwrap(), &["tools", "tools"]);
        } else {
            let body: Value = serde_json::from_slice(&request.unwrap()).unwrap();
            assert_eq!(body["tools"][0]["function"]["name"], "read");
            assert_eq!(body["tools"][0]["function"]["parameters"]["type"], "object");
            let trace = runtime.trace.lock().unwrap();
            assert_eq!(&trace[..2], &["tools", "tools"]);
            assert!(trace[2..].contains(&"schema"));
        }
    }
}

/// Native producer HTTP observations, called by the existing policy coarse
/// group. Fixture traffic only; this is not a real-model/Host acceptance.
pub async fn observe_request_policy_http() {
    use ara_ai::{ActualAssistantEvent, ToolChoice};
    use std::time::Duration;
    for (choice, expected, thinking) in [
        (ToolChoice::Auto, None, true),
        (ToolChoice::Required, Some(json!("required")), false),
        (ToolChoice::Tool("missing".into()), None, true),
    ] {
        let server = super::FakeUpstream::start(
            super::script(json!({"responses":[
                {"events":[super::text("ok"),super::finish("stop"),super::done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (mut model, context, factory, mut options) =
            policy_request(choice, json!({"supportsNamedToolChoice":false}));
        model.base_url = server.base_url();
        options.api_key = Some("sk-test-secret".into());
        options.retry.max_attempts = 1;
        let mut stream = ara_ai::providers::openai_completions::actual::stream(
            reqwest::Client::new(),
            model,
            context,
            factory.clone(),
            options,
        )
        .unwrap();
        let receipt = stream.failure_receipt();
        let events = tokio::time::timeout(Duration::from_secs(8), async {
            let mut events = Vec::new();
            while let Some(event) = stream.recv().await {
                events.push(event);
            }
            events
        })
        .await;
        drop(stream);
        let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled()).await.unwrap();
        assert!(failures.is_empty(), "Native policy request failed: {failures:?}");
        let events = events.expect("Native policy producer EOF");
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
        assert!(matches!(events.last(), Some(ActualAssistantEvent::Done { reason: ara_ai::StopReason::Stop, .. })));
        let content =
            factory.heap().get(events.last().unwrap().partial(), &"content".into(), factory.runtime()).unwrap();
        let first = factory.heap().get(&content, &"0".into(), factory.runtime()).unwrap();
        let text = factory.heap().get(&first, &"text".into(), factory.runtime()).unwrap();
        assert!(
            matches!(factory.heap().snapshot(&text).unwrap(), ara_rpc::value_heap::ValueSnapshot::String(text) if text.equals_ascii("ok"))
        );
        let recorded = server.requests.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_final_policy(&recorded[0]["body"], expected.as_ref(), thinking, 2);
    }
}

pub async fn observe_request_policy_rejection_http() {
    use ara_ai::{OpenAIReasoningPolicy, ReasoningEffort, ReasoningRequest};
    use std::time::Duration;
    let server = super::FakeUpstream::start(super::script(json!({"responses":[]})), None).await.unwrap();
    for with_policy in [false, true] {
        let (mut model, context, factory) = setup(vec![]);
        model.reasoning = true;
        model.base_url = server.base_url();
        let mut options = StreamOptions::default();
        options.api_key = Some("sk-test-secret".into());
        options.retry.max_attempts = 1;
        options.reasoning = Some(ReasoningRequest { effort: Some(ReasoningEffort::Max), ..Default::default() });
        if with_policy {
            options.reasoning_policy = Some(
                OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,
                "thinking":{"mode":"effort","efforts":["high"]},"compat":{}}))
                .unwrap(),
            );
        }
        let mut stream = ara_ai::providers::openai_completions::actual::stream(
            reqwest::Client::new(),
            model,
            context,
            factory.clone(),
            options,
        )
        .unwrap();
        let receipt = stream.failure_receipt();
        let terminal = tokio::time::timeout(Duration::from_secs(8), async {
            let mut last = None;
            while let Some(event) = stream.recv().await {
                last = Some(event);
            }
            last
        })
        .await;
        drop(stream);
        let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled()).await.unwrap();
        let terminal = terminal.unwrap().expect("Native rejection terminal event");
        assert!(matches!(terminal, ara_ai::ActualAssistantEvent::Error { .. }));
        let error = factory.heap().get(terminal.partial(), &"errorMessage".into(), factory.runtime()).unwrap();
        let ara_rpc::value_heap::ValueSnapshot::String(error) = factory.heap().snapshot(&error).unwrap() else {
            panic!("Native policy error detail");
        };
        assert!(
            error
                .to_utf8()
                .unwrap()
                .contains("Actual provider integration failed; original exception is in the Host receipt"),
            "terminal={terminal:?}, failures={failures:?}"
        );
        let [failure] = failures.as_slice() else {
            panic!("Expected one original policy TypeError; terminal={terminal:?}, failures={failures:?}");
        };
        let ara_rpc::value_heap::ValueError::TypeError(detail) = failure.as_ref() else {
            panic!("Expected one original policy TypeError; terminal={terminal:?}, failures={failures:?}");
        };
        assert!(
            detail.contains(if with_policy { "not supported" } else { "materialized model policy" }),
            "terminal={terminal:?}, failures={failures:?}"
        );
    }
    assert!(server.requests.lock().await.is_empty());
}

// Called once by the existing request/history integration family.
pub fn observe_request_history() {
    occurrence_repair_delayed_aborted_duplicates_and_orphans_share_one_history();
    cross_provider_components_demote_reasoning_and_group_result_images();
    exact_reasoning_replay_keeps_empty_capture_and_opaque_pipe_ids();
    fallback_id_uses_one_neutral_hash_owner_and_original_source_seed();
    requested_choice_preparation_and_final_history_share_one_native_request();
    empty_tools_and_history_keep_fixed_choice_guards();
    original_tools_getters_and_schema_keep_their_native_receiver();
}

// OMP b91db66e Retry policy, exercised through the actual Chat HTTP producer.
// The object-args wire branch emits Start / empty Delta / End on a transport
// error; its {} flush is suppressed. String "{}" instead emits a nonempty Delta.
// This fixture proves Native carrier HTTP behavior, not a real model or Host.
#[tokio::test]
async fn actual_retry_tool_lifecycle_http_group() {
    use ara_ai::{ActualAssistantEvent, StopReason};
    use ara_rpc::value_heap::ValueSnapshot;
    use std::time::Duration;

    for (label, arguments, should_retry) in [
        ("empty-object-arguments", json!({}), true),
        ("string-zero-arguments", json!("{}"), false),
    ] {
        let server = super::FakeUpstream::start(
            super::script(json!({"responses":[
                {"events":[
                    {"data":{"id":"retry-tool","choices":[{
                        "index":0,"delta":{"tool_calls":[{
                            "index":0,"id":"failed-call","type":"function",
                            "function":{"name":"read","arguments":arguments}
                        }]}
                    }]}},
                    {"sleep_ms":50}
                ],"end":"drop"},
                {"events":[super::text("recovered"),super::finish("stop"),super::done()]}
            ]})),
            None,
        )
        .await
        .unwrap();
        let (mut model, context, factory) =
            setup(vec![json!({"role":"user","content":"read the file","timestamp":1})]);
        model.base_url = server.base_url();
        set_original(
            &factory,
            &context,
            "tools",
            json!([{"name":"read","description":"read","parameters":{"type":"object"}}]),
        );
        let mut options = super::opts();
        // Isolate the live replay wrapper from inner HTTP POST attempts. This
        // coarse group must begin uncommitted; existing fixtures are unchanged.
        options.retry.max_attempts = 1;
        options.accept_empty_response = false;
        options.first_event_timeout = Some(Duration::from_secs(2));
        options.idle_timeout = Some(Duration::from_secs(2));
        let mut stream = ara_ai::providers::openai_completions::actual::stream(
            reqwest::Client::new(),
            model,
            context,
            factory.clone(),
            options,
        )
        .unwrap();
        let receipt = stream.failure_receipt();
        // Reuse the existing actual HTTP collector's drain-before-settlement
        // order: a full receiver must not prevent the producer from settling.
        let events = tokio::time::timeout(Duration::from_secs(8), async {
            let mut events = Vec::new();
            while let Some(event) = stream.recv().await {
                events.push(event);
            }
            events
        })
        .await;
        drop(stream);
        let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled())
            .await
            .expect("actual retry HTTP producer settlement");
        assert!(failures.is_empty(), "{label}: integration failures {failures:?}");
        let events = events.expect("actual retry HTTP stream must reach EOF");
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1, "{label}");
        assert_eq!(
            events.iter().filter(|event| matches!(event, ActualAssistantEvent::Start { .. })).count(),
            1,
            "{label}: only one consumer Start"
        );
        let terminal = events.last().expect("actual retry HTTP terminal").partial();
        let get = |root: &ValueHandle, key: &str| {
            factory.heap().get(root, &PropertyKey::from(key), factory.runtime()).unwrap()
        };
        let content = get(terminal, "content");
        assert!(
            matches!(factory.heap().snapshot(&get(&content, "length")).unwrap(), ValueSnapshot::Number(1.0)),
            "{label}: one final content block"
        );
        let first = get(&content, "0");
        if should_retry {
            assert_eq!(server.served(), 2, "{label}: one outer retry");
            assert!(matches!(events.last(), Some(ActualAssistantEvent::Done { reason: StopReason::Stop, .. })));
            assert!(
                !events.iter().any(|event| matches!(
                    event,
                    ActualAssistantEvent::ToolcallStart { .. }
                        | ActualAssistantEvent::ToolcallDelta { .. }
                        | ActualAssistantEvent::ToolcallEnd { .. }
                )),
                "{label}: discarded attempt must hide its whole tool lifecycle"
            );
            assert!(
                matches!(factory.heap().snapshot(&get(&first, "type")).unwrap(),
                    ValueSnapshot::String(value) if value.equals_ascii("text"))
            );
            assert!(
                matches!(factory.heap().snapshot(&get(&first, "text")).unwrap(),
                    ValueSnapshot::String(value) if value.equals_ascii("recovered"))
            );
            let accounting = get(terminal, "retryAccounting");
            let attempts = get(&accounting, "attempts");
            assert!(matches!(
                factory.heap().snapshot(&get(&attempts, "length")).unwrap(),
                ValueSnapshot::Number(2.0)
            ));
            for (index, expected) in [("0", "error"), ("1", "stop")] {
                assert!(
                    matches!(factory.heap().snapshot(&get(&get(&attempts, index), "stopReason")).unwrap(),
                        ValueSnapshot::String(value) if value.equals_ascii(expected)),
                    "{label}: ordered retry receipt"
                );
            }
        } else {
            assert_eq!(server.served(), 1, "{label}: committed arguments must not replay");
            assert!(matches!(events.last(), Some(ActualAssistantEvent::Error { reason: StopReason::Error, .. })));
            assert_eq!(
                events.iter().filter(|event| matches!(event, ActualAssistantEvent::ToolcallStart { .. })).count(),
                1
            );
            assert_eq!(
                events.iter().filter(|event| matches!(event, ActualAssistantEvent::ToolcallEnd { .. })).count(),
                1
            );
            let deltas = events.iter().filter_map(|event| match event {
                ActualAssistantEvent::ToolcallDelta { delta, .. } => Some(delta),
                _ => None,
            }).collect::<Vec<_>>();
            assert_eq!(deltas.len(), 1, "{label}: string args produce one Delta");
            assert!(
                matches!(factory.heap().snapshot(deltas[0]).unwrap(),
                    ValueSnapshot::String(value) if value.equals_ascii("{}"))
            );
            assert!(
                matches!(factory.heap().snapshot(&get(&first, "type")).unwrap(),
                    ValueSnapshot::String(value) if value.equals_ascii("toolCall"))
            );
            assert!(
                matches!(factory.heap().snapshot(&get(&first, "id")).unwrap(),
                    ValueSnapshot::String(value) if value.equals_ascii("failed-call"))
            );
            assert!(matches!(factory.heap().snapshot(&get(terminal, "retryAccounting")).unwrap(),
                ValueSnapshot::Undefined | ValueSnapshot::Null));
        }
        let recorded = server.requests.lock().await;
        assert_eq!(recorded.len(), if should_retry { 2 } else { 1 }, "{label}");
        assert_eq!(recorded[0]["body"]["tools"][0]["function"]["name"], "read");
        if should_retry {
            assert_eq!(recorded[0]["body"], recorded[1]["body"], "{label}: same complete request");
            assert_eq!(recorded[0]["headers"], recorded[1]["headers"], "{label}: same credentials");
        }
    }
}
