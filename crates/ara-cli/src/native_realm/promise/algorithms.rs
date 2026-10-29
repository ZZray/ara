//! Actual selected Promise algorithms. Every supplied Get/Call/construct runs
//! after domain guards have dropped; only original native exceptions reject.

use super::*;

impl NativePromiseDomain {
    pub(super) fn catch(
        &self,
        heap: &Arc<ValueHeap>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let then = self.original(heap, heap.get(receiver, &"then".into(), runtime), runtime)?;
        let rejected = arguments.first().cloned().unwrap_or(heap.undefined()?);
        self.original(heap, heap.call(&then, receiver, &[heap.undefined()?, rejected], runtime), runtime)
    }
    fn original<T>(&self, heap: &Arc<ValueHeap>, outcome: Result<T>, runtime: &dyn ValueRuntime) -> Result<T> {
        outcome.map_err(|error| match self.reason(heap, error, runtime) {
            Ok(reason) => ValueError::Thrown(reason),
            Err(error) => error,
        })
    }
    pub(super) fn construct_promise(
        &self,
        heap: &Arc<ValueHeap>,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let (epoch, _, _) = self.admit(heap, runtime)?;
        let promise = arguments.first().ok_or(ValueError::InvalidNode)?;
        let undefined = heap.undefined()?;
        let executor = arguments.get(1).unwrap_or(&undefined);
        if heap.kind(executor)? != ValueKind::Function {
            return self.type_error(heap, "Promise executor is not callable", runtime);
        }
        let target = heap.capture_deferred(promise)?;
        {
            let mut state = self.lock()?;
            Self::active(&state, epoch)?;
            if state.slots.contains_key(&target) {
                return Err(ValueError::SourceIdentityCollision);
            }
            state.slots.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            state.slots.insert(target, Slot { epoch, state: NativePromiseState::Pending, reactions: Vec::new() });
        }
        let (_, resolve, reject) = self.make_pair(heap, target, epoch)?;
        if let Err(error) = heap.call(executor, &undefined, &[resolve, reject.clone()], runtime) {
            let reason = self.reason(heap, error, runtime)?;
            heap.call(&reject, &undefined, &[reason], runtime)?;
        }
        // A callback can synchronously revoke. Do not publish a success then.
        Self::active(&*self.lock()?, epoch)?;
        Ok(promise.clone())
    }
    pub(super) fn bind_capability(
        &self,
        heap: &Arc<ValueHeap>,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let (epoch, _, _) = self.admit(heap, runtime)?;
        let key = heap.capture_deferred(arguments.first().ok_or(ValueError::InvalidNode)?)?;
        let resolve = arguments.get(1).ok_or(ValueError::InvalidNode)?;
        let reject = arguments.get(2).ok_or(ValueError::InvalidNode)?;
        let resolve =
            if heap.kind(resolve)? == ValueKind::Undefined { None } else { Some(heap.capture_deferred(resolve)?) };
        let reject =
            if heap.kind(reject)? == ValueKind::Undefined { None } else { Some(heap.capture_deferred(reject)?) };
        let duplicate = {
            let mut state = self.lock()?;
            Self::active(&state, epoch)?;
            let binding = state.bindings.get_mut(&key).ok_or(ValueError::ExpiredCapability(BIND))?;
            if binding.epoch != epoch {
                return Err(ValueError::ExpiredCapability(BIND));
            }
            if binding.resolve.is_some() || binding.reject.is_some() {
                true
            } else {
                binding.resolve = resolve;
                binding.reject = reject;
                false
            }
        };
        if duplicate {
            return self.type_error(heap, "Promise capability executor was already bound", runtime);
        }
        heap.undefined()
    }
    fn new_capability(
        &self,
        heap: &Arc<ValueHeap>,
        constructor: &ValueHandle,
        epoch: u64,
        runtime: &dyn ValueRuntime,
    ) -> Result<Capability> {
        if !self.original(heap, heap.is_constructor(constructor, runtime), runtime)? {
            return self.type_error(heap, "Promise species is not a constructor", runtime);
        }
        let token = heap.object()?;
        let key = heap.capture_deferred(&token)?;
        let port = heap.restore_deferred(&self.port)?;
        let executor = heap.fresh_compiled_function(&CAPABILITY_EXECUTOR, &[port, token])?;
        {
            let mut state = self.lock()?;
            Self::active(&state, epoch)?;
            state.bindings.try_reserve(1).map_err(|_| ValueError::Allocation)?;
            state.bindings.insert(key, Binding { epoch, resolve: None, reject: None });
        }
        // Generic construct is required for BOTH default and compiled species.
        let promise = self.original(heap, heap.construct(constructor, &[executor], runtime), runtime)?;
        let binding = {
            let state = self.lock()?;
            Self::active(&state, epoch)?;
            *state.bindings.get(&key).ok_or(ValueError::InvalidNode)?
        };
        let (Some(resolve), Some(reject)) = (binding.resolve, binding.reject) else {
            return self.type_error(heap, "Promise capability resolve/reject is not callable", runtime);
        };
        for edge in [resolve, reject] {
            if heap.kind(&heap.restore_deferred(&edge)?)? != ValueKind::Function {
                return self.type_error(heap, "Promise capability resolve/reject is not callable", runtime);
            }
        }
        Ok(Capability { promise: heap.capture_deferred(&promise)?, resolve, reject })
    }
    fn species(&self, heap: &Arc<ValueHeap>, promise: &ValueHandle, runtime: &dyn ValueRuntime) -> Result<ValueHandle> {
        let default = heap.restore_deferred(&self.globals()?.constructor)?;
        let constructor = self.original(heap, heap.get(promise, &"constructor".into(), runtime), runtime)?;
        if heap.kind(&constructor)? == ValueKind::Undefined {
            return Ok(default);
        }
        if !matches!(heap.kind(&constructor)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return self.type_error(heap, "Promise constructor property is not an object", runtime);
        }
        let species = self.original(heap, heap.get(&constructor, &heap.species_key(), runtime), runtime)?;
        if matches!(heap.kind(&species)?, ValueKind::Undefined | ValueKind::Null) {
            return Ok(default);
        }
        if !self.original(heap, heap.is_constructor(&species, runtime), runtime)? {
            return self.type_error(heap, "Promise species is not a constructor", runtime);
        }
        Ok(species)
    }
    pub(super) fn then(
        &self,
        heap: &Arc<ValueHeap>,
        promise: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let key = heap.capture_deferred(promise)?;
        // Brand precedes ALL executable constructor/species Gets.
        let branded = self.lock()?.slots.contains_key(&key);
        if !branded {
            return self.type_error(heap, "Promise.then receiver has no Native Promise brand", runtime);
        }
        let (epoch, scope, parent) = self.admit(heap, runtime)?;
        {
            let state = self.lock()?;
            let slot = state.slots.get(&key).ok_or(ValueError::InvalidNode)?;
            if slot.state == NativePromiseState::Pending && slot.epoch != epoch {
                return Err(ValueError::ExpiredCapability(THEN));
            }
        }
        let constructor = self.species(heap, promise, runtime)?;
        let capability = self.new_capability(heap, &constructor, epoch, runtime)?;
        let handler = |value: Option<&ValueHandle>| -> Result<Option<DeferredValue>> {
            match value {
                Some(value) if heap.kind(value)? == ValueKind::Function => Ok(Some(heap.capture_deferred(value)?)),
                _ => Ok(None),
            }
        };
        let reaction = Reaction {
            epoch,
            scope,
            parent: self.retain_parent(parent)?,
            fulfill: handler(arguments.first())?,
            reject: handler(arguments.get(1))?,
            capability,
        };
        {
            let mut state = self.lock()?;
            Self::active(&state, epoch)?;
            let slot = state.slots.get_mut(&key).ok_or(ValueError::InvalidNode)?;
            if slot.state == NativePromiseState::Pending {
                if slot.epoch != epoch {
                    return Err(ValueError::ExpiredCapability(THEN));
                }
                slot.reactions.try_reserve(1).map_err(|_| ValueError::Allocation)?;
                slot.reactions.push(PendingReaction::Promise(reaction));
            } else {
                let outcome = slot.state;
                Self::job(&mut state, epoch, scope, JobKind::Reaction { reaction, state: outcome })?;
            }
        }
        self.wake.notify_waiters();
        self.wake.notify_one();
        heap.restore_deferred(&capability.promise)
    }
    pub(super) fn static_capability(
        &self,
        heap: &Arc<ValueHeap>,
        arguments: &[ValueHandle],
        reject: bool,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let (epoch, _, _) = self.admit(heap, runtime)?;
        let constructor = arguments.first().ok_or(ValueError::InvalidNode)?;
        if !matches!(heap.kind(constructor)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return self.type_error(heap, "Promise static receiver is not an object", runtime);
        }
        let argument = arguments.get(1).ok_or(ValueError::InvalidNode)?;
        if !reject {
            let key = heap.capture_deferred(argument)?;
            if self.lock()?.slots.contains_key(&key) {
                let actual = self.original(heap, heap.get(argument, &"constructor".into(), runtime), runtime)?;
                if actual.same_identity(constructor) {
                    return Ok(argument.clone());
                }
            }
        }
        let capability = self.new_capability(heap, constructor, epoch, runtime)?;
        let resolver = heap.restore_deferred(&if reject { capability.reject } else { capability.resolve })?;
        self.original(heap, heap.call(&resolver, &heap.undefined()?, &[argument.clone()], runtime), runtime)?;
        heap.restore_deferred(&capability.promise)
    }
    pub(super) fn resolve_pair(
        &self,
        heap: &Arc<ValueHeap>,
        arguments: &[ValueHandle],
        reject: bool,
        runtime: &dyn ValueRuntime,
    ) -> Result<ValueHandle> {
        let (epoch, scope, parent) = self.admit(heap, runtime)?;
        let token = heap.capture_deferred(arguments.first().ok_or(ValueError::InvalidNode)?)?;
        let resolution = arguments.get(1).ok_or(ValueError::InvalidNode)?;
        let resolution_edge = heap.capture_deferred(resolution)?;
        let target = {
            let mut state = self.lock()?;
            Self::active(&state, epoch)?;
            let pair = *state.pairs.get(&token).ok_or(ValueError::ExpiredCapability(RESOLVE))?;
            // Retain the original flag even when its target still awaits adoption.
            if pair.already_resolved {
                None
            } else {
                let slot = state.slots.get(&pair.target).ok_or(ValueError::InvalidNode)?;
                if slot.state == NativePromiseState::Pending && slot.epoch != epoch {
                    return Err(ValueError::ExpiredCapability(RESOLVE));
                }
                state.pairs.get_mut(&token).unwrap().already_resolved = true;
                Some(pair.target)
            }
        };
        let Some(target) = target else {
            return heap.undefined();
        };
        if reject {
            self.settle(target, NativePromiseState::Rejected(resolution_edge), epoch)?;
            return heap.undefined();
        }
        if target == resolution_edge {
            let reason = self.error_value(heap, 0, "Cannot resolve a Promise with itself", runtime)?;
            self.settle(target, NativePromiseState::Rejected(heap.capture_deferred(&reason)?), epoch)?;
            return heap.undefined();
        }
        if !matches!(heap.kind(resolution)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            self.settle(target, NativePromiseState::Fulfilled(resolution_edge), epoch)?;
            return heap.undefined();
        }
        let then = match heap.get(resolution, &"then".into(), runtime) {
            Ok(then) => then,
            Err(error) => {
                let reason = self.reason(heap, error, runtime)?;
                self.settle(target, NativePromiseState::Rejected(heap.capture_deferred(&reason)?), epoch)?;
                return heap.undefined();
            }
        };
        if heap.kind(&then)? != ValueKind::Function {
            self.settle(target, NativePromiseState::Fulfilled(resolution_edge), epoch)?;
        } else {
            let then = heap.capture_deferred(&then)?;
            let mut state = self.lock()?;
            let parent = self.retain_parent(parent)?;
            Self::job(&mut state, epoch, scope, JobKind::Thenable { parent, target, thenable: resolution_edge, then })?;
            drop(state);
            self.wake.notify_waiters();
            self.wake.notify_one();
        }
        heap.undefined()
    }
    pub(super) fn execute_job(&self, heap: &Arc<ValueHeap>, job: JobKind, runtime: &dyn ValueRuntime) -> Result<()> {
        let (epoch, _, _) = self.admit(heap, runtime)?;
        let undefined = heap.undefined()?;
        match job {
            JobKind::NativeAwait { awaiter, state } => {
                let (handler, reason) = match state {
                    NativePromiseState::Fulfilled(edge) => (awaiter.fulfill, edge),
                    NativePromiseState::Rejected(edge) => (awaiter.reject, edge),
                    NativePromiseState::Pending => return Err(ValueError::InvalidNode),
                };
                // The original compiled async body handles its own Throw and
                // settles execute-result. Escaped faults visibly fail the driver.
                heap.call(&heap.restore_deferred(&handler)?, &undefined, &[heap.restore_deferred(&reason)?], runtime)?;
            }
            JobKind::Thenable { parent: _parent, target, thenable, then } => {
                // Each thenable job creates a NEW resolver pair. The caller's
                // AlreadyResolved=true pair is never reused for this adoption.
                let (_, resolve, reject) = self.make_pair(heap, target, epoch)?;
                let result = heap.call(
                    &heap.restore_deferred(&then)?,
                    &heap.restore_deferred(&thenable)?,
                    &[resolve, reject.clone()],
                    runtime,
                );
                if let Err(error) = result {
                    let reason = self.reason(heap, error, runtime)?;
                    heap.call(&reject, &undefined, &[reason], runtime)?;
                }
            }
            JobKind::Reaction { reaction, state } => {
                let (rejected, edge) = match state {
                    NativePromiseState::Fulfilled(edge) => (false, edge),
                    NativePromiseState::Rejected(edge) => (true, edge),
                    NativePromiseState::Pending => return Err(ValueError::InvalidNode),
                };
                let argument = heap.restore_deferred(&edge)?;
                let handler = if rejected { reaction.reject } else { reaction.fulfill };
                let (reject, value) = match handler {
                    None => (rejected, argument),
                    Some(handler) => {
                        match heap.call(&heap.restore_deferred(&handler)?, &undefined, &[argument], runtime) {
                            Ok(value) => (false, value),
                            Err(error) => (true, self.reason(heap, error, runtime)?),
                        }
                    }
                };
                let resolver = heap.restore_deferred(&if reject {
                    reaction.capability.reject
                } else {
                    reaction.capability.resolve
                })?;
                self.original(heap, heap.call(&resolver, &undefined, &[value], runtime), runtime)?;
            }
        }
        Self::active(&*self.lock()?, epoch)
    }
}
