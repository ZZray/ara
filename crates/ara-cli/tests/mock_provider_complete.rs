//! Original mock-provider.test.ts25 consumers at OMP596f2da (MIT).
//! Copyright (c) 2025 Mario Zechner; 2025-2026 Can Boluk; 2026 Stencil Labs, Inc.
//! TS->Rust bridge: plain authored records are same-heap data; functions are
//! compiled graph functions; async generators expose an actual graph async
//! iterator whose next returns a native Promise. Only this test's existing Run
//! drains jobs. Stream.result rejection is the sealed actual failure receipt;
//! terminal results/events remain original ValueHandles, without serialization.

use ara_ai::{
    ActualAssistantEvent, ActualAssistantStream, AssistantMessageEvent as Event, CallOptions,
    graph_values::GraphValueFactory,
    providers::mock::{
        MOCK_API, MockHandler, MockModel, MockModelHandle, create_mock_model, is_mock_model,
        register_mock_api, stream_mock,
    },
};
use ara_cli::{
    custom_models::CustomModelOverlay,
    mock_provider_host::{NativeMockBindings, RegistryMockApiRegistrar, registered_mock_api},
    model_patch::ProviderOverride,
    model_registry_extensions::{
        ExtensionRegistry, ExtensionRegistryHost, ExtensionResult, OpaqueExtensionHandle,
        RuntimeApiRegistration, RuntimeOAuthEntry,
    },
    native_realm::{
        NativeRealm,
        promise::{NativeDrain, NativePromiseRun},
    },
};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[path = "mock_provider_complete/source_observations.rs"]
mod source_observations;

type Result<T> = std::result::Result<T, ValueError>;

fn get(
    heap: &Arc<ValueHeap>,
    value: &ValueHandle,
    key: &str,
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.get(value, &key.into(), rt)
}
fn obj(
    heap: &Arc<ValueHeap>,
    rt: &dyn ValueRuntime,
    entries: &[(&str, ValueHandle)],
) -> Result<ValueHandle> {
    let object = heap.object()?;
    for (key, value) in entries {
        heap.define_own_data(&object, (*key).into(), value.clone(), rt)?;
    }
    Ok(object)
}
fn string(heap: &Arc<ValueHeap>, value: &ValueHandle) -> String {
    let ValueSnapshot::String(text) = heap.snapshot(value).unwrap() else {
        panic!("original string expected");
    };
    text.to_utf8().unwrap()
}
fn text_response(
    heap: &Arc<ValueHeap>,
    rt: &dyn ValueRuntime,
    text: String,
) -> Result<ValueHandle> {
    obj(
        heap,
        rt,
        &[("content", heap.array(vec![heap.string(text.into())?])?)],
    )
}

// These are original test functions, not substitutes for the production owner.
// Their graph captures retain actual context/options/call-array identities.
static HANDLER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mock-provider.test.ts:89,99,337,349 original handler bodies",
    name: "handler",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, rt| {
        let state = &captures[0];
        let context = args.first().cloned().unwrap_or(heap.undefined()?);
        let options = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let trace = get(heap, state, "trace", rt)?;
        heap.array_push(
            &trace,
            &[obj(
                heap,
                rt,
                &[("context", context.clone()), ("options", options.clone())],
            )?],
            rt,
        )?;
        let mode = string(heap, &get(heap, state, "mode", rt)?);
        let text = if mode == "constant" {
            "fallback".to_owned()
        } else {
            let messages = get(heap, &context, "messages", rt)?;
            let length = heap.array_length(&messages, rt)?;
            if mode == "options" {
                let tier = if matches!(heap.kind(&options)?, ValueKind::Undefined | ValueKind::Null)
                {
                    "none".to_owned()
                } else {
                    let value = get(heap, &options, "apiKey", rt)?;
                    if matches!(heap.kind(&value)?, ValueKind::Undefined | ValueKind::Null) {
                        "none".to_owned()
                    } else {
                        string(heap, &value)
                    }
                };
                format!("msgs={length} tier={tier}")
            } else {
                format!("fallback:{length}")
            }
        };
        text_response(heap, rt, text)
    },
};
static ITERATOR_SELF: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "TS->Rust async-generator object bridge: original Symbol.asyncIterator returns its own iterator",
    name: "[Symbol.asyncIterator]",
    length: 0,
    capture_count: 0,
    form: CompiledFunctionForm::Ordinary,
    body: |_, _, receiver, _, _| Ok(receiver.clone()),
};
static RETURN_YIELD: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mock-provider.test.ts:275 await gate.promise; yield after",
    name: "resumeYield",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |_, captures, _, _, _| Ok(captures[0].clone()),
};
static ASYNC_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mock-provider.test.ts:257-375 async generator bridge; original next/yield/await/throw",
    name: "next",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, _, rt| {
        let state = &captures[0];
        let index = heap.to_number(&get(heap, state, "index", rt)?, rt)? as u32;
        heap.assign(
            state,
            "index".into(),
            heap.number(f64::from(index + 1))?,
            rt,
        )?;
        let promise = get(heap, state, "Promise", rt)?;
        let mode = string(heap, &get(heap, state, "mode", rt)?);
        if mode == "throw" && index == 1 {
            let reject = get(heap, &promise, "reject", rt)?;
            return heap.call(&reject, &promise, &[get(heap, state, "error", rt)?], rt);
        }
        let responses = get(heap, state, "responses", rt)?;
        let done = index >= heap.array_length(&responses, rt)?;
        let value = if done {
            heap.undefined()?
        } else if mode == "calls" && index == 1 {
            let calls = get(heap, state, "calls", rt)?;
            let last = get(
                heap,
                &calls,
                &(heap.array_length(&calls, rt)? - 1).to_string(),
                rt,
            )?;
            let context = get(heap, &last, "context", rt)?;
            let messages = get(heap, &context, "messages", rt)?;
            text_response(
                heap,
                rt,
                format!("saw {} message(s)", heap.array_length(&messages, rt)?),
            )?
        } else {
            get(heap, &responses, &index.to_string(), rt)?
        };
        let result = obj(heap, rt, &[("done", heap.boolean(done)?), ("value", value)])?;
        if mode == "gate" && index == 1 {
            let gate = get(heap, state, "gate", rt)?;
            let then = get(heap, &gate, "then", rt)?;
            let continuation = heap.fresh_compiled_function(&RETURN_YIELD, &[result])?;
            return heap.call(&then, &gate, &[continuation], rt);
        }
        let resolve = get(heap, &promise, "resolve", rt)?;
        heap.call(&resolve, &promise, &[result], rt)
    },
};

