//! Stateful owner for one agent transcript and its message queues.
//! OMP source: `packages/agent/src/agent.ts` at the locked upstream commit.

use crate::agent_loop::{
    AgentConfig, ExecutionSnapshot, LoopError, LoopHooks, RunReport, UnpairedTail, agent_loop_continue,
    agent_loop_inputs, unpaired_tool_call_tail,
};
use crate::event::AgentEventSink;
use crate::tool::ToolDecision;
use ara_ai::{Context, JsonObject, Message, Model, ToolCall};
use async_trait::async_trait;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

/// Model input with optional host-owned provenance. The Core carries this
/// opaque value to the input sink without interpreting it or adding it to
/// provider context, the transcript, or a Run report.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInput {
    pub model: Message,
    pub provenance: Option<Arc<serde_json::Value>>,
}

impl From<Message> for AgentInput {
    fn from(model: Message) -> Self {
        Self { model, provenance: None }
    }
}

#[derive(Debug)]
pub enum AgentError {
    Busy,
    NeedsRecovery,
    CannotContinue(LoopError),
    RunPanicked,
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Agent is already running"),
            Self::NeedsRecovery => f.write_str("Agent run failed; reload the persisted session before continuing"),
            Self::CannotContinue(e) => e.fmt(f),
            Self::RunPanicked => f.write_str("Agent run panicked; tool effects may be unknown"),
        }
    }
}

impl std::error::Error for AgentError {}

/// How many Agent-owned queued messages a dequeue returns. OMP defaults to
/// one message per turn for both steering and follow-up queues.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueMode {
    All,
    #[default]
    OneAtATime,
}

#[derive(Default)]
struct Queues {
    steering: VecDeque<AgentInput>,
    follow_up: VecDeque<AgentInput>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
}

impl Queues {
    fn take_steering(&mut self) -> Vec<AgentInput> {
        match self.steering_mode {
            QueueMode::All => self.steering.drain(..).collect(),
            QueueMode::OneAtATime => self.steering.pop_front().into_iter().collect(),
        }
    }

    fn take_follow_up(&mut self) -> Vec<AgentInput> {
        match self.follow_up_mode {
            QueueMode::All => self.follow_up.drain(..).collect(),
            QueueMode::OneAtATime => self.follow_up.pop_front().into_iter().collect(),
        }
    }
}

/// One logical Agent. Hosts decide who may enqueue and persist all messages
/// from the event sink before relying on this in-memory state for recovery.
pub struct Agent {
    config: AgentConfig,
    messages: AsyncMutex<Vec<Message>>,
    queues: Arc<Mutex<Queues>>,
    run_state: Mutex<RunState>,
}

#[derive(Default)]
struct RunState {
    busy: bool,
    needs_recovery: bool,
    active_cancel: Option<CancellationToken>,
}

struct RunningGuard(Arc<Agent>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        let mut state = self.0.run_state.lock().unwrap();
        if std::thread::panicking() {
            state.needs_recovery = true;
        }
        state.active_cancel = None;
        state.busy = false;
    }
}

impl Agent {
    pub fn new(config: AgentConfig, messages: Vec<Message>) -> Arc<Self> {
        Arc::new(Self {
            config,
            messages: AsyncMutex::new(messages),
            queues: Arc::new(Mutex::new(Queues::default())),
            run_state: Mutex::new(RunState::default()),
        })
    }

    pub fn is_busy(&self) -> bool {
        self.run_state.lock().unwrap().busy
    }

    /// This waits for an active run to finish; the event sink exposes live
    /// messages while the transcript is locked by that run.
    pub async fn messages(&self) -> Vec<Message> {
        self.messages.lock().await.clone()
    }

    /// Atomically append an externally observed model projection while idle.
    /// Hold admission's state lock through a non-awaiting transcript try_lock;
    /// a Run cannot enter between the idle check and this update.
    pub fn append_idle_message(&self, message: Message) -> Result<(), AgentError> {
        self.update_idle_messages(|messages| {
            if has_unpaired_idle_calls(messages) {
                return Err(idle_unpaired_error());
            }
            messages.push(message);
            Ok(())
        })
    }

