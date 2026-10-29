//! Persistent Native Session preparation, retained intact by the CLI/RPC host.
//! Foundation preparation is not production activation. Original Bun/logging
//! bindings, the complete advertised tool surface and the provider/journal
//! consumers remain separate prerequisites before installing a formal factory.

use crate::{
    actual_tools::{BuiltinToolSource, NativeBuiltinToolHooks, read_continuation::NativeReadContinuation},
    native_logger::{
        logger_host::{NativeLoggerHost, NativeLoggerHostOptions},
        time::NativeLoggerTimeModule,
    },
    native_realm::NativeRealm,
    native_realm::logger::{NativeLoggerBindings, NativeLoggerDomain},
};
use ara_agent::{ActualProviderError, ActualToolResultSanitizer, SessionValues};
use ara_prompt::actual::NativePromptModule;
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueKind};
use std::sync::Arc;

pub mod catalog;
pub mod host;
use catalog::{NativeCatalogSnapshot, NativeToolCatalog};

/// Imports are handles from this exact realm and its real Host action owners.
/// Required functions have no None/default substitute. Binding availability
/// still requires source and executable evidence for the supplied Host bodies.
pub struct NativeFactoryImports {
    pub source: Arc<BuiltinToolSource>,
    pub string_function: ValueHandle,
    pub bun: ValueHandle,
    pub original_logger: Option<ValueHandle>,
}

/// Actual completed setup in this draft's realm. The Host owns cwd, skills,
/// filtering/overrides, transports and grants before it calls finish.
pub struct NativeCatalogFactoryImports {
    pub catalog: Arc<NativeToolCatalog>,
    pub string_function: ValueHandle,
    pub bun: ValueHandle,
    pub original_logger: Option<ValueHandle>,
}

/// Keep this bundle, not only its values Arc, through preparation/adoption and
/// every owned Run. Runtime/source leases are Weak in the reverse direction;
/// compiled prompt/String captures are same-heap edges, not Rust owner leases.
pub struct PreparedNativeSession {
    realm: Arc<NativeRealm>,
    prompt: NativePromptModule,
    original_logger: Option<ValueHandle>,
    tool_hooks: Option<Arc<NativeBuiltinToolHooks>>,
    catalog: Option<Arc<NativeToolCatalog>>,
    session_state: ValueHandle,
    read: Arc<NativeReadContinuation>,
    timing: Option<NativeLoggerTimeModule>,
    logger_bindings: Option<NativeLoggerBindings>,
}

fn prepare_native_logger(
    realm: &Arc<NativeRealm>,
    host_options: NativeLoggerHostOptions,
) -> Result<(NativeLoggerBindings, NativeLoggerTimeModule), ActualProviderError> {
    let host = NativeLoggerHost::new(realm, host_options)?;
    let heap = realm.values.heap();
    let runtime = realm.values.runtime();
    realm.bind_logger_domain(Arc::new(NativeLoggerDomain::new(heap.heap_token(), host)))?;
    let storage = realm.context().prepare_storage(heap)?;
    let logger = NativeLoggerBindings::prepare_sync(heap, &realm.globals, &storage, runtime)?;
    let timing = NativeLoggerTimeModule::prepare_with_shared_cells(
        realm,
        logger.time_imports(runtime)?,
        logger.module_cells(),
        &storage,
    )?;
    logger.install_actual_time(&timing, runtime)?;
    Ok((logger, timing))
}

impl PreparedNativeSession {
    /// Foundation entry used by selected module observations. It cannot pass
    /// require_tool_hooks or be installed as a production Session factory.
    pub fn prepare() -> Result<Arc<Self>, ValueError> {
        Self::from_realm(NativeRealm::prepare()?, None)
    }

    /// Preserve an already prepared realm and original logger binding. Do not
    /// read logger.warn during preparation: the consumer owns that Get order.
    /// Host logger implementations must not strongly capture this Session,
    /// its values/factory/runtime; use compiled heap edges or Weak Host ports.
    pub fn from_realm(realm: Arc<NativeRealm>, original_logger: Option<ValueHandle>) -> Result<Arc<Self>, ValueError> {
        Ok(Arc::new(Self::foundation(realm, original_logger)?))
    }

