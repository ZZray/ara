//! Actual optional-null normalization and the plain JSON validation context.
//! Fixed OMP 596f2da7101178214aa27a753529d15e6b7ad91d,
//! `packages/ai/src/utils/validation.ts`:53-96, 668-831, 1724-1741.
//! The caller supplies an already upgraded JSON schema in this same lineage.
//! Child modules supply the fixed JSON-context tool-arguments driver and leaf
//! normalizers. Ark runtime and production Graph Session/dispatch stay separate.
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

mod driver;
pub use driver::{GraphToolValidationError, validate_graph_tool_arguments, validate_graph_tool_call};
mod coercions;
mod schema_normalizers;
mod shapes;
mod spill_preview;

pub use coercions::{
    coerce_graph_args_from_issues, delete_graph_value_at_pointer, format_graph_issue_path,
    format_graph_issue_path_with_runtime, get_graph_value_at_pointer, path_to_graph_pointer,
    path_to_graph_pointer_with_runtime, set_graph_value_at_pointer, try_parse_graph_json_for_types,
};
pub use schema_normalizers::{
    normalize_graph_enum_string_whitespace, normalize_graph_identifier_string_whitespace,
    normalize_graph_string_encoded_array_unions,
};
pub use spill_preview::{heal_graph_inband_arg_spill, truncate_graph_args_for_error};

pub fn normalize_graph_flattened_array_properties(
    factory: &crate::graph_values::GraphValueFactory,
    value: &ara_rpc::value_heap::ValueHandle,
) -> Result<GraphNormalization, ara_rpc::value_heap::ValueError> {
    shapes::normalize_graph_flattened_array_properties(factory, value)
}
#[cfg(test)]
pub(super) fn observe_remaining_normalizers_family() {
    shapes::observe_graph_shapes_family();
    schema_normalizers::observe_graph_schema_normalizers_family();
    coercions::observe_graph_coercions_family();
    spill_preview::observe_graph_spill_preview_family();
    driver::observe_graph_driver_family();
}

use crate::graph_values::GraphValueFactory;
use crate::schema_validator::{JsonSchemaValidationResult, graph_numeric_less_than, validate_graph_json_schema_value};
use ara_rpc::WireString;
use ara_rpc::value_heap::{IteratorRecord, PropertyKey, ValueError, ValueHandle, ValueKind, ValueSnapshot};
pub(super) use driver::{flatten_graph_json_issues, graph_validation_messages};

#[derive(Clone, Debug)]
pub struct GraphNormalization {
    pub value: ValueHandle,
    pub changed: bool,
}

#[derive(Clone, Debug)]
pub enum GraphContextValidation {
    /// Plain JSON validateContext returns its original argument reference.
    Accept(ValueHandle),
    /// Keep actual expectedTypes and path handles until Native formatting.
    Reject(JsonSchemaValidationResult<ValueHandle, ValueHandle>),
    /// Authored Ark errors keep actual JSON coercion issues and Ark messages.
    RejectActual { flat_issues: ValueHandle, messages: ValueHandle },
}

pub fn validate_graph_json_context(
    factory: &GraphValueFactory,
    upgraded_json_schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<GraphContextValidation, ValueError> {
    let result = validate_graph_json_schema_value(factory, upgraded_json_schema, value)?;
    Ok(if result.success {
        GraphContextValidation::Accept(value.clone())
    } else {
        GraphContextValidation::Reject(result)
    })
}

pub fn normalize_graph_optional_nulls_for_schema(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
    is_root: bool,
) -> Result<GraphNormalization, ValueError> {
    factory.heap().adopt(schema)?;
    factory.heap().adopt(value)?;
    Normalizer { factory }.normalize(schema, value, is_root)
}

/// Graph leaf pass also consumed by the fixed JSON-context validation driver.
pub fn normalize_graph_double_encoded_keys(
    factory: &GraphValueFactory,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    shapes::normalize_graph_double_encoded_keys(factory, value)
}

/// Graph leaf pass; preserves the selected source schema and original value.
pub fn normalize_graph_single_string_field(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
    value: &ValueHandle,
) -> Result<GraphNormalization, ValueError> {
    shapes::normalize_graph_single_string_field(factory, schema, value)
}

struct Normalizer<'a> {
    factory: &'a GraphValueFactory,
}

