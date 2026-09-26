//! Stateful owner for one agent transcript and its message queues.
//! OMP source: `packages/agent/src/agent.ts` at the locked upstream commit.

use crate::agent_loop::{
    AgentConfig, LoopError, LoopHooks, RunReport, UnpairedTail, agent_loop, agent_loop_continue,
    unpaired_tool_call_tail,
};
use crate::event::AgentEventSink;
use crate::tool::ToolDecision;
use ara_ai::{Context, JsonObject, Message, Model, ToolCall};
use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

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
    steering: VecDeque<Message>,
    follow_up: VecDeque<Message>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
}

impl Queues {
    fn take_steering(&mut self) -> Vec<Message> {
        match self.steering_mode {
            QueueMode::All => self.steering.drain(..).collect(),
            QueueMode::OneAtATime => self.steering.pop_front().into_iter().collect(),
        }
    }

    fn take_follow_up(&mut self) -> Vec<Message> {
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

    pub fn steer(&self, message: Message) {
        self.queues.lock().unwrap().steering.push_back(message);
    }

    pub fn follow_up(&self, message: Message) {
        self.queues.lock().unwrap().follow_up.push_back(message);
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
        self.queues.lock().unwrap().steering.iter().cloned().collect()
    }

    pub fn peek_follow_up_queue(&self) -> Vec<Message> {
        self.queues.lock().unwrap().follow_up.iter().cloned().collect()
    }

    pub fn pop_last_steer(&self) -> Option<Message> {
        self.queues.lock().unwrap().steering.pop_back()
    }

    pub fn pop_last_follow_up(&self) -> Option<Message> {
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

    fn config_with_queues(&self) -> (AgentConfig, Arc<QueueHooks>) {
        let mut config = self.config.clone();
        let hooks = Arc::new(QueueHooks {
            queues: self.queues.clone(),
            base: self.config.hooks.clone(),
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
            let (config, _) = agent.config_with_queues();
            Ok(agent_loop(prompts, &mut messages, &config, &cancel, sink.as_ref()).await)
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
        let cancel = self.enter(&cancel)?;
        let agent = self.clone();
        tokio::spawn(async move {
            let _guard = RunningGuard(agent.clone());
            let mut messages = agent.messages.lock().await;
            let (config, queue_hooks) = agent.config_with_queues();
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
                let mut prompts = config.hooks.steering_messages().await;
                if prompts.is_empty() {
                    steering = false;
                    prompts = config.hooks.follow_up_messages().await;
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
                Ok(agent_loop(prompts, &mut messages, &config, &cancel, sink.as_ref()).await)
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

struct QueueHooks {
    queues: Arc<Mutex<Queues>>,
    base: Arc<dyn LoopHooks>,
    skip_initial_steering_poll: AtomicBool,
}

#[async_trait]
impl LoopHooks for QueueHooks {
    async fn before_tool_call(&self, call: &ToolCall, args: &JsonObject, cancel: &CancellationToken) -> ToolDecision {
        self.base.before_tool_call(call, args, cancel).await
    }

    async fn steering_messages(&self) -> Vec<Message> {
        if self.skip_initial_steering_poll.swap(false, Ordering::AcqRel) {
            return Vec::new();
        }
        let from_base = self.base.steering_messages().await;
        let mut result = self.queues.lock().unwrap().take_steering();
        result.extend(from_base);
        result
    }

    async fn follow_up_messages(&self) -> Vec<Message> {
        let from_base = self.base.follow_up_messages().await;
        let mut result = self.queues.lock().unwrap().take_follow_up();
        result.extend(from_base);
        result
    }

    async fn transform_provider_context(&self, context: Context, model: &Model) -> Context {
        self.base.transform_provider_context(context, model).await
    }
}
