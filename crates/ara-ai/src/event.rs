//! Assistant message stream protocol (OMP `AssistantMessageEvent`).
//!
//! A started response emits `start`, then block lifecycle events, then one
//! terminal `done` or `error` event. A pre-response failure can emit only its
//! terminal `error` event. Every stream has exactly one terminal event.
//! `partial` is the provider's current snapshot of the assistant message.

use crate::types::{AssistantBlock, AssistantMessage, ImageContent, StopReason, ToolCall};
use tokio::sync::mpsc;

#[derive(Clone, Debug, PartialEq)]
pub enum AssistantMessageEvent<
    M = AssistantMessage,
    S = String,
    C = ToolCall,
    I = ImageContent,
    D = S,
    X = usize,
> {
    Start {
        partial: M,
    },
    TextStart {
        content_index: X,
        partial: M,
    },
    TextDelta {
        content_index: X,
        delta: S,
        partial: M,
    },
    TextEnd {
        content_index: X,
        content: S,
        partial: M,
    },
    ThinkingStart {
        content_index: X,
        partial: M,
    },
    ThinkingDelta {
        content_index: X,
        delta: S,
        partial: M,
    },
    ThinkingEnd {
        content_index: X,
        content: S,
        partial: M,
    },
    ImageEnd {
        content_index: X,
        content: I,
        partial: M,
    },
    ToolcallStart {
        content_index: X,
        partial: M,
    },
    ToolcallDelta {
        content_index: X,
        delta: D,
        partial: M,
    },
    ToolcallEnd {
        content_index: X,
        tool_call: C,
        partial: M,
    },
    /// `reason` is `stop`, `length` or `toolUse`.
    Done {
        reason: StopReason,
        message: M,
    },
    /// `reason` is `error` or `aborted`.
    Error {
        reason: StopReason,
        error: M,
    },
}

impl<M, S, C, I, D, X> AssistantMessageEvent<M, S, C, I, D, X> {
    /// Keep the provider's original index value, including native indexOf's
    /// -1 and programmable method results. This accessor performs no coercion.
    pub fn content_index(&self) -> Option<&X> {
        match self {
            Self::TextStart { content_index, .. }
            | Self::TextDelta { content_index, .. }
            | Self::TextEnd { content_index, .. }
            | Self::ThinkingStart { content_index, .. }
            | Self::ThinkingDelta { content_index, .. }
            | Self::ThinkingEnd { content_index, .. }
            | Self::ImageEnd { content_index, .. }
            | Self::ToolcallStart { content_index, .. }
            | Self::ToolcallDelta { content_index, .. }
            | Self::ToolcallEnd { content_index, .. } => Some(content_index),
            Self::Start { .. } | Self::Done { .. } | Self::Error { .. } => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Start { .. } => "start",
            Self::TextStart { .. } => "text_start",
            Self::TextDelta { .. } => "text_delta",
            Self::TextEnd { .. } => "text_end",
            Self::ThinkingStart { .. } => "thinking_start",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ThinkingEnd { .. } => "thinking_end",
            Self::ImageEnd { .. } => "image_end",
            Self::ToolcallStart { .. } => "toolcall_start",
            Self::ToolcallDelta { .. } => "toolcall_delta",
            Self::ToolcallEnd { .. } => "toolcall_end",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Error { .. })
    }

    /// Current snapshot carried by the event.
    pub fn partial(&self) -> &M {
        match self {
            Self::Start { partial }
            | Self::TextStart { partial, .. }
            | Self::TextDelta { partial, .. }
            | Self::TextEnd { partial, .. }
            | Self::ThinkingStart { partial, .. }
            | Self::ThinkingDelta { partial, .. }
            | Self::ThinkingEnd { partial, .. }
            | Self::ImageEnd { partial, .. }
            | Self::ToolcallStart { partial, .. }
            | Self::ToolcallDelta { partial, .. }
            | Self::ToolcallEnd { partial, .. } => partial,
            Self::Done { message, .. } => message,
            Self::Error { error, .. } => error,
        }
    }
}

