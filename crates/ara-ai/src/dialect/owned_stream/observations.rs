//! One grouped observation family in the existing AI block1 entrance.
//! Controlled Graph/provider queues; not production CLI or model acceptance.
use super::*;
use ara_rpc::value_heap::NoRuntime;
use ara_rpc::value_heap::PropertyDescriptor;
use std::time::Duration;

static STRING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled same-realm String donor for owned-stream observations",
    name: "String",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |heap, _, _, args, runtime| {
        let value = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
        heap.string(heap.to_string(&value, runtime)?)
    },
};
static ARG_GET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original argument repeated Get",
    name: "get x",
    length: 0,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 3,
    body: |heap, captures, _, _, runtime| {
        let ex = NativeExecution { heap, runtime };
        let count = heap.to_number(&ex.get(&captures[0], "calls")?, runtime)?;
        ex.put(&captures[0], "calls", heap.number(count + 1.0)?)?;
        Ok(captures[if count == 0.0 { 1 } else { 2 }].clone())
    },
};
static CURRENT_STRING: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled current global String donor",
    name: "String",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        let ex = NativeExecution { heap, runtime };
        let arg = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
        ex.put(&captures[0], "arg", arg)?;
        heap.string("currentString".into())
    },
};
static ARG_SET: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled original argument setter",
    name: "set x",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: |heap, captures, _, args, runtime| {
        let value = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
        NativeExecution { heap, runtime }.put(&captures[0], "stored", value)?;
        heap.undefined()
    },
};
fn setup() -> (Arc<GraphValueFactory>, ValueHandle, ValueHandle) {
    let heap = ValueHeap::new().unwrap();
    let factory = Arc::new(GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
    let ex = execution(&factory);
    let globals = ex.object([("String", heap.fresh_compiled_function(&STRING, &[]).unwrap())]).unwrap();
    let params = ex.object([("type", ex.text("object").unwrap())]).unwrap();
    let tool = ex.object([("name", ex.text("read").unwrap()), ("parameters", params)]).unwrap();
    let tools = ex.array([tool]).unwrap();
    (factory, globals, tools)
}
fn message(ex: NativeExecution<'_>, blocks: impl IntoIterator<Item = ValueHandle>, reason: &str) -> ValueHandle {
    ex.object([
        ("content", ex.array(blocks).unwrap()),
        ("stopReason", ex.text(reason).unwrap()),
        ("retained", ex.heap.object().unwrap()),
    ])
    .unwrap()
}
fn text(ex: NativeExecution<'_>, content: impl Into<WireString>) -> ValueHandle {
    ex.object([("type", ex.text("text").unwrap()), ("text", ex.text(content.into()).unwrap())]).unwrap()
}
fn call(ex: NativeExecution<'_>, name: &str, args: ValueHandle) -> ValueHandle {
    ex.object([
        ("type", ex.text("toolCall").unwrap()),
        ("id", ex.text("native-id").unwrap()),
        ("name", ex.text(name).unwrap()),
        ("arguments", args),
    ])
    .unwrap()
}
fn index(ex: NativeExecution<'_>, n: f64) -> ValueHandle {
    ex.heap.number(n).unwrap()
}
fn queued(events: Vec<ActualAssistantEvent>) -> ActualAssistantStream {
    let (sink, stream, receipt) = ActualAssistantStream::channel();
    let guard = receipt.owner_settlement_guard();
    for event in events {
        receipt.retain_terminal(&event);
        assert!(sink.try_push(event));
    }
    drop(sink);
    drop(guard);
    stream
}
async fn collect(mut stream: ActualAssistantStream) -> Vec<ActualAssistantEvent> {
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
    })
    .await
    .expect("owned stream must settle its own event owner");
    assert!(stream.integration_failures().is_empty());
    events
}
fn final_message(events: &[ActualAssistantEvent]) -> ValueHandle {
    events
        .iter()
        .find_map(|event| match event {
            Event::Done { message, .. } => Some(message.clone()),
            _ => None,
        })
        .expect("selected done")
}
fn blocks(ex: NativeExecution<'_>, message: &ValueHandle) -> Vec<ValueHandle> {
    let mut out = Vec::new();
    ex.for_of(&ex.get(message, "content").unwrap(), |value| {
        out.push(value.clone());
        Ok(true)
    })
    .unwrap();
    out
}
fn tool_calls(ex: NativeExecution<'_>, message: &ValueHandle) -> Vec<ValueHandle> {
    blocks(ex, message).into_iter().filter(|b| ex.eq(b, "type", "toolCall").unwrap()).collect()
}
fn inband(dialect: Dialect) -> WireString {
    match dialect {
        Dialect::Deepseek => "<｜tool▁calls▁begin｜><｜tool▁call▁begin｜>function<｜tool▁sep｜>read\n```json\n{\"x\":1}\n```<｜tool▁call▁end｜><｜tool▁calls▁end｜>".into(),
        _ => "<tool_call>{\"name\":\"read\",\"arguments\":{\"x\":1}}</tool_call>".into(),
    }
}