    fn foundation(realm: Arc<NativeRealm>, original_logger: Option<ValueHandle>) -> Result<Self, ValueError> {
        if let Some(logger) = &original_logger {
            realm.values.check_handle(logger)?;
            if !matches!(realm.values.heap().kind(logger)?, ValueKind::Object | ValueKind::Array | ValueKind::Function)
            {
                return Err(ValueError::TypeError("Original Native logger binding must be an object".into()));
            }
        }
        let prompt = ara_prompt::actual::prepare_module_with_date(
            realm.values.heap(),
            realm.values.runtime(),
            realm.globals.date.clone(),
        )?;
        let session_state = realm.values.heap().object()?;
        if let Some(logger) = &original_logger {
            crate::actual_tools::read_attachments::install(&session_state, &realm, Some(logger))?;
        }
        let read = Arc::new(NativeReadContinuation::prepare(&realm)?);
        Ok(Self {
            realm,
            prompt,
            original_logger,
            tool_hooks: None,
            catalog: None,
            session_state,
            read,
            timing: None,
            logger_bindings: None,
        })
    }

    /// Prepare the concrete existing builtin source/hooks with mandatory
    /// original String/Bun imports. This covers the existing Rust builtin
    /// subset, not complete OMP tools or formal CLI/RPC factory activation.
    pub fn from_complete_bindings(
        realm: Arc<NativeRealm>,
        imports: NativeFactoryImports,
    ) -> Result<Arc<Self>, ActualProviderError> {
        Ok(Arc::new(Self::complete_bindings(realm, imports)?))
    }

    /// Explicitly add the original selected timing exports with real Host
    /// imports. An original logger is mandatory here; existing foundation and
    /// complete-bindings entrances keep their current preparation semantics.
    /// This does not activate the production Session or supply full Logger.
    pub fn from_complete_bindings_with_timing(
        realm: Arc<NativeRealm>,
        imports: NativeFactoryImports,
    ) -> Result<Arc<Self>, ActualProviderError> {
        if imports.original_logger.is_none() {
            return Err(ActualProviderError::Unsupported("Original Native logger is required for timing preparation"));
        }
        let mut prepared = Self::complete_bindings(realm.clone(), imports)?;
        let timing = NativeLoggerTimeModule::prepare(&realm, realm.logger_time_imports()?)?;
        timing.install_into(prepared.require_logger()?, realm.values.runtime())?;
        prepared.timing = Some(timing);
        Ok(Arc::new(prepared))
    }

    /// Assemble the original synchronous Logger and actual time export in one
    /// retained module/state/storage. Host effects are real and paths explicit.
    /// Complete tools/MCP/Session activation remains a separate required gate.
    pub fn from_complete_bindings_with_logger(
        realm: Arc<NativeRealm>,
        mut imports: NativeFactoryImports,
        host_options: NativeLoggerHostOptions,
    ) -> Result<Arc<Self>, ActualProviderError> {
        if imports.original_logger.is_some() {
            return Err(ActualProviderError::Unsupported("Logger assembly cannot replace a supplied original logger"));
        }
        let (logger, timing) = prepare_native_logger(&realm, host_options)?;
        imports.original_logger = Some(logger.sync_module.clone());
        let mut prepared = Self::complete_bindings(realm, imports)?;
        prepared.timing = Some(timing);
        prepared.logger_bindings = Some(logger);
        Ok(Arc::new(prepared))
    }

    fn complete_bindings(realm: Arc<NativeRealm>, imports: NativeFactoryImports) -> Result<Self, ActualProviderError> {
        realm.values.check_handle(&imports.string_function)?;
        realm.values.check_handle(&imports.bun)?;
        if !imports.string_function.same_identity(&realm.globals.string) {
            return Err(ActualProviderError::Protocol("Native factory requires the original realm String Function"));
        }
        if !imports.bun.same_identity(&realm.globals.bun) {
            return Err(ActualProviderError::Protocol("Native factory requires the original realm Bun binding"));
        }
        if !matches!(
            realm.values.heap().kind(&imports.bun)?,
            ValueKind::Object | ValueKind::Array | ValueKind::Function
        ) {
            return Err(ValueError::TypeError("Original Native Bun binding must be an object".into()).into());
        }
        let mut prepared = Self::foundation(realm.clone(), imports.original_logger)?;
        let sanitizer = ActualToolResultSanitizer::new(imports.bun, &realm.values)?;
        let hooks = NativeBuiltinToolHooks::new(
            imports.source.clone(),
            Some(realm.error_constructor()),
            Some(imports.string_function),
            Some(sanitizer),
            &realm.values,
        )?;
        hooks.bind_read_session(&prepared.session_state, &realm.values)?;
        realm.bind_builtin_source(imports.source, &hooks)?;
        prepared.tool_hooks = Some(hooks);
        Ok(prepared)
    }

