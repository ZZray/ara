//! Fixed Native Core hooks, grouped as R (admission), T (lifecycle), I
//! (interruption), G (final prepared-provider gate), and D-H (bounded hard
//! directives). Controlled Core behavior; actual Host/journal/model families
//! are separate receipts and are not claimed by this target.

use ara_agent::*;
use ara_ai::event::EventSink;
use ara_ai::*;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[path = "core_hooks/actual_stream.rs"]
mod actual_stream;

mod argument_stream_groups {
    use super::*;

    #[derive(Default)]
    struct PreviewTrace {
        callbacks: Mutex<Vec<Value>>,
        effects: Mutex<Vec<Value>>,
        retained: Mutex<Vec<ToolArgStreamEmit>>,
    }

    struct PreviewTool {
        name: &'static str,
        alias: Option<&'static str>,
        trace: Arc<PreviewTrace>,
        fault: Option<(&'static str, bool)>,
        enabled: bool,
    }

    struct PreviewReceiver {
        id: String,
        logical: &'static str,
        emit: ToolArgStreamEmit,
        trace: Arc<PreviewTrace>,
        fault: Option<(&'static str, bool)>,
    }

    impl PreviewReceiver {
        fn observe(&self, phase: &str, payload: Value) -> Result<(), ToolError> {
            let value = json!({"phase":phase,"id":self.id,"logical":self.logical,"payload":payload});
            self.trace.callbacks.lock().unwrap().push(value.clone());
            (self.emit)(value);
            if let Some((fault, panic)) = self.fault
                && fault == phase
            {
                assert!(!panic, "preview consumer panic");
                return Err(ToolError("preview consumer error".into()));
            }
            Ok(())
        }
    }

    impl ToolArgStream for PreviewReceiver {
        fn push(&mut self, delta: &str) -> Result<(), ToolError> {
            self.observe("push", json!(delta))
        }
        fn end(&mut self, args: &JsonObject) -> Result<(), ToolError> {
            self.observe("end", json!(args))
        }
        fn cancel(&mut self) -> Result<(), ToolError> {
            self.observe("cancel", Value::Null)
        }
    }

    #[async_trait]
    impl AgentTool for PreviewTool {
        fn definition(&self) -> Tool {
            Tool {
                name: self.name.into(),
                description: "argument preview fixture".into(),
                parameters: json!({"type":"object","properties":{"raw":{"type":"string"},
                    "input":{"type":"string"},"prepared":{"type":"boolean"},"approved":{"type":"boolean"}},
                    "required":[if self.name == "echo" || self.name == "remote_impl" {"raw"} else {"input"}]}),
                ..Default::default()
            }
        }
        fn custom_wire_name(&self) -> Option<String> {
            self.alias.map(str::to_owned)
        }
        fn custom_format(&self) -> Option<CustomToolFormat> {
            self.alias.map(|_| CustomToolFormat { syntax: CustomToolSyntax::Lark, definition: "start: /.+/".into() })
        }
        fn intent_mode(&self) -> ToolIntentMode {
            ToolIntentMode::Omit
        }
        fn open_arg_stream(&self, init: ToolArgStreamInit) -> Result<Option<Box<dyn ToolArgStream>>, ToolError> {
            if !self.enabled {
                return Ok(None);
            }
            self.trace.retained.lock().unwrap().push(init.emit.clone());
            let receiver = PreviewReceiver {
                id: init.tool_call_id,
                logical: self.name,
                emit: init.emit,
                trace: self.trace.clone(),
                fault: self.fault,
            };
            receiver.observe("open", json!({"toolName":init.tool_name,"customWireName":init.custom_wire_name}))?;
            Ok(Some(Box::new(receiver)))
        }
        async fn execute(
            &self,
            id: &str,
            args: JsonObject,
            _: CancellationToken,
            _: UpdateFn,
        ) -> Result<ToolOutput, ToolError> {
            self.trace.effects.lock().unwrap().push(json!({"id":id,"logical":self.name,"args":args}));
            Ok(ToolOutput::text("executed after provider end"))
        }
    }

    struct PreviewHooks {
        fallback: Arc<dyn AgentTool>,
        resolver_panic: bool,
        block_custom: bool,
    }
    #[async_trait]
    impl LoopHooks for PreviewHooks {
        fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
            assert!(!self.resolver_panic, "fallback resolver panic must fail execution");
            (name == "remote").then(|| self.fallback.clone())
        }
        fn transform_tool_call_arguments(&self, mut args: JsonObject, _: &str) -> Result<JsonObject, ToolHookError> {
            args.insert("prepared".into(), json!(true));
            Ok(args)
        }
        async fn before_tool_dispatch(
            &self,
            context: &mut BeforeToolCallContext<'_>,
            _: &CancellationToken,
        ) -> Result<BeforeToolCallResult, ToolHookError> {
            let mut args = context.args.clone();
            args.insert("approved".into(), json!(true));
            Ok(BeforeToolCallResult { args: Some(args), ..Default::default() })
        }
        async fn before_tool_call(&self, call: &ToolCall, _: &JsonObject, _: &CancellationToken) -> ToolDecision {
            if self.block_custom && call.id == "custom" {
                ToolDecision::Block(Some("host fixture denial".into()))
            } else {
                ToolDecision::Allow
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Exit {
        Done,
        Error,
        Missing,
        Abort,
    }
    struct PreviewProvider {
        frames: Mutex<Option<Vec<AssistantMessageEvent>>>,
        exit: Exit,
    }
    impl ModelProvider for PreviewProvider {
        fn stream(&self, model: &Model, _: &Context, options: CallOptions) -> AssistantStream {
            let frames = self.frames.lock().unwrap().take();
            let exit = self.exit;
            let mut final_message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let (sink, stream) = EventSink::channel();
            tokio::spawn(async move {
                if let Some(frames) = frames {
                    for event in frames {
                        final_message = event.partial().clone();
                        sink.push(event).await;
                    }
                    match exit {
                        Exit::Missing => return,
                        Exit::Abort => {
                            options.cancel.cancelled().await;
                            return;
                        }
                        Exit::Error => {
                            final_message.stop_reason = StopReason::Error;
                            final_message.error_message = Some("upstream fixture failure".into());
                            sink.push(AssistantMessageEvent::Error { reason: StopReason::Error, error: final_message })
                                .await;
                            return;
                        }
                        Exit::Done => {
                            final_message.stop_reason = StopReason::ToolUse;
                        }
                    }
                }
                sink.push(AssistantMessageEvent::Done { reason: final_message.stop_reason, message: final_message })
                    .await;
            });
            stream
        }
    }

    fn frames(complete: bool, invalid: bool) -> Vec<AssistantMessageEvent> {
        let mut message = AssistantMessage::empty("openai-responses", "fixture", "preview");
        let calls = [
            ToolCall { id: "json".into(), name: "echo".into(), ..Default::default() },
            ToolCall {
                id: "custom".into(),
                name: "apply_patch".into(),
                custom_wire_name: Some("apply_patch".into()),
                ..Default::default()
            },
            ToolCall { id: "fallback".into(), name: "remote".into(), ..Default::default() },
        ];
        let mut events = vec![AssistantMessageEvent::Start { partial: message.clone() }];
        for (index, call) in calls.into_iter().enumerate() {
            message.content.push(AssistantBlock::ToolCall(call));
            events.push(AssistantMessageEvent::ToolcallStart { content_index: index, partial: message.clone() });
        }
        for (index, delta) in [(2, "{\"raw\":\"remote\"}"), (0, "{\"raw\":\"wire\"}"), (1, "*** raw patch\n")] {
            events.push(AssistantMessageEvent::ToolcallDelta {
                content_index: index,
                delta: delta.into(),
                partial: message.clone(),
            });
        }
        if complete {
            for index in [1, 0, 2] {
                let mut call = message.content[index].as_tool_call().unwrap().clone();
                call.arguments = if index == 1 {
                    json!({"input":"*** provider-final patch\n"})
                } else if invalid && index == 0 {
                    json!({"raw":19})
                } else {
                    json!({"raw":"provider-final"})
                }
                .as_object()
                .unwrap()
                .clone();
                message.content[index] = AssistantBlock::ToolCall(call.clone());
                events.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: index,
                    tool_call: call,
                    partial: message.clone(),
                });
            }
        }
        events
    }

    struct PreviewSink {
        receipts: Arc<Receipts>,
        cancel_on_delta: Option<CancellationToken>,
        drop_hold: Option<Arc<Gate>>,
    }
    #[async_trait]
    impl AgentEventSink for PreviewSink {
        async fn emit(&self, event: AgentEvent) {
            let delta =
                matches!(&event, AgentEvent::MessageUpdate { event: AssistantMessageEvent::ToolcallDelta { .. }, .. });
            self.receipts.emit(event).await;
            if delta {
                if let Some(cancel) = &self.cancel_on_delta {
                    cancel.cancel();
                }
                if let Some(hold) = &self.drop_hold {
                    hold.started.notify_one();
                    hold.release.notified().await;
                }
            }
        }
    }

    fn setup(
        trace: &Arc<PreviewTrace>,
        fault: Option<(&'static str, bool)>,
        collision: bool,
        enabled: bool,
    ) -> (AgentConfig, Arc<PreviewHooks>) {
        let make = |name, alias| -> Arc<dyn AgentTool> {
            Arc::new(PreviewTool { name, alias, trace: trace.clone(), fault, enabled })
        };
        let hooks =
            Arc::new(PreviewHooks { fallback: make("remote_impl", None), resolver_panic: false, block_custom: false });
        let mut cfg = config(Provider::new(Vec::new()), make("echo", None), hooks.clone());
        cfg.tools.push(make("edit", Some("apply_patch")));
        if collision {
            cfg.tools.push(make("apply_patch", None));
        }
        (cfg, hooks)
    }

    fn observe_order(events: &[AgentEvent]) {
        for (position, event) in events.iter().enumerate() {
            if let AgentEvent::MessageUpdate { event, .. } = event {
                let (id, phase) = match event {
                    AssistantMessageEvent::ToolcallStart { content_index, partial } => {
                        (partial.content[*content_index].as_tool_call().unwrap().id.as_str(), "open")
                    }
                    AssistantMessageEvent::ToolcallDelta { content_index, partial, .. } => {
                        (partial.content[*content_index].as_tool_call().unwrap().id.as_str(), "push")
                    }
                    AssistantMessageEvent::ToolcallEnd { tool_call, .. } => (tool_call.id.as_str(), "end"),
                    _ => continue,
                };
                assert!(events[..position].iter().any(|preview| matches!(preview,
                    AgentEvent::ToolStreamUpdate { tool_call_id, update, .. } if tool_call_id == id && update["phase"] == phase)));
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn group_s_argument_preview_lifecycle_errors_aliases_and_existing_state_sink() {
        for (collision, blocked, invalid) in
            [(false, false, false), (true, false, false), (false, true, false), (false, false, true)]
        {
            let trace = Arc::new(PreviewTrace::default());
            let (mut cfg, _) = setup(&trace, None, collision, true);
            cfg.hooks = Arc::new(PreviewHooks {
                fallback: Arc::new(PreviewTool {
                    name: "remote_impl",
                    alias: None,
                    trace: trace.clone(),
                    fault: None,
                    enabled: true,
                }),
                resolver_panic: false,
                block_custom: blocked,
            });
            cfg.provider =
                Arc::new(PreviewProvider { frames: Mutex::new(Some(frames(true, invalid))), exit: Exit::Done });
            let agent = Agent::new(cfg, Vec::new());
            let receipts = Arc::new(Receipts::default());
            let report =
                bounded(agent.prompt(vec![user("preview")], CancellationToken::new(), receipts.clone())).await.unwrap();
            assert_eq!(report.end, RunEnd::Completed);
            {
                let callbacks = trace.callbacks.lock().unwrap();
                assert_eq!(callbacks.iter().filter(|row| row["phase"] == "open").count(), 3);
                assert_eq!(callbacks.iter().filter(|row| row["phase"] == "end").count(), 3);
                assert!(!callbacks.iter().any(|row| row["phase"] == "cancel"));
                assert_eq!(
                    callbacks
                        .iter()
                        .filter(|row| row["phase"] == "push")
                        .map(|row| row["id"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    ["fallback", "json", "custom"]
                );
                assert_eq!(
                    callbacks
                        .iter()
                        .filter(|row| row["phase"] == "end")
                        .map(|row| row["id"].as_str().unwrap())
                        .collect::<Vec<_>>(),
                    ["custom", "json", "fallback"]
                );
                let custom = callbacks.iter().find(|row| row["phase"] == "open" && row["id"] == "custom").unwrap();
                assert_eq!(custom["logical"], if collision { "apply_patch" } else { "edit" });
                assert_eq!(custom["payload"], json!({"toolName":"apply_patch","customWireName":"apply_patch"}));
                for row in callbacks.iter().filter(|row| row["phase"] == "end") {
                    assert!(row["payload"].get("prepared").is_none());
                    assert!(row["payload"].get("approved").is_none());
                }
            }
            {
                let effects = trace.effects.lock().unwrap();
                assert_eq!(effects.len(), 3 - usize::from(blocked) - usize::from(invalid));
                assert!(
                    effects
                        .iter()
                        .all(|effect| effect["args"]["prepared"] == true && effect["args"]["approved"] == true)
                );
            }
            let before = receipts.events.lock().unwrap().len();
            for emit in trace.retained.lock().unwrap().iter() {
                emit(json!({"late":true}));
            }
            assert_eq!(receipts.events.lock().unwrap().len(), before);
            {
                let events = receipts.events.lock().unwrap();
                observe_order(&events);
                assert_eq!(
                    events.iter().filter(|event| matches!(event, AgentEvent::ToolStreamUpdate { .. })).count(),
                    9
                );
                let frame = events.iter().find(|event| matches!(event, AgentEvent::ToolStreamUpdate { .. })).unwrap();
                assert_eq!(frame.full().unwrap(), frame.printable().unwrap());
                assert_eq!(frame.type_name(), "tool_stream_update");
            }
            let raw = agent.raw_messages().await;
            assert_eq!(raw.len(), report.messages.len());
            assert!(raw.iter().all(|message| message.json().unwrap()["role"] != "tool_stream_update"));
            assert!(
                raw.iter()
                    .filter_map(|message| message.checked_model_snapshot().unwrap())
                    .filter_map(|message| message.as_assistant().cloned())
                    .all(|message| message.usage.is_unknown())
            );
        }

        // Error and panic at each consumer stage are nonfatal. An end failure
        // still removes the receiver; a push failure still reaches end/cancel.
        for phase in ["open", "push", "end", "cancel"] {
            for panic in [false, true] {
                let trace = Arc::new(PreviewTrace::default());
                let (mut cfg, _) = setup(&trace, Some((phase, panic)), false, true);
                let complete = phase != "cancel";
                cfg.provider =
                    Arc::new(PreviewProvider { frames: Mutex::new(Some(frames(complete, false))), exit: Exit::Done });
                let receipts = Arc::new(Receipts::default());
                let report = bounded(Agent::new(cfg, Vec::new()).prompt(
                    vec![user("consumer fault")],
                    CancellationToken::new(),
                    receipts,
                ))
                .await
                .unwrap();
                assert_eq!(report.end, RunEnd::Completed);
                let callbacks = trace.callbacks.lock().unwrap();
                assert_eq!(callbacks.iter().filter(|row| row["phase"] == "open").count(), 3);
                assert_eq!(
                    callbacks.iter().filter(|row| row["phase"] == "end").count(),
                    if complete && phase != "open" { 3 } else { 0 }
                );
                assert_eq!(
                    callbacks.iter().filter(|row| row["phase"] == "cancel").count(),
                    if complete { 0 } else { 3 }
                );
            }
        }

        // Preview-only failures/abort never execute, persist a preview record,
        // or create a ToolResult. Missing terminals retain their actual failure.
        for exit in [Exit::Error, Exit::Missing, Exit::Abort] {
            let trace = Arc::new(PreviewTrace::default());
            let (mut cfg, _) = setup(&trace, None, false, true);
            cfg.provider = Arc::new(PreviewProvider { frames: Mutex::new(Some(frames(false, false))), exit });
            let receipts = Arc::new(Receipts::default());
            let token = CancellationToken::new();
            let sink = Arc::new(PreviewSink {
                receipts: receipts.clone(),
                cancel_on_delta: matches!(exit, Exit::Abort).then(|| token.clone()),
                drop_hold: None,
            });
            let report = bounded(Agent::new(cfg, Vec::new()).prompt(vec![user("preview-only failure")], token, sink))
                .await
                .unwrap();
            assert_eq!(report.end, if matches!(exit, Exit::Abort) { RunEnd::Aborted } else { RunEnd::Error });
            assert!(trace.effects.lock().unwrap().is_empty());
            assert_eq!(trace.callbacks.lock().unwrap().iter().filter(|row| row["phase"] == "cancel").count(), 3);
            assert_eq!(
                trace
                    .callbacks
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|row| row["phase"] == "cancel")
                    .map(|row| row["id"].as_str().unwrap().to_owned())
                    .collect::<Vec<_>>(),
                ["json", "custom", "fallback"]
            );
            assert!(!report.model_messages().unwrap().iter().any(|message| matches!(message, Message::ToolResult(_))));
            assert!(!receipts.events.lock().unwrap().iter().any(|event| matches!(
                event,
                AgentEvent::ToolExecutionStart { .. } | AgentEvent::ToolExecutionEnd { .. }
            )));
        }

        for (enabled, done_only) in [(false, false), (true, true)] {
            let trace = Arc::new(PreviewTrace::default());
            let (mut cfg, _) = setup(&trace, None, false, enabled);
            let events = if done_only {
                let events = frames(true, false);
                vec![events[0].clone(), events.last().unwrap().clone()]
            } else {
                frames(true, false)
            };
            // ToolcallEnd without a prior start never opens a preview.
            cfg.provider = Arc::new(PreviewProvider { frames: Mutex::new(Some(events)), exit: Exit::Done });
            let receipts = Arc::new(Receipts::default());
            let report = bounded(Agent::new(cfg, Vec::new()).prompt(
                vec![user("inert")],
                CancellationToken::new(),
                receipts.clone(),
            ))
            .await
            .unwrap();
            assert_eq!(report.end, RunEnd::Completed);
            assert!(trace.callbacks.lock().unwrap().is_empty());
            assert!(
                !receipts
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, AgentEvent::ToolStreamUpdate { .. }))
            );
        }

        let trace = Arc::new(PreviewTrace::default());
        let (mut cfg, _) = setup(&trace, None, false, true);
        cfg.provider = Arc::new(PreviewProvider { frames: Mutex::new(Some(frames(false, false))), exit: Exit::Done });
        cfg.hooks = Arc::new(PreviewHooks {
            fallback: Arc::new(PreviewTool {
                name: "remote_impl",
                alias: None,
                trace: trace.clone(),
                fault: None,
                enabled: true,
            }),
            resolver_panic: true,
            block_custom: false,
        });
        let report = bounded(Agent::new(cfg, Vec::new()).prompt(
            vec![user("resolver failure")],
            CancellationToken::new(),
            Arc::new(Receipts::default()),
        ))
        .await
        .unwrap();
        assert_eq!(report.end, RunEnd::Error);
        assert!(
            report
                .model_messages()
                .unwrap()
                .last()
                .unwrap()
                .as_assistant()
                .unwrap()
                .error_message
                .as_ref()
                .unwrap()
                .contains("Fallback tool resolver panicked")
        );
        assert_eq!(trace.callbacks.lock().unwrap().iter().filter(|row| row["phase"] == "cancel").count(), 2);
        assert!(trace.effects.lock().unwrap().is_empty());

        // Abort the actual provider-attempt future while its existing sink is
        // awaited. RAII cancels once; no async cleanup frame is fabricated.
        let trace = Arc::new(PreviewTrace::default());
        let (mut cfg, _) = setup(&trace, None, false, true);
        cfg.provider =
            Arc::new(PreviewProvider { frames: Mutex::new(Some(frames(false, false))), exit: Exit::Missing });
        let hold = Arc::new(Gate::default());
        let receipts = Arc::new(Receipts::default());
        let sink = PreviewSink { receipts: receipts.clone(), cancel_on_delta: None, drop_hold: Some(hold.clone()) };
        let run = tokio::spawn(async move {
            agent_loop(vec![user("drop future")], &mut Vec::new(), &cfg, &CancellationToken::new(), &sink).await
        });
        bounded(hold.started.notified()).await;
        run.abort();
        assert!(bounded(run).await.unwrap_err().is_cancelled());
        assert_eq!(trace.callbacks.lock().unwrap().iter().filter(|row| row["phase"] == "cancel").count(), 3);
        assert!(trace.effects.lock().unwrap().is_empty());
        let before = receipts.events.lock().unwrap().len();
        for emit in trace.retained.lock().unwrap().iter() {
            emit(json!({"late":true}));
        }
        assert_eq!(receipts.events.lock().unwrap().len(), before);
    }

    #[tokio::test]
    async fn group_s_actual_responses_http_json_and_custom_fragments_reach_core_preview() {
        use ara_testkit::{FakeUpstream, Script};
        let script: Script = serde_json::from_value(json!({"responses":[{"events":[
            {"data":{"type":"response.output_item.added","output_index":0,"item":{"type":"function_call","id":"fc_json","call_id":"json","name":"echo","arguments":""}}},
            {"data":{"type":"response.output_item.added","output_index":1,"item":{"type":"custom_tool_call","id":"fc_custom","call_id":"custom","name":"apply_patch","input":""}}},
            {"data":{"type":"response.custom_tool_call_input.delta","output_index":1,"delta":"*** raw fragment\n"}},
            {"data":{"type":"response.function_call_arguments.delta","output_index":0,"delta":"{\"raw\":\"fragment\"}"}},
            {"data":{"type":"response.output_item.done","output_index":1,"item":{"type":"custom_tool_call","id":"fc_custom","call_id":"custom","name":"apply_patch","input":"*** provider final\n"}}},
            {"data":{"type":"response.output_item.done","output_index":0,"item":{"type":"function_call","id":"fc_json","call_id":"json","name":"echo","arguments":"{\"raw\":\"provider final\"}"}}},
            {"data":{"type":"response.completed","response":{"status":"completed"}}}
        ]}]})).unwrap();
        let endpoint = FakeUpstream::start(script, None).await.unwrap();
        let trace = Arc::new(PreviewTrace::default());
        let (mut cfg, _) = setup(&trace, None, false, true);
        let mut options = ara_ai::providers::openai_responses::StreamOptions::default();
        options.request.apply_patch_tool_type = Some("freeform".into());
        options.retry.max_attempts = 1;
        cfg.model.api = "openai-responses".into();
        cfg.model.provider = "openai".into();
        cfg.model.base_url = endpoint.base_url();
        cfg.provider = Arc::new(OpenAIResponsesProvider { client: reqwest::Client::new(), base: options });
        cfg.max_model_calls = Some(1);
        let receipts = Arc::new(Receipts::default());
        let report = bounded(Agent::new(cfg, Vec::new()).prompt(
            vec![user("HTTP preview")],
            CancellationToken::new(),
            receipts.clone(),
        ))
        .await
        .unwrap();
        assert_eq!(report.end, RunEnd::ModelCallBudget);
        assert_eq!(endpoint.served(), 1);
        assert_eq!(trace.effects.lock().unwrap().len(), 2);
        assert!(
            report
                .model_messages()
                .unwrap()
                .iter()
                .filter_map(|message| message.as_assistant())
                .all(|message| message.usage.is_unknown())
        );
        {
            let callbacks = trace.callbacks.lock().unwrap();
            assert!(callbacks.iter().any(|row| row["id"] == "custom|fc_custom"
                && row["phase"] == "push"
                && row["payload"] == "*** raw fragment\n"));
            assert!(callbacks.iter().any(|row| row["id"] == "json|fc_json"
                && row["phase"] == "push"
                && row["payload"] == "{\"raw\":\"fragment\"}"));
            assert!(callbacks.iter().any(|row| row["id"] == "custom|fc_custom"
                && row["phase"] == "end"
                && row["payload"] == json!({"input":"*** raw fragment\n"})));
            assert!(!callbacks.iter().any(|row| row["phase"] == "cancel"));
        }
        observe_order(&receipts.events.lock().unwrap());
        let requests = endpoint.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert!(!requests[0]["body"].to_string().contains("tool_stream_update"));
    }
}

type FinalAction = dyn Fn(&mut Context, &CancellationToken) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError>
    + Send
    + Sync;

struct FinalCallback {
    name: &'static str,
    visits: Arc<Mutex<Vec<&'static str>>>,
    action: Arc<FinalAction>,
    hold: Option<Arc<Gate>>,
    hold_once: AtomicBool,
}
impl FinalCallback {
    fn new(
        name: &'static str,
        visits: &Arc<Mutex<Vec<&'static str>>>,
        action: impl Fn(&mut Context, &CancellationToken) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            name,
            visits: visits.clone(),
            action: Arc::new(action),
            hold: None,
            hold_once: AtomicBool::new(true),
        })
    }
    fn held(
        name: &'static str,
        visits: &Arc<Mutex<Vec<&'static str>>>,
        hold: &Arc<Gate>,
        action: impl Fn(&mut Context, &CancellationToken) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError>
        + Send
        + Sync
        + 'static,
    ) -> Arc<Self> {
        Arc::new(Self {
            name,
            visits: visits.clone(),
            action: Arc::new(action),
            hold: Some(hold.clone()),
            hold_once: AtomicBool::new(true),
        })
    }
}
#[async_trait]
impl FinalProviderGate for FinalCallback {
    async fn before_model_call(
        &self,
        context: &mut Context,
        cancel: &CancellationToken,
    ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
        self.visits.lock().unwrap().push(self.name);
        if let Some(hold) = &self.hold
            && self.hold_once.swap(false, Ordering::SeqCst)
        {
            hold.started.notify_one();
            tokio::select! { _ = hold.release.notified() => {}, _ = cancel.cancelled() => {} }
        }
        (self.action)(context, cancel)
    }
}

struct FinalHooks {
    binding: FinalProviderGateBinding,
    getter_reads: AtomicUsize,
    rejected: AtomicUsize,
    preparations: Arc<AtomicUsize>,
    pipeline: Arc<Mutex<Vec<&'static str>>>,
    provider_binding: Option<ProviderContextBinding>,
    panic_prepare: AtomicBool,
    panic_reject: AtomicBool,
    queued: Mutex<Vec<AgentInput>>,
    dequeue_hold: Option<Arc<Gate>>,
    dequeue_hold_once: AtomicBool,
}
impl FinalHooks {
    fn new() -> Self {
        Self {
            binding: FinalProviderGateBinding::default(),
            getter_reads: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
            preparations: Arc::new(AtomicUsize::new(0)),
            pipeline: Arc::new(Mutex::new(Vec::new())),
            provider_binding: None,
            panic_prepare: AtomicBool::new(false),
            panic_reject: AtomicBool::new(false),
            queued: Mutex::new(Vec::new()),
            dequeue_hold: None,
            dequeue_hold_once: AtomicBool::new(true),
        }
    }
}
#[async_trait]
impl LoopHooks for FinalHooks {
    fn final_provider_gate_binding(&self) -> Option<FinalProviderGateBinding> {
        self.getter_reads.fetch_add(1, Ordering::SeqCst);
        Some(self.binding.clone())
    }
    fn on_final_provider_gate_rejected(&self) -> Result<(), FinalProviderGateError> {
        self.rejected.fetch_add(1, Ordering::SeqCst);
        assert!(!self.panic_reject.load(Ordering::SeqCst), "rejection callback failed");
        Ok(())
    }
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        self.provider_binding.clone()
    }
    async fn before_queued_input_dequeue(&self, _cancel: &CancellationToken) -> Result<(), AdmissionError> {
        if let Some(hold) = &self.dequeue_hold
            && self.dequeue_hold_once.swap(false, Ordering::SeqCst)
        {
            hold.started.notify_one();
            hold.release.notified().await;
        }
        Ok(())
    }
    async fn transform_provider_context(&self, mut context: Context, _model: &Model) -> Context {
        self.preparations.fetch_add(1, Ordering::SeqCst);
        self.pipeline.lock().unwrap().push("transform");
        assert!(!self.panic_prepare.load(Ordering::SeqCst), "provider preparation failed");
        context.system_prompt.push("transformed".into());
        context
    }
    async fn steering_inputs(&self) -> Vec<AgentInput> {
        std::mem::take(&mut *self.queued.lock().unwrap())
    }
}

struct FinalOwnedPort {
    pipeline: Arc<Mutex<Vec<&'static str>>>,
    projects: AtomicUsize,
    streams: AtomicUsize,
}
impl OwnedProviderContextPort for FinalOwnedPort {
    fn project_context(
        &self,
        _model: &Model,
        mut context: Context,
        side: bool,
    ) -> Result<Context, ProviderContextError> {
        assert!(!side);
        self.projects.fetch_add(1, Ordering::SeqCst);
        self.pipeline.lock().unwrap().push("owned");
        assert_eq!(context.system_prompt.last().map(String::as_str), Some("transformed"));
        context.system_prompt.push("owned".into());
        Ok(context)
    }
    fn stream(
        &self,
        provider: &dyn ModelProvider,
        model: &Model,
        context: &Context,
        options: CallOptions,
    ) -> AssistantStream {
        self.streams.fetch_add(1, Ordering::SeqCst);
        provider.stream(model, context, options)
    }
}

struct FinalSink {
    receipts: Arc<Receipts>,
    preparations: Arc<AtomicUsize>,
    install: Option<(FinalProviderGateBinding, Arc<dyn FinalProviderGate>)>,
}
#[async_trait]
impl AgentEventSink for FinalSink {
    async fn emit(&self, event: AgentEvent) {
        if matches!(&event, AgentEvent::AgentStart)
            && let Some((binding, callback)) = &self.install
        {
            binding.set_primary(Some(callback.clone()));
        }
        if matches!(&event, AgentEvent::TurnStart) {
            assert!(self.preparations.load(Ordering::SeqCst) > 0, "prepare must precede TurnStart");
        }
        self.receipts.emit(event).await;
    }
    async fn emit_input(&self, input: AgentInput) {
        self.receipts.emit_input(input).await;
    }
}

fn final_turn_counts(receipts: &Receipts) -> (usize, usize) {
    let events = receipts.events.lock().unwrap();
    (
        events.iter().filter(|event| matches!(event, AgentEvent::TurnStart)).count(),
        events.iter().filter(|event| matches!(event, AgentEvent::TurnEnd { .. })).count(),
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn group_g_request_boundary_one_preparation_provenance_stop_errors_and_abort() {
    // Observe transform -> owned projection -> gate edits in the actual send.
    let visits = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::new(Mutex::new(None));
    let mut hooks = FinalHooks::new();
    let port = Arc::new(FinalOwnedPort {
        pipeline: hooks.pipeline.clone(),
        projects: AtomicUsize::new(0),
        streams: AtomicUsize::new(0),
    });
    hooks.provider_binding =
        Some(ProviderContextBinding { owned_dialect: true, owned_port: Some(port.clone()), ..Default::default() });
    let hooks = Arc::new(hooks);
    let observed_gate = observed.clone();
    let pipeline = hooks.pipeline.clone();
    hooks.binding.set_primary(Some(FinalCallback::new("edit", &visits, move |context, _| {
        pipeline.lock().unwrap().push("gate");
        assert!(context.tools.is_none());
        assert_eq!(context.system_prompt.last().map(String::as_str), Some("owned"));
        context.system_prompt.push("gated".into());
        context.messages.push(user("request-only gate edit"));
        *observed_gate.lock().unwrap() = Some(context.clone());
        Ok(None)
    })));
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![turn(Vec::new())]);
    let receipts = Arc::new(Receipts::default());
    let sink = FinalSink { receipts: receipts.clone(), preparations: hooks.preparations.clone(), install: None };
    let cfg = config(provider.clone(), tool(&effects), hooks.clone());
    let provenance = Arc::new(json!({"source":"gate-input","opaque":{"id":17}}));
    let input = AgentInput { model: user("owned input"), provenance: Some(provenance.clone()) };
    let mut context = Vec::new();
    let report =
        bounded(agent_loop_inputs(vec![input.clone()], &mut context, &cfg, &CancellationToken::new(), &sink)).await;
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(provider.contexts.lock().unwrap().as_slice(), &[observed.lock().unwrap().clone().unwrap()]);
    assert_eq!(hooks.pipeline.lock().unwrap().as_slice(), &["transform", "owned", "gate"]);
    assert_eq!(port.projects.load(Ordering::SeqCst), 1);
    assert_eq!(port.streams.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.preparations.load(Ordering::SeqCst), 1);
    assert_eq!(receipts.inputs.lock().unwrap().as_slice(), &[input]);
    assert!(Arc::ptr_eq(receipts.inputs.lock().unwrap()[0].provenance.as_ref().unwrap(), &provenance));
    assert!(
        !context
            .iter()
            .any(|message| message.checked_model_snapshot().unwrap().as_ref() == Some(&user("request-only gate edit"))),
        "gate edits are request-only"
    );
    assert_eq!(final_turn_counts(&receipts), (1, 1));

    // An empty owner still prepares before TurnStart, exactly once.
    let hooks = Arc::new(FinalHooks::new());
    let provider = Provider::new(vec![turn(Vec::new())]);
    let receipts = Arc::new(Receipts::default());
    let sink = FinalSink { receipts: receipts.clone(), preparations: hooks.preparations.clone(), install: None };
    let cfg = config(provider.clone(), tool(&effects), hooks.clone());
    let report =
        bounded(agent_loop(vec![user("no callback")], &mut Vec::new(), &cfg, &CancellationToken::new(), &sink)).await;
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(hooks.preparations.load(Ordering::SeqCst), 1);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);

    // Initial owner stop keeps the input/source, no invented turn or Assistant.
    let hooks = Arc::new(FinalHooks::new());
    hooks.binding.set_primary(Some(FinalCallback::new("stop", &visits, |_, _| {
        Ok(Some(FinalProviderGateStop { reason: Some("owner budget".into()) }))
    })));
    let provider = Provider::new(Vec::new());
    let cfg = config(provider.clone(), tool(&effects), hooks.clone());
    let receipts = Receipts::default();
    let token = CancellationToken::new();
    let input = AgentInput { model: user("accepted stopped input"), provenance: Some(provenance.clone()) };
    let report = bounded(agent_loop_inputs(vec![input.clone()], &mut Vec::new(), &cfg, &token, &receipts)).await;
    assert_eq!(report.end, RunEnd::PreModelStop);
    assert_eq!(report.stop_cause, Some(RunStopCause::PreModelStop { reason: Some("owner budget".into()) }));
    assert_eq!(report.model_messages().unwrap(), vec![input.model.clone()]);
    assert_eq!(receipts.inputs.lock().unwrap().as_slice(), &[input]);
    assert_eq!(final_turn_counts(&receipts), (0, 0));
    assert_eq!(hooks.rejected.load(Ordering::SeqCst), 1);
    assert!(provider.contexts.lock().unwrap().is_empty());
    assert!(!token.is_cancelled());

    // After a real tool result, refusal starts no additional turn/provider call.
    let hooks = Arc::new(FinalHooks::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let gate_calls = calls.clone();
    hooks.binding.set_primary(Some(FinalCallback::new("after-tool", &visits, move |context, _| {
        if gate_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            return Ok(None);
        }
        assert!(context.messages.iter().any(|message| matches!(message, Message::ToolResult(_))));
        Ok(Some(FinalProviderGateStop::default()))
    })));
    let provider = Provider::new(vec![turn(vec![call("g-tool", json!({"text":"gate effect"}))])]);
    let local_effects = Arc::new(Mutex::new(Vec::new()));
    let cfg = config(provider.clone(), tool(&local_effects), hooks.clone());
    let receipts = Receipts::default();
    let report =
        bounded(agent_loop(vec![user("tool then stop")], &mut Vec::new(), &cfg, &CancellationToken::new(), &receipts))
            .await;
    assert_eq!(report.end, RunEnd::PreModelStop);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);
    assert_eq!(local_effects.lock().unwrap().len(), 1);
    assert_eq!(hooks.rejected.load(Ordering::SeqCst), 1);
    assert_eq!(final_turn_counts(&receipts), (1, 1));
    assert_eq!(result(&report, "g-tool").tool_call_id, "g-tool");