async fn stream_family() {
    for dialect in [Dialect::Deepseek, Dialect::Hermes, Dialect::Qwen3] {
        let (factory, globals, tools) = setup();
        let ex = execution(&factory);
        let input = inband(dialect);
        let seed = message(ex, [text(ex, input.clone())], "stop");
        let mut events = vec![Event::Start { partial: seed.clone() }];
        for chunk in input.units().chunks(7) {
            events.push(Event::TextDelta {
                content_index: index(ex, 0.0),
                delta: WireString::from_units(chunk.to_vec()),
                partial: seed.clone(),
            });
        }
        events.push(Event::Done { reason: StopReason::Stop, message: seed.clone() });
        let outer = CancellationToken::new();
        let child = outer.child_token();
        let result = collect(
            wrap_owned_stream(
                queued(events),
                factory.clone(),
                globals.clone(),
                tools.clone(),
                dialect,
                child.clone(),
                outer.clone(),
                true,
            )
            .unwrap(),
        )
        .await;
        assert!(result.iter().any(|e| matches!(e, Event::ToolcallStart { .. })));
        assert!(result.iter().any(|e| matches!(e, Event::ToolcallEnd { .. })));
        let done = final_message(&result);
        let calls = tool_calls(ex, &done);
        assert_eq!(calls.len(), 1);
        assert!(ex.eq(&calls[0], "name", "read").unwrap());
        let args = ex.get(&calls[0], "arguments").unwrap();
        assert!(ex.number_is(&ex.get(&args, "x").unwrap(), 1.0).unwrap());
        assert!(ex.eq(&done, "stopReason", "toolUse").unwrap());
        assert!(ex.get(&done, "retained").unwrap().same_identity(&ex.get(&seed, "retained").unwrap()));
        assert!(!outer.is_cancelled());
        assert!(!child.is_cancelled());
        let parsed = parse_owned_message(factory.clone(), &globals, &seed, &tools, dialect).unwrap();
        assert_eq!(tool_calls(ex, &parsed).len(), 1);
    }

    // Native first owns the turn, preserving argument/image/delta identity and
    // symbols. Ghost start cannot lock the channel; late delta salvages it.
    let (factory, globals, tools) = setup();
    let ex = execution(&factory);
    let args = ex.object([("x", index(ex, 9.0))]).unwrap();
    let native = call(ex, "read", args.clone());
    let partial_key = streaming_partial_json_key(ex.heap).unwrap();
    let cursor_key = cursor_exec_resolved_key(ex.heap).unwrap();
    ex.heap.assign(&native, partial_key.clone(), ex.text("{\"x\":").unwrap(), ex.runtime).unwrap();
    ex.heap.assign(&native, cursor_key.clone(), ex.heap.boolean(true).unwrap(), ex.runtime).unwrap();
    let ghost = call(ex, " ", ex.heap.object().unwrap());
    let seed = message(ex, [ghost.clone()], "stop");
    let named = message(ex, [native.clone()], "stop");
    let delta = ex.text("opaque native delta").unwrap();
    let image = ex.object([("type", ex.text("image").unwrap()), ("data", ex.text("YQ==").unwrap())]).unwrap();
    let done = message(ex, [text(ex, inband(Dialect::Hermes))], "length");
    let projected = collect(
        wrap_owned_stream(
            queued(vec![
                Event::Start { partial: seed.clone() },
                Event::ToolcallStart { content_index: index(ex, 0.0), partial: seed },
                Event::ToolcallDelta { content_index: index(ex, 0.0), delta: delta.clone(), partial: named },
                Event::ToolcallEnd { content_index: index(ex, 0.0), tool_call: native.clone(), partial: done.clone() },
                Event::ImageEnd { content_index: index(ex, 1.0), content: image.clone(), partial: done.clone() },
                Event::Done { reason: StopReason::Length, message: done },
            ]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Hermes,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert_eq!(projected.iter().filter(|e| matches!(e, Event::ToolcallStart { .. })).count(), 1);
    assert!(projected.iter().any(|e| matches!(e, Event::ToolcallDelta { delta: d, .. } if d.same_identity(&delta))));
    let done = final_message(&projected);
    let calls = tool_calls(ex, &done);
    assert_eq!(calls.len(), 1);
    assert!(ex.get(&calls[0], "arguments").unwrap().same_identity(&args));
    assert!(
        ex.heap
            .get(&calls[0], &partial_key, ex.runtime)
            .unwrap()
            .same_identity(&ex.heap.get(&native, &partial_key, ex.runtime).unwrap())
    );
    assert!(matches!(
        ex.heap.snapshot(&ex.heap.get(&calls[0], &cursor_key, ex.runtime).unwrap()).unwrap(),
        ValueSnapshot::Boolean(true)
    ));
    assert!(blocks(ex, &done).iter().any(|b| b.same_identity(&image)));
    assert!(ex.eq(&done, "stopReason", "length").unwrap());

    // Whole lifecycle retains symbol clearing shape and sticky Cursor marker.
    let copied = Projector::clone_call(ex, &native).unwrap();
    let changed_args = ex.heap.object().unwrap();
    let changed = call(ex, "changed", changed_args.clone());
    ex.heap.assign(&changed, cursor_key.clone(), ex.heap.boolean(false).unwrap(), ex.runtime).unwrap();
    Projector::sync_call(ex, &copied, &changed).unwrap();
    assert!(ex.heap.has_own(&copied, &partial_key, ex.runtime).unwrap());
    assert!(ex.undefined(&ex.heap.get(&copied, &partial_key, ex.runtime).unwrap()).unwrap());
    assert!(ex.get(&copied, "arguments").unwrap().same_identity(&changed_args));
    assert!(matches!(
        ex.heap.snapshot(&ex.heap.get(&copied, &cursor_key, ex.runtime).unwrap()).unwrap(),
        ValueSnapshot::Boolean(true)
    ));

    // The original delta branch performs two argument Gets and looks up the
    // current String callee before the second, rather than coercing snapshots.
    let seed_for_delta = message(ex, [], "stop");
    let mut projector =
        Projector::new(factory.clone(), &globals, &tools, Dialect::Hermes, &seed_for_delta, true).unwrap();
    projector.begin_tool("manual".into(), InbandNameValue::Source("read".into())).unwrap();
    let entry = map_get(ex, &projector.tool_blocks, ex.text("manual").unwrap()).unwrap();
    let changing = ex.heap.object().unwrap();
    let state = ex.object([("calls", index(ex, 0.0))]).unwrap();
    let second_value = ex.heap.get(&native, &cursor_key, ex.runtime).unwrap();
    let getter = ex
        .heap
        .fresh_compiled_function(&ARG_GET, &[state.clone(), ex.text("first").unwrap(), second_value.clone()])
        .unwrap();
    let setter = ex.heap.fresh_compiled_function(&ARG_SET, &[state.clone()]).unwrap();
    let ValueSnapshot::Function(getter) = ex.heap.snapshot(&getter).unwrap() else { panic!("getter") };
    let ValueSnapshot::Function(setter) = ex.heap.snapshot(&setter).unwrap() else { panic!("setter") };
    ex.heap
        .define_own(
            &changing,
            "x".into(),
            PropertyDescriptor::Accessor { get: Some(getter), set: Some(setter), enumerable: true, configurable: true },
            ex.runtime,
        )
        .unwrap();
    ex.put(&ex.get(&entry, "block").unwrap(), "arguments", changing.clone()).unwrap();
    ex.put(&globals, "String", ex.heap.fresh_compiled_function(&CURRENT_STRING, &[state.clone()]).unwrap()).unwrap();
    projector.delta_tool("manual".into(), InbandNameValue::Source("read".into()), "x".into(), "D".into()).unwrap();
    assert!(ex.number_is(&ex.get(&state, "calls").unwrap(), 2.0).unwrap());
    assert!(ex.get(&state, "arg").unwrap().same_identity(&second_value));
    assert!(ex.eq(&state, "stored", "currentStringD").unwrap());

    // Inband first suppresses native dispatch. A ghost remains harmless.
    let seed = message(ex, [text(ex, inband(Dialect::Hermes))], "stop");
    let mut input = inband(Dialect::Hermes).units().to_vec();
    input.extend(vec![0x20; 32]);
    let result = collect(
        wrap_owned_stream(
            queued(vec![
                Event::Start { partial: seed.clone() },
                Event::TextDelta {
                    content_index: index(ex, 0.0),
                    delta: WireString::from_units(input),
                    partial: seed.clone(),
                },
                Event::ToolcallEnd { content_index: index(ex, 5.0), tool_call: native.clone(), partial: seed.clone() },
                Event::Done { reason: StopReason::Stop, message: seed },
            ]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Hermes,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert_eq!(tool_calls(ex, &final_message(&result)).len(), 1);
    assert_eq!(result.iter().filter(|e| matches!(e, Event::ToolcallStart { .. })).count(), 1);

    // Late end also salvages native name, while nameless end is discarded.
    let seed = message(ex, [], "stop");
    let result = collect(
        wrap_owned_stream(
            queued(vec![
                Event::Start { partial: seed.clone() },
                Event::ToolcallEnd { content_index: index(ex, 0.0), tool_call: ghost, partial: seed.clone() },
                Event::ToolcallEnd { content_index: index(ex, 2.0), tool_call: native, partial: seed.clone() },
                Event::Done { reason: StopReason::Stop, message: seed },
            ]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Qwen3,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert_eq!(tool_calls(ex, &final_message(&result)).len(), 1);

    // Final full-text supplements unstreamed suffix; explicit thinking closes
    // with its original UTF-16 units, not provider's stale end text.
    let lone = WireString::from_units(vec![0xd800]);
    let seed = message(ex, [text(ex, "prefix-tail")], "stop");
    let result = collect(
        wrap_owned_stream(
            queued(vec![
                Event::Start { partial: seed.clone() },
                Event::ThinkingStart { content_index: index(ex, 0.0), partial: seed.clone() },
                Event::ThinkingDelta { content_index: index(ex, 0.0), delta: lone.clone(), partial: seed.clone() },
                Event::ThinkingEnd { content_index: index(ex, 0.0), content: "ignored".into(), partial: seed.clone() },
                Event::TextDelta { content_index: index(ex, 1.0), delta: "prefix".into(), partial: seed.clone() },
                Event::Done { reason: StopReason::Stop, message: seed },
            ]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Qwen3,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert!(result.iter().any(|e| matches!(e, Event::ThinkingEnd { content, .. } if content == &lone)));
    let done = final_message(&result);
    assert!(
        blocks(ex, &done).iter().any(|b| ex.eq(b, "type", "text").unwrap() && ex.eq(b, "text", "prefix-tail").unwrap())
    );

    // Source Error is forwarded by identity without a synthesized success.
    let error = message(ex, [], "error");
    let result = collect(
        wrap_owned_stream(
            queued(vec![Event::Error { reason: StopReason::Error, error: error.clone() }]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Hermes,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert!(matches!(&result[..], [Event::Error { error: actual, .. }] if actual.same_identity(&error)));

    for abort in [true, false] {
        let full = "safe<tool_response>fabricated";
        let seed = message(ex, [text(ex, full)], "stop");
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let result = collect(
            wrap_owned_stream(
                queued(vec![
                    Event::Start { partial: seed.clone() },
                    Event::TextDelta {
                        content_index: index(ex, 0.0),
                        delta: "safe<tool_res".into(),
                        partial: seed.clone(),
                    },
                    Event::TextDelta {
                        content_index: index(ex, 0.0),
                        delta: "ponse>fabricated".into(),
                        partial: seed.clone(),
                    },
                    Event::Done { reason: StopReason::Stop, message: seed },
                ]),
                factory.clone(),
                globals.clone(),
                tools.clone(),
                Dialect::Hermes,
                child.clone(),
                parent.clone(),
                abort,
            )
            .unwrap(),
        )
        .await;
        let done = final_message(&result);
        assert!(
            blocks(ex, &done).iter().any(|b| ex.eq(b, "type", "text").unwrap() && ex.eq(b, "text", "safe").unwrap())
        );
        assert_eq!(child.is_cancelled(), abort);
        assert!(!parent.is_cancelled());
    }

    // Cancellation/closed sink retains ready selected terminals and inherited
    // late faults, without declaring an unsettled original producer sealed.
    for cancel in [true, false] {
        let (sink, inner, original) = ActualAssistantStream::channel();
        let original_guard = original.owner_settlement_guard();
        let error = message(ex, [], "error");
        assert!(sink.try_push(Event::Error { reason: StopReason::Error, error: error.clone() }));
        let parent = CancellationToken::new();
        let child = parent.child_token();
        let mut output = wrap_owned_stream(
            inner,
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Hermes,
            child.clone(),
            parent.clone(),
            true,
        )
        .unwrap();
        let receipt = output.failure_receipt();
        if cancel {
            parent.cancel();
        }
        let terminal =
            tokio::time::timeout(Duration::from_secs(2), output.take_terminal_on_close()).await.unwrap().unwrap();
        assert!(matches!(terminal, Event::Error { error: actual, .. } if actual.same_identity(&error)));
        assert!(child.is_cancelled());
        assert!(!receipt.is_settled());
        let thrown = ex.heap.object().unwrap();
        original.record(ValueError::Thrown(thrown.clone()));
        assert!(
            receipt
                .failures()
                .iter()
                .any(|failure| matches!(failure.as_ref(), ValueError::Thrown(actual) if actual.same_identity(&thrown)))
        );
        drop(sink);
        drop(original_guard);
        assert!(receipt.is_settled());
    }
    // EOF is no model outcome; no fake Done.
    let result = collect(
        wrap_owned_stream(
            queued(vec![]),
            factory.clone(),
            globals.clone(),
            tools.clone(),
            Dialect::Hermes,
            CancellationToken::new(),
            CancellationToken::new(),
            true,
        )
        .unwrap(),
    )
    .await;
    assert!(result.is_empty());
    for dialect in [
        Dialect::Glm,
        Dialect::Kimi,
        Dialect::Xml,
        Dialect::Anthropic,
        Dialect::Harmony,
        Dialect::Gemini,
        Dialect::Gemma,
        Dialect::Minimax,
    ] {
        assert!(matches!(
            wrap_owned_stream(
                queued(vec![]),
                factory.clone(),
                globals.clone(),
                tools.clone(),
                dialect,
                CancellationToken::new(),
                CancellationToken::new(),
                true
            ),
            Err(ValueError::UnsupportedOperation(_))
        ));
        assert!(matches!(
            parse_owned_message(factory.clone(), &globals, &message(ex, [], "stop"), &tools, dialect),
            Err(ValueError::UnsupportedOperation(_))
        ));
    }
}

pub(super) fn observe_owned_stream_family() {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(stream_family());
}
