//! Fixed OMP596f2da Bash parameter/await and managed-job branches.
//! MIT; Copyright (c) 2025 Mario Zechner; 2025-2026 Can Bölük;
//! 2026 Stencil Labs, Inc. See THIRD_PARTY.md.
//! The Session owns jobs; the revocable Run owns their foreground await only.

use super::*;
use ara_tools::bash::{
    BashPreparedOsValues, BashProcessDispatchReceipt, execute_bash_prepared_os_values_with_receipt,
    prepare_bash_os_values,
};
use async_effect::{HostEffectId, HostEffectReceipt, HostEffectSettlementOwner};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use tokio::sync::Notify;

/// Host settings snapshot taken when this logical Session's Bash target is
/// created. Defaults are the pinned settings schema, not a compatibility gate.
#[derive(Clone, Debug)]
pub struct BashSessionSettings {
    pub async_enabled: bool,
    pub auto_background_enabled: bool,
    pub auto_background_threshold_ms: f64,
    pub max_running_jobs: usize,
    pub max_timeout_seconds: f64,
}
impl Default for BashSessionSettings {
    fn default() -> Self {
        Self {
            async_enabled: true,
            auto_background_enabled: true,
            auto_background_threshold_ms: 60_000.0,
            max_running_jobs: 100,
            max_timeout_seconds: 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BashJobSnapshot {
    pub id: String,
    pub label: String,
    pub status: &'static str,
    pub latest_text: String,
    pub result: Option<Result<ToolOutput, NativeToolFault>>,
    pub suppressed: bool,
    pub consumed: bool,
}
struct JobRow {
    snapshot: BashJobSnapshot,
    effect: HostEffectId,
    cancel: CancellationToken,
    timeout_unknown: Arc<AtomicBool>,
    process_settled: Arc<AtomicBool>,
    forward: Option<Arc<Mutex<Option<BashUpdateFn>>>>,
}
struct JobState {
    closed: bool,
    next_id: u64,
    rows: HashMap<String, JobRow>,
    deliveries: VecDeque<String>,
    suppressed: HashSet<String>,
    consumed: HashSet<String>,
    wait_watch:HashMap<String,u64>,
    wait_reserved:HashMap<String,u64>,
    wait_unknown:HashMap<String,String>,
}
enum JobStart {
    Started(String),
    Foreground { prepared: BashPreparedOsValues, permit: EffectPermit },
}

/// Exactly one host owner per logical Session. It reuses the existing native
/// effect owner for spawn, panic monitoring, cancellation and joined receipts.
/// No original Graph value or Run lease enters a background worker.
pub struct SessionBashOwner {
    values: Mutex<Option<Weak<SessionValues>>>,
    settings: BashSessionSettings,
    effects: Arc<HostEffectSettlementOwner>,
    state: Mutex<JobState>,
    wake: Arc<Notify>,
    wait_owner:OnceLock<Arc<super::wait_native::SessionWaitOwner>>,
}
impl SessionBashOwner {
    pub fn new(settings: BashSessionSettings) -> Arc<Self> {
        Arc::new(Self {
            values: Mutex::new(None),
            settings,
            effects: Arc::new(HostEffectSettlementOwner::new()),
            state: Mutex::new(JobState {
                closed: false,
                next_id: 1,
                rows: HashMap::new(),
                deliveries: VecDeque::new(),
                suppressed: HashSet::new(),
                consumed: HashSet::new(),
                wait_watch:HashMap::new(),
                wait_reserved:HashMap::new(),
                wait_unknown:HashMap::new(),
            }),
            wake: Arc::new(Notify::new()),
            wait_owner:OnceLock::new(),
        })
    }
    pub(super) fn bind_values(&self, values: &Arc<SessionValues>) -> Result<(), ActualProviderError> {
        let mut binding = self.values.lock().unwrap_or_else(|e| e.into_inner());
        match binding.as_ref() {
            None => {
                *binding = Some(Arc::downgrade(values));
                Ok(())
            }
            Some(original) if original.upgrade().is_some_and(|original| Arc::ptr_eq(&original, values)) => Ok(()),
            Some(_) => Err(ActualProviderError::Protocol("Bash job owner belongs to another logical Session")),
        }
    }
    /// Lifetime anchor only: Wait stores no producers or duplicate Bash truth.
    pub fn wait_session(self:&Arc<Self>)->Arc<super::wait_native::SessionWaitOwner>{
        self.wait_owner.get_or_init(||super::wait_native::SessionWaitOwner::new(Arc::downgrade(self))).clone()
    }
    pub fn watch_foreground(self:&Arc<Self>,token:u64)->BashWaitWatch {
        self.reconcile();let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        let mut ids=state.rows.keys().filter(|id|!state.consumed.contains(*id)&&!state.suppressed.contains(*id)&&!state.wait_watch.contains_key(*id)&&!state.wait_unknown.contains_key(*id)).cloned().collect::<Vec<_>>();ids.sort();
        for id in &ids {state.wait_watch.insert(id.clone(),token);}
        BashWaitWatch{owner:self.clone(),token,ids}
    }
    pub fn commit_foreground(&self,token:u64,ids:&[String])->Result<(),ToolError>{
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        for id in ids {if state.wait_reserved.get(id)!=Some(&token){return Err(ToolError("Bash foreground receipt lost exact reservation".into()));}}
        for id in ids {state.wait_reserved.remove(id);state.wait_watch.remove(id);state.wait_unknown.remove(id);state.deliveries.retain(|queued|queued!=id);state.consumed.insert(id.clone());}
        Ok(())
    }
    pub fn mark_foreground_unknown(&self,token:u64,ids:&[String],error:&str){
        let mut state=self.state.lock().unwrap_or_else(|e|e.into_inner());
        for id in ids {if state.wait_reserved.get(id)==Some(&token){state.wait_unknown.insert(id.clone(),error.into());}}
    }
    pub fn settings(&self) -> &BashSessionSettings {
        &self.settings
    }
    pub fn effects(&self) -> Arc<HostEffectSettlementOwner> {
        self.effects.clone()
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }
    fn reconcile(&self) {
        let pending = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.rows.iter().map(|(id, row)| (id.clone(), row.effect)).collect::<Vec<_>>()
        };
        for (id, effect) in pending {
            if let Some(outcome) = self.effects.take_completion_for(effect) {
                let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
                let cancelled = state.rows.get(&id).is_some_and(|row| row.snapshot.status == "cancelled");
                let missing = state.rows.get(&id).is_some_and(|row| row.snapshot.result.is_none());
                if !missing {
                    continue;
                }
                if let Some(row) = state.rows.get_mut(&id) {
                    if !cancelled {
                        row.snapshot.status =
                            if outcome.as_ref().is_ok_and(|output| !output.is_error) { "completed" } else { "failed" };
                    }
                    row.snapshot.latest_text =
                        outcome.as_ref().map(output_text).unwrap_or_else(|fault| fault.message().to_owned());
                    row.snapshot.result = Some(outcome);
                }
                if !cancelled && !state.closed && !state.suppressed.contains(&id) {
                    state.deliveries.push_back(id);
                }
            }
        }
    }
    pub fn snapshots(&self) -> Vec<BashJobSnapshot> {
        self.reconcile();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state
            .rows
            .values()
            .map(|row| {
                let mut snapshot = row.snapshot.clone();
                snapshot.suppressed = state.suppressed.contains(&snapshot.id);
                snapshot.consumed = state.consumed.contains(&snapshot.id);
                snapshot
            })
            .collect()
    }
    pub fn at_capacity(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.closed
            || state.rows.values().filter(|row| row.snapshot.status == "running").count()
                >= self.settings.max_running_jobs.max(1)
    }
    pub fn cancel(&self, id: &str) -> bool {
        let token = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(row) = state.rows.get_mut(id) else {
                return false;
            };
            if row.snapshot.status != "running" {
                return false;
            }
            row.snapshot.status = "cancelled";
            row.cancel.clone()
        };
        token.cancel();
        self.wake.notify_waiters();
        true
    }
    pub fn acknowledge_deliveries(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.suppressed.insert(id.to_owned());
        state.deliveries.retain(|queued| queued != id);
    }
    pub fn resume_deliveries(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.suppressed.remove(id) {
            return;
        }
        let ready = state.rows.get(id).is_some_and(|row| matches!(row.snapshot.status, "completed" | "failed"));
        if ready && !state.consumed.contains(id) && !state.deliveries.iter().any(|queued| queued == id) {
            state.deliveries.push_back(id.to_owned());
        }
        drop(state);
        self.wake.notify_waiters();
    }
    /// Reading a candidate is not its delivery receipt. The host commits the
    /// original async-result message, then acknowledges this exact Session row.
    pub fn pending_deliveries(&self) -> Vec<BashJobSnapshot> {
        self.reconcile();
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.deliveries.iter().filter(|id|!state.wait_watch.contains_key(*id)&&!state.wait_reserved.contains_key(*id)&&!state.wait_unknown.contains_key(*id)).filter_map(|id| state.rows.get(id).map(|row| row.snapshot.clone())).collect()
    }
    pub fn commit_delivery(&self, id: &str) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.wait_watch.contains_key(id)||state.wait_reserved.contains_key(id)||state.wait_unknown.contains_key(id)||!state.deliveries.iter().any(|queued| queued == id) {
            return false;
        }
        state.deliveries.retain(|queued| queued != id);
        state.consumed.insert(id.to_owned());
        true
    }
    pub fn close(&self) {
        if let Some(wait)=self.wait_owner.get(){wait.close();}
        let tokens = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            state.deliveries.clear();
            state
                .rows
                .values_mut()
                .filter_map(|row| {
                    if row.snapshot.status == "running" {
                        row.snapshot.status = "cancelled";
                        Some(row.cancel.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        };
        self.effects.seal();
        for token in tokens {
            token.cancel()
        }
        self.wake.notify_waiters();
    }
    /// Reset/new/dispose must retain and await this receipt owner even when the
    /// Graph Run is already revoked. Closure never permits process replay.
    pub async fn close_and_settle(&self) -> Vec<HostEffectReceipt> {
        self.close();
        self.effects.settle().await
    }
    fn start(
        self: &Arc<Self>,
        prepared: BashPreparedOsValues,
        timeout: Option<f64>,
        notices: Vec<String>,
        permit: EffectPermit,
        process_cancel: CancellationToken,
        forward: Option<BashUpdateFn>,
        suppress: bool,
        allow_foreground: bool,
    ) -> Result<JobStart, NativeToolFault> {
        let reservation = self.effects.reserve()?;
        let effect = reservation.id();
        let timeout_unknown = Arc::new(AtomicBool::new(false));
        let process_settled = Arc::new(AtomicBool::new(false));
        let (id, forwarding) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(NativeToolFault::NotDispatched("Async job manager is disposed".into()));
            }
            if state.rows.values().filter(|row| row.snapshot.status == "running").count()
                >= self.settings.max_running_jobs.max(1)
            {
                // The preliminary Graph-side capacity read is only an early
                // branch choice. This lock makes the final slot decision and
                // registration atomic, retaining the admitted process on a race.
                if allow_foreground {
                    return Ok(JobStart::Foreground { prepared, permit });
                }
                return Err(NativeToolFault::NotDispatched(format!(
                    "Background job limit reached ({}). Wait for running jobs to finish or cancel one.",
                    self.settings.max_running_jobs.max(1)
                )));
            }
            let id = format!("bg_{}", state.next_id);
            state.next_id = state.next_id.checked_add(1).ok_or(NativeToolFault::SequenceExhausted)?;
            let label = if prepared.command.encode_utf16().count() > 120 {
                format!("{}...", prepared.command.chars().take(117).collect::<String>())
            } else {
                prepared.command.clone()
            };
            let forwarding = Arc::new(Mutex::new(forward));
            state.rows.insert(
                id.clone(),
                JobRow {
                    snapshot: BashJobSnapshot {
                        id: id.clone(),
                        label,
                        status: "running",
                        latest_text: String::new(),
                        result: None,
                        suppressed: suppress,
                        consumed: false,
                    },
                    effect,
                    cancel: process_cancel.clone(),
                    timeout_unknown: timeout_unknown.clone(),
                    process_settled: process_settled.clone(),
                    forward: Some(forwarding.clone()),
                },
            );
            if suppress {
                state.suppressed.insert(id.clone());
            }
            (id, forwarding)
        };
        let weak = Arc::downgrade(self);
        let update_id = id.clone();
        let progress_forwarding = forwarding.clone();
        let progress: BashUpdateFn = Arc::new(move |partial| {
            if let Some(owner) = weak.upgrade() {
                let mut state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(row) = state.rows.get_mut(&update_id) {
                    row.snapshot.latest_text = partial.output.clone();
                }
                drop(state);
                owner.wake.notify_waiters();
            }
            // Do not run any original Graph callback on the worker. The native
            // publisher queues it under the existing original parent/scope.
            let forwarding = progress_forwarding.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(callback) = forwarding.as_ref() {
                callback(partial);
            }
        });
        let weak = Arc::downgrade(self);
        let worker_id = id.clone();
        let process_token = process_cancel.clone();
        let worker_unknown = timeout_unknown.clone();
        let worker_settled = process_settled.clone();
        let future: NativeEffectFuture = Box::pin(async move {
            let _permit = permit;
            let dispatch = BashProcessDispatchReceipt::default();
            let result =
                execute_bash_prepared_os_values_with_receipt(prepared, timeout, process_token, progress, &dispatch)
                    .await;
            if let Ok(result) = &result {
                worker_settled.store(result.process_settlement_confirmed(), Ordering::Release);
                worker_unknown.store(
                    result.timed_out || ((cfg!(windows) || result.cancelled) && !result.process_settlement_confirmed()),
                    Ordering::Release,
                );
            }
            let outcome = result
                .map_err(|error| {
                    if dispatch.was_launched() {
                        NativeToolFault::OutcomeUnknown(error.0)
                    } else {
                        NativeToolFault::NotDispatched(error.0)
                    }
                })
                .and_then(|result| native_output(result, notices));
            *forwarding.lock().unwrap_or_else(|e| e.into_inner()) = None;
            if let Some(owner) = weak.upgrade() {
                let mut state = owner.state.lock().unwrap_or_else(|e| e.into_inner());
                let cancelled = state.rows.get(&worker_id).is_some_and(|row| row.snapshot.status == "cancelled");
                if let Some(row) = state.rows.get_mut(&worker_id) {
                    if !cancelled {
                        row.snapshot.status =
                            if outcome.as_ref().is_ok_and(|output| !output.is_error) { "completed" } else { "failed" };
                    }
                    row.snapshot.latest_text =
                        outcome.as_ref().map(output_text).unwrap_or_else(|fault| fault.message().to_owned());
                    row.snapshot.result = Some(outcome.clone());
                }
                if !cancelled && !state.closed && !state.suppressed.contains(&worker_id) {
                    state.deliveries.push_back(worker_id.clone());
                }
                drop(state);
                owner.wake.notify_waiters();
            }
            outcome
        });
        if let Err(error) = reservation.enter_bash(process_cancel, future, timeout_unknown, process_settled) {
            self.state.lock().unwrap_or_else(|e| e.into_inner()).rows.remove(&id);
            return Err(error);
        }
        Ok(JobStart::Started(id))
    }
    fn stop_updates(&self, id: &str) {
        let forwarding =
            self.state.lock().unwrap_or_else(|e| e.into_inner()).rows.get(id).and_then(|row| row.forward.clone());
        if let Some(forwarding) = forwarding {
            *forwarding.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }
    async fn wait(
        &self,
        id: &str,
        wait_ms: f64,
        cancel: &CancellationToken,
        steering: Option<&CancellationToken>,
    ) -> WaitResult {
        // The original helper tests abort before steering, before Promise.race.
        if cancel.is_cancelled() {
            return WaitResult::Aborted;
        }
        if steering.is_some_and(CancellationToken::is_cancelled) {
            return WaitResult::Steered;
        }
        let deadline = Instant::now() + timer_duration(wait_ms);
        loop {
            let wake = self.wake.clone();
            let notified = wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            self.reconcile();
            let effect_wake = self.effects.wake();
            let effect_notice = effect_wake.notified();
            tokio::pin!(effect_notice);
            effect_notice.as_mut().enable();
            let settled = self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .rows
                .get(id)
                .and_then(|row| row.snapshot.result.clone());
            if let Some(result) = settled {
                return WaitResult::Completed(result);
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return WaitResult::Aborted,
                _ = async { match steering { Some(token) => token.cancelled().await, None => std::future::pending().await } } => return WaitResult::Steered,
                _ = tokio::time::sleep_until(deadline.into()) => return WaitResult::Running,
                _ = notified => {},
                _ = effect_notice => {},
            }
        }
    }
}
impl Drop for SessionBashOwner {
    fn drop(&mut self) {
        self.close();
    }
}

fn timer_duration(ms: f64) -> Duration {
    // JS setTimeout clamps NaN/nonpositive to a short timer. This helper is
    // limited to the bounded foreground wait, never an OS effect deadline.
    Duration::from_secs_f64(if ms.is_finite() && ms > 0.0 { ms / 1000.0 } else { 0.001 })
}
enum WaitResult {
    Completed(Result<ToolOutput, NativeToolFault>),
    Running,
    Steered,
    Aborted,
}
fn output_text(output: &ToolOutput) -> String {
    output
        .content
        .iter()
        .filter_map(|block| match block {
            ara_ai::UserBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn native_output(result: ara_tools::bash::BashResult, notices: Vec<String>) -> Result<ToolOutput, NativeToolFault> {
    let status = result.native_output_text();
    let cancelled = result.cancelled && !result.timed_out;
    let mut output = result.into_tool_output().map_err(|error| {
        if cancelled { NativeToolFault::BashCancelled(error.0) } else { NativeToolFault::OutcomeUnknown(error.0) }
    })?;
    let wall = output
        .details
        .as_ref()
        .and_then(|details| details.get("wallTimeMs"))
        .and_then(serde_json::Value::as_f64)
        .unwrap_or_default();
    let mut text = if status.is_empty() { "(no output)".to_owned() } else { status };
    text.push_str(&format!("\n\nWall time: {:.2} seconds", wall / 1000.0));
    for notice in notices {
        if !notice.is_empty() {
            text.push('\n');
            text.push_str(&notice);
        }
    }
    if let Some(code) =
        output.details.as_ref().and_then(|details| details.get("exitCode")).and_then(serde_json::Value::as_i64)
    {
        text.push_str(&format!("\n\nCommand exited with code {code}"));
    }
    output.content = ToolOutput::text(text).content;
    Ok(output)
}
fn background_output(id: &str, preview: &str, timeout: Option<f64>, notices: &[String]) -> ToolOutput {
    let mut lines = Vec::new();
    if !preview.trim_end().is_empty() {
        lines.push(preview.trim_end().to_owned());
        lines.push(String::new());
    }
    if !notices.is_empty() {
        lines.extend(notices.iter().cloned());
        lines.push(String::new());
    }
    lines.push(format!("Backgrounded as job {id}; result will be delivered automatically."));
    let mut details = serde_json::json!({"async":{"state":"running","jobId":id,"type":"bash"}});
    match timeout {
        Some(seconds) => details["timeoutSeconds"] = serde_json::json!(seconds),
        None => details["timeoutDisabled"] = true.into(),
    }
    ToolOutput::text(lines.join("\n")).with_details(details)
}

pub(super) struct BashPreparation {
    requested_timeout: ValueHandle,
    background: bool,
    pty: bool,
    prepared: Arc<Mutex<Option<BashPreparedOsValues>>>,
    continuation: Option<BashContinuation>,
    resumed: bool,
    resolved_timeout: Option<Option<f64>>,
}
struct BashContinuation {
    job: HostEffectId,
    promise: NativePendingPromise,
    completion: CapabilityId,
    port: ValueHandle,
    requested_timeout: ValueHandle,
    timeout: Option<f64>,
    completed: bool,
}

impl InvocationRuntime {
    pub(super) fn prepare_bash(
        &self,
        tool: &Arc<BashTool>,
        effective: &dyn ValueRuntime,
    ) -> Result<Action, ActualProviderError> {
        // Fixed destructuring observes all Gets before the body, with defaults
        // only for Undefined (not Null or false).
        let command = self.get("command", effective)?;
        let env = self.get("env", effective)?;
        let timeout = self.get("timeout", effective)?;
        let cwd = self.get("cwd", effective)?;
        let background = self.get("async", effective)?;
        let pty = self.get("pty", effective)?;
        let environment = self.environment(&env, effective)?;
        let heap = self.request.values.heap();
        // The pinned leading-cd scanner observes command methods before the
        // async-enabled error only when cwd is falsy.
        let command_string = if !heap.truthy(&cwd)? { Some(self.string(&command, effective)?) } else { None };
        let background = heap.truthy(&background)?;
        if background && !self.owner.bash_session.settings.async_enabled {
            return Err(ActualProviderError::Callback(
                "Async bash execution is disabled. Enable async.enabled to use async mode.".into(),
            ));
        }
        let pty = heap.truthy(&pty)?;
        let command = match command_string {
            Some(command) => command,
            None => self.string(&command, effective)?,
        };
        let environment = environment
            .into_iter()
            .map(|(key, value)| Ok((key, self.string(&value, effective)?)))
            .collect::<Result<Vec<_>, ActualProviderError>>()?;
        let cwd = if heap.truthy(&cwd)? { Some(self.string(&cwd, effective)?) } else { None };
        let requested_timeout =
            if matches!(heap.snapshot(&timeout)?, ValueSnapshot::Undefined) { heap.number(300.0)? } else { timeout };
        let prepared = Arc::new(Mutex::new(None));
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bash = Some(BashPreparation {
            requested_timeout,
            background,
            pty,
            prepared: prepared.clone(),
            continuation: None,
            resumed: false,
            resolved_timeout: None,
        });
        Ok(Action::BashPreflight(tool.clone(), command, cwd, environment, prepared))
    }
    fn timeout_plan(
        &self,
        effective: &dyn ValueRuntime,
    ) -> Result<(ValueHandle, Option<f64>, Vec<String>, bool, bool), ActualProviderError> {
        let (requested, background, pty) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let bash = state.bash.as_ref().ok_or(ActualProviderError::Protocol("Bash preflight is absent"))?;
            (bash.requested_timeout.clone(), bash.background, bash.pty)
        };
        let heap = self.request.values.heap();
        let timeout = if matches!(heap.snapshot(&requested)?, ValueSnapshot::Number(value) if value == 0.0) {
            None
        } else {
            let raw = match heap.snapshot(&requested)? {
                ValueSnapshot::Null | ValueSnapshot::Undefined => heap.number(300.0)?,
                _ => requested.clone(),
            };
            // Math.min performs Number coercion, including exact original
            // object getter/valueOf throw; no serde or String-to-f64 shortcut.
            let number = heap.to_number(&raw, effective)?;
            let max = self.owner.bash_session.settings.max_timeout_seconds;
            let capped = if max > 0.0 { js_min(number, max) } else { number };
            Some(js_max(1.0, js_min(3600.0, capped)))
        };
        let equal = matches!((heap.snapshot(&requested)?, timeout), (ValueSnapshot::Number(raw), Some(resolved)) if raw == resolved);
        let mut notices = Vec::new();
        if let Some(resolved) = timeout {
            if !equal {
                let primitive = heap.to_primitive(&requested, true, effective)?;
                if matches!(heap.snapshot(&primitive)?, ValueSnapshot::Symbol { .. }) {
                    return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()).into());
                }
                let requested_text = heap
                    .to_string(&primitive, effective)?
                    .to_utf8()
                    .map_err(|_| ActualProviderError::Unsupported("Bash timeout notice UTF16 OS boundary"))?;
                let max = self.owner.bash_session.settings.max_timeout_seconds;
                let limit = if max > 0.0 && resolved == max && max < 3600.0 {
                    format!("global tools.maxTimeout ceiling {}s", ara_prompt::js::f64_to_string(max))
                } else {
                    "allowed range 1-3600s".into()
                };
                notices.push(format!(
                    "Timeout clamped to {}s (requested {requested_text}s; {limit}).",
                    ara_prompt::js::f64_to_string(resolved)
                ));
            }
        }
        // Current concrete CLI/RPC source has no local UI overlay. This is the
        // exact pinned fallback branch, with its original user-facing notice.
        if pty && !background {
            let has_ui = heap.get(&self.request.tool_context, &"hasUI".into(), effective)?;
            if matches!(heap.snapshot(&has_ui)?, ValueSnapshot::Boolean(true)) {
                let ui = heap.get(&self.request.tool_context, &"ui".into(), effective)?;
                if heap.truthy(&ui)? {
                    return Err(ActualProviderError::Unsupported(
                        "Bash original interactive UI PTY host port is not connected",
                    ));
                }
            }
            notices.push("pty requested but unavailable in this environment; ran without a terminal".into());
        }
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bash.as_mut().unwrap().resolved_timeout = Some(timeout);
        Ok((requested, timeout, notices, background, pty))
    }
    pub(super) fn resume_bash_native(&self, effective: &dyn ValueRuntime) -> Result<ValueHandle, ActualProviderError> {
        let run = self.run.as_ref().and_then(Weak::upgrade).ok_or(ActualProviderError::Protocol("Bash Run expired"))?;
        run.check_live()?;
        let (requested, timeout, notices, background, pty) =
            self.timeout_plan(effective).map_err(|error| match error {
                ActualProviderError::Value(ValueError::TypeError(message)) => {
                    match run.native_type_error(&self.request.values, &message, effective) {
                        Ok(reason) => ActualProviderError::Value(ValueError::Thrown(reason)),
                        Err(error) => error.into(),
                    }
                }
                other => other,
            })?;
        let prepared = self
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bash
            .as_ref()
            .ok_or(ActualProviderError::Protocol("Bash preparation absent"))?
            .prepared
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .ok_or(ActualProviderError::Protocol("Bash OS preflight did not settle"))?;
        let scope = run.admission.capture_scope(effective)?;
        let parent = run.admission.capture_parent(effective)?;
        if self.parent.get() != Some(&parent) {
            return Err(ActualProviderError::Protocol("Bash continuation parent changed"));
        }
        let promise = run.admission.prepare_pending(scope, parent)?;
        let completion = capability()?;
        let port = self.request.values.heap().function(completion)?;
        let reservation = run.reserve(&self.request.values, scope, parent, &port)?;
        let job = reservation.id();
        let managed = background
            || (self.owner.bash_session.settings.auto_background_enabled
                && !pty
                && !self.owner.bash_session.at_capacity());
        let process_cancel = if managed { CancellationToken::new() } else { self.cancel.clone() };
        // All original coercion callbacks ran before this repeated exact grant.
        let steering = if managed && !background { self.bash_steering(effective)? } else { None };
        let permit = match run.final_enter(
            &reservation,
            &self.owner,
            self.index,
            self.update,
            &self.request,
            process_cancel.clone(),
        ) {
            Ok(permit) => permit,
            Err(error) => {
                run.release(&reservation);
                return Err(error);
            }
        };
        let publisher = reservation.publisher();
        let update: BashUpdateFn = Arc::new(move |partial| {
            let _ = publisher.partial(ToolOutput::text(partial.output));
        });
        let public = promise.promise.clone();
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bash.as_mut().unwrap().resumed = true;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bash.as_mut().unwrap().continuation =
            Some(BashContinuation {
                job,
                promise,
                completion,
                port,
                requested_timeout: requested,
                timeout,
                completed: false,
            });
        let timeout_unknown = Arc::new(AtomicBool::new(false));
        let process_settled = Arc::new(AtomicBool::new(false));
        let future = execute_action(
            self.owner.bash_session.clone(),
            prepared,
            timeout,
            notices,
            background,
            managed,
            permit,
            process_cancel,
            self.cancel.clone(),
            steering,
            update,
            timeout_unknown.clone(),
            process_settled.clone(),
        );
        if let Err(error) = reservation.enter_bash(self.cancel.clone(), future, timeout_unknown, process_settled) {
            run.finish_graph(job);
            return Err(ActualProviderError::Callback(error.message().to_owned()));
        }
        Ok(public)
    }
    pub(super) fn resume_bash_legacy(&self) -> Result<LegacyEffect, ActualProviderError> {
        let (_requested, timeout, notices, background, pty) = self.timeout_plan(self)?;
        let prepared = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let bash = state.bash.as_mut().ok_or(ActualProviderError::Protocol("Bash preflight absent"))?;
            bash.resumed = true;
            let prepared = bash
                .prepared
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or(ActualProviderError::Protocol("Bash OS preflight not settled"))?;
            prepared
        };
        let managed = background
            || (self.owner.bash_session.settings.auto_background_enabled
                && !pty
                && !self.owner.bash_session.at_capacity());
        let process_cancel = if managed { CancellationToken::new() } else { self.cancel.clone() };
        let steering = if managed && !background { self.bash_steering(self)? } else { None };
        let permit = self.owner.enter(self.index, self.update, &self.request, process_cancel.clone())?;
        let partials = Arc::new(Mutex::new(VecDeque::new()));
        let wake = Arc::new(Notify::new());
        let native_partials = partials.clone();
        let native_wake = wake.clone();
        let update: BashUpdateFn = Arc::new(move |partial| {
            native_partials.lock().unwrap_or_else(|e| e.into_inner()).push_back(ToolOutput::text(partial.output));
            native_wake.notify_waiters();
        });
        Ok(LegacyEffect {
            future: execute_action(
                self.owner.bash_session.clone(),
                prepared,
                timeout,
                notices,
                background,
                managed,
                permit,
                process_cancel,
                self.cancel.clone(),
                steering,
                update,
                Arc::new(AtomicBool::new(false)),
                Arc::new(AtomicBool::new(false)),
            ),
            partials,
            wake,
        })
    }
    pub(super) fn finish_bash_legacy(&self, result: &ValueHandle) -> Result<(), ActualProviderError> {
        let (requested, timeout) = {
            let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(bash) = state.bash.as_ref() else {
                return Ok(());
            };
            let Some(timeout) = bash.resolved_timeout else {
                return Ok(());
            };
            (bash.requested_timeout.clone(), timeout)
        };
        Ok(decorate_graph_timeout(self.request.values.heap(), result, &requested, timeout, self)?)
    }
    pub(super) fn bash_steering(&self, effective: &dyn ValueRuntime) -> Result<Option<CancellationToken>, ActualProviderError> {
        let heap = self.request.values.heap();
        let call = heap.get(&self.request.tool_context, &"toolCall".into(), effective)?;
        if matches!(heap.snapshot(&call)?, ValueSnapshot::Undefined | ValueSnapshot::Null) {
            return Ok(None);
        }
        let signal = heap.get(&call, &"steeringSignal".into(), effective)?;
        let signals = self.owner.context_signals.lock().unwrap_or_else(|e| e.into_inner());
        Ok(signals
            .capabilities
            .values()
            .find(|(candidate, _)| candidate.same_identity(&signal))
            .map(|(_, token)| token.clone()))
    }
    pub(super) fn bash_completion_capability(&self, id: CapabilityId) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .bash
            .as_ref()
            .and_then(|bash| bash.continuation.as_ref())
            .is_some_and(|continuation| continuation.completion == id)
    }
    pub(super) fn complete_bash_native(
        &self,
        receiver: &ValueHandle,
        args: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle, ValueError> {
        let run = self.run.as_ref().and_then(Weak::upgrade).ok_or(ValueError::ExpiredCapability(self.completion))?;
        run.check_live().map_err(|_| ValueError::ExpiredCapability(self.completion))?;
        let heap = self.request.values.heap();
        if args.len() != 2 {
            return Err(ValueError::WrongRuntimeReply("Bash completion arity"));
        }
        let ValueSnapshot::Number(kind) = heap.snapshot(&args[0])? else {
            return Err(ValueError::WrongRuntimeReply("Bash completion kind"));
        };
        let (job, resolve, reject, requested, timeout) = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.phase != Phase::Running {
                return Err(ValueError::ExpiredCapability(self.completion));
            }
            let continuation = state
                .bash
                .as_mut()
                .and_then(|bash| bash.continuation.as_mut())
                .ok_or(ValueError::ExpiredCapability(self.completion))?;
            if continuation.completed || !receiver.same_identity(&continuation.port) {
                return Err(ValueError::ExpiredCapability(continuation.completion));
            }
            if kind == 1.0 || kind == 2.0 {
                continuation.completed = true;
            } else if kind != 0.0 {
                return Err(ValueError::WrongRuntimeReply("Bash completion kind"));
            }
            (
                continuation.job,
                continuation.promise.resolve.clone(),
                continuation.promise.reject.clone(),
                continuation.requested_timeout.clone(),
                continuation.timeout,
            )
        };
        if kind == 0.0 {
            return match heap.call(&self.arguments[3], &heap.undefined()?, &[args[1].clone()], effective) {
                Ok(_) | Err(ValueError::Thrown(_)) => heap.undefined(),
                Err(error) => {
                    let failure = self.state.lock().unwrap_or_else(|e| e.into_inner()).update_failure.clone();
                    if let Some(failure) = failure {
                        run.fail(failure);
                    }
                    Err(error)
                }
            };
        }
        if kind == 1.0 {
            decorate_graph_timeout(heap, &args[1], &requested, timeout, effective)?;
        }
        heap.call(if kind == 1.0 { &resolve } else { &reject }, &heap.undefined()?, &[args[1].clone()], effective)?;
        run.finish_graph(job);
        heap.undefined()
    }
    pub(super) fn has_bash_preflight(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).bash.as_ref().is_some_and(|bash| !bash.resumed)
    }
}

fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.min(b) }
}
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}
fn decorate_graph_timeout(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    result: &ValueHandle,
    requested: &ValueHandle,
    timeout: Option<f64>,
    runtime: &dyn ValueRuntime,
) -> Result<(), ValueError> {
    let details = heap.get(result, &"details".into(), runtime)?;
    match timeout {
        Some(timeout) => {
            heap.define_own_data(&details, "timeoutSeconds".into(), heap.number(timeout)?, runtime)?;
            if !matches!(heap.snapshot(requested)?, ValueSnapshot::Number(raw) if raw == timeout) {
                heap.define_own_data(&details, "requestedTimeoutSeconds".into(), requested.clone(), runtime)?;
            }
        }
        None => {
            heap.define_own_data(&details, "timeoutDisabled".into(), heap.boolean(true)?, runtime)?;
            heap.define_own_data(&details, "requestedTimeoutSeconds".into(), requested.clone(), runtime)?;
        }
    }
    Ok(())
}

pub(super) fn preflight_future(
    tool: Arc<BashTool>,
    command: String,
    cwd: Option<String>,
    env: Vec<(String, String)>,
    prepared: Arc<Mutex<Option<BashPreparedOsValues>>>,
    permit: EffectPermit,
) -> NativeEffectFuture {
    Box::pin(async move {
        let _permit = permit;
        let value = prepare_bash_os_values(&tool.ctx, &command, cwd.as_deref(), &env)
            .await
            .map_err(|error| NativeToolFault::NotDispatched(error.0))?;
        *prepared.lock().unwrap_or_else(|e| e.into_inner()) = Some(value);
        Ok(ToolOutput::text(""))
    })
}
fn execute_action(
    owner: Arc<SessionBashOwner>,
    prepared: BashPreparedOsValues,
    timeout: Option<f64>,
    notices: Vec<String>,
    explicit: bool,
    managed: bool,
    permit: EffectPermit,
    process_cancel: CancellationToken,
    foreground_cancel: CancellationToken,
    steering: Option<CancellationToken>,
    update: BashUpdateFn,
    timeout_unknown: Arc<AtomicBool>,
    process_settled: Arc<AtomicBool>,
) -> NativeEffectFuture {
    Box::pin(async move {
        if !managed {
            return execute_foreground(
                prepared,
                timeout,
                notices,
                permit,
                process_cancel,
                foreground_cancel,
                update,
                timeout_unknown,
                process_settled,
            )
            .await;
        }
        let threshold = owner.settings.auto_background_threshold_ms.floor().max(0.0);
        let wait_ms = if threshold <= 0.0 {
            0.0
        } else {
            match timeout {
                None => threshold,
                Some(timeout) => js_max(0.0, js_min(threshold, timeout * 1000.0 - 1000.0)),
            }
        };
        let immediate = explicit || wait_ms == 0.0;
        let started = owner.start(
            prepared,
            timeout,
            notices.clone(),
            permit,
            process_cancel.clone(),
            if immediate { None } else { Some(update.clone()) },
            !immediate,
            !explicit,
        )?;
        let id = match started {
            JobStart::Started(id) => id,
            JobStart::Foreground { prepared, permit } => {
                // The original permit still owns its independent grant token.
                // Foreground cancellation must also cancel and join this runner.
                return execute_foreground(
                    prepared,
                    timeout,
                    notices,
                    permit,
                    process_cancel,
                    foreground_cancel,
                    update,
                    timeout_unknown,
                    process_settled,
                )
                .await;
            }
        };
        if immediate {
            return Ok(background_output(&id, "", timeout, &notices));
        }
        match owner.wait(&id, wait_ms, &foreground_cancel, steering.as_ref()).await {
            WaitResult::Completed(result) => {
                owner.stop_updates(&id);
                let (unknown, settled) = owner
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .rows
                    .get(&id)
                    .map(|row| {
                        (row.timeout_unknown.load(Ordering::Acquire), row.process_settled.load(Ordering::Acquire))
                    })
                    .unwrap_or((true, false));
                timeout_unknown.store(unknown, Ordering::Release);
                process_settled.store(settled, Ordering::Release);
                result
            }
            WaitResult::Aborted => {
                owner.cancel(&id);
                owner.stop_updates(&id);
                let effect = owner.state.lock().unwrap_or_else(|e| e.into_inner()).rows.get(&id).map(|row| row.effect);
                let receipt = match effect {
                    Some(effect) => owner.effects.settle_one(effect).await,
                    None => None,
                };
                let known = receipt.as_ref().is_some_and(HostEffectReceipt::confirmed_bash_settlement);
                // Native receipt for this exact Job, never latest text/details.
                process_settled.store(known, Ordering::Release);
                timeout_unknown.store(!known, Ordering::Release);
                let latest = owner
                    .snapshots()
                    .into_iter()
                    .find(|row| row.id == id)
                    .map(|row| row.latest_text)
                    .unwrap_or_default();
                let text = if latest.is_empty() { "Command aborted".into() } else { latest };
                Err(if known { NativeToolFault::BashCancelled(text) } else { NativeToolFault::OutcomeUnknown(text) })
            }
            state => {
                owner.stop_updates(&id);
                owner.resume_deliveries(&id);
                let mut notices = notices;
                if matches!(state, WaitResult::Steered) {
                    notices.push("Backgrounded early to handle an incoming message; the command keeps running.".into());
                }
                let latest = owner
                    .snapshots()
                    .into_iter()
                    .find(|row| row.id == id)
                    .map(|row| row.latest_text)
                    .unwrap_or_default();
                Ok(background_output(&id, &latest, timeout, &notices))
            }
        }
    })
}
fn execute_foreground(
    prepared: BashPreparedOsValues,
    timeout: Option<f64>,
    notices: Vec<String>,
    permit: EffectPermit,
    process_cancel: CancellationToken,
    foreground_cancel: CancellationToken,
    update: BashUpdateFn,
    timeout_unknown: Arc<AtomicBool>,
    process_settled: Arc<AtomicBool>,
) -> NativeEffectFuture {
    Box::pin(async move {
        let _permit = permit;
        let dispatch = BashProcessDispatchReceipt::default();
        let process =
            execute_bash_prepared_os_values_with_receipt(prepared, timeout, process_cancel.clone(), update, &dispatch);
        tokio::pin!(process);
        let result = tokio::select! {
            biased;
            _ = foreground_cancel.cancelled() => {
                process_cancel.cancel();
                process.await
            }
            result = &mut process => result,
        };
        if let Ok(result) = &result {
            process_settled.store(result.process_settlement_confirmed(), Ordering::Release);
            timeout_unknown.store(
                result.timed_out || ((cfg!(windows) || result.cancelled) && !result.process_settlement_confirmed()),
                Ordering::Release,
            );
        }
        result
            .map_err(|error| {
                if dispatch.was_launched() {
                    NativeToolFault::OutcomeUnknown(error.0)
                } else {
                    NativeToolFault::NotDispatched(error.0)
                }
            })
            .and_then(|result| native_output(result, notices))
    })
}

