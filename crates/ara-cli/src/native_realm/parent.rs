//! CLI-private authentication of the exact synchronous parent of queued work.
//! Graph/domain metadata contains checked identities, never runtime/heap owners.

use super::context::{NativeContextDomain, NativeScope};
use ara_rpc::value_heap::{
    CapabilityId, DeferredValue, RuntimeReply, RuntimeRequest, ValueError, ValueHandle, ValueHeap, ValueRuntime,
    ValueSnapshot,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
};

type Result<T> = std::result::Result<T, ValueError>;
const CURRENT_PARENT: CapabilityId = CapabilityId(0x4152415f5041524e);

pub(super) fn claims(capability: CapabilityId) -> bool {
    capability == CURRENT_PARENT
}

/// No raw-ID, graph-property or serialized constructor grants this authority.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NativeParentId {
    pub(super) epoch: u64,
    serial: u64,
    key: DeferredValue,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Root,
    Invocation,
}
struct Frame {
    id: NativeParentId,
    kind: Kind,
    references: usize,
    retiring: bool,
}
struct Epoch {
    id: u64,
    revoked: bool,
}
struct State {
    next_epoch: u64,
    next_frame: u64,
    active: Option<Epoch>,
    ordinary: Option<Epoch>,
    frames: HashMap<DeferredValue, Frame>,
}

pub struct NativeParentDomain {
    heap_token: u64,
    port: DeferredValue,
    state: Mutex<State>,
}
impl NativeParentDomain {
    pub(super) fn new(heap: &Arc<ValueHeap>) -> Result<Self> {
        Ok(Self {
            heap_token: heap.heap_token(),
            port: heap.capture_deferred(&heap.object()?)?,
            state: Mutex::new(State {
                next_epoch: 1,
                next_frame: 1,
                active: None,
                ordinary: None,
                frames: HashMap::new(),
            }),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| ValueError::UnsupportedOperation("Native parent domain is poisoned"))
    }
    fn check_heap(&self, heap: &ValueHeap) -> Result<()> {
        if heap.heap_token() != self.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        Ok(())
    }
    fn active(state: &State, epoch: u64) -> Result<()> {
        if ![state.active.as_ref(), state.ordinary.as_ref()]
            .into_iter()
            .flatten()
            .any(|owner| owner.id == epoch && !owner.revoked)
        {
            return Err(ValueError::ExpiredCapability(CURRENT_PARENT));
        }
        Ok(())
    }
    fn frame(state: &State, id: NativeParentId) -> Result<&Frame> {
        Self::active(state, id.epoch)?;
        let frame = state.frames.get(&id.key).ok_or(ValueError::ExpiredCapability(CURRENT_PARENT))?;
        if frame.id != id {
            return Err(ValueError::ExpiredCapability(CURRENT_PARENT));
        }
        if frame.retiring && frame.references == 0 {
            return Err(ValueError::ExpiredCapability(CURRENT_PARENT));
        }
        Ok(frame)
    }
    fn check(&self, id: NativeParentId) -> Result<()> {
        Self::frame(&*self.lock()?, id)?;
        Ok(())
    }
    fn begin(&self, ordinary: bool) -> Result<u64> {
        let mut state = self.lock()?;
        if (if ordinary { &state.ordinary } else { &state.active }).is_some() {
            return Err(ValueError::UnsupportedOperation("Native parent registry already has an exclusive Run"));
        }
        let epoch = state.next_epoch;
        state.next_epoch =
            epoch.checked_add(1).ok_or(ValueError::UnsupportedOperation("Native parent epochs exhausted"))?;
        let owner = if ordinary { &mut state.ordinary } else { &mut state.active };
        *owner = Some(Epoch { id: epoch, revoked: false });
        Ok(epoch)
    }
    pub(super) fn capture(&self, heap: &Arc<ValueHeap>, effective: &dyn ValueRuntime) -> Result<NativeParentId> {
        self.check_heap(heap)?;
        let port = heap.restore_deferred(&self.port)?;
        let reply = effective.resolve_call(CURRENT_PARENT, &port, &[], effective)?;
        let RuntimeReply::Value(current) = reply else {
            return Err(ValueError::WrongRuntimeReply("Native parent port needs a checked graph edge"));
        };
        if matches!(heap.snapshot(&current)?, ValueSnapshot::Undefined) {
            return Err(ValueError::UnsupportedOperation("Native Promise has no registered original parent"));
        }
        let key = heap.capture_deferred(&current)?;
        let state = self.lock()?;
        let id = state.frames.get(&key).ok_or(ValueError::ExpiredCapability(CURRENT_PARENT))?.id;
        Self::frame(&state, id)?;
        Ok(id)
    }
    pub(super) fn resolve_base(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
    ) -> Option<Result<RuntimeReply>> {
        if !claims(capability) {
            return None;
        }
        Some((|| {
            self.check_port(receiver, arguments)?;
            Ok(RuntimeReply::Value(receiver.heap().undefined()?))
        })())
    }
    fn check_port(&self, receiver: &ValueHandle, arguments: &[ValueHandle]) -> Result<()> {
        self.check_heap(receiver.heap())?;
        if receiver.heap().capture_deferred(receiver)? != self.port || !arguments.is_empty() {
            return Err(ValueError::UnsupportedOperation("Native parent port identity mismatch"));
        }
        Ok(())
    }
    pub(super) fn resolve_registered(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        id: NativeParentId,
    ) -> Option<Result<RuntimeReply>> {
        if !claims(capability) {
            return None;
        }
        Some((|| {
            self.check_port(receiver, arguments)?;
            self.check(id)?;
            Ok(RuntimeReply::Value(receiver.heap().restore_deferred(&id.key)?))
        })())
    }
    pub(super) fn retain(self: &Arc<Self>, id: NativeParentId) -> Result<NativeParentLease> {
        let mut state = self.lock()?;
        Self::frame(&state, id)?;
        let frame = state.frames.get_mut(&id.key).unwrap();
        frame.references = frame
            .references
            .checked_add(1)
            .ok_or(ValueError::UnsupportedOperation("Native parent references exhausted"))?;
        Ok(NativeParentLease { domain: Arc::downgrade(self), id })
    }
    fn revoke(&self, epoch: u64, release: bool) {
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
            state.frames.retain(|_, frame| frame.id.epoch != epoch);
        }
    }
}

