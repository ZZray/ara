//! Source-body observations beyond the original 25 tests. Same actual graph,
//! module imports, registration owner and sole Native driver as the originals.
use super::*;
use ara_ai::{GraphProviderContext, Model, ModelProvider};
use ara_rpc::value_heap::PropertyDescriptor;

static COST_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts468 costProvided false skips the final nonenumerable partial.cost getter",
    name: "get cost",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, _, rt| {
        let state = &captures[0];
        let count = heap.to_number(&get(heap, state, "gets", rt)?, rt)?;
        heap.assign(state, "gets".into(), heap.number(count + 1.0)?, rt)?;
        if count != 0.0 {
            return Err(ValueError::TypeError(
                "cost getter read after false costProvided".into(),
            ));
        }
        heap.undefined()
    },
};
static ADOPTION_THEN: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts283 original thenable resolves its original iterator result",
    name: "then",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, args, rt| {
        let resolve = args.first().cloned().unwrap_or(heap.undefined()?);
        heap.call(&resolve, &heap.undefined()?, &[captures[0].clone()], rt)
    },
};
static ADOPTION_GETTER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts283 Promise.resolve synchronously Gets then before second stream next",
    name: "get then",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, _, rt| {
        let state = &captures[0];
        let count = heap.to_number(&get(heap, state, "thenGets", rt)?, rt)?;
        heap.assign(state, "thenGets".into(), heap.number(count + 1.0)?, rt)?;
        heap.assign(state, "phase".into(), heap.number(1.0)?, rt)?;
        get(heap, state, "thenFunction", rt)
    },
};
static ADOPTION_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts283 overlapping next observes the first result then getter immediately",
    name: "next",
    length: 0,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, _, _, rt| {
        let state = &captures[0];
        let index = heap.to_number(&get(heap, state, "index", rt)?, rt)?;
        heap.assign(state, "index".into(), heap.number(index + 1.0)?, rt)?;
        if index == 0.0 {
            return get(heap, state, "thenable", rt);
        }
        let phase = get(heap, state, "phase", rt)?;
        heap.assign(state, "observedPhase".into(), phase.clone(), rt)?;
        obj(
            heap,
            rt,
            &[
                ("done", heap.boolean(false)?),
                (
                    "value",
                    text_response(heap, rt, format!("phase={}", heap.to_number(&phase, rt)?))?,
                ),
            ],
        )
    },
};
fn accessor(h: &Harness, object: &ValueHandle, key: &str, getter: ValueHandle) {
    let ValueSnapshot::Function(target) = h.heap().snapshot(&getter).unwrap() else {
        panic!("native getter callable expected");
    };
    h.heap()
        .define_own(
            object,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(target),
                set: None,
                enumerable: false,
                configurable: true,
            },
            h.factory.runtime(),
        )
        .unwrap();
}

