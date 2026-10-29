//! Actual CLI-owned foundation of one Native logical Session realm.
//! Same-realm global Functions have concrete compiled Rust bodies. The
//! default resolver claims real context AbortSignal and Date Host capabilities;
//! unresolved provider/MCP/general runtime domains remain explicit gaps.

use crate::actual_tools::{BuiltinToolSource, NativeBuiltinToolHooks};
use crate::native_factory::catalog::NativeToolCatalog;
use crate::native_logger::{time::NativeLoggerTimeImports, time_host::NativeLoggerTimeHostDomain};
use ara_agent::{ActualProviderError, ActualToolSource, SessionValues};
use ara_rpc::value_heap::{
    CapabilityId, NoRuntime, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime,
};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub mod ast;
pub mod context;
pub mod date;
pub(crate) mod globals;
pub mod logger;
pub mod parent;
pub mod promise;
pub use globals::{NativeErrorClasses, NativeGlobals};

/// The runtime never retains SessionValues/factory/sinks. The prepared bundle
/// retains the strong tool owner; registry only keeps a Weak original source.
pub struct NativeRuntime {
    source: OnceLock<Weak<BuiltinToolSource>>,
    catalog: OnceLock<Weak<NativeToolCatalog>>,
    date: Weak<date::NativeDateDomain>,
    context: Weak<context::NativeContextDomain>,
    promise: Weak<promise::NativePromiseDomain>,
    parent: Weak<parent::NativeParentDomain>,
    logger_time: OnceLock<Weak<NativeLoggerTimeHostDomain>>,
    logger: OnceLock<Weak<logger::NativeLoggerDomain>>,
    display_callbacks: Mutex<Option<Weak<crate::terminal::graph_renderer::CallbackDomain>>>,
    retry_signals: Mutex<Vec<Weak<crate::retry_host::signal::NativeSignalDomain>>>,
}
impl NativeRuntime {
    fn new(
        date: &Arc<date::NativeDateDomain>,
        context: &Arc<context::NativeContextDomain>,
        promise: &Arc<promise::NativePromiseDomain>,
        parent: &Arc<parent::NativeParentDomain>,
    ) -> Self {
        Self {
            source: OnceLock::new(),
            catalog: OnceLock::new(),
            date: Arc::downgrade(date),
            context: Arc::downgrade(context),
            promise: Arc::downgrade(promise),
            parent: Arc::downgrade(parent),
            logger_time: OnceLock::new(),
            logger: OnceLock::new(),
            display_callbacks: Mutex::new(None),
            retry_signals: Mutex::new(Vec::new()),
        }
    }
    fn known_logger(
        &self,
        request: &RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply, ValueError>> {
        if request.capability() != logger::LOGGER_PORT {
            return None;
        }
        match self.logger.get() {
            Some(locator) => match locator.upgrade() {
                Some(owner) => owner.resolve(request, effective),
                None => Some(Err(ValueError::ExpiredCapability(logger::LOGGER_PORT))),
            },
            None => Some(Err(ValueError::MissingCapability(logger::LOGGER_PORT))),
        }
    }
    fn known_date(
        &self,
        request: &RuntimeRequest<'_>,
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply, ValueError>> {
        if request.capability() != date::DATE_PORT {
            return None;
        }
        match self.date.upgrade() {
            Some(date) => date.resolve(request, effective),
            None => Some(Err(ValueError::ExpiredCapability(date::DATE_PORT))),
        }
    }
    fn known_context(&self, request: &RuntimeRequest<'_>) -> Option<Result<RuntimeReply, ValueError>> {
        if let Some(catalog) = self.catalog.get().and_then(Weak::upgrade) {
            if let Some(reply) = catalog.resolve_context_signal(request) {
                return Some(reply);
            }
        }
        self.source.get()?.upgrade()?.resolve_context_signal(request)
    }
}
impl ValueRuntime for NativeRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        if let Some(result) = self.known_logger(&request, effective_runtime) {
            return result;
        }
        if let Some(result) = self.known_date(&request, effective_runtime) {
            return result;
        }
        if let Some(result) = self.known_context(&request) {
            return result;
        }
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, effective_runtime)
            }
            request => NoRuntime.resolve_effective(request, effective_runtime),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        let request = RuntimeRequest::Call { capability, receiver, arguments };
        if crate::retry_host::signal::claims(capability) {
            let domains = self
                .retry_signals
                .lock()
                .map_err(|_| ValueError::PoisonedHeap)?
                .iter()
                .filter_map(Weak::upgrade)
                .collect::<Vec<_>>();
            for domain in domains {
                if let Some(reply) =
                    domain.resolve_call(capability, receiver, arguments, effective_runtime)
                {
                    return reply;
                }
            }
            return Err(ValueError::ExpiredCapability(capability));
        }
        if let Some(result) = self.known_logger(&request, effective_runtime) {
            return result;
        }
        if let Some(result) = self.known_date(&request, effective_runtime) {
            return result;
        }
        if let Some(result) = self.known_context(&request) {
            return result;
        }
        if crate::terminal::graph_renderer::callback_claims(capability) {
            let domain = self
                .display_callbacks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or(ValueError::ExpiredCapability(capability))?;
            return domain
                .resolve_call(capability, receiver, arguments, effective_runtime)
                .ok_or(ValueError::WrongRuntimeReply("display callback port was not claimed"))?;
        }
        if parent::claims(capability) {
            let parent = self.parent.upgrade().ok_or(ValueError::ExpiredCapability(capability))?;
            return parent
                .resolve_base(capability, receiver, arguments)
                .ok_or(ValueError::WrongRuntimeReply("Native parent port was not claimed"))?;
        }
        if context::claims(capability) {
            let context = self.context.upgrade().ok_or(ValueError::ExpiredCapability(capability))?;
            return context
                .resolve_call(capability, receiver, arguments, effective_runtime)
                .ok_or(ValueError::WrongRuntimeReply("Native context port was not claimed"))?;
        }
        if promise::claims(capability) {
            let promise = self.promise.upgrade().ok_or(ValueError::ExpiredCapability(capability))?;
            return promise
                .resolve_call(capability, receiver, arguments, effective_runtime)
                .ok_or(ValueError::WrongRuntimeReply("Native Promise port was not claimed"))?;
        }
        if crate::native_logger::time_host::claims(capability) {
            let timing =
                self.logger_time.get().and_then(Weak::upgrade).ok_or(ValueError::ExpiredCapability(capability))?;
            return timing
                .resolve_call(capability, receiver, arguments, effective_runtime)
                .ok_or(ValueError::WrongRuntimeReply("Native logger timing port was not claimed"))?;
        }
        // Actual global bodies are Compiled targets handled by ValueHeap.call,
        // and therefore never need a missing opaque evaluator here.
        NoRuntime.resolve_call(capability, receiver, arguments, effective_runtime)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> Result<RuntimeReply, ValueError> {
        NoRuntime.resolve_local_call(token, receiver, arguments, effective_runtime)
    }
}