    // Result error, callback panic, preparation panic and rejection panic all
    // open one balanced noncancelled error turn before input receipts.
    for mode in ["error", "panic", "prepare-panic", "reject-panic", "prepare-error"] {
        let mut hooks = FinalHooks::new();
        if mode == "prepare-error" {
            hooks.provider_binding = Some(ProviderContextBinding { owned_dialect: true, ..Default::default() });
        }
        let hooks = Arc::new(hooks);
        hooks.panic_prepare.store(mode == "prepare-panic", Ordering::SeqCst);
        hooks.panic_reject.store(mode == "reject-panic", Ordering::SeqCst);
        hooks.binding.set_primary(Some(FinalCallback::new("failure", &visits, move |_, _| match mode {
            "error" => Err(FinalProviderGateError("gate failed".into())),
            "panic" => panic!("gate panicked"),
            "reject-panic" => Ok(Some(FinalProviderGateStop::default())),
            _ => Ok(None),
        })));
        let provider = Provider::new(Vec::new());
        let cfg = config(provider.clone(), tool(&effects), hooks.clone());
        let receipts = Receipts::default();
        let report =
            bounded(agent_loop(vec![user(mode)], &mut Vec::new(), &cfg, &CancellationToken::new(), &receipts)).await;
        assert_eq!(report.end, RunEnd::Error, "{mode}");
        assert_eq!(final_turn_counts(&receipts), (1, 1), "{mode}");
        assert!(provider.contexts.lock().unwrap().is_empty());
        let events = receipts.events.lock().unwrap();
        let start = events.iter().position(|event| matches!(event, AgentEvent::TurnStart)).unwrap();
        let input = events
            .iter()
            .position(|event| matches!(event, AgentEvent::MessageStart { message } if message.json().unwrap()["role"] == "user"))
            .unwrap();
        assert!(start < input);
        assert!(
            matches!(report.model_messages().unwrap().last(), Some(Message::Assistant(message)) if message.stop_reason == StopReason::Error)
        );
    }

    // Fresh/Continue throw+external abort: no request/rejection, append abort to
    // the report but emit neither its message events nor TurnEnd.
    for continuing in [false, true] {
        for deadline in [false, true] {
            let hold = Arc::new(Gate::default());
            let hooks = Arc::new(FinalHooks::new());
            hooks.binding.set_primary(Some(FinalCallback::held("abort-throw", &visits, &hold, |_, cancel| {
                assert!(cancel.is_cancelled());
                Err(FinalProviderGateError("gate cancelled while awaiting".into()))
            })));
            let provider = Provider::new(Vec::new());
            let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
            if deadline {
                cfg.deadline = Some(std::time::Instant::now() + Duration::from_millis(750))
            }
            let receipts = Arc::new(Receipts::default());
            let token = CancellationToken::new();
            let run_token = token.clone();
            let run_receipts = receipts.clone();
            let input = AgentInput { model: user("cancelled accepted input"), provenance: Some(provenance.clone()) };
            let run_input = input.clone();
            if continuing {
                hooks.queued.lock().unwrap().push(input.clone())
            }
            let run = tokio::spawn(async move {
                let mut context: Vec<AgentMessage> = if continuing { vec![user("history").into()] } else { Vec::new() };
                if continuing {
                    bounded(agent_loop_continue(
                        &mut context,
                        &cfg,
                        &run_token,
                        run_receipts.as_ref(),
                        UnpairedTail::Refuse,
                    ))
                    .await
                    .unwrap()
                } else {
                    bounded(agent_loop_inputs(vec![run_input], &mut context, &cfg, &run_token, run_receipts.as_ref()))
                        .await
                }
            });
            bounded(hold.started.notified()).await;
            if !deadline {
                token.cancel()
            }
            let report = bounded(run).await.unwrap();
            assert_eq!(report.end, if deadline { RunEnd::Deadline } else { RunEnd::Aborted });
            assert_eq!(
                report.stop_cause,
                Some(if deadline { RunStopCause::Deadline } else { RunStopCause::ExternalAbort })
            );
            assert_eq!(hooks.rejected.load(Ordering::SeqCst), 0);
            assert!(provider.contexts.lock().unwrap().is_empty());
            assert_eq!(receipts.inputs.lock().unwrap().as_slice(), &[input]);
            assert_eq!(final_turn_counts(&receipts), (1, 0));
            assert!(
                matches!(report.model_messages().unwrap().last(), Some(Message::Assistant(message)) if message.stop_reason == StopReason::Aborted)
            );
            assert!(!receipts.events.lock().unwrap().iter().any(|event| {
                matches!(
                    event,
                    AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } if message.json().unwrap()["role"] == "assistant"
                )
            }));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn group_g_live_owner_set_disposers_sampling_and_awaited_mutations() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let visits = Arc::new(Mutex::new(Vec::new()));
    let hooks = Arc::new(FinalHooks::new());
    let provider = Provider::new(vec![turn(Vec::new()), turn(Vec::new())]);
    let agent = Agent::new(config(provider, tool(&effects), hooks.clone()), Vec::new());
    assert_eq!(hooks.getter_reads.load(Ordering::SeqCst), 1, "adopt an empty Host owner once");
    agent.set_final_provider_gate(Some(FinalCallback::new("primary", &visits, |_, _| Ok(None))));
    assert!(hooks.binding.sample_for_run().is_some(), "facade must share the adopted empty owner");
    let same = FinalCallback::new("same", &visits, |_, _| Ok(None));
    let first = agent.add_final_provider_gate(same.clone());
    let second = agent.add_final_provider_gate(same.clone());
    let other = FinalCallback::new("other", &visits, |_, _| Ok(None));
    drop(agent.add_final_provider_gate(other)); // Drop deliberately keeps it.
    bounded(agent.prompt(vec![user("dedup")], CancellationToken::new(), Arc::new(Receipts::default()))).await.unwrap();
    assert_eq!(visits.lock().unwrap().as_slice(), &["primary", "same", "other"]);
    assert!(second.remove());
    assert!(!first.remove());
    let readded = agent.add_final_provider_gate(same);
    assert!(first.remove(), "old disposer deletes the readded allocation");
    assert!(!readded.remove());
    visits.lock().unwrap().clear();
    bounded(agent.prompt(vec![user("disposed readd")], CancellationToken::new(), Arc::new(Receipts::default())))
        .await
        .unwrap();
    assert_eq!(visits.lock().unwrap().as_slice(), &["primary", "other"]);
    assert_eq!(hooks.getter_reads.load(Ordering::SeqCst), 1, "no per-Run base getter or second primary");

    // Replacement during primary await affects only the next invocation; the
    // same Run's real tool continuation reads the new primary and live Set.
    visits.lock().unwrap().clear();
    let hooks = Arc::new(FinalHooks::new());
    let hold = Arc::new(Gate::default());
    hooks.binding.set_primary(Some(FinalCallback::held("old-primary", &visits, &hold, |_, _| Ok(None))));
    let a = hooks.binding.add(FinalCallback::new("a", &visits, |_, _| Ok(None)));
    let b = hooks.binding.add(FinalCallback::new("b", &visits, |_, _| Ok(None)));
    let provider = Provider::new(vec![turn(vec![call("live-gate", json!({"text":"live"}))]), turn(Vec::new())]);
    let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
    let run_agent = agent.clone();
    let run = tokio::spawn(async move {
        bounded(run_agent.prompt(vec![user("live replace")], CancellationToken::new(), Arc::new(Receipts::default())))
            .await
            .unwrap()
    });
    bounded(hold.started.notified()).await;
    assert!(b.remove());
    drop(hooks.binding.add(FinalCallback::new("c", &visits, |_, _| Ok(None))));
    agent.set_final_provider_gate(Some(FinalCallback::new("new-primary", &visits, |_, _| Ok(None))));
    hold.release.notify_one();
    assert_eq!(bounded(run).await.unwrap().end, RunEnd::Completed);
    assert_eq!(visits.lock().unwrap().as_slice(), &["old-primary", "a", "c", "new-primary", "a", "c"]);
    assert_eq!(provider.contexts.lock().unwrap().len(), 2);
    drop(a);

    // Additional traversal is live across its await, including remove/readd of
    // the currently visited object at its new insertion position.
    visits.lock().unwrap().clear();
    let hooks = Arc::new(FinalHooks::new());
    let hold = Arc::new(Gate::default());
    let current = FinalCallback::held("current", &visits, &hold, |_, _| Ok(None));
    let old_current = hooks.binding.add(current.clone());
    let remove_next = hooks.binding.add(FinalCallback::new("removed", &visits, |_, _| Ok(None)));
    drop(hooks.binding.add(FinalCallback::new("kept", &visits, |_, _| Ok(None))));
    let provider = Provider::new(vec![turn(Vec::new())]);
    let cfg = config(provider, tool(&effects), hooks.clone());
    let run = tokio::spawn(async move {
        bounded(agent_loop(
            vec![user("live Set")],
            &mut Vec::new(),
            &cfg,
            &CancellationToken::new(),
            &Receipts::default(),
        ))
        .await
    });
    bounded(hold.started.notified()).await;
    assert!(remove_next.remove());
    assert!(old_current.remove());
    drop(hooks.binding.add(FinalCallback::new("added", &visits, |_, _| Ok(None))));
    drop(hooks.binding.add(current));
    hold.release.notify_one();
    assert_eq!(bounded(run).await.unwrap().end, RunEnd::Completed);
    assert_eq!(visits.lock().unwrap().as_slice(), &["current", "kept", "added", "current"]);

    // Sampling precedes even AgentStart's await for direct and stateful entry.
    for stateful in [false, true] {
        visits.lock().unwrap().clear();
        let hooks = Arc::new(FinalHooks::new());
        let provider = Provider::new(vec![turn(Vec::new())]);
        let cfg = config(provider.clone(), tool(&effects), hooks.clone());
        let receipts = Arc::new(Receipts::default());
        let installed = FinalCallback::new("late-first", &visits, |_, _| Ok(Some(FinalProviderGateStop::default())));
        let sink = Arc::new(FinalSink {
            receipts: receipts.clone(),
            preparations: hooks.preparations.clone(),
            install: Some((hooks.binding.clone(), installed)),
        });
        if stateful {
            let agent = Agent::new(cfg, Vec::new());
            assert_eq!(
                bounded(agent.prompt(vec![user("first")], CancellationToken::new(), sink.clone())).await.unwrap().end,
                RunEnd::Completed
            );
            assert_eq!(
                bounded(agent.prompt(vec![user("next")], CancellationToken::new(), sink)).await.unwrap().end,
                RunEnd::PreModelStop
            );
        } else {
            let mut context = Vec::new();
            assert_eq!(
                bounded(agent_loop(vec![user("first")], &mut context, &cfg, &CancellationToken::new(), sink.as_ref()))
                    .await
                    .end,
                RunEnd::Completed
            );
            assert_eq!(
                bounded(agent_loop(vec![user("next")], &mut context, &cfg, &CancellationToken::new(), sink.as_ref()))
                    .await
                    .end,
                RunEnd::PreModelStop
            );
        }
        assert_eq!(provider.contexts.lock().unwrap().len(), 1);
        assert_eq!(visits.lock().unwrap().as_slice(), &["late-first"]);
    }

    // Stateful idle Continue must sample before consuming queue admission's
    // await, even though its later Core entry is a fresh prompt internally.
    let hold = Arc::new(Gate::default());
    let mut hooks = FinalHooks::new();
    hooks.dequeue_hold = Some(hold.clone());
    hooks.queued.get_mut().unwrap().push(AgentInput::from(user("idle queued input")));
    let hooks = Arc::new(hooks);
    let provider = Provider::new(vec![turn(Vec::new())]);
    let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
    let run_agent = agent.clone();
    let run = tokio::spawn(async move {
        bounded(run_agent.continue_run(CancellationToken::new(), Arc::new(Receipts::default()))).await.unwrap()
    });
    bounded(hold.started.notified()).await;
    hooks
        .binding
        .set_primary(Some(FinalCallback::new("idle-late", &visits, |_, _| Ok(Some(FinalProviderGateStop::default())))));
    hold.release.notify_one();
    assert_eq!(bounded(run).await.unwrap().end, RunEnd::Completed);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);
    assert_eq!(
        bounded(agent.prompt(vec![user("next after idle")], CancellationToken::new(), Arc::new(Receipts::default())))
            .await
            .unwrap()
            .end,
        RunEnd::PreModelStop
    );
    assert_eq!(hooks.getter_reads.load(Ordering::SeqCst), 1);

    // An enabled Run remains installed even when emptied during an await, so
    // the later abort forces a closed stop rather than sending an empty gate.
    let hooks = Arc::new(FinalHooks::new());
    let hold = Arc::new(Gate::default());
    hooks.binding.set_primary(Some(FinalCallback::held("emptied", &visits, &hold, |_, _| Ok(None))));
    let additional = hooks.binding.add(FinalCallback::new("must-not-run", &visits, |_, _| panic!("after stop")));
    let provider = Provider::new(Vec::new());
    let cfg = config(provider.clone(), tool(&effects), hooks.clone());
    let token = CancellationToken::new();
    let run_token = token.clone();
    let receipts = Arc::new(Receipts::default());
    let run_receipts = receipts.clone();
    let run = tokio::spawn(async move {
        bounded(agent_loop(vec![user("empty then abort")], &mut Vec::new(), &cfg, &run_token, run_receipts.as_ref()))
            .await
    });
    bounded(hold.started.notified()).await;
    hooks.binding.set_primary(None);
    assert!(additional.remove());
    assert!(hooks.binding.sample_for_run().is_none());
    token.cancel();
    let report = bounded(run).await.unwrap();
    assert_eq!(report.end, RunEnd::Aborted);
    assert_eq!(report.stop_cause, Some(RunStopCause::ExternalAbort));
    assert_eq!(hooks.rejected.load(Ordering::SeqCst), 0);
    assert_eq!(final_turn_counts(&receipts), (0, 0));
    assert!(provider.contexts.lock().unwrap().is_empty());

    // Stop short-circuits the Set even with subsequent live callbacks present.
    let hooks = Arc::new(FinalHooks::new());
    hooks.binding.set_primary(Some(FinalCallback::new("first-stop", &visits, |_, _| {
        Ok(Some(FinalProviderGateStop::default()))
    })));
    drop(hooks.binding.add(FinalCallback::new("unreached", &visits, |_, _| panic!("after owner stop"))));
    let cfg = config(Provider::new(Vec::new()), tool(&effects), hooks);
    assert_eq!(
        bounded(agent_loop(
            vec![user("short circuit")],
            &mut Vec::new(),
            &cfg,
            &CancellationToken::new(),
            &Receipts::default()
        ))
        .await
        .end,
        RunEnd::PreModelStop
    );
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("group must make bounded progress")
}

#[derive(Default)]
struct Gate {
    started: Notify,
    release: Notify,
    cancelled: AtomicBool,
}

struct Turn {
    calls: Vec<ToolCall>,
    stop: StopReason,
    error_event: bool,
    stop_details: Option<Value>,
    hold: Option<Arc<Gate>>,
}
fn call(id: &str, args: Value) -> ToolCall {
    ToolCall { id: id.into(), name: "echo".into(), arguments: args.as_object().unwrap().clone(), ..Default::default() }
}
fn turn(calls: Vec<ToolCall>) -> Turn {
    let stop = if calls.is_empty() { StopReason::Stop } else { StopReason::ToolUse };
    Turn { calls, stop, error_event: false, stop_details: None, hold: None }
}

struct Provider {
    turns: Mutex<VecDeque<Turn>>,
    contexts: Mutex<Vec<Context>>,
    tool_choices: Mutex<Vec<Option<ToolChoice>>>,
}
impl Provider {
    fn new(turns: Vec<Turn>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            contexts: Mutex::new(Vec::new()),
            tool_choices: Mutex::new(Vec::new()),
        })
    }
}
impl ModelProvider for Provider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        self.contexts.lock().unwrap().push(context.clone());
        self.tool_choices.lock().unwrap().push(options.tool_choice.clone());
        let turn = self.turns.lock().unwrap().pop_front().expect("unexpected model call");
        let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            sink.push(AssistantMessageEvent::Start { partial: message.clone() }).await;
            for tool_call in turn.calls {
                message.content.push(AssistantBlock::ToolCall(tool_call.clone()));
                sink.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: message.content.len() - 1,
                    tool_call,
                    partial: message.clone(),
                })
                .await;
            }
            if let Some(hold) = turn.hold {
                hold.started.notify_one();
                tokio::select! { _ = hold.release.notified() => {}, _ = options.cancel.cancelled() => {} }
            }
            message.stop_reason = turn.stop;
            message.stop_details = turn.stop_details;
            if turn.error_event {
                sink.push(AssistantMessageEvent::Error { reason: turn.stop, error: message }).await;
            } else {
                sink.push(AssistantMessageEvent::Done { reason: turn.stop, message }).await;
            }
        });
        stream
    }
}

#[derive(Default)]
struct Receipts {
    events: Mutex<Vec<AgentEvent>>,
    inputs: Mutex<Vec<AgentInput>>,
}
#[async_trait]
impl AgentEventSink for Receipts {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event
            && let AgentMessageBody::Model(model) = message.body_snapshot()
            && !matches!(model, Message::Assistant(_) | Message::ToolResult(_))
        {
            self.inputs.lock().unwrap().push(AgentInput { model, provenance: message.source() });
        }
        self.events.lock().unwrap().push(event);
    }
}

