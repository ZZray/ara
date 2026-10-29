//! Fixed OMP596f2da convertTools and sanitizeSchemaForGrammar (MIT).
//! Original tools, methods, callbacks and schema handles remain Native values.
use super::callbacks;
use crate::graph_history::NativeExecution as Ex;
use crate::graph_values::{GraphExecution, GraphValueFactory};
use crate::providers::openai_completions::{CompletionsQuarantinePort, ToolSchemaFlavor, ToolStrictMode};
use crate::reasoning::NativeChatSchemaPolicy;
use ara_rpc::value_heap::{
    CompiledFunctionForm, CompiledFunctionSpec, ValueError, ValueHandle, ValueHeap, ValueKind, ValueRuntime,
    ValueSnapshot,
};
use std::sync::Arc;
type Result<T> = std::result::Result<T, ValueError>;

pub(super) struct ConvertedTools {
    pub tools: ValueHandle,
    pub strict_applied: bool,
}
pub(super) fn convert(
    ex: Ex<'_>,
    tools: &ValueHandle,
    policy: NativeChatSchemaPolicy,
    no_strict: bool,
    mode_override: Option<ToolStrictMode>,
    quarantine: Option<&dyn CompletionsQuarantinePort>,
    factory: &GraphValueFactory,
) -> Result<ConvertedTools> {
    let adapted = ex.apply(
        tools,
        "map",
        &callbacks::TOOL_ADAPT,
        &[
            ex.heap.boolean(no_strict)?,
            ex.heap.boolean(policy.supports_strict_mode)?,
            ex.heap.boolean(policy.reject_root_union)?,
        ],
    )?;
    let requested = mode_override.unwrap_or(policy.strict_mode);
    let mode = match requested {
        ToolStrictMode::None => ToolStrictMode::None,
        ToolStrictMode::AllStrict => {
            if ex.heap.truthy(&ex.apply(&adapted, "every", &callbacks::TOOL_IS_STRICT, &[])?)? {
                ToolStrictMode::AllStrict
            } else {
                ToolStrictMode::None
            }
        }
        ToolStrictMode::Mixed => ToolStrictMode::Mixed,
    };
    let output = ex.array([])?;
    let mut any_strict = false;
    ex.for_of(&adapted, |record| {
        // Native destructuring order is observable on an overridden map result.
        let tool = ex.get(record, "tool")?;
        let base = ex.get(record, "baseParameters")?;
        let parameters = ex.get(record, "parameters")?;
        let strict = ex.get(record, "strict")?;
        let include =
            mode == ToolStrictMode::AllStrict || (mode == ToolStrictMode::Mixed && ex.heap.truthy(&strict)?);
        // The author getter is evaluated before the mode/support predicates.
        let explicit_false = !include
            && matches!(ex.heap.snapshot(&ex.get(&tool, "strict")?)?, ValueSnapshot::Boolean(false))
            && mode == ToolStrictMode::Mixed
            && policy.supports_strict_mode;
        let wire = if include { parameters } else { base };
        let emitted = match policy.flavor {
            ToolSchemaFlavor::None => wire,
            ToolSchemaFlavor::Grammar => grammar(ex, &wire)?,
            ToolSchemaFlavor::MoonshotMfjs => {
                return Err(ValueError::UnsupportedOperation("Fixed moonshot-mfjs Native normalization remains OPEN"));
            }
        };
        let violation = crate::providers::openai_responses::actual::chat_schema_violation(
            GraphExecution::new(ex.heap, ex.runtime),
            &emitted,
            policy.reject_root_union,
        )?;
        if let Some(path) = violation {
            // The fixed warning template evaluates and implicitly converts the
            // original name even without a Host sink. String(Symbol) differs
            // from template ToString, so retain its Symbol TypeError as well.
            let name = ex.heap.to_primitive(&ex.get(&tool, "name")?, true, ex.runtime)?;
            if ex.heap.kind(&name)? == ValueKind::Symbol {
                return Err(ValueError::TypeError("Cannot implicitly convert Symbol to string".into()));
            }
            let name = ex.heap.to_string(&name, ex.runtime)?;
            if let Some(port) = quarantine {
                port.quarantined(&name, &path, factory)?;
            }
            return Ok(true);
        }
        if include {
            any_strict = true;
        }
        let name = ex.get(&tool, "name")?;
        let description = ex.get(&tool, "description")?;
        let description = if ex.heap.truthy(&description)? { description } else { ex.text("")? };
        let function = ex.object([("name", name), ("description", description), ("parameters", emitted)])?;
        if include || explicit_false {
            ex.own(&function, "strict", ex.heap.boolean(include)?)?;
        }
        let projected = ex.object([("type", ex.text("function")?), ("function", function)])?;
        ex.method_lazy(&output, "push", || Ok(vec![projected]))?;
        Ok(true)
    })?;
    Ok(ConvertedTools { strict_applied: ex.length(&output)? > 0.0 && any_strict, tools: output })
}

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
    "unevaluatedItems",
];
fn key_in(key: &ara_rpc::WireString, names: &[&str]) -> bool {
    names.iter().any(|name| key.equals_ascii(name))
}
pub(super) fn grammar(ex: Ex<'_>, source: &ValueHandle) -> Result<ValueHandle> {
    let widening = ex.heap.module_open_subschema_widening()?;
    normalize(ex, source, true, &widening, 0)
}
fn normalize(
    ex: Ex<'_>,
    source: &ValueHandle,
    subschema: bool,
    widening: &ValueHandle,
    depth: usize,
) -> Result<ValueHandle> {
    // Native recursively walks schema slots without memoization. A recursive
    // slot exhausts the source stack rather than becoming serialized JSON.
    if depth >= 128 {
        return Err(ValueError::RangeError("Maximum call stack size exceeded".into()));
    }
    match ex.heap.snapshot(source)? {
        ValueSnapshot::Boolean(true) if subschema => return Ok(widening.clone()),
        ValueSnapshot::Boolean(false) if subschema => return ex.object([("not", widening.clone())]),
        _ => {}
    }
    if ex.heap.kind(source)? == ValueKind::Array {
        return normalize_array(ex, source, subschema, widening, depth);
    }
    if ex.heap.kind(source)? != ValueKind::Object {
        return Ok(source.clone());
    }
    let output = ex.heap.object()?;
    let mut changed = false;
    for key in ex.heap.for_in_string_keys(source, ex.runtime)? {
        if !ex.heap.has_own(source, &key.clone().into(), ex.runtime)? {
            continue;
        }
        let child = ex.heap.get(source, &key.clone().into(), ex.runtime)?;
        let next = if key_in(&key, MAP_KEYS) && ex.heap.kind(&child)? == ValueKind::Object {
            let map = ex.heap.object()?;
            let mut map_changed = false;
            for child_key in ex.heap.for_in_string_keys(&child, ex.runtime)? {
                if !ex.heap.has_own(&child, &child_key.clone().into(), ex.runtime)? {
                    continue;
                }
                let member = ex.heap.get(&child, &child_key.clone().into(), ex.runtime)?;
                let normalized = normalize(ex, &member, true, widening, depth + 1)?;
                if !ex.strict(&normalized, &member)? {
                    map_changed = true;
                }
                ex.heap.assign(&map, child_key.into(), normalized, ex.runtime)?;
            }
            if map_changed { map } else { child.clone() }
        } else if key_in(&key, ARRAY_KEYS) && ex.heap.kind(&child)? == ValueKind::Array {
            normalize_array(ex, &child, true, widening, depth + 1)?
        } else if key_in(&key, VALUE_KEYS)
            || ((key.equals_ascii("additionalProperties") || key.equals_ascii("unevaluatedProperties"))
                && ex.heap.kind(&child)? != ValueKind::Boolean)
        {
            normalize(ex, &child, true, widening, depth + 1)?
        } else {
            child.clone()
        };
        if !ex.strict(&next, &child)? {
            changed = true;
        }
        ex.heap.assign(&output, key.into(), next, ex.runtime)?;
    }
    Ok(if changed { output } else { source.clone() })
}
fn normalize_array(
    ex: Ex<'_>,
    source: &ValueHandle,
    subschema: bool,
    widening: &ValueHandle,
    depth: usize,
) -> Result<ValueHandle> {
    let changed = ex.object([("value", ex.heap.boolean(false)?)])?;
    let result = ex.apply(
        source,
        "map",
        &GRAMMAR_ITEM,
        &[ex.heap.boolean(subschema)?, widening.clone(), changed.clone(), ex.heap.number(depth as f64)?],
    )?;
    Ok(if ex.heap.truthy(&ex.get(&changed, "value")?)? { result } else { source.clone() })
}
fn grammar_item(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    runtime: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, runtime };
    let member = callbacks::first(heap, args)?;
    let depth = heap.to_number(&captures[3], runtime)? as usize;
    let next = normalize(ex, &member, heap.truthy(&captures[0])?, &captures[1], depth + 1)?;
    if !ex.strict(&next, &member)? {
        ex.put(&captures[2], "value", heap.boolean(true)?)?;
    }
    Ok(next)
}
static GRAMMAR_ITEM: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "normalize.ts:1537-1543; 1565-1569",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: grammar_item,
};