struct Harness {
    realm: Arc<NativeRealm>,
    driver: NativePromiseRun,
    factory: Arc<GraphValueFactory>,
    bindings: Arc<NativeMockBindings>,
}
struct Collected {
    events: Vec<ActualAssistantEvent>,
    terminal: Option<ValueHandle>,
    failures: Vec<Arc<ValueError>>,
}
impl Collected {
    fn result(&self) -> &ValueHandle {
        assert!(
            self.failures.is_empty(),
            "unexpected original stream failure: {:?}",
            self.failures
        );
        self.terminal.as_ref().expect("original terminal result")
    }
}
impl Harness {
    fn new() -> Self {
        let realm = NativeRealm::prepare().unwrap();
        let driver = realm.begin_promise_run().unwrap();
        let admission = driver.admission();
        let scope = driver.root_scope();
        let parent = driver.root_parent();
        let runtime = Arc::new(admission.scoped(scope, parent).unwrap());
        let factory =
            Arc::new(GraphValueFactory::new(realm.values.heap().clone(), runtime).unwrap());
        let bindings =
            NativeMockBindings::prepare(&factory, &realm, admission, scope, parent).unwrap();
        Self {
            realm,
            driver,
            factory,
            bindings,
        }
    }
    fn heap(&self) -> &Arc<ValueHeap> {
        self.factory.heap()
    }
    fn data(&self, value: Value) -> ValueHandle {
        self.heap().import_serde(&value).unwrap()
    }
    fn get(&self, value: &ValueHandle, key: &str) -> ValueHandle {
        get(self.heap(), value, key, self.factory.runtime()).unwrap()
    }
    fn put(&self, value: &ValueHandle, key: &str, item: ValueHandle) {
        self.heap()
            .define_own_data(value, key.into(), item, self.factory.runtime())
            .unwrap();
    }
    fn s(&self, value: &ValueHandle) -> String {
        string(self.heap(), value)
    }
    fn n(&self, value: &ValueHandle) -> f64 {
        self.heap()
            .to_number(value, self.factory.runtime())
            .unwrap()
    }
    fn len(&self, value: &ValueHandle) -> u32 {
        self.heap()
            .array_length(value, self.factory.runtime())
            .unwrap()
    }
    fn context(&self) -> ValueHandle {
        self.data(
            json!({"systemPrompt":[],"messages":[{"role":"user","content":"hi","timestamp":0}]}),
        )
    }
    fn create(&self, options: ValueHandle) -> Arc<MockModel> {
        create_mock_model(self.factory.clone(), self.bindings.clone(), options).unwrap()
    }
    fn handler(&self, mode: &str) -> (ValueHandle, ValueHandle) {
        let state = self.data(json!({"mode":mode,"trace":[]}));
        let handler = self
            .heap()
            .fresh_compiled_function(&HANDLER, std::slice::from_ref(&state))
            .unwrap();
        (handler, state)
    }
    fn async_source(&self, responses: Value, mode: &str) -> (ValueHandle, ValueHandle) {
        let state = self.data(json!({"responses":responses,"index":0,"mode":mode}));
        let promise = self.get(&self.realm.globals.environment, "Promise");
        self.put(&state, "Promise", promise);
        let source = self.heap().object().unwrap();
        let iterator = self
            .heap()
            .fresh_compiled_function(&ITERATOR_SELF, &[])
            .unwrap();
        let next = self
            .heap()
            .fresh_compiled_function(&ASYNC_NEXT, std::slice::from_ref(&state))
            .unwrap();
        self.heap()
            .define_own_data(
                &source,
                self.bindings.async_iterator_symbol(),
                iterator,
                self.factory.runtime(),
            )
            .unwrap();
        self.put(&source, "next", next);
        (source, state)
    }
    fn from_source(&self, source: ValueHandle, handler: Option<ValueHandle>) -> Arc<MockModel> {
        let options = self.heap().object().unwrap();
        self.put(&options, "responses", source);
        if let Some(handler) = handler {
            self.put(&options, "handler", handler);
        }
        self.create(options)
    }
    fn stream(
        &self,
        mock: &Arc<MockModel>,
        context: ValueHandle,
        raw: Option<ValueHandle>,
        options: CallOptions,
    ) -> ActualAssistantStream {
        mock.stream(&mock.model(), self.factory.clone(), context, raw, options)
    }
    fn drain_ready(&mut self) {
        while matches!(self.driver.drain_one().unwrap(), NativeDrain::RanJob { .. }) {}
    }
    async fn collect(&mut self, mut stream: ActualAssistantStream) -> Collected {
        let receipt = stream.failure_receipt();
        let mut events = Vec::new();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let wake = self.driver.wake(); let notified = wake.notified();
                tokio::pin!(notified); notified.as_mut().enable();
                self.drain_ready();
                tokio::select! {
                    event = stream.recv() => match event { Some(event) => events.push(event), None => break },
                    _ = &mut notified => {},
                }
            }
            receipt.wait_settled().await;
        }).await.expect("bounded original mock stream must settle with its sole driver");
        assert!(receipt.is_settled());
        let terminal = events.iter().rev().find_map(|event| match event {
            Event::Done { message, .. } => Some(message.clone()),
            Event::Error { error, .. } => Some(error.clone()),
            _ => None,
        });
        Collected {
            events,
            terminal,
            failures: receipt.failures(),
        }
    }
    async fn run(&mut self, mock: &Arc<MockModel>) -> Collected {
        let stream = self.stream(mock, self.context(), None, CallOptions::default());
        self.collect(stream).await
    }
    fn assert_plain(&self, actual: &ValueHandle, expected: &Value) {
        match expected {
            Value::Null => assert_eq!(self.heap().kind(actual).unwrap(), ValueKind::Null),
            Value::Bool(value) => assert_eq!(self.heap().truthy(actual).unwrap(), *value),
            Value::Number(value) => assert_eq!(self.n(actual), value.as_f64().unwrap()),
            Value::String(value) => assert_eq!(self.s(actual), *value),
            Value::Array(values) => {
                assert_eq!(self.len(actual) as usize, values.len());
                for (index, expected) in values.iter().enumerate() {
                    self.assert_plain(&self.get(actual, &index.to_string()), expected);
                }
            }
            Value::Object(values) => {
                let keys = self
                    .heap()
                    .own_enumerable_string_keys(actual, self.factory.runtime())
                    .unwrap();
                assert_eq!(keys.len(), values.len());
                for (key, expected) in values {
                    self.assert_plain(&self.get(actual, key), expected);
                }
            }
        }
    }
    fn assert_text(&self, result: &Collected, expected: &str) {
        self.assert_plain(
            &self.get(result.result(), "content"),
            &json!([{"type":"text","text":expected}]),
        );
    }
    fn rejected_message(&self, result: &Collected) -> String {
        assert!(
            result.terminal.is_none(),
            "stream.fail must not become terminal success/error"
        );
        assert_eq!(result.failures.len(), 1);
        let ValueError::Thrown(reason) = result.failures[0].as_ref() else {
            panic!(
                "native original Error identity required: {:?}",
                result.failures
            );
        };
        self.s(&self.get(reason, "message"))
    }
    fn tool_id(&self, result: &Collected) -> String {
        let content = self.get(result.result(), "content");
        for index in 0..self.len(&content) {
            let block = self.get(&content, &index.to_string());
            if self.s(&self.get(&block, "type")) == "toolCall" {
                return self.s(&self.get(&block, "id"));
            }
        }
        panic!("original toolCall expected")
    }
}

