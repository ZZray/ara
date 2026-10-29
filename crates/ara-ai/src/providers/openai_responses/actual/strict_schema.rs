//! Fixed normalize.ts OpenAI strict schema body/stamps (OMP596f2da, MIT).
use super::{
    callbacks::{self, Exec},
    schema,
};
use crate::{graph_values::GraphExecution, schema_stamps};

#[cfg(test)]
pub(crate) fn observe_b91_immutability_family() {
    use ara_rpc::value_heap::NoRuntime;
    use serde_json::json;
    // Native JSON observes values of frozen/sealed/DAG graphs. Object state
    // and aliases are asserted separately; this is not the strict legacy bridge.
    fn json_value(heap: &std::sync::Arc<ara_rpc::value_heap::ValueHeap>, value: &ValueHandle) -> serde_json::Value {
        let text = heap.stringify(value, StringifyMode::Plain, None, &NoRuntime).unwrap().unwrap();
        serde_json::from_str(&text.to_utf8().unwrap()).unwrap()
    }
    fn seal(heap: &std::sync::Arc<ara_rpc::value_heap::ValueHeap>, value: &ValueHandle) {
        for key in heap.own_keys(value, &NoRuntime).unwrap() {
            let descriptor = match heap.own_descriptor(value, &key, &NoRuntime).unwrap().unwrap() {
                PropertyDescriptor::Data { value, writable, enumerable, .. } => PropertyDescriptor::Data { value, writable, enumerable, configurable: false },
                PropertyDescriptor::Accessor { get, set, enumerable, .. } => PropertyDescriptor::Accessor { get, set, enumerable, configurable: false },
            };
            heap.define_own(value, key, descriptor, &NoRuntime).unwrap();
        }
        heap.set_extensible(value, false, &NoRuntime).unwrap();
        assert!(!heap.is_extensible(value, &NoRuntime).unwrap());
    }
    fn freeze(heap: &std::sync::Arc<ara_rpc::value_heap::ValueHeap>, value: &ValueHandle, deep: bool, seen: &mut std::collections::HashSet<ara_rpc::value_heap::NodeId>) {
        if !matches!(heap.kind(value).unwrap(), ValueKind::Object | ValueKind::Array | ValueKind::Function) || !seen.insert(value.node_id()) { return; }
        for key in heap.own_keys(value, &NoRuntime).unwrap() {
            let Some(descriptor) = heap.own_descriptor(value, &key, &NoRuntime).unwrap() else { continue; };
            let descriptor = match descriptor {
                PropertyDescriptor::Data { value, enumerable, .. } => {
                    if deep { freeze(heap, &value, true, seen); }
                    PropertyDescriptor::Data { value, enumerable, writable: false, configurable: false }
                },
                PropertyDescriptor::Accessor { get, set, enumerable, .. } => PropertyDescriptor::Accessor { get, set, enumerable, configurable: false },
            };
            heap.define_own(value, key, descriptor, &NoRuntime).unwrap();
        }
        heap.set_extensible(value, false, &NoRuntime).unwrap();
        assert!(!heap.is_extensible(value, &NoRuntime).unwrap());
        for key in heap.own_keys(value, &NoRuntime).unwrap() {
            assert!(matches!(heap.own_descriptor(value, &key, &NoRuntime).unwrap().unwrap(),
                PropertyDescriptor::Data { writable: false, configurable: false, .. }
                | PropertyDescriptor::Accessor { configurable: false, .. }));
        }
    }
    let heap = ara_rpc::value_heap::ValueHeap::new().unwrap();
    let factory = crate::graph_values::GraphValueFactory::new(heap.clone(), std::sync::Arc::new(NoRuntime)).unwrap();
    let ex = Exec { heap: &heap, runtime: &NoRuntime };
    let source = heap.import_serde(&json!({"type":"object","properties":{"extra":{}},"required":["extra"]})).unwrap();
    let source_keys = heap.own_keys(&source, &NoRuntime).unwrap();
    let source_required = heap.get(&source, &"required".into(), &NoRuntime).unwrap();
    freeze(&heap, &source, true, &mut Default::default());
    let tool = heap.object().unwrap();
    heap.define_own_data(&tool, "parameters".into(), source.clone(), &NoRuntime).unwrap();
    let wire = crate::schema_wire::graph_tool_wire_schema(&factory, &tool).unwrap();
    assert_eq!(json_value(&heap, &wire), json!({"type":"object","properties":{"extra":true},"required":["extra"]}));
    assert_eq!(json_value(&heap, &source), json!({"type":"object","properties":{"extra":{}},"required":["extra"]}));
    assert!(!wire.same_identity(&source));
    assert!(heap.get(&source, &"required".into(), &NoRuntime).unwrap().same_identity(&source_required));
    assert_eq!(heap.own_keys(&source, &NoRuntime).unwrap(), source_keys);
    assert!(matches!(heap.export_serde(&source), Err(ValueError::LegacyBridge(_))), "strict bridge still rejects frozen caller state");
    let leaf = heap.import_serde(&json!({"type":"string","description":"value"})).unwrap();
    heap.set_extensible(&leaf, false, &NoRuntime).unwrap();
    let props = heap.object().unwrap();
    heap.define_own_data(&props, "a".into(), leaf.clone(), &NoRuntime).unwrap();
    heap.define_own_data(&props, "b".into(), leaf.clone(), &NoRuntime).unwrap();
    let root = heap.import_serde(&json!({"type":"object","description":"root","required":["a","b"]})).unwrap();
    heap.define_own_data(&root, "properties".into(), props.clone(), &NoRuntime).unwrap();
    seal(&heap, &root);
    assert!(matches!(heap.own_descriptor(&root, &"properties".into(), &NoRuntime).unwrap(), Some(PropertyDescriptor::Data { writable: true, configurable: false, .. })));
    assert!(!heap.is_extensible(&leaf, &NoRuntime).unwrap());
    let root_before = json_value(&heap, &root);
    let root_keys = heap.own_keys(&root, &NoRuntime).unwrap();
    let stripped = crate::schema_wire::strip_graph_schema_descriptions(&factory, &root).unwrap();
    assert_eq!(json_value(&heap, &stripped), json!({"type":"object","properties":{"a":{"type":"string"},"b":{"type":"string"}},"required":["a","b"]}));
    let expected = json!({"type":"object","description":"root","properties":{"a":{"type":"string","description":"value"},"b":{"type":"string","description":"value"}},"required":["a","b"],"additionalProperties":false});
    assert_eq!(json_value(&heap, &enforce(ex, &root, &schema::memo(ex).unwrap()).unwrap()), expected);
    assert_eq!(json_value(&heap, &root), root_before);
    assert_eq!(heap.own_keys(&root, &NoRuntime).unwrap(), root_keys);
    assert!(heap.get(&root, &"properties".into(), &NoRuntime).unwrap().same_identity(&props));
    assert!(heap.get(&props, &"a".into(), &NoRuntime).unwrap().same_identity(&leaf));
    assert!(heap.get(&props, &"b".into(), &NoRuntime).unwrap().same_identity(&leaf));
    freeze(&heap, &root, true, &mut Default::default());
    assert_eq!(json_value(&heap, &enforce(ex, &root, &schema::memo(ex).unwrap()).unwrap()), expected);
    assert_eq!(json_value(&heap, &root), root_before);
    assert_eq!(heap.own_keys(&root, &NoRuntime).unwrap(), root_keys);
    assert!(heap.get(&root, &"properties".into(), &NoRuntime).unwrap().same_identity(&props));
    assert!(heap.get(&props, &"a".into(), &NoRuntime).unwrap().same_identity(&leaf));
    assert!(heap.get(&props, &"b".into(), &NoRuntime).unwrap().same_identity(&leaf));
    let mutable_props = heap.object().unwrap();
    let frozen_after_visit = heap.object().unwrap();
    heap.define_own_data(&frozen_after_visit, "properties".into(), mutable_props.clone(), &NoRuntime).unwrap();
    assert!(crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &frozen_after_visit).unwrap().same_identity(&frozen_after_visit));
    freeze(&heap, &frozen_after_visit, false, &mut Default::default());
    heap.define_own_data(&mutable_props, "legacy".into(), heap.import_serde(&json!({"type":"string","nullable":true})).unwrap(), &NoRuntime).unwrap();
    let upgraded = crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &frozen_after_visit).unwrap();
    assert_eq!(json_value(&heap, &upgraded), json!({"properties":{"legacy":{"type":["string","null"]}}}));
    assert!(!upgraded.same_identity(&frozen_after_visit));
    assert!(!heap.is_extensible(&frozen_after_visit, &NoRuntime).unwrap());
    assert!(heap.get(&frozen_after_visit, &"properties".into(), &NoRuntime).unwrap().same_identity(&mutable_props));
    assert_eq!(json_value(&heap, &frozen_after_visit), json!({"properties":{"legacy":{"type":"string","nullable":true}}}));
    let cycle = heap.import_serde(&json!({"type":"object","required":["self"]})).unwrap();
    let cyclic_props = heap.object().unwrap();
    heap.define_own_data(&cyclic_props, "self".into(), cycle.clone(), &NoRuntime).unwrap();
    heap.define_own_data(&cycle, "properties".into(), cyclic_props.clone(), &NoRuntime).unwrap();
    freeze(&heap, &cycle, false, &mut Default::default());
    for _ in 0..2 { assert!(matches!(enforce(ex, &cycle, &schema::memo(ex).unwrap()), Err(ValueError::TypeError(message)) if message.contains("circular object graph"))); }
    assert!(crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &cycle).unwrap().same_identity(&cycle));
    let repaired_leaf = heap.import_serde(&json!({"type":"string"})).unwrap();
    freeze(&heap, &repaired_leaf, false, &mut Default::default());
    heap.assign(&cyclic_props, "self".into(), repaired_leaf.clone(), &NoRuntime).unwrap();
    let repaired = enforce(ex, &cycle, &schema::memo(ex).unwrap()).unwrap();
    assert_eq!(json_value(&heap, &repaired), json!({"type":"object","properties":{"self":{"type":"string"}},"required":["self"],"additionalProperties":false}));
    assert!(!heap.is_extensible(&cycle, &NoRuntime).unwrap());
    assert!(heap.get(&cycle, &"properties".into(), &NoRuntime).unwrap().same_identity(&cyclic_props));
    assert!(heap.get(&cyclic_props, &"self".into(), &NoRuntime).unwrap().same_identity(&repaired_leaf));
    let metadata = heap.import_serde(&json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]})).unwrap();
    let helper = heap.argument_property_function(0, "base", "", 1).unwrap();
    heap.define_own_data(&metadata, "x-omp-coerce".into(), helper.clone(), &NoRuntime).unwrap();
    heap.assign(&tool, "parameters".into(), metadata.clone(), &NoRuntime).unwrap();
    let wire = crate::schema_wire::graph_tool_wire_schema(&factory, &tool).unwrap();
    assert!(!heap.has_own(&wire, &"x-omp-coerce".into(), &NoRuntime).unwrap());
    assert!(heap.get(&metadata, &"x-omp-coerce".into(), &NoRuntime).unwrap().same_identity(&helper));
    assert_eq!(json_value(&heap, &wire), json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}));
    // First visit then recursive Reflect.ownKeys freeze must see no bookkeeping.
    let deep = heap.import_serde(&json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"]})).unwrap();
    let before = json_value(&heap, &deep);
    let deep_keys = heap.own_keys(&deep, &NoRuntime).unwrap();
    let deep_expected = json!({"type":"object","properties":{"a":{"type":"string"}},"required":["a"],"additionalProperties":false});
    crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &deep).unwrap();
    assert_eq!(json_value(&heap, &enforce(ex, &deep, &schema::memo(ex).unwrap()).unwrap()), deep_expected);
    freeze(&heap, &deep, true, &mut Default::default());
    crate::schema_draft::upgrade_graph_json_schema_to_202012(&factory, &deep).unwrap();
    assert_eq!(json_value(&heap, &enforce(ex, &deep, &schema::memo(ex).unwrap()).unwrap()), deep_expected);
    assert_eq!(json_value(&heap, &deep), before);
    assert_eq!(heap.own_keys(&deep, &NoRuntime).unwrap(), deep_keys);
    assert!(!heap.is_extensible(&deep, &NoRuntime).unwrap());
}
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, StringifyMode, ValueError, ValueHandle, ValueHeap,
    ValueKind, ValueRuntime, ValueSnapshot,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
