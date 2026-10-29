//! Live host catalogue boundaries (fixed OMP `agent.ts` context sync and
//! `agent-loop.ts` prepared tool dispatch). These are controlled in-process
//! provider tests through the real Agent and standalone execution entrances.

use ara_agent::*;
use ara_ai::event::EventSink;
use ara_ai::*;
use async_trait::async_trait;
use serde_json::json;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Gate {
    started: Notify,
    release: Notify,
}

impl Gate {
    async fn wait(&self, cancel: &CancellationToken) -> bool {
        self.started.notify_one();
        tokio::select! {
            _ = self.release.notified() => true,
            _ = cancel.cancelled() => false,
        }
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future).await.expect("fixture must make bounded progress")
}

struct Turn {
    calls: Vec<ToolCall>,
    stream_gate: Option<Arc<Gate>>,
}

fn turn(calls: &[(&str, &str)]) -> Turn {
    Turn { calls: calls.iter().map(|(id, version)| call(id, version)).collect(), stream_gate: None }
}

fn call(id: &str, version: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "versioned".into(),
        arguments: json!({"version": version}).as_object().unwrap().clone(),
        thought_signature: None,
        ..Default::default()
    }
}

fn assistant(calls: &[(&str, &str)]) -> AssistantMessage {
    let mut message = AssistantMessage::empty("openai-completions", "fixture", "live-tools");
    message.content = calls.iter().map(|(id, version)| AssistantBlock::ToolCall(call(id, version))).collect();
    message.stop_reason = StopReason::ToolUse;
    message
}

type CallOptionsSnapshot = (Option<ToolChoice>, Option<u64>, Option<f64>);

struct Provider {
    turns: Mutex<VecDeque<Turn>>,
    contexts: Mutex<Vec<Context>>,
    models: Mutex<Vec<String>>,
    options: Mutex<Vec<CallOptionsSnapshot>>,
}

impl Provider {
    fn new(turns: Vec<Turn>) -> Arc<Self> {
        Arc::new(Self {
            turns: Mutex::new(turns.into()),
            contexts: Mutex::new(Vec::new()),
            models: Mutex::new(Vec::new()),
            options: Mutex::new(Vec::new()),
        })
    }
}

impl ModelProvider for Provider {
    fn stream(&self, model: &Model, context: &Context, options: CallOptions) -> AssistantStream {
        self.contexts.lock().unwrap().push(context.clone());
        self.models.lock().unwrap().push(model.id.clone());
        self.options.lock().unwrap().push((options.tool_choice.clone(), options.max_tokens, options.temperature));
        let turn = self.turns.lock().unwrap().pop_front().expect("unexpected model call");
        let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
        let (sink, stream) = EventSink::channel();
        tokio::spawn(async move {
            sink.push(AssistantMessageEvent::Start { partial: message.clone() }).await;
            if let Some(gate) = turn.stream_gate
                && !gate.wait(&options.cancel).await
            {
                message.stop_reason = StopReason::Aborted;
                sink.push(AssistantMessageEvent::Error { reason: StopReason::Aborted, error: message }).await;
                return;
            }
            for tool_call in turn.calls {
                message.content.push(AssistantBlock::ToolCall(tool_call.clone()));
                sink.push(AssistantMessageEvent::ToolcallEnd {
                    content_index: message.content.len() - 1,
                    tool_call,
                    partial: message.clone(),
                })
                .await;
            }
            let reason = if message.content.is_empty() { StopReason::Stop } else { StopReason::ToolUse };
            message.stop_reason = reason;
            sink.push(AssistantMessageEvent::Done { reason, message }).await;
        });
        stream
    }
}

struct VersionedTool {
    version: &'static str,
    effects: Arc<Mutex<Vec<String>>>,
    gate: Option<Arc<Gate>>,
}

#[async_trait]
impl AgentTool for VersionedTool {
    fn definition(&self) -> Tool {
        Tool {
            name: "versioned".into(),
            description: format!("Adapter {}", self.version),
            parameters: json!({"type":"object","properties":{"version":{"const":self.version}},
                "required":["version"],"additionalProperties":false}),
            ..Default::default()
        }
    }

    fn concurrency(&self, _args: &JsonObject) -> Concurrency {
        Concurrency::Exclusive
    }

