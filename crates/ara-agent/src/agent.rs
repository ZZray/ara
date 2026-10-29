//! Stateful owner for one agent transcript and its message queues.
//! OMP source: `packages/agent/src/agent.ts` at the locked upstream commit.

use crate::agent_loop::{
    AdmissionError, AfterToolCallContext, AfterToolCallResult, AgentConfig, BeforeToolCallContext,
    BeforeToolCallResult, ExecutionSnapshot, InterruptMode, IrcInterruptPeek, LoopError, LoopHooks,
    ModelExecutionSnapshot, QueueInputKind, RunReport, SteeringInterruptBinding, SteeringQueuePeek,
    SteeringQueueState, SteeringSource, SteeringSubscription, SteeringWake, ToolHookError,
    TurnEndContext, UnpairedTail, agent_loop_continue_with_gate, agent_loop_raw_with_gate,
    await_admission, poll_queued_input, unpaired_tool_call_tail,
};
use crate::agent_message::{
    AgentMessage, AgentMessageBody, RawContextError, checked_model_messages,
};
use crate::append_only_context::SharedAppendOnlyContextManager;
use crate::aside::{AsideError, AsideMessage, AsideMessageProvider, BeforeYieldHook};
use crate::event::{AgentEvent, AgentEventSink};
use crate::final_provider_gate::{
    FinalProviderGate, FinalProviderGateBinding, FinalProviderGateError, GateRegistration,
};
use crate::provider_context::{
    ProviderContextBinding, ProviderContextError, build_side_request_context_from_snapshot,
};
use crate::soft_tool_requirement::{
    SoftToolRequirementState, ToolChoiceDirective, ToolChoiceDirectiveSelection,
};
use crate::tool::{AgentTool, ToolDecision};
use crate::tool_directives::{
    DeferredToolChoices, RunToolChoiceClaim, RunToolChoiceGuard, ToolChoiceSelection,
    ToolDirectiveError, active_tool_choice,
};
use ara_ai::{AssistantMessage, Context, JsonObject, Message, Model, ToolCall, ToolChoice};
use async_trait::async_trait;
use futures::FutureExt;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};
use tokio::sync::{Mutex as AsyncMutex, Notify};
use tokio_util::sync::CancellationToken;

/// Typed compatibility input. Immediately adapted into a canonical Model
/// record with opaque Host source; genuine raw bodies use AgentMessage ports.
/// Source is never interpreted or serialized into provider/public messages.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInput {
    pub model: Message,
    pub provenance: Option<Arc<serde_json::Value>>,
}

impl From<Message> for AgentInput {
    fn from(model: Message) -> Self {
        Self {
            model,
            provenance: None,
        }
    }
}

impl From<AgentInput> for AgentMessage {
    fn from(input: AgentInput) -> Self {
        AgentMessage::from_owned_model(input.model, input.provenance)
    }
}

fn typed_input(message: &AgentMessage) -> Option<AgentInput> {
    match message.body_snapshot() {
        AgentMessageBody::Model(model) => Some(AgentInput {
            model,
            provenance: message.source(),
        }),
        AgentMessageBody::RawJson(_) | AgentMessageBody::Graph(_) => None,
    }
}
fn raw_steering_source(message: &AgentMessage) -> Result<SteeringSource, RawContextError> {
    Ok(match message.checked_model_snapshot()? {
        Some(Message::User(_)) => SteeringSource::User,
        Some(_) => SteeringSource::System,
        None => SteeringSource::Unknown,
    })
}

type AsideProviderBinding = Arc<RwLock<Option<Option<Arc<dyn AsideMessageProvider>>>>>;
type BeforeYieldBinding = Arc<RwLock<Option<Option<Arc<dyn BeforeYieldHook>>>>>;

#[derive(Debug)]
pub enum AgentError {
    Busy,
    NeedsRecovery,
    CannotContinue(LoopError),
    Admission(AdmissionError),
    Aside(AsideError),
    RawContext(RawContextError),
    RunPanicked,
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => f.write_str("Agent is already running"),
            Self::NeedsRecovery => {
                f.write_str("Agent run failed; reload the persisted session before continuing")
            }
            Self::CannotContinue(e) => e.fmt(f),
            Self::Admission(e) => e.fmt(f),
            Self::Aside(e) => e.fmt(f),
            Self::RawContext(e) => e.fmt(f),
            Self::RunPanicked => f.write_str("Agent run panicked; tool effects may be unknown"),
        }
    }
}

impl std::error::Error for AgentError {}
impl From<RawContextError> for AgentError {
    fn from(error: RawContextError) -> Self {
        Self::RawContext(error)
    }
}

/// How many Agent-owned queued messages a dequeue returns. OMP defaults to
/// one message per turn for both steering and follow-up queues.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum QueueMode {
    All,
    #[default]
    OneAtATime,
}

#[async_trait]
pub trait AgentAdmissionHook: Send + Sync {
    async fn run(&self, cancel: &CancellationToken) -> Result<(), AdmissionError>;
}

#[derive(Default)]
struct AdmissionHooks {
    next_id: u64,
    callbacks: BTreeMap<u64, Arc<dyn AgentAdmissionHook>>,
}

/// Explicit unsubscribe. Drop deliberately does not remove a Native callback.
pub struct HookRegistration {
    registry: Weak<Mutex<AdmissionHooks>>,
    id: u64,
}
impl HookRegistration {
    pub fn remove(&self) -> bool {
        self.registry.upgrade().is_some_and(|registry| {
            registry
                .lock()
                .unwrap()
                .callbacks
                .remove(&self.id)
                .is_some()
        })
    }
}
fn add_admission_hook(
    registry: &Arc<Mutex<AdmissionHooks>>,
    hook: Arc<dyn AgentAdmissionHook>,
) -> HookRegistration {
    let mut live = registry.lock().unwrap();
    live.next_id = live
        .next_id
        .checked_add(1)
        .expect("admission registration ID exhausted");
    let id = live.next_id;
    live.callbacks.insert(id, hook);
    HookRegistration {
        registry: Arc::downgrade(registry),
        id,
    }
}
async fn run_admission_hooks(
    registry: &Arc<Mutex<AdmissionHooks>>,
    cancel: &CancellationToken,
) -> Result<(), AdmissionError> {
    let mut cursor = 0;
    loop {
        let next = registry
            .lock()
            .unwrap()
            .callbacks
            .range((Excluded(cursor), Unbounded))
            .next()
            .map(|(id, hook)| (*id, hook.clone()));
        let Some((id, hook)) = next else {
            return Ok(());
        };
        cursor = id;
        match std::panic::AssertUnwindSafe(hook.run(cancel))
            .catch_unwind()
            .await
        {
            Ok(result) => result?,
            Err(_) => return Err(AdmissionError::Blocked("Admission hook panicked".into())),
        }
    }
}

#[derive(Clone)]
struct QueuedSteering {
    input: AgentMessage,
    source: SteeringSource,
}
fn legacy_steering_source(input: &AgentInput) -> SteeringSource {
    if matches!(input.model, Message::User(_)) {
        SteeringSource::User
    } else {
        SteeringSource::System
    }
}

#[derive(Default)]
struct Queues {
    steering: VecDeque<QueuedSteering>,
    follow_up: VecDeque<AgentMessage>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
}

impl Queues {
    fn take_steering(&mut self) -> Vec<QueuedSteering> {
        match self.steering_mode {
            QueueMode::All => self.steering.drain(..).collect(),
            QueueMode::OneAtATime => self.steering.pop_front().into_iter().collect(),
        }
    }

