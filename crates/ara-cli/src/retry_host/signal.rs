//! CLI same-realm AbortController/AbortSignal domain used by fixed OMP retry.
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc. See THIRD_PARTY_NOTICES.md (MIT).
//! Native slots are branded by heap and node; receivers cannot forge slots by
//! copying aborted/reason properties. Reverse callback entries are Weak only.
use crate::mock_provider_host::NativeCallingRun;
use ara_ai::{
    ActualStreamFailureReceipt,
    graph_values::GraphValueFactory,
    retry::{AbortControllerRef, AbortSignalImports},
};
use ara_rpc::value_heap::{
    CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, NodeId, PropertyDescriptor,
    RuntimeReply, ValueError, ValueHandle, ValueKind, ValueRuntime, ValueSnapshot,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock, Weak},
};
use tokio_util::sync::CancellationToken;

type Result<T> = std::result::Result<T, ValueError>;
pub(crate) const SIGNAL_PORT: CapabilityId = CapabilityId(0x4152415f52534947);
pub(crate) fn claims(capability: CapabilityId) -> bool {
    capability == SIGNAL_PORT
}

#[derive(Clone)]
pub(crate) struct NativeRetryErrorImports {
    pub error: ValueHandle,
    pub abort_error: ValueHandle,
    pub dom_exception: ValueHandle,
    pub stream_timeout_error: ValueHandle,
    pub provider_response_error: ValueHandle,
    pub aws_credentials_error: ValueHandle,
    pub anthropic_connection_timeout_error: ValueHandle,
    pub anthropic_connection_error: ValueHandle,
    pub provider_http_error: ValueHandle,
}
enum ListenerTarget {
    Callback(ValueHandle),
    Dependent(NodeId),
}
struct Listener {
    target: ListenerTarget,
    once: bool,
    capture: bool,
    receipt: ActualStreamFailureReceipt,
    calling: Option<NativeCallingRun>,
}
struct SignalSlot {
    signal: ValueHandle,
    token: CancellationToken,
    reason: Option<ValueHandle>,
    listeners: Vec<Listener>,
    sources: Vec<NodeId>,
}
struct SignalState {
    signals: HashMap<NodeId, SignalSlot>,
    controllers: HashMap<NodeId, NodeId>,
    callbacks: HashMap<NodeId, CallbackEntry>,
    headers: HashMap<NodeId, Vec<(String, ara_rpc::WireString)>>,
    operation_domains: Vec<Weak<NativeSignalDomain>>,
}
struct CallbackEntry {
    body: Weak<dyn Fn() -> Result<()> + Send + Sync>,
    handle: NodeId,
    receipt: ActualStreamFailureReceipt,
    calling: NativeCallingRun,
}
pub struct NativeSignalDomain {
    factory: Weak<GraphValueFactory>,
    heap_token: u64,
    identity: ValueHandle,
    state: Arc<Mutex<SignalState>>,
    pub abort_controller: ValueHandle,
    pub abort_signal: ValueHandle,
    pub headers: ValueHandle,
    signal_prototype: ValueHandle,
    errors: NativeRetryErrorImports,
    receipt: ActualStreamFailureReceipt,
    calling: OnceLock<NativeCallingRun>,
}
impl NativeSignalDomain {
    pub(crate) fn prepare(
        factory: &Arc<GraphValueFactory>,
        errors: NativeRetryErrorImports,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<Arc<Self>> {
        let heap = factory.heap();
        for error in [
            &errors.error,
            &errors.abort_error,
            &errors.dom_exception,
            &errors.stream_timeout_error,
            &errors.provider_response_error,
            &errors.aws_credentials_error,
            &errors.anthropic_connection_timeout_error,
            &errors.anthropic_connection_error,
            &errors.provider_http_error,
        ] {
            heap.adopt(error)?;
        }
        let port = heap.function(SIGNAL_PORT)?;
        let identity = heap.object()?;
        let imports = [port, identity.clone()];
        let abort_controller = heap.fresh_compiled_function(&CONTROLLER, &imports)?;
        let abort_signal = heap.fresh_compiled_function(&SIGNAL, &imports)?;
        let headers = heap.fresh_compiled_function(&HEADERS, &imports)?;
        let headers_prototype = heap.get(&headers, &"prototype".into(), factory.runtime())?;
        let controller_prototype =
            heap.get(&abort_controller, &"prototype".into(), factory.runtime())?;
        let signal_prototype = heap.get(&abort_signal, &"prototype".into(), factory.runtime())?;
        for (target, name, spec) in [
            (&controller_prototype, "abort", &ABORT),
            (&signal_prototype, "addEventListener", &ADD_LISTENER),
            (&signal_prototype, "removeEventListener", &REMOVE_LISTENER),
            (&signal_prototype, "throwIfAborted", &THROW_IF_ABORTED),
            (&abort_signal, "any", &ANY),
            (&headers_prototype, "get", &HEADER_GET),
            (&headers_prototype, "set", &HEADER_SET),
            (&headers_prototype, "append", &HEADER_APPEND),
        ] {
            heap.define_own(
                target,
                name.into(),
                PropertyDescriptor::Data {
                    value: heap.fresh_compiled_function(spec, &imports)?,
                    writable: true,
                    enumerable: false,
                    configurable: true,
                },
                factory.runtime(),
            )?;
        }
        for (target, name, spec) in [
            (&controller_prototype, "signal", &GET_SIGNAL),
            (&signal_prototype, "aborted", &GET_ABORTED),
            (&signal_prototype, "reason", &GET_REASON),
        ] {
            let function = heap.fresh_compiled_function(spec, &imports)?;
            let ValueSnapshot::Function(callable) = heap.snapshot(&function)? else {
                return Err(ValueError::InvalidNode);
            };
            heap.define_own(
                target,
                name.into(),
                PropertyDescriptor::Accessor {
                    get: Some(callable),
                    set: None,
                    enumerable: true,
                    configurable: true,
                },
                factory.runtime(),
            )?;
        }
        Ok(Arc::new(Self {
            factory: Arc::downgrade(factory),
            heap_token: heap.heap_token(),
            identity,
            state: Arc::new(Mutex::new(SignalState {
                signals: HashMap::new(),
                controllers: HashMap::new(),
                callbacks: HashMap::new(),
                headers: HashMap::new(),
                operation_domains: Vec::new(),
            })),
            abort_controller,
            abort_signal,
            headers,
            signal_prototype,
            errors,
            receipt,
            calling: OnceLock::new(),
        }))
    }
    /// Realm retains the first prepared domain as the same-Session signal
    /// authority. Operation forks reuse its exact constructors/prototypes and
    /// slots, while their private callback tags and receipts remain distinct.
    pub(crate) fn fork(
        &self,
        factory: &Arc<GraphValueFactory>,
        receipt: ActualStreamFailureReceipt,
    ) -> Result<Arc<Self>> {
        self.check(factory)?;
        let domain = Arc::new(Self {
            factory: Arc::downgrade(factory),
            heap_token: self.heap_token,
            identity: factory.heap().object()?,
            state: self.state.clone(),
            abort_controller: self.abort_controller.clone(),
            abort_signal: self.abort_signal.clone(),
            headers: self.headers.clone(),
            signal_prototype: self.signal_prototype.clone(),
            errors: self.errors.clone(),
            receipt,
            calling: OnceLock::new(),
        });
        self.state
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .operation_domains
            .push(Arc::downgrade(&domain));
        Ok(domain)
    }
    pub(crate) fn bind_calling(&self, calling: NativeCallingRun) -> Result<()> {
        calling.check(&self.factory()?)?;
        self.calling
            .set(calling)
            .map_err(|_| ValueError::UnsupportedOperation("Signal calling Run already bound"))
    }
    pub(crate) fn provider_response_error_constructor(&self) -> ValueHandle {
        self.errors.provider_response_error.clone()
    }
    pub(crate) fn classifier_imports(&self) -> ara_ai::native_error::NativeRetryClassifierImports {
        ara_ai::native_error::NativeRetryClassifierImports {
            aws_credentials_error: self.errors.aws_credentials_error.clone(),
            anthropic_connection_timeout_error: self
                .errors
                .anthropic_connection_timeout_error
                .clone(),
            anthropic_connection_error: self.errors.anthropic_connection_error.clone(),
            provider_http_error: self.errors.provider_http_error.clone(),
            abort_error: self.errors.abort_error.clone(),
        }
    }
    fn registration_owner(
        &self,
        factory: &Arc<GraphValueFactory>,
        runtime: &dyn ValueRuntime,
    ) -> Result<(NativeCallingRun, ActualStreamFailureReceipt)> {
        let domains = self
            .state
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .operation_domains
            .iter()
            .filter_map(Weak::upgrade)
            .collect::<Vec<_>>();
        if let Some(calling) = self.calling.get() {
            if calling.matches_runtime(factory, runtime)? {
                return Ok((calling.clone(), self.receipt.clone()));
            }
        }
        let mut matched = None;
        for domain in domains {
            if let Some(calling) = domain.calling.get() {
                if calling.matches_runtime(factory, runtime)? {
                    if matched.is_some() {
                        return Err(ValueError::UnsupportedOperation(
                            "Signal listener issuing Run matches multiple operation receipts",
                        ));
                    }
                    matched = Some((calling.clone(), domain.receipt.clone()));
                }
            }
        }
        matched.ok_or(ValueError::UnsupportedOperation(
            "Signal listener has no authentic issuing Run",
        ))
    }
    fn check(&self, factory: &Arc<GraphValueFactory>) -> Result<()> {
        let original = self
            .factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation("Signal Session expired"))?;
        if !Arc::ptr_eq(&original, factory) || factory.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
    fn factory(&self) -> Result<Arc<GraphValueFactory>> {
        self.factory
            .upgrade()
            .ok_or(ValueError::UnsupportedOperation("Signal Session expired"))
    }
    fn create_signal(&self, factory: &Arc<GraphValueFactory>) -> Result<ValueHandle> {
        let signal = factory
            .heap()
            .object_with_prototype(Some(&self.signal_prototype))?;
        let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
        state
            .signals
            .try_reserve(1)
            .map_err(|_| ValueError::Allocation)?;
        state.signals.insert(
            signal.node_id(),
            SignalSlot {
                signal: signal.clone(),
                token: CancellationToken::new(),
                reason: None,
                listeners: Vec::new(),
                sources: Vec::new(),
            },
        );
        Ok(signal)
    }
    fn abort_signal(
        &self,
        signal: NodeId,
        reason: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<()> {
        let factory = self.factory()?;
        let (token, source_links, listeners, receiver) = {
            let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
            let slot = state.signals.get_mut(&signal).ok_or_else(|| {
                ValueError::TypeError("AbortSignal receiver has no native slot".into())
            })?;
            if slot.reason.is_some() {
                return Ok(());
            }
            slot.reason = Some(reason.clone());
            let listeners = slot
                .listeners
                .iter()
                .map(|listener| {
                    (
                        match &listener.target {
                            ListenerTarget::Callback(callback) => {
                                ListenerTarget::Callback(callback.clone())
                            }
                            ListenerTarget::Dependent(target) => ListenerTarget::Dependent(*target),
                        },
                        listener.receipt.clone(),
                        listener.calling.clone(),
                    )
                })
                .collect::<Vec<_>>();
            slot.listeners.retain(|listener| !listener.once);
            (
                slot.token.clone(),
                std::mem::take(&mut slot.sources),
                listeners,
                slot.signal.clone(),
            )
        };
        // Latch and wake before callback dispatch; callbacks may reenter abort.
        token.cancel();
        if !source_links.is_empty() {
            let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
            for source in source_links {
                if let Some(slot) = state.signals.get_mut(&source) {
                    slot.listeners.retain(|listener| !matches!(listener.target, ListenerTarget::Dependent(target) if target == signal));
                }
            }
        }
        let event = factory.heap().object()?;
        factory.heap().define_own_data(
            &event,
            "type".into(),
            factory.heap().string("abort".into())?,
            runtime,
        )?;
        factory
            .heap()
            .define_own_data(&event, "target".into(), receiver.clone(), runtime)?;
        for (listener, receipt, calling) in listeners {
            match listener {
                ListenerTarget::Dependent(target) => {
                    self.abort_signal(target, reason.clone(), runtime)?
                }
                ListenerTarget::Callback(callback) => {
                    // DOM dispatch reports listener failures and continues
                    // the remaining listeners/dependent signals.
                    let dispatch = calling
                        .ok_or(ValueError::UnsupportedOperation(
                            "Listener issuing Run missing",
                        ))
                        .and_then(|calling| {
                            calling.with_runtime(&factory, &mut |original_runtime| {
                                factory.heap().call(
                                    &callback,
                                    &receiver,
                                    std::slice::from_ref(&event),
                                    original_runtime,
                                )
                            })
                        });
                    if let Err(error) = dispatch {
                        receipt.record(error);
                    }
                }
            }
        }
        Ok(())
    }
    fn any_impl(
        &self,
        factory: &Arc<GraphValueFactory>,
        signals: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let merged = self.create_signal(factory)?;
        let mut source_ids = Vec::new();
        let early = {
            let state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
            let mut early = None;
            for signal in signals {
                factory.heap().adopt(signal)?;
                let source = state.signals.get(&signal.node_id()).ok_or_else(|| {
                    ValueError::TypeError(
                        "AbortSignal.any requires genuine AbortSignal entries".into(),
                    )
                })?;
                if early.is_none() {
                    if let Some(reason) = &source.reason {
                        early = Some(reason.clone());
                    }
                }
                if !source_ids.contains(&signal.node_id()) {
                    source_ids.push(signal.node_id());
                }
            }
            early
        };
        if let Some(reason) = early {
            self.abort_signal(merged.node_id(), reason, runtime)?;
            return Ok(merged);
        }
        let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
        for source in &source_ids {
            let slot = state
                .signals
                .get_mut(source)
                .ok_or(ValueError::InvalidNode)?;
            slot.listeners
                .try_reserve(1)
                .map_err(|_| ValueError::Allocation)?;
            slot.listeners.push(Listener {
                target: ListenerTarget::Dependent(merged.node_id()),
                once: true,
                capture: false,
                receipt: self.receipt.clone(),
                calling: None,
            });
        }
        state
            .signals
            .get_mut(&merged.node_id())
            .ok_or(ValueError::InvalidNode)?
            .sources = source_ids;
        Ok(merged)
    }
    pub(crate) fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        if capability != SIGNAL_PORT {
            return None;
        }
        if !arguments
            .first()
            .is_some_and(|tag| tag.same_identity(&self.identity))
        {
            return None;
        }
        Some(
            self.invoke(receiver, arguments, runtime)
                .map(RuntimeReply::Value),
        )
    }
    fn invoke(
        &self,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        if receiver.heap().heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        let factory = self.factory()?;
        let heap = factory.heap();
        let operation = match heap.snapshot(arguments.get(1).ok_or(ValueError::InvalidNode)?)? {
            ValueSnapshot::String(text) => text
                .to_utf8()
                .map_err(|_| ValueError::WrongRuntimeReply("Signal operation"))?,
            _ => {
                return Err(ValueError::WrongRuntimeReply(
                    "Signal operation must be primitive string",
                ));
            }
        };
        let arg = |index: usize| {
            arguments
                .get(index + 1)
                .cloned()
                .map(Ok)
                .unwrap_or_else(|| heap.undefined())
        };
        match operation.as_str() {
            "initialize" => {
                let signal = self.create_signal(&factory)?;
                let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
                state
                    .controllers
                    .try_reserve(1)
                    .map_err(|_| ValueError::Allocation)?;
                state
                    .controllers
                    .insert(receiver.node_id(), signal.node_id());
                Ok(receiver.clone())
            }
            "signal" => {
                let state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
                let signal = state.controllers.get(&receiver.node_id()).ok_or_else(|| {
                    ValueError::TypeError("AbortController receiver has no native slot".into())
                })?;
                Ok(state
                    .signals
                    .get(signal)
                    .ok_or(ValueError::InvalidNode)?
                    .signal
                    .clone())
            }
            "aborted" | "reason" | "throwIfAborted" => {
                let reason = self
                    .state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .signals
                    .get(&receiver.node_id())
                    .ok_or_else(|| {
                        ValueError::TypeError("AbortSignal receiver has no native slot".into())
                    })?
                    .reason
                    .clone();
                match operation.as_str() {
                    "aborted" => heap.boolean(reason.is_some()),
                    "reason" => reason.map(Ok).unwrap_or_else(|| heap.undefined()),
                    _ => match reason {
                        Some(reason) => Err(ValueError::Thrown(reason)),
                        None => heap.undefined(),
                    },
                }
            }
            "abort" => {
                let signal = *self
                    .state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .controllers
                    .get(&receiver.node_id())
                    .ok_or_else(|| {
                        ValueError::TypeError("AbortController receiver has no native slot".into())
                    })?;
                let reason = arg(1)?;
                let reason = if heap.kind(&reason)? == ValueKind::Undefined {
                    heap.construct(
                        &self.errors.dom_exception,
                        &[
                            heap.string("This operation was aborted".into())?,
                            heap.string("AbortError".into())?,
                        ],
                        runtime,
                    )?
                } else {
                    reason
                };
                self.abort_signal(signal, reason, runtime)?;
                heap.undefined()
            }
            "any" => {
                let values = arg(1)?;
                let mut iterator = heap.get_iterator(&values, runtime)?;
                let mut signals = Vec::new();
                while let Some(signal) = heap.iterator_next(&mut iterator, runtime)? {
                    signals.push(signal);
                }
                self.any_impl(&factory, &signals, runtime)
            }
            "add" | "remove" => {
                let event = heap.to_string(&arg(1)?, runtime)?;
                let callback = arg(2)?;
                let options = arg(3)?;
                let (once, capture) = if heap.kind(&options)? == ValueKind::Boolean {
                    (false, heap.truthy(&options)?)
                } else if matches!(heap.kind(&options)?, ValueKind::Null | ValueKind::Undefined) {
                    (false, false)
                } else {
                    let capture =
                        heap.truthy(&heap.get(&options, &"capture".into(), runtime)?)?;
                    let once = if operation == "add" {
                        heap.truthy(&heap.get(&options, &"once".into(), runtime)?)?
                    } else {
                        false
                    };
                    (once, capture)
                };
                let registered_owner = self
                    .state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .callbacks
                    .values()
                    .find(|entry| entry.handle == callback.node_id())
                    .map(|entry| (entry.calling.clone(), entry.receipt.clone()));
                let owner = if operation == "add"
                    && event.units() == ara_rpc::WireString::from("abort").units()
                    && !matches!(
                        heap.kind(&callback)?,
                        ValueKind::Null | ValueKind::Undefined
                    ) {
                    Some(match registered_owner {
                        Some(owner) => owner,
                        None => self.registration_owner(&factory, runtime)?,
                    })
                } else {
                    None
                };
                let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
                let slot = state.signals.get_mut(&receiver.node_id()).ok_or_else(|| {
                    ValueError::TypeError("AbortSignal listener receiver has no native slot".into())
                })?;
                if event.units() != ara_rpc::WireString::from("abort").units()
                    || matches!(
                        heap.kind(&callback)?,
                        ValueKind::Null | ValueKind::Undefined
                    )
                {
                    return heap.undefined();
                }
                if operation == "remove" {
                    slot.listeners.retain(|listener| !(listener.capture == capture && matches!(&listener.target, ListenerTarget::Callback(original) if original.same_identity(&callback))));
                } else if !slot.listeners.iter().any(|listener| listener.capture == capture && matches!(&listener.target, ListenerTarget::Callback(original) if original.same_identity(&callback))) {
                    slot.listeners.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                    let (calling, receipt) = owner.ok_or(ValueError::InvalidNode)?;
                    slot.listeners.push(Listener { target: ListenerTarget::Callback(callback), once, capture, receipt, calling: Some(calling) });
                }
                heap.undefined()
            }
            "callback" => {
                let key = arguments.get(2).ok_or(ValueError::InvalidNode)?.node_id();
                let callback = self
                    .state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .callbacks
                    .get(&key)
                    .and_then(|entry| entry.body.upgrade())
                    .ok_or(ValueError::UnsupportedOperation(
                        "Retry callback owner expired",
                    ))?;
                callback()?;
                heap.undefined()
            }
            "headers.initialize" => {
                let supplied = arg(1)?;
                let inherited = self
                    .state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .headers
                    .get(&supplied.node_id())
                    .cloned();
                let mut entries = inherited.unwrap_or_default();
                if entries.is_empty()
                    && !matches!(
                        heap.kind(&supplied)?,
                        ValueKind::Null | ValueKind::Undefined
                    )
                {
                    let iterator_method = heap.get(&supplied, &heap.iterator_key(), runtime)?;
                    if !matches!(
                        heap.kind(&iterator_method)?,
                        ValueKind::Null | ValueKind::Undefined
                    ) {
                        let mut iterator = heap.get_iterator(&supplied, runtime)?;
                        let result = (|| {
                            while let Some(pair) = heap.iterator_next(&mut iterator, runtime)? {
                                let mut fields = heap.get_iterator(&pair, runtime)?;
                                let name = heap.iterator_next(&mut fields, runtime)?.ok_or_else(|| ValueError::TypeError("Headers initializer pair must contain exactly two entries".into()))?;
                                let value = heap.iterator_next(&mut fields, runtime)?.ok_or_else(|| ValueError::TypeError("Headers initializer pair must contain exactly two entries".into()))?;
                                if heap.iterator_next(&mut fields, runtime)?.is_some() {
                                    heap.iterator_close(&mut fields, runtime)?;
                                    return Err(ValueError::TypeError(
                                        "Headers initializer pair must contain exactly two entries"
                                            .into(),
                                    ));
                                }
                                append_header(
                                    &mut entries,
                                    header_name(heap, &name, runtime)?,
                                    header_value(heap, &value, runtime)?,
                                );
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            heap.iterator_close(&mut iterator, runtime)?;
                            return Err(error);
                        }
                    } else {
                        for key in heap.own_keys(&supplied, runtime)? {
                            let ara_rpc::value_heap::PropertyKey::String(name) = &key else {
                                continue;
                            };
                            if !heap
                                .own_descriptor(&supplied, &key, runtime)?
                                .is_some_and(|descriptor| descriptor.enumerable())
                            {
                                continue;
                            }
                            let value = heap.get(&supplied, &key, runtime)?;
                            append_header(
                                &mut entries,
                                header_name(heap, &heap.string(name.clone())?, runtime)?,
                                header_value(heap, &value, runtime)?,
                            );
                        }
                    }
                }
                self.state
                    .lock()
                    .map_err(|_| ValueError::PoisonedHeap)?
                    .headers
                    .insert(receiver.node_id(), entries);
                Ok(receiver.clone())
            }
            "headers.get" | "headers.set" | "headers.append" => {
                let name = header_name(heap, &arg(1)?, runtime)?;
                let value = if operation == "headers.get" {
                    None
                } else {
                    Some(header_value(heap, &arg(2)?, runtime)?)
                };
                let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
                let entries = state.headers.get_mut(&receiver.node_id()).ok_or_else(|| {
                    ValueError::TypeError("Headers receiver has no native slot".into())
                })?;
                if operation == "headers.get" {
                    match entries.iter().find(|(key, _)| key == &name) {
                        Some((_, value)) => heap.string(value.clone()),
                        None => heap.null(),
                    }
                } else {
                    if operation == "headers.set" {
                        entries.retain(|(key, _)| key != &name);
                    }
                    append_header(entries, name, value.ok_or(ValueError::InvalidNode)?);
                    heap.undefined()
                }
            }
            _ => Err(ValueError::WrongRuntimeReply(
                "Unknown AbortSignal operation",
            )),
        }
    }
}
impl AbortSignalImports for NativeSignalDomain {
    fn controller(
        &self,
        factory: &Arc<GraphValueFactory>,
        runtime: &dyn ValueRuntime,
    ) -> Result<AbortControllerRef> {
        self.check(factory)?;
        let controller = factory
            .heap()
            .construct(&self.abort_controller, &[], runtime)?;
        let signal = factory.heap().get(&controller, &"signal".into(), runtime)?;
        Ok(AbortControllerRef { controller, signal })
    }
    fn token(
        &self,
        factory: &Arc<GraphValueFactory>,
        signal: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<CancellationToken> {
        self.check(factory)?;
        factory.heap().adopt(signal)?;
        Ok(self
            .state
            .lock()
            .map_err(|_| ValueError::PoisonedHeap)?
            .signals
            .get(&signal.node_id())
            .ok_or_else(|| ValueError::TypeError("AbortSignal token requires native slot".into()))?
            .token
            .clone())
    }
    fn abort(
        &self,
        factory: &Arc<GraphValueFactory>,
        controller: &AbortControllerRef,
        reason: Option<ValueHandle>,
        runtime: &dyn ValueRuntime,
    ) -> Result<()> {
        self.check(factory)?;
        let callable = factory
            .heap()
            .get(&controller.controller, &"abort".into(), runtime)?;
        factory.heap().call(
            &callable,
            &controller.controller,
            &reason.into_iter().collect::<Vec<_>>(),
            runtime,
        )?;
        Ok(())
    }
    fn any(
        &self,
        factory: &Arc<GraphValueFactory>,
        signals: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check(factory)?;
        let callable = factory
            .heap()
            .get(&self.abort_signal, &"any".into(), runtime)?;
        factory.heap().call(
            &callable,
            &self.abort_signal,
            &[factory.heap().array(signals.to_vec())?],
            runtime,
        )
    }
    fn callback(
        &self,
        factory: &Arc<GraphValueFactory>,
        callback: Arc<dyn Fn() -> Result<()> + Send + Sync>,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check(factory)?;
        let heap = factory.heap();
        let (calling, receipt) = self.registration_owner(factory, runtime)?;
        let key = heap.object()?;
        let body = heap.fresh_compiled_function(
            &CALLBACK,
            &[
                heap.function(SIGNAL_PORT)?,
                self.identity.clone(),
                key.clone(),
            ],
        )?;
        let mut state = self.state.lock().map_err(|_| ValueError::PoisonedHeap)?;
        state
            .callbacks
            .retain(|_, entry| entry.body.strong_count() != 0);
        state
            .callbacks
            .try_reserve(1)
            .map_err(|_| ValueError::Allocation)?;
        state.callbacks.insert(
            key.node_id(),
            CallbackEntry {
                body: Arc::downgrade(&callback),
                handle: body.node_id(),
                receipt,
                calling,
            },
        );
        Ok(body)
    }
    fn error(
        &self,
        factory: &Arc<GraphValueFactory>,
        name: &str,
        message: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.error_value(
            factory,
            name,
            factory.heap().string(message.into())?,
            runtime,
        )
    }
    fn error_value(
        &self,
        factory: &Arc<GraphValueFactory>,
        name: &str,
        message: ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check(factory)?;
        let heap = factory.heap();
        match name {
            "TimeoutError" => heap.construct(
                &self.errors.dom_exception,
                &[message, heap.string(name.into())?],
                runtime,
            ),
            "AbortError" => heap.construct(&self.errors.abort_error, &[message], runtime),
            "StreamTimeoutError" => {
                heap.construct(&self.errors.stream_timeout_error, &[message], runtime)
            }
            _ => Err(ValueError::UnsupportedOperation(
                "Retry signal error constructor is not imported",
            )),
        }
    }
}

fn port(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    operation: &str,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut parameters = Vec::with_capacity(args.len() + 2);
    parameters.push(captures[1].clone());
    parameters.push(heap.string(operation.into())?);
    parameters.extend_from_slice(args);
    heap.call(&captures[0], receiver, &parameters, runtime)
}
static CONTROLLER: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native DOM AbortController constructor for OMP596f2da retry imports",
    name: "AbortController",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::NativeConstructor(|heap, captures, receiver, args, runtime| {
        port(heap, captures, receiver, "initialize", args, runtime)
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AbortController cannot be invoked without new".into(),
        ))
    },
};
static SIGNAL: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native DOM AbortSignal illegal public constructor for OMP596f2da retry imports",
    name: "AbortSignal",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::NativeConstructor(|_, _, _, _, _| {
        Err(ValueError::TypeError("Illegal constructor".into()))
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor AbortSignal cannot be invoked without new".into(),
        ))
    },
};
macro_rules! builtin {
    ($spec:ident,$name:literal,$length:literal,$operation:literal) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: concat!(
                "Native DOM AbortSignal/Controller ",
                $name,
                " for OMP596f2da retry imports"
            ),
            name: $name,
            length: $length,
            capture_count: 2,
            form: CompiledFunctionForm::Builtin,
            body: |heap, captures, receiver, args, runtime| {
                port(heap, captures, receiver, $operation, args, runtime)
            },
        };
    };
}
builtin!(ABORT, "abort", 0, "abort");
builtin!(ADD_LISTENER, "addEventListener", 2, "add");
builtin!(REMOVE_LISTENER, "removeEventListener", 2, "remove");
builtin!(GET_SIGNAL, "get signal", 0, "signal");
builtin!(GET_ABORTED, "get aborted", 0, "aborted");
builtin!(GET_REASON, "get reason", 0, "reason");
builtin!(THROW_IF_ABORTED, "throwIfAborted", 0, "throwIfAborted");
builtin!(ANY, "any", 1, "any");
builtin!(HEADER_GET, "get", 1, "headers.get");
builtin!(HEADER_SET, "set", 2, "headers.set");
builtin!(HEADER_APPEND, "append", 2, "headers.append");
static HEADERS: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "Native Fetch Headers constructor dependency of fixed OMP596f2da retry-after",
    name: "Headers",
    length: 0,
    capture_count: 2,
    form: CompiledFunctionForm::NativeConstructor(|heap, captures, receiver, args, runtime| {
        port(
            heap,
            captures,
            receiver,
            "headers.initialize",
            args,
            runtime,
        )
    }),
    body: |_, _, _, _, _| {
        Err(ValueError::TypeError(
            "Class constructor Headers cannot be invoked without new".into(),
        ))
    },
};

