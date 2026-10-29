//! CLI-owned Promise graph and explicit single-driver queue foundation.
//! Fixed OMP596f2da logger.ts consumes native Promise and dynamic then.
//! No Host await/effect settlement or production RPC driver is installed here.

use super::{
    NativeErrorClasses, NativeRealm,
    context::{NativeContextDomain, NativeContextRevoker, NativeContextRun, NativeScope},
    parent::{
        NativeParentDomain, NativeParentId, NativeParentLease, NativeParentRevoker, NativeRunParentRegistry,
        NativeScopedParent,
    },
};
use ara_rpc::value_heap::{
    CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, DeferredValue, PropertyDescriptor, RuntimeReply,
    ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex, MutexGuard, Weak},
};
use tokio::sync::Notify;

mod algorithms;
type Result<T> = std::result::Result<T, ValueError>;
const CONSTRUCT: CapabilityId = CapabilityId(0x4152415f50524f43);
const CALL_ERROR: CapabilityId = CapabilityId(0x4152415f50524f45);
const THEN: CapabilityId = CapabilityId(0x4152415f50524f54);
const CATCH: CapabilityId = CapabilityId(0x4152415f50524348);
const RESOLVE: CapabilityId = CapabilityId(0x4152415f50524f52);
const REJECT: CapabilityId = CapabilityId(0x4152415f50524f4a);
const BIND: CapabilityId = CapabilityId(0x4152415f50524f42);
const STATIC_RESOLVE: CapabilityId = CapabilityId(0x4152415f50525352);
const STATIC_REJECT: CapabilityId = CapabilityId(0x4152415f5052534a);
const IS_PROMISE: CapabilityId = CapabilityId(0x4152415f50524953);

