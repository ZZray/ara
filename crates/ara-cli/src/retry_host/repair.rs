//! Narrow Edit side-completion resource joins independent of revoked graph work.
//! These records contain original receipts/native cancellation and captured
//! pure Rust writes only. Settlement never polls a graph future or Get/Call.
use crate::actual_tools::async_effect::HostEffectId;
use ara_agent::ActualProviderError;
use ara_ai::ActualStreamFailureReceipt;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
type UsageWrite = Shared<BoxFuture<'static, Result<(), Arc<str>>>>;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RepairAttemptKey {
    effect: HostEffectId,
    serial: u64,
}
struct AttemptResources {
    receipt: Option<ActualStreamFailureReceipt>,
    cancel: CancellationToken,
    registration_pending: bool,
    observed: bool,
    unknown: Option<&'static str>,
    usage: HashMap<u64, UsageWrite>,
}
struct State {
    sealed: bool,
    graph_closed: bool,
    next_serial: u64,
    attempts: HashMap<RepairAttemptKey, AttemptResources>,
    auxiliary: Vec<ActualStreamFailureReceipt>,
    caught: Vec<ara_rpc::value_heap::ValueError>,
}
pub(crate) struct RepairResourcesOwner {
    state: Mutex<State>,
    changed: Notify,
}
impl RepairResourcesOwner {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                sealed: false,
                graph_closed: false,
                next_serial: 1,
                attempts: HashMap::new(),
                auxiliary: Vec::new(),
                caught: Vec::new(),
            }),
            changed: Notify::new(),
        })
    }
    /// Original Edit catches settled completion failures and returns its parse
    /// warning. Retain the original error without promoting it to Run failure.
    pub fn retain_caught(&self, error: ara_rpc::value_heap::ValueError) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .caught
            .push(error);
    }
    pub fn retain_auxiliary(
        &self,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<(), ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        if state.sealed || state.graph_closed {
            return Err(ActualProviderError::Protocol(
                "Repair resource admission closed",
            ));
        }
        state.auxiliary.push(receipt);
        Ok(())
    }
    /// Admit before calling stream_actual. A started dispatch may register its
    /// returned receipt even after seal/revoke; no late producer is discarded.
    pub fn begin_attempt(
        &self,
        effect: HostEffectId,
        cancel: CancellationToken,
    ) -> Result<RepairAttemptKey, ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        if state.sealed || state.graph_closed {
            return Err(ActualProviderError::Protocol(
                "Repair attempt admission closed",
            ));
        }
        let serial = state.next_serial;
        state.next_serial = serial.checked_add(1).ok_or(ActualProviderError::Protocol(
            "Repair attempt identities exhausted",
        ))?;
        let key = RepairAttemptKey { effect, serial };
        state.attempts.insert(
            key,
            AttemptResources {
                receipt: None,
                cancel,
                registration_pending: true,
                observed: false,
                unknown: None,
                usage: HashMap::new(),
            },
        );
        Ok(key)
    }
    pub fn register_started(
        &self,
        key: RepairAttemptKey,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<(), ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        let graph_closed = state.graph_closed;
        let attempt = state
            .attempts
            .get_mut(&key)
            .ok_or(ActualProviderError::Protocol(
                "Repair producer has no admitted attempt",
            ))?;
        if !attempt.registration_pending {
            return Err(ActualProviderError::Protocol(
                "Repair producer registered twice",
            ));
        }
        attempt.receipt = Some(receipt);
        attempt.registration_pending = false;
        if graph_closed {
            attempt.cancel.cancel();
            attempt.unknown = Some("graph revoked before original usage extraction")
        }
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }
    /// Only the prepared production adapter may prove its synchronous error
    /// happened before spawning a producer. Arbitrary foreign factories cannot.
    pub fn no_request(&self, key: RepairAttemptKey) -> Result<(), ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        let attempt = state
            .attempts
            .get_mut(&key)
            .ok_or(ActualProviderError::Protocol(
                "Repair failure has no admitted attempt",
            ))?;
        if !attempt.registration_pending {
            return Err(ActualProviderError::Protocol(
                "Started repair cannot become no-request",
            ));
        }
        attempt.registration_pending = false;
        attempt.observed = true;
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }
    /// An arbitrary failed dispatch cannot prove that no request started.
    /// Complete registration explicitly and preserve that unresolved boundary.
    pub fn dispatch_uncertain(&self, key: RepairAttemptKey) -> Result<(), ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        let attempt = state
            .attempts
            .get_mut(&key)
            .ok_or(ActualProviderError::Protocol(
                "Repair failure has no admitted attempt",
            ))?;
        if !attempt.registration_pending {
            return Err(ActualProviderError::Protocol(
                "Repair dispatch already registered",
            ));
        }
        attempt.registration_pending = false;
        attempt.unknown = Some("dispatch failed without an authoritative no-request receipt");
        drop(state);
        self.changed.notify_waiters();
        Ok(())
    }
    /// Invoke the supplied sink only once for the original observation key and
    /// retain the very same Shared future returned to the live graph caller.
    /// That caller awaits it before a String result or subsequent attempt.
    pub fn register_usage(
        &self,
        key: RepairAttemptKey,
        observation: u64,
        write: impl FnOnce() -> BoxFuture<'static, Result<(), String>>,
    ) -> Result<UsageWrite, ActualProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| ActualProviderError::Protocol("Repair resources owner poisoned"))?;
        let attempt = state
            .attempts
            .get_mut(&key)
            .ok_or(ActualProviderError::Protocol(
                "Repair usage has no admitted attempt",
            ))?;
        if attempt.registration_pending || attempt.receipt.is_none() {
            return Err(ActualProviderError::Protocol(
                "Repair usage preceded original producer receipt",
            ));
        }
        if let Some(existing) = attempt.usage.get(&observation) {
            return Ok(existing.clone());
        }
        // The closure creates only a captured pure-Rust persistence future;
        // no polling or user code runs under this resource bookkeeping lock.
        let future = write()
            .map(|outcome| outcome.map_err(Arc::<str>::from))
            .boxed()
            .shared();
        attempt.usage.insert(observation, future.clone());
        attempt.observed = true;
        attempt.unknown = None;
        Ok(future)
    }
    pub fn seal(&self) {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sealed = true;
        self.changed.notify_waiters();
    }
    pub fn revoke_graph(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.sealed = true;
        state.graph_closed = true;
        for attempt in state.attempts.values_mut() {
            attempt.cancel.cancel();
            if attempt.receipt.is_some() && !attempt.observed {
                attempt.unknown = Some("graph revoked before original usage extraction")
            }
        }
        drop(state);
        self.changed.notify_waiters();
    }
    pub async fn settle(&self) -> Result<(), ActualProviderError> {
        self.seal();
        let (receipts, writes, unknown) = loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let snapshot = {
                let state = self.state.lock().map_err(|_| {
                    ActualProviderError::Protocol("Repair resources owner poisoned")
                })?;
                if state
                    .attempts
                    .values()
                    .any(|attempt| attempt.registration_pending)
                {
                    None
                } else {
                    Some((
                        state
                            .attempts
                            .values()
                            .filter_map(|attempt| attempt.receipt.clone())
                            .chain(state.auxiliary.iter().cloned())
                            .collect::<Vec<_>>(),
                        state
                            .attempts
                            .values()
                            .flat_map(|attempt| attempt.usage.values().cloned())
                            .collect::<Vec<_>>(),
                        state
                            .attempts
                            .iter()
                            .filter_map(|(key, attempt)| {
                                attempt.unknown.map(|reason| format!("{key:?}: {reason}"))
                            })
                            .collect::<Vec<_>>(),
                    ))
                }
            };
            if let Some(snapshot) = snapshot {
                break snapshot;
            }
            changed.await;
        };
        let (mut failures, write_results) = tokio::join!(
            futures::future::join_all(receipts.iter().map(|receipt| receipt.wait_settled())),
            futures::future::join_all(writes)
        );
        let mut failure = None;
        for error in failures.drain(..).flatten() {
            remember(
                &mut failure,
                ActualProviderError::Value(error.as_ref().clone()),
            );
        }
        for write in write_results {
            if let Err(error) = write {
                remember(
                    &mut failure,
                    ActualProviderError::Callback(format!(
                        "Repair usage persistence failed: {error}"
                    )),
                );
            }
        }
        if !unknown.is_empty() {
            remember(
                &mut failure,
                ActualProviderError::Callback(format!(
                    "Repair started requests have unknown usage: {}",
                    unknown.join("; ")
                )),
            );
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
fn remember(owner: &mut Option<ActualProviderError>, next: ActualProviderError) {
    *owner = Some(match owner.take() {
        Some(primary) => ActualProviderError::Compound {
            primary: Arc::new(primary),
            secondary: Arc::new(next),
        },
        None => next,
    });
}

pub(crate) struct RepairEffectResources {
    owner: Arc<RepairResourcesOwner>,
    effect: HostEffectId,
}
impl RepairEffectResources {
    pub fn prepare(owner: Arc<RepairResourcesOwner>, effect: HostEffectId) -> Arc<Self> {
        Arc::new(Self { owner, effect })
    }
    fn key(&self, serial: u64) -> RepairAttemptKey {
        RepairAttemptKey {
            effect: self.effect,
            serial,
        }
    }
}
fn resource_error(error: ActualProviderError) -> ara_rpc::value_heap::ValueError {
    match error {
        ActualProviderError::Value(error) => error,
        _ => ara_rpc::value_heap::ValueError::UnsupportedOperation(
            "Original Edit repair resource admission failed",
        ),
    }
}
impl ara_tools::edit_post_apply::EditRepairResources for RepairEffectResources {
    fn begin_attempt(
        &self,
        cancel: CancellationToken,
    ) -> Result<u64, ara_rpc::value_heap::ValueError> {
        self.owner
            .begin_attempt(self.effect, cancel)
            .map(|key| key.serial)
            .map_err(resource_error)
    }
    fn register_started(
        &self,
        key: u64,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<(), ara_rpc::value_heap::ValueError> {
        self.owner
            .register_started(self.key(key), receipt)
            .map_err(resource_error)
    }
    fn no_request(&self, key: u64) -> Result<(), ara_rpc::value_heap::ValueError> {
        self.owner.no_request(self.key(key)).map_err(resource_error)
    }
    fn dispatch_uncertain(&self, key: u64) -> Result<(), ara_rpc::value_heap::ValueError> {
        self.owner
            .dispatch_uncertain(self.key(key))
            .map_err(resource_error)
    }
    fn register_usage(
        &self,
        key: u64,
        observation: u64,
        write: BoxFuture<'static, Result<(), String>>,
    ) -> Result<BoxFuture<'static, Result<(), String>>, ara_rpc::value_heap::ValueError> {
        let shared = self
            .owner
            .register_usage(self.key(key), observation, || write)
            .map_err(resource_error)?;
        Ok(Box::pin(async move {
            shared.await.map_err(|error| error.to_string())
        }))
    }
}
