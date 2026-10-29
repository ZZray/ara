//! CLI Native Run ownership: one graph driver and an independent effect join owner.
//! This connects prepared Native hosts only; complete Session/tool parity remains open.

use crate::{
    actual_tools::NativeBuiltinToolHooks,
    actual_tools::async_effect::{HostEffectSettlementOwner, NativeBuiltinRun, NativeHostDrain},
    mcp_native::{NativeMcpRun, NativeMcpSource},
    native_factory::PreparedNativeSession,
    native_factory::catalog::NativeCatalogSnapshot,
    native_realm::promise::{
        NativeDrain, NativePromiseAdmission, NativePromiseRevoker, NativePromiseRun,
    },
    native_rpc_host::{NativeRpcHostRun, NativeRpcHostSource},
};
use ara_agent::ActualProviderError;
mod provider_receipt;
pub(crate) use provider_receipt::ProviderDispatchReservation;
use provider_receipt::ProviderDispatchState;
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use futures::{FutureExt, StreamExt, future::BoxFuture, stream::FuturesUnordered};
use tokio_util::sync::CancellationToken;

fn remember_failure(failure: &mut Option<ActualProviderError>, next: ActualProviderError) {
    *failure = Some(match failure.take() {
        Some(primary) => ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(next),
        },
        None => next,
    });
}

/// ActiveRun retains this owner independently of the spawned driver. The complete
/// preparation survives a panic or early return until its entered effects join.
struct RunRegistry {
    snapshots: Vec<Arc<NativeCatalogSnapshot>>,
    mcp: Vec<(Arc<NativeMcpSource>, Arc<NativeMcpRun>)>,
    rpc: Vec<(Arc<NativeRpcHostSource>, Arc<NativeRpcHostRun>)>,
    effects: Vec<Arc<HostEffectSettlementOwner>>,
    provider_dispatches: Vec<Arc<ProviderDispatchState>>,
    provider_drains: Vec<BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>>,
    closed: bool,
    attaching: bool,
}