pub(super) fn claims(capability: CapabilityId) -> bool {
    matches!(
        capability,
        CONSTRUCT | CALL_ERROR | THEN | CATCH | RESOLVE | REJECT | BIND | STATIC_RESOLVE | STATIC_REJECT | IS_PROMISE
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativePromiseState {
    Pending,
    Fulfilled(DeferredValue),
    Rejected(DeferredValue),
}
struct Slot {
    epoch: u64,
    state: NativePromiseState,
    reactions: Vec<PendingReaction>,
}
#[derive(Clone, Copy)]
struct Pair {
    target: DeferredValue,
    already_resolved: bool,
}
#[derive(Clone, Copy)]
struct Capability {
    promise: DeferredValue,
    resolve: DeferredValue,
    reject: DeferredValue,
}
#[derive(Clone, Copy)]
struct Binding {
    epoch: u64,
    resolve: Option<DeferredValue>,
    reject: Option<DeferredValue>,
}
struct Reaction {
    epoch: u64,
    scope: NativeScope,
    parent: NativeParentLease,
    fulfill: Option<DeferredValue>,
    reject: Option<DeferredValue>,
    capability: Capability,
}
struct NativeAwaiter {
    epoch: u64,
    scope: NativeScope,
    parent: NativeParentLease,
    fulfill: DeferredValue,
    reject: DeferredValue,
}
enum PendingReaction {
    Promise(Reaction),
    Native(NativeAwaiter),
}
impl PendingReaction {
    fn epoch(&self) -> u64 {
        match self {
            Self::Promise(reaction) => reaction.epoch,
            Self::Native(awaiter) => awaiter.epoch,
        }
    }
}
struct Job {
    id: u64,
    epoch: u64,
    scope: NativeScope,
    parent: NativeParentId,
    kind: JobKind,
}
enum JobKind {
    NativeAwait { awaiter: NativeAwaiter, state: NativePromiseState },
    Reaction { reaction: Reaction, state: NativePromiseState },
    Thenable { parent: NativeParentLease, target: DeferredValue, thenable: DeferredValue, then: DeferredValue },
}
struct Epoch {
    id: u64,
    context_epoch: u64,
    parent_epoch: u64,
    parents: Weak<NativeRunParentRegistry>,
    context_revoker: NativeContextRevoker,
    parent_revoker: NativeParentRevoker,
    revoked: bool,
}
#[derive(Clone, Copy)]
struct Globals {
    constructor: DeferredValue,
    prototype: DeferredValue,
    errors: [DeferredValue; 3],
}
struct State {
    next_epoch: u64,
    next_job: u64,
    active: Option<Epoch>,
    ordinary: Option<Epoch>,
    globals: Option<Globals>,
    slots: HashMap<DeferredValue, Slot>,
    pairs: HashMap<DeferredValue, Pair>,
    bindings: HashMap<DeferredValue, Binding>,
    ready: VecDeque<Job>,
    failures: HashMap<u64, ValueError>,
}

/// All persistent graph roots are checked identities, never owning handles.
/// Weak context permits authentication without adding a realm/runtime lease.
pub struct NativePromiseDomain {
    heap_token: u64,
    port: DeferredValue,
    context: Weak<NativeContextDomain>,
    parent: Weak<NativeParentDomain>,
    state: Mutex<State>,
    wake: Arc<Notify>,
}

impl NativePromiseDomain {
    pub(super) fn new(
        heap: &Arc<ValueHeap>,
        context: &Arc<NativeContextDomain>,
        parent: &Arc<NativeParentDomain>,
    ) -> Result<Self> {
        Ok(Self {
            heap_token: heap.heap_token(),
            port: heap.capture_deferred(&heap.object()?)?,
            context: Arc::downgrade(context),
            parent: Arc::downgrade(parent),
            wake: Arc::new(Notify::new()),
            state: Mutex::new(State {
                next_epoch: 1,
                next_job: 1,
                active: None,
                ordinary: None,
                globals: None,
                slots: HashMap::new(),
                pairs: HashMap::new(),
                bindings: HashMap::new(),
                ready: VecDeque::new(),
                failures: HashMap::new(),
            }),
        })
    }
    fn lock(&self) -> Result<MutexGuard<'_, State>> {
        self.state.lock().map_err(|_| ValueError::UnsupportedOperation("Native Promise owner is poisoned"))
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
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        Ok(())
    }
    fn admit(&self, heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<(u64, NativeScope, NativeParentId)> {
        self.check_heap(heap)?;
        let context = self.context.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
        let scope = context
            .capture_scope(heap, runtime)?
            .ok_or(ValueError::UnsupportedOperation("Native Promise requires an admitted current Run frame"))?;
        let parent = self.parent.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?.capture(heap, runtime)?;
        let state = self.lock()?;
        let epoch = [state.active.as_ref(), state.ordinary.as_ref()]
            .into_iter()
            .flatten()
            .find(|owner| owner.context_epoch == scope.epoch && owner.parent_epoch == parent.epoch)
            .ok_or(ValueError::ExpiredCapability(CONSTRUCT))?
            .id;
        Self::active(&state, epoch)?;
        Ok((epoch, scope, parent))
    }
    fn retain_parent(&self, parent: NativeParentId) -> Result<NativeParentLease> {
        self.parent.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?.retain(parent)
    }
    fn globals(&self) -> Result<Globals> {
        self.lock()?.globals.ok_or(ValueError::UnsupportedOperation("Native Promise globals are not installed"))
    }
    fn type_error<T>(&self, heap: &Arc<ValueHeap>, message: &str, runtime: &dyn ValueRuntime) -> Result<T> {
        Err(ValueError::Thrown(self.error_value(heap, 0, message, runtime)?))
    }
    fn error_value(
        &self,
        heap: &Arc<ValueHeap>,
        kind: usize,
        message: &str,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let constructor = heap.restore_deferred(&self.globals()?.errors[kind])?;
        heap.construct(&constructor, &[heap.string(message.into())?], runtime)
    }
    fn reason(&self, heap: &Arc<ValueHeap>, error: ValueError, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
        match error {
            ValueError::Thrown(reason) => heap.adopt(&reason),
            ValueError::TypeError(message) => self.error_value(heap, 0, &message, runtime),
            ValueError::SyntaxError(message) => self.error_value(heap, 1, &message, runtime),
            ValueError::RangeError(message) => self.error_value(heap, 2, &message, runtime),
            other => Err(other),
        }
    }

    /// Actual node:util/types isPromise import for this Native realm. This
    /// reads the private native brand, never a public then/species/prototype.
    /// Ordinary objects and primitive values cannot forge that brand. Existing
    /// genuine Promise identity remains branded after Run revocation.
    pub fn prepare_is_promise(&self, heap: &Arc<ValueHeap>) -> Result<ValueHandle> {
        self.check_heap(heap)?;
        heap.fresh_compiled_function(&IS_PROMISE_BODY, &[heap.restore_deferred(&self.port)?])
    }

    pub(super) fn prepare_globals(
        &self,
        heap: &Arc<ValueHeap>,
        errors: &NativeErrorClasses,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        self.check_heap(heap)?;
        let port = heap.restore_deferred(&self.port)?;
        let constructor = heap.fresh_compiled_function(&CONSTRUCTOR, &[port.clone()])?;
        let prototype = heap.get(&constructor, &"prototype".into(), runtime)?;
        let then = heap.fresh_compiled_function(&THEN_BODY, &[])?;
        let catch = heap.fresh_compiled_function(&CATCH_BODY, &[])?;
        let species = heap.fresh_compiled_function(&SPECIES_BODY, &[])?;
        let ValueSnapshot::Function(species_target) = heap.snapshot(&species)? else {
            return Err(ValueError::InvalidNode);
        };
        heap.define_own(
            &constructor,
            heap.species_key(),
            PropertyDescriptor::Accessor {
                get: Some(species_target),
                set: None,
                enumerable: false,
                configurable: true,
            },
            runtime,
        )?;
        for (object, name, value) in [
            (&prototype, "then", then),
            (&prototype, "catch", catch),
            (&constructor, "resolve", heap.fresh_compiled_function(&STATIC_RESOLVE_BODY, &[port.clone()])?),
            (&constructor, "reject", heap.fresh_compiled_function(&STATIC_REJECT_BODY, &[port])?),
        ] {
            heap.define_own(
                object,
                name.into(),
                PropertyDescriptor::Data { value, writable: true, enumerable: false, configurable: true },
                runtime,
            )?;
        }
        let bindings = Globals {
            constructor: heap.capture_deferred(&constructor)?,
            prototype: heap.capture_deferred(&prototype)?,
            errors: [
                heap.capture_deferred(&errors.type_error)?,
                heap.capture_deferred(&errors.syntax_error)?,
                heap.capture_deferred(&errors.range_error)?,
            ],
        };
        let mut state = self.lock()?;
        if state.globals.is_some() {
            return Err(ValueError::UnsupportedOperation("Native Promise globals already installed"));
        }
        state.globals = Some(bindings);
        Ok(constructor)
    }

    pub fn snapshot(&self, heap: &Arc<ValueHeap>, promise: &ValueHandle) -> Result<NativePromiseState> {
        self.check_heap(heap)?;
        let key = heap.capture_deferred(promise)?;
        self.lock()?
            .slots
            .get(&key)
            .map(|slot| slot.state)
            .ok_or(ValueError::UnsupportedOperation("Value has no Native Promise brand"))
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }

    pub(super) fn begin(
        self: &Arc<Self>,
        realm: Arc<NativeRealm>,
        context: NativeContextRun,
    ) -> Result<NativePromiseRun> {
        self.begin_owned(realm, context, false)
    }

    pub(super) fn begin_display(
        self: &Arc<Self>,
        realm: Arc<NativeRealm>,
        context: NativeContextRun,
    ) -> Result<NativeDisplayCallbacks> {
        let driver = self.begin_owned(realm.clone(), context, true)?;
        let callbacks = crate::terminal::graph_renderer::CallbackDomain::new(&realm.values);
        realm.bind_display_callbacks(&callbacks)?;
        Ok(NativeDisplayCallbacks { driver, callbacks })
    }

    fn begin_owned(
        self: &Arc<Self>,
        realm: Arc<NativeRealm>,
        context: NativeContextRun,
        ordinary: bool,
    ) -> Result<NativePromiseRun> {
        self.check_heap(realm.values.heap())?;
        let parents = if ordinary {
            NativeRunParentRegistry::begin_display(realm.parent.clone(), realm.values.heap(), realm.runtime.clone())?
        } else {
            NativeRunParentRegistry::begin(realm.parent.clone(), realm.values.heap(), realm.runtime.clone())?
        };
        let root_parent = parents.root()?;
        let mut state = self.lock()?;
        if (if ordinary { &state.ordinary } else { &state.active }).is_some() {
            return Err(ValueError::UnsupportedOperation("Native Promise already has an exclusive driver"));
        }
        let epoch = state.next_epoch;
        state.next_epoch =
            epoch.checked_add(1).ok_or(ValueError::UnsupportedOperation("Native Promise epochs exhausted"))?;
        let owner = if ordinary { &mut state.ordinary } else { &mut state.active };
        *owner = Some(Epoch {
            id: epoch,
            context_epoch: context.root_scope().epoch,
            parent_epoch: root_parent.epoch,
            parents: Arc::downgrade(&parents),
            context_revoker: context.revoker(),
            parent_revoker: parents.revoker(),
            revoked: false,
        });
        Ok(NativePromiseRun { realm, domain: self.clone(), context, epoch, parents, root_parent })
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
            state.ready.retain(|job| job.epoch != epoch);
            for slot in state.slots.values_mut() {
                slot.reactions.retain(|reaction| reaction.epoch() != epoch);
            }
        }
        if release {
            state.failures.remove(&epoch);
        }
        drop(state);
        // The driver and native state observers share this notification. Wake
        // every registered waiter so an observer cannot consume the driver's
        // only wake, then retain the original permit for a later registration.
        self.wake.notify_waiters();
        self.wake.notify_one();
    }

    fn make_pair(
        &self,
        heap: &Arc<ValueHeap>,
        target: DeferredValue,
        epoch: u64,
    ) -> Result<(DeferredValue, ValueHandle, ValueHandle)> {
        let token_value = heap.object()?;
        let token = heap.capture_deferred(&token_value)?;
        let port = heap.restore_deferred(&self.port)?;
        let resolve = heap.fresh_compiled_function(&RESOLVE_BODY, &[port.clone(), token_value.clone()])?;
        let reject = heap.fresh_compiled_function(&REJECT_BODY, &[port, token_value])?;
        let mut state = self.lock()?;
        Self::active(&state, epoch)?;
        if !state.slots.contains_key(&target) {
            return Err(ValueError::InvalidNode);
        }
        state.pairs.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        state.pairs.insert(token, Pair { target, already_resolved: false });
        Ok((token, resolve, reject))
    }
    fn job(state: &mut State, epoch: u64, scope: NativeScope, kind: JobKind) -> Result<()> {
        Self::active(state, epoch)?;
        let next = state
            .next_job
            .checked_add(1)
            .ok_or(ValueError::UnsupportedOperation("Native Promise Job IDs exhausted"))?;
        state.ready.try_reserve(1).map_err(|_| ValueError::Allocation)?;
        let parent = match &kind {
            JobKind::Reaction { reaction, .. } => reaction.parent.id(),
            JobKind::NativeAwait { awaiter, .. } => awaiter.parent.id(),
            JobKind::Thenable { parent, .. } => parent.id(),
        };
        state.ready.push_back(Job { id: state.next_job, epoch, scope, parent, kind });
        state.next_job = next;
        Ok(())
    }
    fn settle(&self, target: DeferredValue, outcome: NativePromiseState, epoch: u64) -> Result<()> {
        if outcome == NativePromiseState::Pending {
            return Err(ValueError::InvalidNode);
        }
        let mut state = self.lock()?;
        Self::active(&state, epoch)?;
        let slot = state.slots.get(&target).ok_or(ValueError::InvalidNode)?;
        if slot.state != NativePromiseState::Pending {
            return Ok(());
        }
        if slot.epoch != epoch {
            return Err(ValueError::ExpiredCapability(RESOLVE));
        }
        let count = slot.reactions.len();
        state
            .next_job
            .checked_add(u64::try_from(count).map_err(|_| ValueError::Allocation)?)
            .ok_or(ValueError::UnsupportedOperation("Native Promise Job IDs exhausted"))?;
        state.ready.try_reserve(count).map_err(|_| ValueError::Allocation)?;
        let slot = state.slots.get_mut(&target).unwrap();
        slot.state = outcome;
        let reactions = std::mem::take(&mut slot.reactions);
        for reaction in reactions {
            match reaction {
                PendingReaction::Promise(reaction) => Self::job(
                    &mut state,
                    reaction.epoch,
                    reaction.scope,
                    JobKind::Reaction { reaction, state: outcome },
                )?,
                PendingReaction::Native(awaiter) => Self::job(
                    &mut state,
                    awaiter.epoch,
                    awaiter.scope,
                    JobKind::NativeAwait { awaiter, state: outcome },
                )?,
            }
        }
        drop(state);
        self.wake.notify_waiters();
        self.wake.notify_one();
        Ok(())
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
            if capability == THEN {
                return self.then(heap, receiver, arguments, effective).map(RuntimeReply::Value);
            }
            if capability == CATCH {
                return self.catch(heap, receiver, arguments, effective).map(RuntimeReply::Value);
            }
            if heap.capture_deferred(receiver)? != self.port {
                return Err(ValueError::UnsupportedOperation("Native Promise port identity mismatch"));
            }
            match capability {
                IS_PROMISE => {
                    let candidate = match arguments.first() {
                        Some(candidate) => candidate.clone(),
                        None => heap.undefined()?,
                    };
                    let edge = heap.capture_deferred(&candidate)?;
                    let branded = self.lock()?.slots.contains_key(&edge);
                    if !branded
                        && matches!(
                            heap.snapshot(&candidate)?,
                            ValueSnapshot::Opaque { kind: ara_rpc::value_heap::OpaqueKind::Object, .. }
                        )
                    {
                        // OpaqueKind::Object cannot distinguish native external
                        // Promise from an ordinary/proxy object. Do not falsely
                        // accept a classifier whose Host brand is unconnected.
                        return Err(ValueError::UnsupportedOperation("External opaque Promise brand is not connected"));
                    }
                    heap.boolean(branded).map(RuntimeReply::Value)
                }
                CALL_ERROR => self.type_error(heap, "Promise constructor cannot be invoked without new", effective),
                CONSTRUCT => self.construct_promise(heap, arguments, effective).map(RuntimeReply::Value),
                BIND => self.bind_capability(heap, arguments, effective).map(RuntimeReply::Value),
                RESOLVE | REJECT => {
                    self.resolve_pair(heap, arguments, capability == REJECT, effective).map(RuntimeReply::Value)
                }
                STATIC_RESOLVE | STATIC_REJECT => self
                    .static_capability(heap, arguments, capability == STATIC_REJECT, effective)
                    .map(RuntimeReply::Value),
                _ => Err(ValueError::MissingCapability(capability)),
            }
        })())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDrain {
    Idle,
    RanJob { id: u64 },
}

