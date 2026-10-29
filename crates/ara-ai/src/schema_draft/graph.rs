//! Full fixed OMP schema/draft.ts traversal over actual Graph handles.
//! No serde bridge, structural cache, artificial depth limit or copied
//! equality engine. The per-call conversion map is seeded before recursion.
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

use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_stamps;
use crate::schema_validator::{graph_json_values_equal_with_execution, graph_numeric_less_than_with_execution};
use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, NodeId, PropertyKey, ValueError, ValueHandle,
    ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
};
use std::collections::HashMap;

// Actual Functions supplied at the three fixed draft.ts callback occurrences.
static MERGE_VALUE_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:draft.ts:86",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: merge_value_callback,
};
static NULL_VARIANT_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:draft.ts:153",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 0,
    body: null_variant_callback,
};
static UPGRADE_ENTRY_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:draft.ts:200",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 1,
    body: upgrade_entry_callback,
};
fn callback_argument(heap: &std::sync::Arc<ValueHeap>, arguments: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
    arguments.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn merge_value_callback(
    heap: &std::sync::Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let existing = callback_argument(heap, arguments)?;
    heap.boolean(graph_json_values_equal_with_execution(GraphExecution::new(heap, runtime), &existing, &captures[0])?)
}
fn null_variant_callback(
    heap: &std::sync::Arc<ValueHeap>,
    _captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let variant = callback_argument(heap, arguments)?;
    let read = Draft { factory: GraphExecution::new(heap, runtime), runtime };
    if !read.object(&variant)? {
        return heap.boolean(false);
    }
    read.has_null_type_raw(&read.get(&variant, "type")?)
}
fn upgrade_entry_callback(
    heap: &std::sync::Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let entry = callback_argument(heap, arguments)?;
    let epoch = match heap.snapshot(&captures[0])? {
        ValueSnapshot::Number(epoch) => epoch,
        _ => return Err(ValueError::TypeError("Draft epoch capture is not a Number".into())),
    };
    Draft { factory: GraphExecution::new(heap, runtime), runtime }.needs_upgrade_raw(&entry, epoch)
}

const DRAFT_2020_12: &str = "https://json-schema.org/draft/2020-12/schema";
const DRAFT_07: &[&str] = &[
    "http://json-schema.org/draft-07/schema#",
    "https://json-schema.org/draft-07/schema#",
    "http://json-schema.org/draft-07/schema",
    "https://json-schema.org/draft-07/schema",
];
const SCHEMA_MAP_KEYS: &[&str] = &["properties", "patternProperties", "dependentSchemas"];
const NON_SCHEMA_VALUE_KEYS: &[&str] =
    &["const", "default", "enum", "example", "examples", "required", "dependentRequired", "type"];
const REF_PREFIX: &str = "#/definitions/";

type ConversionCache = HashMap<NodeId, ValueHandle>;

pub fn upgrade_graph_json_schema_to_202012(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    upgrade_graph_json_schema_to_202012_with_runtime(factory, schema, factory.runtime())
}

pub fn upgrade_graph_json_schema_to_202012_with_runtime(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    upgrade_graph_json_schema_to_202012_with_execution(GraphExecution::new(factory.heap(), runtime), schema)
}

pub(crate) fn upgrade_graph_json_schema_to_202012_with_execution(
    execution: GraphExecution<'_>,
    schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    let factory = execution;
    let runtime = execution.runtime();
    factory.heap().adopt(schema)?;
    let read = Draft { factory, runtime };
    if !read.needs_upgrade(schema, schema_stamps::epoch_next())? {
        return Ok(schema.clone());
    }
    read.convert(schema, &mut HashMap::new())
}

#[derive(Clone, Copy)]
struct Draft<'a> {
    factory: GraphExecution<'a>,
    runtime: &'a dyn ValueRuntime,
}