    fn take_follow_up(&mut self) -> Vec<AgentMessage> {
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
    interrupt_mode: RwLock<InterruptMode>,
    // None delegates to the Run's fixed hook; Some(None) explicitly disables the static binding.
    static_provider_context: Arc<RwLock<Option<Option<ProviderContextBinding>>>>,
    aside_provider: AsideProviderBinding,
    before_yield: BeforeYieldBinding,
    messages: AsyncMutex<Vec<AgentMessage>>,
    queues: Arc<Mutex<Queues>>,
    steering_wake: Arc<Notify>,
    pre_dequeue_hooks: Arc<Mutex<AdmissionHooks>>,
    pre_model_hooks: Arc<Mutex<AdmissionHooks>>,
    final_provider_gate: FinalProviderGateBinding,
    deferred_tool_choices: Arc<DeferredToolChoices>,
    // Clearing directives replaces this owner; an active Run keeps its old one.
    soft_tool_requirement: Mutex<SoftToolRequirementState>,
    run_state: Mutex<RunState>,
}

#[derive(Default)]
struct RunState {
    busy: bool,
    needs_recovery: bool,
    active_cancel: Option<CancellationToken>,
    raw_error: Option<RawContextError>,
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
        Self::from_adopted(
            config,
            messages
                .into_iter()
                .map(|message| AgentMessage::from_owned_model(message, None))
                .collect(),
        )
    }

    pub fn new_raw(
        config: AgentConfig,
        messages: Vec<AgentMessage>,
    ) -> Result<Arc<Self>, RawContextError> {
        for message in &messages {
            message.adopt()?;
        }
        Ok(Self::from_adopted(config, messages))
    }

    fn from_adopted(config: AgentConfig, messages: Vec<AgentMessage>) -> Arc<Self> {
        let interrupt_mode = config.interrupt_mode;
        // Adopt even a currently empty Host owner, and consult its getter once.
        let final_provider_gate = config
            .hooks
            .final_provider_gate_binding()
            .unwrap_or_default();
        Arc::new(Self {
            config,
            interrupt_mode: RwLock::new(interrupt_mode),
            static_provider_context: Arc::new(RwLock::new(None)),
            aside_provider: Arc::new(RwLock::new(None)),
            before_yield: Arc::new(RwLock::new(None)),
            messages: AsyncMutex::new(messages),
            queues: Arc::new(Mutex::new(Queues::default())),
            steering_wake: Arc::new(Notify::new()),
            pre_dequeue_hooks: Arc::new(Mutex::new(AdmissionHooks::default())),
            pre_model_hooks: Arc::new(Mutex::new(AdmissionHooks::default())),
            final_provider_gate,
            deferred_tool_choices: Arc::new(DeferredToolChoices::default()),
            soft_tool_requirement: Mutex::new(SoftToolRequirementState::default()),
            run_state: Mutex::new(RunState::default()),
        })
    }

    pub fn is_busy(&self) -> bool {
        self.run_state.lock().unwrap().busy
    }

    pub fn interrupt_mode(&self) -> InterruptMode {
        *self.interrupt_mode.read().unwrap()
    }
    pub fn set_interrupt_mode(&self, mode: InterruptMode) {
        *self.interrupt_mode.write().unwrap() = mode;
    }
    fn default_run_config(&self) -> AgentConfig {
        let mut config = self.config.clone();
        config.interrupt_mode = self.interrupt_mode();
        config
    }
    pub fn add_before_queued_message_dequeue_hook(
        &self,
        hook: Arc<dyn AgentAdmissionHook>,
    ) -> HookRegistration {
        add_admission_hook(&self.pre_dequeue_hooks, hook)
    }
    pub fn add_before_model_call_hook(
        &self,
        hook: Arc<dyn AgentAdmissionHook>,
    ) -> HookRegistration {
        add_admission_hook(&self.pre_model_hooks, hook)
    }

    pub fn set_final_provider_gate(&self, gate: Option<Arc<dyn FinalProviderGate>>) {
        self.final_provider_gate.set_primary(gate);
    }

    pub fn add_final_provider_gate(&self, gate: Arc<dyn FinalProviderGate>) -> GateRegistration {
        self.final_provider_gate.add(gate)
    }

    fn static_provider_binding(&self) -> Option<ProviderContextBinding> {
        let override_binding = { self.static_provider_context.read().unwrap().clone() };
        override_binding.unwrap_or_else(|| self.config.hooks.provider_context_binding())
    }

    /// Read one current Host snapshot, or this Core Agent's static binding.
    pub fn append_only_context(&self) -> Option<SharedAppendOnlyContextManager> {
        if let Some(snapshot) = self.config.hooks.execution_snapshot() {
            return snapshot
                .provider_context
                .and_then(|binding| binding.append_only_context);
        }
        self.static_provider_binding()
            .and_then(|binding| binding.append_only_context)
    }

    pub fn set_append_only_context(
        &self,
        manager: Option<SharedAppendOnlyContextManager>,
    ) -> Result<(), ProviderContextError> {
        if self.config.hooks.execution_snapshot().is_some() {
            return self.config.hooks.set_append_only_context(manager);
        }
        let fallback = self.static_provider_binding();
        let mut current = self.static_provider_context.write().unwrap();
        let mut binding = current.clone().unwrap_or(fallback);
        if let Some(binding) = binding.as_mut() {
            binding.append_only_context = manager;
        } else if manager.is_some() {
            binding = Some(ProviderContextBinding {
                append_only_context: manager,
                ..ProviderContextBinding::default()
            });
        }
        *current = Some(binding);
        Ok(())
    }

    /// Static Core configuration only. Dynamic Hosts publish the whole binding with their model route.
    pub fn set_provider_context_binding(
        &self,
        binding: Option<ProviderContextBinding>,
    ) -> Result<(), ProviderContextError> {
        if self.config.hooks.execution_snapshot().is_some() {
            return Err(ProviderContextError::HostOwnedMutation);
        }
        *self.static_provider_context.write().unwrap() = Some(binding);
        Ok(())
    }

    pub async fn build_side_request_context(
        &self,
        messages: Vec<Message>,
        system_prompt_override: Option<Vec<String>>,
    ) -> Result<Context, ProviderContextError> {
        let snapshot = if let Some(snapshot) = self.config.hooks.execution_snapshot() {
            snapshot
        } else {
            let provider_context = self.static_provider_binding();
            ExecutionSnapshot {
                tools: self.config.tools.clone(),
                system_prompt: self.config.system_prompt.clone(),
                model: Some(ModelExecutionSnapshot {
                    model: self.config.model.clone(),
                    provider: self.config.provider.clone(),
                    tool_choice: self.config.tool_choice.clone(),
                    max_tokens: self.config.max_tokens,
                    temperature: self.config.temperature,
                }),
                provider_context,
            }
        };
        build_side_request_context_from_snapshot(
            &self.config,
            snapshot,
            messages,
            system_prompt_override,
        )
        .await
    }

    /// Read-only typed compatibility view. Raw state remains the owner and this
    /// getter never runs transform/conversion or writes a projection back.
    pub async fn messages(&self) -> Result<Vec<Message>, RawContextError> {
        checked_model_messages(&self.raw_messages().await)
    }
    pub async fn raw_messages(&self) -> Vec<AgentMessage> {
        self.messages.lock().await.clone()
    }

    /// Atomically append an externally observed model projection while idle.
    /// Hold admission's state lock through a non-awaiting transcript try_lock;
    /// a Run cannot enter between the idle check and this update.
    pub fn append_idle_message(&self, message: Message) -> Result<(), AgentError> {
        self.append_idle_raw_message(message.into())
    }
    pub fn append_idle_raw_message(&self, message: AgentMessage) -> Result<(), AgentError> {
        self.update_idle_messages(|messages| {
            if has_unpaired_idle_calls(messages)? {
                return Err(idle_unpaired_error());
            }
            message.adopt()?;
            messages.push(message);
            Ok(())
        })
    }