    async fn execute(
        &self,
        id: &str,
        _args: JsonObject,
        cancel: CancellationToken,
        _update: UpdateFn,
    ) -> Result<ToolOutput, ToolError> {
        self.effects.lock().unwrap().push(format!("{}:{id}", self.version));
        if id == "old-1"
            && let Some(gate) = &self.gate
            && !gate.wait(&cancel).await
        {
            return Err(ToolError("fixture cancelled".into()));
        }
        Ok(ToolOutput::text(format!("{}:{id}", self.version)).with_details(json!({"adapter":self.version})))
    }
}

fn tool(version: &'static str, effects: &Arc<Mutex<Vec<String>>>, gate: Option<Arc<Gate>>) -> Arc<dyn AgentTool> {
    Arc::new(VersionedTool { version, effects: effects.clone(), gate })
}

fn snapshot(version: &'static str, effects: &Arc<Mutex<Vec<String>>>, gate: Option<Arc<Gate>>) -> ExecutionSnapshot {
    ExecutionSnapshot {
        tools: vec![tool(version, effects, gate)],
        system_prompt: vec![format!("prompt {version}")],
        model: None,
        provider_context: None,
    }
}

struct LiveHooks {
    live: Mutex<ExecutionSnapshot>,
    snapshots: AtomicUsize,
    transforms: AtomicUsize,
    approvals: Mutex<Vec<String>>,
    model_admissions: AtomicUsize,
    admission_gates: Mutex<VecDeque<Arc<Gate>>>,
    admitted: Mutex<Option<ExecutionSnapshot>>,
    transform_gates: Mutex<VecDeque<Arc<Gate>>>,
}

impl LiveHooks {
    fn new(live: ExecutionSnapshot) -> Arc<Self> {
        Arc::new(Self {
            live: Mutex::new(live),
            snapshots: AtomicUsize::new(0),
            transforms: AtomicUsize::new(0),
            approvals: Mutex::new(Vec::new()),
            model_admissions: AtomicUsize::new(0),
            admission_gates: Mutex::new(VecDeque::new()),
            admitted: Mutex::new(None),
            transform_gates: Mutex::new(VecDeque::new()),
        })
    }

    fn replace(&self, live: ExecutionSnapshot) {
        *self.live.lock().unwrap() = live;
    }
}

#[async_trait]
impl LoopHooks for LiveHooks {
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.snapshots.fetch_add(1, Ordering::SeqCst);
        Some(self.live.lock().unwrap().clone())
    }

    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        self.model_admissions.fetch_add(1, Ordering::SeqCst);
        let gate = self.admission_gates.lock().unwrap().pop_front();
        if let Some(gate) = gate
            && !gate.wait(cancel).await
        {
            return Err(AdmissionError::Cancelled);
        }
        Ok(self.admitted.lock().unwrap().take().unwrap_or(snapshot))
    }

    async fn before_tool_call(&self, call: &ToolCall, _args: &JsonObject, _cancel: &CancellationToken) -> ToolDecision {
        self.approvals.lock().unwrap().push(call.id.clone());
        if call.id == "blocked" { ToolDecision::Block(Some("host denied".into())) } else { ToolDecision::Allow }
    }

    async fn transform_provider_context(&self, mut context: Context, _model: &Model) -> Context {
        self.transforms.fetch_add(1, Ordering::SeqCst);
        let gate = self.transform_gates.lock().unwrap().pop_front();
        if let Some(gate) = gate {
            gate.started.notify_one();
            gate.release.notified().await;
        }
        context.system_prompt.push("transformed".into());
        context
    }
}

fn config(provider: Arc<Provider>, effects: &Arc<Mutex<Vec<String>>>, hooks: Arc<dyn LoopHooks>) -> AgentConfig {
    AgentConfig {
        model: Model {
            id: "live-tools".into(),
            api: "openai-completions".into(),
            provider: "fixture".into(),
            base_url: String::new(),
            reasoning: false,
            max_tokens: None,
            context_window: None,
            tokenizer: None,
        },
        provider,
        system_prompt: vec!["original prompt".into()],
        tools: vec![tool("original", effects, None)],
        tool_choice: None,
        max_tokens: None,
        temperature: None,
        deadline: None,
        max_model_calls: None,
        hooks,
        interrupt_mode: ara_agent::InterruptMode::Immediate,
    }
}

fn assert_live_context(context: &Context, version: &str) {
    assert_eq!(context.system_prompt, [format!("prompt {version}"), "transformed".into()]);
    let tools = context.tools.as_ref().unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].parameters["properties"]["version"]["const"], version);
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
        .expect("recorded tool receipt")
}