/// Foundation only: retains the full NativeRealm and exclusive context lease.
/// This is NOT the contracted production Prepared+HostEffectSettlement driver.
/// Callback graph code receives no mutable driver and cannot recursively drain.
pub struct NativePromiseRun {
    realm: Arc<NativeRealm>,
    domain: Arc<NativePromiseDomain>,
    context: NativeContextRun,
    epoch: u64,
    parents: Arc<NativeRunParentRegistry>,
    root_parent: NativeParentId,
}
/// Private native await identity. Never a graph property, serde row or public
/// then/species consumer; old-Run tickets cannot be imported into a new driver.
#[derive(Clone, Copy, Debug)]
pub struct NativePromiseTicket {
    promise: DeferredValue,
    epoch: u64,
}
/// Native timer notification only: a captured undefined value for one original
/// pending ticket. It carries no runtime, callback or owning graph handle.
/// Publication queues existing native awaiters; only the Run driver calls them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct NativeTimerTick {
    ticket: NativePromiseTicket,
    undefined: DeferredValue,
    heap_token: u64,
}
/// These handles belong on the driver/invocation owner, never a completion worker.
pub struct NativePendingPromise {
    pub promise: ValueHandle,
    pub resolve: ValueHandle,
    pub reject: ValueHandle,
    ticket: NativePromiseTicket,
}
impl NativePendingPromise {
    pub fn ticket(&self) -> NativePromiseTicket {
        self.ticket
    }
}
/// Shared synchronous admission only. No mutable driver, pump, or strong
/// realm/runtime/Invocation owner can escape through this handle.
#[derive(Clone)]
pub struct NativePromiseAdmission {
    realm: Weak<NativeRealm>,
    domain: Weak<NativePromiseDomain>,
    context: Weak<NativeContextDomain>,
    parents: Weak<NativeRunParentRegistry>,
    epoch: u64,
    root_scope: NativeScope,
    root_parent: NativeParentId,
    wake: Arc<Notify>,
}
impl NativePromiseAdmission {
    fn realm(&self) -> Result<Arc<NativeRealm>> {
        self.realm.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))
    }
    fn domain(&self) -> Result<Arc<NativePromiseDomain>> {
        let domain = self.domain.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
        NativePromiseDomain::active(&*domain.lock()?, self.epoch)?;
        Ok(domain)
    }
    fn parents(&self) -> Result<Arc<NativeRunParentRegistry>> {
        self.parents.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))
    }
    pub fn root_scope(&self) -> NativeScope {
        self.root_scope
    }
    pub fn root_parent(&self) -> NativeParentId {
        self.root_parent
    }
    pub fn check_live(&self) -> Result<()> {
        self.domain()?;
        self.parents()?.lookup(self.root_parent)?;
        Ok(())
    }
    pub fn check_values(&self, values: &ara_agent::SessionValues) -> Result<()> {
        self.check_live()?;
        let realm = self.realm()?;
        if !std::ptr::eq(realm.values.as_ref(), values) {
            return Err(ValueError::UnsupportedOperation("Native Run requires its exact original SessionValues"));
        }
        Ok(())
    }
    /// Register the real original InvocationRuntime Arc before its one execute
    /// Call. This metadata registration does not authorize effect admission.
    pub fn register_parent(&self, original: Arc<dyn ValueRuntime>) -> Result<NativeParentId> {
        self.check_live()?;
        self.parents()?.register_invocation(self.realm()?.values.heap(), original)
    }
    pub fn retain_parent(&self, parent: NativeParentId) -> Result<NativeParentLease> {
        self.check_live()?;
        self.parents()?.lookup(parent)?;
        self.realm()?.parent.retain(parent)
    }
    /// Only after authoritative execute-result and admitted updates have
    /// settled. Queued/pending/running reactions still retain the real parent.
    pub fn release_parent_registration(&self, parent: NativeParentId) -> Result<()> {
        self.check_live()?;
        self.parents()?.retire(parent)
    }
    pub fn capture_scope(&self, effective: &dyn ValueRuntime) -> Result<NativeScope> {
        self.check_live()?;
        let realm = self.realm()?;
        let context = self.context.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
        let scope = context
            .capture_scope(realm.values.heap(), effective)?
            .ok_or(ValueError::UnsupportedOperation("Native Run has no admitted current scope"))?;
        if scope.epoch != self.root_scope.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        Ok(scope)
    }
    pub fn capture_parent(&self, effective: &dyn ValueRuntime) -> Result<NativeParentId> {
        self.check_live()?;
        let realm = self.realm()?;
        let parent = realm.parent.capture(realm.values.heap(), effective)?;
        self.parents()?.lookup(parent)?;
        Ok(parent)
    }
    pub fn resolve_parent_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        parent: NativeParentId,
        _effective: &dyn ValueRuntime,
    ) -> Option<Result<RuntimeReply>> {
        if !super::parent::claims(capability) {
            return None;
        }
        Some((|| {
            self.check_live()?;
            self.parents()?.lookup(parent)?;
            self.realm()?
                .parent
                .resolve_registered(capability, receiver, arguments, parent)
                .ok_or(ValueError::WrongRuntimeReply("Native registered parent port was not claimed"))?
        })())
    }
    /// Synchronous adapter only; never store it in that InvocationRuntime's
    /// own state or in an OS worker, because it temporarily clones its Arc.
    pub fn scoped(&self, scope: NativeScope, parent: NativeParentId) -> Result<NativeScopedParent> {
        self.check_live()?;
        if scope.epoch != self.root_scope.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        let context = self.context.upgrade().ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
        NativeScopedParent::new(context, scope, self.parents()?.as_ref(), parent)
    }
    pub fn wake(&self) -> Arc<Notify> {
        self.wake.clone()
    }
    /// Native await continuation after the ticket's original Promise adoption.
    /// No public then/species Gets or extra public Promise capability.
    pub fn attach_native_await(
        &self,
        ticket: &NativePromiseTicket,
        on_fulfilled: &ValueHandle,
        on_rejected: &ValueHandle,
        scope: NativeScope,
        parent: NativeParentId,
    ) -> Result<()> {
        self.check_live()?;
        if ticket.epoch != self.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        let realm = self.realm()?;
        let heap = realm.values.heap();
        heap.restore_deferred(&ticket.promise)?;
        self.scoped(scope, parent)?;
        for handler in [on_fulfilled, on_rejected] {
            realm.values.check_handle(handler)?;
            if !matches!(
                heap.snapshot(handler)?,
                ValueSnapshot::Function(ara_rpc::value_heap::CallableTarget::Compiled(_))
            ) {
                return Err(ValueError::UnsupportedOperation("Native await needs its persistent compiled handlers"));
            }
        }
        let awaiter = NativeAwaiter {
            epoch: self.epoch,
            scope,
            parent: self.retain_parent(parent)?,
            fulfill: heap.capture_deferred(on_fulfilled)?,
            reject: heap.capture_deferred(on_rejected)?,
        };
        let domain = self.domain()?;
        {
            let mut state = domain.lock()?;
            NativePromiseDomain::active(&state, self.epoch)?;
            let slot = state.slots.get_mut(&ticket.promise).ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
            if slot.epoch != ticket.epoch {
                return Err(ValueError::ExpiredCapability(CONSTRUCT));
            }
            if slot.state == NativePromiseState::Pending {
                slot.reactions.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                slot.reactions.push(PendingReaction::Native(awaiter));
            } else {
                let outcome = slot.state;
                NativePromiseDomain::job(
                    &mut state,
                    self.epoch,
                    scope,
                    JobKind::NativeAwait { awaiter, state: outcome },
                )?;
            }
        }
        domain.wake.notify_waiters();
        domain.wake.notify_one();
        Ok(())
    }
    /// Calls no public constructor/executor/then/species. A Host invocation
    /// must create separate kernel and authoritative execute-result slots.
    pub fn prepare_pending(&self, scope: NativeScope, parent: NativeParentId) -> Result<NativePendingPromise> {
        let runtime = self.scoped(scope, parent)?;
        let realm = self.realm()?;
        let domain = self.domain()?;
        let heap = realm.values.heap();
        let (epoch, _, _) = domain.admit(heap, &runtime)?;
        let prototype = heap.restore_deferred(&domain.globals()?.prototype)?;
        let promise = heap.object_with_prototype(Some(&prototype))?;
        let edge = heap.capture_deferred(&promise)?;
        {
            let mut state = domain.lock()?;
            NativePromiseDomain::active(&state, epoch)?;
            state.slots.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            state.slots.insert(edge, Slot { epoch, state: NativePromiseState::Pending, reactions: Vec::new() });
        }
        let (_, resolve, reject) = domain.make_pair(heap, edge, epoch)?;
        Ok(NativePendingPromise { promise, resolve, reject, ticket: NativePromiseTicket { promise: edge, epoch } })
    }
    /// Prepare synchronously for a fresh timer ticket after its compiled native
    /// awaiters have been attached. This captures only the intrinsic undefined;
    /// arbitrary Promise results still use the original adoption machinery.
    pub(crate) fn prepare_timer_tick(&self, ticket: &NativePromiseTicket) -> Result<NativeTimerTick> {
        if self.ticket_state(ticket)? != NativePromiseState::Pending {
            return Err(ValueError::InvalidNode);
        }
        let realm = self.realm()?;
        let heap = realm.values.heap();
        let domain = self.domain()?;
        domain.check_heap(heap)?;
        let undefined = heap.capture_deferred(&heap.undefined()?)?;
        Ok(NativeTimerTick { ticket: *ticket, undefined, heap_token: heap.heap_token() })
    }
    /// A timer worker may publish only this pre-captured tick. Clearing still
    /// publishes the tick so the original driver can release the native awaiter
    /// lease; the Host compiled wrapper suppresses its cancelled callback.
    /// No graph Call, thenable adoption or Promise queue drain occurs here.
    pub(crate) fn publish_timer_tick(&self, tick: &NativeTimerTick) -> Result<()> {
        self.check_live()?;
        if tick.ticket.epoch != self.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        let realm = self.realm()?;
        let heap = realm.values.heap();
        if heap.heap_token() != tick.heap_token {
            return Err(ValueError::ForeignHeap);
        }
        heap.restore_deferred(&tick.ticket.promise)?;
        heap.restore_deferred(&tick.undefined)?;
        let domain = self.domain()?;
        domain.check_heap(heap)?;
        {
            let state = domain.lock()?;
            NativePromiseDomain::active(&state, self.epoch)?;
            let slot = state.slots.get(&tick.ticket.promise).ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
            if slot.epoch != tick.ticket.epoch {
                return Err(ValueError::ExpiredCapability(CONSTRUCT));
            }
        }
        // settle repeats the active/slot checks under its mutation lock, so a
        // revocation between the guard and publication cannot queue a new Run.
        domain.settle(tick.ticket.promise, NativePromiseState::Fulfilled(tick.undefined), self.epoch)
    }
    pub fn ticket_state(&self, ticket: &NativePromiseTicket) -> Result<NativePromiseState> {
        self.check_live()?;
        if ticket.epoch != self.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        let realm = self.realm()?;
        realm.values.heap().restore_deferred(&ticket.promise)?;
        let domain = self.domain()?;
        let state = domain.lock()?;
        NativePromiseDomain::active(&state, self.epoch)?;
        let slot = state.slots.get(&ticket.promise).ok_or(ValueError::ExpiredCapability(CONSTRUCT))?;
        if slot.epoch != ticket.epoch {
            return Err(ValueError::ExpiredCapability(CONSTRUCT));
        }
        Ok(slot.state)
    }
}

