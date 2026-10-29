//! Coarse Graph observations called by the existing block1 module entrance.
//! Controlled same-realm globals exercise the actual ValueHeap JSON encoder;
//! they are not production CLI/account/model acceptance.
use super::*;
use ara_rpc::value_heap::{CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest};
use std::sync::Mutex;

fn json_body(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let value = first(heap, arguments)?;
    match heap.stringify_native(&value, arguments.get(1), arguments.get(2), runtime)? {
        Some(value) => heap.string(value),
        None => heap.undefined(),
    }
}
static JSON: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled actual JSON donor for original owned projection module observations",
    name: "stringify",
    length: 3,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: json_body,
};
fn now_body(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = NativeExecution { heap, runtime };
    let calls = ex.get(&captures[0], "calls")?;
    let calls = heap.to_number(&calls, runtime)?;
    ex.own(&captures[0], "calls", heap.number(calls + 1.0)?)?;
    Ok(captures[1].clone())
}
static NOW: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "controlled same-realm Date.now owned projection observation",
    name: "now",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 2,
    body: now_body,
};

fn globals(ex: NativeExecution<'_>) -> (ValueHandle, ValueHandle, ValueHandle) {
    let json = ex.heap.object().unwrap();
    ex.own(&json, "stringify", ex.heap.fresh_compiled_function(&JSON, &[]).unwrap()).unwrap();
    let count = ex.object([("calls", ex.heap.number(0.0).unwrap())]).unwrap();
    let date = ex.heap.object().unwrap();
    ex.own(
        &date,
        "now",
        ex.heap.fresh_compiled_function(&NOW, &[count.clone(), ex.heap.number(42.0).unwrap()]).unwrap(),
    )
    .unwrap();
    (json, date, count)
}
fn get(ex: NativeExecution<'_>, root: &ValueHandle, key: &str) -> ValueHandle {
    ex.get(root, key).unwrap()
}
fn text(ex: NativeExecution<'_>, root: &ValueHandle) -> WireString {
    ex.string(root).unwrap()
}
fn object(ex: NativeExecution<'_>, fields: impl IntoIterator<Item = (&'static str, ValueHandle)>) -> ValueHandle {
    ex.object(fields).unwrap()
}
fn tool(ex: NativeExecution<'_>, name: &str) -> ValueHandle {
    object(
        ex,
        [
            ("name", ex.text(name).unwrap()),
            ("description", ex.text("literal $& $1 $$").unwrap()),
            ("parameters", object(ex, [("type", ex.text("object").unwrap())])),
        ],
    )
}
fn context(ex: NativeExecution<'_>, tools: ValueHandle, messages: ValueHandle) -> ValueHandle {
    object(
        ex,
        [
            ("systemPrompt", ex.array([ex.text("system-original").unwrap()]).unwrap()),
            ("messages", messages),
            ("tools", tools),
            ("custom", ex.text("same-custom").unwrap()),
        ],
    )
}

struct CaptureRuntime {
    heap: Arc<ValueHeap>,
    original_map: ValueHandle,
    retained: Mutex<Vec<ValueHandle>>,
    log: Mutex<Vec<&'static str>>,
    prompt: ValueHandle,
    messages: ValueHandle,
    other_tools: ValueHandle,
    thrown: ValueHandle,
    throw_tools: Mutex<bool>,
}
impl ValueRuntime for CaptureRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        match request {
            RuntimeRequest::Call { capability: CapabilityId(501), receiver, arguments } => {
                self.retained.lock().unwrap().push(first(&self.heap, arguments)?);
                self.heap.call(&self.original_map, receiver, arguments, self).map(RuntimeReply::Value)
            }
            RuntimeRequest::Call { capability: CapabilityId(502), .. } => {
                self.log.lock().unwrap().push("systemPrompt");
                Ok(RuntimeReply::Value(self.prompt.clone()))
            }
            RuntimeRequest::Call { capability: CapabilityId(503), .. } => {
                self.log.lock().unwrap().push("messages");
                Ok(RuntimeReply::Value(self.messages.clone()))
            }
            RuntimeRequest::Call { capability: CapabilityId(504), .. } => {
                self.log.lock().unwrap().push("tools");
                if *self.throw_tools.lock().unwrap() {
                    Err(ValueError::Thrown(self.thrown.clone()))
                } else {
                    Ok(RuntimeReply::Value(self.other_tools.clone()))
                }
            }
            _ => Err(ValueError::UnsupportedOperation("controlled projection runtime operation")),
        }
    }
}