#[derive(Clone)]
struct Effect {
    id: String,
    args: JsonObject,
    hard_cancelled: bool,
    soft_cancelled: bool,
    context: Option<Value>,
}
struct Echo {
    effects: Arc<Mutex<Vec<Effect>>>,
    waiting: Option<Arc<Gate>>,
    foreground: Option<Arc<Gate>>,
}
impl Echo {
    async fn run(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        context: Option<ToolExecutionContext>,
    ) -> Result<ToolOutput, ToolError> {
        let mode = args.get("mode").and_then(Value::as_str).unwrap_or("normal");
        let gate = if mode == "foreground" {
            self.foreground.as_ref()
        } else if mode == "wait" && id != "pending" {
            self.waiting.as_ref()
        } else {
            None
        };
        if let Some(gate) = gate {
            gate.started.notify_one();
            if mode == "wait" {
                tokio::select! {
                    _ = gate.release.notified() => {},
                    _ = cancel.cancelled() => { gate.cancelled.store(true, Ordering::SeqCst); return Err(ToolError("wait interrupted after entering execute".into())); }
                }
            } else {
                gate.release.notified().await;
            }
        }
        let soft_cancelled = context.as_ref().is_some_and(|context| context.steering_signal.is_cancelled());
        let context_json = context.as_ref().map(|context| {
            json!({"batch":context.batch_id,"index":context.index,"total":context.total,
            "calls":context.tool_calls.iter().map(|call| json!({"id":call.id,"name":call.name})).collect::<Vec<_>>(),
            "metadata":context.provider_metadata})
        });
        self.effects.lock().unwrap().push(Effect {
            id: id.into(),
            args: args.clone(),
            hard_cancelled: cancel.is_cancelled(),
            soft_cancelled,
            context: context_json,
        });
        let mut output = if mode == "foreground" {
            ToolOutput::error("foreground genuine failure")
        } else {
            ToolOutput::text(args.get("text").and_then(Value::as_str).unwrap_or("empty"))
        };
        output.details = Some(json!({"execute":id}));
        output.provider_metadata = snapshot_tool_result_provider_metadata(Some(&json!({"type":"computer",
            "screenshot":{"type":"computer_screenshot","file_id":"result-ref"},"acknowledgedSafetyChecks":[]})))
        .unwrap();
        Ok(output)
    }
}
#[async_trait]
impl AgentTool for Echo {
    fn definition(&self) -> Tool {
        Tool {
            name: "echo".into(),
            description: "controlled effect".into(),
            parameters: json!({"type":"object",
            "properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":true}),
            ..Default::default()
        }
    }
    fn concurrency(&self, args: &JsonObject) -> Concurrency {
        if args.get("exclusive") == Some(&Value::Bool(true)) { Concurrency::Exclusive } else { Concurrency::Shared }
    }
    fn interruptible(&self, args: &JsonObject) -> bool {
        args.get("mode").and_then(Value::as_str) == Some("wait")
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.run(id, args, cancel, None).await
    }
    async fn execute_with_context(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
        context: ToolExecutionContext,
    ) -> Result<ToolOutput, ToolError> {
        self.run(id, args, cancel, Some(context)).await
    }
}

// Exercises the existing execute-only adapter together with Native lenient
// parse fallback and conservative policy resolution, in the T family.
struct LenientTool(Arc<Echo>);
#[async_trait]
impl AgentTool for LenientTool {
    fn definition(&self) -> Tool {
        self.0.definition()
    }
    fn lenient_argument_validation(&self) -> bool {
        true
    }
    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        panic!("policy callback failed")
    }
    fn interruptible(&self, _args: &JsonObject) -> bool {
        panic!("interrupt policy failed")
    }
    async fn execute(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.0.execute(id, args, cancel, update).await
    }
}

fn config(provider: Arc<Provider>, tool: Arc<dyn AgentTool>, hooks: Arc<dyn LoopHooks>) -> AgentConfig {
    AgentConfig {
        model: Model {
            id: "core-hooks".into(),
            api: "openai-completions".into(),
            provider: "fixture".into(),
            base_url: String::new(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        },
        provider,
        system_prompt: vec!["fixed context".into()],
        tools: vec![tool],
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: Some(3),
        hooks,
        interrupt_mode: InterruptMode::Immediate,
    }
}
fn tool(effects: &Arc<Mutex<Vec<Effect>>>) -> Arc<Echo> {
    Arc::new(Echo { effects: effects.clone(), waiting: None, foreground: None })
}
fn user(text: &str) -> Message {
    Message::User(UserMessage::text(text))
}
fn text(content: &[UserBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn result(report: &RunReport, id: &str) -> ToolResultMessage {
    report
        .model_messages()
        .unwrap()
        .into_iter()
        .find_map(|message| match message {
            Message::ToolResult(result) if result.tool_call_id == id => Some(result),
            _ => None,
        })
        .unwrap()
}

struct Admission {
    name: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
    gate: Option<Arc<Gate>>,
    first: AtomicBool,
    fail: bool,
}
#[async_trait]
impl AgentAdmissionHook for Admission {
    async fn run(&self, _cancel: &CancellationToken) -> Result<(), AdmissionError> {
        self.log.lock().unwrap().push(self.name);
        if let Some(gate) = &self.gate
            && !self.first.swap(true, Ordering::SeqCst)
        {
            gate.started.notify_one();
            gate.release.notified().await;
        }
        if self.fail { Err(AdmissionError::Blocked("preserve pending input".into())) } else { Ok(()) }
    }
}
fn admission(
    name: &'static str,
    log: &Arc<Mutex<Vec<&'static str>>>,
    gate: Option<Arc<Gate>>,
    fail: bool,
) -> Arc<Admission> {
    Arc::new(Admission { name, log: log.clone(), gate, first: AtomicBool::new(false), fail })
}

#[tokio::test]
async fn group_r_live_registration_traversal_duplicate_drop_and_owned_dequeue_failure() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![turn(vec![]), turn(vec![])]);
    let agent = Agent::new(config(provider.clone(), tool(&effects), Arc::new(NoHooks)), vec![]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Gate::default());
    let _dequeue = agent.add_before_queued_message_dequeue_hook(admission("dequeue", &log, None, false));
    let _a = agent.add_before_model_call_hook(admission("A", &log, Some(gate.clone()), false));
    let b = agent.add_before_model_call_hook(admission("B", &log, None, false));
    agent.steer(user("owned queued input"));
    let running = agent.clone();
    let pending = tokio::spawn(async move {
        running.prompt(vec![user("prompt")], CancellationToken::new(), Arc::new(NullSink)).await
    });
    bounded(gate.started.notified()).await;
    assert!(b.remove());
    assert!(!b.remove());
    let _c = agent.add_before_model_call_hook(admission("C", &log, None, false));
    let same = admission("D", &log, None, false);
    drop(agent.add_before_model_call_hook(same.clone()));
    let d = agent.add_before_model_call_hook(same);
    assert!(d.remove());
    gate.release.notify_one();
    assert_eq!(bounded(pending).await.unwrap().unwrap().end, RunEnd::Completed);
    assert_eq!(*log.lock().unwrap(), vec!["dequeue", "A", "C", "D"]);
    agent.prompt(vec![user("empty queue")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap();
    assert_eq!(*log.lock().unwrap(), vec!["dequeue", "A", "C", "D", "A", "C", "D"]);
    assert!(effects.lock().unwrap().is_empty());
    let failed = Agent::new(config(Provider::new(vec![]), tool(&effects), Arc::new(NoHooks)), vec![]);
    let input = AgentInput {
        model: user("retain exact projection"),
        provenance: Some(Arc::new(json!({"source":"opaque-host-value"}))),
    };
    failed.steer_input_with_source(input.clone(), SteeringSource::Agent);
    let _f = failed.add_before_queued_message_dequeue_hook(admission("failure", &log, None, true));
    assert_eq!(
        failed.prompt(vec![user("new")], CancellationToken::new(), Arc::new(NullSink)).await.unwrap().end,
        RunEnd::Error
    );
    assert_eq!(failed.peek_steering_inputs(), vec![input]);

    // No registration preserves one legacy request snapshot. A registered
    // callback can update the Host binding before its coherent refresh.
    struct RefreshSnapshot {
        value: AtomicUsize,
        reads: AtomicUsize,
    }
    impl LoopHooks for RefreshSnapshot {
        fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
            Some(false)
        }
        fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Some(ExecutionSnapshot {
                tools: vec![],
                system_prompt: vec![format!("slot {}", self.value.load(Ordering::SeqCst))],
                model: None,
                provider_context: None,
            })
        }
    }
    #[async_trait]
    impl AgentAdmissionHook for RefreshSnapshot {
        async fn run(&self, _cancel: &CancellationToken) -> Result<(), AdmissionError> {
            self.value.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    let provider = Provider::new(vec![turn(vec![]), turn(vec![]), turn(vec![])]);
    let refresh = Arc::new(RefreshSnapshot { value: AtomicUsize::new(0), reads: AtomicUsize::new(0) });
    let agent = Agent::new(config(provider.clone(), self::tool(&effects), refresh.clone()), vec![]);
    bounded(agent.prompt(vec![user("no registration")], CancellationToken::new(), Arc::new(NullSink))).await.unwrap();
    assert_eq!(refresh.reads.load(Ordering::SeqCst), 1);
    assert_eq!(provider.contexts.lock().unwrap()[0].system_prompt, ["slot 0"]);
    let registration = agent.add_before_model_call_hook(refresh.clone());
    bounded(agent.prompt(vec![user("refresh after callback")], CancellationToken::new(), Arc::new(NullSink)))
        .await
        .unwrap();
    assert_eq!(refresh.reads.load(Ordering::SeqCst), 3);
    assert_eq!(provider.contexts.lock().unwrap()[1].system_prompt, ["slot 1"]);
    assert!(registration.remove());
    bounded(agent.prompt(vec![user("removed callback")], CancellationToken::new(), Arc::new(NullSink))).await.unwrap();
    assert_eq!(refresh.reads.load(Ordering::SeqCst), 4);
}

struct LifecycleHooks {
    tool: Arc<Echo>,
    before: Mutex<Vec<String>>,
    legacy: Mutex<Vec<(String, JsonObject)>>,
    after: Mutex<Vec<String>>,
    transformed: AtomicUsize,
    turns: AtomicUsize,
}
impl LifecycleHooks {
    fn new(tool: Arc<Echo>) -> Arc<Self> {
        Arc::new(Self {
            tool,
            before: Mutex::new(Vec::new()),
            legacy: Mutex::new(Vec::new()),
            after: Mutex::new(Vec::new()),
            transformed: AtomicUsize::new(0),
            turns: AtomicUsize::new(0),
        })
    }
}
#[async_trait]
impl LoopHooks for LifecycleHooks {
    fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
        Some(false)
    }
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        _cancel: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        self.transformed.fetch_add(1, Ordering::SeqCst);
        message.content.push(AssistantBlock::text("final rewrite"));
        Ok(())
    }
    fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        (name == "fallback").then(|| self.tool.clone() as Arc<dyn AgentTool>)
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut BeforeToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        self.before.lock().unwrap().push(context.tool_call.id.clone());
        assert_eq!(context.context.system_prompt, ["fixed context"]);
        context.args.insert("text".into(), json!("in-place"));
        match context.tool_call.id.as_str() {
            "replace" => Ok(BeforeToolCallResult {
                args: Some(json!({"text":"replacement","exclusive":true}).as_object().unwrap().clone()),
                ..Default::default()
            }),
            "blocked" => Ok(BeforeToolCallResult {
                block: true,
                reason: Some("rich blocked".into()),
                args: Some(JsonObject::new()),
            }),
            "invalid" => Ok(BeforeToolCallResult {
                args: Some(json!({"text":42}).as_object().unwrap().clone()),
                ..Default::default()
            }),
            "before-error" => Err(ToolHookError("rich preparation error".into())),
            _ => Ok(BeforeToolCallResult::default()),
        }
    }
    async fn before_tool_call(&self, call: &ToolCall, args: &JsonObject, _cancel: &CancellationToken) -> ToolDecision {
        self.legacy.lock().unwrap().push((call.id.clone(), args.clone()));
        ToolDecision::Allow
    }
    fn transform_tool_call_arguments(&self, mut args: JsonObject, _name: &str) -> Result<JsonObject, ToolHookError> {
        args.insert("late".into(), json!(true));
        Ok(args)
    }
    async fn after_tool_call(
        &self,
        context: AfterToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<AfterToolCallResult>, ToolHookError> {
        self.after.lock().unwrap().push(context.tool_call.id.clone());
        match context.tool_call.id.as_str() {
            "inplace" => {
                context.result.content = vec![UserBlock::text("mutated without return")];
                context.result.details = Some(json!({"inplace":true}));
                context.result.is_error = true;
                context.result.useless = true;
                Ok(None)
            }
            "replace" => {
                assert_eq!(context.args["late"], true);
                context.result.details = Some(json!({"mutated":true}));
                Ok(Some(AfterToolCallResult {
                    content: Some(vec![UserBlock::text("returned override")]),
                    details: Some(Value::Null),
                    provider_metadata: Some(Value::Null),
                    ..Default::default()
                }))
            }
            "metadata-bad" => Ok(Some(AfterToolCallResult {
                provider_metadata: Some(json!({"type":"computer",
                "screenshot":{"type":"computer_screenshot","image_url":"one","file_id":null},"acknowledgedSafetyChecks":[]})),
                is_error: Some(false),
                useless: Some(true),
                ..Default::default()
            })),
            "before-error" => Ok(Some(AfterToolCallResult {
                content: Some(vec![UserBlock::text("hook recovered")]),
                is_error: Some(false),
                ..Default::default()
            })),
            _ => Ok(None),
        }
    }
    async fn turn_end_inputs(
        &self,
        _message: &AssistantMessage,
        _results: &[ToolResultMessage],
        _cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        Vec::new()
    }
}

#[tokio::test]
async fn group_t_prepared_revisions_one_guard_after_mutation_metadata_and_receipt_roundtrip() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool = tool(&effects);
    let hooks = LifecycleHooks::new(tool.clone());
    let mut calls: Vec<_> = ["replace", "inplace", "blocked", "invalid", "before-error", "metadata-bad", "fallback"]
        .iter()
        .map(|id| call(id, json!({"text":"authored"})))
        .collect();
    calls[0].provider_metadata =
        snapshot_tool_call_provider_metadata(Some(&json!({"type":"computer","providerItemId":"item",
        "actions":[{"type":"screenshot","nativeExtra":true}],"pendingSafetyChecks":[{"id":"check","code":null}]})));
    calls.last_mut().unwrap().name = "fallback".into();
    let provider = Provider::new(vec![turn(calls), turn(vec![])]);
    let sink = Arc::new(Receipts::default());
    let agent = Agent::new(config(provider.clone(), tool, hooks.clone()), vec![]);
    let report = bounded(agent.prompt(vec![user("run")], CancellationToken::new(), sink.clone())).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(hooks.transformed.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.turns.load(Ordering::SeqCst), 2);
    assert_eq!(
        hooks.legacy.lock().unwrap().iter().map(|entry| entry.0.as_str()).collect::<Vec<_>>(),
        vec!["replace", "inplace", "metadata-bad", "fallback"]
    );
    assert_eq!(hooks.legacy.lock().unwrap()[0].1["text"], "replacement");
    let effects = effects.lock().unwrap().clone();
    assert_eq!(effects.len(), 4);
    assert!(effects.iter().all(|effect| effect.args["late"] == true));
    let replace_effect = effects.iter().find(|effect| effect.id == "replace").unwrap();
    assert_eq!(replace_effect.context.as_ref().unwrap()["metadata"]["actions"][0]["nativeExtra"], true);
    assert_eq!(replace_effect.context.as_ref().unwrap()["total"], 7);
    assert_eq!(text(&result(&report, "replace").content), "returned override");
    assert_eq!(result(&report, "replace").details, Some(json!({"mutated":true})));
    assert!(result(&report, "replace").provider_metadata.is_some());
    let inplace = result(&report, "inplace");
    assert_eq!(text(&inplace.content), "mutated without return");
    assert!(!inplace.is_error);
    assert!(inplace.useless);
    assert_eq!(inplace.details, Some(json!({"inplace":true})));
    {
        let events = sink.events.lock().unwrap();
        assert!(
            events.iter().any(|event| matches!(event,AgentEvent::ToolExecutionEnd{tool_call_id,result,is_error,..}
            if tool_call_id=="inplace" && result.is_error && !*is_error && result.useless))
        );
        let final_message = events
            .iter()
            .find_map(|event| match event {
                AgentEvent::MessageEnd { .. } => match ended_model_message(event) {
                    Some(Message::Assistant(message)) if message.tool_calls().next().is_some() => Some(message),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(
            final_message.tool_calls().find(|call| call.id == "replace").unwrap().arguments["text"],
            "replacement"
        );
        assert_eq!(final_message.tool_calls().find(|call| call.id == "inplace").unwrap().arguments["text"], "in-place");
        assert_eq!(
            provider.contexts.lock().unwrap()[1].messages.iter().find_map(|message| match message {
                Message::Assistant(message) => Some(message),
                _ => None,
            }),
            Some(&final_message)
        );
        assert!(!hooks.after.lock().unwrap().iter().any(|id| id == "invalid"));
        assert_eq!(text(&result(&report, "before-error").content), "hook recovered");
        let bad = result(&report, "metadata-bad");
        assert!(bad.is_error && !bad.useless && bad.provider_metadata.is_none());
        assert!(text(&bad.content).contains("unsupported shape"));
    }
    let roundtrip: Vec<Message> =
        serde_json::from_value(serde_json::to_value(report.model_messages().unwrap()).unwrap()).unwrap();
    assert_eq!(roundtrip, report.model_messages().unwrap());
    let invalid = coerce_raw_tool_result(&json!({"content":"not-array","isError":false,"useless":true}));
    assert!(invalid.is_error && !invalid.useless);

    let lenient_effects = Arc::new(Mutex::new(Vec::new()));
    let lenient_config =
        config(Provider::new(vec![]), Arc::new(LenientTool(self::tool(&lenient_effects))), Arc::new(NoHooks));
    let mut lenient_message = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    lenient_message.content = vec![AssistantBlock::ToolCall(call(
        "lenient",
        json!({"text":"retained","__parseError":"bad parse","__rawJson":"private raw payload"}),
    ))];
    let lenient_results =
        bounded(execute_tool_calls(&lenient_message, &lenient_config, &CancellationToken::new(), &NullSink)).await;
    assert!(!lenient_results[0].is_error);
    let lenient_effects = lenient_effects.lock().unwrap();
    assert_eq!(lenient_effects[0].args, json!({"text":"retained"}).as_object().unwrap().clone());
    assert!(lenient_effects[0].context.is_none());
}

#[tokio::test]
async fn group_t_native_done_error_length_prepare_but_external_abort_unwinds_without_hooks() {
    for (stop, error_event) in [(StopReason::Length, false), (StopReason::Error, true)] {
        let effects = Arc::new(Mutex::new(Vec::new()));
        let tool = tool(&effects);
        let hooks = LifecycleHooks::new(tool.clone());
        let first = Turn {
            calls: vec![call("replace", json!({"text":"authored"}))],
            stop,
            error_event,
            stop_details: None,
            hold: None,
        };
        let provider = Provider::new(if stop == StopReason::Length { vec![first, turn(vec![])] } else { vec![first] });
        let agent = Agent::new(config(provider, tool, hooks.clone()), vec![]);
        let report =
            bounded(agent.prompt(vec![user("run")], CancellationToken::new(), Arc::new(NullSink))).await.unwrap();
        assert_eq!(hooks.before.lock().unwrap().len(), 1);
        assert_eq!(hooks.legacy.lock().unwrap().len(), 1);
        assert!(effects.lock().unwrap().is_empty());
        assert!(result(&report, "replace").details.as_ref().unwrap()["__synthetic"] == true);
    }
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool = tool(&effects);
    let hooks = LifecycleHooks::new(tool.clone());
    let hold = Arc::new(Gate::default());
    let provider = Provider::new(vec![Turn {
        calls: vec![call("replace", json!({"text":"authored"}))],
        stop: StopReason::Aborted,
        error_event: true,
        stop_details: None,
        hold: Some(hold.clone()),
    }]);
    let agent = Agent::new(config(provider, tool, hooks.clone()), vec![]);
    let cancel = CancellationToken::new();
    let signal = cancel.clone();
    let pending = tokio::spawn(async move { agent.prompt(vec![user("run")], signal, Arc::new(NullSink)).await });
    bounded(hold.started.notified()).await;
    cancel.cancel();
    let report = bounded(pending).await.unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Aborted);
    assert_eq!(hooks.transformed.load(Ordering::SeqCst), 0);
    assert!(hooks.before.lock().unwrap().is_empty());
    assert!(hooks.legacy.lock().unwrap().is_empty());
    assert!(effects.lock().unwrap().is_empty());
    assert_eq!(result(&report, "replace").details.as_ref().unwrap()["executed"], false);
}

struct TerminalHooks {
    agent: Mutex<Weak<Agent>>,
    handle: Mutex<Option<TerminalToolResultHandle>>,
    turns: AtomicUsize,
}
#[async_trait]
impl LoopHooks for TerminalHooks {
    fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
        Some(false)
    }
    async fn after_tool_call(
        &self,
        context: AfterToolCallContext<'_>,
        _cancel: &CancellationToken,
    ) -> Result<Option<AfterToolCallResult>, ToolHookError> {
        self.agent
            .lock()
            .unwrap()
            .upgrade()
            .unwrap()
            .steer_input_with_source(user("retain queued work").into(), SteeringSource::System);
        *self.handle.lock().unwrap() = Some(context.terminal.clone());
        assert!(context.terminal.request_terminal_result());
        context.result.content = vec![UserBlock::text("completed terminal evidence")];
        Ok(None)
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.turns.fetch_add(1, Ordering::SeqCst);
        assert!(!cancel.is_cancelled());
        assert!(!context.will_continue);
        assert!(matches!(context.stop_cause, Some(RunStopCause::TerminalToolResult { .. })));
        vec![user("turn end receipt").into()]
    }
}
#[tokio::test]
async fn group_t_terminal_result_is_scoped_preserves_queue_and_calls_turn_end_once() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let tool = tool(&effects);
    let provider = Provider::new(vec![turn(vec![call("terminal", json!({"text":"done"}))])]);
    let hooks = Arc::new(TerminalHooks {
        agent: Mutex::new(Weak::new()),
        handle: Mutex::new(None),
        turns: AtomicUsize::new(0),
    });
    let agent = Agent::new(config(provider.clone(), tool, hooks.clone()), vec![]);
    *hooks.agent.lock().unwrap() = Arc::downgrade(&agent);
    let parent = CancellationToken::new();
    let report = bounded(agent.prompt(vec![user("run")], parent.clone(), Arc::new(NullSink))).await.unwrap();
    assert_eq!(report.end, RunEnd::TerminalToolResult);
    assert_eq!(report.stop_cause, Some(RunStopCause::TerminalToolResult { tool_call_id: "terminal".into() }));
    assert_eq!(text(&result(&report, "terminal").content), "completed terminal evidence");
    assert!(!parent.is_cancelled());
    assert_eq!(agent.queued_counts(), (1, 0));
    assert_eq!(hooks.turns.load(Ordering::SeqCst), 1);
    assert_eq!(provider.contexts.lock().unwrap().len(), 1);
    assert!(!hooks.handle.lock().unwrap().as_ref().unwrap().request_terminal_result());
    assert!(
        report
            .model_messages()
            .unwrap()
            .into_iter()
            .any(|message| matches!(message,Message::User(message)if message.content.plain_text()=="turn end receipt"))
    );
}

struct ExternalQueue {
    queued: AtomicBool,
    peeks: AtomicUsize,
}
#[async_trait]
impl SteeringQueuePeek for ExternalQueue {
    async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
        self.peeks.fetch_add(1, Ordering::SeqCst);
        Ok(SteeringQueueState { queued: self.queued.load(Ordering::SeqCst), source: Some(SteeringSource::System) })
    }
}
struct ExternalHooks(Arc<ExternalQueue>);
#[async_trait]
impl LoopHooks for ExternalHooks {
    fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
        Some(self.0.queued.load(Ordering::SeqCst))
    }
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        Some(SteeringInterruptBinding::new(self.0.clone(), None))
    }
    async fn steering_inputs(&self) -> Vec<AgentInput> {
        if self.0.queued.swap(false, Ordering::SeqCst) { vec![user("external advisory").into()] } else { vec![] }
    }
}

struct IrcQueue(AtomicBool);
#[async_trait]
impl IrcInterruptPeek for IrcQueue {
    async fn queued(&self) -> Result<bool, ToolHookError> {
        Ok(self.0.load(Ordering::SeqCst))
    }
}
struct IrcHooks(Arc<IrcQueue>, Option<SteeringInterruptBinding>);
impl LoopHooks for IrcHooks {
    fn irc_interrupt_port(&self) -> Option<Arc<dyn IrcInterruptPeek>> {
        Some(self.0.clone())
    }
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        self.1.clone()
    }
}

async fn interruption_family(mode: InterruptMode, external_queue: bool) {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let waiting = Arc::new(Gate::default());
    let foreground = Arc::new(Gate::default());
    let tool = Arc::new(Echo {
        effects: effects.clone(),
        waiting: Some(waiting.clone()),
        foreground: Some(foreground.clone()),
    });
    let provider = Provider::new(vec![
        turn(vec![
            call("running", json!({"text":"wait","mode":"wait"})),
            call("foreground", json!({"text":"write","mode":"foreground"})),
            call("pending", json!({"text":"pending","mode":"wait","exclusive":true})),
        ]),
        turn(vec![]),
    ]);
    let external = Arc::new(ExternalQueue { queued: AtomicBool::new(false), peeks: AtomicUsize::new(0) });
    let hooks: Arc<dyn LoopHooks> =
        if external_queue { Arc::new(ExternalHooks(external.clone())) } else { Arc::new(NoHooks) };
    let mut config = config(provider.clone(), tool, hooks);
    config.interrupt_mode = mode;
    let agent = Agent::new(config, vec![]);
    let sink = Arc::new(Receipts::default());
    let run_agent = agent.clone();
    let run_sink = sink.clone();
    let pending =
        tokio::spawn(async move { run_agent.prompt(vec![user("run")], CancellationToken::new(), run_sink).await });
    bounded(waiting.started.notified()).await;
    bounded(foreground.started.notified()).await;
    let original =
        AgentInput { model: user("parent steering"), provenance: Some(Arc::new(json!({"opaque":"preserve"}))) };
    if mode == InterruptMode::Immediate {
        agent.set_interrupt_mode(InterruptMode::Wait);
    }
    if external_queue {
        external.queued.store(true, Ordering::SeqCst);
    } else {
        agent.steer_input_with_source(original.clone(), SteeringSource::Agent);
    }
    if mode == InterruptMode::Wait {
        waiting.release.notify_one();
    } else {
        bounded(async {
            while !waiting.cancelled.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await;
    }
    foreground.release.notify_one();
    let report = bounded(pending).await.unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    let running = result(&report, "running");
    let pending_result = result(&report, "pending");
    let foreground_result = result(&report, "foreground");
    if mode == InterruptMode::Immediate {
        assert_eq!(running.details.as_ref().unwrap()["__interrupted"], true);
        assert_eq!(running.details.as_ref().unwrap()["execution"], "started");
        assert!(running.details.as_ref().unwrap().get("executed").is_none());
        assert_eq!(pending_result.details.as_ref().unwrap()["executed"], false);
        assert!(!effects.lock().unwrap().iter().any(|effect| effect.id == "pending"));
        if !external_queue {
            assert!(text(&running.content).contains("pending parent steering message"));
        }
    } else {
        assert!(!running.is_error);
        assert!(!pending_result.is_error);
        assert!(!waiting.cancelled.load(Ordering::SeqCst));
    }
    assert!(foreground_result.is_error);
    assert_eq!(text(&foreground_result.content), "foreground genuine failure");
    assert!(foreground_result.details.as_ref().unwrap().get("__interrupted").is_none());
    let foreground_effect = effects.lock().unwrap().iter().find(|effect| effect.id == "foreground").cloned().unwrap();
    assert!(!foreground_effect.hard_cancelled);
    assert_eq!(foreground_effect.soft_cancelled, mode == InterruptMode::Immediate);
    let events = sink.events.lock().unwrap();
    for id in ["running", "foreground", "pending"] {
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event,AgentEvent::ToolExecutionEnd{tool_call_id,..}if tool_call_id==id))
                .count(),
            1
        );
    }
    drop(events);
    if external_queue {
        assert!(external.peeks.load(Ordering::SeqCst) > 0);
        assert!(provider.contexts.lock().unwrap()[1].messages.iter().any(
            |message| matches!(message,Message::User(message)if message.content.plain_text()=="external advisory")
        ));
    } else {
        assert!(sink.inputs.lock().unwrap().contains(&original));
    }
    assert_eq!(agent.queued_counts(), (0, 0));
    assert_eq!(agent.interrupt_mode(), InterruptMode::Wait);
}