#[tokio::test]
async fn admitted_owned_route_survives_later_live_change_and_refreshes_after_tool_batch() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider_a = Provider::new(vec![]);
    let provider_b = Provider::new(vec![turn(&[("b-1", "B"), ("b-2", "B")])]);
    let provider_c = Provider::new(vec![turn(&[])]);
    let route = |version: &'static str, provider: Arc<Provider>, max_tokens| {
        let mut config = config(provider, &effects, Arc::new(NoHooks));
        config.model.id = format!("model {version}");
        config.tools = vec![tool(version, &effects, None)];
        config.system_prompt = vec![format!("prompt {version}")];
        config.tool_choice = Some(ToolChoice::Required);
        config.max_tokens = Some(max_tokens);
        config.temperature = Some(0.25);
        ExecutionSnapshot::from_config(&config)
    };
    let hooks = LiveHooks::new(route("A", provider_a.clone(), 100));
    let admission_gate = Arc::new(Gate::default());
    let transform_gate = Arc::new(Gate::default());
    hooks.admission_gates.lock().unwrap().push_back(admission_gate.clone());
    hooks.transform_gates.lock().unwrap().push_back(transform_gate.clone());
    let agent = Agent::new(config(provider_a.clone(), &effects, hooks.clone()), Vec::new());
    let running = tokio::spawn(async move {
        agent.prompt(vec![Message::User(UserMessage::text("run"))], CancellationToken::new(), Arc::new(NullSink)).await
    });
    bounded(admission_gate.started.notified()).await;
    // The Host admits a new route after the asynchronous check, replacing the
    // stale candidate captured by Core. That returned ownership is authoritative.
    let admitted = route("B", provider_b.clone(), 200);
    hooks.replace(admitted.clone());
    *hooks.admitted.lock().unwrap() = Some(admitted);
    admission_gate.release.notify_one();
    bounded(transform_gate.started.notified()).await;
    hooks.replace(route("C", provider_c.clone(), 300));
    transform_gate.release.notify_one();
    let report = bounded(running).await.unwrap().unwrap();
    assert_eq!(report.end, RunEnd::Completed);
    assert!(provider_a.contexts.lock().unwrap().is_empty());
    assert_eq!(*provider_b.models.lock().unwrap(), ["model B"]);
    assert_eq!(*provider_c.models.lock().unwrap(), ["model C"]);
    assert_eq!(*provider_b.options.lock().unwrap(), [(Some(ToolChoice::Required), Some(200), Some(0.25))]);
    assert_eq!(*provider_c.options.lock().unwrap(), [(Some(ToolChoice::Required), Some(300), Some(0.25))]);
    assert_live_context(&provider_b.contexts.lock().unwrap()[0], "B");
    assert_live_context(&provider_c.contexts.lock().unwrap()[0], "C");
    assert_eq!(*effects.lock().unwrap(), ["B:b-1", "B:b-2"]);
    for id in ["b-1", "b-2"] {
        assert!(!result(&report, id).is_error);
        assert_eq!(result(&report, id).details.as_ref().unwrap()["adapter"], "B");
    }
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 2, "no unchecked snapshot reread after admission");
    assert_eq!(hooks.model_admissions.load(Ordering::SeqCst), 2, "tool continuation readmits once");
}

#[tokio::test]
async fn cancellation_during_transform_stops_admitted_provider_dispatch() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![]);
    let hooks = LiveHooks::new(snapshot("A", &effects, None));
    let gate = Arc::new(Gate::default());
    hooks.transform_gates.lock().unwrap().push_back(gate.clone());
    let cancel = CancellationToken::new();
    let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), Vec::new());
    let running = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            agent.prompt(vec![Message::User(UserMessage::text("run"))], cancel, Arc::new(NullSink)).await
        })
    };
    bounded(gate.started.notified()).await;
    cancel.cancel();
    gate.release.notify_one();
    assert_eq!(bounded(running).await.unwrap().unwrap().end, RunEnd::Aborted);
    assert!(provider.contexts.lock().unwrap().is_empty());
    assert_eq!(hooks.model_admissions.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn stream_replacement_uses_same_snapshot_for_response_then_refreshes_next_call() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Gate::default());
    let mut first = turn(&[("a-1", "A"), ("blocked", "A")]);
    first.stream_gate = Some(gate.clone());
    let provider = Provider::new(vec![first, turn(&[("b-1", "B")]), turn(&[])]);
    let hooks = LiveHooks::new(snapshot("A", &effects, None));
    let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), Vec::new());
    let running = tokio::spawn(async move {
        agent.prompt(vec![Message::User(UserMessage::text("run"))], CancellationToken::new(), Arc::new(NullSink)).await
    });
    bounded(gate.started.notified()).await;
    hooks.replace(snapshot("B", &effects, None));
    gate.release.notify_one();
    let report = bounded(running).await.unwrap().unwrap();

    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(*effects.lock().unwrap(), ["A:a-1", "B:b-1"]);
    assert_eq!(*hooks.approvals.lock().unwrap(), ["a-1", "blocked", "b-1"]);
    assert!(result(&report, "blocked").is_error);
    assert!(!result(&report, "a-1").is_error);
    assert_eq!(result(&report, "a-1").details.as_ref().unwrap()["adapter"], "A");
    assert_eq!(result(&report, "b-1").details.as_ref().unwrap()["adapter"], "B");
    let contexts = provider.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 3);
    assert_live_context(&contexts[0], "A");
    assert_live_context(&contexts[1], "B");
    assert_live_context(&contexts[2], "B");
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), contexts.len(), "one read per model call");
    assert_eq!(hooks.transforms.load(Ordering::SeqCst), contexts.len(), "QueueHooks retains transform callback");
}

