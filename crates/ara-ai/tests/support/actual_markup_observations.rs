//! Compound observations attached by Root to the existing openai_http entry.
//! No test attribute, harness, runtime helper or alternate canonical is added.
//! Selected fixed OMP stream-markup-healing.test.ts inputs (596f2da, MIT).

use ara_ai::providers::openai_completions::{
    OpenAICompat,
    actual::markup::{HealingEvent, HealingPattern, StreamMarkupHealing},
};
use ara_ai::{
    ActualAssistantEvent, AssistantMessageEvent, GraphProviderContext, StopReason, graph_values::GraphValueFactory,
};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{NoRuntime, PropertyKey, ValueError, ValueHandle, ValueHeap, ValueSnapshot},
};
use serde_json::{Value, json};
use std::{future::Future, sync::Arc, time::Duration};

fn factory() -> Arc<GraphValueFactory> {
    Arc::new(GraphValueFactory::new(ValueHeap::new().unwrap(), Arc::new(NoRuntime)).unwrap())
}
fn get(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> ValueHandle {
    factory.heap().get(root, &PropertyKey::from(key), factory.runtime()).unwrap()
}
fn string(factory: &GraphValueFactory, value: &ValueHandle) -> WireString {
    match factory.heap().snapshot(value).unwrap() {
        ValueSnapshot::String(value) => value,
        other => panic!("Expected original string, observed {other:?}"),
    }
}
fn text(factory: &GraphValueFactory, root: &ValueHandle, key: &str) -> WireString {
    string(factory, &get(factory, root, key))
}
fn collect(factory: &GraphValueFactory, array: &ValueHandle) -> Vec<ValueHandle> {
    let mut iterator = factory.heap().get_iterator(array, factory.runtime()).unwrap();
    let mut values = Vec::new();
    while let Some(value) = factory.heap().iterator_next(&mut iterator, factory.runtime()).unwrap() {
        values.push(value);
    }
    values
}

pub fn observe_wrapper_compound() {
    let factory = factory();
    // Carrier checks live inside the original coarse entry; no new harness.
    let root = factory.heap().object().unwrap();
    let delta = factory.heap().undefined().unwrap();
    let raw: ActualAssistantEvent = AssistantMessageEvent::ToolcallDelta {
        content_index: factory.heap().number(0.0).unwrap(),
        delta: delta.clone(),
        partial: root.clone(),
    };
    match &raw {
        AssistantMessageEvent::ToolcallDelta { delta: received, .. } => {
            assert!(received.same_identity(&delta));
            assert!(matches!(factory.heap().snapshot(received).unwrap(), ValueSnapshot::Undefined));
        }
        _ => unreachable!(),
    }
    assert_eq!(raw.try_printable().unwrap(), json!({"type":"toolcall_delta","contentIndex":0}));
    let image = factory
        .heap()
        .import_strict(WireValue::parse(r#"{"type":"image","data":"YQ==","mimeType":"image/png"}"#).unwrap())
        .unwrap();
    let content = factory.heap().array_with_length(0).unwrap();
    factory.heap().define_own_data(&content, PropertyKey::from("0"), image.clone(), factory.runtime()).unwrap();
    factory.heap().define_own_data(&root, PropertyKey::from("content"), content, factory.runtime()).unwrap();
    let raw: ActualAssistantEvent = AssistantMessageEvent::ImageEnd {
        content_index: factory.heap().number(0.0).unwrap(),
        content: image.clone(),
        partial: root,
    };
    match &raw {
        AssistantMessageEvent::ImageEnd { content, partial, .. } => {
            assert!(content.same_identity(&image));
            assert!(get(&factory, &get(&factory, partial, "content"), "0").same_identity(content));
        }
        _ => unreachable!(),
    }
    assert_eq!(raw.try_printable().unwrap()["content"], json!({"type":"image","data":"YQ==","mimeType":"image/png"}));
    let specimens = [
        (
            HealingPattern::Kimi,
            "<|tool_calls_section_begin|><|tool_call_begin|>functions.read:0<|tool_call_argument_begin|>{\"path\":\"/etc/hostname\"}<|tool_call_end|><|tool_calls_section_end|>",
        ),
        (
            HealingPattern::Dsml,
            "<｜DSML｜tool_calls><｜DSML｜invoke name=\"read\"><｜DSML｜parameter name=\"path\" string=\"true\">/etc/hostname</｜DSML｜parameter></｜DSML｜invoke></｜DSML｜tool_calls>",
        ),
        (HealingPattern::Qwen, "<tool_calls>\n<read path=\"/etc/hostname\" />\n</tool_calls>"),
    ];
    for (pattern, specimen) in specimens {
        let mut healing = StreamMarkupHealing::new(factory.clone(), pattern).unwrap();
        let mut events = healing.feed_events(&"<think>plan</think>Before".into()).unwrap();
        // Source arbitrary chunk boundaries, including split closing markers.
        for unit in specimen.encode_utf16() {
            events.extend(healing.feed_events(&WireString::from_units(vec![unit])).unwrap());
        }
        events.extend(healing.feed_events(&"After".into()).unwrap());
        events.extend(healing.flush_events().unwrap());
        let mut visible = Vec::new();
        let mut thinking = Vec::new();
        let mut calls = Vec::new();
        for event in events {
            match event {
                HealingEvent::Text(value) => visible.extend_from_slice(value.units()),
                HealingEvent::Thinking(value) => thinking.extend_from_slice(value.units()),
                HealingEvent::ToolCall(call) => calls.push(call),
            }
        }
        assert_eq!(WireString::from_units(visible), WireString::from("BeforeAfter"), "{pattern:?}");
        assert_eq!(WireString::from_units(thinking), WireString::from("plan"), "{pattern:?}");
        assert_eq!(calls.len(), 1, "{pattern:?}");
        assert_eq!(string(&factory, &calls[0].name), WireString::from("read"));
        let arguments: Value = serde_json::from_str(&string(&factory, &calls[0].arguments).to_utf8().unwrap()).unwrap();
        assert_eq!(arguments, json!({"path":"/etc/hostname"}));
        let id = calls[0].id.to_utf8().unwrap();
        assert_eq!(id.len(), 29);
        assert!(id.starts_with("call_"));
        assert!(id[5..].bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
        // The wrapper's sectionClosed observes each input chunk only: split
        // close markers do not magically become one joined source chunk.
        assert!(!healing.section_closed());
    }
    let mut qwen = StreamMarkupHealing::new(factory.clone(), HealingPattern::Qwen).unwrap();
    assert!(qwen.feed(&"<tool_calls><read path=\"/etc/host".into()).unwrap().is_empty());
    assert!(qwen.flush_pending().unwrap().is_empty());
    assert!(qwen.drain_completed().is_empty());
    let mut units = "<tool_calls><read path=\"".encode_utf16().collect::<Vec<_>>();
    units.push(0xd800);
    units.extend("\"/></tool_calls>".encode_utf16());
    let mut qwen = StreamMarkupHealing::new(factory.clone(), HealingPattern::Qwen).unwrap();
    assert!(qwen.feed(&WireString::from_units(units)).unwrap().is_empty());
    let calls = qwen.drain_completed();
    assert_eq!(calls.len(), 1);
    let json = string(&factory, &calls[0].arguments);
    assert!(json.to_utf8().unwrap().contains("\\ud800"));
    let parsed = ara_ai::relaxed_json::parse_streaming_json_units(&json, factory.as_ref()).unwrap();
    assert_eq!(text(&factory, &parsed, "path").units(), &[0xd800]);
    assert!(qwen.section_closed());
    let mut thinking = StreamMarkupHealing::new(factory, HealingPattern::Thinking).unwrap();
    let row = "prefix `<think>` suffix";
    let mut visible = WireString::from("");
    for unit in row.encode_utf16() {
        let emitted = thinking.feed(&WireString::from_units(vec![unit])).unwrap();
        let mut units = visible.units().to_vec();
        units.extend_from_slice(emitted.units());
        visible = WireString::from_units(units);
    }
    let tail = thinking.flush_pending().unwrap();
    let mut units = visible.units().to_vec();
    units.extend_from_slice(tail.units());
    assert_eq!(WireString::from_units(units), WireString::from(row));
}

pub struct ChatObservationCase {
    pub frames: Vec<Value>,
    pub done_sentinel: bool,
    pub compat: OpenAICompat,
}
pub struct ObservedChat {
    pub factory: Arc<GraphValueFactory>,
    pub events: Vec<ActualAssistantEvent>,
    pub terminal: ValueHandle,
    pub failures: Vec<Arc<ValueError>>,
}

/// One invocation from the existing compound HTTP entrance. Server, Script,
/// model and options reuse the parent fixture; actual events retain original
/// handles and their settled failure receipt rather than using typed collect.
pub async fn observe_actual_markup() {
    observe_wrapper_compound();
    observe_actual_chat_compound(|case| async move {
        let mut scripted = case.frames.into_iter().map(|frame| json!({"data":frame})).collect::<Vec<_>>();
        if case.done_sentinel {
            scripted.push(json!({"done":true}));
        }
        let endpoint =
            super::FakeUpstream::start(super::script(json!({"responses":[{"events":scripted}]})), None).await.unwrap();
        let factory = factory();
        let user = factory
            .heap()
            .import_strict(
                WireValue::parse(&json!({"role":"user","content":"list the files","timestamp":1}).to_string()).unwrap(),
            )
            .unwrap();
        let original_context = factory.heap().object().unwrap();
        let messages = factory.heap().array(vec![user]).unwrap();
        for (key, value) in [
            ("systemPrompt", factory.heap().array_with_length(0).unwrap()),
            ("messages", messages),
            ("tools", factory.heap().undefined().unwrap()),
        ] {
            factory
                .heap()
                .define_own_data(&original_context, PropertyKey::from(key), value, factory.runtime())
                .unwrap();
        }
        let context = GraphProviderContext {
            original_context: Some(original_context),
            system_prompt: Vec::new(),
            messages: Vec::new(),
            tools: None,
            history_demotion_dialect: None,
            history_id_hash: None,
            history_id_hash_function: None,
        };
        let mut options = super::opts();
        options.compat = case.compat;
        options.retry.max_attempts = 1;
        options.accept_empty_response = true;
        let mut stream = ara_ai::providers::openai_completions::actual::stream(
            reqwest::Client::new(),
            super::model(&endpoint.base_url()),
            context,
            factory.clone(),
            options,
        )
        .unwrap();
        let receipt = stream.failure_receipt();
        let collected = tokio::time::timeout(Duration::from_secs(8), async {
            let mut events = Vec::new();
            while let Some(event) = stream.recv().await {
                events.push(event);
            }
            events
        })
        .await;
        // Normal EOF has drained all sends; a timeout drops the receiver
        // before waiting. Never wait for settlement with a full live channel.
        drop(stream);
        let failures = tokio::time::timeout(Duration::from_secs(2), receipt.wait_settled())
            .await
            .expect("Actual producer settlement");
        let events = collected.expect("Actual raw stream did not reach EOF");
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
        let terminal = events.last().expect("Actual terminal event").partial().clone();
        assert_eq!(endpoint.served(), 1);
        let recorded = endpoint.requests.lock().await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0]["body"]["messages"][0]["content"], "list the files");
        drop(recorded);
        ObservedChat { factory, events, terminal, failures }
    })
    .await;
}
fn frame(delta: Value, finish: Option<&str>) -> Value {
    json!({"id":"markup-fixture","choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
}
fn joined(observed: &ObservedChat, kind: &str, key: &str) -> WireString {
    let content = get(&observed.factory, &observed.terminal, "content");
    let mut output = Vec::new();
    for block in collect(&observed.factory, &content) {
        if text(&observed.factory, &block, "type").equals_ascii(kind) {
            output.extend_from_slice(text(&observed.factory, &block, key).units());
        }
    }
    WireString::from_units(output)
}
fn reason(observed: &ObservedChat) -> StopReason {
    assert!(observed.failures.is_empty(), "Integration failure is not an ordinary stream outcome");
    match observed.events.last().unwrap() {
        AssistantMessageEvent::Done { reason, .. } | AssistantMessageEvent::Error { reason, .. } => *reason,
        _ => panic!("Missing terminal event"),
    }
}

/// Root supplies its existing fake HTTP/settled raw stream collector. This
/// reuses the shared runner instead of adding another test server/entrance.
pub async fn observe_actual_chat_compound<F, Fut>(mut run: F)
where
    F: FnMut(ChatObservationCase) -> Fut,
    Fut: Future<Output = ObservedChat>,
{
    let qwen = run(ChatObservationCase {
        compat: OpenAICompat { stream_markup_healing_pattern: Some(HealingPattern::Qwen), ..Default::default() },
        frames: vec![
            frame(json!({"reasoning_content":42,"content":"<think>plan</think>Before<tool_ca"}), None),
            frame(json!({"content":"lls><read path=\"/etc/hostname\"/></tool_"}), None),
            frame(json!({"content":"calls>After"}), Some("stop")),
        ],
        done_sentinel: true,
    })
    .await;
    assert_eq!(reason(&qwen), StopReason::ToolUse);
    assert_eq!(joined(&qwen, "thinking", "thinking"), WireString::from("plan"));
    assert_eq!(joined(&qwen, "text", "text"), WireString::from("BeforeAfter"));
    let blocks = collect(&qwen.factory, &get(&qwen.factory, &qwen.terminal, "content"));
    let calls =
        blocks.iter().filter(|block| text(&qwen.factory, block, "type").equals_ascii("toolCall")).collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(text(&qwen.factory, calls[0], "name"), WireString::from("read"));
    assert_eq!(
        text(&qwen.factory, &get(&qwen.factory, calls[0], "arguments"), "path"),
        WireString::from("/etc/hostname")
    );
    assert_eq!(
        qwen.events.iter().filter(|event| matches!(event, AssistantMessageEvent::ToolcallStart { .. })).count(),
        1
    );
    assert_eq!(
        qwen.events.iter().filter(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })).count(),
        1
    );

    let kimi_leak = "<think>duplicate</think>prefix<|tool_calls_section_begin|><|tool_call_begin|>functions.read:0<|tool_call_argument_begin|>{\"path\":\"discarded\"}<|tool_call_end|><|tool_calls_section_end|>suffix";
    let structured = run(ChatObservationCase {
        compat: OpenAICompat { stream_markup_healing_pattern: Some(HealingPattern::Kimi), reasoning_deltas_may_be_cumulative: true, ..Default::default() },
        frames: vec![
            frame(json!({"reasoning_content":"plan","content":kimi_leak,"tool_calls":[{"index":0,"id":"original-structured","function":{"name":"read","arguments":"{\"path\":\"kept\"}"}}]}), None),
            frame(json!({"reasoning_content":"plan","content":"answer"}), None),
            frame(json!({"reasoning_content":"plan more"}), None),
            frame(json!({"reasoning_content":17,"reasoning":"alternate"}), Some("stop")),
        ], done_sentinel: true,
    }).await;
    assert_eq!(reason(&structured), StopReason::ToolUse);
    assert_eq!(joined(&structured, "thinking", "thinking"), WireString::from("plan morealternate"));
    assert_eq!(joined(&structured, "text", "text"), WireString::from("prefixsuffixanswer"));
    let blocks = collect(&structured.factory, &get(&structured.factory, &structured.terminal, "content"));
    let calls = blocks
        .iter()
        .filter(|block| text(&structured.factory, block, "type").equals_ascii("toolCall"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(text(&structured.factory, calls[0], "id"), WireString::from("original-structured"));
    assert_eq!(
        text(&structured.factory, &get(&structured.factory, calls[0], "arguments"), "path"),
        WireString::from("kept")
    );

    let stripped = run(ChatObservationCase {
        compat: OpenAICompat { strip_deepseek_special_tokens: true, ..Default::default() },
        frames: vec![
            frame(json!({"content":"<|begin_of_sentence|>  answer<|im"}), None),
            frame(json!({"content":"end|> \n"}), Some("stop")),
        ],
        done_sentinel: true,
    })
    .await;
    assert_eq!(reason(&stripped), StopReason::Stop);
    assert_eq!(joined(&stripped, "text", "text"), WireString::from("answer"));

    let empty_length = run(ChatObservationCase {
        compat: OpenAICompat {
            stream_markup_healing_pattern: Some(HealingPattern::Thinking),
            empty_length_finish_is_context_error: true,
            ..Default::default()
        },
        frames: vec![frame(json!({"content":"<think>only plan</think>"}), Some("length"))],
        done_sentinel: true,
    })
    .await;
    assert_eq!(reason(&empty_length), StopReason::Error);
    assert_eq!(joined(&empty_length, "thinking", "thinking"), WireString::from("only plan"));
    assert!(text(&empty_length.factory, &empty_length.terminal, "errorMessage").to_utf8().unwrap().contains(
        "Model returned no content: prompt filled the context window; raise Ollama num_ctx or shorten the prompt."
    ));

    let eof = run(ChatObservationCase {
        compat: OpenAICompat { stream_markup_healing_pattern: Some(HealingPattern::Thinking), ..Default::default() },
        frames: vec![frame(json!({"content":"<think>unfinished"}), None)],
        done_sentinel: false,
    })
    .await;
    assert_eq!(reason(&eof), StopReason::Error);
    assert_eq!(joined(&eof, "thinking", "thinking"), WireString::from("unfinished"));
    assert_eq!(
        eof.events.iter().filter(|event| matches!(event, AssistantMessageEvent::ThinkingStart { .. })).count(),
        1
    );
    assert_eq!(eof.events.iter().filter(|event| matches!(event, AssistantMessageEvent::ThinkingEnd { .. })).count(), 1);
}