pub(super) fn observe_owned_projection_family() {
    // Same original message/argument/image handles, lossless source units,
    // contiguous results and the original plain versus BigInt JSON contracts.
    for dialect in [Dialect::Deepseek, Dialect::Hermes, Dialect::Qwen3] {
        let heap = ValueHeap::new().unwrap();
        let factory = Arc::new(GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap());
        let ex = NativeExecution { heap: &heap, runtime: factory.runtime() };
        let (json, date, now_count) = globals(ex);
        let tools = ex.array([tool(ex, "read")]).unwrap();
        let source_text = WireString::from_units(vec![0x61, 0xd800, 0x20, 0x0a]);
        let arguments = object(ex, [("big", heap.bigint_decimal("9007199254740993").unwrap())]);
        let call = object(
            ex,
            [
                ("type", ex.text("toolCall").unwrap()),
                ("id", ex.text("call-1").unwrap()),
                ("name", ex.text("read").unwrap()),
                ("arguments", arguments.clone()),
            ],
        );
        let assistant = object(
            ex,
            [
                ("role", ex.text("assistant").unwrap()),
                (
                    "content",
                    ex.array([
                        object(
                            ex,
                            [("type", ex.text("text").unwrap()), ("text", heap.string(source_text.clone()).unwrap())],
                        ),
                        call.clone(),
                    ])
                    .unwrap(),
                ),
                ("providerPayload", arguments.clone()),
            ],
        );
        let unchanged = object(ex, [("role", ex.text("assistant").unwrap()), ("content", ex.array([]).unwrap())]);
        let user = object(ex, [("role", ex.text("user").unwrap()), ("content", ex.text("original-user").unwrap())]);
        let image = object(
            ex,
            [
                ("type", ex.text("image").unwrap()),
                ("data", ex.text("aW1hZ2U=").unwrap()),
                ("mimeType", ex.text("image/png").unwrap()),
            ],
        );
        let result = |id: &str, body: &str, with_image: bool| {
            object(
                ex,
                [
                    ("role", ex.text("toolResult").unwrap()),
                    ("toolCallId", ex.text(id).unwrap()),
                    ("toolName", ex.text("read").unwrap()),
                    ("isError", heap.boolean(false).unwrap()),
                    (
                        "content",
                        ex.array(if with_image {
                            vec![
                                object(ex, [("type", ex.text("text").unwrap()), ("text", ex.text(body).unwrap())]),
                                image.clone(),
                            ]
                        } else {
                            vec![object(ex, [("type", ex.text("text").unwrap()), ("text", ex.text(body).unwrap())])]
                        })
                        .unwrap(),
                    ),
                ],
            )
        };
        let first_result = result("call-1", "first", true);
        let second_result = result("call-2", "second", false);
        let messages = ex
            .array([user.clone(), unchanged.clone(), assistant.clone(), first_result.clone(), second_result])
            .unwrap();
        let original = context(ex, tools.clone(), messages.clone());
        let projected = project_owned_context(&factory, &original, &tools, dialect, &json, &date).unwrap();
        assert!(!projected.same_identity(&original));
        assert!(get(ex, &original, "tools").same_identity(&tools));
        assert!(ex.undefined(&get(ex, &projected, "tools")).unwrap());
        assert!(get(ex, &projected, "custom").same_identity(&get(ex, &original, "custom")));
        let encoded = get(ex, &projected, "messages");
        assert!(ex.number_is(&get(ex, &encoded, "length"), 4.0).unwrap());
        assert!(get(ex, &encoded, "0").same_identity(&user));
        assert!(get(ex, &encoded, "1").same_identity(&unchanged));
        let encoded_assistant = get(ex, &encoded, "2");
        assert!(!encoded_assistant.same_identity(&assistant));
        assert!(get(ex, &encoded_assistant, "providerPayload").same_identity(&arguments));
        assert!(get(ex, &call, "arguments").same_identity(&arguments));
        let encoded_call = text(ex, &get(ex, &get(ex, &get(ex, &encoded_assistant, "content"), "0"), "text"));
        assert!(encoded_call.units().starts_with(&[0x61, 0xd800, 0x0a]));
        assert!(encoded_call.units().windows(16).any(|w| w == WireString::from("9007199254740993").units()));
        let encoded_result = get(ex, &encoded, "3");
        assert!(get(ex, &get(ex, &encoded_result, "content"), "1").same_identity(&image));
        assert!(ex.number_is(&get(ex, &encoded_result, "timestamp"), 42.0).unwrap());
        assert!(ex.number_is(&get(ex, &now_count, "calls"), 1.0).unwrap());
        let result_text = text(ex, &get(ex, &get(ex, &get(ex, &encoded_result, "content"), "0"), "text"));
        let expected = match dialect {
            Dialect::Deepseek => format!(
                "{}first{}\n{}second{}",
                deepseek::DEEPSEEK_TOOL_OUTPUT_BEGIN,
                deepseek::DEEPSEEK_TOOL_OUTPUT_END,
                deepseek::DEEPSEEK_TOOL_OUTPUT_BEGIN,
                deepseek::DEEPSEEK_TOOL_OUTPUT_END
            ),
            _ => "<tool_response>\nfirst\n</tool_response>\n<tool_response>\nsecond\n</tool_response>".into(),
        };
        assert_eq!(result_text, WireString::from(expected));
        let catalog = text(ex, &get(ex, &get(ex, &projected, "systemPrompt"), "1")).to_utf8().unwrap();
        assert!(catalog.contains("literal $& $1 $$"));
        assert!(catalog.contains(r#""name":"read""#));

        // Zero is an existing timestamp, and unknown arguments become the
        // fixed rendering helper's null fallback. No replacement Date clock.
        ex.own(&first_result, "timestamp", heap.number(0.0).unwrap()).unwrap();
        let second_projection = project_owned_context(&factory, &original, &tools, dialect, &json, &date).unwrap();
        let result_message = get(ex, &get(ex, &second_projection, "messages"), "3");
        assert!(ex.number_is(&get(ex, &result_message, "timestamp"), 0.0).unwrap());
        assert!(ex.number_is(&get(ex, &now_count, "calls"), 1.0).unwrap());
        let undefined_call = object(ex, [("name", ex.text("read").unwrap()), ("arguments", heap.undefined().unwrap())]);
        let calls = ex.array([undefined_call]).unwrap();
        let null_call = text(ex, &render_calls(ex, &calls, dialect, &json).unwrap()).to_utf8().unwrap();
        if dialect == Dialect::Deepseek {
            assert!(null_call.contains("null"));
        } else {
            assert!(null_call.contains(r#"{"name":"read"}"#));
        }

        // Catalogue deliberately uses plain JSON and propagates BigInt errors.
        let invalid_tool = tool(ex, "invalid");
        let parameters = get(ex, &invalid_tool, "parameters");
        ex.own(&parameters, "default", heap.bigint_decimal("2").unwrap()).unwrap();
        let invalid_tools = ex.array([invalid_tool]).unwrap();
        assert!(project_owned_context(&factory, &original, &invalid_tools, dialect, &json, &date).is_err());
        assert!(get(ex, &original, "messages").same_identity(&messages));
    }

    // Source captures the admitted tools before spreading Context. Getter
    // output during spread must never replace that collection in the catalog.
    let heap = ValueHeap::new().unwrap();
    let base = NativeExecution { heap: &heap, runtime: &NoRuntime };
    let tools = base.array([tool(base, "captured-original")]).unwrap();
    let original_map = get(base, &tools, "map");
    let prompt = base.array([base.text("original-prompt").unwrap()]).unwrap();
    let messages = base.array([]).unwrap();
    let other_tools = base.array([tool(base, "wrong-spread-tools")]).unwrap();
    let runtime = Arc::new(CaptureRuntime {
        heap: heap.clone(),
        original_map,
        retained: Mutex::new(Vec::new()),
        log: Mutex::new(Vec::new()),
        prompt,
        messages,
        other_tools,
        thrown: heap.object().unwrap(),
        throw_tools: Mutex::new(false),
    });
    let factory = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
    let ex = NativeExecution { heap: &heap, runtime: runtime.as_ref() };
    let (json, date, _) = globals(ex);
    ex.own(&tools, "map", heap.function(CapabilityId(501)).unwrap()).unwrap();
    let original = heap.object().unwrap();
    for (key, capability) in [("systemPrompt", 502), ("messages", 503), ("tools", 504)] {
        heap.define_own(
            &original,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(capability))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            runtime.as_ref(),
        )
        .unwrap();
    }
    let projected = project_owned_context(&factory, &original, &tools, Dialect::Deepseek, &json, &date).unwrap();
    assert_eq!(*runtime.log.lock().unwrap(), ["systemPrompt", "messages", "tools", "systemPrompt", "messages"]);
    let output = text(ex, &get(ex, &get(ex, &projected, "systemPrompt"), "1")).to_utf8().unwrap();
    assert!(output.contains("captured-original"));
    assert!(!output.contains("wrong-spread-tools"));
    let callback = runtime.retained.lock().unwrap()[0].clone();
    assert!(ex.number_is(&get(ex, &callback, "length"), 1.0).unwrap());
    assert!(text(ex, &get(ex, &callback, "name")).is_empty());
    drop(factory);
    // The compiled source callback survives its original map call, borrowing
    // the supplied current runtime rather than an expired local callback scope.
    let later_tool = tool(ex, "retained-later");
    let later = heap.call(&callback, &heap.undefined().unwrap(), &[later_tool], runtime.as_ref()).unwrap();
    assert!(text(ex, &later).to_utf8().unwrap().contains("retained-later"));
    *runtime.throw_tools.lock().unwrap() = true;
    let factory = Arc::new(GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap());
    let error = project_owned_context(&factory, &original, &tools, Dialect::Deepseek, &json, &date).unwrap_err();
    assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&runtime.thrown)));

    // Missing dialect support fails before any source getters or donors run.
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
        runtime.log.lock().unwrap().clear();
        assert!(matches!(
            project_owned_context(&factory, &original, &tools, dialect, &json, &date),
            Err(ValueError::UnsupportedOperation(_))
        ));
        assert!(runtime.log.lock().unwrap().is_empty());
    }
}