#[tokio::test]
async fn group_i_interruptible_started_pending_foreground_wait_mode_and_composite_poll_fallback() {
    interruption_family(InterruptMode::Immediate, false).await;
    interruption_family(InterruptMode::Wait, false).await;
    interruption_family(InterruptMode::Immediate, true).await;

    // IRC has no steering wake and retains its independent timer source.
    let effects = Arc::new(Mutex::new(Vec::new()));
    let waiting = Arc::new(Gate::default());
    let foreground = Arc::new(Gate::default());
    let echo = Arc::new(Echo {
        effects: effects.clone(),
        waiting: Some(waiting.clone()),
        foreground: Some(foreground.clone()),
    });
    let irc = Arc::new(IrcQueue(AtomicBool::new(false)));
    let irc_config = config(Provider::new(vec![]), echo, Arc::new(IrcHooks(irc.clone(), None)));
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    message.content = vec![
        AssistantBlock::ToolCall(call("running", json!({"text":"wait","mode":"wait"}))),
        AssistantBlock::ToolCall(call("foreground", json!({"text":"write","mode":"foreground"}))),
        AssistantBlock::ToolCall(call("pending", json!({"text":"pending","mode":"wait","exclusive":true}))),
    ];
    let executing =
        tokio::spawn(
            async move { execute_tool_calls(&message, &irc_config, &CancellationToken::new(), &NullSink).await },
        );
    bounded(waiting.started.notified()).await;
    bounded(foreground.started.notified()).await;
    irc.0.store(true, Ordering::SeqCst);
    bounded(async {
        while !waiting.cancelled.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    foreground.release.notify_one();
    let results = bounded(executing).await.unwrap();
    let running = results.iter().find(|result| result.tool_call_id == "running").unwrap();
    assert!(text(&running.content).contains("pending peer interrupt"));
    assert_eq!(running.details.as_ref().unwrap()["execution"], "started");
    let foreground = effects.lock().unwrap().iter().find(|effect| effect.id == "foreground").cloned().unwrap();
    assert!(!foreground.hard_cancelled && foreground.soft_cancelled);

    // IRC's initial receipt stays attributed to IRC. While foreground work
    // remains in flight, the poll fallback must upgrade the pending tail to
    // the first steering source, then retain it across subsequent peeks.
    struct LateSteering {
        source: Mutex<Option<SteeringSource>>,
        user_seen: Notify,
        system_seen: Notify,
    }
    #[async_trait]
    impl SteeringQueuePeek for LateSteering {
        async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
            let source = *self.source.lock().unwrap();
            match source {
                Some(SteeringSource::User) => self.user_seen.notify_one(),
                Some(SteeringSource::System) => self.system_seen.notify_one(),
                _ => {}
            }
            Ok(SteeringQueueState { queued: source.is_some(), source })
        }
    }
    let upgraded_effects = Arc::new(Mutex::new(Vec::new()));
    let upgraded_waiting = Arc::new(Gate::default());
    let upgraded_foreground = Arc::new(Gate::default());
    let echo = Arc::new(Echo {
        effects: upgraded_effects.clone(),
        waiting: Some(upgraded_waiting.clone()),
        foreground: Some(upgraded_foreground.clone()),
    });
    let irc = Arc::new(IrcQueue(AtomicBool::new(false)));
    let late =
        Arc::new(LateSteering { source: Mutex::new(None), user_seen: Notify::new(), system_seen: Notify::new() });
    let hooks = Arc::new(IrcHooks(irc.clone(), Some(SteeringInterruptBinding::new(late.clone(), None))));
    let upgraded_config = config(Provider::new(vec![]), echo, hooks);
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    message.content = vec![
        AssistantBlock::ToolCall(call("running", json!({"text":"wait","mode":"wait"}))),
        AssistantBlock::ToolCall(call("foreground", json!({"text":"write","mode":"foreground"}))),
        AssistantBlock::ToolCall(call("pending", json!({"text":"pending","mode":"wait","exclusive":true}))),
    ];
    let sink = Arc::new(Receipts::default());
    let run_sink = sink.clone();
    let executing = tokio::spawn(async move {
        execute_tool_calls(&message, &upgraded_config, &CancellationToken::new(), run_sink.as_ref()).await
    });
    bounded(upgraded_waiting.started.notified()).await;
    bounded(upgraded_foreground.started.notified()).await;
    irc.0.store(true, Ordering::SeqCst);
    bounded(async {
        loop {
            let running_emitted = sink.events.lock().unwrap().iter().any(|event| {
                matches!(ended_model_message(event), Some(Message::ToolResult(result))
                    if result.tool_call_id == "running")
            });
            if running_emitted {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    *late.source.lock().unwrap() = Some(SteeringSource::User);
    bounded(late.user_seen.notified()).await;
    *late.source.lock().unwrap() = Some(SteeringSource::System);
    bounded(late.system_seen.notified()).await;
    upgraded_foreground.release.notify_one();
    let results = bounded(executing).await.unwrap();
    let running = results.iter().find(|result| result.tool_call_id == "running").unwrap();
    assert!(text(&running.content).contains("pending peer interrupt"));
    assert_eq!(running.details.as_ref().unwrap()["execution"], "started");
    let pending = results.iter().find(|result| result.tool_call_id == "pending").unwrap();
    assert!(text(&pending.content).contains("queued user message"));
    assert!(!text(&pending.content).contains("pending peer interrupt"));
    assert!(!text(&pending.content).contains("pending system advisory"));
    assert_eq!(pending.details.as_ref().unwrap()["executed"], false);
    assert!(!upgraded_effects.lock().unwrap().iter().any(|effect| effect.id == "pending"));
    let foreground = results.iter().find(|result| result.tool_call_id == "foreground").unwrap();
    assert_eq!(text(&foreground.content), "foreground genuine failure");
    assert!(foreground.is_error);
    assert!(foreground.details.as_ref().unwrap().get("__interrupted").is_none());

    // A completes first, but its Native post-tool peek waits while B completes.
    // Report and next-request order follow receipts, not callback settlement.
    struct CompletionPeek {
        sink: Arc<Receipts>,
        gate: Arc<Gate>,
        held: AtomicBool,
    }
    #[async_trait]
    impl SteeringQueuePeek for CompletionPeek {
        async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
            let a_completed = self.sink.events.lock().unwrap().iter().any(|event| {
                matches!(ended_model_message(event), Some(Message::ToolResult(result))
                    if result.tool_call_id == "A")
            });
            if a_completed && !self.held.swap(true, Ordering::SeqCst) {
                self.gate.started.notify_one();
                self.gate.release.notified().await;
            }
            Ok(SteeringQueueState::default())
        }
    }
    impl SteeringWake for CompletionPeek {
        fn subscribe(&self, _cancel: CancellationToken) -> SteeringSubscription {
            SteeringSubscription::from_future(std::future::pending())
        }
    }
    struct CompletionHooks(Arc<CompletionPeek>);
    impl LoopHooks for CompletionHooks {
        fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
            Some(false)
        }
        fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
            Some(SteeringInterruptBinding::new(self.0.clone(), Some(self.0.clone())))
        }
    }
    let ordered_effects = Arc::new(Mutex::new(Vec::new()));
    let b_gate = Arc::new(Gate::default());
    let peek_gate = Arc::new(Gate::default());
    let sink = Arc::new(Receipts::default());
    let peek = Arc::new(CompletionPeek { sink: sink.clone(), gate: peek_gate.clone(), held: AtomicBool::new(false) });
    let provider = Provider::new(vec![
        turn(vec![call("A", json!({"text":"first"})), call("B", json!({"text":"second","mode":"foreground"}))]),
        turn(vec![]),
    ]);
    let echo = Arc::new(Echo { effects: ordered_effects, waiting: None, foreground: Some(b_gate.clone()) });
    let agent = Agent::new(config(provider.clone(), echo, Arc::new(CompletionHooks(peek))), vec![]);
    let run_sink = sink.clone();
    let pending =
        tokio::spawn(
            async move { agent.prompt(vec![user("completion order")], CancellationToken::new(), run_sink).await },
        );
    bounded(peek_gate.started.notified()).await;
    bounded(b_gate.started.notified()).await;
    b_gate.release.notify_one();
    bounded(async {
        loop {
            let b_completed = sink.events.lock().unwrap().iter().any(|event| {
                matches!(ended_model_message(event), Some(Message::ToolResult(result))
                    if result.tool_call_id == "B")
            });
            if b_completed {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    peek_gate.release.notify_one();
    let report = bounded(pending).await.unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    let event_order: Vec<_> = sink
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::MessageEnd { .. } => match ended_model_message(event) {
                Some(Message::ToolResult(result)) => Some(result.tool_call_id),
                _ => None,
            },
            _ => None,
        })
        .collect();
    let result_ids = |messages: &[Message]| {
        messages
            .iter()
            .filter_map(|message| match message {
                Message::ToolResult(result) => Some(result.tool_call_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(event_order, ["A", "B"]);
    assert_eq!(result_ids(&report.model_messages().unwrap()), event_order);
    assert_eq!(result_ids(&provider.contexts.lock().unwrap()[1].messages), event_order);
}

struct WakeFixture {
    subscriptions: AtomicUsize,
    cleanups: Arc<AtomicUsize>,
    reject: bool,
}
#[async_trait]
impl SteeringQueuePeek for WakeFixture {
    async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
        assert!(self.subscriptions.load(Ordering::SeqCst) > 0);
        Ok(SteeringQueueState::default())
    }
}
impl SteeringWake for WakeFixture {
    fn subscribe(&self, _cancel: CancellationToken) -> SteeringSubscription {
        self.subscriptions.fetch_add(1, Ordering::SeqCst);
        let cleanup = self.cleanups.clone();
        let reject = self.reject;
        SteeringSubscription::new(
            async move { if reject { Err(ToolHookError("wake rejected".into())) } else { std::future::pending().await } },
            move || {
                cleanup.fetch_add(1, Ordering::SeqCst);
            },
        )
    }
}
struct WakeHooks(Arc<WakeFixture>);
impl LoopHooks for WakeHooks {
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        Some(SteeringInterruptBinding::new(self.0.clone(), Some(self.0.clone())))
    }
}
#[tokio::test]
async fn group_i_wake_is_installed_before_peek_and_uncooperative_wait_is_cleaned_up() {
    for reject in [false, true] {
        let cleanup = Arc::new(AtomicUsize::new(0));
        let wake = Arc::new(WakeFixture { subscriptions: AtomicUsize::new(0), cleanups: cleanup.clone(), reject });
        let effects = Arc::new(Mutex::new(Vec::new()));
        let config = config(Provider::new(vec![]), tool(&effects), Arc::new(WakeHooks(wake.clone())));
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
        message.content = vec![AssistantBlock::ToolCall(call("quick", json!({"text":"done"})))];
        let results = bounded(execute_tool_calls(&message, &config, &CancellationToken::new(), &NullSink)).await;
        assert_eq!(results.len(), 1);
        assert!(!results[0].is_error);
        assert_eq!(wake.subscriptions.load(Ordering::SeqCst), 1);
        assert_eq!(cleanup.load(Ordering::SeqCst), 1);
    }
}

// D-H's source, admitted catalogue and rejection/unavailable fault controls.
// Native hard identities are retained through deferred claim/admission.
// Availability follows the marker, independently of the fixture's name.
struct HardMarkerTool {
    marked: bool,
}
#[async_trait]
impl AgentTool for HardMarkerTool {
    fn definition(&self) -> Tool {
        Tool {
            name: if self.marked { "marker-alias" } else { "computer" }.into(),
            description: "hard-choice admission fixture".into(),
            parameters: json!({"type":"object","properties":{}}),
            native: self.marked.then_some(ara_ai::NativeToolMarker::Computer),
            ..Default::default()
        }
    }
    async fn execute(
        &self,
        _: &str,
        _: JsonObject,
        _: CancellationToken,
        _: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        panic!("admission fixture does not issue a computer call")
    }
}

struct HardHooks {
    finals: FinalHooks,
    source: Mutex<VecDeque<Option<ToolChoice>>>,
    source_reads: AtomicUsize,
    unavailable: AtomicUsize,
    source_fault: Mutex<Option<&'static str>>,
    unavailable_fault: Mutex<Option<&'static str>>,
    rejection_fault: Mutex<Option<&'static str>>,
    active_tools: Mutex<Option<Vec<Arc<dyn AgentTool>>>>,
    drop_tools_after_turn: AtomicBool,
}
impl HardHooks {
    fn new(source: Vec<Option<ToolChoice>>) -> Self {
        Self {
            finals: FinalHooks::new(),
            source: Mutex::new(source.into()),
            source_reads: AtomicUsize::new(0),
            unavailable: AtomicUsize::new(0),
            source_fault: Mutex::new(None),
            unavailable_fault: Mutex::new(None),
            rejection_fault: Mutex::new(None),
            active_tools: Mutex::new(None),
            drop_tools_after_turn: AtomicBool::new(false),
        }
    }
    fn stop_once(&self) {
        let visits = Arc::new(Mutex::new(Vec::new()));
        let first = AtomicBool::new(true);
        let pipeline = self.finals.pipeline.clone();
        self.finals.binding.set_primary(Some(FinalCallback::new("hard-stop", &visits, move |_, _| {
            pipeline.lock().unwrap().push("hard-gate");
            Ok(first.swap(false, Ordering::SeqCst).then(FinalProviderGateStop::default))
        })));
    }
}
#[async_trait]
impl LoopHooks for HardHooks {
    fn final_provider_gate_binding(&self) -> Option<FinalProviderGateBinding> {
        Some(self.finals.binding.clone())
    }
    fn on_final_provider_gate_rejected(&self) -> Result<(), FinalProviderGateError> {
        self.finals.rejected.fetch_add(1, Ordering::SeqCst);
        let fault = self.rejection_fault.lock().unwrap().take();
        match fault {
            Some("error") => Err(FinalProviderGateError("hard rejection failed".into())),
            Some("panic") => panic!("hard rejection panicked"),
            _ => Ok(()),
        }
    }
    fn select_tool_choice_directive(
        &self,
        _active_tools: &[Arc<dyn AgentTool>],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<ToolChoiceSelection, ToolDirectiveError> {
        self.source_reads.fetch_add(1, Ordering::SeqCst);
        self.finals.pipeline.lock().unwrap().push("select");
        let fault = self.source_fault.lock().unwrap().take();
        match fault {
            Some("error") => return Err(ToolDirectiveError("hard source failed".into())),
            Some("panic") => panic!("hard source panicked"),
            _ => {}
        }
        let directive = self.source.lock().unwrap().pop_front().flatten();
        Ok(ToolChoiceSelection { directive, static_fallback: admitted_static_choice.cloned() })
    }
    fn on_deferred_tool_choice_unavailable(&self) -> Result<(), ToolDirectiveError> {
        self.unavailable.fetch_add(1, Ordering::SeqCst);
        let fault = self.unavailable_fault.lock().unwrap().take();
        match fault {
            Some("error") => Err(ToolDirectiveError("deferred unavailable callback failed".into())),
            Some("panic") => panic!("deferred unavailable callback panicked"),
            _ => Ok(()),
        }
    }
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.active_tools.lock().unwrap().as_ref().map(|tools| ExecutionSnapshot {
            tools: tools.clone(),
            system_prompt: vec!["admitted hard catalogue".into()],
            model: None,
            provider_context: self.finals.provider_binding.clone(),
        })
    }
    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        self.finals.provider_binding.clone()
    }
    async fn transform_provider_context(&self, context: Context, model: &Model) -> Context {
        self.finals.transform_provider_context(context, model).await
    }
    async fn turn_end_inputs(
        &self,
        _message: &AssistantMessage,
        results: &[ToolResultMessage],
        _cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        if !results.is_empty() && self.drop_tools_after_turn.swap(false, Ordering::SeqCst) {
            *self.active_tools.lock().unwrap() = Some(Vec::new());
        }
        Vec::new()
    }
}

struct HardLateSink {
    receipts: Arc<Receipts>,
    hold: Arc<Gate>,
    at_input: bool,
}
#[async_trait]
impl AgentEventSink for HardLateSink {
    async fn emit(&self, event: AgentEvent) {
        let at_end = matches!(&event, AgentEvent::AgentEnd { .. });
        let at_input = matches!(&event, AgentEvent::MessageEnd { message }
            if matches!(message.checked_model_snapshot().unwrap(), Some(Message::User(_) | Message::Developer(_))));
        self.receipts.emit(event).await;
        if at_end && !self.at_input || at_input && self.at_input {
            self.hold.started.notify_one();
            self.hold.release.notified().await;
        }
    }
}

struct HardPanicSink;
#[async_trait]
impl AgentEventSink for HardPanicSink {
    async fn emit(&self, event: AgentEvent) {
        // Canonical input events follow the staged G claim, as before.
        if matches!(event, AgentEvent::MessageEnd { message }
            if matches!(message.checked_model_snapshot().unwrap(), Some(Message::User(_) | Message::Developer(_))))
        {
            panic!("host input receipt panicked");
        }
    }
}

async fn hard_prompt(agent: &Arc<Agent>, label: &str) -> RunReport {
    bounded(agent.prompt(vec![user(label)], CancellationToken::new(), Arc::new(Receipts::default()))).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn group_d_h_hard_claim_deferred_active_fallback_clear_reset_and_failure_boundaries() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let echo = ToolChoice::Tool("echo".into());

    // A closed zero-send stop retains the original source choice. Prompt and
    // native continue both claim deferred first without advancing that source.
    for choice in [
        ToolChoice::Auto,
        ToolChoice::None,
        ToolChoice::Any,
        ToolChoice::Required,
        ToolChoice::Function("echo".into()),
        ToolChoice::FunctionNested("echo".into()),
        echo.clone(),
        ToolChoice::Computer,
    ] {
        for continuing in [false, true] {
            let hooks = Arc::new(HardHooks::new(vec![Some(choice.clone()), Some(ToolChoice::Auto)]));
            hooks.stop_once();
            let provider = Provider::new(vec![turn(Vec::new()), turn(Vec::new())]);
            let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
            if choice == ToolChoice::Computer {
                cfg.tools.push(Arc::new(HardMarkerTool { marked: true }));
            }
            let agent = Agent::new(cfg, Vec::new());
            let receipts = Arc::new(Receipts::default());
            let stopped =
                bounded(agent.prompt(vec![user("claim")], CancellationToken::new(), receipts.clone())).await.unwrap();
            assert_eq!(stopped.end, RunEnd::PreModelStop);
            assert!(matches!(stopped.stop_cause, Some(RunStopCause::PreModelStop { .. })));
            assert_eq!(final_turn_counts(&receipts), (0, 0));
            assert_eq!(receipts.inputs.lock().unwrap().len(), 1);
            assert!(provider.contexts.lock().unwrap().is_empty());
            assert!(effects.lock().unwrap().is_empty());
            assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.finals.preparations.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.finals.pipeline.lock().unwrap().as_slice(), &["select", "transform", "hard-gate"]);
            let resumed = if continuing {
                bounded(agent.continue_run(CancellationToken::new(), Arc::new(Receipts::default()))).await.unwrap()
            } else {
                hard_prompt(&agent, "reuse").await
            };
            assert_eq!(resumed.end, RunEnd::Completed);
            assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 1);
            assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(choice.clone())]);
            assert_eq!(hard_prompt(&agent, "fresh after served").await.end, RunEnd::Completed);
            assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
            assert_eq!(
                provider.tool_choices.lock().unwrap().as_slice(),
                &[Some(choice.clone()), Some(ToolChoice::Auto)]
            );
        }
    }
    // A tool named computer without the marker cannot make Computer available.
    let hooks = Arc::new(HardHooks::new(vec![Some(ToolChoice::Computer)]));
    let provider = Provider::new(vec![turn(Vec::new())]);
    let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
    cfg.tools.push(Arc::new(HardMarkerTool { marked: false }));
    cfg.tool_choice = Some(ToolChoice::Required);
    assert_eq!(hard_prompt(&Agent::new(cfg, Vec::new()), "unmarked computer name").await.end, RunEnd::Completed);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[None]);
    assert_eq!(hooks.unavailable.load(Ordering::SeqCst), 0);

    // Only unavailable deferred notifies; then fresh is read in the same turn.
    // Fresh unavailable wins over stale static fallback and never notifies.
    let hooks =
        Arc::new(HardHooks::new(vec![Some(echo.clone()), Some(ToolChoice::Required), Some(echo.clone()), None]));
    hooks.stop_once();
    let provider = Provider::new(vec![turn(Vec::new()), turn(Vec::new()), turn(Vec::new())]);
    let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
    cfg.tool_choice = Some(echo.clone());
    let agent = Agent::new(cfg, Vec::new());
    assert_eq!(hard_prompt(&agent, "retain before catalogue change").await.end, RunEnd::PreModelStop);
    *hooks.active_tools.lock().unwrap() = Some(Vec::new());
    assert_eq!(hard_prompt(&agent, "unavailable deferred then fresh").await.end, RunEnd::Completed);
    assert_eq!(hooks.unavailable.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
    assert_eq!(hard_prompt(&agent, "fresh unavailable").await.end, RunEnd::Completed);
    assert_eq!(hard_prompt(&agent, "static unavailable").await.end, RunEnd::Completed);
    assert_eq!(hooks.unavailable.load(Ordering::SeqCst), 1);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(ToolChoice::Required), None, None]);

    // A static choice is never claimed/deferred. A real executed tool then
    // replaces the catalogue; the following turn filters its static choice.
    let hooks = Arc::new(HardHooks::new(vec![None, None]));
    hooks.stop_once();
    let provider = Provider::new(vec![turn(Vec::new())]);
    let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
    cfg.tool_choice = Some(echo.clone());
    let agent = Agent::new(cfg, Vec::new());
    assert_eq!(hard_prompt(&agent, "static gate stop").await.end, RunEnd::PreModelStop);
    assert_eq!(hard_prompt(&agent, "static source must advance").await.end, RunEnd::Completed);
    assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(echo.clone())]);
    let local_effects = Arc::new(Mutex::new(Vec::new()));
    let hooks = Arc::new(HardHooks::new(vec![None, None]));
    hooks.drop_tools_after_turn.store(true, Ordering::SeqCst);
    let provider =
        Provider::new(vec![turn(vec![call("d-refresh", json!({"text":"real refresh effect"}))]), turn(Vec::new())]);
    let mut cfg = config(provider.clone(), tool(&local_effects), hooks);
    cfg.tool_choice = Some(echo.clone());
    let agent = Agent::new(cfg, Vec::new());
    assert_eq!(hard_prompt(&agent, "static active refresh").await.end, RunEnd::Completed);
    assert_eq!(local_effects.lock().unwrap().len(), 1);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(echo.clone()), None]);
    assert!(provider.contexts.lock().unwrap()[1].tools.as_ref().is_none_or(Vec::is_empty));

    // A choice already served in an earlier turn is not restored when a later
    // closed gate stop has no new dynamic claim.
    let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), None, Some(ToolChoice::Required)]));
    let visits = Arc::new(Mutex::new(Vec::new()));
    let gate_calls = AtomicUsize::new(0);
    hooks.finals.binding.set_primary(Some(FinalCallback::new("later-hard-stop", &visits, move |_, _| {
        Ok((gate_calls.fetch_add(1, Ordering::SeqCst) == 1).then(FinalProviderGateStop::default))
    })));
    let local_effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![turn(vec![call("d-served", json!({"text":"served"}))]), turn(Vec::new())]);
    let agent = Agent::new(config(provider.clone(), tool(&local_effects), hooks.clone()), Vec::new());
    assert_eq!(hard_prompt(&agent, "served then stop").await.end, RunEnd::PreModelStop);
    assert_eq!(local_effects.lock().unwrap().len(), 1);
    assert_eq!(hard_prompt(&agent, "do not restore served").await.end, RunEnd::Completed);
    assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 3);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(echo.clone()), Some(ToolChoice::Required)]);

    // Explicit clear/all/reset drop deferred. Individual queue clears and
    // replace_idle_messages preserve it and keep their existing queue contract.
    for action in ["explicit", "all", "reset", "individual", "replace"] {
        let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), Some(ToolChoice::Required)]));
        hooks.stop_once();
        let provider = Provider::new(vec![turn(Vec::new())]);
        let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
        agent.set_steering_mode(QueueMode::All);
        agent.set_follow_up_mode(QueueMode::All);
        assert_eq!(hard_prompt(&agent, action).await.end, RunEnd::PreModelStop);
        agent.steer(user("queued steering"));
        agent.follow_up(user("queued follow-up"));
        match action {
            "explicit" => agent.clear_deferred_tool_directives(),
            "all" => agent.clear_all_queues(),
            "reset" => agent.reset_idle().unwrap(),
            "individual" => {
                agent.clear_steering_queue();
                agent.clear_follow_up_queue();
            }
            "replace" => agent.replace_idle_messages(vec![user("replacement")]).unwrap(),
            _ => unreachable!(),
        }
        assert_eq!(agent.queued_counts(), if matches!(action, "explicit" | "replace") { (1, 1) } else { (0, 0) });
        if action == "reset" {
            assert!(agent.messages().await.unwrap().is_empty());
        }
        assert_eq!(agent.steering_mode(), QueueMode::All);
        assert_eq!(agent.follow_up_mode(), QueueMode::All);
        agent.clear_steering_queue();
        agent.clear_follow_up_queue();
        assert_eq!(hard_prompt(&agent, "after clear").await.end, RunEnd::Completed);
        let preserved = matches!(action, "individual" | "replace");
        assert_eq!(hooks.source_reads.load(Ordering::SeqCst), if preserved { 1 } else { 2 });
        assert_eq!(
            provider.tool_choices.lock().unwrap().as_slice(),
            &[Some(if preserved { echo.clone() } else { ToolChoice::Required })]
        );
    }

    // Direct loop's admitted static fallback is preserved, including unavailable
    // names; only the Agent facade applies active filtering/deferred policy.
    for stateful in [false, true] {
        for dynamic in [false, true] {
            let missing = ToolChoice::Tool("missing-logical-tool".into());
            let hooks = Arc::new(HardHooks::new(vec![dynamic.then(|| missing.clone())]));
            let provider = Provider::new(vec![turn(Vec::new())]);
            let mut cfg = config(provider.clone(), tool(&effects), hooks);
            cfg.tool_choice = Some(if dynamic { ToolChoice::Required } else { missing.clone() });
            let report = if stateful {
                hard_prompt(&Agent::new(cfg, Vec::new()), "facade authoritative").await
            } else {
                bounded(agent_loop(
                    vec![user("direct fallback")],
                    &mut Vec::new(),
                    &cfg,
                    &CancellationToken::new(),
                    &Receipts::default(),
                ))
                .await
            };
            assert_eq!(report.end, RunEnd::Completed);
            assert_eq!(
                provider.tool_choices.lock().unwrap().as_slice(),
                &[if stateful { None } else { Some(missing) }]
            );
        }
    }
    let hooks = Arc::new(HardHooks::new(vec![None]));
    let provider = Provider::new(vec![turn(Vec::new())]);
    let mut cfg = config(provider.clone(), tool(&effects), hooks);
    cfg.tool_choice = Some(ToolChoice::Required);
    assert_eq!(
        bounded(agent_loop(
            vec![user("direct absent dynamic")],
            &mut Vec::new(),
            &cfg,
            &CancellationToken::new(),
            &Receipts::default()
        ))
        .await
        .end,
        RunEnd::Completed
    );
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(ToolChoice::Required)]);

    // Owned dialect sends remain in charge of provider choice suppression.
    let mut raw_hooks = HardHooks::new(vec![Some(ToolChoice::Required)]);
    let port = Arc::new(FinalOwnedPort {
        pipeline: raw_hooks.finals.pipeline.clone(),
        projects: AtomicUsize::new(0),
        streams: AtomicUsize::new(0),
    });
    raw_hooks.finals.provider_binding =
        Some(ProviderContextBinding { owned_dialect: true, owned_port: Some(port.clone()), ..Default::default() });
    let hooks = Arc::new(raw_hooks);
    let provider = Provider::new(vec![turn(Vec::new())]);
    let agent = Agent::new(config(provider.clone(), tool(&effects), hooks), Vec::new());
    assert_eq!(hard_prompt(&agent, "owned suppression").await.end, RunEnd::Completed);
    assert_eq!(port.streams.load(Ordering::SeqCst), 1);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[None]);

    // Source and rejection errors/panics, gate failure and preparation failure
    // produce balanced diagnostics and do not retain an unserved hard claim.
    for boundary in ["source", "rejection", "gate", "preparation"] {
        for fault in ["error", "panic"] {
            if boundary == "preparation" && fault == "error" {
                continue;
            }
            let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), None]));
            let visits = Arc::new(Mutex::new(Vec::new()));
            match boundary {
                "source" => {
                    *hooks.source_fault.lock().unwrap() = Some(fault);
                    hooks.stop_once();
                }
                "rejection" => {
                    *hooks.rejection_fault.lock().unwrap() = Some(fault);
                    hooks.stop_once();
                }
                "gate" => hooks.finals.binding.set_primary(Some(FinalCallback::new(
                    "hard-gate-failure",
                    &visits,
                    move |_, _| {
                        if fault == "panic" {
                            panic!("hard gate panicked");
                        }
                        Err(FinalProviderGateError("hard gate failed".into()))
                    },
                ))),
                "preparation" => {
                    hooks.finals.panic_prepare.store(true, Ordering::SeqCst);
                    hooks.stop_once();
                }
                _ => unreachable!(),
            }
            let provider = Provider::new(vec![turn(Vec::new())]);
            let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
            let receipts = Arc::new(Receipts::default());
            let failed =
                bounded(agent.prompt(vec![user(boundary)], CancellationToken::new(), receipts.clone())).await.unwrap();
            assert_eq!(failed.end, RunEnd::Error, "{boundary}/{fault}");
            assert_eq!(final_turn_counts(&receipts), (1, 1), "{boundary}/{fault}");
            assert!(provider.contexts.lock().unwrap().is_empty());
            {
                let events = receipts.events.lock().unwrap();
                let opened = events.iter().position(|event| matches!(event, AgentEvent::TurnStart)).unwrap();
                let input = events
                    .iter()
                    .position(
                        |event| matches!(event, AgentEvent::MessageStart { message } if message.json().unwrap()["role"] == "user"),
                    )
                    .unwrap();
                assert!(opened < input);
            }
            hooks.finals.panic_prepare.store(false, Ordering::SeqCst);
            hooks.finals.binding.set_primary(None);
            assert_eq!(hard_prompt(&agent, "after hard failure").await.end, RunEnd::Completed);
            assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
            assert_eq!(
                provider.tool_choices.lock().unwrap().as_slice(),
                &[if boundary == "source" { Some(echo.clone()) } else { None }]
            );
        }
    }

    // Deferred is consumed before the unavailable callback. Callback failure
    // does not consume a fresh source or silently put the old choice back.
    for fault in ["error", "panic"] {
        let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), Some(ToolChoice::Auto)]));
        hooks.stop_once();
        let provider = Provider::new(vec![turn(Vec::new())]);
        let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
        assert_eq!(hard_prompt(&agent, "retain unavailable fault").await.end, RunEnd::PreModelStop);
        *hooks.active_tools.lock().unwrap() = Some(Vec::new());
        *hooks.unavailable_fault.lock().unwrap() = Some(fault);
        assert_eq!(hard_prompt(&agent, "unavailable callback fault").await.end, RunEnd::Error);
        assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 1);
        assert_eq!(hard_prompt(&agent, "fresh after unavailable fault").await.end, RunEnd::Completed);
        assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
        assert_eq!(hooks.unavailable.load(Ordering::SeqCst), 1);
        assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(ToolChoice::Auto)]);
    }

    // Cancellation/deadline during G never stages; after actual input/AgentEnd
    // awaits it clears an already staged claim. Follow-up observes a fresh source.
    for point in ["gate", "input", "end"] {
        for deadline in [false, true] {
            let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), Some(ToolChoice::Required)]));
            let visits = Arc::new(Mutex::new(Vec::new()));
            let hold = Arc::new(Gate::default());
            if point == "gate" {
                hooks.finals.binding.set_primary(Some(FinalCallback::held("hard-cancel", &visits, &hold, |_, _| {
                    Ok(Some(FinalProviderGateStop::default()))
                })));
            } else {
                hooks.stop_once();
            }
            let provider = Provider::new(vec![turn(Vec::new())]);
            let cfg = config(provider.clone(), tool(&effects), hooks.clone());
            let agent = Agent::new(cfg.clone(), Vec::new());
            let mut run_cfg = cfg;
            if deadline {
                run_cfg.deadline = Some(std::time::Instant::now() + Duration::from_millis(200));
            }
            let receipts = Arc::new(Receipts::default());
            let sink: Arc<dyn AgentEventSink> = if point == "gate" {
                receipts.clone()
            } else {
                Arc::new(HardLateSink { receipts: receipts.clone(), hold: hold.clone(), at_input: point == "input" })
            };
            let token = CancellationToken::new();
            let run_token = token.clone();
            let run_agent = agent.clone();
            let run = tokio::spawn(async move {
                bounded(run_agent.prompt_with_config(vec![user("cancelled hard")], run_cfg, run_token, sink))
                    .await
                    .unwrap()
            });
            bounded(hold.started.notified()).await;
            if deadline {
                tokio::time::sleep(Duration::from_millis(230)).await;
            } else {
                token.cancel();
            }
            hold.release.notify_one();
            let stopped = bounded(run).await.unwrap();
            assert_eq!(stopped.end, if deadline { RunEnd::Deadline } else { RunEnd::Aborted }, "{point}/{deadline}");
            assert_eq!(
                stopped.stop_cause,
                Some(if deadline { RunStopCause::Deadline } else { RunStopCause::ExternalAbort })
            );
            assert_eq!(final_turn_counts(&receipts), (0, 0));
            assert!(provider.contexts.lock().unwrap().is_empty());
            assert_eq!(hooks.finals.rejected.load(Ordering::SeqCst), if point == "gate" { 0 } else { 1 });
            hooks.finals.binding.set_primary(None);
            assert_eq!(hard_prompt(&agent, "fresh after cancellation").await.end, RunEnd::Completed);
            assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 2);
            assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(ToolChoice::Required)]);
        }
    }
    for deadline in [false, true] {
        let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone())]));
        let provider = Provider::new(Vec::new());
        let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
        let token = CancellationToken::new();
        if deadline {
            cfg.deadline = Some(std::time::Instant::now());
        } else {
            token.cancel();
        }
        let agent = Agent::new(cfg, Vec::new());
        let report = bounded(agent.prompt(vec![user("pre-cancelled hard")], token, Arc::new(Receipts::default())))
            .await
            .unwrap();
        assert_eq!(report.end, if deadline { RunEnd::Deadline } else { RunEnd::Aborted });
        assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 0);
        assert!(provider.contexts.lock().unwrap().is_empty());
    }

    // Guarded reset: a Busy refusal preserves queued inputs and the in-flight
    // claim, which still survives the later genuine owner stop.
    let hooks = Arc::new(HardHooks::new(vec![Some(echo.clone()), Some(ToolChoice::Required)]));
    let visits = Arc::new(Mutex::new(Vec::new()));
    let hold = Arc::new(Gate::default());
    hooks.finals.binding.set_primary(Some(FinalCallback::held("busy-reset", &visits, &hold, |_, _| {
        Ok(Some(FinalProviderGateStop::default()))
    })));
    let provider = Provider::new(vec![turn(Vec::new())]);
    let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
    let run_agent = agent.clone();
    let run = tokio::spawn(async move { hard_prompt(&run_agent, "busy reset").await });
    bounded(hold.started.notified()).await;
    agent.steer(user("busy steering"));
    agent.follow_up(user("busy follow-up"));
    assert!(matches!(agent.reset_idle(), Err(AgentError::Busy)));
    assert_eq!(agent.queued_counts(), (1, 1));
    hold.release.notify_one();
    assert_eq!(bounded(run).await.unwrap().end, RunEnd::PreModelStop);
    assert_eq!(agent.messages().await.unwrap().len(), 1);
    assert_eq!(agent.queued_counts(), (1, 1));
    agent.clear_steering_queue();
    agent.clear_follow_up_queue();
    hooks.finals.binding.set_primary(None);
    assert_eq!(hard_prompt(&agent, "retained after busy reset").await.end, RunEnd::Completed);
    assert_eq!(hooks.source_reads.load(Ordering::SeqCst), 1);
    assert_eq!(provider.tool_choices.lock().unwrap().as_slice(), &[Some(echo)]);

    // Unknown runnable effects hidden behind a later ordinary message also
    // refuse reset. A panicked Host sink leaves NeedsRecovery, never an idle wipe.
    let mut unknown = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    unknown.stop_reason = StopReason::ToolUse;
    unknown.content.push(AssistantBlock::ToolCall(call("unknown-reset", json!({"text":"unknown"}))));
    let history = vec![Message::Assistant(unknown), user("later ordinary message")];
    let hooks = Arc::new(HardHooks::new(Vec::new()));
    let agent = Agent::new(config(Provider::new(Vec::new()), tool(&effects), hooks), history.clone());
    agent.steer(user("unknown steering"));
    agent.follow_up(user("unknown follow-up"));
    assert!(matches!(agent.reset_idle(), Err(AgentError::CannotContinue(_))));
    assert_eq!(agent.messages().await.unwrap(), history);
    assert_eq!(agent.queued_counts(), (1, 1));
    let hooks = Arc::new(HardHooks::new(vec![Some(ToolChoice::Required)]));
    hooks.stop_once();
    let agent = Agent::new(config(Provider::new(Vec::new()), tool(&effects), hooks), vec![user("persisted")]);
    let failed =
        bounded(agent.prompt(vec![user("host panic")], CancellationToken::new(), Arc::new(HardPanicSink))).await;
    assert!(matches!(failed, Err(AgentError::RunPanicked)));
    let before_reset = agent.messages().await.unwrap();
    agent.steer(user("recovery steering"));
    agent.follow_up(user("recovery follow-up"));
    assert!(matches!(agent.reset_idle(), Err(AgentError::NeedsRecovery)));
    assert_eq!(agent.messages().await.unwrap(), before_reset);
    assert_eq!(agent.queued_counts(), (1, 1));
    assert!(effects.lock().unwrap().is_empty(), "D-H only designated local effect scenarios execute tools");
}

