//! Fixed OMP b91db66e child runtime contract (MIT; THIRD_PARTY_NOTICES.md).
//! Copyright (c) 2025 Mario Zechner; (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.

pub mod prompt;
pub mod result;
pub mod schema;
#[cfg(any(test, feature = "test-fixture"))]
mod tests;
#[cfg(feature = "test-fixture")]
#[doc(hidden)]
pub use tests::{
    child_schema_yield_bank, child_pool_and_original_graph_bank,
    child_schema_borrowed_resolver_bank, child_publication_dispatch_bank,
};
pub mod yield_tool;

use ara_agent::{ActualProviderError, SessionValues};
use ara_rpc::value_heap::ValueHandle;
pub use result::{ChildResultStatus, ChildRunFinish, ChildRunObserver, ObservedChildResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{Mutex as AsyncMutex, Notify};
pub use yield_tool::{NativeYieldBundle, YieldReceipt, YieldStatus};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPoolYieldItem {
    pub id: String,
    pub index: usize,
}
#[derive(Clone, Debug)]
pub struct ChildContractConfig {
    pub system_prompt: Vec<String>,
    pub output_schema: Option<Value>,
    pub output_schema_mode: String,
    pub structured: bool,
    pub work_pool_items: Vec<WorkPoolYieldItem>,
}
#[derive(Clone, Debug)]
pub struct ContractSnapshot {
    pub generation: u64,
    pub items: Vec<WorkPoolYieldItem>,
    pub system_prompt: Vec<String>,
}
#[derive(Clone, Debug)]
pub struct InstallTicket {
    pub generation: u64,
    unchanged: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChildContractError(pub String);
impl std::fmt::Display for ChildContractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ChildContractError {}
impl From<ara_rpc::value_heap::ValueError> for ChildContractError {
    fn from(e: ara_rpc::value_heap::ValueError) -> Self {
        Self(e.to_string())
    }
}
impl From<ChildContractError> for ActualProviderError {
    fn from(e: ChildContractError) -> Self {
        Self::Callback(e.0)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchKind {
    Ordinary,
    Pooled,
}
struct State {
    desired: ContractSnapshot,
    published: ContractSnapshot,
    pending: BTreeSet<u64>,
    active: Option<u64>,
    next_dispatch: u64,
    failed_publication: Option<String>,
    initialized: bool,
}
/// One logical Native Session. Desired items reserve dispatch synchronously;
/// only the serialized native publisher changes the successful prompt/schema pair.
pub struct ChildContractOwner {
    values: Arc<SessionValues>,
    config: ChildContractConfig,
    pub(crate) graph: ValueHandle,
    state: Mutex<State>,
    transition: AsyncMutex<()>,
    wake: Notify,
}
impl ChildContractOwner {
    pub fn new(
        values: Arc<SessionValues>,
        config: ChildContractConfig,
    ) -> Result<Arc<Self>, ActualProviderError> {
        if !matches!(config.output_schema_mode.as_str(), "strict" | "permissive") {
            return Err(ActualProviderError::Protocol(
                "Unknown child outputSchemaMode",
            ));
        }
        check_items(&config.work_pool_items)?;
        let graph = yield_tool::initial_state(&values, &config)?;
        let snapshot = ContractSnapshot {
            generation: 0,
            items: config.work_pool_items.clone(),
            system_prompt: config.system_prompt.clone(),
        };
        Ok(Arc::new(Self {
            values,
            config,
            graph,
            state: Mutex::new(State {
                desired: snapshot.clone(),
                published: snapshot,
                pending: BTreeSet::new(),
                active: None,
                next_dispatch: 0,
                failed_publication: None,
                initialized: false,
            }),
            transition: AsyncMutex::new(()),
            wake: Notify::new(),
        }))
    }
    pub fn values(&self) -> &Arc<SessionValues> {
        &self.values
    }
    pub fn config(&self) -> &ChildContractConfig {
        &self.config
    }
    pub fn initialize_published_prompt(
        &self,
        prompt: Vec<String>,
    ) -> Result<(), ChildContractError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.initialized || s.desired.generation != 0 || !s.pending.is_empty() || s.active.is_some()
        {
            return Err(ChildContractError(
                "Child published prompt initialization is no longer available".into(),
            ));
        }
        s.published.system_prompt = prompt.clone();
        s.desired.system_prompt = prompt;
        s.initialized = true;
        Ok(())
    }
    pub fn published_snapshot(&self) -> ContractSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .published
            .clone()
    }
    pub fn desired_snapshot(&self) -> ContractSnapshot {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .desired
            .clone()
    }
    pub fn is_current(&self, ticket: &InstallTicket) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .desired
            .generation
            == ticket.generation
    }
    pub fn with_current_publication<T>(
        &self,
        ticket: &InstallTicket,
        commit: impl FnOnce() -> Result<T, ChildContractError>,
    ) -> Result<T, ChildContractError> {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.desired.generation != ticket.generation {
            return Err(ChildContractError(
                "Superseded child publication commit".into(),
            ));
        }
        // The closure is synchronous; it must not re-enter this owner.
        commit()
    }
    pub fn reserve_items(
        &self,
        items: Vec<WorkPoolYieldItem>,
    ) -> Result<InstallTicket, ChildContractError> {
        check_items(&items).map_err(|e| ChildContractError(e.to_string()))?;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.active.is_some() {
            return Err(ChildContractError(
                "Cannot install child output contract under an active turn".into(),
            ));
        }
        if s.desired.items == items {
            return Ok(InstallTicket {
                generation: s.desired.generation,
                unchanged: true,
            });
        }
        let generation = s
            .desired
            .generation
            .checked_add(1)
            .ok_or_else(|| ChildContractError("Child contract generation exhausted".into()))?;
        yield_tool::set_items(&self.values, &self.graph, &items)?;
        s.desired = ContractSnapshot {
            generation,
            items,
            system_prompt: s.published.system_prompt.clone(),
        };
        s.pending.insert(generation);
        Ok(InstallTicket {
            generation,
            unchanged: false,
        })
    }
    /// apply publishes an actual same-Session Native tool/prompt pair, checking
    /// is_current immediately before its commit. On failure it must also accept
    /// the last successful snapshot to repair any partial external publication.
    /// Dropping an admitted publication leaves pending ownership blocked until
    /// the Host reconciles it; cancellation is never reported as settled success.
    pub async fn publish<F, Fut>(
        &self,
        ticket: InstallTicket,
        mut apply: F,
    ) -> Result<ContractSnapshot, ChildContractError>
    where
        F: FnMut(ContractSnapshot) -> Fut,
        Fut: std::future::Future<Output = Result<Vec<String>, ChildContractError>>,
    {
        if ticket.unchanged {
            self.when_settled().await;
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = &s.failed_publication {
                return Err(ChildContractError(e.clone()));
            }
            return Ok(s.published.clone());
        }
        let _serial = self.transition.lock().await;
        let (desired, previous) = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.desired.generation != ticket.generation {
                s.pending.remove(&ticket.generation);
                drop(s);
                self.wake.notify_waiters();
                return Err(ChildContractError(
                    "Superseded child contract publication".into(),
                ));
            }
            (s.desired.clone(), s.published.clone())
        };
        match apply(desired.clone()).await {
            Ok(prompt) => {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                if s.desired.generation != ticket.generation {
                    s.published = ContractSnapshot {
                        system_prompt: prompt,
                        ..desired
                    };
                    s.pending.remove(&ticket.generation);
                    drop(s);
                    self.wake.notify_waiters();
                    return Err(ChildContractError(
                        "Child contract replaced during publication".into(),
                    ));
                }
                s.published = ContractSnapshot {
                    system_prompt: prompt,
                    ..desired
                };
                s.desired = s.published.clone();
                s.pending.remove(&ticket.generation);
                s.failed_publication = None;
                let published = s.published.clone();
                drop(s);
                self.wake.notify_waiters();
                Ok(published)
            }
            Err(error) => {
                let repair = {
                    let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if s.desired.generation == ticket.generation {
                        yield_tool::set_items(&self.values, &self.graph, &previous.items)?;
                        true
                    } else {
                        false
                    }
                };
                if repair {
                    let repaired = apply(previous.clone()).await;
                    let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                    if s.desired.generation == ticket.generation {
                        if let Ok(prompt) = repaired {
                            s.desired = ContractSnapshot {
                                generation: ticket.generation,
                                items: previous.items,
                                system_prompt: prompt.clone(),
                            };
                            s.published = s.desired.clone();
                            s.failed_publication = None;
                        } else {
                            s.failed_publication = Some(
                                "Child contract rollback failed; dispatch remains blocked".into(),
                            );
                        }
                    }
                    s.pending.remove(&ticket.generation);
                } else {
                    self.state
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .pending
                        .remove(&ticket.generation);
                }
                self.wake.notify_waiters();
                Err(error)
            }
        }
    }
    pub async fn when_settled(&self) {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pending
                .is_empty()
            {
                return;
            }
            notified.await;
        }
    }
    pub async fn wait_for_idle(&self) {
        loop {
            let notified = self.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .active
                .is_none()
            {
                return;
            }
            notified.await;
        }
    }
    /// Call at the actual actor's await-free admission boundary, before starting
    /// the Native run. The returned lease lives across that exact run/observer.
    pub fn try_claim_dispatch(
        self: &Arc<Self>,
        kind: DispatchKind,
        native_busy: bool,
    ) -> Result<DispatchLease, ChildContractError> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if native_busy
            || s.active.is_some()
            || !s.pending.is_empty()
            || s.failed_publication.is_some()
        {
            return Err(ChildContractError(
                "Child Native dispatch is busy or contract is unsettled".into(),
            ));
        }
        if kind == DispatchKind::Ordinary && !s.desired.items.is_empty() {
            return Err(ChildContractError(
                "Ordinary wake deferred while pooled turn owns child".into(),
            ));
        }
        if kind == DispatchKind::Pooled && s.desired.items.is_empty() {
            return Err(ChildContractError(
                "Pooled dispatch has no installed items".into(),
            ));
        }
        s.next_dispatch = s
            .next_dispatch
            .checked_add(1)
            .ok_or_else(|| ChildContractError("Child dispatch generation exhausted".into()))?;
        let id = s.next_dispatch;
        s.active = Some(id);
        Ok(DispatchLease {
            owner: Arc::downgrade(self),
            id,
            pub_generation: s.published.generation,
        })
    }
    pub fn dispatch_active(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .active
            .is_some()
    }
    pub fn publication_error(&self) -> Option<String> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .failed_publication
            .clone()
    }
    /// Register before checking publication state; notify_waiters stores no permit.
    pub fn wake(&self) -> &Notify {
        &self.wake
    }
    pub fn notify_deferred_wakes(&self) {
        self.wake.notify_waiters();
    }
    pub async fn changed(&self) {
        self.wake.notified().await;
    }
}
fn check_items(items: &[WorkPoolYieldItem]) -> Result<(), ActualProviderError> {
    let mut ids = BTreeSet::new();
    let mut indices = BTreeSet::new();
    for item in items {
        if item.id.is_empty() || !ids.insert(&item.id) || !indices.insert(item.index) {
            return Err(ActualProviderError::Protocol(
                "Workpool items require unique nonempty ids and unique indices",
            ));
        }
    }
    Ok(())
}
pub struct DispatchLease {
    owner: Weak<ChildContractOwner>,
    id: u64,
    pub_generation: u64,
}
impl DispatchLease {
    pub fn generation(&self) -> u64 {
        self.pub_generation
    }
    pub fn id(&self) -> u64 {
        self.id
    }
}
impl Drop for DispatchLease {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.upgrade() {
            let mut s = owner.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.active == Some(self.id) {
                s.active = None;
                drop(s);
                owner.wake.notify_waiters();
            }
        }
    }
}

impl Drop for ChildContractOwner {
    fn drop(&mut self) {
        if let Some(schema) = self.config.output_schema.take() {
            schema::drop_declaration(schema);
        }
    }
}