    /// Replace only the idle transcript. Queues, modes and recovery state stay
    /// owned by this Agent. Neither the old nor new view may hide runnable tool
    /// calls without known results behind a later ordinary message.
    pub fn replace_idle_messages(&self, replacement: Vec<Message>) -> Result<(), AgentError> {
        self.update_idle_messages(|messages| {
            if has_unpaired_idle_calls(messages) || has_unpaired_idle_calls(&replacement) {
                return Err(idle_unpaired_error());
            }
            *messages = replacement;
            Ok(())
        })
    }

    fn update_idle_messages(
        &self,
        update: impl FnOnce(&mut Vec<Message>) -> Result<(), AgentError>,
    ) -> Result<(), AgentError> {
        let state = self.run_state.lock().unwrap();
        if state.needs_recovery {
            return Err(AgentError::NeedsRecovery);
        }
        if state.busy {
            return Err(AgentError::Busy);
        }
        let mut messages = self.messages.try_lock().map_err(|_| AgentError::Busy)?;
        update(&mut messages)
    }

    pub fn steer(&self, message: Message) {
        self.steer_input(message.into());
    }

    pub fn steer_input(&self, input: AgentInput) {
        self.queues.lock().unwrap().steering.push_back(input);
    }

    pub fn follow_up(&self, message: Message) {
        self.follow_up_input(message.into());
    }

    pub fn follow_up_input(&self, input: AgentInput) {
        self.queues.lock().unwrap().follow_up.push_back(input);
    }

    pub fn queued_counts(&self) -> (usize, usize) {
        let q = self.queues.lock().unwrap();
        (q.steering.len(), q.follow_up.len())
    }

    pub fn has_queued_messages(&self) -> bool {
        let q = self.queues.lock().unwrap();
        !q.steering.is_empty() || !q.follow_up.is_empty()
    }

    pub fn steering_mode(&self) -> QueueMode {
        self.queues.lock().unwrap().steering_mode
    }

    pub fn follow_up_mode(&self) -> QueueMode {
        self.queues.lock().unwrap().follow_up_mode
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        self.queues.lock().unwrap().steering_mode = mode;
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        self.queues.lock().unwrap().follow_up_mode = mode;
    }

    pub fn peek_steering_queue(&self) -> Vec<Message> {
        self.peek_steering_inputs().into_iter().map(|input| input.model).collect()
    }

    pub fn peek_steering_inputs(&self) -> Vec<AgentInput> {
        self.queues.lock().unwrap().steering.iter().cloned().collect()
    }

    pub fn peek_follow_up_queue(&self) -> Vec<Message> {
        self.peek_follow_up_inputs().into_iter().map(|input| input.model).collect()
    }

    pub fn peek_follow_up_inputs(&self) -> Vec<AgentInput> {
        self.queues.lock().unwrap().follow_up.iter().cloned().collect()
    }

    pub fn pop_last_steer(&self) -> Option<Message> {
        self.pop_last_steer_input().map(|input| input.model)
    }

    pub fn pop_last_steer_input(&self) -> Option<AgentInput> {
        self.queues.lock().unwrap().steering.pop_back()
    }

    pub fn pop_last_follow_up(&self) -> Option<Message> {
        self.pop_last_follow_up_input().map(|input| input.model)
    }

    pub fn pop_last_follow_up_input(&self) -> Option<AgentInput> {
        self.queues.lock().unwrap().follow_up.pop_back()
    }

    pub fn clear_steering_queue(&self) {
        self.queues.lock().unwrap().steering.clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.queues.lock().unwrap().follow_up.clear();
    }

    pub fn clear_all_queues(&self) {
        let mut q = self.queues.lock().unwrap();
        q.steering.clear();
        q.follow_up.clear();
    }

    pub fn abort(&self) {
        let active = self.run_state.lock().unwrap().active_cancel.clone();
        if let Some(cancel) = active {
            cancel.cancel();
        }
    }

