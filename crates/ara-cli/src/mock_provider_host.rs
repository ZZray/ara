//! Reference Host binding for fixed OMP local MockModel (596f2da, MIT).
//! Catalog classification and custom API storage remain in their existing Host
//! owners; Promise adoption uses the calling NativePromiseAdmission only.
mod calling_run;
pub use calling_run::NativeCallingRun;
use crate::{
    catalog_rules::{ClassifyOptions, classify_model},
    model_registry_extensions::{ExtensionRegistry, ExtensionRegistryHost, RuntimeApiRegistration},
    native_realm::{
        NativeRealm,
        context::NativeScope,
        parent::NativeParentId,
        promise::{NativePromiseAdmission, NativePromiseState, NativePromiseTicket},
    },
};
use ara_ai::{
    graph_values::GraphValueFactory,
    providers::mock::{MOCK_API, MockApi, MockApiRegistrar, MockBindings},
};
use ara_rpc::{
    WireString, WireValue,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, ValueError, ValueHandle,
    },
};
use futures::future::BoxFuture;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ValueError>;
static VALIDATION_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da packages/ai/src/error/validation.ts ValidationError constructor, mock message-only call site",
    name: "ValidationError",
    length: 1,
    capture_count: 1,
    form: CompiledFunctionForm::DerivedConstructor(
        |heap, imports, _, new_target, args, runtime| {
            let message = args.first().cloned().unwrap_or(heap.undefined()?);
            let error =
                heap.construct_with_new_target(&imports[0], &[message], new_target, runtime)?;
            heap.assign(
                &error,
                "name".into(),
                heap.string("ValidationError".into())?,
                runtime,
            )?;
            Ok(error)
        },
    ),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor ValidationError cannot be invoked without new".into(),
        ))
    },
};
/// Concrete module imports plus an existing admitted invocation scope. This
/// owner does not create or retain a NativePromiseRun and cannot drain jobs.
/// Refresh the binding for the actual current Run; old admission stays revoked.
#[derive(Clone)]
struct CallingRun {
    admission: NativePromiseAdmission,
    scope: NativeScope,
    parent: NativeParentId,
}
pub struct NativeMockBindings {
    calling: Mutex<CallingRun>,
    signals: Mutex<Vec<(ValueHandle, CancellationToken)>>,
    error: ValueHandle,
    validation: ValueHandle,
    zero_cost: ValueHandle,
    async_iterator: PropertyKey,
}
/// Explicit Rust Host translation of the call AbortController/AbortSignal pair.
/// Only the owner publishes abort and cancels its exact retained token.
pub struct NativeMockAbortController {
    signal: ValueHandle,
    cancel: CancellationToken,
    error: ValueHandle,
}
impl NativeMockAbortController {
    pub fn signal(&self) -> &ValueHandle {
        &self.signal
    }
    pub fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
    pub fn abort(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        if !self.cancel.is_cancelled() {
            let heap = factory.heap();
            heap.adopt(&self.signal)?;
            let error = heap.construct(
                &self.error,
                &[heap.string("This operation was aborted".into())?],
                factory.runtime(),
            )?;
            heap.define_own_data(
                &self.signal,
                "aborted".into(),
                heap.boolean(true)?,
                factory.runtime(),
            )?;
            heap.define_own_data(&self.signal, "reason".into(), error, factory.runtime())?;
            self.cancel.cancel();
        }
        Ok(())
    }
}
impl NativeMockBindings {
    pub fn prepare(
        factory: &Arc<GraphValueFactory>,
        realm: &Arc<NativeRealm>,
        admission: NativePromiseAdmission,
        scope: NativeScope,
        parent: NativeParentId,
    ) -> Result<Arc<Self>> {
        admission.check_values(&realm.values)?;
        admission.scoped(scope, parent)?;
        let heap = factory.heap();
        if !Arc::ptr_eq(heap, realm.values.heap()) {
            return Err(ValueError::ForeignHeap);
        }
        let error = realm.error_constructor();
        let validation =
            heap.fresh_compiled_function(&VALIDATION_ERROR, std::slice::from_ref(&error))?;
        heap.set_prototype(&validation, Some(&error), factory.runtime())?;
        let prototype = heap.get(&validation, &"prototype".into(), factory.runtime())?;
        let error_prototype = heap.get(&error, &"prototype".into(), factory.runtime())?;
        heap.set_prototype(&prototype, Some(&error_prototype), factory.runtime())?;
        let zero_cost = heap.object()?;
        for key in ["input", "output", "cacheRead", "cacheWrite"] {
            heap.define_own_data(&zero_cost, key.into(), heap.number(0.0)?, factory.runtime())?;
        }
        // One evaluated Rust module's import of the async-iterator protocol.
        // Native sources receive this actual same-realm Symbol key explicitly;
        // it is never guessed from a string-named property or source contents.
        let async_iterator =
            heap.module_symbol("ara-cli.ecma.well-known", "Symbol.asyncIterator")?;
        Ok(Arc::new(Self {
            calling: Mutex::new(CallingRun {
                admission,
                scope,
                parent,
            }),
            signals: Mutex::new(Vec::new()),
            error,
            validation,
            zero_cost,
            async_iterator,
        }))
    }
    pub fn bind_run(
        &self,
        factory: &Arc<GraphValueFactory>,
        realm: &Arc<NativeRealm>,
        admission: NativePromiseAdmission,
        scope: NativeScope,
        parent: NativeParentId,
    ) -> Result<()> {
        admission.check_values(&realm.values)?;
        if !Arc::ptr_eq(factory.heap(), realm.values.heap()) {
            return Err(ValueError::ForeignHeap);
        }
        admission.scoped(scope, parent)?;
        let actual_scope = admission.capture_scope(factory.runtime())?;
        let actual_parent = admission.capture_parent(factory.runtime())?;
        if actual_scope != scope || actual_parent != parent {
            return Err(ValueError::UnsupportedOperation(
                "Mock binding must use the actual current calling scope and parent",
            ));
        }
        *self.calling.lock().unwrap_or_else(|e| e.into_inner()) = CallingRun {
            admission,
            scope,
            parent,
        };
        Ok(())
    }
    fn calling(&self) -> CallingRun {
        self.calling
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
    pub fn abort_controller(
        &self,
        factory: &Arc<GraphValueFactory>,
    ) -> Result<NativeMockAbortController> {
        let heap = factory.heap();
        heap.adopt(&self.error)?;
        let signal = heap.object()?;
        let cancel = CancellationToken::new();
        heap.define_own_data(
            &signal,
            "aborted".into(),
            heap.boolean(false)?,
            factory.runtime(),
        )?;
        heap.define_own_data(
            &signal,
            "reason".into(),
            heap.undefined()?,
            factory.runtime(),
        )?;
        self.signals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push((signal.clone(), cancel.clone()));
        Ok(NativeMockAbortController {
            signal,
            cancel,
            error: self.error.clone(),
        })
    }
    pub fn async_iterator_symbol(&self) -> PropertyKey {
        self.async_iterator.clone()
    }
    pub fn validation_constructor(&self) -> &ValueHandle {
        &self.validation
    }
}
impl MockBindings for NativeMockBindings {
    fn check_invocation(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let CallingRun {
            admission,
            scope,
            parent,
        } = self.calling();
        admission.check_live()?;
        if admission.capture_scope(factory.runtime())? != scope
            || admission.capture_parent(factory.runtime())? != parent
        {
            return Err(ValueError::UnsupportedOperation(
                "Mock invocation uses a stale or different calling Run",
            ));
        }
        Ok(())
    }

