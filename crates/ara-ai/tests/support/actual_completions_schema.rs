//! Compound schema / strict-retry observations in the existing HTTP coarse group.
//! Controlled upstream only: no model/Host acceptance is implied.
use ara_ai::providers::openai_completions::{self, ProviderSessionState, StreamOptions};
use ara_ai::{
    ActualAssistantEvent, GraphProviderContext, Model, OpenAIReasoningPolicy, StopReason, ToolChoice,
    graph_values::GraphValueFactory,
};
use ara_rpc::{
    WireValue,
    value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
        ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
    },
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

fn import(factory: &GraphValueFactory, value: Value) -> ValueHandle {
    factory.heap().import_strict(WireValue::parse(&value.to_string()).unwrap()).unwrap()
}
fn set(factory: &GraphValueFactory, target: &ValueHandle, key: &str, value: ValueHandle) {
    factory.heap().define_own_data(target, key.into(), value, factory.runtime()).unwrap();
}
fn get(factory: &GraphValueFactory, target: &ValueHandle, key: &str) -> ValueHandle {
    factory.heap().get(target, &key.into(), factory.runtime()).unwrap()
}
fn setup(tools: Value, runtime: Arc<dyn ValueRuntime>) -> (Model, GraphProviderContext, Arc<GraphValueFactory>) {
    let factory = Arc::new(GraphValueFactory::new(ValueHeap::new().unwrap(), runtime).unwrap());
    let original = factory.heap().object().unwrap();
    set(&factory, &original, "systemPrompt", import(&factory, json!([])));
    set(&factory, &original, "messages", import(&factory, json!([{"role":"user","content":"hello","timestamp":1}])));
    set(&factory, &original, "tools", import(&factory, tools));
    let context = GraphProviderContext {
        original_context: Some(original),
        system_prompt: vec![],
        messages: vec![],
        tools: None,
        history_demotion_dialect: Some(ara_ai::dialect::Dialect::Deepseek),
        history_id_hash: None,
        history_id_hash_function: None,
    };
    let model = super::model("http://127.0.0.1:1/v1");
    (model, context, factory)
}
fn options(compat: Value) -> StreamOptions {
    StreamOptions {
        reasoning_policy: Some(
            OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":false,"compat":compat})).unwrap(),
        ),
        retry: openai_completions::RetryPolicy { max_attempts: 1, ..Default::default() },
        ..Default::default()
    }
}
fn tool(name: &str, strict: Option<bool>, schema: Value) -> Value {
    let mut value = json!({"name":name,"description":"read","parameters":schema});
    if let Some(strict) = strict {
        value["strict"] = json!(strict);
    }
    value
}
fn plain_tool() -> Value {
    tool("read", None, json!({"type":"object","properties":{"path":{"type":"string"}}}))
}
fn wire(
    model: &Model,
    context: &GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: &StreamOptions,
) -> Value {
    serde_json::from_slice(&openai_completions::actual::build_request(model, context, options, factory).unwrap())
        .unwrap()
}

