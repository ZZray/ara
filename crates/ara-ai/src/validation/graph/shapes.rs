//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/validation.ts:1104-1186,1337-1377,1401-1555.
//! Actual key decoding, single-string remapping and flattened-array rebuilding.
/*
MIT License

Copyright (c) 2025 Mario Zechner
Copyright (c) 2025-2026 Can Bölük
Copyright (c) 2026 Stencil Labs, Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
*/

use super::{GraphNormalization, Normalizer};
use crate::graph_values::GraphValueFactory;
use crate::schema_validator::{
    graph_numeric_less_than, graph_value_less_than_number_with_runtime, graph_values_compare_with_runtime,
};
use ara_rpc::value_heap::{ValueError, ValueHandle, ValueKind, ValueSnapshot};

pub(super) fn normalize_graph_double_encoded_keys(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(value)?;
    Normalizer { factory }.shape_keys(value)
}

pub(super) fn normalize_graph_single_string_field(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(schema)?;
    factory.heap().adopt(value)?;
    Normalizer { factory }.shape_single_string(schema, value)
}

pub(super) fn normalize_graph_flattened_array_properties(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(value)?;
    Normalizer { factory }.shape_flattened(value)
}

impl Normalizer<'_> {
    fn shape_step(&self, kind: &str, name: Option<ValueHandle>, index: ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let step = heap.object()?;
        heap.define_own_data(&step, "kind".into(), heap.string(kind.into())?, self.factory.runtime())?;
        if let Some(name) = name {
            heap.define_own_data(&step, "name".into(), name, self.factory.runtime())?;
        }
        heap.define_own_data(&step, "index".into(), index, self.factory.runtime())?;
        Ok(step)
    }

    fn shape_parse_flattened(&self, key: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let zero = heap.number(0.0)?;
        if self.shape_strict_equal(&self.get(key, "length")?, &zero)? {
            return Ok(None);
        }
        let steps = heap.array(vec![])?;
        let ident = heap.module_regexp("ara-ai.validation", "FLATTENED_IDENT_RE", r"^[A-Za-z_$][A-Za-z0-9_$]*", "")?;
        let index_re = heap.module_regexp("ara-ai.validation", "FLATTENED_INDEX_RE", r"^\[(\d+)\]", "")?;
        let first = self.method(&ident, "exec", std::slice::from_ref(key))?;
        if !heap.truthy(&first)? {
            return Ok(None);
        }
        let push = self.get(&steps, "push")?;
        let step = self.shape_step("prop", Some(self.get(&first, "0")?), zero.clone())?;
        heap.call(&push, &steps, &[step], runtime)?;
        let mut pos = self.get(&self.get(&first, "0")?, "length")?;
        let mut saw_index = false;
        while graph_values_compare_with_runtime(self.factory, &pos, &self.get(key, "length")?, "<", runtime)? {
            if self.is_text(&heap.get(key, &self.property_key(&pos)?, runtime)?, ".")? {
                pos = heap.increment(&pos, runtime)?;
                let exec = self.get(&ident, "exec")?;
                let suffix = self.method(key, "slice", std::slice::from_ref(&pos))?;
                let matched = heap.call(&exec, &ident, &[suffix], runtime)?;
                if !heap.truthy(&matched)?
                    || self.shape_strict_equal(&self.get(&self.get(&matched, "0")?, "length")?, &zero)?
                {
                    return Ok(None);
                }
                let push = self.get(&steps, "push")?;
                let step = self.shape_step("prop", Some(self.get(&matched, "0")?), zero.clone())?;
                heap.call(&push, &steps, &[step], runtime)?;
                pos = heap.add(&pos, &self.get(&self.get(&matched, "0")?, "length")?, runtime)?;
                continue;
            }
            if self.is_text(&heap.get(key, &self.property_key(&pos)?, runtime)?, "[")? {
                let exec = self.get(&index_re, "exec")?;
                let suffix = self.method(key, "slice", std::slice::from_ref(&pos))?;
                let matched = heap.call(&exec, &index_re, &[suffix], runtime)?;
                if !heap.truthy(&matched)? {
                    return Ok(None);
                }
                // Explicit Number accepts a BigInt primitive; ToNumber does not.
                let primitive = heap.to_primitive(&self.get(&matched, "1")?, false, runtime)?;
                let index = match heap.snapshot(&primitive)? {
                    ValueSnapshot::BigInt(value) => value.as_str().parse::<f64>().unwrap_or(f64::NAN),
                    _ => heap.to_number(&primitive, runtime)?,
                };
                if !index.is_finite()
                    || index.fract() != 0.0
                    || index.abs() > 9_007_199_254_740_991.0
                    || !(0.0..=100_000.0).contains(&index)
                {
                    return Ok(None);
                }
                let push = self.get(&steps, "push")?;
                let step = self.shape_step("index", None, heap.number(index)?)?;
                heap.call(&push, &steps, &[step], runtime)?;
                saw_index = true;
                pos = heap.add(&pos, &self.get(&self.get(&matched, "0")?, "length")?, runtime)?;
                continue;
            }
            return Ok(None);
        }
        Ok(if saw_index { Some(steps) } else { None })
    }

    fn shape_grow_array(&self, array: &ValueHandle, step: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        while graph_values_compare_with_runtime(
            self.factory,
            &self.get(array, "length")?,
            &self.get(step, "index")?,
            "<=",
            runtime,
        )? {
            self.method(array, "push", &[heap.undefined()?])?;
        }
        Ok(())
    }

    fn shape_build_flattened(
        &self,
        root: &ValueHandle,
        steps: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let mut node = root.clone();
        let mut index = 0.0;
        // steps is the actual fresh Array; replaced push may mutate its slots.
        while index < heap.to_number(&self.get(steps, "length")?, runtime)? - 1.0 {
            let step = heap.get(steps, &self.property_key(&heap.number(index)?)?, runtime)?;
            let next = heap.get(steps, &self.property_key(&heap.number(index + 1.0)?)?, runtime)?;
            let next_is_array = self.is_text(&self.get(&next, "kind")?, "index")?;
            if self.is_text(&self.get(&step, "kind")?, "prop")? {
                if self.null(&node)? || !self.typeof_object(&node)? || self.array(&node)? {
                    return Ok(false);
                }
                let existing = if heap.has_own(&node, &self.property_key(&self.get(&step, "name")?)?, runtime)? {
                    heap.get(&node, &self.property_key(&self.get(&step, "name")?)?, runtime)?
                } else {
                    heap.undefined()?
                };
                let child = if heap.kind(&existing)? == ValueKind::Undefined
                    && !heap.has_own(&node, &self.property_key(&self.get(&step, "name")?)?, runtime)?
                {
                    if next_is_array { heap.array(vec![])? } else { heap.object()? }
                } else {
                    if self.array(&existing)? != next_is_array {
                        return Ok(false);
                    }
                    existing
                };
                heap.define_own_data(&node, self.property_key(&self.get(&step, "name")?)?, child.clone(), runtime)?;
                node = child;
                index += 1.0;
                continue;
            }
            if !self.array(&node)? {
                return Ok(false);
            }
            self.shape_grow_array(&node, &step)?;
            let mut child = heap.get(&node, &self.property_key(&self.get(&step, "index")?)?, runtime)?;
            if heap.kind(&child)? == ValueKind::Undefined {
                child = if next_is_array { heap.array(vec![])? } else { heap.object()? };
                heap.assign(&node, self.property_key(&self.get(&step, "index")?)?, child.clone(), runtime)?;
            } else if self.array(&child)? {
                if !next_is_array {
                    return Ok(false);
                }
            } else if !self.typeof_object(&child)? || self.null(&child)? || next_is_array {
                return Ok(false);
            }
            node = child;
            index += 1.0;
        }
        let last_index = heap.to_number(&self.get(steps, "length")?, runtime)? - 1.0;
        let last = heap.get(steps, &self.property_key(&heap.number(last_index)?)?, runtime)?;
        if self.is_text(&self.get(&last, "kind")?, "prop")? {
            if self.null(&node)? || !self.typeof_object(&node)? || self.array(&node)? {
                return Ok(false);
            }
            heap.define_own_data(&node, self.property_key(&self.get(&last, "name")?)?, value.clone(), runtime)?;
        } else {
            if !self.array(&node)? {
                return Ok(false);
            }
            self.shape_grow_array(&node, &last)?;
            heap.assign(&node, self.property_key(&self.get(&last, "index")?)?, value.clone(), runtime)?;
        }
        Ok(true)
    }

    fn shape_flattened(&self, value: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if self.null(value)? || !self.typeof_object(value)? || self.array(value)? {
            return Ok(self.unchanged(value));
        }
        let out = heap.object()?;
        let entries = heap.object_entries(value, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        let mut changed = false;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, entry) = self.pair(&entry)?;
                let Some(steps) = self.shape_parse_flattened(&key)? else {
                    if heap.has_own(&out, &self.property_key(&key)?, runtime)? {
                        return Ok(false);
                    }
                    heap.define_own_data(&out, self.property_key(&key)?, entry, runtime)?;
                    return Ok(true);
                };
                if heap.kind(&entry)? == ValueKind::Undefined {
                    return Ok(true);
                }
                if !self.shape_build_flattened(&out, &steps, &entry)? {
                    return Ok(false);
                }
                changed = true;
                Ok::<_, ValueError>(true)
            })();
            match result {
                Ok(true) => (),
                Ok(false) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(self.unchanged(value));
                }
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(GraphNormalization { value: if changed { out } else { value.clone() }, changed })
    }

    fn shape_strict_equal(&self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        match (heap.snapshot(left)?, heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => heap.object_is(left, right),
        }
    }

    fn shape_decode_key(&self, key: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let mut current = key.clone();
        let mut decoded = None;
        for _ in 0..3 {
            // A replaced entry-pair iterator may yield an exotic key. Keep both
            // length Gets and the numbered reads in their original order.
            let length = self.get(&current, "length")?;
            if graph_value_less_than_number_with_runtime(self.factory, &length, 2.0, runtime)? {
                break;
            }
            if !self.is_text(&self.get(&current, "0")?, "\"")? {
                break;
            }
            let length = self.get(&current, "length")?;
            // Native subtraction mixes this value with Number(1); BigInt and
            // Symbol therefore throw before JSON.parse's catch.
            let last = heap.number(heap.to_number(&length, runtime)? - 1.0)?;
            let last = heap.get(&current, &self.property_key(&last)?, runtime)?;
            if !self.is_text(&last, "\"")? {
                break;
            }
            let parsed = (|| {
                // JSON.parse uses implicit ToString, unlike explicit String(Symbol).
                if heap.kind(&current)? == ValueKind::Symbol {
                    return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
                }
                let text = heap.to_string(&current, runtime)?;
                heap.parse_strict_units(&text)
            })();
            let parsed = match parsed {
                Ok(parsed) => parsed,
                Err(error) if error.is_native_exception() || matches!(&error, ValueError::StrictParse(_)) => {
                    break;
                }
                Err(boundary) => return Err(boundary),
            };
            if heap.kind(&parsed)? != ValueKind::String {
                break;
            }
            current = parsed;
            decoded = Some(current.clone());
        }
        Ok(decoded)
    }

    fn shape_keys(&self, value: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if self.array(value)? {
            let mut changed = false;
            let mut next = value.clone();
            let mut index = 0.0;
            while graph_numeric_less_than(self.factory, index, &self.get(value, "length")?)? {
                let key = self.property_key(&heap.number(index)?)?;
                let entry = heap.get(value, &key, runtime)?;
                let normalized = self.shape_keys(&entry)?;
                if normalized.changed {
                    if !changed {
                        next = self.array_spread(value)?;
                        changed = true;
                    }
                    heap.assign(&next, key, normalized.value, runtime)?;
                }
                index += 1.0;
            }
            return Ok(GraphNormalization { value: next, changed });
        }
        if self.null(value)? || !self.typeof_object(value)? {
            return Ok(self.unchanged(value));
        }
        let out = heap.object()?;
        // Object.entries completes every source getter before the for-of body.
        let entries = heap.object_entries(value, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        let mut changed = false;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, entry) = self.pair(&entry)?;
                let child = self.shape_keys(&entry)?;
                let next_child = if child.changed { child.value } else { entry };
                let decoded = self.shape_decode_key(&key)?;
                let target = if let Some(decoded) = decoded {
                    if !self.shape_strict_equal(&decoded, &key)?
                        && !heap.has_own(value, &self.property_key(&decoded)?, runtime)?
                        && !heap.has_own(&out, &self.property_key(&decoded)?, runtime)?
                    {
                        decoded
                    } else {
                        key.clone()
                    }
                } else {
                    key.clone()
                };
                if !self.shape_strict_equal(&target, &key)? || child.changed {
                    changed = true;
                }
                heap.define_own_data(&out, self.property_key(&target)?, next_child, runtime)?;
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: if changed { out } else { value.clone() }, changed })
    }

    fn shape_single_key(&self, schema: &ValueHandle) -> Result<Option<ValueHandle>, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if !heap.truthy(schema)? || !self.typeof_object(schema)? || self.array(schema)? {
            return Ok(None);
        }
        if !self.is_text(&self.get(schema, "type")?, "object")? {
            return Ok(None);
        }
        let properties = self.get(schema, "properties")?;
        if !heap.truthy(&properties)? || !self.typeof_object(&properties)? {
            return Ok(None);
        }
        let keys = self.keys_array(&properties)?;
        let one = heap.number(1.0)?;
        if !self.shape_strict_equal(&self.get(&keys, "length")?, &one)? {
            return Ok(None);
        }
        let key = self.get(&keys, "0")?;
        let required = self.get(schema, "required")?;
        if !self.array(&required)?
            || !self.shape_strict_equal(&self.get(&required, "length")?, &one)?
            || !self.shape_strict_equal(&self.get(&required, "0")?, &key)?
        {
            return Ok(None);
        }
        let property_schema = heap.get(&properties, &self.property_key(&key)?, runtime)?;
        if !heap.truthy(&property_schema)? || !self.typeof_object(&property_schema)? {
            return Ok(None);
        }
        Ok(if self.is_text(&self.get(&property_schema, "type")?, "string")? { Some(key) } else { None })
    }

    fn shape_single_string(&self, schema: &ValueHandle, value: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let Some(key) = self.shape_single_key(schema)? else {
            return Ok(self.unchanged(value));
        };
        if !self.typeof_object(value)? || self.null(value)? || self.array(value)? {
            return Ok(self.unchanged(value));
        }
        if heap.kind(&heap.get(value, &self.property_key(&key)?, runtime)?)? != ValueKind::Undefined {
            return Ok(self.unchanged(value));
        }
        for candidate in heap.for_in_string_keys(value, runtime)? {
            let candidate_value = heap.string(candidate.clone())?;
            if self.shape_strict_equal(&candidate_value, &key)?
                || !heap.has_own(value, &candidate.clone().into(), runtime)?
            {
                continue;
            }
            let entry = heap.get(value, &candidate.clone().into(), runtime)?;
            if heap.kind(&entry)? != ValueKind::String {
                continue;
            }
            let next = heap.spread(value, runtime)?;
            // Computed object-literal definition never invokes __proto__'s setter.
            heap.define_own_data(&next, self.property_key(&key)?, entry, runtime)?;
            if !heap.delete_own(&next, &candidate.into(), runtime)? {
                return Err(ValueError::TypeError(
                    "Cannot delete property in strict single-string normalization".into(),
                ));
            }
            return Ok(GraphNormalization { value: next, changed: true });
        }
        Ok(self.unchanged(value))
    }
}