// R-RAW is one coarse contract family. Root runs it together with the existing
// Core/Host/Session families on the integrated snapshot; authored is not passed.
struct RawHooks {
    visits: Arc<Mutex<Vec<&'static str>>>,
    conversions: AtomicUsize,
    replacement_view: bool,
    fail: Option<&'static str>,
}
#[async_trait]
impl LoopHooks for RawHooks {
    async fn before_queued_input_dequeue_with_context(
        &self,
        _: ExecutionSnapshot,
        _: Vec<Message>,
        _: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        panic!("a default raw hook must not opt into typed admission")
    }
    async fn before_model_call_with_context(
        &self,
        _: ExecutionSnapshot,
        _: Vec<Message>,
        _: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        panic!("a default raw hook must not opt into typed admission")
    }
    async fn transform_agent_context(
        &self,
        live: &mut Vec<AgentMessage>,
        _: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, RawContextError> {
        self.visits.lock().unwrap().push("raw-transform");
        match self.fail {
            Some("transform-error") => return Err(RawContextError("raw transform failed".into())),
            Some("transform-panic") => panic!("raw transform panic"),
            _ => {}
        }
        if let Some(message) = live.iter().find(|message| message.json().unwrap()["extension"] == "editable") {
            let mut body = message.json().unwrap();
            body["extension"] = json!("edited-current-body");
            message.replace_body(AgentMessageBody::RawJson(body)).unwrap();
        }
        if self.replacement_view {
            live.retain(|message| message.json().unwrap()["extension"] != "history");
            return Ok(Some(vec![
                AgentMessage::from_raw(
                    json!({"role":"custom", "customType":"request-only",
                "content":"replacement fragment", "display":false, "timestamp":11}),
                    None,
                )
                .unwrap(),
            ]));
        }
        Ok(None)
    }
    async fn convert_agent_messages_to_llm(&self, messages: &[AgentMessage]) -> Result<Vec<Message>, RawContextError> {
        self.visits.lock().unwrap().push("raw-convert");
        self.conversions.fetch_add(1, Ordering::SeqCst);
        match self.fail {
            Some("convert-error") => return Err(RawContextError("raw converter failed".into())),
            Some("convert-panic") => panic!("raw converter panic"),
            _ => {}
        }
        let mut output = Vec::new();
        for raw in messages {
            let body = raw.json().unwrap();
            if let Some(message) = raw.checked_model_snapshot()? {
                output.push(message);
            } else if body["customType"] == "request-only" {
                output.push(Message::Developer(DeveloperMessage {
                    content: UserContent::Text("replacement fragment".into()),
                    timestamp: 11,
                    provider_payload: None,
                    provider_payload_origin: None,
                }));
            } else if body["customType"] == "many" {
                output.push(Message::Developer(DeveloperMessage {
                    content: UserContent::Text("ordered text".into()),
                    timestamp: 12,
                    provider_payload: None,
                    provider_payload_origin: None,
                }));
                output.push(Message::User(UserMessage {
                    content: UserContent::Blocks(vec![UserBlock::Image(ImageContent {
                        data: "AA==".into(),
                        mime_type: "image/png".into(),
                        detail: None,
                        compaction_frame: false,
                    })]),
                    synthetic: None,
                    timestamp: 12,
                    provider_payload: None,
                    provider_payload_origin: None,
                }));
            }
        }
        Ok(output)
    }
    async fn turn_end_raw_messages_with_context(
        &self,
        context: TurnEndContext<'_>,
        _: &CancellationToken,
    ) -> Vec<AgentMessage> {
        assert!(!context.context.messages.is_empty());
        Vec::new()
    }
}

#[derive(Default)]
struct RawSink {
    agent: Mutex<Weak<Agent>>,
    events: Mutex<Vec<AgentEvent>>,
    held_assistant: Mutex<Option<AgentMessage>>,
    original_args: Mutex<Option<Value>>,
}

struct RawTailHooks {
    current: Option<AgentMessage>,
}
#[async_trait]
impl LoopHooks for RawTailHooks {
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        _: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        // This fixture mutates only the explicitly resumed raw tail.
        // The following empty provider Assistant is a separate finalized turn.
        if !message.tool_calls().any(|call| call.id == "raw-tail") {
            return Ok(());
        }
        if let AssistantBlock::Text(text) = &mut message.content[0] {
            text.text = "current rewritten text".into();
        }
        if let Some(current) = &self.current {
            tokio::task::yield_now().await;
            let mut body = current.json().unwrap();
            body["content"].as_array_mut().unwrap().insert(
                1,
                json!({"type":"text", "text":"Host inserted while awaiting", "blockExtension":{"retained":"inserted"}}),
            );
            current.replace_body(AgentMessageBody::RawJson(body)).unwrap();
        }
        Ok(())
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut BeforeToolCallContext<'_>,
        _: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        context.args.insert("text".into(), json!("prepared raw tail"));
        // Canonical revision is explicit; execution-only mutation is covered below.
        Ok(BeforeToolCallResult { args: Some(context.args.clone()), ..Default::default() })
    }
}

struct CurrentModelTailHooks {
    current: AgentMessage,
}
#[async_trait]
impl LoopHooks for CurrentModelTailHooks {
    async fn transform_assistant_message(
        &self,
        _: &mut AssistantMessage,
        _: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        tokio::task::yield_now().await;
        let Some(Message::Assistant(mut current)) = self.current.checked_model_snapshot().unwrap() else {
            panic!("Model tail");
        };
        if let AssistantBlock::Text(text) = &mut current.content[0] {
            text.text = "current Model text B".into();
        }
        self.current.replace_body(AgentMessageBody::Model(Message::Assistant(current))).unwrap();
        Ok(())
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut BeforeToolCallContext<'_>,
        _: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        context.args.insert("text".into(), json!("prepared Model tail"));
        // Canonical revision is explicit; execution-only mutation is covered below.
        Ok(BeforeToolCallResult { args: Some(context.args.clone()), ..Default::default() })
    }
}
#[async_trait]
impl AgentEventSink for RawSink {
    async fn emit(&self, event: AgentEvent) {
        if let AgentEvent::MessageEnd { message } = &event {
            let agent = { self.agent.lock().unwrap().upgrade() };
            if let Some(agent) = agent {
                // This would deadlock under the previous whole-Run transcript
                // lock. State is visible before downstream MessageEnd awaits.
                assert!(agent.raw_messages().await.iter().any(|raw| raw.same_record(message)));
            }
            if let Some(Message::Assistant(assistant)) = message.checked_model_snapshot().unwrap() {
                *self.held_assistant.lock().unwrap() = Some(message.clone());
                if assistant.tool_calls().next().is_some() {
                    let mut wrong = message.json().unwrap();
                    wrong["content"][0]["id"] = json!("changed-owner-id");
                    assert!(message.replace_body(AgentMessageBody::RawJson(wrong)).is_err());
                    let mut legal = message.json().unwrap();
                    *self.original_args.lock().unwrap() = Some(legal["content"][0]["arguments"].clone());
                    legal["content"][0]["arguments"]["text"] = json!("host edited body");
                    legal["extension"] = json!("live assistant extension");
                    message.replace_body(AgentMessageBody::RawJson(legal)).unwrap();
                }
            }
        }
        self.events.lock().unwrap().push(event);
    }
}