    fn classify_model(
        &self,
        factory: &Arc<GraphValueFactory>,
        provider: &ValueHandle,
        id: &ValueHandle,
    ) -> Result<ValueHandle> {
        self.calling().admission.check_live()?;
        let heap = factory.heap();
        let provider = heap.to_string(provider, factory.runtime())?;
        let id = heap.to_string(id, factory.runtime())?;
        let provider = provider
            .to_utf8()
            .map_err(|_| ValueError::LegacyBridge("Catalog provider must be scalar Unicode"))?;
        let id = id
            .to_utf8()
            .map_err(|_| ValueError::LegacyBridge("Catalog model ID must be scalar Unicode"))?;
        let identity = classify_model(
            &provider,
            &id,
            ClassifyOptions {
                lenient: true,
                ..Default::default()
            },
        )
        .map_err(|_| ValueError::UnsupportedOperation("Mock catalog classification failed"))?;
        // Host-owned derived model identity is metadata, never actual Context.
        let units: Vec<u16> = identity.to_string().encode_utf16().collect();
        let wire = WireValue::parse_units(&units).map_err(ValueError::StrictParse)?;
        heap.import_strict(wire)
    }
    fn zero_cost(&self) -> ValueHandle {
        self.zero_cost.clone()
    }
    fn async_iterator_key(&self) -> Option<PropertyKey> {
        Some(self.async_iterator.clone())
    }
    fn error_constructor(&self) -> &ValueHandle {
        &self.error
    }
    fn validation_error(
        &self,
        factory: &Arc<GraphValueFactory>,
        message: WireString,
    ) -> Result<ValueError> {
        self.calling().admission.check_live()?;
        let value = factory.heap().construct(
            &self.validation,
            &[factory.heap().string(message)?],
            factory.runtime(),
        )?;
        Ok(ValueError::Thrown(value))
    }
    fn signal_token(
        &self,
        factory: &Arc<GraphValueFactory>,
        signal: &ValueHandle,
    ) -> Result<CancellationToken> {
        factory.heap().adopt(signal)?;
        self.signals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(original, _)| original.same_identity(signal))
            .map(|(_, token)| token.clone())
            .ok_or(ValueError::UnsupportedOperation(
                "Mock AbortSignal has no original Host token binding",
            ))
    }
    fn await_value(
        &self,
        factory: Arc<GraphValueFactory>,
        value: ValueHandle,
    ) -> BoxFuture<'static, Result<ValueHandle>> {
        let CallingRun {
            admission,
            scope,
            parent,
        } = self.calling();
        // Adopt synchronously in the existing current parent before returning
        // a Rust wait future. No public then/species call and no second pump.
        let prepared: Result<NativePromiseTicket> = (|| {
            admission.check_live()?;
            if admission.capture_scope(factory.runtime())? != scope
                || admission.capture_parent(factory.runtime())? != parent
            {
                return Err(ValueError::UnsupportedOperation(
                    "Mock await cannot adopt a different calling Run",
                ));
            }
            factory.heap().adopt(&value)?;
            let pending = admission.prepare_pending(scope, parent)?;
            factory.heap().call(
                &pending.resolve,
                &factory.heap().undefined()?,
                &[value],
                factory.runtime(),
            )?;
            Ok(pending.ticket())
        })();
        Box::pin(async move {
            let ticket = prepared?;
            loop {
                let wake = admission.wake();
                let notified = wake.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                admission.check_live()?;
                match admission.ticket_state(&ticket)? {
                    NativePromiseState::Fulfilled(value) => {
                        return factory.heap().restore_deferred(&value);
                    }
                    NativePromiseState::Rejected(value) => {
                        return Err(ValueError::Thrown(factory.heap().restore_deferred(&value)?));
                    }
                    NativePromiseState::Pending => notified.await,
                }
            }
        })
    }
}

