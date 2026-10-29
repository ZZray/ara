//! Fixed OMP596f2da mcp/errors.ts and tools/tool-errors.ts bodies.
//! MIT; THIRD_PARTY_NOTICES.md. Copyright (c) 2025 Mario Zechner;
//! Copyright (c) 2025-2026 Can Bölük; Copyright (c) 2026 Stencil Labs, Inc.
//! Prepare once in the original Session realm and retain these module bindings.
//! Calls use the calling effective runtime; this module owns no Host runtime.

use crate::native_graph::{get, method, text};
use ara_rpc::{
    WireString,
    value_heap::{
        CompiledFunctionForm, CompiledFunctionSpec, PropertyDescriptor, ValueError, ValueHandle, ValueHeap, ValueKind,
        ValueRuntime, ValueSnapshot,
    },
};
use std::sync::Arc;

type Result<T> = std::result::Result<T, ValueError>;

#[derive(Clone)]
pub struct NativeMcpErrorBindings {
    pub is_record: ValueHandle,
    pub tool_abort_error: ValueHandle,
    pub throw_if_aborted: ValueHandle,
    pub transport_error: ValueHandle,
    pub normalize_transport_error: ValueHandle,
    pub format_failure: ValueHandle,
    pub create_json_rpc_error: ValueHandle,
}

fn nonenumerable(
    heap: &Arc<ValueHeap>,
    object: &ValueHandle,
    key: &str,
    value: ValueHandle,
    rt: &dyn ValueRuntime,
) -> Result<()> {
    heap.define_own(
        object,
        key.into(),
        PropertyDescriptor::Data { value, writable: true, enumerable: false, configurable: true },
        rt,
    )
}

impl NativeMcpErrorBindings {
    /// Error is the already prepared same-realm Native Error constructor.
    /// The actually consumed fixed pi-utils isRecord body is evaluated here.
    pub fn prepare(heap: &Arc<ValueHeap>, environment: ValueHandle, rt: &dyn ValueRuntime) -> Result<Self> {
        heap.adopt(&environment)?;
        let is_record = heap.fresh_compiled_function(&IS_RECORD, &[environment.clone()])?;
        let bindings = heap.object()?;
        heap.define_own_data(&bindings, "isRecord".into(), is_record.clone(), rt)?;
        let error = get(heap, &environment, "Error", rt)?;
        let error_proto = get(heap, &error, "prototype", rt)?;
        heap.define_own_data(&bindings, "ErrorSuper".into(), error.clone(), rt)?;
        let captures = [environment, bindings.clone()];
        let abort_proto = heap.object_with_prototype(Some(&error_proto))?;
        let abort = heap.fresh_native_constructor_with_prototype(&TOOL_ABORT_ERROR, &captures, &abort_proto)?;
        heap.set_prototype(&abort, Some(&error), rt)?;
        nonenumerable(heap, &abort_proto, "constructor", abort.clone(), rt)?;
        heap.define_own_data(&abort, "MESSAGE".into(), text(heap, "Operation aborted")?, rt)?;
        heap.define_own_data(&bindings, "ToolAbortError".into(), abort.clone(), rt)?;
        let transport_proto = heap.object_with_prototype(Some(&error_proto))?;
        let transport =
            heap.fresh_native_constructor_with_prototype(&MCP_TRANSPORT_ERROR, &captures, &transport_proto)?;
        heap.set_prototype(&transport, Some(&error), rt)?;
        nonenumerable(heap, &transport_proto, "constructor", transport.clone(), rt)?;
        heap.define_own_data(&bindings, "MCPTransportError".into(), transport.clone(), rt)?;
        for (key, pattern, flags) in [
            (
                "FETCH_VERBOSE_ADVICE",
                r"\s*For more information, pass `verbose: true` in the second argument to fetch\(\)\.?\s*$",
                "i",
            ),
            (
                "SECRET_KEY",
                r"(?:authorization|bearer|cookie|secret|passw(?:or)?d|pwd|token|credential|api[-_]?key|private[-_]?key|access[-_]?key|signature)",
                "i",
            ),
            ("TRACE_KEYS", r"^(?:trace[-_]?id|request[-_]?id|correlation[-_]?id|traceparent)$", "i"),
        ] {
            heap.define_own_data(&bindings, key.into(), heap.regexp(pattern.into(), flags)?, rt)?;
        }
        let normalize = heap.fresh_compiled_function(&NORMALIZE, &captures)?;
        heap.define_own_data(&bindings, "normalize".into(), normalize.clone(), rt)?;
        Ok(Self {
            is_record,
            tool_abort_error: abort,
            transport_error: transport,
            throw_if_aborted: heap.fresh_compiled_function(&THROW_IF_ABORTED, &captures)?,
            normalize_transport_error: normalize,
            format_failure: heap.fresh_compiled_function(&FORMAT, &captures)?,
            create_json_rpc_error: heap.fresh_compiled_function(&CREATE_JSON_RPC_ERROR, &captures)?,
        })
    }
}