    /// Replace only the idle transcript. Queues, modes and recovery state stay
    /// owned by this Agent. Neither the old nor new view may hide runnable tool
    /// calls without known results behind a later ordinary message.
    pub fn replace_idle_messages(&self, replacement: Vec<Message>) -> Result<(), AgentError> {
        self.replace_idle_raw_messages(replacement.into_iter().map(AgentMessage::from).collect())
    }
    pub fn replace_idle_raw_messages(
        &self,
        replacement: Vec<AgentMessage>,
    ) -> Result<(), AgentError> {
        self.update_idle_messages(|messages| {
            if has_unpaired_idle_calls(messages)? || has_unpaired_idle_calls(&replacement)? {
                return Err(idle_unpaired_error());
            }
            for message in &replacement {
                message.adopt()?;
            }
            *messages = replacement;
            Ok(())
        })
    }

    /// Clear an idle transcript, queues and retained hard directives together.
    /// Refuse unknown effects and recovery/busy states before clearing anything.
    pub fn reset_idle(&self) -> Result<(), AgentError> {
        self.update_idle_messages(|messages| {
            if has_unpaired_idle_calls(messages)? {
                return Err(idle_unpaired_error());
            }
            messages.clear();
            self.clear_all_queues();
            Ok(())
        })
    }

    fn update_idle_messages(
        &self,
        update: impl FnOnce(&mut Vec<AgentMessage>) -> Result<(), AgentError>,
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
        let source = legacy_steering_source(&input);
        self.steer_input_with_source(input, source);
    }

    pub fn steer_input_with_source(&self, input: AgentInput, source: SteeringSource) {
        self.enqueue_steering(input.into(), source);
    }
    pub fn steer_raw(&self, input: AgentMessage) -> Result<(), RawContextError> {
        let source = raw_steering_source(&input)?;
        self.steer_raw_with_source(input, source)
    }
    pub fn steer_raw_with_source(
        &self,
        input: AgentMessage,
        source: SteeringSource,
    ) -> Result<(), RawContextError> {
        input.adopt()?;
        self.enqueue_steering(input, source);
        Ok(())
    }
    fn enqueue_steering(&self, input: AgentMessage, source: SteeringSource) {
        self.queues
            .lock()
            .unwrap()
            .steering
            .push_back(QueuedSteering { input, source });
        self.steering_wake.notify_waiters();
    }

    /// Setter replacement is read at each injection boundary, including an active Run.
    /// None disables the Host's static provider; it never wakes steering or interrupts tools.
    pub fn set_aside_message_provider(&self, provider: Option<Arc<dyn AsideMessageProvider>>) {
        *self.aside_provider.write().unwrap() = Some(provider);
    }

    pub fn set_on_before_yield(&self, hook: Option<Arc<dyn BeforeYieldHook>>) {
        *self.before_yield.write().unwrap() = Some(hook);
    }

    fn record_raw_failure(&self, error: RawContextError) {
        let mut state = self.run_state.lock().unwrap();
        if state.raw_error.is_none() {
            state.raw_error = Some(error);
        }
        state.needs_recovery = true;
        if let Some(cancel) = &state.active_cancel {
            cancel.cancel();
        }
    }
    fn raw_error(&self) -> Option<RawContextError> {
        self.run_state.lock().unwrap().raw_error.clone()
    }
    fn checked_run_result<T>(&self, result: Result<T, AgentError>) -> Result<T, AgentError> {
        if let Some(error) = self.raw_error() {
            Err(AgentError::RawContext(error))
        } else {
            result
        }
    }

    fn aside_failed(&self, error: AsideError) -> AgentError {
        self.run_state.lock().unwrap().needs_recovery = true;
        AgentError::Aside(error)
    }

    pub fn follow_up(&self, message: Message) {
        self.follow_up_input(message.into());
    }

