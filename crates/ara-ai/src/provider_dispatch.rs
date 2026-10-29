//! Host-neutral provider admission contracts from OMP b91db66e stream.ts
//! (MIT). Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md.
//!
//! Hosts own coordination roots, processes, leases and dispatch. A permit is
//! not a Run receipt, an OS sandbox, or evidence of transport settlement.
use ara_rpc::WireString;
use futures::future::BoxFuture;
use std::{collections::HashMap, sync::Arc};
use tokio_util::sync::CancellationToken;

pub type ProviderLimits = HashMap<WireString, f64>;

/// A supplied per-call map replaces the configured map, including an empty
/// map. Invalid/absent entries are unlimited, not a fallback to defaults.
pub fn effective_provider_limit(
    provider: &WireString,
    per_call: Option<&ProviderLimits>,
    configured: &ProviderLimits,
) -> Option<usize> {
    let value = *per_call.unwrap_or(configured).get(provider)?;
    (value.is_finite() && value > 0.0).then(|| value.floor().max(1.0) as usize)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderCoordinationError {
    Cancelled,
    Failed(String),
}
impl std::fmt::Display for ProviderCoordinationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("provider admission cancelled"),
            Self::Failed(message) => f.write_str(message),
        }
    }
}
impl std::error::Error for ProviderCoordinationError {}

/// Admission precedes the physical dispatch. Original native signal reasons
/// stay in the calling Host, which rechecks them after successful acquisition.
pub trait ProviderInFlightPort: Send + Sync {
    fn acquire(
        &self,
        provider: WireString,
        limit: usize,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<Arc<dyn ProviderPermit>, ProviderCoordinationError>>;
}

/// Implementations memoize one bounded release operation for every caller.
/// The capped dispatcher awaits this after original result/drain and before
/// publishing its terminal event. Cleanup failure preserves provider outcome.
pub trait ProviderPermit: Send + Sync {
    fn release_once(&self) -> BoxFuture<'static, Result<(), ProviderCoordinationError>>;
}
