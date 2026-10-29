//! Array methods and iterator operations used by the pinned Native schema checker.
//! OMP 596f2da7101178214aa27a753529d15e6b7ad91d: schemaTypes, enum,
//! isRequiredSet and combinator/dependency for-of in json-schema-validator.ts.
//! All property and callback operations use the existing heap and resolver.

use super::*;
use std::sync::TryLockError;

static NEXT_LOCAL_CALLBACK: AtomicU64 = AtomicU64::new(1);
const MAX_SAFE_LENGTH: u64 = 9_007_199_254_740_991;

pub type LocalValueCallback<'a> =
    dyn FnMut(&ValueHandle, &[ValueHandle], &dyn ValueRuntime) -> ValueResult<ValueHandle> + Send + 'a;

struct OperationRuntime<'a, 'callback> {
    token: u64,
    callback: Mutex<&'a mut LocalValueCallback<'callback>>,
    parent: &'a dyn ValueRuntime,
}

impl ValueRuntime for OperationRuntime<'_, '_> {
    fn resolve(&self, request: RuntimeRequest<'_>) -> ValueResult<RuntimeReply> {
        self.resolve_effective(request, self)
    }

    fn resolve_effective(
        &self,
        request: RuntimeRequest<'_>,
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        self.parent.resolve_effective(request, effective_runtime)
    }

    fn resolve_call(
        &self,
        capability: CapabilityId,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        self.parent.resolve_call(capability, receiver, arguments, effective_runtime)
    }

    fn resolve_is_constructor(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<bool> {
        self.parent.resolve_is_constructor(capability, constructor, effective_runtime)
    }

    fn resolve_construct(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.parent.resolve_construct(capability, constructor, arguments, effective_runtime)
    }

    fn resolve_construct_with_new_target(
        &self,
        capability: CapabilityId,
        constructor: &ValueHandle,
        new_target: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.parent.resolve_construct_with_new_target(capability, constructor, new_target, arguments, effective_runtime)
    }

    fn resolve_local_call(
        &self,
        token: u64,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        effective_runtime: &dyn ValueRuntime,
    ) -> ValueResult<RuntimeReply> {
        if token != self.token {
            return self.parent.resolve_local_call(token, receiver, arguments, effective_runtime);
        }
        let mut callback = self.callback.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => {
                ValueError::UnsupportedOperation("Operation-local callback reentry is unsupported")
            }
            TryLockError::Poisoned(_) => ValueError::UnsupportedOperation("Operation-local callback scope is poisoned"),
        })?;
        callback(receiver, arguments, effective_runtime).map(RuntimeReply::Value)
    }
}

/// Scratch for one synchronous for-of operation; roots remain ordinary handles.
/// `next` is captured once, whereas done/value are observed on each result.
#[derive(Debug)]
pub struct IteratorRecord {
    iterator: ValueHandle,
    next: ValueHandle,
    done: bool,
}

impl ValueHeap {
    pub fn iterator_key(&self) -> PropertyKey {
        PropertyKey::Symbol(SymbolKey { heap_token: self.token, node: self.intrinsics.iterator })
    }