static BAD_SYNC: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts iteratorOf async protocol has priority even when sync protocol exists",
    name: "[Symbol.iterator]",
    length: 0,
    capture_count: 0,
    form: CompiledFunctionForm::Ordinary,
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "async source incorrectly selected sync iterator".into(),
        ))
    },
};
static ON_RESPONSE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts312-334 onResponse await preserves receiver, metadata and original model",
    name: "onResponse",
    length: 2,
    capture_count: 1,
    form: CompiledFunctionForm::Ordinary,
    body: |heap, captures, receiver, args, rt| {
        let state = &captures[0];
        if !receiver.same_identity(&get(heap, state, "options", rt)?) {
            return Err(ValueError::TypeError(
                "original options receiver lost".into(),
            ));
        }
        let model = args.get(1).cloned().unwrap_or(heap.undefined()?);
        if !model.same_identity(&get(heap, state, "model", rt)?) {
            return Err(ValueError::TypeError(
                "original MockModel metadata handle lost".into(),
            ));
        }
        let metadata = args.first().cloned().unwrap_or(heap.undefined()?);
        heap.assign(state, "metadata".into(), metadata, rt)?;
        let count = heap.to_number(&get(heap, state, "called", rt)?, rt)?;
        heap.assign(state, "called".into(), heap.number(count + 1.0)?, rt)?;
        get(heap, state, "gate", rt)
    },
};
static CONCURRENT_NEXT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "mock.ts pullHandler awaits each next but does not serialize overlapping custom iterator next calls",
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
        let value = text_response(
            heap,
            rt,
            if index == 0 { "first" } else { "second" }.to_owned(),
        )?;
        let result = obj(
            heap,
            rt,
            &[("done", heap.boolean(index > 1)?), ("value", value)],
        )?;
        if index == 0 {
            let gate = get(heap, state, "gate", rt)?;
            let then = get(heap, &gate, "then", rt)?;
            let resume = heap.fresh_compiled_function(&RETURN_YIELD, &[result])?;
            return heap.call(&then, &gate, &[resume], rt);
        }
        let resolve = get(heap, state, "release", rt)?;
        heap.call(&resolve, &heap.undefined()?, &[heap.undefined()?], rt)?;
        let promise = get(heap, state, "Promise", rt)?;
        let resolve = get(heap, &promise, "resolve", rt)?;
        heap.call(&resolve, &promise, &[result], rt)
    },
};
async fn callback_entered(h: &mut Harness, state: &ValueHandle) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            h.drain_ready();
            if h.n(&h.get(state, "called")) > 0.0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("actual callback must enter before observing the gate");
}
fn pending(h: &Harness) -> ara_cli::native_realm::promise::NativePendingPromise {
    h.driver
        .prepare_pending(h.driver.root_scope(), h.driver.root_parent())
        .unwrap()
}
fn headers_mock(h: &Harness, gate: ValueHandle) -> (Arc<MockModel>, ValueHandle, ValueHandle) {
    let mock = h.create(h.data(json!({"id":"headers-spec","responses":[{
        "content":["after metadata"],"responseHeaders":{"X-Request-Id":"header-id","MiXeD":"value"},
        "responseStatus":201,"responseRequestId":"req-explicit"
    }]})));
    let options = h.data(json!({}));
    let state = h.data(json!({"called":0}));
    h.put(&state, "options", options.clone());
    h.put(&state, "model", mock.model_value().clone());
    h.put(&state, "gate", gate);
    let callback = h
        .heap()
        .fresh_compiled_function(&ON_RESPONSE, std::slice::from_ref(&state))
        .unwrap();
    h.put(&options, "onResponse", callback);
    (mock, options, state)
}

