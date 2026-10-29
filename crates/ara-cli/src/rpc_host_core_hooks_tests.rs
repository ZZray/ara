//! One grouped actual Host flow through RpcHooks -> ToolLoopHooks -> QueueHooks.
//! Child process/journal restart is covered in rpc_host integration tests.
use super::*;
use ara_agent::{
    AfterToolCallContext, AfterToolCallResult, BeforeToolCallContext, BeforeToolCallResult, IrcInterruptPeek,
    RunStopCause, SteeringInterruptBinding, SteeringQueuePeek, SteeringQueueState, TerminalToolResultHandle,
    ToolExecutionContext, ToolHookError, TurnEndContext,
};
use ara_ai::{AssistantBlock, CallOptions, Context, JsonObject, Model, ModelProvider, Tool, ToolCall};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct HostPreviewTool(Arc<Mutex<Vec<Value>>>);
struct HostPreviewReceiver(ara_agent::ToolArgStreamInit);
impl ara_agent::ToolArgStream for HostPreviewReceiver {
    fn push(&mut self, delta: &str) -> std::result::Result<(), ara_agent::ToolError> {
        (self.0.emit)(json!({"phase":"push","delta":delta,"providerPayload":"preview business field"}));
        Ok(())
    }
    fn end(&mut self, _: &JsonObject) -> std::result::Result<(), ara_agent::ToolError> {
        panic!("the preview-only HTTP failure must never end its arguments")
    }
    fn cancel(&mut self) -> std::result::Result<(), ara_agent::ToolError> {
        (self.0.emit)(json!({"phase":"cancel"}));
        Ok(())
    }
}
#[async_trait]
impl ara_agent::AgentTool for HostPreviewTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "preview".into(),
            description: "existing Host preview receiver".into(),
            parameters: json!({"type":"object"}),
            ..Default::default()
        }
    }
    fn open_arg_stream(
        &self,
        init: ara_agent::ToolArgStreamInit,
    ) -> std::result::Result<Option<Box<dyn ara_agent::ToolArgStream>>, ara_agent::ToolError> {
        self.0.lock().unwrap().push(json!({"id":init.tool_call_id,"name":init.tool_name,"wire":init.custom_wire_name}));
        (init.emit)(json!({"phase":"open"}));
        Ok(Some(Box::new(HostPreviewReceiver(init))))
    }
    async fn execute(
        &self,
        _: &str,
        _: JsonObject,
        _: CancellationToken,
        _: ara_agent::UpdateFn,
    ) -> std::result::Result<ara_agent::ToolOutput, ara_agent::ToolError> {
        self.0.lock().unwrap().push(json!({"executed":true}));
        panic!("argument previews have no execution authority")
    }
}
struct HostPreviewProvider;
impl ModelProvider for HostPreviewProvider {
    fn stream(&self, model: &Model, _: &Context, _: CallOptions) -> ara_ai::AssistantStream {
        let mut partial = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        let (sink, stream) = ara_ai::event::EventSink::channel();
        tokio::spawn(async move {
            sink.push(ara_ai::AssistantMessageEvent::Start { partial: partial.clone() }).await;
            partial.content.push(AssistantBlock::ToolCall(ToolCall {
                id: "host-preview-call".into(),
                name: "preview".into(),
                ..Default::default()
            }));
            sink.push(ara_ai::AssistantMessageEvent::ToolcallStart { content_index: 0, partial: partial.clone() })
                .await;
            sink.push(ara_ai::AssistantMessageEvent::ToolcallDelta {
                content_index: 0,
                delta: "{\"unfinished\":".into(),
                partial: partial.clone(),
            })
            .await;
            partial.stop_reason = ara_ai::StopReason::Error;
            partial.error_message = Some("preview-only upstream failure".into());
            sink.push(ara_ai::AssistantMessageEvent::Error { reason: ara_ai::StopReason::Error, error: partial }).await;
        });
        stream
    }
}

#[tokio::test]
async fn actual_host_argument_preview_frames_do_not_become_session_receipts_group() {
    let (mut host, mut output) = super::tests::fixture();
    let observations = Arc::new(Mutex::new(Vec::new()));
    host.config.provider = Arc::new(HostPreviewProvider);
    host.config.tools = vec![Arc::new(HostPreviewTool(observations.clone()))];
    host.config.max_model_calls = Some(1);
    host.agent = Agent::new(host.config.clone(), Vec::new());
    host.publish_snapshot(&host.config);
    let original_entries = host.session.journal.lock().await.entries().len();
    host.start(
        Some(Message::User(ara_ai::UserMessage::text("actual Host preview")).into()),
        Command::new(wire(json!({"id":"preview-run","type":"prompt"}))),
    )
    .await
    .unwrap();
    let mut active = host.active.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
    let report = result.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap();
    assert_eq!(report.end, RunEnd::Error);
    assert_eq!(report.messages.len(), 2);
    assert!(report.model_messages().unwrap().last().unwrap().as_assistant().unwrap().usage.is_unknown());
    assert!(!report.model_messages().unwrap().iter().any(|message| matches!(message, Message::ToolResult(_))));
    assert_eq!(*observations.lock().unwrap(), vec![json!({"id":"host-preview-call","name":"preview","wire":null})]);
    host.completed(active, result).await;
    let frames: Vec<Value> = std::iter::from_fn(|| output.try_recv().ok())
        .map(|item| match item {
            OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => {
                serde_json::from_str(&frame.stringify()).unwrap()
            }
        })
        .collect();
    let previews: Vec<_> = frames.iter().filter(|frame| frame["type"] == "tool_stream_update").collect();
    assert_eq!(previews.len(), 3);
    assert_eq!(
        previews[0],
        &json!({"type":"tool_stream_update","toolCallId":"host-preview-call","toolName":"preview","update":{"phase":"open"}})
    );
    assert_eq!(
        previews[1],
        &json!({"type":"tool_stream_update","toolCallId":"host-preview-call","toolName":"preview","update":{"phase":"push","delta":"{\"unfinished\":","providerPayload":"preview business field"}})
    );
    assert_eq!(previews[2]["update"], json!({"phase":"cancel"}));
    let preview_position = frames
        .iter()
        .position(|frame| frame["type"] == "tool_stream_update" && frame["update"]["phase"] == "push")
        .unwrap();
    let delta_position = frames
        .iter()
        .position(|frame| {
            frame["type"] == "message_update" && frame["assistantMessageEvent"]["type"] == "toolcall_delta"
        })
        .unwrap();
    assert!(preview_position < delta_position);
    assert!(
        !frames.iter().any(|frame| frame["type"] == "tool_execution_start" || frame["type"] == "tool_execution_end")
    );
    let journal = host.session.journal.lock().await;
    assert_eq!(
        journal.entries().len(),
        original_entries + 2,
        "only actual User and failed Assistant records are journaled"
    );
    let public = Session::public_messages(&journal).unwrap();
    assert_eq!(public.len(), 2);
    assert!(!json!(public).to_string().contains("preview business field"));
    assert!(!journal.entries().iter().any(|entry| entry.raw.to_string().contains("tool_stream_update")));
    assert_eq!(host.session.messages.lock().unwrap().len(), 2);
    assert!(host.session.persistence_error.lock().unwrap().is_none());
    assert!(!host.connection.is_cancelled());
}