fn is_record(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let value = arg(heap, args, 0)?;
    if !heap.truthy(&value)? || !matches!(heap.kind(&value)?, ValueKind::Object | ValueKind::Array | ValueKind::Null) {
        return heap.boolean(false);
    }
    let array = get(heap, &captures[0], "Array", rt)?;
    let is_array = get(heap, &array, "isArray", rt)?;
    heap.boolean(!heap.truthy(&heap.call(&is_array, &array, &[value], rt)?)?)
}
static IS_RECORD: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da packages/utils/src/type-guards.ts isRecord",
    name: "isRecord",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 1,
    body: is_record,
};

#[derive(Clone, Copy)]
struct Ex<'a> {
    heap: &'a Arc<ValueHeap>,
    captures: &'a [ValueHandle],
    rt: &'a dyn ValueRuntime,
}
impl Ex<'_> {
    fn binding(self, key: &str) -> Result<ValueHandle> {
        get(self.heap, &self.captures[1], key, self.rt)
    }
    fn global(self, key: &str) -> Result<ValueHandle> {
        get(self.heap, &self.captures[0], key, self.rt)
    }
    fn call(self, function: &ValueHandle, args: &[ValueHandle]) -> Result<ValueHandle> {
        self.heap.call(function, &self.heap.undefined()?, args, self.rt)
    }
    fn record(self, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(&self.call(&self.binding("isRecord")?, &[value.clone()])?)
    }
    fn undefined(self, value: &ValueHandle) -> Result<bool> {
        Ok(self.heap.kind(value)? == ValueKind::Undefined)
    }
    fn instance(self, value: &ValueHandle, class: &str) -> Result<bool> {
        self.heap.instance_of(value, &self.binding(class)?, self.rt)
    }
    fn literal_test(self, pattern: &str, flags: &str, value: &ValueHandle) -> Result<bool> {
        self.heap.truthy(&method(
            self.heap,
            &self.heap.regexp(pattern.into(), flags)?,
            "test",
            &[value.clone()],
            self.rt,
        )?)
    }
    fn string_call(self, value: &ValueHandle) -> Result<ValueHandle> {
        self.call(&self.global("String")?, &[value.clone()])
    }
    fn json(self, value: impl FnOnce() -> Result<ValueHandle>) -> Result<ValueHandle> {
        let json = self.global("JSON")?;
        self.invoke(&json, "stringify", || Ok(vec![value()?]))
    }
    fn invoke(
        self,
        receiver: &ValueHandle,
        key: &str,
        arguments: impl FnOnce() -> Result<Vec<ValueHandle>>,
    ) -> Result<ValueHandle> {
        let callable = get(self.heap, receiver, key, self.rt)?;
        self.heap.call(&callable, receiver, &arguments()?, self.rt)
    }
}
fn arg(heap: &Arc<ValueHeap>, args: &[ValueHandle], index: usize) -> Result<ValueHandle> {
    args.get(index).cloned().map(Ok).unwrap_or_else(|| heap.undefined())
}
fn equal(heap: &Arc<ValueHeap>, value: &ValueHandle, expected: &str) -> Result<bool> {
    Ok(matches!(heap.snapshot(value)?, ValueSnapshot::String(value) if value.equals_ascii(expected)))
}
fn concat(ex: Ex<'_>, values: &[ValueHandle]) -> Result<ValueHandle> {
    let mut units = Vec::new();
    for value in values {
        if ex.heap.kind(value)? == ValueKind::Symbol {
            return Err(ValueError::TypeError("Cannot convert a Symbol value to a string".into()));
        }
        units.extend_from_slice(ex.heap.to_string(value, ex.rt)?.units());
    }
    ex.heap.string(WireString::from_units(units))
}
fn field_line(ex: Ex<'_>, prefix: &str, value: ValueHandle) -> Result<ValueHandle> {
    concat(ex, &[text(ex.heap, prefix)?, value])
}

