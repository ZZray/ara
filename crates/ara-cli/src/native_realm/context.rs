//! CLI scope ownership for fixed OMP logger.ts spanStorage.run/getStore.
//! Frames store checked graph edges, never heap, runtime or Session leases.
//! Promise scheduling and production Run admission remain separate consumers.

use ara_rpc::value_heap::{
    CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, NoRuntime, RuntimeReply, RuntimeRequest,
    ValueError, ValueHandle, ValueHeap, ValueRuntime, ValueSnapshot,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard, Weak},
};

type Result<T> = std::result::Result<T, ValueError>;
const CURRENT_FRAME: CapabilityId = CapabilityId(0x4152415f43545846);
const GET_STORE: CapabilityId = CapabilityId(0x4152415f43545847);
const RUN_STORE: CapabilityId = CapabilityId(0x4152415f43545852);

pub(super) fn claims(capability: CapabilityId) -> bool {
    matches!(capability, CURRENT_FRAME | GET_STORE | RUN_STORE)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeScope {
    pub(super) epoch: u64,
    key: DeferredValue,
}

struct Frame {
    epoch: u64,
    stores: Arc<HashMap<DeferredValue, DeferredValue>>,
}
struct Epoch {
    id: u64,
    revoked: bool,
}
struct State {
    next_epoch: u64,
    active: Option<Epoch>,
    ordinary: Option<Epoch>,
    storages: HashSet<DeferredValue>,
    frames: HashMap<DeferredValue, Frame>,
}

/// Strongly owned by NativeRealm, with only a Weak reverse runtime reference.
/// The private current-frame port authenticates edge identity, not properties.
pub struct NativeContextDomain {
    heap_token: u64,
    frame_port: DeferredValue,
    state: Mutex<State>,
}
impl NativeContextDomain {
    pub(super) fn new(heap: &Arc<ValueHeap>) -> Result<Self> {
        Ok(Self {
            heap_token: heap.heap_token(),
            frame_port: heap.capture_deferred(&heap.object()?)?,
            state: Mutex::new(State {
                next_epoch: 1,
                active: None,
                ordinary: None,
                storages: HashSet::new(),
                frames: HashMap::new(),
            }),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| ValueError::UnsupportedOperation("Native context owner is poisoned"))
    }

    fn check_heap(&self, heap: &ValueHeap) -> Result<()> {
        if heap.heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }

    fn frame<'a>(state: &'a State, scope: NativeScope) -> Result<&'a Frame> {
        let active = state
            .active
            .as_ref()
            .filter(|entry| entry.id == scope.epoch)
            .or_else(|| state.ordinary.as_ref().filter(|entry| entry.id == scope.epoch))
            .ok_or(ValueError::ExpiredCapability(CURRENT_FRAME))?;
        if active.revoked || active.id != scope.epoch {
            return Err(ValueError::ExpiredCapability(CURRENT_FRAME));
        }
        let frame = state.frames.get(&scope.key).ok_or(ValueError::ExpiredCapability(CURRENT_FRAME))?;
        if frame.epoch != scope.epoch {
            return Err(ValueError::ExpiredCapability(CURRENT_FRAME));
        }
        Ok(frame)
    }

    /// Called only by the owning realm's host entry. Graph code cannot enter
    /// a Run through a capability, copied context property or scope token.
    pub(super) fn begin(self: &Arc<Self>, heap: &Arc<ValueHeap>) -> Result<NativeContextRun> {
        self.begin_owned(heap, false)
    }

    pub(super) fn begin_display(self: &Arc<Self>, heap: &Arc<ValueHeap>) -> Result<NativeContextRun> {
        self.begin_owned(heap, true)
    }

    fn begin_owned(self: &Arc<Self>, heap: &Arc<ValueHeap>, ordinary: bool) -> Result<NativeContextRun> {
        self.check_heap(heap)?;
        let key = heap.capture_deferred(&heap.object()?)?;
        let mut state = self.lock()?;
        if (if ordinary { &state.ordinary } else { &state.active }).is_some() {
            return Err(ValueError::UnsupportedOperation("Native context already has an exclusive Run owner"));
        }
        let epoch = state.next_epoch;
        let next = epoch.checked_add(1).ok_or(ValueError::UnsupportedOperation("Native context epochs exhausted"))?;
        state.frames.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        state.frames.insert(key, Frame { epoch, stores: Arc::new(HashMap::new()) });
        state.next_epoch = next;
        let owner = if ordinary { &mut state.ordinary } else { &mut state.active };
        *owner = Some(Epoch { id: epoch, revoked: false });
        Ok(NativeContextRun { domain: self.clone(), root: NativeScope { epoch, key } })
    }

    /// Prepare the actual two storage methods consumed by the original logger.
    /// No constructor/prototype/enterWith/disable Node surface is implied.
    pub fn prepare_storage(&self, heap: &Arc<ValueHeap>) -> Result<ValueHandle> {
        self.check_heap(heap)?;
        let storage = heap.object()?;
        for (name, spec) in [("getStore", &GET_STORE_BODY), ("run", &RUN_STORE_BODY)] {
            heap.define_own_data(&storage, name.into(), heap.fresh_compiled_function(spec, &[])?, &NoRuntime)?;
        }
        let key = heap.capture_deferred(&storage)?;
        let mut state = self.lock()?;
        state.storages.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        state.storages.insert(key);
        Ok(storage)
    }

    /// Capture the currently effective frame, including any inner storage.run.
    /// No graph work or supplied callback executes under the owner mutex.
    pub fn capture_scope(&self, heap: &Arc<ValueHeap>, effective: &dyn ValueRuntime) -> Result<Option<NativeScope>> {
        self.check_heap(heap)?;
        let port = heap.restore_deferred(&self.frame_port)?;
        let current = value(effective.resolve_call(CURRENT_FRAME, &port, &[], effective)?)?;
        if matches!(heap.snapshot(&current)?, ValueSnapshot::Undefined) {
            return Ok(None);
        }
        let key = heap.capture_deferred(&current)?;
        let state = self.lock()?;
        let epoch = state.frames.get(&key).ok_or(ValueError::ExpiredCapability(CURRENT_FRAME))?.epoch;
        let scope = NativeScope { epoch, key };
        Self::frame(&state, scope)?;
        Ok(Some(scope))
    }

    fn fork(
        &self,
        heap: &Arc<ValueHeap>,
        parent: NativeScope,
        storage: DeferredValue,
        store: &ValueHandle,
    ) -> Result<NativeScope> {
        let store = heap.capture_deferred(store)?;
        let stores = Self::frame(&*self.lock()?, parent)?.stores.clone();
        let mut stores = (*stores).clone();
        stores.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        stores.insert(storage, store);
        let key = heap.capture_deferred(&heap.object()?)?;
        let mut state = self.lock()?;
        Self::frame(&state, parent)?;
        state.frames.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        state.frames.insert(key, Frame { epoch: parent.epoch, stores: Arc::new(stores) });
        Ok(NativeScope { epoch: parent.epoch, key })
    }

    pub(super) fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        if !claims(capability) {
            return None;
        }
        Some((|| {
            let heap = receiver.heap();
            self.check_heap(heap)?;
            let receiver_key = heap.capture_deferred(receiver)?;
            if capability == CURRENT_FRAME {
                if receiver_key != self.frame_port || !arguments.is_empty() {
                    return Err(ValueError::UnsupportedOperation("Native current-frame port identity mismatch"));
                }
                // Only ScopedRuntime can supply an admitted frame. Base runtime
                // does not manufacture a root Run or inherit process state.
                return Ok(RuntimeReply::Value(heap.undefined()?));
            }
            if !self.lock()?.storages.contains(&receiver_key) {
                return Err(ValueError::UnsupportedOperation("Native storage receiver is not registered"));
            }
            let scope = self.capture_scope(heap, effective)?;
            if capability == GET_STORE {
                let edge = match scope {
                    Some(scope) => Self::frame(&*self.lock()?, scope)?.stores.get(&receiver_key).copied(),
                    None => None,
                };
                return Ok(RuntimeReply::Value(match edge {
                    Some(edge) => heap.restore_deferred(&edge)?,
                    None => heap.undefined()?,
                }));
            }
            let parent =
                scope.ok_or(ValueError::UnsupportedOperation("Native storage.run requires an admitted Run scope"))?;
            let undefined = heap.undefined()?;
            let store = arguments.first().unwrap_or(&undefined);
            let callback = arguments.get(1).unwrap_or(&undefined);
            let scope = self.fork(heap, parent, receiver_key, store)?;
            let runtime = ScopedRuntime { domain: self, scope, parent: effective };
            Ok(RuntimeReply::Value(heap.call(callback, &undefined, arguments.get(2..).unwrap_or(&[]), &runtime)?))
        })())
    }

    pub(super) fn scoped<'a>(&'a self, scope: NativeScope, parent: &'a dyn ValueRuntime) -> Result<ScopedRuntime<'a>> {
        Self::frame(&*self.lock()?, scope)?;
        Ok(ScopedRuntime { domain: self, scope, parent })
    }

    fn revoke(&self, epoch: u64, release: bool) {
        // Drop/cancel cannot defer revocation because of an earlier poison.
        // Normal admission continues to report that poison, never erase it.
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let owner = if state.ordinary.as_ref().is_some_and(|owner| owner.id == epoch) {
            &mut state.ordinary
        } else {
            &mut state.active
        };
        if owner.as_ref().is_some_and(|owner| owner.id == epoch) {
            owner.as_mut().unwrap().revoked = true;
            if release {
                *owner = None;
            }
            state.frames.retain(|_, frame| frame.epoch != epoch);
        }
    }
}

