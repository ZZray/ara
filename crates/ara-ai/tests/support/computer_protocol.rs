//! Grouped fixed Native computer and pause-producer wire contracts at 596f2da.
//! Generic AI transport fixture; this does not port the coding-agent OS executor.
use ara_ai::event::AssistantMessageEvent;
use ara_ai::providers::openai_responses::RequestOptions;
use ara_ai::{
    AssistantMessage, AssistantStream, Context, CustomToolFormat, CustomToolSyntax, Message, Model, NativeToolMarker,
    StopReason, Tool, ToolChoice, ToolResultMessage, ToolResultProviderMetadata, UserBlock, UserMessage,
};
use ara_testkit::{FakeUpstream, Script};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

type Start = dyn Fn(String, Context, RequestOptions, CancellationToken) -> AssistantStream;

fn context() -> Context {
    Context {
        system_prompt: vec!["inspect".into()],
        messages: vec![Message::User(UserMessage::text("inspect"))],
        tools: Some(vec![
            Tool {
                name: "computer".into(),
                description: "controlled fixture".into(),
                parameters: json!({"type":"object"}),
                native: Some(NativeToolMarker::Computer),
                ..Default::default()
            },
            Tool { name: "read".into(), parameters: json!({"type":"object"}), ..Default::default() },
        ]),
    }
}
fn text() -> Value {
    json!({"events":[{"data":{"type":"response.output_item.done","output_index":0,"item":{
        "type":"message","id":"msg_final","role":"assistant","content":[{"type":"output_text","text":"ok"}]}}},
        {"data":{"type":"response.completed","response":{"id":"resp_final","status":"completed"}}}]})
}
async fn server(responses: Vec<Value>) -> FakeUpstream {
    let script: Script = serde_json::from_value(json!({"responses":responses})).unwrap();
    FakeUpstream::start(script, None).await.unwrap()
}
async fn drain(mut stream: AssistantStream) -> (Vec<AssistantMessageEvent>, AssistantMessage) {
    let mut events = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = stream.recv().await {
            events.push(event);
        }
    })
    .await
    .expect("computer stream settles");
    assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
    let output = events.last().unwrap().partial().clone();
    (events, output)
}
fn call(actions: Value) -> Value {
    json!({"type":"computer_call","id":"cu_item","call_id":"cu_call","actions":actions,
        "pending_safety_checks":[{"id":"check","code":null,"message":null}],"status":"completed"})
}
fn response(item: Value, added: bool, done: bool, length: bool) -> Value {
    let mut events = Vec::new();
    if added {
        events.push(json!({"data":{"type":"response.output_item.added","output_index":0,"item":item}}));
    }
    if done {
        events.push(json!({"data":{"type":"response.output_item.done","output_index":0,"item":item}}));
    }
    events.push(if length {
        json!({"data":{"type":"response.incomplete","response":{
        "id":"resp_computer","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}})
    } else {
        json!({"data":{"type":"response.completed","response":{"id":"resp_computer","status":"completed"}}})
    });
    json!({"events":events})
}

/// Groups A/B/C reuse this existing protocol target; no per-row build/target.
pub async fn verify(start: &Start, model_for: &dyn Fn(&str) -> Model, codex: bool) {
    verify_pause(start, codex).await;
    let include = "computer_call_output.output.image_url";
    for (choice, expected) in [
        (ToolChoice::Auto, json!("auto")),
        (ToolChoice::None, json!("none")),
        (ToolChoice::Any, json!(if codex { "any" } else { "required" })),
        (ToolChoice::Required, json!("required")),
        (ToolChoice::Function("read".into()), json!({"type":"function","name":"read"})),
        (ToolChoice::FunctionNested("read".into()), json!({"type":"function","name":"read"})),
        (ToolChoice::Tool("read".into()), json!({"type":"function","name":"read"})),
        (ToolChoice::Computer, json!({"type":"computer"})),
    ] {
        let endpoint = server(vec![text()]).await;
        let (_, out) = drain(start(
            endpoint.base_url(),
            context(),
            RequestOptions {
                supports_computer_use: true,
                tool_choice: Some(choice),
                include: vec![include.into(), include.into()],
                ..Default::default()
            },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let body = &recorded[0]["body"];
        assert_eq!(body["tools"][0], json!({"type":"computer"}));
        assert_eq!(body["tool_choice"], expected);
        assert_eq!(body["include"].as_array().unwrap().iter().filter(|v| **v == json!(include)).count(), 1);
        assert!(!body.to_string().contains("display_width"));
        assert!(!body.to_string().contains("display_height"));
    }
    for (supported, marker, lite) in
        [(false, true, false), (true, false, false), (true, true, true), (false, true, true)]
    {
        if lite && !codex {
            continue;
        }
        let endpoint = server(vec![text()]).await;
        let mut ctx = context();
        if !marker {
            ctx.tools.as_mut().unwrap()[0].native = None;
        }
        let (_, out) = drain(start(
            endpoint.base_url(),
            ctx,
            RequestOptions {
                supports_computer_use: supported,
                responses_lite: lite,
                tool_choice: Some(ToolChoice::Computer),
                ..Default::default()
            },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let body = &recorded[0]["body"];
        if lite {
            assert_eq!(body["input"][0]["type"], "additional_tools");
            assert_eq!(body["input"][0]["tools"].as_array().unwrap().len(), 1);
            assert_eq!(body["tool_choice"], "required");
            assert_eq!(body["parallel_tool_calls"], false);
            assert_eq!(body["reasoning"]["context"], "all_turns");
            assert_eq!(recorded[0]["headers"]["x-openai-internal-codex-responses-lite"], "true");
            assert!(body.get("instructions").is_none() && body.get("tools").is_none());
        } else if !marker {
            assert!(body.get("tool_choice").is_none());
        } else {
            assert_eq!(body["tool_choice"], json!({"type":"function","name":"computer"}));
        }
    }
    // Native retains its provider-specific marker priority even when the
    // unsupported marker definition is emitted as a freeform custom tool.
    for supported in [false, true] {
        for choice in [
            ToolChoice::Computer,
            ToolChoice::Function("desktop".into()),
            ToolChoice::FunctionNested("desktop_wire".into()),
            ToolChoice::Tool("desktop".into()),
        ] {
            let computer_choice = choice == ToolChoice::Computer;
            let endpoint = server(vec![text()]).await;
            let mut ctx = context();
            let marker = &mut ctx.tools.as_mut().unwrap()[0];
            marker.name = "desktop".into();
            marker.custom_wire_name = Some("desktop_wire".into());
            marker.custom_format =
                Some(CustomToolFormat { syntax: CustomToolSyntax::Lark, definition: "start: /.+/".into() });
            let (_, out) = drain(start(
                endpoint.base_url(),
                ctx,
                RequestOptions {
                    supports_computer_use: supported,
                    apply_patch_tool_type: Some("freeform".into()),
                    tool_choice: Some(choice),
                    ..Default::default()
                },
                CancellationToken::new(),
            ))
            .await;
            assert_eq!(out.text(), "ok");
            let recorded = endpoint.requests.lock().await;
            let body = &recorded[0]["body"];
            if supported {
                assert_eq!(body["tools"][0], json!({"type":"computer"}));
                assert_eq!(body["tool_choice"], json!({"type":"computer"}));
            } else {
                assert_eq!(body["tools"][0]["type"], "custom");
                assert_eq!(body["tools"][0]["name"], "desktop_wire");
                assert_eq!(
                    body["tool_choice"],
                    if codex && !computer_choice {
                        json!({"type":"custom","name":"desktop_wire"})
                    } else {
                        json!({"type":"function","name":"desktop"})
                    }
                );
            }
        }
    }

    if !codex {
        for choice in [ToolChoice::Computer, ToolChoice::Tool("computer".into())] {
            let endpoint = server(vec![text()]).await;
            let mut ctx = context();
            ctx.tools.as_mut().unwrap()[0].parameters = json!({"type":"object","properties":{
                "value":{"type":"null","const":"not null"}}});
            let (_, out) = drain(start(
                endpoint.base_url(),
                ctx,
                RequestOptions { supports_computer_use: false, tool_choice: Some(choice), ..Default::default() },
                CancellationToken::new(),
            ))
            .await;
            assert_eq!(out.text(), "ok");
            let recorded = endpoint.requests.lock().await;
            let body = &recorded[0]["body"];
            assert_eq!(body["tools"].as_array().unwrap().len(), 1);
            assert_eq!(body["tools"][0]["name"], "read");
            assert!(body.get("tool_choice").is_none(), "quarantined Computer marker cannot be selected");
        }
    }

    let actions = json!([{"type":"screenshot","extra":{"retained":true}},{"type":"type","text":"abc"}]);
    for (added, done, length, empty) in [
        (true, true, false, false),
        (false, true, false, false),
        (true, false, true, false),
        (true, true, true, false),
        (true, false, true, true),
        (true, true, false, true),
    ] {
        let item = call(if empty { json!([]) } else { actions.clone() });
        let endpoint = server(vec![response(item.clone(), added, done, length)]).await;
        let (events, out) = drain(start(
            endpoint.base_url(),
            context(),
            RequestOptions { supports_computer_use: true, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.stop_reason, if empty && length { StopReason::Length } else { StopReason::ToolUse });
        let emitted = out.tool_calls().next().unwrap();
        assert_eq!(emitted.name, "computer");
        assert!(emitted.arguments.is_empty());
        assert_eq!(emitted.id, "cu_call|cu_item");
        assert_eq!(emitted.provider_metadata.is_some(), !empty);
        assert_eq!(
            events.iter().filter(|e| matches!(e, AssistantMessageEvent::ToolcallStart { .. })).count(),
            usize::from(added)
        );
        assert_eq!(
            events.iter().filter(|e| matches!(e, AssistantMessageEvent::ToolcallEnd { .. })).count(),
            usize::from(done)
        );
        assert_eq!(out.provider_payload.as_ref().unwrap()["items"][0], item);
    }
    let mut legacy = call(json!([]));
    legacy.as_object_mut().unwrap().remove("actions");
    legacy["action"] = json!({"type":"screenshot"});
    let endpoint = server(vec![response(legacy, true, true, false)]).await;
    let (_, legacy_out) = drain(start(
        endpoint.base_url(),
        context(),
        RequestOptions { supports_computer_use: true, ..Default::default() },
        CancellationToken::new(),
    ))
    .await;
    assert_eq!(legacy_out.stop_reason, StopReason::ToolUse);
    assert!(legacy_out.tool_calls().next().unwrap().provider_metadata.is_some());

    for file in [false, true] {
        let mut scripts = vec![response(call(actions.clone()), true, true, false), text(), text(), text()];
        if codex {
            scripts.extend([text(), text(), text(), text()]);
        }
        let endpoint = server(scripts).await;
        let (_, first) = drain(start(
            endpoint.base_url(),
            context(),
            RequestOptions { supports_computer_use: true, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        let screenshot = if file {
            json!({"type":"computer_screenshot","file_id":"file_screenshot"})
        } else {
            json!({"type":"computer_screenshot","image_url":"data:image/png;base64,AQ=="})
        };
        let result: ToolResultProviderMetadata = serde_json::from_value(json!({"type":"computer",
            "screenshot":screenshot,"acknowledgedSafetyChecks":[{"id":"check","code":null,"message":null}]}))
        .unwrap();
        let mut history = context();
        history.messages.extend([
            Message::Assistant(first),
            Message::ToolResult(ToolResultMessage {
                tool_call_id: "cu_call|cu_item".into(),
                tool_name: "computer".into(),
                content: vec![UserBlock::text("receipt")],
                details: None,
                is_error: false,
                timestamp: 0,
                provider_metadata: Some(result),
                useless: false,
            }),
        ]);
        let (_, out) = drain(start(
            endpoint.base_url(),
            history.clone(),
            RequestOptions { supports_computer_use: true, supports_images: false, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let (_, out) = drain(start(
            endpoint.base_url(),
            history.clone(),
            RequestOptions { supports_computer_use: false, supports_images: false, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let mut typed_history = history.clone();
        if let Message::Assistant(assistant) = &mut typed_history.messages[1] {
            assistant.provider_payload = None;
        } else {
            unreachable!();
        }
        let (_, out) = drain(start(
            endpoint.base_url(),
            typed_history,
            RequestOptions { supports_computer_use: false, supports_images: false, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let input = recorded[1]["body"]["input"].as_array().unwrap();
        assert!(input.iter().any(|item| item["type"] == "computer_call" && item["id"] == "cu_item"));
        let output = input.iter().find(|item| item["type"] == "computer_call_output").unwrap();
        assert_eq!(output["output"], screenshot);
        assert_eq!(output["acknowledged_safety_checks"][0]["code"], Value::Null);
        let switched = recorded[2]["body"]["input"].as_array().unwrap();
        assert!(!switched.iter().any(|item| item["type"] == "computer_call" || item["type"] == "computer_call_output"));
        if codex {
            let call = switched.iter().find(|item| item["type"] == "function_call").unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
                json!({"actions":actions})
            );
            assert!(switched.iter().any(|item| item["role"] == "user"
                && item["content"].as_array().is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part["type"] == "input_image"
                            && (part["image_url"] == screenshot["image_url"] && screenshot["image_url"].is_string()
                                || part["file_id"] == screenshot["file_id"] && screenshot["file_id"].is_string())
                    })
                })));
            assert_eq!(
                recorded[3]["body"]["input"], recorded[2]["body"]["input"],
                "Codex typed metadata retains the same action/output/image unroll"
            );
        } else {
            assert!(switched.iter().any(|item| item["content"]
                == format!("[Previous computer call; call_id=cu_call]: {}", call(actions.clone()))));
            let typed = recorded[3]["body"]["input"].as_array().unwrap();
            assert!(
                typed
                    .iter()
                    .any(|item| item["content"] == format!("[Previous computer call; call_id=cu_call]: {actions}"))
            );
            assert!(typed.iter().any(|item| item["content"]==format!(
                "[Previous computer result; call_id=cu_call]: {screenshot}")));
            assert!(
                !typed.iter().any(|item| item["type"] == "computer_call" || item["type"] == "computer_call_output")
            );
        }
        drop(recorded);
        if codex {
            for (index, authored) in
                [Some(json!(true)), Some(json!(false)), Some(Value::Null), None].into_iter().enumerate()
            {
                let mut compat = json!({});
                if let Some(value) = authored {
                    compat["supportsImageDetailOriginal"] = value;
                }
                let policy =
                    ara_ai::OpenAIReasoningPolicy::from_model_spec(&json!({"reasoning":true,"compat":compat})).unwrap();
                let (_, out) = drain(start(
                    endpoint.base_url(),
                    history.clone(),
                    RequestOptions {
                        supports_computer_use: false,
                        supports_images: false,
                        reasoning_policy: Some(policy),
                        ..Default::default()
                    },
                    CancellationToken::new(),
                ))
                .await;
                assert_eq!(out.text(), "ok");
                let recorded = endpoint.requests.lock().await;
                let image = recorded[index + 4]["body"]["input"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|item| item["content"].as_array())
                    .flatten()
                    .find(|part| part["type"] == "input_image")
                    .unwrap();
                assert_eq!(image["detail"], if index == 0 { "original" } else { "auto" });
            }
        }
    }

    // Failure, orphan and full replacement use the actual next HTTP request.
    // None may fabricate a screenshot or reconnect an obsolete computer pair.
    for mode in ["failed", "orphan", "replacement"] {
        let endpoint = server(vec![response(call(actions.clone()), true, true, false), text()]).await;
        let (_, first) = drain(start(
            endpoint.base_url(),
            context(),
            RequestOptions { supports_computer_use: true, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        let screenshot: ToolResultProviderMetadata = serde_json::from_value(json!({"type":"computer",
            "screenshot":{"type":"computer_screenshot","file_id":"file_must_not_be_replayed"},
            "acknowledgedSafetyChecks":[]}))
        .unwrap();
        let mut history = context();
        history.messages.push(Message::Assistant(first));
        if mode == "replacement" {
            let route = model_for(&endpoint.base_url());
            let mut replacement = AssistantMessage::empty(&route.api, &route.provider, &route.id);
            replacement.provider_payload = ara_ai::remote_compaction::native_replacement_payload(
                &route,
                &[json!({"type":"compaction","encrypted_content":"new-history-only"})],
            );
            assert!(replacement.provider_payload.is_some());
            history.messages.push(Message::Assistant(replacement));
        }
        history.messages.push(Message::ToolResult(ToolResultMessage {
            tool_call_id: if mode == "orphan" { "never_seen|orphan_item".into() } else { "cu_call|cu_item".into() },
            tool_name: "computer".into(),
            content: vec![UserBlock::text("controlled failure receipt")],
            details: None,
            is_error: mode == "failed",
            timestamp: 0,
            provider_metadata: if mode == "failed" { None } else { Some(screenshot) },
            useless: false,
        }));
        let (_, out) = drain(start(
            endpoint.base_url(),
            history,
            RequestOptions { supports_computer_use: true, supports_images: true, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        let recorded = endpoint.requests.lock().await;
        let input = recorded[1]["body"]["input"].as_array().unwrap();
        assert!(!input.iter().any(|item| item["type"] == "computer_call" || item["type"] == "computer_call_output"));
        assert!(!input.iter().any(|item| item.to_string().contains("file_must_not_be_replayed")));
        // Fixed transformMessages drops an orphan inside an unresolved call
        // window, and moves a real result before a full replacement. The
        // orphan-note converter branch is not reached by either input.
        if mode == "failed" {
            assert!(input.iter().any(|item| item.to_string().contains("failed before a screenshot")));
            assert!(input.iter().any(|item| item.to_string().contains("controlled failure receipt")));
        } else if mode == "orphan" {
            assert!(input.iter().any(|item| item.to_string().contains("failed before a screenshot")));
            assert!(input.iter().any(|item| item.to_string().contains("No result provided")));
            assert!(!input.iter().any(|item| item.to_string().contains("controlled failure receipt")));
            assert!(!input.iter().any(|item| item.to_string().contains("never_seen")));
        } else {
            assert_eq!(mode, "replacement");
            assert_eq!(input.len(), 1, "full replacement removes the old computer pair and result");
            assert_eq!(input[0]["type"], "compaction");
            assert_eq!(input[0]["encrypted_content"], "new-history-only");
            assert!(!input.iter().any(|item| item["role"] == "user"));
            assert!(!input.iter().any(|item| item.to_string().contains("controlled failure receipt")));
        }
    }

    // Added-only EOF has protocol-specific retry admission. Finalized or
    // meaningful effects retain cancellation/failure/no-replay contracts.
    let added = json!({"data":{"type":"response.output_item.added","output_index":0,"item":call(actions.clone())}});
    let mut bad_id = call(actions.clone());
    bad_id["id"] = json!("different_item");
    let mut bad_metadata = call(actions.clone());
    bad_metadata["actions"] = json!([{"type":"click","x":"wrong","y":1}]);
    for (label, events, end, cancelled) in [
        ("added-eof", vec![added.clone()], "close", false),
        (
            "provider-failed",
            vec![
                added.clone(),
                json!({"data":{"type":"response.failed","response":{"status":"failed","error":{"message":"fixture failure"}}}}),
            ],
            "close",
            false,
        ),
        (
            "bad-id",
            vec![added.clone(), json!({"data":{"type":"response.output_item.done","output_index":0,"item":bad_id}})],
            "close",
            false,
        ),
        (
            "bad-metadata",
            vec![
                added.clone(),
                json!({"data":{"type":"response.output_item.done","output_index":0,"item":bad_metadata}}),
            ],
            "close",
            false,
        ),
        // b91 buffers ToolcallStart until meaningful output. Publish a text
        // checkpoint so consumer cancellation occurs before the idle deadline.
        (
            "cancel-after-publish",
            vec![
                added.clone(),
                json!({"data":{"type":"response.output_item.added","output_index":1,"item":{
                    "type":"message","id":"cancel_checkpoint","role":"assistant","content":[]}}}),
                json!({"data":{"type":"response.output_text.delta","output_index":1,"content_index":0,"delta":"checkpoint"}}),
            ],
            "hang",
            true,
        ),
    ] {
        let endpoint = server(vec![json!({"events":events,"end":end}), text()]).await;
        let cancel = CancellationToken::new();
        let mut stream = start(
            endpoint.base_url(),
            context(),
            RequestOptions { supports_computer_use: true, ..Default::default() },
            cancel.clone(),
        );
        let mut events = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.recv().await {
                if cancelled && matches!(event, AssistantMessageEvent::ToolcallStart { .. }) {
                    cancel.cancel();
                }
                events.push(event);
            }
        })
        .await
        .expect("fault stream settles");
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
        if label == "added-eof" && !codex {
            // Fixed b91 Compatible has not committed an added-only computer
            // marker. Its one safe retry discards that dead attempt entirely.
            let output = events.last().unwrap().partial();
            assert_eq!(output.stop_reason, StopReason::Stop);
            assert_eq!(output.text(), "ok");
            assert!(output.tool_calls().next().is_none());
            assert!(!events.iter().any(|event| matches!(event,
                AssistantMessageEvent::ToolcallStart { .. } | AssistantMessageEvent::ToolcallEnd { .. })));
            assert!(!output.provider_payload.as_ref().is_some_and(|payload| payload.to_string().contains("cu_item")));
            assert_eq!(endpoint.served(), 2, "only an uncommitted Compatible EOF retries");
            continue;
        }
        assert_eq!(
            events.last().unwrap().partial().stop_reason,
            if cancelled { StopReason::Aborted } else { StopReason::Error }
        );
        assert!(!events.iter().any(|event| matches!(event, AssistantMessageEvent::ToolcallEnd { .. })));
        assert!(events.last().unwrap().partial().provider_payload.is_none());
        assert_eq!(endpoint.served(), 1, "computer effect boundary does not replay");
    }

    // Real User and Developer DTO ingress, with a Host-confirmed producing
    // route. Raw JSON is preserved; serialization deliberately removes proof.
    for role in ["user", "developer"] {
        let endpoint = server(vec![text(), text(), text(), text(), text()]).await;
        let route = model_for(&endpoint.base_url());
        let payload = json!({"type":"openaiResponsesHistory","provider":route.provider,"items":[
            {"type":"reasoning","id":"rs_mixed","summary":[],"encrypted_content":"mixed-reasoning"},
            {"type":"message","id":"msg_mixed","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"Inspecting the screen."}]},
            {"type":"function_call","id":"fc_mixed","call_id":"call_mixed_tool","name":"read","arguments":"{}","status":"completed"},
            {"type":"computer_call","id":"cu_mixed","call_id":"call_mixed_computer","actions":[{"type":"screenshot"}],
                "pending_safety_checks":[],"status":"completed"},
            {"type":"computer_call_output","call_id":"call_mixed_computer","output":{"type":"computer_screenshot","file_id":"file_mixed"},
                "acknowledged_safety_checks":[]}
        ]});
        let raw = json!({"role":role,"content":"visible compacted fallback","providerPayload":payload,"timestamp":0});
        let mut carrier: Message = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&carrier).unwrap(), raw);
        ara_ai::providers::openai_responses::adopt_client_history_for_checked_route(&route, &mut carrier, false)
            .unwrap();
        assert_eq!(serde_json::to_value(&carrier).unwrap(), raw, "proof must not rewrite the raw payload");
        let ctx = Context { messages: vec![carrier.clone()], ..context() };
        let (_, out) = drain(start(
            endpoint.base_url(),
            ctx,
            RequestOptions { supports_images: false, supports_computer_use: false, ..Default::default() },
            CancellationToken::new(),
        ))
        .await;
        assert_eq!(out.text(), "ok");
        for mode in ["serde", "mutated", "foreign_model", "foreign_endpoint"] {
            let mut rejected = carrier.clone();
            if mode == "serde" {
                rejected = serde_json::from_value(serde_json::to_value(&rejected).unwrap()).unwrap();
            } else if mode == "mutated" {
                match &mut rejected {
                    Message::User(user) => {
                        user.provider_payload.as_mut().unwrap()["items"][1]["content"][0]["text"] = json!("edited")
                    }
                    Message::Developer(developer) => {
                        developer.provider_payload.as_mut().unwrap()["items"][1]["content"][0]["text"] = json!("edited")
                    }
                    _ => unreachable!(),
                }
            } else {
                let mut other = route.clone();
                if mode == "foreign_model" {
                    other.id = "foreign-model".into();
                } else {
                    other.base_url = "https://foreign.example.test/v1".into();
                }
                ara_ai::providers::openai_responses::adopt_client_history_for_checked_route(
                    &other,
                    &mut rejected,
                    true,
                )
                .unwrap();
            }
            let (_, out) = drain(start(
                endpoint.base_url(),
                Context { messages: vec![rejected], ..context() },
                RequestOptions { supports_images: true, supports_computer_use: true, ..Default::default() },
                CancellationToken::new(),
            ))
            .await;
            assert_eq!(out.text(), "ok");
        }
        let recorded = endpoint.requests.lock().await;
        let input = recorded[0]["body"]["input"].as_array().unwrap();
        assert!(!input.iter().any(|item| item["type"] == "computer_call" || item["type"] == "computer_call_output"));
        assert!(input.iter().any(|item| item["type"] == "reasoning"));
        if codex {
            assert!(input.iter().any(|item| item["type"] == "function_call" && item["name"] == "computer"));
            assert!(
                input.iter().all(|item| item.get("id").is_none()),
                "final Codex transformer strips output IDs after conversion"
            );
        } else {
            // Raw checked client history preserves whole Native items in notes;
            // typed ComputerMetadata above preserves only actions/screenshot.
            assert!(input.iter().any(|item| item["content"]
                == format!("[Previous computer call; call_id=call_mixed_computer]: {}", payload["items"][3])));
            assert!(input.iter().any(|item| item["content"]
                == format!("[Previous computer result; call_id=call_mixed_computer]: {}", payload["items"][4])));
        }
        for index in 1..5 {
            let input = recorded[index]["body"]["input"].as_array().unwrap();
            assert!(input.to_vec().iter().any(|item| item.to_string().contains("visible compacted fallback")));
            assert!(!input.iter().any(|item| item["type"] == "reasoning" || item["type"] == "computer_call"));
        }
    }
}

fn hosted_search(status: Option<&str>, id: &str) -> Value {
    let mut item = json!({"type":"web_search_call","id":id,
        "action":{"type":"search","query":"controlled fixture","sources":["https://fixture.invalid/"]},
        "provider_extra":{"kept":[1,"opaque"]}});
    if let Some(status) = status {
        item["status"] = json!(status);
    }
    item
}
fn pause_message(id: &str, value: &str) -> Value {
    json!({"type":"message","id":id,"role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":value}]})
}
fn pause_script(
    items: &[Value],
    search_added: bool,
    search_done: bool,
    terminal: &str,
    end_turn: Option<bool>,
) -> Value {
    let mut events = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let hosted = item["type"] == "web_search_call";
        if hosted && search_added {
            events.push(json!({"data":{"type":"response.output_item.added","output_index":index,"item":item}}));
        }
        if !hosted || search_done {
            events.push(json!({"data":{"type":"response.output_item.done","output_index":index,"item":item}}));
        }
    }
    let mut response = json!({"id":"resp_pause","status":terminal});
    if let Some(end_turn) = end_turn {
        response["end_turn"] = json!(end_turn);
    }
    if terminal == "incomplete" {
        response["incomplete_details"] = json!({"reason":"max_output_tokens"});
    }
    if terminal == "failed" {
        response["error"] = json!({"message":"controlled pause failure"});
    }
    events.push(json!({"data":{"type":if terminal=="incomplete" {"response.incomplete"} else {"response.completed"},
        "response":response}}));
    json!({"events":events})
}
fn assert_no_client_search(events: &[AssistantMessageEvent], output: &AssistantMessage) {
    assert!(output.tool_calls().next().is_none(), "hosted search is not a client ToolCall");
    assert!(!events.iter().any(|event| matches!(
        event,
        AssistantMessageEvent::ToolcallStart { .. }
            | AssistantMessageEvent::ToolcallDelta { .. }
            | AssistantMessageEvent::ToolcallEnd { .. }
    )));
}
fn is_pause(output: &AssistantMessage) -> bool {
    output.stop_details.as_ref().is_some_and(|details| details["type"] == "pause_turn")
}

/// Actual decoder and outgoing HTTP observations, grouped under the existing
/// Responses/Codex protocol targets. No Host executor or full websearch claim.
async fn verify_pause(start: &Start, codex: bool) {
    for (end_turn, visible, expected) in [
        (Some(false), false, true),
        (Some(false), true, true),
        (Some(true), false, false),
        (None, false, false),
        (None, true, false),
    ] {
        let items = if visible { vec![pause_message("visible", "answer")] } else { vec![] };
        let endpoint = server(vec![pause_script(&items, false, false, "completed", end_turn)]).await;
        let (events, out) =
            drain(start(endpoint.base_url(), context(), RequestOptions::default(), CancellationToken::new())).await;
        assert_eq!(out.stop_reason, StopReason::Stop);
        assert_eq!(is_pause(&out), expected);
        assert_no_client_search(&events, &out);
        assert_eq!(endpoint.served(), 1);
    }
    for (items, terminal, reason) in [
        (
            vec![json!({"type":"function_call","id":"fc_pause","call_id":"pause_read","name":"read","arguments":"{}"})],
            "completed",
            StopReason::ToolUse,
        ),
        (vec![], "incomplete", StopReason::Length),
        (vec![], "failed", StopReason::Error),
        (vec![hosted_search(Some("completed"), "ws_length")], "incomplete", StopReason::Length),
        (vec![hosted_search(Some("completed"), "ws_failure")], "failed", StopReason::Error),
    ] {
        let endpoint = server(vec![pause_script(&items, false, false, terminal, Some(false))]).await;
        let (_, out) =
            drain(start(endpoint.base_url(), context(), RequestOptions::default(), CancellationToken::new())).await;
        assert_eq!(out.stop_reason, reason);
        assert!(!is_pause(&out));
    }
    // Completion progress comes only from actual item.done, never added-only
    // or terminal-output fallback. JavaScript whitespace includes FEFF, not 85.
    for (status, added, done, hidden, expected) in [
        (None, false, true, "empty", true),
        (Some("completed"), true, true, "empty", true),
        (Some("completed"), true, true, "whitespace", true),
        (Some("completed"), false, true, "reasoning", true),
        (Some("completed"), true, true, "visible", false),
        (Some("completed"), false, true, "not_js_whitespace", false),
        (Some("in_progress"), true, true, "empty", false),
        (Some("failed"), true, true, "empty", false),
        (Some("completed"), true, false, "empty", false),
    ] {
        let search = hosted_search(status, "ws_progress");
        let mut items = vec![search.clone()];
        match hidden {
            "whitespace" => items.push(pause_message("whitespace", " \t\n\u{feff}")),
            "not_js_whitespace" => items.push(pause_message("not_js_whitespace", "\u{85}")),
            "visible" => items.push(pause_message("visible", "answer")),
            "reasoning" => {
                items.push(json!({"type":"reasoning","id":"rs_pause","summary":[],"encrypted_content":"opaque"}))
            }
            _ => {}
        }
        let endpoint = server(vec![pause_script(&items, added, done, "completed", None)]).await;
        let (events, out) =
            drain(start(endpoint.base_url(), context(), RequestOptions::default(), CancellationToken::new())).await;
        assert_eq!(out.stop_reason, StopReason::Stop);
        assert_eq!(is_pause(&out), expected, "{hidden} / {status:?}");
        assert_no_client_search(&events, &out);
        if done {
            assert_eq!(out.provider_payload.as_ref().unwrap()["items"][0], search);
        } else {
            assert!(out.provider_payload.as_ref().unwrap()["items"].as_array().unwrap().is_empty());
        }
        assert_eq!(endpoint.served(), 1);
    }
    let search = hosted_search(Some("completed"), "ws_terminal_only");
    let endpoint = server(vec![json!({"events":[{"data":{"type":"response.completed","response":{
        "id":"terminal_only","status":"completed","output":[search]}}}]})])
    .await;
    let (events, out) =
        drain(start(endpoint.base_url(), context(), RequestOptions::default(), CancellationToken::new())).await;
    assert_eq!(out.stop_reason, StopReason::Stop);
    assert!(!is_pause(&out));
    assert_no_client_search(&events, &out);
    assert_eq!(out.provider_payload.as_ref().unwrap()["items"][0], search);

    // Incremental invisible slots and same-origin full snapshots preserve source
    // order, opaque fields and real next-request input without client receipts.
    for mixed in [false, true] {
        let first_search = hosted_search(Some("completed"), "ws_first");
        let second_search = hosted_search(None, "ws_last");
        let items = if mixed {
            vec![
                pause_message("before", "before"),
                first_search.clone(),
                json!({"type":"reasoning","id":"rs_order","summary":[],"encrypted_content":"opaque"}),
                pause_message("after", "after"),
                second_search.clone(),
            ]
        } else {
            vec![first_search.clone()]
        };
        let endpoint =
            server(vec![pause_script(&items, true, true, "completed", None), text(), text(), text(), text(), text()])
                .await;
        let (_, first) =
            drain(start(endpoint.base_url(), context(), RequestOptions::default(), CancellationToken::new())).await;
        assert_eq!(is_pause(&first), !mixed);
        assert_eq!(first.provider_payload.as_ref().unwrap()["items"], json!(items));
        for mode in ["incremental", "full", "opt_out", "foreign_endpoint", "visible_edited"] {
            let mut carrier = first.clone();
            if mode == "full" {
                carrier.provider_payload.as_mut().unwrap()["dt"] = json!(false);
            }
            if mode == "foreign_endpoint" {
                carrier.provider_payload.as_mut().unwrap()["endpointSha256"] = json!("wrong-endpoint");
            }
            if mode == "visible_edited" && mixed {
                if let ara_ai::AssistantBlock::Text(text) = &mut carrier.content[0] {
                    text.text = "edited".into();
                } else {
                    unreachable!();
                }
            }
            let mut ctx = context();
            ctx.messages.push(Message::Assistant(carrier));
            let served_before = endpoint.served();
            let (_, out) = drain(start(
                endpoint.base_url(),
                ctx,
                RequestOptions {
                    native_history_replay: if mode == "opt_out" { Some(false) } else { None },
                    ..Default::default()
                },
                CancellationToken::new(),
            ))
            .await;
            if mode == "opt_out" && codex {
                assert_eq!(out.stop_reason, StopReason::Error);
                assert!(!is_pause(&out));
                assert_eq!(endpoint.served(), served_before, "Codex keeps its explicit native-replay requirement");
                continue;
            }
            assert_eq!(out.text(), "ok");
            let recorded = endpoint.requests.lock().await;
            let input = recorded.last().unwrap()["body"]["input"].as_array().unwrap();
            let expected = matches!(mode, "incremental" | "full") || mode == "visible_edited" && !mixed;
            let searches = input.iter().filter(|item| item["type"] == "web_search_call").collect::<Vec<_>>();
            assert_eq!(
                searches.len(),
                if expected { if mixed { 2 } else { 1 } } else { 0 },
                "{mode} / mixed={mixed} / codex={codex}"
            );
            if expected {
                let mut sanitized = first_search.clone();
                sanitized.as_object_mut().unwrap().remove("id");
                assert_eq!(*searches[0], sanitized);
                if mixed {
                    let families = input.iter().filter_map(|item| item["type"].as_str()).collect::<Vec<_>>();
                    assert!(families.windows(5).any(|family| family
                        == ["message", "web_search_call", "reasoning", "message", "web_search_call"].as_slice()));
                }
            }
            assert!(!input.iter().any(|item| matches!(
                item["type"].as_str(),
                Some("function_call" | "function_call_output" | "custom_tool_call" | "custom_tool_call_output")
            )));
        }
    }

    // A hosted wire effect cannot be replayed after failure/disconnect/cancel.
    // A nonempty thinking delta commits the retry buffer before cancellation; the
    // hosted search itself deliberately emits no client progress event.
    let added = json!({"data":{"type":"response.output_item.added","output_index":0,
        "item":hosted_search(Some("in_progress"),"ws_effect")}});
    for fault in ["disconnect", "failed", "cancelled"] {
        let mut scripted = vec![added.clone()];
        if fault == "failed" {
            scripted.push(json!({"data":{"type":"response.failed","response":{
            "status":"failed","error":{"message":"controlled hosted failure"}}}}));
        }
        if fault == "cancelled" {
            scripted.push(json!({"data":{"type":"response.output_item.added","output_index":1,
            "item":{"type":"reasoning","id":"rs_cancel","summary":[]}}}));
            scripted.push(json!({"data":{"type":"response.reasoning_summary_text.delta",
                "output_index":1,"item_id":"rs_cancel","summary_index":0,"delta":"controlled cancellation barrier"}}));
        }
        let endpoint =
            server(vec![json!({"events":scripted,"end":if fault=="cancelled" {"hang"} else {"close"}}), text()]).await;
        let cancel = CancellationToken::new();
        let mut stream = start(endpoint.base_url(), context(), RequestOptions::default(), cancel.clone());
        let mut events = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), async {
            while let Some(event) = stream.recv().await {
                if fault == "cancelled" && matches!(event, AssistantMessageEvent::ThinkingDelta { .. }) {
                    cancel.cancel();
                }
                events.push(event);
            }
        })
        .await
        .expect("hosted effect fault settles");
        assert_eq!(cancel.is_cancelled(), fault == "cancelled");
        assert_eq!(events.iter().filter(|event| event.is_terminal()).count(), 1);
        let out = events.last().unwrap().partial();
        assert_eq!(out.stop_reason, if fault == "cancelled" { StopReason::Aborted } else { StopReason::Error });
        assert!(!is_pause(out));
        assert_no_client_search(&events, out);
        assert!(out.provider_payload.is_none());
        assert_eq!(endpoint.served(), 1, "hosted search effect does not replay");
    }
}
