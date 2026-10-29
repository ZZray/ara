//! Private reference-Host display owner. One existing main task evaluates;
//! OS producers carry metadata and physical paint consumes owned rows only.
use super::{
    driver::TerminalDriver, event_controller::EventController, graph_renderer::DisplayGraph,
    tool_execution::ToolExecutionComponent, transcript::BlockKey,
};
use crate::{
    native_factory::PreparedNativeSession,
    native_realm::{
        context::NativeScope,
        parent::{NativeParentId, NativeParentLease},
        promise::{NativeDisplayCallbacks, NativeDrain, NativePromiseAdmission},
    },
};
use ara_agent::{AgentEvent, SessionValues};
use ara_rpc::value_heap::{ValueError, ValueRuntime};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};

const UPDATE_WINDOW: Duration = Duration::from_millis(33);
const QUANTUM: usize = 8;
pub(crate) enum Origin {
    Session,
    Scoped { admission: NativePromiseAdmission, scope: NativeScope, parent: NativeParentId, _lease: NativeParentLease,
    },
}
impl Origin {
    pub(crate) fn capture(
        values: &SessionValues,
        admission: NativePromiseAdmission,
        effective: &dyn ValueRuntime,
    ) -> Result<Self, ValueError> {
        admission.check_values(values)?;
        let scope = admission.capture_scope(effective)?;
        let parent = admission.capture_parent(effective)?;
        let lease = admission.retain_parent(parent)?;
        Ok(Self::Scoped { admission, scope, parent, _lease: lease,
        })
    }
}
#[derive(Clone, Debug)]
pub(crate) enum Control {
    Expand,
    ToolExpanded {
        generation: u64,
        id: ara_rpc::WireString,
        expanded: bool,
    },
    Replay,
    Invalidate,
    HistoryAck(u64),
}
enum Work {
    Snapshot(super::session_observation::SessionTranscriptObservation),
    Event { generation: u64, event: AgentEvent, persisted: Option<(String, Arc<ara_rpc::WireValue>)>,
    },
    Control(Control),
}
struct Envelope {
    seq: u64,
    origin: Origin,
    work: Work,
    _slot: OwnedSemaphorePermit,
}
struct PendingUpdate {
    envelope: Envelope,
    armed: Instant,
}
struct Demand {
    seq: u64,
    origin: Origin,
}
#[derive(Default)]
struct Queue {
    closed: bool,
    next: u64,
    events: VecDeque<Envelope>,
    update: Option<PendingUpdate>,
    frame: Option<Demand>,
    reveal_deadline: Option<Instant>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    Accepted(u64),
    Full,
    Closed,
}
pub struct DisplayPort {
    queue: Mutex<Queue>,
    slots: Arc<Semaphore>,
    wake: Arc<Notify>,
    fence: AtomicU64,
    active_generation: AtomicU64,
}
impl DisplayPort {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            queue: Mutex::new(Queue::default()),
            slots: Arc::new(Semaphore::new(32)),
            wake: Arc::new(Notify::new()),
            fence: AtomicU64::new(0),
            active_generation: AtomicU64::new(0),
        })
    }
    fn cumulative(event: &AgentEvent) -> bool {
        matches!(event, AgentEvent::MessageUpdate { .. } | AgentEvent::ActualMessageUpdate { .. })
    }
    pub async fn admit(&self, generation: u64, event: AgentEvent) -> Admission {
        self.admit_persisted(generation, event, None).await
    }
    pub async fn admit_persisted(
        &self,
        generation: u64,
        event: AgentEvent,
        persisted: Option<(String, Arc<ara_rpc::WireValue>)>,
    ) -> Admission {
        if generation == 0 || self.active_generation.load(Ordering::Acquire) != generation {
            return Admission::Closed;
        }
        let update = Self::cumulative(&event);
        if update {
            let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            if q.closed {
                return Admission::Closed;
            }
            if q.update.is_some() {
                q.next = q.next.wrapping_add(1);
                let seq = q.next;
                let pending = q.update.as_mut().unwrap();
                pending.envelope.seq = seq;
                pending.envelope.work = Work::Event { generation, event, persisted,
                };
                self.fence.fetch_add(1, Ordering::AcqRel);
                drop(q);
                self.wake.notify_one();
                return Admission::Accepted(seq);
            }
        }
        let Ok(slot) = self.slots.clone().acquire_owned().await else { return Admission::Closed;
        };
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed || self.active_generation.load(Ordering::Acquire) != generation {
            return Admission::Closed;
        }
        q.next = q.next.wrapping_add(1);
        let seq = q.next;
        let envelope =
            Envelope { seq, origin: Origin::Session, work: Work::Event { generation, event, persisted,
            }, _slot: slot,
        };
        if update {
            if let Some(pending) = &mut q.update {
                pending.envelope.seq = seq;
                pending.envelope.work = envelope.work;
            } else {
                q.update = Some(PendingUpdate { envelope, armed: Instant::now(),
                });
            }
        } else {
            if let Some(pending) = q.update.take() {
                q.events.push_back(pending.envelope)
            }
            q.events.push_back(envelope);
        }
        self.fence.fetch_add(1, Ordering::AcqRel);
        drop(q);
        self.wake.notify_one();
        Admission::Accepted(seq)
    }
    pub fn admit_snapshot(&self, snapshot: super::session_observation::SessionTranscriptObservation,
    ) -> Admission {
        let slot = match self.reserve() {
            Ok(slot) => slot,
            Err(error) => return error,
        };
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed {
            return Admission::Closed;
        }
        if let Some(update) = q.update.take() {
            q.events.push_back(update.envelope);
        }
        q.next = q.next.wrapping_add(1);
        let seq = q.next;
        q.events.push_back(Envelope { seq, origin: Origin::Session, work: Work::Snapshot(snapshot), _slot: slot,
        });
        self.fence.fetch_add(1, Ordering::AcqRel);
        drop(q);
        self.wake.notify_one();
        Admission::Accepted(seq)
    }
    pub fn request_frame(&self) {
        self.frame();
    }
    pub(crate) fn reserve(&self) -> Result<OwnedSemaphorePermit, Admission> {
        if self.queue.lock().unwrap_or_else(|e| e.into_inner()).closed {
            return Err(Admission::Closed);
        }
        self.slots.clone().try_acquire_owned().map_err(|_| Admission::Full)
    }
    pub(crate) fn control_reserved(&self, slot: OwnedSemaphorePermit, control: Control,
    ) -> Admission {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed {
            return Admission::Closed;
        }
        if let Some(pending) = q.update.take() {
            q.events.push_back(pending.envelope)
        }
        q.next = q.next.wrapping_add(1);
        let seq = q.next;
        q.events.push_back(Envelope { seq, origin: Origin::Session, work: Work::Control(control), _slot: slot,
        });
        self.fence.fetch_add(1, Ordering::AcqRel);
        drop(q);
        self.wake.notify_one();
        Admission::Accepted(seq)
    }
    pub(crate) fn frame(&self) {
        self.frame_origin(Origin::Session);
    }
    fn schedule_reveal(&self, deadline: Option<Instant>) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed {
            return;
        }
        // Failed/obsolete paint leaves the committed component unchanged;
        // retry cadence remains bounded without a second row writer.
        q.reveal_deadline = deadline.map(|deadline| {
            if deadline <= Instant::now() {
                Instant::now() + Duration::from_nanos(1_000_000_000 / 30)
            } else {
                deadline
            }
        });
        // Called by the same evaluator immediately before its next ready()
        // wait. Notification here would self-wake an unchanged future deadline.
    }
    pub(crate) fn frame_origin(&self, origin: Origin) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.closed {
            return;
        }
        if q.frame.is_none() {
            q.next = q.next.wrapping_add(1);
            let seq = q.next;
            q.frame = Some(Demand { seq, origin });
        }
        drop(q);
        self.wake.notify_one();
    }
    pub(crate) fn run_generation(&self, generation: u64) {
        self.active_generation.store(generation, Ordering::Release);
        self.wake.notify_waiters();
    }
    pub fn cutoff(&self) -> u64 {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).next
    }
    fn take(&self, cutoff: u64, force_update: bool) -> Option<Envelope> {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.update
            .as_ref()
            .is_some_and(|p| {
            p.envelope.seq <= cutoff && (force_update || p.armed.elapsed() >= UPDATE_WINDOW)
        })
        {
            let p = q.update.take().unwrap();
            q.events.push_back(p.envelope);
        }
        if q.events.front().is_some_and(|e| e.seq <= cutoff) { q.events.pop_front() } else { None }
    }
    fn take_frame(&self, cutoff: u64) -> Option<Demand> {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.frame.is_none() && q.reveal_deadline.is_some_and(|deadline| deadline <= Instant::now()) {
            // The existing Session evaluator, not an OS timer callback,
            // owns all heap work and the transactional frame publication.
            q.frame = Some(Demand { seq: cutoff, origin: Origin::Session,
            });
            q.reveal_deadline = None;
        }
        let frontier = q.frame.as_ref().map(|f| f.seq).unwrap_or(cutoff);
        if q.events.front().is_some_and(|e| e.seq <= frontier) {
            return None;
        }
        if q.frame.as_ref().is_some_and(|f| f.seq <= cutoff) { q.frame.take() } else { None }
    }
    pub async fn ready(&self) {
        loop {
            let wake = self.wake.notified();
            tokio::pin!(wake);
            wake.as_mut().enable();
            let deadline = {
                let q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
                if q.closed || !q.events.is_empty() || q.frame.is_some() {
                    return;
                }
                match (q.update.as_ref().map(|p| p.armed + UPDATE_WINDOW), q.reveal_deadline,
                ) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
            };
            if let Some(deadline) = deadline {
                tokio::select! {_=&mut wake=>{},_=tokio::time::sleep_until(deadline.into())=>return}
            } else {
                wake.await;
                return;
            }
        }
    }
    pub fn close(&self) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.closed = true;
        q.events.clear();
        q.update = None;
        q.frame = None;
        q.reveal_deadline = None;
        self.slots.close();
        drop(q);
        self.wake.notify_waiters();
    }
}
#[derive(Debug)]
pub struct DisplayReceipt {
    pub sequence: u64,
    pub phase: &'static str,
    pub component: Option<BlockKey>,
    pub error: ValueError,
}
pub struct DisplayEvaluator {
    pub port: Arc<DisplayPort>,
    driver: Arc<TerminalDriver>,
    controller: EventController,
    prepared: Option<Arc<PreparedNativeSession>>,
    callbacks: Option<NativeDisplayCallbacks>,
    pub receipts: VecDeque<DisplayReceipt>,
    pub history_writes: VecDeque<super::driver::HistoryWriteReceipt>,
}
impl DisplayEvaluator {
    pub fn new(
        driver: Arc<TerminalDriver>,
        prepared: Option<Arc<PreparedNativeSession>>,
        mode: ara_edit::EditMode,
    ) -> Result<Self, ValueError> {
        let callbacks = prepared.as_ref().map(|p| p.realm().begin_display_callbacks()).transpose()?;
        Ok(Self {
            port: driver.display_port(),
            controller: EventController::new(driver.theme(), mode),
            driver,
            prepared,
            callbacks,
            receipts: VecDeque::new(),
            history_writes: VecDeque::new(),
        })
    }
    pub fn set_presentation(&mut self, settings: super::render_utils::TerminalPresentationSettings,
    ) {
        self.controller.set_presentation(settings);
        self.port.frame();
    }
    pub fn attach_native(&mut self, prepared: Option<Arc<PreparedNativeSession>>,
    ) -> Result<(), ValueError> {
        if let Some(prepared) = prepared {
            if let Some(current) = &self.prepared {
                if !Arc::ptr_eq(current.values(), prepared.values()) {
                    return Err(ValueError::UnsupportedOperation("new Native Session needs a new display owner",
                    ));
                }
            } else {
                self.callbacks = Some(prepared.realm().begin_display_callbacks()?);
                self.prepared = Some(prepared);
            }
        }
        Ok(())
    }
    pub fn matches_session(&self, prepared: Option<&Arc<PreparedNativeSession>>) -> bool {
        match (&self.prepared, prepared) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
    pub fn begin_run(&mut self, mode: ara_edit::EditMode) -> u64 {
        self.controller.begin_run(mode)
    }
    fn receipt(&mut self, sequence: u64, phase: &'static str, error: ValueError) {
        self.receipt_component(sequence, phase, None, error);
    }
    fn receipt_component(
        &mut self,
        sequence: u64,
        phase: &'static str,
        component: Option<BlockKey>,
        error: ValueError,
    ) {
        if self.receipts.len() == 32 {
            self.receipts.pop_front();
        }
        self.receipts.push_back(DisplayReceipt { sequence, phase, component, error,
        });
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.callbacks.as_ref().map(NativeDisplayCallbacks::wake).unwrap_or_else(|| self.port.wake.clone())
    }
    fn execute(&mut self, e: Envelope) {
        let presentation = self.driver.display_snapshot();
        self.controller.sync_presentation(presentation.spinner, presentation.presentation_now);
        self.controller.set_native(presentation.native);
        match e.work {
            Work::Control(control @ (Control::Expand | Control::Invalidate)) => {
                let runtime = match &e.origin {
                    Origin::Session => self.callbacks.as_ref().map(NativeDisplayCallbacks::runtime).transpose(),
                    Origin::Scoped { admission, scope, parent, .. } => admission.scoped(*scope, *parent).map(Some),
                };
                match runtime {
                    Ok(runtime) => {
                        let graph = self.prepared.as_ref().map(|p| {
                            DisplayGraph(
                                p.values(),
                                runtime.as_ref().map(|r| r as &dyn ValueRuntime).unwrap_or(p.values().runtime()),
                            )
                        });
                        if matches!(control, Control::Expand) {
                            self.controller.toggle_expanded_in(graph.as_ref());
                        } else {
                            self.controller.invalidate_in(graph.as_ref());
                        }
                    }
                    Err(error) => self.receipt(e.seq, "origin", error),
                }
                for (key, error) in self.controller.take_display_errors() {
                    self.receipt_component(e.seq, "construction", Some(key), error);
                    self.driver.diagnostic("ara: tool display constructor failed");
                }
            }
            Work::Control(Control::ToolExpanded {
                generation,
                id,
                expanded,
            }) => self.controller.set_tool_expanded(generation, &id, expanded),
            Work::Control(Control::Replay) => self.controller.transcript.begin_history_replay(),
            Work::Control(Control::HistoryAck(id)) => {
                self.controller.acknowledge_session_history(id);
            }
            Work::Snapshot(snapshot) => {
                if !snapshot.same_owner(self.prepared.as_ref()) {
                    self.receipt(
                        e.seq,
                        "snapshot-owner",
                        ValueError::UnsupportedOperation("Session snapshot Native owner mismatch"),
                    );
                    return;
                }
                let runtime = self.callbacks.as_ref().map(NativeDisplayCallbacks::runtime).transpose();
                match runtime {
                    Ok(runtime) => {
                        let graph = self.prepared.as_ref().map(|p| {
                            DisplayGraph(
                                p.values(),
                                runtime.as_ref().map(|r| r as &dyn ValueRuntime).unwrap_or(p.values().runtime()),
                            )
                        });
                        if let Err(error) = self.controller.observe_session(snapshot, graph.as_ref()) {
                            self.receipt(e.seq, "snapshot", error);
                        }
                    }
                    Err(error) => self.receipt(e.seq, "snapshot-origin", error),
                }
            }
            Work::Event { generation, event, persisted,
            } => {
                if generation != self.controller.generation || !self.controller.live {
                    return;
                }
                if matches!(&event, AgentEvent::MessageStart { .. }) {
                    self.controller.start_entry_observation();
                }
                let result = (|| {
                    let runtime = match &e.origin {
                        Origin::Session => self.callbacks.as_ref().map(NativeDisplayCallbacks::runtime).transpose()?,
                        Origin::Scoped { admission, scope, parent, .. } => Some(admission.scoped(*scope, *parent)?),
                    };
                    self.controller.consume_in(
                        &event,
                        self.prepared.as_ref().map(|p| p.values().as_ref()),
                        runtime.as_ref().map(|r| r as &dyn ValueRuntime),
                    )
                })();
                if result.is_ok() {
                    if let Some((id, body)) = persisted {
                        if let Err(error) = self.controller.observe_entry_receipt(id, body) {
                            self.receipt(e.seq, "entry-receipt", error);
                        }
                    }
                }
                if let Err(error) = result {
                    self.receipt(e.seq, "admission", error);
                }
                for (key, error) in self.controller.take_display_errors() {
                    self.receipt_component(e.seq, "construction", Some(key), error);
                    self.driver.diagnostic("ara: tool display constructor failed");
                }
            }
        }
        self.port.frame();
    }
    pub fn pump_quantum(&mut self) -> bool {
        if let Some(ack) = self.driver.take_history_receipt() {
            self.controller.acknowledge_session_history(ack.id);
            if self.history_writes.len() == 32 {
                self.history_writes.pop_front();
            }
            self.history_writes.push_back(ack);
        }
        let cutoff = self.port.cutoff();
        let mut count = 0;
        while count < QUANTUM {
            let Some(event) = self.port.take(cutoff, false) else { break;
            };
            self.execute(event);
            count += 1;
        }
        if let Some(demand) = self.port.take_frame(self.port.cutoff()) {
            self.render(demand);
        }
        let mut callback_count = 0;
        let mut callback_error = None;
        if let Some(callbacks) = &mut self.callbacks {
            for _ in 0..QUANTUM {
                match callbacks.drain_one() {
                    Ok(NativeDrain::Idle) => break,
                    Ok(_) => callback_count += 1,
                    Err(error) => {
                        callback_error = Some(error);
                        break;
                    }
                }
            }
        }
        if let Some(error) = callback_error {
            self.receipt(cutoff, "callback", error);
        }
        self.port.schedule_reveal(self.controller.reveal_deadline());
        count == QUANTUM || callback_count == QUANTUM
    }
    pub fn finish_through(&mut self, cutoff: u64) {
        // The captured admission watermark is finite; later Session demand is
        // serviced by the existing prompt pump after NativeRun revokes.
        while let Some(event) = self.port.take(cutoff, true) {
            self.execute(event);
        }
        self.controller.seal_run();
        self.controller.transcript.begin_history_flush();
        self.render(Demand { seq: cutoff, origin: Origin::Session,
        });
        self.port.schedule_reveal(None);
    }
    fn render(&mut self, demand: Demand) {
        let snapshot = self.driver.display_snapshot();
        let fence = self.port.fence.load(Ordering::Acquire);
        let mut touched: HashMap<BlockKey, ToolExecutionComponent> = HashMap::new();
        let mut touched_session = HashMap::new();
        let mut failures = Vec::new();
        let prepared = self.prepared.clone();
        let runtime = match &demand.origin {
            Origin::Session => self.callbacks.as_ref().map(NativeDisplayCallbacks::runtime).transpose(),
            Origin::Scoped { admission, scope, parent, .. } => admission.scoped(*scope, *parent).map(Some),
        };
        let runtime = match runtime {
            Ok(r) => r,
            Err(e) => {
                self.receipt(demand.seq, "origin", e);
                return;
            }
        };
        let callback_frame = match (&prepared, &self.callbacks) {
            (Some(prepared), Some(callbacks)) => Some(super::graph_renderer::FrameCallbacks::new(
                callbacks.domain(),
                runtime.as_ref().map(|r| r as &dyn ValueRuntime).unwrap_or(prepared.values().runtime()),
            )),
            _ => None,
        };
        let graph = prepared.as_ref().map(|p| {
            DisplayGraph(
                p.values(),
                callback_frame
                    .as_ref()
                    .map(|r| r as &dyn ValueRuntime)
                    .or_else(|| runtime.as_ref().map(|r| r as &dyn ValueRuntime))
                    .unwrap_or(p.values().runtime()),
            )
        });
        if snapshot.native {
            let mut surface = super::tsp_backend::Surface::default();
            for (index, (key, text, rows)) in self
                .controller
                .transcript
                .native_entries()
                .into_iter()
                .enumerate()
            {
                let stable = format!("block:{key:?}");
                if let Some(original) = self.controller.tool_at(&key) {
                    let tool = touched
                        .entry(key.clone())
                        .or_insert_with(|| original.clone());
                    let generation = match &key {
                        BlockKey::Tool(g, _) => *g,
                        _ => 0,
                    };
                    match tool.describe_native(graph.as_ref(), generation) {
                        Ok(Some(mut node)) => {
                            node.key = stable;
                            surface.main.push(node);
                            continue;
                        }
                        Err(error) => {
                            failures.push((key.clone(), error, !tool.warned));
                            tool.warned = true;
                        }
                        _ => {}
                    }
                }
                let rows = if let Some(text) = text {
                    text.split('\n')
                        .flat_map(|line| super::render_utils::wrap(line, snapshot.frame.columns))
                        .collect()
                } else if let Some(row) = self.controller.session_frame_copy(&key) {
                    let row = touched_session.entry(key.clone()).or_insert(row);
                    row.render_for_frame(snapshot.frame.columns, &snapshot.frame)
                } else if let Some(rows) = self.controller.marker_rows(&key, snapshot.frame.columns)
                {
                    rows
                } else if let Some(tool) = touched.get_mut(&key) {
                    match &graph {
                        Some(graph) => {
                            for error in tool.rebuild_rows_fallback_in(graph) {failures.push((key.clone(),error,false));}
                            tool.render_in(
                                graph,
                                snapshot.frame.columns,
                                &snapshot.frame,
                                snapshot.spinner,
                                usize::MAX,
                                snapshot.presentation_now,
                            )
                            .unwrap_or_else(|_| tool.fallback_rows(snapshot.frame.columns))
                        },
                        None => {
                            tool.render(snapshot.frame.columns, &snapshot.frame, snapshot.spinner)
                        }
                    }
                } else {
                    rows
                };
                let _ = index;
                surface.main.push(super::tsp_backend::Node::rows(
                    stable,
                    rows,
                    snapshot.frame.columns,
                ));
            }
            let publication_port = self.port.clone();
            let publication = publication_port
                .queue
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if fence != self.port.fence.load(Ordering::Acquire) {
                return;
            }
            let driver = self.driver.clone();
            let result = driver.publish_native_transaction(snapshot, surface, || {
                if let Some(callback_frame) = &callback_frame {
                    callback_frame.commit();
                }
                for (key, tool) in touched {
                    self.controller.commit_tool(&key, tool);
                }
                for (key, row) in touched_session {
                    self.controller.commit_session_row(&key, row);
                }
            });
            drop(publication);
            match result {
                Ok(true) => {
                    for (key, error, warn) in failures {
                        self.receipt_component(demand.seq, "native-frame", Some(key), error);
                        if warn {
                            self.driver.diagnostic("ara: tool description failed");
                        }
                    }
                }
                Ok(false) => {}
                Err(error) => self
                    .driver
                    .diagnostic(&format!("ara: terminal write failed ({error})")),
            };
            return;
        }
        let controller = &self.controller;
        let transaction = self.controller.transcript.prepare_frame(
            snapshot.frame.columns,
            snapshot.frame.rows.saturating_sub(1),
            |key, width, allocation| {
                if let Some(original) = controller.session_frame_copy(key) {
                    let row = touched_session.entry(key.clone()).or_insert(original);
                    return Some(row.render_for_frame(width, &snapshot.frame));
                }
                if let Some(rows) = controller.marker_rows(key, width) {
                    return Some(rows);
                }
                let Some(original) = controller.tool_at(key) else { return None;
                };
                let tool = touched.entry(key.clone()).or_insert_with(|| original.clone());
                let result = match &graph {
                    Some(graph) => tool.render_in(
                        graph,
                        width,
                        &snapshot.frame,
                        snapshot.spinner,
                        allocation,
                        snapshot.presentation_now,
                    ),
                    None => Ok(tool.render(width, &snapshot.frame, snapshot.spinner)),
                };
                Some(match result {
                    Ok(rows) => rows,
                    Err(error) => {
                        let warn = !tool.warned;
                        tool.warned = true;
                        failures.push((key.clone(), error, warn));
                        if let Some(graph) = &graph {
                            if let Err(error) = tool.capture_fallback(graph) {
                                failures.push((key.clone(), error, false));
                            }
                        }
                        let rows = tool.fallback_rows(width);
                        if let Some(graph) = &graph {
                            match tool.compact_in(graph, width, rows, allocation, snapshot.presentation_now,
                            ) {
                                Ok(rows) => rows,
                                Err(error) => {
                                    failures.push((key.clone(), error, false));
                                    tool.fallback_rows(width)
                                }
                            }
                        } else {
                            rows
                        }
                    }
                })
            },
        );
        let publication_port = self.port.clone();
        let publication = publication_port.queue.lock().unwrap_or_else(|e| e.into_inner());
        if fence != self.port.fence.load(Ordering::Acquire) {
            return;
        }
        let plan = transaction.plan.clone();
        let driver = self.driver.clone();
        let spinner_active = self.controller.tools.values().any(|tool| tool.spinner_active());
        let result = driver.publish_transaction(snapshot, plan, spinner_active, || {
            if let Some(callback_frame) = &callback_frame {
                callback_frame.commit();
            }
            self.controller.transcript.commit_frame(transaction);
            for (key, tool) in touched {
                self.controller.commit_tool(&key, tool);
            }
            for (key, row) in touched_session {
                self.controller.commit_session_row(&key, row);
            }
        });
        drop(publication);
        match result {
            Ok(true) => {
                for (key, error, warn) in failures {
                    self.receipt_component(demand.seq, "frame", Some(key), error);
                    if warn {
                        self.driver.diagnostic("ara: tool display renderer failed");
                    }
                }
            }
            Ok(false) => return,
            Err(error) => self.driver.diagnostic(&format!("ara: terminal write failed ({error})")),
        }

        if let Some(ack) = self.driver.take_history_receipt() {
            self.controller.acknowledge_session_history(ack.id);
            if self.history_writes.len() == 32 {
                self.history_writes.pop_front();
            }
            self.history_writes.push_back(ack);
        }
    }
}
// TerminalDriver closes the shared port when the logical Session owner drops.

#[cfg(test)]
include!("evaluation_observations.rs");