pub fn observe_schema_requests() {
    observe_quarantine_name_conversion();
    let tools = json!([plain_tool(), tool("loose", Some(false), json!({"type":"object"}))]);
    for (compat, no_strict, expected) in [
        (json!({}), false, vec![Some(true), Some(false)]),
        (json!({"toolStrictMode":"all_strict"}), false, vec![None, None]),
        (json!({"toolStrictMode":"none"}), false, vec![None, None]),
        (json!({"supportsStrictMode":false}), false, vec![None, None]),
        (json!({}), true, vec![None, Some(false)]),
    ] {
        let (model, context, factory) = setup(tools.clone(), Arc::new(NoRuntime));
        let mut opts = options(compat);
        opts.no_strict = no_strict;
        let body = wire(&model, &context, factory, &opts);
        for (index, expected) in expected.into_iter().enumerate() {
            assert_eq!(body["tools"][index]["function"].get("strict").and_then(Value::as_bool), expected);
        }
    }
    let (model, context, factory) = setup(json!([plain_tool(), plain_tool()]), Arc::new(NoRuntime));
    let body = wire(&model, &context, factory, &options(json!({"toolStrictMode":"all_strict"})));
    assert!(body["tools"].as_array().unwrap().iter().all(|tool| tool["function"]["strict"] == true));
    let (model, context, factory) =
        setup(json!([tool("open", None, json!({"type":"object","additionalProperties":true}))]), Arc::new(NoRuntime));
    let body = wire(&model, &context, factory, &options(json!({})));
    assert!(body["tools"][0]["function"].get("strict").is_none());
    let root_union = json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},
        "anyOf":[{"required":["a"]},{"required":["b"]}]});
    let (model, context, factory) = setup(json!([tool("union", Some(false), root_union)]), Arc::new(NoRuntime));
    let body = wire(&model, &context, factory, &options(json!({"rejectRootObjectUnion":true,"toolStrictMode":"none"})));
    assert!(body["tools"][0]["function"]["parameters"].get("anyOf").is_none());

    // Quarantine precedes named-tool narrowing and surviving final choice.
    let invalid = tool("invalid", None, json!({"type":"object","properties":{"x":{"type":"string","enum":[1]}}}));
    let (model, context, factory) = setup(json!([invalid, plain_tool()]), Arc::new(NoRuntime));
    let mut opts = options(json!({"supportsNamedToolChoice":false}));
    opts.tool_choice = Some(ToolChoice::Tool("invalid".into()));
    let body = wire(&model, &context, factory, &opts);
    assert_eq!(body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(body["tools"][0]["function"]["name"], "read");
    assert!(body.get("tool_choice").is_none());

    // Grammar changes schema slots while instance values and closed objects
    // retain their exact literal booleans. None mode emits the base schema.
    let schema = json!({"type":"object","additionalProperties":false,"unevaluatedProperties":false,
        "properties":{"open":true,"closed":false,"arr":{"type":"array","items":true}},
        "default":{"items":true,"enabled":false},"enum":[{"items":true,"enabled":false}]});
    let (model, context, factory) = setup(json!([tool("grammar", Some(false), schema)]), Arc::new(NoRuntime));
    let opts = options(json!({"toolStrictMode":"none","toolSchemaFlavor":"grammar"}));
    let body = wire(&model, &context, factory, &opts);
    let emitted = &body["tools"][0]["function"]["parameters"];
    assert_eq!(emitted["additionalProperties"], false);
    assert_eq!(emitted["unevaluatedProperties"], false);
    assert_eq!(emitted["properties"]["open"]["anyOf"].as_array().unwrap().len(), 6);
    assert_eq!(emitted["properties"]["closed"]["not"]["anyOf"].as_array().unwrap().len(), 6);
    assert_eq!(emitted["properties"]["arr"]["items"]["anyOf"].as_array().unwrap().len(), 6);
    assert_eq!(emitted["default"], json!({"items":true,"enabled":false}));
    assert_eq!(emitted["enum"], json!([{"items":true,"enabled":false}]));

    observe_literal_identity();
    observe_original_callbacks();
}
fn observe_literal_identity() {
    let heap = ValueHeap::new().unwrap();
    let runtime = NoRuntime;
    let first = heap.module_open_subschema_widening().unwrap();
    let second = heap.module_open_subschema_widening().unwrap();
    assert!(first.same_identity(&second));
    let array = heap.get(&first, &"anyOf".into(), &runtime).unwrap();
    let original = heap.get(&array, &"0".into(), &runtime).unwrap();
    heap.assign(&original, "type".into(), heap.string("integer".into()).unwrap(), &runtime).unwrap();
    let again = heap.module_open_subschema_widening().unwrap();
    let type_name = heap
        .get(
            &heap.get(&heap.get(&again, &"anyOf".into(), &runtime).unwrap(), &"0".into(), &runtime).unwrap(),
            &"type".into(),
            &runtime,
        )
        .unwrap();
    assert!(matches!(heap.snapshot(&type_name).unwrap(), ValueSnapshot::String(text) if text.equals_ascii("integer")));
    assert!(heap.assign(&first, "anyOf".into(), heap.array(vec![]).unwrap(), &runtime).is_err());
    assert!(heap.define_own_data(&first, "extra".into(), heap.boolean(true).unwrap(), &runtime).is_err());
    assert!(!heap.delete_own(&first, &"anyOf".into(), &runtime).unwrap());
    let other_heap = ValueHeap::new().unwrap();
    let other = other_heap.module_open_subschema_widening().unwrap();
    assert!(!first.same_identity(&other));
    let type_name = other_heap
        .get(
            &other_heap
                .get(&other_heap.get(&other, &"anyOf".into(), &runtime).unwrap(), &"0".into(), &runtime)
                .unwrap(),
            &"type".into(),
            &runtime,
        )
        .unwrap();
    assert!(
        matches!(other_heap.snapshot(&type_name).unwrap(), ValueSnapshot::String(text) if text.equals_ascii("string"))
    );
}