impl NativePromiseRun {
    pub fn root_scope(&self) -> NativeScope {
        self.context.root_scope()
    }
    pub fn root_parent(&self) -> NativeParentId {
        self.root_parent
    }
    pub fn admission(&self) -> NativePromiseAdmission {
        NativePromiseAdmission {
            realm: Arc::downgrade(&self.realm),
            domain: Arc::downgrade(&self.domain),
            context: Arc::downgrade(self.realm.context()),
            parents: Arc::downgrade(&self.parents),
            epoch: self.epoch,
            root_scope: self.root_scope(),
            root_parent: self.root_parent,
            wake: self.domain.wake(),
        }
    }
    pub fn scoped(&self, scope: NativeScope, parent: NativeParentId) -> Result<NativeScopedParent> {
        self.admission().scoped(scope, parent)
    }
    /// Register/enable a wake future BEFORE inspecting/draining ready work.
    pub fn wake(&self) -> Arc<Notify> {
        self.domain.wake()
    }
    pub fn revoker(&self) -> NativePromiseRevoker {
        NativePromiseRevoker {
            domain: Arc::downgrade(&self.domain),
            epoch: self.epoch,
            context: self.context.revoker(),
            parent: self.parents.revoker(),
        }
    }
    pub fn prepare_pending(&self, scope: NativeScope, parent: NativeParentId) -> Result<NativePendingPromise> {
        self.admission().prepare_pending(scope, parent)
    }
    pub fn ticket_state(&self, ticket: &NativePromiseTicket) -> Result<NativePromiseState> {
        self.admission().ticket_state(ticket)
    }
    /// The only mutable pump; no supplied/default parent can hide a missing
    /// Invocation owner. Every job authenticates its own registered parent.
    pub fn drain_one(&mut self) -> Result<NativeDrain> {
        let (job, parents, context_revoker, parent_revoker) = {
            let mut state = self.domain.lock()?;
            if let Some(error) = state.failures.remove(&self.epoch) {
                return Err(error);
            }
            NativePromiseDomain::active(&state, self.epoch)?;
            let Some(head) = state.ready.front() else { return Ok(NativeDrain::Idle) };
            let owner = [state.active.as_ref(), state.ordinary.as_ref()]
                .into_iter()
                .flatten()
                .find(|owner| owner.id == head.epoch && !owner.revoked)
                .ok_or(ValueError::ExpiredCapability(THEN))?;
            if owner.context_epoch != head.scope.epoch || owner.parent_epoch != head.parent.epoch {
                return Err(ValueError::ExpiredCapability(THEN));
            }
            let parents = owner.parents.upgrade().ok_or(ValueError::ExpiredCapability(THEN))?;
            let context_revoker = owner.context_revoker.clone();
            let parent_revoker = owner.parent_revoker.clone();
            (state.ready.pop_front().unwrap(), parents, context_revoker, parent_revoker)
        };
        let id = job.id;
        let epoch = job.epoch;
        let outcome = (|| {
            let runtime =
                NativeScopedParent::new(self.realm.context().clone(), job.scope, parents.as_ref(), job.parent)?;
            self.domain.execute_job(self.realm.values.heap(), job.kind, &runtime)
        })();
        match outcome {
            Ok(()) => {
                parents.collect()?;
                Ok(NativeDrain::RanJob { id })
            }
            Err(error) => {
                context_revoker.revoke();
                parent_revoker.revoke();
                self.domain.revoke(epoch, false);
                if epoch == self.epoch {
                    Err(error)
                } else {
                    self.domain.lock()?.failures.insert(epoch, error);
                    self.domain.wake.notify_waiters();
                    self.domain.wake.notify_one();
                    Ok(NativeDrain::RanJob { id })
                }
            }
        }
    }
}