// The actual wrapper stack must forward passive hooks without converting the
// claimed canonical record, and only settled bodies belong to the Host journal.
mod pause_aside_host_port {
    use super::*;
    use ara_agent::{AsideCallbacks, AsideError, AsideMessage};

    struct PassiveHostHooks {
        pending: Mutex<Vec<AsideMessage>>,
        visits: Arc<Mutex<Vec<&'static str>>>,
    }
    #[async_trait]
    impl LoopHooks for PassiveHostHooks {
        async fn aside_messages(&self) -> std::result::Result<Vec<AsideMessage>, AsideError> {
            self.visits.lock().unwrap().push("aside-poll");
            Ok(std::mem::take(&mut *self.pending.lock().unwrap()))
        }
        async fn before_yield(&self) -> std::result::Result<(), AsideError> {
            self.visits.lock().unwrap().push("before-yield");
            Ok(())
        }
    }

    #[tokio::test]
    async fn actual_host_passive_hooks_claim_current_raw_body_and_persistent_reopen_group() {
        let (mut host, mut output) = super::super::tests::fixture();
        let dir = tempfile::tempdir().unwrap();
        let journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
        let path = journal.path().to_path_buf();
        let header = journal.header().clone();
        host.session = Session::new(Some(journal), header.clone(), &[], None, None).unwrap();
        let visits = Arc::new(Mutex::new(Vec::new()));
        let record = AgentMessage::from_raw(
            json!({"role":"user","content":"passive current body","timestamp":23,"application":{"aside":"kept"}}),
            Some(Arc::new(json!({"externalSource":"passive-host-fixture"}))),
        )
        .unwrap();
        let commit_visits = visits.clone();
        let discard_visits = visits.clone();
        record.set_aside_callbacks(AsideCallbacks {
            commit: Some(Arc::new(move || {
                commit_visits.lock().unwrap().push("commit");
                Ok(())
            })),
            discard: Some(Arc::new(move |_| {
                discard_visits.lock().unwrap().push("discard");
                Ok(())
            })),
        });
        let retained = record.clone();
        let hooks = Arc::new(PassiveHostHooks {
            pending: Mutex::new(vec![AsideMessage::Lazy(Arc::new(move || Ok(Some(record.clone()))))]),
            visits: visits.clone(),
        });
        let provider = Arc::new(RawHostProvider(Mutex::new(Vec::new())));
        host.config.provider = provider.clone();
        host.config.hooks = hooks;
        host.config.max_model_calls = Some(3);
        host.agent = Agent::new_raw(host.config.clone(), Vec::new()).unwrap();
        host.publish_snapshot(&host.config);
        host.start(
            Some(Message::User(ara_ai::UserMessage::text("ordinary prompt")).into()),
            Command::new(wire(json!({"id":"passive-host","type":"prompt"}))),
        )
        .await
        .unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        assert!(matches!(&result, Ok(Ok(Some(report))) if report.end == RunEnd::Completed));
        host.completed(active, result).await;
        assert_eq!(*visits.lock().unwrap(), ["before-yield", "aside-poll", "commit", "before-yield", "aside-poll"]);
        {
            let contexts = provider.0.lock().unwrap();
            assert_eq!(contexts.len(), 2);
            assert!(contexts[1].messages.iter().any(
                |message| matches!(message, Message::User(user) if user.content.plain_text() == "passive current body")
            ));
        }
        assert!(host.agent.raw_messages().await.iter().any(|raw| raw.same_record(&retained)));
        assert_eq!(host.agent.queued_counts(), (0, 0));
        let mut frames = Vec::new();
        while let Ok(item) = output.try_recv() {
            if let OutputItem::Frame(frame) = item {
                frames.push(serde_json::from_str::<Value>(&frame.stringify()).unwrap());
            }
        }
        let message_frames: Vec<_> =
            frames.iter().filter(|frame| frame["message"]["application"]["aside"] == "kept").collect();
        assert_eq!(message_frames.len(), 2);
        assert_eq!(message_frames[0]["type"], "message_start");
        assert_eq!(message_frames[1]["type"], "message_end");
        assert_eq!(message_frames[1]["message"], retained.json().unwrap());
        assert!(!host.connection.is_cancelled());
        assert!(host.session.persistence_error.lock().unwrap().is_none());
        let journal = host.session.journal.lock().await;
        let settled: Vec<_> = Session::public_messages(&journal)
            .unwrap()
            .into_iter()
            .filter(|body| body["application"]["aside"] == "kept")
            .collect();
        assert_eq!(settled, [retained.json().unwrap()]);
        assert_eq!(
            journal
                .entries()
                .iter()
                .filter(|entry| entry
                    .agent_message_body()
                    .ok()
                    .flatten()
                    .is_some_and(|body| body.checked_legacy().unwrap()["application"]["aside"] == "kept"))
                .count(),
            1
        );
        drop(journal);
        let reopened = SessionJournal::open(&path).unwrap();
        let session = Session::new(Some(reopened), header, &[], None, None).unwrap();
        assert_eq!(
            session
                .messages
                .lock()
                .unwrap()
                .iter()
                .filter(|body| body.get("application").and_then(|application| application.get("aside"))
                    .and_then(WireValue::as_string).is_some_and(|aside| aside.equals_ascii("kept")))
                .map(|body| ara_session::native_checked_serde(body).expect("original aside fixture public cache is scalar JSON"))
                .collect::<Vec<_>>(),
            settled
        );
        assert_eq!(serde_json::to_value(&retained).unwrap(), retained.json().unwrap());
        assert!(!format!("{retained:?}").contains("callbacks"));
        assert_eq!(visits.lock().unwrap().iter().filter(|visit| **visit == "discard").count(), 0);
    }
}

struct OneTurn(AtomicUsize);
impl ModelProvider for OneTurn {
    fn stream(&self, model: &Model, _: &Context, _: CallOptions) -> ara_ai::AssistantStream {
        assert_eq!(self.0.fetch_add(1, Ordering::SeqCst), 0, "terminal result must prevent next call");
        let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        message.content.push(AssistantBlock::ToolCall(ToolCall {
            id: "terminal-call".into(),
            name: "fallback".into(),
            arguments: json!({"v":1}).as_object().unwrap().clone(),
            ..Default::default()
        }));
        message.stop_reason = ara_ai::StopReason::ToolUse;
        let (sink, stream) = ara_ai::event::EventSink::channel();
        tokio::spawn(async move {
            sink.push(ara_ai::AssistantMessageEvent::Start { partial: message.clone() }).await;
            sink.push(ara_ai::AssistantMessageEvent::Done { reason: message.stop_reason, message }).await;
        });
        stream
    }
}