    fn enter(&self, parent_cancel: &CancellationToken) -> Result<CancellationToken, AgentError> {
        let mut state = self.run_state.lock().unwrap();
        if state.needs_recovery {
            return Err(AgentError::NeedsRecovery);
        }
        if state.busy {
            return Err(AgentError::Busy);
        }
        let run_cancel = parent_cancel.child_token();
        state.active_cancel = Some(run_cancel.clone());
        state.busy = true;
        Ok(run_cancel)
    }

    fn config_with_queues(&self, mut config: AgentConfig) -> (AgentConfig, Arc<QueueHooks>) {
        let hooks = Arc::new(QueueHooks {
            queues: self.queues.clone(),
            base: config.hooks.clone(),
            skip_initial_steering_poll: AtomicBool::new(false),
        });
        config.hooks = hooks.clone();
        (config, hooks)
    }

    /// Run ownership stays with the Agent if the caller drops its waiting
    /// future. The host must keep the event sink alive for durable receipts.
    pub async fn prompt(
        self: &Arc<Self>,
        prompts: Vec<Message>,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.prompt_with_config(prompts, self.config.clone(), cancel, sink).await
    }

    /// Start a Run with an owned configuration snapshot, retaining this
    /// Agent's transcript and queues. The host can supply a fresh deadline or
    /// route without changing the defaults used by `prompt`/`continue_run`.
    /// Run settings remain fixed; `LoopHooks::execution_snapshot` may supply
    /// a live model route, tools and their matching prompt before each model call.
    pub async fn prompt_with_config(
        self: &Arc<Self>,
        prompts: Vec<Message>,
        config: AgentConfig,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.prompt_inputs_with_config(prompts.into_iter().map(AgentInput::from).collect(), config, cancel, sink).await
    }

    /// Start a Run carrying opaque host provenance through input emission.
    pub async fn prompt_inputs(
        self: &Arc<Self>,
        prompts: Vec<AgentInput>,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.prompt_inputs_with_config(prompts, self.config.clone(), cancel, sink).await
    }

    /// Owned input counterpart to `prompt_with_config`. Provenance belongs to
    /// each queued input and remains bound to this Run's sink when consumed.
    pub async fn prompt_inputs_with_config(
        self: &Arc<Self>,
        prompts: Vec<AgentInput>,
        config: AgentConfig,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        let cancel = self.enter(&cancel)?;
        let agent = self.clone();
        tokio::spawn(async move {
            let _guard = RunningGuard(agent.clone());
            let mut messages = agent.messages.lock().await;
            if unpaired_tool_call_tail(&messages).is_some() {
                return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                    crate::agent_loop::UNPAIRED_TAIL_REFUSED.into(),
                )));
            }
            let (config, _) = agent.config_with_queues(config);
            Ok(agent_loop_inputs(prompts, &mut messages, &config, &cancel, sink.as_ref()).await)
        })
        .await
        .map_err(|_| AgentError::RunPanicked)?
    }

    /// Resume a transcript or deliver queued messages after an idle stop.
    /// Unknown tool effects in an unpaired assistant tail always require host
    /// reconciliation before a new run.
    pub async fn continue_run(
        self: &Arc<Self>,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.continue_run_with_config(self.config.clone(), cancel, sink).await
    }

    /// Continue with a Run configuration snapshot while preserving queue
    /// modes, cancellation rollback and the unknown-effect recovery guard.
    /// Like `prompt_with_config`, this leaves the Agent defaults unchanged.
    pub async fn continue_run_with_config(
        self: &Arc<Self>,
        config: AgentConfig,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        let cancel = self.enter(&cancel)?;
        let agent = self.clone();
        tokio::spawn(async move {
            let _guard = RunningGuard(agent.clone());
            let mut messages = agent.messages.lock().await;
            let (config, queue_hooks) = agent.config_with_queues(config);
            if unpaired_tool_call_tail(&messages).is_some() {
                return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                    crate::agent_loop::UNPAIRED_TAIL_REFUSED.into(),
                )));
            }
            if messages.is_empty() || matches!(messages.last(), Some(Message::Assistant(_))) {
                if cancel.is_cancelled() || config.deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: run was cancelled or deadline passed".into(),
                    )));
                }
                let mut steering = true;
                let mut prompts = config.hooks.steering_inputs().await;
                if prompts.is_empty() {
                    steering = false;
                    prompts = config.hooks.follow_up_inputs().await;
                }
                if prompts.is_empty() {
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: no queued messages".into(),
                    )));
                }
                if cancel.is_cancelled() || config.deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                    let mut q = agent.queues.lock().unwrap();
                    let queue = if steering { &mut q.steering } else { &mut q.follow_up };
                    for message in prompts.into_iter().rev() {
                        queue.push_front(message);
                    }
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: run was cancelled or deadline passed".into(),
                    )));
                }
                // The first steering batch is already the initial prompt. OMP
                // skips the loop's first poll here so one-at-a-time stays one
                // message per model turn. A follow-up does not skip that poll.
                if steering {
                    queue_hooks.skip_initial_steering_poll.store(true, Ordering::Release);
                }
                Ok(agent_loop_inputs(prompts, &mut messages, &config, &cancel, sink.as_ref()).await)
            } else {
                agent_loop_continue(&mut messages, &config, &cancel, sink.as_ref(), UnpairedTail::Refuse)
                    .await
                    .map_err(AgentError::CannotContinue)
            }
        })
        .await
        .map_err(|_| AgentError::RunPanicked)?
    }
}