// One observation helper inside the existing coarse R owner family. This uses
// the actual canonical AgentMessageCell/Graph body and current member slots;
// no typed/JSON snapshot, model request or production fault switch supplies authority.
async fn observe_graph_canonical_owner_slots() {
    use ara_rpc::{
        WireString,
        value_heap::{
            CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueError,
            ValueHandle, ValueHeap, ValueRuntime,
        },
    };

    fn put(heap: &Arc<ValueHeap>, target: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(target, key.into(), value, &NoRuntime).unwrap();
    }
    fn text(heap: &Arc<ValueHeap>, value: &str) -> ValueHandle {
        heap.string(value.into()).unwrap()
    }
    fn member(heap: &Arc<ValueHeap>, id: WireString, name: WireString, args: &ValueHandle) -> ValueHandle {
        let member = heap.object().unwrap();
        put(heap, &member, "type", text(heap, "toolCall"));
        put(heap, &member, "id", heap.string(id).unwrap());
        put(heap, &member, "name", heap.string(name).unwrap());
        put(heap, &member, "arguments", args.clone());
        member
    }
    fn assistant_root(heap: &Arc<ValueHeap>, members: Vec<ValueHandle>, reason: &str) -> ValueHandle {
        let root = heap.object().unwrap();
        put(heap, &root, "role", text(heap, "assistant"));
        put(heap, &root, "content", heap.array(members).unwrap());
        put(heap, &root, "stopReason", text(heap, reason));
        root
    }
    fn fixture(heap: &Arc<ValueHeap>) -> (ValueHandle, ValueHandle, ValueHandle) {
        let args = heap.object().unwrap();
        let call = member(heap, "call".into(), "echo".into(), &args);
        (assistant_root(heap, vec![call.clone()], "toolUse"), call, args)
    }
    fn is_admission(error: GraphOwnerError, reason: impl FnOnce(&GraphAdmissionError) -> bool) {
        assert!(!error.is_native_exception());
        match error {
            GraphOwnerError::Admission { reason: actual, callback_failure: None } => {
                assert!(reason(&actual), "{actual:?}")
            }
            other => panic!("expected admission-only receipt, got {other:?}"),
        }
    }
    fn accessor(get: Option<u64>, set: Option<u64>) -> PropertyDescriptor {
        PropertyDescriptor::Accessor {
            get: get.map(|value| CallableTarget::Host(CapabilityId(value))),
            set: set.map(|value| CallableTarget::Host(CapabilityId(value))),
            enumerable: true,
            configurable: true,
        }
    }

    let heap = ValueHeap::new().unwrap();
    let args = heap.object().unwrap();
    put(&heap, &args, "self", args.clone());
    let huge = heap.bigint_decimal("900719925474099312345678901").unwrap();
    put(&heap, &args, "big", huge.clone());
    let id = WireString::from_units(vec![0xd800, 65]);
    let name = WireString::from_units(vec![66, 0xdfff]);
    let selected = member(&heap, id.clone(), name.clone(), &args);
    let second = member(&heap, "other".into(), "echo".into(), &huge);
    let extra = heap.object().unwrap();
    put(&heap, &extra, "type", text(&heap, "text"));
    let graph_root = assistant_root(&heap, vec![selected.clone(), extra.clone(), second.clone()], "toolUse");
    let source = Arc::new(json!({"entryId":"graph-source-is-attribution"}));
    let owned = AgentMessage::from_graph(graph_root.clone(), Some(source.clone())).unwrap();
    let shared = owned.clone();
    assert!(shared.same_record(&owned));
    assert!(Arc::ptr_eq(&owned.source().unwrap(), &source));
    let slot = owned.current_arguments_slot(&selected).unwrap();
    assert!(slot.read(&NoRuntime).unwrap().same_identity(&args));
    let big_slot = owned.current_arguments_slot(&second).unwrap();
    assert!(big_slot.read(&NoRuntime).unwrap().same_identity(&huge));
    assert!(owned.json().is_err(), "checked true-tree export does not normalize Graph arguments");
    assert!(owned.checked_model_snapshot().is_err());
    assert!(checked_record_equal(&owned, &shared).unwrap());
    let other_record = AgentMessage::from_graph(graph_root.clone(), Some(source)).unwrap();
    assert!(checked_record_equal(&owned, &other_record).is_err());
    assert_eq!(owned.body_snapshot(), other_record.body_snapshot(), "Graph BodyEq is exact root identity");

    // Replacing the content Array and moving unrelated blocks preserves actual
    // member identity and selected order. Stop/toolUse is one runnable category.
    put(&heap, &graph_root, "content", heap.array(vec![extra.clone(), selected.clone(), second.clone()]).unwrap());
    put(&heap, &graph_root, "stopReason", text(&heap, "stop"));
    assert!(slot.read(&NoRuntime).unwrap().same_identity(&args));
    slot.assign(&huge, &NoRuntime).unwrap();
    assert!(shared.current_arguments_slot(&selected).unwrap().read(&NoRuntime).unwrap().same_identity(&huge));
    for reason in ["length", "error", "aborted"] {
        put(&heap, &graph_root, "stopReason", text(&heap, reason));
        is_admission(slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::BoundaryChanged));
    }
    let (not_runnable_root, not_runnable_call, not_runnable_args) = fixture(&heap);
    put(&heap, &not_runnable_root, "stopReason", text(&heap, "length"));
    let not_runnable_owner = AgentMessage::from_graph(not_runnable_root.clone(), None).unwrap();
    let not_runnable_slot = not_runnable_owner.current_arguments_slot(&not_runnable_call).unwrap();
    for reason in ["error", "aborted"] {
        put(&heap, &not_runnable_root, "stopReason", text(&heap, reason));
        assert!(not_runnable_slot.read(&NoRuntime).unwrap().same_identity(&not_runnable_args));
    }
    put(&heap, &not_runnable_root, "stopReason", text(&heap, "toolUse"));
    is_admission(not_runnable_slot.read(&NoRuntime).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::BoundaryChanged)
    });
    put(&heap, &graph_root, "stopReason", text(&heap, "stop"));
    put(&heap, &graph_root, "content", heap.array(vec![second.clone(), selected.clone()]).unwrap());
    is_admission(slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::BoundaryChanged));
    put(&heap, &graph_root, "content", heap.array(vec![selected.clone(), second.clone()]).unwrap());
    put(&heap, &selected, "id", text(&heap, "narrowed id"));
    is_admission(slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::BoundaryChanged));
    put(&heap, &selected, "id", heap.string(id.clone()).unwrap());
    assert!(slot.read(&NoRuntime).unwrap().same_identity(&huge));

    // A forged repeated ID/name never authorizes the detached original handle.
    let duplicate = member(&heap, id, name, &args);
    is_admission(owned.current_arguments_slot(&duplicate).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::MissingMember)
    });
    put(&heap, &graph_root, "content", heap.array(vec![duplicate, second.clone()]).unwrap());
    is_admission(slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::MissingMember));
    put(&heap, &graph_root, "content", heap.array(vec![selected.clone(), second]).unwrap());
    let replacement = assistant_root(&heap, Vec::new(), "stop");
    put(&heap, &replacement, "content", heap.get(&graph_root, &"content".into(), &NoRuntime).unwrap());
    assert_ne!(owned.body_snapshot(), AgentMessageBody::Graph(replacement.clone()));
    owned.replace_body(AgentMessageBody::Graph(replacement.clone())).unwrap();
    is_admission(slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::StaleRoot));
    assert!(owned.current_arguments_slot(&selected).unwrap().read(&NoRuntime).unwrap().same_identity(&huge));

    let weak_slot = {
        let (root, call, _) = fixture(&heap);
        let transient = AgentMessage::from_graph(root, None).unwrap();
        transient.current_arguments_slot(&call).unwrap()
    };
    is_admission(weak_slot.read(&NoRuntime).unwrap_err(), |error| matches!(error, GraphAdmissionError::StaleOwner));
    let foreign_heap = ValueHeap::new().unwrap();
    is_admission(owned.current_arguments_slot(&foreign_heap.object().unwrap()).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::ForeignHeap)
    });
    let current = owned.current_arguments_slot(&selected).unwrap();
    is_admission(current.assign(&foreign_heap.object().unwrap(), &NoRuntime).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::ForeignHeap)
    });

    // Required finite controls are exact own Text, without missing/undefined/
    // null/bool/number defaults. Payloads remain outside the admission profile.
    for field in ["role", "id", "name", "stopReason"] {
        for value in [
            None,
            Some(heap.undefined().unwrap()),
            Some(heap.null().unwrap()),
            Some(heap.boolean(true).unwrap()),
            Some(heap.number(1.0).unwrap()),
            Some(heap.object().unwrap()),
        ] {
            let (root, call, _) = fixture(&heap);
            let target = if matches!(field, "id" | "name") { &call } else { &root };
            match value {
                Some(value) => put(&heap, target, field, value),
                None => assert!(heap.delete_own(target, &field.into(), &NoRuntime).unwrap()),
            }
            is_admission(AgentMessage::from_graph(root, None).unwrap_err(), |error| {
                matches!(error, GraphAdmissionError::UnsupportedStructure(_))
            });
        }
    }
    for bad_role in ["", "assistant"] {
        let root = heap.object().unwrap();
        put(&heap, &root, "role", text(&heap, bad_role));
        is_admission(AgentMessage::from_graph(root, None).unwrap_err(), |error| {
            matches!(error, GraphAdmissionError::UnsupportedStructure(_))
        });
    }
    for reason in ["unknown", "ToolUse"] {
        let (root, _, _) = fixture(&heap);
        put(&heap, &root, "stopReason", text(&heap, reason));
        is_admission(AgentMessage::from_graph(root, None).unwrap_err(), |error| {
            matches!(error, GraphAdmissionError::UnsupportedStructure(_))
        });
    }
    for field in ["toolCallId", "toolName"] {
        let root = heap.object().unwrap();
        put(&heap, &root, "role", text(&heap, "toolResult"));
        put(&heap, &root, "toolCallId", text(&heap, "call"));
        put(&heap, &root, "toolName", text(&heap, "echo"));
        heap.define_own(&root, "content".into(), accessor(Some(999), None), &NoRuntime).unwrap();
        heap.define_own(&root, "stopReason".into(), accessor(Some(999), None), &NoRuntime).unwrap();
        let result_owner = AgentMessage::from_graph(root.clone(), None).unwrap();
        assert!(result_owner.replace_body(AgentMessageBody::Graph(root.clone())).is_ok());
        put(&heap, &root, field, heap.undefined().unwrap());
        is_admission(AgentMessage::from_graph(root, None).unwrap_err(), |error| {
            matches!(error, GraphAdmissionError::UnsupportedStructure(_))
        });
    }
    for role in ["user", "developer", "application-notice"] {
        let root = heap.object().unwrap();
        put(&heap, &root, "role", text(&heap, role));
        heap.define_own(&root, "content".into(), accessor(Some(999), None), &NoRuntime).unwrap();
        assert!(AgentMessage::from_graph(root, None).is_ok());
    }
    let no_calls = assistant_root(&heap, vec![extra], "unknown");
    for stop in [None, Some(heap.undefined().unwrap()), Some(heap.null().unwrap())] {
        match stop {
            Some(value) => put(&heap, &no_calls, "stopReason", value),
            None => assert!(heap.delete_own(&no_calls, &"stopReason".into(), &NoRuntime).unwrap()),
        }
        assert!(AgentMessage::from_graph(no_calls.clone(), None).is_ok());
    }
    heap.define_own(&no_calls, "stopReason".into(), accessor(Some(999), None), &NoRuntime).unwrap();
    assert!(AgentMessage::from_graph(no_calls, None).is_ok(), "no calls means no stopReason observation");
    let (sparse, _, _) = fixture(&heap);
    put(&heap, &sparse, "content", heap.array_with_length(1).unwrap());
    is_admission(AgentMessage::from_graph(sparse, None).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::UnsupportedStructure(_))
    });
    let (structural_accessor, _, _) = fixture(&heap);
    heap.define_own(&structural_accessor, "role".into(), accessor(Some(999), None), &NoRuntime).unwrap();
    is_admission(AgentMessage::from_graph(structural_accessor, None).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::UnsupportedStructure(_))
    });

    // Ordinary readonly assignment fails after admission: retain actual native
    // TypeError classification, unchanged arguments and subsequent current guard.
    // Foreign assigned values remain admission failures before operation planning.
    let (readonly_root, readonly_member, readonly_args) = fixture(&heap);
    let readonly_owner = AgentMessage::from_graph(readonly_root, None).unwrap();
    let readonly_slot = readonly_owner.current_arguments_slot(&readonly_member).unwrap();
    heap.define_own(
        &readonly_member,
        "arguments".into(),
        PropertyDescriptor::Data {
            value: readonly_args.clone(),
            writable: false,
            enumerable: true,
            configurable: true,
        },
        &NoRuntime,
    )
    .unwrap();
    let error = readonly_slot.assign(&huge, &NoRuntime).unwrap_err();
    assert!(error.is_native_exception());
    assert!(matches!(error, GraphOwnerError::Native(ValueError::TypeError(_))));
    assert!(readonly_slot.read(&NoRuntime).unwrap().same_identity(&readonly_args));
    is_admission(readonly_slot.assign(&foreign_heap.object().unwrap(), &NoRuntime).unwrap_err(), |error| {
        matches!(error, GraphAdmissionError::ForeignHeap)
    });
    assert!(readonly_slot.read(&NoRuntime).unwrap().same_identity(&readonly_args));

    // The runtime reenters the same owner and heap only after both guards leave.
    // One callback runs once; readmission never replays Get/Set or writes late.
    #[derive(Clone, Copy)]
    enum Mode {
        Return,
        Throw,
        DetachThrow,
        ReplaceThrow,
        BoundaryReturn,
        SetThrow,
        ForeignReturn,
        ForeignThrow,
        Missing,
    }
    struct SlotRuntime {
        heap: Arc<ValueHeap>,
        owner: AgentMessage,
        root: ValueHandle,
        member: ValueHandle,
        reply: ValueHandle,
        thrown: ValueHandle,
        mode: Mode,
        calls: AtomicUsize,
    }
    impl ValueRuntime for SlotRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { receiver, arguments, .. } = request else {
                return Err(ValueError::UnsupportedOperation("slot fixture accepts Call only"));
            };
            assert!(receiver.same_identity(&self.member));
            assert_eq!(self.calls.fetch_add(1, Ordering::SeqCst), 0);
            // Record/heap reentry is real: no test-owned shortcut grants admission.
            let _ = self.owner.body_snapshot();
            self.heap.define_own_data(&self.root, "touched".into(), self.heap.boolean(true)?, &NoRuntime)?;
            match self.mode {
                Mode::DetachThrow => {
                    self.heap.define_own_data(
                        &self.root,
                        "content".into(),
                        self.heap.array(Vec::new())?,
                        &NoRuntime,
                    )?;
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                Mode::ReplaceThrow => {
                    let replacement = assistant_root(&self.heap, vec![self.member.clone()], "stop");
                    self.owner
                        .replace_body(AgentMessageBody::Graph(replacement))
                        .map_err(|_| ValueError::WrongRuntimeReply("equivalent Graph owner replacement"))?;
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                Mode::BoundaryReturn => {
                    self.heap.define_own_data(
                        &self.root,
                        "stopReason".into(),
                        text(&self.heap, "error"),
                        &NoRuntime,
                    )?;
                    Ok(RuntimeReply::Value(self.reply.clone()))
                }
                Mode::SetThrow => {
                    assert_eq!(arguments.len(), 1);
                    self.heap.define_own_data(receiver, "setterEffect".into(), arguments[0].clone(), &NoRuntime)?;
                    Err(ValueError::Thrown(self.thrown.clone()))
                }
                Mode::Throw | Mode::ForeignThrow => Err(ValueError::Thrown(self.thrown.clone())),
                Mode::Missing => Err(ValueError::MissingCapability(CapabilityId(999))),
                Mode::Return | Mode::ForeignReturn => Ok(RuntimeReply::Value(self.reply.clone())),
            }
        }
    }
    for mode in [
        Mode::Return,
        Mode::Throw,
        Mode::DetachThrow,
        Mode::ReplaceThrow,
        Mode::BoundaryReturn,
        Mode::SetThrow,
        Mode::ForeignReturn,
        Mode::ForeignThrow,
        Mode::Missing,
    ] {
        let (root, call, reply) = fixture(&heap);
        let owner = AgentMessage::from_graph(root.clone(), None).unwrap();
        let slot = owner.current_arguments_slot(&call).unwrap();
        heap.define_own(&call, "arguments".into(), accessor(Some(1), Some(2)), &NoRuntime).unwrap();
        slot.recheck_current().unwrap(); // No getter resolution is required.
        let local_thrown = heap.object().unwrap();
        let runtime = SlotRuntime {
            heap: heap.clone(),
            owner: owner.clone(),
            root: root.clone(),
            member: call.clone(),
            reply: if matches!(mode, Mode::ForeignReturn) { foreign_heap.object().unwrap() } else { reply.clone() },
            thrown: if matches!(mode, Mode::ForeignThrow) {
                foreign_heap.object().unwrap()
            } else {
                local_thrown.clone()
            },
            mode,
            calls: AtomicUsize::new(0),
        };
        let outcome = if matches!(mode, Mode::SetThrow) {
            slot.assign(&reply, &runtime).map(|()| reply.clone())
        } else {
            slot.read(&runtime)
        };
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1);
        if matches!(
            mode,
            Mode::Return | Mode::Throw | Mode::SetThrow | Mode::ForeignReturn | Mode::ForeignThrow | Mode::Missing
        ) {
            slot.recheck_current().unwrap();
        } else {
            assert!(slot.recheck_current().is_err(), "stale controls/member/root remain rejected without a second Get");
        }
        assert_eq!(runtime.calls.load(Ordering::SeqCst), 1, "checker re-admission never replays the getter");
        assert!(matches!(
            heap.snapshot(&heap.get(&root, &"touched".into(), &NoRuntime).unwrap()).unwrap(),
            ara_rpc::value_heap::ValueSnapshot::Boolean(true)
        ));
        match mode {
            Mode::Return => assert!(outcome.unwrap().same_identity(&reply)),
            Mode::Throw | Mode::SetThrow => {
                let error = outcome.unwrap_err();
                assert!(error.is_native_exception());
                assert!(
                    matches!(error, GraphOwnerError::Native(ValueError::Thrown(value)) if value.same_identity(&local_thrown))
                );
                if matches!(mode, Mode::SetThrow) {
                    assert!(heap.get(&call, &"setterEffect".into(), &NoRuntime).unwrap().same_identity(&reply));
                    assert!(matches!(
                        heap.own_descriptor(&call, &"arguments".into(), &NoRuntime).unwrap(),
                        Some(PropertyDescriptor::Accessor { .. })
                    ));
                }
            }
            Mode::DetachThrow | Mode::ReplaceThrow => {
                let error = outcome.unwrap_err();
                assert!(!error.is_native_exception());
                let GraphOwnerError::Admission { reason, callback_failure: Some(ValueError::Thrown(value)) } = error
                else {
                    panic!("compound admission must retain actual Thrown receipt");
                };
                assert!(value.same_identity(&local_thrown));
                assert!(matches!(
                    (mode, reason),
                    (Mode::DetachThrow, GraphAdmissionError::MissingMember)
                        | (Mode::ReplaceThrow, GraphAdmissionError::StaleRoot)
                ));
            }
            Mode::BoundaryReturn => {
                is_admission(outcome.unwrap_err(), |error| matches!(error, GraphAdmissionError::BoundaryChanged))
            }
            Mode::ForeignReturn | Mode::ForeignThrow => {
                let error = outcome.unwrap_err();
                assert!(!error.is_native_exception());
                assert!(matches!(error, GraphOwnerError::Native(ValueError::ForeignHeap)));
            }
            Mode::Missing => {
                let error = outcome.unwrap_err();
                assert!(!error.is_native_exception());
                assert!(matches!(error, GraphOwnerError::Native(ValueError::MissingCapability(_))));
            }
        }
    }

    // The legacy tail projection still refuses Graph. Actual continuation
    // reports missing current Session values without using that bridge.
    let (tail_root, _, _) = fixture(&heap);
    let graph_tail = AgentMessage::from_graph(tail_root, None).unwrap();
    assert!(unpaired_raw_tool_call_tail(std::slice::from_ref(&graph_tail)).unwrap_err().0.contains("Graph tail"));
    let provider = Provider::new(Vec::new());
    let effects = Arc::new(Mutex::new(Vec::new()));
    let execution = config(provider.clone(), tool(&effects), Arc::new(NoHooks));
    let sink = RecordingSink::default();
    let mut context = vec![graph_tail.clone()];
    let outcome = bounded(agent_loop_continue_raw(
        &mut context,
        &execution,
        &CancellationToken::new(),
        &sink,
        UnpairedTail::Execute,
    ))
    .await;
    let report = outcome.expect("actual continuation reports its admitted failure");
    assert_eq!(report.end, RunEnd::Error);
    assert!(report.messages.is_empty());
    let events = sink.events.lock().await;
    assert_eq!(events.len(), 4);
    assert!(matches!(&events[0], AgentEvent::AgentStart));
    assert!(matches!(&events[1], AgentEvent::TurnStart));
    assert!(matches!(&events[2], AgentEvent::ActualTurnEnd { message, tool_results, preparation_error: Some(error) }
        if message.same_record(&graph_tail) && tool_results.is_empty()
            && matches!(error.as_ref(), ActualProviderError::Unsupported("known-never-executed actual tail has no original Session values"))));
    assert!(matches!(&events[3], AgentEvent::ActualAgentEnd { messages, failure }
        if messages.is_empty()
            && matches!(failure.as_ref(), ActualProviderError::Unsupported("known-never-executed actual tail has no original Session values"))));
    assert!(provider.contexts.lock().unwrap().is_empty());
    assert!(effects.lock().unwrap().is_empty());
    assert!(context[0].same_record(&graph_tail));
}

#[tokio::test(flavor = "multi_thread")]
async fn group_r_raw_canonical_identity_preparation_state_boundary_and_faults() {
    observe_graph_canonical_owner_slots().await;
    actual_stream::observe_existing_loop_actual_stream_to_owner().await;
    let effects = Arc::new(Mutex::new(Vec::new()));
    let notification = AgentMessage::from_raw(
        json!({"role":"application-notice", "content":{"opaque":true}, "timestamp":3}),
        Some(Arc::new(json!({"entryId":"notification-source"}))),
    )
    .unwrap();
    let provider = Provider::new(vec![turn(Vec::new())]);
    let agent = Agent::new_raw(config(provider.clone(), tool(&effects), Arc::new(NoHooks)), Vec::new()).unwrap();
    let sink = Arc::new(RawSink::default());
    *sink.agent.lock().unwrap() = Arc::downgrade(&agent);
    let report =
        bounded(agent.prompt_raw(vec![notification.clone()], CancellationToken::new(), sink.clone())).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert!(
        provider.contexts.lock().unwrap()[0].messages.is_empty(),
        "default notification converts to zero fragments"
    );
    assert!(agent.raw_messages().await[0].same_record(&notification));
    assert!(report.messages[0].same_record(&notification));
    {
        let events = sink.events.lock().unwrap();
        let input_pair: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message }
                    if message.same_record(&notification) =>
                {
                    Some(message)
                }
                _ => None,
            })
            .collect();
        assert_eq!(input_pair.len(), 2);
        assert!(!events.last().unwrap().full().unwrap().to_string().contains("notification-source"));
    }

    // A transform changes its Run array and current body; returned view never
    // replaces Agent state, and convert runs once only on that returned view.
    let history = AgentMessage::from_raw(
        json!({"role":"user","content":"retained history","timestamp":1,"extension":"history"}),
        None,
    )
    .unwrap();
    let current =
        AgentMessage::from_raw(json!({"role":"user","content":"input","timestamp":2,"extension":"editable"}), None)
            .unwrap();
    let visits = Arc::new(Mutex::new(Vec::new()));
    let hooks = Arc::new(RawHooks {
        visits: visits.clone(),
        conversions: AtomicUsize::new(0),
        replacement_view: true,
        fail: None,
    });
    let provider = Provider::new(vec![turn(Vec::new())]);
    let agent = Agent::new_raw(config(provider.clone(), tool(&effects), hooks.clone()), vec![history.clone()]).unwrap();
    let sink = Arc::new(RawSink::default());
    *sink.agent.lock().unwrap() = Arc::downgrade(&agent);
    let report = bounded(agent.prompt_raw(vec![current.clone()], CancellationToken::new(), sink)).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(hooks.conversions.load(Ordering::SeqCst), 1);
    assert_eq!(*visits.lock().unwrap(), ["raw-transform", "raw-convert"]);
    assert_eq!(agent.raw_messages().await.len(), 3);
    assert!(agent.raw_messages().await[0].same_record(&history));
    assert_eq!(current.json().unwrap()["extension"], "edited-current-body");
    assert_eq!(provider.contexts.lock().unwrap()[0].messages[0].role(), "developer");

    // Raw transform/converter errors and panics retain consumed raw receipts.
    for failure in ["transform-error", "transform-panic", "convert-error", "convert-panic"] {
        let hooks = Arc::new(RawHooks {
            visits: Arc::new(Mutex::new(Vec::new())),
            conversions: AtomicUsize::new(0),
            replacement_view: false,
            fail: Some(failure),
        });
        let provider = Provider::new(Vec::new());
        let agent = Agent::new_raw(config(provider.clone(), tool(&effects), hooks), Vec::new()).unwrap();
        let report = bounded(agent.prompt_raw(
            vec![notification.clone()],
            CancellationToken::new(),
            Arc::new(RawSink::default()),
        ))
        .await
        .unwrap();
        assert_eq!(report.end, RunEnd::Error);
        assert!(provider.contexts.lock().unwrap().is_empty());
        assert!(report.messages[0].same_record(&notification));
        assert!(agent.raw_messages().await[0].same_record(&notification));
    }

    // Events expose mutable raw body, but real tool dispatch keeps the private
    // finalized typed accumulator and compact owned boundary.
    let provider =
        Provider::new(vec![turn(vec![call("raw-owned-call", json!({"text":"provider arguments"}))]), turn(Vec::new())]);
    let agent = Agent::new_raw(config(provider, tool(&effects), Arc::new(NoHooks)), Vec::new()).unwrap();
    let sink = Arc::new(RawSink::default());
    *sink.agent.lock().unwrap() = Arc::downgrade(&agent);
    let report =
        bounded(agent.prompt(vec![user("execute owned call")], CancellationToken::new(), sink.clone())).await.unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(effects.lock().unwrap().last().unwrap().args["text"], "provider arguments");
    let owned = agent
        .raw_messages()
        .await
        .into_iter()
        .find(|raw| raw.json().unwrap()["extension"] == "live assistant extension")
        .unwrap();
    assert_eq!(owned.json().unwrap()["content"][0]["arguments"]["text"], "host edited body");
    let mut wrong = owned.json().unwrap();
    wrong["role"] = json!("application-notice");
    assert!(owned.replace_body(AgentMessageBody::RawJson(wrong)).is_err());
    let raw_result = report.messages.iter().find(|raw| raw.json().unwrap()["role"] == "toolResult").unwrap();
    let mut wrong = raw_result.json().unwrap();
    wrong["toolCallId"] = json!("new-call");
    assert!(raw_result.replace_body(AgentMessageBody::RawJson(wrong)).is_err());

    // The explicit known-never-executed continuation retains a raw record's
    // extensions while the typed hooks edit its text/arguments. The source is
    // attribution, not a saved body to restore over the current transcript.
    let mut raw_tail_assistant = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    raw_tail_assistant.stop_reason = StopReason::ToolUse;
    raw_tail_assistant.content.push(AssistantBlock::ToolCall(call("raw-tail", json!({"text":"original arguments"}))));
    let mut body = serde_json::to_value(Message::Assistant(raw_tail_assistant)).unwrap();
    body["content"]
        .as_array_mut()
        .unwrap()
        .insert(0, json!({"type":"text", "text":"original text", "blockExtension":{"retained":"text"}}));
    body["recordExtension"] = json!({"retained":"record"});
    body["content"][1]["blockExtension"] = json!({"retained":"tool"});
    let source = Arc::new(json!({"entryId":"raw-tail-source", "original":body}));
    let raw_tail = AgentMessage::from_raw(body, Some(source.clone())).unwrap();
    let cfg = config(Provider::new(vec![turn(Vec::new())]), tool(&effects), Arc::new(RawTailHooks { current: None }));
    let owner = Agent::new_raw(cfg, vec![raw_tail.clone()]).unwrap();
    let mut context = owner.raw_messages().await;
    let cfg = config(
        Provider::new(vec![turn(Vec::new())]),
        tool(&effects),
        Arc::new(RawTailHooks { current: Some(raw_tail.clone()) }),
    );
    let continued = bounded(agent_loop_continue_raw(
        &mut context,
        &cfg,
        &CancellationToken::new(),
        &Receipts::default(),
        UnpairedTail::Execute,
    ))
    .await
    .unwrap();
    assert_eq!(continued.end, RunEnd::Completed);
    assert!(context[0].same_record(&raw_tail));
    assert!(Arc::ptr_eq(&raw_tail.source().unwrap(), &source));
    let body = raw_tail.json().unwrap();
    assert_eq!(body["recordExtension"]["retained"], "record");
    assert_eq!(body["content"][0]["blockExtension"]["retained"], "text");
    assert_eq!(body["content"][1]["blockExtension"]["retained"], "inserted");
    assert_eq!(body["content"][1]["text"], "Host inserted while awaiting");
    assert_eq!(body["content"][2]["blockExtension"]["retained"], "tool");
    assert_eq!(body["content"][0]["text"], "current rewritten text");
    assert_eq!(body["content"][2]["arguments"]["text"], "prepared raw tail");
    assert_eq!(effects.lock().unwrap().last().unwrap().args["text"], "prepared raw tail");

    // Model records also merge hook edits into CURRENT body after the await;
    // they must not reinstall the earlier typed snapshot over Host text B.
    let mut model_tail = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
    model_tail.stop_reason = StopReason::ToolUse;
    model_tail.content.push(AssistantBlock::ToolCall(call("model-tail", json!({"text":"original Model arguments"}))));
    model_tail.content.insert(0, AssistantBlock::text("original Model text A"));
    let model_tail = AgentMessage::from_model_with_source(Message::Assistant(model_tail), Some(source.clone()));
    let cfg = config(Provider::new(vec![turn(Vec::new())]), tool(&effects), Arc::new(NoHooks));
    let owner = Agent::new_raw(cfg, vec![model_tail.clone()]).unwrap();
    let mut context = owner.raw_messages().await;
    let cfg = config(
        Provider::new(vec![turn(Vec::new())]),
        tool(&effects),
        Arc::new(CurrentModelTailHooks { current: model_tail.clone() }),
    );
    let continued = bounded(agent_loop_continue_raw(
        &mut context,
        &cfg,
        &CancellationToken::new(),
        &Receipts::default(),
        UnpairedTail::Execute,
    ))
    .await
    .unwrap();
    assert_eq!(continued.end, RunEnd::Completed);
    assert!(context[0].same_record(&model_tail));
    assert!(matches!(model_tail.body_snapshot(), AgentMessageBody::Model(_)));
    assert!(Arc::ptr_eq(&model_tail.source().unwrap(), &source));
    assert_eq!(model_tail.json().unwrap()["content"][0]["text"], "current Model text B");
    assert_eq!(model_tail.json().unwrap()["content"][1]["arguments"]["text"], "prepared Model tail");
    assert_eq!(effects.lock().unwrap().last().unwrap().args["text"], "prepared Model tail");

    // Native JSON-schema validation keeps the original argument reference on
    // the fast path, but optional-null normalization makes a copy. Mutation
    // alone updates the canonical call only in the first case. Rust must
    // reproduce this distinction; do not substitute value equality for origin.
    struct OptionalNullEcho(Arc<Echo>);
    #[async_trait]
    impl AgentTool for OptionalNullEcho {
        fn definition(&self) -> Tool {
            let mut definition = self.0.definition();
            definition.parameters["properties"]["optional"] = json!({"type":"string"});
            definition
        }
        async fn execute(
            &self,
            id: &str,
            args: JsonObject,
            cancel: CancellationToken,
            update: UpdateFn,
        ) -> Result<ToolOutput, ToolError> {
            self.0.execute(id, args, cancel, update).await
        }
        async fn execute_with_context(
            &self,
            id: &str,
            args: JsonObject,
            cancel: CancellationToken,
            update: UpdateFn,
            context: ToolExecutionContext,
        ) -> Result<ToolOutput, ToolError> {
            self.0.execute_with_context(id, args, cancel, update, context).await
        }
    }
    struct MutationOnlyArgs {
        mode: &'static str,
    }
    #[async_trait]
    impl LoopHooks for MutationOnlyArgs {
        fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
            matches!(self.mode, "trace-absent" | "trace-present").then(|| ProviderContextBinding {
                build_options: BuildOptions {
                    intent_tracing: true,
                    suppress_intent_schema: true,
                    ..Default::default()
                },
                ..Default::default()
            })
        }
        async fn before_tool_dispatch(
            &self,
            context: &mut BeforeToolCallContext<'_>,
            cancel: &CancellationToken,
        ) -> Result<BeforeToolCallResult, ToolHookError> {
            assert_eq!(context.args["text"], "original mutation-only arguments");
            assert!(!context.args.contains_key("optional"));
            assert!(!context.args.contains_key("i"));
            context.args.insert("text".into(), json!("execution mutation-only arguments"));
            match self.mode {
                "block" => Ok(BeforeToolCallResult {
                    block: true,
                    reason: Some("mutation-only blocked".into()),
                    ..Default::default()
                }),
                "throw" => Err(ToolHookError("mutation-only hook error".into())),
                "panic" => panic!("mutation-only hook panic"),
                "cancel" => {
                    cancel.cancel();
                    Ok(BeforeToolCallResult::default())
                }
                _ => Ok(BeforeToolCallResult::default()),
            }
        }
    }
    for (normalized_copy, mode) in [
        (false, "normal"),
        (true, "normal"),
        (false, "block"),
        (true, "block"),
        (false, "throw"),
        (true, "throw"),
        (false, "panic"),
        (true, "panic"),
        (false, "cancel"),
        (true, "cancel"),
        (false, "trace-absent"),
        (true, "trace-absent"),
        (false, "trace-present"),
        (true, "trace-present"),
    ] {
        let mut original_args = json!({"text":"original mutation-only arguments"});
        if normalized_copy {
            original_args["optional"] = Value::Null;
        }
        if mode == "trace-present" {
            original_args["i"] = json!("record a Native intent");
        }
        let tracing = matches!(mode, "trace-absent" | "trace-present");
        let executes = matches!(mode, "normal" | "trace-absent" | "trace-present");
        let effect_count = effects.lock().unwrap().len();
        let mut assistant = AssistantMessage::empty("openai-completions", "fixture", "core-hooks");
        assistant.stop_reason = StopReason::ToolUse;
        assistant.content.push(AssistantBlock::ToolCall(call("mutation-only-tail", original_args.clone())));
        let mut body = serde_json::to_value(Message::Assistant(assistant)).unwrap();
        body["recordExtension"] = json!({"retained":"mutation-only-record"});
        body["content"][0]["blockExtension"] = json!({"retained":"mutation-only-tool"});
        let mutation_source = Arc::new(json!({"entryId":"mutation-only-source", "original":body}));
        let tail = AgentMessage::from_raw(body, Some(mutation_source.clone())).unwrap();
        let mut context = vec![tail.clone()];
        let cfg = config(
            Provider::new(vec![turn(Vec::new())]),
            Arc::new(OptionalNullEcho(tool(&effects))),
            Arc::new(MutationOnlyArgs { mode }),
        );
        let continued = bounded(agent_loop_continue_raw(
            &mut context,
            &cfg,
            &CancellationToken::new(),
            &Receipts::default(),
            UnpairedTail::Execute,
        ))
        .await
        .unwrap();
        assert_eq!(
            continued.end,
            if mode == "cancel" { RunEnd::Aborted } else { RunEnd::Completed },
            "{normalized_copy}/{mode}"
        );
        assert!(context[0].same_record(&tail));
        assert!(Arc::ptr_eq(&tail.source().unwrap(), &mutation_source));
        assert_eq!(tail.json().unwrap()["recordExtension"]["retained"], "mutation-only-record");
        assert_eq!(tail.json().unwrap()["content"][0]["blockExtension"]["retained"], "mutation-only-tool");
        let mut expected_canonical = original_args.clone();
        if !normalized_copy && !tracing {
            expected_canonical["text"] = json!("execution mutation-only arguments");
        }
        assert_eq!(
            tail.json().unwrap()["content"][0]["arguments"],
            expected_canonical,
            "normalized_copy={normalized_copy}"
        );
        assert_eq!(mutation_source["original"]["content"][0]["arguments"], original_args);
        assert_eq!(effects.lock().unwrap().len() - effect_count, usize::from(executes), "{normalized_copy}/{mode}");
        let receipt = result(&continued, "mutation-only-tail");
        assert_eq!(receipt.is_error, !executes, "{normalized_copy}/{mode}");
        if executes {
            assert_eq!(effects.lock().unwrap().last().unwrap().args["text"], "execution mutation-only arguments");
            assert!(!effects.lock().unwrap().last().unwrap().args.contains_key("optional"));
            assert!(!effects.lock().unwrap().last().unwrap().args.contains_key("i"));
        } else {
            assert!(
                text(&receipt.content).contains(match mode {
                    "block" => "mutation-only blocked",
                    "throw" => "mutation-only hook error",
                    "panic" => "mutation-only hook panic",
                    "cancel" => "aborted",
                    _ => unreachable!(),
                }),
                "{normalized_copy}/{mode}"
            );
        }
    }

    // Native thrown G plus external cancel retains one unemitted abort in
    // Agent state. There is no Assistant start/end/TurnEnd for that branch.
    let hooks = Arc::new(FinalHooks::new());
    let visits = Arc::new(Mutex::new(Vec::new()));
    let cancelled = CancellationToken::new();
    let trigger = cancelled.clone();
    hooks.binding.set_primary(Some(FinalCallback::new("raw-abort-throw", &visits, move |_, _| {
        trigger.cancel();
        Err(FinalProviderGateError("externally cancelled gate".into()))
    })));
    let provider = Provider::new(Vec::new());
    let agent = Agent::new_raw(config(provider.clone(), tool(&effects), hooks), Vec::new()).unwrap();
    let sink = Arc::new(RawSink::default());
    *sink.agent.lock().unwrap() = Arc::downgrade(&agent);
    let report = bounded(agent.prompt_raw(vec![notification], cancelled, sink.clone())).await.unwrap();
    assert_eq!(report.end, RunEnd::Aborted);
    assert_eq!(agent.raw_messages().await.len(), 2);
    assert!(agent.raw_messages().await[1].same_record(&report.messages[1]));
    assert_eq!(agent.raw_messages().await[1].json().unwrap()["stopReason"], "aborted");
    assert!(provider.contexts.lock().unwrap().is_empty());
    assert!(!sink.events.lock().unwrap().iter().any(|event| match event {
        AgentEvent::MessageStart { message } | AgentEvent::MessageEnd { message } =>
            message.json().unwrap()["role"] == "assistant",
        AgentEvent::TurnEnd { .. } => true,
        _ => false,
    }));
}