const NON_STRUCTURAL: &[&str] = &[
    "format",
    "pattern",
    "minLength",
    "maxLength",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
    "$schema",
    "examples",
    "default",
    "title",
    "$comment",
    "if",
    "then",
    "else",
    "not",
    "unevaluatedProperties",
    "unevaluatedItems",
    "patternProperties",
    "propertyNames",
    "contains",
    "minContains",
    "maxContains",
    "dependentRequired",
    "dependentSchemas",
    "contentEncoding",
    "contentMediaType",
    "contentSchema",
    "deprecated",
    "readOnly",
    "writeOnly",
    "minProperties",
    "maxProperties",
    "$dynamicRef",
    "$dynamicAnchor",
];
const COMBINATORS: &[&str] = &["anyOf", "allOf", "oneOf"];
fn execution(ex: Exec<'_>) -> GraphExecution<'_> {
    GraphExecution::new(ex.heap, ex.runtime)
}
fn combinators(ex: Exec<'_>) -> Result<ValueHandle> {
    ex.heap.module_string_array("ara-ai.schema.normalize", "COMBINATOR_KEYS", COMBINATORS)
}
pub(super) fn prepare(ex: Exec<'_>) -> Result<()> {
    combinators(ex)?;
    ex.heap.module_compiled_function("ara-ai.schema.normalize", "isPureAnyOfNode", &PURE_UNION)?;
    Ok(())
}
fn null_node(ex: Exec<'_>) -> Result<ValueHandle> {
    ex.object([("type", ex.text("null")?)])
}
fn remove(ex: Exec<'_>, target: &ValueHandle, key: &str) -> Result<()> {
    if ex.heap.delete_own(target, &key.into(), ex.runtime)? {
        Ok(())
    } else {
        Err(ValueError::TypeError("Cannot delete strict schema property".into()))
    }
}
fn epoch(ex: Exec<'_>, value: &ValueHandle) -> Result<f64> {
    ex.heap.to_number(value, ex.runtime)
}
fn unrepresentable(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(ex.heap.kind(value)? == ValueKind::Boolean || (schema::object(ex, value)? && schema::empty(ex, value)?))
}
fn check_branch(ex: Exec<'_>, value: &ValueHandle, epoch: f64) -> Result<bool> {
    Ok(unrepresentable(ex, value)? || (schema::object(ex, value)? && has_unrepresentable(ex, value, epoch)?))
}
fn has_unrepresentable(ex: Exec<'_>, source: &ValueHandle, epoch: f64) -> Result<bool> {
    if !schema_stamps::once_with_execution(execution(ex), source, epoch)? {
        return Ok(false);
    }
    let mut has_pattern = false;
    if schema::object(ex, &ex.get(source, "patternProperties")?)? {
        has_pattern = !ex.heap.for_in_string_keys(&ex.get(source, "patternProperties")?, ex.runtime)?.is_empty();
    }
    let additional = ex.get(source, "additionalProperties")?;
    if has_pattern
        || matches!(ex.heap.snapshot(&additional)?, ValueSnapshot::Boolean(true))
        || schema::object(ex, &additional)?
    {
        return Ok(true);
    }
    if schema::object(ex, &ex.get(source, "properties")?)? {
        let props = ex.get(source, "properties")?;
        for name in ex.heap.for_in_string_keys(&props, ex.runtime)? {
            if check_branch(ex, &ex.heap.get(&props, &name.into(), ex.runtime)?, epoch)? {
                return Ok(true);
            }
        }
    }
    if unrepresentable(ex, &ex.get(source, "items")?)? {
        return Ok(true);
    }
    if schema::object(ex, &ex.get(source, "items")?)? {
        if has_unrepresentable(ex, &ex.get(source, "items")?, epoch)? {
            return Ok(true);
        }
    } else if schema::array(ex, &ex.get(source, "items")?)? {
        let mut hit = false;
        ex.for_of(&ex.get(source, "items")?, |value| {
            hit = check_branch(ex, value, epoch)?;
            Ok(!hit)
        })?;
        if hit {
            return Ok(true);
        }
    }
    if schema::array(ex, &ex.get(source, "prefixItems")?)? {
        let mut hit = false;
        ex.for_of(&ex.get(source, "prefixItems")?, |value| {
            hit = check_branch(ex, value, epoch)?;
            Ok(!hit)
        })?;
        if hit {
            return Ok(true);
        }
    }
    let mut hit = false;
    ex.for_of(&combinators(ex)?, |key| {
        let variants = ex.heap.get(source, &ex.string(key)?.into(), ex.runtime)?;
        if schema::array(ex, &variants)? {
            ex.for_of(&variants, |value| {
                hit = check_branch(ex, value, epoch)?;
                Ok(!hit)
            })?;
        }
        Ok(!hit)
    })?;
    if hit {
        return Ok(true);
    }
    ex.for_of(&ex.array([ex.text("$defs")?, ex.text("definitions")?])?, |key| {
        let defs = ex.heap.get(source, &ex.string(key)?.into(), ex.runtime)?;
        if schema::object(ex, &defs)? {
            for name in ex.heap.for_in_string_keys(&defs, ex.runtime)? {
                if check_branch(ex, &ex.heap.get(&defs, &name.into(), ex.runtime)?, epoch)? {
                    hit = true;
                    break;
                }
            }
        }
        Ok(!hit)
    })?;
    Ok(hit)
}
fn resolve_ref(ex: Exec<'_>, root: &ValueHandle, reference: &ValueHandle) -> Result<Option<ValueHandle>> {
    if !ex.heap.truthy(&ex.method(reference, "startsWith", &[ex.text("#/")?])?)? {
        return Ok(None);
    }
    let segments = ex.method(&ex.method(reference, "slice", &[ex.heap.number(2.0)?])?, "split", &[ex.text("/")?])?;
    let mut cursor = root.clone();
    let mut valid = true;
    ex.for_of(&segments, |raw| {
        if !schema::object(ex, &cursor)? {
            valid = false;
            return Ok(false);
        }
        let first = ex.method(raw, "replace", &[ex.heap.regexp("~1".into(), "g")?, ex.text("/")?])?;
        let segment = ex.method(&first, "replace", &[ex.heap.regexp("~0".into(), "g")?, ex.text("~")?])?;
        cursor = ex.heap.get(&cursor, &ex.heap.to_string(&segment, ex.runtime)?.into(), ex.runtime)?;
        Ok(true)
    })?;
    Ok(if valid && schema::object(ex, &cursor)? { Some(cursor) } else { None })
}
fn primitive(ex: Exec<'_>, value: &ValueHandle) -> Result<Option<&'static str>> {
    Ok(match ex.heap.kind(value)? {
        ValueKind::Null => Some("null"),
        ValueKind::String => Some("string"),
        ValueKind::Number => Some("number"),
        ValueKind::Boolean => Some("boolean"),
        _ => None,
    })
}
fn infer(ex: Exec<'_>, node: &ValueHandle) -> Result<Option<&'static str>> {
    let values = if schema::array(ex, &ex.get(node, "enum")?)? {
        ex.get(node, "enum")?
    } else if schema::own(ex, node, "const")? {
        ex.array([ex.get(node, "const")?])?
    } else {
        ex.array([])?
    };
    if matches!(ex.heap.snapshot(&ex.get(&values,"length")?)?,ValueSnapshot::Number(n) if n==0.0) {
        return Ok(None);
    }
    let mut inferred = None;
    let mut valid = true;
    ex.for_of(&values, |value| {
        let Some(ty) = primitive(ex, value)? else {
            valid = false;
            return Ok(false);
        };
        if let Some(current) = inferred {
            if current != ty {
                valid = false;
                return Ok(false);
            }
        } else {
            inferred = Some(ty);
        }
        Ok(true)
    })?;
    Ok(if valid { inferred } else { None })
}
fn equal_json(ex: Exec<'_>, left: &ValueHandle, right: &ValueHandle) -> Result<bool> {
    if ex.heap.object_is(left, right)? {
        return Ok(true);
    }
    if schema::array(ex, left)? || schema::array(ex, right)? {
        if !schema::array(ex, left)?
            || !schema::array(ex, right)?
            || !callbacks::strict_equal(ex.heap, &ex.get(left, "length")?, &ex.get(right, "length")?)?
        {
            return Ok(false);
        }
        let mut index = 0.0;
        while index < ex.heap.to_number(&ex.get(left, "length")?, ex.runtime)? {
            let key = ex.heap.to_string(&ex.heap.number(index)?, ex.runtime)?;
            if !equal_json(
                ex,
                &ex.heap.get(left, &key.clone().into(), ex.runtime)?,
                &ex.heap.get(right, &key.into(), ex.runtime)?,
            )? {
                return Ok(false);
            }
            index += 1.0;
        }
        return Ok(true);
    }
    if !schema::object(ex, left)? || !schema::object(ex, right)? {
        return Ok(false);
    }
    let right_len = ex.heap.for_in_string_keys(right, ex.runtime)?.len();
    let mut left_len = 0usize;
    for key in ex.heap.for_in_string_keys(left, ex.runtime)? {
        left_len += 1;
        if !ex.heap.has(right, &key.clone().into(), ex.runtime)?
            || !equal_json(
                ex,
                &ex.heap.get(left, &key.clone().into(), ex.runtime)?,
                &ex.heap.get(right, &key.into(), ex.runtime)?,
            )?
        {
            return Ok(false);
        }
    }
    Ok(left_len == right_len)
}
fn narrow_enum(ex: Exec<'_>, node: &ValueHandle, ty: &ValueHandle) -> Result<bool> {
    let values = ex.get(node, "enum")?;
    if !schema::array(ex, &values)? {
        return Ok(true);
    }
    let narrowed = ex.apply(&values, "filter", &NARROW_ENUM, &[ty.clone()])?;
    let length = ex.get(&narrowed, "length")?;
    if matches!(ex.heap.snapshot(&length)?,ValueSnapshot::Number(n) if n==0.0) {
        return Ok(false);
    }
    if !callbacks::strict_equal(ex.heap, &length, &ex.get(&values, "length")?)? {
        schema::assign(ex, node, "enum", narrowed)?;
    }
    Ok(true)
}
fn sanitize(
    ex: Exec<'_>,
    source: &ValueHandle,
    epoch_value: f64,
    cache: &ValueHandle,
    root: &ValueHandle,
) -> Result<ValueHandle> {
    if let Some(hit) = schema::cached(ex, cache, source)? {
        if ex.heap.truthy(&hit)? {
            return Ok(hit);
        }
    }
    if !schema_stamps::once_with_execution(execution(ex), source, epoch_value)? {
        return ex.heap.object();
    }
    if ex.heap.kind(&ex.get(source, "$ref")?)? == ValueKind::String {
        let mut sibling = false;
        for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
            if !key.equals_ascii("$ref") && schema::own(ex, source, key)? {
                sibling = true;
                break;
            }
        }
        if sibling {
            if let Some(resolved) = resolve_ref(ex, root, &ex.get(source, "$ref")?)? {
                let merged = ex.heap.spread(&resolved, ex.runtime)?;
                for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
                    if key.equals_ascii("$ref") || !schema::own(ex, source, key.clone())? {
                        continue;
                    }
                    let value = ex.heap.get(source, &key.clone().into(), ex.runtime)?;
                    schema::assign(ex, &merged, key, value)?;
                }
                let result = sanitize(ex, &merged, epoch_value, cache, root)?;
                schema::remember(ex, cache, source, result.clone())?;
                return Ok(result);
            }
        }
    }
    let all = ex.get(source, "allOf")?;
    if schema::array(ex, &all)?
        && matches!(ex.heap.snapshot(&ex.get(&all,"length")?)?,ValueSnapshot::Number(n) if n==1.0)
        && schema::object(ex, &ex.get(&all, "0")?)?
    {
        let merged = ex.heap.spread(source, ex.runtime)?;
        remove(ex, &merged, "allOf")?;
        let sole = ex.get(&all, "0")?;
        for key in ex.heap.for_in_string_keys(&sole, ex.runtime)? {
            if schema::own(ex, &sole, key.clone())? {
                let value = ex.heap.get(&sole, &key.clone().into(), ex.runtime)?;
                schema::assign(ex, &merged, key, value)?;
            }
        }
        let result = sanitize(ex, &merged, epoch_value, cache, root)?;
        schema::remember(ex, cache, source, result.clone())?;
        return Ok(result);
    }
    let ty = ex.get(source, "type")?;
    if schema::array(ex, &ty)? {
        let types = ex.apply(&ty, "filter", &STRING_TYPE, &[])?;
        let without = ex.heap.spread(source, ex.runtime)?;
        remove(ex, &without, "type")?;
        let sanitized = sanitize(ex, &without, epoch_value, cache, root)?;
        if matches!(ex.heap.snapshot(&ex.get(&types,"length")?)?,ValueSnapshot::Number(n) if n==0.0) {
            schema::remember(ex, cache, source, sanitized.clone())?;
            return Ok(sanitized);
        }
        let description = ex.get(&sanitized, "description")?;
        let base = ex.heap.spread_excluding(&sanitized, &["description".into()], ex.runtime)?;
        let variants = ex.heap.array_with_length(0)?;
        ex.for_of(&types, |variant_type| {
            let variant = ex.heap.spread(&base, ex.runtime)?;
            ex.own(&variant, "type", variant_type.clone())?;
            if !matches!(ex.heap.snapshot(variant_type)?,ValueSnapshot::String(text) if text.equals_ascii("object")) {
                for key in ["properties", "required", "additionalProperties"] {
                    remove(ex, &variant, key)?;
                }
            }
            if !matches!(ex.heap.snapshot(variant_type)?,ValueSnapshot::String(text) if text.equals_ascii("array")) {
                remove(ex, &variant, "items")?;
            }
            if narrow_enum(ex, &variant, variant_type)? {
                let push = ex.get(&variants, "push")?;
                let item = sanitize(ex, &variant, epoch_value, cache, root)?;
                ex.heap.call(&push, &variants, &[item], ex.runtime)?;
            }
            Ok(true)
        })?;
        let length = ex.get(&variants, "length")?;
        let result = if matches!(ex.heap.snapshot(&length)?,ValueSnapshot::Number(n) if n==0.0) {
            sanitized
        } else if matches!(ex.heap.snapshot(&length)?,ValueSnapshot::Number(n) if n==1.0) {
            let sole = ex.get(&variants, "0")?;
            if !schema::undefined(ex, &description)? && !schema::own(ex, &sole, "description")? {
                schema::assign(ex, &sole, "description", description)?;
            }
            sole
        } else {
            let result = ex.object([("anyOf", variants)])?;
            if !schema::undefined(ex, &description)? {
                schema::assign(ex, &result, "description", description)?;
            }
            result
        };
        schema::remember(ex, cache, source, result.clone())?;
        return Ok(result);
    }
    let sanitized = ex.heap.object()?;
    schema::remember(ex, cache, source, sanitized.clone())?;
    for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
        let value = ex.heap.get(source, &key.clone().into(), ex.runtime)?;
        if schema::table_has(ex, &key, NON_STRUCTURAL)?
            || ["type", "const", "nullable"].iter().any(|known| key.equals_ascii(known))
        {
            continue;
        }
        if (key.equals_ascii("properties") || key.equals_ascii("$defs") || key.equals_ascii("definitions"))
            && schema::object(ex, &value)?
        {
            let map = ex.heap.object()?;
            for name in ex.heap.for_in_string_keys(&value, ex.runtime)? {
                let member = ex.heap.get(&value, &name.clone().into(), ex.runtime)?;
                let next = if schema::object(ex, &member)? {
                    sanitize(ex, &member, epoch_value, cache, root)?
                } else {
                    member
                };
                schema::assign(ex, &map, name, next)?;
            }
            schema::assign(ex, &sanitized, key, map)?;
            continue;
        }
        if key.equals_ascii("items") && schema::object(ex, &value)? {
            schema::assign(ex, &sanitized, key, sanitize(ex, &value, epoch_value, cache, root)?)?;
            continue;
        }
        let comb = ex.heap.truthy(&ex.method(&combinators(ex)?, "includes", &[ex.text(key.clone())?])?)?;
        if (key.equals_ascii("items") || key.equals_ascii("prefixItems") || comb) && schema::array(ex, &value)? {
            let mapped =
                ex.apply(&value, "map", &SANITIZE_ITEM, &[ex.heap.number(epoch_value)?, cache.clone(), root.clone()])?;
            schema::assign(ex, &sanitized, key, mapped)?;
            continue;
        }
        if key.equals_ascii("additionalProperties") {
            continue;
        }
        if key.equals_ascii("description")
            && ex.heap.kind(&value)? == ValueKind::String
            && !schema::undefined(ex, &ex.get(source, "default")?)?
        {
            let default = ex.get(source, "default")?;
            let formatted = if ex.heap.kind(&default)? == ValueKind::String {
                default
            } else {
                match ex.heap.stringify(&default, StringifyMode::Plain, None, ex.runtime)? {
                    Some(text) => ex.text(text)?,
                    None => ex.heap.undefined()?,
                }
            };
            let description = if ex.heap.truthy(&ex.method(&value, "includes", &[ex.text("(default:")?])?)? {
                value
            } else {
                let text = crate::actual_provider::concat(
                    &crate::actual_provider::concat(
                        &crate::actual_provider::concat(&ex.heap.to_string(&value, ex.runtime)?, &" (default: ".into()),
                        &ex.heap.to_string(&formatted, ex.runtime)?,
                    ),
                    &")".into(),
                );
                ex.text(text)?
            };
            schema::assign(ex, &sanitized, key, description)?;
            continue;
        }
        schema::assign(ex, &sanitized, key, value)?;
    }
    if schema::own(ex, source, "const")? {
        let constant = ex.get(source, "const")?;
        let existing =
            if schema::array(ex, &ex.get(&sanitized, "enum")?)? { ex.get(&sanitized, "enum")? } else { ex.array([])? };
        if !ex.heap.truthy(&ex.apply(&existing, "some", &ENUM_EQUAL, &[constant.clone()])?)? {
            ex.method(&existing, "push", &[constant])?;
        }
        schema::assign(ex, &sanitized, "enum", existing)?;
    }
    if ex.heap.kind(&ty)? == ValueKind::String {
        schema::assign(ex, &sanitized, "type", ty)?;
    }
    if schema::undefined(ex, &ex.get(&sanitized, "type")?)? && schema::object(ex, &ex.get(&sanitized, "properties")?)? {
        schema::assign(ex, &sanitized, "type", ex.text("object")?)?;
    }
    if schema::undefined(ex, &ex.get(&sanitized, "type")?)?
        && (!schema::undefined(ex, &ex.get(&sanitized, "items")?)?
            || !schema::undefined(ex, &ex.get(&sanitized, "prefixItems")?)?)
    {
        schema::assign(ex, &sanitized, "type", ex.text("array")?)?;
    }
    if schema::undefined(ex, &ex.get(&sanitized, "type")?)? {
        if let Some(inferred) = infer(ex, &sanitized)? {
            schema::assign(ex, &sanitized, "type", ex.text(inferred)?)?;
        }
    }
    if matches!(ex.heap.snapshot(&ex.get(source, "nullable")?)?, ValueSnapshot::Boolean(true)) {
        let description = ex.get(&sanitized, "description")?;
        let without = ex.heap.spread_excluding(&sanitized, &["nullable".into(), "description".into()], ex.runtime)?;
        let wrapper = ex.object([("anyOf", ex.array([without, null_node(ex)?])?)])?;
        if !schema::undefined(ex, &description)? {
            schema::assign(ex, &wrapper, "description", description)?;
        }
        return Ok(wrapper);
    }
    Ok(sanitized)
}
fn pure_union(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    if !schema::object(ex, value)? || !schema::array(ex, &ex.get(value, "anyOf")?)? {
        return Ok(false);
    }
    Ok(ex
        .heap
        .for_in_string_keys(value, ex.runtime)?
        .into_iter()
        .all(|key| key.equals_ascii("anyOf") || key.equals_ascii("description")))
}
fn enter(ex: Exec<'_>, source: &ValueHandle) -> Result<bool> {
    schema_stamps::enter_with_execution(execution(ex), source)
}
fn exit(ex: Exec<'_>, source: &ValueHandle) -> Result<()> {
    schema_stamps::exit_with_execution(execution(ex), source)
}
fn enforce(ex: Exec<'_>, source: &ValueHandle, cache: &ValueHandle) -> Result<ValueHandle> {
    if !enter(ex, source)? {
        return Err(ValueError::TypeError(
            "Schema contains a circular object graph — cannot enforce strict mode".into(),
        ));
    }
    let result = (|| {
        if let Some(hit) = schema::cached(ex, cache, source)? {
            if ex.heap.truthy(&hit)? {
                return Ok(hit);
            }
        }
        let output = ex.heap.spread(source, ex.runtime)?;
        schema::remember(ex, cache, source, output.clone())?;
        enforce_body(ex, &output, cache)
    })();
    exit(ex, source)?;
    result
}
fn enforce_body(ex: Exec<'_>, output: &ValueHandle, cache: &ValueHandle) -> Result<ValueHandle> {
    if ex.eq(output, "type", "object")? {
        schema::assign(ex, output, "additionalProperties", ex.heap.boolean(false)?)?;
        let props = ex.get(output, "properties")?;
        let props = if schema::object(ex, &props)? { props } else { ex.heap.object()? };
        let required = if schema::array(ex, &ex.get(output, "required")?)? {
            ex.apply(&ex.get(output, "required")?, "filter", &STRING_TYPE, &[])?
        } else {
            ex.array([])?
        };
        let original_required = ex.heap.new_set(&required, ex.runtime)?;
        let strict_props = ex.heap.object()?;
        for name in ex.heap.for_in_string_keys(&props, ex.runtime)? {
            let value = ex.heap.get(&props, &name.clone().into(), ex.runtime)?;
            let processed = if schema::object(ex, &value)? { enforce(ex, &value, cache)? } else { value };
            let nullable = if !ex.heap.truthy(&ex.method(&original_required, "has", &[ex.text(name.clone())?])?)? {
                if schema::object(ex, &processed)?
                    && schema::array(ex, &ex.get(&processed, "anyOf")?)?
                    && ex.heap.truthy(&ex.apply(&ex.get(&processed, "anyOf")?, "some", &NULL_BRANCH, &[])?)?
                {
                    processed
                } else if pure_union(ex, &processed)? {
                    let next = ex.heap.spread(&processed, ex.runtime)?;
                    let mut variants = Vec::new();
                    ex.for_of(&ex.get(&processed, "anyOf")?, |item| {
                        variants.push(item.clone());
                        Ok(true)
                    })?;
                    variants.push(null_node(ex)?);
                    ex.own(&next, "anyOf", ex.array(variants)?)?;
                    next
                } else if schema::object(ex, &processed)?
                    && ex.heap.kind(&ex.get(&processed, "description")?)? == ValueKind::String
                {
                    let description = ex.get(&processed, "description")?;
                    let without = ex.heap.spread_excluding(&processed, &["description".into()], ex.runtime)?;
                    ex.object([("anyOf", ex.array([without, null_node(ex)?])?), ("description", description)])?
                } else {
                    ex.object([("anyOf", ex.array([processed, null_node(ex)?])?)])?
                }
            } else {
                processed
            };
            schema::assign(ex, &strict_props, name, nullable)?;
        }
        schema::assign(ex, output, "properties", strict_props.clone())?;
        schema::assign(ex, output, "required", schema::keys(ex, &strict_props)?)?;
    }
    let items = ex.get(output, "items")?;
    if !ex.nullish(&items)? && matches!(ex.heap.kind(&ex.get(output, "items")?)?, ValueKind::Object | ValueKind::Array)
    {
        let next = if schema::array(ex, &ex.get(output, "items")?)? {
            ex.apply(&ex.get(output, "items")?, "map", &ENFORCE_ITEM, &[cache.clone()])?
        } else {
            enforce(ex, &ex.get(output, "items")?, cache)?
        };
        schema::assign(ex, output, "items", next)?;
    }
    if schema::array(ex, &ex.get(output, "prefixItems")?)? {
        let next = ex.apply(&ex.get(output, "prefixItems")?, "map", &ENFORCE_ITEM, &[cache.clone()])?;
        schema::assign(ex, output, "prefixItems", next)?;
    }
    ex.for_of(&combinators(ex)?, |key| {
        let name = ex.string(key)?;
        if schema::array(ex, &ex.heap.get(output, &name.clone().into(), ex.runtime)?)? {
            let next = ex.apply(
                &ex.heap.get(output, &name.clone().into(), ex.runtime)?,
                "map",
                &ENFORCE_ITEM,
                &[cache.clone()],
            )?;
            schema::assign(ex, output, name, next)?;
        }
        Ok(true)
    })?;
    if schema::array(ex, &ex.get(output, "anyOf")?)?
        && ex.heap.truthy(&ex.apply_module(
            &ex.get(output, "anyOf")?,
            "some",
            "ara-ai.schema.normalize",
            "isPureAnyOfNode",
            &PURE_UNION,
        )?)?
    {
        let flattened = ex.heap.array_with_length(0)?;
        ex.for_of(&ex.get(output, "anyOf")?, |branch| {
            if !pure_union(ex, branch)? {
                ex.method(&flattened, "push", &[branch.clone()])?;
                return Ok(true);
            }
            let push = ex.get(&flattened, "push")?;
            let mut arguments = Vec::new();
            ex.for_of(&ex.get(branch, "anyOf")?, |item| {
                arguments.push(item.clone());
                Ok(true)
            })?;
            ex.heap.call(&push, &flattened, &arguments, ex.runtime)?;
            if ex.heap.kind(&ex.get(branch, "description")?)? == ValueKind::String
                && schema::undefined(ex, &ex.get(output, "description")?)?
            {
                schema::assign(ex, output, "description", ex.get(branch, "description")?)?;
            }
            Ok(true)
        })?;
        schema::assign(ex, output, "anyOf", flattened)?;
    }
    ex.for_of(&ex.array([ex.text("$defs")?, ex.text("definitions")?])?, |key| {
        let name = ex.string(key)?;
        if schema::object(ex, &ex.heap.get(output, &name.clone().into(), ex.runtime)?)? {
            let defs = ex.heap.get(output, &name.clone().into(), ex.runtime)?;
            let next = ex.heap.object()?;
            for key in ex.heap.for_in_string_keys(&defs, ex.runtime)? {
                let value = ex.heap.get(&defs, &key.clone().into(), ex.runtime)?;
                let value = if schema::object(ex, &value)? { enforce(ex, &value, cache)? } else { value };
                schema::assign(ex, &next, key, value)?;
            }
            schema::assign(ex, output, name, next)?;
        }
        Ok(true)
    })?;
    if schema::undefined(ex, &ex.get(output, "type")?)? {
        if let Some(inferred) = infer(ex, output)? {
            schema::assign(ex, output, "type", ex.text(inferred)?)?;
        }
    }
    if schema::undefined(ex, &ex.get(output, "type")?)?
        && schema::undefined(ex, &ex.get(output, "$ref")?)?
        && !ex.heap.truthy(&ex.apply(&combinators(ex)?, "some", &HAS_COMBINATOR, &[output.clone()])?)?
        && !schema::object(ex, &ex.get(output, "not")?)?
    {
        return Err(ValueError::TypeError(
            "Schema node has no type, combinator, or $ref — cannot enforce strict mode".into(),
        ));
    }
    Ok(output.clone())
}
pub(super) fn adapt(ex: Exec<'_>, source: &ValueHandle, strict: bool) -> Result<(ValueHandle, bool)> {
    let upgraded = crate::schema_draft::upgrade_graph_json_schema_to_202012_with_execution(execution(ex), source)?;
    if !strict {
        return Ok((upgraded, false));
    }
    let key = ex.heap.module_symbol("ara-ai.schema.normalize", "pi.schema.strict")?;
    let result = schema_stamps::stamp_with_execution(execution(ex), &upgraded, &key, |source| {
        let again = crate::schema_draft::upgrade_graph_json_schema_to_202012_with_execution(execution(ex), source)?;
        if has_unrepresentable(ex, &again, schema_stamps::epoch_next())? {
            return ex.object([("schema", again), ("strict", ex.heap.boolean(false)?)]);
        }
        let attempt = (|| {
            let sanitized = sanitize(ex, &again, schema_stamps::epoch_next(), &schema::memo(ex)?, &again)?;
            enforce(ex, &sanitized, &schema::memo(ex)?)
        })();
        match attempt {
            Ok(schema) => ex.object([("schema", schema), ("strict", ex.heap.boolean(true)?)]),
            Err(_) => ex.object([("schema", again), ("strict", ex.heap.boolean(false)?)]),
        }
    })?;
    Ok((ex.get(&result, "schema")?, ex.heap.truthy(&ex.get(&result, "strict")?)?))
}
fn string_type(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(heap.kind(&callbacks::first(heap, args)?)? == ValueKind::String)
}
fn narrow_enum_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(schema::matches_type(Exec { heap, runtime }, &callbacks::first(heap, args)?, &captures[0])?)
}
fn enum_equal(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(equal_json(Exec { heap, runtime }, &callbacks::first(heap, args)?, &captures[0])?)
}
fn sanitize_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    if schema::object(ex, &item)? {
        sanitize(ex, &item, epoch(ex, &captures[0])?, &captures[1], &captures[2])
    } else {
        Ok(item)
    }
}
fn enforce_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    if schema::object(ex, &item)? { enforce(ex, &item, &captures[0]) } else { Ok(item) }
}
fn null_branch(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    heap.boolean(schema::object(ex, &item)? && ex.eq(&item, "type", "null")?)
}
fn pure_union_item(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(pure_union(Exec { heap, runtime }, &callbacks::first(heap, args)?)?)
}
fn has_combinator(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let key = ex.string(&callbacks::first(heap, args)?)?;
    heap.boolean(schema::array(ex, &heap.get(&captures[0], &key.into(), runtime)?)?)
}
macro_rules! spec {
    ($name:ident,$body:ident,$captures:literal,$source:literal) => {
        static $name: CompiledFunctionSpec = CompiledFunctionSpec {
            source: $source,
            name: "",
            length: 1,
            form: CompiledFunctionForm::Arrow,
            capture_count: $captures,
            body: $body,
        };
    };
}
spec!(STRING_TYPE, string_type, 0, "normalize.ts:2070/2313");
spec!(NARROW_ENUM, narrow_enum_item, 1, "normalize.ts:1847");
spec!(ENUM_EQUAL, enum_equal, 1, "normalize.ts:2212");
spec!(SANITIZE_ITEM, sanitize_item, 3, "normalize.ts:2155/2165/2174");
spec!(ENFORCE_ITEM, enforce_item, 1, "normalize.ts:2355/2365/2374");
spec!(NULL_BRANCH, null_branch, 0, "normalize.ts:2329");
static PURE_UNION: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "normalize.ts:2258",
    name: "isPureAnyOfNode",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 0,
    body: pure_union_item,
};
spec!(HAS_COMBINATOR, has_combinator, 1, "normalize.ts:2428");
