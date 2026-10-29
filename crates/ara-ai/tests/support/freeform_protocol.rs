//! Fixed OMP shared freeform cases, exercised by both real HTTP transports.
//! Source: grammar.ts, openai-shared.ts 2700-2742/3088-3111/3311-3332/
//! 3451-3499, and conditional convertTools/buildResponsesInput at 596f2da.

use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_responses::RequestOptions;
use ara_ai::{
    AssistantMessage, AssistantStream, Context, CustomToolFormat, CustomToolSyntax, Message, StopReason, Tool,
    ToolChoice, ToolResultMessage, UserBlock, UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

type StartStream = dyn Fn(String, Context, RequestOptions, CancellationToken) -> AssistantStream;

fn tools() -> Vec<Tool> {
    vec![
        Tool { name: "edit".into(), description: "Edit input".into(),
            parameters: json!({"type":"object","properties":{"input":{"type":"string"}},"required":["input"]}),
            custom_format: Some(CustomToolFormat { syntax: CustomToolSyntax::Lark,
                definition: "// removed\r\n  start: \"// literal\" /a\\/b/ // trailing\u{a0}\r\n\t\r\nTOKEN: 'quoted // value' /x\\/y/   ".into() }),
            custom_wire_name: Some("apply_patch".into()), ..Default::default() },
        Tool { name: "regex_edit".into(), description: "Regex input".into(),
            parameters: json!({"type":"object","properties":{"input":{"type":"string"}}}),
            custom_format: Some(CustomToolFormat { syntax: CustomToolSyntax::Regex,
                definition: "  (?:a|b)//data\n  ".into() }), ..Default::default() },
        Tool { name: "read".into(), description: "Read path".into(),
            parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
            ..Default::default() },
    ]
}

fn text_response() -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"message","id":"msg_ok","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_ok","status":"completed"}}}
    ]})
}

const RAW_DELTA: &str = "*** Begin Patch\n@@ invalid JSON {\" quoted\n+中文 \\ path\n*** End Patch\n";

fn interleaved_response() -> Value {
    json!({"events":[
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_edit","call_id":"call_edit","name":"apply_patch","input":""}}},
        {"data":{"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_read","call_id":"call_read","name":"read","arguments":""}}},
        {"data":{"type":"response.custom_tool_call_input.delta","item_id":"fc_call_edit","delta":RAW_DELTA}},
        {"data":{"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":\"delta\"}"}},
        {"data":{"type":"response.custom_tool_call_input.done","output_index":0,"input":"input.done preview"}},
        {"data":{"type":"response.function_call_arguments.done","output_index":1,"arguments":"{\"path\":\"explicit.done\"}"}},
        {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","id":"fc_read","call_id":"call_read","name":"read","arguments":"{\"path\":\"item.done\"}"}}},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_edit","call_id":"call_edit","name":"apply_patch","input":"item.done conflict"}}},
        {"data":{"type":"response.output_item.added","output_index":2,"item":{"type":"custom_tool_call","id":"ctc_empty","call_id":"call_empty","name":"apply_patch","input":""}}},
        {"data":{"type":"response.custom_tool_call_input.done","output_index":2,"input":"empty-buffer preview"}},
        {"data":{"type":"response.output_item.done","output_index":2,"item":{"type":"custom_tool_call","id":"ctc_empty","call_id":"call_empty","name":"apply_patch","input":"empty-buffer item fallback"}}},
        {"data":{"type":"response.output_item.added","output_index":3,"item":{"type":"custom_tool_call","id":"ctc_pending","call_id":"call_pending","name":"apply_patch","input":""}}},
        {"data":{"type":"response.custom_tool_call_input.delta","output_index":3,"delta":"pending delta"}},
        {"data":{"type":"response.custom_tool_call_input.done","output_index":3,"input":"pending explicit.done"}},
        {"data":{"type":"response.completed","response":{"id":"resp_tools","status":"completed"}}}
    ]})
}

async fn drain(
    mut stream: AssistantStream,
    cancel_on_delta: Option<CancellationToken>,
) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.recv().await {
            if matches!(event, AssistantMessageEvent::ToolcallDelta { .. })
                && let Some(cancel) = &cancel_on_delta
            {
                cancel.cancel();
            }
            events.push(event);
        }
    })
    .await
    .expect("freeform protocol stream must settle");
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    let output = events.last().expect("terminal").partial().clone();
    (events, output)
}