fn sanitize(ex: Ex<'_>, value: ValueHandle, max: f64) -> Result<ValueHandle> {
    let mut value =
        ex.invoke(&value, "replace", || Ok(vec![ex.binding("FETCH_VERBOSE_ADVICE")?, text(ex.heap, "")?]))?;
    for (pattern, replacement) in [
        (r"\b(Bearer|Basic)\s+[^\s,;]+", "$1 [redacted]"),
        (r"([?&](?:access[-_]?token|api[-_]?key|key|token|secret|password)=)[^&#\s]+", "$1[redacted]"),
        (
            r"((?:authorization|api[-_]?key|private[-_]?key|access[-_]?key|token|secret|passw(?:or)?d|pwd|credential)\s*[:=]\s*)[^\s,;}]+",
            "$1[redacted]",
        ),
    ] {
        value = ex.invoke(&value, "replace", || {
            Ok(vec![ex.heap.regexp(pattern.into(), "gi")?, text(ex.heap, replacement)?])
        })?;
    }
    let bounded = method(ex.heap, &value, "slice", &[ex.heap.number(0.0)?, ex.heap.number(max)?], ex.rt)?;
    method(ex.heap, &bounded, "trim", &[], ex.rt)
}

fn class_call(
    _: &Arc<ValueHeap>,
    _: &[ValueHandle],
    _: &ValueHandle,
    _: &[ValueHandle],
    _: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    Err(ValueError::TypeError("Class constructor cannot be invoked without new".into()))
}
fn base_error(ex: Ex<'_>, receiver: &ValueHandle, message: ValueHandle, options: ValueHandle) -> Result<ValueHandle> {
    let error = ex.heap.construct(&ex.binding("ErrorSuper")?, &[message, options], ex.rt)?;
    let prototype = ex.heap.prototype(receiver, ex.rt)?;
    ex.heap.set_prototype(&error, prototype.as_ref(), ex.rt)?;
    Ok(error)
}
fn abort_ctor(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let mut message = arg(heap, args, 0)?;
    if ex.undefined(&message)? {
        message = get(heap, &ex.binding("ToolAbortError")?, "MESSAGE", rt)?;
    }
    let error = base_error(ex, receiver, message, arg(heap, args, 1)?)?;
    heap.assign(&error, "name".into(), text(heap, "ToolAbortError")?, rt)?;
    Ok(error)
}
static TOOL_ABORT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/tool-errors.ts ToolAbortError",
    name: "ToolAbortError",
    length: 0,
    form: CompiledFunctionForm::NativeConstructor(abort_ctor),
    capture_count: 2,
    body: class_call,
};
fn throw_if_aborted(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let signal = arg(heap, args, 0)?;
    if !matches!(heap.kind(&signal)?, ValueKind::Null | ValueKind::Undefined)
        && heap.truthy(&get(heap, &signal, "aborted", rt)?)?
    {
        let reason = get(heap, &signal, "reason", rt)?;
        let reason = if heap.instance_of(&reason, &ex.global("Error")?, rt)? {
            get(heap, &signal, "reason", rt)?
        } else {
            heap.undefined()?
        };
        if ex.instance(&reason, "ToolAbortError")? {
            return Err(ValueError::Thrown(reason));
        }
        let options = heap.object()?;
        heap.define_own_data(&options, "cause".into(), get(heap, &signal, "reason", rt)?, rt)?;
        return Err(ValueError::Thrown(heap.construct(
            &ex.binding("ToolAbortError")?,
            &[heap.undefined()?, options],
            rt,
        )?));
    }
    heap.undefined()
}
static THROW_IF_ABORTED: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da tools/tool-errors.ts throwIfAborted",
    name: "throwIfAborted",
    length: 1,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: throw_if_aborted,
};