/// Reference-Host Session callback owner. No Agent Run, tool grant or native
/// await ticket is exposed. The main-task display evaluator is its only pump.
pub(crate) struct NativeDisplayCallbacks {
    driver: NativePromiseRun,
    callbacks: Arc<crate::terminal::graph_renderer::CallbackDomain>,
}
impl NativeDisplayCallbacks {
    pub(crate) fn domain(&self) -> Arc<crate::terminal::graph_renderer::CallbackDomain> {
        self.callbacks.clone()
    }
    pub(crate) fn runtime(&self) -> Result<NativeScopedParent> {
        self.driver.scoped(self.driver.root_scope(), self.driver.root_parent())
    }
    pub(crate) fn wake(&self) -> Arc<Notify> {
        self.driver.wake()
    }
    pub(crate) fn drain_one(&mut self) -> Result<NativeDrain> {
        self.driver.drain_one()
    }
}

impl Drop for NativeDisplayCallbacks {
    fn drop(&mut self) {
        self.callbacks.close();
    }
}

impl Drop for NativePromiseRun {
    fn drop(&mut self) {
        self.parents.revoker().revoke();
        self.domain.revoke(self.epoch, true);
    }
}
#[derive(Clone)]
pub struct NativePromiseRevoker {
    domain: Weak<NativePromiseDomain>,
    epoch: u64,
    context: NativeContextRevoker,
    parent: NativeParentRevoker,
}
impl NativePromiseRevoker {
    pub fn revoke(&self) {
        if let Some(domain) = self.domain.upgrade() {
            domain.revoke(self.epoch, false);
        }
        self.context.revoke();
        self.parent.revoke();
    }
}