struct Echo(Arc<Mutex<Vec<Value>>>);
#[async_trait]
impl ara_agent::AgentTool for Echo {
    fn definition(&self) -> Tool {
        Tool {
            name: "echo".into(),
            description: "host hook fixture".into(),
            parameters: json!({"type":"object","properties":{"v":{"type":"integer"}},"required":["v"]}),
            ..Default::default()
        }
    }
    fn intent_mode(&self) -> ara_agent::ToolIntentMode {
        ara_agent::ToolIntentMode::Omit
    }
    async fn execute(
        &self,
        _: &str,
        _: JsonObject,
        _: CancellationToken,
        _: ara_agent::UpdateFn,
    ) -> Result<ara_agent::ToolOutput, ara_agent::ToolError> {
        panic!("context forwarding must use execute_with_context")
    }
    async fn execute_with_context(
        &self,
        id: &str,
        args: JsonObject,
        cancel: CancellationToken,
        _: ara_agent::UpdateFn,
        context: ToolExecutionContext,
    ) -> Result<ara_agent::ToolOutput, ara_agent::ToolError> {
        assert_eq!(id, "terminal-call");
        assert_eq!(args["v"], 5);
        assert!(!cancel.is_cancelled());
        assert_eq!(context.total, 1);
        assert_eq!(context.index, 0);
        assert_eq!(context.tool_calls[0].id, id);
        assert!(!context.steering_signal.is_cancelled());
        self.0.lock().unwrap().push(Value::Object(args));
        Ok(ara_agent::ToolOutput::text("actual settled output"))
    }
}

struct EmptyPeek;
#[async_trait]
impl SteeringQueuePeek for EmptyPeek {
    async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
        Ok(Default::default())
    }
}
#[async_trait]
impl IrcInterruptPeek for EmptyPeek {
    async fn queued(&self) -> Result<bool, ToolHookError> {
        Ok(false)
    }
}

struct RichHooks {
    tool: Arc<Echo>,
    trace: Mutex<Vec<&'static str>>,
    terminal: Mutex<Option<TerminalToolResultHandle>>,
    peek: Arc<EmptyPeek>,
}
#[async_trait]
impl LoopHooks for RichHooks {
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        _: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        self.trace.lock().unwrap().push("transform");
        let AssistantBlock::ToolCall(call) = &mut message.content[0] else { panic!("call") };
        call.arguments["v"] = json!(2);
        Ok(())
    }
    fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn ara_agent::AgentTool>> {
        assert_eq!(name, "fallback");
        Some(self.tool.clone())
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut BeforeToolCallContext<'_>,
        _: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        self.trace.lock().unwrap().push("before");
        assert_eq!(context.args["v"], 2);
        context.args["v"] = json!(3);
        Ok(BeforeToolCallResult { args: Some(json!({"v":4}).as_object().unwrap().clone()), ..Default::default() })
    }
    async fn before_tool_call(&self, _: &ToolCall, args: &JsonObject, _: &CancellationToken) -> ToolDecision {
        self.trace.lock().unwrap().push("guard");
        assert_eq!(args["v"], 4);
        ToolDecision::Allow
    }
    fn transform_tool_call_arguments(&self, mut args: JsonObject, _: &str) -> Result<JsonObject, ToolHookError> {
        self.trace.lock().unwrap().push("late");
        assert_eq!(args["v"], 4);
        args["v"] = json!(5);
        Ok(args)
    }
    async fn after_tool_call(
        &self,
        context: AfterToolCallContext<'_>,
        _: &CancellationToken,
    ) -> Result<Option<AfterToolCallResult>, ToolHookError> {
        self.trace.lock().unwrap().push("after");
        assert_eq!(context.args["v"], 5);
        assert!(!context.is_error);
        context.result.details = Some(json!({"mutated":true}));
        context.result.useless = true;
        assert!(context.terminal.request_terminal_result());
        *self.terminal.lock().unwrap() = Some(context.terminal);
        Ok(None)
    }
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        Some(SteeringInterruptBinding::new(self.peek.clone(), None))
    }
    fn irc_interrupt_port(&self) -> Option<Arc<dyn IrcInterruptPeek>> {
        Some(self.peek.clone())
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.trace.lock().unwrap().push("turn-end");
        assert!(!cancel.is_cancelled());
        assert!(!context.will_continue);
        assert_eq!(
            context.stop_cause,
            Some(&RunStopCause::TerminalToolResult { tool_call_id: "terminal-call".into() })
        );
        assert_eq!(context.tool_results[0].details, Some(json!({"mutated":true})));
        Vec::new()
    }
    async fn turn_end_inputs(
        &self,
        _: &AssistantMessage,
        _: &[ara_ai::ToolResultMessage],
        _: &CancellationToken,
    ) -> Vec<AgentInput> {
        panic!("context must reach the base hook")
    }
}

#[tokio::test]
async fn actual_host_wrappers_final_args_terminal_receipt_and_successor_group() {
    let (mut host, mut output) = super::tests::fixture();
    let effects = Arc::new(Mutex::new(Vec::new()));
    let rich = Arc::new(RichHooks {
        tool: Arc::new(Echo(effects.clone())),
        trace: Mutex::new(Vec::new()),
        terminal: Mutex::new(None),
        peek: Arc::new(EmptyPeek),
    });
    let provider = Arc::new(OneTurn(AtomicUsize::new(0)));
    host.config.provider = provider.clone();
    host.config.max_model_calls = Some(2);
    host.config.hooks = rich.clone();
    host.agent = Agent::new(host.config.clone(), Vec::new());
    host.publish_snapshot(&host.config);
    host.start(
        Some(Message::User(ara_ai::UserMessage::text("actual Host hooks")).into()),
        Command::new(wire(json!({"id":"hook-run","type":"prompt"}))),
    )
    .await
    .unwrap();
    let mut active = host.active.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
    let report = result.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap();
    assert_eq!(report.end, RunEnd::TerminalToolResult);
    assert_eq!(report.stop_cause, Some(RunStopCause::TerminalToolResult { tool_call_id: "terminal-call".into() }));
    assert_eq!(*effects.lock().unwrap(), vec![json!({"v":5})]);
    assert_eq!(*rich.trace.lock().unwrap(), vec!["transform", "before", "guard", "late", "after", "turn-end"]);
    assert!(!rich.terminal.lock().unwrap().as_ref().unwrap().request_terminal_result(), "settled handle is revoked");
    host.completed(active, result).await;
    let frames: Vec<Value> = std::iter::from_fn(|| output.try_recv().ok())
        .map(|item| match item {
            OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => {
                serde_json::from_str(&frame.stringify()).unwrap()
            }
        })
        .collect();
    let terminal = frames.iter().find(|frame| frame["type"] == "agent_end").unwrap();
    assert_eq!(terminal["stopCause"], json!({"type":"terminalToolResult","toolCallId":"terminal-call"}));
    let result = frames.iter().find(|frame| frame["type"] == "tool_execution_end").unwrap();
    assert_eq!(result["result"]["useless"], true);
    assert_eq!(result["result"]["details"], json!({"mutated":true}));
    assert!(!frames.iter().any(|frame| frame["success"] == false));
    assert!(!host.connection.is_cancelled());
    assert_eq!(provider.0.load(Ordering::SeqCst), 1);
}

