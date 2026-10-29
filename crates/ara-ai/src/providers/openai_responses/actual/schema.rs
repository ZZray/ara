//! Responses sanitizer and emitted-tool quarantine, fixed OMP596f2da (MIT).
//! Only schema-valued positions recurse. Instance enum/default data stays raw.
use super::callbacks::{self, Exec};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, PropertyKey, ValueError, ValueHandle,
        ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;
const MODULE: &str = "ara-ai.schema.responses";
const MAP_KEYS: &[&str] =
    &["properties", "patternProperties", "dependencies", "dependentSchemas", "$defs", "definitions"];
const ARRAY_KEYS: &[&str] = &["anyOf", "oneOf", "allOf", "prefixItems"];
const VALUE_KEYS: &[&str] = &[
    "items",
    "additionalItems",
    "contains",
    "contentSchema",
    "propertyNames",
    "if",
    "then",
    "else",
    "not",
    "additionalProperties",
    "unevaluatedItems",
    "unevaluatedProperties",
];
pub(super) fn prepare(ex: Exec<'_>) -> Result<()> {
    ex.heap.prepare_module_string_sets(
        MODULE,
        &[
            ("OPENAI_RESPONSES_SCHEMA_MAP_KEYS", MAP_KEYS),
            ("OPENAI_RESPONSES_SCHEMA_ARRAY_KEYS", ARRAY_KEYS),
            ("OPENAI_RESPONSES_SCHEMA_VALUE_KEYS", VALUE_KEYS),
            ("OPENAI_UNSUPPORTED_REGEX_LOOKAROUNDS", &["=", "!", "<=", "<!"]),
        ],
        ex.runtime,
    )
}
pub(super) fn object(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(ex.heap.truthy(value)? && ex.heap.kind(value)? == ValueKind::Object)
}
pub(super) fn array(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(ex.heap.kind(value)? == ValueKind::Array)
}
pub(super) fn undefined(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    Ok(ex.heap.kind(value)? == ValueKind::Undefined)
}
pub(super) fn assign(
    ex: Exec<'_>,
    target: &ValueHandle,
    key: impl Into<PropertyKey>,
    value: ValueHandle,
) -> Result<()> {
    ex.heap.assign(target, key.into(), value, ex.runtime)
}
pub(super) fn keys(ex: Exec<'_>, value: &ValueHandle) -> Result<ValueHandle> {
    ex.array(
        ex.heap
            .own_enumerable_string_keys(value, ex.runtime)?
            .into_iter()
            .map(|text| ex.text(text))
            .collect::<Result<Vec<_>>>()?,
    )
}
pub(super) fn empty(ex: Exec<'_>, value: &ValueHandle) -> Result<bool> {
    let names = keys(ex, value)?;
    Ok(matches!(ex.heap.snapshot(&ex.get(&names,"length")?)?,ValueSnapshot::Number(length) if length==0.0))
}
pub(super) fn own(ex: Exec<'_>, value: &ValueHandle, key: impl Into<PropertyKey>) -> Result<bool> {
    ex.heap.has_own(value, &key.into(), ex.runtime)
}
pub(super) fn memo(ex: Exec<'_>) -> Result<ValueHandle> {
    ex.heap.object_with_prototype(None)
}
pub(super) fn cached(ex: Exec<'_>, cache: &ValueHandle, source: &ValueHandle) -> Result<Option<ValueHandle>> {
    ex.heap.adopt(source)?;
    match ex.heap.own_descriptor(cache, &PropertyKey::String(format!("{:?}", source.node_id()).into()), ex.runtime)? {
        Some(PropertyDescriptor::Data { value, .. }) => Ok(Some(value)),
        None => Ok(None),
        _ => Err(ValueError::WrongRuntimeReply("Private schema memo slot")),
    }
}
pub(super) fn remember(ex: Exec<'_>, cache: &ValueHandle, source: &ValueHandle, result: ValueHandle) -> Result<()> {
    ex.heap.define_own_data(cache, PropertyKey::String(format!("{:?}", source.node_id()).into()), result, ex.runtime)
}
pub(super) fn table_has(ex: Exec<'_>, key: &WireString, known: &[&str]) -> Result<bool> {
    if known.iter().any(|name| key.equals_ascii(name)) {
        Ok(true)
    } else {
        ex.heap.has(&ex.heap.object_prototype(), &key.clone().into(), ex.runtime)
    }
}
fn set_has(ex: Exec<'_>, name: &'static str, values: &'static [&'static str], key: &ValueHandle) -> Result<bool> {
    let set = ex.heap.module_string_set(MODULE, name, values)?;
    Ok(ex.heap.truthy(&ex.method(&set, "has", &[key.clone()])?)?)
}
fn lookaround(ex: Exec<'_>, pattern: &ValueHandle) -> Result<bool> {
    let mut group = ex.method(pattern, "indexOf", &[ex.text("(?")?])?;
    loop {
        let position = ex.heap.to_number(&group, ex.runtime)?;
        if position == -1.0 {
            return Ok(false);
        }
        if !position.is_finite() || position < 0.0 {
            return Err(ValueError::UnsupportedOperation(
                "Native indexOf returned an invalid schema lookaround position",
            ));
        }
        let mut escapes = 0usize;
        let mut cursor = position - 1.0;
        while cursor >= 0.0 {
            let unit =
                ex.heap.get(pattern, &ex.heap.to_string(&ex.heap.number(cursor)?, ex.runtime)?.into(), ex.runtime)?;
            if !matches!(ex.heap.snapshot(&unit)?,ValueSnapshot::String(text) if text.equals_ascii("\\")) {
                break;
            }
            escapes += 1;
            cursor -= 1.0;
        }
        if escapes % 2 == 0 {
            let unit = ex.heap.get(
                pattern,
                &ex.heap.to_string(&ex.heap.number(position + 2.0)?, ex.runtime)?.into(),
                ex.runtime,
            )?;
            let operator = if matches!(ex.heap.snapshot(&unit)?,ValueSnapshot::String(text) if text.equals_ascii("<")) {
                ex.method(pattern, "slice", &[ex.heap.number(position + 2.0)?, ex.heap.number(position + 4.0)?])?
            } else {
                unit
            };
            if set_has(ex, "OPENAI_UNSUPPORTED_REGEX_LOOKAROUNDS", &["=", "!", "<=", "<!"], &operator)? {
                return Ok(true);
            }
        }
        group = ex.method(pattern, "indexOf", &[ex.text("(?")?, ex.heap.number(position + 2.0)?])?;
    }
}
pub(super) fn sanitize(ex: Exec<'_>, source: &ValueHandle) -> Result<ValueHandle> {
    normalize(ex, source, &memo(ex)?)
}
fn normalize(ex: Exec<'_>, source: &ValueHandle, cache: &ValueHandle) -> Result<ValueHandle> {
    if !object(ex, source)? {
        return Ok(source.clone());
    }
    if empty(ex, source)? {
        return ex.heap.boolean(true);
    }
    if let Some(hit) = cached(ex, cache, source)? {
        if ex.heap.truthy(&hit)? {
            return Ok(hit);
        }
    }
    let output = ex.heap.object()?;
    remember(ex, cache, source, output.clone())?;
    let mut changed = false;
    for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
        if !own(ex, source, key.clone())? {
            continue;
        }
        if key.equals_ascii("oneOf") && array(ex, &ex.get(source, "oneOf")?)? {
            changed = true;
            continue;
        }
        if key.equals_ascii("pattern")
            && ex.heap.kind(&ex.get(source, "pattern")?)? == ValueKind::String
            && lookaround(ex, &ex.get(source, "pattern")?)?
        {
            changed = true;
            continue;
        }
        let child = ex.heap.get(source, &key.clone().into(), ex.runtime)?;
        let key_value = ex.text(key.clone())?;
        let next = if key.equals_ascii("patternProperties") && object(ex, &child)? {
            normalize_map(ex, &child, cache, true)?
        } else if set_has(ex, "OPENAI_RESPONSES_SCHEMA_MAP_KEYS", MAP_KEYS, &key_value)? && object(ex, &child)? {
            normalize_map(ex, &child, cache, false)?
        } else if set_has(ex, "OPENAI_RESPONSES_SCHEMA_ARRAY_KEYS", ARRAY_KEYS, &key_value)? && array(ex, &child)? {
            normalize_array(ex, &child, cache)?
        } else if set_has(ex, "OPENAI_RESPONSES_SCHEMA_VALUE_KEYS", VALUE_KEYS, &key_value)? && object(ex, &child)? {
            normalize(ex, &child, cache)?
        } else {
            child.clone()
        };
        if !callbacks::strict_equal(ex.heap, &next, &child)? {
            changed = true;
        }
        assign(ex, &output, key, next)?;
    }
    if array(ex, &ex.get(source, "oneOf")?)? {
        let rewritten = normalize_array(ex, &ex.get(source, "oneOf")?, cache)?;
        let existing = ex.get(&output, "anyOf")?;
        let union = if array(ex, &existing)? {
            let mut items = Vec::new();
            ex.for_of(&existing, |item| {
                items.push(item.clone());
                Ok(true)
            })?;
            ex.for_of(&rewritten, |item| {
                items.push(item.clone());
                Ok(true)
            })?;
            ex.array(items)?
        } else {
            rewritten
        };
        assign(ex, &output, "anyOf", union)?;
    }
    let declared = ex.get(source, "type")?;
    let mut is_object =
        matches!(ex.heap.snapshot(&declared)?,ValueSnapshot::String(text) if text.equals_ascii("object"));
    if !is_object && array(ex, &declared)? {
        ex.for_of(&declared, |variant| {
            if matches!(ex.heap.snapshot(variant)?,ValueSnapshot::String(text) if text.equals_ascii("object")) {
                is_object = true;
                return Ok(false);
            }
            Ok(true)
        })?;
    }
    if is_object && !own(ex, source, "properties")? {
        assign(ex, &output, "properties", ex.heap.object()?)?;
        changed = true;
    }
    let result =
        if changed { if empty(ex, &output)? { ex.heap.boolean(true)? } else { output } } else { source.clone() };
    remember(ex, cache, source, result.clone())?;
    Ok(result)
}
fn normalize_array(ex: Exec<'_>, source: &ValueHandle, cache: &ValueHandle) -> Result<ValueHandle> {
    let state = ex.object([("changed", ex.heap.boolean(false)?)])?;
    let output = ex.apply(source, "map", &NORMALIZE_ITEM, &[cache.clone(), state.clone()])?;
    if ex.heap.truthy(&ex.get(&state, "changed")?)? { Ok(output) } else { Ok(source.clone()) }
}
fn normalize_map(ex: Exec<'_>, source: &ValueHandle, cache: &ValueHandle, strip: bool) -> Result<ValueHandle> {
    let output = ex.heap.object()?;
    let mut changed = false;
    for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
        if !own(ex, source, key.clone())? {
            continue;
        }
        let child = ex.heap.get(source, &key.clone().into(), ex.runtime)?;
        let next = normalize(ex, &child, cache)?;
        if !callbacks::strict_equal(ex.heap, &next, &child)? {
            changed = true;
        }
        if strip && lookaround(ex, &ex.text(key.clone())?)? {
            changed = true;
            let existing = ex.get(&output, ".*")?;
            if undefined(ex, &existing)? {
                assign(ex, &output, ".*", next)?;
            } else if object(ex, &existing)?
                && array(ex, &ex.get(&existing, "anyOf")?)?
                && matches!(ex.heap.snapshot(&ex.get(&keys(ex,&existing)?,"length")?)?,ValueSnapshot::Number(n) if n==1.0)
            {
                let mut items = Vec::new();
                ex.for_of(&ex.get(&existing, "anyOf")?, |item| {
                    items.push(item.clone());
                    Ok(true)
                })?;
                items.push(next);
                assign(ex, &existing, "anyOf", ex.array(items)?)?;
            } else {
                assign(ex, &output, ".*", ex.object([("anyOf", ex.array([existing, next])?)])?)?;
            }
            continue;
        }
        assign(ex, &output, key, next)?;
    }
    Ok(if changed { output } else { source.clone() })
}
fn normalize_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    let next = normalize(ex, &item, &captures[0])?;
    if !callbacks::strict_equal(heap, &next, &item)? {
        assign(ex, &captures[1], "changed", heap.boolean(true)?)?;
    }
    Ok(next)
}