    pub fn follow_up_input(&self, input: AgentInput) {
        self.queues
            .lock()
            .unwrap()
            .follow_up
            .push_back(input.into());
    }
    pub fn follow_up_raw(&self, input: AgentMessage) -> Result<(), RawContextError> {
        input.adopt()?;
        self.queues.lock().unwrap().follow_up.push_back(input);
        Ok(())
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

    pub fn peek_steering_queue(&self) -> Result<Vec<Message>, RawContextError> {
        checked_model_messages(&self.peek_steering_raw_messages())
    }
    pub fn peek_steering_raw_messages(&self) -> Vec<AgentMessage> {
        self.queues
            .lock()
            .unwrap()
            .steering
            .iter()
            .map(|queued| queued.input.clone())
            .collect()
    }
    pub fn peek_steering_inputs(&self) -> Vec<AgentInput> {
        self.peek_steering_raw_messages()
            .iter()
            .filter_map(typed_input)
            .collect()
    }
    pub fn peek_follow_up_queue(&self) -> Result<Vec<Message>, RawContextError> {
        checked_model_messages(&self.peek_follow_up_raw_messages())
    }
    pub fn peek_follow_up_raw_messages(&self) -> Vec<AgentMessage> {
        self.queues
            .lock()
            .unwrap()
            .follow_up
            .iter()
            .cloned()
            .collect()
    }
    pub fn peek_follow_up_inputs(&self) -> Vec<AgentInput> {
        self.peek_follow_up_raw_messages()
            .iter()
            .filter_map(typed_input)
            .collect()
    }
    pub fn pop_last_steer(&self) -> Option<Message> {
        self.pop_last_steer_input().map(|input| input.model)
    }
    pub fn pop_last_steer_input(&self) -> Option<AgentInput> {
        let mut queues = self.queues.lock().unwrap();
        let input = typed_input(&queues.steering.back()?.input)?;
        queues.steering.pop_back();
        Some(input)
    }
    pub fn pop_last_steer_raw(&self) -> Option<AgentMessage> {
        self.queues
            .lock()
            .unwrap()
            .steering
            .pop_back()
            .map(|queued| queued.input)
    }
    pub fn pop_last_follow_up(&self) -> Option<Message> {
        self.pop_last_follow_up_input().map(|input| input.model)
    }
    pub fn pop_last_follow_up_input(&self) -> Option<AgentInput> {
        let mut queues = self.queues.lock().unwrap();
        let input = typed_input(queues.follow_up.back()?)?;
        queues.follow_up.pop_back();
        Some(input)
    }
    pub fn pop_last_follow_up_raw(&self) -> Option<AgentMessage> {
        self.queues.lock().unwrap().follow_up.pop_back()
    }

    pub fn clear_steering_queue(&self) {
        self.queues.lock().unwrap().steering.clear();
    }

    pub fn clear_follow_up_queue(&self) {
        self.queues.lock().unwrap().follow_up.clear();
    }

    /// Drop the bounded hard directive retained across an owner-stopped Run.
    /// Full Native soft lifecycle state is not implemented by this foundation.
    pub fn clear_deferred_tool_directives(&self) {
        self.deferred_tool_choices.clear();
        *self.soft_tool_requirement.lock().unwrap() = SoftToolRequirementState::default();
    }

    pub fn clear_all_queues(&self) {
        {
            let mut q = self.queues.lock().unwrap();
            q.steering.clear();
            q.follow_up.clear();
        }
        self.clear_deferred_tool_directives();
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
            aside_provider: self.aside_provider.clone(),
            before_yield: self.before_yield.clone(),
            steering_wake: self.steering_wake.clone(),
            pre_dequeue_hooks: self.pre_dequeue_hooks.clone(),
            pre_model_hooks: self.pre_model_hooks.clone(),
            final_provider_gate: self.final_provider_gate.clone(),
            deferred_tool_choices: self.deferred_tool_choices.clone(),
            soft_tool_requirement: self.soft_tool_requirement.lock().unwrap().clone(),
            tool_choice_claim: RunToolChoiceClaim::new(self.deferred_tool_choices.clone()),
            taken_steering_sources: Mutex::new(Vec::new()),
            base: config.hooks.clone(),
            static_provider_context: self.static_provider_context.clone(),
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
        self.prompt_with_config(prompts, self.default_run_config(), cancel, sink)
            .await
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
        self.prompt_inputs_with_config(
            prompts.into_iter().map(AgentInput::from).collect(),
            config,
            cancel,
            sink,
        )
        .await
    }

    /// Start a Run carrying opaque host provenance through input emission.
    pub async fn prompt_inputs(
        self: &Arc<Self>,
        prompts: Vec<AgentInput>,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.prompt_inputs_with_config(prompts, self.default_run_config(), cancel, sink)
            .await
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
        self.prompt_raw_with_config(
            prompts.into_iter().map(AgentMessage::from).collect(),
            config,
            cancel,
            sink,
        )
        .await
    }
    pub async fn prompt_raw(
        self: &Arc<Self>,
        prompts: Vec<AgentMessage>,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        self.prompt_raw_with_config(prompts, self.default_run_config(), cancel, sink)
            .await
    }
    pub async fn prompt_raw_with_config(
        self: &Arc<Self>,
        prompts: Vec<AgentMessage>,
        config: AgentConfig,
        cancel: CancellationToken,
        sink: Arc<dyn AgentEventSink>,
    ) -> Result<RunReport, AgentError> {
        let cancel = self.enter(&cancel)?;
        let guard = RunningGuard(self.clone());
        for prompt in &prompts {
            prompt.adopt()?;
        }
        // Sample before spawn/join or any sink/dequeue await can install a gate.
        let final_provider_gate = self.final_provider_gate.sample_for_run();
        let agent = self.clone();
        tokio::spawn(async move {
            let _guard = guard;
            let mut messages = agent.messages.lock().await.clone();
            let sink: Arc<dyn AgentEventSink> = Arc::new(AgentStateSink {
                agent: agent.clone(),
                downstream: sink,
            });
            if crate::agent_loop::raw_has_unpaired_tool_calls(&messages)? {
                return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                    crate::agent_loop::UNPAIRED_TAIL_REFUSED.into(),
                )));
            }
            let (config, queue_hooks) = agent.config_with_queues(config);
            let directive_guard = RunToolChoiceGuard::new(queue_hooks.tool_choice_claim.clone());
            let report = agent_loop_raw_with_gate(
                prompts,
                &mut messages,
                &config,
                &cancel,
                sink.as_ref(),
                final_provider_gate,
            )
            .await
            .map_err(|error| agent.aside_failed(error));
            let report = agent.checked_run_result(report)?;
            directive_guard.finish(&report);
            Ok(report)
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
        self.continue_run_with_config(self.default_run_config(), cancel, sink)
            .await
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
        let final_provider_gate = self.final_provider_gate.sample_for_run();
        let agent = self.clone();
        tokio::spawn(async move {
            let _guard = RunningGuard(agent.clone());
            let mut messages = agent.messages.lock().await.clone();
            let sink: Arc<dyn AgentEventSink> = Arc::new(AgentStateSink {
                agent: agent.clone(),
                downstream: sink,
            });
            let (config, queue_hooks) = agent.config_with_queues(config);
            let directive_guard = RunToolChoiceGuard::new(queue_hooks.tool_choice_claim.clone());
            if crate::agent_loop::raw_has_unpaired_tool_calls(&messages)? {
                return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                    crate::agent_loop::UNPAIRED_TAIL_REFUSED.into(),
                )));
            }
            let graph_assistant = if let Some(last) = messages.last()
                && matches!(
                    last.body_snapshot(),
                    crate::agent_message::AgentMessageBody::Graph(_)
                ) {
                last.graph_stream_tail_role()
                    .map_err(|error| RawContextError(error.to_string()))?
                    == ara_rpc::WireString::from("assistant")
            } else {
                false
            };
            let tail = match messages.last() {
                Some(last)
                    if !matches!(
                        last.body_snapshot(),
                        crate::agent_message::AgentMessageBody::Graph(_)
                    ) =>
                {
                    last.checked_model_snapshot()?
                }
                _ => None,
            };
            let report = if messages.is_empty()
                || graph_assistant
                || tail.as_ref().is_some_and(|message| {
                    matches!(message, Message::Assistant(_))
                        && !ara_ai::remote_compaction::is_native_compaction_carrier(
                            message,
                            &config.model,
                        )
                }) {
                if cancel.is_cancelled()
                    || config
                        .deadline
                        .is_some_and(|d| std::time::Instant::now() >= d)
                {
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: run was cancelled or deadline passed".into(),
                    )));
                }
                let mut steering = true;
                let mut prompts =
                    poll_queued_input(&config, &messages, &cancel, QueueInputKind::Steering)
                        .await
                        .map_err(AgentError::Admission)?;
                if prompts.is_empty() {
                    steering = false;
                    prompts =
                        poll_queued_input(&config, &messages, &cancel, QueueInputKind::FollowUp)
                            .await
                            .map_err(AgentError::Admission)?;
                }
                if prompts.is_empty() {
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: no queued messages".into(),
                    )));
                }
                if cancel.is_cancelled()
                    || config
                        .deadline
                        .is_some_and(|d| std::time::Instant::now() >= d)
                {
                    let mut q = agent.queues.lock().unwrap();
                    let sources = queue_hooks.taken_steering_sources.lock().unwrap().clone();
                    let mut classification_error = None;
                    for (index, input) in prompts.into_iter().enumerate().rev() {
                        if steering {
                            let source = match sources.get(index).copied() {
                                Some(source) => source,
                                None => match raw_steering_source(&input) {
                                    Ok(source) => source,
                                    Err(error) => {
                                        classification_error.get_or_insert(error);
                                        SteeringSource::Unknown
                                    }
                                },
                            };
                            q.steering.push_front(QueuedSteering { input, source });
                        } else {
                            q.follow_up.push_front(input);
                        }
                    }
                    drop(q);
                    if let Some(error) = classification_error {
                        agent.record_raw_failure(error.clone());
                        return Err(AgentError::RawContext(error));
                    }
                    return Err(AgentError::CannotContinue(LoopError::CannotContinue(
                        "Cannot continue: run was cancelled or deadline passed".into(),
                    )));
                }
                // The first steering batch is already the initial prompt. OMP
                // skips the loop's first poll here so one-at-a-time stays one
                // message per model turn. A follow-up does not skip that poll.
                if steering {
                    queue_hooks
                        .skip_initial_steering_poll
                        .store(true, Ordering::Release);
                }
                agent_loop_raw_with_gate(
                    prompts,
                    &mut messages,
                    &config,
                    &cancel,
                    sink.as_ref(),
                    final_provider_gate,
                )
                .await
                .map_err(|error| agent.aside_failed(error))
            } else {
                agent_loop_continue_with_gate(
                    &mut messages,
                    &config,
                    &cancel,
                    sink.as_ref(),
                    UnpairedTail::Refuse,
                    final_provider_gate,
                )
                .await
                .map_err(|error| match error {
                    LoopError::Aside(error) => agent.aside_failed(error),
                    other => AgentError::CannotContinue(other),
                })
            };
            let report = agent.checked_run_result(report)?;
            directive_guard.finish(&report);
            Ok(report)
        })
        .await
        .map_err(|_| AgentError::RunPanicked)?
    }
}

fn idle_unpaired_error() -> AgentError {
    AgentError::CannotContinue(LoopError::CannotContinue(
        crate::agent_loop::UNPAIRED_TAIL_REFUSED.into(),
    ))
}