fn transport_ctor(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    receiver: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let options = arg(heap, args, 0)?;
    let message = sanitize(ex, get(heap, &options, "message", rt)?, 1_000.0)?;
    let super_options = if ex.undefined(&get(heap, &options, "cause", rt)?)? {
        heap.undefined()?
    } else {
        let cause = heap.object()?;
        heap.define_own_data(&cause, "cause".into(), get(heap, &options, "cause", rt)?, rt)?;
        cause
    };
    let error = base_error(ex, receiver, message, super_options)?;
    heap.assign(&error, "name".into(), text(heap, "MCPTransportError")?, rt)?;
    for key in ["transport", "stage", "failure", "retryable", "code", "data", "traceId"] {
        heap.assign(&error, key.into(), get(heap, &options, key, rt)?, rt)?;
    }
    Ok(error)
}
static MCP_TRANSPORT_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/errors.ts MCPTransportError",
    name: "MCPTransportError",
    length: 1,
    form: CompiledFunctionForm::NativeConstructor(transport_ctor),
    capture_count: 2,
    body: class_call,
};

fn error_code(ex: Ex<'_>, error: &ValueHandle, depth: usize) -> Result<ValueHandle> {
    // Selected Rust stack domain. A native-origin RangeError retains the graph
    // carrier on source catch; this is not a promise of JS engine stack depth.
    if depth > 512 {
        return Err(ValueError::RangeError("MCP errorCode recursion exceeds selected stack domain".into()));
    }
    if !ex.record(error)? {
        return ex.heap.undefined();
    }
    if ex.heap.kind(&get(ex.heap, error, "code", ex.rt)?)? == ValueKind::String
        || ex.heap.kind(&get(ex.heap, error, "code", ex.rt)?)? == ValueKind::Number
    {
        return get(ex.heap, error, "code", ex.rt);
    }
    if ex.undefined(&get(ex.heap, error, "cause", ex.rt)?)? {
        ex.heap.undefined()
    } else {
        error_code(ex, &get(ex.heap, error, "cause", ex.rt)?, depth + 1)
    }
}
fn safe_trace(ex: Ex<'_>, value: ValueHandle) -> Result<ValueHandle> {
    if ex.heap.kind(&value)? != ValueKind::String {
        return ex.heap.undefined();
    }
    let trimmed = method(ex.heap, &value, "trim", &[], ex.rt)?;
    if !ex.heap.truthy(&trimmed)?
        || ex.heap.to_number(&get(ex.heap, &trimmed, "length", ex.rt)?, ex.rt)? > 128.0
        || !ex.literal_test(r"^[A-Za-z0-9._:/-]+$", "", &trimmed)?
    {
        return ex.heap.undefined();
    }
    Ok(trimmed)
}
fn find_trace(ex: Ex<'_>, value: &ValueHandle, depth: usize) -> Result<ValueHandle> {
    if depth > 5 || !ex.record(value)? {
        return ex.heap.undefined();
    }
    for key in ex.heap.for_in_string_keys(value, ex.rt)? {
        let item = ex.heap.get(value, &key.clone().into(), ex.rt)?;
        if ex.heap.truthy(&method(ex.heap, &ex.binding("TRACE_KEYS")?, "test", &[ex.heap.string(key)?], ex.rt)?)? {
            let trace = safe_trace(ex, item.clone())?;
            if ex.heap.truthy(&trace)? {
                return Ok(trace);
            }
        }
        let nested = find_trace(ex, &item, depth + 1)?;
        if ex.heap.truthy(&nested)? {
            return Ok(nested);
        }
    }
    ex.heap.undefined()
}
fn sanitize_data(ex: Ex<'_>, value: ValueHandle, depth: usize, seen: &ValueHandle) -> Result<ValueHandle> {
    if depth > 5 {
        return text(ex.heap, "[truncated]");
    }
    match ex.heap.kind(&value)? {
        ValueKind::String => return sanitize(ex, value, 256.0),
        ValueKind::Number | ValueKind::Boolean | ValueKind::Null => return Ok(value),
        _ => {}
    }
    let array = ex.heap.truthy(&method(ex.heap, &ex.global("Array")?, "isArray", &[value.clone()], ex.rt)?)?;
    if array {
        if ex.heap.truthy(&method(ex.heap, seen, "has", &[value.clone()], ex.rt)?)? {
            return text(ex.heap, "[circular]");
        }
        method(ex.heap, seen, "add", &[value.clone()], ex.rt)?;
        let sliced = method(ex.heap, &value, "slice", &[ex.heap.number(0.0)?, ex.heap.number(30.0)?], ex.rt)?;
        return ex.invoke(&sliced, "map", || {
            let mapper = ex.heap.fresh_compiled_function(
                &SANITIZE_MAP,
                &[ex.captures[0].clone(), ex.captures[1].clone(), ex.heap.number((depth + 1) as f64)?, seen.clone()],
            )?;
            Ok(vec![mapper])
        });
    }
    if !ex.record(&value)? {
        return ex.string_call(&value);
    }
    if ex.heap.truthy(&method(ex.heap, seen, "has", &[value.clone()], ex.rt)?)? {
        return text(ex.heap, "[circular]");
    }
    method(ex.heap, seen, "add", &[value.clone()], ex.rt)?;
    let result = ex.heap.object()?;
    for (count, key) in ex.heap.for_in_string_keys(&value, ex.rt)?.into_iter().enumerate() {
        if count == 30 {
            break;
        }
        let item = ex.heap.get(&value, &key.clone().into(), ex.rt)?;
        let secret = ex.heap.truthy(&method(
            ex.heap,
            &ex.binding("SECRET_KEY")?,
            "test",
            &[ex.heap.string(key.clone())?],
            ex.rt,
        )?)?;
        let sanitized = if secret { text(ex.heap, "[redacted]")? } else { sanitize_data(ex, item, depth + 1, seen)? };
        ex.heap.assign(&result, key.into(), sanitized, ex.rt)?;
    }
    Ok(result)
}
fn sanitize_map(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    sanitize_data(ex, arg(heap, args, 0)?, heap.to_number(&captures[2], rt)? as usize, &captures[3])
}
static SANITIZE_MAP: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/errors.ts sanitizeData item=>sanitizeData(item,depth+1,seen)",
    name: "",
    length: 1,
    form: CompiledFunctionForm::Arrow,
    capture_count: 4,
    body: sanitize_map,
};
fn serialize_data(ex: Ex<'_>, value: ValueHandle) -> Result<ValueHandle> {
    if ex.undefined(&value)? {
        return ex.heap.undefined();
    }
    let operation: Result<ValueHandle> = (|| {
        let serialized = ex.json(|| {
            let seen = ex.heap.construct(&ex.global("WeakSet")?, &[], ex.rt)?;
            sanitize_data(ex, value, 0, &seen)
        })?;
        if ex.heap.to_number(&get(ex.heap, &serialized, "length", ex.rt)?, ex.rt)? <= 2_000.0 {
            return Ok(serialized);
        }
        let mut preview_chars = 1_936.0;
        loop {
            let bounded = ex.json(|| {
                let bounded = ex.heap.object()?;
                ex.heap.define_own_data(&bounded, "truncated".into(), ex.heap.boolean(true)?, ex.rt)?;
                ex.heap.define_own_data(
                    &bounded,
                    "preview".into(),
                    method(
                        ex.heap,
                        &serialized,
                        "slice",
                        &[ex.heap.number(0.0)?, ex.heap.number(preview_chars)?],
                        ex.rt,
                    )?,
                    ex.rt,
                )?;
                Ok(bounded)
            })?;
            let length = ex.heap.to_number(&get(ex.heap, &bounded, "length", ex.rt)?, ex.rt)?;
            if length <= 2_000.0 {
                return Ok(bounded);
            }
            preview_chars -= length - 2_000.0;
        }
    })();
    match operation {
        Err(error) if error.is_native_exception() => ex.heap.undefined(),
        other => other,
    }
}