/// Retain this whole bundle in CLI/RPC initial/reopen/reset prepared state.
/// Returning only `values` and dropping the other owners is insufficient.
pub struct NativeRealm {
    pub values: Arc<SessionValues>,
    pub runtime: Arc<NativeRuntime>,
    pub globals: NativeGlobals,
    pub ast: Arc<ast::NativeAstModule>,
    source: OnceLock<Arc<BuiltinToolSource>>,
    catalog: OnceLock<Arc<NativeToolCatalog>>,
    // Strong Host owner; the runtime's reverse registry is Weak only.
    _date: Arc<date::NativeDateDomain>,
    context: Arc<context::NativeContextDomain>,
    promise: Arc<promise::NativePromiseDomain>,
    parent: Arc<parent::NativeParentDomain>,
    logger_time: OnceLock<Arc<NativeLoggerTimeHostDomain>>,
    logger: OnceLock<Arc<logger::NativeLoggerDomain>>,
    // Same Session classes/slots stay alive; operation reverse references are Weak.
    retry_signals: OnceLock<Arc<crate::retry_host::signal::NativeSignalDomain>>,
    tool_errors: OnceLock<ara_mcp::native_errors::NativeMcpErrorBindings>,
}
impl NativeRealm {
    pub fn prepare() -> Result<Arc<Self>, ValueError> {
        Self::prepare_with_date_host(Arc::new(date::SystemDateHost))
    }