struct HostFinalGate {
    stop: AtomicBool,
    calls: AtomicUsize,
}
#[async_trait]
impl ara_agent::FinalProviderGate for HostFinalGate {
    async fn before_model_call(
        &self,
        context: &mut Context,
        _: &CancellationToken,
    ) -> std::result::Result<Option<ara_agent::FinalProviderGateStop>, ara_agent::FinalProviderGateError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        context.system_prompt.push("host final request mutation".into());
        Ok(self
            .stop
            .load(Ordering::SeqCst)
            .then(|| ara_agent::FinalProviderGateStop { reason: Some("host gate refusal".into()) }))
    }
}
struct HostGateHooks {
    selector_calls: AtomicUsize,
    binding: ara_agent::FinalProviderGateBinding,
    getter_calls: AtomicUsize,
    rejected: AtomicUsize,
}
#[async_trait]
impl LoopHooks for HostGateHooks {
    fn final_provider_gate_binding(&self) -> Option<ara_agent::FinalProviderGateBinding> {
        self.getter_calls.fetch_add(1, Ordering::SeqCst);
        Some(self.binding.clone())
    }
    fn on_final_provider_gate_rejected(&self) -> std::result::Result<(), ara_agent::FinalProviderGateError> {
        self.rejected.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn select_tool_choice_directive(
        &self,
        _: &[Arc<dyn ara_agent::AgentTool>],
        _: Option<&ara_ai::ToolChoice>,
    ) -> std::result::Result<ara_agent::ToolChoiceSelection, ara_agent::ToolDirectiveError> {
        let first = self.selector_calls.fetch_add(1, Ordering::SeqCst) == 0;
        Ok(ara_agent::ToolChoiceSelection {
            directive: first.then_some(ara_ai::ToolChoice::Required),
            static_fallback: None,
        })
    }
}
struct HostGateProvider(AtomicUsize);
impl ModelProvider for HostGateProvider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> ara_ai::AssistantStream {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert_eq!(context.system_prompt.last().unwrap(), "host final request mutation");
        assert_eq!(options.tool_choice, Some(ara_ai::ToolChoice::Required));
        let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        message.stop_reason = ara_ai::StopReason::Stop;
        let (sink, stream) = ara_ai::event::EventSink::channel();
        tokio::spawn(async move {
            sink.push(ara_ai::AssistantMessageEvent::Start { partial: message.clone() }).await;
            sink.push(ara_ai::AssistantMessageEvent::Done { reason: message.stop_reason, message }).await;
        });
        stream
    }
}

#[tokio::test]
async fn actual_host_final_gate_request_stop_terminal_and_input_group() {
    for stop in [false, true] {
        let (mut host, mut output) = super::tests::fixture();
        let gate = Arc::new(HostFinalGate { stop: AtomicBool::new(stop), calls: AtomicUsize::new(0) });
        let binding = ara_agent::FinalProviderGateBinding::default();
        binding.set_primary(Some(gate.clone()));
        let hooks = Arc::new(HostGateHooks {
            selector_calls: AtomicUsize::new(0),
            binding,
            getter_calls: AtomicUsize::new(0),
            rejected: AtomicUsize::new(0),
        });
        let provider = Arc::new(HostGateProvider(AtomicUsize::new(0)));
        host.config.provider = provider.clone();
        host.config.max_model_calls = Some(2);
        host.config.hooks = hooks.clone();
        host.agent = Agent::new(host.config.clone(), Vec::new());
        host.publish_snapshot(&host.config);
        host.start(
            Some(Message::User(ara_ai::UserMessage::text("actual Host final gate")).into()),
            Command::new(wire(json!({"id":"gate-run","type":"prompt"}))),
        )
        .await
        .unwrap();
        let mut active = host.active.take().unwrap();
        let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
        let report = result.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap();
        assert_eq!(report.end, if stop { RunEnd::PreModelStop } else { RunEnd::Completed });
        assert!(
            matches!(report.messages.first().and_then(|raw| raw.checked_model_snapshot().unwrap()), Some(Message::User(message)) if message.content.plain_text() == "actual Host final gate")
        );
        if stop {
            assert_eq!(report.messages.len(), 1, "closed gate stop invents no Assistant");
            assert_eq!(
                report.stop_cause,
                Some(RunStopCause::PreModelStop { reason: Some("host gate refusal".into()) })
            );
        }
        assert_eq!(hooks.getter_calls.load(Ordering::SeqCst), 1, "Agent adopts the base owner once");
        assert_eq!(hooks.rejected.load(Ordering::SeqCst), usize::from(stop));
        assert_eq!(gate.calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.0.load(Ordering::SeqCst), usize::from(!stop));
        host.completed(active, result).await;
        let frames: Vec<Value> = std::iter::from_fn(|| output.try_recv().ok())
            .map(|item| match item {
                OutputItem::Frame(frame) | OutputItem::Negotiate(frame) => {
                    serde_json::from_str(&frame.stringify()).unwrap()
                }
            })
            .collect();
        let terminal = frames.iter().find(|frame| frame["type"] == "agent_end").unwrap();
        assert_eq!(frames.iter().filter(|frame| frame["type"] == "turn_start").count(), usize::from(!stop));
        assert_eq!(frames.iter().filter(|frame| frame["type"] == "turn_end").count(), usize::from(!stop));
        assert!(frames.iter().any(|frame| frame["type"] == "message_end" && frame["message"]["role"] == "user"));
        if stop {
            assert_eq!(terminal["stopCause"], json!({"type":"preModelStop","reason":"host gate refusal"}));
            assert!(frames.iter().any(|frame| {
                frame["success"] == false
                    && frame["error"].as_str().is_some_and(|error| error.contains("host gate refusal"))
            }));
            assert!(!host.auto_compaction_pending);
            assert!(!host.drain_queues);
        }
        assert!(!host.connection.is_cancelled());
        assert_eq!(hooks.selector_calls.load(Ordering::SeqCst), 1);
        if stop {
            // The same Host/Agent must serve the retained hard claim without
            // sampling a new source after its initial provider gate refusal.
            let same_agent = host.agent.clone();
            let same_session = host.session.header["id"].clone();
            gate.stop.store(false, Ordering::SeqCst);
            host.start(
                Some(Message::User(ara_ai::UserMessage::text("resume retained directive")).into()),
                Command::new(wire(json!({"id":"directive-resume","type":"prompt"}))),
            )
            .await
            .unwrap();
            assert!(Arc::ptr_eq(&same_agent, &host.agent));
            assert_eq!(host.session.header["id"], same_session);
            let mut active = host.active.take().unwrap();
            let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
            let report = result.as_ref().unwrap().as_ref().unwrap().as_ref().unwrap();
            assert_eq!(report.end, RunEnd::Completed);
            assert_eq!(hooks.selector_calls.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.getter_calls.load(Ordering::SeqCst), 1);
            assert_eq!(hooks.rejected.load(Ordering::SeqCst), 1);
            assert_eq!(gate.calls.load(Ordering::SeqCst), 2);
            assert_eq!(provider.0.load(Ordering::SeqCst), 1);
            host.completed(active, result).await;
        }
    }
}