    /// Supply a Native predicate to the actual selected method, including a
    /// Host override. Callback functions contain only a never-reused token;
    /// invoking a retained callback after this scope fails explicitly.
    pub fn call_with_local_callback(
        self: &Arc<Self>,
        method: &ValueHandle,
        receiver: &ValueHandle,
        callback: &mut LocalValueCallback<'_>,
        tail_arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(method)?;
        self.admit(receiver)?;
        for value in tail_arguments {
            self.admit(value)?;
        }
        let token = NEXT_LOCAL_CALLBACK
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1))
            .map_err(|_| ValueError::UnsupportedOperation("Operation-local callback tokens exhausted"))?;
        let function = self.allocate(Node::Function {
            object: ObjectData::new(Some(self.intrinsics.function_proto)),
            target: CallableTarget::LocalCallback(token),
        })?;
        // Pinned checker predicates are anonymous single-parameter arrows.
        // A selected Host override observes these same Native function fields.
        for (key, value) in [("length", self.number(1.0)?), ("name", self.string("".into())?)] {
            self.define_own(
                &function,
                key.into(),
                PropertyDescriptor::Data { value, writable: false, enumerable: false, configurable: true },
                &NoRuntime,
            )?;
        }
        let mut arguments = Vec::new();
        arguments.try_reserve(1 + tail_arguments.len()).map_err(|_| ValueError::Allocation)?;
        arguments.push(function);
        arguments.extend_from_slice(tail_arguments);
        let scope = OperationRuntime { token, callback: Mutex::new(callback), parent: runtime };
        self.call(method, receiver, &arguments, &scope)
    }

    pub(super) fn length_of_array_like(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<u64> {
        // Get once, before callback admission and before any species lookup.
        let length = self.get(value, &"length".into(), runtime)?;
        let number = self.to_number(&length, runtime)?;
        Ok(if number.is_nan() || number <= 0.0 {
            0
        } else if number >= MAX_SAFE_LENGTH as f64 {
            MAX_SAFE_LENGTH
        } else {
            number.floor() as u64
        })
    }

    fn require_array_object(&self, receiver: &ValueHandle) -> ValueResult<()> {
        match self.kind(receiver)? {
            ValueKind::Object | ValueKind::Array | ValueKind::Function => Ok(()),
            ValueKind::Null | ValueKind::Undefined => {
                Err(ValueError::TypeError("Array method receiver is null or undefined".into()))
            }
            _ => Err(ValueError::UnsupportedOperation("Array method primitive receiver boxing is not connected")),
        }
    }

    /// Original Array.prototype.indexOf for the existing admitted Array-like
    /// receiver domain. Reuse captured LengthOfArrayLike and live Has/Get;
    /// primitive receiver boxing retains the existing explicit Array-method gap.
    pub fn array_index_of_native(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        if length == 0 {
            return self.number(-1.0);
        }
        let from = arguments.get(1).map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(0.0);
        let from = if from.is_nan() { 0.0 } else { from.trunc() };
        if from >= length as f64 {
            return self.number(-1.0);
        }
        let first = if from < 0.0 { (length as f64 + from).max(0.0) } else { from };
        let search = match arguments.first() {
            Some(value) => value.clone(),
            None => self.undefined()?,
        };
        for index in first as u64..length {
            let key = PropertyKey::from(WireString::from(index.to_string()));
            if !self.has(receiver, &key, runtime)? {
                continue;
            }
            let value = self.get(receiver, &key, runtime)?;
            // Strict equality matches +/-0 and never matches NaN. All other
            // primitives and object/Symbol identities reuse the existing owner.
            let equal = match (self.snapshot(&value)?, self.snapshot(&search)?) {
                (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
                _ => self.object_is(&value, &search)?,
            };
            if equal {
                return self.number(index as f64);
            }
        }
        self.number(-1.0)
    }

    pub(super) fn array_predicate(
        self: &Arc<Self>,
        method: Intrinsic,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        let callback = arguments.first().cloned().unwrap_or(self.undefined()?);
        if self.kind(&callback)? != ValueKind::Function {
            return Err(ValueError::TypeError("Array predicate is not callable".into()));
        }
        let this_arg = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        let filtered = if method == Intrinsic::ArrayFilter {
            Some(if self.kind(receiver)? == ValueKind::Array {
                self.array_species_create(receiver, 0, runtime)?
            } else {
                self.array_with_length(0)?
            })
        } else {
            None
        };
        let mut output_index = 0_u64;
        for index in 0..length {
            let key = PropertyKey::from(WireString::from(index.to_string()));
            // Find visits holes through Get; some/every/filter use HasProperty.
            if method != Intrinsic::ArrayFind && !self.has(receiver, &key, runtime)? {
                continue;
            }
            let value = self.get(receiver, &key, runtime)?;
            let index_value = self.number(index as f64)?;
            let selected = self.call(&callback, &this_arg, &[value.clone(), index_value, receiver.clone()], runtime)?;
            let selected = self.truthy(&selected)?;
            match method {
                Intrinsic::ArrayFind if selected => return Ok(value),
                Intrinsic::ArraySome if selected => return self.boolean(true),
                Intrinsic::ArrayEvery if !selected => return self.boolean(false),
                Intrinsic::ArrayFilter if selected => {
                    let target = filtered.as_ref().ok_or(ValueError::InvalidNode)?;
                    self.define_own_data(target, WireString::from(output_index.to_string()).into(), value, runtime)?;
                    output_index += 1;
                }
                _ => {}
            }
        }
        match method {
            Intrinsic::ArrayFind => self.undefined(),
            Intrinsic::ArraySome => self.boolean(false),
            Intrinsic::ArrayEvery => self.boolean(true),
            Intrinsic::ArrayFilter => filtered.ok_or(ValueError::InvalidNode),
            _ => Err(ValueError::InvalidNode),
        }
    }

    pub(super) fn array_includes(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        if length == 0 {
            return self.boolean(false);
        }
        let needle = arguments.first().cloned().unwrap_or(self.undefined()?);
        let from = arguments.get(1).map(|value| self.to_number(value, runtime)).transpose()?.unwrap_or(0.0);
        let from = if from.is_nan() { 0.0 } else { from.trunc() };
        if from >= length as f64 {
            return self.boolean(false);
        }
        let start = if from >= 0.0 { from as u64 } else { ((length as f64 + from).max(0.0)) as u64 };
        for index in start..length {
            let candidate = self.get(receiver, &WireString::from(index.to_string()).into(), runtime)?;
            let same_zero = matches!((self.snapshot(&candidate)?, self.snapshot(&needle)?),
                (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) if left == 0.0 && right == 0.0);
            if same_zero || self.object_is(&candidate, &needle)? {
                return self.boolean(true);
            }
        }
        self.boolean(false)
    }

    pub(super) fn array_values(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let target = self.admit(receiver)?;
        self.allocate(Node::ArrayIterator {
            object: ObjectData::new(Some(self.intrinsics.array_iterator_proto)),
            target: Some(target),
            next_index: 0,
        })
    }

    pub(super) fn iterator_result(self: &Arc<Self>, value: ValueHandle, done: bool) -> ValueResult<ValueHandle> {
        let result = self.object()?;
        self.define_own_data(&result, "value".into(), value, &NoRuntime)?;
        self.define_own_data(&result, "done".into(), self.boolean(done)?, &NoRuntime)?;
        Ok(result)
    }

    pub(super) fn array_iterator_next(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let (target, index) = {
            let arena = self.lock()?;
            let Node::ArrayIterator { target, next_index, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Incompatible Array Iterator receiver".into()));
            };
            (*target, *next_index)
        };
        let Some(target) = target else {
            return self.iterator_result(self.undefined()?, true);
        };
        let target = self.handle(target);
        let length = self.length_of_array_like(&target, runtime)?;
        {
            let mut arena = self.lock()?;
            let Node::ArrayIterator { target, next_index, .. } = arena.get_mut(node)? else {
                return Err(ValueError::InvalidNode);
            };
            if index >= length {
                *target = None;
            } else {
                *next_index = index + 1;
            }
        }
        if index >= length {
            self.iterator_result(self.undefined()?, true)
        } else {
            let value = self.get(&target, &WireString::from(index.to_string()).into(), runtime)?;
            self.iterator_result(value, false)
        }
    }

    pub fn get_iterator(
        self: &Arc<Self>,
        value: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<IteratorRecord> {
        let method = self.get(value, &self.iterator_key(), runtime)?;
        let iterator = self.call(&method, value, &[], runtime)?;
        if !matches!(self.kind(&iterator)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Iterator method did not return an object".into()));
        }
        let next = self.get(&iterator, &"next".into(), runtime)?;
        Ok(IteratorRecord { iterator, next, done: false })
    }

    pub fn iterator_next(
        self: &Arc<Self>,
        record: &mut IteratorRecord,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<Option<ValueHandle>> {
        self.admit(&record.iterator)?;
        self.admit(&record.next)?;
        if record.done {
            return Ok(None);
        }
        let step = (|| {
            let result = self.call(&record.next, &record.iterator, &[], runtime)?;
            if !matches!(self.kind(&result)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
                return Err(ValueError::TypeError("Iterator next did not return an object".into()));
            }
            let done = self.get(&result, &"done".into(), runtime)?;
            if self.truthy(&done)? { Ok(None) } else { self.get(&result, &"value".into(), runtime).map(Some) }
        })();
        if !matches!(&step, Ok(Some(_))) {
            record.done = true;
        }
        step
    }

    /// Caller preserves an existing throw completion over a Native close
    /// failure. Normal early exit must propagate this method's failure.
    /// Host/admission failures remain explicit and require their own receipt.
    pub fn iterator_close(
        self: &Arc<Self>,
        record: &mut IteratorRecord,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        self.admit(&record.iterator)?;
        if record.done {
            return Ok(());
        }
        record.done = true;
        let method = self.get(&record.iterator, &"return".into(), runtime)?;
        if matches!(self.kind(&method)?, ValueKind::Undefined | ValueKind::Null) {
            return Ok(());
        }
        let result = self.call(&method, &record.iterator, &[], runtime)?;
        if !matches!(self.kind(&result)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            return Err(ValueError::TypeError("Iterator return did not return an object".into()));
        }
        Ok(())
    }
}

impl ValueHeap {
    pub(super) fn array_flat_map(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        let callback = arguments.first().cloned().unwrap_or(self.undefined()?);
        if self.kind(&callback)? != ValueKind::Function {
            return Err(ValueError::TypeError("Array.flatMap callback is not callable".into()));
        }
        let this_arg = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        let output = if self.kind(receiver)? == ValueKind::Array {
            self.array_species_create(receiver, 0, runtime)?
        } else {
            self.array_with_length(0)?
        };
        let mut target_index = 0_u64;
        for index in 0..length {
            let key = PropertyKey::from(WireString::from(index.to_string()));
            if !self.has(receiver, &key, runtime)? {
                continue;
            }
            let value = self.get(receiver, &key, runtime)?;
            let selected =
                self.call(&callback, &this_arg, &[value, self.number(index as f64)?, receiver.clone()], runtime)?;
            if self.kind(&selected)? == ValueKind::Array {
                let inner_length = self.length_of_array_like(&selected, runtime)?;
                for inner_index in 0..inner_length {
                    let inner_key = PropertyKey::from(WireString::from(inner_index.to_string()));
                    if !self.has(&selected, &inner_key, runtime)? {
                        continue;
                    }
                    let member = self.get(&selected, &inner_key, runtime)?;
                    if target_index >= MAX_SAFE_LENGTH {
                        return Err(ValueError::TypeError("Array.flatMap exceeds maximum safe length".into()));
                    }
                    self.define_own_data(&output, WireString::from(target_index.to_string()).into(), member, runtime)?;
                    target_index += 1;
                }
            } else {
                if target_index >= MAX_SAFE_LENGTH {
                    return Err(ValueError::TypeError("Array.flatMap exceeds maximum safe length".into()));
                }
                self.define_own_data(&output, WireString::from(target_index.to_string()).into(), selected, runtime)?;
                target_index += 1;
            }
        }
        Ok(output)
    }

    pub(super) fn array_shift(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        if length == 0 {
            self.assign(receiver, "length".into(), self.number(0.0)?, runtime)?;
            return self.undefined();
        }
        let first = self.get(receiver, &"0".into(), runtime)?;
        for index in 1..length {
            let from = PropertyKey::from(WireString::from(index.to_string()));
            let to = PropertyKey::from(WireString::from((index - 1).to_string()));
            if self.has(receiver, &from, runtime)? {
                let value = self.get(receiver, &from, runtime)?;
                self.assign(receiver, to, value, runtime)?;
            } else if !self.delete_own(receiver, &to, runtime)? {
                return Err(ValueError::TypeError("Array.shift cannot delete indexed property".into()));
            }
        }
        if !self.delete_own(receiver, &WireString::from((length - 1).to_string()).into(), runtime)? {
            return Err(ValueError::TypeError("Array.shift cannot delete final indexed property".into()));
        }
        self.assign(receiver, "length".into(), self.number((length - 1) as f64)?, runtime)?;
        Ok(first)
    }

    pub(super) fn array_map(
        self: &Arc<Self>,
        receiver: &ValueHandle,
        arguments: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.require_array_object(receiver)?;
        let length = self.length_of_array_like(receiver, runtime)?;
        let callback = arguments.first().cloned().unwrap_or(self.undefined()?);
        if self.kind(&callback)? != ValueKind::Function {
            return Err(ValueError::TypeError("Array.map callback is not callable".into()));
        }
        let this_arg = arguments.get(1).cloned().unwrap_or(self.undefined()?);
        // ArraySpeciesCreate receives the captured length. Ordinary non-array
        // receivers still use ArrayCreate; holes do not become Undefined writes.
        let length32 = u32::try_from(length).map_err(|_| ValueError::RangeError("Invalid array length".into()))?;
        let output = if self.kind(receiver)? == ValueKind::Array {
            self.array_species_create(receiver, length32, runtime)?
        } else {
            self.array_with_length(length32)?
        };
        for index in 0..length {
            let key = PropertyKey::from(WireString::from(index.to_string()));
            if !self.has(receiver, &key, runtime)? {
                continue;
            }
            let value = self.get(receiver, &key, runtime)?;
            let selected =
                self.call(&callback, &this_arg, &[value, self.number(index as f64)?, receiver.clone()], runtime)?;
            self.define_own_data(&output, key, selected, runtime)?;
        }
        Ok(output)
    }

    pub(super) fn set_values(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let target = self.admit(receiver)?;
        if !matches!(self.lock()?.get(target)?, Node::Set { .. }) {
            return Err(ValueError::TypeError("Set.values receiver has no Set data".into()));
        }
        self.allocate(Node::SetIterator {
            object: ObjectData::new(Some(self.intrinsics.set_iterator_proto)),
            target: Some(target),
            next_index: 0,
        })
    }

    pub(super) fn set_iterator_next(self: &Arc<Self>, receiver: &ValueHandle) -> ValueResult<ValueHandle> {
        let node = self.admit(receiver)?;
        let value = {
            let mut arena = self.lock()?;
            let Node::SetIterator { target, next_index, .. } = arena.get(node)? else {
                return Err(ValueError::TypeError("Incompatible Set Iterator receiver".into()));
            };
            let (target, mut index) = (*target, *next_index);
            let value = if let Some(target) = target {
                let Node::Set { entries, .. } = arena.get(target)? else {
                    return Err(ValueError::InvalidNode);
                };
                loop {
                    match entries.get(index) {
                        Some(Some(value)) => {
                            index += 1;
                            break Some(*value);
                        }
                        Some(None) => {
                            index += 1;
                        }
                        None => break None,
                    }
                }
            } else {
                None
            };
            let Node::SetIterator { target, next_index, .. } = arena.get_mut(node)? else {
                return Err(ValueError::InvalidNode);
            };
            if value.is_some() {
                *next_index = index;
            } else {
                *target = None;
            }
            value
        };
        match value {
            Some(value) => self.iterator_result(self.handle(value), false),
            None => self.iterator_result(self.undefined()?, true),
        }
    }
}
