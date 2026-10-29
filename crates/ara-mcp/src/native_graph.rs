//! Graph bodies from OMP596f2da mcp/tool-bridge.ts (MIT; THIRD_PARTY.md).
//! Copyright (c) 2025 Mario Zechner; Copyright (c) 2025-2026 Can Bölük;
//! Copyright (c) 2026 Stencil Labs, Inc.
//! Imported functions are original same-realm bindings, not typed substitutes.

use ara_rpc::{
    WireString,
    value_heap::{
        CapabilityId, CompiledFunctionForm, CompiledFunctionSpec, PropertyKey, RuntimeReply, ValueError, ValueHandle,
        ValueHeap, ValueKind, ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone)]
pub struct NativeMcpImports {
    pub environment: ValueHandle,
    pub format_failure: ValueHandle,
    pub errors: crate::native_errors::NativeMcpErrorBindings,
}
impl NativeMcpImports {
    /// Prepare the actual fixed-source module bindings once per original realm.
    /// Error/Array/WeakSet/JSON/String must already be installed by that realm.
    pub fn prepare(heap: &Arc<ValueHeap>, environment: ValueHandle, rt: &dyn ValueRuntime) -> Result<Self> {
        let errors = crate::native_errors::NativeMcpErrorBindings::prepare(heap, environment.clone(), rt)?;
        Ok(Self { environment, format_failure: errors.format_failure.clone(), errors })
    }
}

fn close_error(
    heap: &Arc<ValueHeap>,
    iterator: &mut ara_rpc::value_heap::IteratorRecord,
    error: ValueError,
    rt: &dyn ValueRuntime,
) -> ValueError {
    match heap.iterator_close(iterator, rt) {
        Ok(()) => error,
        Err(close) if error.is_native_exception() && close.is_native_exception() => error,
        Err(close) => ValueError::IteratorClose { original: Box::new(error), close: Box::new(close) },
    }
}

fn property_key(heap: &Arc<ValueHeap>, value: &ValueHandle, rt: &dyn ValueRuntime) -> Result<PropertyKey> {
    let value = heap.to_primitive(value, true, rt)?;
    if heap.kind(&value)? == ValueKind::Symbol {
        Ok(PropertyKey::Symbol(heap.symbol_key(&value)?))
    } else {
        Ok(PropertyKey::String(heap.to_string(&value, rt)?))
    }
}