static BASH_CONCURRENCY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da:tools/bash.ts:612 args.pty === true ? exclusive : shared",
    name: "concurrency",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: |heap, _, _, arguments, effective| {
        let args = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let pty = heap.get(&args, &"pty".into(), effective)?;
        heap.string(
            if matches!(heap.snapshot(&pty)?, ValueSnapshot::Boolean(true)) { "exclusive" } else { "shared" }.into(),
        )
    },
};
pub(super) fn concurrency(heap: &Arc<ara_rpc::value_heap::ValueHeap>) -> Result<ValueHandle, ValueError> {
    heap.fresh_compiled_function(&BASH_CONCURRENCY, &[])
}

/// Suppresses automatic delivery for exactly the initial snapshot, not new jobs.
pub struct BashWaitWatch {owner:Arc<SessionBashOwner>,token:u64,ids:Vec<String>}
impl BashWaitWatch {
    pub fn snapshots(&self)->Vec<BashJobSnapshot>{self.owner.snapshots().into_iter().filter(|row|self.ids.contains(&row.id)).collect()}
    pub fn reserve_ready(&self)->Vec<BashJobSnapshot>{
        self.owner.reconcile();let mut state=self.owner.state.lock().unwrap_or_else(|e|e.into_inner());let mut ready=Vec::new();
        for id in &self.ids {if state.wait_watch.get(id)!=Some(&self.token)||state.wait_reserved.contains_key(id)||state.consumed.contains(id){continue;}
            if let Some(row)=state.rows.get(id).filter(|row|row.snapshot.result.is_some()){ready.push(row.snapshot.clone());state.wait_reserved.insert(id.clone(),self.token);}
        }ready
    }
}
impl Drop for BashWaitWatch {fn drop(&mut self){let mut state=self.owner.state.lock().unwrap_or_else(|e|e.into_inner());
    for id in &self.ids {if state.wait_watch.get(id)==Some(&self.token)&&!state.wait_reserved.contains_key(id){state.wait_watch.remove(id);let ready=state.rows.get(id).is_some_and(|row|row.snapshot.result.is_some());if ready&&!state.consumed.contains(id)&&!state.suppressed.contains(id)&&!state.deliveries.contains(id){state.deliveries.push_back(id.clone());}}}
    drop(state);self.owner.wake.notify_waiters();
}}