fn idle_unpaired_error() -> AgentError {
    AgentError::CannotContinue(LoopError::CannotContinue(crate::agent_loop::UNPAIRED_TAIL_REFUSED.into()))
}

/// Narrow mutation guard. The loop's existing helper classifies runnable
/// assistant turns; this also retains partial-result pending IDs and prevents
/// a proposed replacement from hiding one behind a later user/assistant.
fn has_unpaired_idle_calls(messages: &[Message]) -> bool {
    let mut pending: HashSet<(String, String)> = HashSet::new();
    for message in messages {
        if let Some(assistant) = unpaired_tool_call_tail(std::slice::from_ref(message)) {
            pending.extend(assistant.tool_calls().map(|call| (call.id.clone(), call.name.clone())));
        } else if let Message::ToolResult(result) = message {
            pending.remove(&(result.tool_call_id.clone(), result.tool_name.clone()));
        }
    }
    !pending.is_empty()
}

struct QueueHooks {
    queues: Arc<Mutex<Queues>>,
    base: Arc<dyn LoopHooks>,
    skip_initial_steering_poll: AtomicBool,
}

#[async_trait]
impl LoopHooks for QueueHooks {
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.base.execution_snapshot()
    }

    async fn before_tool_call(&self, call: &ToolCall, args: &JsonObject, cancel: &CancellationToken) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }

    async fn steering_inputs(&self) -> Vec<AgentInput> {
        if self.skip_initial_steering_poll.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        let from_base = self.base.steering_inputs().await;
        let mut result = self.queues.lock().unwrap().take_steering();
        result.extend(from_base);
        result
    }

    async fn follow_up_inputs(&self) -> Vec<AgentInput> {
        let from_base = self.base.follow_up_inputs().await;
        let mut result = self.queues.lock().unwrap().take_follow_up();
        result.extend(from_base);
        result
    }

    async fn transform_provider_context(&self, context: Context, model: &Model) -> Context {
        self.base.transform_provider_context(context, model).await
    }

    async fn turn_end_inputs(
        &self,
        message: &ara_ai::AssistantMessage,
        tool_results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.base.turn_end_inputs(message, tool_results, cancel).await
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use crate::{NoHooks, NullSink};
    use ara_ai::event::EventSink;
    use ara_ai::{
        AssistantBlock, AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions, ModelProvider,
        StopReason, ToolResultMessage, UserBlock, UserMessage,
    };
    use std::sync::atomic::AtomicUsize;
    use tokio::sync::Notify;

    #[derive(Default)]
    struct Gate {
        started: Notify,
        release: Notify,
    }

    struct Provider {
        calls: AtomicUsize,
        gate: Option<Arc<Gate>>,
    }

    impl ModelProvider for Provider {
        fn stream(&self, model: &Model, _context: &Context, _options: CallOptions) -> AssistantStream {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (sink, stream) = EventSink::channel();
            let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let gate = self.gate.clone();
            tokio::spawn(async move {
                sink.push(AssistantMessageEvent::Start { partial: message.clone() }).await;
                if let Some(gate) = gate {
                    gate.started.notify_one();
                    gate.release.notified().await;
                }
                message.content.push(AssistantBlock::text("known answer"));
                message.stop_reason = StopReason::Stop;
                sink.push(AssistantMessageEvent::Done { reason: StopReason::Stop, message }).await;
            });
            stream
        }
    }

    fn fixture(messages: Vec<Message>, gate: Option<Arc<Gate>>) -> (Arc<Agent>, Arc<Provider>) {
        let provider = Arc::new(Provider { calls: AtomicUsize::new(0), gate });
        let agent = Agent::new(
            AgentConfig {
                model: Model {
                    id: "idle-fixture".into(),
                    api: "openai-completions".into(),
                    provider: "fixture".into(),
                    base_url: String::new(),
                    reasoning: false,
                    max_tokens: None,
                    tokenizer: None,
                },
                provider: provider.clone(),
                system_prompt: Vec::new(),
                tools: Vec::new(),
                tool_choice: None,
                max_tokens: None,
                temperature: None,
                deadline: None,
                max_model_calls: Some(1),
                hooks: Arc::new(NoHooks),
            },
            messages,
        );
        (agent, provider)
    }

    fn user(text: &str) -> Message {
        Message::User(UserMessage::text(text))
    }

    fn unpaired(ids: &[&str]) -> Message {
        let mut message = AssistantMessage::empty("openai-completions", "fixture", "idle-fixture");
        message.stop_reason = StopReason::ToolUse;
        message.content = ids
            .iter()
            .map(|id| {
                AssistantBlock::ToolCall(ToolCall {
                    id: (*id).into(),
                    name: "write".into(),
                    arguments: JsonObject::new(),
                    thought_signature: None,
                })
            })
            .collect();
        Message::Assistant(message)
    }

    fn result(id: &str) -> Message {
        Message::ToolResult(ToolResultMessage {
            tool_call_id: id.into(),
            tool_name: "write".into(),
            content: vec![UserBlock::text("known result")],
            details: None,
            is_error: false,
            timestamp: 1,
        })
    }

    #[tokio::test]
    async fn idle_append_and_replace_preserve_queue_envelopes_and_modes_without_running_model() {
        let (agent, provider) = fixture(vec![user("initial")], None);
        let steering =
            AgentInput { model: user("steer"), provenance: Some(Arc::new(serde_json::json!({"origin":"s"}))) };
        let follow =
            AgentInput { model: user("follow"), provenance: Some(Arc::new(serde_json::json!({"origin":"f"}))) };
        agent.steer_input(steering.clone());
        agent.follow_up_input(follow.clone());
        agent.set_steering_mode(QueueMode::All);
        agent.set_follow_up_mode(QueueMode::OneAtATime);
        let appended = user("external projection");
        agent.append_idle_message(appended.clone()).unwrap();
        assert_eq!(agent.messages().await.len(), 2);
        agent.replace_idle_messages(vec![appended.clone()]).unwrap();
        assert_eq!(agent.messages().await, vec![appended]);
        assert_eq!(agent.peek_steering_inputs(), vec![steering]);
        assert_eq!(agent.peek_follow_up_inputs(), vec![follow]);
        assert_eq!(agent.steering_mode(), QueueMode::All);
        assert_eq!(agent.follow_up_mode(), QueueMode::OneAtATime);
        assert!(!agent.is_busy());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transcript_try_lock_and_real_run_admission_refuse_mutation_without_waiting() {
        let gate = Arc::new(Gate::default());
        let (agent, provider) = fixture(vec![user("existing")], Some(gate.clone()));
        {
            let _held = agent.messages.lock().await;
            assert!(matches!(agent.append_idle_message(user("blocked")), Err(AgentError::Busy)));
            assert!(matches!(agent.replace_idle_messages(Vec::new()), Err(AgentError::Busy)));
        }
        let running = agent.clone();
        let task = tokio::spawn(async move {
            running.prompt(vec![user("run")], CancellationToken::new(), Arc::new(NullSink)).await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), gate.started.notified()).await.unwrap();
        assert!(agent.is_busy());
        assert!(matches!(agent.append_idle_message(user("during run")), Err(AgentError::Busy)));
        assert!(matches!(agent.replace_idle_messages(Vec::new()), Err(AgentError::Busy)));
        gate.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), task).await.unwrap().unwrap().unwrap();
        assert!(!agent.is_busy());
        agent.append_idle_message(user("after joined Run")).unwrap();
        let messages = agent.messages().await;
        assert_eq!(messages.len(), 4);
        assert!(matches!(&messages[3], Message::User(message) if message.content.plain_text() == "after joined Run"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recovery_state_cannot_be_cleared_by_idle_mutation_or_replacement() {
        let original = user("original");
        let (agent, provider) = fixture(vec![original.clone()], None);
        agent.run_state.lock().unwrap().needs_recovery = true;
        assert!(matches!(agent.append_idle_message(user("hide")), Err(AgentError::NeedsRecovery)));
        assert!(matches!(agent.replace_idle_messages(Vec::new()), Err(AgentError::NeedsRecovery)));
        assert_eq!(agent.messages().await, vec![original]);
        assert!(agent.run_state.lock().unwrap().needs_recovery);
        assert!(matches!(
            agent.prompt(vec![user("next")], CancellationToken::new(), Arc::new(NullSink)).await,
            Err(AgentError::NeedsRecovery)
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn idle_append_cannot_hide_unpaired_tail_and_next_prompt_never_calls_provider_or_replays() {
        let original = unpaired(&["unknown-effect"]);
        let (agent, provider) = fixture(vec![original.clone()], None);
        agent.follow_up(user("still queued"));
        assert!(matches!(agent.append_idle_message(user("hide tail")), Err(AgentError::CannotContinue(_))));
        assert!(matches!(agent.replace_idle_messages(vec![user("hide tail")]), Err(AgentError::CannotContinue(_))));
        assert_eq!(agent.messages().await, vec![original]);
        assert!(matches!(
            agent.prompt(vec![user("next")], CancellationToken::new(), Arc::new(NullSink)).await,
            Err(AgentError::CannotContinue(_))
        ));
        assert_eq!(agent.queued_counts(), (0, 1));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn replacement_checks_embedded_and_partial_runnable_calls_but_accepts_known_pairs() {
        let original = user("original");
        let (agent, provider) = fixture(vec![original.clone()], None);
        assert!(matches!(
            agent.replace_idle_messages(vec![unpaired(&["unknown"]), user("hidden")]),
            Err(AgentError::CannotContinue(_))
        ));
        assert_eq!(agent.messages().await, vec![original]);
        let paired = vec![unpaired(&["known-1", "known-2"]), result("known-1"), result("known-2"), user("later")];
        agent.replace_idle_messages(paired.clone()).unwrap();
        assert_eq!(agent.messages().await, paired);
        let interleaved_known = vec![unpaired(&["known"]), user("historical external receipt"), result("known")];
        agent.replace_idle_messages(interleaved_known.clone()).unwrap();
        assert_eq!(agent.messages().await, interleaved_known);
        let (partial, _) = fixture(vec![unpaired(&["known", "unknown"]), result("known")], None);
        assert!(matches!(partial.append_idle_message(user("hide partial")), Err(AgentError::CannotContinue(_))));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}
