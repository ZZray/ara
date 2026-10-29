//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/validation.ts:833-975,998-1087,1199-1330.
//! Actual schema/ref, identifier and string-encoded-array leaf passes.
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
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::schema_validator::{graph_numeric_less_than, validate_graph_json_schema_value_with_execution};
use ara_rpc::WireString;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, PropertyKey, ValueError, ValueHandle, ValueHeap,
    ValueKind, ValueRuntime, ValueSnapshot,
};

const MODULE: &str = "ara-ai.validation";
const IDENTIFIER_KEYS: &[&str] =
    &["path", "paths", "file", "file_path", "filePath", "filepath", "url", "uri", "title", "label"];
const CONTENT_KEYS: &[&str] = &["content", "input", "body", "text", "command", "code"];

pub fn normalize_graph_enum_string_whitespace(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    let heap = factory.heap();
    heap.adopt(schema)?;
    heap.adopt(value)?;
    // Native's default parameter is new Set(), not new Set([]): no add or
    // iterator acquisition happens until a resolved reference is appended.
    let refs = heap.new_set(&heap.undefined()?, factory.runtime())?;
    Normalizer { factory }.schema_enum(schema, value, schema, &refs)
}

#[cfg(test)]
pub(super) fn observe_graph_schema_normalizers_family() {
    observations::run();
    observe_retained_schema_normalizers();
}

pub fn normalize_graph_identifier_string_whitespace(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(value)?;
    let constants = IdentifierConstants::load(factory)?;
    Normalizer { factory }.schema_identifier(value, &constants)
}

pub fn normalize_graph_string_encoded_array_unions(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(schema)?;
    factory.heap().adopt(value)?;
    Normalizer { factory }.schema_encoded_array(schema, value)
}

static ENUM_BRANCH_CALLBACK: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "omp:596f2da7101178214aa27a753529d15e6b7ad91d:validation.ts:881",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 3,
    body: enum_branch_callback,
};
fn enum_branch_callback(
    heap: &std::sync::Arc<ValueHeap>,
    captures: &[ValueHandle],
    _receiver: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle, ValueError> {
    let branch = args.first().cloned().map(Ok).unwrap_or_else(|| heap.undefined())?;
    let read = ReferenceReads { execution: GraphExecution::new(heap, runtime) };
    heap.boolean(read.branch_matches(&captures[1], &captures[2], &branch, &captures[0])?)
}

struct IdentifierConstants {
    identifier: ValueHandle,
    content: ValueHandle,
    terminator: ValueHandle,
}

impl IdentifierConstants {
    fn load(factory: &GraphValueFactory) -> Result<Self, ValueError> {
        let heap = factory.heap();
        // Root performs these intrinsic initializers at module startup before
        // author hooks. Lookups retain the real mutable module object identity.
        let identifier = heap.module_string_set(MODULE, "IDENTIFIER_STRING_KEYS", IDENTIFIER_KEYS)?;
        let content = heap.module_string_set(MODULE, "CONTENT_CARRYING_KEYS", CONTENT_KEYS)?;
        let terminator = heap.module_regexp(MODULE, "TRAILING_LINE_TERMINATOR_RE", r"[\r\n]+$", "")?;
        Ok(Self { identifier, content, terminator })
    }
}

/// Only reference resolution and branch validation run under some's effective
/// callback runtime. Reuse the checker engine; keep no callback/runtime in heap
/// nodes and do not manufacture a second factory with a different lineage.
struct ReferenceReads<'a> {
    execution: GraphExecution<'a>,
}

