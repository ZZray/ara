//! Fixed utils/index.ts: primitive alias, plain/array structuredClone try/catch,
//! then uncaught plain JSON.stringify/strict JSON.parse fallback.
//! Fixed ai/utils/validation.ts: path ancestors use spread/slice/splice, not
//! this deep clone. These primitives do not implement the normalizer itself.

use super::*;
use std::collections::HashMap;

impl ValueHeap {
    /// CopyDataProperties: own keys are captured first, descriptors/getters
    /// are evaluated per key, and each successful write happens before the next.
    pub fn spread(self: &Arc<Self>, source: &ValueHandle, runtime: &dyn ValueRuntime) -> ValueResult<ValueHandle> {
        self.spread_excluding(source, &[], runtime)
    }
    pub fn spread_excluding(
        self: &Arc<Self>,
        source: &ValueHandle,
        excluded: &[PropertyKey],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(source)?;
        for key in excluded {
            self.admit_key(key)?;
        }
        let target = self.object()?;
        self.copy_data_properties_into(&target, source, excluded, runtime)?;
        Ok(target)
    }
    pub fn spread_into(
        self: &Arc<Self>,
        target: &ValueHandle,
        source: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        self.copy_data_properties_into(target, source, &[], runtime)
    }
    fn copy_data_properties_into(
        self: &Arc<Self>,
        target: &ValueHandle,
        source: &ValueHandle,
        excluded: &[PropertyKey],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<()> {
        self.admit(target)?;
        self.admit(source)?;
        if matches!(self.kind(source)?, ValueKind::Undefined | ValueKind::Null) {
            return Ok(());
        }
        for key in self.own_keys(source, runtime)? {
            if excluded.contains(&key) {
                continue;
            }
            if self.own_descriptor(source, &key, runtime)?.is_some_and(|descriptor| descriptor.enumerable()) {
                let value = self.get(source, &key, runtime)?;
                self.define_own_data(target, key, value, runtime)?;
            }
        }
        Ok(())
    }
    pub(super) fn array_species_create(
        self: &Arc<Self>,
        source: &ValueHandle,
        length: u32,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        // These Gets are observable. Trusted ordinary functions use their live
        // prototype; other custom construction keeps the existing Host port.
        let mut constructor = self.get(source, &"constructor".into(), runtime)?;
        if matches!(self.kind(&constructor)?, ValueKind::Object | ValueKind::Array | ValueKind::Function) {
            constructor = self.get(&constructor, &self.species_key(), runtime)?;
            if self.kind(&constructor)? == ValueKind::Null {
                constructor = self.undefined()?;
            }
        }
        if self.kind(&constructor)? == ValueKind::Undefined || constructor.node == self.intrinsics.array_constructor {
            return self.array_with_length(length);
        }
        let capability = match self.snapshot(&constructor)? {
            ValueSnapshot::Function(CallableTarget::Compiled(_)) => {
                return self.construct_compiled_function(&constructor, &[self.number(f64::from(length))?], runtime);
            }
            ValueSnapshot::Function(CallableTarget::Host(capability))
            | ValueSnapshot::Opaque { kind: OpaqueKind::Function, capability, .. } => capability,
            _ => {
                return Err(ValueError::TypeError("Array species is not a supported constructor".into()));
            }
        };
        let result = self.runtime_value(runtime.resolve(RuntimeRequest::ConstructArray {
            capability,
            constructor: &constructor,
            length,
        }))?;
        self.object_id(&result)?;
        Ok(result)
    }
    pub fn array_slice(
        self: &Arc<Self>,
        source: &ValueHandle,
        start: f64,
        end: Option<f64>,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let length = self.array_length(source, runtime)?;
        let start = relative_index(start, length);
        let end = relative_index(end.unwrap_or(f64::from(length)), length);
        let count = end.saturating_sub(start);
        let target = self.array_species_create(source, count, runtime)?;
        for offset in 0..count {
            let key = index_key(start + offset);
            if self.has(source, &key, runtime)? {
                let value = self.get(source, &key, runtime)?;
                self.define_own_data(&target, index_key(offset), value, runtime)?;
            }
        }
        // Species may return an ordinary object, so the Set is not an array-only shortcut.
        self.assign(&target, "length".into(), self.number(f64::from(count))?, runtime)?;
        Ok(target)
    }
    pub fn array_splice(
        self: &Arc<Self>,
        target: &ValueHandle,
        start: f64,
        delete_count: Option<f64>,
        items: &[ValueHandle],
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        let length = self.array_length(target, runtime)?;
        let start = relative_index(start, length);
        let delete = match delete_count {
            None => length - start,
            Some(value) if value.is_nan() || value <= 0.0 => 0,
            Some(value) => (value.trunc().min(f64::from(length - start))) as u32,
        };
        let item_count = u64::try_from(items.len())
            .map_err(|_| ValueError::TypeError("Splice length exceeds safe integer range".into()))?;
        for item in items {
            self.admit(item)?;
        }
        let new_length = u64::from(length - delete)
            .checked_add(item_count)
            .filter(|length| *length <= 9_007_199_254_740_991)
            .ok_or_else(|| ValueError::TypeError("Splice length exceeds safe integer range".into()))?;
        let removed = self.array_species_create(target, delete, runtime)?;
        for offset in 0..delete {
            let key = index_key(start + offset);
            if self.has(target, &key, runtime)? {
                let value = self.get(target, &key, runtime)?;
                self.define_own_data(&removed, index_key(offset), value, runtime)?;
            }
        }
        self.assign(&removed, "length".into(), self.number(f64::from(delete))?, runtime)?;
        if item_count < u64::from(delete) {
            for index in start..(length - delete) {
                let from = index_key(index + delete);
                let to = PropertyKey::from(WireString::from((u64::from(index) + item_count).to_string()));
                if self.has(target, &from, runtime)? {
                    self.assign(target, to, self.get(target, &from, runtime)?, runtime)?;
                } else if !self.delete_own(target, &to, runtime)? {
                    return Err(ValueError::TypeError("Cannot delete splice element".into()));
                }
            }
            for index in (new_length..u64::from(length)).rev() {
                if !self.delete_own(target, &PropertyKey::from(WireString::from(index.to_string())), runtime)? {
                    return Err(ValueError::TypeError("Cannot delete splice tail".into()));
                }
            }
        } else if item_count > u64::from(delete) {
            for index in (start..(length - delete)).rev() {
                let from = index_key(index + delete);
                let to = PropertyKey::from(WireString::from((u64::from(index) + item_count).to_string()));
                if self.has(target, &from, runtime)? {
                    self.assign(target, to, self.get(target, &from, runtime)?, runtime)?;
                } else if !self.delete_own(target, &to, runtime)? {
                    return Err(ValueError::TypeError("Cannot delete splice element".into()));
                }
            }
        }
        for (offset, item) in items.iter().enumerate() {
            self.assign(
                target,
                PropertyKey::from(WireString::from((u64::from(start) + offset as u64).to_string())),
                item.clone(),
                runtime,
            )?;
        }
        self.assign(target, "length".into(), self.number(new_length as f64)?, runtime)?;
        Ok(removed)
    }
    /// Each strict import creates its own complete graph. Work frames and
    /// allocated partial roots remain operation-local until successful return.
    pub fn import_strict(self: &Arc<Self>, source: WireValue) -> ValueResult<ValueHandle> {
        enum Frame<'a> {
            Value(&'a WireValue),
            Array(usize),
            Object(Vec<WireString>),
        }
        let mut work = vec![Frame::Value(&source)];
        let mut results: Vec<ValueHandle> = Vec::new();
        while let Some(frame) = work.pop() {
            match frame {
                Frame::Value(value) => match value {
                    WireValue::Null => results.push(self.null()?),
                    WireValue::Bool(value) => results.push(self.boolean(*value)?),
                    WireValue::Number(value) => results.push(self.number(*value)?),
                    WireValue::String(value) => results.push(self.string(value.clone())?),
                    WireValue::Array(values) => {
                        work.push(Frame::Array(values.len()));
                        for value in values.iter().rev() {
                            work.push(Frame::Value(value));
                        }
                    }
                    WireValue::Object(entries) => {
                        work.push(Frame::Object(entries.iter().map(|(key, _)| key.clone()).collect()));
                        for (_, value) in entries.iter().rev() {
                            work.push(Frame::Value(value));
                        }
                    }
                },
                Frame::Array(length) => {
                    let begin = results.len().checked_sub(length).ok_or(ValueError::InvalidNode)?;
                    let values = results.split_off(begin);
                    results.push(self.array(values)?);
                }
                Frame::Object(keys) => {
                    let begin = results.len().checked_sub(keys.len()).ok_or(ValueError::InvalidNode)?;
                    let values = results.split_off(begin);
                    let object = self.object()?;
                    for (key, value) in keys.into_iter().zip(values) {
                        // strict JSON own __proto__ is data, never inherited ordinary assignment.
                        self.define_own_data(&object, key.into(), value, &NoRuntime)?;
                    }
                    results.push(object);
                }
            }
        }
        if results.len() != 1 {
            return Err(ValueError::InvalidNode);
        }
        results.pop().ok_or(ValueError::InvalidNode)
    }
    pub fn parse_strict_units(self: &Arc<Self>, source: &WireString) -> ValueResult<ValueHandle> {
        let parsed = WireValue::parse_units(source.units()).map_err(ValueError::StrictParse)?;
        self.import_strict(parsed)
    }
    /// Native structuredClone object traversal, with per-operation identity memo.
    /// Placeholders are never sent to callbacks or published through an owner.
    pub fn structured_clone(
        self: &Arc<Self>,
        source: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        enum Frame {
            Object { source: ValueHandle, target: ValueHandle, keys: Vec<WireString>, next: usize },
            Set { target: ValueHandle, entries: Vec<NodeId>, next: usize },
        }
        enum Internal {
            Set(Vec<NodeId>),
            RegExp(regexp::RegExpData),
            Ordinary,
        }
        fn seed(
            heap: &Arc<ValueHeap>,
            source: &ValueHandle,
            memo: &mut HashMap<NodeId, NodeId>,
            runtime: &dyn ValueRuntime,
        ) -> ValueResult<(ValueHandle, Option<Frame>)> {
            heap.admit(source)?;
            if let Some(&node) = memo.get(&source.node) {
                return Ok((heap.handle(node), None));
            }
            let internal = {
                let arena = heap.lock()?;
                match arena.get(source.node)? {
                    Node::ArrayIterator { .. } => {
                        return Err(ValueError::DataCloneError("Array Iterator cannot be cloned".into()));
                    }
                    Node::WeakSet { .. } => {
                        return Err(ValueError::DataCloneError("WeakSet cannot be cloned".into()));
                    }
                    Node::SetIterator { .. } => {
                        return Err(ValueError::DataCloneError("Set Iterator cannot be cloned".into()));
                    }
                    Node::Map { .. } => {
                        return Err(ValueError::UnsupportedOperation(
                            "Native Map structured-clone data is not connected",
                        ));
                    }
                    Node::MapIterator { .. } => {
                        return Err(ValueError::DataCloneError("Map Iterator cannot be cloned".into()));
                    }
                    Node::Set { entries, .. } => Internal::Set(entries.iter().flatten().copied().collect()),
                    Node::RegExp { data, .. } => Internal::RegExp(data.clone()),
                    _ => Internal::Ordinary,
                }
            };
            match internal {
                Internal::Set(entries) => {
                    let target = heap.allocate(Node::Set {
                        object: ObjectData::new(Some(heap.intrinsics.set_proto)),
                        entries: Vec::new(),
                    })?;
                    memo.insert(source.node, target.node);
                    return Ok((target.clone(), Some(Frame::Set { target, entries, next: 0 })));
                }
                Internal::RegExp(data) => {
                    let target = heap.clone_regexp_data(data)?;
                    memo.insert(source.node, target.node);
                    return Ok((target, None));
                }
                Internal::Ordinary => {}
            }
            let (target, copy_properties) = match heap.snapshot(source)? {
                ValueSnapshot::Undefined => (heap.undefined()?, false),
                ValueSnapshot::Null => (heap.null()?, false),
                ValueSnapshot::Boolean(value) => (heap.boolean(value)?, false),
                ValueSnapshot::Number(value) => (heap.number(value)?, false),
                ValueSnapshot::String(value) => (heap.string(value)?, false),
                ValueSnapshot::BigInt(value) => (heap.bigint_decimal(value.as_str())?, false),
                ValueSnapshot::Object => (heap.object()?, true),
                ValueSnapshot::Array => (heap.array_with_length(heap.array_length(source, runtime)?)?, true),
                ValueSnapshot::Function(_) | ValueSnapshot::Symbol { .. } => {
                    return Err(ValueError::DataCloneError("Function or Symbol cannot be cloned".into()));
                }
                ValueSnapshot::Opaque { kind: OpaqueKind::Function | OpaqueKind::Symbol, .. } => {
                    return Err(ValueError::DataCloneError("Function or Symbol cannot be cloned".into()));
                }
                ValueSnapshot::Opaque { capability, .. } => {
                    let cloned = heap.runtime_value(runtime.resolve(RuntimeRequest::Opaque {
                        capability,
                        target: source,
                        receiver: source,
                        operation: OpaqueOperation::StructuredClone,
                    }))?;
                    if cloned.same_identity(source) {
                        return Err(ValueError::WrongRuntimeReply("StructuredClone must return a fresh identity"));
                    }
                    (cloned, false)
                }
            };
            memo.insert(source.node, target.node);
            let frame = if copy_properties {
                let keys = heap
                    .own_keys(source, runtime)?
                    .into_iter()
                    .filter_map(|key| match key {
                        PropertyKey::String(key) => Some(key),
                        PropertyKey::Symbol(_) => None,
                    })
                    .collect();
                Some(Frame::Object { source: source.clone(), target: target.clone(), keys, next: 0 })
            } else {
                None
            };
            Ok((target, frame))
        }
        let mut memo = HashMap::new();
        let (root, frame) = seed(self, source, &mut memo, runtime)?;
        let mut frames = Vec::new();
        if let Some(frame) = frame {
            frames.push(frame);
        }
        while let Some(frame) = frames.pop() {
            let child_frame = match frame {
                Frame::Object { source, target, keys, next } => {
                    if next >= keys.len() {
                        continue;
                    }
                    let key = PropertyKey::String(keys[next].clone());
                    if !self.own_descriptor(&source, &key, runtime)?.is_some_and(|descriptor| descriptor.enumerable()) {
                        frames.push(Frame::Object { source, target, keys, next: next + 1 });
                        continue;
                    }
                    let child = self.get(&source, &key, runtime)?;
                    let (child, child_frame) = seed(self, &child, &mut memo, runtime)?;
                    self.define_own_data(&target, key, child, &NoRuntime)?;
                    frames.push(Frame::Object { source, target, keys, next: next + 1 });
                    child_frame
                }
                Frame::Set { target, entries, next } => {
                    if next >= entries.len() {
                        continue;
                    }
                    let source = self.handle(entries[next]);
                    let (child, child_frame) = seed(self, &source, &mut memo, runtime)?;
                    self.set_add(&target, Some(&child))?;
                    frames.push(Frame::Set { target, entries, next: next + 1 });
                    child_frame
                }
            };
            if let Some(frame) = child_frame {
                frames.push(frame);
            }
        }
        Ok(root)
    }
    pub fn structured_clone_json(
        self: &Arc<Self>,
        source: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueResult<ValueHandle> {
        self.admit(source)?;
        // Native typeof/function/symbol/primitive branch returns the original value.
        if !matches!(self.kind(source)?, ValueKind::Object | ValueKind::Array) {
            return self.adopt(source);
        }
        // Object.getPrototypeOf is outside the catch and precedes Array.isArray.
        let plain =
            self.prototype(source, runtime)?.is_some_and(|prototype| prototype.node == self.intrinsics.object_proto)
                || self.kind(source)? == ValueKind::Array;
        if plain {
            match self.structured_clone(source, runtime) {
                Ok(cloned) => return Ok(cloned),
                Err(error) if error.is_native_exception() => {}
                Err(boundary) => return Err(boundary),
            }
            // Only this attempted structuredClone is caught, retaining its prior side effects.
        }
        let Some(text) = self.stringify(source, StringifyMode::Plain, None, runtime)? else {
            return Err(ValueError::StrictParse(Utf16JsonError {
                offset: 0,
                message: "Unexpected token 'u', undefined is not valid JSON",
            }));
        };
        self.parse_strict_units(&text)
    }
}

fn relative_index(value: f64, length: u32) -> u32 {
    if value.is_nan() || value == 0.0 {
        return 0;
    }
    let integer = value.trunc();
    if integer < 0.0 { (f64::from(length) + integer).max(0.0) as u32 } else { integer.min(f64::from(length)) as u32 }
}