fn value(reply: RuntimeReply) -> Result<ValueHandle> {
    match reply {
        RuntimeReply::Value(value) => Ok(value),
        _ => Err(ValueError::WrongRuntimeReply("Native Promise port needs a graph value")),
    }
}
static CONSTRUCTOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise constructor with actual executor/resolver-pair graph; OMP596f2da logger Promise consumer",
    name: "Promise",
    length: 1,
    form: CompiledFunctionForm::NativeConstructor(construct_body),
    capture_count: 1,
    body: |_, captures, _, _, runtime| value(runtime.resolve_call(CALL_ERROR, &captures[0], &[], runtime)?),
};
fn construct_body(
    _: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut args = Vec::new();
    args.try_reserve(1 + arguments.len()).map_err(|_| ValueError::Allocation)?;
    args.push(receiver.clone());
    args.extend_from_slice(arguments);
    value(runtime.resolve_call(CONSTRUCT, &captures[0], &args, runtime)?)
}
static THEN_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise.prototype.then brand/SpeciesConstructor/NewPromiseCapability/register",
    name: "then",
    length: 2,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |_, _, receiver, arguments, runtime| value(runtime.resolve_call(THEN, receiver, arguments, runtime)?),
};
static CATCH_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise.prototype.catch dynamic then invocation",
    name: "catch",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |_, _, receiver, arguments, runtime| value(runtime.resolve_call(CATCH, receiver, arguments, runtime)?),
};
static SPECIES_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise constructor @@species getter returns this",
    name: "get [Symbol.species]",
    length: 0,
    form: CompiledFunctionForm::Builtin,
    capture_count: 0,
    body: |_, _, receiver, _, _| Ok(receiver.clone()),
};
static RESOLVE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA persistent Promise resolving function with private shared AlreadyResolved pair",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, arguments, runtime| {
        let value_arg = arguments.first().cloned().unwrap_or(heap.undefined()?);
        value(runtime.resolve_call(RESOLVE, &captures[0], &[captures[1].clone(), value_arg], runtime)?)
    },
};
static REJECT_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA persistent Promise rejection function sharing the resolving pair",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, arguments, runtime| {
        let value_arg = arguments.first().cloned().unwrap_or(heap.undefined()?);
        value(runtime.resolve_call(REJECT, &captures[0], &[captures[1].clone(), value_arg], runtime)?)
    },
};
static CAPABILITY_EXECUTOR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA persistent NewPromiseCapability executor with private exact resolve/reject binding",
    name: "",
    length: 2,
    form: CompiledFunctionForm::Arrow,
    capture_count: 2,
    body: |heap, captures, _, arguments, runtime| {
        let resolve = arguments.first().cloned().unwrap_or(heap.undefined()?);
        let reject = arguments.get(1).cloned().unwrap_or(heap.undefined()?);
        value(runtime.resolve_call(BIND, &captures[0], &[captures[1].clone(), resolve, reject], runtime)?)
    },
};
static STATIC_RESOLVE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise.resolve dynamic constructor check and actual capability resolution",
    name: "resolve",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, receiver, arguments, runtime| {
        let argument = arguments.first().cloned().unwrap_or(heap.undefined()?);
        value(runtime.resolve_call(STATIC_RESOLVE, &captures[0], &[receiver.clone(), argument], runtime)?)
    },
};
static STATIC_REJECT_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "ECMA Promise.reject actual NewPromiseCapability and rejection",
    name: "reject",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, receiver, arguments, runtime| {
        let argument = arguments.first().cloned().unwrap_or(heap.undefined()?);
        value(runtime.resolve_call(STATIC_REJECT, &captures[0], &[receiver.clone(), argument], runtime)?)
    },
};

static IS_PROMISE_BODY: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "node:util/types genuine Native Promise brand; OMP596f2da logger.ts:819",
    name: "isPromise",
    length: 1,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, _, arguments, runtime| {
        let argument = match arguments.first() {
            Some(argument) => argument.clone(),
            None => heap.undefined()?,
        };
        value(runtime.resolve_call(IS_PROMISE, &captures[0], &[argument], runtime)?)
    },
};