/// Narrow mutation guard. The loop's existing helper classifies runnable
/// assistant turns; this also retains partial-result pending IDs and prevents
/// a proposed replacement from hiding one behind a later user/assistant.
fn has_unpaired_idle_calls(messages: &[AgentMessage]) -> Result<bool, RawContextError> {
    let mut pending: HashSet<(ara_rpc::WireString, ara_rpc::WireString)> = HashSet::new();
    for raw in messages {
        if matches!(
            raw.body_snapshot(),
            crate::agent_message::AgentMessageBody::Graph(_)
        ) {
            let (_, runnable, calls, result) = raw
                .graph_identity_controls()
                .map_err(|error| RawContextError(error.to_string()))?;
            if runnable {
                pending.extend(calls);
            }
            if let Some(result) = result {
                pending.remove(&result);
            }
            continue;
        }
        let Some(message) = raw.checked_model_snapshot()? else {
            continue;
        };
        if let Some(assistant) = unpaired_tool_call_tail(std::slice::from_ref(&message)) {
            pending.extend(
                assistant
                    .tool_calls()
                    .map(|call| (call.id.as_str().into(), call.name.as_str().into())),
            );
        } else if let Message::ToolResult(result) = message {
            pending.remove(&(
                result.tool_call_id.as_str().into(),
                result.tool_name.as_str().into(),
            ));
        }
    }
    Ok(!pending.is_empty())
}

/// Native array slice plus explicit message_end commit. No Run holds Agent's
/// transcript lock across a hook, downstream persistence or provider work.
struct AgentStateSink {
    agent: Arc<Agent>,
    downstream: Arc<dyn AgentEventSink>,
}
impl AgentStateSink {
    async fn commit(&self, message: AgentMessage) -> bool {
        if self.agent.raw_error().is_some() {
            return false;
        }
        if let Err(error) = message.adopt() {
            self.agent.record_raw_failure(error);
            return false;
        }
        let mut messages = self.agent.messages.lock().await;
        if self.agent.raw_error().is_some() {
            return false;
        }
        messages.push(message);
        true
    }
}
#[async_trait]
impl AgentEventSink for AgentStateSink {
    fn abort_reason(&self) -> Option<crate::agent_loop::RunAbortReason> {
        self.downstream.abort_reason()
    }

    async fn emit(&self, event: AgentEvent) {
        if self.agent.raw_error().is_some() {
            return;
        }
        if let AgentEvent::MessageEnd { message } = &event
            && !self.commit(message.clone()).await
        {
            return;
        }
        if let AgentEvent::ActualMessageEnd { message, .. } = &event {
            // This explicit failure event cannot adopt successful execution
            // controls. Retain the same failed record and actual receipt; the
            // Graph assistant-tail consumer still refuses preparation/replay.
            let mut messages = self.agent.messages.lock().await;
            if self.agent.raw_error().is_some() {
                return;
            }
            messages.push(message.clone());
        }
        if self.agent.raw_error().is_some() {
            return;
        }
        self.downstream.emit(event).await;
    }
    async fn commit_unemitted_message(&self, message: AgentMessage) {
        self.commit(message).await;
    }
}

struct QueueHooks {
    queues: Arc<Mutex<Queues>>,
    aside_provider: AsideProviderBinding,
    before_yield: BeforeYieldBinding,
    steering_wake: Arc<Notify>,
    pre_dequeue_hooks: Arc<Mutex<AdmissionHooks>>,
    pre_model_hooks: Arc<Mutex<AdmissionHooks>>,
    final_provider_gate: FinalProviderGateBinding,
    deferred_tool_choices: Arc<DeferredToolChoices>,
    tool_choice_claim: Arc<RunToolChoiceClaim>,
    soft_tool_requirement: SoftToolRequirementState,
    taken_steering_sources: Mutex<Vec<SteeringSource>>,
    base: Arc<dyn LoopHooks>,
    static_provider_context: Arc<RwLock<Option<Option<ProviderContextBinding>>>>,
    skip_initial_steering_poll: AtomicBool,
}

impl QueueHooks {
    fn take_steering_raw_messages(&self) -> Vec<AgentMessage> {
        let queued = self.queues.lock().unwrap().take_steering();
        *self.taken_steering_sources.lock().unwrap() =
            queued.iter().map(|item| item.source).collect();
        queued.into_iter().map(|item| item.input).collect()
    }
}

fn source_priority(source: SteeringSource) -> u8 {
    match source {
        SteeringSource::User => 0,
        SteeringSource::Agent => 1,
        SteeringSource::System => 2,
        SteeringSource::Unknown => 3,
    }
}
struct AgentQueuePeek {
    queues: Arc<Mutex<Queues>>,
    base: Option<SteeringInterruptBinding>,
}
#[async_trait]
impl SteeringQueuePeek for AgentQueuePeek {
    async fn peek(&self) -> Result<SteeringQueueState, ToolHookError> {
        let owned = {
            let queues = self.queues.lock().unwrap();
            match queues.steering_mode {
                QueueMode::OneAtATime => queues.steering.front().map(|item| item.source),
                QueueMode::All => queues
                    .steering
                    .iter()
                    .map(|item| item.source)
                    .min_by_key(|source| source_priority(*source)),
            }
        };
        if owned == Some(SteeringSource::User) {
            return Ok(SteeringQueueState {
                queued: true,
                source: owned,
            });
        }
        let external = match &self.base {
            Some(base) => base.peek_port().peek().await?,
            None => SteeringQueueState::default(),
        };
        let external_source = external
            .queued
            .then(|| external.source.unwrap_or(SteeringSource::Unknown));
        let source = owned
            .into_iter()
            .chain(external_source)
            .min_by_key(|source| source_priority(*source));
        Ok(SteeringQueueState {
            queued: source.is_some(),
            source,
        })
    }
}
struct AgentQueueWake {
    owned: Arc<Notify>,
    base: Option<Arc<dyn SteeringWake>>,
}
impl SteeringWake for AgentQueueWake {
    fn subscribe(&self, local_cancel: CancellationToken) -> SteeringSubscription {
        let mut owned = Box::pin(self.owned.clone().notified_owned());
        owned.as_mut().enable(); // Install before peek, not when this future is polled.
        let base = self
            .base
            .as_ref()
            .map(|wake| wake.subscribe(local_cancel.clone()));
        SteeringSubscription::from_future(async move {
            tokio::select! {
                biased;
                _ = local_cancel.cancelled() => Ok(()),
                _ = owned => Ok(()),
                result = async {
                    match base { Some(subscription) => subscription.wait().await, None => std::future::pending().await }
                } => result,
            }
        })
    }
}

#[async_trait]
impl LoopHooks for QueueHooks {
    fn final_provider_gate_binding(&self) -> Option<FinalProviderGateBinding> {
        Some(self.final_provider_gate.clone())
    }

    fn on_final_provider_gate_rejected(&self) -> Result<(), FinalProviderGateError> {
        self.base.on_final_provider_gate_rejected()?;
        self.tool_choice_claim.stage_rejected();
        Ok(())
    }