fn header_name(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<String> {
    if heap.kind(value)? == ValueKind::Symbol {
        return Err(ValueError::TypeError(
            "Headers ByteString cannot convert Symbol".into(),
        ));
    }
    let text = heap.to_string(value, runtime)?;
    if text.units().is_empty()
        || !text.units().iter().all(|unit| {
            *unit <= 127
                && (u8::try_from(*unit).is_ok_and(|byte| {
                    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
                }))
        })
    {
        return Err(ValueError::TypeError("Invalid HTTP header name".into()));
    }
    Ok(text
        .units()
        .iter()
        .map(|unit| (*unit as u8).to_ascii_lowercase() as char)
        .collect())
}
fn header_value(
    heap: &Arc<ara_rpc::value_heap::ValueHeap>,
    value: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ara_rpc::WireString> {
    if heap.kind(value)? == ValueKind::Symbol {
        return Err(ValueError::TypeError(
            "Headers ByteString cannot convert Symbol".into(),
        ));
    }
    let text = heap.to_string(value, runtime)?;
    let units = text.units();
    if units
        .iter()
        .any(|unit| *unit > 255 || matches!(*unit, 0 | 10 | 13))
    {
        return Err(ValueError::TypeError("Invalid HTTP header value".into()));
    }
    let start = units
        .iter()
        .position(|unit| !matches!(*unit, 9 | 32))
        .unwrap_or(units.len());
    let end = units
        .iter()
        .rposition(|unit| !matches!(*unit, 9 | 32))
        .map(|index| index + 1)
        .unwrap_or(start);
    Ok(ara_rpc::WireString::from_units(units[start..end].to_vec()))
}
fn append_header(
    entries: &mut Vec<(String, ara_rpc::WireString)>,
    name: String,
    value: ara_rpc::WireString,
) {
    if let Some((_, prior)) = entries.iter_mut().find(|(key, _)| key == &name) {
        let mut units = prior.units().to_vec();
        units.extend_from_slice(&[44, 32]);
        units.extend_from_slice(value.units());
        *prior = ara_rpc::WireString::from_units(units);
    } else {
        entries.push((name, value));
    }
}
static CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da retry listener/timer compiled callback shim",
    name: "",
    length: 0,
    capture_count: 3,
    form: CompiledFunctionForm::Arrow,
    body: |heap, captures, _, _, runtime| {
        port(
            heap,
            captures,
            &heap.undefined()?,
            "callback",
            std::slice::from_ref(&captures[2]),
            runtime,
        )
    },
};