/// A counted metadata reference only; it never retains an InvocationRuntime.
pub struct NativeParentLease {
    domain: Weak<NativeParentDomain>,
    id: NativeParentId,
}
impl NativeParentLease {
    pub fn id(&self) -> NativeParentId {
        self.id
    }
}
impl Drop for NativeParentLease {
    fn drop(&mut self) {
        let Some(domain) = self.domain.upgrade() else { return };
        let mut state = domain.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(frame) = state.frames.get_mut(&self.id.key).filter(|frame| frame.id == self.id) {
            if let Some(next) = frame.references.checked_sub(1) {
                frame.references = next;
            } else if let Some(active) = if state.ordinary.as_ref().is_some_and(|owner| owner.id == self.id.epoch) {
                state.ordinary.as_mut()
            } else {
                state.active.as_mut()
            } {
                // An impossible ownership mismatch cannot permit more graph work.
                active.revoked = true;
            }
        }
    }
}

/// The Run, not the graph domain, strongly owns the exact original runtime Arcs.
pub(super) struct NativeRunParentRegistry {
    domain: Arc<NativeParentDomain>,
    epoch: u64,
    root: OnceLock<NativeParentId>,
    owners: Mutex<HashMap<NativeParentId, Arc<dyn ValueRuntime>>>,
}
impl NativeRunParentRegistry {
    pub(super) fn begin(
        domain: Arc<NativeParentDomain>,
        heap: &Arc<ValueHeap>,
        root: Arc<dyn ValueRuntime>,
    ) -> Result<Arc<Self>> {
        domain.check_heap(heap)?;
        Self::begin_owned(domain, heap, root, false)
    }
    pub(super) fn begin_display(
        domain: Arc<NativeParentDomain>,
        heap: &Arc<ValueHeap>,
        root: Arc<dyn ValueRuntime>,
    ) -> Result<Arc<Self>> {
        Self::begin_owned(domain, heap, root, true)
    }
    fn begin_owned(
        domain: Arc<NativeParentDomain>,
        heap: &Arc<ValueHeap>,
        root: Arc<dyn ValueRuntime>,
        ordinary: bool,
    ) -> Result<Arc<Self>> {
        domain.check_heap(heap)?;
        let epoch = domain.begin(ordinary)?;
        let registry = Arc::new(Self { domain, epoch, root: OnceLock::new(), owners: Mutex::new(HashMap::new()) });
        let id = registry.register(heap, root, Kind::Root)?;
        registry.root.set(id).map_err(|_| ValueError::SourceIdentityCollision)?;
        Ok(registry)
    }
    pub(super) fn root(&self) -> Result<NativeParentId> {
        self.root.get().copied().ok_or(ValueError::UnsupportedOperation("Native root was not registered"))
    }
    fn register(&self, heap: &Arc<ValueHeap>, runtime: Arc<dyn ValueRuntime>, kind: Kind) -> Result<NativeParentId> {
        self.domain.check_heap(heap)?;
        // Allocate before taking metadata/owner locks; publish an ID only with its real Arc.
        let key = heap.capture_deferred(&heap.object()?)?;
        let mut owners =
            self.owners.lock().map_err(|_| ValueError::UnsupportedOperation("Native parent registry poisoned"))?;
        let mut state = self.domain.lock()?;
        NativeParentDomain::active(&state, self.epoch)?;
        if owners.values().any(|owner| Arc::ptr_eq(owner, &runtime)) {
            return Err(ValueError::SourceIdentityCollision);
        }
        owners.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        state.frames.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let serial = state.next_frame;
        let next =
            serial.checked_add(1).ok_or(ValueError::UnsupportedOperation("Native parent frame IDs exhausted"))?;
        let id = NativeParentId { epoch: self.epoch, serial, key };
        owners.insert(id, runtime);
        state.frames.insert(key, Frame { id, kind, references: 0, retiring: false });
        state.next_frame = next;
        Ok(id)
    }
    pub(super) fn register_invocation(
        &self,
        heap: &Arc<ValueHeap>,
        runtime: Arc<dyn ValueRuntime>,
    ) -> Result<NativeParentId> {
        self.register(heap, runtime, Kind::Invocation)
    }
    pub(super) fn lookup(&self, id: NativeParentId) -> Result<NativeParentRuntime> {
        let owners =
            self.owners.lock().map_err(|_| ValueError::UnsupportedOperation("Native parent registry poisoned"))?;
        let state = self.domain.lock()?;
        if id.epoch != self.epoch {
            return Err(ValueError::ExpiredCapability(CURRENT_PARENT));
        }
        NativeParentDomain::frame(&state, id)?;
        let original = owners.get(&id).cloned().ok_or(ValueError::ExpiredCapability(CURRENT_PARENT))?;
        Ok(NativeParentRuntime { domain: Arc::downgrade(&self.domain), id, original })
    }
    pub(super) fn retire(&self, id: NativeParentId) -> Result<()> {
        {
            let mut state = self.domain.lock()?;
            let frame = NativeParentDomain::frame(&state, id)?;
            if frame.kind == Kind::Root {
                return Err(ValueError::UnsupportedOperation("Native root closes with its Run"));
            }
            state.frames.get_mut(&id.key).unwrap().retiring = true;
        }
        self.collect()
    }
    pub(super) fn collect(&self) -> Result<()> {
        let mut removed = Vec::new();
        {
            let mut owners =
                self.owners.lock().map_err(|_| ValueError::UnsupportedOperation("Native parent registry poisoned"))?;
            let mut state = self.domain.lock()?;
            NativeParentDomain::active(&state, self.epoch)?;
            let ids = state
                .frames
                .values()
                .filter(|frame| frame.id.epoch == self.epoch && frame.retiring && frame.references == 0)
                .map(|frame| frame.id)
                .collect::<Vec<_>>();
            for id in ids {
                state.frames.remove(&id.key);
                if let Some(owner) = owners.remove(&id) {
                    removed.push(owner);
                }
            }
        }
        // Original runtime destructors run outside all registry/domain locks.
        drop(removed);
        Ok(())
    }
    pub(super) fn revoker(&self) -> NativeParentRevoker {
        NativeParentRevoker { domain: Arc::downgrade(&self.domain), epoch: self.epoch }
    }
}
impl Drop for NativeRunParentRegistry {
    fn drop(&mut self) {
        self.domain.revoke(self.epoch, true);
    }
}
#[derive(Clone)]
pub(super) struct NativeParentRevoker {
    domain: Weak<NativeParentDomain>,
    epoch: u64,
}
impl NativeParentRevoker {
    pub(super) fn revoke(&self) {
        if let Some(domain) = self.domain.upgrade() {
            domain.revoke(self.epoch, false);
        }
    }
}