fn normalize(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let error = arg(heap, args, 0)?;
    let options = arg(heap, args, 1)?;
    if ex.instance(&error, "MCPTransportError")? {
        return Ok(error);
    }
    let raw = if heap.instance_of(&error, &ex.global("Error")?, rt)? {
        get(heap, &error, "message", rt)?
    } else {
        ex.string_call(&error)?
    };
    let message = sanitize(ex, raw, 1_000.0)?;
    let mut code = error_code(ex, &error, 0)?;
    let normalized = if heap.kind(&code)? == ValueKind::String {
        method(heap, &code, "toUpperCase", &[], rt)?
    } else {
        code.clone()
    };
    let http = method(heap, &heap.regexp(r"^HTTP (\d{3}):".into(), "i")?, "exec", &[message.clone()], rt)?;
    let (failure, retryable) = if equal(heap, &normalized, "ECONNREFUSED")?
        || equal(heap, &normalized, "CONNECTIONREFUSED")?
        || equal(heap, &normalized, "ENETUNREACH")?
        || equal(heap, &normalized, "EHOSTUNREACH")?
        || ex.literal_test(r"\b(?:ECONNREFUSED|ConnectionRefused|ENETUNREACH|EHOSTUNREACH)\b", "i", &message)?
    {
        ("connect", true)
    } else if equal(heap, &normalized, "ECONNRESET")? || ex.literal_test(r"\bECONNRESET\b", "i", &message)? {
        ("reset", true)
    } else if equal(heap, &normalized, "EPIPE")?
        || ex.literal_test(r"\b(?:EPIPE|eof|transport closed|socket closed)\b", "i", &message)?
    {
        ("eof", true)
    } else if ex.literal_test(r"timeout|timed out", "i", &message)? {
        ("timeout", false)
    } else if heap.instance_of(&error, &ex.global("SyntaxError")?, rt)?
        || ex.literal_test(
            r"(?:invalid|unexpected end).*(?:json|jsonl)|jsonl? (?:parse|parser) error|malformed",
            "i",
            &message,
        )?
    {
        ("malformed_response", false)
    } else if heap.truthy(&http)? {
        code = ex.call(&ex.global("Number")?, &[get(heap, &http, "1", rt)?])?;
        let retryable = matches!(heap.snapshot(&code)?, ValueSnapshot::Number(404.0 | 502.0 | 503.0));
        ("http_status", retryable)
    } else if ex.literal_test(r"not connected|connection failed|fetch failed", "i", &message)? {
        ("connect", true)
    } else {
        ("unknown", false)
    };
    let prepared = heap.object()?;
    let transport = get(heap, &options, "transport", rt)?;
    let stage = if failure == "connect" {
        text(heap, "connect")?
    } else if failure == "malformed_response" {
        text(heap, "decode")?
    } else {
        get(heap, &options, "stage", rt)?
    };
    for (key, value) in [
        ("transport", transport),
        ("stage", stage),
        ("failure", text(heap, failure)?),
        ("message", if heap.truthy(&message)? { message } else { text(heap, "Unknown MCP transport failure")? }),
        ("retryable", heap.boolean(retryable)?),
        ("code", code),
        ("traceId", safe_trace(ex, get(heap, &options, "traceId", rt)?)?),
        ("cause", error),
    ] {
        heap.define_own_data(&prepared, key.into(), value, rt)?;
    }
    heap.construct(&ex.binding("MCPTransportError")?, &[prepared], rt)
}
static NORMALIZE: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/errors.ts normalizeMCPTransportError",
    name: "normalizeMCPTransportError",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: normalize,
};