    /// Finish a fresh foundation after authoritative async setup. This retains
    /// its exact Session graph/module handles, rather than re-evaluating them.
    pub fn finish_catalog_bindings(
        self: &Arc<Self>,
        imports: NativeCatalogFactoryImports,
    ) -> Result<Arc<Self>, ActualProviderError> {
        Ok(Arc::new(self.catalog_bindings(imports)?))
    }

    pub fn finish_catalog_bindings_with_logger(
        self: &Arc<Self>,
        imports: NativeCatalogFactoryImports,
        host_options: NativeLoggerHostOptions,
    ) -> Result<Arc<Self>, ActualProviderError> {
        if imports.original_logger.is_some() || self.original_logger.is_some() {
            return Err(ActualProviderError::Unsupported("Logger assembly cannot replace a supplied original logger"));
        }
        self.check_fresh_draft()?;
        imports.catalog.check_values(self.values())?;
        self.finish_catalog_bindings_with_logger_builder(host_options, |_| Ok((imports, ())))
            .map(|(prepared, ())| prepared)
    }

    /// Authoritative async transport/tool setup has already completed. Prepare
    /// Logger once, then assemble its dependent catalog in this exact realm.
    /// The synchronous builder never reconnects transports or publishes a
    /// retained Session snapshot. On failure, discard this unpublished draft.
    pub fn finish_catalog_bindings_with_logger_builder<T>(
        self: &Arc<Self>,
        host_options: NativeLoggerHostOptions,
        builder: impl FnOnce(&Arc<NativeRealm>) -> Result<(NativeCatalogFactoryImports, T), ActualProviderError>,
    ) -> Result<(Arc<Self>, T), ActualProviderError> {
        if self.original_logger.is_some() {
            return Err(ActualProviderError::Unsupported("Logger assembly cannot replace a supplied original logger"));
        }
        self.check_fresh_draft()?;
        let (logger, timing) = prepare_native_logger(&self.realm, host_options)?;
        let (mut imports, product) = builder(&self.realm)?;
        if imports.original_logger.is_some() {
            return Err(ActualProviderError::Unsupported("Logger assembly cannot replace a supplied original logger"));
        }
        imports.catalog.check_values(self.values())?;
        imports.original_logger = Some(logger.sync_module.clone());
        let mut prepared = self.catalog_bindings(imports)?;
        prepared.timing = Some(timing);
        prepared.logger_bindings = Some(logger);
        Ok((Arc::new(prepared), product))
    }

    fn check_fresh_draft(&self) -> Result<(), ActualProviderError> {
        if self.tool_hooks.is_some()
            || self.catalog.is_some()
            || self.timing.is_some()
            || self.logger_bindings.is_some()
        {
            return Err(ActualProviderError::Protocol("Native catalog finish requires its fresh foundation draft"));
        }
        Ok(())
    }