pub(super) struct NativeParentRuntime {
    domain: Weak<NativeParentDomain>,
    id: NativeParentId,
    original: Arc<dyn ValueRuntime>,
}
impl NativeParentRuntime {
    fn checked(&self) -> Result<Arc<NativeParentDomain>> {
        let domain = self.domain.upgrade().ok_or(ValueError::ExpiredCapability(CURRENT_PARENT))?;
        domain.check(self.id)?;
        Ok(domain)
    }
}
impl ValueRuntime for NativeParentRuntime {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
        self.checked()?;
        match request {
            RuntimeRequest::Call { capability, receiver, arguments } => {
                self.resolve_call(capability, receiver, arguments, effective)
            }
            request => self.original.resolve_effective(request, effective),
        }
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        let domain = self.checked()?;
        if let Some(reply) = domain.resolve_registered(capability, receiver, arguments, self.id) {
            return reply;
        }
        self.original.resolve_call(capability, receiver, arguments, effective)
    }
    fn resolve_is_constructor(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        self.checked()?;
        self.original.resolve_is_constructor(capability, constructor, effective)
    }
    fn resolve_construct(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.checked()?;
        self.original.resolve_construct(capability, constructor, arguments, effective)
    }
    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.checked()?;
        self.original.resolve_construct_with_new_target(capability, constructor, new_target, arguments, effective)
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.checked()?;
        self.original.resolve_local_call(token, receiver, arguments, effective)
    }
}