fn next_step(ex: Ex<'_>, diagnostic: &ValueHandle) -> Result<ValueHandle> {
    let failure = get(ex.heap, diagnostic, "failure", ex.rt)?;
    let next = if equal(ex.heap, &failure, "connect")? {
        "Check that the MCP server is running and reachable, then retry."
    } else if equal(ex.heap, &failure, "timeout")? {
        "Check server health or increase the MCP timeout; the request outcome is unknown."
    } else if equal(ex.heap, &failure, "eof")? || equal(ex.heap, &failure, "reset")? {
        if equal(ex.heap, &get(ex.heap, diagnostic, "transport", ex.rt)?, "stdio")? {
            "Inspect the MCP subprocess logs, restart the server, then retry."
        } else {
            "Check the MCP server logs and availability, then retry."
        }
    } else if equal(ex.heap, &failure, "malformed_response")? {
        "Inspect the MCP server logs for an invalid JSON-RPC response."
    } else if equal(ex.heap, &failure, "json_rpc")? {
        "Address the server-reported MCP error before retrying."
    } else if equal(ex.heap, &failure, "http_status")? {
        if ex.heap.truthy(&get(ex.heap, diagnostic, "retryable", ex.rt)?)? {
            "Check the MCP server status, then retry."
        } else {
            "Check the MCP endpoint and authentication configuration."
        }
    } else if equal(ex.heap, &failure, "closed")? {
        "Reconnect the MCP server, then retry."
    } else if equal(ex.heap, &failure, "unknown")? {
        "Inspect the MCP server logs and transport configuration."
    } else {
        return ex.heap.undefined();
    };
    text(ex.heap, next)
}
fn format_failure(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let error = arg(heap, args, 0)?;
    let diagnostic = if ex.instance(&error, "MCPTransportError")? {
        error
    } else {
        let options = heap.object()?;
        heap.define_own_data(&options, "transport".into(), text(heap, "unknown")?, rt)?;
        heap.define_own_data(&options, "stage".into(), text(heap, "protocol")?, rt)?;
        ex.call(&ex.binding("normalize")?, &[error, options])?
    };
    let mut lines = vec![
        text(heap, "MCP failure")?,
        field_line(ex, "server: ", sanitize(ex, arg(heap, args, 1)?, 1_000.0)?)?,
        field_line(ex, "tool: ", sanitize(ex, arg(heap, args, 2)?, 1_000.0)?)?,
    ];
    for (prefix, key) in [("transport: ", "transport"), ("stage: ", "stage"), ("failure: ", "failure")] {
        lines.push(field_line(ex, prefix, get(heap, &diagnostic, key, rt)?)?);
    }
    lines.push(field_line(
        ex,
        "retryable: ",
        text(heap, if heap.truthy(&get(heap, &diagnostic, "retryable", rt)?)? { "yes" } else { "no" })?,
    )?);
    lines.push(field_line(ex, "message: ", get(heap, &diagnostic, "message", rt)?)?);
    let lines = heap.array(lines)?;
    for (prefix, key) in [("code: ", "code"), ("trace_id: ", "traceId"), ("data: ", "data")] {
        if !ex.undefined(&get(heap, &diagnostic, key, rt)?)? {
            ex.invoke(&lines, "push", || {
                let value = if key == "code" {
                    let string = ex.global("String")?;
                    ex.call(&string, &[get(heap, &diagnostic, key, rt)?])?
                } else {
                    get(heap, &diagnostic, key, rt)?
                };
                Ok(vec![field_line(ex, prefix, value)?])
            })?;
        }
    }
    ex.invoke(&lines, "push", || Ok(vec![field_line(ex, "next: ", next_step(ex, &diagnostic)?)?]))?;
    method(heap, &lines, "join", &[text(heap, "\n")?], rt)
}
static FORMAT: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/errors.ts formatMCPToolFailure",
    name: "formatMCPToolFailure",
    length: 3,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: format_failure,
};