impl ReferenceReads<'_> {
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.execution.heap().get(value, &key.into(), self.execution.runtime())
    }

    fn method(&self, receiver: &ValueHandle, key: &str, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
        let method = self.get(receiver, key)?;
        self.execution.heap().call(&method, receiver, args, self.execution.runtime())
    }

    fn close_error(&self, iterator: &mut IteratorRecord, original: ValueError) -> ValueError {
        match self.execution.heap().iterator_close(iterator, self.execution.runtime()) {
            Ok(()) => original,
            Err(close) if original.is_native_exception() && close.is_native_exception() => original,
            Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
        }
    }

    fn set_has(&self, refs: &ValueHandle, key: &ValueHandle) -> Result<bool, ValueError> {
        self.execution.heap().truthy(&self.method(refs, "has", std::slice::from_ref(key))?)
    }

    fn property_key(&self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let heap = self.execution.heap();
        let primitive = heap.to_primitive(value, true, self.execution.runtime())?;
        if heap.kind(&primitive)? == ValueKind::Symbol {
            Ok(PropertyKey::Symbol(heap.symbol_key(&primitive)?))
        } else {
            Ok(PropertyKey::String(heap.to_string(&primitive, self.execution.runtime())?))
        }
    }

    fn local_ref(&self, root: &ValueHandle, reference: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.execution.heap();
        if matches!(heap.snapshot(reference)?, ValueSnapshot::String(text) if text.equals_ascii("#")) {
            return Ok(root.clone());
        }
        let starts = self.method(reference, "startsWith", &[heap.string("#/".into())?])?;
        if !heap.truthy(&starts)? {
            return heap.undefined();
        }
        let tail = self.method(reference, "slice", &[heap.number(2.0)?])?;
        let tokens = self.method(&tail, "split", &[heap.string("/".into())?])?;
        let mut current = root.clone();
        let mut iterator = heap.get_iterator(&tokens, self.execution.runtime())?;
        while let Some(raw) = heap.iterator_next(&mut iterator, self.execution.runtime())? {
            let step = (|| {
                // Both fresh RegExp literals are observable actual arguments;
                // the second literal is evaluated after the first call returns.
                let replace = self.get(&raw, "replace")?;
                let token = heap.call(
                    &replace,
                    &raw,
                    &[heap.regexp("~1".into(), "g")?, heap.string("/".into())?],
                    self.execution.runtime(),
                )?;
                let replace = self.get(&token, "replace")?;
                let token = heap.call(
                    &replace,
                    &token,
                    &[heap.regexp("~0".into(), "g")?, heap.string("~".into())?],
                    self.execution.runtime(),
                )?;
                if !matches!(heap.kind(&current)?, ValueKind::Object | ValueKind::Array) {
                    return Ok(None);
                }
                // Decode precedes the current-node object check; key coercion
                // happens only after that check and for this actual Get.
                heap.get(&current, &self.property_key(&token)?, self.execution.runtime()).map(Some)
            })();
            match step {
                Ok(Some(value)) => current = value,
                Ok(None) => {
                    heap.iterator_close(&mut iterator, self.execution.runtime())?;
                    return heap.undefined();
                }
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(current)
    }

    fn branch_matches(
        &self,
        root: &ValueHandle,
        refs: &ValueHandle,
        branch: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<bool, ValueError> {
        let heap = self.execution.heap();
        if matches!(heap.kind(branch)?, ValueKind::Object | ValueKind::Array) {
            let reference = self.get(branch, "$ref")?;
            if heap.kind(&reference)? == ValueKind::String && !self.set_has(refs, &reference)? {
                let resolved = self.local_ref(root, &reference)?;
                if heap.kind(&resolved)? != ValueKind::Undefined {
                    // The fixed helper resolves this one reference explicitly;
                    // the checker roots itself at the resolved schema.
                    return Ok(
                        validate_graph_json_schema_value_with_execution(self.execution, &resolved, value)?.success
                    );
                }
            }
        }
        Ok(validate_graph_json_schema_value_with_execution(self.execution, branch, value)?.success)
    }
}

impl Normalizer<'_> {
    fn schema_strict_equal(&self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        match (heap.snapshot(left)?, heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => Ok(left == right),
            _ => heap.object_is(left, right),
        }
    }

    fn schema_index_key(&self, index: f64) -> Result<PropertyKey, ValueError> {
        self.property_key(&self.factory.heap().number(index)?)
    }

    fn schema_append_ref(&self, refs: &ValueHandle, reference: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let out = heap.array_with_length(0)?;
        let mut iterator = heap.get_iterator(refs, runtime)?;
        let mut index = 0_u64;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            if let Err(error) = heap.define_own_data(&out, WireString::from(index.to_string()).into(), entry, runtime) {
                return Err(self.close_error(&mut iterator, error));
            }
            index += 1;
        }
        // [...refs, ref] defines its final own index without Get(length)/push.
        heap.define_own_data(&out, WireString::from(index.to_string()).into(), reference.clone(), runtime)?;
        heap.new_set(&out, runtime)
    }

    fn schema_enum_any(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        root: &ValueHandle,
        refs: &ValueHandle,
        keyword: &str,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let branches = self.get(schema, keyword)?;
        if !self.array(&branches)? {
            return Ok(self.unchanged(value));
        }
        let some = self.get(&branches, "some")?;
        let callback =
            heap.fresh_compiled_function(&ENUM_BRANCH_CALLBACK, &[value.clone(), root.clone(), refs.clone()])?;
        let matches = heap.call(&some, &branches, &[callback], runtime)?;
        if heap.truthy(&matches)? {
            return Ok(self.unchanged(value));
        }
        let read = ReferenceReads { execution: GraphExecution::new(heap, runtime) };
        let mut iterator = heap.get_iterator(&branches, runtime)?;
        while let Some(branch) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let normalized = self.schema_enum(&branch, value, root, refs)?;
                let accepted = normalized.changed && read.branch_matches(root, refs, &branch, &normalized.value)?;
                Ok::<_, ValueError>((normalized, accepted))
            })();
            match result {
                Ok((normalized, true)) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(normalized);
                }
                Ok((_, false)) => {}
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        // No failed candidate is installed by the enum helper.
        Ok(self.unchanged(value))
    }

    fn schema_enum_all(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        root: &ValueHandle,
        refs: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if !self.array(&self.get(schema, "allOf")?)? {
            return Ok(self.unchanged(value));
        }
        let branches = self.get(schema, "allOf")?;
        let mut next = value.clone();
        let mut changed = false;
        let mut iterator = heap.get_iterator(&branches, runtime)?;
        while let Some(branch) = heap.iterator_next(&mut iterator, runtime)? {
            let normalized = match self.schema_enum(&branch, &next, root, refs) {
                Ok(normalized) => normalized,
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            };
            if normalized.changed {
                next = normalized.value;
                changed = true;
            }
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_enum_string(&self, schema: &ValueHandle, value: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let trimmed = self.method(value, "trim", &[])?;
        if !self.schema_strict_equal(&trimmed, value)? {
            let enums = self.get(schema, "enum")?;
            if self.array(&enums)? {
                let original = self.method(&enums, "includes", std::slice::from_ref(value))?;
                if !heap.truthy(&original)? {
                    let trimmed_match = self.method(&enums, "includes", std::slice::from_ref(&trimmed))?;
                    if heap.truthy(&trimmed_match)? {
                        return Ok(GraphNormalization { value: trimmed, changed: true });
                    }
                }
            }
            let constant = self.get(schema, "const")?;
            if heap.kind(&constant)? == ValueKind::String && self.schema_strict_equal(&trimmed, &constant)? {
                return Ok(GraphNormalization { value: trimmed, changed: true });
            }
        }
        Ok(self.unchanged(value))
    }

    fn schema_enum_array(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        root: &ValueHandle,
        refs: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let prefix = self.get(schema, "prefixItems")?;
        let mut next = value.clone();
        let mut changed = false;
        if self.array(&prefix)? {
            let mut index = 0.0;
            while graph_numeric_less_than(self.factory, index, &self.get(value, "length")?)?
                && graph_numeric_less_than(self.factory, index, &self.get(&prefix, "length")?)?
            {
                let key = self.schema_index_key(index)?;
                let item_schema = heap.get(&prefix, &key, runtime)?;
                let current = heap.get(value, &key, runtime)?;
                let normalized = self.schema_enum(&item_schema, &current, root, refs)?;
                if normalized.changed {
                    if !changed {
                        next = self.array_spread(value)?;
                        changed = true;
                    }
                    heap.assign(&next, key, normalized.value, runtime)?;
                }
                index += 1.0;
            }
        }
        let items = self.get(schema, "items")?;
        if !self.null(&items)? && self.typeof_object(&items)? && !self.array(&items)? {
            let mut index = 0.0;
            while graph_numeric_less_than(self.factory, index, &self.get(value, "length")?)? {
                if self.array(&prefix)? && graph_numeric_less_than(self.factory, index, &self.get(&prefix, "length")?)?
                {
                    index += 1.0;
                    continue;
                }
                let key = self.schema_index_key(index)?;
                let current = heap.get(&next, &key, runtime)?;
                let normalized = self.schema_enum(&items, &current, root, refs)?;
                if normalized.changed {
                    if !changed {
                        next = self.array_spread(value)?;
                        changed = true;
                    }
                    heap.assign(&next, key, normalized.value, runtime)?;
                }
                index += 1.0;
            }
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_enum_object(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        root: &ValueHandle,
        refs: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let properties = self.get(schema, "properties")?;
        if !heap.truthy(&properties)? || !self.typeof_object(&properties)? {
            return Ok(self.unchanged(value));
        }
        let entries = heap.object_entries(&properties, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        let mut next = value.clone();
        let mut changed = false;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, property_schema) = self.pair(&entry)?;
                if !heap.has(&next, &self.property_key(&key)?, runtime)? {
                    return Ok(());
                }
                let current = heap.get(&next, &self.property_key(&key)?, runtime)?;
                let normalized = self.schema_enum(&property_schema, &current, root, refs)?;
                if normalized.changed {
                    if !changed {
                        next = heap.spread(&next, runtime)?;
                        changed = true;
                    }
                    heap.assign(&next, self.property_key(&key)?, normalized.value, runtime)?;
                }
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_enum(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        root: &ValueHandle,
        refs: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        if matches!(heap.kind(value)?, ValueKind::Null | ValueKind::Undefined)
            || self.null(schema)?
            || !self.typeof_object(schema)?
        {
            return Ok(self.unchanged(value));
        }
        let reference = self.get(schema, "$ref")?;
        if heap.kind(&reference)? == ValueKind::String {
            let read = ReferenceReads { execution: GraphExecution::from_factory(self.factory) };
            if read.set_has(refs, &reference)? {
                return Ok(self.unchanged(value));
            }
            let resolved = read.local_ref(root, &reference)?;
            if heap.kind(&resolved)? == ValueKind::Undefined {
                return Ok(self.unchanged(value));
            }
            let next_refs = self.schema_append_ref(refs, &reference)?;
            return self.schema_enum(&resolved, value, root, &next_refs);
        }
        let any = self.schema_enum_any(schema, value, root, refs, "anyOf")?;
        if any.changed {
            return Ok(any);
        }
        let one = self.schema_enum_any(schema, value, root, refs, "oneOf")?;
        if one.changed {
            return Ok(one);
        }
        let all = self.schema_enum_all(schema, value, root, refs)?;
        if all.changed {
            return Ok(all);
        }
        if heap.kind(value)? == ValueKind::String {
            return self.schema_enum_string(schema, value);
        }
        if self.array(value)? {
            return self.schema_enum_array(schema, value, root, refs);
        }
        if !self.typeof_object(value)? {
            return Ok(self.unchanged(value));
        }
        self.schema_enum_object(schema, value, root, refs)
    }

    fn schema_trim_identifier(
        &self,
        input: &ValueHandle,
        constants: &IdentifierConstants,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let matched = self.method(&constants.terminator, "test", std::slice::from_ref(input))?;
        if !heap.truthy(&matched)? {
            return Ok(input.clone());
        }
        self.method(input, "replace", &[constants.terminator.clone(), heap.string("".into())?])
    }

    fn schema_identifier_leaf(
        &self,
        input: &ValueHandle,
        constants: &IdentifierConstants,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if heap.kind(input)? == ValueKind::String {
            let trimmed = self.schema_trim_identifier(input, constants)?;
            return Ok(if self.schema_strict_equal(&trimmed, input)? { input.clone() } else { trimmed });
        }
        if !self.array(input)? {
            return Ok(input.clone());
        }
        let mut changed = false;
        let mut next = input.clone();
        let mut index = 0.0;
        while graph_numeric_less_than(self.factory, index, &self.get(input, "length")?)? {
            let key = self.schema_index_key(index)?;
            let entry = heap.get(input, &key, runtime)?;
            if heap.kind(&entry)? == ValueKind::String {
                let trimmed = self.schema_trim_identifier(&entry, constants)?;
                if !self.schema_strict_equal(&trimmed, &entry)? {
                    if !changed {
                        // Native leaf copies via the actual slice method,
                        // whose exotic result and side effects remain live.
                        next = self.method(input, "slice", &[])?;
                        changed = true;
                    }
                    heap.assign(&next, key, trimmed, runtime)?;
                }
            }
            index += 1.0;
        }
        Ok(next)
    }

    fn schema_identifier_array(
        &self,
        value: &ValueHandle,
        constants: &IdentifierConstants,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let mut next = value.clone();
        let mut changed = false;
        let mut index = 0.0;
        while graph_numeric_less_than(self.factory, index, &self.get(value, "length")?)? {
            let key = self.schema_index_key(index)?;
            let current = heap.get(value, &key, runtime)?;
            let normalized = self.schema_identifier(&current, constants)?;
            if normalized.changed {
                if !changed {
                    next = self.array_spread(value)?;
                    changed = true;
                }
                heap.assign(&next, key, normalized.value, runtime)?;
            }
            index += 1.0;
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_identifier_object(
        &self,
        value: &ValueHandle,
        constants: &IdentifierConstants,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let entries = heap.object_entries(value, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        let mut out = value.clone();
        let mut changed = false;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, entry) = self.pair(&entry)?;
                if self.set_has(&constants.content, &key)? {
                    return Ok(());
                }
                let mut next = entry.clone();
                if self.set_has(&constants.identifier, &key)? {
                    let trimmed = self.schema_identifier_leaf(&entry, constants)?;
                    if !self.schema_strict_equal(&trimmed, &entry)? {
                        next = trimmed;
                    }
                }
                let nested = self.schema_identifier(&next, constants)?;
                if nested.changed {
                    next = nested.value;
                }
                if self.schema_strict_equal(&next, &entry)? {
                    return Ok(());
                }
                if !changed {
                    out = heap.spread(value, runtime)?;
                    changed = true;
                }
                heap.assign(&out, self.property_key(&key)?, next, runtime)?;
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: out, changed })
    }

    fn schema_identifier(
        &self,
        value: &ValueHandle,
        constants: &IdentifierConstants,
    ) -> Result<GraphNormalization, ValueError> {
        if self.array(value)? {
            return self.schema_identifier_array(value, constants);
        }
        if self.null(value)? || !self.typeof_object(value)? {
            return Ok(self.unchanged(value));
        }
        self.schema_identifier_object(value, constants)
    }

    fn schema_keyword_literal(&self) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        heap.array(vec![heap.string("anyOf".into())?, heap.string("oneOf".into())?])
    }

    fn schema_union_has_both(&self, branches: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let mut has_string = false;
        let mut has_array = false;
        let mut iterator = heap.get_iterator(branches, runtime)?;
        while let Some(branch) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                if !heap.truthy(&branch)? || !self.typeof_object(&branch)? {
                    return Ok(false);
                }
                let kind = self.get(&branch, "type")?;
                if self.is_text(&kind, "string")?
                    || (self.array(&kind)?
                        && heap.truthy(&self.method(&kind, "includes", &[heap.string("string".into())?])?)?)
                {
                    has_string = true;
                }
                // Keep this check's calls even if a preceding branch already
                // set has_array. Each branch's actual includes can have effects.
                if self.is_text(&kind, "array")?
                    || (self.array(&kind)?
                        && heap.truthy(&self.method(&kind, "includes", &[heap.string("array".into())?])?)?)
                {
                    has_array = true;
                }
                Ok::<_, ValueError>(has_string && has_array)
            })();
            match result {
                Ok(true) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(true);
                }
                Ok(false) => {}
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(false)
    }

    fn schema_accepts_both(&self, schema: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        // Fixed source deliberately Gets schema.type independently at all
        // three expression sites; keep both short-circuit boundaries.
        if self.array(&self.get(schema, "type")?)?
            && heap.truthy(&self.method(&self.get(schema, "type")?, "includes", &[heap.string("string".into())?])?)?
            && heap.truthy(&self.method(&self.get(schema, "type")?, "includes", &[heap.string("array".into())?])?)?
        {
            return Ok(true);
        }
        let keywords = self.schema_keyword_literal()?;
        let mut iterator = heap.get_iterator(&keywords, runtime)?;
        while let Some(key) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let branches = heap.get(schema, &self.property_key(&key)?, runtime)?;
                if !self.array(&branches)? {
                    return Ok(false);
                }
                self.schema_union_has_both(&branches)
            })();
            match result {
                Ok(true) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(true);
                }
                Ok(false) => {}
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(false)
    }

    fn schema_accepts_array(&self, schema: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        if !heap.truthy(schema)? || !self.typeof_object(schema)? {
            return Ok(false);
        }
        let kind = self.get(schema, "type")?;
        Ok(self.is_text(&kind, "array")?
            || (self.array(&kind)?
                && heap.truthy(&self.method(&kind, "includes", &[heap.string("array".into())?])?)?))
    }

    fn schema_matches_array_branches(&self, branches: &ValueHandle, value: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let mut iterator = heap.get_iterator(branches, runtime)?;
        while let Some(branch) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                if !self.schema_accepts_array(&branch)? {
                    return Ok(false);
                }
                self.branch_matches(&branch, value)
            })();
            match result {
                Ok(true) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(true);
                }
                Ok(false) => {}
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(false)
    }

    fn schema_parsed_array_matches(&self, schema: &ValueHandle, value: &ValueHandle) -> Result<bool, ValueError> {
        if self.schema_accepts_array(schema)? {
            // An explicit array shape owns the whole checker result, including
            // rejection. Native does not fall through to individual unions.
            return self.branch_matches(schema, value);
        }
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let keywords = self.schema_keyword_literal()?;
        let mut iterator = heap.get_iterator(&keywords, runtime)?;
        while let Some(key) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let branches = heap.get(schema, &self.property_key(&key)?, runtime)?;
                if !self.array(&branches)? {
                    return Ok(false);
                }
                self.schema_matches_array_branches(&branches, value)
            })();
            match result {
                Ok(true) => {
                    heap.iterator_close(&mut iterator, runtime)?;
                    return Ok(true);
                }
                Ok(false) => {}
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(false)
    }

    fn schema_encoded_array_string(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        // These actual method calls are outside Native's catch.
        let trimmed = self.method(value, "trim", &[])?;
        if !heap.truthy(&self.method(&trimmed, "startsWith", &[heap.string("[".into())?])?)? {
            return Ok(self.unchanged(value));
        }
        let result = (|| {
            if heap.kind(&trimmed)? == ValueKind::Symbol {
                return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
            }
            let text = heap.to_string(&trimmed, self.factory.runtime())?;
            let parsed = heap.parse_strict_units(&text)?;
            if !self.array(&parsed)? {
                return Ok(self.unchanged(value));
            }
            let candidate = super::normalize_graph_double_encoded_keys(self.factory, &parsed)?.value;
            if self.schema_parsed_array_matches(schema, &candidate)? {
                return Ok(GraphNormalization { value: candidate, changed: true });
            }
            Ok(self.unchanged(value))
        })();
        match result {
            Ok(normalized) => Ok(normalized),
            Err(error) if error.is_native_exception() || matches!(&error, ValueError::StrictParse(_)) => {
                Ok(self.unchanged(value))
            }
            Err(boundary) => Err(boundary),
        }
    }

    fn schema_encoded_array_items(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let items = self.get(schema, "items")?;
        if !heap.truthy(&items)? || !self.typeof_object(&items)? || self.array(&items)? {
            return Ok(self.unchanged(value));
        }
        let mut next = value.clone();
        let mut changed = false;
        let mut index = 0.0;
        while graph_numeric_less_than(self.factory, index, &self.get(value, "length")?)? {
            let key = self.schema_index_key(index)?;
            let current = heap.get(value, &key, runtime)?;
            let normalized = self.schema_encoded_array(&items, &current)?;
            if normalized.changed {
                if !changed {
                    next = self.array_spread(value)?;
                    changed = true;
                }
                heap.assign(&next, key, normalized.value, runtime)?;
            }
            index += 1.0;
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_encoded_array_object(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let properties = self.get(schema, "properties")?;
        if !heap.truthy(&properties)? || !self.typeof_object(&properties)? {
            return Ok(self.unchanged(value));
        }
        let entries = heap.object_entries(&properties, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        let mut next = value.clone();
        let mut changed = false;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, property_schema) = self.pair(&entry)?;
                if !heap.has(&next, &self.property_key(&key)?, runtime)? {
                    return Ok(());
                }
                let current = heap.get(&next, &self.property_key(&key)?, runtime)?;
                let normalized = self.schema_encoded_array(&property_schema, &current)?;
                if normalized.changed {
                    if !changed {
                        next = heap.spread(&next, runtime)?;
                        changed = true;
                    }
                    heap.assign(&next, self.property_key(&key)?, normalized.value, runtime)?;
                }
                Ok::<_, ValueError>(())
            })();
            if let Err(error) = result {
                return Err(self.close_error(&mut iterator, error));
            }
        }
        Ok(GraphNormalization { value: next, changed })
    }

    fn schema_encoded_array(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        if matches!(heap.kind(value)?, ValueKind::Null | ValueKind::Undefined)
            || self.null(schema)?
            || !self.typeof_object(schema)?
        {
            return Ok(self.unchanged(value));
        }
        if heap.kind(value)? == ValueKind::String && self.schema_accepts_both(schema)? {
            return self.schema_encoded_array_string(schema, value);
        }
        if self.array(value)? {
            return self.schema_encoded_array_items(schema, value);
        }
        if !self.is_text(&self.get(schema, "type")?, "object")? || !self.typeof_object(value)? || self.null(value)? {
            return Ok(self.unchanged(value));
        }
        self.schema_encoded_array_object(schema, value)
    }
}