// Append to the existing rpc_host_core_hooks_tests.rs target (@bin:ara).
// Fixed OMP 596f2da; grouped raw Host/cache/native-source/rollback observations.

#[test]
fn raw_host_cache_alias_derived_neighbor_and_owner_rewrite_group() {
    use ara_cli::agent_message_projection::AgentMessageProjector;
    let (host, _) = super::tests::fixture();
    let model = host.config.model.clone();
    let projector = AgentMessageProjector::default();
    let user =
        AgentMessage::from_raw(json!({"role":"user","content":"user-A","timestamp":1,"application":{"v":1}}), None)
            .unwrap();
    let mut a = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    a.content.push(AssistantBlock::text("assistant-A"));
    let assistant = AgentMessage::from(Message::Assistant(a));
    let records = vec![user.clone(), assistant.clone()];
    let first = projector.convert_current(&records, &model).unwrap();
    assert!(matches!(&first[0],Message::User(value) if value.content.plain_text()=="user-A"));
    let mut user_body = user.json().unwrap();
    user_body["content"] = json!("user-B");
    user.replace_body(ara_agent::AgentMessageBody::RawJson(user_body)).unwrap();
    let mut assistant_body = assistant.json().unwrap();
    assistant_body["content"] = json!([{"type":"text","text":"assistant-B"}]);
    assistant.replace_body(ara_agent::AgentMessageBody::RawJson(assistant_body)).unwrap();
    let second = projector.convert_current(&records, &model).unwrap();
    assert!(
        matches!(&second[0],Message::User(value) if value.content.plain_text()=="user-A"),
        "derived snapshot remains memoized until owner invalidation"
    );
    assert_eq!(second[1].as_assistant().unwrap().text(), "assistant-B", "normal Assistant cache aliases CURRENT body");
    assert!(
        matches!(&projector.estimate_current(&records,&model).unwrap()[0],Message::User(value) if value.content.plain_text()=="user-B"),
        "Host estimator is independent of ordinary memoized fragments"
    );
    user.invalidate_derived();
    assert!(
        matches!(&projector.convert_current(&records,&model).unwrap()[0],Message::User(value) if value.content.plain_text()=="user-B")
    );

    let mut interrupted = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    interrupted.stop_reason = ara_ai::StopReason::Aborted;
    interrupted.content.push(AssistantBlock::Thinking(ara_ai::ThinkingContent {
        thinking: "  unsigned  ".into(),
        thinking_signature: Some(String::new()),
    }));
    let mut body = serde_json::to_value(Message::Assistant(interrupted)).unwrap();
    body["errorId"] = json!(0x0400_1000_u32); // Native UserInterrupt | Class
    let aborted = AgentMessage::from_raw(body, None).unwrap();
    assert!(
        projector.convert_current(std::slice::from_ref(&aborted), &model).unwrap().is_empty(),
        "raw imported UserInterrupt bit strips empty-signature unsigned thinking"
    );
    let marker=AgentMessage::from_raw(json!({"role":"custom","customType":"interrupted-thinking","content":"continuity","display":false,"timestamp":2}),None).unwrap();
    let with_neighbor = projector.convert_current(&[aborted.clone(), marker], &model).unwrap();
    assert!(
        with_neighbor.iter().any(|value| matches!(value,Message::Assistant(a) if a.content.is_empty())),
        "neighbor flip recomputes demoted fragment"
    );

    let mut journal = SessionJournal::in_memory(
        json!({"type":"session","version":3,"id":"raw-owner","cwd":".","timestamp":"2026-10-05T00:00:00Z"}),
    )
    .unwrap();
    let original = AgentMessage::from_raw(
        json!({"role":"user","content":"owned-A","timestamp":10,"application":{"v":1}}),
        Some(Arc::new(json!({"id":"foreign-provenance"}))),
    )
    .unwrap();
    let id = projector.append_current_body(&mut journal, &original).unwrap();
    assert!(
        matches!(&projector.convert_current(std::slice::from_ref(&original), &model).unwrap()[0],Message::User(value) if value.content.plain_text()=="owned-A")
    );
    assert_eq!(
        projector.entry_id(&original).as_deref(),
        Some(id.as_str()),
        "current Host journal ID overrides immutable external provenance"
    );
    let selected = projector.raw_context(&journal, &model, false, false).unwrap();
    let mut owner_body = selected[0].json().unwrap();
    owner_body["content"] = json!("owned-B");
    owner_body["application"]["v"] = json!(2);
    selected[0].replace_body(ara_agent::AgentMessageBody::RawJson(owner_body)).unwrap();
    let owned = projector.reconcile_owned_context(std::slice::from_ref(&original), selected).unwrap();
    assert!(owned[0].same_record(&original));
    assert_eq!(owned[0].json().unwrap()["application"]["v"], 2);
    assert!(
        matches!(&projector.convert_current(&owned,&model).unwrap()[0],Message::User(value) if value.content.plain_text()=="owned-B")
    );
    let weak = assistant.weak_identity();
    drop(records);
    drop(assistant);
    assert!(weak.upgrade().is_none(), "weak conversion cache must not retain canonical Assistant records");
}

struct RawHostProvider(Mutex<Vec<Context>>);
impl ModelProvider for RawHostProvider {
    fn stream(&self, model: &Model, context: &Context, _: CallOptions) -> ara_ai::AssistantStream {
        self.0.lock().unwrap().push(context.clone());
        let mut answer = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        answer.content.push(AssistantBlock::text("settled raw Host"));
        let (sink, stream) = ara_ai::event::EventSink::channel();
        tokio::spawn(async move {
            sink.push(ara_ai::AssistantMessageEvent::Done { reason: answer.stop_reason, message: answer }).await;
        });
        stream
    }
}
struct RawHostTransform(AtomicUsize);
#[async_trait]
impl LoopHooks for RawHostTransform {
    async fn transform_agent_context(
        &self,
        live: &mut Vec<AgentMessage>,
        _: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, ara_agent::RawContextError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let raw =
            live.iter().find(|raw| raw.json().unwrap()["customType"] == ara_session::SKILL_PROMPT_CUSTOM_TYPE).unwrap();
        assert_eq!(raw.json().unwrap()["content"], "CURRENT skill body");
        assert_eq!(raw.json().unwrap()["application"]["edited"], true);
        Ok(None)
    }
    async fn convert_agent_messages_to_llm(
        &self,
        _: &[AgentMessage],
    ) -> Result<Vec<Message>, ara_agent::RawContextError> {
        panic!("the separate Host estimator must not invoke a programmable Core converter")
    }
}