#[tokio::test]
async fn mock_source_content_usage_and_actual_provider_entry() {
    let mut h = Harness::new();
    let a = h.create(h.data(json!({})));
    let b = h.create(h.data(json!({})));
    assert!(
        h.get(a.model_value(), "cost")
            .same_identity(&h.get(b.model_value(), "cost"))
    );
    for (key, expected) in [
        ("id", json!("mock-model")),
        ("name", json!("mock-model")),
        ("api", json!("mock")),
        ("provider", json!("mock")),
        ("baseUrl", json!("mock://")),
        ("reasoning", json!(false)),
        ("input", json!(["text"])),
        ("contextWindow", json!(200_000)),
        ("maxTokens", json!(32_768)),
    ] {
        h.assert_plain(&h.get(a.model_value(), key), &expected);
    }
    assert_eq!(
        h.heap().kind(&h.get(a.model_value(), "compat")).unwrap(),
        ValueKind::Undefined
    );
    let thinking = h.data(json!({"type":"thinking","thinking":"why","thinkingSignature":"sig"}));
    let text = h.data(json!({"type":"text","text":"text object"}));
    let args = h.data(json!({"nested":{"value":1}}));
    let tool = h.data(json!({"type":"toolCall","id":"explicit-id","name":"read"}));
    h.put(&tool, "arguments", args.clone());
    let raw = h.data(json!({"type":"toolCall","name":"raw","arguments":"{not-json"}));
    let response = h.data(json!({"usage":{"input":3,"output":2,"cacheRead":1,"cacheWrite":4,
        "orchestration":{"input":7,"output":8,"cacheRead":9},"cost":{"input":0.1,"output":0.2,"cacheRead":0.3,"cacheWrite":0.4}}}));
    h.put(
        &response,
        "content",
        h.heap()
            .array(vec![thinking.clone(), text.clone(), tool, raw])
            .unwrap(),
    );
    let options = h.data(json!({}));
    h.put(
        &options,
        "responses",
        h.heap().array(vec![response]).unwrap(),
    );
    let mock = h.create(options);
    let result = h.run(&mock).await;
    let message = result.result();
    assert!(
        result
            .events
            .iter()
            .all(|e| e.partial().same_identity(message))
    );
    assert_eq!(
        result
            .events
            .iter()
            .map(|e| e.type_name())
            .collect::<Vec<_>>(),
        vec![
            "start",
            "thinking_start",
            "thinking_delta",
            "thinking_end",
            "text_start",
            "text_delta",
            "text_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "toolcall_start",
            "toolcall_delta",
            "toolcall_end",
            "done"
        ]
    );
    let content = h.get(message, "content");
    assert!(h.get(&content, "0").same_identity(&thinking));
    assert!(h.get(&content, "1").same_identity(&text));
    let normalized_args = h.get(&h.get(&content, "2"), "arguments");
    assert!(!normalized_args.same_identity(&args));
    assert!(
        h.get(&normalized_args, "nested")
            .same_identity(&h.get(&args, "nested"))
    );
    assert_eq!(h.s(&h.get(&h.get(&content, "2"), "id")), "explicit-id");
    let raw_block = h.get(&content, "3");
    assert_eq!(h.s(&h.get(&raw_block, "id")), "mock-tc-1");
    assert_eq!(h.s(&h.get(&raw_block, "arguments")), "{not-json");
    assert!(
        matches!(&result.events[11], Event::ToolcallDelta{delta,..} if h.s(delta)=="{not-json")
    );
    let usage = h.get(message, "usage");
    assert_eq!(h.n(&h.get(&usage, "totalTokens")), 34.0);
    assert!((h.n(&h.get(&h.get(&usage, "cost"), "total")) - 1.0).abs() < 1e-12);
    assert!(h.n(&h.get(message, "duration")) >= 0.0);
    let mock = h.create(h.data(json!({"responses":[{"content":["actual port"]}]})));
    let original = h.context();
    let context = GraphProviderContext {
        original_context: Some(original.clone()),
        system_prompt: vec![],
        messages: vec![],
        tools: None,
        history_demotion_dialect: None,
        history_id_hash: None,
        history_id_hash_function: None,
    };
    let endpoint = Model {
        id: "ignored foreign id".into(),
        api: "mock".into(),
        provider: "foreign".into(),
        base_url: "mock://".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let stream = ModelProvider::stream_actual(
        mock.as_ref(),
        &endpoint,
        &context,
        h.factory.clone(),
        CallOptions::default(),
    )
    .unwrap();
    let result = h.collect(stream).await;
    h.assert_text(&result, "actual port");
    assert!(
        mock.calls(&h.factory).unwrap()[0]
            .context
            .same_identity(&original)
    );
    let mut typed = ModelProvider::stream(
        mock.as_ref(),
        &endpoint,
        &ara_ai::Context::default(),
        CallOptions::default(),
    );
    let event = tokio::time::timeout(Duration::from_secs(1), typed.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        event,
        Event::Error {
            reason: ara_ai::StopReason::Error,
            ..
        }
    ));
    assert!(typed.recv().await.is_none());
    // A nonenumerable getter is skipped by spread and must be explicitly read
    // just once when costProvided is false, including after totals recompute.
    let cost_state = h.data(json!({"gets":0}));
    let usage = h.data(json!({"input":7}));
    let getter = h
        .heap()
        .fresh_compiled_function(&COST_GETTER, std::slice::from_ref(&cost_state))
        .unwrap();
    accessor(&h, &usage, "cost", getter);
    let response = h.data(json!({"content":["cost short circuit"]}));
    h.put(&response, "usage", usage);
    let options = h.data(json!({}));
    h.put(
        &options,
        "responses",
        h.heap().array(vec![response]).unwrap(),
    );
    let mock = h.create(options);
    let result = h.run(&mock).await;
    h.assert_text(&result, "cost short circuit");
    assert_eq!(h.n(&h.get(&cost_state, "gets")), 1.0);
    let usage = h.get(result.result(), "usage");
    assert_eq!(h.n(&h.get(&usage, "totalTokens")), 7.0);
    h.assert_plain(
        &h.get(&usage, "cost"),
        &json!({"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"total":0}),
    );
    eprintln!("mock-source:cost-provided-short-circuit:pass");
    eprintln!("mock-source:content-usage-actual-entry:pass");
}

#[tokio::test]
async fn mock_source_metadata_await_rejection_and_terminal_errors() {
    let mut h = Harness::new();
    let gate = pending(&h);
    let (mock, options, state) = headers_mock(&h, gate.promise.clone());
    let mut stream = h.stream(&mock, h.context(), Some(options), CallOptions::default());
    callback_entered(&mut h, &state).await;
    assert!(matches!(
        stream.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let metadata = h.get(&state, "metadata");
    assert_eq!(h.n(&h.get(&metadata, "status")), 201.0);
    h.assert_plain(
        &h.get(&metadata, "headers"),
        &json!({"x-request-id":"header-id","mixed":"value"}),
    );
    assert_eq!(h.s(&h.get(&metadata, "requestId")), "req-explicit");
    h.heap()
        .call(
            &gate.resolve,
            &h.heap().undefined().unwrap(),
            &[h.heap().undefined().unwrap()],
            h.factory.runtime(),
        )
        .unwrap();
    let result = h.collect(stream).await;
    h.assert_text(&result, "after metadata");
    assert_eq!(h.n(&h.get(&state, "called")), 1.0);
    let gate = pending(&h);
    let reason = h
        .heap()
        .construct(
            &h.realm.error_constructor(),
            &[h.data(json!("metadata crashed"))],
            h.factory.runtime(),
        )
        .unwrap();
    let (mock, options, state) = headers_mock(&h, gate.promise.clone());
    let stream = h.stream(&mock, h.context(), Some(options), CallOptions::default());
    callback_entered(&mut h, &state).await;
    h.heap()
        .call(
            &gate.reject,
            &h.heap().undefined().unwrap(),
            std::slice::from_ref(&reason),
            h.factory.runtime(),
        )
        .unwrap();
    let result = h.collect(stream).await;
    assert!(result.events.is_empty());
    assert!(
        matches!(result.failures[0].as_ref(),ValueError::Thrown(actual) if actual.same_identity(&reason))
    );
    let response = h.data(json!({}));
    h.put(&response, "throw", reason);
    let opts = h.data(json!({}));
    h.put(&opts, "responses", h.heap().array(vec![response]).unwrap());
    let mock = h.create(opts);
    let result = h.run(&mock).await;
    assert_eq!(
        result
            .events
            .iter()
            .map(|e| e.type_name())
            .collect::<Vec<_>>(),
        vec!["start", "error"]
    );
    assert_eq!(
        h.s(&h.get(result.result(), "errorMessage")),
        "metadata crashed"
    );
    assert!(
        result.events[0]
            .partial()
            .same_identity(result.events[1].partial())
    );
    let mock=h.create(h.data(json!({"responses":[{"content":["blocked"],"stopReason":"error","stopDetails":{"kind":"refusal"},"errorMessage":""}]})));
    let result = h.run(&mock).await;
    let error = result.result();
    assert!(!result.events[0].partial().same_identity(error));
    assert!(
        h.get(result.events[0].partial(), "content")
            .same_identity(&h.get(error, "content"))
    );
    assert_eq!(h.s(&h.get(error, "errorMessage")), "");
    h.assert_plain(&h.get(error, "stopDetails"), &json!({"kind":"refusal"}));
    let mock = h.create(h.data(json!({"responses":[{"stopReason":"aborted"}]})));
    let result = h.run(&mock).await;
    assert_eq!(h.s(&h.get(result.result(), "errorMessage")), "mock error");
    eprintln!("mock-source:metadata-await-original-reject-terminal-errors:pass");
}

#[tokio::test]
async fn mock_source_async_priority_undefined_and_overlapping_next() {
    let mut h = Harness::new();
    let (source, state) = h.async_source(json!([null,{"content":["after undefined"]}]), "plain");
    let responses = h.get(&state, "responses");
    h.put(&responses, "0", h.heap().undefined().unwrap());
    let bad = h.heap().fresh_compiled_function(&BAD_SYNC, &[]).unwrap();
    h.heap()
        .define_own_data(&source, h.heap().iterator_key(), bad, h.factory.runtime())
        .unwrap();
    let mock = h.from_source(source, None);
    let first = h.run(&mock).await;
    assert!(
        h.rejected_message(&first)
            .contains("no response or handler is configured")
    );
    let second = h.run(&mock).await;
    h.assert_text(&second, "after undefined");
    assert_eq!(h.n(&h.get(&state, "index")), 2.0);
    let malformed = h.heap().object().unwrap();
    h.heap()
        .define_own_data(
            &malformed,
            h.bindings.async_iterator_symbol(),
            h.heap().undefined().unwrap(),
            h.factory.runtime(),
        )
        .unwrap();
    let sync = h
        .heap()
        .fresh_compiled_function(&ITERATOR_SELF, &[])
        .unwrap();
    h.heap()
        .define_own_data(
            &malformed,
            h.heap().iterator_key(),
            sync,
            h.factory.runtime(),
        )
        .unwrap();
    let opts = h.data(json!({}));
    h.put(&opts, "responses", malformed);
    assert!(matches!(
        create_mock_model(h.factory.clone(), h.bindings.clone(), opts),
        Err(ValueError::TypeError(_))
    ));
    let gate = pending(&h);
    let state = h.data(json!({"index":0}));
    h.put(&state, "gate", gate.promise.clone());
    h.put(&state, "release", gate.resolve);
    h.put(
        &state,
        "Promise",
        h.get(&h.realm.globals.environment, "Promise"),
    );
    let source = h.heap().object().unwrap();
    let iter = h
        .heap()
        .fresh_compiled_function(&ITERATOR_SELF, &[])
        .unwrap();
    let next = h
        .heap()
        .fresh_compiled_function(&CONCURRENT_NEXT, std::slice::from_ref(&state))
        .unwrap();
    h.heap()
        .define_own_data(
            &source,
            h.bindings.async_iterator_symbol(),
            iter,
            h.factory.runtime(),
        )
        .unwrap();
    h.put(&source, "next", next);
    let mock = h.from_source(source, None);
    let first = h.stream(&mock, h.context(), None, CallOptions::default());
    let second = h.stream(&mock, h.context(), None, CallOptions::default());
    // The second next is entered synchronously even though first awaits gate.
    assert_eq!(h.n(&h.get(&state, "index")), 2.0);
    let first = h.collect(first).await;
    let second = h.collect(second).await;
    h.assert_text(&first, "first");
    h.assert_text(&second, "second");
    // No task yield or driver drain is permitted between these two streams:
    // Promise resolution's then getter must run during the first invocation.
    let state = h.data(json!({"index":0,"phase":0,"thenGets":0}));
    let first_result = h.data(json!({"done":false,"value":{"content":["first thenable"]}}));
    let then = h
        .heap()
        .fresh_compiled_function(&ADOPTION_THEN, &[first_result])
        .unwrap();
    h.put(&state, "thenFunction", then);
    let thenable = h.heap().object().unwrap();
    let getter = h
        .heap()
        .fresh_compiled_function(&ADOPTION_GETTER, std::slice::from_ref(&state))
        .unwrap();
    accessor(&h, &thenable, "then", getter);
    h.put(&state, "thenable", thenable);
    let source = h.heap().object().unwrap();
    let iter = h
        .heap()
        .fresh_compiled_function(&ITERATOR_SELF, &[])
        .unwrap();
    h.heap()
        .define_own_data(
            &source,
            h.bindings.async_iterator_symbol(),
            iter,
            h.factory.runtime(),
        )
        .unwrap();
    let next = h
        .heap()
        .fresh_compiled_function(&ADOPTION_NEXT, std::slice::from_ref(&state))
        .unwrap();
    h.put(&source, "next", next);
    let mock = h.from_source(source, None);
    let first = h.stream(&mock, h.context(), None, CallOptions::default());
    assert_eq!(h.n(&h.get(&state, "thenGets")), 1.0);
    assert_eq!(h.n(&h.get(&state, "phase")), 1.0);
    let second = h.stream(&mock, h.context(), None, CallOptions::default());
    assert_eq!(h.n(&h.get(&state, "observedPhase")), 1.0);
    assert_eq!(h.n(&h.get(&state, "index")), 2.0);
    let first = h.collect(first).await;
    let second = h.collect(second).await;
    h.assert_text(&first, "first thenable");
    h.assert_text(&second, "phase=1");
    assert_eq!(h.n(&h.get(&state, "thenGets")), 1.0);
    eprintln!("mock-source:synchronous-thenable-adoption:pass");
    eprintln!("mock-source:async-priority-undefined-overlapping-next:pass");
}

#[tokio::test]
async fn mock_source_same_model_two_native_runs_no_replay() {
    let mut h = Harness::new();
    let (source, state) = h.async_source(
        json!([{"content":["first"]},{"content":["second"]}]),
        "plain",
    );
    let source_identity = source.clone();
    let source_next = h.get(&source_identity, "next");
    let mock = h.from_source(source, None);
    let metadata = mock.model_value().clone();
    let calls = mock.calls_value().clone();
    let cost = h.get(&metadata, "cost");
    let first = h.run(&mock).await;
    h.assert_text(&first, "first");
    let old_admission = h.driver.admission();
    let factory = h.factory.clone();
    let realm = h.realm.clone();
    drop(h.driver);
    let driver = realm.begin_promise_run().unwrap();
    let admission = driver.admission();
    let scope = driver.root_scope();
    let parent = driver.root_parent();
    let fresh = Arc::new(factory.with_runtime(Arc::new(admission.scoped(scope, parent).unwrap())));
    h.bindings
        .bind_run(&fresh, &realm, admission, scope, parent)
        .unwrap();
    h.factory = fresh;
    h.driver = driver;
    assert!(old_admission.check_live().is_err());
    let second = h.run(&mock).await;
    h.assert_text(&second, "second");
    assert!(metadata.same_identity(mock.model_value()));
    assert!(calls.same_identity(mock.calls_value()));
    assert!(cost.same_identity(&h.get(mock.model_value(), "cost")));
    assert_eq!(h.len(mock.calls_value()), 2);
    assert_eq!(h.n(&h.get(&state, "index")), 2.0);
    assert!(source_next.same_identity(&h.get(&source_identity, "next")));
    // The same source now awaits its second call while Run2 is closed. Run3
    // keeps the state but cannot turn the old awaiting call into a new one.
    let gate = pending(&h);
    let (mock, opts, state) = headers_mock(&h, gate.promise);
    let old_stream = h.stream(&mock, h.context(), Some(opts), CallOptions::default());
    callback_entered(&mut h, &state).await;
    let factory = h.factory.clone();
    let realm = h.realm.clone();
    drop(h.driver);
    let driver = realm.begin_promise_run().unwrap();
    let admission = driver.admission();
    let scope = driver.root_scope();
    let parent = driver.root_parent();
    let fresh = Arc::new(factory.with_runtime(Arc::new(admission.scoped(scope, parent).unwrap())));
    h.bindings
        .bind_run(&fresh, &realm, admission, scope, parent)
        .unwrap();
    h.factory = fresh;
    h.driver = driver;
    let old = h.collect(old_stream).await;
    assert!(old.events.is_empty());
    assert_eq!(old.failures.len(), 1);
    assert!(matches!(
        old.failures[0].as_ref(),
        ValueError::ExpiredCapability(_)
    ));
    assert_eq!(h.len(mock.calls_value()), 1);
    assert_eq!(h.n(&h.get(&state, "called")), 1.0);
    eprintln!("mock-source:same-model-two-runs-old-inflight-not-replayed:pass");
}