#[cfg(test)]
fn observe_retained_schema_normalizers() {
    use super::driver::retained_observations::{LaterRuntime, call, capture, own};
    use ara_rpc::value_heap::{CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor};
    let weak = {
        let heap = ValueHeap::new().unwrap();
        let value = heap.string(" actual ".into()).unwrap();
        let refs = heap.new_set(&heap.undefined().unwrap(), &NoRuntime).unwrap();
        let root = heap.import_serde(&serde_json::json!({"$defs":{"A":{"type":"string"}}})).unwrap();
        let schema = heap.import_serde(&serde_json::json!({"anyOf":[{"$ref":"#/$defs/A"}]})).unwrap();
        let saved = capture(&heap, "some", |factory| {
            let result = Normalizer { factory }.schema_enum_any(&schema, &value, &root, &refs, "anyOf").unwrap();
            assert!(result.value.same_identity(&value));
        });
        assert!(!saved.is_empty());
        let branch = heap.import_serde(&serde_json::json!({"$ref":"#/$defs/A"})).unwrap();
        let defs = heap.get(&root, &"$defs".into(), &NoRuntime).unwrap();
        own(&heap, &defs, "A", heap.import_serde(&serde_json::json!({"type":"integer"})).unwrap());
        assert!(!heap.truthy(&call(&heap, &saved[0], &branch, &NoRuntime)).unwrap());
        own(&heap, &defs, "A", heap.import_serde(&serde_json::json!({"type":"string"})).unwrap());
        own(&heap, &refs, "has", heap.function(CapabilityId(7103)).unwrap());
        heap.define_own(
            &root,
            "$defs".into(),
            PropertyDescriptor::Accessor {
                get: Some(CallableTarget::Host(CapabilityId(7104))),
                set: None,
                enumerable: true,
                configurable: true,
            },
            &NoRuntime,
        )
        .unwrap();
        let runtime = LaterRuntime::new(&heap);
        *runtime.value.lock().unwrap() = Some(defs);
        assert!(heap.truthy(&call(&heap, &saved[0], &branch, &runtime)).unwrap());
        let events = runtime.events.lock().unwrap().clone();
        assert!(events.contains(&7103) && events.contains(&7104));
        std::sync::Arc::downgrade(&heap)
    };
    assert!(weak.upgrade().is_none());
}