#[tokio::test]
async fn actual_raw_skill_guard_bash_journal_group() {
    let (mut host, _) = super::tests::fixture();
    let provider = Arc::new(RawHostProvider(Mutex::new(Vec::new())));
    let hooks = Arc::new(RawHostTransform(AtomicUsize::new(0)));
    host.config.provider = provider.clone();
    host.config.hooks = hooks.clone();
    host.config.max_model_calls = Some(1);
    host.agent = Agent::new_raw(host.config.clone(), Vec::new()).unwrap();
    host.publish_snapshot(&host.config);
    let skill = skill_input(UserSkillPrompt::new(
        UserContent::Text("ORIGINAL skill body".into()),
        Some(json!({"originalText":"/skill:proof"})),
    ));
    // The Host persists MessageEnd before the Core context transform. Editing
    // ingress beforehand proves both owners read CURRENT without asserting a
    // later context transform retroactively rewrites an already settled row.
    let mut skill_body = skill.json().unwrap();
    skill_body["content"] = json!("CURRENT skill body");
    skill_body["application"] = json!({"edited":true});
    skill.replace_body(ara_agent::AgentMessageBody::RawJson(skill_body)).unwrap();
    let retained = skill.clone();
    host.start(Some(skill), Command::new(wire(json!({"id":"raw-host","type":"prompt"})))).await.unwrap();
    let mut active = host.active.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
    host.completed(active, result).await;
    assert_eq!(hooks.0.load(Ordering::SeqCst), 1);
    assert!(
        provider.0.lock().unwrap()[0]
            .messages
            .iter()
            .any(|m| matches!(m,Message::User(u) if u.content.plain_text()=="CURRENT skill body"))
    );
    assert!(host.agent.raw_messages().await.iter().any(|raw| raw.same_record(&retained)));
    let journal = host.session.journal.lock().await;
    let row = journal
        .entries()
        .iter()
        .find(|entry| entry.raw["customType"] == ara_session::SKILL_PROMPT_CUSTOM_TYPE)
        .unwrap();
    assert_eq!(row.agent_message_body().unwrap().unwrap().checked_legacy().unwrap()["application"]["edited"], true);
    assert_eq!(
        Session::public_messages(&journal)
            .unwrap()
            .iter()
            .find(|body| body["customType"] == ara_session::SKILL_PROMPT_CUSTOM_TYPE)
            .unwrap()["application"]["edited"],
        true
    );
    drop(journal);
    let notice = LoopGuardNotice::thinking_loop();
    let mut guard_body = notice.event_message();
    guard_body["application"] = json!({"guard":"CURRENT"});
    let guard = ara_cli::agent_message_projection::raw_custom_input(guard_body).unwrap();
    let retained_guard = guard.clone();
    host.start(Some(guard), Command::new(wire(json!({"id":"raw-guard","type":"prompt"})))).await.unwrap();
    let mut active = host.active.take().unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), &mut active.task).await.unwrap();
    host.completed(active, result).await;
    assert_eq!(hooks.0.load(Ordering::SeqCst), 2);
    assert!(host.agent.raw_messages().await.iter().any(|raw| raw.same_record(&retained_guard)));
    let Message::Developer(projected_guard) = notice.model_message() else { panic!("guard Developer projection") };
    let guard_text = projected_guard.content.plain_text();
    assert!(
        provider.0.lock().unwrap()[1].messages.iter().any(|message| {
            matches!(message, Message::Developer(value) if value.content.plain_text() == guard_text)
        })
    );
    let journal = host.session.journal.lock().await;
    assert!(journal.entries().iter().any(|entry| {
        entry
            .agent_message_body()
            .unwrap()
            .is_some_and(|body| body.checked_legacy().unwrap()["application"]["guard"] == "CURRENT")
    }));
    assert!(Session::public_messages(&journal).unwrap().iter().any(|body| body["application"]["guard"] == "CURRENT"));
    drop(journal);
    host.append_bash(PendingBash {
        target: host.bash_target.clone(),
        message: BashExecutionMessage {
            command: "fixture".into(),
            output: "known command receipt".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            meta: Some(json!({"owned":true})),
            timestamp: 20,
            exclude_from_context: Some(true),
        },
    })
    .await
    .unwrap();
    let current = host.agent.raw_messages().await;
    assert_eq!(current.last().unwrap().json().unwrap()["role"], "bashExecution");
    assert_eq!(
        host.budget_messages(&current).unwrap().len(),
        4,
        "excluded Bash is raw-only and contributes no provider fragment"
    );
}

#[test]
fn checked_raw_native_origin_spoof_current_body_final_route_and_images_group() {
    use ara_cli::agent_message_projection::AgentMessageProjector;
    let dir = tempfile::tempdir().unwrap();
    let mut journal = SessionJournal::create(dir.path(), dir.path()).unwrap();
    let model = Model {
        id: "raw-native".into(),
        provider: "openai".into(),
        api: "openai-responses".into(),
        base_url: "http://fixture.invalid/v1".into(),
        reasoning: false,
        max_tokens: None,
        context_window: None,
        tokenizer: None,
    };
    let first_user = journal.append_message(&Message::User(ara_ai::UserMessage::text("SOURCE ALPHA"))).unwrap();
    let mut source = AssistantMessage::empty(&model.api, &model.provider, &model.id);
    source.content.push(AssistantBlock::text("SOURCE BETA"));
    let first_assistant = journal.append_message(&Message::Assistant(source.clone())).unwrap();
    let boundary = journal.append_model_change("openai/raw-native").unwrap();
    journal.append_message(&Message::User(ara_ai::UserMessage::text("RECENT"))).unwrap();
    let replay_through = journal.append_message(&Message::Assistant(source)).unwrap();
    let snapshot = journal.native_remote_snapshot(&model, true).unwrap();
    let image = json!({"type":"input_image","image_url":"data:image/png;base64,iVBORw0KGgo="});
    let summary = ara_session::NativeRemoteSummary {
        summary: "readable compact summary".into(),
        short_summary: None,
        preserve_data: json!({"openaiRemoteCompaction":{"version":"v2","provider":"openai","replacementHistory":[{"type":"compaction","encrypted_content":"opaque-checked"},{"type":"message","role":"user","content":[{"type":"input_text","text":"RECENT"},image]}]}}),
        read_files: Vec::new(),
        modified_files: Vec::new(),
    };
    journal
        .commit_native_entry_remote(
            &snapshot,
            &summary,
            &boundary,
            &[first_user, first_assistant],
            &replay_through,
            100,
        )
        .unwrap();
    let projector = AgentMessageProjector::default();
    let settings = ara_agent::remote::RemoteSettings::default();
    let actual =
        ara_cli::remote_compaction::route_raw_context(&journal, &projector, &model, None, &settings, true).unwrap();
    projector.refresh_checked(&journal, &actual, &model).unwrap();
    let native = projector.convert_current(&actual, &model).unwrap();
    assert!(native.iter().any(|value|matches!(value,Message::User(user) if user.provider_payload.is_some() && user.provider_payload_origin.is_some())));
    let mut changed = model.clone();
    changed.provider = "other-provider".into();
    assert!(
        projector
            .convert_current(&actual, &changed)
            .unwrap()
            .iter()
            .all(|value| { !matches!(value,Message::User(user) if user.provider_payload.is_some()) }),
        "a previously checked actual Session record loses native replay on final route change"
    );
    projector.refresh_checked(&journal, &actual, &model).unwrap();
    assert!(
        projector
            .convert_current(&actual, &model)
            .unwrap()
            .iter()
            .any(|value| { matches!(value, Message::User(user) if user.provider_payload.is_some()) })
    );
    let selected = actual.iter().find(|raw| raw.json().unwrap()["role"] == "compactionSummary").unwrap();
    let forged = AgentMessage::from_raw(selected.json().unwrap(), selected.source()).unwrap();
    projector.refresh_checked(&journal, std::slice::from_ref(&forged), &model).unwrap();
    let ordinary = projector.convert_current(std::slice::from_ref(&forged), &model).unwrap();
    assert!(
        ordinary.iter().all(|value| !matches!(value,Message::User(user) if user.provider_payload.is_some())),
        "equal public source.id/body/digest cannot register the private Host origin"
    );
    let mut edited = selected.json().unwrap();
    edited["summary"] = json!("CURRENT edited summary");
    selected.replace_body(ara_agent::AgentMessageBody::RawJson(edited)).unwrap();
    projector.refresh_checked(&journal, &actual, &model).unwrap();
    assert!(
        projector
            .convert_current(&actual, &model)
            .unwrap()
            .iter()
            .all(|value| !matches!(value,Message::User(user) if user.provider_payload.is_some())),
        "editing canonical summary revokes source equality"
    );
    let no_images =
        ara_cli::remote_compaction::route_raw_context(&journal, &projector, &model, None, &settings, false).unwrap();
    projector.refresh_checked(&journal, &no_images, &model).unwrap();
    let fallback = projector.convert_current(&no_images, &model).unwrap();
    assert!(
        fallback.iter().all(|value| !matches!(value,Message::User(user) if user.provider_payload.is_some())),
        "actual final route image capability vetoes native replay"
    );
    assert!(
        fallback
            .iter()
            .any(|value| matches!(value,Message::User(user) if user.content.plain_text().contains("SOURCE ALPHA"))),
        "checked original window supplies readable fallback"
    );
}