impl Draft<'_> {
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.property(value, &WireString::from(key))
    }
    fn property(&self, value: &ValueHandle, key: &WireString) -> Result<ValueHandle, ValueError> {
        self.factory.heap().get(value, &PropertyKey::String(key.clone()), self.runtime)
    }
    fn assign(&self, value: &ValueHandle, key: WireString, entry: ValueHandle) -> Result<(), ValueError> {
        self.factory.heap().assign(value, PropertyKey::String(key), entry, self.runtime)
    }
    fn undefined(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Undefined)
    }
    fn boolean(&self, value: &ValueHandle, expected: bool) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().snapshot(value)?, ValueSnapshot::Boolean(actual) if actual == expected))
    }
    fn array(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Array)
    }
    fn object(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        // isJsonObject: !!value && typeof object && !Array.isArray, excluding
        // null/functions and accepting boxed/opaque objects in the carrier.
        Ok(self.factory.heap().kind(value)? == ValueKind::Object)
    }
    fn text(&self, value: &ValueHandle) -> Result<Option<WireString>, ValueError> {
        Ok(match self.factory.heap().snapshot(value)? {
            ValueSnapshot::String(value) => Some(value),
            _ => None,
        })
    }
    fn literal(&self, entries: &[(&str, ValueHandle)]) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let result = heap.object()?;
        for (key, value) in entries {
            heap.define_own_data(&result, (*key).into(), value.clone(), self.runtime)?;
        }
        Ok(result)
    }
    fn method(&self, receiver: &ValueHandle, name: &str, arguments: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
        let method = self.get(receiver, name)?;
        self.factory.heap().call(&method, receiver, arguments, self.runtime)
    }
    fn some_raw(
        &self,
        receiver: &ValueHandle,
        spec: &'static CompiledFunctionSpec,
        captures: &[ValueHandle],
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let method = self.get(receiver, "some")?;
        let callback = heap.fresh_compiled_function(spec, captures)?;
        heap.call(&method, receiver, &[callback], self.runtime)
    }
    fn table_has(&self, key: &WireString, own_keys: &[&str]) -> Result<bool, ValueError> {
        if own_keys.iter().any(|known| key.equals_ascii(known)) {
            return Ok(true);
        }
        // Fixed classification tables are ordinary module-private Object
        // literals, so `key in TABLE` includes their Object.prototype chain.
        // Their fixed own fields are inaccessible to schema callbacks; the
        // shared prototype remains live and may be changed by those callbacks.
        self.factory.heap().has(
            &self.factory.heap().object_prototype(),
            &PropertyKey::String(key.clone()),
            self.runtime,
        )
    }
    fn close_error(&self, iterator: &mut IteratorRecord, original: ValueError) -> ValueError {
        match self.factory.heap().iterator_close(iterator, self.runtime) {
            Ok(()) => original,
            Err(close) if original.is_native_exception() && close.is_native_exception() => original,
            Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
        }
    }
    fn for_of<F>(&self, value: &ValueHandle, mut body: F) -> Result<(), ValueError>
    where
        F: FnMut(ValueHandle) -> Result<(), ValueError>,
    {
        let heap = self.factory.heap();
        let mut iterator = heap.get_iterator(value, self.runtime)?;
        while let Some(value) = heap.iterator_next(&mut iterator, self.runtime)? {
            if let Err(error) = body(value) {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(())
    }
    fn spread(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let result = heap.array_with_length(0)?;
        let mut index = 0_usize;
        self.for_of(value, |value| {
            heap.define_own_data(&result, WireString::from(index.to_string()).into(), value, self.runtime)?;
            index = index.checked_add(1).ok_or(ValueError::UnsupportedOperation("Draft spread index overflow"))?;
            Ok(())
        })?;
        Ok(result)
    }
    fn spread_append(&self, value: &ValueHandle, tail: ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let result = self.spread(value)?;
        let length = self.get(&result, "length")?;
        let key = heap.to_string(&length, self.runtime)?;
        heap.define_own_data(&result, key.into(), tail, self.runtime)?;
        Ok(result)
    }

    fn map_needs_upgrade(&self, value: &ValueHandle, epoch: f64) -> Result<bool, ValueError> {
        if !self.object(value)? {
            return Ok(false);
        }
        for key in self.factory.heap().for_in_string_keys(value, self.runtime)? {
            if self.needs_upgrade(&self.property(value, &key)?, epoch)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    // Only source control-flow consumers need truthiness. Saved200 returns raw.
    fn needs_upgrade(&self, value: &ValueHandle, epoch: f64) -> Result<bool, ValueError> {
        self.factory.heap().truthy(&self.needs_upgrade_raw(value, epoch)?)
    }
    fn needs_upgrade_raw(&self, value: &ValueHandle, epoch: f64) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        if self.array(value)? {
            if !schema_stamps::once_with_execution(self.factory, value, epoch)? {
                return heap.boolean(false);
            }
            let captured_epoch = heap.number(epoch)?;
            return self.some_raw(value, &UPGRADE_ENTRY_CALLBACK, std::slice::from_ref(&captured_epoch));
        }
        if !self.object(value)? || !schema_stamps::once_with_execution(self.factory, value, epoch)? {
            return heap.boolean(false);
        }
        for key in self.factory.heap().for_in_string_keys(value, self.runtime)? {
            let entry = self.property(value, &key)?;
            if key.equals_ascii("$schema") {
                if let Some(uri) = self.text(&entry)?
                    && self.table_has(&uri, DRAFT_07)?
                {
                    return heap.boolean(true);
                }
                continue;
            }
            if ["definitions", "dependencies", "additionalItems", "nullable"].iter().any(|name| key.equals_ascii(name))
            {
                return heap.boolean(true);
            }
            if key.equals_ascii("$ref") {
                if self.text(&entry)?.is_some() {
                    let prefix = self.factory.heap().string(REF_PREFIX.into())?;
                    if self.factory.heap().truthy(&self.method(&entry, "startsWith", &[prefix])?)? {
                        return heap.boolean(true);
                    }
                }
                continue;
            }
            if key.equals_ascii("items") && self.array(&entry)? {
                return heap.boolean(true);
            }
            if key.equals_ascii("$defs") || self.table_has(&key, SCHEMA_MAP_KEYS)? {
                if self.map_needs_upgrade(&entry, epoch)? {
                    return heap.boolean(true);
                }
                continue;
            }
            if self.table_has(&key, NON_SCHEMA_VALUE_KEYS)? {
                continue;
            }
            if self.needs_upgrade(&entry, epoch)? {
                return heap.boolean(true);
            }
        }
        heap.boolean(false)
    }

    fn object_map(&self, target: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        let existing = self.get(target, key)?;
        if self.object(&existing)? {
            return Ok(existing);
        }
        let next = self.factory.heap().object()?;
        self.assign(target, key.into(), next.clone())?;
        Ok(next)
    }
    fn merge_schema_map(
        &self,
        target: &ValueHandle,
        key: &str,
        value: &ValueHandle,
        cache: &mut ConversionCache,
    ) -> Result<(), ValueError> {
        let map = self.object_map(target, key)?;
        for name in self.factory.heap().for_in_string_keys(value, self.runtime)? {
            let converted = self.convert(&self.property(value, &name)?, cache)?;
            self.assign(&map, name, converted)?;
        }
        Ok(())
    }
    fn combine(&self, left: ValueHandle, right: ValueHandle) -> Result<ValueHandle, ValueError> {
        if self.undefined(&left)? || self.boolean(&left, true)? {
            return Ok(right);
        }
        if self.undefined(&right)? || self.boolean(&right, true)? {
            return Ok(left);
        }
        if self.boolean(&left, false)? || self.boolean(&right, false)? {
            return self.factory.heap().boolean(false);
        }
        if graph_json_values_equal_with_execution(self.factory, &left, &right)? {
            return Ok(left);
        }
        self.literal(&[("allOf", self.factory.heap().array(vec![left, right])?)])
    }
    fn merge_array_values(&self, left: &ValueHandle, right: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let merged = self.spread(left)?;
        self.for_of(right, |value| {
            let result = self.some_raw(&merged, &MERGE_VALUE_CALLBACK, std::slice::from_ref(&value))?;
            if !self.factory.heap().truthy(&result)? {
                self.method(&merged, "push", &[value])?;
            }
            Ok(())
        })?;
        Ok(merged)
    }
    fn merge_prefix_items(&self, existing: &ValueHandle, converted: &ValueHandle) -> Result<ValueHandle, ValueError> {
        if !self.array(existing)? {
            return Ok(converted.clone());
        }
        let heap = self.factory.heap();
        let merged = self.spread(existing)?;
        let mut index = 0_usize;
        while graph_numeric_less_than_with_execution(self.factory, index as f64, &self.get(converted, "length")?)? {
            let key = WireString::from(index.to_string());
            let value = if heap.has(&merged, &PropertyKey::String(key.clone()), self.runtime)? {
                self.combine(self.property(&merged, &key)?, self.property(converted, &key)?)?
            } else {
                self.property(converted, &key)?
            };
            self.assign(&merged, key, value)?;
            index = index.checked_add(1).ok_or(ValueError::UnsupportedOperation("Draft tuple index overflow"))?;
        }
        Ok(merged)
    }
    fn merge_dependent_required(
        &self,
        target: &ValueHandle,
        key: WireString,
        deps: ValueHandle,
    ) -> Result<(), ValueError> {
        let required = self.object_map(target, "dependentRequired")?;
        let existing = self.property(&required, &key)?;
        if self.undefined(&existing)? {
            return self.assign(&required, key, deps);
        }
        if self.array(&existing)? {
            let merged = self.merge_array_values(&existing, &deps)?;
            self.assign(&required, key, merged)?;
        }
        Ok(())
    }
    fn dependencies(
        &self,
        source: &ValueHandle,
        target: &ValueHandle,
        cache: &mut ConversionCache,
    ) -> Result<(), ValueError> {
        let dependencies = self.get(source, "dependencies")?;
        if !self.object(&dependencies)? {
            return Ok(());
        }
        for key in self.factory.heap().for_in_string_keys(&dependencies, self.runtime)? {
            let converted = self.convert(&self.property(&dependencies, &key)?, cache)?;
            if self.array(&converted)? {
                self.merge_dependent_required(target, key, converted)?;
            } else {
                let schemas = self.object_map(target, "dependentSchemas")?;
                let combined = self.combine(self.property(&schemas, &key)?, converted)?;
                self.assign(&schemas, key, combined)?;
            }
        }
        Ok(())
    }
    fn has_null_type_raw(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        if self.text(value)?.is_some_and(|value| value.equals_ascii("null")) {
            return heap.boolean(true);
        }
        if !self.array(value)? {
            return heap.boolean(false);
        }
        let null = heap.string("null".into())?;
        self.method(value, "includes", std::slice::from_ref(&null))
    }
    fn has_null_variant_raw(&self, variants: &ValueHandle) -> Result<ValueHandle, ValueError> {
        self.some_raw(variants, &NULL_VARIANT_CALLBACK, &[])
    }
    fn make_nullable(&self, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let kind = self.get(schema, "type")?;
        if let Some(text) = self.text(&kind)? {
            if !text.equals_ascii("null") {
                self.assign(schema, "type".into(), heap.array(vec![kind, heap.string("null".into())?])?)?;
            }
            return Ok(schema.clone());
        }
        if self.array(&kind)? {
            let null = heap.string("null".into())?;
            if !heap.truthy(&self.method(&kind, "includes", std::slice::from_ref(&null))?)? {
                self.assign(schema, "type".into(), self.spread_append(&kind, null)?)?;
            }
            return Ok(schema.clone());
        }
        if self.array(&self.get(schema, "anyOf")?)? {
            if !heap.truthy(&self.has_null_variant_raw(&self.get(schema, "anyOf")?)?)? {
                // Do not fold the three Native anyOf Gets into one snapshot.
                let variants = self.get(schema, "anyOf")?;
                let null = self.literal(&[("type", heap.string("null".into())?)])?;
                self.assign(schema, "anyOf".into(), self.spread_append(&variants, null)?)?;
            }
            return Ok(schema.clone());
        }
        let null = self.literal(&[("type", heap.string("null".into())?)])?;
        self.literal(&[("anyOf", heap.array(vec![schema.clone(), null])?)])
    }
    fn convert_ref(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let prefix = heap.string(REF_PREFIX.into())?;
        if !heap.truthy(&self.method(value, "startsWith", &[prefix])?)? {
            return Ok(value.clone());
        }
        let offset = heap.number(REF_PREFIX.len() as f64)?;
        let tail = self.method(value, "slice", &[offset])?;
        if heap.kind(&tail)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
        }
        let tail = heap.to_string(&tail, self.runtime)?;
        let mut units = WireString::from("#/$defs/").units().to_vec();
        units.extend_from_slice(tail.units());
        heap.string(WireString::from_units(units))
    }
    fn convert(&self, value: &ValueHandle, cache: &mut ConversionCache) -> Result<ValueHandle, ValueError> {
        if self.array(value)? {
            if let Some(cached) = cache.get(&value.node_id()) {
                return Ok(cached.clone());
            }
            let result = self.factory.heap().array_with_length(0)?;
            cache.insert(value.node_id(), result.clone());
            self.for_of(value, |entry| {
                // Native evaluates the call's method Get before its recursive
                // argument. Retain that callable across conversion effects.
                let push = self.get(&result, "push")?;
                let converted = self.convert(&entry, cache)?;
                self.factory.heap().call(&push, &result, &[converted], self.runtime)?;
                Ok(())
            })?;
            return Ok(result);
        }
        if !self.object(value)? {
            return Ok(value.clone());
        }
        if let Some(cached) = cache.get(&value.node_id()) {
            return Ok(cached.clone());
        }
        self.convert_object(value, cache)
    }
    fn convert_object(&self, value: &ValueHandle, cache: &mut ConversionCache) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let result = heap.object()?;
        cache.insert(value.node_id(), result.clone());
        for key in heap.for_in_string_keys(value, self.runtime)? {
            let entry = self.property(value, &key)?;
            self.convert_entry(&result, key, entry, cache)?;
        }
        // Each Get mirrors its separate occurrence in the fixed source.
        if self.array(&self.get(value, "items")?)? {
            let converted = self.convert(&self.get(value, "items")?, cache)?;
            let prefix = self.merge_prefix_items(&self.get(&result, "prefixItems")?, &converted)?;
            self.assign(&result, "prefixItems".into(), prefix)?;
            if !self.undefined(&self.get(value, "additionalItems")?)?
                && !self.boolean(&self.get(value, "additionalItems")?, true)?
            {
                let items = self.convert(&self.get(value, "additionalItems")?, cache)?;
                self.assign(&result, "items".into(), items)?;
            } else if !heap.delete_own(&result, &"items".into(), self.runtime)? {
                return Err(ValueError::TypeError("Cannot delete draft result.items".into()));
            }
        }
        self.dependencies(value, &result, cache)?;
        if self.boolean(&self.get(value, "nullable")?, true)? {
            let nullable = self.make_nullable(&result)?;
            if !nullable.same_identity(&result) {
                cache.insert(value.node_id(), nullable.clone());
            }
            return Ok(nullable);
        }
        Ok(result)
    }
    fn convert_entry(
        &self,
        result: &ValueHandle,
        key: WireString,
        entry: ValueHandle,
        cache: &mut ConversionCache,
    ) -> Result<(), ValueError> {
        if key.equals_ascii("definitions") || key.equals_ascii("$defs") {
            if self.object(&entry)? {
                self.merge_schema_map(result, "$defs", &entry, cache)?;
            }
            return Ok(());
        }
        if self.table_has(&key, SCHEMA_MAP_KEYS)? {
            if self.object(&entry)? {
                // Source map keys may include inherited prototype names.
                let map = self.property(result, &key)?;
                let map = if self.object(&map)? {
                    map
                } else {
                    let next = self.factory.heap().object()?;
                    self.assign(result, key.clone(), next.clone())?;
                    next
                };
                for name in self.factory.heap().for_in_string_keys(&entry, self.runtime)? {
                    let converted = self.convert(&self.property(&entry, &name)?, cache)?;
                    self.assign(&map, name, converted)?;
                }
            } else {
                self.assign(result, key, entry)?;
            }
            return Ok(());
        }
        if self.table_has(&key, NON_SCHEMA_VALUE_KEYS)? {
            return self.assign(result, key, entry);
        }
        if ["dependencies", "additionalItems", "nullable"].iter().any(|name| key.equals_ascii(name)) {
            return Ok(());
        }
        if key.equals_ascii("$schema") {
            let converted = if let Some(uri) = self.text(&entry)?
                && self.table_has(&uri, DRAFT_07)?
            {
                self.factory.heap().string(DRAFT_2020_12.into())?
            } else {
                entry
            };
            return self.assign(result, key, converted);
        }
        if key.equals_ascii("$ref") && self.text(&entry)?.is_some() {
            let converted = self.convert_ref(&entry)?;
            return self.assign(result, key, converted);
        }
        if key.equals_ascii("items") && self.array(&entry)? {
            return Ok(());
        }
        let converted = self.convert(&entry, cache)?;
        self.assign(result, key, converted)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ara_rpc::WireValue;
    use ara_rpc::value_heap::{CallableTarget, CapabilityId, PropertyDescriptor, RuntimeReply, RuntimeRequest};
    use ara_rpc::value_heap::{NoRuntime, StringifyMode, ValueHeap};
    use serde_json::{Value, json};
    use std::sync::{Arc, Mutex, Weak};

    fn read(factory: &GraphValueFactory, value: &ValueHandle, key: &str) -> ValueHandle {
        factory.heap().get(value, &key.into(), factory.runtime()).unwrap()
    }
    fn json_value(factory: &GraphValueFactory, value: &ValueHandle) -> Value {
        let text = factory.heap().stringify(value, StringifyMode::Plain, None, factory.runtime()).unwrap().unwrap();
        serde_json::from_str(&text.to_utf8().unwrap()).unwrap()
    }

    // Invoked by an existing validation coarse family, with no new test entry.
    pub(crate) fn conversion_identity_and_original_keyword_families() {
        retained_callback_observations();
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let raw = heap
            .import_strict(
                WireValue::parse(
                    r##"{
            "definitions":{"A":{"type":"string","nullable":true}},
            "$defs":{"A":{"type":"integer"}},
            "$schema":"http://json-schema.org/draft-07/schema#",
            "$ref":"#/definitions/A",
            "type":"object",
            "items":[{"type":"number"}],
            "prefixItems":[{"minimum":1}],
            "additionalItems":false,
            "dependentRequired":{"need":["a"]},
            "dependentSchemas":{"dep":{"type":"object"}},
            "dependencies":{"need":["a","b"],"dep":{"required":["x"]},"off":false},
            "properties":{
                "x":{"type":["string"],"nullable":true},
                "y":{"anyOf":[{"type":"integer"}],"nullable":true},
                "z":{"nullable":true,"description":"z"},
                "remote":{"$ref":"https://host/definitions/A"}
            },
            "enum":[{"nullable":true}],
            "const":{"definitions":{"literal":true}},
            "default":{"additionalItems":false},
            "example":{"dependencies":{}},
            "examples":[{"nullable":true}],
            "required":["need"],
            "nullable":false
        }"##,
                )
                .unwrap(),
            )
            .unwrap();
        let converted = upgrade_graph_json_schema_to_202012(&factory, &raw).unwrap();
        assert!(!converted.same_identity(&raw));
        let value = json_value(&factory, &converted);
        assert_eq!(value["$schema"], DRAFT_2020_12);
        assert_eq!(value["$ref"], "#/$defs/A");
        assert_eq!(value["$defs"]["A"], json!({"type":"integer"}));
        assert_eq!(value["prefixItems"], json!([{"allOf":[{"minimum":1},{"type":"number"}]}]));
        assert_eq!(value["items"], false);
        assert_eq!(value["dependentRequired"]["need"], json!(["a", "b"]));
        assert_eq!(value["dependentSchemas"]["dep"], json!({"allOf":[{"type":"object"},{"required":["x"]}]}));
        assert_eq!(value["dependentSchemas"]["off"], false);
        assert_eq!(value["properties"]["x"]["type"], json!(["string", "null"]));
        assert_eq!(value["properties"]["y"]["anyOf"], json!([{"type":"integer"},{"type":"null"}]));
        assert_eq!(value["properties"]["z"], json!({"anyOf":[{"description":"z"},{"type":"null"}]}));
        assert_eq!(value["properties"]["remote"]["$ref"], "https://host/definitions/A");
        assert!(!value.as_object().unwrap().contains_key("definitions"));
        assert!(!value.as_object().unwrap().contains_key("dependencies"));
        assert!(!value.as_object().unwrap().contains_key("nullable"));
        for key in ["enum", "const", "default", "example", "examples", "required", "type"] {
            assert!(read(&factory, &raw, key).same_identity(&read(&factory, &converted, key)), "literal key {key}");
        }

        let unchanged = heap.import_serde(&json!({"type":["string","null"],"enum":[{"definitions":{}}]})).unwrap();
        assert!(upgrade_graph_json_schema_to_202012(&factory, &unchanged).unwrap().same_identity(&unchanged));

        // Mutable no-op cycle terminates through the source epoch stamp.
        let cycle = heap.object().unwrap();
        heap.define_own_data(&cycle, "self".into(), cycle.clone(), &NoRuntime).unwrap();
        assert!(upgrade_graph_json_schema_to_202012(&factory, &cycle).unwrap().same_identity(&cycle));
        heap.define_own_data(&cycle, "nullable".into(), heap.boolean(false).unwrap(), &NoRuntime).unwrap();
        let knot = upgrade_graph_json_schema_to_202012(&factory, &cycle).unwrap();
        assert!(!knot.same_identity(&cycle));
        assert!(read(&factory, &knot, "self").same_identity(&knot));

        // Nullable wrappers replace the cache target; both DAG paths receive
        // the same wrapper, while literal user payloads retain original handles.
        let shared = heap.import_serde(&json!({"description":"shared","nullable":true})).unwrap();
        let properties = heap.object().unwrap();
        for key in ["a", "b"] {
            heap.define_own_data(&properties, key.into(), shared.clone(), &NoRuntime).unwrap();
        }
        let dag = heap.object().unwrap();
        heap.define_own_data(&dag, "properties".into(), properties, &NoRuntime).unwrap();
        heap.define_own_data(&dag, "const".into(), shared.clone(), &NoRuntime).unwrap();
        let upgraded = upgrade_graph_json_schema_to_202012(&factory, &dag).unwrap();
        let properties = read(&factory, &upgraded, "properties");
        let a = read(&factory, &properties, "a");
        assert!(a.same_identity(&read(&factory, &properties, "b")));
        assert!(!a.same_identity(&shared));
        assert!(read(&factory, &upgraded, "const").same_identity(&shared));

        // `in` on the source's ordinary classification tables includes their
        // intrinsic prototype. A literal-only matcher would fail both cases.
        let inherited_uri = heap.import_serde(&json!({"$schema":"constructor"})).unwrap();
        let converted = upgrade_graph_json_schema_to_202012(&factory, &inherited_uri).unwrap();
        assert_eq!(json_value(&factory, &converted)["$schema"], DRAFT_2020_12);
        let inherited_key =
            heap.import_serde(&json!({"constructor":{"definitions":{"A":true}},"nullable":false})).unwrap();
        let converted = upgrade_graph_json_schema_to_202012(&factory, &inherited_key).unwrap();
        let constructor = read(&factory, &converted, "constructor");
        assert!(heap.has_own(&constructor, &"definitions".into(), &NoRuntime).unwrap());
        assert!(!heap.has_own(&constructor, &"$defs".into(), &NoRuntime).unwrap());
    }

    // These observations intercept the actual selected method, never directly
    // invoke a static spec as a substitute for source callsite linkage.
    struct CaptureSome {
        heap: Weak<ValueHeap>,
        original: ValueHandle,
        functions: Mutex<Vec<ValueHandle>>,
    }
    impl ValueRuntime for CaptureSome {
        fn resolve(&self, _request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            Err(ValueError::UnsupportedOperation("Draft capture has no extra Host capability"))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if capability != CapabilityId(9200) {
                return Err(ValueError::MissingCapability(capability));
            }
            self.functions.lock().unwrap().push(arguments[0].clone());
            self.heap.upgrade().unwrap().call(&self.original, receiver, arguments, runtime).map(RuntimeReply::Value)
        }
    }
    fn own(heap: &Arc<ValueHeap>, receiver: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(receiver, key.into(), value, &NoRuntime).unwrap();
    }
    fn capture_some(heap: &Arc<ValueHeap>, action: impl FnOnce(&GraphValueFactory)) -> Vec<ValueHandle> {
        let prototype = heap.array_prototype();
        let original = heap.get(&prototype, &"some".into(), &NoRuntime).unwrap();
        let runtime = Arc::new(CaptureSome {
            heap: Arc::downgrade(heap),
            original: original.clone(),
            functions: Mutex::new(Vec::new()),
        });
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        own(heap, &prototype, "some", heap.function(CapabilityId(9200)).unwrap());
        action(&factory);
        own(heap, &prototype, "some", original);
        drop(factory);
        let functions = std::mem::take(&mut *runtime.functions.lock().unwrap());
        drop(runtime);
        functions
    }
    fn invoke(
        heap: &Arc<ValueHeap>,
        function: &ValueHandle,
        argument: &ValueHandle,
        runtime: &dyn ValueRuntime,
    ) -> ValueHandle {
        heap.call(function, &heap.undefined().unwrap(), std::slice::from_ref(argument), runtime).unwrap()
    }
    fn reader(factory: &GraphValueFactory) -> Draft<'_> {
        Draft { factory: GraphExecution::from_factory(factory), runtime: factory.runtime() }
    }
    struct LaterRuntime {
        heap: Weak<ValueHeap>,
        types: ValueHandle,
        output: Mutex<ValueHandle>,
        events: Mutex<Vec<u64>>,
        reenter: Mutex<Option<(ValueHandle, ValueHandle)>>,
        thrown: Mutex<Option<ValueHandle>>,
    }
    impl ValueRuntime for LaterRuntime {
        fn resolve(&self, _request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            Err(ValueError::UnsupportedOperation("Draft later capability is unavailable"))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            _arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.events.lock().unwrap().push(capability.0);
            let heap = self.heap.upgrade().unwrap();
            match capability.0 {
                9300 => heap.number(2.0).map(RuntimeReply::Value),
                9301 => {
                    let reenter = self.reenter.lock().unwrap().take();
                    if let Some((function, argument)) = reenter {
                        let nested =
                            heap.call(&function, &heap.undefined()?, std::slice::from_ref(&argument), runtime)?;
                        assert!(nested.same_identity(&self.output.lock().unwrap()));
                    }
                    Ok(RuntimeReply::Value(self.types.clone()))
                }
                9302 => {
                    assert!(receiver.same_identity(&self.types));
                    Ok(RuntimeReply::Value(self.output.lock().unwrap().clone()))
                }
                9303 => Ok(RuntimeReply::Value(self.output.lock().unwrap().clone())),
                9304 => Err(ValueError::Thrown(self.thrown.lock().unwrap().as_ref().unwrap().clone())),
                _ => Err(ValueError::MissingCapability(capability)),
            }
        }
    }
    fn getter(heap: &Arc<ValueHeap>, receiver: &ValueHandle, name: &str, capability: u64) {
        heap.define_own(
            receiver,
            name.into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(capability))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
    }
    fn retained_callback_observations() {
        let weak = {
            let heap = ValueHeap::new().unwrap();
            let left_value = heap.import_serde(&json!({"x":1})).unwrap();
            let right_value = heap.import_serde(&json!({"x":1})).unwrap();
            let left = heap.array(vec![left_value.clone()]).unwrap();
            let right = heap.array(vec![right_value.clone()]).unwrap();
            let merge = capture_some(&heap, |factory| {
                reader(factory).merge_array_values(&left, &right).unwrap();
                reader(factory).merge_array_values(&left, &right).unwrap();
            });
            assert_eq!(merge.len(), 2);
            assert!(!merge[0].same_identity(&merge[1]));
            own(&heap, &right_value, "x", heap.number(2.0).unwrap());
            own(&heap, &right, "0", heap.import_serde(&json!({"x":1})).unwrap());
            assert!(!heap.truthy(&invoke(&heap, &merge[0], &left_value, &NoRuntime)).unwrap());
            let matching = heap.import_serde(&json!({"x":2})).unwrap();
            assert!(heap.truthy(&invoke(&heap, &merge[0], &matching, &NoRuntime)).unwrap());

            let schema = heap.import_serde(&json!({"anyOf":[{"type":["string"]}]})).unwrap();
            let null = capture_some(&heap, |factory| {
                reader(factory).make_nullable(&schema).unwrap();
            });
            assert_eq!(null.len(), 1);
            assert!(!null[0].same_identity(&merge[0]));

            let epoch = schema_stamps::epoch_next();
            let parent = heap.array(vec![heap.boolean(false).unwrap()]).unwrap();
            let upgrade = capture_some(&heap, |factory| {
                assert!(!reader(factory).needs_upgrade(&parent, epoch).unwrap());
            });
            assert_eq!(upgrade.len(), 1);
            assert!(!upgrade[0].same_identity(&null[0]));
            // Advance an unrelated top-level precheck; the saved200 must retain
            // its old Number rather than reconstruct the latest global epoch.
            let unrelated = heap.object().unwrap();
            let temporary = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
            assert!(upgrade_graph_json_schema_to_202012(&temporary, &unrelated).unwrap().same_identity(&unrelated));
            drop(temporary);

            let types = heap.array(vec![heap.string("string".into()).unwrap()]).unwrap();
            own(&heap, &types, "includes", heap.function(CapabilityId(9302)).unwrap());
            let variant = heap.object().unwrap();
            getter(&heap, &variant, "type", 9301);
            let object_sentinel = heap.object().unwrap();
            let runtime = LaterRuntime {
                heap: Arc::downgrade(&heap),
                types: types.clone(),
                output: Mutex::new(object_sentinel.clone()),
                events: Mutex::new(Vec::new()),
                reenter: Mutex::new(None),
                thrown: Mutex::new(None),
            };
            getter(&heap, &matching, "x", 9300);
            assert!(heap.truthy(&invoke(&heap, &merge[0], &matching, &runtime)).unwrap());
            assert_eq!(*runtime.events.lock().unwrap(), vec![9300]);
            for sentinel in [
                heap.string("raw includes".into()).unwrap(),
                object_sentinel.clone(),
                heap.undefined().unwrap(),
                heap.string("".into()).unwrap(),
            ] {
                *runtime.output.lock().unwrap() = sentinel.clone();
                runtime.events.lock().unwrap().clear();
                let result = invoke(&heap, &null[0], &variant, &runtime);
                assert!(result.same_identity(&sentinel));
                assert_eq!(*runtime.events.lock().unwrap(), vec![9301, 9302]);
            }
            *runtime.output.lock().unwrap() = object_sentinel.clone();
            *runtime.reenter.lock().unwrap() = Some((null[0].clone(), variant.clone()));
            runtime.events.lock().unwrap().clear();
            assert!(invoke(&heap, &null[0], &variant, &runtime).same_identity(&object_sentinel));
            assert_eq!(*runtime.events.lock().unwrap(), vec![9301, 9301, 9302, 9302]);

            let nested = heap.array(vec![]).unwrap();
            own(&heap, &nested, "some", heap.function(CapabilityId(9303)).unwrap());
            runtime.events.lock().unwrap().clear();
            assert!(invoke(&heap, &upgrade[0], &nested, &runtime).same_identity(&object_sentinel));
            let epoch_key = heap.module_symbol("ara-ai.schema.stamps", "pi.schema.epoch").unwrap();
            assert!(!heap.has_own(&nested, &epoch_key, &NoRuntime).unwrap());
            assert!(matches!(
                heap.snapshot(&invoke(&heap, &upgrade[0], &nested, &runtime)).unwrap(),
                ValueSnapshot::Boolean(false)
            ));
            assert_eq!(*runtime.events.lock().unwrap(), vec![9303]);

            let thrown = heap.object().unwrap();
            *runtime.thrown.lock().unwrap() = Some(thrown.clone());
            own(&heap, &types, "includes", heap.function(CapabilityId(9304)).unwrap());
            runtime.events.lock().unwrap().clear();
            assert!(
                matches!(heap.call(&null[0], &heap.undefined().unwrap(), std::slice::from_ref(&variant), &runtime), Err(ValueError::Thrown(value)) if value.same_identity(&thrown))
            );
            assert_eq!(*runtime.events.lock().unwrap(), vec![9301, 9304]);
            Arc::downgrade(&heap)
        };
        assert!(weak.upgrade().is_none());
    }
}