#[tokio::test]
async fn pending_and_later_prepared_calls_keep_old_adapter_after_catalogue_replacement() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let gate = Arc::new(Gate::default());
    let provider = Provider::new(vec![turn(&[("old-1", "A"), ("old-2", "A")]), turn(&[("new-1", "B")]), turn(&[])]);
    let hooks = LiveHooks::new(snapshot("A", &effects, Some(gate.clone())));
    let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), Vec::new());
    let running = tokio::spawn(async move {
        agent.prompt(vec![Message::User(UserMessage::text("run"))], CancellationToken::new(), Arc::new(NullSink)).await
    });
    bounded(gate.started.notified()).await;
    assert_eq!(*effects.lock().unwrap(), ["A:old-1"]);
    assert_eq!(*hooks.approvals.lock().unwrap(), ["old-1", "old-2"], "whole batch was prepared before execution");
    hooks.replace(snapshot("B", &effects, None));
    gate.release.notify_one();
    let report = bounded(running).await.unwrap().unwrap();

    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(*effects.lock().unwrap(), ["A:old-1", "A:old-2", "B:new-1"]);
    for id in ["old-1", "old-2"] {
        assert!(!result(&report, id).is_error);
        assert_eq!(result(&report, id).details.as_ref().unwrap()["adapter"], "A");
    }
    assert_eq!(result(&report, "new-1").details.as_ref().unwrap()["adapter"], "B");
    let contexts = provider.contexts.lock().unwrap();
    assert_live_context(&contexts[0], "A");
    assert_live_context(&contexts[1], "B");
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn standalone_tool_execution_reads_fresh_snapshot_once_without_provider_call() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(Vec::new());
    let hooks = LiveHooks::new(snapshot("A", &effects, None));
    let config = config(provider.clone(), &effects, hooks.clone());
    let cancel = CancellationToken::new();
    let first = bounded(execute_tool_calls(&assistant(&[("first", "A")]), &config, &cancel, &NullSink)).await;
    hooks.replace(snapshot("B", &effects, None));
    let second = bounded(execute_tool_calls(&assistant(&[("second", "B")]), &config, &cancel, &NullSink)).await;

    assert_eq!(*effects.lock().unwrap(), ["A:first", "B:second"]);
    assert!(!first[0].is_error && !second[0].is_error);
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 2);
    assert_eq!(hooks.transforms.load(Ordering::SeqCst), 0);
    assert!(provider.contexts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn known_unexecuted_tail_reads_live_snapshot_before_preparation() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(Vec::new());
    let hooks = LiveHooks::new(snapshot("B", &effects, None));
    let mut config = config(provider.clone(), &effects, hooks.clone());
    config.max_model_calls = Some(0);
    let mut messages: Vec<AgentMessage> = vec![Message::Assistant(assistant(&[("never-run", "B")])).into()];
    let report = bounded(agent_loop_continue(
        &mut messages,
        &config,
        &CancellationToken::new(),
        &NullSink,
        UnpairedTail::Execute,
    ))
    .await
    .unwrap();

    assert_eq!(report.end, RunEnd::ModelCallBudget);
    assert_eq!(*effects.lock().unwrap(), ["B:never-run"]);
    assert!(!result(&report, "never-run").is_error);
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 1);
    assert!(provider.contexts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn empty_live_snapshot_clears_original_tools_and_prompt() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![turn(&[("disabled", "original")]), turn(&[])]);
    let hooks = LiveHooks::new(ExecutionSnapshot {
        tools: Vec::new(),
        system_prompt: Vec::new(),
        model: None,
        provider_context: None,
    });
    let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), Vec::new());
    let report = bounded(agent.prompt(
        vec![Message::User(UserMessage::text("run"))],
        CancellationToken::new(),
        Arc::new(NullSink),
    ))
    .await
    .unwrap();

    assert_eq!(report.end, RunEnd::Completed);
    assert!(effects.lock().unwrap().is_empty());
    assert!(result(&report, "disabled").is_error);
    assert!(hooks.approvals.lock().unwrap().is_empty(), "unknown tools never reach permission gate");
    for context in provider.contexts.lock().unwrap().iter() {
        assert_eq!(context.tools, Some(Vec::new()));
        assert_eq!(context.system_prompt, ["transformed"]);
    }
    assert_eq!(hooks.snapshots.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn default_none_snapshot_preserves_original_agent_configuration() {
    let effects = Arc::new(Mutex::new(Vec::new()));
    let provider = Provider::new(vec![turn(&[("fallback", "original")]), turn(&[])]);
    let agent = Agent::new(config(provider.clone(), &effects, Arc::new(NoHooks)), Vec::new());
    let report = bounded(agent.prompt(
        vec![Message::User(UserMessage::text("run"))],
        CancellationToken::new(),
        Arc::new(NullSink),
    ))
    .await
    .unwrap();

    assert_eq!(report.end, RunEnd::Completed);
    assert_eq!(*effects.lock().unwrap(), ["original:fallback"]);
    assert!(!result(&report, "fallback").is_error);
    let contexts = provider.contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    for context in contexts.iter() {
        assert_eq!(context.system_prompt, ["original prompt"]);
        let tools = context.tools.as_ref().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].parameters["properties"]["version"]["const"], "original");
    }
}