    pub fn prepare_with_date_host(host: Arc<dyn date::DateHost>) -> Result<Arc<Self>, ValueError> {
        host.initialize_timezone_env()?;
        let heap = ValueHeap::new()?;
        let date = Arc::new(date::NativeDateDomain::new(heap.heap_token(), host));
        let context = Arc::new(context::NativeContextDomain::new(&heap)?);
        let parent = Arc::new(parent::NativeParentDomain::new(&heap)?);
        let promise = Arc::new(promise::NativePromiseDomain::new(&heap, &context, &parent)?);
        let runtime = Arc::new(NativeRuntime::new(&date, &context, &promise, &parent));
        // Bodies capture only same-heap NodeIds, not this runtime/owner.
        let globals = NativeGlobals::prepare(&heap, runtime.as_ref())?;
        // Date Host owns the TZ-only environment brand and its process-wide
        // timezone policy; supplied TZ coercions use the original runtime.
        let process = heap.object()?;
        let environment = date.prepare_tz_environment(&heap)?;
        heap.define_own_data(&process, "env".into(), environment, runtime.as_ref())?;
        heap.define_own_data(&globals.environment, "process".into(), process, runtime.as_ref())?;
        let promise_constructor = promise.prepare_globals(&heap, &globals.errors, runtime.as_ref())?;
        globals.install_native_promise(promise_constructor, runtime.as_ref())?;
        let ast = Arc::new(ast::NativeAstModule::prepare(&heap, &globals, runtime.as_ref())?);
        let values = Arc::new(SessionValues::new(heap, runtime.clone())?);
        Ok(Arc::new(Self {
            values,
            runtime,
            globals,
            ast,
            source: OnceLock::new(),
            catalog: OnceLock::new(),
            _date: date,
            context,
            promise,
            parent,
            logger_time: OnceLock::new(),
            logger: OnceLock::new(),
            retry_signals: OnceLock::new(),
            tool_errors: OnceLock::new(),
        }))
    }

    /// One fixed tools/tool-errors module binding per original Session realm.
    /// READ and MCP instanceof checks consume this same class identity.
    pub(crate) fn tool_error_bindings(&self) -> Result<ara_mcp::native_errors::NativeMcpErrorBindings, ValueError> {
        if let Some(bindings) = self.tool_errors.get() { return Ok(bindings.clone()); }
        let candidate = ara_mcp::native_errors::NativeMcpErrorBindings::prepare(
            self.values.heap(), self.globals.environment.clone(), self.values.runtime(),
        )?;
        let _ = self.tool_errors.set(candidate);
        self.tool_errors.get().cloned().ok_or(ValueError::UnsupportedOperation("Tool error module publication failed"))
    }