// Legacy assertions explicitly inspect owned model snapshots; new raw families
// below continue to assert canonical record identity and lossless bodies.
fn ended_model_message(event: &AgentEvent) -> Option<Message> {
    match event {
        AgentEvent::MessageEnd { message } => message.checked_model_snapshot().unwrap(),
        _ => None,
    }
}

// Appended to the existing core_hooks target after R fixture migration.
mod soft_requirement_port {
    use super::*;

    struct SoftHooks {
        directives: Mutex<VecDeque<Option<SoftToolRequirement>>>,
        reads: AtomicUsize,
        owner: SoftToolRequirementState,
        gate: FinalProviderGateBinding,
    }
    impl SoftHooks {
        fn new(directives: Vec<Option<SoftToolRequirement>>) -> Arc<Self> {
            Arc::new(Self {
                directives: Mutex::new(directives.into()),
                reads: AtomicUsize::new(0),
                owner: SoftToolRequirementState::default(),
                gate: FinalProviderGateBinding::default(),
            })
        }
    }
    #[async_trait]
    impl LoopHooks for SoftHooks {
        fn select_tool_choice_directive_full(
            &self,
            _tools: &[Arc<dyn AgentTool>],
            fallback: Option<&ToolChoice>,
        ) -> Result<ToolChoiceDirectiveSelection, ToolDirectiveError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(ToolChoiceDirectiveSelection {
                directive: self.directives.lock().unwrap().pop_front().flatten().map(ToolChoiceDirective::Soft),
                static_fallback: fallback.cloned(),
                suppress_static_soft_block: false,
            })
        }
        fn soft_tool_requirement_state(&self) -> Option<SoftToolRequirementState> {
            Some(self.owner.clone())
        }
        fn final_provider_gate_binding(&self) -> Option<FinalProviderGateBinding> {
            Some(self.gate.clone())
        }
    }
    fn requirement(id: &str, reminder: &AgentMessage) -> SoftToolRequirement {
        SoftToolRequirement {
            id: id.into(),
            tool_name: "echo".into(),
            satisfies: None,
            reminder: vec![reminder.clone()],
        }
    }
    fn reminder() -> AgentMessage {
        AgentMessage::from_raw(json!({"role":"softReminder","content":"resolve the preview","extra":17}), None).unwrap()
    }
    fn results(report: &RunReport) -> Vec<ToolResultMessage> {
        report
            .messages
            .iter()
            .filter_map(|raw| match raw.checked_model_snapshot().unwrap() {
                Some(Message::ToolResult(result)) => Some(result),
                _ => None,
            })
            .collect()
    }
    fn has_reminder(report: &RunReport, handle: &AgentMessage) -> usize {
        report.messages.iter().filter(|message| message.same_record(handle)).count()
    }

    #[tokio::test]
    async fn soft_reminders_predicates_and_mixed_detours_follow_native_turns() {
        for mode in ["cold-compliant", "mixed", "predicate", "new-id"] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let reminder = reminder();
            let mut req = requirement("preview-1", &reminder);
            let bad = call("bad", json!({"text":"wrong"}));
            if mode == "predicate" {
                req.satisfies = Some(Arc::new(|call| call.arguments.get("text") == Some(&json!("accepted"))));
            }
            let good = call("accepted", json!({"text":"accepted"}));
            let (turns, directives) = if mode == "cold-compliant" {
                (vec![turn(vec![good]), turn(Vec::new())], vec![Some(req), None])
            } else if mode == "new-id" {
                let mut next = req.clone();
                next.id = "preview-2".into();
                (vec![turn(vec![good.clone()]), turn(vec![good]), turn(Vec::new())], vec![Some(req), Some(next), None])
            } else {
                let mut detour = bad;
                if mode == "mixed" {
                    detour.name = "other".into();
                }
                let batch = if mode == "mixed" { vec![good.clone(), detour] } else { vec![detour] };
                (vec![turn(batch), turn(vec![good]), turn(Vec::new())], vec![Some(req.clone()), Some(req), None])
            };
            let provider = Provider::new(turns);
            let hooks = SoftHooks::new(directives);
            let cfg = config(provider.clone(), tool(&effects), hooks.clone());
            let receipts = Receipts::default();
            let mut context = Vec::new();
            let report =
                bounded(agent_loop(vec![user(mode)], &mut context, &cfg, &CancellationToken::new(), &receipts)).await;
            assert_eq!(report.end, RunEnd::Completed, "{mode}");
            assert_eq!(has_reminder(&report, &reminder), if mode == "new-id" { 2 } else { 1 }, "{mode}");
            assert_eq!(reminder.json().unwrap()["extra"], 17);
            let choices = provider.tool_choices.lock().unwrap();
            assert_eq!(choices[0], None, "cold reminder must not invalidate tool choice");
            if mode == "mixed" || mode == "predicate" {
                assert_eq!(choices[1], Some(ToolChoice::Tool("echo".into())));
                let results = results(&report);
                assert!(
                    results
                        .iter()
                        .filter(|result| result.is_error)
                        .all(|result| text(&result.content).contains("resolve the pending action"))
                );
                assert_eq!(
                    results.iter().filter(|result| result.is_error).count(),
                    if mode == "mixed" { 2 } else { 1 }
                );
            }
            assert_eq!(effects.lock().unwrap().len(), if mode == "new-id" { 2 } else { 1 });
            assert!(effects.lock().unwrap().iter().all(|effect| effect.id == "accepted"));
            assert_eq!(hooks.owner.snapshot(), SoftToolRequirementSnapshot::default());
            assert_eq!(hooks.reads.load(Ordering::SeqCst), choices.len());
            assert_eq!(final_turn_counts(&receipts), (choices.len(), choices.len()));
        }
        // The Agent facade filters its selected queued Soft authoritatively.
        // Direct-loop static hard blocks are tested separately below.
        for active in [true, false] {
            for configured in [ToolChoice::None, ToolChoice::Tool("other".into())] {
                let effects = Arc::new(Mutex::new(Vec::new()));
                let reminder = reminder();
                let req = requirement("agent-soft-static", &reminder);
                let turns = if active {
                    vec![turn(Vec::new()), turn(vec![call("accepted", json!({"text":"accepted"}))]), turn(Vec::new())]
                } else {
                    vec![turn(Vec::new())]
                };
                let directives = if active { vec![Some(req.clone()), Some(req), None] } else { vec![Some(req)] };
                let hooks = SoftHooks::new(directives);
                let provider = Provider::new(turns);
                let mut cfg = config(provider.clone(), tool(&effects), hooks);
                cfg.tool_choice = Some(configured.clone());
                if !active {
                    cfg.tools.clear();
                }
                let agent = Agent::new(cfg, Vec::new());
                let report = hard_prompt(&agent, "queued soft is authoritative").await;
                assert_eq!(report.end, RunEnd::Completed, "{active}/{configured:?}");
                let choices = provider.tool_choices.lock().unwrap();
                assert_eq!(choices[0], None, "queued Soft must suppress static fallback");
                if active {
                    assert_eq!(choices[1], Some(ToolChoice::Tool("echo".into())));
                    assert_eq!(effects.lock().unwrap().len(), 1);
                    assert_eq!(has_reminder(&report, &reminder), 1);
                } else {
                    assert_eq!(choices.len(), 1);
                    assert!(effects.lock().unwrap().is_empty());
                    assert_eq!(has_reminder(&report, &reminder), 0);
                }
            }
        }
    }

    #[tokio::test]
    async fn soft_cap_terminal_errors_static_conflicts_and_predicate_panic_are_bounded() {
        for (choice, required, blocks) in [
            (ToolChoice::None, "echo", true),
            (ToolChoice::Auto, "echo", false),
            (ToolChoice::Any, "echo", false),
            (ToolChoice::Required, "echo", false),
            (ToolChoice::Function("other".into()), "echo", true),
            (ToolChoice::FunctionNested("other".into()), "echo", true),
            (ToolChoice::Tool("other".into()), "echo", true),
            (ToolChoice::Function("echo".into()), "echo", false),
            (ToolChoice::FunctionNested("echo".into()), "echo", false),
            (ToolChoice::Tool("echo".into()), "echo", false),
            (ToolChoice::Computer, "echo", true),
            (ToolChoice::Computer, "computer", false),
        ] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let reminder = reminder();
            let mut req = requirement("static", &reminder);
            req.tool_name = required.into();
            let hooks = SoftHooks::new(vec![Some(req), None]);
            let provider = Provider::new(vec![turn(Vec::new()), turn(Vec::new())]);
            let mut cfg = config(provider.clone(), tool(&effects), hooks);
            cfg.tool_choice = Some(choice.clone());
            let report = bounded(agent_loop(
                vec![user("static")],
                &mut Vec::new(),
                &cfg,
                &CancellationToken::new(),
                &Receipts::default(),
            ))
            .await;
            assert_eq!(report.end, RunEnd::Completed);
            assert_eq!(provider.contexts.lock().unwrap().len(), if blocks { 1 } else { 2 }, "{choice:?}");
        }
        for mode in ["cap", "error", "aborted", "predicate-panic"] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let reminder = reminder();
            let mut req = requirement(mode, &reminder);
            if mode == "predicate-panic" {
                req.satisfies = Some(Arc::new(|_| panic!("predicate failure")));
            }
            let hooks = SoftHooks::new(vec![Some(req.clone()); 4]);
            let mut initial =
                turn(if mode == "predicate-panic" { vec![call("panic", json!({"text":"x"}))] } else { Vec::new() });
            if mode == "error" || mode == "aborted" {
                initial.stop = if mode == "error" { StopReason::Error } else { StopReason::Aborted };
                initial.error_event = true;
            }
            let provider =
                Provider::new(if mode == "cap" { (0..4).map(|_| turn(Vec::new())).collect() } else { vec![initial] });
            let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
            cfg.max_model_calls = Some(8);
            let receipts = Receipts::default();
            let report =
                bounded(agent_loop(vec![user(mode)], &mut Vec::new(), &cfg, &CancellationToken::new(), &receipts))
                    .await;
            assert_eq!(report.end, if mode == "aborted" { RunEnd::Aborted } else { RunEnd::Error });
            let calls = provider.contexts.lock().unwrap().len();
            assert_eq!(calls, if mode == "cap" { 4 } else { 1 });
            assert_eq!(final_turn_counts(&receipts), (calls, calls), "no duplicate start on post-Assistant failure");
            assert!(effects.lock().unwrap().is_empty());
            assert_eq!(hooks.owner.snapshot(), SoftToolRequirementSnapshot::default());
            if mode == "cap" {
                let choices = provider.tool_choices.lock().unwrap();
                assert_eq!(choices[0], None);
                assert!(choices[1..].iter().all(|choice| *choice == Some(ToolChoice::Tool("echo".into()))));
                let terminal = report.messages.last().unwrap().checked_model_snapshot().unwrap().unwrap();
                assert!(
                    matches!(terminal, Message::Assistant(message) if message.error_message.as_deref().unwrap().contains("after 3 forced turns"))
                );
            }
        }
        // A finalized Length/deadline turn still applies Soft before generic
        // placeholders. Delay only the sink acknowledgement past the deadline.
        // The call helper always names echo: an ID of detour is not a
        // different tool. Keep both the default-compliant native branch and
        // the explicitly noncompliant predicate branch in this original family.
        for (deadline_mode, noncompliant) in [(false, true), (false, false), (true, true), (true, false)] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let reminder = reminder();
            let mut req = requirement("late-soft", &reminder);
            if noncompliant {
                req.satisfies = Some(Arc::new(|call| {
                    call.name == "echo" && call.arguments.get("text") == Some(&json!("accepted"))
                }));
            }
            let hooks = SoftHooks::new(vec![Some(req.clone()), Some(req), None]);
            let mut first = turn(vec![call("detour", json!({"text":"wrong"}))]);
            first.stop = if deadline_mode { StopReason::ToolUse } else { StopReason::Length };
            let turns = if deadline_mode {
                vec![first]
            } else {
                vec![first, turn(vec![call("accepted", json!({"text":"accepted"}))]), turn(Vec::new())]
            };
            let provider = Provider::new(turns);
            let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
            let receipts = Arc::new(Receipts::default());
            struct LateAssistantAck {
                receipts: Arc<Receipts>,
                deadline: std::time::Instant,
            }
            #[async_trait]
            impl AgentEventSink for LateAssistantAck {
                async fn emit(&self, event: AgentEvent) {
                    let final_assistant = matches!(&event, AgentEvent::MessageEnd { message }
                        if matches!(message.checked_model_snapshot(), Ok(Some(Message::Assistant(_)))));
                    self.receipts.emit(event).await;
                    if final_assistant {
                        tokio::time::sleep_until(tokio::time::Instant::from_std(
                            self.deadline + Duration::from_millis(20),
                        ))
                        .await;
                    }
                }
            }
            let sink: Arc<dyn AgentEventSink> = if deadline_mode {
                let deadline = std::time::Instant::now() + Duration::from_millis(250);
                cfg.deadline = Some(deadline);
                Arc::new(LateAssistantAck { receipts: receipts.clone(), deadline })
            } else {
                receipts.clone()
            };
            let report = bounded(agent_loop(
                vec![user("length/deadline Soft order")],
                &mut Vec::new(),
                &cfg,
                &CancellationToken::new(),
                sink.as_ref(),
            ))
            .await;
            assert_eq!(report.end, if deadline_mode { RunEnd::Deadline } else { RunEnd::Completed });
            let skipped = results(&report).into_iter().find(|result| result.tool_call_id == "detour").unwrap();
            assert!(skipped.is_error);
            let (expected_source, expected_text) = if noncompliant {
                (
                    "assistant_stop_skipped",
                    "Tool call was not executed because the assistant ended its turn: Not executed: call the `echo` tool to resolve the pending action before using other tools.",
                )
            } else if deadline_mode {
                ("assistant_stop_aborted", "Tool execution was aborted: Deadline exceeded")
            } else {
                (
                    "assistant_stop_length",
                    "Tool call was not executed because the assistant hit its output token limit (stop_reason: length) before the arguments could complete; the recorded arguments are truncated and unsafe to run. Do NOT retry by re-emitting the same large payload — split the work into several smaller tool calls (e.g. for `write`/`edit`, write the first chunk then append the rest with subsequent `edit` insert ops, or break the file into multiple `write` targets).",
                )
            };
            let mut actual_result = serde_json::to_value(Message::ToolResult(skipped.clone())).unwrap();
            assert!(actual_result["timestamp"].is_i64());
            actual_result.as_object_mut().unwrap().remove("timestamp");
            assert_eq!(
                actual_result,
                json!({
                    "role":"toolResult", "toolCallId":"detour", "toolName":"echo",
                    "content":[{"type":"text","text":expected_text}],
                    "details":{"__synthetic":true,"source":expected_source,"executed":false},
                    "isError":true,
                })
            );
            assert_eq!(text(&skipped.content).contains("resolve the pending action"), noncompliant);
            assert_eq!(effects.lock().unwrap().len(), if deadline_mode { 0 } else { 1 });
            let choices = provider.tool_choices.lock().unwrap();
            assert_eq!(choices[0], None);
            if !deadline_mode {
                assert_eq!(choices[1], if noncompliant { Some(ToolChoice::Tool("echo".into())) } else { None });
            }
            assert_eq!(hooks.owner.snapshot(), SoftToolRequirementSnapshot::default());
        }
    }

    #[tokio::test]
    async fn soft_agent_gstop_preserves_state_and_active_clear_detaches_the_owner() {
        for mode in ["single-queues", "clear-all", "clear-deferred", "cancel", "error", "panic"] {
            let clears = mode != "single-queues";
            let effects = Arc::new(Mutex::new(Vec::new()));
            let reminder = reminder();
            let req = requirement("preview", &reminder);
            let hooks = SoftHooks::new(vec![Some(req.clone()), Some(req.clone()), Some(req), None]);
            let hold = Arc::new(Gate::default());
            let gate_count = Arc::new(AtomicUsize::new(0));
            let count = gate_count.clone();
            let held = hold.clone();
            struct StopSecond {
                count: Arc<AtomicUsize>,
                hold: Arc<Gate>,
                mode: &'static str,
            }
            #[async_trait]
            impl FinalProviderGate for StopSecond {
                async fn before_model_call(
                    &self,
                    _: &mut Context,
                    _: &CancellationToken,
                ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
                    if self.count.fetch_add(1, Ordering::SeqCst) == 1 {
                        self.hold.started.notify_one();
                        self.hold.release.notified().await;
                        match self.mode {
                            "error" => Err(FinalProviderGateError("owner gate error".into())),
                            "panic" => panic!("owner gate panic"),
                            _ => Ok(Some(FinalProviderGateStop::default())),
                        }
                    } else {
                        Ok(None)
                    }
                }
            }
            hooks.gate.set_primary(Some(Arc::new(StopSecond { count, hold: held, mode })));
            let provider = Provider::new(vec![
                turn(Vec::new()),
                turn(vec![call("accepted", json!({"text":"accepted"}))]),
                turn(Vec::new()),
            ]);
            let agent = Agent::new(config(provider.clone(), tool(&effects), hooks.clone()), Vec::new());
            let owned = agent.clone();
            let cancel = CancellationToken::new();
            let run_cancel = cancel.clone();
            let running = tokio::spawn(async move {
                bounded(owned.prompt(vec![user("first")], run_cancel, Arc::new(Receipts::default()))).await.unwrap()
            });
            bounded(hold.started.notified()).await;
            match mode {
                "clear-all" => agent.clear_all_queues(),
                "clear-deferred" => agent.clear_deferred_tool_directives(),
                "cancel" => cancel.cancel(),
                "single-queues" => {
                    agent.clear_steering_queue();
                    agent.clear_follow_up_queue();
                }
                _ => {}
            }
            hold.release.notify_one();
            let first = bounded(running).await.unwrap();
            assert_eq!(
                first.end,
                match mode {
                    "cancel" => RunEnd::Aborted,
                    "error" | "panic" => RunEnd::Error,
                    _ => RunEnd::PreModelStop,
                }
            );
            assert_eq!(has_reminder(&first, &reminder), 1);
            hooks.gate.set_primary(None);
            let second =
                bounded(agent.prompt(vec![user("second")], CancellationToken::new(), Arc::new(Receipts::default())))
                    .await
                    .unwrap();
            assert_eq!(second.end, RunEnd::Completed);
            assert_eq!(has_reminder(&second, &reminder), usize::from(clears));
            let choices = provider.tool_choices.lock().unwrap();
            assert_eq!(choices[1], if clears { None } else { Some(ToolChoice::Tool("echo".into())) });
            assert_eq!(effects.lock().unwrap().len(), 1);
        }
    }
}

// Appended to the existing core_hooks target after R fixture migration.
mod paused_turn_port {
    use super::*;

    fn paused() -> Turn {
        let mut result = turn(Vec::new());
        result.stop_details = Some(json!({"type":"pause_turn"}));
        result
    }

    #[tokio::test]
    async fn nonterminal_pauses_replay_cap_reset_and_respect_terminal_budgets() {
        for mode in ["resume", "cap", "reset", "error", "aborted", "length", "budget", "not-pause"] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let turns = match mode {
                "resume" => vec![paused(), turn(vec![call("effect", json!({"text":"accepted"}))]), turn(Vec::new())],
                "cap" => (0..9).map(|_| paused()).collect(),
                "reset" => (0..8)
                    .map(|_| paused())
                    .chain(std::iter::once(turn(vec![call("reset", json!({"text":"accepted"}))])))
                    .chain((0..8).map(|_| paused()))
                    .chain(std::iter::once(turn(Vec::new())))
                    .collect(),
                "budget" => vec![paused(), paused()],
                _ => {
                    let mut result = paused();
                    match mode {
                        "error" => {
                            result.stop = StopReason::Error;
                            result.error_event = true;
                        }
                        "aborted" => {
                            result.stop = StopReason::Aborted;
                            result.error_event = true;
                        }
                        "length" => result.stop = StopReason::Length,
                        "not-pause" => result.stop_details = Some(json!({"type":"stop"})),
                        _ => unreachable!(),
                    }
                    vec![result]
                }
            };
            let expected_calls = turns.len();
            let provider = Provider::new(turns);
            let mut cfg = config(provider.clone(), tool(&effects), Arc::new(NoHooks));
            cfg.max_model_calls = Some(if mode == "budget" { 2 } else { 32 });
            let receipts = Receipts::default();
            let report = bounded(agent_loop(
                vec![user("continue actual work")],
                &mut Vec::new(),
                &cfg,
                &CancellationToken::new(),
                &receipts,
            ))
            .await;
            let expected_end = match mode {
                "error" => RunEnd::Error,
                "aborted" => RunEnd::Aborted,
                "budget" => RunEnd::ModelCallBudget,
                _ => RunEnd::Completed,
            };
            assert_eq!(report.end, expected_end, "{mode}");
            let contexts = provider.contexts.lock().unwrap();
            assert_eq!(contexts.len(), expected_calls, "{mode}");
            assert_eq!(final_turn_counts(&receipts), (expected_calls, expected_calls), "{mode}");
            if expected_calls > 1 {
                assert!(contexts[1].messages.iter().any(|message| matches!(message, Message::Assistant(message)
                    if message.stop_details.as_ref().is_some_and(|details| details["type"] == "pause_turn"))));
            }
            assert_eq!(effects.lock().unwrap().len(), usize::from(mode == "resume" || mode == "reset"));
            assert!(provider.turns.lock().unwrap().is_empty(), "cap must close at the ninth pause");
        }
    }
}

// Coarse P/A families appended to the existing Core target. They are fixture
// source until Root runs the delivered composition; no Host UI/producer claim.
mod pause_aside_port {
    use super::*;

    fn raw(name: &str) -> AgentMessage {
        AgentMessage::from_raw(
            json!({"role":"user","content":name,"timestamp":7,"application":{"aside":name}}),
            Some(Arc::new(json!({"fixtureSource":name}))),
        )
        .unwrap()
    }

    fn claim(
        name: &'static str,
        visits: &Arc<Mutex<Vec<String>>>,
        commit_error: bool,
        discard_error: bool,
    ) -> AgentMessage {
        let record = raw(name);
        let identity = record.weak_identity();
        let commits = visits.clone();
        let discards = visits.clone();
        record.set_aside_callbacks(AsideCallbacks {
            commit: Some(Arc::new(move || {
                assert_eq!(identity.upgrade().unwrap().json().unwrap()["application"]["aside"], name);
                commits.lock().unwrap().push(format!("commit:{name}"));
                if commit_error { Err(AsideError(format!("commit-{name}"))) } else { Ok(()) }
            })),
            discard: Some(Arc::new(move |_| {
                discards.lock().unwrap().push(format!("discard:{name}"));
                if discard_error { Err(AsideError(format!("discard-{name}"))) } else { Ok(()) }
            })),
        });
        record
    }

