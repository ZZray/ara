//! Reference Host imports for fixed OMP596f2da retry utilities (MIT).
//! Frozen Run authority, same Session DOM signal brand, and the original Promise
//! driver are explicit imports. No product state or retry policy belongs here.
pub(crate) mod errors;
mod imports;
pub mod lazy_provider;
mod oneshot_math;
pub(crate) mod repair;
pub mod signal;
mod timer;
use crate::{mock_provider_host::NativeCallingRun, native_realm::NativeRealm};
use ara_ai::{
    ActualStreamFailureReceipt,
    graph_values::GraphValueFactory,
    retry::{AbortSignalImports, NativeWorkObservation, RetryBindings, TimerImports},
};
use ara_rpc::value_heap::{PropertyKey, ValueError, ValueHandle, ValueRuntime};
use futures::future::BoxFuture;
use signal::NativeSignalDomain;
use std::sync::Arc;
use timer::NativeTimerDomain;
type Result<T> = std::result::Result<T, ValueError>;

pub struct NativeRetryBindings {
    calling: NativeCallingRun,
    signals: Arc<NativeSignalDomain>,
    timers: Arc<NativeTimerDomain>,
    globals: ValueHandle,
    error: ValueHandle,
    type_error: ValueHandle,
    async_iterator: PropertyKey,
    receipt: ActualStreamFailureReceipt,
}
/// Prepare the same-realm Math/Headers dependencies used by terminal retry hints.
/// The Session signal owner supplies the original Headers constructor identity.
pub fn prepare_hint_dependencies(realm: &NativeRealm) -> Result<()> {
    let values = &realm.values;
    let signals = realm.retry_signal_domain(values.factory(), ActualStreamFailureReceipt::default())?;
    imports::install(values.factory(), &realm.globals.environment, &signals.headers, values.runtime())
}

impl NativeRetryBindings {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        realm: &Arc<NativeRealm>,
        calling: NativeCallingRun,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<Arc<Self>> {
        calling.check(factory)?;
        if !Arc::ptr_eq(factory.heap(), realm.values.heap()) {
            return Err(ValueError::ForeignHeap);
        }
        let signals = realm.retry_signal_domain(factory, receipt.clone())?;
        signals.bind_calling(calling.clone())?;
        calling.with_runtime(factory, &mut |runtime| {
            imports::install(
                factory,
                &realm.globals.environment,
                &signals.headers,
                runtime,
            )?;
            oneshot_math::install(
                factory,
                &realm.globals.environment,
                &realm.globals.errors,
                runtime,
            )?;
            factory.heap().undefined()
        })?;
        let timers =
            NativeTimerDomain::prepare(factory, calling.clone(), receipt.clone(), signals.clone());
        Ok(Arc::new(Self {
            calling,
            signals,
            timers,
            globals: realm.globals.environment.clone(),
            error: realm.error_constructor(),
            type_error: realm.globals.errors.type_error.clone(),
            async_iterator: factory
                .heap()
                .module_symbol("ara-cli.ecma.well-known", "Symbol.asyncIterator")?,
            receipt,
        }))
    }
    pub fn signal_domain(&self) -> Arc<NativeSignalDomain> {
        self.signals.clone()
    }
    pub fn calling_run(&self) -> NativeCallingRun {
        self.calling.clone()
    }
    /// Prepare source imports on this original invocation; all resulting
    /// functions retain only heap captures and borrow each calling runtime.
    pub fn oneshot_module(
        &self,
        factory: &Arc<GraphValueFactory>,
    ) -> Result<Arc<ara_ai::retry::oneshot::OneshotModule>> {
        use ara_ai::{
            native_error::{NativeErrorFunctionOwner, NativeErrorImports},
            retry::{
                hints::RetryAfterModule, native_retry_hint::NativeRetryHintModule,
                oneshot::OneshotModule,
            },
        };
        self.calling.check(factory)?;
        let hints = NativeRetryHintModule::prepare(factory, self.globals.clone())?;
        let errors = NativeErrorFunctionOwner::prepare(
            factory,
            NativeErrorImports {
                globals: self.globals.clone(),
                extract_retry_hint: Some(hints.extract_retry_hint.clone()),
            },
        )?;
        let namespace = self.calling.with_runtime(factory, &mut |runtime| {
            let exports = errors.prepare_retry_namespace(
                factory,
                self.signals.classifier_imports(),
                runtime,
            )?;
            Ok(exports.namespace)
        })?;
        Ok(Arc::new(OneshotModule::prepare(
            factory,
            namespace,
            Arc::new(RetryAfterModule::prepare(factory, self.globals.clone())?),
            hints.extract_retry_hint,
        )?))
    }
}
impl RetryBindings for NativeRetryBindings {
    fn adopt_local_work(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
        ticket: ara_ai::event::LocalWorkTicket,
    ) -> Result<BoxFuture<'static, NativeWorkObservation>> {
        self.calling.adopt_local_work(factory, self.globals.clone(), value, ticket)
    }
    fn check_invocation(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        self.calling.check(factory)
    }
    fn with_runtime(
        &self,
        factory: &Arc<GraphValueFactory>,
        operation: &mut dyn FnMut(&dyn ValueRuntime) -> Result<ValueHandle>,
    ) -> Result<ValueHandle> {
        self.calling.with_runtime(factory, operation)
    }
    fn await_value(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, Result<ValueHandle>> {
        self.calling.await_value(factory, value)
    }
    fn await_observed(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, NativeWorkObservation> {
        self.calling.await_observed(factory, value)
    }
    fn async_iterator_key(&self) -> PropertyKey {
        self.async_iterator.clone()
    }
    fn invocation_ended(&self) -> BoxFuture<'static, ValueError> {
        self.calling.invocation_ended()
    }
    fn globals(&self) -> ValueHandle {
        self.globals.clone()
    }
    fn error_constructor(&self) -> ValueHandle {
        self.error.clone()
    }
    fn type_error_constructor(&self) -> Result<ValueHandle> {
        Ok(self.type_error.clone())
    }
    fn provider_response_error_constructor(&self) -> ValueHandle {
        self.signals.provider_response_error_constructor()
    }
    fn signals(&self) -> Arc<dyn AbortSignalImports> {
        self.signals.clone()
    }
    fn timers(&self) -> Arc<dyn TimerImports> {
        self.timers.clone()
    }
    fn receipt(&self) -> ActualStreamFailureReceipt {
        self.receipt.clone()
    }
    fn environment(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}
