//! Fixed OMP596f2da7101178214aa27a753529d15e6b7ad91d,
//! packages/ai/src/utils/schema/wire.ts:1-693, over actual same-lineage values.
//! Persistent source functions use sealed compiled targets and borrowed execution.
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
use crate::schema_draft::upgrade_graph_json_schema_to_202012_with_execution;
use crate::schema_stamps;
use crate::schema_validator::{graph_numeric_less_than_with_execution, graph_value_less_than_number_with_execution};
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, IteratorRecord, PropertyKey, ValueError, ValueHandle, ValueHeap,
    ValueKind, ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;

const MODULE: &str = "ara-ai.schema.wire";
const NULLABLE_SCALAR_TYPES: &[&str] = &["string", "number", "integer", "boolean"];
const SCHEMA_DEFINING_SIBLING_KEYS: &[&str] = &[
    "$ref",
    "additionalProperties",
    "allOf",
    "anyOf",
    "const",
    "contains",
    "enum",
    "if",
    "items",
    "not",
    "oneOf",
    "patternProperties",
    "prefixItems",
    "properties",
    "propertyNames",
    "then",
    "else",
    "unevaluatedItems",
    "unevaluatedProperties",
];
const SCHEMA_VALUE_KEYS: &[&str] = &[
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "items",
    "contains",
    "propertyNames",
    "if",
    "then",
    "else",
    "not",
];
const SCHEMA_MAP_KEYS: &[&str] = &["properties", "patternProperties", "$defs", "definitions"];
const SCHEMA_ARRAY_KEYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
const STRIP_SCHEMA_VALUE_KEYS: &[&str] = &[
    "additionalProperties",
    "unevaluatedProperties",
    "unevaluatedItems",
    "items",
    "additionalItems",
    "contains",
    "propertyNames",
    "contentSchema",
    "if",
    "then",
    "else",
    "not",
    "anyOf",
    "oneOf",
    "allOf",
    "prefixItems",
];
const STRIP_SCHEMA_MAP_KEYS: &[&str] = &["properties", "patternProperties", "$defs", "definitions", "dependentSchemas"];
const SETS: &[(&str, &[&str])] =
    &[("NULLABLE_SCALAR_TYPES", NULLABLE_SCALAR_TYPES), ("SCHEMA_DEFINING_SIBLING_KEYS", SCHEMA_DEFINING_SIBLING_KEYS)];

/// Prepare only true module literals; parseArkObjectKey's RegExp is fresh per call.
pub(crate) fn prepare_graph_wire_module(heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime) -> Result<(), ValueError> {
    for name in ["pi.schema.json.wire", "pi.schema.ark.wire", "pi.schema.descriptions.stripped"] {
        heap.module_symbol(MODULE, name)?;
    }
    heap.prepare_module_string_sets(MODULE, SETS, runtime)?;
    for (name, keys) in [
        ("SCHEMA_VALUE_KEYS", SCHEMA_VALUE_KEYS),
        ("SCHEMA_MAP_KEYS", SCHEMA_MAP_KEYS),
        ("SCHEMA_ARRAY_KEYS", SCHEMA_ARRAY_KEYS),
        ("STRIP_SCHEMA_VALUE_KEYS", STRIP_SCHEMA_VALUE_KEYS),
        ("STRIP_SCHEMA_MAP_KEYS", STRIP_SCHEMA_MAP_KEYS),
    ] {
        heap.module_string_array(MODULE, name, keys)?;
    }
    Ok(())
}

pub fn is_graph_ark_schema(factory: &GraphValueFactory, value: &ValueHandle) -> Result<bool, ValueError> {
    factory.heap().adopt(value)?;
    Wire::new(factory).is_ark_schema(value)
}
pub fn graph_ark_to_wire_schema(factory: &GraphValueFactory, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(schema)?;
    Wire::new(factory).ark_to_wire_schema(schema)
}
pub fn graph_tool_wire_schema(factory: &GraphValueFactory, tool: &ValueHandle) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(tool)?;
    Wire::new(factory).tool_wire_schema(tool)
}
pub fn graph_tool_wire_schema_in(
    heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime, tool: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    graph_tool_wire_schema_with_execution(GraphExecution::new(heap, runtime), tool)
}
pub fn strip_graph_schema_descriptions_in(
    heap: &Arc<ValueHeap>, runtime: &dyn ValueRuntime, schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    heap.adopt(schema)?;
    Wire { factory: GraphExecution::new(heap, runtime), runtime }.strip_schema_descriptions(schema)
}
/// Retained provider callbacks borrow their invocation runtime and reuse the
/// original wire body; no temporary factory or runtime lease is constructed.
pub(crate) fn graph_tool_wire_schema_with_execution(
    execution: GraphExecution<'_>,
    tool: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    execution.heap().adopt(tool)?;
    Wire { factory: execution, runtime: execution.runtime() }.tool_wire_schema(tool)
}
pub fn flatten_graph_exclusive_required_root_union(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(schema)?;
    Wire::new(factory).flatten_exclusive_required_root_union(schema)
}
pub(crate) fn flatten_graph_exclusive_required_root_union_with_execution(
    execution: GraphExecution<'_>,
    schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    execution.heap().adopt(schema)?;
    Wire { factory: execution, runtime: execution.runtime() }.flatten_exclusive_required_root_union(schema)
}
pub fn normalize_graph_empty_schemas(factory: &GraphValueFactory, schema: &ValueHandle) -> Result<(), ValueError> {
    factory.heap().adopt(schema)?;
    Wire::new(factory).normalize_empty_schemas(schema)
}
pub fn strip_graph_schema_descriptions(
    factory: &GraphValueFactory,
    schema: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(schema)?;
    Wire::new(factory).strip_schema_descriptions(schema)
}
pub fn strip_graph_tool_descriptions(
    factory: &GraphValueFactory,
    tools: &ValueHandle,
) -> Result<ValueHandle, ValueError> {
    factory.heap().adopt(tools)?;
    Wire::new(factory).strip_tool_descriptions(tools)
}