#[derive(Default)]
struct RegistryHost {
    registered: AtomicUsize,
}
impl ExtensionRegistryHost for RegistryHost {
    fn custom_api_registered(&self, _: &RuntimeApiRegistration) {
        self.registered.fetch_add(1, Ordering::SeqCst);
    }
    fn custom_api_removed(&self, _: &WireString) {}
    fn oauth_registered(&self, _: &RuntimeOAuthEntry) {
        panic!("mock registration must not register OAuth");
    }
    fn oauth_removed(&self, _: &WireString) {}
    fn set_runtime_usage(&self, _: &WireString, _: &OpaqueExtensionHandle, _: Option<&str>) {
        panic!("no mock usage side effect");
    }
    fn remove_runtime_usage(&self, _: &WireString) {}
    fn install_api_key(&self, _: &WireString, _: &str) -> ExtensionResult<()> {
        panic!("no mock key side effect");
    }
    fn remove_config_api_key(&self, _: &WireString) {}
    fn has_full_snapshot(&self) -> bool {
        true
    }
    fn ensure_full_snapshot(&self) -> ExtensionResult<()> {
        Ok(())
    }
    fn reload_static(&self, _: bool) -> ExtensionResult<()> {
        Ok(())
    }
    fn models_registered(
        &self,
        _: &WireString,
        _: &[CustomModelOverlay],
        _: Option<&ProviderOverride>,
    ) -> ExtensionResult<()> {
        panic!("no mock catalog mutation");
    }
    fn transport_applied(&self, _: &WireString, _: &ProviderOverride) -> ExtensionResult<()> {
        panic!("no mock transport mutation");
    }
    fn invalidate_provider(&self, _: &WireString) {}
    fn invalidate_all_provider_lookups(&self) {}
}