#[cfg(test)]
mod observations {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn parse(heap: &Arc<ValueHeap>, text: &str) -> ValueHandle {
        heap.parse_strict_units(&text.into()).unwrap()
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn own(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, entry: ValueHandle) {
        heap.define_own_data(value, key.into(), entry, &NoRuntime).unwrap();
    }
    fn text(heap: &Arc<ValueHeap>, value: &ValueHandle, expected: &str) {
        assert!(
            matches!(heap.snapshot(value).unwrap(), ValueSnapshot::String(value) if value == WireString::from(expected))
        );
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
    fn ordinary() -> (Arc<ValueHeap>, GraphValueFactory) {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        (heap, factory)
    }

    // A few scoped hooks observe actual method selection, live mutation and
    // catch placement. They never replace the checker or regex engine.
    struct Runtime {
        heap: Arc<ValueHeap>,
        some: ValueHandle,
        replace: ValueHandle,
        type_value: ValueHandle,
        thrown: ValueHandle,
        type_reads: AtomicUsize,
        mode: AtomicUsize,
        events: Mutex<Vec<String>>,
    }
    impl Runtime {
        fn event(&self, event: &str) {
            self.events.lock().unwrap().push(event.to_owned());
        }
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let RuntimeRequest::Call { capability, receiver, arguments } = request else {
                return Err(ValueError::MissingCapability(request.capability()));
            };
            let value = match capability.0 {
                101 => return Err(ValueError::TypeError("unexpected empty Set add Get".into())),
                102 => {
                    self.event("identifier.has");
                    self.heap.boolean(false)?
                }
                103 => {
                    self.event("slice");
                    receiver.clone()
                }
                104 => {
                    let read = self.type_reads.fetch_add(1, Ordering::SeqCst) + 1;
                    self.event("schema.type");
                    if read == 4 {
                        if self.mode.load(Ordering::SeqCst) == 1 {
                            return Err(ValueError::MissingCapability(CapabilityId(904)));
                        }
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.type_value.clone()
                }
                105 => {
                    self.event("trim.throw");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                106 => {
                    self.event("startsWith.throw");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                107 => {
                    self.event("regex.test");
                    self.heap.boolean(true)?
                }
                108 => {
                    let regex = arguments.first().expect("replace actual regex argument");
                    assert_eq!(self.heap.kind(regex)?, ValueKind::Object);
                    let source = self.heap.get(regex, &"source".into(), &NoRuntime)?;
                    let ValueSnapshot::String(source) = self.heap.snapshot(&source)? else {
                        panic!("real regex source")
                    };
                    self.events.lock().unwrap().push(format!("replace:{}", source.to_utf8().unwrap()));
                    self.heap.call(&self.replace, receiver, arguments, self)?
                }
                111 => {
                    self.event("entry.path");
                    self.heap.string("path\n".into())?
                }
                112 => {
                    self.event("entry.url");
                    self.heap.string("url\r\n".into())?
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            if capability.0 == 109 {
                self.event("some");
                let predicate = arguments.first().expect("some selected callback");
                assert!(
                    matches!(self.heap.snapshot(&self.heap.get(predicate, &"length".into(), effective_runtime)?)?, ValueSnapshot::Number(value) if value == 1.0)
                );
                text(&self.heap, &self.heap.get(predicate, &"name".into(), effective_runtime)?, "");
                return self.heap.call(&self.some, receiver, arguments, effective_runtime).map(RuntimeReply::Value);
            }
            self.resolve(RuntimeRequest::Call { capability, receiver, arguments })
        }
    }
    fn binding() -> (Arc<ValueHeap>, GraphValueFactory, Arc<Runtime>) {
        let heap = ValueHeap::new().unwrap();
        let string = heap.string("".into()).unwrap();
        let runtime = Arc::new(Runtime {
            some: get(&heap, &heap.array_prototype(), "some"),
            replace: get(&heap, &string, "replace"),
            type_value: parse(&heap, r#"["string","array"]"#),
            thrown: heap.symbol(Some("original normalization throw".into())).unwrap(),
            type_reads: AtomicUsize::new(0),
            mode: AtomicUsize::new(0),
            events: Mutex::new(Vec::new()),
            heap: heap.clone(),
        });
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        (heap, factory, runtime)
    }

    fn enum_inputs() {
        let (heap, factory) = ordinary();
        for schema in [r##"{"$ref":"#"}"##, r##"{"$ref":"#/missing","enum":["x"]}"##] {
            let input = heap.string(" x ".into()).unwrap();
            let result = normalize_graph_enum_string_whitespace(&factory, &parse(&heap, schema), &input).unwrap();
            assert!(!result.changed && result.value.same_identity(&input));
        }
        let schema = parse(&heap, r##"{"$ref":"#/$defs/a~1b~0c","$defs":{"a/b~c":{"enum":["x"]}}}"##);
        let input = heap.string(" x ".into()).unwrap();
        let result = normalize_graph_enum_string_whitespace(&factory, &schema, &input).unwrap();
        assert!(result.changed);
        text(&heap, &result.value, "x");
        for schema in [
            r#"{"enum":[" x ","x"]}"#,
            r#"{"anyOf":[{"type":"string"},{"enum":["x"]}]}"#,
            r#"{"anyOf":[{"type":"number","enum":["x"]}]}"#,
        ] {
            let result = normalize_graph_enum_string_whitespace(&factory, &parse(&heap, schema), &input).unwrap();
            assert!(!result.changed && result.value.same_identity(&input));
        }
        let result = normalize_graph_enum_string_whitespace(
            &factory,
            &parse(&heap, r#"{"allOf":[{"enum":["x"]},{"const":"y"}]}"#),
            &input,
        )
        .unwrap();
        assert!(result.changed);
        text(&heap, &result.value, "x");
        let schema = parse(&heap, r#"{"prefixItems":[{"enum":["a"]}],"items":{"enum":["b"]}}"#);
        let alias = heap.object().unwrap();
        let input = heap
            .array(vec![heap.string(" a ".into()).unwrap(), heap.string(" b ".into()).unwrap(), alias.clone()])
            .unwrap();
        let result = normalize_graph_enum_string_whitespace(&factory, &schema, &input).unwrap();
        assert!(result.changed && !result.value.same_identity(&input));
        text(&heap, &get(&heap, &result.value, "0"), "a");
        text(&heap, &get(&heap, &result.value, "1"), "b");
        assert!(get(&heap, &result.value, "2").same_identity(&alias));
        text(&heap, &get(&heap, &input, "0"), " a ");

        let (heap, factory, _) = binding();
        getter(&heap, &heap.set_intrinsic_prototype(), "add", 101);
        let result = normalize_graph_enum_string_whitespace(
            &factory,
            &parse(&heap, r#"{"enum":["x"]}"#),
            &heap.string(" x ".into()).unwrap(),
        )
        .unwrap();
        assert!(result.changed, "empty default Set never reads add");
    }

    fn identifier_inputs() {
        let (heap, factory) = ordinary();
        let source = heap.object().unwrap();
        for key in IDENTIFIER_KEYS {
            own(&heap, &source, key, heap.string("name \r\n".into()).unwrap());
        }
        let content = parse(&heap, r#"{"path":"preserved\n"}"#);
        for key in CONTENT_KEYS {
            own(&heap, &source, key, content.clone());
        }
        let alias = heap.object().unwrap();
        own(&heap, &source, "alias", alias.clone());
        own(&heap, &source, "nested", parse(&heap, r#"{"path":"nested\n"}"#));
        let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
        assert!(result.changed && !result.value.same_identity(&source));
        for key in IDENTIFIER_KEYS {
            text(&heap, &get(&heap, &result.value, key), "name ");
        }
        for key in CONTENT_KEYS {
            assert!(get(&heap, &result.value, key).same_identity(&content));
        }
        assert!(get(&heap, &result.value, "alias").same_identity(&alias));
        text(&heap, &get(&heap, &get(&heap, &result.value, "nested"), "path"), "nested");
        text(&heap, &get(&heap, &source, "path"), "name \r\n");
        for value in ["ends ", "u2028\u{2028}", "internal\nx"] {
            let source = heap.object().unwrap();
            let original = heap.string(value.into()).unwrap();
            own(&heap, &source, "path", original.clone());
            let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
            assert!(!result.changed && result.value.same_identity(&source));
            assert!(get(&heap, &result.value, "path").same_identity(&original));
        }
        let root = heap.string("root\n".into()).unwrap();
        let result = normalize_graph_identifier_string_whitespace(&factory, &root).unwrap();
        assert!(!result.changed && result.value.same_identity(&root));
        let source = parse(&heap, r#"{"paths":["a\n",1," b\r\n","c "]}"#);
        let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
        let paths = get(&heap, &result.value, "paths");
        assert!(!paths.same_identity(&get(&heap, &source, "paths")));
        text(&heap, &get(&heap, &paths, "0"), "a");
        text(&heap, &get(&heap, &paths, "2"), " b");
        text(&heap, &get(&heap, &paths, "3"), "c ");
    }

    fn identifier_effects() {
        let (heap, factory, runtime) = binding();
        let constants = IdentifierConstants::load(&factory).unwrap();
        own(&heap, &constants.identifier, "has", heap.function(CapabilityId(102)).unwrap());
        let source = parse(&heap, r#"{"path":"unchanged\n"}"#);
        let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
        assert!(!result.changed && result.value.same_identity(&source));
        assert_eq!(runtime.events.lock().unwrap().as_slice(), &["identifier.has"]);

        let (heap, factory, runtime) = binding();
        let source = parse(&heap, r#"{"paths":["a\n","b\r\n"]}"#);
        let paths = get(&heap, &source, "paths");
        own(&heap, &paths, "slice", heap.function(CapabilityId(103)).unwrap());
        let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
        assert!(
            !result.changed && result.value.same_identity(&source),
            "same-reference slice retains mutation with changed=false"
        );
        text(&heap, &get(&heap, &paths, "0"), "a");
        text(&heap, &get(&heap, &paths, "1"), "b");
        assert_eq!(runtime.events.lock().unwrap().as_slice(), &["slice"]);

        let (heap, factory, runtime) = binding();
        let constants = IdentifierConstants::load(&factory).unwrap();
        own(&heap, &constants.terminator, "test", heap.function(CapabilityId(107)).unwrap());
        let string_proto = heap.prototype(&heap.string("".into()).unwrap(), &NoRuntime).unwrap().unwrap();
        own(&heap, &string_proto, "replace", heap.function(CapabilityId(108)).unwrap());
        let source = heap.object().unwrap();
        getter(&heap, &source, "path", 111);
        getter(&heap, &source, "url", 112);
        let result = normalize_graph_identifier_string_whitespace(&factory, &source).unwrap();
        assert!(result.changed);
        text(&heap, &get(&heap, &result.value, "path"), "path");
        let events = runtime.events.lock().unwrap();
        assert_eq!(&events[..2], &["entry.path", "entry.url"]);
        assert!(events.iter().any(|event| event == r"replace:[\r\n]+$"), "actual regex reaches selected replace");
    }

    fn array_union_inputs() {
        let (heap, factory) = ordinary();
        let schema =
            parse(&heap, r#"{"anyOf":[{"type":"string"},{"type":"array","items":{"type":"string"},"minItems":1}]}"#);
        // Inputs from fixed native tool-argument-coercion.test.ts:1636-1711;
        // this exercises the leaf, not Ark or the complete coercion driver.
        for (input, first, length) in [
            (r#"["package.json"]"#, "package.json", 1.0),
            (r#"["package.json","App.tsx"]"#, "package.json", 2.0),
            (r#"["/home/user/project/package.json"]"#, "/home/user/project/package.json", 1.0),
        ] {
            let result =
                normalize_graph_string_encoded_array_unions(&factory, &schema, &heap.string(input.into()).unwrap())
                    .unwrap();
            assert!(result.changed && heap.kind(&result.value).unwrap() == ValueKind::Array);
            text(&heap, &get(&heap, &result.value, "0"), first);
            assert!(
                matches!(heap.snapshot(&get(&heap, &result.value, "length")).unwrap(), ValueSnapshot::Number(value) if value == length)
            );
        }
        for input in ["package.json", "[abc]", "[1]", "[]"] {
            let original = heap.string(input.into()).unwrap();
            let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &original).unwrap();
            assert!(!result.changed && result.value.same_identity(&original));
        }
        let input = parse(&heap, r#"["package.json","App.tsx"]"#);
        let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &input).unwrap();
        assert!(!result.changed && result.value.same_identity(&input));
        let input = heap.string(r#"["not-a-list"]"#.into()).unwrap();
        let result =
            normalize_graph_string_encoded_array_unions(&factory, &parse(&heap, r#"{"type":"string"}"#), &input)
                .unwrap();
        assert!(!result.changed && result.value.same_identity(&input));
        let schema = parse(
            &heap,
            r#"{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"object","properties":{"op":{"type":"string"}},"required":["op"]}}]}"#,
        );
        let input = heap.string(r#"[{"\"op\"":"done"}]"#.into()).unwrap();
        let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &input).unwrap();
        assert!(result.changed);
        text(&heap, &get(&heap, &get(&heap, &result.value, "0"), "op"), "done");
        let schema = parse(
            &heap,
            r#"{"type":"object","properties":{"paths":{"type":["string","array"],"items":{"type":"string"}}}}"#,
        );
        let input = parse(&heap, r#"{"paths":"[\"package.json\"]","alias":{"keep":true}}"#);
        let alias = get(&heap, &input, "alias");
        let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &input).unwrap();
        assert!(result.changed && get(&heap, &result.value, "alias").same_identity(&alias));
        text(&heap, &get(&heap, &get(&heap, &result.value, "paths"), "0"), "package.json");
        // Explicit root array acceptance routes through the whole root checker;
        // its minItems rejection must not fall through to a permissive union.
        let schema = parse(&heap, r#"{"type":["string","array"],"minItems":2,"anyOf":[{"type":"array"}]}"#);
        let input = heap.string("[1]".into()).unwrap();
        let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &input).unwrap();
        assert!(!result.changed && result.value.same_identity(&input));
    }

    fn array_union_effects() {
        for missing in [false, true] {
            let (heap, factory, runtime) = binding();
            runtime.mode.store(usize::from(missing), Ordering::SeqCst);
            let schema = parse(&heap, r#"{"items":{"type":"string"}}"#);
            getter(&heap, &schema, "type", 104);
            let input = heap.string(r#"["a"]"#.into()).unwrap();
            let result = normalize_graph_string_encoded_array_unions(&factory, &schema, &input);
            assert_eq!(runtime.type_reads.load(Ordering::SeqCst), 4);
            if missing {
                assert!(matches!(result, Err(ValueError::MissingCapability(CapabilityId(904)))));
            } else {
                let result = result.unwrap();
                assert!(!result.changed && result.value.same_identity(&input));
            }
        }
        for (method, cap) in [("trim", 105), ("startsWith", 106)] {
            let (heap, factory, runtime) = binding();
            let string_proto = heap.prototype(&heap.string("".into()).unwrap(), &NoRuntime).unwrap().unwrap();
            own(&heap, &string_proto, method, heap.function(CapabilityId(cap)).unwrap());
            let schema = parse(&heap, r#"{"type":["string","array"]}"#);
            let result =
                normalize_graph_string_encoded_array_unions(&factory, &schema, &heap.string("[1]".into()).unwrap());
            assert!(
                matches!(result, Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown)),
                "outside-catch method throw propagates"
            );
        }
        let (heap, factory, runtime) = binding();
        let schema = parse(&heap, r#"{"anyOf":[{"type":["number","string"]},{"enum":["x"]}]}"#);
        let branches = get(&heap, &schema, "anyOf");
        own(&heap, &branches, "some", heap.function(CapabilityId(109)).unwrap());
        let input = heap.string(" x ".into()).unwrap();
        let result = normalize_graph_enum_string_whitespace(&factory, &schema, &input).unwrap();
        assert!(!result.changed && result.value.same_identity(&input));
        assert_eq!(runtime.events.lock().unwrap().as_slice(), &["some"]);
        let (heap, factory, runtime) = binding();
        let string_proto = heap.prototype(&heap.string("".into()).unwrap(), &NoRuntime).unwrap().unwrap();
        own(&heap, &string_proto, "replace", heap.function(CapabilityId(108)).unwrap());
        let schema = parse(&heap, r##"{"$ref":"#/$defs/a~1b~0c","$defs":{"a/b~c":{"enum":["x"]}}}"##);
        let result =
            normalize_graph_enum_string_whitespace(&factory, &schema, &heap.string(" x ".into()).unwrap()).unwrap();
        assert!(result.changed);
        assert_eq!(
            runtime.events.lock().unwrap().as_slice(),
            &["replace:~1", "replace:~0", "replace:~1", "replace:~0"]
        );
    }

    pub fn run() {
        enum_inputs();
        identifier_inputs();
        identifier_effects();
        array_union_inputs();
        array_union_effects();
    }
}