#[derive(Clone, Copy)]
struct Wire<'a> {
    factory: GraphExecution<'a>,
    runtime: &'a dyn ValueRuntime,
}
impl<'a> Wire<'a> {
    fn new(factory: &'a GraphValueFactory) -> Self {
        Self { factory: GraphExecution::from_factory(factory), runtime: factory.runtime() }
    }
    fn get(&self, value: &ValueHandle, key: &str) -> Result<ValueHandle, ValueError> {
        self.factory.heap().get(value, &key.into(), self.runtime)
    }
    fn property_key(&self, value: &ValueHandle) -> Result<PropertyKey, ValueError> {
        let heap = self.factory.heap();
        let value = heap.to_primitive(value, true, self.runtime)?;
        if heap.kind(&value)? == ValueKind::Symbol {
            Ok(PropertyKey::Symbol(heap.symbol_key(&value)?))
        } else {
            Ok(PropertyKey::String(heap.to_string(&value, self.runtime)?))
        }
    }
    fn get_key(&self, value: &ValueHandle, key: &ValueHandle) -> Result<ValueHandle, ValueError> {
        self.factory.heap().get(value, &self.property_key(key)?, self.runtime)
    }
    fn has_own_key(&self, value: &ValueHandle, key: &ValueHandle) -> Result<bool, ValueError> {
        self.factory.heap().has_own(value, &self.property_key(key)?, self.runtime)
    }
    fn own(&self, value: &ValueHandle, key: &str) -> Result<bool, ValueError> {
        self.factory.heap().has_own(value, &key.into(), self.runtime)
    }
    fn has(&self, value: &ValueHandle, key: &str) -> Result<bool, ValueError> {
        self.factory.heap().has(value, &key.into(), self.runtime)
    }
    fn set(&self, value: &ValueHandle, key: &str, entry: ValueHandle) -> Result<(), ValueError> {
        self.factory.heap().assign(value, key.into(), entry, self.runtime)
    }
    fn set_key(&self, value: &ValueHandle, key: &ValueHandle, entry: ValueHandle) -> Result<(), ValueError> {
        self.factory.heap().assign(value, self.property_key(key)?, entry, self.runtime)
    }
    fn delete(&self, value: &ValueHandle, key: &str) -> Result<(), ValueError> {
        self.strict_delete(value, &key.into())
    }
    fn delete_key(&self, value: &ValueHandle, key: &ValueHandle) -> Result<(), ValueError> {
        self.strict_delete(value, &self.property_key(key)?)
    }
    fn strict_delete(&self, value: &ValueHandle, key: &PropertyKey) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        let deleted = match heap.kind(value)? {
            ValueKind::Object | ValueKind::Array | ValueKind::Function => heap.delete_own(value, key, self.runtime)?,
            ValueKind::Null | ValueKind::Undefined => {
                return Err(ValueError::TypeError("Cannot delete a property of null or undefined".into()));
            }
            ValueKind::String => match heap.own_descriptor(value, key, self.runtime)? {
                Some(ara_rpc::value_heap::PropertyDescriptor::Data { configurable, .. })
                | Some(ara_rpc::value_heap::PropertyDescriptor::Accessor { configurable, .. }) => configurable,
                None => true,
            },
            _ => true,
        };
        if deleted { Ok(()) } else { Err(ValueError::TypeError("Cannot delete a non-configurable property".into())) }
    }
    fn text(&self, value: &ValueHandle, text: &str) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().snapshot(value)?, ValueSnapshot::String(value) if value.equals_ascii(text)))
    }
    fn equal(&self, left: &ValueHandle, right: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        Ok(match (heap.snapshot(left)?, heap.snapshot(right)?) {
            (ValueSnapshot::Number(left), ValueSnapshot::Number(right)) => left == right,
            _ => heap.object_is(left, right)?,
        })
    }
    fn number(&self, value: &ValueHandle, number: f64) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().snapshot(value)?, ValueSnapshot::Number(value) if value == number))
    }
    fn record(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Object)
    }
    fn array(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Array)
    }
    fn string(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::String)
    }
    fn undefined(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.factory.heap().kind(value)? == ValueKind::Undefined)
    }
    fn literal(&self, entries: &[(&str, ValueHandle)]) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let value = heap.object()?;
        for (key, entry) in entries {
            heap.define_own_data(&value, (*key).into(), entry.clone(), self.runtime)?;
        }
        Ok(value)
    }
    fn method<F>(&self, receiver: &ValueHandle, name: &str, arguments: F) -> Result<ValueHandle, ValueError>
    where
        F: FnOnce() -> Result<Vec<ValueHandle>, ValueError>,
    {
        let method = self.get(receiver, name)?;
        let arguments = arguments()?;
        self.factory.heap().call(&method, receiver, &arguments, self.runtime)
    }
    fn close_error(&self, iterator: &mut IteratorRecord, error: ValueError) -> ValueError {
        match self.factory.heap().iterator_close(iterator, self.runtime) {
            Ok(()) => error,
            Err(close) if error.is_native_exception() && close.is_native_exception() => error,
            Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
        }
    }
    fn for_of<F>(&self, source: &ValueHandle, mut body: F) -> Result<(), ValueError>
    where
        F: FnMut(&ValueHandle) -> Result<bool, ValueError>,
    {
        let heap = self.factory.heap();
        let mut iterator = heap.get_iterator(source, self.runtime)?;
        while let Some(value) = heap.iterator_next(&mut iterator, self.runtime)? {
            match body(&value) {
                Ok(true) => {}
                Ok(false) => {
                    heap.iterator_close(&mut iterator, self.runtime)?;
                    break;
                }
                Err(error) => return Err(self.close_error(&mut iterator, error)),
            }
        }
        Ok(())
    }
    fn keys(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        heap.array(
            heap.own_enumerable_string_keys(value, self.runtime)?
                .into_iter()
                .map(|key| heap.string(key))
                .collect::<Result<Vec<_>, _>>()?,
        )
    }
    fn module_array(&self, name: &'static str, keys: &'static [&'static str]) -> Result<ValueHandle, ValueError> {
        self.factory.heap().module_string_array(MODULE, name, keys)
    }
    fn module_set(&self, name: &'static str, keys: &'static [&'static str]) -> Result<ValueHandle, ValueError> {
        self.factory.heap().module_string_set(MODULE, name, keys)
    }
    fn is_ark_schema(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        if heap.kind(value)? != ValueKind::Function {
            return Ok(false);
        }
        if heap.kind(&self.get(value, "toJsonSchema")?)? != ValueKind::Function {
            return Ok(false);
        }
        Ok(heap.kind(&self.get(value, "assert")?)? == ValueKind::Function)
    }
    fn is_ark_json_ast(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        if self.array(value)? {
            let method = self.get(value, "some")?;
            let callback = heap.module_compiled_function(MODULE, "isArkJsonAst", &ARK_AST_SPEC)?;
            return heap.truthy(&heap.call(&method, value, &[callback], self.runtime)?);
        }
        if !self.record(value)? {
            return Ok(false);
        }
        if self.string(&self.get(value, "domain")?)? || self.own(value, "unit")? {
            return Ok(true);
        }
        if self.text(&self.get(value, "proto")?, "Array")? && self.own(value, "sequence")? {
            return Ok(true);
        }
        let required = self.get(value, "required")?;
        if !self.array(&required)? {
            return Ok(false);
        }
        let method = self.get(&required, "some")?;
        let callback = heap.fresh_compiled_function(&AST_ENTRY_SPEC, &[])?;
        heap.truthy(&heap.call(&method, &required, &[callback], self.runtime)?)
    }
    fn parse_ark_object_key(&self, key: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let regexp = heap.regexp(r"^(.*?)\s*\/\*\*\s*([\s\S]*?)\s*\*\/\s*$".into(), "")?;
        let matched = self.method(&regexp, "exec", || Ok(vec![key.clone()]))?;
        if !heap.truthy(&matched)? {
            return self.literal(&[("name", key.clone())]);
        }
        let name = self.get(&matched, "1")?;
        let name = self.method(&name, "trim", || Ok(Vec::new()))?;
        let description = self.get(&matched, "2")?;
        let description = self.method(&description, "trim", || Ok(Vec::new()))?;
        self.literal(&[("name", name), ("description", description)])
    }
    fn with_ark_key_description(
        &self,
        schema: &ValueHandle,
        description: &ValueHandle,
    ) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        if !heap.truthy(description)? {
            return Ok(schema.clone());
        }
        if self.record(schema)? {
            if !self.string(&self.get(schema, "description")?)? {
                self.set(schema, "description", description.clone())?;
            }
            return Ok(schema.clone());
        }
        self.literal(&[("anyOf", heap.array(vec![schema.clone()])?), ("description", description.clone())])
    }
    fn ark_json_ast_to_wire(&self, value: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        if self.string(value)? {
            for kind in ["string", "number", "integer", "boolean", "object"] {
                if self.text(value, kind)? {
                    return self.literal(&[("type", value.clone())]);
                }
            }
            return heap.object();
        }
        if self.array(value)? {
            let every = self.get(value, "every")?;
            let predicate = heap.fresh_compiled_function(&UNIT_PREDICATE_SPEC, &[])?;
            let units = heap.truthy(&heap.call(&every, value, &[predicate], self.runtime)?)?;
            let map = self.get(value, "map")?;
            let callback = if units {
                heap.fresh_compiled_function(&UNIT_VALUE_SPEC, &[])?
            } else {
                heap.module_compiled_function(MODULE, "arkJsonAstToWire", &AST_CONVERSION_SPEC)?
            };
            let converted = heap.call(&map, value, &[callback], self.runtime)?;
            return self.literal(&[(if units { "enum" } else { "anyOf" }, converted)]);
        }
        if !self.record(value)? {
            return heap.object();
        }
        if self.own(value, "unit")? {
            return self.literal(&[("const", self.get(value, "unit")?)]);
        }
        if self.text(&self.get(value, "proto")?, "Array")? && self.own(value, "sequence")? {
            return self.literal(&[
                ("type", heap.string("array".into())?),
                ("items", self.ark_json_ast_to_wire(&self.get(value, "sequence")?)?),
            ]);
        }
        if self.text(&self.get(value, "domain")?, "object")? {
            let properties = heap.object()?;
            let required = heap.array(vec![])?;
            let add_entry = |entry: &ValueHandle, is_required: bool| -> Result<(), ValueError> {
                if !self.record(entry)? || !self.string(&self.get(entry, "key")?)? || !self.has(entry, "value")? {
                    return Ok(());
                }
                let key = self.parse_ark_object_key(&self.get(entry, "key")?)?;
                // Native computes the LHS property key before evaluating its RHS.
                let property_key = self.property_key(&self.get(&key, "name")?)?;
                let schema = self.ark_json_ast_to_wire(&self.get(entry, "value")?)?;
                let schema = self.with_ark_key_description(&schema, &self.get(&key, "description")?)?;
                heap.assign(&properties, property_key, schema, self.runtime)?;
                if is_required {
                    self.method(&required, "push", || Ok(vec![self.get(&key, "name")?]))?;
                }
                Ok(())
            };
            if self.array(&self.get(value, "required")?)? {
                self.for_of(&self.get(value, "required")?, |entry| {
                    add_entry(entry, true)?;
                    Ok(true)
                })?;
            }
            if self.array(&self.get(value, "optional")?)? {
                self.for_of(&self.get(value, "optional")?, |entry| {
                    add_entry(entry, false)?;
                    Ok(true)
                })?;
            }
            let schema = self.literal(&[("type", heap.string("object".into())?), ("properties", properties)])?;
            if graph_numeric_less_than_with_execution(self.factory, 0.0, &self.get(&required, "length")?)? {
                self.set(&schema, "required", required)?;
            }
            return Ok(schema);
        }
        if self.string(&self.get(value, "domain")?)? {
            return self.literal(&[("type", self.get(value, "domain")?)]);
        }
        heap.object()
    }
    fn has_schema_defining_sibling(&self, schema: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        for key in heap.for_in_string_keys(schema, self.runtime)? {
            if key.equals_ascii("anyOf") {
                continue;
            }
            let set = self.module_set("SCHEMA_DEFINING_SIBLING_KEYS", SCHEMA_DEFINING_SIBLING_KEYS)?;
            if heap.truthy(&self.method(&set, "has", || Ok(vec![heap.string(key)?]))?)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn is_null_variant(&self, schema: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.text(&self.get(schema, "type")?, "null")?
            && self.number(&self.get(&self.keys(schema)?, "length")?, 1.0)?)
    }
    fn is_scalar_variant(&self, schema: &ValueHandle) -> Result<bool, ValueError> {
        if !self.string(&self.get(schema, "type")?)? {
            return Ok(false);
        }
        let set = self.module_set("NULLABLE_SCALAR_TYPES", NULLABLE_SCALAR_TYPES)?;
        self.factory.heap().truthy(&self.method(&set, "has", || Ok(vec![self.get(schema, "type")?]))?)
    }
    fn copy_nullable_scalar_constraints(&self, schema: &ValueHandle, scalar: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        for key in heap.for_in_string_keys(scalar, self.runtime)? {
            if key.equals_ascii("type")
                || key.equals_ascii("enum")
                || key.equals_ascii("const")
                || heap.has_own(schema, &PropertyKey::String(key.clone()), self.runtime)?
            {
                continue;
            }
            let value = heap.get(scalar, &PropertyKey::String(key.clone()), self.runtime)?;
            heap.assign(schema, key.into(), value, self.runtime)?;
        }
        if self.own(scalar, "const")? {
            self.set(schema, "enum", heap.array(vec![self.get(scalar, "const")?, heap.null()?])?)?;
            return Ok(());
        }
        let values = self.get(scalar, "enum")?;
        if self.array(&values)? {
            let includes = heap.truthy(&self.method(&values, "includes", || Ok(vec![heap.null()?]))?)?;
            let result = if includes {
                values.clone()
            } else {
                let array = heap.array(vec![])?;
                let mut index = 0_u64;
                self.for_of(&values, |entry| {
                    heap.define_own_data(&array, index.to_string().as_str().into(), entry.clone(), self.runtime)?;
                    index += 1;
                    Ok(true)
                })?;
                heap.define_own_data(&array, index.to_string().as_str().into(), heap.null()?, self.runtime)?;
                array
            };
            self.set(schema, "enum", result)?;
        }
        Ok(())
    }
    fn rewrite_nullable_scalar_any_of(&self, schema: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.has_schema_defining_sibling(schema)? {
            return Ok(());
        }
        let variants = self.get(schema, "anyOf")?;
        if !self.array(&variants)? || !self.number(&self.get(&variants, "length")?, 2.0)? {
            return Ok(());
        }
        let mut scalar: Option<ValueHandle> = None;
        let mut scalar_type: Option<ValueHandle> = None;
        let mut null = false;
        let mut invalid = false;
        self.for_of(&variants, |variant| {
            if !self.record(variant)? {
                invalid = true;
                return Ok(false);
            }
            if self.is_null_variant(variant)? {
                if null {
                    invalid = true;
                    return Ok(false);
                }
                null = true;
                return Ok(true);
            }
            if !self.is_scalar_variant(variant)? || scalar.is_some() {
                invalid = true;
                return Ok(false);
            }
            scalar = Some(variant.clone());
            scalar_type = Some(self.get(variant, "type")?);
            Ok(true)
        })?;
        if invalid || !null {
            return Ok(());
        }
        let (Some(scalar), Some(kind)) = (scalar, scalar_type) else {
            return Ok(());
        };
        if !heap.truthy(&kind)? {
            return Ok(());
        }
        self.delete(schema, "anyOf")?;
        self.copy_nullable_scalar_constraints(schema, &scalar)?;
        self.set(schema, "type", heap.array(vec![kind, heap.string("null".into())?])?)
    }
    fn is_exclusive_required_branch(&self, branch: &ValueHandle) -> Result<bool, ValueError> {
        let heap = self.factory.heap();
        if !self.record(branch)? || self.own(branch, "type")? {
            return Ok(false);
        }
        if !self.array(&self.get(branch, "required")?)?
            || self.number(&self.get(&self.get(branch, "required")?, "length")?, 0.0)?
        {
            return Ok(false);
        }
        let required = self.get(branch, "required")?;
        let every = self.get(&required, "every")?;
        let callback = heap.fresh_compiled_function(&REQUIRED_NAME_SPEC, &[])?;
        if !heap.truthy(&heap.call(&every, &required, &[callback], self.runtime)?)? {
            return Ok(false);
        }
        for key in heap.for_in_string_keys(branch, self.runtime)? {
            if !heap.has_own(branch, &PropertyKey::String(key.clone()), self.runtime)? {
                continue;
            }
            if key.equals_ascii("required") || key.equals_ascii("description") || key.equals_ascii("title") {
                continue;
            }
            return Ok(false);
        }
        Ok(true)
    }
    fn flatten_exclusive_required_root_union(&self, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let key = if self.array(&self.get(schema, "anyOf")?)? {
            "anyOf"
        } else if self.array(&self.get(schema, "oneOf")?)? {
            "oneOf"
        } else {
            return Ok(schema.clone());
        };
        let union = self.get(schema, key)?;
        if !self.array(&union)? || self.number(&self.get(&union, "length")?, 0.0)? {
            return Ok(schema.clone());
        }
        let typed = self.text(&self.get(schema, "type")?, "object")?
            || (self.array(&self.get(schema, "type")?)? && {
                let value = self.get(schema, "type")?;
                heap.truthy(&self.method(&value, "includes", || Ok(vec![heap.string("object".into())?]))?)?
            });
        if !typed && !self.record(&self.get(schema, "properties")?)? {
            return Ok(schema.clone());
        }
        let every = self.get(&union, "every")?;
        let callback = heap.module_compiled_function(MODULE, "isExclusiveRequiredBranch", &EXCLUSIVE_BRANCH_SPEC)?;
        if !heap.truthy(&heap.call(&every, &union, &[callback], self.runtime)?)? {
            return Ok(schema.clone());
        }
        let flattened = heap.spread(schema, self.runtime)?;
        self.delete(&flattened, key)?;
        Ok(flattened)
    }
    fn normalize_ark_property_comments(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.normalize_ark_property_comments(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !self.record(node)? {
            return Ok(());
        }
        let properties = self.get(node, "properties")?;
        if self.record(&properties)? {
            let required =
                if self.array(&self.get(node, "required")?)? { self.get(node, "required")? } else { heap.undefined()? };
            if heap.truthy(&required)? {
                let map = self.get(&required, "map")?;
                let callback = heap.fresh_compiled_function(&REQUIRED_KEY_SPEC, &[])?;
                self.set(node, "required", heap.call(&map, &required, &[callback], self.runtime)?)?;
            }
            let keys = self.keys(&properties)?;
            self.for_of(&keys, |key| {
                let parsed = self.parse_ark_object_key(key)?;
                let target = self.get(&parsed, "name")?;
                let mut property_schema = self.get_key(&properties, key)?;
                if heap.truthy(&self.get(&parsed, "description")?)? {
                    property_schema =
                        self.with_ark_key_description(&property_schema, &self.get(&parsed, "description")?)?;
                    self.delete_key(&properties, key)?;
                    let property_key = self.property_key(&target)?;
                    heap.assign(&properties, property_key, property_schema.clone(), self.runtime)?;
                }
                self.normalize_ark_property_comments(&property_schema)?;
                Ok(true)
            })?;
        }
        self.for_of(&self.module_array("SCHEMA_VALUE_KEYS", SCHEMA_VALUE_KEYS)?, |key| {
            if self.has_own_key(node, key)? {
                self.normalize_ark_property_comments(&self.get_key(node, key)?)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_MAP_KEYS", SCHEMA_MAP_KEYS)?, |key| {
            if self.text(key, "properties")? {
                return Ok(true);
            }
            let map = self.get_key(node, key)?;
            if self.record(&map)? {
                for key in heap.for_in_string_keys(&map, self.runtime)? {
                    self.normalize_ark_property_comments(&heap.get(&map, &key.into(), self.runtime)?)?;
                }
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_ARRAY_KEYS", SCHEMA_ARRAY_KEYS)?, |key| {
            let array = self.get_key(node, key)?;
            if self.array(&array)? {
                self.for_of(&array, |child| {
                    self.normalize_ark_property_comments(child)?;
                    Ok(true)
                })?;
            }
            Ok(true)
        })
    }
    fn is_empty_object(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(self.record(value)? && self.number(&self.get(&self.keys(value)?, "length")?, 0.0)?)
    }
    fn homogeneous_enum_scalar_type(&self, values: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        if self.number(&self.get(values, "length")?, 0.0)? {
            return heap.undefined();
        }
        let mut inferred: Option<&str> = None;
        let mut invalid = false;
        self.for_of(values, |value| {
            let scalar = match heap.kind(value)? {
                ValueKind::String => "string",
                ValueKind::Boolean => "boolean",
                ValueKind::Number => "number",
                _ => {
                    invalid = true;
                    return Ok(false);
                }
            };
            if inferred.is_some_and(|inferred| inferred != scalar) {
                invalid = true;
                return Ok(false);
            }
            inferred = Some(scalar);
            Ok(true)
        })?;
        if invalid {
            heap.undefined()
        } else if let Some(kind) = inferred {
            heap.string(kind.into())
        } else {
            heap.undefined()
        }
    }
    fn infer_bare_enum_scalar_type(&self, object: &ValueHandle) -> Result<(), ValueError> {
        if self.has(object, "type")? || !self.array(&self.get(object, "enum")?)? {
            return Ok(());
        }
        let inferred = self.homogeneous_enum_scalar_type(&self.get(object, "enum")?)?;
        if !self.undefined(&inferred)? {
            self.set(object, "type", inferred)?;
        }
        Ok(())
    }
    fn collapse_const_union_any_of(&self, object: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.has_schema_defining_sibling(object)? || self.has(object, "type")? {
            return Ok(());
        }
        let variants = self.get(object, "anyOf")?;
        if !self.array(&variants)?
            || graph_value_less_than_number_with_execution(self.factory, &self.get(&variants, "length")?, 2.0)?
        {
            return Ok(());
        }
        let values = heap.array(vec![])?;
        let mut branch_description = heap.undefined()?;
        let mut described_count = 0.0;
        let mut invalid = false;
        self.for_of(&variants, |variant| {
            if !self.record(variant)? || !self.own(variant, "const")? {
                invalid = true;
                return Ok(false);
            }
            for key in heap.for_in_string_keys(variant, self.runtime)? {
                if !key.equals_ascii("const") && !key.equals_ascii("description") {
                    invalid = true;
                    return Ok(false);
                }
            }
            let description = self.get(variant, "description")?;
            if self.string(&description)? {
                if described_count == 0.0 {
                    branch_description = description;
                } else if !self.equal(&description, &branch_description)? {
                    invalid = true;
                    return Ok(false);
                }
                described_count += 1.0;
            }
            self.method(&values, "push", || Ok(vec![self.get(variant, "const")?]))?;
            Ok(true)
        })?;
        if invalid {
            return Ok(());
        }
        if described_count != 0.0 && !self.number(&self.get(&variants, "length")?, described_count)? {
            return Ok(());
        }
        if self.number(&self.get(&variants, "length")?, described_count)?
            && self.string(&self.get(object, "description")?)?
            && !self.equal(&self.get(object, "description")?, &branch_description)?
        {
            return Ok(());
        }
        let kind = self.homogeneous_enum_scalar_type(&values)?;
        if self.undefined(&kind)? {
            return Ok(());
        }
        self.delete(object, "anyOf")?;
        self.set(object, "type", kind)?;
        self.set(object, "enum", values)?;
        if !self.string(&self.get(object, "description")?)? && !self.undefined(&branch_description)? {
            self.set(object, "description", branch_description)?;
        }
        Ok(())
    }
    fn walk(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.walk(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !heap.truthy(node)? || !self.record(node)? {
            return Ok(());
        }
        self.rewrite_nullable_scalar_any_of(node)?;
        self.infer_bare_enum_scalar_type(node)?;
        self.collapse_const_union_any_of(node)?;
        for key in heap.for_in_string_keys(node, self.runtime)? {
            self.walk(&heap.get(node, &key.into(), self.runtime)?)?;
        }
        Ok(())
    }
    fn normalize_empty_schemas(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.normalize_empty_schemas(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !heap.truthy(node)? || !self.record(node)? {
            return Ok(());
        }
        self.for_of(&self.module_array("SCHEMA_VALUE_KEYS", SCHEMA_VALUE_KEYS)?, |key| {
            if self.has_own_key(node, key)? && self.is_empty_object(&self.get_key(node, key)?)? {
                self.set_key(node, key, heap.boolean(true)?)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_MAP_KEYS", SCHEMA_MAP_KEYS)?, |key| {
            let map = self.get_key(node, key)?;
            if self.record(&map)? {
                for key in heap.for_in_string_keys(&map, self.runtime)? {
                    if self.is_empty_object(&heap.get(&map, &PropertyKey::String(key.clone()), self.runtime)?)? {
                        heap.assign(&map, key.into(), heap.boolean(true)?, self.runtime)?;
                    }
                }
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_ARRAY_KEYS", SCHEMA_ARRAY_KEYS)?, |key| {
            let array = self.get_key(node, key)?;
            if self.array(&array)? {
                let mut index = 0.0;
                while graph_numeric_less_than_with_execution(self.factory, index, &self.get(&array, "length")?)? {
                    let key = heap.to_string(&heap.number(index)?, self.runtime)?;
                    if self.is_empty_object(&heap.get(&array, &PropertyKey::String(key.clone()), self.runtime)?)? {
                        heap.assign(&array, key.into(), heap.boolean(true)?, self.runtime)?;
                    }
                    index += 1.0;
                }
            }
            Ok(true)
        })?;
        for key in heap.for_in_string_keys(node, self.runtime)? {
            self.normalize_empty_schemas(&heap.get(node, &key.into(), self.runtime)?)?;
        }
        Ok(())
    }
    fn close_declared_objects(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.close_declared_objects(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !heap.truthy(node)? || !self.record(node)? {
            return Ok(());
        }
        let object_type = self.text(&self.get(node, "type")?, "object")?
            || (self.array(&self.get(node, "type")?)? && {
                let kinds = self.get(node, "type")?;
                heap.truthy(&self.method(&kinds, "includes", || Ok(vec![heap.string("object".into())?]))?)?
            });
        if object_type
            && !self.undefined(&self.get(node, "properties")?)?
            && !self.has(node, "additionalProperties")?
            && !self.has(node, "patternProperties")?
        {
            self.set(node, "additionalProperties", heap.boolean(false)?)?;
        }
        self.for_of(&self.module_array("SCHEMA_VALUE_KEYS", SCHEMA_VALUE_KEYS)?, |key| {
            if self.has_own_key(node, key)? {
                self.close_declared_objects(&self.get_key(node, key)?)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_MAP_KEYS", SCHEMA_MAP_KEYS)?, |key| {
            let map = self.get_key(node, key)?;
            if self.record(&map)? {
                for key in heap.for_in_string_keys(&map, self.runtime)? {
                    self.close_declared_objects(&heap.get(&map, &key.into(), self.runtime)?)?;
                }
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_ARRAY_KEYS", SCHEMA_ARRAY_KEYS)?, |key| {
            let array = self.get_key(node, key)?;
            if self.array(&array)? {
                self.for_of(&array, |child| {
                    self.close_declared_objects(child)?;
                    Ok(true)
                })?;
            }
            Ok(true)
        })
    }
    fn is_unconstrained_schema(&self, value: &ValueHandle) -> Result<bool, ValueError> {
        Ok(matches!(self.factory.heap().snapshot(value)?, ValueSnapshot::Boolean(true))
            || self.is_empty_object(value)?)
    }
    fn prune_ark_undefined_union_branches(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.prune_ark_undefined_union_branches(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !heap.truthy(node)? || !self.record(node)? {
            return Ok(());
        }
        // Source function-local array literal: fresh on EVERY recursive call.
        let union_keys = heap.array(vec![heap.string("anyOf".into())?, heap.string("oneOf".into())?])?;
        self.for_of(&union_keys, |key| {
            let branches = self.get_key(node, key)?;
            if !self.array(&branches)? {
                return Ok(true);
            }
            let filter = self.get(&branches, "filter")?;
            let callback = heap.fresh_compiled_function(&CONCRETE_BRANCH_SPEC, &[])?;
            let concrete = heap.call(&filter, &branches, &[callback], self.runtime)?;
            if self.equal(&self.get(&concrete, "length")?, &self.get(&branches, "length")?)?
                || self.number(&self.get(&concrete, "length")?, 0.0)?
            {
                return Ok(true);
            }
            let only = if self.number(&self.get(&concrete, "length")?, 1.0)? {
                self.get(&concrete, "0")?
            } else {
                heap.undefined()?
            };
            if !self.undefined(&only)? && self.record(&only)? {
                self.delete_key(node, key)?;
                for key in heap.for_in_string_keys(&only, self.runtime)? {
                    let key = PropertyKey::String(key);
                    if !heap.has(node, &key, self.runtime)? {
                        heap.assign(node, key.clone(), heap.get(&only, &key, self.runtime)?, self.runtime)?;
                    }
                }
            } else {
                self.set_key(node, key, concrete)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_VALUE_KEYS", SCHEMA_VALUE_KEYS)?, |key| {
            if self.has_own_key(node, key)? {
                self.prune_ark_undefined_union_branches(&self.get_key(node, key)?)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_MAP_KEYS", SCHEMA_MAP_KEYS)?, |key| {
            let map = self.get_key(node, key)?;
            if self.record(&map)? {
                for key in heap.for_in_string_keys(&map, self.runtime)? {
                    self.prune_ark_undefined_union_branches(&heap.get(&map, &key.into(), self.runtime)?)?;
                }
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("SCHEMA_ARRAY_KEYS", SCHEMA_ARRAY_KEYS)?, |key| {
            let array = self.get_key(node, key)?;
            if self.array(&array)? {
                self.for_of(&array, |child| {
                    self.prune_ark_undefined_union_branches(child)?;
                    Ok(true)
                })?;
            }
            Ok(true)
        })
    }
    fn post_process_json_schema(&self, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
        self.walk(schema)?;
        self.normalize_ark_property_comments(schema)?;
        self.normalize_empty_schemas(schema)?;
        Ok(schema.clone())
    }
    fn ark_to_wire_schema(&self, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let key = heap.module_symbol(MODULE, "pi.schema.ark.wire")?;
        schema_stamps::stamp_with_execution(self.factory, schema, &key, |schema| {
            // Member Get precedes options object and fallback function creation.
            let method = self.get(schema, "toJsonSchema")?;
            let options = self.literal(&[
                ("target", heap.string("draft-2020-12".into())?),
                ("fallback", heap.argument_property_function(0, "base", "fallback", 1)?),
            ])?;
            let raw = heap.call(&method, schema, &[options], self.runtime)?;
            self.delete(&raw, "$schema")?;
            self.prune_ark_undefined_union_branches(&raw)?;
            let upgraded = upgrade_graph_json_schema_to_202012_with_execution(self.factory, &raw)?;
            let upgraded = self.post_process_json_schema(&upgraded)?;
            self.close_declared_objects(&upgraded)?;
            Ok(upgraded)
        })
    }
    fn tool_wire_schema(&self, tool: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let parameters = self.get(tool, "parameters")?;
        if self.is_ark_schema(&parameters)? {
            return self.ark_to_wire_schema(&parameters);
        }
        let key = self.factory.heap().module_symbol(MODULE, "pi.schema.json.wire")?;
        schema_stamps::stamp_with_execution(self.factory, &parameters, &key, |parameters| {
            let raw = if self.is_ark_json_ast(parameters)? {
                self.ark_json_ast_to_wire(parameters)?
            } else {
                self.factory.heap().structured_clone_json(parameters, self.runtime)?
            };
            let upgraded = upgrade_graph_json_schema_to_202012_with_execution(self.factory, &raw)?;
            self.post_process_json_schema(&upgraded)
        })
    }
    fn strip_schema_descriptions_in_place(&self, node: &ValueHandle) -> Result<(), ValueError> {
        let heap = self.factory.heap();
        if self.array(node)? {
            self.for_of(node, |child| {
                self.strip_schema_descriptions_in_place(child)?;
                Ok(true)
            })?;
            return Ok(());
        }
        if !self.record(node)? {
            return Ok(());
        }
        self.delete(node, "description")?;
        self.for_of(&self.module_array("STRIP_SCHEMA_VALUE_KEYS", STRIP_SCHEMA_VALUE_KEYS)?, |key| {
            if self.has_own_key(node, key)? {
                self.strip_schema_descriptions_in_place(&self.get_key(node, key)?)?;
            }
            Ok(true)
        })?;
        self.for_of(&self.module_array("STRIP_SCHEMA_MAP_KEYS", STRIP_SCHEMA_MAP_KEYS)?, |key| {
            let map = self.get_key(node, key)?;
            if self.record(&map)? {
                for key in heap.for_in_string_keys(&map, self.runtime)? {
                    self.strip_schema_descriptions_in_place(&heap.get(&map, &key.into(), self.runtime)?)?;
                }
            }
            Ok(true)
        })
    }
    fn strip_schema_descriptions(&self, schema: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let key = heap.module_symbol(MODULE, "pi.schema.descriptions.stripped")?;
        schema_stamps::stamp_with_execution(self.factory, schema, &key, |schema| {
            let cloned = heap.structured_clone(schema, self.runtime)?;
            self.strip_schema_descriptions_in_place(&cloned)?;
            Ok(cloned)
        })
    }
    fn strip_tool_descriptions(&self, tools: &ValueHandle) -> Result<ValueHandle, ValueError> {
        let heap = self.factory.heap();
        let map = self.get(tools, "map")?;
        let callback = heap.fresh_compiled_function(&STRIP_TOOL_SPEC, &[])?;
        heap.call(&map, tools, &[callback], self.runtime)
    }
}

// Fixed source callback bodies. Each spec has an immutable contributor-code
// address; zero captures means the saved Function never roots a factory/runtime.
#[derive(Clone, Copy)]
enum WireCallback {
    ArkAst,
    AstEntry,
    UnitPredicate,
    UnitValue,
    AstConversion,
    RequiredName,
    ExclusiveBranch,
    RequiredKey,
    ConcreteBranch,
    StripTool,
}

fn wire_callback_body(
    heap: &Arc<ValueHeap>,
    arguments: &[ValueHandle],
    runtime: &dyn ValueRuntime,
    body: WireCallback,
) -> Result<ValueHandle, ValueError> {
    let item = match arguments.first() {
        Some(item) => item.clone(),
        None => heap.undefined()?,
    };
    let read = Wire { factory: GraphExecution::new(heap, runtime), runtime };
    match body {
        WireCallback::ArkAst => heap.boolean(read.is_ark_json_ast(&item)?),
        WireCallback::AstEntry => {
            heap.boolean(read.record(&item)? && read.string(&read.get(&item, "key")?)? && read.has(&item, "value")?)
        }
        WireCallback::UnitPredicate => heap.boolean(read.record(&item)? && read.own(&item, "unit")?),
        WireCallback::UnitValue => read.get(&item, "unit"),
        WireCallback::AstConversion => read.ark_json_ast_to_wire(&item),
        WireCallback::RequiredName => heap.boolean(
            read.string(&item)?
                && graph_numeric_less_than_with_execution(read.factory, 0.0, &read.get(&item, "length")?)?,
        ),
        WireCallback::ExclusiveBranch => heap.boolean(read.is_exclusive_required_branch(&item)?),
        WireCallback::RequiredKey => {
            if read.string(&item)? {
                read.get(&read.parse_ark_object_key(&item)?, "name")
            } else {
                Ok(item)
            }
        }
        WireCallback::ConcreteBranch => heap.boolean(!read.is_unconstrained_schema(&item)?),
        WireCallback::StripTool => {
            let result = heap.spread(&item, runtime)?;
            heap.define_own_data(&result, "description".into(), heap.string("".into())?, runtime)?;
            let parameters = read.strip_schema_descriptions(&read.tool_wire_schema(&item)?)?;
            heap.define_own_data(&result, "parameters".into(), parameters, runtime)?;
            Ok(result)
        }
    }
}
macro_rules! fixed_wire_callback {
    ($spec:ident, $source:literal, $name:literal, $form:ident, $body:ident) => {
        static $spec: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: $name,
            length: 1,
            form: CompiledFunctionForm::$form,
            capture_count: 0,
            body: |heap, _captures, _receiver, arguments, runtime| {
                wire_callback_body(heap, arguments, runtime, WireCallback::$body)
            },
        };
    };
}
fixed_wire_callback!(ARK_AST_SPEC, "OMP596f2da/wire29:isArkJsonAst", "isArkJsonAst", Ordinary, ArkAst);
fixed_wire_callback!(AST_ENTRY_SPEC, "OMP596f2da/wire36:entry", "", Arrow, AstEntry);
fixed_wire_callback!(UNIT_PREDICATE_SPEC, "OMP596f2da/wire72:unit-predicate", "", Arrow, UnitPredicate);
fixed_wire_callback!(UNIT_VALUE_SPEC, "OMP596f2da/wire73:unit-value", "", Arrow, UnitValue);
fixed_wire_callback!(
    AST_CONVERSION_SPEC,
    "OMP596f2da/wire75:arkJsonAstToWire",
    "arkJsonAstToWire",
    Ordinary,
    AstConversion
);
fixed_wire_callback!(REQUIRED_NAME_SPEC, "OMP596f2da/wire211:required-name", "", Arrow, RequiredName);
fixed_wire_callback!(
    EXCLUSIVE_BRANCH_SPEC,
    "OMP596f2da/wire232:isExclusiveRequiredBranch",
    "isExclusiveRequiredBranch",
    Ordinary,
    ExclusiveBranch
);
fixed_wire_callback!(REQUIRED_KEY_SPEC, "OMP596f2da/wire270:required-key", "", Arrow, RequiredKey);
fixed_wire_callback!(CONCRETE_BRANCH_SPEC, "OMP596f2da/wire552:concrete", "", Arrow, ConcreteBranch);
fixed_wire_callback!(STRIP_TOOL_SPEC, "OMP596f2da/wire688:strip-tool", "", Arrow, StripTool);
#[cfg(test)]
pub(crate) fn observe_graph_wire_family() {
    observations::run();
    crate::schema_stamps::observe_weak_stamp_family();
    crate::schema_validator::property_owner::observe_property_owner_family();
    crate::providers::openai_responses::actual::observe_b91_immutability_family();
}

#[cfg(test)]
mod observations {
    use super::*;
    use ara_rpc::value_heap::{
        CallableTarget, CapabilityId, NoRuntime, PropertyDescriptor, RuntimeReply, RuntimeRequest,
    };
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn own(heap: &Arc<ValueHeap>, receiver: &ValueHandle, key: &str, value: ValueHandle) {
        heap.define_own_data(receiver, key.into(), value, &NoRuntime).unwrap();
    }
    fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str) -> ValueHandle {
        heap.get(value, &key.into(), &NoRuntime).unwrap()
    }
    fn parse(heap: &Arc<ValueHeap>, source: &str) -> ValueHandle {
        heap.parse_strict_units(&source.into()).unwrap()
    }
    fn text(heap: &Arc<ValueHeap>, value: &ValueHandle, expected: &str) -> bool {
        matches!(heap.snapshot(value).unwrap(), ValueSnapshot::String(value) if value.equals_ascii(expected))
    }
    fn tool(heap: &Arc<ValueHeap>, parameters: ValueHandle) -> ValueHandle {
        let value = heap.object().unwrap();
        own(heap, &value, "parameters", parameters);
        value
    }
    fn accessor(heap: &Arc<ValueHeap>, receiver: &ValueHandle, key: &str, capability: u64) {
        heap.define_own(
            receiver,
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
    fn freeze(heap: &Arc<ValueHeap>, receiver: &ValueHandle) {
        for key in heap.own_keys(receiver, &NoRuntime).unwrap() {
            let descriptor = match heap.own_descriptor(receiver, &key, &NoRuntime).unwrap().unwrap() {
                PropertyDescriptor::Data { value, enumerable, .. } => {
                    PropertyDescriptor::Data { value, enumerable, writable: false, configurable: false }
                }
                PropertyDescriptor::Accessor { get, set, enumerable, .. } => {
                    PropertyDescriptor::Accessor { get, set, enumerable, configurable: false }
                }
            };
            heap.define_own(receiver, key, descriptor, &NoRuntime).unwrap();
        }
        heap.set_extensible(receiver, false, &NoRuntime).unwrap();
    }
    struct Runtime {
        heap: Arc<ValueHeap>,
        raw: ValueHandle,
        thrown: ValueHandle,
        schema: Mutex<Option<ValueHandle>>,
        options: Mutex<Vec<ValueHandle>>,
        calls: Mutex<Vec<&'static str>>,
        regexps: Mutex<Vec<ValueHandle>>,
        iterators: Mutex<Vec<ValueHandle>>,
        original_array_iterator: ValueHandle,
        mode: AtomicUsize,
    }
    impl Runtime {
        fn reply(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            let value = match capability.0 {
                1 => {
                    self.calls.lock().unwrap().push("toJsonSchema-get");
                    if self.mode.load(Ordering::SeqCst) == 1 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.heap.function(CapabilityId(10))?
                }
                2 => {
                    self.calls.lock().unwrap().push("assert-get");
                    if self.mode.load(Ordering::SeqCst) == 4 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.heap.function(CapabilityId(11))?
                }
                3 => {
                    self.calls.lock().unwrap().push("base-get");
                    if self.mode.load(Ordering::SeqCst) == 3 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    self.thrown.clone()
                }
                4 => {
                    self.calls.lock().unwrap().push("parameters-get");
                    self.raw.clone()
                }
                5 => {
                    self.calls.lock().unwrap().push("description-get");
                    self.heap.string("original tool description".into())?
                }
                10 => {
                    self.calls.lock().unwrap().push("toJsonSchema-call");
                    assert!(receiver.same_identity(self.schema.lock().unwrap().as_ref().unwrap()));
                    if self.mode.load(Ordering::SeqCst) == 2 {
                        return Err(ValueError::Thrown(self.thrown.clone()));
                    }
                    let options = arguments.first().unwrap();
                    let target = self.heap.get(options, &"target".into(), runtime)?;
                    assert!(text(&self.heap, &target, "draft-2020-12"));
                    let fallback = self.heap.get(options, &"fallback".into(), runtime)?;
                    assert!(text(&self.heap, &self.heap.get(&fallback, &"name".into(), runtime)?, "fallback"));
                    assert!(matches!(
                        self.heap.snapshot(&self.heap.get(&fallback, &"length".into(), runtime)?)?,
                        ValueSnapshot::Number(1.0)
                    ));
                    self.options.lock().unwrap().push(options.clone());
                    let context = self.heap.object()?;
                    accessor(&self.heap, &context, "base", 3);
                    let base = self.heap.call(&fallback, &self.heap.undefined()?, &[context], runtime)?;
                    assert!(base.same_identity(&self.thrown));
                    self.raw.clone()
                }
                20 => {
                    self.regexps.lock().unwrap().push(receiver.clone());
                    self.heap.null()?
                }
                30 => {
                    self.iterators.lock().unwrap().push(receiver.clone());
                    self.heap.call(&self.original_array_iterator, receiver, arguments, runtime)?
                }
                40 => {
                    self.calls.lock().unwrap().push("map-override");
                    self.thrown.clone()
                }
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    impl ValueRuntime for Runtime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.reply(capability, receiver, arguments, self)
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.reply(capability, receiver, arguments, effective_runtime)
        }
    }
    fn binding(heap: &Arc<ValueHeap>, raw: ValueHandle) -> (GraphValueFactory, Arc<Runtime>) {
        let runtime = Arc::new(Runtime {
            heap: heap.clone(),
            raw,
            thrown: heap.symbol(Some("original wire effect".into())).unwrap(),
            schema: Mutex::new(None),
            options: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            regexps: Mutex::new(Vec::new()),
            iterators: Mutex::new(Vec::new()),
            mode: AtomicUsize::new(0),
            original_array_iterator: heap.get(&heap.array_prototype(), &heap.iterator_key(), &NoRuntime).unwrap(),
        });
        (GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap(), runtime)
    }
    fn ark(heap: &Arc<ValueHeap>, runtime: &Runtime) -> ValueHandle {
        let schema = heap.function(CapabilityId(90)).unwrap();
        accessor(heap, &schema, "toJsonSchema", 1);
        accessor(heap, &schema, "assert", 2);
        *runtime.schema.lock().unwrap() = Some(schema.clone());
        schema
    }

    pub(super) fn run() {
        raw_ast_cache_and_guards();
        ark_cache_and_persistent_fallback();
        strip_and_actual_map();
        literal_receivers_and_failures();
        retained_functions_and_current_runtime();
    }

    // One coarse integration observation over actual selected Array methods.
    // Method overrides save the Function then delegate through the effective
    // runtime; no production body is invoked directly by this observation.
    struct RetainedRuntime {
        heap: Arc<ValueHeap>,
        originals: Vec<ValueHandle>,
        saved: Mutex<Vec<ValueHandle>>,
        ark_output: ValueHandle,
    }
    impl RetainedRuntime {
        fn reply(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            let value = match capability.0 {
                60..=63 => {
                    self.saved.lock().unwrap().push(arguments[0].clone());
                    self.heap.call(&self.originals[(capability.0 - 60) as usize], receiver, arguments, runtime)?
                }
                70 => self.ark_output.clone(),
                _ => return Err(ValueError::MissingCapability(capability)),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    impl ValueRuntime for RetainedRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.reply(capability, receiver, arguments, self)
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.reply(capability, receiver, arguments, effective_runtime)
        }
    }
    struct CurrentRuntime {
        parent: Arc<RetainedRuntime>,
        parameters: ValueHandle,
        items: ValueHandle,
        callback: ValueHandle,
        inner: ValueHandle,
        thrown: ValueHandle,
        events: Mutex<Vec<&'static str>>,
    }
    impl CurrentRuntime {
        fn reply(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            let heap = &self.parent.heap;
            let value = match capability.0 {
                90 => {
                    self.events.lock().unwrap().push("parameters-current");
                    self.parameters.clone()
                }
                91 => {
                    self.events.lock().unwrap().push("items-current");
                    self.items.clone()
                }
                92 => {
                    self.events.lock().unwrap().push("same-callback-reentry");
                    heap.call(&self.callback, &heap.undefined()?, std::slice::from_ref(&self.inner), runtime)?
                }
                93 => {
                    self.events.lock().unwrap().push("effect-before-throw");
                    return Err(ValueError::Thrown(self.thrown.clone()));
                }
                _ => return self.parent.reply(capability, receiver, arguments, runtime),
            };
            Ok(RuntimeReply::Value(value))
        }
    }
    impl ValueRuntime for CurrentRuntime {
        fn resolve(&self, request: RuntimeRequest<'_>) -> Result<RuntimeReply, ValueError> {
            match request {
                RuntimeRequest::Call { capability, receiver, arguments } => {
                    self.reply(capability, receiver, arguments, self)
                }
                request => Err(ValueError::MissingCapability(request.capability())),
            }
        }
        fn resolve_call(
            &self,
            capability: CapabilityId,
            receiver: &ValueHandle,
            arguments: &[ValueHandle],
            effective_runtime: &dyn ValueRuntime,
        ) -> Result<RuntimeReply, ValueError> {
            self.reply(capability, receiver, arguments, effective_runtime)
        }
    }
    fn retained_functions_and_current_runtime() {
        let heap = ValueHeap::new().unwrap();
        let weak = Arc::downgrade(&heap);
        {
            let prototype = heap.array_prototype();
            let methods = ["some", "every", "map", "filter"];
            let originals = methods.iter().map(|name| get(&heap, &prototype, name)).collect();
            let runtime = Arc::new(RetainedRuntime {
                heap: heap.clone(),
                originals,
                saved: Mutex::new(Vec::new()),
                ark_output: parse(
                    &heap,
                    r#"{"type":"object","properties":{"value":{"anyOf":[{}, {"type":"string"}]}}}"#,
                ),
            });
            let factory = GraphValueFactory::new(heap.clone(), runtime.clone()).unwrap();
            for (index, name) in methods.into_iter().enumerate() {
                own(&heap, &prototype, name, heap.function(CapabilityId(60 + index as u64)).unwrap());
            }
            let units = parse(&heap, r#"[{"unit":"a"},{"unit":"b"}]"#);
            let output = graph_tool_wire_schema(&factory, &tool(&heap, units.clone())).unwrap();
            assert!(text(&heap, &get(&heap, &output, "type"), "string"));
            let unit_getter = runtime.saved.lock().unwrap()[2].clone();
            graph_tool_wire_schema(&factory, &tool(&heap, parse(&heap, r#"[{"unit":"c"}]"#))).unwrap();
            for _ in 0..2 {
                let nested = parse(&heap, r#"[{"unit":"a"},"string"]"#);
                graph_tool_wire_schema(&factory, &tool(&heap, nested)).unwrap();
                let root = parse(&heap, r#"{"type":"object","anyOf":[{"required":["a"]},{"required":["b"]}]}"#);
                flatten_graph_exclusive_required_root_union(&factory, &root).unwrap();
            }
            let commented = parse(
                &heap,
                r#"{"type":"object","properties":{"p /** help */":{"type":"string"}},"required":["p /** help */"]}"#,
            );
            graph_tool_wire_schema(&factory, &tool(&heap, commented)).unwrap();
            let ark = heap.function(CapabilityId(99)).unwrap();
            own(&heap, &ark, "toJsonSchema", heap.function(CapabilityId(70)).unwrap());
            graph_ark_to_wire_schema(&factory, &ark).unwrap();
            let strip_start = runtime.saved.lock().unwrap().len();
            let tools = heap.array(vec![tool(&heap, parse(&heap, r#"{"type":"string"}"#))]).unwrap();
            strip_graph_tool_descriptions(&factory, &tools).unwrap();
            let strip_callback = runtime.saved.lock().unwrap()[strip_start].clone();
            let saved = runtime.saved.lock().unwrap().clone();
            for name in ["isArkJsonAst", "arkJsonAstToWire", "isExclusiveRequiredBranch"] {
                let named: Vec<_> =
                    saved.iter().filter(|callback| text(&heap, &get(&heap, callback, "name"), name)).collect();
                assert!(named.len() >= 2, "named callback must actually escape repeatedly");
                assert!(named.iter().all(|callback| callback.same_identity(named[0])));
                assert!(heap.has_own(named[0], &"prototype".into(), &NoRuntime).unwrap());
                let prototype = get(&heap, named[0], "prototype");
                assert!(get(&heap, &prototype, "constructor").same_identity(named[0]));
                assert!(matches!(
                    heap.own_descriptor(named[0], &"name".into(), &NoRuntime).unwrap(),
                    Some(PropertyDescriptor::Data { writable: false, enumerable: false, configurable: true, .. })
                ));
            }
            let fresh: Vec<_> =
                saved.iter().filter(|callback| text(&heap, &get(&heap, callback, "name"), "")).collect();
            for (index, callback) in fresh.iter().enumerate() {
                assert!(fresh[..index].iter().all(|prior| !prior.same_identity(callback)));
                assert!(!heap.has_own(callback, &"prototype".into(), &NoRuntime).unwrap());
                assert!(matches!(heap.snapshot(&get(&heap, callback, "length")).unwrap(), ValueSnapshot::Number(1.0)));
            }
            let named_conversion = saved
                .iter()
                .find(|callback| text(&heap, &get(&heap, callback, "name"), "arkJsonAstToWire"))
                .unwrap()
                .clone();
            drop(factory);
            let recursively_converted = heap
                .call(
                    &named_conversion,
                    &heap.undefined().unwrap(),
                    &[parse(&heap, r#"[{"unit":"later"}]"#)],
                    runtime.as_ref(),
                )
                .unwrap();
            assert!(text(&heap, &get(&heap, &get(&heap, &recursively_converted, "enum"), "0"), "later"));

            let parameters = parse(&heap, r#"{"type":"array","additionalItems":false}"#);
            accessor(&heap, &parameters, "items", 91);
            let inner = parse(&heap, r#"{"unit":"reentered"}"#);
            let current = CurrentRuntime {
                parent: runtime.clone(),
                parameters,
                items: parse(&heap, r#"[{"type":"string"}]"#),
                callback: unit_getter.clone(),
                inner,
                thrown: heap.symbol(Some("retained original throw".into())).unwrap(),
                events: Mutex::new(Vec::new()),
            };
            let later_tool = heap.object().unwrap();
            accessor(&heap, &later_tool, "parameters", 90);
            let stripped = heap.call(&strip_callback, &heap.undefined().unwrap(), &[later_tool], &current).unwrap();
            let upgraded = get(&heap, &stripped, "parameters");
            let prefix = get(&heap, &upgraded, "prefixItems");
            let first = get(&heap, &prefix, "0");
            assert!(text(&heap, &get(&heap, &first, "type"), "string"));
            let events = current.events.lock().unwrap().clone();
            assert_eq!(events.iter().filter(|event| **event == "parameters-current").count(), 2);
            assert!(events.contains(&"items-current"), "draft must keep the current invocation runtime");
            let item = heap.object().unwrap();
            accessor(&heap, &item, "unit", 92);
            let result = heap.call(&unit_getter, &heap.undefined().unwrap(), &[item], &current).unwrap();
            assert!(text(&heap, &result, "reentered"));
            let throwing = heap.object().unwrap();
            accessor(&heap, &throwing, "unit", 93);
            assert!(matches!(heap.call(&unit_getter, &heap.undefined().unwrap(), &[throwing], &current),
                Err(ValueError::Thrown(value)) if value.same_identity(&current.thrown)));
            assert!(current.events.lock().unwrap().contains(&"effect-before-throw"));
            assert!(matches!(
                heap.construct_compiled_function(&unit_getter, &[], &current),
                Err(ValueError::TypeError(_))
            ));
        }
        drop(heap);
        assert!(weak.upgrade().is_none(), "saved functions and module functions must not create Arc cycles");
    }
    fn raw_ast_cache_and_guards() {
        let heap = ValueHeap::new().unwrap();
        let factory = GraphValueFactory::new(heap.clone(), Arc::new(NoRuntime)).unwrap();
        let parameters = parse(
            &heap,
            r#"{"type":"object","properties":{"free":{},"state":{"enum":["a","b"]}},"description":"raw"}"#,
        );
        let original_tool = tool(&heap, parameters.clone());
        let wire = graph_tool_wire_schema(&factory, &original_tool).unwrap();
        assert!(!wire.same_identity(&parameters));
        assert!(matches!(heap.snapshot(&get(&heap, &get(&heap, &parameters, "properties"), "free")).unwrap(), ValueSnapshot::Object));
        assert!(graph_tool_wire_schema(&factory, &original_tool).unwrap().same_identity(&wire));
        let properties = get(&heap, &wire, "properties");
        assert!(matches!(heap.snapshot(&get(&heap, &properties, "free")).unwrap(), ValueSnapshot::Boolean(true)));
        assert!(text(&heap, &get(&heap, &get(&heap, &properties, "state"), "type"), "string"));
        assert!(
            !heap.has(&wire, &"additionalProperties".into(), &NoRuntime).unwrap(),
            "raw wire does not run Ark closeDeclaredObjects"
        );
        let cache = heap.module_symbol(MODULE, "pi.schema.json.wire").unwrap();
        assert!(heap.own_descriptor(&parameters, &cache, &NoRuntime).unwrap().is_none());
        let marker = heap.symbol(Some("arbitrary cache alias".into())).unwrap();
        heap.assign(&parameters, cache, marker.clone(), &NoRuntime).unwrap();
        assert!(graph_tool_wire_schema(&factory, &original_tool).unwrap().same_identity(&wire));
        assert!(!wire.same_identity(&marker));

        let parameters = parse(
            &heap,
            r#"{"domain":"object","required":[{"key":" p /** help */ ","value":"string"}],"optional":[{"key":"list","value":{"proto":"Array","sequence":"integer"}}]}"#,
        );
        let wire = graph_tool_wire_schema(&factory, &tool(&heap, parameters.clone())).unwrap();
        assert!(!wire.same_identity(&parameters));
        let properties = get(&heap, &wire, "properties");
        let p = get(&heap, &properties, "p");
        assert!(text(&heap, &get(&heap, &p, "type"), "string"));
        assert!(text(&heap, &get(&heap, &p, "description"), "help"));
        assert!(text(&heap, &get(&heap, &get(&heap, &wire, "required"), "0"), "p"));
        let list = get(&heap, &properties, "list");
        assert!(text(&heap, &get(&heap, &list, "type"), "array"));
        assert!(text(&heap, &get(&heap, &get(&heap, &list, "items"), "type"), "integer"));
        let array = parse(&heap, r#"[{"unit":"a"},{"unit":"b"}]"#);
        let wire = graph_tool_wire_schema(&factory, &tool(&heap, array)).unwrap();
        assert!(text(&heap, &get(&heap, &wire, "type"), "string"));

        let nullable = parse(
            &heap,
            r#"{"description":"nullable","anyOf":[{"type":"string","minLength":2,"enum":["a"]},{"type":"null"}]}"#,
        );
        let wire = graph_tool_wire_schema(&factory, &tool(&heap, nullable.clone())).unwrap();
        assert!(!wire.same_identity(&nullable));
        assert!(heap.has_own(&nullable, &"anyOf".into(), &NoRuntime).unwrap());
        assert!(!heap.has_own(&wire, &"anyOf".into(), &NoRuntime).unwrap());
        assert!(text(&heap, &get(&heap, &get(&heap, &wire, "type"), "0"), "string"));
        assert!(matches!(heap.snapshot(&get(&heap, &get(&heap, &wire, "enum"), "1")).unwrap(), ValueSnapshot::Null));
        assert!(matches!(heap.snapshot(&get(&heap, &wire, "minLength")).unwrap(), ValueSnapshot::Number(2.0)));
        for source in [
            r#"{"description":"same","anyOf":[{"const":"a","description":"same"},{"const":"b","description":"same"}]}"#,
            r#"{"anyOf":[{"const":"a"},{"const":"b"}]}"#,
        ] {
            let wire = graph_tool_wire_schema(&factory, &tool(&heap, parse(&heap, source))).unwrap();
            assert!(!heap.has_own(&wire, &"anyOf".into(), &NoRuntime).unwrap());
            assert!(text(&heap, &get(&heap, &wire, "type"), "string"));
        }
        for source in [
            r#"{"description":"root","anyOf":[{"const":"a","description":"a"},{"const":"b","description":"b"}]}"#,
            r#"{"anyOf":[{"const":"a"},{"const":2}]}"#,
            r#"{"anyOf":[{"const":"a","minLength":1},{"const":"b"}]}"#,
        ] {
            let wire = graph_tool_wire_schema(&factory, &tool(&heap, parse(&heap, source))).unwrap();
            assert!(heap.has_own(&wire, &"anyOf".into(), &NoRuntime).unwrap());
        }
        let schema = parse(
            &heap,
            r#"{"type":"object","properties":{},"anyOf":[{"required":["a"]},{"required":["b"]}],"unknown":{"x":1}}"#,
        );
        let sibling = get(&heap, &schema, "unknown");
        let symbol = heap.symbol(None).unwrap();
        let symbol_key = PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap());
        heap.define_own_data(&schema, symbol_key.clone(), sibling.clone(), &NoRuntime).unwrap();
        let flattened = flatten_graph_exclusive_required_root_union(&factory, &schema).unwrap();
        assert!(!flattened.same_identity(&schema));
        assert!(get(&heap, &flattened, "unknown").same_identity(&sibling));
        assert!(heap.get(&flattened, &symbol_key, &NoRuntime).unwrap().same_identity(&sibling));
        assert!(!heap.has_own(&flattened, &"anyOf".into(), &NoRuntime).unwrap());
        assert!(heap.has_own(&schema, &"anyOf".into(), &NoRuntime).unwrap());
        let invalid = parse(&heap, r#"{"type":"object","oneOf":[{"type":"object","required":["a"]}]}"#);
        assert!(flatten_graph_exclusive_required_root_union(&factory, &invalid).unwrap().same_identity(&invalid));

        let empty = parse(&heap, r#"{"properties":{"free":{}},"items":{},"anyOf":[{}],"default":{"items":{}}}"#);
        normalize_graph_empty_schemas(&factory, &empty).unwrap();
        assert!(matches!(
            heap.snapshot(&get(&heap, &get(&heap, &empty, "properties"), "free")).unwrap(),
            ValueSnapshot::Boolean(true)
        ));
        assert!(
            matches!(
                heap.snapshot(&get(&heap, &get(&heap, &empty, "default"), "items")).unwrap(),
                ValueSnapshot::Boolean(true)
            ),
            "fixed normalizeEmptySchemas has broad final for-in, unlike the existing tree facade"
        );
        let frozen = parse(&heap, r#"{"type":"string"}"#);
        freeze(&heap, &frozen);
        let frozen_tool = tool(&heap, frozen.clone());
        let frozen_wire = graph_tool_wire_schema(&factory, &frozen_tool).unwrap();
        assert!(!frozen_wire.same_identity(&frozen));
        assert!(graph_tool_wire_schema(&factory, &frozen_tool).unwrap().same_identity(&frozen_wire));
        assert!(
            !heap.has_own(&frozen, &heap.module_symbol(MODULE, "pi.schema.json.wire").unwrap(), &NoRuntime).unwrap()
        );
        let sealed = parse(&heap, r#"{"type":"string"}"#);
        heap.set_extensible(&sealed, false, &NoRuntime).unwrap();
        let sealed_tool = tool(&heap, sealed.clone());
        let sealed_wire = graph_tool_wire_schema(&factory, &sealed_tool).unwrap();
        assert!(!sealed_wire.same_identity(&sealed));
        assert!(graph_tool_wire_schema(&factory, &sealed_tool).unwrap().same_identity(&sealed_wire));
    }
    fn ark_cache_and_persistent_fallback() {
        let heap = ValueHeap::new().unwrap();
        let raw = parse(
            &heap,
            r#"{"$schema":"native","type":"object","properties":{"p":{"description":"pdoc","anyOf":[{"type":"string"},{}]},"q":{"oneOf":[true,{"type":"number"}]}},"required":["p"]}"#,
        );
        let (factory, runtime) = binding(&heap, raw.clone());
        let schema = ark(&heap, &runtime);
        let original_tool = tool(&heap, schema.clone());
        let wire = graph_tool_wire_schema(&factory, &original_tool).unwrap();
        assert!(wire.same_identity(&raw));
        assert_eq!(
            *runtime.calls.lock().unwrap(),
            ["toJsonSchema-get", "assert-get", "toJsonSchema-get", "toJsonSchema-call", "base-get"]
        );
        assert!(!heap.has_own(&raw, &"$schema".into(), &NoRuntime).unwrap());
        assert!(matches!(
            heap.snapshot(&get(&heap, &wire, "additionalProperties")).unwrap(),
            ValueSnapshot::Boolean(false)
        ));
        let properties = get(&heap, &wire, "properties");
        let p = get(&heap, &properties, "p");
        assert!(text(&heap, &get(&heap, &p, "type"), "string"));
        assert!(text(&heap, &get(&heap, &p, "description"), "pdoc"));
        assert!(text(&heap, &get(&heap, &get(&heap, &wire, "required"), "0"), "p"));
        assert!(text(&heap, &get(&heap, &get(&heap, &properties, "q"), "type"), "number"));
        runtime.calls.lock().unwrap().clear();
        assert!(graph_tool_wire_schema(&factory, &original_tool).unwrap().same_identity(&wire));
        assert_eq!(*runtime.calls.lock().unwrap(), ["toJsonSchema-get", "assert-get"]);
        let options = runtime.options.lock().unwrap()[0].clone();
        let fallback = get(&heap, &options, "fallback");
        let context = heap.object().unwrap();
        accessor(&heap, &context, "base", 3);
        assert!(
            heap.call(&fallback, &heap.null().unwrap(), &[context], factory.runtime())
                .unwrap()
                .same_identity(&runtime.thrown),
            "saved fallback remains callable after toJsonSchema returns"
        );
        runtime.mode.store(3, Ordering::SeqCst);
        let context = heap.object().unwrap();
        accessor(&heap, &context, "base", 3);
        assert!(
            matches!(heap.call(&fallback, &heap.undefined().unwrap(), &[context], factory.runtime()), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
        );
        assert!(matches!(
            heap.call(&fallback, &heap.undefined().unwrap(), &[], factory.runtime()),
            Err(ValueError::TypeError(_))
        ));
        runtime.mode.store(0, Ordering::SeqCst);
        let frozen_schema = ark(&heap, &runtime);
        freeze(&heap, &frozen_schema);
        let frozen_tool = tool(&heap, frozen_schema.clone());
        graph_tool_wire_schema(&factory, &frozen_tool).unwrap();
        graph_tool_wire_schema(&factory, &frozen_tool).unwrap();
        let options = runtime.options.lock().unwrap();
        assert_eq!(options.len(), 2); // frozen schemas now reuse their cached conversion
        assert!(
            !heap
                .has_own(&frozen_schema, &heap.module_symbol(MODULE, "pi.schema.ark.wire").unwrap(), &NoRuntime)
                .unwrap()
        );
        drop(options);
        for mode in [1, 2, 4] {
            let schema = ark(&heap, &runtime);
            runtime.mode.store(mode, Ordering::SeqCst);
            assert!(
                matches!(graph_tool_wire_schema(&factory, &tool(&heap, schema)), Err(ValueError::Thrown(value)) if value.same_identity(&runtime.thrown))
            );
        }
    }
    fn strip_and_actual_map() {
        let heap = ValueHeap::new().unwrap();
        let raw = parse(
            &heap,
            r#"{"type":"object","description":"root-doc","properties":{"description":{"type":"string","description":"field-doc"},"free":{}},"default":{"description":"literal-data","properties":{"description":"literal-name"}}}"#,
        );
        let raw_before = heap.export_serde(&raw).unwrap();
        let (factory, runtime) = binding(&heap, raw.clone());
        let source = heap.object().unwrap();
        accessor(&heap, &source, "description", 5);
        accessor(&heap, &source, "parameters", 4);
        let unknown = heap.object().unwrap();
        own(&heap, &source, "unknown", unknown.clone());
        let symbol = heap.symbol(None).unwrap();
        let symbol_key = PropertyKey::Symbol(heap.symbol_key(&symbol).unwrap());
        heap.define_own_data(&source, symbol_key.clone(), unknown.clone(), &NoRuntime).unwrap();
        let stripped = strip_graph_tool_descriptions(&factory, &heap.array(vec![source.clone()]).unwrap()).unwrap();
        let stripped_tool = get(&heap, &stripped, "0");
        assert!(text(&heap, &get(&heap, &stripped_tool, "description"), ""));
        assert!(get(&heap, &stripped_tool, "unknown").same_identity(&unknown));
        assert!(heap.get(&stripped_tool, &symbol_key, &NoRuntime).unwrap().same_identity(&unknown));
        assert_eq!(*runtime.calls.lock().unwrap(), ["description-get", "parameters-get", "parameters-get"]);
        let parameters = get(&heap, &stripped_tool, "parameters");
        assert!(!parameters.same_identity(&raw));
        assert!(!heap.has_own(&parameters, &"description".into(), &NoRuntime).unwrap());
        let description = get(&heap, &get(&heap, &parameters, "properties"), "description");
        assert!(text(&heap, &get(&heap, &description, "type"), "string"));
        assert!(!heap.has_own(&description, &"description".into(), &NoRuntime).unwrap());
        assert!(text(&heap, &get(&heap, &get(&heap, &parameters, "default"), "description"), "literal-data"));
        assert!(text(&heap, &get(&heap, &raw, "description"), "root-doc"));
        // stripToolDescriptions memoizes against the wire clone, not raw input.
        let wire_schema = graph_tool_wire_schema(&factory, &source).unwrap();
        assert!(!wire_schema.same_identity(&raw));
        assert!(strip_graph_schema_descriptions(&factory, &wire_schema).unwrap().same_identity(&parameters));
        assert!(graph_tool_wire_schema(&factory, &source).unwrap().same_identity(&wire_schema));
        assert!(text(&heap, &get(&heap, &wire_schema, "description"), "root-doc"));
        let raw_stripped = strip_graph_schema_descriptions(&factory, &raw).unwrap();
        assert!(!raw_stripped.same_identity(&raw));
        assert!(!raw_stripped.same_identity(&parameters));
        assert!(strip_graph_schema_descriptions(&factory, &raw).unwrap().same_identity(&raw_stripped));
        assert!(!heap.has_own(&raw_stripped, &"description".into(), &NoRuntime).unwrap());
        assert!(matches!(heap.snapshot(&get(&heap, &get(&heap, &raw_stripped, "properties"), "free")).unwrap(), ValueSnapshot::Object));
        assert!(matches!(heap.snapshot(&get(&heap, &get(&heap, &parameters, "properties"), "free")).unwrap(), ValueSnapshot::Boolean(true)));
        assert_eq!(heap.export_serde(&raw).unwrap(), raw_before);
        let function = heap.function(CapabilityId(11)).unwrap();
        let invalid = heap.object().unwrap();
        own(&heap, &invalid, "default", function);
        assert!(
            matches!(strip_graph_schema_descriptions(&factory, &invalid), Err(ValueError::DataCloneError(_))),
            "strip has no cloneJSON fallback"
        );
        let tools = heap.array(vec![source]).unwrap();
        own(&heap, &tools, "map", heap.function(CapabilityId(40)).unwrap());
        runtime.calls.lock().unwrap().clear();
        assert!(strip_graph_tool_descriptions(&factory, &tools).unwrap().same_identity(&runtime.thrown));
        assert_eq!(*runtime.calls.lock().unwrap(), ["map-override"]);
    }
    fn literal_receivers_and_failures() {
        let heap = ValueHeap::new().unwrap();
        let (factory, runtime) = binding(&heap, heap.object().unwrap());
        let probe = heap.regexp("".into(), "").unwrap();
        let prototype = heap.prototype(&probe, &NoRuntime).unwrap().unwrap();
        own(&heap, &prototype, "exec", heap.function(CapabilityId(20)).unwrap());
        let read = Wire::new(&factory);
        for _ in 0..2 {
            let key = heap.string("unchanged".into()).unwrap();
            let parsed = read.parse_ark_object_key(&key).unwrap();
            assert!(get(&heap, &parsed, "name").same_identity(&key));
        }
        let regexps = runtime.regexps.lock().unwrap();
        assert_eq!(regexps.len(), 2);
        assert!(!regexps[0].same_identity(&regexps[1]));
        drop(regexps);
        heap.define_own_data(
            &heap.array_prototype(),
            heap.iterator_key(),
            heap.function(CapabilityId(30)).unwrap(),
            &NoRuntime,
        )
        .unwrap();
        read.prune_ark_undefined_union_branches(&heap.object().unwrap()).unwrap();
        read.prune_ark_undefined_union_branches(&heap.object().unwrap()).unwrap();
        let iterations = runtime.iterators.lock().unwrap();
        let fresh: Vec<_> = iterations
            .iter()
            .filter(|array| matches!(heap.snapshot(&get(&heap, array, "length")).unwrap(), ValueSnapshot::Number(2.0)))
            .collect();
        assert_eq!(fresh.len(), 2);
        assert!(!fresh[0].same_identity(fresh[1]));
        let stable: Vec<_> = iterations
            .iter()
            .filter(|array| {
                matches!(heap.snapshot(&get(&heap, array, "length")).unwrap(), ValueSnapshot::Number(4.0))
                    && text(&heap, &get(&heap, array, "0"), "properties")
            })
            .collect();
        assert_eq!(stable.len(), 2);
        assert!(stable[0].same_identity(stable[1]));
        drop(iterations);
        let object = heap.object().unwrap();
        heap.define_own(
            &object,
            "$schema".into(),
            PropertyDescriptor::Data {
                value: heap.string("locked".into()).unwrap(),
                writable: true,
                enumerable: true,
                configurable: false,
            },
            &NoRuntime,
        )
        .unwrap();
        assert!(matches!(read.delete(&object, "$schema"), Err(ValueError::TypeError(_))));
        read.delete(&heap.boolean(false).unwrap(), "$schema").unwrap();
        assert!(matches!(read.delete(&heap.null().unwrap(), "$schema"), Err(ValueError::TypeError(_))));
        let foreign = ValueHeap::new().unwrap().object().unwrap();
        assert!(matches!(graph_tool_wire_schema(&factory, &foreign), Err(ValueError::ForeignHeap)));
    }
}