fn begin(id: &str) {
    eprintln!("original-mock25:begin:{id}");
}
fn pass(id: &str) {
    eprintln!("original-mock25:pass:{id}");
}

#[tokio::test]
async fn original_mock_provider25_graph_host_and_registry() {
    let mut h = Harness::new();
    // Original afterEach clearCustomApis: registry only exists in the one
    // registration case below and is dropped before the following case.

    begin("B-1421bc4b30");
    let mock = h.create(h.data(json!({"id":"spec-1","provider":"tests"})));
    assert_eq!(h.s(&h.get(mock.model_value(), "api")), MOCK_API);
    assert_eq!(h.s(&h.get(mock.model_value(), "id")), "spec-1");
    assert_eq!(h.s(&h.get(mock.model_value(), "provider")), "tests");
    assert!(is_mock_model(&mock.model()));
    pass("B-1421bc4b30");

    begin("B-e13b580d77");
    let mock = h.create(h.data(json!({"responses":[{"content":["hello world"]}]})));
    let result = h.run(&mock).await;
    h.assert_text(&result, "hello world");
    assert_eq!(h.s(&h.get(result.result(), "stopReason")), "stop");
    assert_eq!(
        h.n(&h.get(&h.get(result.result(), "usage"), "totalTokens")),
        0.0
    );
    pass("B-e13b580d77");

    begin("B-58b76de3aa");
    let mock = h.create(h.data(json!({"responses":[{"content":[{"type":"toolCall","name":"read","arguments":{"path":"/x"}}]}]})));
    let result = h.run(&mock).await;
    let ends: Vec<_> = result
        .events
        .iter()
        .filter_map(|event| match event {
            Event::ToolcallEnd { tool_call, .. } => Some(tool_call),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 1);
    assert_eq!(h.s(&h.get(ends[0], "name")), "read");
    h.assert_plain(&h.get(ends[0], "arguments"), &json!({"path":"/x"}));
    let id = h.s(&h.get(ends[0], "id"));
    let digits = id
        .strip_prefix("mock-tc-")
        .expect("original /^mock-tc-\\d+$/");
    assert!(!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()));
    assert_eq!(h.s(&h.get(result.result(), "stopReason")), "toolUse");
    pass("B-58b76de3aa");

    begin("B-4a03b72cb0");
    let mock =
        h.create(h.data(json!({"responses":[{"content":["first"]},{"content":["second"]}]})));
    let first = h.run(&mock).await;
    let second = h.run(&mock).await;
    h.assert_text(&first, "first");
    h.assert_text(&second, "second");
    assert_eq!(h.len(mock.calls_value()), 2);
    pass("B-4a03b72cb0");

    begin("B-a5af0efaad");
    let (handler, _) = h.handler("fallback");
    let options = h.data(json!({"responses":[{"content":["scripted"]}]}));
    h.put(&options, "handler", handler);
    let mock = h.create(options);
    h.run(&mock).await.result();
    let result = h.run(&mock).await;
    h.assert_text(&result, "fallback:1");
    pass("B-a5af0efaad");

    begin("B-6fc6e2032f");
    let (handler, state) = h.handler("options");
    let options = h.heap().object().unwrap();
    h.put(&options, "handler", handler);
    let mock = h.create(options);
    let context = h.context();
    let raw = h.data(json!({"apiKey":"k"}));
    let stream = h.stream(
        &mock,
        context.clone(),
        Some(raw.clone()),
        CallOptions::default(),
    );
    let result = h.collect(stream).await;
    h.assert_text(&result, "msgs=1 tier=k");
    let seen = h.get(&h.get(&state, "trace"), "0");
    assert!(h.get(&seen, "context").same_identity(&context));
    assert!(h.get(&seen, "options").same_identity(&raw));
    let call = h.get(mock.calls_value(), "0");
    assert!(h.get(&call, "context").same_identity(&context));
    assert!(h.get(&call, "options").same_identity(&raw));
    pass("B-6fc6e2032f");

    begin("B-f571d73ed1");
    let mock = h.create(h.data(json!({"responses":[{"throw":"boom"}]})));
    let result = h.run(&mock).await;
    assert_eq!(h.s(&h.get(result.result(), "stopReason")), "error");
    assert_eq!(h.s(&h.get(result.result(), "errorMessage")), "boom");
    pass("B-f571d73ed1");

    begin("B-42f4592e0a");
    let mock = h.create(h.data(json!({})));
    let result = h.run(&mock).await;
    assert!(
        h.rejected_message(&result)
            .contains("no response or handler is configured")
    );
    pass("B-42f4592e0a");

    begin("B-28481d95c7");
    let mock = h.create(h.data(json!({})));
    mock.push(MockHandler::Response(
        h.data(json!({"content":["late binding"]})),
    ));
    let result = h.run(&mock).await;
    h.assert_text(&result, "late binding");
    pass("B-28481d95c7");

    begin("B-96b64cd879");
    let mock = h.create(h.data(json!({"responses":[{"content":["a"]},{"content":["b"]}]})));
    mock.push(MockHandler::Response(
        h.data(json!({"content":["pushed-pre-reset"]})),
    ));
    h.run(&mock).await.result();
    mock.reset(&h.factory).unwrap();
    assert_eq!(h.len(mock.calls_value()), 0);
    let next = h.run(&mock).await;
    h.assert_text(&next, "b");
    mock.push(MockHandler::Response(
        h.data(json!({"content":["post-reset"]})),
    ));
    let last = h.run(&mock).await;
    h.assert_text(&last, "post-reset");
    pass("B-96b64cd879");

    begin("B-5545f26729");
    {
        let registry = ExtensionRegistry::new();
        let host = RegistryHost::default();
        assert!(registered_mock_api(&registry).unwrap().is_none());
        register_mock_api(&RegistryMockApiRegistrar::new(&registry, &host), None).unwrap();
        let entry = registered_mock_api(&registry)
            .unwrap()
            .expect("original registry entry is defined");
        assert!(std::ptr::fn_addr_eq(
            entry.stream_simple,
            stream_mock as ara_ai::providers::mock::MockStreamFn
        ));
        assert_eq!(host.registered.load(Ordering::SeqCst), 1);
        // Required Host coverage: call both actual functions retrieved from
        // the real registry row, not a shadow test table or no-op registrar.
        let mock = h.create(h.data(json!({"responses":[{"content":["registered wrapper"]},{"content":["registered simple"]}]})));
        let first = (entry.stream)(
            &mock.model(),
            h.factory.clone(),
            h.context(),
            None,
            CallOptions::default(),
        );
        let first = h.collect(first).await;
        h.assert_text(&first, "registered wrapper");
        let second = (entry.stream_simple)(
            &mock.model(),
            h.factory.clone(),
            h.context(),
            None,
            CallOptions::default(),
        );
        let second = h.collect(second).await;
        h.assert_text(&second, "registered simple");
    }
    pass("B-5545f26729");

    begin("B-01811461c2");
    let mock = h.create(h.data(json!({"responses":[{"content":["never"],"delayMs":1000}]})));
    let controller = h.bindings.abort_controller(&h.factory).unwrap();
    let options = CallOptions {
        cancel: controller.cancel(),
        ..Default::default()
    };
    // Actual Host owns one graph AbortSignal and its matching cancellation
    // token. The original raw signal is read by the production delay branch.
    let raw = h.heap().object().unwrap();
    h.put(&raw, "signal", controller.signal().clone());
    let stream = h.stream(&mock, h.context(), Some(raw.clone()), options);
    controller.abort(&h.factory).unwrap();
    let result = h.collect(stream).await;
    assert_eq!(h.s(&h.get(result.result(), "stopReason")), "aborted");
    assert!(
        h.s(&h.get(result.result(), "errorMessage"))
            .to_ascii_lowercase()
            .contains("aborted")
    );
    assert!(
        h.get(&h.get(mock.calls_value(), "0"), "options")
            .same_identity(&raw)
    );
    pass("B-01811461c2");

    begin("B-270c2c8c01");
    let foreign =
        MockModelHandle::Foreign(h.data(json!({"api":MOCK_API,"id":"x"})), h.bindings.clone());
    let result = h
        .collect(stream_mock(
            &foreign,
            h.factory.clone(),
            h.context(),
            None,
            CallOptions::default(),
        ))
        .await;
    assert!(
        h.rejected_message(&result)
            .contains("not produced by createMockModel")
    );
    pass("B-270c2c8c01");

    begin("B-68c8fedb1d");
    let mock = h.create(h.data(json!({"id":"spec","provider":"tests","responses":[{"content":["x"],"usage":{"input":5,"output":2}}]})));
    let result = h.run(&mock).await;
    assert_eq!(h.s(&h.get(result.result(), "api")), MOCK_API);
    assert_eq!(h.s(&h.get(result.result(), "provider")), "tests");
    assert_eq!(h.s(&h.get(result.result(), "model")), "spec");
    assert_eq!(
        h.heap().kind(&h.get(result.result(), "timestamp")).unwrap(),
        ValueKind::Number
    );
    assert_eq!(h.n(&h.get(&h.get(result.result(), "usage"), "input")), 5.0);
    assert_eq!(h.n(&h.get(&h.get(result.result(), "usage"), "output")), 2.0);
    pass("B-68c8fedb1d");

    begin("B-35ff2dccc9");
    let mock =
        h.create(h.data(json!({"responses":[{"content":["x"],"usage":{"input":5,"output":2}}]})));
    let result = h.run(&mock).await;
    assert_eq!(
        h.n(&h.get(&h.get(result.result(), "usage"), "totalTokens")),
        7.0
    );
    pass("B-35ff2dccc9");

    begin("B-d3f05b7c22");
    let mock = h.create(h.data(
        json!({"responses":[{"content":["x"],"usage":{"input":5,"output":2,"totalTokens":999}}]}),
    ));
    let result = h.run(&mock).await;
    assert_eq!(
        h.n(&h.get(&h.get(result.result(), "usage"), "totalTokens")),
        999.0
    );
    pass("B-d3f05b7c22");

    begin("B-0d3954ac5a");
    let mock = h.create(h.data(json!({"responses":[{"content":["x"],"usage":{"input":5,"output":2,"cost":{"input":0.5,"output":0.25,"cacheRead":0,"cacheWrite":0}}}]})));
    let result = h.run(&mock).await;
    let total = h.n(&h.get(&h.get(&h.get(result.result(), "usage"), "cost"), "total"));
    assert!(
        (total - 0.75).abs() < 0.5e-10,
        "original toBeCloseTo(0.75,10)"
    );
    pass("B-0d3954ac5a");

    begin("B-9581fd8628");
    let make_mock = |h: &Harness| {
        h.create(h.data(json!({"responses":[
        {"content":[{"type":"toolCall","name":"read","arguments":{"path":"/x"}}]},
        {"content":[{"type":"toolCall","name":"read","arguments":{"path":"/y"}}]}]})))
    };
    let a = make_mock(&h);
    let b = make_mock(&h);
    let a1 = h.run(&a).await;
    let a2 = h.run(&a).await;
    let b1 = h.run(&b).await;
    let b2 = h.run(&b).await;
    assert_eq!(h.tool_id(&a1), "mock-tc-1");
    assert_eq!(h.tool_id(&a2), "mock-tc-2");
    assert_eq!(h.tool_id(&b1), "mock-tc-1");
    assert_eq!(h.tool_id(&b2), "mock-tc-2");
    a.reset(&h.factory).unwrap();
    a.push(MockHandler::Response(h.data(
        json!({"content":[{"type":"toolCall","name":"read","arguments":{"path":"/z"}}]}),
    )));
    let a3 = h.run(&a).await;
    assert_eq!(h.tool_id(&a3), "mock-tc-1");
    pass("B-9581fd8628");

    begin("B-4db2155825");
    let (source, _) = h.async_source(json!([{"content":["a"]},{"content":["b"]}]), "plain");
    let mock = h.from_source(source, None);
    let a = h.run(&mock).await;
    let b = h.run(&mock).await;
    h.assert_text(&a, "a");
    h.assert_text(&b, "b");
    pass("B-4db2155825");

    begin("B-2e13007167");
    let gate = h
        .driver
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())
        .unwrap();
    let (source, state) = h.async_source(
        json!([{"content":["before"]},{"content":["after"]}]),
        "gate",
    );
    h.put(&state, "gate", gate.promise.clone());
    let mock = h.from_source(source, None);
    let first = h.run(&mock).await;
    h.assert_text(&first, "before");
    let mut second = h.stream(&mock, h.context(), None, CallOptions::default());
    let receipt = second.failure_receipt();
    // The original two Promise.resolve microtask opportunities map to two
    // executor turns with sole-driver graph job drainage. No elapsed delay.
    for _ in 0..2 {
        tokio::task::yield_now().await;
        h.drain_ready();
    }
    assert_eq!(h.n(&h.get(&state, "index")), 2.0);
    assert_eq!(h.len(mock.calls_value()), 2);
    assert!(second.try_recv().is_err());
    assert!(receipt.known_terminals().is_empty());
    assert!(!receipt.is_settled());
    h.heap()
        .call(
            &gate.resolve,
            &h.heap().undefined().unwrap(),
            &[h.heap().undefined().unwrap()],
            h.factory.runtime(),
        )
        .unwrap();
    let second = h.collect(second).await;
    h.assert_text(&second, "after");
    pass("B-2e13007167");

    begin("B-3de5c1bcb2");
    let (source, state) = h.async_source(json!([{"content":["turn 0"]},{"content":[]}]), "calls");
    let mock = h.from_source(source, None);
    h.put(&state, "calls", mock.calls_value().clone());
    let r1 = h.run(&mock).await;
    h.assert_text(&r1, "turn 0");
    let context = h.data(json!({"systemPrompt":[],"messages":[{"role":"user","content":"a","timestamp":0},{"role":"user","content":"b","timestamp":0}]}));
    let r2 = h
        .collect(h.stream(&mock, context, None, CallOptions::default()))
        .await;
    h.assert_text(&r2, "saw 2 message(s)");
    pass("B-3de5c1bcb2");

    begin("B-81c07ed260");
    let set_constructor = h.get(&h.realm.globals.environment, "Set");
    let set = h
        .heap()
        .construct(
            &set_constructor,
            &[h.data(json!([{"content":["x"]},{"content":["y"]}]))],
            h.factory.runtime(),
        )
        .unwrap();
    let mock = h.from_source(set, None);
    let a = h.run(&mock).await;
    let b = h.run(&mock).await;
    h.assert_text(&a, "x");
    h.assert_text(&b, "y");
    pass("B-81c07ed260");

    begin("B-343a5fe3e6");
    let (source, _) = h.async_source(json!([{"content":["scripted"]}]), "plain");
    let (handler, _) = h.handler("constant");
    let mock = h.from_source(source, Some(handler));
    h.run(&mock).await.result();
    let next = h.run(&mock).await;
    h.assert_text(&next, "fallback");
    pass("B-343a5fe3e6");

    begin("B-98eaaa0cc3");
    let (source, _) = h.async_source(json!([{"content":["scripted"]}]), "plain");
    let (handler, _) = h.handler("constant");
    let mock = h.from_source(source, Some(handler));
    mock.push(MockHandler::Response(h.data(json!({"content":["pushed"]}))));
    let a = h.run(&mock).await;
    let b = h.run(&mock).await;
    let c = h.run(&mock).await;
    h.assert_text(&a, "scripted");
    h.assert_text(&b, "pushed");
    h.assert_text(&c, "fallback");
    pass("B-98eaaa0cc3");

    begin("B-74ea6c2201");
    let (source, state) = h.async_source(json!([{"content":["ok"]}]), "throw");
    let thrown = h
        .heap()
        .construct(
            &h.realm.globals.errors.error,
            &[h.heap().string("script crashed".into()).unwrap()],
            h.factory.runtime(),
        )
        .unwrap();
    h.put(&state, "error", thrown.clone());
    let mock = h.from_source(source, None);
    h.run(&mock).await.result();
    let result = h.run(&mock).await;
    assert!(h.rejected_message(&result).contains("script crashed"));
    let ValueError::Thrown(reason) = result.failures[0].as_ref() else {
        unreachable!()
    };
    assert!(reason.same_identity(&thrown));
    pass("B-74ea6c2201");
    eprintln!("original-mock25:complete:25");
}