    pub(crate) fn retry_signal_domain(
        &self,
        factory: &Arc<ara_ai::graph_values::GraphValueFactory>,
        receipt: ara_ai::ActualStreamFailureReceipt,
    ) -> Result<Arc<crate::retry_host::signal::NativeSignalDomain>, ValueError> {
        if !Arc::ptr_eq(factory.heap(), self.values.heap()) {
            return Err(ValueError::ForeignHeap);
        }
        let owner = match self.retry_signals.get() {
            Some(owner) => owner.clone(),
            None => {
                let errors = crate::retry_host::errors::prepare(factory, self.error_constructor())?;
                let candidate = crate::retry_host::signal::NativeSignalDomain::prepare(
                    factory,
                    errors,
                    ara_ai::ActualStreamFailureReceipt::default(),
                )?;
                // Preparation has no graph work under the reverse-registry lock.
                // A concurrent bootstrap winner remains the single authority.
                let _ = self.retry_signals.set(candidate);
                self.retry_signals
                    .get()
                    .ok_or(ValueError::UnsupportedOperation(
                        "Retry signal authority publication failed",
                    ))?
                    .clone()
            }
        };
        let fork = owner.fork(factory, receipt)?;
        let mut domains = self
            .runtime
            .retry_signals
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?;
        domains.retain(|domain| domain.strong_count() != 0);
        if !domains
            .iter()
            .filter_map(Weak::upgrade)
            .any(|domain| Arc::ptr_eq(&domain, &owner))
        {
            domains.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            domains.push(Arc::downgrade(&owner));
        }
        domains.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        domains.push(Arc::downgrade(&fork));
        Ok(fork)
    }

    /// Explicit selected real Host imports. The realm retains their native
    /// owner; the runtime's reverse registration never owns a heap or Session.
    /// A competing initial publication fails visibly rather than replacing an
    /// existing owner or silently falling back to a controlled clock/writer.
    pub fn bind_logger_domain(&self, owner: Arc<logger::NativeLoggerDomain>) -> Result<(), ValueError> {
        owner.check_heap(self.values.heap())?;
        if let Some(current) = self.logger.get() {
            return if Arc::ptr_eq(current, &owner) {
                Ok(())
            } else {
                Err(ValueError::UnsupportedOperation("Native logger owner already installed"))
            };
        }
        self.runtime
            .logger
            .set(Arc::downgrade(&owner))
            .map_err(|_| ValueError::UnsupportedOperation("Concurrent Native logger runtime installation"))?;
        self.logger
            .set(owner)
            .map_err(|_| ValueError::UnsupportedOperation("Native logger owner already installed"))?;
        Ok(())
    }

    pub fn logger_time_imports(&self) -> Result<NativeLoggerTimeImports, ValueError> {
        if let Some(timing) = self.logger_time.get() {
            return timing.imports(self.values.heap(), self.values.runtime());
        }
        let timing = NativeLoggerTimeHostDomain::new(self)?;
        self.runtime
            .logger_time
            .set(Arc::downgrade(&timing))
            .map_err(|_| ValueError::UnsupportedOperation("Concurrent Native logger timing installation"))?;
        self.logger_time
            .set(timing.clone())
            .map_err(|_| ValueError::UnsupportedOperation("Native logger timing owner is already installed"))?;
        timing.imports(self.values.heap(), self.values.runtime())
    }

    /// Publish the existing actual source into the current default resolver
    /// only after hooks and exact values/source identity have been validated.
    /// No tool-name inference and no existing owner replacement.
    pub fn bind_builtin_source(
        &self,
        source: Arc<BuiltinToolSource>,
        hooks: &NativeBuiltinToolHooks,
    ) -> Result<(), ActualProviderError> {
        if self.catalog.get().is_some() {
            return Err(ActualProviderError::Protocol("Native realm already owns a composed catalog"));
        }
        hooks.check_values(&self.values)?;
        let original = hooks.source(&self.values)?;
        let candidate: Arc<dyn ActualToolSource> = source.clone();
        if !Arc::ptr_eq(&original, &candidate) {
            return Err(ActualProviderError::Protocol("Native realm hooks/source owner mismatch"));
        }
        source.bind_ast_module(self.ast.clone(), &self.values)?;
        if let Some(current) = self.source.get() {
            if Arc::ptr_eq(current, &source) {
                return Ok(());
            }
            return Err(ActualProviderError::Protocol("Native realm source is already installed"));
        }
        // Publish the Weak registry first. Every success through the strong
        // source's idempotence branch therefore observes the completed registry.
        // A concurrent loser before strong publication fails visibly, even if
        // it proposed the same source. No callback/Get/await or lock is involved.
        self.runtime
            .source
            .set(Arc::downgrade(&source))
            .map_err(|_| ActualProviderError::Protocol("Concurrent Native runtime source installation"))?;
        self.source
            .set(source)
            .map_err(|_| ActualProviderError::Protocol("Native realm source is already installed"))?;
        Ok(())
    }