impl Normalizer<'_> {
    fn unchanged(&self, value: &ValueHandle) -> GraphNormalization {
        GraphNormalization { value: value.clone(), changed: false }
    }
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.factory.heap().get(value, &key.into(), self.factory.runtime())
    }
    fn is_text(&self, value: &ValueHandle, text: &str) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().snapshot(value)?, ValueSnapshot::String(value) if value.equals_ascii(text)))
    }
    fn typeof_object(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().kind(value)?, ValueKind::Object | ValueKind::Array | ValueKind::Null))
    }
    fn null(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Null)
    }
    fn array(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Array)
    }
    fn method(&self, receiver: &ValueHandle, name: &str, args: &[ValueHandle]) -> Result<ValueHandle, ValueError> {
        let method = self.get(receiver, name)?;
        self.factory.heap().call(&method, receiver, args, self.factory.runtime())
    }
    fn set_has(&self, set: &ValueHandle, key: &ValueHandle) -> Result<bool, ValueError> {
        let result = self.method(set, "has", std::slice::from_ref(key))?;
        self.factory.heap().truthy(&result)
    }
    fn property_key(&self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let heap = self.factory.heap();
        let primitive = heap.to_primitive(value, true, self.factory.runtime())?;
        if heap.kind(&primitive)? == ValueKind::Symbol {
            Ok(PropertyKey::Symbol(heap.symbol_key(&primitive)?))
        } else {
            Ok(PropertyKey::String(heap.to_string(&primitive, self.factory.runtime())?))
        }
    }
    fn branch_matches(&self, schema: &ValueHandle, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(validate_graph_json_schema_value(self.factory, schema, value)?.success)
    }
    fn close_error(&self, iterator: &mut IteratorRecord, original: ValueError) -> ValueError {
        match self.factory.heap().iterator_close(iterator, self.factory.runtime()) {
            Ok(()) => original,
            Err(close) if original.is_native_exception() && close.is_native_exception() => original,
            Err(close) => ValueError::IteratorClose { original: Box::new(original), close: Box::new(close) },
        }
    }
    fn keys_array(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let keys = heap.own_enumerable_string_keys(value, self.factory.runtime())?;
        let keys = keys.into_iter().map(|key| heap.string(key)).collect::<Result<Vec<_>, _>>()?;
        heap.array(keys)
    }
    fn array_spread(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let result = heap.array_with_length(0)?;
        let mut iterator = heap.get_iterator(value, self.factory.runtime())?;
        let mut index = 0_u64;
        while let Some(value) = heap.iterator_next(&mut iterator, self.factory.runtime())? {
            // Array literal spread defines own data; it does not call push or
            // invoke an inherited index setter.
            if let Err(error) =
                heap.define_own_data(&result, WireString::from(index.to_string()).into(), value, self.factory.runtime())
            {
                return Err(self.close_error(&mut iterator, error));
            }
            index += 1;
        }
        Ok(result)
    }
    fn pair(&self, entry: &ValueHandle) -> Result<(ValueHandle, ValueHandle), ValueError> {
        let heap = self.factory.heap();
        let mut iterator = heap.get_iterator(entry, self.factory.runtime())?;
        let result = (|| {
            let key = match heap.iterator_next(&mut iterator, self.factory.runtime())? {
                Some(value) => value,
                None => heap.undefined()?,
            };
            let schema = match heap.iterator_next(&mut iterator, self.factory.runtime())? {
                Some(value) => value,
                None => heap.undefined()?,
            };
            Ok((key, schema))
        })();
        match result {
            Ok(pair) => {
                heap.iterator_close(&mut iterator, self.factory.runtime())?;
                Ok(pair)
            }
            Err(error) => Err(self.close_error(&mut iterator, error)),
        }
    }
    fn any_of_like(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        is_root: bool,
        keyword: &str,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let branches = self.get(schema, keyword)?;
        if !self.array(&branches)? {
            return Ok(self.unchanged(value));
        }
        let mut candidate = None;
        let mut iterator = heap.get_iterator(&branches, self.factory.runtime())?;
        while let Some(branch) = heap.iterator_next(&mut iterator, self.factory.runtime())? {
            let result = (|| {
                let normalized = self.normalize(&branch, value, is_root)?;
                let accepted = normalized.changed && self.branch_matches(&branch, &normalized.value)?;
                Ok::<_, ValueError>((normalized, accepted))
            })();
            let (normalized, accepted) = match result {
                Ok(result) => result,
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            };
            if accepted {
                // A return from for-of performs IteratorClose, including its
                // getter/call and any close failure.
                heap.iterator_close(&mut iterator, self.factory.runtime())?;
                return Ok(normalized);
            }
            if normalized.changed && candidate.is_none() {
                candidate = Some(normalized);
            }
        }
        Ok(candidate.unwrap_or_else(|| self.unchanged(value)))
    }
    fn normalize(
        &self,
        schema: &ValueHandle,
        value: &ValueHandle,
        is_root: bool,
    ) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        if matches!(heap.kind(value)?, ValueKind::Null | ValueKind::Undefined) {
            return Ok(self.unchanged(value));
        }
        if self.null(schema)? || !self.typeof_object(schema)? {
            return Ok(self.unchanged(value));
        }
        for keyword in ["anyOf", "oneOf"] {
            let normalized = self.any_of_like(schema, value, is_root, keyword)?;
            if normalized.changed {
                return Ok(normalized);
            }
        }
        if self.array(&self.get(schema, "allOf")?)? {
            let mut changed = false;
            let mut next = value.clone();
            let branches = self.get(schema, "allOf")?;
            let mut iterator = heap.get_iterator(&branches, runtime)?;
            while let Some(branch) = heap.iterator_next(&mut iterator, runtime)? {
                let normalized = match self.normalize(&branch, &next, is_root) {
                    Ok(result) => result,
                    Err(error) => return Err(self.close_error(&mut iterator, error)),
                };
                if normalized.changed {
                    next = normalized.value;
                    changed = true;
                }
            }
            if changed {
                return Ok(GraphNormalization { value: next, changed });
            }
        }
        if self.array(value)? {
            let items = self.get(schema, "items")?;
            if self.null(&items)? || !self.typeof_object(&items)? || self.array(&items)? {
                return Ok(self.unchanged(value));
            }
            let mut changed = false;
            let mut next = value.clone();
            let mut index = 0_u64;
            loop {
                // Native compares i to the current value.length each time.
                let length = self.get(value, "length")?;
                if !graph_numeric_less_than(self.factory, index as f64, &length)? {
                    break;
                }
                let current = self.get(value, &index.to_string())?;
                let normalized = self.normalize(&items, &current, false)?;
                if normalized.changed {
                    if !changed {
                        next = self.array_spread(value)?;
                        changed = true;
                    }
                    heap.assign(&next, WireString::from(index.to_string()).into(), normalized.value, runtime)?;
                }
                if index >= 9_007_199_254_740_992 {
                    return Err(ValueError::UnsupportedOperation("non-progressing Native numeric array index"));
                }
                index += 1;
            }
            return Ok(GraphNormalization { value: next, changed });
        }
        if (self.is_text(&self.get(schema, "type")?, "number")?
            || self.is_text(&self.get(schema, "type")?, "integer")?)
            && heap.kind(value)? == ValueKind::String
        {
            let expected = heap.array(vec![self.get(schema, "type")?])?;
            return self.number_string(value, &expected);
        }
        if !self.is_text(&self.get(schema, "type")?, "object")? {
            return Ok(self.unchanged(value));
        }
        if !self.typeof_object(value)? || self.null(value)? {
            return Ok(self.unchanged(value));
        }
        if self.array(value)? {
            return Ok(self.unchanged(value));
        }
        if self.null(&self.get(schema, "properties")?)? || !self.typeof_object(&self.get(schema, "properties")?)? {
            return Ok(self.unchanged(value));
        }
        let properties = self.get(schema, "properties")?;
        let required = if self.array(&self.get(schema, "required")?)? {
            self.get(schema, "required")?
        } else {
            heap.array(Vec::new())?
        };
        let required = heap.new_set(&required, runtime)?;
        let mut changed = false;
        let mut next = value.clone();
        let entries = heap.object_entries(&properties, runtime)?;
        let mut iterator = heap.get_iterator(&entries, runtime)?;
        while let Some(entry) = heap.iterator_next(&mut iterator, runtime)? {
            let result = (|| {
                let (key, property_schema) = self.pair(&entry)?;
                if !heap.has(&next, &self.property_key(&key)?, runtime)? {
                    return Ok(());
                }
                let current = heap.get(&next, &self.property_key(&key)?, runtime)?;
                let nullish = self.null(&current)? || self.is_text(&current, "null")?;
                let invalid_empty = self.is_text(&current, "")?
                    && !self.set_has(&required, &key)?
                    && !self.branch_matches(&property_schema, &current)?;
                if (nullish || invalid_empty) && !self.set_has(&required, &key)? {
                    if !changed {
                        next = heap.spread(&next, runtime)?;
                        changed = true;
                    }
                    if !heap.delete_own(&next, &self.property_key(&key)?, runtime)? {
                        return Err(ValueError::TypeError(
                            "Cannot delete property in strict optional-null normalization".into(),
                        ));
                    }
                    return Ok(());
                }
                if nullish
                    && heap.truthy(&property_schema)?
                    && self.typeof_object(&property_schema)?
                    && heap.has(&property_schema, &"default".into(), runtime)?
                {
                    if !changed {
                        next = heap.spread(&next, runtime)?;
                        changed = true;
                    }
                    // Assignment evaluates the computed LHS before its RHS.
                    let property = self.property_key(&key)?;
                    let default = self.get(&property_schema, "default")?;
                    let default = heap.structured_clone_json(&default, runtime)?;
                    heap.assign(&next, property, default, runtime)?;
                    return Ok(());
                }
                let normalized = self.normalize(&property_schema, &current, false)?;
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
        if !is_root
            && matches!(heap.snapshot(&self.get(schema, "additionalProperties")?)?, ValueSnapshot::Boolean(false))
        {
            let known_keys = self.keys_array(&properties)?;
            let known = heap.new_set(&known_keys, runtime)?;
            let keys = self.keys_array(&next)?;
            let mut iterator = heap.get_iterator(&keys, runtime)?;
            while let Some(key) = heap.iterator_next(&mut iterator, runtime)? {
                let result = (|| {
                    if self.set_has(&known, &key)? {
                        return Ok(());
                    }
                    let current = heap.get(&next, &self.property_key(&key)?, runtime)?;
                    if !self.null(&current)? && !self.is_text(&current, "null")? {
                        return Ok(());
                    }
                    if !changed {
                        next = heap.spread(&next, runtime)?;
                        changed = true;
                    }
                    if !heap.delete_own(&next, &self.property_key(&key)?, runtime)? {
                        return Err(ValueError::TypeError(
                            "Cannot delete property in strict optional-null normalization".into(),
                        ));
                    }
                    Ok::<_, ValueError>(())
                })();
                if let Err(error) = result {
                    return Err(self.close_error(&mut iterator, error));
                }
            }
        }
        Ok(GraphNormalization { value: next, changed })
    }
    fn number_string(&self, value: &ValueHandle, expected: &ValueHandle) -> Result<GraphNormalization, ValueError> {
        let heap = self.factory.heap();
        let runtime = self.factory.runtime();
        let number = heap.string("number".into())?;
        if !heap.truthy(&self.method(expected, "includes", &[number])?)? {
            let integer = heap.string("integer".into())?;
            if !heap.truthy(&self.method(expected, "includes", &[integer])?)? {
                return Ok(self.unchanged(value));
            }
        }
        let trimmed = self.method(value, "trim", &[])?;
        if !heap.truthy(&trimmed)? {
            return Ok(self.unchanged(value));
        }
        // Fixed module-local /^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$/,
        // tested over UTF-16. RegExp.test ToString precedes Number(trimmed).
        if heap.kind(&trimmed)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
        }
        let text = heap.to_string(&trimmed, runtime)?;
        if !numeric_pattern(&text) {
            return Ok(self.unchanged(value));
        }
        // Explicit Number() accepts a BigInt primitive; implicit ToNumber does
        // not. Convert the original returned object once with number hint.
        let primitive = heap.to_primitive(&trimmed, false, runtime)?;
        let parsed = match heap.snapshot(&primitive)? {
            ValueSnapshot::BigInt(value) => value
                .as_str()
                .parse::<f64>()
                .map_err(|_| ValueError::UnsupportedOperation("canonical BigInt decimal to Number conversion"))?,
            _ => heap.to_number(&primitive, runtime)?,
        };
        if !parsed.is_finite() {
            return Ok(self.unchanged(value));
        }
        let parsed_handle = heap.number(parsed)?;
        let some = self.get(expected, "some")?;
        let mut callback =
            |_receiver: &ValueHandle, args: &[ValueHandle], _runtime: &dyn ara_rpc::value_heap::ValueRuntime| {
                let kind = args.first().map(|value| heap.snapshot(value)).transpose()?;
                let matches = match kind {
                    Some(ValueSnapshot::String(kind)) if kind.equals_ascii("number") => parsed.is_finite(),
                    Some(ValueSnapshot::String(kind)) if kind.equals_ascii("integer") => {
                        parsed.is_finite() && parsed.fract() == 0.0
                    }
                    _ => false,
                };
                heap.boolean(matches)
            };
        let matched = heap.call_with_local_callback(&some, expected, &mut callback, &[], runtime)?;
        if !heap.truthy(&matched)? {
            return Ok(self.unchanged(value));
        }
        Ok(GraphNormalization { value: parsed_handle, changed: true })
    }
}

