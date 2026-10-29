//! Final prepared-request gate (OMP `agent.ts` / `types.ts` at
//! 596f2da7101178214aa27a753529d15e6b7ad91d; MIT, see THIRD_PARTY_NOTICES.md).

use ara_ai::Context;
use async_trait::async_trait;
use futures::FutureExt;
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::{Arc, Mutex, Weak};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FinalProviderGateStop {
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalProviderGateError(pub String);

impl std::fmt::Display for FinalProviderGateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for FinalProviderGateError {}

impl FinalProviderGateError {
    pub(crate) fn panicked(operation: &str, payload: Box<dyn std::any::Any + Send>) -> Self {
        let detail = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or("non-string panic");
        Self(format!("{operation} panicked: {detail}"))
    }
}

/// Edits the exact Context subsequently sent to the captured provider route.
/// `None` proceeds; a stop refuses this send. Awaiting is cooperative with the
/// Run token, as in Native; the Core adds no arbitrary callback timeout.
#[async_trait]
pub trait FinalProviderGate: Send + Sync {
    async fn before_model_call(
        &self,
        context: &mut Context,
        cancel: &CancellationToken,
    ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError>;
}

#[derive(Default)]
struct Registry {
    primary: Option<Arc<dyn FinalProviderGate>>,
    next_id: u64,
    additional: BTreeMap<u64, Arc<dyn FinalProviderGate>>,
}

/// One shared owner for a Host binding and a stateful Agent's gate facade.
/// Callback presence is sampled once per Run; the enabled registry stays live.
#[derive(Clone, Default)]
pub struct FinalProviderGateBinding {
    registry: Arc<Mutex<Registry>>,
}

impl FinalProviderGateBinding {
    pub fn set_primary(&self, primary: Option<Arc<dyn FinalProviderGate>>) {
        self.registry.lock().unwrap().primary = primary;
    }

    /// Native additional callbacks form a Set by callback object identity.
    /// Adding the same Arc twice is idempotent, independent of the primary.
    pub fn add(&self, callback: Arc<dyn FinalProviderGate>) -> GateRegistration {
        let mut registry = self.registry.lock().unwrap();
        if !registry.additional.values().any(|registered| Arc::ptr_eq(registered, &callback)) {
            registry.next_id = registry.next_id.checked_add(1).expect("final gate registration ID exhausted");
            let id = registry.next_id;
            registry.additional.insert(id, callback.clone());
        }
        GateRegistration { registry: Arc::downgrade(&self.registry), callback }
    }

    /// Call before the Run's first await. An empty owner samples absent; an
    /// enabled sample remains enabled even when its callbacks later disappear.
    pub fn sample_for_run(&self) -> Option<Self> {
        let registry = self.registry.lock().unwrap();
        (registry.primary.is_some() || !registry.additional.is_empty()).then(|| self.clone())
    }

    pub(crate) async fn invoke(
        &self,
        context: &mut Context,
        cancel: &CancellationToken,
    ) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
        let primary = self.registry.lock().unwrap().primary.clone();
        if let Some(primary) = primary
            && let Some(stop) = invoke_callback(primary.as_ref(), context, cancel).await?
        {
            return Ok(Some(stop));
        }
        let mut cursor = 0;
        loop {
            let next = self
                .registry
                .lock()
                .unwrap()
                .additional
                .range((Excluded(cursor), Unbounded))
                .next()
                .map(|(id, callback)| (*id, callback.clone()));
            let Some((id, callback)) = next else { return Ok(None) };
            cursor = id;
            if let Some(stop) = invoke_callback(callback.as_ref(), context, cancel).await? {
                return Ok(Some(stop));
            }
        }
    }
}

async fn invoke_callback(
    callback: &dyn FinalProviderGate,
    context: &mut Context,
    cancel: &CancellationToken,
) -> Result<Option<FinalProviderGateStop>, FinalProviderGateError> {
    // Construct the callback future inside the caught future, so a panic in
    // either construction or polling follows the same new-gate error boundary.
    match std::panic::AssertUnwindSafe(async { callback.before_model_call(context, cancel).await }).catch_unwind().await
    {
        Ok(result) => result,
        Err(payload) => Err(FinalProviderGateError::panicked("Final provider gate", payload)),
    }
}

/// Explicit Native disposer. Drop does not remove the callback. Identity, not
/// insertion ID, means an old disposer also deletes a later readd of this Arc.
pub struct GateRegistration {
    registry: Weak<Mutex<Registry>>,
    callback: Arc<dyn FinalProviderGate>,
}

impl GateRegistration {
    pub fn remove(&self) -> bool {
        let Some(registry) = self.registry.upgrade() else { return false };
        let mut registry = registry.lock().unwrap();
        let id =
            registry.additional.iter().find_map(|(id, callback)| Arc::ptr_eq(callback, &self.callback).then_some(*id));
        id.is_some_and(|id| registry.additional.remove(&id).is_some())
    }
}