// OMP b91db66e mock.ts terminal metadata additions. Keep the original25 above.
#[tokio::test]
async fn upgraded_mock_terminal_metadata_preserves_native_identity_and_response_scope() {
    let mut h = Harness::new();
    for reason in ["stop", "error", "aborted"] {
        let options = h.data(json!({"responses":[
            {"content":["metadata"],"stopReason":reason,"serviceTier":"priority"},
            {"content":["next response"]}
        ]}));
        let response = h.get(&h.get(&options, "responses"), "0");
        let credit = h.heap().object().unwrap();
        h.put(&credit, "opaque", h.heap().string("local fixture".into()).unwrap());
        h.put(&response, "fallbackCreditHandle", credit.clone());
        let mock = h.create(options);
        let result = h.run(&mock).await;
        let terminal = result.result();
        assert_eq!(h.s(&h.get(terminal, "stopReason")), reason);
        assert_eq!(h.s(&h.get(terminal, "serviceTier")), "priority");
        assert!(h.get(terminal, "fallbackCreditHandle").same_identity(&credit));
        assert!(result.failures.is_empty());
        assert!(matches!(result.events.last(), Some(Event::Done { .. })) == (reason == "stop"));
        if reason != "stop" {
            assert!(matches!(result.events.last(), Some(Event::Error { .. })));
        }
        // A following response from the same model cannot retain prior metadata.
        let next = h.run(&mock).await;
        h.assert_text(&next, "next response");
        for field in ["fallbackCreditHandle", "serviceTier"] {
            assert_eq!(h.heap().kind(&h.get(next.result(), field)).unwrap(), ValueKind::Undefined);
        }
    }
    eprintln!("mock-upgrade-b91db66e:terminal-metadata:pass");
}