const TYPE_NAMES: &[&str] = &["string", "number", "integer", "boolean", "object", "array", "null"];
pub(super) fn matches_type(ex: Exec<'_>, value: &ValueHandle, kind: &ValueHandle) -> Result<bool> {
    let ValueSnapshot::String(text) = ex.heap.snapshot(kind)? else { return Ok(true) };
    let ty = ex.heap.kind(value)?;
    Ok(if text.equals_ascii("string") {
        ty == ValueKind::String
    } else if text.equals_ascii("number") {
        ty == ValueKind::Number
    } else if text.equals_ascii("integer") {
        matches!(ex.heap.snapshot(value)?,ValueSnapshot::Number(number) if number.is_finite()&&number.fract()==0.0)
    } else if text.equals_ascii("boolean") {
        ty == ValueKind::Boolean
    } else if text.equals_ascii("null") {
        ty == ValueKind::Null
    } else if text.equals_ascii("object") {
        ty == ValueKind::Object
    } else if text.equals_ascii("array") {
        ty == ValueKind::Array
    } else {
        true
    })
}
fn declared(ex: Exec<'_>, node: &ValueHandle) -> Result<ValueHandle> {
    let ty = ex.get(node, "type")?;
    if let ValueSnapshot::String(text) = ex.heap.snapshot(&ty)? {
        return ex.array(if table_has(ex, &text, TYPE_NAMES)? { vec![ty] } else { Vec::new() });
    }
    if array(ex, &ty)? {
        return ex.apply(&ty, "filter", &KNOWN_TYPE, &[]);
    }
    ex.array([])
}
pub(super) fn violation(
    ex: Exec<'_>,
    schema: &ValueHandle,
    path: &WireString,
    reject_root: bool,
) -> Result<Option<WireString>> {
    if array(ex, schema)? {
        let mut index = 0.0;
        while index < ex.heap.to_number(&ex.get(schema, "length")?, ex.runtime)? {
            let key = ex.heap.to_string(&ex.heap.number(index)?, ex.runtime)?;
            let child = ex.heap.get(schema, &key.clone().into(), ex.runtime)?;
            let next = append_path(path, &key);
            if let Some(hit) = violation(ex, &child, &next, reject_root)? {
                return Ok(Some(hit));
            }
            index += 1.0;
        }
        return Ok(None);
    }
    if ex.heap.kind(schema)? != ValueKind::Object {
        return Ok(None);
    }
    let types = declared(ex, schema)?;
    if ex.heap.to_number(&ex.get(&types, "length")?, ex.runtime)? > 0.0 {
        if array(ex, &ex.get(schema, "enum")?)?
            && ex.heap.truthy(&ex.apply(&ex.get(schema, "enum")?, "some", &ENUM_VIOLATION, &[types.clone()])?)?
        {
            return Ok(Some(append_path(path, &"enum".into())));
        }
        if ex.heap.has(schema, &"const".into(), ex.runtime)?
            && !ex.heap.truthy(&ex.apply(&types, "some", &CONST_MATCH_TYPE, &[schema.clone()])?)?
        {
            return Ok(Some(append_path(path, &"const".into())));
        }
    }
    let typed_object = if reject_root && path.equals_ascii("#") {
        ex.heap.truthy(&ex.method(&types, "includes", &[ex.text("object")?])?)?
            || (!undefined(ex, &ex.get(schema, "properties")?)?
                && matches!(
                    ex.heap.kind(&ex.get(schema, "properties")?)?,
                    ValueKind::Object | ValueKind::Array | ValueKind::Null
                ))
    } else {
        false
    };
    if typed_object {
        for key in ["anyOf", "oneOf"] {
            let arr = ex.get(schema, key)?;
            if !array(ex, &arr)?
                || matches!(ex.heap.snapshot(&ex.get(&arr,"length")?)?,ValueSnapshot::Number(n) if n==0.0)
            {
                continue;
            }
            if ex.heap.truthy(&ex.apply(&arr, "some", &NON_OBJECT_BRANCH, &[])?)? {
                return Ok(Some(append_path(path, &key.into())));
            }
        }
    }
    for key in ["properties", "patternProperties", "$defs", "definitions", "dependentSchemas"] {
        let sub = ex.get(schema, key)?;
        if object(ex, &sub)? {
            let names = keys(ex, &sub)?;
            let mut hit = None;
            ex.for_of(&names, |name| {
                let name = ex.string(name)?;
                let child = ex.heap.get(&sub, &name.clone().into(), ex.runtime)?;
                hit = violation(ex, &child, &append_path(&append_path(path, &key.into()), &name), reject_root)?;
                Ok(hit.is_none())
            })?;
            if hit.is_some() {
                return Ok(hit);
            }
        }
    }
    for key in [
        "items",
        "contains",
        "not",
        "if",
        "then",
        "else",
        "propertyNames",
        "additionalProperties",
        "unevaluatedProperties",
        "unevaluatedItems",
    ] {
        if ex.heap.has(schema, &key.into(), ex.runtime)? {
            if let Some(hit) = violation(ex, &ex.get(schema, key)?, &append_path(path, &key.into()), reject_root)? {
                return Ok(Some(hit));
            }
        }
    }
    for key in ["anyOf", "oneOf", "allOf", "prefixItems"] {
        let arr = ex.get(schema, key)?;
        if array(ex, &arr)? {
            if let Some(hit) = violation(ex, &arr, &append_path(path, &key.into()), reject_root)? {
                return Ok(Some(hit));
            }
        }
    }
    Ok(None)
}
fn append_path(path: &WireString, key: &WireString) -> WireString {
    crate::actual_provider::concat(&crate::actual_provider::concat(path, &"/".into()), key)
}
fn known_type(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    heap.boolean(match heap.snapshot(&item)? {
        ValueSnapshot::String(text) => table_has(ex, &text, TYPE_NAMES)?,
        _ => false,
    })
}
fn match_type(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    heap.boolean(matches_type(Exec { heap, runtime }, &captures[0], &callbacks::first(heap, args)?)?)
}
fn const_match_type(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    heap.boolean(matches_type(ex, &ex.get(&captures[0], "const")?, &callbacks::first(heap, args)?)?)
}
fn enum_violation(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let hit = ex.apply(&captures[0], "some", &MATCH_TYPE, &[callbacks::first(heap, args)?])?;
    heap.boolean(!heap.truthy(&hit)?)
}
fn non_object_branch(
    heap: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Exec { heap, runtime };
    let item = callbacks::first(heap, args)?;
    if !object(ex, &item)? {
        return heap.boolean(true);
    }
    let types = declared(ex, &item)?;
    heap.boolean(!heap.truthy(&ex.method(&types, "includes", &[ex.text("object")?])?)?)
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
spec!(NORMALIZE_ITEM, normalize_item, 2, "normalize.ts:1755");
spec!(KNOWN_TYPE, known_type, 0, "strict-tool-validation.ts:60");
spec!(MATCH_TYPE, match_type, 1, "strict-tool-validation.ts:104");
spec!(CONST_MATCH_TYPE, const_match_type, 1, "strict-tool-validation.ts:107");
spec!(ENUM_VIOLATION, enum_violation, 1, "strict-tool-validation.ts:104");
spec!(NON_OBJECT_BRANCH, non_object_branch, 0, "strict-tool-validation.ts:126");