impl AssistantMessageEvent {
    /// Printable form without the partial snapshot (OMP `printableEvent`).
    pub fn printable(&self) -> serde_json::Value {
        use serde_json::json;
        match self {
            Self::Start { .. } => json!({"type": "start"}),
            Self::TextStart { content_index, .. }
            | Self::ThinkingStart { content_index, .. }
            | Self::ToolcallStart { content_index, .. } => {
                json!({"type": self.type_name(), "contentIndex": content_index})
            }
            Self::TextDelta {
                content_index,
                delta,
                ..
            }
            | Self::ThinkingDelta {
                content_index,
                delta,
                ..
            }
            | Self::ToolcallDelta {
                content_index,
                delta,
                ..
            } => {
                json!({"type": self.type_name(), "contentIndex": content_index, "delta": delta})
            }
            Self::TextEnd {
                content_index,
                content,
                ..
            }
            | Self::ThinkingEnd {
                content_index,
                content,
                ..
            } => {
                json!({"type": self.type_name(), "contentIndex": content_index, "content": content})
            }
            Self::ImageEnd {
                content_index,
                content,
                ..
            } => {
                json!({"type": "image_end", "contentIndex": content_index, "content": AssistantBlock::Image(content.clone())})
            }
            Self::ToolcallEnd {
                content_index,
                tool_call,
                ..
            } => {
                json!({"type": "toolcall_end", "contentIndex": content_index, "toolCall": tool_call})
            }
            Self::Done { reason, .. } | Self::Error { reason, .. } => {
                json!({"type": self.type_name(), "reason": reason})
            }
        }
    }
}

/// Actual payload uses the same lifecycle vocabulary and original handles.
pub type ActualAssistantEvent = AssistantMessageEvent<
    ara_rpc::value_heap::ValueHandle,
    ara_rpc::WireString,
    ara_rpc::value_heap::ValueHandle,
    ara_rpc::value_heap::ValueHandle,
    ara_rpc::value_heap::ValueHandle,
    ara_rpc::value_heap::ValueHandle,
>;
/// Host integration failures preserve their original exception handles. This
/// receipt is independent of channel delivery: a closed consumer cannot erase
/// a failure which happened while constructing or publishing an actual root.
#[derive(Clone, Default)]
pub struct ActualStreamFailureReceipt {
    state: std::sync::Arc<std::sync::Mutex<ActualStreamFailureState>>,
    settlement: std::sync::Arc<tokio::sync::Notify>,
}
#[derive(Default)]
struct ActualStreamFailureState {
    failures: Vec<std::sync::Arc<ara_rpc::value_heap::ValueError>>,
    known_terminals: Vec<ActualAssistantEvent>,
    inherited: Vec<ActualStreamFailureReceipt>,
    sealed: bool,
}
/// The producer's settlement owner. Keep this guard alive until its actual
/// operation has released every resource; dropping a consumer is not settlement.
pub struct ActualStreamSettlementGuard(ActualStreamFailureReceipt);
impl Drop for ActualStreamSettlementGuard {
    fn drop(&mut self) {
        self.0.seal();
    }
}
impl ActualStreamFailureReceipt {
    /// Known model outcomes survive cancelled/full/closed event delivery. This
    /// retains the original event and handles, not a reconstructed transcript.
    pub fn retain_terminal(&self, event: &ActualAssistantEvent) {
        if event.is_terminal() {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.known_terminals.last().is_some_and(|known| {
                known.type_name() == event.type_name()
                    && known.partial().same_identity(event.partial())
            }) {
                return;
            }
            state.known_terminals.push(event.clone());
        }
    }
    /// Only this stream's selected terminals, never a suppressed wire attempt
    /// inherited from another owner. Recursive settlement remains separate.
    pub fn known_terminals(&self) -> Vec<ActualAssistantEvent> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .known_terminals
            .clone()
    }
    /// Link a dispatched attempt's real owner, including failures recorded after
    /// its event channel closes. Host wrappers must retain every attempted owner.
    pub fn inherit_stream(&self, source: &ActualAssistantStream) {
        self.inherit(source.failure_receipt());
    }

    /// Create before spawning and move into the producer future, so even an
    /// unpolled/dropped future releases this wrapper's own settlement owner.
    pub fn owner_settlement_guard(&self) -> ActualStreamSettlementGuard {
        self.settlement_guard()
    }

    pub fn record(&self, error: ara_rpc::value_heap::ValueError) {
        self.record_shared(std::sync::Arc::new(error));
    }
    pub fn record_shared(&self, error: std::sync::Arc<ara_rpc::value_heap::ValueError>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failures
            .push(error);
    }
    pub(crate) fn inherit(&self, source: ActualStreamFailureReceipt) {
        // Only the representation wrapper adds its own inner attempt receipt.
        // Linking the owner (not copying today's snapshot) keeps late failures
        // visible after cancellation or receiver closure, without waiting for
        // an uncooperative callback or introducing a Session/runtime cycle.
        if !std::sync::Arc::ptr_eq(&self.state, &source.state) {
            self.state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .inherited
                .push(source);
        }
    }
    pub fn failures(&self) -> Vec<std::sync::Arc<ara_rpc::value_heap::ValueError>> {
        let (mut failures, inherited) = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (state.failures.clone(), state.inherited.clone())
        };
        for source in inherited {
            failures.extend(source.failures());
        }
        failures
    }
    pub(crate) fn settlement_guard(&self) -> ActualStreamSettlementGuard {
        ActualStreamSettlementGuard(self.clone())
    }
    fn seal(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .sealed = true;
        self.settlement.notify_waiters();
    }
    /// Empty failures are acceptance evidence only after all original owners
    /// have settled. Cancellation/channel closure does not seal a provider.
    pub fn is_settled(&self) -> bool {
        let inherited = {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !state.sealed {
                return false;
            }
            state.inherited.clone()
        };
        inherited.iter().all(Self::is_settled)
    }
    async fn wait_owner_settled(&self) {
        loop {
            let notified = self.settlement.notified();
            tokio::pin!(notified);
            // Register before checking the latch; a seal between the latch
            // check and await cannot turn into a missed wake-up.
            notified.as_mut().enable();
            if self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .sealed
            {
                break;
            }
            notified.await;
        }
    }
    pub async fn wait_settled(&self) -> Vec<std::sync::Arc<ara_rpc::value_heap::ValueError>> {
        self.wait_owner_settled().await;
        let inherited = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inherited
            .clone();
        for source in inherited {
            Box::pin(source.wait_settled()).await;
        }
        self.failures()
    }
}