    #[derive(Default)]
    struct Asides(Mutex<Vec<AsideMessage>>, AtomicUsize);
    #[async_trait]
    impl AsideMessageProvider for Asides {
        async fn messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(std::mem::take(&mut *self.0.lock().unwrap()))
        }
    }

    struct PassiveHooks {
        asides: Arc<Asides>,
        before: Option<Arc<dyn BeforeYieldHook>>,
    }
    #[async_trait]
    impl LoopHooks for PassiveHooks {
        async fn aside_messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
            self.asides.messages().await
        }
        async fn before_yield(&self) -> Result<(), AsideError> {
            if let Some(hook) = &self.before { hook.before_yield().await } else { Ok(()) }
        }
    }
    struct YieldAction(Arc<dyn Fn() -> Result<(), AsideError> + Send + Sync>);
    #[async_trait]
    impl BeforeYieldHook for YieldAction {
        async fn before_yield(&self) -> Result<(), AsideError> {
            (self.0)()
        }
    }

    #[tokio::test]
    async fn pause_gate_waiters_repause_cancel_listener_reentry_and_unsubscribe_family() {
        let gate = AgentPauseGate::new();
        assert!(!gate.paused());
        assert_eq!(gate.paused_at(), None);
        assert_eq!(gate.resume(), None);
        let changes = Arc::new(Mutex::new(Vec::new()));
        let seen = changes.clone();
        let handle = gate.on_change(Arc::new(move |paused| seen.lock().unwrap().push(paused)));
        let throwing = gate.on_change(Arc::new(|_| panic!("listener exception")));
        assert!(gate.pause());
        assert!(gate.paused_at().is_some());
        assert!(!gate.pause());
        let cancel = CancellationToken::new();
        let one = gate.wait_until_resumed(Some(&cancel));
        let two = gate.wait_until_resumed(None);
        tokio::pin!(one, two);
        assert!(futures::poll!(one.as_mut()).is_pending());
        assert!(futures::poll!(two.as_mut()).is_pending());
        assert!(gate.resume().is_some());
        assert!(gate.pause());
        assert!(futures::poll!(one.as_mut()).is_pending(), "a re-pause cannot leak a wake");
        assert!(futures::poll!(two.as_mut()).is_pending());
        cancel.cancel();
        bounded(one.as_mut()).await;
        assert!(gate.paused(), "one cancellation is not global resume");
        assert!(futures::poll!(two.as_mut()).is_pending());
        gate.resume().unwrap();
        bounded(two.as_mut()).await;
        assert_eq!(*changes.lock().unwrap(), vec![true, false, true, false]);
        handle.unsubscribe();
        drop(throwing);
        let count = changes.lock().unwrap().len();
        let self_handle = Arc::new(Mutex::new(None::<PauseSubscription>));
        let owned = self_handle.clone();
        let reentrant = gate.clone();
        *self_handle.lock().unwrap() = Some(gate.on_change(Arc::new(move |paused| {
            if paused {
                assert!(reentrant.paused());
                owned.lock().unwrap().take().unwrap().unsubscribe();
                reentrant.resume().unwrap();
            }
        })));
        assert!(gate.pause());
        assert!(!gate.paused());
        assert_eq!(changes.lock().unwrap().len(), count);
        assert_eq!(gate.paused_at(), None);
    }

    struct GlobalReset;
    impl Drop for GlobalReset {
        fn drop(&mut self) {
            AGENT_PAUSE_GATE.resume();
        }
    }
    #[derive(Default)]
    struct PauseSink {
        receipts: Receipts,
        assistant_done: Notify,
        tool_done: Notify,
    }
    #[async_trait]
    impl AgentEventSink for PauseSink {
        async fn emit(&self, event: AgentEvent) {
            let assistant = matches!(&event, AgentEvent::MessageEnd { message }
                if matches!(message.checked_model_snapshot(), Ok(Some(Message::Assistant(_)))));
            let tool = matches!(&event, AgentEvent::ToolExecutionEnd { .. });
            self.receipts.emit(event).await;
            if assistant {
                self.assistant_done.notify_one();
            }
            if tool {
                self.tool_done.notify_one();
            }
        }
    }

    // Exact-filter child protects unrelated concurrent tests from the real gate.
    #[test]
    fn process_singleton_agents_inflight_and_all_scheduled_slots_family() {
        const SENTINEL: &str = "ARA_CORE_PAUSE_SINGLETON_CHILD_V1";
        if std::env::var(SENTINEL).as_deref() != Ok("1") {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "pause_aside_port::process_singleton_agents_inflight_and_all_scheduled_slots_family",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(SENTINEL, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let _reset = GlobalReset;
            AGENT_PAUSE_GATE.resume();
            let effects = Arc::new(Mutex::new(Vec::new()));
            let tool_hold = Arc::new(Gate::default());
            let provider_hold = Arc::new(Gate::default());
            let waiting =
                Arc::new(Echo { effects: effects.clone(), waiting: Some(tool_hold.clone()), foreground: None });
            let first = Provider::new(vec![
                turn(vec![call("running-tool", json!({"text":"running","mode":"wait"}))]),
                turn(Vec::new()),
                turn(Vec::new()),
            ]);
            let mut other_turn = turn(vec![call("parked-slot", json!({"text":"parked"}))]);
            other_turn.hold = Some(provider_hold.clone());
            let second = Provider::new(vec![other_turn]);
            let one = Agent::new(config(first.clone(), waiting, Arc::new(NoHooks)), Vec::new());
            let two = Agent::new(config(second.clone(), tool(&effects), Arc::new(NoHooks)), Vec::new());
            let first_sink = Arc::new(PauseSink::default());
            let second_sink = Arc::new(PauseSink::default());
            let cancel_two = CancellationToken::new();
            let a = one.clone();
            let s = first_sink.clone();
            let first_run = tokio::spawn(async move { a.prompt(vec![user("one")], CancellationToken::new(), s).await });
            let a = two.clone();
            let s = second_sink.clone();
            let c = cancel_two.clone();
            let second_run = tokio::spawn(async move { a.prompt(vec![user("two")], c, s).await });
            bounded(tool_hold.started.notified()).await;
            bounded(provider_hold.started.notified()).await;
            assert!(AGENT_PAUSE_GATE.pause());
            one.follow_up(user("retained follow-up"));
            two.follow_up(user("retain on abort"));
            tool_hold.release.notify_one();
            provider_hold.release.notify_one();
            bounded(first_sink.tool_done.notified()).await;
            bounded(second_sink.assistant_done.notified()).await;
            assert_eq!(effects.lock().unwrap().len(), 1, "in-flight tool finishes, later slots remain unexecuted");
            assert!(!tool_hold.cancelled.load(Ordering::SeqCst));
            assert!(!effects.lock().unwrap()[0].hard_cancelled);
            assert_eq!(first.contexts.lock().unwrap().len(), 1);
            cancel_two.cancel();
            assert_eq!(bounded(second_run).await.unwrap().unwrap().end, RunEnd::Aborted);
            assert!(AGENT_PAUSE_GATE.paused());
            assert_eq!(two.queued_counts().1, 1);
            assert!(!first_run.is_finished());
            AGENT_PAUSE_GATE.resume().unwrap();
            assert_eq!(bounded(first_run).await.unwrap().unwrap().end, RunEnd::Completed);
            assert_eq!(first.contexts.lock().unwrap().len(), 3);
            assert_eq!(one.queued_counts(), (0, 0));

            struct BlockSlot(bool);
            #[async_trait]
            impl LoopHooks for BlockSlot {
                async fn before_tool_dispatch(
                    &self,
                    _: &mut BeforeToolCallContext<'_>,
                    _: &CancellationToken,
                ) -> Result<BeforeToolCallResult, ToolHookError> {
                    Ok(BeforeToolCallResult {
                        block: self.0,
                        reason: Some("fixture block".into()),
                        ..Default::default()
                    })
                }
            }
            for mode in ["valid", "invalid", "blocked"] {
                AGENT_PAUSE_GATE.pause();
                let cfg = config(Provider::new(Vec::new()), tool(&effects), Arc::new(BlockSlot(mode == "blocked")));
                let mut message = AssistantMessage::empty(&cfg.model.api, &cfg.model.provider, &cfg.model.id);
                message.stop_reason = StopReason::ToolUse;
                message.content.push(AssistantBlock::ToolCall(call(
                    mode,
                    if mode == "invalid" { json!({}) } else { json!({"text":"slot"}) },
                )));
                let cancel = CancellationToken::new();
                let receipts = Receipts::default();
                let execution = execute_tool_calls(&message, &cfg, &cancel, &receipts);
                tokio::pin!(execution);
                assert!(
                    futures::poll!(execution.as_mut()).is_pending(),
                    "{mode} scheduled slot parks before its receipt"
                );
                assert!(receipts.events.lock().unwrap().is_empty());
                AGENT_PAUSE_GATE.resume().unwrap();
                AGENT_PAUSE_GATE.pause();
                assert!(futures::poll!(execution.as_mut()).is_pending(), "actual singleton re-pause");
                if mode == "valid" {
                    AGENT_PAUSE_GATE.resume().unwrap();
                } else {
                    cancel.cancel();
                }
                let result = bounded(execution.as_mut()).await;
                assert_eq!(result.len(), 1);
                assert_eq!(result[0].is_error, mode != "valid");
                if mode != "valid" {
                    assert!(AGENT_PAUSE_GATE.paused());
                }
                AGENT_PAUSE_GATE.resume();
            }
            AGENT_PAUSE_GATE.pause();
            let p = Provider::new(vec![turn(Vec::new())]);
            let mut cfg = config(p.clone(), tool(&effects), Arc::new(NoHooks));
            cfg.deadline = Some(std::time::Instant::now() + Duration::from_millis(40));
            let agent = Agent::new(cfg, Vec::new());
            let report = bounded(agent.prompt(
                vec![user("deadline park")],
                CancellationToken::new(),
                Arc::new(Receipts::default()),
            ))
            .await
            .unwrap();
            assert_eq!(report.end, RunEnd::Deadline);
            assert!(AGENT_PAUSE_GATE.paused());
            assert!(p.contexts.lock().unwrap().is_empty());
        });
    }

    struct OrderedSink(Arc<Mutex<Vec<String>>>);
    #[async_trait]
    impl AgentEventSink for OrderedSink {
        async fn emit(&self, event: AgentEvent) {
            if matches!(event, AgentEvent::AgentStart) {
                self.0.lock().unwrap().push("start".into());
            }
            if matches!(event, AgentEvent::AgentEnd { .. }) {
                self.0.lock().unwrap().push("end".into());
            }
        }
    }

    #[tokio::test]
    async fn aside_initial_lazy_raw_identity_and_native_callback_failure_family() {
        let effects = Arc::new(Mutex::new(Vec::new()));
        let visits = Arc::new(Mutex::new(Vec::new()));
        let record = claim("initial", &visits, false, false);
        let cfg = config(Provider::new(vec![turn(Vec::new())]), tool(&effects), Arc::new(NoHooks));
        let mut context = Vec::new();
        let report = bounded(try_agent_loop_raw(
            vec![record.clone()],
            &mut context,
            &cfg,
            &CancellationToken::new(),
            &OrderedSink(visits.clone()),
        ))
        .await
        .unwrap();
        assert!(context[0].same_record(&record));
        assert!(report.messages[0].same_record(&record));
        assert!(Arc::ptr_eq(&record.source().unwrap(), &context[0].source().unwrap()));
        assert_eq!(*visits.lock().unwrap(), vec!["commit:initial", "start", "end"]);
        assert_eq!(serde_json::to_value(&record).unwrap(), record.json().unwrap());
        assert!(!format!("{record:?}").contains("callbacks"));
        for panic_mode in [false, true] {
            let p = Provider::new(Vec::new());
            let cfg = config(p.clone(), tool(&effects), Arc::new(NoHooks));
            let agent = Agent::new(cfg, Vec::new());
            let failed = raw("initial failure");
            failed.set_aside_callbacks(AsideCallbacks {
                commit: Some(Arc::new(move || {
                    if panic_mode {
                        panic!("initial throw");
                    }
                    Err(AsideError("initial error".into()))
                })),
                ..Default::default()
            });
            let receipts = Arc::new(Receipts::default());
            assert!(matches!(
                bounded(agent.prompt_raw(vec![failed], CancellationToken::new(), receipts.clone())).await,
                Err(AgentError::Aside(_))
            ));
            assert!(receipts.events.lock().unwrap().is_empty(), "initial errors fabricate no start/end pair");
            assert!(p.contexts.lock().unwrap().is_empty());
            assert!(matches!(
                agent.prompt(vec![user("recovery required")], CancellationToken::new(), receipts).await,
                Err(AgentError::NeedsRecovery)
            ));
        }
        for discard_error in [false, true] {
            let visits = Arc::new(Mutex::new(Vec::new()));
            let resolved = claim("resolved", &visits, false, discard_error);
            let untouched = Arc::new(AtomicUsize::new(0));
            let count = untouched.clone();
            let entries = vec![
                AsideMessage::Ready(resolved),
                AsideMessage::Lazy(Arc::new(|| Ok(None))),
                AsideMessage::Lazy(Arc::new(|| Err(AsideError("resolver-error".into())))),
                AsideMessage::Lazy(Arc::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(raw("never")))
                })),
            ];
            let asides = Arc::new(Asides(Mutex::new(entries), AtomicUsize::new(0)));
            let cfg = config(
                Provider::new(vec![turn(Vec::new())]),
                tool(&effects),
                Arc::new(PassiveHooks { asides, before: None }),
            );
            let error = bounded(try_agent_loop_raw(
                vec![raw("prompt")],
                &mut Vec::new(),
                &cfg,
                &CancellationToken::new(),
                &Receipts::default(),
            ))
            .await
            .unwrap_err();
            assert_eq!(error.0, if discard_error { "discard-resolved" } else { "resolver-error" });
            assert_eq!(*visits.lock().unwrap(), vec!["discard:resolved"]);
            assert_eq!(untouched.load(Ordering::SeqCst), 0);
        }
        for discard_error in [false, true] {
            let visits = Arc::new(Mutex::new(Vec::new()));
            let a = claim("a", &visits, false, false);
            let b = claim("b", &visits, true, discard_error);
            let c = claim("c", &visits, false, false);
            let asides = Arc::new(Asides(
                Mutex::new(vec![a.clone().into(), b.clone().into(), c.clone().into()]),
                AtomicUsize::new(0),
            ));
            let provider = Provider::new(vec![turn(Vec::new())]);
            let cfg = config(provider.clone(), tool(&effects), Arc::new(PassiveHooks { asides, before: None }));
            let mut context = Vec::new();
            let error = bounded(try_agent_loop_raw(
                vec![raw("prompt")],
                &mut context,
                &cfg,
                &CancellationToken::new(),
                &Receipts::default(),
            ))
            .await
            .unwrap_err();
            assert_eq!(error.0, if discard_error { "discard-b" } else { "commit-b" });
            let expected = if discard_error {
                vec!["commit:a", "commit:b", "discard:a", "discard:b"]
            } else {
                vec!["commit:a", "commit:b", "discard:a", "discard:b", "discard:c"]
            };
            assert_eq!(*visits.lock().unwrap(), expected);
            assert!(context.iter().any(|message| message.same_record(&a)));
            assert!(context.iter().any(|message| message.same_record(&b)));
            assert!(!context.iter().any(|message| message.same_record(&c)));
            assert_eq!(provider.contexts.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn aside_passive_midwork_live_setters_yield_order_and_cancel_family() {
        let effects = Arc::new(Mutex::new(Vec::new()));
        let hold = Arc::new(Gate::default());
        let provider = Provider::new(vec![
            turn(vec![call("working", json!({"text":"work","mode":"wait"}))]),
            turn(Vec::new()),
            turn(Vec::new()),
        ]);
        let waiting = Arc::new(Echo { effects: effects.clone(), waiting: Some(hold.clone()), foreground: None });
        let agent = Agent::new(config(provider.clone(), waiting, Arc::new(NoHooks)), Vec::new());
        let old = Arc::new(Asides(Mutex::new(vec![raw("old provider").into()]), AtomicUsize::new(0)));
        agent.set_aside_message_provider(Some(old.clone()));
        let old_yields = Arc::new(AtomicUsize::new(0));
        let calls = old_yields.clone();
        agent.set_on_before_yield(Some(Arc::new(YieldAction(Arc::new(move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })))));
        let a = agent.clone();
        let run = tokio::spawn(async move {
            a.prompt(vec![user("start work")], CancellationToken::new(), Arc::new(Receipts::default())).await
        });
        bounded(hold.started.notified()).await;
        let visits = Arc::new(Mutex::new(Vec::new()));
        let passive = claim("passive", &visits, false, false);
        let asides = Arc::new(Asides(
            Mutex::new(vec![AsideMessage::Lazy({
                let passive = passive.clone();
                Arc::new(move || Ok(Some(passive.clone())))
            })]),
            AtomicUsize::new(0),
        ));
        agent.set_aside_message_provider(Some(asides.clone()));
        let owner = Arc::downgrade(&agent);
        let queue = asides.clone();
        let late = raw("late steering");
        let aside = raw("late aside");
        let follow = raw("follow-up");
        let yielded = Arc::new(AtomicUsize::new(0));
        let count = yielded.clone();
        agent.set_on_before_yield(Some(Arc::new(YieldAction(Arc::new(move || {
            if count.fetch_add(1, Ordering::SeqCst) == 0 {
                let agent = owner.upgrade().unwrap();
                agent.steer_raw(late.clone()).unwrap();
                queue.0.lock().unwrap().push(aside.clone().into());
                agent.follow_up_raw(follow.clone()).unwrap();
            }
            Ok(())
        })))));
        assert_eq!(agent.queued_counts(), (0, 0), "passive input is not steering");
        hold.release.notify_one();
        let report = bounded(run).await.unwrap().unwrap();
        assert_eq!(report.end, RunEnd::Completed);
        assert!(!hold.cancelled.load(Ordering::SeqCst));
        assert!(!effects.lock().unwrap()[0].hard_cancelled && !effects.lock().unwrap()[0].soft_cancelled);
        assert_eq!(old.1.load(Ordering::SeqCst), 0);
        assert_eq!(old_yields.load(Ordering::SeqCst), 0);
        assert_eq!(yielded.load(Ordering::SeqCst), 2);
        let inputs: Vec<_> = report
            .messages
            .iter()
            .filter_map(|message| message.json().unwrap()["application"]["aside"].as_str().map(str::to_string))
            .collect();
        assert_eq!(inputs, vec!["passive", "late steering", "late aside", "follow-up"]);
        assert!(report.messages.iter().any(|message| message.same_record(&passive)));
        assert_eq!(*visits.lock().unwrap(), vec!["commit:passive"]);
        assert_eq!(provider.contexts.lock().unwrap().len(), 3);

        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let asides = Arc::new(Asides(Mutex::new(vec![raw("unconsumed").into()]), AtomicUsize::new(0)));
        let hook = Arc::new(YieldAction(Arc::new(move || {
            stop.cancel();
            Ok(())
        })));
        let cfg = config(
            Provider::new(vec![turn(Vec::new())]),
            tool(&effects),
            Arc::new(PassiveHooks { asides: asides.clone(), before: Some(hook) }),
        );
        let report = bounded(try_agent_loop_raw(
            vec![raw("cancel at yield")],
            &mut Vec::new(),
            &cfg,
            &cancel,
            &Receipts::default(),
        ))
        .await
        .unwrap();
        assert_eq!(report.end, RunEnd::Aborted);
        assert_eq!(asides.1.load(Ordering::SeqCst), 0);
        assert_eq!(asides.0.lock().unwrap().len(), 1);

        // A Stop turn can be externally canceled by its TurnEnd sink. Native
        // still settles before-yield, and a hook failure remains a typed error.
        struct CancelAtTurnEnd {
            cancel: CancellationToken,
            receipts: Receipts,
        }
        #[async_trait]
        impl AgentEventSink for CancelAtTurnEnd {
            async fn emit(&self, event: AgentEvent) {
                let turn_end = matches!(&event, AgentEvent::TurnEnd { .. });
                self.receipts.emit(event).await;
                if turn_end {
                    self.cancel.cancel();
                }
            }
        }
        for hook_error in [false, true] {
            let cancel = CancellationToken::new();
            let observed_cancel = cancel.clone();
            let yielded = Arc::new(AtomicUsize::new(0));
            let calls = yielded.clone();
            let asides = Arc::new(Asides(Mutex::new(vec![raw("still queued").into()]), AtomicUsize::new(0)));
            let hook = Arc::new(YieldAction(Arc::new(move || {
                assert!(observed_cancel.is_cancelled(), "TurnEnd cancellation precedes this hook");
                calls.fetch_add(1, Ordering::SeqCst);
                if hook_error { Err(AsideError("yield-after-cancel".into())) } else { Ok(()) }
            })));
            let provider = Provider::new(vec![turn(Vec::new())]);
            let cfg = config(
                provider.clone(),
                tool(&effects),
                Arc::new(PassiveHooks { asides: asides.clone(), before: Some(hook) }),
            );
            let sink = CancelAtTurnEnd { cancel: cancel.clone(), receipts: Receipts::default() };
            let result =
                bounded(try_agent_loop_raw(vec![raw("cancel in TurnEnd")], &mut Vec::new(), &cfg, &cancel, &sink))
                    .await;
            assert_eq!(yielded.load(Ordering::SeqCst), 1, "settlement hook must not be skipped");
            assert_eq!(provider.contexts.lock().unwrap().len(), 1);
            assert_eq!(asides.1.load(Ordering::SeqCst), 0, "cancellation skips new consuming polls");
            assert_eq!(asides.0.lock().unwrap().len(), 1);
            assert_eq!(final_turn_counts(&sink.receipts), (1, 1));
            if hook_error {
                assert!(matches!(result, Err(AsideError(error)) if error == "yield-after-cancel"));
            } else {
                assert_eq!(result.unwrap().end, RunEnd::Aborted);
            }
            assert_eq!(
                sink.receipts
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|event| matches!(event, AgentEvent::AgentEnd { .. }))
                    .count(),
                usize::from(!hook_error),
                "a failed hook cannot fabricate AgentEnd"
            );
        }
    }

    #[tokio::test]
    async fn aside_pending_ordinary_ownership_budget_followup_failure_and_soft_finally_family() {
        struct OwnershipHooks {
            steer: AgentMessage,
            asides: Arc<Asides>,
            polls: AtomicUsize,
            refuse_follow: bool,
        }
        #[async_trait]
        impl LoopHooks for OwnershipHooks {
            fn context_admission_enabled(&self) -> bool {
                true
            }
            async fn aside_messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
                self.asides.messages().await
            }
            async fn poll_queued_raw_messages_with_context(
                &self,
                kind: QueueInputKind,
                _: ExecutionSnapshot,
                _: Vec<AgentMessage>,
                _: &CancellationToken,
                _: Option<std::time::Instant>,
            ) -> Result<Vec<AgentMessage>, AdmissionError> {
                if kind == QueueInputKind::FollowUp && self.refuse_follow {
                    return Err(AdmissionError::Blocked("follow-up refused".into()));
                }
                if kind == QueueInputKind::Steering {
                    let n = self.polls.fetch_add(1, Ordering::SeqCst);
                    if n == if self.refuse_follow { 2 } else { 1 } {
                        return Ok(vec![self.steer.clone()]);
                    }
                }
                Ok(Vec::new())
            }
        }
        let effects = Arc::new(Mutex::new(Vec::new()));
        for refuse_follow in [false, true] {
            let visits = Arc::new(Mutex::new(Vec::new()));
            let aside = claim("pending", &visits, false, false);
            let steer = raw("owned steering");
            let hooks = Arc::new(OwnershipHooks {
                steer: steer.clone(),
                asides: Arc::new(Asides(Mutex::new(vec![aside.clone().into()]), AtomicUsize::new(0))),
                polls: AtomicUsize::new(0),
                refuse_follow,
            });
            let provider = Provider::new(vec![turn(if refuse_follow {
                Vec::new()
            } else {
                vec![call("budget-tool", json!({"text":"x"}))]
            })]);
            let mut cfg = config(provider, tool(&effects), hooks);
            cfg.max_model_calls = Some(1);
            let mut context = Vec::new();
            let report = bounded(try_agent_loop_raw(
                vec![raw("prompt")],
                &mut context,
                &cfg,
                &CancellationToken::new(),
                &Receipts::default(),
            ))
            .await
            .unwrap();
            assert_eq!(report.end, if refuse_follow { RunEnd::Error } else { RunEnd::ModelCallBudget });
            assert!(context.iter().any(|message| message.same_record(&steer)), "dequeued ordinary input survives");
            assert!(!context.iter().any(|message| message.same_record(&aside)));
            assert_eq!(*visits.lock().unwrap(), vec!["discard:pending"]);
        }
        struct SoftAside {
            state: SoftToolRequirementState,
            req: SoftToolRequirement,
            asides: Arc<Asides>,
        }
        #[async_trait]
        impl LoopHooks for SoftAside {
            fn soft_tool_requirement_state(&self) -> Option<SoftToolRequirementState> {
                Some(self.state.clone())
            }
            fn select_tool_choice_directive_full(
                &self,
                _: &[Arc<dyn AgentTool>],
                _: Option<&ToolChoice>,
            ) -> Result<ToolChoiceDirectiveSelection, ToolDirectiveError> {
                Ok(ToolChoiceDirectiveSelection {
                    directive: Some(ToolChoiceDirective::Soft(self.req.clone())),
                    ..Default::default()
                })
            }
            async fn aside_messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
                self.asides.messages().await
            }
        }
        let visits = Arc::new(Mutex::new(Vec::new()));
        let asides =
            Arc::new(Asides(Mutex::new(vec![claim("soft-discard", &visits, false, true).into()]), AtomicUsize::new(0)));
        let hooks = Arc::new(SoftAside {
            state: SoftToolRequirementState::default(),
            req: SoftToolRequirement {
                id: "retained-soft".into(),
                tool_name: "echo".into(),
                satisfies: None,
                reminder: Vec::new(),
            },
            asides,
        });
        let provider = Provider::new(vec![turn(Vec::new())]);
        let mut cfg = config(provider, tool(&effects), hooks.clone());
        cfg.max_model_calls = Some(1);
        let receipts = Receipts::default();
        let error = bounded(try_agent_loop_raw(
            vec![raw("force once")],
            &mut Vec::new(),
            &cfg,
            &CancellationToken::new(),
            &receipts,
        ))
        .await
        .unwrap_err();
        assert_eq!(error.0, "discard-soft-discard");
        assert!(
            receipts.events.lock().unwrap().iter().any(|event| matches!(event, AgentEvent::AgentEnd { .. })),
            "Native already emitted end before its finally throws"
        );
        assert_eq!(
            hooks.state.snapshot(),
            SoftToolRequirementSnapshot {
                id: Some("retained-soft".into()),
                forced_tool_choice: Some(ToolChoice::Tool("echo".into())),
                escalations: 1
            }
        );
        let provider = Provider::new(vec![turn(Vec::new()), turn(Vec::new()), turn(Vec::new())]);
        let mut cfg = config(provider.clone(), tool(&effects), hooks.clone());
        cfg.max_model_calls = Some(5);
        let report = bounded(try_agent_loop_raw(
            vec![raw("continue same owner")],
            &mut Vec::new(),
            &cfg,
            &CancellationToken::new(),
            &Receipts::default(),
        ))
        .await
        .unwrap();
        assert_eq!(report.end, RunEnd::Error, "retained escalation count still reaches the Native cap");
        assert_eq!(provider.contexts.lock().unwrap().len(), 3);
        assert!(
            provider.tool_choices.lock().unwrap().iter().all(|choice| *choice == Some(ToolChoice::Tool("echo".into())))
        );
        assert_eq!(
            hooks.state.snapshot(),
            SoftToolRequirementSnapshot::default(),
            "normal following failure still resets"
        );
    }
}