// Controlled identity fixtures only. NoRuntime here does not supply production
// callback/opaque execution, which this Host milestone has not connected.
fn session_values_fixture(heap: Arc<ara_rpc::value_heap::ValueHeap>) -> Arc<SessionValues> {
    Arc::new(SessionValues::new(heap, Arc::new(ara_rpc::value_heap::NoRuntime)).unwrap())
}

struct SessionValuesHooks(Option<Arc<SessionValues>>);
#[async_trait]
impl LoopHooks for SessionValuesHooks {
    fn provider_context_binding(&self) -> Option<ara_agent::ProviderContextBinding> {
        Some(ara_agent::ProviderContextBinding { actual_values: self.0.clone(), ..Default::default() })
    }
}

#[tokio::test]
async fn retained_session_values_factory_and_adoption_failure_group() {
    let (mut host, _) = super::tests::fixture();
    // Without an injected factory this typed fixture stays typed. An explicitly
    // configured factory may prepare a new draft before publication.
    assert!(host.fresh_actual_session().unwrap().is_none());
    let current = session_values_fixture(ara_rpc::value_heap::ValueHeap::new().unwrap());
    let selected = session_values_fixture(ara_rpc::value_heap::ValueHeap::new().unwrap());
    let alias = session_values_fixture(selected.heap().clone());
    assert!(selected.same_lineage(&alias));
    assert!(!same_session_values(&Some(selected.clone()), &Some(alias.clone())));
    assert!(!same_session_values(&None, &Some(selected.clone())));
    let header = host.session.header.clone();
    host.session = Session::new(None, header.clone(), &[], Some(current.clone()), None).unwrap();
    host.config.hooks = Arc::new(SessionValuesHooks(Some(current.clone())));
    let mut journal = SessionJournal::in_memory(header.clone()).unwrap();
    journal.append_message(&Message::User(ara_ai::UserMessage::text("retained original"))).unwrap();
    let first = Session::new(Some(journal), header.clone(), &[], Some(selected.clone()), None).unwrap();
    let second = Session::new(None, header.clone(), &[], Some(alias.clone()), None).unwrap();
    let first_target =
        Arc::new(Mutex::new(BashTarget { session: first.clone(), destination: BashDestination::Current }));
    let second_target = Arc::new(Mutex::new(BashTarget { session: second, destination: BashDestination::Current }));
    let current_target =
        Arc::new(Mutex::new(BashTarget { session: host.session.clone(), destination: BashDestination::Current }));
    host.bash_targets =
        vec![Arc::downgrade(&first_target), Arc::downgrade(&second_target), Arc::downgrade(&current_target)];
    assert!(
        Arc::ptr_eq(&host.retained_session(&header, None).unwrap(), &first),
        "first retained writer wins over current"
    );
    assert!(host.retained_session(&header, Some(std::path::Path::new("different-persistence"))).is_none());
    let mut foreign_header = header.clone();
    foreign_header["id"] = json!("different-session");
    assert!(host.retained_session(&foreign_header, None).is_none());
    // Preserve the old lexical fallback without requiring either path to exist.
    let lexical = PathBuf::from("missing-session.jsonl");
    let relative = PathBuf::from("missing-dir/../missing-session.jsonl");
    let identity = Session {
        agent_context: None,
        edit_session: Mutex::new(None),
        tool_owner: Mutex::new(super::super::CliToolOwner::default()),
        values: None,
        native_session: None,
        terminal_imports: None,
        journal: tokio::sync::Mutex::new(SessionJournal::in_memory(header.clone()).unwrap()),
        projector: Arc::new(ara_cli::agent_message_projection::AgentMessageProjector::default()),
        artifacts: first.artifacts.clone(),
        messages: Mutex::new(Vec::new()),
        partial: Mutex::new(None),
        persistence_error: Mutex::new(None),
        failed_recovery_message: Mutex::new(None),
        name: Mutex::new(None),
        header: header.clone(),
        file: Some(lexical),
    };
    assert!(same_session_identity(&identity, &header, Some(&relative)));
    host.sessions.actual_values_factory = None;
    assert!(host.fresh_actual_session().is_err(), "actual fresh requires Host injection");
    for reused in [current.clone(), selected.clone(), alias.clone()] {
        host.sessions.actual_values_factory =
            Some(Arc::new(move || Ok(PreparedActualSession::FixtureValues(reused.clone()))));
        assert!(host.fresh_actual_session().is_err());
    }
    let current_alias = session_values_fixture(current.heap().clone());
    host.sessions.actual_values_factory =
        Some(Arc::new(move || Ok(PreparedActualSession::FixtureValues(current_alias.clone()))));
    assert!(host.fresh_actual_session().is_err());
    let target_for_factory = first_target.clone();
    host.sessions.actual_values_factory = Some(Arc::new(move || {
        assert!(target_for_factory.try_lock().is_ok(), "Host callback must run outside target guards");
        Ok(PreparedActualSession::FixtureValues(session_values_fixture(ara_rpc::value_heap::ValueHeap::new().unwrap())))
    }));
    let fresh = host.fresh_actual_session().unwrap().unwrap().values().clone();
    assert_ne!(fresh.heap().heap_token(), current.heap().heap_token());
    assert_ne!(fresh.heap().heap_token(), selected.heap().heap_token());
    let before_leaf = first.journal.lock().await.leaf_id().map(str::to_owned);
    let before_messages = first.messages.lock().unwrap().clone();
    let before_generation = host.sessions.provider.route.generation();
    let before_current = host.session.clone();
    // A same-heap, different Arc candidate cannot replace retained writer state.
    let candidate = |values: Option<Arc<SessionValues>>, captured: Option<Arc<SessionValues>>| {
        let mut config = host.config.clone();
        config.hooks = Arc::new(SessionValuesHooks(captured));
        PreparedSessionConfig {
            values,
            native_session: None,
            native_publication: None,
            config,
            skills: Vec::new(),
            tool_owner: super::super::CliToolOwner::default(),
            context_controller: None,
        }
    };
    let wrong_target = candidate(Some(alias.clone()), Some(alias.clone()));
    let wrong_hook = candidate(Some(selected.clone()), Some(alias));
    for invalid in [wrong_target, wrong_hook] {
        let mut replacement = SessionJournal::in_memory(header.clone()).unwrap();
        replacement.append_message(&Message::User(ara_ai::UserMessage::text("candidate replacement"))).unwrap();
        assert!(host.adopt(Some(replacement), header.clone(), Vec::new(), invalid).await.is_err());
        assert!(Arc::ptr_eq(&host.session, &before_current));
        assert_eq!(first.journal.lock().await.leaf_id().map(str::to_owned), before_leaf);
        assert_eq!(*first.messages.lock().unwrap(), before_messages);
        assert_eq!(host.sessions.provider.route.generation(), before_generation);
        assert!(matches!(&first_target.lock().unwrap().destination, BashDestination::Current));
    }
    let mut accepted_config = host.config.clone();
    accepted_config.hooks = Arc::new(SessionValuesHooks(Some(selected.clone())));
    let accepted = PreparedSessionConfig {
        values: Some(selected.clone()),
        native_session: None,
        native_publication: None,
        config: accepted_config,
        skills: Vec::new(),
        tool_owner: super::super::CliToolOwner::default(),
        context_controller: None,
    };
    host.adopt(None, header, Vec::new(), accepted).await.unwrap();
    assert!(Arc::ptr_eq(&host.session, &first));
    assert!(Arc::ptr_eq(host.session.values.as_ref().unwrap(), &selected));
    assert!(same_session_values(
        &host.session.values,
        &host.snapshot.read().unwrap().provider_context.as_ref().unwrap().actual_values
    ));
}