/// The Host receives the exact owned live context through QueueHooks while
/// Agent holds its transcript lock. Pending input is absent until consumed;
/// tool results are present before the next model admission.
#[tokio::test]
async fn owned_admission_context_matches_each_actual_call_and_preserves_queue_ownership() {
    struct ContextHooks {
        queues: Mutex<Vec<Vec<Message>>>,
        calls: Mutex<Vec<Vec<Message>>>,
        block_queue: bool,
    }
    #[async_trait]
    impl LoopHooks for ContextHooks {
        fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
            Some(false)
        }
        fn context_admission_enabled(&self) -> bool {
            true
        }
        async fn before_queued_input_dequeue_with_context(
            &self,
            _snapshot: ExecutionSnapshot,
            messages: Vec<Message>,
            _cancel: &CancellationToken,
        ) -> Result<(), AdmissionError> {
            self.queues.lock().unwrap().push(messages);
            if self.block_queue { Err(AdmissionError::Blocked("reserve fixture blocked".into())) } else { Ok(()) }
        }
        async fn before_model_call_with_context(
            &self,
            snapshot: ExecutionSnapshot,
            messages: Vec<Message>,
            _cancel: &CancellationToken,
        ) -> Result<ExecutionSnapshot, AdmissionError> {
            self.calls.lock().unwrap().push(messages);
            tokio::task::yield_now().await;
            Ok(snapshot)
        }
    }
    for blocked in [false, true] {
        let effects = Arc::new(Mutex::new(Vec::new()));
        let provider = Provider::new(if blocked { vec![] } else { vec![turn(&[("owned", "original")]), turn(&[])] });
        let hooks = Arc::new(ContextHooks {
            queues: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            block_queue: blocked,
        });
        let initial = Message::User(UserMessage::text("initial live context"));
        let queued = Message::User(UserMessage::text("not owned until dequeue"));
        let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), vec![initial.clone()]);
        agent.steer(queued.clone());
        let result = bounded(agent.continue_run(CancellationToken::new(), Arc::new(NullSink))).await;
        assert_eq!(hooks.queues.lock().unwrap()[0], vec![initial.clone()]);
        if blocked {
            // The existing User tail resumes an admitted loop, whose refusal
            // emits an error receipt; an idle pre-dequeue refusal returns Err.
            let report = result.unwrap();
            assert_eq!(report.end, RunEnd::Error);
            assert_eq!(report.messages.len(), 1);
            let model_messages = report.model_messages().unwrap();
            let terminal = model_messages[0].as_assistant().unwrap();
            assert_eq!(terminal.stop_reason, StopReason::Error);
            assert_eq!(terminal.error_message.as_deref(), Some("reserve fixture blocked"));
            let evidence = terminal.failure_evidence.as_ref().unwrap();
            assert_eq!(evidence.kind, ara_ai::retry_classification::ProviderErrorKind::Config);
            assert!(evidence.replay_blocked && evidence.same_route_blocked);
            assert_eq!(agent.queued_counts(), (1, 0));
            assert_eq!(agent.peek_steering_inputs()[0].model, queued);
            assert!(provider.contexts.lock().unwrap().is_empty());
            assert!(effects.lock().unwrap().is_empty());
            assert!(!agent.is_busy());
            assert_eq!(agent.messages().await.unwrap(), vec![initial, model_messages[0].clone()]);
        } else {
            assert_eq!(result.unwrap().end, RunEnd::Completed);
            assert_eq!(hooks.queues.lock().unwrap().len(), 1, "empty and skipped polls must not run usage admission");
            assert_eq!(agent.queued_counts(), (0, 0));
            let calls = hooks.calls.lock().unwrap();
            let wire = provider.contexts.lock().unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls[0], vec![initial, queued]);
            assert_eq!(calls[0], wire[0].messages);
            assert_eq!(calls[1], wire[1].messages);
            assert!(
                calls[1]
                    .iter()
                    .any(|message| matches!(message, Message::ToolResult(result) if result.tool_call_id == "owned"))
            );
        }
    }
}