#[derive(Default)]
struct OriginalRuntime {
    trace: Mutex<Vec<&'static str>>,
    throw_map: bool,
    override_result: Mutex<Option<ValueHandle>>,
}
impl ValueRuntime for OriginalRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(7811), receiver, arguments } => {
                self.trace.lock().unwrap().push("map");
                if self.throw_map {
                    return Err(ValueError::TypeError("original tools map failed".into()));
                }
                if let Some(value) = self.override_result.lock().unwrap().as_ref() {
                    return Ok(RuntimeReply::Value(value.clone()));
                }
                let heap = receiver.heap();
                let item = heap.get(receiver, &"0".into(), self)?;
                let result =
                    heap.call(&arguments[0], &heap.undefined()?, &[item, heap.number(0.0)?, receiver.clone()], self)?;
                Ok(RuntimeReply::Value(heap.array(vec![result])?))
            }
            RuntimeRequest::Call { capability: CapabilityId(7812), receiver, arguments } => {
                assert!(arguments.is_empty());
                self.trace.lock().unwrap().push("strict");
                Ok(RuntimeReply::Value(receiver.heap().boolean(false)?))
            }
            other => Err(ValueError::MissingCapability(other.capability())),
        }
    }
}
fn runtime_setup(runtime: Arc<OriginalRuntime>) -> (Model, GraphProviderContext, Arc<GraphValueFactory>) {
    let (model, context, factory) = setup(json!([plain_tool()]), runtime);
    let tools = get(&factory, context.original_context.as_ref().unwrap(), "tools");
    let tool = get(&factory, &tools, "0");
    set(&factory, &tools, "map", factory.heap().function(CapabilityId(7811)).unwrap());
    factory
        .heap()
        .define_own(
            &tool,
            "strict".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(7812))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            factory.runtime(),
        )
        .unwrap();
    (model, context, factory)
}
fn observe_original_callbacks() {
    for no_strict in [false, true] {
        let runtime = Arc::new(OriginalRuntime::default());
        let (model, context, factory) = runtime_setup(runtime.clone());
        let mut opts = options(json!({"toolStrictMode":"none"}));
        opts.no_strict = no_strict;
        let body = wire(&model, &context, factory, &opts);
        assert!(body["tools"][0]["function"].get("strict").is_none());
        assert_eq!(
            &*runtime.trace.lock().unwrap(),
            if no_strict { &["map", "strict"][..] } else { &["map", "strict", "strict"][..] }
        );
    }
    let runtime = Arc::new(OriginalRuntime { throw_map: true, ..Default::default() });
    let (model, context, factory) = runtime_setup(runtime);
    assert!(matches!(openai_completions::actual::build_request(&model,&context,&options(json!({})),factory),
        Err(ValueError::TypeError(detail)) if detail == "original tools map failed"));
    // Distinguish the documented carrier limit: ordinary depth is accepted;
    // deep schema slots produce a Native RangeError before POST. This does
    // not claim parity with a JS engine's platform-dependent stack ceiling.
    for (depth, accepted) in [(16, true), (129, false)] {
        let runtime = Arc::new(OriginalRuntime::default());
        let (model, context, factory) = runtime_setup(runtime.clone());
        let tools = get(&factory, context.original_context.as_ref().unwrap(), "tools");
        let tool = get(&factory, &tools, "0");
        let mut schema = factory.heap().boolean(true).unwrap();
        for _ in 0..depth {
            let next = factory.heap().object().unwrap();
            set(&factory, &next, "type", factory.heap().string("array".into()).unwrap());
            set(&factory, &next, "items", schema);
            schema = next;
        }
        let record = factory.heap().object().unwrap();
        for (key, value) in [
            ("tool", tool),
            ("baseParameters", schema.clone()),
            ("parameters", schema),
            ("strict", factory.heap().boolean(false).unwrap()),
        ] {
            set(&factory, &record, key, value);
        }
        *runtime.override_result.lock().unwrap() = Some(factory.heap().array(vec![record]).unwrap());
        let result = openai_completions::actual::build_request(
            &model,
            &context,
            &options(json!({"toolStrictMode":"none","toolSchemaFlavor":"grammar"})),
            factory,
        );
        if accepted {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(ValueError::RangeError(_))));
        }
    }
}