/// Fixed OMP event-stream.ts LocalWorkSource. A delegate may provide the actual
/// same-stream getter (including its original exception), rather than a copy.
pub trait LocalWorkSource: Send + Sync {
    fn has_pending_local_work(&self) -> Result<bool, ara_rpc::value_heap::ValueError>;
    /// Original Math.max consumes the raw Date.now/delegate values here.
    /// The borrowed execution is the issuing Run, never a stored runtime.
    fn local_work_settled_at_in(
        &self,
        execution: crate::graph_values::GraphExecution<'_>,
        globals: &ara_rpc::value_heap::ValueHandle,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>;
}

#[derive(Clone, Default)]
pub struct LocalWorkTracker {
    state: std::sync::Arc<LocalWorkState>,
}
#[derive(Default)]
struct LocalWorkState {
    pending: std::sync::atomic::AtomicUsize,
    settled_at: std::sync::Mutex<Option<(
        std::sync::Weak<ara_rpc::value_heap::ValueHeap>,
        ara_rpc::value_heap::DeferredValue,
    )>>,
    delegate: std::sync::Mutex<Option<std::sync::Arc<dyn LocalWorkSource>>>,
}
impl LocalWorkSource for LocalWorkTracker {
    fn has_pending_local_work(&self) -> Result<bool, ara_rpc::value_heap::ValueError> {
        if self
            .state
            .pending
            .load(std::sync::atomic::Ordering::Acquire)
            > 0
        {
            return Ok(true);
        }
        let delegate = self
            .state
            .delegate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        match delegate {
            Some(source) => source.has_pending_local_work(),
            None => Ok(false),
        }
    }
    fn local_work_settled_at_in(
        &self,
        execution: crate::graph_values::GraphExecution<'_>,
        globals: &ara_rpc::value_heap::ValueHandle,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError> {
        use ara_rpc::value_heap::ValueError;
        let heap = execution.heap();
        let math = heap.get(globals, &"Math".into(), execution.runtime())?;
        let maximum = heap.get(&math, &"max".into(), execution.runtime())?;
        // Snapshot local before observable delegate access, without holding a
        // mutex through user getters, Math.max or numeric coercion.
        let local = self.state.settled_at.lock()
            .map_err(|_| ValueError::PoisonedHeap)?.clone();
        let local = match local {
            Some((source, edge)) => {
                let source = source.upgrade().ok_or(ValueError::UnsupportedOperation("LocalWork timestamp heap expired"))?;
                if !std::sync::Arc::ptr_eq(&source, heap) { return Err(ValueError::ForeignHeap); }
                heap.restore_deferred(&edge)?
            }
            None => heap.number(0.0)?,
        };
        let delegate = self.state.delegate.lock()
            .map_err(|_| ValueError::PoisonedHeap)?.clone();
        let delegated = match delegate {
            Some(source) => source.local_work_settled_at_in(execution, globals)?,
            None => heap.number(0.0)?,
        };
        let delegated = if matches!(heap.kind(&delegated)?, ara_rpc::value_heap::ValueKind::Null | ara_rpc::value_heap::ValueKind::Undefined) {
            heap.number(0.0)?
        } else { delegated };
        heap.call(&maximum, &math, &[local, delegated], execution.runtime())
    }
}

/// A single original work's finally transition. No callback/runtime is retained
/// here. Losing an observer does not complete this ticket or decrement pending.
#[derive(Clone)]
pub struct LocalWorkTicket {
    state: std::sync::Arc<LocalWorkState>,
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl LocalWorkTicket {
    pub fn finish_in_original(
        &self,
        execution: crate::graph_values::GraphExecution<'_>,
        globals: &ara_rpc::value_heap::ValueHandle,
        original: Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>,
    ) -> crate::retry::NativeWorkObservation {
        use ara_rpc::value_heap::ValueError;
        use std::sync::atomic::Ordering;
        if self.finished.swap(true, Ordering::AcqRel) {
            return crate::retry::NativeWorkObservation {
                result: Err(ValueError::UnsupportedOperation("LocalWork finally already settled")),
                settled: false,
            };
        }
        let finally = (|| {
            if self.state.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
                let heap = execution.heap();
                let runtime = execution.runtime();
                let date = heap.get(globals, &"Date".into(), runtime)?;
                let now = heap.get(&date, &"now".into(), runtime)?;
                let raw = heap.call(&now, &date, &[], runtime)?;
                let edge = heap.capture_deferred(&raw)?;
                // No ToNumber, extra zero guard or generation check after a
                // reentrant Date.now: source assigns exactly this returned value.
                *self.state.settled_at.lock().map_err(|_| ValueError::PoisonedHeap)? =
                    Some((std::sync::Arc::downgrade(heap), edge));
            }
            Ok::<_, ValueError>(())
        })();
        crate::retry::NativeWorkObservation {
            result: match finally { Ok(()) => original, Err(error) => Err(error) },
            settled: true,
        }
    }
}
impl LocalWorkTracker {
    pub fn begin_local_work(&self) -> LocalWorkTicket {
        self.state.pending.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        LocalWorkTicket { state: self.state.clone(), finished: Default::default() }
    }
    /// Adoption occurs synchronously after increment. Only the admitted Host
    /// finally changes pending/timestamp. This worker publishes its wrapper.
    pub fn track_in_original<F>(&self, adopt: F) -> TrackedLocalWork
    where F: FnOnce(LocalWorkTicket) -> Result<
        futures::future::BoxFuture<'static, crate::retry::NativeWorkObservation>,
        ara_rpc::value_heap::ValueError,
    > {
        let ticket = self.begin_local_work();
        let result = TrackedLocalWork::default();
        match adopt(ticket) {
            Err(error) => result.publish(Err(error)),
            Ok(work) => {
                let completion = result.clone();
                tokio::spawn(async move { completion.publish(work.await.result); });
            }
        }
        result
    }
    pub fn forward_local_work_from(&self, source: Option<std::sync::Arc<dyn LocalWorkSource>>) {
        *self
            .state
            .delegate
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = source;
    }
    /// Native ticket observation distinguishes actual settlement from revoked
    /// admission or a missing carrier. Losing observation leaves work pending;
    /// its original error remains visible to every waiter.
    pub fn track_observed_local_work<F>(&self, adopt: F) -> TrackedLocalWork
    where
        F: FnOnce() -> Result<
            futures::future::BoxFuture<'static, crate::retry::NativeWorkObservation>,
            ara_rpc::value_heap::ValueError,
        >,
    {
        self.state
            .pending
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let result = TrackedLocalWork::default();
        match adopt() {
            Err(error) => result.publish(Err(error)),
            Ok(work) => {
                let state = self.state.clone();
                let completion = result.clone();
                tokio::spawn(async move {
                    let observation = work.await;
                    if observation.settled {
                        state
                            .pending
                            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    }
                    completion.publish(observation.result);
                });
            }
        }
        result
    }
    /// Increment before synchronous original Promise adoption. A then getter
    /// can consult this stream immediately and must already observe local work.
    /// The existing native driver alone settles its Promise jobs; this Rust
    /// owner waits for that ticket and never creates or pumps another driver.
    pub fn track_local_work<F>(&self, adopt: F) -> TrackedLocalWork
    where
        F: FnOnce() -> Result<
            futures::future::BoxFuture<
                'static,
                Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>,
            >,
            ara_rpc::value_heap::ValueError,
        >,
    {
        self.state
            .pending
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let result = TrackedLocalWork::default();
        match adopt() {
            Err(error) => {
                self.state
                    .pending
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                result.publish(Err(error));
            }
            Ok(work) => {
                let state = self.state.clone();
                let completion = result.clone();
                tokio::spawn(async move {
                    let outcome = work.await;
                    // finally belongs to actual settlement, not receiver Drop.
                    state
                        .pending
                        .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    completion.publish(outcome);
                });
            }
        }
        result
    }
}
/// The original work's retained result/rejection remains inspectable even when
/// one race waiter goes away. Values and thrown handles are never serialized.
#[derive(Clone, Default)]
pub struct TrackedLocalWork {
    state: std::sync::Arc<TrackedLocalWorkState>,
}
#[derive(Default)]
struct TrackedLocalWorkState {
    result: std::sync::Mutex<
        Option<Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>>,
    >,
    changed: tokio::sync::Notify,
}
impl TrackedLocalWork {
    fn publish(
        &self,
        result: Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError>,
    ) {
        *self
            .state
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(result);
        self.state.changed.notify_waiters();
    }
    pub async fn wait(
        &self,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError> {
        loop {
            let changed = self.state.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let settled = self
                .state
                .result
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            if let Some(settled) = settled {
                return settled;
            }
            changed.await;
        }
    }
}

enum ActualEventReceiver {
    Bounded(mpsc::Receiver<ActualAssistantEvent>),
    Source(mpsc::UnboundedReceiver<ActualAssistantEvent>),
}
impl ActualEventReceiver {
    async fn recv(&mut self) -> Option<ActualAssistantEvent> {
        match self {
            Self::Bounded(events) => events.recv().await,
            Self::Source(events) => events.recv().await,
        }
    }
    fn close(&mut self) {
        match self {
            Self::Bounded(events) => events.close(),
            Self::Source(events) => events.close(),
        }
    }
    fn try_recv(&mut self) -> Result<ActualAssistantEvent, mpsc::error::TryRecvError> {
        match self {
            Self::Bounded(events) => events.try_recv(),
            Self::Source(events) => events.try_recv(),
        }
    }
}
pub struct ActualAssistantStream {
    events: ActualEventReceiver,
    failures: ActualStreamFailureReceipt,
    // Only wrappers whose close branch drains synchronously may opt in. The
    // original producer's durable/callback settlement must never be awaited
    // merely to take a snapshot of already-known cancellation evidence.
    close_draining_owner: bool,
    local_work: LocalWorkTracker,
}
impl ActualAssistantStream {
    pub fn local_work_tracker(&self) -> LocalWorkTracker {
        self.local_work.clone()
    }
    pub fn has_pending_local_work(&self) -> Result<bool, ara_rpc::value_heap::ValueError> {
        self.local_work.has_pending_local_work()
    }
    pub fn forward_local_work_from(&self, source: Option<std::sync::Arc<dyn LocalWorkSource>>) {
        self.local_work.forward_local_work_from(source);
    }
    pub async fn recv(&mut self) -> Option<ActualAssistantEvent> {
        self.events.recv().await
    }
    pub fn close(&mut self) {
        self.events.close();
    }
    pub fn try_recv(&mut self) -> Result<ActualAssistantEvent, mpsc::error::TryRecvError> {
        self.events.try_recv()
    }
    /// Cancelled callers inspect the whole ready queue, not just its first
    /// nonterminal. A controlled wrapper gets a chance to retain its own ready
    /// input terminal before it drops that input. This never waits for an
    /// uncooperative original producer or its inherited durable owner.
    pub async fn take_terminal_on_close(&mut self) -> Option<ActualAssistantEvent> {
        self.close();
        let mut terminal = None;
        while let Ok(event) = self.try_recv() {
            if event.is_terminal() {
                terminal = Some(event);
            }
        }
        if self.close_draining_owner {
            self.failures.wait_owner_settled().await;
            while let Ok(event) = self.try_recv() {
                if event.is_terminal() {
                    terminal = Some(event);
                }
            }
        }
        let terminal = terminal.or_else(|| self.failures.known_terminals().last().cloned());
        if let Some(event) = &terminal {
            self.failures.retain_terminal(event);
        }
        terminal
    }
    pub fn failure_receipt(&self) -> ActualStreamFailureReceipt {
        self.failures.clone()
    }
    pub fn integration_failures(&self) -> Vec<std::sync::Arc<ara_rpc::value_heap::ValueError>> {
        self.failures.failures()
    }
    pub fn channel() -> (
        EventSink<ActualAssistantEvent>,
        Self,
        ActualStreamFailureReceipt,
    ) {
        let (sink, events) = EventSink::channel();
        let failures = ActualStreamFailureReceipt::default();
        (
            sink,
            Self {
                events: ActualEventReceiver::Bounded(events),
                failures: failures.clone(),
                close_draining_owner: false,
                local_work: LocalWorkTracker::default(),
            },
            failures,
        )
    }
    /// Original source EventStream.push queues without consumer backpressure.
    /// Selected utility result() can settle before any event consumer exists;
    /// ordinary provider channels retain their existing bounded256 policy.
    pub(crate) fn source_channel() -> (
        EventSink<ActualAssistantEvent>,
        Self,
        ActualStreamFailureReceipt,
    ) {
        let (sink, events) = EventSink::source_channel();
        let failures = ActualStreamFailureReceipt::default();
        (
            sink,
            Self {
                events: ActualEventReceiver::Source(events),
                failures: failures.clone(),
                close_draining_owner: false,
                local_work: LocalWorkTracker::default(),
            },
            failures,
        )
    }
    /// This crate's short forwarding owners drain ready inputs on receiver
    /// closure, with no Host/producer settlement await in that close branch.
    pub(crate) fn channel_with_close_drain() -> (
        EventSink<ActualAssistantEvent>,
        Self,
        ActualStreamFailureReceipt,
    ) {
        let (sink, mut stream, failures) = Self::channel();
        stream.close_draining_owner = true;
        (sink, stream, failures)
    }
}
impl From<mpsc::Receiver<ActualAssistantEvent>> for ActualAssistantStream {
    fn from(events: mpsc::Receiver<ActualAssistantEvent>) -> Self {
        // Explicit legacy fixture adoption has no actual failure producer.
        // Its event channel still owns normal lifecycle/EOF observation.
        let failures = ActualStreamFailureReceipt::default();
        failures.seal();
        Self {
            events: ActualEventReceiver::Bounded(events),
            failures,
            close_draining_owner: false,
            local_work: LocalWorkTracker::default(),
        }
    }
}

impl ActualAssistantEvent {
    /// A fresh public event object retains original field handles. Serializing
    /// the whole object gives native toJSON its actual property key, and native
    /// undefined omission, instead of exporting each field separately.
    pub fn printable_graph(
        &self,
        runtime: &dyn ara_rpc::value_heap::ValueRuntime,
    ) -> Result<ara_rpc::value_heap::ValueHandle, ara_rpc::value_heap::ValueError> {
        let heap = self.partial().heap();
        let public = heap.object()?;
        let put = |key: &str, value: ara_rpc::value_heap::ValueHandle| {
            heap.adopt(&value)?;
            heap.define_own_data(&public, key.into(), value, runtime)
        };
        put("type", heap.string(self.type_name().into())?)?;
        if let Some(index) = self.content_index() {
            put("contentIndex", index.clone())?;
        }
        match self {
            Self::TextDelta { delta, .. } | Self::ThinkingDelta { delta, .. } => {
                put("delta", heap.string(delta.clone())?)?;
            }
            Self::ToolcallDelta { delta, .. } => put("delta", delta.clone())?,
            Self::TextEnd { content, .. } | Self::ThinkingEnd { content, .. } => {
                put("content", heap.string(content.clone())?)?;
            }
            Self::ImageEnd { content, .. } => put("content", content.clone())?,
            Self::ToolcallEnd { tool_call, .. } => put("toolCall", tool_call.clone())?,
            Self::Done { reason, .. } | Self::Error { reason, .. } => {
                let reason = match reason {
                    StopReason::Stop => "stop",
                    StopReason::Length => "length",
                    StopReason::ToolUse => "toolUse",
                    StopReason::Error => "error",
                    StopReason::Aborted => "aborted",
                };
                put("reason", heap.string(reason.into())?)?;
            }
            _ => {}
        }
        Ok(public)
    }