/// Fixed helper body, exposed for graph-only coarse observations. The current
/// stdio Connection still reduces raw transport errors to bounded String, so
/// this helper does not claim end-to-end preservation of raw error data.
fn create_json_rpc_error(
    heap: &Arc<ValueHeap>,
    captures: &[ValueHandle],
    _: &ValueHandle,
    args: &[ValueHandle],
    rt: &dyn ValueRuntime,
) -> Result<ValueHandle> {
    let ex = Ex { heap, captures, rt };
    let transport = arg(heap, args, 0)?;
    let error = arg(heap, args, 1)?;
    let trace = arg(heap, args, 2)?;
    let options = heap.object()?;
    let message = concat(
        ex,
        &[
            text(heap, "MCP error ")?,
            get(heap, &error, "code", rt)?,
            text(heap, ": ")?,
            get(heap, &error, "message", rt)?,
        ],
    )?;
    let code = get(heap, &error, "code", rt)?;
    let data = serialize_data(ex, get(heap, &error, "data", rt)?)?;
    let safe = safe_trace(ex, trace)?;
    let trace = if matches!(heap.kind(&safe)?, ValueKind::Null | ValueKind::Undefined) {
        find_trace(ex, &get(heap, &error, "data", rt)?, 0)?
    } else {
        safe
    };
    for (key, value) in [
        ("transport", transport),
        ("stage", text(heap, "protocol")?),
        ("failure", text(heap, "json_rpc")?),
        ("message", message),
        ("retryable", heap.boolean(false)?),
        ("code", code),
        ("data", data),
        ("traceId", trace),
    ] {
        heap.define_own_data(&options, key.into(), value, rt)?;
    }
    heap.construct(&ex.binding("MCPTransportError")?, &[options], rt)
}
static CREATE_JSON_RPC_ERROR: CompiledFunctionSpec = CompiledFunctionSpec {
    source: "OMP596f2da mcp/errors.ts createMCPJsonRpcError",
    name: "createMCPJsonRpcError",
    length: 2,
    form: CompiledFunctionForm::Ordinary,
    capture_count: 2,
    body: create_json_rpc_error,
};