pub fn get(heap: &Arc<ValueHeap>, value: &ValueHandle, key: &str, rt: &dyn ValueRuntime) -> Result<ValueHandle> {
    heap.get(value, &key.into(), rt)
}
pub fn method(
    heap: &Arc<ValueHeap>,
    receiver: &ValueHandle,
    key: &str,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let callable = get(heap, receiver, key, rt)?;
    heap.call(&callable, receiver, args, rt)
}
pub fn global_method(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    name: &str,
    key: &str,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    method(heap, &get(heap, &imports.environment, name, rt)?, key, args, rt)
}
pub fn text(heap: &Arc<ValueHeap>, value: &str) -> Result<ValueHandle> {
    heap.string(value.into())
}
fn equal_string(heap: &Arc<ValueHeap>, value: &ValueHandle, expected: &str) -> Result<bool> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::String(s) if s.equals_ascii(expected)))
}
pub fn strict_equal(heap: &Arc<ValueHeap>, a: &ValueHandle, b: &ValueHandle) -> Result<bool> {
    if heap.kind(a)? != heap.kind(b)? {
        return Ok(false);
    }
    match (heap.snapshot(a)?, heap.snapshot(b)?) {
        (ValueSnapshot::Number(a), ValueSnapshot::Number(b)) => Ok(a == b),
        _ => heap.object_is(a, b),
    }
}
fn is_array(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    value: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<bool> {
    heap.truthy(&global_method(heap, imports, "Array", "isArray", &[value.clone()], rt)?)
}
fn has_own(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    value: &ValueHandle,
    key: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<bool> {
    heap.truthy(&global_method(heap, imports, "Object", "hasOwn", &[value.clone(), key.clone()], rt)?)
}
fn unused(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    value: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<bool> {
    if heap.kind(value)? == ValueKind::Undefined || equal_string(heap, value, "")? {
        return Ok(true);
    }
    if !matches!(heap.kind(value)?, ValueKind::Object | ValueKind::Array) || is_array(heap, imports, value, rt)? {
        return Ok(false);
    }
    let keys = global_method(heap, imports, "Object", "keys", &[value.clone()], rt)?;
    strict_equal(heap, &get(heap, &keys, "length", rt)?, &heap.number(0.0)?)
}

/// Executes all Object.entries Gets before the first lazy spread, as upstream.
pub fn normalize_prepare(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    params: &ValueHandle,
    schema: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let mut args = if !matches!(heap.kind(params)?, ValueKind::Object | ValueKind::Array)
        || is_array(heap, imports, params, rt)?
    {
        heap.object()?
    } else {
        params.clone()
    };
    let intent = text(heap, "i")?;
    if has_own(heap, imports, &args, &intent, rt)? {
        let properties = get(heap, schema, "properties", rt)?;
        if !heap.truthy(&properties)? || !has_own(heap, imports, &get(heap, schema, "properties", rt)?, &intent, rt)? {
            // Original object destructuring reads the excluded field first.
            let _ = get(heap, &args, "i", rt)?;
            args = heap.spread_excluding(&args, &["i".into()], rt)?;
        }
    }
    let properties = get(heap, schema, "properties", rt)?;
    if !heap.truthy(&properties)? {
        return Ok(args);
    }
    let required = get(heap, schema, "required", rt)?;
    let required = if matches!(heap.kind(&required)?, ValueKind::Null | ValueKind::Undefined) {
        heap.array(Vec::new())?
    } else {
        required
    };
    let ctor = get(heap, &imports.environment, "Set", rt)?;
    let required = heap.construct(&ctor, &[required], rt)?;
    let entries = global_method(heap, imports, "Object", "entries", &[args.clone()], rt)?;
    let mut iterator = heap.get_iterator(&entries, rt)?;
    let mut cleaned: Option<ValueHandle> = None;
    while let Some(entry) = heap.iterator_next(&mut iterator, rt)? {
        let operation = (|| {
            // Array destructuring observes the entry's original iterator.
            let mut pair = heap.get_iterator(&entry, rt)?;
            let key = heap.iterator_next(&mut pair, rt)?.unwrap_or(heap.undefined()?);
            let value = heap.iterator_next(&mut pair, rt)?.unwrap_or(heap.undefined()?);
            heap.iterator_close(&mut pair, rt)?;
            if heap.truthy(&method(heap, &required, "has", &[key.clone()], rt)?)?
                || !has_own(heap, imports, &properties, &key, rt)?
                || !unused(heap, imports, &value, rt)?
            {
                return Ok(());
            }
            let target = match &cleaned {
                Some(target) => target.clone(),
                None => {
                    let target = heap.spread(&args, rt)?;
                    cleaned = Some(target.clone());
                    target
                }
            };
            let key = property_key(heap, &key, rt)?;
            heap.delete_own(&target, &key, rt)?;
            Ok(())
        })();
        if let Err(error) = operation {
            return Err(close_error(heap, &mut iterator, error, rt));
        }
    }
    Ok(cleaned.unwrap_or(args))
}

/// The host port returns the original authoritative native Promise. The fixed
/// method argument order is id, params, onUpdate, context, signal.
pub fn execute_function(heap: &Arc<ValueHeap>, start_port: &ValueHandle) -> Result<ValueHandle> {
    heap.fresh_compiled_function(&EXECUTE, &[start_port.clone()])
}
static EXECUTE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/tool-bridge.ts MCPTool.execute original async entry",
    name: "execute",
    length: 4,
    form: CompiledFunctionForm::Builtin,
    capture_count: 1,
    body: |heap, captures, receiver, args, rt| {
        let params = args.get(1).cloned().unwrap_or(heap.undefined()?);
        let context = args.get(3).cloned().unwrap_or(heap.undefined()?);
        let signal = args.get(4).cloned().unwrap_or(heap.undefined()?);
        heap.call(&captures[0], &captures[0], &[receiver.clone(), params, context, signal], rt)
    },
};

pub fn rpc_frame(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    id: u64,
    name: &str,
    args: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<Vec<u8>> {
    if id > 9_007_199_254_740_991 {
        return Err(ValueError::RangeError("MCP native request id exceeds Number exact range".into()));
    }
    let params = heap.object()?;
    heap.define_own_data(&params, "name".into(), text(heap, name)?, rt)?;
    heap.define_own_data(&params, "arguments".into(), args.clone(), rt)?;
    let request = heap.object()?;
    for (key, value) in [
        ("jsonrpc", text(heap, "2.0")?),
        ("id", heap.number(id as f64)?),
        ("method", text(heap, "tools/call")?),
        ("params", params),
    ] {
        heap.define_own_data(&request, key.into(), value, rt)?;
    }
    let encoded = global_method(heap, imports, "JSON", "stringify", &[request], rt)?;
    let ValueSnapshot::String(encoded) = heap.snapshot(&encoded)? else {
        return Err(ValueError::TypeError("MCP request JSON.stringify did not return a string".into()));
    };
    let mut bytes = encoded
        .to_utf8()
        .map_err(|_| ValueError::TypeError("MCP request stringify has unpaired output surrogates".into()))?
        .into_bytes();
    bytes.push(b'\n');
    Ok(bytes)
}

fn concat(heap: &Arc<ValueHeap>, values: &[ValueHandle], rt: &dyn ValueRuntime) -> Result<ValueHandle> {
    let mut units = Vec::new();
    for value in values {
        units.extend_from_slice(heap.to_string(value, rt)?.units());
    }
    heap.string(WireString::from_units(units))
}
fn text_block(heap: &Arc<ValueHeap>, value: ValueHandle, rt: &dyn ValueRuntime) -> Result<ValueHandle> {
    let block = heap.object()?;
    heap.define_own_data(&block, "type".into(), text(heap, "text")?, rt)?;
    heap.define_own_data(&block, "text".into(), value, rt)?;
    Ok(block)
}

pub fn build_result(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    result: &ValueHandle,
    source: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let raw_content = get(heap, result, "content", rt)?;
    let mut iter = heap.get_iterator(&raw_content, rt)?;
    let content = heap.array(Vec::new())?;
    let mut buffered = heap.string("".into())?;
    while let Some(item) = heap.iterator_next(&mut iter, rt)? {
        let operation = (|| {
            let kind = get(heap, &item, "type", rt)?;
            if equal_string(heap, &kind, "image")? {
                if heap.truthy(&buffered)? {
                    method(heap, &content, "push", &[text_block(heap, buffered.clone(), rt)?], rt)?;
                    buffered = text(heap, "")?;
                }
                method(heap, &content, "push", &[item], rt)?;
            } else {
                let next = if equal_string(heap, &kind, "text")? {
                    Some(get(heap, &item, "text", rt)?)
                } else if equal_string(heap, &kind, "resource")? {
                    let first = get(heap, &item, "resource", rt)?;
                    let truthy = heap.truthy(&get(heap, &first, "text", rt)?)?;
                    let resource = get(heap, &item, "resource", rt)?;
                    let uri = get(heap, &resource, "uri", rt)?;
                    let mut values = vec![text(heap, "[Resource: ")?, uri, text(heap, "]")?];
                    if truthy {
                        values.extend([text(heap, "\n")?, get(heap, &get(heap, &item, "resource", rt)?, "text", rt)?]);
                    }
                    Some(concat(heap, &values, rt)?)
                } else {
                    None
                };
                if let Some(next) = next {
                    let next =
                        if heap.truthy(&buffered)? { concat(heap, &[text(heap, "\n\n")?, next], rt)? } else { next };
                    buffered = heap.add(&buffered, &next, rt)?;
                }
            }
            Ok(())
        })();
        if let Err(error) = operation {
            return Err(close_error(heap, &mut iter, error, rt));
        }
    }
    if heap.truthy(&buffered)? {
        method(heap, &content, "push", &[text_block(heap, buffered, rt)?], rt)?;
    }
    if heap.array_length(&content, rt)? == 0 {
        method(heap, &content, "push", &[text_block(heap, text(heap, "")?, rt)?], rt)?;
    }
    let details = heap.object()?;
    for (key, value) in [
        ("serverName", get(heap, source, "serverName", rt)?),
        ("mcpToolName", get(heap, source, "mcpToolName", rt)?),
        ("isError", get(heap, result, "isError", rt)?),
        ("rawContent", get(heap, result, "content", rt)?),
        ("mcpMeta", get(heap, result, "_meta", rt)?),
        ("provider", get(heap, source, "provider", rt)?),
        ("providerName", get(heap, source, "providerName", rt)?),
    ] {
        heap.define_own_data(&details, key.into(), value, rt)?;
    }
    if heap.truthy(&get(heap, result, "isError", rt)?)? {
        let first = get(heap, &content, "0", rt)?;
        if equal_string(heap, &get(heap, &first, "type", rt)?, "text")? {
            let prefixed = concat(heap, &[text(heap, "Error: ")?, get(heap, &first, "text", rt)?], rt)?;
            heap.assign(&content, "0".into(), text_block(heap, prefixed, rt)?, rt)?;
        } else {
            method(heap, &content, "unshift", &[text_block(heap, text(heap, "Error:")?, rt)?], rt)?;
        }
    }
    let structured = get(heap, result, "structuredContent", rt)?;
    if heap.kind(&structured)? != ValueKind::Undefined
        && !structured_in_text(heap, imports, &structured, &get(heap, result, "content", rt)?, rt)?
    {
        let rendered =
            match global_method(heap, imports, "JSON", "stringify", &[structured, heap.null()?, heap.number(2.0)?], rt)
            {
                Ok(json) => concat(heap, &[text(heap, "```json\n")?, json, text(heap, "\n```")?], rt)?,
                Err(error) if error.is_native_exception() => text(heap, "")?,
                Err(error) => return Err(error),
            };
        if heap.to_string(&rendered, rt)?.len() > 0 {
            method(heap, &content, "push", &[text_block(heap, rendered, rt)?], rt)?;
        }
    }
    let output = heap.object()?;
    heap.define_own_data(&output, "content".into(), content, rt)?;
    heap.define_own_data(&output, "details".into(), details, rt)?;
    if heap.truthy(&get(heap, result, "isError", rt)?)? {
        heap.define_own_data(&output, "isError".into(), heap.boolean(true)?, rt)?;
    }
    Ok(output)
}
fn structured_in_text(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    structured: &ValueHandle,
    content: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<bool> {
    let mut iter = heap.get_iterator(content, rt)?;
    while let Some(item) = heap.iterator_next(&mut iter, rt)? {
        let operation = (|| {
            if !equal_string(heap, &get(heap, &item, "type", rt)?, "text")? {
                return Ok(false);
            }
            let trimmed = method(heap, &get(heap, &item, "text", rt)?, "trim", &[], rt)?;
            if strict_equal(heap, &get(heap, &trimmed, "length", rt)?, &heap.number(0.0)?)? {
                return Ok(false);
            }
            let parsed = match global_method(heap, imports, "JSON", "parse", &[trimmed], rt) {
                Ok(value) => value,
                Err(error) if error.is_native_exception() => return Ok(false),
                Err(error) => return Err(error),
            };
            heap.truthy(&global_method(heap, imports, "Bun", "deepEquals", &[parsed, structured.clone()], rt)?)
        })();
        match operation {
            Ok(true) => {
                heap.iterator_close(&mut iter, rt)?;
                return Ok(true);
            }
            Ok(false) => {}
            Err(error) => return Err(close_error(heap, &mut iter, error, rt)),
        }
    }
    Ok(false)
}
pub fn build_error(
    heap: &Arc<ValueHeap>,
    imports: &NativeMcpImports,
    reason: &ValueHandle,
    source: &ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let server = get(heap, source, "serverName", rt)?;
    let name = get(heap, source, "mcpToolName", rt)?;
    let rendered =
        heap.call(&imports.format_failure, &heap.undefined()?, &[reason.clone(), server.clone(), name.clone()], rt)?;
    let details = heap.object()?;
    for (key, value) in [
        ("serverName", server),
        ("mcpToolName", name),
        ("isError", heap.boolean(true)?),
        ("provider", get(heap, source, "provider", rt)?),
        ("providerName", get(heap, source, "providerName", rt)?),
    ] {
        heap.define_own_data(&details, key.into(), value, rt)?;
    }
    let output = heap.object()?;
    heap.define_own_data(&output, "content".into(), heap.array(vec![text_block(heap, rendered, rt)?])?, rt)?;
    heap.define_own_data(&output, "details".into(), details, rt)?;
    heap.define_own_data(&output, "isError".into(), heap.boolean(true)?, rt)?;
    Ok(output)
}

pub fn port_result(reply: RuntimeReply) -> Result<ValueHandle> {
    match reply {
        RuntimeReply::Value(value) => Ok(value),
        _ => Err(ValueError::WrongRuntimeReply("MCP port returned non-value")),
    }
}
pub fn port(heap: &Arc<ValueHeap>, id: CapabilityId) -> Result<ValueHandle> {
    heap.function(id)
}