    /// Checked standalone display bridge. Hosts with programmable values use
    /// printable_graph and their current runtime; missing capabilities here
    /// fail explicitly and never approximate an original native field.
    pub fn try_printable(&self) -> Result<serde_json::Value, ara_rpc::value_heap::ValueError> {
        use ara_rpc::value_heap::{NoRuntime, StringifyMode, ValueError};
        let public = self.printable_graph(&NoRuntime)?;
        let json = self
            .partial()
            .heap()
            .stringify(&public, StringifyMode::Plain, None, &NoRuntime)?
            .ok_or(ValueError::TypeError(
                "Actual public event root is undefined".into(),
            ))?;
        let text = json.to_utf8().map_err(|_| {
            ValueError::LegacyBridge("Actual public JSON is not a Unicode scalar string")
        })?;
        serde_json::from_str(&text)
            .map_err(|_| ValueError::LegacyBridge("Actual public JSON cannot enter display"))
    }
}
/// Receiving side of a provider stream.
pub type AssistantStream = mpsc::Receiver<AssistantMessageEvent>;

/// Sending side used by providers. Send failures mean the consumer went away;
/// providers keep going only as far as needed to release resources.
#[derive(Clone)]
enum EventSinkTx<E> {
    Bounded(mpsc::Sender<E>),
    Source(mpsc::UnboundedSender<E>),
}
#[derive(Clone)]
pub struct EventSink<E = AssistantMessageEvent> {
    tx: EventSinkTx<E>,
}

impl<E> EventSink<E> {
    pub fn channel() -> (EventSink<E>, mpsc::Receiver<E>) {
        let (tx, rx) = mpsc::channel(256);
        (
            EventSink {
                tx: EventSinkTx::Bounded(tx),
            },
            rx,
        )
    }
    fn source_channel() -> (EventSink<E>, mpsc::UnboundedReceiver<E>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            EventSink {
                tx: EventSinkTx::Source(tx),
            },
            rx,
        )
    }