async fn server(responses: Vec<Value>) -> FakeUpstream {
    let script: Script = serde_json::from_value(json!({"responses":responses})).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}

fn request(capability: Option<&str>, choice: &str) -> RequestOptions {
    RequestOptions {
        apply_patch_tool_type: capability.map(str::to_owned),
        tool_choice: Some(ToolChoice::Tool(choice.into())),
        ..Default::default()
    }
}

fn base_context() -> Context {
    Context {
        messages: vec![Message::User(UserMessage::text("edit the fixture"))],
        tools: Some(tools()),
        ..Default::default()
    }
}

/// Each existing protocol family executes the same complete wire contract.
pub async fn verify(start: &StartStream) {
    // Optional metadata is backward-readable by existing journals. Ordinary
    // Chat still advertises functions and replays the received name unchanged.
    let old_tool = json!({"name":"read","description":"read","parameters":{"type":"object"}});
    let decoded: Tool = serde_json::from_value(old_tool.clone()).unwrap();
    assert!(decoded.custom_format.is_none() && decoded.custom_wire_name.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), old_tool);
    let old_call = json!({"id":"old","name":"read","arguments":{"path":"fixture"}});
    let decoded: ara_ai::ToolCall = serde_json::from_value(old_call.clone()).unwrap();
    assert!(decoded.custom_wire_name.is_none());
    assert_eq!(serde_json::to_value(decoded).unwrap(), old_call);
    let mut previous = AssistantMessage::empty("openai-responses", "old-provider", "old-model");
    let call:ara_ai::ToolCall=serde_json::from_value(json!({"id":"call_chat|ctc_chat","name":"apply_patch","arguments":{"input":RAW_DELTA},"customWireName":"apply_patch"})).unwrap();
    assert_eq!(serde_json::to_value(&call).unwrap()["customWireName"], "apply_patch");
    previous.content.push(ara_ai::AssistantBlock::ToolCall(call));
    previous.stop_reason = StopReason::ToolUse;
    let chat_model = ara_ai::Model {
        id: "chat-model".into(),
        api: "openai-completions".into(),
        provider: "custom".into(),
        base_url: "https://chat.example/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let chat_context = Context {
        tools: Some(tools()),
        messages: vec![
            Message::Assistant(previous),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "call_chat|ctc_chat".into(),
                tool_name: "apply_patch".into(),
                content: vec![UserBlock::text("receipt")],
                is_error: false,
                details: None,
                timestamp: 0,
                provider_metadata: None,
                useless: false,
            }),
        ],
        ..Default::default()
    };
    let chat =
        ara_ai::providers::openai_completions::build_params(&chat_model, &chat_context, &Default::default()).unwrap();
    assert_eq!(chat["tools"][0]["type"], "function");
    assert_eq!(chat["tools"][0]["function"]["name"], "edit");
    assert_eq!(chat["messages"][0]["tool_calls"][0]["function"]["name"], "apply_patch");
    assert_eq!(
        serde_json::from_str::<Value>(chat["messages"][0]["tool_calls"][0]["function"]["arguments"].as_str().unwrap())
            .unwrap(),
        json!({"input":RAW_DELTA})
    );
    // Capability is exact catalog metadata; neither endpoint nor spelling guesses.
    for (capability, choice) in [
        (None, "edit"),
        (Some("function"), "edit"),
        (Some("Freeform"), "edit"),
        (Some("freeform"), "edit"),
        (Some("freeform"), "apply_patch"),
    ] {
        let endpoint = server(vec![text_response()]).await;
        let (_, output) = drain(
            start(endpoint.base_url(), base_context(), request(capability, choice), CancellationToken::new()),
            None,
        )
        .await;
        assert_eq!(output.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let body = &recorded[0]["body"];
        let freeform = capability == Some("freeform");
        assert_eq!(body["tools"][0]["type"], if freeform { "custom" } else { "function" });
        assert_eq!(body["tools"][0]["name"], if freeform { "apply_patch" } else { "edit" });
        assert_eq!(
            body["tool_choice"],
            json!({"type":if freeform {"custom"} else {"function"},"name":if freeform {"apply_patch"} else {"edit"}})
        );
        assert_eq!(body["tools"][2]["type"], "function");
        assert_eq!(body["tools"][2]["name"], "read");
        if freeform {
            assert_eq!(
                body["tools"][0]["format"],
                json!({"type":"grammar","syntax":"lark","definition":"  start: \"// literal\" /a\\/b/\nTOKEN: 'quoted // value' /x\\/y/"})
            );
            assert_eq!(body["tools"][1]["format"]["definition"], "  (?:a|b)//data\n  ");
            assert_eq!(body["tools"][1]["name"], "regex_edit");
            assert!(body["tools"][0].get("parameters").is_none());
        } else {
            assert_eq!(body["tools"][0]["parameters"]["properties"]["input"]["type"], "string");
            assert!(body["tools"][0].get("format").is_none());
        }
    }

    let endpoint =
        server(std::iter::once(interleaved_response()).chain(std::iter::repeat_with(text_response).take(10)).collect())
            .await;
    let (events, output) = drain(
        start(endpoint.base_url(), base_context(), request(Some("freeform"), "edit"), CancellationToken::new()),
        None,
    )
    .await;
    assert_eq!(output.stop_reason, StopReason::ToolUse);
    let calls = output.tool_calls().cloned().collect::<Vec<_>>();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[0].name, "apply_patch");
    assert_eq!(calls[0].custom_wire_name.as_deref(), Some("apply_patch"));
    assert_eq!(calls[0].arguments, json!({"input":RAW_DELTA}).as_object().unwrap().clone());
    assert_eq!(calls[1].arguments, json!({"path":"explicit.done"}).as_object().unwrap().clone());
    assert!(calls[1].custom_wire_name.is_none());
    assert_eq!(calls[2].arguments["input"], "empty-buffer item fallback");
    assert_eq!(calls[3].arguments["input"], "pending explicit.done");
    let ended = events
        .iter()
        .filter_map(|event| match event {
            AssistantMessageEvent::ToolcallEnd { tool_call, .. } => Some(tool_call),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ended.len(), 4);
    assert_eq!(ended.iter().find(|call| call.id == calls[0].id).unwrap().arguments, calls[0].arguments);
    assert!(
        events.iter().any(|event| matches!(event,AssistantMessageEvent::ToolcallEnd {tool_call,partial,..}
        if tool_call.name=="read" && partial.tool_calls().next().unwrap().arguments["input"]=="input.done preview")),
        "Native input.done is visible before output_item.done reapplies the retained buffer"
    );
    assert!(events.iter().any(|event| matches!(event,AssistantMessageEvent::ToolcallDelta {delta,partial,..}
        if delta==RAW_DELTA && partial.tool_calls().next().unwrap().arguments["input"]==RAW_DELTA)));
    let payload = output.provider_payload.as_ref().expect("native output payload");
    let native_items = payload["items"].as_array().unwrap();
    assert_eq!(native_items.iter().find(|item| item["call_id"] == "call_edit").unwrap()["input"], RAW_DELTA);
    assert_eq!(
        native_items.iter().find(|item| item["call_id"] == "call_pending").unwrap()["input"],
        "pending explicit.done"
    );

    // Native incremental, readable fallback, cross-model fallback and full
    // snapshots retain call/result kind together when the next model changes.
    for variant in ["incremental", "readable", "different-model", "full", "wrong-kind-full"] {
        for capability in [Some("freeform"), Some("function")] {
            let mut history = output.clone();
            if variant == "readable" {
                // Native Codex has no cold-history option; its actual readable
                // fallback is selected by the absence of provider history.
                history.provider_payload = None;
            }
            if variant == "different-model" {
                history.model = "previous-model".into();
            }
            if matches!(variant, "full" | "wrong-kind-full") {
                let mut items = native_items.clone();
                if variant == "wrong-kind-full" {
                    items.push(json!({"type":"function_call_output","call_id":"call_edit","output":"wrong kind"}));
                }
                history.provider_payload.as_mut().unwrap()["dt"] = json!(false);
                history.provider_payload.as_mut().unwrap()["items"] = json!(items);
            }
            let mut context = base_context();
            context.messages.push(Message::Assistant(history));
            for call in &calls {
                context.messages.push(Message::ToolResult(ToolResultMessage {
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: vec![UserBlock::text("effect receipt")],
                    is_error: false,
                    details: None,
                    timestamp: 0,
                    provider_metadata: None,
                    useless: false,
                }));
            }
            context.messages.push(Message::User(UserMessage::text("continue")));
            let opts = request(capability, "edit");
            let (_, final_message) =
                drain(start(endpoint.base_url(), context, opts, CancellationToken::new()), None).await;
            assert_eq!(final_message.text(), "ok", "{variant}");
            let recorded = endpoint.requests.lock().await;
            let input = recorded.last().unwrap()["body"]["input"].as_array().unwrap();
            let custom = capability == Some("freeform");
            for original in &calls {
                let id = original.id.split('|').next().unwrap();
                let call = input
                    .iter()
                    .find(|item| {
                        item["call_id"] == id
                            && matches!(item["type"].as_str(), Some("function_call" | "custom_tool_call"))
                    })
                    .unwrap();
                let result = input
                    .iter()
                    .find(|item| {
                        item["call_id"] == id
                            && matches!(item["type"].as_str(), Some("function_call_output" | "custom_tool_call_output"))
                    })
                    .unwrap();
                let is_custom = custom && original.custom_wire_name.is_some();
                assert_eq!(call["type"], if is_custom { "custom_tool_call" } else { "function_call" }, "{variant}");
                assert_eq!(
                    result["type"],
                    if is_custom { "custom_tool_call_output" } else { "function_call_output" },
                    "{variant}"
                );
                if original.custom_wire_name.is_some() {
                    assert_eq!(call["name"], if custom { "apply_patch" } else { "edit" });
                    if custom {
                        assert_eq!(call["input"], original.arguments["input"]);
                    } else {
                        assert_eq!(
                            serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
                            json!(original.arguments)
                        );
                    }
                }
            }
            assert!(!input.iter().any(|item| item["output"] == "wrong kind"));
        }
    }

    // Native max-output-token promotion admits custom calls only with explicit
    // input.done, even when a compatibility proxy emits output_item.done.
    for explicit_done in [false, true] {
        let mut events = vec![
            json!({"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_length","call_id":"call_length","name":"apply_patch","input":""}}}),
            json!({"data":{"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"raw length input"}}),
        ];
        if explicit_done {
            events.push(json!({"data":{"type":"response.custom_tool_call_input.done","output_index":0,"input":"raw length input"}}));
        }
        events.push(json!({"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_length","call_id":"call_length","name":"apply_patch","input":"raw length input"}}}));
        events.push(json!({"data":{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}}));
        let endpoint = server(vec![json!({"events":events})]).await;
        let (_, output) = drain(
            start(endpoint.base_url(), base_context(), request(Some("freeform"), "edit"), CancellationToken::new()),
            None,
        )
        .await;
        assert_eq!(output.stop_reason, if explicit_done { StopReason::ToolUse } else { StopReason::Length });
        assert_eq!(output.tool_calls().next().unwrap().arguments["input"], "raw length input");
    }

    // Native custom lookup: a wrong-kind prefixed alias cannot shadow an
    // exact custom id, identifierless input follows the most recently added
    // item, and closing it does not fall back to an older open sibling.
    let routed = json!({"events":[
        {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"x","name":"read","arguments":""}}},
        {"data":{"type":"response.output_item.added","output_index":1,"item":{"type":"custom_tool_call","id":"fc_x","call_id":"y","name":"apply_patch","input":""}}},
        {"data":{"type":"response.custom_tool_call_input.delta","item_id":"fc_x","delta":"alias raw"}},
        {"data":{"type":"response.custom_tool_call_input.done","item_id":"fc_x","input":"alias raw"}},
        {"data":{"type":"response.output_item.done","output_index":2,"item":{"type":"message","id":"msg_done_only","role":"assistant","content":[{"type":"output_text","text":"done-only sibling"}]}}},
        {"data":{"type":"response.custom_tool_call_input.delta","delta":" + latest"}},
        {"data":{"type":"response.custom_tool_call_input.done","input":"alias raw + latest"}},
        {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"custom_tool_call","id":"fc_x","call_id":"y","name":"apply_patch","input":""}}},
        {"data":{"type":"response.custom_tool_call_input.delta","item_id":"fc_x","delta":" stale keyed"}},
        {"data":{"type":"response.custom_tool_call_input.done","item_id":"fc_x","input":"stale keyed"}},
        {"data":{"type":"response.custom_tool_call_input.delta","delta":" closed latest"}},
        {"data":{"type":"response.custom_tool_call_input.done","input":"closed latest"}},
        {"data":{"type":"response.output_item.added","output_index":3,"item":{"type":"custom_tool_call","id":"ctc_next","call_id":"z","name":"apply_patch","input":""}}},
        {"data":{"type":"response.custom_tool_call_input.delta","delta":"second latest"}},
        {"data":{"type":"response.custom_tool_call_input.done","input":"second latest"}},
        {"data":{"type":"response.output_item.done","output_index":3,"item":{"type":"custom_tool_call","id":"ctc_next","call_id":"z","name":"apply_patch","input":""}}},
        {"data":{"type":"response.function_call_arguments.done","output_index":0,"arguments":"{\"path\":\"a.txt\"}"}},
        {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_a","call_id":"x","name":"read","arguments":"{}"}}},
        {"data":{"type":"response.incomplete","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}}
    ]});
    let endpoint = server(vec![routed]).await;
    let (events, output) = drain(
        start(endpoint.base_url(), base_context(), request(Some("freeform"), "edit"), CancellationToken::new()),
        None,
    )
    .await;
    assert_eq!(output.stop_reason, StopReason::ToolUse);
    let calls = output.tool_calls().collect::<Vec<_>>();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].arguments["path"], "a.txt");
    assert_eq!(calls[1].arguments["input"], "alias raw + latest");
    assert_eq!(calls[2].arguments["input"], "second latest");
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                AssistantMessageEvent::ToolcallDelta { content_index, .. } => Some(*content_index),
                _ => None,
            })
            .collect::<Vec<_>>(),
        [1, 1, 3]
    );
    assert_eq!(events.iter().filter(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })).count(), 3);

    // Raw custom input is not JSON, but it still needs an explicit completed
    // invocation. Protocol failure/disconnect/cancel never produces an end.
    let added = json!({"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"custom_tool_call","id":"ctc_fault","call_id":"call_fault","name":"apply_patch","input":""}}});
    let delta = json!({"data":{"type":"response.custom_tool_call_input.delta","output_index":0,"delta":"unfinished raw input"}});
    let terminal = json!({"data":{"type":"response.completed","response":{"status":"completed"}}});
    let cases = vec![
        (vec![added.clone(), delta.clone(), terminal.clone()], "close", false),
        (
            vec![
                added.clone(),
                json!({"data":{"type":"response.custom_tool_call_input.done","output_index":0,"input":17}}),
            ],
            "close",
            false,
        ),
        (
            vec![
                added.clone(),
                json!({"data":{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{}"}}),
            ],
            "close",
            false,
        ),
        (
            vec![
                added.clone(),
                json!({"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"ctc_fault","call_id":"call_fault","name":"apply_patch","arguments":"{}"}}}),
            ],
            "close",
            false,
        ),
        (vec![added.clone(), delta.clone()], "close", false),
        (vec![added, delta], "hang", true),
    ];
    for (data, end, cancelled) in cases {
        let endpoint = server(vec![json!({"events":data,"end":end}), text_response()]).await;
        let cancel = CancellationToken::new();
        let (events, output) = drain(
            start(endpoint.base_url(), base_context(), request(Some("freeform"), "edit"), cancel.clone()),
            cancelled.then_some(cancel),
        )
        .await;
        assert_eq!(output.stop_reason, if cancelled { StopReason::Aborted } else { StopReason::Error });
        assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
        assert!(output.provider_payload.is_none());
        assert_eq!(endpoint.served(), 1, "unsafe tool boundary must not replay");
    }
}