async fn collect(
    model: Model,
    context: GraphProviderContext,
    factory: Arc<GraphValueFactory>,
    options: StreamOptions,
) -> (Vec<ActualAssistantEvent>, Vec<Arc<ValueError>>) {
    let mut stream =
        openai_completions::actual::stream(reqwest::Client::new(), model, context, factory, options).unwrap();
    let receipt = stream.failure_receipt();
    let events = tokio::time::timeout(Duration::from_secs(8), async {
        let mut events = vec![];
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
        events
    })
    .await
    .expect("Native schema producer EOF");
    drop(stream);
    let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled()).await.unwrap();
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    (events, failures)
}
fn ok() -> Value {
    json!({"events":[super::text("ok"),super::finish("stop"),super::done()]})
}
fn reject(status: u16, body: Value) -> Value {
    json!({"status":status,"body":body.to_string()})
}
fn strict_error() -> Value {
    json!({"error":{"message":"invalid schema for function read","type":"invalid_request_error"}})
}
fn assert_done(events: &[ActualAssistantEvent]) {
    assert!(matches!(events.last(), Some(ActualAssistantEvent::Done { reason: StopReason::Stop, .. })));
}
#[derive(Default)]
struct NameRuntime {
    receiver: Mutex<Option<ValueHandle>>,
    thrown: Mutex<Option<ValueHandle>>,
    calls: std::sync::atomic::AtomicUsize,
}
impl ValueRuntime for NameRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(7821), receiver, arguments } => {
                assert!(receiver.same_identity(self.receiver.lock().unwrap().as_ref().unwrap()));
                if let Some(hint) = arguments.first() {
                    assert_eq!(arguments.len(), 1);
                    assert!(
                        matches!(receiver.heap().snapshot(hint)?,ValueSnapshot::String(text) if text.equals_ascii("string"))
                    );
                }
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if let Some(thrown) = self.thrown.lock().unwrap().as_ref() {
                    return Err(ValueError::Thrown(thrown.clone()));
                }
                Ok(RuntimeReply::Value(receiver.heap().string("converted original name".into())?))
            }
            other => Err(ValueError::MissingCapability(other.capability())),
        }
    }
}
#[derive(Debug, Default)]
struct NameWarning {
    names: Mutex<Vec<ara_rpc::WireString>>,
}
impl openai_completions::CompletionsQuarantinePort for NameWarning {
    fn quarantined(
        &self,
        name: &ara_rpc::WireString,
        _: &ara_rpc::WireString,
        _: &GraphValueFactory,
    ) -> Result<(), ValueError> {
        self.names.lock().unwrap().push(name.clone());
        Ok(())
    }
}
fn name_setup(exotic: bool, runtime: Arc<NameRuntime>) -> (Model, GraphProviderContext, Arc<GraphValueFactory>) {
    let invalid = tool("invalid", None, json!({"type":"object","properties":{"x":{"type":"string","enum":[1]}}}));
    let (model, context, factory) = setup(json!([invalid, plain_tool()]), runtime.clone());
    let name = factory.heap().object().unwrap();
    let key = if exotic { factory.heap().to_primitive_key() } else { "toString".into() };
    factory
        .heap()
        .define_own_data(&name, key, factory.heap().function(CapabilityId(7821)).unwrap(), factory.runtime())
        .unwrap();
    *runtime.receiver.lock().unwrap() = Some(name.clone());
    let tools = get(&factory, context.original_context.as_ref().unwrap(), "tools");
    let invalid = get(&factory, &tools, "0");
    set(&factory, &invalid, "name", name);
    (model, context, factory)
}
fn observe_quarantine_name_conversion() {
    for (exotic, with_port) in [(false, false), (false, true), (true, false), (true, true)] {
        let runtime = Arc::new(NameRuntime::default());
        let (model, context, factory) = name_setup(exotic, runtime.clone());
        let warning = Arc::new(NameWarning::default());
        let mut opts = options(json!({}));
        if with_port {
            opts.quarantine_port = Some(warning.clone());
        }
        let body = wire(&model, &context, factory, &opts);
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["tools"][0]["function"]["name"], "read");
        assert_eq!(runtime.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let names = warning.names.lock().unwrap();
        assert_eq!(names.len(), usize::from(with_port));
        if with_port {
            assert!(names[0].equals_ascii("converted original name"));
        }
    }
}
async fn observe_quarantine_name_throw_no_post() {
    let server = super::FakeUpstream::start(super::script(json!({"responses":[ok()]})), None).await.unwrap();
    for exotic in [false, true] {
        let runtime = Arc::new(NameRuntime::default());
        let (mut model, context, factory) = name_setup(exotic, runtime.clone());
        model.base_url = server.base_url();
        let thrown = factory.heap().object().unwrap();
        *runtime.thrown.lock().unwrap() = Some(thrown.clone());
        let (_, failures) = collect(model, context, factory, options(json!({}))).await;
        assert!(
            matches!(failures.as_slice(),[failure] if matches!(failure.as_ref(),ValueError::Thrown(value) if value.same_identity(&thrown)))
        );
        assert_eq!(runtime.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(server.served(), 0);
    }
}
#[derive(Default)]
struct RebuildRuntime {
    arrays: Mutex<Vec<ValueHandle>>,
    reads: std::sync::atomic::AtomicUsize,
}
impl ValueRuntime for RebuildRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(7814), arguments, .. } => {
                assert!(arguments.is_empty());
                let index = self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let arrays = self.arrays.lock().unwrap();
                Ok(RuntimeReply::Value(arrays[if index < 3 { 0 } else { 1 }].clone()))
            }
            other => Err(ValueError::MissingCapability(other.capability())),
        }
    }
}
async fn observe_rebuild_and_cancel() {
    let server =
        super::FakeUpstream::start(super::script(json!({"responses":[reject(400,strict_error()),ok()]})), None)
            .await
            .unwrap();
    let runtime = Arc::new(RebuildRuntime::default());
    let (mut model, context, factory) = setup(json!([plain_tool()]), runtime.clone());
    model.base_url = server.base_url();
    *runtime.arrays.lock().unwrap() = vec![
        import(&factory, json!([plain_tool()])),
        import(&factory, json!([tool("changed", None, json!({"type":"object"}))])),
    ];
    factory
        .heap()
        .define_own(
            context.original_context.as_ref().unwrap(),
            "tools".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(7814))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            factory.runtime(),
        )
        .unwrap();
    let (events, failures) = collect(model, context, factory, options(json!({}))).await;
    assert_done(&events);
    assert!(failures.is_empty());
    assert_eq!(runtime.reads.load(std::sync::atomic::Ordering::SeqCst), 5);
    let requests = server.requests.lock().await;
    assert_eq!(requests[0]["body"]["tools"][0]["function"]["name"], "read");
    assert_eq!(requests[1]["body"]["tools"][0]["function"]["name"], "changed");
    drop(requests);
    let server = super::FakeUpstream::start(super::script(json!({"responses":[ok()]})), None).await.unwrap();
    let (mut model, context, factory) = setup(json!([plain_tool()]), Arc::new(NoRuntime));
    model.base_url = server.base_url();
    let opts = options(json!({}));
    opts.cancel.cancel();
    let (events, failures) = collect(model, context, factory, opts).await;
    assert!(failures.is_empty());
    assert!(matches!(events.last(), Some(ActualAssistantEvent::Error { reason: StopReason::Aborted, .. })));
    assert_eq!(server.served(), 0);
}
pub async fn observe_strict_http() {
    for no_strict in [false, true] {
        let server = super::FakeUpstream::start(super::script(json!({"responses":[ok()]})), None).await.unwrap();
        let (mut model, context, factory) = setup(
            json!([tool(
                "read",
                Some(true),
                json!({
                    "type":"object","properties":{"path":{"type":"string"},"optional":{"type":"string"}},"required":["path"]
                })
            )]),
            Arc::new(NoRuntime),
        );
        model.base_url = server.base_url();
        let mut opts = options(json!({}));
        opts.no_strict = no_strict;
        let (events, failures) = collect(model, context, factory, opts).await;
        assert_done(&events);
        assert!(failures.is_empty());
        let requests = server.requests.lock().await;
        assert_eq!(requests.len(), 1);
        let tool = &requests[0]["body"]["tools"][0]["function"];
        assert_eq!(tool.get("strict").and_then(Value::as_bool), if no_strict { None } else { Some(true) });
        let required = tool["parameters"]["required"].as_array().unwrap();
        assert!(required.contains(&json!("path")));
        assert_eq!(required.contains(&json!("optional")), !no_strict);
    }
    observe_quarantine_name_throw_no_post().await;
    observe_rebuild_and_cancel().await;
    let server = super::FakeUpstream::start(
        super::script(json!({"responses":[reject(400,strict_error()),ok(),ok(),reject(400,strict_error()),ok()]})),
        None,
    )
    .await
    .unwrap();
    let (mut model, context, factory) = setup(json!([plain_tool()]), Arc::new(NoRuntime));
    model.base_url = server.base_url();
    let state = Arc::new(ProviderSessionState::default());
    let mut opts = options(json!({}));
    opts.provider_session_state = Some(state.clone());
    let (events, failures) = collect(model.clone(), context.clone(), factory.clone(), opts.clone()).await;
    assert_done(&events);
    assert!(failures.is_empty());
    assert!(state.strict_tools_disabled(&model));
    let (events, failures) = collect(model.clone(), context.clone(), factory.clone(), opts.clone()).await;
    assert_done(&events);
    assert!(failures.is_empty());
    let mut other = model.clone();
    other.id.push_str("-other");
    assert!(!state.strict_tools_disabled(&other));
    other = model.clone();
    other.provider.push_str("-other");
    assert!(!state.strict_tools_disabled(&other));
    other = model.clone();
    other.base_url.push_str("/other");
    assert!(!state.strict_tools_disabled(&other));
    assert!(!ProviderSessionState::default().strict_tools_disabled(&model));
    state.clear();
    assert!(!state.strict_tools_disabled(&model));
    let (events, failures) = collect(model, context, factory, opts).await;
    assert_done(&events);
    assert!(failures.is_empty());
    let requests = server.requests.lock().await;
    assert_eq!(requests.len(), 5);
    for (index, strict) in [(0, true), (1, false), (2, false), (3, true), (4, false)] {
        let expected = strict.then_some(json!(true));
        assert_eq!(requests[index]["body"]["tools"][0]["function"].get("strict"), expected.as_ref());
    }
    drop(requests);
    for (provider, status, error, compat, retry) in [
        ("fake", 422, strict_error(), json!({}), true),
        (
            "fake",
            400,
            json!({"error":{"message":"compiled grammar is too large","type":"invalid_request_error"}}),
            json!({"retryWithoutStrictOnGrammarError":true}),
            true,
        ),
        (
            "openrouter",
            400,
            json!({"error":{"message":"Provider returned error","metadata":{"raw":""}}}),
            json!({}),
            true,
        ),
        (
            "openrouter",
            400,
            json!({"error":{"message":"Provider returned error","metadata":{"raw":"upstream reason"}}}),
            json!({}),
            false,
        ),
        ("fake", 400, json!({"error":{"message":"unrelated request rejected"}}), json!({}), false),
    ] {
        let server = super::FakeUpstream::start(super::script(json!({"responses":[reject(status,error),ok()]})), None)
            .await
            .unwrap();
        let (mut model, context, factory) = setup(json!([plain_tool()]), Arc::new(NoRuntime));
        model.base_url = server.base_url();
        model.provider = provider.into();
        let (events, failures) = collect(model, context, factory, options(compat)).await;
        assert!(failures.is_empty());
        if retry {
            assert_done(&events);
        } else {
            assert!(matches!(events.last(), Some(ActualAssistantEvent::Error { .. })));
        }
        assert_eq!(server.served(), if retry { 2 } else { 1 });
    }
    // Exactly one schema fallback; an accepted SSE error is never a candidate.
    for responses in [
        json!([reject(400, strict_error()), reject(400, strict_error()), ok()]),
        json!([{"events":[{"data":{"error":{"code":400,"message":"invalid schema for function read"}}}]},ok()]),
        json!([{"events":[super::tool_call(0,"call_read","read","{}"),{"data":{"error":{"code":400,"message":"invalid schema for function read"}}}]},ok()]),
    ] {
        let accepted = responses[0].get("events").is_some();
        let server = super::FakeUpstream::start(super::script(json!({"responses":responses})), None).await.unwrap();
        let (mut model, context, factory) = setup(json!([plain_tool()]), Arc::new(NoRuntime));
        model.base_url = server.base_url();
        let (events, failures) = collect(model, context, factory, options(json!({}))).await;
        assert!(failures.is_empty());
        assert!(matches!(events.last(), Some(ActualAssistantEvent::Error { .. })));
        assert_eq!(server.served(), if accepted { 1 } else { 2 });
    }
    let server = super::FakeUpstream::start(super::script(json!({"responses":[ok()]})), None).await.unwrap();
    let runtime = Arc::new(OriginalRuntime { throw_map: true, ..Default::default() });
    let (mut model, context, factory) = runtime_setup(runtime);
    model.base_url = server.base_url();
    let (_, failures) = collect(model, context, factory, options(json!({}))).await;
    assert!(
        matches!(failures.as_slice(),[failure] if matches!(failure.as_ref(),ValueError::TypeError(detail) if detail=="original tools map failed"))
    );
    assert_eq!(server.served(), 0);
}