/// Synchronous owned adapter over the registered original parent. Never store
/// this in that parent's own state or send it to an OS completion worker.
pub struct NativeScopedParent {
    context: Arc<NativeContextDomain>,
    scope: NativeScope,
    parent: NativeParentRuntime,
}
impl NativeScopedParent {
    pub(super) fn new(
        context: Arc<NativeContextDomain>,
        scope: NativeScope,
        registry: &NativeRunParentRegistry,
        id: NativeParentId,
    ) -> Result<Self> {
        let parent = registry.lookup(id)?;
        context.scoped(scope, &parent)?;
        Ok(Self { context, scope, parent })
    }
}
impl ValueRuntime for NativeScopedParent {
    fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply> {
        self.resolve_effective(request, self)
    }
    fn resolve_effective(&self, request: RuntimeRequest<'_>, effective: &dyn ValueRuntime) -> Result<RuntimeReply> {
        self.context.scoped(self.scope, &self.parent)?.resolve_effective(request, effective)
    }
    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.context.scoped(self.scope, &self.parent)?.resolve_call(capability, receiver, arguments, effective)
    }
    fn resolve_is_constructor(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        effective: &dyn ValueRuntime,
    ) -> Result<bool> {
        self.context.scoped(self.scope, &self.parent)?.resolve_is_constructor(capability, constructor, effective)
    }
    fn resolve_construct(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.context.scoped(self.scope, &self.parent)?.resolve_construct(capability, constructor, arguments, effective)
    }
    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.context.scoped(self.scope, &self.parent)?.resolve_construct_with_new_target(
            capability,
            constructor,
            new_target,
            arguments,
            effective,
        )
    }
    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective: &dyn ValueRuntime,
    ) -> Result<RuntimeReply> {
        self.context.scoped(self.scope, &self.parent)?.resolve_local_call(token, receiver, arguments, effective)
    }
}