// Complete b91db66e Mock delta: direct native Gets retain order, abrupt
// completion and the preexisting throw/delay early-exit boundaries.
static UPGRADED_METADATA_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP b91db66e mock.ts terminal metadata assignments and early guards",
    name: "get upgraded metadata",
    length: 0,
    capture_count: 5,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, receiver, _, rt| {
        if !receiver.same_identity(&captures[4]) {
            return Err(ValueError::TypeError("metadata receiver changed".into()));
        }
        heap.array_push(&captures[0], &[captures[1].clone()], rt)?;
        if heap.kind(&captures[3])? != ValueKind::Undefined {
            return Err(ValueError::Thrown(captures[3].clone()));
        }
        Ok(captures[2].clone())
    },
};

#[tokio::test]
async fn upgraded_mock_metadata_getter_order_abrupt_and_early_exit_group() {
    const KEYS: [&str; 5] = [
        "stopDetails", "fallbackCreditHandle", "errorMessage", "usage", "serviceTier",
    ];
    for scenario in ["order", "credit-throw", "tier-throw", "response-throw", "delay-abort"] {
        let mut h = Harness::new();
        let options = h.data(json!({"responses":[{"content":["metadata getter"]}]}));
        let response = h.get(&h.get(&options, "responses"), "0");
        let trace = h.data(json!([]));
        let credit = h.heap().object().unwrap();
        let thrown = h.heap().construct(
            &h.realm.globals.errors.error,
            &[h.heap().string(format!("native {scenario}").into()).unwrap()],
            h.factory.runtime(),
        ).unwrap();
        let values = [
            h.heap().string("details".into()).unwrap(),
            credit.clone(),
            h.heap().string("metadata message".into()).unwrap(),
            h.data(json!({"input":7,"totalTokens":7})),
            h.heap().string("priority".into()).unwrap(),
        ];
        for (index, key) in KEYS.iter().enumerate() {
            let forbidden = matches!(scenario, "response-throw" | "delay-abort")
                || (scenario == "credit-throw" && *key == "fallbackCreditHandle")
                || (scenario == "tier-throw" && *key == "serviceTier");
            let getter = h.heap().fresh_compiled_function(
                &UPGRADED_METADATA_GETTER,
                &[
                    trace.clone(), h.heap().string((*key).into()).unwrap(),
                    values[index].clone(),
                    if forbidden { thrown.clone() } else { h.heap().undefined().unwrap() },
                    response.clone(),
                ],
            ).unwrap();
            let ValueSnapshot::Function(target) = h.heap().snapshot(&getter).unwrap() else {
                panic!("native metadata getter callable required");
            };
            h.heap().define_own(
                &response, (*key).into(),
                ara_rpc::value_heap::PropertyDescriptor::Accessor {
                    get: Some(target), set: None, enumerable: false, configurable: true,
                },
                h.factory.runtime(),
            ).unwrap();
        }
        if scenario == "response-throw" {
            h.put(&response, "throw", h.heap().string("early throw".into()).unwrap());
        }
        if scenario == "delay-abort" {
            h.put(&response, "delayMs", h.heap().number(1000.0).unwrap());
        }
        let mock = h.create(options);
        let result = if scenario == "delay-abort" {
            let controller = h.bindings.abort_controller(&h.factory).unwrap();
            let raw = h.heap().object().unwrap();
            h.put(&raw, "signal", controller.signal().clone());
            let stream = h.stream(&mock, h.context(), Some(raw), CallOptions {
                cancel: controller.cancel(), ..Default::default()
            });
            // Same original pre-aborted-delay boundary; no timer race/sleep.
            controller.abort(&h.factory).unwrap();
            h.collect(stream).await
        } else {
            h.run(&mock).await
        };
        let observed: Vec<_> = (0..h.len(&trace))
            .map(|index| h.s(&h.get(&trace, &index.to_string())))
            .collect();
        let expected: &[&str] = match scenario {
            "order" | "tier-throw" => &KEYS,
            "credit-throw" => &KEYS[..2],
            _ => &[],
        };
        assert_eq!(observed, expected, "once-only getter order/cutoff: {scenario}");
        if matches!(scenario, "credit-throw" | "tier-throw") {
            assert!(result.terminal.is_none(), "native Get abrupt must not synthesize terminal");
            assert_eq!(result.failures.len(), 1);
            let ValueError::Thrown(reason) = result.failures[0].as_ref() else {
                panic!("original native thrown identity required");
            };
            assert!(reason.same_identity(&thrown));
            assert!(!result.events.is_empty(), "partial Start remains observable");
            assert!(result.events.iter().all(|event| !matches!(event, Event::Done { .. } | Event::Error { .. })));
            let partial = result.events[0].partial();
            assert!(result.events.iter().all(|event| event.partial().same_identity(partial)));
            assert_eq!(h.s(&h.get(partial, "stopDetails")), "details");
            assert_eq!(h.heap().kind(&h.get(partial, "duration")).unwrap(), ValueKind::Undefined);
            assert_eq!(h.heap().kind(&h.get(partial, "serviceTier")).unwrap(), ValueKind::Undefined);
            if scenario == "credit-throw" {
                assert_eq!(h.heap().kind(&h.get(partial, "fallbackCreditHandle")).unwrap(), ValueKind::Undefined);
                assert_eq!(h.heap().kind(&h.get(partial, "errorMessage")).unwrap(), ValueKind::Undefined);
                assert_eq!(h.n(&h.get(&h.get(partial, "usage"), "input")), 0.0);
            } else {
                assert!(h.get(partial, "fallbackCreditHandle").same_identity(&credit));
                assert_eq!(h.s(&h.get(partial, "errorMessage")), "metadata message");
                assert_eq!(h.n(&h.get(&h.get(partial, "usage"), "input")), 7.0);
                assert_eq!(h.n(&h.get(&h.get(partial, "usage"), "totalTokens")), 7.0);
            }
        } else {
            assert!(result.failures.is_empty());
            let terminal = result.result();
            if scenario == "order" {
                assert!(matches!(result.events.last(), Some(Event::Done { .. })));
                assert_eq!(h.s(&h.get(terminal, "stopDetails")), "details");
                assert!(h.get(terminal, "fallbackCreditHandle").same_identity(&credit));
                assert_eq!(h.s(&h.get(terminal, "serviceTier")), "priority");
                assert_eq!(h.n(&h.get(&h.get(terminal, "usage"), "input")), 7.0);
                assert_eq!(h.heap().kind(&h.get(terminal, "duration")).unwrap(), ValueKind::Number);
            } else {
                assert!(matches!(result.events.last(), Some(Event::Error { .. })));
                let expected_reason = if scenario == "response-throw" { "error" } else { "aborted" };
                assert_eq!(h.s(&h.get(terminal, "stopReason")), expected_reason);
                let message = h.s(&h.get(terminal, "errorMessage"));
                if scenario == "response-throw" {
                    assert_eq!(message, "early throw");
                } else {
                    assert!(message.to_ascii_lowercase().contains("aborted"));
                }
                for key in ["fallbackCreditHandle", "serviceTier"] {
                    assert_eq!(h.heap().kind(&h.get(terminal, key)).unwrap(), ValueKind::Undefined);
                }
            }
        }
        eprintln!("mock-upgrade-b91db66e:metadata-getter:{scenario}:pass");
    }
    eprintln!("mock-upgrade-b91db66e:metadata-getter:complete:5");
}