#[tokio::test]
async fn owned_empty_poll_preserves_late_inputs_until_their_own_admission_family() {
    struct EmptyPollHooks {
        empty_poll: Arc<Gate>,
        first: AtomicUsize,
        admitted: AtomicUsize,
    }
    #[async_trait]
    impl LoopHooks for EmptyPollHooks {
        fn context_admission_enabled(&self) -> bool {
            true
        }
        fn queued_input_pending(&self, _kind: QueueInputKind) -> Option<bool> {
            Some(false)
        }
        async fn before_queued_input_dequeue(&self, cancel: &CancellationToken) -> Result<(), AdmissionError> {
            if self.first.fetch_add(1, Ordering::AcqRel) == 0 && !self.empty_poll.wait(cancel).await {
                return Err(AdmissionError::Cancelled);
            }
            Ok(())
        }
        async fn before_queued_input_dequeue_with_context(
            &self,
            _snapshot: ExecutionSnapshot,
            _messages: Vec<Message>,
            _cancel: &CancellationToken,
        ) -> Result<(), AdmissionError> {
            self.admitted.fetch_add(1, Ordering::AcqRel);
            Err(AdmissionError::Blocked("late input must receive its own admission".into()))
        }
    }
    for kind in [QueueInputKind::Steering, QueueInputKind::FollowUp] {
        for mode in [QueueMode::OneAtATime, QueueMode::All] {
            let effects = Arc::new(Mutex::new(Vec::new()));
            let provider = Provider::new(vec![turn(&[])]);
            let hooks = Arc::new(EmptyPollHooks {
                empty_poll: Arc::new(Gate::default()),
                first: AtomicUsize::new(0),
                admitted: AtomicUsize::new(0),
            });
            let initial = Message::User(UserMessage::text("initial input"));
            let late = Message::User(UserMessage::text("late input is still queue owned"));
            let agent = Agent::new(config(provider.clone(), &effects, hooks.clone()), vec![]);
            agent.set_steering_mode(mode);
            agent.set_follow_up_mode(mode);
            let (owner, input) = (agent.clone(), initial.clone());
            let task =
                tokio::spawn(
                    async move { owner.prompt(vec![input], CancellationToken::new(), Arc::new(NullSink)).await },
                );
            bounded(hooks.empty_poll.started.notified()).await;
            match kind {
                QueueInputKind::Steering => agent.steer(late),
                QueueInputKind::FollowUp => agent.follow_up(late),
            }
            hooks.empty_poll.release.notify_one();
            assert_eq!(bounded(task).await.unwrap().unwrap().end, RunEnd::Error);
            assert_eq!(hooks.admitted.load(Ordering::Acquire), 1);
            assert_eq!(provider.contexts.lock().unwrap()[0].messages, vec![initial]);
            assert_eq!(agent.queued_counts(), if kind == QueueInputKind::Steering { (1, 0) } else { (0, 1) });
        }
    }
}