#[tokio::test]
async fn actual_failure_terminal_uses_existing_sink_latch_group() {
    let (host, mut output) = super::tests::fixture();
    let sink = RunSink {
        session: host.session.clone(),
        observed_usage: None,
        output: host.output.clone(),
        cancel: host.connection.child_token(),
        connection: host.connection.clone(),
        terminal: Mutex::new(None),
        messages: Mutex::new(Vec::new()),
        entries: Mutex::new(Vec::new()),
        header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
    };
    *host.session.partial.lock().unwrap() = Some(wire(json!({"role":"assistant","content":[]})));
    let failure = Arc::new(ara_agent::ActualProviderError::Unsupported("controlled actual turn-end consumer"));
    sink.emit(AgentEvent::ActualAgentEnd { messages: Vec::new(), failure }).await;
    let terminal = sink.terminal.lock().unwrap().take().unwrap();
    assert!(public_wire_is(&terminal, "type", "agent_end"));
    assert!(terminal.get("failure").and_then(WireValue::as_string).unwrap().to_utf8().unwrap().contains("controlled actual turn-end consumer"));
    assert!(host.session.partial.lock().unwrap().is_none());
    assert!(output.try_recv().is_err(), "terminal stays withheld until the owned Run settles");
    assert!(!sink.cancel.is_cancelled());
    *host.session.persistence_error.lock().unwrap() = Some("existing projection failure".into());
    sink.emit(AgentEvent::ActualAgentEnd {
        messages: Vec::new(),
        failure: Arc::new(ara_agent::ActualProviderError::Unsupported("must stay latched")),
    })
    .await;
    assert!(sink.terminal.lock().unwrap().is_none());
    assert!(output.try_recv().is_err());

    // A JSON-exportable Graph still requires the explicit native journal encoder.
    // A fresh sink lets the new failure event establish the existing latch.
    let (host, mut output) = super::tests::fixture();
    let sink = RunSink {
        session: host.session.clone(),
        observed_usage: None,
        output: host.output.clone(),
        cancel: host.connection.child_token(),
        connection: host.connection.clone(),
        terminal: Mutex::new(None),
        messages: Mutex::new(Vec::new()),
        entries: Mutex::new(Vec::new()),
        header_guard: Mutex::new(GeminiHeaderGuard::new(false, 0)),
    };
    let (before_entries, before_leaf) = {
        let journal = host.session.journal.lock().await;
        (journal.entries().len(), journal.leaf_id().map(str::to_owned))
    };
    let heap = ara_rpc::value_heap::ValueHeap::new().unwrap();
    let root = heap.import_serde(&json!({"role":"assistant","content":[],"stopReason":"stop"})).unwrap();
    let message = AgentMessage::from_graph(root, None).unwrap();
    let event = AgentEvent::ActualMessageEnd {
        message: message.clone(),
        finalization_error: Arc::new(ara_agent::ActualProviderError::Unsupported(
            "controlled final Graph admission failure",
        )),
    };
    assert!(event.full().is_ok(), "public JSON projection must reach the missing native journal encoder refusal");
    sink.emit(event).await;
    {
        let journal = host.session.journal.lock().await;
        assert_eq!(journal.entries().len(), before_entries);
        assert_eq!(journal.leaf_id().map(str::to_owned), before_leaf);
    }
    let rejection = host.session.persistence_error.lock().unwrap().clone().unwrap();
    assert!(rejection.contains("actual journal encoder is unavailable"), "{rejection}");
    assert!(sink.cancel.is_cancelled());
    assert!(host.connection.is_cancelled());
    assert!(sink.entries.lock().unwrap().is_empty());
    assert!(sink.messages.lock().unwrap().is_empty());
    assert!(host.session.messages.lock().unwrap().is_empty());
    assert!(output.try_recv().is_err(), "rejected MessageEnd must not be published");
    sink.emit(AgentEvent::ActualAgentEnd {
        messages: vec![message],
        failure: Arc::new(ara_agent::ActualProviderError::Unsupported("terminal after Graph journal rejection")),
    })
    .await;
    assert!(sink.terminal.lock().unwrap().is_none());
    assert!(output.try_recv().is_err());
    assert_eq!(host.session.persistence_error.lock().unwrap().as_deref(), Some(rejection.as_str()));
}
