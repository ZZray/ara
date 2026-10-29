//! Actual-value retry imports for fixed OMP596f2da (MIT).
//! The Host owns current Run admission, signals, scheduling and Promise jobs.
//! No import creates a realm, substitutes typed responses, or drains jobs.

use crate::{ActualStreamFailureReceipt, graph_values::GraphValueFactory};
use ara_rpc::value_heap::{PropertyKey, ValueError, ValueHandle, ValueRuntime};
use futures::future::BoxFuture;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub mod abort;
pub mod hints;
pub mod idle;
pub mod native_retry_hint;
pub mod oneshot;
pub mod result;
pub mod sdk_timeout;
pub mod stream;

type Result<T> = std::result::Result<T, ValueError>;

/// One immutable issuing Run. The synchronous borrowing operation invokes its
/// closure exactly once and drops the scoped runtime before returning. Neither
/// the closure nor the original runtime can be put in an OS completion worker.
pub trait RetryBindings: Send + Sync {
    fn check_invocation(&self, factory: &Arc<GraphValueFactory>) -> Result<()>;
    fn with_runtime(
        &self,
        factory: &Arc<GraphValueFactory>,
        operation: &mut dyn FnMut(&dyn ValueRuntime) -> Result<ValueHandle>,
    ) -> Result<ValueHandle>;
    /// Adopts synchronously before returning a ticket-only wait future. It
    /// retains the original fulfillment or rejection in the existing owner.
    fn await_value(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, Result<ValueHandle>>;
    /// Separates authoritative original work settlement from an expired Host
    /// awaiter. Local-work accounting cannot infer settlement from any error.
    fn await_observed(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, NativeWorkObservation>;
    /// Install an original-Run compiled finally and its dedicated derived
    /// Promise before returning. Workers only observe the wrapper ticket.
    fn adopt_local_work(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
        ticket: crate::event::LocalWorkTicket,
    ) -> Result<BoxFuture<'static, NativeWorkObservation>>;
    /// Existing Run revocation wake only. This observes original admission and
    /// never evaluates graph code or drains Promise/Host queues.
    fn invocation_ended(&self) -> BoxFuture<'static, ValueError>;
    fn async_iterator_key(&self) -> PropertyKey;
    fn globals(&self) -> ValueHandle;
    fn error_constructor(&self) -> ValueHandle;
    /// Node validation uses the captured intrinsic, independent of writable
    /// globalThis.TypeError. Hosts without that import fail explicitly.
    fn type_error_constructor(&self) -> Result<ValueHandle> {
        Err(ValueError::UnsupportedOperation(
            "Original TypeError constructor is not imported",
        ))
    }
    fn provider_response_error_constructor(&self) -> ValueHandle;
    fn signals(&self) -> Arc<dyn AbortSignalImports>;
    fn timers(&self) -> Arc<dyn TimerImports>;
    fn receipt(&self) -> ActualStreamFailureReceipt;
    fn environment(&self, name: &str) -> Option<String>;
}

pub struct NativeWorkObservation {
    pub result: Result<ValueHandle>,
    pub settled: bool,
}

#[derive(Clone)]
pub struct AbortControllerRef {
    pub controller: ValueHandle,
    pub signal: ValueHandle,
}

/// A same-realm branded Host domain. Core observes the original signal Gets
/// and listener methods; token lookup does not manufacture a reason value.
pub trait AbortSignalImports: Send + Sync {
    fn controller(
        &self,
        factory: &Arc<GraphValueFactory>,
        runtime: &dyn ValueRuntime,
    ) -> Result<AbortControllerRef>;
    fn token(
        &self,
        factory: &Arc<GraphValueFactory>,
        signal: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<CancellationToken>;
    fn abort(
        &self,
        factory: &Arc<GraphValueFactory>,
        controller: &AbortControllerRef,
        reason: Option<ValueHandle>,
        runtime: &dyn ValueRuntime,
    ) -> Result<()>;
    fn any(
        &self,
        factory: &Arc<GraphValueFactory>,
        signals: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle>;
    fn callback(
        &self,
        factory: &Arc<GraphValueFactory>,
        callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle>;
    fn error(
        &self,
        factory: &Arc<GraphValueFactory>,
        name: &str,
        message: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle>;
    fn error_value(
        &self,
        factory: &Arc<GraphValueFactory>,
        name: &str,
        message: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle>;
}

/// Source timers clear explicitly. Dropping a lease does not cancel a timer
/// whose callback is still owned by the current operation.
#[derive(Clone)]
pub struct TimerLease {
    clear: Arc<dyn Fn() -> Result<()> + Send + Sync>,
}
impl TimerLease {
    pub fn new(clear: Arc<dyn Fn() -> Result<()> + Send + Sync>) -> Self {
        Self { clear }
    }
    pub fn clear(&self) -> Result<()> {
        (self.clear)()
    }
}

/// Timer callbacks execute under the existing issuing Promise owner drain.
/// Workers publish only a branded native undefined tick, never graph Calls.
pub trait TimerImports: Send + Sync {
    fn arm(
        &self,
        delay_ms: f64,
        callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
    ) -> Result<TimerLease>;
    fn wait(&self, delay_ms: f64) -> BoxFuture<'static, Result<()>>;
}

/// Borrowing adapter for Rust translations of native synchronous expressions.
/// It cannot return or retain the temporary runtime; an import that calls the
/// supplied operation twice is an explicit integration failure.
pub(crate) fn execute<T>(
    factory: &Arc<GraphValueFactory>,
    bindings: &dyn RetryBindings,
    operation: impl FnOnce(&dyn ValueRuntime) -> Result<T>,
) -> Result<T> {
    let mut operation = Some(operation);
    let mut output = None;
    bindings.with_runtime(factory, &mut |runtime| {
        let call = operation.take().ok_or(ValueError::UnsupportedOperation(
            "Retry runtime import invoked one operation more than once",
        ))?;
        output = Some(call(runtime)?);
        factory.heap().undefined()
    })?;
    output.ok_or(ValueError::UnsupportedOperation(
        "Retry runtime import did not invoke its admitted operation",
    ))
}