fn numeric_pattern(value: &WireString) -> bool {
    // Same ECMAScript-regex primitive and UCS2 input domain as the checker;
    // the fixed built-in pattern is not a supplied Graph RegExp capability.
    static PATTERN: std::sync::LazyLock<regress::Regex> = std::sync::LazyLock::new(|| {
        regress::Regex::new(r"^[+-]?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?$").expect("fixed Native numeric pattern is valid")
    });
    PATTERN.find_from_ucs2(value.units(), 0).next().is_some()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest, ValueHeap,
        ValueRuntime,
    };
    use serde_json::json;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    fn own(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(object, key.into(), value, &NoRuntime).unwrap();
    }
    fn getter(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str, capability: u64) {
        heap.define_own(
            object,
            key.into(),
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
    fn get(heap: &Arc<ValueHeap>, object: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(object, &key.into(), &NoRuntime).unwrap()
    }
    enum Reply {
        Value(ValueHandle),
        Error(ValueError),
    }
    struct Runtime {
        replies: Mutex<HashMap<u64, VecDeque<Reply>>>,
        trace: Mutex<Vec<u64>>,
    }
    impl Runtime {
        fn new(replies: Vec<(u64, Vec<Reply>)>) -> Arc<Self> {
            Arc::new(Self {
                replies: Mutex::new(replies.into_iter().map(|(id, values)| (id, values.into())).collect()),
                trace: Mutex::new(Vec::new()),
            })
        }
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            let id = request.capability().0;
            self.trace.lock().unwrap().push(id);
            match self.replies.lock().unwrap().get_mut(&id).and_then(VecDeque::pop_front) {
                Some(Reply::Value(value)) => Ok(RuntimeReply::Value(value)),
                Some(Reply::Error(error)) => Err(error),
                None => Err(ValueError::MissingCapability(CapabilityId(id))),
            }
        }
    }
    fn step(heap: &Arc<ValueHeap>, value: ValueHandle) -> ValueHandle {
        let result = heap.object().unwrap();
        own(heap, &result, "done", heap.boolean(false).unwrap());
        own(heap, &result, "value", value);
        result
    }

    // Called by the existing accepts_valid_and_strips_optional_nulls family.
    pub(crate) fn aliases_defaults_entries_and_context() {
        super::shapes::observe_graph_shapes_family();
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let schema = heap.import_serde(&json!({"type":"object","properties":{"optional":{"type":"string"},"nested":{"type":"object","properties":{"a":{"type":"string"}},"additionalProperties":false},"required":{"default":{"x":[1]}}},"required":["required"]})).unwrap();
        let value = heap.import_serde(&json!({"optional":null,"nested":{"a":"keep","unknown":"null","real":7},"required":"null","rootExtra":null})).unwrap();
        let nested = get(&heap, &value, "nested");
        let result = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(result.changed && !result.value.same_identity(&value));
        assert!(heap.has_own(&value, &"optional".into(), &NoRuntime).unwrap());
        assert!(!heap.has_own(&result.value, &"optional".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&result.value, &"rootExtra".into(), &NoRuntime).unwrap());
        let copied_nested = get(&heap, &result.value, "nested");
        assert!(!copied_nested.same_identity(&nested));
        assert!(heap.has_own(&nested, &"unknown".into(), &NoRuntime).unwrap());
        assert!(!heap.has_own(&copied_nested, &"unknown".into(), &NoRuntime).unwrap());
        assert!(get(&heap, &copied_nested, "a").same_identity(&get(&heap, &nested, "a")));
        let properties = get(&heap, &schema, "properties");
        let default = get(&heap, &get(&heap, &properties, "required"), "default");
        let copied_default = get(&heap, &result.value, "required");
        assert!(!copied_default.same_identity(&default));
        assert!(!get(&heap, &copied_default, "x").same_identity(&get(&heap, &default, "x")));
        own(&heap, &copied_default, "new", heap.boolean(true).unwrap());
        assert!(!heap.has_own(&default, &"new".into(), &NoRuntime).unwrap());

        let sibling = heap.object().unwrap();
        let simple = heap.import_serde(&json!({"type":"object","properties":{"a":{"type":"string"}}})).unwrap();
        let value = heap.object().unwrap();
        own(&heap, &value, "a", heap.string("keep".into()).unwrap());
        own(&heap, &value, "sibling", sibling.clone());
        let unchanged = normalize_graph_optional_nulls_for_schema(&factory, &simple, &value, true).unwrap();
        assert!(!unchanged.changed && unchanged.value.same_identity(&value));
        assert!(
            matches!(validate_graph_json_context(&factory, &simple, &value).unwrap(), GraphContextValidation::Accept(input) if input.same_identity(&value))
        );
        own(&heap, &value, "a", heap.null().unwrap());
        let copied = normalize_graph_optional_nulls_for_schema(&factory, &simple, &value, true).unwrap();
        assert!(get(&heap, &copied.value, "sibling").same_identity(&sibling));
        let reject_schema = heap.import_serde(&json!({"type":"object","required":["missing"]})).unwrap();
        let GraphContextValidation::Reject(rejected) =
            validate_graph_json_context(&factory, &reject_schema, &value).unwrap()
        else {
            panic!("must reject missing property")
        };
        assert!(!rejected.success && rejected.issues[0].keyword == Some("required"));
        assert!(
            matches!(heap.snapshot(&rejected.issues[0].path[0]).unwrap(), ValueSnapshot::String(key) if key.equals_ascii("missing"))
        );

        let empty = heap.import_serde(&json!({"a":"","b":""})).unwrap();
        let empty_schema = heap
            .import_serde(
                &json!({"type":"object","properties":{"a":{"type":"string","minLength":1},"b":{"type":"string"}}}),
            )
            .unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &empty_schema, &empty, true).unwrap();
        assert!(!heap.has_own(&normalized.value, &"a".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&normalized.value, &"b".into(), &NoRuntime).unwrap());

        // Object.entries finishes its getters before entering the first body.
        // The first-copy spread rereads a's getter at its reached position.
        let schema = heap.object().unwrap();
        let props = heap.object().unwrap();
        own(&heap, &schema, "type", heap.string("object".into()).unwrap());
        getter(&heap, &schema, "properties", 1);
        getter(&heap, &props, "a", 2);
        getter(&heap, &props, "b", 3);
        let string_schema = heap.import_serde(&json!({"type":"string"})).unwrap();
        let value = heap.object().unwrap();
        getter(&heap, &value, "a", 4);
        own(&heap, &value, "b", heap.string("keep".into()).unwrap());
        let runtime = Runtime::new(vec![
            (1, vec![Reply::Value(props.clone()), Reply::Value(props.clone()), Reply::Value(props.clone())]),
            (2, vec![Reply::Value(string_schema.clone())]),
            (3, vec![Reply::Value(string_schema)]),
            (4, vec![Reply::Value(heap.null().unwrap()), Reply::Value(heap.null().unwrap())]),
        ]);
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(normalized.changed);
        assert_eq!(*runtime.trace.lock().unwrap(), vec![1, 1, 1, 2, 3, 4, 4]);

        // Inherited current properties use in/Get and are removed from a copy;
        // inherited schema properties are excluded by Object.entries.
        let inherited = heap.object().unwrap();
        own(&heap, &inherited, "a", heap.null().unwrap());
        let value = heap.object_with_prototype(Some(&inherited)).unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &simple, &value, true).unwrap();
        assert!(normalized.changed && !heap.has(&normalized.value, &"a".into(), &NoRuntime).unwrap());
        assert!(heap.has(&value, &"a".into(), &NoRuntime).unwrap());
        let array_schema = heap.array(Vec::new()).unwrap();
        own(&heap, &array_schema, "type", heap.string("object".into()).unwrap());
        own(&heap, &array_schema, "properties", get(&heap, &simple, "properties"));
        assert!(
            normalize_graph_optional_nulls_for_schema(&factory, &array_schema, &value, true).unwrap().changed,
            "Native typeof-object schema gate permits arrays"
        );
        for inert in [heap.null().unwrap(), heap.undefined().unwrap(), heap.number(1.0).unwrap()] {
            let normalized = normalize_graph_optional_nulls_for_schema(&factory, &inert, &value, true).unwrap();
            assert!(!normalized.changed && normalized.value.same_identity(&value));
        }
        let props = heap.object_with_prototype(Some(&properties)).unwrap();
        let schema = heap.object().unwrap();
        own(&heap, &schema, "type", heap.string("object".into()).unwrap());
        own(&heap, &schema, "properties", props);
        assert!(!normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap().changed);
    }

    // Called by the existing numeric-string family, not a new test entry.
    fn opaque_array_relational_length_domain() {
        struct LengthRuntime {
            length: ValueHandle,
            integer: ValueHandle,
            item: ValueHandle,
            trace: Mutex<Vec<&'static str>>,
        }
        impl ValueRuntime for LengthRuntime {
            fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
                let capability = request.capability();
                match request {
                    RuntimeRequest::Opaque { operation: ara_rpc::value_heap::OpaqueOperation::Get(key), .. } => {
                        match key {
                            PropertyKey::String(text) if text.equals_ascii("length") => {
                                self.trace.lock().unwrap().push("length");
                                Ok(RuntimeReply::Value(self.length.clone()))
                            }
                            PropertyKey::String(text) if text.equals_ascii("0") => {
                                self.trace.lock().unwrap().push("0");
                                Ok(RuntimeReply::Value(self.item.clone()))
                            }
                            _ => Err(ValueError::MissingCapability(capability)),
                        }
                    }
                    RuntimeRequest::Call { capability: CapabilityId(82), receiver, .. } => {
                        assert!(receiver.same_identity(&self.length));
                        self.trace.lock().unwrap().push("number");
                        Ok(RuntimeReply::Value(self.integer.clone()))
                    }
                    _ => Err(ValueError::MissingCapability(capability)),
                }
            }
        }
        for object_length in [false, true] {
            let heap = ValueHeap::new().unwrap();
            let integer = heap.bigint_decimal("1").unwrap();
            let length = if object_length {
                let object = heap.object().unwrap();
                own(&heap, &object, "valueOf", heap.function(CapabilityId(82)).unwrap());
                object
            } else {
                integer.clone()
            };
            let value = heap.opaque(ara_rpc::value_heap::OpaqueKind::Array, CapabilityId(81), 1).unwrap();
            let runtime =
                Arc::new(LengthRuntime { length, integer, item: heap.null().unwrap(), trace: Mutex::new(Vec::new()) });
            let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
            let schema = heap.import_serde(&json!({"items":{}})).unwrap();
            let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
            assert!(!normalized.changed && normalized.value.same_identity(&value));
            let expected = if object_length {
                vec!["length", "number", "0", "length", "number"]
            } else {
                vec!["length", "0", "length"]
            };
            assert_eq!(*runtime.trace.lock().unwrap(), expected);
        }
    }

    pub(crate) fn numeric_and_array_domain() {
        opaque_array_relational_length_domain();
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        for (kind, input, expected) in [
            ("number", "  +12.5e-1\u{feff}", Some(1.25)),
            ("integer", " 12e2 ", Some(1200.0)),
            ("integer", "1.2", None),
            ("number", ".5", None),
            ("number", "1.", None),
            ("number", "0x10", None),
            ("number", "1e999", None),
            ("number", "", None),
        ] {
            let schema = heap.import_serde(&json!({"type":kind})).unwrap();
            let value = heap.string(input.into()).unwrap();
            let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
            match expected {
                Some(number) => {
                    assert!(normalized.changed);
                    assert!(
                        matches!(heap.snapshot(&normalized.value).unwrap(), ValueSnapshot::Number(value) if value == number)
                    );
                }
                None => assert!(!normalized.changed && normalized.value.same_identity(&value)),
            }
        }
        let schema = heap.import_serde(&json!({"type":"array","items":{"type":"number"}})).unwrap();
        let value = heap.array(vec![heap.string("2".into()).unwrap(), heap.number(3.0).unwrap()]).unwrap();
        let sibling = get(&heap, &value, "1");
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(normalized.changed && !normalized.value.same_identity(&value));
        assert!(matches!(heap.snapshot(&get(&heap, &value, "0")).unwrap(), ValueSnapshot::String(_)));
        assert!(get(&heap, &normalized.value, "1").same_identity(&sibling));
        let tuple = heap.import_serde(&json!({"items":[{"type":"number"}]})).unwrap();
        assert!(!normalize_graph_optional_nulls_for_schema(&factory, &tuple, &value, true).unwrap().changed);
        let number = heap.import_serde(&json!({"type":"number"})).unwrap();
        let surrogate = heap.string(WireString::from_units(vec![0xd800])).unwrap();
        assert!(!normalize_graph_optional_nulls_for_schema(&factory, &number, &surrogate, true).unwrap().changed);

        // A changed schema getter is reread for the selected expectedTypes.
        let changed_type = heap.object().unwrap();
        getter(&heap, &changed_type, "type", 50);
        let runtime = Runtime::new(vec![(
            50,
            vec![
                Reply::Value(heap.string("number".into()).unwrap()),
                Reply::Value(heap.string("boolean".into()).unwrap()),
            ],
        )]);
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let value = heap.string("2".into()).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &changed_type, &value, true).unwrap();
        assert!(!normalized.changed && normalized.value.same_identity(&value));
        assert_eq!(*runtime.trace.lock().unwrap(), vec![50, 50]);

        // A Host trim may return an object: RegExp.test's string hint happens
        // before explicit Number's number hint, preserving both callbacks.
        let heap = ValueHeap::new().unwrap();
        let value = heap.string("ignored by trim override".into()).unwrap();
        let string_prototype = heap.prototype(&value, &NoRuntime).unwrap().unwrap();
        own(&heap, &string_prototype, "trim", heap.function(CapabilityId(60)).unwrap());
        let trimmed = heap.object().unwrap();
        own(&heap, &trimmed, "toString", heap.function(CapabilityId(61)).unwrap());
        own(&heap, &trimmed, "valueOf", heap.function(CapabilityId(62)).unwrap());
        let schema = heap.import_serde(&json!({"type":"number"})).unwrap();
        let runtime = Runtime::new(vec![
            (60, vec![Reply::Value(trimmed)]),
            (61, vec![Reply::Value(heap.string("2".into()).unwrap())]),
            (62, vec![Reply::Value(heap.number(3.0).unwrap())]),
        ]);
        let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(
            normalized.changed
                && matches!(heap.snapshot(&normalized.value).unwrap(), ValueSnapshot::Number(value) if value == 3.0)
        );
        assert_eq!(*runtime.trace.lock().unwrap(), vec![60, 61, 62]);
        for (decimal, expected) in
            [("9007199254740993".to_string(), Some(9_007_199_254_740_992.0)), (format!("1{}", "0".repeat(400)), None)]
        {
            let heap = ValueHeap::new().unwrap();
            let value = heap.string("ignored".into()).unwrap();
            let string_prototype = heap.prototype(&value, &NoRuntime).unwrap().unwrap();
            own(&heap, &string_prototype, "trim", heap.function(CapabilityId(63)).unwrap());
            let runtime = Runtime::new(vec![(63, vec![Reply::Value(heap.bigint_decimal(&decimal).unwrap())])]);
            let factory = GraphValueFactory::new(heap.clone(), runtime).unwrap();
            let schema = heap.import_serde(&json!({"type":"integer"})).unwrap();
            let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
            match expected {
                Some(expected) => assert!(
                    normalized.changed
                        && matches!(heap.snapshot(&normalized.value).unwrap(), ValueSnapshot::Number(actual) if actual == expected)
                ),
                None => assert!(!normalized.changed && normalized.value.same_identity(&value)),
            }
        }
    }

    // Called by the existing allOf coarse family.
    pub(crate) fn unions_iterators_and_failures() {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let value = heap.import_serde(&json!({"a":null,"b":null})).unwrap();
        let schema = heap.import_serde(&json!({"allOf":[{"type":"object","properties":{"a":{"type":"string"}}},{"type":"object","properties":{"b":{"type":"string"}}}]})).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(
            normalized.changed && heap.own_enumerable_string_keys(&normalized.value, &NoRuntime).unwrap().is_empty()
        );
        let schema = heap.import_serde(&json!({"oneOf":[{"type":"object","properties":{"a":{"type":"string"}},"required":["missing"]},{"type":"object","properties":{"b":{"type":"string"}},"required":["missing"]}]})).unwrap();
        let normalized = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap();
        assert!(normalized.changed);
        assert!(!heap.has_own(&normalized.value, &"a".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&normalized.value, &"b".into(), &NoRuntime).unwrap());

        for close_failure in [
            None,
            Some(ValueError::TypeError("close failed".into())),
            Some(ValueError::MissingCapability(CapabilityId(99))),
        ] {
            let branch = heap.import_serde(&json!({"type":"number"})).unwrap();
            let branches = heap.array(Vec::new()).unwrap();
            let iterator = heap.object().unwrap();
            own(&heap, &iterator, "next", heap.function(CapabilityId(11)).unwrap());
            own(&heap, &iterator, "return", heap.function(CapabilityId(12)).unwrap());
            heap.define_own_data(&branches, heap.iterator_key(), heap.function(CapabilityId(10)).unwrap(), &NoRuntime)
                .unwrap();
            let schema = heap.object().unwrap();
            own(&heap, &schema, "anyOf", branches);
            let expected_close = match &close_failure {
                None => 0,
                Some(ValueError::TypeError(_)) => 1,
                _ => 2,
            };
            let close = match close_failure {
                Some(error) => Reply::Error(error),
                None => Reply::Value(heap.object().unwrap()),
            };
            let runtime = Runtime::new(vec![
                (10, vec![Reply::Value(iterator)]),
                (11, vec![Reply::Value(step(&heap, branch))]),
                (12, vec![close]),
            ]);
            let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
            let value = heap.string("2".into()).unwrap();
            let result = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true);
            assert_eq!(*runtime.trace.lock().unwrap(), vec![10, 11, 12]);
            match (expected_close, result) {
                (0, Ok(normalized)) => assert!(normalized.changed),
                (1, Err(ValueError::TypeError(message))) => assert_eq!(message, "close failed"),
                (2, Err(ValueError::MissingCapability(capability))) => {
                    assert_eq!(capability, CapabilityId(99))
                }
                (_, result) => panic!("unexpected early-close receipt: {result:?}"),
            }
        }
        for host_close in [false, true] {
            let branch = heap.object().unwrap();
            getter(&heap, &branch, "anyOf", 23);
            let branches = heap.array(Vec::new()).unwrap();
            let iterator = heap.object().unwrap();
            own(&heap, &iterator, "next", heap.function(CapabilityId(21)).unwrap());
            own(&heap, &iterator, "return", heap.function(CapabilityId(22)).unwrap());
            heap.define_own_data(&branches, heap.iterator_key(), heap.function(CapabilityId(20)).unwrap(), &NoRuntime)
                .unwrap();
            let schema = heap.object().unwrap();
            own(&heap, &schema, "anyOf", branches);
            let thrown = heap.symbol(Some("body exception".into())).unwrap();
            let close = if host_close {
                ValueError::MissingCapability(CapabilityId(98))
            } else {
                ValueError::TypeError("ignored close Native throw".into())
            };
            let runtime = Runtime::new(vec![
                (20, vec![Reply::Value(iterator)]),
                (21, vec![Reply::Value(step(&heap, branch))]),
                (22, vec![Reply::Error(close)]),
                (23, vec![Reply::Error(ValueError::Thrown(thrown.clone()))]),
            ]);
            let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
            let error = normalize_graph_optional_nulls_for_schema(&factory, &schema, &value, true).unwrap_err();
            assert_eq!(*runtime.trace.lock().unwrap(), vec![20, 21, 23, 22]);
            if host_close {
                assert!(
                    matches!(error, ValueError::IteratorClose { original, close } if matches!(&*original, ValueError::Thrown(value) if value.same_identity(&thrown)) && matches!(*close, ValueError::MissingCapability(_)))
                );
            } else {
                assert!(matches!(error, ValueError::Thrown(value) if value.same_identity(&thrown)));
            }
        }
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(
            normalize_graph_optional_nulls_for_schema(&factory, &foreign, &value, true),
            Err(ValueError::ForeignHeap)
        ));
    }
}