    fn select_tool_choice_directive(
        &self,
        active_tools: &[Arc<dyn AgentTool>],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<ToolChoiceSelection, ToolDirectiveError> {
        self.tool_choice_claim.clear_claim();
        if let Some(deferred) = self.deferred_tool_choices.take() {
            if let Some(active) = active_tool_choice(deferred, active_tools) {
                self.tool_choice_claim.claim(&active);
                return Ok(ToolChoiceSelection {
                    directive: Some(active),
                    static_fallback: None,
                });
            }
            // The retained slot is already empty; no lock crosses this callback.
            self.base.on_deferred_tool_choice_unavailable()?;
        }
        let fresh = self
            .base
            .select_tool_choice_directive(active_tools, admitted_static_choice)?;
        let choice = if let Some(dynamic) = fresh.directive {
            let active = active_tool_choice(dynamic, active_tools);
            if let Some(choice) = &active {
                self.tool_choice_claim.claim(choice);
            }
            active
        } else {
            fresh
                .static_fallback
                .and_then(|choice| active_tool_choice(choice, active_tools))
        };
        // This filtered facade result is authoritative. Do not resurrect a
        // stale configured named choice through the direct-loop fallback.
        Ok(ToolChoiceSelection {
            directive: choice,
            static_fallback: None,
        })
    }

    fn soft_tool_requirement_state(&self) -> Option<SoftToolRequirementState> {
        Some(self.soft_tool_requirement.clone())
    }

    fn select_tool_choice_directive_full(
        &self,
        active_tools: &[Arc<dyn AgentTool>],
        admitted_static_choice: Option<&ToolChoice>,
    ) -> Result<ToolChoiceDirectiveSelection, ToolDirectiveError> {
        self.tool_choice_claim.clear_claim();
        if let Some(deferred) = self.deferred_tool_choices.take() {
            if let Some(active) = active_tool_choice(deferred, active_tools) {
                self.tool_choice_claim.claim(&active);
                return Ok(ToolChoiceDirectiveSelection {
                    directive: Some(ToolChoiceDirective::Hard(active)),
                    static_fallback: None,
                    suppress_static_soft_block: false,
                });
            }
            self.base.on_deferred_tool_choice_unavailable()?;
        }
        let fresh = self
            .base
            .select_tool_choice_directive_full(active_tools, admitted_static_choice)?;
        let choice = match fresh.directive {
            Some(ToolChoiceDirective::Soft(requirement)) => {
                let active = active_tools
                    .iter()
                    .any(|tool| tool.definition().name == requirement.tool_name);
                // A queued soft directive is never a deferred hard claim, and
                // its disappearance does not revive a different dynamic source.
                return Ok(ToolChoiceDirectiveSelection {
                    directive: active.then_some(ToolChoiceDirective::Soft(requirement)),
                    static_fallback: None,
                    suppress_static_soft_block: true,
                });
            }
            Some(ToolChoiceDirective::Hard(dynamic)) => {
                let active = active_tool_choice(dynamic, active_tools);
                if let Some(choice) = &active {
                    self.tool_choice_claim.claim(choice);
                }
                active
            }
            None => fresh
                .static_fallback
                .and_then(|choice| active_tool_choice(choice, active_tools)),
        };
        Ok(ToolChoiceDirectiveSelection {
            directive: choice.map(ToolChoiceDirective::Hard),
            static_fallback: None,
            suppress_static_soft_block: false,
        })
    }

    fn on_deferred_tool_choice_unavailable(&self) -> Result<(), ToolDirectiveError> {
        self.base.on_deferred_tool_choice_unavailable()
    }

    fn context_admission_enabled(&self) -> bool {
        self.base.context_admission_enabled()
            || !self.pre_dequeue_hooks.lock().unwrap().callbacks.is_empty()
    }
    fn execution_snapshot(&self) -> Option<ExecutionSnapshot> {
        self.base.execution_snapshot()
    }

    fn provider_context_binding(&self) -> Option<ProviderContextBinding> {
        let override_binding = { self.static_provider_context.read().unwrap().clone() };
        override_binding.unwrap_or_else(|| self.base.provider_context_binding())
    }

    fn set_append_only_context(
        &self,
        manager: Option<SharedAppendOnlyContextManager>,
    ) -> Result<(), ProviderContextError> {
        self.base.set_append_only_context(manager)
    }

    async fn before_queued_input_dequeue(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        self.base.before_queued_input_dequeue(cancel).await
    }

    async fn before_model_call(
        &self,
        snapshot: ExecutionSnapshot,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        let registered = !self.pre_model_hooks.lock().unwrap().callbacks.is_empty();
        let snapshot = if registered {
            run_admission_hooks(&self.pre_model_hooks, cancel).await?;
            self.base.execution_snapshot().unwrap_or(snapshot)
        } else {
            snapshot
        };
        self.base.before_model_call(snapshot, cancel).await
    }

    async fn before_queued_input_dequeue_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_context(snapshot, messages, cancel)
            .await
    }