    fn catalog_bindings(&self, imports: NativeCatalogFactoryImports) -> Result<Self, ActualProviderError> {
        self.check_fresh_draft()?;
        let realm = &self.realm;
        imports.catalog.check_values(&realm.values)?;
        realm.values.check_handle(&imports.string_function)?;
        realm.values.check_handle(&imports.bun)?;
        if !imports.string_function.same_identity(&realm.globals.string)
            || !imports.bun.same_identity(&realm.globals.bun)
        {
            return Err(ActualProviderError::Protocol(
                "Native catalog factory requires the original realm String/Bun imports",
            ));
        }
        let original_logger = match (&self.original_logger, imports.original_logger) {
            (Some(original), Some(candidate)) if !original.same_identity(&candidate) => {
                return Err(ActualProviderError::Protocol("Native catalog finish cannot replace the draft logger"));
            }
            (_, Some(logger)) => Some(logger),
            (original, None) => original.clone(),
        };
        if let Some(logger) = &original_logger {
            realm.values.check_handle(logger)?;
            if !matches!(realm.values.heap().kind(logger)?, ValueKind::Object | ValueKind::Array | ValueKind::Function)
            {
                return Err(ValueError::TypeError("Original Native logger binding must be an object".into()).into());
            }
        }
        crate::actual_tools::read_attachments::install(&self.session_state, realm, original_logger.as_ref())?;
        let snapshot = imports.catalog.snapshot(&realm.values)?;
        let sanitizer = ActualToolResultSanitizer::new(imports.bun, &realm.values)?;
        let hooks = NativeBuiltinToolHooks::new(
            snapshot.builtin().clone(),
            Some(realm.error_constructor()),
            Some(imports.string_function),
            Some(sanitizer),
            &realm.values,
        )?;
        hooks.bind_read_session(&self.session_state, &realm.values)?;
        hooks.install_catalog(imports.catalog.clone(), &realm.values)?;
        realm.bind_catalog(imports.catalog.clone(), &hooks)?;
        Ok(Self {
            realm: realm.clone(),
            prompt: self.prompt.clone(),
            original_logger,
            tool_hooks: Some(hooks),
            catalog: Some(imports.catalog),
            session_state: self.session_state.clone(),
            read: self.read.clone(),
            timing: None,
            logger_bindings: None,
        })
    }

    /// Publish refreshed sources in the same logical Session. Active Runs must
    /// register every new real source port in their original lease first.
    pub fn publish_catalog_snapshot(
        &self,
        snapshot: Arc<NativeCatalogSnapshot>,
        active: Option<&Arc<crate::native_run::NativeRunLease>>,
    ) -> Result<(), ActualProviderError> {
        self.require_tool_hooks()?.prepare_catalog_snapshot(&snapshot, &self.session_state, self.values())?;
        if let Some(active) = active {
            if !std::ptr::eq(self, active.prepared().as_ref()) {
                return Err(ActualProviderError::Protocol(
                    "Native catalog refresh requires the original prepared Run owner",
                ));
            }
            active.attach_catalog_snapshot(snapshot)
        } else {
            self.require_catalog()?.publish(snapshot, self.values(), None)
        }
    }

    pub fn realm(&self) -> &Arc<NativeRealm> {
        &self.realm
    }

    pub fn require_catalog(&self) -> Result<&Arc<NativeToolCatalog>, ActualProviderError> {
        self.catalog.as_ref().ok_or(ActualProviderError::Unsupported("Native Session has no complete current catalog"))
    }

    pub fn catalog(&self) -> Option<&Arc<NativeToolCatalog>> {
        self.catalog.as_ref()
    }

    pub(crate) fn begin_promise_run(&self) -> Result<crate::native_realm::promise::NativePromiseRun, ValueError> {
        self.realm.begin_promise_run()
    }

    pub(crate) fn read_continuation(&self) -> &NativeReadContinuation {
        &self.read
    }

    pub(crate) fn error_constructor(&self) -> ValueHandle {
        self.realm.error_constructor()
    }

    /// Same logical Session graph across Runs; it is not a serialized execution.
    pub fn session_state(&self) -> &ValueHandle {
        &self.session_state
    }

    pub fn values(&self) -> &Arc<SessionValues> {
        &self.realm.values
    }

    pub fn prompt(&self) -> &NativePromptModule {
        &self.prompt
    }

    pub fn logger(&self) -> Option<&ValueHandle> {
        self.original_logger.as_ref()
    }

    pub fn logger_bindings(&self) -> Option<&NativeLoggerBindings> {
        self.logger_bindings.as_ref()
    }

    pub fn timing(&self) -> Option<&NativeLoggerTimeModule> {
        self.timing.as_ref()
    }

    pub fn require_logger(&self) -> Result<&ValueHandle, ValueError> {
        self.logger().ok_or(ValueError::UnsupportedOperation("Original Native logger binding is not connected"))
    }

    pub fn require_tool_hooks(&self) -> Result<&Arc<NativeBuiltinToolHooks>, ActualProviderError> {
        self.tool_hooks.as_ref().ok_or(ActualProviderError::Unsupported(
            "Native Session foundation has no prepared original Bun/source tool hooks",
        ))
    }

    pub fn check_values(&self, values: &Arc<SessionValues>) -> Result<(), ValueError> {
        self.realm.check_values(values)
    }
}