fn terminal_json(factory: &GraphValueFactory, events: &[ActualAssistantEvent]) -> Value {
    let output=match events.last().unwrap() {
        ActualAssistantEvent::Done { message,.. } => message,
        ActualAssistantEvent::Error { error,.. } => error,
        _ => panic!("missing terminal"),
    };
    let wire=factory.heap().stringify(output,ara_rpc::value_heap::StringifyMode::Plain,None,factory.runtime()).unwrap().unwrap();
    serde_json::from_str(&wire.to_utf8().unwrap()).unwrap()
}
pub async fn observe_upgraded_chat_callback() {
    for mode in ["ok","reject","cancel","body_timeout"] {
        let body=if mode=="body_timeout" {vec![json!({"sleep_ms":1000}),super::text("late"),super::finish("stop"),super::done()]}
            else {vec![super::text("Hello delayed"),super::finish("stop"),super::done()]};
        let server=super::FakeUpstream::start(super::script(json!({"responses":[{"events":body.clone()},{"events":body}]})),None).await.unwrap();
        let (mut model,context,factory)=setup(json!([]),Arc::new(NoRuntime));model.base_url=server.base_url();
        let (events,failures)=collect(model,context,factory.clone(),super::chat_callback_options(mode)).await;
        assert!(failures.is_empty(),"{mode}: {failures:?}");
        let result=terminal_json(&factory,&events);
        match mode {
            "ok" => {assert_done(&events);assert_eq!(result["content"][0]["text"],"Hello delayed");},
            "reject" => {assert_eq!(result["stopReason"],"error");assert!(result["errorMessage"].as_str().unwrap().contains("provider response callback failed"));},
            "cancel" => assert_eq!(result["stopReason"],"aborted"),
            _ => assert_eq!(result["errorMessage"],openai_completions::FIRST_EVENT_TIMEOUT_MESSAGE),
        }
        assert_eq!(server.served(),if mode=="body_timeout" {2} else {1},"{mode}");
    }
}
pub async fn observe_upgraded_chat_inband() {
    for (label,frame,status,account) in super::chat_inband_cases() {
        let server=super::FakeUpstream::start(super::script(json!({"responses":[{"events":[super::text("committed"),{"data":frame},super::done()]}]})),None).await.unwrap();
        let (mut model,context,factory)=setup(json!([]),Arc::new(NoRuntime));model.base_url=server.base_url();
        let (events,failures)=collect(model,context,factory.clone(),options(json!({}))).await;
        assert!(failures.is_empty(),"{label}: {failures:?}");
        let result=terminal_json(&factory,&events);
        assert_eq!(result["stopReason"],"error","{label}: {result}");
        assert_eq!(result.get("errorStatus").and_then(Value::as_u64),status.map(u64::from),"{label}: {result}");
        assert_eq!(result["failureEvidence"]["errorId"].as_u64().unwrap() & u64::from(ara_ai::retry_classification::flag::USAGE_LIMIT) != 0,account,"{label}: fixed account classification");
        if label=="opaque" || label=="framing" {assert_eq!(result["errorMessage"],"429 Provider returned an in-band provider error");}
        if label=="float_code" {
            assert_eq!(result["errorMessage"],"429 Provider returned an in-band provider error");
            assert_eq!(result["failureEvidence"]["code"],"429");
        }
        if label=="markup" {assert_eq!(result["errorMessage"],"503 busy now");}
        assert_eq!(server.served(),1,"{label}");
    }
    for (frame,status,detail) in super::chat_inband_legacy_cases() {
        let events=if detail=="error" {vec![super::text("committed"),json!({"data":frame}),super::done()]}
            else {vec![json!({"data":frame}),super::done()]};
        let server=super::FakeUpstream::start(super::script(json!({"responses":[{"events":events}]})),None).await.unwrap();
        let (mut model,context,factory)=setup(json!([]),Arc::new(NoRuntime));model.base_url=server.base_url();
        let (events,failures)=collect(model,context,factory.clone(),options(json!({}))).await;
        assert!(failures.is_empty());let result=terminal_json(&factory,&events);
        assert_eq!(result["stopReason"],"error");
        assert_eq!(result.get("errorStatus").and_then(Value::as_u64),status.map(u64::from));
        assert!(result["errorMessage"].as_str().unwrap().contains(detail));
        assert_eq!(server.served(),1);
    }
    for frame in [json!({"code":429.5}),json!({"status":65536503u64}),json!({"type":"response.output_text.delta"})] {
        let server=super::FakeUpstream::start(super::script(json!({"responses":[{"events":[{"data":frame},super::text("ok"),super::finish("stop"),super::done()]}]})),None).await.unwrap();
        let (mut model,context,factory)=setup(json!([]),Arc::new(NoRuntime));model.base_url=server.base_url();
        let (events,failures)=collect(model,context,factory.clone(),options(json!({}))).await;
        assert!(failures.is_empty());assert_done(&events);let result=terminal_json(&factory,&events);
        assert!(result.get("errorStatus").is_none());assert_eq!(result["content"][0]["text"],"ok");
    }
}
pub async fn observe_upgraded_chat_replay() {
    for frame in [json!({"status":429}),json!({"error":{"message":"Throttled. Please retry"}})] {
    for committed in ["none","text","tool"] {
        let mut events=vec![];
        match committed {
            "text" => events.push(super::text("partial")),
            "tool" => events.push(super::tool_call(0,"call1","read","{\"path\":\"a\"}")),
            _ => {},
        }
        events.push(json!({"data":frame}));
        let server=super::FakeUpstream::start(super::script(json!({"responses":[{"events":events},ok()]})),None).await.unwrap();
        let (mut model,context,factory)=setup(json!([]),Arc::new(NoRuntime));model.base_url=server.base_url();
        let mut opts=options(json!({}));opts.retry.max_attempts=3;
        let (events,failures)=collect(model,context,factory.clone(),opts).await;assert!(failures.is_empty());
        if committed=="none" && frame.get("status").is_some() {assert_eq!(server.served(),2);assert_done(&events);}
        else if committed=="none" {
            assert_eq!(server.served(),1);let result=terminal_json(&factory,&events);
            assert_eq!(result["stopReason"],"error");
            assert_eq!(result["failureEvidence"]["errorId"].as_u64().unwrap() & u64::from(ara_ai::retry_classification::flag::TRANSIENT),u64::from(ara_ai::retry_classification::flag::TRANSIENT));
            assert!(result.get("errorStatus").is_none());
        }
        else {assert_eq!(server.served(),1);assert!(matches!(events.last(),Some(ActualAssistantEvent::Error {..})));}
    }
    }
}