    pub async fn push(&self, event: E) -> bool {
        match &self.tx {
            EventSinkTx::Bounded(tx) => tx.send(event).await.is_ok(),
            EventSinkTx::Source(tx) => tx.send(event).is_ok(),
        }
    }

    /// Let wrappers release their input receiver when a consumer leaves even
    /// if the provider is currently waiting without emitting another event.
    pub async fn closed(&self) {
        match &self.tx {
            EventSinkTx::Bounded(tx) => tx.closed().await,
            EventSinkTx::Source(tx) => tx.closed().await,
        }
    }

    /// Cancellation must release the producer even if the consumer has filled
    /// its bounded queue. The caller still owns the cancellation receipt.
    pub fn try_push(&self, event: E) -> bool {
        match &self.tx {
            EventSinkTx::Bounded(tx) => tx.try_send(event).is_ok(),
            EventSinkTx::Source(tx) => tx.send(event).is_ok(),
        }
    }

    /// Like [`push`](Self::push) but gives up when `cancel` fires, so a
    /// consumer that stopped reading cannot park a cancelled provider.
    pub async fn push_or_cancel(
        &self,
        event: E,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> bool {
        tokio::select! {
            r = self.push(event) => r,
            _ = cancel.cancelled() => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn image_end_keeps_content_and_current_snapshot() {
        let image = ImageContent {
            detail: None,
            compaction_frame: false,
            data: "YQ==".into(),
            mime_type: "image/png".into(),
        };
        let mut partial = AssistantMessage::empty("openai-completions", "fake", "model");
        partial.content.push(AssistantBlock::Image(image.clone()));
        let event = AssistantMessageEvent::ImageEnd {
            content_index: 0,
            content: image,
            partial: partial.clone(),
        };
        assert_eq!(event.type_name(), "image_end");
        assert!(!event.is_terminal());
        assert_eq!(event.partial(), &partial);
        assert_eq!(
            event.printable(),
            json!({"type": "image_end", "contentIndex": 0, "content": {"type": "image", "data": "YQ==", "mimeType": "image/png"}})
        );
    }
}