/// Host lease. Retain the complete PreparedNativeSession in the future Run
/// driver/ActiveRun as well; this lease by itself intentionally owns no heap.
pub struct NativeContextRun {
    domain: Arc<NativeContextDomain>,
    root: NativeScope,
}
impl NativeContextRun {
    pub fn root_scope(&self) -> NativeScope {
        self.root
    }
    pub fn scoped<'a>(&'a self, scope: NativeScope, parent: &'a dyn ValueRuntime) -> Result<ScopedRuntime<'a>> {
        self.domain.scoped(scope, parent)
    }
    pub fn revoker(&self) -> NativeContextRevoker {
        NativeContextRevoker { domain: Arc::downgrade(&self.domain), epoch: self.root.epoch }
    }
}
impl Drop for NativeContextRun {
    fn drop(&mut self) {
        self.domain.revoke(self.root.epoch, true);
    }
}

#[derive(Clone)]
pub struct NativeContextRevoker {
    domain: Weak<NativeContextDomain>,
    epoch: u64,
}
impl NativeContextRevoker {
    pub fn revoke(&self) {
        if let Some(domain) = self.domain.upgrade() {
            domain.revoke(self.epoch, false);
        }
    }
}

/// Immutable frame layered over the exact synchronous callback parent.
pub struct ScopedRuntime<'a> {
    domain: &'a NativeContextDomain,
    scope: NativeScope,
    parent: &'a dyn ValueRuntime,
}
impl ValueRuntime for ScopedRuntime<'_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, effective)
            }
            request => self.parent.resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        if capability == CURRENT_FRAME {
            let heap = receiver.heap();
            self.domain.check_heap(heap)?;
            if heap.capture_deferred(receiver)? != self.domain.frame_port || !arguments.is_empty() {
                return Err(ValueError::UnsupportedOperation("Native current-frame port identity mismatch"));
            }
            return Ok(RuntimeReply::Value(heap.restore_deferred(&self.scope.key)?));
        }
        self.parent.resolve_call(capability, receiver, arguments, effective)
    }
    fn resolve_is_constructor(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        self.parent.resolve_is_constructor(capability, constructor, effective)
    }
    fn resolve_construct(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        self.parent.resolve_construct(capability, constructor, arguments, effective)
    }
    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        self.parent.resolve_construct_with_new_target(capability, constructor, new_target, arguments, effective)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        NativeContextDomain::frame(&*self.domain.lock()?, self.scope)?;
        self.parent.resolve_local_call(token, receiver, arguments, effective)
    }
}

fn value(reply: RuntimeReply) -> Result<ValueHandle> {
    match reply {
        RuntimeReply::Value(value) => Ok(value),
        _ => Err(ValueError::WrongRuntimeReply("Native context port must return a graph value")),
    }
}
static GET_STORE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da logger.ts:391,497,795,808 AsyncLocalStorage.getStore Host port",
    name: "getStore",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |_, _, receiver, arguments, runtime| value(runtime.resolve_call(GET_STORE, receiver, arguments, runtime)?),
};
static RUN_STORE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da logger.ts:818 AsyncLocalStorage.run Host port",
    name: "run",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |_, _, receiver, arguments, runtime| value(runtime.resolve_call(RUN_STORE, receiver, arguments, runtime)?),
};