pub struct NativeRunLease {
    prepared: Arc<PreparedNativeSession>,
    tools: Arc<NativeBuiltinRun>,
    effects: Arc<HostEffectSettlementOwner>,
    promise: NativePromiseRevoker,
    admission: NativePromiseAdmission,
    cancel: CancellationToken,
    registry: Mutex<RunRegistry>,
    registry_wake: Arc<Notify>,
}
impl NativeRunLease {
    /// Borrowed source finalization port. Callers retain this lease before
    /// moving NativeRun into drive; no mutable driver escapes to the callback.
    pub fn prepare_terminal_return(self: &Arc<Self>, iterator: ara_ai::retry::idle::TerminalGraceIterator) -> Result<BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>, ara_rpc::value_heap::ValueError> {
        let admission = self.provider_drain_admission()?;
        let returned = iterator.prepare_consumer_return()?;
        if returned.await_drain { Ok(returned.drain) }
        else { admission.transfer(returned.drain)?; Ok(futures::future::ready(Ok(())).boxed()) }
    }
    /// Capture before the actual provider Call. This admits only that source's
    /// eventual bounded drain, not arbitrary work after Run seal.
    pub(crate) fn provider_drain_admission(self: &Arc<Self>) -> Result<ProviderDrainAdmission, ara_rpc::value_heap::ValueError> {
        self.admission.check_live()?;
        if self.registry.lock().unwrap_or_else(|e| e.into_inner()).closed {
            return Err(ara_rpc::value_heap::ValueError::UnsupportedOperation("Native provider drain admission is sealed"));
        }
        Ok(ProviderDrainAdmission { lease: Arc::downgrade(self), transferred: Arc::new(std::sync::atomic::AtomicBool::new(false)) })
    }
    fn take_provider_drains(&self) -> Vec<BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>> {
        std::mem::take(&mut self.registry.lock().unwrap_or_else(|e| e.into_inner()).provider_drains)
    }
    /// Each provider reader follows this original Run while local EOF/return
    /// may close its child without cancelling sibling operations or the Run.
    pub(crate) fn provider_cancel_token(&self) -> CancellationToken {
        self.cancel.child_token()
    }
    /// Reserve in this live Run before invoking a registered provider. The
    /// slot is retained by the independent lease across graph revoke/drop.
    pub(crate) fn reserve_provider_dispatch(
        &self,
    ) -> Result<ProviderDispatchReservation, ActualProviderError> {
        self.admission.check_live()?;
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        if registry.closed {
            return Err(ActualProviderError::Protocol(
                "Original provider resource admission is closed",
            ));
        }
        registry
            .provider_dispatches
            .try_reserve(1)
            .map_err(|_| ara_rpc::value_heap::ValueError::Allocation)?;
        let slot = ProviderDispatchState::pending();
        registry.provider_dispatches.push(slot.clone());
        Ok(ProviderDispatchReservation::new(slot))
    }
    fn remote_runs(&self) -> (Vec<Arc<NativeMcpRun>>, Vec<Arc<NativeRpcHostRun>>) {
        let registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        (
            registry.mcp.iter().map(|(_, run)| run.clone()).collect(),
            registry.rpc.iter().map(|(_, run)| run.clone()).collect(),
        )
    }
    /// Attach actual refreshed sources in this same Run/admission. The single
    /// Builtin context/effect queue is shared by every original builtin source.
    /// New remote sources own independent ports and settlement receipts.
    pub fn attach_catalog_snapshot(
        &self,
        snapshot: Arc<NativeCatalogSnapshot>,
    ) -> Result<(), ActualProviderError> {
        self.admission.check_values(self.prepared.values())?;
        self.admission.check_live()?;
        snapshot.check_values(self.prepared.values())?;
        self.tools.check_live()?;
        let catalog = self.prepared.require_catalog()?;
        let (existing_mcp, existing_rpc) = {
            let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
            if registry.closed || registry.attaching {
                return Err(ActualProviderError::Protocol(
                    "Native source attachment is closed or already in preparation",
                ));
            }
            if registry
                .snapshots
                .iter()
                .any(|old| Arc::ptr_eq(old, &snapshot))
            {
                return if Arc::ptr_eq(&catalog.snapshot(self.prepared.values())?, &snapshot) {
                    Ok(())
                } else {
                    Err(ActualProviderError::Protocol(
                        "Old Native snapshot cannot silently replace current membership",
                    ))
                };
            }
            registry.attaching = true;
            (registry.mcp.clone(), registry.rpc.clone())
        };
        // No registry lock spans a graph operation, source binding or callback.
        let prepared_ports = (|| {
            self.prepared
                .require_tool_hooks()?
                .prepare_catalog_snapshot(
                    &snapshot,
                    self.prepared.session_state(),
                    self.prepared.values(),
                )?;
            NativeBuiltinToolHooks::bind_source_run(snapshot.builtin(), &self.tools)?;
            let mcp = match snapshot.mcp() {
                Some(source) if !existing_mcp.iter().any(|(old, _)| Arc::ptr_eq(old, source)) => {
                    Some((
                        source.clone(),
                        NativeMcpRun::new(source.clone(), self.admission.clone())?,
                    ))
                }
                _ => None,
            };
            let rpc = match snapshot.rpc() {
                Some(source) if !existing_rpc.iter().any(|(old, _)| Arc::ptr_eq(old, source)) => {
                    Some((
                        source.clone(),
                        NativeRpcHostRun::new(source.clone(), self.admission.clone())?,
                    ))
                }
                _ => None,
            };
            Ok::<_, ActualProviderError>((mcp, rpc))
        })();
        let mut registry = self.registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.attaching = false;
        let (mcp, rpc) = prepared_ports?;
        if registry.closed || self.cancel.is_cancelled() || self.admission.check_live().is_err() {
            if let Some((_, run)) = &mcp {
                run.revoke();
            }
            if let Some((_, run)) = &rpc {
                run.revoke();
            }
            return Err(ActualProviderError::Protocol(
                "Native attachment lost its original Run admission",
            ));
        }
        let counts = (
            registry.snapshots.len(),
            registry.mcp.len(),
            registry.rpc.len(),
            registry.effects.len(),
        );
        registry.snapshots.push(snapshot.clone());
        if let Some((source, run)) = mcp {
            registry.effects.push(run.effects());
            registry.mcp.push((source, run));
        }
        if let Some((source, run)) = rpc {
            registry.effects.push(run.effects());
            registry.rpc.push((source, run));
        }
        // Checked handle/owner publication has no Get/Call/await. New ports are
        // registered before they can be selected through current membership.
        if let Err(error) = catalog.publish(snapshot, self.prepared.values(), Some(&self.tools)) {
            registry.snapshots.truncate(counts.0);
            registry.mcp.truncate(counts.1);
            registry.rpc.truncate(counts.2);
            registry.effects.truncate(counts.3);
            return Err(error);
        }
        drop(registry);
        self.registry_wake.notify_waiters();
        self.admission.wake().notify_waiters();
        Ok(())
    }
    pub fn revoke_graph(&self) {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        let (mcp, rpc) = self.remote_runs();
        self.tools.revoke();
        for run in mcp {
            run.revoke_graph();
        }
        for run in rpc {
            run.revoke_graph();
        }
        self.promise.revoke();
        self.registry_wake.notify_waiters();
    }
    pub fn revoke(&self) {
        self.revoke_graph();
        self.cancel.cancel();
        for owner in self.effect_owners() {
            owner.cancel();
        }
    }
    fn seal(&self) -> Result<(), ActualProviderError> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .closed = true;
        let (mcp, rpc) = self.remote_runs();
        let mut failure = None;
        if let Err(error) = self.tools.seal() {
            remember_failure(&mut failure, error);
        }
        for run in mcp {
            if let Err(error) = run.seal() {
                remember_failure(&mut failure, error);
            }
        }
        for run in rpc {
            if let Err(error) = run.seal() {
                remember_failure(&mut failure, error);
            }
        }
        for owner in self.effect_owners() {
            owner.seal();
        }
        self.registry_wake.notify_waiters();
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub async fn settle(&self) -> Result<(), ActualProviderError> {
        // Receipts are retained by the native owner across repeated callers.
        let mut failure = self.seal().err();
        // Drop queued graph repair work before joining its Rust receipts.
        // Settlement does not recursively drive graph continuations.
        if let Err(error) = self.tools.prepare_repair_settlement() {
            remember_failure(&mut failure, error);
        }
        let owners = self.effect_owners();
        // Seal forbids new slots; admitted synchronous dispatches may still
        // attach their original receipts. Their slot settlement waits for that
        // attachment without polling any revoked graph continuation.
        let provider_dispatches = self
            .registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .provider_dispatches
            .clone();
        let receipts = futures::future::join_all(owners.iter().map(|owner| owner.settle())).await;
        let unknown: Vec<_> = receipts
            .into_iter()
            .enumerate()
            .flat_map(|(index, receipts)| {
                receipts
                    .into_iter()
                    .filter(|receipt| receipt.uncertain())
                    .map(move |receipt| format!("owner {index}: {}", receipt.summary()))
            })
            .collect();
        if !unknown.is_empty() {
            remember_failure(
                &mut failure,
                ActualProviderError::Callback(format!(
                    "Native entered effects have unknown outcomes: {}",
                    unknown.join("; ")
                )),
            );
        }
        for receipt in
            futures::future::join_all(provider_dispatches.iter().map(|slot| slot.settle())).await
        {
            if let Err(error) = receipt {
                remember_failure(&mut failure, error);
            }
        }
        if let Err(error) = self.tools.settle_repair_resources().await {
            remember_failure(&mut failure, error);
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub fn prepared(&self) -> &Arc<PreparedNativeSession> {
        &self.prepared
    }
    pub fn effects(&self) -> &Arc<HostEffectSettlementOwner> {
        &self.effects
    }
    /// Session Bash background Jobs intentionally do not appear in this list.
    pub fn effect_owners(&self) -> Vec<Arc<HostEffectSettlementOwner>> {
        self.registry
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .effects
            .clone()
    }
}

/// One original source may transfer its already-admitted tail to the SAME
/// NativeRun. The OS cleanup owner is separate; this resource may evaluate Graph.
#[derive(Clone)]
pub(crate) struct ProviderDrainAdmission {
    lease: std::sync::Weak<NativeRunLease>,
    transferred: Arc<std::sync::atomic::AtomicBool>,
}
impl ProviderDrainAdmission {
    pub(crate) fn transfer(&self, drain: BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>) -> Result<(), ara_rpc::value_heap::ValueError> {
        let lease = self.lease.upgrade().ok_or(ara_rpc::value_heap::ValueError::UnsupportedOperation("Original provider drain Run expired"))?;
        lease.admission.check_live()?;
        if self.transferred.swap(true, std::sync::atomic::Ordering::AcqRel) { return Err(ara_rpc::value_heap::ValueError::UnsupportedOperation("Original provider drain transferred twice")); }
        lease.registry.lock().unwrap_or_else(|e| e.into_inner()).provider_drains.push(drain);
        lease.registry_wake.notify_waiters();
        Ok(())
    }
}

/// Only this owner has the mutable Promise driver. No graph callback or tool
/// Invoker can pump it recursively. Drop can revoke/cancel, never perform a join.
pub struct NativeRun {
    promise: NativePromiseRun,
    lease: Arc<NativeRunLease>,
    finished: bool,
    provider_drains: FuturesUnordered<BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>>,
}
impl NativeRun {
    pub fn begin(
        prepared: Arc<PreparedNativeSession>,
        cancel: CancellationToken,
    ) -> Result<Self, ActualProviderError> {
        let hooks = prepared.require_tool_hooks()?;
        let promise = prepared.begin_promise_run()?;
        let admission = promise.admission();
        let effects = Arc::new(HostEffectSettlementOwner::new());
        let tools = NativeBuiltinRun::new(
            prepared.values(),
            admission.clone(),
            effects.clone(),
            prepared.read_continuation(),
            &prepared.error_constructor(),
        )?;
        tools.bind_retry_realm(prepared.realm())?;
        let snapshot = prepared
            .catalog()
            .map(|catalog| catalog.snapshot(prepared.values()))
            .transpose()?;
        if let Some(snapshot) = &snapshot {
            NativeBuiltinToolHooks::bind_source_run(snapshot.builtin(), &tools)?;
            prepared
                .require_catalog()?
                .bind_run_snapshot(snapshot, &tools, prepared.values())?;
        } else {
            hooks.bind_run(&tools)?;
        }
        let mut registry = RunRegistry {
            snapshots: Vec::new(),
            mcp: Vec::new(),
            rpc: Vec::new(),
            effects: vec![effects.clone()],
            provider_dispatches: Vec::new(),
            provider_drains: Vec::new(),
            closed: false,
            attaching: false,
        };
        let ports = (|| {
            if let Some(snapshot) = &snapshot {
                if let Some(source) = snapshot.mcp() {
                    let run = NativeMcpRun::new(source.clone(), admission.clone())?;
                    registry.effects.push(run.effects());
                    registry.mcp.push((source.clone(), run));
                }
                if let Some(source) = snapshot.rpc() {
                    let run = NativeRpcHostRun::new(source.clone(), admission.clone())?;
                    registry.effects.push(run.effects());
                    registry.rpc.push((source.clone(), run));
                }
                registry.snapshots.push(snapshot.clone());
            }
            Ok::<_, ActualProviderError>(())
        })();
        if let Err(error) = ports {
            tools.revoke();
            for (_, run) in &registry.mcp {
                run.revoke();
            }
            for (_, run) in &registry.rpc {
                run.revoke();
            }
            promise.revoker().revoke();
            return Err(error);
        }
        let lease = Arc::new(NativeRunLease {
            prepared,
            tools,
            effects,
            promise: promise.revoker(),
            admission,
            cancel,
            registry: Mutex::new(registry),
            registry_wake: Arc::new(Notify::new()),
        });
        Ok(Self {
            promise,
            lease,
            finished: false,
            provider_drains: FuturesUnordered::new(),
        })
    }
    /// Finalize an original native terminal consumer while this Run still
    /// owns its sole driver. The source guard/Get occurs synchronously here.
    pub fn prepare_terminal_return(
        &self,
        iterator: ara_ai::retry::idle::TerminalGraceIterator,
    ) -> Result<BoxFuture<'static, Result<(), ara_rpc::value_heap::ValueError>>, ara_rpc::value_heap::ValueError> {
        self.lease.prepare_terminal_return(iterator)
    }
    pub fn lease(&self) -> Arc<NativeRunLease> {
        self.lease.clone()
    }
    /// Use this same Run's authenticated root scope and original parent for
    /// synchronous graph calls. Do not send the adapter to an OS worker or
    /// retain it inside the original parent's own state.
    pub fn root_runtime(
        &self,
    ) -> Result<crate::native_realm::parent::NativeScopedParent, ara_rpc::value_heap::ValueError>
    {
        self.promise
            .scoped(self.promise.root_scope(), self.promise.root_parent())
    }
    /// Capture this Run's actual admission/root parent for provider utilities.
    /// The caller supplies its original factory; no new Session or driver is made.
    pub fn calling_run(
        &self,
        factory: &Arc<ara_ai::graph_values::GraphValueFactory>,
    ) -> Result<crate::mock_provider_host::NativeCallingRun, ara_rpc::value_heap::ValueError> {
        crate::mock_provider_host::NativeCallingRun::capture(
            factory,
            self.lease.prepared.values(),
            self.promise.admission(),
            &self.root_runtime()?,
        )
    }
    fn drain_one(&mut self) -> Result<bool, ActualProviderError> {
        // Alternate the two queues so admitted partials/completion and Promise
        // reactions both make progress; each queue preserves its own FIFO.
        let host = self.lease.tools.drain_one(self.lease.prepared.values())?;
        let (mcp, rpc) = self.lease.remote_runs();
        let mut progress = !matches!(host, NativeHostDrain::Idle);
        // One notice per original host queue per round, then one Promise
        // reaction. Newly attached sources enter the next round, same driver.
        for run in mcp {
            progress |= run.drain_one()?;
        }
        for run in rpc {
            progress |= run.drain_one()?;
        }
        let promise = self.promise.drain_one().map_err(|error| {
            // An original builtin continuation may save its complete failure
            // before escaping the Promise driver with an infrastructure marker.
            // Promise admission is already revoked here; query only that receipt.
            match self.lease.tools.retained_failure() {
                Ok(Some(failure)) => failure,
                Ok(None) => error.into(),
                Err(failure) => ActualProviderError::Compound {
                    primary: Arc::new(error.into()),
                    secondary: Arc::new(failure),
                },
            }
        })?;
        progress |= !matches!(promise, NativeDrain::Idle);
        Ok(progress)
    }
    async fn drain_ready(&mut self) -> Result<(), ActualProviderError> {
        let mut burst = 0;
        while self.drain_one()? {
            burst += 1;
            if burst == 64 {
                tokio::task::yield_now().await;
                burst = 0;
            }
        }
        Ok(())
    }
    fn drain_burst(&mut self) -> Result<bool, ActualProviderError> {
        for _ in 0..64 {
            if !self.drain_one()? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// Drive the original Agent future and all admitted graph work in one task.
    /// An observation wake is enabled before inspecting either queue.
    pub async fn drive<F: Future>(self, agent: F) -> Result<F::Output, ActualProviderError> {
        self.drive_with_display(agent, None).await
    }
    pub async fn drive_with_display<F: Future>(
        mut self,
        agent: F,
        display: Option<Arc<tokio::sync::Mutex<crate::terminal::evaluation::DisplayEvaluator>>>,
    ) -> Result<F::Output, ActualProviderError> {
        let display_port = if let Some(display) = &display {
            Some(display.lock().await.port.clone())
        } else {
            None
        };
        let display_wake = if let Some(display) = &display {
            Some(display.lock().await.wake())
        } else {
            None
        };
        tokio::pin!(agent);
        let mut failure = None;
        let mut revoked = false;
        let mut cancellation_sealed = false;
        let outcome = loop {
            for drain in self.lease.take_provider_drains() { self.provider_drains.push(drain); }
            let mut more_ready = false;
            let display_notice = async {
                if let Some(wake) = &display_wake {
                    wake.notified().await
                } else {
                    std::future::pending::<()>().await
                }
            };
            let display_demand = async {
                if let Some(port) = &display_port {
                    port.ready().await
                } else {
                    std::future::pending::<()>().await
                }
            };
            if let Some(display) = &display {
                more_ready |= display.lock().await.pump_quantum();
            }
            let promise_wake = self.promise.wake();
            let registry_wake = self.lease.registry_wake.clone();
            let promise_notice = promise_wake.notified();
            let registry_notice = registry_wake.notified();
            tokio::pin!(promise_notice, registry_notice);
            promise_notice.as_mut().enable();
            registry_notice.as_mut().enable();
            // Enable registry wake before capturing owners: a concurrent
            // attachment stays observable even before its own wake is listed.
            let mut host_notices: Vec<_> = self
                .lease
                .effect_owners()
                .into_iter()
                .map(|owner| Box::pin(owner.wake().notified_owned()))
                .collect();
            for notice in &mut host_notices {
                notice.as_mut().enable();
            }
            let host_notice = futures::future::select_all(host_notices);
            tokio::pin!(host_notice);
            if !revoked {
                if self.lease.cancel.is_cancelled() && !cancellation_sealed {
                    // Stop new effect/attachment admission, retaining the
                    // current Graph and Promise jobs until entered methods
                    // settle and publish their genuine cancelled results.
                    cancellation_sealed = true;
                    if let Err(error) = self.lease.seal() {
                        remember_failure(&mut failure, error);
                        self.lease.revoke();
                        revoked = true;
                    }
                    for owner in self.lease.effect_owners() {
                        owner.cancel();
                    }
                }
                if !revoked {
                    match self.drain_burst() {
                        Ok(more) => more_ready |= more,
                        Err(error) => {
                            if !self.lease.cancel.is_cancelled() {
                                remember_failure(&mut failure, error);
                            }
                            self.lease.revoke();
                            revoked = true;
                        }
                    }
                }
            }
            // After revoke, the same Agent future still gets polled so its Run
            // guard and owned tool waits settle; it is never replaced or replayed.
            tokio::select! {
                result = &mut agent => break result,
                Some(result) = self.provider_drains.next(), if !self.provider_drains.is_empty() && !revoked => {
                    if let Err(error) = result { remember_failure(&mut failure, error.into()); }
                },
                _ = display_notice => {},
                _ = display_demand => {},
                _ = self.lease.cancel.cancelled(), if !revoked && !cancellation_sealed => {},
                _ = &mut promise_notice, if !revoked => {},
                _ = &mut host_notice, if !revoked => {},
                _ = &mut registry_notice, if !revoked => {},
                _ = tokio::task::yield_now(), if more_ready => {},
            }
        };
        // Settlement receipts may now depend on original compiled finally or
        // bounded provider drains. Keep the SAME driver advancing while joining;
        // awaiting settle first and only then drain_ready would deadlock.
        let settlement_lease = self.lease.clone();
        let settlement = settlement_lease.settle();
        tokio::pin!(settlement);
        // Execute the existing seal/prepare_repair prefix before allowing
        // any final Graph reaction, preserving the old admission cutoff.
        let mut joined = match futures::poll!(&mut settlement) {
            std::task::Poll::Ready(result) => { if let Err(error) = result { remember_failure(&mut failure, error); } true },
            std::task::Poll::Pending => false,
        };
        loop {
            let wake = self.promise.wake();
            let registry_wake = self.lease.registry_wake.clone();
            let promise_notice = wake.notified();
            let registry_notice = registry_wake.notified();
            tokio::pin!(promise_notice, registry_notice);
            promise_notice.as_mut().enable(); registry_notice.as_mut().enable();
            for drain in self.lease.take_provider_drains() { self.provider_drains.push(drain); }
            let mut host_notices: Vec<_> = self.lease.effect_owners().into_iter().map(|owner| Box::pin(owner.wake().notified_owned())).collect();
            for notice in &mut host_notices { notice.as_mut().enable(); }
            let host_notice = futures::future::select_all(host_notices);
            tokio::pin!(host_notice);
            let mut more = false;
            if !revoked {
                match self.drain_burst() {
                    Ok(ready) => more = ready,
                    Err(error) => { remember_failure(&mut failure, error); self.lease.revoke(); revoked = true; }
                }
            }
            if revoked && !self.provider_drains.is_empty() {
                // Dropping unexecutable Graph work is unknown, not a receipt of
                // successful transport drain. Independent Host permit cleanup
                // still runs without a runtime.
                self.provider_drains.clear();
                remember_failure(&mut failure, ActualProviderError::Callback("Original provider drain revoked before authoritative settlement".into()));
            }
            if joined && self.provider_drains.is_empty() && self.lease.registry.lock().unwrap_or_else(|e| e.into_inner()).provider_drains.is_empty() { break; }
            tokio::select! {
                result = &mut settlement, if !joined => { joined = true; if let Err(error) = result { remember_failure(&mut failure, error); } },
                Some(result) = self.provider_drains.next(), if !self.provider_drains.is_empty() && !revoked => { if let Err(error) = result { remember_failure(&mut failure, error.into()); } },
                _ = &mut promise_notice, if !revoked => {},
                _ = &mut registry_notice => {},
                _ = &mut host_notice, if !revoked => {},
                _ = tokio::task::yield_now(), if more => {},
            }
        }
        // Every notice from joined workers is now admitted. Drain final reactions,
        // then revoke isolated pending promises instead of waiting indefinitely.
        if !revoked {
            if let Err(error) = self.drain_ready().await {
                remember_failure(&mut failure, error);
            }
        }
        if let Some(display) = &display {
            let mut display = display.lock().await;
            let cutoff = display.port.cutoff();
            display.finish_through(cutoff);
        }
        self.lease.revoke_graph();
        self.finished = true;
        match failure {
            Some(error) => Err(error),
            None => Ok(outcome),
        }
    }
}
impl Drop for NativeRun {
    fn drop(&mut self) {
        if !self.finished {
            self.lease.revoke();
        }
    }
}