/// A borrowed adapter to the existing registration owner. It owns no registry.
pub struct RegistryMockApiRegistrar<'a> {
    registry: &'a ExtensionRegistry,
    host: &'a dyn ExtensionRegistryHost,
}
impl<'a> RegistryMockApiRegistrar<'a> {
    pub fn new(registry: &'a ExtensionRegistry, host: &'a dyn ExtensionRegistryHost) -> Self {
        Self { registry, host }
    }
}
impl MockApiRegistrar for RegistryMockApiRegistrar<'_> {
    fn register_mock_api(&self, api: MockApi, source_id: WireString) -> Result<()> {
        self.registry
            .register_custom_api(
                RuntimeApiRegistration {
                    api: MOCK_API.into(),
                    handle: Arc::new(api),
                    source_id: Some(source_id),
                },
                self.host,
            )
            .map_err(|error| ValueError::TypeError(error.to_string()))
    }
}
/// The source's getCustomApi consumer: retrieve the current existing owner row,
/// downcast its original executable object, and call its retained dispatch.
pub fn registered_mock_api(registry: &ExtensionRegistry) -> Result<Option<MockApi>> {
    registry
        .snapshot()
        .custom_apis
        .into_iter()
        .find(|entry| entry.api.equals_ascii(MOCK_API))
        .map(|entry| {
            entry
                .handle
                .downcast_ref::<MockApi>()
                .copied()
                .ok_or(ValueError::UnsupportedOperation(
                    "Registered mock API has a foreign executable object",
                ))
        })
        .transpose()
}