    async fn before_model_call_with_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<Message>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        let registered = !self.pre_model_hooks.lock().unwrap().callbacks.is_empty();
        let snapshot = if registered {
            run_admission_hooks(&self.pre_model_hooks, cancel).await?;
            self.base.execution_snapshot().unwrap_or(snapshot)
        } else {
            snapshot
        };
        self.base
            .before_model_call_with_context(snapshot, messages, cancel)
            .await
    }

    async fn before_queued_input_dequeue_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<(), AdmissionError> {
        self.base
            .before_queued_input_dequeue_with_raw_context(snapshot, messages, cancel)
            .await
    }
    async fn before_model_call_with_raw_context(
        &self,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<ExecutionSnapshot, AdmissionError> {
        let registered = !self.pre_model_hooks.lock().unwrap().callbacks.is_empty();
        let snapshot = if registered {
            run_admission_hooks(&self.pre_model_hooks, cancel).await?;
            self.base.execution_snapshot().unwrap_or(snapshot)
        } else {
            snapshot
        };
        self.base
            .before_model_call_with_raw_context(snapshot, messages, cancel)
            .await
    }
    async fn transform_agent_context(
        &self,
        live: &mut Vec<AgentMessage>,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<AgentMessage>>, crate::agent_message::RawContextError> {
        self.base.transform_agent_context(live, cancel).await
    }
    async fn convert_agent_messages_to_llm(
        &self,
        messages: &[AgentMessage],
    ) -> Result<Vec<Message>, crate::agent_message::RawContextError> {
        self.base.convert_agent_messages_to_llm(messages).await
    }

    fn actual_tool_source(
        &self,
        values: &crate::provider_context::SessionValues,
    ) -> Result<
        Option<Arc<dyn crate::tool::ActualToolSource>>,
        crate::provider_context::ActualProviderError,
    > {
        self.base.actual_tool_source(values)
    }

    async fn convert_agent_messages_to_llm_actual(
        &self,
        messages: &[AgentMessage],
        values: &crate::provider_context::SessionValues,
    ) -> Result<Vec<ara_rpc::value_heap::ValueHandle>, crate::provider_context::ActualProviderError>
    {
        self.base
            .convert_agent_messages_to_llm_actual(messages, values)
            .await
    }
    async fn transform_provider_context_actual(
        &self,
        context: crate::provider_context::ActualProviderContext,
        model: &Model,
        values: &crate::provider_context::SessionValues,
    ) -> Result<
        crate::provider_context::ActualProviderContext,
        crate::provider_context::ActualProviderError,
    > {
        self.base
            .transform_provider_context_actual(context, model, values)
            .await
    }
    async fn transform_assistant_graph(
        &self,
        owner: &AgentMessage,
        cancel: &CancellationToken,
        values: &crate::provider_context::SessionValues,
    ) -> Result<(), crate::provider_context::ActualProviderError> {
        self.base
            .transform_assistant_graph(owner, cancel, values)
            .await
    }
    fn supports_actual_turn_end(&self) -> bool {
        self.base.supports_actual_turn_end()
    }
    fn supports_actual_tool_hooks(&self) -> bool {
        self.base.supports_actual_tool_hooks()
    }
    async fn before_tool_dispatch_actual(
        &self,
        context: crate::agent_loop::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<
        Option<ara_rpc::value_heap::ValueHandle>,
        crate::provider_context::ActualProviderError,
    > {
        self.base.before_tool_dispatch_actual(context, cancel).await
    }
    async fn before_tool_call_actual(
        &self,
        context: crate::agent_loop::ActualBeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<crate::agent_loop::ActualToolDecision, crate::provider_context::ActualProviderError>
    {
        self.base.before_tool_call_actual(context, cancel).await
    }
    async fn after_tool_call_actual(
        &self,
        context: crate::agent_loop::ActualAfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<
        Option<ara_rpc::value_heap::ValueHandle>,
        crate::provider_context::ActualProviderError,
    > {
        self.base.after_tool_call_actual(context, cancel).await
    }
    fn transform_tool_call_arguments_actual(
        &self,
        args: ara_rpc::value_heap::ValueHandle,
        name: &ara_rpc::WireString,
        values: &crate::provider_context::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, crate::provider_context::ActualProviderError>
    {
        self.base
            .transform_tool_call_arguments_actual(args, name, values)
    }
    fn get_tool_context_actual(
        &self,
        context: &crate::tool::ActualToolExecutionContext,
        values: &crate::provider_context::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, crate::provider_context::ActualProviderError>
    {
        self.base.get_tool_context_actual(context, values)
    }
    fn tool_error_message_actual(
        &self,
        error: &ara_rpc::value_heap::ValueError,
        values: &crate::provider_context::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, crate::provider_context::ActualProviderError>
    {
        self.base.tool_error_message_actual(error, values)
    }
    fn sanitize_tool_result_text_actual(
        &self,
        text: &ara_rpc::value_heap::ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<ara_rpc::value_heap::ValueHandle, crate::provider_context::ActualProviderError>
    {
        self.base.sanitize_tool_result_text_actual(text, values)
    }
    fn soft_tool_satisfies_actual(
        &self,
        requirement: &crate::soft_tool_requirement::SoftToolRequirement,
        call: &ara_rpc::value_heap::ValueHandle,
        values: &crate::provider_context::SessionValues,
    ) -> Result<bool, crate::provider_context::ActualProviderError> {
        self.base
            .soft_tool_satisfies_actual(requirement, call, values)
    }
    async fn turn_end_actual_context(
        &self,
        context: crate::agent_loop::ActualTurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Vec<AgentMessage>, crate::provider_context::ActualProviderError> {
        self.base.turn_end_actual_context(context, cancel).await
    }
    async fn turn_end_actual(
        &self,
        owner: &AgentMessage,
        context: &[AgentMessage],
        cancel: &CancellationToken,
        values: &crate::provider_context::SessionValues,
    ) -> Result<Vec<AgentMessage>, crate::provider_context::ActualProviderError> {
        self.base
            .turn_end_actual(owner, context, cancel, values)
            .await
    }

    async fn before_tool_call(
        &self,
        call: &ToolCall,
        args: &JsonObject,
        cancel: &CancellationToken,
    ) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }
    async fn before_tool_dispatch(
        &self,
        context: &mut BeforeToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<BeforeToolCallResult, ToolHookError> {
        self.base.before_tool_dispatch(context, cancel).await
    }
    async fn after_tool_call(
        &self,
        context: AfterToolCallContext<'_>,
        cancel: &CancellationToken,
    ) -> Result<Option<AfterToolCallResult>, ToolHookError> {
        self.base.after_tool_call(context, cancel).await
    }
    async fn transform_assistant_message(
        &self,
        message: &mut AssistantMessage,
        cancel: &CancellationToken,
    ) -> Result<(), ToolHookError> {
        self.base.transform_assistant_message(message, cancel).await
    }
    fn resolve_fallback_tool(&self, name: &str) -> Option<Arc<dyn AgentTool>> {
        self.base.resolve_fallback_tool(name)
    }
    fn transform_tool_call_arguments(
        &self,
        args: JsonObject,
        name: &str,
    ) -> Result<JsonObject, ToolHookError> {
        self.base.transform_tool_call_arguments(args, name)
    }
    fn steering_interrupt_binding(&self) -> Option<SteeringInterruptBinding> {
        let base = self.base.steering_interrupt_binding();
        let requires_polling = base
            .as_ref()
            .is_some_and(SteeringInterruptBinding::requires_polling);
        let wake = Arc::new(AgentQueueWake {
            owned: self.steering_wake.clone(),
            base: base.as_ref().and_then(|b| b.wake_port().cloned()),
        });
        let peek = Arc::new(AgentQueuePeek {
            queues: self.queues.clone(),
            base,
        });
        Some(SteeringInterruptBinding::composite(
            peek,
            wake,
            requires_polling,
        ))
    }
    fn irc_interrupt_port(&self) -> Option<Arc<dyn IrcInterruptPeek>> {
        self.base.irc_interrupt_port()
    }
    fn queued_input_pending(&self, kind: QueueInputKind) -> Option<bool> {
        let owned = {
            let q = self.queues.lock().unwrap();
            match kind {
                QueueInputKind::Steering => !q.steering.is_empty(),
                QueueInputKind::FollowUp => !q.follow_up.is_empty(),
            }
        };
        if owned {
            Some(true)
        } else {
            self.base.queued_input_pending(kind)
        }
    }

    async fn aside_messages(&self) -> Result<Vec<AsideMessage>, AsideError> {
        let provider = self.aside_provider.read().unwrap().clone();
        match provider {
            Some(Some(provider)) => provider.messages().await,
            Some(None) => Ok(Vec::new()),
            None => self.base.aside_messages().await,
        }
    }

    async fn before_yield(&self) -> Result<(), AsideError> {
        let hook = self.before_yield.read().unwrap().clone();
        match hook {
            Some(Some(hook)) => hook.before_yield().await,
            Some(None) => Ok(()),
            None => self.base.before_yield().await,
        }
    }

    async fn steering_raw_messages(&self) -> Vec<AgentMessage> {
        if self
            .skip_initial_steering_poll
            .swap(false, Ordering::AcqRel)
        {
            return Vec::new();
        }
        let from_base = self.base.steering_raw_messages().await;
        let mut result = self.take_steering_raw_messages();
        result.extend(from_base);
        result
    }

    async fn follow_up_raw_messages(&self) -> Vec<AgentMessage> {
        let from_base = self.base.follow_up_raw_messages().await;
        let mut result = self.queues.lock().unwrap().take_follow_up();
        result.extend(from_base);
        result
    }

    async fn poll_queued_raw_messages_with_context(
        &self,
        kind: QueueInputKind,
        snapshot: ExecutionSnapshot,
        messages: Vec<AgentMessage>,
        cancel: &CancellationToken,
        deadline: Option<std::time::Instant>,
    ) -> Result<Vec<AgentMessage>, AdmissionError> {
        if kind == QueueInputKind::Steering
            && self
                .skip_initial_steering_poll
                .swap(false, Ordering::AcqRel)
        {
            await_admission(
                self.base.before_queued_input_dequeue(cancel),
                cancel,
                deadline,
            )
            .await?;
            return Ok(Vec::new());
        }
        let owned_pending = {
            let queues = self.queues.lock().unwrap();
            match kind {
                QueueInputKind::Steering => !queues.steering.is_empty(),
                QueueInputKind::FollowUp => !queues.follow_up.is_empty(),
            }
        };
        let external_pending = self.base.queued_input_pending(kind) != Some(false);
        if owned_pending {
            await_admission(
                run_admission_hooks(&self.pre_dequeue_hooks, cancel),
                cancel,
                deadline,
            )
            .await?;
        }
        if owned_pending || external_pending {
            await_admission(
                self.base
                    .before_queued_input_dequeue_with_raw_context(snapshot, messages, cancel),
                cancel,
                deadline,
            )
            .await?;
        } else {
            await_admission(
                self.base.before_queued_input_dequeue(cancel),
                cancel,
                deadline,
            )
            .await?;
        }
        let from_base = if external_pending {
            match kind {
                QueueInputKind::Steering => self.base.steering_raw_messages().await,
                QueueInputKind::FollowUp => self.base.follow_up_raw_messages().await,
            }
        } else {
            Vec::new()
        };
        // An empty peek grants no ownership of a late arrival. After an
        // external consuming callback, retain its returned inputs even if
        // cancellation raced; only the still-owned internal queue stays put.
        let mut result = if owned_pending
            && !cancel.is_cancelled()
            && deadline.is_none_or(|d| std::time::Instant::now() < d)
        {
            match kind {
                QueueInputKind::Steering => self.take_steering_raw_messages(),
                QueueInputKind::FollowUp => self.queues.lock().unwrap().take_follow_up(),
            }
        } else {
            Vec::new()
        };
        result.extend(from_base);
        Ok(result)
    }

    async fn transform_provider_context(&self, context: Context, model: &Model) -> Context {
        self.base.transform_provider_context(context, model).await
    }

    async fn turn_end_raw_messages_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentMessage> {
        self.base
            .turn_end_raw_messages_with_context(context, cancel)
            .await
    }
    async fn turn_end_inputs(
        &self,
        message: &ara_ai::AssistantMessage,
        tool_results: &[ara_ai::ToolResultMessage],
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.base
            .turn_end_inputs(message, tool_results, cancel)
            .await
    }
    async fn turn_end_inputs_with_context(
        &self,
        context: TurnEndContext<'_>,
        cancel: &CancellationToken,
    ) -> Vec<AgentInput> {
        self.base
            .turn_end_inputs_with_context(context, cancel)
            .await
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use crate::{NoHooks, NullSink};
    use ara_ai::event::EventSink;
    use ara_ai::{
        AssistantBlock, AssistantMessage, AssistantMessageEvent, AssistantStream, CallOptions,
        ModelProvider, StopReason, ToolResultMessage, UserBlock, UserMessage,
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
        fn stream(
            &self,
            model: &Model,
            _context: &Context,
            _options: CallOptions,
        ) -> AssistantStream {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (sink, stream) = EventSink::channel();
            let mut message = AssistantMessage::empty(&model.api, &model.provider, &model.id);
            let gate = self.gate.clone();
            tokio::spawn(async move {
                sink.push(AssistantMessageEvent::Start {
                    partial: message.clone(),
                })
                .await;
                if let Some(gate) = gate {
                    gate.started.notify_one();
                    gate.release.notified().await;
                }
                message.content.push(AssistantBlock::text("known answer"));
                message.stop_reason = StopReason::Stop;
                sink.push(AssistantMessageEvent::Done {
                    reason: StopReason::Stop,
                    message,
                })
                .await;
            });
            stream
        }
    }

    fn fixture(messages: Vec<Message>, gate: Option<Arc<Gate>>) -> (Arc<Agent>, Arc<Provider>) {
        let provider = Arc::new(Provider {
            calls: AtomicUsize::new(0),
            gate,
        });
        let agent = Agent::new(
            AgentConfig {
                model: Model {
                    id: "idle-fixture".into(),
                    api: "openai-completions".into(),
                    provider: "fixture".into(),
                    base_url: String::new(),
                    reasoning: false,
                    max_tokens: None,
                    context_window: None,
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
                interrupt_mode: InterruptMode::Immediate,
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
                    ..Default::default()
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
            provider_metadata: None,
            useless: false,
            timestamp: 1,
        })
    }

    #[tokio::test]
    async fn idle_append_and_replace_preserve_queue_envelopes_and_modes_without_running_model() {
        let (agent, provider) = fixture(vec![user("initial")], None);
        let steering = AgentInput {
            model: user("steer"),
            provenance: Some(Arc::new(serde_json::json!({"origin":"s"}))),
        };
        let follow = AgentInput {
            model: user("follow"),
            provenance: Some(Arc::new(serde_json::json!({"origin":"f"}))),
        };
        agent.steer_input(steering.clone());
        agent.follow_up_input(follow.clone());
        agent.set_steering_mode(QueueMode::All);
        agent.set_follow_up_mode(QueueMode::OneAtATime);
        let appended = user("external projection");
        agent.append_idle_message(appended.clone()).unwrap();
        assert_eq!(agent.messages().await.unwrap().len(), 2);
        agent.replace_idle_messages(vec![appended.clone()]).unwrap();
        assert_eq!(agent.messages().await.unwrap(), vec![appended]);
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
            assert!(matches!(
                agent.append_idle_message(user("blocked")),
                Err(AgentError::Busy)
            ));
            assert!(matches!(
                agent.replace_idle_messages(Vec::new()),
                Err(AgentError::Busy)
            ));
        }
        let running = agent.clone();
        let task = tokio::spawn(async move {
            running
                .prompt(
                    vec![user("run")],
                    CancellationToken::new(),
                    Arc::new(NullSink),
                )
                .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), gate.started.notified())
            .await
            .unwrap();
        assert!(agent.is_busy());
        assert!(matches!(
            agent.append_idle_message(user("during run")),
            Err(AgentError::Busy)
        ));
        assert!(matches!(
            agent.replace_idle_messages(Vec::new()),
            Err(AgentError::Busy)
        ));
        gate.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!agent.is_busy());
        agent.append_idle_message(user("after joined Run")).unwrap();
        let messages = agent.messages().await.unwrap();
        assert_eq!(messages.len(), 4);
        assert!(
            matches!(&messages[3], Message::User(message) if message.content.plain_text() == "after joined Run")
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn recovery_state_cannot_be_cleared_by_idle_mutation_or_replacement() {
        let original = user("original");
        let (agent, provider) = fixture(vec![original.clone()], None);
        agent.run_state.lock().unwrap().needs_recovery = true;
        assert!(matches!(
            agent.append_idle_message(user("hide")),
            Err(AgentError::NeedsRecovery)
        ));
        assert!(matches!(
            agent.replace_idle_messages(Vec::new()),
            Err(AgentError::NeedsRecovery)
        ));
        assert_eq!(agent.messages().await.unwrap(), vec![original]);
        assert!(agent.run_state.lock().unwrap().needs_recovery);
        assert!(matches!(
            agent
                .prompt(
                    vec![user("next")],
                    CancellationToken::new(),
                    Arc::new(NullSink)
                )
                .await,
            Err(AgentError::NeedsRecovery)
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn idle_append_cannot_hide_unpaired_tail_and_next_prompt_never_calls_provider_or_replays()
    {
        let original = unpaired(&["unknown-effect"]);
        let (agent, provider) = fixture(vec![original.clone()], None);
        agent.follow_up(user("still queued"));
        assert!(matches!(
            agent.append_idle_message(user("hide tail")),
            Err(AgentError::CannotContinue(_))
        ));
        assert!(matches!(
            agent.replace_idle_messages(vec![user("hide tail")]),
            Err(AgentError::CannotContinue(_))
        ));
        assert_eq!(agent.messages().await.unwrap(), vec![original]);
        assert!(matches!(
            agent
                .prompt(
                    vec![user("next")],
                    CancellationToken::new(),
                    Arc::new(NullSink)
                )
                .await,
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
        assert_eq!(agent.messages().await.unwrap(), vec![original]);
        let paired = vec![
            unpaired(&["known-1", "known-2"]),
            result("known-1"),
            result("known-2"),
            user("later"),
        ];
        agent.replace_idle_messages(paired.clone()).unwrap();
        assert_eq!(agent.messages().await.unwrap(), paired);
        let interleaved_known = vec![
            unpaired(&["known"]),
            user("historical external receipt"),
            result("known"),
        ];
        agent
            .replace_idle_messages(interleaved_known.clone())
            .unwrap();
        assert_eq!(agent.messages().await.unwrap(), interleaved_known);
        let (partial, _) = fixture(vec![unpaired(&["known", "unknown"]), result("known")], None);
        assert!(matches!(
            partial.append_idle_message(user("hide partial")),
            Err(AgentError::CannotContinue(_))
        ));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}