    /// A stable catalog owner can publish fresh original source snapshots in
    /// this realm. The reverse resolver remains Weak; exact hooks admission
    /// is retained, and builtin subset publication cannot be silently replaced.
    pub fn bind_catalog(
        &self,
        catalog: Arc<NativeToolCatalog>,
        hooks: &NativeBuiltinToolHooks,
    ) -> Result<(), ActualProviderError> {
        hooks.check_values(&self.values)?;
        catalog.check_values(&self.values)?;
        let original = hooks.source(&self.values)?;
        let candidate: Arc<dyn ActualToolSource> = catalog.clone();
        if !Arc::ptr_eq(&original, &candidate) {
            return Err(ActualProviderError::Protocol("Native realm hooks/catalog owner mismatch"));
        }
        catalog.snapshot(&self.values)?.builtin().bind_ast_module(self.ast.clone(), &self.values)?;
        if self.source.get().is_some() {
            return Err(ActualProviderError::Protocol("Native realm already owns a builtin subset source"));
        }
        if let Some(current) = self.catalog.get() {
            return if Arc::ptr_eq(current, &catalog) {
                Ok(())
            } else {
                Err(ActualProviderError::Protocol("Native realm catalog owner is already installed"))
            };
        }
        self.runtime
            .catalog
            .set(Arc::downgrade(&catalog))
            .map_err(|_| ActualProviderError::Protocol("Concurrent Native runtime catalog installation"))?;
        self.catalog
            .set(catalog)
            .map_err(|_| ActualProviderError::Protocol("Native realm catalog owner is already installed"))?;
        Ok(())
    }

    pub fn check_values(&self, values: &Arc<SessionValues>) -> Result<(), ValueError> {
        if !Arc::ptr_eq(&self.values, values) {
            return Err(ValueError::UnsupportedOperation("Native realm requires its original SessionValues owner"));
        }
        Ok(())
    }

    pub fn promise(&self) -> &Arc<promise::NativePromiseDomain> {
        &self.promise
    }

    /// Foundation admission only. The production caller must retain the full
    /// PreparedNativeSession and separate Host effect settlement owner.
    pub fn begin_promise_run(self: &Arc<Self>) -> Result<promise::NativePromiseRun, ValueError> {
        let context = self.begin_context_run()?;
        self.promise.begin(self.clone(), context)
    }

    /// Pure callback context and Promise scheduling on the original Session.
    /// Independent from Run effect admission; retained expired tools stay expired.
    pub(crate) fn begin_display_callbacks(self: &Arc<Self>) -> Result<promise::NativeDisplayCallbacks, ValueError> {
        let context = self.context.begin_display(self.values.heap())?;
        self.promise.begin_display(self.clone(), context)
    }

    pub(crate) fn bind_display_callbacks(
        &self,
        owner: &Arc<crate::terminal::graph_renderer::CallbackDomain>,
    ) -> Result<(), ValueError> {
        let mut current = self.runtime.display_callbacks.lock().unwrap_or_else(|e| e.into_inner());
        if current.as_ref().and_then(Weak::upgrade).is_some() {
            return Err(ValueError::UnsupportedOperation("Session already has a display callback owner"));
        }
        owner.bind_environment(&self.globals.environment)?;
        owner.bind_device_catalog(self.catalog.get())?;
        *current = Some(Arc::downgrade(owner));
        Ok(())
    }
    pub fn context(&self) -> &Arc<context::NativeContextDomain> {
        &self.context
    }

    /// Foundation host lease; production RPC must retain the complete prepared
    /// bundle and its effect settlement owner before using this admission.
    pub fn begin_context_run(&self) -> Result<context::NativeContextRun, ValueError> {
        self.context.begin(self.values.heap())
    }

    pub fn error_constructor(&self) -> ValueHandle {
        self.globals.errors.error.clone()
    }
}