#[cfg(test)]
pub(super) fn observe_graph_shapes_family() {
    observations::run();
}

#[cfg(test)]
mod observations {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
        ValueRuntime,
    };
    use ara_rpc::{WireString, WireValue};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn own(heap: &Arc<ValueHeap>, value: &ValueHandle, key: WireString, entry: ValueHandle) {
        heap.define_own_data(value, key.into(), entry, &NoRuntime).unwrap();
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: WireString) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn encoded(mut key: WireString, depth: usize) -> WireString {
        for _ in 0..depth {
            key = WireValue::String(key).stringify().into();
        }
        key
    }
    fn getter(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, cap: u64) {
        heap.define_own(
            value,
            key.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(cap))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }
    fn schema(heap: &Arc<ValueHeap>, key: WireString) -> ValueHandle {
        let schema = heap.object().unwrap();
        own(heap, &schema, "type".into(), heap.string("object".into()).unwrap());
        let properties = heap.object().unwrap();
        let leaf = heap.object().unwrap();
        own(heap, &leaf, "type".into(), heap.string("string".into()).unwrap());
        own(heap, &properties, key.clone(), leaf);
        own(heap, &schema, "properties".into(), properties);
        own(heap, &schema, "required".into(), heap.array(vec![heap.string(key).unwrap()]).unwrap());
        schema
    }

    struct Runtime {
        heap: Arc<ValueHeap>,
        iterator: ValueHandle,
        push: ValueHandle,
        pushed_steps: Mutex<Option<ValueHandle>>,
        child: ValueHandle,
        thrown: ValueHandle,
        alternate_key: Mutex<Option<ValueHandle>>,
        array: Mutex<Option<ValueHandle>>,
        events: Mutex<Vec<&'static str>>,
        length_reads: AtomicUsize,
        string_reads: AtomicUsize,
        mode: AtomicUsize,
    }
    impl Runtime {
        fn event(&self, event: &'static str) {
            self.events.lock().unwrap().push(event);
        }
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, receiver, arguments: args } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let heap = &self.heap;
            let value = match capability.0 {
                10 => {
                    self.event("first");
                    heap.define_own_data(receiver, "seen".into(), heap.boolean(true)?, &NoRuntime)?;
                    self.child.clone()
                }
                11 => {
                    self.event("second");
                    self.child.clone()
                }
                12 => {
                    self.event("throw");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                20 => {
                    self.event("length");
                    self.length_reads.fetch_add(1, Ordering::SeqCst);
                    heap.number(2.0)?
                }
                21 => {
                    let at = self.string_reads.fetch_add(1, Ordering::SeqCst);
                    self.event("string");
                    match self.mode.load(Ordering::SeqCst) {
                        1 if at == 0 => return Err(ValueError::Thrown(self.thrown.clone())),
                        1 => heap.string("fallback-key".into())?,
                        2 => return Err(ValueError::MissingCapability(CapabilityId(404))),
                        3 => heap.string(encoded("\"a\\q\"".into(), 1))?,
                        _ => heap.string(encoded("decoded".into(), 1))?,
                    }
                }
                30 => {
                    self.event("index");
                    let array = self.array.lock().unwrap().clone().unwrap();
                    heap.array_set_length(&array, 1, &NoRuntime)?;
                    self.child.clone()
                }
                31 => return Err(ValueError::MissingCapability(CapabilityId(999))),
                40 => {
                    self.event("candidate");
                    let at = self.string_reads.fetch_add(1, Ordering::SeqCst);
                    heap.string(if at == 0 { "first".into() } else { "second".into() })?
                }
                50 => {
                    self.event("push");
                    if self.mode.load(Ordering::SeqCst) == 5 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    if let Some(step) = args.first()
                        && heap.kind(step)? == ValueKind::Object
                    {
                        let kind = heap.get(step, &"kind".into(), &NoRuntime)?;
                        if matches!(heap.snapshot(&kind)?, ValueSnapshot::String(kind) if kind.equals_ascii("index")) {
                            *self.pushed_steps.lock().unwrap() = Some(receiver.clone());
                            heap.define_own_data(step, "index".into(), heap.number(2.0)?, &NoRuntime)?;
                        }
                    }
                    heap.call(&self.push, receiver, args, self)?
                }
                100 => {
                    let first = heap.get(receiver, &"0".into(), &NoRuntime)?;
                    self.event(match heap.kind(&first)? {
                        ValueKind::Undefined => "empty-entries-iterator",
                        ValueKind::Array => "entries-iterator",
                        _ => "pair-iterator",
                    });
                    let alternate = self.alternate_key.lock().unwrap().clone();
                    let source = if matches!(heap.snapshot(&first)?, ValueSnapshot::String(key) if key.equals_ascii("trigger"))
                    {
                        if let Some(key) = alternate {
                            let entry = heap.get(receiver, &"1".into(), &NoRuntime)?;
                            heap.array(vec![key, entry])?
                        } else {
                            receiver.clone()
                        }
                    } else {
                        receiver.clone()
                    };
                    heap.call(&self.iterator, &source, &[], self)?
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    fn binding(heap: &Arc<ValueHeap>, child: ValueHandle) -> (GraphValueFactory, Arc<Runtime>) {
        let runtime = Arc::new(Runtime {
            heap: heap.clone(),
            iterator: heap.get(&heap.array_prototype(), &heap.iterator_key(), &NoRuntime).unwrap(),
            push: heap.get(&heap.array_prototype(), &"push".into(), &NoRuntime).unwrap(),
            pushed_steps: Mutex::new(None),
            child,
            thrown: heap.symbol(Some("original throw".into())).unwrap(),
            alternate_key: Mutex::new(None),
            array: Mutex::new(None),
            events: Mutex::new(Vec::new()),
            length_reads: AtomicUsize::new(0),
            string_reads: AtomicUsize::new(0),
            mode: AtomicUsize::new(0),
        });
        (GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap(), runtime)
    }

    pub(super) fn run() {
        flattened();
        let heap = ValueHeap::new().unwrap();
        let leaf = heap.object().unwrap();
        let (factory, _) = binding(&heap, leaf.clone());
        for key in ["op", "constructor", "toString", "__proto__"] {
            let source = heap.object().unwrap();
            own(&heap, &source, encoded(key.into(), 1), leaf.clone());
            own(&heap, &source, "sibling".into(), leaf.clone());
            let result = normalize_graph_double_encoded_keys(&factory, &source).unwrap();
            assert!(result.changed && !result.value.same_identity(&source));
            assert!(get(&heap, &result.value, key.into()).same_identity(&leaf));
            assert!(get(&heap, &result.value, "sibling".into()).same_identity(&leaf));
            assert!(heap.has_own(&source, &encoded(key.into(), 1).into(), &NoRuntime).unwrap());
            assert!(heap.has_own(&result.value, &WireString::from(key).into(), &NoRuntime).unwrap());
            assert!(
                heap.prototype(&result.value, &NoRuntime).unwrap().unwrap().same_identity(&heap.object_prototype())
            );
        }
        let collision = heap.object().unwrap();
        own(&heap, &collision, "op".into(), leaf.clone());
        own(&heap, &collision, encoded("op".into(), 1), heap.string("keep".into()).unwrap());
        let result = normalize_graph_double_encoded_keys(&factory, &collision).unwrap();
        assert!(!result.changed && result.value.same_identity(&collision));
        let output_collision = heap.object().unwrap();
        own(&heap, &output_collision, encoded("op".into(), 1), leaf.clone());
        own(&heap, &output_collision, encoded("op".into(), 2), heap.string("second".into()).unwrap());
        let result = normalize_graph_double_encoded_keys(&factory, &output_collision).unwrap();
        assert!(result.changed && get(&heap, &result.value, "op".into()).same_identity(&leaf));
        assert!(heap.has_own(&result.value, &encoded("op".into(), 2).into(), &NoRuntime).unwrap());
        let depth = heap.object().unwrap();
        own(&heap, &depth, encoded("op".into(), 4), leaf.clone());
        let result = normalize_graph_double_encoded_keys(&factory, &depth).unwrap();
        assert!(result.changed && heap.has_own(&result.value, &encoded("op".into(), 1).into(), &NoRuntime).unwrap());
        for key in ["plain", "\"a\\q\"", "\"unfinished"] {
            let source = heap.object().unwrap();
            own(&heap, &source, key.into(), leaf.clone());
            let result = normalize_graph_double_encoded_keys(&factory, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
        }
        let lone = WireString::from_units(vec![0xd800]);
        let source = heap.object().unwrap();
        own(&heap, &source, encoded(lone.clone(), 1), leaf.clone());
        let result = normalize_graph_double_encoded_keys(&factory, &source).unwrap();
        assert!(result.changed && get(&heap, &result.value, lone).same_identity(&leaf));
        let scalar = heap.string("ordinary".into()).unwrap();
        assert!(normalize_graph_double_encoded_keys(&factory, &scalar).unwrap().value.same_identity(&scalar));

        // entries captures all getters before actual outer/pair iterator lookup.
        let (factory, runtime) = binding(&heap, leaf.clone());
        heap.define_own_data(
            &heap.array_prototype(),
            heap.iterator_key(),
            heap.function(CapabilityId(100)).unwrap(),
            &NoRuntime,
        )
        .unwrap();
        let source = heap.object().unwrap();
        getter(&heap, &source, "\"first\"", 10);
        getter(&heap, &source, "second", 11);
        let result = normalize_graph_double_encoded_keys(&factory, &source).unwrap();
        assert!(result.changed);
        assert_eq!(
            *runtime.events.lock().unwrap(),
            [
                "first",
                "second",
                "entries-iterator",
                "pair-iterator",
                "empty-entries-iterator",
                "pair-iterator",
                "empty-entries-iterator"
            ]
        );
        runtime.events.lock().unwrap().clear();
        let source = heap.object().unwrap();
        getter(&heap, &source, "first", 10);
        getter(&heap, &source, "failure", 12);
        assert!(
            matches!(normalize_graph_double_encoded_keys(&factory, &source), Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
        assert_eq!(*runtime.events.lock().unwrap(), ["first", "throw"]);
        assert!(heap.has_own(&source, &"seen".into(), &NoRuntime).unwrap());

        // A real pair-iterator replacement supplies an exotic key; parse catch
        // retains Native effects, while missing integration capability escapes.
        let exotic = heap.object().unwrap();
        getter(&heap, &exotic, "length", 20);
        own(&heap, &exotic, "0".into(), heap.string("\"".into()).unwrap());
        own(&heap, &exotic, "1".into(), heap.string("\"".into()).unwrap());
        own(&heap, &exotic, "toString".into(), heap.function(CapabilityId(21)).unwrap());
        *runtime.alternate_key.lock().unwrap() = Some(exotic);
        for mode in 0..4 {
            runtime.mode.store(mode, Ordering::SeqCst);
            runtime.string_reads.store(0, Ordering::SeqCst);
            runtime.length_reads.store(0, Ordering::SeqCst);
            let source = heap.object().unwrap();
            own(&heap, &source, "trigger".into(), leaf.clone());
            let result = normalize_graph_double_encoded_keys(&factory, &source);
            assert_eq!(runtime.length_reads.load(Ordering::SeqCst), 2);
            match mode {
                0 => {
                    let result = result.unwrap();
                    assert!(result.changed && get(&heap, &result.value, "decoded".into()).same_identity(&leaf));
                    assert_eq!(runtime.string_reads.load(Ordering::SeqCst), 1);
                }
                1 => {
                    let result = result.unwrap();
                    assert!(!result.changed && result.value.same_identity(&source));
                    assert_eq!(runtime.string_reads.load(Ordering::SeqCst), 2);
                }
                2 => {
                    assert!(matches!(result, Err(ValueError::MissingCapability(CapabilityId(404)))));
                    assert_eq!(runtime.string_reads.load(Ordering::SeqCst), 1);
                }
                3 => {
                    let result = result.unwrap();
                    assert!(result.changed && get(&heap, &result.value, "\"a\\q\"".into()).same_identity(&leaf));
                }
                _ => unreachable!(),
            }
        }
        // Restore the captured method before testing actual array spread.
        heap.define_own_data(&heap.array_prototype(), heap.iterator_key(), runtime.iterator.clone(), &NoRuntime)
            .unwrap();
        let child = heap.object().unwrap();
        own(&heap, &child, "\"op\"".into(), leaf.clone());
        let (factory, runtime) = binding(&heap, child);
        let array = heap.array_with_length(2).unwrap();
        getter(&heap, &array, "0", 30);
        getter(&heap, &array, "1", 31);
        *runtime.array.lock().unwrap() = Some(array.clone());
        let result = normalize_graph_double_encoded_keys(&factory, &array).unwrap();
        assert!(result.changed && heap.array_length(&result.value, &NoRuntime).unwrap() == 1);
        assert_eq!(*runtime.events.lock().unwrap(), ["index", "index"]);
        assert!(get(&heap, &get(&heap, &result.value, "0".into()), "op".into()).same_identity(&leaf));

        let (factory, runtime) = binding(&heap, leaf.clone());
        let single = schema(&heap, "patch".into());
        let source = heap.object().unwrap();
        own(&heap, &source, "patch".into(), heap.undefined().unwrap());
        getter(&heap, &source, "input", 40);
        own(&heap, &source, "sibling".into(), leaf.clone());
        let symbol = heap.symbol(Some("kept".into())).unwrap();
        let symbol_key = ara_rpc::value_heap::PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap());
        heap.define_own_data(&source, symbol_key.clone(), leaf.clone(), &NoRuntime).unwrap();
        let result = normalize_graph_single_string_field(&factory, &single, &source).unwrap();
        assert!(result.changed);
        assert!(
            matches!(heap.snapshot(&get(&heap, &result.value, "patch".into())).unwrap(), ValueSnapshot::String(text) if text.equals_ascii("first"))
        );
        assert!(!heap.has_own(&result.value, &"input".into(), &NoRuntime).unwrap());
        assert!(get(&heap, &result.value, "sibling".into()).same_identity(&leaf));
        assert!(heap.get(&result.value, &symbol_key, &NoRuntime).unwrap().same_identity(&leaf));
        assert_eq!(*runtime.events.lock().unwrap(), ["candidate", "candidate"]);
        for target in [heap.number(1.0).unwrap(), heap.null().unwrap(), heap.string("present".into()).unwrap()] {
            let source = heap.object().unwrap();
            own(&heap, &source, "patch".into(), target);
            own(&heap, &source, "input".into(), heap.string("payload".into()).unwrap());
            let result = normalize_graph_single_string_field(&factory, &single, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
        }
        let source = heap.object().unwrap();
        own(&heap, &source, "input".into(), heap.string("payload".into()).unwrap());
        let inherited = schema(&heap, "toString".into());
        assert!(!normalize_graph_single_string_field(&factory, &inherited, &source).unwrap().changed);
        let proto = schema(&heap, "__proto__".into());
        own(&heap, &source, "__proto__".into(), heap.undefined().unwrap());
        let result = normalize_graph_single_string_field(&factory, &proto, &source).unwrap();
        assert!(result.changed && heap.has_own(&result.value, &"__proto__".into(), &NoRuntime).unwrap());
        assert!(heap.prototype(&result.value, &NoRuntime).unwrap().unwrap().same_identity(&heap.object_prototype()));
        let no_string = heap.object().unwrap();
        own(&heap, &no_string, "input".into(), heap.number(1.0).unwrap());
        assert!(!normalize_graph_single_string_field(&factory, &single, &no_string).unwrap().changed);
        let properties = get(&heap, &single, "properties".into());
        own(&heap, &properties, "other".into(), heap.object().unwrap());
        assert!(!normalize_graph_single_string_field(&factory, &single, &source).unwrap().changed);
        for schema_value in
            [heap.null().unwrap(), heap.boolean(false).unwrap(), heap.array(vec![]).unwrap(), heap.object().unwrap()]
        {
            let result = normalize_graph_single_string_field(&factory, &schema_value, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
        }
        let valid = schema(&heap, "patch".into());
        for value in [heap.null().unwrap(), heap.array(vec![]).unwrap(), heap.string("root".into()).unwrap()] {
            let result = normalize_graph_single_string_field(&factory, &valid, &value).unwrap();
            assert!(!result.changed && result.value.same_identity(&value));
        }
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(normalize_graph_double_encoded_keys(&factory, &foreign), Err(ValueError::ForeignHeap)));
        assert!(matches!(
            normalize_graph_single_string_field(&factory, &foreign, &source),
            Err(ValueError::ForeignHeap)
        ));
    }

    fn flattened() {
        let heap = ValueHeap::new().unwrap();
        let payload = heap.symbol(Some("leaf identity".into())).unwrap();
        let (factory, _) = binding(&heap, payload.clone());
        let source = heap.object().unwrap();
        own(&heap, &source, "a[003].x".into(), payload.clone());
        own(&heap, &source, "sibling".into(), payload.clone());
        let result = normalize_graph_flattened_array_properties(&factory, &source).unwrap();
        assert!(result.changed && !result.value.same_identity(&source));
        assert!(get(&heap, &result.value, "sibling".into()).same_identity(&payload));
        let array = get(&heap, &result.value, "a".into());
        assert_eq!(heap.array_length(&array, &NoRuntime).unwrap(), 4);
        for index in ["0", "1", "2"] {
            assert!(heap.has_own(&array, &index.into(), &NoRuntime).unwrap());
            assert_eq!(heap.kind(&get(&heap, &array, index.into())).unwrap(), ValueKind::Undefined);
        }
        assert!(get(&heap, &get(&heap, &array, "3".into()), "x".into()).same_identity(&payload));
        for key in ["", "plain", "a.b", "[0].x", "a[foo]", "a[-1]", "a[100001]", "a[1].", "a[1] x"] {
            let source = heap.object().unwrap();
            own(&heap, &source, key.into(), payload.clone());
            let result = normalize_graph_flattened_array_properties(&factory, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
        }
        assert!(
            Normalizer { factory: &factory }
                .shape_parse_flattened(&heap.string("a[100000]".into()).unwrap())
                .unwrap()
                .is_some()
        );
        let skipped = heap.object().unwrap();
        own(&heap, &skipped, "a[0]".into(), heap.undefined().unwrap());
        let result = normalize_graph_flattened_array_properties(&factory, &skipped).unwrap();
        assert!(!result.changed && result.value.same_identity(&skipped));
        let collision = heap.object().unwrap();
        own(&heap, &collision, "a[0]".into(), payload.clone());
        own(&heap, &collision, "a".into(), heap.object().unwrap());
        assert!(!normalize_graph_flattened_array_properties(&factory, &collision).unwrap().changed);
        let own_undefined = heap.object().unwrap();
        own(&heap, &own_undefined, "a".into(), heap.undefined().unwrap());
        own(&heap, &own_undefined, "a[0]".into(), payload.clone());
        assert!(!normalize_graph_flattened_array_properties(&factory, &own_undefined).unwrap().changed);
        assert_eq!(heap.kind(&get(&heap, &own_undefined, "a".into())).unwrap(), ValueKind::Undefined);
        // A later conflict returns the original reference, retaining writes into
        // a shared descendant made by an earlier entry.
        let shared = heap.array(vec![]).unwrap();
        let source = heap.object().unwrap();
        own(&heap, &source, "a".into(), shared.clone());
        own(&heap, &source, "a[0].x".into(), payload.clone());
        own(&heap, &source, "a[0].x.y".into(), heap.number(7.0).unwrap());
        let result = normalize_graph_flattened_array_properties(&factory, &source).unwrap();
        assert!(!result.changed && result.value.same_identity(&source));
        assert!(get(&heap, &get(&heap, &shared, "0".into()), "x".into()).same_identity(&payload));
        let proto = heap.object().unwrap();
        own(&heap, &proto, "__proto__[0].x".into(), payload.clone());
        let result = normalize_graph_flattened_array_properties(&factory, &proto).unwrap();
        assert!(result.changed && heap.has_own(&result.value, &"__proto__".into(), &NoRuntime).unwrap());
        assert!(heap.prototype(&result.value, &NoRuntime).unwrap().unwrap().same_identity(&heap.object_prototype()));
        // A replaced push sees actual path-step objects and changes the index;
        // it is also called to define each intermediate Undefined array slot.
        let (factory, runtime) = binding(&heap, payload.clone());
        own(&heap, &heap.array_prototype(), "push".into(), heap.function(CapabilityId(50)).unwrap());
        let source = heap.object().unwrap();
        own(&heap, &source, "a[0].x".into(), payload.clone());
        let result = normalize_graph_flattened_array_properties(&factory, &source).unwrap();
        assert!(result.changed);
        let array = get(&heap, &result.value, "a".into());
        assert_eq!(heap.array_length(&array, &NoRuntime).unwrap(), 3);
        assert!(get(&heap, &get(&heap, &array, "2".into()), "x".into()).same_identity(&payload));
        assert!(runtime.pushed_steps.lock().unwrap().is_some());
        assert_eq!(runtime.events.lock().unwrap().iter().filter(|event| **event == "push").count(), 6);
        runtime.mode.store(5, Ordering::SeqCst);
        assert!(
            matches!(normalize_graph_flattened_array_properties(&factory, &source), Err(ValueError::Thrown(thrown)) if thrown.same_identity(&runtime.thrown))
        );
    }
}
